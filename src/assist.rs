//! Assisted annotation: character box proposals, reading-order suggestions, the quick-review
//! checklist (what still needs a person), review queues and progress metrics.
//!
//! Everything here only *proposes*. Proposals carry their provenance and stay unverified until a
//! person accepts them.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::geom::{self, P};
use crate::review;
use crate::schema::*;

pub const CHAR_ALGO: &str = "labeldesk.char_split";
pub const CHAR_ALGO_VERSION: &str = "1";

// ------------------------------------------------------------------ scripts and widths

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharKind {
    Han,
    Kana,
    Hangul,
    Latin,
    Arabic,
    Hebrew,
    Cyrillic,
    Thai,
    Digit,
    Punct,
    Space,
    Wide,
    Other,
}

pub fn char_kind(c: char) -> CharKind {
    let u = c as u32;
    match u {
        _ if c.is_whitespace() => CharKind::Space,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => CharKind::Kana,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2FFFF | 0xF900..=0xFAFF | 0x3005 => CharKind::Han,
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => CharKind::Hangul,
        0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => CharKind::Arabic,
        0x0590..=0x05FF => CharKind::Hebrew,
        0x0400..=0x04FF => CharKind::Cyrillic,
        0x0E00..=0x0E7F => CharKind::Thai,
        0x3000..=0x303F | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65 | 0x2026 | 0x2025 => CharKind::Punct,
        0xFF10..=0xFF19 | 0xFF21..=0xFF3A | 0xFF41..=0xFF5A | 0x1F000..=0x1FAFF | 0x2600..=0x27BF => CharKind::Wide,
        _ if c.is_ascii_digit() => CharKind::Digit,
        _ if c.is_ascii_punctuation() || matches!(c, '¡' | '¿' | '«' | '»' | '“' | '”' | '‘' | '’' | '—' | '–') => CharKind::Punct,
        _ if c.is_alphabetic() && u < 0x0250 => CharKind::Latin,
        _ => CharKind::Other,
    }
}

fn is_punct(g: &str) -> bool {
    g.chars().next().is_some_and(|c| char_kind(c) == CharKind::Punct)
}

/// Language and script guessed from the text itself (None when the text does not say).
pub fn detect_script(text: &str) -> (Option<&'static str>, Option<&'static str>) {
    let mut n: BTreeMap<u8, usize> = BTreeMap::new();
    for c in text.chars() {
        let k = match char_kind(c) {
            CharKind::Kana => 1,
            CharKind::Han => 2,
            CharKind::Hangul => 3,
            CharKind::Latin => 4,
            CharKind::Arabic => 5,
            CharKind::Hebrew => 6,
            CharKind::Cyrillic => 7,
            CharKind::Thai => 8,
            _ => continue,
        };
        *n.entry(k).or_default() += 1;
    }
    let has = |k: u8| n.get(&k).copied().unwrap_or(0) > 0;
    if has(1) {
        ("ja".into(), "Jpan".into())
    } else if has(3) {
        (Some("ko"), Some(if has(2) { "Kore" } else { "Hang" }))
    } else if has(2) {
        (None, Some("Hani")) // Han only: Japanese and Chinese both possible
    } else if has(5) {
        (None, Some("Arab"))
    } else if has(6) {
        (Some("he"), Some("Hebr"))
    } else if has(7) {
        (None, Some("Cyrl"))
    } else if has(8) {
        (Some("th"), Some("Thai"))
    } else if has(4) {
        (None, Some("Latn"))
    } else {
        (None, None)
    }
}

/// Does a declared script agree with what the text shows?
pub fn script_compatible(declared: &str, detected: &str) -> bool {
    declared == detected
        || matches!((declared, detected), ("Jpan", "Hani") | ("Jpan", "Latn") | ("Hani", "Jpan") | ("Hans", "Hani") | ("Hant", "Hani") | ("Kore", "Hang") | ("Kore", "Hani") | ("Hang", "Kore"))
}

/// Relative advance of one grapheme cluster along the writing direction (full-width = 1).
pub fn advance(g: &str, vertical: bool, proportional: bool) -> f64 {
    let Some(c) = g.chars().next() else { return 0.0 };
    let k = char_kind(c);
    if vertical {
        return if k == CharKind::Space { 0.5 } else { 1.0 };
    }
    match k {
        CharKind::Han | CharKind::Kana | CharKind::Hangul | CharKind::Wide => {
            if (0xFF66..=0xFF9F).contains(&(c as u32)) { 0.5 } else { 1.0 }
        }
        CharKind::Punct if (c as u32) >= 0x3000 => 1.0,
        _ if !proportional => 0.6,
        CharKind::Space => 0.28,
        CharKind::Punct => match c {
            '.' | ',' | ':' | ';' | '!' | '\'' | '|' | '’' | '‘' => 0.28,
            '?' => 0.55,
            '—' => 1.0,
            '–' | '-' => 0.4,
            _ => 0.4,
        },
        CharKind::Digit => 0.56,
        CharKind::Latin => match c {
            'i' | 'j' | 'l' | 'I' => 0.26,
            'f' | 't' | 'r' => 0.34,
            'm' | 'M' => 0.86,
            'w' | 'W' => 0.8,
            'J' => 0.5,
            _ if c.is_uppercase() => 0.68,
            _ => 0.56,
        },
        _ => 0.6,
    }
}

// ------------------------------------------------------------------ character proposals

#[derive(Clone, Debug, PartialEq)]
pub struct CharProposal {
    pub text: String,
    pub span: [usize; 2],
    pub quad: [P; 4],
    /// Position in the source's frame: `[u0, v0, u1, v1]` fractions.
    pub cell: [f64; 4],
}

/// Cell borders chosen by a person, per row of the text: the fractions (0..1, in reading
/// progression) where each character ends and the next begins. `len = characters in row - 1`.
pub type Splits = Vec<Vec<f64>>;

pub enum Frame {
    /// Text frame quad (TL, TR, BR, BL): positions by bilinear interpolation.
    Quad([P; 4]),
    /// Rectified rectangle `rect` in a flattened space; `inv` maps flattened → image.
    Rect { rect: (P, P), inv: [f64; 9] },
}

impl Frame {
    pub fn at(&self, u: f64, v: f64) -> P {
        match self {
            Frame::Quad(q) => {
                let top = [q[0][0] + (q[1][0] - q[0][0]) * u, q[0][1] + (q[1][1] - q[0][1]) * u];
                let bot = [q[3][0] + (q[2][0] - q[3][0]) * u, q[3][1] + (q[2][1] - q[3][1]) * u];
                [top[0] + (bot[0] - top[0]) * v, top[1] + (bot[1] - top[1]) * v]
            }
            Frame::Rect { rect, inv } => {
                let p = [rect.0[0] + (rect.1[0] - rect.0[0]) * u, rect.0[1] + (rect.1[1] - rect.0[1]) * v];
                geom::apply_h(inv, p)
            }
        }
    }
}

fn own_corners(a: &Annotation) -> [P; 4] {
    a.geometry.corners().unwrap_or_else(|| {
        let (c, s, ang) = geom::min_area_rect(&a.geometry.outline());
        geom::order_quad(geom::rect_corners(c, s, ang))
    })
}

/// The homography to flatten this annotation: its own rectification or a flat surface it sits on.
fn flattening<'a>(a: &'a Annotation, page: &'a PageFile) -> Option<&'a Rectification> {
    if let Some(r) = &a.rectification {
        if geom::quad_order_ok(&r.source) {
            return Some(r);
        }
    }
    let mut cur = a.parent.as_deref();
    let mut guard = 0;
    while let (Some(id), true) = (cur, guard < 16) {
        let p = page.get(id)?;
        if let Some(r) = &p.rectification {
            if r.role == QuadRole::Surface && geom::quad_order_ok(&r.source) {
                return Some(r);
            }
        }
        cur = p.parent.as_deref();
        guard += 1;
    }
    None
}

pub fn frame_for(a: &Annotation, page: &PageFile) -> (Frame, &'static str) {
    let corners = own_corners(a);
    if let Some(r) = flattening(a, page) {
        let (w, h) = (r.target_size[0] as f64, r.target_size[1] as f64);
        let dst = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
        if let (Some(hm), Some(inv)) = (geom::homography(&r.source, &dst), geom::homography(&dst, &r.source)) {
            let mapped: Vec<P> = corners.iter().map(|p| geom::apply_h(&hm, *p)).collect();
            let rect = geom::bounds(&mapped);
            if rect.1[0] - rect.0[0] > 1.0 && rect.1[1] - rect.0[1] > 1.0 {
                return (Frame::Rect { rect, inv }, "rectified");
            }
        }
    }
    let kind = if a.geometry.corners().is_some() { "quad" } else { "axis" };
    (Frame::Quad(corners), kind)
}

pub fn is_vertical(t: &Transcription) -> bool {
    matches!(t.writing_mode, WritingMode::VerticalRl | WritingMode::VerticalLr) || (t.writing_mode == WritingMode::Custom && matches!(t.direction, Direction::Ttb | Direction::Btt))
}

/// Rows (lines or columns, split at line breaks) with their global grapheme indices.
fn text_rows(text: &str) -> Vec<Vec<(usize, &str)>> {
    let mut rows: Vec<Vec<(usize, &str)>> = vec![Vec::new()];
    for (i, g) in text.graphemes(true).enumerate() {
        if g == "\n" || g == "\r\n" {
            rows.push(Vec::new());
        } else {
            rows.last_mut().unwrap().push((i, g));
        }
    }
    rows.retain(|r| !r.is_empty());
    rows
}

/// Upright runs in vertical text that share one cell side by side (tate-chu-yoko): numbers of
/// two or three half-width digits, and "!!", "!?", "?!", "??". Returns (start, length) in the row.
pub fn tcy_runs(row: &[(usize, &str)], vertical: bool) -> Vec<(usize, usize)> {
    if !vertical {
        return Vec::new();
    }
    let digit = |g: &str| g.len() == 1 && g.as_bytes()[0].is_ascii_digit();
    let mark = |g: &str| g == "!" || g == "?";
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        for (test, lens) in [(&digit as &dyn Fn(&str) -> bool, 2..=3usize), (&mark as &dyn Fn(&str) -> bool, 2..=2usize)] {
            let n = row[i..].iter().take_while(|(_, g)| test(g)).count();
            if n > 0 {
                if lens.contains(&n) {
                    out.push((i, n));
                }
                i += n - 1;
                break;
            }
        }
        i += 1;
    }
    out
}

/// The policy for one annotation: lines marked `monospace` get equal cells.
pub fn policy_for(a: &Annotation, policy: &CharPolicy) -> CharPolicy {
    CharPolicy { monospace: policy.monospace || a.attributes.contains("monospace"), ..policy.clone() }
}

fn row_advances(row: &[(usize, &str)], vertical: bool, policy: &CharPolicy) -> Vec<f64> {
    let mut adv: Vec<f64> = row.iter().map(|(_, g)| if policy.monospace { 1.0 } else { advance(g, vertical, policy.proportional_latin) }).collect();
    for (start, n) in tcy_runs(row, vertical) {
        for a in &mut adv[start..start + n] {
            *a = 1.0 / n as f64;
        }
    }
    adv
}

/// Where the cells of each row end by default (estimated character widths).
pub fn default_splits(t: &Transcription, policy: &CharPolicy) -> Splits {
    let vertical = is_vertical(t);
    text_rows(&t.text)
        .iter()
        .map(|row| {
            let adv = row_advances(row, vertical, policy);
            let total: f64 = adv.iter().sum::<f64>().max(1e-9);
            let mut s = 0.0;
            adv[..adv.len().saturating_sub(1)]
                .iter()
                .map(|w| {
                    s += w;
                    s / total
                })
                .collect()
        })
        .collect()
}

/// Do these splits still fit this text (same rows, same number of characters)?
pub fn splits_fit(t: &Transcription, splits: &Splits) -> bool {
    let rows = text_rows(&t.text);
    rows.len() == splits.len() && rows.iter().zip(splits).all(|(r, s)| s.len() + 1 == r.len())
}

fn reversed(t: &Transcription) -> bool {
    if is_vertical(t) { t.direction == Direction::Btt } else { t.direction == Direction::Rtl }
}

/// Cross-axis band of row `k` of `n` (0..1 across the writing direction).
fn row_band(t: &Transcription, k: usize, n: usize) -> (f64, f64) {
    let (k, n) = (k as f64, n.max(1) as f64);
    if is_vertical(t) && t.writing_mode != WritingMode::VerticalLr { (1.0 - (k + 1.0) / n, 1.0 - k / n) } else { (k / n, (k + 1.0) / n) }
}

/// Map a progression interval and cross band to a cell `[u0, v0, u1, v1]` in the frame.
fn to_cell(t: &Transcription, along: (f64, f64), band: (f64, f64)) -> [f64; 4] {
    let (a0, a1) = if reversed(t) { (1.0 - along.1, 1.0 - along.0) } else { along };
    if is_vertical(t) { [band.0, a0, band.1, a1] } else { [a0, band.0, a1, band.1] }
}

