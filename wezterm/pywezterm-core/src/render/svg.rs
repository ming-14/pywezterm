//! SVG 矢量输出。
//!
//! 同色连续字符合并为一条 `<text>` 以减小体积；`compression_level >= 1` 时再做两步
//! 压缩：删掉空 `<text>`、折叠标签之间的空白。
//!
//! 所有字符串处理都按 **`&str` 边界**切片，不按字节重排 —— 逐字节转换会破坏 UTF-8
//! 多字节字符（中文、emoji 会变成乱码）。

use crate::term::grid::Cell;

use super::{CELL_H, CELL_W, DEFAULT_BG, DEFAULT_FG};

/// XML 转义（`&` `<` `>`）。
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// 渲染可见网格为 SVG。
pub fn render(lines: &[Vec<Cell>], cols: usize, rows: usize) -> String {
    let (w, h) = (cols * CELL_W, rows * CELL_H);
    let mut parts = vec![
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xml:space="preserve" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#
        ),
        format!(
            r##"<rect width="100%" height="100%" fill="#{:02x}{:02x}{:02x}"/>"##,
            DEFAULT_BG.0, DEFAULT_BG.1, DEFAULT_BG.2
        ),
        format!(
            r#"<style>text{{font-family:Consolas,"Microsoft YaHei",monospace;font-size:{}px;dominant-baseline:text-before-edge;white-space:pre}}</style>"#,
            CELL_H - 2
        ),
    ];

    for (y, line) in lines.iter().enumerate().take(rows) {
        // 同色 run 合并：run_x 是 run 起点列（像素），遇到样式变化才收尾
        let mut run = Run::default();
        for cell in line {
            if cell.text.is_empty() {
                continue;
            }
            let x = cell.col * CELL_W;
            if run.matches(cell) {
                run.text.push_str(&cell.text);
            } else {
                run.flush(&mut parts, y, x);
                run = Run::new(cell, x);
            }
        }
        run.flush(&mut parts, y, cols * CELL_W);
    }

    parts.push("</svg>".to_string());
    parts.join("\n")
}

/// 一条待输出的同色 run。
#[derive(Default)]
struct Run {
    text: String,
    x: usize,
    fg: (u8, u8, u8),
    bg: Option<(u8, u8, u8)>,
    bold: bool,
}

impl Run {
    fn new(cell: &Cell, x: usize) -> Self {
        Self {
            text: cell.text.clone(),
            x,
            fg: cell.fg.to_rgb().unwrap_or(DEFAULT_FG),
            bg: cell.bg.to_rgb(),
            bold: cell.attrs.bold,
        }
    }

    /// 样式一致才能并入当前 run。
    fn matches(&self, cell: &Cell) -> bool {
        !self.text.is_empty()
            && cell.fg.to_rgb().unwrap_or(DEFAULT_FG) == self.fg
            && cell.bg.to_rgb() == self.bg
            && cell.attrs.bold == self.bold
    }

