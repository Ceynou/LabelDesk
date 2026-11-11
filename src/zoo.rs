//! The models beyond PP-OCR: where their files are, and how their pictures go in and their
//! answers come out.
//!
//! | Model | Does | Runs from |
//! |---|---|---|
//! | deepghs/AnimeText_yolo | text blocks | its ONNX files; ncnn after conversion |
//! | muscgab/manga-ocr-charspot-small | reads a crop: every character with its box, in reading order | its ONNX file (processor only) |
//! | mayocream/koharu-layout-highres | text, sound effects, balloons and panels, with masks | ONNX made by the converter; ncnn after conversion |
//! | mayocream/koharu-text-sam-ts-l | which pixels are text | ONNX made by the converter; ncnn after conversion |
//!
//! Every adapter only *proposes*: its output becomes predictions in an inference run, which
//! `ai::apply_run` turns into proposals people review.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::engine::{Backend, Engine, ModelFiles, RuntimePaths, Tensor};
use crate::geom::P;
use crate::ocr::{Artifact, ModelRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZooId {
    AnimeText,
    Charspot,
    KoharuLayout,
    KoharuTextMask,
}

impl ZooId {
    pub const ALL: [ZooId; 4] = [ZooId::AnimeText, ZooId::Charspot, ZooId::KoharuLayout, ZooId::KoharuTextMask];
    pub fn key(self) -> &'static str {
        match self {
            ZooId::AnimeText => "animetext_yolo",
            ZooId::Charspot => "manga_ocr_charspot_small",
            ZooId::KoharuLayout => "koharu_layout_highres",
            ZooId::KoharuTextMask => "koharu_text_sam_ts_l",
        }
    }
    pub fn repo(self) -> &'static str {
        match self {
            ZooId::AnimeText => "deepghs/AnimeText_yolo",
            ZooId::Charspot => "muscgab/manga-ocr-charspot-small",
            ZooId::KoharuLayout => "mayocream/koharu-layout-highres",
            ZooId::KoharuTextMask => "mayocream/koharu-text-sam-ts-l",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ZooId::AnimeText => "AnimeText YOLO",
            ZooId::Charspot => "Manga OCR charspot (small)",
            ZooId::KoharuLayout => "Koharu layout (high resolution)",
            ZooId::KoharuTextMask => "Koharu text masks (SAM-TS-L)",
        }
    }
    pub fn does(self) -> &'static str {
        match self {
            ZooId::AnimeText => "Finds text blocks on a page.",
            ZooId::Charspot => "Reads a text block or line: every character with its own box, in reading order. Japanese.",
            ZooId::KoharuLayout => "Finds text, sound effects, balloons and panels on a page, with their outlines.",
            ZooId::KoharuTextMask => "Marks which pixels of a page are text (masks for occlusion, ink and cleaning).",
        }
    }
    pub fn license(self) -> &'static str {
        match self {
            ZooId::AnimeText => "GPL-3.0",
            ZooId::Charspot => "weights CC BY-NC-SA 4.0 (non-commercial), code Apache-2.0",
            ZooId::KoharuLayout => "Apache-2.0",
            ZooId::KoharuTextMask => "Apache-2.0",
        }
    }
    pub fn capabilities(self) -> &'static [crate::models::Capability] {
        use crate::models::Capability as C;
        match self {
            ZooId::AnimeText => &[C::RegionDetection],
            ZooId::Charspot => &[C::CharDetection, C::Recognition, C::LineDetection],
            ZooId::KoharuLayout => &[C::RegionDetection, C::ContainerDetection, C::PanelDetection],
            ZooId::KoharuTextMask => &[C::TextMask],
        }
    }
    /// Comes as PyTorch weights only: needs the converter before it can run.
    pub fn needs_export(self) -> bool {
        matches!(self, ZooId::KoharuLayout | ZooId::KoharuTextMask)
    }
    /// ncnn can run it after conversion (charspot keeps box decoding and reading order inside
    /// its ONNX graph, which ncnn can't express).
    pub fn ncnn_possible(self) -> bool {
        self != ZooId::Charspot
    }
    /// Input size the ncnn version is converted for.
    pub fn ncnn_input(self, o: &Options) -> u32 {
        match self {
            ZooId::AnimeText => 640,
            ZooId::KoharuLayout => o.layout_resolution,
            _ => 1024,
        }
    }
    /// Rough memory to run once, in MB (for warnings).
    pub fn memory_mb(self, o: &Options) -> u64 {
        match self {
            ZooId::AnimeText => 600,
            ZooId::Charspot => 300,
            ZooId::KoharuLayout => {
                let r = o.layout_resolution as u64;
                900 + r * r / 1500
            }
            ZooId::KoharuTextMask => 3600,
        }
    }
}

