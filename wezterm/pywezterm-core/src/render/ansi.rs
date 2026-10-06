//! 文本输出：可见屏 ANSI、历史区、纯文本。

use wezterm_term::screen::Screen;

use crate::term::grid::{cells_of_line, Cell, Color};
use crate::term::View;

use super::cursor_seq;

/// 单元格样式 → SGR 序列。全默认时返回 `\x1b[0m`（同时清掉上一格的属性）。
fn cell_sgr(cell: &Cell) -> String {
    let a = &cell.attrs;
    let mut parts: Vec<String> = Vec::new();
    if a.bold {
        parts.push("1".into());
    }
    if a.italic {
        parts.push("3".into());
    }
    if a.underline {
        parts.push("4".into());
    }
    if a.reverse {
        parts.push("7".into());
    }
    if a.strikethrough {
        parts.push("9".into());
    }
    if let Some(s) = sgr_color(cell.fg, true) {
        parts.push(s);
    }
    if let Some(s) = sgr_color(cell.bg, false) {
        parts.push(s);
    }
    if parts.is_empty() {
        "\x1b[0m".to_string()
    } else {
        format!("\x1b[{}m", parts.join(";"))
    }
}

/// 颜色 → SGR 颜色段；默认色不输出（沿用当前状态）。
fn sgr_color(color: Color, is_fg: bool) -> Option<String> {
    let prefix = if is_fg { 38 } else { 48 };
    match color {
        Color::Default => None,
        Color::Palette(n) => Some(format!("{prefix};5;{n}")),
        Color::Rgb(r, g, b) => Some(format!("{prefix};2;{r};{g};{b}")),
    }
}

/// 逐行回调可见区，避免 `lines_in_phys_range` 的整行深拷贝。
fn for_each_visible_line<R>(screen: &Screen, view: &View, mut f: impl FnMut(&wezterm_term::Line) -> R) {
    screen.with_phys_lines(view.window(screen), |lines| {
        for line in lines {
            f(line);
        }
    });
}

/// 一行的 SGR 文本：只在样式变化处发 SGR，末尾回到默认。
fn styled_line(cells: &[Cell]) -> String {
    let mut out = String::new();
    let mut last_sgr = String::new();
    for cell in cells {
        if cell.text.is_empty() {
            continue;
        }
        let sgr = cell_sgr(cell);
        if sgr != last_sgr {
            out.push_str(&sgr);
            last_sgr = sgr;
        }
        out.push_str(&cell.text);
    }
    if !last_sgr.is_empty() {
        out.push_str("\x1b[0m");
    }
    out
}

/// 可见屏幕 ANSI：逐行 `CSI row;1H` + `CSI K` 清行尾 + SGR 文本，末尾空行截断。
///
/// 行首先清行尾是必要的：短内容覆盖长内容时旧字符会残留。
/// `include_cursor` 为真时追加光标定位（不可见或滚出可见区则只发隐藏）。
pub fn screen(
    screen: &Screen,
    view: &View,
    cursor: (usize, i64, bool),
    include_cursor: bool,
) -> String {
    let mut rendered: Vec<String> = Vec::new();
    let mut last_non_empty = 0;
    for_each_visible_line(screen, view, |line| {
        let cells = cells_of_line(line);
        if cells.iter().any(|c| !c.is_blank()) {
            last_non_empty = rendered.len();
        }
        rendered.push(styled_line(&cells));
    });

    let mut out = String::new();
    for (i, line) in rendered.iter().enumerate().take(last_non_empty + 1) {
        out.push_str(&format!("\x1b[{};1H\x1b[K", i + 1));
        out.push_str(line);
    }

    if include_cursor {
        let (col, row, visible) = cursor;
        match view.row_in_view(screen, row) {
            Some(view_row) if visible => {
                // 列号 clamp 到物理列宽：整行写满后光标落在边界外，超界定位会被
                // 宿主 clamp 到最末列，表现为光标跳到行尾
                let col = col.min(screen.physical_cols.saturating_sub(1));
                out.push_str(&cursor_seq(view_row, col, true));
            }
            _ => out.push_str("\x1b[?25l"),
        }
    }
    out
}

