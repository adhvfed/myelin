//! # Chained e2e — Git PR/comment bodies on `myelin-content` + the content-node → `refs.edge.created`
//! emission (GIT-P17 / P-278, M3-G3; EI-01 §4 — chain the mutation end-to-end)
//!
//! **Contracts exercised:** 5.4 (`refs.edge.created` from the mention/artifact_ref/embed content nodes,
//! emitted via the outbox — no standalone edge-write API) + 13.1 (the `myelin-content` markdown-subset,
//! `render(parse(md)) === md`).
//!
//! This is the prompt's required chained e2e: **add an inline comment with a mention + a `Closes`
//! reference + an embed → assert exactly the right edges + the round-trip parity** — driven through the
//! REAL same-transaction outbox ([`myelin_events::OutboxStore`]), so the emit-iff-committed property
//! (no edge without its committed body) is proven end-to-end, not just at the pure-function seam.

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
        caused_by: Some(CausedBy("session:comment-1".into())),
    }
}

fn alice() -> Principal {
    Principal::stub(PrincipalId("p-alice".into()), PrincipalKind::Human, TenantId("acme".into()))
}

/// The inline comment body the e2e authors: a `@`-mention + an inline `Closes` artifact-ref + an embed,
/// each a STRUCTURED node (NOT a regex over prose). The markdown-subset string carries one `OBJ` per
/// node (positional binding). The body round-trips `render(parse(md)) === md`.
fn comment_body() -> Body {
    // "@alice this OBJ closes OBJ and see OBJ" — three structured nodes at three OBJ positions.
    let md = format!("{OBJ} this closes {OBJ} and see {OBJ}");
    let nodes = vec![
        // @alice — a mention (→ mentions).
        InlineNode::Mention(alice()),
        // an inline reference to issue ENG-1 (the structured "Closes" link → links). The trailer-driven
        // TYPED lifecycle edge is GIT-P19's distinct producer; THIS is the content-node reference edge.
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issue/issue/ENG-1".into())),
        // an embed of a knowledge page (→ embeds).
        InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into())),
    ];
    Body::new(md, nodes)
}

/// The body's `git.comment.created` content event — the CAUSE the edge emits carry (the body content
/// event + its edges co-commit in the SAME transaction). Constructed directly (the comment write holds
/// this envelope in hand at emit time); `depth = 2` so the edge's `depth + 1 = 3` is observable.
fn content_event(source: &ArtifactRef) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J-comment".into()),
        type_: EventType(GIT_COMMENT_CREATED.into()),
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
        correlation_id: CorrelationId("01J-comment-corr".into()),
        caused_by: Some(CausedBy("session:comment-1".into())),
        depth: 2,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "comment_ref": source.0 }),
    }
}

