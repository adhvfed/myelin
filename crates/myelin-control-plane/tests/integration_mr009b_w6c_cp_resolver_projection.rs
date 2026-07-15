//! # MR-009b W6c-cp — the cross-cell resolver registry PROJECTS from the durable `cell` table,
//! proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-control-plane --features integration \
//!       --test integration_mr009b_w6c_cp_resolver_projection -- --nocapture
//!
//! It proves the W6c-cp deliverable — the production `CellResolverRegistry` is a boot-time PROJECTION
//! of the durable `cell` table (each cell's PII-free routing `endpoint`), NOT an in-memory
//! system-of-record. Each assertion MUST hit the live DB (a pass on the in-memory double would NOT
//! count):
//!   1. **Project from live rows:** insert cells with endpoints → project → the bridge's `resolve`
//!      dispatches to the projected handle for the pointer's `home_cell` (the handle is the RIGHT
//!      cell's — it carries that cell's durable endpoint).
//!   2. **Fail LOUD, never a silent empty registry:** a cell with an empty endpoint → the projection
//!      REFUSES (`ProjectionError::MissingEndpoint`); a factory that cannot construct a handle →
//!      `ProjectionError::Unresolvable`.
//!   3. **Durable-by-authority:** the projection re-run over a FRESH pool (new connections) reconstructs
//!      the SAME working registry — its authority is the durable rows, not process state.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_control_plane::cross_cell_bridge::{
    BridgeMode, BridgeProjection, BridgeResolution, BridgeTombstoneReason, CellLocalResolver, ViewerId,
};
use myelin_control_plane::{CellResolverRegistry, CrossCellBridge, ProjectionError};
use myelin_storage::migration::HotTables;
use myelin_storage::placement_durable::{
    placement_durable_migrations, DurableCellRow, DurablePlacementBacking,
};
use myelin_storage::SubstrateProvider;
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

/// DDL + DML run as the admin role (control-plane tables carry NO RLS — cross-tenant routing infra).
fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn admin_provider() -> Option<SubstrateProvider> {
    let cfg = admin_config(&MyelinConfig::dev());
    let provider = match SubstrateProvider::connect(cfg, 6).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    provider
        .migrate(&placement_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the placement migrations (cell table + endpoint column)");
    Some(provider)
}

fn cell_row(cell_id: &str, region: &str, endpoint: &str) -> DurableCellRow {
    DurableCellRow {
        cell_id: cell_id.into(),
        region: region.into(),
        status: "Active".into(),
        isolation_kind: "Pool".into(),
        tenants_max: 1000,
        write_qps_max: 5000,
        storage_bytes_max: 1 << 40,
        utilisation: 10,
        version: 1,
        endpoint: endpoint.into(),
    }
}

/// A live resolver handle built by the factory FROM a cell's durable endpoint — it echoes the endpoint
/// back in the projection so a test can prove the handle dispatched to is the RIGHT cell's (the one
/// whose durable row carried that endpoint). A denied viewer gets a tombstone (no leak).
struct EndpointEchoResolver {
    endpoint: String,
}
impl CellLocalResolver for EndpointEchoResolver {
    fn resolve_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        _mode: BridgeMode,
    ) -> BridgeResolution {
        if viewer.as_str() == "denied" {
            return BridgeResolution::Tombstone(myelin_control_plane::BridgeTombstone {
                subject: pointer.subject().clone(),
                reason: BridgeTombstoneReason::Denied,
            });
        }
        BridgeResolution::Projection(BridgeProjection {
            subject: pointer.subject().clone(),
            title: self.endpoint.clone(),
            state: "open".into(),
            icon: "issue".into(),
        })
    }
}

fn pointer(subject: &str, home: &str) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
        ArtifactType::Issue,
        CorrelationId("01J0CORR".into()),
        CellId::from_token(home),
    )
}

/// The resolver FACTORY: durable endpoint → a live handle. Rejects the sentinel `"unresolvable"`
/// endpoint (the loud `Unresolvable` path); accepts any other non-empty endpoint.
fn factory(
) -> Box<dyn Fn(&CellId, &str) -> Result<Arc<dyn CellLocalResolver>, String> + Send + Sync> {
    Box::new(|_cell: &CellId, endpoint: &str| {
        if endpoint == "unresolvable" {
            return Err("transport client refused the endpoint".into());
        }
        Ok(Arc::new(EndpointEchoResolver {
            endpoint: endpoint.to_string(),
        }) as Arc<dyn CellLocalResolver>)
    })
}

