//! ConPTY 侧载宿主（wezterm 自带 OpenConsole）的定位与启用。
//!
//! 侧载二进制与 `.pyd` 同目录（打包时由 maturin 的 `include` 放进去）。找不到就
//! 回落系统 conhost —— 回落是**可观测**的：见 [`conpty_dir`] 与 [`active`]。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::env;

/// 侧载目录（含 conpty.dll + OpenConsole.exe 的那一层）。
static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 侧载二进制文件名；两个都必须在同一目录才启用。
///
/// 与 `pywezterm/build.rs` 的 `SIDECAR`、`pyproject.toml` 的 include 列表是同一份约定，
/// 改名要三处一起改（build.rs 负责打包，这里负责判定可用）。
const SIDECAR: &[&str] = &["conpty.dll", "OpenConsole.exe"];

/// 解析侧载目录：模块目录下两个二进制齐备才算数。
pub fn conpty_dir() -> Option<&'static Path> {
    DIR.get_or_init(|| {
        let dir = env::module_dir()?;
        SIDECAR
            .iter()
            .all(|f| dir.join(f).is_file())
            .then(|| dir.to_path_buf())
    })
    .as_deref()
}

/// 把侧载目录交给 portable-pty（幂等）。返回侧载是否生效。
pub fn activate() -> bool {
    match conpty_dir() {
        Some(dir) => {
            portable_pty::set_conpty_dir(dir.to_path_buf());
            true
        }
        None => false,
    }
}

/// 侧载是否已生效。
pub fn active() -> bool {
    portable_pty::CONPTY_DIR.get().is_some()
}
