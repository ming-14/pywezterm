//! pywezterm 构建脚本
//!
//! Windows x64 构建时把 ConPTY 侧载二进制（conpty.dll + OpenConsole.exe）复制到
//! OUT_DIR/conpty/，由 maturin 的 [tool.maturin] include（from = "out-dir"）
//! 打包进 wheel 的 pywezterm/ 包目录。
//! 其余平台/架构不复制（含 Windows ARM64/i386，侧载 conpty 仅 x64 版，异架构
//! 无法加载，直接走系统内核 ConPTY）——OUT_DIR 无匹配文件时 maturin 仅告警
//! 并继续，wheel 不含 Windows 二进制（干净）。

fn main() {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        // 源文件：assets/windows/conhost/ 下的 conpty.dll + OpenConsole.exe
        let src_dir = manifest_dir.join("../../assets/windows/conhost");

        // 复制到 OUT_DIR/conpty/，由 maturin include conpty/* 打包
        let dest_dir = out_dir.join("conpty");
        std::fs::create_dir_all(&dest_dir).unwrap();

        for file in &["conpty.dll", "OpenConsole.exe"] {
            let src = src_dir.join(file);
            if src.exists() {
                std::fs::copy(&src, dest_dir.join(file)).unwrap();
                println!("cargo:rerun-if-changed={}", src.display());
            }
        }
    }
}