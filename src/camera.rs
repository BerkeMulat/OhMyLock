use anyhow::{Context, Result};
use image::RgbImage;
use nokhwa::{
    Camera,
    pixel_format::RgbFormat,
    utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution,
    },
};

/// Low resolution / low frame rate on purpose: this keeps CPU and RAM usage
/// down since we only need a small image to run face detection on, not a
/// high quality video feed.
const CAPTURE_WIDTH: u32 = 640;
const CAPTURE_HEIGHT: u32 = 480;
const CAPTURE_FPS: u32 = 8;

/// On macOS, nokhwa requires this to run (on the main thread) before any
/// other camera call -- it drives the AVFoundation permission handshake.
/// Skipping it and opening the camera straight from a background thread (as
/// the face-matcher does) deadlocks the whole app: the background thread
/// ends up waiting on AVFoundation setup that itself needs the main thread,
/// while the main thread's winit event loop is idling, so neither side ever
/// makes progress -- the window freezes with no crash and no error message.
/// Doing this once, up front, on the main thread avoids that entirely.
#[cfg(target_os = "macos")]
pub fn init_platform() {
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    nokhwa::nokhwa_initialize(move |granted| {
        let _ = tx.send(granted);
    });
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => {}
        Ok(false) => eprintln!(
            "Camera access was denied -- grant it in System Settings > Privacy & Security > Camera, then restart OhMyLock."
        ),
        Err(_) => {
            eprintln!("Camera permission prompt did not resolve within 10s; continuing anyway.")
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn init_platform() {}

pub struct FaceCamera {
    camera: Camera,
}

impl FaceCamera {
    pub fn open() -> Result<Self> {
        let index = CameraIndex::Index(0);

        // Not every camera/driver supports the exact (resolution, format,
        // fps) tuple we'd prefer, so we try a few progressively looser
        // requests rather than failing on the first mismatch. Whatever
        // frame size we end up with, `face_engine` resizes it down for
        // inference anyway, so a larger capture just costs a bit more
        // decode time, not correctness.
        let attempts: [RequestedFormat; 3] = [
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(CameraFormat::new(
                Resolution::new(CAPTURE_WIDTH, CAPTURE_HEIGHT),
                FrameFormat::MJPEG,
                CAPTURE_FPS,
            ))),
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestFrameRate(CAPTURE_FPS)),
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate),
        ];

        let mut last_err = None;
        for requested in attempts {
            match Camera::new(index.clone(), requested) {
                Ok(mut camera) => {
                    if let Err(err) = camera.open_stream() {
                        last_err = Some(err.into());
                        continue;
                    }
                    return Ok(Self { camera });
                }
                Err(err) => last_err = Some(err.into()),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no camera available")))
            .context("failed to open camera")
    }

    pub fn grab(&mut self) -> Result<RgbImage> {
        let frame = self.camera.frame().context("failed to grab camera frame")?;
        let decoded = frame
            .decode_image::<RgbFormat>()
            .context("failed to decode camera frame")?;
        Ok(decoded)
    }
}
