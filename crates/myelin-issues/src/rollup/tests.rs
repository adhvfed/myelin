//! Unit tests for the rollup consumer (ISS-P18 / P-384): the depth-16 cycle-safe ancestor walk, the
//! `input_hash` no-op suppression (loop-storm — AG-6), the incremental re-sum, the debounce-coalesce,
//! and the reindex-from-source 0-drift parity. These are the mandatory-core mutation-bearing tests.

use super::*;
use myelin_events::{
    Actor, ArtifactRef, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
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

/// Build an `issue.issue.updated` envelope whose subject is the changed leaf.
fn updated_event(event_id: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(events::ISSUE_UPDATED.into()),
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

// ── the incremental re-sum (§6.1) ────────────────────────────────────────────────────────────────

/// **The incremental re-sum counts done/total + sums estimates; cancelled is EXCLUDED (§6.1).** Two
/// completed + one started + one cancelled → total 3 (cancelled excluded), done 2, estimate sum over
/// the non-cancelled.
#[test]
fn re_sum_counts_done_total_and_excludes_cancelled() {
    let leaves = vec![
        LeafFact::new(Some(3), StateCategory::Completed),
        LeafFact::new(Some(5), StateCategory::Completed),
        LeafFact::new(Some(2), StateCategory::Started),
        LeafFact::new(Some(8), StateCategory::Cancelled), // EXCLUDED from total + sum
    ];
    let agg = recompute_incremental(&leaves);
    assert_eq!(agg.total, 3, "cancelled is excluded from the live total");
    assert_eq!(agg.done, 2, "two completed leaves");
    assert_eq!(
        agg.estimate_sum, 10,
        "3 + 5 + 2; the cancelled 8 is excluded"
    );
    assert!((agg.progress() - 2.0 / 3.0).abs() < 1e-9);
}

/// An empty / all-cancelled subtree has 0 total and 0.0 progress (never a divide-by-zero).
#[test]
fn empty_subtree_is_zero_progress() {
    assert_eq!(recompute_incremental(&[]).progress(), 0.0);
    let all_cancelled = vec![LeafFact::new(Some(1), StateCategory::Cancelled)];
    let agg = recompute_incremental(&all_cancelled);
    assert_eq!(agg.total, 0);
    assert_eq!(agg.progress(), 0.0);
}

/// **The `input_hash` is ORDER-INDEPENDENT (the 0-drift property).** The SAME multiset of leaves in a
/// DIFFERENT order yields the SAME `input_hash` — so the cold rebuild's hash byte-matches the live
/// one regardless of visit order (ISS-D8b reindex-parity).
#[test]
fn input_hash_is_order_independent() {
    let a = vec![
        LeafFact::new(Some(3), StateCategory::Completed),
        LeafFact::new(Some(5), StateCategory::Started),
    ];
    let b = vec![
        LeafFact::new(Some(5), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Completed),
    ];
    assert_eq!(
        recompute_incremental(&a).input_hash,
        recompute_incremental(&b).input_hash,
        "the input_hash must be independent of leaf visit order (0-drift)"
    );
}

/// **A changed input CHANGES the `input_hash` (the suppression fires iff NOTHING changed).** Flipping
/// one leaf's category changes the hash — so the no-op suppression does NOT swallow a real change.
#[test]
fn changed_input_changes_the_hash() {
    let before = vec![LeafFact::new(Some(3), StateCategory::Started)];
    let after = vec![LeafFact::new(Some(3), StateCategory::Completed)];
    assert_ne!(
        recompute_incremental(&before).input_hash,
        recompute_incremental(&after).input_hash,
        "a category flip must change the input_hash"
    );
}

// ── the rollup store accessors (the read-time-rollup floor) ──────────────────────────────────────

/// **The store accessors round-trip + `clear_aggregates` wipes ONLY the derived aggregates.** `put_leaf`
/// /`leaf` round-trip the re-sum input; `aggregate` reads the derived value; `clear_aggregates` (the
/// reindex wipe) removes the derived aggregate but LEAVES the leaf facts (they are the source of truth).
#[test]
fn store_accessors_round_trip_and_clear_wipes_only_aggregates() {
    let mut store = RollupStore::new();
    let issue = r("myelin://acme/issue/issue/C0");
    let anc = r("myelin://acme/issue/issue/EPIC-1");
    let fact = LeafFact::new(Some(7), StateCategory::Completed);
    store.put_leaf(&issue, fact.clone());
    assert_eq!(store.leaf(&issue), Some(&fact), "put_leaf/leaf round-trip");
    assert_eq!(
        store.leaf(&anc),
        None,
        "an unrecorded issue has no leaf fact"
    );

    // Seed a derived aggregate via the public re-sum (the store holds it).
    let agg = recompute_incremental(std::slice::from_ref(&fact));
    store.aggregates.insert(anc.0.clone(), agg.clone());
    assert_eq!(
        store.aggregate(&anc),
        Some(&agg),
        "aggregate reads the derived value"
    );

    // clear_aggregates wipes the DERIVED aggregate but leaves the leaf facts (the source).
    store.clear_aggregates();
    assert_eq!(
        store.aggregate(&anc),
        None,
        "clear_aggregates wipes the derived aggregate"
    );
    assert_eq!(
        store.leaf(&issue),
        Some(&fact),
        "the leaf fact (the source) survives the wipe"
    );
}

/// **`RollupConsumer::aggregate` returns the computed aggregate (not None).** A recompute writes the
/// aggregate; the accessor reads it back (the pinned accessor — a mutant returning `None` is caught).
#[test]
fn consumer_aggregate_accessor_returns_the_computed_value() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);
    consumer.put_leaf(&child, LeafFact::new(Some(4), StateCategory::Completed));
    assert_eq!(
        consumer.aggregate(&parent),
        None,
        "no aggregate before the recompute"
    );
    let _ = consumer.recompute(&parent);
    let agg = consumer
        .aggregate(&parent)
        .expect("the recompute wrote the aggregate");
    assert_eq!(agg.done, 1);
    assert_eq!(agg.total, 1);
    assert_eq!(agg.estimate_sum, 4);
}

