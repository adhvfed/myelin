use chrono::{DateTime, SecondsFormat, Utc};
use myelin_agent_service::RunTokenRevoker;
use myelin_events::Timestamp;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_identity::{
    DelegationCaveats as IdentityDelegationCaveats, FailStaticBound, Principal, PrincipalId,
    RevokeTarget, RunId,
};
use myelin_identity_service::{
    MachineKind, ResolvedDelegationPolicy, RevocationStore,
    RunTokenMinter as IdentityRunTokenMinter, RunTokenState,
};
use myelin_storage::TenantScope;

pub fn timestamp_from_epoch(secs: i64) -> Timestamp {
    let dt = DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch 0 is in range"));
    Timestamp(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

pub struct IdentityRunMinter {
    minter: IdentityRunTokenMinter,
    scope: TenantScope,
    agent: Principal,
    trigger_actor: Principal,
    resolved_policy: ResolvedDelegationPolicy,
    now: Timestamp,
    mint_attempt: Option<String>,
}

impl IdentityRunMinter {
    pub(crate) fn new(
        minter: IdentityRunTokenMinter,
        scope: TenantScope,
        agent: Principal,
        trigger_actor: Principal,
        resolved_policy: ResolvedDelegationPolicy,
        now: Timestamp,
        mint_attempt: Option<String>,
    ) -> IdentityRunMinter {
        IdentityRunMinter {
            minter,
            scope,
            agent,
            trigger_actor,
            resolved_policy,
            now,
            mint_attempt,
        }
    }
}

impl RunTokenMinter for IdentityRunMinter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        let identity_caveats = IdentityDelegationCaveats(caveats.0.clone());
        let mint = |attempt: Option<&str>| match attempt {
            Some(attempt) => self.minter.mint_from_resolved_policy_for_attempt(
                &self.scope,
                &PrincipalId(agent_id.to_string()),
                &RunId(run_id.to_string()),
                &self.agent,
                &self.trigger_actor,
                &self.resolved_policy,
                &identity_caveats,
                MachineKind::Agent,
                &FailStaticBound {
                    static_max_secs: ttl_secs,
                },
                &self.now,
                attempt,
            ),
            None => self.minter.mint_from_resolved_policy(
                &self.scope,
                &PrincipalId(agent_id.to_string()),
                &RunId(run_id.to_string()),
                &self.agent,
                &self.trigger_actor,
                &self.resolved_policy,
                &identity_caveats,
                MachineKind::Agent,
                &FailStaticBound {
                    static_max_secs: ttl_secs,
                },
                &self.now,
            ),
        };
        let token = mint(self.mint_attempt.as_deref()).map_err(|e| RunTokenError(e.to_string()))?;
        let (token, jti) = token.into_parts();
        Ok(RunTokenHandle {
            token,
            jti,
            ttl_secs,
        })
    }
}

pub struct IdentityRunRevoker {
    revocations: RevocationStore,
    scope: TenantScope,
}

impl IdentityRunRevoker {
    pub(crate) fn new(revocations: RevocationStore, scope: TenantScope) -> IdentityRunRevoker {
        IdentityRunRevoker { revocations, scope }
    }
}

impl RunTokenRevoker for IdentityRunRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        self.revocations
            .tear_down_run_token(&self.scope, jti, timestamp_from_epoch(now_secs));
        (now_secs - teardown_secs).max(0) as u64
    }

    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        let state = self.revocations.run_token_state(
            &self.scope,
            &RevokeTarget::Jti(jti.to_string()),
            &timestamp_from_epoch(now_secs),
        );
        state != RunTokenState::LiveWithinRunLife
    }
}
