//! # `check_status_store` — the STORE-BACKED `check_status` projection (GIT-P20 / P-281, M3)
//!
//! **Owning architecture:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md`
//! §6.1 (the `check_status` consumer — apply the monotonic `run_attempt` supersession into the
//! projection table, idempotent on `event_id`, exactly ONE current row per `(commit_oid, context)`).
//! **Contract:** index row **5.9** (the Git↔CI CheckStatus seam — Git is the consumer + gate).
//! **Reconciliation:** X-1 (the bus is at-least-once, so the stale-lower-attempt drop is mandatory).
//! **Drill:** GIT-D10 part (a) — out-of-order/dup `ci.check.updated` → supersession holds the correct
//! current row (exactly 1 current row per key; idempotent on `event_id`).
//!
//! ## What GIT-P20 ships here — the data-layer leg the seam-floor named
//! [`check_status`](crate::check_status) declared the in-memory [`CheckStatusProjection`] SEMANTICS
//! (GIT-P6 / P-232) and wired the LIVE Bus-runtime consumer leg (EB-26 / P-246). It named ONE
//! remaining floor (`check_status.rs` §"what is still a FLOOR", leg 2):
//!
//! > **The store-backed projection** — the real `check_status` table + the migration + the same-tx
//! > `consumer_dedup` write — is the data-layer follow-on.
//!
//! THIS module fills that floor. [`PgCheckStatusProjection`] is the LIVE, Postgres-backed
//! `check_status` projection: it runs the migration, and its [`apply`](PgCheckStatusProjection::apply)
//! executes — in ONE transaction —
//! 1. the **idempotent-on-`event_id`** guard (`INSERT … consumer_dedup … ON CONFLICT DO NOTHING`;
//!    a re-delivered `event_id` is a no-op, the at-least-once → effectively-once anchor, contract 2.5),
//! 2. the **monotonic `run_attempt` supersession** UPSERT (`ON CONFLICT … DO UPDATE … WHERE
//!    EXCLUDED.run_attempt >= check_status.run_attempt`) — a `>=` incoming attempt supersedes; a late
//!    LOWER attempt is dropped IN SQL (the `WHERE` makes the drop atomic, never a read-then-write race),
//!
//! so the projection holds **exactly one current row per `(tenant, commit_oid, context)`** regardless
//! of physical arrival order or re-delivery. The same-tx pairing of the dedup write + the projection
//! write is the silent-data-loss guard (a rolled-back apply rolls back its dedup mark for free — EB-06
//! / `myelin_events::dedup`).
//!
//! ## Why this is `#[cfg(feature = "integration")]`
//! `cargo build --workspace` MUST stay DB-free (the binding data-layer policy). The Postgres driver
//! (`sqlx`) is an OPTIONAL dependency pulled ONLY by `--features integration`; this module compiles
//! ONLY then. The SEMANTICS it implements are the SAME ones [`CheckStatusProjection`] proves in pure
//! Rust (the in-memory model is the contract; this is its byte-faithful store binding). The live
//! green artifact is `tests/integration_git_d10_check_status_projection.rs` against the dev stack.
//!
//! ## Acyclic-by-construction (EI-02 §3 / no-cross-sync-cycle)
//! Git reads its OWN projection — it NEVER synchronously calls CI. This module only ever touches the
//! Git-owned `check_status` table; there is no outbound call to CI anywhere in the consumer/gate path.

#![cfg(feature = "integration")]

use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusRow, GitOid, TrustTier,
};
use crate::merge_gate::{evaluate_merge_gate_row, MergeGateOutcome, MergeGatePolicy, UnmetContext};
use sqlx::postgres::PgPool;
use sqlx::Row;

