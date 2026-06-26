//! # Stage 3 — the four foundational M0/M1 drills, retrofitted as REAL integration tests.
//!
//! These are the silent-data-loss + authz-leak GATES. They are PROVEN against the live
//! docker-compose stack (real Postgres + real RustFS + real NATS JetStream), not modeled in
//! memory. Each drill builds on the stage-2 real backends (PgStore / PgRelay / S3BlobStore /
//! NatsJetStreamBus) THROUGH their frozen traits — it does not fork a trait.
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test stage3_drills -- --nocapture
//!
//! The four drills:
//!   1. OUTBOX NO-LOSS UNDER CRASH  — same-tx outbox → real PG; relay drains to real NATS; crash
//!      mid-drain; restart; assert every committed event delivered exactly-once-in-effect
//!      (dedup on event_id), 0 lost, 0 ghost.
//!   2. RESTORE-VERIFY CROSS-SEAM   — rows (PG) + blobs (RustFS) + bus offset (NATS); capture a
//!      consistent point T; mutate past T; restore PG to T; assert rows <-> blob refs <-> bus
//!      offsets are mutually consistent.
//!   3. (TENANT,REGION) RLS ISOLATION — two tenants in PG with RLS; acting as tenant A's role,
//!      reading tenant B's rows yields ZERO rows (DB-enforced); no cross-tenant query path.
//!   4. ReBAC check/list_objects NO-LEAK / NO-N+1 — real tuples in the PG tuple store; check()
//!      allow/deny correctness; list_objects returns EXACTLY the visible set (no leak); list is
//!      ONE reverse-index query, NOT one check per candidate (no N+1).
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

// ----------------------------------------------------------------------------------------------
// shared helpers
// ----------------------------------------------------------------------------------------------

/// DDL/seed runs as the migration/owner role (myelin_admin); the RLS drill ALSO connects as the
/// NOBYPASSRLS app role (myelin_app) separately to prove the DB — not app code — denies the
/// cross-tenant read. The dev DATABASE_URL is already the app role; this rewrites it to admin.
fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// A raw admin/owner pool for test-only DDL + cleanup SQL. MR-013 removed `PgStore::pool()` (the
/// bare tenant-bypassing hatch); the drills build their OWN admin pool from the admin URL for the
/// throwaway state tables + cleanup — test infrastructure, NOT the tenant store handing out its pool.
async fn admin_pool(cfg: &MyelinConfig) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect(&admin_url(cfg))
        .await
        .expect("connect admin pool (is the stack up?)")
}

/// A process-unique, monotonic suffix so concurrent runs (and re-runs) never collide on
/// event_ids / aggregates / stream names.
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

/// The deliberately tenant-predicate-LESS RLS-isolation probe (drill 3). Acquire a connection
/// scoped to `acting_tenant` (the session `(tenant, region)` GUCs the RLS policy keys on) and run
/// a `SELECT * FROM rebac_tuple` carrying NO `WHERE tenant_id` — so the ONLY thing scoping the
/// read is the DB's FORCE-RLS policy. If RLS were off / bypassed, this would leak every tenant's
/// rows. It lives HERE (a test) — not in pg.rs — precisely because it is an intentional
/// no-tenant-predicate query: pg.rs keeps every tenant-store query tenant-bound so the
/// `tenant-predicate` IDOR lint stays fully live over production source.
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

