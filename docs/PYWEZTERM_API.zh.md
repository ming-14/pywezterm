# pywezterm API

wezterm 终端引擎的 Python 绑定。四个类：`Terminal`（终端模型）、`Pty`（伪终端）、
`Surface`（增量渲染表面）、`ConsoleInput`（Windows 控制台输入）。

```python
import pywezterm
pywezterm.version()          # '0.1.0'
```

| 类 / 函数 | 职责 |
|---|---|
| `Terminal` | 纯软件终端：喂字节 → 解析 VT → 查询状态 / 快照 / 编码输入 |
| `Pty`      | 真实子进程 + 伪终端（ConPTY / openpty） |
| `Surface`  | 网格 → 增量 ANSI 字节流 |
| `ConsoleInput` | 采集宿主控制台的键鼠/resize 事件（仅 Windows） |

本库只提供**原语**：一个终端（`Pty` + `Terminal`）、一个渲染出口（`Surface`）、
一个宿主事件源（`ConsoleInput`）。「多个终端怎么摆在屏幕上」「事件发给谁」属于调用方的
UI 层，不在本库范围内。

---

## 1. 通用约定

**坐标**：一律 0-based，`(x=列, y=行)`。

**Cell 元组**（`snapshot` / `scrollback` / `logical_lines` 返回的单元格）：

```python
(col, ch, fg, bg, bold, italic, underline, reverse, strike, width)
# 例: (2, 'c', 'p1', 'default', False, False, False, False, False, 1)
```
- `col` 取单元格的真实列索引：宽字符覆盖的那一格**不会**出现，因此列号会跳位（`0, 2, 4, …`）。渲染时按 `width` 推进，而不是按 1
- `width` 为显示宽度：CJK/emoji = 2，其余 = 1

**颜色字符串**：`"default"` | `"p0"`…`"p15"`（ANSI 调色板） | `"#rrggbb"`

**修饰键**（`mods` 参数，按位或）：

```python
SHIFT, ALT, CTRL = 2, 4, 8
t.key_down("c", CTRL)              # -> b'\x03'
t.key_down("a", SHIFT | CTRL)      # -> b'\x01'
```

**键名**：
`Up Down Left Right Home End Insert Delete PageUp PageDown Backspace Tab Enter Esc Space`、
`F1`…`F24`、或任意单个字符（如 `"a"`、`"Z"`）。
非法键名（多字符且不是功能键名，如 `"Foo"`、`"F99"`）抛 `ValueError` —— 不会悄悄取首字符。

**鼠标**：`kind ∈ {"press","release","move"}`，
`button ∈ {"left","middle","right","wheel_up","wheel_down","none"}`；非法取值抛 `ValueError`。

**字节去向**：`key_down` / `key_up` / `mouse` 直接返回本次编码字节；
`send_paste` 与终端**自发**产生的应答（DSR、DECACK 等）留在内部缓冲，
用 `drain_written()` 统一取出。

**异常**：都继承 `RuntimeError`，因此 `except RuntimeError` 仍然有效。

| 异常 | 触发 |
|---|---|
| `pywezterm.TerminalClosed` | 对已关闭的终端操作 |
| `pywezterm.RenderError` | 渲染或编码失败 |
| `pywezterm.PlatformUnsupported` | 当前平台不支持该能力 |
| `ValueError` | 参数非法（键名、鼠标取值、渲染尺寸等） |

---

## 2. Terminal

### 2.1 离线解析：喂入 → 读取

```python
t = pywezterm.Terminal(cols=80, rows=24, scrollback=10000)

t.feed(b"hello \x1b[31mred\x1b[0m\r\n")   # 喂入原始 VT 字节

t.text()          # 'hello red'          可见区纯文本，行以 \n 连接，去行尾空白
t.cursor()        # (row, col, visible)  0-based
t.snapshot()      # [[cell, ...], ...]   每行一个 cell 列表（含样式）
t.snapshot_lines()# [(wrapped, cells)]   wrapped=True 表示该行被下一行续写
t.scrollback()    # 历史区 cell 网格（不含可见区）
t.scrollback_count()

t.resize(120, 30) # 改行列
t.reset()         # RIS：清屏 + 清 scrollback + 复位
t.clear_scrollback()
```

