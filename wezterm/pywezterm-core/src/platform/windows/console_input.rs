//! 宿主控制台输入采集（Win32）。
//!
//! 把「读控制台输入记录 → 归一化成平台无关的 [`InputEvent`]」整件事收在这里，
//! 调用方不需要接触任何 Win32 结构。
//!
//! 归一化约定：
//! - 键名走 pywezterm 键名（`Backspace` / `Up` / `F1`…），普通键取 Unicode 字符；
//! - Ctrl+字母在 Windows 上是控制码（0x01–0x1A），归一成字母并保留 CTRL 位；
//! - 修饰键自身（`VK_SHIFT` 等）的 `uChar` 是 0，必须丢弃，否则 NUL 会被下发；
//! - 抬键事件保留 `down = false`。

use winapi::shared::minwindef::{DWORD, WORD};
use winapi::um::consoleapi::{
    GetConsoleMode, GetConsoleOutputCP, GetNumberOfConsoleInputEvents, ReadConsoleInputW,
    SetConsoleMode,
};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::processenv::GetStdHandle;
use winapi::um::synchapi::WaitForSingleObject;
use winapi::um::winbase::{STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, WAIT_OBJECT_0};
use winapi::um::wincon::{
    CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN, DOUBLE_CLICK, ENABLE_EXTENDED_FLAGS,
    ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    ENABLE_WINDOW_INPUT, FROM_LEFT_1ST_BUTTON_PRESSED, FROM_LEFT_2ND_BUTTON_PRESSED,
    GetConsoleScreenBufferInfo, INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD, LEFT_ALT_PRESSED,
    LEFT_CTRL_PRESSED, MOUSE_EVENT, MOUSE_EVENT_RECORD, MOUSE_MOVED, MOUSE_WHEELED,
    RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED, RIGHTMOST_BUTTON_PRESSED, SHIFT_PRESSED,
    SetConsoleOutputCP, WINDOW_BUFFER_SIZE_EVENT,
};
use winapi::um::winnt::HANDLE;
use winapi::um::winuser::{
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT, VK_LEFT,
    VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_TAB, VK_UP,
};

use crate::error::{Error, Result};
use crate::input::{InputEvent, MouseButton, MouseKind, MOD_ALT, MOD_CTRL, MOD_SHIFT};

/// 输入侧模式：原始按键事件（不启用 VT 输入转换，避免按键被吞）+ 忽略快速编辑。
const INPUT_MODE: DWORD = ENABLE_EXTENDED_FLAGS | ENABLE_WINDOW_INPUT | ENABLE_MOUSE_INPUT;
/// 输出侧模式：VT 处理 + 禁用换行自动回车。
const OUTPUT_MODE: DWORD =
    ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN;
/// 输出代码页：渲染字节统一 UTF-8。
const CP_UTF8: u32 = 65001;

// ---- 归一化（纯函数，可单测）--------------------------------------------

/// Win32 `dwControlKeyState` → 修饰键位。
fn mods_from_control_state(state: DWORD) -> u16 {
    let mut mods = 0;
    if state & SHIFT_PRESSED != 0 {
        mods |= MOD_SHIFT;
    }
    if state & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
        mods |= MOD_ALT;
    }
    if state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        mods |= MOD_CTRL;
    }
    mods
}

/// 虚拟键码 → pywezterm 键名；`None` 表示该键走 Unicode 字符通道。
fn key_name_from_vk(vk: WORD) -> Option<String> {
    let name = match vk as i32 {
        VK_BACK => "Backspace",
        VK_TAB => "Tab",
        VK_RETURN => "Enter",
        VK_ESCAPE => "Esc",
        VK_PRIOR => "PageUp",
        VK_NEXT => "PageDown",
        VK_END => "End",
        VK_HOME => "Home",
        VK_LEFT => "Left",
        VK_UP => "Up",
        VK_RIGHT => "Right",
        VK_DOWN => "Down",
        VK_INSERT => "Insert",
        VK_DELETE => "Delete",
        _ if (VK_F1..=VK_F24).contains(&(vk as i32)) => {
            return Some(format!("F{}", vk as i32 - VK_F1 + 1))
        }
        _ => return None,
    };
    Some(name.to_string())
}