/// Remove throwaway `cell` rows this test created (the `cell` table is unscoped routing infra).
async fn cleanup(pool: &sqlx::PgPool, cell_ids: &[String]) {
    for id in cell_ids {
        let _ = sqlx::query("DELETE FROM cell WHERE cell_id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolver_registry_projects_from_the_durable_cell_table() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let pool = provider.db_pool().clone();
    let suffix = uniq();
    let cell_b = format!("cell-b-{suffix}");
    let cell_c = format!("cell-c-{suffix}");
    let ep_b = format!("cell-b.eu-west.{suffix}.myelin.eu");
    let ep_c = format!("cell-c.eu-west.{suffix}.myelin.eu");

    let backing = DurablePlacementBacking::new(pool.clone());
    backing
        .insert_cell(&cell_row(&cell_b, "eu-west", &ep_b))
        .await
        .expect("insert cell-b with a durable endpoint");
    backing
        .insert_cell(&cell_row(&cell_c, "eu-west", &ep_c))
        .await
        .expect("insert cell-c with a durable endpoint");

    // (1) PROJECT the registry from the durable cell rows, then resolve through the bridge.
    let f = factory();
    let reg = CellResolverRegistry::project_from_durable_cells(&backing, f.as_ref())
        .await
        .expect("the projection is built from the durable cell endpoints");
    let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

    let res = bridge.resolve(
        &pointer("myelin://01J0BETA/issues/issue/7", &cell_b),
        &ViewerId::from_token("viewer-1"),
        BridgeMode::Live,
    );
    let BridgeResolution::Projection(proj) = res else {
        panic!("an authorised viewer resolves to a projection through the projected registry");
    };
    // The dispatched handle is the RIGHT cell's — it carries cell-b's DURABLE endpoint (proof the
    // projection mapped `home_cell` → the handle built from that cell's row, not some other cell's).
    assert_eq!(proj.title, ep_b, "resolved through cell-b's projected handle");
    assert_eq!(bridge.cross_cell_raw_rows(), 0, "CP-D8 zero holds on the projected arm");

    // An unknown home cell (not in the durable projection) degrades to a Gone tombstone.
    let ghost = bridge.resolve(
        &pointer("myelin://01J0GHOST/issues/issue/1", "cell-unknown"),
        &ViewerId::from_token("viewer-1"),
        BridgeMode::Live,
    );
    assert_eq!(ghost.tombstone_reason(), Some(BridgeTombstoneReason::Gone));

    // (2) Durable-by-authority: RE-PROJECT over a FRESH pool (new connections) — the registry
    // reconstructs identically because its authority is the durable rows, not process state.
    let fresh = SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2)
        .await
        .expect("a fresh pool connects");
    let fresh_backing = DurablePlacementBacking::new(fresh.db_pool().clone());
    let reg2 = CellResolverRegistry::project_from_durable_cells(&fresh_backing, f.as_ref())
        .await
        .expect("the projection reconstructs over a fresh pool");
    let bridge2 = CrossCellBridge::new(CellId::from_token("cell-a"), reg2);
    let BridgeResolution::Projection(proj2) = bridge2.resolve(
        &pointer("myelin://01J0BETA/issues/issue/9", &cell_c),
        &ViewerId::from_token("viewer-1"),
        BridgeMode::Live,
    ) else {
        panic!("the fresh-pool projection resolves cell-c");
    };
    assert_eq!(proj2.title, ep_c, "the fresh-pool projection is authoritative from the durable rows");

    // (3a) FAIL LOUD on a missing endpoint — never a silent empty registry.
    let bad_ep = format!("cell-bad-{suffix}");
    backing
        .insert_cell(&cell_row(&bad_ep, "eu-west", ""))
        .await
        .expect("insert a cell with an EMPTY endpoint");
    let err = CellResolverRegistry::project_from_durable_cells(&backing, f.as_ref())
        .await
        .expect_err("an empty endpoint makes the projection REFUSE (fail loud)");
    assert!(
        matches!(err, ProjectionError::MissingEndpoint { .. }),
        "empty endpoint → MissingEndpoint, got {err}"
    );
    cleanup(&pool, &[bad_ep]).await;

    // (3b) FAIL LOUD on an unresolvable endpoint (the factory refuses it).
    let unres = format!("cell-unres-{suffix}");
    backing
        .insert_cell(&cell_row(&unres, "eu-west", "unresolvable"))
        .await
        .expect("insert a cell whose endpoint the factory rejects");
    let err2 = CellResolverRegistry::project_from_durable_cells(&backing, f.as_ref())
        .await
        .expect_err("an unresolvable endpoint makes the projection REFUSE (fail loud)");
    assert!(
        matches!(err2, ProjectionError::Unresolvable { .. }),
        "unresolvable endpoint → Unresolvable, got {err2}"
    );
    cleanup(&pool, &[unres]).await;

    cleanup(&pool, &[cell_b, cell_c]).await;
}
