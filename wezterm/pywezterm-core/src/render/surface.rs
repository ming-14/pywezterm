//! 增量渲染表面：单元格写入 → 自基线以来的增量 → ANSI 字节。
//!
//! 单元格级 diff 交给 `wezterm-surface`，`Change` → 字节交给 termwiz 的 terminfo
//! renderer。本模块只做两件必要的事：
//!
//! 1. **坐标语义换算**：`wezterm-surface` 的 `CursorPosition` 是 `(x=列, y=行)`，
//!    而 terminfo renderer 把 `x` 当行、`y` 当列，渲染前必须互换；
//! 2. **SGR 归一化**：把 T.416 冒号形式（`38:5:N` / `38:2::R:G:B`）改成分号形式，
//!    后者兼容性更广。

use std::sync::OnceLock;

use termwiz::caps::{Capabilities, ColorLevel, ProbeHints};
use termwiz::render::terminfo::TerminfoRenderer;
use wezterm_surface::{Change, Position};
use wezterm_term::{CellAttributes, Intensity, Underline};

use crate::error::{Error, Result};
use crate::term::grid::{Attrs, Color};

/// 增量渲染表面。
pub struct Surface {
    surface: wezterm_surface::Surface,
}

impl Surface {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            surface: wezterm_surface::Surface::new(cols, rows),
        }
    }

    pub fn dimensions(&self) -> (usize, usize) {
        self.surface.dimensions()
    }

    /// 调整尺寸；尺寸变化会丢弃已缓冲的变更，下一帧必然全量。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.surface.resize(cols, rows);
    }

    /// 清空整屏。
    ///
    /// 不能只往变更流里塞一条 `ClearScreen`：模型里的格子还在，下一次全量重绘会把
    /// 旧内容原样画回来。重建表面才是真的清空（序号归零，调用方的旧基线自动退化为
    /// 全量重绘）。
    pub fn clear(&mut self) {
        let (cols, rows) = self.surface.dimensions();
        *self = Self::new(cols, rows);
    }

    /// 当前序号（增量基线）。
    pub fn current_seqno(&self) -> usize {
        self.surface.current_seqno()
    }

    /// 在 `(x, y)` 写入一段带样式的文本。
    pub fn put(&mut self, x: usize, y: usize, text: &str, fg: Color, bg: Color, attrs: Attrs) {
        if text.is_empty() {
            return;
        }
        self.surface.add_change(Change::CursorPosition {
            x: Position::Absolute(x),
            y: Position::Absolute(y),
        });
        self.surface.add_change(Change::AllAttributes(cell_attributes(fg, bg, attrs)));
        self.surface.add_change(Change::Text(text.to_string()));
    }

    /// 自 `since` 以来的增量 ANSI 字节；无变化时字节为空。
    ///
    /// 基线过旧或增量预算超限时，`get_changes` 会自行改为全量重绘。
    pub fn changes_bytes(&mut self, since: usize) -> Result<(usize, Vec<u8>)> {
        let (cols, rows) = self.surface.dimensions();
        let (seq, bytes) = {
            let (seq, changes) = self.surface.get_changes(since);
            let bytes = if changes.is_empty() {
                Vec::new()
            } else {
                render_changes(&changes, cols, rows)?
            };
            (seq, bytes)
        };
        self.surface.flush_changes_older_than(seq);
        Ok((seq, bytes))
    }

    /// 强制全量重绘。
    ///
    /// `get_changes(0)` 在 wezterm-surface 里恒走全量分支。
    pub fn repaint_bytes(&mut self) -> Result<(usize, Vec<u8>)> {
        self.changes_bytes(0)
    }
}

/// `(前景, 背景, 样式)` → 单元格属性。
pub fn cell_attributes(fg: Color, bg: Color, attrs: Attrs) -> CellAttributes {
    let mut out = CellAttributes::default();
    out.set_foreground(fg.to_attr());
    out.set_background(bg.to_attr());
    if attrs.bold {
        out.set_intensity(Intensity::Bold);
    }
    if attrs.italic {
        out.set_italic(true);
    }
    if attrs.underline {
        out.set_underline(Underline::Single);
    }
    if attrs.reverse {
        out.set_reverse(true);
    }
    if attrs.strikethrough {
        out.set_strikethrough(true);
    }
    out
}

