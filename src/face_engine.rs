use anyhow::{Context, Result};
use image::{Rgb, RgbImage, imageops::FilterType};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Value;
use std::path::Path;

// ---------------------------------------------------------------------
// SCRFD (det_500m, "_kps" variant) face detector.
// Input: 1x3x640x640 RGB, normalized as (pixel - 127.5) / 128.0
// Outputs (9 tensors, order fixed by the graph): for each of the three
// strides [8, 16, 32], a [N,1] score tensor, a [N,4] box-distance tensor,
// and a [N,10] keypoint-distance tensor (5 landmarks x, y).
// ---------------------------------------------------------------------
const DET_SIZE: u32 = 640;
const STRIDES: [u32; 3] = [8, 16, 32];
const NUM_ANCHORS: usize = 2;
const SCORE_THRESHOLD: f32 = 0.5;
const NMS_IOU_THRESHOLD: f32 = 0.4;

// ---------------------------------------------------------------------
// MobileFaceNet (w600k_mbf) embedding model.
// Input: 1x3x112x112 RGB, normalized as (pixel - 127.5) / 127.5
// Output: 512-dim embedding vector.
// ---------------------------------------------------------------------
const EMB_SIZE: u32 = 112;

// ---------------------------------------------------------------------
// MiniFASNetV2 (2.7_80x80) silent anti-spoof classifier.
// Input: 1x3x80x80 BGR, normalized as pixel / 255.0 (no mean/std shift).
// The crop is *not* the aligned 112x112 face used for embedding -- it's a
// separate, wider crop taken directly from the detector's bounding box
// (2.7x the box size, centered on the box, no rotation/warp), matching
// exactly how the upstream model was trained. Feeding it the aligned crop
// instead would shift the input distribution and silently degrade accuracy.
// Output: 3-class softmax [live, print-attack, replay-attack]; liveness
// score is `1 - (p[print] + p[replay])`, equivalently just p[live].
// ---------------------------------------------------------------------
const ANTISPOOF_SIZE: u32 = 80;
const ANTISPOOF_CROP_SCALE: f32 = 2.7;
/// NOTE: this has not been calibrated against real webcam frames (no camera
/// available in the environment that built this) -- it's a starting point
/// from the model's own docs, not a measured value. A consumer webcam's
/// auto white-balance/exposure and MJPEG compression can push a genuine
/// live face's score lower than a curated benchmark would suggest, which
/// is the likely cause if this misfires on a real face. `lock.rs` logs the
/// actual score to stderr on every rejection specifically so this can be
/// retuned from real data instead of guessed again. See also
/// `Settings::antispoof_enabled` for an outright kill switch.
pub const ANTISPOOF_THRESHOLD: f32 = 0.5;

/// Canonical 112x112 ArcFace landmark template (left eye, right eye, nose
/// tip, left mouth corner, right mouth corner). Every enrolled/matched face
/// is warped so its detected landmarks line up with these points -- this
/// alignment step is what makes the embedding model actually discriminate
/// between identities. Skipping it (feeding a raw, unaligned crop) is the
/// classic mistake that makes an ArcFace-family model treat every face as
/// roughly the same.
const ARCFACE_TEMPLATE: [(f32, f32); 5] = [
    (38.2946, 51.6963),
    (73.5318, 51.5014),
    (56.0252, 71.7366),
    (41.5493, 92.3655),
    (70.7299, 92.2041),
];

#[derive(Clone, Copy, Debug)]
struct Point {
    x: f32,
    y: f32,
}

/// The largest detected face's aligned-crop landmarks plus its raw
/// detector bounding box (`x, y, w, h`, in the original frame's pixel
/// coordinates) -- the landmarks drive embedding alignment, the bbox drives
/// the anti-spoof crop, and the two use deliberately different crop
/// geometries (see `ANTISPOOF_CROP_SCALE`), so both are needed downstream.
pub struct DetectedFace {
    pub landmarks: [(f32, f32); 5],
    pub bbox: (f32, f32, f32, f32),
}

#[derive(Clone, Copy, Debug)]
pub struct FaceCandidate {
    xmin: f32,
    ymin: f32,
    xmax: f32,
    ymax: f32,
    score: f32,
    landmarks: [Point; 5],
}

fn area(c: &FaceCandidate) -> f32 {
    (c.xmax - c.xmin).max(0.0) * (c.ymax - c.ymin).max(0.0)
}

