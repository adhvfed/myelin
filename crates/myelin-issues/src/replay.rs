//! # `replay` — Issues' per-owner reindex-from-source `replay` body (EB-27 / P-327, M4)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.9 (reindex-from-source re-emit).
//! **Contract:** index row **2.6** (`events::reindex(scope)` → owner `replay(scope, since)` emits
//! `*.snapshot`; **sub-artifact-granular**). **Floor filled:** the Bus's `myelin_events::reindex`
//! named the per-OWNER `replay` bodies as a floor; EB-26 (P-246, M3) filled Git/KN; this is Issues'
//! M4 body (EB-27 / P-327).
//!
//! Issues is an OWNING subsystem of reindex-from-source. [`IssueReindexSource`] reads Issues' OWN
//! source of truth (its issue / relation / comment / rollup rows — modelled here as the in-memory
//! truth the live store holds) and replays a sub-artifact-granular scope → the `*.snapshot` drafts it
//! re-emits through the SAME outbox→bus→live-consumer path (no backdoor read of a derived index):
//!
//! - **`issue:<key>`** — a single issue (`issue.issue.snapshot`);
//! - **`relation:<id>`** — a single relation edge (`issue.relation.snapshot`);
//! - **`comment:<id>`** — a single comment (`issue.comment.snapshot`);
//! - **`rollup:<id>`** — a single rollup/initiative aggregate (`issue.rollup.snapshot`).
//!
//! The deterministic snapshot `event_id` from `(aggregate, version)` makes a re-run idempotent
//! (cold == live, BUS-D5). An erased aggregate is SKIPPED (X-7) — a tombstoned issue is not
//! re-snapshotted (the erasure stays erased across a reindex).

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

/// The sub-artifact kind an Issues reindex scope selects (contract 2.6 — sub-artifact-granular).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueReplayKind {
    /// A single issue — re-emits `issue.issue.snapshot`.
    Issue,
    /// A single relation edge — re-emits `issue.relation.snapshot`.
    Relation,
    /// A single comment — re-emits `issue.comment.snapshot`.
    Comment,
    /// A single rollup/initiative aggregate — re-emits `issue.rollup.snapshot`.
    Rollup,
}

impl IssueReplayKind {
    /// The `*.snapshot` event type token this kind re-emits (the NAMED issue token, never a literal).
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                IssueReplayKind::Issue => events::ISSUE_SNAPSHOT,
                IssueReplayKind::Relation => events::RELATION_SNAPSHOT,
                IssueReplayKind::Comment => events::COMMENT_SNAPSHOT,
                IssueReplayKind::Rollup => events::ROLLUP_SNAPSHOT,
            }
            .to_string(),
        )
    }

    /// Parse the leading kind token off a `scope.selector`.
    fn from_selector(selector: &str) -> Option<IssueReplayKind> {
        match selector.split(':').next() {
            Some("issue") => Some(IssueReplayKind::Issue),
            Some("relation") => Some(IssueReplayKind::Relation),
            Some("comment") => Some(IssueReplayKind::Comment),
            Some("rollup") => Some(IssueReplayKind::Rollup),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IssueTruthRow {
    kind: IssueReplayKind,
    version: u64,
    payload: serde_json::Value,
    subject: ArtifactRef,
}

/// **Issues' [`ReindexSource`] body (EB-27 / P-327, M4 — the named floor filled).** Holds Issues'
/// OWN source of truth and replays a sub-artifact-granular scope → the `*.snapshot` drafts. A real
/// wiring reads Issues' OLTP rows; this reads its in-memory truth (the SAME `replay` signature).
#[derive(Debug, Default)]
pub struct IssueReindexSource {
    truth: BTreeMap<String, IssueTruthRow>,
}

impl IssueReindexSource {
    /// A fresh, empty source.
    pub fn new() -> IssueReindexSource {
        IssueReindexSource::default()
    }

    /// Record/update Issues' truth for an aggregate (the live write a create/transition made).
    pub fn upsert(
        &mut self,
        kind: IssueReplayKind,
        aggregate: &str,
        version: u64,
        subject: &str,
        mut payload: serde_json::Value,
    ) {
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert("version".into(), serde_json::json!(version));
        }
        self.truth.insert(
            aggregate.to_string(),
            IssueTruthRow {
                kind,
                version,
                payload,
                subject: ArtifactRef(subject.to_string()),
            },
        );
    }

    /// Mark an aggregate erased (X-7) — REMOVED from the truth so a subsequent replay SKIPS it.
    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }
}

