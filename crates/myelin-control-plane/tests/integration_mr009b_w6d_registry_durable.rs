//! # MR-009b W6d — the WHOLE-SURFACE durable placement registry + the durable misroute audit,
//! proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin \
//!     cargo test -p myelin-control-plane --features integration \
//!       --test integration_mr009b_w6d_registry_durable -- --nocapture
//!
//! It proves the W6d deliverables — each MUST hit the live DB (a pass on the in-memory double would
//! NOT count):
//!   1. **All FIVE registry surfaces survive fresh-pool reconstruction (kill-9 equivalent):** cells,
//!      tenant placements, repo placements (the NOT-rebuildable stored facts), the provisioning log,
//!      and the local-tenant directory — written through `Registry::with_pg` on one pool, read back
//!      by a FRESH `Registry::with_pg` over a FRESH pool.
//!   2. **The repo-grain residency derivation cannot drift:** `repo_placement` stores NO region (it
//!      derives from the tenant placement at read time) and the new DB TRIGGER refuses an
//!      adversarial DIRECT INSERT/UPDATE that homes a repo on a cell outside its tenant's region —
//!      even bypassing all Rust app logic. The typed `relocate_repo` cross-region reject holds on
//!      the Pg arm too.
//!   3. **The provisioning log is append-only + append-ordered, durably:** entries land in
//!      registration order and survive a fresh pool (the backing exposes INSERT as its only verb).
//!   4. **A gateway-rejected misroute lands in the DURABLE audit sink** (`MisrouteAudit::with_pg`
//!      wired via `CellGateway::with_audit`) and survives a fresh pool — SI-028 closed at the
//!      gateway, not just at the MR-024 backing.
//!   5. **The self-host root boots on the Pg arm** (`DegenerateControlPlane::with_pg`) and its
//!      routing (discover / placement_of / gateway accept) SURVIVES a restart — a fresh boot over a
//!      fresh pool re-registers the one cell idempotently and still routes the placed tenant.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_control_plane::place::{CounterMinter, PlacementService};
use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellProvisioning, CellStatus, DegenerateControlPlane,
    GatewayReject, IsolationKind, LocalTenant, MisrouteAudit, PlacementStatus,
    ProvisioningOutcome, Registry, RepoPlacementError, StorageGroup, TenantPlacement,
};
use myelin_storage::migration::HotTables;
use myelin_storage::placement_durable::{
    placement_durable_migrations, DurableMisrouteAuditBacking, DurablePlacementBacking,
};
use myelin_storage::SubstrateProvider;
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

/// DDL (CREATE TABLE/FUNCTION/TRIGGER) runs as the migration/owner (admin) role — PG16 revokes
/// CREATE on `public` for the app role. The control-plane tables carry NO RLS (cross-tenant routing
/// infra), so DML is role-agnostic; this test drives both DDL + DML through the admin pool.
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

/// Build an admin-role provider, applying the FULL placement migrations (0030–0039 — including the
/// W6d repo_placement/cell_provisioning/local_tenant tables + the repo residency trigger); `None`
/// (SKIP) if unreachable.
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
        .expect("apply the placement migrations 0030–0039 (W6d whole surface + triggers)");
    Some(provider)
}

async fn fresh_pool() -> sqlx::PgPool {
    SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2)
        .await
        .expect("fresh pool")
        .db_pool()
        .clone()
}

fn cell(id: &str, region: &str) -> Cell {
    Cell {
        cell_id: CellId::from_token(id),
        region: Region::new(region),
        status: CellStatus::Active,
        isolation_kind: IsolationKind::Pool,
        capacity: Capacity {
            tenants_max: 1000,
            write_qps_max: 5000,
            storage_bytes_max: 1 << 40,
        },
        utilisation: 10,
        version: 1,
        endpoint: format!("cell.{region}.{id}.myelin.eu"),
    }
}

