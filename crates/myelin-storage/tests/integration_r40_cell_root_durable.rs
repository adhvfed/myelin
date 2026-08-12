#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_storage::cell_root_durable::{cell_root_durable_migrations, CellRootError};
use myelin_storage::migration::HotTables;
use myelin_storage::{DurableCellRootBacking, SealKey, SubstrateProvider};

const SEAL_K_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const SEAL_WRONG_HEX: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

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
        .migrate(&cell_root_durable_migrations(), &HotTables::none())
        .await
        .expect("apply the cell-root migration (cell_token_root)");
    Some(provider)
}

async fn fresh_pool() -> sqlx::postgres::PgPool {
    SubstrateProvider::connect(admin_config(&MyelinConfig::dev()), 2)
        .await
        .expect("fresh pool")
        .db_pool()
        .clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_or_generate_roundtrips_the_same_root_across_a_fresh_backing() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let cell = format!("cell-{}", uniq());
    let seal = seal_k();

    let backing1 = DurableCellRootBacking::new(provider.db_pool().clone(), &cell);
    let m1 = backing1
        .load_or_generate(&seal)
        .await
        .expect("backing #1 boots (first boot generates + seals the root)");

    let backing2 = DurableCellRootBacking::new(fresh_pool().await, &cell);
    let m2 = backing2
        .load_or_generate(&seal)
        .await
        .expect("backing #2 boots: unseals the SAME root");
    assert_eq!(
        m1.ed25519_seed, m2.ed25519_seed,
        "the Ed25519 signing seed is stable across a restart (the trust anchor does not change)"
    );
    assert_eq!(
        m1.mac_key, m2.mac_key,
        "the macaroon MAC key is stable across a restart"
    );

    cleanup(provider.db_pool(), &cell).await;
    println!(
        "OK [1]: load_or_generate roundtrips the SAME cell-authority root across a fresh backing."
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_seal_key_fails_closed_and_never_generates_a_new_root() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let cell = format!("cell-{}", uniq());
    let seal = seal_k();

    let backing = DurableCellRootBacking::new(provider.db_pool().clone(), &cell);
    let original = backing
        .load_or_generate(&seal)
        .await
        .expect("first boot under K");

    let wrong = SealKey::from_encoded(SEAL_WRONG_HEX).expect("wrong key");
    let wrong_backing = DurableCellRootBacking::new(fresh_pool().await, &cell);
    match wrong_backing.load_or_generate(&wrong).await {
        Err(CellRootError::WrongSealKey { cell_id }) => {
            assert_eq!(cell_id, cell, "the loud error names the cell");
        }
        Err(other) => panic!("expected WrongSealKey, got a different error: {other}"),
        Ok(_) => panic!(
            "FAIL-OPEN: a wrong seal key must NOT boot (it must NOT generate a new root that orphans \
             every token minted under the old root)"
        ),
    }

    let root_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cell_token_root WHERE cell_id = $1")
            .bind(&cell)
            .fetch_one(provider.db_pool())
            .await
            .expect("count roots");
    assert_eq!(
        root_rows, 1,
        "the wrong-key boot did NOT mint a second root"
    );

    let recovered = DurableCellRootBacking::new(fresh_pool().await, &cell)
        .load_or_generate(&seal)
        .await
        .expect("the ORIGINAL root still unseals under the correct key K");
    assert_eq!(
        recovered.ed25519_seed, original.ed25519_seed,
        "the original root is untouched (same seed)"
    );
    assert_eq!(recovered.mac_key, original.mac_key, "same MAC key");

    cleanup(provider.db_pool(), &cell).await;
    println!("OK [2]: a wrong seal key fails closed + loud, never generates a new root; K still recovers.");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_boot_produces_a_single_root() {
    let Some(provider) = admin_provider().await else {
        return;
    };
    let cell = format!("cell-{}", uniq());
    let seal = seal_k();

    let mut handles = Vec::new();
    for _ in 0..6 {
        let cell = cell.clone();
        let seal = seal.clone();
        handles.push(tokio::spawn(async move {
            let backing = DurableCellRootBacking::new(fresh_pool().await, &cell);
            backing
                .load_or_generate(&seal)
                .await
                .expect("concurrent first boot")
        }));
    }
    let mut materials = Vec::new();
    for h in handles {
        materials.push(h.await.expect("join concurrent boot"));
    }
    let first = &materials[0];
    for (i, m) in materials.iter().enumerate() {
        assert_eq!(
            m.ed25519_seed, first.ed25519_seed,
            "concurrent boot #{i} adopted a DIFFERENT root seed (two roots - the race was not resolved)"
        );
        assert_eq!(
            m.mac_key, first.mac_key,
            "concurrent boot #{i} adopted a different MAC key"
        );
    }

    let root_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cell_token_root WHERE cell_id = $1")
            .bind(&cell)
            .fetch_one(provider.db_pool())
            .await
            .expect("count roots");
    assert_eq!(
        root_rows, 1,
        "a concurrent first boot produced EXACTLY one root row"
    );

    cleanup(provider.db_pool(), &cell).await;
    println!(
        "OK [3]: a concurrent first boot converges on a single cell-authority root (never two)."
    );
}

async fn cleanup(pool: &sqlx::postgres::PgPool, cell: &str) {
    let _ = sqlx::query("DELETE FROM cell_token_root WHERE cell_id = $1")
        .bind(cell)
        .execute(pool)
        .await;
}