pub fn cell_quad(frame: &Frame, c: [f64; 4]) -> [P; 4] {
    [frame.at(c[0], c[1]), frame.at(c[2], c[1]), frame.at(c[2], c[3]), frame.at(c[0], c[3])].map(|p| [geom::r2(p[0]), geom::r2(p[1])])
}

/// Split an annotation's transcription into provisional character boxes that follow its geometry
/// (and the perspective of its flat surface when one is known). Returns the proposals and the frame used.
pub fn propose_chars(a: &Annotation, page: &PageFile, policy: &CharPolicy) -> (Vec<CharProposal>, &'static str) {
    propose_chars_split(a, page, policy, None)
}

/// Like [`propose_chars`], with cell borders chosen by a person when `splits` fits the text.
pub fn propose_chars_split(a: &Annotation, page: &PageFile, policy: &CharPolicy, splits: Option<&Splits>) -> (Vec<CharProposal>, &'static str) {
    let Some(t) = &a.transcription else { return (Vec::new(), "") };
    if t.text.trim().is_empty() {
        return (Vec::new(), "");
    }
    let policy = &policy_for(a, policy);
    let (frame, kind) = frame_for(a, page);
    let rows = text_rows(&t.text);
    let splits = match splits {
        Some(s) if splits_fit(t, s) => s.clone(),
        _ => default_splits(t, policy),
    };
    let vertical = is_vertical(t);
    let mut out = Vec::new();
    for (k, row) in rows.iter().enumerate() {
        let band = row_band(t, k, rows.len());
        let mut edges = vec![0.0];
        edges.extend(splits[k].iter().copied());
        edges.push(1.0);
        let runs = tcy_runs(row, vertical);
        for (j, (i, g)) in row.iter().enumerate() {
            let skip = (policy.skip_spaces && g.trim().is_empty()) || (policy.skip_punctuation && is_punct(g));
            if skip {
                continue;
            }
            let cell = match runs.iter().find(|(s, n)| (*s..s + n).contains(&j)) {
                // one shared cell, split side by side, read left to right
                Some(&(s, n)) => {
                    let c = to_cell(t, (edges[s], edges[s + n]), band);
                    let w = (c[2] - c[0]) / n as f64;
                    let x0 = c[0] + w * (j - s) as f64;
                    [x0, c[1], x0 + w, c[3]]
                }
                None => to_cell(t, (edges[j], edges[j + 1]), band),
            };
            out.push(CharProposal { text: g.to_string(), span: [*i, i + 1], quad: cell_quad(&frame, cell), cell });
        }
    }
    (out, kind)
}

/// A draggable border between two character cells.
#[derive(Clone, Debug, PartialEq)]
pub enum CellBorder {
    /// Between previewed cells `idx` and `idx + 1` of row `row`.
    Preview { row: usize, idx: usize },
    /// Between two existing character boxes (first, second in reading order).
    Pair { first: String, second: String },
}

/// Borders of the previewed cells as image-space segments.
pub fn preview_borders(a: &Annotation, page: &PageFile, policy: &CharPolicy, splits: Option<&Splits>) -> Vec<(CellBorder, P, P)> {
    let Some(t) = &a.transcription else { return Vec::new() };
    let policy = &policy_for(a, policy);
    let (frame, _) = frame_for(a, page);
    let rows = text_rows(&t.text);
    let splits = match splits {
        Some(s) if splits_fit(t, s) => s.clone(),
        _ => default_splits(t, policy),
    };
    let mut out = Vec::new();
    for (k, row_splits) in splits.iter().enumerate() {
        let band = row_band(t, k, rows.len());
        let runs = tcy_runs(&rows[k], is_vertical(t));
        for (idx, &x) in row_splits.iter().enumerate() {
            // no border inside a shared (tate-chu-yoko) cell
            if runs.iter().any(|(s, n)| idx >= *s && idx + 1 < s + n) {
                continue;
            }
            let c = to_cell(t, (x, x), band);
            out.push((CellBorder::Preview { row: k, idx }, frame.at(c[0], c[1]), frame.at(c[2], c[3])));
        }
    }
    out
}

/// Borders between consecutive character boxes of a line that remember their cell.
pub fn child_borders(page: &PageFile, line: &Annotation) -> Vec<(CellBorder, P, P)> {
    let Some(t) = &line.transcription else { return Vec::new() };
    let (frame, _) = frame_for(line, page);
    let mut kids: Vec<(&Annotation, [f64; 4], usize)> = chars_of(page, &line.id)
        .into_iter()
        .filter_map(|c| c.provenance.heuristic.as_ref().and_then(|h| h.cell.map(|cell| (c, cell, h.span[0]))))
        .collect();
    kids.sort_by_key(|k| k.2);
    let vertical = is_vertical(t);
    let mut out = Vec::new();
    for w in kids.windows(2) {
        let ((a, ca, _), (b, cb, _)) = (&w[0], &w[1]);
        // same row: same cross band
        let same = if vertical { (ca[0] - cb[0]).abs() < 1e-3 } else { (ca[1] - cb[1]).abs() < 1e-3 };
        if !same {
            continue;
        }
        let x = border_of(t, ca);
        let c = if vertical { [ca[0], x, ca[2], x] } else { [x, ca[1], x, ca[3]] };
        out.push((CellBorder::Pair { first: a.id.clone(), second: b.id.clone() }, frame.at(c[0], c[1]), frame.at(c[2], c[3])));
    }
    out
}

/// Frame coordinate where a cell ends in reading progression.
fn border_of(t: &Transcription, c: &[f64; 4]) -> f64 {
    match (is_vertical(t), reversed(t)) {
        (true, false) => c[3],
        (true, true) => c[1],
        (false, false) => c[2],
        (false, true) => c[0],
    }
}

/// Position of an image point in an annotation's frame as `(u, v)` fractions.
pub fn frame_unit(a: &Annotation, page: &PageFile, p: P) -> Option<(f64, f64)> {
    let (frame, _) = frame_for(a, page);
    let unit = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let corners = [frame.at(0.0, 0.0), frame.at(1.0, 0.0), frame.at(1.0, 1.0), frame.at(0.0, 1.0)];
    let h = geom::homography(&corners, &unit)?;
    let q = geom::apply_h(&h, p);
    Some((q[0], q[1]))
}

/// Progression fraction (0..1 in reading order) of an image point along a line.
pub fn progression_at(a: &Annotation, page: &PageFile, p: P) -> Option<f64> {
    let t = a.transcription.as_ref()?;
    let (u, v) = frame_unit(a, page, p)?;
    let x = if is_vertical(t) { v } else { u };
    Some(if reversed(t) { 1.0 - x } else { x })
}

/// Move a previewed border to progression fraction `x`, keeping every cell at least 2% long.
pub fn move_split(splits: &mut Splits, row: usize, idx: usize, x: f64) {
    let Some(r) = splits.get_mut(row) else { return };
    if idx >= r.len() {
        return;
    }
    let lo = if idx == 0 { 0.0 } else { r[idx - 1] } + 0.02;
    let hi = if idx + 1 >= r.len() { 1.0 } else { r[idx + 1] } - 0.02;
    if lo < hi {
        r[idx] = x.clamp(lo, hi);
    }
}

/// Move the border between two existing character boxes to progression fraction `x`.
pub fn move_pair_border(page: &mut PageFile, line_id: &str, first: &str, second: &str, x: f64) {
    let Some(line) = page.get(line_id).cloned() else { return };
    let Some(t) = line.transcription.clone() else { return };
    let (frame, _) = frame_for(&line, page);
    let cell = |page: &PageFile, id: &str| page.get(id).and_then(|c| c.provenance.heuristic.as_ref()).and_then(|h| h.cell);
    let (Some(ca), Some(cb)) = (cell(page, first), cell(page, second)) else { return };
    let vertical = is_vertical(&t);
    let rev = reversed(&t);
    // progression intervals of both cells
    let prog = |c: &[f64; 4]| {
        let (a0, a1) = if vertical { (c[1], c[3]) } else { (c[0], c[2]) };
        if rev { (1.0 - a1, 1.0 - a0) } else { (a0, a1) }
    };
    let (pa, pb) = (prog(&ca), prog(&cb));
    let x = x.clamp(pa.0 + 0.02, pb.1 - 0.02);
    if pa.0 + 0.02 > pb.1 - 0.02 {
        return;
    }
    let band = if vertical { (ca[0], ca[2]) } else { (ca[1], ca[3]) };
    let band_b = if vertical { (cb[0], cb[2]) } else { (cb[1], cb[3]) };
    let na = to_cell(&t, (pa.0, x), band);
    let nb = to_cell(&t, (x, pb.1), band_b);
    for (id, c) in [(first, na), (second, nb)] {
        if let Some(ch) = page.get_mut(id) {
            ch.geometry = normalize_shape(Geometry::Quad { points: cell_quad(&frame, c) });
            if let Some(h) = &mut ch.provenance.heuristic {
                h.cell = Some(c.map(|v| (v * 1e5).round() / 1e5));
            }
        }
    }
}

/// Character annotations made from the proposals, marked as heuristic and unverified.
pub fn char_annotations(a: &Annotation, page: &PageFile, cfg: &ProjectConfig, user: &str, now: &str) -> Vec<Annotation> {
    char_annotations_split(a, page, cfg, user, now, None)
}

pub fn char_annotations_split(a: &Annotation, page: &PageFile, cfg: &ProjectConfig, user: &str, now: &str, splits: Option<&Splits>) -> Vec<Annotation> {
    let (props, frame) = propose_chars_split(a, page, &cfg.quick.chars, splits);
    let t = a.transcription.clone().unwrap_or_default();
    props
        .into_iter()
        .enumerate()
        .map(|(k, p)| {
            let mut c = Annotation::new(crate::util::new_id("a"), Level::Char, &a.page_id, normalize_shape(Geometry::Quad { points: p.quad }), user, now);
            c.parent = Some(a.id.clone());
            c.classes = a.classes.clone();
            c.transcription = Some(Transcription {
                text: p.text.clone(),
                language: t.language.clone(),
                script: t.script.clone(),
                writing_mode: t.writing_mode,
                direction: t.direction,
                ..Default::default()
            });
            c.reading_order = Some(k as u32 + 1);
            c.reading = Some(ReadingInfo { source: Some(OrderSource::Suggested), ..Default::default() });
            c.provenance = Provenance {
                origin: Origin::Heuristic,
                ai: None,
                fields: [("geometry", FieldState::HeuristicProposal), ("transcription", FieldState::HeuristicProposal), ("reading_order", FieldState::Inferred)]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                heuristic: Some(HeuristicOrigin {
                    algorithm: CHAR_ALGO.into(),
                    version: CHAR_ALGO_VERSION.into(),
                    source: a.id.clone(),
                    source_text: t.text.clone(),
                    source_transcription_revision: a.revision,
                    parent_geometry_revision: a.revision,
                    span: p.span,
                    frame: frame.into(),
                    cell: Some(p.cell.map(|x| (x * 1e5).round() / 1e5)),
                    pinned: false,
                }),
            };
            c.review.state = ReviewState::AiProposed;
            c
        })
        .collect()
}

pub fn chars_of<'a>(page: &'a PageFile, id: &str) -> Vec<&'a Annotation> {
    page.annotations.iter().filter(|c| c.level == Level::Char && c.parent.as_deref() == Some(id)).collect()
}

/// Proposal not yet touched by anyone (safe to replace).
pub fn untouched_proposal(c: &Annotation) -> bool {
    c.provenance.origin == Origin::Heuristic && c.provenance.fields.get("geometry") == Some(&FieldState::HeuristicProposal) && c.review.state == ReviewState::AiProposed
}

