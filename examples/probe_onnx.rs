//! Print the inputs and outputs of ONNX files:
//! `cargo run --example probe_onnx -- /path/libonnxruntime.so a.onnx b.onnx`.
fn main() {
    let mut args = std::env::args().skip(1);
    let lib = args.next().expect("library path");
    let env = ort::init_from(&lib).unwrap().build().unwrap();
    for p in args {
        let s = ort::session::Session::builder(&env).unwrap().commit_from_file(&p).unwrap();
        println!("== {p}");
        for i in s.inputs() { println!("  in  {} {:?}", i.name(), i.dtype()); }
        for o in s.outputs() { println!("  out {} {:?}", o.name(), o.dtype()); }
    }
}
