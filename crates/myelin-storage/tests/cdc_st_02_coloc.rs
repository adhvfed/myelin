use std::sync::Arc;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, MonotonicMinter, OutboxStore, Region, Relay, TenantId, Timestamp, Visibility,
};
use myelin_events::{BusTransport, InProcessBus};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{ColocatedOltp, OltpConfig};

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
        let db = ColocatedOltp::open(config, OutboxStore::new(), minter)
            .expect("co-located OLTP store opens");
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

#[test]
fn cdc_st_02_coloc_emit_then_relay_consume() {
    let issues = IssuesService::boot();
    issues.create_issue("acme", "PROJ-1");

    assert_eq!(
        issues.db.outbox_depth(),
        1,
        "the co-committed event is parked in the outbox"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(issues.db.outbox().clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:02Z".into())
    });
    let report = relay.drain_to_empty();

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

    let delivered = bus.consume("myelin://acme/issues");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].type_.0, "issues.issue.created");
}
