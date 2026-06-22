//! **REF-P8 / P-157 — the edge-extraction emit seam (contract 5.4 EMIT side) CDC + chained
//! emit-iff-committed tests.**
//!
//! These run on the default `cargo test --workspace` (DB-free): they exercise the producer seam
//! through the REAL [`myelin_events::OutboxTransaction`] (the same-tx co-commit, contract 2.2) over
//! the in-memory [`myelin_events::OutboxStore`] (which models the §3.2 outbox table's emit-iff-
//! committed semantics exactly). The REAL Postgres outbox `INSERT` + the relay-visible row count are
//! proven against the live dev stack in `tests/integration_ref_p8_emit_seam.rs` (the `integration`
//! feature).
//!
//! What is proven here:
//! - **CDC 5.4 (provider/emit side):** a content document with N structured ref nodes emits exactly N
//!   `refs.edge.created` events, each via `OutboxTx::emit(draft, cause = Some(content_event))` — there
//!   is NO standalone edge-write API; the only emit path is the outbox.
//! - **One edge per structured node, correct rel/rel_class** (mention→mentions, artifact_ref→links,
//!   embed→embeds; rel_class=reference) — the X-2 uniform producer mapping, structured not regex.
//! - **Causality correct-by-construction:** every emitted edge carries the content event's
//!   correlation root, `causation_id = content_event`, and `depth = content.depth + 1` (the loop-guard
//!   stamp the AG-6 guard reads; the explicit drill is REF-P9).
//! - **Emit-iff-committed (REF-D7 producer half, BUS-D4):** N nodes committed → N rows durable +
//!   relay-visible; the SAME transaction ABORTED (dropped without commit) → 0 rows (no edge without
//!   its content).

use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs_service::{emit_edges, extract_edges, EdgeRel};

use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-7".into()),
        PrincipalKind::Human,
        tenant(),
    )
}
fn source_doc() -> ArtifactRef {
    ArtifactRef("myelin://acme/chat/message/m1".into())
}

/// The content event being written in the SAME transaction the edges co-commit into (the CAUSE).
/// A root-ish `chat.message.created` at `depth = 3` so the edge's `depth + 1 = 4` is observable.
fn content_event(depth: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-content".into()),
        type_: EventType("chat.message.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: source_doc(),
        aggregate: AggregateKey("chat:message:m1".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-root-corr".into()),
        caused_by: Some(CausedBy("session:abc".into())),
        depth,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "body_ref": "r1" }),
    }
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn store_and_minter() -> (OutboxStore, Arc<dyn IdMinter>) {
    (
        OutboxStore::new(),
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>,
    )
}

/// A three-node document: one of each structured inline node. → three edges.
fn three_node_doc() -> Vec<InlineNode> {
    let target = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
    vec![
        InlineNode::Mention(principal()),
        InlineNode::ArtifactRefNode(target.clone()),
        InlineNode::Embed(target),
    ]
}

/// **CDC 5.4 emit side + causality: N structured nodes → N `refs.edge.created`, each caused by the
/// content event (root carries, causation = content, depth+1).** Committed → durable.
#[test]
fn n_nodes_emit_n_edges_caused_by_content_and_committed() {
    let (store, minter) = store_and_minter();
    let content = content_event(3);
    let doc = three_node_doc();

    let mut tx = store.begin(minter, ctx_base());
    // The content row co-commits in this SAME transaction (modelled by a staged state change).
    tx.stage_state_change("chat message m1 written");
    let ids: Vec<EventId> = emit_edges(&mut tx, &source_doc(), &doc, &content).expect("emit ok");

    assert_eq!(ids.len(), 3, "three structured nodes → three edge events");
    // before commit: nothing durable (emit-iff-committed).
    assert_eq!(
        store.outbox_depth(),
        0,
        "an open transaction has written nothing yet"
    );

    tx.commit().expect("commit ok");
    assert_eq!(
        store.outbox_depth(),
        3,
        "three edge rows durable after commit"
    );

    // Every emitted edge: refs.edge.created, reference-class triple, caused by the content event.
    let rels: Vec<String> = ids
        .iter()
        .map(|id| {
            let row = store.row(id).expect("committed edge row present");
            let env = &row.envelope;
            assert_eq!(env.type_, EventType("refs.edge.created".into()));
            assert_eq!(env.payload["rel_class"], "reference");
            assert_eq!(env.payload["source"], source_doc().0);
            // causality correct-by-construction: the content event's root + causation + depth+1.
            assert_eq!(
                env.correlation_id, content.correlation_id,
                "the correlation ROOT carries from the content event"
            );
            assert_eq!(
                env.causation_id.as_ref(),
                Some(&content.event_id),
                "causation = the content event"
            );
            assert_eq!(
                env.depth,
                content.depth + 1,
                "the loop-guard +1 depth stamp (drill REF-P9)"
            );
            env.payload["rel"].as_str().unwrap().to_string()
        })
        .collect();
    // the three rels, in document order: mentions / links / embeds (the X-2 uniform producer).
    assert_eq!(rels, vec!["mentions", "links", "embeds"]);
}

/// **Emit-iff-committed (REF-D7 producer half, BUS-D4): the SAME content transaction ABORTED writes
/// ZERO edges.** No edge without its content — the buffered edge rows drop with the transaction.
#[test]
fn aborted_content_transaction_emits_zero_edges() {
    let (store, minter) = store_and_minter();
    let content = content_event(0);
    let doc = three_node_doc();

    {
        let mut tx = store.begin(minter, ctx_base());
        tx.stage_state_change("chat message m1 written");
        let ids = emit_edges(&mut tx, &source_doc(), &doc, &content).expect("emit ok");
        assert_eq!(
            ids.len(),
            3,
            "three edges buffered into the open transaction"
        );
        // DROP tx without commit (the abort / crash between state-write and publish).
    }
    // the content tx aborted → 0 edge rows, 0 ghosts (emit-iff-committed; no edge without its content).
    assert_eq!(store.outbox_depth(), 0, "aborted content tx → 0 edges");
    assert_eq!(store.committed_count(), 0);
}

/// **A document with NO structured ref nodes emits ZERO edges (the no-op case), committed.** A
/// plain-prose message produces no reference edges — structured extraction never false-positives.
#[test]
fn empty_document_emits_zero_edges() {
    let (store, minter) = store_and_minter();
    let content = content_event(0);

    let mut tx = store.begin(minter, ctx_base());
    let ids = emit_edges(&mut tx, &source_doc(), &[], &content).expect("emit ok");
    assert!(ids.is_empty(), "no structured nodes → no edge events");
    tx.commit().expect("commit ok");
    assert_eq!(store.outbox_depth(), 0, "an empty document writes no edges");
}

/// **Extraction is structured-node-driven (the reliability guarantee, EI-04 §2.4): the mention
/// target is the PSEUDONYMOUS `member` URN, never the name.** The unit-level mapping, asserted at the
/// CDC layer too so the contract pair pins the erasure-safe target shape.
#[test]
fn mention_target_is_the_pseudonymous_member_urn() {
    let doc = vec![InlineNode::Mention(principal())];
    let edges = extract_edges(&source_doc(), &doc);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].rel, EdgeRel::Mentions);
    assert_eq!(
        edges[0].target.0, "myelin://acme/identity/member/p-opaque-7",
        "mention → the pseudonymous member URN (erasure-safe), never the name"
    );
}
