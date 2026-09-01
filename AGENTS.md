- 独立项目，不受上层项目影响
- 不要在该项目任何地方提到 PTY-Agent
- 不要修改原wezterm：`wezterm\wezterm-char-props` `wezterm\wezterm-dynamic` `wezterm\wezterm-escape-parser` `wezterm\wezterm-input-types` `wezterm\wezterm-surface` `wezterm\Cargo.lock` `wezterm\Cargo.toml` `wezterm\LICENSE.md` `wezterm\bidi` `wezterm\color-types` `wezterm\filedescriptor` `wezterm\pty` `wezterm\target` `wezterm\term` `wezterm\termwiz` `wezterm\vtparse` `wezterm\wezterm-blob-leases` `wezterm\wezterm-cell`
- 如果一定一定无法避免要修改，或者是wezterm本身的bug需要修改，请将变更记录写到本文档

## 变更记录

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