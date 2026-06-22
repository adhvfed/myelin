//! # The CDC pair for contract 5.5 — Git typed-edge (lifecycle) mirror edges (GIT-P19 / P-280)
//!
//! **Contract:**
//! - **5.5** the TE-7 typed-edge mirror — lifecycle edges (`closes`/`relates`/…) emitted by the producer
//!   subsystem, mirrored into the Refs projection as `rel_class='lifecycle'`; the producer is the source
//!   of truth, Refs holds the rebuildable projection + fixes the inverse pairing. Provider = Git's
//!   PR-lifecycle producer ([`myelin_git::typed_edges`]); consumer = the Refs mirror
//!   (`myelin_refs_service::mirror`).
//!
//! **The seam this pair pins.** Git is a producer LEAF and CANNOT depend on the Refs SERVICE crate (the
//! §2.9 acyclic DAG — and `myelin-refs-service` already depends ON `myelin-git`, so the edge is
//! one-directional by construction). So the Git-owned producer half
//! ([`myelin_git::typed_edges::emit_lifecycle_edges`]) must emit the **byte-identical**
//! `refs.edge.created` (`rel_class='lifecycle'`) wire shape the Refs mirror consumer ingests. This CDC
//! models the CONSUMER half locally — the exact field reads + the deterministic `edge_id` derivation
//! (`edge_id`) + the mirror's FORWARD + INVERSE projection (`mirror_edges`: `relates` is symmetric, the
//! endpoints swapped; `closes` has no frozen inverse token yet → forward only) — and PROVES the
//! provider's emitted envelope ingests through it with the correct edge identity AND that the Refs
//! mirror would project the expected forward/inverse pair. A drift on either side fails this one CI job.

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

// ── The CONSUMER half (the Refs mirror's field reads + the deterministic edge_id + the forward/inverse
//    projection), modelled here so this crate need not depend on the Refs service crate (the §2.9 DAG
//    one-directional edge). These MUST stay byte-identical to `myelin_refs_service::edge_builder::edge_id`
//    and `myelin_refs_service::mirror::{LifecycleRel, mirror_edges}`. ─────────────────────────────────

/// The deterministic `edge_id = hash(tenant, source, target, rel)` — byte-identical to
/// `myelin_refs_service::edge_builder::edge_id` (FNV-1a 128-bit over the NUL-separated tuple).
fn consumer_edge_id(tenant: &str, source: &str, target: &str, rel: &str) -> String {
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d; // FNV-1a 128-bit offset basis.
    const PRIME: u128 = 0x0000000001000000000000000000013b; // FNV-1a 128-bit prime.
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u128;
            h = h.wrapping_mul(PRIME);
        }
        h ^= 0x00;
        h = h.wrapping_mul(PRIME);
    };
    feed(tenant.as_bytes());
    feed(source.as_bytes());
    feed(target.as_bytes());
    feed(rel.as_bytes());
    format!("{h:032x}")
}

/// The consumer's view of a projected lifecycle edge (the fields the Refs mirror's `EdgeRow` carries).
#[derive(Debug, PartialEq, Eq, Clone)]
struct ProjectedEdge {
    edge_id: String,
    source: String,
    target: String,
    rel: String,
    rel_class: String,
}

/// The CONSUMER's inverse pairing — byte-identical to `myelin_refs_service::mirror::LifecycleRel::inverse`
/// for the two rels Git produces. `relates` is SYMMETRIC (same rel, endpoints swapped); `closes` has no
/// frozen inverse token yet (forward only — the REF-P18/REF-P20 floor). `None` here means "no inverse
/// leg projected".
fn consumer_inverse(rel: &str) -> Option<&'static str> {
    match rel {
        "relates" => Some("relates"), // symmetric: same rel, endpoints swapped.
        "closes" => None,             // Inverse::None floor — forward only.
        other => panic!("git only produces closes/relates, got {other}"),
    }
}

