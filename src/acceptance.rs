//! Acceptance scenarios from the specification, checked on the data layer.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::schema::*;
use crate::store::{SaveOutcome, Store};
use crate::{ai, export, geom, ocr, review, sample, util, validate};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("labeldesk-acc-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sha_of_dir(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        if e.path().is_file() {
            v.push((e.file_name().to_string_lossy().to_string(), util::sha256_bytes(&std::fs::read(e.path()).unwrap())));
        }
    }
    v.sort();
    v
}

fn sample_store(name: &str) -> (Store, PathBuf) {
    let d = tmp(name);
    let s = sample::create_project(&d.join("proj"), "tester").expect("sample project");
    (s, d)
}

#[test]
fn s1_s2_s3_create_configure_import_one_file_per_page() {
    let d = tmp("s1");
    let mut cfg = ProjectConfig::default();
    cfg.levels.insert(Level::Char, Requirement::Required);
    let mut s = Store::create(&d.join("p"), "Test", cfg, "alice").unwrap();
    // pages in several scripts and directions
    let src = d.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let mut files = Vec::new();
    for p in sample::pages() {
        let f = src.join(p.name);
        p.image.save(&f).unwrap();
        files.push(f);
    }
    let (ids, notes) = s.import_images(&files, true, "alice");
    assert_eq!(ids.len(), files.len(), "{notes:?}");
    s.save_all("alice").unwrap();
    for id in &ids {
        assert!(d.join("p/annotations").join(format!("{id}.json")).exists());
    }
    let reopened = Store::open(&d.join("p")).unwrap();
    assert_eq!(reopened.manifest.config.levels[&Level::Char], Requirement::Required);
    assert_eq!(reopened.pages.len(), ids.len());
    // re-importing the same picture is recognised as a duplicate
    let mut s = reopened;
    let (again, notes) = s.import_images(&files[..1], true, "alice");
    assert!(again.is_empty() && notes[0].contains("identical"));
    s.unlock();
}

#[test]
fn s4_geometry_edits_keep_text_and_text_edits_keep_geometry() {
    let (mut s, _d) = sample_store("s4");
    let policy = s.manifest.config.review.clone();
    let page = &mut s.pages[0];
    for kind in ["box", "quad", "polygon"] {
        let a = page.annotations.iter_mut().find(|a| a.geometry.kind() == kind && (kind == "polygon" || !a.text().is_empty())).unwrap();
        let old = a.clone();
        a.geometry.translate([5.0, -3.0]);
        if let Geometry::Quad { points } = &mut a.geometry {
            points[2][0] += 4.0;
        }
        review::on_edit(&old, a, &policy, "carol", "t");
        assert_eq!(a.transcription, old.transcription, "{kind}: text must survive a geometry edit");
        assert_ne!(a.geometry, old.geometry);
        let old2 = a.clone();
        if a.level.is_text() {
            a.transcription.as_mut().unwrap().text.push('!');
            review::on_edit(&old2, a, &policy, "carol", "t");
            assert_eq!(a.geometry, old2.geometry, "{kind}: geometry must survive a text edit");
        }
    }
}

#[test]
fn s5_flatten_a_sign() {
    let (s, _d) = sample_store("s5");
    let page = &s.pages[0];
    let sign = page.annotations.iter().find(|a| a.classes.contains("signage") && a.level == Level::Container).unwrap();
    let r = sign.rectification.as_ref().unwrap();
    assert_eq!(r.role, QuadRole::Surface);
    let img = image::open(s.image_path(s.entry(&page.page_id).unwrap())).unwrap().to_rgba8();
    // move one corner a little, as a person would, and flatten again
    let mut src = r.source;
    src[1][0] += 2.0;
    let (w, h) = geom::proposed_size(&src);
    let flat = geom::warp_quad(&img, &src, w, h).unwrap();
    assert_eq!(flat.dimensions(), (w, h));
    // the sign board is cream coloured; most of the flattened picture should be
    let cream = flat.pixels().filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 150 && p[2] < 230).count();
    assert!(cream as f64 > 0.35 * (w * h) as f64, "flattened sign does not look flat: {cream} of {}", w * h);
    let hm = r.homography.unwrap();
    let p = geom::apply_h(&hm, r.source[2]);
    assert!((p[0] - r.target_size[0] as f64).abs() < 0.01 && (p[1] - r.target_size[1] as f64).abs() < 0.01);
}

