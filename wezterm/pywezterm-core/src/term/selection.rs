//! 选区状态机（纯函数，可单测）。
//!
//! 坐标模型是 **stable 行 + 列**：跨 scrollback 与可见区，且不受视口滚动影响
//! （与语义区坐标基准一致）。

use wezterm_term::screen::Screen;

use crate::term::grid::cells_of_line;

/// 选区类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectionKind {
    /// 区域选择（拖拽）：锚点 → 端点，区间内全部文本
    Region,
    /// 双击选词：锚点所在词的边界（空白/标点分隔）
    Word,
    /// 三击选行：锚点所在物理行整行（含结尾换行）
    Line,
}

/// 选区状态。锚点为 `None` 表示无选区。
#[derive(Clone, Debug)]
pub struct Selection {
    anchor: Option<(isize, usize)>,
    kind: SelectionKind,
    end: (isize, usize),
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            anchor: None,
            kind: SelectionKind::Region,
            end: (0, 0),
        }
    }
}

impl Selection {
    /// 区域选择：anchor → end。
    pub fn set_region(&mut self, anchor: (isize, usize), end: (isize, usize)) {
        self.anchor = Some(anchor);
        self.kind = SelectionKind::Region;
        self.end = end;
    }

    /// 双击选词：命中空白或标点时不选区。
    pub fn select_word(&mut self, screen: &Screen, row: isize, col: usize) {
        if let Some((start, end)) = word_bounds(screen, row, col) {
            self.anchor = Some((row, start));
            self.kind = SelectionKind::Word;
            self.end = (row, end);
        }
    }

    /// 三击选行：整行（含换行）。
    pub fn select_line(&mut self, row: isize, _col: usize) {
        self.anchor = Some((row, 0));
        self.kind = SelectionKind::Line;
        self.end = (row, usize::MAX);
    }

    pub fn clear(&mut self) {
        self.anchor = None;
    }

    pub fn is_active(&self) -> bool {
        self.anchor.is_some()
    }

    /// 选区纯文本；无选区返回空串。
    ///
    /// 首行截 `[start_col, ∞)`、末行截 `[0, end_col]`（闭区间），中间行整行；
    /// 宽字符的续格被跳过；行尾空白裁剪。
    pub fn text(&self, screen: &Screen) -> String {
        let Some(anchor) = self.anchor else {
            return String::new();
        };
        let (lo, hi, start_col, end_col) = if anchor.0 <= self.end.0 {
            (anchor.0, self.end.0, anchor.1, self.end.1)
        } else {
            (self.end.0, anchor.0, self.end.1, anchor.1)
        };

        let mut lines = Vec::new();
        for stable in lo..=hi {
            // 该 stable 行已不在屏幕或历史中：跳过
            let Some(phys) = screen.stable_row_to_phys(stable) else {
                continue;
            };
            let mut s = String::new();
            screen.with_phys_lines(phys..phys + 1, |ls| {
                let Some(line) = ls.first() else { return };
                for cell in cells_of_line(line) {
                    if cell.text.is_empty() {
                        continue;
                    }
                    if stable == lo && cell.col < start_col {
                        continue;
                    }
                    if stable == hi && cell.col > end_col {
                        continue;
                    }
                    s.push_str(&cell.text);
                }
            });
            lines.push(s);
        }

        let mut text = lines.join("\n");
        if self.kind == SelectionKind::Line && !text.is_empty() {
            text.push('\n');
        }
        while text.ends_with(' ') {
            text.pop();
        }
        text
    }
}

/// 词内字符：非空白且为字母数字或下划线。
fn is_word_char(ch: char) -> bool {
    !ch.is_whitespace() && (ch.is_alphanumeric() || ch == '_')
}

