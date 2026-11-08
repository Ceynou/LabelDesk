//! The canonical dataset format (schema 1.0).
//!
//! A project folder holds `project.json` (the manifest: configuration, review policy, page list)
//! and one `annotations/<page-id>.json` per source image. Files are written deterministically:
//! fixed field order, sorted maps, coordinates rounded to 0.01 px, two-space indentation, trailing
//! newline. Unknown fields are kept and written back unchanged, so newer tools can add data.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::geom::{self, P};
use crate::ocr::ModelRef;

pub const SCHEMA_VERSION: &str = "1.2.0";
pub const PROJECT_FORMAT: &str = "labeldesk.comic-ocr.project";
pub const PAGE_FORMAT: &str = "labeldesk.comic-ocr.page";

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

// ------------------------------------------------------------------ small shared types

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Stamp {
    pub by: String,
    pub at: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Page,
    Panel,
    Container,
    Region,
    Line,
    Word,
    Char,
}

impl Level {
    pub const ALL: [Level; 7] = [Level::Page, Level::Panel, Level::Container, Level::Region, Level::Line, Level::Word, Level::Char];
    /// Levels that are drawn as annotations (the page itself is the image).
    pub const DRAWABLE: [Level; 6] = [Level::Panel, Level::Container, Level::Region, Level::Line, Level::Word, Level::Char];

    pub fn name(self) -> &'static str {
        match self {
            Level::Page => "Page",
            Level::Panel => "Panel",
            Level::Container => "Text container",
            Level::Region => "Text region",
            Level::Line => "Line",
            Level::Word => "Word",
            Level::Char => "Character",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Level::Page => "page",
            Level::Panel => "panel",
            Level::Container => "container",
            Level::Region => "region",
            Level::Line => "line",
            Level::Word => "word",
            Level::Char => "char",
        }
    }
    pub fn rank(self) -> u8 {
        self as u8
    }
    /// Levels that normally carry a transcription.
    pub fn is_text(self) -> bool {
        matches!(self, Level::Region | Level::Line | Level::Word | Level::Char)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    Required,
    Optional,
    Disabled,
}

// ------------------------------------------------------------------ geometry

/// All coordinates are continuous pixel coordinates of the original image (see FORMAT.md).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    /// Axis-aligned box (x, y = top-left) rotated clockwise by `rotation_deg` about its centre.
    Box {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        rotation_deg: f64,
    },
    /// Simple polygon, clockwise on screen, no repeated closing point.
    Polygon { points: Vec<P> },
    /// Four corners: top-left, top-right, bottom-right, bottom-left in the text's own frame.
    Quad { points: [P; 4] },
}

impl Geometry {
    pub fn kind(&self) -> &'static str {
        match self {
            Geometry::Box { .. } => "box",
            Geometry::Polygon { .. } => "polygon",
            Geometry::Quad { .. } => "quad",
        }
    }

    pub fn outline(&self) -> Vec<P> {
        match self {
            Geometry::Box { x, y, w, h, rotation_deg } => {
                geom::rect_corners_unordered([x + w / 2.0, y + h / 2.0], (*w, *h), rotation_deg.to_radians()).to_vec()
            }
            Geometry::Polygon { points } => points.clone(),
            Geometry::Quad { points } => points.to_vec(),
        }
    }

    /// Four corners when the shape has them (box or quadrilateral).
    pub fn corners(&self) -> Option<[P; 4]> {
        match self {
            Geometry::Box { x, y, w, h, rotation_deg } => {
                Some(geom::rect_corners_unordered([x + w / 2.0, y + h / 2.0], (*w, *h), rotation_deg.to_radians()))
            }
            Geometry::Quad { points } => Some(*points),
            Geometry::Polygon { .. } => None,
        }
    }

    pub fn bounds(&self) -> (P, P) {
        geom::bounds(&self.outline())
    }

    pub fn area(&self) -> f64 {
        match self {
            Geometry::Box { w, h, .. } => w * h,
            _ => geom::signed_area(&self.outline()).abs(),
        }
    }

    pub fn translate(&mut self, d: P) {
        match self {
            Geometry::Box { x, y, .. } => {
                *x += d[0];
                *y += d[1];
            }
            Geometry::Polygon { points } => points.iter_mut().for_each(|p| {
                p[0] += d[0];
                p[1] += d[1];
            }),
            Geometry::Quad { points } => points.iter_mut().for_each(|p| {
                p[0] += d[0];
                p[1] += d[1];
            }),
        }
    }

    pub fn contains(&self, p: P, tol: f64) -> bool {
        let o = self.outline();
        geom::point_in_poly(p, &o) || (0..o.len()).any(|i| geom::dist_to_segment(p, o[i], o[(i + 1) % o.len()]) < tol)
    }

    pub fn round(&mut self) {
        let r = |p: &mut P| {
            p[0] = geom::r2(p[0]);
            p[1] = geom::r2(p[1]);
        };
        match self {
            Geometry::Box { x, y, w, h, rotation_deg } => {
                *x = geom::r2(*x);
                *y = geom::r2(*y);
                *w = geom::r2(*w);
                *h = geom::r2(*h);
                *rotation_deg = geom::r2(*rotation_deg);
            }
            Geometry::Polygon { points } => points.iter_mut().for_each(r),
            Geometry::Quad { points } => points.iter_mut().for_each(r),
        }
    }
}

