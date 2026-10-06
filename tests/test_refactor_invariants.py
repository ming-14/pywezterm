# 重构回归：不变量与失败路径
#
# 这里放的不是「API 能不能用」，而是「结构保证」：
#   - 分块不变性（同一字节流按不同方式切分，结果必须一致）
#   - 失败路径不留下损坏状态
#   - 幂等性
# 这几类问题原来的用例形态（喂一小段查状态）抓不到。

import os

import pytest

import pywezterm


def _shell_argv(*args):
    if os.name == "posix":
        return ["/bin/sh", "-c", " ".join(args) or "true"]
    return [os.environ.get("COMSPEC", "cmd.exe"), "/c", *args]


# ---- 分块不变性 ----------------------------------------------------------


def _feed_chunked(data, chunk):
    t = pywezterm.Terminal(40, 6, 100)
    for i in range(0, len(data), chunk):
        t.feed(data[i : i + chunk])
    return t


def _stream():
    parts = []
    for i in range(20):
        parts.append(f"line {i} with padding text\r\n".encode())
        if i == 3:
            parts.append(b"\x1b[?1000h\x1b[?1006h\x1b[?2004h")
        if i == 15:
            parts.append(b"\x1b[?1049h")
    return b"".join(parts)


@pytest.mark.parametrize("chunk", [1, 3, 7, 63, 64, 65, 4096])
def test_feed_is_chunk_invariant(chunk):
    """模式序列埋在输出中间时，切分方式不得影响观察结果。

    窗口式实现（只看每块末尾 N 字节）会在这里漏掉整块前面的序列。
    """
    whole = _feed_chunked(_stream(), len(_stream()))
    split = _feed_chunked(_stream(), chunk)

    assert split.text() == whole.text(), f"chunk={chunk} 文本不一致"
    assert split.snapshot() == whole.snapshot(), f"chunk={chunk} 网格不一致"
    assert split.get_mouse_encoding() == whole.get_mouse_encoding(), f"chunk={chunk}"
    assert split.is_alt_screen_active() == whole.is_alt_screen_active(), f"chunk={chunk}"
    assert split.bracketed_paste_enabled() == whole.bracketed_paste_enabled(), f"chunk={chunk}"
    assert split.mode_restore_seq() == whole.mode_restore_seq(), f"chunk={chunk}"


def test_mode_restore_seq_reflects_observed_modes():
    t = pywezterm.Terminal(80, 24)
    t.feed(b"\x1b[?1000h\x1b[?1006h\x1b[?12l")
    seq = t.mode_restore_seq()
    assert "\x1b[?1000h" in seq
    assert "\x1b[?1006h" in seq
    assert "\x1b[?12l" in seq
    # 没观察到的模式不得出现（否则会覆盖客户端自己的配置）
    assert "1049" not in seq
    assert "?25" not in seq


# ---- 渲染 ----------------------------------------------------------------


def test_svg_compression_preserves_cjk():
    t = pywezterm.Terminal(40, 4)
    t.feed("你好，世界 🚀\r\n".encode())
    for level in (0, 1, 2):
        svg = t.render_svg(level)
        assert "你好，世界" in svg, f"level={level} 中文被破坏"
        assert "🚀" in svg, f"level={level} emoji 被破坏"


def test_render_image_rejects_bad_scale():
    t = pywezterm.Terminal(20, 4)
    t.feed(b"hello\r\n")
    for scale in (0.0, -1.0, float("inf"), 1e9):
        with pytest.raises(Exception):
            t.render_image(scale, "png")
    # 合法值仍然可用
    assert t.render_image(1.0, "png")[:4] == b"\x89PNG"


def test_surface_clear_actually_clears():
    s = pywezterm.Surface(8, 2)
    s.set_cell(0, 0, "hi")
    s.get_changes_bytes(0)
    s.clear()
    _, frame = s.repaint_bytes()
    assert b"hi" not in frame, "clear 后旧内容被重绘回来了"


# ---- 参数校验 ------------------------------------------------------------


def test_invalid_key_name_raises_value_error():
    t = pywezterm.Terminal(20, 4)
    with pytest.raises(ValueError):
        t.key_down("NotAKey", 0)
    with pytest.raises(ValueError):
        t.key_down("F99", 0)
    # 单字符与功能键仍可用
    assert t.key_down("a", 0) == b"a"
    assert t.key_down("F1", 0)


def test_invalid_mouse_args_raise_value_error():
    t = pywezterm.Terminal(20, 4)
    with pytest.raises(ValueError):
        t.mouse(0, 0, kind="click")
    with pytest.raises(ValueError):
        t.mouse(0, 0, button="thumb")


# ---- 失败路径与幂等性 ----------------------------------------------------


def test_add_pane_failure_leaves_mux_usable():
    """spawn 失败不得留下损坏状态 —— 原来的实现会先改布局与焦点，之后 render() 越界崩溃。"""
    m = pywezterm.Mux(80, 24)
    try:
        with pytest.raises(Exception):
            m.add_pane(["this-program-does-not-exist-9f3a2b"])
        assert m.pane_count() == 0
        assert m.pane_rects() == []
        # 关键：失败后 render() 仍可用，而不是崩溃
        frame, row, col, visible = m.render()
        assert isinstance(frame, bytes)
        # 失败后仍能正常建窗格
        pane = m.add_pane(_shell_argv("exit"))
        assert pane == 0
    finally:
        m.close()


def test_pane_ids_are_stable_across_close():
    """关掉 0 号不得让 1 号变成 0 号 —— 宿主缓存的 id 不能打到别的窗格上。"""
    m = pywezterm.Mux(80, 24)
    try:
        a = m.add_pane(_shell_argv("exit"))
        b = m.add_pane(_shell_argv("exit"))
        assert (a, b) == (0, 1)

        m.close_pane(a)
        assert m.pane_count() == 1
        assert m.pane_rects() != []
        # b 仍然指向自己
        assert isinstance(m.pane_text(b), str)
        with pytest.raises(pywezterm.PaneNotFound):
            m.pane_text(a)

        # 新窗格不复用已关闭的 id
        c = m.add_pane(_shell_argv("exit"))
        assert c not in (a, b)
    finally:
        m.close()


def test_close_pane_and_close_are_idempotent():
    m = pywezterm.Mux(80, 24)
    pane = m.add_pane(_shell_argv("exit"))
    m.close_pane(pane)
    m.close_pane(pane)
    m.close()
    m.close()
    assert m.pane_count() == 0


def test_pty_close_wakes_blocked_read():
    p = pywezterm.Pty(80, 24)
    p.spawn(_shell_argv("exit"))
    p.close()
    assert p.read(16, timeout=1.0) == b""
    assert p.get_size() == (0, 0)
    p.close()


def test_terminal_resize_is_stable():
    t = pywezterm.Terminal(40, 6, 100)
    t.feed(b"hello world\r\n")
    before = t.text()
    t.resize(40, 6)  # 同尺寸
    assert t.text() == before
    t.resize(20, 6)
    t.resize(40, 6)
    assert "hello world" in t.text()


# ---- 部署自省 ------------------------------------------------------------


def test_env_info_reports_deployment():
    info = pywezterm.env_info()
    assert set(info) == {"module_dir", "conpty_dir", "conpty_active"}
    assert info["module_dir"], "模块目录应可解析（由 .pyd 自身路径推导）"
    if os.name == "nt":
        # 侧载是否生效必须是可查的事实，而不是静默回落
        assert isinstance(info["conpty_active"], bool)
    else:
        assert info["conpty_active"] is False
