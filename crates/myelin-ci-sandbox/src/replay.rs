//! # `replay` — CI's per-owner reindex-from-source `replay` body (EB-27 / P-327, M4)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.9 (reindex-from-source re-emit),
//! with the CI glue doc 03 §7.3 (CI implements `replay(scope, since)` emitting `ci.run.snapshot` /
//! `ci.deployment.snapshot` / `ci.pipeline.snapshot` through the outbox → the live consumer path —
//! the only recovery path for a derived store). **Contract:** index row **2.6**
//! (`events::reindex(scope)` → owner `replay(scope, since)` emits `*.snapshot`; **sub-artifact-
//! granular** — CI **one-run**). **Floor filled:** the Bus's `myelin_events::reindex` named the
//! per-OWNER `replay` bodies as a floor; EB-26 (P-246, M3) filled Git/KN; this is CI's M4 body
//! (EB-27 / P-327), the counterpart that completes the per-owner replay for the M4 owners.
//!
//! ## What this is
//! CI is an OWNING subsystem of reindex-from-source. When a derived store (Search's run index, the
//! OLAP usage rollups, Refs' run edges) is wiped or bootstrapped, the Bus asks CI to
//! `replay(scope, since)` → the `*.snapshot` drafts it re-emits through the SAME outbox→bus→live-
//! consumer path (no backdoor read of the derived index — EI-04 §5.3, steady-state and recovery are
//! one code path). [`CiReindexSource`] is CI's [`myelin_events::ReindexSource`] body: it reads CI's
//! OWN source of truth (its run/deployment/pipeline rows — modelled here as the in-memory truth the
//! live store will hold) and replays it **sub-artifact-granular**:
//!
//! - **`run:<id>`** — a single CI run (the `ci.run.snapshot` re-emit; CI **one-run** granularity);
//! - **`deployment:<id>`** — a single deployment (the `ci.deployment.snapshot` re-emit);
//! - **`pipeline:<id>`** — a single pipeline config (the `ci.pipeline.snapshot` re-emit).
//!
//! The deterministic snapshot `event_id` (from `(aggregate, version)`,
//! `myelin_events::snapshot_event_id`) makes a re-run an idempotent no-op (the outbox
//! `UNIQUE(event_id)` + the consumer dedup ledger both absorb a duplicate) — so cold == live
//! (BUS-D5), and a live `ci.run.succeeded` and its cold `ci.run.snapshot` land the same projection.
//!
//! ## An erased aggregate is SKIPPED (X-7)
//! A tombstoned run/deployment/runner is NOT re-snapshotted — the erasure stays erased across a
//! reindex (the `*.erased` tombstone is the live truth; replay never resurrects a shredded
//! aggregate). The in-memory truth here models that by simply not holding an erased aggregate.

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

/// The sub-artifact kind a CI reindex scope selects (contract 2.6 — sub-artifact-granular, CI
/// one-run). Parsed from the opaque `scope.selector` (`run:<id>`, `deployment:<id>`,
/// `pipeline:<id>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiReplayKind {
    /// A single CI run — re-emits `ci.run.snapshot` (one-run granularity).
    Run,
    /// A single deployment — re-emits `ci.deployment.snapshot`.
    Deployment,
    /// A single pipeline config — re-emits `ci.pipeline.snapshot`.
    Pipeline,
}

impl CiReplayKind {
    /// The `*.snapshot` event type token this kind re-emits (the NAMED ci token, never a literal).
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                CiReplayKind::Run => events::CI_RUN_SNAPSHOT,
                CiReplayKind::Deployment => events::CI_DEPLOYMENT_SNAPSHOT,
                CiReplayKind::Pipeline => events::CI_PIPELINE_SNAPSHOT,
            }
            .to_string(),
        )
    }

    /// Parse the leading kind token off a `scope.selector` (`run:…`, `deployment:…`, `pipeline:…`).
    fn from_selector(selector: &str) -> Option<CiReplayKind> {
        match selector.split(':').next() {
            Some("run") => Some(CiReplayKind::Run),
            Some("deployment") => Some(CiReplayKind::Deployment),
            Some("pipeline") => Some(CiReplayKind::Pipeline),
            _ => None,
        }
    }
}

/// One aggregate in CI's source of truth: the `(version, payload)` the live event of this aggregate
/// carries (references-not-payloads — ids/refs, never log bytes). A snapshot re-emits exactly this,
/// so the cold rebuild is byte-identical to the live projection.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CiTruthRow {
    kind: CiReplayKind,
    version: u64,
    payload: serde_json::Value,
    subject: ArtifactRef,
}

/// **CI's [`ReindexSource`] body (EB-27 / P-327, M4 — the named floor filled).** Holds CI's OWN
/// source of truth (its run/deployment/pipeline rows) and replays a sub-artifact-granular scope → the
/// `*.snapshot` drafts. A real wiring reads CI's OLTP rows; this reads its in-memory truth — the SAME
/// shape (the live store swaps in behind this same `replay` signature).
#[derive(Debug, Default)]
pub struct CiReindexSource {
    /// `aggregate-key → CiTruthRow`. A `BTreeMap` so the replay order is deterministic (ascending
    /// aggregate) — a rebuild is byte-reproducible.
    truth: BTreeMap<String, CiTruthRow>,
}

