//! Documentation written into every project so the data stays usable without the app.

use std::path::Path;

pub const GITIGNORE: &str = "# LabelDesk local state (recovery copies, session lock)\n.labeldesk/\n";

pub const GITATTRIBUTES: &str = "# Canonical annotation files are text; keep line endings stable.\n*.json text eol=lf\n*.jsonl text eol=lf\n# Large binaries: uncomment to store them with Git LFS.\n# images/** filter=lfs diff=lfs merge=lfs -text\n# masks/** filter=lfs diff=lfs merge=lfs -text\n# releases/** filter=lfs diff=lfs merge=lfs -text\n";

pub const FORMAT_MD: &str = r#"# Comic OCR dataset format (schema 1.2)

This folder is a LabelDesk project. Everything here is plain JSON or PNG and can be read
without LabelDesk. The JSON Schemas in `schema/` describe each file.

## Files

| Path | Content |
|---|---|
| `project.json` | Manifest: project id, configuration (annotation levels, classes, fields, review policy, model), page list with image checksums and source/licence information |
| `annotations/<page-id>.json` | Canonical annotations of one page (one file per source image) |
| `images/` | Imported original pictures. Never modified by the app |
| `history/<page-id>.jsonl` | Append-only change log, one JSON object per changed annotation per save (`before` / `after`) |
| `masks/` | Mask PNGs referenced by annotations (`masks[].path`), never embedded in JSON |
| `releases/<release-id>/` | Dataset releases: exports plus a release manifest with checksums |
| `.labeldesk/` | Local recovery copies and session lock; not part of the dataset (git-ignored) |

## Serialization rules

* UTF-8, two-space indentation, `\n` line endings, trailing newline.
* Field order is fixed by the schema; maps are sorted by key; annotations keep creation order.
* Coordinates are rounded to 0.01 px. Empty optional fields are omitted.
* IDs never change once assigned: annotations `a-<UUIDv7>`, inference runs `run-<UUIDv7>`,
  pages `<file-stem-slug>-<first 8 hex of the image SHA-256>`.
* Unknown fields are preserved on load and written back unchanged.
* `schema_version` is semantic: same major = compatible; an older major must be migrated
  (LabelDesk refuses to open newer majors rather than lose data).

## Coordinates

* All geometry is in pixels of the original image in its stored orientation (no EXIF rotation applied).
* Origin at the top-left corner of the top-left pixel; x to the right, y down.
  Pixel (i, j) covers [i, i+1) × [j, j+1); its centre is (i+0.5, j+0.5).
* `box`: `x, y` top-left, `w, h` size, `rotation_deg` clockwise about the box centre.
  Corners are TL, TR, BR, BL of the unrotated box, then rotated.
* `polygon`: simple (non self-intersecting) ring, clockwise on screen, no repeated first point.
* `quad`: exactly four corners ordered top-left, top-right, bottom-right, bottom-left
  **as read in the text's own frame** (so a rotated or mirrored sign keeps its logical corners),
  which is clockwise on screen.
* Points on the right/bottom edge (x = width, y = height) are inside the image.

## Annotation levels

`page` (the image), `panel`, `container` (bubble, caption box, sign), `region`, `line`
(a line or a vertical column), `word`, `char` (one grapheme cluster). Levels are linked with
`parent`; any other links go in `relations` (`continues`, `same_speaker`, `duplicate_of`, …).
No fixed hierarchy is enforced: a line may sit directly in a panel or on the page.
Each project sets every level to `required`, `optional` or `disabled`, optionally per text class
(`config.level_overrides`). A required finer text level (e.g. `char`) means each annotation of
the next coarser enabled text level must have children of that level.

## Transcription

* `text` is the exact visible text, punctuation as printed, `\n` between lines.
* `normalized` is optional. `language` is BCP 47, `script` is ISO 15924.
* `writing_mode` (`horizontal_tb`, `vertical_rl`, `vertical_lr`, `custom`) and `direction`
  (`ltr`, `rtl`, `ttb`, `btt`) are independent of the geometric rotation in `orientation`.
* `spans` mark `uncertain` or `unreadable` parts. Offsets count **extended grapheme clusters**
  (what a reader sees as one character, including combining marks, emoji sequences and
  ligature clusters), end exclusive. Nobody is asked to guess hidden text:
  `transcription.source = inferred` records that some text was inferred, and occluded text can
  simply be left out and marked with `occlusion`.
* `alternatives` hold other readings with who proposed them (people or models).
* Translations and transliterations live in `translations`, never in `transcription`.
* Character annotations (`char`) carry one grapheme cluster in `text`.

## Reading order