#[test]
fn s6_s7_offline_model_proposes_and_people_correct_or_reject() {
    if ocr::find_candidates().is_empty() {
        eprintln!("model not cached here; skipping");
        return;
    }
    let (mut s, _d) = sample_store("s6");
    let mut adapter = ocr::load_adapter(None).unwrap();
    assert!(adapter.model().version.as_str() >= "v0.2");
    let i = 2; // western page: caption done by hand, the rest empty
    let e = s.manifest.pages.iter().find(|e| e.id == s.pages[i].page_id).unwrap().clone();
    let job = ai::Job { page_id: e.id.clone(), image_path: s.image_path(&e), image_sha256: e.sha256.clone(), kind: ai::JobKind::Page, pinned: None, user: "alice".into() };
    let run = ai::run_job(adapter.as_mut(), &job).unwrap();
    assert!(run.model.artifacts.iter().all(|a| a.sha256.len() == 64));
    let before = s.pages[i].clone();
    let res = ai::apply_run(&mut s.pages[i], run, &s.manifest.config);
    assert!(res.added >= 3, "added {}", res.added);
    assert!(res.skipped >= 1, "the hand-made caption must not be overwritten");
    let caption = before.annotations.iter().find(|a| a.text() == "MEANWHILE...").unwrap();
    assert_eq!(s.pages[i].get(&caption.id), Some(caption));
    let texts: Vec<String> = s.pages[i].annotations.iter().filter(|a| a.provenance.origin == Origin::Ai).map(|a| a.text().to_string()).collect();
    assert!(texts.iter().any(|t| t == "WHERE ARE"), "{texts:?}");

    // correct one proposal, reject another
    let policy = s.manifest.config.review.clone();
    let page = &mut s.pages[i];
    let fix = page.annotations.iter_mut().find(|a| a.text() == "BOOM!").unwrap();
    let old = fix.clone();
    fix.transcription.as_mut().unwrap().text = "BOOM !".into();
    review::on_edit(&old, fix, &policy, "alice", "t");
    assert_eq!(fix.provenance.fields["transcription"], FieldState::Corrected);
    assert_eq!(fix.review.state, ReviewState::Draft);
    let ai_ref = fix.provenance.ai.clone().unwrap();
    let original = page.inference_runs.iter().find(|r| r.id == ai_ref.run_id).unwrap().predictions[ai_ref.prediction].text.clone();
    assert_eq!(original, "BOOM!", "the original prediction stays recorded");
    let reject = page.annotations.iter().find(|a| a.text() == "HOTEL").unwrap().id.clone();
    assert!(review::reject_ai(page, &reject, "false positive test", "alice", "t"));
    let pred = page.inference_runs.last().unwrap().predictions.iter().find(|p| p.annotation_id.as_deref() == Some(&reject)).unwrap();
    assert_eq!(pred.status, PredictionStatus::Rejected);
    assert_eq!(pred.text, "HOTEL");

    // rerunning never duplicates or overwrites
    let run2 = ai::run_job(adapter.as_mut(), &job).unwrap();
    let n = s.pages[i].annotations.len();
    let res2 = ai::apply_run(&mut s.pages[i], run2, &s.manifest.config);
    assert_eq!(res2.added, 1, "only the rejected area comes back as a new proposal");
    assert_eq!(s.pages[i].annotations.len(), n + 1);

    // re-reading a flattened sign gives an alternative, never an overwrite
    let sign = s.pages[0].annotations.iter().find(|a| a.text() == "喫茶店").unwrap().clone();
    let r = sign.rectification.clone().unwrap();
    let e0 = s.entry(&s.pages[0].page_id).unwrap().clone();
    let job = ai::Job { page_id: e0.id.clone(), image_path: s.image_path(&e0), image_sha256: e0.sha256.clone(), kind: ai::JobKind::Recognize { annotation_id: sign.id.clone(), quad: r.source, target_size: r.target_size, rectified: true }, pinned: None, user: "alice".into() };
    let run = ai::run_job(adapter.as_mut(), &job).unwrap();
    assert_eq!(run.predictions[0].text, "喫茶店");
    ai::apply_run(&mut s.pages[0], run, &s.manifest.config);
    assert_eq!(s.pages[0].get(&sign.id).unwrap().text(), "喫茶店");
}

#[test]
fn s8_s9_review_policy_and_rereview() {
    let (mut s, _d) = sample_store("s8");
    let policy = s.manifest.config.review.clone();
    let page = &mut s.pages[0];
    let a = page.annotations.iter_mut().find(|a| a.text() == "いらっしゃいませ" || a.text() == "どうして").unwrap();
    assert_eq!(a.review.state, ReviewState::HumanReviewed, "sample bubble line was approved by the sample reviewer");
    let old = a.clone();
    a.transcription.as_mut().unwrap().text = "どうして…".into();
    review::on_edit(&old, a, &policy, "alice", "t");
    assert!(a.review.needs_rereview);
    assert_ne!(a.review.state, ReviewState::HumanReviewed);
    assert!(review::approve(a, &Dimension::ALL, &policy, "alice", "t").is_err(), "the editor cannot approve their own change");
    review::approve(a, &Dimension::ALL, &policy, "bob", "t").unwrap();
    assert_eq!(a.review.state, ReviewState::HumanReviewed);
    assert!(!a.review.needs_rereview);
}

