//! Built-in sample comic pages (drawn from simple shapes and lettered with system fonts) and a
//! sample project with a few hand-made annotations, so the app can be tried at once.

use std::collections::BTreeSet;
use std::path::Path;

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use image::{Rgba, RgbaImage};

use crate::fonts;
use crate::geom::{self, P};
use crate::review;
use crate::schema::*;
use crate::store::Store;
use crate::util;

const INK: [u8; 3] = [20, 20, 22];

struct Pen {
    img: RgbaImage,
}

impl Pen {
    fn new(w: u32, h: u32) -> Self {
        Pen { img: RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255])) }
    }
    fn blend(&mut self, x: i32, y: i32, c: [u8; 3], a: f32) {
        if x < 0 || y < 0 || x >= self.img.width() as i32 || y >= self.img.height() as i32 || a <= 0.0 {
            return;
        }
        let p = self.img.get_pixel_mut(x as u32, y as u32);
        for k in 0..3 {
            p[k] = (p[k] as f32 * (1.0 - a) + c[k] as f32 * a).round() as u8;
        }
    }
    fn rect(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, c: [u8; 3]) {
        for y in y0 as i32..y1 as i32 {
            for x in x0 as i32..x1 as i32 {
                self.blend(x, y, c, 1.0);
            }
        }
    }
    fn frame(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, t: f64) {
        self.rect(x0, y0, x1, y0 + t, INK);
        self.rect(x0, y1 - t, x1, y1, INK);
        self.rect(x0, y0, x0 + t, y1, INK);
        self.rect(x1 - t, y0, x1, y1, INK);
    }
    fn poly(&mut self, pts: &[P], c: [u8; 3]) {
        let (lo, hi) = geom::bounds(pts);
        for y in lo[1] as i32..=hi[1] as i32 {
            for x in lo[0] as i32..=hi[0] as i32 {
                if geom::point_in_poly([x as f64 + 0.5, y as f64 + 0.5], pts) {
                    self.blend(x, y, c, 1.0);
                }
            }
        }
    }
    fn ellipse(&mut self, cx: f64, cy: f64, rx: f64, ry: f64, fill: [u8; 3], stroke: f64) {
        for y in (cy - ry - stroke) as i32..=(cy + ry + stroke) as i32 {
            for x in (cx - rx - stroke) as i32..=(cx + rx + stroke) as i32 {
                let dx = (x as f64 + 0.5 - cx) / (rx + stroke);
                let dy = (y as f64 + 0.5 - cy) / (ry + stroke);
                let di = ((x as f64 + 0.5 - cx) / rx).powi(2) + ((y as f64 + 0.5 - cy) / ry).powi(2);
                if dx * dx + dy * dy <= 1.0 {
                    self.blend(x, y, if di <= 1.0 { fill } else { INK }, 1.0);
                }
            }
        }
    }
    fn halftone(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, step: f64, r: f64) {
        let mut y = y0 + step / 2.0;
        let mut row = 0;
        while y < y1 {
            let mut x = x0 + step / 2.0 + if row % 2 == 1 { step / 2.0 } else { 0.0 };
            while x < x1 {
                for yy in (y - r) as i32..=(y + r) as i32 {
                    for xx in (x - r) as i32..=(x + r) as i32 {
                        if (xx as f64 + 0.5 - x).powi(2) + (yy as f64 + 0.5 - y).powi(2) <= r * r {
                            self.blend(xx, yy, [120, 120, 125], 1.0);
                        }
                    }
                }
                x += step;
            }
            y += step;
            row += 1;
        }
    }
    fn line(&mut self, a: P, b: P, t: f64) {
        let n = (geom::dist(a, b) * 2.0) as i32 + 1;
        for i in 0..=n {
            let k = i as f64 / n as f64;
            let (x, y) = (a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k);
            for yy in (y - t) as i32..=(y + t) as i32 {
                for xx in (x - t) as i32..=(x + t) as i32 {
                    if (xx as f64 + 0.5 - x).powi(2) + (yy as f64 + 0.5 - y).powi(2) <= t * t {
                        self.blend(xx, yy, INK, 1.0);
                    }
                }
            }
        }
    }
    /// Draw a simple face looking sideways.
    fn face(&mut self, cx: f64, cy: f64, r: f64) {
        self.ellipse(cx, cy, r, r * 1.15, [250, 236, 222], 3.0);
        self.ellipse(cx, cy - r * 0.75, r * 1.05, r * 0.55, INK, 0.0);
        self.ellipse(cx - r * 0.35, cy + r * 0.05, r * 0.09, r * 0.14, INK, 0.0);
        self.ellipse(cx + r * 0.35, cy + r * 0.05, r * 0.09, r * 0.14, INK, 0.0);
        self.line([cx - r * 0.2, cy + r * 0.55], [cx + r * 0.2, cy + r * 0.55], 1.5);
        self.rect(cx - r * 0.25, cy + r * 1.1, cx + r * 0.25, cy + r * 1.5, [250, 236, 222]);
        self.poly(&[[cx - r * 1.2, cy + r * 2.6], [cx - r * 0.9, cy + r * 1.5], [cx + r * 0.9, cy + r * 1.5], [cx + r * 1.2, cy + r * 2.6]], [70, 80, 110]);
    }
}

