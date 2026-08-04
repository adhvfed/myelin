//! # `cost_store` — CT-004a: the REAL durable `cost_event` projection store (the metering system-of-record)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md`
//! §3.7 (the `cost_event` schema — one row per metered unit; wholesale & markup as SEPARATE integer
//! minor-units columns; `kind ∈ {ci, agent}`) + `02-internals-and-algorithms.md` §8 (the metering
//! algorithm — resource-seconds are the wholesale meter, `cost_events_per_unit == 1`).
//! **Contracts:** 11.7 (reserve/settle — *Storage owns the durable money ledger*).
//!
//! ## What CT-004a ships — the metering PROJECTION store, turning "model-only" into a real store
//!
//! Before CT-004a the CI metering path was **model-only**: [`crate::metering`] shipped the SQL
//! constants ([`INSERT_COST_EVENT_QUERY`] / [`SELECT_COST_EVENTS_FOR_RUN_QUERY`]), the in-memory
//! [`CostEventRow`] model, and the [`crate::metering::CiMeter`] reserve/settle bookend — but NO
//! production-callable store that executes those constants against a real pool. Every prior "durable"
//! metering proof ran the RAW SQL against a per-test temp table, never through a store the composition
//! root could construct. [`CiCostEventStore`] is that store: it holds a [`PgPool`], pins the provider
//! [`Region`], executes the BYTE-IDENTICAL production constants, and is constructed at the root
//! ([`crate::ci_cost_event_store`]).
//!
//! ## The storage-`CostLedger`-vs-CI-projection SPLIT (the anti-duplication decision)
//!
//! There are TWO reserve/settle-adjacent persistence surfaces, and CT-004a owns ONLY the second:
//!   1. **The money-truth ledger — `myelin_storage::reserve_settle::CostLedger`** (contract 11.7,
//!      migration `0050`): the reserve/settle bookkeeping — `cost_reservation` (reserved amount +
//!      lifecycle state, the wallet debit/refund) + Storage's OWN `cost_event` append log (keyed by
//!      `ord` under a reservation). This is the never-double-bill / never-interrupt-in-flight
//!      money-truth. CI does **NOT** re-own it: [`crate::metering::CiMeter::settle_budget`] already
//!      delegates every wallet movement to [`myelin_flow::BudgetGate`] (which wraps that ledger). The
//!      money-parity invariant (reserve/settle bookends don't double-bill) is enforced THERE.
//!   2. **The CI `ci_cost_event` PROJECTION — this store** (CI's own `ci_cost_event` table, migration
//!      `ci_0014`, [`crate::migrations::CREATE_CI_COST_EVENT_DDL`]): the run/job-attributed,
//!      meter-dimensioned REPORTING projection — `(tenant_id, cost_id)`-keyed rows carrying
//!      `(run_id, job_id, meter, amount, wholesale, markup, kind)`. This is what a tenant's CI usage
//!      view / the `ci.check.updated` `cost_settled` flag reads. CI OWNS this table; the controlplane
//!      is its system-of-record. `CiCostEventStore` persists exactly these projection rows.
//!
//! **Why the split (rationale).** The money-truth is cross-run wallet arithmetic that MUST be one
//! ledger for CI + agent runs (UNIFY / X-6) — duplicating it in CI would be a second metering path
//! (the arch §6 hard rule against). The CI projection is a per-subsystem reporting table with an
//! attribution grain (`run_id`/`job_id`) + a meter taxonomy the money ledger does not carry. So
//! `CiCostEventStore` DELEGATES the money to `CostLedger` (via `CiMeter`/`BudgetGate`) and owns only
//! the `cost_event` CI-projection rows a settle produces — no re-implementation of the storage ledger.
//!
//! ## Exactly-once + the co-commit grain
//!
//! [`Self::settle_in_tx`] is the production-primary entry: it records the projection rows on a
//! caller-supplied transaction so the settle **co-commits** with the run-state transition in ONE tx
//! (the spine's one-tx rule — a crash between "stamp run terminal" and "record cost" cannot half-bill).
//! Each row's `cost_id` is DERIVED deterministically from `(tenant, run_id, job_id, meter)` so a
//! re-delivered settle produces the SAME `cost_id`; combined with the constant's
//! `ON CONFLICT (tenant_id, cost_id) DO NOTHING`, a doubly-delivered `job.done` records each metered
//! unit EXACTLY ONCE (double-effect = 0). [`Self::settle`] is the convenience that opens + commits its
//! own tx.
//!
//! ## Fail-loud, no silent fallback
//!
//! Every DB error is a typed [`CiCostStoreError::Db`] (never a swallowed drop). A `run_id`/`job_id`
//! that is not a UUID (the durable column type) is a loud [`CiCostStoreError::BadId`]; a read-back row
//! whose `meter`/`kind` token is outside the frozen set is a loud [`CiCostStoreError::CorruptRow`]
//! (a corrupt durable write, never silently coerced). A minor-units value that does not fit the
//! `bigint` column is a loud [`CiCostStoreError::AmountOverflow`].
//!
//! ## FLOOR / follow-on NAMED (CT-004d)
//! This store is a real, constructible PRODUCTION store, but it is **dormant** at the shell: no live
//! consumer drives it yet. Attaching it to the `SCHEDULE_AND_RUN_JOB` dispatch settle path (so a real
//! `job.done` co-commits its run-state transition + this projection write) is **CT-004d**.
//!
//! **CT-004m RESOLVED the table-name collision** that CT-004a recorded here: Storage's money-ledger
//! `cost_event` (migration `0050`) and CI's projection formerly shared the name `cost_event` in the
//! ONE shared `myelin` DB. CI's table is now `ci_cost_event` ([`crate::migrations::CI_COST_EVENT_TABLE`]),
//! created by [`crate::ci_durable_migrations`] (applied by BOTH CI mains at boot). So this store's
//! [`INSERT_COST_EVENT_QUERY`] targets a table that reliably exists with the right shape. CT-004d's
//! remaining job is to DRIVE a live settle through it. Standalone operations now take a verified
//! [`TenantScope`] and use transaction-local tenant/region GUCs; a `settle_in_tx` caller supplies the
//! same scoped transaction so the FORCE-RLS table never sees raw request authority.

