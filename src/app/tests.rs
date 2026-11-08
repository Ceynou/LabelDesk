//! Interface tests: drive the real workspace with simulated mouse and keyboard input.

use super::*;
use crate::assist::Pass;
use egui::{Event, PointerButton, Pos2, RawInput};

struct H {
    ctx: egui::Context,
    app: App,
    t: f64,
    _dir: PathBuf,
    cursor: egui::CursorIcon,
}

impl H {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("labeldesk-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = App::base(&ctx, None);
        app.set_store(crate::sample::create_project(&dir, "you").unwrap());
        let mut h = H { ctx, app, t: 0.0, _dir: dir, cursor: egui::CursorIcon::Default };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        h
    }

    fn frame(&mut self, events: Vec<Event>) {
        self.t += 0.05;
        let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1500.0, 940.0))), time: Some(self.t), events, ..Default::default() };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.frame_ui(ui));
        out.textures_delta.clear();
        self.cursor = out.platform_output.cursor_icon;
    }

    fn s(&self, p: P) -> Pos2 {
        self.app.canvas_rect.min + self.app.view.offset + vec2(p[0] as f32, p[1] as f32) * self.app.view.zoom
    }

    fn btn(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    fn click(&mut self, p: P) {
        let pos = self.s(p);
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![Self::btn(pos, true)]);
        self.frame(vec![Self::btn(pos, false)]);
        self.t += 0.6;
        self.frame(vec![]);
    }

    fn drag(&mut self, a: P, b: P) {
        let (pa, pb) = (self.s(a), self.s(b));
        self.frame(vec![Event::PointerMoved(pa)]);
        self.frame(vec![Self::btn(pa, true)]);
        for k in 1..=6 {
            self.frame(vec![Event::PointerMoved(pa + (pb - pa) * (k as f32 / 6.0))]);
        }
        self.frame(vec![Self::btn(pb, false)]);
        self.t += 0.6;
        self.frame(vec![]);
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
        self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]);
    }

    fn page(&self) -> &PageFile {
        self.app.cur().unwrap()
    }
}

fn near(a: P, b: P) -> bool {
    (a[0] - b[0]).abs() < 3.0 && (a[1] - b[1]).abs() < 3.0
}

#[test]
fn draw_box_rotate_and_type_text() {
    let mut h = H::new("box");
    h.app.go_to(3); // blank splash page
    h.frame(vec![]);
    h.frame(vec![]);
    h.key(Key::B, Modifiers::NONE);
    assert_eq!(h.app.tool, Tool::Box);
    h.key(Key::Num4, Modifiers::NONE);
    assert_eq!(h.app.draw_level, Level::Line);
    h.drag([100.0, 100.0], [300.0, 160.0]);
    assert_eq!(h.page().annotations.len(), 1);
    let a = h.page().annotations[0].clone();
    let Geometry::Box { x, y, w, h: bh, .. } = a.geometry else { panic!("not a box") };
    assert!(near([x, y], [100.0, 100.0]) && near([x + w, y + bh], [300.0, 160.0]), "{x} {y} {w} {bh}");
    assert!(a.transcription.is_some(), "line level gets an empty transcription to fill in");
    // rotate with the round handle
    h.key(Key::V, Modifiers::NONE);
    let rot = h.app.handles(&a, h.app.view.zoom).into_iter().find(|(k, _)| *k == Handle::Rotate).unwrap().1;
    h.drag(rot, [320.0, 130.0]);
    let Geometry::Box { rotation_deg, .. } = h.page().annotations[0].geometry else { panic!() };
    assert!((rotation_deg - 90.0).abs() < 5.0, "rotation {rotation_deg}");
    // typing text is an edit of the same annotation, with the same id
    let id = a.id.clone();
    h.app.cur_mut().unwrap().get_mut(&id).unwrap().transcription.as_mut().unwrap().text = "ドン".into();
    h.frame(vec![]);
    let a2 = h.page().get(&id).unwrap().clone();
    assert_eq!(a2.text(), "ドン");
    assert!(a2.revision >= 2);
    assert!(matches!(a2.geometry, Geometry::Box { rotation_deg, .. } if (rotation_deg - 90.0).abs() < 5.0), "text edit kept the geometry");
    // undo the text, then the rotation
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.page().get(&id).unwrap().text(), "");
}

#[test]
fn quad_tool_and_reading_order_tool() {
    let mut h = H::new("quad");
    h.app.go_to(3);
    h.frame(vec![]);
    h.key(Key::Q, Modifiers::NONE);
    for p in [[100.0, 100.0], [300.0, 130.0], [280.0, 220.0], [90.0, 180.0]] {
        h.click(p);
    }
    let quads: Vec<&Annotation> = h.page().annotations.iter().filter(|a| matches!(a.geometry, Geometry::Quad { .. })).collect();
    assert_eq!(quads.len(), 1);
    assert!(crate::geom::quad_order_ok(&quads[0].geometry.corners().unwrap()));
    h.key(Key::B, Modifiers::NONE);
    h.drag([400.0, 100.0], [500.0, 160.0]);
    let ids: Vec<String> = h.page().annotations.iter().map(|a| a.id.clone()).collect();
    // reading order: click the box first, then the quad
    h.key(Key::O, Modifiers::NONE);
    h.click([450.0, 130.0]);
    h.click([200.0, 150.0]);
    let order = |id: &str| h.page().get(id).unwrap().reading_order;
    assert_eq!(order(&ids[1]), Some(1));
    assert_eq!(order(&ids[0]), Some(2));
}

#[test]
fn editing_approved_text_by_moving_it_needs_rereview() {
    let mut h = H::new("rereview");
    let a = h.page().annotations.iter().find(|a| a.text() == "どうして").unwrap().clone();
    assert_eq!(a.review.state, ReviewState::HumanReviewed);
    h.key(Key::V, Modifiers::NONE);
    let (lo, hi) = a.geometry.bounds();
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    h.click(c);
    assert_eq!(h.app.selection, vec![a.id.clone()]);
    h.drag(c, [c[0] + 24.0, c[1] + 16.0]);
    let b = h.page().get(&a.id).unwrap().clone();
    assert_eq!(b.text(), "どうして", "moving keeps the text");
    assert_ne!(b.geometry, a.geometry, "moved");
    assert!(b.review.needs_rereview, "{:#?}", b.review);
    assert!(b.review.approvals.iter().any(|x| x.dimension == Dimension::Geometry && x.invalidated.is_some()));
    assert!(b.review.approvals.iter().any(|x| x.dimension == Dimension::Transcription && x.invalidated.is_none()));
    assert_eq!(b.provenance.fields["geometry"], FieldState::ChangedAfterVerification);
    // another reviewer approves again from the keyboard
    h.app.settings.user = "second.reviewer".into();
    h.key(Key::Enter, Modifiers::COMMAND | Modifiers::SHIFT);
    let c2 = h.page().get(&a.id).unwrap().clone();
    assert!(!c2.review.needs_rereview, "{:?}", h.app.message);
    assert_eq!(c2.review.state, ReviewState::HumanReviewed);
    // save with Ctrl+S writes the page file
    h.key(Key::S, Modifiers::COMMAND);
    assert!(!h.app.is_dirty());
    let on_disk = std::fs::read_to_string(h.app.store.as_ref().unwrap().page_path(&h.page().page_id)).unwrap();
    assert!(on_disk.contains("second.reviewer"));
}

