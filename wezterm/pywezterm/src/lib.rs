//! pywezterm —— wezterm 终端引擎的 Python 绑定。
//!
//! 本 crate 只做绑定：伪终端、终端模型、增量渲染的实现全在
//! `pywezterm-core`，那个 crate 不依赖 pyo3，因此可以独立编译与测试。
//!
//! 分层见 `pywezterm-core` 的模块文档；绑定壳的职责边界见 [`py`]。

mod py;

use pyo3::prelude::*;

/// 模块初始化。
///
/// 领域资源（ConPTY 侧载二进制的定位与启用）在这里一次性确定 —— 不推迟到第一次
/// 建 pty，否则行为会依赖调用顺序。
#[pymodule]
fn pywezterm(m: &Bound<'_, PyModule>) -> PyResult<()> {
    pywezterm_core::env::init();
    py::register(m)
}
