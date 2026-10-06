//! `Surface` —— 增量渲染表面。

use pyo3::prelude::*;

use pywezterm_core::render::surface::Surface;
use pywezterm_core::term::grid::{Attrs, Color};

use super::error::{IntoPyResult, Result};

/// 网格 → 增量 ANSI 字节流。
///
/// 自 `since_seqno` 起只输出变化的格子；无变化时字节为空。输出为 TrueColor ANSI，
/// 直接写到真实终端即可。
#[pyclass(name = "Surface")]
pub struct PySurface {
    surface: Surface,
}

#[pymethods]
impl PySurface {
    #[new]
    #[pyo3(signature = (cols=80, rows=24))]
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            surface: Surface::new(cols, rows),
        }
    }

    fn dimensions(&self) -> (usize, usize) {
        self.surface.dimensions()
    }

    /// 调整尺寸；尺寸变化会丢弃已缓冲的变更，下一帧必然全量。
    fn resize(&mut self, cols: usize, rows: usize) {
        self.surface.resize(cols, rows);
    }

    /// 清空整屏。
    fn clear(&mut self) {
        self.surface.clear();
    }

    /// 当前序号（增量基线）。
    fn current_seqno(&self) -> usize {
        self.surface.current_seqno()
    }

    /// 在 `(x, y)` 写入带样式的文本。`text` 可以是多字符。
    #[pyo3(signature = (x, y, text, fg="default", bg="default", bold=false, italic=false, underline=false, reverse=false, strike=false))]
    #[allow(clippy::too_many_arguments)]
    fn set_cell(
        &mut self,
        x: usize,
        y: usize,
        text: &str,
        fg: &str,
        bg: &str,
        bold: bool,
        italic: bool,
        underline: bool,
        reverse: bool,
        strike: bool,
    ) {
        self.surface.put(
            x,
            y,
            text,
            Color::parse(fg),
            Color::parse(bg),
            Attrs {
                bold,
                italic,
                underline,
                reverse,
                strikethrough: strike,
            },
        );
    }

    /// 自 `since_seqno` 以来的增量，返回 `(新序号, 字节)`。
    fn get_changes_bytes(&mut self, since_seqno: usize) -> Result<(usize, Vec<u8>)> {
        self.surface.changes_bytes(since_seqno).py()
    }

    /// 强制全量重绘。
    fn repaint_bytes(&mut self) -> Result<(usize, Vec<u8>)> {
        self.surface.repaint_bytes().py()
    }
}
