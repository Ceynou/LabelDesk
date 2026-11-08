//! Character n-gram language model with CTC prefix beam search, a faithful port of the
//! `ctc_lm.py` that ships with PP-OCRv6_manga v0.3 (`lm/manga_lm3.npz`). It only re-reads lines
//! containing kana, and a veto keeps the plain (greedy) reading for punctuation, repeated marks
//! and small/large kana swaps.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use unicode_normalization::UnicodeNormalization;

const BOS: char = '\u{2}';
const EOS: char = '\u{3}';
const NEG: f64 = -1e30;

fn kana(t: &str) -> bool {
    t.chars().any(|c| ('\u{3041}'..='\u{3096}').contains(&c) || ('\u{30a1}'..='\u{30fa}').contains(&c))
}

fn fold1(c: &str) -> String {
    let n: String = c.nfkc().collect();
    let mut it = n.chars();
    let (Some(ch), None) = (it.next(), it.next()) else { return n };
    if "ー―─—–‐-".contains(ch) {
        "ー".into()
    } else if "~〜～".contains(ch) {
        "〜".into()
    } else if "・…‥.".contains(ch) {
        "・".into()
    } else {
        ch.to_string()
    }
}

pub fn fold(t: &str) -> String {
    let n: String = t.nfkc().collect();
    n.chars().filter(|c| !c.is_whitespace()).map(|c| fold1(&c.to_string())).collect()
}

fn logaddexp(a: f64, b: f64) -> f64 {
    if a == b {
        return a + std::f64::consts::LN_2;
    }
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    hi + (lo - hi).exp().ln_1p()
}

fn f16_to_f64(h: u16) -> f64 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f64;
    match e {
        0 => s * m * 2f64.powi(-24),
        31 => {
            if m == 0.0 { s * f64::INFINITY } else { f64::NAN }
        }
        _ => s * (1.0 + m / 1024.0) * 2f64.powi(e - 15),
    }
}

/// A `.npy` array: its dtype description and raw little-endian data.
struct Npy {
    descr: String,
    data: Vec<u8>,
}

fn parse_npy(bytes: &[u8]) -> Result<Npy, String> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err("not a .npy file".into());
    }
    let (hlen, start) = if bytes[6] == 1 { (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10) } else { (u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize, 12) };
    let header = String::from_utf8_lossy(&bytes[start..start + hlen]).to_string();
    let descr = header.split("'descr':").nth(1).and_then(|r| r.split('\'').nth(1)).ok_or("no dtype")?.to_string();
    Ok(Npy { descr, data: bytes[start + hlen..].to_vec() })
}

pub struct CtcLm {
    vocab: Vec<String>,
    fv: Vec<String>,
    alpha: f64,
    beta: f64,
    beam: usize,
    cand: usize,
    n: usize,
    v: f64,
    a: HashMap<String, f64>,
    b: HashMap<String, f64>,
    cache: std::cell::RefCell<HashMap<(String, char), f64>>,
}

