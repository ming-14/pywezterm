//! 网格单元 —— 全 crate 的通用 IR。
//!
//! `render` / `host` 都只认识这里的类型。颜色是**枚举**而不是字符串约定：
//! 字符串形式（`default` / `pN` / `#rrggbb`）只出现在 Python 边界，见 `pywezterm` 的
//! `py::convert`。

use wezterm_term::color::ColorAttribute;
use wezterm_term::{CellAttributes, Intensity, Line, Underline};

/// 默认 ANSI 16 色，与 wezterm-term 默认调色板一致。
const ANSI_16: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00),
    (0xcd, 0x00, 0x00),
    (0x00, 0xcd, 0x00),
    (0xcd, 0xcd, 0x00),
    (0x00, 0x00, 0xee),
    (0xcd, 0x00, 0xcd),
    (0x00, 0xcd, 0xcd),
    (0xe5, 0xe5, 0xe5),
    (0x7f, 0x7f, 0x7f),
    (0xff, 0x00, 0x00),
    (0x00, 0xff, 0x00),
    (0xff, 0xff, 0x00),
    (0x5c, 0x5c, 0xff),
    (0xff, 0x00, 0xff),
    (0x00, 0xff, 0xff),
    (0xff, 0xff, 0xff),
];

/// 256 色调色板索引 → RGB。
///
/// 0–15 是 ANSI 16 色；16–231 是 6×6×6 色立方；232–255 是 24 级灰阶。
pub fn palette_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => ANSI_16[index as usize],
        16..=231 => {
            const LEVELS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
            let n = index - 16;
            (
                LEVELS[(n / 36) as usize],
                LEVELS[((n / 6) % 6) as usize],
                LEVELS[(n % 6) as usize],
            )
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    }
}

/// 颜色。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Default,
    Palette(u8),
    Rgb(u8, u8, u8),
}

impl Default for Color {
    fn default() -> Self {
        Color::Default
    }
}

impl Color {
    /// 从模型属性转换。真彩按 sRGB 取整，忽略调色板回退项。
    pub fn from_attr(attr: ColorAttribute) -> Self {
        match attr {
            ColorAttribute::Default => Color::Default,
            ColorAttribute::PaletteIndex(i) => Color::Palette(i),
            ColorAttribute::TrueColorWithDefaultFallback(c)
            | ColorAttribute::TrueColorWithPaletteFallback(c, _) => {
                let (r, g, b, _) = c.to_srgb_u8();
                Color::Rgb(r, g, b)
            }
        }
    }

    /// 从 Python 边界字符串解析。
    ///
    /// 接受 `default` / 空串、`p<N>`、`#rrggbb`、裸 6 位 hex、以及 ANSI 命名色
    /// （`black` … `brightwhite`，兼容旧数据）。无法识别时按默认色处理。
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s.is_empty() || s == "default" {
            return Color::Default;
        }
        if let Some(idx) = s.strip_prefix('p') {
            if let Ok(i) = idx.parse::<u8>() {
                return Color::Palette(i);
            }
        }
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() == 6 {
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                return Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
            }
        }
        match s.to_ascii_lowercase().as_str() {
            "black" => Color::Palette(0),
            "red" => Color::Palette(1),
            "green" => Color::Palette(2),
            "brown" => Color::Palette(3),
            "blue" => Color::Palette(4),
            "magenta" => Color::Palette(5),
            "cyan" => Color::Palette(6),
            "white" => Color::Palette(7),
            "brightblack" => Color::Palette(8),
            "brightred" => Color::Palette(9),
            "brightgreen" => Color::Palette(10),
            "brightbrown" => Color::Palette(11),
            "brightblue" => Color::Palette(12),
            "brightmagenta" => Color::Palette(13),
            "brightcyan" => Color::Palette(14),
            "brightwhite" => Color::Palette(15),
            _ => Color::Default,
        }
    }

    /// 渲染用 RGB；`None` 表示使用调用方的默认色。
    pub fn to_rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            Color::Default => None,
            Color::Palette(i) => Some(palette_rgb(i)),
            Color::Rgb(r, g, b) => Some((r, g, b)),
        }
    }

    /// 回传给 Python 的字符串形式。
    pub fn to_python(self) -> String {
        match self {
            Color::Default => "default".to_string(),
            Color::Palette(i) => format!("p{i}"),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    /// 从模型属性转成 `ColorAttribute`（反向构造用）。
    pub fn to_attr(self) -> ColorAttribute {
        match self {
            Color::Default => ColorAttribute::Default,
            Color::Palette(i) => ColorAttribute::PaletteIndex(i),
            Color::Rgb(r, g, b) => ColorAttribute::TrueColorWithDefaultFallback(
                wezterm_term::color::RgbColor::new_8bpc(r, g, b).to_tuple_rgba(),
            ),
        }
    }
}

/// 字符样式。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub strikethrough: bool,
}

