//! PostgreSQL-backed durable workflow control surface.
//!
//! This module persists the externally visible control operations (`start`, `signal`, `describe`,
//! and `cancel`) and the versioned definition registry. [`crate::pg_drive_store::PgFlowDriveStore`]
//! now supplies the fenced PostgreSQL lease/load/commit boundary for journal, attempts, signals,
//! timers, run state, and outbox. [`crate::pg_dispatcher::PgFlowWorker`] is the production adapter
//! that turns a deterministic workflow body's staged commands into that durable commit batch; this
//! control surface does not silently fall back to the in-memory engine.

use crate::engine::run_state;
use crate::executor::{
    partition_for_run_id, DurableExecutor, ExecutorError, RunId, RunStatus, SignalOutcome,
    SignalSpec, StartSpec,
};
use myelin_events::IdMinter;
use myelin_storage::{with_tenant_tx, with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::Row;
use std::future::Future;
use std::sync::Arc;

/// Bridge the existing synchronous contract to sqlx. Production callers must drive this from a
/// dedicated thread or a multi-thread Tokio runtime, matching the other durable store adapters.
fn bridge<F: Future>(rt: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(future)),
        Err(_) => rt.block_on(future),
    }
}

fn store_error(operation: &str, error: impl std::fmt::Display) -> ExecutorError {
    ExecutorError::Storage(format!("{operation}: {error}"))
}

enum SignalStoreError {
    UnknownRun,
    TerminalRun,
    DivergentReplay,
    Storage(PgError),
}

impl From<PgError> for SignalStoreError {
    fn from(error: PgError) -> Self {
        Self::Storage(error)
    }
}

fn bounded(label: &str, value: &str, max: usize) -> Result<(), ExecutorError> {
    if value.trim().is_empty() || value.len() > max {
        return Err(ExecutorError::InvalidInput(format!(
            "{label} must be non-empty and at most {max} bytes"
        )));
    }
    Ok(())
}

fn validate_refs(
    refs: &[myelin_refs::ArtifactRef],
    tenant: &TenantId,
) -> Result<(), ExecutorError> {
    for artifact in refs {
        let parsed = myelin_refs::parse_scoped(&artifact.0).map_err(|error| {
            ExecutorError::InvalidInput(format!("malformed ArtifactRef: {error}"))
        })?;
        if parsed.tenant != *tenant {
            return Err(ExecutorError::InvalidInput(
                "ArtifactRef tenant does not match the verified executor tenant".into(),
            ));
        }
    }
    Ok(())
}

/// Durable, tenant- and residency-scoped implementation of [`DurableExecutor`].
#[derive(Clone)]
pub struct PgFlowExecutor {
    pool: PgPool,
    rt: tokio::runtime::Handle,
    minter: Arc<dyn IdMinter>,
    tenant: TenantId,
    region: Region,
}

impl PgFlowExecutor {
    /// Bind the durable executor to one verified tenant and one residency cell.
    pub fn new(
        pool: PgPool,
        rt: tokio::runtime::Handle,
        minter: Arc<dyn IdMinter>,
        tenant: TenantId,
        region: Region,
    ) -> Self {
        Self {
            pool,
            rt,
            minter,
            tenant,
            region,
        }
    }

    /// Register immutable workflow code. Re-registering the exact hash is idempotent; reusing a
    /// `(wf_type, version)` for different code fails closed because in-flight runs pin that version.
    pub fn register_definition(
        &self,
        wf_type: &str,
        version: i32,
        code_hash: &str,
    ) -> Result<(), ExecutorError> {
        bridge(
            &self.rt,
            self.register_definition_async(wf_type, version, code_hash),
        )
    }

