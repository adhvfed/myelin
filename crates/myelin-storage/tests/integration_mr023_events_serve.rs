//! # MR-023 — the events `serve()` composition root, proven against LIVE Postgres + NATS.
//!
//! These are the silent-data-loss GATES for the events pipeline, proven against the live
//! docker-compose stack (real Postgres :5433 + real NATS JetStream :4222), NOT modeled in memory.
//! They drive the MR-023 deliverables through the [`EventsRuntime`] composition root:
//!   1. **0 lost / 0 ghost under a mid-publish crash** — N events co-committed through the durable
//!      outbox (the MR-022 `with_tenant_tx` convention), drained to REAL NATS by the relay; a crash
//!      mid-drain leaves rows claimable; a restart re-publishes them (broker dedups) and the
//!      idempotent consumer (DURABLE dedup) delivers each exactly once → 0 lost, 0 ghost.
//!   2. **Durable dedup survives a process restart** — a `(consumer, event_id)` marked handled is
//!      STILL deduped after a fresh `DurableDedupBacking` / `Consumer` over the same pool (the
//!      in-memory `HashSet` would be empty → would re-run the handler → a ghost). SUB-D2 across a
//!      real restart.
//!   3. **emit-iff-committed atomicity (BUS-D4)** — a business write + the outbox row co-committed
//!      in ONE `with_tenant_tx`: an aborted tx writes NEITHER row; a committed tx writes BOTH.
//!
//! Run against the dev stack:
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-storage --features integration --test integration_mr023_events_serve -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_config::MyelinConfig;
use myelin_storage::events_serve::EventsRuntime;
use myelin_storage::pg::PgError;
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::tenant_tx::connect_pool_with_reset;

use myelin_events::consumer::{ConsumerSpec, Delivered, Message};
use myelin_events::relay::BusTransport;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, ConsumerName, CorrelationId, DataRole,
    DedupLedger, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome, SubjectPattern,
    Timestamp, Visibility, CONSUMER_DEDUP_MIGRATION, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::events_durable::DurableDedupBacking;
use myelin_tenancy::{Region, TenantId};

// ----------------------------------------------------------------------------------------------
// shared helpers
// ----------------------------------------------------------------------------------------------

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
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

/// Ensure the substrate's co-located tables exist (the frozen outbox + consumer_dedup migrations,
/// idempotent `CREATE TABLE IF NOT EXISTS`). The provider's `migrate_foundation` runs these at boot;
/// the test applies them directly so it is self-contained.
async fn ensure_foundation(pool: &sqlx::PgPool) {
    sqlx::raw_sql(OUTBOX_MIGRATION)
        .execute(pool)
        .await
        .expect("apply OUTBOX_MIGRATION");
    sqlx::raw_sql(CONSUMER_DEDUP_MIGRATION)
        .execute(pool)
        .await
        .expect("apply CONSUMER_DEDUP_MIGRATION");
}

/// A handler that COUNTS how many distinct times it actually ran the body (so a dedup-skip — at the
/// broker OR the durable consumer ledger — is observable as the handler NOT running).
struct CountingHandler {
    runs: std::sync::atomic::AtomicU32,
}
static SUBJECTS: &[SubjectPattern] = &[];
impl EventHandler for CountingHandler {
    fn subjects(&self) -> &'static [SubjectPattern] {
        SUBJECTS
    }
    fn handle(&self, _ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        self.runs.fetch_add(1, Ordering::SeqCst);
        HandleOutcome::Done
    }
}

