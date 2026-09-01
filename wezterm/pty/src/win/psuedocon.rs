//! Windows ConPTY 函数指针动态加载
//!
//! 通过 `LoadLibraryW`/`GetProcAddress` 动态加载 kernel32.dll（或侧载的
//! conpty.dll）中的 CreatePseudoConsole / ResizePseudoConsole /
//! ClosePseudoConsole，返回 `extern "system"` 函数指针。
//!
//! 为什么不用 `shared_library!` 宏：该宏生成的函数指针是 `extern "Rust"`
//! 调用约定，而 Win32 API 在 x86 上是 stdcall——两者不匹配会导致栈破坏
//! （32 位 segfault 的根因）。`extern "system"` 在 x86 上即 stdcall，
//! 在 x64 上即 C 约定，正确匹配 Win32 API。

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::{mem, ptr};

use anyhow::{bail, ensure, Error};
use filedescriptor::{FileDescriptor, OwnedHandle};
use lazy_static::lazy_static;
use winapi::shared::minwindef::DWORD;
use winapi::shared::winerror::{HRESULT, S_OK};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::libloaderapi::{FreeLibrary, GetProcAddress, LoadLibraryW};
use winapi::um::processthreadsapi::{CreateProcessW, PROCESS_INFORMATION};
use winapi::um::winbase::{
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};
use winapi::um::wincon::COORD;
use winapi::um::winnt::HANDLE;

// HMODULE = HANDLE，但 HMODULE 需要 winnt feature；直接用 HANDLE 等效
type HModule = HANDLE;

use std::ffi::OsString;
use std::io::Error as IoError;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::Mutex;

use crate::cmdbuilder::CommandBuilder;
use crate::win::procthreadattr::ProcThreadAttributeList;
use super::WinChild;

pub type HPCON = HANDLE;

pub const PSUEDOCONSOLE_INHERIT_CURSOR: DWORD = 0x1;
pub const PSEUDOCONSOLE_RESIZE_QUIRK: DWORD = 0x2;
pub const PSEUDOCONSOLE_WIN32_INPUT_MODE: DWORD = 0x4;
#[allow(dead_code)]
pub const PSEUDOCONSOLE_PASSTHROUGH_MODE: DWORD = 0x8;

type CreatePseudoConsoleFn =
    extern "system" fn(COORD, HANDLE, HANDLE, DWORD, *mut HPCON) -> HRESULT;
type ResizePseudoConsoleFn = extern "system" fn(HPCON, COORD) -> HRESULT;
type ClosePseudoConsoleFn = extern "system" fn(HPCON);

pub struct ConPtyFuncs {
    pub CreatePseudoConsole: CreatePseudoConsoleFn,
    pub ResizePseudoConsole: ResizePseudoConsoleFn,
    pub ClosePseudoConsole: ClosePseudoConsoleFn,
}

/// 从 DLL 中按名字解析一个导出函数指针
macro_rules! load_symbol {
    ($dll:expr, $name:literal) => {{
        let cname = concat!($name, "\0");
        let raw = unsafe { GetProcAddress($dll, cname.as_ptr() as *const i8) };
        if raw.is_null() {
            unsafe { FreeLibrary($dll) };
            return Err(());
        }
        unsafe { std::mem::transmute::<_, _>(raw) }
    }};
}

/// 加载指定 DLL 并解析三个 ConPTY 导出函数；任一缺失或加载失败返回 Err
fn load_conpty_impl(path: &Path) -> Result<ConPtyFuncs, ()> {
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let dll = unsafe { LoadLibraryW(path_wide.as_ptr()) };
    if dll.is_null() {
        return Err(());
    }

    Ok(ConPtyFuncs {
        CreatePseudoConsole: load_symbol!(dll, "CreatePseudoConsole"),
        ResizePseudoConsole: load_symbol!(dll, "ResizePseudoConsole"),
        ClosePseudoConsole: load_symbol!(dll, "ClosePseudoConsole"),
    })
}

fn load_conpty() -> ConPtyFuncs {
    // 优先：侧载目录下的 conpty.dll（上层指定）
    if let Some(dir) = crate::CONPTY_DIR.get() {
        let p = dir.join("conpty.dll");
        if let Ok(funcs) = load_conpty_impl(&p) {
            return funcs;
        }
    }
    // 次优：标准 DLL 搜索路径下的 conpty.dll（兼容 wezterm 自身部署）
    if let Ok(funcs) = load_conpty_impl(Path::new("conpty.dll")) {
        return funcs;
    }
    // 回退：系统 kernel32.dll（Windows 10 1809+ 均内置 ConPTY）
    load_conpty_impl(Path::new("kernel32.dll")).expect(
        "this system does not support conpty.  Windows 10 October 2018 or newer is required",
    )
}

