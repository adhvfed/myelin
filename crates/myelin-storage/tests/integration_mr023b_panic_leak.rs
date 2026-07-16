//! # H2 (peer-review #7 re-prosecution) — the PANIC-LEAK is closed, proven against LIVE Postgres.
//!
//! The bug an adversarial verifier found: `DurableCoCommit` used raw `BEGIN`/`COMMIT`/`ROLLBACK` on a
//! bare `PoolConnection` with NO `Drop`, and `Consumer::deliver` called the handler with NO
//! `catch_unwind`. A tokio-task PANIC in the handler (an unwind, not process death) dropped the
//! connection MID-TRANSACTION; sqlx did not know about the hand-rolled `BEGIN`, so it returned the
//! connection to the pool STILL IN THE TRANSACTION. The NEXT delivery reused that connection, its raw
//! `BEGIN` was a no-op nested-tx, and its `COMMIT` durably committed the PANICKED delivery's dedup
//! mark + partial effect → the panicked event read `!fresh` on redelivery → Deduplicated → its valid
//! effect LOST (the MR-023b at-most-once floor resurrected).
//!
//! The fix (both halves):
//!  - **Structural:** `DurableCoCommit` now wraps a NATIVE `sqlx::Transaction`, whose `Drop` rolls the
//!    transaction back before the connection returns to the pool — a leaked open tx is IMPOSSIBLE.
//!  - **Defense in depth:** `Consumer::deliver` `catch_unwind`s the handler; on a panic it ROLLS BACK
//!    the co-commit tx and DEAD-LETTERS the message (no dedup tombstone → the valid effect stays
//!    replayable; a `Retry` was rejected to avoid a panic-loop).
//!
//! This probe forces connection REUSE (`max_connections = 1`) so delivery B literally reuses the
//! connection delivery A panicked on, and asserts A's mark+effect are NOT committed by B, and that A's
//! redelivery still RE-RUNS (0 lost).
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-storage --features integration --test integration_mr023b_panic_leak -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use myelin_config::MyelinConfig;
use myelin_storage::events_durable::DurableDedupBacking;
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::consumer::{Consumer, Delivered, Message, Subscription};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, ConsumerName, CorrelationId, DataRole, DedupLedger,
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

/// A handler that WRITES its effect on the co-commit tx and then, if `boom`, PANICS (a bug after a
/// speculative write). If not `boom`, it returns the scripted outcome (the redelivery re-run).
struct MaybePanicHandler {
    table: String,
    rt: tokio::runtime::Handle,
    boom: bool,
    outcome: HandleOutcome,
    ran: AtomicU32,
}

impl EventHandler for MaybePanicHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, ev: &EventEnvelope, tx: &mut HandlerTx<'_>) -> HandleOutcome {
        self.ran.fetch_add(1, Ordering::SeqCst);
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .expect("durable co-commit exposes a connection");
        let sql = format!(
            "INSERT INTO {} (event_id) VALUES ($1) ON CONFLICT DO NOTHING",
            self.table
        );
        tokio::task::block_in_place(|| {
            self.rt.block_on(async {
                sqlx::query(&sql)
                    .bind(&ev.event_id.0)
                    .execute(&mut *conn)
                    .await
                    .expect("write effect on the co-commit tx");
            })
        });
        if self.boom {
            // The BUG: the handler panics AFTER writing on the still-open co-commit tx. Pre-fix this
            // leaked the open tx to the pool; the fix rolls it back (native tx Drop + deliver's
            // catch_unwind → rollback + dead-letter).
            panic!("handler bug: panic after writing on the co-commit tx");
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
    sqlx::query_scalar(&format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE event_id = $1)"
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("effect read")
}

async fn dedup_present(pool: &sqlx::PgPool, consumer: &str, id: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
    )
    .bind(consumer)
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("dedup read")
}

