# 架构与分层（重构基线）

> 这份文档定的是**重构后的目标结构**。问题清单见 `REVIEW-pywezterm-crate.md`；本文只回答「东西应该放在哪、为什么」。
>
> 一句话概括目标：**每个概念只有一个家，依赖只有一个方向。**

---

## 0. 落地状态

**已落地**（`pywezterm-core` + `pywezterm` 双 crate，81 个 Rust 单测 + Python 用例全绿，两个 crate 零告警）：

| 项 | 状态 |
|---|---|
| 拆 crate：`pywezterm-core`（无 pyo3）+ `pywezterm`（仅绑定壳） | ✅ 编译器强制分层 |
| 目录分层 `py / render / host / term / platform` | ✅ |
| `Pane` 合一（`host/pane.rs` 是唯一宿主实现） | ✅ `Pty` 与 `Terminal` 都是它 |
| 模式状态收敛为正确的增量扫描器 | ✅ 分块不变性测试覆盖（见 §5.2 的取舍说明） |
| `Cell` / `Color` / `Attrs` 取代匿名元组 | ✅ 三个颜色解析器收敛为一个 |
| 渲染按阶段切（`render::ansi` / `surface` / `svg` / `pixmap`） | ✅ |
| `error.rs` 统一错误 + 异常层次 | ✅ 无死变体（`PaneNotFound` 已随复用器一并移除） |
| `env.rs` 由模块自身路径定位资源 | ✅ 不再依赖 `__file__` / CWD |
| `platform/` 收口平台分支 | ✅ 非 Windows 为**设计过的**实现而非占位 |
| 分块不变性 / 失败路径 / 幂等性测试 | ✅ `tests/test_refactor_invariants.py` |

**已移出：复用器（`mux`）。** 见 §10。它不是基础设施而是应用策略，已从库中整体删除。

**刻意未做**：

- **非 Windows 的宿主剪贴板与控制台输入**：前者需要连接 X11/Wayland 会话，后者依赖 Win32
  控制台，都不是本库该提供的抽象；对应绑定按平台注册，调用方看到的是「类不存在」而不是
  「类存在但永远失败」。
- **随包分发字体**：原来的 `src/assets/fonts` 候选路径在仓库里根本不存在（也没有下载步骤），
  属于死代码，已删除；只用系统字体。

---

## 1. 两条分层原则

**原则一：依赖单向，不允许回指。**
`py → render/host → term → platform`。上层可以调下层，下层不知道上层存在。尤其：**领域层（term/render/host）不认识 Python，平台层（platform）不认识终端。**

**原则二：一个概念只有一个所有者。**
这是本次重构真正要解决的东西。审查报告里的 P0 缺陷几乎都是同一件事的不同表现——同一个概念被实现了两遍，然后分叉：

- `Pane` 有两套（`pty.rs` + `mux.rs`）→ A2 只在第一套、A3 只在第二套；
- VT 序列解析有两套（`wezterm_term` + `term.rs::update_mode_state`）→ 64 字节窗口 bug；
- 颜色解析有三套（`color_attr_to_string` / `resolve_color` / `parse_color`）→ 语义不一致；
- 键鼠编码有四份（`term.rs` ×2 + `mux.rs` ×2）；
- 关闭协议有两份。

**所以目录不是审美问题。** 目录结构是「谁拥有什么」的物理体现；下面的第 4 节把每个概念指到唯一一个目录。

---

## 2. 目录树（现状）