### 2.2 增量读取（渲染/同步用）

```python
base = t.current_seqno()
t.feed(b"more output\r\n")
dirty = t.changed_stable_rows(base)   # 只变了这些稳定行
# 只重画 dirty 行即可

t.logical_lines()
# [(first_stable, last_stable, cells), ...]  已把跨物理行的 wrap 重组成逻辑行
```

### 2.3 输入编码（不写 pty，返回字节）

```python
t.feed(b"\x1b[?1h")                    # 应用光标键模式
t.key_down("Up", 0)                    # -> b'\x1bOA'（普通模式则为 b'\x1b[A'）
t.key_up("Up", 0)                      # -> b''（xterm 模式无抬键序列，正常）
t.key_down("c", CTRL)                  # -> b'\x03'

t.feed(b"\x1b[?1000h\x1b[?1006h")      # 开鼠标上报 + SGR
t.mouse(5, 3)                          # -> b'\x1b[<0;6;4M'  (x,y 0-based → 序列 1-based)
t.mouse(5, 3, kind="release")          # 尾部 'm'
t.mouse(5, 3, button="wheel_up")

t.feed(b"\x1b[?2004h")                  # 开 bracketed paste
t.send_paste("hi")                     # 自动包 200~/201~
t.drain_written()                      # -> b'\x1b[200~hi\x1b[201~'，写回 pty 即可
```

### 2.4 选区（坐标 = stable 行 + 列，跨 scrollback、不随视图滚动变化）

```python
t.selection_set(anchor_row, anchor_col, end_row, end_col)   # 区域（顺序可反）
t.selection_select_word(row, col)      # 双击选词；空白处则无选区
t.selection_select_line(row, col)      # 三击选行（含结尾 \n）
t.selection_text()                     # 'abc\ndef'
t.selection_active()                   # bool
t.selection_clear()
```

### 2.5 模式 / 元数据查询

```python
t.get_keyboard_encoding()   # 'xterm' | 'csi-u' | 'win32' | 'kitty'
t.is_alt_screen_active()    # DECSET 1049
t.is_mouse_grabbed()        # DECSET 1000/1002/1003
t.get_mouse_encoding()      # (mode, sgr)  mode ∈ {0,1000,1002,1003}
t.bracketed_paste_enabled() # DECSET 2004
t.get_title()               # str（OSC 0/2）
t.get_current_dir()         # str | None（OSC 7）
t.get_progress()            # ('none'|'percentage'|'error'|'indeterminate', int|None)
t.get_semantic_zones()      # [(y0, x0, y1, x1, 'prompt'|'input'|'output'), ...]  OSC 133
t.mode_restore_seq()        # str：一段可直接喂入、还原当前模式的 DECSET 序列
t.focus_changed(True)       # 上报焦点（DECSET 1004）
```

### 2.6 整屏输出

```python
t.render_ansi(include_cursor=True)   # str：全屏 ANSI（CUP + SGR + \x1b[K）
t.render_scrollback(keep_ansi=False)  # str：历史区文本 / 带 SGR 文本
t.render_svg(compression_level=0)     # str：0=原样，>=1 压缩（非 ASCII 文本不会被破坏）
t.render_image(scale=1.0, fmt="png")  # bytes：png | jpg | jpeg | bmp（8x17 像素/格 × scale）
```

`render_svg` 的压缩按 `&str` 边界处理，中文与 emoji 原样保留。
`render_image` 的 `scale` 必须是有限正数且结果尺寸在 16384 像素以内，否则抛 `ValueError`
（不会 panic）；无法识别的 `fmt` 按 `png` 处理。

### 2.7 回调

```python
t.set_clipboard_callback(lambda sel, data: ...)   # OSC 52，sel ∈ {'clipboard','primary'}, data 可为 None
t.set_download_callback(lambda name, data: ...)   # OSC 8 / 超链接下载，data: bytes
t.set_device_control_callback(lambda info: ...)   # DCS，info 为 str
t.set_notification_callback(lambda info: ...)     # 响铃/报警等，info 为 str
t.make_all_lines_dirty()                          # 标记全部行变更（选区高亮失效时全量重绘）
```
回调异常被捕获并打印，不会中断终端。