fn placement(tenant: &str, region: &str, home: &str, slug: &str) -> TenantPlacement {
    TenantPlacement {
        tenant_id: TenantId::from_token(tenant),
        region: Region::new(region),
        home_cell: CellId::from_token(home),
        isolation_tier: IsolationKind::Pool,
        slug: slug.into(),
        status: PlacementStatus::Active,
        member_cells: vec![CellId::from_token(home)],
    }
}

/// Delete every row this test's unique suffix created (shared dev DB hygiene).
async fn cleanup(pool: &sqlx::PgPool, suffix: &str) {
    let like = format!("%{suffix}%");
    for q in [
        "DELETE FROM repo_placement WHERE repo_ref LIKE $1 OR tenant_id LIKE $1",
        "DELETE FROM local_tenant WHERE cell_id LIKE $1 OR tenant_id LIKE $1",
        "DELETE FROM cell_provisioning WHERE cell_id LIKE $1",
        "DELETE FROM misroute_audit WHERE tenant_id LIKE $1 OR received_by_cell LIKE $1",
        "DELETE FROM tenant_placement WHERE tenant_id LIKE $1 OR home_cell LIKE $1",
        "DELETE FROM cell WHERE cell_id LIKE $1",
    ] {
        let _ = sqlx::query(q).bind(&like).execute(pool).await;
    }
}

// =================================================================================================
// 1 — All FIVE surfaces survive a fresh Registry over a fresh pool (the kill-9 equivalent).
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_five_registry_surfaces_survive_a_fresh_pool() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let west = format!("cellw{suffix}");
    let tenant = format!("01J0ACME{suffix}");
    let repo = ArtifactRef(format!("myelin://{tenant}/git/repo/web"));

    // ONE Registry instance writes all five surfaces (the same sync API the whole CP crate calls).
    {
        let mut reg = Registry::with_pg(
            DurablePlacementBacking::new(provider.db_pool().clone()),
            tokio::runtime::Handle::current(),
        );
        // (1) cell inventory.
        assert!(reg.insert_cell(cell(&west, "eu-west")).is_none(), "fresh cell");
        // (2) tenant placement, through the HARD invariant (the DB trigger admits it).
        reg.place_tenant(placement(&tenant, "eu-west", &west, &format!("acme-{suffix}")))
            .expect("a single-region placement is admitted");
        // (3) repo placement (the NOT-rebuildable stored fact) — via the SAME register_repo the
        //     git wire uses; the repo's region derives from the tenant placement.
        reg.register_repo(&repo, StorageGroup::from_token("pack-0"))
            .expect("a repo on its tenant's home cell is registered");
        // (4) provisioning log (append-only).
        reg.log_provisioning(CellProvisioning {
            cell_id: CellId::from_token(&west),
            step: "restore_verify".into(),
            outcome: ProvisioningOutcome::Passed,
        });
        // (5) local-tenant directory.
        assert!(reg
            .upsert_local_tenant(
                &CellId::from_token(&west),
                LocalTenant {
                    tenant_id: TenantId::from_token(&tenant),
                    isolation_tier: IsolationKind::Pool,
                    active: true,
                },
            )
            .is_none());
    } // the first instance is DROPPED — nothing in-process survives.

    // A FRESH Registry over a FRESH pool (new connections — the kill-9 equivalent): every surface
    // reads back. A pass here CANNOT come from process memory.
    let reg2 = Registry::with_pg(
        DurablePlacementBacking::new(fresh_pool().await),
        tokio::runtime::Handle::current(),
    );
    // (1) cell.
    let c = reg2
        .cell(&CellId::from_token(&west))
        .expect("the cell row SURVIVED (durable)");
    assert_eq!(c.region.as_str(), "eu-west");
    assert_eq!(c.status, CellStatus::Active);
    // (2) placement (the frozen routing tuple).
    let p = reg2
        .placement_of(&TenantId::from_token(&tenant))
        .expect("the tenant→cell routing SURVIVED");
    assert_eq!(p.home_cell.as_str(), west);
    assert_eq!(p.region.as_str(), "eu-west");
    // (3) repo placement — cell is the stored fact; region DERIVES from the tenant placement.
    let rp = reg2
        .placement_of_repo(&repo)
        .expect("the repo placement SURVIVED (the NOT-rebuildable stored fact)");
    assert_eq!(rp.cell_id.as_str(), west);
    assert_eq!(rp.group.as_str(), "pack-0");
    assert_eq!(rp.region.as_str(), "eu-west", "region derived from the tenant placement");
    // (4) provisioning log (filtered to this test's cell — the log is shared infra).
    let log: Vec<_> = reg2
        .provisioning_log()
        .into_iter()
        .filter(|e| e.cell_id.as_str() == west)
        .collect();
    assert_eq!(log.len(), 1, "the provisioning entry SURVIVED");
    assert_eq!(log[0].step, "restore_verify");
    assert_eq!(log[0].outcome, ProvisioningOutcome::Passed);
    // (5) local-tenant directory.
    let dir = reg2.local_tenants(&CellId::from_token(&west));
    assert_eq!(dir.len(), 1, "the local-tenant directory SURVIVED");
    assert_eq!(dir[0].tenant_id.as_str(), tenant);
    assert!(dir[0].active);

    cleanup(provider.db_pool(), &suffix).await;
    println!("OK [1]: all five registry surfaces survived a fresh Registry over a fresh pool.");
}

