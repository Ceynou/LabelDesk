use super::*;
use eframe::egui::{Align2, CursorIcon, FontId, PointerButton, Pos2, Sense, StrokeKind, pos2};

#[derive(Clone, Copy)]
pub struct Xf {
    origin: Pos2,
    offset: Vec2,
    zoom: f32,
}

impl Xf {
    pub fn s(&self, p: P) -> Pos2 {
        self.origin + self.offset + vec2(p[0] as f32, p[1] as f32) * self.zoom
    }
    pub fn i(&self, s: Pos2) -> P {
        let v = (s - self.origin - self.offset) / self.zoom;
        [v.x as f64, v.y as f64]
    }
}

pub fn text_on(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 { Color32::from_rgb(20, 20, 24) } else { Color32::WHITE }
}

/// Split a simple polygon into triangles (ear clipping) so concave outlines fill correctly.
pub fn triangulate(pts: &[Pos2]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    let area: f32 = (0..n).map(|i| pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y).sum();
    if area < 0.0 {
        idx.reverse();
    }
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let mut tris = Vec::new();
    while idx.len() > 3 {
        let m = idx.len();
        let mut found = false;
        for i in 0..m {
            let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i], idx[(i + 1) % m]);
            let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let inside = idx.iter().any(|&j| j != ia && j != ib && j != ic && cross(a, b, pts[j]) > 0.0 && cross(b, c, pts[j]) > 0.0 && cross(c, a, pts[j]) > 0.0);
            if inside {
                continue;
            }
            tris.push([ia, ib, ic]);
            idx.remove(i);
            found = true;
            break;
        }
        if !found {
            break;
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

fn fill_poly(painter: &egui::Painter, pts: &[Pos2], c: Color32) {
    let mut mesh = egui::Mesh::default();
    for p in pts {
        mesh.colored_vertex(*p, c);
    }
    for t in triangulate(pts) {
        mesh.add_triangle(t[0] as u32, t[1] as u32, t[2] as u32);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn outline(painter: &egui::Painter, pts: &[Pos2], stroke: Stroke, dashed: bool) {
    if dashed {
        let mut closed = pts.to_vec();
        closed.push(pts[0]);
        painter.extend(egui::Shape::dashed_line(&closed, stroke, 7.0, 4.0));
    } else {
        painter.add(egui::Shape::closed_line(pts.to_vec(), stroke));
    }
}

pub fn ann_color(cfg: &ProjectConfig, a: &Annotation) -> Color32 {
    a.classes.iter().next().and_then(|c| cfg.class(c)).filter(|_| a.level != Level::Panel).map(|c| rgb(c.color)).unwrap_or_else(|| level_color(a.level))
}

/// Move corner `k` of a quadrilateral by `d`: distort (mode 0) moves it alone; skew (1) moves
/// the other corner of the same top/bottom edge with it; perspective (2) moves that corner the
/// opposite way horizontally, like an image editor's free transform.
pub fn transform_quad(orig: [P; 4], k: usize, d: P, mode: u8) -> [P; 4] {
    let mut q = orig;
    let nb = [1, 0, 3, 2][k.min(3)];
    q[k] = [geom::r2(q[k][0] + d[0]), geom::r2(q[k][1] + d[1])];
    match mode {
        1 => q[nb] = [geom::r2(q[nb][0] + d[0]), geom::r2(q[nb][1] + d[1])],
        2 => q[nb] = [geom::r2(q[nb][0] - d[0]), geom::r2(q[nb][1] + d[1])],
        _ => {}
    }
    q
}

/// Move a box handle (corner, edge or rotation) to `cur`, keeping the opposite side fixed.
pub fn apply_handle(a: &mut Annotation, h: Handle, cur: P, snap: bool) {
    match (&mut a.geometry, h) {
        (Geometry::Box { x, y, w, h: bh, rotation_deg }, Handle::BoxH(i)) => {
            let c = [*x + *w / 2.0, *y + *bh / 2.0];
            let r = rotation_deg.to_radians();
            let (cs, sn) = (r.cos(), r.sin());
            let d = [cur[0] - c[0], cur[1] - c[1]];
            let (u, v) = (d[0] * cs + d[1] * sn, -d[0] * sn + d[1] * cs);
            let (mut lo, mut hi) = ([-*w / 2.0, -*bh / 2.0], [*w / 2.0, *bh / 2.0]);
            match i {
                0 => lo = [u, v],
                1 => {
                    hi[0] = u;
                    lo[1] = v;
                }
                2 => hi = [u, v],
                3 => {
                    lo[0] = u;
                    hi[1] = v;
                }
                4 => lo[1] = v,
                5 => hi[0] = u,
                6 => hi[1] = v,
                _ => lo[0] = u,
            }
            let (l, hh) = ([lo[0].min(hi[0]), lo[1].min(hi[1])], [lo[0].max(hi[0]), lo[1].max(hi[1])]);
            let m = [(l[0] + hh[0]) / 2.0, (l[1] + hh[1]) / 2.0];
            let nc = [c[0] + m[0] * cs - m[1] * sn, c[1] + m[0] * sn + m[1] * cs];
            *w = (hh[0] - l[0]).max(1.0);
            *bh = (hh[1] - l[1]).max(1.0);
            *x = nc[0] - *w / 2.0;
            *y = nc[1] - *bh / 2.0;
        }
        (Geometry::Box { x, y, w, h: bh, rotation_deg }, Handle::Rotate) => {
            let c = [*x + *w / 2.0, *y + *bh / 2.0];
            let mut deg = (cur[1] - c[1]).atan2(cur[0] - c[0]).to_degrees() + 90.0;
            if snap {
                deg = (deg / 15.0).round() * 15.0;
            }
            while deg > 180.0 {
                deg -= 360.0;
            }
            while deg <= -180.0 {
                deg += 360.0;
            }
            *rotation_deg = deg;
        }
        (Geometry::Quad { points }, Handle::Corner(i) | Handle::Edge(i)) => {
            let old = *points;
            if matches!(h, Handle::Edge(_)) {
                geom::move_edge(points, i, cur);
            } else {
                points[i] = cur;
            }
            let new = *points;
            if let Some(r) = &mut a.rectification {
                if r.role == QuadRole::TextRegion && r.source == old {
                    r.source = new;
                    if !r.target_size_manual {
                        let (w, h) = geom::proposed_size(&new);
                        r.target_size = [w, h];
                    }
                    r.update_cache();
                }
            }
        }
        (Geometry::Polygon { points }, Handle::Vertex(i)) => {
            if let Some(p) = points.get_mut(i) {
                *p = cur;
            }
        }
        (Geometry::Polygon { points }, Handle::Edge(i)) => {
            if i < points.len() {
                geom::move_edge(points, i, cur);
            }
        }
        (_, Handle::Rect(i)) => {
            if let Some(r) = &mut a.rectification {
                r.source[i] = cur;
                if !r.target_size_manual {
                    let (w, h) = geom::proposed_size(&r.source);
                    r.target_size = [w, h];
                }
                r.update_cache();
            }
        }
        _ => {}
    }
}

impl App {
    /// Cell borders chosen for the selected line's preview, if they still fit its text.
    pub fn preview_splits(&self, id: &str) -> Option<crate::assist::Splits> {
        let (lid, text, s) = self.cell_splits.as_ref()?;
        let a = self.ann(id)?;
        (lid == id && a.text() == text).then(|| s.clone())
    }

    /// Draggable borders between the character cells of the selected line: previewed cells, or
    /// existing character boxes that remember their cell.
    pub fn cell_borders(&self) -> Vec<(crate::assist::CellBorder, P, P)> {
        if self.selection.len() != 1 {
            return Vec::new();
        }
        let (Some(a), Some(page)) = (self.primary(), self.cur()) else { return Vec::new() };
        if !matches!(a.level, Level::Line | Level::Word | Level::Region) || a.text().is_empty() {
            return Vec::new();
        }
        let cfg = self.cfg();
        if crate::assist::chars_of(page, &a.id).is_empty() {
            if cfg.requirement(Level::Char, &a.classes) == Requirement::Disabled {
                return Vec::new();
            }
            crate::assist::preview_borders(a, page, &cfg.quick.chars, self.preview_splits(&a.id).as_ref())
        } else {
            crate::assist::child_borders(page, a)
        }
    }

    /// Where a border's grip sits: a tick just outside the line's edge, so dragging inside the
    /// line still moves the line.
    pub fn border_grip(&self, a: P, b: P) -> P {
        let len = geom::dist(a, b).max(1e-9);
        let off = 9.0 / self.view.zoom as f64;
        [b[0] + (b[0] - a[0]) / len * off, b[1] + (b[1] - a[1]) / len * off]
    }

    fn border_at(&self, p: P, tol: f64) -> Option<(crate::assist::CellBorder, P, P)> {
        self.cell_borders()
            .into_iter()
            .map(|b| (geom::dist(p, self.border_grip(b.1, b.2)), b))
            .filter(|(d, _)| *d < tol)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|x| x.1)
    }

    /// Move a cell border to the pointer.
    fn drag_border(&mut self, b: &crate::assist::CellBorder, p: P) {
        let Some(id) = self.selection.first().cloned() else { return };
        let (Some(line), Some(page)) = (self.ann(&id).cloned(), self.cur()) else { return };
        let Some(x) = crate::assist::progression_at(&line, page, p) else { return };
        match b {
            crate::assist::CellBorder::Preview { row, idx } => {
                let t = line.transcription.clone().unwrap_or_default();
                let mut s = self.preview_splits(&id).unwrap_or_else(|| crate::assist::default_splits(&t, &crate::assist::policy_for(&line, &self.cfg().quick.chars)));
                crate::assist::move_split(&mut s, *row, *idx, x);
                self.cell_splits = Some((id, t.text, s));
            }
            crate::assist::CellBorder::Pair { first, second } => {
                let (f, s2) = (first.clone(), second.clone());
                if let Some(p) = self.cur_mut() {
                    crate::assist::move_pair_border(p, &id, &f, &s2, x);
                }
            }
        }
    }

    /// Double-sided arrow for a movement direction on screen.
    fn arrow_for(dir: [f64; 2]) -> CursorIcon {
        let mut deg = dir[1].atan2(dir[0]).to_degrees();
        if deg < 0.0 {
            deg += 180.0;
        }
        match deg {
            d if !(22.5..157.5).contains(&d) => CursorIcon::ResizeHorizontal,
            d if d < 67.5 => CursorIcon::ResizeNwSe,
            d if d < 112.5 => CursorIcon::ResizeVertical,
            _ => CursorIcon::ResizeNeSw,
        }
    }

    /// Double-sided arrow pointing the way the handle moves, following the shape's rotation.
    fn resize_cursor(&self, h: Handle) -> CursorIcon {
        if h == Handle::Rotate {
            return CursorIcon::Alias;
        }
        let Some(a) = self.primary() else { return CursorIcon::Crosshair };
        let o = a.geometry.outline();
        let n = o.len();
        let (lo, hi) = a.geometry.bounds();
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let dir = match (h, &a.geometry) {
            (Handle::BoxH(i), _) if i >= 4 => {
                let k = (i - 4) as usize;
                let (p, q) = (o[k], o[(k + 1) % 4]);
                [-(q[1] - p[1]), q[0] - p[0]]
            }
            (Handle::BoxH(i), _) => [o[i as usize][0] - c[0], o[i as usize][1] - c[1]],
            (Handle::Edge(i), _) if n > 1 => {
                let (p, q) = (o[i % n], o[(i + 1) % n]);
                [-(q[1] - p[1]), q[0] - p[0]]
            }
            (Handle::Corner(i) | Handle::Vertex(i), _) if i < n => [o[i][0] - c[0], o[i][1] - c[1]],
            (Handle::Rect(i), _) => a.rectification.as_ref().map(|r| [r.source[i][0] - c[0], r.source[i][1] - c[1]]).unwrap_or([1.0, 1.0]),
            _ => [1.0, 1.0],
        };
        // fold the direction into 0..180 degrees (y points down on screen)
        let mut deg = dir[1].atan2(dir[0]).to_degrees();
        if deg < 0.0 {
            deg += 180.0;
        }
        match deg {
            d if !(22.5..157.5).contains(&d) => CursorIcon::ResizeHorizontal,
            d if d < 67.5 => CursorIcon::ResizeNwSe,
            d if d < 112.5 => CursorIcon::ResizeVertical,
            _ => CursorIcon::ResizeNeSw,
        }
    }

    /// While a line is resized or moved, keep its untouched character proposals fitted to it.
    pub fn refit_live(&mut self, ids: &[String]) {
        let cfg = self.cfg();
        let Some(page) = self.cur_mut() else { return };
        let mut done = Vec::new();
        for id in ids {
            done.extend(crate::assist::refit_children(page, id, &cfg));
        }
        self.refit.extend(done);
    }

    pub fn visible(&self, a: &Annotation) -> bool {
        self.layers.get(&a.level).copied().unwrap_or(true) && !self.hidden.contains(&a.id)
    }

    /// Level of detail: is this shape drawn (and clickable) at the current zoom?
    pub fn shape_shown(&self, a: &Annotation, sr: &Rect, related: &std::collections::HashSet<&str>) -> bool {
        if a.level != Level::Char {
            return true;
        }
        let near = related.contains(a.id.as_str()) || a.parent.as_deref().is_some_and(|p| related.contains(p));
        match self.chars {
            ShowChars::Always => true,
            ShowChars::SelectedLine => near,
            ShowChars::Auto => near || sr.width().min(sr.height()) >= 12.0,
        }
    }

    fn hit_test(&self, p: P, tol: f64) -> Option<String> {
        let page = self.cur()?;
        let related: std::collections::HashSet<&str> = self.selection.iter().map(|x| x.as_str()).collect();
        let zoom = self.view.zoom;
        let focus = self.review_focus();
        page.annotations
            .iter()
            .filter(|a| self.visible(a) && a.geometry.contains(p, tol))
            .filter(|a| focus.as_ref().is_none_or(|f| f.contains(&a.id)))
            .filter(|a| {
                let (lo, hi) = a.geometry.bounds();
                let sr = Rect::from_min_size(Pos2::ZERO, vec2((hi[0] - lo[0]) as f32 * zoom, (hi[1] - lo[1]) as f32 * zoom));
                self.shape_shown(a, &sr, &related)
            })
            .min_by(|a, b| a.geometry.area().total_cmp(&b.geometry.area()))
            .map(|a| a.id.clone())
    }

    pub fn handles(&self, a: &Annotation, zoom: f32) -> Vec<(Handle, P)> {
        let mut v = Vec::new();
        if self.tab == Tab::Flatten {
            if let Some(r) = &a.rectification {
                // in the flatten tab the corners edit the flattening, never the shape itself
                return r.source.iter().enumerate().map(|(i, p)| (Handle::Rect(i), *p)).collect();
            }
        }
        match &a.geometry {
            Geometry::Box { x, y, w, h, rotation_deg } => {
                let c = [x + w / 2.0, y + h / 2.0];
                let r = rotation_deg.to_radians();
                let q = geom::rect_corners_unordered(c, (*w, *h), r);
                let mid = |a: P, b: P| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                for (i, p) in q.iter().enumerate() {
                    v.push((Handle::BoxH(i as u8), *p));
                }
                for (i, (a, b)) in [(0, 1), (1, 2), (2, 3), (3, 0)].iter().enumerate() {
                    v.push((Handle::BoxH(4 + i as u8), mid(q[*a], q[*b])));
                }
                let top = mid(q[0], q[1]);
                let off = 26.0 / zoom as f64;
                v.push((Handle::Rotate, [top[0] + off * r.sin(), top[1] - off * r.cos()]));
            }
            Geometry::Quad { points } => {
                v.extend(points.iter().enumerate().map(|(i, p)| (Handle::Corner(i), *p)));
                if self.edge_handles {
                    v.extend((0..4).map(|i| (Handle::Edge(i), [(points[i][0] + points[(i + 1) % 4][0]) / 2.0, (points[i][1] + points[(i + 1) % 4][1]) / 2.0])));
                }
            }
            Geometry::Polygon { points } => {
                v.extend(points.iter().enumerate().map(|(i, p)| (Handle::Vertex(i), *p)));
                let n = points.len();
                if self.edge_handles && n <= 64 {
                    v.extend((0..n).map(|i| (Handle::Edge(i), [(points[i][0] + points[(i + 1) % n][0]) / 2.0, (points[i][1] + points[(i + 1) % n][1]) / 2.0])));
                }
            }
        }
        if let Some(r) = &a.rectification {
            let linked = matches!(&a.geometry, Geometry::Quad { points } if *points == r.source);
            if !linked && (self.tab == Tab::Flatten || r.role == QuadRole::Surface) {
                v.extend(r.source.iter().enumerate().map(|(i, p)| (Handle::Rect(i), *p)));
            }
        }
        v
    }

    fn handle_at(&self, p: P, tol: f64, zoom: f32) -> Option<(String, Handle)> {
        if self.selection.len() != 1 || !self.review_edits() {
            return None;
        }
        let a = self.primary()?;
        if !self.visible(a) {
            return None;
        }
        let hs = self.handles(a, zoom);
        if let Some((h, _)) = hs.iter().filter(|(_, hp)| geom::dist(*hp, p) < tol).min_by(|x, y| geom::dist(x.1, p).total_cmp(&geom::dist(y.1, p))) {
            return Some((a.id.clone(), *h));
        }
        // anywhere along an edge resizes too, not only its middle handle
        let o = a.geometry.outline();
        let n = o.len();
        let edge = (0..n).map(|i| (i, geom::dist_to_segment(p, o[i], o[(i + 1) % n]))).filter(|(_, d)| *d < tol * 0.8).min_by(|x, y| x.1.total_cmp(&y.1))?;
        let h = match &a.geometry {
            Geometry::Box { .. } => Handle::BoxH(4 + edge.0 as u8),
            _ if self.edge_handles => Handle::Edge(edge.0),
            _ => return None,
        };
        Some((a.id.clone(), h))
    }

    fn assign_order(&mut self, id: &str, ctx: &egui::Context) {
        let Some(a) = self.ann(id).cloned() else { return };
        let fresh = self.order_parent.as_ref() != Some(&a.parent);
        if fresh {
            self.order_parent = Some(a.parent.clone());
            self.order_next = 1;
        }
        let n = self.order_next;
        let level = a.level;
        let parent = a.parent.clone();
        let id = id.to_string();
        self.commit();
        if let Some(page) = self.cur_mut() {
            if fresh {
                for x in page.annotations.iter_mut().filter(|x| x.parent == parent && x.level == level) {
                    x.reading_order = None;
                }
            }
            if let Some(x) = page.get_mut(&id) {
                x.reading_order = Some(n);
            }
        }
        self.commit();
        self.order_next += 1;
        self.flash(format!("Reading order {n} set. Keep clicking in reading order; Esc to stop."), ctx);
    }

    fn link_to(&mut self, target: &str, ctx: &egui::Context) {
        if self.selection.is_empty() {
            self.error("Select the annotations to link first, then click their parent with the link tool.");
            return;
        }
        if self.selection.iter().any(|s| s == target) {
            return;
        }
        if self.link_relation.is_none() {
            let tl = self.ann(target).map(|a| a.level);
            let bad = self.selection.iter().filter_map(|id| self.ann(id)).any(|a| tl.is_some_and(|t| !crate::assist::parent_allowed(a.level, t)));
            if bad {
                self.error("A parent must be a coarser level: character › line › text region › text container › panel.");
                return;
            }
        }
        // refuse loops
        let mut cur = Some(target.to_string());
        let mut guard = 0;
        while let Some(c) = cur {
            if self.selection.contains(&c) {
                self.error("That link would make a loop.");
                return;
            }
            cur = self.ann(&c).and_then(|a| a.parent.clone());
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        let sel = self.selection.clone();
        let rel = self.link_relation.clone();
        let target = target.to_string();
        self.commit();
        if let Some(page) = self.cur_mut() {
            for id in &sel {
                if let Some(a) = page.get_mut(id) {
                    match &rel {
                        None => a.parent = Some(target.clone()),
                        Some(kind) => {
                            if kind == crate::assist::FURIGANA_OF {
                                // furigana reads with its base and leaves the reading sequence
                                a.relations.retain(|r| r.kind != crate::assist::FURIGANA_OF);
                                a.attributes.insert("furigana".into());
                                a.reading_order = None;
                            }
                            if !a.relations.iter().any(|r| &r.kind == kind && r.target == target) {
                                a.relations.push(Relation { kind: kind.clone(), target: target.clone() });
                            }
                        }
                    }
                }
            }
        }
        self.commit();
        self.flash(match &self.link_relation {
            None => format!("Linked {} annotation(s) to their parent", sel.len()),
            Some(k) => format!("Added “{k}” link"),
        }, ctx);
    }

    fn canvas_menu(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let n = self.selection.len();
        if n == 0 {
            if ui.button("Run AI on this page").clicked() {
                let p = self.page;
                self.ai_job(p, ai::JobKind::Page);
                ui.close();
            }
            if ui.button("Mark page inspected and complete").clicked() {
                self.mark_inspected(true, false);
                ui.close();
            }
            return;
        }
        ui.label(egui::RichText::new(format!("{n} selected")).color(MUTED));
        let any_ai = self.selection.iter().any(|id| self.ann(id).is_some_and(|a| a.review.state == ReviewState::AiProposed));
        if any_ai {
            if ui.button("Accept AI proposal   Y").clicked() {
                self.accept_ai();
                ui.close();
            }
            if ui.button("Reject AI proposal   X").clicked() {
                self.reject_ai("rejected from the canvas menu");
                ui.close();
            }
        }
        if ui.button("Submit for review").clicked() {
            self.submit();
            ui.close();
        }
        if ui.button("Approve all aspects").clicked() {
            self.approve(&Dimension::ALL, &ctx);
            ui.close();
        }
        ui.separator();
        ui.menu_button("Change level", |ui| {
            for l in Level::DRAWABLE {
                if ui.button(l.name()).clicked() {
                    let sel = self.selection.clone();
                    self.commit();
                    if let Some(p) = self.cur_mut() {
                        for id in &sel {
                            if let Some(a) = p.get_mut(id) {
                                a.level = l;
                            }
                        }
                    }
                    self.commit();
                    ui.close();
                }
            }
        });
        if ui.button("Read again with AI (flattened)").clicked() {
            self.reread_selected();
            ui.close();
        }
        if ui.button("Split   S").clicked() {
            self.split_selection(&ctx);
            ui.close();
        }
        if ui.add_enabled(n >= 2, egui::Button::new("Merge   M")).clicked() {
            self.merge_selection(&ctx);
            ui.close();
        }
        if ui.button("Remove parent link").clicked() {
            let sel = self.selection.clone();
            self.commit();
            if let Some(p) = self.cur_mut() {
                for id in &sel {
                    if let Some(a) = p.get_mut(id) {
                        a.parent = None;
                    }
                }
            }
            self.commit();
            ui.close();
        }
        ui.separator();
        if ui.button("Delete   Del").clicked() {
            self.delete_selection();
            ui.close();
        }
    }

    pub(super) fn canvas(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, CANVAS_BG);
        self.view_center = rect.size() / 2.0;
        self.canvas_rect = rect;
        let Some(page_id) = self.cur_id() else {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Import pictures to start (File → Import pictures)", FontId::proportional(16.0), MUTED);
            return;
        };
        let (iw, ih) = self.cur().map(|p| (p.image.width as f64, p.image.height as f64)).unwrap();
        if self.view.fitted {
            let z = ((rect.width() - 40.0) / iw as f32).min((rect.height() - 40.0) / ih as f32).max(0.01);
            self.view.zoom = z;
            self.view.offset = (rect.size() - vec2(iw as f32, ih as f32) * z) / 2.0;
        }
        if self.review.is_some() {
            self.review_frame(rect, iw, ih);
        }
        if std::mem::take(&mut self.center_selection) {
            if let Some(a) = self.primary() {
                let (lo, hi) = a.geometry.bounds();
                let c = vec2(((lo[0] + hi[0]) / 2.0) as f32, ((lo[1] + hi[1]) / 2.0) as f32);
                self.view.offset = rect.size() / 2.0 - c * self.view.zoom;
                self.view.fitted = false;
            }
        }

        // ---- zoom and pan ----
        // (in review mode Space switches views instead of panning)
        let space = ui.input(|i| i.key_down(Key::Space)) && !ctx.egui_wants_keyboard_input() && self.review.is_none();
        let (shift, alt, cmd) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt, i.modifiers.command));
        // Shift or Ctrl adds to / removes from the selection
        let multi = shift || cmd;
        if let Some(pp) = resp.hover_pos() {
            let (scroll, zd) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let f = zd * (scroll * 0.0025).exp();
            if (f - 1.0).abs() > 1e-4 {
                self.zoom_by(f, Some(pp - rect.min));
            }
        }
        let xf = Xf { origin: rect.min, offset: self.view.offset, zoom: self.view.zoom };
        let tol = 6.0 / xf.zoom as f64;
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let cur = pointer.map(|p| xf.i(p));
        let clamp = |c: P| [c[0].clamp(0.0, iw), c[1].clamp(0.0, ih)];

        if resp.drag_started_by(PointerButton::Primary) && !space && self.tool != Tool::Pan {
            let start = ui.input(|i| i.pointer.press_origin()).map(|p| xf.i(p)).or(cur).unwrap_or([0.0, 0.0]);
            self.drag = Drag::None;
            if self.draft.is_empty() {
                if let Some((id, h)) = self.handle_at(start, tol * 1.4, xf.zoom) {
                    // Ctrl on a corner: free transform like an image editor (Ctrl distorts one
                    // corner, Ctrl+Shift skews its edge, Ctrl+Alt changes the perspective)
                    let corner = match h {
                        Handle::BoxH(k) if k < 4 => Some(k as usize),
                        Handle::Corner(k) => Some(k),
                        _ => None,
                    };
                    let quad = self.ann(&id).filter(|a| matches!(a.geometry, Geometry::Box { .. } | Geometry::Quad { .. })).map(|a| crate::assist::to_quad(&a.geometry));
                    match (corner, quad) {
                        (Some(k), Some(Geometry::Quad { points })) if cmd => {
                            if let Some(a) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
                                a.geometry = Geometry::Quad { points };
                            }
                            self.keep_form.insert(id.clone());
                            let mode = if shift { 1 } else if alt { 2 } else { 0 };
                            self.drag = Drag::Transform { id, corner: k, start, orig: points, mode };
                        }
                        _ => self.drag = Drag::Handle { id, handle: h },
                    }
                } else if self.review_edits() {
                    if let Some((b, _, _)) = self.border_at(start, tol * 1.2) {
                        self.drag = Drag::Cell(b);
                    }
                }
            }
            if matches!(self.drag, Drag::None) {
                self.drag = match self.tool {
                    Tool::Select => match self.hit_test(start, tol) {
                        Some(id) => {
                            if multi {
                                if !self.selection.contains(&id) {
                                    self.selection.push(id);
                                }
                            } else if !self.selection.contains(&id) {
                                self.selection = vec![id];
                            }
                            if self.review_edits() { Drag::Move { last: start } } else { Drag::None }
                        }
                        None => {
                            if !multi {
                                self.selection.clear();
                            }
                            Drag::Marquee { start }
                        }
                    },
                    Tool::Box | Tool::AiRegion => Drag::NewShape { start: clamp(start) },
                    Tool::Quad if self.draft.is_empty() => Drag::NewShape { start: clamp(start) },
                    _ => Drag::None,
                };
            }
        }
        let panning = resp.dragged_by(PointerButton::Middle) || (resp.dragged_by(PointerButton::Primary) && (space || self.tool == Tool::Pan || matches!(self.drag, Drag::Pan)));
        if panning {
            self.view.offset += resp.drag_delta();
            self.view.fitted = false;
        } else if resp.dragged_by(PointerButton::Primary) {
            if let Some(c) = cur {
                match &mut self.drag {
                    Drag::Move { last } => {
                        let d = [c[0] - last[0], c[1] - last[1]];
                        *last = c;
                        let sel = self.selection.clone();
                        if let Some(p) = self.cur_mut() {
                            for id in &sel {
                                if let Some(a) = p.get_mut(id) {
                                    a.geometry.translate(d);
                                    if let Some(r) = &mut a.rectification {
                                        for q in r.source.iter_mut() {
                                            q[0] += d[0];
                                            q[1] += d[1];
                                        }
                                        r.update_cache();
                                    }
                                }
                            }
                        }
                        self.refit_live(&sel);
                    }
                    Drag::Handle { id, handle } => {
                        let (id, h) = (id.clone(), *handle);
                        let c = clamp(c);
                        // Alt on a character's edge: resize it and share the rest of the line
                        let smart = alt
                            && matches!(h, Handle::BoxH(4..=7) | Handle::Edge(_) | Handle::Corner(_) | Handle::BoxH(_))
                            && self.ann(&id).is_some_and(|a| a.level == Level::Char && a.provenance.heuristic.as_ref().is_some_and(|x| x.cell.is_some()));
                        if smart {
                            if let Some(p) = self.cur_mut() {
                                crate::assist::smart_resize_char(p, &id, c);
                            }
                        } else if let Some(a) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
                            apply_handle(a, h, c, shift);
                        }
                        self.refit_live(&[id]);
                    }
                    Drag::Cell(b) => {
                        let b = b.clone();
                        self.drag_border(&b, c);
                    }
                    Drag::Transform { id, corner, start, orig, mode } => {
                        let (id, k) = (id.clone(), *corner);
                        let q = transform_quad(*orig, k, [c[0] - start[0], c[1] - start[1]], *mode);
                        if let Some(a) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
                            a.geometry = Geometry::Quad { points: q };
                        }
                        self.refit_live(&[id]);
                    }
                    _ => {}
                }
            }
        }
        if resp.drag_stopped() {
            if let Drag::Handle { id, handle: Handle::Corner(_) | Handle::Edge(_) } = &self.drag {
                let id = id.clone();
                let keep = self.keep_form.contains(&id);
                if let Some(x) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
                    if matches!(x.geometry, Geometry::Quad { .. }) && !keep {
                        x.geometry = crate::assist::normalize_shape(x.geometry.clone());
                    }
                }
            }
            let drag = std::mem::replace(&mut self.drag, Drag::None);
            if let Some(c) = cur {
                let c2 = clamp(c);
                match drag {
                    Drag::NewShape { start } => {
                        let lo = [start[0].min(c2[0]), start[1].min(c2[1])];
                        let hi = [start[0].max(c2[0]), start[1].max(c2[1])];
                        if (hi[0] - lo[0]) * xf.zoom as f64 > 4.0 && (hi[1] - lo[1]) * xf.zoom as f64 > 4.0 {
                            match self.tool {
                                Tool::Box => {
                                    self.new_annotation(Geometry::Box { x: lo[0], y: lo[1], w: hi[0] - lo[0], h: hi[1] - lo[1], rotation_deg: 0.0 });
                                }
                                Tool::Quad => {
                                    self.new_annotation(Geometry::Quad { points: [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]] });
                                }
                                Tool::AiRegion => {
                                    let p = self.page;
                                    self.ai_job(p, ai::JobKind::Region { min: lo, max: hi });
                                    self.flash("Reading this region in the background…", &ctx);
                                }
                                _ => {}
                            }
                        }
                    }
                    Drag::Marquee { start } => {
                        let lo = [start[0].min(c[0]), start[1].min(c[1])];
                        let hi = [start[0].max(c[0]), start[1].max(c[1])];
                        let ids: Vec<String> = self
                            .cur()
                            .map(|p| {
                                p.annotations
                                    .iter()
                                    .filter(|a| self.visible(a))
                                    .filter(|a| {
                                        let (l, h) = a.geometry.bounds();
                                        l[0] >= lo[0] && l[1] >= lo[1] && h[0] <= hi[0] && h[1] <= hi[1]
                                    })
                                    .map(|a| a.id.clone())
                                    .collect()
                            })
                            .unwrap_or_default();
                        for id in ids {
                            if !self.selection.contains(&id) {
                                self.selection.push(id);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // ---- clicks ----
        if resp.clicked() && !space {
            if let Some(c) = cur {
                let inside = c[0] >= 0.0 && c[1] >= 0.0 && c[0] <= iw && c[1] <= ih;
                match self.tool {
                    Tool::Select | Tool::Box | Tool::AiRegion => {
                        let hit = self.hit_test(c, tol);
                        if alt {
                            if let Some((id, Handle::Vertex(i))) = self.handle_at(c, tol * 1.4, xf.zoom) {
                                if let Some(Geometry::Polygon { points }) = self.cur_mut().and_then(|p| p.get_mut(&id)).map(|a| &mut a.geometry) {
                                    if points.len() > 3 {
                                        points.remove(i);
                                    }
                                }
                            }
                        } else if multi {
                            if let Some(id) = hit {
                                if let Some(k) = self.selection.iter().position(|s| *s == id) {
                                    self.selection.remove(k);
                                } else {
                                    self.selection.push(id);
                                }
                            }
                        } else {
                            self.selection = hit.into_iter().collect();
                        }
                    }
                    Tool::Pan => {}
                    Tool::Quad => {
                        if inside || !self.draft.is_empty() {
                            self.draft.push(clamp(c));
                            if self.draft.len() == 4 {
                                self.finish_draft();
                            }
                        }
                    }
                    Tool::Polygon => {
                        let close = self.draft.len() >= 3 && geom::dist(self.draft[0], c) * (xf.zoom as f64) < 9.0;
                        if close {
                            self.finish_draft();
                        } else if inside || !self.draft.is_empty() {
                            self.draft.push(clamp(c));
                        }
                    }
                    Tool::Order => {
                        if let Some(id) = self.hit_test(c, tol) {
                            self.assign_order(&id, &ctx);
                        }
                    }
                    Tool::Link => {
                        if let Some(id) = self.hit_test(c, tol) {
                            self.link_to(&id, &ctx);
                        }
                    }
                }
            }
        }
        if resp.double_clicked() {
            match self.tool {
                Tool::Polygon => self.finish_draft(),
                Tool::Select => {
                    // add a corner where an edge was double-clicked (boxes and quadrilaterals become polygons)
                    if let (Some(c), Some(id)) = (cur, self.selection.first().cloned()) {
                        if let Some(a) = self.cur_mut().and_then(|p| p.get_mut(&id)) {
                            crate::assist::insert_point(&mut a.geometry, c, tol * 1.5);
                        }
                    }
                }
                _ => {}
            }
        }
        if resp.secondary_clicked() {
            if !self.draft.is_empty() {
                self.draft.pop();
            } else if let Some(c) = cur {
                if let Some(id) = self.hit_test(c, tol) {
                    if !self.selection.contains(&id) {
                        self.selection = vec![id];
                    }
                }
            }
        }
        if self.draft.is_empty() {
            resp.context_menu(|ui| self.canvas_menu(ui));
        }

        // ---- hover and cursor ----
        let xf = Xf { origin: rect.min, offset: self.view.offset, zoom: self.view.zoom };
        self.hovered = None;
        self.cursor_img = None;
        if let (Some(p), true) = (pointer, resp.hovered()) {
            let c = xf.i(p);
            if c[0] >= 0.0 && c[1] >= 0.0 && c[0] <= iw && c[1] <= ih {
                self.cursor_img = Some(c);
            }
            let handle = match &self.drag {
                Drag::Handle { id, handle } => Some((id.clone(), *handle)),
                Drag::None if self.draft.is_empty() => self.handle_at(c, tol * 1.4, xf.zoom),
                _ => None,
            };
            if matches!(self.drag, Drag::None) {
                self.hovered = self.hit_test(c, tol);
            }
            let border = match &self.drag {
                Drag::Cell(b) => self.cell_borders().into_iter().find(|x| &x.0 == b),
                Drag::None if handle.is_none() && self.draft.is_empty() && self.review_edits() => self.border_at(c, tol * 1.2),
                _ => None,
            };
            let icon = if space || self.tool == Tool::Pan || matches!(self.drag, Drag::Pan) {
                if resp.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab }
            } else if let Some((_, a, b)) = border {
                // borders move across themselves
                Self::arrow_for([-(b[1] - a[1]), b[0] - a[0]])
            } else if let Some((_, h)) = handle {
                self.resize_cursor(h)
            } else if matches!(self.drag, Drag::Move { .. }) {
                CursorIcon::Grabbing
            } else if matches!(self.tool, Tool::Order | Tool::Link) {
                if self.hovered.is_some() { CursorIcon::PointingHand } else { CursorIcon::Default }
            } else if self.tool == Tool::Select {
                if self.hovered.is_some() { CursorIcon::Move } else { CursorIcon::Default }
            } else {
                CursorIcon::Crosshair
            };
            ctx.set_cursor_icon(icon);
        }

        // ---- picture ----
        let img_rect = Rect::from_min_max(xf.s([0.0, 0.0]), xf.s([iw, ih]));
        painter.rect_filled(img_rect.expand(3.0), 2.0, Color32::from_black_alpha(120));
        match self.page_texture(&ctx, &page_id) {
            Some(t) => {
                painter.image(t.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                if self.show_text_mask {
                    if let Some(m) = self.mask_texture(&ctx, &page_id) {
                        painter.image(m.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                }
            }
            None => {
                painter.rect_filled(img_rect, 0.0, Color32::from_rgb(50, 52, 60));
                painter.text(img_rect.center(), Align2::CENTER_CENTER, "This picture is missing or cannot be opened", FontId::proportional(15.0), BAD);
            }
        }

        // ---- annotations ----
        // Level of detail: only what is on screen, character boxes when they are big enough to
        // see (or belong to the selection), name tags where they fit. Nothing is copied per frame.
        let cfg = self.cfg();
        let view = rect.expand(40.0);
        {
            let page = self.cur().unwrap();
            let crowded = page.annotations.len() > 300;
            let related: std::collections::HashSet<&str> = {
                let mut s: std::collections::HashSet<&str> = self.selection.iter().map(|x| x.as_str()).collect();
                for id in &self.selection {
                    if let Some(a) = page.get(id) {
                        if let Some(p) = &a.parent {
                            s.insert(p.as_str());
                        }
                    }
                }
                if let Some(h) = &self.hovered {
                    s.insert(h.as_str());
                }
                s
            };
            let mut anns: Vec<(&Annotation, Rect)> = page
                .annotations
                .iter()
                .filter(|a| self.visible(a))
                .filter_map(|a| {
                    let (lo, hi) = a.geometry.bounds();
                    let sr = Rect::from_two_pos(xf.s(lo), xf.s(hi));
                    view.intersects(sr).then_some((a, sr))
                })
                .filter(|(a, sr)| self.shape_shown(a, sr, &related))
                .collect();
            anns.sort_by_key(|(a, _)| a.level);
            let mut tags = Vec::new();
            let rv_active = self.review.as_ref().is_none_or(|r| r.active_border);
            let focus = self.review_focus();
            let problems: std::collections::HashSet<&str> = if self.show_problems { self.issues.iter().filter(|i| i.page_id == page.page_id).filter_map(|i| i.annotation_id.as_deref()).collect() } else { Default::default() };
            for (a, sr) in &anns {
                let col = ann_color(&cfg, a);
                let sel = self.selection.contains(&a.id);
                if (sel && !rv_active) || (!sel && focus.as_ref().is_some_and(|f| !f.contains(&a.id))) {
                    continue;
                }
                if problems.contains(a.id.as_str()) {
                    let pts: Vec<Pos2> = a.geometry.outline().iter().map(|p| xf.s(*p)).collect();
                    outline(&painter, &pts, Stroke::new(5.0, BAD.gamma_multiply(0.45)), false);
                    painter.circle_filled(sr.right_top(), 7.0, BAD);
                    painter.text(sr.right_top(), Align2::CENTER_CENTER, "!", FontId::proportional(11.0), Color32::WHITE);
                }
                let hov = self.hovered.as_ref() == Some(&a.id);
                let k = if sel { 1.8 } else if hov { 1.4 } else { 1.0 };
                let fill_a = match a.level {
                    Level::Panel => 0.0,
                    Level::Container => self.fill * 0.5,
                    Level::Char => self.fill * 0.6,
                    _ => self.fill,
                };
                let pts: Vec<Pos2> = a.geometry.outline().iter().map(|p| xf.s(*p)).collect();
                let small = sr.width().max(sr.height()) < 6.0;
                if (fill_a > 0.0 && !(crowded && small)) || sel {
                    fill_poly(&painter, &pts, col.gamma_multiply((fill_a.max(0.06) * k).min(0.9)));
                }
                let w = if sel { 2.6 } else if hov { 2.2 } else if a.level == Level::Panel { 1.4 } else if a.level == Level::Char { 1.0 } else { 1.8 };
                let dashed = a.review.state == ReviewState::AiProposed && !(crowded && small);
                outline(&painter, &pts, Stroke::new(w, col), dashed);
                if let Some(r) = &a.rectification {
                    if r.role == QuadRole::Surface && (sel || self.tab == Tab::Flatten) {
                        let q: Vec<Pos2> = r.source.iter().map(|p| xf.s(*p)).collect();
                        outline(&painter, &q, Stroke::new(1.6, WARN), true);
                    }
                }
                let tag_fits = match self.labels {
                    ShowLabels::All => true,
                    ShowLabels::Selected => false,
                    ShowLabels::Auto => {
                        let min_side = sr.width().min(sr.height());
                        match a.level {
                            Level::Char => min_side >= 22.0,
                            Level::Panel => !crowded,
                            _ => !crowded || min_side >= 18.0,
                        }
                    }
                };
                if tag_fits || sel || hov {
                    let mut label = match a.level {
                        Level::Panel => format!("Panel {}", a.reading_order.map(|n| n.to_string()).unwrap_or_default()),
                        Level::Container | Level::Region => a.classes.iter().next().and_then(|c| cfg.class(c)).map(|c| c.name.clone()).unwrap_or_else(|| a.level.name().into()),
                        _ => {
                            let t: String = a.text().replace('\n', " ").chars().take(14).collect();
                            if t.is_empty() { a.level.name().to_string() } else { t }
                        }
                    };
                    if a.level == Level::Char && !sel && !hov {
                        label = a.text().to_string();
                    }
                    tags.push((sr.min, label, col, a.review.state, a.review.needs_rereview, a.level));
                }
            }
            for (anchor, label, col, state, rereview, level) in tags {
                if level == Level::Char && label.is_empty() {
                    continue;
                }
                let font = if level == Level::Panel { FontId::proportional(11.0) } else { FontId::proportional(12.0) };
                let galley = painter.layout_no_wrap(label, font, text_on(col));
                let size = galley.size() + vec2(18.0, 2.0);
                let mut tl = anchor - vec2(1.0, size.y);
                if tl.y < rect.top() {
                    tl.y = anchor.y;
                }
                let r = Rect::from_min_size(tl, size);
                painter.rect_filled(r, CornerRadius { nw: 3, ne: 3, sw: 0, se: 3 }, col);
                let dot = Rect::from_center_size(pos2(r.left() + 7.0, r.center().y), Vec2::splat(7.0));
                painter.rect_filled(dot, 4.0, rgb(state.color()));
                painter.rect_stroke(dot, 4.0, Stroke::new(1.0, if rereview { BAD } else { Color32::from_black_alpha(140) }), StrokeKind::Outside);
                painter.galley(r.min + vec2(14.0, 1.0), galley, Color32::WHITE);
            }

            // reading order: numbers, and arrows for the group being edited or selected
            if self.show_order && self.review.as_ref().is_none_or(|r| r.other_borders) {
                let group: Option<Option<String>> = if self.tool == Tool::Order {
                    self.order_parent.clone()
                } else {
                    self.selection.first().and_then(|id| page.get(id)).map(|a| a.parent.clone())
                };
                let mut by_group: BTreeMap<(Option<&str>, Level), Vec<(&Annotation, Rect)>> = BTreeMap::new();
                for (a, sr) in anns.iter().filter(|(a, _)| a.reading_order.is_some()) {
                    by_group.entry((a.parent.as_deref(), a.level)).or_default().push((a, *sr));
                }
                for ((parent, _), mut items) in by_group {
                    let active = group.as_ref().map(|g| g.as_deref()) == Some(parent) || self.tool == Tool::Order;
                    if crowded && !active {
                        continue;
                    }
                    items.sort_by_key(|(a, _)| a.reading_order);
                    let centers: Vec<Pos2> = items.iter().map(|(_, sr)| sr.center()).collect();
                    if active {
                        for w in centers.windows(2) {
                            let v = w[1] - w[0];
                            if v.length() > 20.0 {
                                let a = w[0] + v.normalized() * 10.0;
                                painter.arrow(a, v - v.normalized() * 20.0, Stroke::new(2.0, Color32::from_rgb(255, 235, 120)));
                            }
                        }
                    }
                    for ((a, sr), c) in items.iter().zip(&centers) {
                        if a.level == Level::Char && !active {
                            continue;
                        }
                        let r: f32 = if active { 10.0 } else { 8.0 };
                        if !active && sr.width().min(sr.height()) < r * 2.0 {
                            continue;
                        }
                        painter.circle_filled(*c, r, Color32::from_rgba_unmultiplied(20, 20, 26, if active { 235 } else { 170 }));
                        painter.circle_stroke(*c, r, Stroke::new(1.2, Color32::from_rgb(255, 235, 120)));
                        painter.text(*c, Align2::CENTER_CENTER, a.reading_order.unwrap().to_string(), FontId::proportional(if active { 11.5 } else { 10.0 }), Color32::WHITE);
                    }
                }
            }
        }

        // furigana links and pinned character boxes of the selection
        if let Some(page) = self.cur() {
            let sel: Vec<&Annotation> = self.selection.iter().filter_map(|id| page.get(id)).collect();
            for a in page.annotations.iter().filter(|a| crate::assist::is_furigana(a)) {
                let Some(base) = a.relations.iter().find(|r| r.kind == crate::assist::FURIGANA_OF).and_then(|r| page.get(&r.target)) else { continue };
                let related = sel.iter().any(|s| s.id == a.id || s.id == base.id || a.parent.as_ref() == Some(&s.id) || base.parent.as_ref() == Some(&s.id));
                if !related && page.annotations.len() > 300 {
                    continue;
                }
                let (c0, c1) = (Rect::from_two_pos(xf.s(a.geometry.bounds().0), xf.s(a.geometry.bounds().1)).center(), Rect::from_two_pos(xf.s(base.geometry.bounds().0), xf.s(base.geometry.bounds().1)).center());
                painter.extend(egui::Shape::dashed_line(&[c0, c1], Stroke::new(1.2, Color32::from_rgb(255, 128, 171)), 3.0, 3.0));
                painter.circle_filled(c1, 2.5, Color32::from_rgb(255, 128, 171));
            }
            for s in &sel {
                for c in page.annotations.iter().filter(|c| c.level == Level::Char && (c.id == s.id || c.parent.as_ref() == Some(&s.id))) {
                    if c.provenance.heuristic.as_ref().is_some_and(|h| h.pinned) {
                        let r = Rect::from_two_pos(xf.s(c.geometry.bounds().0), xf.s(c.geometry.bounds().1));
                        painter.circle_filled(r.right_top() + vec2(-4.0, 4.0), 3.5, Color32::from_rgb(255, 213, 79));
                        painter.circle_stroke(r.right_top() + vec2(-4.0, 4.0), 3.5, Stroke::new(1.0, Color32::BLACK));
                    }
                }
            }
        }

        // review: transcription overlay
        if let (Some(r), Some(a)) = (&self.review, self.primary().cloned()) {
            if r.overlay && r.tmode != super::review_mode::TMode::ImageOnly && !a.text().is_empty() {
                self.draw_overlay(&painter, &|p| xf.s(p), &a, r.opacity);
            }
        }
        // previews for the selected annotation: character boxes (follow the shape live while it
        // is resized), a tighter fit around the ink, and text regions for nearby lines
        let pink = Color32::from_rgb(236, 64, 122);
        let mut hint_y = rect.bottom() - 26.0;
        let mut hint = |painter: &egui::Painter, text: String, col: Color32| {
            let g = painter.layout_no_wrap(text, FontId::proportional(12.0), col);
            let r = Rect::from_min_size(pos2(rect.left() + 12.0, hint_y), g.size() + vec2(14.0, 6.0));
            painter.rect_filled(r, 5.0, Color32::from_rgba_unmultiplied(20, 21, 26, 225));
            painter.galley(r.min + vec2(7.0, 3.0), g, col);
            hint_y -= 26.0;
        };
        if self.selection.len() == 1 {
            if let (Some(a), Some(page)) = (self.primary().cloned(), self.cur()) {
                if matches!(a.level, Level::Line | Level::Word | Level::Region) && !a.text().is_empty() && crate::assist::chars_of(page, &a.id).is_empty() && cfg.requirement(Level::Char, &a.classes) != Requirement::Disabled {
                    let (cells, frame) = crate::assist::propose_chars_split(&a, page, &cfg.quick.chars, self.preview_splits(&a.id).as_ref());
                    for c in &cells {
                        let q: Vec<Pos2> = c.quad.iter().map(|p| xf.s(*p)).collect();
                        let mut closed = q.clone();
                        closed.push(q[0]);
                        painter.extend(egui::Shape::dashed_line(&closed, Stroke::new(1.0, pink), 3.0, 3.0));
                    }
                    let key = if self.review.is_some() { self.settings.shortcut("review_chars") } else { self.settings.shortcut("propose_chars") };
                    let how = if frame == "rectified" { " (following the perspective)" } else { "" };
                    hint(&painter, format!("Preview: {} character boxes{how}. Drag the pink ticks beside the line to size each cell; {key} creates them", cells.len()), pink);
                }
            }
            if self.review.as_ref().is_none_or(|r| r.geometry_edit) {
                let active = match &self.drag {
                    Drag::Cell(b) => Some(b.clone()),
                    _ => None,
                };
                for (b, p0, p1) in self.cell_borders() {
                    let (s0, s1) = (xf.s(p0), xf.s(p1));
                    let on = active.as_ref() == Some(&b);
                    painter.line_segment([s0, s1], Stroke::new(if on { 2.5 } else { 1.6 }, pink));
                    let m = xf.s(self.border_grip(p0, p1));
                    let along = (s1 - s0).normalized();
                    painter.line_segment([s1, m], Stroke::new(1.0, pink));
                    let r = Rect::from_center_size(m, if along.x.abs() > along.y.abs() { vec2(12.0, 6.0) } else { vec2(6.0, 12.0) });
                    painter.rect_filled(r, 2.0, if on { Color32::WHITE } else { pink });
                    painter.rect_stroke(r, 2.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
                }
            }
            if self.review.as_ref().is_none_or(|r| r.geometry_edit || !r.overlay) {
                if let Some(g) = self.tighten_suggestion() {
                    let pts: Vec<Pos2> = g.outline().iter().map(|p| xf.s(*p)).collect();
                    outline(&painter, &pts, Stroke::new(1.5, GOOD), true);
                    hint(&painter, format!("The box can be tighter around the text (green): {} applies", self.settings.shortcut("tighten")), GOOD);
                }
            }
            if let Some(a) = self.primary().cloned() {
                if let Some(b) = crate::assist::quad_to_box(&a.geometry) {
                    let _ = b;
                    hint(&painter, "This quadrilateral is a rectangle: it can become a simple box (Annotation tab)".into(), MUTED);
                }
                if let (Geometry::Box { rotation_deg, .. }, Some(g)) = (&a.geometry, crate::assist::straighten(&a.geometry, cfg.quick.straighten_max_deg)) {
                    let pts: Vec<Pos2> = g.outline().iter().map(|p| xf.s(*p)).collect();
                    outline(&painter, &pts, Stroke::new(1.2, Color32::from_rgb(120, 200, 255)), true);
                    hint(&painter, format!("Tilted only {rotation_deg:.1}°: probably level (blue). Remove the tilt in the Annotation tab"), Color32::from_rgb(120, 200, 255));
                }
            }
        }
        if self.review.is_none() {
            let sel: Vec<String> = self.selection.clone();
            let crowded = self.cur().is_some_and(|p| p.annotations.len() > 300);
            let sugg = self.region_suggestions();
            let cyan = Color32::from_rgb(38, 198, 218);
            let mut shown = 0;
            for s in &sugg {
                let mine = s.lines.iter().any(|l| sel.contains(l));
                if !mine && (crowded || !sel.is_empty()) {
                    continue;
                }
                let r = Rect::from_two_pos(xf.s(s.bounds.0), xf.s(s.bounds.1));
                if !view.intersects(r) {
                    continue;
                }
                painter.extend(egui::Shape::dashed_line(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()], Stroke::new(1.3, cyan), 6.0, 4.0));
                painter.text(r.left_bottom() + vec2(2.0, 3.0), Align2::LEFT_TOP, format!("text region? {} lines", s.lines.len()), FontId::proportional(10.5), cyan);
                shown += 1;
            }
            if shown > 0 {
                hint(&painter, format!("{shown} group(s) of nearby lines could share a text region (dashed cyan): {} creates them", self.settings.shortcut("make_regions")), cyan);
            }
        }

        // handles
        if self.selection.len() == 1 && self.review.as_ref().is_none_or(|r| r.geometry_edit) {
            if let Some(a) = self.primary().cloned() {
                let col = ann_color(&cfg, &a);
                let hs = self.handles(&a, xf.zoom);
                if let Some((_, rp)) = hs.iter().find(|(h, _)| *h == Handle::Rotate) {
                    let corners = a.geometry.corners().unwrap();
                    let top = xf.s([(corners[0][0] + corners[1][0]) / 2.0, (corners[0][1] + corners[1][1]) / 2.0]);
                    painter.line_segment([top, xf.s(*rp)], Stroke::new(1.2, col));
                }
                for (h, p) in hs {
                    let sp = xf.s(p);
                    match h {
                        Handle::BoxH(_) => {
                            let r = Rect::from_center_size(sp, Vec2::splat(8.0));
                            painter.rect_filled(r, 1.0, Color32::WHITE);
                            painter.rect_stroke(r, 1.0, Stroke::new(1.5, col), StrokeKind::Middle);
                        }
                        Handle::Rotate => {
                            painter.circle_filled(sp, 5.5, col);
                            painter.circle_stroke(sp, 5.5, Stroke::new(1.5, Color32::WHITE));
                        }
                        Handle::Corner(i) | Handle::Rect(i) => {
                            let c = if matches!(h, Handle::Rect(_)) { WARN } else { col };
                            painter.circle_filled(sp, 6.0, Color32::WHITE);
                            painter.circle_stroke(sp, 6.0, Stroke::new(2.0, c));
                            painter.text(sp + vec2(9.0, -9.0), Align2::LEFT_BOTTOM, ["TL", "TR", "BR", "BL"][i], FontId::proportional(10.0), c);
                        }
                        Handle::Vertex(_) => {
                            painter.circle_filled(sp, 4.5, Color32::WHITE);
                            painter.circle_stroke(sp, 4.5, Stroke::new(1.5, col));
                        }
                        Handle::Edge(_) => {
                            let r = Rect::from_center_size(sp, Vec2::splat(6.0));
                            painter.rect_filled(r, 1.0, col);
                            painter.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Middle);
                        }
                    }
                }
            }
        }

        // shapes being drawn
        let draw_col = self.draw_class.as_ref().and_then(|c| cfg.class(c)).map(|c| rgb(c.color)).unwrap_or_else(|| level_color(self.draw_level));
        if let (Drag::NewShape { start }, Some(c)) = (&self.drag, cur) {
            let c = clamp(c);
            let r = Rect::from_two_pos(xf.s(*start), xf.s(c));
            let col = if self.tool == Tool::AiRegion { Color32::from_rgb(186, 104, 200) } else { draw_col };
            painter.rect_filled(r, 0.0, col.gamma_multiply(0.15));
            painter.extend(egui::Shape::dashed_line(&[r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()], Stroke::new(2.0, col), 6.0, 4.0));
            painter.text(r.right_bottom() + vec2(6.0, 6.0), Align2::LEFT_TOP, format!("{:.0} × {:.0}", (c[0] - start[0]).abs(), (c[1] - start[1]).abs()), FontId::monospace(11.0), Color32::WHITE);
        }
        if let (Drag::Marquee { start }, Some(c)) = (&self.drag, cur) {
            let r = Rect::from_two_pos(xf.s(*start), xf.s(c));
            painter.rect_filled(r, 0.0, ACCENT.gamma_multiply(0.12));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, ACCENT), StrokeKind::Middle);
        }
        if !self.draft.is_empty() {
            let pts: Vec<Pos2> = self.draft.iter().map(|p| xf.s(*p)).collect();
            let near_first = self.tool == Tool::Polygon && pts.len() >= 3 && pointer.is_some_and(|p| p.distance(pts[0]) < 9.0);
            if pts.len() >= 3 {
                fill_poly(&painter, &pts, draw_col.gamma_multiply(0.12));
            }
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.0, draw_col)));
            if let Some(p) = pointer.filter(|_| resp.hovered()) {
                let target = if near_first { pts[0] } else { p };
                painter.extend(egui::Shape::dashed_line(&[*pts.last().unwrap(), target], Stroke::new(1.5, draw_col), 6.0, 4.0));
            }
            for (i, p) in pts.iter().enumerate() {
                let r = if i == 0 && near_first { 7.0 } else { 4.0 };
                painter.circle_filled(*p, r, if i == 0 { draw_col } else { Color32::WHITE });
                painter.circle_stroke(*p, r, Stroke::new(1.5, draw_col));
                if self.tool == Tool::Quad {
                    painter.text(*p + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, ["TL", "TR", "BR", "BL"][i.min(3)], FontId::proportional(10.0), draw_col);
                }
            }
        }
        let drawing = matches!(self.tool, Tool::Box | Tool::Quad | Tool::Polygon | Tool::AiRegion);
        if drawing && !space {
            if let (Some(p), true) = (pointer, resp.hovered()) {
                if img_rect.contains(p) {
                    let s = Stroke::new(1.0, Color32::from_white_alpha(140));
                    painter.extend(egui::Shape::dashed_line(&[pos2(img_rect.left(), p.y), pos2(img_rect.right(), p.y)], s, 5.0, 4.0));
                    painter.extend(egui::Shape::dashed_line(&[pos2(p.x, img_rect.top()), pos2(p.x, img_rect.bottom())], s, 5.0, 4.0));
                }
            }
        }

        // badge
        let badge = match self.tool {
            _ if self.review.is_some() => {
                let r = self.review.as_ref().unwrap();
                let view = match r.view {
                    super::review_mode::RView::Focused => "focused",
                    super::review_mode::RView::Context => "context",
                    super::review_mode::RView::Page => "page",
                };
                let pass = self.review_pass().map(|p| p.name()).unwrap_or("Review");
                let hidden = if self.hidden.is_empty() { String::new() } else { format!(" · {} hidden", self.hidden.len()) };
                format!("{pass} · step {} / {} · {view} view{}{hidden}", (r.session.pos + 1).min(r.session.items.len()), r.session.items.len(), if r.geometry_edit { " · editing shape" } else { "" })
            }
            Tool::Order => format!("Reading order: click items in order (next: {})", self.order_next),
            Tool::Link => match &self.link_relation {
                None => "Link: click the parent of the selection".to_string(),
                Some(k) => format!("Link “{k}”: click the target"),
            },
            Tool::AiRegion => "AI: drag around text to read just that area".to_string(),
            _ => {
                let class = self.draw_class.as_ref().and_then(|c| cfg.class(c)).map(|c| format!(" · {}", c.name)).unwrap_or_default();
                let req = cfg.requirement(self.draw_level, &self.draw_class.iter().cloned().collect());
                let warn = if req == Requirement::Disabled { "  (disabled in this project)" } else { "" };
                format!("Drawing: {}{class}{warn}", self.draw_level.name())
            }
        };
        let galley = painter.layout_no_wrap(badge, FontId::proportional(12.5), Color32::WHITE);
        let r = Rect::from_min_size(rect.left_top() + vec2(12.0, 10.0), galley.size() + vec2(30.0, 10.0));
        painter.rect_filled(r, 6.0, Color32::from_rgba_unmultiplied(20, 21, 26, 225));
        painter.rect_filled(Rect::from_center_size(pos2(r.left() + 12.0, r.center().y), Vec2::splat(10.0)), 2.0, draw_col);
        painter.galley(pos2(r.left() + 22.0, r.top() + 5.0), galley, Color32::WHITE);
    }

    pub fn zoom_by(&mut self, factor: f32, about: Option<Vec2>) {
        let about = about.unwrap_or(self.view_center);
        let new_zoom = (self.view.zoom * factor).clamp(0.02, 40.0);
        let f = new_zoom / self.view.zoom;
        self.view.offset = about - (about - self.view.offset) * f;
        self.view.zoom = new_zoom;
        self.view.fitted = false;
    }
}
