# pywezterm

[简体中文](README.zh.md)

wezterm's core, librarified into a standalone Python extension module: **pseudo-terminal engine** (portable-pty / ConPTY) + **terminal emulator** (wezterm-term) + **incremental rendering** + **image/SVG export**.

Written as Rust bindings (pyo3 / abi3). A pure extension module — no C runtime dependency, no GUI dependency. Any Python program can `import pywezterm` and get a VT state machine that behaves like a real terminal.

```python
import pywezterm, time

p = pywezterm.Pty(cols=80, rows=24)
t = pywezterm.Terminal(cols=80, rows=24)
p.spawn(["python", "-c", "print('hello')"])

deadline = time.time() + 10
while time.time() < deadline:
    chunk = p.read(4096, timeout=0.2)     # releases the GIL; other threads keep running
    if not chunk:
        if p.try_wait() is not None:
            break                          # child exited and nothing is left
        continue
    t.feed(chunk)                          # VT byte stream → terminal model
    resp = t.drain_written()               # responses/encodings the model produced
    if resp:
        p.write(resp)                      # write back, or the child blocks waiting for an answer

print(t.text())                            # plain text of the visible screen
print(t.render_svg(1))                     # the same screen as SVG
p.close()
```

## Contents

- [Install](#install)
- [Build](#build)
- [Capabilities](#capabilities)
- [API reference](#api-reference)
  - [Module-level functions](#module-level-functions)
  - [Pty — pseudo-terminal engine](#pty--pseudo-terminal-engine)
  - [Terminal — terminal emulator](#terminal--terminal-emulator)
  - [Surface — incremental render surface](#surface--incremental-render-surface)
  - [ConsoleInput — Windows console input](#consoleinput--windows-console-input)
- [Core concepts](#core-concepts)
- [Examples](#examples)
- [Platform support](#platform-support)
- [Repository layout](#repository-layout)
- [Development and testing](#development-and-testing)
- [License](#license)

---

## Install

Download the wheel for your platform from [GitHub Releases](https://github.com/ming-14/pywezterm/releases):

```bash
pip install pywezterm-0.1.0-cp38-abi3-win_amd64.whl
```

Built with ABI3 (`abi3-py38`), so **one wheel covers every Python ≥ 3.8**. Published assets:

| Platform | wheel filename suffix |
|---|---|
| Windows x64 / x86 / ARM64 | `win_amd64` / `win32` / `win_arm64` |
| Linux x64 / ARM64 | `manylinux_2_28_x86_64` / `manylinux_2_28_aarch64` |
| macOS Apple Silicon | `macosx_11_0_arm64` |

The Windows x64 wheel ships `conpty.dll` + `OpenConsole.exe` as well (see [ConPTY sideloading](#conpty-sideloading-on-windows-x64)); every other platform contains only the extension module.

## Build

Requirements: a Rust toolchain (rustup), Python, maturin (installed automatically if missing); on Windows also the Visual Studio C++ desktop workload.

```bash
python BUILD.py                              # Release, output in target/wheels/
python BUILD.py --config Debug --rebuild     # Debug, from a clean cache
python BUILD.py --wheel-dir dist             # choose the output directory
python BUILD.py --vcvars "D:\VS\...\vcvars64.bat"   # point at vcvars64.bat manually
```

`BUILD.py` discovers `cargo` (`~/.cargo/bin` → `PATH`), `python`, and `vcvars64.bat` (`vswhere` → common install paths) on its own; `--cargo-dir` / `--python` override that. The equivalent manual build:

```bash
pip install maturin
maturin build --release --out target/wheels
```

---

## Capabilities

| Class / function | Platforms | What it does |
|---|---|---|
| `Pty` | all | ConPTY / Unix PTY: create a pseudo-console, spawn a child, read/write, resize, expose native handles |
| `Terminal` | all | VT/ANSI state machine: feed bytes, screen snapshots, scrollback, key/mouse encoding, selection, mode tracking, SVG/image rendering |
| `Surface` | all | build a frame cell by cell → emit only the changed bytes as incremental ANSI |
| `ConsoleInput` | Windows | normalized console input capture (keys / mouse / resize), saves and restores console modes |
| `clipboard_read` / `clipboard_write` | Windows | clipboard text access (the landing point for OSC 52) |
| `version()` / `cursor_seq()` | all | version string / cursor positioning sequence |

Terminal features handled by wezterm-term: the full CSI/SGR/OSC set, alternate screen and scrollback, wide characters and bidi text, `OSC 0/2` title, `OSC 7` cwd, `OSC 9` progress, `OSC 52` clipboard, `OSC 133` semantic zones, `OSC 8` hyperlink downloads, Kitty keyboard / CSI-u encoding, mouse tracking (1000/1002/1003/1006/1016), bracketed paste, synchronized output (2026), Sixel/iTerm inline images.

---

## API reference

### Module-level functions

```python
pywezterm.version() -> str
```
Version of the bindings.

```python
pywezterm.cursor_seq(row: int, col: int, visible: bool) -> str
```
Cursor positioning sequence. Input is **0-based**, output is a 1-based CUP plus `\x1b[?25h` / `\x1b[?25l`.

```python
pywezterm.clipboard_read() -> str            # Windows; "" when there is no text
pywezterm.clipboard_write(text: str) -> None # Windows
```

### `Pty` — pseudo-terminal engine

Wraps `portable-pty`. Internally an "owner reader thread + buffered queue" model (see [Read buffer and backpressure](#read-buffer-and-backpressure)).

| Method | Notes |
|---|---|
| `Pty(cols=80, rows=24)` | create the pseudo-console; does **not** spawn a child |
| `spawn(argv, cwd=None, env=None, raw_cmdline=None) -> (pid, handle)` | start a child. `env` is `{k: v}`; `handle` is the process handle (always 0 on POSIX) |
| `read(n=65536, timeout=None) -> bytes` | read output. `timeout=None` blocks until data or EOF; otherwise waits at most `timeout` seconds. Returns `b""` on EOF / timeout / after close. Polling **releases the GIL** |
| `write(data: bytes) -> None` | write input. A blocked write **releases the GIL** |
| `resize(cols, rows) -> None` | resize |
| `get_size() -> (cols, rows)` | current size |
| `buffered_bytes() -> int` | bytes sitting in the read buffer waiting for `read` |
| `child_pid() -> int \| None` | child PID |
| `try_wait() -> int \| None` | non-blocking exit code; `None` = still running or never spawned |
| `kill() -> None` | terminate the child |
| `close() -> None` | terminate child + cancel the reader's blocking read + release the pseudo-console. Idempotent; `read()` returns `b""` forever afterwards |
| `hpcon() -> int \| None` | **Windows**: the `HPCON` handle of the underlying ConPTY (for sandboxed / external spawn) |
| `child_handle() -> int \| None` | **Windows**: process handle (for Job Object registration) |

`raw_cmdline` (Windows only): when given, the **entire command line** (program name included) is passed verbatim to the child, bypassing argv quote serialization; the program name is taken from the first token (paths with spaces inside quotes are handled). This is for programs that parse their own command line, like `cmd.exe /c` — the `\"` escaping produced by argv serialization follows C runtime rules and turns into a literal backslash under `cmd.exe`. Other platforms ignore the parameter and use `argv`, so supply both:

```python
import os, pywezterm

comspec = os.environ["COMSPEC"]
p = pywezterm.Pty()
p.spawn([comspec, "/c", 'echo "a b"'],
        raw_cmdline=f"{comspec} /c echo \"a b\"")   # let cmd.exe interpret the quotes
```

### `Terminal` — terminal emulator

A pure state machine: `feed()` takes VT bytes, the getters report screen state; key/mouse input is encoded into bytes destined for the application, and the application's queries are answered into a capture buffer for `drain_written()` to pick up.

**Lifecycle and feeding**

| Method | Notes |
|---|---|
| `Terminal(cols=80, rows=24, scrollback=10000)` | create |
| `feed(data: bytes)` | feed the VT byte stream the program produced (tracks DECSET/SM mode state along the way) |
| `resize(cols, rows)` | resize (anchor-top semantics, identical on every platform) |
| `reset()` | full reset (RIS semantics: erase screen + scrollback + all state) |
| `scrollback_count() -> int` | number of history lines |
| `clear_scrollback()` | drop history (equivalent to `\x1b[3J`) |

**Reading the screen**

| Method | Notes |
|---|---|
| `text() -> str` | plain text of the visible screen (trailing spaces stripped per line, trailing blank lines dropped, `\n` separated) |
| `snapshot() -> list[list[Cell]]` | character grid of the visible region |
| `snapshot_lines() -> list[tuple[bool, list[Cell]]]` | as above, plus `wrapped` (this physical row ends in a wrap and must be joined with the next) |
| `scrollback() -> list[list[Cell]]` | character grid of the history region |
| `logical_lines() -> list[(start_stable, end_stable, cells)]` | logical lines (reflowed by wezterm itself, with over-long-line protection) |
| `cursor() -> (row, col, visible)` | cursor, **0-based**; follows the scroll offset, `visible=False` when scrolled out of view |
| `render_ansi(include_cursor: bool) -> str` | ANSI reconstruction of the visible screen (per-line CUP + SGR, trailing blank lines truncated) |
| `render_scrollback(keep_ansi: bool) -> str` | history region. `False` = plain text; `True` = SGR text + `\r\n` per line (for replaying history into a frontend) |
| `current_seqno() -> int` | current sequence number (incremented per `feed`), the baseline for dirty-row diffing |
| `changed_stable_rows(since_seqno) -> list[int]` | stable rows that changed since `since_seqno` (visible area + history) |

The `Cell` tuple has 10 items:

```python
(col, char, fg, bg, bold, italic, underline, reverse, strike, width)
#  colors are "default" | "p<index>" | "#rrggbb"; width is the display width (2 for wide chars)
```
`col` is the cell's real column index: the padding cells skipped after a wide character never appear, so **column numbers jump**; renderers must advance by `width`.

**Scrolling**

`scroll(delta)` (`delta>0` scrolls up into older content, `<0` comes back down, clamped), `scroll_to_bottom()` (resume following live output). Every reader (`text` / `snapshot` / `cursor` / `render_*`) reflects the current view offset.

**Input encoding**

| Method | Notes |
|---|---|
| `key_down(key, mods) -> bytes` | encode a key press; returns the bytes to write to the pty |
| `key_up(key, mods) -> bytes` | encode a key release |
| `mouse(x, y, kind="press", button="left", mods=0) -> bytes` | encode a mouse event, and record it in the model (applications react accordingly) |
| `send_paste(text) -> None` | mode-aware paste (wrapped automatically when bracketed paste is on); the bytes land in the capture buffer — take them with `drain_written()` |
| `focus_changed(focused: bool)` | report focus (pairs with DECSET 1004) |
| `drain_written() -> bytes` | take everything the terminal produced: key encodings **and answers to application queries** |

> **`Terminal` only encodes; it never writes a pty** — it does not know whether a pty exists. The caller must write the returned bytes down.

- `key`: `Up` `Down` `Left` `Right` `Home` `End` `Insert` `Delete` `PageUp` `PageDown` `Backspace` `Tab` `Enter` `Esc` `Space` `F1`–`F24`, or any single character.
- `mods`: `KeyModifiers` bit flags — `SHIFT=2`, `ALT=4`, `CTRL=8`.
- `kind`: `press` / `release` / `move`; `button`: `left` / `middle` / `right` / `wheel_up` / `wheel_down` / `none`.

**Modes and state**

| Method | Notes |
|---|---|
| `is_mouse_grabbed() -> bool` | whether the app grabbed the mouse (1000/1002/1003) |
| `get_mouse_encoding() -> (mode, sgr)` | the exact mode number (0/1000/1002/1003) and whether 1006 is on |
| `get_keyboard_encoding() -> str` | `xterm` / `csi-u` / `win32` / `kitty` |
| `is_alt_screen_active()` / `bracketed_paste_enabled()` | alternate screen / paste mode |
| `mode_restore_seq() -> str` | ANSI sequence restoring the tracked mode state, see [mode tracking](#mode-tracking-and-mode_restore_seq) |
| `get_title() -> str` | `OSC 0/2` |
| `get_current_dir() -> str \| None` | `OSC 7` |
| `get_progress() -> (label, percent)` | `OSC 9`; label ∈ `none`/`percentage`/`error`/`indeterminate` |
| `get_semantic_zones() -> list[(y0,x0,y1,x1,type)]` | `OSC 133` prompt/input/output zones |

**Selection**

`selection_set(anchor_row, anchor_col, end_row, end_col)` (stable coordinates), `selection_select_word(row, col)` (double click), `selection_select_line(row, col)` (triple click), `selection_text() -> str`, `selection_active() -> bool`, `selection_clear()`, `make_all_lines_dirty()` (force a full invalidation after the selection changes).

**Callbacks** (things the application initiates, handed back to Python)

| Method | Callback signature |
|---|---|
| `set_clipboard_callback(cb)` | `(selection: str, content: str \| None) -> None` —— OSC 52 |
| `set_download_callback(cb)` | `(name: str \| None, data: bytes) -> None` —— OSC 8 download |
| `set_device_control_callback(cb)` | DCS sequences |
| `set_notification_callback(cb)` | Alert: bell / title / progress etc. |

> Callbacks should **only** perform their side effect (write the clipboard, set an event). Never query terminal state from inside one — the callback can contend with the reader thread for the same lock and deadlock.

**Graphics export**

| Method | Notes |
|---|---|
| `render_svg(compression_level: int) -> str` | SVG of the visible screen. `0` = as-is; `>=1` compresses (drop empty text nodes, collapse whitespace between tags) |
| `render_image(scale: float, fmt: str) -> bytes` | image bytes. `fmt` ∈ `png`/`jpg`/`jpeg`/`bmp`; `scale` multiplies cell pixels (`1.0` standard, `2.0` hi-dpi) |

Rasterization is pure Rust (fontdb for discovery, fontdue for glyphs, tiny-skia for compositing, image for encoding) — no GUI library involved. Same viewpoint as `snapshot()`.

### `Surface` — incremental render surface

Draw cells yourself, spawn nothing, and get only the changes back as ANSI bytes. Good for putting non-terminal sources (tables, dashboards, generated content) onto a real terminal or a frontend.

```python
s = pywezterm.Surface(80, 24)
s.set_cell(0, 0, "Hello", "red", "default", bold=True)   # x, y, text, fg, bg, ...
seqno, data = s.get_changes_bytes(0)                      # first frame is full
seqno, data = s.get_changes_bytes(seqno)                  # b"" = nothing changed
```

| Method | Notes |
|---|---|
| `Surface(cols=80, rows=24)` | create (matching the host's real size) |
| `set_cell(x, y, text, fg="default", bg="default", bold=False, italic=False, underline=False, reverse=False, strike=False)` | write one cell, 0-based |
| `get_changes_bytes(since_seqno) -> (seqno, bytes)` | changes since `since_seqno`. If the baseline is too old or exceeds the budget it falls back to a full repaint |
| `repaint_bytes() -> (seqno, bytes)` | force a full frame |
| `current_seqno()` / `dimensions()` / `resize(cols, rows)` / `clear()` | baseline / size / resize (a size change discards buffered changes, next frame is full) / clear |

### `ConsoleInput` — Windows console input

Moves console input capture for a host that owns a real console into the bindings, replacing hand-rolled ctypes Win32. Construction sets the console modes (VT processing on, wrap auto-CR off for output; raw key events for input) and **saves the original modes**; `restore()` or dropping the object puts them back. Constructing when stdio is redirected (not a console) fails.

```python
ci = pywezterm.ConsoleInput()
try:
    if ci.wait_input(100):                # wait for input events, False = timeout
        for ev in ci.read_inputs():       # drain every pending record at once
            print(ev)
finally:
    ci.restore()
```

`read_inputs()` returns normalized tuples:

```python
("key", key, mods, down)                      # key names as in Terminal.key_down; Ctrl+letter normalized to the letter + CTRL bit; bare modifier keys ignored
("mouse", x, y, kind, button, mods)           # release events get the last_pressed button filled in
("resize",)
```

`size() -> (cols, rows)` returns the current logical window size.

---

## Core concepts

### The closed loop: answers must be written back

Children constantly query the terminal (`\x1b[6n` cursor position, `\x1b]10;?` color, Kitty keyboard capability queries, …). `Terminal` answers into a **capture buffer**, and draining it is your job:

```python
t.feed(chunk)                 # 1. feed output into the model
resp = t.drain_written()      # 2. take the answers/encodings
if resp: p.write(resp)        # 3. write them back to the pty — otherwise the child waits forever
```

There is no helper that closes the loop for you: using `Pty` + `Terminal` means doing all three yourself.

### Read buffer and backpressure

`Pty`'s reader thread keeps pulling from the pipe into a queue. That queue has **high/low watermarks** (1 MiB / 256 KiB): once the high watermark is reached it **stops** reading from the PTY, so the pipe fills up and the child's `write` blocks — the only way backpressure actually reaches the child. Without a bound the reader drains the pipe and piles data up here forever; throttling at the consumer level only stops the consumer.

`buffered_bytes()` makes the invariant observable. Once Python's `read()` takes data out, a reader parked at the high watermark is woken automatically.

### GIL

`read()` and `write()` both poll/block with the **GIL released**. When the PTY is full (the child isn't reading input, and it's waiting for your answer to a query), a blocked write holding the lock freezes the entire Python process — the asyncio event loop included. Running the write on another thread does not help; that is precisely what releasing the GIL buys you.

### Mode tracking and `mode_restore_seq()`

`feed()` scans the mode sequences that reach it (`CSI ? Pm h/l` and `CSI Pm h/l`) and tracks DECSET 1 / 6 / 7 / 12 / 25 / 45 / 47 / 66 / 1000 / 1002 / 1003 / 1004 / 1006 / 1016 / 1047 / 1049 / 2004 plus SM 4 / 20. `mode_restore_seq()` emits the sequences that restore that state — used when a client (xterm.js and friends) reconnects or subscribes and needs the screen brought in line.

Three things worth knowing:

- **Every mode field is an `Option`.** `None` means the application never set it, so restore does **not** emit that sequence and the client keeps its own default. Storing a `bool` instead would overwrite client settings with *our* idea of the default (cursor blinking being the obvious case).
- We only see sequences that **reach us**. Windows/ConPTY swallows some (mouse modes, DECCKM, cursor visibility, origin mode, alternate screen) — the application set them and we have no way to know, so they cannot be re-emitted either. That is an environment limitation, not an implementation gap.
- `?2026` (synchronized output) is deliberately **not** restored: it is a per-frame transient mode, and turning it on leaves the client buffering renders forever — the picture freezes on the last frame.

### ConPTY sideloading on Windows x64

The Windows x64 wheel carries `conpty.dll` + `OpenConsole.exe` (the conhost shipped by wezterm). On the first `Pty` creation the package directory is registered as the sideload directory, so portable-pty prefers wezterm's OpenConsole host over the system conhost — matching wezterm's behavior and avoiding several system-conhost quirks. Windows ARM64 / i386 and all other platforms ship no such binaries (they cannot load across architectures) and use the in-kernel ConPTY.

---

## Examples

### Run a command headless and read the screen back

```python
import os, time, pywezterm


def run(argv, cols=80, rows=24, timeout=10.0):
    """Run one command headlessly; return the screen text after it exits."""
    p = pywezterm.Pty(cols, rows)
    t = pywezterm.Terminal(cols, rows)
    p.spawn(argv)
    deadline = time.time() + timeout
    try:
        while time.time() < deadline:
            chunk = p.read(4096, timeout=0.2)
            if chunk:
                t.feed(chunk)
                resp = t.drain_written()   # the child queried the terminal; answer it
                if resp:
                    p.write(resp)
            elif p.try_wait() is not None:
                while True:                # after exit, drain the tail (keep feeding)
                    tail = p.read(4096, timeout=0.3)
                    if not tail:
                        break
                    t.feed(tail)
                break
        return t.text()
    finally:
        p.close()


print(run([os.environ.get("COMSPEC", "cmd.exe"), "/c", "ver"]))
```

### Several terminals in one loop

The library hands you one terminal per `Pty` + `Terminal` pair. How many you run, and where their
pictures land on screen, is the application's call — `Terminal.render_ansi()` (full frame) and
`Surface` (incremental frame) are the output primitives to build that on.

```python
import os, pywezterm


def shell():
    return ["/bin/sh"] if os.name == "posix" else [os.environ.get("COMSPEC", "cmd.exe")]


pairs = []
for _ in range(2):
    p, t = pywezterm.Pty(60, 24), pywezterm.Terminal(60, 24)
    p.spawn(shell())
    pairs.append((p, t))

try:
    while True:
        for p, t in pairs:
            chunk = p.read(4096, timeout=0.05)          # non-blocking poll
            if chunk:
                t.feed(chunk)
                resp = t.drain_written()                # answer the child's queries
                if resp:
                    p.write(resp)
        frames = [t.render_ansi(include_cursor=True) for _, t in pairs]
        # frames[i] is terminal i's picture; place them however your UI wants
finally:
    for p, _ in pairs:
        p.close()
```

### Export the terminal picture

```python
import pywezterm

t = pywezterm.Terminal(80, 24)
t.feed(open("capture.bin", "rb").read())

open("term.svg", "w", encoding="utf-8").write(t.render_svg(1))
open("term.png", "wb").write(t.render_image(2.0, "png"))
print(t.render_scrollback(keep_ansi=False))   # plain text, history included
```

### OSC 52 into the system clipboard (Windows)

```python
import pywezterm

t = pywezterm.Terminal(80, 24)
t.set_clipboard_callback(lambda sel, content: content and pywezterm.clipboard_write(content))
t.feed(b"\x1b]52;c;aGVsbG8=\x1b\\")
print(pywezterm.clipboard_read())      # 'hello'
```

---

## Platform support

| Platform | Architecture | Notes |
|---|---|---|
| Windows | x64 | full support; wheel includes the sideloaded ConPTY binaries |
| Windows | x86 (i386) | supported (the 32-bit ConPTY calling-convention stack corruption is fixed) |
| Windows | ARM64 | supported; uses the in-kernel ConPTY |
| Linux | x64 / ARM64 | supported; CI builds inside a `manylinux_2_28` container, so glibc 2.28+ works |
| macOS | Apple Silicon | supported, prebuilt wheel from CI |
| macOS | Intel x64 | supported by the code, build locally (no Intel runner in CI) |

Python ≥ 3.8, one ABI3 wheel each. CI covers every buildable combination above and runs `pytest`; pushing a `v*` tag creates or updates the GitHub Release with `SHA256SUMS.txt` attached.

---

## Repository layout

```
pywezterm/                 Python package shim (from .pywezterm import *)
BUILD.py                   cross-platform build script
pyproject.toml             maturin config + Windows binary packaging rules
AGENTS.md                  development constraints + log of upstream wezterm changes
assets/windows/conhost/    sideloaded conpty.dll + OpenConsole.exe
tests/                     library-level self-tests (pytest)
wezterm/                   vendored wezterm core crates (upstream sources + this project's bindings)
  pywezterm-core/          ← domain layer (no pyo3): pty, emulator model, rendering
    src/
      error.rs          unified error type
      env.rs            module's own asset location + deployment introspection
      input.rs          input event vocabulary (platform produces, terminal consumes)
      term/             grid (Cell/Color/Attrs) · model · view · encode · selection
      render/           ansi · surface (incremental) · svg · pixmap · font
      host/             Pane (pty + model + reader + backpressure + close)
      platform/         windows/ · posix/ — same interface, per-platform implementation
  pywezterm/               ← binding shell only
    Cargo.toml  build.rs
    src/
      lib.rs            module registration
      py/               signatures, defaults, type conversion, GIL, error mapping
  term/ pty/ termwiz/ vtparse/ bidi/ wezterm-surface/ ...   upstream crates
```

The domain layer does not depend on `pyo3` — writing `use pyo3::` there fails to compile, so the
layering is enforced by the compiler rather than by convention. See `ARCHITECTURE.md`.

The upstream crates under `wezterm/` are **not** modified by default; when a change really is required (say a genuine wezterm bug), it gets recorded in `AGENTS.md`.

## Development and testing

```bash
python BUILD.py                                   # build the wheel
python -m pip install --force-reinstall target/wheels/*.whl
python -m pytest tests/ -v
```

In CI the tests run against the installed wheel, so the source `pywezterm/` directory is removed there — otherwise that shell containing nothing but `import *` shadows the installed package. When testing a **locally built** extension, drop the freshly built `pywezterm.pyd` into `pywezterm/` instead: `pytest.ini` (`pythonpath = .`) puts the repository root first on `sys.path`, so the local build wins over any other copy of the package that happens to be installed.

`tests/` is split by capability: `test_pty` (pseudo-terminal + closed loop), `test_term` / `test_stage1_state` (VT state and modes), `test_stage2_render` / `test_surface_render` (rendering), `test_selection`, `test_console_input`, `test_edge`, `test_refactor_invariants` (chunk invariance, failure paths, idempotence), `test_blackbox_comprehensive` (public API surface contract).

Rust-side unit tests live in `pywezterm-core` and run without a Python interpreter:

```bash
cargo test -p pywezterm-core          # 94 tests
```

The Rust side has its own unit tests: `cargo test --manifest-path wezterm/pywezterm/Cargo.toml`.

## License

MIT. The `pywezterm` bindings belong to this project; the vendored upstream crates under `wezterm/` remain copyright of their respective authors and are MIT as well (see `wezterm/LICENSE.md`).
