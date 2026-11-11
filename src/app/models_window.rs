//! The Models window: which programs run the models on this computer, which models are here,
//! where each one runs (processor or graphics card), and converting them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText};

use super::*;
use crate::convert::{self, Target};
use crate::engine::Backend;
use crate::runtime::{self, Component, Probe, Progress};
use crate::zoo::{self, Located, ZooId};

/// What the window found the last time it looked (filled in the background).
#[derive(Clone, Default)]
pub struct Snapshot {
    pub runtimes: BTreeMap<u8, (Component, Option<PathBuf>, Option<Result<Probe, String>>)>,
    pub models: BTreeMap<ZooId, Result<Located, String>>,
    pub ppocr: Option<String>,
    pub python: Option<String>,
    /// Python version of the chosen virtual environment, when it exists.
    pub venv: Option<String>,
    pub memory: Option<(u64, u64)>,
}

#[derive(Default)]
pub struct ModelsUi {
    pub open: bool,
    pub snapshot: Arc<Mutex<Option<Snapshot>>>,
    pub refreshing: Arc<std::sync::atomic::AtomicBool>,
    looked_for: Option<zoo::Options>,
    pub install: Option<(Component, Arc<Mutex<Progress>>, Arc<Mutex<Option<Result<PathBuf, String>>>>)>,
    pub task: Option<convert::Task>,
    /// What each zoo model loaded as (runtime, files), from the AI worker.
    pub loaded: BTreeMap<ZooId, ModelRef>,
    pub show_log: bool,
    /// The finished conversion was noticed (and the window looked again).
    task_seen: bool,
}

impl ModelsUi {
    pub fn start_task(&mut self, id: ZooId, target: Target, o: zoo::Options) {
        self.task = Some(convert::Task::start(id, target, o));
        self.task_seen = false;
        self.show_log = true;
    }
}

fn comp_key(c: Component) -> u8 {
    match c {
        Component::OnnxRuntime => 0,
        Component::Ncnn => 1,
        Component::Pnnx => 2,
    }
}

