//! 取消 reader 线程的阻塞读。
//!
//! ConPTY 的 `ClosePseudoConsole` 会等待未完成的 `ReadFile`，而 reader 线程正卡在
//! 那个 `ReadFile` 里 —— 不先取消读，两者互等。`GetCurrentThread` 返回的是伪句柄，
//! 跨线程无效，所以登记时必须复制一份。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use winapi::um::handleapi::{CloseHandle, DuplicateHandle};
use winapi::um::ioapiset::CancelSynchronousIo;
use winapi::um::processthreadsapi::{GetCurrentProcess, GetCurrentThread};
use winapi::um::winnt::{DUPLICATE_SAME_ACCESS, HANDLE};

/// 重试取消的间隔与次数：取消可能恰好落在两次 read 之间而落空。
const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);
const RETRY_ATTEMPTS: u32 = 200;

pub struct ReaderCancel {
    handle: HANDLE,
}

impl ReaderCancel {
    /// 在 reader 线程内调用，登记当前线程。失败返回 None（关闭时退化为不做取消）。
    pub fn register_current_thread() -> Option<Self> {
        unsafe {
            let mut h: HANDLE = std::ptr::null_mut();
            let ok = DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentThread(),
                GetCurrentProcess(),
                &mut h,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            );
            if ok == 0 {
                None
            } else {
                Some(Self { handle: h })
            }
        }
    }

    /// 取消该线程的同步阻塞 IO。
    pub fn cancel(&self) -> bool {
        unsafe { CancelSynchronousIo(self.handle) != 0 }
    }

    /// 反复取消直到 `done()` 为真或重试耗尽。
    ///
    /// 单次 `CancelSynchronousIo` 可能恰好落在两次 read 之间而落空，因此这里重试；
    /// `done` 由调用方传入（通常是「reader 已置 EOF 标志」）。
    pub fn cancel_until(&self, done: &dyn Fn() -> bool) {
        for _ in 0..RETRY_ATTEMPTS {
            if done() {
                return;
            }
            self.cancel();
            if done() {
                return;
            }
            std::thread::sleep(RETRY_INTERVAL);
        }
    }
}

impl Drop for ReaderCancel {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

// 线程句柄是进程级资源，不绑定创建它的线程；这里只做取消与关闭，且总是经由共享锁访问。
unsafe impl Send for ReaderCancel {}
unsafe impl Sync for ReaderCancel {}

/// reader 线程持有的取消句柄：线程启动时登记，关闭方在 `close()` 里取用。
pub type Shared = Arc<std::sync::Mutex<Option<ReaderCancel>>>;

pub fn new_shared() -> Shared {
    Arc::new(std::sync::Mutex::new(None))
}

/// 关闭方调用：取消阻塞读并等待 reader 退出（`eof` 为 reader 的完成标志）。
pub fn cancel_and_wait(shared: &Shared, eof: &AtomicBool) {
    let guard = shared.lock().unwrap();
    if let Some(rc) = guard.as_ref() {
        rc.cancel_until(&|| eof.load(Ordering::SeqCst));
    }
}
