//! Acceptance checks for assisted annotation (spec v1.1, sections 18–25), on the data layer.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::assist::{self, CharStatus, NeedKind, QueueKind, Suggest};
use crate::schema::*;
use crate::store::Store;
use crate::{geom, review, util};

fn sample(name: &str) -> (Store, PathBuf) {
    let d = std::env::temp_dir().join(format!("labeldesk-assist-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let s = crate::sample::create_project(&d, "tester").unwrap();
    (s, d)
}

fn find<'a>(p: &'a PageFile, text: &str) -> &'a Annotation {
    p.annotations.iter().find(|a| a.text() == text && a.level == Level::Line).unwrap_or_else(|| panic!("no line {text}"))
}

fn centre(q: &[[f64; 2]; 4]) -> [f64; 2] {
    [(q[0][0] + q[1][0] + q[2][0] + q[3][0]) / 4.0, (q[0][1] + q[1][1] + q[2][1] + q[3][1]) / 4.0]
}

#[test]
fn eight_ordered_boxes_for_an_eight_character_japanese_line() {
    let (mut s, _d) = sample("eight");
    let cfg = s.manifest.config.clone();
    let page = &mut s.pages[0];
    let line = find(page, "いらっしゃいませ").clone();
    assert!(assist::chars_of(page, &line.id).is_empty());
    assert_eq!(assist::char_status(page, &line, &cfg), CharStatus::None(8));
    let (added, _, _) = assist::regenerate_chars(page, &line.id, &cfg, "tester", "t");
    assert_eq!(added, 8);
    let mut kids: Vec<&Annotation> = assist::chars_of(page, &line.id);
    kids.sort_by_key(|c| c.reading_order);
    let text: String = kids.iter().map(|c| c.text()).collect();
    assert_eq!(text, "いらっしゃいませ");
    let ys: Vec<f64> = kids.iter().map(|c| c.geometry.bounds().0[1]).collect();
    assert!(ys.windows(2).all(|w| w[1] > w[0]), "vertical text goes down: {ys:?}");
    for c in &kids {
        let q = c.geometry.corners().unwrap();
        assert!(line.geometry.contains(centre(&q), 1.0));
        assert_eq!(c.provenance.origin, Origin::Heuristic);
        assert_eq!(c.provenance.fields["geometry"], FieldState::HeuristicProposal);
        assert_eq!(c.review.state, ReviewState::AiProposed);
        let h = c.provenance.heuristic.as_ref().unwrap();
        assert_eq!(h.algorithm, assist::CHAR_ALGO);
        assert_eq!(h.source_text, "いらっしゃいませ");
        assert_eq!(h.source_transcription_revision, line.revision);
        assert_eq!(h.span[1] - h.span[0], 1);
    }
    assert_eq!(assist::char_status(page, &line, &cfg), CharStatus::Proposed(8));
}

#[test]
fn graphemes_not_code_points_and_proportional_latin() {
    let (s, _d) = sample("graph");
    let mut page = s.pages[0].clone();
    let mut a = Annotation::new("a-x".into(), Level::Line, &page.page_id, Geometry::Box { x: 0.0, y: 0.0, w: 400.0, h: 40.0, rotation_deg: 0.0 }, "t", "t");
    a.transcription = Some(Transcription { text: "e\u{301}👩‍👩‍👧 Wil".into(), ..Default::default() });
    page.annotations.push(a.clone());
    let (props, frame) = assist::propose_chars(&a, &page, &CharPolicy::default());
    assert_eq!(frame, "quad");
    let texts: Vec<&str> = props.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, vec!["e\u{301}", "👩‍👩‍👧", "W", "i", "l"], "space skipped, clusters kept whole");
    assert_eq!(props[1].span, [1, 2]);
    let width = |p: &assist::CharProposal| p.quad[1][0] - p.quad[0][0];
    assert!(width(&props[2]) > width(&props[3]) * 2.0, "W is wider than i");
    // right-to-left text runs the other way
    let mut b = a.clone();
    b.transcription.as_mut().unwrap().direction = Direction::Rtl;
    let (rtl, _) = assist::propose_chars(&b, &page, &CharPolicy::default());
    assert!(rtl[0].quad[0][0] > rtl[4].quad[0][0]);
}

