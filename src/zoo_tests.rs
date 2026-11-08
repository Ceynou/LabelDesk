//! Tests for the added models: how their output becomes proposals, and (when the models and
//! ONNX Runtime are on this computer) that they really read the sample page.

use crate::ai::{self, JobKind};
use crate::geom::P;
use crate::ocr::ModelRef;
use crate::schema::*;
use crate::zoo::{self, SpotChar};

fn model() -> ModelRef {
    ModelRef { adapter: "test".into(), name: "test-model".into(), provider: "test".into(), repo: "t/t".into(), version: "1".into(), revision: "abc".into(), artifacts: vec![], runtime: "test".into() }
}

fn page() -> PageFile {
    PageFile::new("p1", ImageRef { path: "images/p1.png".into(), sha256: "x".into(), width: 1000, height: 1400 })
}

fn bx(x0: f64, y0: f64, x1: f64, y1: f64) -> [P; 4] {
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn run(scope: RunScope, predictions: Vec<Prediction>) -> InferenceRun {
    InferenceRun { id: "run-1".into(), at: "2026-11-08T00:00:00Z".into(), by: "tester".into(), model: model(), config: Default::default(), scope, image_sha256: "x".into(), predictions }
}

#[test]
fn layout_proposals_land_at_their_levels_inside_each_other() {
    let mut pg = page();
    let preds = vec![
        Prediction { index: 0, quad: bx(10.0, 10.0, 600.0, 700.0), level: Some(Level::Panel), label: Some("panel".into()), ..Default::default() },
        Prediction {
            index: 1,
            quad: bx(100.0, 100.0, 300.0, 400.0),
            level: Some(Level::Container),
            label: Some("bubble".into()),
            polygon: Some(vec![[200.0, 100.0], [300.0, 250.0], [200.0, 400.0], [100.0, 250.0]]),
            ..Default::default()
        },
        Prediction { index: 2, quad: bx(150.0, 150.0, 250.0, 350.0), level: Some(Level::Region), label: Some("text".into()), ..Default::default() },
        Prediction { index: 3, quad: bx(400.0, 400.0, 550.0, 600.0), level: Some(Level::Region), class: Some("sfx".into()), label: Some("onomatopoeia".into()), ..Default::default() },
    ];
    let res = ai::apply_run(&mut pg, run(RunScope::Page, preds), &ProjectConfig::default());
    assert_eq!(res.added, 4);
    let at = |l: Level| pg.annotations.iter().filter(|a| a.level == l).collect::<Vec<_>>();
    let panel = at(Level::Panel)[0].id.clone();
    let balloon = at(Level::Container)[0];
    assert_eq!(balloon.parent.as_deref(), Some(panel.as_str()), "the balloon sits in its panel");
    assert!(matches!(balloon.geometry, Geometry::Polygon { .. } | Geometry::Quad { .. }), "the balloon keeps its outline");
    let regions = at(Level::Region);
    assert_eq!(regions.iter().find(|r| r.classes.is_empty()).unwrap().parent.as_deref(), Some(balloon.id.as_str()), "text sits in its balloon");
    let sfx = regions.iter().find(|r| r.classes.contains("sfx")).unwrap();
    assert_eq!(sfx.parent.as_deref(), Some(panel.as_str()));
    assert!(pg.annotations.iter().all(|a| a.review.state == ReviewState::AiProposed && a.provenance.origin == Origin::Ai), "everything is a proposal");
    assert!(regions.iter().all(|r| r.transcription.is_none()), "detected blocks have no text yet");
    // running again proposes nothing twice
    let again = vec![Prediction { index: 0, quad: bx(12.0, 12.0, 598.0, 698.0), level: Some(Level::Panel), ..Default::default() }];
    let res = ai::apply_run(&mut pg, run(RunScope::Page, again), &ProjectConfig::default());
    assert_eq!((res.added, res.skipped), (0, 1));
}

#[test]
fn read_blocks_makes_lines_with_their_characters_in_order() {
    let mut pg = page();
    let block = Annotation::new("a-block".into(), Level::Region, "p1", Geometry::Box { x: 100.0, y: 100.0, w: 120.0, h: 200.0, rotation_deg: 0.0 }, "tester", "now");
    pg.annotations.push(block);
    let mut preds = vec![Prediction { index: 0, quad: bx(160.0, 100.0, 200.0, 300.0), text: "どうして".into(), level: Some(Level::Line), parent_id: Some("a-block".into()), ..Default::default() }];
    for (k, ch) in ["ど", "う", "し", "て"].iter().enumerate() {
        let y = 100.0 + k as f64 * 50.0;
        preds.push(Prediction { index: k + 1, quad: bx(162.0, y, 198.0, y + 45.0), text: ch.to_string(), level: Some(Level::Char), parent: Some(0), ..Default::default() });
    }
    preds.push(Prediction { index: 5, quad: bx(110.0, 100.0, 150.0, 200.0), text: "ここに".into(), level: Some(Level::Line), parent_id: Some("a-block".into()), ..Default::default() });
    let res = ai::apply_run(&mut pg, run(RunScope::Page, preds), &ProjectConfig::default());
    assert_eq!(res.added, 6);
    let lines: Vec<&Annotation> = pg.annotations.iter().filter(|a| a.level == Level::Line).collect();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l.parent.as_deref() == Some("a-block")));
    assert_eq!(lines.iter().find(|l| l.text() == "どうして").unwrap().reading_order, Some(1));
    assert_eq!(lines.iter().find(|l| l.text() == "ここに").unwrap().reading_order, Some(2));
    let first = lines.iter().find(|l| l.text() == "どうして").unwrap().id.clone();
    let chars = crate::assist::chars_of(&pg, &first);
    assert_eq!(chars.len(), 4);
    let mut ordered: Vec<(u32, String)> = chars.iter().map(|c| (c.reading_order.unwrap(), c.text().to_string())).collect();
    ordered.sort();
    assert_eq!(ordered.iter().map(|x| x.1.as_str()).collect::<String>(), "どうして");
    assert!(chars.iter().all(|c| c.transcription.as_ref().unwrap().writing_mode == WritingMode::VerticalRl), "characters follow their line's writing mode");
}

