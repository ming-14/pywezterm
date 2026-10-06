//! 平台原语：只和操作系统打交道，不认识终端语义。
//!
//! 业务层（`host` / `term` / `render` / `mux`）里不出现 `#[cfg(windows)]`：
//! 同名接口在这里按平台分派，各平台各自实现。
//!
//! | 接口 | 用途 | 各平台 |
//! |---|---|---|
//! | `self_module_dir` | 扩展模块自身所在目录 —— 随包资源的位置基准 | `GetModuleFileNameW` / `dladdr` |
//! | `conpty` | ConPTY 侧载二进制的解析与启用 | Windows 有侧载；其他平台恒空 |
//! | `reader_cancel` | 取消 reader 线程的阻塞读 | Windows 需要；其他平台关 master 即可 |
//! | `clipboard` | 宿主剪贴板 | 仅 Windows |
//! | `console_input` | 宿主控制台输入采集 | 仅 Windows |

#[cfg(unix)]
mod posix;
#[cfg(unix)]
pub use posix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{clipboard, conpty, console_input, reader_cancel, self_module_dir};
