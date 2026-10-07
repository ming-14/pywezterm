//! pty 生命周期：创建、启动子进程、尺寸、终止、关闭。
//!
//! 只管 pty 本身 —— 读写由调用方（或 [`super::Pane`] 的 reader 线程）驱动。

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize, SlavePty};

use crate::error::{Error, Result};

/// pty 写端。reader 线程与调用方共用，因此用 `Arc<Mutex<..>>`。
pub type Writer = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

/// 伪终端。
pub struct Pty {
    master: Option<Box<dyn MasterPty + Send>>,
    /// slave 与 master 持有同一份 HPCON 引用，必须一起释放，否则 ConPTY 会提前退出
    slave: Option<Box<dyn SlavePty + Send>>,
    writer: Writer,
    child: Option<Box<dyn Child + Send + Sync>>,
}

impl Pty {
    /// 创建伪终端，返回自身与读端。
    ///
    /// 此时还没有子进程；读端可以立刻交给 reader 线程 —— 它会在子进程写入前阻塞。
    pub fn open(cols: u16, rows: u16) -> Result<(Self, Box<dyn Read + Send>)> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Pty(e.into()))?;
        let writer = pair.master.take_writer().map_err(|e| Error::Pty(e.into()))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Error::Pty(e.into()))?;
        Ok((
            Self {
                master: Some(pair.master),
                slave: Some(pair.slave),
                writer: Arc::new(Mutex::new(Some(writer))),
                child: None,
            },
            reader,
        ))
    }

    /// 写端句柄。reader 线程只读不写，写端由调用方使用；`Arc<Mutex<..>>` 是因为 `Pane`
    /// 与 `Pty` 各持一份引用，且关闭时要能同时作废。
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    /// 启动子进程，返回 `(pid, 进程句柄)`。
    ///
    /// - `raw_cmdline`：整条命令行原样传递（绕过 argv 引号序列化），供 `cmd.exe /c`
    ///   这类自解析命令行的程序保留引号语义；
    /// - `job_handle`：作业对象句柄。提供时子进程在 `CreateProcessW` 时就进入该作业，
    ///   不存在「创建后再赋值」的时间窗。句柄由调用方持有并负责关闭。
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &mut self,
        argv: Vec<String>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        raw_cmdline: Option<String>,
        job_handle: Option<usize>,
    ) -> Result<(u32, usize)> {
        let args: Vec<OsString> = argv.into_iter().map(OsString::from).collect();
        let mut builder = CommandBuilder::from_argv(args);
        #[cfg(windows)]
        {
            if let Some(raw) = raw_cmdline {
                builder.set_raw_cmdline(raw);
            }
            if let Some(job) = job_handle {
                builder.set_job_handle(job as winapi::um::winnt::HANDLE);
            }
        }
        #[cfg(not(windows))]
        let _ = (raw_cmdline, job_handle);
        if let Some(cwd) = cwd {
            builder.cwd(cwd);
        }
        if let Some(env) = env {
            for (k, v) in env {
                builder.env(k, v);
            }
        }

        let child = self
            .slave
            .as_ref()
            .ok_or(Error::Closed)?
            .spawn_command(builder)
            .map_err(|e| Error::Pty(e.into()))?;
        let pid = child.process_id().unwrap_or(0);
        #[cfg(windows)]
        let handle = child
            .as_raw_handle()
            .map(|h| h as usize)
            .unwrap_or_default();
        #[cfg(not(windows))]
        let handle = 0;
        self.child = Some(child);
        Ok((pid, handle))
    }

    /// 调整尺寸。
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .as_ref()
            .ok_or(Error::Closed)?
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| Error::Pty(e.into()))
    }

    /// 当前尺寸；pty 已关闭时为 `(0, 0)`。
    pub fn size(&self) -> (u16, u16) {
        self.master
            .as_ref()
            .and_then(|m| m.get_size().ok())
            .map(|s| (s.cols, s.rows))
            .unwrap_or((0, 0))
    }

    /// 写入 pty。阻塞调用，调用方负责放掉 GIL。
    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut guard = self.writer.lock().unwrap();
        match guard.as_mut() {
            Some(w) => w.write_all(data).map_err(|e| Error::Pty(e.into())),
            None => Err(Error::Closed),
        }
    }

    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }

    /// 非阻塞查询退出码；`None` = 仍在运行或未启动。
    pub fn try_wait(&mut self) -> Option<u32> {
        match self.child.as_mut()?.try_wait() {
            Ok(Some(status)) => Some(status.exit_code()),
            _ => None,
        }
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.process_id())
    }

    /// 子进程句柄（作业对象注册用），仅 Windows 有。
    pub fn child_handle(&self) -> Option<usize> {
        #[cfg(windows)]
        {
            self.child
                .as_ref()
                .and_then(|c| c.as_raw_handle())
                .map(|h| h as usize)
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 底层 ConPTY 句柄，仅 Windows 有。
    pub fn hpcon(&self) -> Option<usize> {
        #[cfg(windows)]
        {
            self.master.as_ref().and_then(|m| m.hpcon().map(|h| h as usize))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// 释放 pty：写端作废、master/slave 一起释放（关闭伪控制台并让 reader 的阻塞读返回）。
    pub fn close(&mut self) {
        *self.writer.lock().unwrap() = None;
        self.master = None;
        self.slave = None;
    }
}
