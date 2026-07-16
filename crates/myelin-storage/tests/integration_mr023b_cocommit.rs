//! # MR-023b / peer-review #7 — the same-transaction co-commit, proven against LIVE Postgres.
//!
//! This is the kill-9 co-commit GATE for the idempotent consumer: it proves the dedup mark and the
//! handler's DB effect land ATOMICALLY in ONE transaction (either both or neither), against the live
//! docker-compose Postgres (:5433), NOT modeled in memory.
//!
//! The bug it closes (the OLD at-most-once floor): the consumer marked `(consumer, event_id)`
//! handled — a durable INSERT that COMMITTED IMMEDIATELY — BEFORE calling the handler, whose effect
//! committed LATER in a SEPARATE transaction. A kill-9 between the two → on restart the broker
//! redelivers → `mark_handled` reads NOT-fresh → `Deduplicated` → ack → THE EFFECT IS LOST FOREVER.
//!
//! The fix (Option A — same-tx co-commit): [`DedupLedger::begin_co_commit`] opens ONE tx, INSERTs
//! the dedup mark within it, and hands the transaction-bound connection to the handler; the runtime
//! commits on `Done` / rolls back on `Retry`/failure. A crash before commit leaves NEITHER → a
//! redelivery re-runs and the effect lands (exactly-once-WITH-EFFECT).
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test integration_mr023b_cocommit -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::events_durable::DurableDedupBacking;
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::consumer::{Consumer, Delivered, Message, Subscription};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, Backoff, ConsumerName, CorrelationId, DataRole, DedupLedger,
    DurableDedup, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome, HandlerTx,
    PrefetchBound, SubjectPattern, Timestamp, Visibility, CONSUMER_DEDUP_MIGRATION,
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

fn envelope(id: &str) -> EventEnvelope {
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
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-16T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-16T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": "x" }),
    }
}

static SUBJECTS: &[SubjectPattern] = &[];

/// A handler whose EFFECT is a DB write on the co-commit transaction — the exact shape #7/MR-023b is
/// about. It downcasts `tx.connection::<sqlx::PgConnection>()` and INSERTs the event_id into an
/// effect table ON THE SAME TX the dedup mark is in, then returns a scripted terminal outcome. A
/// handler that finds NO tx fails-closed (Retry), never a silent write outside the tx.
struct EffectHandler {
    table: String,
    rt: tokio::runtime::Handle,
    /// The terminal outcome to return AFTER writing (Done commits; Retry rolls back — the kill-9 model).
    outcome: HandleOutcome,
    /// How many times the handler actually ran its body (so a dedup-skip is observable).
    ran: AtomicU32,
}

impl EventHandler for EffectHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, ev: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome {
        self.ran.fetch_add(1, Ordering::SeqCst);
        let Some(conn) = tx.connection::<sqlx::PgConnection>() else {
            // Fail-closed: a durable handler with no co-commit tx must NOT write outside it.
            return HandleOutcome::Retry(Backoff { seconds: 1 });
        };
        let sql = format!(
            "INSERT INTO {} (event_id) VALUES ($1) ON CONFLICT DO NOTHING",
            self.table
        );
        let wrote = tokio::task::block_in_place(|| {
            self.rt.block_on(async {
                sqlx::query(&sql)
                    .bind(&ev.event_id.0)
                    .execute(&mut *conn)
                    .await
            })
        });
        if wrote.is_err() {
            return HandleOutcome::Retry(Backoff { seconds: 1 });
        }
        self.outcome.clone()
    }
}

async fn setup(pool: &sqlx::PgPool, effect_table: &str) {
    sqlx::raw_sql(CONSUMER_DEDUP_MIGRATION)
        .execute(pool)
        .await
        .expect("apply CONSUMER_DEDUP_MIGRATION");
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {effect_table} (event_id text PRIMARY KEY)"
    ))
    .execute(pool)
    .await
    .expect("create effect table");
}

