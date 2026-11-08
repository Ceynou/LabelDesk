//! Review Mode: a focused, per-page workflow in passes the reviewer chooses (line boxes,
//! missing boxes, re-reading, transcriptions, character boxes, regions, containers, panels,
//! legibility, text type, confidence, the rest, reading order). Resumable, keyboard first.
//! The cards shown for each pass are in `review_cards.rs`.

use super::*;
use crate::assist::{self, CharStatus, Need, Pass, QueueItem, ReviewSession, Side, Suggest, Workflow};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RView {
    Focused,
    Context,
    Page,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TMode {
    Side,
    Overlay,
    Compare,
    ImageOnly,
}

/// Which pages a new review covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    AllPages,
    FromHere,
    ThisPage,
    /// The results of the last search.
    Found,
}

pub struct ReviewUi {
    /// Choosing the workflow before starting (no session yet).
    pub setup: bool,
    pub scope: Scope,
    pub session: ReviewSession,
    pub view: RView,
    pub tmode: TMode,
    pub active_border: bool,
    pub other_borders: bool,
    pub overlay: bool,
    pub opacity: f32,
    pub(super) framed: Option<(usize, usize, RView, [i32; 4])>,
    pub focus_editor: bool,
    pub independent: String,
    pub all_fields: bool,
    pub geometry_edit: bool,
    pub comment: String,
    pub tag: String,
}

/// A quick answer to one checklist field.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Recog(Recognizability),
    Class(String),
    Lang(String, String),
    Writing(WritingMode, Direction),
    Mirrored(Option<bool>),
    Occlusion(Option<Option<OcclusionLevel>>),
    QualityOk,
    CharsPropose,
    CharsAccept,
    CharsRegenerate,
    CharsSkip,
    OrderAccept,
    PerspectiveAccept,
    PerspectiveNa,
    TranscriptionConfirm,
    GeometryConfirm,
    Confidence(Confidence),
    Suggestion(Suggest),
}

fn script_of(lang: &str) -> &'static str {
    match lang {
        "ja" => "Jpan",
        "ko" => "Kore",
        "zh" | "zh-Hans" => "Hans",
        "zh-Hant" => "Hant",
        "ar" | "fa" => "Arab",
        "he" => "Hebr",
        "th" => "Thai",
        "ru" | "uk" => "Cyrl",
        _ => "Latn",
    }
}

/// Numbered quick options for a field (keys 1–9 pick them).
pub fn options(field: &str, a: &Annotation, page: &PageFile, cfg: &ProjectConfig) -> Vec<(String, Answer)> {
    match field {
        "recognizability" => {
            // the two-way answer first; the finer ones refine "not perfectly readable"
            let mut v: Vec<(String, Answer)> = Recognizability::COARSE.iter().map(|r| (r.name().to_string(), Answer::Recog(*r))).collect();
            v.extend(Recognizability::ALL.iter().skip(1).map(|r| (r.name().to_string(), Answer::Recog(*r))));
            v
        }
        "confidence" => vec![
            ("Certain".into(), Answer::Confidence(Confidence::High)),
            ("Uncertain".into(), Answer::Confidence(Confidence::Uncertain)),
            ("Low".into(), Answer::Confidence(Confidence::Low)),
            ("Medium".into(), Answer::Confidence(Confidence::Medium)),
        ],
        "classes" => cfg.text_classes.iter().take(9).map(|c| (c.name.clone(), Answer::Class(c.id.clone()))).collect(),
        "language" => cfg.languages.iter().take(9).map(|l| (l.clone(), Answer::Lang(l.clone(), script_of(l).into()))).collect(),
        "direction" => vec![
            ("Vertical, columns ←".into(), Answer::Writing(WritingMode::VerticalRl, Direction::Ttb)),
            ("Horizontal →".into(), Answer::Writing(WritingMode::HorizontalTb, Direction::Ltr)),
            ("Horizontal ←".into(), Answer::Writing(WritingMode::HorizontalTb, Direction::Rtl)),
            ("Vertical, columns →".into(), Answer::Writing(WritingMode::VerticalLr, Direction::Ttb)),
        ],
        "mirrored" => vec![("No".into(), Answer::Mirrored(Some(false))), ("Yes".into(), Answer::Mirrored(Some(true))), ("Unknown".into(), Answer::Mirrored(None))],
        "occlusion" => vec![
            ("None".into(), Answer::Occlusion(Some(None))),
            ("Partial".into(), Answer::Occlusion(Some(Some(OcclusionLevel::Partial)))),
            ("Severe".into(), Answer::Occlusion(Some(Some(OcclusionLevel::Severe)))),
            ("Unknown".into(), Answer::Occlusion(None)),
        ],
        "quality" => vec![("No issues".into(), Answer::QualityOk)],
        "char_boxes" => {
            let mut v = Vec::new();
            if !assist::chars_of(page, &a.id).is_empty() {
                v.push(("Accept".into(), Answer::CharsAccept));
            } else {
                v.push(("Propose".into(), Answer::CharsPropose));
            }
            v.push(("Regenerate".into(), Answer::CharsRegenerate));
            v.push(("Not needed".into(), Answer::CharsSkip));
            v
        }
        "reading_order" => vec![("Accept".into(), Answer::OrderAccept)],
        "perspective" => vec![("Accept".into(), Answer::PerspectiveAccept), ("Not applicable".into(), Answer::PerspectiveNa)],
        "transcription" => vec![("Correct as shown".into(), Answer::TranscriptionConfirm)],
        "geometry" => vec![("Shape is right".into(), Answer::GeometryConfirm)],
        _ => Vec::new(),
    }
}