/// `KEY_EVENT_RECORD` → `(键名, 修饰键, 是否按下)`；无法表达的按键返回 `None`。
fn normalize_key(rec: &KEY_EVENT_RECORD) -> Option<(String, u16, bool)> {
    let mods = mods_from_control_state(rec.dwControlKeyState);
    let down = rec.bKeyDown != 0;
    if let Some(name) = key_name_from_vk(rec.wVirtualKeyCode) {
        return Some((name, mods, down));
    }
    let ch = char::from_u32(*unsafe { rec.uChar.UnicodeChar() } as u32)?;
    if ch == '\0' {
        return None;
    }
    if mods & MOD_CTRL != 0 && ('\x01'..='\x1a').contains(&ch) {
        return Some((char::from_u32(ch as u32 + 0x60)?.to_string(), mods, down));
    }
    Some((ch.to_string(), mods, down))
}

/// 按钮状态位 → 鼠标按钮。
fn mouse_button(state: DWORD) -> MouseButton {
    if state & FROM_LEFT_1ST_BUTTON_PRESSED != 0 {
        MouseButton::Left
    } else if state & RIGHTMOST_BUTTON_PRESSED != 0 {
        MouseButton::Right
    } else if state & FROM_LEFT_2ND_BUTTON_PRESSED != 0 {
        MouseButton::Middle
    } else {
        MouseButton::None
    }
}

/// 跨批次的鼠标状态。
#[derive(Clone, Copy)]
struct MouseState {
    /// 最后一次按下的按钮。Windows 的抬键事件按钮位为 0，不带被抬起的按钮号，
    /// 而编码 release 必须带按钮 —— 只能自己记住。
    last_pressed: MouseButton,
    /// 连续点击计数（双击 = 2、三击 = 3）。Windows 双击序列里后续 press 带
    /// `DOUBLE_CLICK` 标志，据此累加，替代宿主用时间窗口模拟。
    clicks: u32,
}

impl Default for MouseState {
    fn default() -> Self {
        Self {
            last_pressed: MouseButton::None,
            clicks: 1,
        }
    }
}

/// `MOUSE_EVENT_RECORD` → 归一化事件；同时更新跨批次状态。
fn normalize_mouse(rec: &MOUSE_EVENT_RECORD, st: &mut MouseState) -> InputEvent {
    let x = rec.dwMousePosition.X.max(0) as usize;
    let y = rec.dwMousePosition.Y.max(0) as usize;
    let mods = mods_from_control_state(rec.dwControlKeyState);
    let (flags, state) = (rec.dwEventFlags, rec.dwButtonState);

    let (kind, button, clicks) = if flags & MOUSE_WHEELED != 0 {
        // 高 16 位是带符号滚轮增量（±120），正上负下
        let delta = (state >> 16) as i16;
        let button = if delta > 0 {
            MouseButton::WheelUp
        } else {
            MouseButton::WheelDown
        };
        (MouseKind::Press, button, 1)
    } else if flags & MOUSE_MOVED != 0 {
        // 悬停为 None，按住移动（拖动）为对应按钮
        (MouseKind::Move, mouse_button(state), 1)
    } else if flags & DOUBLE_CLICK != 0 {
        // 双击/三击：系统已按时间与位置判定为连续点击，这里只累加
        let clicks = if st.clicks >= 1 { st.clicks + 1 } else { 2 };
        st.clicks = clicks;
        (MouseKind::Press, mouse_button(state), clicks)
    } else if state & 0x7 != 0 {
        let button = mouse_button(state);
        st.last_pressed = button;
        st.clicks = 1;
        (MouseKind::Press, button, 1)
    } else {
        let button = st.last_pressed;
        st.last_pressed = MouseButton::None;
        st.clicks = 1;
        (MouseKind::Release, button, 1)
    };

    InputEvent::Mouse {
        x,
        y,
        kind,
        button,
        mods,
        clicks,
    }
}

// ---- 控制台输入对象 ------------------------------------------------------

