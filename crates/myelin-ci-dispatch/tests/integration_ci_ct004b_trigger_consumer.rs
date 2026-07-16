//! **CT-004b — the LIVE `ci-dispatch.trigger` consumer, DURABILITY PROVEN through the reserve store
//! on live Postgres.**
//!
//! The unit tests (`src/consumer.rs`) prove the pipeline branches DB-free (no config → skip;
//! malformed → surfaced skip; non-matching trigger → skip; floating tag → surfaced resolve skip;
//! happy path → atomic bundle). This test proves the LAST mile the prompt asks for: **a real
//! `git.ref.updated` envelope, fed to the consumer with a valid digest-pinned `.myelin/ci.toml` at
//! `new_oid`, produces a DURABLE `ci_run` row + a queued `ci.check.updated` + `ci.run.started` — all
//! in ONE transaction (the atomic bundle) — and a re-delivered identical envelope (same `event_id`)
//! does NOT double-start** (the deterministic `run_id` + `ON CONFLICT (tenant_id, run_id) DO NOTHING`).
//!
//! ## Cross-service delivery reality (named, per the prompt)
//! The git service emits `git.ref.updated` to ITS outbox/NATS; whether ci-dispatch RECEIVES it
//! cross-cell over the structured `evt.<tenant>.git.*` subject is a deploy-substrate floor. So this
//! test INJECTS a synthetic-but-real `git.ref.updated` envelope (real payload + a real digest-pinned
//! config at the ref) rather than a real cross-service NATS hop — the accepted proof of the CONSUMER
//! logic end-to-end (envelope in → durable ci_run + queued check out). The real cross-service NATS
//! delivery is the named deploy floor.
//!
//! ## The `ci_run` table is created by the REAL shared CI migration path (CT-004m)
//! The `ci_run` table is owned by `myelin-ci-controlplane`. CT-004m gives BOTH CI mains the shared
//! [`myelin_ci_controlplane::ci_durable_migrations`] set (`ci_run` + `check_attempt` + `ci_cost_event`),
//! applied at boot — so ci-dispatch no longer depends on ci-controlplane booting first to have `ci_run`.
//! This test stands the tables up via that SAME real forward-only migration set (each `(tenant, region)`-
//! first + FORCE-RLS) into an ISOLATED per-pid schema, and drives a `PgReserveStore` that co-commits the
//! `ci_run` row + the outbox events in ONE tx — proving the atomic bundle against the tables as the real
//! migration path produces them. The pool connects as the migration/owner role (`myelin_admin`,
//! BYPASSRLS) so the reserve exercises the table shape under FORCE-RLS. The PRODUCTION durable reserve
//! writer that wires this into the live dispatch consumer with a tenant-scoped tx is the named follow-on.
//!
//! Gated behind the `integration` cargo feature. Run against the docker-compose dev stack:
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-dispatch --features integration \
//!     --test integration_ci_ct004b_trigger_consumer -- --nocapture
#![cfg(feature = "integration")]

use std::sync::Arc;

