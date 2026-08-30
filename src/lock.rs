use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::camera::FaceCamera;
use crate::face_engine::{ANTISPOOF_THRESHOLD, FaceEngine, cosine_similarity};
use crate::render::LockStatus;
use crate::storage::{EnrolledFace, Settings};
use crate::window::UserEvent;

/// How often we sample the camera and run inference. Sparse polling (2 Hz)
/// keeps CPU usage low while still unlocking within about half a second of a
/// recognized face appearing.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Consecutive matching frames required before unlocking. A single frame
/// was found to false-accept a different person in practice -- a lucky
/// noisy frame (motion blur, exposure shift, an unlucky pose) can push a
/// stranger's similarity over the threshold for one sample even when the
/// alignment + threshold combination separates identities well on average.
/// Requiring two consecutive matches (~1s at `POLL_INTERVAL`) makes that a
/// much rarer coincidence while staying fast enough not to feel laggy for
/// the real, enrolled face.
const REQUIRED_CONSECUTIVE_MATCHES: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameOutcome {
    NoFace,
    NoMatch,
    SpoofSuspected,
    Matched,
}

/// Spawns the background camera + face-matching loop for one lock session.
/// It runs until it finds a confident match (sending `UserEvent::Unlocked`)
/// or the process exits; a new lock session spawns a fresh matcher, but
/// reuses the same `engine` rather than reloading the ONNX models from disk
/// every time -- repeatedly loading and dropping ORT sessions on every
/// lock/unlock cycle was fragmenting the heap and creeping RSS up over the
/// life of the process instead of returning to baseline.
pub fn spawn_matcher(
    engine: Arc<Mutex<FaceEngine>>,
    enrolled: EnrolledFace,
    settings: Settings,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) {
    thread::spawn(move || {
        if let Err(err) = matcher_loop(engine, enrolled, settings, proxy) {
            eprintln!("face matcher stopped: {err:#}");
        }
    });
}

