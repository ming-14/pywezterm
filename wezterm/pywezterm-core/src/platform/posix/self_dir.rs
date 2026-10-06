//! 扩展模块自身所在目录。
//!
//! 用 `dladdr` 问动态链接器：本函数地址属于哪个 `.so`。这是唯一与「谁 import 了我」
//! 「进程 CWD 是什么」都无关的来源。

use std::ffi::{CStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

pub fn self_module_dir() -> Option<PathBuf> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    let addr = self_module_dir as *const libc::c_void;
    if unsafe { libc::dladdr(addr, &mut info) } == 0 || info.dli_fname.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes().to_vec();
    PathBuf::from(OsString::from_vec(bytes))
        .parent()
        .map(|p| p.to_path_buf())
}