impl CtcLm {
    pub fn load(vocab: &[String], path: &Path) -> Result<Self, String> {
        let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        let mut get = |name: &str| -> Result<Npy, String> {
            let mut e = z.by_name(name).map_err(|e| format!("{name}: {e}"))?;
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            parse_npy(&buf)
        };
        let chars_n = get("chars.npy")?;
        if chars_n.descr != "<U1" {
            return Err(format!("unexpected chars dtype {}", chars_n.descr));
        }
        let chars: Vec<char> = chars_n.data.chunks_exact(4).map(|c| char::from_u32(u32::from_le_bytes([c[0], c[1], c[2], c[3]])).unwrap_or('\u{fffd}')).collect();
        let scalar = |n: Npy| -> i64 { i64::from_le_bytes(n.data[..8].try_into().unwrap_or([0; 8])) };
        let n = scalar(get("N.npy")?) as usize;
        let v = scalar(get("V.npy")?) as f64;
        let unk = |k: u64| -> String {
            let len = (k >> 60) as usize;
            (0..len).map(|j| chars.get((((k >> (12 * j)) & 4095) as usize).wrapping_sub(1)).copied().unwrap_or('\u{fffd}')).collect()
        };
        let table = |keys: Npy, vals: Npy| -> HashMap<String, f64> {
            let ks = keys.data.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap()));
            let vs = vals.data.chunks_exact(2).map(|c| f16_to_f64(u16::from_le_bytes([c[0], c[1]])).exp());
            ks.map(unk).zip(vs).collect()
        };
        let a = table(get("gk.npy")?, get("gv.npy")?);
        let b = table(get("ck.npy")?, get("cv.npy")?);
        let fv = vocab.iter().enumerate().map(|(i, s)| if i == 0 { String::new() } else { fold(s) }).collect();
        Ok(CtcLm { vocab: vocab.to_vec(), fv, alpha: 0.35, beta: 2.0, beam: 10, cand: 8, n, v, a, b, cache: Default::default() })
    }

    pub fn order(&self) -> usize {
        self.n
    }

    fn lp(&self, h: &str, w: char) -> f64 {
        let key = (h.to_string(), w);
        if let Some(v) = self.cache.borrow().get(&key) {
            return *v;
        }
        let hc: Vec<char> = h.chars().collect();
        let mut p = 1.0 / self.v;
        for n in 1..=self.n {
            let hh: String = if n > 1 { hc[hc.len().saturating_sub(n - 1)..].iter().collect() } else { String::new() };
            let Some(b) = self.b.get(&hh) else { continue };
            let mut k = hh.clone();
            k.push(w);
            p = self.a.get(&k).copied().unwrap_or(0.0) + b * p;
        }
        let r = p.max(1e-12).ln();
        self.cache.borrow_mut().insert(key, r);
        r
    }

    fn lm_seq(&self, h: &str, s: &str) -> (f64, String) {
        let mut lp = 0.0;
        let mut h: String = h.to_string();
        for ch in s.chars() {
            lp += self.lp(&h, ch);
            h.push(ch);
            let c: Vec<char> = h.chars().collect();
            h = c[c.len().saturating_sub(self.n - 1)..].iter().collect();
        }
        (lp, h)
    }

    /// Rescore one line. `probs` is `steps × vocabulary` (row-major), `greedy` the plain decode.
    pub fn decode(&self, probs: &[f32], vocab_len: usize, greedy: &str) -> String {
        if self.alpha <= 0.0 || !kana(greedy) || vocab_len == 0 {
            return greedy.to_string();
        }
        let steps = probs.len() / vocab_len;
        let k = 12.min(vocab_len);
        #[derive(Clone)]
        struct Beam {
            pb: f64,
            pnb: f64,
            lm: f64,
            h: String,
        }
        let mut beams: Vec<(Vec<usize>, Beam)> = vec![(Vec::new(), Beam { pb: 0.0, pnb: NEG, lm: 0.0, h: std::iter::repeat_n(BOS, self.n - 1).collect() })];
        for t in 0..steps {
            let row = &probs[t * vocab_len..(t + 1) * vocab_len];
            let mut idx: Vec<usize> = (0..vocab_len).collect();
            idx.select_nth_unstable_by(k - 1, |a, b| row[*b].total_cmp(&row[*a]));
            let mut top: Vec<usize> = idx[..k].to_vec();
            top.sort_by(|a, b| row[*b].total_cmp(&row[*a]));
            let lps: Vec<f64> = top.iter().map(|i| (row[*i] as f64 + 1e-12).ln()).collect();
            let cand: Vec<(usize, f64)> = top.iter().zip(&lps).take(self.cand).filter(|(_, l)| **l > -9.0).map(|(i, l)| (*i, *l)).collect();
            let pblank = top.iter().position(|i| *i == 0).map(|p| lps[p]).unwrap_or(-20.0);
            let mut nxt: Vec<(Vec<usize>, Beam)> = Vec::new();
            let mut pos: HashMap<Vec<usize>, usize> = HashMap::new();
            let mut add = |nxt: &mut Vec<(Vec<usize>, Beam)>, pref: Vec<usize>, pb: f64, pnb: f64, lm: f64, h: &str| match pos.get(&pref) {
                Some(&i) => {
                    let x = &mut nxt[i].1;
                    x.pb = logaddexp(x.pb, pb);
                    x.pnb = logaddexp(x.pnb, pnb);
                }
                None => {
                    pos.insert(pref.clone(), nxt.len());
                    nxt.push((pref, Beam { pb, pnb, lm, h: h.to_string() }));
                }
            };
            for (pref, bm) in &beams {
                let tot = logaddexp(bm.pb, bm.pnb);
                add(&mut nxt, pref.clone(), tot + pblank, NEG, bm.lm, &bm.h);
                let last = pref.last().copied();
                for &(i, l) in &cand {
                    if i == 0 {
                        continue;
                    }
                    let mut np = pref.clone();
                    np.push(i);
                    if Some(i) == last {
                        add(&mut nxt, pref.clone(), NEG, bm.pnb + l, bm.lm, &bm.h);
                        if bm.pb > NEG / 2.0 {
                            let (dl, h2) = self.lm_seq(&bm.h, &self.fv[i]);
                            add(&mut nxt, np, NEG, bm.pb + l, bm.lm + dl, &h2);
                        }
                    } else {
                        let (dl, h2) = self.lm_seq(&bm.h, &self.fv[i]);
                        add(&mut nxt, np, NEG, tot + l, bm.lm + dl, &h2);
                    }
                }
            }
            let score = |p: &Vec<usize>, b: &Beam| logaddexp(b.pb, b.pnb) + self.alpha * b.lm + self.beta * p.len() as f64;
            // stable sort, best first (like Python's sorted(..., reverse=True))
            nxt.sort_by(|x, y| score(&y.0, &y.1).total_cmp(&score(&x.0, &x.1)));
            nxt.truncate(self.beam);
            beams = nxt;
        }
        let mut eos = String::new();
        eos.push(EOS);
        let fin = |p: &Vec<usize>, b: &Beam| logaddexp(b.pb, b.pnb) + self.alpha * (b.lm + self.lp(&b.h, EOS)) + self.beta * p.len() as f64;
        let mut best = 0;
        for i in 1..beams.len() {
            if fin(&beams[i].0, &beams[i].1) > fin(&beams[best].0, &beams[best].1) {
                best = i;
            }
        }
        let out: String = beams[best].0.iter().map(|i| self.vocab[*i].as_str()).collect();
        if out != greedy { veto(greedy, &out) } else { out }
    }
}

