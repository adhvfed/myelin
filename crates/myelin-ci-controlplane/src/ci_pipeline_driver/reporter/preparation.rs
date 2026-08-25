use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, JobKind, PreparationPhase, PreparationTerminalDisposition,
    ResourceUsage, TerminalReport,
};
use myelin_flow::{
    ExecutorError, RunId, SignalOutcome, SignalPayload, TypedSignalSpec, JOB_DONE_SIGNAL,
};
use myelin_storage::with_tenant_tx_error;
use sqlx::types::Uuid;

use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_prelaunch_usage_journal::{
    CiPrelaunchParentExpectation, CiPrelaunchUnresolvedPolicy,
};
use crate::job_accounting_store::{CiJobAccountingWriteVersion, CiJobTerminalDisposition};
use crate::job_queue_store::{
    CiJobQueueStore, ClaimConsumeOutcome, PreparationClaimConsumeSpec, PreparationRequeueOutcome,
    PreparationRequeueSpec,
};

use super::{CiPipelineReporter, PreparationRetryOutcome};
use crate::ci_pipeline_driver::accounting::ReporterAccounting;
use crate::ci_pipeline_driver::completion::{
    preparation_completion_receipts, verify_claimed_identity, PreparationCompletionReceiptInput,
};
use crate::ci_pipeline_driver::retry::{aggregate_usage, retry_attempts_for_terminal_on_conn};
use crate::ci_pipeline_driver::{
    bridge, close_cancelled_run_if_accounted, co_commit_terminal_accounting,
    resolve_terminal_usage_on_conn, verify_preparation_disposition_on_conn,
    verify_preparation_retry_permitted_on_conn, CompletionTxError, PreparationRetryGate,
    TerminalAccountingInput, TerminalUsageResolutionInput,
};

impl CiPipelineReporter {
    pub fn report_preparation_terminal(
        &self,
        claim: &CiJobTokenRequest,
        disposition: PreparationTerminalDisposition,
    ) -> Result<SignalOutcome, ExecutorError> {
        self.report_preparation_terminal_with_diagnostic(claim, disposition, None)
    }