use myelin_flow::MicroUsd;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::metering::{
    CostEventRow, CostKind, Meter, INSERT_COST_EVENT_QUERY, SELECT_COST_EVENTS_FOR_RUN_QUERY,
    SELECT_COST_EVENT_BY_ID_QUERY,
};

/// A durable CI-metering-store failure. Loud + typed — a settle/read NEVER silently drops or coerces
/// (the census theme-#2 silent-data-loss floor). Safe to log: public formatting carries only the
/// structural fault and redacts tenant, region, identifiers, corrupt tokens, and monetary values.
pub enum CiCostStoreError {
    /// A durable-store DB error (the write/read did NOT succeed) — never a silent partial write.
    Db(&'static str),
    /// The verified request scope does not belong to this store's pinned residency cell, is empty,
    /// or disagrees with a row presented for persistence. Carries no authority values by design.
    ScopeMismatch,
    /// A `run_id`/`job_id` in a [`CostEventRow`] is not a UUID (the durable `cost_event.run_id` /
    /// `cost_event.job_id` column type). CI run/job ids ARE uuids in production; a non-uuid token
    /// (e.g. a `test-support` drill's synthetic `"ci/run/0"`) never reaches the durable projection —
    /// this is the loud refusal if one is presented.
    BadId {
        /// Which field failed (`run_id` | `job_id`).
        field: &'static str,
    },
    /// A minor-units / amount value does not fit the `bigint` durable column (a loud refusal, never a
    /// silent wrap — integer minor-units, arch 01 §3.7).
    AmountOverflow {
        /// Which column overflowed (`amount` | `wholesale` | `markup`).
        column: &'static str,
        /// The offending value.
        value: u64,
    },
    /// A read-back row carries a `meter`/`kind` token outside the frozen CHECK-constraint set — a
    /// corrupt durable write, surfaced loudly (never silently coerced).
    CorruptRow(String),
    /// **Boot-time schema-shape mismatch (peer-review #11).** The `ci_cost_event` table the store binds
    /// to does not match the CI metering-projection shape — a required column is absent or the wrong
    /// type (e.g. a table left over from the pre-CT-004m `cost_event` name collision with Storage's
    /// money-ledger). Surfaced LOUDLY at boot so a wrong-shaped MONEY table is never written to.
    SchemaShapeMismatch {
        /// The column that is absent / mis-typed.
        column: String,
        /// The type the CI metering projection requires.
        expected: String,
        /// What the durable table actually has (`<absent>` if the column is missing).
        actual: String,
    },
    /// **Verify-on-conflict divergence (peer-review #13).** A re-delivered settle derived the SAME
    /// `cost_id` (the `(tenant, run_id, job_id, meter)` idempotency key) as an already-recorded unit,
    /// but a MONETARY column differs. `ON CONFLICT DO NOTHING` would silently keep the first amount and
    /// drop this one — a hidden billing-table divergence. Surfaced loudly instead (never a silent drop).
    AmountDivergence {
        /// Which monetary column diverged (`amount` | `wholesale` | `markup`).
        column: &'static str,
        /// The value already recorded (the first settle's).
        recorded: i64,
        /// The incoming re-delivered value that disagrees.
        incoming: i64,
    },
}

impl core::fmt::Display for CiCostStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiCostStoreError::Db(operation) => {
                write!(f, "durable CI cost_event store error during {operation}")
            }
            CiCostStoreError::ScopeMismatch => {
                write!(f, "durable CI cost_event scope rejected")
            }
            CiCostStoreError::BadId { field } => write!(
                f,
                "durable CI cost_event settle refused: {field} is not a UUID (the \
                 cost_event.{field} column is uuid — CI run/job ids are uuids in production)"
            ),
            CiCostStoreError::AmountOverflow { column, .. } => write!(
                f,
                "durable CI cost_event settle refused: {column} value does not fit the \
                 bigint column (integer minor-units, never a silent wrap)"
            ),
            CiCostStoreError::CorruptRow(_) => {
                write!(f, "corrupt durable cost_event row (outside the frozen token set)")
            }
            CiCostStoreError::AmountDivergence { column, .. } => write!(
                f,
                "durable CI cost_event settle refused: a re-delivered unit (same cost_id) disagrees \
                 on {column} (never a silent drop of a divergent bill; the idempotency key is \
                 (tenant, run, job, meter), not the amount)"
            ),
            CiCostStoreError::SchemaShapeMismatch { column, expected, actual } => write!(
                f,
                "ci_cost_event schema-shape assertion FAILED: column `{column}` is `{actual}`, the \
                 CI metering projection requires `{expected}` — refusing to bind the money table to a \
                 wrong-shaped table (pre-CT-004m cost_event collision, or drift)"
            ),
        }
    }
}

