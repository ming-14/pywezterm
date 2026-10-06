# pywezterm

把 [wezterm](https://github.com/wez/wezterm) 的核心库化为一个独立的 Python 扩展模块：**伪终端引擎**（portable-pty / ConPTY）+ **终端模拟器**（wezterm-term）+ **多 pane 复用器** + **增量渲染** + **图片/SVG 导出**。

用 Rust（pyo3 / abi3）绑定，纯扩展模块、无 C 运行时依赖、无 GUI 依赖。任何 Python 程序 `import pywezterm` 即可获得一个行为与真实终端一致的 VT 状态机。

```python
import pywezterm, time

p = pywezterm.Pty(cols=80, rows=24)
t = pywezterm.Terminal(cols=80, rows=24)
p.spawn(["python", "-c", "print('hello')"])

deadline = time.time() + 10
while time.time() < deadline:
    chunk = p.read(4096, timeout=0.2)     # 释放 GIL，不阻塞其他线程
    if not chunk:
        if p.try_wait() is not None:
            break                          # 子进程已退出且无余量
        continue
    t.feed(chunk)                          # VT 字节流 → 终端模型
    resp = t.drain_written()               # 终端生成的应答/编码
    if resp:
        p.write(resp)                      # 回写，否则子进程会等应答卡死

print(t.text())                            # 可见屏幕纯文本
print(t.render_svg(1))                     # 同一屏幕的 SVG
p.close()
```

## 目录

- [安装](#安装)
- [构建](#构建)
- [能力总览](#能力总览)
- [API 参考](#api-参考)
  - [模块级函数](#模块级函数)
  - [Pty — 伪终端引擎](#pty--伪终端引擎)
  - [Terminal — 终端模拟器](#terminal--终端模拟器)
  - [Mux — 多 pane 复用器](#mux--多-pane-复用器)
  - [Surface — 增量渲染表面](#surface--增量渲染表面)
  - [ConsoleInput — Windows 控制台输入](#consoleinput--windows-控制台输入)
- [核心概念](#核心概念)
- [使用示例](#使用示例)
- [平台支持](#平台支持)
- [仓库结构](#仓库结构)
- [开发与测试](#开发与测试)
- [许可](#许可)

---

## 安装

从 [GitHub Releases](https://github.com/ming-14/pywezterm/releases) 下载对应平台的 wheel：

```bash
pip install pywezterm-0.1.0-cp38-abi3-win_amd64.whl
```

ABI3（`abi3-py38`）构建，**一个 wheel 通吃 Python ≥ 3.8 的所有版本**。已发布的资产：

| 平台 | wheel 文件名片段 |
|---|---|
| Windows x64 / x86 / ARM64 | `win_amd64` / `win32` / `win_arm64` |
| Linux x64 / ARM64 | `manylinux_2_28_x86_64` / `manylinux_2_28_aarch64` |
| macOS Apple Silicon | `macosx_11_0_arm64` |

Windows x64 的 wheel 自带 `conpty.dll` + `OpenConsole.exe`（见 [ConPTY 侧载](#conpty-侧载-windows-x64)）；其余平台只含扩展模块本身。

## 构建

依赖：Rust 工具链（rustup）、Python、maturin（缺失时自动 `pip install`）；Windows 另需 Visual Studio C++ 桌面工作负载。

```bash
python BUILD.py                              # Release，产物在 target/wheels/
python BUILD.py --config Debug --rebuild     # Debug + 清理缓存全新构建
python BUILD.py --wheel-dir dist             # 指定输出目录
python BUILD.py --vcvars "D:\VS\...\vcvars64.bat"   # 手动指定 vcvars
```

`BUILD.py` 自动探测 `cargo`（`~/.cargo/bin` → `PATH`）、`python`、`vcvars64.bat`（`vswhere` → 常见安装路径），也可用 `--cargo-dir` / `--python` 显式指定。等价的手工构建：

```bash
pip install maturin
maturin build --release --out target/wheels
```

---

## 能力总览

| 类 / 函数 | 平台 | 用途 |
|---|---|---|
| `Pty` | 全平台 | ConPTY / Unix PTY：创建伪终端、spawn 子进程、读写、resize、句柄暴露 |
| `Terminal` | 全平台 | VT/ANSI 状态机：喂字节流、屏幕快照、scrollback、键盘鼠标编码、选区、模式跟踪、SVG/图片渲染 |
| `Mux` | 全平台 | 多 pane（`Pty` + `Terminal` 组合）、布局矩形、增量整屏合成、状态栏 |
| `Surface` | 全平台 | 手工构造帧 → 只输出变化的增量 ANSI 字节 |
| `ConsoleInput` | Windows | 归一化的控制台输入采集（按键 / 鼠标 / resize），自动保存恢复控制台模式 |
| `clipboard_read` / `clipboard_write` | Windows | 剪贴板文本读写（供 OSC 52 落地） |
| `version()` / `cursor_seq()` | 全平台 | 版本号 / 光标定位序列 |

`Terminal` 支持的终端特性（由 wezterm-term 提供）：CSI/SGR/OSC 全集、备用屏与 scrollback、宽字符与双向文本、`OSC 0/2` 标题、`OSC 7` 当前目录、`OSC 9` 进度、`OSC 52` 剪贴板、`OSC 133` 语义区、`OSC 8` 超链接下载、Kitty keyboard / CSI-u 编码、鼠标追踪（1000/1002/1003/1006/1016）、bracketed paste、同步输出（2026）、Sixel/iTerm 图像内嵌。

---

## API 参考

### 模块级函数

```python
pywezterm.version() -> str
```
绑定库版本。

```python
pywezterm.cursor_seq(row: int, col: int, visible: bool) -> str
```
光标定位序列。入参 **0-based**，输出 1-based CUP + `\x1b[?25h` / `\x1b[?25l`。

```python
pywezterm.clipboard_read() -> str            # Windows；无文本返回 ""
pywezterm.clipboard_write(text: str) -> None # Windows
```

### `Pty` — 伪终端引擎

封装 `portable-pty`。内部为「reader 线程 + 缓冲队列」模型（见 [读缓冲与背压](#读缓冲与背压)）。

| 方法 | 说明 |
|---|---|
| `Pty(cols=80, rows=24)` | 创建伪终端，**不** spawn 子进程 |
| `spawn(argv, cwd=None, env=None, raw_cmdline=None) -> (pid, handle)` | 启动子进程。`env` 为 `{k: v}`；`handle` 为进程句柄（POSIX 恒为 0） |
| `read(n=65536, timeout=None) -> bytes` | 读取输出。`timeout=None` 阻塞到有数据或 EOF；否则最多等 `timeout` 秒。EOF / 超时 / 已关闭返回 `b""`。等待期间**释放 GIL** |
| `write(data: bytes) -> None` | 写入输入。阻塞写期间**释放 GIL** |
| `resize(cols, rows) -> None` | 调整尺寸 |
| `get_size() -> (cols, rows)` | 当前尺寸 |
| `buffered_bytes() -> int` | 读缓冲中待 `read` 取走的字节数 |
| `child_pid() -> int \| None` | 子进程 PID |
| `try_wait() -> int \| None` | 非阻塞退出码；`None` = 仍在运行或未 spawn |
| `kill() -> None` | 终止子进程 |
| `close() -> None` | 终止子进程 + 取消 reader 阻塞读 + 释放伪控制台。幂等；关闭后 `read()` 恒返回 `b""` |
| `hpcon() -> int \| None` | **Windows**：底层 ConPTY 的 `HPCON` 句柄（沙箱 / 外部 spawn 用） |
| `child_handle() -> int \| None` | **Windows**：进程句柄（Job Object 注册用） |

`raw_cmdline`（仅 Windows 生效）：提供时**整条命令行**（含程序名）原样传给子进程，绕过 argv 引号序列化，程序名取第一个 token（支持引号内空格路径）。给 `cmd.exe /c` 这类自解析命令行的程序用——argv 序列化的 `\"` 转义是 C 运行时规则，在 `cmd.exe` 里会变成字面反斜杠。其余平台忽略该参数、按 `argv` 执行，因此两处都给：

```python
import os, pywezterm

comspec = os.environ["COMSPEC"]
p = pywezterm.Pty()
p.spawn([comspec, "/c", 'echo "a b"'],
        raw_cmdline=f"{comspec} /c echo \"a b\"")   # 引号语义交给 cmd.exe
```

### `Terminal` — 终端模拟器

一个纯状态机：`feed()` 进 VT 字节，各种 getter 出屏幕状态；键盘/鼠标输入编码成要下发给应用的字节，应用查询的应答进捕获缓冲等 `drain_written()` 取走。

**生命周期与喂入**

| 方法 | 说明 |
|---|---|
| `Terminal(cols=80, rows=24, scrollback=10000)` | 创建 |
| `feed(data: bytes)` | 喂入程序输出的 VT 字节流（同步跟踪 DECSET/SM 模式状态） |
| `resize(cols, rows)` | 调整尺寸（锚顶语义，各平台一致） |
| `reset()` | 完整复位（RIS 语义：擦屏幕 + scrollback + 全部状态） |
| `scrollback_count() -> int` | 历史行数 |
| `clear_scrollback()` | 清空历史（等价 `\x1b[3J`） |

**屏幕读取**

| 方法 | 说明 |
|---|---|
| `text() -> str` | 可见屏幕纯文本（每行去尾空白、去掉末尾空行、`\n` 分隔） |
| `snapshot() -> list[list[Cell]]` | 可见区字符网格 |
| `snapshot_lines() -> list[tuple[bool, list[Cell]]]` | 同上，附带 `wrapped`（该物理行以折行结尾，需与下行拼接） |
| `scrollback() -> list[list[Cell]]` | 历史区字符网格 |
| `logical_lines() -> list[(start_stable, end_stable, cells)]` | 逻辑行（由 wezterm 做折行重组，含超长行防护） |
| `cursor() -> (row, col, visible)` | 光标，**0-based**；计入滚动偏移，滚出可见区则 `visible=False` |
| `render_ansi(include_cursor: bool) -> str` | 可见屏幕的 ANSI 重建序列（逐行 CUP + SGR，截断末尾空行） |
| `render_scrollback(keep_ansi: bool) -> str` | 历史区。`False` = 纯文本；`True` = 每行带 SGR + `\r\n`（供前端恢复） |
| `current_seqno() -> int` | 当前序列号（每次 `feed` 递增），脏行差分基线 |
| `changed_stable_rows(since_seqno) -> list[int]` | 自 `since_seqno` 起变化过的稳定行号（可见区 + 历史） |

`Cell` 元组共 10 项：

```python
(col, char, fg, bg, bold, italic, underline, reverse, strike, width)
#  颜色为 "default" | "p<索引>" | "#rrggbb"；width 为显示宽度（宽字符 2）
```
`col` 取单元格的真实列索引：宽字符后被跳过的空白格不出现，因此**列号会跳位**，渲染时按 `width` 占列。

**滚动**

`scroll(delta)`（`delta>0` 上滚看更早，`<0` 回落，自动 clamp）、`scroll_to_bottom()`（恢复跟随最新输出）。所有读取接口（`text` / `snapshot` / `cursor` / `render_*`）都计入当前视图偏移。

**输入编码**

| 方法 | 说明 |
|---|---|
| `key_down(key, mods) -> bytes` | 按下编码，返回应下发到 pty 的字节 |
| `key_up(key, mods) -> bytes` | 抬起编码 |
| `mouse(x, y, kind="press", button="left", mods=0) -> bytes` | 鼠标事件编码，同时把事件记进模型（应用据此改状态） |
| `send_paste(text) -> None` | 模式感知粘贴（bracketed paste 开启时自动包裹）；字节进捕获缓冲，用 `drain_written()` 取走 |
| `focus_changed(focused: bool)` | 上报焦点（配合 DECSET 1004） |
| `drain_written() -> bytes` | 取走终端生成的一切输出：键盘编码回显 + **应用查询的应答** |

> **`Terminal` 只编码、不写 pty** —— 它不知道有没有 pty。返回值要由调用方写下去（`Mux` 的 `key_down` / `mouse` 则会自动下发到对应 pane 的 pty，这是两者的关键差别）。

- `key`：`Up` `Down` `Left` `Right` `Home` `End` `Insert` `Delete` `PageUp` `PageDown` `Backspace` `Tab` `Enter` `Esc` `Space` `F1`–`F24`，或任意单字符。
- `mods`：`KeyModifiers` 位掩码 —— `SHIFT=2`、`ALT=4`、`CTRL=8`。
- `kind`：`press` / `release` / `move`；`button`：`left` / `middle` / `right` / `wheel_up` / `wheel_down` / `none`。

**模式与状态查询**

| 方法 | 说明 |
|---|---|
| `is_mouse_grabbed() -> bool` | 应用是否接管鼠标（1000/1002/1003） |
| `get_mouse_encoding() -> (mode, sgr)` | 具体模式号（0/1000/1002/1003）与 1006 是否启用 |
| `get_keyboard_encoding() -> str` | `xterm` / `csi-u` / `win32` / `kitty` |
| `is_alt_screen_active()` / `bracketed_paste_enabled()` | 备用屏 / 粘贴模式 |
| `mode_restore_seq() -> str` | 恢复当前模式状态的 ANSI 序列，见[模式跟踪](#模式跟踪与-mode_restore_seq) |
| `get_title() -> str` | `OSC 0/2` |
| `get_current_dir() -> str \| None` | `OSC 7` |
| `get_progress() -> (label, percent)` | `OSC 9`；label ∈ `none`/`percentage`/`error`/`indeterminate` |
| `get_semantic_zones() -> list[(y0,x0,y1,x1,type)]` | `OSC 133` 提示符/输入/输出区 |

**选区**

`selection_set(anchor_row, anchor_col, end_row, end_col)`（stable 坐标）、`selection_select_word(row, col)`（双击选词）、`selection_select_line(row, col)`（三击选行）、`selection_text() -> str`、`selection_active() -> bool`、`selection_clear()`、`make_all_lines_dirty()`（选区变化后强制全量失效）。

**回调**（应用主动发起的事，交回 Python 处理）

| 方法 | 回调签名 |
|---|---|
| `set_clipboard_callback(cb)` | `(selection: str, content: str \| None) -> None` —— OSC 52 |
| `set_download_callback(cb)` | `(name: str \| None, data: bytes) -> None` —— OSC 8 下载 |
| `set_device_control_callback(cb)` | DCS 序列 |
| `set_notification_callback(cb)` | Alert：Bell / 标题 / 进度等 |

> 回调里**只**做落地动作（写剪贴板、置事件），不要在回调内反查终端状态——回调可能与 reader 线程争同一把锁而死锁。

**图形导出**

| 方法 | 说明 |
|---|---|
| `render_svg(compression_level: int) -> str` | 可见屏幕 SVG。`0` = 原样；`>=1` 压缩（去空 text、折叠标签间空白） |
| `render_image(scale: float, fmt: str) -> bytes` | 位图字节。`fmt` ∈ `png`/`jpg`/`jpeg`/`bmp`；`scale` 为格子像素倍数（`1.0` 标准、`2.0` 高清） |

纯 Rust 光栅化（fontdb 字体发现 + fontdue 字形 + tiny-skia 合成 + image 编码），不依赖任何 GUI 库。与 `snapshot()` 同视角。

### `Mux` — 多 pane 复用器

`Pty` + `Terminal` 的组合体，附带布局、整屏增量合成与输入路由，适合做宿主（网页终端、录屏、CI 终端面板）。

| 方法 | 说明 |
|---|---|
| `Mux(cols=80, rows=24)` | 创建（整屏尺寸） |
| `add_pane(argv, cwd=None, env=None) -> pane_id` | 建 pane：openpty + spawn + 起 reader 线程自动喂终端。**当前布局最多 2 个 pane**（左右分屏） |
| `pane_count()` / `focused()` / `dimensions()` | 查询 |
| `pane_rects() -> list[(x,y,w,h)]` | 各 pane 布局矩形 |
| `pane_at(x, y) -> pane_id \| None` | 命中测试（分隔线/状态栏/屏幕外返回 `None`） |
| `resize(cols, rows)` | 宿主屏尺寸变化：重算矩形 + resize 各 pane 的 pty 与模型 |
| `pane_resize(pane_id, cols, rows)` | 单 pane 尺寸 + 布局矩形同步 |
| `set_sep(sep=True)` / `set_split_col(split)` / `set_status_rows(n)` / `set_status(text)` | 分隔线 / 分屏列 / 状态栏行数 / 状态栏文本 |
| `render() -> (bytes, row, col, visible)` | **增量整屏合成**，见下 |
| `force_repaint()` | 强制下一帧全量重绘（录制暂停恢复 / 收敛帧） |
| `set_output_callback(cb)` | 任一 pane 有新输出并喂入后调用（无参数）。事件驱动渲染，替代轮询；传 `None` 清除 |
| `close_pane(pane_id)` / `close()` | 关闭 pane（从布局移除 + 重算矩形）/ 关闭全部。幂等 |

输入路由（`key_down` / `key_up` / `mouse` / `scroll` / `scroll_to_bottom` / `send_paste` 作用于**焦点** pane；对应 `pane_*` 版本指定 pane）：

`key_down(key, mods)`、`key_up(key, mods)`、`mouse(x, y, kind="press", button="left", mods=0)`、`scroll(delta)`、`scroll_to_bottom()`、`send_paste(text)`、`set_focus(pane_id)`。均返回编码字节。整屏 `mouse(x, y, ...)` 坐标未命中任何 pane 时抛 `RuntimeError`，先用 `pane_at()` 判定。

每 pane 读取：`pane_text(id)`、`pane_cursor(id) -> (row,col,visible)`、`pane_is_mouse_grabbed(id)`、`pane_try_wait(id)`、`pane_take_output(id) -> bytes`（原始输出，供录制）、`pane_output_len(id)`、`pane_write(id, data)`。选区与 OSC 52：`pane_selection_*`、`set_focus_selection_callback(cb)`（实际挂到所有 pane，焦点切换不丢回调）。

**`render()` 返回值**

```python
bytes, cursor_row, cursor_col, cursor_visible = mux.render()
```

- `bytes`：增量 ANSI（含 CUP 定位，序列内坐标是 **1-based** 终端语义）；未变化帧为空 `b""`（但光标可能仍需重绘）。
- `cursor_row` / `cursor_col`：焦点光标整屏坐标，**0-based** —— 与 `bytes` 里的 1-based 并存，注意区分。
- 增量策略：首帧、视图滚动、resize、`force_repaint()` 后全量重写；否则按终端脏行只重写变化的行。

### `Surface` — 增量渲染表面

不 spawn 进程、自己按格子画内容，只把变化输出成 ANSI 字节。适合把非终端来源的画面（表格、仪表盘、AI 生成内容）接到真实终端或前端。

```python
s = pywezterm.Surface(80, 24)
s.set_cell(0, 0, "Hello", "red", "default", bold=True)   # x, y, text, fg, bg, ...
seqno, data = s.get_changes_bytes(0)                     # 首帧全量
seqno, data = s.get_changes_bytes(seqno)                 # 空 b"" = 无变化
```

| 方法 | 说明 |
|---|---|
| `Surface(cols=80, rows=24)` | 创建（对应宿主真实尺寸） |
| `set_cell(x, y, text, fg="default", bg="default", bold=False, italic=False, underline=False, reverse=False, strike=False)` | 写一格，0-based |
| `get_changes_bytes(since_seqno) -> (seqno, bytes)` | 自 `since_seqno` 起的增量。`since` 过旧或超预算时自动退回全量重绘 |
| `repaint_bytes() -> (seqno, bytes)` | 强制全量 |
| `current_seqno()` / `dimensions()` / `resize(cols, rows)` / `clear()` | 基线 / 尺寸（尺寸变化丢弃缓冲，下次全量）/ 清空 |

### `ConsoleInput` — Windows 控制台输入

把宿主持有控制台时的输入采集搬到绑定层，替代手写 ctypes Win32。构造即设置控制台模式（输出侧 VT + 禁自动换行回车，输入侧原始按键）并**保存原模式**；`restore()` 或对象析构时恢复。stdio 被重定向（非控制台）时构造失败。

```python
ci = pywezterm.ConsoleInput()
try:
    if ci.wait_input(100):                # 等输入事件，False = 超时
        for ev in ci.read_inputs():       # 一次读空全部待处理记录
            print(ev)
finally:
    ci.restore()
```

`read_inputs()` 返回归一化 tuple 列表：

```python
("key", key, mods, down)                      # key 名同 Terminal.key_down；Ctrl+字母归一为字母 + CTRL 位；修饰键自身忽略
("mouse", x, y, kind, button, mods)           # 抬键自动补 last_pressed 按钮
("resize",)
```

`size() -> (cols, rows)` 取当前窗口逻辑尺寸。

---

## 核心概念

### 闭环：应答必须回写

子进程常向终端发查询（`\x1b[6n` 光标位置、`\x1b]10;?` 颜色、Kitty keyboard 能力查询等）。`Terminal` 生成的应答进**捕获缓冲**，取走它是调用方的责任：

```python
t.feed(chunk)                 # 1. 输出喂进模型
resp = t.drain_written()      # 2. 取走应答/编码
if resp: p.write(resp)        # 3. 回写 pty —— 否则子进程等应答卡死
```

`Mux` 已把这三步放进它自己的 reader 线程，无需手工处理；裸用 `Pty` + `Terminal` 时必须自己闭环。

### 读缓冲与背压

`Pty` 的 reader 线程持续从管道读入队列。队列有**高/低水位**（1 MiB / 256 KiB）：到达高水位就**停止**从 PTY 读取，让管道自然填满，子进程的 `write` 被阻塞——这是背压真正传导到子进程的唯一途径。没有上限时，reader 会把管道抽干、数据无限堆积在队列里，上层限流只是拦住自己。

`buffered_bytes()` 让这个不变量可观测。Python 侧的 `read()` 取走数据后会自动唤醒停在高水位的 reader。

### GIL

`read()` 与 `write()` 都在**释放 GIL** 的前提下轮询/阻塞。PTY 写满时（子进程不读输入、且它在等你的应答），持锁的阻塞写会卡死整个 Python 进程，包括 asyncio 事件循环。

### 模式跟踪与 `mode_restore_seq()`

`feed()` 扫描到达的模式序列（`CSI ? Pm h/l` 与 `CSI Pm h/l`），记录 DECSET 1 / 6 / 7 / 12 / 25 / 45 / 47 / 66 / 1000 / 1002 / 1003 / 1004 / 1006 / 1016 / 1047 / 1049 / 2004 与 SM 4 / 20。`mode_restore_seq()` 输出恢复序列，用于客户端（如 xterm.js）重连或订阅时把屏幕状态补齐。

两点值得知道：

- **每个模式字段是 `Option`**：`None` = 应用从未设置过它，恢复时**不发**这条序列，让客户端保留自己的默认值。记成 `bool` 会用「我们以为的默认值」覆盖客户端设置（例如光标闪烁）。
- 只能看到**到达我们的**序列。Windows/ConPTY 会吞掉一部分（鼠标模式、DECCKM、光标可见、原点模式、备用屏），那些无从得知也不可能补回——环境限制，不是实现缺陷。
- 有意**不**恢复 `?2026`（同步输出）：那是逐帧瞬时模式，打开会让客户端一直缓冲渲染，画面冻在最后一帧。

### ConPTY 侧载 Windows x64

Windows x64 的 wheel 里带 `conpty.dll` + `OpenConsole.exe`（来自 wezterm 自带的 conhost）。首次创建 `Pty` 时把包目录设为侧载目录，portable-pty 便优先使用 wezterm 的 OpenConsole 宿主，而非系统 conhost —— 行为与 wezterm 一致，也规避若干系统 conhost 的差异。Windows ARM64 / i386 与其余平台不带这些二进制（跨架构无法加载），走系统内核 ConPTY。

---

## 使用示例

### 无头跑命令并读回屏幕

```python
import os, time, pywezterm


def run(argv, cols=80, rows=24, timeout=10.0):
    """无头跑一条命令，返回它退出后的屏幕文本"""
    p = pywezterm.Pty(cols, rows)
    t = pywezterm.Terminal(cols, rows)
    p.spawn(argv)
    deadline = time.time() + timeout
    try:
        while time.time() < deadline:
            chunk = p.read(4096, timeout=0.2)
            if chunk:
                t.feed(chunk)
                resp = t.drain_written()   # 子进程查询终端 → 应答必须回写
                if resp:
                    p.write(resp)
            elif p.try_wait() is not None:
                while True:                # 退出后排空残余输出（继续喂模型）
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

### 分屏：左右两个 shell，增量取帧

```python
import os, pywezterm


def shell():
    return ["/bin/sh"] if os.name == "posix" else [os.environ.get("COMSPEC", "cmd.exe")]


mux = pywezterm.Mux(120, 30)
left = mux.add_pane(shell())
right = mux.add_pane(shell())          # 第 2 个 pane 触发左右二分布局
mux.set_sep(True)                      # 中间留一列分隔线
mux.set_status_rows(1)
mux.set_status("pywezterm demo")

data, row, col, visible = mux.render() # 首帧全量 ANSI
print(repr(data[:60]), row, col, visible)
print(mux.pane_rects())                # [(x, y, w, h), ...]

mux.pane_key_down(left, "v", 0)        # 键入编码后自动下发该 pane 的 pty
mux.pane_key_down(left, "e", 0)
mux.pane_key_down(left, "r", 0)
mux.pane_key_down(left, "Enter", 0)

data, *_ = mux.render()                # 只输出这一帧变化的字节
```

### 事件驱动渲染（替代轮询）

```python
import os, threading, pywezterm

wake = threading.Event()
mux = pywezterm.Mux(80, 24)
mux.set_output_callback(wake.set)   # reader 线程喂完数据后唤醒
mux.add_pane(["/bin/sh"] if os.name == "posix" else [os.environ.get("COMSPEC", "cmd.exe")])

while True:
    if wake.wait(0.1):
        wake.clear()
        data, *_ = mux.render()
        if data:
            ...            # 发给前端 / 写回宿主控制台
```

### 导出终端画面

```python
import pywezterm

t = pywezterm.Terminal(80, 24)
t.feed(open("capture.bin", "rb").read())

open("term.svg", "w", encoding="utf-8").write(t.render_svg(1))
open("term.png", "wb").write(t.render_image(2.0, "png"))
print(t.render_scrollback(keep_ansi=False))   # 含 scrollback 的纯文本
```

### OSC 52 落到系统剪贴板（Windows）

```python
import pywezterm

t = pywezterm.Terminal(80, 24)
t.set_clipboard_callback(lambda sel, content: content and pywezterm.clipboard_write(content))
t.feed(b"\x1b]52;c;aGVsbG8=\x1b\\")
print(pywezterm.clipboard_read())      # 'hello'
```

---

## 平台支持

| 平台 | 架构 | 说明 |
|---|---|---|
| Windows | x64 | 完整支持，wheel 含侧载 ConPTY 二进制 |
| Windows | x86 (i386) | 支持（修复了 32 位 ConPTY 调用约定导致的栈破坏） |
| Windows | ARM64 | 支持，走系统内核 ConPTY |
| Linux | x64 / ARM64 | 支持；CI 在 `manylinux_2_28` 容器构建，兼容 glibc 2.28+ |
| macOS | Apple Silicon | 支持，CI 出预构建 wheel |
| macOS | Intel x64 | 代码支持，需本地构建（CI 无 Intel runner） |

Python ≥ 3.8，ABI3 单 wheel。CI 覆盖上述可构建矩阵并跑 `pytest`；打 `v*` tag 自动创建/更新 GitHub Release 并附 `SHA256SUMS.txt`。

---

## 仓库结构

```
pywezterm/                 Python 包壳（from .pywezterm import *）
BUILD.py                   跨平台构建脚本
pyproject.toml             maturin 构建配置 + Windows 二进制打包规则
AGENTS.md                  开发约束 + 对上游 wezterm 的改动记录
assets/windows/conhost/    侧载用 conpty.dll + OpenConsole.exe
tests/                     库级自测（pytest）
wezterm/                   vendored wezterm 核心 crate（上游源码 + 本项目的绑定）
  pywezterm-core/          ← 领域层（不依赖 pyo3）：pty、终端模型、渲染、复用器
    src/
      error.rs          统一错误类型
      env.rs            模块自身资源位置 + 部署自省
      input.rs          输入事件词汇表（平台层产出、终端层消费）
      term/             grid（Cell/Color/Attrs）· model · view · encode · selection
      render/           ansi · surface（增量）· svg · pixmap · font
      host/             Pane（pty + 模型 + reader + 背压 + 关闭）· registry
      mux/              layout · compose · chrome
      platform/         windows/ · posix/ —— 同名接口，各平台各自实现
  pywezterm/               ← 只有绑定壳
    Cargo.toml  build.rs
    src/
      lib.rs            模块注册
      py/               签名、默认值、类型转换、GIL 管理、错误映射
  term/ pty/ termwiz/ vtparse/ bidi/ wezterm-surface/ ...   上游 crate
```

领域层不依赖 `pyo3` —— 在那里写 `use pyo3::` 直接编译不过，分层由编译器保证而不是靠约定。
详见 `ARCHITECTURE.md`。

`wezterm/` 下的上游 crate 默认**不修改**；确有必要（如修 wezterm 自身 bug）时，改动记录写进 `AGENTS.md`。

## 开发与测试

```bash
python BUILD.py                                   # 构建 wheel
python -m pip install --force-reinstall target/wheels/*.whl
python -m pytest tests/ -v
```

测试直接跑已安装的 wheel —— 在仓库根执行 pytest 前请先移除源码 `pywezterm/` 目录，否则那个只有 `import *` 的空壳会遮蔽已安装的包（CI 就是这么做的）。

`tests/` 按能力划分：`test_pty`（伪终端 + 闭环）、`test_term` / `test_stage1_state`（VT 状态与模式）、`test_stage2_render` / `test_surface_render`（渲染）、`test_mux_*`（pane、布局、低层）、`test_selection`、`test_console_input`、`test_edge`、`test_refactor_invariants`（分块不变性、失败路径、幂等性）。

Rust 侧单测在 `pywezterm-core` 里，不需要 Python 解释器即可运行：

```bash
cargo test -p pywezterm-core          # 94 个
```

Rust 侧另有单元测试：`cargo test --manifest-path wezterm/pywezterm/Cargo.toml`。

## 许可

MIT。`pywezterm` 绑定部分为本项目；`wezterm/` 下 vendored 的上游 crate 版权归各自作者，同为 MIT（见 `wezterm/LICENSE.md`）。