```
wezterm/
├── pywezterm-core/             # 领域层：rlib，不依赖 pyo3
│   └── src/
│       ├── lib.rs
│       ├── error.rs            # 统一 Error enum
│       ├── env.rs              # 自身资源位置 + env_info() 自省
│       ├── input.rs            # 输入事件词汇表（平台层产出、终端层消费）
│       │
│       ├── term/               # L2 终端领域
│       │   ├── mod.rs
│       │   ├── grid.rs         # Cell / Color / Attrs（替代 CellTuple）
│       │   ├── model.rs        # wezterm_term::Terminal 封装 + 模式查询
│       │   ├── view.rs         # 视口偏移与可见窗口计算
│       │   ├── encode.rs       # 键鼠编码（唯一一份）
│       │   └── selection.rs    # 选区状态机
│       │
│       ├── render/             # L3 渲染：只吃网格，不认识 pty / Python
│       │   ├── mod.rs          # 统一出口：网格 → {ansi, svg, image}
│       │   ├── ansi.rs         # 全量 ANSI（原 term.rs::render_ansi）
│       │   ├── surface.rs      # 增量（原 surface_render.rs）
│       │   ├── svg.rs
│       │   ├── pixmap.rs
│       │   └── font.rs
│       │
│       ├── host/               # L1 宿主原语：唯一的「一个终端宿主单元」
│       │   ├── mod.rs
│       │   ├── pane.rs         # Pane：pty + 模型 + reader + 背压 + 关闭协议
│       │   └── pty.rs          # pty 生命周期（openpty / spawn / resize / close）
│       │
│       └── platform/           # L0 平台原语：只知道 OS，不知道终端
│           ├── mod.rs          # 同名接口（各平台各自实现）
│           ├── windows/        # conpty · reader_cancel · console_input · clipboard · self_dir
│           └── posix/          # conpty（恒空）· reader_cancel（无需）· self_dir
│
└── pywezterm/                  # cdylib，只有绑定壳
    ├── Cargo.toml
    ├── build.rs                # 只做一件事：把 assets/windows/conhost 交给打包
    └── src/
        ├── lib.rs              # 只剩 mod 声明 + pymodule 注册，零逻辑
        └── py/                 # L5 绑定壳：只做签名 / 默认值 / 类型转换 / GIL / 错误映射
            ├── mod.rs
            ├── error.rs        # Error → PyErr（异常层次）
            ├── convert.rs      # 领域类型 → Python 对象
            ├── pty.rs          # #[pyclass] Pty        → host::Pane
            ├── terminal.rs     # #[pyclass] Terminal   → host::Pane + term::*
            ├── surface.rs      # #[pyclass] Surface    → render::surface
            ├── console_input.rs# #[pyclass] ConsoleInput → platform::console_input
            ├── clipboard.rs    # 两个 #[pyfunction]     → platform::clipboard
            └── callbacks.rs    # Python 回调 → wezterm_term 的 handler trait 适配
```

**每个目录一句话职责：**

| 目录 | 一句话 | 判据（写代码时自问） |
|---|---|---|
| `py/` | 把 Rust 能力翻译成 Python 形状 | 这里出现 `for` 循环做终端逻辑了吗？→ 该下沉 |
| `render/` | 网格 → 字节 | 这里出现 pty / Python 了吗？→ 该上移 |
| `host/` | 一个终端宿主单元怎么活、怎么死 | 这里出现「多个终端怎么摆」了吗？→ 不属于本库 |
| `term/` | 终端模型与它的派生状态 | 这里自己解析 VT 字节了吗？→ 不该 |
| `platform/` | 和 OS 打交道 | 这里认识 Terminal 了吗？→ 不该 |

---

## 3. 依赖规则

```
py/      →  host, term, render, platform, error, env
render/  →  term(grid), error
host/    →  term(model), platform, error
term/    →  wezterm_term / wezterm_cell, error
platform/→  error
error.rs, env.rs, input.rs  →  （无内部依赖）
```

**禁止清单：**

1. `term/` `render/` `host/` `platform/` 里出现 `use pyo3` —— 领域层不认识 Python；
2. `platform/` 里出现 `term` / `render` —— 平台层不认识终端；
3. `render/` 里出现 `host` / `platform` —— 渲染不认识进程；
4. **任何模块自己扫描 `\x1b` 字节流做输入侧解析** —— 解析只有 `wezterm_term` 一份
   （唯一例外见 §5.2：模式设置序列的**观察记录**）；
