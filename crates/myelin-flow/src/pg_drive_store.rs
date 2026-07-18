//! PostgreSQL persistence boundary for one deterministic workflow drive.
//!
//! A claim, replay load, and commit are intentionally separate operations, but every mutation is
//! fenced by `(tenant_id, region, run_id, lease_owner, lease_epoch, cursor, live lease)`. The epoch
//! prevents a stale drive from becoming valid when a process reuses the same worker name after an
//! expiry. A commit writes journal rows, attempt rows, timer arms, exact signal consumption, staged
//! outbox rows, and the run settlement in one tenant-scoped PostgreSQL transaction.

use crate::engine::run_state;
use crate::wfctx::{WAIT_IDEM_PREFIX, WAIT_KEYREF_PREFIX, WAIT_SIGNAL_NAME_PREFIX};
use myelin_events::OutboxRow;
use myelin_refs::ArtifactRef;
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;
use std::collections::HashSet;

const MAX_LEASE_SECS: i64 = 300;
const MAX_TOKEN_BYTES: usize = 512;
const MAX_FLOW_EVENT_BYTES: usize = 1024 * 1024;
const HISTORY_KINDS: &[&str] = &[
    "wf_started",
    "wf_completed",
    "activity_scheduled",
    "activity_completed",
    "activity_failed",
    "timer_set",
    "timer_fired",
    "signal_waited",
    "signal_received",
    "side_marker",
];
const ATTEMPT_STATES: &[&str] = &["scheduled", "running", "succeeded", "failed", "retrying"];

/// A fail-closed drive-storage error. Invariant errors are separate from database errors so a
/// dispatcher can distinguish retryable infrastructure failure from work that has lost authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveStoreError {
    InvalidInput(String),
    Storage(String),
    LeaseLost,
    CursorConflict { expected: i64, actual: i64 },
    JournalConflict(String),
    AttemptConflict(String),
    SignalConflict(String),
    TimerConflict(String),
    DuplicateDrive(String),
    CorruptState(String),
}

impl std::fmt::Display for DriveStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for DriveStoreError {}

impl From<PgError> for DriveStoreError {
    fn from(value: PgError) -> Self {
        Self::Storage(value.to_string())
    }
}

fn db(operation: &str, error: impl std::fmt::Display) -> DriveStoreError {
    DriveStoreError::Storage(format!("{operation}: {error}"))
}

fn bounded(label: &str, value: &str) -> Result<(), DriveStoreError> {
    if value.trim().is_empty() || value.len() > MAX_TOKEN_BYTES {
        return Err(DriveStoreError::InvalidInput(format!(
            "{label} must be non-empty and at most {MAX_TOKEN_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_ttl(ttl_secs: i64) -> Result<(), DriveStoreError> {
    if !(1..=MAX_LEASE_SECS).contains(&ttl_secs) {
        return Err(DriveStoreError::InvalidInput(format!(
            "lease TTL must be between 1 and {MAX_LEASE_SECS} seconds"
        )));
    }
    Ok(())
}

fn refs_from_json(
    value: serde_json::Value,
    label: &str,
) -> Result<Vec<ArtifactRef>, DriveStoreError> {
    serde_json::from_value(value).map_err(|e| db(&format!("decode {label} ArtifactRefs"), e))
}

/// A claimed, version-pinned workflow run. `lease_epoch` is the fencing token and must travel with
/// every load, renewal, release, and commit.
#[derive(Clone, Debug, PartialEq)]
pub struct DriveLease {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub wf_type: String,
    pub wf_version: i32,
    pub input: Vec<ArtifactRef>,
    pub budget: Option<serde_json::Value>,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub caused_by: Option<String>,
    pub depth: i32,
    pub partition: i16,
    pub cursor: i64,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub lease_expires_unix_ms: i64,
    /// Stable run-creation timestamp used as the detached outbox envelope clock. Replaying the
    /// same deterministic drive therefore derives byte-identical rows for exact absorption.
    pub created_at_rfc3339: String,
}

/// One ordered durable journal row loaded for replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedHistory {
    pub seq: i64,
    pub kind: String,
    pub command_id: String,
    pub result: Option<Vec<ArtifactRef>>,
    pub result_key_ref: Option<String>,
}

/// One exact unconsumed signal candidate. Consumption later names all key dimensions again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSignal {
    pub signal_name: String,
    pub idem_key: String,
    pub payload: Vec<ArtifactRef>,
    pub payload_key_ref: Option<String>,
    pub received_unix_ms: i64,
}

/// The immutable drive input: the pinned run plus its journal in sequence order and its pending
/// signals in receive order.
#[derive(Clone, Debug, PartialEq)]
pub struct DriveSnapshot {
    pub run: DriveLease,
    pub history: Vec<LoadedHistory>,
    pub pending_signals: Vec<PendingSignal>,
}

/// A staged journal write. `consume_signal`, when present, is valid only for `signal_received` and
/// binds the receipt to the exact durable signal row consumed by this commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryWrite {
    pub seq: i64,
    pub kind: String,
    pub command_id: String,
    pub result: Option<Vec<ArtifactRef>>,
    pub result_key_ref: Option<String>,
    pub consume_signal: Option<SignalKey>,
}

/// Exact `wf_signal` key (tenant/region/run are supplied by the lease).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalKey {
    pub signal_name: String,
    pub idem_key: String,
}

/// A durable activity-attempt ledger write for the leased run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityAttemptWrite {
    pub command_id: String,
    pub attempt: i32,
    pub idem_token: String,
    pub state: String,
    pub error: Option<String>,
    pub started_unix_ms: Option<i64>,
    pub ended_unix_ms: Option<i64>,
}

/// A timer armed by the leased run. A workflow drive may only arm a timer for itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerArm {
    pub timer_id: String,
    pub command_id: String,
    pub fire_at_unix_secs: i64,
    pub partition: i16,
}

