#![allow(dead_code)]
#[cfg(test)]
mod acceptance;
mod ai;
mod assist;
mod ctc_lm;
#[cfg(test)]
mod assist_tests;
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
            "--screenshot" => screenshot = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!(
                    "LabelDesk: comic OCR annotation.\n\nUsage: labeldesk [project-folder] [--screenshot out.png]\n\n\
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