/// Why a heuristic character box no longer matches its source, if it doesn't.
pub fn stale_reason(page: &PageFile, c: &Annotation) -> Option<String> {
    let h = c.provenance.heuristic.as_ref().filter(|h| h.algorithm == CHAR_ALGO)?;
    let Some(src) = page.get(&h.source) else { return Some("its line was deleted".into()) };
    if src.text() != h.source_text {
        return Some(format!("the line's text changed from “{}”", h.source_text));
    }
    let (lo, hi) = c.geometry.bounds();
    let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    if !src.geometry.contains(centre, 2.0) {
        return Some("it no longer lies inside its line".into());
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharStatus {
    /// No character boxes yet; `n` would be proposed.
    None(usize),
    /// Heuristic proposals nobody accepted yet.
    Proposed(usize),
    Stale(usize),
    Done(usize),
}

pub fn char_status(page: &PageFile, a: &Annotation, cfg: &ProjectConfig) -> CharStatus {
    let kids = chars_of(page, &a.id);
    if kids.is_empty() {
        return CharStatus::None(propose_chars(a, page, &cfg.quick.chars).0.len());
    }
    let stale = kids.iter().filter(|c| stale_reason(page, c).is_some()).count();
    if stale > 0 {
        return CharStatus::Stale(stale);
    }
    let pending = kids.iter().filter(|c| c.review.state == ReviewState::AiProposed && c.provenance.origin == Origin::Heuristic).count();
    if pending > 0 { CharStatus::Proposed(pending) } else { CharStatus::Done(kids.len()) }
}

/// (Re)generate character proposals for one annotation. Boxes a person touched are kept; only
/// untouched proposals are replaced, and only spans not covered by kept boxes are proposed again.
/// Returns (added, kept, removed).
pub fn regenerate_chars(page: &mut PageFile, id: &str, cfg: &ProjectConfig, user: &str, now: &str) -> (usize, usize, usize) {
    regenerate_chars_split(page, id, cfg, user, now, None)
}

/// [`regenerate_chars`] using cell borders a person chose in the preview.
pub fn regenerate_chars_split(page: &mut PageFile, id: &str, cfg: &ProjectConfig, user: &str, now: &str, splits: Option<&Splits>) -> (usize, usize, usize) {
    let Some(a) = page.get(id).cloned() else { return (0, 0, 0) };
    let before = page.annotations.len();
    page.annotations.retain(|c| !(c.level == Level::Char && c.parent.as_deref() == Some(id) && untouched_proposal(c)));
    let removed = before - page.annotations.len();
    let graphemes: Vec<&str> = a.text().graphemes(true).collect();
    let mut covered = BTreeSet::new();
    let kept: Vec<Annotation> = chars_of(page, id).into_iter().cloned().collect();
    for c in &kept {
        match &c.provenance.heuristic {
            Some(h) if h.span[1] <= graphemes.len() && graphemes[h.span[0]..h.span[1]].concat() == c.text() => {
                covered.extend(h.span[0]..h.span[1]);
            }
            _ => {
                if let Some(i) = c.reading_order.map(|r| r as usize - 1).filter(|i| graphemes.get(*i) == Some(&c.text())) {
                    covered.insert(i);
                }
            }
        }
    }
    let new: Vec<Annotation> = char_annotations_split(&a, page, cfg, user, now, splits)
        .into_iter()
        .filter(|c| c.provenance.heuristic.as_ref().is_some_and(|h| !covered.contains(&h.span[0])))
        .collect();
    let added = new.len();
    page.annotations.extend(new);
    // number all children by their position in the text
    let mut kids: Vec<(usize, String)> = page
        .annotations
        .iter()
        .filter(|c| c.level == Level::Char && c.parent.as_deref() == Some(id))
        .map(|c| (c.provenance.heuristic.as_ref().map(|h| h.span[0]).or(c.reading_order.map(|r| r as usize)).unwrap_or(usize::MAX), c.id.clone()))
        .collect();
    kids.sort();
    for (k, (_, cid)) in kids.iter().enumerate() {
        if let Some(c) = page.get_mut(cid) {
            if !manual_order(c) {
                c.reading_order = Some(k as u32 + 1);
            }
        }
    }
    (added, kept.len(), removed)
}

/// Accept heuristic character boxes as drafts: still not verified, but owned by a person now.
pub fn accept_chars(page: &mut PageFile, id: &str, user: &str, now: &str) -> usize {
    let ids: Vec<String> = chars_of(page, id).iter().filter(|c| c.review.state == ReviewState::AiProposed).map(|c| c.id.clone()).collect();
    for cid in &ids {
        if let Some(c) = page.get_mut(cid) {
            review::accept_ai(c, user, now);
        }
    }
    ids.len()
}

// ------------------------------------------------------------------ reading order

pub fn manual_order(a: &Annotation) -> bool {
    a.reading_order.is_some() && a.reading.as_ref().and_then(|r| r.source) != Some(OrderSource::Suggested)
}

fn parse_dir(s: &str) -> Option<Direction> {
    match s.trim().to_lowercase().as_str() {
        "rtl" | "right-to-left" | "right_to_left" => Some(Direction::Rtl),
        "ltr" | "left-to-right" | "left_to_right" => Some(Direction::Ltr),
        _ => None,
    }
}

/// Page reading direction: the page's source setting, else the project default.
pub fn page_direction(source: Option<&SourceInfo>, cfg: &ProjectConfig) -> Direction {
    source.and_then(|s| parse_dir(&s.reading_direction)).unwrap_or(cfg.reading.page_direction)
}

fn region_progression(page: &PageFile, parent: Option<&str>, page_dir: Direction) -> Direction {
    let mut cur = parent;
    let mut guard = 0;
    while let (Some(id), true) = (cur, guard < 16) {
        let Some(p) = page.get(id) else { break };
        if let Some(d) = p.reading.as_ref().and_then(|r| r.progression) {
            return d;
        }
        cur = p.parent.as_deref();
        guard += 1;
    }
    page_dir
}

fn region_writing(page: &PageFile, items: &[&Annotation], parent: Option<&str>, cfg: &ProjectConfig) -> (WritingMode, Direction) {
    let mut votes: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for a in items {
        if let Some(t) = &a.transcription {
            *votes.entry((t.writing_mode as u8, t.direction as u8)).or_default() += 1;
        }
    }
    if let Some((&(wm, d), _)) = votes.iter().max_by_key(|(_, n)| **n) {
        return (WritingMode::ALL[wm as usize], Direction::ALL[d as usize]);
    }
    let mut cur = parent;
    while let Some(id) = cur {
        let Some(p) = page.get(id) else { break };
        if let Some(wm) = p.reading.as_ref().and_then(|r| r.writing_mode) {
            return (wm, cfg.default_direction);
        }
        cur = p.parent.as_deref();
    }
    (cfg.default_writing_mode, cfg.default_direction)
}

type R = (P, P);

fn overlap_1d(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Group items into bands along one axis (rows: axis 1 = y; columns: axis 0 = x).
fn bands(items: &[(String, R)], axis: usize) -> Vec<Vec<(String, R)>> {
    let mut sorted = items.to_vec();
    sorted.sort_by(|a, b| a.1.0[axis].total_cmp(&b.1.0[axis]));
    let mut out: Vec<(f64, f64, f64, Vec<(String, R)>)> = Vec::new();
    for it in sorted {
        let (lo, hi) = (it.1.0[axis], it.1.1[axis]);
        let size = hi - lo;
        match out.iter_mut().find(|b| overlap_1d(b.0, b.1, lo, hi) >= 0.5 * size.min(b.2)) {
            Some(b) => {
                b.0 = b.0.min(lo);
                b.1 = b.1.max(hi);
                b.2 = b.2.min(size);
                b.3.push(it);
            }
            None => out.push((lo, hi, size, vec![it])),
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|b| b.3).collect()
}

fn centre(r: &R) -> P {
    [(r.0[0] + r.1[0]) / 2.0, (r.0[1] + r.1[1]) / 2.0]
}

/// Order blocks (panels, bubbles, regions) by the layout strategy.
fn order_blocks(items: &[(String, R)], dir: Direction, strategy: OrderStrategy) -> Vec<String> {
    let rtl = dir == Direction::Rtl;
    let by_x = |v: &mut Vec<(String, R)>| v.sort_by(|a, b| if rtl { centre(&b.1)[0].total_cmp(&centre(&a.1)[0]) } else { centre(&a.1)[0].total_cmp(&centre(&b.1)[0]) });
    match strategy {
        OrderStrategy::Tiers => bands(items, 1)
            .into_iter()
            .flat_map(|mut row| {
                by_x(&mut row);
                row.into_iter().map(|x| x.0)
            })
            .collect(),
        OrderStrategy::Columns => {
            let mut cols = bands(items, 0);
            if rtl {
                cols.reverse();
            }
            cols.into_iter()
                .flat_map(|mut col| {
                    col.sort_by(|a, b| centre(&a.1)[1].total_cmp(&centre(&b.1)[1]));
                    col.into_iter().map(|x| x.0)
                })
                .collect()
        }
        OrderStrategy::Position => {
            let mut v = items.to_vec();
            let max_x = v.iter().map(|x| x.1.1[0]).fold(0.0, f64::max);
            let start = |r: &R| if rtl { [max_x - r.1[0], r.0[1]] } else { [r.0[0], r.0[1]] };
            v.sort_by(|a, b| {
                let (pa, pb) = (start(&a.1), start(&b.1));
                (pa[0] * pa[0] + pa[1] * pa[1]).total_cmp(&(pb[0] * pb[0] + pb[1] * pb[1]))
            });
            v.into_iter().map(|x| x.0).collect()
        }
    }
}

/// Order lines or columns of text by their writing mode.
fn order_lines(items: &[(String, R)], wm: WritingMode, dir: Direction) -> Vec<String> {
    match wm {
        WritingMode::VerticalRl | WritingMode::VerticalLr => {
            let mut cols = bands(items, 0);
            if wm == WritingMode::VerticalRl {
                cols.reverse();
            }
            cols.into_iter()
                .flat_map(|mut c| {
                    c.sort_by(|a, b| a.1.0[1].total_cmp(&b.1.0[1]));
                    c.into_iter().map(|x| x.0)
                })
                .collect()
        }
        _ => order_blocks(items, if dir == Direction::Rtl { Direction::Rtl } else { Direction::Ltr }, OrderStrategy::Tiers),
    }
}

/// Order characters along their line (through the flattening homography when there is one).
fn order_chars(page: &PageFile, parent: Option<&Annotation>, items: &[&Annotation]) -> Vec<String> {
    let Some(line) = parent else {
        return items.iter().map(|a| a.id.clone()).collect();
    };
    let t = line.transcription.clone().unwrap_or_default();
    let vertical = is_vertical(&t);
    let q = own_corners(line);
    let flat = flattening(line, page).and_then(|r| {
        let (w, h) = (r.target_size[0] as f64, r.target_size[1] as f64);
        geom::homography(&r.source, &[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]])
    });
    let axis = if vertical { [q[3][0] - q[0][0], q[3][1] - q[0][1]] } else { [q[1][0] - q[0][0], q[1][1] - q[0][1]] };
    let key = |a: &Annotation| {
        let (lo, hi) = a.geometry.bounds();
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let k = match &flat {
            Some(h) => {
                let p = geom::apply_h(h, c);
                if vertical { p[1] } else { p[0] }
            }
            None => (c[0] - q[0][0]) * axis[0] + (c[1] - q[0][1]) * axis[1],
        };
        let rev = if vertical { t.direction == Direction::Btt } else { t.direction == Direction::Rtl };
        if rev { -k } else { k }
    };
    let mut v: Vec<&&Annotation> = items.iter().collect();
    v.sort_by(|a, b| key(a).total_cmp(&key(b)));
    v.into_iter().map(|a| a.id.clone()).collect()
}

/// Suggested order of every sibling group: ((parent, level), ordered ids).
pub fn suggested_sequences(page: &PageFile, cfg: &ProjectConfig, page_dir: Direction) -> Vec<((Option<String>, Level), Vec<String>)> {
    suggested_sequences_in(page, cfg, page_dir, None)
}

/// Like [`suggested_sequences`], optionally for one sibling group only (much cheaper).
pub fn suggested_sequences_in(page: &PageFile, cfg: &ProjectConfig, page_dir: Direction, only: Option<(&Option<String>, Level)>) -> Vec<((Option<String>, Level), Vec<String>)> {
    let mut groups: BTreeMap<(Option<String>, Level), Vec<&Annotation>> = BTreeMap::new();
    for a in &page.annotations {
        if a.reading.as_ref().and_then(|r| r.mark) == Some(OrderMark::Unordered) || is_unordered(a, cfg) {
            continue;
        }
        if let Some((p, l)) = only {
            if &a.parent != p || a.level != l {
                continue;
            }
        }
        groups.entry((a.parent.clone(), a.level)).or_default().push(a);
    }
    let mut out = Vec::new();
    for ((parent, level), items) in groups {
        let parent_ann = parent.as_deref().and_then(|p| page.get(p));
        if parent_ann.and_then(|p| p.reading.as_ref()).and_then(|r| r.mark) == Some(OrderMark::Unordered) {
            continue;
        }
        let rects: Vec<(String, R)> = items.iter().map(|a| (a.id.clone(), a.geometry.bounds())).collect();
        let seq = match level {
            Level::Page | Level::Panel | Level::Container | Level::Region => {
                order_blocks(&rects, region_progression(page, parent.as_deref(), page_dir), cfg.reading.strategy)
            }
            Level::Line | Level::Word => {
                let (wm, d) = region_writing(page, &items, parent.as_deref(), cfg);
                order_lines(&rects, wm, d)
            }
            Level::Char => order_chars(page, parent_ann, &items),
        };
        out.push(((parent, level), seq));
    }
    out
}

/// Reading-order numbers for items without a manual order. Manual numbers stay fixed;
/// suggested items fill the free numbers in layout order.
pub fn suggest_order(page: &PageFile, cfg: &ProjectConfig, page_dir: Direction) -> Vec<(String, u32)> {
    suggest_order_in(page, cfg, page_dir, None)
}

/// Suggested number for one annotation, looking only at its own sibling group.
pub fn suggest_order_for(page: &PageFile, cfg: &ProjectConfig, page_dir: Direction, a: &Annotation) -> Option<u32> {
    suggest_order_in(page, cfg, page_dir, Some((&a.parent, a.level))).into_iter().find(|x| x.0 == a.id).map(|x| x.1)
}

fn suggest_order_in(page: &PageFile, cfg: &ProjectConfig, page_dir: Direction, only: Option<(&Option<String>, Level)>) -> Vec<(String, u32)> {
    let index: std::collections::HashMap<&str, &Annotation> = page.annotations.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut out = Vec::new();
    for (_, seq) in suggested_sequences_in(page, cfg, page_dir, only) {
        let used: BTreeSet<u32> = seq.iter().filter_map(|id| index.get(id.as_str())).filter(|a| manual_order(a)).filter_map(|a| a.reading_order).collect();
        let mut next = 1;
        for id in &seq {
            let a = index[id.as_str()];
            if manual_order(a) {
                continue;
            }
            while used.contains(&next) {
                next += 1;
            }
            if a.reading_order != Some(next) {
                out.push((id.clone(), next));
            }
            next += 1;
        }
    }
    out
}

/// Apply suggested numbers (never to manually ordered items). Returns how many changed.
pub fn apply_order(page: &mut PageFile, cfg: &ProjectConfig, page_dir: Direction) -> usize {
    let assigns = suggest_order(page, cfg, page_dir);
    for (id, n) in &assigns {
        if let Some(a) = page.get_mut(id) {
            a.reading_order = Some(*n);
            a.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Suggested);
            a.provenance.fields.insert("reading_order".into(), FieldState::Inferred);
        }
    }
    assigns.len()
}

/// Likely ordering mistakes: groups whose (manual) order disagrees with a clear layout, and
/// heavily overlapping siblings whose order is ambiguous.
pub fn order_suspects(page: &PageFile, cfg: &ProjectConfig, page_dir: Direction) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for ((_, _), seq) in suggested_sequences(page, cfg, page_dir) {
        let items: Vec<&Annotation> = seq.iter().filter_map(|id| page.get(id)).collect();
        if items.len() < 2 || items.iter().any(|a| a.reading_order.is_none() || a.reading.as_ref().and_then(|r| r.mark).is_some()) {
            continue;
        }
        let mut actual = items.clone();
        actual.sort_by_key(|a| a.reading_order);
        for (k, (want, got)) in seq.iter().zip(&actual).enumerate() {
            if *want != got.id {
                // only complain when the two are clearly apart on the page
                let a = page.get(want).unwrap();
                if geom::iou(a.geometry.bounds(), got.geometry.bounds()) < 0.05 {
                    out.push((got.id.clone(), format!("reading order {} differs from the usual layout order (expected position {})", got.reading_order.unwrap_or(0), k + 1)));
                }
                break;
            }
        }
        for (i, a) in items.iter().enumerate() {
            for b in &items[i + 1..] {
                if geom::iou(a.geometry.bounds(), b.geometry.bounds()) > 0.5 {
                    out.push((b.id.clone(), "overlaps a sibling heavily; the order may be ambiguous".into()));
                }
            }
        }
    }
    out
}

/// Move an item one step earlier (-1) or later (+1) among its ordered siblings. Both swapped
/// items become manually ordered.
pub fn nudge_order(page: &mut PageFile, id: &str, delta: i32) -> bool {
    let Some(a) = page.get(id).cloned() else { return false };
    let mut sib: Vec<(u32, String)> = page.annotations.iter().filter(|x| x.parent == a.parent && x.level == a.level).filter_map(|x| x.reading_order.map(|r| (r, x.id.clone()))).collect();
    sib.sort();
    let Some(pos) = sib.iter().position(|s| s.1 == id) else { return false };
    let other = pos as i32 + delta;
    if other < 0 || other as usize >= sib.len() {
        return false;
    }
    let (ra, rb) = (sib[pos].0, sib[other as usize].0);
    let ob = sib[other as usize].1.clone();
    for (x, r) in [(id.to_string(), rb), (ob, ra)] {
        if let Some(t) = page.get_mut(&x) {
            t.reading_order = Some(r);
            t.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
        }
    }
    true
}

// ------------------------------------------------------------------ quick review checklist

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NeedKind {
    /// No value yet.
    Missing,
    /// A value proposed or inferred by software that a person has not confirmed.
    Unverified,
    /// The value contradicts the image or other fields.
    Invalid,
    /// Derived data out of date (e.g. character boxes after a text change).
    Stale,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Suggest {
    Recognizability(Recognizability),
    Class(String),
    Language(Option<String>, Option<String>),
    Writing(WritingMode, Direction),
    Order(u32),
    ProposeChars(usize),
    AcceptChars(usize),
    AcceptPerspective,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Need {
    pub field: &'static str,
    pub kind: NeedKind,
    pub message: String,
    pub suggestion: Option<(Suggest, String)>,
}

pub fn field_label(f: &str) -> &'static str {
    QUICK_FIELDS.iter().find(|x| x.0 == f).map(|x| x.1).unwrap_or("Project field")
}

fn siblings(page: &PageFile, a: &Annotation, cfg: &ProjectConfig) -> usize {
    page.annotations.iter().filter(|x| x.parent == a.parent && x.level == a.level && !is_unordered(x, cfg)).count()
}

/// Read outside the reading sequence: furigana, and the project's unordered text types
/// (background text, sound effects and page numbers by default). Character boxes always keep
/// their order inside the line.
pub fn is_unordered(a: &Annotation, cfg: &ProjectConfig) -> bool {
    is_furigana(a) || (a.level != Level::Char && a.classes.iter().any(|c| cfg.reading.unordered_classes.contains(c)))
}

pub const FURIGANA_OF: &str = "furigana_of";

/// Furigana (ruby) is read with the characters it sits beside, not in the reading sequence.
pub fn is_furigana(a: &Annotation) -> bool {
    a.attributes.contains("furigana") || a.relations.iter().any(|r| r.kind == FURIGANA_OF)
}

/// Link a furigana annotation to the characters (or word, line) it reads.
pub fn link_furigana(page: &mut PageFile, ruby: &str, base: &str) -> bool {
    if ruby == base || page.get(base).is_none() {
        return false;
    }
    let Some(a) = page.get_mut(ruby) else { return false };
    a.relations.retain(|r| r.kind != FURIGANA_OF);
    a.relations.push(Relation { kind: FURIGANA_OF.into(), target: base.to_string() });
    a.attributes.insert("furigana".into());
    a.reading_order = None;
    true
}

/// Resize one character box and spread the rest of its line over the space that is left,
/// leaving pinned boxes alone. The resized box becomes pinned. `p` is the pointer position;
/// the box end nearest to it (in reading progression) moves there. Works on character boxes
/// that remember their cell (made from proposals).
pub fn smart_resize_char(page: &mut PageFile, char_id: &str, p: P) -> bool {
    let Some(ch) = page.get(char_id).cloned() else { return false };
    let Some(line_id) = ch.parent.clone() else { return false };
    let Some(line) = page.get(&line_id).cloned() else { return false };
    let Some(t) = line.transcription.clone() else { return false };
    let Some(x) = progression_at(&line, page, p) else { return false };
    let (frame, _) = frame_for(&line, page);
    let vertical = is_vertical(&t);
    let rev = reversed(&t);
    let prog = |c: &[f64; 4]| {
        let (a0, a1) = if vertical { (c[1], c[3]) } else { (c[0], c[2]) };
        if rev { (1.0 - a1, 1.0 - a0) } else { (a0, a1) }
    };
    let band = |c: &[f64; 4]| if vertical { (c[0], c[2]) } else { (c[1], c[3]) };
    let Some(target_cell) = ch.provenance.heuristic.as_ref().and_then(|h| h.cell) else { return false };
    // the row of the target: same cross band
    let tb = band(&target_cell);
    let mut row: Vec<(String, [f64; 4], bool)> = chars_of(page, &line_id)
        .into_iter()
        .filter_map(|c| c.provenance.heuristic.as_ref().and_then(|h| h.cell.map(|cell| (c.id.clone(), cell, h.pinned))))
        .filter(|(_, c, _)| (band(c).0 - tb.0).abs() < 1e-3)
        .collect();
    row.sort_by(|a, b| prog(&a.1).0.total_cmp(&prog(&b.1).0));
    let Some(ti) = row.iter().position(|r| r.0 == char_id) else { return false };
    let old: Vec<(f64, f64)> = row.iter().map(|r| prog(&r.1)).collect();
    let mut new = old.clone();
    // which end moves: the one nearest the pointer
    let (s0, s1) = old[ti];
    let min = 0.01;
    let lo_lim = (0..ti).rev().find(|&k| row[k].2).map(|k| old[k].1).unwrap_or(0.0) + min * (ti - (0..ti).rev().find(|&k| row[k].2).map(|k| k + 1).unwrap_or(0)) as f64;
    let hi_lim = (ti + 1..row.len()).find(|&k| row[k].2).map(|k| old[k].0).unwrap_or(1.0) - min * ((ti + 1..row.len()).find(|&k| row[k].2).unwrap_or(row.len()) - ti - 1) as f64;
    if (x - s0).abs() < (x - s1).abs() {
        new[ti].0 = x.clamp(lo_lim, s1 - min);
    } else {
        new[ti].1 = x.clamp(s0 + min, hi_lim);
    }
    // redistribute the free boxes between fixed ones, keeping their relative sizes and gaps
    let fixed: Vec<usize> = (0..row.len()).filter(|&k| k == ti || row[k].2).collect();
    let mut bounds: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let mut prev = None;
    for &f in &fixed {
        bounds.push((prev, Some(f)));
        prev = Some(f);
    }
    bounds.push((prev, None));
    for (a, b) in bounds {
        let first = a.map(|k| k + 1).unwrap_or(0);
        let last = b.unwrap_or(row.len());
        if first >= last {
            continue;
        }
        let (o0, o1) = (a.map(|k| old[k].1).unwrap_or(0.0), b.map(|k| old[k].0).unwrap_or(1.0));
        let (n0, n1) = (a.map(|k| new[k].1).unwrap_or(0.0), b.map(|k| new[k].0).unwrap_or(1.0));
        let span = (o1 - o0).max(1e-9);
        for k in first..last {
            let f = |v: f64| n0 + (v - o0) / span * (n1 - n0);
            new[k] = (f(old[k].0), f(old[k].1));
        }
    }
    for (k, (id, cell, _)) in row.iter().enumerate() {
        let c = to_cell(&t, new[k], band(cell));
        if let Some(a) = page.get_mut(id) {
            a.geometry = normalize_shape(Geometry::Quad { points: cell_quad(&frame, c) });
            if let Some(h) = &mut a.provenance.heuristic {
                h.cell = Some(c.map(|v| (v * 1e5).round() / 1e5));
                if k == ti {
                    h.pinned = true;
                }
            }
        }
    }
    true
}

/// Let every character box of a line be resized automatically again.
pub fn unpin_chars(page: &mut PageFile, line_id: &str) -> usize {
    let ids: Vec<String> = chars_of(page, line_id).iter().map(|c| c.id.clone()).collect();
    let mut n = 0;
    for id in ids {
        if let Some(h) = page.get_mut(&id).and_then(|c| c.provenance.heuristic.as_mut()) {
            if h.pinned {
                h.pinned = false;
                n += 1;
            }
        }
    }
    n
}

pub fn applicable(field: &str, a: &Annotation, page: &PageFile, cfg: &ProjectConfig) -> bool {
    match field {
        // a region's text is the text of its lines: it is not typed twice
        "transcription" => a.level.is_text() && !text_in_lines(page, a),
        "recognizability" | "language" | "direction" => a.level.is_text(),
        "classes" => matches!(a.level, Level::Container | Level::Region | Level::Line | Level::Word),
        "mirrored" | "occlusion" | "quality" => a.level.is_text() || a.level == Level::Container,
        "char_boxes" => matches!(a.level, Level::Line | Level::Word | Level::Region) && !a.text().trim().is_empty() && cfg.requirement(Level::Char, &a.classes) == Requirement::Required,
        "reading_order" => !is_unordered(a, cfg) && siblings(page, a, cfg) > 1 && a.reading.as_ref().and_then(|r| r.mark).is_none(),
        "perspective" => a.rectification.is_some() || a.classes.contains("signage"),
        "confidence" => a.level.is_text(),
        _ => true,
    }
}

fn check_state(a: &Annotation, f: &str) -> Option<CheckState> {
    a.checks.get(f).map(|c| c.state)
}

/// What still needs a person for this annotation, in checklist order (only required fields).
pub fn needs(a: &Annotation, page: &PageFile, cfg: &ProjectConfig) -> Vec<Need> {
    let mut v = Vec::new();
    let mut push = |field: &'static str, kind: NeedKind, message: String, suggestion: Option<(Suggest, String)>| v.push(Need { field, kind, message, suggestion });
    let rec = a.provenance.ai.as_ref().and_then(|x| x.recognition_score);
    for f in &cfg.quick.required_fields {
        let Some(field) = QUICK_FIELDS.iter().map(|x| x.0).find(|x| x == f) else { continue };
        if !applicable(field, a, page, cfg) {
            continue;
        }
        let st = check_state(a, field);
        if matches!(st, Some(CheckState::Unknown | CheckState::NotApplicable)) {
            continue; // answered
        }
        let inferred = st == Some(CheckState::Inferred);
        let src = a.checks.get(field).map(|c| c.source.clone()).unwrap_or_default();
        match field {
            "transcription" => {
                let unreadable = a.recognizability == Some(Recognizability::Unreadable);
                if a.text().trim().is_empty() && !unreadable {
                    push(field, NeedKind::Missing, "type what the text says, or mark it unreadable".into(), None);
                } else if matches!(a.provenance.fields.get("transcription"), Some(FieldState::Proposed | FieldState::HeuristicProposal)) && st != Some(CheckState::Confirmed) {
                    push(field, NeedKind::Unverified, "read by the model; check it against the image".into(), None);
                }
            }
            "recognizability" => {
                if a.recognizability.is_none() {
                    let s = match rec {
                        Some(r) if r >= 0.9 => Some((Suggest::Recognizability(Recognizability::Readable), format!("model read it with score {r:.2}"))),
                        _ if !a.text().trim().is_empty() && a.provenance.origin != Origin::Ai => Some((Suggest::Recognizability(Recognizability::Readable), "a person transcribed it".into())),
                        _ => None,
                    };
                    push(field, NeedKind::Missing, "how readable is it?".into(), s);
                } else if a.recognizability.is_some_and(|r| r.coarse()) {
                    push(field, NeedKind::Missing, "not perfectly readable: say how (partly readable, ambiguous or unreadable)".into(), None);
                } else if inferred {
                    push(field, NeedKind::Unverified, format!("inferred ({src})"), None);
                }
            }
            "classes" => {
                if a.classes.is_empty() {
                    let from_parent = a.parent.as_deref().and_then(|p| page.get(p)).and_then(|p| p.classes.iter().next().cloned().map(|c| (c, p.level)));
                    let s = match from_parent {
                        Some((c, l)) => Some((Suggest::Class(c), format!("from the surrounding {}", l.name().to_lowercase()))),
                        None => {
                            let parent_shape = a.parent.as_deref().and_then(|p| page.get(p)).map(|p| (p.level, p.geometry.kind()));
                            match parent_shape {
                                Some((Level::Container, "polygon")) => Some((Suggest::Class("dialogue".into()), "inside a balloon".into())),
                                _ => None,
                            }
                        }
                    };
                    push(field, NeedKind::Missing, "what kind of text is it?".into(), s);
                } else if inferred {
                    push(field, NeedKind::Unverified, format!("inferred ({src})"), None);
                }
            }
            "language" => {
                let t = a.transcription.clone().unwrap_or_default();
                let (dl, ds) = detect_script(&t.text);
                let sugg = || {
                    let lang = dl.map(String::from).or_else(|| (!cfg.default_language.is_empty()).then(|| cfg.default_language.clone()));
                    let script = ds.map(String::from).filter(|s| s != "Hani" || cfg.default_script.is_empty()).or_else(|| (!cfg.default_script.is_empty()).then(|| cfg.default_script.clone()));
                    let why = if dl.is_some() { "detected from the text" } else { "project default" };
                    (Suggest::Language(lang, script), why.to_string())
                };
                if t.language.is_none() {
                    push(field, NeedKind::Missing, "which language?".into(), Some(sugg()));
                } else if let (Some(decl), Some(det)) = (&t.script, ds) {
                    if !script_compatible(decl, det) {
                        push(field, NeedKind::Invalid, format!("script {decl} does not match the text ({det})"), Some(sugg()));
                    } else if inferred {
                        push(field, NeedKind::Unverified, format!("inferred ({src})"), None);
                    }
                } else if inferred {
                    push(field, NeedKind::Unverified, format!("inferred ({src})"), None);
                }
            }
            "direction" => {
                let Some(t) = &a.transcription else { continue };
                let q = own_corners(a);
                let w = (geom::dist(q[0], q[1]) + geom::dist(q[3], q[2])) / 2.0;
                let h = (geom::dist(q[0], q[3]) + geom::dist(q[1], q[2])) / 2.0;
                let n = t.text.graphemes(true).filter(|g| !g.trim().is_empty()).count();
                let looks_vertical = h > w * 1.6 && n >= 2;
                let looks_horizontal = w > h * 1.6 && n >= 2;
                let vertical = is_vertical(t);
                let vdir = if cfg.default_writing_mode == WritingMode::VerticalLr { WritingMode::VerticalLr } else { WritingMode::VerticalRl };
                if looks_vertical && !vertical {
                    push(field, NeedKind::Invalid, "the box is tall but the text is set horizontal".into(), Some((Suggest::Writing(vdir, Direction::Ttb), "box shape".into())));
                } else if looks_horizontal && vertical {
                    push(field, NeedKind::Invalid, "the box is wide but the text is set vertical".into(), Some((Suggest::Writing(WritingMode::HorizontalTb, Direction::Ltr), "box shape".into())));
                } else if inferred {
                    push(field, NeedKind::Unverified, format!("inferred ({src})"), None);
                }
            }
            "mirrored" | "occlusion" | "quality" | "perspective" => {
                let answered = match field {
                    "occlusion" => a.occlusion.is_some() && st != Some(CheckState::Inferred),
                    _ => false,
                } || st == Some(CheckState::Confirmed);
                if !answered {
                    let s = (field == "perspective" && a.rectification.is_some()).then(|| (Suggest::AcceptPerspective, "flattening already set up".into()));
                    push(field, NeedKind::Missing, "not answered yet".into(), s);
                }
            }
            "char_boxes" => match char_status(page, a, cfg) {
                CharStatus::None(n) => push(field, NeedKind::Missing, format!("no character boxes; {n} can be proposed"), Some((Suggest::ProposeChars(n), "split from the transcription".into()))),
                CharStatus::Proposed(n) => push(field, NeedKind::Unverified, format!("{n} proposed boxes not yet accepted"), Some((Suggest::AcceptChars(n), "heuristic proposal".into()))),
                CharStatus::Stale(n) => push(field, NeedKind::Stale, format!("{n} boxes no longer match the text"), None),
                CharStatus::Done(_) => {}
            },
            "reading_order" => {
                let page_dir = cfg.reading.page_direction;
                if a.reading_order.is_none() {
                    let s = suggest_order_for(page, cfg, page_dir, a).map(|n| (Suggest::Order(n), "from the layout".into()));
                    push(field, NeedKind::Missing, "no reading order".into(), s);
                } else if !manual_order(a) && st != Some(CheckState::Confirmed) {
                    push(field, NeedKind::Unverified, format!("suggested as {} from the layout", a.reading_order.unwrap()), None);
                }
            }
            _ => {}
        }
    }
    // a quick "uncertain" answer is refined later, whether or not confidence is required
    let c = &a.confidence;
    if [c.transcription, c.localization, c.geometry, c.reading_order].contains(&Some(Confidence::Uncertain)) {
        v.push(Need { field: "confidence", kind: NeedKind::Missing, message: "marked uncertain: refine to low or medium per aspect".into(), suggestion: None });
    } else if cfg.quick.required_fields.iter().any(|f| f == "confidence") && a.level.is_text() && c.transcription.is_none() && c.geometry.is_none() {
        v.push(Need { field: "confidence", kind: NeedKind::Missing, message: "are you certain of this annotation?".into(), suggestion: None });
    }
    for f in cfg.custom_fields.iter().filter(|f| f.required && (f.levels.is_empty() || f.levels.contains(&a.level))) {
        if a.fields.get(&f.key).is_none_or(|v| v.is_null()) {
            v.push(Need { field: "custom", kind: NeedKind::Missing, message: format!("“{}” is required", f.label), suggestion: None });
        }
    }
    v
}

/// The needs that stop one-key approval under the project policy.
pub fn blocking<'a>(needs: &'a [Need], policy: &QuickReviewPolicy) -> Vec<&'a Need> {
    needs
        .iter()
        .filter(|n| match n.kind {
            NeedKind::Missing | NeedKind::Invalid | NeedKind::Stale => true,
            // proposed character geometry is never accepted implicitly
            NeedKind::Unverified => n.field == "char_boxes" || (policy.confirm_inferred_individually && n.field != "transcription"),
        })
        .collect()
}