#[test]
fn perspective_aware_boxes_follow_the_sign() {
    let (s, _d) = sample("persp");
    let page = &s.pages[0];
    let line = find(page, "喫茶店");
    let (props, frame) = assist::propose_chars(line, page, &CharPolicy::default());
    assert_eq!(frame, "rectified");
    assert_eq!(props.len(), 3);
    // compare with the generator's true character quads (drawn through the same perspective)
    let truth: Vec<&Annotation> = assist::chars_of(page, &line.id);
    for (p, t) in props.iter().zip(truth.iter()) {
        let (pc, tc) = (centre(&p.quad), centre(&t.geometry.corners().unwrap()));
        assert!(geom::dist(pc, tc) < 12.0, "{} proposed at {pc:?}, truth {tc:?}", p.text);
        assert!(geom::quad_order_ok(&p.quad));
    }
    // mapping back and forth through the homography is consistent
    let r = line.rectification.as_ref().unwrap();
    let h = geom::homography(&r.source, &[[0.0, 0.0], [r.target_size[0] as f64, 0.0], [r.target_size[0] as f64, r.target_size[1] as f64], [0.0, r.target_size[1] as f64]]).unwrap();
    let inv = geom::homography(&[[0.0, 0.0], [r.target_size[0] as f64, 0.0], [r.target_size[0] as f64, r.target_size[1] as f64], [0.0, r.target_size[1] as f64]], &r.source).unwrap();
    let c = centre(&props[1].quad);
    assert!(geom::dist(geom::apply_h(&inv, geom::apply_h(&h, c)), c) < 1e-6);
}

#[test]
fn text_change_makes_boxes_stale_and_regeneration_keeps_manual_fixes() {
    let (mut s, _d) = sample("stale");
    let cfg = s.manifest.config.clone();
    let policy = cfg.review.clone();
    let page = &mut s.pages[0];
    let line = find(page, "いらっしゃいませ").clone();
    assist::regenerate_chars(page, &line.id, &cfg, "tester", "t");
    // a person nudges the first box
    let mut kids: Vec<Annotation> = assist::chars_of(page, &line.id).into_iter().cloned().collect();
    kids.sort_by_key(|c| c.reading_order);
    let first = kids[0].id.clone();
    {
        let c = page.get_mut(&first).unwrap();
        let old = c.clone();
        c.geometry.translate([1.0, 0.5]);
        review::on_edit(&old, c, &policy, "alice", "t2");
        assert_eq!(c.provenance.fields["geometry"], FieldState::Corrected);
        assert!(!assist::untouched_proposal(c));
    }
    // the transcription changes: boxes are flagged, not deleted
    {
        let l = page.get_mut(&line.id).unwrap();
        let old = l.clone();
        l.transcription.as_mut().unwrap().text = "いらっしゃい".into();
        review::on_edit(&old, l, &policy, "alice", "t3");
    }
    let l2 = page.get(&line.id).unwrap().clone();
    assert!(matches!(assist::char_status(page, &l2, &cfg), CharStatus::Stale(8)));
    assert_eq!(assist::chars_of(page, &line.id).len(), 8, "nothing silently deleted");
    let issues = crate::validate::validate_page(&cfg, page, None);
    assert!(issues.iter().any(|i| i.rule == "char_stale"));
    // regenerate: untouched proposals are replaced, the manual fix survives
    let (added, kept, removed) = assist::regenerate_chars(page, &line.id, &cfg, "alice", "t4");
    assert_eq!((kept, removed), (1, 7));
    assert_eq!(added, 5, "6 characters now, the first one is kept");
    assert!(page.get(&first).is_some());
    let kids = assist::chars_of(page, &line.id);
    assert_eq!(kids.len(), 6);
}

