//! # Real Identity-backed per-run token mint + revocation (the composition-root adapters).
//!
//! The hosted-agent driving loop ([`myelin_agent_service::SkeletonAgent::handle_run`]) mints a
//! per-run token at dispatch and revokes it on teardown through two frozen seams — the
//! [`myelin_flow::RunTokenMinter`] mint half and the [`myelin_agent_service::RunTokenRevoker`]
//! teardown half. Those seams are string/epoch-shaped so the platform loop and the durable-workflow
//! engine stay provider-agnostic (they never depend on `myelin-identity-service`). This module is the
//! ONE place — the leaf composition root — where the seams are adapted onto the REAL Identity
//! providers:
//!
//! - the mint is Identity's [`myelin_identity_service::RunTokenMinter::mint_run_token`] (contract
//!   4.7): a real per-run attenuated capability token, PASETO-signed under the cell token authority,
//!   whose authority is the monotone intersection of the run's delegation caveats, bound to the run's
//!   agent [`Principal`] and the per-run `run:<id>` caveat, with an `expires_at == now + ttl` TTL
//!   registered in the durable S7 revocation store (the revoke-on-crash defence-in-depth); and
//! - the teardown is Identity's durable S7 [`RevocationStore::tear_down_run_token`]: a
//!   `(tenant, region)`-partitioned, RLS-scoped, mirror-first denylist write that survives a process
//!   restart, is idempotent even on a double-fire (explicit teardown + crash-recovery sweep), and
//!   whose `run_token_state` consult drives [`RunTokenRevoker::is_dead`] (dead on TornDown OR the
//!   auto-expiring TTL).
//!
//! Both adapters are bound to ONE run's verified `(tenant, region)` scope + agent principal + mint
//! instant, captured by [`crate::AgentHost`] before the run starts, so a mint/revoke can never reach
//! outside the run's tenant partition. A failed mint returns [`RunTokenError`] LOUD, so the loop
//! aborts the run BEFORE any reserve (fail-closed: a run that cannot mint never starts).

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

/// Convert the engine's epoch-seconds clock into the canonical RFC-3339 `YYYY-MM-DDTHH:MM:SSZ`
/// [`Timestamp`] that BOTH the Identity mint (its `expires_at == now + ttl` arithmetic) and the S7
/// revocation store (its `now < expires_at` instant compare) parse. Both adapters — the mint's `now`
/// anchor and the revoke/`is_dead` consult — funnel through this ONE conversion so the mint instant
/// and the teardown instant share a comparable time base (the TTL window is honoured correctly). An
/// out-of-range epoch falls back to the Unix epoch (never a malformed, unparseable instant).
pub fn timestamp_from_epoch(secs: i64) -> Timestamp {
    let dt = DateTime::<Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).expect("epoch 0 is in range"));
    Timestamp(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// **The real per-run token MINTER — the [`myelin_flow::RunTokenMinter`] seam over Identity's
/// `mint_run_token` (contract 4.7).** Bound to ONE run's verified scope + agent principal + mint
/// instant. Each mint produces a real PASETO-signed per-run token whose authority is the run's
/// attenuated caveat set (incl. the per-run `run:<id>` caveat the loop appends), bound to the agent
/// [`Principal`], with an `expires_at == now + ttl` TTL registered in the durable S7 store.
pub struct IdentityRunMinter {
    /// Identity's real per-run minter (cloneable handle over the S7 store + the PASETO signer).
    minter: IdentityRunTokenMinter,
    /// The run's verified `(tenant, region)` partition — the tenant-from-token scope the TTL is
    /// registered in (never a path; the mint cannot reach another tenant).
    scope: TenantScope,
    /// The run's agent principal (`PrincipalKind::Agent { runtime_ref, on_behalf_of }`) — the subject
    /// the token is minted for.
    agent: Principal,
    /// The trigger actor whose held authority is one delegation conjunct. For a hosted run this is the
    /// agent itself (the run acts as the agent); the delegation intersection is driven by the caveats.
    trigger_actor: Principal,
    /// The mint instant (the run's `now`), captured from the run task so the mint's `expires_at` and
    /// the teardown consult share one time base.
    now: Timestamp,
}

impl IdentityRunMinter {
    /// Bind Identity's real minter to ONE run's scope + agent + mint instant.
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
        // The run's attenuated authority IS its caveat set (the delegation chain PLUS the per-run
        // `run:<id>` caveat the loop appended). All four delegation conjuncts are that same set, so
        // the monotone intersection the mint re-applies (`agent ∩ delegation ∩ tenant ∩ trigger`)
        // yields exactly the run's authority — a token can never carry authority the run was not
        // granted, and the mint never widens it.
        let authority = Authority::of(caveats.0.iter().cloned());
        let input = DelegationInput {
            agent_policy: authority.clone(),
            delegation: authority.clone(),
            tenant_policy: authority.clone(),
            trigger_actor_held: authority,
        };
        // The frozen ABI carrier — the projection of the delegation the rich `input` also carries.
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
            // A refused/failed mint surfaces LOUD — the loop aborts the run BEFORE any reserve, so a
            // run that cannot mint a real token never starts (fail-closed).
            .map_err(|e| RunTokenError(e.to_string()))?;
        let (token, jti) = token.into_parts();
        Ok(RunTokenHandle {
            token,
            jti,
            ttl_secs,
        })
    }
}

/// **The real per-run token REVOKER — the [`myelin_agent_service::RunTokenRevoker`] seam over
/// Identity's durable S7 store (contract 4.7).** Bound to ONE run's verified scope. The teardown
/// writes the `(tenant, region)`-partitioned, RLS-scoped `run_token_teardown` denylist row — durable
/// (survives a process restart), idempotent even on a double-fire, and fail-closed (a teardown that
/// cannot durably land panics rather than silently letting a torn-down token validate).
pub struct IdentityRunRevoker {
    /// Identity's durable S7 revocation store (cloneable handle over the PG-backed denylist).
    revocations: RevocationStore,
    /// The run's verified `(tenant, region)` partition — the teardown + the `is_dead` consult are
    /// scoped to it (a revoked run's token is dead for everyone in the partition; no cross-tenant path).
    scope: TenantScope,
}

impl IdentityRunRevoker {
    /// Bind Identity's durable S7 store to ONE run's scope.
    pub(crate) fn new(revocations: RevocationStore, scope: TenantScope) -> IdentityRunRevoker {
        IdentityRunRevoker {
            revocations,
            scope,
        }
    }
}

impl RunTokenRevoker for IdentityRunRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> u64 {
        // The durable, idempotent teardown: the `run_token_teardown` row lands mirror-first (survives
        // a restart) and a re-revoke of the same jti is a no-op success (`ON CONFLICT DO NOTHING`).
        // The deny is effective immediately (a hot consult) — the revocation lag is the gap between
        // the run's teardown instant and this revoke landing.
        self.revocations
            .tear_down_run_token(&self.scope, jti, timestamp_from_epoch(now_secs));
        (now_secs - teardown_secs).max(0) as u64
    }

    fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
        // Dead when the token is NOT live within its run life — i.e. torn down (the explicit teardown
        // landed), expired (the `expires_at == run-life` TTL passed, even absent a teardown — the
        // revoke-on-crash defence), or unknown (no S7 record). Only `LiveWithinRunLife` is alive.
        let state = self.revocations.run_token_state(
            &self.scope,
            &RevokeTarget::Jti(jti.to_string()),
            &timestamp_from_epoch(now_secs),
        );
        state != RunTokenState::LiveWithinRunLife
    }
}
