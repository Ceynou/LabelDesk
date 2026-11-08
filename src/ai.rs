//! AI pre-annotation: a background worker that runs model adapters, and the rules that turn its
//! raw predictions into editable proposals without ever overwriting human work.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use unicode_segmentation::UnicodeSegmentation;

use crate::geom::{self, P};
use crate::ocr::{self, ModelRef, OcrAdapter};
use crate::schema::*;
use crate::util;
use crate::zoo::{self, ZooId};

#[derive(Clone, Debug)]
pub enum JobKind {
    Page,
    Region { min: P, max: P },
    /// Read one annotation again. `rectified`: flatten `quad` to `target_size` first.
    Recognize { annotation_id: String, quad: [P; 4], target_size: [u32; 2], rectified: bool },
    /// Read an already flattened picture (rectification preview).
    RecognizeImage { annotation_id: String, image: Arc<image::RgbaImage>, quad: [P; 4], target_size: [u32; 2] },
    /// AnimeText: text blocks of the page.
    Blocks,
    /// Koharu layout: text, sound effects, balloons and panels.
    Layout,
    /// Koharu text masks: which pixels are text.
    TextMask,
    /// charspot on text blocks: lines with their characters inside each block.
    ReadBlocks { blocks: Vec<(String, [P; 4])> },
    /// charspot on one line: its characters (and its text when it has none).
    Characters { annotation_id: String, quad: [P; 4], target_size: [u32; 2], rectified: bool },
}

