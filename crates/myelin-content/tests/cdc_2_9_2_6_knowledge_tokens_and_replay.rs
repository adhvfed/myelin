use myelin_content::events::{
    register_knowledge_tokens, KNOWLEDGE_DURABLE_TOKENS, KNOWLEDGE_FIREHOSE_TOKENS,
};
use myelin_content::replay::KnowledgeReindexSource;
use myelin_events::{
    reindex, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope, HarnessError,
    OutboxStore, Region, ReindexSource, SnapshotDraft, SnapshotScope, SubsystemTokenList, TenantId,
    Timestamp, TokenListHarness,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

#[test]
fn cdc_2_9_knowledge_complete_list_admitted_by_the_bus_harness() {
    assert!(
        register_knowledge_tokens().is_ok(),
        "KN's list parses the §6.1 grammar"
    );

    let mut harness = TokenListHarness::new();
    let all: Vec<&str> = KNOWLEDGE_DURABLE_TOKENS
        .iter()
        .chain(KNOWLEDGE_FIREHOSE_TOKENS)
        .copied()
        .collect();
    let admitted = harness
        .register(&SubsystemTokenList::references_only("knowledge", &all))
        .expect("KN's complete list is admitted");
    assert_eq!(admitted, all.len());
    assert!(harness.is_registered("knowledge.block.updated"));
    assert!(harness.is_registered("knowledge.page.snapshot"));
}

#[test]
fn cdc_2_9_harness_rejects_a_malformed_knowledge_addition() {
    let mut harness = TokenListHarness::new();
    harness
        .register(&SubsystemTokenList::references_only(
            "knowledge",
            KNOWLEDGE_DURABLE_TOKENS,
        ))
        .unwrap();
    assert!(matches!(
        harness.add(
            "knowledge",
            myelin_events::RegisteredToken::references_only("knowledge.pages.created")
        ),
        Err(HarnessError::UngrammaticalToken { .. })
    ));
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
        caused_by: None,
    }
}

fn snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: draft.event_id(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(draft.event_id().0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

#[test]
fn cdc_2_6_knowledge_replay_rebuilds_cold_equals_live_block_granular() {
    let mut source = KnowledgeReindexSource::new();
    source.upsert_page(
        "home",
        4,
        &[
            (
                "b1",
                1,
                serde_json::json!({ "kind": "heading", "text_ref": "r1" }),
            ),
            (
                "b2",
                3,
                serde_json::json!({ "kind": "paragraph", "text_ref": "r2" }),
            ),
        ],
    );
    let scope = SnapshotScope::new("knowledge", "page:home");

    let mut live = DerivedStore::new();
    for draft in source.replay(&scope, None) {
        live.ingest(&snapshot_envelope(&draft));
    }

    let mut cold = DerivedStore::new();
    let sources: &[&dyn ReindexSource] = &[&source];
    let mut outbox = OutboxStore::new();
    let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    assert_eq!(
        r1.snapshots_emitted, 3,
        "1 page + 2 blocks (block granularity)"
    );
    for draft in source.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row present");
        cold.ingest(&row.envelope);
    }
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "cold == live (byte-identical)"
    );

    let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
    assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 new (idempotent)");
    assert_eq!(r2.snapshots_skipped_duplicate, 3);
}
