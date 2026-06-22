//! # CDC + drill: the Knowledge transactional outbox + `knowledge.*` taxonomy (KN-P06 → P-296, M3)
//!
//! **Contracts proven here (the CDC pairs the prompt's TESTS field names):**
//! - **2.2** `OutboxTx::emit` — every Knowledge state change emits via the ONE sanctioned emit verb;
//!   emit-iff-committed (the row is buffered into the open transaction, durable iff it commits).
//! - **2.3** the `outbox` table + `UNIQUE(aggregate, seq)` — the aggregate is the doc/row/db (§4),
//!   so per-doc events stay per-aggregate ordered and different docs fan out in parallel.
//! - **2.4** the `EventHandler` consumer template — the living-doc consumer's `subjects()` whitelist
//!   is `*`-free (rule 3) and is idempotent through the runtime (rule 1, the `consumer_dedup` ledger).
//! - **2.9** the complete `knowledge.*` token list — every `KnowledgeChange` maps to a frozen,
//!   grammatical, `knowledge.`-prefixed token admitted by the one Bus harness.
//!
//! **GATE / DRILL — KN-D7 (0 ghost / 0 lost):** crash the Knowledge service between the block/row
//! commit and the relay-publish → the event is STILL delivered (the outbox survived) and is NEVER
//! delivered without the state change. The drill writes through the REAL [`OutboxStore`] +
//! [`emit_change`], severs the broker mid-relay, recovers, and asserts the outbox-depth telemetry
//! returns to baseline (0) with EXACTLY-ONCE delivery (0 ghost, 0 lost). The live-Postgres half is
//! `tests/integration_kn_d7_outbox.rs` (the `integration` feature).
//!
//! **no-raw-publish lint (CI):** the Knowledge green/red fixtures are admitted/rejected by the one
//! central `no-raw-publish` scanner (the live workspace gate scans `myelin-knowledge/src` directly;
//! `emit.rs` only ever calls `tx.emit(..)`, so the Knowledge crate is structurally green).
//!
//! **CDC pair markers (the contract-coverage gate).** This file is BOTH sides of the seam:
//! - **PROVIDER side** — Knowledge is the PROVIDER of the `knowledge.*` events: [`emit_change`]
//!   stamps the frozen token + the `(aggregate, subject)` pair, and the live-Postgres integration
//!   test proves the provider's outbox co-commit (the provider emits iff committed).
//! - **CONSUMER side** — Knowledge is also a CONSUMER: the living-doc [`KnowledgeLivingDocHandler`]
//!   binds a `*`-free whitelist and the Bus harness (the CONSUMER of the token list) admits KN's
//!   complete registered list. So this file carries both the provider assertion and the consumer
//!   assertion the CDC-pair gate requires.