pub const ANIME_VARIANTS: [(&str, &str); 5] = [
    ("n", "nano: fastest, F1 0.86"),
    ("s", "small: F1 0.88"),
    ("m", "medium: F1 0.89"),
    ("l", "large: F1 0.89"),
    ("x", "extra large: slowest, F1 0.90"),
];

/// Input sizes the layout model can be converted for (multiples of 24). Its own is 1920.
pub const LAYOUT_RESOLUTIONS: [u32; 4] = [1920, 1536, 1152, 960];

/// Per-computer model choices (kept in the app settings, recorded in every run).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Options {
    #[serde(default = "default_variant")]
    pub anime_variant: String,
    #[serde(default = "default_resolution")]
    pub layout_resolution: u32,
    /// Where each model runs (missing: the processor with ONNX Runtime).
    #[serde(default)]
    pub backends: BTreeMap<ZooId, Backend>,
    /// Graphics card used by ncnn.
    #[serde(default)]
    pub gpu: usize,
    /// Explicit library paths; empty means "find them".
    #[serde(default)]
    pub ort_path: String,
    #[serde(default)]
    pub ncnn_path: String,
    /// Score above which AnimeText's blocks are proposed (empty: the model's own F1 threshold).
    #[serde(default)]
    pub anime_threshold: Option<f32>,
    /// Python virtual environment used to convert PyTorch models (created there if missing).
    #[serde(default = "default_venv")]
    pub python_venv: String,
}

pub fn default_venv() -> String {
    "~/.venv".into()
}

fn default_variant() -> String {
    "m".into()
}
fn default_resolution() -> u32 {
    1920
}

impl Default for Options {
    fn default() -> Self {
        Options { anime_variant: default_variant(), layout_resolution: default_resolution(), backends: BTreeMap::new(), gpu: 0, ort_path: String::new(), ncnn_path: String::new(), anime_threshold: None, python_venv: default_venv() }
    }
}

impl Options {
    pub fn backend(&self, id: ZooId) -> Backend {
        if !id.ncnn_possible() {
            return Backend::Onnx;
        }
        self.backends.get(&id).copied().unwrap_or_default()
    }
    pub fn runtime(&self) -> RuntimePaths {
        let p = |s: &str| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()));
        RuntimePaths { ort: p(&self.ort_path), ncnn: p(&self.ncnn_path), gpu: self.gpu }
    }
    /// Folder name of the converted copy.
    pub fn variant(&self, id: ZooId) -> String {
        match id {
            ZooId::AnimeText => format!("yolo12{}_animetext", self.anime_variant),
            ZooId::KoharuLayout => format!("r{}", self.layout_resolution),
            ZooId::KoharuTextMask => "r1024".into(),
            ZooId::Charspot => "model".into(),
        }
    }
}

// -------------------------------------------------------------------------------- locating

/// A model found in the local Hugging Face cache, with its converted copies.
#[derive(Clone, Debug)]
pub struct Located {
    pub id: ZooId,
    pub snapshot: PathBuf,
    pub revision: String,
    /// Original files (role, path).
    pub sources: Vec<(String, PathBuf)>,
    pub files: ModelFiles,
    /// Folder of LabelDesk's converted copies for this variant.
    pub converted: PathBuf,
}

fn snapshots(repo: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for cache in crate::ocr::hf_cache_dirs() {
        let dir = cache.join(format!("models--{}", repo.replace('/', "--"))).join("snapshots");
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut s: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            // newest download first
            s.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).and_then(|m| m.modified()).ok()));
            v.extend(s);
        }
        if !v.is_empty() {
            break;
        }
    }
    v
}

pub fn converted_dir(id: ZooId, revision: &str, o: &Options) -> PathBuf {
    crate::runtime::data_dir().join("converted").join(id.key()).join(&revision[..revision.len().min(12)]).join(o.variant(id))
}

