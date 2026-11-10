//! Validation and quality control. Each rule has an id and a default severity that projects can
//! change in `config.validation` (error blocks export, warning is shown, off is ignored).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use unicode_segmentation::UnicodeSegmentation;

use crate::geom;
use crate::review;
use crate::schema::*;
use crate::store::Store;

pub const RULES: [(&str, Severity, &str); 25] = [
    ("parent_level", Severity::Warning, "Parent is not a coarser level (character › line › region › container › panel)"),
    ("char_stale", Severity::Warning, "Proposed character boxes no longer match their line"),
    ("heuristic_unreviewed", Severity::Warning, "Proposed character boxes nobody accepted yet"),
    ("reading_order_suspect", Severity::Warning, "Reading order looks unusual for the layout"),
    ("schema_version", Severity::Error, "Unsupported or unmigrated schema version"),
    ("missing_asset", Severity::Error, "Picture or mask missing or changed since import"),
    ("invalid_geometry", Severity::Error, "Shape with no area, too few points or invalid numbers"),
    ("self_intersecting", Severity::Error, "Polygon edges cross each other"),
    ("quad_order", Severity::Error, "Quadrilateral corners not in TL, TR, BR, BL order"),
    ("out_of_bounds", Severity::Warning, "Shape extends outside the picture"),
    ("missing_reference", Severity::Error, "Parent or related annotation does not exist"),
    ("reading_order", Severity::Error, "Reading order repeats a number or forms a loop"),
    ("reading_order_partial", Severity::Warning, "Only some siblings have a reading order"),
    ("level_disabled", Severity::Error, "Annotation of a level this project disables"),
    ("level_required", Severity::Warning, "A required annotation level is missing"),
    ("missing_transcription", Severity::Error, "Text annotation without transcription (regions whose lines have text are exempt)"),
    ("required_field", Severity::Error, "Required project field is empty"),
    ("field_value", Severity::Error, "Project field value is not allowed"),
    ("span_range", Severity::Error, "Uncertain/unreadable span outside the text"),
    ("char_line_mismatch", Severity::Warning, "Character transcriptions do not add up to the line"),
    ("approval_missing", Severity::Warning, "Reviewed state without the approvals it needs"),
    ("gold_policy", Severity::Error, "Gold annotation does not meet the review policy"),
    ("ai_unreviewed", Severity::Warning, "AI proposals nobody has looked at yet"),
    ("page_not_inspected", Severity::Warning, "Page not yet checked for missing text"),
    ("unresolved_conflict", Severity::Error, "Merge conflict waiting for a decision"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub rule: &'static str,
    pub severity: Severity,
    pub page_id: String,
    pub annotation_id: Option<String>,
    pub message: String,
}

pub fn severity(cfg: &ProjectConfig, rule: &str) -> Severity {
    if let Some(s) = cfg.validation.get(rule) {
        return *s;
    }
    RULES.iter().find(|r| r.0 == rule).map(|r| r.1).unwrap_or(Severity::Warning)
}

struct Out<'a> {
    cfg: &'a ProjectConfig,
    page: &'a str,
    issues: Vec<Issue>,
}

impl Out<'_> {
    fn push(&mut self, rule: &'static str, ann: Option<&str>, msg: String) {
        let s = severity(self.cfg, rule);
        if s != Severity::Off {
            self.issues.push(Issue { rule, severity: s, page_id: self.page.into(), annotation_id: ann.map(String::from), message: msg });
        }
    }
}

fn label(a: &Annotation) -> String {
    let t = a.text();
    if t.is_empty() { a.level.name().to_string() } else { format!("{} “{}”", a.level.name(), t.chars().take(16).collect::<String>()) }
}

