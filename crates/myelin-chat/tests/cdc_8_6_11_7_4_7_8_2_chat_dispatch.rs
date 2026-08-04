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
use myelin_storage::reserve_settle::{CostLedger, MicroUsd, RunId as LedgerRunId};
use myelin_tenancy::{ArtifactRef, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_id() -> PrincipalId {
    PrincipalId("agent:assistant".into())
}

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

struct ApplyingEffectApi;
impl EffectApi for ApplyingEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        assert!(
            run.0.starts_with("jti:"),
            "the run's chat output applies under the minted run token (4.7 → 8.2)"
        );
        EffectResult::Applied(FxEventId(format!("applied:{}", effect.0)))
    }
}

#[test]
fn cdc_a_casual_mention_consumes_no_cost_contracts() {
    assert_eq!(
        dispatch_disposition_class(CHAT_MESSAGE_MENTIONED,  false),
        DispatchOutcome::NotifyOnly,
        "8.6 CONSUMER: a casual @agent mention notifies - no auto-spawn"
    );
    let ledger = CostLedger::new();
    let disp = match dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false) {
        DispatchOutcome::NotifyOnly => Disposition::NotifiedInbox,
        DispatchOutcome::WouldDispatch => unreachable!("a mention never would-dispatch"),
    };
    assert_eq!(disp, Disposition::NotifiedInbox);
    drop(ledger);
}

#[test]
fn cdc_an_explicit_run_consumes_reserve_mint_and_effect_api() {
    assert_eq!(
        dispatch_disposition_class(CHAT_REACTION_ADDED,  true),
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
        MicroUsd(5),
        MicroUsd(10),
        ProposedEffect("chat.post".into()),
    );

    assert_eq!(
        disp,
        Disposition::Dispatched {
            run_token_jti: "jti:run:cdc:1".into()
        },
        "4.7 CONSUMER: the run is attributed under the minted per-run token"
    );
    assert_eq!(
        applied,
        Some(EffectResult::Applied(FxEventId("applied:chat.post".into()))),
        "8.2 CONSUMER: the run's chat output applied through EffectApi"
    );
    let dup = ledger.reserve(
        tenant(),
        LedgerRunId("run:cdc:1".into()),
        MicroUsd(5),
        MicroUsd(10),
    );
    assert!(
        dup.is_err(),
        "11.7 CONSUMER: the explicit run reserved exactly once (a re-reserve is a loud duplicate)"
    );
}

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
        MicroUsd(50),
        MicroUsd(0),
        ProposedEffect("chat.post".into()),
    );
    assert_eq!(
        disp,
        Disposition::NoBalanceRefused {
            requested: MicroUsd(50),
            available: MicroUsd(0)
        },
        "11.7 CONSUMER: no balance → no run (reserve gates even the explicit run)"
    );
    assert_eq!(
        applied, None,
        "nothing minted, nothing applied - the run never started"
    );
}