/// `Change` 流 → ANSI 字节。
///
/// `cols` / `rows` 用于把越界坐标 clamp 回合法范围：文本写满整行后表面内部光标会推进
/// 到行尾边界外（如 100 列写满 → `x = 100`），全量重绘会把该位置输出成 CUP，宿主收到
/// 后 clamp 到行尾，表现为光标跳到最末列。
fn render_changes(changes: &[Change], cols: usize, rows: usize) -> Result<Vec<u8>> {
    let mut renderer = TerminfoRenderer::new(capabilities()?.clone());
    let mut buf: Vec<u8> = Vec::new();
    let mut tty = BufferTty { buf: &mut buf };

    let (max_col, max_row) = (cols.saturating_sub(1), rows.saturating_sub(1));
    let fixed: Vec<Change> = changes
        .iter()
        .map(|c| match c {
            Change::CursorPosition {
                x: Position::Absolute(x),
                y: Position::Absolute(y),
            } => Change::CursorPosition {
                // 先 clamp，再互换（见模块头）
                x: Position::Absolute((*y).min(max_row)),
                y: Position::Absolute((*x).min(max_col)),
            },
            other => other.clone(),
        })
        .collect();

    renderer
        .render_to(&fixed, &mut tty)
        .map_err(|e| Error::Render(format!("渲染失败: {e:#}")))?;

    // 帧首强制 SGR 全重置：renderer 每次从默认状态开始，但宿主终端还停在上一帧末尾的
    // 颜色上，不重置会让本帧的默认色 run 沿用旧颜色（多 pane 之间互相串色）。
    let mut out = Vec::with_capacity(buf.len() + 4);
    out.extend_from_slice(b"\x1b[0m");
    out.extend_from_slice(&normalize_sgr_colon(&buf));
    Ok(out)
}

/// ANSI SGR 渲染能力。构建过程会探针环境，因此只做一次并缓存结果（含失败）。
fn capabilities() -> Result<&'static Capabilities> {
    static CAPS: OnceLock<std::result::Result<Capabilities, String>> = OnceLock::new();
    CAPS.get_or_init(|| {
        let hints = ProbeHints::default()
            .color_level(Some(ColorLevel::TrueColor))
            .force_terminfo_render_to_use_ansi_sgr(Some(true));
        Capabilities::new_with_hints(hints).map_err(|e| format!("构建渲染能力失败: {e:#}"))
    })
    .as_ref()
    .map_err(|e| Error::Render(e.clone()))
}

