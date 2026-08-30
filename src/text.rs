//! Minimal text rasterization for the lock screen, so status copy and
//! branding can render straight into the same packed-RGB pixel buffer
//! `render.rs` already draws into -- no GPU text stack, no windowing-toolkit
//! label widgets, just glyph outlines rasterized on demand via `ab_glyph`.
//! Bundles DejaVu Sans (`assets/fonts/DejaVuSans.ttf`, permissively licensed,
//! full Turkish diacritic coverage) so this works identically on every
//! platform without depending on whatever fonts happen to be installed.

use ab_glyph::{Font, FontRef, Glyph, Point, PxScale, ScaleFont};
use std::sync::OnceLock;

static FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(FONT_BYTES).expect("bundled font is valid"))
}

pub enum HAlign {
    /// `x` is the horizontal center of the drawn text (status/brand copy on
    /// the lock screen).
    Center,
    /// `x` is the left edge of the drawn text (labels in the settings
    /// window, which sit in a left-aligned column next to right-aligned
    /// toggles).
    Left,
}

/// Draws `text` into `buffer` with its baseline at `y`, horizontally
/// positioned per `align`, alpha-blended over whatever is already there.
/// `size_px` is the font's ascent-to-descent scale; `color` and `opacity`
/// (0..=1) control the blend.
#[allow(clippy::too_many_arguments)]
pub fn draw_text(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    text: &str,
    x: f32,
    y: f32,
    size_px: f32,
    color: (u8, u8, u8),
    opacity: f32,
    align: HAlign,
) {
    if opacity <= 0.0 || text.is_empty() {
        return;
    }
    let font = font();
    let scale = PxScale::from(size_px);
    let scaled = font.as_scaled(scale);

    let mut cursor = match align {
        HAlign::Left => x,
        HAlign::Center => {
            let total_advance: f32 = text
                .chars()
                .map(|c| scaled.h_advance(font.glyph_id(c)))
                .sum();
            x - total_advance / 2.0
        }
    };
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let id = font.glyph_id(c);
        if let Some(prev_id) = prev {
            cursor += scaled.kern(prev_id, id);
        }
        let glyph = Glyph {
            id,
            scale,
            position: Point { x: cursor, y },
        };
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                    return;
                }
                let a = coverage * opacity;
                if a <= 0.0 {
                    return;
                }
                let idx = (py as u32 * width + px as u32) as usize;
                let bg = unpack_rgb(buffer[idx]);
                let out = lerp_color(bg, color, a);
                buffer[idx] = pack_rgb(out);
            });
        }
        cursor += scaled.h_advance(id);
        prev = Some(id);
    }
}

fn pack_rgb((r, g, b): (u8, u8, u8)) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn unpack_rgb(v: u32) -> (u8, u8, u8) {
    (
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    )
}

fn lerp_color(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let l = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
    (l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}
