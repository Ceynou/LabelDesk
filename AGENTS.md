# AGENTS.md — working on LabelDesk

Guidance for people and coding agents changing this repository.

## Build, test, check

```sh
cargo build                                 # must succeed with no warnings
cargo test                                  # all tests must pass (about 6 s)
cargo run -- --screenshot shots/review.png  # renders ~120 frames, saves a PNG, exits
```

- The screenshot run removes `WAYLAND_DISPLAY` so it renders through X11/XWayland without vsync;
  it works with the screen off.
- Tests create projects in the system temp folder; never point them at real data.
- Never start long-running servers; the app is a desktop program.

## Layout

| Path | What lives there |
|---|---|
| `src/main.rs` | command line (`[project]`, `--screenshot out.png`), window setup |
| `src/schema.rs` | the data format: annotations, pages, project config, `SCHEMA_VERSION`, defaults (text types, attributes) |
| `src/store.rs` | project folders: open/create/import (incl. JPEG XL), atomic saves, history, recovery, merge |
| `src/review.rs` | review rules: `on_edit` (what an edit invalidates), approvals, notes |
| `src/validate.rs` | validation rules (`RULES`), severities per project |
| `src/assist.rs` | assisted annotation: character proposals (incl. monospace and tate-chu-yoko), reading order, quick-review needs, review workflow passes (`Pass`, `Workflow`, `page_steps`), grouping, shape helpers, taxonomy migration |
| `src/ai.rs`, `src/ocr.rs`, `src/ctc_lm.rs` | background OCR worker, PP-OCRv6 manga adapter, CTC + language-model decoder |
| `src/models.rs` | model capabilities (lines, reading, characters, regions, balloons, panels, masks, order) and known/planned models |
| `src/git.rs` | committing reviewed pages (only that page's files) |
| `src/export.rs` | releases (native, JSONL, COCO, crops) and imports |
| `src/docs.rs` | `FORMAT.md` and JSON Schemas written into projects |
| `src/app.rs` | the `App` state, settings, shortcuts (`ACTIONS`), undo/redo, frame layout |
| `src/app/canvas.rs` | drawing, hit testing, handles, transforms, level of detail |
| `src/app/review_mode.rs` | Review Mode logic: sessions, steps, answers, keys |
| `src/app/review_cards.rs` | Review Mode screens: setup, pass bar, one card per pass |
| `src/app/find.rs` | multi-criteria search, bulk edits, hide, problems, re-read, Git menu |
| `src/app/inspector.rs`, `panels.rs`, `batch.rs`, `dialogs.rs` | side panels, menus, batch tools, dialogs |
| `src/app/tests.rs`, `src/assist_tests.rs`, `src/acceptance.rs` | interface tests (simulated input), assisted-annotation and acceptance tests |

## Rules that keep the data trustworthy

- **Proposals are never ground truth.** Anything from a model or a heuristic carries
  `provenance.origin` `ai`/`heuristic` and review state `ai_proposed` until a person accepts it.
  Resizing a proposal keeps it a proposal.
- **Edits go through `review::on_edit`** (via `App::commit` / `App::op` / `App::op_on`), which bumps
  the revision, records field states and invalidates approvals. Do not mutate pages outside
  these paths in the UI.
- **One undo step per operation.** Use `App::op` for the current page and `App::op_on` for others.
- **Deterministic files.** Serialize with `schema::to_json`; keep new fields
  `#[serde(default, skip_serializing_if = ...)]` so older files load and diffs stay small.
  Bump `SCHEMA_VERSION` (minor for additions) and document the field in `docs.rs`
  (`FORMAT_MD` and the schema) in the same change.
- **Grapheme clusters, not chars**, for every text offset.
- **Git**: the app commits a page only when its review is complete, and only that page's files.

## Adding things

- **A shortcut**: add it to `ACTIONS` in `app.rs` (it then appears in the shortcut editor) and
  check it with `App::shortcut`. Longer combinations must be checked before shorter ones
  (Ctrl+Shift+R before Ctrl+R).
- **A review pass**: add a `Pass` variant (name, hint, `per_page`, `edits_shapes`), its items in
  `assist::pass_items` / done test in `pass_checked`, its Enter behaviour in
  `App::review_confirm`, and its card in `review_cards.rs`.
- **A model**: implement an adapter (see `ocr::OcrAdapter`), declare its `Capability` list in
  `models.rs`, pin its revision and artifact hashes per project, and write its output as an
  inference run whose predictions become proposals at the matching level.
- **A validation rule**: add it to `validate::RULES` with a default severity and a test.

## Style

- Rust 2024, egui/eframe 0.37 (`App::ui`, `Panel::…show(ui, …)`, `ui.close()`).
- Comments explain *why*, briefly; user-facing text is plain language for non-programmers.
- Commit messages: imperative summary line, then details; end with the co-author line used in
  this repository's history.
