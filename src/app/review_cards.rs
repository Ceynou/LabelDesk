//! The Review Mode screens: workflow choices, the bar of passes, and one card per pass.

use super::review_mode::{Answer, RView, Scope, TMode, options};
use super::*;
use crate::assist::{self, CharStatus, Need, NeedKind, Pass, Suggest};
use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{Align, Align2, FontId, Layout, Pos2, RichText, Sense, TextEdit, pos2};
use unicode_segmentation::UnicodeSegmentation;

/// Grapheme diff of two readings: (model graphemes with "kept" flags, current graphemes with flags).
fn diff(a: &str, b: &str) -> (Vec<(String, bool)>, Vec<(String, bool)>) {
    let x: Vec<&str> = a.graphemes(true).collect();
    let y: Vec<&str> = b.graphemes(true).collect();
    let mut t = vec![vec![0usize; y.len() + 1]; x.len() + 1];
    for i in (0..x.len()).rev() {
        for j in (0..y.len()).rev() {
            t[i][j] = if x[i] == y[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut kx, mut ky) = (vec![false; x.len()], vec![false; y.len()]);
    while i < x.len() && j < y.len() {
        if x[i] == y[j] {
            kx[i] = true;
            ky[j] = true;
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    (x.iter().zip(kx).map(|(g, k)| (g.to_string(), k)).collect(), y.iter().zip(ky).map(|(g, k)| (g.to_string(), k)).collect())
}

fn diff_job(parts: &[(String, bool)], changed: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (g, keep) in parts {
        let mut f = TextFormat::simple(FontId::proportional(20.0), if *keep { Color32::WHITE } else { changed });
        if !keep {
            f.background = changed.gamma_multiply(0.25);
        }
        job.append(g, 0.0, f);
    }
    job
}

fn suggestion_label(s: &Suggest, cfg: &ProjectConfig) -> String {
    match s {
        Suggest::Recognizability(r) => r.name().to_string(),
        Suggest::Class(c) => cfg.class(c).map(|c| c.name.clone()).unwrap_or(c.clone()),
        Suggest::Language(l, sc) => format!("{} {}", l.clone().unwrap_or_default(), sc.clone().unwrap_or_default()),
        Suggest::Writing(wm, d) => format!("{} · {}", wm.name(), d.name()),
        Suggest::Order(n) => format!("order {n}"),
        Suggest::ProposeChars(n) => format!("propose {n} boxes"),
        Suggest::AcceptChars(n) => format!("accept {n} boxes"),
        Suggest::AcceptPerspective => "accept the flattening".into(),
    }
}

impl App {
    /// Transcription drawn over the image in each character cell (follows writing direction and
    /// perspective). Tinted and outlined so it never looks like image pixels.
    pub fn draw_overlay(&self, painter: &egui::Painter, to_screen: &dyn Fn(P) -> Pos2, a: &Annotation, opacity: f32) {
        let Some(page) = self.cur() else { return };
        let (cells, _) = assist::propose_chars(a, page, &CharPolicy { skip_spaces: true, skip_punctuation: false, proportional_latin: true, monospace: false });
        let col = Color32::from_rgb(255, 80, 220).gamma_multiply(opacity);
        for c in cells {
            let q: Vec<Pos2> = c.quad.iter().map(|p| to_screen(*p)).collect();
            let centre = pos2(q.iter().map(|p| p.x).sum::<f32>() / 4.0, q.iter().map(|p| p.y).sum::<f32>() / 4.0);
            let size = (q[0].distance(q[1]).min(q[0].distance(q[3])) * 0.82).clamp(6.0, 120.0);
            painter.add(egui::Shape::convex_polygon(q.clone(), Color32::from_rgba_unmultiplied(30, 0, 40, (150.0 * opacity) as u8), Stroke::NONE));
            for d in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0), vec2(0.0, 1.0)] {
                painter.text(centre + d, Align2::CENTER_CENTER, &c.text, FontId::proportional(size), Color32::from_black_alpha((220.0 * opacity) as u8));
            }
            painter.text(centre, Align2::CENTER_CENTER, &c.text, FontId::proportional(size), col);
        }
    }

    fn crop(&mut self, ui: &mut Ui, a: &Annotation, max: Vec2) {
        let ctx = ui.ctx().clone();
        let Some(pid) = self.cur_id() else { return };
        let (Some(img), Some(t)) = (self.page_image(&pid), self.page_texture(&ctx, &pid)) else { return };
        let (lo, hi) = a.geometry.bounds();
        let pad = ((hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.08).max(4.0);
        let (lo, hi) = ([(lo[0] - pad).max(0.0), (lo[1] - pad).max(0.0)], [(hi[0] + pad).min(img.width() as f64), (hi[1] + pad).min(img.height() as f64)]);
        let (bw, bh) = ((hi[0] - lo[0]).max(1.0) as f32, (hi[1] - lo[1]).max(1.0) as f32);
        let s = (max.x / bw).min(max.y / bh).min(4.0);
        let (rect, _) = ui.allocate_exact_size(vec2(bw * s, bh * s), Sense::hover());
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        let uv = Rect::from_min_max(pos2(lo[0] as f32 / iw, lo[1] as f32 / ih), pos2(hi[0] as f32 / iw, hi[1] as f32 / ih));
        ui.painter().image(t.id(), rect, uv, Color32::WHITE);
        let to = |p: P| pos2(rect.left() + (p[0] - lo[0]) as f32 * s, rect.top() + (p[1] - lo[1]) as f32 * s);
        let pts: Vec<Pos2> = a.geometry.outline().iter().map(|p| to(*p)).collect();
        ui.painter().add(egui::Shape::closed_line(pts, Stroke::new(1.5, Color32::from_rgb(255, 167, 38))));
    }

    /// Thin top bar replacing the page header while reviewing: the page and its passes.
    pub fn review_bar(&mut self, ui: &mut Ui) {
        let Some(r) = &self.review else { return };
        let s = &r.session;
        let setup = r.setup;
        let (page_no, pages) = (s.page + 1, s.pages.len());
        let order = s.workflow.order();
        let cur = s.pass();
        let in_pass: Vec<usize> = s.items.iter().enumerate().filter(|(_, i)| Some(i.pass) == cur).map(|(k, _)| k).collect();
        let (k, n) = (in_pass.iter().position(|x| *x == s.pos).map(|x| x + 1).unwrap_or(0), in_pass.len());
        let ahead: BTreeSet<Pass> = s.items[s.pos.min(s.items.len())..].iter().map(|i| i.pass).collect();
        let finished = s.finished();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("REVIEW").strong().color(Color32::from_rgb(255, 200, 80)));
            if setup {
                ui.label(RichText::new("choose your passes on the right").color(MUTED));
            } else if finished {
                ui.label(RichText::new(format!("All {pages} page(s) done")).color(GOOD));
            } else {
                ui.label(RichText::new(format!("Page {page_no} / {pages}")).strong());
                ui.separator();
                for p in &order {
                    let here = Some(*p) == cur;
                    let col = if here { Color32::WHITE } else if ahead.contains(p) { MUTED } else { Color32::from_gray(90) };
                    let txt = if here { format!("{}  {k}/{n}", p.name()) } else { p.name().to_string() };
                    ui.label(RichText::new(txt).size(11.5).color(col).background_color(if here { ACCENT } else { Color32::TRANSPARENT }));
                }
            }
        });
        ui.horizontal(|ui| {
            let Some(r) = &mut self.review else { return };
            ui.label(RichText::new("View").color(MUTED));
            for (v, name, key) in [(RView::Focused, "Focus", "Space"), (RView::Context, "Context", "Space"), (RView::Page, "Page", "P")] {
                if ui.selectable_label(r.view == v, name).on_hover_text(key).clicked() {
                    r.view = v;
                    r.framed = None;
                }
            }
            ui.checkbox(&mut r.active_border, "Border").on_hover_text("B");
            ui.checkbox(&mut r.other_borders, "Other labels").on_hover_text("O");
            if ui.checkbox(&mut self.settings.review_fullscreen, "Full screen").changed() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.settings.review_fullscreen));
                self.settings.save();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Exit review  Esc").clicked() {
                    self.exit_review();
                }
                let can_restart = self.review.as_ref().is_some_and(|r| !r.setup);
                if can_restart && ui.button("New review…").on_hover_text("Choose the passes and pages again").clicked() {
                    if let Some(r) = &mut self.review {
                        r.setup = true;
                    }
                }
            });
        });
    }

    fn setup_card(&mut self, ui: &mut Ui) {
        ui.heading("Your review");
        ui.label(RichText::new("Each page goes through the passes below, one after the other. The page is saved — and committed when the project uses Git — once its last pass is done.").color(MUTED).size(11.5));
        ui.add_space(6.0);
        let mut wf = self.settings.workflow.clone();
        for p in Pass::ALL {
            let mut on = wf.passes.contains(&p);
            if ui.checkbox(&mut on, RichText::new(p.name()).strong()).on_hover_text(p.hint()).changed() {
                if on {
                    wf.passes.push(p);
                } else {
                    wf.passes.retain(|x| *x != p);
                }
            }
            if p == Pass::MissingBoxes && on {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(24.0);
                    ui.radio_value(&mut wf.missing_at_end, false, "right after the line boxes");
                    ui.radio_value(&mut wf.missing_at_end, true, "at the end (new boxes then get the passes they missed)");
                });
            }
        }
        ui.checkbox(&mut wf.skip_checked, "Leave out what is already checked");
        if wf != self.settings.workflow {
            self.settings.workflow = wf;
            self.settings.save();
        }
        ui.add_space(6.0);
        let found = self.find.results.len();
        let Some(r) = &mut self.review else { return };
        ui.label(RichText::new("Pages").strong());
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut r.scope, Scope::AllPages, "All pages");
            ui.radio_value(&mut r.scope, Scope::FromHere, "From this page on");
            ui.radio_value(&mut r.scope, Scope::ThisPage, "This page only");
            ui.add_enabled_ui(found > 0, |ui| {
                ui.radio_value(&mut r.scope, Scope::Found, format!("Search results ({found} labels)")).on_hover_text("Find labels first (Ctrl+F); page passes are left out");
            });
        });
        let scope = r.scope;
        ui.add_space(8.0);
        if ui.add(egui::Button::new(RichText::new("Start  ⏎").color(Color32::WHITE)).fill(ACCENT).min_size(vec2(120.0, 30.0))).clicked() {
            self.start_review(scope);
        }
    }

    /// Comment and tag any label while reviewing (the selection, else the reviewed label).
    fn notes_row(&mut self, ui: &mut Ui, aid: &str) {
        let Some(r) = &mut self.review else { return };
        let mut comment = std::mem::take(&mut r.comment);
        let mut tag = std::mem::take(&mut r.tag);
        let (mut add_c, mut add_t) = (false, false);
        let target = self.selection.first().cloned().unwrap_or_else(|| aid.to_string());
        let tags: Vec<String> = self.ann(&target).map(|a| a.tags.iter().cloned().collect()).unwrap_or_default();
        ui.horizontal(|ui| {
            let w = ui.available_width();
            let r1 = ui.add(TextEdit::singleline(&mut comment).hint_text("Comment…").desired_width(w * 0.6));
            add_c = r1.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !comment.trim().is_empty();
            let r2 = ui.add(TextEdit::singleline(&mut tag).hint_text("Tag…").desired_width(ui.available_width()));
            add_t = r2.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !tag.trim().is_empty();
        });
        let mut remove = None;
        if !tags.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for t in &tags {
                    if ui.small_button(format!("#{t} ✕")).on_hover_text("Remove this tag").clicked() {
                        remove = Some(t.clone());
                    }
                }
            });
        }
        if add_c {
            let text = comment.trim().to_string();
            let id = target.clone();
            self.op(move |p, _, user, now| {
                if let Some(a) = p.get_mut(&id) {
                    review::add_note(a, NoteKind::Comment, None, &text, user, now);
                }
            });
            comment.clear();
        }
        if add_t || remove.is_some() {
            let t = tag.trim().to_string();
            let id = target.clone();
            self.op(move |p, _, _, _| {
                if let Some(a) = p.get_mut(&id) {
                    match &remove {
                        Some(x) => {
                            a.tags.remove(x);
                        }
                        None => {
                            a.tags.insert(t);
                        }
                    }
                }
            });
            if add_t {
                tag.clear();
            }
        }
        if let Some(r) = &mut self.review {
            r.comment = comment;
            r.tag = tag;
        }
    }

    /// The review card shown instead of the inspector.
    pub fn review_card(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(6.0);
        let Some(r) = &self.review else { return };
        if r.setup {
            self.setup_card(ui);
            return;
        }
        if r.session.finished() {
            let n = r.session.completed_pages.len();
            ui.heading("Review finished");
            ui.label(RichText::new(format!("{n} page(s) reviewed, {} step(s) answered. Esc leaves Review Mode.", r.session.reviewed)).color(MUTED));
            if ui.button("New review…").clicked() {
                if let Some(r) = &mut self.review {
                    r.setup = true;
                }
            }
            return;
        }
        let Some(item) = self.review_item() else { return };
        let pass = item.pass;
        ui.label(RichText::new(pass.name().to_uppercase()).size(11.0).strong().color(Color32::from_rgb(255, 200, 80)));
        ui.label(RichText::new(pass.hint()).color(MUTED).size(11.5));
        ui.add_space(6.0);
        match item.annotation_id.clone() {
            None => self.page_step_card(ui, pass, &ctx),
            Some(aid) => {
                let Some(a) = self.ann(&aid).cloned() else { return };
                self.item_header(ui, &a);
                match pass {
                    Pass::LineBoxes => self.line_box_card(ui, &a, &ctx),
                    Pass::Transcription => self.transcription_card(ui, &a, &ctx),
                    Pass::CharBoxes => self.char_card(ui, &a),
                    Pass::Legibility => self.quick_card(ui, &a, "recognizability"),
                    Pass::TextType => self.quick_card(ui, &a, "classes"),
                    Pass::Confidence => self.quick_card(ui, &a, "confidence"),
                    _ => self.rest_card(ui, &a),
                }
                ui.add_space(6.0);
                if self.review_item().is_some_and(|i| i.annotation_id.as_deref() == Some(aid.as_str())) {
                    self.notes_row(ui, &aid);
                }
            }
        }
        // the step may have changed while drawing the card
        if self.review_item().is_none_or(|i| i != item) {
            return;
        }
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            let label = match pass {
                Pass::Rest => "Approve & next  ⏎",
                Pass::MissingBoxes => "Nothing missing  ⏎",
                Pass::ReadingOrder => "Order is right  ⏎",
                Pass::CharBoxes => "Accept & next  ⏎",
                _ if item.annotation_id.is_none() => "Done  ⏎",
                _ => "Right & next  ⏎",
            };
            if ui.add(egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(Color32::from_rgb(46, 125, 80)).min_size(vec2(0.0, 30.0))).clicked() {
                self.review_confirm(&ctx);
            } else if ui.add(egui::Button::new("Skip  ↓").min_size(vec2(0.0, 30.0))).clicked() {
                self.review_step(1);
            } else if ui.add(egui::Button::new("Next pass  PgDn").min_size(vec2(0.0, 30.0))).clicked() {
                self.skip_pass();
            } else if item.annotation_id.is_some() && ui.add(egui::Button::new("Needs work  F").min_size(vec2(0.0, 30.0))).clicked() {
                self.review_flag(&ctx);
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new("↑/↓ previous/next · PgDn next pass · Space focus/context · P page · O other labels · H hide selection · F8 next problem · Ctrl+Shift+R read again · Esc leave").color(MUTED).size(10.5));
    }

    fn item_header(&mut self, ui: &mut Ui, a: &Annotation) {
        let cfg = self.cfg();
        ui.horizontal(|ui| {
            let col = super::canvas::ann_color(&cfg, a);
            let (rr, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(rr, 3.0, col);
            ui.label(RichText::new(a.level.name()).strong());
            if let Some(c) = a.classes.iter().next().and_then(|c| cfg.class(c)) {
                ui.label(RichText::new(&c.name).color(rgb(c.color)));
            }
            let st = a.review.state;
            ui.label(RichText::new(st.name()).color(rgb(st.color())));
            if a.review.needs_rereview {
                ui.label(RichText::new("changed after approval").color(BAD));
            }
        });
        let issues: Vec<String> = self.issues.iter().filter(|i| i.annotation_id.as_deref() == Some(a.id.as_str())).map(|i| i.message.clone()).collect();
        for m in issues.iter().take(3) {
            ui.label(RichText::new(format!("⚠ {m}")).color(WARN).size(11.0));
        }
    }

    fn line_box_card(&mut self, ui: &mut Ui, a: &Annotation, ctx: &egui::Context) {
        self.crop(ui, a, vec2(ui.available_width(), 160.0));
        let (lo, hi) = a.geometry.bounds();
        ui.label(RichText::new(format!("{} · {:.0} × {:.0} px · “{}”", a.geometry.kind(), hi[0] - lo[0], hi[1] - lo[1], a.text().replace('\n', " "))).color(MUTED).size(11.5));
        let tight = self.tighten_suggestion();
        let tilt = assist::straighten(&a.geometry, self.cfg().quick.straighten_max_deg).is_some();
        let ai = a.provenance.origin == Origin::Ai && a.review.state == ReviewState::AiProposed;
        let (mut t, mut s, mut rej, mut del) = (false, false, false, false);
        ui.horizontal_wrapped(|ui| {
            if tight.is_some() {
                t = ui.add(egui::Button::new(RichText::new("Tighten to the text (green)  Ctrl+T").color(Color32::WHITE)).fill(Color32::from_rgb(40, 110, 70))).clicked();
            }
            if tilt {
                s = ui.button("Remove the tilt").clicked();
            }
            if ai {
                rej = ui.button("Not text: reject  X").clicked();
            }
            del = ui.button("Delete").clicked();
        });
        if tight.is_none() {
            ui.label(RichText::new("Already tight around the text.").color(GOOD).size(11.0));
        }
        ui.label(RichText::new("Shift+arrows grow a side · Ctrl+arrows shrink it · Alt+arrows move · or drag the edges").color(MUTED).size(10.5));
        if t {
            self.tighten_selection(ctx);
        } else if s {
            self.straighten_selection(ctx, false);
        } else if rej {
            self.reject_ai("rejected in review");
            self.review_next(false);
        } else if del {
            self.delete_selection();
            let pos = self.review.as_ref().map(|r| r.session.pos).unwrap_or(0);
            self.review_goto(pos);
        }
    }

    fn transcription_card(&mut self, ui: &mut Ui, a: &Annotation, ctx: &egui::Context) {
        let aid = a.id.clone();
        let Some(page) = self.cur().cloned() else { return };
        let (mut tmode, mut overlay, mut opacity, focus_req, mut independent) = {
            let r = self.review.as_ref().unwrap();
            (r.tmode, r.overlay, r.opacity, r.focus_editor, r.independent.clone())
        };
        if let Some(ai) = &a.provenance.ai {
            ui.label(RichText::new(format!("{} {} · recognition {} (model score)", ai.model, ai.model_version, ai.recognition_score.map(|s| format!("{s:.2}")).unwrap_or("—".into()))).color(MUTED).size(11.0));
        }
        ui.horizontal(|ui| {
            for (m, name) in [(TMode::Side, "Side by side"), (TMode::Compare, "Compare"), (TMode::ImageOnly, "Image only")] {
                ui.selectable_value(&mut tmode, m, name);
            }
            ui.toggle_value(&mut overlay, "Overlay  T");
        });
        if overlay {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Overlay opacity").color(MUTED).size(11.5));
                ui.add(egui::Slider::new(&mut opacity, 0.1..=1.0).show_value(false));
            });
        }
        ui.add_space(4.0);
        let tall = {
            let (lo, hi) = a.geometry.bounds();
            hi[1] - lo[1] > (hi[0] - lo[0]) * 1.3
        };
        let mut confirm_text = false;
        let mut edited = a.clone();
        let text_block = |ui: &mut Ui, edited: &mut Annotation, independent: &mut String, confirm: &mut bool| match tmode {
            TMode::ImageOnly => {
                ui.label(RichText::new("Your own reading (the stored text stays hidden)").color(MUTED).size(11.5));
                let r = ui.add(TextEdit::singleline(independent).font(FontId::proportional(22.0)).desired_width(f32::INFINITY).hint_text("type what you read"));
                if focus_req {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !independent.trim().is_empty() {
                    *confirm = true;
                }
            }
            TMode::Compare => {
                let model = edited
                    .provenance
                    .ai
                    .as_ref()
                    .and_then(|ai| page.inference_runs.iter().find(|r| r.id == ai.run_id).and_then(|r| r.predictions.get(ai.prediction)))
                    .map(|p| p.text.clone())
                    .or_else(|| edited.transcription.as_ref().and_then(|t| t.alternatives.iter().find(|x| x.by.starts_with("ai:")).map(|x| x.text.clone())));
                match model {
                    Some(m) => {
                        let (dm, dc) = diff(&m, edited.text());
                        ui.label(RichText::new("Model read").color(MUTED).size(11.0));
                        ui.label(diff_job(&dm, BAD));
                        ui.label(RichText::new("Current").color(MUTED).size(11.0));
                        ui.label(diff_job(&dc, GOOD));
                        ui.label(RichText::new(format!("Character error rate of the model: {:.0}%", ai::cer(edited.text(), &m) * 100.0)).color(MUTED).size(11.0));
                    }
                    None => {
                        ui.label(RichText::new("No model reading to compare with.").color(MUTED));
                    }
                }
            }
            _ => {
                let t = edited.transcription.get_or_insert_with(Default::default);
                let multi = t.text.contains('\n');
                let te = if multi { TextEdit::multiline(&mut t.text) } else { TextEdit::singleline(&mut t.text) };
                let r = ui.add(te.font(FontId::proportional(24.0)).desired_width(f32::INFINITY).hint_text("Exact visible text"));
                if focus_req {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    *confirm = true;
                }
                ui.label(RichText::new(format!("{} characters · E edits · Enter accepts", t.text.graphemes(true).count())).color(MUTED).size(10.5));
            }
        };
        if tall {
            ui.horizontal_top(|ui| {
                self.crop(ui, a, vec2(110.0, 300.0));
                ui.vertical(|ui| text_block(ui, &mut edited, &mut independent, &mut confirm_text));
            });
        } else {
            self.crop(ui, a, vec2(ui.available_width(), 130.0));
            text_block(ui, &mut edited, &mut independent, &mut confirm_text);
        }
        if let Some(r) = &mut self.review {
            r.tmode = tmode;
            r.overlay = overlay;
            r.opacity = opacity;
            r.focus_editor = false;
            r.independent = independent.clone();
        }
        if edited != *a {
            if let Some(x) = self.cur_mut().and_then(|p| p.get_mut(&aid)) {
                *x = edited;
            }
        }
        let (mut again, mut unreadable) = (false, false);
        ui.horizontal_wrapped(|ui| {
            again = ui.button("Read it again (AI)  Ctrl+Shift+R").clicked();
            unreadable = ui.button("Unreadable").clicked();
        });
        if again {
            self.reread(std::slice::from_ref(&aid));
        }
        if unreadable {
            self.answer("recognizability", Answer::Recog(Recognizability::Unreadable));
            self.review_next(true);
            return;
        }
        if !confirm_text {
            return;
        }
        if tmode != TMode::ImageOnly {
            self.commit();
            self.answer("transcription", Answer::TranscriptionConfirm);
            self.review_next(true);
            return;
        }
        let same = independent.trim() == a.text().trim();
        let user = self.user();
        let reading = independent.trim().to_string();
        self.op(move |p, _, _, now| {
            if let Some(x) = p.get_mut(&aid) {
                if same {
                    assist::set_check(x, "transcription", CheckState::Confirmed, &user, now, "independent reading matched", false);
                } else {
                    let t = x.transcription.get_or_insert_with(Default::default);
                    t.alternatives.push(Alternative { text: reading.clone(), by: user.clone(), at: now.into(), note: "independent reading".into() });
                    let stored = t.text.clone();
                    review::add_note(x, NoteKind::Disagreement, Some(Dimension::Transcription), &format!("Independent reading “{reading}” differs from “{stored}”"), &user, now);
                }
            }
        });
        if let Some(r) = &mut self.review {
            r.independent.clear();
        }
        if same {
            self.flash("Your reading matches the transcription", ctx);
            self.review_next(true);
        } else {
            self.error("Your reading differs: saved as an alternative with a disagreement to settle");
        }
    }

    fn char_card(&mut self, ui: &mut Ui, a: &Annotation) {
        let Some(page) = self.cur().cloned() else { return };
        let cfg = self.cfg();
        self.crop(ui, a, vec2(ui.available_width(), 120.0));
        let status = assist::char_status(&page, a, &cfg);
        let msg = match status {
            CharStatus::None(n) => format!("No character boxes yet: {n} previewed in pink. Drag the pink ticks to size the cells, then C or Enter proposes them."),
            CharStatus::Proposed(n) => format!("{n} proposed box(es), dashed. Click one to resize it (it stays a proposal), then Enter accepts them all."),
            CharStatus::Stale(n) => format!("{n} box(es) no longer match the text: Enter proposes them again (boxes you adjusted are kept)."),
            CharStatus::Done(n) => format!("{n} box(es) accepted."),
        };
        ui.label(RichText::new(msg).size(11.5));
        let mono = a.attributes.contains("monospace");
        let (mut toggle, mut again, mut skip, mut unpin) = (false, false, false, false);
        ui.horizontal_wrapped(|ui| {
            toggle = ui.selectable_label(mono, "Monospaced line").on_hover_text("Every character takes the same room; the proposal is spaced accordingly").clicked();
            again = ui.button("Propose again").clicked();
            skip = ui.button("Not needed").clicked();
            unpin = ui.button("Unpin sizes").on_hover_text("Let resized boxes follow the line again").clicked();
        });
        let id = a.id.clone();
        if toggle {
            self.op(|p, _, _, _| {
                if let Some(x) = p.get_mut(&id) {
                    if mono {
                        x.attributes.remove("monospace");
                    } else {
                        x.attributes.insert("monospace".into());
                    }
                }
            });
            if matches!(status, CharStatus::Proposed(_)) {
                self.answer("char_boxes", Answer::CharsRegenerate);
            }
        }
        if again {
            self.answer("char_boxes", Answer::CharsRegenerate);
        }
        if unpin {
            self.op(|p, _, _, _| assist::unpin_chars(p, &id));
        }
        if skip {
            self.answer("char_boxes", Answer::CharsSkip);
            self.review_next(true);
            return;
        }
        let selected_char = self.selection.first().and_then(|id| self.ann(id)).filter(|c| c.level == Level::Char).cloned();
        if let Some(c) = selected_char {
            ui.label(RichText::new(format!("Selected box “{}”: drag its edges or use Shift/Ctrl+arrows; Alt+drag shares the room with its neighbour.", c.text())).color(MUTED).size(11.0));
        }
    }

    /// Legibility, text type and confidence: one question, number keys answer and move on.
    fn quick_card(&mut self, ui: &mut Ui, a: &Annotation, field: &'static str) {
        let Some(page) = self.cur().cloned() else { return };
        let cfg = self.cfg();
        self.crop(ui, a, vec2(ui.available_width(), 170.0));
        if a.level == Level::Region {
            let n = assist::descendants(&page, &a.id).iter().filter(|d| page.get(d).is_some_and(|x| x.level == Level::Line)).count();
            ui.label(RichText::new(format!("The answer also goes to its {n} line(s) without an answer of their own.")).color(MUTED).size(11.0));
        } else {
            ui.label(RichText::new(format!("“{}”", a.text().replace('\n', " "))).size(15.0));
        }
        let current = match field {
            "recognizability" => a.recognizability.map(Answer::Recog),
            "classes" => a.classes.iter().next().cloned().map(Answer::Class),
            _ => a.confidence.transcription.map(Answer::Confidence),
        };
        let need = assist::needs(a, &page, &cfg).into_iter().find(|n| n.field == field);
        if let Some((s, why)) = need.as_ref().and_then(|n| n.suggestion.clone()) {
            ui.label(RichText::new(format!("Suggested: {} ({why}) — Enter accepts", suggestion_label(&s, &cfg))).color(Color32::from_rgb(150, 170, 255)).size(11.5));
        }
        let opts = options(field, a, &page, &cfg);
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            for (k, (label, ans)) in opts.iter().enumerate() {
                let on = current.as_ref() == Some(ans);
                let big = field == "classes" || k < 2;
                let txt = RichText::new(format!("{}  {label}", k + 1)).size(if big { 14.0 } else { 11.5 });
                if ui.add(egui::Button::selectable(on, txt).min_size(vec2(0.0, if big { 30.0 } else { 22.0 }))).clicked() {
                    chosen = Some(ans.clone());
                }
            }
        });
        if field != "classes" {
            ui.label(RichText::new("1 or 2 is enough now; the finer answers refine it, now or later in “Everything else”.").color(MUTED).size(10.5));
        }
        if let Some(ans) = chosen {
            self.answer(field, ans);
            self.review_next(true);
        }
    }

    fn page_step_card(&mut self, ui: &mut Ui, pass: Pass, ctx: &egui::Context) {
        let Some(page) = self.cur().cloned() else { return };
        let count = |l: Level| page.annotations.iter().filter(|a| a.level == l).count();
        match pass {
            Pass::MissingBoxes => {
                ui.label(RichText::new(format!("{} line box(es) on this page. Draw any line still missing.", count(Level::Line))).size(13.0));
                self.tool_row(ui);
                let (mut later, mut none) = (false, false);
                ui.horizontal_wrapped(|ui| {
                    if self.review.as_ref().is_some_and(|r| !r.session.workflow.missing_at_end) {
                        later = ui.button("Later: at the end of this page").on_hover_text("The new boxes then go through the passes they missed").clicked();
                    }
                    none = ui.button("This page has no text").clicked();
                });
                if later {
                    self.settings.workflow.missing_at_end = true;
                    self.settings.save();
                    if let Some(r) = &mut self.review {
                        r.session.workflow.missing_at_end = true;
                        let pos = r.session.pos;
                        let it = r.session.items.remove(pos);
                        let at = r.session.items.iter().position(|i| i.pass == Pass::ReadingOrder).unwrap_or(r.session.items.len());
                        r.session.items.insert(at, it);
                    }
                    let pos = self.review.as_ref().map(|r| r.session.pos).unwrap_or(0);
                    self.review_goto(pos);
                } else if none {
                    self.mark_inspected(true, true);
                    self.review_next(true);
                }
            }
            Pass::Reread => {
                let n = count(Level::Line);
                ui.label(RichText::new(format!("Read all {n} line box(es) again with the AI model? New readings fill empty lines and are added as alternatives elsewhere.")).size(13.0));
                if !self.cfg().model.enabled {
                    ui.label(RichText::new("AI is off for this project (Project settings → Model).").color(WARN));
                }
                if ui.add(egui::Button::new(RichText::new("Read all lines again").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    self.reread_page_lines(ctx);
                    self.review_next(true);
                }
            }
            Pass::Regions => {
                let sugg = self.region_suggestions();
                ui.label(RichText::new(format!("{} text region(s). {} group(s) of nearby lines suggested.", count(Level::Region), sugg.len())).size(13.0));
                let sel = self.selection.len();
                let (mut all, mut group) = (false, false);
                ui.horizontal_wrapped(|ui| {
                    if !sugg.is_empty() {
                        all = ui.button(format!("Create the {} suggested region(s)", sugg.len())).clicked();
                    }
                    group = ui.add_enabled(sel > 0, egui::Button::new(format!("Group the {sel} selected  Ctrl+G"))).clicked();
                });
                if all {
                    self.selection.clear();
                    self.make_regions(ctx);
                } else if group {
                    self.make_regions(ctx);
                }
                ui.label(RichText::new("Click lines with Ctrl to select several, or drag a frame around them.").color(MUTED).size(11.0));
            }
            Pass::Containers | Pass::Panels => {
                let l = pass.draw_level().unwrap_or(Level::Container);
                ui.label(RichText::new(format!("{} {}(s) on this page. Draw missing ones; select labels and press Ctrl+G to group them into one.", count(l), l.name().to_lowercase())).size(13.0));
                self.tool_row(ui);
            }
            Pass::ReadingOrder => {
                let cfg = self.cfg();
                let ordered = page.annotations.iter().filter(|a| a.level != Level::Char && !assist::is_unordered(a, &cfg) && a.reading_order.is_some()).count();
                let free = page.annotations.iter().filter(|a| a.level != Level::Char && assist::is_unordered(a, &cfg)).count();
                ui.label(RichText::new(format!("{ordered} label(s) numbered inside their parent; {free} left out of the order (background text, sound effects, page numbers, furigana).")).size(13.0));
                ui.label(RichText::new("Select a label and press Alt+↑/↓ to move it earlier or later.").color(MUTED).size(11.0));
                if ui.button("Suggest again from the layout").clicked() {
                    self.prepare_order();
                }
            }
            _ => {}
        }
    }

    fn tool_row(&mut self, ui: &mut Ui) {
        let mut pick = None;
        ui.horizontal(|ui| {
            for (t, name) in [(Tool::Select, "Select  V"), (Tool::Box, "Box  B"), (Tool::Quad, "Quad  Q"), (Tool::Polygon, "Polygon  L")] {
                if ui.selectable_label(self.tool == t, name).clicked() {
                    pick = Some(t);
                }
            }
        });
        if let Some(t) = pick {
            self.set_tool(t);
        }
    }

    /// Everything else: the checklist of open fields, then approve.
    fn rest_card(&mut self, ui: &mut Ui, a: &Annotation) {
        let cfg = self.cfg();
        let aid = a.id.clone();
        let Some(page) = self.cur().cloned() else { return };
        self.crop(ui, a, vec2(ui.available_width(), 110.0));
        if !a.text().is_empty() {
            ui.label(RichText::new(format!("“{}”", a.text().replace('\n', " "))).size(15.0));
        }
        let needs = assist::needs(a, &page, &cfg);
        let blocking: Vec<Need> = assist::blocking(&needs, &cfg.quick).into_iter().cloned().collect();
        let all = self.review.as_ref().is_some_and(|r| r.all_fields);
        let mut flip = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(if needs.is_empty() { "NOTHING LEFT TO CHECK" } else { "STILL OPEN" }).size(11.0).strong().color(if needs.is_empty() { GOOD } else { WARN }));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                flip = ui.small_button(if all { "Fewer fields" } else { "All fields  M" }).clicked();
            });
        });
        if flip {
            if let Some(r) = &mut self.review {
                r.all_fields = !r.all_fields;
            }
        }
        let first_open = blocking.first().map(|n| n.field).or(needs.first().map(|n| n.field));
        let fields: Vec<&'static str> = if all {
            QUICK_FIELDS.iter().map(|f| f.0).filter(|f| assist::applicable(f, a, &page, &cfg)).collect()
        } else {
            needs.iter().map(|n| n.field).filter(|f| *f != "custom").collect()
        };
        let mut picked: Option<(&'static str, Answer)> = None;
        let mut unknown: Option<&'static str> = None;
        let mut adjust = false;
        for f in fields {
            let need = needs.iter().find(|n| n.field == f);
            let focused = Some(f) == first_open;
            egui::Frame::new()
                .fill(if focused { Color32::from_rgb(52, 58, 92) } else { PANEL_BG2 })
                .stroke(if focused { Stroke::new(1.0, ACCENT) } else { Stroke::NONE })
                .corner_radius(6.0)
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (icon, col) = match need.map(|n| n.kind) {
                            Some(NeedKind::Missing) => ("○", WARN),
                            Some(NeedKind::Invalid) | Some(NeedKind::Stale) => ("!", BAD),
                            Some(NeedKind::Unverified) => ("?", Color32::from_rgb(186, 104, 200)),
                            None => ("✓", GOOD),
                        };
                        ui.label(RichText::new(icon).strong().color(col));
                        ui.label(RichText::new(assist::field_label(f)).strong());
                        if let Some(n) = need {
                            ui.label(RichText::new(&n.message).color(MUTED).size(11.5));
                        }
                    });
                    if let Some((s, why)) = need.and_then(|n| n.suggestion.clone()) {
                        let b = egui::Button::new(RichText::new(format!("Use: {}  ({why})  Y", suggestion_label(&s, &cfg))).size(11.5)).fill(Color32::from_rgb(60, 70, 110)).corner_radius(9.0);
                        if ui.add(b).on_hover_text("Suggested; nothing is filled in until you accept it").clicked() {
                            picked = Some((f, Answer::Suggestion(s)));
                        }
                    }
                    let opts = options(f, a, &page, &cfg);
                    if !opts.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for (k, (label, ans)) in opts.iter().enumerate() {
                                let txt = if focused && k < 9 { format!("{} {label}", k + 1) } else { label.clone() };
                                if ui.small_button(txt).clicked() {
                                    picked = Some((f, ans.clone()));
                                }
                            }
                            if matches!(f, "classes" | "language" | "direction" | "recognizability") && ui.small_button("Unknown").clicked() {
                                unknown = Some(f);
                            }
                            if f == "perspective" && ui.small_button("Adjust…").clicked() {
                                adjust = true;
                            }
                        });
                    }
                });
        }
        if let Some((f, ans)) = picked {
            self.answer(f, ans);
        }
        if let Some(f) = unknown {
            self.op(move |p, _, user, now| {
                if let Some(x) = p.get_mut(&aid) {
                    assist::set_check(x, f, CheckState::Unknown, user, now, "", false);
                }
            });
        }
        if adjust {
            self.tab = Tab::Flatten;
            if let Some(r) = &mut self.review {
                r.geometry_edit = true;
                r.all_fields = false;
            }
        }
        if needs.iter().any(|n| n.field == "custom") {
            ui.label(RichText::new("Project fields are required: open All fields (M).").color(WARN).size(11.5));
        }
        if all {
            ui.separator();
            self.tab_text_public(ui);
        }
        if let Err(e) = review::can_review(a, &cfg.review, &self.settings.user) {
            ui.label(RichText::new(e).color(WARN).size(11.0));
        }
    }
}
