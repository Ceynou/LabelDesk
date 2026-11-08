//! The desktop workspace.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Color32, CornerRadius, Key, Margin, Modifiers, Panel, Rect, Stroke, TextureHandle, TextureOptions, Ui, Vec2, vec2};
use serde::{Deserialize, Serialize};

use crate::ai::{self, Worker};
use crate::geom::{self, P};
use crate::ocr::ModelRef;
use crate::review;
use crate::schema::*;
use crate::store::{RecoveryItem, Store};
use crate::util;
use crate::validate::{self, Issue};

mod canvas;
mod dialogs;
mod inspector;
mod batch;
mod panels;
mod review_mode;
mod review_cards;
mod find;
pub mod models_window;
#[cfg(test)]
mod tests;

pub const ACCENT: Color32 = Color32::from_rgb(92, 112, 240);
pub const CANVAS_BG: Color32 = Color32::from_rgb(22, 23, 28);
pub const PANEL_BG: Color32 = Color32::from_rgb(33, 35, 42);
pub const PANEL_BG2: Color32 = Color32::from_rgb(40, 42, 50);
pub const MUTED: Color32 = Color32::from_rgb(140, 146, 160);
pub const GOOD: Color32 = Color32::from_rgb(90, 200, 120);
pub const WARN: Color32 = Color32::from_rgb(240, 180, 60);
pub const BAD: Color32 = Color32::from_rgb(240, 110, 100);

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn level_color(l: Level) -> Color32 {
    match l {
        Level::Page => MUTED,
        Level::Panel => Color32::from_rgb(150, 156, 172),
        Level::Container => Color32::from_rgb(66, 165, 245),
        Level::Region => Color32::from_rgb(38, 198, 218),
        Level::Line => Color32::from_rgb(255, 167, 38),
        Level::Word => Color32::from_rgb(156, 204, 101),
        Level::Char => Color32::from_rgb(236, 64, 122),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Pan,
    Box,
    Quad,
    Polygon,
    Order,
    Link,
    AiRegion,
}

/// Which name tags are drawn on the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShowLabels {
    All,
    /// Only where they fit; fewer on crowded pages.
    Auto,
    Selected,
}

/// When character boxes are drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShowChars {
    Always,
    /// When zoomed in enough to see them, and always for the selected line.
    Auto,
    /// Only for the selected line.
    SelectedLine,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Text,
    Review,
    Flatten,
    Page,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageFilter {
    All,
    NeedsWork,
    AiProposed,
    NotInspected,
    Complete,
}

/// Actions that can have a keyboard shortcut.
pub const ACTIONS: [(&str, &str, &str); 46] = [
    ("review_mode", "Enter or leave Review Mode", "F6"),
    ("review_approve", "Review: approve and next", "Enter"),
    ("review_next", "Review: next annotation", "ArrowDown"),
    ("review_prev", "Review: previous annotation", "ArrowUp"),
    ("review_edit", "Review: edit the transcription", "E"),
    ("review_overlay", "Review: transcription overlay", "T"),
    ("review_borders", "Review: annotation border", "B"),
    ("review_view", "Review: focus / context view", "Space"),
    ("review_page", "Review: whole page view", "P"),
    ("review_flag", "Review: flag for more work", "F"),
    ("review_chars", "Review: propose / accept character boxes", "C"),
    ("review_geometry", "Review: edit the shape", "G"),
    ("review_meta", "Review: all fields", "M"),
    ("tighten", "Tighten the selected shape to its text", "Ctrl+T"),
    ("propose_chars", "Propose / accept character boxes", "C"),
    ("make_regions", "Group nearby lines into a text region", "Ctrl+G"),
    ("tool_select", "Select tool", "V"),
    ("tool_pan", "Move view tool", "G"),
    ("tool_box", "Box tool", "B"),
    ("tool_quad", "Quadrilateral tool", "Q"),
    ("tool_polygon", "Polygon tool", "P"),
    ("tool_order", "Reading order tool", "O"),
    ("tool_link", "Link to parent tool", "K"),
    ("tool_ai_region", "AI on a region tool", "R"),
    ("next_page", "Next page", "D"),
    ("prev_page", "Previous page", "A"),
    ("next_work", "Next item needing work", "N"),
    ("delete", "Delete selection", "Delete"),
    ("split", "Split selection", "S"),
    ("merge", "Merge selection", "M"),
    ("accept_ai", "Accept AI proposal", "Y"),
    ("reject_ai", "Reject AI proposal", "X"),
    ("submit", "Submit for review", "Ctrl+Enter"),
    ("approve", "Approve all aspects", "Ctrl+Shift+Enter"),
    ("ai_page", "Run AI on page", "Ctrl+R"),
    ("fit", "Fit page", "F"),
    ("save", "Save", "Ctrl+S"),
    ("undo", "Undo", "Ctrl+Z"),
    ("redo", "Redo", "Ctrl+Y"),
    ("inspected", "Mark page inspected and complete", "Ctrl+I"),
    ("reread", "Read the selection again with AI", "Ctrl+Shift+R"),
    ("ai_chars", "Find the characters of the selected lines with AI", "Ctrl+Shift+K"),
    ("hide", "Hide the selection (nothing selected: show all)", "H"),
    ("show_hidden", "Show hidden labels again", "Shift+H"),
    ("next_problem", "Next label with a problem", "F8"),
    ("find", "Find labels", "Ctrl+F"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub user: String,
    #[serde(default)]
    pub recent: Vec<PathBuf>,
    #[serde(default = "default_autosave")]
    pub autosave_secs: u32,
    #[serde(default)]
    pub shortcuts: BTreeMap<String, String>,
    /// Review mode switches the window to full screen.
    #[serde(default = "yes")]
    pub review_fullscreen: bool,
    /// The passes of Review Mode and their order options.
    #[serde(default)]
    pub workflow: crate::assist::Workflow,
    /// Commit a page to Git once its review is complete (projects inside a Git repository).
    #[serde(default = "yes")]
    pub git_commit_reviewed: bool,
    /// Model variants, where each model runs, and runtime locations on this computer.
    #[serde(default)]
    pub models: crate::zoo::Options,
}

/// "3 panels, 5 balloons and 12 text regions": what a run added, by level.
pub fn run_summary(run: &InferenceRun) -> String {
    let mut counts: BTreeMap<Level, usize> = BTreeMap::new();
    for p in run.predictions.iter().filter(|p| p.status == PredictionStatus::Proposed && p.annotation_id.is_some()) {
        *counts.entry(p.level.unwrap_or(Level::Line)).or_default() += 1;
    }
    let name = |l: Level, n: usize| {
        let s = match l {
            Level::Panel => "panel",
            Level::Container => "balloon or caption",
            Level::Region => "text region",
            Level::Line => "line",
            Level::Word => "word",
            Level::Char => "character box",
            Level::Page => "page",
        };
        if n == 1 { format!("1 {s}") } else if s.ends_with("caption") { format!("{n} balloons or captions") } else { format!("{n} {s}{}", if s.ends_with('x') { "es" } else { "s" }) }
    };
    let parts: Vec<String> = counts.into_iter().map(|(l, n)| name(l, n)).collect();
    match parts.len() {
        0 => "nothing new".into(),
        1 => parts[0].clone(),
        _ => format!("{} and {}", parts[..parts.len() - 1].join(", "), parts[parts.len() - 1]),
    }
}

fn yes() -> bool {
    true
}

fn default_autosave() -> u32 {
    20
}

impl Default for Settings {
    fn default() -> Self {
        let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "annotator".into());
        Settings { user: util::slug(&user), recent: Vec::new(), autosave_secs: 20, shortcuts: BTreeMap::new(), review_fullscreen: true, workflow: Default::default(), git_commit_reviewed: true, models: Default::default() }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        let home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(home.join("labeldesk").join("settings.json"))
    }
    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        if let Some(p) = Self::path() {
            let _ = util::atomic_write(&p, to_json(self).as_bytes());
        }
    }
    pub fn shortcut(&self, action: &str) -> String {
        self.shortcuts.get(action).cloned().unwrap_or_else(|| ACTIONS.iter().find(|a| a.0 == action).map(|a| a.2.to_string()).unwrap_or_default())
    }
}