#[test]
fn flatten_tab_preview_follows_corner_drags() {
    let mut h = H::new("flat");
    let sign = h.page().annotations.iter().find(|a| a.level == Level::Container && a.classes.contains("signage")).unwrap().clone();
    h.app.selection = vec![sign.id.clone()];
    h.app.tab = Tab::Flatten;
    h.frame(vec![]);
    h.frame(vec![]);
    let key0 = h.app.preview.as_ref().map(|p| p.key.clone()).expect("preview made");
    let src = sign.rectification.as_ref().unwrap().source;
    h.key(Key::V, Modifiers::NONE);
    h.drag(src[1], [src[1][0] + 8.0, src[1][1] - 6.0]);
    h.frame(vec![]);
    let s2 = h.page().get(&sign.id).unwrap().rectification.clone().unwrap();
    assert!(near(s2.source[1], [src[1][0] + 8.0, src[1][1] - 6.0]), "{:?}", s2.source[1]);
    assert_eq!(h.page().get(&sign.id).unwrap().geometry, sign.geometry, "the shape itself is untouched");
    assert_ne!(h.app.preview.as_ref().unwrap().key, key0, "preview recomputed");
}

fn review_on(h: &mut H, text: &str) -> String {
    review_pass(h, text, Pass::Rest)
}

fn review_pass(h: &mut H, text: &str, pass: Pass) -> String {
    h.key(Key::F6, Modifiers::NONE);
    assert!(h.app.review.is_some(), "F6 enters review mode");
    assert!(h.app.review.as_ref().unwrap().setup, "a new review starts with the workflow choices");
    h.key(Key::Enter, Modifiers::NONE);
    assert!(!h.app.review.as_ref().unwrap().setup, "Enter starts it");
    let pos = h.app.review.as_ref().unwrap().session.items.iter().position(|i| i.pass == pass && i.annotation_id.as_ref().and_then(|id| h.page().get(id)).is_some_and(|a| a.text() == text)).unwrap();
    h.app.review_goto(pos);
    h.frame(vec![]);
    h.frame(vec![]);
    h.app.selection[0].clone()
}

fn screen_rect(h: &H, id: &str) -> Rect {
    let (lo, hi) = h.page().get(id).unwrap().geometry.bounds();
    Rect::from_two_pos(h.s(lo), h.s(hi))
}

#[test]
fn review_frames_toggles_views_and_moves_with_the_keyboard() {
    let mut h = H::new("review-views");
    let fit_zoom = h.app.view.zoom;
    let id = review_pass(&mut h, "いらっしゃいませ", Pass::Transcription);
    let canvas = h.app.canvas_rect;
    let r = screen_rect(&h, &id);
    assert!(h.app.view.zoom > fit_zoom * 1.5, "focused view zooms in: {} vs {fit_zoom}", h.app.view.zoom);
    assert!(canvas.contains_rect(r), "the whole annotation is visible: {r:?} in {canvas:?}");
    assert!(r.height() > canvas.height() * 0.45, "and fills most of the view");
    // Space: clean context view and back
    let focused = h.app.view.zoom;
    h.key(Key::Space, Modifiers::NONE);
    assert!(h.app.view.zoom < focused * 0.8, "context view zooms out");
    h.key(Key::Space, Modifiers::NONE);
    assert!((h.app.view.zoom - focused).abs() < 1e-3);
    h.key(Key::P, Modifiers::NONE);
    assert!(h.app.view.zoom < focused * 0.6, "page view");
    h.key(Key::P, Modifiers::NONE);
    // T overlay, B border
    let (o, b) = { let r = h.app.review.as_ref().unwrap(); (r.overlay, r.active_border) };
    h.key(Key::T, Modifiers::NONE);
    h.key(Key::B, Modifiers::NONE);
    let r2 = h.app.review.as_ref().unwrap();
    assert_eq!((r2.overlay, r2.active_border), (!o, !b));
    // Down / Up move through the queue
    let pos = h.app.review.as_ref().unwrap().session.pos;
    h.key(Key::ArrowDown, Modifiers::NONE);
    assert_eq!(h.app.review.as_ref().unwrap().session.pos, pos + 1);
    assert_ne!(h.app.selection[0], id);
    h.key(Key::ArrowUp, Modifiers::NONE);
    assert_eq!(h.app.selection[0], id);
}

#[test]
fn enter_points_at_missing_fields_then_approves_and_advances() {
    let mut h = H::new("review-approve");
    let id = review_on(&mut h, "いらっしゃいませ");
    let pos = h.app.review.as_ref().unwrap().session.pos;
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.app.review.as_ref().unwrap().session.pos, pos, "incomplete: stays");
    let msg = h.app.message.clone().map(|m| m.0).unwrap_or_else(|| panic!("no message; wants kb {}", h.ctx.egui_wants_keyboard_input()));
    assert!(msg.contains("Legibility"), "{msg}");
    assert_eq!(h.page().get(&id).unwrap().recognizability, None, "nothing invented");
    h.key(Key::Num1, Modifiers::NONE); // Readable
    assert_eq!(h.page().get(&id).unwrap().recognizability, Some(Recognizability::Readable));
    h.key(Key::Y, Modifiers::NONE); // use the suggested text type (dialogue, from the balloon)
    assert!(h.page().get(&id).unwrap().classes.contains("dialogue"));
    h.key(Key::Enter, Modifiers::NONE);
    let a = h.app.store.as_ref().unwrap().pages.iter().find_map(|p| p.get(&id)).unwrap().clone();
    assert_eq!(a.review.state, ReviewState::HumanReviewed, "{:?}", h.app.message);
    assert_eq!(h.app.review.as_ref().unwrap().session.pos, pos + 1, "approve and next");
    assert!(h.app.review.as_ref().unwrap().session.done.contains(&pos));
}

