//! # `replay` — Git's per-owner reindex-from-source `replay` body (EB-26 / P-246, M3)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.9 (reindex-from-source re-emit),
//! the Git glue doc §4 (the `check_status` projection is rebuilt by asking the Bus to `reindex`
//! `ci.check.updated` for the scope; a derived projection is never restored). **Contract:** index
//! row **2.6** (`events::reindex(scope)` → owner `replay(scope, since)` emits `*.snapshot`;
//! **sub-artifact-granular** — Git per-repo / per-blob / per-PR). **Floor filled:** the Bus's
//! `myelin_events::reindex` named the per-OWNER `replay` bodies as EB-26 (P-246, M3); this is GIT's.
//!
//! ## What this is
//! Git is an OWNING subsystem of reindex-from-source. When a derived store (Search's code index,
//! Refs' edges, the `check_status` projection) is wiped or bootstrapped, the Bus asks Git to
//! `replay(scope, since)` → the `*.snapshot` drafts it re-emits through the SAME outbox→bus→live-
//! consumer path (no backdoor read of the derived index — EI-04 §5.3, steady-state and recovery are
//! one code path). [`GitReindexSource`] is Git's [`myelin_events::ReindexSource`] body: it reads
//! Git's OWN source of truth (its repo/blob/PR rows — modelled here as the in-memory truth the live
//! store will hold) and replays it **sub-artifact-granular**:
//!
//! - **`repo:<id>`** — a whole repo (the `git.repo.snapshot` re-emit);
//! - **`blob:<repo>/<oid>`** — a single indexed blob / code-projection unit (the `git.blob.snapshot`
//!   re-emit — Search's code index re-derives at blob granularity, GIT-P25/P31);
//! - **`pr:<repo>/<num>`** — a single PR (the `git.pr.snapshot` re-emit).
//!
//! The deterministic snapshot `event_id` (from `(aggregate, version)`, `myelin_events::snapshot_event_id`)
//! makes a re-run an idempotent no-op (the outbox `UNIQUE(event_id)` + the consumer dedup ledger both
//! absorb a duplicate) — so cold == live (BUS-D5), and a Git push's `git.ref.updated` (the live event)
//! and its `git.repo.snapshot` (the cold re-emit) land the same projection bytes.
//!
//! ## An erased aggregate is SKIPPED (X-7)
//! A tombstoned repo/PR/blob is NOT re-snapshotted — the erasure stays erased across a reindex (the
//! `*.erased` tombstone is the live truth; replay never resurrects a shredded aggregate). The
//! in-memory truth here models that by simply not holding an erased aggregate.

use std::collections::BTreeMap;

use myelin_events::{
    ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope, Visibility,
    AggregateKey,
};

use crate::events;

/// The sub-artifact kind a Git reindex scope selects (contract 2.6 — sub-artifact-granular). Parsed
/// from the opaque `scope.selector` (`repo:<id>`, `blob:<repo>/<oid>`, `pr:<repo>/<num>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitReplayKind {
    /// A whole repo — re-emits `git.repo.snapshot` for the repo aggregate.
    Repo,
    /// A single indexed blob / code-projection unit — re-emits `git.blob.snapshot`.
    Blob,
    /// A single PR — re-emits `git.pr.snapshot`.
    Pr,
}

impl GitReplayKind {
    /// The `*.snapshot` event type token this kind re-emits (the NAMED git token, never a literal).
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                GitReplayKind::Repo => events::GIT_REPO_SNAPSHOT,
                GitReplayKind::Blob => events::GIT_BLOB_SNAPSHOT,
                GitReplayKind::Pr => events::GIT_PR_SNAPSHOT,
            }
            .to_string(),
        )
    }

    /// Parse the leading kind token off a `scope.selector` (`repo:…`, `blob:…`, `pr:…`).
    fn from_selector(selector: &str) -> Option<GitReplayKind> {
        match selector.split(':').next() {
            Some("repo") => Some(GitReplayKind::Repo),
            Some("blob") => Some(GitReplayKind::Blob),
            Some("pr") => Some(GitReplayKind::Pr),
            _ => None,
        }
    }
}

/// One aggregate in Git's source of truth: the `(version, payload)` the live event of this aggregate
/// carries (references-not-payloads — ids/refs, never blob bytes). A snapshot re-emits exactly this,
/// so the cold rebuild is byte-identical to the live projection.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GitTruthRow {
    /// The kind (repo/blob/pr) — selects the `*.snapshot` type.
    kind: GitReplayKind,
    /// The aggregate version (half the deterministic snapshot id).
    version: u64,
    /// The live payload (refs/ids — references-not-payloads).
    payload: serde_json::Value,
    /// The live subject ref the snapshot carries.
    subject: ArtifactRef,
}

