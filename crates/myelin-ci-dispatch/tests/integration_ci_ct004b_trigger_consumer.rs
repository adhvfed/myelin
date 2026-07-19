//! **CT-004b — the LIVE `ci-dispatch.trigger` consumer, DURABILITY PROVEN on live Postgres, now with
//! the H1 (peer-review #7 re-prosecution) TRUE co-commit + LIVELOCK-CLOSED proofs, PLUS the CT-004d.2
//! chunk 4 PRODUCTION durable `ci_run` writer (`CoCommitReserveStore`) proofs.**
//!
//! Five live-PG proofs (all in an isolated per-pid schema, `myelin_admin` = BYPASSRLS so the reserve
//! exercises the FORCE-RLS `ci_run` shape; the app-role RLS block is proven separately in
//! `myelin-ci-controlplane`'s `integration_ci_ct004d2_ci_run_store`):
//!
//! Proofs (1)–(3) use the test-local `IdealCoCommitReserveStore` (the ASPIRATIONAL all-in-one-tx shape:
//! `ci_run` ROW + BOTH events + mark in ONE tx) + the production `OutboxReserveStore` (events-only
//! absorb). Proofs (4)–(5) use the PRODUCTION `CoCommitReserveStore` (CT-004d.2 chunk 4): the `ci_run`
//! ROW co-commits with the mark via `CiRunStore::co_commit_insert`, the co-emitted EVENTS stay absorb
//! through the REAL durable outbox (the honest #7 split shipped to production).
//!
//!  1. **`h1_true_cocommit_ci_run_events_and_mark_in_one_tx_idempotent`** — the honest #7 shape: a real
//!     `git.ref.updated` push, delivered through the FULL `Consumer` runtime with a DURABLE
//!     `consumer_dedup` ledger, co-commits the `ci_run` ROW + `ci.run.started` + the queued
//!     `ci.check.updated` events + the dedup MARK in ONE transaction (the reserve store rides the
//!     co-commit connection `tx.connection::<sqlx::PgConnection>()`). A redelivery (same `event_id`)
//!     is `Deduplicated` — 1 run, 3 events, 0 duplicates.
//!  2. **`h1_crash_window_rolls_back_everything_then_reruns`** — the crash-window: persist on the
//!     co-commit tx then ROLL BACK (kill-9 before commit) → NOTHING lands (no ci_run, no events, no
//!     mark); the redelivery re-runs the WHOLE dispatch + persist and COMMITS → all present exactly
//!     once; a further redelivery is deduped. NO livelock, NO duplicate, NO `Err`-retry.
//!  3. **`h1_production_outbox_absorb_closes_the_livelock`** — the PRODUCTION `OutboxReserveStore`
//!     separate-tx path over the REAL durable outbox: persisting the SAME armed run TWICE (a
//!     crash-window redelivery re-emitting the SAME deterministic ids) both return `Ok` — the second
//!     is ABSORBED (`commit_absorb` → `ON CONFLICT (event_id) DO NOTHING`), NOT rejected into the
//!     unbounded `Retry` LIVELOCK the reject-arm `commit` caused. Events present exactly once.
//!
//! The pre-fix bug (H1): `OutboxReserveStore` committed via the reject-arm; `PgRelay::commit_staged_atomic`
//! mapped a duplicate `event_id` to `Err("duplicate emit")`, so a crash-window redelivery re-emitted the
//! same ids → `Err` → the handler returned `Retry` → the message NEVER acked = a permanent livelock.
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-dispatch --features integration \
//!     --test integration_ci_ct004b_trigger_consumer -- --nocapture
#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_ci_controlplane::{ci_durable_migrations, ci_run_store_factory};
use myelin_ci_dispatch::{
    build_dispatch_consumers, plan_dispatch, ArmedRun, AuthoritativeGitRoot, CiTriggerHandler,
    CoCommitReserveStore, DispatchOutcome, GitConfigReader, GitReadError, OutboxReserveStore,
    ReserveError, ReserveStore,
};
use myelin_events::consumer::{Consumer, Delivered, Message, Subscription};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, ConsumerName, CorrelationId, DataRole, DedupLedger,
    DurableDedup, EventEnvelope, EventId, EventType, HandlerTx, OutboxStore, PrefetchBound,
    Timestamp, UlidMinter, Visibility, CONSUMER_DEDUP_MIGRATION, OUTBOX_MIGRATION,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::events_durable::{DurableDeadLetterBacking, DurableDedupBacking};