pub fn set_check(a: &mut Annotation, field: &str, state: CheckState, user: &str, now: &str, source: &str, batch: bool) {
    a.checks.insert(field.into(), FieldCheck { state, by: user.into(), at: now.into(), source: source.into(), batch });
}

/// Apply a suggestion to the annotation (a person chose it).
pub fn apply_suggestion(page: &mut PageFile, id: &str, s: &Suggest, cfg: &ProjectConfig, user: &str, now: &str) {
    if let Suggest::ProposeChars(_) = s {
        regenerate_chars(page, id, cfg, user, now);
        return;
    }
    if let Suggest::AcceptChars(_) = s {
        accept_chars(page, id, user, now);
        if let Some(a) = page.get_mut(id) {
            set_check(a, "char_boxes", CheckState::Confirmed, user, now, "accepted proposals", false);
        }
        return;
    }
    let Some(a) = page.get_mut(id) else { return };
    let (field, source) = match s {
        Suggest::Recognizability(r) => {
            a.recognizability = Some(*r);
            ("recognizability", "suggestion accepted")
        }
        Suggest::Class(c) => {
            a.classes.insert(c.clone());
            ("classes", "suggestion accepted")
        }
        Suggest::Language(l, sc) => {
            let t = a.transcription.get_or_insert_with(Default::default);
            if l.is_some() {
                t.language = l.clone();
            }
            if sc.is_some() {
                t.script = sc.clone();
            }
            ("language", "suggestion accepted")
        }
        Suggest::Writing(wm, d) => {
            let t = a.transcription.get_or_insert_with(Default::default);
            t.writing_mode = *wm;
            t.direction = *d;
            ("direction", "suggestion accepted")
        }
        Suggest::Order(n) => {
            a.reading_order = Some(*n);
            a.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
            ("reading_order", "suggestion accepted")
        }
        Suggest::AcceptPerspective => ("perspective", "flattening accepted"),
        Suggest::ProposeChars(_) | Suggest::AcceptChars(_) => unreachable!(),
    };
    set_check(a, field, CheckState::Confirmed, user, now, source, false);
}

