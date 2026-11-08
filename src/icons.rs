//! Small icons painted with lines and shapes, so they look the same on every system.

use eframe::egui::{self, Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Select,
    Pan,
    Rect,
    Polygon,
    Line,
    Point,
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Trash,
    Prev,
    Next,
    Fit,
    Plus,
    Minus,
    Check,
    Quad,
    Order,
    Link,
    Ai,
    Split,
    Merge,
    Undo,
    Redo,
}

pub fn paint(p: &egui::Painter, r: Rect, icon: Icon, c: Color32) {
    let s = Stroke::new(1.6, c);
    let at = |x: f32, y: f32| pos2(r.left() + r.width() * x, r.top() + r.height() * y);
    match icon {
        Icon::Select => {
            let pts = vec![at(0.3, 0.15), at(0.3, 0.8), at(0.45, 0.65), at(0.58, 0.88), at(0.68, 0.83), at(0.56, 0.6), at(0.76, 0.6)];
            p.add(egui::Shape::convex_polygon(pts[..3].to_vec(), c, Stroke::NONE));
            p.add(egui::Shape::closed_line(pts, s));
        }
        Icon::Pan => {
            let m = at(0.5, 0.5);
            for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
                let tip = m + d * r.width() * 0.36;
                p.line_segment([m, tip], s);
                let side = vec2(-d.y, d.x) * r.width() * 0.1;
                p.line_segment([tip, tip - d * r.width() * 0.1 + side], s);
                p.line_segment([tip, tip - d * r.width() * 0.1 - side], s);
            }
        }
        Icon::Rect => {
            let rr = Rect::from_min_max(at(0.18, 0.25), at(0.82, 0.75));
            p.rect_stroke(rr, 1.0, s, StrokeKind::Middle);
            for q in [rr.left_top(), rr.right_top(), rr.left_bottom(), rr.right_bottom()] {
                p.rect_filled(Rect::from_center_size(q, Vec2::splat(4.0)), 0.0, c);
            }
        }
        Icon::Polygon => {
            let pts = vec![at(0.5, 0.15), at(0.85, 0.42), at(0.7, 0.85), at(0.3, 0.85), at(0.15, 0.42)];
            p.add(egui::Shape::closed_line(pts.clone(), s));
            for q in pts {
                p.circle_filled(q, 2.2, c);
            }
        }
        Icon::Line => {
            let pts = vec![at(0.15, 0.75), at(0.4, 0.35), at(0.6, 0.65), at(0.85, 0.25)];
            p.add(egui::Shape::line(pts.clone(), s));
            for q in pts {
                p.circle_filled(q, 2.2, c);
            }
        }
        Icon::Point => {
            p.circle_filled(at(0.5, 0.5), r.width() * 0.13, c);
            p.circle_stroke(at(0.5, 0.5), r.width() * 0.3, s);
        }
        Icon::Eye | Icon::EyeOff => {
            let n = 14;
            let mut top = Vec::new();
            let mut bot = Vec::new();
            for i in 0..=n {
                let t = i as f32 / n as f32;
                let x = 0.12 + 0.76 * t;
                let h = (t * std::f32::consts::PI).sin() * 0.25;
                top.push(at(x, 0.5 - h));
                bot.push(at(x, 0.5 + h));
            }
            p.add(egui::Shape::line(top, s));
            p.add(egui::Shape::line(bot, s));
            p.circle_filled(at(0.5, 0.5), r.width() * 0.11, c);
            if icon == Icon::EyeOff {
                p.line_segment([at(0.18, 0.18), at(0.82, 0.82)], Stroke::new(1.8, c));
            }
        }
        Icon::Lock | Icon::Unlock => {
            let body = Rect::from_min_max(at(0.25, 0.47), at(0.75, 0.85));
            p.rect_filled(body, 2.0, c);
            let cx = if icon == Icon::Lock { 0.5 } else { 0.62 };
            let mut arc = Vec::new();
            for i in 0..=12 {
                let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                arc.push(at(cx + 0.16 * a.cos(), 0.36 + 0.2 * a.sin()));
            }
            arc.insert(0, at(cx - 0.16, 0.47));
            if icon == Icon::Lock {
                arc.push(at(cx + 0.16, 0.47));
            }
            p.add(egui::Shape::line(arc, s));
        }
        Icon::Trash => {
            p.line_segment([at(0.2, 0.28), at(0.8, 0.28)], s);
            p.line_segment([at(0.4, 0.2), at(0.6, 0.2)], s);
            let pts = vec![at(0.28, 0.28), at(0.33, 0.85), at(0.67, 0.85), at(0.72, 0.28)];
            p.add(egui::Shape::line(pts, s));
            p.line_segment([at(0.44, 0.4), at(0.45, 0.74)], s);
            p.line_segment([at(0.56, 0.4), at(0.55, 0.74)], s);
        }
        Icon::Prev => {
            p.add(egui::Shape::line(vec![at(0.6, 0.22), at(0.32, 0.5), at(0.6, 0.78)], Stroke::new(2.0, c)));
        }
        Icon::Next => {
            p.add(egui::Shape::line(vec![at(0.4, 0.22), at(0.68, 0.5), at(0.4, 0.78)], Stroke::new(2.0, c)));
        }
        Icon::Fit => {
            for (a, b, d) in [
                (at(0.18, 0.38), at(0.18, 0.18), at(0.38, 0.18)),
                (at(0.62, 0.18), at(0.82, 0.18), at(0.82, 0.38)),
                (at(0.82, 0.62), at(0.82, 0.82), at(0.62, 0.82)),
                (at(0.38, 0.82), at(0.18, 0.82), at(0.18, 0.62)),
            ] {
                p.add(egui::Shape::line(vec![a, b, d], s));
            }
        }
        Icon::Plus | Icon::Minus => {
            p.line_segment([at(0.22, 0.5), at(0.78, 0.5)], Stroke::new(2.0, c));
            if icon == Icon::Plus {
                p.line_segment([at(0.5, 0.22), at(0.5, 0.78)], Stroke::new(2.0, c));
            }
        }
        Icon::Quad => {
            let pts = vec![at(0.22, 0.25), at(0.8, 0.18), at(0.85, 0.78), at(0.15, 0.7)];
            p.add(egui::Shape::closed_line(pts.clone(), s));
            for q in pts {
                p.circle_filled(q, 2.4, c);
            }
        }
        Icon::Order => {
            p.circle_stroke(at(0.25, 0.3), r.width() * 0.14, s);
            p.circle_stroke(at(0.72, 0.72), r.width() * 0.14, s);
            p.arrow(at(0.36, 0.42), egui::vec2(r.width() * 0.22, r.height() * 0.2), s);
            p.line_segment([at(0.25, 0.22), at(0.25, 0.38)], s);
        }
        Icon::Link => {
            for (a, b) in [((0.18, 0.62), (0.48, 0.32)), ((0.52, 0.68), (0.82, 0.38))] {
                let (x0, y0) = a;
                let (x1, y1) = b;
                p.line_segment([at(x0, y0), at(x1, y1)], egui::Stroke::new(5.0, c));
                p.line_segment([at(x0, y0), at(x1, y1)], egui::Stroke::new(2.2, egui::Color32::from_rgb(40, 42, 50)));
            }
            p.line_segment([at(0.38, 0.62), at(0.62, 0.38)], s);
        }
        Icon::Ai => {
            let star = |cx: f32, cy: f32, k: f32| {
                vec![at(cx, cy - k), at(cx + k * 0.3, cy - k * 0.3), at(cx + k, cy), at(cx + k * 0.3, cy + k * 0.3), at(cx, cy + k), at(cx - k * 0.3, cy + k * 0.3), at(cx - k, cy), at(cx - k * 0.3, cy - k * 0.3)]
            };
            p.add(egui::Shape::convex_polygon(star(0.42, 0.55, 0.32), c, Stroke::NONE));
            p.add(egui::Shape::convex_polygon(star(0.76, 0.24, 0.16), c, Stroke::NONE));
        }
        Icon::Split => {
            p.rect_stroke(Rect::from_min_max(at(0.15, 0.3), at(0.45, 0.7)), 1.0, s, StrokeKind::Middle);
            p.rect_stroke(Rect::from_min_max(at(0.55, 0.3), at(0.85, 0.7)), 1.0, s, StrokeKind::Middle);
        }
        Icon::Merge => {
            p.rect_stroke(Rect::from_min_max(at(0.15, 0.3), at(0.85, 0.7)), 1.0, s, StrokeKind::Middle);
            p.line_segment([at(0.5, 0.36), at(0.5, 0.64)], egui::Stroke::new(1.0, c.gamma_multiply(0.5)));
        }
        Icon::Check => {
            p.add(egui::Shape::line(vec![at(0.2, 0.52), at(0.42, 0.74), at(0.8, 0.28)], Stroke::new(2.2, c)));
        }
        Icon::Undo | Icon::Redo => {
            let flip = icon == Icon::Redo;
            let fx = |x: f32| if flip { 1.0 - x } else { x };
            let mut arc = Vec::new();
            for i in 0..=12 {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                arc.push(at(fx(0.5 + 0.25 * a.cos()), 0.55 + 0.22 * a.sin()));
            }
            arc.insert(0, at(fx(0.22), 0.33));
            arc.push(at(fx(0.35), 0.77));
            p.add(egui::Shape::line(arc, s));
            p.add(egui::Shape::line(vec![at(fx(0.33), 0.2), at(fx(0.2), 0.33), at(fx(0.33), 0.46)], s));
        }
    }
}

/// A square button showing an icon. `active` paints it highlighted.
pub fn button(ui: &mut Ui, icon: Icon, size: f32, active: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let vis = ui.visuals();
    let accent = vis.selection.bg_fill;
    let (bg, fg) = if active {
        (accent, Color32::WHITE)
    } else if resp.hovered() {
        (vis.widgets.hovered.bg_fill, vis.strong_text_color())
    } else {
        (Color32::TRANSPARENT, vis.text_color())
    };
    let p = ui.painter();
    p.rect_filled(rect, 5.0, bg);
    let pad = size * 0.2;
    paint(p, rect.shrink(pad), icon, fg);
    resp.on_hover_text(tip)
}
