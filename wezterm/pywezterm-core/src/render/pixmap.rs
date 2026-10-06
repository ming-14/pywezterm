//! 位图输出：背景层 + 字形层合成，编码为 PNG / JPEG / BMP。
//!
//! 两遍渲染（对齐 wezterm 的 layer 模型）：先画所有单元格的背景，再画字形与几何。
//! 字形按**基线**定位，descender 允许溢出到下一行背景之上 —— 这正是两遍的意义。
//!
//! 全部纯 Rust：tiny-skia 合成与 PNG 编码、fontdue 光栅化、image 编码 JPEG/BMP。

use tiny_skia::{Pixmap, PremultipliedColorU8, Transform};

use crate::error::{Error, Result};
use crate::term::grid::Cell;

use super::font::FontStack;
use super::{CELL_H, CELL_W, DEFAULT_BG, DEFAULT_FG};

/// 单边像素上限：超过它直接报错，而不是让 Pixmap 分配失败后 panic。
const MAX_DIM: u32 = 16_384;

/// 渲染可见网格为图片字节。`fmt` 取 `png` / `jpg` / `jpeg` / `bmp`。
pub fn render_image(
    lines: &[Vec<Cell>],
    cols: usize,
    rows: usize,
    scale: f64,
    fmt: &str,
) -> Result<Vec<u8>> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(Error::Invalid(format!("scale 非法: {scale}")));
    }
    let (img_w, img_h) = (cols as f64 * CELL_W as f64 * scale, rows as f64 * CELL_H as f64 * scale);
    if img_w < 1.0 || img_h < 1.0 || img_w > MAX_DIM as f64 || img_h > MAX_DIM as f64 {
        return Err(Error::Invalid(format!(
            "渲染尺寸超出上限: {img_w:.0}x{img_h:.0}（上限 {MAX_DIM}）"
        )));
    }
    let (img_w, img_h) = (img_w as u32, img_h as u32);
    let cell_w = (CELL_W as f64 * scale) as u32;
    let cell_h = (CELL_H as f64 * scale) as u32;

    let mut pix = Pixmap::new(img_w, img_h)
        .ok_or_else(|| Error::Render(format!("无法分配 {img_w}x{img_h} 画布")))?;
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(DEFAULT_BG.0, DEFAULT_BG.1, DEFAULT_BG.2, 255);
    pix.fill_rect(
        tiny_skia::Rect::from_xywh(0.0, 0.0, img_w as f32, img_h as f32).unwrap(),
        &paint,
        Transform::identity(),
        None,
    );

    let stack = FontStack::global();
    let font_size = stack.font_size_for_cell(cell_h as f64);
    let descent = stack.descent(font_size as f32);

    for (y, line) in lines.iter().enumerate().take(rows) {
        let yp = (y as f64 * cell_h as f64) as i32;
        for cell in line {
            if cell.text.is_empty() {
                continue;
            }
            let (xp, w) = geometry(cell, cell_w);
            let (fg, bg) = cell_colors(cell);

            if let Some(bg) = bg {
                paint.set_color_rgba8(bg.0, bg.1, bg.2, 255);
                pix.fill_rect(
                    tiny_skia::Rect::from_xywh(xp as f32, yp as f32, w as f32, cell_h as f32)
                        .unwrap(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }

            let ch = cell.text.chars().next().unwrap_or(' ');
            if is_block_element(ch) {
                draw_block_element(&mut pix, ch, xp, yp, w as i32, cell_h as i32, fg);
            } else {
                blit_glyph(&mut pix, stack, ch, font_size, xp, yp, cell_h as i32, descent, fg, cell.attrs.bold);
            }

            let thickness = (scale.max(1.0)) as f32;
            if cell.attrs.underline {
                paint.set_color_rgba8(fg.0, fg.1, fg.2, 255);
                fill(&mut pix, &paint, xp as f32, (yp + cell_h as i32 - 2).max(yp) as f32, w as f32, thickness);
            }
            if cell.attrs.strikethrough {
                paint.set_color_rgba8(fg.0, fg.1, fg.2, 255);
                fill(&mut pix, &paint, xp as f32, (yp + cell_h as i32 / 2) as f32, w as f32, thickness);
            }
        }
    }

    encode(&pix, fmt)
}

/// 单元格的像素位置与宽度。
fn geometry(cell: &Cell, cell_w: u32) -> (i32, u32) {
    (
        (cell.col as f64 * cell_w as f64) as i32,
        cell.width.max(1) as u32 * cell_w,
    )
}

/// `(前景, 背景)`；`reverse` 时两者互换。
fn cell_colors(cell: &Cell) -> ((u8, u8, u8), Option<(u8, u8, u8)>) {
    let fg = cell.fg.to_rgb().unwrap_or(DEFAULT_FG);
    let bg = cell.bg.to_rgb();
    if cell.attrs.reverse {
        (bg.unwrap_or(DEFAULT_BG), Some(fg))
    } else {
        (fg, bg)
    }
}

fn fill(pix: &mut Pixmap, paint: &tiny_skia::Paint, x: f32, y: f32, w: f32, h: f32) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        pix.fill_rect(rect, paint, Transform::identity(), None);
    }
}