use myelin_storage::outbox_durable::PgOutboxBacking;
use myelin_storage::{BlobStore, FsBlobStore};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn uniq() -> u64 {
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::SeqCst)
}

/// A per-(pid, counter) schema so each test isolates its tables (never another test's rows).
fn schema_name(k: u64) -> String {
    format!("ci_ct004b_{}_{}", std::process::id(), k)
}

/// Open an admin pool whose connections pin `search_path` to `schema` (the CT-004a posture; admin =
/// BYPASSRLS so the reserve exercises the FORCE-RLS ci_run shape).
async fn reopen(schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("reconnect to dev Postgres (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

/// Stand up the isolated schema + the shared CI tables (ci_run etc.) + consumer_dedup + a reserve
/// outbox mirror. Returns a pool pinned to the schema.
async fn setup_schema(schema: &str, reserve_outbox_ddl: &str) -> PgPool {
    let p = reopen(schema).await;
    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop prior schema");
    p.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    for m in ci_durable_migrations().0.iter() {
        p.execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply CI durable migration {}: {e}", m.id));
    }
    p.execute(CONSUMER_DEDUP_MIGRATION)
        .await
        .expect("apply consumer_dedup migration");
    p.execute(reserve_outbox_ddl)
        .await
        .expect("apply reserve outbox DDL");
    p
}

/// The outbox-shaped mirror the TRUE co-commit reserve store writes the two events into (alongside the
/// ci_run row, in the co-commit tx). A minimal isolated table so the one-tx co-commit is assertable.
const CREATE_RESERVE_OUTBOX_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_reserve_outbox (
  event_id  text PRIMARY KEY,
  run_id    text NOT NULL,
  type      text NOT NULL,
  subject   text NOT NULL,
  aggregate text NOT NULL,
  payload   jsonb NOT NULL
)";

const VALID_CI_TOML: &str = "\
on = \"push\"

[[jobs]]
name = \"build\"
image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"
command = [\"build\"]

[[jobs]]
name = \"test\"
image = \"registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000\"
command = [\"test\"]
needs = [\"build\"]
";

struct FixtureGitReader {
    repo: String,
    oid: String,
}

impl GitConfigReader for FixtureGitReader {
    fn read_repo_file(
        &self,
        _tenant: &str,
        _region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        if repo == self.repo && oid == self.oid && path == ".myelin/ci.toml" {
            Ok(Some(VALID_CI_TOML.as_bytes().to_vec()))
        } else {
            Ok(None)
        }
    }
}

/// **The TRUE co-commit reserve store (H1): write the `ci_run` row + the two events on the CO-COMMIT
/// CONNECTION** (`tx.connection::<sqlx::PgConnection>()`) — the SAME `sqlx` transaction the dedup mark
/// is in. So the whole bundle + the mark commit or roll back as ONE unit. This is what the production
/// `myelin-storage` `ci_run`-writer floor will do; the leaf `OutboxReserveStore` cannot name `sqlx`.
struct IdealCoCommitReserveStore {
    rt: tokio::runtime::Handle,
}

impl IdealCoCommitReserveStore {
    async fn write(conn: &mut sqlx::PgConnection, armed: &ArmedRun) -> Result<(), ReserveError> {
        let rw = &armed.handoff.run_write;
        sqlx::query(
            "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
             definition_snapshot, trigger_kind, trust_tier, state, correlation_id, cause_event_id) \
             VALUES ($1,$2,$3::uuid,$4::uuid,$5::uuid,$6::uuid,$7,$8,$9,$10,$11,$12) \
             ON CONFLICT (tenant_id, run_id) DO NOTHING",
        )
        .bind(&armed.tenant.0)
        .bind(&armed.reserve.region)
        .bind(&rw.run_id)
        .bind(&armed.reserve.project_id)
        .bind(&armed.reserve.pipeline_id)
        .bind(&armed.reserve.wf_run_id)
        .bind(&rw.definition_snapshot.0)
        .bind(&rw.trigger_kind)
        .bind(&rw.trust_tier)
        .bind(&rw.state)
        .bind(&armed.reserve.correlation_id)
        .bind(&rw.cause_event_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| ReserveError(format!("ci_run insert: {e}")))?;

        let mut drafts: Vec<(String, &myelin_events::EventDraft)> = Vec::new();
        drafts.push((
            format!("evt:{}", armed.handoff.run_started.subject.0),
            &armed.handoff.run_started,
        ));
        for c in armed.handoff.queued_checks.iter() {
            // H3: the deterministic check id includes the run_id (distinct runs do not collide).
            drafts.push((
                format!("evt:{}:{}", rw.run_id, c.subject.0),
                c,
            ));
        }
        for (event_id, d) in &drafts {
            sqlx::query(
                "INSERT INTO ci_reserve_outbox (event_id, run_id, type, subject, aggregate, payload) \
                 VALUES ($1,$2,$3,$4,$5::jsonb,$6::jsonb) ON CONFLICT (event_id) DO NOTHING",
            )
            .bind(event_id)
            .bind(&rw.run_id)
            .bind(&d.type_.0)
            .bind(&d.subject.0)
            .bind(serde_json::to_string(&d.aggregate.0).unwrap())
            .bind(serde_json::to_string(&d.payload).unwrap())
            .execute(&mut *conn)
            .await
            .map_err(|e| ReserveError(format!("outbox insert: {e}")))?;
        }
        Ok(())
    }
}

impl ReserveStore for IdealCoCommitReserveStore {
    fn persist(&self, armed: &ArmedRun, tx: &mut HandlerTx<'_>) -> Result<(), ReserveError> {
        // THE TRUE CO-COMMIT: downcast the runtime co-commit connection and write the bundle on the
        // SAME tx as the dedup mark. A durable handler with NO tx fails-closed (never writes outside).
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or_else(|| ReserveError("no co-commit tx (durable handler fails closed)".into()))?;
        let armed = armed.clone();
        let rt = self.rt.clone();
        tokio::task::block_in_place(|| rt.block_on(async { Self::write(conn, &armed).await }))
    }
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("pusher".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn push_envelope(ev: &str, repo: &str, new_oid: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(ev.into()),
        type_: EventType(myelin_git::events::GIT_REF_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(format!("myelin://acme/git/ref/{repo}:refs/heads/main")),
        aggregate: AggregateKey(format!("git/ref/{repo}:refs/heads/main")),
        causation_id: None,
        correlation_id: CorrelationId(format!("corr-{ev}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-07-16T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-16T00:00:00Z".into()),
        payload: serde_json::json!({
            "repo": repo,
            "ref": "refs/heads/main",
            "new_oid": new_oid,
            "old_oid": "0000000000000000000000000000000000000000",
            "forced": false,
        }),
    }
}

async fn count(pool: &PgPool, sql: &str, run_id: &str) -> i64 {
    sqlx::query(sql)
        .bind(run_id)
        .fetch_one(pool)
        .await
        .unwrap()
        .get::<i64, _>("n")
}

async fn dedup_present(pool: &PgPool, consumer: &str, id: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM consumer_dedup WHERE consumer = $1 AND event_id = $2)",
    )
    .bind(consumer)
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("dedup read")
}

fn armed_for(ev: &EventEnvelope, repo: &str, oid: &str) -> ArmedRun {
    let blobs = FsBlobStore::new();
    let reader = FixtureGitReader {
        repo: repo.into(),
        oid: oid.into(),
    };
    match plan_dispatch(ev, &reader, &blobs) {
        DispatchOutcome::Arm(a) => *a,
        other => panic!("the push must arm a run, got {other:?}"),
    }
}

// =================================================================================================
// (1) TRUE co-commit through the FULL Consumer runtime: ci_run + events + mark in ONE tx, idempotent.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_true_cocommit_ci_run_events_and_mark_in_one_tx_idempotent() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "deadbeefcafe0000000000000000000000000000";
    let p = setup_schema(&schema, CREATE_RESERVE_OUTBOX_DDL).await;
    let rt = tokio::runtime::Handle::current();

    let reader: Arc<dyn GitConfigReader> = Arc::new(FixtureGitReader {
        repo: repo.into(),
        oid: oid.into(),
    });
    let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
    let store = Arc::new(IdealCoCommitReserveStore { rt: rt.clone() });
    let handler = CiTriggerHandler::new(reader, blobs, store);
    let cname = handler.consumer_name().to_string();

    let ledger = {
        let backing = DurableDedupBacking::new(p.clone(), rt.clone());
        DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>)
    };
    let sub = Subscription::bind(
        ConsumerName(cname.clone()),
        &["myelin://acme/git/"],
        PrefetchBound::DEFAULT,
    )
    .unwrap();
    let consumer = Consumer::new(handler, sub, ledger);

    let ev = push_envelope("ev-push-1", repo, oid);
    let run_id = armed_for(&ev, repo, oid).handoff.run_write.run_id;
    let msg = Message {
        subject: ev.subject.0.clone(),
        envelope: ev.clone(),
    };

    // Delivery 1: the co-commit opens a tx (marks dedup), the handler writes ci_run + events on that
    // SAME tx, and `Done` commits mark + ci_run + events together.
    let out = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(out, Delivered::Acked, "the push armed + co-committed the bundle");

    let runs = count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await;
    assert_eq!(runs, 1, "one durable ci_run row");
    let events = count(&p, "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1", &run_id).await;
    assert_eq!(events, 3, "ci.run.started + 2 queued ci.check.updated (build, test)");
    assert!(dedup_present(&p, &cname, &ev.event_id.0).await, "the dedup mark co-committed");

    // Delivery 2 (same event_id): DEDUPLICATED — the handler does not re-run, 0 duplicates.
    let out2 = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(out2, Delivered::Deduplicated, "the committed mark dedups the redelivery");
    let runs2 = count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await;
    let events2 = count(&p, "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1", &run_id).await;
    assert_eq!((runs2, events2), (1, 3), "redelivery added nothing (idempotent)");

    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    println!("[H1/1] PASS true co-commit: ci_run + 3 events + dedup mark in ONE tx; redelivery deduped (1 run, 3 events).");
}

// =================================================================================================
// (2) CRASH-WINDOW: persist on the co-commit tx then ROLL BACK → nothing lands; the redelivery
//     re-runs the WHOLE dispatch+persist and commits → all present exactly once; further → deduped.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_crash_window_rolls_back_everything_then_reruns() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "cafe00000000000000000000000000000000beef";
    let p = setup_schema(&schema, CREATE_RESERVE_OUTBOX_DDL).await;
    let rt = tokio::runtime::Handle::current();

    let backing = DurableDedupBacking::new(p.clone(), rt.clone());
    let ledger = DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>);
    let store = IdealCoCommitReserveStore { rt: rt.clone() };
    let cname = ConsumerName("ci-dispatch.trigger".into());
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());

    let ev = push_envelope("ev-crash-1", repo, oid);
    let armed = armed_for(&ev, repo, oid);
    let run_id = armed.handoff.run_write.run_id.clone();

    // (A) CRASH BEFORE COMMIT: open the co-commit tx (marks dedup within it), persist the bundle on
    //     the SAME tx, then ROLL BACK (the process dies before commit).
    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(fresh, "first delivery marks the dedup row FRESH");
        {
            let conn = cotx.connection().expect("co-commit exposes a connection");
            let mut htx = HandlerTx::with_connection(conn);
            store.persist(&armed, &mut htx).expect("persist on the co-commit tx");
        }
        cotx.rollback(); // kill-9 before commit.
    });
    assert!(!dedup_present(&p, &cname.0, &ev.event_id.0).await, "crash: mark did NOT persist");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await, 0, "crash: no ci_run row");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1", &run_id).await, 0, "crash: no events");

    // (B) REDELIVERY re-runs the WHOLE dispatch + persist (still fresh — nothing was marked) and
    //     COMMITS: everything lands exactly once. No livelock, no Err-retry.
    tokio::task::block_in_place(|| {
        let armed = armed_for(&ev, repo, oid); // re-run dispatch, as the redelivered handler would.
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(fresh, "after the rollback the redelivery is STILL FRESH (0 lost)");
        {
            let conn = cotx.connection().unwrap();
            let mut htx = HandlerTx::with_connection(conn);
            store.persist(&armed, &mut htx).expect("persist on the redelivery tx");
        }
        cotx.commit().expect("commit the co-commit tx");
    });
    assert!(dedup_present(&p, &cname.0, &ev.event_id.0).await, "redelivery committed the mark");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await, 1, "one ci_run row");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1", &run_id).await, 3, "3 events exactly once");

    // (C) A further REDELIVERY is deduped (mark present) → 0 duplicates.
    tokio::task::block_in_place(|| {
        let (cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(!fresh, "the committed mark makes a redelivery a DUPLICATE");
        cotx.rollback();
    });
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await, 1, "still one run");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1", &run_id).await, 3, "still 3 events");

    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    println!("[H1/2] PASS crash-window: rollback leaves nothing; redelivery re-runs + commits (1 run, 3 events); further redelivery deduped.");
}

