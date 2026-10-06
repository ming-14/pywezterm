//! 终端模型：wezterm-term 的封装 + 模式观察记录。
//!
//! 输入侧的 VT 解析全权交给 `wezterm_term`，这里只做两件事：
//!
//! 1. 把模型没暴露的模式**设置序列**记下来（见 [`ModeState`]），供订阅方恢复客户端状态；
//! 2. 转发模型已经提供的能力（光标、标题、CWD、进度、语义区、键鼠编码）。
//!
//! 键鼠编码产生的字节进捕获缓冲，由调用方取走（`drain_written`）—— 本模块不碰 pty。

use std::sync::{Arc, Mutex};

use wezterm_term::color::ColorPalette;
use wezterm_term::input::{KeyCode, KeyModifiers, MouseEvent};
use wezterm_term::screen::Screen;
use wezterm_term::{
    AlertHandler, Clipboard, DeviceControlHandler, DownloadHandler, Progress, SemanticType,
    Terminal, TerminalConfiguration, TerminalSize,
};

use crate::error::{Error, Result};

/// 内嵌终端配置：默认调色板 + 可配滚动行数。
#[derive(Debug)]
struct EmbeddedConfig {
    scrollback: usize,
}

impl TerminalConfiguration for EmbeddedConfig {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
    fn scrollback_size(&self) -> usize {
        self.scrollback
    }
}

/// 写入捕获缓冲的 writer：模型自发产生的应答（DSR、DA、编码输出）统一落到这里，
/// 由调用方决定下发路径。
#[derive(Clone, Default)]
pub(crate) struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 应用显式设置过的模式。
///
/// **每个字段都是 `Option`**：`None` 表示应用从未设置过它，于是恢复时**不发**这条序列，
/// 让客户端保留自己的默认值与配置。记成 `bool` 并用「我们以为的默认值」去恢复，会把
/// 客户端自己的设置覆盖掉（例如光标闪烁）。
#[derive(Clone, Copy, Default)]
struct ModeState {
    application_cursor_keys: Option<bool>,
    origin: Option<bool>,
    wraparound: Option<bool>,
    cursor_blink: Option<bool>,
    cursor_visible: Option<bool>,
    reverse_wraparound: Option<bool>,
    application_keypad: Option<bool>,
    /// 备用屏幕（DECSET 47 / 1047 / 1049 归一到同一状态）
    alt_screen: Option<bool>,
    /// 鼠标追踪模式号：0 / 1000 / 1002 / 1003
    mouse_tracking: Option<u16>,
    focus_reporting: Option<bool>,
    sgr_mouse: Option<bool>,
    sgr_pixels: Option<bool>,
    bracketed_paste: Option<bool>,
    /// 插入模式（SM 4）
    insert: Option<bool>,
    /// 换行模式（SM 20 / LNM）
    newline: Option<bool>,
}

impl ModeState {
    /// 应用单条模式设置。`dec` 为真表示 `CSI ? Pm h/l`，否则是 `CSI Pm h/l`。
    ///
    /// 只记**客户端也能生效**的模式：客户端不认的那些记了也没用，恢复时发过去只会被忽略。
    fn apply(&mut self, param: u16, enable: bool, dec: bool) {
        if dec {
            match param {
                1 => self.application_cursor_keys = Some(enable),
                6 => self.origin = Some(enable),
                7 => self.wraparound = Some(enable),
                12 => self.cursor_blink = Some(enable),
                25 => self.cursor_visible = Some(enable),
                45 => self.reverse_wraparound = Some(enable),
                47 | 1047 | 1049 => self.alt_screen = Some(enable),
                66 => self.application_keypad = Some(enable),
                1000 | 1002 | 1003 => {
                    self.mouse_tracking = Some(if enable { param } else { 0 })
                }
                1004 => self.focus_reporting = Some(enable),
                1006 => self.sgr_mouse = Some(enable),
                1016 => self.sgr_pixels = Some(enable),
                2004 => self.bracketed_paste = Some(enable),
                _ => {}
            }
        } else {
            match param {
                4 => self.insert = Some(enable),
                20 => self.newline = Some(enable),
                _ => {}
            }
        }
    }

