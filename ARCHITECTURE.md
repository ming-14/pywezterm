# 架构与分层（重构基线）

> 这份文档定的是**重构后的目标结构**。问题清单见 `REVIEW-pywezterm-crate.md`；本文只回答「东西应该放在哪、为什么」。
>
> 一句话概括目标：**每个概念只有一个家，依赖只有一个方向。**

---

## 0. 落地状态

**已落地**（`pywezterm-core` + `pywezterm` 双 crate，93 个 Rust 单测 + 83 个 Python 用例全绿，两个 crate 零告警）：

| 项 | 状态 |
|---|---|
| 拆 crate：`pywezterm-core`（无 pyo3）+ `pywezterm`（仅绑定壳） | ✅ 编译器强制分层 |
| 目录分层 `py / mux / render / host / term / platform` | ✅ |
| `Pane` 合一（`host/pane.rs` 是唯一宿主实现） | ✅ `Pty` / `Terminal` / 复用器窗格都是它 |
| 模式状态收敛为正确的增量扫描器 | ✅ 分块不变性测试覆盖（见 §5.2 的取舍说明） |
| `Cell` / `Color` / `Attrs` 取代匿名元组 | ✅ 三个颜色解析器收敛为一个 |
| 渲染按阶段切（`render::ansi` / `surface` / `svg` / `pixmap`） | ✅ |
| 布局版本收口到 `State::relayout` | ✅ 取代 8 处手写失效 |
| pane 身份用单调 id（不复用） | ✅ 关闭一个不影响其他 |
| `error.rs` 统一错误 + 异常层次 | ✅ |
| `env.rs` 由模块自身路径定位资源 | ✅ 不再依赖 `__file__` / CWD |
| `platform/` 收口平台分支 | ✅ 非 Windows 为**设计过的**实现而非占位 |
| 分块不变性 / 失败路径 / 幂等性测试 | ✅ `tests/test_refactor_invariants.py` |

**刻意未做**：

- **跨窗格帧原子性**（§5.5）—— 见该节的取舍说明。
- **非 Windows 的宿主剪贴板与控制台输入**：前者需要连接 X11/Wayland 会话，后者依赖 Win32
  控制台，都不是本库该提供的抽象；对应绑定按平台注册，调用方看到的是「类不存在」而不是
  「类存在但永远失败」。
- **随包分发字体**：原来的 `src/assets/fonts` 候选路径在仓库里根本不存在（也没有下载步骤），
  属于死代码，已删除；只用系统字体。

---

## 1. 两条分层原则

**原则一：依赖单向，不允许回指。**
`py → mux → render/host → term → platform`。上层可以调下层，下层不知道上层存在。尤其：**领域层（term/render/host）不认识 Python，平台层（platform）不认识终端。**

**原则二：一个概念只有一个所有者。**
这是本次重构真正要解决的东西。审查报告里的 P0 缺陷几乎都是同一件事的不同表现——同一个概念被实现了两遍，然后分叉：

- `Pane` 有两套（`pty.rs` + `mux.rs`）→ A2 只在第一套、A3 只在第二套；
- VT 序列解析有两套（`wezterm_term` + `term.rs::update_mode_state`）→ 64 字节窗口 bug；
- 颜色解析有三套（`color_attr_to_string` / `resolve_color` / `parse_color`）→ 语义不一致；
- 键鼠编码有四份（`term.rs` ×2 + `mux.rs` ×2）；
- 关闭协议有两份。

**所以目录不是审美问题。** 目录结构是「谁拥有什么」的物理体现；下面的第 4 节把每个概念指到唯一一个目录。

---

## 2. 目录树（目标）

