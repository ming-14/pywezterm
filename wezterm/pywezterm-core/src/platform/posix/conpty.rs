//! ConPTY 是 Windows 专有概念。
//!
//! 其他平台的 pty 由内核提供，没有「宿主实现可替换」这一层，因此这里恒为空 ——
//! 这是设计结论，不是待办占位。

use std::path::Path;

pub fn conpty_dir() -> Option<&'static Path> {
    None
}

pub fn activate() -> bool {
    false
}

pub fn active() -> bool {
    false
}