pub fn validate_page(cfg: &ProjectConfig, page: &PageFile, assets_ok: Option<&str>) -> Vec<Issue> {
    let mut o = Out { cfg, page: &page.page_id, issues: Vec::new() };
    if let Err(e) = check_version(&page.schema_version) {
        o.push("schema_version", None, e);
    }
    if let Some(problem) = assets_ok {
        o.push("missing_asset", None, problem.into());
    }
    let (w, h) = (page.image.width as f64, page.image.height as f64);
    let ids: HashMap<&str, &Annotation> = page.annotations.iter().map(|a| (a.id.as_str(), a)).collect();

    for a in &page.annotations {
        let id = Some(a.id.as_str());
        let name = label(a);
        // geometry
        let outline = a.geometry.outline();
        let finite = outline.iter().all(|p| p[0].is_finite() && p[1].is_finite());
        match &a.geometry {
            Geometry::Box { w: bw, h: bh, .. } if !finite || *bw <= 0.0 || *bh <= 0.0 => o.push("invalid_geometry", id, format!("{name}: box has no area")),
            Geometry::Polygon { points } if points.len() >= 3 && finite && geom::self_intersects(points) => {
                o.push("self_intersecting", id, format!("{name}: polygon edges cross"))
            }
            Geometry::Polygon { points } if points.len() < 3 || !finite || a.geometry.area() <= 0.0 => {
                o.push("invalid_geometry", id, format!("{name}: polygon needs at least 3 corners and an area"))
            }
            Geometry::Quad { points } if !finite || a.geometry.area() <= 0.0 => o.push("invalid_geometry", id, format!("{name}: quadrilateral has no area")),
            Geometry::Quad { points } if !geom::quad_order_ok(points) => {
                o.push("quad_order", id, format!("{name}: corners must run top-left, top-right, bottom-right, bottom-left"))
            }
            _ => {}
        }
        if let Some(r) = &a.rectification {
            if !geom::quad_order_ok(&r.source) {
                o.push("quad_order", id, format!("{name}: rectification corners must run TL, TR, BR, BL"));
            }
            if r.target_size[0] == 0 || r.target_size[1] == 0 {
                o.push("invalid_geometry", id, format!("{name}: flattened size is empty"));
            }
        }
        let (lo, hi) = a.geometry.bounds();
        if lo[0] < -0.5 || lo[1] < -0.5 || hi[0] > w + 0.5 || hi[1] > h + 0.5 {
            o.push("out_of_bounds", id, format!("{name}: outside the {}×{} picture", page.image.width, page.image.height));
        }
        // references
        if let Some(p) = &a.parent {
            match ids.get(p.as_str()) {
                None => o.push("missing_reference", id, format!("{name}: parent {p} does not exist on this page")),
                Some(pa) => {
                    if !crate::assist::parent_allowed(a.level, pa.level) {
                        o.push("parent_level", id, format!("{name}: sits inside a {} (should be inside a coarser level)", pa.level.name().to_lowercase()));
                    }
                    // loop check
                    let mut seen = BTreeSet::new();
                    let mut cur = a.parent.clone();
                    while let Some(c) = cur {
                        if !seen.insert(c.clone()) || c == a.id {
                            o.push("reading_order", id, format!("{name}: parent links form a loop"));
                            break;
                        }
                        cur = ids.get(c.as_str()).and_then(|x| x.parent.clone());
                    }
                }
            }
        }
        for r in &a.relations {
            if !ids.contains_key(r.target.as_str()) {
                o.push("missing_reference", id, format!("{name}: {} link points to missing {}", r.kind, r.target));
            }
        }
        // levels
        if cfg.requirement(a.level, &a.classes) == Requirement::Disabled {
            o.push("level_disabled", id, format!("{name}: the {} level is disabled in this project", a.level.name().to_lowercase()));
        }
        // transcription
        let unreadable = a.recognizability == Some(Recognizability::Unreadable) || a.occlusion.as_ref().is_some_and(|x| x.occluded && x.fraction.unwrap_or(0.0) >= 1.0);
        if a.level.is_text() && a.text().trim().is_empty() && !unreadable && !crate::assist::text_in_lines(page, a) {
            o.push("missing_transcription", id, format!("{}: no transcription (mark it unreadable if it cannot be read)", a.level.name()));
        }
        if let Some(t) = &a.transcription {
            let n = t.text.graphemes(true).count();
            for s in &t.spans {
                if s.start >= s.end || s.end > n {
                    o.push("span_range", id, format!("{name}: marked span {}..{} outside the {n} characters", s.start, s.end));
                }
            }
        }
        // project fields
        for f in &cfg.custom_fields {
            if !f.levels.is_empty() && !f.levels.contains(&a.level) {
                continue;
            }
            match a.fields.get(&f.key) {
                None | Some(serde_json::Value::Null) => {
                    if f.required {
                        o.push("required_field", id, format!("{name}: “{}” is required", f.label));
                    }
                }
                Some(v) => {
                    let ok = match f.kind {
                        FieldKind::Text => v.is_string() && !(f.required && v.as_str() == Some("")),
                        FieldKind::Bool => v.is_boolean(),
                        FieldKind::Number => v.as_f64().is_some_and(|x| f.min.is_none_or(|m| x >= m) && f.max.is_none_or(|m| x <= m)),
                        FieldKind::Choice => v.as_str().is_some_and(|s| f.choices.iter().any(|c| c == s)),
                    };
                    if !ok {
                        o.push("field_value", id, format!("{name}: “{}” has a value that is not allowed ({v})", f.label));
                    }
                }
            }
        }
        // review
        let n = review::full_reviewers(a, &cfg.review).len();
        if a.review.state >= ReviewState::HumanReviewed && n == 0 && a.review.gold.is_none() {
            o.push("approval_missing", id, format!("{name}: marked {} without approvals", a.review.state.name()));
        }
        if let Some(g) = &a.review.gold {
            let blockers = review::gold_blockers(a, page, &cfg.review);
            if !blockers.is_empty() {
                o.push("gold_policy", id, format!("{name}: gold but {}", blockers.join("; ")));
            } else if g.policy_version < cfg.review.version {
                o.issues.push(Issue {
                    rule: "gold_policy",
                    severity: Severity::Warning,
                    page_id: page.page_id.clone(),
                    annotation_id: Some(a.id.clone()),
                    message: format!("{name}: approved as gold under review policy v{} (now v{})", g.policy_version, cfg.review.version),
                });
            }
        }
    }

    // reading order among siblings
    let mut groups: BTreeMap<Option<&str>, Vec<&Annotation>> = BTreeMap::new();
    for a in &page.annotations {
        groups.entry(a.parent.as_deref()).or_default().push(a);
    }
    for (parent, kids) in &groups {
        let ordered: Vec<u32> = kids.iter().filter(|a| !crate::assist::is_unordered(a, cfg)).filter_map(|a| a.reading_order).collect();
        let mut seen = BTreeSet::new();
        for a in kids {
            if let Some(r) = a.reading_order {
                if !seen.insert((a.level, r)) {
                    o.push("reading_order", Some(&a.id), format!("{}: reading order {r} is used twice", label(a)));
                }
            }
        }
        let text_kids = kids.iter().filter(|a| (a.level != Level::Panel || parent.is_none()) && !crate::assist::is_unordered(a, cfg)).count();
        if !ordered.is_empty() && ordered.len() < text_kids {
            let under = parent.and_then(|p| ids.get(p)).map(|p| label(p)).unwrap_or_else(|| "the page".into());
            o.push("reading_order_partial", parent.map(|s| s), format!("{} of {} items in {under} have a reading order", ordered.len(), text_kids));
        }
    }

    // required levels
    let text_levels: Vec<Level> = Level::DRAWABLE.iter().copied().filter(|l| l.is_text()).collect();
    for l in [Level::Panel, Level::Container] {
        if cfg.levels.get(&l) == Some(&Requirement::Required) && !page.completeness.no_text && !page.annotations.iter().any(|a| a.level == l) {
            o.push("level_required", None, format!("no {} annotation on this page", l.name().to_lowercase()));
        }
    }
    for (i, &fine) in text_levels.iter().enumerate() {
        // nearest coarser enabled text level
        let coarse = text_levels[..i].iter().rev().copied().find(|l| cfg.levels.get(l) != Some(&Requirement::Disabled));
        let Some(coarse) = coarse else { continue };
        for a in page.annotations.iter().filter(|a| a.level == coarse) {
            if cfg.requirement(fine, &a.classes) != Requirement::Required {
                continue;
            }
            if a.recognizability == Some(Recognizability::Unreadable) {
                continue;
            }
            if !page.annotations.iter().any(|c| c.level == fine && c.parent.as_deref() == Some(&a.id)) {
                o.push("level_required", Some(&a.id), format!("{}: {} annotations are required inside it", label(a), fine.name().to_lowercase()));
            }
        }
    }
    if cfg.levels.get(&Level::Line) == Some(&Requirement::Required)
        && !page.completeness.no_text
        && page.completeness.inspected
        && !page.annotations.iter().any(|a| a.level == Level::Line)
    {
        o.push("level_required", None, "page is inspected but has no lines; mark it “no text” if that is right".into());
    }

    // characters vs line text
    for line in page.annotations.iter().filter(|a| matches!(a.level, Level::Line | Level::Word)) {
        let mut kids: Vec<&Annotation> = page.annotations.iter().filter(|c| c.level == Level::Char && c.parent.as_deref() == Some(&line.id)).collect();
        if kids.is_empty() || line.text().is_empty() {
            continue;
        }
        kids.sort_by_key(|c| c.reading_order.unwrap_or(u32::MAX));
        let chars: String = kids.iter().map(|c| c.text()).collect();
        let strip = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        if strip(&chars) != strip(line.text()) {
            o.push("char_line_mismatch", Some(&line.id), format!("{}: characters read “{chars}”", label(line)));
        }
    }

    let ai = page.annotations.iter().filter(|a| a.review.state == ReviewState::AiProposed && a.provenance.origin == Origin::Ai).count();
    if ai > 0 {
        o.push("ai_unreviewed", None, format!("{ai} AI proposals not yet accepted, corrected or rejected"));
    }
    let heur = page.annotations.iter().filter(|a| a.review.state == ReviewState::AiProposed && a.provenance.origin == Origin::Heuristic).count();
    if heur > 0 {
        o.push("heuristic_unreviewed", None, format!("{heur} proposed character boxes not yet accepted"));
    }
    for c in page.annotations.iter().filter(|a| a.provenance.heuristic.is_some()) {
        if let Some(why) = crate::assist::stale_reason(page, c) {
            o.push("char_stale", Some(&c.id), format!("character “{}” is out of date: {why}; regenerate it", c.text()));
        }
    }
    for (id, why) in crate::assist::order_suspects(page, cfg, cfg.reading.page_direction) {
        let name = page.get(&id).map(label).unwrap_or_default();
        o.push("reading_order_suspect", Some(&id), format!("{name}: {why}"));
    }
    if !page.completeness.inspected {
        o.push("page_not_inspected", None, "not yet checked for missing text".into());
    }
    for c in &page.conflicts {
        o.push("unresolved_conflict", Some(&c.annotation_id), format!("conflict from {}: {}", c.source, c.note));
    }
    o.issues
}

pub fn validate_project(store: &Store) -> Vec<Issue> {
    let bad: HashMap<String, String> = store.verify_images().into_iter().collect();
    let mut all = Vec::new();
    for page in &store.pages {
        all.extend(validate_page(&store.manifest.config, page, bad.get(&page.page_id).map(|s| s.as_str())));
        for a in &page.annotations {
            for m in &a.masks {
                if !store.root.join(&m.path).exists() {
                    all.push(Issue {
                        rule: "missing_asset",
                        severity: severity(&store.manifest.config, "missing_asset"),
                        page_id: page.page_id.clone(),
                        annotation_id: Some(a.id.clone()),
                        message: format!("mask {} is missing", m.path),
                    });
                }
            }
        }
    }
    if let Err(e) = check_version(&store.manifest.schema_version) {
        all.push(Issue { rule: "schema_version", severity: Severity::Error, page_id: String::new(), annotation_id: None, message: format!("project.json: {e}") });
    }
    all
}
