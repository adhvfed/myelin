//! # MR-009b W3b.2 — the durable `outbox` backing (`PgOutboxBacking`), proven against LIVE Postgres.
//!
//! The **CDC PARITY SUITE** the W3b design contract requires: ONE set of observable-behavior
//! assertions run against BOTH backends — the in-memory `OutboxStore::new()` and the durable
//! `OutboxStore::durable(PgOutboxBacking)` over real Postgres — asserting they are
//! byte-for-byte-equivalent at the seam. Plus the durable-only live gates the memory model cannot
//! reach: gap-free per-aggregate seq under CONCURRENT committers (EB-03, durably), and
//! crash-window re-publish idempotency (0 ghost / 0 lost across a killed relay tx).
//!
//! Proven behaviors (each asserted for BOTH backends unless marked durable-only):
//!   1. commit atomicity — an aborted (dropped, un-committed) transaction stages NOTHING;
//!   2. duplicate-`event_id` REJECTION — a duplicate id ERRORS the whole commit (not
//!      `ON CONFLICT DO NOTHING`), staging nothing (the W3b.1 verifier's open question, resolved);
//!   3. per-aggregate seq gap-free + true-commit-order, incl. under CONCURRENT committers (EB-03);
//!   4. depth / age / committed_rows / committed_count read parity;
//!   5. drain claim → publish → mark-sent → dead-letter counts vs `MAX_PUBLISH_ATTEMPTS`;
//!   6. crash-window re-publish idempotency (durable-only; mirrors `relay_once_crash_after`).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test integration_mr009b_outbox_durable -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use myelin_config::MyelinConfig;
use myelin_storage::outbox_durable::PgOutboxBacking;
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::relay::{InProcessBus, Relay};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, MonotonicMinter, OutboxRow, OutboxStore, OutboxTx, Timestamp, Ulid, Visibility,
    MAX_PUBLISH_ATTEMPTS, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

use sqlx::postgres::PgPool;

// ----------------------------------------------------------------------------------------------
// shared helpers
// ----------------------------------------------------------------------------------------------

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Serializes the durable tests (they share the ONE `outbox` table and TRUNCATE it for isolation).
fn db_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn draft(type_: &str, aggregate: &str) -> EventDraft {
    EventDraft {
        type_: EventType(type_.into()),
        subject: ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": aggregate }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// A minter that ALWAYS returns the same ULID — forces a duplicate `event_id` for the rejection
/// parity test (the production `MonotonicMinter` can never collide).
struct StuckMinter(String);
impl IdMinter for StuckMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.clone())
    }
}

/// The structural identity of a row (ignores `published_at`, which differs by wall-clock between
/// the memory arm's injected test clock and the durable arm's `now()`).
fn key(r: &OutboxRow) -> (String, String, u64) {
    (r.event_id.0.clone(), r.aggregate.0.clone(), r.seq)
}

async fn ensure_foundation(pool: &PgPool) {
    sqlx::raw_sql(OUTBOX_MIGRATION)
        .execute(pool)
        .await
        .expect("apply OUTBOX_MIGRATION");
}

/// A FRESH durable store over a truncated `outbox` table (call while holding `db_lock`).
async fn fresh_durable_store(pool: &PgPool) -> OutboxStore {
    sqlx::query("TRUNCATE outbox")
        .execute(pool)
        .await
        .expect("truncate outbox");
    let backing = PgOutboxBacking::new(pool.clone(), tokio::runtime::Handle::current());
    OutboxStore::durable(Arc::new(backing))
}

async fn connect() -> PgPool {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 8)
        .await
        .expect("connect Postgres");
    ensure_foundation(&pool).await;
    pool
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

// ----------------------------------------------------------------------------------------------
// (1) commit atomicity + (4) read parity + drain-to-empty — asserted for BOTH backends.
// ----------------------------------------------------------------------------------------------