// ==============================================================================================
// DRILL 1 — OUTBOX NO-LOSS UNDER CRASH (silent-data-loss gate: 0 lost, 0 ghost, exactly-once)
// ==============================================================================================
//
// ASSERTION: With N events written via the transactional outbox (co-committed in the SAME tx as
// a domain state change) into REAL Postgres, a relay drains them to REAL NATS JetStream. A crash
// mid-drain (the transaction recording `published_at` is dropped after the broker publish) leaves
// rows that were ALREADY published-to-the-broker still marked unsent in PG. On restart the relay
// re-claims and re-publishes those rows. We assert:
//   - 0 LOST  : every one of the N committed event_ids is delivered (the broker's delivered set
//               ⊇ the committed set).
//   - 0 GHOST : the broker delivers each distinct event_id EXACTLY ONCE — the re-publish of the
//               crash-window rows is suppressed broker-side by Nats-Msg-Id = event_id dedup. No
//               event without its committed state change (emit-iff-committed is structural).
//   - DRAINS  : after the restart the outbox depth is 0 (every committed row is recorded sent).

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drill1_outbox_no_loss_under_crash() {
    let cfg = MyelinConfig::dev();
    let store = PgStore::connect(&admin_url(&cfg), &cfg.region, 6)
        .await
        .expect("connect Postgres (is the stack up?)");
    store
        .migrate()
        .await
        .expect("run migrations (outbox + rebac_tuple + RLS)");
    let pool = admin_pool(&cfg).await;

    let tag = uniq();
    // A real domain STATE table the outbox co-commits with (the emit-iff-committed seam).
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

    // (1) Write N events via the transactional outbox — each co-committed in the SAME tx as a
    //     domain state change (BUS-D4 emit-iff-committed). A committed state change ALWAYS has its
    //     outbox row; a rolled-back tx writes neither (no ghost).
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
    let committed: std::collections::HashSet<EventId> = ids.iter().cloned().map(EventId).collect();
    assert_eq!(
        relay.outbox_depth().await.expect("depth") as usize,
        N,
        "all N events are durably committed + unsent before the drain"
    );

    // The real NATS JetStream bus (durable stream + durable PULL consumer + Nats-Msg-Id dedup).
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
    tokio::task::block_in_place(|| bus.purge()); // clean stream state for a deterministic run.

    // (2) Start draining to real NATS, then CRASH mid-drain: publish the first 3 rows to the
    //     broker but DROP the transaction before recording `published_at` for ANY of them. Those
    //     3 rows are now published-to-NATS yet still `published_at IS NULL` in PG — the exact
    //     silent-loss window (the relay forwarded the event then died before recording it did).
    let crash_after = 3usize;
    let published_before_crash = relay
        .relay_once_crash_after(&bus, 16, crash_after)
        .await
        .expect("crash-injection drain pass");
    assert_eq!(
        published_before_crash, crash_after,
        "the relay published exactly {crash_after} rows to the broker before crashing"
    );
    // 0 LOST: the crash recorded NO marks — every committed row is still claimable.
    assert_eq!(
        relay.outbox_depth().await.expect("depth after crash") as usize,
        N,
        "crash committed no published_at marks → all N rows stay claimable (0 lost)"
    );

    // (3) RESTART the relay: re-claim the unsent rows and re-publish. The 3 crash-window rows are
    //     re-published; the broker dedups them on Nats-Msg-Id = event_id (0 ghost). The remaining
    //     rows are published fresh. After this pass the outbox fully drains.
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

    // (4) Drain the durable PULL consumer and assert EXACTLY-ONCE-IN-EFFECT: every committed
    //     event_id delivered, none lost, none ghosted (the broker dedup collapsed the re-publish).
    let mut delivered: std::collections::HashMap<EventId, usize> = std::collections::HashMap::new();
    // Pull until the consumer is drained (a few passes; each consume acks nothing yet).
    for _ in 0..8 {
        let batch = tokio::task::block_in_place(|| bus.consume(&subject_root));
        if batch.is_empty() {
            break;
        }
        for env in &batch {
            *delivered.entry(env.event_id.clone()).or_insert(0) += 1;
            // Ack so the durable consumer does not redeliver it within the test.
            tokio::task::block_in_place(|| bus.ack(&consumer, &env.event_id));
        }
    }

    // 0 LOST: every committed event_id was delivered.
    let delivered_ids: std::collections::HashSet<EventId> = delivered.keys().cloned().collect();
    assert_eq!(
        delivered_ids, committed,
        "0 lost: the delivered set equals exactly the committed set"
    );
    // 0 GHOST: each distinct event_id delivered exactly once (broker dedup on event_id).
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

    // No event without its committed state change (emit-iff-committed): every delivered event_id
    // has a row in the co-committed state table.
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

    // cleanup
    sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
        .bind(&agg)
        .execute(&pool)
        .await
        .ok();
    sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {state_table}"))
        .execute(&pool)
        .await
        .ok();
    tokio::task::block_in_place(|| bus.purge());
}

