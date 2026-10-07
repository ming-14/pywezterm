# pywezterm API

Python bindings for the wezterm terminal engine. Four classes: `Terminal` (terminal model), `Pty` (pseudo-terminal),
`Surface` (incremental rendering surface), `ConsoleInput` (Windows console input).

```python
import pywezterm
pywezterm.version()          # '0.1.0'
```

| Class / Function | Responsibility |
|---|---|
| `Terminal` | Pure software terminal: feed bytes → parse VT → query state / snapshot / encode input |
| `Pty`      | Real subprocess + pseudo-terminal (ConPTY / openpty) |
| `Surface`  | Grid → incremental ANSI byte stream |
| `ConsoleInput` | Capture host console key/mouse/resize events (Windows only) |

The library provides **primitives only**: one terminal (`Pty` + `Terminal`), one rendering outlet
(`Surface`), one host event source (`ConsoleInput`). "How multiple terminals are laid out on screen"
and "who receives an event" belong to the caller's UI layer and are out of scope here.

---

## 1. General Conventions

**Coordinates**: All 0-based, `(x=column, y=row)`.

**Cell tuple** (cells returned by `snapshot` / `scrollback` / `logical_lines`):

```python
(col, ch, fg, bg, bold, italic, underline, reverse, strike, width)
# Example: (2, 'c', 'p1', 'default', False, False, False, False, False, 1)
```
- `ch == ""` indicates continuation cell for wide characters (wide characters occupy 2 cells, only the first cell contains the character)
- `width` is display width: CJK/emoji = 2, others = 1

**Color strings**: `"default"` | `"p0"`…`"p15"` (ANSI palette) | `"#rrggbb"`

**Modifier keys** (`mods` parameter, bitwise OR):

```python
SHIFT, ALT, CTRL = 2, 4, 8
t.key_down("c", CTRL)              # -> b'\x03'
t.key_down("a", SHIFT | CTRL)      # -> b'\x01'
```

**Key names**:
`Up Down Left Right Home End Insert Delete PageUp PageDown Backspace Tab Enter Esc Space`,
`F1`…`F24`, or any single character (e.g., `"a"`, `"Z"`).
An invalid key name (multi-character and not a function key, e.g. `"Foo"`, `"F99"`) raises
`ValueError` — the first character is never silently taken.

**Mouse**: `kind ∈ {"press","release","move"}`,
`button ∈ {"left","middle","right","wheel_up","wheel_down","none"}`; invalid values raise `ValueError`.

**Byte destination**: `key_down` / `key_up` / `mouse` directly return encoded bytes for this call;
`send_paste` and terminal-generated responses (DSR, DECACK, etc.) remain in internal buffer,
retrieve with `drain_written()`.

**Exceptions**: all derive from `RuntimeError`, so `except RuntimeError` keeps working.

| Exception | Raised when |
|---|---|
| `pywezterm.TerminalClosed` | operating on a closed terminal |
| `pywezterm.RenderError` | rendering or encoding failed |
| `pywezterm.PlatformUnsupported` | the current platform does not support the capability |
| `ValueError` | invalid argument (key name, mouse value, render size, …) |

---

## 2. Terminal

### 2.1 Offline parsing: feed → read

```python
t = pywezterm.Terminal(cols=80, rows=24, scrollback=10000)

t.feed(b"hello \x1b[31mred\x1b[0m\r\n")   # Feed raw VT bytes

t.text()          # 'hello red'          Visible area plain text, lines joined with \n, trailing whitespace stripped
t.cursor()        # (row, col, visible)  0-based
t.snapshot()      # [[cell, ...], ...]   Each row is a cell list (with styles)
t.snapshot_lines()# [(wrapped, cells)]   wrapped=True indicates this line is continued by next line
t.scrollback()    # History area cell grid (excluding visible area)
t.scrollback_count()

t.resize(120, 30) # Change columns/rows
t.reset()         # RIS: clear screen + clear scrollback + reset
t.clear_scrollback()
```

### 2.2 Incremental reading (for rendering/sync)

```python
base = t.current_seqno()
t.feed(b"more output\r\n")
dirty = t.changed_stable_rows(base)   # Only these stable rows changed
# Only need to redraw dirty rows

t.logical_lines()
# [(first_stable, last_stable, cells), ...]  Reassembles wraps across physical lines into logical lines
```

