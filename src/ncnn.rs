//! A small binding to ncnn's C interface, loaded at run time from the official shared library.
//!
//! ncnn runs a converted model (`.ncnn.param` + `.ncnn.bin`, made by pnnx) on any graphics card
//! with a Vulkan driver, or on the processor. Nothing is linked when the app is built, so the app
//! starts the same whether ncnn is installed or not.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::Path;

type H = *mut c_void;

#[allow(non_snake_case)]
struct Fns {
    version: unsafe extern "C" fn() -> *const c_char,
    option_create: unsafe extern "C" fn() -> H,
    option_destroy: unsafe extern "C" fn(H),
    option_set_num_threads: unsafe extern "C" fn(H, c_int),
    option_set_use_vulkan_compute: unsafe extern "C" fn(H, c_int),
    net_create: unsafe extern "C" fn() -> H,
    net_destroy: unsafe extern "C" fn(H),
    net_set_option: unsafe extern "C" fn(H, H),
    net_set_vulkan_device: unsafe extern "C" fn(H, c_int),
    net_load_param: unsafe extern "C" fn(H, *const c_char) -> c_int,
    net_load_model: unsafe extern "C" fn(H, *const c_char) -> c_int,
    net_get_input_count: unsafe extern "C" fn(H) -> c_int,
    net_get_output_count: unsafe extern "C" fn(H) -> c_int,
    net_get_input_name: unsafe extern "C" fn(H, c_int) -> *const c_char,
    net_get_output_name: unsafe extern "C" fn(H, c_int) -> *const c_char,
    extractor_create: unsafe extern "C" fn(H) -> H,
    extractor_destroy: unsafe extern "C" fn(H),
    extractor_input: unsafe extern "C" fn(H, *const c_char, H) -> c_int,
    extractor_extract: unsafe extern "C" fn(H, *const c_char, *mut H) -> c_int,
    mat_create_3d: unsafe extern "C" fn(c_int, c_int, c_int, H) -> H,
    mat_destroy: unsafe extern "C" fn(H),
    mat_get_dims: unsafe extern "C" fn(H) -> c_int,
    mat_get_w: unsafe extern "C" fn(H) -> c_int,
    mat_get_h: unsafe extern "C" fn(H) -> c_int,
    mat_get_d: unsafe extern "C" fn(H) -> c_int,
    mat_get_c: unsafe extern "C" fn(H) -> c_int,
    mat_get_elemsize: unsafe extern "C" fn(H) -> usize,
    mat_get_cstep: unsafe extern "C" fn(H) -> usize,
    mat_get_data: unsafe extern "C" fn(H) -> *mut c_void,
    mat_get_channel_data: unsafe extern "C" fn(H, c_int) -> *mut c_void,
    // C++ functions found by their exported names (Itanium or MSVC); optional
    gpu_count: Option<unsafe extern "C" fn() -> c_int>,
    gpu_info: Option<unsafe extern "C" fn(c_int) -> *const c_void>,
    gpu_name: Option<unsafe extern "C" fn(*const c_void) -> *const c_char>,
}

pub struct Ncnn {
    f: Fns,
    _lib: libloading::Library,
}

unsafe impl Send for Ncnn {}
unsafe impl Sync for Ncnn {}