`reading_order` is an integer (from 1) among annotations with the same `parent` **and the same
level** (panels among panels, balloons among balloons, lines within their balloon or region,
characters within their line). It is set by people and never derived from position alone (the
app may *suggest* an order, which a person must confirm). By default the suggestion is made in
the last review pass of a page, once every box exists (`config.reading.auto_suggest = false`).

Some text is read outside the sequence and has no `reading_order`: furigana (always), and the
types listed in `config.reading.unordered_classes` (by default `background`, `sfx`,
`page_number`).

## Text types and attributes

`classes` holds the text type (one is usual). Default types: `dialogue`, `thought`, `narration`,
`signage`, `secondary` (*Secondary text*: meant to be read but outside the story text, such as
screens, notes or labels in the art), `background` (*Background text*: part of the drawing, may
be read), `sfx`, `title`, `credits`, `page_number`, `other`. Projects add, rename or remove types.

`attributes` are optional properties, any number per label, configured per project
(`config.attributes`). Defaults: `handwritten`, `furigana`, `monospace` (every character takes
the same room; character boxes are split evenly), `stylized`, `outlined`, `curved`. Rotation and
mirroring are not attributes: they are part of `orientation`.

Schema 1.2 retired three types: `handwritten` became an attribute, `decorative` became
`background`, `screen` became `secondary` (Project settings offers the update).

`tags` are free words people add while working (`check-later`, `odd-font`, …). They help
search and review and carry no meaning for training.

## Quality fields

Kept separate on purpose:
* `recognizability`: `readable` (perfectly readable), `partially_readable`, `ambiguous`,
  `unreadable`, or the quick two-way answer `not_perfect` (not perfectly readable), which
  still has to be refined into one of the three before approval.
* `quality`: set of `blur`, `noise`, `low_resolution`, `low_contrast`, `compression_artifacts`,
  `clipping`, `halftone_interference`, `artistic_deformation`.
* `confidence`: the annotator's own certainty (`low`/`medium`/`high`) for `transcription`,
  `localization`, `reading_order`, `geometry`; `high` reads as *certain*. The quick two-way
  answer is `high` or `uncertain`, and `uncertain` is refined into `low` or `medium` later.
  Never merged with model scores.
* `occlusion`: `occluded`, optional `kind`, optional `fraction` (0–1); masks in `masks`.

## Perspective rectification

`rectification.role` says whether the quadrilateral encloses the text region (`text_region`)
or outlines the physical surface carrying it (`surface`). `source` holds four corners
(TL, TR, BR, BL), `target_size` the flattened width and height. These are the primary data;
`homography` is a cached 3×3 row-major matrix (h33 = 1) mapping `source` to
(0,0), (w,0), (w,h), (0,h). Rectification never changes the image or `geometry`.
`deformation` is reserved for baselines, text boundaries and meshes in later versions.

## Provenance

`provenance.origin` is `manual`, `ai`, `imported` or `sample`. AI annotations point to the
page's `inference_runs[]` entry (`run_id`, `prediction`), which records the model (repository,
version, revision, SHA-256 of every artifact), the full preprocessing/post-processing
configuration, time, the scope (page, region, or a single annotation with the transform used)
and every raw prediction with its model-native scores. Scores are **not** calibrated
probabilities. Raw predictions are never edited; only their `status` changes.
`provenance.fields` tracks each field group (`geometry`, `transcription`, `reading_order`,
`metadata`): `proposed`, `accepted`, `corrected`, `manual`, `verified`,
`changed_after_verification`.

## Review

States: `ai_proposed`, `draft`, `submitted`, `changes_requested`, `human_reviewed`,
`peer_verified`, `gold_approved` (a project may disable some). Approvals are given per
dimension (`transcription`, `geometry`, `reading_order`) by a named reviewer, for a given
annotation `revision` and review-policy `version`. Editing an approved aspect invalidates or
flags its approvals according to `config.review.on_edit_after_approval`; invalidated
approvals stay in the record with `invalidated`. Changing the policy never upgrades or drops
earlier approvals; they keep the policy version they were given under.
`notes` hold comments, change requests, disagreements (to adjudicate) and rejections.
Pages carry `completeness` (inspected for missing text, complete, `no_text`) and
`review.approved_for_export`.

## Assisted annotation (schema 1.1, 1.2)