impl ModelsUi {
    /// Look again in the background (finding files, testing the runtimes in a separate process).
    pub fn refresh(&mut self, o: &zoo::Options) {
        if self.refreshing.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        self.looked_for = Some(o.clone());
        let (snap, flag, o) = (self.snapshot.clone(), self.refreshing.clone(), o.clone());
        std::thread::spawn(move || {
            runtime::forget_probes();
            let rt = o.runtime();
            let mut s = Snapshot::default();
            for c in Component::ALL {
                let explicit = match c {
                    Component::OnnxRuntime => rt.ort.clone(),
                    Component::Ncnn => rt.ncnn.clone(),
                    Component::Pnnx => None,
                };
                let path = runtime::locate(c, explicit.as_deref());
                let probe = path.as_ref().map(|p| runtime::probe(c, p));
                s.runtimes.insert(comp_key(c), (c, path, probe));
            }
            for id in ZooId::ALL {
                s.models.insert(id, zoo::locate(id, &o));
            }
            s.ppocr = crate::ocr::find_candidates().first().map(|c| format!("{} · rev {}", c.version_str(), &c.revision[..c.revision.len().min(8)]));
            s.venv = convert::venv_version(&o);
            s.python = s.venv.clone().or_else(|| convert::find_python().map(|(_, v)| v));
            s.memory = runtime::memory_mb();
            *snap.lock().unwrap() = Some(s);
            flag.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    pub fn snap(&self) -> Option<Snapshot> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Whether a model can run where it is set to, from the last look.
    pub fn ready(&self, id: ZooId, o: &zoo::Options) -> Result<(), String> {
        let Some(s) = self.snap() else { return Ok(()) };
        let l = s.models.get(&id).cloned().unwrap_or_else(|| Err("not looked for yet".into()))?;
        match o.backend(id) {
            Backend::Onnx if l.files.onnx.is_none() => Err("needs converting first (AI → Models…)".into()),
            Backend::NcnnGpu | Backend::NcnnCpu if l.files.ncnn.is_none() => Err("needs converting to ncnn first (AI → Models…)".into()),
            _ => Ok(()),
        }
    }
}

fn good(t: impl Into<String>) -> RichText {
    RichText::new(t.into()).color(GOOD)
}
fn bad(t: impl Into<String>) -> RichText {
    RichText::new(t.into()).color(BAD)
}
fn muted(t: impl Into<String>) -> RichText {
    RichText::new(t.into()).color(MUTED).size(11.5)
}

impl App {
    pub fn models_window(&mut self, ctx: &egui::Context) {
        if !self.models_ui.open {
            return;
        }
        if self.models_ui.looked_for.as_ref() != Some(&self.settings.models) && !self.models_ui.refreshing.load(std::sync::atomic::Ordering::SeqCst) {
            let o = self.settings.models.clone();
            self.models_ui.refresh(&o);
        }
        // a finished install or conversion changes what is there
        let mut look_again = false;
        let finished = self.models_ui.install.as_ref().and_then(|(c, _, result)| result.lock().unwrap().take().map(|r| (*c, r)));
        if let Some((c, r)) = finished {
            match r {
                Ok(p) => self.flash(format!("{} installed: {}", c.name(), p.display()), ctx),
                Err(e) => self.error(format!("{}: {e}", c.name())),
            }
            self.models_ui.install = None;
            look_again = true;
        }
        if let Some(t) = &self.models_ui.task {
            let finished = !t.state.lock().unwrap().running;
            if finished && !self.models_ui.task_seen {
                // keep the finished log on screen; look again once
                self.models_ui.task_seen = true;
                look_again = true;
            }
        }
        if look_again {
            self.models_ui.looked_for = None;
        }
        if self.models_ui.install.is_some() || self.models_ui.task.as_ref().is_some_and(|t| t.state.lock().unwrap().running) || self.models_ui.refreshing.load(std::sync::atomic::Ordering::SeqCst) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        let mut open = self.models_ui.open;
        egui::Window::new("Models").open(&mut open).default_size([860.0, 780.0]).default_pos([60.0, 40.0]).vscroll(true).show(ctx, |ui| {
            self.models_body(ui, ctx);
        });
        self.models_ui.open = open;
    }

    fn models_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let snap = self.models_ui.snap();
        let busy = self.models_ui.refreshing.load(std::sync::atomic::Ordering::SeqCst);
        ui.horizontal(|ui| {
            ui.label(muted("Models run on this computer only. Everything they find is a proposal you review."));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!busy, egui::Button::new(if busy { "Looking…" } else { "Refresh" })).clicked() {
                    self.models_ui.looked_for = None;
                }
            });
        });
        let Some(snap) = snap else {
            ui.add(egui::Spinner::new());
            return;
        };
        if let Some((total, free)) = snap.memory {
            ui.label(muted(format!("Memory: {free} MB free of {total} MB.")));
        }

