//! 绑定壳：把 `pywezterm-core` 的能力翻译成 Python 形状。
//!
//! 本层的职责只有四件事：签名与默认值、类型转换、GIL 管理、错误映射。
//! **任何终端逻辑都不应该出现在这里** —— 出现了就说明它该下沉到 core。
//!
//! GIL 规则：凡是会取 core 内部锁的调用，一律先 `py.detach`。否则会形成
//! 「Python 线程持 GIL 等锁 / reader 线程持锁等 GIL」的互等。

mod callbacks;
mod clipboard;
mod console_input;
mod convert;
mod error;
mod pty;
mod surface;
mod terminal;

use pyo3::prelude::*;

/// 注册模块内容。领域初始化（资源定位等）由调用方在此之前完成。
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    error::register(m)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(cursor_seq, m)?)?;
    m.add_function(wrap_pyfunction!(env_info, m)?)?;
    m.add_class::<pty::PyPty>()?;
    m.add_class::<terminal::PyTerminal>()?;
    m.add_class::<surface::PySurface>()?;

    #[cfg(windows)]
    {
        m.add_function(wrap_pyfunction!(clipboard::clipboard_read, m)?)?;
        m.add_function(wrap_pyfunction!(clipboard::clipboard_write, m)?)?;
        m.add_class::<console_input::PyConsoleInput>()?;
    }
    Ok(())
}

/// 绑定库版本。
#[pyfunction]
fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 光标定位序列：0-based 坐标 → 1-based CUP + 显示/隐藏。
#[pyfunction]
fn cursor_seq(row: usize, col: usize, visible: bool) -> String {
    pywezterm_core::render::cursor_seq(row, col, visible)
}

/// 部署自省：模块目录、ConPTY 侧载目录、侧载是否生效。
///
/// 「侧载没生效」原本是静默回落，这个函数让它变成可查的事实。
#[pyfunction]
fn env_info(py: Python<'_>) -> PyResult<Py<PyAny>> {
    let info = pywezterm_core::env::info();
    let dict = pyo3::types::PyDict::new(py);
    dict.set_item("module_dir", info.module_dir.map(|p| p.display().to_string()))?;
    dict.set_item("conpty_dir", info.conpty_dir.map(|p| p.display().to_string()))?;
    dict.set_item("conpty_active", info.conpty_active)?;
    Ok(dict.into_any().unbind())
}
