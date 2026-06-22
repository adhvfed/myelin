//! # The CDC pair for mid-workflow `mint_run_token` re-mint on resume — contract 4.7
//! (workflow CONSUMER ↔ Identity PROVIDER)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 4.7
//! (`mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` — callable mid-workflow on
//! resume, CONSUMED by workflow). Owning architecture: `durable-workflow.md` §6.2 (mid-workflow token
//! re-mint on resume — *token life == activity life*) + §5.2 (the 4.7 pin).
//!
//! ## What this pair pins (the CONSUMER ↔ PROVIDER agreement of 4.7 across the mint seam)
//!
//! **4.7 CONSUMER (the workflow engine's re-mint on resume) — what it drives:** on a resume across a
//! multi-day wait it calls the contract-4.7 mint surface with `(agent_id, run_id, delegation_caveats,
//! ttl)` where `ttl` is the SHORT fail-static window (token life == activity life) and the caveats are
//! ATTENUATED per-run. It NEVER fabricates a token — it always asks Identity.
//!
//! **4.7 PROVIDER (Identity's `IdentityService::mint_run_token`):** mints a per-run attenuated token
//! `RunToken{token, jti}` for `(agent_id, run_id)` with the delegation caveats + the TTL bound. This
//! fixture adapts the REAL `myelin_identity::IdentityService` trait (the frozen contract-4.7 surface)
//! onto the engine's [`RunTokenMinter`] seam — the agreement is the SAME `(agent_id, run_id, caveats,
//! ttl)` flows engine → Identity → `RunToken` → engine.
//!
//! **FLOOR named:** the `mint_run_token` BODY is Identity's (body → P-ID-18, M1 — the M0 stub returns
//! `NotYetImplemented`). This CDC fixture is a REAL `IdentityService` impl that mints a deterministic
//! test token, proving the engine drives the surface with the right args (the short TTL + per-run
//! attenuation). The production binding onto the live Identity service lands when the dispatcher wires
//! the run's agent identity (the spine, P-FLOW-27).

