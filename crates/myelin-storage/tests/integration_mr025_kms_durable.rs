//! # MR-025 — durable software-sealed KMS cell root + KEK/DEK persistence, proven against LIVE Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!   MYELIN_KMS_SEAL_KEY=00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff \
//!     cargo test -p myelin-storage --features integration \
//!       --test integration_mr025_kms_durable -- --nocapture
//!
//! It proves the MR-025 deliverables — each MUST hit the live DB (a pass on the in-memory engine
//! would NOT count — that engine mints a fresh root per process and proves nothing about durability):
//!   1. **Decrypt-across-restart (the core proof):** engine #1 (`load_or_generate`, durable store,
//!      seal key K) provisions a KEK + DEK and seals ciphertext; a FRESH engine #2 (`load_or_generate`,
//!      SAME store, SAME seal key K, FRESH pool) unwraps the KEK + DEK and DECRYPTS the ciphertext.
//!   2. **Wrong seal key fails closed:** engine #3 (same store, DIFFERENT seal key) FAILS to unseal
//!      the root (loud `WrongSealKey`), does NOT silently generate a new root, and does NOT decrypt;
//!      the original root is untouched (engine #1's ciphertext still decrypts under K).
//!   3. **backup_snapshot → restore recovers data:** a snapshot from engine #1 restored into a
//!      cleared store lets a fresh engine (same seal key) decrypt the original ciphertext again; a
//!      crypto-shredded DEK is absent from the snapshot and stays unrecoverable after restore.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::kms_durable::{kms_durable_migrations, DurableKmsBacking, KmsDurableError};
use myelin_storage::migration::HotTables;
use myelin_storage::{KeyClass, KekId, SealKey, SubstrateProvider};
use myelin_tenancy::{Region, TenantId};

/// A fixed, deterministic test seal key (32 bytes as 64 hex chars) — used as the CORRECT key K across
/// every test so the per-cell sealed root is consistent. Exercises `SealKey::from_encoded` (the env
/// decode path). Production supplies this via `MYELIN_KMS_SEAL_KEY`.
const SEAL_K_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
/// A DIFFERENT seal key — the WRONG key for the fail-closed proof.
const SEAL_WRONG_HEX: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

/// DDL (CREATE TABLE) runs as the migration/owner (admin) role — PG16 revokes CREATE on `public` for
/// the app role. The KMS tables carry NO RLS (cell-infra key material, cross-tenant by design), so
/// DML is role-agnostic; this test drives both DDL + DML through the admin pool.
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

fn seal_k() -> SealKey {
    SealKey::from_encoded(SEAL_K_HEX).expect("valid 32-byte hex seal key K")
}

/// Build an admin-role provider, applying the KMS migrations once; `None` (SKIP) if unreachable.
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
        .migrate(&kms_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the KMS migrations (kms_sealed_root + kms_wrapped_kek + kms_wrapped_dek)");
    Some(provider)
}

fn fresh_pool() -> impl std::future::Future<Output = sqlx::postgres::PgPool> {
    async {
        SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2)
            .await
            .expect("fresh pool")
            .db_pool()
            .clone()
    }
}

// =================================================================================================
// 1 — Decrypt-across-restart (THE core proof): a DEK provisioned by engine #1 decrypts under a FRESH
//     engine #2 over the SAME store + SAME seal key (impossible with the fresh-root-per-process today).
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dek_provisioned_before_restart_decrypts_after_via_a_fresh_engine() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0KMS{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    // Engine #1: load_or_generate (generates + persists the sealed root on this empty first boot).
    let backing1 = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing1
        .load_or_generate(&seal)
        .await
        .expect("engine #1 boots (first boot generates + seals the root)");
    // Provision a KEK + DEK through the write-through helpers (durable from the moment they are minted).
    backing1
        .ensure_kek(&engine1, &KekId::new(tenant.clone(), region.clone()))
        .await
        .expect("ensure + persist KEK");
    let key_ref = backing1
        .ensure_dek(&engine1, &tenant, &region, KeyClass::Tenant)
        .await
        .expect("ensure + persist DEK");
    // Seal some ciphertext under the resolved DEK.
    let dek = engine1.resolve_dek(&key_ref, &region).expect("resolve DEK");
    let (nonce, ct) = dek.seal(b"a personal-data column value");

    // Engine #2: a FRESH engine over a FRESH pool (new connections — proves it is in Postgres, not
    // process memory), SAME store (cell), SAME seal key K.
    let backing2 = DurableKmsBacking::new(fresh_pool().await, &cell);
    let engine2 = backing2
        .load_or_generate(&seal)
        .await
        .expect("engine #2 boots: unseals the SAME root + loads the wrapped KEKs/DEKs");
    // Unwrap the KEK + DEK (walks L0 root → L1 KEK → L2 DEK) and DECRYPT — impossible if the root
    // were freshly generated per process.
    let dek2 = engine2
        .resolve_dek(&key_ref, &region)
        .expect("the DEK resolves under engine #2 (root + KEK + DEK are durable)");
    let plain = dek2
        .open(&nonce, &ct)
        .expect("the pre-restart ciphertext DECRYPTS after a fresh-engine restart");
    assert_eq!(plain, b"a personal-data column value");

    // cleanup.
    cleanup(provider.db_pool(), &cell).await;
    println!("OK [1]: a DEK provisioned before restart decrypts via a FRESH engine (durable root+KEK+DEK).");
}