impl core::fmt::Debug for CiCostStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Db(operation) => f.debug_tuple("Db").field(operation).finish(),
            Self::ScopeMismatch => f.write_str("ScopeMismatch"),
            Self::BadId { field } => f.debug_struct("BadId").field("field", field).finish(),
            Self::AmountOverflow { column, .. } => f
                .debug_struct("AmountOverflow")
                .field("column", column)
                .field("value", &"<redacted>")
                .finish(),
            Self::CorruptRow(_) => f.debug_tuple("CorruptRow").field(&"<redacted>").finish(),
            Self::SchemaShapeMismatch {
                column,
                expected,
                actual,
            } => f
                .debug_struct("SchemaShapeMismatch")
                .field("column", column)
                .field("expected", expected)
                .field("actual", actual)
                .finish(),
            Self::AmountDivergence { column, .. } => f
                .debug_struct("AmountDivergence")
                .field("column", column)
                .field("recorded", &"<redacted>")
                .field("incoming", &"<redacted>")
                .finish(),
        }
    }
}

impl std::error::Error for CiCostStoreError {}

impl From<myelin_storage::PgError> for CiCostStoreError {
    fn from(_: myelin_storage::PgError) -> Self {
        Self::Db("tenant-scoped transaction")
    }
}

#[derive(Clone, PartialEq, Eq)]
struct VerifiedCostScope {
    tenant_id: TenantId,
    region: Region,
}

impl VerifiedCostScope {
    fn new(scope: &TenantScope, store_region: &Region) -> Result<Self, CiCostStoreError> {
        if scope.tenant().as_str().is_empty()
            || scope.region().0.is_empty()
            || scope.region() != store_region
        {
            return Err(CiCostStoreError::ScopeMismatch);
        }
        Ok(Self {
            tenant_id: scope.tenant().clone(),
            region: scope.region().clone(),
        })
    }

