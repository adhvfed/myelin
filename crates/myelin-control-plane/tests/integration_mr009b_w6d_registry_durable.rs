#![cfg(feature = "integration")]

mod common;

use myelin_control_plane::place::{CounterMinter, PlacementService};
use myelin_control_plane::{
    Capacity, Cell, CellGateway, CellProvisioning, CellStatus, DegenerateControlPlane,
    GatewayReject, IsolationKind, LocalTenant, MisrouteAudit, PlacementStatus, ProvisioningOutcome,
    Registry, RepoPlacementError, StorageGroup, TenantPlacement,
};
use myelin_storage::migration::HotTables;
use myelin_storage::placement_durable::{
    placement_durable_migrations, DurableMisrouteAuditBacking, DurablePlacementBacking,
};
use myelin_storage::SubstrateProvider;
use myelin_tenancy::{ArtifactRef, CellId, Region, TenantId};

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

async fn admin_provider() -> SubstrateProvider {
    let provider = SubstrateProvider::connect(common::admin_database_config(), 6)
        .await
        .expect("control-plane integration tests require the configured Postgres backend");
    provider
        .migrate(&placement_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the placement migrations 0030–0039 (W6d whole surface + triggers)");
    provider
}

async fn fresh_pool() -> sqlx::PgPool {
    SubstrateProvider::connect(common::admin_database_config(), 2)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_five_registry_surfaces_survive_a_fresh_pool() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let west = format!("cellw{suffix}");
    let tenant = format!("01J0ACME{suffix}");
    let repo = ArtifactRef(format!("myelin://{tenant}/git/repo/web"));

    common::with_cleanup(
        || async {
            {
                let mut reg = Registry::with_pg(
                    DurablePlacementBacking::new(provider.db_pool().clone()),
                    tokio::runtime::Handle::current(),
                );
                assert!(
                    reg.insert_cell(cell(&west, "eu-west")).is_none(),
                    "fresh cell"
                );
                reg.place_tenant(placement(
                    &tenant,
                    "eu-west",
                    &west,
                    &format!("acme-{suffix}"),
                ))
                .expect("a single-region placement is admitted");
                reg.register_repo(&repo, StorageGroup::from_token("pack-0"))
                    .expect("a repo on its tenant's home cell is registered");
                reg.log_provisioning(CellProvisioning {
                    cell_id: CellId::from_token(&west),
                    step: "restore_verify".into(),
                    outcome: ProvisioningOutcome::Passed,
                });
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
            }

            let reg2 = Registry::with_pg(
                DurablePlacementBacking::new(fresh_pool().await),
                tokio::runtime::Handle::current(),
            );
            let c = reg2
                .cell(&CellId::from_token(&west))
                .expect("the cell row SURVIVED (durable)");
            assert_eq!(c.region.as_str(), "eu-west");
            assert_eq!(c.status, CellStatus::Active);
            let p = reg2
                .placement_of(&TenantId::from_token(&tenant))
                .expect("the tenant→cell routing SURVIVED");
            assert_eq!(p.home_cell.as_str(), west);
            assert_eq!(p.region.as_str(), "eu-west");
            let rp = reg2
                .placement_of_repo(&repo)
                .expect("the repo placement SURVIVED (the NOT-rebuildable stored fact)");
            assert_eq!(rp.cell_id.as_str(), west);
            assert_eq!(rp.group.as_str(), "pack-0");
            assert_eq!(
                rp.region.as_str(),
                "eu-west",
                "region derived from the tenant placement"
            );
            let log: Vec<_> = reg2
                .provisioning_log()
                .into_iter()
                .filter(|e| e.cell_id.as_str() == west)
                .collect();
            assert_eq!(log.len(), 1, "the provisioning entry SURVIVED");
            assert_eq!(log[0].step, "restore_verify");
            assert_eq!(log[0].outcome, ProvisioningOutcome::Passed);
            let dir = reg2.local_tenants(&CellId::from_token(&west));
            assert_eq!(dir.len(), 1, "the local-tenant directory SURVIVED");
            assert_eq!(dir[0].tenant_id.as_str(), tenant);
            assert!(dir[0].active);
        },
        || async {
            cleanup(provider.db_pool(), &suffix).await;
        },
    )
    .await;
    println!("OK [1]: all five registry surfaces survived a fresh Registry over a fresh pool.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repo_placement_region_derivation_cannot_drift() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let west = format!("cellw{suffix}");
    let north = format!("celln{suffix}");
    let tenant = format!("01J0PIN{suffix}");
    let repo = ArtifactRef(format!("myelin://{tenant}/git/repo/web"));
    let pool = provider.db_pool();

    common::with_cleanup(
        || async {
            let mut reg = Registry::with_pg(
                DurablePlacementBacking::new(pool.clone()),
                tokio::runtime::Handle::current(),
            );
            reg.insert_cell(cell(&west, "eu-west"));
            reg.insert_cell(cell(&north, "eu-north"));
            reg.place_tenant(placement(
                &tenant,
                "eu-west",
                &west,
                &format!("pin-{suffix}"),
            ))
            .expect("placed in eu-west");
            reg.register_repo(&repo, StorageGroup::from_token("pack-0"))
                .expect("registered on the home cell");

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
            let err = direct.expect_err(
                "the DB TRIGGER must reject a cross-region repo home on a DIRECT insert",
            );
            let dberr = err.as_database_error().expect("a database error");
            assert_eq!(
                dberr.code().as_deref(),
                Some("23514"),
                "SQLSTATE check_violation"
            );
            assert!(
                err.to_string()
                    .contains("residency pin holds at repo grain"),
                "the trigger's rejection is loud + named: {err}"
            );

            let update = sqlx::query("UPDATE repo_placement SET cell_id = $2 WHERE repo_ref = $1")
                .bind(&repo.0)
                .bind(&north)
                .execute(pool)
                .await;
            let uerr = update
                .expect_err("the trigger must reject a cross-region UPDATE of the stored fact");
            assert_eq!(
                uerr.as_database_error().and_then(|d| d.code()).as_deref(),
                Some("23514")
            );

            let ghost = sqlx::query(
                "INSERT INTO repo_placement (repo_ref, tenant_id, cell_id, storage_group) \
                 VALUES ($1, $2, $3, 'g')",
            )
            .bind(format!("myelin://01J0GHOST{suffix}/git/repo/x"))
            .bind(format!("01J0GHOST{suffix}"))
            .bind(&west)
            .execute(pool)
            .await;
            let gerr =
                ghost.expect_err("an unplaced tenant's repo is refused fail-closed at the DB");
            assert!(gerr.to_string().contains("fail-closed"), "loud: {gerr}");

            let e = reg
                .relocate_repo(
                    &repo,
                    CellId::from_token(&north),
                    StorageGroup::from_token("g"),
                )
                .expect_err("a cross-region relocation target is rejected (the residency pin)");
            assert!(matches!(e, RepoPlacementError::Invariant(_)), "{e}");
            let still = reg.placement_of_repo(&repo).expect("still placed");
            assert_eq!(
                still.cell_id.as_str(),
                west,
                "the rejected relocation did not move the repo"
            );
            assert_eq!(still.region.as_str(), "eu-west");

            let west2 = format!("cellw2{suffix}");
            reg.insert_cell(cell(&west2, "eu-west"));
            reg.relocate_repo(
                &repo,
                CellId::from_token(&west2),
                StorageGroup::from_token("pack-7"),
            )
            .expect("a same-region relocation is admitted");
            let moved = reg.placement_of_repo(&repo).expect("placed");
            assert_eq!(moved.cell_id.as_str(), west2);
            assert_eq!(
                moved.region.as_str(),
                "eu-west",
                "region unchanged - derived, not stored"
            );
        },
        || async {
            cleanup(pool, &suffix).await;
        },
    )
    .await;
    println!("OK [2]: the repo residency pin holds at the DATABASE - direct INSERT/UPDATE drift refused (23514), typed reject intact.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_log_is_append_ordered_and_durable() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell_id = format!("cellp{suffix}");

    common::with_cleanup(
        || async {
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
        },
        || async {
            cleanup(provider.db_pool(), &suffix).await;
        },
    )
    .await;
    println!("OK [3]: the provisioning log is append-ordered + durable across a fresh pool.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_misroute_lands_in_the_durable_audit_sink() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let home = format!("cellh{suffix}");
    let wrong = format!("cellx{suffix}");
    let tenant = format!("01J0MIS{suffix}");
    let ghost = format!("01J0GHOST{suffix}");
    let pool = provider.db_pool();

    common::with_cleanup(
        || async {
            let mut reg = Registry::with_pg(
                DurablePlacementBacking::new(pool.clone()),
                tokio::runtime::Handle::current(),
            );
            reg.insert_cell(cell(&home, "eu-west"));
            reg.insert_cell(cell(&wrong, "eu-west"));
            reg.place_tenant(placement(
                &tenant,
                "eu-west",
                &home,
                &format!("mis-{suffix}"),
            ))
            .expect("placed on the home cell");

            let gw = CellGateway::with_audit(
                CellId::from_token(&wrong),
                MisrouteAudit::with_pg(
                    DurableMisrouteAuditBacking::new(pool.clone()),
                    tokio::runtime::Handle::current(),
                ),
            );
            let reject = gw
                .route(&reg, &TenantId::from_token(&tenant))
                .expect_err("the wrong cell rejects the misroute");
            assert!(matches!(reject, GatewayReject::Misroute(_)));
            let no_route = gw
                .route(&reg, &TenantId::from_token(&ghost))
                .expect_err("an unknown tenant is rejected");
            assert!(matches!(no_route, GatewayReject::NoSuchTenant { .. }));
            assert_eq!(gw.misroute_count(), 2);
            assert_eq!(
                gw.cross_tenant_reads(),
                0,
                "the CP-D2 zero holds on the durable-audit path"
            );

            let audit2 = DurableMisrouteAuditBacking::new(fresh_pool().await);
            let recs: Vec<_> = audit2
                .records()
                .await
                .expect("read the durable trail")
                .into_iter()
                .filter(|r| r.tenant_id == tenant || r.tenant_id == ghost)
                .collect();
            assert_eq!(
                recs.len(),
                2,
                "both rejections landed in the durable sink via the gateway"
            );
            assert_eq!(recs[0].tenant_id, tenant);
            assert_eq!(recs[0].received_by_cell, wrong);
            assert_eq!(
                recs[0].home_cell.as_deref(),
                Some(home.as_str()),
                "the redirect target survived"
            );
            assert_eq!(recs[1].tenant_id, ghost);
            assert_eq!(
                recs[1].home_cell, None,
                "no redirect target for an unknown tenant"
            );
        },
        || async {
            cleanup(pool, &suffix).await;
        },
    )
    .await;
    println!("OK [4]: gateway-rejected misroutes land in the DURABLE audit sink and survive a fresh pool.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn self_host_boots_on_pg_and_routing_survives_restart() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell_id = format!("cellself{suffix}");
    let pool = provider.db_pool();
    let tenant_slot: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);

    common::with_cleanup(
        || async {
            let tenant = {
                let mut sh = DegenerateControlPlane::with_pg(
                    CellId::from_token(&cell_id),
                    Region::new("eu-west"),
                    format!("cell.eu-west.{cell_id}.local"),
                    DurablePlacementBacking::new(pool.clone()),
                    DurableMisrouteAuditBacking::new(pool.clone()),
                    tokio::runtime::Handle::current(),
                );
                assert_eq!(
                    sh.cell().cell_id.as_str(),
                    cell_id,
                    "the one cell row is durable"
                );
                let service = PlacementService::new(CounterMinter::new());
                let answer = sh
                    .place(&service, IsolationKind::Pool, &format!("team-{suffix}"))
                    .expect("the one Active cell is eligible → placed");
                assert_eq!(answer.home_cell.as_str(), cell_id);
                answer.tenant_id
            };
            *tenant_slot.borrow_mut() = Some(tenant.as_str().to_string());

            let sh2 = DegenerateControlPlane::with_pg(
                CellId::from_token(&cell_id),
                Region::new("eu-west"),
                format!("cell.eu-west.{cell_id}.local"),
                DurablePlacementBacking::new(fresh_pool().await),
                DurableMisrouteAuditBacking::new(fresh_pool().await),
                tokio::runtime::Handle::current(),
            );
            let discovered = sh2
                .discover_cell(&tenant)
                .expect("the placed tenant STILL discovers after the restart");
            assert_eq!(
                discovered.as_str(),
                cell_id,
                "discover returns 'this cell' after restart"
            );
            let p = sh2
                .placement_of(&tenant)
                .expect("the placement survived the restart");
            assert_eq!(p.home_cell.as_str(), cell_id);
            assert_eq!(p.region.as_str(), "eu-west");
            let served = sh2
                .route(&tenant)
                .expect("the one cell serves its tenant after the restart");
            assert_eq!(served.home_cell.as_str(), cell_id);

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
        },
        || async {
            let tenant = tenant_slot.borrow_mut().take();
            if let Some(tenant) = tenant {
                let _ = sqlx::query("DELETE FROM tenant_placement WHERE tenant_id = $1")
                    .bind(&tenant)
                    .execute(pool)
                    .await;
            }
            cleanup(pool, &suffix).await;
        },
    )
    .await;
    println!(
        "OK [5]: the self-host root boots on the Pg arm, routing survives a restart, and a \
         wrong-region re-boot is REFUSED at boot."
    );
}