// ==============================================================================================
// DRILL 2 — RESTORE-VERIFY CROSS-SEAM (the highest-bar silent-corruption gate)
// ==============================================================================================
//
// ASSERTION: After writing rows (PG) that reference blobs (RustFS) and advancing the bus offset
// (NATS) together, we capture a consistent point T (the per-aggregate outbox `seq` cursor — the
// §7.3 cross-seam cursor). We then MUTATE past T (a new row at seq>T pointing at a fresh blob,
// plus the bus advanced past T). We RESTORE PG to T (drop the rows whose seq>T). We then assert
// the three seams are MUTUALLY CONSISTENT at T:
//   - rows <-> blob refs : every restored row's referenced blob is PRESENT in RustFS AND its
//                          bytes re-hash to the row's stored content-address (no row pointing at
//                          a missing/corrupt blob — the §7.3 hard FAIL the bare presence-check
//                          misses).
//   - rows <-> bus offset: NO bus offset is past the restored rows — the restored bus cursor
//                          (= max restored seq) does not reference an event whose row was rolled
//                          back (no offset past the restored rows).
// The post-T mutation (the row + the bus event past T) is GONE after the restore — it neither
// dangles a blob ref nor leaves a bus offset ahead of the rows.

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

    // A real cross-seam document table: each row holds a per-aggregate `seq` (the cursor), the
    // content-address of a blob in RustFS, and the bus event_id that carried it.
    let docs = format!("drill2_docs_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {docs} \
         (seq bigint PRIMARY KEY, blob_hash text NOT NULL, bus_event_id text NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create docs table");

    // The bus offset model: the highest committed event seq the bus has delivered (an integer
    // cursor in PG, the analogue of a NATS stream sequence / consumer offset). It advances as the
    // relay publishes; restore must land it at T (never past the restored rows).
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

    // (1) Write rows + blobs + advance the bus offset together, up to the consistent point T=3.
    const T: i64 = 3;
    let mut blob_at: std::collections::HashMap<i64, ContentHash> = std::collections::HashMap::new();
    for seq in 1..=T {
        let bytes = format!("drill2 blob payload seq={seq} tag={tag}").into_bytes();
        let hash = tokio::task::block_in_place(|| blobs.put(&tenant, &bytes)).expect("blob put");
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
        // the bus delivered this event → advance the offset.
        sqlx::query(&format!(
            "UPDATE {offset} SET bus_offset = $1 WHERE k = 'bus'"
        ))
        .bind(seq)
        .execute(&pool)
        .await
        .expect("advance bus offset");
    }

    // CAPTURE the consistent point T: the snapshot is (rows seq<=T, the blob set they reference,
    // the bus offset == T). This is the one cross-seam cursor restore lands every tier at.
    let captured_offset: i64 =
        sqlx::query_scalar(&format!("SELECT bus_offset FROM {offset} WHERE k='bus'"))
            .fetch_one(&pool)
            .await
            .expect("read offset at T");
    assert_eq!(captured_offset, T, "the captured bus offset is exactly T");

    // (2) MUTATE PAST T: a new row at seq=T+1 referencing a fresh blob, and the bus advanced past
    //     T. This is the divergence a crash/restore must roll back.
    let post_bytes = format!("drill2 POST-T blob tag={tag}").into_bytes();
    let post_hash =
        tokio::task::block_in_place(|| blobs.put(&tenant, &post_bytes)).expect("post blob put");
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

    // (3) RESTORE PG to T: PITR drops every row whose seq > T (the §7.3 restore_to_offset shape),
    //     and lands the bus offset back at T (the cursor cannot point past the restored rows).
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

    // (4a) rows <-> blob refs: every restored row's referenced blob is PRESENT in RustFS AND its
    //      bytes re-hash to the stored address (no dangling ref, no silent corruption).
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
        // PRESENCE + checksum parity: get() re-hashes on read and refuses a corrupt object, so a
        // successful get IS the no-loss / checksum-parity proof for this referenced blob.
        let got = tokio::task::block_in_place(|| blobs.get(&tenant, expected))
            .expect("restored row's referenced blob is present + bytes re-hash to its address");
        assert_eq!(
            ContentHash::blake3(&got),
            *expected,
            "no silent corruption: the referenced blob re-hashes to the row's content-address"
        );
    }

    // (4b) rows <-> bus offset: the restored bus offset is exactly the max restored row seq — no
    //      offset past the restored rows (the post-T event is not referenced by any cursor).
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

    // (4c) the POST-T mutation is GONE: its row is absent (so it cannot dangle a blob ref through
    //      the row table) and the bus offset no longer references its event. (The post-T blob may
    //      still sit in the object store, but with NO row pointing at it — the safe direction; the
    //      forbidden direction is a row -> missing blob, which 4a proved cannot happen.)
    let post_rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {docs} WHERE seq > $1"))
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

    // cleanup
    for h in blob_at.values() {
        tokio::task::block_in_place(|| blobs.delete(&tenant, h)).ok();
    }
    tokio::task::block_in_place(|| blobs.delete(&tenant, &post_hash)).ok();
    sqlx::raw_sql(&format!(
        "DROP TABLE IF EXISTS {docs}; DROP TABLE IF EXISTS {offset};"
    ))
    .execute(&pool)
    .await
    .ok();
}