    fn verify_rows(&self, rows: &[CostEventRow]) -> Result<(), CiCostStoreError> {
        if rows.iter().any(|row| row.tenant != self.tenant_id) {
            return Err(CiCostStoreError::ScopeMismatch);
        }
        Ok(())
    }
}

impl core::fmt::Debug for VerifiedCostScope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VerifiedCostScope")
            .field("tenant_id", &"<redacted>")
            .field("region", &"<redacted>")
            .finish()
    }
}

/// **The REAL durable CI `cost_event` projection store (CT-004a).** Holds the OLTP [`PgPool`] and
/// executes the BYTE-IDENTICAL production constants [`INSERT_COST_EVENT_QUERY`] /
/// [`SELECT_COST_EVENTS_FOR_RUN_QUERY`] against it. Cloneable (the pool is an `Arc`-backed handle).
/// Named `…Store` + carries a `PgPool` so the `no-in-memory-durable-store` scanner reads it as a
/// genuine durable store. The caller must have applied the CI durable migrations (via the shell's
/// `serve(AppSpec)` migrate, or [`crate::ci_durable_migrations`] at boot) so the `ci_cost_event`
/// table exists.
#[derive(Clone)]
pub struct CiCostEventStore {
    pool: PgPool,
    region: Region,
}

impl core::fmt::Debug for CiCostEventStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CiCostEventStore")
            .field("pool", &"<redacted>")
            .field("region", &"<redacted>")
            .finish()
    }
}

impl CiCostEventStore {
    /// Wrap the controlplane OLTP pool as the durable CI metering projection store, pinned to the
    /// provider's typed residency region. The production composition-root constructor is
    /// [`crate::ci_cost_event_store`] (from the MR-022 provider pool).
    pub fn with_pg(pool: PgPool, region: Region) -> CiCostEventStore {
        CiCostEventStore { pool, region }
    }

