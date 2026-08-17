use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use myelin_ci_sandbox::ResourceUsage;
use myelin_events::check_seam::{ci_result_draft, rollup_ci_result};
use myelin_events::{
    derive_envelope_from_persisted_cause, Actor, AggregateKey, ArtifactRef, CausedBy,
    CorrelationId, DataRole, EmitContext, EventDraft, EventId, EventType, MonotonicMinter,
    PersistedEventCause, Timestamp, Visibility,
};
use myelin_flow::{
    CancelOnConnOutcome, PgFlowExecutor, RunId, SignalPayload, TypedSignalSpec, JOB_DONE_SIGNAL,
};
use myelin_identity::{
    DataRole as IdentityDataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::{DurableCostLedger, MeteredUnit, RunId as CostRunId, TenantScope};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::ci_pipeline_driver::{
    checked_accounting_usage, checked_add_accounting_usage, close_cancelled_run_if_accounted,
    decode_retry_attempt_usage, priced_cost_rows, settle_cancelled_ci_job_surface_on_conn,
    validate_reservation_pricing_policy, CiUsageAggregationError, DurableCiJobAccounting,
    TIER_P_OPERATIONAL_PRICING_REVISION, TIER_P_OPERATIONAL_RESERVATION_PREFIX,
    TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX,
};
use crate::ci_prelaunch_usage_journal::{
    resolve_prelaunch_usage_on_conn, CiPrelaunchParentExpectation, CiPrelaunchSettlementIdentity,
    CiPrelaunchUnresolvedPolicy, CiPrelaunchUsageAccrual, CiPrelaunchUsageJournalError,
};
use crate::job_accounting_store::{
    disposition_receipt_v4, versioned_accounting_receipt, CiJobTerminalDisposition,
};
use crate::{
    CiCostEventStore, CiDriveManifestStore, CiJobAccountingPricer, CiJobAccountingRecord,
    CiJobAccountingStore, Meter, TierPOperationalCiJobPricer,
};

const SUPERSEDED_REASON: &str = "superseded-by-newer-pr-head";
const EVENT_ID_DOMAIN: &str = "myelin.ci.pr-run-supersession-event.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeadDecision {
    Current,
    Superseded,
}

#[derive(Clone, Debug)]
struct ActivePrRun {
    run_id: String,
    wf_run_id: String,
    state: String,
    generation: Option<i64>,
}

#[derive(Debug)]
struct QueueLifecycle {
    state: String,
    completion_receipt: Option<String>,
    retry_usage: Option<ResourceUsage>,
}

#[derive(Debug)]
struct CancellationProvenance {
    repo_ref: String,
    cause_event_id: String,
    correlation_id: String,
    caused_by: Option<String>,
    cause_depth: u32,
    finished_at: String,
    emitted_at: String,
}