/// 把 ITU T.416 冒号形式的颜色 SGR 归一化为分号形式。
///
/// `38:5:N` → `38;5;N`，`38:2::R:G:B` → `38;2;R;G;B`（双冒号里是 colorspace 空槽）。
fn normalize_sgr_colon(buf: &[u8]) -> Vec<u8> {
    const CSI: u8 = 0x1b;
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        let is_csi = buf[i] == CSI && buf.get(i + 1) == Some(&b'[');
        if is_csi {
            // 参数以 'm' 结束；颜色参数的冒号只会出现在数字之间，因此首个 'm' 即终点
            if let Some(rel) = buf[i + 2..].iter().position(|&b| b == b'm') {
                let end = i + 2 + rel;
                let params = &buf[i + 2..end];
                if params.len() >= 3 && matches!(&params[..3], b"38:" | b"48:" | b"58:") {
                    out.extend_from_slice(&[CSI, b'[']);
                    let mut first = true;
                    for seg in params.split(|&b| b == b':').filter(|s| !s.is_empty()) {
                        if !first {
                            out.push(b';');
                        }
                        out.extend_from_slice(seg);
                        first = false;
                    }
                    out.push(b'm');
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push(buf[i]);
        i += 1;
    }
    out
}

/// `RenderTty` 只需 `Write`；渲染到内存缓冲时尺寸用不到。
struct BufferTty<'a> {
    buf: &'a mut Vec<u8>,
}

impl std::io::Write for BufferTty<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl termwiz::render::RenderTty for BufferTty<'_> {
    fn get_size_in_cells(&mut self) -> termwiz::Result<(usize, usize)> {
        Ok((0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_colon_256_and_truecolor() {
        assert_eq!(normalize_sgr_colon(b"\x1b[38:5:208mX"), b"\x1b[38;5;208mX");
        assert_eq!(normalize_sgr_colon(b"\x1b[48:5:17mX"), b"\x1b[48;5;17mX");
        assert_eq!(
            normalize_sgr_colon(b"\x1b[38:2::255:0:255mX"),
            b"\x1b[38;2;255;0;255mX"
        );
        assert_eq!(
            normalize_sgr_colon(b"\x1b[48:2::1:2:3mX"),
            b"\x1b[48;2;1;2;3mX"
        );
    }

    #[test]
    fn sgr_colon_leaves_other_sequences_alone() {
        for seq in [
            &b"\x1b[31mX"[..],
            b"\x1b[1;32mX",
            b"\x1b[0mX",
            b"\x1b[2J",
            b"\x1b[1;1H",
        ] {
            assert_eq!(normalize_sgr_colon(seq), seq);
        }
    }

    #[test]
    fn sgr_colon_mixed_sequence() {
        let input = b"\x1b[38:5:208mO\x1b[39mT\x1b[38:2::10:20:30mX";
        let want = b"\x1b[38;5;208mO\x1b[39mT\x1b[38;2;10;20;30mX";
        assert_eq!(normalize_sgr_colon(input), want);
    }

    #[test]
    fn first_frame_is_full_repaint() {
        let mut s = Surface::new(4, 2);
        s.put(0, 0, "hi", Color::Palette(1), Color::Default, Attrs::default());
        let (seq, bytes) = s.changes_bytes(0).unwrap();
        assert!(seq > 0);
        assert!(bytes.starts_with(b"\x1b[0m"), "帧首应重置 SGR");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn no_change_yields_no_bytes() {
        let mut s = Surface::new(4, 2);
        s.put(0, 0, "hi", Color::Default, Color::Default, Attrs::default());
        let (seq, _) = s.changes_bytes(0).unwrap();
        let (seq2, bytes) = s.changes_bytes(seq).unwrap();
        assert_eq!(seq, seq2);
        assert!(bytes.is_empty(), "无变化不应产生字节");
    }

    #[test]
    fn clear_actually_clears() {
        let mut s = Surface::new(4, 2);
        s.put(0, 0, "hi", Color::Default, Color::Default, Attrs::default());
        let _ = s.changes_bytes(0).unwrap();
        s.clear();
        // 清空后全量重绘不得把旧内容画回来
        let (_, bytes) = s.repaint_bytes().unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("hi"), "clear 后旧内容仍在: {:?}", text);
    }

    /// 坐标语义：把渲染结果喂回一个终端模型，直接看落点。
    ///
    /// 不断言具体转义序列 —— renderer 会用相对移动等更短的形式表达同一个位置。
    #[test]
    fn writes_land_at_requested_coordinates() {
        let mut s = Surface::new(8, 2);
        s.put(3, 1, "x", Color::Default, Color::Default, Attrs::default());
        let (_, bytes) = s.repaint_bytes().unwrap();

        let mut term = crate::term::Model::new(8, 2, 0);
        term.feed(&bytes);
        let line = &term.screen().lines_in_phys_range(1..2)[0];
        let hit = crate::term::cells_of_line(line)
            .into_iter()
            .find(|c| c.text == "x")
            .expect("x 未出现在第 1 行");
        assert_eq!(hit.col, 3, "列错误");
    }

    /// 行列不能写反：写在 (0,1) 与 (1,0) 的两个字符必须落在不同格子。
    #[test]
    fn row_and_column_are_not_swapped() {
        let mut s = Surface::new(8, 2);
        s.put(0, 1, "a", Color::Default, Color::Default, Attrs::default());
        s.put(1, 0, "b", Color::Default, Color::Default, Attrs::default());
        let (_, bytes) = s.repaint_bytes().unwrap();

        let mut term = crate::term::Model::new(8, 2, 0);
        term.feed(&bytes);
        let at = |row: std::ops::Range<usize>| -> Vec<(usize, String)> {
            crate::term::cells_of_line(&term.screen().lines_in_phys_range(row)[0])
                .into_iter()
                .map(|c| (c.col, c.text))
                .collect()
        };
        assert!(at(0..1).contains(&(1, "b".to_string())), "b 应在第 0 行第 1 列");
        assert!(at(1..2).contains(&(0, "a".to_string())), "a 应在第 1 行第 0 列");
    }
}
