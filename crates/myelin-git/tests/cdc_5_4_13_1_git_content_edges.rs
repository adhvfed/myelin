//! # The CDC pair for contracts 5.4 + 13.1 — Git content-node reference edges (GIT-P17 / P-278)
//!
//! **Contracts:**
//! - **5.4** `refs.edge.created` — emitted by producers via the outbox; the `mention`/`artifact_ref`/
//!   `embed` content nodes are the producers; **no standalone edge-write API**. Provider = Git's body
//!   producer ([`myelin_git::body`]); consumer = the Refs edge-builder
//!   (`myelin_refs_service::edge_builder::RefsEdgeBuilder`).
//! - **13.1** the `myelin-content` markdown-subset for the body content (`render(parse(md)) === md`) +
//!   the three structured inline nodes. Provider = Knowledge (freezes the taxonomy); Git CONSUMES the
//!   frozen subset for its PR/review/comment bodies.
//!
//! **The seam this pair pins.** Git is a producer LEAF and CANNOT depend on the Refs SERVICE crate (the
//! §2.9 acyclic DAG — and `myelin-refs-service` already depends ON `myelin-git`, so the edge is
//! one-directional by construction). So the Git-owned producer half ([`myelin_git::body::emit_body_edges`])
//! must emit the **byte-identical** `refs.edge.created` wire shape the Refs edge-builder consumes. This
//! CDC models the CONSUMER half locally (the exact field reads + the deterministic `edge_id` derivation
//! `RefsEdgeBuilder::apply_created` + `edge_id` perform) and PROVES the provider's emitted envelope
//! ingests through it with the correct edge identity — so a drift on either side fails this one CI job.
//! (This mirrors `cdc_2_2_2_3_git_ref_updated.rs`, which pins the `git.ref.updated` wire shape with a
//! local consumer decoder for the same reason.)

use std::sync::Arc;

use myelin_content::{InlineNode, OBJ};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_git::body::{emit_body_edges, extract_body_edges, Body, EdgeRel};
use myelin_git::events::GIT_COMMENT_CREATED;
use myelin_git::subs::mint_pr_comment;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

// ── The CONSUMER half (the Refs edge-builder's field reads + the deterministic edge_id), modelled here
//    so this crate need not depend on the Refs service crate (the §2.9 DAG one-directional edge). These
//    MUST stay byte-identical to `myelin_refs_service::edge_builder::{edge_id, apply_created}`. ────────

/// The deterministic `edge_id = hash(tenant, source, target, rel)` — byte-identical to
/// `myelin_refs_service::edge_builder::edge_id` (the FNV-1a 128-bit over the NUL-separated tuple). The
/// consumer derives the idempotency key from the PROVIDER's payload triple; a replay of the same logical
/// edge upserts the same row. Pinned here so a Git-emitted edge resolves to the SAME id the Refs
/// consumer computes.
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

/// The consumer's decoded edge row (exactly the fields `RefsEdgeBuilder::apply_created` reads off a
/// `refs.edge.created` envelope). A decode failure names the missing field (the consumer's fail-closed
/// poison) — so the CDC also proves the provider never omits a required field.
#[derive(Debug, PartialEq, Eq)]
struct DecodedEdge {
    edge_id: String,
    source: String,
    target: String,
    rel: String,
    rel_class: String,
}

/// The CONSUMER decode: read `source`/`target`/`rel`/`rel_class` off the envelope payload + derive the
/// deterministic `edge_id` from `(tenant, source, target, rel)` — exactly what the Refs edge-builder
/// does on `*.created`. Returns `Err(field)` if a required field is missing (fail-closed).
fn consumer_decode(env: &EventEnvelope) -> Result<DecodedEdge, String> {
    assert_eq!(env.type_.0, "refs.edge.created", "the consumer only ingests refs.edge.created here");
    let p = &env.payload;
    let get = |k: &str| p.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let source = get("source").ok_or_else(|| "source".to_string())?;
    let target = get("target").ok_or_else(|| "target".to_string())?;
    let rel = get("rel").ok_or_else(|| "rel".to_string())?;
    let rel_class = get("rel_class").ok_or_else(|| "rel_class".to_string())?;
    let edge_id = consumer_edge_id(&env.tenant.0, &source, &target, &rel);
    Ok(DecodedEdge { edge_id, source, target, rel, rel_class })
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
        caused_by: Some(CausedBy("session:c1".into())),
    }
}

