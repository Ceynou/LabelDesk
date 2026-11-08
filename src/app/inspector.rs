use super::*;
use eframe::egui::{Align, CollapsingHeader, FontId, Layout, RichText, TextEdit};
use unicode_segmentation::UnicodeSegmentation;

fn chip(ui: &mut Ui, text: &str, col: Color32) {
    egui::Frame::new().fill(col.gamma_multiply(0.25)).stroke(Stroke::new(1.0, col)).corner_radius(9.0).inner_margin(Margin::symmetric(8, 1)).show(ui, |ui| {
        ui.label(RichText::new(text).color(Color32::WHITE).size(11.5));
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).size(11.0).strong().color(MUTED));
}

/// Character offset → grapheme cluster offset.
fn grapheme_index(text: &str, char_idx: usize) -> usize {
    let mut chars = 0;
    for (gi, g) in text.graphemes(true).enumerate() {
        if chars >= char_idx {
            return gi;
        }
        chars += g.chars().count();
    }
    text.graphemes(true).count()
}

fn conf_row(ui: &mut Ui, label: &str, v: &mut Option<Confidence>) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 18.0], egui::Label::new(RichText::new(label).color(MUTED)));
        ui.selectable_value(v, None, "—");
        if *v == Some(Confidence::Uncertain) {
            ui.label(RichText::new("uncertain →").color(WARN));
        }
        ui.selectable_value(v, Some(Confidence::Low), "Low");
        ui.selectable_value(v, Some(Confidence::Medium), "Medium");
        ui.selectable_value(v, Some(Confidence::High), "Certain");
    });
}