/// One-action approval: if nothing blocks, confirm the shown values and approve every required
/// aspect. Otherwise return the first unresolved need (nothing is invented to fill it).
/// Returns `Ok(true)` when approved, `Ok(false)` when the reviewer may not approve their own work
/// under the project policy: the checks are still recorded and it is submitted for someone else.
pub fn quick_approve(page: &mut PageFile, id: &str, cfg: &ProjectConfig, user: &str, now: &str) -> Result<bool, String> {
    let snapshot = page.clone();
    let a = snapshot.get(id).ok_or("annotation not found")?;
    let ns = needs(a, &snapshot, cfg);
    if let Some(b) = blocking(&ns, &cfg.quick).first() {
        return Err(format!("{}: {}", field_label(b.field), b.message));
    }
    let may_approve = review::can_review(a, &cfg.review, user).is_ok();
    let a = page.get_mut(id).unwrap();
    for f in &cfg.quick.required_fields {
        if applicable(f, a, &snapshot, cfg) && !matches!(check_state(a, f), Some(CheckState::Unknown | CheckState::NotApplicable)) {
            set_check(a, f, CheckState::Confirmed, user, now, "approved at review", false);
        }
    }
    if a.reading_order.is_some() && !manual_order(a) {
        a.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
    }
    if !may_approve {
        review::submit(a, &cfg.review);
        return Ok(false);
    }
    review::approve(a, &cfg.review.required_dimensions, &cfg.review, user, now).map(|_| true)
}

// ------------------------------------------------------------------ review queues

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueKind {
    All,
    AiUnreviewed,
    LowConfidence,
    MissingFields,
    SuspectGeometry,
    HeuristicChars,
    NeedsPeer,
    ChangedAfterVerification,
}

