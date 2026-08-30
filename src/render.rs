//! Pure pixel-buffer rendering for the lock screen: no windowing, camera, or
//! model code here, so it can be exercised directly (see
//! `examples/preview_render.rs`) without a display or webcam.

use crate::text;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LockStatus {
    Scanning,
    NoMatch,
    /// The enrolled face's embedding matched, but the anti-spoof check
    /// didn't -- e.g. a printed photo or a phone screen held up to the
    /// camera. Deliberately distinct from `NoMatch` so it reads as "that
    /// looks like a photo of the right person, not the right person" rather
    /// than "wrong face", which is a different (and more alarming) signal.
    SpoofSuspected,
    Matched,
}

/// The background stays this same neutral graphite/navy in every state --
/// matching the tray and app icon's palette -- rather than washing the
/// whole screen red or green. Only the accent (glow, brackets, spinner,
/// status text) shifts per status, which reads as a calm security product
/// giving a specific signal instead of a flashing alarm screen.
const BG_CENTER: (u8, u8, u8) = (34, 43, 63);
const BG_EDGE: (u8, u8, u8) = (11, 13, 20);
const ICON_FILL: (u8, u8, u8) = (232, 236, 246);
const CARD_FILL: (u8, u8, u8) = (58, 68, 92);
const CARD_BORDER: (u8, u8, u8) = (108, 122, 156);

struct Accent {
    glow: (u8, u8, u8),
    subtitle: &'static str,
}

const SCANNING: Accent = Accent {
    glow: (94, 150, 240),
    subtitle: "Yüz taranıyor…",
};
const NO_MATCH: Accent = Accent {
    glow: (255, 76, 76),
    subtitle: "Yüz tanınmadı",
};
const SPOOF_SUSPECTED: Accent = Accent {
    glow: (255, 176, 32),
    subtitle: "Fotoğraf/ekran algılandı",
};
const MATCHED: Accent = Accent {
    glow: (86, 224, 140),
    subtitle: "Hoş geldiniz",
};