### 2.3 Input encoding (no write to pty, returns bytes)

```python
t.feed(b"\x1b[?1h")                    # Apply cursor key mode
t.key_down("Up", 0)                    # -> b'\x1bOA' (normal mode would be b'\x1b[A')
t.key_up("Up", 0)                      # -> b'' (xterm mode has no key-up sequence, normal)
t.key_down("c", CTRL)                  # -> b'\x03'

t.feed(b"\x1b[?1000h\x1b[?1006h")      # Enable mouse reporting + SGR
t.mouse(5, 3)                          # -> b'\x1b[<0;6;4M'  (x,y 0-based → sequence 1-based)
t.mouse(5, 3, kind="release")          # Ending 'm'
t.mouse(5, 3, button="wheel_up")

t.feed(b"\x1b[?2004h")                  # Enable bracketed paste
t.send_paste("hi")                     # Automatically wrapped with 200~/201~
t.drain_written()                      # -> b'\x1b[200~hi\x1b[201~', write back to pty
```

### 2.4 Selection (coordinates = stable row + column, spans scrollback, doesn't change with view scrolling)

```python
t.selection_set(anchor_row, anchor_col, end_row, end_col)   # Region (order can be reversed)
t.selection_select_word(row, col)      # Double-click to select word; no selection if on whitespace
t.selection_select_line(row, col)      # Triple-click to select line (including trailing \n)
t.selection_text()                     # 'abc\ndef'
t.selection_active()                   # bool
t.selection_clear()
```

### 2.5 Mode / metadata queries

```python
t.get_keyboard_encoding()   # 'xterm' | 'csi-u' | 'win32' | 'kitty'
t.is_alt_screen_active()    # DECSET 1049
t.is_mouse_grabbed()        # DECSET 1000/1002/1003
t.get_mouse_encoding()      # (mode, sgr)  mode ∈ {0,1000,1002,1003}
t.bracketed_paste_enabled() # DECSET 2004
t.get_title()               # str (OSC 0/2)
t.get_current_dir()         # str | None (OSC 7)
t.get_progress()            # ('none'|'percentage'|'error'|'indeterminate', int|None)
t.get_semantic_zones()      # [(y0, x0, y1, x1, 'prompt'|'input'|'output'), ...]  OSC 133
t.mode_restore_seq()        # str: DECSET sequence that can be fed directly to restore current mode
t.focus_changed(True)       # Report focus (DECSET 1004)
```

### 2.6 Full-screen output

```python
t.render_ansi(include_cursor=True)   # str: Full-screen ANSI (CUP + SGR + \x1b[K)
t.render_scrollback(keep_ansi=False)  # str: History area text / text with SGR
t.render_svg(compression_level=0)     # str: 0=as-is, >=1 compressed
t.render_image(scale=1.0, fmt="png")  # bytes: png | jpg | jpeg | bmp (8x17 pixels/cell × scale)
```

`render_svg` compresses on `&str` boundaries, so CJK text and emoji survive intact.
`render_image`'s `scale` must be a finite positive number producing a side of at most 16384 pixels,
otherwise it raises `ValueError` (never panics); an unrecognized `fmt` is treated as `png`.

### 2.7 Callbacks

```python
t.set_clipboard_callback(lambda sel, data: ...)   # OSC 52, sel ∈ {'clipboard','primary'}, data can be None
t.set_download_callback(lambda name, data: ...)   # OSC 8 / hyperlink download, data: bytes
t.set_device_control_callback(lambda info: ...)   # DCS, info is str
t.set_notification_callback(lambda info: ...)     # Bell/alarm etc., info is str
t.make_all_lines_dirty()                          # Mark all lines changed (full redraw when selection highlight invalid)
```
Callback exceptions are caught and printed, won't interrupt terminal.

### 2.8 Quick reference