    async fn register_definition_async(
        &self,
        wf_type: &str,
        version: i32,
        code_hash: &str,
    ) -> Result<(), ExecutorError> {
        bounded("wf_type", wf_type, 128)?;
        bounded("code_hash", code_hash, 256)?;
        if version <= 0 {
            return Err(ExecutorError::InvalidInput(
                "workflow definition version must be positive".into(),
            ));
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| store_error("begin definition registration", e))?;
        // Global code registry: tenant_id/region do not apply because definitions contain no tenant
        // data. The explicit comment is also a loud architecture-lint marker, not a silent waiver.
        let tenant_id_not_applicable = sqlx::query(
            "INSERT INTO wf_definition (wf_type, version, code_hash, status) \
             VALUES ($1, $2, $3, 'active') ON CONFLICT (wf_type, version) DO NOTHING \
             /* global registry: tenant_id and region do not apply */",
        );
        tenant_id_not_applicable
            .bind(wf_type)
            .bind(version)
            .bind(code_hash)
            .execute(&mut *tx)
            .await
            .map_err(|e| store_error("insert workflow definition", e))?;

        let tenant_id_not_applicable = sqlx::query(
            "SELECT code_hash, status FROM wf_definition WHERE wf_type = $1 AND version = $2 \
             /* global registry: tenant_id and region do not apply */",
        );
        let row = tenant_id_not_applicable
            .bind(wf_type)
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| store_error("read workflow definition", e))?;
        let recorded_hash: String = row
            .try_get("code_hash")
            .map_err(|e| store_error("decode workflow definition hash", e))?;
        let status: String = row
            .try_get("status")
            .map_err(|e| store_error("decode workflow definition status", e))?;
        if recorded_hash != code_hash {
            return Err(ExecutorError::DefinitionDrift(format!(
                "{wf_type}@{version} is already registered with a different code hash"
            )));
        }
        if status != "active" {
            return Err(ExecutorError::DefinitionDrift(format!(
                "{wf_type}@{version} is `{status}`, not active"
            )));
        }
        tx.commit()
            .await
            .map_err(|e| store_error("commit definition registration", e))?;
        Ok(())
    }

    async fn start_async(
        &self,
        spec: StartSpec,
        requested_run_id: Option<RunId>,
    ) -> Result<RunId, ExecutorError> {
        bounded("wf_type", &spec.wf_type, 128)?;
        bounded("idem_key", &spec.idem_key, 512)?;
        validate_refs(&spec.input, &self.tenant)?;
        if let Some(run_id) = requested_run_id.as_ref() {
            bounded("run_id", &run_id.0, 256)?;
        }
        let scope_tenant = self.tenant.0.clone();
        let scope_region = self.region.0.clone();
        let tenant = scope_tenant.clone();
        let region = scope_region.clone();
        let minted = requested_run_id
            .clone()
            .unwrap_or_else(|| RunId(self.minter.mint().0));
        let partition = partition_for_run_id(&minted.0);
        let input = serde_json::to_string(&spec.input)
            .map_err(|e| store_error("encode workflow input refs", e))?;
        let budget = spec
            .budget
            .as_ref()
            .map(|budget| serde_json::json!({ "minor_units": budget.minor_units }).to_string());
        let requested_id = requested_run_id.is_some();
        let error_wf_type = spec.wf_type.clone();
        let error_run_id = minted.0.clone();

        with_tenant_tx(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                // The idempotency anchor wins before definition lookup. A replay after a deploy
                // returns the original handle even if that definition is now draining.
                if let Some(existing) = sqlx::query_scalar::<_, String>(
                    "SELECT run_id FROM workflow_run WHERE tenant_id = $1 AND region = $2 \
                     AND idem_key = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&spec.idem_key)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("find idempotent workflow start: {e}")))?
                {
                    return Ok(RunId(existing));
                }

                // Global code registry: tenant_id/region do not apply to code-only definitions.
                let tenant_id_not_applicable = sqlx::query_scalar::<_, i32>(
                    "SELECT version FROM wf_definition WHERE wf_type = $1 AND status = 'active' \
                     /* global registry: tenant_id and region do not apply */ \
                     ORDER BY version DESC LIMIT 1",
                );
                let wf_version = tenant_id_not_applicable
                    .bind(&spec.wf_type)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| {
                        PgError::Query(format!("resolve active workflow definition: {e}"))
                    })?;
                let Some(wf_version) = wf_version else {
                    return Err(PgError::Query(format!(
                        "unknown workflow type: {}",
                        spec.wf_type
                    )));
                };

                let inserted = sqlx::query(
                    "INSERT INTO workflow_run (tenant_id, region, run_id, wf_type, wf_version, \
                     input, state, cursor, budget, correlation_id, causation_id, caused_by, depth, \
                     partition, idem_key) VALUES ($1, $2, $3, $4, $5, CAST($6 AS jsonb), \
                     'running', 0, CAST($7 AS jsonb), $3, NULL, NULL, 0, $8, $9) \
                     ON CONFLICT DO NOTHING",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&minted.0)
                .bind(&spec.wf_type)
                .bind(wf_version)
                .bind(&input)
                .bind(&budget)
                .bind(partition)
                .bind(&spec.idem_key)
                .execute(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("insert durable workflow run: {e}")))?;
                if inserted.rows_affected() == 1 {
                    return Ok(minted);
                }

                // A concurrent start under the same key is success and returns the winner.
                if let Some(existing) = sqlx::query_scalar::<_, String>(
                    "SELECT run_id FROM workflow_run WHERE tenant_id = $1 AND region = $2 \
                     AND idem_key = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&spec.idem_key)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("resolve concurrent workflow start: {e}")))?
                {
                    return Ok(RunId(existing));
                }

                let kind = if requested_id { "provided" } else { "minted" };
                Err(PgError::Query(format!(
                    "{kind} run_id collision: {}",
                    minted.0
                )))
            })
        })
        .await
        .map_err(|error| {
            let text = error.to_string();
            if text.contains("unknown workflow type:") {
                ExecutorError::UnknownWorkflow(error_wf_type)
            } else if text.contains("run_id collision:") {
                ExecutorError::RunIdConflict(error_run_id)
            } else {
                store_error("start workflow", error)
            }
        })
    }

    async fn signal_async(&self, spec: SignalSpec) -> Result<SignalOutcome, ExecutorError> {
        bounded("run_id", &spec.run.0, 256)?;
        bounded("signal_name", &spec.signal_name, 128)?;
        bounded("idem_key", &spec.idem_key, 512)?;
        validate_refs(&spec.payload, &self.tenant)?;
        let scope_tenant = self.tenant.0.clone();
        let scope_region = self.region.0.clone();
        let tenant = scope_tenant.clone();
        let region = scope_region.clone();
        let run_id = spec.run.0.clone();
        let error_run_id = run_id.clone();
        let payload = serde_json::to_string(&spec.payload)
            .map_err(|e| store_error("encode signal payload refs", e))?;
        let expected_payload = serde_json::to_value(&spec.payload)
            .map_err(|e| store_error("encode signal payload refs", e))?;
        with_tenant_tx_error(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                // Serialise signal delivery against drive commits/cancellation and pin the lifecycle
                // decision in the same transaction as insertion. Terminal history is immutable.
                let state = sqlx::query_scalar::<_, String>(
                    "SELECT state FROM workflow_run WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&run_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| {
                    PgError::Query(format!("lock signal target run: {e}"))
                })?;
                let Some(state) = state else {
                    return Err(SignalStoreError::UnknownRun);
                };
                if run_state::is_terminal(&state) {
                    return Err(SignalStoreError::TerminalRun);
                }

                let inserted = sqlx::query(
                    "INSERT INTO wf_signal (tenant_id, region, run_id, signal_name, idem_key, \
                     payload, payload_key_ref) VALUES ($1, $2, $3, $4, $5, CAST($6 AS jsonb), $7) \
                     ON CONFLICT (tenant_id, run_id, signal_name, idem_key) DO NOTHING",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&run_id)
                .bind(&spec.signal_name)
                .bind(&spec.idem_key)
                .bind(&payload)
                .bind(&spec.payload_key_ref)
                .execute(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("buffer durable workflow signal: {e}")))?;

                let first_delivery = inserted.rows_affected() == 1;
                let should_wake = if first_delivery {
                    true
                } else {
                    let existing = sqlx::query(
                        "SELECT payload, payload_key_ref, consumed_seq FROM wf_signal \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                         AND signal_name = $4 AND idem_key = $5",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&run_id)
                    .bind(&spec.signal_name)
                    .bind(&spec.idem_key)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(|e| {
                        PgError::Query(format!("verify duplicate durable workflow signal: {e}"))
                    })?;
                    let stored_payload: serde_json::Value = existing.try_get("payload").map_err(|e| {
                        PgError::Query(format!("decode duplicate signal payload: {e}"))
                    })?;
                    let stored_key_ref: Option<String> = existing
                        .try_get("payload_key_ref")
                        .map_err(|e| PgError::Query(format!("decode duplicate signal key ref: {e}")))?;
                    let consumed_seq: Option<i64> = existing
                        .try_get("consumed_seq")
                        .map_err(|e| PgError::Query(format!("decode duplicate signal state: {e}")))?;
                    if stored_payload != expected_payload || stored_key_ref != spec.payload_key_ref {
                        return Err(SignalStoreError::DivergentReplay);
                    }
                    consumed_seq.is_none()
                };

                // Signal insertion and waiting→running wake are one transaction: a crash cannot
                // leave a new/pending durable signal parked behind a sleeping run. A duplicate
                // already consumed by history is observational only and must never resurrect it.
                if should_wake {
                    sqlx::query(
                        "UPDATE workflow_run SET state = 'running', updated_at = now() \
                         WHERE tenant_id = $1 AND region = $2 AND run_id = $3 AND state = 'waiting'",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&run_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| PgError::Query(format!("wake signalled workflow run: {e}")))?;
                }
                Ok(if first_delivery {
                    SignalOutcome::Buffered
                } else {
                    SignalOutcome::Duplicate
                })
            })
        })
        .await
        .map_err(|error| match error {
            SignalStoreError::UnknownRun => ExecutorError::UnknownRun(error_run_id),
            SignalStoreError::TerminalRun => ExecutorError::InvalidInput(
                "signals cannot target a terminal workflow run".into(),
            ),
            SignalStoreError::DivergentReplay => ExecutorError::InvalidInput(
                "signal idempotency key was reused with a divergent payload or payload key reference"
                    .into(),
            ),
            SignalStoreError::Storage(error) => store_error("signal workflow", error),
        })
    }

    async fn describe_async(&self, run: &RunId) -> Result<RunStatus, ExecutorError> {
        let scope_tenant = self.tenant.0.clone();
        let scope_region = self.region.0.clone();
        let tenant = scope_tenant.clone();
        let region = scope_region.clone();
        let run_id = run.0.clone();
        let row = with_tenant_tx(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "SELECT wf_type, state, cursor, wf_version FROM workflow_run \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&run_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("describe durable workflow run: {e}")))
            })
        })
        .await
        .map_err(|e| store_error("describe workflow", e))?;
        let Some(row) = row else {
            return Err(ExecutorError::UnknownRun(run.0.clone()));
        };
        let state: String = row
            .try_get("state")
            .map_err(|e| store_error("decode workflow state", e))?;
        Ok(RunStatus {
            run_id: run.clone(),
            wf_type: row
                .try_get("wf_type")
                .map_err(|e| store_error("decode workflow type", e))?,
            cursor: row
                .try_get("cursor")
                .map_err(|e| store_error("decode workflow cursor", e))?,
            wf_version: row
                .try_get("wf_version")
                .map_err(|e| store_error("decode workflow version", e))?,
            terminal: run_state::is_terminal(&state),
            state,
        })
    }

    async fn cancel_async(&self, run: &RunId, reason: &str) -> Result<(), ExecutorError> {
        bounded("run_id", &run.0, 256)?;
        bounded("cancel reason", reason, 128)?;
        let scope_tenant = self.tenant.0.clone();
        let scope_region = self.region.0.clone();
        let tenant = scope_tenant.clone();
        let region = scope_region.clone();
        let run_id = run.0.clone();
        let reason = reason.to_string();
        let changed = with_tenant_tx(&self.pool, &scope_tenant, &scope_region, move |conn| {
            Box::pin(async move {
                let result = sqlx::query(
                    "UPDATE workflow_run SET state = 'terminated', cancel_reason = $4, \
                     lease_owner = NULL, lease_expires = NULL, updated_at = now() \
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3 \
                     AND state NOT IN ('completed','failed','terminated','nondeterministic')",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&run_id)
                .bind(&reason)
                .execute(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("cancel durable workflow run: {e}")))?;
                if result.rows_affected() == 1 {
                    return Ok(true);
                }
                let exists = sqlx::query_scalar::<_, i32>(
                    "SELECT 1 FROM workflow_run WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                )
                .bind(&tenant)
                .bind(&region)
                .bind(&run_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| PgError::Query(format!("verify cancelled workflow run: {e}")))?;
                Ok(exists.is_some())
            })
        })
        .await
        .map_err(|e| store_error("cancel workflow", e))?;
        if changed {
            Ok(())
        } else {
            Err(ExecutorError::UnknownRun(run.0.clone()))
        }
    }
}

