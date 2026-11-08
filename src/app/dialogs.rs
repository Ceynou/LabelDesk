use super::*;
use eframe::egui::{RichText, TextEdit};

fn req_picker(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, r: &mut Requirement) {
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(r, Requirement::Required, "Required");
            ui.selectable_value(r, Requirement::Optional, "Optional");
            ui.selectable_value(r, Requirement::Disabled, "Off");
        });
    });
}

impl App {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        self.new_project_dialog(ctx);
        self.settings_dialog(ctx);
        self.release_dialog(ctx);
        self.recovery_dialog(ctx);
        self.conflicts_dialog(ctx);
        self.user_dialog(ctx);
        self.validation_dialog(ctx);
        self.shortcuts_dialog(ctx);
        self.reject_dialog(ctx);
        self.note_dialog(ctx);
        self.close_dialog(ctx);
        self.progress_dialog(ctx);
        self.find_dialog(ctx);
    }

    fn new_project_dialog(&mut self, ctx: &egui::Context) {
        let Some((mut name, mut parent, mut chars)) = self.dialogs.new_project.clone() else { return };
        let mut open = true;
        let mut create = false;
        egui::Window::new("New project").open(&mut open).collapsible(false).resizable(false).default_width(460.0).show(ctx, |ui| {
            egui::Grid::new("np").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                ui.label("Name");
                ui.add(TextEdit::singleline(&mut name).desired_width(300.0));
                ui.end_row();
                ui.label("Folder");
                ui.horizontal(|ui| {
                    ui.label(RichText::new(parent.join(&name).display().to_string()).monospace().size(11.0));
                    if ui.small_button("Change…").clicked() {
                        if let Some(d) = rfd::FileDialog::new().set_title("Where to create the project folder").pick_folder() {
                            parent = d;
                        }
                    }
                });
                ui.end_row();
                ui.label("Characters");
                req_picker(ui, "chars", &mut chars);
                ui.end_row();
            });
            ui.label(RichText::new("Lines are required; panels, bubbles and regions optional; words off. Everything can be changed later in Project settings, including per text class (e.g. characters only on signs).").color(MUTED).size(11.5));
            ui.add_space(6.0);
            if ui.add(egui::Button::new(RichText::new("Create project").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                create = true;
            }
        });
        if create {
            let mut cfg = ProjectConfig::default();
            cfg.levels.insert(Level::Char, chars);
            let dir = parent.join(&name);
            match Store::create(&dir, &name, cfg, &self.user()) {
                Ok(s) => {
                    self.set_store(s);
                    self.dialogs.new_project = None;
                    self.flash(format!("Project created in {}. Import pictures to begin.", dir.display()), ctx);
                }
                Err(e) => self.error(e),
            }
            return;
        }
        self.dialogs.new_project = if open { Some((name, parent, chars)) } else { None };
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut cfg) = self.dialogs.settings.clone() else { return };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Project settings").open(&mut open).default_width(560.0).default_height(620.0).vscroll(true).show(ctx, |ui| {
            ui.heading("Annotation levels");
            egui::Grid::new("levels").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                for l in Level::ALL {
                    ui.label(l.name());
                    let mut r = cfg.levels.get(&l).copied().unwrap_or(Requirement::Optional);
                    req_picker(ui, l, &mut r);
                    cfg.levels.insert(l, r);
                    ui.end_row();
                }
            });
            ui.label(RichText::new("Per text class").strong());
            let mut drop = None;
            for (i, o) in cfg.level_overrides.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let classes: Vec<(String, String)> = cfg.text_classes.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
                    egui::ComboBox::from_id_salt(("oc", i)).width(130.0).selected_text(&o.class).show_ui(ui, |ui| {
                        for (id, name) in &classes {
                            ui.selectable_value(&mut o.class, id.clone(), name);
                        }
                    });
                    egui::ComboBox::from_id_salt(("ol", i)).width(110.0).selected_text(o.level.name()).show_ui(ui, |ui| {
                        for l in Level::DRAWABLE {
                            ui.selectable_value(&mut o.level, l, l.name());
                        }
                    });
                    req_picker(ui, ("or", i), &mut o.requirement);
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                cfg.level_overrides.remove(i);
            }
            if ui.small_button("+ Rule for a class").clicked() {
                cfg.level_overrides.push(LevelOverride { class: "signage".into(), level: Level::Char, requirement: Requirement::Required });
            }

            ui.separator();
            ui.heading("Review policy");
            let p = &mut cfg.review;
            ui.horizontal(|ui| {
                ui.label("Reviewers needed");
                ui.add(egui::DragValue::new(&mut p.reviewers_required).range(1..=5));
            });
            ui.checkbox(&mut p.independent_reviewers, "Reviewers must be different people from the annotator");
            ui.checkbox(&mut p.allow_self_approval, "Annotators may approve their own work");
            ui.horizontal(|ui| {
                ui.label("Aspects approved separately:");
                for d in Dimension::ALL {
                    let mut on = p.required_dimensions.contains(&d);
                    if ui.checkbox(&mut on, d.name()).changed() {
                        if on { p.required_dimensions.push(d) } else { p.required_dimensions.retain(|x| *x != d) }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Disagreements to adjudicate:");
                for d in Dimension::ALL {
                    let mut on = p.adjudicate_on.contains(&d);
                    if ui.checkbox(&mut on, d.name()).changed() {
                        if on { p.adjudicate_on.push(d) } else { p.adjudicate_on.retain(|x| *x != d) }
                    }
                }
            });
            ui.checkbox(&mut p.gold_requires_page_complete, "Gold needs the page marked inspected and complete");
            ui.horizontal(|ui| {
                ui.label("Editing approved data");
                ui.selectable_value(&mut p.on_edit_after_approval, EditPolicy::Invalidate, "withdraws the approval");
                ui.selectable_value(&mut p.on_edit_after_approval, EditPolicy::MarkForRereview, "flags it for re-review");
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("States used:");
                for s in ReviewState::ALL {
                    let mut on = p.enabled_states.contains(&s);
                    let fixed = matches!(s, ReviewState::Draft);
                    if ui.add_enabled(!fixed, egui::Checkbox::new(&mut on, s.name())).changed() {
                        if on { p.enabled_states.push(s) } else { p.enabled_states.retain(|x| *x != s) }
                    }
                }
            });
            ui.label(RichText::new(format!("Policy version {}. Saving changes makes a new version; earlier approvals keep the version they were given under.", p.version)).color(MUTED).size(11.0));

            ui.separator();
            ui.heading("Text classes and defaults");
            let mut drop = None;
            for (i, c) in cfg.text_classes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgb(&mut c.color);
                    ui.add(TextEdit::singleline(&mut c.name).desired_width(150.0));
                    ui.label(RichText::new(&c.id).monospace().color(MUTED).size(11.0));
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                cfg.text_classes.remove(i);
            }
            let missing: Vec<_> = PUBLISHING_CLASSES.iter().filter(|(id, _, _)| !cfg.text_classes.iter().any(|c| c.id == *id)).collect();
            if !missing.is_empty() && ui.small_button("+ Publishing classes (title, credits, page number)").on_hover_text("For text about the book rather than in the story: titles, author, publisher, edition, copyright, translated titles, page numbers").clicked() {
                for (id, name, color) in missing {
                    let at = cfg.text_classes.iter().position(|c| c.id == "other").unwrap_or(cfg.text_classes.len());
                    cfg.text_classes.insert(at, ClassDef { id: id.to_string(), name: name.to_string(), color: *color });
                }
            }
            if ui.small_button("+ Class").clicked() {
                let n = cfg.text_classes.len();
                cfg.text_classes.push(ClassDef { id: format!("class_{n}"), name: format!("Class {n}"), color: [180, 180, 190] });
            }
            let retired: usize = self.pages().iter().map(crate::assist::retired_labels).sum();
            let old_types = cfg.text_classes.iter().any(|c| RETIRED_CLASSES.iter().any(|r| r.0 == c.id));
            if retired > 0 || old_types {
                ui.label(RichText::new("Handwritten is now an attribute; Decorative became Background text and Screen / interface became Secondary text.").color(WARN).size(11.5));
                if ui.button(format!("Update to the new text types ({retired} label(s))")).on_hover_text("Saved with the settings; one undo step per page").clicked() {
                    crate::assist::migrate_config(&mut cfg);
                    self.dialogs.migrate_types = true;
                }
            }
            ui.label(RichText::new("Attributes (optional, any number per label)").strong());
            let mut drop = None;
            for (i, at) in cfg.attributes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(at).desired_width(150.0));
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                cfg.attributes.remove(i);
            }
            if ui.small_button("+ Attribute").clicked() {
                cfg.attributes.push(format!("attribute_{}", cfg.attributes.len()));
            }
            ui.label(RichText::new("Rotation and mirroring are part of the orientation, not attributes. “furigana” and “monospace” change how boxes are ordered and split.").color(MUTED).size(11.0));
            egui::Grid::new("defaults").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.label("Default language");
                ui.add(TextEdit::singleline(&mut cfg.default_language).desired_width(80.0));
                ui.end_row();
                ui.label("Default script");
                ui.add(TextEdit::singleline(&mut cfg.default_script).desired_width(80.0));
                ui.end_row();
                ui.label("Writing mode");
                egui::ComboBox::from_id_salt("dwm").selected_text(cfg.default_writing_mode.name()).show_ui(ui, |ui| {
                    for m in WritingMode::ALL {
                        ui.selectable_value(&mut cfg.default_writing_mode, m, m.name());
                    }
                });
                ui.end_row();
                ui.label("Annotation licence");
                ui.add(TextEdit::singleline(&mut cfg.annotation_license).hint_text("e.g. CC-BY-4.0").desired_width(160.0));
                ui.end_row();
            });

            ui.separator();
            ui.heading("Project fields");
            ui.label(RichText::new("Extra fields saved under a namespace, e.g. studio:speaker.").color(MUTED).size(11.5));
            let mut drop = None;
            for (i, f) in cfg.custom_fields.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut f.key).desired_width(120.0));
                    ui.add(TextEdit::singleline(&mut f.label).desired_width(110.0));
                    egui::ComboBox::from_id_salt(("fk", i)).width(80.0).selected_text(format!("{:?}", f.kind)).show_ui(ui, |ui| {
                        for k in [FieldKind::Text, FieldKind::Number, FieldKind::Bool, FieldKind::Choice] {
                            ui.selectable_value(&mut f.kind, k, format!("{k:?}"));
                        }
                    });
                    ui.checkbox(&mut f.required, "required");
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
                if f.kind == FieldKind::Choice {
                    let mut s = f.choices.join(", ");
                    if ui.add(TextEdit::singleline(&mut s).hint_text("choices, comma separated").desired_width(400.0)).changed() {
                        f.choices = s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                    }
                }
            }
            if let Some(i) = drop {
                cfg.custom_fields.remove(i);
            }
            if ui.small_button("+ Field").clicked() {
                cfg.custom_fields.push(FieldDef { key: "project:field".into(), label: "Field".into(), kind: FieldKind::Text, choices: vec![], required: false, default: None, min: None, max: None, levels: vec![] });
            }

            ui.separator();
            App::reading_settings(ui, &mut cfg);
            ui.separator();
            ui.heading("Model");
            ui.checkbox(&mut cfg.model.enabled, "Use AI pre-annotation in this project");
            ui.checkbox(&mut cfg.model.auto_run_on_import, "Run it automatically on imported pages");
            match &cfg.model.pinned {
                Some(m) => {
                    ui.label(RichText::new(format!("Pinned: {} {} (revision {})", m.name, m.version, &m.revision[..10.min(m.revision.len())])).monospace().size(11.0));
                    for a in &m.artifacts {
                        ui.label(RichText::new(format!("{} {} sha256 {}…", a.role, a.file, &a.sha256[..12])).monospace().color(MUTED).size(10.0));
                    }
                }
                None => {
                    ui.label(RichText::new("Not pinned yet: the newest cached version (v0.2 or higher) is pinned on first use.").color(MUTED));
                }
            }
            let cands = crate::ocr::find_candidates();
            ui.horizontal_wrapped(|ui| {
                ui.label("Cached versions:");
                if cands.is_empty() {
                    ui.label(RichText::new("none found in the Hugging Face cache").color(WARN));
                }
                for c in &cands {
                    let pinned = cfg.model.pinned.as_ref().is_some_and(|m| m.version == c.version_str() && m.revision == c.revision);
                    if ui.add_enabled(!pinned, egui::Button::new(format!("Use {}", c.version_str()))).on_hover_text("Switch explicitly; existing labels are not changed").clicked() {
                        if let Ok(m) = crate::ocr::model_ref(c) {
                            cfg.model.pinned = Some(m);
                        }
                    }
                }
            });
            ui.add_enabled(false, egui::Checkbox::new(&mut cfg.model.character_model_enabled, "Character-level model (muscgab/manga-ocr-charspot-small): planned"));

            ui.separator();
            ui.heading("Validation");
            egui::Grid::new("val").num_columns(2).spacing([10.0, 2.0]).show(ui, |ui| {
                for (rule, _, desc) in validate::RULES {
                    ui.label(RichText::new(desc).size(11.5));
                    let mut s = validate::severity(&cfg, rule);
                    ui.push_id(rule, |ui| {
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut s, Severity::Error, "Blocks export");
                            ui.selectable_value(&mut s, Severity::Warning, "Warn");
                            ui.selectable_value(&mut s, Severity::Off, "Off");
                        });
                    });
                    if s != validate::severity(&cfg, rule) {
                        cfg.validation.insert(rule.to_string(), s);
                    }
                    ui.end_row();
                }
            });
            ui.separator();
            ui.heading("Exports");
            ui.checkbox(&mut cfg.export.exclude_restricted, "Leave restricted pages out of releases");
            ui.horizontal(|ui| {
                ui.label("Split train / validation / test");
                for k in 0..3 {
                    ui.add(egui::DragValue::new(&mut cfg.export.split[k]).range(0.0..=1.0).speed(0.01));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Split seed");
                ui.add(TextEdit::singleline(&mut cfg.export.split_seed).desired_width(120.0));
            });
            ui.add_space(8.0);
            if ui.add(egui::Button::new(RichText::new("Save settings").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                save = true;
            }
        });
        if save {
            if let Some(s) = self.store.as_mut() {
                if cfg.review != s.manifest.config.review {
                    cfg.review.version = s.manifest.config.review.version + 1;
                }
                s.manifest.config = cfg;
                s.manifest_dirty = true;
            }
            if std::mem::take(&mut self.dialogs.migrate_types) {
                let n = self.pages().len();
                let mut changed = 0;
                for i in 0..n {
                    changed += self.op_on(i, |p, _, _, _| crate::assist::migrate_classes(p)).unwrap_or(0);
                }
                self.flash(format!("{changed} label(s) moved to the new text types"), ctx);
            }
            self.issues_dirty = true;
            self.dialogs.settings = None;
            self.save(ctx, true);
            self.flash("Project settings saved", ctx);
            return;
        }
        self.dialogs.settings = if open { Some(cfg) } else { None };
    }

    fn release_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut opt) = self.dialogs.release.clone() else { return };
        let mut open = true;
        let mut go = false;
        egui::Window::new("Create dataset release").open(&mut open).collapsible(false).default_width(440.0).show(ctx, |ui| {
            ui.label(RichText::new("A release is a reproducible snapshot in the project's releases folder, with checksums, splits and provenance. Your annotation files are not changed.").color(MUTED));
            ui.add_space(6.0);
            ui.checkbox(&mut opt.gold_only, "Gold only (gold annotations on pages approved for export)");
            ui.checkbox(&mut opt.exclude_restricted, "Leave out restricted pages");
            ui.label(RichText::new("Formats").strong());
            ui.checkbox(&mut opt.native, "Native JSON (full fidelity)");
            ui.checkbox(&mut opt.jsonl, "JSONL for training");
            ui.checkbox(&mut opt.coco, "COCO JSON (regions and outlines)");
            ui.checkbox(&mut opt.crops, "Cropped and flattened text images");
            ui.add(TextEdit::singleline(&mut opt.note).hint_text("Note for this release").desired_width(f32::INFINITY));
            let errors = self.issues.iter().filter(|i| i.severity == Severity::Error).count();
            if errors > 0 {
                ui.label(RichText::new(format!("{errors} problem(s) currently block exports. File → Check the whole project lists them.")).color(BAD));
            }
            if ui.add(egui::Button::new(RichText::new("Create release").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                go = true;
            }
        });
        if go {
            self.save(ctx, true);
            let user = self.user();
            if let Some(s) = &self.store {
                match crate::export::release(s, &opt, &user) {
                    Ok(r) => {
                        self.flash(format!("Release {} written: {} pages, {} files ({})", r.id, r.pages, r.files, r.dir.display()), ctx);
                        self.dialogs.release = None;
                        return;
                    }
                    Err(e) => self.error(e),
                }
            }
        }
        self.dialogs.release = if open { Some(opt) } else { None };
    }

    fn recovery_dialog(&mut self, ctx: &egui::Context) {
        if self.dialogs.recovery.is_empty() {
            return;
        }
        let mut decide: Option<(usize, bool)> = None;
        egui::Modal::new(egui::Id::unique("recovery")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Recover unsaved work?");
            ui.label("LabelDesk was closed before these pages were saved. Their last changes were kept:");
            for (i, r) in self.dialogs.recovery.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} · {} annotations · {}", r.page_id, r.page.annotations.len(), r.when)).size(12.0));
                    if ui.button("Recover").clicked() {
                        decide = Some((i, true));
                    }
                    if ui.button("Discard").clicked() {
                        decide = Some((i, false));
                    }
                });
            }
        });
        if let Some((i, keep)) = decide {
            let r = self.dialogs.recovery.remove(i);
            if let Some(s) = self.store.as_mut() {
                if keep {
                    if let Some(k) = s.index_of(&r.page_id) {
                        s.pages[k] = r.page;
                        s.dirty.insert(r.page_id.clone());
                    }
                } else {
                    s.discard_recovery(&r.page_id);
                }
            }
            self.rebaseline();
            self.issues_dirty = true;
            if keep {
                self.save(ctx, true);
            }
        }
    }

    fn conflicts_dialog(&mut self, ctx: &egui::Context) {
        if self.dialogs.conflicts.is_empty() {
            return;
        }
        let mut decide: Option<(usize, u8)> = None;
        egui::Modal::new(egui::Id::unique("conflicts")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("A page changed on disk");
            ui.label("Another program (for example git pull) changed these files since you opened them. Nothing was overwritten.");
            for (i, (id, theirs)) in self.dialogs.conflicts.iter().enumerate() {
                ui.label(RichText::new(format!("{id}: the other version has {} annotations", theirs.annotations.len())).strong());
                ui.horizontal(|ui| {
                    if ui.button("Merge (recommended)").on_hover_text("Combine both; where both changed the same annotation you settle it on the Page tab").clicked() {
                        decide = Some((i, 0));
                    }
                    if ui.button("Use the version on disk").clicked() {
                        decide = Some((i, 1));
                    }
                    if ui.button("Keep mine").clicked() {
                        decide = Some((i, 2));
                    }
                });
            }
        });
        if let Some((i, how)) = decide {
            let (id, theirs) = self.dialogs.conflicts.remove(i);
            if let Some(s) = self.store.as_mut() {
                if let Some(k) = s.index_of(&id) {
                    let base = s.saved_version(&id).cloned();
                    let new = match how {
                        0 => crate::store::merge_page(base.as_ref(), &s.pages[k], &theirs, "file changed on disk").0,
                        1 => (*theirs).clone(),
                        _ => s.pages[k].clone(),
                    };
                    s.adopt_disk_version(&id, &theirs);
                    s.pages[k] = new;
                }
            }
            self.rebaseline();
            self.issues_dirty = true;
            self.save(ctx, true);
        }
    }

    fn user_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut u) = self.dialogs.user.clone() else { return };
        let mut open = true;
        let mut ok = false;
        egui::Window::new("Who is annotating?").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label("This short name is written into every change and approval you make.");
            ui.add(TextEdit::singleline(&mut u).desired_width(220.0));
            if ui.button("Use this name").clicked() {
                ok = true;
            }
        });
        if ok && !u.trim().is_empty() {
            self.commit();
            self.settings.user = util::slug(u.trim());
            self.settings.save();
            self.dialogs.user = None;
            return;
        }
        self.dialogs.user = if open { Some(u) } else { None };
    }

    fn validation_dialog(&mut self, ctx: &egui::Context) {
        if !self.dialogs.validation {
            return;
        }
        let mut open = true;
        let mut goto: Option<(String, Option<String>)> = None;
        let assets = self.store.as_ref().map(|s| s.verify_images()).unwrap_or_default();
        egui::Window::new("Project check").open(&mut open).default_width(560.0).default_height(480.0).vscroll(true).show(ctx, |ui| {
            let e = self.issues.iter().filter(|i| i.severity == Severity::Error).count();
            let w = self.issues.len() - e;
            ui.label(RichText::new(format!("{e} error(s) that block exports, {w} warning(s)")).strong());
            for (pid, msg) in &assets {
                ui.label(RichText::new(format!("{pid}: {msg}")).color(BAD));
            }
            for i in &self.issues {
                let col = if i.severity == Severity::Error { BAD } else { WARN };
                let r = ui.add(egui::Label::new(RichText::new(format!("{} · {}", i.page_id, i.message)).color(col).size(12.0)).sense(egui::Sense::click()));
                if r.on_hover_text(i.rule).clicked() {
                    goto = Some((i.page_id.clone(), i.annotation_id.clone()));
                }
            }
        });
        if let Some((pid, aid)) = goto {
            if let Some(i) = self.store.as_ref().and_then(|s| s.index_of(&pid)) {
                self.go_to(i);
                if let Some(a) = aid {
                    self.selection = vec![a];
                }
            }
        }
        self.dialogs.validation = open;
    }

    fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        let Some(capture) = self.dialogs.shortcuts.clone() else { return };
        let mut open = true;
        let mut new_capture = capture.clone();
        if let Some(action) = &capture {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((k, m)) = pressed {
                if k == Key::Escape {
                    new_capture = None;
                } else {
                    let mut s = String::new();
                    if m.command {
                        s.push_str("Ctrl+");
                    }
                    if m.shift {
                        s.push_str("Shift+");
                    }
                    if m.alt {
                        s.push_str("Alt+");
                    }
                    s.push_str(k.name());
                    self.settings.shortcuts.insert(action.clone(), s);
                    self.settings.save();
                    new_capture = None;
                }
            }
        }
        egui::Window::new("Keyboard shortcuts").open(&mut open).default_width(420.0).vscroll(true).show(ctx, |ui| {
            ui.label(RichText::new("Click a shortcut, then press the new key. 1–6 pick the level to draw; Space+drag moves the view; Esc cancels.").color(MUTED).size(11.5));
            egui::Grid::new("keys").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                for (id, name, _) in ACTIONS {
                    ui.label(name);
                    let label = if capture.as_deref() == Some(id) { "press a key…".to_string() } else { self.settings.shortcut(id) };
                    if ui.button(RichText::new(label).monospace()).clicked() {
                        new_capture = Some(id.to_string());
                    }
                    ui.end_row();
                }
            });
            if ui.button("Restore defaults").clicked() {
                self.settings.shortcuts.clear();
                self.settings.save();
            }
        });
        self.dialogs.shortcuts = if open { Some(new_capture) } else { None };
    }

    fn reject_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, mut reason)) = self.dialogs.reject.clone() else { return };
        let mut open = true;
        let mut ok = false;
        egui::Window::new("Reject AI proposal").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label("The proposal disappears from the page; the model's original output stays recorded as rejected.");
            ui.horizontal_wrapped(|ui| {
                for r in ["not text", "duplicate", "wrong area", "furigana", "decoration"] {
                    if ui.small_button(r).clicked() {
                        reason = r.into();
                    }
                }
            });
            ui.add(TextEdit::singleline(&mut reason).desired_width(260.0));
            if ui.button("Reject").clicked() {
                ok = true;
            }
        });
        if ok {
            if !self.selection.contains(&id) {
                self.selection = vec![id];
            }
            self.reject_ai(&reason);
            self.dialogs.reject = None;
            return;
        }
        self.dialogs.reject = if open { Some((id, reason)) } else { None };
    }

    fn note_dialog(&mut self, ctx: &egui::Context) {
        let Some((id, kind, mut dim, mut text)) = self.dialogs.note.clone() else { return };
        let mut open = true;
        let mut ok = false;
        let title = match kind {
            NoteKind::ChangeRequest => "Request changes",
            NoteKind::Disagreement => "Record a disagreement",
            _ => "Comment",
        };
        egui::Window::new(title).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            if kind == NoteKind::Disagreement {
                ui.horizontal(|ui| {
                    for d in Dimension::ALL {
                        ui.selectable_value(&mut dim, Some(d), d.name());
                    }
                });
            }
            ui.add(TextEdit::multiline(&mut text).desired_rows(3).desired_width(320.0).hint_text("What and why"));
            if ui.button("Save").clicked() && !text.trim().is_empty() {
                ok = true;
            }
        });
        if ok {
            let t = text.clone();
            self.op(move |p, _, user, now| {
                if let Some(a) = p.get_mut(&id) {
                    review::add_note(a, kind, dim, &t, user, now);
                }
            });
            self.dialogs.note = None;
            return;
        }
        self.dialogs.note = if open { Some((id, kind, dim, text)) } else { None };
    }

    fn close_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_close {
            return;
        }
        egui::Modal::new(egui::Id::unique("close")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Some changes could not be saved");
            ui.label("Check the message at the bottom of the window. Your work is also kept as a recovery copy.");
            ui.horizontal(|ui| {
                if ui.button("Close anyway").clicked() {
                    self.allow_close = true;
                    self.confirm_close = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.button("Stay").clicked() {
                    self.confirm_close = false;
                }
            });
        });
    }
}