async fn effect_present(pool: &sqlx::PgPool, table: &str, id: &str) -> bool {
    let exists: bool =
        sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE event_id = $1)"))
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("effect read");
    exists
}

async fn dedup_present(pool: &sqlx::PgPool, consumer: &str, id: &str) -> bool {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
    )
    .bind(consumer)
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("dedup read");
    exists
}

// ==============================================================================================
// TEST 1 — the co-commit PRIMITIVE: a crash-before-commit leaves NEITHER the mark NOR the effect;
//          a commit lands BOTH; a redelivery is deduped with 0 duplicate effect. (kill-9 model.)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023b_cocommit_primitive_crash_leaves_neither_commit_lands_both() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 6)
        .await
        .expect("connect Postgres (is the dev stack up?)");
    let tag = uniq();
    let effect = format!("mr023b_effect_{tag}");
    setup(&pool, &effect).await;

    let backing = DurableDedupBacking::new(pool.clone(), tokio::runtime::Handle::current());
    let ledger = DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>);
    let consumer = ConsumerName(format!("mr023b_consumer_{tag}"));
    let id = format!("mr023b-evt-{tag}");
    let tenant = TenantId("acme".into());
    let region = Region(cfg.region.clone());

    // (A) CRASH BEFORE COMMIT: open the co-commit tx (marks dedup within it), write the effect on
    //     the SAME tx, then ROLL BACK (models the process dying before commit).
    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger.begin_co_commit(&consumer, &EventId(id.clone()), &tenant, &region);
        assert!(fresh, "first delivery marks the dedup row FRESH");
        let conn = cotx
            .connection()
            .expect("durable co-commit exposes a connection")
            .downcast_mut::<sqlx::PgConnection>()
            .expect("the erased connection is a PgConnection");
        tokio::runtime::Handle::current().block_on(async {
            sqlx::query(&format!("INSERT INTO {effect} (event_id) VALUES ($1)"))
                .bind(&id)
                .execute(&mut *conn)
                .await
                .expect("write effect on the co-commit tx");
        });
        // the process "dies" here — roll back instead of commit.
        cotx.rollback();
    });

    // The OLD bug would have a COMMITTED mark with a LOST effect. The FIX: NEITHER is present.
    assert!(
        !dedup_present(&pool, &consumer.0, &id).await,
        "crash-before-commit: the dedup mark did NOT persist (rolled back with the effect)"
    );
    assert!(
        !effect_present(&pool, &effect, &id).await,
        "crash-before-commit: the effect did NOT persist (rolled back with the mark)"
    );

    // (B) REDELIVERY re-runs (the mark is gone → still fresh) and this time COMMITS: both land.
    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger.begin_co_commit(&consumer, &EventId(id.clone()), &tenant, &region);
        assert!(fresh, "after the rollback the redelivery is STILL FRESH (0 lost)");
        let conn = cotx
            .connection()
            .unwrap()
            .downcast_mut::<sqlx::PgConnection>()
            .unwrap();
        tokio::runtime::Handle::current().block_on(async {
            sqlx::query(&format!("INSERT INTO {effect} (event_id) VALUES ($1)"))
                .bind(&id)
                .execute(&mut *conn)
                .await
                .expect("write effect on the redelivery tx");
        });
        cotx.commit().expect("commit the co-commit tx");
    });

    assert!(
        dedup_present(&pool, &consumer.0, &id).await,
        "after commit: the dedup mark IS durable"
    );
    assert!(
        effect_present(&pool, &effect, &id).await,
        "after commit: the effect IS durable (exactly-once-with-effect)"
    );

    // (C) A further REDELIVERY is now deduped (mark present) → 0 duplicate effect.
    tokio::task::block_in_place(|| {
        let (cotx, fresh) = ledger.begin_co_commit(&consumer, &EventId(id.clone()), &tenant, &region);
        assert!(!fresh, "the committed mark makes a redelivery a DUPLICATE (deduped)");
        cotx.rollback();
    });
    let effect_count: i64 =
        sqlx::query_scalar(&format!("SELECT count(*) FROM {effect} WHERE event_id = $1"))
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(effect_count, 1, "exactly ONE effect row — no duplicate on redelivery");
}

