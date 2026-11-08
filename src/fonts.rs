//! System fonts: used to show any script in the interface (egui's own fonts are Latin only)
//! and to letter the built-in sample comic pages.

use std::path::PathBuf;

const CJK: &[&str] = &[
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Medium.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\msgothic.ttc",
];

const BOLD_CJK: &[&str] = &[
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Black.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "C:\\Windows\\Fonts\\YuGothB.ttc",
];

const OTHER: &[(&str, &[&str])] = &[
    ("arabic", &["/usr/share/fonts/noto/NotoNaskhArabic-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoNaskhArabic-Regular.ttf", "C:\\Windows\\Fonts\\arial.ttf"]),
    ("hebrew", &["/usr/share/fonts/noto/NotoSansHebrew-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf"]),
    ("thai", &["/usr/share/fonts/noto/NotoSansThai-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf"]),
    ("devanagari", &["/usr/share/fonts/noto/NotoSansDevanagari-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf"]),
    ("symbols", &["/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf", "/usr/share/fonts/TTF/DejaVuSans.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]),
];

fn first_existing(list: &[&str]) -> Option<PathBuf> {
    list.iter().map(PathBuf::from).find(|p| p.exists())
}

fn fc_match(pattern: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    p.exists().then_some(p)
}

pub fn cjk_path() -> Option<PathBuf> {
    first_existing(CJK).or_else(|| fc_match(":lang=ja"))
}

pub fn cjk_bold_path() -> Option<PathBuf> {
    first_existing(BOLD_CJK).or_else(cjk_path)
}

/// Add system fonts as fallbacks so transcriptions in any installed script are visible.
pub fn install(ctx: &eframe::egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, path: PathBuf| {
        if let Ok(bytes) = std::fs::read(&path) {
            defs.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families.entry(fam).or_default().push(name.to_string());
            }
        }
    };
    if let Some(p) = cjk_path() {
        add("cjk", p);
    }
    for (name, list) in OTHER {
        if let Some(p) = first_existing(list) {
            add(name, p);
        }
    }
    ctx.set_fonts(defs);
}

pub fn load_ab(path: Option<PathBuf>) -> Option<ab_glyph::FontVec> {
    let bytes = std::fs::read(path?).ok()?;
    ab_glyph::FontVec::try_from_vec_and_index(bytes, 0).ok()
}