    /// 输出到 `end_x`（下一个 run 的起点或行尾）。
    fn flush(&mut self, parts: &mut Vec<String>, y: usize, end_x: usize) {
        if self.text.is_empty() {
            return;
        }
        let (top, height) = (y * CELL_H, CELL_H);
        let width = end_x.saturating_sub(self.x);
        if let Some(bg) = self.bg {
            parts.push(format!(
                r##"<rect x="{}" y="{top}" width="{width}" height="{height}" fill="#{:02x}{:02x}{:02x}"/>"##,
                self.x, bg.0, bg.1, bg.2
            ));
        }
        let bold = if self.bold { r#" font-weight="bold""# } else { "" };
        parts.push(format!(
            r##"<text x="{}" y="{top}" fill="#{:02x}{:02x}{:02x}"{bold}>{}</text>"##,
            self.x,
            self.fg.0,
            self.fg.1,
            self.fg.2,
            xml_escape(&self.text)
        ));
        self.text.clear();
    }
}

/// 压缩 SVG：`level >= 1` 时删空 `<text>` 并折叠标签间空白。
pub fn compress(svg: &str, level: u8) -> String {
    if level == 0 {
        return svg.to_string();
    }
    collapse_intertag_whitespace(&strip_empty_text(svg))
}

/// 删掉内容为纯空白的 `<text>` 元素。
fn strip_empty_text(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(start) = rest.find("<text") {
        let Some(gt) = rest[start..].find('>') else {
            break;
        };
        let content_at = start + gt + 1;
        let Some(close) = rest[content_at..].find("</text>") else {
            break;
        };
        let content = &rest[content_at..content_at + close];
        if content.trim().is_empty() {
            // 丢弃整个元素
            out.push_str(&rest[..start]);
            rest = &rest[content_at + close + "</text>".len()..];
        } else {
            // 保留，从 "<text" 之后继续找下一个
            let keep = start + "<text".len();
            out.push_str(&rest[..keep]);
            rest = &rest[keep..];
        }
    }
    out.push_str(rest);
    out
}

/// 折叠标签之间的空白：`>` 之后的空白**只有紧接 `<` 时**才丢弃。
///
/// 无条件丢弃会把文本内容的前导空白也吃掉（`>  hello` → `>hello`）。
fn collapse_intertag_whitespace(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(i) = rest.find('>') {
        let after = i + 1;
        out.push_str(&rest[..after]);
        let tail = &rest[after..];
        let trimmed = tail.trim_start_matches(char::is_whitespace);
        if trimmed.starts_with('<') {
            rest = trimmed;
        } else {
            out.push_str(&rest[after..after + (tail.len() - trimmed.len())]);
            rest = trimmed;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::term::grid::{Attrs, Color};

    fn cell(col: usize, text: &str, fg: Color, bg: Color, bold: bool) -> Cell {
        Cell {
            col,
            text: text.to_string(),
            fg,
            bg,
            attrs: Attrs {
                bold,
                ..Default::default()
            },
            width: text.chars().count().max(1) as u8,
        }
    }

    fn render_one(line: Vec<Cell>, cols: usize) -> String {
        render(&[line], cols, 1)
    }

    #[test]
    fn basic_layout() {
        let svg = render_one(vec![cell(0, "hi", Color::Palette(2), Color::Default, false)], 4);
        assert!(svg.contains("<svg"));
        assert!(svg.contains(">hi<"));
        assert!(svg.contains(r#"width="32""#));
        assert!(svg.contains(r#"height="17""#));
    }

    #[test]
    fn merges_same_style_runs() {
        let svg = render_one(
            vec![
                cell(0, "a", Color::Palette(1), Color::Default, false),
                cell(1, "b", Color::Palette(1), Color::Default, false),
                cell(2, "c", Color::Palette(4), Color::Default, false),
            ],
            4,
        );
        assert!(svg.contains(">ab<"), "同色 run 应合并: {}", svg);
        assert!(svg.contains(">c<"));
    }

    /// 回归：压缩不得破坏非 ASCII（曾按字节重排，中文变成乱码）。
    #[test]
    fn compression_preserves_non_ascii() {
        let svg = render_one(
            vec![
                cell(0, "你", Color::Default, Color::Default, false),
                cell(2, "好", Color::Default, Color::Default, false),
                cell(4, "，", Color::Default, Color::Default, false),
                cell(6, "世界", Color::Default, Color::Default, false),
            ],
            12,
        );
        for level in [0, 1, 2] {
            let out = compress(&svg, level);
            assert!(out.contains("你好，世界"), "level={} 压缩后乱码: {}", level, out);
        }
    }

    #[test]
    fn compression_preserves_emoji_and_escapes() {
        let svg = render_one(vec![cell(0, "🚀<&>", Color::Default, Color::Default, false)], 8);
        let out = compress(&svg, 1);
        assert!(out.contains("🚀"), "emoji 丢失: {}", out);
        assert!(out.contains("&lt;&amp;&gt;"), "转义丢失: {}", out);
    }

    #[test]
    fn compression_removes_empty_text_only() {
        let svg = r#"<text x="0" y="0"></text><text x="8" y="0">a</text>"#;
        let out = compress(svg, 1);
        assert_eq!(out, r#"<text x="8" y="0">a</text>"#);
    }

    #[test]
    fn compression_keeps_leading_spaces_in_text() {
        let svg = r#"<text x="0" y="0">  hi</text>"#;
        assert_eq!(compress(svg, 1), svg);
    }

    #[test]
    fn compression_collapses_intertag_whitespace() {
        let svg = "<svg>\n  <rect/>\n</svg>";
        assert_eq!(compress(svg, 1), "<svg><rect/></svg>");
    }

    #[test]
    fn escape_rules() {
        assert_eq!(xml_escape("a<b&c>d"), "a&lt;b&amp;c&gt;d");
    }
}