5. `py/` 里出现 `#[cfg(windows)]` 分支的**实现**（允许出现「该平台不支持」的报错分支，不允许出现 `unsafe` Win32 调用）。

---

## 4. 一个概念一个所有者 ★

这张表是本次重构的核心。左边是概念，中间是**唯一**该拥有它的目录，右边是现在散落在哪。

| 概念 | 唯一所有者 | 现在散落在 |
|---|---|---|
| VT 解析 | `wezterm_term`（不改） | + `term.rs::update_mode_state` 手写第二份 ✗ |
| 模式状态 | `term/model.rs`（**从模型取**） | `term.rs::TermModeState` 自持一份 ✗ |
| 网格单元 | `term/grid.rs` | `CellTuple` 匿名 10 元组，5 个模块按位置解构 ✗ |
| 颜色语义 | `term/grid.rs`（`Color` 枚举） | 3 个字符串解析器 ✗ |
| pty 生命周期 | `host/pty.rs` | `pty.rs` + `mux.rs` 各一份 ✗ |
| reader 线程 | `host/pane.rs` | `pty.rs` + `mux.rs` 各一份 ✗ |
| 背压 | `host/pane.rs` | 只有 `pty.rs` 有，Mux 完全没有 ✗ |
| 关闭协议 | `host/pane.rs` | `Pty::close` + `close_pane_inner` ✗ |
| 键鼠编码 | `term/encode.rs` | `term.rs` + `mux.rs` 共 4 份 ✗ |
| 可见窗口计算 | `term/view.rs` | 每处 `total - offset` 各算一遍 ✗ |
| 选区 | `term/selection.rs` | 已经是单一实现 ✓ |
| 增量渲染 | `render/surface.rs` | `surface_render.rs` ✓（但只有 Mux 能用） |
| 全量 ANSI | `render/ansi.rs` | `term.rs::render_ansi` ✓（但绑死在 Terminal 上） |
| 帧合成 | 随复用器一并移除 | `mux.rs` 内联 ✓ |
| 布局失效 | 随复用器一并移除 | 8 处手写 `last_seqno = None` ✗ |
| pane 身份 | 随复用器一并移除 | `Vec` 下标（关掉 0 号，1 号变 0 号）✗ → 已无此概念 |
| 资源位置 | `env.rs` | `__file__`（pty.rs）+ **CWD**（font.rs）✗ |
| 平台能力 | `platform/*` | `#[cfg(windows)]` 散在 5 个业务模块 ✗ |
| 错误类型 | `error.rs` | 三种风格混用（PyResult / 静默 / panic）✗ |

---

## 5. 目录要承载的设计决策

### 5.1 `host/pane.rs` —— 唯一的宿主单元

**现状（重构前）**：`PyPty`+`PyTerminal` 一套，`Mux::PaneInner` 另一套，已经分叉（背压、模式跟踪、导出能力只在一套里有）。

**重构后**：一个 `Pane` 类型 = `pty + term::Model + 视口 + 选区 + reader 线程 + 背压缓冲 + 关闭协议`。
- `py/pty.rs` 与 `py/terminal.rs` 是它的薄壳（`Pty` 只暴露 pty 面，`Terminal` 只暴露模型面，同一个 `Pane`）；
- reader 只把字节放进读缓冲，**谁喂模型、谁回应答由调用方决定** —— 宿主不替调用方解释字节流；
- 背压、模式查询、渲染导出对两边同时生效。

### 5.2 `term/model.rs` —— 模式状态由一个**正确的**增量扫描器独占

**现状（重构前）**：`update_mode_state` 在字节流里扫 `ESC [ ... h/l`，且只看每块末尾 64 字节
（块大于窗口时前面的序列全部漏掉）。而 `wezterm_term::TerminalState` 已经存了
`mouse_tracking` / `button_event_mouse` / `any_event_mouse` / alt screen / bracketed paste /
focus / keyboard encoding，只是 `is_mouse_grabbed()` 只返回布尔 OR、拿不到模式号。

