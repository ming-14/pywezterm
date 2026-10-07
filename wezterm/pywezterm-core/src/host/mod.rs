//! 宿主原语：一个终端宿主单元怎么活、怎么死。
//!
//! [`Pane`] 把「跑一个终端」需要的东西装在一起：pty 生命周期、终端模型、视口、
//! 选区、reader 线程、读缓冲、关闭协议。Python 的 `Pty` 与 `Terminal` 都建在它上面。
//!
//! 这里不含任何「多个终端怎么摆」的概念 —— 那属于调用方的 UI 层。

pub mod pane;
pub mod pty;

pub use pane::Pane;
pub use pty::Pty;
