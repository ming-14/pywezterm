//! 终端宿主单元。
//!
//! [`Pane`] = pty + 终端模型 + 视口 + 选区 + reader 线程 + 读缓冲 + 关闭协议。
//! Python 的 `Pty` 与 `Terminal` 都建在它上面，差别只在是否带 pty。
//!
//! reader 只做一件事：把子进程输出放进读缓冲（带高/低水位背压），调用方用 `read()`
//! 取走后自行决定喂模型、回应答。宿主不替调用方解释字节流。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use wezterm_term::input::{KeyCode, KeyModifiers};
use wezterm_term::{AlertHandler, Clipboard, DeviceControlHandler, DownloadHandler};

use crate::error::{Error, Result};
use crate::platform;
use crate::render;
use crate::term::grid::Cell;
use crate::term::{encode, Model, Selection, View};

use super::pty::{Pty, Writer};

/// 读缓冲高水位：到这里就停止从 pty 读取。
///
/// 这是背压传导到 pty 的唯一途径：缓冲满 → reader 不再读 → 管道满 → 子进程 write 阻塞。
/// 没有它，reader 会把管道抽干、在缓冲里无限堆积，内存无界增长而子进程永不被逼停。
const READ_BUFFER_HIGH: usize = 1 << 20;
/// 低水位：降到这里才恢复读取。与高水位拉开距离，避免围绕阈值反复起停。
const READ_BUFFER_LOW: usize = 1 << 18;
/// 等空间的复查周期：只是周期性复查 closed，不必很密。
const SPACE_WAIT: Duration = Duration::from_millis(50);
/// 单次 read 的块大小。
const CHUNK: usize = 8192;

/// 终端宿主单元（多线程安全）。
pub struct Pane {
    inner: Arc<Inner>,
}

struct Inner {
    /// 无 pty 时为 `None`（纯模型用法）
    pty: Mutex<Option<Pty>>,
    writer: Writer,
    model: Mutex<Model>,
    view: Mutex<View>,
    selection: Mutex<Selection>,
    buf: Mutex<VecDeque<u8>>,
    /// 缓冲腾出空间（或 pane 关闭）时唤醒 reader
    space: Condvar,
    /// 缓冲有新数据（或 pane 关闭 / EOF）时唤醒 `read`
    data: Condvar,
    eof: AtomicBool,
    closed: AtomicBool,
    reader_cancel: platform::reader_cancel::Shared,
}