### 2.8 速查

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

### 3.1 手动闭环（读 → 喂终端 → 应答回写）

```python
p = pywezterm.Pty(cols=80, rows=24)
t = pywezterm.Terminal(cols=80, rows=24)
p.spawn([r"C:\Windows\System32\cmd.exe", "/c", "echo hi"])

while True:
    chunk = p.read(4096, timeout=0.2)   # bytes；timeout 到期/EOF -> b''
    if chunk:
        t.feed(chunk)
        resp = t.drain_written()        # 子进程的 DSR 等查询必须回写，否则它会卡住
        if resp:
            p.write(resp)
    elif p.try_wait() is not None:      # 已退出，继续排空到 EOF
        ...
```

### 3.2 常用操作

```python
pid, handle = p.spawn(["/bin/sh", "-c", "sleep 10"],
                      cwd="/tmp",
                      env={"PATH": "/usr/bin"})    # env 为覆盖，其余继承当前进程
# Windows 且子程序自行解析命令行（如 cmd.exe /c）时保留原始引号语义：
p.spawn([r"C:\Windows\System32\cmd.exe", "/c", "echo \"a b\""],
        raw_cmdline=r'cmd.exe /c echo "a b"')
# Windows：创建时即把子进程放进作业对象（PROC_THREAD_ATTRIBUTE_JOB_LIST）。
# 创建后再赋值有时间窗——子进程在此期间先 fork 出的进程会逃出作业。
# 作业句柄由调用方创建并持有。
p.spawn([r"C:\Windows\System32\cmd.exe"], job_handle=job)

p.resize(100, 30);  p.get_size()      # (cols, rows)
p.write(b"dir\r\n")
p.child_pid();     p.child_handle()   # Windows 进程句柄
p.hpcon()                             # Windows ConPTY 句柄
p.try_wait()                          # 退出码 | None(运行中)
p.kill()
p.buffered_bytes()                    # 读缓冲中待取字节数
p.close()                             # 幂等；关闭后 read() 恒为 b''，get_size()==(0,0)
```

### 3.3 速查

```
Pty(cols=80, rows=24)
spawn(argv, cwd=None, env=None, raw_cmdline=None, job_handle=None) -> (pid, handle)
read(n=65536, timeout=None) -> bytes · write(data) · resize(cols, rows) · get_size() -> (cols, rows)
try_wait() -> int|None · kill() · close() · buffered_bytes() -> int
child_pid() -> int|None · child_handle() -> int|None · hpcon() -> int|None
```

---

## 4. Surface（网格 → 增量 ANSI 字节）

```python
s = pywezterm.Surface(cols=80, rows=24)

s.set_cell(0, 0, "Hi", fg="p1", bg="#000000", bold=True)   # 可选样式全默认
seq, frame = s.get_changes_bytes(0)      # 首帧：全量
# ... 画完整屏 ...
seq, frame = s.get_changes_bytes(seq)    # 之后只含变化；无变化则 frame == b''

s.repaint_bytes()                        # 强制全量 = get_changes_bytes(0)
s.resize(100, 30)                        # 尺寸变化 → 下帧全量
s.clear()                                # 重建表面：模型与变更流一起清空
s.dimensions()                           # (cols, rows)
s.current_seqno()
```

`clear()` 会**重建**表面（序号归零），而不是只往变更流里塞一条清屏：只塞清屏的话模型里的格子还在，
下一次全量重绘会把旧内容原样画回来。

输出为 TrueColor ANSI；直接写到真实终端即可。`set_cell` 的 `text` 可为多字符（如 `"Hello"`）。

```
Surface(cols=80, rows=24)
set_cell(x, y, text, fg='default', bg='default', bold=False, italic=False,
         underline=False, reverse=False, strike=False)
get_changes_bytes(since_seqno) -> (seq, bytes) · repaint_bytes() -> (seq, bytes)
resize(cols, rows) · clear() · dimensions() · current_seqno()
```