    pub fn report_preparation_terminal_with_diagnostic(
        &self,
        claim: &CiJobTokenRequest,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        claim.validate().map_err(|error| {
            ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation completion refused: {}",
                error.0
            ))
        })?;
        if claim.tenant_id != self.tenant.0 || claim.region != self.region {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation completion refused: reporter scope mismatch".into(),
            ));
        }
        let accounting = match &self.accounting {
            ReporterAccounting::Durable(accounting)
                if accounting.receipt_store.write_version() == CiJobAccountingWriteVersion::V4 =>
            {
                accounting.clone()
            }
            ReporterAccounting::Durable(_) => {
                return Err(ExecutorError::InvalidInput(
                    "ci.pipeline preparation completion refused: v4 accounting writer is not \
                     activated"
                        .into(),
                ))
            }
            #[cfg(any(test, feature = "test-support"))]
            ReporterAccounting::TestBypass => {
                return Err(ExecutorError::InvalidInput(
                    "ci.pipeline preparation completion requires durable v4 accounting".into(),
                ))
            }
        };

        let job_uuid = Uuid::parse_str(&claim.job_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid job UUID".into()))?;
        let wf_run_uuid = Uuid::parse_str(&claim.wf_run_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid workflow run UUID".into()))?;
        let ci_run_uuid = Uuid::parse_str(&claim.ci_run_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid CI run UUID".into()))?;
        let nonce_uuid = Uuid::parse_str(&claim.claim_nonce)
            .map_err(|_| ExecutorError::InvalidInput("invalid claim nonce UUID".into()))?;
        let claim = claim.clone();
        let diagnostic = diagnostic.map(str::to_owned);
        let refusal_job = claim.job_id.clone();
        let refusal_owner = claim.lease_owner.clone();
        let refusal_epoch = claim.lease_epoch;
        let tenant = self.tenant.clone();
        let region = self.region.clone();
        let transaction_tenant = tenant.0.clone();
        let transaction_region = region.clone();
        let spec_store = self.spec_store.clone();
        let pg_executor = self.pg_executor.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &transaction_tenant,
                &transaction_region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                tenant.as_str(),
                                job_uuid,
                                &claim.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &tenant,
                            &tenant,
                            &claim.wf_run_id,
                            &claim.job_id,
                            &claim.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let launch = spec_store
                            .get_launch_template_on_conn(conn, tenant.as_str(), &claim.job_id)
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let checkout_scope = derive_checkout_authorization_scope(
                            JobKind::Ci,
                            &launch.spec.workspace,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        if launch.ci_run_id != claim.ci_run_id
                            || launch.token_authority_handle != claim.token_authority_handle
                            || launch.spec.idem_token.0 != claim.idem_token
                            || launch.spec.meter_to.reserve_id != reserve_handle
                            || (checkout_scope.is_none()
                                && !matches!(
                                    disposition,
                                    PreparationTerminalDisposition::Failed {
                                        phase: PreparationPhase::SecretResolution
                                    }
                                ))
                        {
                            return Err(CompletionTxError::Refused);
                        }

                        let signal = TypedSignalSpec {
                            run: RunId(claim.wf_run_id.clone()),
                            signal_name: JOB_DONE_SIGNAL.to_string(),
                            idem_key: claim.idem_token.clone(),
                            payload: SignalPayload::CiJobDone {
                                stage: stage.clone(),
                                passed: false,
                                result_refs: Vec::new(),
                            },
                            payload_key_ref: None,
                        };
                        let signal_outcome = pg_executor
                            .signal_typed_on_conn(conn, signal)
                            .await
                            .map_err(CompletionTxError::Signal)?;
                        let attempts =
                            retry_attempts_for_terminal_on_conn(conn, &tenant, &region, job_uuid)
                                .await?;
                        let base_usage = aggregate_usage(
                            attempts.as_ref(),
                            ResourceUsage {
                                cpu_seconds: 0,
                                mem_byte_seconds: 0,
                            },
                        )?;
                        let (ci_run_id, usage) = resolve_terminal_usage_on_conn(
                            conn,
                            &accounting,
                            TerminalUsageResolutionInput {
                                tenant: &tenant,
                                wf_run_id: &claim.wf_run_id,
                                job_id: &claim.job_id,
                                reserve_handle: &reserve_handle,
                                base_usage,
                                parent_expectation: if matches!(
                                    disposition,
                                    PreparationTerminalDisposition::Failed {
                                        phase: PreparationPhase::SecretResolution
                                    }
                                ) {
                                    CiPrelaunchParentExpectation::OptionalBeforeLaunch
                                } else {
                                    CiPrelaunchParentExpectation::Required
                                },
                                unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                            },
                        )
                        .await?;
                        if ci_run_id != claim.ci_run_id {
                            return Err(CompletionTxError::Refused);
                        }
                        verify_preparation_disposition_on_conn(
                            conn,
                            &claim,
                            &reserve_handle,
                            disposition,
                        )
                        .await?;
                        let receipts = preparation_completion_receipts(
                            PreparationCompletionReceiptInput {
                                tenant: &tenant,
                                region: &region,
                                wf_run_id: &claim.wf_run_id,
                                ci_run_id: &claim.ci_run_id,
                                job_id: &claim.job_id,
                                idem_token: &claim.idem_token,
                                stage: &stage,
                                reserve_handle: &reserve_handle,
                                usage,
                                lease_owner: &claim.lease_owner,
                                lease_epoch: claim.lease_epoch,
                                claim_nonce: &claim.claim_nonce,
                                claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                                claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                            },
                            disposition,
                        );
                        let consume_spec = PreparationClaimConsumeSpec {
                            tenant_id: tenant.as_str(),
                            region: &region,
                            job_id: job_uuid,
                            wf_run_id: wf_run_uuid,
                            ci_run_id: ci_run_uuid,
                            idem_token: &claim.idem_token,
                            lease_owner: &claim.lease_owner,
                            lease_epoch: claim.lease_epoch,
                            claim_nonce: nonce_uuid,
                            stage: &stage,
                            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                            reserve_handle: &reserve_handle,
                            completion_receipt: &receipts.current_v4,
                        };
                        let claim_outcome = match disposition {
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::SecretResolution,
                            } => {
                                CiJobQueueStore::consume_secret_withheld_claim_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                            PreparationTerminalDisposition::AttemptsExhausted => {
                                CiJobQueueStore::consume_preparation_claim_exhausted_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                            PreparationTerminalDisposition::Failed { .. }
                            | PreparationTerminalDisposition::TimedOut { .. } => {
                                CiJobQueueStore::consume_preparation_claim_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                        };
                        if claim_outcome == ClaimConsumeOutcome::Refused {
                            return Err(CompletionTxError::Refused);
                        }
                        let report = TerminalReport {
                            passed: false,
                            timed_out: matches!(
                                disposition,
                                PreparationTerminalDisposition::TimedOut { .. }
                            ),
                            usage,
                            result_refs: Vec::new(),
                        };
                        co_commit_terminal_accounting(
                            conn,
                            &accounting,
                            TerminalAccountingInput {
                                tenant: &tenant,
                                wf_run: &RunId(claim.wf_run_id.clone()),
                                ci_run_id: &claim.ci_run_id,
                                job_id: &claim.job_id,
                                reserve_handle: &reserve_handle,
                                report: &report,
                                receipts: &receipts,
                                disposition: CiJobTerminalDisposition::Preparation(disposition),
                                diagnostic: diagnostic.as_deref(),
                                replay: claim_outcome == ClaimConsumeOutcome::AlreadyConsumed,
                            },
                        )
                        .await?;
                        close_cancelled_run_if_accounted(conn, &accounting, &claim.wf_run_id)
                            .await?;
                        Ok(signal_outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(outcome) => Ok(outcome),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation completion refused (stale or divergent generation): job \
                 `{}` owner `{}` epoch `{}`",
                refusal_job, refusal_owner, refusal_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable preparation dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Signal(error)) => Err(error),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "preparation usage resolution refused: {error}"
            ))),
            Err(CompletionTxError::Usage(error)) => Err(ExecutorError::Storage(format!(
                "preparation usage aggregation refused: {error}"
            ))),
            Err(CompletionTxError::Pricing(error)) => Err(ExecutorError::Storage(format!(
                "preparation accounting pricing refused: {error}"
            ))),
            Err(CompletionTxError::Money(error)) => Err(ExecutorError::Storage(format!(
                "preparation money settlement refused: {error}"
            ))),
            Err(CompletionTxError::Projection(error)) => Err(ExecutorError::Storage(format!(
                "preparation cost projection refused: {error}"
            ))),
            Err(CompletionTxError::Accounting(error)) => Err(ExecutorError::Storage(format!(
                "preparation accounting receipt refused: {error}"
            ))),
            Err(error) => Err(ExecutorError::Storage(format!(
                "preparation completion transaction failed: {error:?}"
            ))),
        }
    }

    pub fn report_preparation_retry(
        &self,
        claim: &CiJobTokenRequest,
    ) -> Result<PreparationRetryOutcome, ExecutorError> {
        claim.validate().map_err(|error| {
            ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation retry refused: {}",
                error.0
            ))
        })?;
        if claim.tenant_id != self.tenant.0 || claim.region != self.region {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation retry refused: reporter scope mismatch".into(),
            ));
        }
        let job_uuid = Uuid::parse_str(&claim.job_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid job UUID".into()))?;
        let claim = claim.clone();
        let refusal_job = claim.job_id.clone();
        let refusal_owner = claim.lease_owner.clone();
        let refusal_epoch = claim.lease_epoch;
        let tenant = self.tenant.clone();
        let region = self.region.clone();
        let transaction_tenant = tenant.0.clone();
        let transaction_region = region.clone();
        let spec_store = self.spec_store.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &transaction_tenant,
                &transaction_region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                tenant.as_str(),
                                job_uuid,
                                &claim.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &tenant,
                            &tenant,
                            &claim.wf_run_id,
                            &claim.job_id,
                            &claim.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let launch = spec_store
                            .get_launch_template_on_conn(conn, tenant.as_str(), &claim.job_id)
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        if launch.ci_run_id != claim.ci_run_id
                            || launch.token_authority_handle != claim.token_authority_handle
                            || launch.spec.idem_token.0 != claim.idem_token
                            || launch.spec.meter_to.reserve_id != reserve_handle
                            || derive_checkout_authorization_scope(
                                JobKind::Ci,
                                &launch.spec.workspace,
                            )
                            .map_err(|_| CompletionTxError::Refused)?
                            .is_none()
                        {
                            return Err(CompletionTxError::Refused);
                        }
                        if let PreparationRetryGate::NotLive =
                            verify_preparation_retry_permitted_on_conn(
                                conn,
                                &claim,
                                &reserve_handle,
                            )
                            .await?
                        {
                            return Ok(PreparationRequeueOutcome::NoOp);
                        }
                        let outcome = CiJobQueueStore::requeue_preparation_claim_on_conn(
                            conn,
                            PreparationRequeueSpec {
                                tenant_id: tenant.as_str(),
                                region: &region,
                                job_id: &claim.job_id,
                                wf_run_id: &claim.wf_run_id,
                                ci_run_id: &claim.ci_run_id,
                                idem_token: &claim.idem_token,
                                lease_owner: &claim.lease_owner,
                                lease_epoch: claim.lease_epoch,
                                claim_nonce: &claim.claim_nonce,
                                stage: &stage,
                                claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                                claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                                reserve_handle: &reserve_handle,
                            },
                        )
                        .await?;
                        Ok(outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(PreparationRequeueOutcome::Requeued) => Ok(PreparationRetryOutcome::Requeued),
            Ok(PreparationRequeueOutcome::NoOp) => Ok(PreparationRetryOutcome::NoOp),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation retry refused (stale or divergent generation): job `{}` \
                 owner `{}` epoch `{}`",
                refusal_job, refusal_owner, refusal_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable preparation dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "preparation retry verification refused: {error}"
            ))),
            Err(error) => Err(ExecutorError::Storage(format!(
                "preparation retry transaction failed: {error:?}"
            ))),
        }
    }
}
