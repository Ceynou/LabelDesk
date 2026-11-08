//! OCR model adapters. The app talks to models only through [`OcrAdapter`], so other
//! models (other scripts, character spotting, ...) can be added without touching the dataset format.
//!
//! The bundled adapter runs Kellenok/PP-OCRv6_manga (v0.2 or higher) through ONNX Runtime,
//! resolved from the local Hugging Face cache. Nothing is downloaded.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::geom::{self, P};

pub const PPOCR_REPO: &str = "Kellenok/PP-OCRv6_manga";
pub const PPOCR_ADAPTER: &str = "ppocr_manga_onnx";
pub const MIN_VERSION: (u32, u32) = (0, 2);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    pub role: String,
    /// Path inside the model snapshot.
    pub file: String,
    pub sha256: String,
}

/// Everything needed to identify exactly which model produced a prediction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelRef {
    pub adapter: String,
    pub name: String,
    pub provider: String,
    pub repo: String,
    pub version: String,
    pub revision: String,
    pub artifacts: Vec<Artifact>,
    pub runtime: String,
}

pub struct Detection {
    pub quad: [P; 4],
    pub score: f64,
}

pub struct Recognition {
    pub text: String,
    pub score: f64,
    /// The crop was taller than wide and was turned 90° before reading.
    pub rotated_vertical: bool,
}

pub trait OcrAdapter: Send {
    fn model(&self) -> &ModelRef;
    fn config(&self) -> BTreeMap<String, serde_json::Value>;
    fn detect(&mut self, img: &image::RgbaImage) -> Result<Vec<Detection>, String>;
    fn recognize(&mut self, crop: &image::RgbaImage) -> Result<Recognition, String>;
}

// ---------------------------------------------------------------- model resolution

pub fn hf_cache_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("HF_HUB_CACHE") {
        v.push(PathBuf::from(p));
    }
    if let Some(p) = std::env::var_os("HF_HOME") {
        v.push(PathBuf::from(p).join("hub"));
    }
    if let Some(p) = std::env::var_os("XDG_CACHE_HOME") {
        v.push(PathBuf::from(p).join("huggingface").join("hub"));
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        v.push(PathBuf::from(h).join(".cache").join("huggingface").join("hub"));
    }
    v
}

/// A model version that exists in the local cache.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub snapshot: PathBuf,
    pub revision: String,
    pub version: (u32, u32),
    pub det: PathBuf,
    pub rec: PathBuf,
    pub dict: PathBuf,
    /// Optional character language model (`lm/manga_lm*.npz`, from v0.3).
    pub lm: Option<PathBuf>,
}

impl Candidate {
    pub fn version_str(&self) -> String {
        format!("v{}.{}", self.version.0, self.version.1)
    }
}

