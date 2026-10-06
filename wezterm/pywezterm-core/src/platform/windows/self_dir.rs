//! 扩展模块自身所在目录。
//!
//! 取当前 `.pyd` 的路径：这是唯一与「谁 import 了我」「进程 CWD 是什么」都无关的
//! 来源。随包资源（conpty.dll / OpenConsole.exe）都相对它定位。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use winapi::shared::minwindef::{DWORD, HMODULE};
use winapi::um::libloaderapi::{GetModuleFileNameW, GetModuleHandleExW};

/// `GetModuleHandleExW` 标志：按地址反查所属模块，且不增加引用计数。
const FROM_ADDRESS: DWORD = 0x0000_0004;
const UNCHANGED_REFCOUNT: DWORD = 0x0000_0002;

pub fn self_module_dir() -> Option<PathBuf> {
    unsafe {
        let mut hmod: HMODULE = std::ptr::null_mut();
        let addr = self_module_dir as *const ();
        if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, addr as *const _, &mut hmod) == 0 {
            return None;
        }
        // 路径可能超过 MAX_PATH，一次给足 32K
        let mut buf = vec![0u16; 32 * 1024];
        let n = GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as DWORD);
        if n == 0 {
            return None;
        }
        buf.truncate(n as usize);
        PathBuf::from(OsString::from_wide(&buf))
            .parent()
            .map(|p| p.to_path_buf())
    }
}
