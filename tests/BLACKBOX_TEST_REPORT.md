# pywezterm 黑盒测试报告

> **更新说明**：复用器（`Mux`）已从本库整体移出（它不是基础设施而是应用策略，见
> `ARCHITECTURE.md` §10）。本报告中 Mux 相关的覆盖条目、用例与结论已随之删除，计数按移出后
> 重跑的结果更新。仍待决策的一项（F1 弱格式校验）保留在 §4。
>
> 生成时间视角：本机 Windows（win32），Python 3.11 venv，`maturin develop`（abi3-py38）构建的可编辑安装，ConPTY 侧载二进制生效。
> 测试哲学：**仅从公开 Python 接口的外部可观察行为验证**，不依赖任何内部实现细节。所有断言都基于「喂入输入 → 读回可观测输出」的黑盒证据。

## 1. 如何运行

```bash
cd reference/pywezterm
.venv/Scripts/python.exe -m pytest tests/ -q
```

- `pytest.ini`（`pythonpath = .`，`testpaths = tests`）与 `tests/conftest.py` 确保本仓库作为
  pytest rootdir 且仓库根排在 `sys.path` 最前，避免父项目 `agentic-tty/pyproject.toml` 的
  `pythonpath=["src","vendor"]` 把旧的 `vendor/pywezterm` 副本抢先导入（ARCHITECTURE.md 已警告此坑）。
- 补充套件：`tests/test_blackbox_comprehensive.py`。
- 既有基线套件：其余 9 个 `test_*.py`。

## 2. 结果汇总

| 范围 | passed | skipped | xfailed | 说明 |
| --- | --- | --- | --- | --- |
| 补充黑盒套件 | 29 | 1 | 1 | 见 §4 的 xfail 发现 |
| 既有基线套件 | 66 | 4 | 0 | 全部通过 |
| **合计** | **95** | **5** | **1** | 全绿 |

Rust 侧另有 `cargo test -p pywezterm-core` 的 **81** 个单测（不需要 Python 解释器）。

- 5 个 skipped：4 个来自 `test_console_input.py`、1 个来自本套件 `test_console_input_lifecycle`。
  原因一致——当前运行环境**无宿主交互控制台**（`读取控制台模式失败: 句柄无效。os error 6`），
  `ConsoleInput` 仅能完成导入冒烟，生命周期路径（wait_input/size/read_inputs）无法在非 TTY 下执行。
- 1 个 xfailed：为**真实发现**（见 §4），标 `strict=False` 以便后续修复后自动转回 XPASS。

## 3. 覆盖矩阵（公开 API → 黑盒覆盖）

| 公开接口 | 黑盒验证点 | 状态 |
| --- | --- | --- |
| `Pty(cols, rows)` | 构造 | ✅ |
| `Pty.spawn(argv, cwd, env, raw_cmdline, job_handle)` | pid/handle 返回；Windows 下 `child_handle()`/`hpcon()` | ✅ |
| `Pty.spawn` — `raw_cmdline` | 引号语义保留（对比 argv 的 C 运行时转义） | ✅ 已验证 |
| `Pty.spawn` — `job_handle` | ctypes `IsProcessInJob` 确认子进程在作业内 | ✅ 已验证 |
| `Pty.read / write / buffered_bytes / resize / try_wait / close` | 读写闭环；背压窗口 `buffered_bytes>0`；write-after-close 优雅无操作 | ✅ |
| `Terminal` 渲染 | `text() / render_ansi / render_scrollback / render_svg / render_image(png,jpg,bmp)` | ✅ |
| `Terminal` 模式 | `reset / clear_scrollback / get_keyboard_encoding / is_alt_screen_active` | ✅ |
| `Terminal` 输入编码 | `key_up / key_down / mouse / get_mouse_encoding / send_paste(括号粘贴) / cursor_seq` | ✅ |
| `Terminal` 回调 | `set_notification_callback`(响铃触发、回调异常不崩) / `set_download_callback` / `set_device_control_callback` | ✅ |
| `Surface` 增量 | `set_cell / get_changes_bytes / repaint_bytes / resize(丢缓冲)` / 样式发 SGR | ✅ |
| `ConsoleInput` | 导入冒烟（生命周期需交互控制台，本环境 skip） | ⚠️ 仅冒烟 |
| `clipboard_read / clipboard_write` | 读写往返（Windows） | ✅ |
| 异常层次 | `TerminalClosed / RenderError / PlatformUnsupported`（继承 RuntimeError）+ `ValueError` | ✅ 表面契约 |
| 模块函数 | `version / cursor_seq / env_info` | ✅ |