// =================================================================================================
// 2 — The repo-grain residency derivation cannot drift (adversarial direct SQL is REFUSED).
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repo_placement_region_derivation_cannot_drift() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let west = format!("cellw{suffix}");
    let north = format!("celln{suffix}");
    let tenant = format!("01J0PIN{suffix}");
    let repo = ArtifactRef(format!("myelin://{tenant}/git/repo/web"));
    let pool = provider.db_pool();

    let mut reg = Registry::with_pg(
        DurablePlacementBacking::new(pool.clone()),
        tokio::runtime::Handle::current(),
    );
    reg.insert_cell(cell(&west, "eu-west"));
    reg.insert_cell(cell(&north, "eu-north")); // the WRONG-region cell the pin must refuse.
    reg.place_tenant(placement(&tenant, "eu-west", &west, &format!("pin-{suffix}")))
        .expect("placed in eu-west");
    reg.register_repo(&repo, StorageGroup::from_token("pack-0"))
        .expect("registered on the home cell");

    // (a) ADVERSARIAL: a DIRECT raw INSERT (bypassing ALL Rust app logic) homing a repo on a cell
    //     in a DIFFERENT region than its tenant is REJECTED BY THE DATABASE — the W6d trigger.
    let direct = sqlx::query(
        "INSERT INTO repo_placement (repo_ref, tenant_id, cell_id, storage_group) \
         VALUES ($1, $2, $3, 'evil-pack') \
         ON CONFLICT (repo_ref) DO UPDATE SET cell_id = EXCLUDED.cell_id",
    )
    .bind(format!("myelin://{tenant}/git/repo/evil"))
    .bind(&tenant)
    .bind(&north)
    .execute(pool)
    .await;
    let err = direct.expect_err("the DB TRIGGER must reject a cross-region repo home on a DIRECT insert");
    let dberr = err.as_database_error().expect("a database error");
    assert_eq!(dberr.code().as_deref(), Some("23514"), "SQLSTATE check_violation");
    assert!(
        err.to_string().contains("residency pin holds at repo grain"),
        "the trigger's rejection is loud + named: {err}"
    );

    // (b) ADVERSARIAL: a DIRECT raw UPDATE relocating the EXISTING repo row cross-region is refused
    //     too (BEFORE INSERT OR UPDATE — a stored fact cannot be drifted by SQL either).
    let update = sqlx::query("UPDATE repo_placement SET cell_id = $2 WHERE repo_ref = $1")
        .bind(&repo.0)
        .bind(&north)
        .execute(pool)
        .await;
    let uerr = update.expect_err("the trigger must reject a cross-region UPDATE of the stored fact");
    assert_eq!(
        uerr.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("23514")
    );

    // (c) An unplaced-tenant repo is refused fail-closed at the DB (no region of record).
    let ghost = sqlx::query(
        "INSERT INTO repo_placement (repo_ref, tenant_id, cell_id, storage_group) \
         VALUES ($1, $2, $3, 'g')",
    )
    .bind(format!("myelin://01J0GHOST{suffix}/git/repo/x"))
    .bind(format!("01J0GHOST{suffix}"))
    .bind(&west)
    .execute(pool)
    .await;
    let gerr = ghost.expect_err("an unplaced tenant's repo is refused fail-closed at the DB");
    assert!(gerr.to_string().contains("fail-closed"), "loud: {gerr}");

    // (d) The TYPED reject holds on the Pg arm: `relocate_repo` to a cross-region cell is refused
    //     by the app-level check (the same predicate) and the stored fact is UNCHANGED.
    let e = reg
        .relocate_repo(&repo, CellId::from_token(&north), StorageGroup::from_token("g"))
        .expect_err("a cross-region relocation target is rejected (the residency pin)");
    assert!(matches!(e, RepoPlacementError::Invariant(_)), "{e}");
    let still = reg.placement_of_repo(&repo).expect("still placed");
    assert_eq!(still.cell_id.as_str(), west, "the rejected relocation did not move the repo");
    assert_eq!(still.region.as_str(), "eu-west");

    // (e) A same-region relocation IS admitted (relocatable, never node-pinned) and derives the
    //     same region.
    let west2 = format!("cellw2{suffix}");
    reg.insert_cell(cell(&west2, "eu-west"));
    reg.relocate_repo(&repo, CellId::from_token(&west2), StorageGroup::from_token("pack-7"))
        .expect("a same-region relocation is admitted");
    let moved = reg.placement_of_repo(&repo).expect("placed");
    assert_eq!(moved.cell_id.as_str(), west2);
    assert_eq!(moved.region.as_str(), "eu-west", "region unchanged — derived, not stored");

    cleanup(pool, &suffix).await;
    println!("OK [2]: the repo residency pin holds at the DATABASE — direct INSERT/UPDATE drift refused (23514), typed reject intact.");
}