/// Parse "Ctrl+Shift+S" into modifiers and key.
pub fn parse_shortcut(s: &str) -> Option<(Modifiers, Key)> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part.trim() {
            "Ctrl" | "Cmd" => m = m | Modifiers::COMMAND,
            "Shift" => m = m | Modifiers::SHIFT,
            "Alt" => m = m | Modifiers::ALT,
            k => key = Key::from_name(k),
        }
    }
    Some((m, key?))
}

pub struct View {
    pub zoom: f32,
    pub offset: Vec2,
    pub fitted: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Handle {
    /// Box: 0..4 corners TL TR BR BL, 4..8 edges T R B L (in the box's own frame).
    BoxH(u8),
    Rotate,
    Corner(usize),
    Vertex(usize),
    /// Corner of the rectification quadrilateral.
    Rect(usize),
    /// Edge i (point i to point i+1) of a quadrilateral or polygon.
    Edge(usize),
}

pub enum Drag {
    None,
    Pan,
    Marquee { start: P },
    NewShape { start: P },
    Move { last: P },
    Handle { id: String, handle: Handle },
    /// Border between two character cells of the selected line.
    Cell(crate::assist::CellBorder),
    /// Free transform of a quadrilateral corner: 0 distort, 1 skew (its edge moves along), 2 perspective.
    Transform { id: String, corner: usize, start: P, orig: [P; 4], mode: u8 },
}

pub enum ModelStatus {
    Unknown,
    Ready(ModelRef),
    Missing(String),
}

pub struct Preview {
    pub key: (String, [P; 4], [u32; 2]),
    pub flat: Arc<image::RgbaImage>,
    pub texture: TextureHandle,
}

#[derive(Default)]
pub struct Dialogs {
    pub new_project: Option<(String, PathBuf, Requirement)>,
    pub settings: Option<ProjectConfig>,
    pub release: Option<crate::export::ReleaseOptions>,
    pub recovery: Vec<RecoveryItem>,
    pub conflicts: Vec<(String, Box<PageFile>)>,
    pub user: Option<String>,
    pub validation: bool,
    pub shortcuts: Option<Option<String>>,
    pub reject: Option<(String, String)>,
    pub note: Option<(String, NoteKind, Option<Dimension>, String)>,
    pub welcome: bool,
    pub progress: bool,
    pub migrate_types: bool,
}

pub struct App {
    pub store: Option<Store>,
    pub settings: Settings,
    pub page: usize,
    pub images: HashMap<String, Arc<image::RgbaImage>>,
    pub textures: HashMap<String, TextureHandle>,
    pub thumbs: HashMap<String, Option<TextureHandle>>,
    pub tool: Tool,
    pub draw_level: Level,
    pub draw_class: Option<String>,
    pub selection: Vec<String>,
    pub hovered: Option<String>,
    pub view: View,
    pub view_center: Vec2,
    pub canvas_rect: Rect,
    pub drag: Drag,
    pub draft: Vec<P>,
    pub undo: Vec<PageFile>,
    pub redo: Vec<PageFile>,
    baseline: Option<PageFile>,
    pub tab: Tab,
    pub layers: BTreeMap<Level, bool>,
    pub show_order: bool,
    pub labels: ShowLabels,
    pub chars: ShowChars,
    /// Edge handles on quadrilaterals and polygons (boxes always have them).
    pub edge_handles: bool,
    commit_pending: bool,
    /// Untouched character proposals refitted to their line during a drag (not a person's edit).
    pub refit: std::collections::HashSet<String>,
    /// Cached tightening suggestion: (annotation id, its geometry, suggestion).
    pub tighten_cache: Option<(String, Geometry, Option<Geometry>)>,
    pub region_cache: Option<((String, usize, usize), Vec<crate::assist::RegionSuggestion>)>,
    /// Tree rows in the page list whose default open/closed state was flipped.
    pub tree_toggled: std::collections::HashSet<String>,
    /// Character cell borders chosen in the preview: (line id, its text, borders).
    pub cell_splits: Option<(String, String, crate::assist::Splits)>,
    /// Shapes a person converted to a quadrilateral on purpose: not turned back into boxes.
    pub keep_form: std::collections::HashSet<String>,
    /// Leave full screen on the next frame (after review mode ends).
    pub leave_fullscreen: bool,
    pub fill: f32,
    pub page_filter: PageFilter,
    pub search: String,
    pub preview: Option<Preview>,
    pub worker: Worker,
    pub model: ModelStatus,
    /// The Models window (runtimes, model files, conversions) and what the zoo models loaded as.
    pub models_ui: models_window::ModelsUi,
    /// Text masks of pages (from Koharu text masks), loaded on demand; shown over the page.
    pub text_masks: HashMap<String, Arc<image::GrayImage>>,
    pub show_text_mask: bool,
    pub message: Option<(String, f64, bool)>,
    pub dialogs: Dialogs,
    pub issues: Vec<Issue>,
    pub page_checks: HashMap<String, (Vec<Issue>, crate::assist::Metrics)>,
    pub metrics: crate::assist::Metrics,
    issues_dirty: bool,
    pub order_next: u32,
    pub order_parent: Option<Option<String>>,
    pub link_relation: Option<String>,
    pub text_sel: Option<(usize, usize)>,
    pub new_translation: (String, String),
    last_recovery: f64,
    last_autosave: f64,
    pub cursor_img: Option<P>,
    pub screenshot: Option<PathBuf>,
    frame_no: u32,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
    pub allow_close: bool,
    pub confirm_close: bool,
    pub review: Option<review_mode::ReviewUi>,
    pub find: find::Find,
    /// Labels hidden for now (this session only).
    pub hidden: std::collections::HashSet<String>,
    /// Pages selected in the page list (Ctrl+click) for bulk actions.
    pub page_sel: BTreeSet<usize>,
    /// Outline labels with validation problems.
    pub show_problems: bool,
    /// Bring the selection into view on the next frame.
    pub center_selection: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<PathBuf>, open: Option<PathBuf>) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        let mut app = Self::base(&cc.egui_ctx, screenshot.clone());
        if screenshot.is_some() {
            let dir = std::env::temp_dir().join(format!("labeldesk-screenshot-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            match crate::sample::create_project(&dir, &app.settings.user) {
                Ok(s) => app.set_store(s),
                Err(e) => eprintln!("sample project: {e}"),
            }
            app.screenshot_state();
            if std::env::var_os("LABELDESK_SHOT_MODELS").is_some() {
                app.models_ui.open = true;
            }
            return app;
        }
        let target = open.or_else(|| app.settings.recent.first().cloned().filter(|p| p.join("project.json").exists()));
        match target {
            Some(p) => app.open_project(&p),
            None => app.dialogs.welcome = true,
        }
        app
    }