fn iou(a: &FaceCandidate, b: &FaceCandidate) -> f32 {
    let ix1 = a.xmin.max(b.xmin);
    let iy1 = a.ymin.max(b.ymin);
    let ix2 = a.xmax.min(b.xmax);
    let iy2 = a.ymax.min(b.ymax);
    let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn nms(mut candidates: Vec<FaceCandidate>) -> Vec<FaceCandidate> {
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    let mut kept: Vec<FaceCandidate> = Vec::new();
    for candidate in candidates {
        if kept.iter().all(|k| iou(k, &candidate) < NMS_IOU_THRESHOLD) {
            kept.push(candidate);
        }
    }
    kept
}

/// Resizes `img` to fit inside a `target`x`target` canvas without distorting
/// its aspect ratio, padding the remainder with black. Returns the padded
/// canvas plus the scale factor needed to map canvas coordinates back to
/// coordinates in the original image.
fn letterbox(img: &RgbImage, target: u32) -> (RgbImage, f32) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let im_ratio = h / w;
    let (new_w, new_h) = if im_ratio > 1.0 {
        let nh = target as f32;
        let nw = (nh / im_ratio).round().max(1.0);
        (nw as u32, nh as u32)
    } else {
        let nw = target as f32;
        let nh = (nw * im_ratio).round().max(1.0);
        (nw as u32, nh as u32)
    };
    let resized = image::imageops::resize(img, new_w, new_h, FilterType::Triangle);
    let mut canvas = RgbImage::from_pixel(target, target, Rgb([0, 0, 0]));
    image::imageops::overlay(&mut canvas, &resized, 0, 0);
    let scale = new_h as f32 / h;
    (canvas, scale)
}

fn generate_anchor_centers(stride: u32) -> Vec<Point> {
    let feat = DET_SIZE / stride;
    let mut centers = Vec::with_capacity((feat * feat) as usize * NUM_ANCHORS);
    for i in 0..feat {
        for j in 0..feat {
            let cx = (j * stride) as f32;
            let cy = (i * stride) as f32;
            for _ in 0..NUM_ANCHORS {
                centers.push(Point { x: cx, y: cy });
            }
        }
    }
    centers
}

pub struct FaceEngine {
    detector: Session,
    embedder: Session,
    antispoof: Session,
}

