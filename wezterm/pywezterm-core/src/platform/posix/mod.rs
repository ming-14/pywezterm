//! 非 Windows 平台原语。
//!
//! 只实现本平台**确实存在**的能力：
//!
//! - [`self_module_dir`]：`dladdr` 反查 `.so` 路径
//! - [`conpty`]：恒空（ConPTY 是 Windows 专有，其他平台由内核提供 pty）
//! - [`reader_cancel`]：无需取消（关闭 master 即可解除 reader 阻塞）
//!
//! 宿主剪贴板与控制台输入采集**不在此列**：前者需要连接 X11/Wayland 会话，后者依赖
//! Win32 控制台的 `ReadConsoleInputW`，都不是本库该提供的抽象。对应绑定在
//! `pywezterm` 里按平台注册，调用方看到的是「类不存在」而不是「类存在但永远失败」。

pub mod conpty;
pub mod reader_cancel;
mod self_dir;

pub use self_dir::self_module_dir;
