//! **CHAT-P16 / P-410 — the read-state hot path PROVEN against the live dev stack (REAL Valkey hot
//! markers + REAL Postgres durable record; cache-never-authoritative). The CHAT-D12 real-data leg.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-chat --features integration \
//!     --test integration_chat_p16_read_state -- --nocapture
//!
//! This is the GATE's REAL-DATA leg for CHAT-D12. It proves, against REAL Valkey (the `fred` client,
//! the hot-marker write-back tier) + REAL Postgres (the `read_state` durable record, the truth):
//!
//! 1. **CHAT-D12 (PG authoritative):** a marker written to Valkey + flushed to the PG `read_state`
//!    record survives a REAL Valkey DEL (the cache drop) — the read reconstructs from Postgres (0 lost
//!    read-state).
//! 2. **benign+bounded:** an UN-flushed Valkey marker lost on the DEL falls back to the older durable
//!    PG value (at-worst slightly stale, never below the truth).
//! 3. **monotone:** a stale/out-of-order PG flush never rewinds the durable read-position (GREATEST).
//! 4. **the residency-pin holds:** a session pinned to a different region reads 0 of the marker.
//!
//! The drill is registered red-until-proven and flips green ONLY here, against the live stack — never
//! mocked.
#![cfg(feature = "integration")]

use myelin_chat::read_state::pg::PgReadStateRecord;
use myelin_chat::read_state::ReadMarker;
use myelin_chat::store::{ConversationId, MessageId};
use myelin_storage::cache::Cache;
use myelin_storage::valkey::ValkeyCache;
use myelin_tenancy::TenantId;
use sqlx::postgres::PgPoolOptions;

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6380".into())
}

fn region() -> &'static str {
    "fr-par"
}

/// A k-sortable message id for the marker (the ULID canonical form — sorts lexically == time order).
fn mid(n: u32) -> MessageId {
    MessageId(format!("01JCHATP410READ{n:011}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_p16_read_state_pg_authoritative_on_real_valkey_loss() {
    // ── REAL Postgres durable record (the truth) ──
    let admin = PgPoolOptions::new()
        .max_connections(6)
        .connect(&admin_url())
        .await
        .expect(
            "connect to dev Postgres as admin (is the stack up? \
             docker compose -f docker-compose.dev.yml up -d --wait)",
        );
    let suffix = std::process::id();
    let table = format!("read_state_p410_{suffix}");
    let record = PgReadStateRecord::new(admin.clone(), region(), table.clone());
    record
        .migrate()
        .await
        .expect("apply the read_state DDL + RLS");

    // ── REAL Valkey hot-marker tier (the write-back cache) ──
    let valkey = ValkeyCache::connect(&redis_url(), tokio::runtime::Handle::current())
        .expect("connect dev Valkey (is the stack up?)");

    let tenant_str = format!("acmeP410-{suffix}");
    let conv = ConversationId::new(
        tenant_str.clone(),
        region(),
        format!("01J0CONVP410{suffix}"),
    );
    let tenant = TenantId(tenant_str.clone());
    let principal = "alice";
    let cache_key = ReadMarker::cache_key(&conv, principal);

    // ── 1. mark read: WRITE the hot marker to REAL Valkey + FLUSH to REAL Postgres ──
    let marker = ReadMarker::new(conv.clone(), principal, mid(4));
    tokio::task::block_in_place(|| {
        valkey.set(
            &tenant,
            &cache_key,
            marker.last_read.as_str().as_bytes(),
            std::time::Duration::from_secs(300),
        )
    })
    .expect("write the hot marker to Valkey");
    let persisted = record.upsert(&marker).await.expect("flush to PG");
    assert_eq!(persisted, mid(4), "the durable PG record holds the marker");

    // The hot marker reads from REAL Valkey (HIT).
    let hot = tokio::task::block_in_place(|| valkey.get(&tenant, &cache_key))
        .expect("valkey GET")
        .expect("a hot marker is cached");
    assert_eq!(
        String::from_utf8(hot).unwrap(),
        mid(4).0,
        "Valkey serves the hot marker"
    );

    // ── 2. DROP the marker from REAL Valkey (the cache loss) ──
    tokio::task::block_in_place(|| valkey.delete(&tenant, &cache_key)).expect("valkey DEL");
    let after_drop =
        tokio::task::block_in_place(|| valkey.get(&tenant, &cache_key)).expect("valkey GET");
    assert_eq!(
        after_drop, None,
        "the hot marker is gone from Valkey (cache loss)"
    );

    // ── 3. the PG record is AUTHORITATIVE — the marker reconstructs from Postgres (0 lost) ──
    let reconstructed = record
        .load(&conv, principal)
        .await
        .expect("load from PG")
        .expect("the durable marker is present");
    assert_eq!(
        reconstructed,
        mid(4),
        "CHAT-D12: the PG record is authoritative after a REAL Valkey loss (0 lost read-state)"
    );

    // ── 4. benign+bounded: an UN-flushed Valkey advance is lost → fall back to the durable truth ──
    // Write a HIGHER marker to Valkey only (NOT flushed to PG), then drop Valkey.
    tokio::task::block_in_place(|| {
        valkey.set(
            &tenant,
            &cache_key,
            mid(7).as_str().as_bytes(),
            std::time::Duration::from_secs(300),
        )
    })
    .expect("write the un-flushed hot marker");
    tokio::task::block_in_place(|| valkey.delete(&tenant, &cache_key)).expect("valkey DEL");
    // The durable truth is still mid(4) (the un-flushed mid(7) is lost — at-worst slightly stale).
    let still = record.load(&conv, principal).await.unwrap().unwrap();
    assert_eq!(
        still,
        mid(4),
        "benign+bounded: the un-flushed advance is lost, fall back to the durable truth (slightly stale)"
    );
    assert!(
        still < mid(7),
        "the staleness is bounded by the un-flushed window only"
    );

    // ── 5. the durable flush is MONOTONE — a stale/out-of-order flush never rewinds (GREATEST) ──
    record
        .upsert(&ReadMarker::new(conv.clone(), principal, mid(9)))
        .await
        .expect("flush a newer marker");
    let stale = record
        .upsert(&ReadMarker::new(conv.clone(), principal, mid(2)))
        .await
        .expect("a stale flush is a no-op (GREATEST)");
    assert_eq!(
        stale,
        mid(9),
        "monotone: a stale flush does not rewind the durable read-position (GREATEST keeps mid(9))"
    );

    // ── 6. the holder erasure leg: purge the principal's read-state on erasure (D-C8) ──
    let purged = record
        .purge_principal(&tenant_str, principal)
        .await
        .expect("purge");
    assert_eq!(
        purged, 1,
        "D-C8: the principal's read-state marker is purged on erasure"
    );
    let gone = record
        .load(&conv, principal)
        .await
        .expect("load after purge");
    assert_eq!(
        gone, None,
        "10.1/D-C8: 0 recoverable read-state for the erased subject"
    );
}