## 4. 发现的真实问题（xfail 记录）

### F1. `render_image` 对非受支持格式静默回落 PNG（弱格式校验）
- **黑盒证据**：`Terminal.render_image(1.0, "gif")` **不抛异常**，返回 948 字节、magic 为
  `\x89PNG`（即静默回落 PNG）。对比之下 `png/jpg/bmp` 三种格式按契约正确返回对应 magic。
- **影响**：调用方传入非法/不支持格式时期望得到 `ValueError`/`RenderError`，实际却拿到一张
  “看起来能渲染”的 PNG，可能掩盖调用错误。属中低严重度。
- **状态**：**尚未决策**。`docs/PYWEZTERM_API.*.md` 目前按实际行为写明「无法识别的 `fmt` 按 `png`
  处理」。二选一：(a) 在 `render_image` 入口对非 `{png,jpg,bmp}` 显式 `raise ValueError`
  （改行为 + 改文档 + 把本 xfail 转正）；(b) 维持宽松并在文档里把这一点写成正式契约（删掉本 xfail）。

## 5. 测试编写过程中的断言修正（透明度记录）

以下 6 项最初是**测试自身的错误断言**，已据黑盒实测证据修正（非库缺陷）：

1. `cursor_seq(2,3,True)` → 实测 `\x1b[3;4H`（0-based→1-based：row+1;col+1），原断言 `4;3H` 写反。
2. `get_mouse_encoding()` 默认 `(0,False)`；`?1006h` 只开 SGR **格式**，跟踪模式仍为 0 → `(0,True)`，
   原断言 `(1006,True)` 错把格式号当跟踪模式。
3. `Terminal.mouse` 签名 `(x,y,kind="press",button="left",mods=0)`；滚轮正确用法是
   `kind="press", button="wheel_up"`；且编码仅在跟踪模式（1000/1002/1003）开启时产生字节，
   原测试缺跟踪模式致拿到空字节。
4. `Pty.write` 在 `close()` 后为**优雅无操作**（不抛异常），与 `read()` 一致；原测试期望 `TerminalClosed` 错。
5. `Pty.spawn` 在 Windows 返回的 `(pid, handle)` 中 `handle` 为进程句柄（非 0）；原断言 `child==0` 错。
6. `raw_cmdline`：原测试用裸 `"cmd"` 作命令行首 token，CreateProcessW 报“找不到文件”；
   且 `&` 因 `cmd /c` 会剥离外层引号、在 argv 与 raw 两种形式下被同等当分隔符，**不构成区分**。
   改为用 `COMSPEC` 完整路径 + 内部引号 `echo "a b"`，清晰验证 argv 走 C 运行时 `\"` 转义、
   raw_cmdline 原样保留引号语义（F1 之外的又一项文档契约确认）。

## 6. 结论

- pywezterm 公开接口的外部可观察行为**整体符合文档契约**，渲染/输入编码/回调/作业对象/引号语义等核心能力均通过黑盒验证。
- 发现 **1 个待决策问题**（F1 弱格式校验），已用 `xfail` 标记。
- `ConsoleInput` 生命周期因当前无交互控制台而跳过，需在 TTY 环境补测。
- 复用器移出后，本套件的公开 API 表面契约断言已同步收窄为
  `Pty / Terminal / Surface / ConsoleInput` 四类 + `version / cursor_seq / env_info` + 三个异常类型。