    /// 恢复序列。**只发应用真的设置过的那些**，未观察到的什么都不发。
    ///
    /// 两处顺序不能动：备用屏最先（`?1049h` 会清屏，必须先进入再画内容）；光标可见性
    /// 最后（不影响内容绘制）。有意**不**恢复 `?2026`（同步输出）—— 那是逐帧的瞬时
    /// 模式，打开它会让客户端一直缓冲渲染，等于把画面冻在最后一帧。
    fn restore_seq(&self) -> String {
        let mut out = String::new();
        if self.alt_screen == Some(true) {
            out.push_str("\x1b[?1049h");
        }
        if let Some(tracking) = self.mouse_tracking {
            if tracking > 0 {
                out.push_str(&format!("\x1b[?{tracking}h"));
            }
            if self.sgr_mouse == Some(true) {
                out.push_str("\x1b[?1006h");
            }
            if self.sgr_pixels == Some(true) {
                out.push_str("\x1b[?1016h");
            }
        }
        if self.focus_reporting == Some(true) {
            out.push_str("\x1b[?1004h");
        }
        if self.bracketed_paste == Some(true) {
            out.push_str("\x1b[?2004h");
        }
        if self.application_cursor_keys == Some(true) {
            out.push_str("\x1b[?1h");
        }
        if self.application_keypad == Some(true) {
            out.push_str("\x1b[?66h");
        }
        if self.origin == Some(true) {
            out.push_str("\x1b[?6h");
        }
        // 自动换行默认是开，所以只可能恢复它被关掉的那一侧
        if self.wraparound == Some(false) {
            out.push_str("\x1b[?7l");
        }
        if self.reverse_wraparound == Some(true) {
            out.push_str("\x1b[?45h");
        }
        if self.insert == Some(true) {
            out.push_str("\x1b[4h");
        }
        if self.newline == Some(true) {
            out.push_str("\x1b[20h");
        }
        if let Some(blink) = self.cursor_blink {
            out.push_str(if blink { "\x1b[?12h" } else { "\x1b[?12l" });
        }
        if self.cursor_visible == Some(false) {
            out.push_str("\x1b[?25l");
        }
        out
    }
}

/// 模式设置序列的增量扫描器。
///
/// 逐字节推进的显式状态机：跨 chunk 只需要记住当前状态，不依赖任何窗口大小。窗口式
/// 实现（只看每块末尾 N 字节）在块大于窗口时必然漏扫。
#[derive(Default)]
struct ModeScanner {
    state: ScanState,
    dec: bool,
    params: Vec<u16>,
    cur: u16,
    has_digit: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Csi,
}

impl ModeScanner {
    fn feed(&mut self, data: &[u8], mode: &mut ModeState) {
        for &b in data {
            self.step(b, mode);
        }
    }

    fn reset_sequence(&mut self) {
        self.dec = false;
        self.params.clear();
        self.cur = 0;
        self.has_digit = false;
    }

    fn step(&mut self, b: u8, mode: &mut ModeState) {
        match self.state {
            ScanState::Ground => {
                if b == 0x1b {
                    self.state = ScanState::Esc;
                }
            }
            ScanState::Esc => {
                self.state = if b == b'[' {
                    self.reset_sequence();
                    ScanState::Csi
                } else if b == 0x1b {
                    ScanState::Esc
                } else {
                    ScanState::Ground
                };
            }
            ScanState::Csi => match b {
                0x1b => {
                    self.state = ScanState::Esc;
                }
                b'?' if !self.has_digit && self.params.is_empty() => self.dec = true,
                b'0'..=b'9' => {
                    self.cur = self.cur.saturating_mul(10).saturating_add((b - b'0') as u16);
                    self.has_digit = true;
                }
                b';' => {
                    self.params.push(self.cur);
                    self.cur = 0;
                    self.has_digit = false;
                }
                b'h' | b'l' => {
                    if self.has_digit || !self.params.is_empty() {
                        self.params.push(self.cur);
                    }
                    let enable = b == b'h';
                    for param in self.params.drain(..) {
                        mode.apply(param, enable, self.dec);
                    }
                    self.state = ScanState::Ground;
                }
                // 不是模式设置序列（SGR、CUP、DECRQM 等）：放弃，重新找 ESC
                _ => self.state = ScanState::Ground,
            },
        }
    }
}

