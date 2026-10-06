//! 宿主原语：一个终端宿主单元怎么活、怎么死。
//!
//! [`Pane`] 是全库唯一的宿主实现 —— Python 的 `Pty` / `Terminal` 与复用器的每个窗格
//! 都是它。这里只放「pty 生命周期、reader 线程、读缓冲、关闭协议、身份管理」，
//! 布局与帧合成属于 [`crate::mux`]。

pub mod pane;
pub mod pty;
pub mod registry;

pub use pane::{Driver, OutputNotifier, Pane};
pub use pty::Pty;
pub use registry::{PaneId, Registry};
