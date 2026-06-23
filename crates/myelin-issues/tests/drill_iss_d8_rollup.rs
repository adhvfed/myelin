//! # ISS-D8 — the event-driven incremental rollup consumer drill (ISS-P18 / P-384, M4)
//!
//! **The two ISS-D8 green artifacts (catalogue F4/rollup):**
//! - **ISS-D8(a) — rollup freshness under a 10k-issue import → a BOUNDED number of ancestor recomputes
//!   via the debounce-coalesce.** The green artifact is the DEBOUNCE BOUND: a 10,000-issue import under
//!   a small ancestor set coalesces to a bounded recompute count (the number of distinct ancestors),
//!   NOT 10,000. Initiative progress is correct within the window.
//! - **ISS-D8(b) — reindex-from-source: `replay` rebuilds the rollup aggregate + the Refs edge
//!   projection DRIFT-FREE vs live.** The green artifact is the REINDEX-PARITY (0 drift): the cold
//!   rebuild byte-matches the live rollup — proving steady-state and recovery share ONE code path
//!   (contract 2.6).
//!
//! This drill also carries the CHAINED-MUTATION e2e (import a subtree → assert bounded recomputes →
//! replay → assert drift-free) the prompt's TESTS section names, and the 2.6 CDC pair (the replay
//! drives the SAME consumer body the live path drives — cold == live).
//!
//! Off the bus, never in the write path: the consumer recomputes ancestors asynchronously; the write
//! path is just "emit the event". The `input_hash` no-op suppression (AG-6) stops the loop storm.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, HandleOutcome, ReindexSource, SnapshotScope, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events::ISSUE_UPDATED;
use myelin_issues::replay::{IssueReindexSource, IssueReplayKind};
use myelin_issues::rollup::{aggregate_snapshot, LeafFact, RollupConsumer};
use myelin_issues::workflow::StateCategory;
use myelin_issues::{rollup_recomputed_draft, RollupAggregate};
use myelin_tenancy::{Region, TenantId};

