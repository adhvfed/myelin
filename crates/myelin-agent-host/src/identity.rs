//! Adapts the `RunTokenMinter` / `RunTokenRevoker` seams (string/epoch-shaped so the platform loop
//! stays provider-agnostic) onto the real Identity providers: a PASETO-signed per-run token minted
//! under the cell authority, revoked via the durable S7 denylist. Each adapter is bound to one run's
//! verified `(tenant, region)` scope + agent principal, so a mint/revoke can't reach another tenant;
//! a failed mint returns `RunTokenError` loud so the loop aborts before reserve (fail-closed).

use chrono::{DateTime, SecondsFormat, Utc};
use myelin_agent_service::RunTokenRevoker;
use myelin_events::Timestamp;
use myelin_flow::{DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenMinter};
use myelin_identity::{
    DelegationCaveats as IdentityDelegationCaveats, FailStaticBound, Principal, PrincipalId,
    RevokeTarget, RunId,
};
use myelin_identity_service::{
    Authority, DelegationInput, MachineKind, RevocationStore,
    RunTokenMinter as IdentityRunTokenMinter, RunTokenState,
};
use myelin_storage::TenantScope;

/// Epoch seconds → the RFC-3339 `Timestamp` both the mint (`expires_at = now + ttl`) and the S7 store
/// (its instant compare) parse. Out-of-range falls back to the Unix epoch, never a malformed instant.
pub fn timestamp_from_epoch(secs: i64) -> Timestamp {
    let dt = DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch 0 is in range"));
    Timestamp(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// The `RunTokenMinter` seam over Identity's `mint_run_token`, bound to one run's scope + agent +
/// mint instant.
pub struct IdentityRunMinter {
    minter: IdentityRunTokenMinter,
    scope: TenantScope,
    agent: Principal,
    /// The delegation trigger actor — the agent itself for a hosted run (the intersection is driven
    /// by the caveats, so this doesn't widen authority).
    trigger_actor: Principal,
    /// The run's `now`, so the mint's `expires_at` and the teardown consult share one time base.
    now: Timestamp,
}

impl IdentityRunMinter {
    pub(crate) fn new(
        minter: IdentityRunTokenMinter,
        scope: TenantScope,
        agent: Principal,
        trigger_actor: Principal,
        now: Timestamp,
    ) -> IdentityRunMinter {
        IdentityRunMinter {
            minter,
            scope,
            agent,
            trigger_actor,
            now,
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
        // The run's authority is its caveat set (the delegation chain + the loop's per-run caveat).
        // All four conjuncts are that set, so the mint's monotone intersection can't widen it.
        let authority = Authority::of(caveats.0.iter().cloned());
        let input = DelegationInput {
            agent_policy: authority.clone(),
            delegation: authority.clone(),
            tenant_policy: authority.clone(),
            trigger_actor_held: authority,
        };
        let identity_caveats = IdentityDelegationCaveats(caveats.0.clone());
        let token = self
            .minter
            .mint_run_token(
                &self.scope,
                &PrincipalId(agent_id.to_string()),
                &RunId(run_id.to_string()),
                &self.agent,
                &self.trigger_actor,
                &input,
                &identity_caveats,
                MachineKind::Agent,
                &FailStaticBound {
                    static_max_secs: ttl_secs,
                },
                &self.now,
            )
            .map_err(|e| RunTokenError(e.to_string()))?; // fail-closed: the loop aborts before reserve
        let (token, jti) = token.into_parts();
        Ok(RunTokenHandle {
            token,
            jti,
            ttl_secs,
        })
    }
}

/// The `RunTokenRevoker` seam over Identity's durable S7 store, bound to one run's scope. The teardown
/// is durable (survives a restart), idempotent, and tenant-partitioned.
pub struct IdentityRunRevoker {
    revocations: RevocationStore,
    scope: TenantScope,
}

impl IdentityRunRevoker {
    pub(crate) fn new(revocations: RevocationStore, scope: TenantScope) -> IdentityRunRevoker {
        IdentityRunRevoker {
            revocations,
            scope,
        }
    }
}

impl RunTokenRevoker for IdentityRunRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        // Durable, idempotent (ON CONFLICT DO NOTHING), effective immediately. Returns the revocation
        // lag (teardown instant → revoke landing).
        self.revocations
            .tear_down_run_token(&self.scope, jti, timestamp_from_epoch(now_secs));
        (now_secs - teardown_secs).max(0) as u64
    }

    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        // Dead unless live within its run life — i.e. torn down, TTL-expired (revoke-on-crash defence),
        // or unknown.
        let state = self.revocations.run_token_state(
            &self.scope,
            &RevokeTarget::Jti(jti.to_string()),
            &timestamp_from_epoch(now_secs),
        );
        state != RunTokenState::LiveWithinRunLife
    }
}