impl Ncnn {
    pub fn open(path: &Path) -> Result<Ncnn, String> {
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| format!("ncnn could not be loaded: {e}"))?;
        macro_rules! sym {
            ($name:literal) => {
                *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.map_err(|e| format!("this ncnn has no {}: {e}", $name))?
            };
        }
        macro_rules! opt {
            ($($name:literal),+) => {{
                let mut f = None;
                $( if f.is_none() { f = unsafe { lib.get(concat!($name, "\0").as_bytes()) }.ok().map(|s| *s); } )+
                f
            }};
        }
        let f = Fns {
            version: sym!("ncnn_version"),
            option_create: sym!("ncnn_option_create"),
            option_destroy: sym!("ncnn_option_destroy"),
            option_set_num_threads: sym!("ncnn_option_set_num_threads"),
            option_set_use_vulkan_compute: sym!("ncnn_option_set_use_vulkan_compute"),
            net_create: sym!("ncnn_net_create"),
            net_destroy: sym!("ncnn_net_destroy"),
            net_set_option: sym!("ncnn_net_set_option"),
            net_set_vulkan_device: sym!("ncnn_net_set_vulkan_device"),
            net_load_param: sym!("ncnn_net_load_param"),
            net_load_model: sym!("ncnn_net_load_model"),
            net_get_input_count: sym!("ncnn_net_get_input_count"),
            net_get_output_count: sym!("ncnn_net_get_output_count"),
            net_get_input_name: sym!("ncnn_net_get_input_name"),
            net_get_output_name: sym!("ncnn_net_get_output_name"),
            extractor_create: sym!("ncnn_extractor_create"),
            extractor_destroy: sym!("ncnn_extractor_destroy"),
            extractor_input: sym!("ncnn_extractor_input"),
            extractor_extract: sym!("ncnn_extractor_extract"),
            mat_create_3d: sym!("ncnn_mat_create_3d"),
            mat_destroy: sym!("ncnn_mat_destroy"),
            mat_get_dims: sym!("ncnn_mat_get_dims"),
            mat_get_w: sym!("ncnn_mat_get_w"),
            mat_get_h: sym!("ncnn_mat_get_h"),
            mat_get_d: sym!("ncnn_mat_get_d"),
            mat_get_c: sym!("ncnn_mat_get_c"),
            mat_get_elemsize: sym!("ncnn_mat_get_elemsize"),
            mat_get_cstep: sym!("ncnn_mat_get_cstep"),
            mat_get_data: sym!("ncnn_mat_get_data"),
            mat_get_channel_data: sym!("ncnn_mat_get_channel_data"),
            gpu_count: opt!("_ZN4ncnn13get_gpu_countEv", "?get_gpu_count@ncnn@@YAHXZ"),
            gpu_info: opt!("_ZN4ncnn12get_gpu_infoEi", "?get_gpu_info@ncnn@@YAAEBVGpuInfo@1@H@Z"),
            gpu_name: opt!("_ZNK4ncnn7GpuInfo11device_nameEv", "?device_name@GpuInfo@ncnn@@QEBAPEBDXZ"),
        };
        Ok(Ncnn { f, _lib: lib })
    }

    pub fn version(&self) -> String {
        unsafe { CStr::from_ptr((self.f.version)()) }.to_string_lossy().to_string()
    }

    /// Names of the Vulkan graphics cards ncnn sees (empty: none, or no Vulkan driver).
    pub fn gpus(&self) -> Vec<String> {
        let Some(count) = self.f.gpu_count else { return Vec::new() };
        let n = unsafe { count() }.max(0);
        (0..n)
            .map(|i| match (self.f.gpu_info, self.f.gpu_name) {
                (Some(info), Some(name)) => {
                    let p = unsafe { info(i) };
                    let s = if p.is_null() { std::ptr::null() } else { unsafe { name(p) } };
                    if s.is_null() { format!("graphics card {i}") } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().to_string() }
                }
                _ => format!("graphics card {i}"),
            })
            .collect()
    }

    /// Load a converted model. `gpu`: the graphics card to use, or `None` for the processor.
    pub fn load(&self, param: &Path, bin: &Path, gpu: Option<usize>, threads: usize) -> Result<Net<'_>, String> {
        let p = CString::new(param.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let b = CString::new(bin.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        unsafe {
            let opt = (self.f.option_create)();
            (self.f.option_set_num_threads)(opt, threads.max(1) as c_int);
            (self.f.option_set_use_vulkan_compute)(opt, gpu.is_some() as c_int);
            let net = (self.f.net_create)();
            (self.f.net_set_option)(net, opt);
            (self.f.option_destroy)(opt);
            if let Some(g) = gpu {
                (self.f.net_set_vulkan_device)(net, g as c_int);
            }
            if (self.f.net_load_param)(net, p.as_ptr()) != 0 {
                (self.f.net_destroy)(net);
                return Err(format!("ncnn could not read {}", param.display()));
            }
            if (self.f.net_load_model)(net, b.as_ptr()) != 0 {
                (self.f.net_destroy)(net);
                return Err(format!("ncnn could not read {}", bin.display()));
            }
            let names = |count: unsafe extern "C" fn(H) -> c_int, get: unsafe extern "C" fn(H, c_int) -> *const c_char| -> Vec<CString> {
                (0..count(net)).map(|i| CStr::from_ptr(get(net, i)).to_owned()).collect()
            };
            let inputs = names(self.f.net_get_input_count, self.f.net_get_input_name);
            let outputs = names(self.f.net_get_output_count, self.f.net_get_output_name);
            Ok(Net { n: self, net, inputs, outputs, gpu })
        }
    }
}