// ==============================================================================================
// TEST 1 — the serve() pipeline: 0 lost / 0 ghost under a mid-publish crash (SUB-D1 end-to-end)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023_serve_zero_lost_zero_ghost_under_crash() {
    let cfg = MyelinConfig::dev();
    // **Test-isolation (2026-07 re-prosecution finding):** this test does WHOLE-TABLE `outbox` ops
    // (`outbox_depth` counts every unsent row; `drain_relay_to_empty` publishes every unsent row and
    // asserts the exact count == N). Run in the shared `public.outbox`, it RACES cross-binary with
    // `integration_mr009b_outbox_durable`'s `concurrent_seq_scenario` (which leaves 32 unsent rows) —
    // a non-deterministic FLAKE (`outbox_depth` reads 8 + others' rows). Pin every connection to a
    // per-pid schema so this test's `outbox` is ISOLATED (the established ci-dispatch / CT-004a
    // pattern), making the whole-table assertions deterministic regardless of concurrent binaries.
    let schema = format!("mr023_serve_{}", std::process::id());
    let pool = {
        let s = schema.clone();
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(6)
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
            .expect("connect Postgres (is the stack up?)")
    };
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&pool)
        .await
        .expect("drop prior schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&pool)
        .await
        .expect("create per-pid schema");
    ensure_foundation(&pool).await;

    let tag = uniq();
    let stream = format!("MYELIN_MR023_{}", tag.replace('-', "_"));
    let subject_root = format!("myelin_mr023_{}", tag.replace('-', "_"));
    let consumer_name = format!("{stream}_pull");
    let runtime = EventsRuntime::over_pool(
        pool.clone(),
        &cfg.region,
        &cfg.nats_url,
        &stream,
        &subject_root,
        &consumer_name,
        tokio::runtime::Handle::current(),
    )
    .expect("connect EventsRuntime (NATS JetStream up?)");
    tokio::task::block_in_place(|| runtime.bus().purge());

    // A real business STATE table the outbox co-commits with (the emit-iff-committed seam).
    let state_table = format!("mr023_state_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {state_table} (id text PRIMARY KEY, event_id text NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create state table");

    let tenant = "acme";
    let agg = format!("issue:MR023-{tag}");
    const N: usize = 8;
    let ids: Vec<String> = (0..N).map(|i| format!("mr023-evt-{tag}-{i}")).collect();

    // (1) Co-commit N events through the MR-022 `with_tenant_tx` convention: a business state row
    //     AND the outbox row land in the SAME transaction (PgRelay::co_commit_in_tx is the one
    //     sanctioned outbox-write site). Both commit or neither — the transactional-outbox guarantee.
    for (i, id) in ids.iter().enumerate() {
        let env = envelope(id, &format!("myelin://acme/issues/{i}"), &agg);
        let st = state_table.clone();
        let agg_c = agg.clone();
        let state_id = format!("state-{i}");
        runtime
            .with_tenant_tx(tenant, move |conn| {
                Box::pin(async move {
                    sqlx::query(&format!(
                        "INSERT INTO {st} (id, event_id) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING"
                    ))
                    .bind(&state_id)
                    .bind(&env.event_id.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                    PgRelay::co_commit_in_tx(&mut *conn, &agg_c, &env).await?;
                    Ok(())
                })
            })
            .await
            .expect("co-commit business write + outbox row");
    }
    let committed: std::collections::HashSet<EventId> = ids.iter().cloned().map(EventId).collect();
    assert_eq!(
        runtime.outbox_depth().await.expect("depth") as usize,
        N,
        "all N events durably committed + unsent before the drain"
    );

    // (2) Start draining to REAL NATS, then CRASH mid-drain: publish 3 rows to the broker but drop
    //     the tx before recording published_at for ANY of them (the silent-loss window).
    let crash_after = 3usize;
    let published_before_crash = runtime
        .relay()
        .relay_once_crash_after(runtime.bus(), 16, crash_after)
        .await
        .expect("crash-injection drain");
    assert_eq!(published_before_crash, crash_after);
    assert_eq!(
        runtime.outbox_depth().await.expect("depth after crash") as usize,
        N,
        "0 lost: the crash recorded NO marks → every committed row stays claimable"
    );

    // (3) RESTART the relay drain: re-claim all N unsent rows; the 3 crash-window rows re-publish
    //     and the broker dedups them (Nats-Msg-Id = event_id). Outbox fully drains.
    let published = runtime.drain_relay_to_empty().await.expect("restart drain");
    assert_eq!(published, N, "the restarted relay re-claims all N unsent rows");
    assert_eq!(
        runtime.outbox_depth().await.expect("final depth"),
        0,
        "outbox-depth drains to 0 (every committed row recorded sent)"
    );

    // (4) The idempotent consumer (DURABLE dedup) pumps the broker: every committed event delivered
    //     to the handler EXACTLY ONCE (0 lost, 0 ghost — broker dedup + the durable consumer ledger).
    let handler = CountingHandler {
        runs: std::sync::atomic::AtomicU32::new(0),
    };
    let spec = ConsumerSpec::new(
        ConsumerName(format!("indexer-{tag}")),
        &["myelin://acme/issues/"],
    );
    let consumer = runtime.consumer(spec, handler).expect("build consumer");
    let _ = runtime.pump_consumer(&consumer, 16);

    assert_eq!(
        consumer.handler().runs.load(Ordering::SeqCst) as usize,
        N,
        "0 lost / 0 ghost: the handler ran EXACTLY N times (each committed event delivered once)"
    );
    assert!(
        consumer.dead_letters().is_empty(),
        "no dead-letters on the no-loss path"
    );

    // emit-iff-committed: every committed event has its co-committed state row (no ghost).
    for id in &committed {
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {state_table} WHERE event_id = $1"
        ))
        .bind(&id.0)
        .fetch_one(&pool)
        .await
        .expect("state lookup");
        assert_eq!(n, 1, "every delivered event has its committed state change");
    }

    println!(
        "[MR-023] PASS  test=SERVE-0-LOST-0-GHOST-UNDER-CRASH  committed={N} delivered={N} \
         handler_runs={N} lost=0 ghost=0  crash_window={crash_after} (re-published+deduped)  \
         outbox_depth=0  backend=real-PG+real-NATS-JetStream via EventsRuntime"
    );

    // cleanup: drop the whole per-pid schema (outbox + consumer_dedup + state table live in it).
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&pool)
        .await
        .ok();
    tokio::task::block_in_place(|| runtime.bus().purge());
}