/// 宿主控制台输入采集器。
///
/// 构造即改写控制台模式（保存原模式）并把输出代码页切到 UTF-8；[`restore`](Self::restore)
/// 与 `Drop` 都会还原，且幂等。
pub struct ConsoleInput {
    hin: HANDLE,
    hout: HANDLE,
    orig_in: DWORD,
    orig_out: DWORD,
    orig_cp: u32,
    restored: bool,
    /// 跨批次保持的鼠标状态（抬键补全按钮、双击计数）。
    mouse: MouseState,
    /// `read_events` 的批量缓冲，跨调用复用。
    records: Vec<INPUT_RECORD>,
}

// 句柄是进程级资源，不随线程释放，可安全跨线程。
unsafe impl Send for ConsoleInput {}
unsafe impl Sync for ConsoleInput {}

impl ConsoleInput {
    /// 打开宿主控制台并设置模式。stdio 被重定向（非控制台）时失败。
    pub fn open() -> Result<Self> {
        unsafe {
            let hin = GetStdHandle(STD_INPUT_HANDLE);
            let hout = GetStdHandle(STD_OUTPUT_HANDLE);
            if hin.is_null()
                || hin == INVALID_HANDLE_VALUE
                || hout.is_null()
                || hout == INVALID_HANDLE_VALUE
            {
                return Err(Error::Platform("宿主控制台句柄不可用".into()));
            }
            let (mut orig_in, mut orig_out) = (0, 0);
            if GetConsoleMode(hin, &mut orig_in) == 0 || GetConsoleMode(hout, &mut orig_out) == 0 {
                return Err(Error::Platform(format!(
                    "读取控制台模式失败: {}",
                    std::io::Error::last_os_error()
                )));
            }
            // 先改输出再改输入：输入侧失败时要把输出侧还原，否则构造失败会留下
            // 一个被永久改掉模式的宿主控制台（对象不存在，Drop 不会跑）
            set_mode(hout, OUTPUT_MODE)?;
            if let Err(e) = set_mode(hin, INPUT_MODE) {
                let _ = set_mode(hout, orig_out);
                return Err(e);
            }
            let orig_cp = GetConsoleOutputCP();
            SetConsoleOutputCP(CP_UTF8);
            Ok(Self {
                hin,
                hout,
                orig_in,
                orig_out,
                orig_cp,
                restored: false,
                mouse: MouseState::default(),
                records: Vec::new(),
            })
        }
    }

    /// 等待输入事件，返回是否有事件（`false` = 超时）。
    pub fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.hin, ms) == WAIT_OBJECT_0 }
    }

    /// 读出全部待处理事件并归一化。非阻塞：`wait` 先行等待。
    pub fn read_events(&mut self) -> Result<Vec<InputEvent>> {
        let mut pending: DWORD = 0;
        if unsafe { GetNumberOfConsoleInputEvents(self.hin, &mut pending) } == 0 {
            return Err(Error::Platform(format!(
                "GetNumberOfConsoleInputEvents 失败: {}",
                std::io::Error::last_os_error()
            )));
        }
        if pending == 0 {
            return Ok(Vec::new());
        }
        self.records.clear();
        self.records
            .resize_with(pending as usize, || unsafe { std::mem::zeroed() });
        let mut read: DWORD = 0;
        if unsafe {
            ReadConsoleInputW(
                self.hin,
                self.records.as_mut_ptr(),
                pending,
                &mut read,
            )
        } == 0
        {
            return Err(Error::Platform(format!(
                "ReadConsoleInputW 失败: {}",
                std::io::Error::last_os_error()
            )));
        }

        let mut out = Vec::with_capacity(read as usize);
        for rec in &self.records[..read as usize] {
            match rec.EventType {
                KEY_EVENT => {
                    if let Some((key, mods, down)) = normalize_key(unsafe { rec.Event.KeyEvent() }) {
                        out.push(InputEvent::Key { key, mods, down });
                    }
                }
                MOUSE_EVENT => out.push(normalize_mouse(
                    unsafe { rec.Event.MouseEvent() },
                    &mut self.mouse,
                )),
                WINDOW_BUFFER_SIZE_EVENT => out.push(InputEvent::Resize),
                _ => {}
            }
        }
        Ok(out)
    }

    /// 当前窗口逻辑尺寸 `(cols, rows)`。
    pub fn size(&self) -> Result<(usize, usize)> {
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        if unsafe { GetConsoleScreenBufferInfo(self.hout, &mut info) } == 0 {
            return Err(Error::Platform(format!(
                "GetConsoleScreenBufferInfo 失败: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok((
            (info.srWindow.Right - info.srWindow.Left + 1) as usize,
            (info.srWindow.Bottom - info.srWindow.Top + 1) as usize,
        ))
    }

    /// 还原控制台原模式与代码页（幂等）。
    pub fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        unsafe { SetConsoleOutputCP(self.orig_cp) };
        set_mode(self.hout, self.orig_out)?;
        set_mode(self.hin, self.orig_in)
    }
}