pub fn locate(id: ZooId, o: &Options) -> Result<Located, String> {
    let need: Vec<(&str, String)> = match id {
        ZooId::AnimeText => vec![("weights", format!("{}/model.onnx", o.variant(id))), ("threshold", format!("{}/threshold.json", o.variant(id)))],
        ZooId::Charspot => vec![("weights", "model.onnx".into()), ("vocabulary", "vocab.json".into()), ("config", "config.json".into())],
        ZooId::KoharuLayout => vec![("weights", "model.safetensors".into()), ("config", "config.json".into())],
        ZooId::KoharuTextMask => vec![("weights", "model.safetensors".into()), ("config", "config.json".into())],
    };
    let snaps = snapshots(id.repo());
    if snaps.is_empty() {
        return Err(format!("{} is not in the local Hugging Face cache. Download it with `hf download {}` (or huggingface-cli), then press Refresh.", id.repo(), id.repo()));
    }
    for s in snaps {
        if !need.iter().all(|(_, f)| s.join(f).exists()) {
            continue;
        }
        let revision = s.file_name().unwrap_or_default().to_string_lossy().to_string();
        let converted = converted_dir(id, &revision, o);
        let mut files = ModelFiles::default();
        if id.needs_export() {
            files.onnx = Some(converted.join("model.onnx")).filter(|p| p.exists());
        } else {
            files.onnx = Some(s.join(&need[0].1));
        }
        let (p, b) = (converted.join("model.ncnn.param"), converted.join("model.ncnn.bin"));
        if p.exists() && b.exists() {
            files.ncnn = Some((p, b));
        }
        let sources = need.iter().map(|(r, f)| (r.to_string(), s.join(f))).collect();
        return Ok(Located { id, snapshot: s, revision, sources, files, converted });
    }
    let what = need.iter().map(|(_, f)| f.as_str()).collect::<Vec<_>>().join(", ");
    Err(format!("{} is in the cache, but not with {what}.", id.repo()))
}

/// The exact files a run used: originals plus the converted copy that ran.
pub fn model_ref(l: &Located, backend: Backend, runtime: &str) -> Result<ModelRef, String> {
    let rel = |p: &Path| p.strip_prefix(&l.snapshot).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.strip_prefix(crate::runtime::data_dir()).map(|r| format!("labeldesk-data/{}", r.to_string_lossy().replace('\\', "/"))).unwrap_or_else(|_| p.display().to_string()));
    let mut artifacts = Vec::new();
    for (role, p) in &l.sources {
        artifacts.push(Artifact { role: role.clone(), file: rel(p), sha256: crate::runtime::sha256_cached(p)? });
    }
    match backend {
        Backend::Onnx => {
            if l.id.needs_export() {
                if let Some(p) = &l.files.onnx {
                    artifacts.push(Artifact { role: "converted_onnx".into(), file: rel(p), sha256: crate::runtime::sha256_cached(p)? });
                }
            }
        }
        _ => {
            if let Some((p, b)) = &l.files.ncnn {
                artifacts.push(Artifact { role: "ncnn_param".into(), file: rel(p), sha256: crate::runtime::sha256_cached(p)? });
                artifacts.push(Artifact { role: "ncnn_bin".into(), file: rel(b), sha256: crate::runtime::sha256_cached(b)? });
            }
        }
    }
    let version = std::fs::read_to_string(l.snapshot.join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| format!("rev {}", &l.revision[..l.revision.len().min(8)]));
    Ok(ModelRef {
        adapter: l.id.key().into(),
        name: l.id.repo().rsplit('/').next().unwrap_or("").into(),
        provider: format!("{} (Hugging Face)", l.id.repo().split('/').next().unwrap_or("")),
        repo: l.id.repo().into(),
        version,
        revision: l.revision.clone(),
        artifacts,
        runtime: runtime.into(),
    })
}

// ------------------------------------------------------------------------- shared helpers

fn rgb(p: &image::Rgba<u8>) -> [f32; 3] {
    // transparent pixels are composited on white
    let a = p[3] as f32 / 255.0;
    [0, 1, 2].map(|k| p[k] as f32 * a + 255.0 * (1.0 - a))
}

/// Planar RGB values of `img`, each mapped by `f`.
fn planar(img: &image::RgbaImage, f: impl Fn(usize, f32) -> f32) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let plane = (w * h) as usize;
    let mut v = vec![0f32; plane * 3];
    for (i, p) in img.pixels().enumerate() {
        let c = rgb(p);
        for k in 0..3 {
            v[k * plane + i] = f(k, c[k]);
        }
    }
    v
}

fn iou(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: &[f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(0.0);
    inter / (area(a) + area(b) - inter).max(1e-9)
}

/// Greedy non-maximum suppression; returns kept indices, best first.
pub fn nms(boxes: &[[f64; 4]], scores: &[f64], thr: f64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..boxes.len()).collect();
    idx.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
    let mut keep: Vec<usize> = Vec::new();
    for i in idx {
        if keep.iter().all(|&k| iou(&boxes[k], &boxes[i]) <= thr) {
            keep.push(i);
        }
    }
    keep
}