impl Pane {
    /// 建一个带 pty 的宿主单元（尚未启动子进程）。
    ///
    /// 模型不预留 scrollback：带 pty 的用法里模型由调用方持有（`Terminal`），本单元的模型
    /// 不参与喂入。
    pub fn open_pty(cols: usize, rows: usize) -> Result<Self> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let clamp = |v: usize| v.min(u16::MAX as usize) as u16;
        let (pty, reader) = Pty::open(clamp(cols), clamp(rows))?;
        let writer = pty.writer();
        let inner = Arc::new(Inner {
            pty: Mutex::new(Some(pty)),
            writer: writer.clone(),
            model: Mutex::new(Model::new(cols, rows, 0)),
            view: Mutex::new(View::default()),
            selection: Mutex::new(Selection::default()),
            buf: Mutex::new(VecDeque::new()),
            space: Condvar::new(),
            data: Condvar::new(),
            eof: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            reader_cancel: platform::reader_cancel::new_shared(),
        });
        spawn_reader(inner.clone(), reader);
        Ok(Self { inner })
    }

    /// 建一个纯模型宿主单元（无 pty）。
    pub fn detached(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                pty: Mutex::new(None),
                writer: Arc::new(Mutex::new(None)),
                model: Mutex::new(Model::new(cols, rows, scrollback)),
                view: Mutex::new(View::default()),
                selection: Mutex::new(Selection::default()),
                buf: Mutex::new(VecDeque::new()),
                space: Condvar::new(),
                data: Condvar::new(),
                eof: AtomicBool::new(true),
                closed: AtomicBool::new(false),
                reader_cancel: platform::reader_cancel::new_shared(),
            }),
        }
    }

    // ---- 生命周期 --------------------------------------------------------

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::SeqCst)
    }

    /// 启动子进程，返回 `(pid, 进程句柄)`。参数语义见 [`Pty::spawn`]。
    pub fn spawn(
        &self,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        raw_cmdline: Option<String>,
        job_handle: Option<usize>,
    ) -> Result<(u32, usize)> {
        if self.is_closed() {
            return Err(Error::Closed);
        }
        self.inner
            .pty
            .lock()
            .unwrap()
            .as_mut()
            .ok_or(Error::Closed)?
            .spawn(argv, cwd, env, raw_cmdline, job_handle)
    }

    /// 终止子进程。
    pub fn kill(&self) {
        if let Some(pty) = self.inner.pty.lock().unwrap().as_mut() {
            pty.kill();
        }
    }

    /// 非阻塞查询退出码；`None` = 仍在运行或未启动。
    pub fn try_wait(&self) -> Option<u32> {
        self.inner.pty.lock().unwrap().as_mut()?.try_wait()
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.inner
            .pty
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|p| p.child_pid())
    }

    pub fn child_handle(&self) -> Option<usize> {
        self.inner
            .pty
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|p| p.child_handle())
    }

    pub fn hpcon(&self) -> Option<usize> {
        self.inner.pty.lock().unwrap().as_ref().and_then(|p| p.hpcon())
    }

    /// pty 当前尺寸；无 pty 时为 `(0, 0)`。
    pub fn pty_size(&self) -> (u16, u16) {
        self.inner
            .pty
            .lock()
            .unwrap()
            .as_ref()
            .map(|p| p.size())
            .unwrap_or((0, 0))
    }

    /// 关闭：终止子进程、取消 reader 阻塞读、释放 pty、唤醒所有等待方。幂等。
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        // 在锁外等：reader 登记句柄可能要等线程启动，持着 pty 锁睡会挡住其他 pty 操作
        wait_for_reader_registration(&self.inner);
        {
            let mut guard = self.inner.pty.lock().unwrap();
            if let Some(pty) = guard.as_mut() {
                pty.kill();
                // 必须先取消 reader 的阻塞读，再释放 master —— 否则
                // ClosePseudoConsole 会与未完成的 ReadFile 互等（ConPTY 死锁）
                platform::reader_cancel::cancel_and_wait(
                    &self.inner.reader_cancel,
                    &self.inner.eof,
                );
                pty.close();
            }
        }
        self.inner.buf.lock().unwrap().clear();
        self.inner.data.notify_all();
        self.inner.space.notify_all();
    }

    // ---- 读 / 写 ---------------------------------------------------------

    /// 从读缓冲取最多 `n` 字节。EOF 或关闭后返回空。
    ///
    /// `timeout` 为 `None` 时阻塞到有数据或 EOF。等待期间不持任何其他锁。
    pub fn read(&self, n: usize, timeout: Option<Duration>) -> Vec<u8> {
        let n = n.max(1);
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut guard = self.inner.buf.lock().unwrap();
        loop {
            // 先判 closed：关闭后无论缓冲是否残留都返回空
            if self.inner.closed.load(Ordering::SeqCst) {
                return Vec::new();
            }
            if !guard.is_empty() {
                let take = guard.len().min(n);
                let out: Vec<u8> = guard.drain(..take).collect();
                let freed = guard.len() <= READ_BUFFER_LOW;
                drop(guard);
                if freed {
                    self.inner.space.notify_one();
                }
                return out;
            }
            if self.inner.eof.load(Ordering::SeqCst) {
                return Vec::new();
            }
            guard = match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now >= d {
                        return Vec::new();
                    }
                    self.inner.data.wait_timeout(guard, d - now).unwrap().0
                }
                None => self.inner.data.wait(guard).unwrap(),
            };
        }
    }

    /// 读缓冲中待取走的字节数。
    pub fn buffered_bytes(&self) -> usize {
        self.inner.buf.lock().unwrap().len()
    }

    /// 写入 pty。阻塞调用，调用方负责放掉 GIL。
    pub fn write(&self, data: &[u8]) -> Result<()> {
        if self.is_closed() {
            return Ok(());
        }
        let mut guard = self.inner.writer.lock().unwrap();
        match guard.as_mut() {
            Some(w) => w.write_all(data).map_err(|e| Error::Pty(e.into())),
            None => Err(Error::Closed),
        }
    }

    // ---- 模型面 ----------------------------------------------------------

    /// 调整尺寸：pty 与模型一起改，并按稳定行保持视口位置。
    ///
    /// 全程持有模型锁，使「改 pty 尺寸」与「rewrap 模型」对并发 `feed` 是一个原子操作 ——
    /// 否则 feed 的字节会按旧列宽落进行里，随后被 rewrap 拆错。
    pub fn resize(&self, cols: usize, rows: usize) -> Result<()> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut model = self.inner.model.lock().unwrap();
        let prev_top = {
            let view = self.inner.view.lock().unwrap();
            let screen = model.screen();
            screen.phys_to_stable_row_index(view.window(screen).start)
        };

        if let Some(pty) = self.inner.pty.lock().unwrap().as_ref() {
            // pty 尺寸是 u16；模型保留 usize 精度
            let clamp = |v: usize| v.min(u16::MAX as usize) as u16;
            pty.resize(clamp(cols), clamp(rows))?;
        }
        model.resize(cols, rows);

        // rewrap 后物理行数变了，但稳定行不变：用视口顶部稳定行反算新偏移
        let new_offset = {
            let screen = model.screen();
            let total = screen.scrollback_rows();
            let visible = screen.physical_rows;
            screen
                .stable_row_to_phys(prev_top)
                .map(|p| total.saturating_sub(p).saturating_sub(visible))
                .unwrap_or(0)
                .min(total.saturating_sub(visible))
        };
        self.inner.view.lock().unwrap().set_offset(new_offset);
        Ok(())
    }

    pub fn reset(&self) {
        self.inner.model.lock().unwrap().reset();
    }

    pub fn clear_scrollback(&self) {
        self.inner.model.lock().unwrap().clear_scrollback();
    }

    /// 喂入 VT 字节流。字节从哪来由调用方决定 —— 通常是 `read()` 的返回值。
    pub fn feed(&self, data: &[u8]) {
        self.inner.model.lock().unwrap().feed(data);
    }

    /// 可见屏幕纯文本（计入视口滚动）。
    pub fn text(&self) -> String {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        render::ansi::plain_text(model.screen(), &view)
    }

    /// 光标 `(列, 视口行, 可见)`，0-based。滚出可见区时 `visible` 为假。
    pub fn cursor(&self) -> (usize, usize, bool) {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        let (col, row, visible) = model.cursor();
        let screen = model.screen();
        let col = col.min(screen.physical_cols.saturating_sub(1));
        match view.row_in_view(screen, row) {
            Some(r) => (col, r, visible),
            None => (col, 0, false),
        }
    }

    pub fn scroll(&self, delta: i64) {
        let model = self.inner.model.lock().unwrap();
        let mut view = self.inner.view.lock().unwrap();
        view.scroll(delta, model.screen());
    }

    pub fn scroll_to_bottom(&self) {
        self.inner.view.lock().unwrap().to_bottom();
    }

    /// 历史区行数。
    pub fn scrollback_count(&self) -> usize {
        let model = self.inner.model.lock().unwrap();
        crate::term::view::max_offset(model.screen())
    }

    /// 可见网格，每行一个单元格列表。
    pub fn snapshot(&self) -> Vec<Vec<Cell>> {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        render::visible_cells(model.screen(), &view)
    }

    /// 可见网格，每行带「是否折行结尾」标记（宿主渲染用）。
    pub fn snapshot_lines(&self) -> Vec<(bool, Vec<Cell>)> {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        let screen = model.screen();
        let mut out = Vec::new();
        screen.with_phys_lines(view.window(screen), |lines| {
            out.extend(
                lines
                    .iter()
                    .map(|line| (line.last_cell_was_wrapped(), crate::term::cells_of_line(line))),
            );
        });
        out
    }

    /// 历史区网格。
    pub fn scrollback_cells(&self) -> Vec<Vec<Cell>> {
        let model = self.inner.model.lock().unwrap();
        let screen = model.screen();
        let mut out = Vec::new();
        screen.with_phys_lines(crate::term::view::history(screen), |lines| {
            out.extend(lines.iter().map(|line| crate::term::cells_of_line(line)));
        });
        out
    }

    /// 可见区逻辑行（跨折行已重组）：`(首 stable 行, 末 stable 行, 单元格)`。
    pub fn logical_lines(&self) -> Vec<(isize, isize, Vec<Cell>)> {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        let screen = model.screen();
        let window = view.window(screen);
        let start = screen.phys_to_stable_row_index(window.start);
        let end = screen.phys_to_stable_row_index(window.end);
        let mut out = Vec::new();
        screen.for_each_logical_line_in_stable_range(start..end, |range, lines| {
            let mut cells = Vec::new();
            for line in lines {
                cells.extend(crate::term::cells_of_line(line));
            }
            out.push((range.start, range.end - 1, cells));
            true
        });
        out
    }

    /// 可见屏幕 ANSI。
    pub fn render_ansi(&self, include_cursor: bool) -> String {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        render::ansi::screen(model.screen(), &view, model.cursor(), include_cursor)
    }

    /// 历史区文本；`keep_ansi` 为真时带 SGR。
    pub fn render_scrollback(&self, keep_ansi: bool) -> String {
        let model = self.inner.model.lock().unwrap();
        render::ansi::scrollback(model.screen(), keep_ansi)
    }

    /// 可见屏幕 SVG。
    pub fn render_svg(&self, compression_level: u8) -> String {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        let screen = model.screen();
        let lines = render::visible_cells(screen, &view);
        let svg = render::svg::render(&lines, screen.physical_cols, screen.physical_rows);
        render::svg::compress(&svg, compression_level)
    }

    /// 可见屏幕位图；`fmt` 取 `png` / `jpg` / `jpeg` / `bmp`。
    pub fn render_image(&self, scale: f64, fmt: &str) -> Result<Vec<u8>> {
        let model = self.inner.model.lock().unwrap();
        let view = self.inner.view.lock().unwrap();
        let screen = model.screen();
        let lines = render::visible_cells(screen, &view);
        render::pixmap::render_image(&lines, screen.physical_cols, screen.physical_rows, scale, fmt)
    }

    pub fn current_seqno(&self) -> usize {
        self.inner.model.lock().unwrap().current_seqno()
    }

    pub fn changed_stable_rows(&self, since_seqno: usize) -> Vec<isize> {
        self.inner
            .model
            .lock()
            .unwrap()
            .changed_stable_rows(since_seqno)
    }

    pub fn make_all_lines_dirty(&self) {
        self.inner.model.lock().unwrap().make_all_lines_dirty();
    }

    pub fn focus_changed(&self, focused: bool) {
        self.inner.model.lock().unwrap().focus_changed(focused);
    }

    // ---- 模式与元数据 ----------------------------------------------------

    pub fn is_mouse_grabbed(&self) -> bool {
        self.inner.model.lock().unwrap().is_mouse_grabbed()
    }

    pub fn mouse_encoding(&self) -> (u16, bool) {
        self.inner.model.lock().unwrap().mouse_encoding()
    }

    pub fn is_alt_screen_active(&self) -> bool {
        self.inner.model.lock().unwrap().is_alt_screen_active()
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.inner.model.lock().unwrap().bracketed_paste_enabled()
    }

    pub fn mode_restore_seq(&self) -> String {
        self.inner.model.lock().unwrap().mode_restore_seq()
    }

    pub fn keyboard_encoding(&self) -> String {
        self.inner.model.lock().unwrap().keyboard_encoding()
    }

    pub fn title(&self) -> String {
        self.inner.model.lock().unwrap().title()
    }

    pub fn current_dir(&self) -> Option<String> {
        self.inner.model.lock().unwrap().current_dir()
    }

    pub fn progress(&self) -> (String, Option<u8>) {
        self.inner.model.lock().unwrap().progress()
    }

    pub fn semantic_zones(&self) -> Result<Vec<(isize, usize, isize, usize, String)>> {
        self.inner.model.lock().unwrap().semantic_zones()
    }

    // ---- 输入编码 --------------------------------------------------------

    /// 键盘按下编码，返回应下发 pty 的字节。
    pub fn key_down(&self, key: &str, mods: u16) -> Result<Vec<u8>> {
        self.encode(key, mods, |m, code, mods| m.key_down(code, mods))
    }

    pub fn key_up(&self, key: &str, mods: u16) -> Result<Vec<u8>> {
        self.encode(key, mods, |m, code, mods| m.key_up(code, mods))
    }

    fn encode(
        &self,
        key: &str,
        mods: u16,
        f: impl FnOnce(&mut Model, KeyCode, KeyModifiers) -> Result<()>,
    ) -> Result<Vec<u8>> {
        let code = encode::keycode(key)?;
        let mut model = self.inner.model.lock().unwrap();
        model.clear_capture();
        f(&mut model, code, encode::modifiers(mods))?;
        Ok(model.drain_written())
    }

    /// 鼠标事件编码，返回应下发 pty 的字节。
    pub fn mouse(&self, x: usize, y: i64, kind: &str, button: &str, mods: u16) -> Result<Vec<u8>> {
        let event = encode::mouse_event(
            encode::mouse_kind(kind)?,
            x,
            y,
            encode::mouse_button(button)?,
            mods,
        );
        let mut model = self.inner.model.lock().unwrap();
        model.clear_capture();
        model.mouse_event(event)?;
        Ok(model.drain_written())
    }

    /// 模式感知粘贴；bracketed paste 开启时自动包裹。
    ///
    /// 产生的字节**留在捕获缓冲**里，由 [`drain_written`](Self::drain_written) 取走 ——
    /// 与 `key_down` 等「编码即取走」不同，这是 `Terminal` 侧文档化的语义。
    pub fn send_paste(&self, text: &str) -> Result<()> {
        self.inner.model.lock().unwrap().send_paste(text)
    }

    /// 取走模型自发产生的应答（DSR、DA 等）。
    pub fn drain_written(&self) -> Vec<u8> {
        self.inner.model.lock().unwrap().drain_written()
    }

    // ---- 选区 ------------------------------------------------------------

    pub fn selection_set(&self, anchor: (isize, usize), end: (isize, usize)) {
        self.inner.selection.lock().unwrap().set_region(anchor, end);
    }

    pub fn selection_select_word(&self, row: isize, col: usize) {
        let model = self.inner.model.lock().unwrap();
        self.inner
            .selection
            .lock()
            .unwrap()
            .select_word(model.screen(), row, col);
    }

    pub fn selection_select_line(&self, row: isize, col: usize) {
        self.inner.selection.lock().unwrap().select_line(row, col);
    }

    pub fn selection_text(&self) -> String {
        let model = self.inner.model.lock().unwrap();
        self.inner.selection.lock().unwrap().text(model.screen())
    }

    pub fn selection_active(&self) -> bool {
        self.inner.selection.lock().unwrap().is_active()
    }

    pub fn selection_clear(&self) {
        self.inner.selection.lock().unwrap().clear();
    }

    // ---- 回调 ------------------------------------------------------------

    pub fn set_clipboard(&self, cb: Arc<dyn Clipboard>) {
        self.inner.model.lock().unwrap().set_clipboard(cb);
    }

    pub fn set_download_handler(&self, cb: Arc<dyn DownloadHandler>) {
        self.inner.model.lock().unwrap().set_download_handler(cb);
    }

    pub fn set_device_control_handler(&self, cb: Box<dyn DeviceControlHandler>) {
        self.inner
            .model
            .lock()
            .unwrap()
            .set_device_control_handler(cb);
    }

    pub fn set_notification_handler(&self, cb: Box<dyn AlertHandler>) {
        self.inner.model.lock().unwrap().set_notification_handler(cb);
    }
}

