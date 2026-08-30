//! Pure pixel-buffer rendering for the settings window, mirroring
//! `render.rs`'s split: no windowing or input-handling code here, just a
//! function from state to pixels. `render` also returns the hitboxes it
//! drew each control at, so `window.rs` hit-tests clicks against the exact
//! geometry that was rendered instead of keeping a second, driftable copy.

use crate::text::{self, HAlign};

/// Same graphite/navy the lock screen and app icon use, so the settings
/// window reads as the same product rather than a generic system dialog.
const BG_CENTER: (u8, u8, u8) = (34, 43, 63);
const BG_EDGE: (u8, u8, u8) = (16, 19, 28);
const TITLE_COLOR: (u8, u8, u8) = (236, 239, 247);
const LABEL_COLOR: (u8, u8, u8) = (222, 226, 236);
const SUBLABEL_COLOR: (u8, u8, u8) = (146, 154, 174);
const DIVIDER_COLOR: (u8, u8, u8) = (56, 65, 88);
const TOGGLE_ON: (u8, u8, u8) = (94, 150, 240);
const TOGGLE_OFF: (u8, u8, u8) = (72, 80, 102);
const KNOB_COLOR: (u8, u8, u8) = (245, 247, 251);
const BUTTON_IDLE: (u8, u8, u8) = (58, 68, 92);
const BUTTON_DONE: (u8, u8, u8) = (86, 224, 140);
const BUTTON_FAILED: (u8, u8, u8) = (255, 76, 76);

#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

#[derive(Clone)]
pub enum EnrollButtonState {
    Idle,
    /// Checking that the face currently in frame matches the face already
    /// enrolled, before any new samples are captured -- otherwise anyone
    /// who reaches this (already-unlocked) settings window could overwrite
    /// the enrolled face with their own and lock the real owner out.
    Verifying,
    InProgress { captured: u32, total: u32 },
    Done,
    Failed(String),
}

pub struct SettingsState {
    pub autostart: bool,
    pub antispoof_enabled: bool,
    pub lock_on_absence: bool,
    pub enroll: EnrollButtonState,
}

/// Where each interactive control ended up, in the same pixel space as
/// `buffer` -- used for click hit-testing.
pub struct Hitboxes {
    pub autostart_toggle: Rect,
    pub antispoof_toggle: Rect,
    pub lock_on_absence_toggle: Rect,
    pub reenroll_button: Rect,
}

const REFERENCE_WIDTH: f32 = 420.0;

