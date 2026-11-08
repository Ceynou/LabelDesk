//! The programs LabelDesk runs models with, found or installed on this computer.
//!
//! * **ONNX Runtime** runs `.onnx` models on the processor. It is loaded only when a model is
//!   first used, never at start-up: a copy built for newer processors (AVX) would otherwise stop
//!   the whole app on older ones. Every library is first tried in a separate process
//!   (`labeldesk --probe-runtime ...`), so a copy that can't run here is reported, not fatal.
//! * **ncnn** runs converted models on any graphics card with Vulkan (NVIDIA, AMD, Intel, Apple
//!   through MoltenVK), or on the processor.
//! * **pnnx** converts ONNX models to ncnn.
//!
//! Downloads are the projects' official releases, pinned by version and SHA-256; a download whose
//! checksum differs is thrown away. Everything goes into the app's own data folder.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Where LabelDesk keeps runtimes, converted models and its converter.
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("LABELDESK_DATA_DIR") {
        return PathBuf::from(p);
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(p).join("LabelDesk");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = std::env::var_os("HOME") {
            return PathBuf::from(h).join("Library").join("Application Support").join("LabelDesk");
        }
    }
    if let Some(p) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(p).join("labeldesk");
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(h).join(".local").join("share").join("labeldesk");
    }
    std::env::temp_dir().join("labeldesk")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Component {
    OnnxRuntime,
    Ncnn,
    Pnnx,
}

impl Component {
    pub const ALL: [Component; 3] = [Component::OnnxRuntime, Component::Ncnn, Component::Pnnx];
    pub fn name(self) -> &'static str {
        match self {
            Component::OnnxRuntime => "ONNX Runtime",
            Component::Ncnn => "ncnn",
            Component::Pnnx => "pnnx",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Component::OnnxRuntime => "onnxruntime",
            Component::Ncnn => "ncnn",
            Component::Pnnx => "pnnx",
        }
    }
    pub fn purpose(self) -> &'static str {
        match self {
            Component::OnnxRuntime => "Runs ONNX models on the processor.",
            Component::Ncnn => "Runs converted models on the graphics card (Vulkan) or the processor.",
            Component::Pnnx => "Converts ONNX models to ncnn.",
        }
    }
    /// File names that identify the component inside its folder.
    fn file_matches(self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        match self {
            Component::OnnxRuntime => n == "onnxruntime.dll" || n.starts_with("libonnxruntime.so") || (n.starts_with("libonnxruntime.") && n.ends_with(".dylib")),
            Component::Ncnn => n == "ncnn.dll" || n.starts_with("libncnn.so") || (n.starts_with("libncnn") && n.ends_with(".dylib")),
            Component::Pnnx => n == "pnnx" || n == "pnnx.exe",
        }
    }
    fn env_var(self) -> &'static str {
        match self {
            Component::OnnxRuntime => "ORT_DYLIB_PATH",
            Component::Ncnn => "LABELDESK_NCNN",
            Component::Pnnx => "LABELDESK_PNNX",
        }
    }
}

/// One official release file.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