impl QueueKind {
    pub const ALL: [QueueKind; 8] = [
        QueueKind::All,
        QueueKind::AiUnreviewed,
        QueueKind::LowConfidence,
        QueueKind::MissingFields,
        QueueKind::SuspectGeometry,
        QueueKind::HeuristicChars,
        QueueKind::NeedsPeer,
        QueueKind::ChangedAfterVerification,
    ];
    pub fn name(self) -> &'static str {
        match self {
            QueueKind::All => "Any label",
            QueueKind::AiUnreviewed => "AI proposals not reviewed",
            QueueKind::LowConfidence => "Low confidence",
            QueueKind::MissingFields => "Missing required fields",
            QueueKind::SuspectGeometry => "Suspect geometry",
            QueueKind::HeuristicChars => "Proposed character boxes",
            QueueKind::NeedsPeer => "Needs peer verification",
            QueueKind::ChangedAfterVerification => "Changed after verification",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueueItem {
    pub page_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_id: Option<String>,
    pub pass: Pass,
}

/// One step of the page review workflow. Item passes visit labels one by one; page passes are
/// one step for the whole page.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Pass {
    LineBoxes,
    MissingBoxes,
    Reread,
    Transcription,
    CharBoxes,
    Regions,
    Containers,
    Panels,
    Legibility,
    TextType,
    Confidence,
    Rest,
    ReadingOrder,
}

impl Pass {
    pub const ALL: [Pass; 13] = [
        Pass::LineBoxes,
        Pass::MissingBoxes,
        Pass::Reread,
        Pass::Transcription,
        Pass::CharBoxes,
        Pass::Regions,
        Pass::Containers,
        Pass::Panels,
        Pass::Legibility,
        Pass::TextType,
        Pass::Confidence,
        Pass::Rest,
        Pass::ReadingOrder,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Pass::LineBoxes => "Line boxes",
            Pass::MissingBoxes => "Missing boxes",
            Pass::Reread => "Re-read lines (AI)",
            Pass::Transcription => "Transcriptions",
            Pass::CharBoxes => "Character boxes",
            Pass::Regions => "Text regions",
            Pass::Containers => "Text containers",
            Pass::Panels => "Panels",
            Pass::Legibility => "Legibility",
            Pass::TextType => "Text type",
            Pass::Confidence => "Confidence",
            Pass::Rest => "Everything else & approve",
            Pass::ReadingOrder => "Reading order",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Pass::LineBoxes => "Check the shape of every line box. Shift+arrows grow a side, Ctrl+arrows shrink it, Alt+arrows move; Ctrl+T takes the tighter suggestion. Delete removes a false box.",
            Pass::MissingBoxes => "Draw any line still missing (the box tool is on), or leave it for the end of the page.",
            Pass::Reread => "Read every line box of the page again with the AI model, now that the boxes are right.",
            Pass::Transcription => "Check each transcription against the image.",
            Pass::CharBoxes => "Propose, resize and accept the character boxes of each line.",
            Pass::Regions => "Group lines that belong together into text regions: select lines and press Ctrl+G, or accept the suggestions.",
            Pass::Containers => "Check the balloons, captions and signs that hold text; draw missing ones (box tool) and group them with Ctrl+G.",
            Pass::Panels => "Check the panels; draw missing ones and group their content with Ctrl+G.",
            Pass::Legibility => "How readable is each text region? A two-way answer is enough for now.",
            Pass::TextType => "What kind of text is each region?",
            Pass::Confidence => "Are you certain of this line? Refine uncertain answers later.",
            Pass::Rest => "Answer what is still open and approve each label.",
            Pass::ReadingOrder => "Now that every box exists, the reading order is suggested inside each parent; fix it with Alt+arrows and accept.",
        }
    }
    pub fn per_page(self) -> bool {
        matches!(self, Pass::MissingBoxes | Pass::Reread | Pass::Regions | Pass::Containers | Pass::Panels | Pass::ReadingOrder)
    }
    /// Shapes can be edited (and drawn on page passes) during this pass.
    pub fn edits_shapes(self) -> bool {
        matches!(self, Pass::LineBoxes | Pass::MissingBoxes | Pass::CharBoxes | Pass::Regions | Pass::Containers | Pass::Panels)
    }
    /// The level drawn during a page pass.
    pub fn draw_level(self) -> Option<Level> {
        match self {
            Pass::MissingBoxes => Some(Level::Line),
            Pass::Containers => Some(Level::Container),
            Pass::Panels => Some(Level::Panel),
            Pass::Regions => Some(Level::Region),
            _ => None,
        }
    }
}

/// How a person runs the review, saved in their own settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Workflow {
    pub passes: Vec<Pass>,
    /// Draw missing boxes at the end of the page instead of right after the line boxes; the
    /// item passes are then repeated for the new boxes only.
    #[serde(default)]
    pub missing_at_end: bool,
    /// Leave out labels whose step was already confirmed.
    #[serde(default = "yes")]
    pub skip_checked: bool,
}

fn yes() -> bool {
    true
}

impl Default for Workflow {
    fn default() -> Self {
        Workflow { passes: Pass::ALL.to_vec(), missing_at_end: false, skip_checked: true }
    }
}

impl Workflow {
    /// The passes in the order they run on a page.
    pub fn order(&self) -> Vec<Pass> {
        let mut v: Vec<Pass> = Pass::ALL.iter().copied().filter(|p| self.passes.contains(p)).collect();
        if self.missing_at_end && v.contains(&Pass::MissingBoxes) {
            v.retain(|p| *p != Pass::MissingBoxes);
            let at = v.iter().position(|p| *p == Pass::ReadingOrder).unwrap_or(v.len());
            v.insert(at, Pass::MissingBoxes);
        }
        v
    }
}

/// A review that survives restarts (stored per person in `.labeldesk/review/`): the workflow,
/// the pages in scope and the steps of the current page.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReviewSession {
    pub workflow: Workflow,
    pub pages: Vec<String>,
    /// Only these labels (e.g. the results of a search); `None` reviews everything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<BTreeSet<String>>,
    pub page: usize,
    pub items: Vec<QueueItem>,
    pub pos: usize,
    #[serde(default)]
    pub done: BTreeSet<usize>,
    /// Labels present when the page started (new ones get the item passes again).
    #[serde(default)]
    pub known: BTreeSet<String>,
    #[serde(default)]
    pub completed_pages: BTreeSet<String>,
    #[serde(default)]
    pub reviewed: usize,
    pub started: String,
    pub user: String,
}

impl ReviewSession {
    pub fn finished(&self) -> bool {
        self.page >= self.pages.len()
    }
    pub fn pass(&self) -> Option<Pass> {
        self.items.get(self.pos).map(|i| i.pass)
    }
}

/// Is the step of `pass` already confirmed for this label?
pub fn pass_checked(pass: Pass, a: &Annotation, page: &PageFile, cfg: &ProjectConfig) -> bool {
    let confirmed = |f: &str| a.checks.get(f).is_some_and(|c| matches!(c.state, CheckState::Confirmed | CheckState::NotApplicable | CheckState::Unknown));
    match pass {
        Pass::LineBoxes => confirmed("geometry"),
        Pass::Transcription => confirmed("transcription") || a.recognizability == Some(Recognizability::Unreadable),
        Pass::CharBoxes => confirmed("char_boxes") || matches!(char_status(page, a, cfg), CharStatus::Done(_)),
        Pass::Legibility => a.recognizability.is_some() && confirmed("recognizability"),
        Pass::TextType => !a.classes.is_empty() && confirmed("classes"),
        Pass::Confidence => a.confidence.transcription.is_some() || a.confidence.geometry.is_some(),
        Pass::Rest => a.review.state >= ReviewState::HumanReviewed && !a.review.needs_rereview && blocking(&needs(a, page, cfg), &cfg.quick).is_empty(),
        _ => false,
    }
}

/// Text regions, plus lines that sit in no region: the units of the per-region passes.
fn region_units<'a>(page: &'a PageFile) -> Vec<&'a Annotation> {
    let in_region = |a: &Annotation| {
        let mut cur = a.parent.as_deref();
        let mut guard = 0;
        while let (Some(p), true) = (cur, guard < 16) {
            match page.get(p) {
                Some(x) if x.level == Level::Region => return true,
                Some(x) => cur = x.parent.as_deref(),
                None => return false,
            }
            guard += 1;
        }
        false
    };
    page.annotations.iter().filter(|a| a.level == Level::Region || (a.level == Level::Line && !in_region(a))).collect()
}

/// Labels a pass visits on a page, in reading order.
pub fn pass_items(page: &PageFile, cfg: &ProjectConfig, pass: Pass, filter: Option<&BTreeSet<String>>, skip_checked: bool) -> Vec<String> {
    let mut v: Vec<&Annotation> = match pass {
        Pass::LineBoxes | Pass::Transcription | Pass::Confidence => page.annotations.iter().filter(|a| a.level == Level::Line).collect(),
        Pass::CharBoxes => page.annotations.iter().filter(|a| a.level == Level::Line && !a.text().trim().is_empty() && cfg.requirement(Level::Char, &a.classes) != Requirement::Disabled).collect(),
        Pass::Legibility | Pass::TextType => region_units(page),
        Pass::Rest => page.annotations.iter().filter(|a| a.level != Level::Char && a.level != Level::Page).collect(),
        _ => Vec::new(),
    };
    v.retain(|a| filter.is_none_or(|f| f.contains(&a.id)));
    if skip_checked {
        v.retain(|a| !pass_checked(pass, a, page, cfg));
    }
    v.sort_by_cached_key(|a| reading_key(page, a));
    v.into_iter().map(|a| a.id.clone()).collect()
}

/// Steps of the given passes on a page (page passes are skipped when a filter is set).
pub fn page_steps(page: &PageFile, cfg: &ProjectConfig, passes: &[Pass], filter: Option<&BTreeSet<String>>, skip_checked: bool) -> Vec<QueueItem> {
    let mut out = Vec::new();
    for &pass in passes {
        if pass.per_page() {
            if filter.is_none() {
                out.push(QueueItem { page_id: page.page_id.clone(), annotation_id: None, pass });
            }
            continue;
        }
        for id in pass_items(page, cfg, pass, filter, skip_checked) {
            out.push(QueueItem { page_id: page.page_id.clone(), annotation_id: Some(id), pass });
        }
    }
    out
}

/// Labels inside `id` (any depth), without `id` itself.
/// A text region whose lines (or words) carry the text, so it needs no transcription of its own.
pub fn text_in_lines(page: &PageFile, a: &Annotation) -> bool {
    a.level == Level::Region
        && descendants(page, &a.id)
            .iter()
            .filter_map(|id| page.get(id))
            .any(|x| matches!(x.level, Level::Line | Level::Word) && (!x.text().trim().is_empty() || x.recognizability == Some(Recognizability::Unreadable)))
}

pub fn descendants(page: &PageFile, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![id.to_string()];
    while let Some(p) = stack.pop() {
        for a in page.annotations.iter().filter(|a| a.parent.as_deref() == Some(p.as_str())) {
            if !out.contains(&a.id) && a.id != id {
                out.push(a.id.clone());
                stack.push(a.id.clone());
            }
        }
    }
    out
}

/// Labels that match a search kind (also used for problem lists).
pub fn matches_kind(kind: QueueKind, a: &Annotation, page: &PageFile, cfg: &ProjectConfig) -> bool {
    in_queue(kind, a, page, cfg)
}

fn in_queue(kind: QueueKind, a: &Annotation, page: &PageFile, cfg: &ProjectConfig) -> bool {
    match kind {
        QueueKind::All => true,
        QueueKind::AiUnreviewed => a.review.state == ReviewState::AiProposed && a.provenance.origin == Origin::Ai,
        QueueKind::LowConfidence => {
            let ai = a.provenance.ai.as_ref();
            ai.and_then(|x| x.recognition_score).is_some_and(|s| s < cfg.quick.low_confidence)
                || ai.and_then(|x| x.detection_score).is_some_and(|s| s < 0.5)
                || [a.confidence.transcription, a.confidence.localization, a.confidence.geometry, a.confidence.reading_order].contains(&Some(Confidence::Low))
                || matches!(a.recognizability, Some(Recognizability::PartiallyReadable | Recognizability::Ambiguous))
        }
        QueueKind::MissingFields => needs(a, page, cfg).iter().any(|n| matches!(n.kind, NeedKind::Missing | NeedKind::Invalid | NeedKind::Stale)),
        QueueKind::SuspectGeometry => {
            let outline = a.geometry.outline();
            let bad = match &a.geometry {
                Geometry::Polygon { points } => geom::self_intersects(points),
                Geometry::Quad { points } => !geom::quad_order_ok(points),
                _ => false,
            };
            let (lo, hi) = a.geometry.bounds();
            let outside = lo[0] < -0.5 || lo[1] < -0.5 || hi[0] > page.image.width as f64 + 0.5 || hi[1] > page.image.height as f64 + 0.5;
            let escapes_parent = a.parent.as_deref().and_then(|p| page.get(p)).is_some_and(|p| outline.iter().filter(|q| !p.geometry.contains(**q, 3.0)).count() >= 2);
            bad || outside || escapes_parent || stale_reason(page, a).is_some() || needs(a, page, cfg).iter().any(|n| n.field == "direction" && n.kind == NeedKind::Invalid)
        }
        QueueKind::HeuristicChars => a.provenance.origin == Origin::Heuristic && (a.review.state == ReviewState::AiProposed || stale_reason(page, a).is_some()),
        QueueKind::NeedsPeer => {
            let n = review::full_reviewers(a, &cfg.review).len() as u32;
            n >= 1 && (n < cfg.review.reviewers_required || (a.review.state == ReviewState::HumanReviewed && cfg.state_enabled(ReviewState::PeerVerified)))
        }
        QueueKind::ChangedAfterVerification => a.review.needs_rereview || a.provenance.fields.values().any(|f| *f == FieldState::ChangedAfterVerification),
    }
}

