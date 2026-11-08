use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LabelClass {
    pub name: String,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Shape {
    Rect { min: [f32; 2], max: [f32; 2] },
    Polygon { points: Vec<[f32; 2]> },
    Polyline { points: Vec<[f32; 2]> },
    Point { at: [f32; 2] },
}

impl Shape {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => "Box",
            Shape::Polygon { .. } => "Polygon",
            Shape::Polyline { .. } => "Line",
            Shape::Point { .. } => "Point",
        }
    }

    /// Axis-aligned bounds as (min, max).
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let pts: Vec<[f32; 2]> = match self {
            Shape::Rect { min, max } => vec![*min, *max],
            Shape::Polygon { points } | Shape::Polyline { points } => points.clone(),
            Shape::Point { at } => vec![*at],
        };
        let mut lo = [f32::MAX, f32::MAX];
        let mut hi = [f32::MIN, f32::MIN];
        for p in pts {
            lo[0] = lo[0].min(p[0]);
            lo[1] = lo[1].min(p[1]);
            hi[0] = hi[0].max(p[0]);
            hi[1] = hi[1].max(p[1]);
        }
        (lo, hi)
    }

    pub fn translate(&mut self, d: [f32; 2]) {
        let mv = |p: &mut [f32; 2]| {
            p[0] += d[0];
            p[1] += d[1];
        };
        match self {
            Shape::Rect { min, max } => {
                mv(min);
                mv(max);
            }
            Shape::Polygon { points } | Shape::Polyline { points } => points.iter_mut().for_each(mv),
            Shape::Point { at } => mv(at),
        }
    }

    pub fn clamp_to(&mut self, w: f32, h: f32) {
        let cl = |p: &mut [f32; 2]| {
            p[0] = p[0].clamp(0.0, w);
            p[1] = p[1].clamp(0.0, h);
        };
        match self {
            Shape::Rect { min, max } => {
                cl(min);
                cl(max);
            }
            Shape::Polygon { points } | Shape::Polyline { points } => points.iter_mut().for_each(cl),
            Shape::Point { at } => cl(at),
        }
    }

    pub fn area(&self) -> f32 {
        match self {
            Shape::Rect { min, max } => (max[0] - min[0]).abs() * (max[1] - min[1]).abs(),
            Shape::Polygon { points } => {
                let n = points.len();
                if n < 3 {
                    return 0.0;
                }
                let mut s = 0.0;
                for i in 0..n {
                    let a = points[i];
                    let b = points[(i + 1) % n];
                    s += a[0] * b[1] - b[0] * a[1];
                }
                (s * 0.5).abs()
            }
            _ => 0.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub label: usize,
    pub shape: Shape,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Todo,
    InProgress,
    Done,
    Skipped,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Todo => "To do",
            Status::InProgress => "In progress",
            Status::Done => "Done",
            Status::Skipped => "Skipped",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImageItem {
    pub name: String,
    /// File on disk. None for the built-in sample pictures.
    pub path: Option<String>,
    /// Seed of a built-in sample picture.
    #[serde(default)]
    pub sample: Option<u32>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: Status,
}

impl ImageItem {
    pub fn touch(&mut self) {
        if self.status == Status::Todo && (!self.annotations.is_empty() || !self.tags.is_empty()) {
            self.status = Status::InProgress;
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub labels: Vec<LabelClass>,
    /// Picture-level tags (choices), e.g. "Daytime", "Blurry".
    #[serde(default)]
    pub tags: Vec<String>,
    pub images: Vec<ImageItem>,
    #[serde(default)]
    pub next_id: u64,
}

impl Project {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn done_count(&self) -> usize {
        self.images.iter().filter(|i| i.status == Status::Done).count()
    }

    pub fn label_count(&self, label: usize) -> usize {
        self.images
            .iter()
            .flat_map(|i| i.annotations.iter())
            .filter(|a| a.label == label)
            .count()
    }

    /// Remove a label class and fix up the indices of annotations.
    pub fn remove_label(&mut self, idx: usize) {
        if idx >= self.labels.len() {
            return;
        }
        self.labels.remove(idx);
        for img in &mut self.images {
            img.annotations.retain(|a| a.label != idx);
            for a in &mut img.annotations {
                if a.label > idx {
                    a.label -= 1;
                }
            }
        }
    }
}

pub const PALETTE: [[u8; 3]; 12] = [
    [239, 83, 80],
    [66, 165, 245],
    [102, 187, 106],
    [255, 202, 40],
    [171, 71, 188],
    [255, 138, 101],
    [38, 198, 218],
    [236, 64, 122],
    [156, 204, 101],
    [126, 87, 194],
    [255, 167, 38],
    [120, 144, 156],
];

pub const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff"];
