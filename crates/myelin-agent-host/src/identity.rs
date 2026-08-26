use myelin_agent_service::RunTokenRevoker;
use myelin_events::clock::clock_reading_from_unix;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimestampOutOfRange {
    seconds: i64,
}

impl core::fmt::Display for TimestampOutOfRange {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "epoch seconds {} are outside the supported timestamp range",
            self.seconds
        )
    }
}

impl std::error::Error for TimestampOutOfRange {}

pub fn timestamp_from_epoch(secs: i64) -> Result<Timestamp, TimestampOutOfRange> {
    clock_reading_from_unix(secs)
        .map(|reading| reading.timestamp())
        .map_err(|_| TimestampOutOfRange { seconds: secs })
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
    fn revoke(&self, jti: &str) -> Result<u64, String> {
        let started = std::time::Instant::now();
        self.revocations
            .tear_down_run_token(&self.scope, jti)
            .map_err(|error| error.to_string())?;
        Ok(revocation_lag_seconds(started.elapsed()))
    }

    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        let Ok(now) = timestamp_from_epoch(now_secs) else {
            return true;
        };
        let state = self.revocations.run_token_state(
            &self.scope,
            &RevokeTarget::Jti(jti.to_string()),
            &now,
        );
        state != RunTokenState::LiveWithinRunLife
    }
}

fn revocation_lag_seconds(elapsed: std::time::Duration) -> u64 {
    elapsed
        .as_secs()
        .saturating_add(u64::from(elapsed.subsec_nanos() != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_conversion_refuses_unrepresentable_instants() {
        assert_eq!(
            timestamp_from_epoch(0),
            Ok(Timestamp("1970-01-01T00:00:00Z".into()))
        );
        assert_eq!(
            timestamp_from_epoch(i64::MAX),
            Err(TimestampOutOfRange { seconds: i64::MAX })
        );
        assert_eq!(
            timestamp_from_epoch(i64::MIN),
            Err(TimestampOutOfRange { seconds: i64::MIN })
        );
    }

    #[test]
    fn revocation_lag_rounds_up_so_a_completed_write_never_disappears_as_zero() {
        assert_eq!(revocation_lag_seconds(std::time::Duration::ZERO), 0);
        assert_eq!(
            revocation_lag_seconds(std::time::Duration::from_nanos(1)),
            1
        );
        assert_eq!(revocation_lag_seconds(std::time::Duration::from_secs(1)), 1);
        assert_eq!(
            revocation_lag_seconds(
                std::time::Duration::from_secs(1) + std::time::Duration::from_nanos(1)
            ),
            2
        );
    }
}
