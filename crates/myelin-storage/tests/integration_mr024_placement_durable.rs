//! # MR-024 — durable control-plane placement registry, proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-storage --features integration \
//!       --test integration_mr024_placement_durable -- --nocapture
//!
//! It proves the MR-024 deliverables — each MUST hit the live DB (a pass on the in-memory model
//! would NOT count):
//!   1. **Durability (SI-011):** a tenant→cell placement registered via ONE backing instance is read
//!      back by a FRESH instance over a FRESH pool (new connections) — it survived in Postgres.
//!   2. **The placement invariant is a REAL DB TRIGGER:** a DIRECT raw `INSERT` (bypassing all Rust
//!      app logic) of a cross-region placement is REJECTED by the database; an unknown-cell placement
//!      is refused fail-closed; region-immutability on UPDATE is enforced.
//!   3. **MisrouteAudit durability (SI-028):** a recorded misroute survives a fresh instance.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::migration::HotTables;
use myelin_storage::placement_durable::{
    placement_durable_migrations, DurableCellRow, DurableMisrouteAuditBacking,
    DurablePlacementBacking, DurablePlacementRow, PlacementWriteError,
};
use myelin_storage::{SubstrateProvider};

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

/// Build an admin-role provider, applying the placement migrations once; `None` (SKIP) if unreachable.
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
        .expect("apply the placement migrations (cell + tenant_placement + TRIGGER + misroute_audit)");
    Some(provider)
}

fn cell_row(cell_id: &str, region: &str) -> DurableCellRow {
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
        endpoint: format!("cell.{region}.myelin.eu"),
    }
}

fn placement_row(tenant: &str, region: &str, home: &str, members: &[&str], slug: &str) -> DurablePlacementRow {
    DurablePlacementRow {
        tenant_id: tenant.into(),
        region: region.into(),
        home_cell: home.into(),
        isolation_tier: "Pool".into(),
        slug: slug.into(),
        status: "Active".into(),
        member_cells: members.iter().map(|s| s.to_string()).collect(),
    }
}

// =================================================================================================
// 1 — Durability (SI-011): a placement registered via one instance is read back by a FRESH instance.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn placement_survives_a_fresh_instance_over_a_fresh_pool() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let cell_id = format!("cell-w-{suffix}");
    let tenant = format!("01J0ACME{suffix}");
    let slug = format!("acme-{suffix}");

    // (1) Register the cell + place the tenant through ONE backing instance.
    let backing1 = DurablePlacementBacking::new(provider.db_pool().clone());
    backing1.insert_cell(&cell_row(&cell_id, "eu-west")).await.expect("insert cell");
    backing1
        .place_tenant(&placement_row(&tenant, "eu-west", &cell_id, &[&cell_id], &slug))
        .await
        .expect("a single-region placement is admitted by the trigger");

    // (2) A FRESH backing over a FRESH pool (new connections — proves it is in Postgres, not memory).
    let fresh_pool = SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2)
        .await
        .expect("fresh pool")
        .db_pool()
        .clone();
    let backing2 = DurablePlacementBacking::new(fresh_pool);
    let read = backing2
        .get_placement(&tenant)
        .await
        .expect("read")
        .expect("the placement SURVIVED into the fresh instance (durable)");
    assert_eq!(read.home_cell, cell_id, "the tenant→cell routing survived restart");
    assert_eq!(read.region, "eu-west");
    assert_eq!(read.member_cells, vec![cell_id.clone()]);
    assert_eq!(read.status, "Active");

    // cleanup.
    let _ = sqlx::query("DELETE FROM tenant_placement WHERE tenant_id = $1").bind(&tenant).execute(provider.db_pool()).await;
    let _ = sqlx::query("DELETE FROM cell WHERE cell_id = $1").bind(&cell_id).execute(provider.db_pool()).await;
    println!("OK [1]: tenant→cell placement survived a fresh instance over a fresh pool (durable).");
}

