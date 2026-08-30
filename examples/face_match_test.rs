use std::path::PathBuf;

#[path = "../src/face_engine.rs"]
mod face_engine;

use face_engine::{FaceEngine, cosine_similarity};

fn load_embedding(engine: &mut FaceEngine, path: &str) -> anyhow::Result<Vec<f32>> {
    let img = image::open(path)?.to_rgb8();
    let detected = engine
        .detect_largest_face(&img)?
        .ok_or_else(|| anyhow::anyhow!("no face detected in {path}"))?;
    println!(
        "  landmarks in {path}: {:?} (bbox {:?})",
        detected.landmarks, detected.bbox
    );
    engine.embed_face(&img, &detected.landmarks)
}

fn main() -> anyhow::Result<()> {
    let models = PathBuf::from(std::env::var("HOME").unwrap())
        .join("Library/Application Support/dev.facelock.FaceLock/models");
    let mut engine = FaceEngine::load(
        &models.join("detector.onnx"),
        &models.join("embedder.onnx"),
        &models.join("antispoof.onnx"),
    )?;

    let args: Vec<String> = std::env::args().collect();
    let path_a = &args[1];
    let path_b = &args[2];

    println!("embedding A ({path_a})...");
    let emb_a = load_embedding(&mut engine, path_a)?;
    println!("embedding A again (determinism check)...");
    let emb_a2 = load_embedding(&mut engine, path_a)?;
    println!("embedding B ({path_b})...");
    let emb_b = load_embedding(&mut engine, path_b)?;

    println!();
    println!(
        "A vs A (same image, twice)   : {:.4}",
        cosine_similarity(&emb_a, &emb_a2)
    );
    println!(
        "A vs B (different identities): {:.4}",
        cosine_similarity(&emb_a, &emb_b)
    );
    Ok(())
}
