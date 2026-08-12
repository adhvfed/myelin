use crate::runner::{
    PreparationAttemptDisposition, PreparationLeaseCheckpoint, PreparationLeaseLost,
    PreparationPhase, PreparationReportClaim, PreparationTerminalDisposition,
};
use crate::{
    CheckoutPhase, JobSpec, ReserveHandle, ResourceUsage, RunTokenAuthorizationContext,
    RunTokenCredential, SandboxLaunch,
};

pub(crate) fn rotate_spec_for_generation(
    base: &JobSpec,
    credential: RunTokenCredential,
    authorization_context: RunTokenAuthorizationContext,
) -> JobSpec {
    let mut spec = base.clone();
    spec.run_token = credential;
    spec.run_token_authorization = Some(authorization_context);
    spec
}

#[derive(Clone, Debug)]
pub struct PhaseCredentialCarrier {
    credential: RunTokenCredential,
    authorization_context: RunTokenAuthorizationContext,
    generation_id: String,
}

impl PhaseCredentialCarrier {
    pub fn new(
        credential: RunTokenCredential,
        authorization_context: RunTokenAuthorizationContext,
        generation_id: impl Into<String>,
    ) -> Self {
        Self {
            credential,
            authorization_context,
            generation_id: generation_id.into(),
        }
    }

    pub fn credential(&self) -> &RunTokenCredential {
        &self.credential
    }

    pub fn authorization_context(&self) -> &RunTokenAuthorizationContext {
        &self.authorization_context
    }

    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    pub(crate) fn phase_local_spec(&self, base: &JobSpec) -> JobSpec {
        rotate_spec_for_generation(
            base,
            self.credential.clone(),
            self.authorization_context.clone(),
        )
    }

    pub fn into_credential(self) -> RunTokenCredential {
        self.credential
    }
}

#[derive(Clone, Debug)]
pub struct WorkloadCredentialCarrier {
    credential: RunTokenCredential,
    authorization_context: RunTokenAuthorizationContext,
    generation_id: String,
}

impl WorkloadCredentialCarrier {
    pub fn new(
        credential: RunTokenCredential,
        authorization_context: RunTokenAuthorizationContext,
        generation_id: impl Into<String>,
    ) -> Self {
        Self {
            credential,
            authorization_context,
            generation_id: generation_id.into(),
        }
    }

    pub fn generation_id(&self) -> &str {
        &self.generation_id
    }

    pub fn credential(&self) -> &RunTokenCredential {
        &self.credential
    }

    pub(crate) fn workload_local_spec(&self, base: &JobSpec) -> JobSpec {
        rotate_spec_for_generation(
            base,
            self.credential.clone(),
            self.authorization_context.clone(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct AttemptAuthorityError(pub String);

impl std::fmt::Display for AttemptAuthorityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "attempt authority operation failed: {}", self.0)
    }
}

impl std::error::Error for AttemptAuthorityError {}

pub(crate) fn authorize_phase_generation(
    hooks: &crate::RunnerHooks,
    base_spec: &JobSpec,
    scope: &crate::CheckoutAuthorizationScope,
    phase: CheckoutPhase,
    carrier: PhaseCredentialCarrier,
) -> Result<(RunTokenCredential, crate::PhaseAuthorization), crate::HookError> {
    let phase_spec = carrier.phase_local_spec(base_spec);
    let authorization = hooks.authorize_checkout_phase(
        &phase_spec,
        scope.clone(),
        phase,
        carrier.generation_id(),
    )?;
    Ok((carrier.into_credential(), authorization))
}

pub trait AttemptAuthority: Send + Sync {
    fn begin_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError>;

    fn complete_phase(
        &self,
        phase: PreparationPhase,
        usage: ResourceUsage,
    ) -> Result<(), AttemptAuthorityError>;

    fn seal_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError>;

    fn renew_preparation_lease(&self) -> Result<(), PreparationLeaseLost>;

    fn mint_phase_credential(
        &self,
        phase: CheckoutPhase,
    ) -> Result<PhaseCredentialCarrier, AttemptAuthorityError>;

