//! Project folders on disk: create, open, import pictures, save (atomically, with history),
//! crash recovery and merging two copies of the same page.
//!
//! Layout (Git friendly):
//! ```text
//! project.json                 manifest: configuration, review policy, page list
//! annotations/<page-id>.json   one canonical annotation file per page
//! images/                      imported pictures (never modified; Git LFS friendly)
//! history/<page-id>.jsonl      append-only change log (who changed what, before/after)
//! masks/                       mask PNGs referenced by annotations
//! schema/                      JSON Schemas for the files above
//! FORMAT.md                    human description of the format and conventions
//! releases/                    dataset releases (exports)
//! .labeldesk/                  local only: recovery copies and session lock (git-ignored)
//! ```

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::docs;
use crate::schema::*;
use crate::util::{self, atomic_write, now, rel_path, sha256_bytes};

pub const IMAGE_EXTS: [&str; 8] = ["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff", "jxl"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAction {
    Created,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub at: String,
    pub by: String,
    pub annotation_id: String,
    pub action: HistoryAction,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub before: Option<Value>,
    #[serde(default)]
    pub after: Option<Value>,
}

pub enum SaveOutcome {
    Unchanged,
    Saved,
    /// The file on disk changed since it was loaded (e.g. `git pull`); nothing was written.
    Conflict(Box<PageFile>),
}

pub struct RecoveryItem {
    pub page_id: String,
    pub page: PageFile,
    pub when: String,
}

pub struct Store {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub pages: Vec<PageFile>,
    saved: HashMap<String, PageFile>,
    disk_hash: HashMap<String, String>,
    manifest_disk_hash: String,
    pub dirty: BTreeSet<String>,
    pub manifest_dirty: bool,
    /// The previous session did not close cleanly.
    pub crashed_before: bool,
}

fn read_json(path: &Path) -> Result<(Value, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((v, sha256_bytes(&bytes)))
}

pub fn parse_page(v: Value) -> Result<PageFile, String> {
    let v = migrate(v)?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

impl Store {
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("project.json")
    }

    pub fn page_path(&self, id: &str) -> PathBuf {
        let rel = self.manifest.pages.iter().find(|p| p.id == id).map(|p| p.annotation_file.clone()).unwrap_or_else(|| format!("annotations/{id}.json"));
        self.root.join(rel)
    }

    pub fn image_path(&self, entry: &PageEntry) -> PathBuf {
        let p = PathBuf::from(&entry.image);
        if p.is_absolute() { p } else { self.root.join(p) }
    }

    pub fn recovery_dir(&self) -> PathBuf {
        self.root.join(".labeldesk").join("recovery")
    }

    pub fn create(root: &Path, name: &str, config: ProjectConfig, who: &str) -> Result<Store, String> {
        if root.join("project.json").exists() {
            return Err(format!("{} already contains a project", root.display()));
        }
        for d in ["annotations", "images", "history", "masks", "schema", "releases", ".labeldesk/recovery"] {
            std::fs::create_dir_all(root.join(d)).map_err(|e| e.to_string())?;
        }
        let manifest = Manifest {
            format: PROJECT_FORMAT.into(),
            schema_version: SCHEMA_VERSION.into(),
            project_id: util::new_id("proj"),
            name: name.into(),
            description: String::new(),
            created: Stamp { by: who.into(), at: now() },
            config,
            pages: Vec::new(),
            unknown: Default::default(),
        };
        let mut s = Store {
            root: root.to_path_buf(),
            manifest,
            pages: Vec::new(),
            saved: HashMap::new(),
            disk_hash: HashMap::new(),
            manifest_disk_hash: String::new(),
            dirty: BTreeSet::new(),
            manifest_dirty: true,
            crashed_before: false,
        };
        docs::write_docs(root).map_err(|e| e.to_string())?;
        s.save_manifest()?;
        s.lock();
        Ok(s)
    }

    pub fn open(root: &Path) -> Result<Store, String> {
        let (mv, mh) = read_json(&root.join("project.json"))?;
        let mv = migrate(mv)?;
        let manifest: Manifest = serde_json::from_value(mv).map_err(|e| format!("project.json: {e}"))?;
        let mut s = Store {
            root: root.to_path_buf(),
            manifest,
            pages: Vec::new(),
            saved: HashMap::new(),
            disk_hash: HashMap::new(),
            manifest_disk_hash: mh,
            dirty: BTreeSet::new(),
            manifest_dirty: false,
            crashed_before: false,
        };
        for entry in s.manifest.pages.clone() {
            let path = s.root.join(&entry.annotation_file);
            let page = if path.exists() {
                let (v, h) = read_json(&path)?;
                s.disk_hash.insert(entry.id.clone(), h);
                parse_page(v).map_err(|e| format!("{}: {e}", path.display()))?
            } else {
                PageFile::new(&entry.id, ImageRef { path: entry.image.clone(), sha256: entry.sha256.clone(), width: entry.width, height: entry.height })
            };
            s.saved.insert(entry.id.clone(), page.clone());
            s.pages.push(page);
        }
        s.crashed_before = s.root.join(".labeldesk/session.lock").exists();
        s.lock();
        Ok(s)
    }

    fn lock(&self) {
        let p = self.root.join(".labeldesk/session.lock");
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, format!("{}\n{}\n", std::process::id(), now()));
    }

    /// Clean shutdown: remove the session lock.
    pub fn unlock(&self) {
        let _ = std::fs::remove_file(self.root.join(".labeldesk/session.lock"));
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.page_id == id)
    }

    pub fn entry(&self, id: &str) -> Option<&PageEntry> {
        self.manifest.pages.iter().find(|p| p.id == id)
    }

    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty() || self.manifest_dirty
    }

    // ------------------------------------------------------------ import

    /// Import pictures. Copies them into `images/` when `copy` is set (recommended for sharing).
    /// Returns (new page ids, messages about skipped files).
    pub fn import_images(&mut self, files: &[PathBuf], copy: bool, who: &str) -> (Vec<String>, Vec<String>) {
        let mut added = Vec::new();
        let mut notes = Vec::new();
        for f in files {
            let bytes = match std::fs::read(f) {
                Ok(b) => b,
                Err(e) => {
                    notes.push(format!("{}: {e}", f.display()));
                    continue;
                }
            };
            let sha = sha256_bytes(&bytes);
            if let Some(dup) = self.manifest.pages.iter().find(|p| p.sha256 == sha) {
                notes.push(format!("{} is identical to {} and was skipped", f.display(), dup.image));
                continue;
            }
            let (w, h) = match crate::util::decode_image(&bytes) {
                Ok(img) => (img.width(), img.height()),
                Err(e) => {
                    notes.push(format!("{}: not a readable picture ({e})", f.display()));
                    continue;
                }
            };
            let stem = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let id = format!("{}-{}", util::slug(&stem), &sha[..8]);
            let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "png".into());
            let image = if copy {
                let mut dest = self.root.join("images").join(f.file_name().unwrap());
                if dest.exists() {
                    dest = self.root.join("images").join(format!("{id}.{ext}"));
                }
                if let Err(e) = std::fs::copy(f, &dest) {
                    notes.push(format!("{}: could not copy ({e})", f.display()));
                    continue;
                }
                rel_path(&self.root, &dest)
            } else {
                rel_path(&self.root, f)
            };
            let encoding = if crate::util::is_jxl(&bytes) { "jxl".to_string() } else { match image::guess_format(&bytes) {
                Ok(fmt) => format!("{fmt:?}").to_lowercase(),
                Err(_) => ext.clone(),
            } };
            let entry = PageEntry {
                id: id.clone(),
                image: image.clone(),
                sha256: sha.clone(),
                width: w,
                height: h,
                encoding,
                annotation_file: format!("annotations/{id}.json"),
                source: SourceInfo { original_path: f.to_string_lossy().to_string(), ..Default::default() },
                added: Stamp { by: who.into(), at: now() },
            };
            let page = PageFile::new(&id, ImageRef { path: image, sha256: sha, width: w, height: h });
            self.manifest.pages.push(entry);
            self.pages.push(page);
            self.dirty.insert(id.clone());
            added.push(id);
        }
        if !added.is_empty() {
            self.manifest_dirty = true;
        }
        (added, notes)
    }

    // ------------------------------------------------------------ saving

    pub fn save_manifest(&mut self) -> Result<(), String> {
        let path = self.manifest_path();
        let json = to_json(&self.manifest);
        if let Ok(bytes) = std::fs::read(&path) {
            if !self.manifest_disk_hash.is_empty() && sha256_bytes(&bytes) != self.manifest_disk_hash && bytes != json.as_bytes() {
                // someone else changed the manifest: keep their copy next to ours instead of overwriting
                let _ = std::fs::write(self.root.join(".labeldesk/project.theirs.json"), &bytes);
            }
        }
        atomic_write(&path, json.as_bytes()).map_err(|e| e.to_string())?;
        self.manifest_disk_hash = sha256_bytes(json.as_bytes());
        self.manifest_dirty = false;
        Ok(())
    }

    pub fn save_page(&mut self, id: &str, who: &str) -> Result<SaveOutcome, String> {
        let Some(i) = self.index_of(id) else { return Err(format!("unknown page {id}")) };
        let path = self.page_path(id);
        let mut page = self.pages[i].clone();
        for a in &mut page.annotations {
            a.round();
        }
        let json = to_json(&page);
        if let Ok(bytes) = std::fs::read(&path) {
            let disk = sha256_bytes(&bytes);
            if bytes == json.as_bytes() {
                self.after_save(id, page, disk);
                return Ok(SaveOutcome::Unchanged);
            }
            if self.disk_hash.get(id).is_some_and(|h| *h != disk) {
                let theirs: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                return Ok(SaveOutcome::Conflict(Box::new(parse_page(theirs)?)));
            }
        }
        atomic_write(&path, json.as_bytes()).map_err(|e| e.to_string())?;
        self.append_history(id, &page, who);
        self.after_save(id, page, sha256_bytes(json.as_bytes()));
        Ok(SaveOutcome::Saved)
    }

    fn after_save(&mut self, id: &str, page: PageFile, hash: String) {
        if let Some(i) = self.index_of(id) {
            self.pages[i] = page.clone();
        }
        self.saved.insert(id.into(), page);
        self.disk_hash.insert(id.into(), hash);
        self.dirty.remove(id);
        let _ = std::fs::remove_file(self.recovery_dir().join(format!("{id}.json")));
    }

    /// Save everything that changed. Returns pages that hit a conflict.
    pub fn save_all(&mut self, who: &str) -> Result<Vec<(String, Box<PageFile>)>, String> {
        let mut conflicts = Vec::new();
        for id in self.dirty.clone() {
            if let SaveOutcome::Conflict(theirs) = self.save_page(&id, who)? {
                conflicts.push((id, theirs));
            }
        }
        if self.manifest_dirty {
            self.save_manifest()?;
        }
        Ok(conflicts)
    }

    /// Accept the other version as the new base after a merge so the next save goes through.
    pub fn adopt_disk_version(&mut self, id: &str, theirs: &PageFile) {
        let path = self.page_path(id);
        if let Ok(bytes) = std::fs::read(&path) {
            self.disk_hash.insert(id.into(), sha256_bytes(&bytes));
        }
        self.saved.insert(id.into(), theirs.clone());
        self.dirty.insert(id.into());
    }

    pub fn saved_version(&self, id: &str) -> Option<&PageFile> {
        self.saved.get(id)
    }

    fn append_history(&self, id: &str, page: &PageFile, who: &str) {
        let at = now();
        let mut lines = String::new();
        let old = self.saved.get(id);
        let empty = Vec::new();
        let before = old.map(|p| &p.annotations).unwrap_or(&empty);
        for a in &page.annotations {
            match before.iter().find(|b| b.id == a.id) {
                None => lines.push_str(&history_line(&at, who, &a.id, HistoryAction::Created, vec![], None, Some(a))),
                Some(b) if b != a => {
                    let mut groups: Vec<String> = a.changed_groups(b).into_iter().map(String::from).collect();
                    if b.review != a.review {
                        groups.push("review".into());
                    }
                    if b.provenance != a.provenance {
                        groups.push("provenance".into());
                    }
                    lines.push_str(&history_line(&at, who, &a.id, HistoryAction::Modified, groups, Some(b), Some(a)));
                }
                _ => {}
            }
        }
        for b in before {
            if !page.annotations.iter().any(|a| a.id == b.id) {
                lines.push_str(&history_line(&at, who, &b.id, HistoryAction::Deleted, vec![], Some(b), None));
            }
        }
        if !lines.is_empty() {
            use std::io::Write;
            let p = self.root.join("history").join(format!("{id}.jsonl"));
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = f.write_all(lines.as_bytes());
            }
        }
    }

    pub fn history(&self, page_id: &str) -> Vec<HistoryEntry> {
        let p = self.root.join("history").join(format!("{page_id}.jsonl"));
        std::fs::read_to_string(p).map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
    }

    // ------------------------------------------------------------ recovery

    pub fn write_recovery(&self, id: &str) {
        if let Some(i) = self.index_of(id) {
            let _ = atomic_write(&self.recovery_dir().join(format!("{id}.json")), to_json(&self.pages[i]).as_bytes());
        }
    }

    /// Unsaved work left behind by an earlier session that differs from the saved files.
    pub fn pending_recovery(&self) -> Vec<RecoveryItem> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.recovery_dir()) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Ok((v, _)) = read_json(&p) else { continue };
            let Ok(page) = parse_page(v) else { continue };
            let Some(i) = self.index_of(&page.page_id) else { continue };
            if self.pages[i] == page {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            let when = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| util::format_ts(d.as_secs() as i64))
                .unwrap_or_default();
            out.push(RecoveryItem { page_id: page.page_id.clone(), page, when });
        }
        out.sort_by(|a, b| a.page_id.cmp(&b.page_id));
        out
    }

    pub fn discard_recovery(&self, id: &str) {
        let _ = std::fs::remove_file(self.recovery_dir().join(format!("{id}.json")));
    }

    /// Check that every picture is still there and unchanged.
    pub fn verify_images(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for e in &self.manifest.pages {
            let p = self.image_path(e);
            match std::fs::read(&p) {
                Ok(b) if sha256_bytes(&b) == e.sha256 => {}
                Ok(_) => out.push((e.id.clone(), format!("{} changed since import (checksum differs)", e.image))),
                Err(_) => out.push((e.id.clone(), format!("{} is missing", e.image))),
            }
        }
        out
    }
}