/// Letter text; returns the cell box of every character (min, max).
fn letter(pen: &mut Pen, font: &FontVec, px: f32, text: &str, x: f64, y: f64, vertical: bool, color: [u8; 3]) -> Vec<(P, P)> {
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let mut cells = Vec::new();
    let (mut cx, mut cy) = (x, y);
    for ch in text.chars() {
        let gid = font.glyph_id(ch);
        let adv = sf.h_advance(gid) as f64;
        let (gx, base, cell_w, cell_h) = if vertical {
            (cx + (px as f64 - adv) / 2.0, cy + sf.ascent() as f64 * 0.88, px as f64, px as f64 * 1.05)
        } else {
            (cx, cy + sf.ascent() as f64, adv, (sf.ascent() - sf.descent()) as f64)
        };
        let g = gid.with_scale_and_position(scale, point(gx as f32, base as f32));
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|dx, dy, cov| pen.blend(b.min.x as i32 + dx as i32, b.min.y as i32 + dy as i32, color, cov));
        }
        if !ch.is_whitespace() {
            cells.push(([cx, cy], [cx + cell_w, cy + cell_h]));
        }
        if vertical {
            cy += cell_h;
        } else {
            cx += adv;
        }
    }
    cells
}

/// Paste `src` onto the page so that its corners land on `quad` (TL, TR, BR, BL).
fn paste_warped(pen: &mut Pen, src: &RgbaImage, quad: &[P; 4]) {
    let (w, h) = (src.width() as f64, src.height() as f64);
    let Some(hm) = geom::homography(quad, &[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]) else { return };
    let (lo, hi) = geom::bounds(quad);
    for y in lo[1] as i32..=hi[1] as i32 {
        for x in lo[0] as i32..=hi[0] as i32 {
            let s = geom::apply_h(&hm, [x as f64 + 0.5, y as f64 + 0.5]);
            if s[0] >= 0.0 && s[1] >= 0.0 && s[0] < w && s[1] < h {
                let c = geom::sample(src, s[0], s[1]);
                pen.blend(x, y, [c[0], c[1], c[2]], c[3] as f32 / 255.0);
            }
        }
    }
}

/// Ground truth from the generator.
pub struct Truth {
    pub level: Level,
    pub key: &'static str,
    pub parent: Option<&'static str>,
    pub geometry: Geometry,
    pub text: Option<(String, &'static str, &'static str, WritingMode, Direction)>,
    pub class: Option<&'static str>,
    pub order: Option<u32>,
    pub rectify: Option<([P; 4], QuadRole)>,
}

