use std::sync::Arc;

use myelin_ci_sandbox::checkout_orchestration::{
    AttemptAuthority, AttemptAuthorityError, ParentAttemptAdmission, PhaseCredentialCarrier,
    WorkloadCredentialCarrier,
};
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CheckoutAuthorizationScope, CheckoutPhase, HookError,
    JobSpec, PreparationLeaseCheckpoint, PreparationLeaseLost, PreparationPhase,
    PreparationReportClaim, ReserveHandle, ResourceUsage, RunTokenAuthorizationContext,
};

use crate::ci_credential_generation::{
    CiCredentialPurpose, CiJobCredentialGenerationStore, CiJobCredentialWriteVersion,
    CiPhaseCredentialMinter, MintedPhaseCredential,
};
use crate::ci_identity_adapter::ci_job_phase_authorization_context;
use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_prelaunch_usage_journal::{
    CiJobParentAttempt, CiParentAttemptAdmission, CiPrelaunchUsageJournal, CiPrelaunchUsagePhase,
};
use crate::job_queue_store::{CiJobLaunchClaim, CiJobQueueStore};
use crate::runner_bind::{bridge, DurablePreparationLeaseCheckpoint};

pub fn v2_phase_credential_store(
    pool: sqlx::PgPool,
    region: impl Into<String>,
    minter: Arc<dyn CiPhaseCredentialMinter>,
) -> CiJobCredentialGenerationStore {
    CiJobCredentialGenerationStore::with_pg_and_write_version(
        pool,
        region,
        minter,
        CiJobCredentialWriteVersion::V2PhaseBound,
    )
}

pub fn initial_phase_purpose(checkout: Option<&CheckoutAuthorizationScope>) -> CiCredentialPurpose {
    match checkout {
        Some(_) => CiCredentialPurpose::CheckoutAdvertise,
        None => CiCredentialPurpose::Workload,
    }
}

fn journal_phase(phase: PreparationPhase) -> CiPrelaunchUsagePhase {
    match phase {
        PreparationPhase::SecretResolution => {
            unreachable!("secret resolution is terminalized before checkout journaling")
        }
        PreparationPhase::CheckoutTransport => CiPrelaunchUsagePhase::CheckoutTransport,
        PreparationPhase::CheckoutMaterialization => CiPrelaunchUsagePhase::CheckoutMaterialization,
    }
}

fn phase_purpose(phase: CheckoutPhase) -> CiCredentialPurpose {
    match phase {
        CheckoutPhase::Advertise => CiCredentialPurpose::CheckoutAdvertise,
        CheckoutPhase::Fetch => CiCredentialPurpose::CheckoutFetch,
        CheckoutPhase::Materialization => CiCredentialPurpose::CheckoutMaterialization,
    }
}