/// block elements（U+2580–U+259F）按 cell 尺寸几何填充，不用字体字形 ——
/// 字形高度往往超过 cell，会越界覆盖相邻行。
fn is_block_element(ch: char) -> bool {
    (0x2580..=0x259F).contains(&(ch as u32))
}

/// 按 f32 坐标填充矩形；关闭抗锯齿以保证相邻 cell 像素精确衔接、无缝连续。
fn fill_rect_f(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgb: (u8, u8, u8), alpha: u8) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, alpha);
    paint.anti_alias = false;
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        pix.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// block elements 几何绘制（1/8 网格）。
///
/// `░▒▓` 按整格填充 + alpha（25%/50%/75%），不是点阵 —— 这样相邻 cell 无缝连续，
/// 才是进度条该有的语义。
fn draw_block_element(pix: &mut Pixmap, ch: char, x: i32, y: i32, w: i32, h: i32, rgb: (u8, u8, u8)) {
    let (fw, fh) = (w as f32, h as f32);
    let (x8, y8) = (fw / 8.0, fh / 8.0);
    let (fx, fy) = (x as f32, y as f32);
    let solid = |pix: &mut Pixmap, x0: f32, x1: f32, y0: f32, y1: f32| {
        fill_rect_f(pix, fx + x0, fy + y0, x1 - x0, y1 - y0, rgb, 255);
    };
    let shade = |pix: &mut Pixmap, alpha: u8| {
        fill_rect_f(pix, fx, fy, fw, fh, rgb, alpha);
    };

    match ch as u32 {
        // 上半块族（1..8 个八分之一）
        0x2580 => solid(pix, 0.0, fw, 0.0, 4.0 * y8),
        0x2581 => solid(pix, 0.0, fw, fh - y8, fh),
        0x2582 => solid(pix, 0.0, fw, fh - 2.0 * y8, fh),
        0x2583 => solid(pix, 0.0, fw, fh - 3.0 * y8, fh),
        0x2584 => solid(pix, 0.0, fw, fh - 4.0 * y8, fh),
        0x2585 => solid(pix, 0.0, fw, fh - 5.0 * y8, fh),
        0x2586 => solid(pix, 0.0, fw, fh - 6.0 * y8, fh),
        0x2587 => solid(pix, 0.0, fw, fh - 7.0 * y8, fh),
        0x2588 => solid(pix, 0.0, fw, 0.0, fh),
        // 左块族
        0x2589 => solid(pix, 0.0, 7.0 * x8, 0.0, fh),
        0x258a => solid(pix, 0.0, 6.0 * x8, 0.0, fh),
        0x258b => solid(pix, 0.0, 5.0 * x8, 0.0, fh),
        0x258c => solid(pix, 0.0, 4.0 * x8, 0.0, fh),
        0x258d => solid(pix, 0.0, 3.0 * x8, 0.0, fh),
        0x258e => solid(pix, 0.0, 2.0 * x8, 0.0, fh),
        0x258f => solid(pix, 0.0, x8, 0.0, fh),
        // 右半块 / 上 1/8 / 右 1/8
        0x2590 => solid(pix, 4.0 * x8, fw, 0.0, fh),
        0x2594 => solid(pix, 0.0, fw, 0.0, y8),
        0x2595 => solid(pix, fw - x8, fw, 0.0, fh),
        // 阴影：整格 + alpha
        0x2591 => shade(pix, 64),
        0x2592 => shade(pix, 128),
        0x2593 => shade(pix, 192),
        // 象限块（2×2）
        0x2596 => solid(pix, 0.0, 4.0 * x8, 4.0 * y8, fh),
        0x2597 => solid(pix, 4.0 * x8, fw, 4.0 * y8, fh),
        0x2598 => solid(pix, 0.0, 4.0 * x8, 0.0, 4.0 * y8),
        0x2599 => {
            solid(pix, 0.0, 4.0 * x8, 0.0, 4.0 * y8);
            solid(pix, 0.0, 4.0 * x8, 4.0 * y8, fh);
            solid(pix, 4.0 * x8, fw, 4.0 * y8, fh);
        }
        0x259a => {
            solid(pix, 0.0, 4.0 * x8, 0.0, 4.0 * y8);
            solid(pix, 4.0 * x8, fw, 4.0 * y8, fh);
        }
        0x259b => {
            solid(pix, 0.0, 4.0 * x8, 0.0, 4.0 * y8);
            solid(pix, 4.0 * x8, fw, 0.0, 4.0 * y8);
            solid(pix, 0.0, 4.0 * x8, 4.0 * y8, fh);
        }
        0x259c => {
            solid(pix, 0.0, 4.0 * x8, 0.0, 4.0 * y8);
            solid(pix, 4.0 * x8, fw, 0.0, 4.0 * y8);
            solid(pix, 4.0 * x8, fw, 4.0 * y8, fh);
        }
        0x259d => solid(pix, 4.0 * x8, fw, 0.0, 4.0 * y8),
        0x259e => {
            solid(pix, 4.0 * x8, fw, 0.0, 4.0 * y8);
            solid(pix, 0.0, 4.0 * x8, 4.0 * y8, fh);
        }
        0x259f => {
            solid(pix, 4.0 * x8, fw, 0.0, 4.0 * y8);
            solid(pix, 0.0, 4.0 * x8, 4.0 * y8, fh);
            solid(pix, 4.0 * x8, fw, 4.0 * y8, fh);
        }
        // 未知 block：按全块处理（保守）
        _ => solid(pix, 0.0, fw, 0.0, fh),
    }
}