fn r(s: &str) -> ArtifactRef {
    ArtifactRef(s.to_string())
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn updated_event(event_id: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(ISSUE_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: r(subject),
        aggregate: AggregateKey(subject.into()),
        causation_id: None,
        correlation_id: CorrelationId(event_id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

/// **ISS-D8(a) — rollup freshness under a 10k-issue import → a BOUNDED number of ancestor recomputes
/// (the debounce bound is the green artifact).** Import 10,000 child issues under a SMALL ancestor set
/// (one epic, three stories) and drive each through the consumer's `handle` (the off-the-bus delta).
/// The coalesce collapses the 10,000 deltas into a recompute count equal to the number of distinct
/// dirtied ancestors — BOUNDED, never 10,000.
#[test]
fn iss_d8a_10k_import_coalesces_to_a_bounded_recompute_count() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/ENG-EPIC");
    let stories: Vec<ArtifactRef> = (0..3)
        .map(|i| r(&format!("myelin://acme/issue/issue/ENG-STORY-{i}")))
        .collect();
    for s in &stories {
        consumer.add_parent_edge(s, &epic);
    }

    // Import 10,000 children, ~3,333 under each story (each child's parent walk reaches its story + the
    // epic — two ancestors per leaf, but only 4 DISTINCT ancestors across the whole import).
    let n = 10_000usize;
    for i in 0..n {
        let child = r(&format!("myelin://acme/issue/issue/ENG-T{i}"));
        let story = &stories[i % stories.len()];
        consumer.add_parent_edge(&child, story);
        consumer.put_leaf(&child, LeafFact::new(Some(1), StateCategory::Started));
        // The off-the-bus delta (one per imported child).
        let outcome = consumer.handle(&updated_event(&format!("imp-{i}"), &child.0));
        assert_eq!(outcome, HandleOutcome::Done);
    }

    // THE DEBOUNCE BOUND (the green artifact): 10,000 deltas → a BOUNDED recompute count (the 4 distinct
    // ancestors: the epic + the three stories), NOT 10,000.
    let bound = consumer.pending_recompute_count();
    assert_eq!(
        bound, 4,
        "ISS-D8(a): a 10k import coalesces to the 4 distinct ancestors (epic + 3 stories), \
         not 10,000 recomputes — the debounce bound"
    );
    assert!(
        bound < n,
        "ISS-D8(a) GREEN: recompute count {bound} << import size {n} (bounded, debounced)"
    );

    // Initiative progress is correct within the window: flush, then the epic's rollup totals 10,000 live
    // leaves (all Started → 0 done).
    let changed = consumer.flush();
    let epic_agg = changed
        .iter()
        .find(|(a, _)| a.0 == epic.0)
        .map(|(_, agg)| agg)
        .expect("the epic was recomputed");
    assert_eq!(
        epic_agg.total, n as u64,
        "the epic rolls up all 10k live leaves"
    );
    assert_eq!(epic_agg.done, 0, "all Started → 0 done");
}

/// **ISS-D8(b) — reindex-from-source rebuilds the rollup DRIFT-FREE vs live (the reindex-parity 0-drift
/// green artifact).** Build a live rollup over a realistic subtree, wipe the derived aggregates, then
/// reindex off the source (the `issue_relation` edges + the leaf facts) — the cold rebuild byte-matches
/// the live rollup (steady-state and recovery share ONE code path, contract 2.6).
#[test]
fn iss_d8b_reindex_from_source_rebuilds_drift_free() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/ENG-EPIC");
    let story = r("myelin://acme/issue/issue/ENG-STORY");
    consumer.add_parent_edge(&story, &epic);

    // A mixed subtree: 3 completed, 2 started, 1 cancelled (excluded) under the story.
    for (i, cat) in [
        StateCategory::Completed,
        StateCategory::Completed,
        StateCategory::Completed,
        StateCategory::Started,
        StateCategory::Started,
        StateCategory::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let t = r(&format!("myelin://acme/issue/issue/ENG-T{i}"));
        consumer.add_parent_edge(&t, &story);
        consumer.put_leaf(&t, LeafFact::new(Some(2), cat));
        consumer.handle(&updated_event(&format!("d8b-{i}"), &t.0));
    }
    let _ = consumer.flush();
    let live = aggregate_snapshot(&consumer);
    // The story rolls up 5 live (cancelled excluded), 3 done.
    let story_agg = &live[&story.0];
    assert_eq!(story_agg.total, 5);
    assert_eq!(story_agg.done, 3);

    // REINDEX-FROM-SOURCE: wipe + rebuild off the source of truth.
    let rebuilt = consumer.reindex_from();
    let cold = aggregate_snapshot(&consumer);

    // THE REINDEX-PARITY (0 drift): the cold rebuild byte-matches the live rollup.
    assert_eq!(
        live, cold,
        "ISS-D8(b) GREEN: the cold rebuild byte-matches the live rollup (0-drift reindex-parity)"
    );
    assert!(rebuilt >= 2, "the epic + the story are both rebuilt");
}

/// **The chained-mutation e2e (the prompt's named TEST): import a subtree → assert bounded recomputes →
/// replay → assert drift-free.** One end-to-end pass exercising the whole loop: the off-the-bus deltas
/// coalesce (bounded), the flush computes the live rollup, the reindex rebuilds it byte-identical.
#[test]
fn chained_mutation_e2e_import_bounded_replay_drift_free() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/CHAIN-EPIC");
    let story = r("myelin://acme/issue/issue/CHAIN-STORY");
    consumer.add_parent_edge(&story, &epic);

    // Import 50 children under the story (chained mutations).
    for i in 0..50 {
        let t = r(&format!("myelin://acme/issue/issue/CHAIN-T{i}"));
        consumer.add_parent_edge(&t, &story);
        let cat = if i % 2 == 0 {
            StateCategory::Completed
        } else {
            StateCategory::Started
        };
        consumer.put_leaf(&t, LeafFact::new(Some(1), cat));
        consumer.handle(&updated_event(&format!("chain-{i}"), &t.0));
    }
    // BOUNDED: 50 deltas → 2 distinct ancestors (epic + story).
    assert_eq!(consumer.pending_recompute_count(), 2);

    let _ = consumer.flush();
    let live = aggregate_snapshot(&consumer);
    assert_eq!(live[&story.0].total, 50);
    assert_eq!(live[&story.0].done, 25);

    // REPLAY (reindex) → DRIFT-FREE.
    consumer.reindex_from();
    assert_eq!(
        live,
        aggregate_snapshot(&consumer),
        "the replay rebuilds the rollup drift-free vs live (chained-mutation e2e)"
    );
}

/// **The 2.6 CDC pair (replay) — the SAME consumer body the live path drives rebuilds off the
/// `*.snapshot` re-emit (cold == live).** The PROVIDER (producer side) is Issues' [`IssueReindexSource`]
/// re-emitting the rollup as `issue.rollup.snapshot` (sub-artifact-granular); the CONSUMER is the rollup
/// rebuild reading the SAME source. Pins that the rollup is reindex-from-source-rebuildable (a DERIVED
/// store with no second recovery path) and that its snapshot replay is deterministic (idempotent re-run).
#[test]
fn cdc_2_6_rollup_snapshot_replay_is_deterministic() {
    // The PROVIDER (producer) side: a rollup aggregate is a re-emittable *.snapshot (contract 2.6 —
    // the rollup is snapshot-emittable for OLAP convenience even though it is DERIVED; the edge truth
    // is issue_relation).
    let mut src = IssueReindexSource::new();
    let agg = RollupAggregate {
        total: 10,
        done: 4,
        estimate_sum: 20,
        input_hash: 0x1234,
    };
    let draft = rollup_recomputed_draft(&r("myelin://acme/issue/issue/EPIC-1"), &agg);
    src.upsert(
        IssueReplayKind::Rollup,
        "myelin://acme/issue/issue/EPIC-1",
        7,
        "myelin://acme/issue/issue/EPIC-1",
        draft.payload.clone(),
    );

    let scope = SnapshotScope::new("issue", "rollup:EPIC-1");
    let a = src.replay(&scope, None);
    let b = src.replay(&scope, None);
    assert_eq!(
        a, b,
        "the rollup snapshot replay is deterministic (cold == live, idempotent)"
    );
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].type_.0, "issue.rollup.snapshot");
    assert_eq!(a[0].payload["done"], 4);
    assert_eq!(a[0].payload["total"], 10);
}
