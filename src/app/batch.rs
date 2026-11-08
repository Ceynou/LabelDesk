//! Batch verification, the assistant strip shown above an annotation's fields, and the progress overview.

use super::*;
use crate::assist::{self, CharStatus};
use eframe::egui::{RichText, TextEdit};

impl App {
    /// Select everything nested inside the selected annotation (a whole panel or balloon).
    pub fn select_contents(&mut self) {
        let Some(root) = self.selection.first().cloned() else { return };
        let Some(p) = self.cur() else { return };
        let mut out = vec![root.clone()];
        let mut i = 0;
        while i < out.len() {
            let id = out[i].clone();
            out.extend(p.annotations.iter().filter(|a| a.parent.as_deref() == Some(id.as_str())).map(|a| a.id.clone()));
            i += 1;
        }
        self.selection = out;
    }

    /// Apply one decision to all selected annotations, recorded as a batch check (undoable, in history).
    fn batch<F: Fn(&mut Annotation, &str, &str)>(&mut self, field: &'static str, f: F) -> usize {
        let ids = self.selection.clone();
        self.commit();
        let n = self
            .op(|p, _, user, now| {
                let mut n = 0;
                for id in &ids {
                    if let Some(a) = p.get_mut(id) {
                        f(a, user, now);
                        if !field.is_empty() {
                            assist::set_check(a, field, CheckState::Confirmed, user, now, "batch decision", true);
                        }
                        n += 1;
                    }
                }
                n
            })
            .unwrap_or(0);
        self.rebaseline();
        n
    }

