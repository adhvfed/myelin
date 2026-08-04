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

fn mid(n: u32) -> MessageId {
    MessageId(format!("01JCHATP410READ{n:011}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_p16_read_state_pg_authoritative_on_real_valkey_loss() {
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

    let hot = tokio::task::block_in_place(|| valkey.get(&tenant, &cache_key))
        .expect("valkey GET")
        .expect("a hot marker is cached");
    assert_eq!(
        String::from_utf8(hot).unwrap(),
        mid(4).0,
        "Valkey serves the hot marker"
    );

    tokio::task::block_in_place(|| valkey.delete(&tenant, &cache_key)).expect("valkey DEL");
    let after_drop =
        tokio::task::block_in_place(|| valkey.get(&tenant, &cache_key)).expect("valkey GET");
    assert_eq!(
        after_drop, None,
        "the hot marker is gone from Valkey (cache loss)"
    );

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