#[test]
fn characters_found_for_a_line_keep_the_persons_text() {
    let mut pg = page();
    let mut line = Annotation::new("a-line".into(), Level::Line, "p1", Geometry::Box { x: 100.0, y: 100.0, w: 40.0, h: 160.0, rotation_deg: 0.0 }, "tester", "now");
    line.transcription = Some(Transcription { text: "その夜".into(), writing_mode: WritingMode::VerticalRl, direction: Direction::Ttb, ..Default::default() });
    pg.annotations.push(line);
    // an even-split proposal nobody touched makes way for the model's boxes
    let cfg = ProjectConfig::default();
    crate::assist::regenerate_chars(&mut pg, "a-line", &cfg, "tester", "now");
    assert_eq!(crate::assist::chars_of(&pg, "a-line").len(), 3);
    let preds: Vec<Prediction> = ["そ", "の", "衣"]
        .iter()
        .enumerate()
        .map(|(k, t)| {
            let y = 100.0 + k as f64 * 52.0;
            Prediction { index: k, quad: bx(102.0, y, 138.0, y + 48.0), text: t.to_string(), level: Some(Level::Char), parent_id: Some("a-line".into()), ..Default::default() }
        })
        .collect();
    let scope = RunScope::Annotation { annotation_id: "a-line".into(), transform: "axis_aligned_crop".into(), quad: bx(100.0, 100.0, 140.0, 260.0), target_size: [40, 160] };
    let res = ai::apply_run(&mut pg, run(scope, preds), &cfg);
    assert_eq!(res.added, 3);
    let chars = crate::assist::chars_of(&pg, "a-line");
    assert_eq!(chars.len(), 3, "the heuristic boxes were replaced, not doubled");
    let mut text: Vec<(u32, String)> = chars.iter().map(|c| (c.reading_order.unwrap(), c.text().to_string())).collect();
    text.sort();
    assert_eq!(text.iter().map(|x| x.1.as_str()).collect::<String>(), "その夜", "the line's own characters are kept");
    assert!(chars.iter().all(|c| c.provenance.origin == Origin::Ai));
    let alt = &pg.get("a-line").unwrap().transcription.as_ref().unwrap().alternatives;
    assert!(alt.iter().any(|a| a.text == "その衣"), "the model's different reading is kept as an alternative");
}