#[test]
fn character_boxes_from_the_keyboard_and_resume_later() {
    let mut h = H::new("review-chars");
    let id = review_pass(&mut h, "いらっしゃいませ", Pass::CharBoxes);
    h.key(Key::C, Modifiers::NONE);
    let kids = crate::assist::chars_of(h.page(), &id).len();
    assert_eq!(kids, 8, "C proposes the eight boxes");
    assert!(crate::assist::chars_of(h.page(), &id).iter().all(|c| c.review.state == ReviewState::AiProposed));
    // resizing one proposal keeps it (and every other box) a visible proposal
    let first = chars_sorted(&h, &id)[0].clone();
    h.app.selection = vec![first.id.clone()];
    h.frame(vec![]);
    let focus = h.app.review_focus().expect("focused view");
    assert!(chars_sorted(&h, &id).iter().all(|c| focus.contains(&c.id)), "all eight boxes stay visible while one is selected");
    h.key(Key::ArrowDown, Modifiers::SHIFT);
    let resized = h.page().get(&first.id).unwrap().clone();
    assert_ne!(resized.geometry, first.geometry, "Shift+arrow grows the selected box");
    assert_eq!(resized.review.state, ReviewState::AiProposed, "still a proposal after resizing");
    assert_eq!(resized.provenance.fields["geometry"], FieldState::Corrected, "and it keeps its new size (no longer refitted)");
    assert!(chars_sorted(&h, &id).iter().all(|c| c.review.state == ReviewState::AiProposed));
    h.app.selection = vec![id.clone()];
    h.key(Key::C, Modifiers::NONE);
    assert!(crate::assist::chars_of(h.page(), &id).iter().all(|c| c.review.state == ReviewState::Draft), "C again accepts them (as drafts)");
    assert_eq!(crate::assist::chars_of(h.page(), &id).len(), 8);
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(crate::assist::chars_of(h.page(), &id).iter().all(|c| c.review.state == ReviewState::AiProposed), "undo works in review");
    // interrupt and resume
    h.key(Key::ArrowDown, Modifiers::NONE);
    let pos = h.app.review.as_ref().unwrap().session.pos;
    h.key(Key::Escape, Modifiers::NONE);
    assert!(h.app.review.is_none());
    h.key(Key::F6, Modifiers::NONE);
    assert_eq!(h.app.review.as_ref().unwrap().session.pos, pos, "resumed where we left off");
}

/// Fills a page with many labels and reports how long a frame takes.
fn crowded(h: &mut H, n_lines: usize) {
    h.app.go_to(3);
    h.frame(vec![]);
    let pid = h.page().page_id.clone();
    let p = h.app.cur_mut().unwrap();
    for i in 0..n_lines {
        let (x, y) = (60.0 + (i % 40) as f64 * 20.0, 60.0 + (i / 40) as f64 * 30.0);
        let mut a = Annotation::new(format!("a-stress-{i}"), Level::Line, &pid, Geometry::Box { x, y, w: 16.0, h: 26.0, rotation_deg: 0.0 }, "t", "t");
        a.transcription = Some(Transcription { text: "テスト文字".into(), writing_mode: WritingMode::VerticalRl, direction: Direction::Ttb, ..Default::default() });
        a.reading_order = Some(i as u32 + 1);
        p.annotations.push(a);
        for k in 0..3 {
            let mut c = Annotation::new(format!("a-stress-{i}-{k}"), Level::Char, &pid, Geometry::Box { x, y: y + k as f64 * 8.0, w: 16.0, h: 8.0, rotation_deg: 0.0 }, "t", "t");
            c.parent = Some(format!("a-stress-{i}"));
            c.transcription = Some(Transcription { text: "テ".into(), ..Default::default() });
            p.annotations.push(c);
        }
    }
    h.app.rebaseline();
}