/// 终端模型。
pub struct Model {
    terminal: Terminal,
    capture: Arc<Mutex<Vec<u8>>>,
    mode: ModeState,
    scanner: ModeScanner,
}

impl Model {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let capture = Arc::new(Mutex::new(Vec::new()));
        let size = TerminalSize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        };
        let mut terminal = Terminal::new(
            size,
            Arc::new(EmbeddedConfig { scrollback }),
            "pywezterm",
            env!("CARGO_PKG_VERSION"),
            Box::new(CaptureWriter(capture.clone())),
        );
        // 锚顶 resize 语义：resize 时内容锚顶、光标绑定文本行（保留 scrollback），
        // 各平台一致；同时抑制 ConPTY 启动时那条 title OSC。
        terminal.enable_conpty_quirks();
        Self {
            terminal,
            capture,
            mode: ModeState::default(),
            scanner: ModeScanner::default(),
        }
    }

    /// 喂入子进程输出的 VT 字节流。
    pub fn feed(&mut self, data: &[u8]) {
        self.scanner.feed(data, &mut self.mode);
        self.terminal.advance_bytes(data);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.terminal.resize(TerminalSize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
            dpi: 0,
        });
    }

    /// 重置到初始状态（RIS）：擦除屏幕与 scrollback、重置全部状态。
    ///
    /// 用 RIS 而不是 `full_reset()` —— 后者只清 keyboard stack，不清可见内容。
    pub fn reset(&mut self) {
        self.terminal.advance_bytes(b"\x1bc");
    }

    pub fn clear_scrollback(&mut self) {
        self.terminal.erase_scrollback();
    }

    pub fn screen(&self) -> &Screen {
        self.terminal.screen()
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// 光标 `(列, 行, 可见)`，坐标为**模型内**相对值；视口偏移由 [`super::View`] 施加。
    pub fn cursor(&self) -> (usize, i64, bool) {
        let c = self.terminal.cursor_pos();
        let visible = matches!(c.visibility, wezterm_surface::CursorVisibility::Visible);
        (c.x, c.y, visible)
    }

    pub fn current_seqno(&self) -> usize {
        self.terminal.current_seqno()
    }

    pub fn changed_stable_rows(&self, since_seqno: usize) -> Vec<isize> {
        let screen = self.screen();
        screen.get_changed_stable_rows(0..screen.scrollback_rows() as isize, since_seqno)
    }

    /// 取走捕获缓冲中的全部字节（模型自发产生的应答与编码输出）。
    pub fn drain_written(&mut self) -> Vec<u8> {
        self.terminal.flush_sync();
        std::mem::take(&mut *self.capture.lock().unwrap())
    }

    /// 丢弃捕获缓冲（编码前清场，只取本次产生的字节）。
    pub fn clear_capture(&mut self) {
        self.capture.lock().unwrap().clear();
    }

    // ---- 键鼠编码 --------------------------------------------------------

    /// 键盘按下编码。产生的字节留在捕获缓冲里，由 `drain_written` 取走。
    pub fn key_down(&mut self, key: KeyCode, mods: KeyModifiers) -> Result<()> {
        self.terminal
            .key_down(key, mods)
            .map_err(|e| Error::Render(format!("key_down 编码失败: {e:#}")))
    }

    pub fn key_up(&mut self, key: KeyCode, mods: KeyModifiers) -> Result<()> {
        self.terminal
            .key_up(key, mods)
            .map_err(|e| Error::Render(format!("key_up 编码失败: {e:#}")))
    }

    pub fn mouse_event(&mut self, ev: MouseEvent) -> Result<()> {
        self.terminal
            .mouse_event(ev)
            .map_err(|e| Error::Render(format!("mouse 编码失败: {e:#}")))
    }

    pub fn send_paste(&mut self, text: &str) -> Result<()> {
        self.terminal
            .send_paste(text)
            .map_err(|e| Error::Render(format!("send_paste 失败: {e}")))
    }

    pub fn focus_changed(&mut self, focused: bool) {
        self.terminal.focus_changed(focused);
    }

    pub fn make_all_lines_dirty(&mut self) {
        self.terminal.make_all_lines_dirty();
    }

    // ---- 模式与元数据查询 ------------------------------------------------

    /// 应用是否接管鼠标（DECSET 1000/1002/1003）。
    pub fn is_mouse_grabbed(&self) -> bool {
        self.mode.mouse_tracking.unwrap_or(0) > 0
    }

    /// 鼠标追踪模式与 SGR 编码：`(mode, sgr)`，`mode ∈ {0, 1000, 1002, 1003}`。
    pub fn mouse_encoding(&self) -> (u16, bool) {
        (
            self.mode.mouse_tracking.unwrap_or(0),
            self.mode.sgr_mouse.unwrap_or(false),
        )
    }

    /// 是否处于备用屏幕。
    pub fn is_alt_screen_active(&self) -> bool {
        self.mode.alt_screen.unwrap_or(false)
    }

    /// bracketed paste 是否启用。
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.mode.bracketed_paste.unwrap_or(false)
    }

    /// 终端模式恢复序列（新订阅者重建客户端状态用）。
    pub fn mode_restore_seq(&self) -> String {
        self.mode.restore_seq()
    }

    pub fn keyboard_encoding(&self) -> String {
        use termwiz::input::KeyboardEncoding;
        match self.terminal.get_keyboard_encoding() {
            KeyboardEncoding::Xterm => "xterm",
            KeyboardEncoding::CsiU => "csi-u",
            KeyboardEncoding::Win32 => "win32",
            KeyboardEncoding::Kitty(_) => "kitty",
        }
        .to_string()
    }

    pub fn title(&self) -> String {
        self.terminal.get_title().to_string()
    }

    pub fn current_dir(&self) -> Option<String> {
        self.terminal.get_current_dir().map(|url| url.to_string())
    }

    /// `(标签, 百分比)`；标签 ∈ `none` / `percentage` / `error` / `indeterminate`。
    pub fn progress(&self) -> (String, Option<u8>) {
        match self.terminal.get_progress() {
            Progress::None => ("none".to_string(), None),
            Progress::Percentage(p) => ("percentage".to_string(), Some(p)),
            Progress::Error(p) => ("error".to_string(), Some(p)),
            Progress::Indeterminate => ("indeterminate".to_string(), None),
        }
    }

    /// 语义区 `(start_y, start_x, end_y, end_x, 类型)`（OSC 133）。
    pub fn semantic_zones(&mut self) -> Result<Vec<(isize, usize, isize, usize, String)>> {
        let zones = self
            .terminal
            .get_semantic_zones()
            .map_err(|e| Error::Render(format!("get_semantic_zones 失败: {e:#}")))?;
        Ok(zones
            .into_iter()
            .map(|z| {
                let ty = match z.semantic_type {
                    SemanticType::Prompt => "prompt",
                    SemanticType::Input => "input",
                    SemanticType::Output => "output",
                };
                (z.start_y, z.start_x, z.end_y, z.end_x, ty.to_string())
            })
            .collect())
    }

    // ---- 回调 ------------------------------------------------------------

    pub fn set_clipboard(&mut self, cb: Arc<dyn Clipboard>) {
        self.terminal.set_clipboard(&cb);
    }

    pub fn set_download_handler(&mut self, cb: Arc<dyn DownloadHandler>) {
        self.terminal.set_download_handler(&cb);
    }

    pub fn set_device_control_handler(&mut self, cb: Box<dyn DeviceControlHandler>) {
        self.terminal.set_device_control_handler(cb);
    }

    pub fn set_notification_handler(&mut self, cb: Box<dyn AlertHandler>) {
        self.terminal.set_notification_handler(cb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 `chunk` 字节切分喂入，返回最终模式状态。
    fn scan(data: &[u8], chunk: usize) -> ModeState {
        let mut scanner = ModeScanner::default();
        let mut mode = ModeState::default();
        for part in data.chunks(chunk.max(1)) {
            scanner.feed(part, &mut mode);
        }
        mode
    }

    /// 扫描结果必须与切分方式无关 —— 窗口式实现正是在这里出错的。
    #[test]
    fn scan_is_chunk_invariant() {
        let mut data = Vec::new();
        // 把模式序列埋在大量普通输出里，且远离块尾
        for i in 0..200 {
            data.extend_from_slice(format!("line {i} with some padding text\r\n").as_bytes());
            if i == 3 {
                data.extend_from_slice(b"\x1b[?1000h\x1b[?1006h\x1b[?2004h");
            }
            if i == 150 {
                data.extend_from_slice(b"\x1b[?1049h\x1b[?25l\x1b[4h");
            }
        }
        let want = scan(&data, data.len());
        assert_eq!(want.mouse_tracking, Some(1000));
        assert_eq!(want.sgr_mouse, Some(true));
        assert_eq!(want.bracketed_paste, Some(true));
        assert_eq!(want.alt_screen, Some(true));
        assert_eq!(want.cursor_visible, Some(false));
        assert_eq!(want.insert, Some(true));

        for chunk in [1, 2, 3, 7, 63, 64, 65, 127, 4096] {
            let got = scan(&data, chunk);
            assert_eq!(got.mouse_tracking, want.mouse_tracking, "chunk={}", chunk);
            assert_eq!(got.sgr_mouse, want.sgr_mouse, "chunk={}", chunk);
            assert_eq!(got.bracketed_paste, want.bracketed_paste, "chunk={}", chunk);
            assert_eq!(got.alt_screen, want.alt_screen, "chunk={}", chunk);
            assert_eq!(got.cursor_visible, want.cursor_visible, "chunk={}", chunk);
            assert_eq!(got.insert, want.insert, "chunk={}", chunk);
        }
    }

    #[test]
    fn scan_handles_split_sequence() {
        // 序列被切成两半，甚至切在 ESC 与 '[' 之间
        assert_eq!(scan(b"\x1b[?10", 8).mouse_tracking, None);
        assert_eq!(scan(b"\x1b[?10\x1b[?10", 8).mouse_tracking, None);
        let split_at_esc = scan(b"\x1b", 1);
        assert_eq!(split_at_esc.mouse_tracking, None);
        let joined = scan(b"\x1b[?1002h", 1);
        assert_eq!(joined.mouse_tracking, Some(1002));
    }

    #[test]
    fn scan_ignores_non_mode_sequences() {
        // SGR / CUP / DECRQM 都不是模式设置序列，且不能干扰后续扫描
        let m = scan(b"\x1b[31m\x1b[1;1H\x1b[?25$p\x1b[?1003h", 5);
        assert_eq!(m.mouse_tracking, Some(1003));
    }

    #[test]
    fn scan_resets_mouse_mode() {
        assert_eq!(scan(b"\x1b[?1002h\x1b[?1002l", 4).mouse_tracking, Some(0));
    }

    #[test]
    fn restore_seq_only_emits_observed_modes() {
        // 什么都没观察到 → 什么都不发（否则会覆盖客户端自己的配置）
        assert_eq!(ModeState::default().restore_seq(), "");

        let m = scan(b"\x1b[?12l\x1b[?2004h", 3);
        let seq = m.restore_seq();
        assert!(seq.contains("\x1b[?12l"), "应恢复被关掉的闪烁: {}", seq);
        assert!(seq.contains("\x1b[?2004h"), "应恢复 bracketed paste: {}", seq);
        // 没设置过的模式不得出现
        assert!(!seq.contains("1049"), "未设置备用屏却发了: {}", seq);
        assert!(!seq.contains("?25"), "未设置光标可见性却发了: {}", seq);
    }

    #[test]
    fn restore_seq_puts_alt_screen_first() {
        let m = scan(b"\x1b[?1049h\x1b[?25l", 4);
        let seq = m.restore_seq();
        assert!(seq.starts_with("\x1b[?1049h"), "备用屏必须先进入: {}", seq);
        assert!(seq.ends_with("\x1b[?25l"), "光标可见性最后: {}", seq);
    }

    #[test]
    fn model_reports_mode_through_public_api() {
        let mut m = Model::new(80, 24, 100);
        m.feed(b"\x1b[?1006h\x1b[?1000h");
        assert!(m.is_mouse_grabbed());
        assert_eq!(m.mouse_encoding(), (1000, true));
        m.feed(b"\x1b[?1000l");
        assert!(!m.is_mouse_grabbed());
        assert_eq!(m.mouse_encoding(), (0, true));
    }
}