fn bx(lo: P, hi: P) -> Geometry {
    Geometry::Box { x: lo[0], y: lo[1], w: hi[0] - lo[0], h: hi[1] - lo[1], rotation_deg: 0.0 }
}

fn union(cells: &[(P, P)], pad: f64) -> (P, P) {
    let mut pts = Vec::new();
    for c in cells {
        pts.push(c.0);
        pts.push(c.1);
    }
    let (lo, hi) = geom::bounds(&pts);
    ([lo[0] - pad, lo[1] - pad], [hi[0] + pad, hi[1] + pad])
}

fn ellipse_poly(cx: f64, cy: f64, rx: f64, ry: f64, tail: Option<P>) -> Vec<P> {
    let mut v = Vec::new();
    for i in 0..28 {
        let a = std::f64::consts::TAU * i as f64 / 28.0 - std::f64::consts::FRAC_PI_2;
        v.push([geom::r2(cx + rx * a.cos()), geom::r2(cy + ry * a.sin())]);
        if i == 10 {
            if let Some(t) = tail {
                v.push(t);
            }
        }
    }
    v
}

pub struct SamplePage {
    pub name: &'static str,
    pub image: RgbaImage,
    pub truth: Vec<Truth>,
    pub work: &'static str,
    pub language: &'static str,
    pub reading: &'static str,
}

pub fn pages() -> Vec<SamplePage> {
    let regular = fonts::load_ab(fonts::cjk_path());
    let bold = fonts::load_ab(fonts::cjk_bold_path());
    let (Some(regular), Some(bold)) = (regular, bold) else {
        return vec![blank_page()];
    };
    vec![manga_page(&regular, &bold), webtoon_page(&regular, &bold), western_page(&bold), blank_page()]
}

fn jp(t: &str, vertical: bool) -> Option<(String, &'static str, &'static str, WritingMode, Direction)> {
    Some((t.into(), "ja", "Jpan", if vertical { WritingMode::VerticalRl } else { WritingMode::HorizontalTb }, if vertical { Direction::Ttb } else { Direction::Ltr }))
}

