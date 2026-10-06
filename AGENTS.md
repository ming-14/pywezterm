- 独立项目，不受上层项目影响
- 不要把使用方的项目名、需求或上下文写进这里（含代码注释与变更记录）
- 不要修改原wezterm：`wezterm\wezterm-char-props` `wezterm\wezterm-dynamic` `wezterm\wezterm-escape-parser` `wezterm\wezterm-input-types` `wezterm\wezterm-surface` `wezterm\Cargo.lock` `wezterm\Cargo.toml` `wezterm\LICENSE.md` `wezterm\bidi` `wezterm\color-types` `wezterm\filedescriptor` `wezterm\pty` `wezterm\target` `wezterm\term` `wezterm\termwiz` `wezterm\vtparse` `wezterm\wezterm-blob-leases` `wezterm\wezterm-cell`
- 如果一定一定无法避免要修改，或者是wezterm本身的bug需要修改，请将变更记录写到本文档

## 变更记录

### 新增 workspace 成员 pywezterm-core（wezterm\Cargo.toml + wezterm\Cargo.lock）
- 绑定层与领域实现拆成两个 crate：`pywezterm-core`（rlib，**不依赖 pyo3**）+ `pywezterm`
  （cdylib，只有绑定壳）。领域层不依赖 Python 由编译器保证，而不是靠约定
- 为此必须把 `pywezterm-core` 加进 `wezterm\Cargo.toml` 的 `workspace.members`
  （位于 workspace 目录树内的包必须显式列名，否则 cargo 拒绝加载），
  `wezterm\Cargo.lock` 随之更新
- 除此之外未改动 wezterm 自身任何代码

### raw 命令行支持（wezterm\pty\src\cmdbuilder.rs + pywezterm\src\pty.rs）
- 修改原 wezterm（wezterm\pty）以支持 raw 命令行
- `CommandBuilder` 新增 Windows 专属 `raw_cmdline` 字段与 `set_raw_cmdline()`：
  `cmdline()` 返回原样命令行（绕过 argv 引号序列化），供自解析命令行的程序
  （cmd.exe /c）保留其引号语义——argv 序列化的 `\"` 转义（C 运行时规则）在
  cmd.exe 中会变成字面反斜杠
- `pywezterm.Pty.spawn` 新增可选参数 `raw_cmdline`（Windows），透传到 CommandBuilder
- 原 `append_quoted` 保持 `\"` 转义不变（bash/python/node 等 C 运行时/POSIX 场景正确）

### Windows x86 栈破坏修复（wezterm\pty\src\win\psuedocon.rs + wezterm\pty\Cargo.toml）
- 修改原 wezterm（wezterm\pty）：`shared_library!` 宏生成 `extern "Rust"` 函数指针，
  与 Win32 API 的 `stdcall` 调用约定不匹配，在 32 位 Windows 上调用
  `CreatePseudoConsole` 时栈破坏（segfault）；64 位恰好兼容未暴露。
- 改为手动 `LoadLibraryW`/`GetProcAddress` 解析 ConPTY 函数，函数指针类型为
  `extern "system"`（x86 = stdcall，x64 = C，正确匹配 Win32 API）。
- `wezterm\pty\Cargo.toml`：移除不再使用的 `shared_library` 依赖，winapi 补
  `libloaderapi`/`winbase`/`wincon`/`minwinbase` features。

### 创建时即入作业对象（wezterm\pty\src\cmdbuilder.rs + win\procthreadattr.rs + win\psuedocon.rs + pywezterm\src\pty.rs）
- 修改原 wezterm（wezterm\pty）以支持把子进程**直接创建进作业对象**
- `CommandBuilder` 新增 Windows 专属 `job_handle` 字段与 `set_job_handle()` /
  `get_job_handle()`，与既有 `raw_cmdline` 同一模式
- `ProcThreadAttributeList` 新增 `set_job_list()`：走
  `PROC_THREAD_ATTRIBUTE_JOB_LIST`（`0x0002000D`），接受作业句柄数组
- `spawn_command` 按是否带作业决定属性列表容量（2 / 1）并写入该属性。子进程在
  `CreateProcessW` 时即入作业，**消除"创建后再 `AssignProcessToJobObject`"的时间窗**
  ——后者存在竞态：子进程在被赋值前 fork 出的孙进程不进作业，从此既枚举不到也杀不到
- `pywezterm.Pty.spawn` 新增可选参数 `job_handle`（Windows），透传到 CommandBuilder
- 作业句柄由调用方创建、持有并关闭，本库不参与其生命周期；未提供时行为与从前一致
- **踩坑**：`UpdateProcThreadAttribute` 对"值是数据指针"的属性**只登记指针、不复制数据**，
  该数据必须活到 `CreateProcess` 返回为止。最初直接把调用方临时切片的地址传进去，函数一
  返回指针即悬空，随后 `cmdline()` / `environment_block()` 的分配复用了那块内存，
  `CreateProcess` 读到垃圾句柄并报 `ERROR_INVALID_HANDLE`（而 `UpdateProcThreadAttribute`
  本身返回成功，极具迷惑性）。现由 `ProcThreadAttributeList` 自己持有 `job_handles`
  保证其生命周期。
