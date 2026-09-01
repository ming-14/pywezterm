#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""BUILD.py - pywezterm 跨平台构建脚本（Windows / Linux / macOS）

功能：编译 pywezterm（pyo3/abi3，portable-pty + wezterm-term），
      输出 wheel 到 --wheel-dir（默认 <仓库根>/target/wheels）。
依赖：Rust（cargo/rustc，rustup 安装）；Windows 另需 Visual Studio
      （vcvars64.bat，自动探测或 --vcvars 指定）；Python 与 maturin
      （缺失时自动 pip 安装）。

参数：
  --config <Debug|Release>  构建类型（默认 Release）
  --rebuild                 清理编译缓存后全新构建
  --wheel-dir <path>        wheel 输出目录（默认 <仓库根>/target/wheels）
  --vcvars <path>           Windows 手动指定 vcvars64.bat（默认自动探测）
  --cargo-dir <path>        手动指定 cargo 所在目录（默认探测 ~/.cargo/bin 与 PATH）
  --python <path>           手动指定 python 可执行文件（默认取 PATH）

示例：
  python BUILD.py
  python BUILD.py --config Debug --rebuild
  python BUILD.py --rebuild --vcvars "D:\\VS\\VC\\Auxiliary\\Build\\vcvars64.bat"
  python BUILD.py --wheel-dir dist
"""

import argparse
import os
import shutil
import subprocess

IS_WINDOWS = os.name == "nt"


def find_python(manual):
    """定位 python 可执行文件：--python 优先，否则按 python/python3 顺序探测 PATH。"""
    if manual:
        return manual
    for name in ("python", "python3"):
        found = shutil.which(name)
        if found:
            return found
    return None


def find_cargo(cargo_dir):
    """定位 cargo：--cargo-dir 优先，其次 ~/.cargo/bin，最后 PATH。"""
    exe = "cargo.exe" if IS_WINDOWS else "cargo"
    candidates = []
    if cargo_dir:
        candidates.append(os.path.join(cargo_dir, exe))
    candidates.append(os.path.join(os.path.expanduser("~"), ".cargo", "bin", exe))
    candidates.append(shutil.which("cargo"))
    for c in candidates:
        if c and os.path.isfile(c):
            return c
    return None


def find_vcvars(manual):
    """定位 vcvars64.bat：--vcvars 优先，其次 vswhere 探测，最后常见安装路径。"""
    if manual:
        if os.path.isfile(manual):
            return manual
        raise SystemExit("指定的 vcvars64.bat 不存在: %s" % manual)
    vswhere = os.path.join(
        os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        "Microsoft Visual Studio", "Installer", "vswhere.exe",
    )
    if os.path.isfile(vswhere):
        out = subprocess.run(
            [vswhere, "-latest", "-products", "*",
             "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
             "-property", "installationPath"],
            capture_output=True, text=True,
        ).stdout.strip()
        if out:
            candidate = os.path.join(out, "VC", "Auxiliary", "Build", "vcvars64.bat")
            if os.path.isfile(candidate):
                return candidate
    pf = os.environ.get("ProgramFiles", r"C:\Program Files")
    fallback = [
        os.path.join(pf, "Microsoft Visual Studio", "2022", "Community",
                     "VC", "Auxiliary", "Build", "vcvars64.bat"),
        os.path.join(pf, "Microsoft Visual Studio", "2022", "BuildTools",
                     "VC", "Auxiliary", "Build", "vcvars64.bat"),
        os.path.join(pf, "Microsoft Visual Studio", "17", "Community",
                     "VC", "Auxiliary", "Build", "vcvars64.bat"),
        os.path.join(pf, "Microsoft Visual Studio", "17", "BuildTools",
                     "VC", "Auxiliary", "Build", "vcvars64.bat"),
    ]
    for c in fallback:
        if os.path.isfile(c):
            return c
    return None


def vcvars_env(vcvars_path):
    """运行 vcvars64.bat 并解析其设置的环境变量（cmd 输出 KEY=VALUE 逐行解析）。"""
    out = subprocess.run(
        ['cmd', '/c', 'call "%s" >nul 2>&1 && set' % vcvars_path],
        capture_output=True, text=True, errors="replace",
    )
    env = {}
    for line in out.stdout.splitlines():
        if "=" in line:
            key, _, value = line.partition("=")
            env[key] = value
    return env


def ensure_maturin(py_exe):
    """检查 maturin 是否可用，缺失时自动安装。"""
    if subprocess.run([py_exe, "-m", "maturin", "--version"],
                      capture_output=True).returncode == 0:
        return
    print("[pywezterm] maturin 未安装，正在安装...")
    if subprocess.run([py_exe, "-m", "pip", "install", "maturin>=1.0,<2.0"]).returncode != 0:
        raise SystemExit("maturin 安装失败")


def main():
    parser = argparse.ArgumentParser(description="pywezterm cross-platform build script")
    parser.add_argument("--config", choices=("Debug", "Release"), default="Release")
    parser.add_argument("--rebuild", action="store_true")
    parser.add_argument("--wheel-dir", default="")
    parser.add_argument("--vcvars", default="")
    parser.add_argument("--cargo-dir", default="")
    parser.add_argument("--python", default="")
    args = parser.parse_args()

    script_dir = os.path.dirname(os.path.abspath(__file__))
    wheels_dir = args.wheel_dir or os.path.join(script_dir, "target", "wheels")

    py_exe = find_python(args.python)
    if not py_exe:
        raise SystemExit("未找到 python，请安装 Python 并加入 PATH，或用 --python 显式指定")

    cargo_exe = find_cargo(args.cargo_dir)
    if not cargo_exe:
        raise SystemExit("未找到 cargo，请安装 Rust（https://rustup.rs）或用 --cargo-dir 指定")
    cargo_bin = os.path.dirname(cargo_exe)

    ensure_maturin(py_exe)

    # 构建环境：默认继承当前环境；Windows 额外注入 vcvars 环境
    build_env = dict(os.environ)
    if IS_WINDOWS:
        vcvars = find_vcvars(args.vcvars)
        if not vcvars:
            raise SystemExit("未找到 vcvars64.bat，请安装 VS C++ 桌面工作负载或用 --vcvars 显式指定")
        print("[pywezterm] vcvars64.bat: %s" % vcvars)
        build_env.update(vcvars_env(vcvars))
    build_env["PATH"] = cargo_bin + os.pathsep + build_env.get("PATH", "")
    print("[pywezterm] Python: %s" % py_exe)
    print("[pywezterm] cargo: %s" % cargo_exe)

    # 清理编译缓存
    if args.rebuild:
        for d in (os.path.join(script_dir, "wezterm", "target"), wheels_dir):
            if os.path.isdir(d):
                shutil.rmtree(d)
                print("[pywezterm] 清理: %s" % d)

    # 构建 wheel
    release_flag = ["--release"] if args.config == "Release" else []
    print("[pywezterm] 构建 %s ..." % args.config)
    result = subprocess.run(
        [py_exe, "-m", "maturin", "build"] + release_flag + ["--out", wheels_dir],
        cwd=script_dir, env=build_env,
    )
    if result.returncode != 0:
        raise SystemExit("构建失败（exit=%d），详见上方日志" % result.returncode)

    print("[pywezterm] 构建完成: %s" % wheels_dir)


if __name__ == "__main__":
    main()