    /// The pool this store is bound to (for a co-commit caller that wants to begin its own tx).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// **Record a settle's CI `cost_event` projection rows on a caller-supplied transaction (the
    /// production-primary path — the one-tx co-commit).** Executes [`INSERT_COST_EVENT_QUERY`] once per
    /// metered [`CostEventRow`] on `conn`, so the caller (CT-004d) can co-commit the projection writes
    /// with the run-state terminal transition in ONE tx — a crash between the two cannot half-bill. Each
    /// row's `cost_id` is [`cost_id_for`] (deterministic on `(tenant, run_id, job_id, meter)`), so a
    /// re-delivered settle records each unit EXACTLY ONCE via the constant's `ON CONFLICT DO NOTHING`.
    /// Returns the number of rows the INSERTs affected (a re-delivery returns `0` — double-effect = 0).
    /// The verified scope supplies both the tenant predicate and residency region and must match the
    /// typed region fixed at store construction.
    pub async fn settle_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        scope: &TenantScope,
        rows: &[CostEventRow],
    ) -> Result<u64, CiCostStoreError> {
        let verified = VerifiedCostScope::new(scope, &self.region)?;
        verified.verify_rows(rows)?;
        let mut affected = 0u64;
        for row in rows {
            let tenant_id = &verified.tenant_id;
            let region = &verified.region;
            let run_uuid = parse_id("run_id", &row.run_id)?;
            let job_uuid = parse_id("job_id", &row.job_id)?;
            let cost_id = cost_id_for(&row.tenant, run_uuid, job_uuid, row.meter);
            let amount = fit_bigint("amount", row.amount)?;
            let wholesale = fit_bigint("wholesale", row.wholesale.0)?;
            let markup = fit_bigint("markup", row.markup.0)?;
            let done = sqlx::query(INSERT_COST_EVENT_QUERY)
                .bind(tenant_id.as_str()) // $1 tenant_id — verified RLS/partition key
                .bind(&region.0) // $2 typed, store-pinned residency region
                .bind(cost_id) // $3 cost_id (deterministic idempotency key)
                .bind(run_uuid) // $4 run_id
                .bind(job_uuid) // $5 job_id
                .bind(row.meter.token()) // $6 meter
                .bind(amount) // $7 amount
                .bind(wholesale) // $8 wholesale_minor_units
                .bind(markup) // $9 markup_minor_units
                .bind(row.kind.token()) // $10 kind
                .execute(&mut *conn)
                .await
                .map_err(|_| CiCostStoreError::Db("record cost event"))?;
            if done.rows_affected() == 0 {
                // VERIFY-ON-CONFLICT (#13): the `ON CONFLICT (tenant_id, cost_id) DO NOTHING` absorbed a
                // re-delivery. Because `cost_id` keys on `(tenant, run, job, meter)` — NOT the amount —
                // read the recorded amounts back and confirm they MATCH the incoming unit. A divergence
                // is a metering anomaly `DO NOTHING` would silently hide; surface it loudly (this keys
                // the billing table). Same-amount re-delivery is the normal idempotent no-op (returns 0).
                let existing = sqlx::query(SELECT_COST_EVENT_BY_ID_QUERY)
                    .bind(tenant_id.as_str()) // $1 verified tenant_id
                    .bind(cost_id) // $2 cost_id
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|_| CiCostStoreError::Db("verify existing cost event"))?;
                for (column, incoming, recorded) in [
                    ("amount", amount, existing.get::<i64, _>("amount")),
                    (
                        "wholesale",
                        wholesale,
                        existing.get::<i64, _>("wholesale_minor_units"),
                    ),
                    (
                        "markup",
                        markup,
                        existing.get::<i64, _>("markup_minor_units"),
                    ),
                ] {
                    if incoming != recorded {
                        return Err(CiCostStoreError::AmountDivergence {
                            column,
                            recorded,
                            incoming,
                        });
                    }
                }
            }
            affected += done.rows_affected();
        }
        Ok(affected)
    }

    /// Verify one job's complete immutable CI projection without inserting or repairing rows.
    ///
    /// Supersession uses this when accepting accounting written by a reporter that won an earlier
    /// race. Missing, extra, or value-divergent projection rows are corruption, not an invitation to
    /// reconstruct monetary truth during cancellation.
    pub(crate) async fn verify_exact_job_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        scope: &TenantScope,
        expected: &[CostEventRow],
    ) -> Result<(), CiCostStoreError> {
        let verified = VerifiedCostScope::new(scope, &self.region)?;
        verified.verify_rows(expected)?;
        let Some(first) = expected.first() else {
            return Err(CiCostStoreError::CorruptRow(
                "empty expected job projection".into(),
            ));
        };
        if expected
            .iter()
            .any(|row| row.run_id != first.run_id || row.job_id != first.job_id)
        {
            return Err(CiCostStoreError::CorruptRow(
                "mixed job projection authority".into(),
            ));
        }
        let run_id = parse_id("run_id", &first.run_id)?;
        let job_id = parse_id("job_id", &first.job_id)?;
        let rows = sqlx::query(
            "SELECT meter, amount, wholesale_minor_units, markup_minor_units, kind \
             FROM ci_cost_event \
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND job_id = $4 \
             ORDER BY meter",
        )
        .bind(verified.tenant_id.as_str())
        .bind(&verified.region.0)
        .bind(run_id)
        .bind(job_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(|_| CiCostStoreError::Db("verify exact job cost projection"))?;
        if rows.len() != expected.len() {
            return Err(CiCostStoreError::CorruptRow(
                "job projection cardinality divergence".into(),
            ));
        }
        let mut expected = expected.to_vec();
        expected.sort_by_key(|row| row.meter.token());
        for (row, expected) in rows.iter().zip(expected.iter()) {
            let meter: String = row.get("meter");
            let kind: String = row.get("kind");
            let amount: i64 = row.get("amount");
            let wholesale: i64 = row.get("wholesale_minor_units");
            let markup: i64 = row.get("markup_minor_units");
            if meter != expected.meter.token()
                || kind != expected.kind.token()
                || u64::try_from(amount).ok() != Some(expected.amount)
                || u64::try_from(wholesale).ok() != Some(expected.wholesale.0)
                || u64::try_from(markup).ok() != Some(expected.markup.0)
            {
                return Err(CiCostStoreError::CorruptRow(
                    "job projection value divergence".into(),
                ));
            }
        }
        Ok(())
    }

    /// **Settle-and-commit convenience.** Opens a transaction, records the projection rows via
    /// [`Self::settle_in_tx`], and commits — the standalone-settle path (no co-commit with run-state).
    /// Returns the rows affected (0 on a full re-delivery). Fail-loud: a mid-settle DB error rolls the
    /// whole tx back (no half-billed run) and returns [`CiCostStoreError::Db`]. The shared scoped-tx
    /// helper sets transaction-local tenant/region GUCs and scrubs them at transaction end.
    pub async fn settle(
        &self,
        scope: &TenantScope,
        rows: &[CostEventRow],
    ) -> Result<u64, CiCostStoreError> {
        let verified = VerifiedCostScope::new(scope, &self.region)?;
        verified.verify_rows(rows)?;
        let tenant_id = verified.tenant_id.0.clone();
        let region = verified.region.0.clone();
        let store = self.clone();
        let scope = scope.clone();
        let rows = rows.to_vec();
        myelin_storage::with_tenant_tx_error(&self.pool, &tenant_id, &region, move |conn| {
            Box::pin(async move { store.settle_in_tx(conn, &scope, &rows).await })
        })
        .await
    }

    /// **Read back every metered unit attributed to a run (the durability/attribution verify side).**
    /// Executes [`SELECT_COST_EVENTS_FOR_RUN_QUERY`] keyed on `(tenant, run_id)` and rebuilds the
    /// persisted [`CostEventRow`]s (the wholesale + markup split intact, in the canonical `(job_id,
    /// meter)` order). A row whose `meter`/`kind` token is outside the frozen set is a loud
    /// [`CiCostStoreError::CorruptRow`]. `run_id` must be a UUID (the durable column type).
    pub async fn cost_events_for_run(
        &self,
        scope: &TenantScope,
        run_id: &str,
    ) -> Result<Vec<CostEventRow>, CiCostStoreError> {
        let verified = VerifiedCostScope::new(scope, &self.region)?;
        let tenant = verified.tenant_id;
        let transaction_tenant = tenant.0.clone();
        let transaction_region = verified.region.0;
        let run_uuid = parse_id("run_id", run_id)?;
        let tenant_id = tenant.clone();
        let sql_rows = myelin_storage::with_tenant_tx_error(
            &self.pool,
            &transaction_tenant,
            &transaction_region,
            |conn| {
                Box::pin(async move {
                    sqlx::query(SELECT_COST_EVENTS_FOR_RUN_QUERY)
                        .bind(tenant_id.as_str()) // $1 verified tenant_id
                        .bind(run_uuid) // $2 run_id
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|_| CiCostStoreError::Db("read cost events"))
                })
            },
        )
        .await?;
        let mut out = Vec::with_capacity(sql_rows.len());
        for r in &sql_rows {
            let job_uuid: Uuid = r.get("job_id");
            let meter_token: String = r.get("meter");
            let kind_token: String = r.get("kind");
            let amount: i64 = r.get("amount");
            let wholesale: i64 = r.get("wholesale_minor_units");
            let markup: i64 = r.get("markup_minor_units");
            let meter = Meter::from_token(&meter_token).ok_or_else(|| {
                CiCostStoreError::CorruptRow(format!("unknown meter token `{meter_token}`"))
            })?;
            let kind = parse_kind(&kind_token)?;
            out.push(CostEventRow {
                tenant: tenant.clone(),
                run_id: run_id.to_string(),
                job_id: job_uuid.to_string(),
                meter,
                amount: u64::try_from(amount).map_err(|_| {
                    CiCostStoreError::CorruptRow(format!("negative amount {amount}"))
                })?,
                wholesale: MicroUsd(u64::try_from(wholesale).map_err(|_| {
                    CiCostStoreError::CorruptRow(format!("negative wholesale {wholesale}"))
                })?),
                markup: MicroUsd(u64::try_from(markup).map_err(|_| {
                    CiCostStoreError::CorruptRow(format!("negative markup {markup}"))
                })?),
                kind,
            });
        }
        Ok(out)
    }
}