/// **The forward-only DDL for the store-backed `check_status` projection (GIT-P20 / 5.9).** Keyed
/// `(tenant, commit_oid, context_provider, context_name)` — exactly ONE current row per key
/// (last-writer-wins by monotonic `run_attempt`). Mirrors the contract-surface
/// [`crate::check_status::CHECK_STATUS_PROJECTION_DDL`] (the same column set) with `IF NOT EXISTS` so
/// the migration is idempotent. The companion `consumer_dedup` table is the idempotency ledger
/// (contract 2.5): keyed `(tenant_id, consumer, event_id)` — TENANT-SCOPED (EI-02 §1, the partition
/// key threads everywhere — a dedup row is isolated per tenant, never a cross-tenant key), whose
/// presence means "already applied".
///
/// The `tenant_id` column name is the platform convention (`PgStore`, the RLS policy keys on
/// `tenant_id`); the architecture §6.1 key is `(tenant, repo, commit_oid, context.*)` — `tenant_id`
/// IS that `tenant`. The table name is parameterised at construction (the integration test isolates
/// per-process) so the drill never collides with a sibling run; the shape is identical.
pub fn projection_ddl(table: &str, dedup_table: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {table} (\
            tenant_id         text   NOT NULL,\
            commit_oid        text   NOT NULL,\
            context_provider  text   NOT NULL,\
            context_name      text   NOT NULL,\
            state             text   NOT NULL,\
            run_ref           text   NOT NULL,\
            run_attempt       bigint NOT NULL,\
            trust_tier        text   NOT NULL,\
            details_ref       text   NOT NULL,\
            summary_key       text   NOT NULL,\
            summary_args      jsonb  NOT NULL,\
            cost_settled      boolean NOT NULL,\
            PRIMARY KEY (tenant_id, commit_oid, context_provider, context_name));\
         CREATE TABLE IF NOT EXISTS {dedup_table} (\
            tenant_id text NOT NULL,\
            consumer  text NOT NULL,\
            event_id  text NOT NULL,\
            CONSTRAINT {dedup_table}_pk PRIMARY KEY (tenant_id, consumer, event_id))"
    )
}

/// The outcome of a store-backed [`apply`](PgCheckStatusProjection::apply) — the loud, observable
/// distinction the drill asserts (never a silent drop). It mirrors
/// [`crate::check_status::ApplyOutcome`] but adds the `DuplicateEvent` arm the store guard surfaces
/// (the in-memory model leaves `event_id` dedup to the Bus runtime; the store does it in-tx here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreApplyOutcome {
    /// The fact became (or seeded) the current row — its `run_attempt` is the new high-water mark.
    Superseded,
    /// A late LOWER-attempt fact — dropped IN SQL by the supersession `WHERE`; the row is unchanged.
    DroppedStale,
    /// A re-delivered `event_id` — the `consumer_dedup` guard absorbed it (idempotent no-op).
    DuplicateEvent,
}

/// **The LIVE, Postgres-backed `check_status` projection (GIT-P20).** Holds exactly one current row
/// per `(tenant, commit_oid, context)`, applying the monotonic `run_attempt` supersession + the
/// `event_id` idempotency in ONE transaction. This is the store binding of the in-memory
/// [`crate::check_status::CheckStatusProjection`] (the same semantics, proven against live Postgres by
/// the GIT-D10 part-(a) drill).
pub struct PgCheckStatusProjection {
    pool: PgPool,
    table: String,
    dedup_table: String,
    /// The durable consumer name (the `consumer_dedup` half of the PK) — `git.check_status`.
    consumer: String,
}