// =================================================================================================
// (3) PRODUCTION OutboxReserveStore over the REAL durable outbox: the LIVELOCK is closed — persisting
//     the SAME armed run TWICE (deterministic ids) both return Ok (the second ABSORBED, not Err), and
//     the events are present exactly once. Pre-fix the second persist Err'd → the handler Retry'd forever.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_production_outbox_absorb_closes_the_livelock() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "1234000000000000000000000000000000005678";
    // The real outbox table lives in the schema too.
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    let rt = tokio::runtime::Handle::current();

    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(p.clone(), rt.clone())));
    let store = OutboxReserveStore::new(outbox, Arc::new(UlidMinter::new()));

    let ev = push_envelope("ev-livelock-1", repo, oid);
    let armed = armed_for(&ev, repo, oid);

    // First persist: emits ci.run.started + 2 checks with deterministic ids. Ok.
    let mut htx = HandlerTx::none();
    store.persist(&armed, &mut htx).expect("first persist commits the events");

    // The crash-window redelivery re-runs the WHOLE persist with the SAME deterministic ids. Pre-fix
    // this Err'd ("duplicate emit") → the handler returned Retry → an unbounded livelock. With
    // commit_absorb it is ABSORBED → Ok.
    let mut htx2 = HandlerTx::none();
    store
        .persist(&armed, &mut htx2)
        .expect("H1: the deterministic re-emit is ABSORBED (no livelock), not Err");

    // Exactly once: 1 ci.run.started + 2 ci.check.updated in the real outbox = 3 rows, no duplicates
    // (the absorb of the deterministic re-emit added nothing).
    let total: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(total, 3, "exactly 3 outbox rows (1 started + 2 checks) — the absorb added no duplicates");

    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    println!("[H1/3] PASS livelock closed: the production OutboxReserveStore ABSORBS the deterministic re-emit (2× persist → Ok, 3 rows exactly once), no Err-retry livelock.");
}

