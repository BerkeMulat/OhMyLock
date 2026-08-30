#[path = "../src/render.rs"]
mod render;
#[path = "../src/text.rs"]
mod text;

use render::LockStatus;

fn save(name: &str, status: LockStatus, t: f32, status_t: f32) {
    let (w, h) = (960u32, 600u32);
    let mut buffer = vec![0u32; (w * h) as usize];
    render::render(&mut buffer, w, h, status, t, status_t);

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
    save("/tmp/preview_scanning.png", LockStatus::Scanning, 1.3, 5.0);
    save(
        "/tmp/preview_nomatch_flash.png",
        LockStatus::NoMatch,
        0.05,
        0.05,
    );
    save(
        "/tmp/preview_nomatch_settled.png",
        LockStatus::NoMatch,
        1.0,
        1.0,
    );
    save("/tmp/preview_matched.png", LockStatus::Matched, 0.5, 0.5);
    save(
        "/tmp/preview_spoof.png",
        LockStatus::SpoofSuspected,
        1.0,
        1.0,
    );
}
