use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_git::events::GIT_PR_MERGED;
use myelin_git::project::git_pr_ref;
use myelin_git::typed_edges::{emit_lifecycle_edges, extract_lifecycle_edges, LifecycleRel};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn consumer_edge_id(tenant: &str, source: &str, target: &str, rel: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.refs.edge.v2");
    for field in [
        tenant.as_bytes(),
        source.as_bytes(),
        target.as_bytes(),
        rel.as_bytes(),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

#[derive(Debug, PartialEq, Eq, Clone)]
struct ProjectedEdge {
    edge_id: String,
    source: String,
    target: String,
    rel: String,
    rel_class: String,
}

fn consumer_inverse(rel: &str) -> Option<&'static str> {
    match rel {
        "relates" => Some("relates"),
        "closes" => None,
        other => panic!("git only produces closes/relates, got {other}"),
    }
}

fn consumer_mirror(env: &EventEnvelope) -> Result<Vec<ProjectedEdge>, String> {
    assert_eq!(
        env.type_.0, "refs.edge.created",
        "the mirror ingests refs.edge.created"
    );
    let p = &env.payload;
    let get = |k: &str| p.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let source = get("source").ok_or_else(|| "source".to_string())?;
    let target = get("target").ok_or_else(|| "target".to_string())?;
    let rel = get("rel").ok_or_else(|| "rel".to_string())?;
    let rel_class = get("rel_class").ok_or_else(|| "rel_class".to_string())?;
    assert_eq!(
        rel_class, "lifecycle",
        "a typed-edge mirror edge MUST be lifecycle-class"
    );

    let tenant = &env.tenant.0;
    let mut rows = vec![ProjectedEdge {
        edge_id: consumer_edge_id(tenant, &source, &target, &rel),
        source: source.clone(),
        target: target.clone(),
        rel: rel.clone(),
        rel_class: rel_class.clone(),
    }];
    if let Some(inv) = consumer_inverse(&rel) {
        rows.push(ProjectedEdge {
            edge_id: consumer_edge_id(tenant, &target, &source, inv),
            source: target.clone(),
            target: source.clone(),
            rel: inv.to_string(),
            rel_class,
        });
    }
    Ok(rows)
}

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
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "pr_ref": source.0, "merged": true }),
    }
}

fn issue(key: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/issue/issue/{key}"))
}

#[test]
fn git_closes_edge_mirrors_through_the_refs_consumer_forward_only() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = git_pr_ref("acme", "repo7", 42);
    let closes = vec![issue("ENG-1")];

    let ev = merged_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git pr 42 merged");
    let ids = emit_lifecycle_edges(&mut tx, &source, &closes, &[], &ev).unwrap();
    tx.commit().unwrap();

    let provider = extract_lifecycle_edges(&source, &closes, &[]);
    assert_eq!(provider.len(), 1);
    assert_eq!(provider[0].rel, LifecycleRel::Closes);

    let env = outbox.row(&ids[0]).unwrap().envelope;
    let projected = consumer_mirror(&env).expect("the Refs mirror ingests the Git lifecycle edge");
    assert_eq!(
        projected.len(),
        1,
        "closes mirrors forward-only (no frozen inverse token yet)"
    );
    assert_eq!(projected[0].rel, "closes");
    assert_eq!(projected[0].rel_class, "lifecycle");
    assert_eq!(projected[0].source, source.0);
    assert_eq!(projected[0].target, issue("ENG-1").0);
    let expected = consumer_edge_id("acme", &source.0, &issue("ENG-1").0, "closes");
    assert_eq!(
        projected[0].edge_id, expected,
        "the deterministic edge_id is provider/consumer-stable"
    );
}

#[test]
fn git_relates_edge_mirrors_to_both_directions_through_the_refs_consumer() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = git_pr_ref("acme", "repo7", 42);
    let linked = git_pr_ref("acme", "repo7", 7);

    let ev = merged_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git pr 42 links pr 7");
    let ids =
        emit_lifecycle_edges(&mut tx, &source, &[], std::slice::from_ref(&linked), &ev).unwrap();
    tx.commit().unwrap();

    assert_eq!(
        ids.len(),
        1,
        "Git emits ONE forward relates event; Refs mirrors the inverse"
    );

    let env = outbox.row(&ids[0]).unwrap().envelope;
    let projected = consumer_mirror(&env).expect("the Refs mirror ingests the relates edge");
    assert_eq!(
        projected.len(),
        2,
        "relates is symmetric → the mirror projects BOTH directions"
    );

    assert_eq!(projected[0].rel, "relates");
    assert_eq!(projected[0].source, source.0);
    assert_eq!(projected[0].target, linked.0);
    assert_eq!(projected[1].rel, "relates");
    assert_eq!(projected[1].source, linked.0);
    assert_eq!(projected[1].target, source.0);
    assert_eq!(
        projected[0].edge_id,
        consumer_edge_id("acme", &source.0, &linked.0, "relates")
    );
    assert_eq!(
        projected[1].edge_id,
        consumer_edge_id("acme", &linked.0, &source.0, "relates")
    );
    assert_ne!(
        projected[0].edge_id, projected[1].edge_id,
        "the two legs are distinct edge rows"
    );
}

#[test]
fn git_lifecycle_tokens_match_the_refs_mirror_vocabulary() {
    assert_eq!(LifecycleRel::Closes.as_str(), "closes");
    assert_eq!(LifecycleRel::Relates.as_str(), "relates");
    assert_eq!(myelin_git::typed_edges::REL_CLASS_LIFECYCLE, "lifecycle");
    assert_ne!(
        myelin_git::typed_edges::REL_CLASS_LIFECYCLE,
        myelin_git::body::REL_CLASS_REFERENCE,
        "the lifecycle class never aliases the reference class"
    );
}