    pub fn base(ctx: &egui::Context, screenshot: Option<PathBuf>) -> Self {
        setup_style(ctx);
        let repaint_ctx = ctx.clone();
        let mut layers = BTreeMap::new();
        for l in Level::DRAWABLE {
            layers.insert(l, true);
        }
        let mut settings = Settings::load();
        let shot_models = screenshot.is_some() && std::env::var_os("LABELDESK_SHOT_MODELS").is_some();
        if screenshot.is_some() || cfg!(test) {
            let models = settings.models.clone();
            settings = Settings::default();
            settings.user = "you".into();
            if shot_models {
                // picture of the Models window as this computer sees it
                settings.models = models;
            }
        }
        App {
            store: None,
            settings,
            page: 0,
            images: HashMap::new(),
            textures: HashMap::new(),
            thumbs: HashMap::new(),
            tool: Tool::Select,
            draw_level: Level::Line,
            draw_class: None,
            selection: Vec::new(),
            hovered: None,
            view: View { zoom: 1.0, offset: Vec2::ZERO, fitted: true },
            view_center: Vec2::ZERO,
            canvas_rect: Rect::NOTHING,
            drag: Drag::None,
            draft: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            baseline: None,
            tab: Tab::Text,
            layers,
            show_order: true,
            labels: ShowLabels::Auto,
            chars: ShowChars::Auto,
            edge_handles: true,
            commit_pending: false,
            refit: Default::default(),
            tighten_cache: None,
            region_cache: None,
            tree_toggled: Default::default(),
            cell_splits: None,
            keep_form: Default::default(),
            leave_fullscreen: false,
            fill: 0.16,
            page_filter: PageFilter::All,
            search: String::new(),
            preview: None,
            worker: Worker::start(move || repaint_ctx.request_repaint()),
            model: ModelStatus::Unknown,
            models_ui: Default::default(),
            text_masks: HashMap::new(),
            show_text_mask: false,
            message: None,
            dialogs: Dialogs::default(),
            issues: Vec::new(),
            page_checks: HashMap::new(),
            metrics: Default::default(),
            issues_dirty: true,
            order_next: 1,
            order_parent: None,
            link_relation: None,
            text_sel: None,
            new_translation: (String::new(), String::new()),
            last_recovery: 0.0,
            last_autosave: 0.0,
            cursor_img: None,
            screenshot,
            frame_no: 0,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
            allow_close: false,
            confirm_close: false,
            review: None,
            find: Default::default(),
            hidden: Default::default(),
            page_sel: Default::default(),
            show_problems: false,
            center_selection: false,
        }
    }

    fn screenshot_state(&mut self) {
        // a typical moment: reviewing a freshly typed line, its eight character boxes proposed
        let Some(s) = &self.store else { return };
        self.page = 0;
        let line = s.pages[0].annotations.iter().find(|a| a.text() == "いらっしゃいませ").map(|a| a.id.clone());
        self.rebaseline();
        let Some(id) = line else { return };
        self.op(|p, cfg, user, now| crate::assist::regenerate_chars(p, &id, cfg, user, now));
        if let Some(s) = self.store.as_mut() {
            let _ = s.save_all("you");
        }
        let ctx = egui::Context::default();
        self.enter_review(&ctx);
        self.start_review(review_mode::Scope::ThisPage);
        let pos = self.review.as_ref().and_then(|r| r.session.items.iter().position(|i| i.pass == crate::assist::Pass::CharBoxes && i.annotation_id.as_deref() == Some(id.as_str())));
        if let Some(p) = pos {
            self.review_goto(p);
        }
        if let Some(r) = &mut self.review {
            r.overlay = true;
            r.opacity = 0.55;
        }
        self.message = None;
    }

    // ------------------------------------------------------------ messages