impl DurableExecutor for PgFlowExecutor {
    fn start_with_id(
        &self,
        spec: StartSpec,
        run_id: Option<RunId>,
    ) -> Result<RunId, ExecutorError> {
        bridge(&self.rt, self.start_async(spec, run_id))
    }

    fn signal(&self, spec: SignalSpec) -> Result<SignalOutcome, ExecutorError> {
        bridge(&self.rt, self.signal_async(spec))
    }

    fn describe(&self, run: &RunId) -> Result<RunStatus, ExecutorError> {
        bridge(&self.rt, self.describe_async(run))
    }

    fn cancel(&self, run: &RunId, reason: &str) -> Result<(), ExecutorError> {
        bridge(&self.rt, self.cancel_async(run, reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_and_memory_executors_share_partitioning() {
        for run_id in ["01J00000000000000000000000", "wf:evt-1", "another-run"] {
            let partition = partition_for_run_id(run_id);
            assert!((0..crate::PARTITION_COUNT as i16).contains(&partition));
            assert_eq!(partition, partition_for_run_id(run_id));
        }
    }

    #[test]
    fn storage_errors_do_not_expose_a_memory_fallback() {
        let error = store_error("start workflow", "database unavailable");
        assert_eq!(
            error,
            ExecutorError::Storage("start workflow: database unavailable".into())
        );
    }

    #[test]
    fn malformed_and_cross_tenant_refs_fail_before_database_access() {
        let tenant = TenantId("acme".into());
        assert!(matches!(
            validate_refs(&[myelin_refs::ArtifactRef("short-ref".into())], &tenant),
            Err(ExecutorError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_refs(
                &[myelin_refs::ArtifactRef(
                    "myelin://other/git/repo/core".into()
                )],
                &tenant
            ),
            Err(ExecutorError::InvalidInput(_))
        ));
    }
}