    fn mint_workload_credential(&self) -> Result<WorkloadCredentialCarrier, AttemptAuthorityError>;

    fn should_requeue(&self) -> bool;
}

pub(crate) struct AttemptAuthorityLeaseCheckpoint<'a>(pub &'a dyn AttemptAuthority);

impl PreparationLeaseCheckpoint for AttemptAuthorityLeaseCheckpoint<'_> {
    fn renew(&self) -> Result<(), PreparationLeaseLost> {
        self.0.renew_preparation_lease()
    }
}

pub enum ParentAttemptAdmission {
    Admitted {
        claim: PreparationReportClaim,
        reserve: ReserveHandle,
        attempt_authority: Box<dyn AttemptAuthority>,
    },
    AttemptsExhausted {
        claim: PreparationReportClaim,
        reserve: ReserveHandle,
    },
}

#[derive(Debug)]
pub enum CheckoutOrchestrationError {
    Hook(crate::HookError),
    Authority(AttemptAuthorityError),
    LeaseLost(PreparationLeaseLost),
}

impl std::fmt::Display for CheckoutOrchestrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hook(e) => write!(f, "checkout orchestration hook refused: {e}"),
            Self::Authority(e) => write!(f, "checkout orchestration authority failed: {e}"),
            Self::LeaseLost(e) => write!(f, "checkout orchestration lease lost: {e}"),
        }
    }
}

impl std::error::Error for CheckoutOrchestrationError {}

impl From<AttemptAuthorityError> for CheckoutOrchestrationError {
    fn from(e: AttemptAuthorityError) -> Self {
        Self::Authority(e)
    }
}

impl From<crate::HookError> for CheckoutOrchestrationError {
    fn from(e: crate::HookError) -> Self {
        Self::Hook(e)
    }
}

#[derive(Debug)]
pub enum CheckoutContinuationOutcome {
    WorkloadLaunched(SandboxLaunch),
    WorkloadRetryable {
        cause: crate::runner::RetryableAttemptCause,
        usage: ResourceUsage,
        message: String,
    },
    PreparationTerminal {
        claim: PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<String>,
    },
    PreparationRetryable {
        claim: PreparationReportClaim,
        phase: PreparationPhase,
    },
    ReconciliationRequired {
        phase: PreparationPhase,
        teardown_unproven: bool,
        usage_unrepresentable: bool,
        quarantine_required: bool,
    },
}

pub(crate) fn route_preparation_disposition(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    disposition: PreparationAttemptDisposition,
    usage: ResourceUsage,
    diagnostic: Option<String>,
) -> Result<CheckoutContinuationOutcome, AttemptAuthorityError> {
    match disposition {
        PreparationAttemptDisposition::Terminal(terminal) => {
            authority.complete_phase(terminal_phase(terminal), usage)?;
            Ok(CheckoutContinuationOutcome::PreparationTerminal {
                claim: claim.clone(),
                disposition: terminal,
                diagnostic,
            })
        }
        PreparationAttemptDisposition::RefusedBeforeExecution { phase } => {
            authority.complete_phase(
                phase,
                ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
            )?;
            Ok(requeue_or_exhausted(authority, claim, phase))
        }
        PreparationAttemptDisposition::RetryableInfrastructure { phase } => {
            authority.complete_phase(phase, usage)?;
            Ok(requeue_or_exhausted(authority, claim, phase))
        }
        PreparationAttemptDisposition::ReconciliationRequired {
            phase,
            teardown_unproven,
            usage_unrepresentable,
            quarantine_required,
        } => {
            authority.seal_phase(phase)?;
            Ok(CheckoutContinuationOutcome::ReconciliationRequired {
                phase,
                teardown_unproven,
                usage_unrepresentable,
                quarantine_required,
            })
        }
    }
}