/// A shape a model found, in page pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// `[x0, y0, x1, y1]`.
    pub bbox: [f64; 4],
    pub score: f64,
    /// The model's own class name.
    pub label: String,
    /// Outline from the model's mask, when it is not simply the box.
    pub polygon: Option<Vec<P>>,
}

fn read_json(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

// ------------------------------------------------------------------------------ AnimeText

pub struct AnimeText {
    pub engine: Box<dyn Engine>,
    pub model: ModelRef,
    pub size: u32,
    pub threshold: f32,
    pub iou: f64,
}

impl AnimeText {
    pub fn load(o: &Options) -> Result<Self, String> {
        let l = locate(ZooId::AnimeText, o)?;
        let backend = o.backend(ZooId::AnimeText);
        let engine = crate::engine::load(&l.files, backend, &o.runtime())?;
        let model = model_ref(&l, backend, &engine.label())?;
        let own = read_json(&l.snapshot.join(o.variant(ZooId::AnimeText)).join("threshold.json")).and_then(|v| v.get("threshold").and_then(|t| t.as_f64())).unwrap_or(0.3) as f32;
        Ok(AnimeText { engine, model, size: 640, threshold: o.anime_threshold.unwrap_or(own), iou: 0.6 })
    }

    pub fn config(&self) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert("input".into(), format!("letterbox to {0}x{0}, grey 114 padding, RGB / 255", self.size).into());
        m.insert("score_threshold".into(), (self.threshold as f64).into());
        m.insert("nms_iou".into(), self.iou.into());
        m
    }

    pub fn detect(&mut self, img: &image::RgbaImage) -> Result<Vec<Found>, String> {
        let (w, h) = img.dimensions();
        let s = self.size as f64;
        let r = (s / w as f64).min(s / h as f64);
        let (nw, nh) = (((w as f64 * r).round() as u32).max(1), ((h as f64 * r).round() as u32).max(1));
        let (dx, dy) = ((s - nw as f64) / 2.0, (s - nh as f64) / 2.0);
        let (left, top) = ((dx - 0.1).round().max(0.0) as u32, (dy - 0.1).round().max(0.0) as u32);
        let small = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
        let mut canvas = image::RgbaImage::from_pixel(self.size, self.size, image::Rgba([114, 114, 114, 255]));
        image::imageops::overlay(&mut canvas, &small, left as i64, top as i64);
        let data = planar(&canvas, |_, v| v / 255.0);
        let out = self.engine.run(3, self.size as usize, self.size as usize, data)?;
        let t = out.first().ok_or("the model returned nothing")?;
        let d = t.dims();
        let (rows, n, transposed) = match d.as_slice() {
            [5, n] => (5, *n, false),
            [n, 5] => (5, *n, true),
            _ => return Err(format!("unexpected AnimeText output {:?}", t.shape)),
        };
        let at = |k: usize, j: usize| if transposed { t.data[j * rows + k] } else { t.data[k * n + j] };
        let mut boxes = Vec::new();
        let mut scores = Vec::new();
        for j in 0..n {
            let sc = at(4, j);
            if sc < self.threshold {
                continue;
            }
            let (cx, cy, bw, bh) = (at(0, j) as f64, at(1, j) as f64, at(2, j) as f64, at(3, j) as f64);
            let map = |x: f64, pad: f64, max: u32| ((x - pad) / r).clamp(0.0, max as f64);
            boxes.push([map(cx - bw / 2.0, left as f64, w), map(cy - bh / 2.0, top as f64, h), map(cx + bw / 2.0, left as f64, w), map(cy + bh / 2.0, top as f64, h)]);
            scores.push(sc as f64);
        }
        Ok(nms(&boxes, &scores, self.iou)
            .into_iter()
            .take(300)
            .filter(|&i| boxes[i][2] - boxes[i][0] >= 2.0 && boxes[i][3] - boxes[i][1] >= 2.0)
            .map(|i| Found { bbox: boxes[i].map(|v| (v * 100.0).round() / 100.0), score: scores[i], label: "text_block".into(), polygon: None })
            .collect())
    }
}

// ------------------------------------------------------------------------------- charspot

/// One character read by charspot, in crop pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct SpotChar {
    pub text: String,
    pub bbox: [f64; 4],
    pub score: f64,
}

pub struct Charspot {
    pub engine: Box<dyn Engine>,
    pub model: ModelRef,
    vocab: Vec<String>,
}

const CELL: f64 = 16.0;
const BUDGET: f64 = 256.0;