fn manga_page(font: &FontVec, bold: &FontVec) -> SamplePage {
    let (w, h) = (900u32, 1300u32);
    let mut pen = Pen::new(w, h);
    let mut t = Vec::new();
    // panels
    let panels = [([40.0, 40.0], [860.0, 520.0]), ([470.0, 550.0], [860.0, 1260.0]), ([40.0, 550.0], [440.0, 1260.0])];
    for (i, (lo, hi)) in panels.iter().enumerate() {
        pen.frame(lo[0], lo[1], hi[0], hi[1], 4.0);
        t.push(Truth { level: Level::Panel, key: ["p1", "p2", "p3"][i], parent: None, geometry: bx(*lo, *hi), text: None, class: None, order: Some(i as u32 + 1), rectify: None });
    }
    // panel 1: a face, a vertical speech bubble and a caption
    pen.halftone(44.0, 44.0, 380.0, 516.0, 9.0, 2.0);
    pen.face(230.0, 260.0, 80.0);
    let tail = [470.0, 330.0];
    pen.poly(&[[540.0, 300.0], tail, [560.0, 340.0]], INK);
    pen.ellipse(640.0, 270.0, 120.0, 175.0, [255, 255, 255], 3.0);
    pen.poly(&[[545.0, 304.0], [478.0, 330.0], [558.0, 334.0]], [255, 255, 255]);
    let c1 = letter(&mut pen, font, 44.0, "どうして", 650.0, 160.0, true, INK);
    let c2 = letter(&mut pen, font, 44.0, "ここに？", 588.0, 160.0, true, INK);
    t.push(Truth { level: Level::Container, key: "b1", parent: Some("p1"), geometry: Geometry::Polygon { points: ellipse_poly(640.0, 270.0, 123.0, 178.0, None) }, text: None, class: Some("dialogue"), order: Some(2), rectify: None });
    let u1 = union(&c1, 3.0);
    let u2 = union(&c2, 3.0);
    t.push(Truth { level: Level::Line, key: "l1", parent: Some("b1"), geometry: bx(u1.0, u1.1), text: jp("どうして", true), class: Some("dialogue"), order: Some(1), rectify: None });
    t.push(Truth { level: Level::Line, key: "l2", parent: Some("b1"), geometry: bx(u2.0, u2.1), text: jp("ここに？", true), class: Some("dialogue"), order: Some(2), rectify: None });
    pen.rect(770.0, 60.0, 840.0, 300.0, [255, 255, 255]);
    pen.frame(770.0, 60.0, 840.0, 300.0, 2.5);
    let cap = letter(&mut pen, font, 40.0, "その夜", 785.0, 85.0, true, INK);
    t.push(Truth { level: Level::Container, key: "cap", parent: Some("p1"), geometry: bx([770.0, 60.0], [840.0, 300.0]), text: None, class: Some("narration"), order: Some(1), rectify: None });
    let uc = union(&cap, 3.0);
    t.push(Truth { level: Level::Line, key: "lcap", parent: Some("cap"), geometry: bx(uc.0, uc.1), text: jp("その夜", true), class: Some("narration"), order: Some(1), rectify: None });

    // panel 2: shop sign in perspective, and a small bubble
    pen.rect(474.0, 860.0, 856.0, 1256.0, [235, 235, 238]);
    for k in 0..6 {
        pen.line([474.0, 900.0 + k as f64 * 60.0], [856.0, 880.0 + k as f64 * 64.0], 1.0);
    }
    let mut board = RgbaImage::from_pixel(360, 120, Rgba([250, 238, 205, 255]));
    {
        let mut bp = Pen { img: board.clone() };
        bp.frame(0.0, 0.0, 360.0, 120.0, 6.0);
        let cells = letter(&mut bp, bold, 84.0, "喫茶店", 50.0, 6.0, false, [120, 40, 30]);
        board = bp.img;
        let quad = [[505.0, 600.0], [835.0, 650.0], [828.0, 790.0], [500.0, 712.0]];
        paste_warped(&mut pen, &board, &quad);
        let hm = geom::homography(&[[0.0, 0.0], [360.0, 0.0], [360.0, 120.0], [0.0, 120.0]], &quad).unwrap();
        t.push(Truth { level: Level::Container, key: "sign", parent: Some("p2"), geometry: Geometry::Quad { points: quad }, text: None, class: Some("signage"), order: Some(1), rectify: Some((quad, QuadRole::Surface)) });
        let lo = cells.first().unwrap().0;
        let hi = cells.last().unwrap().1;
        let lq = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]].map(|p| {
            let q = geom::apply_h(&hm, p);
            [geom::r2(q[0]), geom::r2(q[1])]
        });
        t.push(Truth { level: Level::Line, key: "lsign", parent: Some("sign"), geometry: Geometry::Quad { points: lq }, text: jp("喫茶店", false), class: Some("signage"), order: Some(1), rectify: Some((lq, QuadRole::TextRegion)) });
        for (i, (clo, chi)) in cells.iter().enumerate() {
            let cq = [*clo, [chi[0], clo[1]], *chi, [clo[0], chi[1]]].map(|p| {
                let q = geom::apply_h(&hm, p);
                [geom::r2(q[0]), geom::r2(q[1])]
            });
            let ch = ["喫", "茶", "店"][i];
            t.push(Truth { level: Level::Char, key: ["c1", "c2", "c3"][i], parent: Some("lsign"), geometry: Geometry::Quad { points: cq }, text: jp(ch, false), class: Some("signage"), order: Some(i as u32 + 1), rectify: None });
        }
    }
    pen.ellipse(700.0, 1010.0, 72.0, 190.0, [255, 255, 255], 3.0);
    let c3 = letter(&mut pen, font, 34.0, "いらっしゃいませ", 683.0, 868.0, true, INK);
    t.push(Truth { level: Level::Container, key: "b2", parent: Some("p2"), geometry: Geometry::Polygon { points: ellipse_poly(700.0, 1010.0, 75.0, 193.0, None) }, text: None, class: Some("dialogue"), order: Some(2), rectify: None });
    let u3 = union(&c3, 3.0);
    t.push(Truth { level: Level::Line, key: "l3", parent: Some("b2"), geometry: bx(u3.0, u3.1), text: jp("いらっしゃいませ", true), class: Some("dialogue"), order: Some(1), rectify: None });

    // panel 3: a sound effect, tilted
    pen.halftone(44.0, 554.0, 436.0, 1256.0, 7.0, 1.6);
    pen.face(240.0, 1000.0, 70.0);
    let mut sfx = RgbaImage::from_pixel(300, 150, Rgba([255, 255, 255, 0]));
    {
        let mut sp = Pen { img: sfx.clone() };
        for p in sp.img.pixels_mut() {
            *p = Rgba([255, 255, 255, 0]);
        }
        let mut tmp = Pen::new(300, 150);
        letter(&mut tmp, bold, 120.0, "ドン", 20.0, 0.0, false, INK);
        for (x, y, p) in tmp.img.enumerate_pixels() {
            let a = 255 - p[0];
            sp.img.put_pixel(x, y, Rgba([20, 20, 22, a]));
        }
        sfx = sp.img;
    }
    let ang: f64 = -14f64.to_radians();
    let sq = geom::rect_corners_unordered([230.0, 700.0], (300.0, 150.0), ang);
    paste_warped(&mut pen, &sfx, &sq);
    let tight = geom::rect_corners_unordered([230.0, 708.0], (270.0, 128.0), ang).map(|p| [geom::r2(p[0]), geom::r2(p[1])]);
    t.push(Truth { level: Level::Line, key: "sfx", parent: Some("p3"), geometry: Geometry::Quad { points: tight }, text: jp("ドン", false), class: Some("sfx"), order: Some(1), rectify: None });
    SamplePage { name: "manga_ch01_p001.png", image: pen.img, truth: t, work: "Sample manga", language: "ja", reading: "rtl" }
}