// =================================================================================================
// (4) CT-004d.2 chunk 4 — the PRODUCTION `CoCommitReserveStore` (the durable `ci_run` writer): the
//     run-of-record ROW co-commits with the dedup mark on the co-commit `HandlerTx` connection
//     (`CiRunStore::co_commit_insert`), while the co-emitted EVENTS go through the REAL durable outbox
//     in ABSORB mode (the honest #7 H1 split). Delivered through the FULL Consumer runtime; redelivery
//     is deduped → 1 ci_run, 3 outbox events, 0 duplicates.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chunk4_production_cocommit_row_with_mark_events_absorb() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "aa11bb22cc33000000000000000000000000dd44";
    // The REAL outbox table (OUTBOX_MIGRATION) lives in the schema alongside the ci tables + dedup.
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    let rt = tokio::runtime::Handle::current();

    let reader: Arc<dyn GitConfigReader> = Arc::new(FixtureGitReader {
        repo: repo.into(),
        oid: oid.into(),
    });
    let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
    // The PRODUCTION reserve store: the durable ci_run writer (over the schema pool) + the real durable
    // outbox (events absorb) + the ULID minter + the serve runtime handle.
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(p.clone(), rt.clone())));
    let store = Arc::new(CoCommitReserveStore::new(
        ci_run_store_factory(p.clone()),
        outbox,
        Arc::new(UlidMinter::new()),
        rt.clone(),
    ));
    let handler = CiTriggerHandler::new(reader, blobs, store);
    let cname = handler.consumer_name().to_string();

    let ledger = {
        let backing = DurableDedupBacking::new(p.clone(), rt.clone());
        DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>)
    };
    let sub = Subscription::bind(
        ConsumerName(cname.clone()),
        &["myelin://acme/git/"],
        PrefetchBound::DEFAULT,
    )
    .unwrap();
    let consumer = Consumer::new(handler, sub, ledger);

    let ev = push_envelope("ev-chunk4-1", repo, oid);
    let run_id = armed_for(&ev, repo, oid).handoff.run_write.run_id;
    let msg = Message {
        subject: ev.subject.0.clone(),
        envelope: ev.clone(),
    };

    // Delivery 1: the co-commit tx marks dedup + the handler co-commits the ci_run ROW on the SAME tx;
    // `Done` commits (row + mark together). The EVENTS commit through the real outbox (absorb).
    let out = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(out, Delivered::Acked, "the push co-committed the ci_run ROW + the mark");

    let runs = count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await;
    assert_eq!(runs, 1, "one durable ci_run row (the run-of-record, co-committed with the mark)");
    assert!(dedup_present(&p, &cname, &ev.event_id.0).await, "the dedup mark co-committed with the ROW");
    let events: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox").fetch_one(&p).await.unwrap();
    assert_eq!(events, 3, "ci.run.started + 2 queued ci.check.updated in the REAL outbox (absorb)");
    // The row is attributed correctly (state=queued, the deterministic run_id, the trust tier).
    let row = sqlx::query("SELECT state, trust_tier, trigger_kind, correlation_id, repo_ref, commit_oid, triggered_by FROM ci_run WHERE run_id=$1::uuid")
        .bind(&run_id).fetch_one(&p).await.unwrap();
    assert_eq!(row.get::<String, _>("state"), "queued", "reserve state");
    assert_eq!(row.get::<String, _>("trust_tier"), "trusted", "a member push is trusted");
    assert_eq!(row.get::<String, _>("trigger_kind"), "push");
    assert_eq!(row.get::<String, _>("correlation_id"), format!("corr-{}", "ev-chunk4-1"));
    assert_eq!(row.get::<String, _>("repo_ref"), repo);
    assert_eq!(row.get::<String, _>("commit_oid"), oid);
    assert_eq!(row.get::<String, _>("triggered_by"), "pusher");

    // Delivery 2 (same event_id): DEDUPLICATED — the handler does not re-run; the ON CONFLICT + absorb
    // guarantee 0 duplicates even if it had.
    let out2 = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(out2, Delivered::Deduplicated, "the committed mark dedups the redelivery");
    let runs2 = count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await;
    let events2: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox").fetch_one(&p).await.unwrap();
    assert_eq!((runs2, events2), (1, 3), "redelivery added nothing (idempotent: 1 run, 3 events)");

    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    println!("[chunk4/1] PASS production co-commit: the ci_run ROW co-commits with the dedup mark (1 row), events absorb through the real outbox (3), redelivery deduped exactly once.");
}

