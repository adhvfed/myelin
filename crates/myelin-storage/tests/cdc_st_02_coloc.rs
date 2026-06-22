//! P-ST-02 (global P-016) CDC pair — the co-located outbox emit (2.2) + a relay consumer (2.3).
//!
//! The prompt requires "the provider+consumer pair for 2.2/2.3 (the outbox emit + a relay
//! consumer)". The PROVIDER is `myelin-storage` — `ColocatedOltp`, the OLTP store that
//! co-locates its outbox, on which a subsystem stages a domain-state write and emits an event
//! in ONE transaction (`OutboxTx::emit`, 2.2; the outbox table, 2.3). The CONSUMER is the
//! `myelin-events` relay draining that same co-located outbox to a broker and a downstream
//! consumer reading the delivered envelope.
//!
//! This pins the frozen call shape the cross-seam cursor (§7.3) depends on: a co-located emit
//! becomes durable iff the OLTP transaction commits, and the relay then delivers exactly that
//! committed row. If the co-location surface drifts (the begin/stage/emit/commit shape, or the
//! reuse of the one `OUTBOX_MIGRATION` table), this stops compiling/passing.

use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, MonotonicMinter, Region, Relay, TenantId, Timestamp, Visibility,
};
use myelin_events::{BusTransport, InProcessBus};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{ColocatedOltp, OltpConfig};

/// A subsystem (the CDC consumer of the co-location surface): it owns its `issue` table and a
/// co-located outbox, opens ONE `ColocatedOltp` for its service DB, and writes state + emits an
/// event in one transaction. The `no-cross-db` boundary holds — it uses the co-location GUARD,
/// it does not reach another subsystem's tables.
struct IssuesService {
    db: ColocatedOltp,
}

impl IssuesService {
    fn boot() -> IssuesService {
        let config = OltpConfig {
            max_pool_size: 16,
            statement_timeout_ms: 3_000,
            per_tenant_in_flight_cap: 4,
        };
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let db = ColocatedOltp::open(config, minter).expect("co-located OLTP store opens");
        // The service's migration set co-locates the outbox table in the same DB (proven below).
        let migrations = ColocatedOltp::migrations(&["CREATE TABLE issue (id TEXT PRIMARY KEY);"]);
        assert!(
            migrations.iter().any(|m| m.contains("outbox")),
            "the outbox table must be co-located in the service DB migration set"
        );
        IssuesService { db }
    }

    fn ctx(&self, tenant: &str) -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId(tenant.into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("u1".into()),
                PrincipalKind::Human,
                TenantId(tenant.into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:cdc".into())),
        }
    }

    /// Create an issue: stage the state write AND emit `issues.issue.created` in ONE co-located
    /// transaction — the provider honours the 2.2/2.3 co-commit shape.
    fn create_issue(&self, tenant: &str, key: &str) {
        let mut tx = self
            .db
            .begin(self.ctx(tenant))
            .expect("a connection is available");
        tx.stage_state(format!("INSERT issue {key}"));
        tx.emit(
            EventDraft {
                type_: EventType("issues.issue.created".into()),
                subject: ArtifactRef(format!("myelin://{tenant}/issues/issue/{key}")),
                aggregate: AggregateKey(format!("issue:{key}")),
                payload: serde_json::json!({ "ref": key }),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .expect("emit on the co-located tx");
        tx.commit()
            .expect("the co-located tx commits state + outbox atomically");
    }
}

/// THE CDC pair: the provider (`myelin-storage` `ColocatedOltp`) emits into the co-located
/// outbox in the same tx as the state write; the consumer (the `myelin-events` relay) drains
/// that outbox to a broker, and a downstream reader receives exactly the committed event.
#[test]
fn cdc_st_02_coloc_emit_then_relay_consume() {
    let issues = IssuesService::boot();
    issues.create_issue("acme", "PROJ-1");

    // The co-committed event is durable + unsent (the cross-seam cursor row).
    assert_eq!(
        issues.db.outbox_depth(),
        1,
        "the co-committed event is parked in the outbox"
    );

    // The consumer side: a relay drains the SAME co-located outbox to an in-process broker.
    let bus = InProcessBus::new();
    let relay = Relay::new(issues.db.outbox().clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:02Z".into())
    });
    let report = relay.drain_to_empty();

    // Exactly one event delivered (no ghost, no loss), and the outbox drained to 0.
    assert_eq!(
        report.published, 1,
        "exactly the one committed event is delivered"
    );
    assert_eq!(
        bus.delivered_count(),
        1,
        "the broker received exactly one event"
    );
    assert_eq!(
        issues.db.outbox_depth(),
        0,
        "outbox_depth → 0 after the relay drains"
    );

    // The downstream consumer reads the delivered envelope — the round-trip the cursor depends on.
    let delivered = bus.consume("myelin://acme/issues");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].type_.0, "issues.issue.created");
}
