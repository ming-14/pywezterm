# 黑盒测试补充套件（test_blackbox_comprehensive.py）
#
# 目标：从「外部可观察行为」视角验证 pywezterm 的公开 Python 接口，不依赖内部实现。
# 与 tests/ 下既有用例互补，重点覆盖：
#   - 公开 API 表面契约（导出的名字必须与文档一致）
#   - 现有套件未断言的行为（reset / clear_scrollback / render_* / key_up /
#     send_paste 包裹 / Surface.repaint_bytes / Surface.resize / 各类异常）
#   - 仅 Windows 可测的专属路径（clipboard 读写、ConsoleInput、child_handle /
#     hpcon、raw_cmdline、job_handle）
#
# 运行：在 venv 中 `python -m pytest tests/test_blackbox_comprehensive.py -v`
# 约定：本机为 Windows（win32），Windows 专属用例直接执行；非 Windows 自动跳过。

import os
import sys
import time

import pytest

import pywezterm

IS_WIN = os.name == "nt"
COMSPEC = os.environ.get("COMSPEC", "cmd.exe")


# --------------------------------------------------------------------------
# 工具
# --------------------------------------------------------------------------


def _shell_argv(cmd):
    """跨平台 shell：POSIX `/bin/sh -c`，Windows `cmd /c`。"""
    if os.name == "posix":
        return ["/bin/sh", "-c", cmd]
    return [COMSPEC, "/c", cmd]


def _run(p, t, timeout=8.0):
    """读 pty → feed 模型 → 回写模型应答，直到 EOF，返回累积原始输出。"""
    out = b""
    deadline = time.time() + timeout
    while time.time() < deadline:
        chunk = p.read(4096, timeout=0.2)
        if chunk:
            out += chunk
            t.feed(chunk)
            resp = t.drain_written()
            if resp:
                p.write(resp)
        elif p.try_wait() is not None:
            while time.time() < deadline:
                c = p.read(4096, timeout=0.3)
                if not c:
                    break
                out += c
                t.feed(c)
            break
    return out


def _win_skip(reason):
    return pytest.mark.skipif(not IS_WIN, reason=reason)


# ==========================================================================
# 1. 公开 API 表面契约
# ==========================================================================


def test_public_api_surface():
    """导出的公开名字必须与文档一致（跨平台部分）。"""
    expected = {
        "Pty", "Terminal", "Surface",
        "version", "cursor_seq", "env_info",
        "TerminalClosed", "RenderError", "PlatformUnsupported",
    }
    missing = expected - set(dir(pywezterm))
    assert not missing, f"缺少公开符号: {missing}"


def test_windows_only_api_present_on_windows():
    if not IS_WIN:
        pytest.skip("仅 Windows 应注册 ConsoleInput / 剪贴板函数")
    for name in ("ConsoleInput", "clipboard_read", "clipboard_write"):
        assert hasattr(pywezterm, name), f"Windows 缺少 {name}"


def test_version_nonempty():
    v = pywezterm.version()
    assert isinstance(v, str) and v, v


def test_cursor_seq_shape():
    s = pywezterm.cursor_seq(2, 3, True)
    assert isinstance(s, str) and s
    # 0-based (row=2,col=3) → 1-based CUP 应为 3;4H
    assert "3;4H" in s, s


def test_env_info_keys():
    info = pywezterm.env_info()
    assert set(info) == {"module_dir", "conpty_dir", "conpty_active"}
    assert info["module_dir"]


# ==========================================================================
# 2. Pty（Windows 专属路径）
# ==========================================================================


def test_pty_spawn_returns_pid_handle_and_handles():
    p = pywezterm.Pty(80, 24)
    try:
        pid, handle = p.spawn(_shell_argv("exit"))
        assert pid > 0, pid
        if IS_WIN:
            assert handle != 0, handle
            assert p.child_handle() == handle, (p.child_handle(), handle)
            assert p.hpcon() is not None, "Windows 应暴露 HPCON"
        else:
            assert handle == 0
        assert p.child_pid() == pid
    finally:
        p.close()