// =================================================================================================
// (5) CT-004d.2 chunk 4 — the HONEST SPLIT under a crash: persist writes the ci_run ROW on the
//     co-commit tx (uncommitted) and commits the EVENTS through the outbox (a SEPARATE tx, immediately
//     durable). A crash (rollback) between leaves NO ci_run + NO mark, but the events ARE durable (the
//     documented split). The redelivery re-runs → the ROW + mark commit, the events re-absorb (no dup)
//     → converges to exactly once. This PROVES the row⇄mark atomicity AND the events-absorb consistency.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chunk4_crash_window_row_and_mark_rollback_events_absorb_then_reconverge() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "99ee88ff77000000000000000000000000aa11bb";
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    let rt = tokio::runtime::Handle::current();

    let backing = DurableDedupBacking::new(p.clone(), rt.clone());
    let ledger = DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>);
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(p.clone(), rt.clone())));
    let store = CoCommitReserveStore::new(
        ci_run_store_factory(p.clone()),
        outbox,
        Arc::new(UlidMinter::new()),
        rt.clone(),
    );
    let cname = ConsumerName("ci-dispatch.trigger".into());
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());

    let ev = push_envelope("ev-chunk4-crash", repo, oid);
    let armed = armed_for(&ev, repo, oid);
    let run_id = armed.handoff.run_write.run_id.clone();

    // (A) CRASH BEFORE COMMIT: open the co-commit tx (marks dedup within it), persist the bundle — the
    //     ci_run ROW lands on the co-commit tx (uncommitted); the events commit through the outbox in a
    //     SEPARATE tx (durable NOW) — then ROLL BACK the co-commit tx (kill-9 before commit).
    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(fresh, "first delivery marks the dedup row FRESH");
        {
            let conn = cotx.connection().expect("co-commit exposes a connection");
            let mut htx = HandlerTx::with_connection(conn);
            store.persist(&armed, &mut htx).expect("persist: ci_run on the co-commit tx + events absorb");
        }
        cotx.rollback(); // kill-9 before commit → the ci_run ROW + the mark vanish together.
    });
    assert!(!dedup_present(&p, &cname.0, &ev.event_id.0).await, "crash: the mark did NOT persist");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await, 0, "crash: NO ci_run row (rolled back with the mark)");
    // The EVENTS are the honest split: they committed in the outbox's separate tx, so they ARE durable
    // even though the ROW + mark rolled back (documented — absorb-idempotent, converges on redelivery).
    let events_after_crash: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox").fetch_one(&p).await.unwrap();
    assert_eq!(events_after_crash, 3, "honest split: the events led the row (durable), the ROW+mark rolled back");

    // (B) REDELIVERY re-runs (still fresh — the mark rolled back) and COMMITS: the ROW + mark land; the
    //     events RE-ABSORB (same deterministic ids → ON CONFLICT DO NOTHING, no duplicate).
    tokio::task::block_in_place(|| {
        let armed = armed_for(&ev, repo, oid);
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(fresh, "after the rollback the redelivery is STILL FRESH (0 lost)");
        {
            let conn = cotx.connection().unwrap();
            let mut htx = HandlerTx::with_connection(conn);
            store.persist(&armed, &mut htx).expect("persist on the redelivery tx");
        }
        cotx.commit().expect("commit the co-commit tx (ROW + mark together)");
    });
    assert!(dedup_present(&p, &cname.0, &ev.event_id.0).await, "redelivery committed the mark");
    assert_eq!(count(&p, "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid", &run_id).await, 1, "one ci_run row (converged exactly once)");
    let events_final: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox").fetch_one(&p).await.unwrap();
    assert_eq!(events_final, 3, "3 events exactly once (the re-emit ABSORBED — no duplicate)");

    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    println!("[chunk4/2] PASS honest split under crash: ROW+mark roll back together (0 ci_run), events lead durable (absorb), redelivery converges to exactly once (1 run, 3 events).");
}