/// **THE CHAIN: a comment body with a mention + a Closes ref + an embed → EXACTLY 3 edges + round-trip
/// parity, all committed in ONE transaction.**
#[test]
fn inline_comment_with_mention_closes_embed_emits_exactly_three_edges() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();
    let body = comment_body();

    // 1. the round-trip parity leg (contract 13.1): render(parse(md)) === md byte-identical.
    assert!(body.round_trips(), "the comment body must round-trip render(parse(md)) === md");

    // 2. open ONE outbox transaction; the body row co-commits (staged state change) with its edges.
    let content_event = content_event(&source);
    let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
    tx.stage_state_change("git comment cAbc body written");
    let edge_ids = emit_body_edges(&mut tx, &source, body.structured_nodes(), &content_event)
        .expect("the body edges emit");

    // exactly 3 edges (1 per structured node; 0 dup, 0 missed).
    assert_eq!(edge_ids.len(), 3, "3 structured nodes → exactly 3 refs.edge.created");

    // before commit nothing is durable (emit-iff-committed).
    assert_eq!(outbox.committed_count(), 0, "nothing durable before commit");
    tx.commit().expect("the body + edges co-commit");

    // 3. the 3 edges are durable (the body row co-committed; the edges are the outbox rows).
    assert_eq!(outbox.committed_count(), 3, "the 3 edges co-committed with the body row");

    // 4. assert the EXACT edge set the three nodes produced (mention→mentions, ref→links, embed→embeds),
    //    each a `refs.edge.created` with the references-not-payloads triple + the shared edge aggregate.
    let mut seen_rels: Vec<String> = Vec::new();
    for id in &edge_ids {
        let env = outbox.row(id).expect("the edge row is durable").envelope;
        assert_eq!(env.type_.0, "refs.edge.created", "each edge is a refs.edge.created");
        assert_eq!(env.payload["rel_class"], "reference", "content-node edges are reference-class");
        assert_eq!(env.payload["source"], source.0, "the source is the comment body URN");
        // the edge inherits the content event's correlation root (causality correct-by-construction).
        assert_eq!(
            env.correlation_id, content_event.correlation_id,
            "the edge carries the content event's correlation root"
        );
        assert_eq!(
            env.causation_id.as_ref().map(|c| &c.0),
            Some(&content_event.event_id.0),
            "the edge's causation is the content event"
        );
        assert_eq!(env.depth, content_event.depth + 1, "the edge is depth+1 (the loop-guard stamp)");
        seen_rels.push(env.payload["rel"].as_str().unwrap().to_string());
    }
    seen_rels.sort();
    assert_eq!(
        seen_rels,
        vec!["embeds".to_string(), "links".to_string(), "mentions".to_string()],
        "exactly one mentions + one links + one embeds edge"
    );

    // 5. the mention's target is the PSEUDONYMOUS member URN, never the name (erasure-safe).
    let mention_edge = edge_ids
        .iter()
        .map(|id| outbox.row(id).unwrap().envelope)
        .find(|e| e.payload["rel"] == "mentions")
        .expect("a mentions edge");
    assert_eq!(
        mention_edge.payload["target"], "myelin://acme/identity/member/p-alice",
        "the mention target is the pseudonymous member URN"
    );
}

/// **Emit-iff-committed: an ABORTED body write produces ZERO edges (no edge without its committed
/// body).** The chained mutation is dropped with the transaction — the silent-data-loss floor (GIT-D9
/// class) holds for content edges.
#[test]
fn aborted_body_write_emits_zero_edges() {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();
    let body = comment_body();

    {
        let content_event = content_event(&source);
        let mut tx = outbox.begin(Arc::clone(&minter), ctx_base());
        tx.stage_state_change("git comment cAbc body written");
        let ids = emit_body_edges(&mut tx, &source, body.structured_nodes(), &content_event).unwrap();
        assert_eq!(ids.len(), 3, "3 edges were buffered");
        // DROP the transaction without committing (the abort).
    }
    assert_eq!(outbox.committed_count(), 0, "an aborted body write commits 0 rows (no ghost edges)");
}

/// **A plain-prose comment (a literal `Closes ENG-1` + a literal `@alice` in text) produces ZERO content
/// edges** — extraction is a structured-node walk, never a regex over prose. The trailer-driven typed
/// edge is GIT-P19's distinct producer.
#[test]
fn plain_prose_comment_produces_no_content_edges() {
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();
    let body = Body::new("Closes ENG-1, cc @alice — see the design doc.", vec![]);
    assert!(body.round_trips());
    assert!(
        extract_body_edges(&source, body.structured_nodes()).is_empty(),
        "prose mentions/closes are NOT content edges (only structured nodes are)"
    );
}

/// Each structured node kind maps to its frozen rel (the X-2 uniform producer), asserted at the
/// extraction seam for the chained corpus.
#[test]
fn each_structured_node_maps_to_its_frozen_rel() {
    let source = mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap();
    let edges = extract_body_edges(&source, comment_body().structured_nodes());
    assert_eq!(edges[0].rel, EdgeRel::Mentions);
    assert_eq!(edges[1].rel, EdgeRel::Links);
    assert_eq!(edges[2].rel, EdgeRel::Embeds);
}