fn history_line(at: &str, by: &str, id: &str, action: HistoryAction, groups: Vec<String>, before: Option<&Annotation>, after: Option<&Annotation>) -> String {
    let e = HistoryEntry {
        at: at.into(),
        by: by.into(),
        annotation_id: id.into(),
        action,
        groups,
        before: before.map(|a| serde_json::to_value(a).unwrap()),
        after: after.map(|a| serde_json::to_value(a).unwrap()),
    };
    let mut s = serde_json::to_string(&e).unwrap();
    s.push('\n');
    s
}

fn has_valid_approval(a: &Annotation) -> bool {
    a.review.approvals.iter().any(|x| x.invalidated.is_none()) || a.review.gold.is_some()
}

/// Three-way merge of one page. `base` is the last version both sides shared (None for an
/// unrelated copy). Our version wins where both changed; their version is kept as a conflict for a
/// person to settle. Reviewed data from either side is never dropped silently.
pub fn merge_page(base: Option<&PageFile>, ours: &PageFile, theirs: &PageFile, source: &str) -> (PageFile, usize) {
    let mut out = ours.clone();
    let mut conflicts = 0;
    let at = now();
    let base_get = |id: &str| base.and_then(|b| b.get(id));
    // annotations present in theirs
    for t in &theirs.annotations {
        let b = base_get(&t.id);
        match out.annotations.iter().position(|a| a.id == t.id) {
            Some(i) => {
                let o = &out.annotations[i];
                if o == t {
                    continue;
                }
                let ours_changed = b.is_none_or(|b| b != o);
                let theirs_changed = b.is_none_or(|b| b != t);
                if !ours_changed && theirs_changed {
                    out.annotations[i] = t.clone();
                } else if ours_changed && theirs_changed {
                    conflicts += 1;
                    out.conflicts.push(Conflict {
                        annotation_id: t.id.clone(),
                        detected: at.clone(),
                        source: source.into(),
                        theirs: Some(t.clone()),
                        note: "Changed differently in both copies; this copy's version is shown.".into(),
                    });
                }
            }
            None => {
                if b.is_none() {
                    out.annotations.push(t.clone()); // added on their side
                } else if b != Some(t) || has_valid_approval(t) {
                    // we deleted it but they changed or approved it: bring it back for a decision
                    out.annotations.push(t.clone());
                    conflicts += 1;
                    out.conflicts.push(Conflict {
                        annotation_id: t.id.clone(),
                        detected: at.clone(),
                        source: source.into(),
                        theirs: None,
                        note: "Deleted here but changed or approved in the other copy; restored.".into(),
                    });
                }
            }
        }
    }
    // annotations only we have
    let their_ids: BTreeSet<&str> = theirs.annotations.iter().map(|a| a.id.as_str()).collect();
    let mut keep = Vec::new();
    for o in out.annotations.drain(..) {
        if their_ids.contains(o.id.as_str()) {
            keep.push(o);
            continue;
        }
        match base_get(&o.id) {
            Some(b) if *b == o && !has_valid_approval(&o) => {} // they deleted it, we did not touch it
            Some(_) => {
                conflicts += 1;
                out.conflicts.push(Conflict {
                    annotation_id: o.id.clone(),
                    detected: at.clone(),
                    source: source.into(),
                    theirs: None,
                    note: "Deleted in the other copy but changed or approved here; kept.".into(),
                });
                keep.push(o);
            }
            None => keep.push(o),
        }
    }
    out.annotations = keep;
    for r in &theirs.inference_runs {
        if !out.inference_runs.iter().any(|x| x.id == r.id) {
            out.inference_runs.push(r.clone());
        }
    }
    if theirs.completeness.inspected && !out.completeness.inspected {
        out.completeness = theirs.completeness.clone();
    }
    if out.review.approved_for_export.is_none() {
        out.review.approved_for_export = theirs.review.approved_for_export.clone();
    }
    for c in &theirs.conflicts {
        if !out.conflicts.contains(c) {
            out.conflicts.push(c.clone());
        }
    }
    (out, conflicts)
}
