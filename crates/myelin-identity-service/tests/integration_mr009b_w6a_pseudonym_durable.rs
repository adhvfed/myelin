#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_events::Timestamp;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, PseudonymHandle};
use myelin_identity_service::pseudonym_erase::PseudonymErasureLedger;
use myelin_identity_service::pseudonym_store::{PseudonymError, PseudonymStore};
use myelin_identity_service::{CellTokenAuthority, StoreBackedCheck};
use myelin_storage::migration::HotTables;
use myelin_storage::{
    identity_durable_migrations, pseudonym_durable_migrations, DekId, DurableErasureLedgerBacking,
    DurablePseudonymBacking, KeyClass, KmsEngine, SubstrateProvider, TenantScope,
};
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

fn scope(tenant: &str, region: &str) -> TenantScope {
    let p = Principal::stub(
        PrincipalId("p:admin".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    );
    TenantScope::from_verified_token(&p, Region(region.into()))
}

fn handle(pseudonym: &str, tenant: &str) -> PseudonymHandle {
    PseudonymHandle::new(pseudonym, tenant).expect("a well-formed handle")
}

fn at(t: &str) -> Timestamp {
    Timestamp(t.into())
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

async fn migrate_admin() -> Option<()> {
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
    admin
        .migrate(&pseudonym_durable_migrations(), &HotTables::none())
        .await
        .expect("W6a pseudonym durable migrations execute against the live DB");
    Some(())
}

async fn cleanup(tenant: &str, region: &str) {
    let Ok(admin) = SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2).await else {
        return;
    };
    let mut conn = admin.db_pool().acquire().await.expect("admin acquire");
    let _ = sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
        .bind(tenant)
        .execute(&mut *conn)
        .await;
    let _ = sqlx::query("SELECT set_config('myelin.region', $1, false)")
        .bind(region)
        .execute(&mut *conn)
        .await;
    for table in [
        "pseudonym_map",
        "identity_pseudonym_erasure_ledger",
        "revocation",
    ] {
        let _ = sqlx::query(&format!(
            "DELETE FROM {table} WHERE tenant_id = $1 AND region = $2"
        ))
        .bind(tenant)
        .bind(region)
        .execute(&mut *conn)
        .await;
    }
}

fn pseudonym_store(kms: &Arc<KmsEngine>, provider: &SubstrateProvider) -> PseudonymStore {
    PseudonymStore::with_pg(
        kms.clone(),
        DurablePseudonymBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    )
}

fn erasure_ledger(provider: &SubstrateProvider) -> PseudonymErasureLedger {
    PseudonymErasureLedger::with_pg(
        DurableErasureLedgerBacking::new(provider.clone()),
        tokio::runtime::Handle::current(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s2_row_survives_fresh_pool_and_crypto_shred_stays_loud_while_render_survives() {
    if migrate_admin().await.is_none() {
        return;
    }
    let Some(app1) = app_provider().await else {
        return;
    };
    let region = app1.config().region.clone();
    let suffix = uniq();
    let tenant = format!("mr009b-w6a-cs-{suffix}");
    let s = scope(&tenant, &region);
    let alice = PrincipalId("p:alice".into());
    let h = handle("anon-7f3a", &tenant);

    let kms = Arc::new(KmsEngine::new());

    let store1 = pseudonym_store(&kms, &app1);
    let written = store1
        .put_mapping(&s, &alice, h.clone())
        .expect("durable put_mapping");
    assert_eq!(written.pseudonym, h);

    let Some(app2) = app_provider().await else {
        return;
    };
    let store2 = pseudonym_store(&kms, &app2);
    let row = store2
        .mapping_of(&s, &alice)
        .expect("the S2 row survives a fresh engine over a fresh pool (kill-9-equivalent)");
    assert_eq!(row.pseudonym, h, "the public render survived");
    assert_eq!(
        store2.resolve(&s, &h).expect("resolve"),
        Some(alice.clone()),
        "the sealed real-identity link resolves after restart (KMS shared, ciphertext durable)"
    );

    let key_ref = store2
        .shred_key_for(&s, &alice)
        .expect("the subject has a per-subject shred key");
    let dek_id = DekId::new(key_ref.tenant.clone(), key_ref.class.clone());
    assert!(kms.destroy_dek(&dek_id), "the per-subject DEK is destroyed");

    let Some(app3) = app_provider().await else {
        return;
    };
    let store3 = pseudonym_store(&kms, &app3);
    assert!(
        store3.mapping_of(&s, &alice).is_some(),
        "the PUBLIC pseudonym row survives the crypto-shred across a restart (attribution intact)"
    );
    assert!(
        matches!(store3.resolve(&s, &h), Err(PseudonymError::Kms(_))),
        "a crypto-shredded resolve fails LOUD across restart, never plaintext-without-key"
    );
    assert!(
        store3.resolve_subject(&s, &alice).is_none(),
        "the subject is erased (0 recoverable real identity) after the crypto-shred + restart"
    );

    cleanup(&tenant, &region).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_erase_deletes_durably_and_the_ledger_drives_re_erasure_across_restart() {
    if migrate_admin().await.is_none() {
        return;
    }
    let Some(app1) = app_provider().await else {
        return;
    };
    let region = app1.config().region.clone();
    let suffix = uniq();
    let tenant = format!("mr009b-w6a-d8-{suffix}");
    let s = scope(&tenant, &region);
    let alice = PrincipalId("p:alice".into());
    let bob = PrincipalId("p:bob".into());

    let kms = Arc::new(KmsEngine::new());
    let cell = Arc::new(CellTokenAuthority::generate());
    let handle_a = handle("anon-a", &tenant);
    let handle_b = handle("anon-b", &tenant);

    let engine1 = StoreBackedCheck::with_pg(
        app1.clone(),
        kms.clone(),
        cell.clone(),
        tokio::runtime::Handle::current(),
    );
    engine1
        .pseudonyms()
        .put_mapping(&s, &alice, handle_a.clone())
        .expect("seed alice");
    engine1
        .pseudonyms()
        .put_mapping(&s, &bob, handle_b.clone())
        .expect("seed bob");

    engine1.erase_in(&s, &alice, at("2026-06-19T10:00:00Z"));
    engine1.erase_in(&s, &bob, at("2026-06-19T10:00:01Z"));
    assert!(
        engine1.pseudonyms().mapping_of(&s, &alice).is_none(),
        "the full erase DELETEd the durable map row (the resolvable mapping is gone)"
    );
    assert!(engine1.erasure_ledger().is_erased(&s, &alice));

    let Some(app2) = app_provider().await else {
        return;
    };
    let engine2 = StoreBackedCheck::with_pg(
        app2.clone(),
        kms.clone(),
        cell.clone(),
        tokio::runtime::Handle::current(),
    );
    assert!(
        engine2.pseudonyms().mapping_of(&s, &alice).is_none(),
        "the crypto-shred DELETE stays dead across a restart"
    );
    assert!(
        engine2.erasure_ledger().is_erased(&s, &alice)
            && engine2.erasure_ledger().is_erased(&s, &bob),
        "the PII-free erasure ledger survived the restart (it must, to drive re-erasure)"
    );

    engine2
        .pseudonyms()
        .put_mapping(&s, &alice, handle_a.clone())
        .expect("restore alice");
    engine2
        .pseudonyms()
        .put_mapping(&s, &bob, handle_b.clone())
        .expect("restore bob");
    assert!(
        engine2.pseudonyms().resolve_subject(&s, &alice).is_some(),
        "the restore resurrected alice"
    );

    let receipt = engine2
        .re_erase_after_restore(&s, at("2026-06-19T11:00:00Z"))
        .expect("re-erasure verification");
    assert_eq!(
        receipt.re_erased, 2,
        "the ledger drove re-erasure of BOTH subjects"
    );
    assert_eq!(
        receipt.pre_pass_resurrected, 2,
        "the restore resurrected both (the honest signal)"
    );
    assert_eq!(
        receipt.resurrected, 0,
        "0 resurrected AFTER the pass - the ID-D8 threshold"
    );
    assert!(
        receipt.is_green(),
        "the ID-D8 re-erasure drill is GREEN across a live-PG restart"
    );
    assert!(
        engine2.pseudonyms().resolve_subject(&s, &alice).is_none(),
        "alice re-erased (0 recoverable real identity)"
    );

    cleanup(&tenant, &region).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partition_isolation_and_idempotent_ledger_on_live_pg() {
    if migrate_admin().await.is_none() {
        return;
    }
    let Some(app) = app_provider().await else {
        return;
    };
    let region = app.config().region.clone();
    let suffix = uniq();
    let tenant_a = format!("mr009b-w6a-a-{suffix}");
    let tenant_b = format!("mr009b-w6a-b-{suffix}");
    let sa = scope(&tenant_a, &region);
    let sb = scope(&tenant_b, &region);
    let alice = PrincipalId("p:alice".into());

    let kms = Arc::new(KmsEngine::new());
    let store = pseudonym_store(&kms, &app);
    let ledger = erasure_ledger(&app);

    store
        .put_mapping(&sa, &alice, handle("anon-a", &tenant_a))
        .expect("acme write");
    ledger.record(
        &sa,
        &alice,
        KeyClass::Subject("p:alice".into()),
        at("2026-06-19T10:00:00Z"),
    );

    assert!(
        store.mapping_of(&sb, &alice).is_none(),
        "no cross-tenant map read: B cannot see A's mapping"
    );
    assert!(
        store
            .resolve(&sb, &handle("anon-a", &tenant_a))
            .expect("resolve")
            .is_none(),
        "no cross-tenant resolve"
    );
    assert!(
        store.mappings_in(&sb).is_empty(),
        "B's map partition is empty"
    );
    assert!(
        !ledger.is_erased(&sb, &alice),
        "no cross-tenant erasure-ledger read"
    );
    assert!(
        ledger.entries_in(&sb).is_empty(),
        "B's ledger partition is empty"
    );
    assert!(store.mapping_of(&sa, &alice).is_some());
    assert!(ledger.is_erased(&sa, &alice));

    ledger.record(
        &sa,
        &alice,
        KeyClass::Subject("p:alice".into()),
        at("2026-06-19T12:00:00Z"),
    );
    let Some(app2) = app_provider().await else {
        return;
    };
    let ledger2 = erasure_ledger(&app2);
    let entries = ledger2.entries_in(&sa);
    assert_eq!(
        entries.len(),
        1,
        "a re-record does not duplicate (idempotent upsert)"
    );
    assert_eq!(
        entries[0].erased_at,
        at("2026-06-19T12:00:00Z"),
        "the timestamp updated on the re-record (survives restart)"
    );

    cleanup(&tenant_a, &region).await;
    cleanup(&tenant_b, &region).await;
}
