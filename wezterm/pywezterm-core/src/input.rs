//! 输入事件词汇表：平台层产出，终端层消费。
//!
//! 单独成模块是为了让 `platform` 与 `term` 互不依赖 —— 两边都只认识这里的类型。

/// 修饰键位，取值与 wezterm-input-types 一致（可直接喂给 `KeyModifiers::from_bits_truncate`）。
pub const MOD_SHIFT: u16 = 2;
pub const MOD_ALT: u16 = 4;
pub const MOD_CTRL: u16 = 8;

/// 鼠标事件类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseKind {
    Press,
    Release,
    Move,
}

/// 鼠标按钮。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    None,
}

/// 归一化后的输入事件。
#[derive(Clone, Debug)]
pub enum InputEvent {
    Key {
        /// pywezterm 键名（`Up` / `F1` / 单字符）
        key: String,
        mods: u16,
        down: bool,
    },
    Mouse {
        x: usize,
        y: usize,
        kind: MouseKind,
        button: MouseButton,
        mods: u16,
        /// 连续点击次数：1 单击、2 双击、3 三击
        clicks: u32,
    },
    Resize,
}
