//! Converting models so LabelDesk can run them.
//!
//! * **PyTorch → ONNX** (Koharu layout and text masks, which ship as safetensors only): LabelDesk
//!   makes a private Python environment in its data folder, installs PyTorch (processor build)
//!   and the model's own code (RF-DETR 1.7.0, or Hi-SAM at a fixed commit), and runs
//!   `convert/export_koharu.py`. The system Python is never changed.
//! * **ONNX → ncnn** with pnnx, for the graphics card.
//!
//! A conversion runs in the background and writes a log; when it ends, `conversion.json` next to
//! the result records the source files with their SHA-256, the tools and the commands.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::runtime::{self, Component};
use crate::zoo::{self, ZooId};

pub const EXPORT_SCRIPT: &str = include_str!("../convert/export_koharu.py");
pub const RFDETR_VERSION: &str = "1.7.0";
pub const HISAM_COMMIT: &str = "69009434d4dba5541f228d8f5acb0754c333d417";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// PyTorch weights → ONNX.
    Onnx,
    /// ONNX → ncnn (making the ONNX first when the model has none).
    Ncnn,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub running: bool,
    pub done: bool,
    pub error: Option<String>,
    pub log: Vec<String>,
    pub stage: String,
    pub progress: runtime::Progress,
}

#[derive(Clone)]
pub struct Task {
    pub id: ZooId,
    pub target: Target,
    pub state: Arc<Mutex<State>>,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl Task {
    pub fn start(id: ZooId, target: Target, o: zoo::Options) -> Task {
        let state = Arc::new(Mutex::new(State { running: true, ..Default::default() }));
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let t = Task { id, target, state: state.clone(), cancel: cancel.clone() };
        std::thread::Builder::new()
            .name("convert".into())
            .spawn(move || {
                let r = convert(id, target, &o, &state, &cancel);
                let mut s = state.lock().unwrap();
                s.running = false;
                match r {
                    Ok(msg) => {
                        s.done = true;
                        s.log.push(msg.clone());
                        s.stage = msg;
                    }
                    Err(e) => {
                        s.log.push(format!("Stopped: {e}"));
                        s.error = Some(e);
                    }
                }
            })
            .expect("convert thread");
        t
    }
}

fn say(state: &Mutex<State>, msg: impl Into<String>) {
    let m = msg.into();
    let mut s = state.lock().unwrap();
    s.stage = m.clone();
    s.log.push(m);
    if s.log.len() > 4000 {
        s.log.drain(..1000);
    }
}

/// Run a command, streaming its output into the log. Fails on a non-zero exit.
fn run(state: &Mutex<State>, cancel: &std::sync::atomic::AtomicBool, mut cmd: Command, what: &str) -> Result<(), String> {
    say(state, format!("{what}…"));
    {
        let mut s = state.lock().unwrap();
        let line = format!("$ {:?}", cmd);
        s.log.push(line);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("{what}: could not start ({e})"))?;
    let (out, err) = (child.stdout.take(), child.stderr.take());
    let st = Arc::new(Mutex::new(Vec::<String>::new()));
    let pump = |r: Box<dyn std::io::Read + Send>, sink: Arc<Mutex<Vec<String>>>| {
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
            }
        })
    };
    let h1 = out.map(|o| pump(Box::new(o), st.clone()));
    let h2 = err.map(|e| pump(Box::new(e), st.clone()));
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("stopped by you".into());
        }
        let lines: Vec<String> = std::mem::take(&mut *st.lock().unwrap());
        if !lines.is_empty() {
            let mut s = state.lock().unwrap();
            s.log.extend(lines);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    for h in [h1, h2].into_iter().flatten() {
        let _ = h.join();
    }
    let lines: Vec<String> = std::mem::take(&mut *st.lock().unwrap());
    state.lock().unwrap().log.extend(lines);
    if status.success() {
        Ok(())
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if status.signal() == Some(9) {
                let mem = runtime::memory_mb().map(|(t, _)| format!(" (this computer has {:.1} GB)", t as f64 / 1024.0)).unwrap_or_default();
                return Err(format!("{what}: the system stopped it, almost certainly for lack of memory{mem}. Close other programs, choose a smaller picture size where the model offers one, or convert on a computer with more memory and copy its converted folder (shown in the Models window) to the same place here."));
            }
        }
        let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "a signal".into());
        let tail = state.lock().unwrap().log.iter().rev().find(|l| !l.trim().is_empty() && !l.starts_with('$')).cloned().unwrap_or_default();
        Err(format!("{what} failed (exit {code}): {tail}"))
    }
}

