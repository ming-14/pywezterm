//! `Terminal` —— 终端模型。

use std::sync::Arc;

use pyo3::prelude::*;

use pywezterm_core::host::Pane;

use super::callbacks::{
    PyAlertHandler, PyClipboard, PyDeviceControlHandler, PyDownloadHandler,
};
use super::convert;
use super::error::{IntoPyResult, Result};

/// 终端模型：喂字节 → 解析 → 查询状态、快照、编码输入。
///
/// 无 pty —— 字节从哪来由调用方决定（通常来自 `Pty.read`）。
#[pyclass(name = "Terminal")]
pub struct PyTerminal {
    pane: Pane,
}

impl PyTerminal {
    fn pane(&self) -> &Pane {
        &self.pane
    }
}

#[pymethods]
impl PyTerminal {
    #[new]
    #[pyo3(signature = (cols=80, rows=24, scrollback=10000))]
    fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            pane: Pane::detached(cols, rows, scrollback),
        }
    }

    /// 喂入 VT 字节流。
    fn feed(&self, py: Python<'_>, data: &[u8]) {
        py.detach(|| self.pane().feed(data))
    }

    /// 调整尺寸。
    fn resize(&self, py: Python<'_>, cols: usize, rows: usize) -> Result<()> {
        py.detach(|| self.pane().resize(cols, rows)).py()
    }

    /// 重置到初始状态：清空屏幕与 scrollback、重置全部状态。
    fn reset(&self, py: Python<'_>) {
        py.detach(|| self.pane().reset())
    }

    /// 清空 scrollback 历史区（等价 `CSI 3 J`）。
    fn clear_scrollback(&self, py: Python<'_>) {
        py.detach(|| self.pane().clear_scrollback())
    }

    /// 上报焦点状态（配合 DECSET 1004）。
    fn focus_changed(&self, py: Python<'_>, focused: bool) {
        py.detach(|| self.pane().focus_changed(focused))
    }

    /// 标记全部行变更（选区高亮失效时全量重绘）。
    fn make_all_lines_dirty(&self, py: Python<'_>) {
        py.detach(|| self.pane().make_all_lines_dirty())
    }

    // ---- 读取 ------------------------------------------------------------

    /// 光标 `(row, col, visible)`，0-based，计入视图滚动偏移。
    fn cursor(&self, py: Python<'_>) -> (usize, usize, bool) {
        let (col, row, visible) = py.detach(|| self.pane().cursor());
        (row, col, visible)
    }

    /// 当前序号（每次 feed 递增），供调用方记录渲染基线。
    fn current_seqno(&self, py: Python<'_>) -> usize {
        py.detach(|| self.pane().current_seqno())
    }

    /// 自 `since_seqno` 以来变化过的稳定行号（含可见区与 scrollback）。
    fn changed_stable_rows(&self, py: Python<'_>, since_seqno: usize) -> Vec<isize> {
        py.detach(|| self.pane().changed_stable_rows(since_seqno))
    }

    /// 可见区逻辑行 `(首 stable, 末 stable, cells)`（跨折行已重组）。
    fn logical_lines(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let lines = py.detach(|| self.pane().logical_lines());
        convert::logical_rows(py, &lines)
    }

    /// 可见网格，每行一个单元格列表。
    fn snapshot(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let lines = py.detach(|| self.pane().snapshot());
        convert::rows(py, &lines)
    }

    /// 可见网格，每行 `(wrapped, cells)`。
    fn snapshot_lines(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let lines = py.detach(|| self.pane().snapshot_lines());
        convert::wrapped_rows(py, &lines)
    }

    /// scrollback 历史区网格。
    fn scrollback(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let lines = py.detach(|| self.pane().scrollback_cells());
        convert::rows(py, &lines)
    }

    /// scrollback 历史行数。
    fn scrollback_count(&self, py: Python<'_>) -> usize {
        py.detach(|| self.pane().scrollback_count())
    }

    /// 可见屏幕纯文本。
    fn text(&self, py: Python<'_>) -> String {
        py.detach(|| self.pane().text())
    }

    /// 可见屏幕 ANSI（CUP + SGR + 清行尾）。
    fn render_ansi(&self, py: Python<'_>, include_cursor: bool) -> String {
        py.detach(|| self.pane().render_ansi(include_cursor))
    }

    /// 历史区文本；`keep_ansi` 为真时带 SGR。
    fn render_scrollback(&self, py: Python<'_>, keep_ansi: bool) -> String {
        py.detach(|| self.pane().render_scrollback(keep_ansi))
    }

    /// 可见屏幕 SVG。`compression_level >= 1` 时压缩。
    fn render_svg(&self, py: Python<'_>, compression_level: u8) -> String {
        py.detach(|| self.pane().render_svg(compression_level))
    }

    /// 可见屏幕位图；`fmt` 取 `png` / `jpg` / `jpeg` / `bmp`。
    #[pyo3(signature = (scale=1.0, fmt="png"))]
    fn render_image(&self, py: Python<'_>, scale: f64, fmt: &str) -> Result<Vec<u8>> {
        py.detach(|| self.pane().render_image(scale, fmt)).py()
    }

    /// 滚动查看历史：`delta > 0` 上滚。
    fn scroll(&self, py: Python<'_>, delta: i64) {
        py.detach(|| self.pane().scroll(delta))
    }

    /// 回落到底部，恢复跟随最新输出。
    fn scroll_to_bottom(&self, py: Python<'_>) {
        py.detach(|| self.pane().scroll_to_bottom())
    }

    // ---- 输入编码 --------------------------------------------------------

    /// 键盘按下编码（模式感知），返回应下发 pty 的字节。本方法不写 pty。
    fn key_down(&self, py: Python<'_>, key: &str, mods: u16) -> Result<Vec<u8>> {
        py.detach(|| self.pane().key_down(key, mods)).py()
    }

    /// 键盘抬起编码。
    fn key_up(&self, py: Python<'_>, key: &str, mods: u16) -> Result<Vec<u8>> {
        py.detach(|| self.pane().key_up(key, mods)).py()
    }

    /// 鼠标事件编码（模式感知）。
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
        py.detach(|| self.pane().mouse(x, y, kind, button, mods)).py()
    }

    /// 模式感知粘贴；bracketed paste 开启时自动包裹。字节留在内部缓冲，用
    /// [`drain_written`](Self::drain_written) 取走。
    fn send_paste(&self, py: Python<'_>, text: &str) -> Result<()> {
        py.detach(|| self.pane().send_paste(text)).py()
    }

    /// 取走模型自发产生的应答（DSR、DA 等）。
    fn drain_written(&self, py: Python<'_>) -> Vec<u8> {
        py.detach(|| self.pane().drain_written())
    }

    // ---- 模式与元数据 ----------------------------------------------------

    fn is_mouse_grabbed(&self, py: Python<'_>) -> bool {
        py.detach(|| self.pane().is_mouse_grabbed())
    }

    /// `(mode, sgr)`；`mode ∈ {0, 1000, 1002, 1003}`。
    fn get_mouse_encoding(&self, py: Python<'_>) -> (u16, bool) {
        py.detach(|| self.pane().mouse_encoding())
    }

    fn is_alt_screen_active(&self, py: Python<'_>) -> bool {
        py.detach(|| self.pane().is_alt_screen_active())
    }

    fn bracketed_paste_enabled(&self, py: Python<'_>) -> bool {
        py.detach(|| self.pane().bracketed_paste_enabled())
    }

    /// 终端模式恢复序列（新订阅者重建客户端状态用）。
    fn mode_restore_seq(&self, py: Python<'_>) -> String {
        py.detach(|| self.pane().mode_restore_seq())
    }

    /// 当前键盘编码协议：`xterm` / `csi-u` / `win32` / `kitty`。
    fn get_keyboard_encoding(&self, py: Python<'_>) -> String {
        py.detach(|| self.pane().keyboard_encoding())
    }

    fn get_title(&self, py: Python<'_>) -> String {
        py.detach(|| self.pane().title())
    }

    fn get_current_dir(&self, py: Python<'_>) -> Option<String> {
        py.detach(|| self.pane().current_dir())
    }

    /// `(标签, 百分比)`；标签 ∈ `none` / `percentage` / `error` / `indeterminate`。
    fn get_progress(&self, py: Python<'_>) -> (String, Option<u8>) {
        py.detach(|| self.pane().progress())
    }

    /// 语义区（OSC 133）：`(y0, x0, y1, x1, 类型)`。
    fn get_semantic_zones(&self, py: Python<'_>) -> Result<Vec<(isize, usize, isize, usize, String)>> {
        py.detach(|| self.pane().semantic_zones()).py()
    }

    // ---- 选区 ------------------------------------------------------------

    /// 区域选择：anchor → end（stable 坐标，顺序可反）。
    fn selection_set(&self, py: Python<'_>, anchor_row: isize, anchor_col: usize, end_row: isize, end_col: usize) {
        py.detach(|| self.pane().selection_set((anchor_row, anchor_col), (end_row, end_col)))
    }

    /// 双击选词；命中空白或标点时不选区。
    fn selection_select_word(&self, py: Python<'_>, row: isize, col: usize) {
        py.detach(|| self.pane().selection_select_word(row, col))
    }

    /// 三击选行（含结尾换行）。
    fn selection_select_line(&self, py: Python<'_>, row: isize, col: usize) {
        py.detach(|| self.pane().selection_select_line(row, col))
    }

    fn selection_text(&self, py: Python<'_>) -> String {
        py.detach(|| self.pane().selection_text())
    }

    fn selection_active(&self, py: Python<'_>) -> bool {
        py.detach(|| self.pane().selection_active())
    }

    fn selection_clear(&self, py: Python<'_>) {
        py.detach(|| self.pane().selection_clear())
    }

    // ---- 回调 ------------------------------------------------------------

    /// OSC 52 剪贴板写：`callback(selection: str, data: str|None)`。
    fn set_clipboard_callback(&self, callback: Py<PyAny>) {
        self.pane().set_clipboard(Arc::new(PyClipboard(callback)));
    }

    /// 下载请求：`callback(name: str|None, data: bytes)`。
    fn set_download_callback(&self, callback: Py<PyAny>) {
        self.pane()
            .set_download_handler(Arc::new(PyDownloadHandler(callback)));
    }

    /// DCS 设备控制：`callback(control: str)`。
    fn set_device_control_callback(&self, callback: Py<PyAny>) {
        self.pane()
            .set_device_control_handler(Box::new(PyDeviceControlHandler(callback)));
    }

    /// 通知（响铃、标题、进度等）：`callback(alert: str)`。
    fn set_notification_callback(&self, callback: Py<PyAny>) {
        self.pane()
            .set_notification_handler(Box::new(PyAlertHandler(callback)));
    }
}