#[test]
fn crowded_page_stays_fast() {
    let mut h = H::new("perf");
    crowded(&mut h, 1200);
    h.frame(vec![]);
    let t = std::time::Instant::now();
    let n = 20;
    for k in 0..n {
        let p = h.s([100.0 + k as f64, 100.0]);
        h.frame(vec![Event::PointerMoved(p)]);
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    eprintln!("PERF: {per:.1} ms per frame with {} annotations", h.page().annotations.len());
    assert!(per < 250.0, "a crowded page must stay interactive ({per:.0} ms per frame)");
}

#[test]
fn crowded_page_profile() {
    let mut h = H::new("perf2");
    crowded(&mut h, 300);
    let cfg = h.app.cfg();
    let page = h.page().clone();
    let t = std::time::Instant::now();
    let _ = crate::validate::validate_page(&cfg, &page, None);
    eprintln!("PROFILE validate {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let _ = crate::assist::metrics(&[&page], &cfg);
    eprintln!("PROFILE metrics {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let _ = crate::assist::suggest_order(&page, &cfg, Direction::Rtl);
    eprintln!("PROFILE suggest_order {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let _ = crate::assist::order_suspects(&page, &cfg, Direction::Rtl);
    eprintln!("PROFILE suspects {:?}", t.elapsed());
    let t = std::time::Instant::now();
    h.frame(vec![]);
    eprintln!("PROFILE frame {:?}", t.elapsed());
    let t = std::time::Instant::now();
    h.frame(vec![]);
    eprintln!("PROFILE frame2 {:?}", t.elapsed());
}


fn line_id(h: &H, text: &str) -> String {
    h.page().annotations.iter().find(|a| a.level == Level::Line && a.text() == text).unwrap().id.clone()
}

#[test]
fn rectangular_quads_become_boxes_and_skewed_ones_do_not() {
    let r = crate::geom::rect_corners_unordered([200.0, 100.0], (120.0, 40.0), 12f64.to_radians());
    let b = crate::assist::quad_to_box(&Geometry::Quad { points: r }).expect("a rotated rectangle qualifies");
    let Geometry::Box { w, h, rotation_deg, .. } = b else { panic!() };
    assert!((w - 120.0).abs() < 0.1 && (h - 40.0).abs() < 0.1 && (rotation_deg - 12.0).abs() < 0.1);
    for (o, n) in b.outline().iter().zip(r.iter()) {
        assert!(crate::geom::dist(*o, *n) < 0.2, "same corners after conversion");
    }
    let skew = [[0.0, 0.0], [100.0, 0.0], [120.0, 40.0], [20.0, 40.0]];
    assert!(crate::assist::quad_to_box(&Geometry::Quad { points: skew }).is_none());
    // the sign's perspective quadrilateral stays a quadrilateral
    let mut h = H::new("q2b");
    let sign = h.page().annotations.iter().find(|a| a.level == Level::Container && a.classes.contains("signage")).unwrap().clone();
    assert!(crate::assist::quad_to_box(&sign.geometry).is_none());
    // a rectangle drawn with the quad tool converts from the menu action
    h.app.go_to(3);
    h.frame(vec![]);
    h.key(Key::Q, Modifiers::NONE);
    h.drag([100.0, 100.0], [300.0, 180.0]);
    let id = h.app.selection[0].clone();
    let ctx = h.ctx.clone();
    h.app.convert_quads(&ctx, false);
    assert!(matches!(h.page().get(&id).unwrap().geometry, Geometry::Box { .. }));
}

#[test]
fn tightening_finds_the_text_inside_a_loose_box() {
    let mut h = H::new("tight");
    let id = line_id(&h, "どうして");
    let truth = h.page().get(&id).unwrap().geometry.clone();
    let Geometry::Box { x, y, w, h: bh, .. } = truth.clone() else { panic!() };
    // loosen the box by 15 px on each side, then tighten it
    h.app.cur_mut().unwrap().get_mut(&id).unwrap().geometry = Geometry::Box { x: x - 15.0, y: y - 15.0, w: w + 30.0, h: bh + 30.0, rotation_deg: 0.0 };
    h.app.rebaseline();
    h.app.selection = vec![id.clone()];
    h.frame(vec![]);
    assert!(h.app.tighten_suggestion().is_some(), "a looser box gets a suggestion");
    h.key(Key::T, Modifiers::COMMAND);
    let Geometry::Box { x: nx, y: ny, w: nw, h: nh, .. } = h.page().get(&id).unwrap().geometry.clone() else { panic!() };
    assert!(nw < w + 10.0 && nh < bh + 10.0, "tightened: {nw}x{nh} vs text {w}x{bh}");
    assert!(nx >= x - 6.0 && ny >= y - 6.0 && nx + nw <= x + w + 6.0 && ny + nh <= y + bh + 6.0, "still holds the text");
    assert!(h.app.tighten_suggestion().is_none(), "no further suggestion once tight");
    h.key(Key::Z, Modifiers::COMMAND);
    assert!(matches!(h.page().get(&id).unwrap().geometry, Geometry::Box { w: ww, .. } if (ww - (w + 30.0)).abs() < 0.01), "undo restores");
}

#[test]
fn quads_resize_from_their_edges_with_a_resize_cursor() {
    let mut h = H::new("edges");
    h.app.go_to(3);
    h.frame(vec![]);
    h.key(Key::Q, Modifiers::NONE);
    // a trapezoid (not a rectangle, so it stays a quadrilateral)
    for p in [[100.0, 100.0], [300.0, 100.0], [300.0, 200.0], [60.0, 200.0]] {
        h.click(p);
    }
    h.key(Key::V, Modifiers::NONE);
    let id = h.app.selection[0].clone();
    assert!(matches!(h.page().get(&id).unwrap().geometry, Geometry::Quad { .. }));
    // hover the right edge's middle handle: a horizontal double arrow
    let mid = h.s([300.0, 150.0]);
    h.frame(vec![Event::PointerMoved(mid)]);
    h.frame(vec![Event::PointerMoved(mid)]);
    assert_eq!(h.cursor, egui::CursorIcon::ResizeHorizontal);
    // and anywhere else along that edge too
    let near_top = h.s([300.0, 118.0]);
    h.frame(vec![Event::PointerMoved(near_top)]);
    h.frame(vec![Event::PointerMoved(near_top)]);
    assert_eq!(h.cursor, egui::CursorIcon::ResizeHorizontal, "edges resize away from their middle");
    h.drag([300.0, 118.0], [340.0, 128.0]);
    let Geometry::Quad { points } = h.page().get(&id).unwrap().geometry.clone() else { panic!() };
    assert!((points[1][0] - 340.0).abs() < 2.0 && (points[2][0] - 340.0).abs() < 2.0, "both right corners moved: {points:?}");
    assert!((points[1][1] - 100.0).abs() < 0.5 && (points[2][1] - 200.0).abs() < 0.5, "the edge kept its direction");
    // a box corner shows a diagonal arrow
    h.key(Key::B, Modifiers::NONE);
    h.drag([400.0, 300.0], [500.0, 360.0]);
    h.key(Key::V, Modifiers::NONE);
    let br = h.s([500.0, 360.0]);
    h.frame(vec![Event::PointerMoved(br)]);
    h.frame(vec![Event::PointerMoved(br)]);
    assert_eq!(h.cursor, egui::CursorIcon::ResizeNwSe);
}

#[test]
fn proposed_characters_follow_the_line_while_it_is_resized() {
    let mut h = H::new("refit");
    let id = line_id(&h, "いらっしゃいませ");
    h.app.selection = vec![id.clone()];
    h.frame(vec![]);
    h.key(Key::C, Modifiers::NONE);
    assert_eq!(crate::assist::chars_of(h.page(), &id).len(), 8);
    let Geometry::Box { x, y, w, h: bh, .. } = h.page().get(&id).unwrap().geometry.clone() else { panic!() };
    // drag the bottom edge down by 40 px
    h.app.selection = vec![id.clone()];
    h.key(Key::V, Modifiers::NONE);
    h.drag([x + w / 2.0, y + bh], [x + w / 2.0, y + bh + 40.0]);
    let kids = crate::assist::chars_of(h.page(), &id);
    let bottom = kids.iter().map(|c| c.geometry.bounds().1[1]).fold(0.0, f64::max);
    assert!((bottom - (y + bh + 40.0)).abs() < 2.5, "last character follows the new bottom: {bottom} vs {}", y + bh + 40.0);
    assert!(kids.iter().all(|c| crate::assist::untouched_proposal(c)), "refitted boxes are still untouched proposals");
}

#[test]
fn nearby_lines_get_a_text_region_and_nesting_follows_levels() {
    let mut h = H::new("regions");
    let a = line_id(&h, "どうして");
    let b = line_id(&h, "ここに？");
    let bubble = h.page().get(&a).unwrap().parent.clone().unwrap();
    let sugg = crate::assist::suggest_regions(h.page());
    assert!(sugg.iter().any(|s| s.lines.contains(&a) && s.lines.contains(&b)), "{sugg:?}");
    h.app.selection = vec![a.clone()];
    h.frame(vec![]);
    h.key(Key::G, Modifiers::COMMAND);
    let region = h.page().get(&a).unwrap().parent.clone().unwrap();
    let r = h.page().get(&region).unwrap().clone();
    assert_eq!(r.level, Level::Region);
    assert_eq!(r.parent.as_deref(), Some(bubble.as_str()), "region sits in the balloon");
    assert_eq!(h.page().get(&b).unwrap().parent.as_deref(), Some(region.as_str()));
    assert_eq!(r.review.state, ReviewState::AiProposed, "a proposal, not ground truth");
    let issues = crate::validate::validate_page(&h.app.cfg(), h.page(), None);
    assert!(!issues.iter().any(|i| i.rule == "parent_level"), "{issues:?}");
    // drawing a container around lines adopts them; a line can't become a panel's parent
    h.app.go_to(3);
    h.frame(vec![]);
    h.key(Key::Num4, Modifiers::NONE);
    h.key(Key::B, Modifiers::NONE);
    h.drag([100.0, 100.0], [130.0, 300.0]);
    h.drag([150.0, 100.0], [180.0, 300.0]);
    let lines: Vec<String> = h.page().annotations.iter().filter(|a| a.level == Level::Line).map(|a| a.id.clone()).collect();
    h.key(Key::Num2, Modifiers::NONE);
    h.drag([80.0, 80.0], [200.0, 320.0]);
    let cont = h.app.selection[0].clone();
    assert_eq!(h.page().get(&cont).unwrap().level, Level::Container);
    let dbg: Vec<_> = h.page().annotations.iter().map(|a| (a.level, a.id.clone(), a.parent.clone(), a.geometry.bounds())).collect();
    assert!(lines.iter().all(|l| h.page().get(l).unwrap().parent.as_deref() == Some(cont.as_str())), "the container adopted both lines: {cont} {dbg:#?}");
    assert!(crate::assist::parent_allowed(Level::Char, Level::Line) && !crate::assist::parent_allowed(Level::Panel, Level::Line));
}

#[test]
fn page_tree_folds_characters_under_their_line() {
    let mut h = H::new("tree");
    let line = line_id(&h, "喫茶店");
    h.app.selection.clear();
    h.frame(vec![]);
    // characters of a line are folded by default; toggling unfolds them
    assert!(!h.app.tree_toggled.contains(&line));
    h.app.tree_toggled.insert(line.clone());
    h.frame(vec![]);
    h.app.tree_toggled.clear();
    h.frame(vec![]);
}

#[test]
fn canvas_does_not_jump_when_selecting() {
    let mut h = H::new("jump");
    h.app.go_to(3);
    h.frame(vec![]);
    h.frame(vec![]);
    let before = h.app.canvas_rect;
    h.key(Key::Num4, Modifiers::NONE);
    h.key(Key::B, Modifiers::NONE);
    h.drag([100.0, 100.0], [130.0, 300.0]);
    h.frame(vec![]);
    h.frame(vec![]);
    assert_eq!(h.app.canvas_rect, before, "selecting must not resize the page area");
}

#[test]
fn character_cells_can_be_sized_one_by_one() {
    let mut h = H::new("cells");
    let id = line_id(&h, "いらっしゃいませ");
    h.app.selection = vec![id.clone()];
    h.key(Key::V, Modifiers::NONE);
    h.frame(vec![]);
    let borders = h.app.cell_borders();
    assert_eq!(borders.len(), 7, "eight previewed cells have seven borders");
    // drag the first border (between い and ら) 30 px down, with a resize cursor
    let (_, a, b) = borders[0].clone();
    let mid = h.app.border_grip(a, b);
    let sp = h.s(mid);
    h.frame(vec![Event::PointerMoved(sp)]);
    h.frame(vec![Event::PointerMoved(sp)]);
    assert_eq!(h.cursor, egui::CursorIcon::ResizeVertical, "vertical text: borders move up and down");
    h.drag(mid, [mid[0], mid[1] + 30.0]);
    assert!(h.app.preview_splits(&id).is_some(), "the preview remembers the new border");
    let line = h.page().get(&id).unwrap().clone();
    let (cells, _) = crate::assist::propose_chars_split(&line, h.page(), &h.app.cfg().quick.chars, h.app.preview_splits(&id).as_ref());
    let height = |q: &[[f64; 2]; 4]| q[3][1] - q[0][1];
    assert!(height(&cells[0].quad) > height(&cells[1].quad) + 20.0, "first cell is now taller than the second");
    // C creates the boxes with these sizes
    h.key(Key::C, Modifiers::NONE);
    let mut kids: Vec<Annotation> = crate::assist::chars_of(h.page(), &id).into_iter().cloned().collect();
    kids.sort_by_key(|c| c.reading_order);
    assert_eq!(kids.len(), 8);
    let hk = |c: &Annotation| { let (lo, hi) = c.geometry.bounds(); hi[1] - lo[1] };
    let (h0, h1) = (hk(&kids[0]), hk(&kids[1]));
    assert!(h0 > h1 + 20.0, "created boxes keep the chosen sizes: {h0} vs {h1}");
    // resizing the line keeps each cell's share
    let Geometry::Box { x, y, w, h: bh, .. } = line.geometry.clone() else { panic!() };
    h.drag([x + w / 2.0, y + bh], [x + w / 2.0, y + bh + 40.0]);
    let mut kids: Vec<Annotation> = crate::assist::chars_of(h.page(), &id).into_iter().cloned().collect();
    kids.sort_by_key(|c| c.reading_order);
    let ratio_before = h0 / h1;
    let ratio_after = hk(&kids[0]) / hk(&kids[1]);
    assert!((ratio_before - ratio_after).abs() < 0.05, "proportions kept: {ratio_before} vs {ratio_after}");
    // the border between two created boxes can be dragged too; both boxes follow
    let pair = h.app.cell_borders();
    assert_eq!(pair.len(), 7, "borders between the created boxes");
    let (_, a, b) = pair[3].clone();
    let mid = h.app.border_grip(a, b);
    let (k3, k4) = (hk(&kids[3]), hk(&kids[4]));
    h.drag(mid, [mid[0], mid[1] - 12.0]);
    let mut kids: Vec<Annotation> = crate::assist::chars_of(h.page(), &id).into_iter().cloned().collect();
    kids.sort_by_key(|c| c.reading_order);
    assert!(hk(&kids[3]) < k3 - 6.0 && hk(&kids[4]) > k4 + 6.0, "one shrinks, its neighbour grows");
    assert!((kids[3].geometry.bounds().1[1] - kids[4].geometry.bounds().0[1]).abs() < 0.5, "they still touch");
    assert!(!crate::assist::untouched_proposal(&kids[3]), "a person adjusted it");
}

fn chars_sorted(h: &H, line: &str) -> Vec<Annotation> {
    let mut v: Vec<Annotation> = crate::assist::chars_of(h.page(), line).into_iter().cloned().collect();
    v.sort_by_key(|c| c.reading_order);
    v
}

#[test]
fn next_item_stays_on_the_same_line() {
    let mut h = H::new("next");
    let long = line_id(&h, "いらっしゃいませ");
    let short = line_id(&h, "どうして");
    for id in [&long, &short] {
        let id = id.clone();
        h.app.op(|p, cfg, user, now| crate::assist::regenerate_chars(p, &id, cfg, user, now));
    }
    let kids = chars_sorted(&h, &long);
    h.app.selection = vec![kids[4].id.clone()];
    h.key(Key::N, Modifiers::NONE);
    assert_eq!(h.app.selection, vec![kids[5].id.clone()], "the 6th character of the same line comes next");
}

#[test]
fn merging_characters_undoes_in_one_step() {
    let mut h = H::new("merge-undo");
    let id = line_id(&h, "いらっしゃいませ");
    let lid = id.clone();
    h.app.op(|p, cfg, user, now| crate::assist::regenerate_chars(p, &lid, cfg, user, now));
    let before = h.page().clone();
    let kids = chars_sorted(&h, &id);
    h.app.selection = vec![kids[0].id.clone(), kids[1].id.clone()];
    h.key(Key::M, Modifiers::NONE);
    let merged = h.page().get(&kids[0].id).unwrap().clone();
    assert_eq!(merged.text(), "いら", "characters join without a line break");
    assert!(h.page().get(&kids[1].id).is_none());
    h.key(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.page().get(&kids[0].id).unwrap().geometry, before.get(&kids[0].id).unwrap().geometry, "one undo restores the first box too");
    assert_eq!(h.page().get(&kids[0].id).unwrap().text(), "い");
    assert!(h.page().get(&kids[1].id).is_some());
}

#[test]
fn review_buttons_act_on_the_reviewed_line_and_own_work_is_submitted() {
    let mut h = H::new("review-own");
    // this line was typed by the reviewer themself
    let lid = line_id(&h, "いらっしゃいませ");
    h.app.cur_mut().unwrap().get_mut(&lid).unwrap().created.by = "you".into();
    h.app.rebaseline();
    let id = review_on(&mut h, "いらっしゃいませ");
    // clicking a character on the page must not redirect the review actions
    h.key(Key::C, Modifiers::NONE);
    let first_char = chars_sorted(&h, &id)[0].clone();
    h.app.selection = vec![first_char.id.clone()];
    h.key(Key::Num1, Modifiers::NONE);
    assert_eq!(h.page().get(&id).unwrap().recognizability, Some(Recognizability::Readable), "answer went to the reviewed line");
    assert_eq!(h.page().get(&first_char.id).unwrap().recognizability, None);
    h.key(Key::Y, Modifiers::NONE); // text type from the balloon
    h.key(Key::C, Modifiers::NONE); // accept the character boxes
    // the reviewer is the person who last changed it: approval is refused by policy, so it is submitted
    let pos = h.app.review.as_ref().unwrap().session.pos;
    h.key(Key::Enter, Modifiers::NONE);
    let a = h.app.store.as_ref().unwrap().pages.iter().find_map(|p| p.get(&id)).unwrap().clone();
    assert_eq!(h.app.review.as_ref().unwrap().session.pos, pos + 1, "moved on: {:?}", h.app.message);
    assert_eq!(a.review.state, ReviewState::Submitted);
    assert!(a.review.approvals.is_empty());
    assert_eq!(a.checks["recognizability"].state, CheckState::Confirmed);
}

#[test]
fn rectangles_are_boxes_and_shapes_convert() {
    let mut h = H::new("shapes2");
    h.app.go_to(3);
    h.frame(vec![]);
    h.key(Key::Q, Modifiers::NONE);
    h.drag([100.0, 100.0], [300.0, 180.0]);
    let id = h.app.selection[0].clone();
    assert!(matches!(h.page().get(&id).unwrap().geometry, Geometry::Box { .. }), "a rectangle drawn with the quadrilateral tool is a box");
    // convert to a quadrilateral on purpose: it stays one even after editing a corner
    let ctx = h.ctx.clone();
    h.app.convert_selection(&ctx, "quad");
    h.key(Key::V, Modifiers::NONE);
    h.drag([300.0, 180.0], [301.0, 181.0]);
    assert!(matches!(h.page().get(&id).unwrap().geometry, Geometry::Quad { .. }));
    h.app.convert_selection(&ctx, "polygon");
    assert!(matches!(&h.page().get(&id).unwrap().geometry, Geometry::Polygon { points } if points.len() == 4));
    h.app.convert_selection(&ctx, "box");
    assert!(matches!(h.page().get(&id).unwrap().geometry, Geometry::Box { .. }));
    // double-clicking an edge adds a point (the box becomes a polygon)
    let p = h.s([200.0, 100.0]);
    h.frame(vec![Event::PointerMoved(p)]);
    for _ in 0..2 {
        h.frame(vec![H::btn(p, true)]);
        h.frame(vec![H::btn(p, false)]);
    }
    h.t += 0.6;
    h.frame(vec![]);
    assert!(matches!(&h.page().get(&id).unwrap().geometry, Geometry::Polygon { points } if points.len() == 5), "{:?}", h.page().get(&id).unwrap().geometry);
    // nearly level boxes get a straighten suggestion
    let g = Geometry::Box { x: 0.0, y: 0.0, w: 100.0, h: 20.0, rotation_deg: 1.4 };
    assert!(crate::assist::straighten(&g, 3.0).is_some());
    assert!(crate::assist::straighten(&Geometry::Box { x: 0.0, y: 0.0, w: 100.0, h: 20.0, rotation_deg: 12.0 }, 3.0).is_none());
}

#[test]
fn ctrl_click_adds_to_the_selection() {
    let mut h = H::new("ctrl");
    let a = line_id(&h, "どうして");
    let b = line_id(&h, "ここに？");
    h.key(Key::V, Modifiers::NONE);
    let ca = h.page().get(&a).unwrap().geometry.bounds();
    let cb = h.page().get(&b).unwrap().geometry.bounds();
    h.click([(ca.0[0] + ca.1[0]) / 2.0, (ca.0[1] + ca.1[1]) / 2.0 + 30.0]);
    let pb = h.s([(cb.0[0] + cb.1[0]) / 2.0, (cb.0[1] + cb.1[1]) / 2.0 + 30.0]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::COMMAND), Event::PointerMoved(pb)]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::COMMAND), Event::PointerButton { pos: pb, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::COMMAND }]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::COMMAND), Event::PointerButton { pos: pb, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::COMMAND }]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    assert_eq!(h.app.selection.len(), 2, "{:?}", h.app.selection);
}