/// A Python 3.10 or newer on this computer.
pub fn find_python() -> Option<(PathBuf, String)> {
    let names: &[&str] = if cfg!(windows) { &["py", "python", "python3"] } else { &["python3", "python"] };
    for n in names {
        let mut c = Command::new(n);
        if *n == "py" {
            c.arg("-3");
        }
        c.arg("-c").arg("import sys; print('%d.%d' % sys.version_info[:2]); print(sys.executable)");
        if let Ok(out) = runtime::run_with_timeout(c, std::time::Duration::from_secs(20)) {
            if out.0 == Some(0) {
                let text = String::from_utf8_lossy(&out.1).to_string();
                let mut lines = text.lines();
                let ver = lines.next().unwrap_or("").trim().to_string();
                let exe = lines.next().unwrap_or("").trim().to_string();
                let ok = ver.split_once('.').and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))).is_some_and(|v| v >= (3, 10));
                if ok && !exe.is_empty() {
                    return Some((PathBuf::from(exe), ver));
                }
            }
        }
    }
    None
}

pub fn venv_dir() -> PathBuf {
    runtime::data_dir().join("converter").join("venv")
}

fn venv_python() -> PathBuf {
    if cfg!(windows) { venv_dir().join("Scripts").join("python.exe") } else { venv_dir().join("bin").join("python") }
}

/// Make the private environment and install what the model's exporter needs.
fn ensure_python(id: ZooId, state: &Mutex<State>, cancel: &std::sync::atomic::AtomicBool) -> Result<PathBuf, String> {
    let py = venv_python();
    if !py.exists() {
        let (sys_py, ver) = find_python().ok_or("Python 3.10 or newer is needed once, to convert this model. Install it from python.org, then try again.")?;
        say(state, format!("Using Python {ver} to make a private environment"));
        let mut c = Command::new(sys_py);
        c.arg("-m").arg("venv").arg(venv_dir());
        run(state, cancel, c, "Making the private Python environment")?;
    }
    let pip = |args: &[&str]| {
        let mut c = Command::new(&py);
        c.arg("-m").arg("pip").arg("install").arg("--disable-pip-version-check").args(args);
        c
    };
    let have = |module: &str| {
        let mut c = Command::new(&py);
        c.arg("-c").arg(format!("import {module}"));
        runtime::run_with_timeout(c, std::time::Duration::from_secs(120)).map(|o| o.0 == Some(0)).unwrap_or(false)
    };
    const CPU: &str = "https://download.pytorch.org/whl/cpu";
    if !have("torch") || !have("torchvision") {
        run(state, cancel, pip(&["torch", "torchvision", "--index-url", CPU]), "Installing PyTorch (processor build, about 300 MB)")?;
    }
    let mut extra: Vec<&str> = vec!["safetensors", "onnx", "numpy", "pillow", "--extra-index-url", CPU];
    let rf = format!("rfdetr=={RFDETR_VERSION}");
    match id {
        ZooId::KoharuLayout => {
            if !have("rfdetr") {
                extra.insert(0, &rf);
            }
        }
        _ => extra.insert(0, "einops"),
    }
    run(state, cancel, pip(&extra), "Installing the model's code")?;
    // a package may have pulled a torchvision built for another PyTorch: put the matching one back
    if !have("torch, torchvision; torchvision.ops.nms(torch.zeros(1, 4), torch.zeros(1), 0.5)") {
        run(state, cancel, pip(&["--force-reinstall", "--no-deps", "torchvision", "--index-url", CPU]), "Matching torchvision to PyTorch")?;
    }
    Ok(py)
}