#[test]
fn reading_order_follows_conventions_and_manual_fixes_survive() {
    let (s, _d) = sample("order");
    let cfg = s.manifest.config.clone();
    let mut page = s.pages[1].clone(); // empty webtoon page used as canvas
    page.annotations.clear();
    // three vertical Japanese columns inside one balloon, created without any order
    let mut bubble = Annotation::new("a-b".into(), Level::Container, &page.page_id, Geometry::Box { x: 100.0, y: 100.0, w: 300.0, h: 300.0, rotation_deg: 0.0 }, "t", "t");
    bubble.classes.insert("dialogue".into());
    page.annotations.push(bubble);
    for (i, x) in [150.0, 330.0, 240.0].iter().enumerate() {
        let mut l = Annotation::new(format!("a-l{i}"), Level::Line, &page.page_id, Geometry::Box { x: *x, y: 120.0, w: 40.0, h: 220.0, rotation_deg: 0.0 }, "t", "t");
        l.parent = Some("a-b".into());
        l.transcription = Some(Transcription { text: "テスト".into(), writing_mode: WritingMode::VerticalRl, direction: Direction::Ttb, ..Default::default() });
        page.annotations.push(l);
    }
    assert!(assist::apply_order(&mut page, &cfg, Direction::Rtl) >= 3);
    let o = |p: &PageFile, id: &str| p.get(id).unwrap().reading_order.unwrap();
    assert_eq!((o(&page, "a-l1"), o(&page, "a-l2"), o(&page, "a-l0")), (1, 2, 3), "rightmost column first");
    assert!(!assist::manual_order(page.get("a-l1").unwrap()));
    // a person decides the left column comes first
    {
        let policy = cfg.review.clone();
        let l = page.get_mut("a-l0").unwrap();
        let old = l.clone();
        l.reading_order = Some(1);
        review::on_edit(&old, l, &policy, "alice", "t");
    }
    assert!(assist::manual_order(page.get("a-l0").unwrap()));
    assist::apply_order(&mut page, &cfg, Direction::Rtl);
    assert_eq!(o(&page, "a-l0"), 1, "manual order survives recalculation");
    assert_eq!((o(&page, "a-l1"), o(&page, "a-l2")), (2, 3));
    // the same columns set horizontal on a left-to-right page read top to bottom, left to right
    let mut h = page.clone();
    for a in h.annotations.iter_mut() {
        a.reading_order = None;
        a.reading = None;
        if let Some(t) = &mut a.transcription {
            t.writing_mode = WritingMode::HorizontalTb;
            t.direction = Direction::Ltr;
        }
    }
    assist::apply_order(&mut h, &cfg, Direction::Ltr);
    assert_eq!((o(&h, "a-l0"), o(&h, "a-l2"), o(&h, "a-l1")), (1, 2, 3));
    // keyboard reordering swaps neighbours and marks them manual
    assert!(assist::nudge_order(&mut h, "a-l1", -1));
    assert_eq!((o(&h, "a-l1"), o(&h, "a-l2")), (2, 3));
}

#[test]
fn quick_review_points_at_missing_fields_and_approves_in_one_action() {
    let (mut s, _d) = sample("quick");
    let cfg = s.manifest.config.clone();
    let page = &mut s.pages[0];
    let id = find(page, "いらっしゃいませ").id.clone();
    let needs = assist::needs(page.get(&id).unwrap(), page, &cfg);
    let fields: Vec<&str> = needs.iter().map(|n| n.field).collect();
    assert_eq!(fields, vec!["recognizability", "classes"], "{needs:#?}");
    assert!(matches!(needs[1].suggestion, Some((Suggest::Class(ref c), _)) if c == "dialogue"), "text type suggested from the balloon");
    // nothing is invented: approval stops at the first unresolved field
    let err = assist::quick_approve(page, &id, &cfg, "reviewer.b", "t").unwrap_err();
    assert!(err.starts_with("Legibility"), "{err}");
    assert_eq!(page.get(&id).unwrap().recognizability, None);
    for n in &needs {
        let (sugg, _) = n.suggestion.clone().unwrap();
        assist::apply_suggestion(page, &id, &sugg, &cfg, "reviewer.b", "t");
    }
    assist::quick_approve(page, &id, &cfg, "reviewer.b", "t").unwrap();
    let a = page.get(&id).unwrap();
    assert_eq!(a.review.state, ReviewState::HumanReviewed);
    assert_eq!(a.checks["classes"].state, CheckState::Confirmed);
    assert_eq!(a.checks["recognizability"].by, "reviewer.b");
}

