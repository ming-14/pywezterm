"""ConsoleInput 绑定的条件测试：仅 Windows 有该绑定，其余平台跳过。"""

import os

import pytest

import pywezterm


@pytest.fixture(scope="module")
def console():
    if os.name != "nt":
        pytest.skip("ConsoleInput 仅支持 Windows")
    try:
        return pywezterm.ConsoleInput()
    except Exception as e:
        pytest.skip("无宿主交互控制台: {}".format(e))


def test_wait_input_no_event_timeout(console):
    assert console.wait_input(0) is False  # 无待处理事件：立即超时


def test_size_valid(console):
    cols, rows = console.size()
    assert cols > 0 and rows > 0


def test_read_inputs_type(console):
    # 空事件或任意事件都应能返回（不抛异常）；形状由绑定侧单测保证
    evs = console.read_inputs()
    assert isinstance(evs, list)


def test_restore_idempotent(console):
    console.restore()
    console.restore()