fn reconstruct_claim(
    spec: &JobSpec,
) -> Result<(CiJobTokenRequest, Option<CheckoutAuthorizationScope>), HookError> {
    let context = match &spec.run_token_authorization {
        Some(RunTokenAuthorizationContext::CiJob(context)) => context,
        None => {
            return Err(HookError(
                "V2 parent-attempt admission requires a resolved CI-job authorization context"
                    .into(),
            ))
        }
    };
    let binding = context.credential_binding.as_ref().ok_or_else(|| {
        HookError(
            "V2 parent-attempt admission requires a V2 phase-credential binding (the reconstructed \
             claim's ci_run_id/token_authority_handle/idem_token live there); a legacy V1 context is \
             refused"
                .into(),
        )
    })?;
    let derived_scope =
        derive_checkout_authorization_scope(spec.kind, &spec.workspace).map_err(|reason| {
            HookError(format!(
                "deriving the spec's checkout scope failed: {reason}"
            ))
        })?;
    if derived_scope != context.checkout_scope {
        return Err(HookError(
            "V2 parent-attempt admission refused: the checkout scope derived from the spec's \
             workspace does not equal the resolved authorization context's scope (substitution)"
                .into(),
        ));
    }
    let expected_initial = initial_phase_purpose(derived_scope.as_ref());
    if CiCredentialPurpose::from_token(&binding.purpose) != Some(expected_initial) {
        return Err(HookError(format!(
            "V2 parent-attempt admission refused: the phase-credential binding purpose `{}` is not \
             this job shape's expected initial purpose `{}`",
            binding.purpose,
            expected_initial.token()
        )));
    }
    if binding.idem_token != spec.idem_token.0 {
        return Err(HookError(
            "V2 parent-attempt admission refused: the phase-credential binding's idem token does not \
             equal the dispatched spec's idem token"
                .into(),
        ));
    }
    let claim = CiJobTokenRequest {
        tenant_id: context.tenant_id.clone(),
        region: context.region.clone(),
        project_id: context.project_id.clone(),
        wf_run_id: context.wf_run_id.clone(),
        ci_run_id: binding.ci_run_id.clone(),
        job_id: context.job_id.clone(),
        token_authority_handle: binding.token_authority_handle.clone(),
        idem_token: binding.idem_token.clone(),
        lease_owner: context.lease_owner.clone(),
        lease_epoch: context.lease_epoch,
        claim_nonce: context.claim_nonce.clone(),
        claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
    };
    claim.validate().map_err(|error| {
        HookError(format!(
            "reconstructed V2 claim failed validation (the resolved context is malformed): {}",
            error.0
        ))
    })?;
    Ok((claim, derived_scope))
}

pub(crate) fn preparation_report_claim(claim: &CiJobTokenRequest) -> PreparationReportClaim {
    PreparationReportClaim {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        project_id: claim.project_id.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        ci_run_id: claim.ci_run_id.clone(),
        job_id: claim.job_id.clone(),
        token_authority_handle: claim.token_authority_handle.clone(),
        idem_token: claim.idem_token.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
    }
}

fn launch_claim(claim: &CiJobTokenRequest) -> CiJobLaunchClaim {
    CiJobLaunchClaim {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        job_id: claim.job_id.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
    }
}

#[derive(Clone)]
pub struct V2CheckoutComposition {
    journal: CiPrelaunchUsageJournal,
    credential_store: CiJobCredentialGenerationStore,
    queue_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
}