// ── the depth-16 cycle-safe ancestor walk (contract 5.3, §6.1) ────────────────────────────────────

/// **The ancestor walk is CYCLE-SAFE (contract 5.3 / §6.1).** A `parent` cycle (A parent B parent A)
/// TERMINATES with a finite ancestor set — never a hang. The visited-set prunes the revisit.
#[test]
fn ancestor_walk_is_cycle_safe() {
    let mut graph = IssueRelationGraph::new();
    let a = r("myelin://acme/issue/issue/A");
    let b = r("myelin://acme/issue/issue/B");
    // A's parent is B; B's parent is A — a cycle (a roadmap diagnostic, never a hang).
    graph.add_edge(&a, &b, IssueLifecycleRel::Parent);
    graph.add_edge(&b, &a, IssueLifecycleRel::Parent);
    let ancestors = walk_parent_edges(&graph, &a);
    // Finite — B is reached, then A is pruned (already the seed). No hang.
    assert!(ancestors.iter().any(|n| n.0 == b.0));
    assert!(
        ancestors.len() <= 2,
        "a cycle yields a finite ancestor set, not an infinite walk"
    );
}

/// **The ancestor walk is DEPTH-BOUNDED at 16 (contract 5.3).** A `parent` chain longer than 16 stops
/// at the ceiling — no ancestor past depth 16 is reported (a malformed deep chain never blows the
/// rollup fan-out).
#[test]
fn ancestor_walk_is_depth_bounded() {
    let mut graph = IssueRelationGraph::new();
    // A 30-deep parent chain: I0 parent I1 parent ... parent I29.
    let nodes: Vec<ArtifactRef> = (0..30)
        .map(|i| r(&format!("myelin://acme/issue/issue/I{i}")))
        .collect();
    for i in 0..29 {
        graph.add_edge(&nodes[i], &nodes[i + 1], IssueLifecycleRel::Parent);
    }
    let ancestors = walk_parent_edges(&graph, &nodes[0]);
    // The depth ceiling is TRAVERSE_MAX_DEPTH (16): at most 16 ancestors are reported.
    assert!(
        ancestors.len() <= TRAVERSE_MAX_DEPTH,
        "no ancestor past the depth-16 ceiling: got {}",
        ancestors.len()
    );
    assert_eq!(TRAVERSE_MAX_DEPTH, 16);
}

