//! 领域错误 → Python 异常。
//!
//! 领域层只产出 [`pywezterm_core::Error`]，映射集中在这里。异常类型都继承
//! `RuntimeError`，因此 `except RuntimeError` 仍然有效。

use pyo3::create_exception;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use pywezterm_core::Error;

create_exception!(pywezterm, TerminalClosed, PyRuntimeError, "终端已关闭。");
create_exception!(pywezterm, RenderError, PyRuntimeError, "渲染失败。");
create_exception!(
    pywezterm,
    PlatformUnsupported,
    PyRuntimeError,
    "当前平台不支持该能力。"
);

/// 领域错误 → Python 异常。
fn to_pyerr(err: Error) -> PyErr {
    let msg = err.to_string();
    match err {
        Error::Closed => TerminalClosed::new_err(msg),
        Error::Invalid(_) => PyValueError::new_err(msg),
        Error::Render(_) => RenderError::new_err(msg),
        Error::Platform(_) => PlatformUnsupported::new_err(msg),
        Error::Pty(_) => PyRuntimeError::new_err(msg),
    }
}

pub type Result<T> = std::result::Result<T, PyErr>;

/// 把 `Result` 里的领域错误转成 Python 异常。
pub trait IntoPyResult<T> {
    fn py(self) -> Result<T>;
}

impl<T> IntoPyResult<T> for pywezterm_core::Result<T> {
    fn py(self) -> Result<T> {
        self.map_err(to_pyerr)
    }
}

/// 注册异常类型。
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("TerminalClosed", py.get_type::<TerminalClosed>())?;
    m.add("RenderError", py.get_type::<RenderError>())?;
    m.add("PlatformUnsupported", py.get_type::<PlatformUnsupported>())?;
    Ok(())
}
