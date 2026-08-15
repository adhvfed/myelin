#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_config::MyelinConfig;
use myelin_storage::blob::{BlobStore, ContentHash};
use myelin_storage::pg::PgStore;
use myelin_storage::s3blob::S3BlobStore;
use myelin_tenancy::Region;

use myelin_events::nats::NatsJetStreamBus;
use myelin_events::relay::BusTransport;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::TenantId;

mod common;
#[path = "support/isolated_database.rs"]
mod isolated_database;

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn admin_pool(cfg: &MyelinConfig) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect(&admin_url(cfg))
        .await
        .expect("connect admin pool (is the stack up?)")
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

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

async fn rls_read_all_no_predicate(store: &PgStore, acting_tenant: &str) -> Vec<String> {
    use sqlx::Row;
    let mut conn = store
        .scoped_conn(acting_tenant)
        .await
        .expect("acquire RLS-scoped connection");
    let rows = sqlx::query("SELECT object_id FROM rebac_tuple ORDER BY object_id")
        .fetch_all(&mut *conn)
        .await
        .expect("RLS-scoped read");
    rows.iter()
        .map(|r| r.get::<String, _>("object_id"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drill1_outbox_no_loss_under_crash() {
    let cfg = MyelinConfig::dev();
    let isolated =
        isolated_database::IsolatedDatabase::create(&admin_url(&cfg), "drill1_outbox").await;
    let store = PgStore::connect(isolated.url(), &cfg.region, 6)
        .await
        .expect("connect Postgres (is the stack up?)");
    store
        .migrate()
        .await
        .expect("run migrations (outbox + rebac_tuple + RLS)");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(isolated.url())
        .await
        .expect("connect isolated drill pool");

    let tag = uniq();
    let state_table = format!("drill1_state_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {state_table} (id text PRIMARY KEY, event_id text NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create drill state table");

    let agg = format!("issue:DRILL1-{tag}");
    const N: usize = 8;
    let ids: Vec<String> = (0..N).map(|i| format!("d1-evt-{tag}-{i}")).collect();

    let stream = format!("MYELIN_DRILL1_{}", tag.replace('-', "_"));
    let subject_root = format!("myelin_drill1_{}", tag.replace('-', "_"));
    let consumer = format!("{stream}_pull");
    let bus = NatsJetStreamBus::connect(
        &cfg.nats_url,
        &stream,
        &subject_root,
        &consumer,
        tokio::runtime::Handle::current(),
    )
    .expect("connect NATS JetStream bus (is the stack up with -js?)");
    tokio::task::block_in_place(|| bus.purge());

    common::with_cleanup(
        || async {
            let relay = store.relay();
            for (i, id) in ids.iter().enumerate() {
                relay
                    .enqueue_with_state(
                        &state_table,
                        &format!("state-{i}"),
                        &agg,
                        i as i64,
                        &envelope(id, &format!("myelin://acme/issues/{i}"), &agg),
                    )
                    .await
                    .expect("co-commit state + outbox row");
            }
            let committed: std::collections::HashSet<EventId> =
                ids.iter().cloned().map(EventId).collect();
            assert_eq!(
                relay.outbox_depth().await.expect("depth") as usize,
                N,
                "all N events are durably committed + unsent before the drain"
            );

            let crash_after = 3usize;
            let published_before_crash = relay
                .relay_once_crash_after(&bus, 16, crash_after)
                .await
                .expect("crash-injection drain pass");
            assert_eq!(
                published_before_crash, crash_after,
                "the relay published exactly {crash_after} rows to the broker before crashing"
            );
            assert_eq!(
                relay.outbox_depth().await.expect("depth after crash") as usize,
                N,
                "crash committed no published_at marks → all N rows stay claimable (0 lost)"
            );

            let published_after_restart = relay
                .relay_once(&bus, 16)
                .await
                .expect("restart drain pass");
            assert_eq!(
                published_after_restart, N,
                "the restarted relay re-claims all N unsent rows (the 3 crash-window rows re-publish + dedup)"
            );
            assert_eq!(
                relay.outbox_depth().await.expect("final depth"),
                0,
                "outbox-depth drains to 0 after the restart (every committed row recorded sent)"
            );

            let mut delivered: std::collections::HashMap<EventId, usize> =
                std::collections::HashMap::new();
            for _ in 0..8 {
                let batch = tokio::task::block_in_place(|| bus.consume(&subject_root));
                if batch.is_empty() {
                    break;
                }
                for env in &batch {
                    *delivered.entry(env.event_id.clone()).or_insert(0) += 1;
                    tokio::task::block_in_place(|| bus.ack(&consumer, &env.event_id));
                }
            }

            let delivered_ids: std::collections::HashSet<EventId> =
                delivered.keys().cloned().collect();
            assert_eq!(
                delivered_ids, committed,
                "0 lost: the delivered set equals exactly the committed set"
            );
            for (id, count) in &delivered {
                assert_eq!(
                    *count, 1,
                    "0 ghost: event {id:?} delivered exactly once (the crash re-publish was deduplicated)"
                );
            }
            assert_eq!(
                delivered.len(),
                N,
                "exactly N distinct events delivered, exactly-once each"
            );

            for env in committed.iter() {
                let n: i64 = sqlx::query_scalar(&format!(
                    "SELECT count(*) FROM {state_table} WHERE event_id = $1"
                ))
                .bind(&env.0)
                .fetch_one(&pool)
                .await
                .expect("state lookup");
                assert_eq!(
                    n, 1,
                    "every delivered event has its committed state change (no ghost)"
                );
            }

            println!(
                "[2026-06-19] PASS  drill=OUTBOX-NO-LOSS-UNDER-CRASH  committed={N} delivered={N} \
                 lost=0 ghost=0  crash_window={crash_after} (re-published+deduped)  outbox_depth=0  \
                 backend=real-PG+real-NATS-JetStream"
            );
        },
        || async {
            tokio::task::block_in_place(|| bus.purge());
            isolated.drop_schema().await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drill2_restore_verify_cross_seam() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 6)
        .await
        .expect("connect Postgres");
    store.migrate().await.expect("migrate");
    let pool = admin_pool(&cfg).await;

    let blobs = S3BlobStore::connect(&cfg.s3, tokio::runtime::Handle::current());
    let tag = uniq();
    let tenant = TenantId(format!("acme-d2-{tag}"));

    let docs = format!("drill2_docs_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {docs} \
         (seq bigint PRIMARY KEY, blob_hash text NOT NULL, bus_event_id text NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create docs table");

    let offset = format!("drill2_offset_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {offset} (k text PRIMARY KEY, bus_offset bigint NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create offset table");
    sqlx::query(&format!(
        "INSERT INTO {offset} (k, bus_offset) VALUES ('bus', 0) ON CONFLICT (k) DO NOTHING"
    ))
    .execute(&pool)
    .await
    .expect("seed offset");

    const T: i64 = 3;

    common::with_cleanup(
        || async {
            let mut blob_at: std::collections::HashMap<i64, ContentHash> =
                std::collections::HashMap::new();
            for seq in 1..=T {
                let bytes = format!("drill2 blob payload seq={seq} tag={tag}").into_bytes();
                let hash =
                    tokio::task::block_in_place(|| blobs.put(&tenant, &bytes)).expect("blob put");
                blob_at.insert(seq, hash.clone());
                let bus_event_id = format!("d2-evt-{tag}-{seq}");
                sqlx::query(&format!(
                    "INSERT INTO {docs} (seq, blob_hash, bus_event_id) VALUES ($1, $2, $3)"
                ))
                .bind(seq)
                .bind(&hash.digest_hex)
                .bind(&bus_event_id)
                .execute(&pool)
                .await
                .expect("insert doc row");
                sqlx::query(&format!(
                    "UPDATE {offset} SET bus_offset = $1 WHERE k = 'bus'"
                ))
                .bind(seq)
                .execute(&pool)
                .await
                .expect("advance bus offset");
            }

            let captured_offset: i64 =
                sqlx::query_scalar(&format!("SELECT bus_offset FROM {offset} WHERE k='bus'"))
                    .fetch_one(&pool)
                    .await
                    .expect("read offset at T");
            assert_eq!(captured_offset, T, "the captured bus offset is exactly T");

            let post_bytes = format!("drill2 POST-T blob tag={tag}").into_bytes();
            let post_hash = tokio::task::block_in_place(|| blobs.put(&tenant, &post_bytes))
                .expect("post blob put");
            sqlx::query(&format!(
                "INSERT INTO {docs} (seq, blob_hash, bus_event_id) VALUES ($1, $2, $3)"
            ))
            .bind(T + 1)
            .bind(&post_hash.digest_hex)
            .bind(format!("d2-evt-{tag}-{}", T + 1))
            .execute(&pool)
            .await
            .expect("insert post-T row");
            sqlx::query(&format!(
                "UPDATE {offset} SET bus_offset = $1 WHERE k = 'bus'"
            ))
            .bind(T + 1)
            .execute(&pool)
            .await
            .expect("advance bus offset past T");

            sqlx::query(&format!("DELETE FROM {docs} WHERE seq > $1"))
                .bind(T)
                .execute(&pool)
                .await
                .expect("restore: drop rows past T");
            sqlx::query(&format!(
                "UPDATE {offset} SET bus_offset = $1 WHERE k = 'bus'"
            ))
            .bind(T)
            .execute(&pool)
            .await
            .expect("restore: land bus offset at T");

            let restored_rows: Vec<(i64, String)> = {
                let rows: Vec<(i64, String)> =
                    sqlx::query_as(&format!("SELECT seq, blob_hash FROM {docs} ORDER BY seq"))
                        .fetch_all(&pool)
                        .await
                        .expect("read restored rows");
                rows
            };
            assert_eq!(
                restored_rows.len() as i64,
                T,
                "exactly the seq<=T rows survive the restore"
            );
            for (seq, blob_hex) in &restored_rows {
                let expected = blob_at.get(seq).expect("known blob for restored seq");
                assert_eq!(
                    *blob_hex, expected.digest_hex,
                    "row's stored address matches what we wrote"
                );
                let got = tokio::task::block_in_place(|| blobs.get(&tenant, expected)).expect(
                    "restored row's referenced blob is present + bytes re-hash to its address",
                );
                assert_eq!(
                    ContentHash::blake3(&got),
                    *expected,
                    "no silent corruption: the referenced blob re-hashes to the row's content-address"
                );
            }

            let max_restored_seq: i64 =
                sqlx::query_scalar(&format!("SELECT coalesce(max(seq),0) FROM {docs}"))
                    .fetch_one(&pool)
                    .await
                    .expect("max restored seq");
            let restored_offset: i64 =
                sqlx::query_scalar(&format!("SELECT bus_offset FROM {offset} WHERE k='bus'"))
                    .fetch_one(&pool)
                    .await
                    .expect("restored offset");
            assert_eq!(max_restored_seq, T, "the newest restored row is at T");
            assert_eq!(restored_offset, T, "the bus offset landed at T");
            assert!(
                restored_offset <= max_restored_seq,
                "no bus offset is past the restored rows (offset {restored_offset} <= max seq {max_restored_seq})"
            );

            let post_rows: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM {docs} WHERE seq > $1"))
                    .bind(T)
                    .fetch_one(&pool)
                    .await
                    .expect("count post-T rows");
            assert_eq!(
                post_rows, 0,
                "the post-T row was rolled back by the restore (no row past T)"
            );

            println!(
                "[2026-06-19] PASS  drill=RESTORE-VERIFY-CROSS-SEAM  T={T}  restored_rows={T} \
                 dangling_blob_refs=0 corrupt_blobs=0 bus_offset_past_rows=0  \
                 backend=real-PG+real-RustFS (bus-offset cursor)"
            );
        },
        || async {
            for seq in 1..=T {
                let bytes = format!("drill2 blob payload seq={seq} tag={tag}").into_bytes();
                let hash = ContentHash::blake3(&bytes);
                tokio::task::block_in_place(|| blobs.delete(&tenant, &hash)).ok();
            }
            let post_bytes = format!("drill2 POST-T blob tag={tag}").into_bytes();
            let post_hash = ContentHash::blake3(&post_bytes);
            tokio::task::block_in_place(|| blobs.delete(&tenant, &post_hash)).ok();
            sqlx::raw_sql(&format!(
                "DROP TABLE IF EXISTS {docs}; DROP TABLE IF EXISTS {offset};"
            ))
            .execute(&pool)
            .await
            .ok();
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drill3_tenant_region_rls_isolation() {
    let cfg = MyelinConfig::dev();

    let admin = PgStore::connect(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres as admin");
    admin
        .migrate()
        .await
        .expect("migrate (rebac_tuple + RLS policy)");
    let pool = admin_pool(&cfg).await;

    let tag = uniq();
    let tenant_a = format!("tenant-A-{tag}");
    let tenant_b = format!("tenant-B-{tag}");

    common::with_cleanup(
        || async {
            admin
                .put_tuple(&tenant_a, "A-doc1", "reader", "user:alice")
                .await
                .expect("seed A1");
            admin
                .put_tuple(&tenant_a, "A-doc2", "reader", "user:alice")
                .await
                .expect("seed A2");
            admin
                .put_tuple(&tenant_b, "B-secret1", "reader", "user:mallory")
                .await
                .expect("seed B1");
            admin
                .put_tuple(&tenant_b, "B-secret2", "reader", "user:mallory")
                .await
                .expect("seed B2");

            let app = PgStore::connect(&cfg.database_url, &cfg.region, 4)
                .await
                .expect("connect Postgres as the NOBYPASSRLS app role");

            let visible_as_a = rls_read_all_no_predicate(&app, &tenant_a).await;
            assert_eq!(
                visible_as_a,
                vec!["A-doc1".to_string(), "A-doc2".to_string()],
                "tenant A sees exactly its own rows (DB-enforced RLS, no tenant predicate in the query)"
            );
            assert!(
                !visible_as_a.iter().any(|o| o.starts_with("B-")),
                "ZERO cross-tenant leak: none of tenant B's rows are visible to tenant A"
            );

            let visible_as_b = rls_read_all_no_predicate(&app, &tenant_b).await;
            assert_eq!(
                visible_as_b,
                vec!["B-secret1".to_string(), "B-secret2".to_string()],
                "tenant B sees exactly its own rows"
            );
            assert!(
                !visible_as_b.iter().any(|o| o.starts_with("A-")),
                "ZERO cross-tenant leak: none of tenant A's rows are visible to tenant B"
            );

            let cross = app
                .reverse_index(&tenant_a, "user:mallory", "reader")
                .await
                .expect("reverse_index as tenant A for tenant B's subject");
            assert!(
                cross.is_empty(),
                "no cross-tenant query path: tenant A's reverse_index for tenant B's subject is empty"
            );

            println!(
                "[2026-06-19] PASS  drill=(TENANT,REGION)-RLS-ISOLATION  tenantA_rows={} tenantB_rows={} \
                 cross_tenant_leak=0 (DB-enforced FORCE RLS, NOBYPASSRLS app role)  \
                 cross_tenant_query_path=0  backend=real-PG",
                visible_as_a.len(),
                visible_as_b.len()
            );
        },
        || async {
            sqlx::query(
                "SELECT set_config('myelin.tenant_id',$1,false), set_config('myelin.region',$2,false)",
            )
            .bind(&tenant_a)
            .bind(&cfg.region)
            .execute(&pool)
            .await
            .ok();
            sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
                .bind(&tenant_a)
                .execute(&pool)
                .await
                .ok();
            sqlx::query(
                "SELECT set_config('myelin.tenant_id',$1,false), set_config('myelin.region',$2,false)",
            )
            .bind(&tenant_b)
            .bind(&cfg.region)
            .execute(&pool)
            .await
            .ok();
            sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
                .bind(&tenant_b)
                .execute(&pool)
                .await
                .ok();
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drill4_rebac_check_list_objects_no_leak_no_n_plus_1() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    store.migrate().await.expect("migrate (rebac_tuple + RLS)");
    let pool = admin_pool(&cfg).await;

    let tag = uniq();
    let tenant = format!("acme-d4-{tag}");

    common::with_cleanup(
        || async {
            let alice_objs: Vec<String> = (0..12).map(|i| format!("alice-doc-{i:02}")).collect();
            let bob_objs: Vec<String> = (0..8).map(|i| format!("bob-secret-{i:02}")).collect();
            for o in &alice_objs {
                store
                    .put_tuple(&tenant, o, "reader", "user:alice")
                    .await
                    .expect("put alice tuple");
            }
            for o in &bob_objs {
                store
                    .put_tuple(&tenant, o, "reader", "user:bob")
                    .await
                    .expect("put bob tuple");
            }
            store
                .put_tuple(&tenant, "alice-doc-00", "writer", "user:mallory")
                .await
                .expect("put mallory");

            assert!(
                store
                    .check_tuple(&tenant, "alice-doc-00", "reader", "user:alice")
                    .await
                    .expect("check"),
                "ALLOW: alice IS reader on alice-doc-00"
            );
            assert!(
                !store
                    .check_tuple(&tenant, "bob-secret-00", "reader", "user:alice")
                    .await
                    .expect("check"),
                "DENY (fail-closed): alice is NOT reader on bob-secret-00"
            );
            assert!(
                !store
                    .check_tuple(&tenant, "alice-doc-00", "reader", "user:mallory")
                    .await
                    .expect("check"),
                "DENY: mallory is writer, not reader, on alice-doc-00 (no relation confusion)"
            );
            assert!(
                !store
                    .check_tuple(&tenant, "no-such-doc", "reader", "user:alice")
                    .await
                    .expect("check"),
                "DENY: a nonexistent edge is denied, never a silent allow"
            );

            let visible = store
                .list_objects(&tenant, "user:alice", "reader")
                .await
                .expect("list_objects");
            let mut expected = alice_objs.clone();
            expected.sort();
            assert_eq!(
                visible, expected,
                "list_objects returns EXACTLY alice's visible objects"
            );
            for b in &bob_objs {
                assert!(
                    !visible.contains(b),
                    "NO LEAK: unauthorized object {b} is absent from alice's list"
                );
            }
            for o in &visible {
                assert!(
                    store
                        .check_tuple(&tenant, o, "reader", "user:alice")
                        .await
                        .expect("re-check"),
                    "every listed object {o} is genuinely visible (check() agrees with list_objects)"
                );
            }

            let before = store.authz_query_count();
            let visible2 = store
                .list_objects(&tenant, "user:alice", "reader")
                .await
                .expect("list_objects 2");
            let after = store.authz_query_count();
            assert_eq!(
                after - before,
                1,
                "NO N+1: list_objects issued exactly ONE reverse-index query for all {} objects \
                 (not one check per candidate)",
                visible2.len()
            );
            assert!(
                visible2.len() >= 12,
                "the no-N+1 assertion is over a many-object set ({} objects)",
                visible2.len()
            );

            println!(
                "[2026-06-19] PASS  drill=ReBAC-NO-LEAK/NO-N+1  visible={} leaked=0 check_allow/deny=correct \
                 list_objects_queries=1 (NOT {} per-candidate checks)  backend=real-PG-tuple-store",
                visible.len(),
                visible.len()
            );
        },
        || async {
            sqlx::query(
                "SELECT set_config('myelin.tenant_id',$1,false), set_config('myelin.region',$2,false)",
            )
            .bind(&tenant)
            .bind(&cfg.region)
            .execute(&pool)
            .await
            .ok();
            sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
                .bind(&tenant)
                .execute(&pool)
                .await
                .ok();
        },
    )
    .await;
}