/// Everything made durable by one deterministic drive transaction.
#[derive(Clone, Debug)]
pub struct DriveCommit {
    pub drive_id: String,
    pub expected_cursor: i64,
    pub next_state: String,
    pub history: Vec<HistoryWrite>,
    pub attempts: Vec<ActivityAttemptWrite>,
    pub timers: Vec<TimerArm>,
    pub timer_disarms: Vec<String>,
    pub outbox: Vec<OutboxRow>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitOutcome {
    Committed,
    AlreadyCommitted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiredTimer {
    pub timer_id: String,
    pub run_id: Option<String>,
    pub command_id: String,
}

/// Production PostgreSQL drive store pinned to one verified tenant and one residency region.
#[derive(Clone)]
pub struct PgFlowDriveStore {
    pool: PgPool,
    tenant: TenantId,
    region: Region,
}

impl PgFlowDriveStore {
    pub fn new(pool: PgPool, tenant: TenantId, region: Region) -> Self {
        Self {
            pool,
            tenant,
            region,
        }
    }

    /// Claim one runnable row with a bounded lease. The candidate row lock is skipped rather than
    /// waited on, allowing many partition workers without convoying.
    pub async fn claim_runnable(
        &self,
        partition: i16,
        owner: &str,
        ttl_secs: i64,
    ) -> Result<Option<DriveLease>, DriveStoreError> {
        bounded("lease owner", owner)?;
        validate_ttl(ttl_secs)?;
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let owner = owner.to_owned();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(
                    "WITH candidate AS (\
                           SELECT run_id FROM workflow_run \
                           WHERE tenant_id = $1 AND region = $2 AND partition = $3 \
                             AND state = 'running' \
                             AND (lease_expires IS NULL OR lease_expires <= clock_timestamp()) \
                           ORDER BY updated_at, run_id \
                           FOR UPDATE SKIP LOCKED LIMIT 1\
                         ) \
                         UPDATE workflow_run AS run \
                         SET lease_owner = $4, \
                             lease_expires = clock_timestamp() + ($5 * INTERVAL '1 second'), \
                             lease_epoch = run.lease_epoch + 1, updated_at = clock_timestamp() \
                         FROM candidate \
                         WHERE run.tenant_id = $1 AND run.region = $2 \
                           AND run.run_id = candidate.run_id \
                         RETURNING run.run_id, run.wf_type, run.wf_version, run.input, run.budget, \
                           run.correlation_id, run.causation_id, run.caused_by, run.depth, \
                           run.partition, run.cursor, run.lease_owner, run.lease_epoch, \
                           (EXTRACT(EPOCH FROM run.lease_expires) * 1000)::bigint AS lease_ms, \
                           to_char(run.created_at AT TIME ZONE 'UTC', \
                             'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_rfc3339",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(partition)
                .bind(&owner)
                .bind(ttl_secs)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| db("claim runnable workflow", e))?;
                row.map(|row| decode_lease(row, &tenant, &region))
                    .transpose()
            })
        })
        .await
    }

    /// Claim one runnable row for an exact locally registered definition. Production dispatchers
    /// use this form so a worker never leases a workflow type/version whose body is absent from the
    /// process. Unsupported definitions remain untouched for their owning adapter.
    pub async fn claim_runnable_definition(
        &self,
        partition: i16,
        wf_type: &str,
        wf_version: i32,
        owner: &str,
        ttl_secs: i64,
    ) -> Result<Option<DriveLease>, DriveStoreError> {
        bounded("workflow type", wf_type)?;
        bounded("lease owner", owner)?;
        validate_ttl(ttl_secs)?;
        if wf_version <= 0 {
            return Err(DriveStoreError::InvalidInput(
                "workflow version must be positive".into(),
            ));
        }
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let owner = owner.to_owned();
        let wf_type = wf_type.to_owned();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query(
                    "WITH candidate AS (\
                       SELECT run_id FROM workflow_run \
                       WHERE tenant_id = $1 AND region = $2 AND partition = $3 \
                         AND wf_type = $6 AND wf_version = $7 AND state = 'running' \
                         AND (lease_expires IS NULL OR lease_expires <= clock_timestamp()) \
                       ORDER BY updated_at, run_id FOR UPDATE SKIP LOCKED LIMIT 1\
                     ) \
                     UPDATE workflow_run AS run SET lease_owner = $4, \
                       lease_expires = clock_timestamp() + ($5 * INTERVAL '1 second'), \
                       lease_epoch = run.lease_epoch + 1, updated_at = clock_timestamp() \
                     FROM candidate WHERE run.tenant_id = $1 AND run.region = $2 \
                       AND run.run_id = candidate.run_id \
                     RETURNING run.run_id, run.wf_type, run.wf_version, run.input, run.budget, \
                       run.correlation_id, run.causation_id, run.caused_by, run.depth, \
                       run.partition, run.cursor, run.lease_owner, run.lease_epoch, \
                       (EXTRACT(EPOCH FROM run.lease_expires) * 1000)::bigint AS lease_ms, \
                       to_char(run.created_at AT TIME ZONE 'UTC', \
                         'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_rfc3339",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(partition)
                .bind(&owner)
                .bind(ttl_secs)
                .bind(&wf_type)
                .bind(wf_version)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| db("claim runnable workflow definition", e))?;
                row.map(|row| decode_lease(row, &tenant, &region))
                    .transpose()
            })
        })
        .await
    }

    /// Renew only the exact still-live owner+epoch claim. An expired lease is never resurrected.
    pub async fn renew_lease(
        &self,
        lease: &DriveLease,
        ttl_secs: i64,
    ) -> Result<i64, DriveStoreError> {
        self.validate_lease_scope(lease)?;
        validate_ttl(ttl_secs)?;
        let lease = lease.clone();
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                let row = sqlx::query_scalar::<_, i64>(
                    "UPDATE workflow_run SET \
                       lease_expires = clock_timestamp() + ($7 * INTERVAL '1 second'), \
                       updated_at = clock_timestamp() \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND state = 'running' \
                       AND lease_owner = $4 AND lease_epoch = $5 AND cursor = $6 \
                       AND lease_expires > clock_timestamp() \
                     RETURNING (EXTRACT(EPOCH FROM lease_expires) * 1000)::bigint",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&lease.run_id)
                .bind(&lease.lease_owner)
                .bind(lease.lease_epoch)
                .bind(lease.cursor)
                .bind(ttl_secs)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| db("renew workflow lease", e))?;
                row.ok_or(DriveStoreError::LeaseLost)
            })
        })
        .await
    }

    /// Release only the exact owner+epoch claim. A stale process cannot clear a successor's lease.
    pub async fn release_lease(&self, lease: &DriveLease) -> Result<(), DriveStoreError> {
        self.validate_lease_scope(lease)?;
        let lease = lease.clone();
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                let changed = sqlx::query(
                    "UPDATE workflow_run SET lease_owner = NULL, lease_expires = NULL, \
                       updated_at = clock_timestamp() \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                       AND lease_owner = $4 AND lease_epoch = $5 AND cursor = $6",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&lease.run_id)
                .bind(&lease.lease_owner)
                .bind(lease.lease_epoch)
                .bind(lease.cursor)
                .execute(&mut *conn)
                .await
                .map_err(|e| db("release workflow lease", e))?
                .rows_affected();
                if changed == 1 {
                    Ok(())
                } else {
                    Err(DriveStoreError::LeaseLost)
                }
            })
        })
        .await
    }

    /// Load the pinned run, ordered journal, and pending signals under the exact live lease fence.
    pub async fn load_drive(&self, lease: &DriveLease) -> Result<DriveSnapshot, DriveStoreError> {
        self.validate_lease_scope(lease)?;
        let lease = lease.clone();
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(
            &self.pool,
            &scope_tenant,
            &scope_region,
            move |conn| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT run_id, wf_type, wf_version, input, budget, correlation_id, \
                           causation_id, caused_by, depth, partition, cursor, lease_owner, \
                           lease_epoch, (EXTRACT(EPOCH FROM lease_expires) * 1000)::bigint AS lease_ms, \
                           to_char(created_at AT TIME ZONE 'UTC', \
                             'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_rfc3339 \
                         FROM workflow_run \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND state = 'running' \
                           AND lease_owner = $4 AND lease_epoch = $5 AND cursor = $6 \
                           AND lease_expires > clock_timestamp()",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .bind(&lease.lease_owner)
                    .bind(lease.lease_epoch)
                    .bind(lease.cursor)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| db("load leased workflow", e))?
                    .ok_or(DriveStoreError::LeaseLost)?;
                    let run = decode_lease(row, &tenant, &region)?;

                    let rows = sqlx::query(
                        "SELECT seq, kind, command_id, result, result_key_ref \
                         FROM wf_history \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                         ORDER BY seq, command_id",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| db("load ordered workflow history", e))?;
                    let history = rows
                        .into_iter()
                        .map(decode_history)
                        .collect::<Result<Vec<_>, _>>()?;
                    if history.len() as i64 != run.cursor {
                        return Err(DriveStoreError::CorruptState(format!(
                            "workflow cursor {} does not match {} journal rows",
                            run.cursor,
                            history.len()
                        )));
                    }

                    let rows = sqlx::query(
                        "SELECT signal_name, idem_key, payload, payload_key_ref, \
                           (EXTRACT(EPOCH FROM received_at) * 1000)::bigint AS received_ms \
                         FROM wf_signal \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                           AND consumed_seq IS NULL \
                         ORDER BY received_at, signal_name, idem_key",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|e| db("load pending workflow signals", e))?;
                    let pending_signals = rows
                        .into_iter()
                        .map(decode_signal)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(DriveSnapshot {
                        run,
                        history,
                        pending_signals,
                    })
                })
            },
        )
        .await
    }

    /// Atomically persist one drive and release its lease. Exact deterministic re-entry after a
    /// successful commit returns `AlreadyCommitted`; every stale/different re-entry fails closed.
    pub async fn commit_drive(
        &self,
        lease: &DriveLease,
        commit: DriveCommit,
    ) -> Result<CommitOutcome, DriveStoreError> {
        self.validate_lease_scope(lease)?;
        validate_commit(lease, &commit)?;
        let fingerprint = drive_fingerprint(&commit)?;
        let lease = lease.clone();
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(
            &self.pool,
            &scope_tenant,
            &scope_region,
            move |conn| {
                Box::pin(async move {
                    let run = sqlx::query(
                        "SELECT state, cursor, lease_owner, lease_epoch, \
                           lease_expires > clock_timestamp() AS lease_live, \
                           last_drive_id, last_drive_fingerprint \
                         FROM workflow_run \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| db("lock workflow for drive commit", e))?
                    .ok_or(DriveStoreError::LeaseLost)?;

                    let last_id: Option<String> = run.try_get("last_drive_id").map_err(|e| db("decode last drive id", e))?;
                    let last_fingerprint: Option<String> = run.try_get("last_drive_fingerprint").map_err(|e| db("decode last drive fingerprint", e))?;
                    if last_id.as_deref() == Some(commit.drive_id.as_str()) {
                        return if last_fingerprint.as_deref() == Some(fingerprint.as_str()) {
                            Ok(CommitOutcome::AlreadyCommitted)
                        } else {
                            Err(DriveStoreError::DuplicateDrive(commit.drive_id.clone()))
                        };
                    }

                    let actual_cursor: i64 = run.try_get("cursor").map_err(|e| db("decode workflow cursor", e))?;
                    if actual_cursor != commit.expected_cursor {
                        return Err(DriveStoreError::CursorConflict {
                            expected: commit.expected_cursor,
                            actual: actual_cursor,
                        });
                    }
                    let owner: Option<String> = run.try_get("lease_owner").map_err(|e| db("decode lease owner", e))?;
                    let epoch: i64 = run.try_get("lease_epoch").map_err(|e| db("decode lease epoch", e))?;
                    let live: Option<bool> = run.try_get("lease_live").map_err(|e| db("decode lease liveness", e))?;
                    let state: String = run.try_get("state").map_err(|e| db("decode workflow state", e))?;
                    if state != run_state::RUNNING
                        || owner.as_deref() != Some(lease.lease_owner.as_str())
                        || epoch != lease.lease_epoch
                        || live != Some(true)
                    {
                        return Err(DriveStoreError::LeaseLost);
                    }

                    let history_count: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM wf_history \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| db("verify workflow cursor", e))?;
                    if history_count != actual_cursor {
                        return Err(DriveStoreError::CorruptState(format!(
                            "workflow cursor {actual_cursor} does not match {history_count} journal rows"
                        )));
                    }
                    let mut max_seq: i64 = sqlx::query_scalar(
                        "SELECT COALESCE(MAX(seq), -1) FROM wf_history \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| db("load workflow journal sequence", e))?;

                    let mut new_history = 0i64;
                    let mut consumed = HashSet::new();
                    for write in &commit.history {
                        let existing = sqlx::query(
                            "SELECT seq, kind, result, result_key_ref FROM wf_history \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                               AND command_id = $4 FOR UPDATE",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&lease.run_id)
                        .bind(&write.command_id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| db("check idempotent journal write", e))?;

                        let actual_seq = if let Some(existing) = existing {
                            let seq: i64 = existing.try_get("seq").map_err(|e| db("decode journal seq", e))?;
                            let kind: String = existing.try_get("kind").map_err(|e| db("decode journal kind", e))?;
                            let result: Option<serde_json::Value> = existing.try_get("result").map_err(|e| db("decode journal result", e))?;
                            let key_ref: Option<String> = existing.try_get("result_key_ref").map_err(|e| db("decode journal key ref", e))?;
                            let wanted_result = refs_json(&write.result)?;
                            if kind == write.kind
                                && seq == write.seq
                                && result == wanted_result
                                && key_ref == write.result_key_ref
                            {
                                seq
                            } else if kind == "signal_waited"
                                && write.kind == "signal_received"
                                && seq == write.seq
                            {
                                sqlx::query(
                                    "UPDATE wf_history SET kind = 'signal_received', result = $5, result_key_ref = $6 \
                                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                                       AND command_id = $4 AND kind = 'signal_waited'",
                                )
                                .bind(&tenant)
                                .bind(&region)
                                .bind(&lease.run_id)
                                .bind(&write.command_id)
                                .bind(&wanted_result)
                                .bind(&write.result_key_ref)
                                .execute(&mut *conn)
                                .await
                                .map_err(|e| db("upgrade signal wait journal", e))?;
                                seq
                            } else {
                                return Err(DriveStoreError::JournalConflict(write.command_id.clone()));
                            }
                        } else {
                            if write.seq != max_seq + 1 {
                                return Err(DriveStoreError::JournalConflict(format!(
                                    "{} has seq {}, expected the contiguous next seq {}",
                                    write.command_id,
                                    write.seq,
                                    max_seq + 1
                                )));
                            }
                            let result = refs_json(&write.result)?;
                            sqlx::query(
                                "INSERT INTO wf_history \
                                   (tenant_id, region, run_id, seq, kind, command_id, result, result_key_ref) \
                                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                            )
                            .bind(&tenant)
                            .bind(&region)
                            .bind(&lease.run_id)
                            .bind(write.seq)
                            .bind(&write.kind)
                            .bind(&write.command_id)
                            .bind(result)
                            .bind(&write.result_key_ref)
                            .execute(&mut *conn)
                            .await
                            .map_err(|e| db("insert workflow history", e))?;
                            max_seq = write.seq;
                            new_history += 1;
                            write.seq
                        };

                        if let Some(signal) = &write.consume_signal {
                            let key = (signal.signal_name.clone(), signal.idem_key.clone());
                            if !consumed.insert(key) {
                                return Err(DriveStoreError::SignalConflict(
                                    "the same signal row was consumed twice in one drive".into(),
                                ));
                            }
                            consume_exact_signal(
                                conn,
                                &tenant,
                                &region,
                                &lease.run_id,
                                signal,
                                actual_seq,
                                write,
                            )
                            .await?;
                        }
                    }

                    persist_attempts(conn, &tenant, &region, &lease.run_id, &commit.attempts).await?;
                    persist_timer_arms(conn, &tenant, &region, &lease, &commit.timers).await?;
                    persist_timer_disarms(conn, &tenant, &region, &lease, &commit.timer_disarms).await?;
                    PgRelay::co_commit_rows_in_tx(conn, &commit.outbox)
                        .await
                        .map_err(DriveStoreError::from)?;

                    let next_cursor = actual_cursor + new_history;
                    let settled = sqlx::query(
                        "UPDATE workflow_run SET cursor = $7, state = $8, lease_owner = NULL, \
                           lease_expires = NULL, last_drive_id = $9, last_drive_fingerprint = $10, \
                           updated_at = clock_timestamp() \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                           AND lease_owner = $4 AND lease_epoch = $5 AND cursor = $6 \
                           AND lease_expires > clock_timestamp()",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&lease.run_id)
                    .bind(&lease.lease_owner)
                    .bind(lease.lease_epoch)
                    .bind(actual_cursor)
                    .bind(next_cursor)
                    .bind(&commit.next_state)
                    .bind(&commit.drive_id)
                    .bind(&fingerprint)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| db("settle workflow drive", e))?
                    .rows_affected();
                    if settled != 1 {
                        return Err(DriveStoreError::LeaseLost);
                    }
                    Ok(CommitOutcome::Committed)
                })
            },
        )
        .await
    }

    /// Claim and fire one due timer atomically. The fired pivot, deterministic fire journal row,
    /// cursor advance, and waiting→running wake commit together, so a restart observes all or none.
    pub async fn fire_due_timer(
        &self,
        partition: i16,
        now_unix_secs: i64,
    ) -> Result<Option<FiredTimer>, DriveStoreError> {
        let tenant = self.tenant.0.clone();
        let region = self.region.0.clone();
        let scope_tenant = tenant.clone();
        let scope_region = region.clone();
        with_tenant_tx_error(
            &self.pool,
            &scope_tenant,
            &scope_region,
            move |conn| {
                Box::pin(async move {
                    let timer = sqlx::query(
                        "SELECT timer.timer_id, timer.run_id, timer.command_id FROM wf_timer AS timer \
                         WHERE timer.tenant_id = $1 AND timer.region = $2 AND timer.partition = $3 \
                           AND NOT timer.fired AND timer.fire_at <= to_timestamp($4) \
                           AND (timer.run_id IS NULL OR NOT EXISTS (\
                             SELECT 1 FROM workflow_run AS run \
                             WHERE run.tenant_id = $1 AND run.region = $2 \
                               AND run.run_id = timer.run_id AND run.lease_owner IS NOT NULL \
                               AND run.lease_expires > clock_timestamp()\
                           )) \
                         ORDER BY timer.bucket, timer.fire_at, timer.timer_id \
                         FOR UPDATE OF timer SKIP LOCKED LIMIT 1",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(partition)
                    .bind(now_unix_secs as f64)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| db("claim due workflow timer", e))?;
                    let Some(timer) = timer else { return Ok(None) };
                    let timer_id: String = timer.try_get("timer_id").map_err(|e| db("decode timer id", e))?;
                    let run_id: Option<String> = timer.try_get("run_id").map_err(|e| db("decode timer run", e))?;
                    let command_id: String = timer.try_get("command_id").map_err(|e| db("decode timer command", e))?;

                    let mut cursor_increment = 0i64;
                    if let Some(run_id) = &run_id {
                        let run = sqlx::query(
                            "SELECT state, cursor, lease_owner, \
                               lease_expires > clock_timestamp() AS lease_live \
                             FROM workflow_run \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(run_id)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| db("lock timer workflow", e))?
                        .ok_or_else(|| DriveStoreError::TimerConflict(format!("timer {timer_id} has no run")))?;
                        let state: String = run
                            .try_get("state")
                            .map_err(|e| db("decode timer run state", e))?;
                        let lease_owner: Option<String> = run
                            .try_get("lease_owner")
                            .map_err(|e| db("decode timer run lease owner", e))?;
                        let lease_live: Option<bool> = run
                            .try_get("lease_live")
                            .map_err(|e| db("decode timer run lease liveness", e))?;
                        if lease_owner.is_some() && lease_live == Some(true) {
                            return Err(DriveStoreError::TimerConflict(format!(
                                "timer {timer_id} cannot mutate an actively leased run"
                            )));
                        }
                        if state != run_state::WAITING {
                            // A signal may already have woken a running run, or cancellation/
                            // completion may have made it terminal. In either case the timer is
                            // obsolete: disarm it without touching history, cursor, or run state.
                            let changed = sqlx::query(
                                "UPDATE wf_timer SET fired = true \
                                 WHERE tenant_id = $1 AND region = $2 AND timer_id = $3 AND NOT fired",
                            )
                            .bind(&tenant)
                            .bind(&region)
                            .bind(&timer_id)
                            .execute(&mut *conn)
                            .await
                            .map_err(|e| db("disarm obsolete workflow timer", e))?
                            .rows_affected();
                            if changed != 1 {
                                return Err(DriveStoreError::TimerConflict(timer_id));
                            }
                            return Ok(Some(FiredTimer {
                                timer_id,
                                run_id: Some(run_id.clone()),
                                command_id,
                            }));
                        }
                        let cursor: i64 = run
                            .try_get("cursor")
                            .map_err(|e| db("decode timer run cursor", e))?;
                        let history_count: i64 = sqlx::query_scalar(
                            "SELECT count(*) FROM wf_history \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(run_id)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(|e| db("verify timer run cursor", e))?;
                        if cursor != history_count {
                            return Err(DriveStoreError::CorruptState(format!(
                                "timer run cursor {cursor} does not match {history_count} journal rows"
                            )));
                        }
                        let fire_command = format!("{command_id}/fired");
                        let existing: Option<String> = sqlx::query_scalar(
                            "SELECT kind FROM wf_history \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND command_id = $4",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(run_id)
                        .bind(&fire_command)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|e| db("check timer fire journal", e))?;
                        match existing.as_deref() {
                            None => {
                                let seq: i64 = sqlx::query_scalar(
                                    "SELECT COALESCE(MAX(seq), -1) + 1 FROM wf_history \
                                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                                )
                                .bind(&tenant)
                                .bind(&region)
                                .bind(run_id)
                                .fetch_one(&mut *conn)
                                .await
                                .map_err(|e| db("allocate timer fire journal seq", e))?;
                                sqlx::query(
                                    "INSERT INTO wf_history \
                                       (tenant_id, region, run_id, seq, kind, command_id) \
                                     VALUES ($1, $2, $3, $4, 'timer_fired', $5)",
                                )
                                .bind(&tenant)
                                .bind(&region)
                                .bind(run_id)
                                .bind(seq)
                                .bind(&fire_command)
                                .execute(&mut *conn)
                                .await
                                .map_err(|e| db("journal timer fire", e))?;
                                cursor_increment = 1;
                            }
                            Some("timer_fired") => {}
                            Some(_) => return Err(DriveStoreError::TimerConflict(fire_command)),
                        }
                        sqlx::query(
                            "UPDATE workflow_run SET state = CASE WHEN state = 'waiting' THEN 'running' ELSE state END, \
                               cursor = cursor + $4, updated_at = clock_timestamp() \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(run_id)
                        .bind(cursor_increment)
                        .execute(&mut *conn)
                        .await
                        .map_err(|e| db("wake timer workflow", e))?;
                    }

                    let changed = sqlx::query(
                        "UPDATE wf_timer SET fired = true \
                         WHERE tenant_id = $1 AND region = $2 AND timer_id = $3 AND NOT fired",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&timer_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| db("fire workflow timer", e))?
                    .rows_affected();
                    if changed != 1 {
                        return Err(DriveStoreError::TimerConflict(timer_id));
                    }
                    Ok(Some(FiredTimer { timer_id, run_id, command_id }))
                })
            },
        )
        .await
    }

    fn validate_lease_scope(&self, lease: &DriveLease) -> Result<(), DriveStoreError> {
        if lease.tenant != self.tenant || lease.region != self.region {
            return Err(DriveStoreError::LeaseLost);
        }
        Ok(())
    }
}

fn decode_lease(row: PgRow, tenant: &str, region: &str) -> Result<DriveLease, DriveStoreError> {
    let input = refs_from_json(
        row.try_get("input")
            .map_err(|e| db("decode workflow input", e))?,
        "workflow input",
    )?;
    Ok(DriveLease {
        tenant: TenantId(tenant.to_owned()),
        region: Region(region.to_owned()),
        run_id: row.try_get("run_id").map_err(|e| db("decode run id", e))?,
        wf_type: row
            .try_get("wf_type")
            .map_err(|e| db("decode workflow type", e))?,
        wf_version: row
            .try_get("wf_version")
            .map_err(|e| db("decode workflow version", e))?,
        input,
        budget: row
            .try_get("budget")
            .map_err(|e| db("decode workflow budget", e))?,
        correlation_id: row
            .try_get("correlation_id")
            .map_err(|e| db("decode correlation id", e))?,
        causation_id: row
            .try_get("causation_id")
            .map_err(|e| db("decode causation id", e))?,
        caused_by: row
            .try_get("caused_by")
            .map_err(|e| db("decode caused by", e))?,
        depth: row.try_get("depth").map_err(|e| db("decode depth", e))?,
        partition: row
            .try_get("partition")
            .map_err(|e| db("decode partition", e))?,
        cursor: row.try_get("cursor").map_err(|e| db("decode cursor", e))?,
        lease_owner: row
            .try_get("lease_owner")
            .map_err(|e| db("decode lease owner", e))?,
        lease_epoch: row
            .try_get("lease_epoch")
            .map_err(|e| db("decode lease epoch", e))?,
        lease_expires_unix_ms: row
            .try_get("lease_ms")
            .map_err(|e| db("decode lease expiry", e))?,
        created_at_rfc3339: row
            .try_get("created_rfc3339")
            .map_err(|e| db("decode run creation time", e))?,
    })
}

fn decode_history(row: PgRow) -> Result<LoadedHistory, DriveStoreError> {
    let result: Option<serde_json::Value> = row
        .try_get("result")
        .map_err(|e| db("decode history result", e))?;
    Ok(LoadedHistory {
        seq: row
            .try_get("seq")
            .map_err(|e| db("decode history seq", e))?,
        kind: row
            .try_get("kind")
            .map_err(|e| db("decode history kind", e))?,
        command_id: row
            .try_get("command_id")
            .map_err(|e| db("decode history command", e))?,
        result: result
            .map(|v| refs_from_json(v, "history result"))
            .transpose()?,
        result_key_ref: row
            .try_get("result_key_ref")
            .map_err(|e| db("decode history key ref", e))?,
    })
}

fn decode_signal(row: PgRow) -> Result<PendingSignal, DriveStoreError> {
    Ok(PendingSignal {
        signal_name: row
            .try_get("signal_name")
            .map_err(|e| db("decode signal name", e))?,
        idem_key: row
            .try_get("idem_key")
            .map_err(|e| db("decode signal idem key", e))?,
        payload: refs_from_json(
            row.try_get("payload")
                .map_err(|e| db("decode signal payload", e))?,
            "signal payload",
        )?,
        payload_key_ref: row
            .try_get("payload_key_ref")
            .map_err(|e| db("decode signal key ref", e))?,
        received_unix_ms: row
            .try_get("received_ms")
            .map_err(|e| db("decode signal receive time", e))?,
    })
}

fn validate_commit(lease: &DriveLease, commit: &DriveCommit) -> Result<(), DriveStoreError> {
    bounded("drive id", &commit.drive_id)?;
    if commit.expected_cursor != lease.cursor {
        return Err(DriveStoreError::CursorConflict {
            expected: lease.cursor,
            actual: commit.expected_cursor,
        });
    }
    if !matches!(
        commit.next_state.as_str(),
        run_state::RUNNING
            | run_state::WAITING
            | run_state::COMPLETED
            | run_state::FAILED
            | run_state::NONDETERMINISTIC
    ) {
        return Err(DriveStoreError::InvalidInput(format!(
            "invalid drive settlement state {}",
            commit.next_state
        )));
    }
    let mut commands = HashSet::new();
    for write in &commit.history {
        bounded("history command id", &write.command_id)?;
        if !HISTORY_KINDS.contains(&write.kind.as_str()) {
            return Err(DriveStoreError::InvalidInput(format!(
                "invalid history kind {}",
                write.kind
            )));
        }
        if !commands.insert(write.command_id.as_str()) {
            return Err(DriveStoreError::InvalidInput(format!(
                "duplicate history command {}",
                write.command_id
            )));
        }
        if write.consume_signal.is_some() && write.kind != "signal_received" {
            return Err(DriveStoreError::InvalidInput(
                "only signal_received may consume a signal row".into(),
            ));
        }
        if let Some(signal) = &write.consume_signal {
            bounded("signal name", &signal.signal_name)?;
            bounded("signal idem key", &signal.idem_key)?;
        }
    }
    for attempt in &commit.attempts {
        bounded("attempt command id", &attempt.command_id)?;
        bounded("attempt idem token", &attempt.idem_token)?;
        if attempt.attempt <= 0 || !ATTEMPT_STATES.contains(&attempt.state.as_str()) {
            return Err(DriveStoreError::InvalidInput(
                "invalid activity attempt".into(),
            ));
        }
    }
    for timer in &commit.timers {
        bounded("timer id", &timer.timer_id)?;
        bounded("timer command id", &timer.command_id)?;
        if timer.partition != lease.partition {
            return Err(DriveStoreError::InvalidInput(
                "timer partition does not match leased run".into(),
            ));
        }
    }
    if run_state::is_terminal(&commit.next_state) && !commit.timers.is_empty() {
        return Err(DriveStoreError::InvalidInput(
            "a terminal drive cannot arm new workflow timers".into(),
        ));
    }
    let mut event_ids = HashSet::new();
    for row in &commit.outbox {
        bounded("outbox event id", &row.event_id.0)?;
        if row.event_id != row.envelope.event_id
            || row.aggregate != row.envelope.aggregate
            || row.subject != row.envelope.subject
            || row.envelope.tenant != lease.tenant
            || row.envelope.region != lease.region
            || row.seq != 0
            || row.published_at.is_some()
            || row.attempts != 0
        {
            return Err(DriveStoreError::InvalidInput(format!(
                "outbox row {} is not an unpublished canonical event in the lease scope",
                row.event_id.0
            )));
        }
        if !event_ids.insert(row.event_id.0.as_str()) {
            return Err(DriveStoreError::InvalidInput(format!(
                "duplicate staged outbox event {}",
                row.event_id.0
            )));
        }
        PgRelay::validate_staged_row(row, &lease.region, MAX_FLOW_EVENT_BYTES)
            .map_err(DriveStoreError::from)?;
    }
    Ok(())
}

fn refs_json(
    refs: &Option<Vec<ArtifactRef>>,
) -> Result<Option<serde_json::Value>, DriveStoreError> {
    refs.as_ref()
        .map(|refs| serde_json::to_value(refs).map_err(|e| db("encode history refs", e)))
        .transpose()
}

async fn consume_exact_signal(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
    signal: &SignalKey,
    consumed_seq: i64,
    write: &HistoryWrite,
) -> Result<(), DriveStoreError> {
    let row = sqlx::query(
        "SELECT payload, payload_key_ref, consumed_seq FROM wf_signal \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
           AND signal_name = $4 AND idem_key = $5 FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(run_id)
    .bind(&signal.signal_name)
    .bind(&signal.idem_key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| db("lock exact workflow signal", e))?
    .ok_or_else(|| {
        DriveStoreError::SignalConflict(format!("missing signal {}", signal.idem_key))
    })?;
    let payload: serde_json::Value = row
        .try_get("payload")
        .map_err(|e| db("decode consumed signal payload", e))?;
    let payload_refs = refs_from_json(payload, "consumed signal payload")?;
    let key_ref: Option<String> = row
        .try_get("payload_key_ref")
        .map_err(|e| db("decode consumed signal key ref", e))?;
    let already: Option<i64> = row
        .try_get("consumed_seq")
        .map_err(|e| db("decode consumed signal seq", e))?;
    let mut legacy_expected = vec![ArtifactRef(format!(
        "{WAIT_IDEM_PREFIX}{}",
        signal.idem_key
    ))];
    if let Some(key_ref) = &key_ref {
        legacy_expected.push(ArtifactRef(format!("{WAIT_KEYREF_PREFIX}{key_ref}")));
    }
    legacy_expected.extend(payload_refs);
    let mut bound_expected = legacy_expected.clone();
    bound_expected.insert(
        1,
        ArtifactRef(format!("{WAIT_SIGNAL_NAME_PREFIX}{}", signal.signal_name)),
    );
    let receipt_matches = write.result.as_ref() == Some(&bound_expected)
        || write.result.as_ref() == Some(&legacy_expected);
    if !receipt_matches || write.result_key_ref.is_some() {
        return Err(DriveStoreError::SignalConflict(format!(
            "signal receipt {} does not match buffered payload",
            signal.idem_key
        )));
    }
    match already {
        None => {
            let changed = sqlx::query(
                "UPDATE wf_signal SET consumed_seq = $6 \
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                   AND signal_name = $4 AND idem_key = $5 AND consumed_seq IS NULL",
            )
            .bind(tenant)
            .bind(region)
            .bind(run_id)
            .bind(&signal.signal_name)
            .bind(&signal.idem_key)
            .bind(consumed_seq)
            .execute(&mut *conn)
            .await
            .map_err(|e| db("consume exact workflow signal", e))?
            .rows_affected();
            if changed != 1 {
                return Err(DriveStoreError::SignalConflict(signal.idem_key.clone()));
            }
        }
        Some(seq) if seq == consumed_seq => {}
        Some(_) => return Err(DriveStoreError::SignalConflict(signal.idem_key.clone())),
    }
    Ok(())
}

async fn persist_attempts(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    run_id: &str,
    attempts: &[ActivityAttemptWrite],
) -> Result<(), DriveStoreError> {
    for attempt in attempts {
        let inserted = sqlx::query(
            "INSERT INTO wf_activity_attempt \
               (tenant_id, region, run_id, command_id, attempt, idem_token, state, error, started_at, ended_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
               CASE WHEN $9::bigint IS NULL THEN NULL ELSE to_timestamp($9::double precision / 1000) END, \
               CASE WHEN $10::bigint IS NULL THEN NULL ELSE to_timestamp($10::double precision / 1000) END) \
             ON CONFLICT (tenant_id, region, run_id, command_id, attempt) DO NOTHING",
        )
        .bind(tenant)
        .bind(region)
        .bind(run_id)
        .bind(&attempt.command_id)
        .bind(attempt.attempt)
        .bind(&attempt.idem_token)
        .bind(&attempt.state)
        .bind(&attempt.error)
        .bind(attempt.started_unix_ms)
        .bind(attempt.ended_unix_ms)
        .execute(&mut *conn)
        .await
        .map_err(|e| db("insert workflow activity attempt", e))?;
        if inserted.rows_affected() == 0 {
            let exact: bool = sqlx::query_scalar(
                "SELECT idem_token = $6 AND state = $7 AND error IS NOT DISTINCT FROM $8 \
                   AND (EXTRACT(EPOCH FROM started_at) * 1000)::bigint IS NOT DISTINCT FROM $9 \
                   AND (EXTRACT(EPOCH FROM ended_at) * 1000)::bigint IS NOT DISTINCT FROM $10 \
                 FROM wf_activity_attempt \
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                   AND command_id = $4 AND attempt = $5",
            )
            .bind(tenant)
            .bind(region)
            .bind(run_id)
            .bind(&attempt.command_id)
            .bind(attempt.attempt)
            .bind(&attempt.idem_token)
            .bind(&attempt.state)
            .bind(&attempt.error)
            .bind(attempt.started_unix_ms)
            .bind(attempt.ended_unix_ms)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| db("verify workflow activity attempt", e))?;
            if !exact {
                return Err(DriveStoreError::AttemptConflict(attempt.command_id.clone()));
            }
        }
    }
    Ok(())
}

async fn persist_timer_arms(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    lease: &DriveLease,
    timers: &[TimerArm],
) -> Result<(), DriveStoreError> {
    for timer in timers {
        let bucket = timer.fire_at_unix_secs.div_euclid(60);
        let bucket: i32 = bucket.try_into().map_err(|_| {
            DriveStoreError::InvalidInput("timer deadline is outside supported bucket range".into())
        })?;
        let inserted = sqlx::query(
            "INSERT INTO wf_timer \
               (tenant_id, region, timer_id, run_id, command_id, fire_at, bucket, fired, partition) \
             VALUES ($1, $2, $3, $4, $5, to_timestamp($6), $7, false, $8) \
             ON CONFLICT (tenant_id, region, timer_id) DO NOTHING",
        )
        .bind(tenant)
        .bind(region)
        .bind(&timer.timer_id)
        .bind(&lease.run_id)
        .bind(&timer.command_id)
        .bind(timer.fire_at_unix_secs as f64)
        .bind(bucket)
        .bind(timer.partition)
        .execute(&mut *conn)
        .await
        .map_err(|e| db("arm workflow timer", e))?;
        if inserted.rows_affected() == 0 {
            let exact: bool = sqlx::query_scalar(
                "SELECT run_id = $4 AND command_id = $5 \
                   AND EXTRACT(EPOCH FROM fire_at)::bigint = $6 AND bucket = $7 \
                   AND partition = $8 \
                 FROM wf_timer \
                 WHERE tenant_id = $1 AND region = $2 AND timer_id = $3",
            )
            .bind(tenant)
            .bind(region)
            .bind(&timer.timer_id)
            .bind(&lease.run_id)
            .bind(&timer.command_id)
            .bind(timer.fire_at_unix_secs)
            .bind(bucket)
            .bind(timer.partition)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| db("verify workflow timer arm", e))?;
            if !exact {
                return Err(DriveStoreError::TimerConflict(timer.timer_id.clone()));
            }
        }
    }
    Ok(())
}

async fn persist_timer_disarms(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    lease: &DriveLease,
    timer_ids: &[String],
) -> Result<(), DriveStoreError> {
    for timer_id in timer_ids {
        let result = sqlx::query(
            "UPDATE wf_timer SET fired = true \
             WHERE tenant_id = $1 AND region = $2 AND timer_id = $3 AND run_id = $4",
        )
        .bind(tenant)
        .bind(region)
        .bind(timer_id)
        .bind(&lease.run_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| db("disarm workflow timer", e))?;
        if result.rows_affected() != 1 {
            return Err(DriveStoreError::TimerConflict(format!(
                "missing or foreign timer disarm `{timer_id}`"
            )));
        }
    }
    Ok(())
}

fn drive_fingerprint(commit: &DriveCommit) -> Result<String, DriveStoreError> {
    let history = commit.history.iter().map(|write| {
        serde_json::json!({
            "seq": write.seq,
            "kind": write.kind,
            "command_id": write.command_id,
            "result": write.result,
            "result_key_ref": write.result_key_ref,
            "consume_signal": write.consume_signal.as_ref().map(|signal| {
                serde_json::json!({"signal_name": signal.signal_name, "idem_key": signal.idem_key})
            }),
        })
    }).collect::<Vec<_>>();
    let attempts = commit
        .attempts
        .iter()
        .map(|attempt| {
            serde_json::json!({
                "command_id": attempt.command_id,
                "attempt": attempt.attempt,
                "idem_token": attempt.idem_token,
                "state": attempt.state,
                "error": attempt.error,
                "started_unix_ms": attempt.started_unix_ms,
                "ended_unix_ms": attempt.ended_unix_ms,
            })
        })
        .collect::<Vec<_>>();
    let timers = commit
        .timers
        .iter()
        .map(|timer| {
            serde_json::json!({
                "timer_id": timer.timer_id,
                "command_id": timer.command_id,
                "fire_at_unix_secs": timer.fire_at_unix_secs,
                "partition": timer.partition,
            })
        })
        .collect::<Vec<_>>();
    let outbox = commit
        .outbox
        .iter()
        .map(|row| {
            let envelope = serde_json::to_value(&row.envelope)
                .map_err(|e| db("encode outbox fingerprint", e))?;
            Ok(serde_json::json!({
                "event_id": row.event_id.0,
                "aggregate": row.aggregate.0,
                "subject": row.subject.0,
                "envelope": envelope,
            }))
        })
        .collect::<Result<Vec<_>, DriveStoreError>>()?;
    let value = serde_json::json!({
        "drive_id": commit.drive_id,
        "expected_cursor": commit.expected_cursor,
        "next_state": commit.next_state,
        "history": history,
        "attempts": attempts,
        "timers": timers,
        "timer_disarms": commit.timer_disarms,
        "outbox": outbox,
    });
    let bytes = serde_json::to_vec(&value).map_err(|e| db("encode drive fingerprint", e))?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}
