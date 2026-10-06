//! 字体栈：系统字体发现、字形光栅化、按字形可用性回退。
//!
//! 回退判定依据是 `font.has_glyph(ch)`，**不能按 `is_ascii` 分类** —— box drawing
//! （`█▄▀` 等）落在 ASCII 范围里但主字体多半没有字形，按 ASCII 分类会被错配成 CJK
//! 双宽。同理，符号（`✔✘⚠`）要先于 CJK 尝试，否则会拿到一个双宽的替代字形。
//!
//! 只用系统字体：本库不随包分发字体，字体环境由宿主负责。

use std::sync::OnceLock;

use fontdb::{Database, Family, Query, Source};
use fontdue::{Font, FontSettings, Metrics};

/// 主字体候选（按序回退；字母序，无平台偏好 —— 各平台系统字体覆盖面不同）
const ASCII_FAMILIES: &[&str] = &["Cascadia Mono", "Consolas", "DejaVu Sans Mono", "Menlo"];
/// 符号字体候选（`✔✘⚠★` 等）
const SYMBOL_FAMILIES: &[&str] = &[
    "Apple Symbols",
    "DejaVu Sans",
    "Noto Sans Symbols",
    "Segoe UI Emoji",
    "Segoe UI Symbol",
    "Symbola",
];
/// CJK 字体候选（按序回退）
const CJK_FAMILIES: &[&str] = &[
    "DengXian",
    "Hiragino Sans GB",
    "Microsoft YaHei",
    "Noto Sans CJK SC",
    "PingFang SC",
    "SimHei",
    "SimSun",
    "WenQuanYi Micro Hei",
];

/// 字形缺失时的替代字符。
const REPLACEMENT: char = '\u{fffd}';

/// 字体栈（全局单例，懒初始化）。
pub struct FontStack {
    /// 主字体；系统无任何字体时为 `None`
    ascii: Option<Font>,
    symbol: Option<Font>,
    cjk: Option<Font>,
}

impl FontStack {
    fn new() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        let ascii = load_first(&db, ASCII_FAMILIES).or_else(|| load_any(&db));
        if ascii.is_none() {
            log::warn!("render: 系统未找到任何字体，字形将为空");
        }
        Self {
            ascii,
            symbol: load_first(&db, SYMBOL_FAMILIES),
            cjk: load_first(&db, CJK_FAMILIES),
        }
    }

    /// 全局字体栈。
    pub fn global() -> &'static FontStack {
        static STACK: OnceLock<FontStack> = OnceLock::new();
        STACK.get_or_init(FontStack::new)
    }

    /// 光栅化字符 → `(度量, 覆盖率, 显示宽度)`。
    ///
    /// 宽度按**实际命中的字体**返回：1 = 单宽，2 = 双宽。系统无字体时返回 `None`。
    pub fn rasterize(&self, ch: char, px: f32) -> Option<(Metrics, Vec<u8>, u8)> {
        if let Some(f) = &self.ascii {
            if f.has_glyph(ch) {
                let (m, c) = f.rasterize(ch, px);
                return Some((m, c, 1));
            }
        }
        if let Some(f) = &self.symbol {
            if f.has_glyph(ch) {
                let (m, c) = f.rasterize(ch, px);
                return Some((m, c, 1));
            }
        }
        if let Some(f) = &self.cjk {
            if f.has_glyph(ch) {
                let (m, c) = f.rasterize(ch, px);
                return Some((m, c, 2));
            }
        }
        self.ascii.as_ref().map(|f| {
            let (m, c) = f.rasterize(REPLACEMENT, px);
            (m, c, 1)
        })
    }

    /// 适配 cell 高度的字号。
    ///
    /// 按主字体的 line metrics 缩放：固定字号时 block 字形（`█`）会比 cell 高，越界覆盖
    /// 相邻行；按 line height 缩放才适配。
    pub fn font_size_for_cell(&self, cell_h: f64) -> f64 {
        const REF_PX: f32 = 15.0;
        if let Some(f) = &self.ascii {
            if let Some(lm) = f.horizontal_line_metrics(REF_PX) {
                let line_h = (lm.ascent - lm.descent).max(1.0) as f64;
                return cell_h * REF_PX as f64 / line_h;
            }
        }
        (cell_h - 2.0).max(1.0)
    }

    /// 主字体的 descent（负值），用于基线定位。
    pub fn descent(&self, px: f32) -> f64 {
        if let Some(f) = &self.ascii {
            if let Some(lm) = f.horizontal_line_metrics(px) {
                return lm.descent as f64;
            }
        }
        -(px as f64) / 4.0
    }
}

/// 按候选族列表加载第一个可用字体。
fn load_first(db: &Database, families: &[&str]) -> Option<Font> {
    families.iter().find_map(|name| load_family(db, name))
}

fn load_family(db: &Database, family: &str) -> Option<Font> {
    let id = db.query(&Query {
        families: &[Family::Name(family)],
        ..Default::default()
    })?;
    let data = read_font_data(&db.face(id)?.source)?;
    Font::from_bytes(data, FontSettings::default()).ok()
}

/// 兜底：取数据库里任意一个字体。
fn load_any(db: &Database) -> Option<Font> {
    let data = read_font_data(&db.faces().next()?.source)?;
    Font::from_bytes(data, FontSettings::default()).ok()
}

fn read_font_data(source: &Source) -> Option<Vec<u8>> {
    match source {
        Source::File(path) | Source::SharedFile(path, _) => std::fs::read(path).ok(),
        Source::Binary(data) => Some(data.as_ref().as_ref().to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_monospace_font() {
        let stack = FontStack::new();
        assert!(stack.ascii.is_some(), "系统未找到任何等宽字体");
        assert_eq!(stack.rasterize('A', 14.0).unwrap().2, 1, "ASCII 应为单宽");
    }

    /// box drawing 必须走主字体单宽 —— 错配到 CJK 会变成双宽，导致制表线错位。
    /// 主字体没有该字形时本断言不适用（回退字体的宽度由字体本身决定）。
    #[test]
    fn box_drawing_stays_single_width() {
        let stack = FontStack::new();
        let Some(ascii) = &stack.ascii else { return };
        if ascii.has_glyph('█') {
            assert_eq!(stack.rasterize('█', 14.0).unwrap().2, 1, "box drawing 错配成双宽");
        }
    }

    /// CJK 走双宽，符号不得被 CJK 抢走（否则符号会占两格）。
    #[test]
    fn cjk_is_double_width_and_symbols_are_not_stolen() {
        let stack = FontStack::new();
        if stack.cjk.is_some() {
            assert_eq!(stack.rasterize('你', 14.0).unwrap().2, 2, "CJK 应为双宽");
        }
        if stack.symbol.is_some() && !stack.ascii.as_ref().is_some_and(|f| f.has_glyph('\u{2714}')) {
            assert_eq!(stack.rasterize('\u{2714}', 14.0).unwrap().2, 1, "符号应单宽");
        }
    }

    #[test]
    fn font_size_fits_cell_height() {
        let stack = FontStack::new();
        let size = stack.font_size_for_cell(17.0);
        assert!((5.0..40.0).contains(&size), "字号不合常理: {}", size);
        assert!(stack.descent(size as f32) < 0.0, "descent 应为负");
    }
}