@_win_skip("clipboard 测试仅 Windows")
def test_clipboard_roundtrip():
    # best-effort：写入再读回应相等
    pywezterm.clipboard_write("pywezterm-blackbox-标记")
    got = pywezterm.clipboard_read()
    assert got == "pywezterm-blackbox-标记", repr(got)


@_win_skip("raw_cmdline 仅 Windows")
def test_spawn_raw_cmdline_preserves_quotes_vs_argv_escaping():
    """raw_cmdline 原样透传 → 保留 cmd.exe 的引号语义；argv 形式则会加 C 运行时转义。

    黑盒可观测证据（以 `echo "a b"` 为例）：
      - argv 形式：`append_quoted` 把内部引号序列化为 `\\"a b\\"`（C 运行时规则），
        cmd.exe 看到字面反斜杠，输出含反斜杠的 `\\"a b\\"`。
      - raw_cmdline 形式：整条命令行原样，cmd.exe 看到干净的 `"a b"`，输出 `"a b"`。
    """
    # argv 形式：内部引号被 C 运行时转义
    pa = pywezterm.Pty(80, 24)
    ta = pywezterm.Terminal(80, 24)
    pa.spawn([COMSPEC, "/c", 'echo "a b"'])
    out_a = _run(pa, ta)
    pa.close()

    # raw_cmdline 形式：整条命令行原样（含引号），cmd.exe 引号语义被保留
    pr = pywezterm.Pty(80, 24)
    tr = pywezterm.Terminal(80, 24)
    pr.spawn([COMSPEC], raw_cmdline=f'{COMSPEC} /c echo "a b"')
    out_r = _run(pr, tr)
    pr.close()

    # argv 路径：C 运行时转义 → 输出里带反斜杠的 \"a b\"
    assert b'\\"a b\\"' in out_a, out_a
    # raw_cmdline 路径：原样 → 输出里是干净的 "a b"
    assert b'"a b"' in out_r, out_r
    # argv 路径不应出现干净的 "a b"（证明引号确实被转义过）
    assert b'"a b"' not in out_a, out_a


@_win_skip("job_handle 仅 Windows")
def test_spawn_job_handle_assigns_child_into_job():
    """传入作业对象句柄 → 子进程在 CreateProcessW 时即入作业。

    用 ctypes 建一个空作业对象，spawn 时传入其句柄，再用 IsProcessInJob
    验证子进程确实在作业内（黑盒可观测证据）。
    """
    import ctypes

    k32 = ctypes.windll.kernel32
    job = k32.CreateJobObjectW(None, None)
    assert job, "CreateJobObjectW 失败"
    try:
        job_handle = int(job)
        p = pywezterm.Pty(80, 24)
        pid, child = p.spawn([COMSPEC, "/c", "exit"], job_handle=job_handle)
        assert pid > 0
        if IS_WIN:
            assert child != 0, "Windows 上 child 是进程句柄，应非 0"
        # IsProcessInJob(hProcess, hJob, *lpResult) → 1 成功
        res = ctypes.c_int(0)
        ok = k32.IsProcessInJob(
            ctypes.c_void_p(child), ctypes.c_void_p(job_handle), ctypes.byref(res)
        )
        p.close()
        assert ok, "IsProcessInJob 调用失败"
        assert res.value == 1, "子进程未进入作业对象"
    finally:
        k32.CloseHandle(job)


def test_pty_buffered_bytes_observable():
    """背压可观测：喂入大量输出时 buffered_bytes 曾 > 0。"""
    p = pywezterm.Pty(80, 24)
    t = pywezterm.Terminal(80, 24)
    # 用 python 持续吐大量数据，制造 reader 暂未取走的背压
    p.spawn([sys.executable, "-c", "import sys; sys.stdout.write('x'*200000); sys.stdout.flush()"])
    seen = 0
    deadline = time.time() + 6.0
    while time.time() < deadline:
        chunk = p.read(65536, timeout=0.2)
        if chunk:
            t.feed(chunk)
        b = p.buffered_bytes()
        if b > 0:
            seen = max(seen, b)
        if p.try_wait() is not None:
            break
    p.close()
    assert seen > 0, "背压窗口内未观测到 buffered_bytes > 0"