impl App {
    pub(super) fn inspector(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (t, name) in [(Tab::Text, "Annotation"), (Tab::Review, "Review"), (Tab::Flatten, "Flatten"), (Tab::Page, "Page")] {
                let on = self.tab == t;
                let b = egui::Button::new(RichText::new(name).color(if on { Color32::WHITE } else { MUTED })).fill(if on { ACCENT } else { PANEL_BG2 }).corner_radius(6.0);
                if ui.add(b).clicked() {
                    self.tab = t;
                }
            }
        });
        ui.separator();
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| match self.tab {
            Tab::Text => self.tab_text(ui),
            Tab::Review => self.tab_review(ui),
            Tab::Flatten => self.tab_flatten(ui),
            Tab::Page => self.tab_page(ui),
        });
    }

    fn write_back(&mut self, before: &Annotation, after: Annotation) {
        if *before != after {
            if let Some(a) = self.cur_mut().and_then(|p| p.get_mut(&after.id)) {
                *a = after;
            }
        }
    }

    fn nothing_selected(&mut self, ui: &mut Ui) {
        ui.add_space(10.0);
        ui.label(RichText::new("Nothing selected").strong());
        ui.label(RichText::new("Click a shape on the page, or draw one with the tools on the left. AI proposals have dashed outlines.").color(MUTED));
        ui.add_space(8.0);
        self.structure_tree(ui, 520.0);
    }

    /// The page's annotations as a tree (panel › container › region › line › character).
    /// Rows with children fold; lines fold their characters by default.
    pub(super) fn structure_tree(&mut self, ui: &mut Ui, max_h: f32) {
        let Some(p) = self.cur() else { return };
        let ids: std::collections::HashSet<&str> = p.annotations.iter().map(|a| a.id.as_str()).collect();
        let mut kids: std::collections::HashMap<Option<&str>, Vec<&Annotation>> = std::collections::HashMap::new();
        for a in &p.annotations {
            let parent = a.parent.as_deref().filter(|x| ids.contains(x));
            kids.entry(parent).or_default().push(a);
        }
        for v in kids.values_mut() {
            v.sort_by_key(|a| (a.level, a.reading_order.unwrap_or(u32::MAX)));
        }
        let default_open = |a: &Annotation, kids: &std::collections::HashMap<Option<&str>, Vec<&Annotation>>| {
            !kids.get(&Some(a.id.as_str())).is_some_and(|k| k.iter().all(|c| c.level == Level::Char))
        };
        // flatten what is visible
        let mut rows: Vec<(usize, &Annotation, bool, bool)> = Vec::new();
        let mut stack: Vec<(usize, &Annotation)> = kids.get(&None).map(|v| v.iter().rev().map(|a| (0, *a)).collect()).unwrap_or_default();
        while let Some((d, a)) = stack.pop() {
            let ch = kids.get(&Some(a.id.as_str()));
            let has = ch.is_some_and(|c| !c.is_empty());
            let open = default_open(a, &kids) != self.tree_toggled.contains(&a.id);
            rows.push((d, a, has, open));
            if has && open {
                for c in ch.unwrap().iter().rev() {
                    stack.push((d + 1, c));
                }
            }
        }
        let total = p.annotations.len();
        let (mut open_default, mut closed_default) = (Vec::new(), Vec::new());
        for a in &p.annotations {
            if kids.get(&Some(a.id.as_str())).is_some_and(|k| !k.is_empty()) {
                if default_open(a, &kids) { open_default.push(a.id.clone()) } else { closed_default.push(a.id.clone()) }
            }
        }
        let rows: Vec<(usize, String, String, Level, ReviewState, bool, bool, Option<u32>)> =
            rows.into_iter().map(|(d, a, has, open)| (d, a.id.clone(), a.text().replace('\n', " / "), a.level, a.review.state, has, open, a.reading_order)).collect();
        ui.horizontal(|ui| {
            section(ui, &format!("ON THIS PAGE ({total})"));
            if ui.small_button("Fold all").clicked() {
                self.tree_toggled = open_default.iter().cloned().collect();
            }
            if ui.small_button("Unfold all").clicked() {
                self.tree_toggled = closed_default.iter().cloned().collect();
            }
        });
        let row_h = 20.0;
        egui::ScrollArea::vertical().id_salt("tree").max_height(max_h).auto_shrink([false, true]).show_rows(ui, row_h, rows.len(), |ui, range| {
            for (d, id, text, level, state, has, open, order) in &rows[range] {
                ui.horizontal(|ui| {
                    ui.add_space(*d as f32 * 14.0);
                    if *has {
                        if ui.add(egui::Button::new(RichText::new(if *open { "▾" } else { "▸" }).size(12.0)).frame(false).min_size(vec2(14.0, 14.0))).clicked() {
                            if !self.tree_toggled.remove(id) {
                                self.tree_toggled.insert(id.clone());
                            }
                        }
                    } else {
                        ui.add_space(18.0);
                    }
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 4.0, rgb(state.color()));
                    let ord = order.map(|o| format!("{o}. ")).unwrap_or_default();
                    let label = if text.is_empty() { format!("{ord}{}", level.name()) } else { format!("{ord}{}: {}", level.name(), text.chars().take(24).collect::<String>()) };
                    let sel = self.selection.contains(id);
                    if ui.selectable_label(sel, label).clicked() {
                        let multi = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                        if !multi {
                            self.selection = vec![id.clone()];
                        } else if let Some(k) = self.selection.iter().position(|s| s == id) {
                            self.selection.remove(k);
                        } else {
                            self.selection.push(id.clone());
                        }
                    }
                });
            }
        });
    }

    pub(super) fn tab_text_public(&mut self, ui: &mut Ui) {
        self.tab_text(ui);
    }

    fn tab_text(&mut self, ui: &mut Ui) {
        let cfg = self.cfg();
        if self.selection.len() > 1 {
            ui.label(RichText::new(format!("{} annotations selected", self.selection.len())).strong());
            ui.label(RichText::new("Set for all of them:").color(MUTED));
            let mut set_level = None;
            let mut set_class = None;
            ui.horizontal_wrapped(|ui| {
                for l in Level::DRAWABLE {
                    if ui.small_button(l.name()).clicked() {
                        set_level = Some(l);
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                for c in &cfg.text_classes {
                    if ui.small_button(&c.name).clicked() {
                        set_class = Some(c.id.clone());
                    }
                }
            });
            if set_level.is_some() || set_class.is_some() {
                let sel = self.selection.clone();
                if let Some(p) = self.cur_mut() {
                    for id in &sel {
                        if let Some(a) = p.get_mut(id) {
                            if let Some(l) = set_level {
                                a.level = l;
                            }
                            if let Some(c) = &set_class {
                                a.classes = [c.clone()].into_iter().collect();
                            }
                        }
                    }
                }
            }
            ui.separator();
            self.batch_panel(ui);
            return;
        }
        let Some(orig) = self.primary().cloned() else {
            self.nothing_selected(ui);
            return;
        };
        if !self.cur().is_some_and(|p| p.get(&orig.id).is_some()) {
            return;
        }
        self.assist_strip(ui, &orig);
        let mut a = orig.clone();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("level").width(130.0).selected_text(a.level.name()).show_ui(ui, |ui| {
                for l in Level::DRAWABLE {
                    ui.selectable_value(&mut a.level, l, l.name());
                }
            });
            chip(ui, a.review.state.name(), rgb(a.review.state.color()));
            if a.review.needs_rereview {
                chip(ui, "needs re-review", BAD);
            }
        });
        ui.label(RichText::new(format!("{} · {} · revision {}", a.id, a.geometry.kind(), a.revision)).monospace().size(10.0).color(MUTED));

        section(ui, "TEXT CLASS");
        ui.horizontal_wrapped(|ui| {
            for c in &cfg.text_classes {
                let on = a.classes.contains(&c.id);
                let b = egui::Button::selectable(on, RichText::new(&c.name).size(12.0)).corner_radius(9.0);
                let r = ui.add(b);
                if on {
                    ui.painter().rect_stroke(r.rect, 9.0, Stroke::new(1.5, rgb(c.color)), egui::StrokeKind::Inside);
                }
                if r.clicked() {
                    if on {
                        a.classes.remove(&c.id);
                    } else {
                        a.classes.insert(c.id.clone());
                    }
                }
            }
        });
        if !cfg.attributes.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Attributes").color(MUTED).size(11.5));
                for at in &cfg.attributes {
                    let mut on = a.attributes.contains(at);
                    if ui.toggle_value(&mut on, RichText::new(at).size(11.5)).changed() {
                        if on {
                            a.attributes.insert(at.clone());
                        } else {
                            a.attributes.remove(at);
                        }
                    }
                }
            });
        }

        section(ui, "TRANSCRIPTION");
        if a.transcription.is_none() {
            if ui.button("Add a transcription").clicked() {
                a.transcription = Some(Transcription {
                    language: (!cfg.default_language.is_empty()).then(|| cfg.default_language.clone()),
                    script: (!cfg.default_script.is_empty()).then(|| cfg.default_script.clone()),
                    writing_mode: cfg.default_writing_mode,
                    direction: cfg.default_direction,
                    ..Default::default()
                });
            }
        }
        let user = self.user();
        if let Some(t) = &mut a.transcription {
            let out = TextEdit::multiline(&mut t.text).font(FontId::proportional(19.0)).desired_rows(2).desired_width(f32::INFINITY).hint_text("Exact visible text").show(ui);
            if let Some(cr) = out.cursor_range {
                let (x, y) = (cr.primary.index.0, cr.secondary.index.0);
                self.text_sel = (x != y).then(|| (x.min(y), x.max(y)));
            }
            let n = t.text.graphemes(true).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{n} characters")).color(MUTED).size(11.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let sel = self.text_sel;
                    for (kind, name) in [(SpanKind::Unreadable, "Mark unreadable"), (SpanKind::Uncertain, "Mark uncertain")] {
                        if ui.add_enabled(sel.is_some(), egui::Button::new(RichText::new(name).size(11.5))).on_hover_text("Select part of the text first").clicked() {
                            if let Some((s, e)) = sel {
                                let (gs, ge) = (grapheme_index(&t.text, s), grapheme_index(&t.text, e));
                                if ge > gs {
                                    t.spans.push(Span { start: gs, end: ge, kind, note: String::new() });
                                    t.spans.sort_by_key(|x| x.start);
                                }
                            }
                        }
                    }
                });
            });
            let graphemes: Vec<&str> = t.text.graphemes(true).collect();
            let mut remove = None;
            for (i, s) in t.spans.iter().enumerate() {
                ui.horizontal(|ui| {
                    let part: String = graphemes.get(s.start.min(graphemes.len())..s.end.min(graphemes.len())).map(|g| g.concat()).unwrap_or_default();
                    let col = if s.kind == SpanKind::Uncertain { WARN } else { BAD };
                    ui.label(RichText::new(format!("{} “{part}”", if s.kind == SpanKind::Uncertain { "uncertain" } else { "unreadable" })).color(col).size(12.0));
                    if ui.small_button("remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                t.spans.remove(i);
            }
            egui::Grid::new("tr_grid").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("Language").color(MUTED));
                ui.horizontal(|ui| {
                    let mut lang = t.language.clone().unwrap_or_default();
                    if ui.add(TextEdit::singleline(&mut lang).desired_width(60.0).hint_text("ja")).changed() {
                        t.language = (!lang.trim().is_empty()).then(|| lang.trim().to_string());
                    }
                    for l in cfg.languages.iter().take(5) {
                        if ui.small_button(l).clicked() {
                            t.language = Some(l.clone());
                            t.script = Some(match l.as_str() {
                                "ja" => "Jpan",
                                "ko" => "Hang",
                                "zh" | "zh-Hans" => "Hans",
                                "zh-Hant" => "Hant",
                                "ar" | "fa" => "Arab",
                                "he" => "Hebr",
                                "th" => "Thai",
                                "ru" | "uk" => "Cyrl",
                                _ => "Latn",
                            }
                            .into());
                        }
                    }
                });
                ui.end_row();
                ui.label(RichText::new("Script").color(MUTED));
                let mut sc = t.script.clone().unwrap_or_default();
                if ui.add(TextEdit::singleline(&mut sc).desired_width(60.0).hint_text("Jpan")).changed() {
                    t.script = (!sc.trim().is_empty()).then(|| sc.trim().to_string());
                }
                ui.end_row();
                ui.label(RichText::new("Writing").color(MUTED));
                egui::ComboBox::from_id_salt("wm").width(200.0).selected_text(t.writing_mode.name()).show_ui(ui, |ui| {
                    for m in WritingMode::ALL {
                        ui.selectable_value(&mut t.writing_mode, m, m.name());
                    }
                });
                ui.end_row();
                ui.label(RichText::new("Direction").color(MUTED));
                egui::ComboBox::from_id_salt("dir").width(200.0).selected_text(t.direction.name()).show_ui(ui, |ui| {
                    for d in Direction::ALL {
                        ui.selectable_value(&mut t.direction, d, d.name());
                    }
                });
                ui.end_row();
            });
            let mut has_norm = t.normalized.is_some();
            if ui.checkbox(&mut has_norm, "Normalized form").changed() {
                t.normalized = has_norm.then(|| t.text.clone());
            }
            if let Some(nz) = &mut t.normalized {
                ui.add(TextEdit::singleline(nz).desired_width(f32::INFINITY));
            }
            let mut inferred = t.source == TextSource::Inferred;
            if ui.checkbox(&mut inferred, "Some of this text is inferred, not directly visible").changed() {
                t.source = if inferred { TextSource::Inferred } else { TextSource::Visible };
            }
            if !t.alternatives.is_empty() {
                section(ui, "OTHER READINGS");
                let mut use_alt = None;
                let mut drop_alt = None;
                for (i, alt) in t.alternatives.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&alt.text).size(15.0));
                        ui.label(RichText::new(format!("by {}", alt.by)).color(MUTED).size(10.5));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✕").on_hover_text("Remove").clicked() {
                                drop_alt = Some(i);
                            }
                            if ui.small_button("Use").on_hover_text("Make this the transcription; the current one becomes an alternative").clicked() {
                                use_alt = Some(i);
                            }
                        });
                    });
                }
                if let Some(i) = use_alt {
                    let alt = t.alternatives.remove(i);
                    let old = std::mem::replace(&mut t.text, alt.text);
                    t.spans.clear();
                    if !old.is_empty() {
                        t.alternatives.push(Alternative { text: old, by: user.clone(), at: util::now(), note: "replaced".into() });
                    }
                }
                if let Some(i) = drop_alt {
                    t.alternatives.remove(i);
                }
            }
        }

        CollapsingHeader::new("Translations and transliterations").default_open(!a.translations.is_empty()).show(ui, |ui| {
            let mut drop = None;
            for (i, tr) in a.translations.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} ({})", tr.language, if tr.kind == TranslationKind::Translation { "translation" } else { "transliteration" })).color(MUTED).size(11.0));
                    ui.add(TextEdit::singleline(&mut tr.text).desired_width(170.0));
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                a.translations.remove(i);
            }
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.new_translation.0).desired_width(40.0).hint_text("en"));
                ui.add(TextEdit::singleline(&mut self.new_translation.1).desired_width(150.0).hint_text("Translation"));
                for (kind, label) in [(TranslationKind::Translation, "+ translation"), (TranslationKind::Transliteration, "+ romanised")] {
                    if ui.small_button(label).clicked() && !self.new_translation.1.is_empty() {
                        a.translations.push(Translation {
                            kind,
                            language: if self.new_translation.0.is_empty() { "en".into() } else { self.new_translation.0.clone() },
                            text: std::mem::take(&mut self.new_translation.1),
                            by: user.clone(),
                        });
                    }
                }
            });
        });

        CollapsingHeader::new("Legibility and image quality").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut a.recognizability, None, "—");
                if a.recognizability == Some(Recognizability::NotPerfect) {
                    ui.label(RichText::new("not perfect → refine:").color(WARN));
                }
                for r in Recognizability::ALL {
                    ui.selectable_value(&mut a.recognizability, Some(r), r.name());
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (k, name) in QUALITY_FLAGS {
                    let mut on = a.quality.contains(k);
                    if ui.toggle_value(&mut on, RichText::new(name).size(11.5)).changed() {
                        if on {
                            a.quality.insert(k.to_string());
                        } else {
                            a.quality.remove(k);
                        }
                    }
                }
            });
        });

        CollapsingHeader::new("Orientation and occlusion").default_open(false).show(ui, |ui| {
            egui::Grid::new("orient").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                ui.label(RichText::new("Text rotation").color(MUTED));
                ui.add(egui::DragValue::new(&mut a.orientation.rotation_deg).speed(1.0).range(-180.0..=180.0).suffix("°"));
                ui.end_row();
                ui.label(RichText::new("Skew").color(MUTED));
                ui.add(egui::DragValue::new(&mut a.orientation.skew_deg).speed(0.5).range(-80.0..=80.0).suffix("°"));
                ui.end_row();
                ui.label(RichText::new("Mirrored").color(MUTED));
                ui.checkbox(&mut a.orientation.mirrored, "");
                ui.end_row();
            });
            if let Geometry::Box { rotation_deg, .. } = &mut a.geometry {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Box rotation").color(MUTED));
                    ui.add(egui::DragValue::new(rotation_deg).speed(1.0).range(-180.0..=180.0).suffix("°"));
                });
            }
            let mut occ = a.occlusion.clone().unwrap_or_default();
            ui.checkbox(&mut occ.occluded, "Partly hidden by something");
            if occ.occluded {
                let mut kind = occ.kind.clone().unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("By").color(MUTED));
                    if ui.add(TextEdit::singleline(&mut kind).hint_text("hair, panel edge, object…").desired_width(170.0)).changed() {
                        occ.kind = (!kind.is_empty()).then(|| kind.clone());
                    }
                });
                let mut has_f = occ.fraction.is_some();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut has_f, "Estimate hidden share");
                    if has_f {
                        let mut f = occ.fraction.unwrap_or(0.25);
                        ui.add(egui::Slider::new(&mut f, 0.0..=1.0).show_value(true));
                        occ.fraction = Some(f);
                    } else {
                        occ.fraction = None;
                    }
                });
            }
            a.occlusion = if occ.occluded || occ.kind.is_some() { Some(occ) } else { None };
        });

        CollapsingHeader::new("Your confidence").default_open(false).show(ui, |ui| {
            conf_row(ui, "Transcription", &mut a.confidence.transcription);
            conf_row(ui, "Localization", &mut a.confidence.localization);
            conf_row(ui, "Reading order", &mut a.confidence.reading_order);
            conf_row(ui, "Geometry", &mut a.confidence.geometry);
            ui.label(RichText::new("Kept apart from any model score.").color(MUTED).size(11.0));
        });

        let candidates: Vec<(String, String)> = self
            .cur()
            .map(|p| {
                p.annotations
                    .iter()
                    .filter(|x| x.id != a.id && crate::assist::parent_allowed(a.level, x.level))
                    .map(|x| {
                        let t = x.text().replace('\n', " ");
                        let cls = x.classes.iter().next().and_then(|c| cfg.class(c)).map(|c| c.name.clone()).unwrap_or_default();
                        (x.id.clone(), format!("{} {} {}", x.level.name(), cls, t.chars().take(18).collect::<String>()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        CollapsingHeader::new("Structure and reading order").default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Inside").color(MUTED));
                let cur = a.parent.as_ref().and_then(|p| candidates.iter().find(|c| &c.0 == p)).map(|c| c.1.clone()).unwrap_or_else(|| "the page".into());
                egui::ComboBox::from_id_salt("parent").width(230.0).selected_text(cur).show_ui(ui, |ui| {
                    ui.selectable_value(&mut a.parent, None, "the page");
                    for (id, label) in &candidates {
                        ui.selectable_value(&mut a.parent, Some(id.clone()), label);
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Reading order").color(MUTED));
                let mut has = a.reading_order.is_some();
                if ui.checkbox(&mut has, "").changed() {
                    a.reading_order = if has { Some(1) } else { None };
                }
                if let Some(r) = &mut a.reading_order {
                    ui.add(egui::DragValue::new(r).range(1..=999));
                }
                if ui.small_button("Order tool").on_hover_text("Click items in reading order on the page").clicked() {
                    self.set_tool(Tool::Order);
                }
            });
            let mut drop = None;
            for (i, r) in a.relations.iter().enumerate() {
                ui.horizontal(|ui| {
                    let target = candidates.iter().find(|c| c.0 == r.target).map(|c| c.1.clone()).unwrap_or_else(|| r.target.clone());
                    ui.label(RichText::new(format!("{} → {target}", r.kind)).size(12.0));
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                a.relations.remove(i);
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Add link:").color(MUTED).size(11.5));
                if ui.small_button("furigana of…").on_hover_text("Click the characters this furigana reads; it then leaves the reading order").clicked() {
                    self.link_relation = Some(crate::assist::FURIGANA_OF.into());
                    self.set_tool(Tool::Link);
                }
                for kind in ["continues", "same_speaker", "refers_to", "duplicate_of"] {
                    if ui.small_button(kind).on_hover_text("Then click the target on the page").clicked() {
                        self.link_relation = Some(kind.into());
                        self.set_tool(Tool::Link);
                    }
                }
                if ui.small_button("parent…").clicked() {
                    self.link_relation = None;
                    self.set_tool(Tool::Link);
                }
            });
        });

        if !cfg.custom_fields.is_empty() {
            CollapsingHeader::new("Project fields").default_open(true).show(ui, |ui| {
                for f in cfg.custom_fields.iter().filter(|f| f.levels.is_empty() || f.levels.contains(&a.level)) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}{}", f.label, if f.required { " *" } else { "" })).color(MUTED)).on_hover_text(&f.key);
                        let v = a.fields.get(&f.key).cloned().unwrap_or(serde_json::Value::Null);
                        let new = match f.kind {
                            FieldKind::Text => {
                                let mut s = v.as_str().unwrap_or("").to_string();
                                ui.add(TextEdit::singleline(&mut s).desired_width(160.0));
                                (!s.is_empty()).then(|| serde_json::Value::from(s))
                            }
                            FieldKind::Number => {
                                let mut x = v.as_f64().unwrap_or(f.min.unwrap_or(0.0));
                                let mut has = !v.is_null();
                                ui.checkbox(&mut has, "");
                                if has {
                                    ui.add(egui::DragValue::new(&mut x).speed(0.1));
                                }
                                has.then(|| serde_json::json!(x))
                            }
                            FieldKind::Bool => {
                                let mut b = v.as_bool().unwrap_or(false);
                                ui.checkbox(&mut b, "");
                                Some(serde_json::Value::from(b))
                            }
                            FieldKind::Choice => {
                                let mut s = v.as_str().map(String::from);
                                egui::ComboBox::from_id_salt(&f.key).selected_text(s.clone().unwrap_or_else(|| "—".into())).show_ui(ui, |ui| {
                                    ui.selectable_value(&mut s, None, "—");
                                    for c in &f.choices {
                                        ui.selectable_value(&mut s, Some(c.clone()), c);
                                    }
                                });
                                s.map(serde_json::Value::from)
                            }
                        };
                        match new {
                            Some(n) => {
                                if a.fields.get(&f.key) != Some(&n) {
                                    a.fields.insert(f.key.clone(), n);
                                }
                            }
                            None => {
                                a.fields.remove(&f.key);
                            }
                        }
                    });
                }
            });
        }

        CollapsingHeader::new("Where this came from").default_open(a.provenance.origin == Origin::Ai).show(ui, |ui| {
            ui.label(format!("Origin: {:?}", a.provenance.origin).to_lowercase());
            if let Some(ai) = &a.provenance.ai {
                ui.label(RichText::new(format!("{} {} (revision {})", ai.model, ai.model_version, &ai.model_revision[..10.min(ai.model_revision.len())])).monospace().size(11.0));
                ui.label(format!(
                    "Model scores: detection {} · recognition {}",
                    ai.detection_score.map(|s| format!("{s:.2}")).unwrap_or("—".into()),
                    ai.recognition_score.map(|s| format!("{s:.2}")).unwrap_or("—".into())
                ));
                ui.label(RichText::new("Model-native scores, not calibrated probabilities.").color(MUTED).size(11.0));
            }
            for (g, st) in &a.provenance.fields {
                ui.label(RichText::new(format!("{g}: {}", st.name())).size(12.0));
            }
            ui.label(RichText::new(format!("Created by {} at {}", a.created.by, a.created.at)).color(MUTED).size(11.0));
            ui.label(RichText::new(format!("Last changed by {} at {}", a.modified.by, a.modified.at)).color(MUTED).size(11.0));
        });

        self.write_back(&orig, a);
    }

    fn tab_review(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let cfg = self.cfg();
        let Some(a) = self.primary().cloned() else {
            self.nothing_selected(ui);
            return;
        };
        let n = self.selection.len();
        ui.horizontal(|ui| {
            chip(ui, a.review.state.name(), rgb(a.review.state.color()));
            if a.review.needs_rereview {
                chip(ui, "changed after approval", BAD);
            }
            if n > 1 {
                ui.label(RichText::new(format!("(actions apply to all {n} selected)")).color(MUTED));
            }
        });
        if a.review.state == ReviewState::AiProposed {
            ui.add_space(4.0);
            ui.label(RichText::new("Proposed by the model. Accept it as is, correct it (just edit it), or reject it.").color(MUTED));
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Accept  (Y)").color(Color32::WHITE)).fill(Color32::from_rgb(46, 125, 80))).clicked() {
                    self.accept_ai();
                }
                if ui.button("Reject…  (X)").clicked() {
                    self.dialogs.reject = Some((a.id.clone(), "not text".into()));
                }
            });
        }

        section(ui, "APPROVALS");
        let required = &cfg.review.required_dimensions;
        egui::Grid::new("approvals").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
            for d in Dimension::ALL {
                let req = required.contains(&d);
                ui.label(RichText::new(format!("{}{}", d.name(), if req { "" } else { " (optional)" })).color(if req { Color32::WHITE } else { MUTED }));
                ui.vertical(|ui| {
                    let list: Vec<&Approval> = a.review.approvals.iter().filter(|x| x.dimension == d).collect();
                    if list.is_empty() {
                        ui.label(RichText::new("not yet approved").color(MUTED).size(12.0));
                    }
                    for ap in list {
                        match &ap.invalidated {
                            None => ui.label(RichText::new(format!("✔ {} · rev {} · policy v{}", ap.by, ap.revision, ap.policy_version)).color(GOOD).size(12.0)),
                            Some(inv) => ui.label(RichText::new(format!("✗ {} (rev {}) · withdrawn: {}", ap.by, ap.revision, inv.reason)).color(MUTED).strikethrough().size(11.5)),
                        };
                    }
                });
                ui.end_row();
            }
        });
        let others = review::full_reviewers(&a, &cfg.review).len();
        ui.label(RichText::new(format!("{others} of {} required reviewer(s) approved everything", cfg.review.reviewers_required)).color(MUTED).size(11.5));
        if let Err(e) = review::can_review(&a, &cfg.review, &self.settings.user) {
            ui.label(RichText::new(e).color(WARN).size(11.5));
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Submit for review").clicked() {
                self.submit();
            }
            if ui.add(egui::Button::new(RichText::new("Approve all").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                self.approve(&Dimension::ALL, &ctx);
            }
            for d in Dimension::ALL {
                if ui.small_button(format!("✔ {}", d.name().to_lowercase())).clicked() {
                    self.approve(&[d], &ctx);
                }
            }
        });
        let page = self.cur().cloned();
        if let Some(page) = page {
            let blockers = review::gold_blockers(&a, &page, &cfg.review);
            ui.horizontal(|ui| {
                let gold = egui::Button::new(RichText::new("Approve as gold").color(Color32::BLACK)).fill(rgb(ReviewState::GoldApproved.color()));
                if ui.add_enabled(blockers.is_empty() && a.review.gold.is_none(), gold).clicked() {
                    self.approve_gold(&ctx);
                }
                if let Some(g) = &a.review.gold {
                    ui.label(RichText::new(format!("Gold by {} (policy v{})", g.by, g.policy_version)).color(rgb(ReviewState::GoldApproved.color())));
                }
            });
            for b in blockers {
                ui.label(RichText::new(format!("• {b}")).color(MUTED).size(11.5));
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Request changes…").clicked() {
                self.dialogs.note = Some((a.id.clone(), NoteKind::ChangeRequest, None, String::new()));
            }
            if ui.button("Disagree…").clicked() {
                self.dialogs.note = Some((a.id.clone(), NoteKind::Disagreement, Some(Dimension::Transcription), String::new()));
            }
            if ui.button("Comment…").clicked() {
                self.dialogs.note = Some((a.id.clone(), NoteKind::Comment, None, String::new()));
            }
        });

        if !a.review.notes.is_empty() {
            section(ui, "NOTES");
            let mut resolve = None;
            for (i, note) in a.review.notes.iter().enumerate() {
                egui::Frame::new().fill(PANEL_BG2).corner_radius(5.0).inner_margin(6).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let kind = match note.kind {
                        NoteKind::Comment => "Comment",
                        NoteKind::ChangeRequest => "Change request",
                        NoteKind::Disagreement => "Disagreement",
                        NoteKind::Rejection => "Rejection",
                    };
                    ui.label(RichText::new(format!("{kind}{} · {} · {}", note.dimension.map(|d| format!(" ({})", d.name().to_lowercase())).unwrap_or_default(), note.by, note.at)).color(MUTED).size(11.0));
                    ui.label(&note.text);
                    match &note.resolved {
                        Some(r) => {
                            ui.label(RichText::new(format!("Resolved by {}: {}", r.by, note.resolution)).color(GOOD).size(11.0));
                        }
                        None if note.kind != NoteKind::Comment => {
                            if ui.small_button("Mark resolved").clicked() {
                                resolve = Some(i);
                            }
                        }
                        None => {}
                    }
                });
            }
            if let Some(i) = resolve {
                let id = a.id.clone();
                self.op(|p, _, user, now| {
                    if let Some(x) = p.get_mut(&id) {
                        if let Some(n) = x.review.notes.get_mut(i) {
                            n.resolved = Some(Stamp { by: user.into(), at: now.into() });
                            n.resolution = "resolved".into();
                        }
                    }
                });
            }
        }

        if let Some(ai) = &a.provenance.ai {
            section(ui, "MODEL OUTPUT VS. NOW");
            let pred = self.cur().and_then(|p| p.inference_runs.iter().find(|r| r.id == ai.run_id)).and_then(|r| r.predictions.get(ai.prediction)).cloned();
            if let Some(pred) = pred {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Model read").color(MUTED));
                    ui.label(RichText::new(&pred.text).size(16.0));
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Now").color(MUTED));
                    ui.label(RichText::new(a.text()).size(16.0));
                });
                let cer = ai::cer(a.text(), &pred.text);
                ui.label(RichText::new(format!("Character error rate of the model here: {:.0}%", cer * 100.0)).color(if cer == 0.0 { GOOD } else { WARN }));
                let moved = geom::dist(pred.quad[0], a.geometry.outline()[0]);
                ui.label(RichText::new(format!("Shape {}", if moved < 0.5 { "unchanged".to_string() } else { format!("adjusted (first corner moved {moved:.1} px)") })).color(MUTED).size(11.5));
            }
        }

        section(ui, "HISTORY (saved revisions)");
        let hist = self.store.as_ref().zip(self.cur_id()).map(|(s, pid)| s.history(&pid)).unwrap_or_default();
        let mine: Vec<_> = hist.into_iter().filter(|h| h.annotation_id == a.id).collect();
        if mine.is_empty() {
            ui.label(RichText::new("No saved changes yet.").color(MUTED).size(11.5));
        }
        for h in mine.iter().rev().take(20) {
            CollapsingHeader::new(RichText::new(format!("{} · {} · {:?} {}", h.at, h.by, h.action, h.groups.join(", ")).to_lowercase()).size(11.5)).id_salt(format!("{}{}", h.at, h.groups.join(""))).show(ui, |ui| {
                let (b, af) = (h.before.clone().unwrap_or_default(), h.after.clone().unwrap_or_default());
                let keys: Vec<String> = b.as_object().into_iter().chain(af.as_object()).flat_map(|o| o.keys().cloned()).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                for k in keys {
                    if matches!(k.as_str(), "modified" | "revision") {
                        continue;
                    }
                    let (x, y) = (b.get(&k), af.get(&k));
                    if x != y {
                        let short = |v: Option<&serde_json::Value>| v.map(|v| v.to_string().chars().take(120).collect::<String>()).unwrap_or_else(|| "—".into());
                        ui.label(RichText::new(&k).strong().size(11.5));
                        ui.label(RichText::new(format!("before: {}", short(x))).color(MUTED).monospace().size(10.5));
                        ui.label(RichText::new(format!("after:  {}", short(y))).monospace().size(10.5));
                    }
                }
            });
        }
    }

    fn tab_flatten(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(orig) = self.primary().cloned() else {
            ui.label(RichText::new("Select a sign, a screen or any flat text surface to flatten it.").color(MUTED));
            return;
        };
        let mut a = orig.clone();
        let Some(corners) = a.geometry.corners() else {
            ui.label(RichText::new("Flattening needs four corners: use a box or a quadrilateral. (Polygons keep their outline.)").color(MUTED));
            if ui.button("Add a flattening quadrilateral around this shape").clicked() {
                let (c, s, ang) = geom::min_area_rect(&a.geometry.outline());
                let q = geom::order_quad(geom::rect_corners(c, s, ang));
                let (w, h) = geom::proposed_size(&q);
                let mut r = Rectification { role: QuadRole::Surface, source: q, target_size: [w, h], target_size_manual: false, homography: None };
                r.update_cache();
                a.rectification = Some(r);
                self.write_back(&orig, a);
            }
            return;
        };
        if a.rectification.is_none() {
            ui.label(RichText::new("Perspective flattening").strong());
            ui.label(RichText::new("Pick what the four corners describe. You can then drag them on the page while the preview updates.").color(MUTED));
            ui.add_space(6.0);
            let make = |role: QuadRole| {
                let (w, h) = geom::proposed_size(&corners);
                let mut r = Rectification { role, source: corners, target_size: [w, h], target_size_manual: false, homography: None };
                r.update_cache();
                r
            };
            if ui.button("The text region itself").clicked() {
                a.rectification = Some(make(QuadRole::TextRegion));
            }
            if ui.button("The flat surface carrying the text (sign, page, screen)").clicked() {
                a.rectification = Some(make(QuadRole::Surface));
            }
            self.write_back(&orig, a);
            return;
        }
        let r = a.rectification.as_mut().unwrap();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Corners describe").color(MUTED));
            ui.selectable_value(&mut r.role, QuadRole::TextRegion, "text region");
            ui.selectable_value(&mut r.role, QuadRole::Surface, "flat surface");
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Flat size").color(MUTED));
            let mut w = r.target_size[0];
            let mut h = r.target_size[1];
            let cw = ui.add(egui::DragValue::new(&mut w).range(1..=4096).suffix(" px"));
            ui.label("×");
            let ch = ui.add(egui::DragValue::new(&mut h).range(1..=4096).suffix(" px"));
            if cw.changed() || ch.changed() {
                r.target_size = [w, h];
                r.target_size_manual = true;
            }
            if ui.small_button("Auto").on_hover_text("Propose from the corner distances").clicked() {
                let (w, h) = geom::proposed_size(&r.source);
                r.target_size = [w, h];
                r.target_size_manual = false;
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Reset corners to the shape").clicked() {
                r.source = corners;
                if !r.target_size_manual {
                    let (w, h) = geom::proposed_size(&corners);
                    r.target_size = [w, h];
                }
            }
            if ui.small_button("Turn corners 90°").on_hover_text("When the text reads sideways on the surface").clicked() {
                r.source = [r.source[3], r.source[0], r.source[1], r.source[2]];
                r.target_size = [r.target_size[1], r.target_size[0]];
            }
        });
        if !geom::quad_order_ok(&r.source) {
            ui.label(RichText::new("Corners must run top-left, top-right, bottom-right, bottom-left.").color(BAD));
        }
        r.update_cache();
        let key = (a.id.clone(), r.source, r.target_size);
        let page_id = a.page_id.clone();
        let size = r.target_size;
        let source = r.source;
        let hm = r.homography;
        let remove = ui.small_button("Remove flattening").clicked();
        if remove {
            a.rectification = None;
        }
        self.write_back(&orig, a.clone());
        if remove {
            return;
        }
        // live preview (scaled for speed; exports use full resolution)
        if self.preview.as_ref().is_none_or(|p| p.key != key) {
            if let Some(img) = self.page_image(&page_id) {
                let scale = (720.0 / size[0].max(size[1]) as f64).min(1.0);
                let (pw, ph) = (((size[0] as f64 * scale).round() as u32).max(1), ((size[1] as f64 * scale).round() as u32).max(1));
                if let Some(flat) = geom::warp_quad(&img, &source, pw, ph) {
                    let ci = egui::ColorImage::from_rgba_unmultiplied([pw as usize, ph as usize], flat.as_raw());
                    let tex = ctx.load_texture("flat-preview", ci, TextureOptions::LINEAR);
                    self.preview = Some(Preview { key: key.clone(), flat: Arc::new(flat), texture: tex });
                }
            }
        }
        section(ui, "ORIGINAL");
        if let (Some(img), Some(t)) = (self.page_image(&page_id), self.page_texture(&ctx, &page_id)) {
            let (lo, hi) = geom::bounds(&source);
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            let uv = Rect::from_min_max(egui::pos2(lo[0] as f32 / iw, lo[1] as f32 / ih), egui::pos2(hi[0] as f32 / iw, hi[1] as f32 / ih));
            let bw = ((hi[0] - lo[0]) as f32).max(1.0);
            let bh = ((hi[1] - lo[1]) as f32).max(1.0);
            let w = ui.available_width().min(bw * 3.0);
            let h = (w * bh / bw).min(260.0);
            let w = h * bw / bh;
            let (rect, _) = ui.allocate_exact_size(vec2(w, h), egui::Sense::hover());
            ui.painter().image(t.id(), rect, uv, Color32::WHITE);
            let to = |p: P| egui::pos2(rect.left() + ((p[0] - lo[0]) as f32 / bw) * w, rect.top() + ((p[1] - lo[1]) as f32 / bh) * h);
            let q: Vec<egui::Pos2> = source.iter().map(|p| to(*p)).collect();
            ui.painter().add(egui::Shape::closed_line(q, Stroke::new(1.5, WARN)));
        }
        section(ui, &format!("FLATTENED  {} × {} px", size[0], size[1]));
        if let Some(p) = &self.preview {
            let (pw, ph) = (p.flat.width() as f32, p.flat.height() as f32);
            let w = ui.available_width().min(pw * 3.0);
            let h = (w * ph / pw).min(320.0);
            let w = h * pw / ph;
            let (rect, _) = ui.allocate_exact_size(vec2(w, h), egui::Sense::hover());
            ui.painter().image(p.texture.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, MUTED), egui::StrokeKind::Outside);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new("Read flattened text with AI").color(Color32::WHITE)).fill(Color32::from_rgb(120, 70, 150))).clicked() {
                self.reread_selected();
                self.flash("Reading the flattened text in the background…", &ctx);
            }
        });
        ui.label(RichText::new("The original picture and the shape's own outline are never changed.").color(MUTED).size(11.0));
        if let Some(h) = hm {
            CollapsingHeader::new("Homography (cached, derived from the corners)").show(ui, |ui| {
                for row in 0..3 {
                    ui.label(RichText::new(format!("{:>12.5} {:>12.5} {:>12.5}", h[row * 3], h[row * 3 + 1], h[row * 3 + 2])).monospace().size(10.5));
                }
            });
        }
    }

    fn tab_page(&mut self, ui: &mut Ui) {
        let Some(page) = self.cur().cloned() else { return };
        let Some(entry) = self.store.as_ref().and_then(|s| s.entry(&page.page_id)).cloned() else { return };
        ui.label(RichText::new(&page.page_id).strong());
        ui.label(RichText::new(format!("{} · {} × {} · {}", entry.image, entry.width, entry.height, entry.encoding)).color(MUTED).size(11.5));
        ui.label(RichText::new(format!("sha256 {}", &entry.sha256[..16])).monospace().color(MUTED).size(10.5));

        section(ui, "COMPLETENESS");
        let mut c = page.completeness.clone();
        ui.checkbox(&mut c.inspected, "Inspected for missing text");
        ui.checkbox(&mut c.complete, "Complete (all required text annotated)");
        ui.checkbox(&mut c.no_text, "This page has no text");
        ui.add(TextEdit::singleline(&mut c.note).hint_text("Note").desired_width(f32::INFINITY));
        if let Some(s) = &c.checked {
            ui.label(RichText::new(format!("Checked by {} at {}", s.by, s.at)).color(MUTED).size(11.0));
        }
        if c != page.completeness {
            let user = self.user();
            self.op(move |p, _, _, now| {
                c.checked = Some(Stamp { by: user, at: now.into() });
                p.completeness = c;
            });
        }
        ui.horizontal(|ui| match &page.review.approved_for_export {
            Some(s) => {
                ui.label(RichText::new(format!("Approved for export by {}", s.by)).color(GOOD));
                if ui.small_button("Withdraw").clicked() {
                    self.op(|p, _, _, _| p.review.approved_for_export = None);
                }
            }
            None => {
                if ui.button("Approve page for export").clicked() {
                    self.op(|p, _, user, now| p.review.approved_for_export = Some(Stamp { by: user.into(), at: now.into() }));
                }
            }
        });

        section(ui, "SOURCE AND LICENCE");
        let mut src = entry.source.clone();
        let others: Vec<String> = self.pages().iter().filter(|p| p.page_id != page.page_id).map(|p| p.page_id.clone()).collect();
        egui::Grid::new("source").num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
            for (label, v) in [
                ("Work", &mut src.work),
                ("Series", &mut src.series),
                ("Volume", &mut src.volume),
                ("Chapter", &mut src.chapter),
                ("Page number", &mut src.page_number),
                ("Language", &mut src.language),
                ("Reading direction", &mut src.reading_direction),
                ("Acquired how", &mut src.acquisition),
                ("Image licence", &mut src.license),
                ("Notes", &mut src.notes),
            ] {
                ui.label(RichText::new(label).color(MUTED));
                ui.add(TextEdit::singleline(v).desired_width(190.0));
                ui.end_row();
            }
            ui.label(RichText::new("Restricted").color(MUTED));
            ui.checkbox(&mut src.restricted, "leave out of exports");
            ui.end_row();
            ui.label(RichText::new("Duplicate of").color(MUTED));
            egui::ComboBox::from_id_salt("dup").width(190.0).selected_text(src.duplicate_of.clone().unwrap_or_else(|| "—".into())).show_ui(ui, |ui| {
                ui.selectable_value(&mut src.duplicate_of, None, "—");
                for o in &others {
                    ui.selectable_value(&mut src.duplicate_of, Some(o.clone()), o);
                }
            });
            ui.end_row();
        });
        if src != entry.source {
            if let Some(s) = self.store.as_mut() {
                if let Some(e) = s.manifest.pages.iter_mut().find(|e| e.id == page.page_id) {
                    e.source = src;
                    s.manifest_dirty = true;
                }
            }
        }

        if !page.conflicts.is_empty() {
            section(ui, "CONFLICTS TO SETTLE");
            let mut action: Option<(usize, bool)> = None;
            for (i, c) in page.conflicts.iter().enumerate() {
                egui::Frame::new().fill(PANEL_BG2).corner_radius(5.0).inner_margin(6).show(ui, |ui| {
                    ui.label(RichText::new(&c.note).color(WARN));
                    ui.label(RichText::new(format!("{} · from {}", c.annotation_id, c.source)).color(MUTED).size(10.5));
                    if let Some(t) = &c.theirs {
                        ui.label(format!("Other copy reads: “{}” ({})", t.text(), t.review.state.name()));
                    }
                    ui.horizontal(|ui| {
                        if ui.small_button("Keep this copy").clicked() {
                            action = Some((i, false));
                        }
                        if c.theirs.is_some() && ui.small_button("Take the other copy").clicked() {
                            action = Some((i, true));
                        }
                    });
                });
            }
            if let Some((i, take)) = action {
                self.op(|p, _, _, _| {
                    let c = p.conflicts.remove(i);
                    if take {
                        if let Some(t) = c.theirs {
                            match p.get_mut(&t.id) {
                                Some(a) => *a = t,
                                None => p.annotations.push(t),
                            }
                        }
                    }
                });
            }
        }

        self.structure_tree(ui, 320.0);

        section(ui, "AI RUNS ON THIS PAGE");
        if page.inference_runs.is_empty() {
            ui.label(RichText::new("None yet.").color(MUTED).size(11.5));
        }
        for r in page.inference_runs.iter().rev() {
            let count = |s: PredictionStatus| r.predictions.iter().filter(|p| p.status == s).count();
            let scope = match &r.scope {
                RunScope::Page => "whole page".to_string(),
                RunScope::Region { .. } => "a region".to_string(),
                RunScope::Annotation { transform, .. } => format!("one annotation ({transform})"),
            };
            ui.label(RichText::new(format!("{} {} · {} · {}", r.model.name, r.model.version, scope, r.at)).size(11.5));
            ui.label(
                RichText::new(format!(
                    "{} proposed · {} skipped (overlap) · {} rejected · {} alternatives",
                    count(PredictionStatus::Proposed),
                    count(PredictionStatus::SkippedOverlap),
                    count(PredictionStatus::Rejected),
                    count(PredictionStatus::Alternative)
                ))
                .color(MUTED)
                .size(10.5),
            );
        }

        section(ui, "CHECKS ON THIS PAGE");
        let issues: Vec<Issue> = self.issues.iter().filter(|i| i.page_id == page.page_id).cloned().collect();
        if issues.is_empty() {
            ui.label(RichText::new("No problems found.").color(GOOD));
        }
        for i in issues {
            let col = if i.severity == Severity::Error { BAD } else { WARN };
            let r = ui.add(egui::Label::new(RichText::new(format!("• {}", i.message)).color(col).size(11.5)).sense(egui::Sense::click()));
            if r.clicked() {
                if let Some(id) = &i.annotation_id {
                    self.selection = vec![id.clone()];
                    self.tab = Tab::Text;
                }
            }
        }
    }
}