const PROT_EXTRA: &str = "ー〜~♡♥❤…・.,!?！？、。「」『』()（）-―";
const MARKS: &str = "♡♥❤♪♫☆★〜～";

fn small_pair(x: char, y: char) -> bool {
    const SMALL: &str = "ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ";
    const LARGE: &str = "あいうえおつやゆよわアイウエオツヤユヨワ";
    let (s, l): (Vec<char>, Vec<char>) = (SMALL.chars().collect(), LARGE.chars().collect());
    s.iter().position(|c| *c == x).is_some_and(|i| l[i] == y) || l.iter().position(|c| *c == x).is_some_and(|i| s[i] == y)
}

fn prot(c: char) -> bool {
    PROT_EXTRA.contains(c) || (33..127).contains(&(c as u32))
}

/// Keep the greedy reading where the language model only changed punctuation, marks, repeats or
/// small/large kana (the "1mrs" veto of the reference implementation).
fn veto(g: &str, p: &str) -> String {
    let (gc, pc): (Vec<char>, Vec<char>) = (g.chars().collect(), p.chars().collect());
    let mut out = String::new();
    for (op, i1, i2, j1, j2) in opcodes(&gc, &pc) {
        let (gs, ps) = (&gc[i1..i2], &pc[j1..j2]);
        let both: Vec<char> = gs.iter().chain(ps).copied().collect();
        let keep_g = if op == Op::Equal {
            false
        } else if both.iter().any(|c| MARKS.contains(*c)) {
            true
        } else if both.iter().collect::<std::collections::BTreeSet<_>>().len() == 1 {
            true
        } else if gs.len() == ps.len() && gs.iter().zip(ps).all(|(x, y)| x == y || small_pair(*x, *y)) {
            true
        } else {
            both.iter().all(|c| prot(*c))
        };
        out.extend(if keep_g { gs } else { ps });
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Op {
    Equal,
    Replace,
    Delete,
    Insert,
}

/// Python difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes().
fn opcodes(a: &[char], b: &[char]) -> Vec<(Op, usize, usize, usize, usize)> {
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, c) in b.iter().enumerate() {
        b2j.entry(*c).or_default().push(j);
    }
    fn longest(a: &[char], b2j: &HashMap<char, Vec<usize>>, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (mut bi, mut bj, mut bs) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut new: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(&a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                    new.insert(j, k);
                    if k > bs {
                        bi = i + 1 - k;
                        bj = j + 1 - k;
                        bs = k;
                    }
                }
            }
            j2len = new;
        }
        (bi, bj, bs)
    }
    let mut queue = vec![(0, a.len(), 0, b.len())];
    let mut blocks = Vec::new();
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest(a, &b2j, alo, ahi, blo, bhi);
        if k > 0 {
            blocks.push((i, j, k));
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    blocks.sort();
    // merge adjacent blocks
    let mut merged: Vec<(usize, usize, usize)> = Vec::new();
    for (i, j, k) in blocks {
        if let Some(last) = merged.last_mut() {
            if last.0 + last.2 == i && last.1 + last.2 == j {
                last.2 += k;
                continue;
            }
        }
        merged.push((i, j, k));
    }
    merged.push((a.len(), b.len(), 0));
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    for (ai, bj, size) in merged {
        let tag = match (i < ai, j < bj) {
            (true, true) => Some(Op::Replace),
            (true, false) => Some(Op::Delete),
            (false, true) => Some(Op::Insert),
            _ => None,
        };
        if let Some(t) = tag {
            out.push((t, i, ai, j, bj));
        }
        i = ai + size;
        j = bj + size;
        if size > 0 {
            out.push((Op::Equal, ai, i, bj, j));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcodes_match_difflib() {
        let a: Vec<char> = "abxcd".chars().collect();
        let b: Vec<char> = "abcyd".chars().collect();
        let ops = opcodes(&a, &b);
        assert_eq!(ops, vec![(Op::Equal, 0, 2, 0, 2), (Op::Delete, 2, 3, 2, 2), (Op::Equal, 3, 4, 2, 3), (Op::Insert, 4, 4, 3, 4), (Op::Equal, 4, 5, 4, 5)]);
        assert_eq!(f16_to_f64(0x3c00), 1.0);
        assert_eq!(f16_to_f64(0xc000), -2.0);
        assert_eq!(fold("ｱ～ abc"), "ア〜abc");
        assert_eq!(veto("すごい!!", "すごい!?"), "すごい!!", "punctuation changes are vetoed");
    }
}

/// Saves real recogniser outputs and this decoder's results, so the Python reference
/// (`ctc_lm.py`) can be run on exactly the same inputs.
#[cfg(test)]
mod reference_dump {
    #[test]
    fn dump_cases_for_reference_check() {
        let Some(c) = crate::ocr::find_candidates().into_iter().find(|c| c.lm.is_some()) else {
            eprintln!("v0.3 with LM not cached; skipping");
            return;
        };
        let mut ocr = crate::ocr::PpOcrManga::load(&c, None).unwrap();
        use crate::ocr::OcrAdapter;
        let dir = std::env::temp_dir().join("labeldesk-lm-cases");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut n = 0;
        let mut meta = Vec::new();
        for p in crate::sample::pages() {
            for d in ocr.detect(&p.image).unwrap() {
                let (lo, hi) = crate::geom::bounds(&d.quad);
                let Some(crop) = crate::geom::crop(&p.image, lo, hi) else { continue };
                for (kind, img) in [("clean", crop.clone()), ("blur", image::imageops::blur(&crop, 2.2)), ("small", {
                    let (w, h) = crop.dimensions();
                    let s = image::imageops::resize(&crop, (w / 3).max(4), (h / 3).max(4), image::imageops::FilterType::Triangle);
                    image::imageops::resize(&s, w, h, image::imageops::FilterType::Nearest)
                })] {
                    let r = ocr.recognize(&img).unwrap();
                    let v = ocr.vocab_len();
                    let bytes: Vec<u8> = ocr.last_probs.iter().flat_map(|x| x.to_le_bytes()).collect();
                    std::fs::write(dir.join(format!("{n}.bin")), bytes).unwrap();
                    meta.push(serde_json::json!({"id": n, "kind": kind, "v": v, "greedy": ocr.last_greedy, "rust": r.text}));
                    n += 1;
                }
            }
        }
        std::fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta).unwrap()).unwrap();
        let changed = meta.iter().filter(|m| m["greedy"] != m["rust"]).count();
        eprintln!("LMDUMP {n} cases in {}, LM changed {changed}", dir.display());
    }
}