**重构后**：`ModeScanner` 是一个逐字节推进的显式状态机（跨 chunk 只保留当前状态，不依赖任何
窗口大小），且它是本库 API 里模式状态的**唯一来源** —— `is_mouse_grabbed()` /
`bracketed_paste_enabled()` / `is_alt_screen_active()` / `get_mouse_encoding()` /
`mode_restore_seq()` 全部读它，因此它们不可能互相矛盾。

**为什么没有改成「读模型」**（这是与初版计划不同的一处，理由记在这里）：

1. `mode_restore_seq` 需要区分「应用**显式设置过**」与「应用没碰过」。模型只存 `bool`（带默认值），
   拿不到这个区分；而恢复时用「我们以为的默认值」去发序列，会覆盖客户端自己的配置
   （例如光标闪烁）—— 这正是原实现刻意做成 `Option` 的原因。
2. 模型内部的模式状态是**模型的**事（它自己要用它决定鼠标编码），我们暴露的是「观察到应用设置过
   哪些模式」，这是本库的概念，归本库所有。
3. 走「改 vendored wezterm 加访问器」这条路需要动 `wezterm\term` 十余处，且仍然解决不了第 1 点。

**判据**：`term/model.rs` 不解析**输入**字节流（解析归 `wezterm_term`）；它只维护
「模式设置序列的观察记录」，这是唯一一处例外，且被测试钉住。

### 5.3 `term/grid.rs` —— 具名 `Cell` 取代匿名元组

**现状（重构前）**：`CellTuple = (usize, String, String, String, bool×5, usize)`，5 个模块按位置解构。

**重构后**：
```rust
pub struct Cell { pub col: usize, pub text: String, pub fg: Color, pub bg: Color, pub attrs: Attrs, pub width: u8 }
pub enum Color { Default, Palette(u8), Rgb(u8,u8,u8) }
```
颜色语义成为类型，三个字符串解析器收敛成一个（且只在 `py/` 边界做字符串转换）。

### 5.4 `render/` —— 按阶段切，而不是按类切

**现状（重构前）**：要增量必须用 `Mux`、要导出 SVG/图片必须用 `Terminal`，两者兼得没有路。

**重构后**：两条正交的轴，各自有独立出口——
- `模型 → 网格快照`（`term/view.rs`）
- `网格 → {ansi 全量, svg, image}`（`render/{ansi,svg,pixmap}`）
- 增量 diff 独立成层（`render/surface.rs`），**两边都能挂**。

于是 `Terminal` 既能导出 SVG/图片，也能接增量（`Surface` 是独立的 Python 类）。

### 5.5 `host/process.rs` —— 句柄用不透明类型（**未做**）

**现状**：`spawn` 返回 `(pid, handle)`，`hpcon()` / `child_handle()` 返回 `usize`，`job_handle` 收 `usize`；所有权靠文档约定。

**目标**：`ProcessHandle` / `Hpcon` / `JobObject` 不透明包装，`Drop` 管理生命周期，要整数时显式 `.raw()`。让「用错」写不出来。

这是**尚未落地**的一项，留在这里作为后续工作的起点。

### 5.6 `error.rs` —— 一种错误模型

**现状（重构前）**：`PyRuntimeError` + 中文串 / 静默成功 / `panic`+`expect` 三种混用。

**重构后**：
```rust
pub enum Error { Pty(anyhow::Error), Closed, Invalid(String), Render(String), Platform(String) }
```
→ 映射到 `RuntimeError` 子类（`TerminalClosed` / `RenderError` / `PlatformUnsupported`）+ `ValueError`。
宿主可以按类型处理，而不是匹配中文字符串。best-effort 的场合（剪贴板失败）显式写进文档契约。
变体按「调用方需要怎么处理」划分，不留无人构造的死变体。

### 5.7 `env.rs` —— 问自己，不问别人

**现状（重构前）**：`ensure_conpty_dir` 从 `pywezterm.__file__` 反推；`font.rs` 从**进程 CWD** 找字体（该路径已彻底失效）。

