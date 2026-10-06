//! 视口：可见窗口在「scrollback + 可见区」中的位置。
//!
//! 偏移为 0 表示跟随最新输出。所有「可见区物理行区间」的计算都收在这里 ——
//! 此前每个读取接口都自己算一遍 `total - offset` / `end - rows`。

use std::ops::Range;

use wezterm_term::screen::Screen;

/// 视口滚动状态。
#[derive(Clone, Copy, Default, Debug)]
pub struct View {
    offset: usize,
}

/// 可用历史行数上限（偏移不能越过它，否则视野会离开可见区）。
pub fn max_offset(screen: &Screen) -> usize {
    screen.scrollback_rows().saturating_sub(screen.physical_rows)
}

/// 历史区（可见区之上的全部行）的物理行区间。
pub fn history(screen: &Screen) -> Range<usize> {
    0..max_offset(screen)
}

impl View {
    /// 原始偏移（未 clamp）。
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// 上滚 `delta > 0` 查看更早内容，`delta < 0` 回落。
    pub fn scroll(&mut self, delta: i64, screen: &Screen) {
        let max = max_offset(screen);
        self.offset = (self.offset as i64 + delta).clamp(0, max as i64) as usize;
    }

    /// 回到底部，恢复跟随最新输出。
    pub fn to_bottom(&mut self) {
        self.offset = 0;
    }

    /// 直接设定偏移（resize 后按稳定行恢复滚动位置用）。读取时会再 clamp。
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
    }

    /// 实际生效的偏移。
    pub fn effective(&self, screen: &Screen) -> usize {
        self.offset.min(max_offset(screen))
    }

    /// 可见区在物理行索引中的半开区间。
    pub fn window(&self, screen: &Screen) -> Range<usize> {
        let end = screen.scrollback_rows().saturating_sub(self.effective(screen));
        end.saturating_sub(screen.physical_rows)..end
    }

    /// 模型行 `y` 映射到视口行；滚出可见区则为 `None`。
    pub fn row_in_view(&self, screen: &Screen, y: i64) -> Option<usize> {
        let row = y.max(0) as usize + self.effective(screen);
        (row < screen.physical_rows).then_some(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::term::Model;

    fn screen_of(model: &Model) -> &Screen {
        model.screen()
    }

    #[test]
    fn window_follows_bottom_by_default() {
        let model = Model::new(80, 4, 100);
        let view = View::default();
        let s = screen_of(&model);
        let w = view.window(s);
        assert_eq!(w.end - w.start, 4);
        assert_eq!(w.end, s.scrollback_rows());
    }

    #[test]
    fn scroll_clamps_to_history() {
        let mut model = Model::new(80, 4, 100);
        // 造 20 行输出，使 scrollback 非空
        for i in 0..20 {
            model.feed(format!("line {i}\r\n").as_bytes());
        }
        let mut view = View::default();
        let max = max_offset(model.screen());
        assert!(max > 0, "应已产生 scrollback");

        view.scroll(5, model.screen());
        assert_eq!(view.offset(), 5);
        assert_eq!(view.effective(model.screen()), 5);

        view.scroll(10_000, model.screen());
        assert_eq!(view.effective(model.screen()), max, "不得超过可用历史");

        view.scroll(-10_000, model.screen());
        assert_eq!(view.offset(), 0);
    }

    #[test]
    fn row_in_view_hides_scrolled_out_cursor() {
        let mut model = Model::new(80, 4, 100);
        for i in 0..20 {
            model.feed(format!("line {i}\r\n").as_bytes());
        }
        let mut view = View::default();
        assert!(view.row_in_view(model.screen(), 0).is_some());
        view.scroll(10, model.screen());
        assert!(view.row_in_view(model.screen(), 0).is_none());
    }
}