/// 把字形 blit 到 pixmap。
///
/// 基线 = cell 顶 + cell 高 + descent；位图顶行 = 基线 − (`ymin` + 高度)。
/// 只裁剪到整图边界，不裁剪到 cell —— 保留 descender 的视觉。
#[allow(clippy::too_many_arguments)]
fn blit_glyph(
    pix: &mut Pixmap,
    stack: &FontStack,
    ch: char,
    font_size: f64,
    xp: i32,
    yp: i32,
    cell_h: i32,
    descent: f64,
    fg: (u8, u8, u8),
    bold: bool,
) {
    let Some((metrics, coverage, _)) = stack.rasterize(ch, font_size as f32) else {
        return;
    };
    let (gw, gh) = (metrics.width as i32, metrics.height as i32);
    if gw <= 0 || gh <= 0 {
        return;
    }

    let base_x = xp + metrics.xmin;
    let base_y = yp + cell_h + descent.round() as i32 - metrics.ymin - gh;
    let (img_w, img_h) = (pix.width() as i32, pix.height() as i32);

    for py in 0..gh {
        let dy = base_y + py;
        if dy < 0 || dy >= img_h {
            continue;
        }
        for px in 0..gw {
            let alpha = coverage[(py * gw + px) as usize];
            if alpha == 0 {
                continue;
            }
            let dx = base_x + px;
            if dx >= 0 && dx < img_w {
                blend_pixel(pix, dx, dy, fg, alpha);
            }
            // 粗体：向右加一像素
            if bold && dx + 1 < img_w {
                blend_pixel(pix, dx + 1, dy, fg, alpha);
            }
        }
    }
}

