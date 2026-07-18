//! **MR-009b W3b.4 — the durable composition root PROVEN through a spec-built app against the
//! live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-issues --features integration \
//!     --test integration_w3b4_durable_spec_boot -- --nocapture
//!
//! This is the W3b.4 exit proof the design contract names: an emit through a SPEC-BUILT app (the
//! `issues_app_spec(config, outbox)` injection seam + the harness `boot` lifecycle) lands in the
//! REAL PG `outbox` table (verified with an independent raw SELECT, not the store's own reads),
//! while the service lifecycle cannot claim or stamp the shared row. The separately elected cell
//! publisher owns publication; this proof prevents a service-local relay from stealing another
//! subsystem's row into a process-private bus. The id source is the PRODUCTION `UlidMinter` (the P-S12
//! stand-in), satisfying the W3b.3 named condition — a per-run-unique `event_id`, never the
//! per-instance-resetting default `MonotonicMinter` whose collisions the durable
//! `ON CONFLICT (event_id) DO NOTHING` path silently drops.
//!
//! The multi-thread test flavor is REQUIRED: the sync `DurableOutboxBacking` verbs bridge to
//! async sqlx via `block_in_place` + `block_on`, which panics on a current-thread runtime — this
//! test also pins that the harness lifecycle verbs (`boot`/`tick`) are drivable on the same
//! multi-thread runtime shape the rewired service mains use (`#[tokio::main]`).
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::{Mode, MyelinConfig};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxStore, OutboxTx, Region, TenantId, Timestamp, UlidMinter, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::issues_app_spec;
use myelin_storage::{PgOutboxBacking, SubstrateProvider};
use myelin_substrate::{boot, Config};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    let principal = Principal::stub(
        PrincipalId("p:w3b4".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("fr-par".into()),
        actor: Actor(principal),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-15T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-15T00:00:01Z".into()),
        caused_by: None,
    }
}

/// The shared-outbox producer proof: the spec-built app commits to PG, while its lifecycle leaves
/// publication to the elected cell relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spec_built_app_emit_lands_in_pg_without_claiming_the_shared_relay() {
    // The SAME composition-root shape the rewired service mains use (W3b.4): provider from env →
    // foundation migrations → OutboxStore::durable(PgOutboxBacking).
    let mut migration_config = MyelinConfig::from_env(Mode::DevDefaults).expect("dev config");
    migration_config.database_url = admin_url();
    let migrator = SubstrateProvider::connect(migration_config.clone(), 2)
        .await
        .expect("connect the migration-owner pool (is the dev stack up?)");
    migrator
        .migrate_foundation()
        .await
        .expect("apply the foundation migrations (outbox/consumer_dedup)");

    // Runtime operations use the constrained NOBYPASSRLS app role. Production still needs one
    // platform-wide way to supply this credential split; the test models the required boundary.
    migration_config.database_url = app_url();
    let provider = SubstrateProvider::connect(migration_config, 4)
        .await
        .expect("connect the RLS-enforced runtime pool");
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        tokio::runtime::Handle::current(),
    )));

    // The SPEC-BUILT app owns the durable producer store but deliberately has no embedded relay.
    let handle = boot(issues_app_spec(Config::default(), outbox.clone())).expect("boot");

    // Emit through the spec's outbox with the PRODUCTION UlidMinter (the W3b.3 named condition:
    // a unique id source, never the resetting default MonotonicMinter). The aggregate is
    // per-run-unique so re-runs against the shared dev DB never contend on (aggregate, seq).
    let minter: Arc<dyn IdMinter> = Arc::new(UlidMinter::new());
    let run_tag = format!(
        "w3b4-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let aggregate = format!("issue:{run_tag}");
    let mut tx = handle.outbox().begin(Arc::clone(&minter), ctx_base("acme"));
    tx.stage_state_change("w3b4 durable-root proof");
    let event_id = tx
        .emit(
            EventDraft {
                type_: EventType("issues.issue.created".into()),
                subject: ArtifactRef(format!("myelin://acme/issues/issue/{run_tag}")),
                aggregate: AggregateKey(aggregate.clone()),
                payload: serde_json::json!({ "ref": run_tag }),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            None,
        )
        .expect("emit");
    tx.commit().expect("durable co-commit");

    // INDEPENDENT PG proof (not the store's own reads): the committed row is in the REAL outbox
    // table, unsent (published_at IS NULL), under the per-run aggregate at seq 0.
    let (n, published): (i64, Option<bool>) = {
        let row: (i64,) =
            sqlx::query_as("SELECT count(*) FROM outbox WHERE event_id = $1 AND aggregate = $2")
                .bind(&event_id.0)
                .bind(&aggregate)
                .fetch_one(provider.db_pool())
                .await
                .expect("select committed row");
        let sent: Option<(bool,)> =
            sqlx::query_as("SELECT published_at IS NOT NULL FROM outbox WHERE event_id = $1")
                .bind(&event_id.0)
                .fetch_optional(provider.db_pool())
                .await
                .expect("select published flag");
        (row.0, sent.map(|s| s.0))
    };
    assert_eq!(n, 1, "the emit landed in the PG outbox exactly once");
    assert_eq!(
        published,
        Some(false),
        "a freshly committed row is unsent (published_at IS NULL)"
    );

    // A service tick must not claim the global table. The elected cell publisher runs separately.
    handle.tick();

    let sent_after: (bool,) =
        sqlx::query_as("SELECT published_at IS NOT NULL FROM outbox WHERE event_id = $1")
            .bind(&event_id.0)
            .fetch_one(provider.db_pool())
            .await
            .expect("select published flag after drain");
    assert!(
        !sent_after.0,
        "the producer lifecycle must leave publication to the elected cell relay"
    );

    // The durable store's own read agrees (the dispatching read path — no memory arm involved).
    let row = handle
        .outbox()
        .row(&event_id)
        .expect("the durable store re-reads the row from PG");
    assert!(
        row.published_at.is_none(),
        "store read confirms the shared row was not claimed locally"
    );

    // Graceful drain still works over the durable arm (stop intake, finish in-flight, ack-exit).
    handle.signal_drain();
}