* **Heuristic character boxes**: `provenance.origin = heuristic`, field state
  `heuristic_proposal`, review state `ai_proposed` until a person accepts them. `provenance.heuristic`
  records the algorithm (`labeldesk.char_split`) and version, the source annotation, the exact
  source text, its revision, and the grapheme-cluster `span` the box stands for, plus the `frame`
  used (`rectified` through a homography, `quad`, or `axis`). A box is stale when the source text
  changed or the box left its line; regeneration replaces only untouched proposals. `cell` keeps
  the box's place in its line's frame (`[u0, v0, u1, v1]` fractions), so boxes a person sized one
  by one keep their proportions when the line is resized. `pinned: true` marks a box whose size a
  person fixed; automatic redistribution of the line (Alt+drag) leaves it alone. A proposal a
  person resized stays a proposal (`ai_proposed`, geometry `corrected`) until it is accepted.
* **Upright numbers in vertical text** (tate-chu-yoko): two or three half-width digits (and
  `!!`, `!?`, `?!`, `??`) share one character cell, split side by side and read left to right.
* **Furigana**: attribute `furigana` and a relation `furigana_of` pointing at the characters, word
  or line it reads. Furigana has no `reading_order`; it is read with its base.
* **Reading order**: `reading.source` is `manual` or `suggested`; suggestions never overwrite manual
  numbers. `reading.mark` is `ambiguous` or `unordered`; `reading.progression` and
  `reading.writing_mode` override the conventions inside a region. Project conventions live in
  `config.reading` (page direction, layout strategy `tiers` / `columns` / `position`); a page's
  `source.reading_direction` (`rtl` / `ltr`) overrides the page direction.
* **Quick-review answers**: `checks.<field>` with `state` `confirmed`, `unknown`,
  `not_applicable` or `inferred` (with its `source`), who and when, and `batch` when decided for
  several annotations at once. Unknown and not-applicable are different answers. Fields:
  geometry, transcription, recognizability, classes, language, direction, mirrored, occlusion,
  quality, char_boxes, reading_order, perspective, confidence. `config.quick` says which are required and whether
  inferred values need individual confirmation. `occlusion.level` is `partial` or `severe`.
* Review sessions are personal and local (`.labeldesk/review/<user>.json`): the passes chosen,
  the pages in scope and the position; they are not dataset data.

## Version control

Projects work well in Git: one file per page, stable key order, append-only change logs.
LabelDesk commits a page (its annotation file, change log, masks and `project.json`) when the
page's review is complete; pages still being worked on stay as uncommitted changes. Other files
are never swept into those commits.

## Conflicts

When a page file changed on disk (for example after `git pull`) or another copy is imported,
LabelDesk merges per annotation. Where both sides changed, this copy wins and the other version
is stored in `conflicts[]` until a person settles it. Reviewed data is never dropped silently.

## Exports

Exports are written to `releases/` and never change the canonical files. Each release has a
`release.json` with its id, schema version, export settings, review policy, approval summary,
train/validation/test assignment (split by work, so pages of one work never cross splits;
declared duplicates follow their original) and SHA-256 of every file. `OMISSIONS.md` in each
release lists what each secondary format leaves out.
"#;