#[test]
fn proposed_characters_are_never_promoted_silently() {
    let (mut s, _d) = sample("promote");
    let mut cfg = s.manifest.config.clone();
    cfg.level_overrides.push(LevelOverride { class: "dialogue".into(), level: Level::Char, requirement: Requirement::Required });
    let page = &mut s.pages[0];
    let id = find(page, "どうして").id.clone();
    let n = assist::needs(page.get(&id).unwrap(), page, &cfg);
    assert!(n.iter().any(|x| x.field == "char_boxes" && x.kind == NeedKind::Missing));
    assist::regenerate_chars(page, &id, &cfg, "reviewer.b", "t");
    let err = assist::quick_approve(page, &id, &cfg, "reviewer.b", "t").unwrap_err();
    assert!(err.starts_with("Character boxes"), "{err}");
    assert!(assist::chars_of(page, &id).iter().all(|c| c.review.state == ReviewState::AiProposed && c.review.approvals.is_empty()));
    // accepting makes them drafts owned by a person, still not verified
    assist::apply_suggestion(page, &id, &Suggest::AcceptChars(4), &cfg, "reviewer.b", "t");
    assert!(assist::chars_of(page, &id).iter().all(|c| c.review.state == ReviewState::Draft && c.provenance.fields["geometry"] == FieldState::Accepted));
    assist::quick_approve(page, &id, &cfg, "reviewer.b", "t").unwrap();
    assert!(assist::chars_of(page, &id).iter().all(|c| c.review.approvals.is_empty()), "approving the line does not approve its characters");
}

#[test]
fn queues_metrics_and_sessions() {
    let (mut s, _d) = sample("queue");
    let cfg = s.manifest.config.clone();
    let id = find(&s.pages[0], "いらっしゃいませ").id.clone();
    assist::regenerate_chars(&mut s.pages[0], &id, &cfg, "t", "t");
    let q: usize = s.pages.iter().map(|p| p.annotations.iter().filter(|a| assist::matches_kind(QueueKind::HeuristicChars, a, p, &cfg)).count()).sum();
    assert_eq!(q, 8);
    // the review workflow visits lines in reading order, pass by pass
    let wf = assist::Workflow::default();
    let steps = assist::page_steps(&s.pages[0], &cfg, &wf.order(), None, false);
    let first: Vec<&str> = steps.iter().filter(|i| i.pass == assist::Pass::LineBoxes).filter_map(|i| s.pages[0].get(i.annotation_id.as_ref().unwrap())).map(|a| a.text()).collect();
    assert_eq!(first.first(), Some(&"その夜"), "reading order: the caption in panel 1 comes first: {first:?}");
    let passes: Vec<assist::Pass> = steps.iter().map(|i| i.pass).collect();
    assert!(passes.windows(2).all(|w| w[0] <= w[1]), "passes run one after the other");
    assert_eq!(passes.last(), Some(&assist::Pass::ReadingOrder), "reading order is last");
    assert!(steps.iter().any(|i| i.pass == assist::Pass::MissingBoxes && i.annotation_id.is_none()), "page steps have no label");
    // missing boxes at the end move before the reading order
    let late = assist::Workflow { missing_at_end: true, ..Default::default() };
    let o = late.order();
    assert_eq!(o[o.len() - 2], assist::Pass::MissingBoxes);
    // a search filter keeps only those labels and leaves out page steps
    let only: BTreeSet<String> = [id.clone()].into_iter().collect();
    let f = assist::page_steps(&s.pages[0], &cfg, &wf.order(), Some(&only), false);
    assert!(!f.is_empty() && f.iter().all(|i| i.annotation_id.as_deref() == Some(id.as_str())));
    let missing = s.pages.iter().any(|p| p.get(&id).is_some_and(|a| assist::matches_kind(QueueKind::MissingFields, a, p, &cfg)));
    assert!(missing);
    let sess = assist::ReviewSession {
        workflow: wf,
        pages: s.pages.iter().map(|p| p.page_id.clone()).collect(),
        filter: None,
        page: 0,
        items: steps.clone(),
        pos: 3,
        done: [0, 1, 2].into_iter().collect(),
        known: Default::default(),
        completed_pages: Default::default(),
        reviewed: 3,
        started: util::now(),
        user: "t".into(),
    };
    let back: assist::ReviewSession = serde_json::from_str(&serde_json::to_string(&sess).unwrap()).unwrap();
    assert_eq!(back, sess);
    let refs: Vec<&PageFile> = s.pages.iter().collect();
    let m = assist::metrics(&refs, &cfg);
    assert_eq!(m.provisional_chars, 8);
    assert!(m.complete_required > m.fully_approved, "complete is not the same as verified: {m:?}");
    assert!(m.verified_transcription >= 4);
    assert_eq!(m.pages_awaiting_inspection, 4);
}