impl PgCheckStatusProjection {
    /// Bind a projection to a live pool + table names, running the (idempotent) migration. The
    /// `consumer` name is the `consumer_dedup` PK half (one ledger per logical consumer).
    pub async fn connect(
        pool: PgPool,
        table: &str,
        dedup_table: &str,
        consumer: &str,
    ) -> Result<PgCheckStatusProjection, sqlx::Error> {
        // Run the `CREATE TABLE IF NOT EXISTS` projection DDL under the SAME app-wide advisory-lock
        // discipline the storage `PgMigrator` uses (`myelin_storage::with_migration_lock`), NOT a
        // bare `raw_sql(ddl).execute(&pool)`. Concurrent startup of multiple consumers against the
        // same DB would otherwise race two `CREATE TABLE`s on Postgres's `pg_type_typname_nsp_index`
        // (the same bug `PgStore::migrate` had); serializing on the shared migration lock closes it.
        // We do NOT version-record this DDL (it is a per-table, idempotent `IF NOT EXISTS` projection
        // a consumer re-runs on every startup), so the lock-around-DDL helper is the right tool
        // rather than a recorded `PgMigrator::apply`.
        myelin_storage::with_migration_lock(&pool, &projection_ddl(table, dedup_table))
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("check_status migration: {e}")))?;
        Ok(PgCheckStatusProjection {
            pool,
            table: table.to_string(),
            dedup_table: dedup_table.to_string(),
            consumer: consumer.to_string(),
        })
    }

    /// **Apply one decoded [`CheckStatus`] fact under the X-1 rules, in ONE transaction (§6.1).**
    ///
    /// 1. **Idempotency on `event_id`** (contract 2.5): `INSERT … {dedup_table} … ON CONFLICT DO
    ///    NOTHING`. If the row already existed (the `event_id` was applied), this is a
    ///    [`StoreApplyOutcome::DuplicateEvent`] — the projection write is SKIPPED and the tx commits
    ///    (the message is acked, 0 dup). This is the at-least-once → effectively-once anchor.
    /// 2. **Monotonic supersession** (X-1): the projection UPSERT's `DO UPDATE … WHERE
    ///    EXCLUDED.run_attempt >= {table}.run_attempt` lets a `>=` incoming attempt supersede and
    ///    DROPS a late lower attempt — IN SQL, atomically (no read-then-write race). A fresh key is an
    ///    `INSERT` (the `ON CONFLICT` never fires).
    ///
    /// Returns the loud [`StoreApplyOutcome`]: `Superseded` if the row changed, `DroppedStale` if the
    /// supersession `WHERE` rejected a lower attempt, `DuplicateEvent` if the dedup guard absorbed it.
    /// The dedup write + the projection write share the tx, so a rollback rolls back BOTH (the
    /// silent-data-loss guard).
    pub async fn apply(
        &self,
        event_id: &str,
        fact: &CheckStatus,
    ) -> Result<StoreApplyOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let tenant_id = &fact.tenant.0;

        // 1. The idempotency guard — INSERT the (tenant_id, consumer, event_id) ledger row.
        //    `rows_affected == 0` means the triple already existed → a re-delivery → the projection
        //    write is skipped. The ledger is TENANT-SCOPED (the tenant_id predicate, EI-02 §1).
        let dedup = sqlx::query(&format!(
            "INSERT INTO {} (tenant_id, consumer, event_id) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
            self.dedup_table
        ))
        .bind(tenant_id)
        .bind(&self.consumer)
        .bind(event_id)
        .execute(&mut *tx)
        .await?;
        if dedup.rows_affected() == 0 {
            // Already applied — the effectively-once no-op. Commit (the dedup row stays; nothing else).
            tx.commit().await?;
            return Ok(StoreApplyOutcome::DuplicateEvent);
        }

        // 2. The monotonic supersession UPSERT. The `WHERE EXCLUDED.run_attempt >= …` clause is the
        //    `>=` supersedes / `<` dropped rule IN SQL — a late lower attempt updates 0 rows.
        let provider = match fact.context.provider {
            crate::check_status::CheckProvider::Ci => "ci",
            crate::check_status::CheckProvider::External => "external",
        };
        let state = state_str(fact.state);
        let trust = trust_str(fact.trust_tier);
        let summary_args = serde_json::to_value(&fact.summary.args)
            .expect("BTreeMap<String,String> always serialises to a JSON object");

        let upsert = sqlx::query(&format!(
            "INSERT INTO {table} (tenant_id, commit_oid, context_provider, context_name, state, \
                run_ref, run_attempt, trust_tier, details_ref, summary_key, summary_args, \
                cost_settled) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) \
             ON CONFLICT (tenant_id, commit_oid, context_provider, context_name) DO UPDATE SET \
               state = EXCLUDED.state, run_ref = EXCLUDED.run_ref, run_attempt = EXCLUDED.run_attempt, \
               trust_tier = EXCLUDED.trust_tier, details_ref = EXCLUDED.details_ref, \
               summary_key = EXCLUDED.summary_key, summary_args = EXCLUDED.summary_args, \
               cost_settled = EXCLUDED.cost_settled \
             WHERE EXCLUDED.run_attempt >= {table}.run_attempt",
            table = self.table
        ))
        .bind(tenant_id)
        .bind(&fact.commit_oid.0)
        .bind(provider)
        .bind(&fact.context.name)
        .bind(state)
        .bind(&fact.run.0)
        .bind(i64::from(fact.run_attempt))
        .bind(trust)
        .bind(&fact.details_ref.0)
        .bind(&fact.summary.template_key)
        .bind(&summary_args)
        .bind(fact.cost_settled)
        .execute(&mut *tx)
        .await?;

        let outcome = if upsert.rows_affected() == 0 {
            // The `WHERE EXCLUDED.run_attempt >= …` rejected a late LOWER attempt — dropped in SQL.
            StoreApplyOutcome::DroppedStale
        } else {
            StoreApplyOutcome::Superseded
        };
        tx.commit().await?;
        Ok(outcome)
    }

    /// The current [`CheckStatusRow`] for a `(commit_oid, context)` key, if any — the row the merge
    /// gate reads. Exactly one per key (the PK + the supersession guarantee that).
    pub async fn current(
        &self,
        tenant_id: &str,
        commit_oid: &GitOid,
        provider: &str,
        context_name: &str,
    ) -> Result<Option<CheckStatusRow>, sqlx::Error> {
        let row = sqlx::query(&format!(
            "SELECT tenant_id, commit_oid, context_provider, context_name, state, run_ref, \
                    run_attempt, trust_tier, details_ref, summary_key, summary_args, cost_settled \
             FROM {} WHERE tenant_id = $1 AND commit_oid = $2 AND context_provider = $3 \
                       AND context_name = $4",
            self.table
        ))
        .bind(tenant_id)
        .bind(&commit_oid.0)
        .bind(provider)
        .bind(context_name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(decode_row))
    }

    /// **THE LIVE MERGE GATE (GIT-P21 / §6.2 — the required-set policy over the STORE-BACKED
    /// projection).** Evaluate a [`MergeGatePolicy`] against the live `check_status` table for the PR's
    /// `head_oid`: for EACH required context, fetch its current row from Postgres and classify it via
    /// the SHARED [`evaluate_merge_gate_row`] primitive (the IDENTICAL state/trust logic the in-memory
    /// gate applies — the DB path and the in-memory path can never drift). `endorsed_contexts` is the
    /// set of fork-endorsed contexts (the GIT-P22 `approve_untrusted_ci` input).
    ///
    /// Returns [`MergeGateOutcome::Admitted`] iff every required context has a current `success` row
    /// with an acceptable trust posture, else [`MergeGateOutcome::Blocked`] with the specific unmet
    /// contexts. **0 merges are admitted with a missing/stale/un-endorsed required context** (the
    /// 0-under-gated-merges invariant, proven against the LIVE stack). Git reads its OWN table — it
    /// never synchronously calls CI (acyclic, EI-02 §3); it reads `trust_tier` OFF the row, never
    /// recomputes it.
    pub async fn merge_gate(
        &self,
        tenant_id: &str,
        head_oid: &GitOid,
        policy: &MergeGatePolicy,
        endorsed_contexts: &[CheckContext],
    ) -> Result<MergeGateOutcome, sqlx::Error> {
        let mut unmet: Vec<UnmetContext> = Vec::new();
        for ctx in &policy.required {
            let provider = match ctx.provider {
                crate::check_status::CheckProvider::Ci => "ci",
                crate::check_status::CheckProvider::External => "external",
            };
            // Read Git's OWN projection row for this required (head_oid, context) — never CI.
            let row = self
                .current(tenant_id, head_oid, provider, &ctx.name)
                .await?;
            let endorsed = endorsed_contexts.contains(ctx);
            // The IDENTICAL classify logic as the in-memory gate (no drift between the DB + memory path).
            if let Some(reason) = evaluate_merge_gate_row(row.as_ref(), endorsed) {
                unmet.push(UnmetContext {
                    context: ctx.clone(),
                    reason,
                });
            }
        }
        Ok(if unmet.is_empty() {
            MergeGateOutcome::Admitted
        } else {
            MergeGateOutcome::Blocked { unmet }
        })
    }

    /// The number of current rows for a commit (one per context) — the "exactly 1 current row per key"
    /// signal the GIT-D10 drill asserts.
    pub async fn row_count_for_commit(
        &self,
        tenant_id: &str,
        commit_oid: &GitOid,
    ) -> Result<i64, sqlx::Error> {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {} WHERE tenant_id = $1 AND commit_oid = $2",
            self.table
        ))
        .bind(tenant_id)
        .bind(&commit_oid.0)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    /// Drop the projection + dedup tables — the integration test's teardown (per-process isolation).
    pub async fn drop_tables(&self) -> Result<(), sqlx::Error> {
        sqlx::raw_sql(&format!(
            "DROP TABLE IF EXISTS {}; DROP TABLE IF EXISTS {}",
            self.table, self.dedup_table
        ))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// The closed-set `CheckState` → column string (the projection's `state` column). The inverse of
/// [`parse_state`]; together they are the round-trip the `current` read proves.
fn state_str(state: CheckState) -> &'static str {
    match state {
        CheckState::Queued => "queued",
        CheckState::InProgress => "in_progress",
        CheckState::Success => "success",
        CheckState::Failure => "failure",
        CheckState::Error => "error",
        CheckState::Neutral => "neutral",
        CheckState::Cancelled => "cancelled",
    }
}

