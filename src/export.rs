//! Exports and dataset releases. Exports only read the project; canonical files are never touched.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::geom;
use crate::review;
use crate::schema::*;
use crate::store::{self, Store};
use crate::util::{self, sha256_bytes};
use crate::validate::{self, Issue};

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseOptions {
    /// Only gold-approved annotations on pages approved for export.
    pub gold_only: bool,
    pub native: bool,
    pub jsonl: bool,
    pub coco: bool,
    pub crops: bool,
    pub exclude_restricted: bool,
    pub note: String,
}

impl Default for ReleaseOptions {
    fn default() -> Self {
        ReleaseOptions { gold_only: false, native: true, jsonl: true, coco: true, crops: true, exclude_restricted: true, note: String::new() }
    }
}

pub struct ReleaseResult {
    pub id: String,
    pub dir: PathBuf,
    pub files: usize,
    pub annotations: usize,
    pub pages: usize,
    pub warnings: usize,
}

const OMISSIONS: &str = r#"# What each export leaves out

The canonical data is `native/` (identical to the project files). Other formats are derived:

* **annotations.jsonl** – one annotation per line with page and split information. Complete,
  except the inference-run records (kept in `native/`) and history.
* **coco.json** – COCO detection/segmentation. Categories are text classes (first class only;
  all classes listed in `attributes.classes`). `bbox` is axis-aligned; `segmentation` is the
  outline. Hierarchy, reading order, transcription, writing mode, quality, provenance and review
  live in each annotation's non-standard `attributes` object; tools that ignore it lose them.
  Rotated boxes become polygons. Rectification homographies and history are not included.