#[test]
fn two_digit_numbers_share_one_cell_in_vertical_text() {
    let (mut s, _d) = sample("tcy");
    let cfg = s.manifest.config.clone();
    let id = find(&s.pages[0], "いらっしゃいませ").id.clone();
    let line = s.pages[0].get_mut(&id).unwrap();
    line.transcription.as_mut().unwrap().text = "あと12分".into();
    let a = line.clone();
    let (cells, _) = assist::propose_chars(&a, &s.pages[0], &cfg.quick.chars);
    assert_eq!(cells.len(), 5);
    let (one, two) = (&cells[2], &cells[3]);
    assert_eq!((one.text.as_str(), two.text.as_str()), ("1", "2"));
    assert!((one.cell[1] - two.cell[1]).abs() < 1e-9 && (one.cell[3] - two.cell[3]).abs() < 1e-9, "same place in the column");
    assert!(one.cell[2] <= two.cell[0] + 1e-9, "side by side, 1 on the left: {:?} {:?}", one.cell, two.cell);
    let span = |c: &assist::CharProposal| c.cell[3] - c.cell[1];
    assert!((span(one) - span(&cells[0])).abs() < 1e-6, "the pair takes one character's height");
    // the shared cell has no draggable border inside it
    let borders = assist::preview_borders(&a, &s.pages[0], &cfg.quick.chars, None);
    assert_eq!(borders.len(), 3);
}

#[test]
fn monospaced_lines_get_equal_cells() {
    let (mut s, _d) = sample("mono");
    let cfg = s.manifest.config.clone();
    let id = find(&s.pages[0], "いらっしゃいませ").id.clone();
    let line = s.pages[0].get_mut(&id).unwrap();
    {
        let t = line.transcription.as_mut().unwrap();
        t.text = "Hi, Wim!".into();
        t.writing_mode = WritingMode::HorizontalTb;
        t.direction = Direction::Ltr;
    }
    let width = |a: &Annotation, p: &PageFile| assist::propose_chars(a, p, &cfg.quick.chars).0.iter().map(|c| ((c.cell[2] - c.cell[0]) * 1e4).round() as i64).collect::<BTreeSet<_>>().len();
    let a = line.clone();
    assert!(width(&a, &s.pages[0]) > 1, "proportional by default");
    s.pages[0].get_mut(&id).unwrap().attributes.insert("monospace".into());
    let a = s.pages[0].get(&id).unwrap().clone();
    assert_eq!(width(&a, &s.pages[0]), 1, "monospace: one width");
}