use myelin_content::events::{
    register_knowledge_tokens, KNOWLEDGE_DURABLE_TOKENS, KNOWLEDGE_FIREHOSE_TOKENS,
};
use myelin_events::{
    consume, validate_event_type, Actor, ArtifactRef, CausedBy, ConsumerName, ConsumerSpec,
    CorrelationId, DataRole, DedupLedger, Delivered, EmitContextBase, EventEnvelope, EventId,
    EventType, IdMinter, InProcessBus, Message, MonotonicMinter, OutboxStore, Region, Relay,
    SubsystemTokenList, TenantId, Timestamp, TokenListHarness, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::{
    emit_change, KnowledgeChange, KnowledgeLivingDocHandler, LIVING_DOC_CONSUMER,
};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn principal() -> Principal {
    Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>
}

// =================================================================================================
// 2.9 — the complete knowledge.* taxonomy registers + parses (the OWNER list, arch 03 §1)
// =================================================================================================

/// **CDC 2.9 — Knowledge's COMPLETE `knowledge.*` list is admitted by the one Bus harness.** The
/// full architecture-03 §1 owner taxonomy (page/block/database/view/row/comment/mention lifecycle +
/// access/DSR + the cross-cutting `*.erased`/`*.snapshot`) parses the §6.1 grammar and is admitted
/// in full — every name `knowledge.`-prefixed + grammatical + unique.
#[test]
fn cdc_2_9_complete_knowledge_taxonomy_is_admitted() {
    assert!(
        register_knowledge_tokens().is_ok(),
        "KN's complete list parses the §6.1 grammar"
    );

    let mut harness = TokenListHarness::new();
    let all: Vec<&str> = KNOWLEDGE_DURABLE_TOKENS
        .iter()
        .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        .copied()
        .collect();
    let admitted = harness
        .register(&SubsystemTokenList::references_only("knowledge", &all))
        .expect("KN's complete list is admitted by the Bus harness");
    assert_eq!(admitted, all.len(), "the WHOLE list registers");

    // The full arch-03 §1 lifecycle is present (the new KN-P06 owner additions, named, not literals).
    for tok in [
        "knowledge.page.published",
        "knowledge.page.deleted",
        "knowledge.block.created",
        "knowledge.database.schema_changed",
        "knowledge.view.created",
        "knowledge.row.moved",
        "knowledge.comment.created",
        "knowledge.mention.created",
        "knowledge.access.granted",
        "knowledge.subject.erasure_requested",
        "knowledge.row.snapshot",
    ] {
        assert!(
            harness.is_registered(tok),
            "the owner taxonomy must include `{tok}`"
        );
    }
}

/// **Every `KnowledgeChange` the emit seam can produce maps to a grammatical, registered token.** A
/// change can NOT reach the bus except through a named token that is in the registered owner list —
/// the names anchor (X-5): no ad-hoc string at a write call site.
#[test]
fn cdc_2_9_every_change_maps_to_a_registered_token() {
    for ch in representative_changes() {
        let t = ch.event_type();
        assert!(validate_event_type(t).is_ok(), "`{t}` is ungrammatical");
        assert!(
            KNOWLEDGE_DURABLE_TOKENS.contains(&t),
            "`{t}` (from {ch:?}) must be in the registered durable owner list"
        );
    }
}

// =================================================================================================
// 2.2 / 2.3 — OutboxTx::emit, emit-iff-committed, the aggregate is the doc/row/db
// =================================================================================================

/// **CDC 2.2 — a Knowledge state change emits via `OutboxTx::emit` and co-commits with the state.**
/// The block write + its `knowledge.block.updated` event become durable together on commit; an OPEN
/// transaction has written nothing (the row is buffered — emit-iff-committed).
#[test]
fn cdc_2_2_emit_change_co_commits_with_the_state() {
    let store = OutboxStore::new();
    let mut tx = store.begin(minter(), ctx_base());
    tx.stage_state_change("block b9 of page 7c2 written (version 5)");
    let change = KnowledgeChange::BlockUpdated {
        page_id: "7c2".into(),
        block_id: "9".into(),
    };
    let id = emit_change(&mut tx, &tenant(), &change, None).expect("emit");

    assert_eq!(
        store.outbox_depth(),
        0,
        "the OPEN transaction has written nothing (buffered)"
    );
    tx.commit().expect("the block write + its event co-commit");
    assert_eq!(
        store.outbox_depth(),
        1,
        "after commit: exactly the one knowledge event is durable"
    );
    assert_eq!(
        store.row(&id).unwrap().envelope.type_.0,
        "knowledge.block.updated"
    );
}

/// **CDC 2.3 — the aggregate is the doc/row/db (the per-aggregate `seq` ordering key).** Two block
/// changes on the SAME page share the page aggregate and get monotonic seqs `0, 1`; a row change on
/// a database aggregates on the database. Per-doc ordering holds; different docs fan out in parallel.
#[test]
fn cdc_2_3_aggregate_is_the_doc_or_db_with_monotonic_seq() {
    let store = OutboxStore::new();
    let mut tx = store.begin(minter(), ctx_base());
    let b1 = emit_change(
        &mut tx,
        &tenant(),
        &KnowledgeChange::BlockCreated {
            page_id: "p1".into(),
            block_id: "b1".into(),
        },
        None,
    )
    .unwrap();
    let b2 = emit_change(
        &mut tx,
        &tenant(),
        &KnowledgeChange::BlockUpdated {
            page_id: "p1".into(),
            block_id: "b2".into(),
        },
        None,
    )
    .unwrap();
    let row = emit_change(
        &mut tx,
        &tenant(),
        &KnowledgeChange::RowUpdated {
            db_id: "tasks".into(),
            row_id: "r1".into(),
        },
        None,
    )
    .unwrap();
    tx.commit().unwrap();

    // The two blocks of page p1 share the page aggregate → seqs 0, 1 (per-doc ordering).
    let agg = "myelin://acme/knowledge/page/p1";
    assert_eq!(store.row(&b1).unwrap().aggregate.0, agg);
    assert_eq!(store.row(&b2).unwrap().aggregate.0, agg);
    assert_eq!(store.row(&b1).unwrap().seq, 0);
    assert_eq!(
        store.row(&b2).unwrap().seq,
        1,
        "the second block of p1 is seq 1 (per-doc ordering)"
    );
    // The row aggregates on its database (independent counter, starts at 0).
    assert_eq!(
        store.row(&row).unwrap().aggregate.0,
        "myelin://acme/knowledge/database/tasks"
    );
    assert_eq!(
        store.row(&row).unwrap().seq,
        0,
        "a different aggregate has its own seq counter"
    );
}

// =================================================================================================
// 2.4 — the EventHandler consumer template (whitelisted, idempotent)
// =================================================================================================

/// **CDC 2.4 — the living-doc consumer binds a `*`-free whitelist and is idempotent (rules 1+3+4).**
/// It registers through the sanctioned `consume` (rejecting `*`), binds the durable name, and a
/// redelivered event is deduped (the handler runs exactly once).
#[test]
fn cdc_2_4_living_doc_consumer_is_whitelisted_and_idempotent() {
    let spec = ConsumerSpec::new(
        ConsumerName(LIVING_DOC_CONSUMER.into()),
        &["myelin://acme/issues/"],
    );
    let consumer = consume(spec, KnowledgeLivingDocHandler::new(), DedupLedger::new())
        .expect("the *-free whitelist binds (rule 3)");
    assert_eq!(
        consumer.name(),
        &ConsumerName(LIVING_DOC_CONSUMER.into()),
        "bound by durable name (rule 4)"
    );

    let msg = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: trigger("issue.issue.updated"),
    };
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "first delivery runs + acks"
    );
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Deduplicated,
        "redelivery deduped (0 dup, rule 1)"
    );
    assert_eq!(
        consumer.handler().observed(),
        1,
        "the handler ran EXACTLY once"
    );
}