fn webtoon_page(font: &FontVec, _bold: &FontVec) -> SamplePage {
    let (w, h) = (800u32, 1300u32);
    let mut pen = Pen::new(w, h);
    for y in 0..h {
        let k = y as f64 / h as f64;
        for x in 0..w {
            let c = [(200.0 + 40.0 * k) as u8, (225.0 + 20.0 * k) as u8, 250];
            pen.blend(x as i32, y as i32, c, 1.0);
        }
    }
    let mut t = Vec::new();
    let ko = |s: &str| Some((s.to_string(), "ko", "Hang", WritingMode::HorizontalTb, Direction::Ltr));
    pen.rect(60.0, 60.0, 300.0, 130.0, [40, 40, 50]);
    let cap = letter(&mut pen, font, 40.0, "다음 날", 100.0, 70.0, false, [255, 255, 255]);
    t.push(Truth { level: Level::Container, key: "cap", parent: None, geometry: bx([60.0, 60.0], [300.0, 130.0]), text: None, class: Some("narration"), order: Some(1), rectify: None });
    let uc = union(&cap, 3.0);
    t.push(Truth { level: Level::Line, key: "lc", parent: Some("cap"), geometry: bx(uc.0, uc.1), text: ko("다음 날"), class: Some("narration"), order: Some(1), rectify: None });
    pen.face(560.0, 420.0, 90.0);
    pen.ellipse(300.0, 330.0, 200.0, 80.0, [255, 255, 255], 3.0);
    let a1 = letter(&mut pen, font, 42.0, "안녕하세요!", 175.0, 305.0, false, INK);
    t.push(Truth { level: Level::Container, key: "b1", parent: None, geometry: Geometry::Polygon { points: ellipse_poly(300.0, 330.0, 203.0, 83.0, None) }, text: None, class: Some("dialogue"), order: Some(2), rectify: None });
    let ua = union(&a1, 3.0);
    t.push(Truth { level: Level::Line, key: "l1", parent: Some("b1"), geometry: bx(ua.0, ua.1), text: ko("안녕하세요!"), class: Some("dialogue"), order: Some(1), rectify: None });
    pen.face(220.0, 900.0, 90.0);
    pen.ellipse(520.0, 800.0, 210.0, 110.0, [255, 255, 255], 3.0);
    let b1 = letter(&mut pen, font, 40.0, "오늘은", 460.0, 735.0, false, INK);
    let b2 = letter(&mut pen, font, 40.0, "비가 와요.", 420.0, 790.0, false, INK);
    t.push(Truth { level: Level::Container, key: "b2", parent: None, geometry: Geometry::Polygon { points: ellipse_poly(520.0, 800.0, 213.0, 113.0, None) }, text: None, class: Some("dialogue"), order: Some(3), rectify: None });
    let ub1 = union(&b1, 3.0);
    let ub2 = union(&b2, 3.0);
    t.push(Truth { level: Level::Line, key: "l2", parent: Some("b2"), geometry: bx(ub1.0, ub1.1), text: ko("오늘은"), class: Some("dialogue"), order: Some(1), rectify: None });
    t.push(Truth { level: Level::Line, key: "l3", parent: Some("b2"), geometry: bx(ub2.0, ub2.1), text: ko("비가 와요."), class: Some("dialogue"), order: Some(2), rectify: None });
    for k in 0..14 {
        let x = 40.0 + k as f64 * 55.0;
        pen.line([x, 1050.0 + (k % 3) as f64 * 30.0], [x - 20.0, 1110.0 + (k % 3) as f64 * 30.0], 1.2);
    }
    SamplePage { name: "webtoon_ep03_cut12.png", image: pen.img, truth: t, work: "Sample webtoon", language: "ko", reading: "ltr" }
}

