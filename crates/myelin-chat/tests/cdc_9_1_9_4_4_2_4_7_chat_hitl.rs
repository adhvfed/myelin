//! # The CDC quad for the chat HITL approval-card bridge (CHAT-P18 → P-413, M4-C6)
//!
//! **Contracts:** `contract-index.md` rows
//! - `9.1` / `9.4` `DurableExecutor::signal` (idempotent on the per-effect `idem_key`; the durable
//!   HITL signal) — chat is the **CONSUMER** (the card POSTS the signal); `myelin_flow::FlowExecutor`
//!   is the **PROVIDER**.
//! - `4.2` `check(human, approve, run)` (the approve gate) — chat CONSUMES it (the click gate).
//! - `4.7` `mint_run_token` (the resume token) — chat CONSUMES it (the resume-token mint).
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` §OQ-F (the per-effect `idem_key` rule —
//! `card_id` single, `card_id:<effect_idx>` multi/partial — frozen). **Owning architecture:** chat
//! `02-internals-and-algorithms.md` §5 (Chat is the SURFACE; steps 2 + 3 of the round-trip).
//!
//! ## The seam this quad pins (chat POSTS the decision; the engine DEDUPS + parks)
//! - **CONSUMER (chat — [`myelin_chat::hitl`])** builds the per-effect signal and posts it through the
//!   [`SignalPort`]; chat depends on the TRAIT, never the concrete engine (the production DAG stays
//!   acyclic).
//! - **PROVIDER (the engine — [`myelin_flow::FlowExecutor::signal`])** buffers the signal idempotently
//!   on `(tenant, run_id, signal_name, idem_key)` — a double-click re-posting the SAME per-effect key
//!   is a no-op (`Duplicate`); a partial approval is three independent keys (well-defined).
//! - **PARITY:** chat's `per_effect_idem_key` and `myelin_flow::per_effect_idem_key` produce the
//!   BYTE-IDENTICAL key — ONE rule (OQ-F), not two divergent copies.

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

// ───────────────────────── PARITY: ONE per-effect rule (OQ-F) ─────────────────────────

/// **The PARITY leg (OQ-F): chat's `per_effect_idem_key` IS the engine's, byte-for-byte.** There is
/// ONE frozen rule, not two — if chat's copy ever diverged from the engine's, the per-effect dedup
/// would break silently. This asserts byte parity across single + multi arities.
#[test]
fn cdc_chat_and_engine_per_effect_key_are_byte_identical() {
    // single-effect: the bare card id (a double-click is one approval).
    assert_eq!(chat_key("card-1", 0, 1), engine_key("card-1", 0, 1));
    assert_eq!(chat_key("card-1", 0, 1), "card-1");
    // multi-effect: card_id:effect_idx (each effect independently keyed).
    for idx in 0..3 {
        assert_eq!(
            chat_key("card-7", idx, 3),
            engine_key("card-7", idx, 3),
            "chat and engine must agree on the per-effect key for effect {idx}"
        );
    }
    assert_eq!(chat_key("card-7", 2, 3), "card-7:2");
}

// ───────────────────────── the real FlowExecutor as a SignalPort (9.1 / 9.4) ─────────────────────

/// **The PROVIDER adapter — chat's [`SignalPort`] over the REAL `myelin_flow::FlowExecutor::signal`.**
/// This is the production wiring shape: lower a chat [`CardSignal`] onto a `myelin_flow::SignalSpec`
/// and map the engine's [`SignalOutcome`] back. The engine OWNS the idempotency (`ON CONFLICT DO
/// NOTHING` on `(tenant, run_id, signal_name, idem_key)`) — chat just posts.
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

// ───────────────────────── an always-allow IdentityService (the 4.2 / 4.7 seam) ─────────────────

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
        // the click gate checks `approve` on the run object at Strong consistency (the new-enemy
        // guard) — the CDC asserts the consumer calls it with the FROZEN shape.
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
        // the resume token TTL is W-bounded (4.11) so a revoked resume token expires inside the SLA.
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

// ───────────────────────── 9.1 / 9.4: the card posts onto the REAL engine, double-click dedups ───

/// **The 9.1/9.4 pair end-to-end: chat's gated card decision POSTS onto the REAL
/// `FlowExecutor::signal`, and a double-click DEDUPS (one approval).** A single-effect card; a
/// gated approve buffers ONE signal on the engine; a re-click under the SAME per-effect key is a
/// `Duplicate` (the engine's `ON CONFLICT DO NOTHING`) → the engine's buffered depth stays 1.
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

    // first click → the engine BUFFERS the approval (the first decision).
    let o1 = post_decision(&gate, &port, &card, &approve, &human("alice"), Some("zk")).unwrap();
    assert_eq!(o1, CardOutcome::Approved(SignalDelivery::Buffered));
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "the approve buffered exactly one signal on the real engine"
    );

    // DOUBLE-CLICK: re-post the SAME per-effect key → the engine DEDUPS (Duplicate; one approval).
    let o2 = post_decision(&gate, &port, &card, &approve, &human("alice"), Some("zk")).unwrap();
    assert_eq!(o2, CardOutcome::Approved(SignalDelivery::Duplicate));
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "the double-click did NOT buffer a second signal (a double-click is ONE approval)"
    );
}

/// **A partial approval (2-of-3) posts THREE independent per-effect keys onto the real engine; the
/// declined effect carries NO payload (0 mutation, AG-8).** approve 0, decline 1, approve 2 — the
/// engine buffers three distinct signals keyed `card-7:0` / `card-7:1` / `card-7:2`; the declined
/// signal carries the `DECLINE_MARKER` (the engine WITHHOLDS it — `EffectApi::apply` never reached).
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

    // three independent signals buffered on the real engine (one per per-effect key).
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "a partial approval is three independent per-effect signals on the engine"
    );
    // the declined effect's signal carries the DECLINE_MARKER (the engine withholds it — 0 mutation).
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

// ───────────────────────── 4.2: the gate is fail-closed (a denied click posts nothing) ───────────

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

/// **4.2: a denied click posts NO signal onto the engine (fail-closed; the tool stays withheld).** A
/// `Deny` verdict short-circuits before any post — the engine's buffered depth stays 0.
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
        "a denied click posted NO signal — the gate is the chokepoint (the tool stays withheld)"
    );
}

// ───────────────────────── 4.7: the resume token is freshly minted (W-bounded) ──────────────────

/// **4.7: chat mints a FRESH resume token (W-bounded TTL) for a days-later approval.** The resume
/// runs under a fresh attenuated token, not a stale one (the original may be revoked/expired after a
/// multi-day park). The CDC drives the consumer call against the real `mint_run_token` shape.
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

/// The full build_card_signal → engine round-trip for a decline: the signal the chat card builds is
/// exactly the WITHHELD shape the engine's gated loop reads (empty payload + DECLINE_MARKER, AG-8).
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
    // the engine's `apply_approved_effects` reads exactly this: empty payload + DECLINE_MARKER → AG-8.
    assert!(sig.payload.is_empty());
    assert_eq!(
        sig.payload_key_ref.as_deref(),
        Some(myelin_flow::DECLINE_MARKER)
    );
    // chat's DECLINE_MARKER IS the engine's (byte parity — one marker, not two).
    assert_eq!(DECLINE_MARKER, myelin_flow::DECLINE_MARKER);
}
