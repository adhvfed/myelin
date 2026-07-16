//! # CDC 10.6 — the tamper-evident audit log (the construction half, P-GA-19 → P-062)
//!
//! **Contract:** index row 10.6 (the tamper-evident audit log — written via the outbox only;
//! per-tenant hash-chain + Merkle leaves; minimised pseudonym actors). The construction half is
//! P-GA-19's deliverable; the inclusion/consistency PROOFS + the signed-tree-head + the
//! independent-witness anchoring are P-GA-20 / P-119 (they prove over THIS construction).
//!
//! The contract-coverage scanner (P-S21) reads BOTH halves of the pair from this file:
//! - **provider** = an action-taking service emitting an action event via the **outbox** (the
//!   one sanctioned emit path — no service writes the audit log directly; coverage is a bus
//!   property). Here a minimal `iam.tuple_written`-shaped emitter stands in for any
//!   action-taking subsystem;
//! - **consumer** = `myelin_gdpr_service::AuditConsumer`, the infra subscription on the outbox
//!   that appends each delivered action as one minimised, causality-carried, hash-chained,
//!   Merkle-leafed entry.
//!
//! The dated green artifact: an action emitted via the outbox + drained by the relay is delivered
//! to the audit consumer, which appends a minimised (`<pseudonym>@<tenant>.noreply`) entry with a
//! hash-chain link + a Merkle leaf + the carried causality — and the entry carries NONE of the
//! action's payload (references-not-payloads / minimisation).

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, BusTransport, DataRole, EmitContextBase, EventDraft,
    EventHandler, EventType, HandleOutcome, IdMinter, InProcessBus, MonotonicMinter, OutboxStore,
    OutboxTx, Relay, Timestamp, Visibility,
};
use myelin_gdpr_service::AuditConsumer;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;
use std::sync::Arc;

/// The dotted action token the provider emits (an action-bearing event — any human/agent action).
const IAM_TUPLE_WRITTEN: &str = "iam.tuple_written";

/// **The provider side (10.6): an action-taking service emits an action event via the outbox.**
/// This is the ONLY way an action reaches the audit log — there is no direct-write path. The
/// emit derives causality correct-by-construction (root here) and stages a co-committed state
/// change (the tuple write the action represents); on commit the row is durable + visible to the
/// relay. Returns the outbox the relay will drain.
fn provider_emits_action(actor: Principal, subject: &str) -> OutboxStore {
    let outbox = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let ctx_base = EmitContextBase {
        tenant: actor.tenant.clone(),
        region: actor.region.clone(),
        actor: Actor(actor),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: None,
    };
    let mut tx = outbox.begin(minter, ctx_base);
    // The action carries a NAME-shaped payload deliberately — the audit entry must never read it.
    let draft = EventDraft {
        type_: EventType(IAM_TUPLE_WRITTEN.into()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey("iam:acme".into()),
        payload: serde_json::json!({ "real_name": "Alice Example", "email": "alice@example.test" }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    tx.stage_state_change("tuple org:acme#member@p:alice written");
    tx.emit(draft, None)
        .expect("the action emits via the outbox");
    tx.commit().expect("the action + its state co-commit");
    outbox
}

/// **The 10.6 provider+consumer CDC pair.** The provider emits an action via the outbox; the relay
/// (what `serve` runs) publishes it; the audit CONSUMER appends it as one minimised, hash-chained,
/// Merkle-leafed entry carrying the action's causality — and NONE of its payload.
#[test]
fn cdc_10_6_provider_emits_via_outbox_consumer_appends_minimised_hash_chained_entry() {
    let actor = Principal::stub(
        PrincipalId("u-42".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );

    // PROVIDER: the action-taking service emits via the outbox (the one emit path).
    let outbox = provider_emits_action(actor, "myelin://acme/iam/tuple/t1");
    assert_eq!(
        outbox.outbox_depth(),
        1,
        "exactly one event for the one committed action"
    );

    // The relay publishes the committed event onto the bus (the audit consumer's subscription).
    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), || Timestamp("t".into()));
    relay.drain_to_empty();
    let published = bus.consume("");
    assert_eq!(
        published.len(),
        1,
        "the relay published exactly the one action event"
    );
    assert_eq!(published[0].type_.0, IAM_TUPLE_WRITTEN);

    // CONSUMER: the audit consumer (the outbox subscription) appends the delivered action.
    let audit = AuditConsumer::new();
    assert_eq!(
        audit.handle(&published[0], &mut myelin_events::HandlerTx::none()),
        HandleOutcome::Done,
        "the audit consumer appends + acks"
    );

    // The appended entry: minimised actor, a hash-chain link + a Merkle leaf, carried causality.
    let tenant = TenantId("acme".into());
    let entries = audit.log().entries_for(&tenant);
    assert_eq!(
        entries.len(),
        1,
        "one delivered action → one appended audit entry"
    );
    let e = &entries[0];

    // Minimised: the frozen `<pseudonym>@<tenant>.noreply` grammar over the PII-free principal_id.
    assert_eq!(
        e.actor.actor, "u-42@acme.noreply",
        "actor is the minimised pseudonym grammar (4.8)"
    );
    assert_eq!(e.actor.actor_kind, "human");
    // The action is the dotted type token; the subject is an ArtifactRef (an id), never content.
    assert_eq!(e.action, IAM_TUPLE_WRITTEN);
    assert_eq!(e.subject, ArtifactRef("myelin://acme/iam/tuple/t1".into()));
    // Hash-chain link + Merkle leaf both present (the construction the proofs prove over).
    assert!(
        e.prev_hash.starts_with("blake3:"),
        "hash-chain link present"
    );
    assert!(e.leaf_hash.starts_with("blake3:"), "Merkle leaf present");
    // Causality carried (the why-walk anchor): this action is its own root.
    assert_eq!(
        e.correlation_id, published[0].correlation_id.0,
        "correlation (root) carried verbatim"
    );
    assert_eq!(
        e.causation_id, None,
        "a root action has no immediate parent"
    );

    // Minimisation is structural: the action's NAME/email payload reaches the entry NOWHERE.
    let serialized = serde_json::to_string(e).expect("entry serialises");
    assert!(
        !serialized.contains("Alice Example"),
        "no real name reaches the audit entry"
    );
    assert!(
        !serialized.contains("alice@example.test"),
        "no email reaches the audit entry"
    );

    // The chain verifies intact (the tamper-evidence the construction guarantees).
    assert!(
        audit.log().verify_chain(&tenant),
        "the appended chain verifies intact"
    );
    // The per-tenant Merkle root exists (what the STH signs, P-GA-20).
    assert!(
        audit.log().root(&tenant).is_some(),
        "a per-tenant Merkle root exists (the STH input)"
    );

    // The audit_append_lag SLO reads green after the synchronous append.
    assert_eq!(
        audit.append_lag(),
        0,
        "audit_append_lag reads green (0) after the append"
    );
}