/// The pinned release for this platform, if the project publishes one.
pub fn release(c: Component) -> Option<Release> {
    let r = |version, url, sha256, bytes| Some(Release { version, url, sha256, bytes });
    match (c, std::env::consts::OS, std::env::consts::ARCH) {
        (Component::OnnxRuntime, "linux", "x86_64") => r("1.31.1", "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-linux-x64-1.31.1.tgz", "c849e36a1ef3e22d4ff217b9d1ce4a3cf5b9467b975610913ae959ad5b16bdc9", 11413655),
        (Component::OnnxRuntime, "windows", "x86_64") => r("1.31.1", "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-win-x64-1.31.1.zip", "5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c", 82929944),
        (Component::OnnxRuntime, "macos", "aarch64") => r("1.31.1", "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-osx-arm64-1.31.1.tgz", "2e1344a1926ee50eb25b38153cb6992c3567515aef0e677a1a520a1798896d6f", 36936435),
        (Component::Ncnn, "linux", "x86_64") => r("20261020", "https://github.com/Tencent/ncnn/releases/download/20261020/ncnn-20261020-ubuntu-2204-shared.zip", "d3152bf1bc0003b41f7cc8f50636df4fa28c87e63afaa21122e5f4bf759f50ca", 10292422),
        (Component::Ncnn, "windows", "x86_64") => r("20261020", "https://github.com/Tencent/ncnn/releases/download/20261020/ncnn-20261020-windows-vs2022-shared.zip", "7f39107af5ee4c1e657b8fd01e8b3cad6c0ab7fdbb0713532f01937134285b3d", 16729350),
        (Component::Pnnx, "linux", "x86_64") => r("20261020", "https://github.com/pnnx/pnnx/releases/download/20261020/pnnx-20261020-linux.zip", "6cfff631178e02e62542e72bf14e0045dfce62095bbc1f633176e9c0778ab8a6", 29228181),
        (Component::Pnnx, "linux", "aarch64") => r("20261020", "https://github.com/pnnx/pnnx/releases/download/20261020/pnnx-20261020-linux-aarch64.zip", "15621b585d80bb0c49c7c52ca9b95e04221034332f532448a3e8a4ec94e84502", 24612543),
        (Component::Pnnx, "windows", "x86_64") => r("20261020", "https://github.com/pnnx/pnnx/releases/download/20261020/pnnx-20261020-windows.zip", "e4f91348a399a0060a6e7dd323d40427e1cbdcab67469cf1904b5d09b23606c5", 18244293),
        (Component::Pnnx, "macos", _) => r("20261020", "https://github.com/pnnx/pnnx/releases/download/20261020/pnnx-20261020-macos.zip", "03ed124a4d9351c3bcadbd7bdef1f59851746bfcf80bd87b01aa766abb2766f7", 54529889),
        _ => None,
    }
}

fn component_dir(c: Component) -> PathBuf {
    data_dir().join("runtimes").join(c.id())
}

fn find_file(dir: &Path, c: Component, depth: usize) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let rd = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if c.file_matches(&e.file_name().to_string_lossy()) {
            found.push(p);
        }
    }
    // the plain name (libonnxruntime.so) loads its versioned twin; prefer the shortest name
    found.sort_by_key(|p| p.file_name().map(|n| n.len()).unwrap_or(0));
    if let Some(p) = found.into_iter().next() {
        return Some(p);
    }
    if depth == 0 {
        return None;
    }
    subdirs.sort();
    // Windows packages carry x64 and x86 builds side by side
    subdirs.sort_by_key(|p| !p.to_string_lossy().contains("x64"));
    subdirs.into_iter().find_map(|d| find_file(&d, c, depth - 1))
}