/// The full commit → read → drain scenario, written once against the public `OutboxStore` API so it
/// runs UNCHANGED on the memory arm and the durable arm. Returns the observed committed-row keys so
/// the caller can assert cross-backend parity.
fn commit_read_drain_scenario(store: &OutboxStore) -> Vec<(String, String, u64)> {
    let m = minter();
    // Commit 2 events to aggregate A and 1 to aggregate B in ONE transaction.
    let mut tx = store.begin(m, ctx_base());
    tx.stage_state_change("state");
    let a0 = tx.emit(draft("issues.issue.created", "issue:A"), None).unwrap();
    let b0 = tx.emit(draft("issues.issue.created", "issue:B"), None).unwrap();
    let a1 = tx.emit(draft("issues.issue.updated", "issue:A"), None).unwrap();
    // emit-iff-committed: nothing durable before commit.
    assert_eq!(store.outbox_depth(), 0, "open tx wrote nothing");
    tx.commit().unwrap();

    // reads: depth, committed_count, per-aggregate gap-free seq.
    assert_eq!(store.outbox_depth(), 3);
    assert_eq!(store.committed_count(), 3);
    assert_eq!(store.row(&a0).unwrap().seq, 0, "A seq 0");
    assert_eq!(store.row(&a1).unwrap().seq, 1, "A seq 1 (gap-free)");
    assert_eq!(store.row(&b0).unwrap().seq, 0, "B seq 0 (per-aggregate)");
    assert_eq!(
        store.oldest_unsent_recorded_at(),
        Some(Timestamp("2026-06-19T00:00:01Z".into())),
        "oldest-unsent age anchor"
    );
    assert_eq!(store.dead_letter_count(), 0);

    // drain to a reachable broker: every row delivered exactly once, depth → 0.
    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    });
    let report = relay.drain_to_empty();
    assert_eq!(report.published, 3, "3 published exactly once");
    assert_eq!(report.drain_errors, 0, "no drain errors");
    assert_eq!(store.outbox_depth(), 0, "depth drains to 0");
    assert_eq!(bus.delivered_count(), 3, "0 ghost / 0 lost");

    let mut keys: Vec<_> = store.committed_rows().iter().map(key).collect();
    keys.sort();
    keys
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_commit_reads_and_drain() {
    // memory arm.
    let mem_keys = commit_read_drain_scenario(&OutboxStore::new());

    // durable arm (real PG) — identical observable behavior.
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    let dur_keys = commit_read_drain_scenario(&store);

    assert_eq!(
        mem_keys, dur_keys,
        "CDC parity: memory and durable committed_rows are identical (event_id, aggregate, seq)"
    );
    eprintln!("PARITY OK — commit/reads/drain identical across memory + real-PG backends");
}

// ----------------------------------------------------------------------------------------------
// (1) abort stages nothing — BOTH backends.
// ----------------------------------------------------------------------------------------------