// =================================================================================================
// 3 — The provisioning log is append-only + append-ordered, durably.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_log_is_append_ordered_and_durable() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let cell_id = format!("cellp{suffix}");

    let mut reg = Registry::with_pg(
        DurablePlacementBacking::new(provider.db_pool().clone()),
        tokio::runtime::Handle::current(),
    );
    reg.insert_cell(cell(&cell_id, "eu-west"));
    for (step, outcome) in [
        ("restore_verify", ProvisioningOutcome::Passed),
        ("readiness_probe", ProvisioningOutcome::Running),
        ("readiness_probe", ProvisioningOutcome::Passed),
    ] {
        reg.log_provisioning(CellProvisioning {
            cell_id: CellId::from_token(&cell_id),
            step: step.into(),
            outcome,
        });
    }

    // A FRESH Registry over a FRESH pool reads the log back IN APPEND ORDER (id-ordered) — the
    // orchestration history survived the kill-9 equivalent. The API surface exposes NO update or
    // delete verb over the log (append-only by construction, mirrored at the backing).
    let reg2 = Registry::with_pg(
        DurablePlacementBacking::new(fresh_pool().await),
        tokio::runtime::Handle::current(),
    );
    let mine: Vec<_> = reg2
        .provisioning_log()
        .into_iter()
        .filter(|e| e.cell_id.as_str() == cell_id)
        .collect();
    assert_eq!(mine.len(), 3, "every appended entry survived");
    assert_eq!(
        mine.iter().map(|e| e.step.as_str()).collect::<Vec<_>>(),
        vec!["restore_verify", "readiness_probe", "readiness_probe"],
        "append order preserved"
    );
    assert_eq!(mine[1].outcome, ProvisioningOutcome::Running);
    assert_eq!(mine[2].outcome, ProvisioningOutcome::Passed);

    cleanup(provider.db_pool(), &suffix).await;
    println!("OK [3]: the provisioning log is append-ordered + durable across a fresh pool.");
}