```
wezterm/pywezterm/
├── Cargo.toml
├── build.rs                    # 只做一件事：把 assets/windows/conhost 交给打包
└── src/
    ├── lib.rs                  # 只剩 mod 声明 + pymodule 注册，零逻辑
    │
    ├── py/                     # L5 绑定壳：只做签名 / 默认值 / 类型转换
    │   ├── mod.rs
    │   ├── error.rs            # Error → PyErr（异常层次）
    │   ├── pty.rs              # #[pyclass] Pty        → host::Pane
    │   ├── terminal.rs         # #[pyclass] Terminal   → host::Pane + term::*
    │   ├── surface.rs          # #[pyclass] Surface    → render::surface
    │   ├── mux.rs              # #[pyclass] Mux        → mux::*
    │   ├── console_input.rs    # #[pyclass] ConsoleInput → platform::console_input
    │   ├── clipboard.rs        # 两个 #[pyfunction]     → platform::clipboard
    │   └── callbacks.rs        # Python 回调 → wezterm_term 的 handler trait 适配
    │
    ├── mux/                    # L4 复用器编排
    │   ├── mod.rs              # Mux：pane 表 + 焦点 + 路由
    │   ├── layout.rs           # 布局树 + 矩形 + 布局版本号
    │   ├── compose.rs          # 帧合成（按帧序号取各 pane 快照 → Surface）
    │   └── chrome.rs           # 分隔线 + 状态栏
    │
    ├── render/                 # L3 渲染：只吃网格，不认识 pty / Python
    │   ├── mod.rs              # 统一出口：grid → {ansi, svg, image}
    │   ├── ansi.rs             # 全量 ANSI（原 term.rs::render_ansi）
    │   ├── surface.rs          # 增量（原 surface_render.rs）
    │   ├── svg.rs
    │   ├── pixmap.rs
    │   └── font.rs
    │
    ├── host/                   # L1 宿主原语：唯一的「一个终端宿主单元」
    │   ├── mod.rs
    │   ├── pane.rs             # Pane：pty + 模型 + reader + 背压 + capture + 关闭协议
    │   ├── reader.rs           # reader 线程循环
    │   ├── pty.rs              # pty 生命周期（openpty / spawn / resize / close）
    │   ├── process.rs          # 进程句柄 / job object / HPCON 的不透明包装
    │   └── registry.rs         # 世代索引 pane 表
    │
    ├── term/                   # L2 终端领域
    │   ├── mod.rs
    │   ├── grid.rs             # Cell / Color / Attrs（替代 CellTuple）
    │   ├── model.rs            # wezterm_term::Terminal 封装 + 模式查询
    │   ├── view.rs             # view_offset / 可见窗口计算
    │   ├── encode.rs           # 键鼠编码（唯一一份）
    │   └── selection.rs        # 选区状态机（原样搬）
    │
    ├── platform/               # L0 平台原语：只知道 OS，不知道终端
    │   ├── mod.rs              # 同名接口（各平台各自实现）
    │   ├── windows/
    │   │   ├── conpty.rs       # 侧载目录解析 + 生效自省
    │   │   ├── reader_cancel.rs# CancelSynchronousIo
    │   │   ├── process.rs      # job object / HPCON
    │   │   ├── console_input.rs
    │   │   └── clipboard.rs
    │   └── posix/
    │       ├── reader_cancel.rs# 关 SA_RESTART + pthread_kill
    │       └── process.rs      # pidfd
    │
    ├── error.rs                # 统一 Error enum
    └── env.rs                  # 自身资源位置 + env_info() 自省
```

**每个目录一句话职责：**

| 目录 | 一句话 | 判据（写代码时自问） |
|---|---|---|
| `py/` | 把 Rust 能力翻译成 Python 形状 | 这里出现 `for` 循环做终端逻辑了吗？→ 该下沉 |
| `mux/` | 多个 pane 怎么摆、焦点给谁、一帧怎么拼 | 这里出现 `\x1b` 了吗？→ 该下沉 |
| `render/` | 网格 → 字节 | 这里出现 pty / Python 了吗？→ 该上移 |
| `host/` | 一个终端宿主单元怎么活、怎么死 | 这里出现布局 / 合成了吗？→ 该上移 |
| `term/` | 终端模型与它的派生状态 | 这里自己解析 VT 字节了吗？→ 不该 |
| `platform/` | 和 OS 打交道 | 这里认识 Terminal 了吗？→ 不该 |

---

## 3. 依赖规则

```
py/      →  mux, host, term, render, platform, error, env
mux/     →  host, term, render, error
render/  →  term(grid), error
host/    →  term(model), platform, error
term/    →  wezterm_term / wezterm_cell, error
platform/→  error
error.rs, env.rs  →  （无内部依赖）
```

**禁止清单（可写成 CI 断言，见第 6 节）：**

