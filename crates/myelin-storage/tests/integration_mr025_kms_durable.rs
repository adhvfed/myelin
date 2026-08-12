#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::kms_durable::{kms_durable_migrations, DurableKmsBacking, KmsDurableError};
use myelin_storage::migration::HotTables;
use myelin_storage::{KekId, KeyClass, SealKey, SubstrateProvider};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

const SEAL_K_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const SEAL_WRONG_HEX: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

fn test_config() -> MyelinConfig {
    let mut config = MyelinConfig::dev();
    if let Ok(database_url) = std::env::var("MYELIN_TEST_DATABASE_URL") {
        if !database_url.trim().is_empty() {
            config.database_url = database_url;
        }
    }
    config
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

fn seal_k() -> SealKey {
    SealKey::from_encoded(SEAL_K_HEX).expect("valid 32-byte hex seal key K")
}

async fn admin_provider() -> SubstrateProvider {
    let cfg = admin_config(&test_config());
    let provider = SubstrateProvider::connect(cfg, 6)
        .await
        .expect("connect to the Postgres required by durable KMS integration tests");
    provider
        .migrate(&kms_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the KMS migrations (kms_sealed_root + kms_wrapped_kek + kms_wrapped_dek)");
    provider
}

async fn fresh_pool() -> sqlx::postgres::PgPool {
    SubstrateProvider::connect(admin_config(&test_config()), 2)
        .await
        .expect("fresh pool")
        .db_pool()
        .clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dek_provisioned_before_restart_decrypts_after_via_a_fresh_engine() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0KMS{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    let backing1 = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing1
        .load_or_generate(&seal)
        .await
        .expect("engine #1 boots (first boot generates + seals the root)");
    backing1
        .ensure_kek(&engine1, &KekId::new(tenant.clone(), region.clone()))
        .await
        .expect("ensure + persist KEK");
    let key_ref = backing1
        .ensure_dek(&engine1, &tenant, &region, KeyClass::Tenant)
        .await
        .expect("ensure + persist DEK");
    let dek = engine1.resolve_dek(&key_ref, &region).expect("resolve DEK");
    let (nonce, ct) = dek.seal(b"a personal-data column value");

    let backing2 = DurableKmsBacking::new(fresh_pool().await, &cell);
    let engine2 = backing2
        .load_or_generate(&seal)
        .await
        .expect("engine #2 boots: unseals the SAME root + loads the wrapped KEKs/DEKs");
    let dek2 = engine2
        .resolve_dek(&key_ref, &region)
        .expect("the DEK resolves under engine #2 (root + KEK + DEK are durable)");
    let plain = dek2
        .open(&nonce, &ct)
        .expect("the pre-restart ciphertext DECRYPTS after a fresh-engine restart");
    assert_eq!(plain, b"a personal-data column value");

    cleanup(provider.db_pool(), &cell).await;
    println!("OK [1]: a DEK provisioned before restart decrypts via a FRESH engine (durable root+KEK+DEK).");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_seal_key_fails_closed_and_never_generates_a_new_root() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0WRG{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    let backing = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing
        .load_or_generate(&seal)
        .await
        .expect("first boot under K");
    backing
        .ensure_kek(&engine1, &KekId::new(tenant.clone(), region.clone()))
        .await
        .expect("kek");
    let key_ref = backing
        .ensure_dek(&engine1, &tenant, &region, KeyClass::Tenant)
        .await
        .expect("dek");
    let (nonce, ct) = engine1
        .resolve_dek(&key_ref, &region)
        .expect("resolve")
        .seal(b"secret under K");

    let wrong = SealKey::from_encoded(SEAL_WRONG_HEX).expect("wrong key");
    let wrong_backing = DurableKmsBacking::new(fresh_pool().await, &cell);
    let result = wrong_backing.load_or_generate(&wrong).await;
    match result {
        Err(KmsDurableError::WrongSealKey { cell_id }) => {
            assert_eq!(cell_id, cell, "the loud error names the cell");
        }
        Err(other) => panic!("expected WrongSealKey, got a different error: {other}"),
        Ok(_) => panic!(
            "FAIL-OPEN: a wrong seal key must NOT boot the engine (it must NOT generate a new root \
             that orphans existing ciphertext)"
        ),
    }

    let root_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kms_sealed_root WHERE cell_id = $1")
            .bind(&cell)
            .fetch_one(provider.db_pool())
            .await
            .expect("count sealed roots");
    assert_eq!(
        root_rows, 1,
        "the wrong-key boot did NOT mint a second root"
    );

    let engine_ok = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("the ORIGINAL root still unseals under the correct key K");
    let plain = engine_ok
        .resolve_dek(&key_ref, &region)
        .expect("resolve under K")
        .open(&nonce, &ct)
        .expect("the original ciphertext STILL decrypts under K (the root was untouched)");
    assert_eq!(plain, b"secret under K");

    cleanup(provider.db_pool(), &cell).await;
    println!("OK [2]: a wrong seal key fails closed + loud, never generates a new root; K still decrypts.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_snapshot_restore_recovers_data_and_keeps_a_shredded_key_dead() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0BAK{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    let backing = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing.load_or_generate(&seal).await.expect("first boot");
    backing
        .ensure_kek(&engine1, &KekId::new(tenant.clone(), region.clone()))
        .await
        .expect("kek");
    let live_ref = backing
        .ensure_dek(&engine1, &tenant, &region, KeyClass::Tenant)
        .await
        .expect("tenant dek");
    let doomed_ref = backing
        .ensure_dek(
            &engine1,
            &tenant,
            &region,
            KeyClass::Subject("u-doomed".into()),
        )
        .await
        .expect("subject dek");
    let (nonce, ct) = engine1
        .resolve_dek(&live_ref, &region)
        .expect("resolve")
        .seal(b"value to recover via restore");

    backing
        .destroy_dek(
            &engine1,
            &myelin_storage::DekId::new(tenant.clone(), KeyClass::Subject("u-doomed".into())),
        )
        .await
        .expect("crypto-shred the subject DEK");
    let snapshot = engine1.backup_snapshot_durable(&seal).unwrap();
    assert!(
        !snapshot
            .deks
            .iter()
            .any(|(id, ..)| id.class == KeyClass::Subject("u-doomed".into())),
        "a crypto-shredded DEK is EXCLUDED from the snapshot"
    );

    cleanup(provider.db_pool(), &cell).await;
    let after_loss = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await;
    let empty = after_loss.expect("a cleared store is a genuine empty first boot (new root)");
    assert!(
        empty.resolve_dek(&live_ref, &region).is_err(),
        "after data loss the original DEK is unrecoverable (the cleared store has no KEK/DEK)"
    );

    let restore_backing = DurableKmsBacking::new(fresh_pool().await, &cell);
    restore_backing
        .restore(&snapshot)
        .await
        .expect("restore the snapshot into the cleared store");
    let recovered = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("a fresh engine boots over the restored store");
    let plain = recovered
        .resolve_dek(&live_ref, &region)
        .expect("the surviving DEK resolves after restore")
        .open(&nonce, &ct)
        .expect("and DECRYPTS the original ciphertext (restore recovered the data)");
    assert_eq!(plain, b"value to recover via restore");
    assert!(
        recovered.resolve_dek(&doomed_ref, &region).is_err(),
        "a crypto-shredded DEK stays unrecoverable after restore (§7.5)"
    );

    cleanup(provider.db_pool(), &cell).await;
    println!("OK [3]: snapshot→restore recovers the data; a crypto-shredded DEK stays dead after restore.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keys_minted_through_the_default_sync_api_survive_engine_reconstruction() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0W5D{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    let backing1 = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing1
        .load_or_generate(&seal)
        .await
        .expect("production boot: load_or_generate");

    engine1
        .ensure_kek(&KekId::new(tenant.clone(), region.clone()))
        .expect("tenant KEK via the plain sync API");
    let live_ref = engine1
        .ensure_dek(&tenant, &region, KeyClass::Tenant)
        .expect("tenant DEK via the plain sync API");
    let doomed_ref = engine1
        .ensure_dek(&tenant, &region, KeyClass::Subject("u-doomed".into()))
        .expect("subject DEK via the plain sync API");
    let (nonce, ct) = engine1
        .resolve_dek(&live_ref, &region)
        .expect("resolve")
        .seal(b"minted through the default path");

    assert!(
        engine1
            .destroy_dek(&myelin_storage::DekId::new(
                tenant.clone(),
                KeyClass::Subject("u-doomed".into()),
            ))
            .unwrap(),
        "the subject DEK was present to destroy"
    );

    drop(engine1);
    drop(backing1);
    let engine2 = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("engine #2 re-constructs from the same pool/store");
    let plain = engine2
        .resolve_dek(&live_ref, &region)
        .expect("the sync-API-minted DEK survives re-construction (the write-through is real)")
        .open(&nonce, &ct)
        .expect("and decrypts the pre-restart ciphertext");
    assert_eq!(plain, b"minted through the default path");
    assert!(
        engine2.resolve_dek(&doomed_ref, &region).is_err(),
        "the sync-API crypto-shred deleted the durable row (stays dead across restart, §7.5)"
    );

    cleanup(provider.db_pool(), &cell).await;
    println!(
        "OK [4]: keys minted via the DEFAULT sync engine API survive engine re-construction; a \
         sync-API shred stays dead (Wave 5 write-through)."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_processes_observe_key_creation_and_crypto_shred_without_restart() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0LIVE{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    let reader = DurableKmsBacking::new(provider.db_pool().clone(), &cell)
        .load_or_generate(&seal)
        .await
        .expect("the long-running reader boots before the subject key exists");
    let writer = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("a second process shares the sealed cell root");
    writer
        .ensure_kek(&KekId::new(tenant.clone(), region.clone()))
        .expect("the writer publishes the tenant KEK");
    let key_ref = writer
        .ensure_dek(&tenant, &region, KeyClass::Subject("u-live".into()))
        .expect("the writer creates and durably publishes the subject key");
    let (nonce, ciphertext) = writer
        .resolve_dek(&key_ref, &region)
        .expect("the writer resolves its key")
        .seal(b"visible only while the subject key is live");

    assert_eq!(
        reader
            .resolve_dek(&key_ref, &region)
            .expect("an already-running reader discovers the new durable key")
            .open(&nonce, &ciphertext)
            .expect("the reader decrypts through the shared key"),
        b"visible only while the subject key is live"
    );

    assert!(writer
        .destroy_dek(&myelin_storage::DekId::new(
            tenant.clone(),
            KeyClass::Subject("u-live".into()),
        ))
        .unwrap());
    assert!(
        reader.resolve_dek(&key_ref, &region).is_err(),
        "a reader that cached the key still observes the durable crypto-shred without restarting"
    );

    cleanup(provider.db_pool(), &cell).await;
    println!(
        "OK [6]: live processes discover durable key creation and immediately stop resolving a \
         crypto-shredded subject key."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_processes_converge_on_one_new_subject_key() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0RACE{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();
    let first = Arc::new(
        DurableKmsBacking::new(provider.db_pool().clone(), &cell)
            .load_or_generate(&seal)
            .await
            .expect("the first process boots before the key exists"),
    );
    let second = Arc::new(
        DurableKmsBacking::new(fresh_pool().await, &cell)
            .load_or_generate(&seal)
            .await
            .expect("the second process boots before the key exists"),
    );

    let provision = |engine: Arc<myelin_storage::KmsEngine>| {
        let tenant = tenant.clone();
        let region = region.clone();
        tokio::task::spawn_blocking(move || {
            engine
                .ensure_dek(&tenant, &region, KeyClass::Subject("u-race".into()))
                .expect("concurrent provisioning converges")
        })
    };
    let (first_ref, second_ref) = tokio::join!(provision(first.clone()), provision(second.clone()));
    let first_ref = first_ref.expect("first provisioning task joins");
    let second_ref = second_ref.expect("second provisioning task joins");
    assert_eq!(
        first_ref, second_ref,
        "both processes publish the same key ref"
    );

    let (nonce, ciphertext) = first
        .resolve_dek(&first_ref, &region)
        .expect("the first process resolves the winning key")
        .seal(b"one winner, no split-brain ciphertext");
    assert_eq!(
        second
            .resolve_dek(&second_ref, &region)
            .expect("the second process resolves the same winning key")
            .open(&nonce, &ciphertext)
            .expect("ciphertext crosses the process boundary"),
        b"one winner, no split-brain ciphertext"
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM kms_wrapped_dek \
          WHERE cell_id = $1 AND tenant_id = $2 AND class = 'subject:u-race'",
    )
    .bind(&cell)
    .bind(tenant.as_str())
    .fetch_one(provider.db_pool())
    .await
    .expect("count the canonical subject key");
    assert_eq!(rows, 1, "one durable key wins concurrent provisioning");

    cleanup(provider.db_pool(), &cell).await;
    println!("OK [7]: concurrent processes converge on one durable subject key.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_process_cannot_rotate_a_shredded_subject_key_back_into_the_database() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0SHRED{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();
    let stale = DurableKmsBacking::new(provider.db_pool().clone(), &cell)
        .load_or_generate(&seal)
        .await
        .expect("the soon-to-be-stale process boots");
    let doomed_ref = stale
        .ensure_dek(&tenant, &region, KeyClass::Subject("u-shredded".into()))
        .expect("the subject key exists in the stale process cache");
    let shredder = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("a second process sees the same durable keys");
    assert!(shredder
        .destroy_dek(&myelin_storage::DekId::new(
            tenant.clone(),
            KeyClass::Subject("u-shredded".into()),
        ))
        .unwrap());

    stale
        .rotate_kek(&KekId::new(tenant.clone(), region.clone()))
        .expect("rotation refreshes durable membership before rewrapping keys");
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM kms_wrapped_dek \
          WHERE cell_id = $1 AND tenant_id = $2 AND class = 'subject:u-shredded'",
    )
    .bind(&cell)
    .bind(tenant.as_str())
    .fetch_one(provider.db_pool())
    .await
    .expect("count the shredded durable key rows");
    assert_eq!(rows, 0, "rotation never UPSERTs a deleted subject key");
    assert!(
        stale
            .backup_snapshot_durable(&seal)
            .unwrap()
            .deks
            .iter()
            .all(|(id, ..)| id.class != KeyClass::Subject("u-shredded".into())),
        "a snapshot reads durable membership, not the process's stale cache",
    );
    assert!(
        stale
            .backup_snapshot()
            .unwrap()
            .iter()
            .all(|(id, _)| id.class != KeyClass::Subject("u-shredded".into())),
        "the application backup path also reads durable membership",
    );
    let restarted = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("the post-rotation process restarts");
    assert!(
        restarted.resolve_dek(&doomed_ref, &region).is_err(),
        "the shredded ciphertext remains unrecoverable after rotation and restart",
    );

    cleanup(provider.db_pool(), &cell).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotation_persists_kek_and_all_rewrapped_deks_atomically_and_survives_restart() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0W5R{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    let backing1 = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing1
        .load_or_generate(&seal)
        .await
        .expect("production boot: load_or_generate");
    let kek_id = KekId::new(tenant.clone(), region.clone());
    let mint_epoch = engine1.ensure_kek(&kek_id).expect("mint tenant KEK");
    let tenant_ref = engine1
        .ensure_dek(&tenant, &region, KeyClass::Tenant)
        .expect("tenant DEK");
    let subj_a = engine1
        .ensure_dek(&tenant, &region, KeyClass::Subject("u-a".into()))
        .expect("subject DEK a");
    let subj_b = engine1
        .ensure_dek(&tenant, &region, KeyClass::Subject("u-b".into()))
        .expect("subject DEK b");
    let (nonce, ct) = engine1
        .resolve_dek(&tenant_ref, &region)
        .expect("resolve pre-rotation")
        .seal(b"sealed before rotation");
    let new_epoch = engine1
        .rotate_kek(&kek_id)
        .expect("rotation (durable path)");
    assert!(new_epoch > mint_epoch, "rotation bumped the KEK epoch");

    let kek_epoch: i64 = sqlx::query_scalar(
        "SELECT epoch FROM kms_wrapped_kek WHERE cell_id = $1 AND tenant_id = $2",
    )
    .bind(&cell)
    .bind(tenant.0.as_str())
    .fetch_one(provider.db_pool())
    .await
    .expect("KEK row present");
    let dek_epochs: Vec<i64> = sqlx::query_scalar(
        "SELECT kek_epoch FROM kms_wrapped_dek WHERE cell_id = $1 AND tenant_id = $2",
    )
    .bind(&cell)
    .bind(tenant.0.as_str())
    .fetch_all(provider.db_pool())
    .await
    .expect("DEK rows present");
    assert_eq!(dek_epochs.len(), 3, "all three DEK rows persisted");
    assert!(
        dek_epochs.iter().all(|e| *e == kek_epoch),
        "every persisted DEK envelope is wrapped under the persisted KEK epoch \
         (rotation rows are consistent - the one-transaction persist)"
    );

    drop(engine1);
    drop(backing1);
    let engine2 = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("engine #2 re-constructs post-rotation");
    let plain = engine2
        .resolve_dek(&tenant_ref, &region)
        .expect("pre-rotation ref resolves post-rotation post-restart")
        .open(&nonce, &ct)
        .expect("pre-rotation ciphertext decrypts after rotation + restart");
    assert_eq!(plain, b"sealed before rotation");
    for r in [&subj_a, &subj_b] {
        engine2
            .resolve_dek(r, &region)
            .expect("every re-wrapped subject DEK survives rotation + restart");
    }

    cleanup(provider.db_pool(), &cell).await;
    println!(
        "OK [5]: rotation persists the KEK + all re-wrapped DEK rows consistently (one PG tx) and \
         everything decrypts after restart."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_database_outage_refuses_new_keys_without_taking_the_worker_down() {
    let provider = admin_provider().await;
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0DOWN{suffix}"));
    let region = Region("eu-west".into());

    let backing = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine = backing
        .load_or_generate(&seal_k())
        .await
        .expect("the worker boots while its durable key store is healthy");
    backing.close_pool_for_test().await;

    let refusal = engine.ensure_dek(&tenant, &region, KeyClass::Tenant);
    assert!(
        matches!(refusal, Err(myelin_storage::KmsError::Durability(_))),
        "an unavailable key store is a controlled refusal, never a worker panic: {refusal:?}"
    );

    cleanup(provider.db_pool(), &cell).await;
    println!("OK [8]: a database outage refuses key provisioning and leaves the worker alive.");
}

async fn cleanup(pool: &sqlx::postgres::PgPool, cell: &str) {
    for table in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
        let _ = sqlx::query(&format!("DELETE FROM {table} WHERE cell_id = $1"))
            .bind(cell)
            .execute(pool)
            .await;
    }
}
