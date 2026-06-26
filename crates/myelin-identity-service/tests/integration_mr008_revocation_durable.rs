//! # MR-008 — durable revocation + expiry, proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test` stays DB-free. Runs ONLY
//! against the docker-compose dev stack:
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-identity-service --features integration \
//!       --test integration_mr008_revocation_durable -- --nocapture
//!
//! Proves the MR-008 deliverables (each hits live PG via a `with_pg` store — a pass on the in-memory
//! model would not count):
//!   (a) **revocation durability** — revoke a target, read it back as revoked via a FRESH store
//!       instance over the SAME pool (restart simulation; the real kill-9 is MR-009);
//!   (b) **expiry durability** — register a run-token TTL; after its `expires_at` it reads expired
//!       across a fresh instance (the column MR-007 deferred, now persisted + correct); plus the
//!       explicit-teardown leg reads `TornDown` across a fresh instance;
//!   (c) **tenant isolation** — tenant A's revocations are invisible to tenant B under RLS (NOBYPASSRLS
//!       app role), and no GUC bleeds after the connection is released.
//!
//! Idempotency (4.7) across a restart is also asserted: a double-revoke (across a fresh instance) does
//! not grow the count and preserves the FIRST `revoked_at` (ON CONFLICT DO NOTHING).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RevokeTarget};
use myelin_identity_service::revocation::{RevocationStore, RunTokenState};
use myelin_storage::migration::HotTables;
use myelin_storage::{identity_durable_migrations, DurableRevocationBacking, SubstrateProvider};
use myelin_tenancy::{Region, TenantId};

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

fn scope(tenant: &str, region: &str) -> myelin_storage::TenantScope {
    let p = Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    myelin_storage::TenantScope::from_verified_token(&p, Region(region.into()))
}

fn ts(s: &str) -> Timestamp {
    Timestamp(s.into())
}

async fn app_provider() -> Option<SubstrateProvider> {
    match SubstrateProvider::connect(MyelinConfig::dev(), 6).await {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            None
        }
    }
}

async fn migrate() -> Option<SubstrateProvider> {
    let admin = match SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return None;
        }
    };
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations execute against the live DB");
    Some(admin)
}

async fn residual_guc(pool: &sqlx::PgPool) -> String {
    let mut conn = pool.acquire().await.expect("acquire");
    let v: Option<String> = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
        .fetch_one(&mut *conn)
        .await
        .expect("read GUC");
    v.unwrap_or_default()
}

async fn cleanup(admin: &SubstrateProvider, tenants: &[&str]) {
    for t in tenants {
        for sql in [
            "DELETE FROM revocation WHERE tenant_id = $1",
            "DELETE FROM run_token_teardown WHERE tenant_id = $1",
        ] {
            let _ = sqlx::query(sql).bind(t).execute(admin.db_pool()).await;
        }
    }
}

// =================================================================================================
// (a) Revocation durability + idempotency across a fresh store instance over the same pool.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_is_durable_and_idempotent_across_a_fresh_store_instance() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else { return };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr008-rev-{suffix}");
    let s = scope(&tenant, &region);

    // Revoke a jti + disable a principal through store instance #1.
    let store1 = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let jti = RevokeTarget::Jti("jti-1".into());
    store1.revoke(&s, &jti, ts("2026-06-26T00:00:00Z"));
    store1.disable_principal(&s, &PrincipalId("p:alice".into()), ts("2026-06-26T00:00:00Z"));
    // A double-revoke at a LATER time — idempotent (ON CONFLICT DO NOTHING preserves the first).
    store1.revoke(&s, &jti, ts("2026-06-26T09:00:00Z"));

    // Read back through a FRESH store instance #2 over the SAME pool (restart simulation).
    let store2 = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    assert!(
        store2.is_revoked(&s, &jti, &ts("2026-06-26T00:00:01Z")),
        "a revoked jti reads back as revoked from a fresh store instance (durable)"
    );
    assert!(
        store2.is_revoked(
            &s,
            &RevokeTarget::Principal(PrincipalId("p:alice".into())),
            &ts("2026-06-26T00:00:01Z")
        ),
        "a disabled principal reads back as revoked across surfaces (durable)"
    );
    assert_eq!(
        store2.revocation_count(&s),
        2,
        "a double-revoke does not grow the durable denylist (idempotent even across a fresh instance)"
    );
    // recover_from_mirror is a no-op on the Pg path (the durable table IS the mirror) — still revoked.
    store2.recover_from_mirror();
    assert!(store2.is_revoked(&s, &jti, &ts("2026-06-26T00:00:01Z")));

    cleanup(&admin, &[&tenant]).await;
    println!("OK [a]: revocation durable + idempotent across a fresh store instance over the same pool.");
}

