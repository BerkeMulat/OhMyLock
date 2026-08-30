// Minimal stand-in for src/window.rs's UserEvent, just so tray.rs's
// `use crate::window::UserEvent` resolves -- pulling in the real window.rs
// would drag in face_engine/lock/render/storage too, which this preview
// doesn't need.
mod window {
    pub enum UserEvent {
        Status(crate::render_stub::LockStatus),
        Unlocked,
        LockRequested,
        OpenSettingsRequested,
        EnrollProgress(u32),
        EnrollFinished(Result<(), String>),
        QuitRequested,
    }
}
mod render_stub {
    pub enum LockStatus {}
}

#[path = "../src/autostart.rs"]
mod autostart;
#[path = "../src/tray.rs"]
mod tray;

fn main() {
    let (rgba, size) = tray::build_icon_rgba();
    let scale = 8;
    let mut img = image::RgbaImage::new(size * scale, size * scale);
    for px in img.pixels_mut() {
        *px = image::Rgba([30, 30, 34, 255]);
    }
    for y in 0..size {
        for x in 0..size {
            let idx = ((y * size + x) * 4) as usize;
            let a = rgba[idx + 3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            let px = image::Rgba([
                (rgba[idx] as f32 * a + 30.0 * (1.0 - a)) as u8,
                (rgba[idx + 1] as f32 * a + 30.0 * (1.0 - a)) as u8,
                (rgba[idx + 2] as f32 * a + 34.0 * (1.0 - a)) as u8,
                255,
            ]);
            for dy in 0..scale {
                for dx in 0..scale {
                    img.put_pixel(x * scale + dx, y * scale + dy, px);
                }
            }
        }
    }
    std::fs::create_dir_all("assets").unwrap();
    img.save("assets/tray_icon_preview.png").unwrap();
    println!("wrote assets/tray_icon_preview.png");
}