// =================================================================================================
// 4 — A gateway-rejected misroute lands in the DURABLE audit sink (SI-028 at the gateway).
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_misroute_lands_in_the_durable_audit_sink() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let home = format!("cellh{suffix}");
    let wrong = format!("cellx{suffix}");
    let tenant = format!("01J0MIS{suffix}");
    let ghost = format!("01J0GHOST{suffix}");
    let pool = provider.db_pool();

    let mut reg = Registry::with_pg(
        DurablePlacementBacking::new(pool.clone()),
        tokio::runtime::Handle::current(),
    );
    reg.insert_cell(cell(&home, "eu-west"));
    reg.insert_cell(cell(&wrong, "eu-west"));
    reg.place_tenant(placement(&tenant, "eu-west", &home, &format!("mis-{suffix}")))
        .expect("placed on the home cell");

    // The WRONG cell's gateway, wired to the DURABLE audit sink (the W6d gateway re-point).
    let gw = CellGateway::with_audit(
        CellId::from_token(&wrong),
        MisrouteAudit::with_pg(
            DurableMisrouteAuditBacking::new(pool.clone()),
            tokio::runtime::Handle::current(),
        ),
    );
    // A misrouted tenant → REJECTED (not proxied) + REDIRECTED + AUDITED durably.
    let reject = gw
        .route(&reg, &TenantId::from_token(&tenant))
        .expect_err("the wrong cell rejects the misroute");
    assert!(matches!(reject, GatewayReject::Misroute(_)));
    // An unknown tenant → rejected + audited with NO redirect target.
    let no_route = gw
        .route(&reg, &TenantId::from_token(&ghost))
        .expect_err("an unknown tenant is rejected");
    assert!(matches!(no_route, GatewayReject::NoSuchTenant { .. }));
    assert_eq!(gw.misroute_count(), 2);
    assert_eq!(gw.cross_tenant_reads(), 0, "the CP-D2 zero holds on the durable-audit path");

    // The evidence SURVIVES a fresh backing over a fresh pool (durable — not the gateway's memory).
    let audit2 = DurableMisrouteAuditBacking::new(fresh_pool().await);
    let recs: Vec<_> = audit2
        .records()
        .await
        .expect("read the durable trail")
        .into_iter()
        .filter(|r| r.tenant_id == tenant || r.tenant_id == ghost)
        .collect();
    assert_eq!(recs.len(), 2, "both rejections landed in the durable sink via the gateway");
    assert_eq!(recs[0].tenant_id, tenant);
    assert_eq!(recs[0].received_by_cell, wrong);
    assert_eq!(recs[0].home_cell.as_deref(), Some(home.as_str()), "the redirect target survived");
    assert_eq!(recs[1].tenant_id, ghost);
    assert_eq!(recs[1].home_cell, None, "no redirect target for an unknown tenant");

    cleanup(pool, &suffix).await;
    println!("OK [4]: gateway-rejected misroutes land in the DURABLE audit sink and survive a fresh pool.");
}

