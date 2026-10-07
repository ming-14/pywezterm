# 黑盒测试入口：确保导入的是「本仓库刚构建」的 pywezterm，
# 而不是 agentic-tty/vendor/pywezterm 这类旧副本（ARCHITECTURE 已警告该坑：
# vendor 副本在 sys.path 上排在仓库构建之前，会抢先被导入，导致测的是旧代码）。
#
# 这里把仓库根插到 sys.path 最前，使 `import pywezterm` 解析到
# reference/pywezterm/pywezterm/（含新鲜编译的 .pyd）。
import os
import sys

_REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _REPO_ROOT not in sys.path:
    sys.path.insert(0, _REPO_ROOT)