#[test]
fn furigana_links_to_its_characters_and_has_no_reading_order() {
    let mut h = H::new("ruby");
    let base = line_id(&h, "どうして");
    let pid = h.page().page_id.clone();
    let parent = h.page().get(&base).unwrap().parent.clone();
    let mut ruby = Annotation::new("a-ruby".into(), Level::Line, &pid, Geometry::Box { x: 700.0, y: 160.0, w: 10.0, h: 40.0, rotation_deg: 0.0 }, "t", "t");
    ruby.parent = parent;
    ruby.reading_order = Some(9);
    ruby.transcription = Some(Transcription { text: "どう".into(), writing_mode: WritingMode::VerticalRl, direction: Direction::Ttb, ..Default::default() });
    h.app.cur_mut().unwrap().annotations.push(ruby);
    h.app.rebaseline();
    assert!(crate::assist::link_furigana(h.app.cur_mut().unwrap(), "a-ruby", &base));
    let r = h.page().get("a-ruby").unwrap().clone();
    assert!(crate::assist::is_furigana(&r) && r.reading_order.is_none() && r.attributes.contains("furigana"));
    let cfg = h.app.cfg();
    assert!(!crate::assist::suggest_order(h.page(), &cfg, Direction::Rtl).iter().any(|x| x.0 == "a-ruby"), "furigana is not numbered");
    assert!(!crate::assist::needs(&r, h.page(), &cfg).iter().any(|n| n.field == "reading_order"));
    let issues = crate::validate::validate_page(&cfg, h.page(), None);
    assert!(!issues.iter().any(|i| i.rule == "reading_order_partial"), "{issues:?}");
}