// =================================================================================================
// 5 — The self-host root boots on the Pg arm; its routing survives a restart.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_host_boots_on_pg_and_routing_survives_restart() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let cell_id = format!("cellself{suffix}");
    let pool = provider.db_pool();

    // BOOT 1: the degenerate control plane over the DURABLE registry + audit sink (the production
    // self-host boot — fail-loud, non-empty registry asserted inside with_pg).
    let tenant = {
        let mut sh = DegenerateControlPlane::with_pg(
            CellId::from_token(&cell_id),
            Region::new("eu-west"),
            format!("cell.eu-west.{cell_id}.local"),
            DurablePlacementBacking::new(pool.clone()),
            DurableMisrouteAuditBacking::new(pool.clone()),
            tokio::runtime::Handle::current(),
        );
        assert_eq!(sh.cell().cell_id.as_str(), cell_id, "the one cell row is durable");
        // Place a tenant through the SAME PlacementService::place a fleet cell calls.
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, &format!("team-{suffix}"))
            .expect("the one Active cell is eligible → placed");
        assert_eq!(answer.home_cell.as_str(), cell_id);
        answer.tenant_id
    }; // BOOT 1 is DROPPED — the kill-9 equivalent.

    // BOOT 2: a FRESH self-host boot over a FRESH pool with the SAME cell id (the restart). The
    // one-cell registration is an idempotent upsert (region immutable on the conflict path) and
    // every previously placed tenant STILL ROUTES.
    let sh2 = DegenerateControlPlane::with_pg(
        CellId::from_token(&cell_id),
        Region::new("eu-west"),
        format!("cell.eu-west.{cell_id}.local"),
        DurablePlacementBacking::new(fresh_pool().await),
        DurableMisrouteAuditBacking::new(fresh_pool().await),
        tokio::runtime::Handle::current(),
    );
    // discover → "this cell", off the durable rows.
    let discovered = sh2
        .discover_cell(&tenant)
        .expect("the placed tenant STILL discovers after the restart");
    assert_eq!(discovered.as_str(), cell_id, "discover returns 'this cell' after restart");
    // placement_of → the frozen routing tuple, off the durable row.
    let p = sh2
        .placement_of(&tenant)
        .expect("the placement survived the restart");
    assert_eq!(p.home_cell.as_str(), cell_id);
    assert_eq!(p.region.as_str(), "eu-west");
    // The gateway ACCEPTS the tenant it homes (layer 4 runs off the durable routing answer).
    let served = sh2
        .route(&tenant)
        .expect("the one cell serves its tenant after the restart");
    assert_eq!(served.home_cell.as_str(), cell_id);

    // ADVERSARIAL (W6d verifier finding, probe-proven pre-fix): a RE-BOOT of the SAME cell
    // claiming a DIFFERENT region must be REFUSED AT BOOT — pre-fix it booted silently (the
    // durable row correctly kept eu-west, but self.region carried the wrong claim and the
    // endpoint was overwritten under the wrong-region config; the misconfig only surfaced at the
    // first place()). The region-claim read-back assert in with_pg must panic.
    let wrong_region_boot = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DegenerateControlPlane::with_pg(
            CellId::from_token(&cell_id),
            Region::new("de-fra"),
            format!("cell.de-fra.{cell_id}.local"),
            DurablePlacementBacking::new(pool.clone()),
            DurableMisrouteAuditBacking::new(pool.clone()),
            tokio::runtime::Handle::current(),
        )
    }));
    assert!(
        wrong_region_boot.is_err(),
        "a re-boot claiming de-fra over an eu-west durable cell row must REFUSE at boot \
         (region-claim read-back), never boot on a silently-divergent region"
    );

    // cleanup — repo rows first if any, then the tenant (the repo_placement FK now RESTRICTs a
    // tenant_placement delete while repos are placed; this test places none, but order is the
    // convention). (the CounterMinter-minted tenant id is not suffix-tagged — delete explicitly.)
    let _ = sqlx::query("DELETE FROM tenant_placement WHERE tenant_id = $1")
        .bind(tenant.as_str())
        .execute(pool)
        .await;
    cleanup(pool, &suffix).await;
    println!(
        "OK [5]: the self-host root boots on the Pg arm, routing survives a restart, and a \
         wrong-region re-boot is REFUSED at boot."
    );
}