/// What a rectification quadrilateral describes.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuadRole {
    /// The quadrilateral encloses the text region itself.
    TextRegion,
    /// The quadrilateral outlines the physical flat surface carrying the text (a sign, a page, a screen).
    Surface,
}

/// Perspective correction. Primary data: the four source corners and the destination size.
/// The homography is derived and cached; it maps source corners to
/// (0,0), (w,0), (w,h), (0,h) of the flattened picture.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Rectification {
    pub role: QuadRole,
    pub source: [P; 4],
    pub target_size: [u32; 2],
    #[serde(default)]
    pub target_size_manual: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homography: Option<[f64; 9]>,
}

impl Rectification {
    pub fn update_cache(&mut self) {
        let (w, h) = (self.target_size[0] as f64, self.target_size[1] as f64);
        self.homography = geom::homography(&self.source, &[[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]])
            .map(|m| m.map(|v| (v * 1e9).round() / 1e9));
    }
}

// ------------------------------------------------------------------ text

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    Custom,
}

impl WritingMode {
    pub const ALL: [WritingMode; 4] = [WritingMode::HorizontalTb, WritingMode::VerticalRl, WritingMode::VerticalLr, WritingMode::Custom];
    pub fn name(self) -> &'static str {
        match self {
            WritingMode::HorizontalTb => "Horizontal",
            WritingMode::VerticalRl => "Vertical, columns right→left",
            WritingMode::VerticalLr => "Vertical, columns left→right",
            WritingMode::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
    Ttb,
    Btt,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Direction::Ltr, Direction::Rtl, Direction::Ttb, Direction::Btt];
    pub fn name(self) -> &'static str {
        match self {
            Direction::Ltr => "Left to right",
            Direction::Rtl => "Right to left",
            Direction::Ttb => "Top to bottom",
            Direction::Btt => "Bottom to top",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Uncertain,
    Unreadable,
}

/// A marked part of the transcription. Offsets count extended grapheme clusters
/// (what a reader sees as one character), `end` exclusive.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SpanKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Alternative {
    pub text: String,
    pub by: String,
    pub at: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextSource {
    /// Every character is directly visible.
    #[default]
    Visible,
    /// Some characters were inferred (e.g. hidden behind an object).
    Inferred,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Transcription {
    /// Exact visible text. Line breaks are `\n`; punctuation is kept as printed.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized: Option<String>,
    /// BCP 47 language tag, e.g. `ja`, `ko`, `zh-Hant`, `en`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// ISO 15924 script code, e.g. `Jpan`, `Hang`, `Latn`, `Arab`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default)]
    pub writing_mode: WritingMode,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<Span>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<Alternative>,
    #[serde(default)]
    pub source: TextSource,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranslationKind {
    Translation,
    Transliteration,
}

/// Translations and transliterations are kept apart from the original-language ground truth.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Translation {
    pub kind: TranslationKind,
    pub language: String,
    pub text: String,
    pub by: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Recognizability {
    Readable,
    PartiallyReadable,
    Ambiguous,
    Unreadable,
    /// Quick binary answer "not perfectly readable", to be refined into one of the three above.
    NotPerfect,
}

impl Recognizability {
    pub const ALL: [Recognizability; 4] = [Recognizability::Readable, Recognizability::PartiallyReadable, Recognizability::Ambiguous, Recognizability::Unreadable];
    /// The two-way first answer: refine `NotPerfect` later.
    pub const COARSE: [Recognizability; 2] = [Recognizability::Readable, Recognizability::NotPerfect];
    pub fn name(self) -> &'static str {
        match self {
            Recognizability::Readable => "Perfectly readable",
            Recognizability::PartiallyReadable => "Partly readable",
            Recognizability::Ambiguous => "Ambiguous",
            Recognizability::Unreadable => "Unreadable",
            Recognizability::NotPerfect => "Not perfectly readable",
        }
    }
    /// A provisional answer that still needs refining.
    pub fn coarse(self) -> bool {
        self == Recognizability::NotPerfect
    }
}

pub const QUALITY_FLAGS: [(&str, &str); 8] = [
    ("blur", "Blur"),
    ("noise", "Noise"),
    ("low_resolution", "Low resolution"),
    ("low_contrast", "Low contrast"),
    ("compression_artifacts", "Compression artifacts"),
    ("clipping", "Clipped"),
    ("halftone_interference", "Halftone"),
    ("artistic_deformation", "Artistic deformation"),
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Orientation {
    /// Rotation of the text relative to upright, degrees clockwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_deg: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mirrored: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub skew_deg: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MaskRef {
    /// `text_region`, `ink` or `occlusion`.
    pub role: String,
    /// PNG relative to the project folder; non-zero pixels are inside.
    pub path: String,
    pub sha256: String,
    /// Where the mask's top-left pixel sits in original image coordinates.
    pub offset: [u32; 2],
    pub size: [u32; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Occlusion {
    pub occluded: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Estimated hidden fraction 0..1 (optional, never required).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    /// Quick answer from review: `partial` or `severe` (absent = not stated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<OcclusionLevel>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcclusionLevel {
    Partial,
    Severe,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
    /// Quick binary answer "uncertain", to be refined into Low or Medium later.
    Uncertain,
}

impl Confidence {
    pub fn name(self) -> &'static str {
        match self {
            Confidence::Low => "Low",
            Confidence::Medium => "Medium",
            Confidence::High => "Certain",
            Confidence::Uncertain => "Uncertain",
        }
    }
}

/// Annotator certainty, recorded per aspect and never mixed with model scores.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct HumanConfidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub localization: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_order: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<Confidence>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Relation {
    /// e.g. `continues`, `same_speaker`, `duplicate_of`, `refers_to`.
    pub kind: String,
    pub target: String,
}

// ------------------------------------------------------------------ provenance

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Manual,
    Ai,
    Imported,
    Sample,
    /// Produced by a rule-based helper (e.g. character boxes split from a line).
    Heuristic,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldState {
    Proposed,
    Accepted,
    Corrected,
    Manual,
    Verified,
    ChangedAfterVerification,
    /// Generated by a heuristic (not a model, not a person); never ground truth by itself.
    HeuristicProposal,
    /// Filled in from defaults or context (project settings, parent, detected script).
    Inferred,
}

impl FieldState {
    pub fn name(self) -> &'static str {
        match self {
            FieldState::Proposed => "proposed by AI",
            FieldState::Accepted => "accepted as proposed",
            FieldState::Corrected => "corrected by a person",
            FieldState::Manual => "entered by a person",
            FieldState::Verified => "verified",
            FieldState::ChangedAfterVerification => "changed after verification",
            FieldState::HeuristicProposal => "heuristic proposal",
            FieldState::Inferred => "inferred from defaults or context",
        }
    }
}

/// Points into the page's inference run that produced this annotation. The full model identity
/// (artifact hashes, configuration) lives in the run record; the prediction there is never edited.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AiOrigin {
    pub run_id: String,
    pub prediction: usize,
    pub model: String,
    pub model_version: String,
    pub model_revision: String,
    /// Model-native scores, not calibrated probabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recognition_score: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Provenance {
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<AiOrigin>,
    /// State per field group: `geometry`, `transcription`, `reading_order`, `metadata`.
    #[serde(default)]
    pub fields: BTreeMap<String, FieldState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heuristic: Option<HeuristicOrigin>,
}

/// Where a heuristic proposal came from, so it can be found stale and regenerated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HeuristicOrigin {
    /// e.g. `labeldesk.char_split`.
    pub algorithm: String,
    pub version: String,
    /// Annotation it was derived from.
    pub source: String,
    /// The transcription it was split from, and that annotation's revision at the time.
    pub source_text: String,
    pub source_transcription_revision: u32,
    pub parent_geometry_revision: u32,
    /// Grapheme-cluster range of `source_text` this proposal stands for (end exclusive).
    pub span: [usize; 2],
    /// `rectified` (through a homography), `quad` or `axis`.
    pub frame: String,
    /// The cell inside the source's frame, as fractions `[u0, v0, u1, v1]` (u along the frame's
    /// top edge, v along its left edge). Lets the box follow its line when the line is resized,
    /// keeping any cell size a person chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<[f64; 4]>,
    /// A person fixed this box's size: automatic redistribution of the line leaves it alone.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    /// A person confirmed the current value.
    Confirmed,
    /// A person looked and could not tell.
    Unknown,
    /// The question does not apply to this annotation.
    NotApplicable,
    /// Filled in automatically; nobody confirmed it yet.
    Inferred,
}