impl Drop for ConsoleInput {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn set_mode(handle: HANDLE, mode: DWORD) -> Result<()> {
    if unsafe { SetConsoleMode(handle, mode) } == 0 {
        return Err(Error::Platform(format!(
            "SetConsoleMode 失败: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use winapi::shared::minwindef::BOOL;

    fn key_rec(down: bool, vk: i32, ch: char, state: DWORD) -> KEY_EVENT_RECORD {
        let mut rec: KEY_EVENT_RECORD = unsafe { std::mem::zeroed() };
        rec.bKeyDown = down as BOOL;
        rec.wVirtualKeyCode = vk as WORD;
        rec.dwControlKeyState = state;
        *unsafe { rec.uChar.UnicodeChar_mut() } = ch as u16;
        rec
    }

    fn mouse_rec(x: i16, y: i16, state: DWORD, flags: DWORD) -> MOUSE_EVENT_RECORD {
        let mut rec: MOUSE_EVENT_RECORD = unsafe { std::mem::zeroed() };
        rec.dwMousePosition.X = x;
        rec.dwMousePosition.Y = y;
        rec.dwButtonState = state;
        rec.dwEventFlags = flags;
        rec
    }

    fn mouse_fields(ev: &InputEvent) -> (usize, usize, MouseKind, MouseButton, u16, u32) {
        match ev {
            InputEvent::Mouse {
                x,
                y,
                kind,
                button,
                mods,
                clicks,
            } => (*x, *y, *kind, *button, *mods, *clicks),
            _ => panic!("不是鼠标事件"),
        }
    }

    #[test]
    fn mods_combination() {
        assert_eq!(mods_from_control_state(0), 0);
        assert_eq!(mods_from_control_state(SHIFT_PRESSED), MOD_SHIFT);
        assert_eq!(
            mods_from_control_state(LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED),
            MOD_CTRL | MOD_ALT
        );
        assert_eq!(
            mods_from_control_state(RIGHT_CTRL_PRESSED | RIGHT_ALT_PRESSED),
            MOD_CTRL | MOD_ALT
        );
    }

    #[test]
    fn key_name_mapping() {
        assert_eq!(key_name_from_vk(VK_UP as WORD).as_deref(), Some("Up"));
        assert_eq!(key_name_from_vk(VK_PRIOR as WORD).as_deref(), Some("PageUp"));
        assert_eq!(key_name_from_vk(VK_F1 as WORD).as_deref(), Some("F1"));
        assert_eq!(key_name_from_vk(VK_F24 as WORD).as_deref(), Some("F24"));
        assert_eq!(key_name_from_vk(0x41), None); // 'A' 走字符通道
    }

    #[test]
    fn key_char_and_ctrl_letter() {
        assert_eq!(
            normalize_key(&key_rec(true, 0x41, 'a', 0)),
            Some(("a".to_string(), 0, true))
        );
        // Ctrl+A：uChar 是控制码 0x01，应归一成 "a" + CTRL
        assert_eq!(
            normalize_key(&key_rec(true, 0x41, '\x01', LEFT_CTRL_PRESSED)),
            Some(("a".to_string(), MOD_CTRL, true))
        );
    }

    #[test]
    fn key_special_and_up_shape() {
        assert_eq!(
            normalize_key(&key_rec(true, VK_UP, '\0', SHIFT_PRESSED)),
            Some(("Up".to_string(), MOD_SHIFT, true))
        );
        assert_eq!(
            normalize_key(&key_rec(false, 0x41, 'a', 0)),
            Some(("a".to_string(), 0, false))
        );
    }

    #[test]
    fn key_modifier_alone_ignored() {
        // uChar = 0 的修饰键自身必须丢弃，否则 NUL 会被编码下发
        assert_eq!(normalize_key(&key_rec(true, 0x10, '\0', SHIFT_PRESSED)), None);
        assert_eq!(normalize_key(&key_rec(true, 0x11, '\0', LEFT_CTRL_PRESSED)), None);
    }

    #[test]
    fn mouse_press_records_button() {
        let mut st = MouseState::default();
        let ev = normalize_mouse(&mouse_rec(10, 5, FROM_LEFT_1ST_BUTTON_PRESSED, 0), &mut st);
        assert_eq!(
            mouse_fields(&ev),
            (10, 5, MouseKind::Press, MouseButton::Left, 0, 1)
        );
        assert_eq!(st.last_pressed, MouseButton::Left);
    }

    #[test]
    fn mouse_release_carries_last_pressed() {
        let mut st = MouseState {
            last_pressed: MouseButton::Left,
            clicks: 1,
        };
        let ev = normalize_mouse(&mouse_rec(10, 5, 0, 0), &mut st);
        assert_eq!(
            mouse_fields(&ev),
            (10, 5, MouseKind::Release, MouseButton::Left, 0, 1)
        );
        assert_eq!(st.last_pressed, MouseButton::None);
    }

    #[test]
    fn mouse_release_without_pressed_is_none() {
        let mut st = MouseState::default();
        let ev = normalize_mouse(&mouse_rec(10, 5, 0, 0), &mut st);
        assert_eq!(mouse_fields(&ev).3, MouseButton::None);
    }

    #[test]
    fn mouse_wheel_direction() {
        let mut st = MouseState::default();
        let up = normalize_mouse(&mouse_rec(10, 5, 120 << 16, MOUSE_WHEELED), &mut st);
        assert_eq!(mouse_fields(&up).3, MouseButton::WheelUp);
        let down = normalize_mouse(
            &mouse_rec(10, 5, ((-120i64) & 0xFFFF_FFFF) as u32, MOUSE_WHEELED),
            &mut st,
        );
        assert_eq!(mouse_fields(&down).3, MouseButton::WheelDown);
    }

    #[test]
    fn mouse_move_keeps_button_memory() {
        let mut st = MouseState {
            last_pressed: MouseButton::Left,
            clicks: 1,
        };
        let hover = normalize_mouse(&mouse_rec(3, 4, 0, MOUSE_MOVED), &mut st);
        assert_eq!(mouse_fields(&hover).3, MouseButton::None);
        assert_eq!(st.last_pressed, MouseButton::Left);
        let drag = normalize_mouse(
            &mouse_rec(3, 4, FROM_LEFT_1ST_BUTTON_PRESSED, MOUSE_MOVED),
            &mut st,
        );
        assert_eq!(mouse_fields(&drag).3, MouseButton::Left);
    }

    #[test]
    fn mouse_double_and_triple_click() {
        let mut st = MouseState::default();
        let first = normalize_mouse(&mouse_rec(10, 5, FROM_LEFT_1ST_BUTTON_PRESSED, 0), &mut st);
        assert_eq!(mouse_fields(&first).5, 1);
        let second = normalize_mouse(
            &mouse_rec(10, 5, FROM_LEFT_1ST_BUTTON_PRESSED, DOUBLE_CLICK),
            &mut st,
        );
        assert_eq!(mouse_fields(&second).5, 2);
        let third = normalize_mouse(
            &mouse_rec(10, 5, FROM_LEFT_1ST_BUTTON_PRESSED, DOUBLE_CLICK),
            &mut st,
        );
        assert_eq!(mouse_fields(&third).5, 3);
        // 新位置的普通 press 重置计数
        let fresh = normalize_mouse(&mouse_rec(20, 8, FROM_LEFT_1ST_BUTTON_PRESSED, 0), &mut st);
        assert_eq!(mouse_fields(&fresh).5, 1);
    }

    #[test]
    fn mouse_button_bits() {
        assert_eq!(mouse_button(RIGHTMOST_BUTTON_PRESSED), MouseButton::Right);
        assert_eq!(mouse_button(FROM_LEFT_2ND_BUTTON_PRESSED), MouseButton::Middle);
        assert_eq!(mouse_button(0), MouseButton::None);
    }
}