/// Largest scale whose 14-px grid fits the cell budget (the model's own search), as
/// (scale, grid rows, grid columns).
pub fn pad_grid(h: u32, w: u32) -> (f64, u32, u32) {
    let (mut lo, mut hi) = (1e-4f64, 1000.0f64);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if (h as f64 * mid / 14.0).ceil() * (w as f64 * mid / 14.0).ceil() <= BUDGET {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo * CELL / 14.0, (h as f64 * lo / 14.0).ceil() as u32, (w as f64 * lo / 14.0).ceil() as u32)
}

impl Charspot {
    pub fn load(o: &Options) -> Result<Self, String> {
        let l = locate(ZooId::Charspot, o)?;
        let engine = crate::engine::load(&l.files, Backend::Onnx, &o.runtime())?;
        let model = model_ref(&l, Backend::Onnx, &engine.label())?;
        let v = read_json(&l.snapshot.join("vocab.json")).ok_or("vocab.json could not be read")?;
        let mut vocab: Vec<String> = v.get("characters").and_then(|c| c.as_array()).ok_or("vocab.json has no characters")?.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect();
        let n_cls = read_json(&l.snapshot.join("config.json")).and_then(|c| c.get("n_cls").and_then(|x| x.as_u64())).unwrap_or(vocab.len() as u64) as usize;
        vocab.resize(n_cls.max(vocab.len()), String::new());
        // tate-chu-yoko "!?" glyphs follow the characters (the model's own table)
        for a in ["!", "?"] {
            for b in ["!", "?"] {
                vocab.push(format!("{a}{b}"));
            }
        }
        for a in ["!", "?"] {
            for b in ["!", "?"] {
                for c in ["!", "?"] {
                    vocab.push(format!("{a}{b}{c}"));
                }
            }
        }
        Ok(Charspot { engine, model, vocab })
    }

    pub fn config(&self) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert("input".into(), "aspect kept, at most 256 cells of 16 px, bicubic, padded with the median border colour, raw 0-255 RGB".into());
        m.insert("decoding".into(), "inside the model: heat 0.3, NMS 0.2, furigana dropped, learned reading order".into());
        m
    }

    /// Read a crop: characters in reading order, boxes in crop pixels.
    pub fn read(&mut self, crop: &image::RgbaImage) -> Result<Vec<SpotChar>, String> {
        let (w, h) = crop.dimensions();
        if w == 0 || h == 0 {
            return Err("empty crop".into());
        }
        let (s, gh, gw) = pad_grid(h, w);
        let rw = ((w as f64 * s).round() as u32).clamp(1, gw * 16);
        let rh = ((h as f64 * s).round() as u32).clamp(1, gh * 16);
        let flat = image::RgbaImage::from_fn(w, h, |x, y| {
            let c = rgb(crop.get_pixel(x, y));
            image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255])
        });
        let small = image::imageops::resize(&flat, rw, rh, image::imageops::FilterType::CatmullRom);
        let mut border: [Vec<u8>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut push = |p: &image::Rgba<u8>| (0..3).for_each(|k| border[k].push(p[k]));
        for x in 0..rw {
            push(small.get_pixel(x, 0));
            push(small.get_pixel(x, rh - 1));
        }
        for y in 0..rh {
            push(small.get_pixel(0, y));
            push(small.get_pixel(rw - 1, y));
        }
        let median = |v: &mut Vec<u8>| -> u8 {
            v.sort_unstable();
            let n = v.len();
            if n % 2 == 1 { v[n / 2] } else { ((v[n / 2 - 1] as u16 + v[n / 2] as u16) / 2) as u8 }
        };
        let fill = [median(&mut border[0]), median(&mut border[1]), median(&mut border[2])];
        let (cw, ch) = (gw * 16, gh * 16);
        let mut canvas = image::RgbaImage::from_pixel(cw, ch, image::Rgba([fill[0], fill[1], fill[2], 255]));
        image::imageops::overlay(&mut canvas, &small, 0, 0);
        let data = planar(&canvas, |_, v| v);
        let out = self.engine.run(3, ch as usize, cw as usize, data)?;
        if out.len() < 4 {
            return Err("charspot returned fewer outputs than expected".into());
        }
        let (boxes, scores, ids, order) = (&out[0], &out[1], &out[2], &out[3]);
        let n = scores.data.len();
        let (fx, fy) = (w as f64 / rw as f64, h as f64 / rh as f64);
        let mut chars = Vec::new();
        for &k in order.data.iter().take(n) {
            let k = k as usize;
            if k >= n || boxes.data.len() < (k + 1) * 4 {
                continue;
            }
            let b = &boxes.data[k * 4..k * 4 + 4];
            let id = ids.data.get(k).copied().unwrap_or(0.0) as usize;
            let text = self.vocab.get(id).cloned().unwrap_or_default();
            if text.is_empty() {
                continue;
            }
            chars.push(SpotChar {
                text,
                bbox: [(b[0] as f64 * fx).clamp(0.0, w as f64), (b[1] as f64 * fy).clamp(0.0, h as f64), (b[2] as f64 * fx).clamp(0.0, w as f64), (b[3] as f64 * fy).clamp(0.0, h as f64)],
                score: scores.data[k] as f64,
            });
        }
        Ok(chars)
    }
}

