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

    let agg = "myelin://acme/knowledge/page/p1";
    assert_eq!(store.row(&b1).unwrap().aggregate.0, agg);
    assert_eq!(store.row(&b2).unwrap().aggregate.0, agg);
    assert_eq!(store.row(&b1).unwrap().seq, 0);
    assert_eq!(
        store.row(&b2).unwrap().seq,
        1,
        "the second block of p1 is seq 1 (per-doc ordering)"
    );
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

#[test]
fn kn_d7_crash_between_commit_and_publish_zero_ghost_zero_lost() {
    let store = OutboxStore::new();
    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus, || {
        Timestamp("2026-06-21T00:00:02Z".into())
    });

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
        "still 3 unsent - the crash lost NOTHING (durable in the outbox)"
    );

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