/// Sort key that follows the reading order through the hierarchy (panel, bubble, line…).
pub fn reading_key(page: &PageFile, a: &Annotation) -> Vec<(u32, i64, i64)> {
    let mut chain = Vec::new();
    let mut cur = Some(a);
    let mut guard = 0;
    while let (Some(x), true) = (cur, guard < 16) {
        let (lo, _) = x.geometry.bounds();
        chain.push((x.reading_order.unwrap_or(u32::MAX), lo[1] as i64, lo[0] as i64));
        cur = x.parent.as_deref().and_then(|p| page.get(p));
        guard += 1;
    }
    chain.reverse();
    chain
}

// ------------------------------------------------------------------ progress metrics

/// Coverage (what is annotated), completeness (required fields answered) and verification
/// (what people approved) are counted separately: a full dataset is not a verified one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    pub pages: usize,
    pub pages_covered: usize,
    pub pages_awaiting_inspection: usize,
    pub total: usize,
    pub awaiting_review: usize,
    pub incomplete_required: usize,
    pub complete_required: usize,
    pub verified_transcription: usize,
    pub verified_geometry: usize,
    pub verified_order: usize,
    pub fully_approved: usize,
    pub provisional_chars: usize,
    pub gold: usize,
}

impl Metrics {
    pub fn add(&mut self, o: &Metrics) {
        self.pages += o.pages;
        self.pages_covered += o.pages_covered;
        self.pages_awaiting_inspection += o.pages_awaiting_inspection;
        self.total += o.total;
        self.awaiting_review += o.awaiting_review;
        self.incomplete_required += o.incomplete_required;
        self.complete_required += o.complete_required;
        self.verified_transcription += o.verified_transcription;
        self.verified_geometry += o.verified_geometry;
        self.verified_order += o.verified_order;
        self.fully_approved += o.fully_approved;
        self.provisional_chars += o.provisional_chars;
        self.gold += o.gold;
    }
}

pub fn metrics(pages: &[&PageFile], cfg: &ProjectConfig) -> Metrics {
    let mut m = Metrics { pages: pages.len(), ..Default::default() };
    for p in pages {
        if p.completeness.no_text || p.annotations.iter().any(|a| a.level.is_text()) {
            m.pages_covered += 1;
        }
        if !p.completeness.inspected {
            m.pages_awaiting_inspection += 1;
        }
        for a in &p.annotations {
            m.total += 1;
            if a.review.state.needs_work() || a.review.state == ReviewState::Submitted || a.review.needs_rereview {
                m.awaiting_review += 1;
            }
            if needs(a, p, cfg).iter().any(|n| matches!(n.kind, NeedKind::Missing | NeedKind::Invalid | NeedKind::Stale)) {
                m.incomplete_required += 1;
            } else {
                m.complete_required += 1;
            }
            let v = |d: Dimension| review::valid_approvals(a).any(|x| x.dimension == d);
            m.verified_transcription += v(Dimension::Transcription) as usize;
            m.verified_geometry += v(Dimension::Geometry) as usize;
            m.verified_order += v(Dimension::ReadingOrder) as usize;
            if !review::full_reviewers(a, &cfg.review).is_empty() && !a.review.needs_rereview {
                m.fully_approved += 1;
            }
            if a.provenance.origin == Origin::Heuristic && a.review.state == ReviewState::AiProposed {
                m.provisional_chars += 1;
            }
            if a.review.gold.is_some() {
                m.gold += 1;
            }
        }
    }
    m
}

// ------------------------------------------------------------------ shape helpers

/// The same outline as a polygon (boxes and quadrilaterals keep their corners).
pub fn to_polygon(g: &Geometry) -> Geometry {
    match g {
        Geometry::Polygon { .. } => g.clone(),
        _ => Geometry::Polygon { points: g.outline().iter().map(|p| [geom::r2(p[0]), geom::r2(p[1])]).collect() },
    }
}

/// The shape as a quadrilateral: its own corners, or the tightest rotated rectangle of a polygon.
pub fn to_quad(g: &Geometry) -> Geometry {
    match g {
        Geometry::Quad { .. } => g.clone(),
        _ => match g.corners() {
            Some(q) => Geometry::Quad { points: q.map(|p| [geom::r2(p[0]), geom::r2(p[1])]) },
            None => {
                let (c, s, ang) = geom::min_area_rect(&g.outline());
                Geometry::Quad { points: geom::order_quad(geom::rect_corners(c, s, ang)).map(|p| [geom::r2(p[0]), geom::r2(p[1])]) }
            }
        },
    }
}

/// The shape as a box: rectangles keep their rotation, other shapes get their upright bounds.
pub fn to_box(g: &Geometry) -> Geometry {
    match g {
        Geometry::Box { .. } => g.clone(),
        _ => quad_to_box(&to_quad(g)).filter(|_| matches!(g, Geometry::Quad { .. })).unwrap_or_else(|| {
            let (lo, hi) = g.bounds();
            Geometry::Box { x: geom::r2(lo[0]), y: geom::r2(lo[1]), w: geom::r2(hi[0] - lo[0]), h: geom::r2(hi[1] - lo[1]), rotation_deg: 0.0 }
        }),
    }
}

/// Rectangular quadrilaterals are stored as boxes; anything else is left as it is.
pub fn normalize_shape(g: Geometry) -> Geometry {
    quad_to_box(&g).unwrap_or(g)
}

/// A box tilted by a small angle (0 < |angle| <= `max_deg`): its upright version, same centre and size.
pub fn straighten(g: &Geometry, max_deg: f64) -> Option<Geometry> {
    let Geometry::Box { x, y, w, h, rotation_deg } = g else { return None };
    (rotation_deg.abs() > 0.0 && rotation_deg.abs() <= max_deg).then(|| Geometry::Box { x: *x, y: *y, w: *w, h: *h, rotation_deg: 0.0 })
}

/// Add a corner on the edge nearest to `p` (within `tol`). Boxes and quadrilaterals become polygons.
pub fn insert_point(g: &mut Geometry, p: P, tol: f64) -> bool {
    let poly = to_polygon(g);
    let Geometry::Polygon { mut points } = poly else { return false };
    let n = points.len();
    let best = (0..n).map(|i| (i, geom::dist_to_segment(p, points[i], points[(i + 1) % n]))).min_by(|a, b| a.1.total_cmp(&b.1));
    match best {
        Some((i, d)) if d < tol => {
            points.insert(i + 1, [geom::r2(p[0]), geom::r2(p[1])]);
            *g = Geometry::Polygon { points };
            true
        }
        _ => false,
    }
}

/// A quadrilateral that is (nearly) a rectangle, as an equivalent rotated box.
pub fn quad_to_box(g: &Geometry) -> Option<Geometry> {
    let Geometry::Quad { points } = g else { return None };
    let (x, y, w, h, rotation_deg) = geom::quad_as_box(points, 2.5, 0.04)?;
    Some(Geometry::Box { x: geom::r2(x), y: geom::r2(y), w: geom::r2(w), h: geom::r2(h), rotation_deg: geom::r2(rotation_deg) })
}

/// Suggest a tighter shape around the ink inside an annotation (never larger than it is).
/// Works in the shape's own frame, so rotated boxes and quadrilaterals stay rotated.
/// Returns None when the shape already fits within `min_gain` pixels on every side.
pub fn tighten(img: &image::RgbaImage, a: &Annotation, pad: f64, min_gain: f64) -> Option<Geometry> {
    let q = a.geometry.corners()?;
    let (fw, fh) = geom::proposed_size(&q);
    if fw < 4 || fh < 4 {
        return None;
    }
    // sample the shape at most ~600 px on its long side
    let scale = (600.0 / fw.max(fh) as f64).min(1.0);
    let (w, h) = (((fw as f64 * scale).round() as u32).max(4), ((fh as f64 * scale).round() as u32).max(4));
    let flat = geom::warp_quad(img, &q, w, h)?;
    let lum = |p: &image::Rgba<u8>| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
    let mut border: Vec<f64> = Vec::new();
    for x in 0..w {
        border.push(lum(flat.get_pixel(x, 0)));
        border.push(lum(flat.get_pixel(x, h - 1)));
    }
    for y in 0..h {
        border.push(lum(flat.get_pixel(0, y)));
        border.push(lum(flat.get_pixel(w - 1, y)));
    }
    border.sort_by(|a, b| a.total_cmp(b));
    let bg = border[border.len() / 2];
    let darkest = flat.pixels().map(lum).fold(255.0, f64::min);
    let lightest = flat.pixels().map(lum).fold(0.0, f64::max);
    let contrast = (bg - darkest).max(lightest - bg);
    if contrast < 40.0 {
        return None;
    }
    let thr = (contrast * 0.45).max(35.0);
    let mut cols = vec![0u32; w as usize];
    let mut rows = vec![0u32; h as usize];
    for (x, y, p) in flat.enumerate_pixels() {
        if (lum(p) - bg).abs() > thr {
            cols[x as usize] += 1;
            rows[y as usize] += 1;
        }
    }
    // ignore specks: a column/row counts when it holds a few ink pixels
    let (cmin, rmin) = ((h / 40).max(1), (w / 40).max(1));
    let first = |v: &[u32], m: u32| v.iter().position(|c| *c >= m);
    let last = |v: &[u32], m: u32| v.iter().rposition(|c| *c >= m);
    let (u0, u1, v0, v1) = (first(&cols, cmin)?, last(&cols, cmin)?, first(&rows, rmin)?, last(&rows, rmin)?);
    let pu = pad * scale;
    let (u0, u1) = ((u0 as f64 - pu).max(0.0), (u1 as f64 + 1.0 + pu).min(w as f64));
    let (v0, v1) = ((v0 as f64 - pu).max(0.0), (v1 as f64 + 1.0 + pu).min(h as f64));
    let gain = [u0, w as f64 - u1, v0, h as f64 - v1].iter().fold(0.0f64, |m, g| m.max(*g)) / scale;
    if gain < min_gain || u1 - u0 < 2.0 || v1 - v0 < 2.0 {
        return None;
    }
    let at = |u: f64, v: f64| {
        let (u, v) = (u / w as f64, v / h as f64);
        let top = [q[0][0] + (q[1][0] - q[0][0]) * u, q[0][1] + (q[1][1] - q[0][1]) * u];
        let bot = [q[3][0] + (q[2][0] - q[3][0]) * u, q[3][1] + (q[2][1] - q[3][1]) * u];
        [geom::r2(top[0] + (bot[0] - top[0]) * v), geom::r2(top[1] + (bot[1] - top[1]) * v)]
    };
    let nq = [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)];
    Some(match &a.geometry {
        Geometry::Box { rotation_deg, .. } => {
            let (bw, bh) = (geom::dist(nq[0], nq[1]), geom::dist(nq[0], nq[3]));
            let cx = nq.iter().map(|p| p[0]).sum::<f64>() / 4.0;
            let cy = nq.iter().map(|p| p[1]).sum::<f64>() / 4.0;
            Geometry::Box { x: geom::r2(cx - bw / 2.0), y: geom::r2(cy - bh / 2.0), w: geom::r2(bw), h: geom::r2(bh), rotation_deg: *rotation_deg }
        }
        _ => Geometry::Quad { points: nq },
    })
}

/// Keep untouched character proposals fitted to their line while the line is being resized or
/// moved (real-time preview). Returns the ids that were refitted.
pub fn refit_children(page: &mut PageFile, id: &str, cfg: &ProjectConfig) -> Vec<String> {
    let Some(a) = page.get(id).cloned() else { return Vec::new() };
    let kids: Vec<(String, [usize; 2], Option<[f64; 4]>)> = page
        .annotations
        .iter()
        .filter(|c| c.level == Level::Char && c.parent.as_deref() == Some(id) && untouched_proposal(c))
        .filter_map(|c| c.provenance.heuristic.as_ref().map(|h| (c.id.clone(), h.span, h.cell)))
        .collect();
    if kids.is_empty() {
        return Vec::new();
    }
    let (frame, _) = frame_for(&a, page);
    let (props, _) = propose_chars(&a, page, &cfg.quick.chars);
    let mut out = Vec::new();
    for (cid, span, cell) in kids {
        // a remembered cell keeps its own size; otherwise fall back to a fresh proposal
        let quad = match cell {
            Some(c) => Some(cell_quad(&frame, c)),
            None => props.iter().find(|p| p.span == span).map(|p| p.quad),
        };
        if let (Some(q), Some(c)) = (quad, page.get_mut(&cid)) {
            let g = normalize_shape(Geometry::Quad { points: q });
            if c.geometry != g {
                c.geometry = g;
                out.push(cid);
            }
        }
    }
    out
}

/// Parents must be a coarser level: character < word/line < region < container < panel.
/// Levels may be skipped (a line directly in a balloon is fine).
pub fn parent_allowed(child: Level, parent: Level) -> bool {
    parent != Level::Page && parent.rank() < child.rank()
}

