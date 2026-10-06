//! 取消 reader 线程的阻塞读。
//!
//! 其他平台不需要这一层：reader 阻塞在 `read(2)` 上，关闭 master 端即返回，不存在
//! Windows ConPTY 那种「关闭要等未完成的读」的互等。因此 [`cancel`](ReaderCancel::cancel)
//! 恒返回 `false`，调用方据此跳过重试等待。

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub struct ReaderCancel;

impl ReaderCancel {
    /// 其他平台无需登记，恒成功。
    pub fn register_current_thread() -> Option<Self> {
        Some(ReaderCancel)
    }

    /// 恒为 `false`：本平台关闭 master 端即可解除阻塞。
    pub fn cancel(&self) -> bool {
        false
    }
}

pub type Shared = Arc<Mutex<Option<ReaderCancel>>>;

pub fn new_shared() -> Shared {
    Arc::new(Mutex::new(None))
}

/// 其他平台无需等待 reader 退出：关闭 master 后 reader 自然返回。
pub fn cancel_and_wait(_shared: &Shared, _eof: &AtomicBool) {}