fn parse_version(file: &str, prefix: &str) -> Option<(u32, u32)> {
    let rest = file.strip_prefix(prefix)?.strip_suffix(".onnx")?;
    if rest.contains("fp16") {
        return None;
    }
    let (a, b) = rest.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// All usable PP-OCRv6_manga versions (≥ v0.2) in the local cache, newest first.
pub fn find_candidates() -> Vec<Candidate> {
    let mut out = Vec::new();
    for cache in hf_cache_dirs() {
        let repo = cache.join(format!("models--{}", PPOCR_REPO.replace('/', "--")));
        let Ok(snaps) = std::fs::read_dir(repo.join("snapshots")) else { continue };
        for snap in snaps.flatten() {
            let s = snap.path();
            let revision = s.file_name().unwrap().to_string_lossy().to_string();
            let dict = s.join("ppocrv6_dict.txt");
            if !dict.exists() {
                continue;
            }
            let versions = |dir: &str, prefix: &str| -> Vec<((u32, u32), PathBuf)> {
                std::fs::read_dir(s.join(dir))
                    .map(|rd| {
                        rd.flatten()
                            .filter_map(|e| {
                                let n = e.file_name().to_string_lossy().to_string();
                                parse_version(&n, prefix).map(|v| (v, e.path()))
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let lm = std::fs::read_dir(s.join("lm"))
                .map(|rd| {
                    let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "npz")).collect();
                    v.sort();
                    v.pop()
                })
                .ok()
                .flatten();
            let dets = versions("det", "manga_det_v");
            let recs = versions("rec", "manga_rec_v");
            for (v, det) in &dets {
                if *v < MIN_VERSION {
                    continue;
                }
                if let Some((_, rec)) = recs.iter().find(|(rv, _)| rv == v) {
                    out.push(Candidate { snapshot: s.clone(), revision: revision.clone(), version: *v, det: det.clone(), rec: rec.clone(), dict: dict.clone(), lm: lm.clone().filter(|_| *v >= (0, 3)) });
                }
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out.sort_by(|a, b| b.version.cmp(&a.version));
    out
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

pub fn model_ref(c: &Candidate) -> Result<ModelRef, String> {
    let rel = |p: &Path| p.strip_prefix(&c.snapshot).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let art = |role: &str, p: &Path| -> Result<Artifact, String> {
        Ok(Artifact { role: role.into(), file: rel(p), sha256: sha256_file(p).map_err(|e| e.to_string())? })
    };
    Ok(ModelRef {
        adapter: PPOCR_ADAPTER.into(),
        name: "PP-OCRv6_manga".into(),
        provider: "Kellenok (Hugging Face)".into(),
        repo: PPOCR_REPO.into(),
        version: c.version_str(),
        revision: c.revision.clone(),
        artifacts: {
            let mut v = vec![art("detector", &c.det)?, art("recognizer", &c.rec)?, art("dictionary", &c.dict)?];
            if let Some(lm) = &c.lm {
                v.push(art("language_model", lm)?);
            }
            v
        },
        runtime: "ONNX Runtime via ort 2.0.0-rc.15 (CPU)".into(),
    })
}

// ---------------------------------------------------------------- PP-OCRv6_manga

pub struct PpOcrManga {
    model: ModelRef,
    det: Session,
    rec: Session,
    vocab: Vec<String>,
    version: (u32, u32),
    lm: Option<crate::ctc_lm::CtcLm>,
    /// Probabilities and plain reading of the last recognised crop (for checks).
    pub last_probs: Vec<f32>,
    pub last_greedy: String,
    pub det_long_side: f64,
    pub thresh: f32,
    pub box_thresh: f64,
    pub unclip_ratio: f64,
}

static ENV: std::sync::OnceLock<Result<ort::environment::Environment, String>> = std::sync::OnceLock::new();

impl PpOcrManga {
    /// Load a specific cached version. `expect` pins the artifacts: if a file's hash differs,
    /// loading fails instead of silently using another model.
    pub fn load(c: &Candidate, expect: Option<&ModelRef>) -> Result<Self, String> {
        let model = model_ref(c)?;
        if let Some(e) = expect {
            for a in &e.artifacts {
                let now = model.artifacts.iter().find(|b| b.role == a.role);
                if now.map(|b| &b.sha256) != Some(&a.sha256) {
                    return Err(format!("The cached {} file no longer matches the pinned model ({}).", a.role, a.file));
                }
            }
        }
        let env = ENV
            .get_or_init(|| ort::init().with_name("labeldesk").build().map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| e.clone())?;
        let threads = std::thread::available_parallelism().map(|n| n.get().clamp(1, 8)).unwrap_or(4);
        let load = |p: &Path| -> Result<Session, String> {
            Session::builder(env)
                .map_err(|e| e.to_string())?
                .with_intra_threads(threads)
                .map_err(|e| e.to_string())?
                .commit_from_file(p)
                .map_err(|e| format!("{}: {e}", p.display()))
        };
        let det = load(&c.det)?;
        let rec = load(&c.rec)?;
        let dict = std::fs::read_to_string(&c.dict).map_err(|e| e.to_string())?;
        let mut vocab = vec![String::new()];
        vocab.extend(dict.lines().map(|l| l.trim_end_matches(['\r', '\n']).to_string()));
        vocab.push(" ".into());
        let lm = match &c.lm {
            Some(p) => Some(crate::ctc_lm::CtcLm::load(&vocab, p)?),
            None => None,
        };
        Ok(Self { model, det, rec, vocab, version: c.version, lm, last_probs: Vec::new(), last_greedy: String::new(), det_long_side: 960.0, thresh: 0.15, box_thresh: 0.25, unclip_ratio: 1.4 })
    }
}

fn to_bgr_tensor(img: &image::RgbaImage, mean: [f32; 3], std: [f32; 3]) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let plane = (w * h) as usize;
    let mut v = vec![0f32; plane * 3];
    for (i, p) in img.pixels().enumerate() {
        // composite transparent pixels on white, then B, G, R planes like OpenCV
        let a = p[3] as f32 / 255.0;
        let ch = |c: u8| (c as f32 * a + 255.0 * (1.0 - a)) / 255.0;
        let bgr = [ch(p[2]), ch(p[1]), ch(p[0])];
        for k in 0..3 {
            v[k * plane + i] = (bgr[k] - mean[k]) / std[k];
        }
    }
    v
}

impl PpOcrManga {
    pub fn vocab_len(&self) -> usize {
        self.vocab.len()
    }
}

impl OcrAdapter for PpOcrManga {
    fn model(&self) -> &ModelRef {
        &self.model
    }

    fn config(&self) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert("det_long_side".into(), self.det_long_side.into());
        m.insert("det_strip_rule".into(), "pages longer than 2:1 use the pixel budget of a 3:4 page".into());
        m.insert("det_normalize".into(), "BGR, mean 0.485/0.456/0.406, std 0.229/0.224/0.225".into());
        m.insert("db_thresh".into(), (self.thresh as f64).into());
        m.insert("db_box_thresh".into(), self.box_thresh.into());
        m.insert("db_unclip_ratio".into(), self.unclip_ratio.into());
        m.insert("db_boxes".into(), "minimum-area rotated rectangle of each component".into());
        m.insert("rec_height".into(), 48.into());
        let width = if self.version >= (0, 3) { "natural width at height 48, at least 16 (v0.3 rule)" } else { "natural width clamped to 16..640 (2400 above 2000)" };
        m.insert("rec_width_rule".into(), width.into());
        m.insert("rec_vertical_rule".into(), "crops taller than wide are turned 90° counter-clockwise".into());
        let decode = match &self.lm {
            Some(lm) => format!("CTC prefix beam search with a character {}-gram LM (alpha 0.35, beta 2.0, beam 10, 8 candidates per step); only lines with kana; greedy kept for punctuation, marks, repeats and small/large kana (veto 1mrs)", lm.order()),
            None => "CTC greedy".to_string(),
        };
        m.insert("rec_decode".into(), decode.into());
        m
    }

    fn detect(&mut self, img: &image::RgbaImage) -> Result<Vec<Detection>, String> {
        let (w, h) = img.dimensions();
        let (wf, hf) = (w as f64, h as f64);
        let ls = self.det_long_side;
        let ratio = if wf.max(hf) / wf.min(hf).max(1.0) > 2.0 {
            (0.75 * ls * ls / (wf * hf)).sqrt().min(1.0)
        } else {
            ls / wf.max(hf)
        };
        let rh = (((hf * ratio) / 32.0).round() * 32.0).max(32.0) as u32;
        let rw = (((wf * ratio) / 32.0).round() * 32.0).max(32.0) as u32;
        let resized = image::imageops::resize(img, rw, rh, image::imageops::FilterType::Triangle);
        let data = to_bgr_tensor(&resized, [0.485, 0.456, 0.406], [0.229, 0.224, 0.225]);
        let t = Tensor::from_array(([1usize, 3, rh as usize, rw as usize], data)).map_err(|e| e.to_string())?;
        let outputs = self.det.run(ort::inputs![t]).map_err(|e| e.to_string())?;
        let (_, pred) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let (pw, ph) = (rw as usize, rh as usize);
        if pred.len() < pw * ph {
            return Err("unexpected detector output size".into());
        }
        let pred = &pred[..pw * ph];

        // connected components of the binarised map (8-connectivity)
        let mut label = vec![0u32; pw * ph];
        let mut next = 0u32;
        let mut out = Vec::new();
        let mut stack = Vec::new();
        for start in 0..pw * ph {
            if pred[start] <= self.thresh || label[start] != 0 {
                continue;
            }
            next += 1;
            label[start] = next;
            stack.push(start);
            let mut pts: Vec<P> = Vec::new();
            let mut sum = 0.0f64;
            while let Some(i) = stack.pop() {
                let (x, y) = ((i % pw) as i64, (i / pw) as i64);
                sum += pred[i] as f64;
                pts.push([x as f64, y as f64]);
                for dy in -1..=1i64 {
                    for dx in -1..=1i64 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= pw as i64 || ny >= ph as i64 {
                            continue;
                        }
                        let j = ny as usize * pw + nx as usize;
                        if pred[j] > self.thresh && label[j] == 0 {
                            label[j] = next;
                            stack.push(j);
                        }
                    }
                }
            }
            if pts.len() < 4 {
                continue;
            }
            let mean = sum / pts.len() as f64; // mean score inside the region
            if mean < self.box_thresh {
                continue;
            }
            let hull = geom::convex_hull(pts.clone());
            if hull.len() < 3 {
                continue;
            }
            let area = geom::signed_area(&hull).abs();
            let perim: f64 = (0..hull.len()).map(|i| geom::dist(hull[i], hull[(i + 1) % hull.len()])).sum();
            if area <= 0.0 || perim <= 0.0 {
                continue;
            }
            let d = area * self.unclip_ratio / perim;
            let (c, (bw, bh), ang) = geom::min_area_rect(&hull);
            let (bw, bh) = (bw + 2.0 * d, bh + 2.0 * d);
            if bw.min(bh) < 3.0 {
                continue;
            }
            let mut q = geom::rect_corners(c, (bw, bh), ang);
            for p in &mut q {
                p[0] = (p[0] / rw as f64 * wf).round().clamp(0.0, wf);
                p[1] = (p[1] / rh as f64 * hf).round().clamp(0.0, hf);
            }
            out.push(Detection { quad: geom::order_quad(q), score: sum / pts.len() as f64 });
        }
        Ok(out)
    }

    fn recognize(&mut self, crop: &image::RgbaImage) -> Result<Recognition, String> {
        let (cw, ch) = crop.dimensions();
        if cw == 0 || ch == 0 {
            return Err("empty crop".into());
        }
        let vertical = ch > cw;
        let crop = if vertical { image::imageops::rotate270(crop) } else { crop.clone() };
        let (cw, ch) = crop.dimensions();
        let natural = (48.0 * cw as f64 / ch.max(1) as f64).round() as u32;
        let cap = if self.version >= (0, 3) { 4096 } else if natural > 2000 { 2400 } else { 640 };
        let tw = natural.min(cap).max(16);
        let resized = image::imageops::resize(&crop, tw, 48, image::imageops::FilterType::Triangle);
        let data = to_bgr_tensor(&resized, [0.5; 3], [0.5; 3]);
        let t = Tensor::from_array(([1usize, 3, 48, tw as usize], data)).map_err(|e| e.to_string())?;
        let outputs = self.rec.run(ort::inputs![t]).map_err(|e| e.to_string())?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        let v = *dims.last().unwrap_or(&0) as usize;
        if v == 0 || logits.len() % v != 0 {
            return Err(format!("unexpected recognizer output {dims:?}"));
        }
        let steps = logits.len() / v;
        // per-step probabilities (the exported model ends in softmax; compute it when it does not)
        let (lo, hi) = logits.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)));
        let probs: Vec<f32> = if hi > 1.01 || lo < -0.01 {
            let mut p = logits.to_vec();
            for row in p.chunks_mut(v) {
                let m = row.iter().copied().fold(f32::MIN, f32::max);
                let mut sum = 0.0;
                for x in row.iter_mut() {
                    *x = (*x - m).exp();
                    sum += *x;
                }
                row.iter_mut().for_each(|x| *x /= sum);
            }
            p
        } else {
            logits.to_vec()
        };
        self.last_probs = probs.clone();
        let mut text = String::new();
        let mut kept = Vec::new();
        let mut prev = usize::MAX;
        for t in 0..steps {
            let row = &probs[t * v..(t + 1) * v];
            let (idx, &best) = row.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
            if idx != 0 && idx != prev {
                if let Some(s) = self.vocab.get(idx) {
                    text.push_str(s);
                    kept.push(best as f64);
                }
            }
            prev = idx;
        }
        let score = if kept.is_empty() { 0.0 } else { kept.iter().sum::<f64>() / kept.len() as f64 };
        self.last_greedy = text.clone();
        if let Some(lm) = &self.lm {
            if v == self.vocab.len() {
                text = lm.decode(&probs, v, &text);
            }
        }
        Ok(Recognition { text, score, rotated_vertical: vertical })
    }
}

/// Load the pinned model when given, otherwise the newest cached version ≥ v0.2.
pub fn load_adapter(pinned: Option<&ModelRef>) -> Result<Box<dyn OcrAdapter>, String> {
    let cands = find_candidates();
    if cands.is_empty() {
        return Err(format!(
            "{PPOCR_REPO} (v0.2 or higher) was not found in the local Hugging Face cache ({}). Manual annotation still works.",
            hf_cache_dirs().first().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }
    let chosen = match pinned {
        Some(p) => cands
            .iter()
            .find(|c| c.revision == p.revision && c.version_str() == p.version)
            .ok_or_else(|| format!("The pinned model {} ({}) is no longer in the cache. Choose a model explicitly in Project settings.", p.version, &p.revision[..p.revision.len().min(10)]))?,
        None => &cands[0],
    };
    Ok(Box::new(PpOcrManga::load(chosen, pinned)?))
}
