//! What AI models can do for a project, and which ones are known.
//!
//! Every model is wrapped by an adapter that declares its capabilities. The app asks "who can
//! detect panels?" rather than calling a model by name, so new models plug in without touching
//! the editor. Whatever a model produces enters as a proposal (`origin: ai`, unreviewed) through
//! the same path as OCR results: inference runs on the page, then reviewed by a person.

use crate::schema::Level;

/// One kind of work a model can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    /// Find text lines (boxes or quadrilaterals).
    LineDetection,
    /// Read the text of a line image.
    Recognition,
    /// Find characters inside a line.
    CharDetection,
    /// Group text into regions / blocks.
    RegionDetection,
    /// Find balloons, captions and other text containers.
    ContainerDetection,
    /// Find panels.
    PanelDetection,
    /// Pixel masks of the text (for cleaning, occlusion and ink).
    TextMask,
    /// Order the labels of a page.
    ReadingOrder,
}

impl Capability {
    pub const ALL: [Capability; 8] = [
        Capability::LineDetection,
        Capability::Recognition,
        Capability::CharDetection,
        Capability::RegionDetection,
        Capability::ContainerDetection,
        Capability::PanelDetection,
        Capability::TextMask,
        Capability::ReadingOrder,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Capability::LineDetection => "Text lines",
            Capability::Recognition => "Reading text",
            Capability::CharDetection => "Characters",
            Capability::RegionDetection => "Text regions",
            Capability::ContainerDetection => "Balloons and captions",
            Capability::PanelDetection => "Panels",
            Capability::TextMask => "Text masks",
            Capability::ReadingOrder => "Reading order",
        }
    }
    /// The annotation level its proposals are made at, if it proposes shapes.
    pub fn level(self) -> Option<Level> {
        match self {
            Capability::LineDetection => Some(Level::Line),
            Capability::CharDetection => Some(Level::Char),
            Capability::RegionDetection => Some(Level::Region),
            Capability::ContainerDetection => Some(Level::Container),
            Capability::PanelDetection => Some(Level::Panel),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Runs in the app today.
    Available,
    /// Known and planned; not wired in yet.
    Planned,
}

pub struct KnownModel {
    pub id: &'static str,
    pub name: &'static str,
    pub capabilities: &'static [Capability],
    pub status: Status,
    pub note: &'static str,
}

/// Models the app knows about. Adding one: implement `ocr::OcrAdapter` (or a sibling trait for
/// shapes and masks), list it here with its capabilities, and pin its exact revision per project.
pub const KNOWN: [KnownModel; 5] = [
    KnownModel {
        id: "Kellenok/PP-OCRv6_manga",
        name: "PP-OCRv6 manga (v0.3, with language model)",
        capabilities: &[Capability::LineDetection, Capability::Recognition],
        status: Status::Available,
        note: "Runs offline with ONNX Runtime; detection and recognition with the manga language model.",
    },
    KnownModel {
        id: "koharu/text-sam-ts-l",
        name: "Koharu text segmentation",
        capabilities: &[Capability::TextMask],
        status: Status::Planned,
        note: "Pixel masks of text for occlusion, ink and cleaning.",
    },
    KnownModel {
        id: "koharu/layout-highres",
        name: "Koharu comic layout",
        capabilities: &[Capability::PanelDetection, Capability::ContainerDetection, Capability::RegionDetection, Capability::ReadingOrder],
        status: Status::Planned,
        note: "Panels, balloons and text blocks with an order; proposals at the matching levels.",
    },
    KnownModel {
        id: "manga-ocr-ar-experimental",
        name: "Manga OCR (autoregressive, experimental)",
        capabilities: &[Capability::Recognition],
        status: Status::Planned,
        note: "A second reading to compare against; disagreements become review notes.",
    },
    KnownModel {
        id: "muscgab/manga-ocr-charspot-small",
        name: "Manga OCR charspot",
        capabilities: &[Capability::CharDetection, Capability::Recognition],
        status: Status::Planned,
        note: "Character positions inside a line, replacing the even split.",
    },
];

/// Known models that can do `c`.
pub fn providers(c: Capability) -> impl Iterator<Item = &'static KnownModel> {
    KNOWN.iter().filter(move |m| m.capabilities.contains(&c))
}
