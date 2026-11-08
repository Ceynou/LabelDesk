# Comic OCR dataset format (schema 1.3)

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

Since schema 1.3 a prediction can be more than a text line: `level` (`panel`, `container`,
`region`, `line`, `char`; missing means a line), `class` (the text type it was given, e.g. `sfx`),
`label` (the model's own class name: `text_block`, `bubble`, `panel`, `onomatopoeia`...),
`polygon` (an outline from the model's mask, when it is not simply `quad`), `parent` (index of
another prediction of the same run it belongs to: a character's line) and `parent_id` (an
existing annotation it belongs to). `model.artifacts` lists the original files and, for
converted models, the converted copy that ran (`converted_onnx`, `ncnn_param`, `ncnn_bin`);
`model.runtime` says what ran it (ONNX Runtime on the processor, or ncnn on a named graphics
card). Text-mask runs record the page mask in `config.mask` (`path`, `sha256`, `size`) and give
lines and regions an `ink` entry in `masks` with their part of it.
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
