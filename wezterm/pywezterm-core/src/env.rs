//! 扩展模块自身所在目录与随包资源。
//!
//! 资源路径只能由**模块自身的磁盘位置**推导：进程 CWD 是宿主的工程目录（与库无关），
//! Python 的 `__file__` 在模块初始化阶段尚未设置。见 [`platform::self_module_dir`]。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::platform;

/// 模块目录，解析一次后固定。
static MODULE_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 模块加载时调用一次：定位自身、启用随包资源。
///
/// 必须在 `#[pymodule]` 初始化里调用 —— 这样「资源是否可用」在模块加载那一刻就确定，
/// 而不是推迟到第一次建 pty（推迟会让行为依赖调用顺序）。
pub fn init() {
    let _ = module_dir();
    platform::conpty::activate();
}

/// 扩展模块所在目录；解析失败为 `None`。
pub fn module_dir() -> Option<&'static Path> {
    MODULE_DIR
        .get_or_init(platform::self_module_dir)
        .as_deref()
}

/// 随包资源的部署状态，供调用方自省。
#[derive(Debug, Clone)]
pub struct Info {
    /// 扩展模块所在目录
    pub module_dir: Option<PathBuf>,
    /// ConPTY 侧载目录（仅 Windows，且二进制齐备时才有值）
    pub conpty_dir: Option<PathBuf>,
    /// ConPTY 侧载是否已生效（`false` 表示回落到系统 conhost）
    pub conpty_active: bool,
}

/// 当前部署状态。
pub fn info() -> Info {
    Info {
        module_dir: module_dir().map(|p| p.to_path_buf()),
        conpty_dir: platform::conpty::conpty_dir().map(|p| p.to_path_buf()),
        conpty_active: platform::conpty::active(),
    }
}
