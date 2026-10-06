//! 复用器：多个窗格的布局、焦点与帧合成。
//!
//! `Mux` 只负责编排 —— 窗格本体是 [`crate::host::Pane`]，布局在 [`layout`]，
//! 分隔线与状态栏在 [`chrome`]，合成在 [`compose`]。
//!
//! 所有改动布局的操作都经由 [`State::relayout`] 收口：矩形重算与「合成基线失效」
//! 必须同时发生，否则会留下用旧矩形画的残留内容。

pub mod chrome;
mod compose;
pub mod layout;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use wezterm_term::Clipboard;

use crate::error::{Error, Result};
use crate::host::{Driver, OutputNotifier, Pane, PaneId, Registry};
use crate::render::surface::Surface;

use layout::Rect;

/// 每个窗格的合成基线。
#[derive(Clone, Copy, Default)]
struct Mark {
    /// 上次合成时的终端序号；`None` = 首帧，整窗格重写
    seqno: Option<usize>,
    /// 上次合成时的视口偏移；变化说明视图平移，整窗格重写
    view: usize,
}

/// 单帧合成结果。
pub struct Frame {
    /// 增量 ANSI 字节（内部 CUP 为 1-based 终端语义）
    pub bytes: Vec<u8>,
    /// 焦点光标整屏坐标，**0-based**；滚出可见区时 [`Frame::cursor_visible`] 为假
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_visible: bool,
}

struct State {
    cols: usize,
    rows: usize,
    registry: Registry,
    /// 布局顺序（插入序）
    order: Vec<PaneId>,
    focused: Option<PaneId>,
    rects: Vec<(PaneId, Rect)>,
    split: Option<usize>,
    sep: bool,
    status_rows: usize,
    status: String,
    surface: Surface,
    /// 表面合成基线
    seqno: usize,
    /// 上次画过的分隔线（列, 高）；相同则跳过
    last_sep: Option<(usize, usize)>,
    last_status: String,
    last_status_rows: usize,
    marks: HashMap<PaneId, Mark>,
    /// 焦点窗格的剪贴板回调（OSC 52）；新窗格自动继承
    clipboard: Option<Arc<dyn Clipboard>>,
    /// 输出通知；新窗格自动继承
    notifier: Option<Arc<dyn OutputNotifier>>,
}

impl State {
    fn pane(&self, id: PaneId) -> Result<&Arc<Pane>> {
        self.registry.get(id)
    }

    fn rect_of(&self, id: PaneId) -> Option<Rect> {
        self.rects
            .iter()
            .find(|(pid, _)| *pid == id)
            .map(|(_, r)| *r)
    }

    fn mark(&self, id: PaneId) -> Mark {
        self.marks.get(&id).copied().unwrap_or_default()
    }

    fn set_mark(&mut self, id: PaneId, seqno: usize, view: usize) {
        self.marks.insert(
            id,
            Mark {
                seqno: Some(seqno),
                view,
            },
        );
    }

    /// 重算布局并使合成基线失效。**所有改动布局的操作都必须经过这里。**
    fn relayout(&mut self) {
        let avail = self.rows.saturating_sub(self.status_rows).max(1);
        self.rects = self
            .order
            .iter()
            .copied()
            .zip(layout::rects(
                self.order.len(),
                self.cols,
                avail,
                self.sep,
                self.split,
            ))
            .collect();
        self.last_sep = None;
        self.last_status = String::new();
        self.marks.clear();
    }

    /// 把窗格调整到它当前的矩形尺寸。
    fn fit_pane(&self, id: PaneId) -> Result<()> {
        let Some(rect) = self.rect_of(id) else {
            return Ok(());
        };
        self.pane(id)?.resize(rect.w.max(1), rect.h.max(1))
    }

    /// 焦点窗格的光标，换算到整屏坐标。
    fn focus_cursor(&self) -> (usize, usize, bool) {
        let Some(id) = self.focused else {
            return (0, 0, false);
        };
        let (Some(rect), Ok(pane)) = (self.rect_of(id), self.pane(id)) else {
            return (0, 0, false);
        };
        let (col, row, visible) = pane.cursor();
        let last_col = rect.x + rect.w.saturating_sub(1);
        (rect.y + row, (rect.x + col).min(last_col), visible)
    }
}

/// 多窗格复用器。
pub struct Mux {
    inner: Mutex<State>,
}

