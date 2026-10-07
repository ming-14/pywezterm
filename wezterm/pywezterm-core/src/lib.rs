//! pywezterm-core —— 终端宿主：pty 引擎、终端模型、渲染。
//!
//! 分层（依赖单向，下层不认识上层）：
//!
//! - [`platform`] 平台原语：只和 OS 打交道，不认识终端
//! - [`term`] 终端领域：模型与它的派生状态
//! - [`host`] 宿主原语：一个终端宿主单元（[`host::Pane`]）
//! - [`render`] 渲染：网格 → 字节
//!
//! 本 crate 不依赖 pyo3：Python 绑定在 `pywezterm` crate 的 `py` 模块里。
//!
//! 本 crate 只提供「一个终端」的原语。多个终端怎么摆、事件发给谁，属于调用方的 UI 层。

pub mod env;
pub mod error;
pub mod host;
pub mod input;
pub mod platform;
pub mod render;
pub mod term;

pub use error::{Error, Result};
