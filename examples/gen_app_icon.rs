//! One-off generator for the app icon (`assets/AppIcon.png`, 1024x1024).
//! Run with `cargo run --release --example gen_app_icon`, then feed the PNG
//! through `scripts/package_macos.sh` (sips + iconutil) to produce an
//! `.icns`. Not part of the app itself -- there's no reason to regenerate
//! this at build time, only when the design changes.

use image::{Rgba, RgbaImage};

const SIZE: u32 = 1024;

// Same palette as the lock screen's "Scanning" theme (see src/render.rs),
// so the app icon and the lock screen read as the same product.
const BG_CENTER: (u8, u8, u8) = (36, 46, 68);
const BG_EDGE: (u8, u8, u8) = (14, 17, 26);
const GLOW: (u8, u8, u8) = (90, 140, 230);
const GLYPH: (u8, u8, u8) = (235, 239, 247);

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn lerp_color(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t))
}

fn rounded_rect_sdf(
    px: f32,
    py: f32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    radius: f32,
) -> f32 {
    let qx = (px - cx).abs() - (half_w - radius);
    let qy = (py - cy).abs() - (half_h - radius);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    outside + inside - radius
}

/// Coverage of a thick rounded line segment from `a` to `b`, `half_w` wide.
fn capsule_coverage(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, half_w: f32) -> f32 {
    let (abx, aby) = (bx - ax, by - ay);
    let len2 = (abx * abx + aby * aby).max(1e-6);
    let t = (((px - ax) * abx + (py - ay) * aby) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (ax + abx * t, ay + aby * t);
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - half_w;
    (0.5 - d).clamp(0.0, 1.0)
}

fn ring_coverage(px: f32, py: f32, cx: f32, cy: f32, rx: f32, ry: f32, thickness: f32) -> f32 {
    // Approximate an elliptical ring by comparing an ellipse "radius" metric
    // at two scales; good enough at this size for a clean, soft edge.
    let dx = (px - cx) / rx;
    let dy = (py - cy) / ry;
    let d = (dx * dx + dy * dy).sqrt();
    let t = thickness / ((rx + ry) / 2.0);
    let outer = (0.5 - (d - 1.0) * ((rx + ry) / 2.0)).clamp(0.0, 1.0);
    let inner = (0.5 - ((1.0 - t) - d) * ((rx + ry) / 2.0)).clamp(0.0, 1.0);
    outer.min(inner)
}

fn disc_coverage(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r;
    (0.5 - d).clamp(0.0, 1.0)
}

fn main() -> anyhow::Result<()> {
    let mut img = RgbaImage::new(SIZE, SIZE);
    let s = SIZE as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);

    // macOS "squircle" background: full-bleed rounded square, transparent
    // outside it, per Apple's icon template proportions.
    let pad = s * 0.03;
    let half = s / 2.0 - pad;
    let radius = half * 0.44;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let d = rounded_rect_sdf(px, py, cx, cy, half, half, radius);
            let bg_coverage = (0.5 - d).clamp(0.0, 1.0);
            if bg_coverage <= 0.0 {
                continue;
            }

            let dist_from_center = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() / (half * 1.3);
            let mut color = lerp_color(BG_CENTER, BG_EDGE, dist_from_center.clamp(0.0, 1.0));

            // Soft glow behind the glyph.
            let glow_d = (dist_from_center).clamp(0.0, 1.0);
            let glow_amount = (1.0 - glow_d).powf(2.2) * 0.55;
            color = lerp_color(color, GLOW, glow_amount);

            img.put_pixel(
                x,
                y,
                Rgba([color.0, color.1, color.2, (bg_coverage * 255.0) as u8]),
            );
        }
    }

    // Face-scan glyph: four corner brackets + a face ring + two eyes,
    // scaled up from the tray icon's design and anti-aliased.
    let bracket_len = s * 0.16;
    let bracket_half_w = s * 0.017;
    let inset = half * 0.32;
    let corners = [
        (cx - half + inset, cy - half + inset, 1.0, 1.0),
        (cx + half - inset, cy - half + inset, -1.0, 1.0),
        (cx - half + inset, cy + half - inset, 1.0, -1.0),
        (cx + half - inset, cy + half - inset, -1.0, -1.0),
    ];

    let (frx, fry) = (s * 0.15, s * 0.175);
    let ring_thickness = s * 0.028;
    let eye_r = s * 0.022;
    let eye_dx = s * 0.065;
    let eye_dy = s * 0.03;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let bg_d = rounded_rect_sdf(px, py, cx, cy, half, half, radius);
            if bg_d > 0.5 {
                continue;
            }

            let mut coverage = 0f32;
            for &(bx, by, dx, dy) in &corners {
                coverage = coverage.max(capsule_coverage(
                    px,
                    py,
                    bx,
                    by,
                    bx + dx * bracket_len,
                    by,
                    bracket_half_w,
                ));
                coverage = coverage.max(capsule_coverage(
                    px,
                    py,
                    bx,
                    by,
                    bx,
                    by + dy * bracket_len,
                    bracket_half_w,
                ));
            }
            coverage = coverage.max(ring_coverage(
                px,
                py,
                cx,
                cy + s * 0.01,
                frx,
                fry,
                ring_thickness,
            ));
            coverage = coverage.max(disc_coverage(px, py, cx - eye_dx, cy - eye_dy, eye_r));
            coverage = coverage.max(disc_coverage(px, py, cx + eye_dx, cy - eye_dy, eye_r));

            if coverage > 0.0 {
                let bg = img.get_pixel(x, y);
                let bg_color = (bg[0], bg[1], bg[2]);
                let out = lerp_color(bg_color, GLYPH, coverage);
                img.put_pixel(x, y, Rgba([out.0, out.1, out.2, 255]));
            }
        }
    }

    std::fs::create_dir_all("assets")?;
    img.save("assets/AppIcon.png")?;
    println!("wrote assets/AppIcon.png");
    Ok(())
}
