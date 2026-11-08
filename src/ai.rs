//! AI pre-annotation: a background worker that runs OCR adapters, and the rules that turn its
//! raw predictions into editable proposals without ever overwriting human work.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use crate::geom::{self, P};
use crate::ocr::{self, ModelRef, OcrAdapter};
use crate::schema::*;
use crate::util;

#[derive(Clone, Debug)]
pub enum JobKind {
    Page,
    Region { min: P, max: P },
    /// Read one annotation again. `rectified`: flatten `quad` to `target_size` first.
    Recognize { annotation_id: String, quad: [P; 4], target_size: [u32; 2], rectified: bool },
    /// Read an already flattened picture (rectification preview).
    RecognizeImage { annotation_id: String, image: Arc<image::RgbaImage>, quad: [P; 4], target_size: [u32; 2] },
}

#[derive(Clone, Debug)]
pub struct Job {
    pub page_id: String,
    pub image_path: PathBuf,
    pub image_sha256: String,
    pub kind: JobKind,
    pub pinned: Option<ModelRef>,
    pub user: String,
}

pub enum Event {
    Done { page_id: String, run: InferenceRun },
    Failed { page_id: String, error: String },
    ModelReady(ModelRef),
}

pub struct Worker {
    tx: Sender<Job>,
    pub rx: Receiver<Event>,
    pending: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    pub current: Arc<Mutex<String>>,
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
                while let Ok(job) = jobs.recv() {
                    if c2.load(Ordering::SeqCst) {
                        if p2.fetch_sub(1, Ordering::SeqCst) == 1 {
                            c2.store(false, Ordering::SeqCst);
                        }
                        continue;
                    }
                    *cur2.lock().unwrap() = job.page_id.clone();
                    let need_load = match (&adapter, &job.pinned) {
                        (None, _) => true,
                        (Some(a), Some(p)) => a.model() != p,
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
                            Ok(run) => Event::Done { page_id: job.page_id.clone(), run },
                            Err(error) => Event::Failed { page_id: job.page_id.clone(), error },
                        };
                        let _ = etx.send(ev);
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
                    status: PredictionStatus::Proposed,
                    annotation_id: None,
                    decided: None,
                    reason: String::new(),
                });
            }
        }
        JobKind::Recognize { annotation_id, quad, target_size, rectified } => {
            let img = crate::util::open_image(&job.image_path)?.to_rgba8();
            let crop = if *rectified {
                geom::warp_quad(&img, quad, target_size[0], target_size[1]).ok_or("could not flatten")?
            } else {
                let (lo, hi) = geom::bounds(quad);
                geom::crop(&img, lo, hi).ok_or("empty crop")?
            };
            let transform = if *rectified { "rectified" } else { "axis_aligned_crop" };
            scope = RunScope::Annotation { annotation_id: annotation_id.clone(), transform: transform.into(), quad: *quad, target_size: *target_size };
            let r = a.recognize(&crop)?;
            predictions.push(prediction_single(*quad, r));
        }
        JobKind::RecognizeImage { annotation_id, image, quad, target_size } => {
            scope = RunScope::Annotation { annotation_id: annotation_id.clone(), transform: "rectified".into(), quad: *quad, target_size: *target_size };
            let r = a.recognize(image)?;
            predictions.push(prediction_single(*quad, r));
        }
    }
    Ok(InferenceRun {
        id: util::new_id("run"),
        at: util::now(),
        by: job.user.clone(),
        model: a.model().clone(),
        config,
        scope,
        image_sha256: job.image_sha256.clone(),
        predictions,
    })
}

fn prediction_single(quad: [P; 4], r: ocr::Recognition) -> Prediction {
    Prediction {
        index: 0,
        quad,
        text: r.text,
        detection_score: None,
        recognition_score: Some((r.score * 1e4).round() / 1e4),
        status: PredictionStatus::Alternative,
        annotation_id: None,
        decided: None,
        reason: String::new(),
    }
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

/// Turn a finished run into proposals. Existing annotations are never replaced: overlapping
/// predictions are kept in the run as `skipped_overlap`, re-read text becomes an alternative.
pub fn apply_run(page: &mut PageFile, mut run: InferenceRun, cfg: &ProjectConfig) -> Applied {
    let mut res = Applied { added: 0, skipped: 0, alternatives: 0 };
    let author = ai_author(&run.model);
    let now = run.at.clone();
    if let RunScope::Annotation { annotation_id, .. } = &run.scope {
        let id = annotation_id.clone();
        if let (Some(p), Some(a)) = (run.predictions.get_mut(0), page.annotations.iter_mut().find(|a| a.id == id)) {
            p.annotation_id = Some(id.clone());
            let empty = a.text().trim().is_empty();
            let approved = a.review.approvals.iter().any(|x| x.invalidated.is_none()) || a.review.gold.is_some();
            if empty && !approved && !p.text.is_empty() {
                let t = a.transcription.get_or_insert_with(|| default_transcription(cfg, &p.quad));
                t.text = p.text.clone();
                a.provenance.fields.insert("transcription".into(), FieldState::Proposed);
                a.modified = Stamp { by: author.clone(), at: now.clone() };
                a.revision += 1;
                p.status = PredictionStatus::Proposed;
            } else if !p.text.is_empty() && a.text() != p.text {
                let t = a.transcription.get_or_insert_with(|| default_transcription(cfg, &p.quad));
                if !t.alternatives.iter().any(|x| x.text == p.text) {
                    t.alternatives.push(Alternative { text: p.text.clone(), by: author.clone(), at: now.clone(), note: format!("run {}", run.id) });
                }
                res.alternatives += 1;
            }
        }
        page.inference_runs.push(run);
        return res;
    }
    for p in run.predictions.iter_mut() {
        let b = geom::bounds(&p.quad);
        let clash = page.annotations.iter().any(|a| matches!(a.level, Level::Line | Level::Word | Level::Region) && overlap(a.geometry.bounds(), b) > 0.5);
        if clash {
            p.status = PredictionStatus::SkippedOverlap;
            res.skipped += 1;
            continue;
        }
        let id = util::new_id("a");
        let mut a = Annotation::new(id.clone(), Level::Line, &page.page_id, crate::assist::normalize_shape(Geometry::Quad { points: p.quad }), &author, &now);
        let mut t = default_transcription(cfg, &p.quad);
        t.text = p.text.clone();
        a.transcription = Some(t);
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
            fields: [("geometry".to_string(), FieldState::Proposed), ("transcription".to_string(), FieldState::Proposed)].into_iter().collect(),
            heuristic: None,
        };
        a.review.state = if cfg.state_enabled(ReviewState::AiProposed) { ReviewState::AiProposed } else { ReviewState::Draft };
        // put it inside the smallest container or region that holds its centre
        let c = [(b.0[0] + b.1[0]) / 2.0, (b.0[1] + b.1[1]) / 2.0];
        a.parent = page
            .annotations
            .iter()
            .filter(|x| matches!(x.level, Level::Container | Level::Region | Level::Panel) && x.geometry.contains(c, 0.0))
            .min_by(|x, y| x.geometry.area().total_cmp(&y.geometry.area()))
            .map(|x| x.id.clone());
        p.annotation_id = Some(id);
        page.annotations.push(a);
        res.added += 1;
    }
    page.inference_runs.push(run);
    res
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

/// Character error rate between two strings, counted in grapheme clusters.
pub fn cer(reference: &str, hypothesis: &str) -> f64 {
    use unicode_segmentation::UnicodeSegmentation;
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