pub const PAGE_SCHEMA: &str = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "labeldesk.comic-ocr.page/1",
  "title": "LabelDesk page annotations (schema 1.x)",
  "type": "object",
  "required": ["format", "schema_version", "page_id", "image", "annotations"],
  "properties": {
    "format": {"const": "labeldesk.comic-ocr.page"},
    "schema_version": {"type": "string", "pattern": "^1\\.\\d+\\.\\d+$"},
    "page_id": {"type": "string"},
    "image": {"type": "object", "required": ["path", "sha256", "width", "height"],
      "properties": {"path": {"type": "string"}, "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
        "width": {"type": "integer", "minimum": 1}, "height": {"type": "integer", "minimum": 1}}},
    "completeness": {"type": "object", "properties": {"inspected": {"type": "boolean"}, "complete": {"type": "boolean"},
      "no_text": {"type": "boolean"}, "checked": {"$ref": "#/$defs/stamp"}, "note": {"type": "string"}}},
    "review": {"type": "object", "properties": {"approved_for_export": {"$ref": "#/$defs/stamp"}}},
    "annotations": {"type": "array", "items": {"$ref": "#/$defs/annotation"}},
    "inference_runs": {"type": "array", "items": {"$ref": "#/$defs/run"}},
    "conflicts": {"type": "array"}
  },
  "$defs": {
    "point": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2},
    "quad": {"type": "array", "items": {"$ref": "#/$defs/point"}, "minItems": 4, "maxItems": 4},
    "stamp": {"type": "object", "required": ["by", "at"], "properties": {"by": {"type": "string"}, "at": {"type": "string", "format": "date-time"}}},
    "geometry": {"oneOf": [
      {"type": "object", "required": ["type", "x", "y", "w", "h"], "properties": {"type": {"const": "box"},
        "x": {"type": "number"}, "y": {"type": "number"}, "w": {"type": "number", "minimum": 0}, "h": {"type": "number", "minimum": 0}, "rotation_deg": {"type": "number"}}},
      {"type": "object", "required": ["type", "points"], "properties": {"type": {"const": "polygon"}, "points": {"type": "array", "items": {"$ref": "#/$defs/point"}, "minItems": 3}}},
      {"type": "object", "required": ["type", "points"], "properties": {"type": {"const": "quad"}, "points": {"$ref": "#/$defs/quad"}}}
    ]},
    "level": {"enum": ["page", "panel", "container", "region", "line", "word", "char"]},
    "dimension": {"enum": ["transcription", "geometry", "reading_order"]},
    "transcription": {"type": "object", "required": ["text"], "properties": {
      "text": {"type": "string"}, "normalized": {"type": "string"}, "language": {"type": "string"}, "script": {"type": "string", "pattern": "^[A-Z][a-z]{3}$"},
      "writing_mode": {"enum": ["horizontal_tb", "vertical_rl", "vertical_lr", "custom"]},
      "direction": {"enum": ["ltr", "rtl", "ttb", "btt"]},
      "spans": {"type": "array", "items": {"type": "object", "required": ["start", "end", "kind"], "properties": {
        "start": {"type": "integer", "minimum": 0}, "end": {"type": "integer", "minimum": 0}, "kind": {"enum": ["uncertain", "unreadable"]}, "note": {"type": "string"}}}},
      "alternatives": {"type": "array", "items": {"type": "object", "required": ["text", "by", "at"]}},
      "source": {"enum": ["visible", "inferred"]}}},
    "approval": {"type": "object", "required": ["dimension", "by", "at", "revision", "policy_version"], "properties": {
      "dimension": {"$ref": "#/$defs/dimension"}, "by": {"type": "string"}, "at": {"type": "string"}, "revision": {"type": "integer"}, "policy_version": {"type": "integer"},
      "invalidated": {"type": "object", "required": ["at", "by", "reason"]}}},
    "annotation": {"type": "object", "required": ["id", "type", "page_id", "geometry", "created", "modified", "revision"], "properties": {
      "id": {"type": "string"}, "type": {"$ref": "#/$defs/level"}, "page_id": {"type": "string"}, "parent": {"type": "string"},
      "relations": {"type": "array", "items": {"type": "object", "required": ["kind", "target"]}},
      "geometry": {"$ref": "#/$defs/geometry"},
      "rectification": {"type": "object", "required": ["role", "source", "target_size"], "properties": {
        "role": {"enum": ["text_region", "surface"]}, "source": {"$ref": "#/$defs/quad"},
        "target_size": {"type": "array", "items": {"type": "integer", "minimum": 1}, "minItems": 2, "maxItems": 2},
        "target_size_manual": {"type": "boolean"},
        "homography": {"type": "array", "items": {"type": "number"}, "minItems": 9, "maxItems": 9}}},
      "classes": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
      "attributes": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
      "tags": {"type": "array", "items": {"type": "string"}, "uniqueItems": true},
      "transcription": {"$ref": "#/$defs/transcription"},
      "translations": {"type": "array", "items": {"type": "object", "required": ["kind", "language", "text", "by"], "properties": {"kind": {"enum": ["translation", "transliteration"]}}}},
      "recognizability": {"enum": ["readable", "partially_readable", "ambiguous", "unreadable", "not_perfect"]},
      "quality": {"type": "array", "items": {"enum": ["blur", "noise", "low_resolution", "low_contrast", "compression_artifacts", "clipping", "halftone_interference", "artistic_deformation"]}, "uniqueItems": true},
      "orientation": {"type": "object", "properties": {"rotation_deg": {"type": "number"}, "mirrored": {"type": "boolean"}, "skew_deg": {"type": "number"}}},
      "reading_order": {"type": "integer", "minimum": 1},
      "occlusion": {"type": "object", "required": ["occluded"], "properties": {"occluded": {"type": "boolean"}, "kind": {"type": "string"}, "fraction": {"type": "number", "minimum": 0, "maximum": 1}}},
      "confidence": {"type": "object", "additionalProperties": {"enum": ["low", "medium", "high", "uncertain"]}},
      "masks": {"type": "array", "items": {"type": "object", "required": ["role", "path", "sha256", "offset", "size"]}},
      "deformation": {},
      "fields": {"type": "object", "propertyNames": {"pattern": "^[a-z0-9_-]+:[a-z0-9_.-]+$"}},
      "provenance": {"type": "object", "required": ["origin"], "properties": {
        "origin": {"enum": ["manual", "ai", "imported", "sample"]},
        "ai": {"type": "object", "required": ["run_id", "prediction", "model", "model_version", "model_revision"]},
        "fields": {"type": "object", "additionalProperties": {"enum": ["proposed", "accepted", "corrected", "manual", "verified", "changed_after_verification"]}}}},
      "review": {"type": "object", "required": ["state"], "properties": {
        "state": {"enum": ["ai_proposed", "draft", "submitted", "changes_requested", "human_reviewed", "peer_verified", "gold_approved"]},
        "approvals": {"type": "array", "items": {"$ref": "#/$defs/approval"}},
        "notes": {"type": "array", "items": {"type": "object", "required": ["kind", "by", "at", "text"], "properties": {"kind": {"enum": ["comment", "change_request", "disagreement", "rejection"]}}}},
        "needs_rereview": {"type": "boolean"},
        "gold": {"type": "object", "required": ["by", "at", "revision", "policy_version"]}}},
      "created": {"$ref": "#/$defs/stamp"}, "modified": {"$ref": "#/$defs/stamp"}, "revision": {"type": "integer", "minimum": 1}
    }},
    "run": {"type": "object", "required": ["id", "at", "by", "model", "config", "scope", "image_sha256", "predictions"], "properties": {
      "model": {"type": "object", "required": ["adapter", "name", "repo", "version", "revision", "artifacts"]},
      "scope": {"type": "object", "required": ["kind"], "properties": {"kind": {"enum": ["page", "region", "annotation"]}}},
      "predictions": {"type": "array", "items": {"type": "object", "required": ["index", "quad", "text", "status"], "properties": {
        "quad": {"$ref": "#/$defs/quad"}, "status": {"enum": ["proposed", "skipped_overlap", "rejected", "alternative"]}}}}}}
  }
}
"##;

