mod autostart;
mod camera;
mod enroll;
mod face_engine;
#[cfg(target_os = "macos")]
mod kiosk_macos;
mod lock;
mod render;
mod settings_render;
mod storage;
mod text;
mod tray;
mod window;

use anyhow::Result;
use clap::Parser;

/// OhMyLock: a lightweight, cross-platform screen lock unlocked by a
/// previously enrolled face.
#[derive(Parser)]
#[command(name = "ohmylock")]
struct Cli {
    /// (Re-)enroll the face used to unlock, instead of starting the lock screen.
    #[arg(long)]
    enroll: bool,

    /// Path to the face detector ONNX model (SCRFD det_500m "_kps" style).
    #[arg(long)]
    detector_model: Option<std::path::PathBuf>,

    /// Path to the face embedding ONNX model (MobileFaceNet w600k_mbf style).
    #[arg(long)]
    embedder_model: Option<std::path::PathBuf>,

    /// Path to the anti-spoof ONNX model (MiniFASNetV2 2.7_80x80 style).
    #[arg(long)]
    antispoof_model: Option<std::path::PathBuf>,
}

fn main() -> Result<()> {
    camera::init_platform();

    let cli = Cli::parse();
    let models = storage::models_dir()?;
    let detector_path = cli
        .detector_model
        .unwrap_or_else(|| models.join("detector.onnx"));
    let embedder_path = cli
        .embedder_model
        .unwrap_or_else(|| models.join("embedder.onnx"));
    let antispoof_path = cli
        .antispoof_model
        .unwrap_or_else(|| models.join("antispoof.onnx"));

    for path in [&detector_path, &embedder_path, &antispoof_path] {
        if !path.exists() {
            anyhow::bail!(
                "model file not found: {}\nSee README.md for where to download the required ONNX models.",
                path.display()
            );
        }
    }

    if cli.enroll {
        enroll::run(detector_path, embedder_path, antispoof_path)
    } else {
        window::run(detector_path, embedder_path, antispoof_path)
    }
}