#[test]
fn s10_s11_release_is_reproducible_and_round_trips() {
    let (mut s, d) = sample_store("s10");
    let canonical_before = sha_of_dir(&s.root.join("annotations"));
    let images_before = sha_of_dir(&s.root.join("images"));
    let manifest_before = std::fs::read(s.root.join("project.json")).unwrap();
    let r1 = export::release(&s, &export::ReleaseOptions::default(), "alice").unwrap();
    assert_eq!(sha_of_dir(&s.root.join("annotations")), canonical_before, "export must not change canonical files");
    assert_eq!(std::fs::read(s.root.join("project.json")).unwrap(), manifest_before);
    assert_eq!(sha_of_dir(&s.root.join("images")), images_before);
    for f in ["release.json", "annotations.jsonl", "coco.json", "crops.jsonl", "OMISSIONS.md", "eval/transcriptions.jsonl", "native/project.json"] {
        assert!(r1.dir.join(f).exists(), "{f} missing");
    }
    let rel: serde_json::Value = serde_json::from_slice(&std::fs::read(r1.dir.join("release.json")).unwrap()).unwrap();
    assert!(rel["files"].as_object().unwrap().len() > 10);
    assert!(rel["pages"].as_array().unwrap().iter().all(|p| p["split"].is_string()));
    // the same work never spans two splits
    let works: BTreeSet<(String, String)> = rel["pages"].as_array().unwrap().iter().map(|p| (p["work"].to_string(), p["split"].to_string())).collect();
    let names: BTreeSet<String> = works.iter().map(|w| w.0.clone()).collect();
    assert_eq!(works.len(), names.len());
    // rectified crop of the sign exists
    assert!(std::fs::read_to_string(r1.dir.join("crops.jsonl")).unwrap().contains("rectified/"));

    // a second release of unchanged data has identical content
    let r2 = export::release(&s, &export::ReleaseOptions::default(), "alice").unwrap();
    let rel2: serde_json::Value = serde_json::from_slice(&std::fs::read(r2.dir.join("release.json")).unwrap()).unwrap();
    assert_eq!(rel["content_sha256"], rel2["content_sha256"]);

    // import the native export into a fresh copy of the project and export again: nothing lost
    let copy = d.join("copy");
    let mut c = sample::create_project(&copy, "tester").unwrap();
    for p in &mut c.pages {
        p.annotations.clear();
    }
    let msg = export::import_annotations(&mut c, &r1.dir.join("native")).unwrap();
    assert!(msg.contains("updated"), "{msg}");
    for (a, b) in s.pages.iter().zip(&c.pages) {
        assert_eq!(a.annotations, b.annotations, "page {} differs after import", a.page_id);
    }
    // conflicting edits are kept for a decision, never dropped
    let mut theirs = s.pages[0].clone();
    theirs.annotations[3].transcription = Some(Transcription { text: "THEIRS".into(), ..Default::default() });
    let mut ours = s.pages[0].clone();
    ours.annotations[3].transcription = Some(Transcription { text: "OURS".into(), ..Default::default() });
    let base = s.pages[0].clone();
    let (m, n) = crate::store::merge_page(Some(&base), &ours, &theirs, "test");
    assert_eq!(n, 1);
    assert_eq!(m.annotations[3].text(), "OURS");
    assert_eq!(m.conflicts[0].theirs.as_ref().unwrap().text(), "THEIRS");
    s.dirty.clear();
}

#[test]
fn s12_manual_work_without_model() {
    let d = tmp("s12");
    let mut s = Store::create(&d.join("p"), "No model", ProjectConfig::default(), "alice").unwrap();
    let f = d.join("x.png");
    image::RgbaImage::from_pixel(200, 100, image::Rgba([255, 255, 255, 255])).save(&f).unwrap();
    let (ids, _) = s.import_images(&[f], true, "alice");
    let mut a = Annotation::new(util::new_id("a"), Level::Line, &ids[0], Geometry::Box { x: 10.0, y: 10.0, w: 50.0, h: 20.0, rotation_deg: 15.0 }, "alice", "t");
    a.transcription = Some(Transcription { text: "مرحبا 👩‍👩‍👧 é".into(), language: Some("ar".into()), script: Some("Arab".into()), direction: Direction::Rtl, ..Default::default() });
    a.transcription.as_mut().unwrap().spans.push(Span { start: 6, end: 7, kind: SpanKind::Uncertain, note: String::new() });
    s.pages[0].annotations.push(a);
    s.dirty.insert(ids[0].clone());
    s.save_all("alice").unwrap();
    let r = Store::open(&s.root).unwrap();
    assert_eq!(r.pages[0].annotations[0].text(), "مرحبا 👩‍👩‍👧 é");
    let issues = validate::validate_page(&r.manifest.config, &r.pages[0], None);
    assert!(!issues.iter().any(|i| i.rule == "span_range"), "the family emoji is one grapheme: {issues:?}");
}