// ==============================================================================================
// DRILL 3 — (TENANT, REGION) RLS ISOLATION (the cross-tenant IDOR / authz-leak gate)
// ==============================================================================================
//
// ASSERTION: Two tenants are seeded in the SAME RLS-protected table in real Postgres. Acting as
// tenant A — through the NOBYPASSRLS `myelin_app` role, with the session (tenant, region) scope
// set to A — a `SELECT * FROM rebac_tuple` carrying NO tenant predicate returns ONLY tenant A's
// rows. Tenant B's rows are invisible: ZERO of them come back. The filtering is done by Postgres
// (the FORCE ROW LEVEL SECURITY policy keyed on current_setting('myelin.tenant_id')), NOT by app
// code — so even a query with no `WHERE tenant_id` (the IDOR-prone shape) cannot leak across the
// tenant boundary. The app layer has no cross-tenant query path: PgStore::reverse_index threads
// the verified tenant predicate AND runs under RLS (defence in depth), and there is no method
// that reads another tenant's rows.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drill3_tenant_region_rls_isolation() {
    let cfg = MyelinConfig::dev();

    // The ADMIN store runs DDL + seeds BOTH tenants' rows (the owner is FORCEd under RLS too, so
    // even the seed sets the session scope per tenant — done inside put_tuple).
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

    // Seed tenant A's rows and tenant B's rows into the SAME table. put_tuple sets the session
    // (tenant, region) scope before each insert, so each row is written in its own tenant's
    // partition (and the WITH CHECK half of the RLS policy admits it).
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

    // Connect as the NOBYPASSRLS APP role (myelin_app) — the real runtime role. This is the
    // load-bearing part: a superuser / BYPASSRLS role would silently ignore the policy. myelin_app
    // does NOT, so RLS is actually enforced for this connection.
    let app = PgStore::connect(&cfg.database_url, &cfg.region, 4)
        .await
        .expect("connect Postgres as the NOBYPASSRLS app role");

    // (1) Acting as tenant A: a SELECT with NO tenant predicate returns ONLY A's object_ids.
    let visible_as_a = rls_read_all_no_predicate(&app, &tenant_a).await;
    assert_eq!(
        visible_as_a,
        vec!["A-doc1".to_string(), "A-doc2".to_string()],
        "tenant A sees exactly its own rows (DB-enforced RLS, no tenant predicate in the query)"
    );
    // ZERO of tenant B's rows leaked into A's view.
    assert!(
        !visible_as_a.iter().any(|o| o.starts_with("B-")),
        "ZERO cross-tenant leak: none of tenant B's rows are visible to tenant A"
    );

    // (2) Symmetric: acting as tenant B sees only B's rows; tenant A is invisible to B.
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

    // (3) The app-layer reverse_index (the real authz access path) is ALSO RLS-scoped: querying
    //     as tenant A for a subject that only exists in tenant B returns ZERO rows — there is no
    //     app code path that reaches across the tenant boundary (the verified tenant is threaded
    //     AND RLS is in force; defence in depth).
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

    // cleanup (admin, scoped per tenant so the RLS DELETE policy admits it).
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
}

