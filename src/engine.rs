//! One way to run a single-input model, whichever program runs it: ONNX Runtime on the
//! processor, or ncnn on a graphics card (Vulkan) or the processor. Model adapters in `zoo.rs`
//! are written once against [`Engine`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ort::session::Session;
use ort::value::Tensor as OrtTensor;
use serde::{Deserialize, Serialize};

/// Where a model runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// ONNX Runtime on the processor.
    #[default]
    Onnx,
    /// ncnn on the graphics card (Vulkan).
    NcnnGpu,
    /// ncnn on the processor.
    NcnnCpu,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Onnx, Backend::NcnnGpu, Backend::NcnnCpu];
    pub fn name(self) -> &'static str {
        match self {
            Backend::Onnx => "Processor (ONNX Runtime)",
            Backend::NcnnGpu => "Graphics card (ncnn, Vulkan)",
            Backend::NcnnCpu => "Processor (ncnn)",
        }
    }
    pub fn is_ncnn(self) -> bool {
        self != Backend::Onnx
    }
}

/// A tensor: shape and values (integers are read as floats).
#[derive(Clone, Debug)]
pub struct Tensor {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl Tensor {
    /// The shape without leading 1s (ncnn drops the batch dimension, ONNX keeps it).
    pub fn dims(&self) -> Vec<usize> {
        let mut s: &[usize] = &self.shape;
        while s.len() > 1 && s[0] == 1 {
            s = &s[1..];
        }
        s.to_vec()
    }
}

pub trait Engine: Send {
    /// Run with one `[1, c, h, w]` input; outputs in the model's order.
    fn run(&mut self, c: usize, h: usize, w: usize, data: Vec<f32>) -> Result<Vec<Tensor>, String>;
    /// What runs it, for the record ("ONNX Runtime 1.31.1 (processor)").
    fn label(&self) -> String;
}

pub struct OrtEngine {
    session: Session,
    label: String,
}

impl OrtEngine {
    pub fn load(model: &Path, ort_path: Option<&Path>) -> Result<Self, String> {
        let env = crate::runtime::ort_env(ort_path)?;
        let threads = std::thread::available_parallelism().map(|n| n.get().clamp(1, 8)).unwrap_or(4);
        let session = Session::builder(env)
            .map_err(|e| e.to_string())?
            .with_intra_threads(threads)
            .map_err(|e| e.to_string())?
            .commit_from_file(model)
            .map_err(|e| format!("{}: {e}", model.display()))?;
        Ok(OrtEngine { session, label: crate::runtime::ort_runtime_label() })
    }
}

impl Engine for OrtEngine {
    fn run(&mut self, c: usize, h: usize, w: usize, data: Vec<f32>) -> Result<Vec<Tensor>, String> {
        let t = OrtTensor::from_array(([1usize, c, h, w], data)).map_err(|e| e.to_string())?;
        let outputs = self.session.run(ort::inputs![t]).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        for (_, o) in outputs.iter() {
            if let Ok((shape, d)) = o.try_extract_tensor::<f32>() {
                v.push(Tensor { shape: shape.iter().map(|x| *x as usize).collect(), data: d.to_vec() });
            } else if let Ok((shape, d)) = o.try_extract_tensor::<i64>() {
                v.push(Tensor { shape: shape.iter().map(|x| *x as usize).collect(), data: d.iter().map(|x| *x as f32).collect() });
            } else if let Ok((shape, d)) = o.try_extract_tensor::<bool>() {
                v.push(Tensor { shape: shape.iter().map(|x| *x as usize).collect(), data: d.iter().map(|x| *x as u8 as f32).collect() });
            } else {
                return Err("the model returned a kind of value LabelDesk can't read".into());
            }
        }
        Ok(v)
    }
    fn label(&self) -> String {
        self.label.clone()
    }
}

static NCNN: Mutex<Option<(&'static crate::ncnn::Ncnn, PathBuf, String, Vec<String>)>> = Mutex::new(None);

/// The ncnn library, loaded once (after a probe in a separate process).
pub fn ncnn_lib(explicit: Option<&Path>) -> Result<(&'static crate::ncnn::Ncnn, String, Vec<String>), String> {
    let mut g = NCNN.lock().unwrap();
    if let Some((n, _, v, gpus)) = g.as_ref() {
        return Ok((n, v.clone(), gpus.clone()));
    }
    let path = crate::runtime::locate(crate::runtime::Component::Ncnn, explicit)
        .ok_or("ncnn is not installed. Open AI → Models… and press Install next to ncnn (about 10 MB, Tencent's official release).")?;
    let p = crate::runtime::probe(crate::runtime::Component::Ncnn, &path)?;
    let n: &'static crate::ncnn::Ncnn = Box::leak(Box::new(crate::ncnn::Ncnn::open(&path)?));
    *g = Some((n, path, p.version.clone(), p.gpus.clone()));
    Ok((n, p.version, p.gpus))
}

pub struct NcnnEngine {
    net: crate::ncnn::Net<'static>,
    label: String,
}

impl NcnnEngine {
    pub fn load(param: &Path, bin: &Path, gpu: Option<usize>, explicit: Option<&Path>) -> Result<Self, String> {
        let (lib, version, gpus) = ncnn_lib(explicit)?;
        if gpu.is_some() && gpus.is_empty() {
            return Err("ncnn found no graphics card with a Vulkan driver on this computer; choose the processor instead.".into());
        }
        let gpu = gpu.map(|g| g.min(gpus.len().saturating_sub(1)));
        let threads = std::thread::available_parallelism().map(|n| n.get().clamp(1, 8)).unwrap_or(4);
        let net = lib.load(param, bin, gpu, threads)?;
        let label = match gpu {
            Some(g) => format!("ncnn {version} (graphics card: {})", gpus.get(g).cloned().unwrap_or_default()),
            None => format!("ncnn {version} (processor)"),
        };
        Ok(NcnnEngine { net, label })
    }
}

impl Engine for NcnnEngine {
    fn run(&mut self, c: usize, h: usize, w: usize, data: Vec<f32>) -> Result<Vec<Tensor>, String> {
        let out = self.net.run(c, h, w, &data)?;
        Ok(out.into_iter().map(|b| Tensor { shape: b.shape, data: b.data }).collect())
    }
    fn label(&self) -> String {
        self.label.clone()
    }
}

/// Files of a model in the formats it can run from.
#[derive(Clone, Debug, Default)]
pub struct ModelFiles {
    pub onnx: Option<PathBuf>,
    pub ncnn: Option<(PathBuf, PathBuf)>,
}

/// Paths to the runtimes chosen in the settings (empty: find them).
#[derive(Clone, Debug, Default)]
pub struct RuntimePaths {
    pub ort: Option<PathBuf>,
    pub ncnn: Option<PathBuf>,
    pub gpu: usize,
}

pub fn load(files: &ModelFiles, backend: Backend, rt: &RuntimePaths) -> Result<Box<dyn Engine>, String> {
    match backend {
        Backend::Onnx => {
            let p = files.onnx.as_ref().ok_or("this model has no ONNX file yet; convert it in the Models window")?;
            Ok(Box::new(OrtEngine::load(p, rt.ort.as_deref())?))
        }
        Backend::NcnnGpu | Backend::NcnnCpu => {
            let (p, b) = files.ncnn.as_ref().ok_or("this model has not been converted to ncnn yet; convert it in the Models window")?;
            let gpu = (backend == Backend::NcnnGpu).then_some(rt.gpu);
            Ok(Box::new(NcnnEngine::load(p, b, gpu, rt.ncnn.as_deref())?))
        }
    }
}