/// Segment-anchored aggregate match (the over-match guard — a short selector matches a whole
/// trailing segment, never a substring).
fn matches_aggregate(agg: &str, target: &str) -> bool {
    if agg == target {
        return true;
    }
    agg.strip_suffix(target)
        .and_then(|head| head.chars().next_back())
        .is_some_and(|boundary| boundary == '/' || boundary == '#')
}

impl ReindexSource for IssueReindexSource {
    fn owner_token(&self) -> &str {
        "issue"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let kind = match IssueReplayKind::from_selector(&scope.selector) {
            Some(k) => k,
            None => return Vec::new(),
        };
        let target = scope
            .selector
            .split_once(':')
            .map(|(_, rest)| rest)
            .unwrap_or("");
        self.truth
            .iter()
            .filter(|(_, row)| row.kind == kind)
            .filter(|(agg, _)| target == "all" || matches_aggregate(agg, target))
            .filter(|(_, row)| since.is_none_or(|s| row.version > s))
            .map(|(agg, row)| SnapshotDraft {
                aggregate: AggregateKey(agg.clone()),
                version: row.version,
                type_: kind.snapshot_type(),
                subject: row.subject.clone(),
                payload: row.payload.clone(),
                // An issue snapshot carries the issue's controller metadata + refs (PII body lives
                // behind a per-subject DEK, never in a ref-only snapshot).
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::snapshot_event_id;

    fn source() -> IssueReindexSource {
        let mut s = IssueReindexSource::new();
        s.upsert(
            IssueReplayKind::Issue,
            "myelin://acme/issue/CORE-1",
            2,
            "myelin://acme/issue/CORE-1",
            serde_json::json!({ "state": "open", "title_ref": "ref" }),
        );
        s.upsert(
            IssueReplayKind::Issue,
            "myelin://acme/issue/CORE-2",
            5,
            "myelin://acme/issue/CORE-2",
            serde_json::json!({ "state": "closed" }),
        );
        s
    }

    /// **Sub-artifact-granular replay (contract 2.6).** An `issue:CORE-1` scope replays exactly that
    /// issue's snapshot — not a sibling's (segment-anchored match).
    #[test]
    fn issue_granular_replay() {
        let drafts = source().replay(&SnapshotScope::new("issue", "issue:CORE-1"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/issue/CORE-1");
        assert_eq!(drafts[0].type_.0, "issue.issue.snapshot");
    }

    /// **cold == live + idempotent re-run (BUS-D5).** The replay is deterministic; the snapshot
    /// `event_id` is stable from `(aggregate, version)`.
    #[test]
    fn cold_equals_live_idempotent() {
        let src = source();
        let scope = SnapshotScope::new("issue", "issue:all");
        let a = src.replay(&scope, None);
        let b = src.replay(&scope, None);
        assert_eq!(a, b);
        assert_eq!(
            snapshot_event_id(&a[0].aggregate, a[0].version),
            snapshot_event_id(&b[0].aggregate, b[0].version)
        );
    }

    /// **An erased issue is SKIPPED (X-7).**
    #[test]
    fn erased_issue_is_skipped() {
        let mut src = source();
        assert!(src.erase("myelin://acme/issue/CORE-1"));
        let drafts = src.replay(&SnapshotScope::new("issue", "issue:all"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/issue/CORE-2");
    }

    /// **The owner_token is the canonical `issue` subsystem token** (the Bus reindex dispatch key).
    #[test]
    fn owner_token_is_issue() {
        assert_eq!(source().owner_token(), "issue");
    }
}