fn empty_session(workflow: Workflow, user: String) -> ReviewSession {
    ReviewSession {
        workflow,
        pages: Vec::new(),
        filter: None,
        page: 0,
        items: Vec::new(),
        pos: 0,
        done: Default::default(),
        known: Default::default(),
        completed_pages: Default::default(),
        reviewed: 0,
        started: util::now(),
        user,
    }
}

impl App {
    fn session_path(&self) -> Option<PathBuf> {
        let s = self.store.as_ref()?;
        Some(s.root.join(".labeldesk").join("review").join(format!("{}.json", util::slug(&self.settings.user))))
    }

    pub fn save_session(&self) {
        if let (Some(r), Some(p)) = (&self.review, self.session_path()) {
            if !r.setup {
                let _ = util::atomic_write(&p, to_json(&r.session).as_bytes());
            }
        }
    }

    /// F6: resume the unfinished review, or show the workflow choices.
    pub fn enter_review(&mut self, ctx: &egui::Context) {
        if self.store.is_none() {
            return;
        }
        self.commit();
        let saved: Option<ReviewSession> = self.session_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok());
        let resume = saved.filter(|s| !s.finished() && !s.items.is_empty());
        let setup = resume.is_none();
        let session = resume.unwrap_or_else(|| empty_session(self.settings.workflow.clone(), self.user()));
        self.review = Some(ReviewUi {
            setup,
            scope: if self.find.results.is_empty() { Scope::AllPages } else { Scope::Found },
            session,
            view: RView::Focused,
            tmode: TMode::Side,
            active_border: true,
            other_borders: false,
            overlay: false,
            opacity: 0.7,
            framed: None,
            focus_editor: false,
            independent: String::new(),
            all_fields: false,
            geometry_edit: false,
            comment: String::new(),
            tag: String::new(),
        });
        self.set_tool(Tool::Select);
        if self.settings.review_fullscreen && self.screenshot.is_none() && !cfg!(test) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        if setup {
            self.selection.clear();
            if let Some(r) = &mut self.review {
                r.view = RView::Page;
            }
            self.flash("Choose the passes of your review, then press Enter to start", ctx);
        } else {
            let pos = self.review.as_ref().map(|r| r.session.pos).unwrap_or(0);
            self.enter_page_of_session();
            self.review_goto(pos);
            let (p, n) = self.review.as_ref().map(|r| (r.session.page + 1, r.session.pages.len())).unwrap_or((0, 0));
            self.flash(format!("Resumed your review: page {p} of {n}"), ctx);
        }
    }

    pub fn exit_review(&mut self) {
        self.commit();
        self.save_session();
        self.review = None;
        self.set_tool(Tool::Select);
        self.view.fitted = true;
        if self.settings.review_fullscreen && self.screenshot.is_none() && !cfg!(test) {
            self.leave_fullscreen = true;
        }
    }

    /// Start a new review of `scope` with the person's workflow.
    pub fn start_review(&mut self, scope: Scope) {
        if self.review.is_none() {
            return;
        }
        self.commit();
        let wf = self.settings.workflow.clone();
        let filter: Option<BTreeSet<String>> = (scope == Scope::Found && !self.find.results.is_empty()).then(|| self.find.results.iter().map(|r| r.1.clone()).collect());
        let all: Vec<String> = self.pages().iter().map(|p| p.page_id.clone()).collect();
        let pages: Vec<String> = match scope {
            Scope::AllPages => all,
            Scope::FromHere => all[self.page.min(all.len())..].to_vec(),
            Scope::ThisPage => all.get(self.page).cloned().into_iter().collect(),
            Scope::Found => {
                let f = filter.clone().unwrap_or_default();
                self.pages().iter().filter(|p| p.annotations.iter().any(|a| f.contains(&a.id))).map(|p| p.page_id.clone()).collect()
            }
        };
        let mut s = empty_session(wf, self.user());
        s.pages = pages;
        s.filter = filter;
        if let Some(r) = &mut self.review {
            r.session = s;
            r.setup = false;
            r.scope = scope;
        }
        self.begin_page();
    }

    /// Build the steps of the session's current page (moving on past pages with nothing to do).
    fn begin_page(&mut self) {
        loop {
            let Some(r) = &self.review else { return };
            let s = &r.session;
            let Some(pid) = s.pages.get(s.page).cloned() else {
                if let Some(r) = &mut self.review {
                    r.session.items.clear();
                    r.session.pos = 0;
                }
                self.save_session();
                return;
            };
            let Some(idx) = self.store.as_ref().and_then(|st| st.index_of(&pid)) else {
                if let Some(r) = &mut self.review {
                    r.session.page += 1;
                }
                continue;
            };
            let cfg = self.cfg();
            let page = &self.pages()[idx];
            let items = assist::page_steps(page, &cfg, &s.workflow.order(), s.filter.as_ref(), s.workflow.skip_checked);
            let known = page.annotations.iter().map(|a| a.id.clone()).collect();
            if items.is_empty() {
                if let Some(r) = &mut self.review {
                    r.session.page += 1;
                }
                continue;
            }
            if let Some(r) = &mut self.review {
                r.session.items = items;
                r.session.pos = 0;
                r.session.done.clear();
                r.session.known = known;
            }
            self.go_to(idx);
            self.review_goto(0);
            return;
        }
    }

    fn enter_page_of_session(&mut self) {
        let Some(pid) = self.review.as_ref().and_then(|r| r.session.pages.get(r.session.page).cloned()) else { return };
        if let Some(i) = self.store.as_ref().and_then(|s| s.index_of(&pid)) {
            self.go_to(i);
        }
    }

    /// The page's review is complete: save it, commit it when the project uses Git, go on.
    fn finish_page(&mut self) {
        self.commit();
        let Some(pid) = self.cur_id() else { return };
        let user = self.user();
        if let Some(s) = self.store.as_mut() {
            let _ = s.save_page(&pid, &user);
            if s.manifest_dirty {
                let _ = s.save_all(&user);
            }
        }
        let mut msg = format!("Page {pid} reviewed");
        if self.settings.git_commit_reviewed {
            if let Some(s) = &self.store {
                if crate::git::is_repo(&s.root) {
                    let files = crate::git::page_files(s, &pid);
                    match crate::git::commit_files(&s.root, &files, &format!("Review page {pid}"), &user) {
                        Ok(Some(id)) => msg = format!("Page {pid} reviewed and committed ({id})"),
                        Ok(None) => {}
                        Err(e) => msg = format!("Page {pid} reviewed; the commit failed: {e}"),
                    }
                }
            }
        }
        if let Some(r) = &mut self.review {
            r.session.completed_pages.insert(pid);
            r.session.page += 1;
        }
        self.message = Some((msg, 0.0, false));
        self.begin_page();
    }

    pub fn review_item(&self) -> Option<QueueItem> {
        let r = self.review.as_ref()?;
        r.session.items.get(r.session.pos).cloned()
    }

    pub fn review_pass(&self) -> Option<Pass> {
        self.review.as_ref().filter(|r| !r.setup).and_then(|r| r.session.pass())
    }

    /// Shapes can be edited now (always outside review mode).
    pub fn review_edits(&self) -> bool {
        match &self.review {
            None => true,
            Some(r) => r.geometry_edit || self.review_pass().is_some_and(|p| p.edits_shapes()),
        }
    }

    /// Labels drawn while reviewing (`None`: all of them): the reviewed label with everything
    /// inside it, and the selection with its line.
    pub fn review_focus(&self) -> Option<std::collections::HashSet<String>> {
        let r = self.review.as_ref()?;
        if r.other_borders || r.setup {
            return None;
        }
        let it = self.review_item()?;
        let page = self.cur()?;
        let mut s: std::collections::HashSet<String> = self.selection.iter().cloned().collect();
        let id = it.annotation_id?;
        s.extend(assist::descendants(page, &id));
        s.insert(id);
        for id in &self.selection {
            if let Some(p) = page.get(id).and_then(|a| a.parent.clone()) {
                s.extend(assist::descendants(page, &p));
                s.insert(p);
            }
        }
        Some(s)
    }

    fn item_exists(&self, it: &QueueItem) -> bool {
        let Some(s) = &self.store else { return false };
        match (s.index_of(&it.page_id), &it.annotation_id) {
            (Some(i), Some(a)) => s.pages[i].get(a).is_some(),
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// Go to step `pos` of the page (skipping labels deleted meanwhile) and frame it. Past the
    /// last step the page is finished.
    pub fn review_goto(&mut self, pos: usize) {
        self.commit();
        let Some(r) = &self.review else { return };
        let n = r.session.items.len();
        let mut p = pos;
        while p < n && !self.item_exists(&self.review.as_ref().unwrap().session.items[p]) {
            p += 1;
        }
        if p >= n {
            if n > 0 {
                self.finish_page();
            }
            return;
        }
        let it = self.review.as_ref().unwrap().session.items[p].clone();
        if let Some(i) = self.store.as_ref().and_then(|s| s.index_of(&it.page_id)) {
            self.go_to(i);
        }
        self.selection = it.annotation_id.clone().into_iter().collect();
        self.draft.clear();
        match it.pass.draw_level() {
            Some(l) if it.pass != Pass::Regions => {
                self.draw_level = l;
                self.draw_class = None;
                self.set_tool(Tool::Box);
            }
            _ => self.set_tool(Tool::Select),
        }
        if it.pass == Pass::ReadingOrder {
            self.show_order = true;
            self.prepare_order();
        }
        if let Some(r) = &mut self.review {
            r.session.pos = p;
            r.framed = None;
            r.focus_editor = false;
            r.independent.clear();
            r.geometry_edit = false;
            r.view = if it.annotation_id.is_none() {
                RView::Page
            } else if matches!(it.pass, Pass::Legibility | Pass::TextType) {
                RView::Context
            } else if r.view == RView::Page {
                RView::Focused
            } else {
                r.view
            };
            r.other_borders = it.annotation_id.is_none() || it.pass == Pass::LineBoxes;
        }
        self.save_session();
    }

    /// Leaving a page step: steps after it are rebuilt so new boxes are reviewed too.
    fn rebuild_after_page_step(&mut self) {
        let Some(it) = self.review_item() else { return };
        if !it.pass.per_page() {
            return;
        }
        let Some(r) = &self.review else { return };
        let order = r.session.workflow.order();
        let Some(k) = order.iter().position(|p| *p == it.pass) else { return };
        let later: Vec<Pass> = order[k + 1..].to_vec();
        let earlier_items: Vec<Pass> = order[..k].iter().copied().filter(|p| !p.per_page()).collect();
        let (filter, skip, known, pos) = (r.session.filter.clone(), r.session.workflow.skip_checked, r.session.known.clone(), r.session.pos);
        let cfg = self.cfg();
        let Some(page) = self.cur() else { return };
        let mut extra = Vec::new();
        // boxes added at the end of the page go through the item passes they missed
        if it.pass == Pass::MissingBoxes {
            let new: BTreeSet<String> = page.annotations.iter().filter(|a| !known.contains(&a.id)).map(|a| a.id.clone()).filter(|id| filter.as_ref().is_none_or(|f| f.contains(id))).collect();
            if !new.is_empty() {
                extra = assist::page_steps(page, &cfg, &earlier_items, Some(&new), skip);
            }
        }
        let rest = assist::page_steps(page, &cfg, &later, filter.as_ref(), skip);
        let ids: BTreeSet<String> = page.annotations.iter().map(|a| a.id.clone()).collect();
        if let Some(r) = &mut self.review {
            r.session.items.truncate(pos + 1);
            r.session.items.extend(extra);
            r.session.items.extend(rest);
            if it.pass == Pass::MissingBoxes {
                r.session.known = ids;
            }
        }
    }

    /// Move on (`confirmed`: this step was answered).
    pub fn review_next(&mut self, confirmed: bool) {
        if let Some(r) = &mut self.review {
            if confirmed {
                r.session.done.insert(r.session.pos);
                r.session.reviewed += 1;
            }
        }
        self.rebuild_after_page_step();
        let pos = self.review.as_ref().map(|r| r.session.pos + 1).unwrap_or(0);
        self.review_goto(pos);
    }

    pub fn review_step(&mut self, delta: i32) {
        let Some(r) = &self.review else { return };
        if delta > 0 {
            self.review_next(false);
            return;
        }
        let mut q = (r.session.pos as i32 + delta).max(0) as usize;
        while q > 0 && !self.item_exists(&self.review.as_ref().unwrap().session.items[q]) {
            q -= 1;
        }
        self.review_goto(q);
    }

    /// Jump to the first step of the next pass on this page.
    pub fn skip_pass(&mut self) {
        let Some(r) = &self.review else { return };
        let Some(cur) = r.session.pass() else { return };
        let pos = r.session.pos;
        self.rebuild_after_page_step();
        let Some(r) = &self.review else { return };
        let n = r.session.items.len();
        let next = r.session.items.iter().enumerate().skip(pos).find(|(_, i)| i.pass != cur).map(|(k, _)| k);
        self.review_goto(next.unwrap_or(n));
    }

    /// The annotation review actions apply to: the reviewed label (even if something else was
    /// clicked on the page), otherwise the selection.
    pub fn review_target(&self) -> Option<String> {
        match self.review_item() {
            Some(it) if self.review.is_some() => it.annotation_id.or_else(|| self.selection.first().cloned()),
            _ => self.selection.first().cloned(),
        }
    }

    /// Ids an answer applies to: a region passes it on to its lines (and words) that have no
    /// confirmed answer of their own.
    fn answer_scope(&self, id: &str, field: &str) -> Vec<String> {
        let mut v = vec![id.to_string()];
        let Some(page) = self.cur() else { return v };
        if page.get(id).is_some_and(|a| a.level == Level::Region) && matches!(field, "recognizability" | "classes" | "confidence") {
            for d in assist::descendants(page, id) {
                if let Some(a) = page.get(&d) {
                    let own = a.checks.get(field).is_some_and(|c| c.state == CheckState::Confirmed);
                    if matches!(a.level, Level::Line | Level::Word) && !own {
                        v.push(d);
                    }
                }
            }
        }
        v
    }

    pub fn answer(&mut self, field: &str, ans: Answer) {
        let Some(id) = self.review_target() else { return };
        self.commit();
        let ids = self.answer_scope(&id, field);
        let splits = self.preview_splits(&id);
        if matches!(ans, Answer::CharsPropose | Answer::CharsRegenerate) {
            self.cell_splits = None;
        }
        self.op(move |page, cfg, user, now| {
            use assist::set_check;
            match &ans {
                Answer::CharsPropose | Answer::CharsRegenerate => {
                    assist::regenerate_chars_split(page, &id, cfg, user, now, splits.as_ref());
                    return;
                }
                Answer::CharsAccept => {
                    assist::apply_suggestion(page, &id, &Suggest::AcceptChars(0), cfg, user, now);
                    return;
                }
                Answer::Suggestion(s) => {
                    assist::apply_suggestion(page, &id, s, cfg, user, now);
                    return;
                }
                _ => {}
            }
            for id in &ids {
                let Some(a) = page.get_mut(id) else { continue };
                let ok = |a: &mut Annotation, f: &str| set_check(a, f, CheckState::Confirmed, user, now, "quick answer", false);
                match &ans {
                    Answer::Recog(r) => {
                        a.recognizability = Some(*r);
                        ok(a, "recognizability");
                    }
                    Answer::Class(c) => {
                        a.classes = [c.clone()].into_iter().collect();
                        ok(a, "classes");
                    }
                    Answer::Confidence(c) => {
                        a.confidence.transcription = Some(*c);
                        a.confidence.geometry = Some(*c);
                        ok(a, "confidence");
                    }
                    Answer::Lang(l, s) => {
                        let t = a.transcription.get_or_insert_with(Default::default);
                        t.language = Some(l.clone());
                        t.script = Some(s.clone());
                        ok(a, "language");
                    }
                    Answer::Writing(wm, d) => {
                        let t = a.transcription.get_or_insert_with(Default::default);
                        t.writing_mode = *wm;
                        t.direction = *d;
                        ok(a, "direction");
                    }
                    Answer::Mirrored(Some(v)) => {
                        a.orientation.mirrored = *v;
                        ok(a, "mirrored");
                    }
                    Answer::Mirrored(None) => set_check(a, "mirrored", CheckState::Unknown, user, now, "", false),
                    Answer::Occlusion(Some(None)) => {
                        a.occlusion = Some(Occlusion { occluded: false, ..Default::default() });
                        ok(a, "occlusion");
                    }
                    Answer::Occlusion(Some(Some(l))) => {
                        let mut oc = a.occlusion.clone().unwrap_or_default();
                        oc.occluded = true;
                        oc.level = Some(*l);
                        a.occlusion = Some(oc);
                        ok(a, "occlusion");
                    }
                    Answer::Occlusion(None) => set_check(a, "occlusion", CheckState::Unknown, user, now, "", false),
                    Answer::QualityOk => ok(a, "quality"),
                    Answer::CharsSkip => set_check(a, "char_boxes", CheckState::NotApplicable, user, now, "", false),
                    Answer::OrderAccept => {
                        a.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
                        set_check(a, "reading_order", CheckState::Confirmed, user, now, "suggestion accepted", false);
                    }
                    Answer::PerspectiveAccept => ok(a, "perspective"),
                    Answer::PerspectiveNa => set_check(a, "perspective", CheckState::NotApplicable, user, now, "", false),
                    Answer::TranscriptionConfirm => set_check(a, "transcription", CheckState::Confirmed, user, now, "checked against the image", false),
                    Answer::GeometryConfirm => set_check(a, "geometry", CheckState::Confirmed, user, now, "checked against the image", false),
                    _ => {}
                }
            }
        });
    }

    pub fn current_needs(&self) -> Vec<Need> {
        let (Some(a), Some(p)) = (self.review_target().and_then(|id| self.ann(&id)), self.cur()) else { return Vec::new() };
        assist::needs(a, p, &self.cfg())
    }

    /// Rest pass: approve when nothing blocks, otherwise point at the first open field.
    pub fn review_approve_next(&mut self, ctx: &egui::Context) {
        let Some(id) = self.review_target() else { return };
        self.commit();
        let res = self.op(|page, cfg, user, now| assist::quick_approve(page, &id, cfg, user, now));
        match res {
            Some(Ok(approved)) => {
                if !approved {
                    self.flash("Checked and submitted: this project needs a different person to approve your own work", ctx);
                }
                self.review_next(true);
            }
            Some(Err(e)) => {
                if let Some(r) = &mut self.review {
                    r.all_fields = false;
                }
                self.error(format!("Not yet: {e}"));
            }
            None => {}
        }
    }

    pub fn review_flag(&mut self, ctx: &egui::Context) {
        let Some(id) = self.review_item().and_then(|i| i.annotation_id) else { return };
        self.op(|page, _, user, now| {
            if let Some(a) = page.get_mut(&id) {
                review::add_note(a, NoteKind::ChangeRequest, None, "Flagged during review for further work", user, now);
            }
        });
        self.review_next(false);
        self.flash("Flagged for further review", ctx);
    }

    /// Enter: confirm the current step the way its pass expects, then move on.
    pub fn review_confirm(&mut self, ctx: &egui::Context) {
        let Some(it) = self.review_item() else { return };
        if !self.draft.is_empty() {
            self.finish_draft();
            return;
        }
        match it.pass {
            Pass::LineBoxes => {
                self.answer("geometry", Answer::GeometryConfirm);
                self.review_next(true);
            }
            Pass::Transcription => {
                self.answer("transcription", Answer::TranscriptionConfirm);
                self.review_next(true);
            }
            Pass::CharBoxes => {
                let (Some(a), Some(p)) = (self.review_target().and_then(|id| self.ann(&id)).cloned(), self.cur().cloned()) else { return };
                match assist::char_status(&p, &a, &self.cfg()) {
                    CharStatus::None(_) | CharStatus::Stale(_) => self.answer("char_boxes", Answer::CharsRegenerate),
                    CharStatus::Proposed(_) => {
                        self.answer("char_boxes", Answer::CharsAccept);
                        self.review_next(true);
                    }
                    CharStatus::Done(_) => self.review_next(true),
                }
            }
            Pass::Legibility | Pass::TextType | Pass::Confidence => {
                let field = match it.pass {
                    Pass::Legibility => "recognizability",
                    Pass::TextType => "classes",
                    _ => "confidence",
                };
                // Enter keeps the current answer, or takes the suggestion
                let a = self.review_target().and_then(|id| self.ann(&id)).cloned();
                let current = a.as_ref().and_then(|a| match field {
                    "recognizability" => a.recognizability.map(Answer::Recog),
                    "classes" => a.classes.iter().next().cloned().map(Answer::Class),
                    _ => a.confidence.transcription.map(Answer::Confidence),
                });
                let suggested = self.current_needs().iter().find(|n| n.field == field).and_then(|n| n.suggestion.clone()).and_then(|(s, _)| match s {
                    Suggest::Recognizability(r) => Some(Answer::Recog(r)),
                    Suggest::Class(c) => Some(Answer::Class(c)),
                    _ => None,
                });
                match current.or(suggested) {
                    Some(ans) => {
                        self.answer(field, ans);
                        self.review_next(true);
                    }
                    None => self.error("Pick an answer with the number keys first"),
                }
            }
            Pass::Rest => self.review_approve_next(ctx),
            Pass::MissingBoxes => {
                self.mark_inspected(true, false);
                self.review_next(true);
            }
            Pass::Reread | Pass::Regions | Pass::Containers | Pass::Panels => self.review_next(true),
            Pass::ReadingOrder => {
                self.accept_page_order();
                self.review_next(true);
            }
        }
    }

    /// Reading order pass: suggest numbers for every ordered label of the page that has none.
    pub(super) fn prepare_order(&mut self) {
        let dir = crate::assist::page_direction(self.cur_id().and_then(|id| self.store.as_ref().and_then(|s| s.entry(&id))).map(|e| &e.source), &self.cfg());
        self.op(|p, cfg, _, _| crate::assist::apply_order(p, cfg, dir));
    }

    fn accept_page_order(&mut self) {
        self.op(|page, cfg, user, now| {
            for a in page.annotations.iter_mut() {
                if a.reading_order.is_some() && a.level != Level::Char && !assist::is_unordered(a, cfg) {
                    a.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
                    assist::set_check(a, "reading_order", CheckState::Confirmed, user, now, "reviewed in the reading order pass", false);
                }
            }
        });
    }

    /// Read every line of the page again with the AI model.
    pub fn reread_page_lines(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self.cur().map(|p| p.annotations.iter().filter(|a| a.level == Level::Line).map(|a| a.id.clone()).collect()).unwrap_or_default();
        let n = ids.len();
        self.reread(&ids);
        self.flash(format!("Reading {n} line(s) again in the background; new readings are added as alternatives"), ctx);
    }

    /// Keyboard resizing: Shift+arrow grows that side, Ctrl+arrow shrinks it, Alt+arrow moves.
    pub fn nudge_keys(&mut self, ctx: &egui::Context) -> bool {
        let Some(id) = self.selection.first().cloned() else { return false };
        let arrows = [(Key::ArrowLeft, Side::Left), (Key::ArrowRight, Side::Right), (Key::ArrowUp, Side::Top), (Key::ArrowDown, Side::Bottom)];
        let step = (1.0 / self.view.zoom as f64).clamp(1.0, 4.0).round();
        let mut hit = None;
        for (k, side) in arrows {
            for (m, d, whole) in [(Modifiers::SHIFT, step, false), (Modifiers::COMMAND, -step, false), (Modifiers::ALT, step, true)] {
                if ctx.input_mut(|i| i.consume_key(m, k)) {
                    hit = Some((side, d, whole));
                }
            }
        }
        let Some((side, d, whole)) = hit else { return false };
        let Some(g) = self.ann(&id).map(|a| a.geometry.clone()) else { return false };
        let ng = assist::nudge_shape(&g, side, d, whole);
        if let Some(a) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
            a.geometry = ng;
        }
        self.refit_live(std::slice::from_ref(&id));
        self.commit();
        true
    }

    /// Keyboard handling while reviewing.
    pub fn review_keys(&mut self, ctx: &egui::Context) {
        let sc = |app: &App, a: &str| app.shortcut(ctx, a);
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if esc && (!self.draft.is_empty() || !matches!(self.drag, Drag::None)) {
            self.draft.clear();
            self.drag = Drag::None;
            return;
        }
        if sc(self, "review_mode") || esc {
            self.exit_review();
            return;
        }
        if self.review.as_ref().is_some_and(|r| r.setup) {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                let scope = self.review.as_ref().map(|r| r.scope).unwrap_or(Scope::AllPages);
                self.start_review(scope);
            }
            return;
        }
        if self.review.as_ref().is_some_and(|r| r.session.finished()) {
            return;
        }
        let pass = self.review_pass();
        // Alt+↑/↓ reorders during the reading order pass; shape passes resize from the keyboard
        if pass.is_some_and(|p| p.edits_shapes()) {
            if self.nudge_keys(ctx) {
                return;
            }
        } else {
            if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowUp)) {
                self.nudge(-1);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::ALT, Key::ArrowDown)) {
                self.nudge(1);
            }
        }
        if self.shortcut(ctx, "reread") {
            let ids = self.review_target().into_iter().collect::<Vec<_>>();
            self.reread(&ids);
            self.flash("Reading it again in the background", ctx);
        }
        if self.shortcut(ctx, "make_regions") {
            self.make_regions(ctx);
        }
        if self.shortcut(ctx, "hide") {
            self.hide_selection(ctx);
        }
        if self.shortcut(ctx, "next_problem") {
            self.next_problem(ctx);
        }
        if sc(self, "review_approve") {
            self.review_confirm(ctx);
        }
        if sc(self, "review_next") {
            self.review_step(1);
        }
        if sc(self, "review_prev") {
            self.review_step(-1);
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::PageDown)) {
            self.skip_pass();
        }
        if sc(self, "delete") && pass.is_some_and(|p| p.edits_shapes()) {
            let target = self.review_item().and_then(|i| i.annotation_id);
            let deleting_target = target.as_ref().is_some_and(|t| self.selection.contains(t));
            self.delete_selection();
            if deleting_target {
                let pos = self.review.as_ref().map(|r| r.session.pos).unwrap_or(0);
                self.review_goto(pos);
            }
        }
        if sc(self, "review_flag") {
            self.review_flag(ctx);
        }
        let drawing = pass.and_then(|p| p.draw_level()).is_some();
        if drawing {
            for (k, t) in [(Key::V, Tool::Select), (Key::B, Tool::Box), (Key::Q, Tool::Quad), (Key::L, Tool::Polygon)] {
                if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k)) {
                    self.set_tool(t);
                }
            }
        }
        if sc(self, "accept_ai") {
            let needs = self.current_needs();
            let blocking = assist::blocking(&needs, &self.cfg().quick).into_iter().cloned().collect::<Vec<_>>();
            if let Some((s, _)) = blocking.iter().chain(needs.iter()).find_map(|n| n.suggestion.clone()) {
                self.answer("", Answer::Suggestion(s));
            } else if self.review_target().and_then(|id| self.ann(&id)).is_some_and(|a| a.review.state == ReviewState::AiProposed) {
                self.selection = self.review_target().into_iter().collect();
                self.accept_ai();
            }
        }
        if sc(self, "reject_ai") && self.review_target().and_then(|id| self.ann(&id)).is_some_and(|a| a.provenance.origin == Origin::Ai) {
            self.selection = self.review_target().into_iter().collect();
            self.reject_ai("rejected in review");
            self.review_next(false);
        }
        if sc(self, "review_chars") {
            let (Some(a), Some(p)) = (self.review_target().and_then(|id| self.ann(&id)).cloned(), self.cur().cloned()) else { return };
            if a.level != Level::Char {
                match assist::char_status(&p, &a, &self.cfg()) {
                    CharStatus::None(_) | CharStatus::Stale(_) => self.answer("char_boxes", Answer::CharsRegenerate),
                    CharStatus::Proposed(_) => self.answer("char_boxes", Answer::CharsAccept),
                    CharStatus::Done(_) => {}
                }
            }
        }
        let Some(r) = &mut self.review else { return };
        let pressed = |a: &str| parse_shortcut(&self.settings.shortcut(a)).is_some_and(|(m, k)| ctx.input_mut(|i| i.consume_key(m, k)));
        if pressed("review_edit") {
            r.focus_editor = true;
        }
        if pressed("review_overlay") {
            r.overlay = !r.overlay;
            if r.overlay && r.tmode == TMode::ImageOnly {
                r.tmode = TMode::Side;
            }
        }
        if !drawing && pressed("review_borders") {
            r.active_border = !r.active_border;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::O)) {
            r.other_borders = !r.other_borders;
        }
        if pressed("review_view") {
            r.view = if r.view == RView::Focused { RView::Context } else { RView::Focused };
            r.framed = None;
        }
        if pressed("review_page") {
            r.view = if r.view == RView::Page { RView::Focused } else { RView::Page };
            r.framed = None;
        }
        if pressed("review_geometry") {
            r.geometry_edit = !r.geometry_edit;
        }
        if pressed("review_meta") {
            r.all_fields = !r.all_fields;
        }
        // number keys answer the pass question (or the first open field)
        let nums = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        let hit = nums.iter().position(|k| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, *k)));
        if let Some(k) = hit {
            let field = match pass {
                Some(Pass::Legibility) => Some("recognizability"),
                Some(Pass::TextType) => Some("classes"),
                Some(Pass::Confidence) => Some("confidence"),
                _ => {
                    let needs = self.current_needs();
                    let cfg = self.cfg();
                    assist::blocking(&needs, &cfg.quick).first().map(|n| n.field).or(needs.first().map(|n| n.field))
                }
            };
            let cfg = self.cfg();
            if let (Some(f), Some(a), Some(page)) = (field, self.review_target().and_then(|id| self.ann(&id)).cloned(), self.cur().cloned()) {
                if let Some((_, ans)) = options(f, &a, &page, &cfg).get(k) {
                    self.answer(f, ans.clone());
                    // the one-question passes move on by themselves
                    if matches!(pass, Some(Pass::Legibility | Pass::TextType | Pass::Confidence)) {
                        self.review_next(true);
                    }
                }
            }
        }
    }

    fn nudge(&mut self, d: i32) {
        let Some(id) = self.selection.first().cloned().or_else(|| self.review_target()) else { return };
        self.commit();
        self.op(|p, _, _, _| assist::nudge_order(p, &id, d));
    }

    /// Frame the current item in the canvas for the chosen view.
    pub fn review_frame(&mut self, rect: Rect, iw: f64, ih: f64) {
        let Some(r) = &self.review else { return };
        let key = (r.session.page, r.session.pos, r.view, [rect.min.x as i32, rect.min.y as i32, rect.width() as i32, rect.height() as i32]);
        if r.framed == Some(key) {
            return;
        }
        let margin = self.cfg().quick.zoom_margin as f64;
        let target_ann = self.review_item().and_then(|i| i.annotation_id).and_then(|id| self.ann(&id).cloned());
        let target = match (r.view, target_ann.as_ref()) {
            (RView::Page, _) | (_, None) => ([0.0, 0.0], [iw, ih]),
            (RView::Focused, Some(a)) => {
                let (lo, hi) = a.geometry.bounds();
                let m = ((hi[0] - lo[0]).max(hi[1] - lo[1]) * margin).max(16.0);
                ([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m])
            }
            (RView::Context, Some(a)) => {
                // the surrounding panel when there is one, else three times the annotation
                let mut cur = a.parent.clone();
                let mut ctxr = None;
                while let Some(pid) = cur {
                    let Some(p) = self.ann(&pid) else { break };
                    if p.level == Level::Panel {
                        ctxr = Some(p.geometry.bounds());
                        break;
                    }
                    cur = p.parent.clone();
                }
                let (lo, hi) = a.geometry.bounds();
                let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
                let around = ([lo[0] - w * 1.5 - 40.0, lo[1] - h * 1.0 - 40.0], [hi[0] + w * 1.5 + 40.0, hi[1] + h * 1.0 + 40.0]);
                match ctxr {
                    Some((plo, phi)) => ([plo[0].min(around.0[0]) - 10.0, plo[1].min(around.0[1]) - 10.0], [phi[0].max(around.1[0]) + 10.0, phi[1].max(around.1[1]) + 10.0]),
                    None => around,
                }
            }
        };
        let (lo, hi) = target;
        let (bw, bh) = ((hi[0] - lo[0]).max(1.0) as f32, (hi[1] - lo[1]).max(1.0) as f32);
        let zoom = ((rect.width() - 30.0) / bw).min((rect.height() - 70.0) / bh).clamp(0.02, 12.0);
        let c = vec2(((lo[0] + hi[0]) / 2.0) as f32, ((lo[1] + hi[1]) / 2.0) as f32);
        self.view.zoom = zoom;
        self.view.offset = rect.size() / 2.0 - c * zoom + vec2(0.0, 20.0);
        self.view.fitted = false;
        if let Some(r) = &mut self.review {
            r.framed = Some(key);
        }
    }
}