/// 单像素 SourceOver 混合（预乘域）：`out = src + dst * (1 - a)`。
fn blend_pixel(pix: &mut Pixmap, dx: i32, dy: i32, fg: (u8, u8, u8), alpha: u8) {
    let idx = (dy as u32 * pix.width() + dx as u32) as usize;
    if alpha == 255 {
        pix.pixels_mut()[idx] = PremultipliedColorU8::from_rgba(fg.0, fg.1, fg.2, 255).unwrap();
        return;
    }
    let dst = pix.pixels_mut()[idx];
    let src = |c: u8| (c as u32 * alpha as u32) / 255;
    let keep = 255 - alpha as u32;
    let mix = |s: u32, d: u8| (s + d as u32 * keep / 255).min(255) as u8;
    pix.pixels_mut()[idx] = PremultipliedColorU8::from_rgba(
        mix(src(fg.0), dst.red()),
        mix(src(fg.1), dst.green()),
        mix(src(fg.2), dst.blue()),
        mix(alpha as u32, dst.alpha()),
    )
    .unwrap();
}

/// 编码。PNG 走 tiny-skia；JPEG / BMP 不支持 alpha，先按黑底合成 RGB 再交给 image。
fn encode(pix: &Pixmap, fmt: &str) -> Result<Vec<u8>> {
    match fmt.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "bmp" => {
            let format = if fmt.eq_ignore_ascii_case("bmp") {
                image::ImageFormat::Bmp
            } else {
                image::ImageFormat::Jpeg
            };
            let (w, h) = (pix.width(), pix.height());
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for p in pix.pixels() {
                let c = p.demultiply();
                rgb.extend_from_slice(&[c.red(), c.green(), c.blue()]);
            }
            let img = image::RgbImage::from_raw(w, h, rgb)
                .ok_or_else(|| Error::Render("RGB 缓冲尺寸不匹配".into()))?;
            let mut out = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut out), format)
                .map_err(|e| Error::Render(format!("{fmt} 编码失败: {e}")))?;
            Ok(out)
        }
        _ => pix
            .encode_png()
            .map_err(|e| Error::Render(format!("PNG 编码失败: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::grid::{Attrs, Color};

    fn cell(col: usize, text: &str, fg: Color, bg: Color, attrs: Attrs) -> Cell {
        Cell {
            col,
            text: text.to_string(),
            fg,
            bg,
            attrs,
            width: text.chars().count().max(1) as u8,
        }
    }

    fn plain(text: &str) -> Vec<Vec<Cell>> {
        vec![vec![cell(0, text, Color::Palette(7), Color::Default, Attrs::default())]]
    }

    #[test]
    fn png_has_magic_and_scales() {
        let png = render_image(&plain("hi"), 4, 1, 1.0, "png").unwrap();
        assert_eq!(&png[..4], [0x89, b'P', b'N', b'G']);
        let big = render_image(&plain("hi"), 4, 1, 2.0, "png").unwrap();
        assert!(big.len() > png.len(), "2x 应产生更大图像");
    }

    #[test]
    fn jpg_and_bmp_magic() {
        let jpg = render_image(&plain("X"), 4, 1, 1.0, "jpg").unwrap();
        assert_eq!(&jpg[..2], [0xFF, 0xD8], "JPEG SOI");
        let bmp = render_image(&plain("X"), 4, 1, 1.0, "bmp").unwrap();
        assert_eq!(&bmp[..2], b"BM", "BMP magic");
    }

    #[test]
    fn unknown_format_falls_back_to_png() {
        let png = render_image(&plain("X"), 4, 1, 1.0, "tiff").unwrap();
        assert_eq!(&png[..4], [0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn invalid_scale_is_an_error_not_a_panic() {
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
            assert!(render_image(&plain("X"), 4, 1, scale, "png").is_err(), "scale={}", scale);
        }
    }

    #[test]
    fn reverse_swaps_colors() {
        let attrs = Attrs {
            reverse: true,
            ..Default::default()
        };
        let lines = vec![vec![cell(0, "A", Color::Palette(15), Color::Palette(1), attrs)]];
        assert_eq!(&render_image(&lines, 4, 1, 1.0, "png").unwrap()[..4], [0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn block_elements_and_wide_cells_render() {
        let lines = vec![
            vec![cell(0, "█", Color::Palette(2), Color::Default, Attrs::default())],
            vec![cell(0, "你", Color::Palette(7), Color::Default, Attrs::default())],
            vec![cell(0, "░", Color::Palette(4), Color::Default, Attrs::default())],
        ];
        assert!(render_image(&lines, 4, 3, 1.0, "png").is_ok());
    }

    #[test]
    fn empty_grid_renders() {
        assert!(render_image(&[], 4, 2, 1.0, "png").is_ok());
    }
}