impl JobKind {
    /// The model this job needs, when it is not PP-OCR.
    pub fn zoo_model(&self, reader: Reader) -> Option<ZooId> {
        match self {
            JobKind::Blocks => Some(ZooId::AnimeText),
            JobKind::Layout => Some(ZooId::KoharuLayout),
            JobKind::TextMask => Some(ZooId::KoharuTextMask),
            JobKind::ReadBlocks { .. } | JobKind::Characters { .. } => Some(ZooId::Charspot),
            JobKind::Recognize { .. } | JobKind::RecognizeImage { .. } if reader == Reader::Charspot => Some(ZooId::Charspot),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub page_id: String,
    pub image_path: PathBuf,
    pub image_sha256: String,
    pub kind: JobKind,
    pub pinned: Option<ModelRef>,
    pub user: String,
    /// Model choices of this computer (variants, where each model runs, runtime paths).
    pub zoo: zoo::Options,
    pub reader: Reader,
}

pub enum Event {
    /// `mask`: the text mask of the page (text mask runs).
    Done { page_id: String, run: InferenceRun, mask: Option<Arc<image::GrayImage>> },
    Failed { page_id: String, error: String },
    ModelReady(ModelRef),
    ZooReady(ZooId, ModelRef),
}

pub struct Worker {
    tx: Sender<Job>,
    pub rx: Receiver<Event>,
    pending: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    pub current: Arc<Mutex<String>>,
}

/// Loaded models other than PP-OCR, kept between jobs while their options stay the same.
#[derive(Default)]
pub struct ZooCache {
    anime: Option<(zoo::Options, zoo::AnimeText)>,
    charspot: Option<(zoo::Options, zoo::Charspot)>,
    layout: Option<(zoo::Options, zoo::KoharuLayout)>,
    mask: Option<(zoo::Options, zoo::KoharuTextMask)>,
}

macro_rules! cached {
    ($slot:expr, $o:expr, $ty:ty, $id:expr, $ready:expr) => {{
        if $slot.as_ref().map(|(k, _)| k != $o).unwrap_or(true) {
            $slot = None;
            let m = <$ty>::load($o)?;
            $ready($id, m.model.clone());
            $slot = Some(($o.clone(), m));
        }
        &mut $slot.as_mut().unwrap().1
    }};
}

impl Worker {
    pub fn start(repaint: impl Fn() + Send + 'static) -> Self {
        let (tx, jobs) = channel::<Job>();
        let (etx, rx) = channel::<Event>();
        let pending = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let current = Arc::new(Mutex::new(String::new()));
        let (p2, c2, cur2) = (pending.clone(), cancel.clone(), current.clone());
        std::thread::Builder::new()
            .name("ocr-worker".into())
            .spawn(move || {
                let mut adapter: Option<Box<dyn OcrAdapter>> = None;
                let mut cache = ZooCache::default();
                while let Ok(job) = jobs.recv() {
                    if c2.load(Ordering::SeqCst) {
                        if p2.fetch_sub(1, Ordering::SeqCst) == 1 {
                            c2.store(false, Ordering::SeqCst);
                        }
                        continue;
                    }
                    *cur2.lock().unwrap() = job.page_id.clone();
                    ocr::set_ort_path(job.zoo.runtime().ort);
                    if job.kind.zoo_model(job.reader).is_some() {
                        let ready = |id: ZooId, m: ModelRef| {
                            let _ = etx.send(Event::ZooReady(id, m));
                        };
                        let ev = match run_zoo(&mut cache, &job, &ready) {
                            Ok((run, mask)) => Event::Done { page_id: job.page_id.clone(), run, mask },
                            Err(error) => Event::Failed { page_id: job.page_id.clone(), error },
                        };
                        let _ = etx.send(ev);
                    } else {
                        let need_load = match (&adapter, &job.pinned) {
                            (None, _) => true,
                            (Some(a), Some(p)) => a.model().artifacts != p.artifacts || a.model().revision != p.revision,
                            _ => false,
                        };
                        if need_load {
                            adapter = None;
                            match ocr::load_adapter(job.pinned.as_ref()) {
                                Ok(a) => {
                                    let _ = etx.send(Event::ModelReady(a.model().clone()));
                                    adapter = Some(a);
                                }
                                Err(e) => {
                                    let _ = etx.send(Event::Failed { page_id: job.page_id.clone(), error: e });
                                }
                            }
                        }
                        if let Some(a) = adapter.as_mut() {
                            let ev = match run_job(a.as_mut(), &job) {
                                Ok(run) => Event::Done { page_id: job.page_id.clone(), run, mask: None },
                                Err(error) => Event::Failed { page_id: job.page_id.clone(), error },
                            };
                            let _ = etx.send(ev);
                        }
                    }
                    cur2.lock().unwrap().clear();
                    p2.fetch_sub(1, Ordering::SeqCst);
                    repaint();
                }
            })
            .expect("worker thread");
        Worker { tx, rx, pending, cancel, current }
    }

    pub fn submit(&self, job: Job) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let _ = self.tx.send(job);
    }

    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Drop queued jobs (the one running finishes).
    pub fn cancel(&self) {
        if self.pending() > 0 {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

/// Run a job synchronously (the worker uses this; tests call it directly).
pub fn run_job(a: &mut dyn OcrAdapter, job: &Job) -> Result<InferenceRun, String> {
    let mut config = a.config();
    let mut predictions = Vec::new();
    let scope;
    match &job.kind {
        JobKind::Page | JobKind::Region { .. } => {
            let img = crate::util::open_image(&job.image_path)?.to_rgba8();
            let (offset, view) = match &job.kind {
                JobKind::Region { min, max } => {
                    scope = RunScope::Region { min: *min, max: *max };
                    let c = geom::crop(&img, *min, *max).ok_or("empty region")?;
                    ([min[0].floor().max(0.0), min[1].floor().max(0.0)], c)
                }
                _ => {
                    scope = RunScope::Page;
                    ([0.0, 0.0], img.clone())
                }
            };
            config.insert("rec_crop".into(), "axis-aligned bounding box of the detected quadrilateral".into());
            let dets = a.detect(&view)?;
            for (i, d) in dets.iter().enumerate() {
                let quad = d.quad.map(|p| [p[0] + offset[0], p[1] + offset[1]]);
                let (lo, hi) = geom::bounds(&quad);
                let rec = match geom::crop(&img, lo, hi) {
                    Some(c) => a.recognize(&c).ok(),
                    None => None,
                };
                predictions.push(Prediction {
                    index: i,
                    quad: quad.map(|p| [geom::r2(p[0]), geom::r2(p[1])]),
                    text: rec.as_ref().map(|r| r.text.clone()).unwrap_or_default(),
                    detection_score: Some((d.score * 1e4).round() / 1e4),
                    recognition_score: rec.map(|r| (r.score * 1e4).round() / 1e4),
                    ..Default::default()
                });
            }
        }
        JobKind::Recognize { annotation_id, quad, target_size, rectified } => {
            let img = crate::util::open_image(&job.image_path)?.to_rgba8();
            let crop = annotation_crop(&img, quad, *target_size, *rectified)?;
            let transform = if *rectified { "rectified" } else { "axis_aligned_crop" };
            scope = RunScope::Annotation { annotation_id: annotation_id.clone(), transform: transform.into(), quad: *quad, target_size: *target_size };
            let r = a.recognize(&crop)?;
            predictions.push(prediction_single(*quad, r.text, r.score));
        }
        JobKind::RecognizeImage { annotation_id, image, quad, target_size } => {
            scope = RunScope::Annotation { annotation_id: annotation_id.clone(), transform: "rectified".into(), quad: *quad, target_size: *target_size };
            let r = a.recognize(image)?;
            predictions.push(prediction_single(*quad, r.text, r.score));
        }
        _ => return Err("this job needs another model".into()),
    }
    Ok(InferenceRun { id: util::new_id("run"), at: util::now(), by: job.user.clone(), model: a.model().clone(), config, scope, image_sha256: job.image_sha256.clone(), predictions })
}

fn annotation_crop(img: &image::RgbaImage, quad: &[P; 4], target: [u32; 2], rectified: bool) -> Result<image::RgbaImage, String> {
    if rectified {
        geom::warp_quad(img, quad, target[0], target[1]).ok_or_else(|| "could not flatten".to_string())
    } else {
        let (lo, hi) = geom::bounds(quad);
        geom::crop(img, lo, hi).ok_or_else(|| "empty crop".to_string())
    }
}

fn prediction_single(quad: [P; 4], text: String, score: f64) -> Prediction {
    Prediction { index: 0, quad, text, recognition_score: Some((score * 1e4).round() / 1e4), status: PredictionStatus::Alternative, ..Default::default() }
}

fn box_quad(b: &[f64; 4]) -> [P; 4] {
    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].map(|p| [geom::r2(p[0]), geom::r2(p[1])])
}

fn r4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

/// Run a job that needs one of the zoo models. Returns the run and, for text masks, the mask.
pub fn run_zoo(cache: &mut ZooCache, job: &Job, ready: &dyn Fn(ZooId, ModelRef)) -> Result<(InferenceRun, Option<Arc<image::GrayImage>>), String> {
    let o = &job.zoo;
    let img = crate::util::open_image(&job.image_path)?.to_rgba8();
    let mut predictions = Vec::new();
    let mut mask = None;
    let (model, mut config, scope) = match &job.kind {
        JobKind::Blocks => {
            let m = cached!(cache.anime, o, zoo::AnimeText, ZooId::AnimeText, ready);
            for (i, f) in m.detect(&img)?.into_iter().enumerate() {
                predictions.push(Prediction { index: i, quad: box_quad(&f.bbox), detection_score: Some(r4(f.score)), level: Some(Level::Region), label: Some(f.label), ..Default::default() });
            }
            (m.model.clone(), m.config(), RunScope::Page)
        }
        JobKind::Layout => {
            let m = cached!(cache.layout, o, zoo::KoharuLayout, ZooId::KoharuLayout, ready);
            let mut found = m.detect(&img)?;
            // panels first, then balloons, then text: parents are proposed before what they hold
            let rank = |l: &str| match l {
                "panel" => 0,
                "bubble" => 1,
                _ => 2,
            };
            found.sort_by(|a, b| rank(&a.label).cmp(&rank(&b.label)).then(b.score.total_cmp(&a.score)));
            for (i, f) in found.into_iter().enumerate() {
                let (level, class) = match f.label.as_str() {
                    "panel" => (Level::Panel, None),
                    "bubble" => (Level::Container, None),
                    "onomatopoeia" => (Level::Region, Some("sfx".to_string())),
                    _ => (Level::Region, None),
                };
                predictions.push(Prediction { index: i, quad: box_quad(&f.bbox), detection_score: Some(r4(f.score)), level: Some(level), class, label: Some(f.label), polygon: f.polygon, ..Default::default() });
            }
            (m.model.clone(), m.config(), RunScope::Page)
        }
        JobKind::TextMask => {
            let m = cached!(cache.mask, o, zoo::KoharuTextMask, ZooId::KoharuTextMask, ready);
            let mk = m.mask(&img)?;
            let set = mk.pixels().filter(|p| p[0] > 0).count();
            let mut c = m.config();
            c.insert("text_pixels".into(), (set as u64).into());
            mask = Some(Arc::new(mk));
            (m.model.clone(), c, RunScope::Page)
        }
        JobKind::ReadBlocks { blocks } => {
            let m = cached!(cache.charspot, o, zoo::Charspot, ZooId::Charspot, ready);
            let mut i = 0usize;
            for (block_id, quad) in blocks {
                let (lo, hi) = geom::bounds(quad);
                let Some(crop) = geom::crop(&img, lo, hi) else { continue };
                let off = [lo[0].floor().max(0.0), lo[1].floor().max(0.0)];
                let chars = m.read(&crop)?;
                let (vertical, lines) = zoo::group_lines(&chars);
                for (li, line) in lines.iter().enumerate() {
                    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                    for &k in line {
                        let c = chars[k].bbox;
                        b = [b[0].min(c[0]), b[1].min(c[1]), b[2].max(c[2]), b[3].max(c[3])];
                    }
                    let lb = [b[0] + off[0] - 1.0, b[1] + off[1] - 1.0, b[2] + off[0] + 1.0, b[3] + off[1] + 1.0];
                    let text: String = line.iter().map(|&k| chars[k].text.as_str()).collect();
                    let score = line.iter().map(|&k| chars[k].score).sum::<f64>() / line.len() as f64;
                    let line_index = i;
                    predictions.push(Prediction {
                        index: i,
                        quad: box_quad(&lb),
                        text,
                        recognition_score: Some(r4(score)),
                        level: Some(Level::Line),
                        label: Some(format!("line {} of block, {}", li + 1, if vertical { "vertical" } else { "horizontal" })),
                        parent_id: Some(block_id.clone()),
                        ..Default::default()
                    });
                    i += 1;
                    for &k in line {
                        let c = &chars[k];
                        let cb = [c.bbox[0] + off[0], c.bbox[1] + off[1], c.bbox[2] + off[0], c.bbox[3] + off[1]];
                        predictions.push(Prediction { index: i, quad: box_quad(&cb), text: c.text.clone(), detection_score: Some(r4(c.score)), level: Some(Level::Char), parent: Some(line_index), ..Default::default() });
                        i += 1;
                    }
                }
            }
            let mut c = m.config();
            c.insert("lines".into(), "a new line starts where the next character jumps sideways by more than 0.6 of a character".into());
            (m.model.clone(), c, RunScope::Page)
        }
        JobKind::Characters { annotation_id, quad, target_size, rectified } => {
            let m = cached!(cache.charspot, o, zoo::Charspot, ZooId::Charspot, ready);
            let crop = annotation_crop(&img, quad, *target_size, *rectified)?;
            let (cw, ch) = crop.dimensions();
            let (lo, _) = geom::bounds(quad);
            let to_page: Box<dyn Fn(P) -> P> = if *rectified {
                let rect = [[0.0, 0.0], [cw as f64, 0.0], [cw as f64, ch as f64], [0.0, ch as f64]];
                let h = geom::homography(&rect, quad).ok_or("could not map the line")?;
                Box::new(move |p| geom::apply_h(&h, p))
            } else {
                let off = [lo[0].floor().max(0.0), lo[1].floor().max(0.0)];
                Box::new(move |p| [p[0] + off[0], p[1] + off[1]])
            };
            for (i, c) in m.read(&crop)?.into_iter().enumerate() {
                let b = c.bbox;
                let q = [to_page([b[0], b[1]]), to_page([b[2], b[1]]), to_page([b[2], b[3]]), to_page([b[0], b[3]])].map(|p| [geom::r2(p[0]), geom::r2(p[1])]);
                predictions.push(Prediction { index: i, quad: q, text: c.text, detection_score: Some(r4(c.score)), level: Some(Level::Char), parent_id: Some(annotation_id.clone()), ..Default::default() });
            }
            let transform = if *rectified { "rectified" } else { "axis_aligned_crop" };
            (m.model.clone(), m.config(), RunScope::Annotation { annotation_id: annotation_id.clone(), transform: transform.into(), quad: *quad, target_size: *target_size })
        }
        JobKind::Recognize { annotation_id, quad, target_size, rectified } => {
            let m = cached!(cache.charspot, o, zoo::Charspot, ZooId::Charspot, ready);
            let crop = annotation_crop(&img, quad, *target_size, *rectified)?;
            let chars = m.read(&crop)?;
            let text: String = chars.iter().map(|c| c.text.as_str()).collect();
            let score = if chars.is_empty() { 0.0 } else { chars.iter().map(|c| c.score).sum::<f64>() / chars.len() as f64 };
            predictions.push(prediction_single(*quad, text, score));
            let transform = if *rectified { "rectified" } else { "axis_aligned_crop" };
            (m.model.clone(), m.config(), RunScope::Annotation { annotation_id: annotation_id.clone(), transform: transform.into(), quad: *quad, target_size: *target_size })
        }
        JobKind::RecognizeImage { annotation_id, image, quad, target_size } => {
            let m = cached!(cache.charspot, o, zoo::Charspot, ZooId::Charspot, ready);
            let chars = m.read(image)?;
            let text: String = chars.iter().map(|c| c.text.as_str()).collect();
            let score = if chars.is_empty() { 0.0 } else { chars.iter().map(|c| c.score).sum::<f64>() / chars.len() as f64 };
            predictions.push(prediction_single(*quad, text, score));
            (m.model.clone(), m.config(), RunScope::Annotation { annotation_id: annotation_id.clone(), transform: "rectified".into(), quad: *quad, target_size: *target_size })
        }
        JobKind::Page | JobKind::Region { .. } => return Err("this job runs with PP-OCR".into()),
    };
    config.insert("runtime".into(), model.runtime.clone().into());
    let run = InferenceRun { id: util::new_id("run"), at: util::now(), by: job.user.clone(), model, config, scope, image_sha256: job.image_sha256.clone(), predictions };
    Ok((run, mask))
}

pub fn ai_author(m: &ModelRef) -> String {
    format!("ai:{}@{}", m.name, m.version)
}

/// Overlap measure that also catches a small box inside a big one.
fn overlap(a: (P, P), b: (P, P)) -> f64 {
    let ix = (a.1[0].min(b.1[0]) - a.0[0].max(b.0[0])).max(0.0);
    let iy = (a.1[1].min(b.1[1]) - a.0[1].max(b.0[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: (P, P)| ((r.1[0] - r.0[0]) * (r.1[1] - r.0[1])).max(1e-9);
    (inter / area(a).min(area(b))).max(geom::iou(a, b))
}

pub struct Applied {
    pub added: usize,
    pub skipped: usize,
    pub alternatives: usize,
}

/// Levels an existing annotation must be at for a new proposal at `level` to count as a duplicate.
fn clashes_with(level: Level, other: Level) -> bool {
    match level {
        // PP-OCR lines (schema 1.2) also stop at existing regions
        Level::Line => matches!(other, Level::Line | Level::Word),
        l => l == other,
    }
}

/// Levels that can hold an annotation at `level`, innermost first.
fn parent_levels(level: Level) -> &'static [Level] {
    match level {
        Level::Panel => &[],
        Level::Container => &[Level::Panel],
        Level::Region => &[Level::Container, Level::Panel],
        _ => &[Level::Region, Level::Container, Level::Panel],
    }
}

/// Turn a finished run into proposals. Existing annotations are never replaced: overlapping
/// predictions are kept in the run as `skipped_overlap`, re-read text becomes an alternative.
pub fn apply_run(page: &mut PageFile, mut run: InferenceRun, cfg: &ProjectConfig) -> Applied {
    let mut res = Applied { added: 0, skipped: 0, alternatives: 0 };
    let author = ai_author(&run.model);
    let now = run.at.clone();
    if let RunScope::Annotation { annotation_id, .. } = &run.scope {
        if run.predictions.iter().all(|p| p.level.is_none()) {
            let id = annotation_id.clone();
            if let (Some(p), Some(a)) = (run.predictions.get_mut(0), page.annotations.iter_mut().find(|a| a.id == id)) {
                p.annotation_id = Some(id.clone());
                res.alternatives += propose_text(a, &p.text, &author, &now, &run.id, cfg, &p.quad);
                if res.alternatives == 0 && !p.text.is_empty() {
                    p.status = PredictionStatus::Proposed;
                }
            }
            page.inference_runs.push(run);
            return res;
        }
    }
    fit_line_text(page, &mut run, &author, &now, cfg, &mut res);
    let mut made: Vec<Option<String>> = vec![None; run.predictions.len()];
    for i in 0..run.predictions.len() {
        let p = run.predictions[i].clone();
        let level = p.level.unwrap_or(Level::Line);
        let b = match &p.polygon {
            Some(poly) => {
                let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
                for q in poly {
                    lo = [lo[0].min(q[0]), lo[1].min(q[1])];
                    hi = [hi[0].max(q[0]), hi[1].max(q[1])];
                }
                (lo, hi)
            }
            None => geom::bounds(&p.quad),
        };
        let parent = match (p.parent, &p.parent_id) {
            (Some(k), _) => match made.get(k).cloned().flatten() {
                Some(id) => Some(id),
                None => {
                    let pr = &mut run.predictions[i];
                    pr.status = PredictionStatus::SkippedOverlap;
                    pr.reason = "the line it belongs to was not added".into();
                    res.skipped += 1;
                    continue;
                }
            },
            (None, Some(id)) if page.get(id).is_some() => Some(id.clone()),
            _ => None,
        };
        let clash = page.annotations.iter().any(|a| {
            let same_family = level != Level::Char || a.parent == parent;
            same_family && (clashes_with(level, a.level) || (p.level.is_none() && a.level == Level::Region)) && overlap(a.geometry.bounds(), b) > 0.5
        });
        if clash {
            let pr = &mut run.predictions[i];
            pr.status = PredictionStatus::SkippedOverlap;
            res.skipped += 1;
            continue;
        }
        let id = util::new_id("a");
        let geometry = match &p.polygon {
            Some(poly) if poly.len() >= 3 => Geometry::Polygon { points: poly.clone() },
            _ => Geometry::Quad { points: p.quad },
        };
        let mut a = Annotation::new(id.clone(), level, &page.page_id, crate::assist::normalize_shape(geometry), &author, &now);
        let mut fields: std::collections::BTreeMap<String, FieldState> = [("geometry".to_string(), FieldState::Proposed)].into_iter().collect();
        if level.is_text() && (!p.text.is_empty() || p.level.is_none()) {
            let mut t = default_transcription(cfg, &p.quad);
            if level == Level::Char {
                if let Some(t0) = parent.as_ref().and_then(|pid| page.get(pid)).and_then(|l| l.transcription.clone()) {
                    t.writing_mode = t0.writing_mode;
                    t.direction = t0.direction;
                    t.language = t0.language.clone();
                    t.script = t0.script.clone();
                }
            }
            t.text = p.text.clone();
            a.transcription = Some(t);
            fields.insert("transcription".into(), FieldState::Proposed);
        }
        if let Some(c) = &p.class {
            a.classes.insert(c.clone());
            fields.insert("classes".into(), FieldState::Proposed);
        }
        a.provenance = Provenance {
            origin: Origin::Ai,
            ai: Some(AiOrigin {
                run_id: run.id.clone(),
                prediction: p.index,
                model: run.model.name.clone(),
                model_version: run.model.version.clone(),
                model_revision: run.model.revision.clone(),
                detection_score: p.detection_score,
                recognition_score: p.recognition_score,
            }),
            fields,
            heuristic: None,
        };
        a.review.state = if cfg.state_enabled(ReviewState::AiProposed) { ReviewState::AiProposed } else { ReviewState::Draft };
        // put it inside the given parent, or the smallest container that holds its centre
        let c = [(b.0[0] + b.1[0]) / 2.0, (b.0[1] + b.1[1]) / 2.0];
        a.parent = parent.clone().or_else(|| {
            let lv = parent_levels(level);
            page.annotations.iter().filter(|x| lv.contains(&x.level) && x.geometry.contains(c, 0.0)).min_by(|x, y| x.geometry.area().total_cmp(&y.geometry.area())).map(|x| x.id.clone())
        });
        if level == Level::Char || p.parent_id.is_some() && level == Level::Line {
            // the model read them in this order
            let n = page.annotations.iter().filter(|x| x.level == level && x.parent == a.parent).count();
            a.reading_order = Some(n as u32 + 1);
            a.reading = Some(ReadingInfo { source: Some(OrderSource::Suggested), ..Default::default() });
            a.provenance.fields.insert("reading_order".into(), FieldState::Inferred);
        }
        run.predictions[i].annotation_id = Some(id.clone());
        made[i] = Some(id);
        page.annotations.push(a);
        res.added += 1;
    }
    page.inference_runs.push(run);
    res
}

/// Fill an empty transcription with `text` as a proposal, or offer it as an alternative.
/// Returns 1 when it became an alternative.
fn propose_text(a: &mut Annotation, text: &str, author: &str, now: &str, run_id: &str, cfg: &ProjectConfig, quad: &[P; 4]) -> usize {
    let empty = a.text().trim().is_empty();
    let approved = a.review.approvals.iter().any(|x| x.invalidated.is_none()) || a.review.gold.is_some();
    if empty && !approved && !text.is_empty() {
        let t = a.transcription.get_or_insert_with(|| default_transcription(cfg, quad));
        t.text = text.to_string();
        a.provenance.fields.insert("transcription".into(), FieldState::Proposed);
        a.modified = Stamp { by: author.into(), at: now.into() };
        a.revision += 1;
        0
    } else if !text.is_empty() && a.text() != text {
        let t = a.transcription.get_or_insert_with(|| default_transcription(cfg, quad));
        if !t.alternatives.iter().any(|x| x.text == text) {
            t.alternatives.push(Alternative { text: text.to_string(), by: author.into(), at: now.into(), note: format!("run {run_id}") });
        }
        1
    } else {
        0
    }
}

/// Characters found for an existing line: when the line already has text with as many
/// characters, the boxes keep the person's characters; an empty line gets the model's reading;
/// a different reading becomes an alternative. Untouched heuristic boxes of the line make way.
fn fit_line_text(page: &mut PageFile, run: &mut InferenceRun, author: &str, now: &str, cfg: &ProjectConfig, res: &mut Applied) {
    let RunScope::Annotation { annotation_id, .. } = &run.scope else { return };
    let id = annotation_id.clone();
    let idx: Vec<usize> = (0..run.predictions.len()).filter(|&i| run.predictions[i].level == Some(Level::Char) && run.predictions[i].parent_id.as_deref() == Some(id.as_str())).collect();
    if idx.is_empty() {
        return;
    }
    page.annotations.retain(|c| !(c.level == Level::Char && c.parent.as_deref() == Some(id.as_str()) && crate::assist::untouched_proposal(c)));
    let model_text: String = idx.iter().map(|&i| run.predictions[i].text.as_str()).collect();
    let Some(line) = page.annotations.iter_mut().find(|a| a.id == id) else { return };
    let own: Vec<String> = line.text().graphemes(true).filter(|g| !g.trim().is_empty()).map(|s| s.to_string()).collect();
    if !own.is_empty() && own.len() == idx.len() {
        for (k, &i) in idx.iter().enumerate() {
            if run.predictions[i].text != own[k] {
                run.predictions[i].reason = format!("the model read “{}”; the line's own character is kept", run.predictions[i].text);
                run.predictions[i].text = own[k].clone();
            }
        }
        if model_text != own.concat() {
            res.alternatives += propose_text(line, &model_text, author, now, &run.id, cfg, &run.predictions[idx[0]].quad);
        }
    } else {
        let q = line.geometry.corners().unwrap_or(run.predictions[idx[0]].quad);
        res.alternatives += propose_text(line, &model_text, author, now, &run.id, cfg, &q);
    }
}

fn default_transcription(cfg: &ProjectConfig, quad: &[P; 4]) -> Transcription {
    let w = (geom::dist(quad[0], quad[1]) + geom::dist(quad[3], quad[2])) / 2.0;
    let h = (geom::dist(quad[0], quad[3]) + geom::dist(quad[1], quad[2])) / 2.0;
    let vertical = h > w * 1.2;
    Transcription {
        language: (!cfg.default_language.is_empty()).then(|| cfg.default_language.clone()),
        script: (!cfg.default_script.is_empty()).then(|| cfg.default_script.clone()),
        writing_mode: if vertical { WritingMode::VerticalRl } else { WritingMode::HorizontalTb },
        direction: if vertical { Direction::Ttb } else if cfg.default_direction == Direction::Rtl { Direction::Rtl } else { Direction::Ltr },
        ..Default::default()
    }
}

/// Attach a page's text mask to its text annotations: each line and region without an ink mask
/// gets the part of the mask inside its box, as a proposal. Returns (annotation id, PNG bytes,
/// offset, size) for the files to write, and updates the annotations' mask references.
pub fn attach_ink(page: &mut PageFile, mask: &image::GrayImage, run_id: &str) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for a in page.annotations.iter_mut() {
        if !matches!(a.level, Level::Line | Level::Region | Level::Word) || a.masks.iter().any(|m| m.role == "ink") {
            continue;
        }
        let (lo, hi) = a.geometry.bounds();
        let Some(b) = zoo::ink_bounds(mask, lo, hi) else { continue };
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let part = image::imageops::crop_imm(mask, b[0], b[1], w, h).to_image();
        // keep only ink inside the annotation's own shape
        let part = image::GrayImage::from_fn(w, h, |x, y| {
            let p = [(b[0] + x) as f64 + 0.5, (b[1] + y) as f64 + 0.5];
            if a.geometry.contains(p, 0.5) { *part.get_pixel(x, y) } else { image::Luma([0]) }
        });
        let mut png = Vec::new();
        if image::DynamicImage::ImageLuma8(part).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
            continue;
        }
        let path = format!("masks/{}-ink.png", a.id);
        let sha = {
            use sha2::Digest;
            format!("{:x}", sha2::Sha256::digest(&png))
        };
        a.masks.push(MaskRef { role: "ink".into(), path: path.clone(), sha256: sha, offset: [b[0], b[1]], size: [w, h] });
        a.provenance.fields.insert("masks".into(), FieldState::Proposed);
        let _ = run_id;
        files.push((path, png));
    }
    files
}

/// Character error rate between two strings, counted in grapheme clusters.
pub fn cer(reference: &str, hypothesis: &str) -> f64 {
    let r: Vec<&str> = reference.graphemes(true).collect();
    let h: Vec<&str> = hypothesis.graphemes(true).collect();
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut cur = vec![i; h.len() + 1];
        for j in 1..=h.len() {
            let sub = prev[j - 1] + usize::from(r[i - 1] != h[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[h.len()] as f64 / r.len() as f64
}