pub(crate) fn route_after_disposal(
    disposal_diagnostics: Vec<String>,
    phase: PreparationPhase,
    clean: CheckoutContinuationOutcome,
) -> CheckoutContinuationOutcome {
    if disposal_diagnostics.is_empty() {
        clean
    } else {
        CheckoutContinuationOutcome::ReconciliationRequired {
            phase,
            teardown_unproven: true,
            usage_unrepresentable: false,
            quarantine_required: true,
        }
    }
}

pub(crate) fn resolve_hop_b_failure(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    disposition: PreparationAttemptDisposition,
    usage: ResourceUsage,
    diagnostic: Option<String>,
    disposal_diagnostics: Vec<String>,
) -> Result<CheckoutContinuationOutcome, AttemptAuthorityError> {
    let routed = route_preparation_disposition(authority, claim, disposition, usage, diagnostic)?;
    if disposal_diagnostics.is_empty() {
        Ok(routed)
    } else {
        Ok(CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutMaterialization,
            teardown_unproven: true,
            usage_unrepresentable: false,
            quarantine_required: true,
        })
    }
}

pub(crate) fn route_post_acquisition_authority_failure(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    disposal_diagnostics: Vec<String>,
    phase_was_begun: bool,
) -> CheckoutContinuationOutcome {
    let quarantined = !disposal_diagnostics.is_empty();
    if phase_was_begun
        && authority
            .complete_phase(
                PreparationPhase::CheckoutMaterialization,
                ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
            )
            .is_err()
    {
        return CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutMaterialization,
            teardown_unproven: quarantined,
            usage_unrepresentable: false,
            quarantine_required: quarantined,
        };
    }
    if quarantined {
        CheckoutContinuationOutcome::ReconciliationRequired {
            phase: PreparationPhase::CheckoutMaterialization,
            teardown_unproven: true,
            usage_unrepresentable: false,
            quarantine_required: true,
        }
    } else {
        requeue_or_exhausted(authority, claim, PreparationPhase::CheckoutMaterialization)
    }
}

pub(crate) fn requeue_or_exhausted(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    phase: PreparationPhase,
) -> CheckoutContinuationOutcome {
    requeue_or_exhausted_with_diagnostic(authority, claim, phase, None)
}

pub(crate) fn requeue_or_exhausted_with_diagnostic(
    authority: &dyn AttemptAuthority,
    claim: &PreparationReportClaim,
    phase: PreparationPhase,
    terminal_diagnostic: Option<String>,
) -> CheckoutContinuationOutcome {
    if authority.should_requeue() {
        CheckoutContinuationOutcome::PreparationRetryable {
            claim: claim.clone(),
            phase,
        }
    } else {
        CheckoutContinuationOutcome::PreparationTerminal {
            claim: claim.clone(),
            disposition: PreparationTerminalDisposition::AttemptsExhausted,
            diagnostic: terminal_diagnostic,
        }
    }
}