/// Split characters in reading order into lines: a new line starts where the next character
/// jumps sideways (to the next column in vertical text, the next row in horizontal text).
pub fn group_lines(chars: &[SpotChar]) -> (bool, Vec<Vec<usize>>) {
    if chars.is_empty() {
        return (false, Vec::new());
    }
    let c = |b: &[f64; 4]| [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
    let mut dx = Vec::new();
    let mut dy = Vec::new();
    for w in chars.windows(2) {
        let (a, b) = (c(&w[0].bbox), c(&w[1].bbox));
        dx.push((b[0] - a[0]).abs());
        dy.push((b[1] - a[1]).abs());
    }
    let med = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        if v.is_empty() { 0.0 } else { v[v.len() / 2] }
    };
    let vertical = if chars.len() == 1 {
        let b = chars[0].bbox;
        b[3] - b[1] > b[2] - b[0]
    } else {
        med(&mut dy.clone()) >= med(&mut dx.clone())
    };
    let mut lines: Vec<Vec<usize>> = vec![vec![0]];
    for i in 1..chars.len() {
        let (a, b) = (&chars[i - 1].bbox, &chars[i].bbox);
        let (ca, cb) = (c(a), c(b));
        let size = if vertical { (a[2] - a[0]).max(b[2] - b[0]) } else { (a[3] - a[1]).max(b[3] - b[1]) };
        let side = if vertical { (cb[0] - ca[0]).abs() } else { (cb[1] - ca[1]).abs() };
        // going back up (vertical) or left (horizontal) also starts a line
        let back = if vertical { cb[1] < a[1] - size * 0.5 } else { cb[0] < a[0] - size * 0.5 };
        if side > size * 0.6 || back {
            lines.push(vec![i]);
        } else {
            lines.last_mut().unwrap().push(i);
        }
    }
    (vertical, lines)
}

// ------------------------------------------------------------------------- Koharu layout

pub struct KoharuLayout {
    pub engine: Box<dyn Engine>,
    pub model: ModelRef,
    pub resolution: u32,
    classes: Vec<String>,
    thresholds: BTreeMap<String, f32>,
    num_select: usize,
}

