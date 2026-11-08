//! Review workflow: edit tracking, approvals, re-review after edits, gold approval.

use std::collections::BTreeSet;

use crate::schema::*;

fn group_dimension(g: &str) -> Option<Dimension> {
    match g {
        "geometry" => Some(Dimension::Geometry),
        "transcription" => Some(Dimension::Transcription),
        "reading_order" => Some(Dimension::ReadingOrder),
        _ => None,
    }
}

pub fn valid_approvals(a: &Annotation) -> impl Iterator<Item = &Approval> {
    a.review.approvals.iter().filter(|x| x.invalidated.is_none())
}

/// People who approved every required aspect (valid approvals only).
pub fn full_reviewers(a: &Annotation, policy: &ReviewPolicy) -> BTreeSet<String> {
    let mut by: BTreeSet<String> = valid_approvals(a).map(|x| x.by.clone()).collect();
    by.retain(|u| policy.required_dimensions.iter().all(|d| valid_approvals(a).any(|x| &x.by == u && x.dimension == *d)));
    by
}

/// A person changed annotation content: bump revision, record who, update field provenance and
/// handle approvals of the changed aspects according to the project policy.
pub fn on_edit(old: &Annotation, new: &mut Annotation, policy: &ReviewPolicy, user: &str, now: &str) {
    let groups = new.changed_groups(old);
    if groups.is_empty() {
        return;
    }
    new.revision = old.revision + 1;
    new.modified = Stamp { by: user.into(), at: now.into() };
    for g in &groups {
        let next = match new.provenance.fields.get(*g) {
            Some(FieldState::Proposed | FieldState::Accepted | FieldState::Corrected | FieldState::HeuristicProposal) => FieldState::Corrected,
            Some(FieldState::Verified | FieldState::ChangedAfterVerification) => FieldState::ChangedAfterVerification,
            _ if matches!(new.provenance.origin, Origin::Ai | Origin::Heuristic) => FieldState::Corrected,
            _ => FieldState::Manual,
        };
        new.provenance.fields.insert(g.to_string(), next);
    }
    // a person's own reading order always wins over later suggestions
    if new.reading_order != old.reading_order && new.reading_order.is_some() {
        new.reading.get_or_insert_with(Default::default).source = Some(OrderSource::Manual);
    }
    // values a person set by hand count as confirmed quick-review answers
    let (ot, nt) = (old.transcription.clone().unwrap_or_default(), new.transcription.clone().unwrap_or_default());
    let mut confirmed = Vec::new();
    if ot.language != nt.language || ot.script != nt.script {
        confirmed.push("language");
    }
    if ot.writing_mode != nt.writing_mode || ot.direction != nt.direction {
        confirmed.push("direction");
    }
    if old.recognizability != new.recognizability {
        confirmed.push("recognizability");
    }
    if old.classes != new.classes {
        confirmed.push("classes");
    }
    if old.orientation.mirrored != new.orientation.mirrored {
        confirmed.push("mirrored");
    }
    if old.occlusion != new.occlusion {
        confirmed.push("occlusion");
    }
    if old.quality != new.quality {
        confirmed.push("quality");
    }
    for f in confirmed {
        if old.checks.get(f) == new.checks.get(f) {
            new.checks.insert(f.into(), FieldCheck { state: CheckState::Confirmed, by: user.into(), at: now.into(), source: "set by hand".into(), batch: false });
        }
    }
    let dims: Vec<Dimension> = groups.iter().filter_map(|g| group_dimension(g)).collect();
    let was_reviewed = old.review.state >= ReviewState::HumanReviewed || old.review.gold.is_some();
    let mut touched_approval = false;
    for ap in new.review.approvals.iter_mut().filter(|x| x.invalidated.is_none()) {
        if dims.contains(&ap.dimension) {
            touched_approval = true;
            if policy.on_edit_after_approval == EditPolicy::Invalidate {
                ap.invalidated = Some(Invalidation {
                    at: now.into(),
                    by: user.into(),
                    reason: format!("{} changed in revision {}", ap.dimension.key(), new.revision),
                });
            }
        }
    }
    if touched_approval || (was_reviewed && groups.contains(&"metadata")) {
        new.review.needs_rereview = true;
    }
    if let Some(g) = new.review.gold.take() {
        new.review.notes.push(ReviewNote {
            kind: NoteKind::Comment,
            dimension: None,
            by: user.into(),
            at: now.into(),
            text: format!("Gold approval by {} (revision {}) withdrawn: edited after approval.", g.by, g.revision),
            resolved: None,
            resolution: String::new(),
        });
    }
    // resizing a proposed character box adjusts the proposal; it is accepted separately
    let adjusts_proposal = new.provenance.origin == Origin::Heuristic && groups.iter().all(|g| *g == "geometry");
    if new.review.state == ReviewState::AiProposed && !adjusts_proposal {
        new.review.state = ReviewState::Draft;
    }
    recompute_state(new, policy);
}

