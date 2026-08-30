#[path = "../src/settings_render.rs"]
mod settings_render;
#[path = "../src/text.rs"]
mod text;

use settings_render::{EnrollButtonState, SettingsState};

fn save(name: &str, state: &SettingsState) {
    let (w, h) = (420u32, 460u32);
    let mut buffer = vec![0u32; (w * h) as usize];
    settings_render::render(&mut buffer, w, h, state);

    let mut img = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = buffer[(y * w + x) as usize];
            let r = ((v >> 16) & 0xff) as u8;
            let g = ((v >> 8) & 0xff) as u8;
            let b = (v & 0xff) as u8;
            img.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    img.save(name).unwrap();
    println!("wrote {name}");
}

fn main() {
    save(
        "/tmp/preview_settings_idle.png",
        &SettingsState {
            autostart: true,
            antispoof_enabled: true,
            lock_on_absence: false,
            enroll: EnrollButtonState::Idle,
        },
    );
    save(
        "/tmp/preview_settings_progress.png",
        &SettingsState {
            autostart: false,
            antispoof_enabled: false,
            lock_on_absence: true,
            enroll: EnrollButtonState::InProgress {
                captured: 3,
                total: 8,
            },
        },
    );
    save(
        "/tmp/preview_settings_done.png",
        &SettingsState {
            autostart: true,
            antispoof_enabled: true,
            lock_on_absence: true,
            enroll: EnrollButtonState::Done,
        },
    );
}