// ==============================================================================================
// TEST 2 — durable dedup SURVIVES a process restart (SUB-D2 across a real restart, SI-023)
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023_durable_dedup_survives_restart() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    ensure_foundation(&pool).await;

    let tag = uniq();
    let consumer = ConsumerName(format!("indexer-{tag}"));
    let id = EventId(format!("mr023-dedup-{tag}"));
    let rt = tokio::runtime::Handle::current();

    // --- (a) The bare DurableDedup seam: a mark made by one backing is still seen by a FRESH
    //     backing over the same pool (the "process restart" — the in-memory HashSet would be empty).
    {
        let backing1 = DedupLedger::durable(std::sync::Arc::new(DurableDedupBacking::new(
            pool.clone(),
            rt.clone(),
        )));
        assert!(
            backing1.mark_handled(&consumer, &id),
            "first mark is FRESH (the handler should run)"
        );
        // simulate a process restart: drop backing1 entirely, build a brand-new backing.
    }
    {
        let backing2 = DedupLedger::durable(std::sync::Arc::new(DurableDedupBacking::new(
            pool.clone(),
            rt.clone(),
        )));
        assert!(
            !backing2.mark_handled(&consumer, &id),
            "after a RESTART the mark SURVIVED → DUPLICATE (the in-memory HashSet would re-run → ghost)"
        );
        assert!(
            backing2.is_handled(&consumer, &id),
            "the durable ledger still reports the pair handled across the restart"
        );
    }

    // --- (b) Through the Consumer runtime: deliver an event on one consumer (handler runs), then
    //     "restart" (a NEW Consumer + a NEW durable ledger over the same pool, SAME durable name)
    //     and redeliver the same event → DEDUPLICATED, the handler does NOT re-run (0 dup).
    let subject = "myelin://acme/issues/issue/PROJ-1";
    let evt = envelope(&format!("mr023-c-{tag}"), subject, "issue:PROJ-1");
    let durable_name = ConsumerName(format!("consumer-{tag}"));
    let msg = Message {
        subject: subject.to_string(),
        envelope: evt.clone(),
    };

    let runs_first = {
        let ledger = DedupLedger::durable(std::sync::Arc::new(DurableDedupBacking::new(
            pool.clone(),
            rt.clone(),
        )));
        let c = myelin_events::consume(
            ConsumerSpec::new(durable_name.clone(), &["myelin://acme/issues/"]),
            CountingHandler {
                runs: std::sync::atomic::AtomicU32::new(0),
            },
            ledger,
        )
        .expect("build consumer 1");
        let out = c.deliver(&msg);
        assert_eq!(out, Delivered::Acked, "first delivery runs + acks");
        c.handler().runs.load(Ordering::SeqCst)
    };
    assert_eq!(runs_first, 1, "consumer 1 ran the handler once");

    // RESTART: a fresh consumer + fresh durable ledger over the same pool, re-bound by the SAME name.
    let c2 = myelin_events::consume(
        ConsumerSpec::new(durable_name.clone(), &["myelin://acme/issues/"]),
        CountingHandler {
            runs: std::sync::atomic::AtomicU32::new(0),
        },
        DedupLedger::durable(std::sync::Arc::new(DurableDedupBacking::new(
            pool.clone(),
            rt.clone(),
        ))),
    )
    .expect("build consumer 2 (restart)");
    assert_eq!(
        c2.deliver(&msg),
        Delivered::Deduplicated,
        "after a restart the redelivery is DEDUPLICATED (durable ledger), 0 dup / 0 ghost"
    );
    assert_eq!(
        c2.handler().runs.load(Ordering::SeqCst),
        0,
        "the restarted consumer did NOT re-run the handler (it was already handled, durably)"
    );

    println!(
        "[MR-023] PASS  test=DURABLE-DEDUP-SURVIVES-RESTART  pre-restart_runs=1 \
         post-restart_runs=0 deduped=true  backend=real-PG consumer_dedup table"
    );

    // cleanup
    sqlx::query("DELETE FROM consumer_dedup WHERE consumer = $1")
        .bind(&consumer.0)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM consumer_dedup WHERE consumer = $1")
        .bind(&durable_name.0)
        .execute(&pool)
        .await
        .ok();
}