impl KoharuLayout {
    pub fn load(o: &Options) -> Result<Self, String> {
        let l = locate(ZooId::KoharuLayout, o)?;
        let backend = o.backend(ZooId::KoharuLayout);
        if l.files.onnx.is_none() && l.files.ncnn.is_none() {
            return Err(format!("Koharu layout has not been converted for {} px yet: open AI → Models… and press Convert.", o.layout_resolution));
        }
        let engine = crate::engine::load(&l.files, backend, &o.runtime())?;
        let model = model_ref(&l, backend, &engine.label())?;
        let cfg = read_json(&l.snapshot.join("config.json")).unwrap_or_default();
        let mut classes = vec![String::new(); 4];
        if let Some(m) = cfg.get("classes").and_then(|c| c.as_object()) {
            for (k, v) in m {
                if let (Ok(i), Some(n)) = (k.parse::<usize>(), v.as_str()) {
                    if i >= classes.len() {
                        classes.resize(i + 1, String::new());
                    }
                    classes[i] = n.to_string();
                }
            }
        }
        let thresholds = cfg.get("recommended_thresholds").and_then(|c| c.as_object()).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32))).collect()).unwrap_or_default();
        let num_select = cfg.get("num_select").and_then(|v| v.as_u64()).unwrap_or(160) as usize;
        Ok(KoharuLayout { engine, model, resolution: o.layout_resolution, classes, thresholds, num_select })
    }

    pub fn config(&self) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert("input".into(), format!("page stretched to {0}x{0}, RGB / 255, ImageNet mean and deviation", self.resolution).into());
        m.insert("num_select".into(), (self.num_select as u64).into());
        m.insert("thresholds".into(), serde_json::to_value(&self.thresholds).unwrap_or_default());
        m.insert("outlines".into(), "convex hull of the mask, simplified; boxes when the hull fills its box".into());
        m
    }

    pub fn detect(&mut self, img: &image::RgbaImage) -> Result<Vec<Found>, String> {
        let (w, h) = img.dimensions();
        let r = self.resolution;
        let small = image::imageops::resize(img, r, r, image::imageops::FilterType::Triangle);
        let mean = [0.485f32, 0.456, 0.406];
        let std = [0.229f32, 0.224, 0.225];
        let data = planar(&small, |k, v| (v / 255.0 - mean[k]) / std[k]);
        let out = self.engine.run(3, r as usize, r as usize, data)?;
        if out.len() < 2 {
            return Err("the layout model returned fewer outputs than expected".into());
        }
        let (dets, logits) = (&out[0], &out[1]);
        let masks = out.get(2);
        let dd = dets.dims();
        let q = *dd.first().ok_or("empty boxes")?;
        let ld = logits.dims();
        let ncls = *ld.last().ok_or("empty labels")?;
        let mut cand: Vec<(f32, usize, usize)> = Vec::new();
        for qi in 0..q {
            for c in 0..ncls.min(self.classes.len()) {
                let s = 1.0 / (1.0 + (-logits.data[qi * ncls + c]).exp());
                cand.push((s, qi, c));
            }
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0));
        cand.truncate(self.num_select);
        let (mh, mw) = masks.map(|m| {
            let d = m.dims();
            (d[d.len() - 2], d[d.len() - 1])
        }).unwrap_or((0, 0));
        let mut found = Vec::new();
        for (s, qi, c) in cand {
            let label = self.classes[c].clone();
            let thr = self.thresholds.get(&label).copied().unwrap_or(0.4);
            if label.is_empty() || s < thr {
                continue;
            }
            let b = &dets.data[qi * 4..qi * 4 + 4];
            let (cx, cy, bw, bh) = (b[0] as f64, b[1] as f64, b[2] as f64, b[3] as f64);
            let bbox = [((cx - bw / 2.0) * w as f64).clamp(0.0, w as f64), ((cy - bh / 2.0) * h as f64).clamp(0.0, h as f64), ((cx + bw / 2.0) * w as f64).clamp(0.0, w as f64), ((cy + bh / 2.0) * h as f64).clamp(0.0, h as f64)];
            if bbox[2] - bbox[0] < 2.0 || bbox[3] - bbox[1] < 2.0 {
                continue;
            }
            let polygon = match masks {
                Some(m) if mh > 0 && matches!(label.as_str(), "bubble" | "panel") => {
                    let plane = &m.data[qi * mh * mw..(qi + 1) * mh * mw];
                    mask_outline(plane, mw, mh, w, h, &bbox)
                }
                _ => None,
            };
            found.push(Found { bbox: bbox.map(|v| (v * 100.0).round() / 100.0), score: s as f64, label, polygon });
        }
        // the same thing found twice by two queries
        let boxes: Vec<[f64; 4]> = found.iter().map(|f| f.bbox).collect();
        let scores: Vec<f64> = found.iter().map(|f| f.score).collect();
        let mut keep = Vec::new();
        for i in nms(&boxes, &scores, 1.0) {
            if !keep.iter().any(|&k: &usize| found[k].label == found[i].label && iou(&boxes[k], &boxes[i]) > 0.8) {
                keep.push(i);
            }
        }
        Ok(keep.into_iter().map(|i| found[i].clone()).collect())
    }
}

/// Outline of a low-resolution mask (logits > 0) inside `bbox`, in page pixels: its convex hull,
/// simplified. `None` when the hull nearly fills the box (then the box is the shape).
pub fn mask_outline(plane: &[f32], mw: usize, mh: usize, w: u32, h: u32, bbox: &[f64; 4]) -> Option<Vec<P>> {
    let (sx, sy) = (w as f64 / mw as f64, h as f64 / mh as f64);
    let (x0, x1) = (((bbox[0] / sx).floor() as usize).min(mw), ((bbox[2] / sx).ceil() as usize).min(mw));
    let (y0, y1) = (((bbox[1] / sy).floor() as usize).min(mh), ((bbox[3] / sy).ceil() as usize).min(mh));
    let mut pts = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            if plane[y * mw + x] > 0.0 {
                for (ox, oy) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                    pts.push([((x as f64 + ox) * sx).clamp(bbox[0], bbox[2]), ((y as f64 + oy) * sy).clamp(bbox[1], bbox[3])]);
                }
            }
        }
    }
    if pts.len() < 12 {
        return None;
    }
    let hull = crate::geom::convex_hull(pts);
    if hull.len() < 3 {
        return None;
    }
    let area = crate::geom::signed_area(&hull).abs();
    let box_area = (bbox[2] - bbox[0]) * (bbox[3] - bbox[1]);
    if area >= box_area * 0.93 {
        return None;
    }
    Some(simplify(hull, area * 0.004).into_iter().map(|p| [(p[0] * 100.0).round() / 100.0, (p[1] * 100.0).round() / 100.0]).collect())
}