impl V2CheckoutComposition {
    pub fn new(
        pool: sqlx::PgPool,
        region: impl Into<String>,
        minter: Arc<dyn CiPhaseCredentialMinter>,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, HookError> {
        let region = region.into();
        let journal = CiPrelaunchUsageJournal::new(pool.clone(), region.clone())
            .map_err(|error| HookError(format!("V2 checkout composition refused: {error}")))?;
        let credential_store = v2_phase_credential_store(pool, region, minter);
        Ok(Self {
            journal,
            credential_store,
            queue_store,
            rt,
        })
    }

    pub fn mint_initial_phase_credential(
        &self,
        claim: &CiJobTokenRequest,
        reserve_id: &str,
        checkout: Option<&CheckoutAuthorizationScope>,
    ) -> Result<(MintedPhaseCredential, RunTokenAuthorizationContext), AttemptAuthorityError> {
        let purpose = initial_phase_purpose(checkout);
        let minted = bridge(
            &self.rt,
            self.credential_store
                .mint_phase_credential_for_checkout_scope(claim, purpose, checkout),
        )
        .map_err(|error| AttemptAuthorityError(error.to_string()))?;
        let context = ci_job_phase_authorization_context(
            claim,
            reserve_id,
            minted.checkout.as_ref(),
            &minted.binding,
        );
        Ok((minted, context))
    }

    pub fn parent_attempt_reserve_hook(&self) -> myelin_ci_sandbox::ParentAttemptReserveHook {
        let this = self.clone();
        Box::new(move |spec: &JobSpec| this.admit(spec))
    }

    fn admit(&self, spec: &JobSpec) -> Result<ParentAttemptAdmission, HookError> {
        let (claim, checkout) = reconstruct_claim(spec)?;
        let report_claim = preparation_report_claim(&claim);
        let reserve_handle = spec.meter_to.reserve_id.clone();
        match bridge(
            &self.rt,
            self.journal.admit_parent_attempt(&claim, &reserve_handle),
        )
        .map_err(|error| HookError(format!("V2 parent-attempt admission refused: {error}")))?
        {
            CiParentAttemptAdmission::Admitted { attempt, .. } => {
                let lease_checkpoint = DurablePreparationLeaseCheckpoint::new(
                    self.queue_store.clone(),
                    launch_claim(&claim),
                    self.rt.clone(),
                );
                let authority = DurableAttemptAuthority {
                    journal: self.journal.clone(),
                    credential_store: self.credential_store.clone(),
                    lease_checkpoint,
                    attempt,
                    claim,
                    checkout,
                    reserve_handle: reserve_handle.clone(),
                    rt: self.rt.clone(),
                };
                Ok(ParentAttemptAdmission::Admitted {
                    claim: report_claim,
                    reserve: ReserveHandle(reserve_handle),
                    attempt_authority: Box::new(authority),
                })
            }
            CiParentAttemptAdmission::AttemptsExhausted { reserve_handle } => {
                Ok(ParentAttemptAdmission::AttemptsExhausted {
                    claim: report_claim,
                    reserve: ReserveHandle(reserve_handle),
                })
            }
        }
    }
}

struct DurableAttemptAuthority {
    journal: CiPrelaunchUsageJournal,
    credential_store: CiJobCredentialGenerationStore,
    lease_checkpoint: DurablePreparationLeaseCheckpoint,
    attempt: CiJobParentAttempt,
    claim: CiJobTokenRequest,
    checkout: Option<CheckoutAuthorizationScope>,
    reserve_handle: String,
    rt: tokio::runtime::Handle,
}

impl DurableAttemptAuthority {
    fn mint(
        &self,
        purpose: CiCredentialPurpose,
    ) -> Result<
        (
            myelin_ci_sandbox::RunTokenCredential,
            RunTokenAuthorizationContext,
            String,
        ),
        AttemptAuthorityError,
    > {
        let minted = bridge(
            &self.rt,
            self.credential_store
                .mint_phase_credential_for_checkout_scope(
                    &self.claim,
                    purpose,
                    self.checkout.as_ref(),
                ),
        )
        .map_err(|error| AttemptAuthorityError(error.to_string()))?;
        let context = ci_job_phase_authorization_context(
            &self.claim,
            &self.reserve_handle,
            minted.checkout.as_ref(),
            &minted.binding,
        );
        Ok((minted.credential, context, minted.binding.generation_id))
    }
}

impl AttemptAuthority for DurableAttemptAuthority {
    fn begin_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
        bridge(
            &self.rt,
            self.journal
                .begin_phase(&self.attempt, journal_phase(phase)),
        )
        .map(|_outcome| ())
        .map_err(|error| AttemptAuthorityError(error.to_string()))
    }

    fn complete_phase(
        &self,
        phase: PreparationPhase,
        usage: ResourceUsage,
    ) -> Result<(), AttemptAuthorityError> {
        bridge(
            &self.rt,
            self.journal
                .complete_phase(&self.attempt, journal_phase(phase), usage),
        )
        .map(|_outcome| ())
        .map_err(|error| AttemptAuthorityError(error.to_string()))
    }