/// Parse a `run_id`/`job_id` token into the durable `uuid` column type. A non-uuid is a loud refusal
/// (production CI run/job ids are uuids; a `test-support` drill's synthetic token never reaches here).
fn parse_id(field: &'static str, value: &str) -> Result<Uuid, CiCostStoreError> {
    Uuid::parse_str(value).map_err(|_| CiCostStoreError::BadId { field })
}

/// Widen a `u64` minor-units / amount to the `bigint` (`i64`) durable column, loudly refusing a value
/// that does not fit (never a silent wrap — integer minor-units, arch 01 §3.7).
fn fit_bigint(column: &'static str, value: u64) -> Result<i64, CiCostStoreError> {
    i64::try_from(value).map_err(|_| CiCostStoreError::AmountOverflow { column, value })
}

/// Parse a `cost_event.kind` token to a [`CostKind`] (the read-side of the schema's CHECK constraint —
/// a token outside `{ci, agent}` is a corrupt write, surfaced loudly).
fn parse_kind(token: &str) -> Result<CostKind, CiCostStoreError> {
    match token {
        "ci" => Ok(CostKind::Ci),
        "agent" => Ok(CostKind::Agent),
        other => Err(CiCostStoreError::CorruptRow(format!(
            "unknown kind token `{other}`"
        ))),
    }
}