/// `SessionBuilder`'s option setters return the builder itself on error (so
/// callers can recover and keep using it), which drags a raw-pointer-bearing
/// `SessionBuilder` into the error type and makes it `!Sync` -- and thus
/// unusable with `anyhow`'s `?`, which requires `Send + Sync` errors. We
/// never want to recover here, so every step below is flattened to a plain
/// message with `map_err` instead of propagated directly.
fn configure_low_memory(
    builder: ort::session::builder::SessionBuilder,
) -> Result<ort::session::builder::SessionBuilder> {
    builder
        .with_intra_threads(1)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_parallel_execution(false)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_memory_pattern(false)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        // `with_memory_pattern(false)` only disables *pattern* reuse across
        // runs; ORT's CPU allocator still pools memory in a growing arena
        // by default regardless. That arena is sized for throughput, not
        // footprint, and never shrinks -- disabling it entirely trades a
        // bit of alloc/free overhead per run (irrelevant at 2Hz) for RSS
        // that actually tracks what's live instead of a high-water mark.
        .with_execution_providers([ort::ep::CPU::default().build()])
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

impl FaceEngine {
    pub fn load(detector_path: &Path, embedder_path: &Path, antispoof_path: &Path) -> Result<Self> {
        // We only ever run one frame through one model at a time (2Hz
        // polling, one face), so ORT's default thread pool -- sized to the
        // number of logical CPUs, per session -- and its growing memory
        // arena buy no speed here and are the main reason this idle-most-
        // of-the-time app was sitting at hundreds of MB of RSS. Pinning both
        // sessions to a single thread with no arena keeps each inference
        // call's memory bounded to roughly what the model itself needs.
        let builder = Session::builder().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let detector = configure_low_memory(builder)?
            .commit_from_file(detector_path)
            .with_context(|| {
                format!(
                    "failed to load detector model at {}",
                    detector_path.display()
                )
            })?;
        let builder = Session::builder().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let embedder = configure_low_memory(builder)?
            .commit_from_file(embedder_path)
            .with_context(|| {
                format!(
                    "failed to load embedder model at {}",
                    embedder_path.display()
                )
            })?;
        let builder = Session::builder().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let antispoof = configure_low_memory(builder)?
            .commit_from_file(antispoof_path)
            .with_context(|| {
                format!(
                    "failed to load antispoof model at {}",
                    antispoof_path.display()
                )
            })?;
        Ok(Self {
            detector,
            embedder,
            antispoof,
        })
    }

    fn detect_candidates(&mut self, image: &RgbImage) -> Result<Vec<FaceCandidate>> {
        let (canvas, det_scale) = letterbox(image, DET_SIZE);

        let side = DET_SIZE as usize;
        let mut input = vec![0f32; 3 * side * side];
        for y in 0..side {
            for x in 0..side {
                let px = canvas.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    let v = (px[c] as f32 - 127.5) / 128.0;
                    input[c * side * side + y * side + x] = v;
                }
            }
        }

        let input_value = Value::from_array(([1usize, 3, side, side], input))?;
        let outputs = self.detector.run(ort::inputs![input_value])?;

        // Identify the nine outputs by tensor shape rather than name, since
        // ONNX exports commonly leave outputs named as raw graph node ids.
        let mut by_len: std::collections::HashMap<usize, Vec<(usize, Vec<f32>)>> =
            std::collections::HashMap::new();
        for (_name, value) in outputs.iter() {
            let (shape, data) = value.try_extract_tensor::<f32>()?;
            let n = shape[0] as usize;
            let last = *shape.last().unwrap() as usize;
            by_len.entry(last).or_default().push((n, data.to_vec()));
        }

        let mut scores_by_n: std::collections::HashMap<usize, Vec<f32>> = Default::default();
        let mut boxes_by_n: std::collections::HashMap<usize, Vec<f32>> = Default::default();
        let mut kps_by_n: std::collections::HashMap<usize, Vec<f32>> = Default::default();
        for (n, data) in by_len.remove(&1).unwrap_or_default() {
            scores_by_n.insert(n, data);
        }
        for (n, data) in by_len.remove(&4).unwrap_or_default() {
            boxes_by_n.insert(n, data);
        }
        for (n, data) in by_len.remove(&10).unwrap_or_default() {
            kps_by_n.insert(n, data);
        }

        let mut candidates = Vec::new();
        for &stride in &STRIDES {
            let centers = generate_anchor_centers(stride);
            let n = centers.len();
            let scores = scores_by_n
                .get(&n)
                .with_context(|| format!("missing score tensor for stride {stride}"))?;
            let boxes = boxes_by_n
                .get(&n)
                .with_context(|| format!("missing box tensor for stride {stride}"))?;
            let kps = kps_by_n
                .get(&n)
                .with_context(|| format!("missing keypoint tensor for stride {stride}"))?;

            for i in 0..n {
                let score = scores[i];
                if score < SCORE_THRESHOLD {
                    continue;
                }
                let c = centers[i];
                let stride_f = stride as f32;
                let xmin = c.x - boxes[i * 4] * stride_f;
                let ymin = c.y - boxes[i * 4 + 1] * stride_f;
                let xmax = c.x + boxes[i * 4 + 2] * stride_f;
                let ymax = c.y + boxes[i * 4 + 3] * stride_f;

                let mut landmarks = [Point { x: 0.0, y: 0.0 }; 5];
                for (k, lm) in landmarks.iter_mut().enumerate() {
                    lm.x = c.x + kps[i * 10 + k * 2] * stride_f;
                    lm.y = c.y + kps[i * 10 + k * 2 + 1] * stride_f;
                }

                // Map from the 640x640 letterboxed canvas back to the
                // original captured frame's pixel coordinates.
                let inv = 1.0 / det_scale;
                candidates.push(FaceCandidate {
                    xmin: xmin * inv,
                    ymin: ymin * inv,
                    xmax: xmax * inv,
                    ymax: ymax * inv,
                    score,
                    landmarks: landmarks.map(|p| Point {
                        x: p.x * inv,
                        y: p.y * inv,
                    }),
                });
            }
        }

        Ok(nms(candidates))
    }

    /// Detects faces in `image` and returns the landmarks + bounding box of
    /// the largest one, if any.
    pub fn detect_largest_face(&mut self, image: &RgbImage) -> Result<Option<DetectedFace>> {
        let candidates = self.detect_candidates(image)?;
        let largest = candidates
            .iter()
            .max_by(|a, b| area(a).partial_cmp(&area(b)).unwrap());
        Ok(largest.map(|c| DetectedFace {
            landmarks: c.landmarks.map(|p| (p.x, p.y)),
            bbox: (c.xmin, c.ymin, c.xmax - c.xmin, c.ymax - c.ymin),
        }))
    }

    /// Warps `image` so the given detected landmarks line up with the
    /// canonical ArcFace template, producing a 112x112 aligned face crop,
    /// then runs the embedding model on it and returns an L2-normalized
    /// embedding vector.
    pub fn embed_face(
        &mut self,
        image: &RgbImage,
        landmarks: &[(f32, f32); 5],
    ) -> Result<Vec<f32>> {
        let aligned = align_face(image, landmarks);

        let side = EMB_SIZE as usize;
        let mut input = vec![0f32; 3 * side * side];
        for y in 0..side {
            for x in 0..side {
                let px = aligned.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    let v = (px[c] as f32 - 127.5) / 127.5;
                    input[c * side * side + y * side + x] = v;
                }
            }
        }

        let input_value = Value::from_array(([1usize, 3, side, side], input))?;
        let outputs = self.embedder.run(ort::inputs![input_value])?;
        let (_name, value) = outputs
            .iter()
            .next()
            .context("embedder produced no output")?;
        let (_shape, data) = value.try_extract_tensor::<f32>()?;
        let mut embedding: Vec<f32> = data.to_vec();

        let norm = embedding.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in embedding.iter_mut() {
                *v /= norm;
            }
        }
        Ok(embedding)
    }

    /// Runs the silent anti-spoof classifier on the detector's raw bbox
    /// crop (`x, y, w, h`) and returns a liveness score in `[0, 1]` -- high
    /// means "looks like a real face in front of the camera", low means
    /// "looks like a printed photo or a screen replay". This is a
    /// best-effort signal, not a hard security guarantee: it catches the
    /// common physical-bypass case (holding up a photo or a phone) but
    /// isn't a defense against a determined, well-resourced attacker.
    pub fn check_liveness(&mut self, image: &RgbImage, bbox: (f32, f32, f32, f32)) -> Result<f32> {
        let crop = crop_for_antispoof(image, bbox);
        let resized =
            image::imageops::resize(&crop, ANTISPOOF_SIZE, ANTISPOOF_SIZE, FilterType::Triangle);

        let side = ANTISPOOF_SIZE as usize;
        let mut input = vec![0f32; 3 * side * side];
        for y in 0..side {
            for x in 0..side {
                let px = resized.get_pixel(x as u32, y as u32);
                // BGR, not RGB -- matches the upstream training pipeline.
                for (c, channel) in [px[2], px[1], px[0]].into_iter().enumerate() {
                    input[c * side * side + y * side + x] = channel as f32 / 255.0;
                }
            }
        }

        let input_value = Value::from_array(([1usize, 3, side, side], input))?;
        let outputs = self.antispoof.run(ort::inputs![input_value])?;
        let (_name, value) = outputs
            .iter()
            .next()
            .context("antispoof model produced no output")?;
        let (_shape, logits) = value.try_extract_tensor::<f32>()?;

        let max_logit = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exp: Vec<f32> = logits.iter().map(|v| (v - max_logit).exp()).collect();
        let sum: f32 = exp.iter().sum();
        let probs: Vec<f32> = exp.iter().map(|v| v / sum.max(1e-12)).collect();

        // [live, print-attack, replay-attack]; liveness score is 1 minus
        // the two spoof-class probabilities, equivalently just probs[0].
        Ok(probs.first().copied().unwrap_or(0.0))
    }
}

