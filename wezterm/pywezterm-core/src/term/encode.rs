//! 键鼠编码：名字 → wezterm-term 输入类型。
//!
//! 编码本身是模式感知的，由模型完成；这里只做字符串解析与事件组装。

use wezterm_term::input::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::error::{Error, Result};

/// 解析键名。
///
/// 合法取值：方向键与编辑键名（`Up` `Down` `Left` `Right` `Home` `End` `Insert`
/// `Delete` `PageUp` `PageDown`）、`Backspace` `Tab` `Enter` `Esc` `Space`、
/// `F1`–`F24`，以及任意**单个**字符。
pub fn keycode(name: &str) -> Result<KeyCode> {
    use KeyCode::*;
    let code = match name {
        "Up" => UpArrow,
        "Down" => DownArrow,
        "Left" => LeftArrow,
        "Right" => RightArrow,
        "Home" => Home,
        "End" => End,
        "Insert" => Insert,
        "Delete" => Delete,
        "PageUp" => PageUp,
        "PageDown" => PageDown,
        "Backspace" => Backspace,
        "Tab" => Tab,
        "Enter" => Enter,
        "Esc" => Escape,
        "Space" => Char(' '),
        _ => match parse_function_key(name) {
            Some(n) => Function(n),
            None => {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Char(c),
                    _ => return Err(Error::Invalid(format!("无法解析按键: {name:?}"))),
                }
            }
        },
    };
    Ok(code)
}

/// `F1`–`F24` → 功能键号；其余为 `None`。
fn parse_function_key(name: &str) -> Option<u8> {
    let n = name.strip_prefix('F')?.parse::<u8>().ok()?;
    (1..=24).contains(&n).then_some(n)
}

/// 解析修饰键位。
pub fn modifiers(bits: u16) -> KeyModifiers {
    KeyModifiers::from_bits_truncate(bits)
}

/// 解析鼠标事件类型。
pub fn mouse_kind(name: &str) -> Result<MouseEventKind> {
    Ok(match name {
        "press" => MouseEventKind::Press,
        "release" => MouseEventKind::Release,
        "move" => MouseEventKind::Move,
        _ => return Err(Error::Invalid(format!("未知鼠标事件类型: {name:?}"))),
    })
}

/// 解析鼠标按钮。滚轮一次一格。
pub fn mouse_button(name: &str) -> Result<MouseButton> {
    Ok(match name {
        "left" => MouseButton::Left,
        "middle" => MouseButton::Middle,
        "right" => MouseButton::Right,
        "wheel_up" => MouseButton::WheelUp(1),
        "wheel_down" => MouseButton::WheelDown(1),
        "none" => MouseButton::None,
        _ => return Err(Error::Invalid(format!("未知鼠标按钮: {name:?}"))),
    })
}

/// 组装鼠标事件。像素偏移恒为 0：本库不掌握宿主的字体度量。
pub fn mouse_event(
    kind: MouseEventKind,
    x: usize,
    y: i64,
    button: MouseButton,
    mods: u16,
) -> MouseEvent {
    MouseEvent {
        kind,
        x,
        y,
        x_pixel_offset: 0,
        y_pixel_offset: 0,
        button,
        modifiers: modifiers(mods),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_and_function_keys() {
        assert_eq!(keycode("Up").unwrap(), KeyCode::UpArrow);
        assert_eq!(keycode("PageDown").unwrap(), KeyCode::PageDown);
        assert_eq!(keycode("F1").unwrap(), KeyCode::Function(1));
        assert_eq!(keycode("F24").unwrap(), KeyCode::Function(24));
        assert_eq!(keycode("Space").unwrap(), KeyCode::Char(' '));
    }

    #[test]
    fn parses_single_char_only() {
        assert_eq!(keycode("a").unwrap(), KeyCode::Char('a'));
        assert_eq!(keycode("中").unwrap(), KeyCode::Char('中'));
        // 多字符且非功能键名：不再悄悄取首字符，而是报错
        assert!(keycode("Foo").is_err());
        assert!(keycode("F25").is_err());
        assert!(keycode("").is_err());
    }

    #[test]
    fn parses_mouse() {
        assert_eq!(mouse_kind("press").unwrap(), MouseEventKind::Press);
        assert!(mouse_kind("click").is_err());
        assert_eq!(mouse_button("wheel_up").unwrap(), MouseButton::WheelUp(1));
        assert!(mouse_button("middle-x").is_err());
    }

    #[test]
    fn modifier_bits_match_wezterm() {
        assert_eq!(modifiers(0), KeyModifiers::NONE);
        assert_eq!(
            modifiers(crate::input::MOD_CTRL),
            KeyModifiers::CTRL
        );
    }
}
