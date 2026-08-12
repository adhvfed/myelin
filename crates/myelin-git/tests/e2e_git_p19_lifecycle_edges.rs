use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_git::events::GIT_PR_MERGED;
use myelin_git::lifecycle::{PrState, PrTransition, PullRequest};
use myelin_git::project::git_pr_ref;
use myelin_git::typed_edges::{emit_lifecycle_edges, parse_closes_trailers};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p-author".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:merge-1".into())),
    }
}

fn merged_event(source: &ArtifactRef) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-merged".into()),
        type_: EventType(GIT_PR_MERGED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p-author".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: source.clone(),
        aggregate: AggregateKey("git/pr/repo7:42".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-merged-corr".into()),
        caused_by: Some(CausedBy("session:merge-1".into())),
        depth: 2,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "pr_ref": source.0, "merged": true }),
    }
}

#[test]
fn open_pr_with_closes_trailer_then_merge_emits_exactly_one_closes_edge() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = git_pr_ref("acme", "repo7", 42).unwrap();

    let mut pr = PullRequest::open(
        42,
        "refs/heads/main",
        "refs/heads/feature",
        "psn:alice",
        false,
    );
    assert_eq!(pr.state, PrState::Open);
    let merge_message = "Land the charge fix\n\nReviewed and tested.\nCloses ENG-1\n";

    let issue_keys = parse_closes_trailers(merge_message).expect("bounded fixture");
    assert_eq!(
        issue_keys,
        vec!["ENG-1".to_string()],
        "exactly the one trailer key"
    );
    let closes_targets: Vec<ArtifactRef> = issue_keys
        .iter()
        .map(|k| ArtifactRef(format!("myelin://acme/issue/issue/{k}")))
        .collect();

    assert_eq!(
        pr.transition(PrTransition::Merge, true).unwrap(),
        PrState::Merged
    );
    let ev = merged_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git pr 42 merged");
    let edge_ids = emit_lifecycle_edges(&mut tx, &source, &closes_targets, &[], &ev)
        .expect("the lifecycle edges emit");

    assert_eq!(
        edge_ids.len(),
        1,
        "one Closes trailer → exactly one closes edge"
    );

    assert_eq!(outbox.committed_count(), 0, "nothing durable before commit");
    tx.commit()
        .expect("the merge + the lifecycle edge co-commit");
    assert_eq!(
        outbox.committed_count(),
        1,
        "the closes edge co-committed with the merge"
    );

    let env = outbox.row(&edge_ids[0]).unwrap().envelope;
    assert_eq!(
        env.type_.0, "refs.edge.created",
        "the lifecycle edge is a refs.edge.created"
    );
    assert_eq!(env.payload["rel"], "closes", "the trailer → a closes edge");
    assert_eq!(
        env.payload["rel_class"], "lifecycle",
        "a typed-edge mirror edge is lifecycle-class"
    );
    assert_eq!(
        env.payload["source"], source.0,
        "the source is the merged PR URN"
    );
    assert_eq!(
        env.payload["target"], "myelin://acme/issue/issue/ENG-1",
        "the target is the issue URN"
    );
    assert_eq!(
        env.correlation_id, ev.correlation_id,
        "the edge carries the merge's correlation root"
    );
    assert_eq!(
        env.causation_id.as_ref().map(|c| &c.0),
        Some(&ev.event_id.0),
        "the edge's causation is the merge event"
    );
    assert_eq!(
        env.depth,
        ev.depth + 1,
        "the edge is depth+1 (the loop-guard stamp)"
    );
}

#[test]
fn aborted_merge_emits_zero_lifecycle_edges() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = git_pr_ref("acme", "repo7", 42).unwrap();
    let closes = vec![ArtifactRef("myelin://acme/issue/issue/ENG-1".into())];

    {
        let ev = merged_event(&source);
        let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("git pr 42 merged");
        let ids = emit_lifecycle_edges(&mut tx, &source, &closes, &[], &ev).unwrap();
        assert_eq!(ids.len(), 1, "one edge was buffered");
    }
    assert_eq!(
        outbox.committed_count(),
        0,
        "an aborted merge commits 0 rows (no ghost lifecycle edge)"
    );
}

#[test]
fn plain_merge_without_trailer_or_link_emits_zero_edges() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = git_pr_ref("acme", "repo7", 42).unwrap();

    let message = "This closes a long-standing gap in the charge path, finally.";
    let keys = parse_closes_trailers(message).expect("bounded fixture");
    assert!(keys.is_empty(), "a prose `closes` is not a trailer");

    let ev = merged_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git pr 42 merged");
    let ids = emit_lifecycle_edges(&mut tx, &source, &[], &[], &ev).unwrap();
    tx.commit().unwrap();
    assert!(ids.is_empty(), "a plain merge produces 0 lifecycle edges");
    assert_eq!(
        outbox.committed_count(),
        0,
        "no lifecycle edge rows committed"
    );
}