* **crops/** – axis-aligned crops of every annotation with text; **rectified/** – the same
  annotations flattened through their rectification quadrilateral (or their own four corners).
  `crops.jsonl` lists text, language, direction and the transform. Geometry outside the crop,
  relations and review data are not included.
* **eval/** – `transcriptions.jsonl` (normalised references for CER/WER), `detection.jsonl`
  (boxes for precision/recall), `model_comparison.jsonl` (original AI reading vs. final text).
* Masks are copied when present (`masks/`); PAGE XML is planned for a later version.
"#;

fn page_included(store: &Store, page: &PageFile, opt: &ReleaseOptions) -> bool {
    let e = store.entry(&page.page_id);
    if opt.exclude_restricted && e.is_some_and(|e| e.source.restricted) {
        return false;
    }
    !opt.gold_only || page.review.approved_for_export.is_some()
}

fn ann_included(a: &Annotation, opt: &ReleaseOptions) -> bool {
    !opt.gold_only || a.review.gold.is_some()
}

/// Deterministic split: whole works go to one split; declared duplicates follow their original.
pub fn assign_splits(store: &Store) -> BTreeMap<String, &'static str> {
    let cfg = &store.manifest.config.export;
    let mut out = BTreeMap::new();
    let group_of = |id: &str| -> String {
        let mut e = store.entry(id);
        let mut guard = 0;
        while let Some(d) = e.and_then(|e| e.source.duplicate_of.clone()) {
            e = store.entry(&d);
            guard += 1;
            if guard > 16 {
                break;
            }
        }
        match e {
            Some(e) if !e.source.work.is_empty() => format!("work:{}", e.source.work),
            Some(e) if !e.source.series.is_empty() => format!("series:{}", e.source.series),
            Some(e) => format!("page:{}", e.id),
            None => format!("page:{id}"),
        }
    };
    for p in &store.manifest.pages {
        let g = group_of(&p.id);
        let h = sha256_bytes(format!("{}|{g}", cfg.split_seed).as_bytes());
        let x = u64::from_str_radix(&h[..12], 16).unwrap_or(0) as f64 / (1u64 << 48) as f64;
        let s = if x < cfg.split[0] { "train" } else if x < cfg.split[0] + cfg.split[1] { "validation" } else { "test" };
        out.insert(p.id.clone(), s);
    }
    out
}

pub fn normalize(text: &str) -> String {
    // NFKC is approximated by folding full-width ASCII; whitespace is removed.
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c as u32 {
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            0x3000 => ' ',
            _ => c,
        })
        .collect()
}

fn write(dir: &Path, rel: &str, data: &[u8], files: &mut BTreeMap<String, String>) -> Result<(), String> {
    let p = dir.join(rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))?;
    files.insert(rel.to_string(), sha256_bytes(data));
    Ok(())
}

fn level_rank_sorted(page: &PageFile) -> Vec<&Annotation> {
    page.annotations.iter().collect()
}

/// Build a dataset release in `releases/<id>/`. Fails when validation finds blocking errors.
pub fn release(store: &Store, opt: &ReleaseOptions, who: &str) -> Result<ReleaseResult, String> {
    let issues = validate::validate_project(store);
    let included: Vec<&PageFile> = store.pages.iter().filter(|p| page_included(store, p, opt)).collect();
    let inc_ids: Vec<&str> = included.iter().map(|p| p.page_id.as_str()).collect();
    let blocking: Vec<&Issue> = issues.iter().filter(|i| i.severity == Severity::Error && (i.page_id.is_empty() || inc_ids.contains(&i.page_id.as_str()))).collect();
    let blocking: Vec<&Issue> = if opt.gold_only {
        blocking
    } else {
        // drafts may be exported for inspection, but broken data never
        blocking.into_iter().filter(|i| !matches!(i.rule, "gold_policy" | "missing_transcription" | "level_required" | "required_field")).collect()
    };
    if !blocking.is_empty() {
        let mut msg = format!("{} problem(s) block this export:\n", blocking.len());
        for i in blocking.iter().take(8) {
            let _ = writeln!(msg, "• {}: {}", i.page_id, i.message);
        }
        return Err(msg);
    }
    let splits = assign_splits(store);
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    let tmp_id = util::new_id("tmp");
    let tmp = store.root.join("releases").join(&tmp_id);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut n_ann = 0;

    // native: canonical files, filtered when needed
    if opt.native {
        let mut m = store.manifest.clone();
        m.pages.retain(|p| inc_ids.contains(&p.id.as_str()));
        write(&tmp, "native/project.json", to_json(&m).as_bytes(), &mut files)?;
        for p in &included {
            let mut p = (*p).clone();
            p.annotations.retain(|a| ann_included(a, opt));
            let rel = store.entry(&p.page_id).map(|e| e.annotation_file.clone()).unwrap_or_else(|| format!("annotations/{}.json", p.page_id));
            write(&tmp, &format!("native/{rel}"), to_json(&p).as_bytes(), &mut files)?;
        }
        write(&tmp, "native/FORMAT.md", crate::docs::FORMAT_MD.as_bytes(), &mut files)?;
        write(&tmp, "native/schema/page.schema.json", crate::docs::PAGE_SCHEMA.as_bytes(), &mut files)?;
        write(&tmp, "native/schema/project.schema.json", crate::docs::PROJECT_SCHEMA.as_bytes(), &mut files)?;
    }

    // JSONL
    if opt.jsonl {
        let mut out = String::new();
        for p in &included {
            let e = store.entry(&p.page_id);
            for a in level_rank_sorted(p).into_iter().filter(|a| ann_included(a, opt)) {
                let line = json!({
                    "page_id": p.page_id,
                    "image": p.image.path,
                    "image_sha256": p.image.sha256,
                    "image_size": [p.image.width, p.image.height],
                    "work": e.map(|e| e.source.work.clone()).unwrap_or_default(),
                    "split": splits.get(&p.page_id).copied().unwrap_or("train"),
                    "annotation": a,
                });
                out.push_str(&serde_json::to_string(&line).unwrap());
                out.push('\n');
                n_ann += 1;
            }
        }
        write(&tmp, "annotations.jsonl", out.as_bytes(), &mut files)?;
    }

    // COCO
    if opt.coco {
        let cfg = &store.manifest.config;
        let mut cats: Vec<Value> = cfg.text_classes.iter().enumerate().map(|(i, c)| json!({"id": i + 1, "name": c.id, "supercategory": "text"})).collect();
        let none_id = cats.len() + 1;
        cats.push(json!({"id": none_id, "name": "unclassified", "supercategory": "text"}));
        let mut images = Vec::new();
        let mut anns = Vec::new();
        for (ii, p) in included.iter().enumerate() {
            images.push(json!({"id": ii + 1, "file_name": p.image.path, "width": p.image.width, "height": p.image.height, "labeldesk_page_id": p.page_id, "split": splits.get(&p.page_id)}));
            for a in p.annotations.iter().filter(|a| ann_included(a, opt)) {
                let (lo, hi) = a.geometry.bounds();
                let cat = a.classes.iter().next().and_then(|c| cfg.text_classes.iter().position(|d| &d.id == c)).map(|i| i + 1).unwrap_or(none_id);
                let seg: Vec<f64> = a.geometry.outline().iter().flat_map(|p| [geom::r2(p[0]), geom::r2(p[1])]).collect();
                anns.push(json!({
                    "id": anns.len() + 1,
                    "image_id": ii + 1,
                    "category_id": cat,
                    "bbox": [geom::r2(lo[0]), geom::r2(lo[1]), geom::r2(hi[0] - lo[0]), geom::r2(hi[1] - lo[1])],
                    "area": geom::r2(a.geometry.area()),
                    "segmentation": [seg],
                    "iscrowd": 0,
                    "attributes": {
                        "labeldesk_id": a.id, "level": a.level, "parent": a.parent, "reading_order": a.reading_order,
                        "classes": a.classes, "text": a.text(),
                        "language": a.transcription.as_ref().and_then(|t| t.language.clone()),
                        "writing_mode": a.transcription.as_ref().map(|t| t.writing_mode),
                        "direction": a.transcription.as_ref().map(|t| t.direction),
                        "recognizability": a.recognizability, "quality": a.quality, "orientation": a.orientation,
                        "occlusion": a.occlusion, "origin": a.provenance.origin, "review_state": a.review.state,
                    }
                }));
            }
        }
        let doc = json!({
            "info": {"description": store.manifest.name, "version": SCHEMA_VERSION, "contributor": who},
            "licenses": [],
            "images": images, "annotations": anns, "categories": cats
        });
        write(&tmp, "coco.json", to_json(&doc).as_bytes(), &mut files)?;
    }

    // crops and rectified crops (+ evaluation files)
    let mut eval_tr = String::new();
    let mut eval_det = String::new();
    let mut comparison = String::new();
    {
        let mut crops_index = String::new();
        for p in &included {
            let entry = store.entry(&p.page_id);
            let img = if opt.crops { entry.and_then(|e| crate::util::open_image(&store.image_path(e)).ok()).map(|i| i.to_rgba8()) } else { None };
            for a in p.annotations.iter().filter(|a| ann_included(a, opt)) {
                let (lo, hi) = a.geometry.bounds();
                let _ = writeln!(
                    eval_det,
                    "{}",
                    json!({"page_id": p.page_id, "id": a.id, "level": a.level, "classes": a.classes, "bbox": [lo[0], lo[1], hi[0], hi[1]], "quality": a.quality, "recognizability": a.recognizability})
                );
                if a.text().is_empty() {
                    continue;
                }
                let t = a.transcription.as_ref().unwrap();
                let _ = writeln!(
                    eval_tr,
                    "{}",
                    json!({"page_id": p.page_id, "id": a.id, "level": a.level, "text": t.text, "normalized": normalize(&t.text), "language": t.language, "script": t.script,
                        "classes": a.classes, "quality": a.quality, "recognizability": a.recognizability, "rotation_deg": a.orientation.rotation_deg})
                );
                if let Some(ai) = &a.provenance.ai {
                    let original = p.inference_runs.iter().find(|r| r.id == ai.run_id).and_then(|r| r.predictions.get(ai.prediction)).map(|x| x.text.clone()).unwrap_or_default();
                    let _ = writeln!(
                        comparison,
                        "{}",
                        json!({"page_id": p.page_id, "id": a.id, "model": ai.model, "model_version": ai.model_version, "predicted": original, "final": t.text,
                            "cer": geom::r2(crate::ai::cer(&normalize(&t.text), &normalize(&original)) * 100.0) / 100.0, "review_state": a.review.state})
                    );
                }
                let Some(img) = &img else { continue };
                let crop_rel = format!("crops/{}/{}.png", p.page_id, a.id);
                let mut rec = json!({"page_id": p.page_id, "id": a.id, "text": t.text, "language": t.language, "writing_mode": t.writing_mode, "direction": t.direction, "split": splits.get(&p.page_id), "crop": crop_rel});
                if let Some(c) = geom::crop(img, lo, hi) {
                    let mut buf = Vec::new();
                    c.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).map_err(|e| e.to_string())?;
                    write(&tmp, &crop_rel, &buf, &mut files)?;
                    rec["crop_origin"] = json!([lo[0].floor().max(0.0), lo[1].floor().max(0.0)]);
                }
                let quad = a.rectification.as_ref().map(|r| (r.source, r.target_size)).or_else(|| a.geometry.corners().map(|q| {
                    let (w, h) = geom::proposed_size(&q);
                    (q, [w, h])
                }));
                if let Some((q, size)) = quad {
                    if let Some(f) = geom::warp_quad(img, &q, size[0], size[1]) {
                        let rel = format!("rectified/{}/{}.png", p.page_id, a.id);
                        let mut buf = Vec::new();
                        f.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).map_err(|e| e.to_string())?;
                        write(&tmp, &rel, &buf, &mut files)?;
                        rec["rectified"] = json!(rel);
                        rec["source_quad"] = json!(q);
                        rec["target_size"] = json!(size);
                        rec["homography"] = json!(geom::homography(&q, &[[0.0, 0.0], [size[0] as f64, 0.0], [size[0] as f64, size[1] as f64], [0.0, size[1] as f64]]));
                    }
                }
                let _ = writeln!(crops_index, "{rec}");
            }
            for a in p.annotations.iter().filter(|a| ann_included(a, opt)) {
                for m in &a.masks {
                    if let Ok(b) = std::fs::read(store.root.join(&m.path)) {
                        write(&tmp, &m.path, &b, &mut files)?;
                    }
                }
            }
        }
        if opt.crops {
            write(&tmp, "crops.jsonl", crops_index.as_bytes(), &mut files)?;
        }
    }
    write(&tmp, "eval/transcriptions.jsonl", eval_tr.as_bytes(), &mut files)?;
    write(&tmp, "eval/detection.jsonl", eval_det.as_bytes(), &mut files)?;
    write(&tmp, "eval/model_comparison.jsonl", comparison.as_bytes(), &mut files)?;
    write(&tmp, "OMISSIONS.md", OMISSIONS.as_bytes(), &mut files)?;

    // approvals summary
    let mut by_state: BTreeMap<&str, usize> = BTreeMap::new();
    let mut reviewers: BTreeMap<String, usize> = BTreeMap::new();
    for p in &included {
        for a in p.annotations.iter().filter(|a| ann_included(a, opt)) {
            *by_state.entry(a.review.state.name()).or_default() += 1;
            for r in review::valid_approvals(a) {
                *reviewers.entry(r.by.clone()).or_default() += 1;
            }
        }
    }
    let content_hash = sha256_bytes(serde_json::to_string(&files).unwrap().as_bytes());
    let id = format!("rel-{}-{}", util::now().replace([':', '-'], "").trim_end_matches('Z'), &content_hash[..8]);
    let manifest = json!({
        "release_id": id,
        "created": {"by": who, "at": util::now()},
        "project_id": store.manifest.project_id,
        "project_name": store.manifest.name,
        "schema_version": SCHEMA_VERSION,
        "content_sha256": content_hash,
        "export_configuration": {
            "gold_only": opt.gold_only, "native": opt.native, "jsonl": opt.jsonl, "coco": opt.coco, "crops": opt.crops,
            "exclude_restricted": opt.exclude_restricted, "split_shares": store.manifest.config.export.split,
            "split_seed": store.manifest.config.export.split_seed, "split_grouping": "work, else series, else page; duplicates follow their original",
            "evaluation_normalization": store.manifest.config.export.eval_normalization, "note": opt.note,
        },
        "review_policy": store.manifest.config.review,
        "approval_summary": {"by_state": by_state, "approvals_by_reviewer": reviewers},
        "pages": included.iter().map(|p| {
            let e = store.entry(&p.page_id);
            json!({"page_id": p.page_id, "image": p.image.path, "image_sha256": p.image.sha256, "split": splits.get(&p.page_id),
                "work": e.map(|e| e.source.work.clone()), "license": e.map(|e| e.source.license.clone()),
                "approved_for_export": p.review.approved_for_export, "annotations": p.annotations.iter().filter(|a| ann_included(a, opt)).count()})
        }).collect::<Vec<_>>(),
        "excluded_pages": store.pages.iter().filter(|p| !inc_ids.contains(&p.page_id.as_str())).map(|p| p.page_id.clone()).collect::<Vec<_>>(),
        "models": store.pages.iter().flat_map(|p| p.inference_runs.iter().map(|r| r.model.clone())).fold(Vec::new(), |mut v: Vec<_>, m| { if !v.contains(&m) { v.push(m) } v }),
        "validation_warnings": issues.iter().filter(|i| i.severity == Severity::Warning).count(),
        "files": files,
        "annotation_license": store.manifest.config.annotation_license,
    });
    let mut id = id;
    let mut dir = store.root.join("releases").join(&id);
    let mut k = 2;
    while dir.exists() {
        id = format!("{}-{k}", id.trim_end_matches(&format!("-{}", k - 1)));
        dir = store.root.join("releases").join(&id);
        k += 1;
    }
    let mut manifest = manifest;
    manifest["release_id"] = json!(id);
    std::fs::write(tmp.join("release.json"), to_json(&manifest)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
    Ok(ReleaseResult {
        id,
        dir,
        files: files.len() + 1,
        annotations: n_ann,
        pages: included.len(),
        warnings: issues.iter().filter(|i| i.severity == Severity::Warning).count(),
    })
}

/// Bring annotations from another copy of the project (a folder with `project.json`, a release's
/// `native/` folder, or a single page file). Pages are matched by id or image checksum and merged.
pub fn import_annotations(store: &mut Store, source: &Path) -> Result<String, String> {
    let mut theirs: Vec<PageFile> = Vec::new();
    let mut dirs = vec![source.to_path_buf()];
    if source.is_file() {
        let v: Value = serde_json::from_slice(&std::fs::read(source).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        theirs.push(store::parse_page(v)?);
        dirs.clear();
    }
    for d in dirs {
        let ann = if d.join("annotations").is_dir() { d.join("annotations") } else { d.clone() };
        let rd = std::fs::read_dir(&ann).map_err(|e| format!("{}: {e}", ann.display()))?;
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                let v: Value = serde_json::from_slice(&std::fs::read(e.path()).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                if v.get("format").and_then(|f| f.as_str()) == Some(PAGE_FORMAT) {
                    theirs.push(store::parse_page(v)?);
                }
            }
        }
    }
    let by_sha: HashMap<String, String> = store.manifest.pages.iter().map(|p| (p.sha256.clone(), p.id.clone())).collect();
    let (mut merged, mut conflicts, mut unknown) = (0, 0, 0);
    for t in theirs {
        let id = if store.index_of(&t.page_id).is_some() { Some(t.page_id.clone()) } else { by_sha.get(&t.image.sha256).cloned() };
        let Some(id) = id else {
            unknown += 1;
            continue;
        };
        let i = store.index_of(&id).unwrap();
        let base = store.saved_version(&id).cloned();
        let ours = store.pages[i].clone();
        let (out, c) = store::merge_page(base.as_ref(), &ours, &t, &source.display().to_string());
        if out != ours {
            store.pages[i] = out;
            store.dirty.insert(id.clone());
            merged += 1;
        }
        conflicts += c;
    }
    Ok(format!("{merged} page(s) updated, {conflicts} conflict(s) to settle{}", if unknown > 0 { format!(", {unknown} page(s) not in this project") } else { String::new() }))
}