/// reader 线程：读 pty → 放进读缓冲（带背压）→ 唤醒等待方。
fn spawn_reader(inner: Arc<Inner>, mut reader: Box<dyn Read + Send>) {
    std::thread::spawn(move || {
        if let Some(rc) = platform::reader_cancel::ReaderCancel::register_current_thread() {
            *inner.reader_cancel.lock().unwrap() = Some(rc);
        }
        let mut tmp = [0u8; CHUNK];
        loop {
            if !wait_for_space(&inner) {
                break;
            }
            match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    inner.buf.lock().unwrap().extend(tmp[..n].iter().copied());
                    inner.data.notify_all();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        inner.eof.store(true, Ordering::SeqCst);
        inner.data.notify_all();
        inner.space.notify_all();
    });
}

/// 等 reader 线程登记取消句柄。
///
/// 句柄只能由 reader 线程自己登记，因此 `close()` 紧跟在 `open` 之后时可能抢在它前面；
/// 那一次取消就会落空，随后释放 master 与未完成的读互等。这里等线程启动的调度延迟，
/// 超时即放弃（此时 reader 要么已在读、要么已退出）。其他平台登记恒成功，不会等待。
fn wait_for_reader_registration(inner: &Inner) {
    const STEP: Duration = Duration::from_millis(1);
    const ATTEMPTS: u32 = 50;
    for _ in 0..ATTEMPTS {
        if inner.eof.load(Ordering::SeqCst) || inner.reader_cancel.lock().unwrap().is_some() {
            return;
        }
        std::thread::sleep(STEP);
    }
}

/// 高水位背压：缓冲满就停读，等调用方腾出空间。返回 `false` 表示该退出了。
fn wait_for_space(inner: &Inner) -> bool {
    let mut guard = inner.buf.lock().unwrap();
    while !inner.closed.load(Ordering::SeqCst) && guard.len() >= READ_BUFFER_HIGH {
        guard = inner.space.wait_timeout(guard, SPACE_WAIT).unwrap().0;
    }
    !inner.closed.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯模型单元：无 pty，可离线验证模型面与视口。
    #[test]
    fn detached_pane_has_no_pty_but_full_model() {
        let pane = Pane::detached(20, 4, 100);
        assert_eq!(pane.pty_size(), (0, 0));
        assert!(pane.child_pid().is_none());
        assert!(pane.hpcon().is_none());
        // 没有 reader，读缓冲恒空
        assert_eq!(pane.read(16, Some(Duration::from_millis(1))), Vec::<u8>::new());
        assert_eq!(pane.buffered_bytes(), 0);
    }

    #[test]
    fn read_returns_empty_after_close() {
        let pane = Pane::open_pty(80, 24).unwrap();
        pane.close();
        assert!(pane.is_closed());
        assert_eq!(pane.read(16, Some(Duration::from_millis(1))), Vec::<u8>::new());
        // 幂等
        pane.close();
        assert!(pane.is_closed());
    }

    #[test]
    fn close_wakes_blocked_read() {
        let pane = Arc::new(Pane::open_pty(80, 24).unwrap());
        let p2 = pane.clone();
        let reader = std::thread::spawn(move || p2.read(16, None));
        std::thread::sleep(Duration::from_millis(50));
        pane.close();
        // 关闭必须唤醒阻塞中的 read，否则这里会挂住
        assert_eq!(reader.join().unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn write_after_close_is_noop() {
        let pane = Pane::open_pty(80, 24).unwrap();
        pane.close();
        assert!(pane.write(b"x").is_ok());
    }

    #[test]
    fn invalid_key_name_is_rejected_before_touching_pty() {
        let pane = Pane::detached(20, 4, 10);
        assert!(matches!(pane.key_down("Nope", 0), Err(Error::Invalid(_))));
        assert!(pane.mouse(0, 0, "click", "left", 0).is_err());
    }
}
