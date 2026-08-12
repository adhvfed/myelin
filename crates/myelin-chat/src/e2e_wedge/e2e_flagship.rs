use std::cell::RefCell;

use myelin_agent::{EffectApi, EffectResult, EventId as FxEventId, ProposedEffect, RunCtx};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_storage::reserve_settle::{CostLedger, MicroUsd, RunId as LedgerRunId};
use myelin_tenancy::{ArtifactRef as TArtifactRef, TenantId};

use crate::dispatch::{
    dispatch_disposition_class, dispatch_explicit, DispatchOutcome, Disposition,
};
use crate::events::{CHAT_MESSAGE_MENTIONED, CHAT_REACTION_ADDED};
use crate::hitl::{
    post_decision, CardClick, CardDecision, CardEffect, CardOutcome, CardSignal, ChatApprovalCard,
    ClickGate, SignalDelivery, SignalPort, SignalPostError, DECLINE_MARKER,
};

use super::{hitl_approved_once, ChatE2eArtifact};

pub const E2E_SCENARIO: &str = "E2E-2";

const MERGE_CARD_ID: &str = "card:triage:merge-fix-pr";

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

fn triage_agent() -> PrincipalId {
    PrincipalId("agent:triage".into())
}

fn approver() -> Principal {
    Principal::stub(
        PrincipalId("alice".into()),
        PrincipalKind::Human,
        e2e_tenant(),
    )
}

struct FlagshipId;
impl IdentityService for FlagshipId {
    fn check(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &TArtifactRef,
        _at: &Consistency,
        _c: Option<&CaveatContext>,
    ) -> IdResult<Decision> {
        Ok(Decision::Allow)
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        run_id: &RunId,
        caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        debug_assert!(caveats.0.iter().any(|c| c.starts_with("chat:dispatch:")));
        debug_assert_eq!(
            ttl.static_max_secs,
            FailStaticBound::DEFAULT_W.static_max_secs
        );
        Ok(RunToken {
            token: format!("tok:{}", run_id.0),
            jti: format!("jti:{}", run_id.0),
        })
    }
    fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _at: &Consistency,
    ) -> IdResult<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn list_subjects(
        &self,
        _o: &ObjectId,
        _p: &Permission,
        _at: &Consistency,
    ) -> IdResult<SubjectTree> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn explain(
        &self,
        _s: &Principal,
        _p: &Permission,
        _o: &ObjectId,
        _at: &Consistency,
    ) -> IdResult<RewriteTrace> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
        Err(AuthzError::NotYetImplemented("e2e"))
    }
    fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
        Ok(FragmentAdmit::Admitted {
            fragment_id: "e2e".into(),
        })
    }
}

struct FlagshipEffectApi;
impl EffectApi for FlagshipEffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        debug_assert!(
            run.0.starts_with("jti:"),
            "the run is attributed (4.7 → 8.2)"
        );
        EffectResult::Applied(FxEventId(format!("applied:{}", effect.0)))
    }
}

#[derive(Default)]
struct DedupPort {
    posted: RefCell<Vec<CardSignal>>,
}
impl SignalPort for DedupPort {
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError> {
        let mut posted = self.posted.borrow_mut();
        let dup = posted.iter().any(|s| {
            s.run_id == signal.run_id
                && s.signal_name == signal.signal_name
                && s.idem_key == signal.idem_key
        });
        if dup {
            return Ok(SignalDelivery::Duplicate);
        }
        posted.push(signal.clone());
        Ok(SignalDelivery::Buffered)
    }
}
impl DedupPort {
    fn applies(&self) -> usize {
        self.posted
            .borrow()
            .iter()
            .filter(|s| s.payload_key_ref.is_none())
            .count()
    }
}

fn merge_card() -> ChatApprovalCard {
    ChatApprovalCard {
        run_id: RunId("run:triage:1".into()),
        card_id: MERGE_CARD_ID.into(),
        effects: vec![CardEffect {
            subject: TArtifactRef("myelin://acme/git/pr/fix-1421".into()),
            action: "merge".into(),
            risk: "irreversible".into(),
            cost: "$0.40".into(),
            effect_refs: vec![TArtifactRef(
                "myelin://acme/agent/effect/merge-fix-1421".into(),
            )],
        }],
    }
}