#[test]
fn grouping_the_selection_makes_one_parent() {
    let (mut s, _d) = sample("group");
    let cfg = s.manifest.config.clone();
    let lines: Vec<String> = s.pages[0].annotations.iter().filter(|a| a.level == Level::Line).take(2).map(|a| a.id.clone()).collect();
    let (gid, level, n) = assist::group_into(&mut s.pages[0], &lines, &cfg, "t", "t").unwrap();
    assert_eq!((level, n), (Level::Region, 2));
    for l in &lines {
        assert_eq!(s.pages[0].get(l).unwrap().parent.as_deref(), Some(gid.as_str()));
        let (lo, hi) = s.pages[0].get(l).unwrap().geometry.bounds();
        assert!(s.pages[0].get(&gid).unwrap().geometry.contains(lo, 0.5) && s.pages[0].get(&gid).unwrap().geometry.contains(hi, 0.5));
    }
    let (_, l2, _) = assist::group_into(&mut s.pages[0], &[gid], &cfg, "t", "t").unwrap();
    assert_eq!(l2, Level::Container, "regions group into a container");
}

#[test]
fn old_text_types_move_to_the_new_ones_and_order_skips_background() {
    let (mut s, _d) = sample("migrate");
    let mut cfg = s.manifest.config.clone();
    cfg.text_classes.push(ClassDef { id: "handwritten".into(), name: "Handwritten".into(), color: [1, 2, 3] });
    let id = find(&s.pages[0], "いらっしゃいませ").id.clone();
    s.pages[0].get_mut(&id).unwrap().classes = ["handwritten".to_string()].into_iter().collect();
    assert_eq!(assist::retired_labels(&s.pages[0]), 1);
    assert!(assist::migrate_config(&mut cfg));
    assert!(cfg.class("handwritten").is_none() && cfg.attributes.iter().any(|a| a == "handwritten") && cfg.class("secondary").is_some());
    assert_eq!(assist::migrate_classes(&mut s.pages[0]), 1);
    let a = s.pages[0].get(&id).unwrap();
    assert!(a.classes.is_empty() && a.attributes.contains("handwritten"));
    // background text, sound effects and page numbers are read outside the order
    let mut bg = a.clone();
    bg.classes = ["sfx".to_string()].into_iter().collect();
    assert!(assist::is_unordered(&bg, &cfg));
    assert!(!cfg.reading.auto_suggest, "order is computed at the end by default");
}

#[test]
fn keyboard_nudges_grow_shrink_and_move() {
    let g = Geometry::Box { x: 10.0, y: 10.0, w: 20.0, h: 40.0, rotation_deg: 0.0 };
    assert_eq!(assist::nudge_shape(&g, assist::Side::Right, 2.0, false), Geometry::Box { x: 10.0, y: 10.0, w: 22.0, h: 40.0, rotation_deg: 0.0 });
    assert_eq!(assist::nudge_shape(&g, assist::Side::Left, -1.0, false), Geometry::Box { x: 11.0, y: 10.0, w: 19.0, h: 40.0, rotation_deg: 0.0 });
    assert_eq!(assist::nudge_shape(&g, assist::Side::Top, 3.0, true), Geometry::Box { x: 10.0, y: 7.0, w: 20.0, h: 40.0, rotation_deg: 0.0 });
    let q = Geometry::Quad { points: [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]] };
    assert_eq!(assist::nudge_shape(&q, assist::Side::Bottom, 1.0, false), Geometry::Quad { points: [[0.0, 0.0], [10.0, 0.0], [10.0, 11.0], [0.0, 11.0]] });
}

#[test]
fn coarse_answers_need_refining() {
    let (mut s, _d) = sample("coarse");
    let cfg = s.manifest.config.clone();
    let id = find(&s.pages[0], "いらっしゃいませ").id.clone();
    s.pages[0].get_mut(&id).unwrap().recognizability = Some(Recognizability::NotPerfect);
    s.pages[0].get_mut(&id).unwrap().confidence.transcription = Some(Confidence::Uncertain);
    let a = s.pages[0].get(&id).unwrap().clone();
    let n = assist::needs(&a, &s.pages[0], &cfg);
    assert!(n.iter().any(|x| x.field == "recognizability" && x.message.contains("not perfectly")));
    assert!(n.iter().any(|x| x.field == "confidence"));
}
