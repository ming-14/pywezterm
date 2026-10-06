//! 分隔线与状态栏。

use wezterm_char_props::widechar_width::WcLookupTable;

use crate::render::surface::Surface;
use crate::term::grid::{Attrs, Color};

/// 两窗格之间的竖线。
pub fn separator(surface: &mut Surface, col: usize, rows: usize) {
    for y in 0..rows {
        surface.put(col, y, "│", Color::Default, Color::Default, Attrs::default());
    }
}

/// 状态栏整行（默认样式）。
///
/// 必须按**显示宽度**而非字符数截断与补白：CJK/emoji 是双宽，按字符数补满会让写入
/// 位置超出表面宽度，触发 Surface 的滚动语义（终端行为），把整屏内容顶上去。
pub fn status(surface: &mut Surface, row: usize, cols: usize, text: &str) {
    let classifier = WcLookupTable::new();
    let mut line = String::with_capacity(cols);
    let mut width = 0usize;
    for ch in text.chars() {
        let w = classifier.classify(ch).width_unicode_9_or_later() as usize;
        if width + w > cols {
            break;
        }
        line.push(ch);
        width += w;
    }
    while width < cols {
        line.push(' ');
        width += 1;
    }
    surface.put(0, row, &line, Color::Default, Color::Default, Attrs::default());
}