impl CiReindexSource {
    /// A fresh, empty source.
    pub fn new() -> CiReindexSource {
        CiReindexSource::default()
    }

    /// Record/update CI's truth for an aggregate (the live write a run/deploy/pipeline event made).
    /// The `payload` carries refs/ids (references-not-payloads); a `version` field is stamped in so
    /// the derived store reads it for LWW.
    pub fn upsert(
        &mut self,
        kind: CiReplayKind,
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
            CiTruthRow {
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

/// **Does aggregate `agg` match a specific (non-`all`) selector `target`?** A match is EITHER an
/// EXACT aggregate id OR a SEGMENT-ANCHORED trailing suffix (the boundary char before the suffix is
/// `/` or `#`), so a short selector matches a whole trailing segment but NEVER a substring (the
/// over-match guard that keeps a one-run reindex from widening into a sibling's snapshot).
fn matches_aggregate(agg: &str, target: &str) -> bool {
    if agg == target {
        return true;
    }
    agg.strip_suffix(target)
        .and_then(|head| head.chars().next_back())
        .is_some_and(|boundary| boundary == '/' || boundary == '#')
}

impl ReindexSource for CiReindexSource {
    fn owner_token(&self) -> &str {
        "ci"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let kind = match CiReplayKind::from_selector(&scope.selector) {
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
                // A CI run snapshot is controller metadata (the fact a run happened + its refs),
                // references-not-payloads (no inline PII; log bytes never ride a snapshot).
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        reindex, snapshot_event_id, Actor, EmitContextBase, OutboxStore, Region, TenantId,
        Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("ci".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn source_with_two_runs() -> CiReindexSource {
        let mut src = CiReindexSource::new();
        src.upsert(
            CiReplayKind::Run,
            "myelin://acme/ci/run/r1",
            1,
            "myelin://acme/ci/run/r1",
            serde_json::json!({ "overall": "success", "commit": "abc" }),
        );
        src.upsert(
            CiReplayKind::Run,
            "myelin://acme/ci/run/r2",
            3,
            "myelin://acme/ci/run/r2",
            serde_json::json!({ "overall": "failure", "commit": "def" }),
        );
        src
    }

    /// **One-run granular replay (contract 2.6).** A `run:r1` scope replays exactly that run's
    /// snapshot — not a sibling's (the segment-anchored match).
    #[test]
    fn one_run_granular_replay() {
        let src = source_with_two_runs();
        let scope = SnapshotScope::new("ci", "run:r1");
        let drafts = src.replay(&scope, None);
        assert_eq!(drafts.len(), 1, "exactly the one run");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r1");
        assert_eq!(drafts[0].type_.0, "ci.run.snapshot");
    }

    /// **cold == live + idempotent re-run (BUS-D5).** A full `run:all` replay emits both runs with a
    /// deterministic `event_id` from `(aggregate, version)`; replaying again yields byte-identical
    /// drafts (the outbox `UNIQUE(event_id)` absorbs the re-run → idempotent).
    #[test]
    fn cold_equals_live_idempotent_rerun() {
        let src = source_with_two_runs();
        let scope = SnapshotScope::new("ci", "run:all");

        let first = src.replay(&scope, None);
        let second = src.replay(&scope, None);
        assert_eq!(first, second, "the replay is deterministic (cold == live)");

        // The deterministic event_id makes a re-run idempotent.
        let id_a = snapshot_event_id(&first[0].aggregate, first[0].version);
        let id_b = snapshot_event_id(&second[0].aggregate, second[0].version);
        assert_eq!(id_a, id_b, "the snapshot event_id is deterministic");
    }

    /// **An erased run is SKIPPED (X-7).** A tombstoned run is not re-snapshotted — the erasure stays
    /// erased across a reindex.
    #[test]
    fn erased_run_is_skipped() {
        let mut src = source_with_two_runs();
        assert!(src.erase("myelin://acme/ci/run/r1"));
        let drafts = src.replay(&SnapshotScope::new("ci", "run:all"), None);
        assert_eq!(drafts.len(), 1, "only the non-erased run replays");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r2");
    }

    /// **The `since` cursor replays only newer aggregates (incremental reindex).**
    #[test]
    fn since_cursor_replays_only_newer() {
        let src = source_with_two_runs();
        // r1 is version 1, r2 is version 3; since=1 yields only r2.
        let drafts = src.replay(&SnapshotScope::new("ci", "run:all"), Some(1));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r2");
    }

    /// **The Bus's `reindex` seam dispatches to CI's owner body** (the owner_token = "ci" wiring).
    #[test]
    fn bus_reindex_dispatches_to_ci_owner() {
        let src = source_with_two_runs();
        let sources: Vec<&dyn ReindexSource> = vec![&src];
        let mut outbox = OutboxStore::new();
        let receipt = reindex(
            &SnapshotScope::new("ci", "run:all"),
            None,
            &sources,
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex dispatches to the ci owner");
        assert_eq!(receipt.owners_replayed, vec!["ci".to_string()]);
        assert_eq!(
            receipt.snapshots_emitted, 2,
            "both runs re-emitted through the outbox"
        );
    }
}
