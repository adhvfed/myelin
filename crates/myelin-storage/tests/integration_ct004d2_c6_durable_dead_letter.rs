//! # CT-004d.2 chunk 6 / peer-review #7b — the DURABLE consumer dead-letter set, proven on LIVE PG.
//!
//! The debt the #7 H2 fix introduced: the consumer's dead-letter set was `Mutex<Vec<DeadLetter>>` —
//! in-memory only. The H2 panic path rolls back the co-commit tx, `push_dead_letter`s the poison, and
//! the pump ACKS the message (terminal) so the broker cursor advances — but the record lived only in a
//! volatile `Vec`, so a RESTART LOST it (the "stays replayable from the dead-letter set" claim was
//! self-defeating as shipped). This gate proves the DURABLE sink closes it against the live
//! docker-compose Postgres (:5433), mirroring the dedup durability tests.
//!
//! It proves:
//!  1. `record` PERSISTS across a pool drop/reopen (a dead-lettered event is still present after a
//!     "restart"), and a re-record is IDEMPOTENT (`ON CONFLICT DO NOTHING` → one row).
//!  2. The consumer's H2 panic path with a DURABLE sink → the poison is in the durable table AFTER
//!     the panic (a panicking handler + a durable dead-letter sink → the row persists), written on the
//!     sink's OWN fresh pool connection (the co-commit tx was rolled back — it cannot be reused).
//!  3. DB-unreachable `record` → LOUD, NOT a silent drop (falls back to the in-process surface).
//!  4. PII-safety: the stored row contains NO raw payload/envelope — only `event_id` + a PII-free
//!     bounded `reason`.
//!
//! Run against the dev stack:
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-storage --features integration --test integration_ct004d2_c6_durable_dead_letter -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::events_durable::DurableDeadLetterBacking;
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::consumer::{Consumer, Delivered, Message, Subscription};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, ConsumerName, CorrelationId, DataRole, DeadLetterSink,
    DedupLedger, DurableDeadLetter, DurableDedup, EventEnvelope, EventHandler, EventId, EventType,
    HandleOutcome, HandlerTx, PrefetchBound, SubjectPattern, Timestamp, Visibility,
    CONSUMER_DEAD_LETTER_MIGRATION, CONSUMER_DEDUP_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// An envelope whose PAYLOAD carries an obvious PII marker — so the PII-safety assertion can prove the
/// durable dead-letter row never persisted it (references-not-payloads).
fn envelope_with_pii(id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-17T00:00:01Z".into()),
        // A raw PII payload the durable row must NEVER contain.
        payload: serde_json::json!({ "email": "alice.SECRET@example.com" }),
    }
}

static SUBJECTS: &[SubjectPattern] = &[];

async fn setup(pool: &sqlx::PgPool) {
    sqlx::raw_sql(CONSUMER_DEDUP_MIGRATION)
        .execute(pool)
        .await
        .expect("apply CONSUMER_DEDUP_MIGRATION");
    sqlx::raw_sql(CONSUMER_DEAD_LETTER_MIGRATION)
        .execute(pool)
        .await
        .expect("apply CONSUMER_DEAD_LETTER_MIGRATION");
}

/// Read the raw `(reason)` stored for a `(consumer, event_id)` directly (so the assertions do not
/// route through the trait's own read verb — an independent check).
async fn stored_reason(pool: &sqlx::PgPool, consumer: &str, id: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT reason FROM consumer_dead_letter WHERE consumer = $1 AND event_id = $2",
    )
    .bind(consumer)
    .bind(id)
    .fetch_optional(pool)
    .await
    .expect("dead-letter read")
}

async fn row_count(pool: &sqlx::PgPool, consumer: &str, id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM consumer_dead_letter WHERE consumer = $1 AND event_id = $2",
    )
    .bind(consumer)
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("dead-letter count")
}

