//! Version control of a project folder with Git.
//!
//! A page is committed when its review is complete; pages still being worked on stay as
//! uncommitted changes. Each commit holds only the files of that page (annotation file, change
//! log, masks) plus the manifest, so other unfinished work is never swept into it.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root);
    c
}

fn run(mut c: Command) -> Result<String, String> {
    let out = c.output().map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let msg = if err.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { err };
        Err(msg)
    }
}

/// Is Git installed and is `root` inside a repository?
pub fn is_repo(root: &Path) -> bool {
    let mut c = git(root);
    c.args(["rev-parse", "--is-inside-work-tree"]);
    run(c).is_ok_and(|s| s == "true")
}

pub fn available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Author flags when the repository has no identity configured yet.
fn identity(root: &Path, user: &str) -> Vec<String> {
    let mut c = git(root);
    c.args(["config", "user.name"]);
    if run(c).is_ok_and(|s| !s.is_empty()) {
        return Vec::new();
    }
    vec!["-c".into(), format!("user.name={user}"), "-c".into(), format!("user.email={user}@labeldesk.invalid")]
}

/// Paths (relative to the root) that changed and are not committed yet.
pub fn pending(root: &Path) -> Vec<String> {
    let mut c = git(root);
    c.args(["status", "--porcelain", "--untracked-files=all"]);
    run(c).map(|s| s.lines().filter_map(|l| l.get(3..)).map(|p| p.trim_matches('"').to_string()).collect()).unwrap_or_default()
}

/// Commit exactly these files (relative to the root). Returns the short commit id, or `None`
/// when nothing in them changed.
pub fn commit_files(root: &Path, files: &[PathBuf], message: &str, user: &str) -> Result<Option<String>, String> {
    let rel: Vec<String> = files.iter().filter(|f| root.join(f).exists()).map(|f| f.to_string_lossy().replace('\\', "/")).collect();
    if rel.is_empty() {
        return Ok(None);
    }
    let mut add = git(root);
    add.arg("add").arg("--").args(&rel);
    run(add)?;
    let mut diff = git(root);
    diff.args(["diff", "--cached", "--quiet", "--"]).args(&rel);
    if diff.status().is_ok_and(|s| s.success()) {
        return Ok(None); // nothing staged for these files
    }
    let mut c = git(root);
    c.args(identity(root, user)).args(["commit", "-q", "-m", message, "--"]).args(&rel);
    run(c)?;
    let mut h = git(root);
    h.args(["rev-parse", "--short", "HEAD"]);
    run(h).map(Some)
}

/// Start tracking a project: `git init`, then one commit with the manifest, documentation,
/// schemas and pictures. Annotation files are committed page by page once reviewed.
pub fn init(root: &Path, user: &str) -> Result<Option<String>, String> {
    if !is_repo(root) {
        let mut c = git(root);
        c.args(["init", "-q"]);
        run(c)?;
    }
    let base: Vec<PathBuf> = [".gitignore", ".gitattributes", "project.json", "FORMAT.md", "schema", "images"].iter().map(PathBuf::from).collect();
    commit_files(root, &base, "Start tracking the annotation project", user)
}

/// Files that belong to one page of the project.
pub fn page_files(store: &crate::store::Store, page_id: &str) -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("project.json")];
    if let Ok(p) = store.page_path(page_id).strip_prefix(&store.root) {
        v.push(p.to_path_buf());
    }
    v.push(PathBuf::from("history").join(format!("{page_id}.jsonl")));
    if let Some(e) = store.entry(page_id) {
        let img = PathBuf::from(&e.image);
        if img.is_relative() {
            v.push(img);
        }
    }
    let masks = store.root.join("masks");
    if let Ok(rd) = std::fs::read_dir(&masks) {
        for f in rd.flatten() {
            if f.file_name().to_string_lossy().starts_with(page_id) {
                v.push(PathBuf::from("masks").join(f.file_name()));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_only_the_reviewed_page() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("labeldesk-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = crate::sample::create_project(&dir, "tester").unwrap();
        assert!(init(&dir, "tester").unwrap().is_some(), "initial commit");
        assert!(is_repo(&dir));
        let (a, b) = (s.pages[0].page_id.clone(), s.pages[1].page_id.clone());
        for id in [&a, &b] {
            let i = s.index_of(id).unwrap();
            s.pages[i].completeness.inspected = true;
            s.dirty.insert(id.clone());
        }
        s.save_all("tester").unwrap();
        let id = commit_files(&dir, &page_files(&s, &a), "Review page", "tester").unwrap();
        assert!(id.is_some());
        let left = pending(&dir);
        assert!(left.iter().any(|p| p.contains(&b)), "the other page stays uncommitted: {left:?}");
        assert!(!left.iter().any(|p| p.contains(&format!("{a}.json"))), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