/// Draws one frame of the lock screen into `buffer` (row-major, packed
/// 0x00RRGGBB per pixel). `t` is seconds since the lock screen opened (drives
/// idle animation); `status_t` is seconds since `status` last changed
/// (drives the "wrong face" flash/shake so it reads as a reaction).
pub fn render(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    status: LockStatus,
    t: f32,
    status_t: f32,
) {
    let accent = match status {
        LockStatus::Scanning => &SCANNING,
        LockStatus::NoMatch => &NO_MATCH,
        LockStatus::SpoofSuspected => &SPOOF_SUSPECTED,
        LockStatus::Matched => &MATCHED,
    };

    // A sharp, fast flash right when a mismatch is detected makes "wrong
    // face" read as an alert rather than just a color swap; it settles into
    // a slower, gentler pulse if the wrong face lingers in frame -- restrained
    // rather than a strobing alarm, since this can sit on screen for a while.
    // SpoofSuspected reuses the same shape at a calmer pace: it's a real
    // "hold on" signal, but not the sharper "wrong person" alarm NoMatch is.
    let pulse = match status {
        LockStatus::NoMatch => {
            let flash = (1.0 - status_t * 6.0).clamp(0.0, 1.0);
            let alarm = (t * 3.2).sin() * 0.5 + 0.5;
            (flash + alarm * (1.0 - flash) * 0.5).clamp(0.0, 1.0)
        }
        LockStatus::SpoofSuspected => (t * 2.0).sin() * 0.5 + 0.5,
        LockStatus::Scanning => (t * 1.2).sin() * 0.5 + 0.5,
        LockStatus::Matched => ((t * 3.0).sin() * 0.5 + 0.5).powf(0.5),
    };

    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_dist = (cx * cx + cy * cy).sqrt();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt() / max_dist;
            let mut color = lerp_color(BG_CENTER, BG_EDGE, d.clamp(0.0, 1.0));
            // Gentle brightness pulse layered on the gradient, stronger near
            // the center so it reads as the icon "breathing" rather than
            // the whole screen flickering.
            let boost = pulse * (1.0 - d).max(0.0) * 0.18;
            color = lerp_color(color, accent.glow, boost);
            // Cheap ordered dither (a per-pixel hash, not a real noise
            // texture) so the wide radial gradient doesn't band on displays
            // with limited color depth -- imperceptible as texture, but
            // removes the visible gradient rings a flat lerp leaves behind.
            let dither = ((x.wrapping_mul(374761393) ^ y.wrapping_mul(668265263)) & 0xff) as i32;
            let jitter = dither - 128;
            color = jitter_color(color, jitter / 96);
            buffer[(y * width + x) as usize] = pack_rgb(color);
        }
    }

    let scale = (width.min(height) as f32 / 480.0).max(1.0);
    // Icon sits slightly above true center so the status subtitle and the
    // bottom brand mark both have breathing room below it.
    let icon_cx = cx;
    let icon_cy = cy - 28.0 * scale;

    let shake_x = match status {
        LockStatus::NoMatch => {
            let decay = (1.0 - status_t * 2.5).clamp(0.0, 1.0);
            (status_t * 45.0).sin() * 10.0 * decay
        }
        LockStatus::SpoofSuspected => {
            let decay = (1.0 - status_t * 2.5).clamp(0.0, 1.0);
            (status_t * 45.0).sin() * 5.0 * decay
        }
        _ => 0.0,
    };

    draw_card(
        buffer,
        width,
        height,
        icon_cx + shake_x,
        icon_cy,
        scale,
        pulse,
        accent,
    );
    draw_viewfinder_brackets(
        buffer,
        width,
        height,
        icon_cx + shake_x,
        icon_cy,
        scale,
        status,
        t,
        accent,
    );

    if status == LockStatus::Scanning {
        draw_spinner(
            buffer,
            width,
            height,
            icon_cx,
            icon_cy,
            92.0 * scale,
            scale,
            accent.glow,
            t,
        );
    }

    draw_lock_icon(
        buffer,
        width,
        height,
        icon_cx + shake_x,
        icon_cy,
        scale,
        status,
        t,
    );

    draw_labels(
        buffer, width, height, cx, cy, scale, status, status_t, accent,
    );
}

/// Soft translucent rounded panel behind the padlock, giving the icon depth
/// against the flat gradient (a lightweight stand-in for a real glass-blur
/// effect, cheap enough to redraw at 30fps without a compute shader).
#[allow(clippy::too_many_arguments)]
fn draw_card(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    scale: f32,
    pulse: f32,
    accent: &Accent,
) {
    let half_w = 130.0 * scale;
    let half_h = 150.0 * scale;
    let radius = 36.0 * scale;
    let border_glow = 0.15 + pulse * 0.1;

    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let d = rounded_rect_sdf(px, py, cx, cy, half_w, half_h, radius);
            if d > 1.5 {
                continue;
            }
            let idx = (y * width + x) as usize;
            let bg = unpack_rgb(buffer[idx]);

            // Fill: a faint lightening over the background, fading out at
            // the very edge for a soft (not hard-cut) panel silhouette.
            let fill_coverage = (0.5 - d).clamp(0.0, 1.0) * 0.9;
            let mut out = lerp_color(bg, CARD_FILL, fill_coverage);

            // A thin brighter rim right at the border reads as glass edge
            // highlight; tinted slightly toward the accent so it echoes the
            // current status without repainting the whole panel.
            let rim = (1.0 - (d.abs() / 1.6)).clamp(0.0, 1.0);
            let rim_color = lerp_color(CARD_BORDER, accent.glow, 0.35);
            out = lerp_color(out, rim_color, rim * border_glow);

            buffer[idx] = pack_rgb(out);
        }
    }
}

