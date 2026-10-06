//! 渲染层：网格 → 字节。
//!
//! 只吃 [`crate::term::grid`] 的类型，不认识 pty、Python 与窗口系统。
//!
//! - [`ansi`] 文本输出：可见屏 ANSI、历史区、纯文本
//! - [`surface`] 增量输出：wezterm-surface 的单元格级 diff → ANSI 字节
//! - [`svg`] / [`pixmap`] 矢量与位图导出
//!
//! 网格常量是 SVG 与位图共用的像素级基准，两者必须一致。

pub mod ansi;
pub mod font;
pub mod pixmap;
pub mod surface;
pub mod svg;

use wezterm_term::screen::Screen;

use crate::term::grid::{cells_of_line, Cell};
use crate::term::View;

/// 单元格像素尺寸。
pub const CELL_W: usize = 8;
pub const CELL_H: usize = 17;

/// 无前景/背景色时的回退色，与 SVG 背景保持一致。
pub const DEFAULT_FG: (u8, u8, u8) = (0xe5, 0xe5, 0xe5);
pub const DEFAULT_BG: (u8, u8, u8) = (0x0c, 0x0c, 0x0c);

/// 光标定位序列：0-based 坐标 → 1-based CUP + 显示/隐藏。
pub fn cursor_seq(row: usize, col: usize, visible: bool) -> String {
    let mut s = format!("\x1b[{};{}H", row + 1, col + 1);
    s.push_str(if visible { "\x1b[?25h" } else { "\x1b[?25l" });
    s
}

/// 视口内的可见网格，每行一项。
///
/// 用借用式取行（`with_phys_lines`）而非 `lines_in_phys_range` —— 后者会深拷贝整行。
pub fn visible_cells(screen: &Screen, view: &View) -> Vec<Vec<Cell>> {
    let mut out = Vec::new();
    screen.with_phys_lines(view.window(screen), |lines| {
        out.extend(lines.iter().copied().map(cells_of_line));
    });
    out
}