---

## 5. ConsoleInput（Windows）

构造即接管控制台输入/输出模式与代码页，`restore()`（或对象销毁）时还原。
事件读取非阻塞：先 `wait_input(ms)` 等待，再 `read_inputs()` 取全部。

事件已归一化，调用方拿到的是 pywezterm 键名与整屏坐标，不需要接触任何 Win32 结构：

```python
("key", key, mods, down)                      # key 名同 Terminal.key_down；Ctrl+字母归一为字母 + CTRL 位；修饰键自身忽略
("mouse", x, y, kind, button, mods, clicks)   # 抬键自动补 last_pressed 按钮；clicks 为连击次数（1 / 2 / 3）
("resize",)
```

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
                    t.key_down(key, mods)         # 编码后自行写 pty
            elif ev[0] == "mouse":
                _, x, y, kind, button, mods, clicks = ev  # ('mouse', 12, 4, 'press', 'left', 0, 1)
                t.mouse(x, y, kind, button, mods)
            elif ev[0] == "resize":
                cols, rows = ci.size()            # ('resize',) 后立即取尺寸
                t.resize(cols, rows)
finally:
    ci.restore()
```

坐标落在哪个终端、事件发给谁，由调用方决定 —— 本库只提供「宿主事件 → 归一化输入」这一步。

```
ConsoleInput()
wait_input(ms) -> bool · read_inputs() -> list[tuple] · size() -> (cols, rows) · restore()
```

---

## 6. 模块函数

```python
pywezterm.version()                       # '0.1.0'
pywezterm.cursor_seq(row, col, visible)   # '\x1b[r+1;c+1H' + '\x1b[?25h' / '\x1b[?25l'
pywezterm.env_info() -> dict              # 部署自省：见下
pywezterm.clipboard_read()  -> str        # 仅 Windows；无内容返回 ''
pywezterm.clipboard_write(text)           # 仅 Windows；空串为 no-op
```

**`env_info()`** 把原本隐式的部署前提变成可查的事实：

```python
pywezterm.env_info()
# {'module_dir': '.../site-packages/pywezterm',
#  'conpty_dir': '.../site-packages/pywezterm',   # None = 未找到侧载二进制
#  'conpty_active': True}                          # False = 回落系统 conhost
```

- `module_dir` 由**扩展模块自身的路径**推导（Windows `GetModuleFileNameW`，POSIX `dladdr`），
  不依赖进程 CWD，也不依赖 `__file__`；
- `conpty_active` 为 `False` 时说明侧载没生效、正在用系统 conhost —— 这原本是静默回落。

**平台能力**：宿主剪贴板与 `ConsoleInput` 仅在 Windows 存在（前者需要连接 X11/Wayland 会话，
后者依赖 Win32 控制台）。其他平台上这两个名字**不存在**，而不是「存在但永远失败」。

---

## 7. 常用配方

**无子进程的终端仿真**（解析日志/测试转义序列）
```python
t = pywezterm.Terminal(120, 40)
t.feed(data)
text, snap, zones = t.text(), t.snapshot(), t.get_semantic_zones()
```

**子进程交互驱动**
```python
p, t = pywezterm.Pty(80, 24), pywezterm.Terminal(80, 24)
p.spawn(argv)
def pump():
    b = p.read(65536, timeout=0.1)
    if b:
        t.feed(b); p.write(t.drain_written())
def send(keys):                 # 键入
    for k, mods in keys: p.write(t.key_down(k, mods)); p.write(t.key_up(k, mods))
```

**截屏 / 导出**
```python
open("shot.png", "wb").write(t.render_image(scale=2, fmt="png"))
open("shot.svg", "w", encoding="utf-8").write(t.render_svg(1))
ansi = t.render_ansi(include_cursor=True)   # 可直接写到真实终端
```

**整屏差分渲染**
```python
base = t.current_seqno()
...
for row in t.changed_stable_rows(base):
    ...   # 只重绘这些行
```