// =================================================================================================
// 2 — The placement invariant is a REAL DB TRIGGER (a DIRECT raw INSERT is rejected by the DB).
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn placement_invariant_is_a_real_db_trigger() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let west = format!("cell-w-{suffix}");
    let north = format!("cell-n-{suffix}");
    let tenant = format!("01J0X{suffix}");
    let pool = provider.db_pool();

    let backing = DurablePlacementBacking::new(pool.clone());
    backing.insert_cell(&cell_row(&west, "eu-west")).await.expect("home cell");
    backing.insert_cell(&cell_row(&north, "eu-north")).await.expect("member cell, WRONG region");

    // (a) A DIRECT raw INSERT (bypassing ALL Rust app logic) of a cross-region placement is REJECTED
    //     by the database — the trigger fires, not the in-code check. This is the headline proof.
    let direct = sqlx::query(
        "INSERT INTO tenant_placement (tenant_id, region, home_cell, isolation_tier, slug, status, member_cells) \
         VALUES ($1,'eu-west',$2,'Pool',$3,'Active',$4)",
    )
    .bind(&tenant)
    .bind(&west)
    .bind(format!("slugdirect-{suffix}"))
    .bind(vec![west.clone(), north.clone()]) // home in-region, member in WRONG region.
    .execute(pool)
    .await;
    let err = direct.expect_err("the DB TRIGGER must reject a cross-region member cell on a DIRECT insert");
    let dberr = err.as_database_error().expect("a database error");
    assert_eq!(dberr.code().as_deref(), Some("23514"), "the trigger raises SQLSTATE check_violation");
    assert!(
        err.to_string().contains("single-region by construction"),
        "the DB trigger's rejection is loud + named: {err}"
    );

    // (b) The typed backing surfaces the SAME rejection as `InvariantRejected` (not a silent failure).
    let via_backing = backing
        .place_tenant(&placement_row(&tenant, "eu-west", &west, &[&west, &north], &format!("s2-{suffix}")))
        .await
        .expect_err("the backing surfaces the trigger rejection");
    assert!(matches!(via_backing, PlacementWriteError::InvariantRejected(_)), "got {via_backing}");

    // (c) Unknown-cell placement → refused FAIL-CLOSED (the region pin cannot be verified).
    let ghost = format!("cell-ghost-{suffix}");
    let unknown = backing
        .place_tenant(&placement_row(&tenant, "eu-west", &ghost, &[&ghost], &format!("s3-{suffix}")))
        .await
        .expect_err("an unknown cell is refused fail-closed by the trigger");
    match &unknown {
        PlacementWriteError::InvariantRejected(why) => assert!(why.contains("fail-closed"), "loud: {why}"),
        other => panic!("expected InvariantRejected(fail-closed), got {other}"),
    }

    // (d) NOTHING landed — every rejected placement was refused (the trigger refuses the write).
    assert_eq!(
        backing.get_placement(&tenant).await.expect("read"),
        None,
        "no rejected placement was stored (the trigger refuses the write)"
    );

    // (e) Region immutability on UPDATE (§5.3 layer 1): place legally, then a region change is rejected.
    backing.insert_cell(&cell_row(&format!("{north}b"), "eu-north")).await.expect("a north cell for the legal placement");
    backing
        .place_tenant(&placement_row(&tenant, "eu-north", &format!("{north}b"), &[&format!("{north}b")], &format!("s4-{suffix}")))
        .await
        .expect("a legal single-region (eu-north) placement is admitted");
    let region_change = sqlx::query("UPDATE tenant_placement SET region = 'eu-west' WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(pool)
        .await
        .expect_err("a region change is rejected by the trigger (region is immutable)");
    assert!(region_change.to_string().contains("region is immutable"), "loud: {region_change}");

    // cleanup.
    let _ = sqlx::query("DELETE FROM tenant_placement WHERE tenant_id = $1").bind(&tenant).execute(pool).await;
    for c in [&west, &north, &format!("{north}b")] {
        let _ = sqlx::query("DELETE FROM cell WHERE cell_id = $1").bind(c).execute(pool).await;
    }
    println!("OK [2]: the placement invariant is a REAL DB trigger — cross-region + unknown-cell + region-change all rejected at the DB.");
}

// =================================================================================================
// 3 — MisrouteAudit durability (SI-028): a recorded misroute survives a fresh instance.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn misroute_audit_survives_a_fresh_instance() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let tenant = format!("01J0MIS{suffix}");
    let received = format!("cell-w2-{suffix}");
    let home = format!("cell-w1-{suffix}");

    // Record via one instance.
    let audit1 = DurableMisrouteAuditBacking::new(provider.db_pool().clone());
    let before = audit1.count().await.expect("count");
    audit1.record(&tenant, &received, Some(&home)).await.expect("record a misroute");
    audit1.record(&tenant, &received, None).await.expect("record an unknown-tenant misroute (no home)");

    // A FRESH instance over a FRESH pool reads the audit trail back (durable).
    let fresh_pool = SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2)
        .await
        .expect("fresh pool")
        .db_pool()
        .clone();
    let audit2 = DurableMisrouteAuditBacking::new(fresh_pool);
    assert_eq!(audit2.count().await.expect("count") - before, 2, "both misroutes survived");
    let mine: Vec<_> = audit2
        .records()
        .await
        .expect("records")
        .into_iter()
        .filter(|r| r.tenant_id == tenant)
        .collect();
    assert_eq!(mine.len(), 2, "the audit trail for this tenant survived the fresh instance");
    assert_eq!(mine[0].home_cell.as_deref(), Some(home.as_str()), "the redirect target survived");
    assert_eq!(mine[1].home_cell, None, "the no-home (unknown-tenant) misroute survived");

    // cleanup.
    let _ = sqlx::query("DELETE FROM misroute_audit WHERE tenant_id = $1").bind(&tenant).execute(provider.db_pool()).await;
    println!("OK [3]: the misroute audit trail survived a fresh instance (durable SI-028).");
}
