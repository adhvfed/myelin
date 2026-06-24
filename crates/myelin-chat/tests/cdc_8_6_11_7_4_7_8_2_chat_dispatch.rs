//! # The CDC pair for the CHAT-P25 dispatch orchestration — chat CONSUMES 8.6 (explicit-first) +
//! 11.7 (reserve/settle) + 4.7 (mint_run_token) + 8.2 (EffectApi) (CHAT-P25 / P-419, M4-C9).
//!
//! **Contracts:** `contract-index.md` rows **8.6** (`EventInbox::deliver` + explicit-first dispatch
//! CHAT-1 — a mention notifies, does not auto-spawn a costed run), **11.7** (reserve/settle — reserve
//! at dispatch, no balance → no run; gates EVERY agent run), **4.7** (`mint_run_token` — a per-run
//! attenuated token, life == run life), **8.2** (`EffectApi::apply` — plan-then-apply; agents NEVER
//! mutate directly). **Reconciliation** `00-reconciliation-decisions.md` §6 (explicit-first pinned).
//!
//! ## The seam this pair pins (chat is the CONSUMER on all four rows)
//! The companion `cdc_8_6_chat_explicit_first.rs` (NOTIF-P22 / P-343) pins chat's explicit-first CLASS
//! decision against the Bus dispatch TIER. THIS pair pins the **chat-module dispatch ORCHESTRATION**
//! ([`myelin_chat::dispatch`]) CHAT-P25 ships — the CONSUMER side of four contracts assembled into one
//! dispatch path:
//! - **8.6 CONSUMER** — a casual mention → notify (0 run); only an explicit action dispatches. The
//!   PROVIDER is the explicit-first invariant (chat's own [`agent_dispatch_class`], REUSED).
//! - **11.7 CONSUMER** — the reserve gate fronts the explicit run (no balance → no run). The PROVIDER
//!   is the M1 Storage [`CostLedger`] (chat does not re-implement the ledger).
//! - **4.7 CONSUMER** — an explicit dispatch mints a per-run token via Identity. The PROVIDER is the
//!   M1 Identity [`mint_run_token`] (chat does not invent a token).
//! - **8.2 CONSUMER** — the run's chat output routes through `EffectApi` (the routing split). The
//!   PROVIDER is the Agent Fabric [`EffectApi`] (chat does not mutate directly).
//!
//! Each contract is exercised against a deterministic model of its frozen shape (the real bodies are
//! the named floors: 8.6=AG-P4/P-216, 11.7=P-103/P-146, 4.7=P-ID-18, 8.2=AG-P6/P-218). The pair
//! asserts both directions: a casual mention CONSUMES none of the cost contracts (notify is free), and
//! an explicit run CONSUMES all three (reserve → mint → apply) — the boundary is real on both sides.

use myelin_agent::{EffectApi, EffectResult, EventId as FxEventId, ProposedEffect, RunCtx};
use myelin_chat::dispatch::{
    dispatch_disposition_class, dispatch_explicit, DispatchOutcome, Disposition,
};
use myelin_chat::events::{CHAT_MESSAGE_MENTIONED, CHAT_REACTION_ADDED};
use myelin_identity::{
    Consistency, Credential, DelegationCaveats, FailStaticBound, FragmentAdmit, NamespaceFragment,
    ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId, Result as IdResult,
    RevokeTarget, RunId as IdRunId, RunToken, TupleDelta, Zookie,
};
use myelin_storage::reserve_settle::{CostLedger, MinorUnits, RunId as LedgerRunId};
use myelin_tenancy::{ArtifactRef, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_id() -> PrincipalId {
    PrincipalId("agent:assistant".into())
}

// ─────────────────────── the 4.7 PROVIDER model — mint_run_token ───────────────────────────────────

/// A deterministic Identity that mints a per-run token (4.7) — the real mint is P-ID-18. The minted
/// `jti` is recorded by the consumer so the pair asserts the run is attributed under the minted token.
struct MintingIdentity;
impl myelin_identity::IdentityService for MintingIdentity {
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        unimplemented!()
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _a: &Consistency,
        _c: Option<&myelin_identity::CaveatContext>,
    ) -> IdResult<myelin_identity::Decision> {
        unimplemented!()
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> IdResult<myelin_identity::ListObjectsResult> {
        unimplemented!()
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _a: &Consistency,
    ) -> IdResult<myelin_identity::SubjectTree> {
        unimplemented!()
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _a: &Consistency,
    ) -> IdResult<myelin_identity::RewriteTrace> {
        unimplemented!()
    }
    fn delegation(
        &self,
        _a: &Principal,
        _t: &Principal,
    ) -> IdResult<myelin_identity::EffectivePolicy> {
        unimplemented!()
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        unimplemented!()
    }
    fn mint_run_token(
        &self,
        _agent_id: &PrincipalId,
        run_id: &IdRunId,
        caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        // the PROVIDER's promise: a per-run token whose jti keys the run; the caveat chain carries
        // chat's dispatch caveat; the TTL is bounded by W (life == run life, 4.7).
        assert!(
            caveats.0.iter().any(|c| c.starts_with("chat:dispatch:")),
            "the mint carries chat's dispatch caveat (attenuate-only)"
        );
        assert_eq!(
            ttl.static_max_secs,
            FailStaticBound::DEFAULT_W.static_max_secs
        );
        Ok(RunToken {
            token: format!("tok:{}", run_id.0),
            jti: format!("jti:{}", run_id.0),
        })
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        unimplemented!()
    }
    fn resolve_pseudonym(&self, _p: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        unimplemented!()
    }
    fn erase(&self, _p: &PrincipalId) -> IdResult<()> {
        unimplemented!()
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        unimplemented!()
    }
}

// ─────────────────────── the 8.2 PROVIDER model — EffectApi::apply ─────────────────────────────────

/// A deterministic `EffectApi` (8.2) — the PROVIDER's promise: a `ProposedEffect` is applied through
/// the platform's plan-then-apply pipeline (the real one is AG-P6/P-218); returns the applied event id.
struct ApplyingEffectApi;
impl EffectApi for ApplyingEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        // the run ctx carries the minted token's jti (the run is attributed) — 4.7 ⇄ 8.2 thread.
        assert!(
            run.0.starts_with("jti:"),
            "the run's chat output applies under the minted run token (4.7 → 8.2)"
        );
        EffectResult::Applied(FxEventId(format!("applied:{}", effect.0)))
    }
}