    fn seal_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
        bridge(
            &self.rt,
            self.journal.seal_phase(&self.attempt, journal_phase(phase)),
        )
        .map(|_outcome| ())
        .map_err(|error| AttemptAuthorityError(error.to_string()))
    }

    fn renew_preparation_lease(&self) -> Result<(), PreparationLeaseLost> {
        PreparationLeaseCheckpoint::renew(&self.lease_checkpoint)
    }

    fn mint_phase_credential(
        &self,
        phase: CheckoutPhase,
    ) -> Result<PhaseCredentialCarrier, AttemptAuthorityError> {
        let (credential, context, generation_id) = self.mint(phase_purpose(phase))?;
        Ok(PhaseCredentialCarrier::new(
            credential,
            context,
            generation_id,
        ))
    }

    fn mint_workload_credential(&self) -> Result<WorkloadCredentialCarrier, AttemptAuthorityError> {
        let (credential, context, generation_id) = self.mint(CiCredentialPurpose::Workload)?;
        Ok(WorkloadCredentialCarrier::new(
            credential,
            context,
            generation_id,
        ))
    }

    fn should_requeue(&self) -> bool {
        bridge(
            &self.rt,
            self.journal
                .parent_attempt_retry_permitted(&self.claim, &self.reserve_handle),
        )
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_ci_sandbox::{
        CiJobAuthorizationContext, CiJobCredentialBinding, EgressPolicy, IdemToken, ImageRef,
        JobKind, MeterTarget, ResourceLimits, RunTokenCredential, TrustTier, WorkspaceSpec,
    };

    fn checkout_workspace() -> WorkspaceSpec {
        WorkspaceSpec {
            repo_ref: Some("myelin://acme/git/repo/widgets".into()),
            commit: Some("a".repeat(40)),
        }
    }

    fn job_spec(
        workspace: WorkspaceSpec,
        context: Option<RunTokenAuthorizationContext>,
    ) -> JobSpec {
        let mut spec = JobSpec::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.invalid/ci@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["true".into()],
            Vec::new(),
            Vec::new(),
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 1024 * 1024 * 1024,
                pids_max: 128,
                timeout_secs: 30,
            },
            workspace,
            TrustTier::Trusted,
            RunTokenCredential::new("bearer", "advertise-jti", 300).unwrap(),
            MeterTarget {
                reserve_id: "ci-reserve:v2:reserve-1".into(),
            },
            IdemToken("11111111-1111-1111-1111-111111111111/build".into()),
        )
        .unwrap();
        spec.run_token_authorization = context;
        spec
    }

    fn checkout_scope() -> CheckoutAuthorizationScope {
        derive_checkout_authorization_scope(JobKind::Ci, &checkout_workspace())
            .expect("scope derives")
            .expect("the checkout workspace is checkout-bearing")
    }

    fn v2_context() -> CiJobAuthorizationContext {
        CiJobAuthorizationContext {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            principal_id: "ci-job".into(),
            project_id: "11111111-1111-4111-8111-111111111111".into(),
            wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
            job_id: "22222222-2222-2222-2222-222222222222".into(),
            lease_owner: "worker-1".into(),
            lease_epoch: 7,
            claim_nonce: "33333333-3333-3333-3333-333333333333".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
            reserve_id: "ci-reserve:v2:reserve-1".into(),
            required_capabilities: vec![],
            checkout_scope: Some(checkout_scope()),
            credential_binding: Some(CiJobCredentialBinding {
                binding_version: 1,
                purpose: "checkout_advertise".into(),
                generation_id: "cigen:abc".into(),
                issued_at_epoch_secs: 1_000,
                expires_at_epoch_secs: 1_300,
                ci_run_id: "44444444-4444-4444-4444-444444444444".into(),
                token_authority_handle: "tah-xyz".into(),
                idem_token: "11111111-1111-1111-1111-111111111111/build".into(),
            }),
        }
    }

    fn spec_with_context(context: Option<RunTokenAuthorizationContext>) -> JobSpec {
        job_spec(checkout_workspace(), context)
    }

    #[test]
    fn reconstruct_claim_recovers_every_field_from_the_context_and_binding() {
        let context = v2_context();
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context.clone())));
        let (claim, _checkout) = reconstruct_claim(&spec).expect("reconstructs an exact claim");
        assert_eq!(claim.tenant_id, context.tenant_id);
        assert_eq!(claim.region, context.region);
        assert_eq!(claim.wf_run_id, context.wf_run_id);
        assert_eq!(claim.job_id, context.job_id);
        assert_eq!(claim.lease_owner, context.lease_owner);
        assert_eq!(claim.lease_epoch, context.lease_epoch);
        assert_eq!(claim.claim_nonce, context.claim_nonce);
        assert_eq!(
            claim.claim_started_at_epoch_secs,
            context.claim_started_at_epoch_secs
        );
        assert_eq!(
            claim.claim_expires_at_epoch_secs,
            context.claim_expires_at_epoch_secs
        );
        let binding = context.credential_binding.as_ref().unwrap();
        assert_eq!(claim.ci_run_id, binding.ci_run_id);
        assert_eq!(claim.token_authority_handle, binding.token_authority_handle);
        assert_eq!(claim.idem_token, binding.idem_token);
        claim.validate().expect("the reconstructed claim validates");
    }

    #[test]
    fn reconstruct_claim_refuses_a_legacy_v1_context_without_a_binding() {
        let mut context = v2_context();
        context.credential_binding = None;
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context)));
        assert!(
            reconstruct_claim(&spec).is_err(),
            "a V1 shape has no claim identity to reconstruct"
        );
    }

    #[test]
    fn reconstruct_claim_refuses_a_missing_context() {
        let spec = spec_with_context(None);
        assert!(reconstruct_claim(&spec).is_err());
    }

    #[test]
    fn reconstruct_claim_refuses_a_scope_substitution() {
        let context = v2_context();
        let spec = job_spec(
            WorkspaceSpec::default(),
            Some(RunTokenAuthorizationContext::CiJob(context)),
        );
        assert!(
            reconstruct_claim(&spec).is_err(),
            "a spec whose workspace disagrees with the context's scope is a substitution and must refuse"
        );
    }

    #[test]
    fn reconstruct_claim_refuses_a_binding_purpose_that_is_not_the_initial() {
        let mut context = v2_context();
        context.credential_binding.as_mut().unwrap().purpose = "workload".into();
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context)));
        assert!(reconstruct_claim(&spec).is_err());
    }

    #[test]
    fn reconstruct_claim_refuses_an_idem_token_mismatch() {
        let mut context = v2_context();
        context.credential_binding.as_mut().unwrap().idem_token =
            "11111111-1111-1111-1111-111111111111/other".into();
        let spec = spec_with_context(Some(RunTokenAuthorizationContext::CiJob(context)));
        assert!(reconstruct_claim(&spec).is_err());
    }

    #[test]
    fn initial_phase_purpose_selects_advertise_for_checkout_and_workload_for_compute() {
        let scope = myelin_ci_sandbox::derive_checkout_authorization_scope(
            JobKind::Ci,
            &checkout_workspace(),
        )
        .expect("scope derives")
        .expect("the test spec is checkout-bearing");
        assert_eq!(
            initial_phase_purpose(Some(&scope)),
            CiCredentialPurpose::CheckoutAdvertise
        );
        assert_eq!(initial_phase_purpose(None), CiCredentialPurpose::Workload);
    }

    #[test]
    fn phase_and_purpose_mappings_are_total_and_disjoint() {
        assert_eq!(
            journal_phase(PreparationPhase::CheckoutTransport),
            CiPrelaunchUsagePhase::CheckoutTransport
        );
        assert_eq!(
            journal_phase(PreparationPhase::CheckoutMaterialization),
            CiPrelaunchUsagePhase::CheckoutMaterialization
        );
        assert_eq!(
            phase_purpose(CheckoutPhase::Advertise),
            CiCredentialPurpose::CheckoutAdvertise
        );
        assert_eq!(
            phase_purpose(CheckoutPhase::Fetch),
            CiCredentialPurpose::CheckoutFetch
        );
        assert_eq!(
            phase_purpose(CheckoutPhase::Materialization),
            CiCredentialPurpose::CheckoutMaterialization
        );
    }
}