/// **CDC 2.4 — a `*` (or empty) subject is REJECTED at registration (rule 3, head-of-line guard).**
#[test]
fn cdc_2_4_wildcard_consumer_is_rejected() {
    for bad in [&["*"][..], &["knowledge.>"][..], &[""][..]] {
        let spec = ConsumerSpec::new(ConsumerName(LIVING_DOC_CONSUMER.into()), bad);
        assert!(
            consume(spec, KnowledgeLivingDocHandler::new(), DedupLedger::new()).is_err(),
            "an over-broad subscription {bad:?} must be rejected (rule 3)"
        );
    }
}

// =================================================================================================
// KN-D7 — crash between commit and relay-publish → 0 ghost, 0 lost (the GATE drill)
// =================================================================================================

/// **KN-D7 (0 ghost / 0 lost) — crash the service between the block commit and the relay-publish;
/// the event is still delivered exactly once and never without its state.** The scenario:
///
/// 1. Write three block changes through the REAL outbox (the state + the events co-commit).
/// 2. The broker is SEVERED before the relay can publish (the "crash mid-relay" point) — the rows
///    are durable in the outbox but undelivered; outbox-depth is at its written height (3).
/// 3. Recover: heal the broker, drain to empty. Every committed event is delivered EXACTLY ONCE
///    (0 ghost — the broker dedups on the stable ULID even across a re-claim), and NONE is lost
///    (0 lost — the relay re-claims the unsent rows). The outbox-depth telemetry returns to baseline
///    (0) and the dead-letter count is 0.
#[test]
fn kn_d7_crash_between_commit_and_publish_zero_ghost_zero_lost() {
    let store = OutboxStore::new();
    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus, || {
        Timestamp("2026-06-21T00:00:02Z".into())
    });

    // (1) Three Knowledge state changes co-commit with their events.
    let changes = [
        KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "1".into(),
        },
        KnowledgeChange::BlockUpdated {
            page_id: "7c2".into(),
            block_id: "2".into(),
        },
        KnowledgeChange::RowUpdated {
            db_id: "tasks".into(),
            row_id: "r1".into(),
        },
    ];
    let mut ids = Vec::new();
    let m = minter();
    for ch in &changes {
        let mut tx = store.begin(Arc::clone(&m), ctx_base());
        tx.stage_state_change(format!("state for {ch:?}"));
        ids.push(emit_change(&mut tx, &tenant(), ch, None).expect("emit"));
        tx.commit().expect("the state + event co-commit");
    }
    assert_eq!(
        store.outbox_depth(),
        3,
        "all three committed events are durable + unsent"
    );

    // (2) CRASH mid-relay: the broker is unreachable when the relay tries to publish (the kill
    // point between the commit and the publish). The drain fails every publish; the rows stay
    // claimable (0 lost so far — they are durable, just undelivered).
    relay.transport().sever();
    let crashed = relay.drain_once();
    assert_eq!(
        crashed.published, 0,
        "the broker is severed → nothing published (the crash window)"
    );
    assert_eq!(
        crashed.failed, 3,
        "the rows failed to publish, but stay claimable (not lost)"
    );
    assert_eq!(
        store.outbox_depth(),
        3,
        "still 3 unsent — the crash lost NOTHING (durable in the outbox)"
    );

    // (3) RECOVER: heal the broker + drain to empty. Exactly-once delivery, depth → baseline.
    relay.transport().heal();
    let recovered = relay.drain_to_empty();
    assert_eq!(
        recovered.published, 3,
        "recovery delivers every committed event (0 lost)"
    );
    assert_eq!(
        store.outbox_depth(),
        0,
        "the outbox-depth telemetry returns to baseline (0)"
    );
    assert_eq!(
        store.dead_letter_count(),
        0,
        "0 dead-letters on the no-loss path"
    );
    assert_eq!(
        relay.transport().delivered_count(),
        3,
        "exactly 3 distinct events delivered (0 ghost)"
    );

    // 0 ghost, made precise: the delivered set is exactly the three committed event_ids (none extra).
    let delivered = relay.transport().delivered_ids();
    for id in &ids {
        assert!(
            delivered.contains(id),
            "committed event {id:?} was delivered (0 lost)"
        );
    }
    assert_eq!(
        delivered.len(),
        3,
        "no ghost: exactly the committed events were delivered, no more"
    );
}