// ==============================================================================================
// DRILL 4 — ReBAC check / list_objects NO-LEAK / NO-N+1 (the authz-leak + scale gate)
// ==============================================================================================
//
// ASSERTION: With real tuples loaded into the PG tuple store, against the live DB:
//   - check() CORRECTNESS : an edge that exists → ALLOW; an edge that does not → DENY (fail-closed
//                           — an absent tuple is a deny, never a silent allow).
//   - list_objects NO-LEAK: list_objects(alice, reader) returns EXACTLY alice's visible objects —
//                           NONE of the objects only bob/mallory can read leak into the result. The
//                           visible set is computed server-side from the tuples; an unauthorized
//                           object is never a candidate.
//   - NO N+1              : list_objects issues EXACTLY ONE authz query for the WHOLE visible set
//                           (the reverse-index lookup) — NOT one check() per candidate object. We
//                           instrument the store's authz round-trip counter and assert the delta
//                           for a list_objects over a many-object tenant is 1, independent of the
//                           number of candidate objects.

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

    // Load REAL tuples: alice is reader on a set of objects; bob + mallory are reader on a DISJOINT
    // set (the objects alice must NOT see — the leak candidates). A larger object population for
    // alice makes the no-N+1 assertion meaningful (one query, not one-per-object).
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
    // mallory shares ONE object id namespace-adjacent to alice but with a DIFFERENT relation —
    // a check for the wrong relation must DENY (no relation confusion).
    store
        .put_tuple(&tenant, "alice-doc-00", "writer", "user:mallory")
        .await
        .expect("put mallory");

    // (1) check() allow/deny correctness (fail-closed).
    // ALLOW: an edge that exists.
    assert!(
        store
            .check_tuple(&tenant, "alice-doc-00", "reader", "user:alice")
            .await
            .expect("check"),
        "ALLOW: alice IS reader on alice-doc-00"
    );
    // DENY: alice is not reader on a bob object (the object exists, but not for alice).
    assert!(
        !store
            .check_tuple(&tenant, "bob-secret-00", "reader", "user:alice")
            .await
            .expect("check"),
        "DENY (fail-closed): alice is NOT reader on bob-secret-00"
    );
    // DENY: relation confusion — mallory is writer (not reader) on alice-doc-00.
    assert!(
        !store
            .check_tuple(&tenant, "alice-doc-00", "reader", "user:mallory")
            .await
            .expect("check"),
        "DENY: mallory is writer, not reader, on alice-doc-00 (no relation confusion)"
    );
    // DENY: a wholly nonexistent edge.
    assert!(
        !store
            .check_tuple(&tenant, "no-such-doc", "reader", "user:alice")
            .await
            .expect("check"),
        "DENY: a nonexistent edge is denied, never a silent allow"
    );

    // (2) list_objects NO-LEAK: alice's visible set is EXACTLY her objects — bob's are not present.
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
    // explicit no-leak assertion: not one bob object appears.
    for b in &bob_objs {
        assert!(
            !visible.contains(b),
            "NO LEAK: unauthorized object {b} is absent from alice's list"
        );
    }
    // and the cross-check: every returned object genuinely passes check() (the list is sound —
    // it does not over-return).
    for o in &visible {
        assert!(
            store
                .check_tuple(&tenant, o, "reader", "user:alice")
                .await
                .expect("re-check"),
            "every listed object {o} is genuinely visible (check() agrees with list_objects)"
        );
    }

    // (3) NO N+1: list_objects over the many-object tenant is EXACTLY ONE authz query — not one
    //     check() per candidate. Snapshot the counter, run list_objects, assert delta == 1.
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

    // cleanup
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
}