/// Review record for one quick-review field (`language`, `mirrored`, `occlusion`, …).
/// Unknown and not-applicable are different answers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FieldCheck {
    pub state: CheckState,
    pub by: String,
    pub at: String,
    /// Where an inferred value came from (`project default`, `detected from text`, …).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Set by a batch decision over several annotations.
    #[serde(default, skip_serializing_if = "is_false")]
    pub batch: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderSource {
    Manual,
    Suggested,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderMark {
    /// The reading order here is genuinely unclear.
    Ambiguous,
    /// Intentionally not ordered (background text, scattered sound effects).
    Unordered,
}

/// Reading-order bookkeeping and per-region overrides of the project conventions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct ReadingInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<OrderSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<OrderMark>,
    /// Progression of the items inside this region (`rtl` or `ltr`), overriding the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progression: Option<Direction>,
    /// Writing mode of the text inside this region, overriding the project default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writing_mode: Option<WritingMode>,
}

// ------------------------------------------------------------------ review

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    AiProposed,
    Draft,
    Submitted,
    ChangesRequested,
    HumanReviewed,
    PeerVerified,
    GoldApproved,
}

impl ReviewState {
    pub const ALL: [ReviewState; 7] = [
        ReviewState::AiProposed,
        ReviewState::Draft,
        ReviewState::Submitted,
        ReviewState::ChangesRequested,
        ReviewState::HumanReviewed,
        ReviewState::PeerVerified,
        ReviewState::GoldApproved,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ReviewState::AiProposed => "AI proposed",
            ReviewState::Draft => "Draft",
            ReviewState::Submitted => "Submitted",
            ReviewState::ChangesRequested => "Changes requested",
            ReviewState::HumanReviewed => "Reviewed",
            ReviewState::PeerVerified => "Peer verified",
            ReviewState::GoldApproved => "Gold",
        }
    }
    pub fn color(self) -> [u8; 3] {
        match self {
            ReviewState::AiProposed => [186, 104, 200],
            ReviewState::Draft => [144, 152, 168],
            ReviewState::Submitted => [79, 160, 240],
            ReviewState::ChangesRequested => [240, 120, 90],
            ReviewState::HumanReviewed => [100, 190, 150],
            ReviewState::PeerVerified => [60, 180, 110],
            ReviewState::GoldApproved => [240, 190, 60],
        }
    }
    pub fn needs_work(self) -> bool {
        matches!(self, ReviewState::AiProposed | ReviewState::Draft | ReviewState::ChangesRequested)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Transcription,
    Geometry,
    ReadingOrder,
}

