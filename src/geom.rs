//! Geometry helpers: polygons, rotated rectangles, homographies and image warping.
//!
//! Coordinate convention (also written into every project's FORMAT.md):
//! continuous pixel coordinates of the original image in its canonical orientation,
//! origin at the top-left corner of the top-left pixel, x to the right, y down.
//! Pixel (i, j) covers [i, i+1) × [j, j+1); its centre is (i + 0.5, j + 0.5).
//! Quadrilateral corners are ordered top-left, top-right, bottom-right, bottom-left
//! as read in the text's own frame (clockwise on screen).

pub type P = [f64; 2];

pub fn dist(a: P, b: P) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

pub fn signed_area(pts: &[P]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum::<f64>() * 0.5
}

pub fn bounds(pts: &[P]) -> (P, P) {
    let mut lo = [f64::MAX, f64::MAX];
    let mut hi = [f64::MIN, f64::MIN];
    for p in pts {
        lo[0] = lo[0].min(p[0]);
        lo[1] = lo[1].min(p[1]);
        hi[0] = hi[0].max(p[0]);
        hi[1] = hi[1].max(p[1]);
    }
    (lo, hi)
}

pub fn point_in_poly(p: P, pts: &[P]) -> bool {
    let n = pts.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn dist_to_segment(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn segments_intersect(p1: P, p2: P, p3: P, p4: P) -> bool {
    let d1 = cross(p3, p4, p1);
    let d2 = cross(p3, p4, p2);
    let d3 = cross(p1, p2, p3);
    let d4 = cross(p1, p2, p4);
    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

/// True when two non-adjacent edges of the closed polygon cross.
pub fn self_intersects(pts: &[P]) -> bool {
    let n = pts.len();
    if n < 4 {
        return false;
    }
    for i in 0..n {
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            if segments_intersect(pts[i], pts[(i + 1) % n], pts[j], pts[(j + 1) % n]) {
                return true;
            }
        }
    }
    false
}

/// A quadrilateral is valid when it is simple, convex-or-concave but not twisted,
/// and its corners run clockwise on screen (TL, TR, BR, BL).
pub fn quad_order_ok(q: &[P; 4]) -> bool {
    !self_intersects(q) && signed_area(q) > 0.0
}

pub fn convex_hull(mut pts: Vec<P>) -> Vec<P> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<P> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<P> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Minimum-area enclosing rectangle: (centre, (width, height), angle in radians of the width axis).
pub fn min_area_rect(pts: &[P]) -> (P, (f64, f64), f64) {
    let hull = convex_hull(pts.to_vec());
    if hull.len() < 3 {
        let (lo, hi) = bounds(pts);
        return ([(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0], (hi[0] - lo[0], hi[1] - lo[1]), 0.0);
    }
    let mut best = (f64::MAX, [0.0, 0.0], (0.0, 0.0), 0.0);
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        let ang = (b[1] - a[1]).atan2(b[0] - a[0]);
        let (c, s) = (ang.cos(), ang.sin());
        let (mut u0, mut u1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for p in &hull {
            let u = p[0] * c + p[1] * s;
            let v = -p[0] * s + p[1] * c;
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let area = (u1 - u0) * (v1 - v0);
        if area < best.0 {
            let (cu, cv) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
            best = (area, [cu * c - cv * s, cu * s + cv * c], (u1 - u0, v1 - v0), ang);
        }
    }
    (best.1, best.2, best.3)
}

/// Corners of a rotated rectangle, ordered TL, TR, BR, BL on screen.
pub fn rect_corners(center: P, size: (f64, f64), angle: f64) -> [P; 4] {
    let (c, s) = (angle.cos(), angle.sin());
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let pt = |u: f64, v: f64| [center[0] + u * c - v * s, center[1] + u * s + v * c];
    order_quad([pt(-hw, -hh), pt(hw, -hh), pt(hw, hh), pt(-hw, hh)])
}

/// Corners of a rectangle rotated by `angle` (radians, clockwise on screen), in the rectangle's own
/// TL, TR, BR, BL order (not re-sorted by screen position).
pub fn rect_corners_unordered(center: P, size: (f64, f64), angle: f64) -> [P; 4] {
    let (c, s) = (angle.cos(), angle.sin());
    let (hw, hh) = (size.0 / 2.0, size.1 / 2.0);
    let pt = |u: f64, v: f64| [center[0] + u * c - v * s, center[1] + u * s + v * c];
    [pt(-hw, -hh), pt(hw, -hh), pt(hw, hh), pt(-hw, hh)]
}

/// Reorder four points as TL, TR, BR, BL (clockwise on screen, starting nearest the top-left).
pub fn order_quad(q: [P; 4]) -> [P; 4] {
    let cx = q.iter().map(|p| p[0]).sum::<f64>() / 4.0;
    let cy = q.iter().map(|p| p[1]).sum::<f64>() / 4.0;
    let mut v = q.to_vec();
    v.sort_by(|a, b| (a[1] - cy).atan2(a[0] - cx).total_cmp(&(b[1] - cy).atan2(b[0] - cx)));
    // atan2 order with y down runs clockwise on screen starting from the left; rotate so the
    // corner with the smallest x + y comes first.
    let start = (0..4).min_by(|&i, &j| (v[i][0] + v[i][1]).total_cmp(&(v[j][0] + v[j][1]))).unwrap();
    [v[start], v[(start + 1) % 4], v[(start + 2) % 4], v[(start + 3) % 4]]
}

/// Homography H (row-major 3×3, h33 = 1) mapping src[i] → dst[i].
pub fn homography(src: &[P; 4], dst: &[P; 4]) -> Option<[f64; 9]> {
    let mut a = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (src[i][0], src[i][1]);
        let (u, v) = (dst[i][0], dst[i][1]);
        a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    // Gaussian elimination with partial pivoting
    for col in 0..8 {
        let piv = (col..8).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        for row in 0..8 {
            if row != col {
                let f = a[row][col] / a[col][col];
                for k in col..9 {
                    a[row][k] -= f * a[col][k];
                }
            }
        }
    }
    let mut h = [0.0; 9];
    for i in 0..8 {
        h[i] = a[i][8] / a[i][i];
    }
    h[8] = 1.0;
    Some(h)
}

pub fn apply_h(h: &[f64; 9], p: P) -> P {
    let w = h[6] * p[0] + h[7] * p[1] + h[8];
    [(h[0] * p[0] + h[1] * p[1] + h[2]) / w, (h[3] * p[0] + h[4] * p[1] + h[5]) / w]
}

/// If the quadrilateral is (nearly) a rectangle, return it as (top-left x, y, width, height,
/// rotation in degrees clockwise) of an equivalent rotated box, keeping its TL, TR, BR, BL order.
/// `angle_tol` is the largest corner-angle error in degrees, `side_tol` the relative side mismatch.
pub fn quad_as_box(q: &[P; 4], angle_tol: f64, side_tol: f64) -> Option<(f64, f64, f64, f64, f64)> {
    if !quad_order_ok(q) {
        return None;
    }
    for i in 0..4 {
        let (a, b, c) = (q[(i + 3) % 4], q[i], q[(i + 1) % 4]);
        let (u, v) = ([a[0] - b[0], a[1] - b[1]], [c[0] - b[0], c[1] - b[1]]);
        let cos = (u[0] * v[0] + u[1] * v[1]) / ((u[0].hypot(u[1]) * v[0].hypot(v[1])).max(1e-9));
        if (cos.clamp(-1.0, 1.0).acos().to_degrees() - 90.0).abs() > angle_tol {
            return None;
        }
    }
    let (w1, w2) = (dist(q[0], q[1]), dist(q[3], q[2]));
    let (h1, h2) = (dist(q[0], q[3]), dist(q[1], q[2]));
    if (w1 - w2).abs() > side_tol * w1.max(w2) || (h1 - h2).abs() > side_tol * h1.max(h2) {
        return None;
    }
    let (w, h) = ((w1 + w2) / 2.0, (h1 + h2) / 2.0);
    let cx = q.iter().map(|p| p[0]).sum::<f64>() / 4.0;
    let cy = q.iter().map(|p| p[1]).sum::<f64>() / 4.0;
    let mut deg = (q[1][1] - q[0][1]).atan2(q[1][0] - q[0][0]).to_degrees();
    if deg.abs() < 0.5 {
        deg = 0.0;
    }
    Some((cx - w / 2.0, cy - h / 2.0, w, h, deg))
}

/// Move edge `i` (from point i to point i+1 of a closed ring) along its normal so that it
/// passes through `p`. Both end points move by the same amount; the edge keeps its direction.
pub fn move_edge(points: &mut [P], i: usize, p: P) {
    let n = points.len();
    if n < 3 {
        return;
    }
    let (a, b) = (points[i], points[(i + 1) % n]);
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy).max(1e-9);
    let normal = [-dy / len, dx / len];
    let off = (p[0] - a[0]) * normal[0] + (p[1] - a[1]) * normal[1];
    for k in [i, (i + 1) % n] {
        points[k][0] += normal[0] * off;
        points[k][1] += normal[1] * off;
    }
}

/// Destination size proposal for flattening a quadrilateral: mean opposite edge lengths.
pub fn proposed_size(q: &[P; 4]) -> (u32, u32) {
    let w = (dist(q[0], q[1]) + dist(q[3], q[2])) / 2.0;
    let h = (dist(q[0], q[3]) + dist(q[1], q[2])) / 2.0;
    (w.round().max(1.0) as u32, h.round().max(1.0) as u32)
}

/// Bilinear sample of an RGBA image at continuous pixel coordinates.
pub fn sample(img: &image::RgbaImage, x: f64, y: f64) -> [u8; 4] {
    let (w, h) = img.dimensions();
    let fx = (x - 0.5).clamp(0.0, w as f64 - 1.0);
    let fy = (y - 0.5).clamp(0.0, h as f64 - 1.0);
    let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
    let p = |x, y| img.get_pixel(x, y).0;
    let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
    let mut out = [0u8; 4];
    for k in 0..4 {
        let top = a[k] as f64 * (1.0 - tx) + b[k] as f64 * tx;
        let bot = c[k] as f64 * (1.0 - tx) + d[k] as f64 * tx;
        out[k] = (top * (1.0 - ty) + bot * ty).round() as u8;
    }
    out
}

/// Flatten the region under `quad` (TL, TR, BR, BL) into a `w × h` picture.
/// The original image is only read, never changed.
pub fn warp_quad(img: &image::RgbaImage, quad: &[P; 4], w: u32, h: u32) -> Option<image::RgbaImage> {
    let (w, h) = (w.clamp(1, 4096), h.clamp(1, 4096));
    let dst = [[0.0, 0.0], [w as f64, 0.0], [w as f64, h as f64], [0.0, h as f64]];
    let inv = homography(&dst, quad)?;
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let s = apply_h(&inv, [x as f64 + 0.5, y as f64 + 0.5]);
            out.put_pixel(x, y, image::Rgba(sample(img, s[0], s[1])));
        }
    }
    Some(out)
}

/// Axis-aligned crop (clamped to the image).
pub fn crop(img: &image::RgbaImage, lo: P, hi: P) -> Option<image::RgbaImage> {
    let (w, h) = img.dimensions();
    let x0 = lo[0].floor().clamp(0.0, w as f64) as u32;
    let y0 = lo[1].floor().clamp(0.0, h as f64) as u32;
    let x1 = hi[0].ceil().clamp(0.0, w as f64) as u32;
    let y1 = hi[1].ceil().clamp(0.0, h as f64) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image())
}