#[test]
fn alt_resizing_a_character_shares_the_rest_and_pins_it() {
    let mut h = H::new("smart");
    let id = line_id(&h, "いらっしゃいませ");
    let lid = id.clone();
    h.app.op(|p, cfg, user, now| crate::assist::regenerate_chars(p, &lid, cfg, user, now));
    let height = |c: &Annotation| { let (lo, hi) = c.geometry.bounds(); hi[1] - lo[1] };
    let kids = chars_sorted(&h, &id);
    let (lo, hi) = h.page().get(&id).unwrap().geometry.bounds();
    let h0 = height(&kids[0]);
    // select the first character and Alt-drag its bottom edge 20 px down
    h.app.selection = vec![kids[0].id.clone()];
    h.key(Key::V, Modifiers::NONE);
    let (clo, chi) = kids[0].geometry.bounds();
    let edge = [(clo[0] + chi[0]) / 2.0, chi[1]];
    let (pa, pb) = (h.s(edge), h.s([edge[0], edge[1] + 20.0]));
    h.frame(vec![Event::PointerMoved(pa)]);
    h.frame(vec![Event::PointerButton { pos: pa, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::ALT }]);
    for k in 1..=6 {
        h.frame(vec![Event::ModifiersChanged(Modifiers::ALT), Event::PointerMoved(pa + (pb - pa) * (k as f32 / 6.0))]);
    }
    h.frame(vec![Event::PointerButton { pos: pb, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::ALT }]);
    h.t += 0.6;
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    let kids2 = chars_sorted(&h, &id);
    assert!(height(&kids2[0]) > h0 + 15.0, "first character grew: {} vs {h0}", height(&kids2[0]));
    assert!(height(&kids2[1]) < height(&kids[1]) - 1.0, "the others shrank to make room");
    let last = kids2.last().unwrap().geometry.bounds().1[1];
    assert!((last - hi[1]).abs() < 1.0 && kids2[0].geometry.bounds().0[1] >= lo[1] - 1.0, "still fills the line exactly");
    assert!(kids2[0].provenance.heuristic.as_ref().unwrap().pinned, "the resized box is pinned");
    // resizing another box leaves the pinned one alone
    let pinned_geom = kids2[0].geometry.clone();
    let c5 = kids2[5].clone();
    {
        let (l5, h5) = c5.geometry.bounds();
        assert!(crate::assist::smart_resize_char(h.app.cur_mut().unwrap(), &c5.id, [(l5[0] + h5[0]) / 2.0, h5[1] + 10.0]));
    }
    assert_eq!(h.page().get(&kids2[0].id).unwrap().geometry, pinned_geom);
    let n = crate::assist::unpin_chars(h.app.cur_mut().unwrap(), &id);
    assert_eq!(n, 2);
}