#[test]
fn s13_recovers_after_crash() {
    let (mut s, _d) = sample_store("s13");
    let id = s.pages[1].page_id.clone();
    let mut a = Annotation::new(util::new_id("a"), Level::Line, &id, Geometry::Box { x: 1.0, y: 1.0, w: 30.0, h: 30.0, rotation_deg: 0.0 }, "alice", "t");
    a.transcription = Some(Transcription { text: "未保存".into(), ..Default::default() });
    s.pages[1].annotations.push(a);
    s.dirty.insert(id.clone());
    s.write_recovery(&id);
    let root = s.root.clone();
    drop(s); // no unlock: as if the app crashed
    let r = Store::open(&root).unwrap();
    assert!(r.crashed_before);
    let items = r.pending_recovery();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].page.annotations.last().unwrap().text(), "未保存");
    r.unlock();
}

#[test]
fn s14_conflicting_disk_change_is_not_overwritten() {
    let (mut s, _d) = sample_store("s14");
    let id = s.pages[0].page_id.clone();
    let path = s.page_path(&id);
    let mut other = s.pages[0].clone();
    other.completeness.note = "edited elsewhere".into();
    std::fs::write(&path, to_json(&other)).unwrap();
    s.pages[0].completeness.note = "edited here".into();
    s.dirty.insert(id.clone());
    match s.save_page(&id, "alice").unwrap() {
        SaveOutcome::Conflict(theirs) => assert_eq!(theirs.completeness.note, "edited elsewhere"),
        _ => panic!("expected a conflict"),
    }
    assert!(std::fs::read_to_string(&path).unwrap().contains("edited elsewhere"));
}

#[test]
fn validation_catches_broken_data() {
    let (s, _d) = sample_store("val");
    let mut page = s.pages[0].clone();
    let cfg = &s.manifest.config;
    let base = validate::validate_page(cfg, &page, None);
    assert!(!base.iter().any(|i| i.severity == Severity::Error), "sample page should have no errors: {base:#?}");
    page.annotations[0].geometry = Geometry::Polygon { points: vec![[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]] };
    page.annotations[1].parent = Some("a-missing".into());
    if let Some(a) = page.annotations.iter_mut().find(|a| matches!(a.geometry, Geometry::Quad { .. })) {
        if let Geometry::Quad { points } = &mut a.geometry {
            points.swap(1, 3);
        }
    }
    let rules: BTreeSet<&str> = validate::validate_page(cfg, &page, Some("image missing")).iter().map(|i| i.rule).collect();
    for r in ["self_intersecting", "missing_reference", "quad_order", "missing_asset"] {
        assert!(rules.contains(r), "{r} not detected: {rules:?}");
    }
    // characters are required on signs: removing them is reported
    let mut page = s.pages[0].clone();
    page.annotations.retain(|a| a.level != Level::Char);
    let rules: BTreeSet<&str> = validate::validate_page(cfg, &page, None).iter().map(|i| i.rule).collect();
    assert!(rules.contains("level_required"));
}

#[test]
fn ocr_v03_with_language_model_reads_offline() {
    let cands = ocr::find_candidates();
    let Some(v3) = cands.iter().find(|c| c.version >= (0, 3)) else {
        eprintln!("v0.3 not cached; skipping");
        return;
    };
    assert_eq!(cands[0].version, v3.version, "newest version is offered first");
    assert!(v3.lm.is_some(), "v0.3 comes with its language model");
    let mut a = ocr::load_adapter(None).unwrap();
    assert_eq!(a.model().version, "v0.3");
    assert!(a.model().artifacts.iter().any(|x| x.role == "language_model" && x.sha256.len() == 64));
    assert!(a.config()["rec_decode"].as_str().unwrap().contains("LM"));
    let (s, _d) = sample_store("v03");
    let img = image::open(s.image_path(s.entry(&s.pages[0].page_id).unwrap())).unwrap().to_rgba8();
    let texts: Vec<String> = a.detect(&img).unwrap().iter().filter_map(|d| {
        let (lo, hi) = geom::bounds(&d.quad);
        geom::crop(&img, lo, hi).and_then(|c| a.recognize(&c).ok()).map(|r| r.text)
    }).collect();
    for want in ["どうして", "いらっしゃいませ", "喫茶店"] {
        assert!(texts.iter().any(|t| t == want), "{want} not read: {texts:?}");
    }
}