```
Terminal(cols=80, rows=24, scrollback=10000)
feed(data) · resize(cols, rows) · reset() · clear_scrollback() · focus_changed(b)
text() · cursor() · snapshot() · snapshot_lines() · scrollback() · scrollback_count()
logical_lines() · current_seqno() · changed_stable_rows(since) · make_all_lines_dirty()
scroll(delta) · scroll_to_bottom()
key_down(key, mods)->bytes · key_up(key, mods)->bytes
mouse(x, y, kind='press', button='left', mods=0)->bytes · send_paste(text) · drain_written()->bytes
selection_set(r0,c0,r1,c1) · selection_select_word(r,c) · selection_select_line(r,c)
selection_text() · selection_active() · selection_clear()
get_keyboard_encoding() · is_alt_screen_active() · is_mouse_grabbed() · get_mouse_encoding()
bracketed_paste_enabled() · get_title() · get_current_dir() · get_progress()
get_semantic_zones() · mode_restore_seq()
render_ansi(include_cursor) · render_scrollback(keep_ansi) · render_svg(level) · render_image(scale, fmt)
set_clipboard_callback(cb) · set_download_callback(cb) · set_device_control_callback(cb) · set_notification_callback(cb)
```

---

## 3. Pty

### 3.1 Manual closed loop (read → feed terminal → write back response)

```python
p = pywezterm.Pty(cols=80, rows=24)
t = pywezterm.Terminal(cols=80, rows=24)
p.spawn([r"C:\Windows\System32\cmd.exe", "/c", "echo hi"])

while True:
    chunk = p.read(4096, timeout=0.2)   # bytes; timeout/EOF -> b''
    if chunk:
        t.feed(chunk)
        resp = t.drain_written()        # Child process DSR etc. queries must be written back, otherwise it will hang
        if resp:
            p.write(resp)
    elif p.try_wait() is not None:      # Exited, continue draining to EOF
        ...
```

### 3.2 Common operations

```python
pid, handle = p.spawn(["/bin/sh", "-c", "sleep 10"],
                      cwd="/tmp",
                      env={"PATH": "/usr/bin"})    # env is override, rest inherited from current process
# Windows and child program parses command line itself (e.g., cmd.exe /c), preserve original quote semantics:
p.spawn([r"C:\Windows\System32\cmd.exe", "/c", "echo \"a b\""],
        raw_cmdline=r'cmd.exe /c echo "a b"')
# Windows: put the child into a job object at creation time (PROC_THREAD_ATTRIBUTE_JOB_LIST).
# Assigning the job after creation instead leaves a window in which whatever the child
# forks first escapes the job.  The caller creates and owns the job handle.
p.spawn([r"C:\Windows\System32\cmd.exe"], job_handle=job)

p.resize(100, 30);  p.get_size()      # (cols, rows)
p.write(b"dir\r\n")
p.child_pid();     p.child_handle()   # Windows process handle
p.hpcon()                             # Windows ConPTY handle
p.try_wait()                          # Exit code | None (running)
p.kill()
p.buffered_bytes()                    # Bytes pending in read buffer
p.close()                             # Idempotent; after close read() always b'', get_size()==(0,0)
```

### 3.3 Quick reference

```
Pty(cols=80, rows=24)
spawn(argv, cwd=None, env=None, raw_cmdline=None, job_handle=None) -> (pid, handle)
read(n=65536, timeout=None) -> bytes · write(data) · resize(cols, rows) · get_size() -> (cols, rows)
try_wait() -> int|None · kill() · close() · buffered_bytes() -> int
child_pid() -> int|None · child_handle() -> int|None · hpcon() -> int|None
```

---

## 4. Surface (grid → incremental ANSI bytes)

```python
s = pywezterm.Surface(cols=80, rows=24)

s.set_cell(0, 0, "Hi", fg="p1", bg="#000000", bold=True)   # Optional styles all default
seq, frame = s.get_changes_bytes(0)      # First frame: full
# ... draw full screen ...
seq, frame = s.get_changes_bytes(seq)    # Afterwards only contains changes; no change then frame == b''

s.repaint_bytes()                        # Force full = get_changes_bytes(0)
s.resize(100, 30)                        # Size change → next frame full
s.clear()                                # Rebuilds the surface: model and change stream both reset
s.dimensions()                           # (cols, rows)
s.current_seqno()
```

Output is TrueColor ANSI; can write directly to real terminal. `set_cell`'s `text` can be multi-character (e.g., `"Hello"`).

