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
//! root could construct. [`CiCostEventStore`] is that store: it holds a [`PgPool`], executes the
//! BYTE-IDENTICAL production constants, and is constructed at the controlplane composition root
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
//!   2. **The CI `cost_event` PROJECTION — this store** (CI's own `cost_event` table, migration
//!      `ci_0014`, [`crate::migrations::CREATE_COST_EVENT_DDL`]): the run/job-attributed,
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
//! `job.done` co-commits its run-state transition + this projection write) is **CT-004d**. That chunk
//! must ALSO reconcile the `cost_event` table-name collision recorded in [`crate::ci_cost_event_store`]
//! (Storage's money-ledger `cost_event`, migration `0050`, and CI's projection `cost_event`,
//! `ci_0014`, share a name in the single-binary composition) before this store executes against the
//! live controlplane pool — see that fn's docs.

use myelin_flow::MinorUnits;
use myelin_tenancy::TenantId;
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

use crate::metering::{
    CostEventRow, CostKind, Meter, INSERT_COST_EVENT_QUERY, SELECT_COST_EVENTS_FOR_RUN_QUERY,
};

/// A durable CI-metering-store failure. Loud + typed — a settle/read NEVER silently drops or coerces
/// (the census theme-#2 silent-data-loss floor). Safe to log: carries only the structural fault, never
/// tenant PII beyond the opaque tenant/run tokens the CI schema already keys on.
#[derive(Debug)]
pub enum CiCostStoreError {
    /// A durable-store DB error (the write/read did NOT succeed) — never a silent partial write.
    Db(String),
    /// A `run_id`/`job_id` in a [`CostEventRow`] is not a UUID (the durable `cost_event.run_id` /
    /// `cost_event.job_id` column type). CI run/job ids ARE uuids in production; a non-uuid token
    /// (e.g. a `test-support` drill's synthetic `"ci/run/0"`) never reaches the durable projection —
    /// this is the loud refusal if one is presented.
    BadId {
        /// Which field failed (`run_id` | `job_id`).
        field: &'static str,
        /// The offending value (an opaque CI id token — not PII).
        value: String,
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
}

impl core::fmt::Display for CiCostStoreError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CiCostStoreError::Db(e) => write!(f, "durable CI cost_event store error: {e}"),
            CiCostStoreError::BadId { field, value } => write!(
                f,
                "durable CI cost_event settle refused: {field} `{value}` is not a UUID (the \
                 cost_event.{field} column is uuid — CI run/job ids are uuids in production)"
            ),
            CiCostStoreError::AmountOverflow { column, value } => write!(
                f,
                "durable CI cost_event settle refused: {column} value {value} does not fit the \
                 bigint column (integer minor-units, never a silent wrap)"
            ),
            CiCostStoreError::CorruptRow(e) => {
                write!(f, "corrupt durable cost_event row (outside the frozen token set): {e}")
            }
        }
    }
}

impl std::error::Error for CiCostStoreError {}

/// **The REAL durable CI `cost_event` projection store (CT-004a).** Holds the OLTP [`PgPool`] and
/// executes the BYTE-IDENTICAL production constants [`INSERT_COST_EVENT_QUERY`] /
/// [`SELECT_COST_EVENTS_FOR_RUN_QUERY`] against it. Cloneable (the pool is an `Arc`-backed handle).
/// Named `…Store` + carries a `PgPool` so the `no-in-memory-durable-store` scanner reads it as a
/// genuine durable store. The caller must have applied the CI control-plane migrations (via the shell's
/// `serve(AppSpec)` migrate) so the `cost_event` table exists.
#[derive(Clone)]
pub struct CiCostEventStore {
    pool: PgPool,
}

impl CiCostEventStore {
    /// Wrap the controlplane OLTP pool as the durable CI metering projection store. The production
    /// composition-root constructor is [`crate::ci_cost_event_store`] (from the MR-022 provider pool).
    pub fn with_pg(pool: PgPool) -> CiCostEventStore {
        CiCostEventStore { pool }
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
    /// `region` is the residency pin the `cost_event.region` column carries.
    pub async fn settle_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        region: &str,
        rows: &[CostEventRow],
    ) -> Result<u64, CiCostStoreError> {
        let mut affected = 0u64;
        for row in rows {
            let tenant_id = row.tenant.as_str();
            let run_uuid = parse_id("run_id", &row.run_id)?;
            let job_uuid = parse_id("job_id", &row.job_id)?;
            let cost_id = cost_id_for(&row.tenant, run_uuid, job_uuid, row.meter);
            let amount = fit_bigint("amount", row.amount)?;
            let wholesale = fit_bigint("wholesale", row.wholesale.0)?;
            let markup = fit_bigint("markup", row.markup.0)?;
            let done = sqlx::query(INSERT_COST_EVENT_QUERY)
                .bind(tenant_id) // $1 tenant_id — the tenant predicate (RLS/partition key)
                .bind(region) // $2 region
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
                .map_err(|e| CiCostStoreError::Db(e.to_string()))?;
            affected += done.rows_affected();
        }
        Ok(affected)
    }

    /// **Settle-and-commit convenience.** Opens a transaction, records the projection rows via
    /// [`Self::settle_in_tx`], and commits — the standalone-settle path (no co-commit with run-state).
    /// Returns the rows affected (0 on a full re-delivery). Fail-loud: a mid-settle DB error rolls the
    /// whole tx back (no half-billed run) and returns [`CiCostStoreError::Db`].
    pub async fn settle(
        &self,
        region: &str,
        rows: &[CostEventRow],
    ) -> Result<u64, CiCostStoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| CiCostStoreError::Db(e.to_string()))?;
        let affected = self.settle_in_tx(&mut tx, region, rows).await?;
        tx.commit()
            .await
            .map_err(|e| CiCostStoreError::Db(e.to_string()))?;
        Ok(affected)
    }

    /// **Read back every metered unit attributed to a run (the durability/attribution verify side).**
    /// Executes [`SELECT_COST_EVENTS_FOR_RUN_QUERY`] keyed on `(tenant, run_id)` and rebuilds the
    /// persisted [`CostEventRow`]s (the wholesale + markup split intact, in the canonical `(job_id,
    /// meter)` order). A row whose `meter`/`kind` token is outside the frozen set is a loud
    /// [`CiCostStoreError::CorruptRow`]. `run_id` must be a UUID (the durable column type).
    pub async fn cost_events_for_run(
        &self,
        tenant: &TenantId,
        run_id: &str,
    ) -> Result<Vec<CostEventRow>, CiCostStoreError> {
        let tenant_id = tenant.as_str();
        let run_uuid = parse_id("run_id", run_id)?;
        let sql_rows = sqlx::query(SELECT_COST_EVENTS_FOR_RUN_QUERY)
            .bind(tenant_id) // $1 tenant_id — the tenant predicate
            .bind(run_uuid) // $2 run_id
            .fetch_all(&self.pool)
            .await
            .map_err(|e| CiCostStoreError::Db(e.to_string()))?;
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
                wholesale: MinorUnits(u64::try_from(wholesale).map_err(|_| {
                    CiCostStoreError::CorruptRow(format!("negative wholesale {wholesale}"))
                })?),
                markup: MinorUnits(u64::try_from(markup).map_err(|_| {
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
    Uuid::parse_str(value).map_err(|_| CiCostStoreError::BadId {
        field,
        value: value.to_string(),
    })
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