// ── the debounce-coalesce (§6.1 — a burst → ONE recompute) ────────────────────────────────────────

/// **The debounce coalesces a burst into ONE recompute per ancestor (§6.1 — the ISS-D8a bound).** Many
/// child changes under ONE parent collapse to a SINGLE pending recompute (not N) — a 10k-import
/// triggers a BOUNDED number of recomputes.
#[test]
fn debounce_coalesces_a_burst_into_one_recompute() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    // 100 children, all under the SAME parent.
    let children: Vec<ArtifactRef> = (0..100)
        .map(|i| r(&format!("myelin://acme/issue/issue/C{i}")))
        .collect();
    for c in &children {
        consumer.add_parent_edge(c, &parent);
        consumer.mark_changed(c);
    }
    // 100 child changes → ONE pending recompute (the coalesce).
    assert_eq!(
        consumer.pending_recompute_count(),
        1,
        "a burst of 100 child changes under one parent coalesces to ONE recompute"
    );
}

// ── the input_hash no-op suppression (§6.1 / AG-6 — the loop-storm guard) ─────────────────────────

/// **The `input_hash` no-op suppression STOPS the loop storm (§6.1 / AG-6).** Re-running the SAME
/// recompute (no input change) returns `Suppressed` — NO `issue.rollup.recomputed` event is emitted, so
/// a rollup-event storm cannot amplify.
#[test]
fn unchanged_recompute_is_suppressed_no_event() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);
    consumer.put_leaf(&child, LeafFact::new(Some(3), StateCategory::Completed));

    // First recompute changes the aggregate (an event is owed).
    let first = consumer.recompute(&parent);
    assert!(
        !first.is_suppressed(),
        "the first recompute writes the aggregate"
    );
    assert_eq!(first.aggregate().unwrap().done, 1);

    // Second recompute with NO input change → SUPPRESSED (no event; the loop storm stops, AG-6).
    let second = consumer.recompute(&parent);
    assert!(
        second.is_suppressed(),
        "an unchanged recompute is suppressed — NO event (AG-6 loop-storm guard)"
    );
}

/// **The `input_hash` fold is XOR, not OR (the duplicate-leaf distinguisher).** Two IDENTICAL leaves
/// XOR to a DIFFERENT hash than one leaf (XOR of a value with itself is 0, distinct from the value);
/// an OR fold would collapse them to the same hash and miss the multiplicity change. A subtree that
/// goes from one leaf to two identical leaves MUST change the hash (a real count change), so the no-op
/// suppression does not swallow it.
#[test]
fn input_hash_fold_is_xor_distinguishing_multiplicity() {
    let one = vec![LeafFact::new(Some(3), StateCategory::Started)];
    let two_identical = vec![
        LeafFact::new(Some(3), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Started),
    ];
    // One leaf vs two identical leaves: the totals differ (1 vs 2) AND the input_hash MUST differ
    // (an OR fold would collapse `h` and `h|h == h`, missing the change; XOR gives `h` vs `h^h == 0`).
    assert_ne!(
        recompute_incremental(&one).total,
        recompute_incremental(&two_identical).total
    );
    assert_ne!(
        recompute_incremental(&one).input_hash,
        recompute_incremental(&two_identical).input_hash,
        "the XOR fold distinguishes a multiplicity change (an OR fold would not)"
    );
    // Three identical leaves XOR back to a single `h` (h^h^h == h) but the TOTAL is 3 — the aggregate
    // still differs from one leaf via the counts, and from two via both.
    let three_identical = vec![
        LeafFact::new(Some(3), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Started),
    ];
    assert_eq!(recompute_incremental(&three_identical).total, 3);
}

