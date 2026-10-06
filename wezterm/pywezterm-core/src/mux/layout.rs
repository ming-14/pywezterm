//! 布局：把可用矩形切给各窗格。
//!
//! 目前只有两种形态（单窗格、左右二分）。刻意表达成数据而不是散在编排代码里的 `if`：
//! 扩展成 N 叉只需要改本模块，编排层不用动。

use crate::host::PaneId;

/// 窗格矩形（列、行、宽、高）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// 布局形态。目前只有两种；[`rects`] 按窗格数选择。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    Single,
    SplitLR,
}

impl Layout {
    fn of(pane_count: usize) -> Self {
        if pane_count >= 2 {
            Layout::SplitLR
        } else {
            Layout::Single
        }
    }
}

/// 按窗格数计算矩形，顺序与窗格顺序一致。
///
/// `sep` 为真且是左右二分时，两窗格之间预留一列分隔线（左 `[0, split)`、分隔线
/// `x = split`、右 `[split + 1, cols)`）。`split` 为 `None` 时取中点。
pub fn rects(
    pane_count: usize,
    cols: usize,
    avail_rows: usize,
    sep: bool,
    split: Option<usize>,
) -> Vec<Rect> {
    let (cols, avail_rows) = (cols.max(1), avail_rows.max(1));
    match Layout::of(pane_count) {
        _ if pane_count == 0 => Vec::new(),
        Layout::Single => vec![Rect {
            x: 0,
            y: 0,
            w: cols,
            h: avail_rows,
        }],
        Layout::SplitLR => {
            let (left, right_x, right_w) = if sep {
                // 分隔线占一列，两侧各留至少一列
                let split = split.unwrap_or(cols / 2).clamp(1, cols.saturating_sub(2).max(1));
                (split, split + 1, cols.saturating_sub(split + 1).max(1))
            } else {
                let split = cols / 2;
                (split, split, cols.saturating_sub(split).max(1))
            };
            (0..pane_count)
                .map(|i| {
                    if i == 0 {
                        Rect {
                            x: 0,
                            y: 0,
                            w: left.max(1),
                            h: avail_rows,
                        }
                    } else {
                        Rect {
                            x: right_x,
                            y: 0,
                            w: right_w,
                            h: avail_rows,
                        }
                    }
                })
                .collect()
        }
    }
}

/// 命中测试：整屏坐标落在哪个窗格；分隔线、状态栏、屏幕外都返回 `None`。
pub fn hit(rects: &[(PaneId, Rect)], x: usize, y: usize) -> Option<PaneId> {
    rects
        .iter()
        .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_panes_yields_no_rects() {
        assert!(rects(0, 80, 24, false, None).is_empty());
    }

    #[test]
    fn single_pane_fills_the_screen() {
        let r = rects(1, 80, 24, true, None);
        assert_eq!(
            r,
            vec![Rect {
                x: 0,
                y: 0,
                w: 80,
                h: 24
            }]
        );
    }

    #[test]
    fn split_without_separator_is_contiguous() {
        let r = rects(2, 80, 24, false, None);
        assert_eq!(r[0].w, 40);
        assert_eq!(r[1].x, 40);
        assert_eq!(r[1].w, 40);
        assert_eq!(r[0].w + r[1].w, 80, "两窗格应恰好铺满，没有缝隙");
    }

    #[test]
    fn split_with_separator_leaves_exactly_one_column() {
        let r = rects(2, 80, 24, true, None);
        assert_eq!(r[0].w, 40);
        assert_eq!(r[1].x, 41, "分隔线占第 40 列");
        assert_eq!(r[1].w, 39);
        assert_eq!(r[0].w + 1 + r[1].w, 80);
    }

    #[test]
    fn split_column_is_clamped_to_leave_room_for_both_sides() {
        for want in [0usize, 1, 78, 79, 999] {
            let r = rects(2, 80, 24, true, Some(want));
            assert!(r[0].w >= 1, "左侧至少要有一列: {}", want);
            assert!(r[1].w >= 1, "右侧至少要有一列: {}", want);
            assert!(r[1].x + r[1].w <= 80, "右侧不得越界: {}", want);
        }
    }

    #[test]
    fn tiny_screens_still_produce_valid_rects() {
        for cols in [1usize, 2, 3] {
            for rect in rects(2, cols, 1, true, None) {
                assert!(rect.w >= 1 && rect.h >= 1, "cols={} 产生了零尺寸矩形", cols);
            }
        }
    }

    #[test]
    fn hit_skips_separator_and_status_rows() {
        let ids = [PaneId::new(7), PaneId::new(9)];
        let paired: Vec<(PaneId, Rect)> = ids
            .iter()
            .copied()
            .zip(rects(2, 80, 20, true, None))
            .collect();
        assert_eq!(hit(&paired, 0, 0), Some(ids[0]));
        assert_eq!(hit(&paired, 79, 19), Some(ids[1]));
        assert_eq!(hit(&paired, 40, 0), None, "分隔线不属于任何窗格");
        assert_eq!(hit(&paired, 0, 20), None, "状态栏行不属于任何窗格");
    }
}