    pub(super) fn batch_panel(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let cfg = self.cfg();
        ui.label(RichText::new("BATCH DECISIONS").size(11.0).strong().color(MUTED));
        if !cfg.quick.allow_batch_metadata {
            ui.label(RichText::new("This project's policy asks for one-by-one decisions (Project settings → Quick review).").color(WARN).size(11.5));
            return;
        }
        ui.label(RichText::new("Each decision is recorded per annotation as a batch check. It never approves transcriptions.").color(MUTED).size(11.0));
        let text_sel: Vec<String> = self.selection.iter().filter(|id| self.ann(id).is_some_and(|a| a.level.is_text())).cloned().collect();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Writing").color(MUTED));
            for (name, wm, d) in [("Vertical ←", WritingMode::VerticalRl, Direction::Ttb), ("Horizontal →", WritingMode::HorizontalTb, Direction::Ltr), ("Horizontal ←", WritingMode::HorizontalTb, Direction::Rtl)] {
                if ui.small_button(name).clicked() {
                    let n = self.batch("direction", |a, _, _| {
                        if a.level.is_text() {
                            let t = a.transcription.get_or_insert_with(Default::default);
                            t.writing_mode = wm;
                            t.direction = d;
                        }
                    });
                    self.flash(format!("Writing direction set for {n} annotation(s)"), &ctx);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Language").color(MUTED));
            for l in cfg.languages.iter().take(5) {
                if ui.small_button(l).on_hover_text("for the selection").clicked() {
                    let (l2, s) = (l.clone(), crate::assist::detect_script(&format!("{}", l)).1.map(String::from));
                    let script = match l.as_str() {
                        "ja" => "Jpan",
                        "ko" => "Kore",
                        "zh" => "Hans",
                        _ => "Latn",
                    }
                    .to_string();
                    let _ = s;
                    let n = self.batch("language", move |a, _, _| {
                        if let Some(t) = &mut a.transcription {
                            t.language = Some(l2.clone());
                            t.script = Some(script.clone());
                        }
                    });
                    self.flash(format!("Language {l} set for {n} annotation(s)"), &ctx);
                }
            }
            if ui.small_button("…for the whole page").on_hover_text("Selects every text annotation on the page first").clicked() {
                self.selection = self.cur().map(|p| p.annotations.iter().filter(|a| a.level.is_text()).map(|a| a.id.clone()).collect()).unwrap_or_default();
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Confirm: not obstructed").clicked() {
                let n = self.batch("occlusion", |a, _, _| a.occlusion = Some(Occlusion { occluded: false, ..Default::default() }));
                self.flash(format!("{n} annotation(s) confirmed unobstructed"), &ctx);
            }
            if ui.small_button("Confirm: not mirrored").clicked() {
                let n = self.batch("mirrored", |a, _, _| a.orientation.mirrored = false);
                self.flash(format!("{n} annotation(s) confirmed not mirrored"), &ctx);
            }
            if ui.small_button("Confirm: no image problems").clicked() {
                let n = self.batch("quality", |_, _, _| {});
                self.flash(format!("{n} annotation(s): image quality confirmed"), &ctx);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Text type").color(MUTED));
            for c in cfg.text_classes.iter().take(6) {
                if ui.small_button(&c.name).clicked() {
                    let id = c.id.clone();
                    let n = self.batch("classes", move |a, _, _| a.classes = [id.clone()].into_iter().collect());
                    self.flash(format!("{n} annotation(s) classified as {}", c.name), &ctx);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Accept suggested reading order").on_hover_text("Turns suggested numbers of the selection into confirmed ones").clicked() {
                let n = self.batch("reading_order", |a, _, _| {
                    if a.reading_order.is_some() {
                        a.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
                    }
                });
                self.flash(format!("Reading order confirmed for {n} annotation(s)"), &ctx);
            }
            if ui.small_button(format!("Propose character boxes ({})", text_sel.len())).clicked() {
                let ids = text_sel.clone();
                let n = self.op(|p, cfg, user, now| ids.iter().map(|id| assist::regenerate_chars(p, id, cfg, user, now).0).sum::<usize>()).unwrap_or(0);
                self.flash(format!("{n} character box(es) proposed"), &ctx);
            }
            if ui.small_button("Accept proposed character boxes").clicked() {
                let ids = text_sel.clone();
                let n = self
                    .op(|p, _, user, now| {
                        let mut n = 0;
                        for id in &ids {
                            n += assist::accept_chars(p, id, user, now);
                            if let Some(a) = p.get_mut(id) {
                                assist::set_check(a, "char_boxes", CheckState::Confirmed, user, now, "batch decision", true);
                            }
                        }
                        n
                    })
                    .unwrap_or(0);
                self.flash(format!("{n} character box(es) accepted as drafts"), &ctx);
            }
        });
    }

    /// Character boxes, reading order and open checklist items for one annotation.
    pub(super) fn assist_strip(&mut self, ui: &mut Ui, a: &Annotation) {
        let ctx = ui.ctx().clone();
        let cfg = self.cfg();
        let Some(page) = self.cur().cloned() else { return };
        let needs = assist::needs(a, &page, &cfg);
        let open = needs.iter().filter(|n| n.kind != assist::NeedKind::Unverified || n.field == "char_boxes").count();
        let tight = self.tighten_suggestion().is_some();
        let as_box = assist::quad_to_box(&a.geometry).is_some();
        let group = self.region_suggestions().into_iter().find(|s| s.lines.contains(&a.id)).map(|s| s.lines.len());
        egui::Frame::new().fill(PANEL_BG2).corner_radius(6.0).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let tilt = match a.geometry {
                Geometry::Box { rotation_deg, .. } if assist::straighten(&a.geometry, cfg.quick.straighten_max_deg).is_some() => Some(rotation_deg),
                _ => None,
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Convert to").color(MUTED));
                for (kind, name) in [("box", "Box"), ("quad", "Quadrilateral"), ("polygon", "Polygon")] {
                    if a.geometry.kind() != kind && ui.small_button(name).clicked() {
                        self.convert_selection(&ctx, kind);
                    }
                }
                if let Some(t) = tilt {
                    if ui.button(format!("Remove the {t:.1}° tilt")).on_hover_text("Nearly level: make it an upright box (same centre and size)").clicked() {
                        self.straighten_selection(&ctx, false);
                    }
                }
            });
            if tight || as_box || group.is_some() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Shape").color(MUTED));
                    if tight && ui.button(format!("Tighten to the text  {}", self.settings.shortcut("tighten"))).on_hover_text("Shown as a green dashed outline on the page").clicked() {
                        self.tighten_selection(&ctx);
                    }
                    if as_box && ui.button("Convert to a box").on_hover_text("This quadrilateral is a rectangle; a (rotated) box is simpler to edit").clicked() {
                        self.convert_quads(&ctx, false);
                    }
                    if let Some(n) = group {
                        if ui.button(format!("Group {n} nearby lines into a text region  {}", self.settings.shortcut("make_regions"))).clicked() {
                            self.make_regions(&ctx);
                        }
                    }
                });
            }
            if matches!(a.level, Level::Line | Level::Word | Level::Region) && !a.text().is_empty() && cfg.requirement(Level::Char, &a.classes) != Requirement::Disabled {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Characters").color(MUTED));
                    match assist::char_status(&page, a, &cfg) {
                        CharStatus::None(n) => {
                            if self.preview_splits(&a.id).is_some() && ui.button("Even sizes").on_hover_text("Forget the cell borders you dragged").clicked() {
                                self.cell_splits = None;
                            }
                            if ui.button(format!("Propose {n} boxes")).on_hover_text("Split from the transcription along the line (perspective-aware). Drag the pink borders on the page to size each cell first.").clicked() {
                                let id = a.id.clone();
                                let splits = self.preview_splits(&id);
                                self.op(|p, cfg, user, now| assist::regenerate_chars_split(p, &id, cfg, user, now, splits.as_ref()));
                                self.cell_splits = None;
                                self.flash(format!("{n} character boxes proposed: check them, then accept (C in Review Mode)"), &ctx);
                            }
                        }
                        CharStatus::Proposed(n) => {
                            ui.label(RichText::new(format!("{n} proposed")).color(rgb(ReviewState::AiProposed.color())));
                            if ui.button("Accept").clicked() {
                                let id = a.id.clone();
                                self.op(|p, cfg, user, now| assist::apply_suggestion(p, &id, &assist::Suggest::AcceptChars(n), cfg, user, now));
                            }
                            if ui.button("Regenerate").clicked() {
                                let id = a.id.clone();
                                self.op(|p, cfg, user, now| assist::regenerate_chars(p, &id, cfg, user, now));
                            }
                            if ui.button("Reject").clicked() {
                                let id = a.id.clone();
                                self.op(|p, _, _, _| p.annotations.retain(|c| !(c.parent.as_deref() == Some(id.as_str()) && assist::untouched_proposal(c))));
                            }
                        }
                        CharStatus::Stale(n) => {
                            ui.label(RichText::new(format!("{n} out of date")).color(BAD));
                            if ui.button("Regenerate (keeps your fixes)").clicked() {
                                let id = a.id.clone();
                                self.op(|p, cfg, user, now| assist::regenerate_chars(p, &id, cfg, user, now));
                            }
                        }
                        CharStatus::Done(n) => {
                            ui.label(RichText::new(format!("{n} boxes")).color(GOOD));
                        }
                    }
                    let pinned = assist::chars_of(&page, &a.id).iter().filter(|c| c.provenance.heuristic.as_ref().is_some_and(|h| h.pinned)).count();
                    if pinned > 0 && ui.small_button(format!("Unpin {pinned}")).on_hover_text("Let these boxes be resized automatically again").clicked() {
                        let id = a.id.clone();
                        self.op(|p, _, _, _| assist::unpin_chars(p, &id));
                    }
                    ui.label(RichText::new("Alt+drag a character's edge: the others share the rest").color(MUTED).size(10.5));
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Reading order").color(MUTED));
                let txt = match (a.reading_order, assist::manual_order(a)) {
                    (None, _) => "none".to_string(),
                    (Some(n), true) => format!("{n} (set by a person)"),
                    (Some(n), false) => format!("{n} (suggested)"),
                };
                ui.label(txt);
                let mark = a.reading.as_ref().and_then(|r| r.mark);
                for (m, name) in [(Some(OrderMark::Ambiguous), "Ambiguous"), (Some(OrderMark::Unordered), "Unordered")] {
                    if ui.selectable_label(mark == m, name).on_hover_text("Mark when the order is unclear or deliberately absent").clicked() {
                        let id = a.id.clone();
                        let new = if mark == m { None } else { m };
                        self.commit();
                        if let Some(x) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
                            x.reading.get_or_insert_with(Default::default).mark = new;
                        }
                        self.commit();
                    }
                }
            });
            if open > 0 {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{open} required field(s) open: {}", needs.iter().map(|n| assist::field_label(n.field)).collect::<Vec<_>>().join(", "))).color(WARN).size(11.5));
                });
            }
        });
    }

    pub(super) fn progress_dialog(&mut self, ctx: &egui::Context) {
        if !self.dialogs.progress {
            return;
        }
        let m = self.metrics.clone();
        let pm = self.cur_id().and_then(|id| self.page_checks.get(&id)).map(|c| c.1.clone()).unwrap_or_default();
        let mut open = true;
        egui::Window::new("Progress overview").open(&mut open).default_width(470.0).show(ctx, |ui| {
            ui.label(RichText::new("Coverage, completeness and verification are counted separately: a fully filled-in dataset is not a verified one.").color(MUTED).size(11.5));
            egui::Grid::new("metrics").num_columns(3).striped(true).spacing([18.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("").strong());
                ui.label(RichText::new("Project").strong());
                ui.label(RichText::new("This page").strong());
                ui.end_row();
                let rows: [(&str, usize, usize); 12] = [
                    ("Pages with text annotated (coverage)", m.pages_covered, pm.pages_covered),
                    ("Pages awaiting completeness inspection", m.pages_awaiting_inspection, pm.pages_awaiting_inspection),
                    ("Annotations", m.total, pm.total),
                    ("Awaiting review", m.awaiting_review, pm.awaiting_review),
                    ("Required fields incomplete", m.incomplete_required, pm.incomplete_required),
                    ("Required fields complete", m.complete_required, pm.complete_required),
                    ("Transcription verified", m.verified_transcription, pm.verified_transcription),
                    ("Geometry verified", m.verified_geometry, pm.verified_geometry),
                    ("Reading order verified", m.verified_order, pm.verified_order),
                    ("Fully approved", m.fully_approved, pm.fully_approved),
                    ("Provisional character boxes", m.provisional_chars, pm.provisional_chars),
                    ("Gold", m.gold, pm.gold),
                ];
                for (k, a, b) in rows {
                    ui.label(k);
                    ui.label(RichText::new(a.to_string()).monospace());
                    ui.label(RichText::new(b.to_string()).monospace());
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            if ui.button("Review what is incomplete").clicked() {
                // find what is incomplete everywhere, then review just that
                self.find.conds = vec![super::find::Cond { crit: super::find::Crit::Needs(assist::QueueKind::MissingFields), not: false }];
                self.find.scope = super::find::FindScope::AllPages;
                self.run_find();
                self.enter_review(ctx);
                if let Some(r) = &mut self.review {
                    r.setup = true;
                    r.scope = super::review_mode::Scope::Found;
                }
            }
        });
        self.dialogs.progress = open;
    }

    pub(super) fn reading_settings(ui: &mut Ui, cfg: &mut ProjectConfig) {
        ui.heading("Reading conventions");
        ui.horizontal(|ui| {
            ui.label("Panels and balloons read");
            ui.selectable_value(&mut cfg.reading.page_direction, Direction::Rtl, "right to left (manga)");
            ui.selectable_value(&mut cfg.reading.page_direction, Direction::Ltr, "left to right");
        });
        ui.horizontal(|ui| {
            ui.label("Layout strategy");
            ui.selectable_value(&mut cfg.reading.strategy, OrderStrategy::Tiers, "rows (tiers)");
            ui.selectable_value(&mut cfg.reading.strategy, OrderStrategy::Columns, "columns (4-koma)");
            ui.selectable_value(&mut cfg.reading.strategy, OrderStrategy::Position, "distance from start");
        });
        ui.checkbox(&mut cfg.reading.auto_suggest, "Suggest reading order as soon as annotations are added (otherwise in the last review pass, once the page is complete)");
        ui.horizontal_wrapped(|ui| {
            ui.label("Read outside the reading order:");
            let classes: Vec<(String, String)> = cfg.text_classes.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
            for (id, name) in classes {
                let mut on = cfg.reading.unordered_classes.contains(&id);
                if ui.checkbox(&mut on, name).changed() {
                    if on {
                        cfg.reading.unordered_classes.push(id);
                    } else {
                        cfg.reading.unordered_classes.retain(|x| *x != id);
                    }
                }
            }
            ui.label(RichText::new("(furigana always)").color(MUTED).size(11.0));
        });
        ui.label(RichText::new("Order is suggested inside each parent (page, panel, balloon, region, line) among labels of the same level; unordered types are left out.").color(MUTED).size(11.0));
        ui.label(RichText::new("Text writing mode and direction defaults are under Text classes and defaults. Pages override the direction in Page → Source (reading direction); regions can override it too.").color(MUTED).size(11.0));
        ui.separator();
        ui.heading("Quick review");
        ui.label("Fields every applicable annotation must have answered:");
        ui.horizontal_wrapped(|ui| {
            for (f, name) in QUICK_FIELDS {
                let mut on = cfg.quick.required_fields.iter().any(|x| x == f);
                if ui.checkbox(&mut on, name).changed() {
                    if on {
                        cfg.quick.required_fields.push(f.to_string());
                    } else {
                        cfg.quick.required_fields.retain(|x| x != f);
                    }
                }
            }
        });
        ui.checkbox(&mut cfg.quick.confirm_inferred_individually, "Inferred values must be confirmed one by one (otherwise approving accepts them together)");
        ui.checkbox(&mut cfg.quick.allow_batch_metadata, "Allow batch metadata decisions");
        ui.horizontal(|ui| {
            ui.label("Zoom margin around the reviewed text");
            ui.add(egui::Slider::new(&mut cfg.quick.zoom_margin, 0.05..=1.5));
        });
        ui.horizontal(|ui| {
            ui.label("Low model confidence below");
            ui.add(egui::DragValue::new(&mut cfg.quick.low_confidence).range(0.0..=1.0).speed(0.01));
        });
        ui.horizontal(|ui| {
            ui.label("Suggest removing tilt up to");
            ui.add(egui::DragValue::new(&mut cfg.quick.straighten_max_deg).range(0.5..=45.0).speed(0.25).suffix("°"));
            ui.label(RichText::new("(larger tilts are kept as intended rotation)").color(MUTED).size(11.0));
        });
        ui.label("Character boxes:");
        ui.checkbox(&mut cfg.quick.chars.skip_spaces, "Spaces get no box");
        ui.checkbox(&mut cfg.quick.chars.skip_punctuation, "Punctuation gets no box");
        ui.checkbox(&mut cfg.quick.chars.proportional_latin, "Estimate widths of Latin letters (otherwise equal widths)");
        let _ = TextEdit::singleline;
    }
}

impl App {
    /// Tightening suggestion for the selected annotation (cached until its shape changes).
    pub fn tighten_suggestion(&mut self) -> Option<Geometry> {
        if self.selection.len() != 1 {
            return None;
        }
        let a = self.primary()?.clone();
        if let Some((id, g, s)) = &self.tighten_cache {
            if *id == a.id && *g == a.geometry {
                return s.clone();
            }
        }
        let img = self.page_image(&a.page_id)?;
        let s = crate::assist::tighten(&img, &a, 2.0, 2.0);
        self.tighten_cache = Some((a.id.clone(), a.geometry.clone(), s.clone()));
        s
    }

    /// Shrink the selected shapes to the ink they contain (one undo step).
    pub fn tighten_selection(&mut self, ctx: &egui::Context) {
        let ids = self.selection.clone();
        let Some(pid) = self.cur_id() else { return };
        let Some(img) = self.page_image(&pid) else { return };
        self.commit();
        let mut n = 0;
        if let Some(p) = self.cur_mut() {
            for id in &ids {
                let Some(a) = p.get(id).cloned() else { continue };
                if let Some(g) = crate::assist::tighten(&img, &a, 2.0, 2.0) {
                    if let Some(x) = p.get_mut(id) {
                        x.geometry = g;
                        n += 1;
                    }
                }
            }
        }
        self.refit_live(&ids);
        self.commit();
        self.flash(if n == 0 { "Already tight: nothing to trim".to_string() } else { format!("Tightened {n} shape(s) to their text (Ctrl+Z undoes)") }, ctx);
    }

    /// C: propose character boxes for the selected line, or accept the pending proposals.
    pub fn propose_or_accept_chars(&mut self, ctx: &egui::Context) {
        let (Some(a), Some(p)) = (self.primary().cloned(), self.cur().cloned()) else { return };
        if !matches!(a.level, Level::Line | Level::Word | Level::Region) || a.text().trim().is_empty() {
            self.error("Select a line with a transcription to propose its character boxes.");
            return;
        }
        let id = a.id.clone();
        match crate::assist::char_status(&p, &a, &self.cfg()) {
            CharStatus::None(n) | CharStatus::Stale(n) => {
                let splits = self.preview_splits(&id);
                self.op(|p, cfg, user, now| crate::assist::regenerate_chars_split(p, &id, cfg, user, now, splits.as_ref()));
                self.cell_splits = None;
                self.flash(format!("{n} character box(es) proposed; press C again to accept them"), ctx);
            }
            CharStatus::Proposed(n) => {
                self.op(|p, cfg, user, now| crate::assist::apply_suggestion(p, &id, &crate::assist::Suggest::AcceptChars(n), cfg, user, now));
                self.flash(format!("{n} character box(es) accepted as drafts"), ctx);
            }
            CharStatus::Done(_) => {}
        }
    }

    /// Text-region suggestions for the current page (cached until the page changes).
    pub fn region_suggestions(&mut self) -> Vec<crate::assist::RegionSuggestion> {
        let Some(p) = self.cur() else { return Vec::new() };
        let key = (p.page_id.clone(), self.undo.len(), self.redo.len());
        if let Some((k, v)) = &self.region_cache {
            if *k == key {
                return v.clone();
            }
        }
        let v = crate::assist::suggest_regions(p);
        self.region_cache = Some((key, v.clone()));
        v
    }

    /// Ctrl+G: group the selected labels into one new parent of the next coarser level (lines
    /// into a text region, regions into a balloon…). With nothing selected, create the suggested
    /// text regions of the page.
    pub fn make_regions(&mut self, ctx: &egui::Context) {
        let sel = self.selection.clone();
        // one label that sits among nearby lines: take the suggested group around it
        let single_suggested = sel.len() == 1 && self.region_suggestions().iter().any(|s| s.lines.contains(&sel[0]));
        if !sel.is_empty() && !single_suggested {
            let res = self.op(|p, cfg, user, now| crate::assist::group_into(p, &sel, cfg, user, now));
            match res.flatten() {
                Some((id, level, n)) => {
                    self.selection = vec![id];
                    self.flash(format!("Grouped {n} label(s) into a new {} (Ctrl+Z undoes)", level.name().to_lowercase()), ctx);
                }
                None => self.error("Nothing to group: select labels of one level (e.g. lines) that are not already the coarsest level."),
            }
            return;
        }
        let sugg = self.region_suggestions();
        let chosen: Vec<_> = if sel.is_empty() { sugg } else { sugg.into_iter().filter(|s| s.lines.iter().any(|l| sel.contains(l))).collect() };
        if chosen.is_empty() {
            self.error("No nearby lines to group: lines must sit side by side, outside any text region.");
            return;
        }
        let ids = self.op(|p, cfg, user, now| {
            let ids: Vec<String> = chosen.iter().map(|s| crate::assist::make_region(p, s, user, now)).collect();
            if cfg.reading.auto_suggest {
                crate::assist::apply_order(p, cfg, cfg.reading.page_direction);
            }
            ids
        });
        let n = ids.as_ref().map(|v| v.len()).unwrap_or(0);
        if let Some(ids) = ids {
            self.selection = ids;
        }
        self.flash(format!("{n} text region(s) proposed around nearby lines; accept with Y or adjust them"), ctx);
    }

    /// Change the selected shapes to another form: `box`, `quad` or `polygon`.
    pub fn convert_selection(&mut self, ctx: &egui::Context, kind: &str) {
        let sel = self.selection.clone();
        self.commit();
        let mut n = 0;
        if let Some(p) = self.cur_mut() {
            for a in p.annotations.iter_mut().filter(|a| sel.contains(&a.id)) {
                let g = match kind {
                    "box" => crate::assist::to_box(&a.geometry),
                    "quad" => crate::assist::to_quad(&a.geometry),
                    _ => crate::assist::to_polygon(&a.geometry),
                };
                if g != a.geometry {
                    a.geometry = g;
                    n += 1;
                }
            }
        }
        if kind == "quad" {
            self.keep_form.extend(sel.iter().cloned());
        } else {
            sel.iter().for_each(|id| {
                self.keep_form.remove(id);
            });
        }
        self.commit();
        let name = match kind {
            "box" => "boxes",
            "quad" => "quadrilaterals",
            _ => "polygons",
        };
        self.flash(format!("{n} shape(s) converted to {name}"), ctx);
    }

    /// Remove small tilts (up to the project threshold) from boxes: selection, or every box on the page.
    pub fn straighten_selection(&mut self, ctx: &egui::Context, whole_page: bool) {
        let sel = self.selection.clone();
        let max_deg = self.cfg().quick.straighten_max_deg;
        self.commit();
        let mut n = 0;
        if let Some(p) = self.cur_mut() {
            for a in p.annotations.iter_mut().filter(|a| whole_page || sel.contains(&a.id)) {
                if let Some(g) = crate::assist::straighten(&a.geometry, max_deg) {
                    a.geometry = g;
                    n += 1;
                }
            }
        }
        self.commit();
        self.flash(if n == 0 { "No slightly tilted boxes".to_string() } else { format!("{n} box(es) straightened") }, ctx);
    }

    /// Turn rectangular quadrilaterals into rotated boxes (selection, or the whole page).
    pub fn convert_quads(&mut self, ctx: &egui::Context, whole_page: bool) {
        let sel = self.selection.clone();
        self.commit();
        let mut n = 0;
        if let Some(p) = self.cur_mut() {
            for a in p.annotations.iter_mut().filter(|a| whole_page || sel.contains(&a.id)) {
                if let Some(b) = crate::assist::quad_to_box(&a.geometry) {
                    let linked = matches!(&a.geometry, Geometry::Quad { points } if a.rectification.as_ref().is_some_and(|r| r.source == *points));
                    a.geometry = b;
                    // a flattening that followed the quadrilateral keeps its own corners
                    let _ = linked;
                    n += 1;
                }
            }
        }
        self.commit();
        self.flash(if n == 0 { "No rectangular quadrilaterals to convert".to_string() } else { format!("{n} quadrilateral(s) converted to boxes") }, ctx);
    }
}