pub fn iou(a: (P, P), b: (P, P)) -> f64 {
    let ix = (a.1[0].min(b.1[0]) - a.0[0].max(b.0[0])).max(0.0);
    let iy = (a.1[1].min(b.1[1]) - a.0[1].max(b.0[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: (P, P)| (r.1[0] - r.0[0]).max(0.0) * (r.1[1] - r.0[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Round to 2 decimals so saved files stay stable and readable.
pub fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_round_trip() {
        let src = [[10.0, 20.0], [110.0, 30.0], [100.0, 90.0], [5.0, 80.0]];
        let dst = [[0.0, 0.0], [200.0, 0.0], [200.0, 100.0], [0.0, 100.0]];
        let h = homography(&src, &dst).unwrap();
        for i in 0..4 {
            let p = apply_h(&h, src[i]);
            assert!(dist(p, dst[i]) < 1e-6);
        }
    }

    #[test]
    fn quads_and_polygons() {
        let q = [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]];
        assert!(quad_order_ok(&q));
        assert!(!quad_order_ok(&[q[0], q[3], q[2], q[1]]));
        assert!(!quad_order_ok(&[q[0], q[2], q[1], q[3]]));
        assert_eq!(order_quad([q[2], q[0], q[3], q[1]]), q);
        let bow = [[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
        assert!(self_intersects(&bow));
        let (_, (w, h), _) = min_area_rect(&[[0.0, 0.0], [4.0, 4.0], [3.0, 5.0], [-1.0, 1.0]]);
        assert!((w * h - 4.0 * 2f64.sqrt() * 2f64.sqrt()).abs() < 1e-6);
    }
}