        // ---------------------------------------------------------------- runtimes
        ui.add_space(6.0);
        let installing = self.models_ui.install.as_ref().map(|(c, p, _)| (*c, p.lock().unwrap().clone()));
        let all_ready = snap.runtimes.values().all(|(_, _, p)| matches!(p, Some(Ok(_))));
        let summary = snap
            .runtimes
            .values()
            .map(|(c, _, p)| format!("{} {}", c.name(), if matches!(p, Some(Ok(_))) { "✔" } else { "✖" }))
            .collect::<Vec<_>>()
            .join("   ");
        egui::CollapsingHeader::new(RichText::new(format!("Programs that run the models   {summary}")).heading().size(17.0)).id_salt("runtimes").default_open(!all_ready || installing.is_some()).show(ui, |ui| {
        for (c, path, probe) in snap.runtimes.values() {
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(c.name()).strong());
                    ui.label(muted(c.purpose()));
                });
                match (path, probe) {
                    (Some(p), Some(Ok(pr))) => {
                        ui.label(good(format!("Ready{}", if pr.version.is_empty() || pr.version == "ready" { String::new() } else { format!(", version {}", pr.version) }))).on_hover_text(p.display().to_string());
                        if *c == Component::Ncnn {
                            if pr.gpus.is_empty() {
                                ui.label(RichText::new("No graphics card with a Vulkan driver was found: ncnn will run on the processor.").color(WARN));
                            } else {
                                ui.label(format!("Graphics cards: {}", pr.gpus.join(", ")));
                            }
                        }
                    }
                    (Some(p), Some(Err(e))) => {
                        ui.label(bad(e.clone()));
                        ui.label(muted(p.display().to_string()));
                    }
                    _ => {
                        ui.label(RichText::new("Not installed").color(WARN));
                    }
                }
                ui.horizontal(|ui| {
                    match runtime::release(*c) {
                        Some(r) => {
                            let ok = matches!(probe, Some(Ok(_)));
                            let label = if ok { format!("Reinstall {} {}", c.name(), r.version) } else { format!("Install {} {} ({:.0} MB)", c.name(), r.version, r.bytes as f64 / 1e6) };
                            let running = installing.as_ref().is_some_and(|(ic, _)| ic == c);
                            if running {
                                let p = installing.as_ref().unwrap().1.clone();
                                ui.add(egui::ProgressBar::new(if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 }).desired_width(220.0).text(p.stage));
                            } else if !ok && ui.add_enabled(installing.is_none(), egui::Button::new(label)).on_hover_text(format!("Downloads {}\nand checks its SHA-256 before using it.", r.url)).clicked() {
                                let prog = Arc::new(Mutex::new(Progress::default()));
                                let result = Arc::new(Mutex::new(None));
                                let (p2, r2, c2) = (prog.clone(), result.clone(), *c);
                                std::thread::spawn(move || {
                                    let r = runtime::install(c2, &p2).and_then(|p| runtime::probe(c2, &p).map(|_| p));
                                    *r2.lock().unwrap() = Some(r);
                                });
                                self.models_ui.install = Some((*c, prog, result));
                            }
                        }
                        None => {
                            ui.label(muted(format!("{} publishes no ready-made build for this computer; choose its file instead.", c.name())));
                        }
                    }
                    if *c != Component::Pnnx && ui.button("Choose file…").on_hover_text("Use a copy you already have").clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title(format!("Choose {} ({} or its release archive)", c.name(), c.library_hint())).pick_file() {
                            // a release archive is unpacked first: the library inside is what loads
                            match runtime::adopt_chosen(*c, &p) {
                                Ok(lib) => {
                                    match c {
                                        Component::OnnxRuntime => self.settings.models.ort_path = lib.display().to_string(),
                                        _ => self.settings.models.ncnn_path = lib.display().to_string(),
                                    }
                                    self.settings.save();
                                    self.models_ui.looked_for = None;
                                }
                                Err(e) => self.error(e),
                            }
                        }
                    }
                    let explicit = match c {
                        Component::OnnxRuntime => !self.settings.models.ort_path.is_empty(),
                        Component::Ncnn => !self.settings.models.ncnn_path.is_empty(),
                        _ => false,
                    };
                    if explicit && ui.button("Forget chosen file").clicked() {
                        match c {
                            Component::OnnxRuntime => self.settings.models.ort_path.clear(),
                            _ => self.settings.models.ncnn_path.clear(),
                        }
                        self.settings.save();
                    }
                });
            });
        }
        });
        if let Some((c, Some(_), Some(Ok(p)))) = snap.runtimes.get(&comp_key(Component::Ncnn)) {
            if !p.gpus.is_empty() && p.gpus.len() > 1 {
                ui.horizontal(|ui| {
                    ui.label(format!("{} uses graphics card", c.name()));
                    let mut g = self.settings.models.gpu;
                    egui::ComboBox::from_id_salt("gpu").selected_text(p.gpus.get(g).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                        for (i, n) in p.gpus.iter().enumerate() {
                            ui.selectable_value(&mut g, i, n);
                        }
                    });
                    if g != self.settings.models.gpu {
                        self.settings.models.gpu = g;
                        self.settings.save();
                    }
                });
            }
        }

        // ---------------------------------------------------------------- models
        ui.add_space(8.0);
        ui.heading("Models");
        ui.horizontal(|ui| {
            ui.label("Python environment for conversions");
            let mut v = self.settings.models.python_venv.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut v).hint_text("~/.venv").desired_width(220.0));
            if ui.button("Choose folder…").clicked() {
                if let Some(d) = rfd::FileDialog::new().set_title("Choose a Python virtual environment").pick_folder() {
                    v = d.display().to_string();
                }
            }
            if v != self.settings.models.python_venv {
                self.settings.models.python_venv = v;
                if !r.has_focus() {
                    self.settings.save();
                }
            }
            if r.lost_focus() {
                self.settings.save();
            }
        });
        let where_ = convert::venv_dir(&self.settings.models).display().to_string();
        match &snap.venv {
            Some(v) => ui.label(good(format!("Python {v} in {where_}: PyTorch and the model's code are installed into it when a model is converted"))),
            None => ui.label(muted(format!("{where_} does not exist yet: it is made with the system Python on the first conversion (empty field: LabelDesk's own folder)"))),
        };
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("PP-OCRv6 manga").strong());
            ui.label(muted("Finds text lines and reads them (Kellenok/PP-OCRv6_manga). Runs on the processor."));
            match &snap.ppocr {
                Some(v) => ui.label(good(format!("In the local cache: {v}"))),
                None => ui.label(bad("Not in the local Hugging Face cache")),
            };
        });
        let task_running = self.models_ui.task.as_ref().is_some_and(|t| t.state.lock().unwrap().running);
        for id in ZooId::ALL {
            let found = snap.models.get(&id).cloned();
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(id.name()).strong());
                    ui.label(muted(format!("{} · licence {}", id.repo(), id.license())));
                });
                ui.label(id.does());
                // variant choices
                match id {
                    ZooId::AnimeText => {
                        ui.horizontal(|ui| {
                            ui.label("Size");
                            let mut v = self.settings.models.anime_variant.clone();
                            egui::ComboBox::from_id_salt("anime-size").selected_text(zoo::ANIME_VARIANTS.iter().find(|x| x.0 == v).map(|x| x.1).unwrap_or("?")).show_ui(ui, |ui| {
                                for (k, label) in zoo::ANIME_VARIANTS {
                                    ui.selectable_value(&mut v, k.to_string(), label);
                                }
                            });
                            if v != self.settings.models.anime_variant {
                                self.settings.models.anime_variant = v;
                                self.settings.save();
                            }
                        });
                    }
                    ZooId::KoharuLayout => {
                        ui.horizontal(|ui| {
                            ui.label("Picture size");
                            let mut r = self.settings.models.layout_resolution;
                            egui::ComboBox::from_id_salt("layout-res").selected_text(format!("{r} px")).show_ui(ui, |ui| {
                                for x in zoo::LAYOUT_RESOLUTIONS {
                                    ui.selectable_value(&mut r, x, format!("{x} px{}", if x == 1920 { " (the model's own, best)" } else { " (faster, less memory, misses more small text)" }));
                                }
                            });
                            if r != self.settings.models.layout_resolution {
                                self.settings.models.layout_resolution = r;
                                self.settings.save();
                            }
                        });
                    }
                    _ => {}
                }
                match &found {
                    Some(Ok(l)) => {
                        let onnx = l.files.onnx.is_some();
                        let ncnn = l.files.ncnn.is_some();
                        ui.label(good(format!("In the local cache · revision {}", &l.revision[..l.revision.len().min(10)])));
                        ui.horizontal(|ui| {
                            ui.label(if onnx { good("✔ ONNX (processor)") } else { RichText::new("✖ no ONNX yet").color(WARN) });
                            if id.ncnn_possible() {
                                ui.label(if ncnn { good(format!("✔ ncnn ({0}×{0})", id.ncnn_input(&self.settings.models))) } else { muted("✖ not converted to ncnn") });
                            } else {
                                ui.label(muted("ncnn: not possible for this model (decoding is inside its graph); it is fast on the processor"));
                            }
                        });
                        // where it runs
                        ui.horizontal(|ui| {
                            ui.label("Runs on");
                            let mut b = self.settings.models.backend(id);
                            egui::ComboBox::from_id_salt(("backend", id.key())).selected_text(b.name()).show_ui(ui, |ui| {
                                for x in Backend::ALL {
                                    let possible = x == Backend::Onnx || id.ncnn_possible();
                                    ui.add_enabled_ui(possible, |ui| {
                                        ui.selectable_value(&mut b, x, x.name());
                                    });
                                }
                            });
                            if b != self.settings.models.backend(id) {
                                self.settings.models.backends.insert(id, b);
                                self.settings.save();
                            }
                            let need = id.memory_mb(&self.settings.models);
                            if let Some((_, free)) = snap.memory {
                                if need > free {
                                    ui.label(RichText::new(format!("needs about {:.1} GB free; {:.1} GB are", need as f64 / 1024.0, free as f64 / 1024.0)).color(WARN).size(11.5));
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            if id.needs_export() && !onnx {
                                let tip = match &snap.python {
                                    Some(v) if snap.venv.is_some() => format!("Uses your Python {v} environment, installs PyTorch and the model's code there if missing (about 1 GB), then writes the ONNX model. Takes many minutes."),
                                    Some(v) => format!("Uses Python {v} once to make the environment with PyTorch (about 1 GB on disk), then writes the ONNX model. Takes many minutes."),
                                    None => "Needs Python 3.10 or newer once (python.org). Then LabelDesk makes a private environment with PyTorch and writes the ONNX model.".to_string(),
                                };
                                if ui.add_enabled(!task_running, egui::Button::new("Convert to ONNX")).on_hover_text(tip).clicked() {
                                    let o = self.settings.models.clone();
                                    self.models_ui.start_task(id, Target::Onnx, o);
                                }
                            }
                            if id.ncnn_possible() {
                                let label = if ncnn { "Convert to ncnn again" } else { "Convert to ncnn (graphics card)" };
                                let tip = if onnx { "Converts the ONNX model with pnnx (installed when needed). A few minutes." } else { "Makes the ONNX model first, then converts it with pnnx." };
                                if ui.add_enabled(!task_running, egui::Button::new(label)).on_hover_text(tip).clicked() {
                                    let o = self.settings.models.clone();
                                    self.models_ui.start_task(id, Target::Ncnn, o);
                                }
                            }
                            if onnx || ncnn {
                                if ui.add_enabled(l.converted.exists(), egui::Button::new("Open folder")).on_hover_text(l.converted.display().to_string()).clicked() {
                                    let _ = open_folder(&l.converted);
                                }
                            }
                        });
                        if id.needs_export() || id.ncnn_possible() {
                            ui.label(muted("Converted copies go in LabelDesk's data folder (point here to see where)")).on_hover_text(l.converted.display().to_string());
                        }
                    }
                    Some(Err(e)) => {
                        ui.label(bad(e.clone()));
                    }
                    None => {
                        ui.label(muted("Looking…"));
                    }
                }
                if let Some(m) = self.models_ui.loaded.get(&id) {
                    ui.label(muted(format!("Last run here: {} · {} file(s) checked", m.runtime, m.artifacts.len())));
                }
            });
        }

        // ---------------------------------------------------------------- project choice
        if self.store.is_some() {
            ui.add_space(8.0);
            ui.heading("This project");
            let mut reader = self.cfg().model.reader;
            ui.horizontal(|ui| {
                ui.label("Read a selection again with");
                egui::ComboBox::from_id_salt("reader").selected_text(reader.name()).show_ui(ui, |ui| {
                    ui.selectable_value(&mut reader, Reader::PpOcr, Reader::PpOcr.name());
                    ui.selectable_value(&mut reader, Reader::Charspot, Reader::Charspot.name());
                });
            });
            if reader != self.cfg().model.reader {
                if let Some(s) = self.store.as_mut() {
                    s.manifest.config.model.reader = reader;
                    s.manifest_dirty = true;
                }
            }
            ui.label(muted("Every run records the exact model files (with their SHA-256) and what ran them, in the page's inference runs."));
        }

        // ---------------------------------------------------------------- conversion log
        if let Some(t) = self.models_ui.task.clone() {
            ui.add_space(8.0);
            let s = t.state.lock().unwrap().clone();
            ui.horizontal(|ui| {
                ui.heading(format!("Converting {}", t.id.name()));
                if s.running {
                    ui.add(egui::Spinner::new());
                    if ui.button("Stop").clicked() {
                        t.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                } else if ui.button("Close log").clicked() {
                    self.models_ui.task = None;
                }
            });
            match (&s.error, s.done) {
                (Some(e), _) => {
                    ui.label(bad(e.clone()));
                }
                (None, true) => {
                    ui.label(good(s.stage.clone()));
                }
                _ => {
                    ui.label(s.stage.clone());
                }
            }
            egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).id_salt("convert-log").show(ui, |ui| {
                for l in s.log.iter().rev().take(400).rev() {
                    ui.label(RichText::new(l).monospace().size(11.0).color(Color32::from_gray(190)));
                }
            });
        }
        let _ = ctx;
    }
}

fn open_folder(p: &std::path::Path) -> std::io::Result<()> {
    let prog = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    std::process::Command::new(prog).arg(p).spawn().map(|_| ())
}
