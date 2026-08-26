#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use myelin_config::MyelinConfig;
use myelin_storage::outbox_durable::PgOutboxBacking;
use myelin_storage::pgrelay::PgRelay;

use myelin_events::relay::{InProcessBus, Relay, DEFAULT_DRAIN_BATCH};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EmitContextBase,
    EventDraft, EventEnvelope, EventId, EventType, IdMinter, MonotonicMinter, OutboxRow,
    OutboxStore, OutboxTx, Timestamp, Ulid, Visibility, MAX_PUBLISH_ATTEMPTS, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

use sqlx::postgres::PgPool;

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn db_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
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

fn staged_row(id: &str, aggregate: &str) -> OutboxRow {
    let subject = ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}"));
    let aggregate = AggregateKey(aggregate.into());
    let envelope = EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.updated".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: subject.clone(),
        aggregate: aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-18T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-18T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": subject.0 }),
    };
    OutboxRow {
        event_id: envelope.event_id.clone(),
        aggregate,
        seq: 0,
        subject,
        envelope,
        published_at: None,
        attempts: 0,
    }
}

struct StuckMinter(String);
impl IdMinter for StuckMinter {
    fn mint(&self) -> Ulid {
        Ulid(self.0.clone())
    }
}

fn key(r: &OutboxRow) -> (String, String, u64) {
    (r.event_id.0.clone(), r.aggregate.0.clone(), r.seq)
}

async fn ensure_foundation(pool: &PgPool) {
    sqlx::raw_sql(OUTBOX_MIGRATION)
        .execute(pool)
        .await
        .expect("apply OUTBOX_MIGRATION");
}

async fn fresh_durable_store(pool: &PgPool) -> OutboxStore {
    sqlx::query("TRUNCATE outbox")
        .execute(pool)
        .await
        .expect("truncate outbox");
    let backing = PgOutboxBacking::new(pool.clone(), tokio::runtime::Handle::current());
    OutboxStore::durable(Arc::new(backing))
}

fn schema() -> String {
    format!("mr009b_outbox_{}", std::process::id())
}

fn setup_once() -> &'static tokio::sync::Mutex<bool> {
    static S: OnceLock<tokio::sync::Mutex<bool>> = OnceLock::new();
    S.get_or_init(|| tokio::sync::Mutex::new(false))
}

async fn connect() -> PgPool {
    let cfg = MyelinConfig::dev();
    let s = schema();
    let pool = {
        let s = s.clone();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |conn, _meta| {
                let s = s.clone();
                Box::pin(async move {
                    sqlx::Executor::execute(
                        conn,
                        format!("SET search_path TO {s}, public").as_str(),
                    )
                    .await?;
                    Ok(())
                })
            })
            .connect(&admin_url(&cfg))
            .await
            .expect("connect Postgres")
    };
    {
        let mut done = setup_once().lock().await;
        if !*done {
            sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {s}"))
                .execute(&pool)
                .await
                .expect("create per-pid schema");
            ensure_foundation(&pool).await;
            *done = true;
        }
    }
    pool
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn commit_read_drain_scenario(store: &OutboxStore) -> Vec<(String, String, u64)> {
    let m = minter();
    let mut tx = store.begin(m, ctx_base());
    tx.stage_state_change("state");
    let a0 = tx
        .emit(draft("issues.issue.created", "issue:A"), None)
        .unwrap();
    let b0 = tx
        .emit(draft("issues.issue.created", "issue:B"), None)
        .unwrap();
    let a1 = tx
        .emit(draft("issues.issue.updated", "issue:A"), None)
        .unwrap();
    assert_eq!(store.outbox_depth(), 0, "open tx wrote nothing");
    tx.commit().unwrap();

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
    let mem_keys = commit_read_drain_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    let dur_keys = commit_read_drain_scenario(&store);

    assert_eq!(
        mem_keys, dur_keys,
        "CDC parity: memory and durable committed_rows are identical (event_id, aggregate, seq)"
    );
    eprintln!("PARITY OK - commit/reads/drain identical across memory + real-PG backends");
}