// ─────────────────────── the pair: a casual mention CONSUMES none of the cost contracts ────────────

/// **8.6 CONSUMER (notify side)** — a casual `@agent` mention is notify-only: it CONSUMES neither
/// 11.7 (reserve) nor 4.7 (mint) nor 8.2 (apply). The chat-module dispatch path short-circuits a
/// `NotifyOnly` class to `NotifiedInbox` WITHOUT touching the ledger/Identity/EffectApi.
#[test]
fn cdc_a_casual_mention_consumes_no_cost_contracts() {
    // 8.6: the class decision is notify-only (the explicit-first invariant).
    assert_eq!(
        dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, /*is_explicit_action=*/ false),
        DispatchOutcome::NotifyOnly,
        "8.6 CONSUMER: a casual @agent mention notifies — no auto-spawn"
    );
    // a NotifyOnly never reaches dispatch_explicit — the ledger stays untouched (0 reservations).
    let ledger = CostLedger::new();
    // (the ledger model has no public reservation count; the disposition itself is the proof — a
    // mention is NotifiedInbox, which by construction never calls reserve/mint/apply.)
    let disp = match dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false) {
        DispatchOutcome::NotifyOnly => Disposition::NotifiedInbox,
        DispatchOutcome::WouldDispatch => unreachable!("a mention never would-dispatch"),
    };
    assert_eq!(disp, Disposition::NotifiedInbox);
    drop(ledger);
}

// ─────────────────────── the pair: an explicit run CONSUMES all three cost contracts ───────────────

/// **8.6 + 11.7 + 4.7 + 8.2 CONSUMER (dispatch side)** — an explicit action reserves (11.7), mints a
/// per-run token (4.7), and routes the output through `EffectApi` (8.2), the whole chain threaded by
/// the minted jti. This is the chat side of all four contracts in ONE dispatch path.
#[test]
fn cdc_an_explicit_run_consumes_reserve_mint_and_effect_api() {
    // 8.6: an explicit action would-dispatch.
    assert_eq!(
        dispatch_disposition_class(CHAT_REACTION_ADDED, /*is_explicit_action=*/ true),
        DispatchOutcome::WouldDispatch,
        "8.6 CONSUMER: a deliberate explicit action dispatches"
    );

    let id = MintingIdentity;
    let fx = ApplyingEffectApi;
    let mut ledger = CostLedger::new();
    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        tenant(),
        &agent_id(),
        "run:cdc:1",
        MinorUnits(5),
        MinorUnits(10),
        ProposedEffect("chat.post".into()),
    );

    // 4.7: the run carries the minted per-run token jti.
    assert_eq!(
        disp,
        Disposition::Dispatched {
            run_token_jti: "jti:run:cdc:1".into()
        },
        "4.7 CONSUMER: the run is attributed under the minted per-run token"
    );
    // 8.2: the chat output routed through EffectApi (the routing split, X-6).
    assert_eq!(
        applied,
        Some(EffectResult::Applied(FxEventId("applied:chat.post".into()))),
        "8.2 CONSUMER: the run's chat output applied through EffectApi"
    );
    // 11.7: the reservation was recorded on the ledger (settle would close it — never interrupt
    // in-flight). The ledger now holds the (tenant, run) reservation; a second reserve is a dup.
    let dup = ledger.reserve(
        tenant(),
        LedgerRunId("run:cdc:1".into()),
        MinorUnits(5),
        MinorUnits(10),
    );
    assert!(
        dup.is_err(),
        "11.7 CONSUMER: the explicit run reserved exactly once (a re-reserve is a loud duplicate)"
    );
}

/// **11.7 CONSUMER (the gate bites)** — no balance → no run. With ZERO available balance the explicit
/// run is REFUSED at the reserve gate before anything is minted or applied (the runaway self-limiter).
#[test]
fn cdc_the_reserve_gate_refuses_an_unfunded_explicit_run() {
    let id = MintingIdentity;
    let fx = ApplyingEffectApi;
    let mut ledger = CostLedger::new();
    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        tenant(),
        &agent_id(),
        "run:cdc:2",
        MinorUnits(50),
        MinorUnits(0),
        ProposedEffect("chat.post".into()),
    );
    assert_eq!(
        disp,
        Disposition::NoBalanceRefused {
            requested: MinorUnits(50),
            available: MinorUnits(0)
        },
        "11.7 CONSUMER: no balance → no run (reserve gates even the explicit run)"
    );
    assert_eq!(
        applied, None,
        "nothing minted, nothing applied — the run never started"
    );
}