/// `(row, col)` 所在词的列区间 `[start, end]`（闭区间）；未命中词返回 `None`。
fn word_bounds(screen: &Screen, row: isize, col: usize) -> Option<(usize, usize)> {
    let phys = screen.stable_row_to_phys(row)?;
    let mut cells = Vec::new();
    screen.with_phys_lines(phys..phys + 1, |ls| {
        if let Some(line) = ls.first() {
            cells = cells_of_line(line);
        }
    });

    // 命中包含 col 的格（宽字符覆盖 [col, col + width)）
    let hit = cells
        .iter()
        .position(|c| col >= c.col && col < c.col + c.width.max(1) as usize)?;
    if !is_word_char(cells[hit].text.chars().next()?) {
        return None;
    }

    let mut start = hit;
    while start > 0 && is_word_char(cells[start - 1].text.chars().next()?) {
        start -= 1;
    }
    let mut end = hit;
    while end + 1 < cells.len() && is_word_char(cells[end + 1].text.chars().next()?) {
        end += 1;
    }
    Some((cells[start].col, cells[end].col + cells[end].width.max(1) as usize - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::term::Model;

    /// 构造已喂入文本的模型（列/行固定，避免滚动干扰）。
    fn model(text: &[u8], cols: usize, rows: usize) -> Model {
        let mut m = Model::new(cols, rows, 10_000);
        m.feed(text);
        m
    }

    fn all_lines(model: &Model) -> Vec<String> {
        let screen = model.screen();
        let mut out = Vec::new();
        for line in screen.lines_in_phys_range(0..screen.scrollback_rows()) {
            let mut s = String::new();
            for c in line.visible_cells() {
                s.push_str(c.str());
            }
            out.push(s.trim_end().to_string());
        }
        out
    }

    #[test]
    fn no_selection_is_empty() {
        let m = model(b"abc\r\ndef\r\n", 5, 3);
        let sel = Selection::default();
        assert_eq!(sel.text(m.screen()), "");
        assert!(!sel.is_active());
    }

    #[test]
    fn region_single_row_includes_end_col() {
        let m = model(b"abcdef\r\n", 10, 3);
        let mut sel = Selection::default();
        sel.set_region((0, 1), (0, 3));
        assert_eq!(sel.text(m.screen()), "bcd");
    }

    #[test]
    fn region_across_rows() {
        let m = model(b"abc\r\ndef\r\n", 5, 3);
        let mut sel = Selection::default();
        sel.set_region((0, 1), (1, 2));
        assert_eq!(sel.text(m.screen()), "bc\ndef");
    }

    #[test]
    fn region_reversed_drag() {
        let m = model(b"abc\r\ndef\r\n", 5, 3);
        let mut sel = Selection::default();
        sel.set_region((1, 0), (0, 2));
        assert_eq!(sel.text(m.screen()), "c\nd");
    }

    #[test]
    fn region_middle_rows_whole() {
        let m = model(b"aaa\r\nbbb\r\nccc\r\n", 5, 4);
        let mut sel = Selection::default();
        sel.set_region((0, 1), (2, 1));
        assert_eq!(sel.text(m.screen()), "aa\nbbb\ncc");
    }

    #[test]
    fn word_selection() {
        let m = model(b"hello world\r\n", 20, 3);
        let mut sel = Selection::default();
        sel.select_word(m.screen(), 0, 6);
        assert_eq!(sel.text(m.screen()), "world");
        assert!(sel.is_active());
    }

    #[test]
    fn word_selection_respects_punctuation() {
        let m = model(b"foo,bar(baz)\r\n", 20, 3);
        let mut sel = Selection::default();
        sel.select_word(m.screen(), 0, 2);
        assert_eq!(sel.text(m.screen()), "foo");
        sel.clear();
        sel.select_word(m.screen(), 0, 5);
        assert_eq!(sel.text(m.screen()), "bar");
    }

    #[test]
    fn word_selection_on_gap_selects_nothing() {
        let m = model(b"hello world\r\n", 20, 3);
        let mut sel = Selection::default();
        sel.select_word(m.screen(), 0, 5);
        assert!(!sel.is_active());
        assert_eq!(sel.text(m.screen()), "");
    }

    #[test]
    fn line_selection_includes_newline() {
        let m = model(b"hello world\r\n", 20, 3);
        let mut sel = Selection::default();
        sel.select_line(0, 3);
        assert_eq!(sel.text(m.screen()), "hello world\n");
    }

    #[test]
    fn clear_resets() {
        let m = model(b"abc\r\n", 5, 3);
        let mut sel = Selection::default();
        sel.set_region((0, 0), (0, 2));
        assert!(sel.is_active());
        sel.clear();
        assert!(!sel.is_active());
        assert_eq!(sel.text(m.screen()), "");
    }

    #[test]
    fn wide_chars_use_cell_columns() {
        // "你"(0-1) "好"(2-3) "world"(4-8)
        let m = model("你好world\r\n".as_bytes(), 20, 3);
        let mut sel = Selection::default();
        sel.set_region((0, 2), (0, 6));
        assert_eq!(sel.text(m.screen()), "好wor");
    }

    #[test]
    fn trailing_spaces_trimmed() {
        let m = model(b"ab   \r\n", 10, 3);
        let mut sel = Selection::default();
        sel.set_region((0, 0), (0, 4));
        assert_eq!(sel.text(m.screen()), "ab");
    }

    /// 窄化→复原后 CJK 宽字符文本必须完整 —— 回归：dir 输出里的中文摘要行
    /// 窄化时宽字符会被按列切割，复原后必须恢复。
    fn assert_rewrap_cjk_integrity() {
        let mut feed = Vec::new();
        for i in 0..8 {
            feed.extend_from_slice(
                format!(
                    "2026/08/09  16:35             4,5{i:02} speedtest_nodes2_extra_long_file_name_{i:02}.py\r\n"
                )
                .as_bytes(),
            );
            if i == 4 {
                feed.extend_from_slice("              31 个文件        458,874 字节\r\n".as_bytes());
                feed.extend_from_slice("              85 个目录 156,158,554,112 可用字节\r\n".as_bytes());
            }
        }
        let mut m = model(&feed, 80, 10);
        m.resize(40, 10);
        m.resize(80, 10);

        let text = all_lines(&m).join("\n");
        assert!(
            text.contains("31 个文件") && text.contains("458,874 字节"),
            "CJK 行 rewrap 错乱: {:?}",
            text
        );
        assert!(
            text.contains("85 个目录") && text.contains("可用字节"),
            "CJK 行 rewrap 错乱: {:?}",
            text
        );
        for i in 0..8 {
            assert!(
                text.contains(&format!("file_name_{i:02}.py")),
                "文件行 {} 缺失或错乱: {:?}",
                i,
                text
            );
        }
    }

    #[test]
    fn rewrap_cjk_integrity() {
        assert_rewrap_cjk_integrity();
    }
}