**重构后**：模块加载时用**扩展模块自身的磁盘位置**（Windows `GetModuleFileNameW` 拿 `.pyd` 路径；POSIX `dladdr`）算一次，存 `OnceLock`。对外给 `pywezterm.env_info()`（模块目录 / sidecar 路径 / 是否生效），把「侧载 conpty 到底生效没有」从静默变成可查。

### 5.8 `platform/` —— 平台分支收口

**现状（重构前）**：`#[cfg(windows)]` 散在 5 个业务模块，混着两种语义：「Windows 才有这个能力」（job object / HPCON / console input）和「非 Windows 先占位」（剪贴板返回空、`child_handle` 返回 0、取消阻塞读不存在）。

**重构后**：同名接口各平台各自实现。业务模块里不再出现 `cfg`，非 Windows 从「能编译」变成「被设计过」；
**确实不存在**的能力（剪贴板、控制台输入）在该平台不注册，调用方看到的是「类不存在」而不是「类存在但永远失败」。

---

## 6. 强制执行：让分层变成机制，而不是纪律

两个选项：

### 选项 A —— 单 crate，模块分层

就是上面那棵树，全在 `src/` 下。零构建改动，一次 PR 落地。

分层靠：CI 加一条断言（第 3 节的禁止清单 1–3），例如

```bash
! grep -rn "use pyo3" wezterm/pywezterm/src/{term,render,host,platform}
! grep -rn "wezterm_term\|crate::term" wezterm/pywezterm/src/platform
```

### 选项 B —— 拆 `pywezterm-core`（**已采用**）

```
wezterm/
├── pywezterm-core/     # rlib，纯 Rust，不依赖 pyo3
│   └── src/{host,term,render,platform,error,env,input}
└── pywezterm/          # cdylib，只有绑定壳
    └── src/py/*.rs
```

`use pyo3::` 写在 core 里 = **编译不过**。分层从「约定」变成「机制」，不需要 CI grep 断言。

**代价（已付）**：把 `pywezterm-core` 加进 `wezterm/Cargo.toml` 的 `workspace.members`
（位于 workspace 目录树内的包必须显式列名），`wezterm/Cargo.lock` 随之更新 —— 已记入 AGENTS.md。

**顺带解决**：Rust 单测终于能独立运行。原来 `.github/workflows/ci.yml` 只有 maturin build + pytest，
**没有 `cargo test`**，`#[cfg(test)]` 那批从未在 CI 里执行过（编译它们才发现 2 处 panic 消息在
edition 2018 下不做格式化插值的告警）。现在 `cargo test -p pywezterm-core` 可独立运行，
不依赖 Python 环境；**建议在 CI 里加这一步**。

---

## 7. 迁移顺序（五刀）—— 已全部落地

| 刀 | 内容 | 结果 |
|---|---|---|
| **1** | 建目录、拆 crate、按层搬移 | ✅ |
| **2** | 消重：`Cell`/`Color` 取代元组；模式扫描器改为正确的增量状态机；键鼠编码收敛到 `term/encode.rs` | ✅ 分块不变性测试覆盖 |
| **3** | **Pane 合一**：`host/pane.rs` 成为唯一宿主单元 | ✅ `Pty` 与 `Terminal` 都建在它上面 |
| **4** | 渲染按阶段切：`render/` 统一出口 | ✅ |
| **5** | 拆 crate | ✅ |

**验证**：`cargo test -p pywezterm-core` 93 通过；`pytest tests/` 83 通过 4 跳过；两个 crate 零告警。

> ⚠️ 跑 Python 用例时必须让**本地构建**优先：若环境里存在 `vendor/pywezterm` 之类的另一份副本，
  它可能先于仓库被导入，于是测的是旧代码（这个坑实际踩到过）。`import pywezterm` 前把仓库根插到
  `sys.path` 最前，并断言 `hasattr(pywezterm, "env_info")` 即可确认。

---

## 8. 测试布局