/// The CONSUMER mirror: read `source`/`target`/`rel`/`rel_class` off the `refs.edge.created` envelope,
/// validate the lifecycle class, then project the FORWARD lifecycle edge AND (if the rel has a frozen
/// inverse) the INVERSE edge with the endpoints swapped — exactly what `mirror_edges` does. Each row
/// carries the deterministic `edge_id`. Returns `Err(field)` if a required field is missing (fail-closed).
fn consumer_mirror(env: &EventEnvelope) -> Result<Vec<ProjectedEdge>, String> {
    assert_eq!(env.type_.0, "refs.edge.created", "the mirror ingests refs.edge.created");
    let p = &env.payload;
    let get = |k: &str| p.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let source = get("source").ok_or_else(|| "source".to_string())?;
    let target = get("target").ok_or_else(|| "target".to_string())?;
    let rel = get("rel").ok_or_else(|| "rel".to_string())?;
    let rel_class = get("rel_class").ok_or_else(|| "rel_class".to_string())?;
    assert_eq!(rel_class, "lifecycle", "a typed-edge mirror edge MUST be lifecycle-class");

    let tenant = &env.tenant.0;
    // Forward leg.
    let mut rows = vec![ProjectedEdge {
        edge_id: consumer_edge_id(tenant, &source, &target, &rel),
        source: source.clone(),
        target: target.clone(),
        rel: rel.clone(),
        rel_class: rel_class.clone(),
    }];
    // Inverse leg (the Refs mirror's, not Git's): endpoints swapped, the inverse rel token.
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

/// The PR's `git.pr.merged` lifecycle event (the CAUSE) — the merge write holds it in hand; the
/// lifecycle edges co-commit with the PR state change in the SAME transaction.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 5.5 — the PROVIDER (Git PR lifecycle) emits the wire shape the CONSUMER (Refs mirror) ingests
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **A Git-emitted `closes` lifecycle edge mirrors through the Refs consumer to the SAME deterministic
/// `edge_id`, with `rel_class = lifecycle` and FORWARD-ONLY projection** (`closes` has no frozen inverse
/// token yet — the REF-P18/REF-P20 floor). This is the 5.5 provider↔consumer equivalence for the
/// trailer-driven close edge.
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
    // FORWARD ONLY — closes has no frozen inverse (the floor).
    assert_eq!(projected.len(), 1, "closes mirrors forward-only (no frozen inverse token yet)");
    assert_eq!(projected[0].rel, "closes");
    assert_eq!(projected[0].rel_class, "lifecycle");
    assert_eq!(projected[0].source, source.0);
    assert_eq!(projected[0].target, issue("ENG-1").0);
    let expected = consumer_edge_id("acme", &source.0, &issue("ENG-1").0, "closes");
    assert_eq!(projected[0].edge_id, expected, "the deterministic edge_id is provider/consumer-stable");
}

/// **A Git-emitted `relates` lifecycle edge mirrors through the Refs consumer to BOTH directions** (the
/// symmetric pair — the Refs mirror projects the forward AND the endpoint-swapped inverse, both
/// `relates`, both lifecycle-class). Git emits ONLY the forward; the BOTH-directions traversal is the
/// Refs mirror's discipline.
#[test]
fn git_relates_edge_mirrors_to_both_directions_through_the_refs_consumer() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = git_pr_ref("acme", "repo7", 42);
    let linked = git_pr_ref("acme", "repo7", 7);

    let ev = merged_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git pr 42 links pr 7");
    let ids = emit_lifecycle_edges(&mut tx, &source, &[], std::slice::from_ref(&linked), &ev).unwrap();
    tx.commit().unwrap();

    // Git emits exactly ONE forward event (the inverse is the Refs mirror's projection, not a second emit).
    assert_eq!(ids.len(), 1, "Git emits ONE forward relates event; Refs mirrors the inverse");

    let env = outbox.row(&ids[0]).unwrap().envelope;
    let projected = consumer_mirror(&env).expect("the Refs mirror ingests the relates edge");
    assert_eq!(projected.len(), 2, "relates is symmetric → the mirror projects BOTH directions");

    // forward: PR 42 → PR 7.
    assert_eq!(projected[0].rel, "relates");
    assert_eq!(projected[0].source, source.0);
    assert_eq!(projected[0].target, linked.0);
    // inverse: PR 7 → PR 42 (same rel, endpoints swapped).
    assert_eq!(projected[1].rel, "relates");
    assert_eq!(projected[1].source, linked.0);
    assert_eq!(projected[1].target, source.0);
    // both legs carry their own deterministic edge_id (distinct rows, idempotent rebuild).
    assert_eq!(projected[0].edge_id, consumer_edge_id("acme", &source.0, &linked.0, "relates"));
    assert_eq!(projected[1].edge_id, consumer_edge_id("acme", &linked.0, &source.0, "relates"));
    assert_ne!(projected[0].edge_id, projected[1].edge_id, "the two legs are distinct edge rows");
}

/// **The frozen lifecycle tokens are exactly the Refs mirror wire tokens** (the names anchor X-5; no
/// second vocabulary). The lifecycle class is NEVER the reference class (a content edge's).
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