lazy_static! {
    static ref CONPTY: ConPtyFuncs = load_conpty();
}

pub struct PsuedoCon {
    con: HPCON,
}

unsafe impl Send for PsuedoCon {}
unsafe impl Sync for PsuedoCon {}

impl Drop for PsuedoCon {
    fn drop(&mut self) {
        unsafe { (CONPTY.ClosePseudoConsole)(self.con) };
    }
}

impl PsuedoCon {
    pub fn new(size: COORD, input: FileDescriptor, output: FileDescriptor) -> Result<Self, Error> {
        let mut con: HPCON = INVALID_HANDLE_VALUE;
        let result = unsafe {
            (CONPTY.CreatePseudoConsole)(
                size,
                input.as_raw_handle() as _,
                output.as_raw_handle() as _,
                // 仅启用 WIN32_INPUT_MODE。不使用 PSEUDOCONSOLE_INHERIT_CURSOR：
                // 该 flag 在旧版 Windows（如 Win10 22H2）会使 ClosePseudoConsole
                // 因光标继承握手而无限挂起（microsoft/terminal#17716）。
                PSEUDOCONSOLE_WIN32_INPUT_MODE,
                &mut con,
            )
        };
        ensure!(
            result == S_OK,
            "failed to create psuedo console: HRESULT {}",
            result
        );
        Ok(Self { con })
    }

    pub fn resize(&self, size: COORD) -> Result<(), Error> {
        let result = unsafe { (CONPTY.ResizePseudoConsole)(self.con, size) };
        ensure!(
            result == S_OK,
            "failed to resize console to {}x{}: HRESULT: {}",
            size.X,
            size.Y,
            result
        );
        Ok(())
    }

    /// 暴露底层 HPCON 句柄，供外部进程通过 CreateProcess 附加到同一伪控制台
    /// （如沙箱场景由外部引擎用该 HPCON 启动受隔离进程）。
    pub fn hpcon(&self) -> HPCON {
        self.con
    }

    pub fn spawn_command(&self, cmd: CommandBuilder) -> anyhow::Result<WinChild> {
        let mut si: STARTUPINFOEXW = unsafe { mem::zeroed() };
        si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
        // Explicitly set the stdio handles as invalid handles otherwise
        // we can end up with a weird state where the spawned process can
        // inherit the explicitly redirected output handles from its parent.
        // For example, when daemonizing wezterm-mux-server, the stdio handles
        // are redirected to a log file and the spawned process would end up
        // writing its output there instead of to the pty we just created.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;

        let mut attrs = ProcThreadAttributeList::with_capacity(1)?;
        attrs.set_pty(self.con)?;
        si.lpAttributeList = attrs.as_mut_ptr();

        let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        let (mut exe, mut cmdline) = cmd.cmdline()?;
        let cmd_os = OsString::from_wide(&cmdline);

        let cwd = cmd.current_directory();

        let res = unsafe {
            CreateProcessW(
                exe.as_mut_slice().as_mut_ptr(),
                cmdline.as_mut_slice().as_mut_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                cmd.environment_block().as_mut_slice().as_mut_ptr() as *mut _,
                cwd.as_ref()
                    .map(|c| c.as_slice().as_ptr())
                    .unwrap_or(ptr::null()),
                &mut si.StartupInfo,
                &mut pi,
            )
        };
        if res == 0 {
            let err = IoError::last_os_error();
            let msg = format!(
                "CreateProcessW `{:?}` in cwd `{:?}` failed: {}",
                cmd_os,
                cwd.as_ref().map(|c| OsString::from_wide(c)),
                err
            );
            log::error!("{}", msg);
            bail!("{}", msg);
        }

        // Make sure we close out the thread handle so we don't leak it;
        // we do this simply by making it owned
        let _main_thread = unsafe { OwnedHandle::from_raw_handle(pi.hThread as _) };
        let proc = unsafe { OwnedHandle::from_raw_handle(pi.hProcess as _) };

        Ok(WinChild {
            proc: Mutex::new(proc),
        })
    }
}