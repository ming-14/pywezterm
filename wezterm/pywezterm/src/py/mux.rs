//! `Mux` —— 多窗格复用器。

use std::collections::HashMap;
use std::sync::Arc;

use pyo3::prelude::*;

use pywezterm_core::host::{Pane, PaneId};
use pywezterm_core::mux::Mux;

use super::callbacks::{PyClipboard, PyOutputNotifier};
use super::error::{IntoPyResult, Result};

/// 多窗格复用器。
///
/// 每个窗格是一个真实伪终端 + 终端模型，reader 线程自动喂模型并把应答回写 pty；
/// [`render`](Self::render) 合成一帧增量 ANSI 字节。
///
/// 布局目前支持 1 个窗格（全屏）与 2 个窗格（左右二分）。
#[pyclass(name = "Mux")]
pub struct PyMux {
    mux: Mux,
}

impl PyMux {
    /// 取窗格；不存在时报 `PaneNotFound`。
    fn pane(&self, id: u64) -> Result<Arc<Pane>> {
        self.mux.pane(PaneId::new(id)).py()
    }
}

#[pymethods]
impl PyMux {
    #[new]
    #[pyo3(signature = (cols=80, rows=24))]
    fn new(cols: usize, rows: usize) -> Self {
        Self {
            mux: Mux::new(cols, rows),
        }
    }

    // ---- 窗格 ------------------------------------------------------------

    /// 新建窗格并启动子进程，返回窗格 id（0 起）。
    #[pyo3(signature = (argv, cwd=None, env=None))]
    fn add_pane(
        &self,
        py: Python<'_>,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
    ) -> Result<u64> {
        Ok(py
            .detach(|| self.mux.add_pane(argv, cwd, env))
            .py()?
            .raw())
    }

    /// 关闭一个窗格。幂等。
    fn close_pane(&self, py: Python<'_>, pane_id: u64) -> Result<()> {
        py.detach(|| self.mux.close_pane(PaneId::new(pane_id))).py()
    }

    /// 关闭全部窗格。
    fn close(&self, py: Python<'_>) {
        py.detach(|| self.mux.close())
    }

    fn pane_count(&self, py: Python<'_>) -> usize {
        py.detach(|| self.mux.pane_count())
    }

    fn dimensions(&self, py: Python<'_>) -> (usize, usize) {
        py.detach(|| self.mux.dimensions())
    }

    /// 各窗格矩形 `[(x, y, w, h), ...]`。
    fn pane_rects(&self, py: Python<'_>) -> Vec<(usize, usize, usize, usize)> {
        py.detach(|| {
            self.mux
                .pane_rects()
                .into_iter()
                .map(|r| (r.x, r.y, r.w, r.h))
                .collect()
        })
    }

    /// 焦点窗格 id。
    fn focused(&self, py: Python<'_>) -> Option<u64> {
        py.detach(|| self.mux.focused()).map(|id| id.raw())
    }

    fn set_focus(&self, py: Python<'_>, pane_id: u64) -> Result<()> {
        py.detach(|| self.mux.set_focus(PaneId::new(pane_id))).py()
    }

    /// 命中测试：整屏坐标落在哪个窗格；分隔线、状态栏、屏幕外返回 `None`。
    fn pane_at(&self, py: Python<'_>, x: usize, y: i64) -> Option<u64> {
        py.detach(|| self.mux.pane_at(x, y.max(0) as usize))
            .map(|id| id.raw())
    }

    // ---- 渲染 ------------------------------------------------------------

    /// 合成一帧，返回 `(bytes, cursor_row, cursor_col, cursor_visible)`。
    ///
    /// `bytes` 为增量 ANSI（内部 CUP 是 1-based）；光标坐标为 **0-based** 整屏坐标。
    fn render(&self, py: Python<'_>) -> Result<(Vec<u8>, usize, usize, bool)> {
        let frame = py.detach(|| self.mux.render()).py()?;
        Ok((
            frame.bytes,
            frame.cursor_row,
            frame.cursor_col,
            frame.cursor_visible,
        ))
    }

    /// 宿主屏尺寸变化。
    fn resize(&self, py: Python<'_>, cols: usize, rows: usize) {
        py.detach(|| self.mux.resize(cols, rows))
    }

    /// 强制下一帧全量重绘。
    fn force_repaint(&self, py: Python<'_>) {
        py.detach(|| self.mux.force_repaint())
    }

    // ---- 布局 ------------------------------------------------------------

    /// 是否在窗格之间画分隔线。
    #[pyo3(signature = (sep=true))]
    fn set_sep(&self, py: Python<'_>, sep: bool) {
        py.detach(|| self.mux.set_sep(sep))
    }

    /// 分隔线所在列；`None` 回退中点。
    fn set_split_col(&self, py: Python<'_>, split: Option<usize>) {
        py.detach(|| self.mux.set_split_col(split))
    }

