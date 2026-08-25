#![cfg(feature = "integration")]

mod common;

use myelin_events::Timestamp;
use myelin_identity::{
    DelegationCaveats, FailStaticBound, Principal, PrincipalId, PrincipalKind, RevokeTarget, RunId,
    RuntimeRef,
};
use myelin_identity_service::revocation::{RevocationStore, RunTokenState};
use myelin_identity_service::{
    Authority, CellTokenAuthority, DelegationInput, MachineKind, MintError, PasetoCapabilitySigner,
    RunTokenMinter, TupleStore, RUN_GRANT_RELATION,
};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, DurableRevocationBacking, DurableTupleBacking, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};

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

fn run_participants(s: &myelin_storage::TenantScope) -> (Principal, Principal) {
    let mut agent = Principal::stub(
        PrincipalId("p:agent".into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt:agent".into()),
            on_behalf_of: Some(PrincipalId("p:human".into())),
        },
        s.tenant().clone(),
    );
    agent.region = s.region().clone();
    let mut human = Principal::stub(
        PrincipalId("p:human".into()),
        PrincipalKind::Human,
        s.tenant().clone(),
    );
    human.region = s.region().clone();
    (agent, human)
}

fn run_minter(
    revocations: RevocationStore,
    tuples: Option<TupleStore>,
    signer: std::sync::Arc<PasetoCapabilitySigner>,
) -> RunTokenMinter {
    RunTokenMinter::with_signer_and_tuples(revocations, tuples, signer)
}

fn mint_one_run(
    minter: &RunTokenMinter,
    s: &myelin_storage::TenantScope,
    run_id: &str,
) -> Result<myelin_identity::RunToken, MintError> {
    let grant = "repo:acme/core#read";
    let (agent, human) = run_participants(s);
    let authority = Authority::of([grant]);
    minter.mint_run_token(
        s,
        &agent.principal_id,
        &RunId(run_id.into()),
        &agent,
        &human,
        &DelegationInput {
            agent_policy: authority.clone(),
            delegation: authority.clone(),
            tenant_policy: authority.clone(),
            trigger_actor_held: authority,
        },
        &DelegationCaveats(vec![grant.into()]),
        MachineKind::Agent,
        &FailStaticBound {
            static_max_secs: 300,
        },
        &ts("2099-06-26T00:00:00Z"),
    )
}