/// **Git's [`ReindexSource`] body (EB-26 / P-246, M3 — the named floor filled).** Holds Git's OWN
/// source of truth (its repo/blob/PR rows) and replays a sub-artifact-granular scope → the
/// `*.snapshot` drafts. A real wiring reads Git's OLTP rows / the content store; this reads its
/// in-memory truth — the SAME shape (the live store swaps in behind this same `replay` signature).
#[derive(Debug, Default)]
pub struct GitReindexSource {
    /// `aggregate-key → GitTruthRow`. A `BTreeMap` so the replay order is deterministic (ascending
    /// aggregate) — a rebuild is byte-reproducible.
    truth: BTreeMap<String, GitTruthRow>,
}

impl GitReindexSource {
    /// A fresh, empty source.
    pub fn new() -> GitReindexSource {
        GitReindexSource::default()
    }

    /// Record/update Git's truth for an aggregate (the live write a push/PR-update would make). The
    /// `payload` carries refs/ids (references-not-payloads); a `version` field is stamped in so the
    /// derived store reads it for LWW.
    pub fn upsert(
        &mut self,
        kind: GitReplayKind,
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
            GitTruthRow {
                kind,
                version,
                payload,
                subject: ArtifactRef(subject.to_string()),
            },
        );
    }

    /// Mark an aggregate erased (a tombstone) — it is REMOVED from the truth, so a subsequent replay
    /// SKIPS it (the erasure stays erased across a reindex, X-7). Returns `true` if it was present.
    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }
}

/// **Does aggregate `agg` match a specific (non-`all`) selector `target`?** A match is EITHER an EXACT
/// aggregate id (`repo:myelin://acme/git/repo/core`) OR a SEGMENT-ANCHORED trailing suffix
/// (`repo:core` → `…/repo/core`; the suffix must begin at a `/` or `#` boundary, so a short selector
/// `core` matches `…/core` but NEVER `mycore`). The boundary anchor is the correctness guard: an
/// unanchored `ends_with` would over-match (`core` → `mycore`) and silently widen a sub-artifact-
/// granular reindex into a sibling's snapshot. The exact-match arm is NOT redundant: it matches a
/// whole-id selector that is its own first segment (no leading boundary char).
fn matches_aggregate(agg: &str, target: &str) -> bool {
    if agg == target {
        return true;
    }
    // A segment-anchored suffix: `agg` ends with `target` AND the char just before the suffix is a
    // segment boundary (`/` or `#`) — so the selector names a whole trailing segment, not a substring.
    agg.strip_suffix(target)
        .and_then(|head| head.chars().next_back())
        .is_some_and(|boundary| boundary == '/' || boundary == '#')
}