    pub fn flash(&mut self, msg: impl Into<String>, ctx: &egui::Context) {
        self.message = Some((msg.into(), ctx.input(|i| i.time), false));
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), f64::MAX, true));
    }

    pub fn user(&self) -> String {
        self.settings.user.clone()
    }

    // ------------------------------------------------------------ project

    pub fn set_store(&mut self, s: Store) {
        if let Some(old) = self.store.take() {
            old.unlock();
        }
        let recovery = s.pending_recovery();
        let root = s.root.clone();
        self.store = Some(s);
        self.page = 0;
        self.images.clear();
        self.textures.clear();
        self.thumbs.clear();
        self.undo.clear();
        self.redo.clear();
        self.selection.clear();
        self.draft.clear();
        self.preview = None;
        self.view.fitted = true;
        self.issues_dirty = true;
        self.dialogs.recovery = recovery;
        self.page_checks.clear();
        self.refresh_checks(true);
        self.dialogs.welcome = false;
        self.model = ModelStatus::Unknown;
        self.rebaseline();
        if self.screenshot.is_none() && !cfg!(test) {
            self.settings.recent.retain(|p| p != &root);
            self.settings.recent.insert(0, root);
            self.settings.recent.truncate(8);
            self.settings.save();
        }
    }

    pub fn open_project(&mut self, root: &Path) {
        match Store::open(root) {
            Ok(s) => self.set_store(s),
            Err(e) => self.error(format!("Could not open the project: {e}")),
        }
    }

    pub fn pages(&self) -> &[PageFile] {
        self.store.as_ref().map(|s| s.pages.as_slice()).unwrap_or(&[])
    }

    pub fn cur(&self) -> Option<&PageFile> {
        self.store.as_ref()?.pages.get(self.page)
    }

    pub fn cur_mut(&mut self) -> Option<&mut PageFile> {
        self.store.as_mut()?.pages.get_mut(self.page)
    }

    pub fn cur_id(&self) -> Option<String> {
        self.cur().map(|p| p.page_id.clone())
    }

    pub fn cfg(&self) -> ProjectConfig {
        self.store.as_ref().map(|s| s.manifest.config.clone()).unwrap_or_default()
    }

    pub fn ann(&self, id: &str) -> Option<&Annotation> {
        self.cur()?.get(id)
    }

    pub fn primary(&self) -> Option<&Annotation> {
        self.ann(self.selection.first()?)
    }

    pub fn go_to(&mut self, idx: usize) {
        if self.store.as_ref().is_none_or(|s| idx >= s.pages.len()) || idx == self.page {
            return;
        }
        self.commit();
        self.page = idx;
        self.selection.clear();
        self.draft.clear();
        self.drag = Drag::None;
        self.view.fitted = true;
        self.preview = None;
        self.order_parent = None;
        let keep: Vec<String> = self.pages().iter().enumerate().filter(|(i, _)| i.abs_diff(idx) <= 1).map(|(_, p)| p.page_id.clone()).collect();
        self.images.retain(|k, _| keep.contains(k));
        self.textures.retain(|k, _| keep.contains(k));
        self.text_masks.retain(|k, _| keep.contains(k));
        self.rebaseline();
    }

    pub fn page_image(&mut self, id: &str) -> Option<Arc<image::RgbaImage>> {
        if let Some(i) = self.images.get(id) {
            return Some(i.clone());
        }
        let s = self.store.as_ref()?;
        let path = s.image_path(s.entry(id)?);
        let img = Arc::new(crate::util::open_image(&path).ok()?.to_rgba8());
        self.images.insert(id.into(), img.clone());
        Some(img)
    }

    /// The latest text mask of a page (from memory, or the file its run recorded).
    pub fn text_mask(&mut self, id: &str) -> Option<Arc<image::GrayImage>> {
        if let Some(m) = self.text_masks.get(id) {
            return Some(m.clone());
        }
        let s = self.store.as_ref()?;
        let page = s.pages.iter().find(|p| p.page_id == id)?;
        let rel = page.inference_runs.iter().rev().find_map(|r| r.config.get("mask").and_then(|m| m.get("path")).and_then(|p| p.as_str()).map(|p| p.to_string()))?;
        let img = image::open(s.root.join(rel)).ok()?.to_luma8();
        let m = Arc::new(img);
        self.text_masks.insert(id.into(), m.clone());
        Some(m)
    }

    /// The text mask drawn as a see-through magenta layer.
    pub fn mask_texture(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        let key = format!("mask:{id}");
        if let Some(t) = self.textures.get(&key) {
            return Some(t.clone());
        }
        let m = self.text_mask(id)?;
        let (w, h) = (m.width() as usize, m.height() as usize);
        let px: Vec<Color32> = m.pixels().map(|p| if p[0] > 0 { Color32::from_rgba_unmultiplied(236, 64, 160, 120) } else { Color32::TRANSPARENT }).collect();
        let ci = egui::ColorImage { size: [w, h], source_size: vec2(w as f32, h as f32), pixels: px };
        let t = ctx.load_texture(key.clone(), ci, TextureOptions::NEAREST);
        self.textures.insert(key, t.clone());
        Some(t)
    }

    pub fn page_texture(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(id) {
            return Some(t.clone());
        }
        let img = self.page_image(id)?;
        let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
        let t = ctx.load_texture(format!("page-{id}"), ci, TextureOptions::LINEAR);
        self.textures.insert(id.into(), t.clone());
        Some(t)
    }

    // ------------------------------------------------------------ edits, undo, provenance

    pub fn rebaseline(&mut self) {
        self.baseline = self.cur().cloned();
    }

    /// Record what changed since the last commit: provenance, revision, approvals, undo, dirty.
    pub fn commit(&mut self) {
        let Some(base) = self.baseline.clone() else { return };
        let user = self.user();
        let now = util::now();
        let Some(store) = self.store.as_mut() else { return };
        let policy = store.manifest.config.review.clone();
        let Some(page) = store.pages.get_mut(self.page) else { return };
        if page.page_id != base.page_id || *page == base {
            return;
        }
        let refit = std::mem::take(&mut self.refit);
        let index: HashMap<&str, &Annotation> = base.annotations.iter().map(|a| (a.id.as_str(), a)).collect();
        for a in page.annotations.iter_mut() {
            if let Some(old) = index.get(a.id.as_str()) {
                if *old != a {
                    // a proposal refitted to its resized line is still an untouched proposal
                    if refit.contains(&a.id) && crate::assist::untouched_proposal(old) && a.changed_groups(old) == ["geometry"] {
                        continue;
                    }
                    review::on_edit(old, a, &policy, &user, &now);
                }
            }
        }
        let id = page.page_id.clone();
        let snapshot = page.clone();
        store.dirty.insert(id);
        self.undo.push(base);
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.baseline = Some(snapshot);
        self.issues_dirty = true;
    }

    /// Run an explicit operation (approve, AI, merge…) as one undo step without counting it as a content edit.
    pub fn op<R>(&mut self, f: impl FnOnce(&mut PageFile, &ProjectConfig, &str, &str) -> R) -> Option<R> {
        self.commit();
        let user = self.user();
        let now = util::now();
        let cfg = self.cfg();
        let before = self.cur()?.clone();
        let r = f(self.cur_mut()?, &cfg, &user, &now);
        let after = self.cur()?.clone();
        if after != before {
            self.undo.push(before);
            self.redo.clear();
            if let Some(s) = self.store.as_mut() {
                s.dirty.insert(after.page_id.clone());
            }
            self.issues_dirty = true;
        }
        self.baseline = Some(after);
        Some(r)
    }

    /// [`App::op`] on another page (one undo step for that page).
    pub fn op_on<R>(&mut self, idx: usize, f: impl FnOnce(&mut PageFile, &ProjectConfig, &str, &str) -> R) -> Option<R> {
        if idx == self.page {
            return self.op(f);
        }
        self.commit();
        let user = self.user();
        let now = util::now();
        let cfg = self.cfg();
        let s = self.store.as_mut()?;
        let page = s.pages.get_mut(idx)?;
        let before = page.clone();
        let r = f(page, &cfg, &user, &now);
        if *page != before {
            s.dirty.insert(before.page_id.clone());
            self.undo.push(before);
            self.redo.clear();
            self.issues_dirty = true;
        }
        Some(r)
    }

    pub fn undo(&mut self, redo: bool) {
        self.commit();
        let Some(snap) = (if redo { self.redo.pop() } else { self.undo.pop() }) else { return };
        let Some(store) = self.store.as_mut() else { return };
        let Some(i) = store.index_of(&snap.page_id) else { return };
        let current = std::mem::replace(&mut store.pages[i], snap);
        store.dirty.insert(current.page_id.clone());
        if redo {
            self.undo.push(current);
        } else {
            self.redo.push(current);
        }
        if self.page != i {
            self.page = i;
            self.view.fitted = true;
        }
        let ids: Vec<String> = self.cur().map(|p| p.annotations.iter().map(|a| a.id.clone()).collect()).unwrap_or_default();
        self.selection.retain(|s| ids.contains(s));
        self.issues_dirty = true;
        self.rebaseline();
    }

    pub fn is_dirty(&self) -> bool {
        self.store.as_ref().is_some_and(|s| s.is_dirty())
    }

    pub fn save(&mut self, ctx: &egui::Context, quiet: bool) {
        self.commit();
        let user = self.user();
        let Some(s) = self.store.as_mut() else { return };
        match s.save_all(&user) {
            Ok(conflicts) if conflicts.is_empty() => {
                if !quiet {
                    self.flash("Saved", ctx);
                }
            }
            Ok(conflicts) => {
                self.dialogs.conflicts = conflicts;
            }
            Err(e) => self.error(format!("Could not save: {e}")),
        }
        self.rebaseline();
        self.last_autosave = ctx.input(|i| i.time);
    }

    fn housekeeping(&mut self, ctx: &egui::Context) {
        let t = ctx.input(|i| i.time);
        let busy = !matches!(self.drag, Drag::None) || ctx.egui_wants_keyboard_input();
        // Comparing the page for edits costs time on big pages: only do it after input that can
        // change something (clicks, keys, typing, scrolling), not on plain mouse moves.
        let acted = ctx.input(|i| i.events.iter().any(|e| !matches!(e, egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_) | egui::Event::PointerGone | egui::Event::WindowFocused(_))));
        if !busy && (acted || self.commit_pending) {
            self.commit();
            self.commit_pending = false;
        } else if acted {
            self.commit_pending = true;
        }
        if self.issues_dirty {
            self.issues_dirty = false;
            self.refresh_checks(false);
        }
        if self.store.is_none() || self.screenshot.is_some() {
            return;
        }
        // recovery copy of unsaved pages every 2 s
        if t - self.last_recovery > 2.0 {
            self.last_recovery = t;
            if let Some(s) = &self.store {
                for id in s.dirty.iter() {
                    s.write_recovery(id);
                }
            }
        }
        if self.is_dirty() && !busy && self.dialogs.conflicts.is_empty() && t - self.last_autosave > self.settings.autosave_secs as f64 {
            self.save(ctx, true);
        }
        if self.is_dirty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    /// Re-check pages that may have changed (current page, unsaved pages, pages never checked)
    /// and keep the results per page, so big projects are not re-checked on every edit.
    pub fn refresh_checks(&mut self, all: bool) {
        let Some(s) = &self.store else { return };
        let cur = s.pages.get(self.page).map(|p| p.page_id.clone());
        for p in &s.pages {
            let stale = all || Some(&p.page_id) == cur.as_ref() || s.dirty.contains(&p.page_id) || !self.page_checks.contains_key(&p.page_id);
            if stale {
                let issues = validate::validate_page(&s.manifest.config, p, None);
                let m = crate::assist::metrics(&[p], &s.manifest.config);
                self.page_checks.insert(p.page_id.clone(), (issues, m));
            }
        }
        let ids: Vec<&str> = s.pages.iter().map(|p| p.page_id.as_str()).collect();
        self.page_checks.retain(|k, _| ids.contains(&k.as_str()));
        self.issues = s.pages.iter().flat_map(|p| self.page_checks[&p.page_id].0.clone()).collect();
        let mut total = crate::assist::Metrics::default();
        for p in &s.pages {
            total.add(&self.page_checks[&p.page_id].1);
        }
        self.metrics = total;
    }

    // ------------------------------------------------------------ AI

    pub fn poll_ai(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.worker.rx.try_recv() {
            match ev {
                ai::Event::ModelReady(m) => {
                    if let Some(s) = self.store.as_mut() {
                        if s.manifest.config.model.pinned.is_none() {
                            s.manifest.config.model.pinned = Some(m.clone());
                            s.manifest_dirty = true;
                        }
                    }
                    self.model = ModelStatus::Ready(m);
                }
                ai::Event::ZooReady(id, m) => {
                    self.models_ui.loaded.insert(id, m);
                }
                ai::Event::Failed { page_id, error } => {
                    if matches!(self.model, ModelStatus::Unknown) && !error.contains("convert") && !error.contains("charspot") && !error.contains("AnimeText") && !error.contains("Koharu") {
                        self.model = ModelStatus::Missing(error.clone());
                    }
                    self.error(format!("AI on {page_id}: {error}"));
                }
                ai::Event::Done { page_id, run, mask } => {
                    let cur = self.cur_id();
                    if cur.as_deref() == Some(page_id.as_str()) {
                        self.commit();
                    }
                    let cfg = self.cfg();
                    let Some(s) = self.store.as_mut() else { continue };
                    let Some(i) = s.index_of(&page_id) else { continue };
                    let before = s.pages[i].clone();
                    let single = matches!(run.scope, RunScope::Annotation { .. }) && run.predictions.iter().all(|p| p.level.is_none());
                    let what = run.model.name.clone();
                    let mut run = run;
                    let mut ink = 0usize;
                    if let Some(mk) = &mask {
                        // the page's text mask, then each text label's part of it
                        let rel = format!("masks/{}-text-{}.png", page_id, run.id);
                        let mut png = Vec::new();
                        if image::DynamicImage::ImageLuma8((**mk).clone()).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_ok() {
                            let _ = util::atomic_write(&s.root.join(&rel), &png);
                            use sha2::Digest;
                            run.config.insert("mask".into(), serde_json::json!({"path": rel, "sha256": format!("{:x}", sha2::Sha256::digest(&png)), "size": [mk.width(), mk.height()]}));
                        }
                        for (path, bytes) in ai::attach_ink(&mut s.pages[i], mk, &run.id) {
                            let _ = util::atomic_write(&s.root.join(&path), &bytes);
                            ink += 1;
                        }
                        self.text_masks.insert(page_id.clone(), mk.clone());
                        self.textures.remove(&format!("mask:{page_id}"));
                        self.show_text_mask = true;
                    }
                    let res = ai::apply_run(&mut s.pages[i], run, &cfg);
                    let kinds = s.pages[i].inference_runs.last().map(run_summary).unwrap_or_default();
                    if cfg.reading.auto_suggest && !single {
                        let dir = crate::assist::page_direction(s.entry(&page_id).map(|e| &e.source), &cfg);
                        crate::assist::apply_order(&mut s.pages[i], &cfg, dir);
                    }
                    s.dirty.insert(page_id.clone());
                    self.undo.push(before);
                    self.issues_dirty = true;
                    if cur.as_deref() == Some(page_id.as_str()) {
                        self.rebaseline();
                    }
                    let msg = if single {
                        format!("{what} read the selection again: see Transcription (filled if empty, otherwise added as an alternative)")
                    } else if mask.is_some() {
                        format!("{what} marked the text pixels of {page_id}; {ink} label(s) got an ink mask (View → Text mask shows it)")
                    } else {
                        format!("{what} proposed {} on {page_id}; {} overlapped existing work and were left out{}", if kinds.is_empty() { format!("{} label(s)", res.added) } else { kinds }, res.skipped, if res.alternatives > 0 { format!("; {} reading(s) added as alternatives", res.alternatives) } else { String::new() })
                    };
                    self.flash(msg, ctx);
                }
            }
        }
    }

    pub fn ai_job(&mut self, page_idx: usize, kind: ai::JobKind) {
        if !self.cfg().model.enabled {
            self.error("AI is switched off for this project (Project settings → Model).");
            return;
        }
        let user = self.user();
        let reader = self.cfg().model.reader;
        let zoo = self.settings.models.clone();
        let Some(s) = &self.store else { return };
        let Some(p) = s.pages.get(page_idx) else { return };
        let Some(e) = s.entry(&p.page_id) else { return };
        self.worker.submit(ai::Job {
            page_id: p.page_id.clone(),
            image_path: s.image_path(e),
            image_sha256: e.sha256.clone(),
            kind,
            pinned: s.manifest.config.model.pinned.clone(),
            user,
            zoo,
            reader,
        });
    }

    /// Run a page-wide zoo model (text blocks, layout, text masks) on pages.
    pub fn ai_zoo_pages(&mut self, pages: &[usize], kind: ai::JobKind, ctx: &egui::Context) {
        let id = kind.zoo_model(Reader::PpOcr);
        for &p in pages {
            self.ai_job(p, kind.clone());
        }
        if let Some(id) = id {
            self.flash(format!("{} is working on {} page(s) in the background; keep working meanwhile", id.name(), pages.len()), ctx);
        }
    }

    /// Read each text region of a page that has no lines yet with charspot: lines and characters.
    pub fn ai_read_blocks(&mut self, page_idx: usize, only: Option<&[String]>, ctx: &egui::Context) {
        let Some(page) = self.pages().get(page_idx) else { return };
        let blocks: Vec<(String, [P; 4])> = page
            .annotations
            .iter()
            .filter(|a| a.level == Level::Region && only.is_none_or(|o| o.contains(&a.id)))
            .filter(|a| !page.annotations.iter().any(|l| matches!(l.level, Level::Line | Level::Word) && l.parent.as_deref() == Some(a.id.as_str())))
            .map(|a| {
                let (lo, hi) = a.geometry.bounds();
                (a.id.clone(), [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]])
            })
            .collect();
        if blocks.is_empty() {
            self.error("No text region without lines to read here. Find text blocks first (AI → Find text blocks), or select a region.");
            return;
        }
        let n = blocks.len();
        self.ai_job(page_idx, ai::JobKind::ReadBlocks { blocks });
        self.flash(format!("Reading {n} text region(s) into lines and characters"), ctx);
    }

    /// Find the characters of the selected lines with charspot.
    pub fn ai_find_chars(&mut self, ctx: &egui::Context) {
        let ids = self.selection.clone();
        let lines: Vec<Annotation> = self.cur().map(|p| p.annotations.iter().filter(|a| ids.contains(&a.id) && matches!(a.level, Level::Line | Level::Word)).cloned().collect()).unwrap_or_default();
        if lines.is_empty() {
            self.error("Select one or more lines first.");
            return;
        }
        let page = self.page;
        for a in &lines {
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
            let axis = matches!(a.geometry, Geometry::Box { rotation_deg, .. } if rotation_deg.abs() < 0.01) && a.rectification.is_none();
            self.ai_job(page, ai::JobKind::Characters { annotation_id: a.id.clone(), quad, target_size: size, rectified: !axis });
        }
        self.flash(format!("Finding the characters of {} line(s) with charspot", lines.len()), ctx);
    }

    pub fn ai_all_pages(&mut self, ctx: &egui::Context) {
        let n = self.pages().len();
        let todo: Vec<usize> = (0..n).filter(|&i| self.pages()[i].inference_runs.iter().all(|r| !matches!(r.scope, RunScope::Page))).collect();
        for i in &todo {
            self.ai_job(*i, ai::JobKind::Page);
        }
        self.flash(format!("Pre-annotating {} page(s) in the background; keep working meanwhile", todo.len()), ctx);
    }

    pub fn reread_selected(&mut self) {
        let Some(a) = self.primary().cloned() else { return };
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
        let page = self.page;
        self.ai_job(page, ai::JobKind::Recognize { annotation_id: a.id, quad, target_size: size, rectified: true });
    }

    // ------------------------------------------------------------ editing actions

    pub fn new_annotation(&mut self, geometry: Geometry) -> Option<String> {
        // rectangles drawn as quadrilaterals are kept as (rotated) boxes
        let geometry = crate::assist::normalize_shape(geometry);
        let level = self.draw_level;
        let class = self.draw_class.clone();
        let id = util::new_id("a");
        let r = self.op(|page, cfg, user, now| {
            let mut a = Annotation::new(id.clone(), level, &page.page_id, geometry, user, now);
            if let Some(c) = class {
                a.classes.insert(c);
            }
            if level.is_text() {
                let (lo, hi) = a.geometry.bounds();
                let vertical = (hi[1] - lo[1]) > (hi[0] - lo[0]) * 1.2;
                let vmode = if cfg.default_writing_mode == WritingMode::VerticalLr { WritingMode::VerticalLr } else { WritingMode::VerticalRl };
                let hdir = if cfg.default_direction == Direction::Rtl { Direction::Rtl } else { Direction::Ltr };
                a.transcription = Some(Transcription {
                    language: (!cfg.default_language.is_empty()).then(|| cfg.default_language.clone()),
                    script: (!cfg.default_script.is_empty()).then(|| cfg.default_script.clone()),
                    writing_mode: if vertical { vmode } else { WritingMode::HorizontalTb },
                    direction: if vertical { Direction::Ttb } else { hdir },
                    ..Default::default()
                });
                // smart defaults carry their provenance and still need a person's confirmation
                if !cfg.default_language.is_empty() {
                    crate::assist::set_check(&mut a, "language", CheckState::Inferred, user, now, "project default", false);
                }
                crate::assist::set_check(&mut a, "direction", CheckState::Inferred, user, now, "box shape", false);
            }
            for g in ["geometry", "metadata"] {
                a.provenance.fields.insert(g.into(), FieldState::Manual);
            }
            for f in &cfg.custom_fields {
                if let Some(d) = &f.default {
                    if f.levels.is_empty() || f.levels.contains(&level) {
                        a.fields.insert(f.key.clone(), d.clone());
                    }
                }
            }
            // parent: smallest coarser annotation that contains the centre
            let (lo, hi) = a.geometry.bounds();
            let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
            a.parent = page
                .annotations
                .iter()
                .filter(|x| x.level.rank() < level.rank() && x.geometry.contains(c, 0.0))
                .min_by(|x, y| x.geometry.area().total_cmp(&y.geometry.area()))
                .map(|x| x.id.clone());
            let (w, h) = (page.image.width as f64, page.image.height as f64);
            clamp_geometry(&mut a.geometry, w, h);
            a.round();
            page.annotations.push(a);
            // a new region/container/panel takes in the finer shapes it now encloses
            crate::assist::adopt_children(page, &id);
            if cfg.reading.auto_suggest {
                crate::assist::apply_order(page, cfg, cfg.reading.page_direction);
            }
        });
        r.map(|_| {
            self.selection = vec![id.clone()];
            id
        })
    }

    pub fn delete_selection(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        self.op(|page, _, _, _| {
            page.annotations.retain(|a| !ids.contains(&a.id));
            for a in page.annotations.iter_mut() {
                if a.parent.as_ref().is_some_and(|p| ids.contains(p)) {
                    a.parent = None;
                }
                a.relations.retain(|r| !ids.contains(&r.target));
            }
        });
        self.selection.clear();
    }

    pub fn accept_ai(&mut self) {
        let ids = self.selection.clone();
        self.op(|page, _, user, now| {
            for id in &ids {
                if let Some(a) = page.get_mut(id) {
                    review::accept_ai(a, user, now);
                }
            }
        });
    }

    pub fn reject_ai(&mut self, reason: &str) {
        let ids: Vec<String> = self.selection.iter().filter(|id| self.ann(id).is_some_and(|a| a.provenance.origin == Origin::Ai)).cloned().collect();
        let reason = reason.to_string();
        self.op(|page, _, user, now| {
            for id in &ids {
                review::reject_ai(page, id, &reason, user, now);
            }
        });
        self.selection.retain(|s| !ids.contains(s));
    }

    pub fn submit(&mut self) {
        let ids = self.selection.clone();
        self.op(|page, cfg, _, _| {
            for id in &ids {
                if let Some(a) = page.get_mut(id) {
                    review::submit(a, &cfg.review);
                }
            }
        });
    }

    pub fn approve(&mut self, dims: &[Dimension], ctx: &egui::Context) {
        let ids = self.selection.clone();
        let dims = dims.to_vec();
        let errs = self.op(|page, cfg, user, now| {
            let mut errs = Vec::new();
            for id in &ids {
                if let Some(a) = page.get_mut(id) {
                    if let Err(e) = review::approve(a, &dims, &cfg.review, user, now) {
                        errs.push(e);
                    }
                }
            }
            errs
        });
        match errs {
            Some(e) if !e.is_empty() => self.error(e[0].clone()),
            _ => self.flash(format!("Approved {} for {} annotation(s)", review::dims_text(&dims), ids.len()), ctx),
        }
    }

    pub fn approve_gold(&mut self, ctx: &egui::Context) {
        let ids = self.selection.clone();
        let errs = self.op(|page, cfg, user, now| {
            let snapshot = page.clone();
            let mut errs = Vec::new();
            for id in &ids {
                if let Some(a) = page.get_mut(id) {
                    if let Err(e) = review::approve_gold(a, &snapshot, &cfg.review, user, now) {
                        errs.push(e);
                    }
                }
            }
            errs
        });
        match errs {
            Some(e) if !e.is_empty() => self.error(format!("Not gold yet: {}", e[0])),
            _ => self.flash("Approved as gold", ctx),
        }
    }

    pub fn mark_inspected(&mut self, complete: bool, no_text: bool) {
        self.op(|page, _, user, now| {
            page.completeness.inspected = true;
            page.completeness.complete = complete;
            page.completeness.no_text = no_text;
            page.completeness.checked = Some(Stamp { by: user.into(), at: now.into() });
        });
    }

    pub fn split_selection(&mut self, ctx: &egui::Context) {
        let Some(a) = self.primary().cloned() else { return };
        let Some(q) = a.geometry.corners() else {
            self.error("Split works on boxes and quadrilaterals.");
            return;
        };
        let wlen = (geom::dist(q[0], q[1]) + geom::dist(q[3], q[2])) / 2.0;
        let hlen = (geom::dist(q[0], q[3]) + geom::dist(q[1], q[2])) / 2.0;
        let mid = |a: P, b: P| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let (qa, qb) = if wlen >= hlen {
            let (m0, m1) = (mid(q[0], q[1]), mid(q[3], q[2]));
            ([q[0], m0, m1, q[3]], [m0, q[1], q[2], m1])
        } else {
            let (m0, m1) = (mid(q[0], q[3]), mid(q[1], q[2]));
            ([q[0], q[1], m1, m0], [m0, m1, q[2], q[3]])
        };
        let text = a.text().to_string();
        let (ta, tb) = match text.split_once('\n') {
            Some((x, y)) => (x.to_string(), y.to_string()),
            None => {
                use unicode_segmentation::UnicodeSegmentation;
                let g: Vec<&str> = text.graphemes(true).collect();
                let h = g.len() / 2;
                (g[..h].concat(), g[h..].concat())
            }
        };
        let new_id = util::new_id("a");
        let id = a.id.clone();
        // one undo step: the first half keeps the identity (an edit), the second half is new
        self.op(|page, cfg, user, now| {
            if let Some(x) = page.get_mut(&id) {
                let old = x.clone();
                x.geometry = crate::assist::quad_to_box(&Geometry::Quad { points: qa }).unwrap_or(Geometry::Quad { points: qa });
                if let Some(t) = &mut x.transcription {
                    t.text = ta;
                    t.spans.clear();
                }
                review::on_edit(&old, x, &cfg.review, user, now);
            }
            let mut b = a.clone();
            b.id = new_id.clone();
            b.geometry = crate::assist::quad_to_box(&Geometry::Quad { points: qb }).unwrap_or(Geometry::Quad { points: qb });
            if let Some(t) = &mut b.transcription {
                t.text = tb;
                t.spans.clear();
            }
            b.created = Stamp { by: user.into(), at: now.into() };
            b.modified = b.created.clone();
            b.revision = 1;
            b.review = Review::default();
            b.reading_order = a.reading_order.map(|r| r + 1);
            b.provenance.fields.insert("geometry".into(), FieldState::Manual);
            b.relations.push(Relation { kind: "split_from".into(), target: a.id.clone() });
            page.annotations.push(b);
        });
        self.selection = vec![id, new_id];
        self.flash("Split in two; check both halves", ctx);
    }

    pub fn merge_selection(&mut self, ctx: &egui::Context) {
        if self.selection.len() < 2 {
            self.error("Select two or more annotations to merge (Shift+click).");
            return;
        }
        let mut items: Vec<Annotation> = self.selection.iter().filter_map(|id| self.ann(id).cloned()).collect();
        if let Some(p) = self.cur() {
            items.sort_by_cached_key(|a| crate::assist::reading_key(p, a));
        }
        let keep = items[0].id.clone();
        let removed: Vec<String> = items[1..].iter().map(|a| a.id.clone()).collect();
        let pts: Vec<P> = items.iter().flat_map(|a| a.geometry.outline()).collect();
        let all_boxes = items.iter().all(|a| matches!(a.geometry, Geometry::Box { rotation_deg, .. } if rotation_deg == 0.0));
        let geometry = if all_boxes {
            let (lo, hi) = geom::bounds(&pts);
            Geometry::Box { x: lo[0], y: lo[1], w: hi[0] - lo[0], h: hi[1] - lo[1], rotation_deg: 0.0 }
        } else {
            let (c, s, ang) = geom::min_area_rect(&pts);
            let q = Geometry::Quad { points: geom::order_quad(geom::rect_corners(c, s, ang)) };
            crate::assist::quad_to_box(&q).unwrap_or(q)
        };
        // characters and words join without a separator, lines and regions with line breaks
        let sep = if matches!(items[0].level, Level::Char | Level::Word) { "" } else { "\n" };
        let joined: Vec<String> = items.iter().map(|a| a.text().to_string()).filter(|t| !t.is_empty()).collect();
        let keep2 = keep.clone();
        // one undo step for the whole merge
        self.op(|page, cfg, user, now| {
            if let Some(x) = page.get_mut(&keep2) {
                let old = x.clone();
                x.geometry = geometry;
                if !joined.is_empty() {
                    x.transcription.get_or_insert_with(Default::default).text = joined.join(sep);
                }
                review::on_edit(&old, x, &cfg.review, user, now);
            }
            for a in page.annotations.iter_mut() {
                if a.parent.as_ref().is_some_and(|p| removed.contains(p)) {
                    a.parent = Some(keep2.clone());
                }
                for r in a.relations.iter_mut() {
                    if removed.contains(&r.target) {
                        r.target = keep2.clone();
                    }
                }
            }
            if let Some(k) = page.get_mut(&keep2) {
                review::add_note(k, NoteKind::Comment, None, &format!("Merged with {}", removed.join(", ")), user, now);
            }
            page.annotations.retain(|a| !removed.contains(&a.id));
        });
        self.selection = vec![keep];
        self.flash("Merged; the text was joined in reading order", ctx);
    }

    pub fn next_needing_work(&mut self) {
        let start = self.page;
        let n = self.pages().len();
        let cur_sel = self.selection.first().cloned();
        for step in 0..=n {
            let i = (start + step) % n.max(1);
            let Some(p) = self.pages().get(i) else { return };
            let mut cands: Vec<&Annotation> = p.annotations.iter().filter(|a| a.review.state.needs_work() || a.review.needs_rereview).collect();
            // same level first, then the reading order through the hierarchy (panel › balloon › line › character)
            cands.sort_by_cached_key(|a| (a.level, crate::assist::reading_key(p, a)));
            let pick = if step == 0 {
                let pos = cur_sel.as_ref().and_then(|c| cands.iter().position(|a| &a.id == c));
                match pos {
                    Some(k) => cands.get(k + 1).map(|a| a.id.clone()),
                    None => cands.first().map(|a| a.id.clone()),
                }
            } else {
                cands.first().map(|a| a.id.clone())
            };
            let not_inspected = !p.completeness.inspected;
            if let Some(id) = pick {
                self.go_to(i);
                self.selection = vec![id];
                return;
            }
            if step > 0 && not_inspected {
                self.go_to(i);
                return;
            }
        }
    }

    // ------------------------------------------------------------ keyboard

    fn shortcut(&self, ctx: &egui::Context, action: &str) -> bool {
        let Some((m, k)) = parse_shortcut(&self.settings.shortcut(action)) else { return false };
        ctx.input_mut(|i| i.consume_key(m, k))
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if self.dialogs.shortcuts.as_ref().is_some_and(|s| s.is_some()) {
            return; // capturing a new shortcut
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)) {
            self.undo(true);
        }
        for a in ["save", "undo", "redo"] {
            if self.shortcut(ctx, a) {
                match a {
                    "save" => self.save(ctx, false),
                    "undo" => self.undo(false),
                    _ => self.undo(true),
                }
            }
        }
        if ctx.egui_wants_keyboard_input() || self.store.is_none() {
            return;
        }
        if self.shortcut(ctx, "tighten") {
            self.tighten_selection(ctx);
        }
        if self.shortcut(ctx, "find") {
            self.find.open = !self.find.open;
            if self.find.open && self.find.conds.is_empty() {
                self.find.conds.push(find::Cond { crit: find::Crit::Text(String::new()), not: false });
            }
        }
        if self.shortcut(ctx, "show_hidden") {
            let n = self.hidden.len();
            self.hidden.clear();
            self.flash(format!("{n} hidden label(s) shown again"), ctx);
        }
        if self.review.is_some() {
            self.review_keys(ctx);
            return;
        }
        if self.shortcut(ctx, "make_regions") {
            self.make_regions(ctx);
        }
        if self.shortcut(ctx, "propose_chars") {
            self.propose_or_accept_chars(ctx);
        }
        if self.shortcut(ctx, "review_mode") {
            self.enter_review(ctx);
            return;
        }
        // longer shortcuts first so Ctrl+Shift+Enter is not taken by Ctrl+Enter
        if self.shortcut(ctx, "approve") {
            self.approve(&Dimension::ALL, ctx);
        }
        if self.shortcut(ctx, "submit") {
            self.submit();
        }
        if self.shortcut(ctx, "reread") {
            let ids = self.selection.clone();
            self.reread(&ids);
            self.flash(format!("Reading {} label(s) again in the background", ids.len()), ctx);
        }
        if self.shortcut(ctx, "ai_chars") {
            self.ai_find_chars(ctx);
        }
        if self.shortcut(ctx, "hide") {
            self.hide_selection(ctx);
        }
        if self.shortcut(ctx, "next_problem") {
            self.next_problem(ctx);
        }
        if self.shortcut(ctx, "ai_page") {
            let p = self.page;
            self.ai_job(p, ai::JobKind::Page);
        }
        if self.shortcut(ctx, "inspected") {
            self.mark_inspected(true, false);
        }
        let tools = [
            ("tool_select", Tool::Select),
            ("tool_pan", Tool::Pan),
            ("tool_box", Tool::Box),
            ("tool_quad", Tool::Quad),
            ("tool_polygon", Tool::Polygon),
            ("tool_order", Tool::Order),
            ("tool_link", Tool::Link),
            ("tool_ai_region", Tool::AiRegion),
        ];
        for (name, t) in tools {
            if self.shortcut(ctx, name) {
                self.set_tool(t);
            }
        }
        let levels = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6];
        for (k, l) in levels.iter().zip(Level::DRAWABLE) {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, *k)) {
                self.draw_level = l;
            }
        }
        if self.shortcut(ctx, "next_page") || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::PageDown)) {
            let p = self.page + 1;
            self.go_to(p);
        }
        if self.shortcut(ctx, "prev_page") || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::PageUp)) {
            let p = self.page.saturating_sub(1);
            self.go_to(p);
        }
        if self.shortcut(ctx, "next_work") {
            self.next_needing_work();
        }
        if self.shortcut(ctx, "fit") {
            self.view.fitted = true;
        }
        if self.shortcut(ctx, "split") {
            self.split_selection(ctx);
        }
        if self.shortcut(ctx, "merge") {
            self.merge_selection(ctx);
        }
        if self.shortcut(ctx, "accept_ai") {
            self.accept_ai();
        }
        if self.shortcut(ctx, "reject_ai") {
            self.reject_ai("rejected with keyboard shortcut");
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            self.finish_draft();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if self.draft.is_empty() {
                self.selection.clear();
            }
            self.draft.clear();
            self.drag = Drag::None;
            self.order_parent = None;
            self.order_next = 1;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace)) {
            if self.draft.is_empty() {
                self.delete_selection();
            } else {
                self.draft.pop();
            }
        }
        if self.shortcut(ctx, "delete") {
            self.delete_selection();
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            let layers = self.layers.clone();
            self.selection = self.cur().map(|p| p.annotations.iter().filter(|a| layers.get(&a.level).copied().unwrap_or(true)).map(|a| a.id.clone()).collect()).unwrap_or_default();
        }
    }

    pub fn set_tool(&mut self, t: Tool) {
        if self.tool != t {
            self.finish_draft();
            self.tool = t;
            self.order_parent = None;
            self.order_next = 1;
        }
    }

    pub fn finish_draft(&mut self) {
        let mut pts = std::mem::take(&mut self.draft);
        pts.dedup_by(|a, b| geom::dist(*a, *b) < 1.0);
        match self.tool {
            Tool::Polygon if pts.len() >= 3 => {
                if geom::signed_area(&pts) < 0.0 {
                    pts.reverse();
                }
                self.new_annotation(Geometry::Polygon { points: pts });
            }
            Tool::Quad if pts.len() == 4 => {
                let q = [pts[0], pts[1], pts[2], pts[3]];
                let q = if geom::quad_order_ok(&q) { q } else { geom::order_quad(q) };
                self.new_annotation(Geometry::Quad { points: q });
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ frame

    pub fn frame_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;
        self.poll_ai(&ctx);
        self.keys(&ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close && self.screenshot.is_none() {
            self.commit();
            if self.is_dirty() {
                self.save(&ctx, true);
            }
            if self.is_dirty() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_close = true;
            } else if let Some(s) = &self.store {
                s.unlock();
            }
        }

        let bar = |fill: Color32, m: Margin| egui::Frame::new().fill(fill).inner_margin(m);
        Panel::top("menu").frame(bar(PANEL_BG, Margin::symmetric(10, 5))).show(ui, |ui| self.menu_bar(ui));
        Panel::bottom("status").frame(bar(PANEL_BG, Margin::symmetric(10, 4))).show(ui, |ui| self.status_bar(ui));
        // review mode gives the page and the review card the whole window
        if self.store.is_some() && self.review.is_none() {
            Panel::left("pages")
                .resizable(true)
                .default_size(240.0)
                .size_range(190.0..=420.0)
                .frame(bar(PANEL_BG, Margin::symmetric(10, 4)))
                .show(ui, |ui| self.page_list(ui));
            Panel::left("tools").resizable(false).exact_size(50.0).frame(bar(PANEL_BG2, Margin::same(4))).show(ui, |ui| self.tool_strip(ui));
        }
        if self.store.is_some() {
            Panel::right("inspector")
                .resizable(true)
                .default_size(370.0)
                .size_range(300.0..=620.0)
                .frame(bar(PANEL_BG, Margin::symmetric(12, 4)))
                .show(ui, |ui| {
                    if self.review.is_some() {
                        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.review_card(ui));
                    } else {
                        self.inspector(ui);
                    }
                });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if self.store.is_some() {
                let fill = if self.review.is_some() { Color32::from_rgb(58, 50, 30) } else { PANEL_BG2 };
                bar(fill, Margin::symmetric(10, 5)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if self.review.is_some() {
                        self.review_bar(ui);
                    } else {
                        self.top_strip(ui);
                    }
                });
                self.canvas(ui);
            } else {
                self.welcome(ui);
            }
        });
        if self.leave_fullscreen {
            self.leave_fullscreen = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        self.dialogs(&ctx);
        self.models_window(&ctx);
        self.housekeeping(&ctx);
        self.screenshot_tick(&ctx);
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        ctx.request_repaint();
        if self.shot_done.load(Ordering::SeqCst) {
            if let Some(s) = &self.store {
                s.unlock();
                let _ = std::fs::remove_dir_all(&s.root);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let models_pending = self.models_ui.open && (self.models_ui.snap().is_none() || self.models_ui.refreshing.load(Ordering::SeqCst));
        if self.frame_no >= 120 && !self.shot_requested && !models_pending {
            self.shot_requested = true;
            let done = self.shot_done.clone();
            let ctx2 = ctx.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let mut bytes = Vec::with_capacity(w * h * 4);
                for p in &img.pixels {
                    bytes.extend_from_slice(&[p.r(), p.g(), p.b(), 255]);
                }
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                match image::RgbaImage::from_raw(w as u32, h as u32, bytes).map(|i| i.save(&path)) {
                    Some(Ok(())) => eprintln!("saved screenshot to {}", path.display()),
                    other => eprintln!("could not save screenshot: {other:?}"),
                }
                done.store(true, Ordering::SeqCst);
                ctx2.request_repaint();
            });
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.screenshot.is_some() {
            // keep drawing even when the window is hidden or the screen is off
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.frame_ui(ui);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(s) = &self.store {
            if !s.is_dirty() {
                s.unlock();
            }
        }
    }
}

pub fn clamp_geometry(g: &mut Geometry, w: f64, h: f64) {
    let cl = |p: &mut P| {
        p[0] = p[0].clamp(0.0, w);
        p[1] = p[1].clamp(0.0, h);
    };
    match g {
        Geometry::Box { x, y, w: bw, h: bh, rotation_deg } if *rotation_deg == 0.0 => {
            let (x1, y1) = ((*x + *bw).min(w), (*y + *bh).min(h));
            *x = x.clamp(0.0, w);
            *y = y.clamp(0.0, h);
            *bw = (x1 - *x).max(0.0);
            *bh = (y1 - *y).max(0.0);
        }
        Geometry::Box { .. } => {}
        Geometry::Polygon { points } => points.iter_mut().for_each(cl),
        Geometry::Quad { points } => points.iter_mut().for_each(cl),
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL_BG;
    v.window_fill = PANEL_BG2;
    v.extreme_bg_color = Color32::from_rgb(24, 25, 31);
    v.faint_bg_color = PANEL_BG2;
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.hyperlink_color = ACCENT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(50, 53, 62));
    v.widgets.inactive.bg_fill = Color32::from_rgb(50, 53, 63);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(50, 53, 63);
    v.widgets.hovered.bg_fill = Color32::from_rgb(62, 66, 80);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(62, 66, 80);
    v.widgets.active.bg_fill = Color32::from_rgb(72, 78, 100);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(5);
    }
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 5.0);
        s.spacing.button_padding = vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
    });
}