/// 可见屏幕纯文本：每行去尾空白、去掉末尾空行、行间 `\n`。
pub fn plain_text(screen: &Screen, view: &View) -> String {
    let mut lines: Vec<String> = Vec::new();
    for_each_visible_line(screen, view, |line| {
        let mut s = String::new();
        for c in line.visible_cells() {
            s.push_str(c.str());
        }
        while s.ends_with(' ') {
            s.pop();
        }
        lines.push(s);
    });
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// 历史区输出。
///
/// `keep_ansi` 为假时是纯文本（去尾空白、去末尾空行、行间 `\n`）；为真时每行 SGR
/// 文本 + `\r\n`，供前端恢复 scrollback。
pub fn scrollback(screen: &Screen, keep_ansi: bool) -> String {
    let mut out = String::new();
    screen.with_phys_lines(crate::term::view::history(screen), |lines| {
        for line in lines {
            let cells = cells_of_line(line);
            if keep_ansi {
                out.push_str(&styled_line(&cells));
                out.push_str("\r\n");
                continue;
            }
            let mut s: String = cells.iter().map(|c| c.text.as_str()).collect();
            while s.ends_with(' ') {
                s.pop();
            }
            out.push_str(&s);
            out.push('\n');
        }
    });
    if !keep_ansi {
        while out.ends_with("\n\n") {
            out.pop();
        }
        if out.ends_with('\n') {
            out.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::term::Model;

    fn model_with(feed: &[u8], cols: usize, rows: usize) -> Model {
        let mut m = Model::new(cols, rows, 1_000);
        m.feed(feed);
        m
    }

    #[test]
    fn plain_text_trims_and_drops_trailing_blanks() {
        let m = model_with(b"hello   \r\nworld\r\n\r\n\r\n", 20, 6);
        assert_eq!(plain_text(m.screen(), &View::default()), "hello\nworld");
    }

    #[test]
    fn screen_positions_each_line_and_clears_tail() {
        let m = model_with(b"ab\r\ncd\r\n", 20, 4);
        let out = screen(m.screen(), &View::default(), (0, 0, true), false);
        assert!(out.starts_with("\x1b[1;1H\x1b[K"), "首行定位: {:?}", out);
        assert!(out.contains("\x1b[2;1H\x1b[K"), "次行定位: {:?}", out);
        assert!(out.contains("ab") && out.contains("cd"));
        // 只有两行有内容，第三行不该出现
        assert!(!out.contains("\x1b[3;1H"), "末尾空行应截断: {:?}", out);
    }

    #[test]
    fn screen_emits_sgr_for_colors() {
        let m = model_with(b"\x1b[31mred\x1b[0m\r\n", 20, 3);
        let out = screen(m.screen(), &View::default(), (0, 0, true), false);
        assert!(out.contains("\x1b[38;5;1m"), "前景色 SGR: {:?}", out);
        assert!(out.contains("\x1b[0m"), "末尾应回到默认: {:?}", out);
    }

    #[test]
    fn screen_hides_cursor_when_invisible() {
        let m = model_with(b"abc\r\n\x1b[?25l", 20, 3);
        let out = screen(m.screen(), &View::default(), (3, 1, false), true);
        assert!(out.ends_with("\x1b[?25l"), "不可见光标只发隐藏: {:?}", out);
    }

    #[test]
    fn screen_clamps_cursor_column_at_line_end() {
        // 写满整行后光标在边界外，必须 clamp 回最后一列
        let m = model_with(b"12345", 5, 3);
        let (col, row, vis) = m.cursor();
        let out = screen(m.screen(), &View::default(), (col, row, vis), true);
        assert!(out.contains("\x1b[1;5H"), "应 clamp 到最后一列: {:?}", out);
    }

    #[test]
    fn scrollback_plain_and_ansi() {
        let mut m = Model::new(10, 2, 100);
        for i in 0..6 {
            m.feed(format!("line{i}\r\n").as_bytes());
        }
        let plain = scrollback(m.screen(), false);
        assert!(plain.contains("line0"), "历史区应含最早行: {:?}", plain);
        assert!(!plain.ends_with('\n'), "末尾换行应剔除: {:?}", plain);

        let ansi = scrollback(m.screen(), true);
        assert!(ansi.ends_with("\r\n"), "ANSI 模式每行以 CRLF 结尾");
    }
}