impl ReindexSource for GitReindexSource {
    fn owner_token(&self) -> &str {
        "git"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        // The selector names the sub-artifact kind + (optionally) a specific aggregate. A
        // kind-only selector (`repo:all`) replays every aggregate of that kind; a specific selector
        // (`repo:core`) replays just that aggregate. An unparseable selector replays nothing (the
        // Bus's `reindex` LOUD-errors a no-source-for-owner separately; an empty git scope is a
        // no-op replay, not an error — the scope simply matched nothing in git's truth).
        let kind = match GitReplayKind::from_selector(&scope.selector) {
            Some(k) => k,
            None => return Vec::new(),
        };
        // The part after `kind:` — `all` (every aggregate of this kind) or a specific aggregate id.
        let target = scope.selector.split_once(':').map(|(_, rest)| rest).unwrap_or("");
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
                // A git snapshot is controller-or-processor depending on the artifact; repo content
                // is tenant-content (processor posture, §4.3) — references-not-payloads, no inline PII
                // in the snapshot payload (the body PII lives behind a per-subject DEK, never in a
                // ref-only snapshot).
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        reindex, snapshot_event_id, DerivedStore, EmitContextBase, OutboxStore, Region, TenantId,
        Timestamp, Actor,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn source() -> GitReindexSource {
        let mut s = GitReindexSource::new();
        s.upsert(
            GitReplayKind::Repo,
            "myelin://acme/git/repo/core",
            3,
            "myelin://acme/git/repo/core",
            serde_json::json!({ "default_branch": "main", "visibility": "internal" }),
        );
        s.upsert(
            GitReplayKind::Repo,
            "myelin://acme/git/repo/docs",
            1,
            "myelin://acme/git/repo/docs",
            serde_json::json!({ "default_branch": "main", "visibility": "private" }),
        );
        s.upsert(
            GitReplayKind::Blob,
            "myelin://acme/git/repo/core#blob-abc",
            2,
            "myelin://acme/git/repo/core#blob-abc",
            serde_json::json!({ "path": "src/main.rs", "language": "rust" }),
        );
        s
    }

    /// Git's `replay` re-emits `git.repo.snapshot` for the repo scope — sub-artifact-granular (the
    /// blob row is NOT in a repo replay).
    #[test]
    fn replay_repo_scope_emits_git_repo_snapshots() {
        let s = source();
        let scope = SnapshotScope::new("git", "repo:all");
        let drafts = s.replay(&scope, None);
        assert_eq!(drafts.len(), 2, "two repos, not the blob");
        for d in &drafts {
            assert_eq!(d.type_.0, "git.repo.snapshot");
        }
    }

    /// A specific sub-artifact selector replays just that aggregate (blob granularity).
    #[test]
    fn replay_specific_blob_scope_is_sub_artifact_granular() {
        let s = source();
        let scope = SnapshotScope::new("git", "blob:myelin://acme/git/repo/core#blob-abc");
        let drafts = s.replay(&scope, None);
        assert_eq!(drafts.len(), 1, "exactly the one blob");
        assert_eq!(drafts[0].type_.0, "git.blob.snapshot");
        assert_eq!(drafts[0].version, 2);
    }

    /// An ERASED aggregate is SKIPPED by replay (the erasure stays erased across a reindex, X-7).
    #[test]
    fn replay_skips_an_erased_aggregate() {
        let mut s = source();
        assert!(s.erase("myelin://acme/git/repo/docs"));
        let drafts = s.replay(&SnapshotScope::new("git", "repo:all"), None);
        assert_eq!(drafts.len(), 1, "the erased repo is not re-snapshotted");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    /// **The `pr:` selector arm parses (kills the `delete Some("pr")` arm mutant).** A `pr:all` scope
    /// resolves the PR kind — without the arm `from_selector` returns `None` and the replay is empty.
    #[test]
    fn replay_pr_selector_arm_resolves() {
        let mut s = source();
        s.upsert(
            GitReplayKind::Pr,
            "myelin://acme/git/repo/core#pr-1",
            7,
            "myelin://acme/git/repo/core#pr-1",
            serde_json::json!({ "title": "ref" }),
        );
        let drafts = s.replay(&SnapshotScope::new("git", "pr:all"), None);
        assert_eq!(drafts.len(), 1, "the pr arm resolved (else the selector is unparseable → empty)");
        assert_eq!(drafts[0].type_.0, "git.pr.snapshot");
    }

    /// **A specific aggregate id matches EXACTLY ONE, not `all` (kills `==`→`!=` and `||`→`&&` on the
    /// target filter).** A `repo:<full-id>` selector replays only that repo — the other repo is NOT
    /// re-emitted. If `==` flipped to `!=` the wrong repo(s) would match; if `||` flipped to `&&` the
    /// `target == "all"` term would force-empty a specific selector.
    #[test]
    fn replay_specific_aggregate_matches_exactly_one_not_all() {
        let s = source();
        let scope = SnapshotScope::new("git", "repo:myelin://acme/git/repo/core");
        let drafts = s.replay(&scope, None);
        assert_eq!(drafts.len(), 1, "exactly the named repo (not all repos, not none)");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    /// **The `ends_with` suffix match resolves a short selector (kills the `ends_with` term drop).** A
    /// selector that is a trailing suffix of the aggregate (`core` of `…/repo/core`) matches it. Drop
    /// the `ends_with` term and a suffix selector matches nothing.
    #[test]
    fn replay_suffix_selector_matches_via_ends_with() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("git", "repo:core"), None);
        assert_eq!(drafts.len(), 1, "the `core` suffix selector matched the full aggregate");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    /// **A suffix selector is SEGMENT-ANCHORED — `core` matches `…/repo/core` but NOT a substring
    /// `mycore` (kills the boundary-anchor mutants + the over-match hazard).** The anchor is the
    /// correctness guard: an unanchored suffix would widen a per-repo reindex into a sibling's.
    #[test]
    fn replay_suffix_selector_is_segment_anchored_not_substring() {
        let mut s = source();
        // a sibling repo whose id ENDS WITH the substring `core` but is NOT segment `core`.
        s.upsert(
            GitReplayKind::Repo,
            "myelin://acme/git/repo/mycore",
            1,
            "myelin://acme/git/repo/mycore",
            serde_json::json!({ "default_branch": "main" }),
        );
        let drafts = s.replay(&SnapshotScope::new("git", "repo:core"), None);
        assert_eq!(drafts.len(), 1, "`core` matches ONLY the segment `…/repo/core`, never `…/mycore`");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    /// `matches_aggregate` directly: exact, segment-anchored suffix, and the rejected substring.
    #[test]
    fn matches_aggregate_exact_anchored_and_substring_reject() {
        assert!(matches_aggregate("myelin://acme/git/repo/core", "myelin://acme/git/repo/core"), "exact");
        assert!(matches_aggregate("myelin://acme/git/repo/core", "core"), "segment-anchored `/core`");
        assert!(matches_aggregate("myelin://acme/git/repo/core#blob-1", "blob-1"), "anchored at `#`");
        assert!(!matches_aggregate("myelin://acme/git/repo/mycore", "core"), "substring is NOT a match");
        assert!(!matches_aggregate("myelin://acme/git/repo/core", "other"), "a non-suffix is no match");
    }

    /// **The `since` cursor replays STRICTLY ABOVE the cursor (kills `>`→`==`/`<`/`>=`).** With
    /// `since = Some(2)` only the version-3 repo replays; the version-1 repo is below the cursor and is
    /// skipped, and the version-2 cursor value itself is NOT re-emitted (`>` is strict, the incremental
    /// backfill resume invariant — re-emitting the cursor row would double-apply it).
    #[test]
    fn replay_since_cursor_is_strictly_above() {
        let s = source(); // repos: core@3, docs@1 ; blob@2
        // since = 2 over the repo scope: only core@3 (>2) replays; docs@1 (<2) and any @2 are skipped.
        let drafts = s.replay(&SnapshotScope::new("git", "repo:all"), Some(2));
        assert_eq!(drafts.len(), 1, "only the version-3 repo replays past since=2");
        assert_eq!(drafts[0].version, 3);
        // since exactly AT the highest version (3) → nothing replays (`>` is strict, not `>=`).
        assert!(
            s.replay(&SnapshotScope::new("git", "repo:all"), Some(3)).is_empty(),
            "since == the high-water version re-emits nothing (the cursor row is not re-applied)"
        );
        // since = 0 → every repo replays (the full-rebuild floor).
        assert_eq!(
            s.replay(&SnapshotScope::new("git", "repo:all"), Some(0)).len(),
            2,
            "since=0 replays every repo (full rebuild)"
        );
    }

    /// **cold == live + idempotent re-run (BUS-D5 for the git owner).** Build a LIVE projection from
    /// git's events; wipe + rebuild from the reindex snapshot replay through the outbox; assert
    /// byte-identical. Then re-run the reindex — 0 new snapshots (the deterministic id no-ops it).
    #[test]
    fn git_replay_rebuilds_byte_identically_and_is_idempotent() {
        let s = source();
        let scope = SnapshotScope::new("git", "repo:all");

        // LIVE projection — ingest the drafts as the live events would have been.
        let mut live = DerivedStore::new();
        for draft in s.replay(&scope, None) {
            live.ingest(&snapshot_envelope(&draft));
        }

        // COLD projection — wiped, rebuilt from the reindex snapshot replay through the outbox.
        let mut cold = DerivedStore::new();
        let sources: &[&dyn ReindexSource] = &[&s];
        let mut outbox = OutboxStore::new();
        let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        assert_eq!(r1.snapshots_emitted, 2);
        for draft in s.replay(&scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row present");
            cold.ingest(&row.envelope);
        }
        assert_eq!(cold.parity_bytes(), live.parity_bytes(), "cold == live (byte-identical)");

        // Re-run — idempotent (0 new; the deterministic ids are already present).
        let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 new (idempotent)");
        assert_eq!(r2.snapshots_skipped_duplicate, 2);
    }

    /// Build the `*.snapshot` envelope a relay would deliver for a draft (the consumer's input).
    fn snapshot_envelope(draft: &SnapshotDraft) -> myelin_events::EventEnvelope {
        use myelin_events::CorrelationId;
        myelin_events::EventEnvelope {
            event_id: draft.event_id(),
            type_: draft.type_.clone(),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: draft.subject.clone(),
            aggregate: draft.aggregate.clone(),
            causation_id: None,
            correlation_id: CorrelationId(draft.event_id().0),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: draft.data_role,
            visibility: draft.visibility,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            payload: draft.payload.clone(),
        }
    }

    /// The deterministic snapshot id is stable for a git aggregate@version (the re-run idempotency).
    #[test]
    fn git_snapshot_id_is_deterministic() {
        let a = AggregateKey("myelin://acme/git/repo/core".into());
        assert_eq!(snapshot_event_id(&a, 3), snapshot_event_id(&a, 3));
        assert_ne!(snapshot_event_id(&a, 3), snapshot_event_id(&a, 4));
    }
}
