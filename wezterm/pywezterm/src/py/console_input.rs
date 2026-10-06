//! `ConsoleInput` —— 宿主控制台输入采集（仅 Windows）。

use pyo3::prelude::*;

use pywezterm_core::input::{InputEvent, MouseButton, MouseKind};
use pywezterm_core::platform::console_input::ConsoleInput;

use super::error::{IntoPyResult, Result};

/// 宿主控制台输入采集。
///
/// 构造即改写控制台模式（保存原模式）并把输出代码页切到 UTF-8；
/// [`restore`](Self::restore) 与对象析构都会还原，且幂等。
///
/// 归一化结果不依赖调用方线程，也不暴露任何 Win32 结构。
#[pyclass(name = "ConsoleInput")]
pub struct PyConsoleInput {
    inner: ConsoleInput,
}

fn mouse_kind_name(kind: MouseKind) -> &'static str {
    match kind {
        MouseKind::Press => "press",
        MouseKind::Release => "release",
        MouseKind::Move => "move",
    }
}

fn mouse_button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Middle => "middle",
        MouseButton::Right => "right",
        MouseButton::WheelUp => "wheel_up",
        MouseButton::WheelDown => "wheel_down",
        MouseButton::None => "none",
    }
}

#[pymethods]
impl PyConsoleInput {
    /// 打开宿主控制台并设置模式。stdio 被重定向（非控制台）时构造失败。
    #[new]
    fn new(py: Python<'_>) -> Result<Self> {
        Ok(Self {
            inner: py.detach(ConsoleInput::open).py()?,
        })
    }

    /// 等待输入事件，返回是否有事件（`False` = 超时）。
    fn wait_input(&self, py: Python<'_>, ms: u32) -> bool {
        py.detach(|| self.inner.wait(ms))
    }

    /// 读出全部待处理事件并归一化：
    ///
    /// - `("key", key, mods, down)`
    /// - `("mouse", x, y, kind, button, mods, clicks)`
    /// - `("resize",)`
    fn read_inputs(&mut self, py: Python<'_>) -> Result<Py<PyAny>> {
        let events = py.detach(|| self.inner.read_events()).py()?;
        let tuples: Vec<Py<PyAny>> = events
            .into_iter()
            .map(|ev| {
                let tuple: Py<PyAny> = match ev {
                    InputEvent::Key { key, mods, down } => ("key", key, mods, down)
                        .into_pyobject(py)?
                        .into_any()
                        .unbind(),
                    InputEvent::Mouse {
                        x,
                        y,
                        kind,
                        button,
                        mods,
                        clicks,
                    } => (
                        "mouse",
                        x,
                        y,
                        mouse_kind_name(kind),
                        mouse_button_name(button),
                        mods,
                        clicks,
                    )
                        .into_pyobject(py)?
                        .into_any()
                        .unbind(),
                    InputEvent::Resize => ("resize",).into_pyobject(py)?.into_any().unbind(),
                };
                Ok(tuple)
            })
            .collect::<PyResult<_>>()?;
        Ok(tuples.into_pyobject(py)?.into_any().unbind())
    }

    /// 当前窗口逻辑尺寸 `(cols, rows)`。
    fn size(&self, py: Python<'_>) -> Result<(usize, usize)> {
        py.detach(|| self.inner.size()).py()
    }

    /// 还原控制台原模式与代码页。幂等。
    fn restore(&mut self, py: Python<'_>) -> Result<()> {
        py.detach(|| self.inner.restore()).py()
    }
}
