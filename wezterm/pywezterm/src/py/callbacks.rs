//! Python 回调 → 领域 trait 的适配。
//!
//! 这些回调在 **reader 线程**里被调用。实现只做三件事：取 GIL、调用、把异常打印出来。
//! 异常绝不允许逃逸 —— 逃逸会沿 reader 线程展开，把终端留在半更新状态。
//!
//! 回调体内**不得反查终端状态**：调用发生在领域层持锁期间，反查会自锁。

use pyo3::prelude::*;
use wezterm_term::{Alert, AlertHandler, Clipboard, ClipboardSelection, DownloadHandler};

/// 调用并吞掉异常。
fn invoke(cb: &Py<PyAny>, args: impl for<'py> FnOnce(Bound<'py, PyAny>) -> PyResult<()>) {
    Python::attach(|py| {
        let bound = cb.bind(py);
        if let Err(e) = args(bound.clone()) {
            log::error!("pywezterm 回调异常: {e}");
            e.print(py);
        }
    });
}

/// OSC 52 剪贴板写。
pub struct PyClipboard(pub Py<PyAny>);

impl Clipboard for PyClipboard {
    fn set_contents(
        &self,
        selection: ClipboardSelection,
        data: Option<String>,
    ) -> anyhow::Result<()> {
        let sel = match selection {
            ClipboardSelection::Clipboard => "clipboard",
            ClipboardSelection::PrimarySelection => "primary",
        };
        invoke(&self.0, |f| f.call1((sel, data)).map(|_| ()));
        Ok(())
    }
}

/// 下载请求。
pub struct PyDownloadHandler(pub Py<PyAny>);

impl DownloadHandler for PyDownloadHandler {
    fn save_to_downloads(&self, name: Option<String>, data: Vec<u8>) {
        invoke(&self.0, |f| f.call1((name, data)).map(|_| ()));
    }
}

/// DCS 设备控制。
pub struct PyDeviceControlHandler(pub Py<PyAny>);

impl wezterm_term::DeviceControlHandler for PyDeviceControlHandler {
    fn handle_device_control(&mut self, control: wezterm_escape_parser::DeviceControlMode) {
        invoke(&self.0, |f| {
            f.call1((format!("{control:?}"),)).map(|_| ())
        });
    }
}

/// 通知（响铃、标题、进度等）。
pub struct PyAlertHandler(pub Py<PyAny>);

impl AlertHandler for PyAlertHandler {
    fn alert(&mut self, alert: Alert) {
        invoke(&self.0, |f| f.call1((format!("{alert:?}"),)).map(|_| ()));
    }
}
