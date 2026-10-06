//! 帧合成：把各窗格的可见内容写进整屏表面。
//!
//! 增量策略：只重写「确实变化」的行。整帧全写会让 `wezterm-surface` 每帧全量重绘
//! （`Line::set_cell` 无条件推进 `last_change_seqno`），增量的意义就没了。
//!
//! 每个窗格在自己的模型锁内完成合成，因此**单个窗格的内容一定是同一时刻的**；
//! 跨窗格不追求同一时刻（那需要一把全局锁，与按窗格加锁的设计冲突，代价远大于收益）。

use crate::host::PaneId;
use crate::render::surface::Surface;
use crate::term::grid::{Attrs, Cell, Color};

use super::layout::Rect;
use super::State;

/// 一个连续同风格的文本段。
struct Run {
    start: usize,
    end: usize,
    text: String,
    fg: Color,
    bg: Color,
    attrs: Attrs,
}

/// 把一行单元格写进表面。
///
/// 合并相邻同风格格为一段文本，并在内容间隙与行尾补默认空格清掉过期内容；
/// 宽字符按 `width` 占列，续列不单独发。
fn emit_row(surface: &mut Surface, x0: usize, y: usize, rw: usize, cells: &[Cell]) {
    let blank = || (Color::Default, Color::Default, Attrs::default());
    let mut items: Vec<(usize, String, Color, Color, Attrs, usize)> = Vec::new();
    let mut prev_end = 0usize;
    for cell in cells {
        let (col, width) = (cell.col, cell.width.max(1) as usize);
        if col >= rw {
            break;
        }
        if col > prev_end {
            let gap = col - prev_end;
            let (fg, bg, attrs) = blank();
            items.push((prev_end, " ".repeat(gap), fg, bg, attrs, gap));
        }
        items.push((col, cell.text.clone(), cell.fg, cell.bg, cell.attrs, width));
        prev_end = col + width;
    }
    if prev_end < rw {
        let gap = rw - prev_end;
        let (fg, bg, attrs) = blank();
        items.push((prev_end, " ".repeat(gap), fg, bg, attrs, gap));
    }

    let mut runs: Vec<Run> = Vec::new();
    for (col, text, fg, bg, attrs, width) in items {
        if let Some(last) = runs.last_mut() {
            if last.end == col && last.fg == fg && last.bg == bg && last.attrs == attrs {
                last.text.push_str(&text);
                last.end = col + width;
                continue;
            }
        }
        runs.push(Run {
            start: col,
            end: col + width,
            text,
            fg,
            bg,
            attrs,
        });
    }
    for run in runs {
        surface.put(x0 + run.start, y, &run.text, run.fg, run.bg, run.attrs);
    }
}

/// 合成一个窗格到它当前的矩形区域。
pub fn pane(st: &mut State, id: PaneId, rect: Rect) {
    let Ok(pane) = st.pane(id).cloned() else {
        return;
    };
    let mark = st.mark(id);
    let (rw, rh) = (rect.w.max(1), rect.h.max(1));

    let (seqno, offset) = pane.for_each_dirty_row(mark.seqno, mark.view, |li, cells| {
        if li < rh {
            emit_row(&mut st.surface, rect.x, rect.y + li, rw, cells);
        }
    });
    st.set_mark(id, seqno, offset);
}