/// Hi-SAM's code at the fixed commit, downloaded once.
fn ensure_hisam(state: &Mutex<State>) -> Result<PathBuf, String> {
    let root = runtime::data_dir().join("converter").join(format!("Hi-SAM-{HISAM_COMMIT}"));
    if root.join("hi_sam").join("modeling").join("build.py").exists() {
        return Ok(root);
    }
    let url = format!("https://github.com/ymy-k/Hi-SAM/archive/{HISAM_COMMIT}.zip");
    let zip = runtime::data_dir().join("downloads").join("hi-sam.zip");
    say(state, "Downloading Hi-SAM's code (the text mask model's own code)");
    let sha = runtime::download(&url, &zip, None, &Mutex::new(runtime::Progress::default()))?;
    say(state, format!("Hi-SAM archive SHA-256 {sha}"));
    runtime::unpack(&zip, &runtime::data_dir().join("converter"))?;
    let _ = std::fs::remove_file(&zip);
    if root.join("hi_sam").exists() {
        Ok(root)
    } else {
        Err("Hi-SAM's code was downloaded but not where expected".into())
    }
}

fn ensure_component(c: Component, state: &Mutex<State>) -> Result<PathBuf, String> {
    if let Some(p) = runtime::locate(c, None) {
        if runtime::probe(c, &p).is_ok() {
            return Ok(p);
        }
    }
    say(state, format!("Installing {}", c.name()));
    let prog = Mutex::new(runtime::Progress::default());
    let p = runtime::install(c, &prog)?;
    runtime::probe(c, &p)?;
    Ok(p)
}

fn record(dir: &Path, id: ZooId, sources: &[(String, PathBuf)], tools: serde_json::Value, outputs: &[&str]) -> Result<(), String> {
    let mut src = Vec::new();
    for (role, p) in sources {
        src.push(serde_json::json!({"role": role, "file": p.display().to_string(), "sha256": runtime::sha256_cached(p)?}));
    }
    let mut out = Vec::new();
    for f in outputs {
        let p = dir.join(f);
        if p.exists() {
            out.push(serde_json::json!({"file": f, "sha256": runtime::sha256_cached(&p)?, "bytes": std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0)}));
        }
    }
    let path = dir.join("conversion.json");
    let mut doc: serde_json::Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| serde_json::json!({"model": id.repo(), "steps": []}));
    if let Some(steps) = doc.get_mut("steps").and_then(|s| s.as_array_mut()) {
        steps.push(serde_json::json!({"at": crate::util::now(), "sources": src, "tools": tools, "outputs": out}));
    }
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap_or_default()).map_err(|e| e.to_string())
}