/// Derive the state from valid approvals. Never moves an annotation backwards past `submitted`
/// on its own, and never upgrades it to gold.
pub fn recompute_state(a: &mut Annotation, policy: &ReviewPolicy) {
    let n = full_reviewers(a, policy).len() as u32;
    let en = |s: ReviewState| policy.enabled_states.contains(&s);
    let reviewed = if a.review.needs_rereview {
        None
    } else if a.review.gold.is_some() {
        Some(ReviewState::GoldApproved)
    } else if n >= 2 && en(ReviewState::PeerVerified) {
        Some(ReviewState::PeerVerified)
    } else if n >= 1 && en(ReviewState::HumanReviewed) {
        Some(ReviewState::HumanReviewed)
    } else {
        None
    };
    a.review.state = match reviewed {
        Some(s) => s,
        None if a.review.state >= ReviewState::HumanReviewed => {
            if en(ReviewState::Submitted) { ReviewState::Submitted } else { ReviewState::Draft }
        }
        None => a.review.state,
    };
}

pub fn can_review(a: &Annotation, policy: &ReviewPolicy, user: &str) -> Result<(), String> {
    let own = a.created.by == user || a.modified.by == user;
    if own && (!policy.allow_self_approval || policy.independent_reviewers) {
        return Err(if policy.independent_reviewers {
            "This project needs an independent reviewer: you created or last changed this annotation.".into()
        } else {
            "This project does not allow approving your own work.".into()
        });
    }
    Ok(())
}

/// Approve some aspects. Field provenance for those aspects becomes `verified`.
pub fn approve(a: &mut Annotation, dims: &[Dimension], policy: &ReviewPolicy, user: &str, now: &str) -> Result<(), String> {
    can_review(a, policy, user)?;
    for d in dims {
        let already = valid_approvals(a).any(|x| x.by == user && x.dimension == *d && x.revision == a.revision);
        if !already {
            a.review.approvals.push(Approval {
                dimension: *d,
                by: user.into(),
                at: now.into(),
                revision: a.revision,
                policy_version: policy.version,
                invalidated: None,
            });
        }
        a.provenance.fields.insert(d.key().into(), FieldState::Verified);
    }
    // once every required aspect has a current approval the re-review flag is cleared
    if policy.required_dimensions.iter().all(|d| valid_approvals(a).any(|x| x.dimension == *d && x.revision == a.revision)) {
        a.review.needs_rereview = false;
    }
    if a.review.state.needs_work() && policy.enabled_states.contains(&ReviewState::Submitted) {
        a.review.state = ReviewState::Submitted;
    }
    recompute_state(a, policy);
    Ok(())
}

/// Why an annotation does not (yet) qualify for gold under the current policy. Empty = qualifies.
pub fn gold_blockers(a: &Annotation, page: &PageFile, policy: &ReviewPolicy) -> Vec<String> {
    let mut v = Vec::new();
    let n = full_reviewers(a, policy).len() as u32;
    if n < policy.reviewers_required {
        v.push(format!("needs {} reviewer(s) approving {}, has {n}", policy.reviewers_required, dims_text(&policy.required_dimensions)));
    }
    if a.review.needs_rereview {
        v.push("changed after approval: needs re-review".into());
    }
    for note in &a.review.notes {
        if note.kind == NoteKind::Disagreement && note.resolved.is_none() && note.dimension.is_none_or(|d| policy.adjudicate_on.contains(&d)) {
            v.push(format!("unresolved disagreement: {}", note.text));
        }
        if note.kind == NoteKind::ChangeRequest && note.resolved.is_none() {
            v.push(format!("open change request: {}", note.text));
        }
    }
    if a.review.state == ReviewState::AiProposed {
        v.push("still an unreviewed AI proposal".into());
    }
    if policy.gold_requires_page_complete && !(page.completeness.inspected && page.completeness.complete) {
        v.push("the page is not marked inspected and complete".into());
    }
    v
}

pub fn dims_text(d: &[Dimension]) -> String {
    d.iter().map(|d| d.name().to_lowercase()).collect::<Vec<_>>().join(", ")
}

pub fn approve_gold(a: &mut Annotation, page: &PageFile, policy: &ReviewPolicy, user: &str, now: &str) -> Result<(), String> {
    can_review(a, policy, user)?;
    let b = gold_blockers(a, page, policy);
    if !b.is_empty() {
        return Err(b.join("; "));
    }
    a.review.gold = Some(GoldRecord { by: user.into(), at: now.into(), revision: a.revision, policy_version: policy.version });
    if policy.enabled_states.contains(&ReviewState::GoldApproved) {
        a.review.state = ReviewState::GoldApproved;
    }
    Ok(())
}

pub fn submit(a: &mut Annotation, policy: &ReviewPolicy) {
    if a.review.state.needs_work() {
        a.review.state = if policy.enabled_states.contains(&ReviewState::Submitted) { ReviewState::Submitted } else { ReviewState::Draft };
        if a.review.state == ReviewState::Submitted {
            for n in a.review.notes.iter_mut().filter(|n| n.kind == NoteKind::ChangeRequest && n.resolved.is_none()) {
                n.resolution = "resubmitted".into();
            }
        }
    }
}