pub fn render(buffer: &mut [u32], width: u32, height: u32, state: &SettingsState) -> Hitboxes {
    let scale = width as f32 / REFERENCE_WIDTH;
    let bg_cx = width as f32 / 2.0;
    let bg_cy = 0.0;
    let max_dist = ((width as f32).powi(2) + (height as f32).powi(2)).sqrt() / 2.0;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - bg_cx;
            let dy = y as f32 - bg_cy;
            let d = (dx * dx + dy * dy).sqrt() / max_dist;
            let color = lerp_color(BG_CENTER, BG_EDGE, d.clamp(0.0, 1.0));
            buffer[(y * width + x) as usize] = pack_rgb(color);
        }
    }

    let margin = 24.0 * scale;
    let content_w = width as f32 - margin * 2.0;

    text::draw_text(
        buffer,
        width,
        height,
        "Ayarlar",
        margin,
        44.0 * scale,
        24.0 * scale,
        TITLE_COLOR,
        1.0,
        HAlign::Left,
    );
    text::draw_text(
        buffer,
        width,
        height,
        "Kilit ekranı davranışını özelleştir",
        margin,
        66.0 * scale,
        13.0 * scale,
        SUBLABEL_COLOR,
        0.9,
        HAlign::Left,
    );

    draw_divider(buffer, width, margin, 90.0 * scale, content_w, scale);

    let mut y = 96.0 * scale;
    let autostart_toggle = draw_toggle_row(
        buffer,
        width,
        height,
        margin,
        y,
        content_w,
        scale,
        "Açılışta Başlat",
        "Oturum açılışında otomatik başlar",
        state.autostart,
    );
    y += 56.0 * scale;
    let antispoof_toggle = draw_toggle_row(
        buffer,
        width,
        height,
        margin,
        y,
        content_w,
        scale,
        "Canlılık tespiti",
        "Fotoğraf/ekran ile açılmaya karşı korur",
        state.antispoof_enabled,
    );
    y += 56.0 * scale;
    let lock_on_absence_toggle = draw_toggle_row(
        buffer,
        width,
        height,
        margin,
        y,
        content_w,
        scale,
        "Yüz görünmediğinde kilitle",
        "~10 sn kimse görünmezse otomatik kilitler",
        state.lock_on_absence,
    );
    y += 56.0 * scale;

    draw_divider(buffer, width, margin, y, content_w, scale);
    y += 24.0 * scale;

    text::draw_text(
        buffer,
        width,
        height,
        "Yüz Tanımlama",
        margin,
        y,
        15.0 * scale,
        LABEL_COLOR,
        1.0,
        HAlign::Left,
    );
    y += 14.0 * scale;

    let reenroll_button = draw_reenroll_button(
        buffer,
        width,
        height,
        margin,
        y,
        content_w,
        scale,
        &state.enroll,
    );

    Hitboxes {
        autostart_toggle,
        antispoof_toggle,
        lock_on_absence_toggle,
        reenroll_button,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_toggle_row(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    content_w: f32,
    scale: f32,
    label: &str,
    sublabel: &str,
    on: bool,
) -> Rect {
    let toggle_w = 44.0 * scale;
    let toggle_h = 24.0 * scale;
    let toggle_x = x + content_w - toggle_w;
    let toggle_y = y;

    text::draw_text(
        buffer,
        width,
        height,
        label,
        x,
        y + 16.0 * scale,
        16.0 * scale,
        LABEL_COLOR,
        1.0,
        HAlign::Left,
    );
    text::draw_text(
        buffer,
        width,
        height,
        sublabel,
        x,
        y + 34.0 * scale,
        12.0 * scale,
        SUBLABEL_COLOR,
        0.85,
        HAlign::Left,
    );

    draw_toggle(
        buffer, width, height, toggle_x, toggle_y, toggle_w, toggle_h, on,
    );

    Rect {
        x: toggle_x,
        y: toggle_y,
        w: toggle_w,
        h: toggle_h,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_toggle(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    on: bool,
) {
    let track_color = if on { TOGGLE_ON } else { TOGGLE_OFF };
    fill_rounded_rect(buffer, width, height, x, y, w, h, h / 2.0, track_color, 1.0);

    let knob_r = h / 2.0 - 3.0;
    let knob_cx = if on { x + w - h / 2.0 } else { x + h / 2.0 };
    let knob_cy = y + h / 2.0;
    fill_circle(
        buffer, width, height, knob_cx, knob_cy, knob_r, KNOB_COLOR, 1.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_reenroll_button(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    content_w: f32,
    scale: f32,
    enroll: &EnrollButtonState,
) -> Rect {
    let h = 44.0 * scale;
    let (fill, label): (_, String) = match enroll {
        EnrollButtonState::Idle => (BUTTON_IDLE, "Yüzü Yeniden Tanımla".to_string()),
        EnrollButtonState::Verifying => (BUTTON_IDLE, "Kimlik doğrulanıyor…".to_string()),
        EnrollButtonState::InProgress { captured, total } => {
            (BUTTON_IDLE, format!("Taranıyor… {captured}/{total}"))
        }
        EnrollButtonState::Done => (BUTTON_DONE, "Tamamlandı ✓".to_string()),
        EnrollButtonState::Failed(_) => (BUTTON_FAILED, "Başarısız, tekrar dene".to_string()),
    };

    fill_rounded_rect(
        buffer,
        width,
        height,
        x,
        y,
        content_w,
        h,
        10.0 * scale,
        fill,
        1.0,
    );
    let text_color = match enroll {
        EnrollButtonState::Idle
        | EnrollButtonState::Verifying
        | EnrollButtonState::InProgress { .. } => (232, 236, 246),
        EnrollButtonState::Done | EnrollButtonState::Failed(_) => (16, 19, 28),
    };
    text::draw_text(
        buffer,
        width,
        height,
        &label,
        x + content_w / 2.0,
        y + h / 2.0 + 6.0 * scale,
        15.0 * scale,
        text_color,
        1.0,
        HAlign::Center,
    );

    if let EnrollButtonState::Failed(reason) = enroll {
        text::draw_text(
            buffer,
            width,
            height,
            reason,
            x + content_w / 2.0,
            y + h + 20.0 * scale,
            11.0 * scale,
            BUTTON_FAILED,
            0.9,
            HAlign::Center,
        );
    }

    Rect {
        x,
        y,
        w: content_w,
        h,
    }
}

fn draw_divider(buffer: &mut [u32], width: u32, x: f32, y: f32, w: f32, scale: f32) {
    let thickness = (1.0 * scale).max(1.0);
    let x0 = x.max(0.0) as u32;
    let x1 = ((x + w) as u32).min(width);
    let y0 = y as u32;
    let y1 = (y0 as f32 + thickness).ceil() as u32;
    for yy in y0..y1.max(y0 + 1) {
        for xx in x0..x1 {
            let idx = (yy * width + xx) as usize;
            if idx < buffer.len() {
                buffer[idx] = pack_rgb(DIVIDER_COLOR);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_rounded_rect(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: (u8, u8, u8),
    opacity: f32,
) {
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let half_w = w / 2.0;
    let half_h = h / 2.0;
    let min_x = x.max(0.0) as u32;
    let max_x = (x + w).min(width as f32) as u32;
    let min_y = y.max(0.0) as u32;
    let max_y = (y + h).min(height as f32) as u32;

    for yy in min_y..max_y {
        for xx in min_x..max_x {
            let px = xx as f32 + 0.5;
            let py = yy as f32 + 0.5;
            let d = rounded_rect_sdf(px, py, cx, cy, half_w, half_h, radius);
            let coverage = (0.5 - d).clamp(0.0, 1.0) * opacity;
            if coverage <= 0.0 {
                continue;
            }
            let idx = (yy * width + xx) as usize;
            let bg = unpack_rgb(buffer[idx]);
            buffer[idx] = pack_rgb(lerp_color(bg, color, coverage));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fill_circle(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    r: f32,
    color: (u8, u8, u8),
    opacity: f32,
) {
    let min_x = (cx - r - 1.0).max(0.0) as u32;
    let max_x = (cx + r + 1.0).min(width as f32) as u32;
    let min_y = (cy - r - 1.0).max(0.0) as u32;
    let max_y = (cy + r + 1.0).min(height as f32) as u32;

    for yy in min_y..max_y {
        for xx in min_x..max_x {
            let px = xx as f32 + 0.5;
            let py = yy as f32 + 0.5;
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r;
            let coverage = (0.5 - d).clamp(0.0, 1.0) * opacity;
            if coverage <= 0.0 {
                continue;
            }
            let idx = (yy * width + xx) as usize;
            let bg = unpack_rgb(buffer[idx]);
            buffer[idx] = pack_rgb(lerp_color(bg, color, coverage));
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