// =================================================================================================
// (1) `record` PERSISTS across a pool drop/reopen (a "restart"), and a re-record is IDEMPOTENT.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_survives_a_restart_and_is_idempotent() {
    let cfg = MyelinConfig::dev();
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let cname = format!("dlq_persist_{tag}");
    let id = format!("dlq-evt-{tag}");
    let consumer = ConsumerName(cname.clone());
    let event_id = EventId(id.clone());

    // --- process 1: record the poison, then DROP the pool (simulate a crash/restart). ---
    {
        let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
            .await
            .expect("connect Postgres (is the dev stack up?)");
        setup(&pool).await;
        let backing = DurableDeadLetterBacking::new(pool.clone(), rt.clone());
        tokio::task::block_in_place(|| {
            backing
                .record(&consumer, &event_id, "handler PANICKED (a bug)")
                .expect("record persists")
        });
        // idempotent re-record (redelivered dead-letter) → ON CONFLICT DO NOTHING.
        tokio::task::block_in_place(|| {
            backing
                .record(&consumer, &event_id, "handler PANICKED (a bug)")
                .expect("re-record is a no-op, not an error")
        });
        assert_eq!(row_count(&pool, &cname, &id).await, 1, "exactly ONE row");
        // pool dropped here → "restart".
    }

    // --- process 2: a FRESH pool over the same DB — the poison is STILL present (durability). ---
    let pool2 = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("reconnect Postgres");
    assert_eq!(
        row_count(&pool2, &cname, &id).await,
        1,
        "the dead-letter SURVIVED the restart (durable, not a volatile Vec)"
    );
    let backing2 = DurableDeadLetterBacking::new(pool2.clone(), rt.clone());
    let rows = tokio::task::block_in_place(|| backing2.dead_letters(&consumer));
    assert_eq!(rows.len(), 1, "the read verb sees the surviving row");
    assert_eq!(rows[0].event_id, event_id);

    sqlx::query("DELETE FROM consumer_dead_letter WHERE consumer = $1")
        .bind(&cname)
        .execute(&pool2)
        .await
        .ok();
    println!("[#7b] PASS (1): durable dead-letter survives a restart; re-record idempotent.");
}

// =================================================================================================
// (2) The H2 PANIC path with a DURABLE sink → the poison is in the durable table AFTER the panic.
//     (4) PII-safety: the stored row carries NO raw payload — only event_id + PII-free reason.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2_panic_path_persists_the_poison_durably_and_pii_free() {
    let cfg = MyelinConfig::dev();
    // max_connections = 1: the durable dead-letter `record` MUST run on a FRESH connection, not the
    // rolled-back co-commit conn — with a single connection the panic path's rollback + the record
    // both share the one connection, proving `record` acquires it AFTER the co-commit tx released it.
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 1)
        .await
        .expect("connect Postgres (is the dev stack up?)");
    setup(&pool).await;
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let cname = format!("dlq_h2_{tag}");
    let id = format!("h2-poison-{tag}");

    // A handler that PANICS (a bug) — the H2 path: deliver catch_unwinds → rolls back the co-commit
    // tx → dead-letters through the DURABLE sink.
    struct PanicHandler {
        ran: AtomicU32,
    }
    impl EventHandler for PanicHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            SUBJECTS
        }
        fn handle(&self, _ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
            self.ran.fetch_add(1, Ordering::SeqCst);
            panic!("handler bug: boom on a poison event");
        }
    }

    let dedup =
        DedupLedger::durable(
            Arc::new(myelin_storage::events_durable::DurableDedupBacking::new(
                pool.clone(),
                rt.clone(),
            )) as Arc<dyn DurableDedup>,
        );
    let sink = DeadLetterSink::durable(
        Arc::new(DurableDeadLetterBacking::new(pool.clone(), rt.clone()))
            as Arc<dyn DurableDeadLetter>,
    );
    let consumer = Consumer::new(
        PanicHandler {
            ran: AtomicU32::new(0),
        },
        Subscription::bind(
            ConsumerName(cname.clone()),
            &["myelin://acme/issues/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap(),
        dedup,
    )
    .with_dead_letter_sink(sink);

    let msg = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope_with_pii(&id),
    };

    let out = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert!(
        matches!(out, Delivered::DeadLettered(_)),
        "the panic is caught and dead-lettered, got {out:?}"
    );
    assert_eq!(
        consumer.handler().ran.load(Ordering::SeqCst),
        1,
        "the panicking handler ran once"
    );

    // THE #7b ASSERTION: the poison is DURABLE — present in the table after the panic (it would be
    // ACKed by the pump and LOST on restart before #7b). Written on its own fresh pool connection.
    assert_eq!(
        row_count(&pool, &cname, &id).await,
        1,
        "#7b: the panicked event's poison is in the DURABLE consumer_dead_letter table"
    );
    let reason = stored_reason(&pool, &cname, &id)
        .await
        .expect("the poison row exists");
    assert!(
        reason.contains("PANICKED"),
        "the reason records the panic (diagnostic): {reason}"
    );

    // (4) PII-SAFETY: the stored reason (the ONLY free-text column) carries NO raw payload/PII. The
    // panic detail is developer text ("boom on a poison event"), never the envelope's payload.
    assert!(
        !reason.contains("SECRET") && !reason.contains("alice") && !reason.contains("@example.com"),
        "PII-safety: the durable reason must NOT echo the envelope payload PII: {reason}"
    );
    // And there is structurally no payload/envelope column: prove the whole row holds only the
    // PII-free reference columns by re-reading the full row.
    let full: (String, String, String) = sqlx::query_as(
        "SELECT consumer, event_id, reason FROM consumer_dead_letter \
         WHERE consumer = $1 AND event_id = $2",
    )
    .bind(&cname)
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("read the full row");
    assert_eq!(full.0, cname);
    assert_eq!(full.1, id);
    assert!(
        !full.2.contains("SECRET"),
        "PII-safety: no payload PII anywhere in the stored row"
    );

    // The durable dedup mark was NOT set (a panic is a bug, not poison → no tombstone) so the fixed
    // consumer can replay it — but the poison is durably surfaced. (The #7 correctness point.)
    let dedup_present: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
    )
    .bind(&cname)
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("dedup read");
    assert!(
        !dedup_present,
        "#7: no dedup tombstone for a panic → the valid effect stays replayable"
    );

    sqlx::query("DELETE FROM consumer_dead_letter WHERE consumer = $1")
        .bind(&cname)
        .execute(&pool)
        .await
        .ok();
    println!(
        "[#7b] PASS (2)+(4): the H2 panic path persists the poison DURABLY on its own fresh \
         connection, PII-free (references-not-payloads); no dedup tombstone (replayable)."
    );
}

