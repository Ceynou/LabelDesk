//! Find labels by any combination of criteria, then act on the results in bulk: select, hide,
//! review them, or set their type, attributes, tags, legibility and confidence. Also: hiding
//! labels while working, jumping between problems, and reading labels again with the AI.

use super::*;
use crate::assist::{self, QueueKind};
use eframe::egui::RichText;

#[derive(Clone, Debug, PartialEq)]
pub enum Crit {
    Text(String),
    Level(Level),
    Class(String),
    NoClass,
    Attribute(String),
    Tag(String),
    State(ReviewState),
    Needs(QueueKind),
    Problem,
    Legibility(Option<Recognizability>),
    Uncertain,
    Origin(Origin),
    Comment,
}

impl Crit {
    fn kind(&self) -> &'static str {
        match self {
            Crit::Text(_) => "Text contains",
            Crit::Level(_) => "Level",
            Crit::Class(_) | Crit::NoClass => "Text type",
            Crit::Attribute(_) => "Attribute",
            Crit::Tag(_) => "Tag",
            Crit::State(_) => "Review state",
            Crit::Needs(_) => "Needs",
            Crit::Problem => "Has a problem",
            Crit::Legibility(_) => "Legibility",
            Crit::Uncertain => "Marked uncertain",
            Crit::Origin(_) => "Made by",
            Crit::Comment => "Has comments",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub crit: Crit,
    pub not: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FindScope {
    #[default]
    ThisPage,
    AllPages,
    SelectedPages,
}

#[derive(Default)]
pub struct Find {
    pub open: bool,
    pub conds: Vec<Cond>,
    pub scope: FindScope,
    /// (page id, annotation id) of the last search.
    pub results: Vec<(String, String)>,
    pub bulk_tag: String,
}

/// Does the label meet one criterion?
pub fn meets(c: &Crit, a: &Annotation, page: &PageFile, cfg: &ProjectConfig, problems: &std::collections::HashSet<&str>) -> bool {
    match c {
        Crit::Text(t) => !t.is_empty() && a.text().to_lowercase().contains(&t.to_lowercase()),
        Crit::Level(l) => a.level == *l,
        Crit::Class(k) => a.classes.contains(k),
        Crit::NoClass => a.classes.is_empty(),
        Crit::Attribute(k) => a.attributes.contains(k),
        Crit::Tag(t) => a.tags.contains(t),
        Crit::State(s) => a.review.state == *s,
        Crit::Needs(k) => assist::matches_kind(*k, a, page, cfg),
        Crit::Problem => problems.contains(a.id.as_str()),
        Crit::Legibility(r) => a.recognizability == *r,
        Crit::Uncertain => {
            let c = &a.confidence;
            [c.transcription, c.localization, c.geometry, c.reading_order].contains(&Some(Confidence::Uncertain)) || a.recognizability == Some(Recognizability::NotPerfect)
        }
        Crit::Origin(o) => a.provenance.origin == *o,
        Crit::Comment => !a.review.notes.is_empty(),
    }
}

impl App {
    /// Run the search over the chosen pages.
    pub fn run_find(&mut self) {
        let cfg = self.cfg();
        let problems: std::collections::HashSet<&str> = self.issues.iter().filter_map(|i| i.annotation_id.as_deref()).collect();
        let idx: Vec<usize> = match self.find.scope {
            FindScope::ThisPage => vec![self.page],
            FindScope::AllPages => (0..self.pages().len()).collect(),
            FindScope::SelectedPages => {
                let mut v: Vec<usize> = self.page_sel.iter().copied().collect();
                if v.is_empty() {
                    v.push(self.page);
                }
                v
            }
        };
        let mut out = Vec::new();
        for i in idx {
            let Some(page) = self.pages().get(i) else { continue };
            for a in &page.annotations {
                if self.find.conds.iter().all(|c| meets(&c.crit, a, page, &cfg, &problems) != c.not) {
                    out.push((page.page_id.clone(), a.id.clone()));
                }
            }
        }
        self.find.results = out;
    }

    /// Apply `f` to every found label, page by page (one undo step per page).
    fn bulk(&mut self, f: impl Fn(&mut Annotation, &ProjectConfig, &str, &str)) -> usize {
        let results = self.find.results.clone();
        let mut n = 0;
        let pages: BTreeSet<String> = results.iter().map(|r| r.0.clone()).collect();
        for pid in pages {
            let Some(i) = self.store.as_ref().and_then(|s| s.index_of(&pid)) else { continue };
            let ids: BTreeSet<String> = results.iter().filter(|r| r.0 == pid).map(|r| r.1.clone()).collect();
            n += self
                .op_on(i, |p, cfg, user, now| {
                    let mut k = 0;
                    for a in p.annotations.iter_mut().filter(|a| ids.contains(&a.id)) {
                        let old = a.clone();
                        f(a, cfg, user, now);
                        if *a != old {
                            review::on_edit(&old, a, &cfg.review, user, now);
                            k += 1;
                        }
                    }
                    k
                })
                .unwrap_or(0);
        }
        self.rebaseline();
        n
    }

    pub fn find_dialog(&mut self, ctx: &egui::Context) {
        if !self.find.open {
            return;
        }
        let cfg = self.cfg();
        let mut open = true;
        let mut search = false;
        let mut action: Option<&'static str> = None;
        let mut set_class: Option<String> = None;
        let mut set_attr: Option<(String, bool)> = None;
        let mut set_recog: Option<Recognizability> = None;
        let mut set_conf: Option<Confidence> = None;
        let mut goto: Option<(String, String)> = None;
        let npages = self.page_sel.len();
        egui::Window::new("Find labels").open(&mut open).default_width(460.0).default_height(560.0).vscroll(true).show(ctx, |ui| {
            ui.label(RichText::new("Every condition must hold. “not” turns a condition around.").color(MUTED).size(11.5));
            let mut drop = None;
            for (i, c) in self.find.conds.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut c.not, "not");
                    ui.label(RichText::new(c.crit.kind()).strong());
                    match &mut c.crit {
                        Crit::Text(t) => {
                            if ui.add(egui::TextEdit::singleline(t).desired_width(160.0)).changed() {
                                search = true;
                            }
                        }
                        Crit::Level(l) => {
                            egui::ComboBox::from_id_salt(("fl", i)).selected_text(l.name()).show_ui(ui, |ui| {
                                for x in Level::DRAWABLE {
                                    search |= ui.selectable_value(l, x, x.name()).changed();
                                }
                            });
                        }
                        Crit::Class(k) => {
                            let name = cfg.class(k).map(|c| c.name.clone()).unwrap_or(k.clone());
                            egui::ComboBox::from_id_salt(("fc", i)).selected_text(name).show_ui(ui, |ui| {
                                for x in &cfg.text_classes {
                                    search |= ui.selectable_value(k, x.id.clone(), &x.name).changed();
                                }
                            });
                        }
                        Crit::Attribute(k) => {
                            egui::ComboBox::from_id_salt(("fa", i)).selected_text(k.as_str()).show_ui(ui, |ui| {
                                for x in &cfg.attributes {
                                    search |= ui.selectable_value(k, x.clone(), x).changed();
                                }
                            });
                        }
                        Crit::Tag(t) => {
                            search |= ui.add(egui::TextEdit::singleline(t).desired_width(120.0)).changed();
                        }
                        Crit::State(s) => {
                            egui::ComboBox::from_id_salt(("fs", i)).selected_text(s.name()).show_ui(ui, |ui| {
                                for x in ReviewState::ALL {
                                    search |= ui.selectable_value(s, x, x.name()).changed();
                                }
                            });
                        }
                        Crit::Needs(k) => {
                            egui::ComboBox::from_id_salt(("fn", i)).selected_text(k.name()).show_ui(ui, |ui| {
                                for x in QueueKind::ALL {
                                    search |= ui.selectable_value(k, x, x.name()).changed();
                                }
                            });
                        }
                        Crit::Legibility(r) => {
                            egui::ComboBox::from_id_salt(("fr", i)).selected_text(r.map(|r| r.name()).unwrap_or("not answered")).show_ui(ui, |ui| {
                                search |= ui.selectable_value(r, None, "not answered").changed();
                                for x in Recognizability::COARSE.iter().chain(Recognizability::ALL[1..].iter()) {
                                    search |= ui.selectable_value(r, Some(*x), x.name()).changed();
                                }
                            });
                        }
                        Crit::Origin(o) => {
                            egui::ComboBox::from_id_salt(("fo", i)).selected_text(format!("{o:?}")).show_ui(ui, |ui| {
                                for x in [Origin::Manual, Origin::Ai, Origin::Heuristic, Origin::Imported, Origin::Sample] {
                                    search |= ui.selectable_value(o, x, format!("{x:?}")).changed();
                                }
                            });
                        }
                        _ => {}
                    }
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                self.find.conds.remove(i);
                search = true;
            }
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Add:").color(MUTED));
                let first_class = cfg.text_classes.first().map(|c| c.id.clone()).unwrap_or_default();
                let first_attr = cfg.attributes.first().cloned().unwrap_or_default();
                for (name, c) in [
                    ("text", Crit::Text(String::new())),
                    ("level", Crit::Level(Level::Line)),
                    ("type", Crit::Class(first_class)),
                    ("no type", Crit::NoClass),
                    ("attribute", Crit::Attribute(first_attr)),
                    ("tag", Crit::Tag(String::new())),
                    ("state", Crit::State(ReviewState::AiProposed)),
                    ("needs", Crit::Needs(QueueKind::MissingFields)),
                    ("problem", Crit::Problem),
                    ("legibility", Crit::Legibility(None)),
                    ("uncertain", Crit::Uncertain),
                    ("made by", Crit::Origin(Origin::Ai)),
                    ("comments", Crit::Comment),
                ] {
                    if ui.small_button(name).clicked() {
                        self.find.conds.push(Cond { crit: c, not: false });
                        search = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("In");
                search |= ui.selectable_value(&mut self.find.scope, FindScope::ThisPage, "this page").changed();
                search |= ui.selectable_value(&mut self.find.scope, FindScope::AllPages, "all pages").changed();
                search |= ui.selectable_value(&mut self.find.scope, FindScope::SelectedPages, format!("selected pages ({npages})")).on_hover_text("Ctrl+click pages in the list").changed();
            });
            if ui.button("Search").clicked() {
                search = true;
            }
            ui.separator();
            let n = self.find.results.len();
            ui.label(RichText::new(format!("{n} label(s) found")).strong());
            ui.horizontal_wrapped(|ui| {
                for (name, key) in [("Select (this page)", "select"), ("Hide", "hide"), ("Show only these", "only"), ("Review these", "review"), ("Approve", "approve")] {
                    if ui.add_enabled(n > 0, egui::Button::new(name)).clicked() {
                        action = Some(key);
                    }
                }
            });
            ui.add_enabled_ui(n > 0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Set type").color(MUTED));
                    egui::ComboBox::from_id_salt("bulk_class").selected_text("choose…").show_ui(ui, |ui| {
                        for c in &cfg.text_classes {
                            if ui.selectable_label(false, &c.name).clicked() {
                                set_class = Some(c.id.clone());
                            }
                        }
                    });
                    ui.label(RichText::new("Attribute").color(MUTED));
                    egui::ComboBox::from_id_salt("bulk_attr").selected_text("add / remove…").show_ui(ui, |ui| {
                        for at in &cfg.attributes {
                            if ui.selectable_label(false, format!("+ {at}")).clicked() {
                                set_attr = Some((at.clone(), true));
                            }
                            if ui.selectable_label(false, format!("− {at}")).clicked() {
                                set_attr = Some((at.clone(), false));
                            }
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Legibility").color(MUTED));
                    for r in Recognizability::COARSE {
                        if ui.small_button(r.name()).clicked() {
                            set_recog = Some(r);
                        }
                    }
                    ui.label(RichText::new("Confidence").color(MUTED));
                    if ui.small_button("Certain").clicked() {
                        set_conf = Some(Confidence::High);
                    }
                    if ui.small_button("Uncertain").clicked() {
                        set_conf = Some(Confidence::Uncertain);
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.find.bulk_tag).hint_text("tag").desired_width(120.0));
                    if ui.small_button("Add tag").clicked() && !self.find.bulk_tag.trim().is_empty() {
                        action = Some("tag");
                    }
                    if ui.small_button("Remove tag").clicked() && !self.find.bulk_tag.trim().is_empty() {
                        action = Some("untag");
                    }
                });
            });
            ui.separator();
            let pages: HashMap<String, usize> = self.pages().iter().enumerate().map(|(i, p)| (p.page_id.clone(), i)).collect();
            for (pid, aid) in self.find.results.iter().take(300) {
                let Some(a) = pages.get(pid).and_then(|i| self.pages()[*i].get(aid)) else { continue };
                let label = format!("p{} · {} · {}", pages[pid] + 1, a.level.name(), a.text().replace('\n', " ").chars().take(30).collect::<String>());
                if ui.selectable_label(self.selection.contains(aid), label).clicked() {
                    goto = Some((pid.clone(), aid.clone()));
                }
            }
            if n > 300 {
                ui.label(RichText::new(format!("… and {} more", n - 300)).color(MUTED));
            }
        });
        self.find.open = open;
        if search {
            self.run_find();
        }
        if let Some((pid, aid)) = goto {
            if let Some(i) = self.store.as_ref().and_then(|s| s.index_of(&pid)) {
                self.go_to(i);
                self.selection = vec![aid];
            }
        }
        let user = self.user();
        let changed = if let Some(c) = set_class {
            self.bulk(move |a, _, _, _| a.classes = [c.clone()].into_iter().collect())
        } else if let Some((at, on)) = set_attr {
            self.bulk(move |a, _, _, _| {
                if on {
                    a.attributes.insert(at.clone());
                } else {
                    a.attributes.remove(&at);
                }
            })
        } else if let Some(r) = set_recog {
            self.bulk(move |a, _, u, now| {
                a.recognizability = Some(r);
                assist::set_check(a, "recognizability", CheckState::Confirmed, u, now, "batch answer", true);
            })
        } else if let Some(c) = set_conf {
            self.bulk(move |a, _, u, now| {
                a.confidence.transcription = Some(c);
                a.confidence.geometry = Some(c);
                assist::set_check(a, "confidence", CheckState::Confirmed, u, now, "batch answer", true);
            })
        } else {
            usize::MAX
        };
        if changed != usize::MAX {
            self.flash(format!("{changed} label(s) changed (Ctrl+Z undoes, page by page)"), ctx);
            self.run_find();
        }
        let tag = self.find.bulk_tag.trim().to_string();
        match action {
            Some("select") => {
                let pid = self.cur_id().unwrap_or_default();
                self.selection = self.find.results.iter().filter(|r| r.0 == pid).map(|r| r.1.clone()).collect();
            }
            Some("hide") => {
                self.hidden.extend(self.find.results.iter().map(|r| r.1.clone()));
                self.selection.retain(|s| !self.hidden.contains(s));
            }
            Some("only") => {
                let keep: std::collections::HashSet<String> = self.find.results.iter().map(|r| r.1.clone()).collect();
                let all: Vec<String> = self.pages().iter().flat_map(|p| p.annotations.iter().map(|a| a.id.clone())).collect();
                self.hidden = all.into_iter().filter(|id| !keep.contains(id)).collect();
            }
            Some("review") => {
                self.find.open = false;
                self.enter_review(ctx);
                if let Some(r) = &mut self.review {
                    r.setup = true;
                    r.scope = super::review_mode::Scope::Found;
                }
            }
            Some("approve") => {
                let n = self.bulk(|a, cfg, u, now| {
                    let _ = review::approve(a, &cfg.review.required_dimensions, &cfg.review, u, now);
                });
                let _ = user;
                self.flash(format!("{n} label(s) approved (labels you made yourself need another reviewer under this project's policy)"), ctx);
            }
            Some("tag") => {
                let n = self.bulk(|a, _, _, _| {
                    a.tags.insert(tag.clone());
                });
                self.flash(format!("Tag “{tag}” added to {n} label(s)"), ctx);
            }
            Some("untag") => {
                let n = self.bulk(|a, _, _, _| {
                    a.tags.remove(&tag);
                });
                self.flash(format!("Tag “{tag}” removed from {n} label(s)"), ctx);
            }
            _ => {}
        }
    }

    /// H: hide the selection to work on other things undisturbed (Shift+H shows everything).
    pub fn hide_selection(&mut self, ctx: &egui::Context) {
        if self.selection.is_empty() {
            let n = self.hidden.len();
            self.hidden.clear();
            if n > 0 {
                self.flash(format!("{n} hidden label(s) shown again"), ctx);
            }
            return;
        }
        let n = self.selection.len();
        self.hidden.extend(self.selection.drain(..));
        self.flash(format!("{n} label(s) hidden; Shift+H shows them again"), ctx);
    }

    /// F8: select and show the next label with a problem on this page (then the next pages).
    pub fn next_problem(&mut self, ctx: &egui::Context) {
        self.refresh_checks(false);
        let n = self.pages().len();
        let cur = self.selection.first().cloned();
        for step in 0..=n {
            let i = (self.page + step) % n.max(1);
            let Some(page) = self.pages().get(i) else { return };
            let mut ids: Vec<&Annotation> = self.issues.iter().filter(|x| x.page_id == page.page_id).filter_map(|x| x.annotation_id.as_deref()).filter_map(|id| page.get(id)).collect();
            ids.sort_by_cached_key(|a| assist::reading_key(page, a));
            ids.dedup_by(|a, b| a.id == b.id);
            let start = if step == 0 { cur.as_ref().and_then(|c| ids.iter().position(|a| &a.id == c)).map(|k| k + 1).unwrap_or(0) } else { 0 };
            if let Some(a) = ids.get(start) {
                let id = a.id.clone();
                let msg = self.issues.iter().find(|x| x.annotation_id.as_deref() == Some(id.as_str())).map(|x| x.message.clone()).unwrap_or_default();
                self.go_to(i);
                self.hidden.remove(&id);
                self.selection = vec![id];
                self.show_problems = true;
                self.center_selection = true;
                self.flash(format!("Problem: {msg}  (F8 next)"), ctx);
                return;
            }
        }
        self.flash("No problems found", ctx);
    }

    /// Read these labels again with the AI model (Ctrl+Shift+R). Readings fill empty
    /// transcriptions and are added as alternatives otherwise.
    pub fn reread(&mut self, ids: &[String]) {
        let page = self.page;
        for id in ids {
            let Some(a) = self.ann(id).cloned() else { continue };
            if !a.level.is_text() {
                continue;
            }
            let (quad, size) = match (&a.rectification, a.geometry.corners()) {
                (Some(r), _) => (r.source, r.target_size),
                (_, Some(q)) => {
                    let (w, h) = geom::proposed_size(&q);
                    (q, [w, h])
                }
                _ => {
                    let (lo, hi) = a.geometry.bounds();
                    let q = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
                    let (w, h) = geom::proposed_size(&q);
                    (q, [w, h])
                }
            };
            self.ai_job(page, ai::JobKind::Recognize { annotation_id: a.id, quad, target_size: size, rectified: true });
        }
    }

    /// Version control: pages are committed once their review is complete; the rest stays as
    /// uncommitted changes.
    pub fn git_menu(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let Some(root) = self.store.as_ref().map(|s| s.root.clone()) else { return };
        if !crate::git::available() {
            ui.label(RichText::new("Git is not installed: versions are not tracked").color(MUTED).size(11.0));
            return;
        }
        let user = self.user();
        if !crate::git::is_repo(&root) {
            if ui.button("Track this project with Git").on_hover_text("Creates a repository; pages are then committed one by one as their review completes").clicked() {
                let _ = self.store.as_mut().map(|s| s.save_all(&user));
                match crate::git::init(&root, &user) {
                    Ok(_) => self.flash("Git repository created; reviewed pages will be committed", ctx),
                    Err(e) => self.error(format!("Git: {e}")),
                }
                ui.close();
            }
            return;
        }
        if ui.checkbox(&mut self.settings.git_commit_reviewed, "Commit each page when its review is complete").changed() {
            self.settings.save();
        }
        if ui.button("Commit this page now").on_hover_text("Commits this page's files only, whatever their review state").clicked() {
            self.commit();
            let pid = self.cur_id().unwrap_or_default();
            let _ = self.store.as_mut().map(|s| s.save_page(&pid, &user));
            let res = self.store.as_ref().map(|s| crate::git::commit_files(&s.root, &crate::git::page_files(s, &pid), &format!("Update page {pid}"), &user));
            match res {
                Some(Ok(Some(id))) => self.flash(format!("Committed {pid} ({id})"), ctx),
                Some(Ok(None)) => self.flash("Nothing to commit for this page", ctx),
                Some(Err(e)) => self.error(format!("Git: {e}")),
                None => {}
            }
            ui.close();
        }
        let pending = crate::git::pending(&root).len();
        ui.label(RichText::new(format!("{pending} file(s) changed and not committed")).color(MUTED).size(11.0));
    }

    /// Bulk actions on the pages selected in the page list (Ctrl+click).
    pub fn page_bulk_menu(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let sel: Vec<usize> = self.page_sel.iter().copied().collect();
        if sel.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} page(s) selected", sel.len())).strong());
            if ui.small_button("Mark inspected").clicked() {
                for &i in &sel {
                    self.op_on(i, |p, _, user, now| {
                        p.completeness.inspected = true;
                        p.completeness.complete = true;
                        p.completeness.checked = Some(Stamp { by: user.into(), at: now.into() });
                    });
                }
            }
            if ui.small_button("Run AI").clicked() {
                for &i in &sel {
                    self.ai_job(i, ai::JobKind::Page);
                }
                self.flash(format!("AI queued for {} page(s)", sel.len()), ctx);
            }
            if ui.small_button("Suggest reading order").clicked() {
                for &i in &sel {
                    self.op_on(i, |p, cfg, _, _| assist::apply_order(p, cfg, cfg.reading.page_direction));
                }
            }
            if ui.small_button("Find in these").clicked() {
                self.find.scope = FindScope::SelectedPages;
                self.find.open = true;
                self.run_find();
            }
            if ui.small_button("Clear").clicked() {
                self.page_sel.clear();
            }
        });
    }
}