fn western_page(bold: &FontVec) -> SamplePage {
    let (w, h) = (900u32, 1300u32);
    let mut pen = Pen::new(w, h);
    let mut t = Vec::new();
    let en = |s: &str| Some((s.to_string(), "en", "Latn", WritingMode::HorizontalTb, Direction::Ltr));
    let panels = [([40.0, 40.0], [860.0, 620.0]), ([40.0, 650.0], [860.0, 1260.0])];
    for (i, (lo, hi)) in panels.iter().enumerate() {
        pen.frame(lo[0], lo[1], hi[0], hi[1], 4.0);
        t.push(Truth { level: Level::Panel, key: ["p1", "p2"][i], parent: None, geometry: bx(*lo, *hi), text: None, class: None, order: Some(i as u32 + 1), rectify: None });
    }
    pen.rect(60.0, 60.0, 360.0, 120.0, [255, 236, 140]);
    pen.frame(60.0, 60.0, 360.0, 120.0, 2.5);
    let cap = letter(&mut pen, bold, 30.0, "MEANWHILE...", 80.0, 72.0, false, INK);
    t.push(Truth { level: Level::Container, key: "cap", parent: Some("p1"), geometry: bx([60.0, 60.0], [360.0, 120.0]), text: None, class: Some("narration"), order: Some(1), rectify: None });
    let uc = union(&cap, 3.0);
    t.push(Truth { level: Level::Line, key: "lc", parent: Some("cap"), geometry: bx(uc.0, uc.1), text: en("MEANWHILE..."), class: Some("narration"), order: Some(1), rectify: None });
    pen.face(250.0, 430.0, 85.0);
    pen.ellipse(600.0, 260.0, 210.0, 110.0, [255, 255, 255], 3.0);
    let a = letter(&mut pen, bold, 38.0, "WHERE ARE", 490.0, 195.0, false, INK);
    let b = letter(&mut pen, bold, 38.0, "YOU GOING?", 480.0, 250.0, false, INK);
    t.push(Truth { level: Level::Container, key: "b1", parent: Some("p1"), geometry: Geometry::Polygon { points: ellipse_poly(600.0, 260.0, 213.0, 113.0, None) }, text: None, class: Some("dialogue"), order: Some(2), rectify: None });
    let ua = union(&a, 3.0);
    let ub = union(&b, 3.0);
    t.push(Truth { level: Level::Line, key: "l1", parent: Some("b1"), geometry: bx(ua.0, ua.1), text: en("WHERE ARE"), class: Some("dialogue"), order: Some(1), rectify: None });
    t.push(Truth { level: Level::Line, key: "l2", parent: Some("b1"), geometry: bx(ub.0, ub.1), text: en("YOU GOING?"), class: Some("dialogue"), order: Some(2), rectify: None });
    // a hotel sign in perspective
    pen.rect(44.0, 654.0, 856.0, 1256.0, [210, 215, 225]);
    let mut tmp = Pen::new(320, 110);
    tmp.rect(0.0, 0.0, 320.0, 110.0, [30, 60, 120]);
    letter(&mut tmp, bold, 80.0, "HOTEL", 30.0, 2.0, false, [255, 230, 120]);
    let quad = [[480.0, 720.0], [800.0, 690.0], [805.0, 830.0], [485.0, 815.0]];
    paste_warped(&mut pen, &tmp.img, &quad);
    t.push(Truth { level: Level::Container, key: "sign", parent: Some("p2"), geometry: Geometry::Quad { points: quad }, text: None, class: Some("signage"), order: Some(1), rectify: Some((quad, QuadRole::Surface)) });
    let mut sfx = Pen::new(320, 130);
    letter(&mut sfx, bold, 110.0, "BOOM!", 10.0, 0.0, false, [200, 30, 30]);
    let mut img = sfx.img;
    for p in img.pixels_mut() {
        let a = 255 - p[1];
        *p = Rgba([200, 30, 30, a]);
    }
    let sq = geom::rect_corners_unordered([250.0, 1050.0], (320.0, 130.0), 10f64.to_radians());
    paste_warped(&mut pen, &img, &sq);
    let tight = geom::rect_corners_unordered([245.0, 1055.0], (300.0, 100.0), 10f64.to_radians()).map(|p| [geom::r2(p[0]), geom::r2(p[1])]);
    t.push(Truth { level: Level::Line, key: "sfx", parent: Some("p2"), geometry: Geometry::Quad { points: tight }, text: en("BOOM!"), class: Some("sfx"), order: Some(2), rectify: None });
    SamplePage { name: "western_issue02_p014.png", image: pen.img, truth: t, work: "Sample western comic", language: "en", reading: "ltr" }
}