#[test]
fn publishing_text_has_its_own_classes() {
    let cfg = ProjectConfig::default();
    for c in ["title", "credits", "page_number"] {
        assert!(cfg.class(c).is_some(), "{c}");
    }
    assert_eq!(cfg.text_classes.last().unwrap().id, "other");
}

#[test]
fn review_centres_the_item() {
    let mut h = H::new("centre");
    let id = review_on(&mut h, "いらっしゃいませ");
    h.frame(vec![]);
    let r = screen_rect(&h, &id);
    let c = h.app.canvas_rect;
    eprintln!("CENTRE item {:?} canvas {:?}", r.center(), c.center());
    assert!((r.center().x - c.center().x).abs() < 6.0, "centred horizontally: {:?} vs {:?}", r.center(), c.center());
}

#[test]
fn line_boxes_resize_from_the_keyboard_in_review() {
    let mut h = H::new("review-lines");
    let id = review_pass(&mut h, "いらっしゃいませ", Pass::LineBoxes);
    let before = h.page().get(&id).unwrap().geometry.bounds();
    h.key(Key::ArrowRight, Modifiers::SHIFT);
    let after = h.page().get(&id).unwrap().geometry.bounds();
    assert!(after.1[0] > before.1[0] && (after.0[0] - before.0[0]).abs() < 1e-9, "Shift+→ grows the right side: {before:?} → {after:?}");
    h.key(Key::ArrowRight, Modifiers::COMMAND);
    let back = h.page().get(&id).unwrap().geometry.bounds();
    assert!((back.1[0] - before.1[0]).abs() < 1e-6, "Ctrl+→ shrinks it again");
    let pos = h.app.review.as_ref().unwrap().session.pos;
    h.key(Key::Enter, Modifiers::NONE);
    assert_eq!(h.page().get(&id).unwrap().checks["geometry"].state, CheckState::Confirmed);
    assert_eq!(h.app.review.as_ref().unwrap().session.pos, pos + 1);
}

#[test]
fn a_reviewed_page_is_committed_and_the_others_stay_uncommitted() {
    let mut h = H::new("review-git");
    if !crate::git::available() {
        return;
    }
    let root = h.app.store.as_ref().unwrap().root.clone();
    crate::git::init(&root, "tester").unwrap();
    let other = h.app.pages()[1].page_id.clone();
    h.app.op_on(1, |p, _, _, _| p.completeness.inspected = true);
    h.key(Key::S, Modifiers::COMMAND);
    h.key(Key::F6, Modifiers::NONE);
    h.app.review.as_mut().unwrap().scope = super::review_mode::Scope::ThisPage;
    h.key(Key::Enter, Modifiers::NONE);
    let pid = h.page().page_id.clone();
    for _ in 0..40 {
        if h.app.review.as_ref().unwrap().session.finished() {
            break;
        }
        h.key(Key::PageDown, Modifiers::NONE);
    }
    let r = h.app.review.as_ref().unwrap();
    assert!(r.session.finished() && r.session.completed_pages.contains(&pid), "{:?}", r.session.items.len());
    let left = crate::git::pending(&root);
    assert!(!left.iter().any(|f| f.contains(&format!("{pid}.json"))), "the reviewed page is committed: {left:?}");
    assert!(left.iter().any(|f| f.contains(&other)), "the unfinished page is not: {left:?}");
}