/// Drop the corners that add least area until each would change it by more than `tol`.
pub fn simplify(mut poly: Vec<P>, tol: f64) -> Vec<P> {
    while poly.len() > 4 {
        let n = poly.len();
        let (i, a) = (0..n)
            .map(|i| {
                let (p, c, q) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
                (i, ((c[0] - p[0]) * (q[1] - p[1]) - (q[0] - p[0]) * (c[1] - p[1])).abs() / 2.0)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        if a > tol && n <= 24 {
            break;
        }
        poly.remove(i);
    }
    poly
}

// ---------------------------------------------------------------------- Koharu text masks

pub struct KoharuTextMask {
    pub engine: Box<dyn Engine>,
    pub model: ModelRef,
}

impl KoharuTextMask {
    pub fn load(o: &Options) -> Result<Self, String> {
        let l = locate(ZooId::KoharuTextMask, o)?;
        let backend = o.backend(ZooId::KoharuTextMask);
        if l.files.onnx.is_none() && l.files.ncnn.is_none() {
            return Err("Koharu text masks have not been converted yet: open AI → Models… and press Convert.".into());
        }
        let engine = crate::engine::load(&l.files, backend, &o.runtime())?;
        let model = model_ref(&l, backend, &engine.label())?;
        Ok(KoharuTextMask { engine, model })
    }

    pub fn config(&self) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert("input".into(), "long side resized to 1024 (bilinear), padded right and bottom with 128 grey, raw 0-255 RGB".into());
        m.insert("output".into(), "high-resolution text logits > 0, padding cropped, resized to the page (nearest)".into());
        m
    }

    /// Which pixels of the page are text (255) or not (0).
    pub fn mask(&mut self, img: &image::RgbaImage) -> Result<image::GrayImage, String> {
        let (w, h) = img.dimensions();
        let scale = 1024.0 / w.max(h) as f64;
        let (rw, rh) = (((w as f64 * scale).round() as u32).clamp(1, 1024), ((h as f64 * scale).round() as u32).clamp(1, 1024));
        let small = image::imageops::resize(img, rw, rh, image::imageops::FilterType::Triangle);
        let mut canvas = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([128, 128, 128, 255]));
        image::imageops::overlay(&mut canvas, &small, 0, 0);
        let out = self.engine.run(3, 1024, 1024, planar(&canvas, |_, v| v))?;
        let t: &Tensor = out.first().ok_or("the mask model returned nothing")?;
        let d = t.dims();
        let (mh, mw) = (d[d.len() - 2], d[d.len() - 1]);
        let small_mask = image::GrayImage::from_fn(rw, rh, |x, y| {
            let (mx, my) = ((x as usize * mw / 1024).min(mw - 1), (y as usize * mh / 1024).min(mh - 1));
            image::Luma([if t.data[my * mw + mx] > 0.0 { 255 } else { 0 }])
        });
        Ok(image::imageops::resize(&small_mask, w, h, image::imageops::FilterType::Nearest))
    }
}

/// Bounding box `[x0, y0, x1, y1]` of the set pixels of `mask` inside `lo..hi`.
pub fn ink_bounds(mask: &image::GrayImage, lo: P, hi: P) -> Option<[u32; 4]> {
    let (w, h) = mask.dimensions();
    let (x0, y0) = (lo[0].floor().clamp(0.0, w as f64) as u32, lo[1].floor().clamp(0.0, h as f64) as u32);
    let (x1, y1) = (hi[0].ceil().clamp(0.0, w as f64) as u32, hi[1].ceil().clamp(0.0, h as f64) as u32);
    let mut b: Option<[u32; 4]> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            if mask.get_pixel(x, y)[0] > 0 {
                b = Some(match b {
                    None => [x, y, x + 1, y + 1],
                    Some(r) => [r[0].min(x), r[1].min(y), r[2].max(x + 1), r[3].max(y + 1)],
                });
            }
        }
    }
    b
}
