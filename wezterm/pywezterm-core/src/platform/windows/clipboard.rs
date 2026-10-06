//! 宿主剪贴板读写（Win32，UTF-16）。
//!
//! 两类失败分开对待：**能力缺失**由平台层报错（非 Windows 实现返回
//! [`Error::Platform`](crate::Error::Platform)）；**瞬时失败**（剪贴板被别的进程占着、
//! 分配失败）按尽力而为忽略 —— 剪贴板不在关键路径上，为它中断调用方不值得。

use crate::error::Result;

use winapi::um::winbase::{GlobalAlloc, GlobalFree, GlobalLock, GlobalUnlock, GlobalSize, GMEM_MOVEABLE};
use winapi::um::winuser::{
    CF_UNICODETEXT, CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
    SetClipboardData,
};

/// 读纯文本；剪贴板被占用或无文本内容时返回空串。
pub fn read() -> Result<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Ok(String::new());
        }
        let out = read_locked();
        CloseClipboard();
        out
    }
}

unsafe fn read_locked() -> Result<String> {
    let h = GetClipboardData(CF_UNICODETEXT);
    if h.is_null() {
        return Ok(String::new());
    }
    // 用 GlobalSize 定界：剪贴板内容来自任意进程，不能假定它在 1 MiB 内一定 NUL 终止
    let units = std::slice::from_raw_parts(GlobalLock(h) as *const u16, GlobalSize(h) / 2);
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]);
    GlobalUnlock(h);
    Ok(text)
}

/// 写纯文本；瞬时失败静默忽略。
pub fn write(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut data: Vec<u16> = text.encode_utf16().collect();
    data.push(0);
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Ok(());
        }
        let out = write_locked(&data);
        CloseClipboard();
        out
    }
}

unsafe fn write_locked(data: &[u16]) -> Result<()> {
    // 先分配再清空：分配失败时不该把别的进程留下的剪贴板内容清掉
    let h = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
    if h.is_null() {
        return Ok(());
    }
    let p = GlobalLock(h);
    if p.is_null() {
        GlobalFree(h);
        return Ok(());
    }
    std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u16, data.len());
    GlobalUnlock(h);

    EmptyClipboard();
    if SetClipboardData(CF_UNICODETEXT, h).is_null() {
        // 交接失败时所有权仍在自己手上
        GlobalFree(h);
    }
    Ok(())
}