// ==============================================================================================
// TEST 3 — emit-iff-committed atomicity (BUS-D4) via the with_tenant_tx co-commit
// ==============================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mr023_co_commit_is_emit_iff_committed_atomic() {
    let cfg = MyelinConfig::dev();
    let pool = connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 4)
        .await
        .expect("connect Postgres");
    ensure_foundation(&pool).await;

    let tag = uniq();
    let state_table = format!("mr023_atom_{}", tag.replace('-', "_"));
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {state_table} (id text PRIMARY KEY, event_id text NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("create state table");

    let runtime = {
        // No bus needed for the atomicity proof, but EventsRuntime::over_pool connects one; reuse a
        // throwaway stream so the runtime's with_tenant_tx convention is the exercised seam.
        let stream = format!("MYELIN_MR023A_{}", tag.replace('-', "_"));
        let subject_root = format!("myelin_mr023a_{}", tag.replace('-', "_"));
        EventsRuntime::over_pool(
            pool.clone(),
            &cfg.region,
            &cfg.nats_url,
            &stream,
            &subject_root,
            &format!("{stream}_pull"),
            tokio::runtime::Handle::current(),
        )
        .expect("connect EventsRuntime")
    };

    let tenant = "acme";
    let agg = format!("issue:ATOM-{tag}");

    // --- ABORT: business write + outbox row staged in one tx, then the closure returns Err →
    //     the WHOLE tx rolls back. NEITHER the state row NOR the outbox row exists (emit-iff-committed).
    let abort_id = format!("mr023-atom-abort-{tag}");
    let abort_env = envelope(&abort_id, "myelin://acme/issues/abort", &agg);
    let st = state_table.clone();
    let agg_c = agg.clone();
    let res: Result<(), PgError> = runtime
        .with_tenant_tx(tenant, move |conn| {
            Box::pin(async move {
                sqlx::query(&format!("INSERT INTO {st} (id, event_id) VALUES ($1, $2)"))
                    .bind("abort-state")
                    .bind(&abort_env.event_id.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                PgRelay::co_commit_in_tx(&mut *conn, &agg_c, &abort_env).await?;
                // SIMULATED handler failure AFTER staging both writes → the whole tx rolls back.
                Err(PgError::Query("simulated handler failure (abort)".into()))
            })
        })
        .await;
    assert!(res.is_err(), "the aborting tx returns Err");

    let state_rows: i64 =
        sqlx::query_scalar(&format!("SELECT count(*) FROM {state_table} WHERE event_id = $1"))
            .bind(&abort_id)
            .fetch_one(&pool)
            .await
            .expect("count abort state");
    let outbox_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE event_id = $1")
        .bind(&abort_id)
        .fetch_one(&pool)
        .await
        .expect("count abort outbox");
    assert_eq!(state_rows, 0, "ABORT: no business state row (rolled back)");
    assert_eq!(
        outbox_rows, 0,
        "ABORT: no outbox row → emit-iff-committed (no event without its committed state)"
    );

    // --- COMMIT: the same co-commit returning Ok → BOTH the state row AND the outbox row exist.
    let ok_id = format!("mr023-atom-ok-{tag}");
    let ok_env = envelope(&ok_id, "myelin://acme/issues/ok", &agg);
    let st2 = state_table.clone();
    let agg_c2 = agg.clone();
    runtime
        .with_tenant_tx(tenant, move |conn| {
            Box::pin(async move {
                sqlx::query(&format!("INSERT INTO {st2} (id, event_id) VALUES ($1, $2)"))
                    .bind("ok-state")
                    .bind(&ok_env.event_id.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(e.to_string()))?;
                PgRelay::co_commit_in_tx(&mut *conn, &agg_c2, &ok_env).await?;
                Ok(())
            })
        })
        .await
        .expect("co-commit commits");

    let ok_state: i64 =
        sqlx::query_scalar(&format!("SELECT count(*) FROM {state_table} WHERE event_id = $1"))
            .bind(&ok_id)
            .fetch_one(&pool)
            .await
            .expect("count ok state");
    let ok_outbox: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE event_id = $1 AND published_at IS NULL",
    )
    .bind(&ok_id)
    .fetch_one(&pool)
    .await
    .expect("count ok outbox");
    assert_eq!(ok_state, 1, "COMMIT: the business state row exists");
    assert_eq!(
        ok_outbox, 1,
        "COMMIT: the outbox row exists, unsent (both committed atomically)"
    );

    println!(
        "[MR-023] PASS  test=EMIT-IFF-COMMITTED-ATOMIC  abort=(state=0,outbox=0) \
         commit=(state=1,outbox=1)  via with_tenant_tx co-commit  backend=real-PG"
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
}