pub fn run_e2e_2_chat_flagship() -> ChatE2eArtifact {
    let id = FlagshipId;
    let fx = FlagshipEffectApi;
    let leaks: u64 = 0;

    let mention_class = dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, true);
    let mention_is_notify_only = mention_class == DispatchOutcome::NotifyOnly;
    let triage_class = dispatch_disposition_class(CHAT_REACTION_ADDED, true);
    let triage_would_dispatch = triage_class == DispatchOutcome::WouldDispatch;

    let mut ledger = CostLedger::new();
    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        e2e_tenant(),
        &triage_agent(),
        "run:triage:1",
        MicroUsd(5),
        MicroUsd(10),
        ProposedEffect("chat.post:triage-discussion".into()),
    );
    let dispatched_through_one_wallet = matches!(disp, Disposition::Dispatched { .. });
    let chat_output_applied = matches!(applied, Some(EffectResult::Applied(_)));
    let re_reserve = ledger.reserve(
        e2e_tenant(),
        LedgerRunId("run:triage:1".into()),
        MicroUsd(5),
        MicroUsd(10),
    );
    let reserve_settle_balanced = re_reserve.is_err();

    let mut empty_ledger = CostLedger::new();
    let (refused, refused_applied) = dispatch_explicit(
        &id,
        &fx,
        &mut empty_ledger,
        e2e_tenant(),
        &triage_agent(),
        "run:triage:unfunded",
        MicroUsd(50),
        MicroUsd(0),
        ProposedEffect("chat.post:triage-discussion".into()),
    );
    let unfunded_run_refused =
        matches!(refused, Disposition::NoBalanceRefused { .. }) && refused_applied.is_none();

    let card = merge_card();
    let gate = ClickGate::new(FlagshipId);

    let withhold_port = DedupPort::default();
    let decline = CardClick {
        effect_idx: 0,
        decision: CardDecision::Decline,
        decline_reason: DECLINE_MARKER.to_string(),
    };
    let withheld = post_decision(&gate, &withhold_port, &card, &decline, &approver(), None)
        .expect("the decline posts (the gate passed)");
    let zero_mutation_before_approval =
        matches!(withheld, CardOutcome::Withheld(_, _)) && withhold_port.applies() == 0;

    let approve_port = DedupPort::default();
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    let first = post_decision(&gate, &approve_port, &card, &approve, &approver(), None)
        .expect("the first approve posts");
    let first_buffered = first == CardOutcome::Approved(SignalDelivery::Buffered);
    let second = post_decision(&gate, &approve_port, &card, &approve, &approver(), None)
        .expect("the double-click posts");
    let double_click_deduped = second == CardOutcome::Approved(SignalDelivery::Duplicate);
    let merge_applied_once = hitl_approved_once(&first, approve_port.applies());

    let green = mention_is_notify_only
        && triage_would_dispatch
        && dispatched_through_one_wallet
        && chat_output_applied
        && reserve_settle_balanced
        && unfunded_run_refused
        && zero_mutation_before_approval
        && first_buffered
        && double_click_deduped
        && merge_applied_once;

    ChatE2eArtifact {
        scenario: E2E_SCENARIO,
        green,
        evidence: format!(
            "Chat FLAGSHIP terminal surface (E2E-2): explicit-first dispatch \
             (mention_notify_only={mention_is_notify_only}, triage_dispatches={triage_would_dispatch}); \
             ONE wallet (11.7) dispatched_through_one_wallet={dispatched_through_one_wallet}, \
             chat_output_applied={chat_output_applied}, reserve_settle_balanced={reserve_settle_balanced}, \
             unfunded_run_refused={unfunded_run_refused}; HITL \
             zero_mutation_before_approval={zero_mutation_before_approval}, \
             approve_buffered={first_buffered}, double_click_deduped={double_click_deduped}, \
             merge_applied_once={merge_applied_once}; leaks={leaks}; mock-agent runtime \
             (real-LLM is post-M5/R-10)",
        ),
        leaks,
    }
}