#[test]
fn an_empty_line_gets_the_models_reading() {
    let mut pg = page();
    pg.annotations.push(Annotation::new("a-line".into(), Level::Line, "p1", Geometry::Box { x: 0.0, y: 0.0, w: 40.0, h: 100.0, rotation_deg: 0.0 }, "tester", "now"));
    let preds = vec![
        Prediction { index: 0, quad: bx(2.0, 2.0, 38.0, 48.0), text: "い".into(), level: Some(Level::Char), parent_id: Some("a-line".into()), ..Default::default() },
        Prediction { index: 1, quad: bx(2.0, 52.0, 38.0, 98.0), text: "ら".into(), level: Some(Level::Char), parent_id: Some("a-line".into()), ..Default::default() },
    ];
    let scope = RunScope::Annotation { annotation_id: "a-line".into(), transform: "axis_aligned_crop".into(), quad: bx(0.0, 0.0, 40.0, 100.0), target_size: [40, 100] };
    ai::apply_run(&mut pg, run(scope, preds), &ProjectConfig::default());
    let l = pg.get("a-line").unwrap();
    assert_eq!(l.text(), "いら");
    assert_eq!(l.provenance.fields.get("transcription"), Some(&FieldState::Proposed));
}

#[test]
fn reading_order_splits_into_columns_and_rows() {
    let c = |t: &str, x: f64, y: f64| SpotChar { text: t.into(), bbox: [x, y, x + 30.0, y + 30.0], score: 0.9 };
    // two vertical columns, right one first
    let v = vec![c("ど", 120.0, 10.0), c("う", 120.0, 50.0), c("し", 120.0, 90.0), c("こ", 60.0, 10.0), c("こ", 60.0, 50.0)];
    let (vertical, lines) = zoo::group_lines(&v);
    assert!(vertical);
    assert_eq!(lines, vec![vec![0, 1, 2], vec![3, 4]]);
    // two horizontal rows
    let h = vec![c("a", 10.0, 10.0), c("b", 45.0, 10.0), c("c", 80.0, 10.0), c("d", 10.0, 60.0), c("e", 45.0, 60.0)];
    let (vertical, lines) = zoo::group_lines(&h);
    assert!(!vertical);
    assert_eq!(lines, vec![vec![0, 1, 2], vec![3, 4]]);
}

#[test]
fn mask_outlines_follow_round_shapes_and_skip_boxes() {
    // a disc in a 40x40 mask, page 400x400
    let (mw, mh) = (40usize, 40usize);
    let disc: Vec<f32> = (0..mw * mh).map(|i| if (((i % mw) as f64 - 20.0).powi(2) + ((i / mw) as f64 - 20.0).powi(2)).sqrt() < 15.0 { 1.0 } else { -1.0 }).collect();
    let poly = zoo::mask_outline(&disc, mw, mh, 400, 400, &[40.0, 40.0, 360.0, 360.0]).expect("a disc is not its box");
    assert!(poly.len() >= 6 && poly.len() <= 24, "{} corners", poly.len());
    let full: Vec<f32> = vec![1.0; mw * mh];
    assert!(zoo::mask_outline(&full, mw, mh, 400, 400, &[0.0, 0.0, 400.0, 400.0]).is_none(), "a full box stays a box");
}

#[test]
fn nms_keeps_the_best_of_overlapping_boxes() {
    let b = [[0.0, 0.0, 10.0, 10.0], [1.0, 1.0, 11.0, 11.0], [50.0, 50.0, 60.0, 60.0]];
    assert_eq!(zoo::nms(&b, &[0.5, 0.9, 0.3], 0.5), vec![1, 2]);
}

#[test]
fn charspot_grid_fits_the_cell_budget() {
    for (h, w) in [(300, 40), (40, 300), (100, 100), (1200, 90)] {
        let (s, gh, gw) = zoo::pad_grid(h, w);
        assert!(gh * gw <= 256, "{h}x{w}: {gh}x{gw}");
        assert!((w as f64 * s).round() as u32 <= gw * 16 + 1 && (h as f64 * s).round() as u32 <= gh * 16 + 1);
    }
}

#[test]
fn schema_1_2_predictions_still_load() {
    let old = r#"{"index":0,"quad":[[0,0],[1,0],[1,1],[0,1]],"text":"a","status":"proposed"}"#;
    let p: Prediction = serde_json::from_str(old).unwrap();
    assert!(p.level.is_none() && p.polygon.is_none());
    // and new fields stay out of files when unused
    assert!(!serde_json::to_string(&p).unwrap().contains("level"));
}