fn abort_stages_nothing_scenario(store: &OutboxStore) {
    {
        let mut tx = store.begin(minter(), ctx_base());
        tx.stage_state_change("aborted");
        tx.emit(draft("issues.issue.created", "issue:GHOST"), None)
            .unwrap();
        // dropped WITHOUT commit.
    }
    assert_eq!(store.outbox_depth(), 0, "aborted tx wrote no event");
    assert_eq!(store.committed_count(), 0, "no ghost row");
    assert_eq!(store.dead_letter_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_abort_stages_nothing() {
    abort_stages_nothing_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    abort_stages_nothing_scenario(&store);
}

// ----------------------------------------------------------------------------------------------
// (2) duplicate-event_id REJECTION (error, not ON CONFLICT DO NOTHING) — BOTH backends.
// ----------------------------------------------------------------------------------------------

fn duplicate_rejected_scenario(store: &OutboxStore) {
    // The realistic duplicate-`event_id` case both arms share: a SECOND transaction re-emits an
    // id that a FIRST transaction already durably committed. (The in-memory arm's uniqueness check
    // is against already-committed rows, `Inner::rows`; a within-one-batch duplicate is a
    // programming error the monotonic minter makes impossible and the arm does not screen. So the
    // meaningful, observable parity is the cross-transaction duplicate → both REJECT.)
    let stuck: Arc<dyn IdMinter> = Arc::new(StuckMinter(format!("01JDUP{}", uniq())));

    // tx1: commit id X.
    let mut tx1 = store.begin(Arc::clone(&stuck), ctx_base());
    tx1.emit(draft("issues.issue.created", "issue:DUP"), None)
        .unwrap();
    tx1.commit().unwrap();
    assert_eq!(store.committed_count(), 1);

    // tx2: re-emit the SAME id X → the whole commit is REJECTED (not ON CONFLICT DO NOTHING).
    let mut tx2 = store.begin(Arc::clone(&stuck), ctx_base());
    tx2.emit(draft("issues.issue.updated", "issue:DUP"), None)
        .unwrap();
    let err = tx2.commit().unwrap_err();
    assert!(
        err.0.contains("UNIQUE(event_id)") || err.0.contains("duplicate"),
        "duplicate event_id must ERROR the whole commit, got: {}",
        err.0
    );
    // atomicity: the rejected commit staged NOTHING new — the original row is untouched.
    assert_eq!(store.outbox_depth(), 1, "a rejected commit stages nothing new");
    assert_eq!(store.committed_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_duplicate_event_id_rejected() {
    duplicate_rejected_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    duplicate_rejected_scenario(&store);
    eprintln!("PARITY OK — duplicate event_id REJECTED (not silently ignored) on both backends");
}

// ----------------------------------------------------------------------------------------------
// (5) bounded retries → dead-letter at MAX_PUBLISH_ATTEMPTS — BOTH backends.
// ----------------------------------------------------------------------------------------------

fn dead_letter_scenario(store: &OutboxStore) {
    let mut tx = store.begin(minter(), ctx_base());
    let id = tx
        .emit(draft("issues.issue.created", "issue:POISON"), None)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(store.outbox_depth(), 1);

    let bus = InProcessBus::new();
    bus.sever(); // broker permanently down for this row.
    let relay = Relay::new(store.clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    });

    // Each pass is one failed attempt; the MAX_PUBLISH_ATTEMPTS-th pass dead-letters.
    for pass in 1..=MAX_PUBLISH_ATTEMPTS {
        let r = relay.drain_once();
        if pass < MAX_PUBLISH_ATTEMPTS {
            assert_eq!(r.failed, 1, "pass {pass}: one failed under the bound");
            assert_eq!(r.dead_lettered, 0);
        } else {
            assert_eq!(r.dead_lettered, 1, "the bound-th pass dead-letters");
            assert_eq!(r.failed, 0);
        }
    }
    assert_eq!(store.outbox_depth(), 0, "the poison row left the unsent set");
    assert_eq!(store.dead_letter_count(), 1, "quarantined, not silently lost");
    let dl = store.dead_letters();
    assert_eq!(dl.len(), 1);
    assert_eq!(dl[0].attempts, MAX_PUBLISH_ATTEMPTS, "attempts hit the bound");
    assert_eq!(dl[0].event_id, id);
    // a dead-lettered row reads as absent from the live set (parity with the memory arm).
    assert!(store.row(&id).is_none(), "dead row absent from live rows");
    assert_eq!(store.committed_count(), 0, "dead row not counted committed-live");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_dead_letter_at_max_attempts() {
    dead_letter_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    dead_letter_scenario(&store);
    eprintln!("PARITY OK — dead-letter at MAX_PUBLISH_ATTEMPTS identical across backends");
}

// ----------------------------------------------------------------------------------------------
// (3) EB-03 — per-aggregate seq gap-free + no-dup under CONCURRENT committers — BOTH backends.
// ----------------------------------------------------------------------------------------------

async fn concurrent_seq_scenario(store: &OutboxStore, n: u64) {
    let m = minter();
    let hot = "issue:HOT";
    let mut handles = Vec::new();
    for _ in 0..n {
        let store = store.clone();
        let m = Arc::clone(&m);
        // A spawned task runs on a runtime worker, so the durable arm's internal
        // block_in_place + block_on bridge is valid (the sanctioned sync→async pattern).
        handles.push(tokio::spawn(async move {
            let mut tx = store.begin(m, ctx_base());
            let id = tx.emit(draft("issues.issue.updated", hot), None).unwrap();
            tx.commit().unwrap();
            id
        }));
    }
    let mut seqs = Vec::new();
    for h in handles {
        let id = h.await.unwrap();
        seqs.push(store.row(&id).unwrap().seq);
    }
    seqs.sort_unstable();
    let expected: Vec<u64> = (0..n).collect();
    assert_eq!(
        seqs, expected,
        "concurrent committers → contiguous, unique seqs {{0..{n}}} (no gap, no dup)"
    );
    assert_eq!(store.committed_count(), n as usize);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn eb03_seq_gap_free_under_concurrent_committers_both_backends() {
    // memory arm (the events unit test proves 64; re-prove through this harness at 32).
    concurrent_seq_scenario(&OutboxStore::new(), 32).await;

    // durable arm — the EB-03 gate re-run DURABLY against real PG under true concurrency.
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    concurrent_seq_scenario(&store, 32).await;
    eprintln!("EB-03 OK (DURABLE) — gap-free per-aggregate seq under 32 concurrent PG committers");
}

// ----------------------------------------------------------------------------------------------
// (6) crash-window re-publish idempotency (durable-only) — mirrors relay_once_crash_after.
// ----------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn crash_window_republish_is_idempotent_zero_ghost_zero_lost() {
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;

    // Commit 4 events through the durable store (co-committed, emit-iff-committed).
    let m = minter();
    let mut tx = store.begin(m, ctx_base());
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push(
            tx.emit(draft(&format!("issues.issue.e{i}"), "issue:CRASH"), None)
                .unwrap(),
        );
    }
    tx.commit().unwrap();
    assert_eq!(store.outbox_depth(), 4);

    let bus = InProcessBus::new();
    let relay = PgRelay::new(pool.clone());

    // CRASH mid-drain: publish 2 rows to the broker, then drop the tx WITHOUT committing the
    // published_at marks (the worst-case silent-data-loss window).
    let published_before_crash = relay
        .relay_once_crash_after(&bus, 16, 2)
        .await
        .expect("crash drain");
    assert_eq!(published_before_crash, 2, "2 forwarded before the crash");
    // The marks rolled back: all 4 rows are still unsent (0 lost).
    assert_eq!(store.outbox_depth(), 4, "0 lost — crash rolled back the marks");
    assert_eq!(bus.delivered_count(), 2, "2 delivered to the broker pre-crash");

    // RESTART: drain again. The 2 re-claimed already-delivered rows are DEDUPLICATED (0 ghost),
    // the other 2 are freshly published — every committed event delivered exactly once.
    let report = Relay::new(store.clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    })
    .drain_to_empty();
    assert_eq!(report.deduplicated, 2, "the 2 re-claims were deduplicated (0 ghost)");
    assert_eq!(report.published, 2, "the other 2 freshly published");
    assert_eq!(report.drain_errors, 0);
    assert_eq!(store.outbox_depth(), 0, "depth drains after restart");
    assert_eq!(
        bus.delivered_count(),
        4,
        "exactly 4 delivered — 0 ghost / 0 lost across the crash window"
    );
    let delivered = bus.delivered_ids();
    for id in &ids {
        assert!(delivered.contains(id), "every committed event was delivered");
    }
    eprintln!("SUB-D1 OK (DURABLE) — crash-window re-publish idempotent: 0 ghost / 0 lost");
}
