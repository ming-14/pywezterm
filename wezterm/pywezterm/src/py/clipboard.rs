//! 宿主剪贴板读写（仅 Windows 实现）。

use pyo3::prelude::*;

use pywezterm_core::platform::clipboard;

use super::error::{IntoPyResult, Result};

/// 读纯文本；剪贴板被占用或无文本内容时返回空串。
#[pyfunction]
pub fn clipboard_read(py: Python<'_>) -> Result<String> {
    py.detach(clipboard::read).py()
}

/// 写纯文本。剪贴板被其他进程占用等瞬时失败按尽力而为忽略。
#[pyfunction]
pub fn clipboard_write(py: Python<'_>, text: &str) -> Result<()> {
    py.detach(|| clipboard::write(text)).py()
}