/// Where a component is: an explicit path from the settings, its environment variable, the
/// copy LabelDesk installed, or a copy next to the program.
pub fn locate(c: Component, explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit.filter(|p| !p.as_os_str().is_empty()) {
        return Some(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os(c.env_var()).map(PathBuf::from).filter(|p| p.exists()) {
        return Some(p);
    }
    if let Some(p) = find_file(&component_dir(c), c, 5) {
        return Some(p);
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    find_file(&exe_dir, c, 0)
}

// ------------------------------------------------------------------------------- probing

#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    pub version: String,
    /// Graphics cards ncnn can use (ncnn only).
    pub gpus: Vec<String>,
}

static PROBES: Mutex<Option<HashMap<PathBuf, Result<Probe, String>>>> = Mutex::new(None);

/// Try a library in a separate process and remember the answer for this session.
pub fn probe(c: Component, path: &Path) -> Result<Probe, String> {
    if let Some(r) = PROBES.lock().unwrap().get_or_insert_with(HashMap::new).get(path) {
        return r.clone();
    }
    let r = probe_uncached(c, path);
    PROBES.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.to_path_buf(), r.clone());
    r
}

pub fn forget_probes() {
    *PROBES.lock().unwrap() = None;
}

fn probe_uncached(c: Component, path: &Path) -> Result<Probe, String> {
    if !path.exists() {
        return Err(format!("{} is not there any more", path.display()));
    }
    if c == Component::Pnnx {
        let out = run_with_timeout(Command::new(path), Duration::from_secs(30)).map_err(|e| format!("pnnx could not start: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.1), String::from_utf8_lossy(&out.2));
        return if text.contains("pnnx") || text.contains("Usage") { Ok(Probe { version: "ready".into(), gpus: Vec::new() }) } else { Err("pnnx did not answer as expected".into()) };
    }
    if cfg!(test) {
        // tests run inside the test harness, which has no --probe-runtime entry point
        // (loading ONNX Runtime twice in one process is not allowed; `ort_env` loads it)
        return match c {
            Component::OnnxRuntime => Ok(Probe { version: "test".into(), gpus: Vec::new() }),
            _ => crate::ncnn::Ncnn::open(path).map(|n| Probe { version: n.version(), gpus: n.gpus() }),
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("--probe-runtime").arg(c.id()).arg(path);
    let (status, stdout, stderr) = run_with_timeout(cmd, Duration::from_secs(90)).map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&stdout);
    match status {
        Some(0) => {
            let mut p = Probe { version: String::new(), gpus: Vec::new() };
            for line in out.lines() {
                if let Some(v) = line.strip_prefix("version ") {
                    p.version = v.trim().to_string();
                } else if let Some(g) = line.strip_prefix("gpu ") {
                    p.gpus.push(g.trim().to_string());
                }
            }
            Ok(p)
        }
        Some(code) => {
            let err = String::from_utf8_lossy(&stderr);
            let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            Err(format!("{} could not be loaded (code {code}){}", c.name(), if last.is_empty() { String::new() } else { format!(": {last}") }))
        }
        None => Err(format!(
            "This copy of {} stops on this processor (it was built for newer processors). Install the official one from the Models window.",
            c.name()
        )),
    }
}

/// Run a command with a deadline; `None` status means it was killed by a signal (or the deadline).
pub fn run_with_timeout(mut cmd: Command, limit: Duration) -> std::io::Result<(Option<i32>, Vec<u8>, Vec<u8>)> {
    let dir = std::env::temp_dir();
    let id = crate::util::new_id("p");
    let (o, e) = (dir.join(format!("ld-{id}.out")), dir.join(format!("ld-{id}.err")));
    cmd.stdout(std::fs::File::create(&o)?).stderr(std::fs::File::create(&e)?).stdin(std::process::Stdio::null());
    let mut child = cmd.spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s.code();
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = std::fs::read(&o).unwrap_or_default();
    let err = std::fs::read(&e).unwrap_or_default();
    let _ = std::fs::remove_file(&o);
    let _ = std::fs::remove_file(&e);
    Ok((status, out, err))
}

/// `labeldesk --probe-runtime <component> <path>`: load the library and report. Runs in the child.
pub fn probe_main(component: &str, path: &Path) -> i32 {
    match component {
        "onnxruntime" => match probe_ort_here(path) {
            Ok(v) => {
                println!("version {v}");
                0
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        "ncnn" => match crate::ncnn::Ncnn::open(path) {
            Ok(n) => {
                println!("version {}", n.version());
                for g in n.gpus() {
                    println!("gpu {g}");
                }
                0
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        _ => 3,
    }
}

fn probe_ort_here(path: &Path) -> Result<String, String> {
    let env = ort::init_from(path).map_err(|e| e.to_string())?.with_name("labeldesk-probe").build().map_err(|e| e.to_string())?;
    drop(env);
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let v = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let ver = v.trim_start_matches("libonnxruntime.so.").trim_start_matches("libonnxruntime.").trim_end_matches(".dylib").to_string();
    Ok(if ver.chars().next().is_some_and(|c| c.is_ascii_digit()) { ver } else { "loaded".into() })
}

// ---------------------------------------------------------------------------- ONNX Runtime

struct OrtState {
    env: &'static ort::environment::Environment,
    path: PathBuf,
    version: String,
}

static ORT: Mutex<Option<OrtState>> = Mutex::new(None);

/// The ONNX Runtime environment, loading the library the first time it is needed.
pub fn ort_env(explicit: Option<&Path>) -> Result<&'static ort::environment::Environment, String> {
    let mut g = ORT.lock().unwrap();
    if let Some(s) = g.as_ref() {
        return Ok(s.env);
    }
    let path = locate(Component::OnnxRuntime, explicit).ok_or_else(|| {
        "ONNX Runtime is not installed. Open AI → Models… and press Install next to ONNX Runtime (about 11 MB, from Microsoft's official release).".to_string()
    })?;
    let p = probe(Component::OnnxRuntime, &path)?;
    let env = ort::init_from(&path).map_err(|e| e.to_string())?.with_name("labeldesk").build().map_err(|e| e.to_string())?;
    let env: &'static ort::environment::Environment = Box::leak(Box::new(env));
    *g = Some(OrtState { env, path, version: p.version });
    Ok(env)
}

/// The loaded ONNX Runtime, if any: (library, version).
pub fn ort_loaded() -> Option<(PathBuf, String)> {
    ORT.lock().unwrap().as_ref().map(|s| (s.path.clone(), s.version.clone()))
}

pub fn ort_runtime_label() -> String {
    match ort_loaded() {
        Some((_, v)) => format!("ONNX Runtime {v} (processor)"),
        None => "ONNX Runtime (processor)".into(),
    }
}

// -------------------------------------------------------------------------------- install

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub stage: String,
}

/// Download `url` to `dest`, checking its SHA-256 when given. Returns the actual checksum.
pub fn download(url: &str, dest: &Path, expect_sha: Option<&str>, progress: &Mutex<Progress>) -> Result<String, String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let resp = ureq::get(url).header("User-Agent", "LabelDesk").call().map_err(|e| format!("download failed: {e}"))?;
    let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.into_body().into_reader();
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if let Ok(mut p) = progress.lock() {
            p.done = done;
            p.total = total;
        }
    }
    drop(f);
    let sha = format!("{:x}", h.finalize());
    if let Some(e) = expect_sha {
        if !sha.eq_ignore_ascii_case(e) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("the download's checksum is {sha}, expected {e}; it was thrown away"));
        }
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(sha)
}

/// Unpack a .zip or .tgz into `dir`.
pub fn unpack(archive: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = archive.to_string_lossy().to_ascii_lowercase();
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    if name.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        z.extract(dir).map_err(|e| e.to_string())?;
    } else {
        let gz = flate2::read::GzDecoder::new(f);
        let mut t = tar::Archive::new(gz);
        t.unpack(dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Install the pinned release of a component. Returns the file to load or run.
pub fn install(c: Component, progress: &Mutex<Progress>) -> Result<PathBuf, String> {
    let rel = release(c).ok_or_else(|| format!("{} publishes no ready-made build for this computer ({} {}); set its location in the Models window instead.", c.name(), std::env::consts::OS, std::env::consts::ARCH))?;
    let dir = component_dir(c).join(rel.version);
    if let Some(p) = find_file(&dir, c, 5) {
        return Ok(p);
    }
    let file = rel.url.rsplit('/').next().unwrap_or("download");
    let tmp = data_dir().join("downloads").join(file);
    progress.lock().unwrap().stage = format!("Downloading {} {}", c.name(), rel.version);
    let sha = download(rel.url, &tmp, Some(rel.sha256), progress)?;
    progress.lock().unwrap().stage = format!("Unpacking {}", c.name());
    let staging = dir.with_extension("unpacking");
    let _ = std::fs::remove_dir_all(&staging);
    unpack(&tmp, &staging)?;
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&staging, &dir).map_err(|e| e.to_string())?;
    let note = serde_json::json!({"component": c.id(), "version": rel.version, "url": rel.url, "sha256": sha, "installed": crate::util::now()});
    let _ = std::fs::write(dir.join("SOURCE.json"), serde_json::to_string_pretty(&note).unwrap_or_default());
    let found = find_file(&dir, c, 5).ok_or_else(|| format!("{} was unpacked but its library was not found inside", c.name()))?;
    #[cfg(unix)]
    if c == Component::Pnnx {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&found, std::fs::Permissions::from_mode(0o755));
    }
    forget_probes();
    Ok(found)
}

// ---------------------------------------------------------------------------- checksums

static HASHES: Mutex<Option<HashMap<(PathBuf, u64, u128), String>>> = Mutex::new(None);

/// SHA-256 of a file, remembered by size and modification time (model files are large).
pub fn sha256_cached(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mtime = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
    let key = (path.to_path_buf(), meta.len(), mtime);
    if let Some(h) = HASHES.lock().unwrap().get_or_insert_with(HashMap::new).get(&key) {
        return Ok(h.clone());
    }
    let cache_file = data_dir().join("hash-cache.json");
    let skey = format!("{}|{}|{}", path.display(), meta.len(), mtime);
    let mut disk: HashMap<String, String> = std::fs::read(&cache_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let h = match disk.get(&skey) {
        Some(h) => h.clone(),
        None => {
            let h = crate::ocr::sha256_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            disk.insert(skey, h.clone());
            if !cfg!(test) {
                let _ = std::fs::create_dir_all(data_dir());
                let _ = crate::util::atomic_write(&cache_file, serde_json::to_string(&disk).unwrap_or_default().as_bytes());
            }
            h
        }
    };
    HASHES.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, h.clone());
    Ok(h)
}

/// Total and free memory in megabytes, when the system says.
pub fn memory_mb() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok());
    Some((field("MemTotal:")? / 1024, field("MemAvailable:")? / 1024))
}