pub fn add_note(a: &mut Annotation, kind: NoteKind, dim: Option<Dimension>, text: &str, user: &str, now: &str) {
    a.review.notes.push(ReviewNote { kind, dimension: dim, by: user.into(), at: now.into(), text: text.into(), resolved: None, resolution: String::new() });
    if kind == NoteKind::ChangeRequest && a.review.state != ReviewState::GoldApproved {
        a.review.state = ReviewState::ChangesRequested;
    }
}

/// Accept an AI proposal as it is: it becomes a draft owned by people; nothing is verified yet.
pub fn accept_ai(a: &mut Annotation, user: &str, now: &str) {
    if a.review.state == ReviewState::AiProposed {
        a.review.state = ReviewState::Draft;
        for v in a.provenance.fields.values_mut() {
            if matches!(*v, FieldState::Proposed | FieldState::HeuristicProposal) {
                *v = FieldState::Accepted;
            }
        }
        a.modified = Stamp { by: user.into(), at: now.into() };
    }
}

/// Reject an AI proposal: the annotation goes away, the raw prediction stays in its run as rejected.
pub fn reject_ai(page: &mut PageFile, id: &str, reason: &str, user: &str, now: &str) -> bool {
    let Some(a) = page.get(id).cloned() else { return false };
    if let Some(ai) = &a.provenance.ai {
        if let Some(run) = page.inference_runs.iter_mut().find(|r| r.id == ai.run_id) {
            if let Some(p) = run.predictions.get_mut(ai.prediction) {
                p.status = PredictionStatus::Rejected;
                p.decided = Some(Stamp { by: user.into(), at: now.into() });
                p.reason = reason.into();
            }
        }
    }
    page.annotations.retain(|x| x.id != id);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(user: &str) -> Annotation {
        let mut a = Annotation::new("a-1".into(), Level::Line, "p", Geometry::Box { x: 0.0, y: 0.0, w: 10.0, h: 10.0, rotation_deg: 0.0 }, user, "t0");
        a.transcription = Some(Transcription { text: "テスト".into(), ..Default::default() });
        a
    }

    #[test]
    fn approval_policy_and_rereview() {
        let policy = ReviewPolicy::default();
        let mut a = ann("alice");
        assert!(approve(&mut a, &Dimension::ALL, &policy, "alice", "t1").is_err(), "self approval must be refused");
        approve(&mut a, &Dimension::ALL, &policy, "bob", "t1").unwrap();
        assert_eq!(a.review.state, ReviewState::HumanReviewed);
        assert_eq!(a.provenance.fields["transcription"], FieldState::Verified);

        // editing the transcription invalidates only that approval and asks for re-review
        let old = a.clone();
        a.transcription.as_mut().unwrap().text = "テスト！".into();
        on_edit(&old, &mut a, &policy, "alice", "t2");
        assert_eq!(a.revision, 2);
        assert!(a.review.needs_rereview);
        assert_eq!(a.review.state, ReviewState::Submitted);
        assert_eq!(a.provenance.fields["transcription"], FieldState::ChangedAfterVerification);
        let tr: Vec<_> = a.review.approvals.iter().filter(|x| x.dimension == Dimension::Transcription).collect();
        assert!(tr[0].invalidated.is_some());
        assert!(a.review.approvals.iter().any(|x| x.dimension == Dimension::Geometry && x.invalidated.is_none()));

        approve(&mut a, &[Dimension::Transcription], &policy, "bob", "t3").unwrap();
        assert!(a.review.needs_rereview, "geometry approval is from an older revision");
        approve(&mut a, &[Dimension::Geometry, Dimension::ReadingOrder], &policy, "bob", "t3").unwrap();
        assert!(!a.review.needs_rereview);
        assert_eq!(a.review.state, ReviewState::HumanReviewed);
    }

    #[test]
    fn gold_needs_policy_and_complete_page() {
        let mut policy = ReviewPolicy::default();
        policy.reviewers_required = 2;
        let mut page = PageFile::new("p", ImageRef { path: "x.png".into(), sha256: "0".repeat(64), width: 100, height: 100 });
        let mut a = ann("alice");
        approve(&mut a, &Dimension::ALL, &policy, "bob", "t").unwrap();
        assert!(approve_gold(&mut a, &page, &policy, "carol", "t").is_err());
        approve(&mut a, &Dimension::ALL, &policy, "carol", "t").unwrap();
        assert_eq!(a.review.state, ReviewState::PeerVerified);
        assert!(approve_gold(&mut a, &page, &policy, "carol", "t").is_err(), "page not complete");
        page.completeness.inspected = true;
        page.completeness.complete = true;
        approve_gold(&mut a, &page, &policy, "carol", "t").unwrap();
        assert_eq!(a.review.state, ReviewState::GoldApproved);
        // a later policy change does not touch the record
        policy.version = 2;
        recompute_state(&mut a, &policy);
        assert_eq!(a.review.state, ReviewState::GoldApproved);
        assert_eq!(a.review.gold.as_ref().unwrap().policy_version, 1);
    }
}