/// Four corner brackets around the card, echoing the face-scan viewfinder
/// glyph used for the tray and app icon so the fullscreen lock screen reads
/// as the same product rather than a generic padlock screen. Bright and
/// tight to the card while scanning ("actively looking for a face"),
/// fading out once a face is confirmed matched (the search is over).
#[allow(clippy::too_many_arguments)]
fn draw_viewfinder_brackets(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    scale: f32,
    status: LockStatus,
    t: f32,
    accent: &Accent,
) {
    let opacity = match status {
        LockStatus::Scanning => 0.55 + ((t * 1.2).sin() * 0.5 + 0.5) * 0.25,
        LockStatus::NoMatch => 0.85,
        LockStatus::SpoofSuspected => 0.85,
        LockStatus::Matched => (1.0 - t * 2.0).clamp(0.0, 0.6),
    };
    if opacity <= 0.0 {
        return;
    }

    let half_w = 168.0 * scale;
    let half_h = 188.0 * scale;
    let arm = 30.0 * scale;
    let thickness = 3.0 * scale;

    for &(bx, by, dx, dy) in &[
        (cx - half_w, cy - half_h, 1.0f32, 1.0f32),
        (cx + half_w, cy - half_h, -1.0, 1.0),
        (cx - half_w, cy + half_h, 1.0, -1.0),
        (cx + half_w, cy + half_h, -1.0, -1.0),
    ] {
        draw_capsule(
            buffer,
            width,
            height,
            bx,
            by,
            bx + dx * arm,
            by,
            thickness,
            accent.glow,
            opacity,
        );
        draw_capsule(
            buffer,
            width,
            height,
            bx,
            by,
            bx,
            by + dy * arm,
            thickness,
            accent.glow,
            opacity,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_capsule(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
    half_w: f32,
    color: (u8, u8, u8),
    opacity: f32,
) {
    let min_x = (ax.min(bx) - half_w - 1.0).max(0.0) as u32;
    let max_x = (ax.max(bx) + half_w + 1.0).min(width as f32) as u32;
    let min_y = (ay.min(by) - half_w - 1.0).max(0.0) as u32;
    let max_y = (ay.max(by) + half_w + 1.0).min(height as f32) as u32;

    let (abx, aby) = (bx - ax, by - ay);
    let len2 = (abx * abx + aby * aby).max(1e-6);

    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let tt = (((px - ax) * abx + (py - ay) * aby) / len2).clamp(0.0, 1.0);
            let (qx, qy) = (ax + abx * tt, ay + aby * tt);
            let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt() - half_w;
            let coverage = (0.5 - d).clamp(0.0, 1.0) * opacity;
            if coverage <= 0.0 {
                continue;
            }
            let idx = (y * width + x) as usize;
            let bg = unpack_rgb(buffer[idx]);
            buffer[idx] = pack_rgb(lerp_color(bg, color, coverage));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_labels(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    scale: f32,
    status: LockStatus,
    status_t: f32,
    accent: &Accent,
) {
    // Status subtitle fades in quickly whenever it changes, rather than
    // popping instantly, so a rapid Scanning/NoMatch flicker (a face
    // passing briefly through frame) doesn't read as flashing text.
    let subtitle_opacity = (status_t * 4.0).clamp(0.0, 1.0) * 0.92;
    let subtitle_color = if status == LockStatus::Scanning {
        (198, 205, 222)
    } else {
        accent.glow
    };
    text::draw_text(
        buffer,
        width,
        height,
        accent.subtitle,
        cx,
        cy + 172.0 * scale,
        22.0 * scale,
        subtitle_color,
        subtitle_opacity,
        text::HAlign::Center,
    );

    // A small tracked-out brand mark anchored near the bottom of the
    // screen -- present but quiet, the way a lock screen's product name
    // sits out of the way of the actual unlock affordance.
    text::draw_text(
        buffer,
        width,
        height,
        "O H M Y L O C K",
        cx,
        height as f32 - 34.0 * scale,
        13.0 * scale,
        (150, 158, 178),
        0.7,
        text::HAlign::Center,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_lock_icon(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    scale: f32,
    status: LockStatus,
    t: f32,
) {
    let body_half_w = 62.0 * scale;
    let body_half_h = 48.0 * scale;
    let body_radius = 15.0 * scale;
    let body_cy = cy + 16.0 * scale;

    let shackle_r_outer = 40.0 * scale;
    let shackle_thick = 12.0 * scale;
    let shackle_cy = body_cy - body_half_h;
    // When unlocked, swing the shackle up and to the side like an open
    // padlock instead of just recoloring the same closed silhouette.
    let open_t = match status {
        LockStatus::Matched => (t * 4.0).min(1.0),
        _ => 0.0,
    };
    let shackle_cx = cx + open_t * 30.0 * scale;
    let shackle_cy = shackle_cy - open_t * 9.0 * scale;

    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let ring_d = ring_sdf(
                px,
                py,
                shackle_cx,
                shackle_cy,
                shackle_r_outer,
                shackle_thick,
            );
            let body_d =
                rounded_rect_sdf(px, py, cx, body_cy, body_half_w, body_half_h, body_radius);
            let d = ring_d.min(body_d);
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }

            let idx = (y * width + x) as usize;
            let bg = unpack_rgb(buffer[idx]);
            let mut out = lerp_color(bg, ICON_FILL, coverage);

            // Keyhole cutout: a small circle + slot punched through the
            // body, shaded toward the background color so it reads as a
            // hole rather than a flat decal.
            let hole_d = keyhole_sdf(px, py, cx, body_cy - 5.0 * scale, 8.0 * scale);
            let hole_coverage = (0.5 - hole_d).clamp(0.0, 1.0) * coverage;
            if hole_coverage > 0.0 {
                out = lerp_color(out, bg, hole_coverage);
            }

            buffer[idx] = pack_rgb(out);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_spinner(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    scale: f32,
    color: (u8, u8, u8),
    t: f32,
) {
    let thickness = 3.0 * scale;
    let rotation = t * 2.4;
    let span = std::f32::consts::PI * 0.6;

    let min_x = (cx - radius - thickness).max(0.0) as u32;
    let max_x = (cx + radius + thickness).min(width as f32) as u32;
    let min_y = (cy - radius - thickness).max(0.0) as u32;
    let max_y = (cy + radius + thickness).min(height as f32) as u32;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let band = (thickness / 2.0 - (dist - radius).abs()).clamp(0.0, 1.0);
            if band <= 0.0 {
                continue;
            }
            let mut angle = dy.atan2(dx) - rotation;
            angle = angle.rem_euclid(std::f32::consts::TAU);
            if angle > span {
                continue;
            }
            // Fade the tail of the arc so it looks like a comet, not a bar.
            let fade = 1.0 - (angle / span);
            let idx = (y * width + x) as usize;
            let bg = unpack_rgb(buffer[idx]);
            let out = lerp_color(bg, color, band * fade);
            buffer[idx] = pack_rgb(out);
        }
    }
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
    let outside = (qx.max(0.0)).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    outside + inside - radius
}

fn ring_sdf(px: f32, py: f32, cx: f32, cy: f32, outer_r: f32, thickness: f32) -> f32 {
    let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    let inner_r = outer_r - thickness;
    (dist - outer_r).max(inner_r - dist)
}

fn keyhole_sdf(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let circle = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r;
    let slot = rounded_rect_sdf(px, py, cx, cy + r * 1.6, r * 0.35, r * 1.1, r * 0.3);
    circle.min(slot)
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

fn jitter_color((r, g, b): (u8, u8, u8), j: i32) -> (u8, u8, u8) {
    let c = |v: u8| -> u8 { (v as i32 + j).clamp(0, 255) as u8 };
    (c(r), c(g), c(b))
}