fn abort_stages_nothing_scenario(store: &OutboxStore) {
    {
        let mut tx = store.begin(minter(), ctx_base());
        tx.stage_state_change("aborted");
        tx.emit(draft("issues.issue.created", "issue:GHOST"), None)
            .unwrap();
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

fn duplicate_rejected_scenario(store: &OutboxStore) {
    let stuck: Arc<dyn IdMinter> = Arc::new(StuckMinter(format!("01JDUP{}", uniq())));

    let mut tx1 = store.begin(Arc::clone(&stuck), ctx_base());
    tx1.emit(draft("issues.issue.created", "issue:DUP"), None)
        .unwrap();
    tx1.commit().unwrap();
    assert_eq!(store.committed_count(), 1);

    let mut tx2 = store.begin(Arc::clone(&stuck), ctx_base());
    tx2.emit(draft("issues.issue.updated", "issue:DUP"), None)
        .unwrap();
    let err = tx2.commit().unwrap_err();
    assert!(
        err.0.contains("UNIQUE(event_id)") || err.0.contains("duplicate"),
        "duplicate event_id must ERROR the whole commit, got: {}",
        err.0
    );
    assert_eq!(
        store.outbox_depth(),
        1,
        "a rejected commit stages nothing new"
    );
    assert_eq!(store.committed_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_duplicate_event_id_rejected() {
    duplicate_rejected_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    duplicate_rejected_scenario(&store);
    eprintln!("PARITY OK - duplicate event_id REJECTED (not silently ignored) on both backends");
}

fn absorb_scenario(store: &OutboxStore) {
    let stuck: Arc<dyn IdMinter> = Arc::new(StuckMinter(format!("01JABS{}", uniq())));

    let mut tx1 = store.begin(Arc::clone(&stuck), ctx_base());
    tx1.emit(draft("issues.issue.created", "issue:ABS"), None)
        .unwrap();
    tx1.commit().unwrap();
    assert_eq!(store.committed_count(), 1);

    let mut tx2 = store.begin(Arc::clone(&stuck), ctx_base());
    tx2.emit(draft("issues.issue.created", "issue:ABS"), None)
        .unwrap();
    tx2.commit_absorb()
        .expect("H1: byte-identical deterministic re-emit is ABSORBED (no livelock)");
    assert_eq!(store.committed_count(), 1, "absorb added NO duplicate");

    let mut tx3 = store.begin(Arc::clone(&stuck), ctx_base());
    tx3.emit(draft("issues.issue.updated", "issue:ABS"), None)
        .unwrap();
    let err = tx3.commit_absorb().unwrap_err();
    assert!(
        err.0.contains("DIFFERENT payload") || err.0.contains("collision"),
        "a divergent payload under the same id must still REJECT, got: {}",
        err.0
    );
    assert_eq!(
        store.committed_count(),
        1,
        "the rejected divergent commit staged nothing new"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_absorb_mode_idempotent_but_rejects_divergent_payload() {
    absorb_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    absorb_scenario(&store);
    eprintln!(
        "PARITY OK (H1) - absorb-mode ABSORBS a byte-identical re-emit (no livelock) but REJECTS a \
         divergent-payload collision, on both backends"
    );
}

fn dead_letter_scenario(store: &OutboxStore) {
    let mut tx = store.begin(minter(), ctx_base());
    let id = tx
        .emit(draft("issues.issue.created", "issue:POISON"), None)
        .unwrap();
    tx.commit().unwrap();
    assert_eq!(store.outbox_depth(), 1);

    let bus = InProcessBus::new();
    bus.sever();
    let relay = Relay::new(store.clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    });

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
    assert_eq!(
        store.outbox_depth(),
        0,
        "the poison row left the unsent set"
    );
    assert_eq!(
        store.dead_letter_count(),
        1,
        "quarantined, not silently lost"
    );
    let dl = store.dead_letters();
    assert_eq!(dl.len(), 1);
    assert_eq!(
        dl[0].attempts, MAX_PUBLISH_ATTEMPTS,
        "attempts hit the bound"
    );
    assert_eq!(dl[0].event_id, id);
    assert!(store.row(&id).is_none(), "dead row absent from live rows");
    assert_eq!(
        store.committed_count(),
        0,
        "dead row not counted committed-live"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn cdc_parity_dead_letter_at_max_attempts() {
    dead_letter_scenario(&OutboxStore::new());

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    dead_letter_scenario(&store);
    eprintln!("PARITY OK - dead-letter at MAX_PUBLISH_ATTEMPTS identical across backends");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_full_poison_batch_does_not_hide_the_healthy_work_behind_it() {
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    let mut tx = store.begin(minter(), ctx_base());
    for index in 0..=DEFAULT_DRAIN_BATCH {
        tx.emit(
            draft("issues.issue.updated", &format!("issue:BATCH-{index:03}")),
            None,
        )
        .expect("stage one event in the ordered relay backlog");
    }
    tx.commit().expect("commit the whole relay backlog");

    let bus = InProcessBus::new();
    bus.sever();
    let relay = Relay::new(store.clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    });
    for attempt in 1..MAX_PUBLISH_ATTEMPTS {
        let report = relay.drain_once();
        assert_eq!(
            report.failed, DEFAULT_DRAIN_BATCH,
            "attempt {attempt} parks exactly the first bounded batch"
        );
        assert_eq!(report.dead_lettered, 0);
    }

    bus.heal();
    bus.fail_next(
        u32::try_from(DEFAULT_DRAIN_BATCH).expect("the relay batch fits the fault counter"),
    );
    let report = relay.drain_to_empty();

    assert_eq!(
        report.dead_lettered, DEFAULT_DRAIN_BATCH,
        "the receipt accounts for every poison row removed from the live queue"
    );
    assert_eq!(
        report.published, 1,
        "the healthy event behind a full poison batch is still delivered"
    );
    assert_eq!(store.try_outbox_depth().unwrap(), 0, "drain means empty");
    assert_eq!(
        store.try_dead_letter_count().unwrap(),
        DEFAULT_DRAIN_BATCH,
        "all poison rows remain available for operator recovery"
    );
    assert_eq!(bus.delivered_count(), 1, "the healthy event arrives once");
}

async fn concurrent_seq_scenario(store: &OutboxStore, n: u64) {
    let m = minter();
    let hot = "issue:HOT";
    let mut handles = Vec::new();
    for _ in 0..n {
        let store = store.clone();
        let m = Arc::clone(&m);
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
    concurrent_seq_scenario(&OutboxStore::new(), 32).await;

    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;
    concurrent_seq_scenario(&store, 32).await;
    eprintln!("EB-03 OK (DURABLE) - gap-free per-aggregate seq under 32 concurrent PG committers");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_outbox_outage_is_retryable_state_not_a_process_panic() {
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;

    let mut tx = store.begin(minter(), ctx_base());
    let event_id = tx
        .emit(draft("issues.issue.created", "issue:OUTAGE"), None)
        .expect("stage the event");
    tx.commit().expect("the event is durable before the outage");
    assert_eq!(store.try_outbox_depth().unwrap(), 1);
    assert!(store
        .try_retained_rows_bounded(0, 10_000)
        .expect_err("the row ceiling is enforced in storage")
        .0
        .contains("row limit"));
    assert!(store
        .try_retained_rows_bounded(1, 0)
        .expect_err("the envelope ceiling is enforced in storage")
        .0
        .contains("envelope byte limit"));

    pool.close().await;

    let unavailable = [
        store.try_outbox_depth().map(|_| ()),
        store.try_dead_letter_count().map(|_| ()),
        store.try_oldest_unsent_recorded_at().map(|_| ()),
        store.try_committed_count().map(|_| ()),
        store.try_row(&event_id).map(|_| ()),
        store.try_committed_rows().map(|_| ()),
        store.try_retained_rows().map(|_| ()),
        store.try_retained_rows_bounded(10, 10_000).map(|_| ()),
        store.try_dead_letters().map(|_| ()),
    ];
    for read in unavailable {
        let error = read.expect_err("a closed pool leaves outbox state unknown");
        assert!(error.0.contains("unavailable"));
        assert!(error.0.contains("state is unknown"));
        assert!(
            !error.0.contains("pool") && !error.0.contains("database"),
            "storage diagnostics stay payload-free: {}",
            error.0
        );
    }

    let bus = InProcessBus::new();
    let relay = Relay::new(store, bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    });
    let interrupted = relay.drain_to_empty();
    assert_eq!(interrupted.published, 0);
    assert_eq!(interrupted.drain_errors, 1);
    assert_eq!(bus.delivered_count(), 0, "unknown work is never invented");

    let recovered_pool = connect().await;
    let recovered = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        recovered_pool,
        tokio::runtime::Handle::current(),
    )));
    assert_eq!(
        recovered.try_outbox_depth().unwrap(),
        1,
        "a fresh worker sees the pending event after storage recovers"
    );
    let recovered_bus = InProcessBus::new();
    let recovered_relay = Relay::new(recovered.clone(), recovered_bus.clone(), || {
        Timestamp("2026-06-19T00:00:10Z".into())
    });
    let completed = recovered_relay.drain_to_empty();
    assert_eq!(completed.published, 1);
    assert_eq!(completed.drain_errors, 0);
    assert_eq!(recovered_bus.delivered_count(), 1);
    assert_eq!(recovered.try_outbox_depth().unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn domain_batch_and_ordinary_staged_writer_share_the_same_aggregate_lock() {
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let _store = fresh_durable_store(&pool).await;
    let tag = uniq().replace('-', "_");
    let domain_table = format!("outbox_domain_race_{tag}");
    sqlx::query(&format!(
        "CREATE TABLE {domain_table} (id text PRIMARY KEY)"
    ))
    .execute(&pool)
    .await
    .expect("create domain state table");

    let aggregate = format!("issue:DOMAIN-RACE-{tag}");
    let domain_row = staged_row(&format!("domain-{tag}"), &aggregate);
    let ordinary_row = staged_row(&format!("ordinary-{tag}"), &aggregate);
    let mut domain_tx = pool.begin().await.expect("begin domain transaction");
    sqlx::query(&format!(
        "INSERT INTO {domain_table} (id) VALUES ('committed')"
    ))
    .execute(&mut *domain_tx)
    .await
    .expect("stage domain state");
    PgRelay::co_commit_rows_in_tx(&mut domain_tx, &[domain_row])
        .await
        .expect("stage domain outbox row while holding the aggregate lock");

    let relay = PgRelay::new(pool.clone());
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let ordinary = tokio::spawn(async move {
        let _ = entered_tx.send(());
        relay.commit_staged_atomic(&[ordinary_row]).await
    });
    entered_rx.await.expect("ordinary writer entered");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !ordinary.is_finished(),
        "the ordinary staged writer waits for the domain writer's aggregate lock"
    );

    domain_tx
        .commit()
        .await
        .expect("domain state and its outbox row commit together");
    ordinary
        .await
        .expect("ordinary writer task")
        .expect("ordinary writer commits after the domain transaction");

    let domain_count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {domain_table}"))
        .fetch_one(&pool)
        .await
        .expect("count domain state");
    assert_eq!(
        domain_count, 1,
        "the domain state was not partially rolled back"
    );
    let seqs: Vec<i64> =
        sqlx::query_scalar("SELECT seq FROM outbox WHERE aggregate = $1 ORDER BY seq")
            .bind(&aggregate)
            .fetch_all(&pool)
            .await
            .expect("read raced aggregate sequence");
    assert_eq!(
        seqs,
        [0, 1],
        "both writers commit with contiguous distinct seqs"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn crash_window_republish_is_idempotent_zero_ghost_zero_lost() {
    let pool = connect().await;
    let _guard = db_lock().lock().await;
    let store = fresh_durable_store(&pool).await;

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

    let published_before_crash = relay
        .relay_once_crash_after(&bus, 16, 2)
        .await
        .expect("crash drain");
    assert_eq!(published_before_crash, 2, "2 forwarded before the crash");
    assert_eq!(
        store.outbox_depth(),
        4,
        "0 lost - crash rolled back the marks"
    );
    assert_eq!(
        bus.delivered_count(),
        2,
        "2 delivered to the broker pre-crash"
    );

    let report = Relay::new(store.clone(), bus.clone(), || {
        Timestamp("2026-06-19T00:00:09Z".into())
    })
    .drain_to_empty();
    assert_eq!(
        report.deduplicated, 2,
        "the 2 re-claims were deduplicated (0 ghost)"
    );
    assert_eq!(report.published, 2, "the other 2 freshly published");
    assert_eq!(report.drain_errors, 0);
    assert_eq!(store.outbox_depth(), 0, "depth drains after restart");
    assert_eq!(
        bus.delivered_count(),
        4,
        "exactly 4 delivered - 0 ghost / 0 lost across the crash window"
    );
    let delivered = bus.delivered_ids();
    for id in &ids {
        assert!(
            delivered.contains(id),
            "every committed event was delivered"
        );
    }
    eprintln!("SUB-D1 OK (DURABLE) - crash-window re-publish idempotent: 0 ghost / 0 lost");
}
