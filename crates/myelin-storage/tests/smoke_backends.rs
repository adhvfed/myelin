//! Stage 2 smoke integration tests — the REAL storage/cache backends round-tripped THROUGH the
//! frozen traits (not the raw SDK).
//!
//! Distinct from the Stage 1 `integration_backends.rs` / `integration_cache.rs` (which prove the
//! raw sqlx / aws-sdk-s3 / fred SDKs are reachable). Here we drive:
//!   - [`S3BlobStore`] entirely through the [`BlobStore`] trait (put/get/head/delete + the
//!     re-hash-on-read integrity gate);
//!   - [`ValkeyCache`] entirely through the [`Cache`] trait (set/get/expire/delete);
//!   - [`PgStore`] — the OLTP client + the outbox table/relay (real FOR UPDATE SKIP LOCKED) +
//!     the `(tenant, region)`-RLS ReBAC tuple store — against real Postgres.
//!
//! Run against the docker-compose dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test smoke_backends -- --nocapture
#![cfg(feature = "integration")]

use std::time::Duration;

use myelin_config::MyelinConfig;
use myelin_storage::blob::{BlobError, BlobStore, ContentHash};
use myelin_storage::cache::Cache;
use myelin_storage::pg::PgStore;
use myelin_storage::s3blob::S3BlobStore;
use myelin_storage::valkey::ValkeyCache;
use myelin_tenancy::TenantId;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, InProcessBus, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::Region;

mod common;

// ---- S3BlobStore: the BlobStore trait, real RustFS ----------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn s3_blobstore_put_get_head_delete() {
    let cfg = MyelinConfig::dev();
    let store = S3BlobStore::connect(&cfg.s3, tokio::runtime::Handle::current());
    let tenant = TenantId(format!("acme-{}", std::process::id()));
    let bytes = b"myelin-stage2-blob-through-the-trait".to_vec();

    // put → content address (BLAKE3 of plaintext).
    let addr = tokio::task::block_in_place(|| store.put(&tenant, &bytes)).expect("put");
    assert_eq!(
        addr,
        ContentHash::blake3(&bytes),
        "address is the plaintext BLAKE3 hash"
    );

    // get → exact bytes back (re-hash-on-read passed).
    let got = tokio::task::block_in_place(|| store.get(&tenant, &addr)).expect("get");
    assert_eq!(got, bytes);

    // head → stored length, no bytes.
    let meta = tokio::task::block_in_place(|| store.head(&tenant, &addr)).expect("head");
    assert_eq!(meta.stored_len, bytes.len());
    assert_eq!(meta.hash, addr);

    // delete → then get is a NotFound (the object is gone).
    tokio::task::block_in_place(|| store.delete(&tenant, &addr)).expect("delete");
    let after = tokio::task::block_in_place(|| store.get(&tenant, &addr));
    assert!(
        matches!(after, Err(BlobError::NotFound { .. })),
        "get after delete must be NotFound, got {after:?}"
    );
}

// ---- ValkeyCache: the Cache trait, real Valkey ---------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valkey_cache_set_get_expire_delete() {
    let cfg = MyelinConfig::dev();
    let cache = ValkeyCache::connect(&cfg.redis_url, tokio::runtime::Handle::current())
        .expect("connect Valkey (is the stack up?)");
    let tenant = TenantId(format!("acme-{}", std::process::id()));
    let key = "session-token";

    // set with a TTL, then get hits.
    tokio::task::block_in_place(|| cache.set(&tenant, key, b"v1", Duration::from_secs(60)))
        .expect("set");
    let got = tokio::task::block_in_place(|| cache.get(&tenant, key)).expect("get");
    assert_eq!(got, Some(b"v1".to_vec()));

    // a DIFFERENT tenant must MISS the same key (per-tenant namespacing).
    let other = TenantId(format!("globex-{}", std::process::id()));
    let cross = tokio::task::block_in_place(|| cache.get(&other, key)).expect("cross get");
    assert_eq!(
        cross, None,
        "another tenant must not read this tenant's cached value"
    );

    // expire: a short TTL set, then a miss after it lapses.
    tokio::task::block_in_place(|| cache.set(&tenant, "ephem", b"x", Duration::from_secs(1)))
        .expect("set short");
    std::thread::sleep(Duration::from_millis(1500));
    let expired = tokio::task::block_in_place(|| cache.get(&tenant, "ephem")).expect("get expired");
    assert_eq!(expired, None, "value must be gone after its TTL");

    // delete → miss.
    tokio::task::block_in_place(|| cache.delete(&tenant, key)).expect("delete");
    let after = tokio::task::block_in_place(|| cache.get(&tenant, key)).expect("get after del");
    assert_eq!(after, None, "deleted key must be a miss");
}

// ---- PgStore: OLTP reachable + migrations run ----------------------------------------------