#[test]
fn hide_find_bulk_edit_and_group() {
    let mut h = H::new("find");
    let id = line_id(&h, "いらっしゃいませ");
    h.app.selection = vec![id.clone()];
    h.key(Key::H, Modifiers::NONE);
    assert!(h.app.hidden.contains(&id) && h.app.selection.is_empty());
    let (lo, hi) = h.page().get(&id).unwrap().geometry.bounds();
    h.click([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
    assert!(!h.app.selection.contains(&id), "hidden labels cannot be clicked");
    h.key(Key::H, Modifiers::SHIFT);
    assert!(h.app.hidden.is_empty(), "Shift+H shows them again");
    // several conditions at once, over all pages
    h.app.find.conds = vec![
        super::find::Cond { crit: super::find::Crit::Level(Level::Line), not: false },
        super::find::Cond { crit: super::find::Crit::Text("らっしゃ".into()), not: false },
    ];
    h.app.find.scope = super::find::FindScope::AllPages;
    h.app.run_find();
    assert_eq!(h.app.find.results.len(), 1);
    h.app.find.conds[1].not = true;
    h.app.run_find();
    assert!(h.app.find.results.len() > 1 && !h.app.find.results.iter().any(|r| r.1 == id));
    // Ctrl+G groups exactly the selected lines
    let lines = vec![line_id(&h, "どうして"), line_id(&h, "いらっしゃいませ")];
    h.app.selection = lines.clone();
    h.key(Key::G, Modifiers::COMMAND);
    let g = h.app.selection[0].clone();
    assert_eq!(h.page().get(&g).unwrap().level, Level::Region);
    for l in &lines {
        assert_eq!(h.page().get(l).unwrap().parent.as_deref(), Some(g.as_str()), "exactly the selected lines are grouped");
    }
    assert_eq!(h.page().annotations.iter().filter(|a| a.parent.as_deref() == Some(g.as_str())).count(), 2);
}

#[test]
fn ctrl_dragging_a_corner_distorts_the_box() {
    let mut h = H::new("transform");
    let id = line_id(&h, "いらっしゃいませ");
    h.app.selection = vec![id.clone()];
    h.frame(vec![]);
    let q = crate::assist::to_quad(&h.page().get(&id).unwrap().geometry);
    let Geometry::Quad { points } = q else { panic!() };
    let (a, b) = (points[1], [points[1][0] + 12.0, points[1][1] - 8.0]);
    let (pa, pb) = (h.s(a), h.s(b));
    let m = Modifiers::COMMAND;
    h.frame(vec![Event::ModifiersChanged(m), Event::PointerMoved(pa)]);
    h.frame(vec![Event::ModifiersChanged(m), Event::PointerButton { pos: pa, button: PointerButton::Primary, pressed: true, modifiers: m }]);
    for k in 1..=6 {
        h.frame(vec![Event::ModifiersChanged(m), Event::PointerMoved(pa + (pb - pa) * (k as f32 / 6.0))]);
    }
    h.frame(vec![Event::ModifiersChanged(m), Event::PointerButton { pos: pb, button: PointerButton::Primary, pressed: false, modifiers: m }]);
    h.frame(vec![Event::ModifiersChanged(Modifiers::NONE)]);
    let Geometry::Quad { points: after } = h.page().get(&id).unwrap().geometry.clone() else { panic!("became a quadrilateral") };
    assert!(near(after[1], b), "{:?} vs {b:?}", after[1]);
    assert!(near(after[0], points[0]) && near(after[2], points[2]) && near(after[3], points[3]), "only that corner moved");
    // skew moves the whole top edge
    let skew = super::canvas::transform_quad(points, 1, [5.0, 0.0], 1);
    assert_eq!((skew[0][0] - points[0][0], skew[1][0] - points[1][0]), (5.0, 5.0));
}

/// The added models through the real window: text blocks, then lines and characters, then the
/// characters of one line (skipped when the models or ONNX Runtime are not on this computer).
#[test]
fn ai_text_blocks_then_lines_and_characters_through_the_window() {
    let o = crate::zoo::Options::default();
    if crate::runtime::locate(crate::runtime::Component::OnnxRuntime, None).is_none() || crate::zoo::locate(crate::zoo::ZooId::AnimeText, &o).is_err() || crate::zoo::locate(crate::zoo::ZooId::Charspot, &o).is_err() {
        eprintln!("models or ONNX Runtime not here; skipping");
        return;
    }
    let mut h = H::new("zoo");
    h.app.page = 0;
    h.app.op(|p, _, _, _| p.annotations.clear());
    let wait = |h: &mut H| {
        for _ in 0..1200 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            h.frame(vec![]);
            if h.app.worker.pending() == 0 {
                break;
            }
        }
        for _ in 0..3 {
            h.frame(vec![]);
        }
    };
    let ctx = h.ctx.clone();
    h.app.ai_zoo_pages(&[0], ai::JobKind::Blocks, &ctx);
    wait(&mut h);
    let regions = h.app.cur().unwrap().annotations.iter().filter(|a| a.level == Level::Region).count();
    assert!(regions >= 3, "text blocks proposed: {regions} ({:?})", h.app.message);
    h.app.ai_read_blocks(0, None, &ctx);
    wait(&mut h);
    let page = h.app.cur().unwrap().clone();
    let line = page.annotations.iter().find(|a| a.level == Level::Line && a.text() == "いらっしゃいませ").expect("the line was read");
    assert_eq!(crate::assist::chars_of(&page, &line.id).len(), 8);
    assert!(line.parent.as_ref().is_some_and(|p| page.get(p).is_some_and(|r| r.level == Level::Region)), "the line sits in its block");
    // characters of a line drawn by hand, with its text typed in
    let id = line.id.clone();
    h.app.op(|p, _, _, _| p.annotations.retain(|a| !(a.level == Level::Char && a.parent.as_deref() == Some(id.as_str()))));
    h.app.selection = vec![id.clone()];
    h.app.ai_find_chars(&ctx);
    wait(&mut h);
    let page = h.app.cur().unwrap().clone();
    let chars = crate::assist::chars_of(&page, &id);
    assert_eq!(chars.len(), 8, "{:?}", h.app.message);
    assert!(chars.iter().all(|c| c.provenance.origin == Origin::Ai && c.review.state == ReviewState::AiProposed));
    assert!(h.app.models_ui.loaded.contains_key(&crate::zoo::ZooId::Charspot), "the window knows what ran");
    // one undo step per run
    h.app.undo(false);
    assert!(crate::assist::chars_of(h.app.cur().unwrap(), &id).is_empty());
}