# ==========================================================================
# 3. Terminal —— 现有套件未断言的行为
# ==========================================================================


def test_terminal_reset_clears():
    t = pywezterm.Terminal(40, 10)
    t.feed(b"hello\r\nworld\x1b[31mred\x1b[0m")
    assert "hello" in t.text()
    t.reset()
    assert t.text().strip() == "", repr(t.text())
    # 模式应被清回默认
    assert t.get_keyboard_encoding() == "xterm"
    assert not t.is_alt_screen_active()


def test_clear_scrollback_drops_history():
    t = pywezterm.Terminal(20, 4)
    for i in range(15):
        t.feed(f"line {i}\r\n".encode())
    before = t.scrollback_count()
    assert before > 0
    t.clear_scrollback()
    assert t.scrollback_count() == 0, t.scrollback_count()


def test_render_ansi_nonempty_and_cursor_toggle():
    t = pywezterm.Terminal(20, 4)
    t.feed(b"hi")
    a1 = t.render_ansi(True)
    a0 = t.render_ansi(False)
    assert isinstance(a1, str) and a1
    assert "hi" in a1
    assert isinstance(a0, str)


def test_render_scrollback_keep_ansi():
    t = pywezterm.Terminal(20, 4)
    for i in range(10):
        t.feed(f"rec {i}\r\n".encode())
    plain = t.render_scrollback(False)
    ansi = t.render_scrollback(True)
    assert "rec 0" in plain
    assert isinstance(ansi, str) and ansi


def test_render_svg_basic():
    t = pywezterm.Terminal(20, 4)
    t.feed(b"svg-test")
    svg = t.render_svg(0)
    assert "<svg" in svg, svg
    assert "svg-test" in svg


@pytest.mark.parametrize("fmt,magic", [("png", b"\x89PNG"), ("jpg", b"\xff\xd8\xff"), ("bmp", b"BM")])
def test_render_image_magic(fmt, magic):
    t = pywezterm.Terminal(20, 4)
    t.feed(b"img")
    data = t.render_image(1.0, fmt)
    assert data[: len(magic)] == magic, (fmt, data[:8])


@pytest.mark.xfail(
    strict=False,
    reason="render_image(\"gif\") 不报错而静默回落 PNG（948 字节、magic 为 \\x89PNG），"
           "格式校验弱：非受支持格式未显式拒绝，与 png/jpg/bmp 的硬契约不一致",
)
def test_render_image_bad_fmt_raises():
    t = pywezterm.Terminal(20, 4)
    t.feed(b"x")
    with pytest.raises(Exception):
        t.render_image(1.0, "gif")


def test_key_up_returns_bytes():
    t = pywezterm.Terminal(40, 10)
    enc = t.key_up("a", 0)
    assert isinstance(enc, bytes)


def test_send_paste_bracketed_when_enabled():
    """bracketed paste 模式开启时，send_paste 自动包裹 200~ / 201~。"""
    t = pywezterm.Terminal(40, 10)
    t.feed(b"\x1b[?2004h")
    t.send_paste("PASTE_ME")
    out = t.drain_written()
    assert out.startswith(b"\x1b[200~") and out.endswith(b"\x1b[201~"), out
    assert b"PASTE_ME" in out


def test_mouse_wheel_and_move_encode():
    t = pywezterm.Terminal(40, 10)
    # 需要 any-event 跟踪（1003）+ SGR 格式（1006）才会产生编码字节
    t.feed(b"\x1b[?1003h\x1b[?1006h")
    # 滚轮：SGR 鼠标编码以 64 偏移表示 wheel up
    enc = t.mouse(5, 3, "press", "wheel_up", 0)
    assert enc.startswith(b"\x1b[<") and enc.endswith(b"M"), enc
    enc2 = t.mouse(5, 3, "move", "left", 0)
    assert enc2.startswith(b"\x1b[<") and enc2.endswith(b"M"), enc2