fn blank_page() -> SamplePage {
    let mut pen = Pen::new(900, 1300);
    pen.frame(40.0, 40.0, 860.0, 1260.0, 4.0);
    pen.halftone(44.0, 700.0, 856.0, 1256.0, 10.0, 2.5);
    for k in 0..9 {
        pen.line([100.0 + k as f64 * 80.0, 680.0], [450.0, 300.0], 1.5);
    }
    pen.ellipse(450.0, 300.0, 60.0, 60.0, [255, 255, 255], 3.0);
    SamplePage { name: "manga_ch01_p002_splash.png", image: pen.img, truth: Vec::new(), work: "Sample manga", language: "ja", reading: "rtl" }
}

/// Create the sample project in `dir` (which must not contain a project yet).
pub fn create_project(dir: &Path, user: &str) -> Result<Store, String> {
    let mut cfg = ProjectConfig::default();
    // characters required on signs only, as an example of a per-class rule
    cfg.level_overrides.push(LevelOverride { class: "signage".into(), level: Level::Char, requirement: Requirement::Required });
    let mut store = Store::create(dir, "Comic OCR sample", cfg, user)?;
    let tmp = dir.join(".labeldesk").join("sample-src");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let pages = pages();
    let mut files = Vec::new();
    for p in &pages {
        let f = tmp.join(p.name);
        p.image.save(&f).map_err(|e| e.to_string())?;
        files.push(f);
    }
    let (ids, _) = store.import_images(&files, true, user);
    let _ = std::fs::remove_dir_all(&tmp);
    let now = util::now();
    for (pi, (p, id)) in pages.iter().zip(&ids).enumerate() {
        if let Some(e) = store.manifest.pages.iter_mut().find(|e| &e.id == id) {
            e.source.work = p.work.into();
            e.source.language = p.language.into();
            e.source.reading_direction = p.reading.into();
            e.source.license = "CC0 (generated sample)".into();
            e.source.acquisition = "generated by LabelDesk".into();
            e.source.original_path = String::new();
        }
        let page_index = store.index_of(id).unwrap();
        let mut keys: Vec<(&str, String)> = Vec::new();
        // first page: the speech bubble and caption are annotated and reviewed, the sign is a draft
        // with flattening set up, the sound effect is left for the model. Other pages start empty
        // except a reviewed caption on the western page.
        let keep: Option<&[&str]> = match pi {
            0 => Some(&["p1", "p2", "p3", "b1", "l1", "l2", "cap", "lcap", "sign", "lsign", "c1", "c2", "c3", "b2", "l3"]),
            2 => Some(&["p1", "p2", "cap", "lc"]),
            _ => None,
        };
        let Some(keep) = keep else { continue };
        let author = "sample.annotator";
        for tr in p.truth.iter().filter(|t| keep.contains(&t.key)) {
            let aid = util::new_id("a");
            let mut a = Annotation::new(aid.clone(), tr.level, id, tr.geometry.clone(), author, &now);
            a.parent = tr.parent.and_then(|k| keys.iter().find(|(kk, _)| *kk == k).map(|(_, v)| v.clone()));
            a.reading_order = tr.order;
            if let Some(c) = tr.class {
                a.classes = BTreeSet::from([c.to_string()]);
            }
            if let Some((text, lang, script, wm, dir)) = &tr.text {
                a.transcription = Some(Transcription {
                    text: text.clone(),
                    language: Some(lang.to_string()),
                    script: Some(script.to_string()),
                    writing_mode: *wm,
                    direction: *dir,
                    ..Default::default()
                });
                a.recognizability = Some(Recognizability::Readable);
            }
            if let Some((q, role)) = tr.rectify {
                let (w, h) = geom::proposed_size(&q);
                let mut r = Rectification { role, source: q, target_size: [w, h], target_size_manual: false, homography: None };
                r.update_cache();
                a.rectification = Some(r);
            }
            a.provenance.origin = Origin::Sample;
            for g in ["geometry", "transcription", "reading_order", "metadata"] {
                a.provenance.fields.insert(g.into(), FieldState::Manual);
            }
            if tr.key == "sign" || tr.key == "lsign" {
                a.quality.insert("artistic_deformation".into());
            }
            if tr.key == "l3" {
                // a fresh draft: typed in, nothing else checked yet (shows the review checklist)
                a.recognizability = None;
                a.classes.clear();
            }
            keys.push((tr.key, aid));
            store.pages[page_index].annotations.push(a);
        }
        let policy = store.manifest.config.review.clone();
        let reviewed: &[&str] = if pi == 0 { &["b1", "l1", "l2", "cap", "lcap", "p1", "p2", "p3"] } else { &["cap", "lc"] };
        for (k, aid) in &keys {
            if let Some(a) = store.pages[page_index].get_mut(aid) {
                if reviewed.contains(k) {
                    let _ = review::approve(a, &Dimension::ALL, &policy, "sample.reviewer", &now);
                } else if *k == "sign" || *k == "lsign" {
                    review::submit(a, &policy);
                }
            }
        }
        store.dirty.insert(id.clone());
    }
    store.save_all(user)?;
    Ok(store)
}
