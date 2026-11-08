use super::*;
use crate::icons::{self, Icon};
use eframe::egui::{Align, Align2, FontId, Layout, RichText, Sense, pos2};

pub fn documents_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let docs = home.join("Documents");
    if docs.is_dir() { docs } else { home }
}

impl App {
    pub fn open_sample(&mut self, ctx: &egui::Context) {
        let base = documents_dir().join("LabelDesk");
        let mut dir = base.join("Comic OCR sample");
        let mut k = 2;
        while dir.exists() {
            dir = base.join(format!("Comic OCR sample {k}"));
            k += 1;
        }
        match crate::sample::create_project(&dir, &self.user()) {
            Ok(s) => {
                self.set_store(s);
                self.flash(format!("Sample project created in {}", dir.display()), ctx);
            }
            Err(e) => self.error(format!("Could not create the sample: {e}")),
        }
    }

    pub fn import_pictures(&mut self, ctx: &egui::Context, folder: bool) {
        let files: Vec<PathBuf> = if folder {
            let Some(d) = rfd::FileDialog::new().set_title("Import every picture in a folder").pick_folder() else { return };
            let mut v: Vec<PathBuf> = std::fs::read_dir(d)
                .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| crate::store::IMAGE_EXTS.contains(&e.to_string_lossy().to_lowercase().as_str()))).collect())
                .unwrap_or_default();
            v.sort();
            v
        } else {
            rfd::FileDialog::new().set_title("Import pictures").add_filter("Pictures", &crate::store::IMAGE_EXTS).pick_files().unwrap_or_default()
        };
        if files.is_empty() {
            return;
        }
        self.commit();
        let user = self.user();
        let Some(s) = self.store.as_mut() else { return };
        let before = s.pages.len();
        let (ids, notes) = s.import_images(&files, true, &user);
        let auto = s.manifest.config.model.auto_run_on_import && s.manifest.config.model.enabled;
        self.issues_dirty = true;
        let mut msg = format!("Imported {} picture(s) into the project's images folder", ids.len());
        if !notes.is_empty() {
            msg.push_str(&format!("; {} skipped ({})", notes.len(), notes[0]));
        }
        self.flash(msg, ctx);
        if auto {
            for i in before..before + ids.len() {
                self.ai_job(i, ai::JobKind::Page);
            }
        }
        if before == 0 {
            self.page = 0;
            self.rebaseline();
        }
    }

    pub fn menu_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let has = self.store.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            let c = r.center();
            ui.painter().add(egui::Shape::convex_polygon(vec![c + vec2(0.0, -8.0), c + vec2(8.0, 0.0), c + vec2(0.0, 8.0), c + vec2(-8.0, 0.0)], ACCENT, Stroke::NONE));
            ui.painter().rect_stroke(Rect::from_center_size(c, Vec2::splat(6.0)), 1.0, Stroke::new(1.5, Color32::WHITE), egui::StrokeKind::Middle);
            ui.label(RichText::new("LabelDesk").strong().color(Color32::WHITE).size(15.0));
            ui.label(RichText::new("comic OCR").color(MUTED).size(12.0));
            ui.add_space(6.0);
            ui.menu_button("File", |ui| {
                if ui.button("New project…").clicked() {
                    self.dialogs.new_project = Some(("My comic OCR dataset".into(), documents_dir().join("LabelDesk"), Requirement::Optional));
                    ui.close();
                }
                if ui.button("Open project…").clicked() {
                    ui.close();
                    if let Some(d) = rfd::FileDialog::new().set_title("Open a project folder (contains project.json)").pick_folder() {
                        self.commit();
                        if self.is_dirty() {
                            self.save(&ctx, true);
                        }
                        self.open_project(&d);
                    }
                }
                ui.menu_button("Recent projects", |ui| {
                    for p in self.settings.recent.clone() {
                        if ui.button(p.display().to_string()).clicked() {
                            self.save(&ctx, true);
                            self.open_project(&p);
                            ui.close();
                        }
                    }
                });
                if ui.button("Open the sample project").clicked() {
                    ui.close();
                    self.save(&ctx, true);
                    self.open_sample(&ctx);
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("Import pictures…")).clicked() {
                    ui.close();
                    self.import_pictures(&ctx, false);
                }
                if ui.add_enabled(has, egui::Button::new("Import a folder of pictures…")).clicked() {
                    ui.close();
                    self.import_pictures(&ctx, true);
                }
                if ui.add_enabled(has, egui::Button::new("Import annotations from another copy (merge)…")).clicked() {
                    ui.close();
                    if let Some(d) = rfd::FileDialog::new().set_title("Pick the other project folder or a release's native folder").pick_folder() {
                        self.commit();
                        if let Some(s) = self.store.as_mut() {
                            match crate::export::import_annotations(s, &d) {
                                Ok(m) => self.flash(m, &ctx),
                                Err(e) => self.error(e),
                            }
                        }
                        self.issues_dirty = true;
                        self.rebaseline();
                    }
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(format!("Save   {}", self.settings.shortcut("save")))).clicked() {
                    self.save(&ctx, false);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Create dataset release…")).clicked() {
                    self.save(&ctx, true);
                    self.dialogs.release = Some(Default::default());
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Check the whole project…")).clicked() {
                    self.issues_dirty = true;
                    self.dialogs.validation = true;
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Project settings…")).clicked() {
                    self.dialogs.settings = Some(self.cfg());
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("Close project")).clicked() {
                    self.save(&ctx, true);
                    if !self.is_dirty() {
                        if let Some(s) = self.store.take() {
                            s.unlock();
                        }
                        self.dialogs.welcome = true;
                    }
                    ui.close();
                }
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new(format!("Undo   {}", self.settings.shortcut("undo")))).clicked() {
                    self.undo(false);
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new(format!("Redo   {}", self.settings.shortcut("redo")))).clicked() {
                    self.undo(true);
                }
                ui.separator();
                if ui.button("Select all   Ctrl+A").clicked() {
                    self.selection = self.cur().map(|p| p.annotations.iter().map(|a| a.id.clone()).collect()).unwrap_or_default();
                    ui.close();
                }
                let n = self.selection.len();
                if ui.add_enabled(n == 1, egui::Button::new("Split   S")).clicked() {
                    self.split_selection(&ctx);
                    ui.close();
                }
                if ui.add_enabled(n >= 2, egui::Button::new("Merge   M")).clicked() {
                    self.merge_selection(&ctx);
                    ui.close();
                }
                if ui.add_enabled(n > 0, egui::Button::new("Delete   Del")).clicked() {
                    self.delete_selection();
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(n > 0, egui::Button::new(format!("Tighten to the text   {}", self.settings.shortcut("tighten")))).clicked() {
                    self.tighten_selection(&ctx);
                    ui.close();
                }
                if ui.add_enabled(n == 1, egui::Button::new(format!("Propose / accept character boxes   {}", self.settings.shortcut("propose_chars")))).clicked() {
                    self.propose_or_accept_chars(&ctx);
                    ui.close();
                }
                if ui.button(format!("Group the selection (or nearby lines) into a parent   {}", self.settings.shortcut("make_regions"))).clicked() {
                    self.make_regions(&ctx);
                    ui.close();
                }
                if ui.button("Convert rectangular quadrilaterals to boxes (page)").clicked() {
                    self.convert_quads(&ctx, true);
                    ui.close();
                }
                if ui.button(format!("Straighten nearly level boxes (page, up to {:.1}°)", self.cfg().quick.straighten_max_deg)).clicked() {
                    self.straighten_selection(&ctx, true);
                    ui.close();
                }
                ui.menu_button("Convert selection to", |ui| {
                    for (kind, name) in [("box", "Box"), ("quad", "Quadrilateral"), ("polygon", "Polygon")] {
                        if ui.button(name).clicked() {
                            self.convert_selection(&ctx, kind);
                            ui.close();
                        }
                    }
                });
                ui.separator();
                if ui.button(format!("Find labels…   {}", self.settings.shortcut("find"))).clicked() {
                    self.find.open = true;
                    if self.find.conds.is_empty() {
                        self.find.conds.push(super::find::Cond { crit: super::find::Crit::Text(String::new()), not: false });
                    }
                    ui.close();
                }
                if ui.add_enabled(n > 0, egui::Button::new(format!("Hide the selection   {}", self.settings.shortcut("hide")))).clicked() {
                    self.hide_selection(&ctx);
                    ui.close();
                }
                if ui.add_enabled(!self.hidden.is_empty(), egui::Button::new(format!("Show {} hidden label(s)   {}", self.hidden.len(), self.settings.shortcut("show_hidden")))).clicked() {
                    self.hidden.clear();
                    ui.close();
                }
                ui.separator();
                if ui.button("Keyboard shortcuts…").clicked() {
                    self.dialogs.shortcuts = Some(None);
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_problems, "Highlight problems (red)").on_hover_text("Labels with validation problems get a red outline; F8 jumps to the next one");
                ui.checkbox(&mut self.show_text_mask, "Text mask").on_hover_text("The text pixels Koharu text masks found on this page (AI → Mark the text pixels), in magenta");
                if ui.button(format!("Next problem   {}", self.settings.shortcut("next_problem"))).clicked() {
                    self.next_problem(&ctx);
                    ui.close();
                }
                ui.separator();
                ui.label(RichText::new("Show levels").color(MUTED).size(11.0));
                for l in Level::DRAWABLE {
                    let mut on = self.layers[&l];
                    if ui.checkbox(&mut on, l.name()).changed() {
                        self.layers.insert(l, on);
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.show_order, "Reading order numbers");
                ui.label(RichText::new("Name tags").color(MUTED).size(11.0));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.labels, ShowLabels::Auto, "Where they fit");
                    ui.selectable_value(&mut self.labels, ShowLabels::All, "All");
                    ui.selectable_value(&mut self.labels, ShowLabels::Selected, "Selection only");
                });
                ui.label(RichText::new("Character boxes").color(MUTED).size(11.0));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.chars, ShowChars::Auto, "When zoomed in");
                    ui.selectable_value(&mut self.chars, ShowChars::Always, "Always");
                    ui.selectable_value(&mut self.chars, ShowChars::SelectedLine, "Selected line only");
                });
                ui.checkbox(&mut self.edge_handles, "Resize quadrilaterals and polygons by their edges");
                ui.horizontal(|ui| {
                    ui.label("Fill");
                    ui.add(egui::Slider::new(&mut self.fill, 0.0..=0.6).show_value(false));
                });
                if ui.button(format!("Fit page   {}", self.settings.shortcut("fit"))).clicked() {
                    self.view.fitted = true;
                    ui.close();
                }
            });
            ui.menu_button("AI", |ui| {
                ui.set_min_width(300.0);
                match &self.model {
                    ModelStatus::Ready(m) => ui.label(RichText::new(format!("{} {} · rev {}", m.name, m.version, &m.revision[..8.min(m.revision.len())])).color(GOOD)),
                    ModelStatus::Missing(e) => ui.label(RichText::new(e).color(BAD)),
                    ModelStatus::Unknown => ui.label(RichText::new("Model loads from the local cache on first use").color(MUTED)),
                };
                if let (Some(pinned), Some(newest)) = (self.cfg().model.pinned.clone(), crate::ocr::find_candidates().into_iter().next()) {
                    if newest.version_str() != pinned.version {
                        ui.label(RichText::new(format!("{} is available (this project uses {}). Switch in Project settings → Model; existing labels stay as they are.", newest.version_str(), pinned.version)).color(WARN).size(11.5));
                    }
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new(format!("Pre-annotate this page   {}", self.settings.shortcut("ai_page")))).clicked() {
                    let p = self.page;
                    self.ai_job(p, ai::JobKind::Page);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Pre-annotate all pages not yet run")).clicked() {
                    self.ai_all_pages(&ctx);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new(format!("Read a region…   {}", self.settings.shortcut("tool_ai_region")))).clicked() {
                    self.set_tool(Tool::AiRegion);
                    ui.close();
                }
                if ui.add_enabled(!self.selection.is_empty(), egui::Button::new(format!("Read the selection again   {}", self.settings.shortcut("reread")))).on_hover_text(format!("With {} (choose in AI → Models…)", self.cfg().model.reader.name())).clicked() {
                    let ids = self.selection.clone();
                    self.reread(&ids);
                    ui.close();
                }
                ui.separator();
                ui.label(RichText::new("More models").color(MUTED).size(11.0));
                let o = self.settings.models.clone();
                let whys: BTreeMap<crate::zoo::ZooId, Option<String>> = crate::zoo::ZooId::ALL.iter().map(|&z| (z, self.models_ui.ready(z, &o).err())).collect();
                let item = move |ui: &mut egui::Ui, text: &str, id: crate::zoo::ZooId, enabled: bool| {
                    let why = whys.get(&id).cloned().flatten();
                    let r = ui.add_enabled(enabled && why.is_none(), egui::Button::new(text));
                    match why {
                        Some(w) => r.on_disabled_hover_text(format!("{}: {w}", id.name())),
                        None => r.on_hover_text(format!("{} · {}", id.name(), o.backend(id).name())),
                    }
                };
                let n = self.pages().len();
                if item(ui, "Find text blocks on this page (AnimeText)", crate::zoo::ZooId::AnimeText, has).clicked() {
                    let p = self.page;
                    self.ai_zoo_pages(&[p], ai::JobKind::Blocks, &ctx);
                    ui.close();
                }
                if item(ui, "Find text blocks on all pages (AnimeText)", crate::zoo::ZooId::AnimeText, has && n > 1).clicked() {
                    let all: Vec<usize> = (0..n).collect();
                    self.ai_zoo_pages(&all, ai::JobKind::Blocks, &ctx);
                    ui.close();
                }
                if item(ui, "Find panels, balloons, text and sound effects (Koharu layout)", crate::zoo::ZooId::KoharuLayout, has).clicked() {
                    let p = self.page;
                    self.ai_zoo_pages(&[p], ai::JobKind::Layout, &ctx);
                    ui.close();
                }
                if item(ui, "Mark the text pixels of this page (Koharu text masks)", crate::zoo::ZooId::KoharuTextMask, has).clicked() {
                    let p = self.page;
                    self.ai_zoo_pages(&[p], ai::JobKind::TextMask, &ctx);
                    ui.close();
                }
                let regions: Vec<String> = self.cur().map(|p| p.annotations.iter().filter(|a| a.level == Level::Region && self.selection.contains(&a.id)).map(|a| a.id.clone()).collect()).unwrap_or_default();
                let read_label = if regions.is_empty() { "Read text regions into lines and characters (charspot)".to_string() } else { format!("Read the {} selected region(s) into lines and characters (charspot)", regions.len()) };
                if item(ui, &read_label, crate::zoo::ZooId::Charspot, has).on_hover_text("Each text region without lines gets its lines, their text and one box per character, in reading order").clicked() {
                    let p = self.page;
                    let only = (!regions.is_empty()).then_some(regions.as_slice());
                    self.ai_read_blocks(p, only, &ctx);
                    ui.close();
                }
                let lines = self.cur().map(|p| p.annotations.iter().filter(|a| matches!(a.level, Level::Line | Level::Word) && self.selection.contains(&a.id)).count()).unwrap_or(0);
                if item(ui, &format!("Find the characters of the selected line(s) (charspot)   {}", self.settings.shortcut("ai_chars")), crate::zoo::ZooId::Charspot, lines > 0).clicked() {
                    self.ai_find_chars(&ctx);
                    ui.close();
                }
                if ui.button("Models…").on_hover_text("Where each model runs (processor or graphics card), installing runtimes, converting models").clicked() {
                    self.models_ui.open = true;
                    ui.close();
                }
                ui.separator();
                ui.label(RichText::new("Model roles").color(MUTED).size(11.0));
                for c in crate::models::Capability::ALL {
                    let mut names: Vec<String> = crate::models::providers(c).filter(|m| m.status == crate::models::Status::Available).map(|m| m.name.to_string()).collect();
                    names.extend(crate::zoo::ZooId::ALL.iter().filter(|z| z.capabilities().contains(&c)).map(|z| z.name().to_string()));
                    ui.label(RichText::new(format!("{}: {}", c.name(), if names.is_empty() { "—".to_string() } else { names.join(", ") })).size(11.0));
                }
                if ui.add_enabled(self.worker.pending() > 0, egui::Button::new("Cancel queued AI work")).clicked() {
                    self.worker.cancel();
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(has, egui::Button::new("Select all AI proposals on this page")).clicked() {
                    self.selection = self.cur().map(|p| p.annotations.iter().filter(|a| a.review.state == ReviewState::AiProposed).map(|a| a.id.clone()).collect()).unwrap_or_default();
                    ui.close();
                }
            });
            ui.menu_button("Review", |ui| {
                if ui.add_enabled(has, egui::Button::new(format!("Review Mode   {}", self.settings.shortcut("review_mode")))).clicked() {
                    ui.close();
                    if self.review.is_some() {
                        self.exit_review();
                    } else {
                        self.enter_review(&ctx);
                    }
                }
                if ui.add_enabled(has, egui::Button::new("Progress overview…")).clicked() {
                    self.dialogs.progress = true;
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Suggest reading order for this page")).on_hover_text("Fills in items without a manual order; your own numbers are kept").clicked() {
                    let dir = crate::assist::page_direction(self.cur_id().and_then(|id| self.store.as_ref().and_then(|s| s.entry(&id))).map(|e| &e.source), &self.cfg());
                    let n = self.op(|p, cfg, _, _| crate::assist::apply_order(p, cfg, dir)).unwrap_or(0);
                    self.flash(format!("Reading order suggested for {n} item(s); manual numbers kept"), &ctx);
                    ui.close();
                }
                if ui.add_enabled(self.selection.len() == 1, egui::Button::new("Select everything inside the selection")).clicked() {
                    self.select_contents();
                    ui.close();
                }
                ui.separator();
                let n = !self.selection.is_empty();
                if ui.add_enabled(n, egui::Button::new(format!("Submit for review   {}", self.settings.shortcut("submit")))).clicked() {
                    self.submit();
                    ui.close();
                }
                if ui.add_enabled(n, egui::Button::new(format!("Approve all aspects   {}", self.settings.shortcut("approve")))).clicked() {
                    self.approve(&Dimension::ALL, &ctx);
                    ui.close();
                }
                for d in Dimension::ALL {
                    if ui.add_enabled(n, egui::Button::new(format!("Approve {} only", d.name().to_lowercase()))).clicked() {
                        self.approve(&[d], &ctx);
                        ui.close();
                    }
                }
                if ui.add_enabled(n, egui::Button::new("Approve as gold")).clicked() {
                    self.approve_gold(&ctx);
                    ui.close();
                }
                if ui.add_enabled(n, egui::Button::new("Request changes…")).clicked() {
                    self.dialogs.note = Some((self.selection[0].clone(), NoteKind::ChangeRequest, None, String::new()));
                    ui.close();
                }
                if ui.add_enabled(n, egui::Button::new("Record a disagreement…")).clicked() {
                    self.dialogs.note = Some((self.selection[0].clone(), NoteKind::Disagreement, Some(Dimension::Transcription), String::new()));
                    ui.close();
                }
                ui.separator();
                if ui.button(format!("Next item needing work   {}", self.settings.shortcut("next_work"))).clicked() {
                    self.next_needing_work();
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new(format!("Page inspected and complete   {}", self.settings.shortcut("inspected")))).clicked() {
                    self.mark_inspected(true, false);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Page inspected: it has no text")).clicked() {
                    self.mark_inspected(true, true);
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Approve this page for export")).clicked() {
                    self.op(|p, _, user, now| p.review.approved_for_export = Some(Stamp { by: user.into(), at: now.into() }));
                    ui.close();
                }
                ui.separator();
                self.git_menu(ui, &ctx);
            });
            ui.menu_button("Help", |ui| {
                ui.set_min_width(320.0);
                if ui.button("Keyboard shortcuts…").clicked() {
                    self.dialogs.shortcuts = Some(None);
                    ui.close();
                }
                if let Some(s) = &self.store {
                    ui.label(RichText::new("Your data lives in plain files:").color(MUTED));
                    ui.label(RichText::new(s.root.display().to_string()).monospace().size(11.0));
                    ui.label(RichText::new("FORMAT.md in that folder explains every field.").color(MUTED));
                }
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let who = ui.add(egui::Button::new(RichText::new(format!("● {}", self.settings.user)).color(Color32::WHITE)).fill(PANEL_BG2).corner_radius(10.0));
                if who.on_hover_text("Who you are in review records. Click to change.").clicked() {
                    self.dialogs.user = Some(self.settings.user.clone());
                }
                if has {
                    let (txt, col) = if self.is_dirty() { ("Unsaved changes (autosaves)", WARN) } else { ("All changes saved", GOOD) };
                    ui.label(RichText::new(txt).color(col).size(12.0));
                    let n = self.worker.pending();
                    if n > 0 {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(RichText::new(format!("AI working ({n} queued)")).color(Color32::from_rgb(186, 104, 200)).size(12.0));
                    }
                    if let Some(s) = &self.store {
                        ui.label(RichText::new(&s.manifest.name).color(MUTED));
                    }
                }
            });
        });
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        let now = ui.input(|i| i.time);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let (name, hint) = match self.tool {
                Tool::Select => ("Select", "Click to select, Shift+click to add, drag empty space to select several. Drag handles to reshape; the round handle rotates a box."),
                Tool::Pan => ("Move view", "Drag to move. Scroll to zoom. Space+drag works with any tool."),
                Tool::Box => ("Box", "Drag to draw a box at the chosen level. Rotate it afterwards with its round handle."),
                Tool::Quad => ("Quadrilateral", "Click four corners in order TL, TR, BR, BL (or drag a rectangle and move its corners)."),
                Tool::Polygon => ("Polygon", "Click corners; click the first corner or double-click to close. Alt+click a corner removes it."),
                Tool::Order => ("Reading order", "Click items in the order a reader reads them. Starting a group renumbers it."),
                Tool::Link => ("Link", "With items selected, click their parent (bubble, panel…)."),
                Tool::AiRegion => ("AI region", "Drag around text; the model reads only that area in the background."),
            };
            ui.label(RichText::new(name).strong().color(ACCENT));
            ui.label(RichText::new(hint).color(MUTED).size(12.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(p) = self.cur() {
                    ui.label(RichText::new(format!("{} × {}", p.image.width, p.image.height)).monospace().color(MUTED));
                }
                ui.label(RichText::new(self.cursor_img.map(|c| format!("x {:>6.1}  y {:>6.1}", c[0], c[1])).unwrap_or_default()).monospace().color(MUTED));
                if let Some((m, t, err)) = self.message.clone() {
                    if err || now - t < 6.0 {
                        let r = ui.add(egui::Label::new(RichText::new(&m).color(if err { BAD } else { GOOD })).sense(Sense::click()).truncate());
                        if r.on_hover_text(format!("{m}\n(click to dismiss)")).clicked() {
                            self.message = None;
                        }
                    }
                }
            });
        });
    }

    fn page_matches(&self, p: &PageFile) -> bool {
        let q = self.search.to_lowercase();
        let ok = match self.page_filter {
            PageFilter::All => true,
            PageFilter::NeedsWork => p.annotations.iter().any(|a| a.review.state.needs_work() || a.review.needs_rereview) || !p.completeness.inspected,
            PageFilter::AiProposed => p.annotations.iter().any(|a| a.review.state == ReviewState::AiProposed),
            PageFilter::NotInspected => !p.completeness.inspected,
            PageFilter::Complete => p.completeness.complete,
        };
        let work = self.store.as_ref().and_then(|s| s.entry(&p.page_id)).map(|e| e.source.work.to_lowercase()).unwrap_or_default();
        ok && (q.is_empty() || p.page_id.to_lowercase().contains(&q) || p.image.path.to_lowercase().contains(&q) || work.contains(&q))
    }

    pub fn page_list(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(6.0);
        let pages = self.pages();
        let total = pages.len().max(1);
        let complete = pages.iter().filter(|p| p.completeness.complete).count();
        let mut states: BTreeMap<ReviewState, usize> = BTreeMap::new();
        for a in pages.iter().flat_map(|p| p.annotations.iter()) {
            *states.entry(a.review.state).or_default() += 1;
        }
        let n_ann: usize = states.values().sum();
        let m = self.metrics.clone();
        let mut open_details = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new("PROGRESS").size(11.0).strong().color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Details…").clicked() {
                    open_details = true;
                }
            });
        });
        let bar = |ui: &mut Ui, v: usize, of: usize, label: &str, col: Color32, tip: &str| {
            ui.add(egui::ProgressBar::new(v as f32 / of.max(1) as f32).desired_height(13.0).fill(col).text(RichText::new(format!("{label} {v} / {of}")).size(10.5))).on_hover_text(tip);
        };
        bar(ui, m.pages_covered, m.pages, "Annotated pages", Color32::from_rgb(70, 130, 200), "Coverage: pages with text annotated (or marked as having none)");
        bar(ui, m.complete_required, m.total, "Required fields done", WARN.gamma_multiply(0.8), "Metadata completeness: annotations whose required fields are all answered");
        bar(ui, m.fully_approved, m.total, "Verified by people", GOOD, "Human verification: annotations approved by enough reviewers");
        ui.add(egui::ProgressBar::new(complete as f32 / total as f32).desired_height(13.0).fill(Color32::from_rgb(60, 150, 90)).text(RichText::new(format!("{complete} / {} pages complete", pages.len())).size(10.5)));
        // stacked bar of review states
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        let mut x = r.left();
        ui.painter().rect_filled(r, 3.0, PANEL_BG2);
        for (s, n) in &states {
            let w = r.width() * *n as f32 / n_ann.max(1) as f32;
            ui.painter().rect_filled(Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height())), 0.0, rgb(s.color()));
            x += w;
        }
        resp.on_hover_text(states.iter().map(|(s, n)| format!("{}: {n}", s.name())).collect::<Vec<_>>().join("\n"));
        if open_details {
            self.dialogs.progress = true;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (s, n) in &states {
                ui.label(RichText::new(format!("■ {} {n}", s.name())).color(rgb(s.color())).size(10.5));
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Import…").on_hover_text("Import pictures (copied into the project)").clicked() {
                self.import_pictures(&ctx, false);
            }
            if ui.button("AI on all pages").on_hover_text("Pre-annotate every page not yet run, in the background").clicked() {
                self.ai_all_pages(&ctx);
            }
        });
        ui.add_space(4.0);
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search pages or works").desired_width(f32::INFINITY));
        egui::ComboBox::from_id_salt("page_filter")
            .width(ui.available_width())
            .selected_text(match self.page_filter {
                PageFilter::All => "All pages",
                PageFilter::NeedsWork => "Needing work",
                PageFilter::AiProposed => "With AI proposals",
                PageFilter::NotInspected => "Not yet inspected",
                PageFilter::Complete => "Complete",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.page_filter, PageFilter::All, "All pages");
                ui.selectable_value(&mut self.page_filter, PageFilter::NeedsWork, "Needing work");
                ui.selectable_value(&mut self.page_filter, PageFilter::AiProposed, "With AI proposals");
                ui.selectable_value(&mut self.page_filter, PageFilter::NotInspected, "Not yet inspected");
                ui.selectable_value(&mut self.page_filter, PageFilter::Complete, "Complete");
            });
        self.page_bulk_menu(ui, &ctx);
        ui.separator();
        let list: Vec<usize> = (0..self.pages().len()).filter(|&i| self.page_matches(&self.pages()[i])).collect();
        let row_h = 66.0;
        let mut budget = 1;
        let mut clicked = None;
        let errors: HashMap<&str, usize> = self.issues.iter().filter(|i| i.severity == Severity::Error).fold(HashMap::new(), |mut m, i| {
            *m.entry(i.page_id.as_str()).or_default() += 1;
            m
        });
        let errors: HashMap<String, usize> = errors.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, list.len(), |ui, range| {
            for &idx in &list[range] {
                let pid = self.pages()[idx].page_id.clone();
                if !self.thumbs.contains_key(&pid) && budget > 0 {
                    budget -= 1;
                    let t = self.page_image(&pid).map(|img| {
                        let th = image::imageops::thumbnail(img.as_ref(), 60, (60 * img.height() / img.width().max(1)).max(1));
                        ctx.load_texture(format!("thumb-{pid}"), egui::ColorImage::from_rgba_unmultiplied([th.width() as usize, th.height() as usize], th.as_raw()), TextureOptions::LINEAR)
                    });
                    self.thumbs.insert(pid.clone(), t);
                    if idx.abs_diff(self.page) > 1 {
                        self.images.remove(&pid);
                    }
                }
                let p = &self.pages()[idx];
                let selected = idx == self.page;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h - 4.0), Sense::click());
                let painter = ui.painter();
                painter.rect_filled(rect, 6.0, if selected { Color32::from_rgb(52, 58, 92) } else if resp.hovered() { PANEL_BG2 } else { Color32::TRANSPARENT });
                if selected {
                    painter.rect_filled(Rect::from_min_size(rect.min, vec2(3.0, rect.height())), 2.0, ACCENT);
                }
                let th = Rect::from_min_size(rect.min + vec2(8.0, 4.0), vec2(42.0, rect.height() - 8.0));
                match self.thumbs.get(&pid) {
                    Some(Some(t)) => {
                        painter.image(t.id(), th, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    _ => {
                        painter.rect_filled(th, 3.0, Color32::from_rgb(55, 58, 68));
                    }
                }
                let tx = th.right() + 8.0;
                let name: String = std::path::Path::new(&p.image.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let name: String = if name.chars().count() > 24 { format!("{}…", name.chars().take(23).collect::<String>()) } else { name };
                painter.text(pos2(tx, rect.top() + 7.0), Align2::LEFT_TOP, name, FontId::proportional(12.5), if selected { Color32::WHITE } else { ui.visuals().text_color() });
                let lines = p.annotations.iter().filter(|a| a.level.is_text()).count();
                let ai_n = p.annotations.iter().filter(|a| a.review.state == ReviewState::AiProposed).count();
                let gold = p.annotations.iter().filter(|a| a.review.state == ReviewState::GoldApproved).count();
                let mut x = tx;
                let y = rect.top() + 27.0;
                let mut chip = |txt: String, col: Color32| {
                    let r = painter.text(pos2(x, y), Align2::LEFT_TOP, txt, FontId::proportional(11.0), col);
                    x = r.right() + 8.0;
                };
                chip(format!("{lines} text"), MUTED);
                if ai_n > 0 {
                    chip(format!("{ai_n} AI"), rgb(ReviewState::AiProposed.color()));
                }
                if gold > 0 {
                    chip(format!("{gold} gold"), rgb(ReviewState::GoldApproved.color()));
                }
                if let Some(e) = errors.get(&pid) {
                    chip(format!("{e} errors"), BAD);
                }
                let state = if p.completeness.complete {
                    ("complete", GOOD)
                } else if p.completeness.inspected {
                    ("inspected", WARN)
                } else {
                    ("not inspected", MUTED)
                };
                painter.text(pos2(tx, rect.top() + 44.0), Align2::LEFT_TOP, state.0, FontId::proportional(10.5), state.1);
                if p.completeness.complete {
                    icons::paint(painter, Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), Vec2::splat(14.0)), Icon::Check, GOOD);
                }
                if self.worker.current.lock().map(|c| *c == pid).unwrap_or(false) {
                    painter.circle_filled(pos2(rect.right() - 14.0, rect.top() + 12.0), 4.0, rgb(ReviewState::AiProposed.color()));
                }
                if self.page_sel.contains(&idx) {
                    painter.rect_stroke(rect, 6.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
                }
                if resp.clicked() {
                    clicked = Some(idx);
                }
            }
        });
        if let Some(i) = clicked {
            // Ctrl+click picks several pages for bulk actions, Shift+click a range
            let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
            if cmd {
                if !self.page_sel.remove(&i) {
                    self.page_sel.insert(i);
                }
            } else if shift {
                let (a, b) = (self.page.min(i), self.page.max(i));
                self.page_sel.extend(a..=b);
            } else {
                self.page_sel.clear();
                self.go_to(i);
            }
        }
    }

    pub fn tool_strip(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            let tools = [
                (Tool::Select, Icon::Select, "Select and edit", "tool_select"),
                (Tool::Pan, Icon::Pan, "Move the view (or hold Space)", "tool_pan"),
                (Tool::Box, Icon::Rect, "Box (rotatable)", "tool_box"),
                (Tool::Quad, Icon::Quad, "Quadrilateral (four corners)", "tool_quad"),
                (Tool::Polygon, Icon::Polygon, "Polygon", "tool_polygon"),
                (Tool::Order, Icon::Order, "Reading order: click in order", "tool_order"),
                (Tool::Link, Icon::Link, "Link to parent", "tool_link"),
                (Tool::AiRegion, Icon::Ai, "AI: read a region", "tool_ai_region"),
            ];
            for (t, icon, tip, action) in tools {
                let tip = format!("{tip}  ({})", self.settings.shortcut(action));
                if icons::button(ui, icon, 34.0, self.tool == t, &tip).clicked() {
                    self.set_tool(t);
                }
                ui.add_space(1.0);
                if matches!(t, Tool::Pan | Tool::Polygon | Tool::Link) {
                    ui.add_space(3.0);
                    ui.separator();
                    ui.add_space(3.0);
                }
            }
            ui.add_space(6.0);
            ui.separator();
            if icons::button(ui, Icon::Split, 34.0, false, "Split the selection  (S)").clicked() {
                self.split_selection(&ctx);
            }
            if icons::button(ui, Icon::Merge, 34.0, false, "Merge the selection  (M)").clicked() {
                self.merge_selection(&ctx);
            }
            if icons::button(ui, Icon::Trash, 34.0, false, "Delete the selection  (Del)").clicked() {
                self.delete_selection();
            }
            ui.separator();
            if icons::button(ui, Icon::Undo, 34.0, false, "Undo  (Ctrl+Z)").clicked() {
                self.undo(false);
            }
            if icons::button(ui, Icon::Redo, 34.0, false, "Redo  (Ctrl+Y)").clicked() {
                self.undo(true);
            }
        });
    }

    pub fn top_strip(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let cfg = self.cfg();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if icons::button(ui, Icon::Prev, 26.0, false, "Previous page  (A)").clicked() {
                let p = self.page.saturating_sub(1);
                self.go_to(p);
            }
            if icons::button(ui, Icon::Next, 26.0, false, "Next page  (D)").clicked() {
                let p = self.page + 1;
                self.go_to(p);
            }
            if let Some(p) = self.cur() {
                let name = std::path::Path::new(&p.image.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                ui.label(RichText::new(name).strong().size(13.5));
                ui.label(RichText::new(format!("{} / {}", self.page + 1, self.pages().len())).color(MUTED));
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Draw").color(MUTED));
            egui::ComboBox::from_id_salt("draw_level").width(104.0).selected_text(self.draw_level.name()).show_ui(ui, |ui| {
                for (i, l) in Level::DRAWABLE.iter().enumerate() {
                    let req = cfg.levels.get(l).copied().unwrap_or(Requirement::Optional);
                    let tag = match req {
                        Requirement::Required => " · required",
                        Requirement::Optional => "",
                        Requirement::Disabled => " · disabled",
                    };
                    ui.add_enabled_ui(req != Requirement::Disabled || cfg.level_overrides.iter().any(|o| o.level == *l), |ui| {
                        ui.selectable_value(&mut self.draw_level, *l, format!("{}  {}{tag}", i + 1, l.name()));
                    });
                }
            });
            let class_name = self.draw_class.as_ref().and_then(|c| cfg.class(c)).map(|c| c.name.clone()).unwrap_or_else(|| "No class".into());
            egui::ComboBox::from_id_salt("draw_class").width(104.0).selected_text(class_name).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.draw_class, None, "No class");
                for c in &cfg.text_classes {
                    ui.selectable_value(&mut self.draw_class, Some(c.id.clone()), &c.name);
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let inspected = self.cur().is_some_and(|p| p.completeness.complete);
                let btn = egui::Button::new(RichText::new(if inspected { "Page complete" } else { "Mark page complete" }).color(Color32::WHITE))
                    .fill(if inspected { Color32::from_rgb(46, 125, 80) } else { PANEL_BG })
                    .corner_radius(6.0);
                if ui.add(btn).on_hover_text("I checked the whole page for missing text (Ctrl+I)").clicked() {
                    if inspected {
                        self.op(|p, _, _, _| {
                            p.completeness.complete = false;
                        });
                    } else {
                        self.mark_inspected(true, false);
                    }
                }
                let rv = egui::Button::new(RichText::new("Review mode").color(Color32::BLACK)).fill(Color32::from_rgb(255, 200, 80)).corner_radius(6.0);
                if ui.add(rv).on_hover_text("Go through annotations one by one with the keyboard (F6)").clicked() {
                    self.enter_review(&ctx);
                }
                let ai = egui::Button::new(RichText::new("AI pre-annotate").color(Color32::WHITE)).fill(Color32::from_rgb(120, 70, 150)).corner_radius(6.0);
                if ui.add(ai).on_hover_text("Run the OCR model on this page in the background (Ctrl+R)").clicked() {
                    let p = self.page;
                    self.ai_job(p, ai::JobKind::Page);
                    self.flash("Reading the page in the background…", &ctx);
                }
                ui.add_space(6.0);
                if icons::button(ui, Icon::Fit, 26.0, false, "Fit page  (F)").clicked() {
                    self.view.fitted = true;
                }
                ui.label(RichText::new(format!("{:.0}%", self.view.zoom * 100.0)).monospace().color(MUTED)).on_hover_text("Scroll to zoom");
            });
        });
    }

    pub fn welcome(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, CANVAS_BG);
        ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_center_size(rect.center(), vec2(520.0, 460.0))), |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(RichText::new("Comic OCR annotation").size(26.0).color(Color32::WHITE));
                ui.label(RichText::new("Build a reviewable gold-standard OCR dataset from manga, manhua, webtoons and comics.").color(MUTED));
                ui.add_space(18.0);
                let big = |t: &str| egui::Button::new(RichText::new(t).size(15.0)).min_size(vec2(260.0, 34.0));
                if ui.add(big("New project…")).clicked() {
                    self.dialogs.new_project = Some(("My comic OCR dataset".into(), documents_dir().join("LabelDesk"), Requirement::Optional));
                }
                if ui.add(big("Open project…")).clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_title("Open a project folder").pick_folder() {
                        self.open_project(&d);
                    }
                }
                if ui.add(big("Try the sample project")).clicked() {
                    self.open_sample(&ctx);
                }
                ui.add_space(14.0);
                if !self.settings.recent.is_empty() {
                    ui.label(RichText::new("Recent").color(MUTED));
                    for p in self.settings.recent.clone() {
                        if ui.link(p.display().to_string()).clicked() {
                            self.open_project(&p);
                        }
                    }
                }
            });
        });
    }
}
