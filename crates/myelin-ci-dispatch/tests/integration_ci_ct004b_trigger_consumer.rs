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

fn schema_name(k: u64) -> String {
    format!("ci_ct004b_{}_{}", std::process::id(), k)
}

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
        .expect("reconnect to dev Postgres (is the stack up? `fed test:backend`)")
}

async fn reopen_app(schema: &str) -> PgPool {
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
        .connect(&app_url())
        .await
        .expect("connect the constrained runtime role to dev Postgres")
}

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

struct CatchUnwind<F> {
    inner: std::pin::Pin<Box<F>>,
}

impl<F: std::future::Future> std::future::Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner.as_mut().poll(cx)
        })) {
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(payload) => std::task::Poll::Ready(Err(payload)),
        }
    }
}

async fn with_schema_cleanup<Fut>(pool: &PgPool, schema: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = CatchUnwind {
        inner: Box::pin(body()),
    }
    .await;
    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(pool)
        .await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

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

const VALID_PR_CI_TOML: &str = "\
on = \"pull_request\"

[[jobs]]
name = \"build\"
image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"
command = [\"build\"]
";

struct FixtureGitReader {
    repo: String,
    oid: String,
}

struct PrFixtureGitReader {
    repo: String,
    oid: String,
}

impl GitConfigReader for PrFixtureGitReader {
    fn read_repo_file(
        &self,
        _tenant: &str,
        _region: &str,
        repo: &str,
        oid: &str,
        path: &str,
    ) -> Result<Option<Vec<u8>>, GitReadError> {
        if repo == self.repo && oid == self.oid && path == ".myelin/ci.toml" {
            Ok(Some(VALID_PR_CI_TOML.as_bytes().to_vec()))
        } else {
            Ok(None)
        }
    }
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
            drafts.push((format!("evt:{}:{}", rw.run_id, c.subject.0), c));
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
    let ref_key = myelin_git::receive_pack::GitRefEventKey::new(
        repo,
        &myelin_git::receive_pack::RefName::new("refs/heads/main"),
    )
    .unwrap();
    EventEnvelope {
        event_id: EventId(ev.into()),
        type_: EventType(myelin_git::events::GIT_REF_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(principal()),
        subject: ref_key.subject("acme").unwrap(),
        aggregate: ref_key.aggregate(),
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

fn pr_envelope(ev: &str, repo: &str, number: u64, head_oid: &str) -> EventEnvelope {
    let mut envelope = push_envelope(ev, repo, head_oid);
    envelope.type_ = EventType(myelin_git::events::GIT_PR_OPENED.into());
    envelope.schema_ver = myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2;
    envelope.subject = ArtifactRef(format!("myelin://acme/git/pr/{repo}:{number}"));
    envelope.aggregate = AggregateKey(format!("git/pr/{repo}:{number}"));
    envelope.payload = serde_json::json!({
        "repo": repo,
        "number": number,
        "head_oid": head_oid,
        "head_generation": 1,
        "is_fork": false,
    });
    envelope
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_true_cocommit_ci_run_events_and_mark_in_one_tx_idempotent() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "deadbeefcafe0000000000000000000000000000";
    let p = setup_schema(&schema, CREATE_RESERVE_OUTBOX_DDL).await;
    with_schema_cleanup(&p, &schema, || async {
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

    let out = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(
        out,
        Delivered::Acked,
        "the push armed + co-committed the bundle"
    );

    let runs = count(
        &p,
        "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
        &run_id,
    )
    .await;
    assert_eq!(runs, 1, "one durable ci_run row");
    let events = count(
        &p,
        "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1",
        &run_id,
    )
    .await;
    assert_eq!(
        events, 3,
        "ci.run.started + 2 queued ci.check.updated (build, test)"
    );
    assert!(
        dedup_present(&p, &cname, &ev.event_id.0).await,
        "the dedup mark co-committed"
    );

    let out2 = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(
        out2,
        Delivered::Deduplicated,
        "the committed mark dedups the redelivery"
    );
    let runs2 = count(
        &p,
        "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
        &run_id,
    )
    .await;
    let events2 = count(
        &p,
        "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1",
        &run_id,
    )
    .await;
    assert_eq!(
        (runs2, events2),
        (1, 3),
        "redelivery added nothing (idempotent)"
    );

    println!("[H1/1] PASS true co-commit: ci_run + 3 events + dedup mark in ONE tx; redelivery deduped (1 run, 3 events).");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_crash_window_rolls_back_everything_then_reruns() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "cafe00000000000000000000000000000000beef";
    let p = setup_schema(&schema, CREATE_RESERVE_OUTBOX_DDL).await;
    with_schema_cleanup(&p, &schema, || async {
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

    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(fresh, "first delivery marks the dedup row FRESH");
        {
            let conn = cotx.connection().expect("co-commit exposes a connection");
            let mut htx = HandlerTx::with_connection(conn);
            store
                .persist(&armed, &mut htx)
                .expect("persist on the co-commit tx");
        }
        cotx.rollback();
    });
    assert!(
        !dedup_present(&p, &cname.0, &ev.event_id.0).await,
        "crash: mark did NOT persist"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
            &run_id
        )
        .await,
        0,
        "crash: no ci_run row"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1",
            &run_id
        )
        .await,
        0,
        "crash: no events"
    );

    tokio::task::block_in_place(|| {
        let armed = armed_for(&ev, repo, oid);
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(
            fresh,
            "after the rollback the redelivery is STILL FRESH (0 lost)"
        );
        {
            let conn = cotx.connection().unwrap();
            let mut htx = HandlerTx::with_connection(conn);
            store
                .persist(&armed, &mut htx)
                .expect("persist on the redelivery tx");
        }
        cotx.commit().expect("commit the co-commit tx");
    });
    assert!(
        dedup_present(&p, &cname.0, &ev.event_id.0).await,
        "redelivery committed the mark"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
            &run_id
        )
        .await,
        1,
        "one ci_run row"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1",
            &run_id
        )
        .await,
        3,
        "3 events exactly once"
    );

    tokio::task::block_in_place(|| {
        let (cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(!fresh, "the committed mark makes a redelivery a DUPLICATE");
        cotx.rollback();
    });
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
            &run_id
        )
        .await,
        1,
        "still one run"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1",
            &run_id
        )
        .await,
        3,
        "still 3 events"
    );

    println!("[H1/2] PASS crash-window: rollback leaves nothing; redelivery re-runs + commits (1 run, 3 events); further redelivery deduped.");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h1_test_fixture_outbox_absorb_closes_the_livelock() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "1234000000000000000000000000000000005678";
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    with_schema_cleanup(&p, &schema, || async {
        let rt = tokio::runtime::Handle::current();

        let outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(p.clone(), rt.clone())));
        let store = OutboxReserveStore::new(outbox, Arc::new(UlidMinter::new()));

        let ev = push_envelope("ev-livelock-1", repo, oid);
        let armed = armed_for(&ev, repo, oid);

        let mut htx = HandlerTx::none();
        store
            .persist(&armed, &mut htx)
            .expect("first persist commits the events");

        let mut htx2 = HandlerTx::none();
        store
            .persist(&armed, &mut htx2)
            .expect("H1: the deterministic re-emit is ABSORBED (no livelock), not Err");

        let total: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(
            total, 3,
            "exactly 3 outbox rows (1 started + 2 checks) - the absorb added no duplicates"
        );
        let envelopes: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT envelope FROM outbox ORDER BY aggregate, seq")
                .fetch_all(&p)
                .await
                .unwrap();
        assert!(
            envelopes.iter().all(|envelope| {
                envelope["causation_id"] == ev.event_id.0
                    && envelope["correlation_id"] == ev.correlation_id.0
                    && envelope["depth"] == ev.depth + 1
            }),
            "every reserve/start fact preserves the trigger's immediate parent, root, and depth"
        );

        println!("[H1/3] PASS test-fixture absorb: deterministic re-emit remains exactly once.");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_cocommits_run_attempt_events_and_mark() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "aa11bb22cc33000000000000000000000000dd44";
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    with_schema_cleanup(&p, &schema, || async {
    let rt = tokio::runtime::Handle::current();

    let reader: Arc<dyn GitConfigReader> = Arc::new(FixtureGitReader {
        repo: repo.into(),
        oid: oid.into(),
    });
    let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
    let store = Arc::new(CoCommitReserveStore::new(
        ci_run_store_factory(p.clone()),
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

    let out = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(
        out,
        Delivered::Acked,
        "the push co-committed the ci_run ROW + the mark"
    );

    let runs = count(
        &p,
        "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
        &run_id,
    )
    .await;
    assert_eq!(
        runs, 1,
        "one durable ci_run row (the run-of-record, co-committed with the mark)"
    );
    assert!(
        dedup_present(&p, &cname, &ev.event_id.0).await,
        "the dedup mark co-committed with the ROW"
    );
    let events: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(
        events, 3,
        "ci.run.started + 2 queued ci.check.updated in the REAL outbox (absorb)"
    );
    let row = sqlx::query("SELECT state, trust_tier, trigger_kind, correlation_id, repo_ref, commit_oid, triggered_by FROM ci_run WHERE run_id=$1::uuid")
        .bind(&run_id).fetch_one(&p).await.unwrap();
    assert_eq!(row.get::<String, _>("state"), "queued", "reserve state");
    assert_eq!(
        row.get::<String, _>("trust_tier"),
        "trusted",
        "a member push is trusted"
    );
    assert_eq!(row.get::<String, _>("trigger_kind"), "push");
    assert_eq!(
        row.get::<String, _>("correlation_id"),
        format!("corr-{}", "ev-chunk4-1")
    );
    assert_eq!(
        row.get::<String, _>("repo_ref"),
        format!("myelin://acme/git/repo/{repo}")
    );
    assert_eq!(row.get::<String, _>("commit_oid"), oid);
    assert_eq!(row.get::<String, _>("triggered_by"), "pusher");

    let out2 = tokio::task::block_in_place(|| consumer.deliver(&msg));
    assert_eq!(
        out2,
        Delivered::Deduplicated,
        "the committed mark dedups the redelivery"
    );
    let runs2 = count(
        &p,
        "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
        &run_id,
    )
    .await;
    let events2: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(
        (runs2, events2),
        (1, 3),
        "redelivery added nothing (idempotent: 1 run, 3 events)"
    );

    println!("[chunk4/1] PASS production co-commit: run+attempt+events+mark committed once; redelivery deduped.");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_pr_cocommit_persists_canonical_concurrency_identity() {
    let schema = schema_name(uniq());
    let repo = "team/web";
    let number = 42;
    let oid = "aa11bb22cc33000000000000000000000000dd44";
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    with_schema_cleanup(&p, &schema, || async {
        let rt = tokio::runtime::Handle::current();
        let reader: Arc<dyn GitConfigReader> = Arc::new(PrFixtureGitReader {
            repo: repo.into(),
            oid: oid.into(),
        });
        let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
        let store = Arc::new(CoCommitReserveStore::new(
            ci_run_store_factory(p.clone()),
            Arc::new(UlidMinter::new()),
            rt.clone(),
        ));
        let handler = CiTriggerHandler::new(reader, blobs, store);
        let cname = handler.consumer_name().to_string();
        let ledger =
            DedupLedger::durable(Arc::new(DurableDedupBacking::new(p.clone(), rt.clone())));
        let sub = Subscription::bind(
            ConsumerName(cname),
            &["myelin://acme/git/"],
            PrefetchBound::DEFAULT,
        )
        .unwrap();
        let consumer = Consumer::new(handler, sub, ledger);
        let ev = pr_envelope("ev-chunk4-pr", repo, number, oid);
        let run_id = match plan_dispatch(
            &ev,
            &PrFixtureGitReader {
                repo: repo.into(),
                oid: oid.into(),
            },
            &FsBlobStore::new(),
        ) {
            DispatchOutcome::Arm(armed) => armed.handoff.run_write.run_id,
            other => panic!("validated PR must arm, got {other:?}"),
        };
        let msg = Message {
            subject: ev.subject.0.clone(),
            envelope: ev,
        };

        assert_eq!(
            tokio::task::block_in_place(|| consumer.deliver(&msg)),
            Delivered::Acked
        );
        let row = sqlx::query(
            "SELECT trigger_kind, concurrency_group, pr_head_generation \
           FROM ci_run WHERE run_id = $1::uuid",
        )
        .bind(run_id)
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("trigger_kind"), "pull_request");
        assert_eq!(
            row.get::<Option<String>, _>("concurrency_group").as_deref(),
            Some("pr:team/web:42")
        );
        assert_eq!(row.get::<Option<i64>, _>("pr_head_generation"), Some(1));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_crash_rolls_back_run_attempt_events_and_mark_then_reconverges() {
    let schema = schema_name(uniq());
    let repo = "web";
    let oid = "99ee88ff77000000000000000000000000aa11bb";
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    with_schema_cleanup(&p, &schema, || async {
    let rt = tokio::runtime::Handle::current();

    let backing = DurableDedupBacking::new(p.clone(), rt.clone());
    let ledger = DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>);
    let store = CoCommitReserveStore::new(
        ci_run_store_factory(p.clone()),
        Arc::new(UlidMinter::new()),
        rt.clone(),
    );
    let cname = ConsumerName("ci-dispatch.trigger".into());
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());

    let ev = push_envelope("ev-chunk4-crash", repo, oid);
    let armed = armed_for(&ev, repo, oid);
    let run_id = armed.handoff.run_write.run_id.clone();

    tokio::task::block_in_place(|| {
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(fresh, "first delivery marks the dedup row FRESH");
        {
            let conn = cotx.connection().expect("co-commit exposes a connection");
            let mut htx = HandlerTx::with_connection(conn);
            store
                .persist(&armed, &mut htx)
                .expect("stage the complete reserve bundle");
        }
        cotx.rollback();
    });
    assert!(
        !dedup_present(&p, &cname.0, &ev.event_id.0).await,
        "crash: the mark did NOT persist"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
            &run_id
        )
        .await,
        0,
        "crash: NO ci_run row (rolled back with the mark)"
    );
    let events_after_crash: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(
        events_after_crash, 0,
        "rollback leaves no queued fact without its run and attempt authority"
    );
    let attempts_after_crash: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM check_attempt")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(
        attempts_after_crash, 0,
        "attempt allocation rolled back too"
    );
    let issued_after_crash: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run_check_attempt")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(
        issued_after_crash, 0,
        "run-scoped attempt authority rolled back too"
    );

    tokio::task::block_in_place(|| {
        let armed = armed_for(&ev, repo, oid);
        let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &ev.event_id, &tenant, &region);
        assert!(
            fresh,
            "after the rollback the redelivery is STILL FRESH (0 lost)"
        );
        {
            let conn = cotx.connection().unwrap();
            let mut htx = HandlerTx::with_connection(conn);
            store
                .persist(&armed, &mut htx)
                .expect("persist on the redelivery tx");
        }
        cotx.commit()
            .expect("commit the co-commit tx (ROW + mark together)");
    });
    assert!(
        dedup_present(&p, &cname.0, &ev.event_id.0).await,
        "redelivery committed the mark"
    );
    assert_eq!(
        count(
            &p,
            "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
            &run_id
        )
        .await,
        1,
        "one ci_run row (converged exactly once)"
    );
    let events_final: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM outbox")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(events_final, 3, "3 reserve events committed exactly once");
    let issued_attempt: i32 =
        sqlx::query_scalar("SELECT next_attempt - 1 FROM check_attempt WHERE context='build'")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(issued_attempt, 1);
    let run_attempt: i32 =
        sqlx::query_scalar("SELECT run_attempt FROM ci_run_check_attempt WHERE context='build'")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(run_attempt, 1);

    println!("[chunk4/2] PASS production co-commit crash: run+attempt+events+mark roll back together; redelivery commits the complete bundle exactly once.");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_reserve_and_replay_need_no_update_grant_on_immutable_attempts() {
    let schema = schema_name(uniq());
    let admin = setup_schema(&schema, OUTBOX_MIGRATION).await;
    with_schema_cleanup(&admin, &schema, || async {
        sqlx::raw_sql(&format!(
            "GRANT USAGE ON SCHEMA {schema} TO myelin_app;
         GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA {schema} TO myelin_app;
         REVOKE UPDATE, DELETE ON {schema}.ci_run_check_attempt FROM myelin_app;"
        ))
        .execute(&admin)
        .await
        .expect("install the production-shaped runtime grants");

        let can_select: bool =
            sqlx::query_scalar("SELECT has_table_privilege('myelin_app', $1, 'SELECT')")
                .bind(format!("{schema}.ci_run_check_attempt"))
                .fetch_one(&admin)
                .await
                .unwrap();
        let can_update: bool =
            sqlx::query_scalar("SELECT has_table_privilege('myelin_app', $1, 'UPDATE')")
                .bind(format!("{schema}.ci_run_check_attempt"))
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(can_select);
        assert!(
            !can_update,
            "the immutable attempt authority stays non-updatable"
        );

        let app = reopen_app(&schema).await;
        let rt = tokio::runtime::Handle::current();
        let ledger =
            DedupLedger::durable(Arc::new(DurableDedupBacking::new(app.clone(), rt.clone()))
                as Arc<dyn DurableDedup>);
        let store = CoCommitReserveStore::new(
            ci_run_store_factory(app.clone()),
            Arc::new(UlidMinter::new()),
            rt,
        );
        let cname = ConsumerName("ci-dispatch.trigger".into());
        let tenant = TenantId("acme".into());
        let region = Region("fr-par".into());
        let event = push_envelope(
            "ev-runtime-immutable-attempt",
            "web",
            "1234567890abcdef1234567890abcdef12345678",
        );
        let armed = armed_for(&event, "web", "1234567890abcdef1234567890abcdef12345678");

        for event_id in [
            EventId("ev-runtime-immutable-attempt".into()),
            EventId("ev-runtime-immutable-attempt-replay".into()),
        ] {
            tokio::task::block_in_place(|| {
                let (mut cotx, fresh) = ledger.begin_co_commit(&cname, &event_id, &tenant, &region);
                assert!(fresh);
                {
                    let conn = cotx.connection().expect("co-commit connection");
                    let mut tx = HandlerTx::with_connection(conn);
                    store
                        .persist(&armed, &mut tx)
                        .expect("reserve/replay under SELECT+INSERT attempt authority");
                }
                cotx.commit().expect("commit the production-shaped reserve");
            });
        }

        let issued: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run_check_attempt")
            .fetch_one(&admin)
            .await
            .unwrap();
        assert_eq!(
            issued, 2,
            "one immutable authority per configured check context"
        );

        app.close().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn finding6_build_dispatch_consumers_registers_the_live_trigger_consumer() {
    let schema = schema_name(uniq());
    let p = setup_schema(&schema, OUTBOX_MIGRATION).await;
    with_schema_cleanup(&p, &schema, || async {
    let rt = tokio::runtime::Handle::current();

    let dedup = {
        let backing = DurableDedupBacking::new(p.clone(), rt.clone());
        DedupLedger::durable(Arc::new(backing) as Arc<dyn DurableDedup>)
    };
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
        Arc::new(myelin_storage::s3blob::S3BlobStore::connect(
            &s3,
            rt.clone(),
        )),
        ci_run_store_factory(p.clone()),
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

    std::fs::remove_dir_all(std::env::temp_dir().join(format!("myelin-ci-finding6-{schema}"))).ok();
    println!("[finding6] PASS: build_dispatch_consumers registers 1 live trigger consumer over real durable backings (no more Vec::new() shell).");
    })
    .await;
}