// =================================================================================================
// H2 — a PANICKING handler on delivery A must NOT leak its OPEN co-commit tx into the pool: a
// subsequent delivery B on the REUSED connection commits ONLY B's mark+effect, never A's; and A's
// redelivery still RE-RUNS (0 lost — the panicked event's valid effect is not deduped-and-lost).
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h2_panicking_handler_does_not_leak_open_tx_to_a_reused_pool_connection() {
    let cfg = MyelinConfig::dev();
    // max_connections = 1 FORCES delivery B to reuse the exact connection A panicked on — the leak's
    // resurrection window. If the open tx leaked, B's commit would durably land A's mark+effect.
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 1)
        .await
        .expect("connect Postgres (is the dev stack up?)");
    let tag = uniq();
    let effect = format!("h2_leak_effect_{tag}");
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
    let cname = format!("h2_leak_{tag}");
    let id_a = format!("h2-evt-A-{tag}");
    let id_b = format!("h2-evt-B-{tag}");
    let msg_a = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope(&id_a),
    };
    let msg_b = Message {
        subject: "myelin://acme/issues/issue/PROJ-1".into(),
        envelope: envelope(&id_b),
    };

    // (A) A PANICKING handler: writes the effect on the co-commit tx, then panics. `deliver`
    //     catch_unwinds → rolls back the tx → DEAD-LETTERS. NEITHER A's mark NOR A's effect lands.
    let panic_consumer = Consumer::new(
        MaybePanicHandler {
            table: effect.clone(),
            rt: rt.clone(),
            boom: true,
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        sub(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| panic_consumer.deliver(&msg_a));
    assert!(
        matches!(out, Delivered::DeadLettered(_)),
        "a handler panic is caught and dead-lettered (not propagated), got {out:?}"
    );
    assert_eq!(
        panic_consumer.handler().ran.load(Ordering::SeqCst),
        1,
        "the panicking handler ran once"
    );
    assert!(
        !dedup_present(&pool, &cname, &id_a).await,
        "A's dedup mark did NOT commit (rolled back on the panic)"
    );
    assert!(
        !effect_present(&pool, &effect, &id_a).await,
        "A's effect did NOT commit (rolled back on the panic)"
    );

    // (B) A SUCCEEDING delivery for a DIFFERENT event on the REUSED connection (max_connections=1).
    //     If A's open tx had leaked, B's COMMIT would also commit A's leftover mark+effect. It must
    //     commit ONLY B's.
    let done_consumer = Consumer::new(
        MaybePanicHandler {
            table: effect.clone(),
            rt: rt.clone(),
            boom: false,
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        sub(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| done_consumer.deliver(&msg_b));
    assert_eq!(out, Delivered::Acked, "B committed its own mark+effect");
    assert!(
        dedup_present(&pool, &cname, &id_b).await && effect_present(&pool, &effect, &id_b).await,
        "B's mark+effect are durable"
    );
    // THE LEAK ASSERTION: B did NOT resurrect A's rolled-back mark/effect via a leaked open tx.
    assert!(
        !dedup_present(&pool, &cname, &id_a).await,
        "H2: B on the reused connection did NOT commit A's leaked mark (no open-tx leak)"
    );
    assert!(
        !effect_present(&pool, &effect, &id_a).await,
        "H2: B on the reused connection did NOT commit A's leaked effect (no open-tx leak)"
    );

    // (C) A's REDELIVERY (a non-panicking handler, as a fixed+redeployed consumer would be) is STILL
    //     FRESH (A was never durably marked) → it RE-RUNS and lands (0 lost — the panicked event's
    //     valid effect was not deduped-and-lost).
    let redeliver_a = Consumer::new(
        MaybePanicHandler {
            table: effect.clone(),
            rt: rt.clone(),
            boom: false,
            outcome: HandleOutcome::Done,
            ran: AtomicU32::new(0),
        },
        sub(&cname),
        make_ledger(),
    );
    let out = tokio::task::block_in_place(|| redeliver_a.deliver(&msg_a));
    assert_eq!(
        out,
        Delivered::Acked,
        "A's redelivery is fresh (A was never marked) → it re-runs and commits"
    );
    assert!(
        dedup_present(&pool, &cname, &id_a).await && effect_present(&pool, &effect, &id_a).await,
        "A's effect finally lands on the redelivery (0 lost)"
    );

    // Cleanup.
    sqlx::query(&format!("DROP TABLE IF EXISTS {effect}"))
        .execute(&pool)
        .await
        .ok();
    println!(
        "[H2] PASS panic-leak closed: a panicking handler rolls back its co-commit tx and \
         dead-letters; a subsequent delivery on the REUSED pool connection commits only its own \
         mark+effect (no leaked open tx); the panicked event re-runs on redelivery (0 lost)."
    );
}