pub const PROJECT_SCHEMA: &str = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "labeldesk.comic-ocr.project/1",
  "title": "LabelDesk project manifest (schema 1.x)",
  "type": "object",
  "required": ["format", "schema_version", "project_id", "name", "created", "config", "pages"],
  "properties": {
    "format": {"const": "labeldesk.comic-ocr.project"},
    "schema_version": {"type": "string", "pattern": "^1\\.\\d+\\.\\d+$"},
    "project_id": {"type": "string"},
    "name": {"type": "string"},
    "config": {"type": "object", "required": ["levels", "text_classes"], "properties": {
      "levels": {"type": "object", "additionalProperties": {"enum": ["required", "optional", "disabled"]}},
      "level_overrides": {"type": "array", "items": {"type": "object", "required": ["class", "level", "requirement"]}},
      "text_classes": {"type": "array", "items": {"type": "object", "required": ["id", "name", "color"]}},
      "custom_fields": {"type": "array", "items": {"type": "object", "required": ["key", "label", "kind"], "properties": {
        "key": {"type": "string", "pattern": "^[a-z0-9_-]+:[a-z0-9_.-]+$"}, "kind": {"enum": ["text", "number", "bool", "choice"]}}}},
      "review": {"type": "object", "required": ["version", "reviewers_required", "required_dimensions", "enabled_states", "on_edit_after_approval"]},
      "model": {"type": "object", "properties": {"enabled": {"type": "boolean"}, "auto_run_on_import": {"type": "boolean"}, "pinned": {"type": "object"}}},
      "validation": {"type": "object", "additionalProperties": {"enum": ["error", "warning", "off"]}},
      "export": {"type": "object"}}},
    "pages": {"type": "array", "items": {"type": "object", "required": ["id", "image", "sha256", "width", "height", "encoding", "annotation_file", "added"], "properties": {
      "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
      "source": {"type": "object", "properties": {"work": {"type": "string"}, "series": {"type": "string"}, "volume": {"type": "string"},
        "page_number": {"type": "string"}, "license": {"type": "string"}, "restricted": {"type": "boolean"}, "duplicate_of": {"type": "string"}}}}}}
  }
}
"##;

pub fn write_docs(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root.join("schema"))?;
    std::fs::write(root.join("FORMAT.md"), FORMAT_MD)?;
    std::fs::write(root.join("schema/page.schema.json"), PAGE_SCHEMA)?;
    std::fs::write(root.join("schema/project.schema.json"), PROJECT_SCHEMA)?;
    if !root.join(".gitignore").exists() {
        std::fs::write(root.join(".gitignore"), GITIGNORE)?;
    }
    if !root.join(".gitattributes").exists() {
        std::fs::write(root.join(".gitattributes"), GITATTRIBUTES)?;
    }
    Ok(())
}