/// After drawing a new annotation, adopt the finer annotations it now encloses when it is a
/// tighter home for them than their current parent.
pub fn adopt_children(page: &mut PageFile, id: &str) -> usize {
    let Some(a) = page.get(id).cloned() else { return 0 };
    let area = a.geometry.area();
    let parent_area = |p: &Option<String>, page: &PageFile| p.as_deref().and_then(|p| page.get(p)).map(|p| (p.level, p.geometry.area()));
    let mut ids = Vec::new();
    for c in &page.annotations {
        if c.id == a.id || !parent_allowed(c.level, a.level) {
            continue;
        }
        let (lo, hi) = c.geometry.bounds();
        if !a.geometry.contains([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], 0.0) {
            continue;
        }
        let better = match parent_area(&c.parent, page) {
            None => true,
            Some((pl, pa)) => pl.rank() < a.level.rank() || (pl == a.level && pa > area),
        };
        if better && c.parent.as_deref() != Some(id) {
            ids.push(c.id.clone());
        }
    }
    for cid in &ids {
        if let Some(c) = page.get_mut(cid) {
            c.parent = Some(id.to_string());
        }
    }
    ids.len()
}

pub const REGION_ALGO: &str = "labeldesk.region_group";

/// A proposed text region around lines that sit close together.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionSuggestion {
    pub lines: Vec<String>,
    pub bounds: (P, P),
    pub parent: Option<String>,
}

/// Group nearby lines (same parent, same writing direction, next to each other across the
/// writing direction and overlapping along it) that are not inside a text region yet.
pub fn suggest_regions(page: &PageFile) -> Vec<RegionSuggestion> {
    let index: std::collections::HashMap<&str, &Annotation> = page.annotations.iter().map(|a| (a.id.as_str(), a)).collect();
    let lines: Vec<&Annotation> = page
        .annotations
        .iter()
        .filter(|a| a.level == Level::Line)
        .filter(|a| a.parent.as_deref().and_then(|p| index.get(p)).is_none_or(|p| p.level != Level::Region))
        .collect();
    let n = lines.len();
    let mut root: Vec<usize> = (0..n).collect();
    fn find(r: &mut Vec<usize>, i: usize) -> usize {
        let mut i = i;
        while r[i] != i {
            r[i] = r[r[i]];
            i = r[i];
        }
        i
    }
    let rects: Vec<(P, P)> = lines.iter().map(|a| a.geometry.bounds()).collect();
    let vertical: Vec<bool> = lines.iter().map(|a| a.transcription.as_ref().is_some_and(is_vertical)).collect();
    for i in 0..n {
        for j in i + 1..n {
            if lines[i].parent != lines[j].parent || vertical[i] != vertical[j] {
                continue;
            }
            let (a, b) = (rects[i], rects[j]);
            // along = writing direction, across = line-to-line direction
            let (al, ac) = if vertical[i] { (1, 0) } else { (0, 1) };
            let along = overlap_1d(a.0[al], a.1[al], b.0[al], b.1[al]);
            let short = (a.1[al] - a.0[al]).min(b.1[al] - b.0[al]).max(1.0);
            let gap = (a.0[ac].max(b.0[ac]) - a.1[ac].min(b.1[ac])).max(0.0);
            let thick = (a.1[ac] - a.0[ac]).min(b.1[ac] - b.0[ac]).max(1.0);
            if along >= 0.3 * short && gap <= 0.8 * thick {
                let (ri, rj) = (find(&mut root, i), find(&mut root, j));
                root[ri] = rj;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = find(&mut root, i);
        groups.entry(r).or_default().push(i);
    }
    groups
        .into_values()
        .filter(|g| g.len() >= 2)
        .map(|g| {
            let pts: Vec<P> = g.iter().flat_map(|&i| [rects[i].0, rects[i].1]).collect();
            let (lo, hi) = geom::bounds(&pts);
            RegionSuggestion { lines: g.iter().map(|&i| lines[i].id.clone()).collect(), bounds: ([lo[0] - 3.0, lo[1] - 3.0], [hi[0] + 3.0, hi[1] + 3.0]), parent: lines[g[0]].parent.clone() }
        })
        .collect()
}

/// Create a text region (as an unverified proposal) for a suggestion and move its lines into it.
pub fn make_region(page: &mut PageFile, s: &RegionSuggestion, user: &str, now: &str) -> String {
    let id = crate::util::new_id("a");
    let (lo, hi) = s.bounds;
    let mut r = Annotation::new(id.clone(), Level::Region, &page.page_id, Geometry::Box { x: geom::r2(lo[0]), y: geom::r2(lo[1]), w: geom::r2(hi[0] - lo[0]), h: geom::r2(hi[1] - lo[1]), rotation_deg: 0.0 }, user, now);
    r.parent = s.parent.clone();
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut texts = Vec::new();
    for lid in &s.lines {
        if let Some(l) = page.get(lid) {
            for c in &l.classes {
                *classes.entry(c.clone()).or_default() += 1;
            }
            texts.push(l.text().to_string());
        }
    }
    if let Some((c, _)) = classes.into_iter().max_by_key(|x| x.1) {
        r.classes.insert(c);
    }
    r.provenance = Provenance {
        origin: Origin::Heuristic,
        ai: None,
        fields: [("geometry".to_string(), FieldState::HeuristicProposal)].into_iter().collect(),
        heuristic: Some(HeuristicOrigin {
            algorithm: REGION_ALGO.into(),
            version: "1".into(),
            source: s.lines.first().cloned().unwrap_or_default(),
            source_text: texts.join("\n"),
            source_transcription_revision: 0,
            parent_geometry_revision: 0,
            span: [0, 0],
            frame: "axis".into(),
            cell: None,
            pinned: false,
        }),
    };
    r.review.state = ReviewState::AiProposed;
    page.annotations.push(r);
    for lid in &s.lines {
        if let Some(l) = page.get_mut(lid) {
            l.parent = Some(id.clone());
        }
    }
    id
}

// ------------------------------------------------------------------ taxonomy update

/// Labels on this page still using a retired text type (see `RETIRED_CLASSES`).
pub fn retired_labels(page: &PageFile) -> usize {
    page.annotations.iter().filter(|a| RETIRED_CLASSES.iter().any(|(old, _, _)| a.classes.contains(*old))).count()
}

/// Move retired text types to their replacement type and/or attribute. Returns labels changed.
pub fn migrate_classes(page: &mut PageFile) -> usize {
    let mut n = 0;
    for a in page.annotations.iter_mut() {
        let mut changed = false;
        for (old, new, attr) in RETIRED_CLASSES {
            if a.classes.remove(old) {
                changed = true;
                if let Some(c) = new {
                    a.classes.insert(c.to_string());
                }
                if let Some(at) = attr {
                    a.attributes.insert(at.to_string());
                }
            }
        }
        n += changed as usize;
    }
    n
}

/// Update the project's type list: drop retired types, add the new ones, keep the attributes
/// they became. Returns true when something changed.
pub fn migrate_config(cfg: &mut ProjectConfig) -> bool {
    let before = cfg.clone();
    cfg.text_classes.retain(|c| !RETIRED_CLASSES.iter().any(|(old, _, _)| c.id == *old));
    for (id, name, color) in DEFAULT_CLASSES.into_iter().filter(|c| RETIRED_CLASSES.iter().any(|r| r.1 == Some(c.0))) {
        if !cfg.text_classes.iter().any(|c| c.id == id) {
            let at = cfg.text_classes.iter().position(|c| c.id == "background").unwrap_or(cfg.text_classes.len());
            cfg.text_classes.insert(at, ClassDef { id: id.into(), name: name.into(), color });
        }
    }
    cfg.attributes.retain(|a| a != "rotated");
    for at in ["handwritten", "monospace"] {
        if !cfg.attributes.iter().any(|a| a == at) {
            cfg.attributes.insert(0, at.into());
        }
    }
    *cfg != before
}

// ------------------------------------------------------------------ grouping

/// The next coarser level a group of `level` labels goes into (skipping disabled levels).
pub fn coarser_level(level: Level, cfg: &ProjectConfig) -> Option<Level> {
    Level::DRAWABLE.iter().rev().copied().filter(|l| l.rank() < level.rank()).find(|l| cfg.levels.get(l) != Some(&Requirement::Disabled))
}

/// Group the selected labels into one new parent around them. The group takes the finest
/// coarser level than all of them, the common parent of the members, and the most common
/// text type. Returns (new id, its level, members).
pub fn group_into(page: &mut PageFile, ids: &[String], cfg: &ProjectConfig, user: &str, now: &str) -> Option<(String, Level, usize)> {
    let mut members: Vec<Annotation> = ids.iter().filter_map(|id| page.get(id).cloned()).collect();
    // members inside another member move with it
    let inside = |a: &Annotation| {
        let mut cur = a.parent.clone();
        while let Some(p) = cur {
            if ids.contains(&p) {
                return true;
            }
            cur = page.get(&p).and_then(|x| x.parent.clone());
        }
        false
    };
    members.retain(|m| !inside(m));
    if members.is_empty() {
        return None;
    }
    let coarsest = members.iter().map(|a| a.level).min_by_key(|l| l.rank())?;
    let level = coarser_level(coarsest, cfg)?;
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for a in &members {
        let (l, h) = a.geometry.bounds();
        lo = [lo[0].min(l[0]), lo[1].min(l[1])];
        hi = [hi[0].max(h[0]), hi[1].max(h[1])];
    }
    let pad = 2.0;
    let (lo, hi) = ([(lo[0] - pad).max(0.0), (lo[1] - pad).max(0.0)], [hi[0] + pad, hi[1] + pad]);
    // the closest ancestor shared by every member that may hold the new level
    let chain = |a: &Annotation| {
        let mut v = Vec::new();
        let mut cur = a.parent.clone();
        while let Some(p) = cur {
            let next = page.get(&p).and_then(|x| x.parent.clone());
            v.push(p);
            cur = next;
        }
        v
    };
    let first = chain(&members[0]);
    let parent = first.into_iter().find(|p| members[1..].iter().all(|m| chain(m).contains(p)) && page.get(p).is_some_and(|x| parent_allowed(level, x.level)));
    let id = crate::util::new_id("a");
    let mut g = Annotation::new(id.clone(), level, &page.page_id, Geometry::Box { x: geom::r2(lo[0]), y: geom::r2(lo[1]), w: geom::r2(hi[0] - lo[0]), h: geom::r2(hi[1] - lo[1]), rotation_deg: 0.0 }, user, now);
    g.parent = parent;
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    for m in &members {
        for c in &m.classes {
            *classes.entry(c.clone()).or_default() += 1;
        }
    }
    if level != Level::Panel {
        if let Some((c, _)) = classes.into_iter().max_by_key(|x| x.1) {
            g.classes.insert(c);
        }
    }
    page.annotations.push(g);
    for m in &members {
        if let Some(x) = page.get_mut(&m.id) {
            x.parent = Some(id.clone());
        }
    }
    Some((id, level, members.len()))
}

// ------------------------------------------------------------------ keyboard shape edits

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Grow (`d > 0`) or shrink (`d < 0`) one side of a shape by `d` pixels, or move the whole
/// shape (`whole`) by `d` towards that side. Boxes change in their own frame.
pub fn nudge_shape(g: &Geometry, side: Side, d: f64, whole: bool) -> Geometry {
    let (dx, dy) = match side {
        Side::Left => (-d, 0.0),
        Side::Right => (d, 0.0),
        Side::Top => (0.0, -d),
        Side::Bottom => (0.0, d),
    };
    match g {
        Geometry::Box { x, y, w, h, rotation_deg } => {
            let (mut x, mut y, mut w, mut h) = (*x, *y, *w, *h);
            if whole {
                x += dx;
                y += dy;
            } else {
                match side {
                    Side::Left => {
                        x -= d;
                        w += d;
                    }
                    Side::Right => w += d,
                    Side::Top => {
                        y -= d;
                        h += d;
                    }
                    Side::Bottom => h += d,
                }
            }
            if w < 1.0 || h < 1.0 {
                return g.clone();
            }
            Geometry::Box { x: geom::r2(x), y: geom::r2(y), w: geom::r2(w), h: geom::r2(h), rotation_deg: *rotation_deg }
        }
        _ => {
            let pts = g.outline();
            let (lo, hi) = g.bounds();
            let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
            let moved: Vec<P> = pts
                .iter()
                .map(|p| {
                    let on_side = whole
                        || match side {
                            Side::Left => p[0] < c[0],
                            Side::Right => p[0] > c[0],
                            Side::Top => p[1] < c[1],
                            Side::Bottom => p[1] > c[1],
                        };
                    if on_side { [geom::r2(p[0] + dx), geom::r2(p[1] + dy)] } else { *p }
                })
                .collect();
            match g {
                Geometry::Quad { .. } if moved.len() == 4 => Geometry::Quad { points: [moved[0], moved[1], moved[2], moved[3]] },
                _ => Geometry::Polygon { points: moved },
            }
        }
    }
}
