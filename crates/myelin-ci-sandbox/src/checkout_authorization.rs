use crate::workspace_intent::ExpectedGitCommitId;
use crate::{
    CheckoutAuthorizationScope, HookError, JobSpec, LaunchPermit, RunTokenCredential, RunnerHooks,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckoutPhase {
    Advertise,
    Fetch,
    Materialization,
}

impl CheckoutPhase {
    pub fn purpose_token(self) -> &'static str {
        match self {
            Self::Advertise => "checkout_advertise",
            Self::Fetch => "checkout_fetch",
            Self::Materialization => "checkout_materialization",
        }
    }
}

pub(crate) struct PhaseAuthorization {
    scope: CheckoutAuthorizationScope,
    run_token_jti: String,
    phase: CheckoutPhase,
    generation_id: String,
    permit: LaunchPermit,
}

impl std::fmt::Debug for PhaseAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhaseAuthorization")
            .field("phase", &self.phase)
            .field("generation_id", &self.generation_id)
            .field("run_token_jti", &self.run_token_jti)
            .field("permit", &"<retained durable permit>")
            .finish()
    }
}

impl PhaseAuthorization {
    pub(crate) fn generation_id(&self) -> &str {
        &self.generation_id
    }

    fn verify_provenance(
        &self,
        expected_phase: CheckoutPhase,
        run_token: &RunTokenCredential,
    ) -> Result<(), HookError> {
        if self.phase != expected_phase {
            return Err(HookError(format!(
                "phase authorization was minted for the {:?} boundary, but this is the \
                 {expected_phase:?} boundary -- refusing before any spawn",
                self.phase
            )));
        }
        if self.run_token_jti != run_token.jti {
            return Err(HookError(format!(
                "phase authorization was minted against run-token jti {:?}, but this boundary is \
                 running under jti {:?} -- refusing before any spawn",
                self.run_token_jti, run_token.jti
            )));
        }
        if self.generation_id.trim().is_empty() {
            return Err(HookError(
                "phase authorization carries no durable generation id".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn into_transport_permit(
        self,
        expected_phase: CheckoutPhase,
        run_token: &RunTokenCredential,
        tenant: &str,
        repo: &str,
        expected: &ExpectedGitCommitId,
    ) -> Result<LaunchPermit, HookError> {
        self.verify_provenance(expected_phase, run_token)?;
        if self.scope.tenant().0 != tenant {
            return Err(HookError(format!(
                "phase authorization was minted for tenant {:?}, but this transport is requesting \
                 tenant {tenant:?}",
                self.scope.tenant().0
            )));
        }
        if self.scope.repo_id() != repo {
            return Err(HookError(format!(
                "phase authorization was minted for repo {:?}, but this transport is requesting \
                 repo {repo:?}",
                self.scope.repo_id()
            )));
        }
        if self.scope.commit_hex() != expected.as_str()
            || self.scope.commit_format() != expected.format()
        {
            return Err(HookError(format!(
                "phase authorization was minted for commit {:?} ({:?}), but this transport is \
                 requesting {:?} ({:?})",
                self.scope.commit_hex(),
                self.scope.commit_format(),
                expected.as_str(),
                expected.format()
            )));
        }
        Ok(self.permit)
    }

    pub(crate) fn into_preparation_permit_for_scope(
        self,
        run_token: &RunTokenCredential,
        expected_scope: &CheckoutAuthorizationScope,
        expected_commit: &ExpectedGitCommitId,
    ) -> Result<LaunchPermit, HookError> {
        self.verify_provenance(CheckoutPhase::Materialization, run_token)?;
        if &self.scope != expected_scope {
            return Err(HookError(format!(
                "materialization authorization was minted for scope (tenant {:?}, repo {:?}, commit \
                 {:?}), but this preparation capsule was acquired for scope (tenant {:?}, repo {:?}, \
                 commit {:?}) -- refusing before any spawn",
                self.scope.tenant().0,
                self.scope.repo_id(),
                self.scope.commit_hex(),
                expected_scope.tenant().0,
                expected_scope.repo_id(),
                expected_scope.commit_hex()
            )));
        }
        if expected_scope.commit_hex() != expected_commit.as_str()
            || expected_scope.commit_format() != expected_commit.format()
        {
            return Err(HookError(format!(
                "the capsule's checkout scope names commit {:?} ({:?}), but this preparation is \
                 checking out {:?} ({:?})",
                expected_scope.commit_hex(),
                expected_scope.commit_format(),
                expected_commit.as_str(),
                expected_commit.format()
            )));
        }
        Ok(self.permit)
    }
}

impl RunnerHooks {
    pub(crate) fn authorize_checkout_phase(
        &self,
        spec: &JobSpec,
        scope: CheckoutAuthorizationScope,
        phase: CheckoutPhase,
        generation_id: &str,
    ) -> Result<PhaseAuthorization, HookError> {
        let Some(hook) = &self.checkout_phase_authorization else {
            return Err(HookError(format!(
                "the {phase:?} phase boundary requires a configured phase-authorization hook, but \
                 none was provided"
            )));
        };
        if generation_id.trim().is_empty() {
            return Err(HookError(
                "a phase authorization requires a non-empty durable generation id".to_string(),
            ));
        }
        let permit = hook(spec, &scope, phase)?;
        Ok(PhaseAuthorization {
            scope,
            run_token_jti: spec.run_token.jti.clone(),
            phase,
            generation_id: generation_id.to_owned(),
            permit,
        })
    }
}
