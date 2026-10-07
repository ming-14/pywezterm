//! 统一错误类型。
//!
//! 领域层不构造 Python 异常，只产出 [`Error`]；到 Python 的映射集中在
//! `pywezterm` 的 `py::error`。

use std::fmt;

/// 本 crate 的错误。变体按「宿主调用方需要怎么处理」划分，而不是按出错位置。
#[derive(Debug)]
pub enum Error {
    /// pty / 子进程 / 管道 IO 失败。保留 anyhow 链，便于打印根因。
    Pty(anyhow::Error),
    /// 宿主单元已关闭。
    Closed,
    /// 参数非法（键名、鼠标按钮、渲染参数等）。
    Invalid(String),
    /// 渲染参数或编码失败。
    Render(String),
    /// 平台能力不可用（非 Windows 上的 ConPTY 侧载、控制台采集等）。
    Platform(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Pty(e) => write!(f, "{e:#}"),
            Error::Closed => write!(f, "终端已关闭"),
            Error::Invalid(m) => write!(f, "{m}"),
            Error::Render(m) => write!(f, "{m}"),
            Error::Platform(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Pty(e) => e.source(),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for Error {
    fn from(e: anyhow::Error) -> Self {
        Error::Pty(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Pty(e.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