fn admin_url(cfg: &MyelinConfig) -> String {
    // DDL/seed runs as the migration/owner role; the app role connects separately so RLS is
    // enforced. The smoke test owns its throwaway rows, so it uses the admin URL throughout.
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// A raw admin/owner pool for test-only DDL + cleanup SQL. MR-013 removed `PgStore::pool()` (the
/// bare tenant-bypassing hatch); the test harness builds its OWN admin pool from the admin URL for
/// throwaway-row cleanup — this is test infrastructure, NOT the tenant store handing out its pool.
async fn admin_pool(cfg: &MyelinConfig) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url(cfg))
        .await
        .expect("connect admin pool (is the stack up?)")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_oltp_connect_and_select() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("connect Postgres (is the stack up?)");
    // The OLTP-reachability probe — through the scoped health helper that replaced the bare
    // `pool()` hatch (MR-013). A successful `SELECT 1` proves the tier answers.
    store.health_check().await.expect("OLTP health check");
}

// ---- PgStore: the ReBAC tuple store, RLS-scoped --------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_rebac_tuple_store_reverse_index() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    store
        .migrate()
        .await
        .expect("run migrations (outbox + rebac_tuple + RLS)");

    let tenant = format!("acme-{}", std::process::id());
    // The admin/owner pool for this run's cleanup — connected here (ahead of the wrapped body
    // below) so it is available to the cleanup closure regardless of how the body exits.
    let pool = admin_pool(&cfg).await;

    // Wrapped so a mid-test assertion failure or panic still drops this run's seeded rows,
    // instead of only the happy path reaching the final cleanup below (see tests/common/mod.rs).
    common::with_cleanup(
        || async {
            // alice is reader on doc1 + doc2; bob on doc3 (must not match alice's lookup).
            store
                .put_tuple(&tenant, "doc1", "reader", "user:alice")
                .await
                .expect("put doc1");
            store
                .put_tuple(&tenant, "doc2", "reader", "user:alice")
                .await
                .expect("put doc2");
            store
                .put_tuple(&tenant, "doc3", "reader", "user:bob")
                .await
                .expect("put doc3");

            // The S8 reverse-index lookup, RLS-scoped to this tenant.
            let objs = store
                .reverse_index(&tenant, "user:alice", "reader")
                .await
                .expect("reverse index");
            assert_eq!(objs, vec!["doc1".to_string(), "doc2".to_string()]);
        },
        || async {
            // Cleanup this run's rows (best-effort, via the admin/owner pool).
            sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
                .bind(&tenant)
                .execute(&pool)
                .await
                .ok();
        },
    )
    .await;
}

// ---- PgStore: the outbox + relay (real FOR UPDATE SKIP LOCKED) drained THROUGH BusTransport --

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn envelope(id: &str, subject: &str, aggregate: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: Some(CausedBy("session:abc".into())),
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "x" }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_outbox_relay_drains_to_bus() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    store.migrate().await.expect("run migrations");

    let agg = format!("issue:SMOKE-{}", std::process::id());
    let id1 = format!("smoke-out-1-{}", std::process::id());
    let id2 = format!("smoke-out-2-{}", std::process::id());
    // The admin/owner pool for this run's cleanup — connected here (ahead of the wrapped body
    // below) so it is available to the cleanup closure regardless of how the body exits.
    let pool = admin_pool(&cfg).await;

    // Wrapped so a mid-test assertion failure or panic still drops this run's outbox rows,
    // instead of only the happy path reaching the final cleanup below (see tests/common/mod.rs).
    common::with_cleanup(
        || async {
            // The OLTP-co-located relay (drains the outbox to the BusTransport trait).
            let relay = store.relay();

            // Enqueue two outbox rows (the envelope stored as JSONB), published_at = NULL.
            relay
                .enqueue(&agg, 0, &envelope(&id1, "myelin://acme/issues/A", &agg))
                .await
                .expect("enqueue 1");
            relay
                .enqueue(&agg, 1, &envelope(&id2, "myelin://acme/issues/B", &agg))
                .await
                .expect("enqueue 2");

            // The relay claims unsent rows with FOR UPDATE SKIP LOCKED and publishes them through
            // the BusTransport trait (here the in-process bus — the drain is real PG; the bus is
            // the trait).
            let bus = InProcessBus::new();
            let n = relay.relay_once(&bus, 16).await.expect("relay drain");
            assert_eq!(n, 2, "the relay must publish both enqueued rows");

            // Both events reached the bus (0 lost), and the outbox is fully drained for this
            // aggregate.
            assert!(bus.delivered_ids().contains(&EventId(id1.clone())));
            assert!(bus.delivered_ids().contains(&EventId(id2.clone())));

            // A second drain pass publishes nothing (the rows are marked published_at = now()).
            let n2 = relay.relay_once(&bus, 16).await.expect("relay drain 2");
            assert_eq!(n2, 0, "already-published rows are not re-claimed");
        },
        || async {
            // Cleanup this run's rows. See `common::delete_outbox_for_aggregate` for why this
            // isn't a bare `DELETE FROM outbox WHERE aggregate = $1`: `outbox_quarantine` FKs to
            // `outbox` with `ON DELETE RESTRICT`, and a concurrent quarantine sweep on this shared
            // dev DB (confirmed live) can otherwise leave this run's tagged rows stuck forever.
            common::delete_outbox_for_aggregate(&pool, &agg).await;
        },
    )
    .await;
}