use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
};
use myelin_flow::engine::{SignalRow, SignalStore};
use myelin_flow::{
    approval_wait_name, DelegationCaveats, RunTokenError, RunTokenHandle, RunTokenLease,
    RunTokenMinter, WaitOutcome, WfCtx, WfJournal,
};
use myelin_identity::{
    AuthzError, Consistency, Credential, Decision, DelegationCaveats as IdCaveats, EffectivePolicy,
    FailStaticBound, FragmentAdmit, IdentityService, NamespaceFragment, ObjectId, ObjectType,
    Permission, Precondition, Principal, PrincipalId, PrincipalKind, RevokeTarget, RewriteTrace,
    RunId as IdRunId, RunToken, SubjectTree, TupleDelta, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

/// **The PROVIDER fixture: a REAL `myelin_identity::IdentityService` whose `mint_run_token` mints a
/// deterministic per-run token (contract 4.7).** It RECORDS the `(agent_id, run_id, caveats, ttl)` it
/// was called with (so the test can prove the engine drove the surface with the short TTL + the
/// per-run attenuation) and mints a fresh, DISTINCT `RunToken` per call (a re-mint is a NEW token).
/// Every OTHER method is the M0 fail-closed / not-yet-implemented floor (this CDC pins ONLY the 4.7
/// mint path).
#[derive(Default)]
struct MintingIdentity {
    calls: AtomicU64,
    last: Mutex<Option<(PrincipalId, IdRunId, IdCaveats, FailStaticBound)>>,
}

impl IdentityService for MintingIdentity {
    fn mint_run_token(
        &self,
        agent_id: &PrincipalId,
        run_id: &IdRunId,
        delegation_caveats: &IdCaveats,
        ttl: &FailStaticBound,
    ) -> Result<RunToken, AuthzError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some((
            agent_id.clone(),
            run_id.clone(),
            delegation_caveats.clone(),
            *ttl,
        ));
        // a deterministic, DISTINCT per-mint token (the real mint is P-ID-18; this proves the seam).
        Ok(RunToken {
            token: format!("rt-{}-{}", run_id.0, n),
            jti: format!("jti-{}-{}", run_id.0, n),
        })
    }

    // ---- the rest of the eleven-method surface is the M0 floor (this CDC pins only 4.7) ----
    fn authenticate(&self, _c: &Credential) -> Result<Principal, AuthzError> {
        Err(AuthzError::NotYetImplemented("authenticate (CDC stub)"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _at: &Consistency,
        _cv: Option<&myelin_identity::CaveatContext>,
    ) -> Result<Decision, AuthzError> {
        Ok(Decision::Deny)
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> Result<myelin_identity::ListObjectsResult, AuthzError> {
        Err(AuthzError::NotYetImplemented("list_objects (CDC stub)"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> Result<SubjectTree, AuthzError> {
        Err(AuthzError::NotYetImplemented("list_subjects (CDC stub)"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> Result<RewriteTrace, AuthzError> {
        Err(AuthzError::NotYetImplemented("explain (CDC stub)"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> Result<EffectivePolicy, AuthzError> {
        Err(AuthzError::NotYetImplemented("delegation (CDC stub)"))
    }
    fn write_tuples(
        &self,
        _d: &[TupleDelta],
        _pre: Option<&Precondition>,
    ) -> Result<Zookie, AuthzError> {
        Err(AuthzError::NotYetImplemented("write_tuples (CDC stub)"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> Result<(), AuthzError> {
        Err(AuthzError::NotYetImplemented("revoke (CDC stub)"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> Result<String, AuthzError> {
        Err(AuthzError::NotYetImplemented(
            "resolve_pseudonym (CDC stub)",
        ))
    }
    fn erase(&self, _s: &PrincipalId) -> Result<(), AuthzError> {
        Err(AuthzError::NotYetImplemented("erase (CDC stub)"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> Result<FragmentAdmit, AuthzError> {
        Err(AuthzError::NotYetImplemented("admit_fragment (CDC stub)"))
    }
}

/// **The CONSUMER-side adapter: the engine's [`RunTokenMinter`] over the REAL `IdentityService`
/// (contract 4.7).** It maps the engine's `(agent_id, run_id, caveats, ttl_secs)` onto Identity's
/// `mint_run_token(PrincipalId, RunId, DelegationCaveats, FailStaticBound)` and maps the returned
/// `RunToken` back onto the engine's [`RunTokenHandle`] — the 1:1 contract-4.7 agreement across the
/// mint seam. A `NotYetImplemented` / fail-static from Identity maps to a loud [`RunTokenError`].
struct IdentityRemintAdapter {
    id: Arc<MintingIdentity>,
}

impl RunTokenMinter for IdentityRemintAdapter {
    fn mint_run_token(
        &self,
        agent_id: &str,
        run_id: &str,
        caveats: &DelegationCaveats,
        ttl_secs: u64,
    ) -> Result<RunTokenHandle, RunTokenError> {
        let token = self
            .id
            .mint_run_token(
                &PrincipalId(agent_id.into()),
                &IdRunId(run_id.into()),
                &IdCaveats(caveats.0.clone()),
                &FailStaticBound {
                    static_max_secs: ttl_secs,
                },
            )
            .map_err(|e| RunTokenError(format!("{e:?}")))?;
        Ok(RunTokenHandle {
            token: token.token,
            jti: token.jti,
            ttl_secs,
        })
    }
}

fn lease(id: Arc<MintingIdentity>) -> RunTokenLease {
    RunTokenLease::new(
        Arc::new(IdentityRemintAdapter { id }),
        "agent://acme/agent/triage",
        DelegationCaveats(vec!["tenant:acme".into()]),
    )
}

fn deliver_approval(signals: &SignalStore, call_id: &str, payload: Vec<ArtifactRef>) {
    signals.deliver(SignalRow {
        tenant: tenant(),
        region: region(),
        run_id: "R1".into(),
        signal_name: approval_wait_name(call_id),
        idem_key: format!("card:{call_id}"),
        payload,
        payload_key_ref: None,
        consumed_seq: None,
    });
}

/// **PROVIDER ↔ CONSUMER: a days-later HITL resume re-mints a fresh token via Identity's
/// `mint_run_token` (contract 4.7, §6.2).** Drive 1 parks on the approval wait (state=waiting holds no
/// runtime, no mint). The approval arrives DAYS later. Drive 2 (the resume) consumes it AND drives the
/// contract-4.7 mint surface: Identity mints a short-lived per-run token — the engine's re-mint.
#[test]
fn provider_mints_a_short_lived_per_run_token_on_a_days_later_hitl_resume() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let signals = SignalStore::new();
    let id = Arc::new(MintingIdentity::default());

    // DRIVE 1: park on the approval wait (no signal yet) → no mint (the run holds NO token while parked).
    let mut c1 = WfCtx::begin(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_signals(signals.clone())
    .with_run_identity(lease(id.clone()));
    let out1 = c1
        .wait_for_signal(&approval_wait_name("call-7"), Some(3600))
        .expect("park on the approval wait");
    assert_eq!(
        out1,
        WaitOutcome::Parked,
        "drive 1 parks (state=waiting, holds no runtime)"
    );
    assert_eq!(
        c1.reminted_tokens(),
        0,
        "a PARK does not re-mint (the run holds no token while waiting)"
    );
    assert_eq!(
        id.calls.load(Ordering::SeqCst),
        0,
        "Identity was NOT asked to mint on the park"
    );
    c1.commit().expect("co-commit the park");
    let history = journal.history_for(&tenant(), "R1");

    // ... DAYS later, the human clicks Approve ...
    deliver_approval(
        &signals,
        "call-7",
        vec![ArtifactRef("myelin://acme/approval/yes".into())],
    );

    // DRIVE 2 (the resume): consume the approval AND re-mint a fresh token via Identity (contract 4.7).
    let mut c2 = WfCtx::resume(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        42,
        history,
    )
    .with_signals(signals.clone())
    .with_run_identity(lease(id.clone()));
    let out2 = c2
        .wait_for_signal(&approval_wait_name("call-7"), Some(3600))
        .expect("resume + consume");
    assert!(
        matches!(out2, WaitOutcome::Signalled { .. }),
        "the resume consumed the approval"
    );
    assert_eq!(
        c2.reminted_tokens(),
        1,
        "the resume re-minted exactly one fresh token"
    );

    // CONSUMER ↔ PROVIDER agreement: Identity's mint_run_token was driven with the engine's args.
    assert_eq!(
        id.calls.load(Ordering::SeqCst),
        1,
        "Identity minted exactly once on resume"
    );
    let (agent, run, caveats, ttl) = id.last.lock().unwrap().clone().expect("a mint recorded");
    assert_eq!(
        agent.0, "agent://acme/agent/triage",
        "minted for the run's agent (4.7 agent_id)"
    );
    assert_eq!(run.0, "R1", "minted for THIS run (4.7 run_id)");
    // SHORT-LIVED: the TTL is the fail-static window (token life == activity life), not the workflow life.
    assert_eq!(
        ttl.static_max_secs,
        RunTokenLease::DEFAULT_TTL_SECS,
        "the mint TTL is the SHORT fail-static window — not the days-long workflow life (§6.2)"
    );
    // ATTENUATED per-run: the caveat chain carries a per-run caveat naming THIS run.
    assert!(
        caveats.0.contains(&"run:R1".to_string()),
        "the token is attenuated per-run (scoped to THIS run): {caveats:?}"
    );
    assert!(
        caveats.0.contains(&"tenant:acme".to_string()),
        "the lease's grant chain is carried (attenuate-only)"
    );
}

/// **CONSUMER: the explicit `remint_on_resume` surface drives Identity's `mint_run_token` directly
/// (contract 4.7).** A body that calls the public re-mint gets a fresh `RunToken` from the real
/// Identity provider — the engine never fabricates a token.
#[test]
fn consumer_remint_on_resume_drives_identity_mint_run_token() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let id = Arc::new(MintingIdentity::default());
    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        journal,
        ctx_base(),
        "R1",
        "agent.run",
        "2026-06-21T00:00:00Z",
        42,
    )
    .with_run_identity(lease(id.clone()));

    let handle = ctx.remint_on_resume().expect("re-mint via Identity");
    assert_eq!(
        handle.token, "rt-R1-0",
        "the token came from Identity's mint_run_token (not fabricated)"
    );
    assert_eq!(
        handle.ttl_secs,
        RunTokenLease::DEFAULT_TTL_SECS,
        "short-lived (token life == activity life)"
    );
    assert_eq!(
        id.calls.load(Ordering::SeqCst),
        1,
        "Identity's mint surface was driven"
    );
}