`clear()` **rebuilds** the surface (sequence number resets) instead of only pushing a clear-screen
change into the stream: with the latter the model's cells would still be there and the next full
repaint would draw the old content right back.

```
Surface(cols=80, rows=24)
set_cell(x, y, text, fg='default', bg='default', bold=False, italic=False,
         underline=False, reverse=False, strike=False)
get_changes_bytes(since_seqno) -> (seq, bytes) · repaint_bytes() -> (seq, bytes)
resize(cols, rows) · clear() · dimensions() · current_seqno()
```
## 5. ConsoleInput (Windows)

Construction takes over console input/output mode and code page, restored on `restore()` (or object destruction).
Event reading is non-blocking: first `wait_input(ms)` to wait, then `read_inputs()` to get all.

Events come back normalized — the caller sees pywezterm key names and screen coordinates, never a Win32 structure:

```python
ci = pywezterm.ConsoleInput()
try:
    while True:
        if not ci.wait_input(100):
            continue
        for ev in ci.read_inputs():
            if ev[0] == "key":
                _, key, mods, down = ev           # ('key', 'Up', 0, True)
                if down:
                    t.key_down(key, mods)         # encode, then write to the pty yourself
            elif ev[0] == "mouse":
                _, x, y, kind, button, mods = ev  # ('mouse', 12, 4, 'press', 'left', 0)
                t.mouse(x, y, kind, button, mods)
            elif ev[0] == "resize":
                cols, rows = ci.size()            # Get size immediately after ('resize',)
                t.resize(cols, rows)
finally:
    ci.restore()
```

Which terminal a coordinate lands in, and who receives an event, is the caller's decision — the library
only provides the "host event → normalized input" step.

```
ConsoleInput()
wait_input(ms) -> bool · read_inputs() -> list[tuple] · size() -> (cols, rows) · restore()
```

---

## 6. Module functions

```python
pywezterm.version()                       # '0.1.0'
pywezterm.cursor_seq(row, col, visible)   # '\x1b[r+1;c+1H' + '\x1b[?25h' / '\x1b[?25l'
pywezterm.env_info() -> dict              # deployment introspection, see below
pywezterm.clipboard_read()  -> str        # Windows only; returns '' if no content
pywezterm.clipboard_write(text)           # Windows only; empty string is no-op
```

**`env_info()`** turns previously implicit deployment preconditions into queryable facts:

```python
pywezterm.env_info()
# {'module_dir': '.../site-packages/pywezterm',
#  'conpty_dir': '.../site-packages/pywezterm',   # None = sidecar binaries not found
#  'conpty_active': True}                          # False = fell back to system conhost
```

- `module_dir` is derived from the **extension module's own path** (Windows `GetModuleFileNameW`,
  POSIX `dladdr`) — it depends on neither the process CWD nor `__file__`;
- `conpty_active == False` means the sidecar did not take effect and the system conhost is in use.
  Previously this fallback was silent.

**Platform capabilities**: host clipboard and `ConsoleInput` exist on Windows only (the former needs
an X11/Wayland session, the latter depends on the Win32 console). On other platforms those names are
**absent**, rather than present-but-always-failing.

---

## 7. Common recipes

**Terminal emulation without subprocess** (parse logs/test escape sequences)
```python
t = pywezterm.Terminal(120, 40)
t.feed(data)
text, snap, zones = t.text(), t.snapshot(), t.get_semantic_zones()
```

**Subprocess interactive driver**
```python
p, t = pywezterm.Pty(80, 24), pywezterm.Terminal(80, 24)
p.spawn(argv)
def pump():
    b = p.read(65536, timeout=0.1)
    if b:
        t.feed(b); p.write(t.drain_written())
def send(keys):                 # Type
    for k, mods in keys: p.write(t.key_down(k, mods)); p.write(t.key_up(k, mods))
```

**Screenshot / export**
```python
open("shot.png", "wb").write(t.render_image(scale=2, fmt="png"))
open("shot.svg", "w", encoding="utf-8").write(t.render_svg(1))
ansi = t.render_ansi(include_cursor=True)   # Can write directly to real terminal
```

**Full-screen differential rendering**
```python
base = t.current_seqno()
...
for row in t.changed_stable_rows(base):
    ...   # Only redraw these rows
```