fn matcher_loop(
    engine: Arc<Mutex<FaceEngine>>,
    enrolled: EnrolledFace,
    settings: Settings,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) -> Result<()> {
    let mut camera = FaceCamera::open()?;
    let mut consecutive_matches = 0u32;

    loop {
        let frame = match camera.grab() {
            Ok(f) => f,
            Err(err) => {
                eprintln!("camera error: {err:#}");
                thread::sleep(POLL_INTERVAL);
                continue;
            }
        };

        // Distinguishes four outcomes per frame, since each needs different
        // feedback: no face at all (Scanning), a face that isn't the
        // enrolled one (NoMatch), the enrolled face's embedding but a
        // failed liveness check (SpoofSuspected -- a photo or screen held
        // up to the camera), or a genuine match (Matched). Wrapped in
        // catch_unwind so one bad frame (a malformed camera buffer, an
        // unexpected model output shape, etc.) can't take the whole matcher
        // thread down -- given the lock screen is only supposed to close on
        // a real match, a matcher that silently dies would otherwise turn
        // into an unrecoverable lock.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Result<FrameOutcome> {
                // Recover from poisoning rather than propagating it: a panic
                // here is caught by the outer catch_unwind and just costs
                // this one frame, but a poisoned Mutex would otherwise wedge
                // every future frame's lock() forever, turning one bad frame
                // into a lock screen that can never see a face again.
                let mut engine = engine.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(detected) = engine.detect_largest_face(&frame)? else {
                    return Ok(FrameOutcome::NoFace);
                };
                let embedding = engine.embed_face(&frame, &detected.landmarks)?;
                let similarity = cosine_similarity(&embedding, &enrolled.embedding);
                if similarity < enrolled.threshold {
                    return Ok(FrameOutcome::NoMatch);
                }
                if !settings.antispoof_enabled {
                    return Ok(FrameOutcome::Matched);
                }
                // Only the (rare) candidate-match frame pays for the extra
                // anti-spoof inference, not every frame -- a stranger's face
                // never reaches this check.
                let liveness = engine.check_liveness(&frame, detected.bbox)?;
                if liveness < ANTISPOOF_THRESHOLD {
                    eprintln!(
                        "antispoof: rejected candidate match (score={liveness:.3}, threshold={ANTISPOOF_THRESHOLD:.3}) -- if this is actually you, turn off \"Canlılık tespiti\" in Ayarlar"
                    );
                    return Ok(FrameOutcome::SpoofSuspected);
                }
                Ok(FrameOutcome::Matched)
            },
        ))
        .unwrap_or(Ok(FrameOutcome::NoFace))
        .unwrap_or(FrameOutcome::NoFace);

        if outcome == FrameOutcome::Matched {
            consecutive_matches += 1;
        } else {
            consecutive_matches = 0;
        }

        let status = match outcome {
            FrameOutcome::NoFace => LockStatus::Scanning,
            FrameOutcome::NoMatch => LockStatus::NoMatch,
            FrameOutcome::SpoofSuspected => LockStatus::SpoofSuspected,
            FrameOutcome::Matched => LockStatus::Matched,
        };
        let _ = proxy.send_event(UserEvent::Status(status));

        if consecutive_matches >= REQUIRED_CONSECUTIVE_MATCHES {
            let _ = proxy.send_event(UserEvent::Unlocked);
            return Ok(());
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// How often the absence sentinel samples the camera while unlocked. Much
/// sparser than the 2 Hz lock-screen matcher: this can run for hours in the
/// background, so it trades responsiveness for a lighter footprint --
/// noticing an empty desk within `ABSENCE_POLL_INTERVAL` is plenty timely
/// for a "walked away" signal.
const ABSENCE_POLL_INTERVAL: Duration = Duration::from_secs(4);
/// How long the desk has to stay empty before auto-locking. Long enough
/// that leaning out of frame to grab something doesn't lock the screen.
const ABSENCE_LOCK_AFTER: Duration = Duration::from_secs(10);
/// The stop flag is only checked between chunks of this size, so this
/// bounds how long `stop_absence_sentinel` can block waiting for the
/// thread to actually release the camera.
const ABSENCE_STOP_GRANULARITY: Duration = Duration::from_millis(200);

/// A running absence sentinel: the stop flag tells the background thread to
/// exit (and release the camera) at its next poll; the join handle lets a
/// caller block until that's actually happened, so it's safe to immediately
/// open the camera again afterward (e.g. for the lock-screen matcher)
/// without a device-busy race.
pub struct AbsenceSentinel {
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl AbsenceSentinel {
    /// Signals the sentinel to stop and blocks until its thread has exited
    /// and dropped its camera handle.
    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

/// Spawns a background thread that periodically checks whether a face is
/// present while the screen is unlocked, and requests a lock after a
/// sustained absence. Only meant to run while unlocked -- the caller is
/// responsible for stopping it before a lock session starts (and camera
/// device contention is exactly why `AbsenceSentinel::stop` blocks until
/// the camera is actually released).
pub fn spawn_absence_sentinel(
    engine: Arc<Mutex<FaceEngine>>,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) -> AbsenceSentinel {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let handle = thread::spawn(move || {
        let mut camera = match FaceCamera::open() {
            Ok(camera) => camera,
            Err(err) => {
                eprintln!("absence sentinel: failed to open camera: {err:#}");
                return;
            }
        };
        let mut absent_since: Option<Instant> = None;

        while !stop_for_thread.load(Ordering::Relaxed) {
            if sleep_unless_stopped(&stop_for_thread, ABSENCE_POLL_INTERVAL) {
                break;
            }

            let has_face =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
                    let frame = camera.grab()?;
                    let mut engine = engine
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    Ok(engine.detect_largest_face(&frame)?.is_some())
                }))
                .unwrap_or(Ok(true)) // a bad frame shouldn't itself trigger a lock
                .unwrap_or(true);

            if has_face {
                absent_since = None;
                continue;
            }
            let since = *absent_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= ABSENCE_LOCK_AFTER {
                let _ = proxy.send_event(UserEvent::LockRequested);
                return;
            }
        }
    });

    AbsenceSentinel { stop, handle }
}

/// Sleeps for `total`, checking `stop` every `ABSENCE_STOP_GRANULARITY` so a
/// stop request lands quickly instead of after a full multi-second poll
/// interval. Returns `true` if it woke up early because of a stop request.
fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) -> bool {
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        let chunk = remaining.min(ABSENCE_STOP_GRANULARITY);
        thread::sleep(chunk);
        remaining -= chunk;
    }
    stop.load(Ordering::Relaxed)
}
