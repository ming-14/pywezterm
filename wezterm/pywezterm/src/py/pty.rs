//! `Pty` —— 伪终端。

use std::collections::HashMap;
use std::time::Duration;

use pyo3::prelude::*;

use pywezterm_core::host::{Driver, Pane};

use super::error::{IntoPyResult, Result};

/// 伪终端。
///
/// 只做「字节管道」：reader 线程把子进程输出放进读缓冲，[`read`](Self::read) 取走；
/// 喂模型与回写应答由调用方决定（见 `Terminal`）。自驱动形态见 `Mux` 的窗格。
#[pyclass(name = "Pty")]
pub struct PyPty {
    pane: Pane,
}

impl PyPty {
    fn pane(&self) -> &Pane {
        &self.pane
    }
}

#[pymethods]
impl PyPty {
    /// 创建伪终端（仅创建伪控制台，尚未启动子进程）。
    #[new]
    #[pyo3(signature = (cols=80, rows=24))]
    fn new(cols: usize, rows: usize) -> Result<Self> {
        Ok(Self {
            pane: Pane::open_pty(cols, rows, 0, Driver::Caller).py()?,
        })
    }

    /// 启动子进程，返回 `(pid, 进程句柄)`。
    ///
    /// `raw_cmdline`（Windows）：整条命令行原样传递，供 `cmd.exe /c` 这类自解析命令行的
    /// 程序保留引号语义。`job_handle`（Windows）：作业对象句柄，子进程在 `CreateProcessW`
    /// 时就进入该作业，不存在「创建后再赋值」的时间窗；句柄由调用方持有并关闭。
    #[pyo3(signature = (argv, cwd=None, env=None, raw_cmdline=None, job_handle=None))]
    fn spawn(
        &self,
        py: Python<'_>,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        raw_cmdline: Option<String>,
        job_handle: Option<usize>,
    ) -> Result<(u32, usize)> {
        py.detach(|| self.pane().spawn(argv, cwd, env, raw_cmdline, job_handle))
            .py()
    }

    /// 读取最多 `n` 字节；超时或 EOF 返回 `b""`。
    ///
    /// `timeout=None` 阻塞到有数据或 EOF。等待期间释放 GIL，不阻塞其他 Python 线程。
    #[pyo3(signature = (n=65536, timeout=None))]
    fn read(&self, py: Python<'_>, n: usize, timeout: Option<f64>) -> Vec<u8> {
        let timeout = timeout.map(|t| Duration::from_secs_f64(t.clamp(0.0, 86_400.0)));
        py.detach(|| self.pane().read(n, timeout))
    }

    /// 写入 pty。pty 写满时会阻塞，期间释放 GIL。
    fn write(&self, py: Python<'_>, data: Vec<u8>) -> Result<()> {
        py.detach(|| self.pane().write(&data)).py()
    }

    /// 调整伪终端尺寸。
    fn resize(&self, py: Python<'_>, cols: usize, rows: usize) -> Result<()> {
        py.detach(|| self.pane().resize(cols, rows)).py()
    }

    /// 当前尺寸 `(cols, rows)`；已关闭时为 `(0, 0)`。
    fn get_size(&self, py: Python<'_>) -> (u16, u16) {
        py.detach(|| self.pane().pty_size())
    }

    /// 读缓冲中待取走的字节数。正常不会超过 1 MiB —— 它是背压生效的可观测证据。
    fn buffered_bytes(&self, py: Python<'_>) -> usize {
        py.detach(|| self.pane().buffered_bytes())
    }

    /// 子进程 pid；未启动为 `None`。
    fn child_pid(&self, py: Python<'_>) -> Option<u32> {
        py.detach(|| self.pane().child_pid())
    }

    /// 子进程句柄（作业对象注册用，仅 Windows）；未启动为 `None`。
    #[cfg(windows)]
    fn child_handle(&self, py: Python<'_>) -> Option<usize> {
        py.detach(|| self.pane().child_handle())
    }

    /// 底层 ConPTY 句柄（仅 Windows）。
    #[cfg(windows)]
    fn hpcon(&self, py: Python<'_>) -> Option<usize> {
        py.detach(|| self.pane().hpcon())
    }

    /// 非阻塞查询退出码；`None` = 仍在运行或未启动。
    fn try_wait(&self, py: Python<'_>) -> Option<u32> {
        py.detach(|| self.pane().try_wait())
    }

    /// 终止子进程。
    fn kill(&self, py: Python<'_>) {
        py.detach(|| self.pane().kill())
    }

    /// 关闭伪终端。幂等；关闭后 [`read`](Self::read) 恒为 `b""`。
    fn close(&self, py: Python<'_>) {
        py.detach(|| self.pane().close())
    }
}