fn alice() -> Principal {
    Principal::stub(PrincipalId("p-alice".into()), PrincipalKind::Human, TenantId("acme".into()))
}

/// The body's `git.comment.created` content event (the CAUSE) — constructed directly (the comment write
/// holds it in hand). The edges co-commit with the body row in the SAME transaction.
fn content_event(source: &ArtifactRef) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-comment".into()),
        type_: EventType(GIT_COMMENT_CREATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(alice()),
        subject: source.clone(),
        aggregate: AggregateKey("git/pr/repo7:42".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-comment-corr".into()),
        caused_by: Some(CausedBy("session:c1".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "comment_ref": source.0 }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 5.4 — the PROVIDER (Git body) emits the wire shape the CONSUMER (Refs edge-builder) ingests
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **The Git-emitted `refs.edge.created` decodes cleanly through the Refs edge-builder's reads, with the
/// SAME deterministic `edge_id`, `rel`, and `reference` class.** This is the 5.4 provider↔consumer
/// equivalence: a Git body's structured nodes produce edges the Refs index ingests byte-compatibly.
#[test]
fn git_body_edges_decode_through_the_refs_consumer_with_the_right_edge_id() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();

    let nodes = vec![
        InlineNode::Mention(alice()),
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into())),
    ];

    let ce = content_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git comment cAbc body written");
    let ids = emit_body_edges(&mut tx, &source, &nodes, &ce).unwrap();
    tx.commit().unwrap();

    // the PROVIDER's view of the edge set (the same extraction the emit ran).
    let provider_edges = extract_body_edges(&source, &nodes);
    assert_eq!(provider_edges.len(), 3);

    // every emitted envelope decodes through the CONSUMER, to the SAME edge_id the provider's triple
    // would key, with rel_class = reference.
    for (id, p_edge) in ids.iter().zip(provider_edges.iter()) {
        let env = outbox.row(id).unwrap().envelope;
        let decoded = consumer_decode(&env).expect("the consumer decodes the Git edge");
        assert_eq!(decoded.rel_class, "reference", "content-node edges are reference-class");
        assert_eq!(decoded.rel, p_edge.rel.as_str(), "the provider rel == the consumer-decoded rel");
        assert_eq!(decoded.source, source.0);
        assert_eq!(decoded.target, p_edge.target.0);
        // the consumer-derived edge_id matches the provider triple → idempotent ingest, no drift.
        let expected = consumer_edge_id("acme", &source.0, &p_edge.target.0, p_edge.rel.as_str());
        assert_eq!(decoded.edge_id, expected, "the deterministic edge_id is provider/consumer-stable");
    }
}

/// **The frozen X-2 uniform mapping is provider/consumer-agreed: mention→mentions, artifact_ref→links,
/// embed→embeds.** Pinned at the wire level (the consumer-decoded `rel` per node kind).
#[test]
fn the_three_node_kinds_map_to_the_frozen_rels() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();

    let cases = [
        (InlineNode::Mention(alice()), EdgeRel::Mentions, "mentions"),
        (
            InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
            EdgeRel::Links,
            "links",
        ),
        (
            InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into())),
            EdgeRel::Embeds,
            "embeds",
        ),
    ];

    for (node, rel, wire) in cases {
        assert_eq!(rel.as_str(), wire);
        let ce = content_event(&source);
        let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("git comment cAbc body written");
        let ids = emit_body_edges(&mut tx, &source, std::slice::from_ref(&node), &ce).unwrap();
        tx.commit().unwrap();
        let env = outbox.row(&ids[0]).unwrap().envelope;
        assert_eq!(consumer_decode(&env).unwrap().rel, wire, "{node:?} → `{wire}`");
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// 13.1 — the body content is the frozen myelin-content subset (render(parse(md)) === md)
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// **The Git body is the frozen `myelin-content` subset: `render(parse(md)) === md`** (13.1). A body
/// mixing marks + structured nodes round-trips byte-identically through the ONE editor render path —
/// Git CONSUMES the frozen subset, it does not author a second content model.
#[test]
fn git_body_is_the_frozen_content_subset_and_round_trips() {
    let body = Body::new(
        format!("**ship it** — see {OBJ} and `cargo test` per [doc](https://x.test/d)"),
        vec![InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2".into()))],
    );
    assert!(body.round_trips(), "render(parse(md)) === md (the 13.1 gate on git bodies)");
    // the structured node is preserved positionally (one OBJ ⇒ one node).
    assert_eq!(body.parse().nodes.len(), 1);
}