// =================================================================================================
// 2 — Wrong seal key FAILS CLOSED: a different seal key cannot unseal the root, does NOT generate a
//     new one, does NOT decrypt; the original root is untouched.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_seal_key_fails_closed_and_never_generates_a_new_root() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let suffix = uniq();
    let cell = format!("cell-{suffix}");
    let tenant = TenantId(format!("01J0WRG{suffix}"));
    let region = Region("eu-west".into());
    let seal = seal_k();

    // Establish a root + a sealed ciphertext under the CORRECT key K.
    let backing = DurableKmsBacking::new(provider.db_pool().clone(), &cell);
    let engine1 = backing.load_or_generate(&seal).await.expect("first boot under K");
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

    // Attempt to boot with a WRONG seal key over the SAME store: must FAIL CLOSED + LOUD.
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

    // The wrong-key attempt did NOT overwrite the root: there is still EXACTLY ONE sealed_root row
    // for this cell, and it still unseals under K (engine #1's ciphertext still decrypts).
    let root_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kms_sealed_root WHERE cell_id = $1")
            .bind(&cell)
            .fetch_one(provider.db_pool())
            .await
            .expect("count sealed roots");
    assert_eq!(root_rows, 1, "the wrong-key boot did NOT mint a second root");

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

    // cleanup.
    cleanup(provider.db_pool(), &cell).await;
    println!("OK [2]: a wrong seal key fails closed + loud, never generates a new root; K still decrypts.");
}

// =================================================================================================
// 3 — backup_snapshot → restore recovers data; a crypto-shredded DEK stays dead across the restore.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_snapshot_restore_recovers_data_and_keeps_a_shredded_key_dead() {
    let Some(provider) = admin_provider().await else {
        return;
    };
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
    // A surviving tenant DEK + a doomed per-subject DEK (the GD-4 individual-erasure lever).
    let live_ref = backing
        .ensure_dek(&engine1, &tenant, &region, KeyClass::Tenant)
        .await
        .expect("tenant dek");
    let doomed_ref = backing
        .ensure_dek(&engine1, &tenant, &region, KeyClass::Subject("u-doomed".into()))
        .await
        .expect("subject dek");
    let (nonce, ct) = engine1
        .resolve_dek(&live_ref, &region)
        .expect("resolve")
        .seal(b"value to recover via restore");

    // Crypto-shred the doomed subject DEK (reaches the durable store), THEN snapshot.
    backing
        .destroy_dek(
            &engine1,
            &myelin_storage::DekId::new(tenant.clone(), KeyClass::Subject("u-doomed".into())),
        )
        .await
        .expect("crypto-shred the subject DEK");
    let snapshot = engine1.backup_snapshot_durable(&seal);
    assert!(
        !snapshot
            .deks
            .iter()
            .any(|(id, ..)| id.class == KeyClass::Subject("u-doomed".into())),
        "a crypto-shredded DEK is EXCLUDED from the snapshot"
    );

    // Simulate DATA LOSS: clear this cell's durable KMS rows entirely.
    cleanup(provider.db_pool(), &cell).await;
    // A fresh engine over the cleared store can no longer boot the cell (no root) — prove the loss.
    let after_loss = DurableKmsBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await;
    // With the root gone, load_or_generate would GENERATE a new (empty) root — so the DEK is gone.
    let empty = after_loss.expect("a cleared store is a genuine empty first boot (new root)");
    assert!(
        empty.resolve_dek(&live_ref, &region).is_err(),
        "after data loss the original DEK is unrecoverable (the cleared store has no KEK/DEK)"
    );

    // RESTORE the snapshot into a clean store, then a FRESH engine (same seal key) recovers the data.
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
    // The crypto-shredded subject DEK stays DEAD across the restore (it was never in the snapshot).
    assert!(
        recovered.resolve_dek(&doomed_ref, &region).is_err(),
        "a crypto-shredded DEK stays unrecoverable after restore (§7.5)"
    );

    // cleanup.
    cleanup(provider.db_pool(), &cell).await;
    println!("OK [3]: snapshot→restore recovers the data; a crypto-shredded DEK stays dead after restore.");
}

/// Remove every durable KMS row for a test cell (the sealed root + all wrapped KEKs/DEKs).
async fn cleanup(pool: &sqlx::postgres::PgPool, cell: &str) {
    for table in ["kms_wrapped_dek", "kms_wrapped_kek", "kms_sealed_root"] {
        let _ = sqlx::query(&format!("DELETE FROM {table} WHERE cell_id = $1"))
            .bind(cell)
            .execute(pool)
            .await;
    }
}