    /// 底部预留的状态栏行数。
    fn set_status_rows(&self, py: Python<'_>, rows: usize) {
        py.detach(|| self.mux.set_status_rows(rows))
    }

    /// 状态栏文本。
    fn set_status(&self, py: Python<'_>, text: String) {
        py.detach(|| self.mux.set_status(text))
    }

    // ---- 焦点窗格输入（编码后已下发该窗格的 pty）--------------------------

    fn key_down(&self, py: Python<'_>, key: &str, mods: u16) -> Result<Vec<u8>> {
        self.route(py, None, key, mods, true)
    }

    fn key_up(&self, py: Python<'_>, key: &str, mods: u16) -> Result<Vec<u8>> {
        self.route(py, None, key, mods, false)
    }

    /// 鼠标事件（整屏坐标）。未命中窗格时报错；先用 [`pane_at`](Self::pane_at) 判定。
    #[pyo3(signature = (x, y, kind="press", button="left", mods=0))]
    fn mouse(
        &self,
        py: Python<'_>,
        x: usize,
        y: i64,
        kind: &str,
        button: &str,
        mods: u16,
    ) -> Result<Vec<u8>> {
        let (id, lx, ly) = py.detach(|| self.mux.to_pane_coords(x, y)).py()?;
        let pane = self.mux.pane(id).py()?;
        let bytes = py.detach(|| pane.mouse(lx, ly, kind, button, mods)).py()?;
        self.send(py, &pane, bytes)
    }

    fn scroll(&self, py: Python<'_>, delta: i64) -> Result<()> {
        py.detach(|| self.mux.scroll(delta)).py()
    }

    fn scroll_to_bottom(&self, py: Python<'_>) -> Result<()> {
        py.detach(|| self.mux.scroll_to_bottom()).py()
    }

    /// 向焦点窗格粘贴（模式感知）。
    fn send_paste(&self, py: Python<'_>, text: &str) -> Result<()> {
        let pane = self.focused_pane(py)?;
        self.paste_into(py, &pane, text)
    }

    // ---- 指定窗格输入 ----------------------------------------------------

    fn pane_key_down(&self, py: Python<'_>, pane_id: u64, key: &str, mods: u16) -> Result<Vec<u8>> {
        self.route(py, Some(pane_id), key, mods, true)
    }

    fn pane_key_up(&self, py: Python<'_>, pane_id: u64, key: &str, mods: u16) -> Result<Vec<u8>> {
        self.route(py, Some(pane_id), key, mods, false)
    }

