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

    let n = 10_000usize;
    for i in 0..n {
        let child = r(&format!("myelin://acme/issue/issue/ENG-T{i}"));
        let story = &stories[i % stories.len()];
        consumer.add_parent_edge(&child, story);
        consumer.put_leaf(&child, LeafFact::new(Some(1), StateCategory::Started));
        let outcome = consumer.handle(&updated_event(&format!("imp-{i}"), &child.0), &mut myelin_events::HandlerTx::none());
        assert_eq!(outcome, HandleOutcome::Done);
    }

    let bound = consumer.pending_recompute_count();
    assert_eq!(
        bound, 4,
        "ISS-D8(a): a 10k import coalesces to the 4 distinct ancestors (epic + 3 stories), \
         not 10,000 recomputes - the debounce bound"
    );
    assert!(
        bound < n,
        "ISS-D8(a) GREEN: recompute count {bound} << import size {n} (bounded, debounced)"
    );

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

#[test]
fn iss_d8b_reindex_from_source_rebuilds_drift_free() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/ENG-EPIC");
    let story = r("myelin://acme/issue/issue/ENG-STORY");
    consumer.add_parent_edge(&story, &epic);

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
        consumer.handle(&updated_event(&format!("d8b-{i}"), &t.0), &mut myelin_events::HandlerTx::none());
    }
    let _ = consumer.flush();
    let live = aggregate_snapshot(&consumer);
    let story_agg = &live[&story.0];
    assert_eq!(story_agg.total, 5);
    assert_eq!(story_agg.done, 3);

    let rebuilt = consumer.reindex_from();
    let cold = aggregate_snapshot(&consumer);

    assert_eq!(
        live, cold,
        "ISS-D8(b) GREEN: the cold rebuild byte-matches the live rollup (0-drift reindex-parity)"
    );
    assert!(rebuilt >= 2, "the epic + the story are both rebuilt");
}

#[test]
fn chained_mutation_e2e_import_bounded_replay_drift_free() {
    let consumer = RollupConsumer::new();
    let epic = r("myelin://acme/issue/issue/CHAIN-EPIC");
    let story = r("myelin://acme/issue/issue/CHAIN-STORY");
    consumer.add_parent_edge(&story, &epic);

    for i in 0..50 {
        let t = r(&format!("myelin://acme/issue/issue/CHAIN-T{i}"));
        consumer.add_parent_edge(&t, &story);
        let cat = if i % 2 == 0 {
            StateCategory::Completed
        } else {
            StateCategory::Started
        };
        consumer.put_leaf(&t, LeafFact::new(Some(1), cat));
        consumer.handle(&updated_event(&format!("chain-{i}"), &t.0), &mut myelin_events::HandlerTx::none());
    }
    assert_eq!(consumer.pending_recompute_count(), 2);

    let _ = consumer.flush();
    let live = aggregate_snapshot(&consumer);
    assert_eq!(live[&story.0].total, 50);
    assert_eq!(live[&story.0].done, 25);

    consumer.reindex_from();
    assert_eq!(
        live,
        aggregate_snapshot(&consumer),
        "the replay rebuilds the rollup drift-free vs live (chained-mutation e2e)"
    );
}

#[test]
fn cdc_2_6_rollup_snapshot_replay_is_deterministic() {
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