1. `term/` `render/` `host/` `platform/` 里出现 `use pyo3` —— 领域层不认识 Python；
2. `platform/` 里出现 `term` / `render` / `mux` —— 平台层不认识终端；
3. `render/` 里出现 `host` / `platform` —— 渲染不认识进程；
4. **任何模块自己扫描 `\x1b` 字节流做输入侧解析** —— 解析只有 `wezterm_term` 一份；
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
| reader 线程 | `host/reader.rs` | `pty.rs` + `mux.rs` 各一份 ✗ |
| 背压 | `host/pane.rs` | 只有 `pty.rs` 有，Mux 完全没有 ✗ |
| 关闭协议 | `host/pane.rs` | `Pty::close` + `close_pane_inner` ✗ |
| 键鼠编码 | `term/encode.rs` | `term.rs` + `mux.rs` 共 4 份 ✗ |
| 可见窗口计算 | `term/view.rs` | 每处 `total - offset` 各算一遍 ✗ |
| 选区 | `term/selection.rs` | 已经是单一实现 ✓ |
| 增量渲染 | `render/surface.rs` | `surface_render.rs` ✓（但只有 Mux 能用） |
| 全量 ANSI | `render/ansi.rs` | `term.rs::render_ansi` ✓（但绑死在 Terminal 上） |
| 帧合成 | `mux/compose.rs` | `mux.rs` 内联 ✓ |
| 布局失效 | `mux/layout.rs`（版本号） | 8 处手写 `last_seqno = None` ✗ |
| pane 身份 | `host/registry.rs` | `Vec` 下标（关掉 0 号，1 号变 0 号）✗ |
| 资源位置 | `env.rs` | `__file__`（pty.rs）+ **CWD**（font.rs）✗ |
| 平台能力 | `platform/*` | `#[cfg(windows)]` 散在 5 个业务模块 ✗ |
| 错误类型 | `error.rs` | 三种风格混用（PyResult / 静默 / panic）✗ |

---

## 5. 目录要承载的设计决策

### 5.1 `host/pane.rs` —— 唯一的宿主单元

**现状**：`PyPty`+`PyTerminal` 一套，`Mux::PaneInner` 另一套，已经分叉（背压、模式跟踪、导出能力只在一套里有）。

**目标**：一个 `Pane` 类型 = `pty + term::Model + reader 线程 + 背压缓冲 + capture 回写 + 关闭协议`。
- `py/pty.rs` 与 `py/terminal.rs` 是它的薄壳（`Pty` 只暴露 pty 面，`Terminal` 只暴露模型面，同一个 `Pane`）；
- `Mux` 持有 `Vec<Pane>`，不再自己实现 reader / close；
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

**现状**：`CellTuple = (usize, String, String, String, bool×5, usize)`，5 个模块按位置解构。

**目标**：
```rust
pub struct Cell { pub col: usize, pub text: ..., pub fg: Color, pub bg: Color, pub attrs: Attrs, pub width: u8 }
pub enum Color { Default, Palette(u8), Rgb(u8,u8,u8) }
```
颜色语义成为类型，三个字符串解析器收敛成一个（且只在 `py/` 边界做字符串转换）。

### 5.4 `render/` —— 按阶段切，而不是按类切

**现状**：要增量必须用 `Mux`、要导出 SVG/图片必须用 `Terminal`，两者兼得没有路。

**目标**：两条正交的轴——
- `模型 → 网格快照`（`term/view.rs`）
- `网格 → {ansi 全量, svg, image}`（`render/{ansi,svg,pixmap}`）
- 增量 diff 独立成层（`render/surface.rs`），**两边都能挂**。

于是 `Mux` 的 pane 天然支持导出，`Terminal` 也能接增量。

### 5.5 `mux/compose.rs` —— 单窗格原子，跨窗格不追求原子

**现状**：`render()` 是「取 A 锁→合成 A→取 B 锁→合成 B」，两个 pane 在不同时刻取的。

**目标**：**每个窗格在自己的模型锁内完成合成**，因此单个窗格的内容一定是同一时刻的。

**取舍**：跨窗格不追求同一时刻。那需要在整帧期间同时持有所有窗格的锁，与「按窗格加锁」的设计
直接冲突，而收益只是消除一个亚毫秒级的采样差 —— 不值得。写在这里是为了让后来的人知道它是
**有意的**，不是漏掉了。

### 5.6 `mux/layout.rs` —— 布局版本号

**现状**：布局变更后要靠 8 处手写 `last_seqno = None` / `last_sep = None` 来失效（`set_sep` / `set_split_col` / `set_status_rows` / `set_status` / `resize` / `force_repaint` / `close_pane` / `add_pane`）。目前**都写对了**，但正确性依赖「记得写」。