/// Replicates the exact crop geometry MiniFASNetV2 was trained on: a square
/// region `scale`x the detector bbox's size, centered on the bbox's center,
/// clamped to stay inside the source image (shifting rather than shrinking
/// when a naive centered crop would run off an edge). This is deliberately
/// *not* the rotation-aligned crop `align_face` builds for the embedding
/// model -- feeding this model an aligned crop would test it on an input
/// distribution it never saw during training.
fn crop_for_antispoof(image: &RgbImage, bbox: (f32, f32, f32, f32)) -> RgbImage {
    let (src_w, src_h) = (image.width() as f32, image.height() as f32);
    let (x, y, box_w, box_h) = bbox;

    let scale = ANTISPOOF_CROP_SCALE
        .min((src_w - 1.0) / box_w.max(1.0))
        .min((src_h - 1.0) / box_h.max(1.0));
    let (new_w, new_h) = (box_w * scale, box_h * scale);
    let (center_x, center_y) = (x + box_w / 2.0, y + box_h / 2.0);

    let mut left = center_x - new_w / 2.0;
    let mut top = center_y - new_h / 2.0;
    let mut right = center_x + new_w / 2.0;
    let mut bottom = center_y + new_h / 2.0;

    if left < 0.0 {
        right -= left;
        left = 0.0;
    }
    if top < 0.0 {
        bottom -= top;
        top = 0.0;
    }
    if right > src_w - 1.0 {
        left -= right - (src_w - 1.0);
        right = src_w - 1.0;
    }
    if bottom > src_h - 1.0 {
        top -= bottom - (src_h - 1.0);
        bottom = src_h - 1.0;
    }
    // Final clamp: the shifts above can still leave `left`/`top` negative on
    // a source image too small for even one bbox-sized crop, which would
    // otherwise panic the `imageops::crop_imm` call below.
    left = left.max(0.0);
    top = top.max(0.0);

    let crop_w = ((right - left).max(1.0) as u32).min(image.width());
    let crop_h = ((bottom - top).max(1.0) as u32).min(image.height());
    image::imageops::crop_imm(image, left as u32, top as u32, crop_w, crop_h).to_image()
}