    /// 直接写入窗格 pty（原始字节）。
    fn pane_write(&self, py: Python<'_>, pane_id: u64, data: Vec<u8>) -> Result<()> {
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.write(&data)).py()
    }

    fn pane_send_paste(&self, py: Python<'_>, pane_id: u64, text: &str) -> Result<()> {
        let pane = self.pane(pane_id)?;
        self.paste_into(py, &pane, text)
    }

    /// 窗格内坐标的鼠标事件。
    #[pyo3(signature = (pane_id, x, y, kind="press", button="left", mods=0))]
    fn pane_mouse(
        &self,
        py: Python<'_>,
        pane_id: u64,
        x: usize,
        y: i64,
        kind: &str,
        button: &str,
        mods: u16,
    ) -> Result<Vec<u8>> {
        let pane = self.pane(pane_id)?;
        let bytes = py.detach(|| pane.mouse(x, y, kind, button, mods)).py()?;
        self.send(py, &pane, bytes)
    }

    // ---- 指定窗格查询 ----------------------------------------------------

    /// 窗格可见区纯文本。
    fn pane_text(&self, py: Python<'_>, pane_id: u64) -> Result<String> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.text()))
    }

    /// 窗格光标 `(row, col, visible)`，窗格内 0-based。
    fn pane_cursor(&self, py: Python<'_>, pane_id: u64) -> Result<(usize, usize, bool)> {
        let pane = self.pane(pane_id)?;
        let (col, row, visible) = py.detach(|| pane.cursor());
        Ok((row, col, visible))
    }

    fn pane_is_mouse_grabbed(&self, py: Python<'_>, pane_id: u64) -> Result<bool> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.is_mouse_grabbed()))
    }

    /// 非阻塞查询子进程退出码；`None` = 仍在运行。
    fn pane_try_wait(&self, py: Python<'_>, pane_id: u64) -> Result<Option<u32>> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.try_wait()))
    }

    /// 单独调整某个窗格的 pty 与模型尺寸。
    fn pane_resize(&self, py: Python<'_>, pane_id: u64, cols: usize, rows: usize) -> Result<()> {
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.resize(cols, rows)).py()
    }

    fn pane_scroll(&self, py: Python<'_>, pane_id: u64, delta: i64) -> Result<()> {
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.scroll(delta));
        Ok(())
    }

    fn pane_scroll_to_bottom(&self, py: Python<'_>, pane_id: u64) -> Result<()> {
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.scroll_to_bottom());
        Ok(())
    }

    /// 取走并清空该窗格的子进程原始输出（录制用）。
    fn pane_take_output(&self, py: Python<'_>, pane_id: u64) -> Result<Vec<u8>> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.take_output()))
    }

    /// 录制缓冲中待取走的字节数（上限 16 MiB）。
    fn pane_output_len(&self, py: Python<'_>, pane_id: u64) -> Result<usize> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.output_len()))
    }

    // ---- 指定窗格选区（整屏坐标入口）--------------------------------------

    fn pane_selection_set(
        &self,
        py: Python<'_>,
        pane_id: u64,
        anchor_x: usize,
        anchor_y: i64,
        end_x: usize,
        end_y: i64,
    ) -> Result<()> {
        let id = PaneId::new(pane_id);
        let (a, e) = py
            .detach(|| -> pywezterm_core::Result<_> {
                Ok((
                    self.mux.to_stable(id, anchor_x, anchor_y)?,
                    self.mux.to_stable(id, end_x, end_y)?,
                ))
            })
            .py()?;
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.selection_set(a, e));
        Ok(())
    }

    fn pane_selection_select_word(&self, py: Python<'_>, pane_id: u64, x: usize, y: i64) -> Result<()> {
        let id = PaneId::new(pane_id);
        let (row, col) = py.detach(|| self.mux.to_stable(id, x, y)).py()?;
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.selection_select_word(row, col));
        Ok(())
    }

    fn pane_selection_select_line(&self, py: Python<'_>, pane_id: u64, x: usize, y: i64) -> Result<()> {
        let id = PaneId::new(pane_id);
        let (row, col) = py.detach(|| self.mux.to_stable(id, x, y)).py()?;
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.selection_select_line(row, col));
        Ok(())
    }

    fn pane_selection_text(&self, py: Python<'_>, pane_id: u64) -> Result<String> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.selection_text()))
    }

    fn pane_selection_active(&self, py: Python<'_>, pane_id: u64) -> Result<bool> {
        let pane = self.pane(pane_id)?;
        Ok(py.detach(|| pane.selection_active()))
    }

    fn pane_selection_clear(&self, py: Python<'_>, pane_id: u64) -> Result<()> {
        let pane = self.pane(pane_id)?;
        py.detach(|| pane.selection_clear());
        Ok(())
    }

    // ---- 回调 ------------------------------------------------------------

    /// 任一窗格有新输出时调用（无参）。`None` 取消。
    fn set_output_callback(&self, py: Python<'_>, callback: Option<Py<PyAny>>) {
        py.detach(|| match callback {
            Some(cb) => self.mux.set_notifier(Arc::new(PyOutputNotifier(cb))),
            None => self.mux.clear_notifier(),
        })
    }

    /// OSC 52 剪贴板写：`callback(selection: str, data: str|None)`。
    fn set_focus_selection_callback(&self, py: Python<'_>, callback: Py<PyAny>) {
        py.detach(|| self.mux.set_clipboard(Arc::new(PyClipboard(callback))))
    }
}

impl PyMux {
    /// 把编码字节下发到窗格 pty，并原样返回给调用方。
    fn send(&self, py: Python<'_>, pane: &Arc<Pane>, bytes: Vec<u8>) -> Result<Vec<u8>> {
        py.detach(|| pane.write(&bytes)).py()?;
        Ok(bytes)
    }

    /// 编码粘贴内容并下发。
    ///
    /// `Pane::send_paste` 把字节留在捕获缓冲（`Terminal` 侧要能自己 `drain_written`），
    /// 这里取走并下发。
    fn paste_into(&self, py: Python<'_>, pane: &Arc<Pane>, text: &str) -> Result<()> {
        py.detach(|| pane.send_paste(text)).py()?;
        let bytes = py.detach(|| pane.drain_written());
        self.send(py, pane, bytes).map(|_| ())
    }

    /// 焦点窗格；没有焦点时报错。
    fn focused_pane(&self, py: Python<'_>) -> Result<Arc<Pane>> {
        let id = py.detach(|| self.mux.focused()).ok_or_else(|| {
            super::error::to_pyerr(pywezterm_core::Error::Platform("没有焦点窗格".into()))
        })?;
        self.mux.pane(id).py()
    }

    /// 编码按键并下发到指定窗格（`None` = 焦点窗格）。
    fn route(
        &self,
        py: Python<'_>,
        pane_id: Option<u64>,
        key: &str,
        mods: u16,
        down: bool,
    ) -> Result<Vec<u8>> {
        let pane = match pane_id {
            Some(id) => self.pane(id)?,
            None => self.focused_pane(py)?,
        };
        let bytes = py
            .detach(|| {
                if down {
                    pane.key_down(key, mods)
                } else {
                    pane.key_up(key, mods)
                }
            })
            .py()?;
        self.send(py, &pane, bytes)
    }
}