**目标**：布局结构自带版本号，`compose` 比对版本；所有改布局的入口收进一个 `layout_mut()` 守卫，Drop 时自动递增版本。让「忘记失效」写不出来。

### 5.7 `host/registry.rs` —— pane 身份不是下标

**现状**：`close_pane(0)` 之后 `panes[1]` 变成 `panes[0]`，宿主缓存的 id 指向另一个 pane。

**目标**：世代索引（`slotmap` 已在依赖树里）。id 永久有效；关闭后查询返回明确的「不存在」，而不是打到别人身上。

### 5.8 `host/process.rs` —— 句柄用不透明类型，不用裸 `usize`

**现状**：`spawn` 返回 `(pid, handle)`，`hpcon()` / `child_handle()` 返回 `usize`，`job_handle` 收 `usize`；所有权靠文档约定。

**目标**：`ProcessHandle` / `Hpcon` / `JobObject` 不透明包装，`Drop` 管理生命周期，要整数时显式 `.raw()`。让「用错」写不出来。

### 5.9 `error.rs` —— 一种错误模型

**现状**：`PyRuntimeError` + 中文串 / 静默成功 / `panic`+`expect` 三种混用。

**目标**：
```rust
pub enum Error { Pty(io::Error), Closed, NoSuchPane(PaneId), Render(String), Platform(String) }
```
→ 映射到 `PyWeztermError` 子类（`TerminalClosed` / `PaneNotFound` / …）。宿主可以按类型处理，而不是匹配中文字符串。best-effort 的场合（剪贴板失败）显式写进文档契约。

### 5.10 `env.rs` —— 问自己，不问别人

**现状**：`ensure_conpty_dir` 从 `pywezterm.__file__` 反推；`font.rs` 从**进程 CWD** 找字体（该路径已彻底失效）。

**目标**：模块加载时用**扩展模块自身的磁盘位置**（Windows `GetModuleFileNameW` 拿 `.pyd` 路径；POSIX `dladdr`）算一次，存 `OnceLock`。对外给 `pywezterm.env_info()`（sidecar 路径 / 是否生效 / 字体来源），把「侧载 conpty 到底生效没有」从静默变成可查。

### 5.11 `platform/` —— 平台分支收口

**现状**：`#[cfg(windows)]` 散在 5 个业务模块，混着两种语义：「Windows 才有这个能力」（job object / HPCON / console input）和「非 Windows 先占位」（剪贴板返回空、`child_handle` 返回 0、取消阻塞读不存在）。

**目标**：同名接口各平台各自实现（Linux 的取消阻塞读用「关 `SA_RESTART` + `pthread_kill`」，进程句柄用 pidfd）。业务模块里不再出现 `cfg`，非 Windows 从「能编译」变成「被设计过」。

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
│   └── src/{host,term,render,mux,platform,error,env,input}
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
| **3** | **Pane 合一**：`host/pane.rs` 成为唯一宿主单元 | ✅ `Pty`/`Terminal`/窗格都建在它上面，差异只在 `Driver` 与是否带 pty |
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
2. **幂等性** —— `close()` 两次、`close_pane` 两次、`resize` 同尺寸、`set_sep` 同值。
3. **失败路径** —— `add_pane` spawn 失败后 Mux 仍可用；`render_image` 非法 `scale` 报错而非 panic；
   `Surface.clear()` 之后全量重绘不得把旧内容画回来；非法键名报 `ValueError`。

---

## 9. 已拍板的设计选择

1. **拆 `pywezterm-core`** —— 已采用（第 6 节选项 B）。
2. **模式状态自持**（不读模型）—— 理由见 §5.2：`mode_restore_seq` 需要「应用是否显式设置过」，
   模型只存 `bool` 给不出这个区分。扫描器改为正确的增量状态机，并成为本库 API 模式状态的唯一来源。
3. **pane id 用单调 id + `HashMap`** —— 不复用即不会误指；未引入 `slotmap`（多一个依赖换来的
   世代校验在这里用不上，id 单调已经保证「要么有效、要么明确不存在」）。
4. **`Mux` 仍限制 2 个窗格** —— 这是能力边界而非实现限制：`layout::rects` 是数据驱动的，
   放开 N 叉只需改那一个函数。之所以没顺手放开，是因为「最多 2 个」是当前文档化的对外契约，
   改它属于功能变更而不是重构。

