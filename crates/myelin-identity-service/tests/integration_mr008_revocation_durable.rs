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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_is_durable_and_idempotent_across_a_fresh_store_instance() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr008-rev-{suffix}");
    let s = scope(&tenant, &region);

    let store1 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let jti = RevokeTarget::Jti("jti-1".into());
    store1.revoke(&s, &jti, ts("2026-06-26T00:00:00Z"));
    store1.disable_principal(
        &s,
        &PrincipalId("p:alice".into()),
        ts("2026-06-26T00:00:00Z"),
    );
    store1.revoke(&s, &jti, ts("2026-06-26T09:00:00Z"));

    let store2 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
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
    store2.recover_from_mirror();
    assert!(store2.is_revoked(&s, &jti, &ts("2026-06-26T00:00:01Z")));

    cleanup(&admin, &[&tenant]).await;
    println!(
        "OK [a]: revocation durable + idempotent across a fresh store instance over the same pool."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_token_expiry_and_teardown_are_durable_across_a_fresh_instance() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr008-ttl-{suffix}");
    let s = scope(&tenant, &region);

    let store1 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    store1.register_run_token_ttl(
        &s,
        "run-jti",
        ts("2026-06-26T00:00:00Z"),
        ts("2026-06-26T00:05:00Z"),
    );
    store1.register_run_token_ttl(
        &s,
        "torn-jti",
        ts("2026-06-26T00:00:00Z"),
        ts("2026-06-26T00:05:00Z"),
    );
    store1.tear_down_run_token(&s, "torn-jti", ts("2026-06-26T00:01:00Z"));

    let store2 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let run = RevokeTarget::Jti("run-jti".into());

    assert!(
        store2.is_revoked(&s, &run, &ts("2026-06-26T00:02:00Z")),
        "within run-life the token is still denylisted across a fresh instance"
    );
    assert_eq!(
        store2.run_token_state(&s, &run, &ts("2026-06-26T00:02:00Z")),
        RunTokenState::LiveWithinRunLife,
        "the TTL is durable: Live within run-life after a fresh instance"
    );
    assert!(
        !store2.is_revoked(&s, &run, &ts("2026-06-26T00:06:00Z")),
        "after expires_at the token is no longer a revocation (durable auto-expire)"
    );
    assert_eq!(
        store2.run_token_state(&s, &run, &ts("2026-06-26T00:06:00Z")),
        RunTokenState::Expired,
        "expiry survives a fresh instance: a token past its TTL reads Expired"
    );

    assert_eq!(
        store2.run_token_state(
            &s,
            &RevokeTarget::Jti("torn-jti".into()),
            &ts("2026-06-26T00:02:00Z")
        ),
        RunTokenState::TornDown,
        "an explicit teardown is durable: reads TornDown across a fresh instance"
    );
    assert_eq!(
        store2.run_token_state(
            &s,
            &RevokeTarget::Jti("never".into()),
            &ts("2026-06-26T00:02:00Z")
        ),
        RunTokenState::Unknown,
        "an unminted jti fails closed (Unknown), never Live"
    );

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tenant_a_revocations_invisible_to_b_and_no_guc_bleeds() {
    let Some(admin) = migrate().await else { return };
    let Some(app) = app_provider().await else {
        return;
    };
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
    store.disable_principal(
        &sa,
        &PrincipalId("p:alice".into()),
        ts("2026-06-26T00:00:00Z"),
    );

    assert!(
        store.is_revoked(&sa, &jti, &ts("2026-06-26T00:00:01Z")),
        "tenant A sees its revocation"
    );
    assert_eq!(store.revocation_count(&sa), 2);

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
    assert_eq!(
        store.revocation_count(&sb),
        0,
        "tenant B's revocation partition is empty"
    );

    assert!(
        residual_guc(app.db_pool()).await.is_empty(),
        "no residual myelin.tenant_id GUC after the tenant-scoped revocation ops (no bleed)"
    );

    cleanup(&admin, &[&tenant_a, &tenant_b]).await;
    println!("OK [c]: tenant A's revocations invisible to tenant B (RLS via with_tenant_tx); no GUC bleed.");
}