impl Dimension {
    pub const ALL: [Dimension; 3] = [Dimension::Transcription, Dimension::Geometry, Dimension::ReadingOrder];
    pub fn name(self) -> &'static str {
        match self {
            Dimension::Transcription => "Transcription",
            Dimension::Geometry => "Geometry",
            Dimension::ReadingOrder => "Reading order",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Dimension::Transcription => "transcription",
            Dimension::Geometry => "geometry",
            Dimension::ReadingOrder => "reading_order",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Invalidation {
    pub at: String,
    pub by: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Approval {
    pub dimension: Dimension,
    pub by: String,
    pub at: String,
    /// Annotation revision that was approved.
    pub revision: u32,
    /// Review-policy version in force when approving. Later policy changes never upgrade or drop it.
    pub policy_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidated: Option<Invalidation>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    Comment,
    ChangeRequest,
    Disagreement,
    Rejection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReviewNote {
    pub kind: NoteKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<Dimension>,
    pub by: String,
    pub at: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Stamp>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolution: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GoldRecord {
    pub by: String,
    pub at: String,
    pub revision: u32,
    pub policy_version: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Review {
    pub state: ReviewState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<Approval>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ReviewNote>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub needs_rereview: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gold: Option<GoldRecord>,
}

impl Default for Review {
    fn default() -> Self {
        Review { state: ReviewState::Draft, approvals: Vec::new(), notes: Vec::new(), needs_rereview: false, gold: None }
    }
}

// ------------------------------------------------------------------ annotation

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Annotation {
    pub id: String,
    #[serde(rename = "type")]
    pub level: Level,
    pub page_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    pub geometry: Geometry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rectification: Option<Rectification>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub classes: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub attributes: BTreeSet<String>,
    /// Free tags people add while working (e.g. `check-later`, `odd-font`); never exported as labels.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription: Option<Transcription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translations: Vec<Translation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recognizability: Option<Recognizability>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub quality: BTreeSet<String>,
    #[serde(default)]
    pub orientation: Orientation,
    /// Position in the reading sequence among siblings (same parent), starting at 1.
    /// Set by people; never inferred from position alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_order: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occlusion: Option<Occlusion>,
    #[serde(default)]
    pub confidence: HumanConfidence,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<MaskRef>,
    /// Reserved for curved baselines, text boundaries and deformation meshes (not edited yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deformation: Option<Value>,
    /// Project-specific fields, namespaced `namespace:name`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<ReadingInfo>,
    /// Quick-review answers per field (confirmed, unknown, not applicable, inferred).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, FieldCheck>,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub review: Review,
    pub created: Stamp,
    pub modified: Stamp,
    pub revision: u32,
    /// Fields written by newer tools, kept as they are.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl Annotation {
    pub fn new(id: String, level: Level, page_id: &str, geometry: Geometry, who: &str, now: &str) -> Self {
        let stamp = Stamp { by: who.into(), at: now.into() };
        Annotation {
            id,
            level,
            page_id: page_id.into(),
            parent: None,
            relations: Vec::new(),
            geometry,
            rectification: None,
            classes: BTreeSet::new(),
            attributes: BTreeSet::new(),
            tags: BTreeSet::new(),
            transcription: None,
            translations: Vec::new(),
            recognizability: None,
            quality: BTreeSet::new(),
            orientation: Orientation::default(),
            reading_order: None,
            occlusion: None,
            confidence: HumanConfidence::default(),
            masks: Vec::new(),
            deformation: None,
            fields: BTreeMap::new(),
            reading: None,
            checks: BTreeMap::new(),
            provenance: Provenance::default(),
            review: Review::default(),
            created: stamp.clone(),
            modified: stamp,
            revision: 1,
            unknown: BTreeMap::new(),
        }
    }

    pub fn text(&self) -> &str {
        self.transcription.as_ref().map(|t| t.text.as_str()).unwrap_or("")
    }

    /// Which field groups differ between two versions.
    pub fn changed_groups(&self, other: &Annotation) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.geometry != other.geometry || self.rectification != other.rectification || self.orientation != other.orientation {
            v.push("geometry");
        }
        if self.transcription != other.transcription {
            v.push("transcription");
        }
        if self.reading_order != other.reading_order || self.parent != other.parent || self.relations != other.relations || self.reading != other.reading {
            v.push("reading_order");
        }
        if self.level != other.level
            || self.classes != other.classes
            || self.attributes != other.attributes
            || self.translations != other.translations
            || self.recognizability != other.recognizability
            || self.quality != other.quality
            || self.occlusion != other.occlusion
            || self.confidence != other.confidence
            || self.fields != other.fields
            || self.masks != other.masks
        {
            v.push("metadata");
        }
        v
    }

    pub fn round(&mut self) {
        self.geometry.round();
        if let Some(r) = &mut self.rectification {
            for p in &mut r.source {
                p[0] = geom::r2(p[0]);
                p[1] = geom::r2(p[1]);
            }
        }
        self.orientation.rotation_deg = geom::r2(self.orientation.rotation_deg);
        self.orientation.skew_deg = geom::r2(self.orientation.skew_deg);
    }
}

// ------------------------------------------------------------------ inference records

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PredictionStatus {
    /// Became an annotation (see `annotation_id`).
    Proposed,
    /// Not turned into an annotation because it overlaps existing work.
    SkippedOverlap,
    Rejected,
    /// Offered as an alternative transcription of an existing annotation.
    Alternative,
}

/// A raw model output. Never edited after the run, except its status.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Prediction {
    pub index: usize,
    pub quad: [P; 4],
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detection_score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recognition_score: Option<f64>,
    pub status: PredictionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotation_id: Option<String>,
    /// Who rejected it and when (status `rejected`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<Stamp>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunScope {
    Page,
    Region { min: P, max: P },
    /// Recognition only, on an annotation's crop. `transform` is `rectified` or `axis_aligned_crop`.
    Annotation { annotation_id: String, transform: String, quad: [P; 4], target_size: [u32; 2] },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InferenceRun {
    pub id: String,
    pub at: String,
    pub by: String,
    pub model: ModelRef,
    pub config: BTreeMap<String, Value>,
    pub scope: RunScope,
    /// Image the model saw (checksum of the original file).
    pub image_sha256: String,
    pub predictions: Vec<Prediction>,
}

// ------------------------------------------------------------------ page file

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Completeness {
    /// A person looked over the whole page for text that is still missing.
    pub inspected: bool,
    /// All text the project requires is annotated (or the page has none).
    pub complete: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_text: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<Stamp>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct PageReview {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_for_export: Option<Stamp>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ImageRef {
    pub path: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

/// An annotation that changed differently on two copies of the project, waiting for a person.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Conflict {
    pub annotation_id: String,
    pub detected: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theirs: Option<Annotation>,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PageFile {
    pub format: String,
    pub schema_version: String,
    pub page_id: String,
    pub image: ImageRef,
    #[serde(default)]
    pub completeness: Completeness,
    #[serde(default)]
    pub review: PageReview,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inference_runs: Vec<InferenceRun>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<Conflict>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl PageFile {
    pub fn new(page_id: &str, image: ImageRef) -> Self {
        PageFile {
            format: PAGE_FORMAT.into(),
            schema_version: SCHEMA_VERSION.into(),
            page_id: page_id.into(),
            image,
            completeness: Completeness::default(),
            review: PageReview::default(),
            annotations: Vec::new(),
            inference_runs: Vec::new(),
            conflicts: Vec::new(),
            unknown: BTreeMap::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Annotation> {
        self.annotations.iter_mut().find(|a| a.id == id)
    }
}

// ------------------------------------------------------------------ project manifest

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceInfo {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub work: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub series: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub volume: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chapter: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub page_number: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reading_direction: String,
    /// How the image was obtained.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub acquisition: String,
    /// Licence of the source image (kept apart from the annotation licence).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub restricted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub original_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PageEntry {
    pub id: String,
    /// Relative to the project folder when possible.
    pub image: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub encoding: String,
    pub annotation_file: String,
    #[serde(default)]
    pub source: SourceInfo,
    pub added: Stamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClassDef {
    pub id: String,
    pub name: String,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LevelOverride {
    pub class: String,
    pub level: Level,
    pub requirement: Requirement,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    Number,
    Bool,
    Choice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FieldDef {
    /// `namespace:name`; the namespace keeps project fields apart from standard ones.
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Empty means all levels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<Level>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EditPolicy {
    /// Approvals for the changed aspect are invalidated (kept in the record, no longer counted).
    Invalidate,
    /// Approvals stay but the annotation is flagged and cannot be gold until re-approved.
    MarkForRereview,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReviewPolicy {
    /// Increases on every policy change; approvals remember the version they were given under.
    pub version: u32,
    pub reviewers_required: u32,
    /// Reviewers must be different people from the annotator.
    pub independent_reviewers: bool,
    pub allow_self_approval: bool,
    /// Aspects that need their own approval.
    pub required_dimensions: Vec<Dimension>,
    /// Disagreements on these aspects must be resolved before gold.
    pub adjudicate_on: Vec<Dimension>,
    pub enabled_states: Vec<ReviewState>,
    pub gold_requires_page_complete: bool,
    pub on_edit_after_approval: EditPolicy,
}

impl Default for ReviewPolicy {
    fn default() -> Self {
        ReviewPolicy {
            version: 1,
            reviewers_required: 1,
            independent_reviewers: true,
            allow_self_approval: false,
            required_dimensions: vec![Dimension::Transcription, Dimension::Geometry, Dimension::ReadingOrder],
            adjudicate_on: vec![Dimension::Transcription],
            enabled_states: ReviewState::ALL.to_vec(),
            gold_requires_page_complete: true,
            on_edit_after_approval: EditPolicy::Invalidate,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModelSettings {
    pub enabled: bool,
    pub auto_run_on_import: bool,
    /// The exact model used by this project. Changing it is an explicit action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<ModelRef>,
    /// Optional character-level model (muscgab/manga-ocr-charspot-small); not wired yet.
    #[serde(default)]
    pub character_model_enabled: bool,
}

impl Default for ModelSettings {
    fn default() -> Self {
        ModelSettings { enabled: true, auto_run_on_import: false, pinned: None, character_model_enabled: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportSettings {
    pub exclude_restricted: bool,
    /// Train / validation / test shares.
    pub split: [f64; 3],
    /// Text that seeds the deterministic split.
    pub split_seed: String,
    /// Normalisation applied when comparing transcriptions for evaluation.
    pub eval_normalization: String,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings { exclude_restricted: true, split: [0.8, 0.1, 0.1], split_seed: "labeldesk".into(), eval_normalization: "NFKC, remove whitespace".into() }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Off,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProjectConfig {
    pub levels: BTreeMap<Level, Requirement>,
    #[serde(default)]
    pub level_overrides: Vec<LevelOverride>,
    pub text_classes: Vec<ClassDef>,
    #[serde(default)]
    pub attributes: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub default_language: String,
    #[serde(default)]
    pub default_script: String,
    #[serde(default)]
    pub default_writing_mode: WritingMode,
    #[serde(default)]
    pub default_direction: Direction,
    #[serde(default)]
    pub custom_fields: Vec<FieldDef>,
    #[serde(default)]
    pub review: ReviewPolicy,
    #[serde(default)]
    pub model: ModelSettings,
    /// Severity per validation rule id (see validate.rs); missing rules use their default.
    #[serde(default)]
    pub validation: BTreeMap<String, Severity>,
    #[serde(default)]
    pub export: ExportSettings,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub annotation_license: String,
    #[serde(default)]
    pub reading: ReadingConventions,
    #[serde(default)]
    pub quick: QuickReviewPolicy,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderStrategy {
    /// Rows (tiers) from top to bottom, items in a row by the page direction. Usual manga/comics.
    Tiers,
    /// Columns by the page direction, top to bottom inside a column (e.g. 4-koma strips).
    Columns,
    /// Distance from the starting corner.
    Position,
}

/// Default reading conventions; pages (source reading direction) and regions (`reading`) override them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ReadingConventions {
    /// Panel and bubble progression inside a tier: `rtl` (manga) or `ltr` (western).
    pub page_direction: Direction,
    pub strategy: OrderStrategy,
    /// Suggest an order for new and imported annotations automatically. Off by default: the
    /// order is best computed once every box of the page exists (last review pass).
    pub auto_suggest: bool,
    /// Text types read outside the reading sequence (furigana is always exempt too).
    #[serde(default = "default_unordered")]
    pub unordered_classes: Vec<String>,
}

pub fn default_unordered() -> Vec<String> {
    ["background", "sfx", "page_number"].iter().map(|s| s.to_string()).collect()
}

impl Default for ReadingConventions {
    fn default() -> Self {
        ReadingConventions { page_direction: Direction::Rtl, strategy: OrderStrategy::Tiers, auto_suggest: false, unordered_classes: default_unordered() }
    }
}

/// Fields offered by the quick review checklist.
pub const QUICK_FIELDS: [(&str, &str); 13] = [
    ("geometry", "Box shape"),
    ("transcription", "Transcription"),
    ("recognizability", "Legibility"),
    ("classes", "Text type"),
    ("language", "Language / script"),
    ("direction", "Writing direction"),
    ("mirrored", "Mirrored"),
    ("occlusion", "Occlusion"),
    ("quality", "Image quality"),
    ("char_boxes", "Character boxes"),
    ("reading_order", "Reading order"),
    ("perspective", "Perspective"),
    ("confidence", "Your confidence"),
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CharPolicy {
    /// Spaces take room but get no box.
    pub skip_spaces: bool,
    /// Punctuation takes room but gets no box.
    pub skip_punctuation: bool,
    /// Estimate proportional widths for Latin and other non-CJK text (otherwise equal widths).
    pub proportional_latin: bool,
    /// Every character takes the same room (also per line with the `monospace` attribute).
    #[serde(default, skip_serializing_if = "is_false")]
    pub monospace: bool,
}

impl Default for CharPolicy {
    fn default() -> Self {
        CharPolicy { skip_spaces: true, skip_punctuation: false, proportional_latin: true, monospace: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuickReviewPolicy {
    /// Ids from `QUICK_FIELDS` that every applicable annotation must have answered.
    pub required_fields: Vec<String>,
    /// Inferred values must be confirmed one by one (otherwise approving accepts them together).
    pub confirm_inferred_individually: bool,
    pub allow_batch_metadata: bool,
    /// Extra room around the annotation in focused review, as a share of its size.
    pub zoom_margin: f32,
    /// Model recognition score below which an annotation counts as low confidence.
    pub low_confidence: f64,
    #[serde(default)]
    pub chars: CharPolicy,
    /// Largest tilt (degrees) the "remove tilt" suggestion offers to straighten.
    #[serde(default = "default_straighten")]
    pub straighten_max_deg: f64,
}

fn default_straighten() -> f64 {
    8.0
}

impl Default for QuickReviewPolicy {
    fn default() -> Self {
        QuickReviewPolicy {
            required_fields: ["transcription", "recognizability", "classes", "language", "direction", "char_boxes", "reading_order"].iter().map(|s| s.to_string()).collect(),
            confirm_inferred_individually: false,
            allow_batch_metadata: true,
            zoom_margin: 0.35,
            low_confidence: 0.85,
            chars: CharPolicy::default(),
            straighten_max_deg: default_straighten(),
        }
    }
}

/// Text that is about the book rather than inside the story.
pub const PUBLISHING_CLASSES: [(&str, &str, [u8; 3]); 3] = [
    ("title", "Title / logo", [255, 213, 79]),
    ("credits", "Credits & publishing", [161, 136, 127]),
    ("page_number", "Page number", [176, 190, 197]),
];

/// Text types. Handwriting, furigana and other looks are attributes, not types.
/// `secondary`: meant to be read but outside the dialogue (screens, notes, captions in the art);
/// `background`: part of the drawing, may be read but is not part of the story text.
pub const DEFAULT_CLASSES: [(&str, &str, [u8; 3]); 11] = [
    ("dialogue", "Dialogue", [66, 165, 245]),
    ("thought", "Thought", [126, 87, 194]),
    ("narration", "Narration", [38, 198, 218]),
    ("signage", "Signage", [255, 167, 38]),
    ("secondary", "Secondary text", [0, 172, 193]),
    ("background", "Background text", [141, 110, 99]),
    ("sfx", "Sound effect", [239, 83, 80]),
    ("title", "Title / logo", [255, 213, 79]),
    ("credits", "Credits & publishing", [161, 136, 127]),
    ("page_number", "Page number", [176, 190, 197]),
    ("other", "Other / unknown", [120, 144, 156]),
];

/// Optional attributes; projects add, rename or remove them. Rotation is not an attribute: it is
/// part of the orientation, like mirroring.
pub const DEFAULT_ATTRIBUTES: [&str; 6] = ["handwritten", "furigana", "monospace", "stylized", "outlined", "curved"];

/// Old text types that became attributes or merged into other types (schema 1.1 → 1.2).
pub const RETIRED_CLASSES: [(&str, Option<&str>, Option<&str>); 3] = [
    // (old class, new class, attribute added)
    ("handwritten", None, Some("handwritten")),
    ("decorative", Some("background"), None),
    ("screen", Some("secondary"), None),
];

impl Default for ProjectConfig {
    fn default() -> Self {
        let mut levels = BTreeMap::new();
        for (l, r) in [
            (Level::Page, Requirement::Required),
            (Level::Panel, Requirement::Optional),
            (Level::Container, Requirement::Optional),
            (Level::Region, Requirement::Optional),
            (Level::Line, Requirement::Required),
            (Level::Word, Requirement::Disabled),
            (Level::Char, Requirement::Optional),
        ] {
            levels.insert(l, r);
        }
        ProjectConfig {
            levels,
            level_overrides: Vec::new(),
            text_classes: DEFAULT_CLASSES.iter().map(|(id, n, c)| ClassDef { id: id.to_string(), name: n.to_string(), color: *c }).collect(),
            attributes: DEFAULT_ATTRIBUTES.iter().map(|s| s.to_string()).collect(),
            languages: vec!["ja".into(), "zh".into(), "ko".into(), "en".into(), "fr".into()],
            default_language: "ja".into(),
            default_script: "Jpan".into(),
            default_writing_mode: WritingMode::VerticalRl,
            default_direction: Direction::Ttb,
            custom_fields: Vec::new(),
            review: ReviewPolicy::default(),
            model: ModelSettings::default(),
            validation: BTreeMap::new(),
            export: ExportSettings::default(),
            annotation_license: String::new(),
            reading: ReadingConventions::default(),
            quick: QuickReviewPolicy::default(),
        }
    }
}

impl ProjectConfig {
    pub fn requirement(&self, level: Level, classes: &BTreeSet<String>) -> Requirement {
        // the strictest matching class override wins
        let mut best: Option<Requirement> = None;
        for o in &self.level_overrides {
            if o.level == level && classes.contains(&o.class) {
                best = Some(match (best, o.requirement) {
                    (Some(Requirement::Required), _) | (_, Requirement::Required) => Requirement::Required,
                    (Some(Requirement::Optional), _) | (_, Requirement::Optional) => Requirement::Optional,
                    _ => Requirement::Disabled,
                });
            }
        }
        best.unwrap_or_else(|| *self.levels.get(&level).unwrap_or(&Requirement::Optional))
    }

    pub fn class(&self, id: &str) -> Option<&ClassDef> {
        self.text_classes.iter().find(|c| c.id == id)
    }

    pub fn state_enabled(&self, s: ReviewState) -> bool {
        self.review.enabled_states.contains(&s)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub format: String,
    pub schema_version: String,
    pub project_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub created: Stamp,
    pub config: ProjectConfig,
    pub pages: Vec<PageEntry>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

/// Write any value as deterministic, diff-friendly JSON.
pub fn to_json<T: Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("serialisable");
    s.push('\n');
    s
}

/// Schema compatibility: same major version is readable; older majors need a migration.
pub fn check_version(v: &str) -> Result<(), String> {
    let major = |s: &str| s.split('.').next().and_then(|m| m.parse::<u32>().ok());
    match (major(v), major(SCHEMA_VERSION)) {
        (Some(a), Some(b)) if a == b => Ok(()),
        (Some(a), Some(b)) if a < b => Err(format!("schema {v} is older than {SCHEMA_VERSION}; run the migration first")),
        _ => Err(format!("schema {v} is newer than this app understands ({SCHEMA_VERSION})")),
    }
}

/// Migration hook: upgrades older files in memory. Schema 1.x has no predecessors yet.
pub fn migrate(value: Value) -> Result<Value, String> {
    let v = value.get("schema_version").and_then(|v| v.as_str()).unwrap_or("0").to_string();
    check_version(&v)?;
    Ok(value)
}
