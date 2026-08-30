use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct EnrolledFace {
    pub embedding: Vec<f32>,
    pub threshold: f32,
}

/// User-facing toggles surfaced in the settings window. Kept as a small,
/// separate file from `EnrolledFace` (rather than folded into it) since
/// these are app behavior preferences, not identity data, and someone
/// re-enrolling shouldn't reset them.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    /// Kill switch for the MiniFASNetV2 liveness check (see
    /// `face_engine::ANTISPOOF_THRESHOLD`). Defaults on, but the threshold
    /// hasn't been calibrated against real webcam frames, so this exists to
    /// be turned off immediately if it's rejecting genuine faces.
    pub antispoof_enabled: bool,
    /// While unlocked, periodically check for a face in frame and
    /// auto-lock after a sustained absence (see `lock::spawn_absence_sentinel`).
    /// Defaults off: it means the camera and face models stay loaded while
    /// idle, which trades away this app's normal near-zero idle footprint.
    pub lock_on_absence: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            antispoof_enabled: false,
            lock_on_absence: false,
        }
    }
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("dev", "facelock", "FaceLock")
        .context("could not determine platform config directory")
}

pub fn data_dir() -> Result<PathBuf> {
    let dirs = project_dirs()?;
    let dir = dirs.data_dir().to_path_buf();
    fs::create_dir_all(&dir).context("failed to create data directory")?;
    Ok(dir)
}

pub fn models_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("models"))
}

fn face_file() -> Result<PathBuf> {
    Ok(data_dir()?.join("face.json"))
}

pub fn save(face: &EnrolledFace) -> Result<()> {
    let path = face_file()?;
    let json = serde_json::to_string_pretty(face)?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn load() -> Result<Option<EnrolledFace>> {
    let path = face_file()?;
    if !path.exists() {
        return Ok(None);
    }
    let json = fs::read_to_string(&path)?;
    let face: EnrolledFace = serde_json::from_str(&json)?;
    Ok(Some(face))
}

fn settings_file() -> Result<PathBuf> {
    Ok(data_dir()?.join("settings.json"))
}

pub fn save_settings(settings: &Settings) -> Result<()> {
    let path = settings_file()?;
    let json = serde_json::to_string_pretty(settings)?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Falls back to `Settings::default()` both when the file is absent (first
/// run) and when it fails to parse (e.g. an older version's schema) --
/// settings are low-stakes preferences, so silently resetting to sane
/// defaults is friendlier than refusing to start the app over a bad file.
pub fn load_settings() -> Result<Settings> {
    let path = settings_file()?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let json = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&json).unwrap_or_default())
}