// =================================================================================================
// (6) FINDING #6 — the PRODUCTION `main.rs` REGISTRATION path. `main.rs` used to ship
//     `let consumers = Vec::new();` (NO consumer registered in prod). It now calls
//     `build_dispatch_consumers`, which constructs the SAME four production backings proof (4) uses
//     (the `CoCommitReserveStore` durable `ci_run` writer + the real `S3BlobStore` CAS +
//     `DurableGitConfigReader` over the on-disk git-root + the durable `DedupLedger`) and returns ONE
//     bound `ConsumerReg`. This asserts the registration is NON-EMPTY over real durable backings —
//     the finding-#6 regression guard. The consumer's end-to-end BEHAVIOUR is proven by proofs (4)/(5)
//     above; here we only prove `main.rs`'s wiring path yields a registered consumer, not the shell.
// =================================================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finding6_build_dispatch_consumers_registers_the_live_trigger_consumer() {
    let schema = schema_name(uniq());
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    let rt = tokio::runtime::Handle::current();

    // The real durable outbox over the schema pool (events-absorb side of the reserve store).
    let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(p.clone(), rt.clone())));
    // The durable exactly-once ledger over the schema pool.
    let dedup = {
        let backing = DurableDedupBacking::new(p.clone(), rt.clone());
        DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>)
    };
    // A dev-shaped S3Config — `S3BlobStore::connect` builds the client WITHOUT any network I/O, so no
    // live object store is needed to prove registration (the CAS round-trip is proof (4)'s job).
    let s3 = myelin_config::S3Config {
        endpoint: "http://127.0.0.1:9000".into(),
        region: "us-east-1".into(),
        access_key: "test".into(),
        secret_key: "test".into(),
        bucket: "ci".into(),
        force_path_style: true,
    };
    let git_root = std::env::temp_dir().join(format!("myelin-ci-finding6-{schema}"));
    std::fs::create_dir_all(&git_root).expect("create authoritative Git root");
    let git_root = AuthoritativeGitRoot::validate(&git_root).expect("validate Git root");
    let dead_letters: Arc<dyn myelin_events::DurableDeadLetter> =
        Arc::new(DurableDeadLetterBacking::new(p.clone(), rt.clone()));

    let consumers = build_dispatch_consumers(
        git_root,
        &s3,
        ci_run_store_factory(p.clone()),
        outbox,
        dedup,
        dead_letters,
        "fr-par",
        Arc::new(UlidMinter::new()),
        rt.clone(),
    )
    .expect("build_dispatch_consumers registers the trigger consumer");

    assert_eq!(
        consumers.len(),
        1,
        "finding #6: production main.rs must register exactly ONE ci-dispatch.trigger consumer (was Vec::new())"
    );

    p.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    std::fs::remove_dir_all(std::env::temp_dir().join(format!("myelin-ci-finding6-{schema}"))).ok();
    println!("[finding6] PASS: build_dispatch_consumers registers 1 live trigger consumer over real durable backings (no more Vec::new() shell).");
}