/// The column string → `CheckState`. A row written by [`state_str`] always parses back.
fn parse_state(s: &str) -> CheckState {
    match s {
        "queued" => CheckState::Queued,
        "in_progress" => CheckState::InProgress,
        "success" => CheckState::Success,
        "failure" => CheckState::Failure,
        "error" => CheckState::Error,
        "neutral" => CheckState::Neutral,
        "cancelled" => CheckState::Cancelled,
        other => panic!("check_status row has an unknown state {other:?} (corrupt projection)"),
    }
}

/// `TrustTier` → column string.
fn trust_str(tier: TrustTier) -> &'static str {
    match tier {
        TrustTier::Trusted => "trusted",
        TrustTier::UntrustedFork => "untrusted_fork",
    }
}

/// Column string → `TrustTier`.
fn parse_trust(s: &str) -> TrustTier {
    match s {
        "trusted" => TrustTier::Trusted,
        "untrusted_fork" => TrustTier::UntrustedFork,
        other => {
            panic!("check_status row has an unknown trust_tier {other:?} (corrupt projection)")
        }
    }
}

/// Decode a projection table row into the typed [`CheckStatusRow`] the merge gate reads.
fn decode_row(row: sqlx::postgres::PgRow) -> CheckStatusRow {
    use crate::check_status::{CheckContext, CheckProvider, HumanisedRef};
    use myelin_tenancy::{ArtifactRef, TenantId};
    use std::collections::BTreeMap;

    let provider = match row.get::<String, _>("context_provider").as_str() {
        "ci" => CheckProvider::Ci,
        "external" => CheckProvider::External,
        other => panic!("check_status row has an unknown context_provider {other:?}"),
    };
    let summary_args: BTreeMap<String, String> =
        serde_json::from_value(row.get::<serde_json::Value, _>("summary_args"))
            .expect("summary_args is a JSON object of String→String");
    CheckStatusRow {
        tenant: TenantId(row.get::<String, _>("tenant_id")),
        commit_oid: GitOid(row.get::<String, _>("commit_oid")),
        context: CheckContext {
            provider,
            name: row.get::<String, _>("context_name"),
        },
        state: parse_state(&row.get::<String, _>("state")),
        run: ArtifactRef(row.get::<String, _>("run_ref")),
        run_attempt: u32::try_from(row.get::<i64, _>("run_attempt"))
            .expect("run_attempt fits u32 (the fact's counter is u32)"),
        trust_tier: parse_trust(&row.get::<String, _>("trust_tier")),
        details_ref: ArtifactRef(row.get::<String, _>("details_ref")),
        summary: HumanisedRef {
            template_key: row.get::<String, _>("summary_key"),
            args: summary_args,
        },
        cost_settled: row.get::<bool, _>("cost_settled"),
    }
}