/// **KN-D7 (no ghost from an ABORT) — an aborted block transaction publishes nothing.** A crash
/// BEFORE commit (the state change never committed) leaves no outbox row — so the relay can never
/// deliver an event whose block/row did not commit (0 ghost). The complement of the loss case above.
#[test]
fn kn_d7_aborted_transaction_yields_no_ghost() {
    let store = OutboxStore::new();
    {
        let mut tx = store.begin(minter(), ctx_base());
        tx.stage_state_change("block written but never committed");
        emit_change(
            &mut tx,
            &tenant(),
            &KnowledgeChange::BlockUpdated {
                page_id: "7c2".into(),
                block_id: "9".into(),
            },
            None,
        )
        .expect("emit");
        // tx dropped here WITHOUT commit (the crash before the state commit).
    }
    assert_eq!(
        store.outbox_depth(),
        0,
        "an aborted transaction wrote NO event (0 ghost)"
    );
    assert_eq!(
        store.committed_count(),
        0,
        "no committed state without its event, none with a ghost"
    );
}

// ---- helpers ----

fn representative_changes() -> Vec<KnowledgeChange> {
    vec![
        KnowledgeChange::PageCreated {
            page_id: "p".into(),
        },
        KnowledgeChange::PagePublished {
            page_id: "p".into(),
        },
        KnowledgeChange::BlockCreated {
            page_id: "p".into(),
            block_id: "b".into(),
        },
        KnowledgeChange::DatabaseSchemaChanged { db_id: "d".into() },
        KnowledgeChange::ViewUpdated {
            db_id: "d".into(),
            view_id: "v".into(),
        },
        KnowledgeChange::RowMoved {
            db_id: "d".into(),
            row_id: "r".into(),
        },
        KnowledgeChange::CommentCreated {
            page_id: "p".into(),
            comment_id: "c".into(),
        },
        KnowledgeChange::MentionCreated {
            page_id: "p".into(),
            comment_id: "c".into(),
        },
        KnowledgeChange::AccessGranted {
            page_id: "p".into(),
        },
        KnowledgeChange::SubjectErasureRequested {
            page_id: "p".into(),
        },
    ]
}

fn trigger(type_: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("01J-{type_}")),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: myelin_events::AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J-corr".into()),
        caused_by: Some(CausedBy("session:abc".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}