/// A recompute AFTER a real leaf change is NOT suppressed (the suppression is precise — it swallows
/// only true no-ops, never a real change).
#[test]
fn changed_leaf_recompute_is_not_suppressed() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);
    consumer.put_leaf(&child, LeafFact::new(Some(3), StateCategory::Started));
    assert!(!consumer.recompute(&parent).is_suppressed());

    // The leaf transitions Started → Completed — a real change; the recompute is NOT suppressed.
    consumer.put_leaf(&child, LeafFact::new(Some(3), StateCategory::Completed));
    let outcome = consumer.recompute(&parent);
    assert!(
        !outcome.is_suppressed(),
        "a real leaf change must not be suppressed"
    );
    assert_eq!(outcome.aggregate().unwrap().done, 1);
}

// ── flush: coalesced recompute + the owed events ─────────────────────────────────────────────────

/// **Flush recomputes each dirty ancestor once + returns ONLY the changed ones (§6.1).** A burst marks
/// one ancestor dirty; flush recomputes it once and returns it (the owed `issue.rollup.recomputed`); a
/// second flush with no change returns empty (the suppression — no spurious event).
#[test]
fn flush_recomputes_once_and_suppresses_the_no_op() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let c0 = r("myelin://acme/issue/issue/C0");
    let c1 = r("myelin://acme/issue/issue/C1");
    consumer.add_parent_edge(&c0, &parent);
    consumer.add_parent_edge(&c1, &parent);
    consumer.put_leaf(&c0, LeafFact::new(Some(3), StateCategory::Completed));
    consumer.put_leaf(&c1, LeafFact::new(Some(5), StateCategory::Started));
    consumer.mark_changed(&c0);
    consumer.mark_changed(&c1);

    let changed = consumer.flush();
    assert_eq!(
        changed.len(),
        1,
        "two child changes under one parent → ONE recompute"
    );
    let (anc, agg) = &changed[0];
    assert_eq!(anc.0, parent.0);
    assert_eq!(agg.total, 2);
    assert_eq!(agg.done, 1);

    // A second flush with nothing dirty + no change → no events (the no-op suppression).
    consumer.mark_changed(&c0);
    assert!(
        consumer.flush().is_empty(),
        "a re-flush with no input change emits NO event (the suppression)"
    );
}

/// The owed `issue.rollup.recomputed` draft carries the derived counts + is PII-free (the
/// references-not-payloads discipline).
#[test]
fn rollup_recomputed_draft_is_pii_free_and_carries_counts() {
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let agg = RollupAggregate {
        total: 5,
        done: 2,
        estimate_sum: 13,
        input_hash: 0xABCD,
    };
    let draft = rollup_recomputed_draft(&parent, &agg);
    assert_eq!(draft.type_, EventType(events::ROLLUP_RECOMPUTED.into()));
    assert!(
        !draft.contains_personal_data,
        "the rollup aggregate is PII-free"
    );
    assert_eq!(draft.payload["done"], 2);
    assert_eq!(draft.payload["total"], 5);
    assert_eq!(draft.payload["estimate_sum"], 13);
}

// ── reindex-from-source: 0-drift parity (contract 2.6) ───────────────────────────────────────────