#[test]
fn text_masks_give_lines_their_ink() {
    let mut pg = page();
    pg.annotations.push(Annotation::new("a-line".into(), Level::Line, "p1", Geometry::Box { x: 10.0, y: 10.0, w: 20.0, h: 40.0, rotation_deg: 0.0 }, "tester", "now"));
    pg.annotations.push(Annotation::new("a-panel".into(), Level::Panel, "p1", Geometry::Box { x: 0.0, y: 0.0, w: 100.0, h: 100.0, rotation_deg: 0.0 }, "tester", "now"));
    let mask = image::GrayImage::from_fn(100, 100, |x, y| image::Luma([if (15..25).contains(&x) && (20..40).contains(&y) { 255 } else { 0 }]));
    let files = ai::attach_ink(&mut pg, &mask, "run-1");
    assert_eq!(files.len(), 1, "only text levels get ink");
    let m = &pg.get("a-line").unwrap().masks[0];
    assert_eq!((m.role.as_str(), m.offset, m.size), ("ink", [15, 20], [10, 20]));
    assert!(ai::attach_ink(&mut pg, &mask, "run-2").is_empty(), "an ink mask is never replaced");
}

#[test]
fn zoo_jobs_ask_for_the_right_model() {
    assert_eq!(JobKind::Blocks.zoo_model(Reader::PpOcr), Some(zoo::ZooId::AnimeText));
    assert_eq!(JobKind::Layout.zoo_model(Reader::PpOcr), Some(zoo::ZooId::KoharuLayout));
    let rec = JobKind::Recognize { annotation_id: "a".into(), quad: bx(0.0, 0.0, 1.0, 1.0), target_size: [1, 1], rectified: false };
    assert_eq!(rec.zoo_model(Reader::PpOcr), None, "PP-OCR reads by default");
    assert_eq!(rec.zoo_model(Reader::Charspot), Some(zoo::ZooId::Charspot));
    assert!(!zoo::ZooId::Charspot.ncnn_possible() && zoo::ZooId::AnimeText.ncnn_possible());
}

/// The real models on the sample page, when they and ONNX Runtime are on this computer.
#[test]
fn real_models_read_the_sample_page() {
    let o = zoo::Options::default();
    if crate::runtime::locate(crate::runtime::Component::OnnxRuntime, None).is_none() || zoo::locate(zoo::ZooId::AnimeText, &o).is_err() || zoo::locate(zoo::ZooId::Charspot, &o).is_err() {
        eprintln!("models or ONNX Runtime not here; skipping");
        return;
    }
    let d = std::env::temp_dir().join(format!("labeldesk-zoo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let s = crate::sample::create_project(&d, "tester").unwrap();
    let e = s.entry(&s.pages[0].page_id).unwrap().clone();
    let img = crate::util::open_image(&s.image_path(&e)).unwrap().to_rgba8();
    let mut anime = zoo::AnimeText::load(&o).unwrap();
    let blocks = anime.detect(&img).unwrap();
    assert!(blocks.len() >= 3, "{blocks:?}");
    assert!(anime.model.artifacts.iter().all(|a| a.sha256.len() == 64));
    let mut cs = zoo::Charspot::load(&o).unwrap();
    let texts: Vec<String> = blocks
        .iter()
        .map(|b| {
            let crop = crate::geom::crop(&img, [b.bbox[0], b.bbox[1]], [b.bbox[2], b.bbox[3]]).unwrap();
            cs.read(&crop).unwrap().iter().map(|c| c.text.as_str()).collect()
        })
        .collect();
    assert!(texts.iter().any(|t| t == "いらっしゃいませ"), "{texts:?}");
    // the whole job path: blocks, then lines and characters
    let mut cache = ai::ZooCache::default();
    let job = |kind| ai::Job { page_id: e.id.clone(), image_path: s.image_path(&e), image_sha256: e.sha256.clone(), kind, pinned: None, user: "tester".into(), zoo: o.clone(), reader: Reader::PpOcr };
    let (r, _) = ai::run_zoo(&mut cache, &job(JobKind::Blocks), &|_, _| {}).unwrap();
    let mut pg = s.pages[0].clone();
    pg.annotations.clear();
    ai::apply_run(&mut pg, r, &s.manifest.config);
    let blocks: Vec<(String, [P; 4])> = pg.annotations.iter().map(|a| (a.id.clone(), a.geometry.corners().unwrap())).collect();
    let (r, _) = ai::run_zoo(&mut cache, &job(JobKind::ReadBlocks { blocks }), &|_, _| {}).unwrap();
    ai::apply_run(&mut pg, r, &s.manifest.config);
    assert!(pg.annotations.iter().any(|a| a.level == Level::Line && a.text() == "いらっしゃいませ"));
    assert!(pg.annotations.iter().filter(|a| a.level == Level::Char).count() >= 8);
    let _ = std::fs::remove_dir_all(&d);
}
