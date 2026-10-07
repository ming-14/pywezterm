//! 把 ConPTY 侧载二进制放进 OUT_DIR 根下，交给 maturin 打包。
//!
//! Windows x64 构建时复制 `conpty.dll` + `OpenConsole.exe` 到 `OUT_DIR/` **根下**，
//! 由 `pyproject.toml` 的 `[tool.maturin] include`（`from = "out-dir"`，按单个文件名）
//! 打包进 wheel 的 `pywezterm/` 包目录，与 `pywezterm.pyd` 同目录 ——
//! `pywezterm-core` 的 `platform::windows::conpty` 正是按 `<包目录>/conpty.dll` 定位它。
//!
//! 不要先复制到 `OUT_DIR` 的子目录再用 `conpty/*` 通配：maturin 的 include 会保留 glob
//! 里的目录层级，文件会落进 `pywezterm/conpty/`，加载器就找不到（曾如此）。
//!
//! 其他平台/架构不复制（侧载 conpty 只有 x64 版，异架构无法加载，直接走系统内核 ConPTY）。
//! OUT_DIR 无匹配文件时 maturin 仅告警并继续，wheel 因此不含 Windows 二进制。

/// 侧载二进制文件名。必须与 `pyproject.toml` 的 include 列表、
/// `pywezterm-core/src/platform/windows/conpty.rs` 的 `SIDECAR` 一致。
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const SIDECAR: &[&str] = &["conpty.dll", "OpenConsole.exe"];

fn main() {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let src_dir = manifest_dir.join("../../assets/windows/conhost");

        for file in SIDECAR {
            let src = src_dir.join(file);
            // 源文件缺失是打包错误，不是可忽略的情况：静默跳过会产出一个
            // 「看起来正常但实际回落到系统 conhost」的 wheel
            if let Err(e) = std::fs::copy(&src, out_dir.join(file)) {
                panic!(
                    "复制侧载二进制失败 {} -> {}: {e}",
                    src.display(),
                    out_dir.display()
                );
            }
            println!("cargo:rerun-if-changed={}", src.display());
        }
    }
}