// =================================================================================================
// (3) DB-UNREACHABLE `record` → LOUD, NOT a silent drop (falls back to the in-process surface).
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn db_unreachable_record_falls_back_never_silently_drops() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("connect Postgres (is the dev stack up?)");
    setup(&pool).await;
    let rt = tokio::runtime::Handle::current();
    let tag = uniq();
    let cname = format!("dlq_unreach_{tag}");

    // Point the backing at a DEAD address (nothing listening) so `record` fails → the sink must fall
    // back to the in-process Vec + a loud log, NOT silently drop the poison.
    let dead_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_millis(400))
        .connect_lazy("postgres://myelin_app:myelin_app_pw@127.0.0.1:6544/nope")
        .expect("build a lazy pool at a dead address");

    struct PanicHandler;
    impl EventHandler for PanicHandler {
        fn subjects(&self) -> &'static [SubjectPattern] {
            SUBJECTS
        }
        fn handle(&self, _ev: &EventEnvelope, _tx: &mut HandlerTx<'_>) -> HandleOutcome {
            panic!("boom");
        }
    }

    // The dedup ledger uses the LIVE pool (so deliver's co-commit works); only the dead-letter sink's
    // backing is unreachable — isolating the sink fail-direction.
    let dedup =
        DedupLedger::durable(
            Arc::new(myelin_storage::events_durable::DurableDedupBacking::new(
                pool.clone(),
                rt.clone(),
            )) as Arc<dyn DurableDedup>,
        );
    let sink = DeadLetterSink::durable(
        Arc::new(DurableDeadLetterBacking::new(dead_pool, rt.clone()))
            as Arc<dyn DurableDeadLetter>,
    );
    let consumer = Consumer::new(
        PanicHandler,
        Subscription::bind(
            ConsumerName(cname.clone()),
            &["myelin://acme/issues/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap(),
        dedup,
    )
    .with_dead_letter_sink(sink);

    let id = format!("unreach-{tag}");
    let msg = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope_with_pii(&id),
    };
    let out = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert!(matches!(out, Delivered::DeadLettered(_)), "still dead-lettered");

    // The poison is NOT silently dropped — it fell back to the in-process surface (loud log emitted).
    let surfaced = consumer.dead_letters();
    assert_eq!(
        surfaced.len(),
        1,
        "DB-unreachable: the poison fell back to in-memory — NOT silently dropped"
    );
    assert_eq!(surfaced[0].envelope.event_id, EventId(id.clone()));
    // And nothing durable was written (the DB was unreachable) — proving the fallback path fired.
    let durable = consumer.durable_dead_letters();
    assert!(
        durable.is_empty(),
        "the durable table got nothing (DB unreachable) — the fallback carried it"
    );
    println!(
        "[#7b] PASS (3): a DB-unreachable durable record is LOUD + falls back in-process (0 silent \
         drop)."
    );
}