// ==============================================================================================
// TEST 2 — through the Consumer RUNTIME with a DB-writing handler: Done co-commits mark+effect; a
//          redelivery is Deduplicated (0 dup); a Retry rolls BOTH back → a redelivery re-runs and
//          the effect lands (the kill-9 invariant end-to-end through `Consumer::deliver`).
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023b_consumer_runtime_cocommit_retry_rolls_back_then_reruns() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 6)
        .await
        .expect("connect Postgres");
    let tag = uniq();
    let effect = format!("mr023b_rt_effect_{tag}");
    setup(&pool, &effect).await;

    let rt = tokio::runtime::Handle::current();
    let make_ledger = || {
        let backing = DurableDedupBacking::new(pool.clone(), rt.clone());
        DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>)
    };
    let sub = |name: &str| {
        Subscription::bind(
            ConsumerName(name.into()),
            &["myelin://acme/issues/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap()
    };
    let cname = format!("mr023b_rt_{tag}");
    let id = format!("mr023b-rt-evt-{tag}");
    let msg = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope(&id),
    };

    // (A) A RETRY handler: writes the effect on the tx, then returns Retry → the runtime ROLLS BACK
    //     (models a crash / transient failure before commit). NEITHER the mark NOR the effect lands.
    let retry_consumer = Consumer::new(
        EffectHandler {
            table: effect.clone(),
            rt: rt.clone(),
            outcome: HandleOutcome::Retry(Backoff { seconds: 1 }),
            ran: AtomicU32::new(0),
        },
        sub(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| retry_consumer.deliver(&msg));
    assert_eq!(out, Delivered::Retried(1), "a Retry is not acked");
    assert_eq!(retry_consumer.handler().ran.load(Ordering::SeqCst), 1, "the handler ran");
    assert!(
        !dedup_present(&pool, &cname, &id).await,
        "Retry rolled back the co-commit tx → the dedup mark is GONE"
    );
    assert!(
        !effect_present(&pool, &effect, &id).await,
        "Retry rolled back the co-commit tx → the effect is GONE (both vanish together)"
    );

    // (B) REDELIVERY to a Done handler (same durable ledger over the same pool — a fresh Consumer,
    //     as a reconnect / restart would be): the handler RE-RUNS (0 lost), writes the effect, and
    //     the runtime COMMITS → mark + effect both durable.
    let done_consumer = Consumer::new(
        EffectHandler {
            table: effect.clone(),
            rt: rt.clone(),
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        sub(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| done_consumer.deliver(&msg));
    assert_eq!(out, Delivered::Acked, "the redelivery ran + committed");
    assert!(
        dedup_present(&pool, &cname, &id).await,
        "Done committed the dedup mark"
    );
    assert!(
        effect_present(&pool, &effect, &id).await,
        "Done committed the effect (exactly-once-WITH-EFFECT — not deduped-and-lost)"
    );

    // (C) A further REDELIVERY is Deduplicated — the handler does NOT re-run, 0 duplicate effect.
    let out = tokio::task::block_in_place(|| done_consumer.deliver(&msg));
    assert_eq!(out, Delivered::Deduplicated, "the committed mark dedups the redelivery");
    assert_eq!(
        done_consumer.handler().ran.load(Ordering::SeqCst),
        1,
        "the Done handler ran exactly once across the redelivery (dedup-skip)"
    );
    let effect_count: i64 =
        sqlx::query_scalar(&format!("SELECT count(*) FROM {effect} WHERE event_id = $1"))
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(effect_count, 1, "exactly ONE effect row — no duplicate");
}