/// **Reindex-from-source rebuilds the rollup DRIFT-FREE (contract 2.6 / ISS-D8b).** Build a live
/// rollup, wipe the derived aggregates, reindex off the source (leaves + edges), and assert the rebuilt
/// snapshot is BYTE-IDENTICAL to the live one (steady-state and recovery share one code path; 0 drift).
#[test]
fn reindex_from_source_is_drift_free() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/EPIC-1");
    // A two-level tree: EPIC-1 ← STORY-1 ← {TASK-1, TASK-2}; plus a direct child STORY-2.
    let story1 = r("myelin://acme/issue/issue/STORY-1");
    let story2 = r("myelin://acme/issue/issue/STORY-2");
    let task1 = r("myelin://acme/issue/issue/TASK-1");
    let task2 = r("myelin://acme/issue/issue/TASK-2");
    consumer.add_parent_edge(&story1, &epic);
    consumer.add_parent_edge(&story2, &epic);
    consumer.add_parent_edge(&task1, &story1);
    consumer.add_parent_edge(&task2, &story1);
    consumer.put_leaf(&task1, LeafFact::new(Some(2), StateCategory::Completed));
    consumer.put_leaf(&task2, LeafFact::new(Some(3), StateCategory::Started));
    consumer.put_leaf(&story2, LeafFact::new(Some(8), StateCategory::Completed));

    // Build the live rollup (recompute every ancestor).
    consumer.mark_changed(&task1);
    consumer.mark_changed(&task2);
    consumer.mark_changed(&story2);
    let _ = consumer.flush();
    let live = aggregate_snapshot(&consumer);
    assert!(!live.is_empty(), "the live rollup has aggregates");

    // Reindex-from-source: wipe + rebuild off the SAME source.
    let rebuilt_count = consumer.reindex_from();
    let cold = aggregate_snapshot(&consumer);

    assert_eq!(
        live, cold,
        "the cold rebuild must byte-match the live rollup (0-drift reindex-parity, ISS-D8b)"
    );
    assert!(
        rebuilt_count >= 2,
        "EPIC-1 + STORY-1 are both rebuilt ancestors"
    );
    // The input_hash matches too (the no-op-suppression fingerprint survives a rebuild — one code path).
    for (k, live_agg) in &live {
        assert_eq!(
            live_agg.input_hash, cold[k].input_hash,
            "the input_hash is drift-free"
        );
    }
}

// ── the consumer template (contract 2.4) ─────────────────────────────────────────────────────────

/// **The consumer whitelist is `*`-free (BUS-3 / 2.4).** The rollup consumer binds the rollup-driving
/// `issue.*` deltas ONLY — never `*` (an over-broad subscription head-of-line-blocks everything).
#[test]
fn subjects_are_whitelisted_never_star() {
    let consumer = RollupConsumer::new();
    let subjects = consumer.subjects();
    assert!(!subjects.is_empty());
    for s in subjects {
        assert_ne!(s.0, "*", "the rollup consumer must NEVER bind `*` (BUS-3)");
        assert!(
            s.0.starts_with("issue."),
            "binds only issue.* deltas: {}",
            s.0
        );
    }
    assert!(subjects.iter().any(|s| s.0 == events::ISSUE_UPDATED));
}

/// **The handle is idempotent on `event_id` (contract 2.4 / ADR-04.1).** Re-handling the SAME event
/// does NOT re-dirty the ancestor (a redelivery is a no-op).
#[test]
fn handle_is_idempotent_on_event_id() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);

    let ev = updated_event("01J-1", &child.0);
    assert_eq!(consumer.handle(&ev), HandleOutcome::Done);
    assert_eq!(consumer.pending_recompute_count(), 1);

    // A redelivery of the SAME event_id is a no-op (the coalescer is not re-dirtied; the count holds).
    assert_eq!(consumer.handle(&ev), HandleOutcome::Done);
    assert_eq!(
        consumer.pending_recompute_count(),
        1,
        "a redelivery is idempotent — the ancestor is not re-dirtied"
    );
}

/// **A malformed event (no subject ref) is NON-RETRYABLE (poison).** It can never become well-formed
/// by retry — the consumer dead-letters it loudly rather than head-of-line-blocking.
#[test]
fn malformed_event_is_non_retryable() {
    let consumer = RollupConsumer::new();
    let mut ev = updated_event("01J-2", "");
    ev.subject = ArtifactRef(String::new());
    assert!(matches!(
        consumer.handle(&ev),
        HandleOutcome::NonRetryable(_)
    ));
}

/// The named floors are documented (VISION §3 / EI-01 §1 — name-your-floors): read-time rollup, the
/// debounce-window calibration, the cross-cell ancestor bridge, the forecast agent.
#[test]
fn the_floors_are_named() {
    assert!(RollupFloors::READ_TIME_ROLLUP.contains("ISS-P32"));
    assert!(RollupFloors::DEBOUNCE_WINDOW_CALIBRATION.contains("OQ-K"));
    assert!(RollupFloors::CROSS_CELL_ANCESTORS.contains("OQ-I"));
    assert!(RollupFloors::FORECAST_AGENT.contains("Monte-Carlo"));
    // The debounce window is per-tenant tunable (a default, NOT a frozen constant).
    assert_eq!(DebounceWindow::DEFAULT.width, 1);
}
