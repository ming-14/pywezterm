//! 窗格表：id 是身份，不是下标。
//!
//! 用位置当 id 时，关掉 0 号会让 1 号变成 0 号 —— 宿主缓存的 id 会打到别人身上。
//! 这里 id 单调递增、永不复用，关闭后查询得到明确的「不存在」。

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Error, Result};

use super::Pane;

/// 窗格身份。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct PaneId(u64);

impl PaneId {
    /// 由整数构造。合法性由使用方校验 —— 不存在的 id 会被 [`Registry::get`] 拒绝。
    pub fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// 窗格表。插入序即 id 升序。
#[derive(Default)]
pub struct Registry {
    next: u64,
    panes: HashMap<PaneId, Arc<Pane>>,
}

impl Registry {
    pub fn insert(&mut self, pane: Arc<Pane>) -> PaneId {
        let id = PaneId(self.next);
        self.next += 1;
        self.panes.insert(id, pane);
        id
    }

    pub fn get(&self, id: PaneId) -> Result<&Arc<Pane>> {
        self.panes.get(&id).ok_or(Error::NoSuchPane(id.raw()))
    }

    /// 移除并返回窗格；不存在时报错。
    pub fn remove(&mut self, id: PaneId) -> Result<Arc<Pane>> {
        self.panes.remove(&id).ok_or(Error::NoSuchPane(id.raw()))
    }

    pub fn contains(&self, id: PaneId) -> bool {
        self.panes.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.panes.len()
    }

    /// 清空全部窗格。**保留 id 计数器** —— 重置它会让新窗格复用旧 id，
    /// 宿主缓存的旧 id 就会指向另一个窗格。
    pub fn clear(&mut self) {
        self.panes.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// 按插入序（即 id 升序）返回所有 id。
    pub fn ids(&self) -> Vec<PaneId> {
        let mut ids: Vec<PaneId> = self.panes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PaneId, &Arc<Pane>)> {
        self.panes.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Arc<Pane> {
        Arc::new(Pane::detached(20, 4, 10))
    }

    #[test]
    fn first_id_is_zero() {
        let mut reg = Registry::default();
        assert_eq!(reg.insert(pane()).raw(), 0);
        assert_eq!(reg.insert(pane()).raw(), 1);
    }

    #[test]
    fn ids_are_stable_across_removal() {
        let mut reg = Registry::default();
        let a = reg.insert(pane());
        let b = reg.insert(pane());

        reg.remove(a).unwrap();
        // 关键：b 的身份不因 a 被移除而改变
        assert!(!reg.contains(a));
        assert!(reg.contains(b));
        assert!(matches!(reg.get(a), Err(Error::NoSuchPane(_))));
        assert!(reg.get(b).is_ok());
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut reg = Registry::default();
        let a = reg.insert(pane());
        reg.remove(a).unwrap();
        let b = reg.insert(pane());
        assert_ne!(a, b, "id 复用会让旧句柄打到新窗格上");
    }

    #[test]
    fn ids_are_in_insertion_order() {
        let mut reg = Registry::default();
        let a = reg.insert(pane());
        let b = reg.insert(pane());
        let c = reg.insert(pane());
        assert_eq!(reg.ids(), vec![a, b, c]);
        reg.remove(b).unwrap();
        assert_eq!(reg.ids(), vec![a, c]);
    }

    #[test]
    fn removing_twice_is_an_error() {
        let mut reg = Registry::default();
        let a = reg.insert(pane());
        assert!(reg.remove(a).is_ok());
        assert!(reg.remove(a).is_err());
    }

    #[test]
    fn clear_does_not_recycle_ids() {
        let mut reg = Registry::default();
        reg.insert(pane());
        reg.insert(pane());
        reg.clear();
        assert!(reg.is_empty());
        // 清空后新窗格不得复用 0 / 1
        assert_eq!(reg.insert(pane()).raw(), 2);
    }
}
