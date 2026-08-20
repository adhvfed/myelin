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

#[test]
fn re_sum_counts_done_total_and_excludes_cancelled() {
    let leaves = vec![
        LeafFact::new(Some(3), StateCategory::Completed),
        LeafFact::new(Some(5), StateCategory::Completed),
        LeafFact::new(Some(2), StateCategory::Started),
        LeafFact::new(Some(8), StateCategory::Cancelled),
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

#[test]
fn empty_subtree_is_zero_progress() {
    assert_eq!(recompute_incremental(&[]).progress(), 0.0);
    let all_cancelled = vec![LeafFact::new(Some(1), StateCategory::Cancelled)];
    let agg = recompute_incremental(&all_cancelled);
    assert_eq!(agg.total, 0);
    assert_eq!(agg.progress(), 0.0);
}

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

    let agg = recompute_incremental(std::slice::from_ref(&fact));
    store.aggregates.insert(anc.0.clone(), agg.clone());
    assert_eq!(
        store.aggregate(&anc),
        Some(&agg),
        "aggregate reads the derived value"
    );

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

#[test]
fn ancestor_walk_is_cycle_safe() {
    let mut graph = IssueRelationGraph::new();
    let a = r("myelin://acme/issue/issue/A");
    let b = r("myelin://acme/issue/issue/B");
    graph.add_edge(&a, &b, IssueLifecycleRel::Parent);
    graph.add_edge(&b, &a, IssueLifecycleRel::Parent);
    let ancestors = walk_parent_edges(&graph, &a);
    assert!(ancestors.iter().any(|n| n.0 == b.0));
    assert!(
        ancestors.len() <= 2,
        "a cycle yields a finite ancestor set, not an infinite walk"
    );
}

#[test]
fn ancestor_walk_is_depth_bounded() {
    let mut graph = IssueRelationGraph::new();
    let nodes: Vec<ArtifactRef> = (0..30)
        .map(|i| r(&format!("myelin://acme/issue/issue/I{i}")))
        .collect();
    for i in 0..29 {
        graph.add_edge(&nodes[i], &nodes[i + 1], IssueLifecycleRel::Parent);
    }
    let ancestors = walk_parent_edges(&graph, &nodes[0]);
    assert!(
        ancestors.len() <= TRAVERSE_MAX_DEPTH,
        "no ancestor past the depth-16 ceiling: got {}",
        ancestors.len()
    );
    assert_eq!(TRAVERSE_MAX_DEPTH, 16);
}

#[test]
fn debounce_coalesces_a_burst_into_one_recompute() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let children: Vec<ArtifactRef> = (0..100)
        .map(|i| r(&format!("myelin://acme/issue/issue/C{i}")))
        .collect();
    for c in &children {
        consumer.add_parent_edge(c, &parent);
        consumer.mark_changed(c);
    }
    assert_eq!(
        consumer.pending_recompute_count(),
        1,
        "a burst of 100 child changes under one parent coalesces to ONE recompute"
    );
}

#[test]
fn unchanged_recompute_is_suppressed_no_event() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);
    consumer.put_leaf(&child, LeafFact::new(Some(3), StateCategory::Completed));

    let first = consumer.recompute(&parent);
    assert!(
        !first.is_suppressed(),
        "the first recompute writes the aggregate"
    );
    assert_eq!(first.aggregate().unwrap().done, 1);

    let second = consumer.recompute(&parent);
    assert!(
        second.is_suppressed(),
        "an unchanged recompute is suppressed - NO event (AG-6 loop-storm guard)"
    );
}

#[test]
fn input_hash_fold_is_xor_distinguishing_multiplicity() {
    let one = vec![LeafFact::new(Some(3), StateCategory::Started)];
    let two_identical = vec![
        LeafFact::new(Some(3), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Started),
    ];
    assert_ne!(
        recompute_incremental(&one).total,
        recompute_incremental(&two_identical).total
    );
    assert_ne!(
        recompute_incremental(&one).input_hash,
        recompute_incremental(&two_identical).input_hash,
        "the XOR fold distinguishes a multiplicity change (an OR fold would not)"
    );
    let three_identical = vec![
        LeafFact::new(Some(3), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Started),
        LeafFact::new(Some(3), StateCategory::Started),
    ];
    assert_eq!(recompute_incremental(&three_identical).total, 3);
}

#[test]
fn changed_leaf_recompute_is_not_suppressed() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);
    consumer.put_leaf(&child, LeafFact::new(Some(3), StateCategory::Started));
    assert!(!consumer.recompute(&parent).is_suppressed());

    consumer.put_leaf(&child, LeafFact::new(Some(3), StateCategory::Completed));
    let outcome = consumer.recompute(&parent);
    assert!(
        !outcome.is_suppressed(),
        "a real leaf change must not be suppressed"
    );
    assert_eq!(outcome.aggregate().unwrap().done, 1);
}

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

    consumer.mark_changed(&c0);
    assert!(
        consumer.flush().is_empty(),
        "a re-flush with no input change emits NO event (the suppression)"
    );
}

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

#[test]
fn reindex_from_source_is_drift_free() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/EPIC-1");
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

    consumer.mark_changed(&task1);
    consumer.mark_changed(&task2);
    consumer.mark_changed(&story2);
    let _ = consumer.flush();
    let live = aggregate_snapshot(&consumer);
    assert!(!live.is_empty(), "the live rollup has aggregates");

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
    for (k, live_agg) in &live {
        assert_eq!(
            live_agg.input_hash, cold[k].input_hash,
            "the input_hash is drift-free"
        );
    }
}

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

#[test]
fn handle_is_idempotent_on_event_id() {
    let consumer = RollupConsumer::new();
    let parent = r("myelin://acme/issue/issue/EPIC-1");
    let child = r("myelin://acme/issue/issue/C0");
    consumer.add_parent_edge(&child, &parent);

    let ev = updated_event("01J-1", &child.0);
    assert_eq!(
        consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done
    );
    assert_eq!(consumer.pending_recompute_count(), 1);

    assert_eq!(
        consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done
    );
    assert_eq!(
        consumer.pending_recompute_count(),
        1,
        "a redelivery is idempotent - the ancestor is not re-dirtied"
    );
}

#[test]
fn malformed_event_is_non_retryable() {
    let consumer = RollupConsumer::new();
    let mut ev = updated_event("01J-2", "");
    ev.subject = ArtifactRef(String::new());
    assert!(matches!(
        consumer.handle(&ev, &mut myelin_events::HandlerTx::none()),
        HandleOutcome::NonRetryable(_)
    ));
}