/// **Boot-time column-shape assertion for `ci_cost_event` (peer-review #11).** In the pre-CT-004m
/// window CI's metering projection and Storage's money-ledger shared the physical name `cost_event`,
/// so a store could bind to a WRONG-shaped table (`CREATE TABLE IF NOT EXISTS` silently no-ops on an
/// existing table). CT-004m renamed CI's table to `ci_cost_event`, but a boot that inherits a stale /
/// mis-shaped table has no repair path. This asserts — at boot, after the migrations apply — that the
/// table the store will bind to has EXACTLY the CI metering-projection columns + types; a mismatch is
/// a LOUD refusal, never a silent write of money data to a wrong-shaped table.
///
/// Resolves the table via `to_regclass` (search-path aware — the SAME table the unqualified store
/// writes) and reads `pg_attribute`, so it is correct under a per-schema search_path (the test harness)
/// and in the shared `myelin` DB alike. A NULL oid (absent table) is itself a loud failure.
pub async fn verify_ci_cost_event_shape(pool: &PgPool) -> Result<(), CiCostStoreError> {
    const EXPECTED: &[(&str, &str)] = &[
        ("tenant_id", "text"),
        ("region", "text"),
        ("cost_id", "uuid"),
        ("run_id", "uuid"),
        ("job_id", "uuid"),
        ("meter", "text"),
        ("amount", "bigint"),
        ("wholesale_minor_units", "bigint"),
        ("markup_minor_units", "bigint"),
        ("kind", "text"),
    ];
    let rows: Vec<(String, String, bool, bool)> = sqlx::query_as(
        "SELECT a.attname AS name, format_type(a.atttypid, a.atttypmod) AS typ, \
                EXISTS (SELECT 1 FROM pg_attribute tenant_id \
                         WHERE tenant_id.attrelid = a.attrelid \
                           AND tenant_id.attname = 'tenant_id' \
                           AND tenant_id.attnum > 0 AND NOT tenant_id.attisdropped) AS has_tenant_id, \
                EXISTS (SELECT 1 FROM pg_attribute region_column \
                         WHERE region_column.attrelid = a.attrelid \
                           AND region_column.attname = 'region' \
                           AND region_column.attnum > 0 AND NOT region_column.attisdropped) AS has_region \
         FROM pg_attribute a \
         WHERE a.attrelid = to_regclass('ci_cost_event') AND a.attnum > 0 AND NOT a.attisdropped",
    )
    .fetch_all(pool)
    .await
    .map_err(|_| CiCostStoreError::Db("verify schema shape"))?;
    if rows.is_empty() {
        return Err(CiCostStoreError::SchemaShapeMismatch {
            column: "<table>".into(),
            expected: "ci_cost_event (10 metering columns)".into(),
            actual: "<absent — to_regclass resolved nothing>".into(),
        });
    }
    for (present, column) in [(rows[0].2, "tenant_id"), (rows[0].3, "region")] {
        if !present {
            return Err(CiCostStoreError::SchemaShapeMismatch {
                column: column.into(),
                expected: "text".into(),
                actual: "<absent>".into(),
            });
        }
    }
    let actual: std::collections::HashMap<String, String> = rows
        .into_iter()
        .map(|(name, typ, _, _)| (name, typ))
        .collect();
    for (col, want) in EXPECTED {
        let got = actual.get(*col).map(String::as_str).unwrap_or("<absent>");
        if got != *want {
            return Err(CiCostStoreError::SchemaShapeMismatch {
                column: (*col).to_string(),
                expected: (*want).to_string(),
                actual: got.to_string(),
            });
        }
    }
    Ok(())
}

