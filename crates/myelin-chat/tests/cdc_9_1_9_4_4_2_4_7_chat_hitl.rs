use myelin_chat::hitl::{
    build_card_signal, per_effect_idem_key as chat_key, post_decision, CardClick, CardDecision,
    CardEffect, CardOutcome, CardSignal, ChatApprovalCard, ClickGate, SignalDelivery, SignalPort,
    SignalPostError, DECLINE_MARKER,
};
use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{
    per_effect_idem_key as engine_key, DurableExecutor, FlowExecutor, RunBudget,
    RunId as FlowRunId, SignalOutcome, SignalSpec, StartSpec,
};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency as IdConsistency, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, RevokeTarget, RewriteTrace, RunId as IdRunId, RunToken, SubjectTree, TupleDelta,
    Zookie,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn human(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

#[test]
fn cdc_chat_and_engine_per_effect_key_are_byte_identical() {
    assert_eq!(chat_key("card-1", 0, 1), engine_key("card-1", 0, 1));
    assert_eq!(chat_key("card-1", 0, 1), "card-1");
    for idx in 0..3 {
        assert_eq!(
            chat_key("card-7", idx, 3),
            engine_key("card-7", idx, 3),
            "chat and engine must agree on the per-effect key for effect {idx}"
        );
    }
    assert_eq!(chat_key("card-7", 2, 3), "card-7:2");
}

struct FlowSignalPort {
    ex: FlowExecutor,
}

impl SignalPort for FlowSignalPort {
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError> {
        let outcome = self
            .ex
            .signal(SignalSpec {
                run: FlowRunId(signal.run_id.0.clone()),
                signal_name: signal.signal_name.clone(),
                idem_key: signal.idem_key.clone(),
                payload: signal.payload.clone(),
                payload_key_ref: signal.payload_key_ref.clone(),
            })
            .map_err(|e| SignalPostError {
                reason: format!("{e}"),
            })?;
        Ok(match outcome {
            SignalOutcome::Buffered => SignalDelivery::Buffered,
            SignalOutcome::Duplicate => SignalDelivery::Duplicate,
            SignalOutcome::TerminalNoOp => {
                unreachable!("the in-memory FlowExecutor buffers signals to terminal runs")
            }
        })
    }
}

fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn executor() -> FlowExecutor {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    ex
}

fn start_run(ex: &FlowExecutor, card_id: &str) -> FlowRunId {
    ex.start(StartSpec {
        wf_type: "agent.run".into(),
        input: vec![],
        budget: Some(RunBudget { minor_units: 1_000 }),
        idem_key: format!("start-{card_id}"),
    })
    .expect("start")
}

fn effect(refs: &[&str]) -> CardEffect {
    CardEffect {
        subject: ArtifactRef("myelin://acme/git/pr/88".into()),
        action: "merge".into(),
        risk: "irreversible".into(),
        cost: "$0.40".into(),
        effect_refs: refs.iter().map(|r| ArtifactRef((*r).into())).collect(),
    }
}

struct AllowId;
impl IdentityService for AllowId {
    fn authenticate(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn check(
        &self,
        _subject: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        at: &IdConsistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        assert_eq!(permission.0, "approve");
        assert!(object.0.starts_with("run:"));
        assert!(matches!(at.mode, myelin_identity::ConsistencyMode::Strong));
        Ok(Decision::Allow)
    }
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn list_subjects(
        &self,
        _object: &ObjectId,
        _permission: &Permission,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ObjectId,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn write_tuples(
        &self,
        _deltas: &[TupleDelta],
        _precondition: Option<&Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn mint_run_token(
        &self,
        _agent_id: &PrincipalId,
        run_id: &IdRunId,
        _delegation_caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        assert_eq!(
            ttl.static_max_secs,
            FailStaticBound::DEFAULT_W.static_max_secs
        );
        Ok(RunToken {
            token: format!("resume-{}", run_id.0),
            jti: format!("jti-{}", run_id.0),
        })
    }
    fn revoke(&self, _target: &RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn resolve_pseudonym(
        &self,
        _subject: &PrincipalId,
        _tenant: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn erase(&self, _subject: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn admit_fragment(
        &self,
        _fragment: &NamespaceFragment,
    ) -> myelin_identity::Result<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
}

#[test]
fn cdc_9_1_9_4_card_decision_posts_onto_the_real_engine_double_click_dedups() {
    let ex = executor();
    let run = start_run(&ex, "card-1");
    let card = ChatApprovalCard {
        run_id: IdRunId(run.0.clone()),
        card_id: "card-1".into(),
        effects: vec![effect(&["myelin://acme/agent/effect/merge-88"])],
    };
    let gate = ClickGate::new(AllowId);
    let port = FlowSignalPort { ex: ex.clone() };
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };

    let o1 = post_decision(&gate, &port, &card, &approve, &human("alice"), Some("zk")).unwrap();
    assert_eq!(o1, CardOutcome::Approved(SignalDelivery::Buffered));
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "the approve buffered exactly one signal on the real engine"
    );

    let o2 = post_decision(&gate, &port, &card, &approve, &human("alice"), Some("zk")).unwrap();
    assert_eq!(o2, CardOutcome::Approved(SignalDelivery::Duplicate));
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "the double-click did NOT buffer a second signal (a double-click is ONE approval)"
    );
}

#[test]
fn cdc_9_1_partial_approval_posts_three_independent_keys_decline_withholds() {
    let ex = executor();
    let run = start_run(&ex, "card-7");
    let card = ChatApprovalCard {
        run_id: IdRunId(run.0.clone()),
        card_id: "card-7".into(),
        effects: vec![effect(&["e0"]), effect(&["e1"]), effect(&["e2"])],
    };
    let gate = ClickGate::new(AllowId);
    let port = FlowSignalPort { ex: ex.clone() };
    let alice = human("alice");

    for (idx, decision) in [
        (0usize, CardDecision::Approve),
        (1, CardDecision::Decline),
        (2, CardDecision::Approve),
    ] {
        post_decision(
            &gate,
            &port,
            &card,
            &CardClick {
                effect_idx: idx,
                decision,
                decline_reason: if matches!(decision, CardDecision::Decline) {
                    DECLINE_MARKER.into()
                } else {
                    String::new()
                },
            },
            &alice,
            Some("zk"),
        )
        .unwrap();
    }

    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "a partial approval is three independent per-effect signals on the engine"
    );
    let declined = ex
        .signals()
        .get(&tenant(), &run.0, "approval:card-7", "card-7:1")
        .expect("the declined effect's signal is buffered under its own key");
    assert_eq!(
        declined.payload_key_ref.as_deref(),
        Some(DECLINE_MARKER),
        "the declined effect carries the DECLINE_MARKER → the engine WITHHOLDS it (AG-8, 0 mutation)"
    );
}

struct DenyId;
impl IdentityService for DenyId {
    fn authenticate(&self, _c: &myelin_identity::Credential) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ArtifactRef,
        _at: &IdConsistency,
        _c: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        Ok(Decision::Deny)
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<SubjectTree> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &IdConsistency,
    ) -> myelin_identity::Result<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn delegation(
        &self,
        _a: &Principal,
        _t: &Principal,
    ) -> myelin_identity::Result<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn write_tuples(
        &self,
        _d: &[TupleDelta],
        _p: Option<&Precondition>,
    ) -> myelin_identity::Result<Zookie> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        _r: &IdRunId,
        _d: &DelegationCaveats,
        _t: &FailStaticBound,
    ) -> myelin_identity::Result<RunToken> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn resolve_pseudonym(
        &self,
        _s: &PrincipalId,
        _t: &TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn erase(&self, _s: &PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> myelin_identity::Result<FragmentAdmit> {
        Err(AuthzError::NotYetImplemented("cdc"))
    }
}

#[test]
fn cdc_4_2_denied_click_posts_no_signal_onto_the_engine() {
    let ex = executor();
    let run = start_run(&ex, "card-1");
    let card = ChatApprovalCard {
        run_id: IdRunId(run.0.clone()),
        card_id: "card-1".into(),
        effects: vec![effect(&["e0"])],
    };
    let gate = ClickGate::new(DenyId);
    let port = FlowSignalPort { ex: ex.clone() };
    let res = post_decision(
        &gate,
        &port,
        &card,
        &CardClick {
            effect_idx: 0,
            decision: CardDecision::Approve,
            decline_reason: String::new(),
        },
        &human("mallory"),
        Some("zk"),
    );
    assert!(res.is_err(), "a denied click is fail-closed");
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        0,
        "a denied click posted NO signal - the gate is the chokepoint (the tool stays withheld)"
    );
}

#[test]
fn cdc_4_7_resume_token_is_freshly_minted_w_bounded() {
    use myelin_chat::hitl::ResumeTokenMinter;
    let minter = ResumeTokenMinter::new(AllowId);
    let token = minter
        .mint_resume_token(
            &PrincipalId("agent-x".into()),
            &IdRunId("R7".into()),
            &DelegationCaveats(vec!["scope:merge".into()]),
        )
        .expect("mint resume token");
    assert_eq!(
        token.token, "resume-R7",
        "a FRESH token for the resumed run"
    );
}

#[test]
fn cdc_decline_signal_shape_matches_the_engine_withhold_contract() {
    let card = ChatApprovalCard {
        run_id: IdRunId("R1".into()),
        card_id: "card-1".into(),
        effects: vec![effect(&["e0"])],
    };
    let sig = build_card_signal(
        &card,
        &CardClick {
            effect_idx: 0,
            decision: CardDecision::Decline,
            decline_reason: DECLINE_MARKER.into(),
        },
    );
    assert!(sig.payload.is_empty());
    assert_eq!(
        sig.payload_key_ref.as_deref(),
        Some(myelin_flow::DECLINE_MARKER)
    );
    assert_eq!(DECLINE_MARKER, myelin_flow::DECLINE_MARKER);
}