pub struct Net<'a> {
    n: &'a Ncnn,
    net: H,
    inputs: Vec<CString>,
    outputs: Vec<CString>,
    pub gpu: Option<usize>,
}

unsafe impl Send for Net<'_> {}

impl Drop for Net<'_> {
    fn drop(&mut self) {
        unsafe { (self.n.f.net_destroy)(self.net) }
    }
}

/// A tensor read back from ncnn: shape without the batch dimension (`[c, h, w]`, `[h, w]`, ...).
pub struct Blob {
    pub shape: Vec<usize>,
    pub data: Vec<f32>,
}

impl Net<'_> {
    pub fn output_names(&self) -> Vec<String> {
        self.outputs.iter().map(|s| s.to_string_lossy().to_string()).collect()
    }

    /// Run with one `[c, h, w]` input (planar, row-major) and read every output in order.
    pub fn run(&mut self, c: usize, h: usize, w: usize, data: &[f32]) -> Result<Vec<Blob>, String> {
        if data.len() != c * h * w {
            return Err("input size does not match its shape".into());
        }
        let f = &self.n.f;
        let input = self.inputs.first().ok_or("the model has no input")?;
        unsafe {
            let m = (f.mat_create_3d)(w as c_int, h as c_int, c as c_int, std::ptr::null_mut());
            if m.is_null() {
                return Err("out of memory".into());
            }
            for k in 0..c {
                let dst = (f.mat_get_channel_data)(m, k as c_int) as *mut f32;
                std::ptr::copy_nonoverlapping(data.as_ptr().add(k * h * w), dst, h * w);
            }
            let ex = (f.extractor_create)(self.net);
            let r = (f.extractor_input)(ex, input.as_ptr(), m);
            if r != 0 {
                (f.extractor_destroy)(ex);
                (f.mat_destroy)(m);
                return Err("ncnn refused the input".into());
            }
            let mut out = Vec::new();
            for name in &self.outputs {
                let mut o: H = std::ptr::null_mut();
                let r = (f.extractor_extract)(ex, name.as_ptr(), &mut o);
                if r != 0 || o.is_null() {
                    (f.extractor_destroy)(ex);
                    (f.mat_destroy)(m);
                    return Err(format!("ncnn could not compute {}", name.to_string_lossy()));
                }
                out.push(read_mat(f, o));
                (f.mat_destroy)(o);
            }
            (f.extractor_destroy)(ex);
            (f.mat_destroy)(m);
            Ok(out)
        }
    }
}

unsafe fn read_mat(f: &Fns, m: H) -> Blob {
    unsafe {
        let dims = (f.mat_get_dims)(m);
        let (w, h, d, c) = ((f.mat_get_w)(m) as usize, (f.mat_get_h)(m) as usize, (f.mat_get_d)(m) as usize, (f.mat_get_c)(m) as usize);
        let es = (f.mat_get_elemsize)(m);
        if es != 4 {
            return Blob { shape: vec![0], data: Vec::new() };
        }
        match dims {
            1 | 2 => {
                let n = if dims == 1 { w } else { w * h };
                let p = (f.mat_get_data)(m) as *const f32;
                Blob { shape: if dims == 1 { vec![w] } else { vec![h, w] }, data: std::slice::from_raw_parts(p, n).to_vec() }
            }
            _ => {
                let per = w * h * d.max(1);
                let _ = (f.mat_get_cstep)(m);
                let mut data = Vec::with_capacity(per * c);
                for k in 0..c {
                    let p = (f.mat_get_channel_data)(m, k as c_int) as *const f32;
                    data.extend_from_slice(std::slice::from_raw_parts(p, per));
                }
                Blob { shape: if dims == 4 { vec![c, d, h, w] } else { vec![c, h, w] }, data }
            }
        }
    }
}