// =================================================================================================
// (b) Expiry durability: a run-token TTL reads expired after its TTL; teardown reads TornDown — both
//     across a fresh store instance over the same pool.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_token_expiry_and_teardown_are_durable_across_a_fresh_instance() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else { return };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr008-ttl-{suffix}");
    let s = scope(&tenant, &region);

    // Register a per-run-token TTL (expires_at == run-life) through instance #1.
    let store1 = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    store1.register_run_token_ttl(
        &s,
        "run-jti",
        ts("2026-06-26T00:00:00Z"),
        ts("2026-06-26T00:05:00Z"),
    );
    // A separate run token we will explicitly tear down.
    store1.register_run_token_ttl(
        &s,
        "torn-jti",
        ts("2026-06-26T00:00:00Z"),
        ts("2026-06-26T00:05:00Z"),
    );
    store1.tear_down_run_token(&s, "torn-jti", ts("2026-06-26T00:01:00Z"));

    // FRESH instance #2 over the SAME pool.
    let store2 = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let run = RevokeTarget::Jti("run-jti".into());

    // BEFORE expiry: denylisted + Live (the TTL survived the "restart").
    assert!(
        store2.is_revoked(&s, &run, &ts("2026-06-26T00:02:00Z")),
        "within run-life the token is still denylisted across a fresh instance"
    );
    assert_eq!(
        store2.run_token_state(&s, &run, &ts("2026-06-26T00:02:00Z")),
        RunTokenState::LiveWithinRunLife,
        "the TTL is durable: Live within run-life after a fresh instance"
    );
    // AFTER expiry: no longer a revocation + reads Expired — expiry is DURABLE + correct across restart.
    assert!(
        !store2.is_revoked(&s, &run, &ts("2026-06-26T00:06:00Z")),
        "after expires_at the token is no longer a revocation (durable auto-expire)"
    );
    assert_eq!(
        store2.run_token_state(&s, &run, &ts("2026-06-26T00:06:00Z")),
        RunTokenState::Expired,
        "expiry survives a fresh instance: a token past its TTL reads Expired"
    );

    // The explicit teardown survived: torn-jti reads TornDown across the fresh instance (the immediate
    // deny takes precedence over its TTL).
    assert_eq!(
        store2.run_token_state(
            &s,
            &RevokeTarget::Jti("torn-jti".into()),
            &ts("2026-06-26T00:02:00Z")
        ),
        RunTokenState::TornDown,
        "an explicit teardown is durable: reads TornDown across a fresh instance"
    );
    // An unknown jti fails closed (no record → Unknown, never Live).
    assert_eq!(
        store2.run_token_state(&s, &RevokeTarget::Jti("never".into()), &ts("2026-06-26T00:02:00Z")),
        RunTokenState::Unknown,
        "an unminted jti fails closed (Unknown), never Live"
    );

    // Expiry is INSTANT-compared on the DURABLE path too (the verifier's fail-open, closed): a token
    // with `expires_at == …00:05:00Z` read at a `now` that is chronologically PAST it but lexically
    // BEFORE it (differing fractional precision; and a non-`Z` offset) reads EXPIRED across the fresh
    // instance — a raw string compare would have read LIVE here.
    store1.register_run_token_ttl(
        &s,
        "frac-jti",
        ts("2026-06-26T00:00:00Z"),
        ts("2026-06-26T00:05:00Z"),
    );
    let frac = RevokeTarget::Jti("frac-jti".into());
    assert!(
        !store2.is_revoked(&s, &frac, &ts("2026-06-26T00:05:00.5Z")),
        "durable expiry by instant: 0.5s past expiry reads not-revoked (lexical compare would fail open)"
    );
    assert_eq!(
        store2.run_token_state(&s, &frac, &ts("2026-06-26T02:06:00+02:00")),
        RunTokenState::Expired,
        "durable expiry by instant: a non-`Z` offset chronologically past expiry reads Expired"
    );

    cleanup(&admin, &[&tenant]).await;
    println!("OK [b]: run-token TTL expiry + explicit teardown durable + correct across a fresh instance.");
}

// =================================================================================================
// (c) Tenant isolation through the NOBYPASSRLS app role + no GUC bleed.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenant_a_revocations_invisible_to_b_and_no_guc_bleeds() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else { return };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant_a = format!("mr008A-{suffix}");
    let tenant_b = format!("mr008B-{suffix}");
    let sa = scope(&tenant_a, &region);
    let sb = scope(&tenant_b, &region);

    let store = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle);
    let jti = RevokeTarget::Jti("jti-secret".into());
    store.revoke(&sa, &jti, ts("2026-06-26T00:00:00Z"));
    store.disable_principal(&sa, &PrincipalId("p:alice".into()), ts("2026-06-26T00:00:00Z"));

    // Tenant A sees its own revocations.
    assert!(store.is_revoked(&sa, &jti, &ts("2026-06-26T00:00:01Z")), "tenant A sees its revocation");
    assert_eq!(store.revocation_count(&sa), 2);

    // Tenant B — through the SAME app-role pool — sees NONE of tenant A's revocations. The isolation
    // is the DB FORCE-RLS policy inside the with_tenant_tx transaction (a wrong-tenant session reads
    // zero rows). If RLS were absent, B would see A's revocation OR (worse for revocation) a revoked
    // handle could read across tenants — both are caught here.
    assert!(
        !store.is_revoked(&sb, &jti, &ts("2026-06-26T00:00:01Z")),
        "tenant B cannot see tenant A's revoked jti (RLS via with_tenant_tx)"
    );
    assert!(
        !store.is_revoked(
            &sb,
            &RevokeTarget::Principal(PrincipalId("p:alice".into())),
            &ts("2026-06-26T00:00:01Z")
        ),
        "tenant B cannot see tenant A's disabled principal"
    );
    assert_eq!(store.revocation_count(&sb), 0, "tenant B's revocation partition is empty");

    // No GUC bleeds after the tenant-scoped ops.
    assert!(
        residual_guc(app.db_pool()).await.is_empty(),
        "no residual myelin.tenant_id GUC after the tenant-scoped revocation ops (no bleed)"
    );

    cleanup(&admin, &[&tenant_a, &tenant_b]).await;
    println!("OK [c]: tenant A's revocations invisible to tenant B (RLS via with_tenant_tx); no GUC bleed.");
}