impl Attrs {
    pub fn from_attrs(a: &CellAttributes) -> Self {
        Self {
            bold: a.intensity() == Intensity::Bold,
            italic: a.italic(),
            underline: a.underline() != Underline::None,
            reverse: a.reverse(),
            strikethrough: a.strikethrough(),
        }
    }

    /// 全部为默认。
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// 可见网格中的一个单元格。
#[derive(Clone, Debug)]
pub struct Cell {
    /// 列号。宽字符的续格不出现在网格里，因此列号可能跳位。
    pub col: usize,
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    /// 显示宽度：CJK / emoji 为 2，其余为 1。
    pub width: u8,
}

impl Cell {
    /// 无字符且无样式 —— 渲染剪枝用。
    pub fn is_blank(&self) -> bool {
        (self.text.is_empty() || self.text == " ")
            && self.fg == Color::Default
            && self.bg == Color::Default
            && self.attrs.is_default()
    }
}

/// 物理行 → 可见单元格列表。
pub fn cells_of_line(line: &Line) -> Vec<Cell> {
    line.visible_cells()
        .map(|cell| {
            let attrs = cell.attrs();
            Cell {
                col: cell.cell_index(),
                text: cell.str().to_string(),
                fg: Color::from_attr(attrs.foreground()),
                bg: Color::from_attr(attrs.background()),
                attrs: Attrs::from_attrs(attrs),
                width: cell.width() as u8,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_covers_all_256() {
        assert_eq!(palette_rgb(0), (0, 0, 0));
        assert_eq!(palette_rgb(15), (0xff, 0xff, 0xff));
        assert_eq!(palette_rgb(16), (0, 0, 0));
        assert_eq!(palette_rgb(231), (0xff, 0xff, 0xff));
        assert_eq!(palette_rgb(232), (8, 8, 8));
        assert_eq!(palette_rgb(255), (238, 238, 238));
    }

    #[test]
    fn parse_accepts_all_documented_forms() {
        assert_eq!(Color::parse("default"), Color::Default);
        assert_eq!(Color::parse(""), Color::Default);
        assert_eq!(Color::parse("p7"), Color::Palette(7));
        assert_eq!(Color::parse("#ff0000"), Color::Rgb(0xff, 0, 0));
        assert_eq!(Color::parse("00ff00"), Color::Rgb(0, 0xff, 0));
        assert_eq!(Color::parse("red"), Color::Palette(1));
        assert_eq!(Color::parse("brightwhite"), Color::Palette(15));
        assert_eq!(Color::parse("bogus"), Color::Default);
        assert_eq!(Color::parse("px"), Color::Default);
    }

    #[test]
    fn python_roundtrip() {
        for c in [
            Color::Default,
            Color::Palette(0),
            Color::Palette(255),
            Color::Rgb(1, 2, 3),
        ] {
            assert_eq!(Color::parse(&c.to_python()), c);
        }
    }

    #[test]
    fn blank_detection() {
        let blank = Cell {
            col: 0,
            text: " ".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            width: 1,
        };
        assert!(blank.is_blank());
        let styled = Cell {
            attrs: Attrs {
                bold: true,
                ..Default::default()
            },
            ..blank.clone()
        };
        assert!(!styled.is_blank());
    }
}