```
wezterm/pywezterm-core/src/**/*.rs   纯逻辑单测（不碰 pty / Python），可独立运行
tests/*.py                           端到端（含不变量与失败路径）
```

三类不变量（`tests/test_refactor_invariants.py` 与 core 单测）：

1. **分块不变性** —— 同一段字节流按 `1 / 7 / 63 / 64 / 65 / 4096` 字节切分喂入，断言
   `text()` / `snapshot()` / **模式状态** / `mode_restore_seq()` 完全一致。窗口式实现正是在这里出错。
2. **幂等性** —— `close()` 两次、`resize` 同尺寸。
3. **失败路径** —— `render_image` 非法 `scale` 报错而非 panic；`Surface.clear()` 之后全量重绘不得把
   旧内容画回来；非法键名报 `ValueError`。

---

## 9. 已拍板的设计选择

1. **拆 `pywezterm-core`** —— 已采用（第 6 节选项 B）。
2. **模式状态自持**（不读模型）—— 理由见 §5.2：`mode_restore_seq` 需要「应用是否显式设置过」，
   模型只存 `bool` 给不出这个区分。扫描器改为正确的增量状态机，并成为本库 API 模式状态的唯一来源。
3. **不提供复用器** —— 见 §10。

---

## 10. 复用器（`mux`）已移出本库

**结论**：`Mux` 是这一层里唯一的**应用策略**，不是基础设施，已从库中整体删除。

**为什么**：

- 「把终端模型渲染成增量 ANSI」是原语（`Surface` 就是，它有自己的 Python 出口，不依赖 `Mux`）；
  「屏幕切成两半、左边放 A、右边放 B、第 40 列画竖线、底下一行放状态栏、鼠标命中哪个矩形就发给谁」
  是策略。库不该替调用方决定这些。
- **证据在 API 自己身上**：`Mux.render()` 同时返回字节流和 0-based 的光标坐标，而字节流内部的 CUP
  是 1-based，文档里不得不写「二者并存，调用方注意区分」。需要写下这句话，就说明有两个人在管同一块
  屏幕 —— 库在画，宿主也在画。
- 还有几个只在应用层成立的东西也长在库里：状态栏文本、分隔线列号、子进程原始输出录制（asciicast）。
  终端引擎不该知道「录制」这件事。
- 「一个通用库只支持左右二分、且只支持 2 个窗格」本身也说明它是一个具体应用的需要。

**删掉了什么**（约 1300 行，当时占总量的 18%）：

| 位置 | 内容 |
|---|---|
| `pywezterm-core/src/mux/` | `mod`（Mux 编排）· `layout` · `compose` · `chrome` |
| `pywezterm/src/py/mux.rs` | `#[pyclass] Mux`（41 个公开方法） |
| `pywezterm-core/src/host/registry.rs` | pane 表与 `PaneId` |
| `host/pane.rs` 内的专属部件 | `Driver` 枚举与自驱动分支 · `OutputNotifier` · 录制缓冲（`take_output` / `output_len`）· `screen_to_stable` · `for_each_dirty_row` · ConPTY 重绘跳过 |
| `error.rs` / `py/error.rs` | `Error::NoSuchPane` 与 `PaneNotFound` 异常（无人构造的死变体） |
| 测试与文档 | `test_mux_*.py` · blackbox 套件第 5 节 · API 文档 §5 · README 相关章节 |

**没有删掉什么**：`Pane` 与 `Surface` 都不受影响。`Pane` 合一的价值本来就在 `Pty` 与 `Terminal` 之间
（那才是重构前真正分叉的两套实现），复用器只是第三个消费者；因此**移出 `mux` 不会让 §1–§5 的工作白做**。

**调用方要自己做**：布局、焦点路由、分隔线与状态栏。可用的原语是
`Terminal.render_ansi()`（整帧）与 `Surface`（增量帧）—— README 的「一个循环里跑多个终端」示例给了骨架。

**代码去向**：删除前的实现保留在 git 历史里（见引入本次重构的提交），需要时可按需取回。