/// Fits the best-fit similarity transform (uniform scale + rotation +
/// translation, no reflection) mapping `landmarks` onto `ARCFACE_TEMPLATE`,
/// by treating each 2D point as a complex number: this turns the usual
/// SVD-based Umeyama alignment into a single linear least-squares solve
/// (`z = sum(conj(p') * q') / sum(|p'|^2)`), avoiding a full linear-algebra
/// dependency for what is a 2D-only problem.
fn align_face(image: &RgbImage, landmarks: &[(f32, f32); 5]) -> RgbImage {
    let (src_mean_x, src_mean_y) = mean_point(landmarks.iter().copied());
    let (dst_mean_x, dst_mean_y) = mean_point(ARCFACE_TEMPLATE.iter().copied());

    let mut num_re = 0f32;
    let mut num_im = 0f32;
    let mut denom = 0f32;
    for i in 0..5 {
        let px = landmarks[i].0 - src_mean_x;
        let py = landmarks[i].1 - src_mean_y;
        let qx = ARCFACE_TEMPLATE[i].0 - dst_mean_x;
        let qy = ARCFACE_TEMPLATE[i].1 - dst_mean_y;
        // conj(p) * q = (px - i py)(qx + i qy)
        num_re += px * qx + py * qy;
        num_im += px * qy - py * qx;
        denom += px * px + py * py;
    }
    let denom = denom.max(1e-6);
    let (z_re, z_im) = (num_re / denom, num_im / denom);
    let t_x = dst_mean_x - (z_re * src_mean_x - z_im * src_mean_y);
    let t_y = dst_mean_y - (z_im * src_mean_x + z_re * src_mean_y);

    // Forward map (source -> template): dst = z * src + t.
    // For sampling we need the inverse: src = (dst - t) / z.
    let z_norm2 = (z_re * z_re + z_im * z_im).max(1e-12);
    let inv_re = z_re / z_norm2;
    let inv_im = -z_im / z_norm2;

    let mut out = RgbImage::new(EMB_SIZE, EMB_SIZE);
    for oy in 0..EMB_SIZE {
        for ox in 0..EMB_SIZE {
            let dx = ox as f32 - t_x;
            let dy = oy as f32 - t_y;
            let sx = inv_re * dx - inv_im * dy;
            let sy = inv_im * dx + inv_re * dy;
            out.put_pixel(ox, oy, sample_bilinear(image, sx, sy));
        }
    }
    out
}

fn mean_point(points: impl Iterator<Item = (f32, f32)>) -> (f32, f32) {
    let mut sx = 0f32;
    let mut sy = 0f32;
    let mut n = 0f32;
    for (x, y) in points {
        sx += x;
        sy += y;
        n += 1.0;
    }
    (sx / n, sy / n)
}

fn sample_bilinear(image: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    if x < 0.0 || y < 0.0 || x >= (w - 1) as f32 || y >= (h - 1) as f32 {
        // Outside the source image: fall back to the nearest clamped pixel
        // rather than leaving black, since landmarks near the frame edge
        // would otherwise punch a hole in the aligned crop.
        let cx = x.clamp(0.0, (w - 1) as f32) as u32;
        let cy = y.clamp(0.0, (h - 1) as f32) as u32;
        return *image.get_pixel(cx, cy);
    }
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let (x0, y0) = (x0 as u32, y0 as u32);
    let p00 = image.get_pixel(x0, y0);
    let p10 = image.get_pixel(x0 + 1, y0);
    let p01 = image.get_pixel(x0, y0 + 1);
    let p11 = image.get_pixel(x0 + 1, y0 + 1);
    let mut out = [0u8; 3];
    for c in 0..3 {
        let top = p00[c] as f32 * (1.0 - fx) + p10[c] as f32 * fx;
        let bottom = p01[c] as f32 * (1.0 - fx) + p11[c] as f32 * fx;
        out[c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
    }
    Rgb(out)
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return -1.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}