async fn migrate() -> SubstrateProvider {
    let admin = common::admin_provider(4).await;
    admin
        .migrate(&identity_durable_migrations(), &HotTables::none())
        .await
        .expect("identity durable migrations execute against the live DB");
    admin
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
            "DELETE FROM outbox WHERE tenant_id = $1",
            "DELETE FROM rebac_tuple WHERE tenant_id = $1",
            "DELETE FROM revocation WHERE tenant_id = $1",
            "DELETE FROM run_token_teardown WHERE tenant_id = $1",
        ] {
            let _ = sqlx::query(sql).bind(t).execute(admin.db_pool()).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revocation_is_durable_and_idempotent_across_a_fresh_store_instance() {
    let admin = migrate().await;
    let app = common::app_provider(6).await;
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr008-rev-{suffix}");
    let s = scope(&tenant, &region);

    let store1 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    let jti = RevokeTarget::Jti("jti-1".into());
    store1
        .revoke(&s, &jti, ts("2026-06-26T00:00:00Z"))
        .expect("persist revoked token");
    store1
        .disable_principal(
            &s,
            &PrincipalId("p:alice".into()),
            ts("2026-06-26T00:00:00Z"),
        )
        .expect("persist disabled principal");
    store1
        .revoke(&s, &jti, ts("2026-06-26T09:00:00Z"))
        .expect("repeat revocation remains idempotent");

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
        store2.revocation_count(&s).expect("count revocations"),
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
    let admin = migrate().await;
    let app = common::app_provider(6).await;
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant = format!("mr008-ttl-{suffix}");
    let s = scope(&tenant, &region);

    let store1 =
        RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle.clone());
    store1
        .register_run_token_ttl(
            &s,
            "run-jti",
            ts("2026-06-26T00:00:00Z"),
            ts("2026-06-26T00:05:00Z"),
        )
        .expect("persist live run lifetime");
    store1
        .register_run_token_ttl(
            &s,
            "torn-jti",
            ts("2026-06-26T00:00:00Z"),
            ts("2026-06-26T00:05:00Z"),
        )
        .expect("persist torn-down run lifetime");
    store1
        .tear_down_run_token(&s, "torn-jti", ts("2026-06-26T00:01:00Z"))
        .expect("persist run teardown");

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

    store1
        .register_run_token_ttl(
            &s,
            "frac-jti",
            ts("2026-06-26T00:00:00Z"),
            ts("2026-06-26T00:05:00Z"),
        )
        .expect("persist fractional-expiry run lifetime");
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
    let admin = migrate().await;
    let app = common::app_provider(6).await;
    let region = app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let suffix = uniq();
    let tenant_a = format!("mr008A-{suffix}");
    let tenant_b = format!("mr008B-{suffix}");
    let sa = scope(&tenant_a, &region);
    let sb = scope(&tenant_b, &region);

    let store = RevocationStore::with_pg(DurableRevocationBacking::new(app.clone()), handle);
    let jti = RevokeTarget::Jti("jti-secret".into());
    store
        .revoke(&sa, &jti, ts("2026-06-26T00:00:00Z"))
        .expect("persist tenant A revocation");
    store
        .disable_principal(
            &sa,
            &PrincipalId("p:alice".into()),
            ts("2026-06-26T00:00:00Z"),
        )
        .expect("persist tenant A principal disablement");

    assert!(
        store.is_revoked(&sa, &jti, &ts("2026-06-26T00:00:01Z")),
        "tenant A sees its revocation"
    );
    assert_eq!(store.revocation_count(&sa).expect("count tenant A"), 2);

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
        store.revocation_count(&sb).expect("count tenant B"),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_durable_revocation_write_reports_an_outage_and_can_be_retried() {
    let admin = migrate().await;
    let unavailable = common::app_provider(2).await;
    let region = unavailable.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let tenant = format!("mr008-outage-{}", uniq());
    let s = scope(&tenant, &region);
    let store = RevocationStore::with_pg(
        DurableRevocationBacking::new(unavailable.clone()),
        handle.clone(),
    );
    let minter = run_minter(
        store.clone(),
        None,
        std::sync::Arc::new(PasetoCapabilitySigner::new(std::sync::Arc::new(
            CellTokenAuthority::generate(),
        ))),
    );

    unavailable.db_pool().close().await;

    assert_eq!(
        mint_one_run(&minter, &s, "during-outage"),
        Err(MintError::RevocationUnavailable),
        "the caller receives no credential when its durable run lifetime cannot be recorded"
    );

    assert!(
        store
            .revoke(
                &s,
                &RevokeTarget::Jti("revoked-during-outage".into()),
                ts("2026-06-26T00:00:00Z"),
            )
            .is_err(),
        "an operator is told that the denylist write did not happen"
    );
    assert!(
        store
            .disable_principal(
                &s,
                &PrincipalId("p:disabled-during-outage".into()),
                ts("2026-06-26T00:00:00Z"),
            )
            .is_err(),
        "an erasure or SCIM caller is told that principal disablement did not happen"
    );
    assert!(
        store
            .register_run_token_ttl(
                &s,
                "minted-during-outage",
                ts("2026-06-26T00:00:00Z"),
                ts("2026-06-26T00:05:00Z"),
            )
            .is_err(),
        "a mint caller cannot mistake an unrecorded run lifetime for a usable credential"
    );
    assert!(
        store
            .tear_down_run_token(&s, "torn-down-during-outage", ts("2026-06-26T00:01:00Z"),)
            .is_err(),
        "a run owner is told that teardown did not reach durable storage"
    );
    assert!(
        store.revocation_count(&s).is_err(),
        "an unavailable database is not reported as an empty denylist"
    );
    assert_eq!(
        store.telemetry().revocation_count(),
        0,
        "failed writes are not counted as successful revocation observations"
    );

    let recovered = common::app_provider(4).await;
    let store = RevocationStore::with_pg(DurableRevocationBacking::new(recovered), handle);
    store
        .revoke(
            &s,
            &RevokeTarget::Jti("revoked-during-outage".into()),
            ts("2026-06-26T00:02:00Z"),
        )
        .expect("retry denylist write after recovery");
    store
        .disable_principal(
            &s,
            &PrincipalId("p:disabled-during-outage".into()),
            ts("2026-06-26T00:02:00Z"),
        )
        .expect("retry principal disablement after recovery");
    store
        .register_run_token_ttl(
            &s,
            "minted-during-outage",
            ts("2026-06-26T00:02:00Z"),
            ts("2026-06-26T00:05:00Z"),
        )
        .expect("retry run lifetime write after recovery");
    store
        .tear_down_run_token(&s, "torn-down-during-outage", ts("2026-06-26T00:02:00Z"))
        .expect("retry teardown after recovery");

    assert_eq!(
        store.revocation_count(&s).expect("count after recovery"),
        3,
        "the retried token, principal, and run lifetime are now durable"
    );
    assert_eq!(
        store.telemetry().revocation_count(),
        4,
        "each successful retry is observed once"
    );

    cleanup(&admin, &[&tenant]).await;
    println!("OK [d]: revocation mutations report outages honestly and remain retryable.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mint_returns_a_credential_only_after_its_run_grant_is_durable() {
    let admin = migrate().await;
    let revocation_app = common::app_provider(4).await;
    let unavailable_graph = common::app_provider(2).await;
    let region = revocation_app.config().region.clone();
    let handle = tokio::runtime::Handle::current();
    let tenant = format!("mr008-grant-outage-{}", uniq());
    let s = scope(&tenant, &region);
    let revocations = RevocationStore::with_pg(
        DurableRevocationBacking::new(revocation_app.clone()),
        handle.clone(),
    );
    let unavailable_tuples = TupleStore::with_pg(
        DurableTupleBacking::new(unavailable_graph.clone()),
        handle.clone(),
    );
    let signer = std::sync::Arc::new(PasetoCapabilitySigner::new(std::sync::Arc::new(
        CellTokenAuthority::generate(),
    )));
    let minter = run_minter(
        revocations.clone(),
        Some(unavailable_tuples),
        signer.clone(),
    );

    unavailable_graph.db_pool().close().await;

    assert_eq!(
        mint_one_run(&minter, &s, "grant-retry"),
        Err(MintError::RunGrantUnavailable),
        "a healthy lifetime store is insufficient: no credential leaves the mint boundary while its authorization graph is down"
    );

    let recovered_tuples =
        TupleStore::with_pg(DurableTupleBacking::new(revocation_app.clone()), handle);
    let recovered_minter = run_minter(revocations, Some(recovered_tuples.clone()), signer);
    mint_one_run(&recovered_minter, &s, "grant-retry")
        .expect("the same mint request succeeds after the authorization graph recovers");

    let grants = recovered_tuples
        .tuples_in(&s)
        .expect("read the recovered run authorization graph");
    let grant = grants
        .iter()
        .find(|stored| stored.tuple.object.0 == "run:grant-retry")
        .expect("the run grant is durable before the credential is returned");
    assert_eq!(grant.tuple.relation.0, RUN_GRANT_RELATION);
    assert_eq!(grant.tuple.subject.0, "p:agent");
    assert_eq!(
        grant.expires_at,
        Some(ts("2099-06-26T00:05:00.000000Z")),
        "the recovered grant expires with the run credential"
    );

    cleanup(&admin, &[&tenant]).await;
    println!("OK [e]: credential mint waits for both durable lifetime and run grant.");
}