#[derive(Debug)]
pub enum CiRunSupersessionError {
    InvalidScope,
    Database(&'static str),
    Manifest,
    Workflow,
    Accounting,
    Pricing,
    Prelaunch(CiPrelaunchUsageJournalError),
    Usage(CiUsageAggregationError),
    Settlement,
    CorruptState(&'static str),
}

impl std::fmt::Display for CiRunSupersessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScope => f.write_str("CI run supersession scope is invalid"),
            Self::Database(operation) => {
                write!(
                    f,
                    "CI run supersession database operation failed: {operation}"
                )
            }
            Self::Manifest => f.write_str("CI run supersession manifest authority is unavailable"),
            Self::Workflow => f.write_str("CI run supersession workflow termination was refused"),
            Self::Accounting => f.write_str("CI run supersession accounting was refused"),
            Self::Pricing => f.write_str("CI run supersession pricing policy was refused"),
            Self::Prelaunch(error) => {
                write!(
                    f,
                    "CI run supersession prelaunch usage was refused: {error}"
                )
            }
            Self::Usage(error) => {
                write!(
                    f,
                    "CI run supersession usage aggregation was refused: {error}"
                )
            }
            Self::Settlement => f.write_str("CI run supersession settlement was refused"),
            Self::CorruptState(detail) => {
                write!(
                    f,
                    "CI run supersession found corrupt durable state: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for CiRunSupersessionError {}

#[derive(Clone)]
pub struct PgCiRunSupersession {
    pool: sqlx::PgPool,
    tenant: TenantId,
    region: Region,
    scope: TenantScope,
    manifest: CiDriveManifestStore,
    accounting: CiJobAccountingStore,
    costs: CiCostEventStore,
    ledger: DurableCostLedger,
    executor: PgFlowExecutor,
}

impl PgCiRunSupersession {
    pub fn new(
        pool: sqlx::PgPool,
        ledger: DurableCostLedger,
        tenant: TenantId,
        region: Region,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, CiRunSupersessionError> {
        if !valid_machine_token(&tenant.0) || !valid_machine_token(&region.0) {
            return Err(CiRunSupersessionError::InvalidScope);
        }
        let principal = Principal::new(
            tenant.clone(),
            region.clone(),
            PrincipalId("svc:ci-controlplane".into()),
            PrincipalKind::Service,
            IdentityDataRole::Processor,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&principal, region.clone());
        let manifest = CiDriveManifestStore::new(pool.clone(), tenant.clone(), region.clone())
            .map_err(|_| CiRunSupersessionError::Manifest)?;
        Ok(Self {
            pool: pool.clone(),
            tenant: tenant.clone(),
            region: region.clone(),
            scope,
            manifest,
            accounting: CiJobAccountingStore::with_pg_and_write_version(
                pool.clone(),
                region.clone(),
                crate::ci_pipeline_protocol::PRODUCTION_ACCOUNTING_WRITE_VERSION,
            ),
            costs: CiCostEventStore::with_pg(pool.clone(), region.clone()),
            ledger,
            executor: PgFlowExecutor::new(
                pool,
                rt,
                Arc::new(MonotonicMinter::new()),
                tenant,
                region,
            ),
        })
    }

    pub(crate) async fn lock_group_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        group: &str,
    ) -> Result<(), CiRunSupersessionError> {
        if !crate::ci_run_store::valid_pr_concurrency_group(group) {
            return Err(CiRunSupersessionError::InvalidScope);
        }
        crate::ci_run_store::lock_pr_concurrency_group_on_conn(
            conn,
            &self.tenant.0,
            &self.region.0,
            group,
        )
        .await
        .map_err(|_| CiRunSupersessionError::Database("lock PR concurrency group"))?;
        Ok(())
    }

    pub(crate) async fn classify_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        run_id: &str,
        group: &str,
        generation: Option<i64>,
    ) -> Result<HeadDecision, CiRunSupersessionError> {
        let peers = self.generation_peers(conn, run_id, group).await?;
        let stale = match generation {
            Some(candidate) => peers
                .iter()
                .any(|peer| peer.generation.is_some_and(|other| other > candidate)),
            None => peers.iter().any(|peer| peer.generation.is_some()),
        };
        Ok(if stale {
            HeadDecision::Superseded
        } else {
            HeadDecision::Current
        })
    }

    pub(crate) async fn cancel_stale_queued_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        run_id: &str,
        wf_run_id: &str,
    ) -> Result<(), CiRunSupersessionError> {
        let attached: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM ci_drive_manifest \
                              WHERE tenant_id = $1 AND region = $2 \
                                AND ci_run_id = $3::uuid) \
                 OR EXISTS (SELECT 1 FROM workflow_run \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $4) \
                 OR EXISTS (SELECT 1 FROM ci_job \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid) \
                 OR EXISTS (SELECT 1 FROM job_queue \
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $4::uuid) \
                 OR EXISTS (SELECT 1 FROM ci_job_accounting \
                             WHERE tenant_id = $1 AND region = $2 AND ci_run_id = $3::uuid) \
                 OR EXISTS (SELECT 1 FROM cost_reservation \
                             WHERE tenant_id = $1 AND region = $2 \
                               AND (run_id LIKE ('ci-reserve:v1:' || $3::text || ':%') \
                                    OR run_id LIKE ('ci-reserve:v2:' || $3::text || ':%')))",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(run_id)
        .bind(wf_run_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("verify stale queued run"))?;
        if attached {
            return Err(CiRunSupersessionError::CorruptState(
                "queued run already owns launch, workflow, or accounting authority",
            ));
        }
        let updated = sqlx::query(
            "UPDATE ci_run \
             SET state = 'cancelled', cost_settled = true, finished_at = clock_timestamp() \
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
               AND wf_run_id = $4::uuid AND state = 'queued' \
               AND cost_settled = false AND finished_at IS NULL",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(run_id)
        .bind(wf_run_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("cancel stale queued run"))?;
        if updated.rows_affected() != 1 {
            return Err(CiRunSupersessionError::CorruptState(
                "stale queued run lifecycle changed under its lock",
            ));
        }
        emit_stale_run_cancelled_on_conn(conn, &self.tenant, &self.region, run_id).await?;
        Ok(())
    }

    pub(crate) async fn cancel_older_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        keep_run_id: &str,
        group: &str,
        generation: Option<i64>,
    ) -> Result<Vec<String>, CiRunSupersessionError> {
        let Some(candidate_generation) = generation else {
            return Ok(Vec::new());
        };
        let peers = self.active_peers(conn, keep_run_id, group).await?;
        let mut cancelled = Vec::new();
        for peer in peers {
            let is_older = peer
                .generation
                .is_none_or(|peer_generation| peer_generation < candidate_generation);
            if !is_older {
                continue;
            }
            match peer.state.as_str() {
                "queued" => {
                    self.cancel_stale_queued_on_conn(conn, &peer.run_id, &peer.wf_run_id)
                        .await?;
                }
                "running" => self.cancel_running_on_conn(conn, &peer).await?,
                _ => {
                    return Err(CiRunSupersessionError::CorruptState(
                        "active peer has an impossible lifecycle token",
                    ))
                }
            }
            cancelled.push(peer.run_id);
        }
        Ok(cancelled)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub async fn cancel_running_for_test(
        &self,
        ci_run_id: &str,
        wf_run_id: &str,
    ) -> Result<(), CiRunSupersessionError> {
        let authority = self.clone();
        let peer = ActivePrRun {
            run_id: ci_run_id.to_owned(),
            wf_run_id: wf_run_id.to_owned(),
            state: "running".into(),
            generation: Some(1),
        };
        myelin_storage::with_tenant_tx_error(
            &self.pool,
            &self.tenant.0,
            &self.region.0,
            move |conn| {
                Box::pin(async move { authority.cancel_running_on_conn(conn, &peer).await })
            },
        )
        .await
    }

    async fn generation_peers(
        &self,
        conn: &mut sqlx::PgConnection,
        keep_run_id: &str,
        group: &str,
    ) -> Result<Vec<ActivePrRun>, CiRunSupersessionError> {
        self.peers(conn, keep_run_id, group, false).await
    }

    async fn active_peers(
        &self,
        conn: &mut sqlx::PgConnection,
        keep_run_id: &str,
        group: &str,
    ) -> Result<Vec<ActivePrRun>, CiRunSupersessionError> {
        self.peers(conn, keep_run_id, group, true).await
    }

    async fn peers(
        &self,
        conn: &mut sqlx::PgConnection,
        keep_run_id: &str,
        group: &str,
        active_only: bool,
    ) -> Result<Vec<ActivePrRun>, CiRunSupersessionError> {
        let rows = sqlx::query(
            "SELECT run_id::text AS run_id, wf_run_id::text AS wf_run_id, state, \
                    pr_head_generation \
             FROM ci_run \
             WHERE tenant_id = $1 AND region = $2 AND concurrency_group = $3 \
               AND run_id <> $4::uuid \
               AND (NOT $5 OR state IN ('queued','running')) \
             ORDER BY run_id",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(group)
        .bind(keep_run_id)
        .bind(active_only)
        .fetch_all(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("read active PR peers"))?;
        Ok(rows
            .into_iter()
            .map(|row| ActivePrRun {
                run_id: row.get("run_id"),
                wf_run_id: row.get("wf_run_id"),
                state: row.get("state"),
                generation: row.get("pr_head_generation"),
            })
            .collect())
    }

    async fn cancel_running_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        peer: &ActivePrRun,
    ) -> Result<(), CiRunSupersessionError> {
        let (manifest, _) = self
            .manifest
            .load_by_wf_run_on_conn(conn, &peer.wf_run_id)
            .await
            .map_err(|_| CiRunSupersessionError::Manifest)?
            .ok_or(CiRunSupersessionError::Manifest)?;
        if manifest.tenant_id != self.tenant.0
            || manifest.region != self.region.0
            || manifest.ci_run_id != peer.run_id
            || manifest.wf_run_id != peer.wf_run_id
        {
            return Err(CiRunSupersessionError::CorruptState(
                "manifest disagrees with superseded run identity",
            ));
        }
        let manifest_jobs: BTreeSet<&str> = manifest
            .jobs
            .iter()
            .map(|job| job.job_id.as_str())
            .collect();
        if manifest_jobs.len() != manifest.jobs.len() {
            return Err(CiRunSupersessionError::CorruptState(
                "manifest contains duplicate job identity",
            ));
        }
        let flow_outcome = self
            .executor
            .cancel_on_conn(conn, &RunId(peer.wf_run_id.clone()), SUPERSEDED_REASON)
            .await
            .map_err(|_| CiRunSupersessionError::Workflow)?;
        let queue_rows = sqlx::query(
            "SELECT job_id::text AS job_id, state, completion_receipt, retry_attempts \
             FROM job_queue \
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
             ORDER BY job_id FOR UPDATE",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(&peer.wf_run_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("lock superseded job queue"))?;
        let mut queued = BTreeMap::new();
        for row in queue_rows {
            let job_id: String = row.get("job_id");
            let retry_usage =
                decode_retry_attempt_usage(row.get("retry_attempts")).map_err(|_| {
                    CiRunSupersessionError::CorruptState(
                        "job queue carries malformed retry-attempt accrual",
                    )
                })?;
            if !manifest_jobs.contains(job_id.as_str())
                || queued
                    .insert(
                        job_id,
                        QueueLifecycle {
                            state: row.get("state"),
                            completion_receipt: row.get("completion_receipt"),
                            retry_usage,
                        },
                    )
                    .is_some()
            {
                return Err(CiRunSupersessionError::CorruptState(
                    "job queue diverges from immutable manifest",
                ));
            }
        }

        let mut unsettled_launched = 0usize;
        for job in &manifest.jobs {
            if let Some(existing) = self
                .accounting
                .load_in_tx(conn, &self.scope, &job.job_id)
                .await
                .map_err(|_| CiRunSupersessionError::Accounting)?
            {
                self.verify_existing_accounting_on_conn(
                    conn,
                    peer,
                    job,
                    queued.get(job.job_id.as_str()),
                    &existing,
                )
                .await?;
                continue;
            }
            if flow_outcome == CancelOnConnOutcome::TerminalNoOp {
                return Err(CiRunSupersessionError::CorruptState(
                    "terminal Flow lacks complete immutable accounting",
                ));
            }
            match queued
                .get(job.job_id.as_str())
                .map(|lifecycle| lifecycle.state.as_str())
            {
                Some("running") => unsettled_launched += 1,
                Some("queued" | "leased") => {
                    let retry_usage = queued
                        .get(job.job_id.as_str())
                        .and_then(|lifecycle| lifecycle.retry_usage);
                    let updated = sqlx::query(
                        "UPDATE job_queue \
                         SET state = 'terminal', lease_owner = NULL, lease_expires = NULL \
                         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid \
                           AND state IN ('queued','leased') AND completion_receipt IS NULL",
                    )
                    .bind(&self.tenant.0)
                    .bind(&self.region.0)
                    .bind(&job.job_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(|_| {
                        CiRunSupersessionError::Database("cancel superseded pre-launch job")
                    })?;
                    if updated.rows_affected() != 1 {
                        return Err(CiRunSupersessionError::CorruptState(
                            "pre-launch cancellation lost its locked queue authority",
                        ));
                    }
                    self.settle_cancelled_job(
                        conn,
                        &manifest,
                        job,
                        retry_usage.unwrap_or(ResourceUsage {
                            cpu_seconds: 0,
                            mem_byte_seconds: 0,
                        }),
                        retry_usage.is_none(),
                    )
                    .await?;
                }
                None => {
                    self.settle_cancelled_job(
                        conn,
                        &manifest,
                        job,
                        ResourceUsage {
                            cpu_seconds: 0,
                            mem_byte_seconds: 0,
                        },
                        true,
                    )
                    .await?
                }
                Some("terminal") => {
                    return Err(CiRunSupersessionError::CorruptState(
                        "terminal queue job lacks immutable accounting",
                    ))
                }
                Some(_) => {
                    return Err(CiRunSupersessionError::CorruptState(
                        "job queue has an impossible lifecycle token",
                    ))
                }
            }
        }

        let updated = sqlx::query(
            "UPDATE ci_run \
             SET state = 'cancelled', cost_settled = $5, \
                 finished_at = COALESCE(finished_at, clock_timestamp()) \
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
               AND wf_run_id = $4::uuid AND state = 'running' \
               AND cost_settled = false AND finished_at IS NULL",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(&peer.run_id)
        .bind(&peer.wf_run_id)
        .bind(unsettled_launched == 0)
        .execute(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("finalize superseded CI run"))?;
        if updated.rows_affected() == 1 && flow_outcome == CancelOnConnOutcome::Terminated {
            emit_cancelled_manifest_facts_on_conn(
                conn,
                &self.tenant,
                &self.region,
                &manifest,
                unsettled_launched == 0,
                true,
            )
            .await?;
        } else if updated.rows_affected() == 0 && flow_outcome == CancelOnConnOutcome::TerminalNoOp
        {
            let row = sqlx::query(
                "SELECT state, cost_settled FROM ci_run \
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
                   AND wf_run_id = $4::uuid",
            )
            .bind(&self.tenant.0)
            .bind(&self.region.0)
            .bind(&peer.run_id)
            .bind(&peer.wf_run_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| CiRunSupersessionError::Database("verify superseded CI run"))?
            .ok_or(CiRunSupersessionError::CorruptState(
                "superseded run disappeared",
            ))?;
            let state: String = row.get("state");
            let settled: bool = row.get("cost_settled");
            if !matches!(state.as_str(), "succeeded" | "failed" | "timed_out") || !settled {
                return Err(CiRunSupersessionError::CorruptState(
                    "superseded run lifecycle collided",
                ));
            }
        } else {
            return Err(CiRunSupersessionError::CorruptState(
                "Flow and CI run terminal transitions disagreed",
            ));
        }
        Ok(())
    }

    async fn verify_existing_accounting_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        peer: &ActivePrRun,
        job: &crate::GrantedCiJobV1,
        queue: Option<&QueueLifecycle>,
        existing: &CiJobAccountingRecord,
    ) -> Result<(), CiRunSupersessionError> {
        if existing.tenant != self.tenant
            || existing.wf_run_id != peer.wf_run_id
            || existing.ci_run_id != peer.run_id
            || existing.reserve_handle != job.reserve_handle
        {
            return Err(CiRunSupersessionError::CorruptState(
                "accounting receipt diverges from manifest",
            ));
        }
        enum ReplayKind {
            Reported,
            Superseded,
        }
        let replay_kind = match queue {
            Some(queue)
                if queue.state == "terminal"
                    && !existing.skipped
                    && queue.completion_receipt.as_deref()
                        == Some(existing.completion_receipt.as_str()) =>
            {
                ReplayKind::Reported
            }
            Some(queue)
                if queue.state == "terminal"
                    && queue.completion_receipt.is_none()
                    && !existing.passed
                    && !existing.timed_out =>
            {
                ReplayKind::Superseded
            }
            None if existing.skipped && !existing.passed && !existing.timed_out => {
                ReplayKind::Superseded
            }
            _ => {
                return Err(CiRunSupersessionError::CorruptState(
                    "accounting receipt disagrees with queue lifecycle",
                ))
            }
        };
        let base_usage = queue
            .and_then(|queue| queue.retry_usage)
            .unwrap_or(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            });
        let (accounted_floor, prelaunch) = self
            .resolve_job_usage_on_conn(
                conn,
                CiPrelaunchSettlementIdentity {
                    tenant_id: self.tenant.as_str(),
                    region: self.region.as_str(),
                    job_id: &job.job_id,
                    wf_run_id: &peer.wf_run_id,
                    ci_run_id: &peer.run_id,
                    reserve_handle: &job.reserve_handle,
                },
                base_usage,
                if existing.skipped {
                    CiPrelaunchParentExpectation::OptionalBeforeLaunch
                } else {
                    CiPrelaunchParentExpectation::Required
                },
                CiPrelaunchUnresolvedPolicy::Refuse,
            )
            .await?;
        let usage =
            checked_accounting_usage(existing.usage).map_err(CiRunSupersessionError::Usage)?;
        let contains_floor = usage.cpu_seconds >= accounted_floor.cpu_seconds
            && usage.mem_byte_seconds >= accounted_floor.mem_byte_seconds;
        let legacy_superseded_receipt = superseded_receipt(
            &self.scope,
            &peer.run_id,
            &peer.wf_run_id,
            &job.job_id,
            &job.reserve_handle,
            usage,
            existing.skipped,
        );
        let superseded_disposition = if existing.skipped {
            if prelaunch.parent_attempts == 0 {
                CiJobTerminalDisposition::SkippedBeforeStart
            } else {
                CiJobTerminalDisposition::CancelledDuringPreparation
            }
        } else {
            CiJobTerminalDisposition::CancelledAfterWorkloadLaunch
        };
        let exact_superseded = matches!(replay_kind, ReplayKind::Superseded)
            && usage == accounted_floor
            && match existing.disposition {
                None => existing.completion_receipt == legacy_superseded_receipt,
                Some(disposition) => {
                    disposition == superseded_disposition
                        && existing.completion_receipt
                            == disposition_receipt_v4(
                                &legacy_superseded_receipt,
                                superseded_disposition,
                            )
                }
            };
        match replay_kind {
            ReplayKind::Reported if !contains_floor => {
                return Err(CiRunSupersessionError::CorruptState(
                    "accounting usage omits durable prelaunch usage",
                ));
            }
            ReplayKind::Superseded if !exact_superseded => {
                return Err(CiRunSupersessionError::CorruptState(
                    "accounting receipt disagrees with queue lifecycle",
                ));
            }
            _ => {}
        }
        let priced = TierPOperationalCiJobPricer
            .price(usage)
            .map_err(|_| CiRunSupersessionError::Pricing)?;
        validate_reservation_pricing_policy(&job.reserve_handle, usage, &priced)
            .map_err(|_| CiRunSupersessionError::Pricing)?;
        if existing.pricing_revision != priced.pricing_revision {
            return Err(CiRunSupersessionError::CorruptState(
                "accounting pricing revision diverges from immutable policy",
            ));
        }
        let rows = priced_cost_rows(&self.tenant, &peer.run_id, &job.job_id, usage, &priced)
            .map_err(|_| CiRunSupersessionError::Pricing)?;
        let units: Vec<MeteredUnit> = rows
            .iter()
            .map(|row| MeteredUnit {
                unit: row.meter.token(),
                wholesale: row.wholesale,
                markup: row.markup,
            })
            .collect();
        let settlement = self
            .ledger
            .settle_in_tx(
                conn,
                &self.tenant,
                &CostRunId(job.reserve_handle.clone()),
                &units,
            )
            .await
            .map_err(|_| CiRunSupersessionError::Settlement)?;
        if existing.billed != settlement.billed_total || existing.refunded != settlement.refunded {
            return Err(CiRunSupersessionError::CorruptState(
                "accounting monetary outcome diverges from Storage settlement",
            ));
        }
        self.costs
            .verify_exact_job_in_tx(conn, &self.scope, &rows)
            .await
            .map_err(|_| CiRunSupersessionError::Accounting)?;
        Ok(())
    }

    async fn settle_cancelled_job(
        &self,
        conn: &mut sqlx::PgConnection,
        manifest: &crate::CiDriveManifestV1,
        job: &crate::GrantedCiJobV1,
        base_usage: ResourceUsage,
        skipped: bool,
    ) -> Result<(), CiRunSupersessionError> {
        if !job
            .reserve_handle
            .starts_with(TIER_P_OPERATIONAL_RESERVATION_PREFIX)
            && !job
                .reserve_handle
                .starts_with(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX)
        {
            return Err(CiRunSupersessionError::Settlement);
        }
        let (usage, prelaunch) = self
            .resolve_job_usage_on_conn(
                conn,
                CiPrelaunchSettlementIdentity {
                    tenant_id: self.tenant.as_str(),
                    region: self.region.as_str(),
                    job_id: &job.job_id,
                    wf_run_id: &manifest.wf_run_id,
                    ci_run_id: &manifest.ci_run_id,
                    reserve_handle: &job.reserve_handle,
                },
                base_usage,
                if skipped {
                    CiPrelaunchParentExpectation::OptionalBeforeLaunch
                } else {
                    CiPrelaunchParentExpectation::Required
                },
                CiPrelaunchUnresolvedPolicy::SealToCeiling,
            )
            .await?;
        let disposition = if skipped {
            if prelaunch.parent_attempts == 0 {
                CiJobTerminalDisposition::SkippedBeforeStart
            } else {
                CiJobTerminalDisposition::CancelledDuringPreparation
            }
        } else {
            CiJobTerminalDisposition::CancelledAfterWorkloadLaunch
        };
        let legacy_completion_receipt_v3 = superseded_receipt(
            &self.scope,
            &manifest.ci_run_id,
            &manifest.wf_run_id,
            &job.job_id,
            &job.reserve_handle,
            usage,
            skipped,
        );
        let priced = TierPOperationalCiJobPricer
            .price(usage)
            .map_err(|_| CiRunSupersessionError::Pricing)?;
        validate_reservation_pricing_policy(&job.reserve_handle, usage, &priced)
            .map_err(|_| CiRunSupersessionError::Pricing)?;
        let units = vec![
            MeteredUnit {
                unit: Meter::CpuSeconds.token(),
                wholesale: priced.cpu_wholesale,
                markup: priced.cpu_markup,
            },
            MeteredUnit {
                unit: Meter::MemGbSeconds.token(),
                wholesale: priced.memory_wholesale,
                markup: priced.memory_markup,
            },
        ];
        let settled = self
            .ledger
            .settle_in_tx(
                conn,
                &self.tenant,
                &CostRunId(job.reserve_handle.clone()),
                &units,
            )
            .await
            .map_err(|_| CiRunSupersessionError::Settlement)?;
        let rows = priced_cost_rows(
            &self.tenant,
            &manifest.ci_run_id,
            &job.job_id,
            usage,
            &priced,
        )
        .map_err(|_| CiRunSupersessionError::Pricing)?;
        self.costs
            .settle_in_tx(conn, &self.scope, &rows)
            .await
            .map_err(|_| CiRunSupersessionError::Accounting)?;
        let receipt = versioned_accounting_receipt(
            self.accounting.write_version(),
            legacy_completion_receipt_v3,
            disposition,
        );
        self.accounting
            .record_in_tx(
                conn,
                &self.scope,
                &CiJobAccountingRecord {
                    tenant: self.tenant.clone(),
                    job_id: job.job_id.clone(),
                    wf_run_id: manifest.wf_run_id.clone(),
                    ci_run_id: manifest.ci_run_id.clone(),
                    reserve_handle: job.reserve_handle.clone(),
                    passed: false,
                    timed_out: false,
                    skipped,
                    usage,
                    pricing_revision: TIER_P_OPERATIONAL_PRICING_REVISION.into(),
                    billed: settled.billed_total,
                    refunded: settled.refunded,
                    disposition: receipt.disposition,
                    completion_receipt: receipt.completion_receipt,
                    legacy_completion_receipt_v3: receipt.legacy_completion_receipt_v3,
                },
            )
            .await
            .map_err(|_| CiRunSupersessionError::Accounting)?;
        settle_cancelled_ci_job_surface_on_conn(
            conn,
            &self.tenant,
            &self.region,
            &manifest.ci_run_id,
            &job.job_id,
            disposition,
        )
        .await
        .map_err(|_| CiRunSupersessionError::Accounting)?;
        Ok(())
    }

    async fn resolve_job_usage_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        identity: CiPrelaunchSettlementIdentity<'_>,
        base_usage: ResourceUsage,
        parent_expectation: CiPrelaunchParentExpectation,
        unresolved_policy: CiPrelaunchUnresolvedPolicy,
    ) -> Result<(ResourceUsage, CiPrelaunchUsageAccrual), CiRunSupersessionError> {
        let prelaunch =
            resolve_prelaunch_usage_on_conn(conn, identity, parent_expectation, unresolved_policy)
                .await
                .map_err(CiRunSupersessionError::Prelaunch)?;
        let usage = checked_add_accounting_usage(base_usage, prelaunch.usage)
            .map_err(CiRunSupersessionError::Usage)?;
        Ok((usage, prelaunch))
    }

    pub async fn reconcile_abandoned_job(
        &self,
        wf_run_id: &str,
        job_id: &str,
    ) -> Result<bool, CiRunSupersessionError> {
        let wf_run_id = wf_run_id.to_owned();
        let job_id = job_id.to_owned();
        let authority = self.clone();
        myelin_storage::with_tenant_tx_error(
            &self.pool,
            &self.tenant.0,
            &self.region.0,
            move |conn| {
                Box::pin(async move {
                    authority
                        .reconcile_abandoned_job_on_conn(conn, &wf_run_id, &job_id)
                        .await
                })
            },
        )
        .await
    }

    pub async fn reconcile_timed_out_prelaunch_job(
        &self,
        wf_run_id: &str,
        job_id: &str,
    ) -> Result<bool, CiRunSupersessionError> {
        let wf_run_id = wf_run_id.to_owned();
        let job_id = job_id.to_owned();
        let authority = self.clone();
        myelin_storage::with_tenant_tx_error(
            &self.pool,
            &self.tenant.0,
            &self.region.0,
            move |conn| {
                Box::pin(async move {
                    authority
                        .reconcile_timed_out_prelaunch_job_on_conn(conn, &wf_run_id, &job_id)
                        .await
                })
            },
        )
        .await
    }

    async fn reconcile_timed_out_prelaunch_job_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        wf_run_id: &str,
        job_id: &str,
    ) -> Result<bool, CiRunSupersessionError> {
        let job_uuid = sqlx::types::Uuid::parse_str(job_id)
            .map_err(|_| CiRunSupersessionError::CorruptState("prelaunch job id is invalid"))?;
        let lifecycle = sqlx::query(
            "SELECT w.state AS workflow_state, c.state AS ci_state
             FROM workflow_run w
             JOIN ci_run c
               ON c.tenant_id = w.tenant_id AND c.region = w.region
              AND c.wf_run_id = w.run_id::uuid
             WHERE w.tenant_id = $1 AND w.region = $2 AND w.run_id = $3
             FOR UPDATE OF w, c",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(wf_run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("lock timed-out prelaunch workflow"))?;
        let Some(lifecycle) = lifecycle else {
            return Ok(false);
        };
        if lifecycle.get::<String, _>("workflow_state") != "waiting"
            || lifecycle.get::<String, _>("ci_state") != "running"
        {
            return Ok(false);
        }

        let queue = sqlx::query(
            "SELECT idem_token, stage, retry_attempts
             FROM job_queue
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3
               AND run_id = $4::uuid AND state IN ('queued', 'leased')
             FOR UPDATE",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(job_uuid)
        .bind(wf_run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("lock timed-out prelaunch job"))?;
        let Some(queue) = queue else {
            return Ok(false);
        };
        let idem_token: String = queue.get("idem_token");
        let stage: String =
            queue
                .get::<Option<String>, _>("stage")
                .ok_or(CiRunSupersessionError::CorruptState(
                    "prelaunch job has no concrete stage",
                ))?;
        let wait_idem = format!("myelin://flow/wait-idem/{idem_token}");
        let late_accounting_wait: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1
               FROM wf_history h
               WHERE h.tenant_id = $1 AND h.region = $2 AND h.run_id = $3
                 AND h.kind = 'signal_waited'
                 AND h.result @> jsonb_build_array($4::text)
                 AND h.result @> '[\"myelin://flow/wait-name/job.done\"]'::jsonb
                 AND NOT EXISTS (
                   SELECT 1 FROM jsonb_array_elements_text(h.result) marker(value)
                   WHERE marker.value LIKE 'myelin://flow/wait-deadline/%'
                 )
             ) AND EXISTS (
               SELECT 1
               FROM wf_history timeout
               WHERE timeout.tenant_id = $1 AND timeout.region = $2 AND timeout.run_id = $3
                 AND timeout.kind = 'signal_received'
                 AND timeout.result @> jsonb_build_array(
                       'myelin://flow/signal-idem/wait:timeout'::text,
                       'myelin://flow/signal-name/job.done'::text
                     )
             )",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(wf_run_id)
        .bind(wait_idem)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("verify late accounting wait"))?;
        if !late_accounting_wait {
            return Ok(false);
        }

        let retry_usage =
            decode_retry_attempt_usage(queue.get("retry_attempts")).map_err(|_| {
                CiRunSupersessionError::CorruptState("prelaunch job retry accrual is malformed")
            })?;
        let (manifest, _) = self
            .manifest
            .load_by_wf_run_on_conn(conn, wf_run_id)
            .await
            .map_err(|_| CiRunSupersessionError::Manifest)?
            .ok_or(CiRunSupersessionError::Manifest)?;
        let job = manifest
            .jobs
            .iter()
            .find(|job| job.job_id == job_id)
            .ok_or(CiRunSupersessionError::CorruptState(
                "prelaunch job is absent from its immutable manifest",
            ))?;
        if job.name != stage {
            return Err(CiRunSupersessionError::CorruptState(
                "prelaunch stage disagrees with its immutable manifest",
            ));
        }
        let updated = sqlx::query(
            "UPDATE job_queue
             SET state = 'terminal', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3
               AND run_id = $4::uuid AND state IN ('queued', 'leased')",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(job_uuid)
        .bind(wf_run_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("terminalize timed-out prelaunch job"))?;
        if updated.rows_affected() != 1 {
            return Err(CiRunSupersessionError::CorruptState(
                "timed-out prelaunch job lost its locked generation",
            ));
        }
        self.settle_cancelled_job(
            conn,
            &manifest,
            job,
            retry_usage.unwrap_or(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
            retry_usage.is_none(),
        )
        .await?;
        self.executor
            .signal_typed_on_conn(
                conn,
                TypedSignalSpec {
                    run: RunId(wf_run_id.to_owned()),
                    signal_name: JOB_DONE_SIGNAL.into(),
                    idem_key: idem_token,
                    payload: SignalPayload::CiJobDone {
                        stage,
                        passed: false,
                        result_refs: Vec::new(),
                    },
                    payload_key_ref: None,
                },
            )
            .await
            .map_err(|_| CiRunSupersessionError::Workflow)?;
        Ok(true)
    }

    async fn reconcile_abandoned_job_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        wf_run_id: &str,
        job_id: &str,
    ) -> Result<bool, CiRunSupersessionError> {
        let job_uuid = sqlx::types::Uuid::parse_str(job_id)
            .map_err(|_| CiRunSupersessionError::CorruptState("abandoned job id is invalid"))?;
        let flow_state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM workflow_run
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3 FOR UPDATE",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(wf_run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("lock abandoned workflow"))?;
        if flow_state.as_deref() != Some("terminated") {
            return Ok(false);
        }
        let ci_state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM ci_run
             WHERE tenant_id = $1 AND region = $2 AND wf_run_id = $3::uuid",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(wf_run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("read abandoned CI run"))?;
        if ci_state.as_deref() != Some("cancelled") {
            return Ok(false);
        }
        let retry_attempts: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT retry_attempts FROM job_queue
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3
               AND run_id = $4::uuid AND state = 'running' AND lease_expires < now()
               AND pg_try_advisory_xact_lock(
                 hashtextextended(
                   jsonb_build_array(
                     tenant_id::text, region::text, job_id::text,
                     lease_epoch::text, claim_nonce::text
                   )::text,
                   0
                 )
               )
             FOR UPDATE",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(job_uuid)
        .bind(wf_run_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("lock abandoned queue generation"))?;
        let Some(retry_attempts) = retry_attempts else {
            return Ok(false);
        };
        let prior_usage = decode_retry_attempt_usage(retry_attempts).map_err(|_| {
            CiRunSupersessionError::CorruptState("abandoned job retry accrual is malformed")
        })?;
        let (manifest, _) = self
            .manifest
            .load_by_wf_run_on_conn(conn, wf_run_id)
            .await
            .map_err(|_| CiRunSupersessionError::Manifest)?
            .ok_or(CiRunSupersessionError::Manifest)?;
        let job = manifest
            .jobs
            .iter()
            .find(|job| job.job_id == job_id)
            .ok_or(CiRunSupersessionError::CorruptState(
                "abandoned job is absent from its immutable manifest",
            ))?;
        let timeout = u64::from(job.limits.timeout_secs);
        let cpu_millis = u64::from(job.limits.cpu_millis);
        let attempt_usage = ResourceUsage {
            cpu_seconds: cpu_millis
                .checked_mul(timeout)
                .and_then(|value| value.checked_add(999))
                .map(|value| value / 1_000)
                .ok_or(CiRunSupersessionError::Pricing)?,
            mem_byte_seconds: job
                .limits
                .mem_bytes
                .checked_mul(timeout)
                .ok_or(CiRunSupersessionError::Pricing)?,
        };
        let usage = checked_add_accounting_usage(
            attempt_usage,
            prior_usage.unwrap_or(ResourceUsage {
                cpu_seconds: 0,
                mem_byte_seconds: 0,
            }),
        )
        .map_err(CiRunSupersessionError::Usage)?;
        let updated = sqlx::query(
            "UPDATE job_queue
             SET state = 'terminal', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3
               AND run_id = $4::uuid AND state = 'running'",
        )
        .bind(&self.tenant.0)
        .bind(&self.region.0)
        .bind(job_uuid)
        .bind(wf_run_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| CiRunSupersessionError::Database("terminalize abandoned job"))?;
        if updated.rows_affected() != 1 {
            return Err(CiRunSupersessionError::CorruptState(
                "abandoned job lost its locked queue generation",
            ));
        }
        self.settle_cancelled_job(conn, &manifest, job, usage, false)
            .await?;
        let accounting = DurableCiJobAccounting::new(
            self.scope.clone(),
            self.manifest.clone(),
            self.ledger.clone(),
            self.costs.clone(),
            self.accounting.clone(),
            Arc::new(TierPOperationalCiJobPricer),
        );
        close_cancelled_run_if_accounted(conn, &accounting, wf_run_id)
            .await
            .map_err(|_| CiRunSupersessionError::Accounting)?;
        Ok(true)
    }
}

impl From<myelin_storage::PgError> for CiRunSupersessionError {
    fn from(_: myelin_storage::PgError) -> Self {
        Self::Database("tenant-scoped transaction")
    }
}

pub(crate) async fn emit_settled_cancelled_checks_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    manifest: &crate::CiDriveManifestV1,
) -> Result<(), CiRunSupersessionError> {
    emit_cancelled_manifest_facts_on_conn(conn, tenant, region, manifest, true, false).await
}

async fn emit_stale_run_cancelled_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    run_id: &str,
) -> Result<(), CiRunSupersessionError> {
    let provenance = cancellation_provenance(conn, tenant, region, run_id).await?;
    let run_ref = format!("myelin://{}/ci/run/{run_id}", tenant.0);
    let draft = EventDraft {
        type_: EventType(myelin_ci_sandbox::events::CI_RUN_CANCELLED.into()),
        subject: ArtifactRef(run_ref.clone()),
        aggregate: AggregateKey(format!("ci/run/{run_ref}")),
        payload: serde_json::json!({
            "run": run_ref,
            "repo_ref": &provenance.repo_ref,
            "reason": SUPERSEDED_REASON,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    co_commit_fact(
        conn,
        tenant,
        region,
        &provenance,
        supersession_event_id(tenant, region, run_id, "run-cancelled", None),
        draft,
    )
    .await
}

async fn emit_cancelled_manifest_facts_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    manifest: &crate::CiDriveManifestV1,
    cost_settled: bool,
    include_terminal_lifecycle: bool,
) -> Result<(), CiRunSupersessionError> {
    if manifest.tenant_id != tenant.0 || manifest.region != region.0 {
        return Err(CiRunSupersessionError::CorruptState(
            "cancelled fact manifest crosses its transaction scope",
        ));
    }
    let provenance = cancellation_provenance(conn, tenant, region, &manifest.ci_run_id).await?;
    let cost = if cost_settled {
        crate::check_emitter::CostPosture::Settled
    } else {
        crate::check_emitter::CostPosture::Unsettled
    };
    for (context, attempt) in &manifest.check_attempts {
        let required = manifest
            .merge_waiter
            .as_ref()
            .is_none_or(|waiter| waiter.required_contexts.binary_search(context).is_ok());
        let emit_context = crate::check_emitter::CheckEmitContext {
            tenant: manifest.tenant_id.clone(),
            repo: manifest.repo_ref.clone(),
            commit_oid: manifest.commit_oid.clone(),
            run_ref: manifest.run_ref.clone(),
            run_attempt: *attempt,
            trust_tier: manifest_check_trust_tier(manifest.trust_tier),
            started_at: manifest.started_at.clone(),
            completed_at: Some(provenance.finished_at.clone()),
        };
        let status = crate::check_emitter::CheckStatusUpdate::new(
            crate::check_emitter::CheckProvider::Ci,
            context,
            crate::check_emitter::CheckState::Cancelled,
            required,
        )
        .with_cost(cost);
        let draft = crate::check_emitter::assemble_check_status(&emit_context, &status);
        co_commit_fact(
            conn,
            tenant,
            region,
            &provenance,
            supersession_event_id(
                tenant,
                region,
                &manifest.ci_run_id,
                if cost_settled {
                    "check-cancelled-settled"
                } else {
                    "check-cancelled-unsettled"
                },
                Some(context),
            ),
            draft,
        )
        .await?;
    }
    if !include_terminal_lifecycle {
        return Ok(());
    }

    let run_draft = EventDraft {
        type_: EventType(myelin_ci_sandbox::events::CI_RUN_CANCELLED.into()),
        subject: ArtifactRef(manifest.run_ref.clone()),
        aggregate: AggregateKey(format!("ci/run/{}", manifest.run_ref)),
        payload: serde_json::json!({
            "run": manifest.run_ref,
            "repo_ref": manifest.repo_ref,
            "commit_oid": manifest.commit_oid,
            "reason": SUPERSEDED_REASON,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    co_commit_fact(
        conn,
        tenant,
        region,
        &provenance,
        supersession_event_id(tenant, region, &manifest.ci_run_id, "run-cancelled", None),
        run_draft,
    )
    .await?;

    if let Some(waiter) = &manifest.merge_waiter {
        let current = manifest
            .check_attempts
            .keys()
            .map(|context| (context.clone(), false))
            .collect();
        let result = rollup_ci_result(
            &manifest.commit_oid,
            &current,
            &waiter.required_contexts,
            &waiter.idem_token,
        );
        co_commit_fact(
            conn,
            tenant,
            region,
            &provenance,
            supersession_event_id(
                tenant,
                region,
                &manifest.ci_run_id,
                "ci-result-cancelled",
                None,
            ),
            ci_result_draft(&manifest.repo_ref, &result),
        )
        .await?;
    }
    Ok(())
}

async fn cancellation_provenance(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    run_id: &str,
) -> Result<CancellationProvenance, CiRunSupersessionError> {
    let row = sqlx::query(
        "SELECT repo_ref, cause_event_id, correlation_id, caused_by, cause_depth, \
                to_char(finished_at AT TIME ZONE 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS finished_at, \
                to_char(clock_timestamp() AT TIME ZONE 'UTC', \
                        'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS emitted_at \
         FROM ci_run \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
           AND state = 'cancelled' AND finished_at IS NOT NULL",
    )
    .bind(&tenant.0)
    .bind(&region.0)
    .bind(run_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CiRunSupersessionError::Database("read cancellation provenance"))?
    .ok_or(CiRunSupersessionError::CorruptState(
        "cancelled run lacks terminal provenance",
    ))?;
    let depth: i64 = row.get("cause_depth");
    Ok(CancellationProvenance {
        repo_ref: row.get::<Option<String>, _>("repo_ref").ok_or(
            CiRunSupersessionError::CorruptState("cancelled run lacks repository identity"),
        )?,
        cause_event_id: row.get::<Option<String>, _>("cause_event_id").ok_or(
            CiRunSupersessionError::CorruptState("cancelled run lacks triggering event identity"),
        )?,
        correlation_id: row.get("correlation_id"),
        caused_by: row.get("caused_by"),
        cause_depth: u32::try_from(depth).map_err(|_| {
            CiRunSupersessionError::CorruptState("cancelled run has invalid causal depth")
        })?,
        finished_at: row.get("finished_at"),
        emitted_at: row.get("emitted_at"),
    })
}

async fn co_commit_fact(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    provenance: &CancellationProvenance,
    event_id: EventId,
    draft: EventDraft,
) -> Result<(), CiRunSupersessionError> {
    let timestamp = Timestamp(provenance.emitted_at.clone());
    let cause = PersistedEventCause {
        event_id: EventId(provenance.cause_event_id.clone()),
        correlation_id: CorrelationId(provenance.correlation_id.clone()),
        caused_by: provenance.caused_by.clone().map(CausedBy),
        depth: provenance.cause_depth,
    };
    let envelope = derive_envelope_from_persisted_cause(
        draft,
        EmitContext {
            event_id,
            tenant: tenant.clone(),
            region: region.clone(),
            actor: Actor(Principal::stub(
                PrincipalId("ci-controlplane".into()),
                PrincipalKind::Service,
                tenant.clone(),
            )),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        Some(&cause),
    );
    let aggregate = envelope.aggregate.0.clone();
    PgRelay::co_commit_in_tx(conn, &aggregate, &envelope)
        .await
        .map_err(|_| CiRunSupersessionError::Database("co-commit cancellation fact"))
}

fn supersession_event_id(
    tenant: &TenantId,
    region: &Region,
    run_id: &str,
    kind: &str,
    context: Option<&str>,
) -> EventId {
    let mut hasher = blake3::Hasher::new_derive_key(EVENT_ID_DOMAIN);
    for field in [
        tenant.0.as_str(),
        region.0.as_str(),
        run_id,
        kind,
        context.unwrap_or(""),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    EventId(format!("ci-supersession-{}", hasher.finalize().to_hex()))
}

fn manifest_check_trust_tier(
    tier: crate::CiManifestTrustTierV1,
) -> crate::check_emitter::TrustTier {
    match tier {
        crate::CiManifestTrustTierV1::Trusted | crate::CiManifestTrustTierV1::SelfHosted => {
            crate::check_emitter::TrustTier::Trusted
        }
        crate::CiManifestTrustTierV1::UntrustedFork => {
            crate::check_emitter::TrustTier::UntrustedFork
        }
    }
}

pub(crate) fn superseded_receipt(
    scope: &TenantScope,
    ci_run_id: &str,
    wf_run_id: &str,
    job_id: &str,
    reserve_handle: &str,
    usage: ResourceUsage,
    skipped: bool,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.ci.superseded-accounting.v2\0");
    for field in [
        scope.tenant().as_str(),
        scope.region().as_str(),
        ci_run_id,
        wf_run_id,
        job_id,
        reserve_handle,
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.update(&usage.cpu_seconds.to_be_bytes());
    hasher.update(&usage.mem_byte_seconds.to_be_bytes());
    hasher.update(&[u8::from(skipped)]);
    format!("v3:{}", hasher.finalize().to_hex())
}

fn valid_machine_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}