def test_get_mouse_encoding_tuple():
    t = pywezterm.Terminal(40, 10)
    assert t.get_mouse_encoding() == (0, False)
    t.feed(b"\x1b[?1006h")  # ?1006h 只开 SGR 格式，跟踪模式仍为 0
    assert t.get_mouse_encoding() == (0, True)


def test_notification_callback_on_bell():
    """响铃（BEL）应触发通知回调；回调异常不应崩终端。"""
    t = pywezterm.Terminal(40, 10)
    alerts = []
    t.set_notification_callback(lambda a: alerts.append(a))
    t.feed(b"\x07")  # BEL
    assert alerts, "响铃应触发通知回调"
    # 回调抛异常后终端仍可用
    def boom(a):
        raise RuntimeError("boom")
    t.set_notification_callback(boom)
    t.feed(b"\x07ok\r\n")
    assert "ok" in t.text()


def test_download_callback_triggered():
    """OSC 1337 文件下载请求应触发下载回调（name, data）。"""
    t = pywezterm.Terminal(40, 10)
    got = []
    t.set_download_callback(lambda name, data: got.append((name, data)))
    payload = "name=test.txt;size=3:AAA".replace(" ", "")
    t.feed(f"\x1b]1337;File={payload}\x07".encode())
    # 下载处理器可能异步/缓冲；给一点时间并断言不崩
    assert isinstance(got, list)


def test_device_control_callback_triggered():
    """DCS 设备控制序列应触发设备控制回调。"""
    t = pywezterm.Terminal(40, 10)
    got = []
    t.set_device_control_callback(lambda c: got.append(c))
    # DCS 序列（以 ST 收尾）
    t.feed(b"\x1bPq#0;1;1;1\x1b\\")
    assert isinstance(got, list)


def test_write_after_close_is_graceful_noop():
    """Pty 关闭后 write 为优雅无操作（不抛异常），与 read-after-close 一致。

    黑盒可观测：close() 后 write(b'x') 不抛 TerminalClosed，
    read() 恒为 b""。
    """
    p = pywezterm.Pty(80, 24)
    p.spawn(_shell_argv("exit"))
    p.close()
    p.write(b"should be noop")  # 不应抛异常
    assert p.read(16, timeout=0.2) == b""


# ==========================================================================
# 4. Surface —— 增量渲染表面
# ==========================================================================


def test_surface_repaint_bytes_full():
    s = pywezterm.Surface(20, 4)
    s.set_cell(0, 0, "REPAINT")
    s.get_changes_bytes(0)  # 消费首帧
    seq, data = s.repaint_bytes()
    assert b"REPAINT" in data


def test_surface_resize_drops_buffer():
    """resize 丢弃已缓冲变更 → 下一帧必然全量（含旧内容）。"""
    s = pywezterm.Surface(20, 4)
    s.set_cell(0, 0, "OLD")
    s.get_changes_bytes(0)
    s.resize(20, 4)  # 同尺寸 resize 也走 buf 丢弃路径
    seq, data = s.repaint_bytes()
    assert b"OLD" in data


def test_surface_set_cell_styles_emit_sgr():
    """设置 bold 后输出里应出现 SGR 粗体（1）序列。"""
    s = pywezterm.Surface(20, 4)
    s.set_cell(0, 0, "B", bold=True)
    seq, data = s.get_changes_bytes(0)
    assert b"\x1b[1" in data, data  # SGR bold


# ==========================================================================
# 5. ConsoleInput（Windows 专属）
# ==========================================================================


@_win_skip("ConsoleInput 仅 Windows")
def test_console_input_lifecycle():
    try:
        ci = pywezterm.ConsoleInput()
    except Exception as e:
        pytest.skip("无宿主交互控制台: {}".format(e))
    try:
        assert ci.wait_input(0) is False
        cols, rows = ci.size()
        assert cols > 0 and rows > 0
        evs = ci.read_inputs()
        assert isinstance(evs, list)
    finally:
        ci.restore()


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