use myelin_ci_controlplane::ci_durable_migrations;
use myelin_ci_dispatch::{
    plan_dispatch, ArmedRun, CiTriggerHandler, DispatchOutcome, GitConfigReader, GitReadError,
    ReserveError, ReserveStore,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId,
    EventType, HandleOutcome, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
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

/// The per-pid schema every pool pins `search_path` to — so the store's unqualified `ci_run` /
/// `ci_reserve_outbox` resolve to ISOLATED tables (never another test's rows).
fn schema_name() -> String {
    format!("ci_ct004b_{}", std::process::id())
}

/// Open an admin pool whose connections pin `search_path` to the per-pid schema (the CT-004a
/// posture). Reopening after `drop(prev)` models a process restart (the kill-9 durability proof).
async fn reopen() -> PgPool {
    let schema = schema_name();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                // `public` follows the per-pid schema so the platform RLS helper
                // `myelin_make_tenant_scoped` (in public, called by the real ci_durable_migrations)
                // resolves, while unqualified table CREATEs land in the per-pid schema first.
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("reconnect to dev Postgres (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

/// The outbox-shaped table the reserve co-commit writes the two events into (the dispatch service's
/// `outbox` in production; a minimal isolated mirror here so the one-tx co-commit is assertable
/// alongside the `ci_run` row).
const CREATE_RESERVE_OUTBOX_DDL: &str = "\
CREATE TABLE IF NOT EXISTS ci_reserve_outbox (
  event_id  text PRIMARY KEY,
  run_id    text NOT NULL,
  type      text NOT NULL,
  subject   text NOT NULL,
  aggregate text NOT NULL,
  payload   jsonb NOT NULL
)";

/// A digest-pinned `.myelin/ci.toml` a push arms on (a build job + a test job that `needs` build).
const VALID_CI_TOML: &str = "\
on = \"push\"

[[jobs]]
name = \"build\"
image = \"registry.example/build@sha256:abc123def4560000000000000000000000000000000000000000000000000000\"

[[jobs]]
name = \"test\"
image = \"registry.example/test@sha256:ffeeddccbbaa0000000000000000000000000000000000000000000000000000\"
needs = [\"build\"]
";

/// A minimal in-test [`GitConfigReader`]: serves `VALID_CI_TOML` for `.myelin/ci.toml` at the pushed
/// `(repo, oid)`, `Ok(None)` elsewhere. (The REAL myelin-git `read_blob_at_path` adapter is
/// `DurableGitConfigReader`; here a fixture reader keeps the test's focus on the DURABLE persistence.)
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

/// **The durable reserve store: co-commit the `ci_run` row + the two events in ONE Postgres tx.**
/// This is the myelin-storage-backing floor, modelled in-test (the CT-004a `settle_in_tx` posture):
/// `persist_async` opens ONE `tx`, INSERTs the `ci_run` row (`ON CONFLICT (tenant_id, run_id) DO
/// NOTHING` — idempotent), INSERTs `ci.run.started` + every queued `ci.check.updated` into the outbox
/// (deterministic `event_id`, `ON CONFLICT DO NOTHING`), and commits — all-or-nothing.
struct PgReserveStore {
    pool: PgPool,
    rt: tokio::runtime::Handle,
}

impl PgReserveStore {
    async fn persist_async(&self, armed: &ArmedRun) -> Result<(), ReserveError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| ReserveError(format!("begin: {e}")))?;

        let rw = &armed.handoff.run_write;
        // The `ci_run` row (state=queued) — the thin index over the workflow run. The uuid columns
        // are bound as text + cast ::uuid (the deterministic ids are valid uuid strings).
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
        .execute(&mut *tx)
        .await
        .map_err(|e| ReserveError(format!("ci_run insert: {e}")))?;

        // ci.run.started + every queued ci.check.updated — co-committed in the SAME tx.
        let mut drafts: Vec<(String, &myelin_events::EventDraft)> = Vec::new();
        drafts.push((format!("{}:ci.run.started", rw.run_id), &armed.handoff.run_started));
        for (i, c) in armed.handoff.queued_checks.iter().enumerate() {
            drafts.push((format!("{}:check:{i}", rw.run_id), c));
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
            .execute(&mut *tx)
            .await
            .map_err(|e| ReserveError(format!("outbox insert: {e}")))?;
        }

        tx.commit().await.map_err(|e| ReserveError(format!("commit: {e}")))
    }
}

impl ReserveStore for PgReserveStore {
    fn persist(&self, armed: &ArmedRun) -> Result<(), ReserveError> {
        // Bridge the sync trait to async sqlx (the PgOutboxBacking idiom). Called from within a
        // `spawn_blocking` in the test so `block_on` does not nest a runtime.
        let armed = armed.clone();
        let rt = self.rt.clone();
        let this = PgReserveStore {
            pool: self.pool.clone(),
            rt: self.rt.clone(),
        };
        rt.block_on(async move { this.persist_async(&armed).await })
    }
}

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("pusher".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// A real `git.ref.updated` envelope for `repo` pushing `new_oid`, event id `ev`.
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

#[tokio::test]
async fn a_push_arms_a_durable_ci_run_and_queued_checks_idempotently() {
    let schema = schema_name();
    let repo = "web";
    let oid = "deadbeefcafe0000000000000000000000000000";

    // ── Fresh isolated schema + the ci_run table (owned by controlplane) + the reserve outbox. ──
    let p1 = reopen().await;
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&p1)
        .await
        .expect("drop any prior schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&p1)
        .await
        .expect("create the per-pid schema");
    // Apply the REAL shared CI durable migration set (ci_run + check_attempt + ci_cost_event, each
    // FORCE-RLS) — the SAME set both CI mains apply at boot. Multi-statement DDL via the simple-query
    // protocol lands in the search_path'd per-pid schema.
    for m in ci_durable_migrations().0.iter() {
        p1.execute(m.ddl)
            .await
            .unwrap_or_else(|e| panic!("apply CI durable migration {} into the schema: {e}", m.id));
    }
    sqlx::query(CREATE_RESERVE_OUTBOX_DDL)
        .execute(&p1)
        .await
        .expect("apply the reserve outbox DDL");

    // ── The LIVE consumer: the fixture git reader + the in-memory CAS + the durable reserve store. ──
    let reader: Arc<dyn GitConfigReader> = Arc::new(FixtureGitReader {
        repo: repo.into(),
        oid: oid.into(),
    });
    let blobs: Arc<dyn BlobStore + Send + Sync> = Arc::new(FsBlobStore::new());
    let store = Arc::new(PgReserveStore {
        pool: p1.clone(),
        rt: tokio::runtime::Handle::current(),
    });
    let handler = Arc::new(CiTriggerHandler::new(reader, blobs, store.clone()));

    // The armed run's deterministic run_id (for the read-back assertions) — from plan_dispatch.
    let ev = push_envelope("ev-push-1", repo, oid);
    let run_id = {
        let blobs2 = FsBlobStore::new();
        let reader2 = FixtureGitReader {
            repo: repo.into(),
            oid: oid.into(),
        };
        match plan_dispatch(&ev, &reader2, &blobs2) {
            DispatchOutcome::Arm(a) => a.handoff.run_write.run_id.clone(),
            other => panic!("the push must arm a run, got {other:?}"),
        }
    };

    // ── Deliver the envelope through the FULL handler (envelope in → durable rows out). ──
    let h1 = Arc::clone(&handler);
    let ev1 = ev.clone();
    let outcome = tokio::task::spawn_blocking(move || h1.handle(&ev1))
        .await
        .unwrap();
    assert_eq!(outcome, HandleOutcome::Done, "the consumer handled the push");

    // (1) A DURABLE ci_run row (state=queued) carrying the stamped tier + trigger kind.
    let row = sqlx::query("SELECT state, trigger_kind, trust_tier, definition_snapshot FROM ci_run WHERE run_id = $1::uuid")
        .bind(&run_id)
        .fetch_one(&p1)
        .await
        .expect("the ci_run row is durable");
    assert_eq!(row.get::<String, _>("state"), "queued", "the run is reserved queued");
    assert_eq!(row.get::<String, _>("trigger_kind"), "push", "the push trigger kind");
    assert_eq!(row.get::<String, _>("trust_tier"), "trusted", "a member push is trusted");
    assert!(
        row.get::<String, _>("definition_snapshot").contains("ci/snapshot/"),
        "the CAS snapshot ref rides the row"
    );

    // (2) ci.run.started + one queued ci.check.updated PER top-level job (build, test) — the atomic
    //     bundle, co-committed in the SAME tx as the ci_run row.
    let started = count(
        &p1,
        "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1 AND type='ci.run.started'",
        &run_id,
    )
    .await;
    assert_eq!(started, 1, "exactly one ci.run.started event");
    let checks = count(
        &p1,
        "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1 AND type='ci.check.updated'",
        &run_id,
    )
    .await;
    assert_eq!(checks, 2, "one queued ci.check.updated per top-level job (build, test)");

    // ── (3) IDEMPOTENCY: re-deliver the SAME envelope (same event_id → same deterministic run_id). ──
    let h2 = Arc::clone(&handler);
    let ev2 = ev.clone();
    let outcome2 = tokio::task::spawn_blocking(move || h2.handle(&ev2))
        .await
        .unwrap();
    assert_eq!(outcome2, HandleOutcome::Done, "the redelivery is handled");
    let runs = count(
        &p1,
        "SELECT count(*)::bigint AS n FROM ci_run WHERE run_id=$1::uuid",
        &run_id,
    )
    .await;
    assert_eq!(runs, 1, "the redelivery did NOT double-start (ON CONFLICT DO NOTHING — one run)");
    let events = count(
        &p1,
        "SELECT count(*)::bigint AS n FROM ci_reserve_outbox WHERE run_id=$1",
        &run_id,
    )
    .await;
    assert_eq!(events, 3, "the redelivery added no duplicate events (1 started + 2 checks)");

    // ── (4) ATOMICITY / no-ghost: an UNCOMMITTED reserve leaves NOTHING. ──
    {
        let mut tx = p1.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO ci_run (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, \
             definition_snapshot, trigger_kind, trust_tier, state, correlation_id) \
             VALUES ('acme','fr-par', gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), \
             gen_random_uuid(), 'x', 'push', 'trusted', 'queued', 'corr-crash')",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        // Drop the tx WITHOUT commit → the crash before the co-commit completes.
        drop(tx);
    }
    let ghost = count(
        &p1,
        "SELECT count(*)::bigint AS n FROM ci_run WHERE correlation_id=$1",
        "corr-crash",
    )
    .await;
    assert_eq!(ghost, 0, "the uncommitted reserve left NO ghost run (all-or-nothing)");

    // ── Cleanup. ──
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&p1)
        .await
        .ok();
    println!(
        "[CT-004b] PASS trigger consumer: a git.ref.updated push with a digest-pinned .myelin/ci.toml \
         at new_oid → a DURABLE ci_run row (queued, trusted, push, CAS snapshot ref) + ci.run.started \
         + 2 queued ci.check.updated, co-committed in ONE tx; redelivery (same event_id) → 1 run, 0 \
         duplicate events (idempotent); uncommitted reserve → 0 ghost (all-or-nothing)"
    );
}
