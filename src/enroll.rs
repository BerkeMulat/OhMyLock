use anyhow::{Context, Result, bail};
use image::RgbImage;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::camera::FaceCamera;
use crate::face_engine::{FaceEngine, cosine_similarity};
use crate::storage::{self, EnrolledFace};
use crate::window::UserEvent;

pub const SAMPLES: usize = 8;
/// Cosine-similarity threshold used at unlock time. With properly aligned
/// ArcFace/MobileFaceNet embeddings, the same person across photos usually
/// scores well above 0.6 while different people usually stay below 0.3.
/// 0.5 was too permissive in practice (a false accept was observed with a
/// different person in frame); 0.62 sits solidly above the "different
/// person" band while still comfortably below typical same-person scores,
/// closing that gap without making genuine matches flaky.
const DEFAULT_THRESHOLD: f32 = 0.62;
/// Consecutive matching frames required to confirm identity before a
/// re-enrollment is allowed to overwrite the enrolled face -- mirrors
/// `lock::REQUIRED_CONSECUTIVE_MATCHES` so a re-enroll can't be started off
/// a single lucky/noisy frame any more than an unlock can.
const VERIFY_REQUIRED_MATCHES: u32 = 2;
/// Bounds how long the verification step will keep polling for a match
/// before giving up (at ~200ms per attempt, this is roughly a minute) --
/// long enough to allow for a bad angle or lighting, short enough not to
/// hang forever if it's someone other than the enrolled user.
const VERIFY_MAX_ATTEMPTS: usize = 300;

pub fn run(
    detector_path: std::path::PathBuf,
    embedder_path: std::path::PathBuf,
    antispoof_path: std::path::PathBuf,
) -> Result<()> {
    println!("Enrollment: look directly at the camera. Capturing {SAMPLES} samples...");

    let mut engine = FaceEngine::load(&detector_path, &embedder_path, &antispoof_path)?;
    let mut camera = FaceCamera::open()?;

    let embeddings = capture_samples(
        &mut camera,
        |frame| {
            let Some(detected) = engine.detect_largest_face(frame)? else {
                return Ok(None);
            };
            Ok(Some(engine.embed_face(frame, &detected.landmarks)?))
        },
        |n| println!("  captured sample {n}/{SAMPLES}"),
    )?;

    storage::save(&EnrolledFace {
        embedding: average_normalized(&embeddings),
        threshold: DEFAULT_THRESHOLD,
    })?;

    println!("Enrollment complete. You can now run without --enroll to start the lock screen.");
    Ok(())
}

/// Spawns a background re-enrollment capture for the settings window's
/// "Yüzü Yeniden Tanımla" button. Reuses the already-loaded `engine` (no
/// reason to reload the ONNX sessions from disk for this) and reports
/// progress via `UserEvent`s instead of `println!`, since this runs with no
/// attached terminal and needs to update the settings window's UI.
///
/// `current` is the face already on file: `reenroll` must verify it's
/// actually looking at that face before it's allowed to capture and save a
/// replacement, so opening Ayarlar (which only requires being unlocked, not
/// re-proving identity) can't be used to silently swap in a different
/// person's face and lock the real owner out.
pub fn spawn_reenroll(
    engine: Arc<Mutex<FaceEngine>>,
    current: EnrolledFace,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) {
    thread::spawn(move || {
        let result = reenroll(&engine, &current, &proxy).map_err(|err| format!("{err:#}"));
        let _ = proxy.send_event(UserEvent::EnrollFinished(result));
    });
}

/// Polls the camera until `VERIFY_REQUIRED_MATCHES` consecutive frames match
/// `current`'s embedding above its threshold, confirming the person about to
/// re-enroll is the one already enrolled. Bails after `VERIFY_MAX_ATTEMPTS`
/// attempts rather than blocking forever if it never matches.
fn verify_current_identity(
    camera: &mut FaceCamera,
    engine: &Arc<Mutex<FaceEngine>>,
    current: &EnrolledFace,
) -> Result<()> {
    let mut consecutive_matches = 0u32;
    for _ in 0..VERIFY_MAX_ATTEMPTS {
        let frame = camera.grab().context("failed to read camera frame")?;
        let matched = {
            let mut engine = engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match engine.detect_largest_face(&frame)? {
                Some(detected) => {
                    let embedding = engine.embed_face(&frame, &detected.landmarks)?;
                    cosine_similarity(&embedding, &current.embedding) >= current.threshold
                }
                None => false,
            }
        };
        consecutive_matches = if matched { consecutive_matches + 1 } else { 0 };
        if consecutive_matches >= VERIFY_REQUIRED_MATCHES {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    bail!("mevcut yüz doğrulanamadı -- yeniden tanımlamak için önce kayıtlı yüzünüzü kameraya gösterin")
}

fn reenroll(
    engine: &Arc<Mutex<FaceEngine>>,
    current: &EnrolledFace,
    proxy: &winit::event_loop::EventLoopProxy<UserEvent>,
) -> Result<()> {
    let mut camera = FaceCamera::open()?;

    verify_current_identity(&mut camera, engine, current)?;

    let embeddings = capture_samples(
        &mut camera,
        |frame| {
            let mut engine = engine
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(detected) = engine.detect_largest_face(frame)? else {
                return Ok(None);
            };
            Ok(Some(engine.embed_face(frame, &detected.landmarks)?))
        },
        |n| {
            let _ = proxy.send_event(UserEvent::EnrollProgress(n as u32));
        },
    )?;

    storage::save(&EnrolledFace {
        embedding: average_normalized(&embeddings),
        threshold: DEFAULT_THRESHOLD,
    })
}

/// Repeatedly grabs frames and calls `detect_and_embed` (which returns
/// `None` for a frame with no usable face) until `SAMPLES` embeddings are
/// collected, reporting progress after each one via `on_progress`. Shared
/// between the CLI `--enroll` flow (owns its `FaceEngine` outright) and the
/// settings-window re-enroll flow (locks a shared `Arc<Mutex<FaceEngine>>`
/// per frame) -- the two differ only in how they get from a frame to an
/// embedding, which is exactly what the closure parameterizes.
fn capture_samples(
    camera: &mut FaceCamera,
    mut detect_and_embed: impl FnMut(&RgbImage) -> Result<Option<Vec<f32>>>,
    mut on_progress: impl FnMut(usize),
) -> Result<Vec<Vec<f32>>> {
    let mut embeddings = Vec::with_capacity(SAMPLES);
    let mut attempts = 0;
    while embeddings.len() < SAMPLES {
        attempts += 1;
        if attempts > SAMPLES * 10 {
            bail!("could not reliably detect a face -- check camera and lighting, then retry");
        }
        let frame = camera.grab().context("failed to read camera frame")?;
        let Some(embedding) = detect_and_embed(&frame)? else {
            continue;
        };
        embeddings.push(embedding);
        on_progress(embeddings.len());
        thread::sleep(Duration::from_millis(200));
    }
    Ok(embeddings)
}

/// Mean of `embeddings`, re-normalized to unit length (the mean of several
/// unit vectors isn't itself unit length).
fn average_normalized(embeddings: &[Vec<f32>]) -> Vec<f32> {
    let dim = embeddings[0].len();
    let mut mean = vec![0f32; dim];
    for emb in embeddings {
        for (i, v) in emb.iter().enumerate() {
            mean[i] += v;
        }
    }
    for v in mean.iter_mut() {
        *v /= embeddings.len() as f32;
    }
    let norm = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in mean.iter_mut() {
            *v /= norm;
        }
    }
    mean
}