/// **The deterministic `cost_event.cost_id` for one metered unit (the idempotency key).** A stable
/// UUID derived from `(tenant, run_id, job_id, meter)` — the natural identity of a metered unit
/// (`cost_events_per_unit == 1`, arch 02 §8). Deterministic so a re-delivered settle derives the SAME
/// `cost_id` and the constant's `ON CONFLICT (tenant_id, cost_id) DO NOTHING` records it EXACTLY ONCE
/// (double-effect = 0). NOT a security primitive — an idempotency key; a documented FNV-1a fill over
/// the composite (the same deterministic-uuid idiom the CT-004 durability test's `uid()` helper uses),
/// so no new hashing dependency is pulled into the default build.
pub fn cost_id_for(tenant: &TenantId, run_id: Uuid, job_id: Uuid, meter: Meter) -> Uuid {
    // Compose the metered-unit identity into one byte string, then fill 16 bytes via two FNV-1a
    // passes (forward + reverse-seeded) — the same construction the durability harness uses for its
    // deterministic ids, so the derivation is stable across processes (a reopened store derives the
    // identical cost_id for the same unit).
    let mut composite = Vec::new();
    composite.extend_from_slice(tenant.as_str().as_bytes());
    composite.push(0);
    composite.extend_from_slice(run_id.as_bytes());
    composite.extend_from_slice(job_id.as_bytes());
    composite.push(0);
    composite.extend_from_slice(meter.token().as_bytes());

    let mut bytes = [0u8; 16];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &composite {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[..8].copy_from_slice(&h.to_be_bytes());
    let mut h2: u64 = h ^ 0x00ff_00ff_00ff_00ff;
    for b in composite.iter().rev() {
        h2 ^= *b as u64;
        h2 = h2.wrapping_mul(0x0000_0100_0000_01b3);
    }
    bytes[8..].copy_from_slice(&h2.to_be_bytes());
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn scope(tenant: &str, region: &str) -> TenantScope {
        let mut principal = Principal::stub(
            PrincipalId("cost-store-subject".into()),
            PrincipalKind::Service,
            TenantId(tenant.into()),
        );
        principal.region = Region(region.into());
        TenantScope::from_verified_token(&principal, principal.region.clone())
    }

    #[test]
    fn verified_cost_scope_retains_types_and_redacts_authority() {
        let request = scope("tenant-secret", "region-secret");
        let verified = VerifiedCostScope::new(&request, &Region("region-secret".into()))
            .expect("matching verified cost scope");
        assert_eq!(verified.tenant_id, TenantId("tenant-secret".into()));
        assert_eq!(verified.region, Region("region-secret".into()));

        let debug = format!("{verified:?}");
        for secret in ["tenant-secret", "region-secret"] {
            assert!(!debug.contains(secret), "scope debug disclosed {secret}");
        }

        let mismatch = VerifiedCostScope::new(&request, &Region("provider-region-secret".into()))
            .expect_err("a cross-region request must fail closed");
        for rendered in [mismatch.to_string(), format!("{mismatch:?}")] {
            for secret in ["tenant-secret", "region-secret", "provider-region-secret"] {
                assert!(!rendered.contains(secret), "scope error disclosed {secret}");
            }
        }
    }

    #[test]
    fn verified_cost_scope_rejects_mixed_tenant_rows_without_disclosure() {
        let request = scope("tenant-secret", "region-secret");
        let verified = VerifiedCostScope::new(&request, &Region("region-secret".into())).unwrap();
        let rows = vec![CostEventRow {
            tenant: TenantId("other-tenant-secret".into()),
            run_id: Uuid::nil().to_string(),
            job_id: Uuid::nil().to_string(),
            meter: Meter::CpuSeconds,
            amount: 1,
            wholesale: MicroUsd(1),
            markup: MicroUsd(0),
            kind: CostKind::Ci,
        }];

        let error = verified
            .verify_rows(&rows)
            .expect_err("a row cannot override verified tenant authority");
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(!rendered.contains("tenant-secret"));
            assert!(!rendered.contains("other-tenant-secret"));
        }
    }

    #[test]
    fn malformed_identifiers_are_redacted() {
        let secret = "run-id-containing-customer-secret";
        let error = parse_id("run_id", secret).expect_err("invalid UUID must be refused");
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));
    }
}
