#![allow(dead_code)]
#[cfg(test)]
mod acceptance;
mod ai;
mod convert;
mod engine;
mod ncnn;
mod runtime;
mod zoo;
mod assist;
mod ctc_lm;
#[cfg(test)]
mod assist_tests;
#[cfg(test)]
mod zoo_tests;
mod app;
mod docs;
mod export;
mod fonts;
mod geom;
mod git;
mod models;
mod icons;
mod ocr;
mod review;
mod sample;
mod schema;
mod store;
mod util;
mod validate;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let mut screenshot = None;
    let mut open = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--probe-runtime" => {
                // run in a child process by `runtime::probe`: load a library and report
                let c = args.next().unwrap_or_default();
                let p = PathBuf::from(args.next().unwrap_or_default());
                std::process::exit(runtime::probe_main(&c, &p));
            }
            "--sample" => {
                // write the sample project (a drawn comic page) into a new folder
                let dir = PathBuf::from(args.next().unwrap_or_else(|| "Comic OCR sample".into()));
                match sample::create_project(&dir, "sample") {
                    Ok(_) => {
                        println!("sample project written to {}", dir.display());
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            "--install" => {
                let c = match args.next().as_deref() {
                    Some("onnxruntime") => runtime::Component::OnnxRuntime,
                    Some("ncnn") => runtime::Component::Ncnn,
                    Some("pnnx") => runtime::Component::Pnnx,
                    _ => {
                        eprintln!("--install onnxruntime|ncnn|pnnx");
                        std::process::exit(2);
                    }
                };
                let p = std::sync::Mutex::new(runtime::Progress::default());
                match runtime::install(c, &p).and_then(|path| runtime::probe(c, &path).map(|pr| (path, pr))) {
                    Ok((path, pr)) => {
                        println!("{} ready: {} (version {}{})", c.name(), path.display(), pr.version, if pr.gpus.is_empty() { String::new() } else { format!(", graphics cards: {}", pr.gpus.join(", ")) });
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        std::process::exit(1);
                    }
                }
            }
            "--convert" => {
                let id = match args.next().as_deref() {
                    Some("animetext") => zoo::ZooId::AnimeText,
                    Some("layout") => zoo::ZooId::KoharuLayout,
                    Some("textmask") => zoo::ZooId::KoharuTextMask,
                    _ => {
                        eprintln!("--convert animetext|layout|textmask [onnx|ncnn]");
                        std::process::exit(2);
                    }
                };
                let target = if args.next().as_deref() == Some("ncnn") { convert::Target::Ncnn } else { convert::Target::Onnx };
                let t = convert::Task::start(id, target, app::Settings::load().models);
                let mut shown = 0;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let s = t.state.lock().unwrap().clone();
                    for l in s.log.iter().skip(shown) {
                        println!("{l}");
                    }
                    shown = s.log.len();
                    if !s.running {
                        std::process::exit(if s.error.is_some() { 1 } else { 0 });
                    }
                }
            }
            "--try-model" => {
                let model = args.next().unwrap_or_default();
                let image = PathBuf::from(args.next().unwrap_or_default());
                let backend = args.next();
                std::process::exit(try_model(&model, &image, backend.as_deref()));
            }
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!(
                    "LabelDesk: comic OCR annotation.\n\nUsage: labeldesk [project-folder] [--screenshot out.png]\n       \
                     labeldesk --try-model <animetext|charspot|layout|textmask> <picture> [onnx|ncnn-gpu|ncnn-cpu]\n\n\
                     A project folder holds project.json and one annotations/<page>.json per picture (see FORMAT.md inside)."
                );
                return Ok(());
            }
            other => open = Some(PathBuf::from(other)),
        }
    }
    if screenshot.is_some() && std::env::var_os("DISPLAY").is_some() {
        // A screenshot run must not depend on the screen being on: Wayland only lets a window
        // draw while the compositor shows it, so use X11 (XWayland) without waiting for vsync.
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("LabelDesk: comic OCR")
            .with_inner_size([1500.0, 940.0])
            .with_min_inner_size([1000.0, 620.0]),
        ..Default::default()
    };
    if screenshot.is_some() {
        options.glow_options.vsync = false;
    }
    eframe::run_native("LabelDesk", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, screenshot, open)))))
}

/// `--try-model`: run one model on a picture and print what it found (for checking a setup).
fn try_model(model: &str, image: &std::path::Path, backend: Option<&str>) -> i32 {
    let mut o = app::Settings::load().models;
    let id = match model {
        "animetext" => zoo::ZooId::AnimeText,
        "charspot" => zoo::ZooId::Charspot,
        "layout" => zoo::ZooId::KoharuLayout,
        "textmask" => zoo::ZooId::KoharuTextMask,
        _ => {
            eprintln!("unknown model {model}");
            return 2;
        }
    };
    if let Some(b) = backend {
        let b = match b {
            "ncnn-gpu" => engine::Backend::NcnnGpu,
            "ncnn-cpu" => engine::Backend::NcnnCpu,
            _ => engine::Backend::Onnx,
        };
        o.backends.insert(id, b);
    }
    let img = match util::open_image(image) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let t = std::time::Instant::now();
    let r: Result<(String, Vec<String>), String> = (|| match id {
        zoo::ZooId::AnimeText => {
            let mut m = zoo::AnimeText::load(&o)?;
            let lt = t.elapsed();
            let f = m.detect(&img)?;
            Ok((format!("{} · loaded in {:.1}s", m.model.runtime, lt.as_secs_f64()), f.iter().map(|f| format!("{} {:.3} {:?}", f.label, f.score, f.bbox)).collect()))
        }
        zoo::ZooId::Charspot => {
            let mut m = zoo::Charspot::load(&o)?;
            let c = m.read(&img)?;
            let (v, lines) = zoo::group_lines(&c);
            let mut out = vec![format!("text: {}", c.iter().map(|c| c.text.as_str()).collect::<String>()), format!("{} line(s), vertical {v}", lines.len())];
            out.extend(c.iter().map(|c| format!("{} {:.3} {:?}", c.text, c.score, c.bbox)));
            Ok((m.model.runtime.clone(), out))
        }
        zoo::ZooId::KoharuLayout => {
            let mut m = zoo::KoharuLayout::load(&o)?;
            let f = m.detect(&img)?;
            Ok((m.model.runtime.clone(), f.iter().map(|f| format!("{} {:.3} {:?}{}", f.label, f.score, f.bbox, if f.polygon.is_some() { " (outline)" } else { "" })).collect()))
        }
        zoo::ZooId::KoharuTextMask => {
            let mut m = zoo::KoharuTextMask::load(&o)?;
            let mk = m.mask(&img)?;
            let n = mk.pixels().filter(|p| p[0] > 0).count();
            let out = image.with_extension("textmask.png");
            let _ = mk.save(&out);
            Ok((m.model.runtime.clone(), vec![format!("{n} text pixels; mask saved to {}", out.display())]))
        }
    })();
    match r {
        Ok((rt, lines)) => {
            println!("{} on {} in {:.2}s", id.name(), rt, t.elapsed().as_secs_f64());
            for l in lines {
                println!("  {l}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