impl Mux {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Self {
            inner: Mutex::new(State {
                cols,
                rows,
                registry: Registry::default(),
                order: Vec::new(),
                focused: None,
                rects: Vec::new(),
                split: None,
                sep: false,
                status_rows: 0,
                status: String::new(),
                surface: Surface::new(cols, rows),
                seqno: 0,
                last_sep: None,
                last_status: String::new(),
                last_status_rows: 0,
                marks: HashMap::new(),
                clipboard: None,
                notifier: None,
            }),
        }
    }

    // ---- 窗格 ------------------------------------------------------------

    /// 新建窗格并启动子进程。
    pub fn add_pane(
        &self,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
    ) -> Result<PaneId> {
        let mut st = self.inner.lock().unwrap();
        let rect = st
            .rect_for_new_pane()
            .ok_or_else(|| Error::Platform("无法计算窗格矩形".into()))?;

        let pane = Pane::open_pty(
            rect.w.max(1),
            rect.h.max(1),
            crate::host::pane::DEFAULT_SCROLLBACK,
            Driver::Auto,
        )?;
        if let Some(cb) = &st.clipboard {
            pane.set_clipboard(cb.clone());
        }
        if let Some(n) = &st.notifier {
            pane.set_notifier(n.clone());
        }
        // 先建后改：spawn 失败时布局与焦点都没被动过
        pane.spawn(argv, cwd, env, None, None)?;

        let id = st.registry.insert(Arc::new(pane));
        st.order.push(id);
        st.focused = Some(id);
        st.relayout();
        Ok(id)
    }

    /// 关闭一个窗格。**幂等**：不存在的 id 静默忽略。
    pub fn close_pane(&self, id: PaneId) -> Result<()> {
        let pane = {
            let mut st = self.inner.lock().unwrap();
            match st.registry.remove(id) {
                Ok(pane) => {
                    st.order.retain(|pid| *pid != id);
                    if st.focused == Some(id) {
                        st.focused = st.order.last().copied();
                    }
                    st.relayout();
                    pane
                }
                Err(Error::NoSuchPane(_)) => return Ok(()),
                Err(e) => return Err(e),
            }
        };
        pane.close();
        Ok(())
    }

    /// 关闭全部窗格。
    pub fn close(&self) {
        let panes: Vec<Arc<Pane>> = {
            let mut st = self.inner.lock().unwrap();
            let ids = st.order.clone();
            let panes: Vec<Arc<Pane>> = ids
                .iter()
                .filter_map(|id| st.registry.get(*id).ok().cloned())
                .collect();
            st.order.clear();
            st.registry.clear();
            st.focused = None;
            st.relayout();
            panes
        };
        for pane in panes {
            pane.close();
        }
    }

    pub fn pane_count(&self) -> usize {
        self.inner.lock().unwrap().order.len()
    }

    pub fn dimensions(&self) -> (usize, usize) {
        let st = self.inner.lock().unwrap();
        (st.cols, st.rows)
    }

    /// 各窗格矩形，顺序与插入序一致。
    pub fn pane_rects(&self) -> Vec<Rect> {
        let st = self.inner.lock().unwrap();
        st.order.iter().filter_map(|id| st.rect_of(*id)).collect()
    }

    pub fn focused(&self) -> Option<PaneId> {
        self.inner.lock().unwrap().focused
    }

    pub fn set_focus(&self, id: PaneId) -> Result<()> {
        let mut st = self.inner.lock().unwrap();
        st.registry.get(id)?;
        st.focused = Some(id);
        Ok(())
    }

    /// 命中测试：整屏坐标落在哪个窗格。分隔线、状态栏、屏幕外返回 `None`。
    pub fn pane_at(&self, x: usize, y: usize) -> Option<PaneId> {
        let st = self.inner.lock().unwrap();
        layout::hit(&st.rects, x, y)
    }

    /// 取窗格（绑定层直接转发模型面调用）。
    pub fn pane(&self, id: PaneId) -> Result<Arc<Pane>> {
        self.inner.lock().unwrap().registry.get(id).cloned()
    }

    // ---- 布局 ------------------------------------------------------------

    pub fn set_sep(&self, sep: bool) {
        let mut st = self.inner.lock().unwrap();
        st.sep = sep;
        st.relayout();
    }

    pub fn set_split_col(&self, split: Option<usize>) {
        let mut st = self.inner.lock().unwrap();
        st.split = split;
        st.relayout();
    }

    pub fn set_status_rows(&self, rows: usize) {
        let mut st = self.inner.lock().unwrap();
        st.status_rows = rows;
        st.relayout();
    }

    pub fn set_status(&self, text: String) {
        self.inner.lock().unwrap().status = text;
    }

    /// 宿主屏尺寸变化：重算矩形，并把各窗格的 pty 与模型调到新矩形。
    pub fn resize(&self, cols: usize, rows: usize) {
        let mut st = self.inner.lock().unwrap();
        st.cols = cols.max(1);
        st.rows = rows.max(1);
        let (cols, rows) = (st.cols, st.rows);
        st.surface.resize(cols, rows);
        st.seqno = 0;
        st.relayout();
        for id in st.order.clone() {
            let _ = st.fit_pane(id);
        }
        st.marks.clear();
    }

    /// 强制下一帧全量重绘。
    pub fn force_repaint(&self) {
        let mut st = self.inner.lock().unwrap();
        st.seqno = 0;
        st.last_sep = None;
        st.last_status = String::new();
    }

    // ---- 合成 ------------------------------------------------------------

    /// 合成一帧。
    pub fn render(&self) -> Result<Frame> {
        let mut st = self.inner.lock().unwrap();
        let (cols, rows) = (st.cols, st.rows);
        if st.surface.dimensions() != (cols, rows) {
            st.surface.resize(cols, rows);
            st.seqno = 0;
        }

        for (id, rect) in st.rects.clone() {
            compose::pane(&mut st, id, rect);
        }

        // 分隔线：左窗格宽度即分隔线所在列
        if st.sep && st.order.len() == 2 {
            if let Some((_, left)) = st.rects.first().copied() {
                if st.last_sep != Some((left.w, left.h)) {
                    chrome::separator(&mut st.surface, left.w, left.h);
                    st.last_sep = Some((left.w, left.h));
                }
            }
        } else {
            st.last_sep = None;
        }

        // 状态栏：仅在文本或行数变化时重画
        if st.status_rows > 0 {
            let row = st.rows.saturating_sub(st.status_rows);
            if st.last_status != st.status || st.last_status_rows != st.status_rows {
                let (cols, status) = (st.cols, st.status.clone());
                chrome::status(&mut st.surface, row, cols, &status);
                st.last_status = status;
                st.last_status_rows = st.status_rows;
            }
        } else {
            st.last_status_rows = 0;
        }

        let (row, col, visible) = st.focus_cursor();
        let baseline = st.seqno;
        let (seqno, bytes) = st.surface.changes_bytes(baseline)?;
        st.seqno = seqno;
        Ok(Frame {
            bytes,
            cursor_row: row,
            cursor_col: col,
            cursor_visible: visible,
        })
    }

    // ---- 回调 ------------------------------------------------------------

    /// 设置剪贴板回调（OSC 52）。已存在的窗格立即生效，新窗格自动继承。
    pub fn set_clipboard(&self, cb: Arc<dyn Clipboard>) {
        let mut st = self.inner.lock().unwrap();
        let panes: Vec<Arc<Pane>> = st.registry.iter().map(|(_, p)| p.clone()).collect();
        for pane in &panes {
            pane.set_clipboard(cb.clone());
        }
        st.clipboard = Some(cb);
    }

    /// 设置输出通知。已存在的窗格立即生效，新窗格自动继承。
    pub fn set_notifier(&self, notifier: Arc<dyn OutputNotifier>) {
        let mut st = self.inner.lock().unwrap();
        let panes: Vec<Arc<Pane>> = st.registry.iter().map(|(_, p)| p.clone()).collect();
        for pane in &panes {
            pane.set_notifier(notifier.clone());
        }
        st.notifier = Some(notifier);
    }

    /// 取消输出通知。
    pub fn clear_notifier(&self) {
        let mut st = self.inner.lock().unwrap();
        let panes: Vec<Arc<Pane>> = st.registry.iter().map(|(_, p)| p.clone()).collect();
        for pane in &panes {
            pane.clear_notifier();
        }
        st.notifier = None;
    }

    // ---- 输入路由 --------------------------------------------------------

    /// 滚动焦点窗格的视口。
    pub fn scroll(&self, delta: i64) -> Result<()> {
        self.focused_pane()?.scroll(delta);
        Ok(())
    }

    /// 焦点窗格回到底部。
    pub fn scroll_to_bottom(&self) -> Result<()> {
        self.focused_pane()?.scroll_to_bottom();
        Ok(())
    }

    fn focused_pane(&self) -> Result<Arc<Pane>> {
        let id = self
            .inner
            .lock()
            .unwrap()
            .focused
            .ok_or_else(|| Error::Platform("没有焦点窗格".into()))?;
        self.pane(id)
    }

    /// 整屏坐标 → `(窗格, 窗格内列, 窗格内行)`。
    pub fn to_pane_coords(&self, x: usize, y: i64) -> Result<(PaneId, usize, i64)> {
        let st = self.inner.lock().unwrap();
        let id = layout::hit(&st.rects, x, y.max(0) as usize)
            .ok_or_else(|| Error::Invalid(format!("坐标 ({x},{y}) 未命中任何窗格")))?;
        let rect = st.rect_of(id).unwrap_or_default();
        Ok((id, x.saturating_sub(rect.x), (y - rect.y as i64).max(0)))
    }

    /// 整屏坐标 → 该窗格内的 stable 行 + 列。
    pub fn to_stable(&self, id: PaneId, x: usize, y: i64) -> Result<(isize, usize)> {
        let (lx, ly) = {
            let st = self.inner.lock().unwrap();
            let rect = st.rect_of(id).ok_or(Error::NoSuchPane(id.raw()))?;
            (x.saturating_sub(rect.x), (y - rect.y as i64).max(0) as usize)
        };
        Ok(self.pane(id)?.screen_to_stable(lx, ly))
    }
}

impl State {
    /// 假想新增一个窗格后的矩形。
    fn rect_for_new_pane(&self) -> Option<Rect> {
        let avail = self.rows.saturating_sub(self.status_rows).max(1);
        layout::rects(self.order.len() + 1, self.cols, avail, self.sep, self.split)
            .last()
            .copied()
    }
}