fn convert(id: ZooId, target: Target, o: &zoo::Options, state: &Mutex<State>, cancel: &std::sync::atomic::AtomicBool) -> Result<String, String> {
    let l = zoo::locate(id, o)?;
    std::fs::create_dir_all(&l.converted).map_err(|e| e.to_string())?;
    let mut onnx = l.files.onnx.clone();
    if id.needs_export() && onnx.is_none() {
        if let (ZooId::KoharuLayout, Some((_, free))) = (id, runtime::memory_mb()) {
            say(state, format!("This computer has about {free} MB of memory free; converting needs roughly 3 GB."));
        }
        let py = ensure_python(id, state, cancel)?;
        let script = runtime::data_dir().join("converter").join("export_koharu.py");
        std::fs::create_dir_all(script.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&script, EXPORT_SCRIPT).map_err(|e| e.to_string())?;
        let out = l.converted.join("model.onnx");
        let part = l.converted.join("model.onnx.part");
        let weights = l.sources.iter().find(|(r, _)| r == "weights").map(|(_, p)| p.clone()).ok_or("weights missing")?;
        let mut c = Command::new(&py);
        c.arg(&script);
        let tools = match id {
            ZooId::KoharuLayout => {
                let config = l.sources.iter().find(|(r, _)| r == "config").map(|(_, p)| p.clone()).ok_or("config missing")?;
                c.arg("layout").arg("--weights").arg(&weights).arg("--config").arg(&config).arg("--resolution").arg(o.layout_resolution.to_string());
                serde_json::json!({"exporter": "export_koharu.py layout", "rfdetr": RFDETR_VERSION, "resolution": o.layout_resolution, "attention": "computed in slices"})
            }
            _ => {
                let root = ensure_hisam(state)?;
                c.arg("textmask").arg("--weights").arg(&weights).arg("--hi-sam-root").arg(&root);
                serde_json::json!({"exporter": "export_koharu.py textmask", "hi_sam_commit": HISAM_COMMIT, "attention": "computed in slices"})
            }
        };
        c.arg("--out").arg(&part);
        let _ = std::fs::remove_file(&part);
        run(state, cancel, c, "Converting the PyTorch weights to ONNX (can take many minutes)")?;
        std::fs::rename(&part, &out).map_err(|e| e.to_string())?;
        record(&l.converted, id, &l.sources, tools, &["model.onnx"])?;
        say(state, format!("ONNX model written ({:.0} MB)", std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) as f64 / 1e6));
        onnx = Some(out);
    }
    if target == Target::Onnx {
        return Ok("Ready to run on the processor.".into());
    }
    if !id.ncnn_possible() {
        return Err(format!("{} can't be converted to ncnn: its box decoding and reading order are inside the ONNX graph, which ncnn can't express. It runs on the processor (about 20 ms per crop).", id.name()));
    }
    let onnx = onnx.ok_or("there is no ONNX model to convert")?;
    let pnnx = ensure_component(Component::Pnnx, state)?;
    let work = l.converted.join("pnnx-work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let size = id.ncnn_input(o);
    let (param, bin) = (l.converted.join("model.ncnn.param"), l.converted.join("model.ncnn.bin"));
    let mut c = Command::new(&pnnx);
    c.current_dir(&work)
        .arg(&onnx)
        .arg(format!("inputshape=[1,3,{size},{size}]"))
        .arg(format!("ncnnparam={}", param.display()))
        .arg(format!("ncnnbin={}", bin.display()))
        .arg(format!("pnnxparam={}", work.join("m.pnnx.param").display()))
        .arg(format!("pnnxbin={}", work.join("m.pnnx.bin").display()))
        .arg(format!("pnnxpy={}", work.join("m_pnnx.py").display()))
        .arg(format!("pnnxonnx={}", work.join("m.pnnx.onnx").display()))
        .arg(format!("ncnnpy={}", work.join("m_ncnn.py").display()));
    let r = run(state, cancel, c, &format!("Converting to ncnn with pnnx ({size}×{size} input)"));
    let _ = std::fs::remove_dir_all(&work);
    if r.is_err() {
        let _ = std::fs::remove_file(&param);
        let _ = std::fs::remove_file(&bin);
    }
    r?;
    if !param.exists() || !bin.exists() {
        return Err("pnnx finished but wrote no ncnn model; see the log above".into());
    }
    let pnnx_ver = runtime::release(Component::Pnnx).map(|r| r.version).unwrap_or("local");
    record(&l.converted, id, &[("onnx".to_string(), onnx)], serde_json::json!({"pnnx": pnnx_ver, "inputshape": [1, 3, size, size]}), &["model.ncnn.param", "model.ncnn.bin"])?;
    Ok("Ready to run on the graphics card.".into())
}