fn terminal_phase(disposition: PreparationTerminalDisposition) -> PreparationPhase {
    match disposition {
        PreparationTerminalDisposition::Failed { phase }
        | PreparationTerminalDisposition::TimedOut { phase } => phase,
        PreparationTerminalDisposition::AttemptsExhausted => PreparationPhase::CheckoutTransport,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingAuthority {
        ops: Mutex<Vec<String>>,
        should_requeue: bool,
    }

    impl RecordingAuthority {
        fn new(should_requeue: bool) -> Self {
            Self {
                ops: Mutex::new(Vec::new()),
                should_requeue,
            }
        }
        fn ops(&self) -> Vec<String> {
            self.ops.lock().unwrap().clone()
        }
    }

    impl AttemptAuthority for RecordingAuthority {
        fn begin_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!("begin:{phase:?}"));
            Ok(())
        }
        fn complete_phase(
            &self,
            phase: PreparationPhase,
            usage: ResourceUsage,
        ) -> Result<(), AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!(
                "complete:{phase:?}:{}:{}",
                usage.cpu_seconds, usage.mem_byte_seconds
            ));
            Ok(())
        }
        fn seal_phase(&self, phase: PreparationPhase) -> Result<(), AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!("seal:{phase:?}"));
            Ok(())
        }
        fn renew_preparation_lease(&self) -> Result<(), PreparationLeaseLost> {
            self.ops.lock().unwrap().push("renew".to_string());
            Ok(())
        }
        fn mint_phase_credential(
            &self,
            phase: CheckoutPhase,
        ) -> Result<PhaseCredentialCarrier, AttemptAuthorityError> {
            self.ops.lock().unwrap().push(format!("mint:{phase:?}"));
            Ok(PhaseCredentialCarrier::new(
                RunTokenCredential::new("bearer", format!("jti-{phase:?}"), 300).unwrap(),
                test_authorization_context(),
                format!("gen-{phase:?}"),
            ))
        }
        fn mint_workload_credential(
            &self,
        ) -> Result<WorkloadCredentialCarrier, AttemptAuthorityError> {
            self.ops.lock().unwrap().push("mint:Workload".to_string());
            Ok(WorkloadCredentialCarrier::new(
                RunTokenCredential::new("bearer", "jti-Workload", 300).unwrap(),
                test_authorization_context(),
                "gen-Workload",
            ))
        }
        fn should_requeue(&self) -> bool {
            self.should_requeue
        }
    }

    fn test_authorization_context() -> RunTokenAuthorizationContext {
        RunTokenAuthorizationContext::CiJob(crate::CiJobAuthorizationContext {
            tenant_id: "acme".to_string(),
            region: "fr-par".to_string(),
            principal_id: "p".to_string(),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf".to_string(),
            job_id: "j".to_string(),
            lease_owner: "o".to_string(),
            lease_epoch: 1,
            claim_nonce: "n".to_string(),
            claim_started_at_epoch_secs: 0,
            claim_expires_at_epoch_secs: 1,
            reserve_id: "r".to_string(),
            required_capabilities: vec![],
            checkout_scope: None,
            credential_binding: None,
        })
    }

    fn distinct_authorization_context(generation: &str) -> RunTokenAuthorizationContext {
        RunTokenAuthorizationContext::CiJob(crate::CiJobAuthorizationContext {
            tenant_id: "acme".to_string(),
            region: "us-west-2".to_string(),
            principal_id: format!("principal-{generation}"),
            project_id: "00000000-0000-0000-0000-000000000001".to_string(),
            wf_run_id: "wf-rotated".to_string(),
            job_id: format!("job-{generation}"),
            lease_owner: "rotated-owner".to_string(),
            lease_epoch: 99,
            claim_nonce: format!("nonce-{generation}"),
            claim_started_at_epoch_secs: 1000,
            claim_expires_at_epoch_secs: 2000,
            reserve_id: format!("reserve-{generation}"),
            required_capabilities: vec!["repo:widgets#pull".to_string()],
            checkout_scope: None,
            credential_binding: None,
        })
    }

    fn usage(cpu: u64, mem: u64) -> ResourceUsage {
        ResourceUsage {
            cpu_seconds: cpu,
            mem_byte_seconds: mem,
        }
    }

    fn report_claim() -> PreparationReportClaim {
        PreparationReportClaim {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
            ci_run_id: "44444444-4444-4444-4444-444444444444".into(),
            job_id: "22222222-2222-2222-2222-222222222222".into(),
            token_authority_handle: "tah-xyz".into(),
            idem_token: "11111111-1111-1111-1111-111111111111/build".into(),
            lease_owner: "worker-1".into(),
            lease_epoch: 7,
            claim_nonce: "33333333-3333-3333-3333-333333333333".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
        }
    }

    #[test]
    fn phase_credential_carrier_rotates_only_the_credential_and_context() {
        let base = crate::checkout_job_spec_for_tests();
        let carrier_context = distinct_authorization_context("materialization");
        let carrier_credential = RunTokenCredential::new("bearer", "the-jti", 300).unwrap();
        let carrier = PhaseCredentialCarrier::new(
            carrier_credential.clone(),
            carrier_context.clone(),
            "gen-42",
        );
        assert_eq!(carrier.generation_id(), "gen-42");

        assert_ne!(base.run_token, carrier_credential);
        assert_ne!(base.run_token_authorization, Some(carrier_context.clone()));

        let phase_spec = carrier.phase_local_spec(&base);
        assert_eq!(phase_spec.run_token, carrier_credential);
        assert_eq!(
            phase_spec.run_token_authorization,
            Some(carrier_context.clone())
        );
        let mut expected = base.clone();
        expected.run_token = carrier_credential.clone();
        expected.run_token_authorization = Some(carrier_context);
        assert_eq!(phase_spec, expected);
        assert_eq!(carrier.into_credential(), carrier_credential);
    }

    #[test]
    fn workload_carrier_is_type_separate_and_rotates_its_own_spec() {
        let base = crate::checkout_job_spec_for_tests();
        let carrier_context = distinct_authorization_context("workload");
        let carrier_credential = RunTokenCredential::new("bearer", "wl-jti", 300).unwrap();
        let carrier = WorkloadCredentialCarrier::new(
            carrier_credential.clone(),
            carrier_context.clone(),
            "gen-wl",
        );
        assert_ne!(base.run_token, carrier_credential);
        assert_ne!(base.run_token_authorization, Some(carrier_context.clone()));

        let workload_spec = carrier.workload_local_spec(&base);
        assert_eq!(workload_spec.run_token, carrier_credential);
        assert_eq!(
            workload_spec.run_token_authorization,
            Some(carrier_context.clone())
        );
        let mut expected = base.clone();
        expected.run_token = carrier_credential;
        expected.run_token_authorization = Some(carrier_context);
        assert_eq!(workload_spec, expected);
        assert_eq!(carrier.generation_id(), "gen-wl");
    }

    #[test]
    fn authorize_phase_generation_rotates_and_threads_the_matching_credential() {
        use std::sync::Arc;
        #[allow(clippy::type_complexity)]
        let seen: Arc<Mutex<Option<(String, Option<RunTokenAuthorizationContext>)>>> =
            Arc::new(Mutex::new(None));
        let recorder = seen.clone();
        let hooks = crate::RunnerHooks::new(
            crate::CompletionSettlementOwner::TerminalReporter,
            Box::new(|s| Ok(crate::ReserveHandle(s.meter_to.reserve_id.clone()))),
            Box::new(|_, _, _| Ok(())),
            Box::new(|_| Ok(())),
            Box::new(|_| Ok(())),
        )
        .with_checkout_phase_authorization(Box::new(move |spec, _scope, _phase| {
            *recorder.lock().unwrap() = Some((
                spec.run_token.jti.clone(),
                spec.run_token_authorization.clone(),
            ));
            Ok(crate::LaunchPermit::immediate())
        }));
        let base = crate::checkout_job_spec_for_tests();
        assert_eq!(
            base.run_token.jti, "advertise-jti",
            "the base carries the advertise generation"
        );
        let scope = crate::derive_checkout_authorization_scope(base.kind, &base.workspace)
            .expect("scope derives")
            .expect("checkout-bearing");
        let carrier_context = distinct_authorization_context("fetch");
        let carrier = PhaseCredentialCarrier::new(
            RunTokenCredential::new("bearer", "fetch-jti-xyz", 300).unwrap(),
            carrier_context.clone(),
            "gen-fetch",
        );
        let (credential, _authorization) =
            authorize_phase_generation(&hooks, &base, &scope, CheckoutPhase::Fetch, carrier)
                .expect("authorizes");
        let (seen_jti, seen_ctx) = seen.lock().unwrap().clone().expect("the hook was invoked");
        assert_eq!(
            seen_jti, "fetch-jti-xyz",
            "the phase hook must be handed the rotated credential, not the advertise base"
        );
        assert_eq!(
            seen_ctx,
            Some(carrier_context),
            "the phase hook must be handed the rotated authorization context, not the advertise base's"
        );
        assert_eq!(credential.jti, "fetch-jti-xyz");
    }

    #[test]
    fn resolve_hop_b_failure_seals_and_reconciles_a_quarantined_teardown_unproven() {
        let authority = RecordingAuthority::new(true);
        let outcome = resolve_hop_b_failure(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: true,
            },
            usage(2, 2),
            None,
            vec!["slot quarantined; workspace manager poisoned".to_string()],
        )
        .expect("routes");
        assert_eq!(
            authority.ops(),
            vec!["seal:CheckoutMaterialization"],
            "the started materialization phase must be sealed immediately"
        );
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                quarantine_required: true,
                ..
            }
        ));
    }

    #[test]
    fn resolve_hop_b_failure_completes_terminal_on_a_clean_disposal() {
        let authority = RecordingAuthority::new(true);
        let outcome = resolve_hop_b_failure(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::Terminal(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutMaterialization,
            }),
            usage(4, 4),
            Some("host-side HEAD re-verification disagreed: injected".to_string()),
            vec![],
        )
        .expect("routes");
        assert_eq!(
            authority.ops(),
            vec!["complete:CheckoutMaterialization:4:4"]
        );
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal { .. }
        ));
    }

    #[test]
    fn post_acquisition_authority_failure_routing_matrix() {
        let authority = RecordingAuthority::new(true);
        let out =
            route_post_acquisition_authority_failure(&authority, &report_claim(), vec![], false);
        assert!(authority.ops().is_empty(), "no begun phase to complete");
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable { .. }
        ));

        let authority = RecordingAuthority::new(true);
        let out =
            route_post_acquisition_authority_failure(&authority, &report_claim(), vec![], true);
        assert_eq!(
            authority.ops(),
            vec!["complete:CheckoutMaterialization:0:0"]
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationRetryable { .. }
        ));

        let authority = RecordingAuthority::new(false);
        let out =
            route_post_acquisition_authority_failure(&authority, &report_claim(), vec![], true);
        assert_eq!(
            authority.ops(),
            vec!["complete:CheckoutMaterialization:0:0"]
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));

        let authority = RecordingAuthority::new(true);
        let out = route_post_acquisition_authority_failure(
            &authority,
            &report_claim(),
            vec!["slot quarantined".to_string()],
            true,
        );
        assert!(matches!(
            out,
            CheckoutContinuationOutcome::ReconciliationRequired {
                quarantine_required: true,
                ..
            }
        ));
    }

    #[test]
    fn requeue_or_exhausted_carries_the_exact_claim_into_both_outcomes() {
        let claim = report_claim();
        let requeued = requeue_or_exhausted(
            &RecordingAuthority::new(true),
            &claim,
            PreparationPhase::CheckoutTransport,
        );
        match requeued {
            CheckoutContinuationOutcome::PreparationRetryable {
                claim: carried,
                phase,
            } => {
                assert_eq!(carried, claim, "the retry outcome carries the exact claim");
                assert_eq!(phase, PreparationPhase::CheckoutTransport);
            }
            other => panic!("expected a retryable, got {other:?}"),
        }
        let exhausted = requeue_or_exhausted(
            &RecordingAuthority::new(false),
            &claim,
            PreparationPhase::CheckoutTransport,
        );
        match exhausted {
            CheckoutContinuationOutcome::PreparationTerminal {
                claim: carried,
                disposition,
                ..
            } => {
                assert_eq!(
                    carried, claim,
                    "the exhausted terminal carries the exact claim"
                );
                assert_eq!(
                    disposition,
                    PreparationTerminalDisposition::AttemptsExhausted
                );
            }
            other => panic!("expected an exhausted terminal, got {other:?}"),
        }
    }

    #[test]
    fn exhausted_attempt_preserves_the_terminal_diagnostic() {
        let outcome = requeue_or_exhausted_with_diagnostic(
            &RecordingAuthority::new(false),
            &report_claim(),
            PreparationPhase::CheckoutMaterialization,
            Some("workload launch permit was revoked".to_string()),
        );
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                diagnostic: Some(ref diagnostic),
                ..
            } if diagnostic == "workload launch permit was revoked"
        ));
    }

    #[test]
    fn route_terminal_completes_the_active_phase_with_exact_usage() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::Terminal(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutTransport,
            }),
            usage(7, 9),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutTransport:7:9"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::Failed {
                    phase: PreparationPhase::CheckoutTransport
                },
                ..
            }
        ));
    }

    #[test]
    fn route_timed_out_completes_and_reports_terminal() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::Terminal(PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutMaterialization,
            }),
            usage(3, 3),
            None,
        )
        .expect("routes");
        assert_eq!(
            authority.ops(),
            vec!["complete:CheckoutMaterialization:3:3"]
        );
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::TimedOut { .. },
                ..
            }
        ));
    }

    #[test]
    fn route_refused_completes_zero_then_requeues_when_permitted() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::RefusedBeforeExecution {
                phase: PreparationPhase::CheckoutTransport,
            },
            usage(5, 5),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutTransport:0:0"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutTransport,
                ..
            }
        ));
    }

    #[test]
    fn route_refused_terminalizes_attempts_exhausted_when_not_permitted() {
        let authority = RecordingAuthority::new(false);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::RefusedBeforeExecution {
                phase: PreparationPhase::CheckoutTransport,
            },
            usage(0, 0),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["complete:CheckoutTransport:0:0"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationTerminal {
                disposition: PreparationTerminalDisposition::AttemptsExhausted,
                ..
            }
        ));
    }

    #[test]
    fn route_retryable_infrastructure_completes_with_exact_usage_then_requeues() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::RetryableInfrastructure {
                phase: PreparationPhase::CheckoutMaterialization,
            },
            usage(11, 13),
            None,
        )
        .expect("routes");
        assert_eq!(
            authority.ops(),
            vec!["complete:CheckoutMaterialization:11:13"]
        );
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::PreparationRetryable {
                phase: PreparationPhase::CheckoutMaterialization,
                ..
            }
        ));
    }

    #[test]
    fn route_usage_unrepresentable_seals_at_the_ceiling_and_requires_reconciliation() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutTransport,
                teardown_unproven: false,
                usage_unrepresentable: true,
                quarantine_required: false,
            },
            usage(999, 999),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["seal:CheckoutTransport"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::ReconciliationRequired {
                usage_unrepresentable: true,
                teardown_unproven: false,
                ..
            }
        ));
    }

    #[test]
    fn route_after_disposal_reconciles_on_a_quarantined_disposal() {
        let clean = CheckoutContinuationOutcome::PreparationRetryable {
            claim: report_claim(),
            phase: PreparationPhase::CheckoutMaterialization,
        };
        let quarantined = route_after_disposal(
            vec!["slot quarantined; workspace manager poisoned".to_string()],
            PreparationPhase::CheckoutMaterialization,
            CheckoutContinuationOutcome::PreparationRetryable {
                claim: report_claim(),
                phase: PreparationPhase::CheckoutMaterialization,
            },
        );
        assert!(matches!(
            quarantined,
            CheckoutContinuationOutcome::ReconciliationRequired {
                quarantine_required: true,
                teardown_unproven: true,
                ..
            }
        ));
        let ok = route_after_disposal(vec![], PreparationPhase::CheckoutMaterialization, clean);
        assert!(matches!(
            ok,
            CheckoutContinuationOutcome::PreparationRetryable { .. }
        ));
    }

    #[test]
    fn route_teardown_unproven_seals_and_requires_reconciliation() {
        let authority = RecordingAuthority::new(true);
        let outcome = route_preparation_disposition(
            &authority,
            &report_claim(),
            PreparationAttemptDisposition::ReconciliationRequired {
                phase: PreparationPhase::CheckoutMaterialization,
                teardown_unproven: true,
                usage_unrepresentable: false,
                quarantine_required: true,
            },
            usage(1, 1),
            None,
        )
        .expect("routes");
        assert_eq!(authority.ops(), vec!["seal:CheckoutMaterialization"]);
        assert!(matches!(
            outcome,
            CheckoutContinuationOutcome::ReconciliationRequired {
                teardown_unproven: true,
                quarantine_required: true,
                usage_unrepresentable: false,
                ..
            }
        ));
    }
}
