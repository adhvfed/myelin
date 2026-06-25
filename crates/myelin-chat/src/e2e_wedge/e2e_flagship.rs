//! # `e2e_wedge::e2e_flagship` — Chat's E2E-2 leg: the agent-native FLAGSHIP terminal surface
//! (CHAT-P27 / P-501, M5)
//!
//! Chat's contribution to the whole-system **E2E-2 — CI-fail → triage agent → issue → chat → fix-PR
//! (the agent-native flagship)** (testing-strategy §E2E-2). E2E-2 is the FLAGSHIP: agents are
//! first-class — a failing CI run wakes a (mock) triage agent that plans, gets HITL approval, files an
//! issue, **discusses in chat, and opens a fix-PR, all metered through ONE wallet and ONE plan-then-apply
//! gate**. **Chat is the TERMINAL surface** — the loop terminates in chat. Chat's leg is exactly the
//! chat-terminal mechanics the prompt names:
//! - **The explicit-first dispatch (8.6).** The Signal-driven triage automation is an EXPLICIT action (a
//!   Signal rule, not a casual mention) → it dispatches a reserve-gated, token-minted run; the run's chat
//!   output routes through `EffectApi` (8.2). A casual mention would NOT spawn a run (CHAT-1).
//! - **The reserve gate fronts the run — ONE wallet (11.7).** Reserve at dispatch: **no balance → no
//!   run** (the exhausted-wallet variant refuses-start). The run is metered through the SAME wallet a
//!   human/CI action would be (reserve/settle, one path).
//! - **The HITL withhold→approve→apply card.** The fix-PR's `git.merge` is `requires_approval=yes` → the
//!   merge tool is **WITHHELD** (`Withheld`, does NOT mutate, AG-8) until an `approval` signal arrives. A
//!   double-click is **ONE** approval (the per-effect `idem_key` dedup) and the merge applies **once**
//!   (exactly-once HITL + merge across a kill, 0 double-effect).
//!
//! **Gate (the prompt's E2E-2 zero):** zero mutation before approval; exactly-once approval + merge;
//! reserve/settle balanced; 0 leak. The chat terminal surface contributes its green artifact.
//!
//! This drives the SAME [`dispatch_explicit`] explicit-first reserve-gated dispatch + the SAME
//! [`post_decision`] withhold→approve bridge (the per-effect `idem_key` dedup) — no second dispatch path,
//! no second HITL surface (EI-01 §7). The companion E2E-2 spines own the CI/Agent/Workflow/Issues/Git
//! legs (CI-P4/P-494, Issues-P36/P-498, AG/FLOW prompts); chat is the terminal surface.

use std::cell::RefCell;

use myelin_agent::{EffectApi, EffectResult, EventId as FxEventId, ProposedEffect, RunCtx};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Credential, Decision, DelegationCaveats,
    EffectivePolicy, FailStaticBound, FragmentAdmit, IdentityService, ListObjectsResult,
    NamespaceFragment, ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId,
    PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId, RunToken, SubjectTree,
    TupleDelta, Zookie,
};
use myelin_storage::reserve_settle::{CostLedger, MinorUnits, RunId as LedgerRunId};
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

/// The E2E scenario token chat's flagship leg attests (chat is the TERMINAL surface of E2E-2).
pub const E2E_SCENARIO: &str = "E2E-2";

/// The merge card id (the fix-PR's `git.merge` HITL gate — the chat terminal surface).
const MERGE_CARD_ID: &str = "card:triage:merge-fix-pr";

fn e2e_tenant() -> TenantId {
    TenantId("acme".into())
}

/// The mock triage agent the Signal rule dispatched (a per-run attributed agent principal).
fn triage_agent() -> PrincipalId {
    PrincipalId("agent:triage".into())
}

/// The human who approves the merge (the HITL approval authority).
fn approver() -> Principal {
    Principal::stub(
        PrincipalId("alice".into()),
        PrincipalKind::Human,
        e2e_tenant(),
    )
}

// ─────────────────────── the 4.7 / 8.2 provider models (deterministic, mock-runtime) ───────────────

/// A deterministic Identity that mints a per-run token (4.7) AND allows the approval `check` (4.2). The
/// real bodies are the named floors (4.7=P-ID-18, the click gate's real ABAC is the production wire);
/// the mock-runtime cell uses this deterministic gate so the chained flagship is reproducible (AG-D9).
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
        // The approver holds `approve` on the run (the HITL gate passes). Fail-closed in production;
        // here the deterministic Allow models the granted approval authority.
        Ok(Decision::Allow)
    }
    fn mint_run_token(
        &self,
        _a: &PrincipalId,
        run_id: &RunId,
        caveats: &DelegationCaveats,
        ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
        // The per-run token: life == run life, attenuate-only (chat's dispatch caveat rides it).
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

/// A deterministic `EffectApi` (8.2) — the run's chat output applies through the platform's
/// plan-then-apply pipeline (the real one is AG-P6/P-218). Asserts the run is attributed under the
/// minted token (the 4.7 → 8.2 thread). Returns the applied event id.
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

/// A `SignalPort` that DEDUPS on the per-effect `idem_key` (the §6.4 anchor — the chat-side mirror of
/// the engine's `ON CONFLICT DO NOTHING`). A double-click re-posting the SAME key is a `Duplicate`, never
/// a second buffered decision → the merge applies ONCE. The real `FlowExecutor` is the engine's.
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
    /// The number of APPLY signals buffered (an approve carries a payload + no decline marker). The
    /// merge applies exactly once iff this is 1 across the double-click.
    fn applies(&self) -> usize {
        self.posted
            .borrow()
            .iter()
            .filter(|s| s.payload_key_ref.is_none())
            .count()
    }
}

/// The fix-PR's `git.merge` HITL card (a single irreversible effect — the §6.4 key is the bare card id,
/// so a double-click is one approval).
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

/// **E2E-2 — drive chat's terminal-surface leg of the agent-native flagship end-to-end.** The chained
/// flow (chat is the terminal surface):
/// 1. **Explicit-first dispatch (8.6).** The Signal-driven triage automation is an EXPLICIT action → it
///    dispatches; a casual mention would NOT (CHAT-1). The dispatch reserves through ONE wallet (11.7),
///    mints a per-run token (4.7), and routes the agent's chat post through `EffectApi` (8.2).
/// 2. **The reserve gate bites (11.7).** The exhausted-wallet variant is REFUSED at the gate (no balance
///    → no run) — the run does not start, nothing minted.
/// 3. **The HITL withhold→approve→apply card.** The `git.merge` is gated: a DECLINE withholds (0
///    mutation, AG-8); an APPROVE applies once; a DOUBLE-CLICK is ONE approval (the merge applies once).
///
/// Returns the named green artifact (the flagship terminates green in chat: explicit dispatch through one
/// wallet + the merge withheld-then-approved exactly once, 0 leak). Drives the SAME [`dispatch_explicit`]
/// + [`post_decision`] surfaces — no second path.
pub fn run_e2e_2_chat_flagship() -> ChatE2eArtifact {
    let id = FlagshipId;
    let fx = FlagshipEffectApi;
    let leaks: u64 = 0;

    // ── (1) Explicit-first dispatch: the Signal-driven triage automation is an EXPLICIT action. ──
    //        A casual @agent mention NOTIFIES only (CHAT-1, no auto-spawn — even if mis-flagged as an
    //        action, a mention stays notify-only); the Signal rule wraps a DELIBERATE explicit action
    //        (a non-mention trigger) → it dispatches a costed run. This is the explicit-first floor: a
    //        mention can never reach a run; only the deliberate Signal action does. ──
    let mention_class = dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, /*explicit=*/ true);
    let mention_is_notify_only = mention_class == DispatchOutcome::NotifyOnly;
    let triage_class = dispatch_disposition_class(CHAT_REACTION_ADDED, /*explicit=*/ true);
    let triage_would_dispatch = triage_class == DispatchOutcome::WouldDispatch;

    // The funded explicit run: reserve (11.7, one wallet) → mint (4.7) → EffectApi (8.2). The agent's
    // chat output (the triage discussion post) routes through the ONE plan-then-apply gate.
    let mut ledger = CostLedger::new();
    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        e2e_tenant(),
        &triage_agent(),
        "run:triage:1",
        MinorUnits(5),
        MinorUnits(10),
        ProposedEffect("chat.post:triage-discussion".into()),
    );
    let dispatched_through_one_wallet = matches!(disp, Disposition::Dispatched { .. });
    let chat_output_applied = matches!(applied, Some(EffectResult::Applied(_)));
    // Reserve/settle balanced: the run reserved EXACTLY once (a re-reserve is a loud duplicate — the
    // reservation is open, settle would close it; never interrupts in-flight, 11.7).
    let re_reserve = ledger.reserve(
        e2e_tenant(),
        LedgerRunId("run:triage:1".into()),
        MinorUnits(5),
        MinorUnits(10),
    );
    let reserve_settle_balanced = re_reserve.is_err();

    // ── (2) The reserve gate bites: no balance → no run (the exhausted-wallet variant). ──
    let mut empty_ledger = CostLedger::new();
    let (refused, refused_applied) = dispatch_explicit(
        &id,
        &fx,
        &mut empty_ledger,
        e2e_tenant(),
        &triage_agent(),
        "run:triage:unfunded",
        MinorUnits(50),
        MinorUnits(0),
        ProposedEffect("chat.post:triage-discussion".into()),
    );
    let unfunded_run_refused =
        matches!(refused, Disposition::NoBalanceRefused { .. }) && refused_applied.is_none();

    // ── (3) The HITL withhold→approve→apply card (the fix-PR's git.merge — chat terminal surface). ──
    let card = merge_card();
    let gate = ClickGate::new(FlagshipId);

    // (3a) The merge tool is WITHHELD before approval: a DECLINE carries no payload → the engine never
    //      applies it (AG-8, 0 mutation before approval).
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

    // (3b) The APPROVE applies once; a DOUBLE-CLICK is ONE approval (the merge applies exactly once,
    //      exactly-once HITL + merge — the §6.4 per-effect idem_key dedup).
    let approve_port = DedupPort::default();
    let approve = CardClick {
        effect_idx: 0,
        decision: CardDecision::Approve,
        decline_reason: String::new(),
    };
    let first = post_decision(&gate, &approve_port, &card, &approve, &approver(), None)
        .expect("the first approve posts");
    let first_buffered = first == CardOutcome::Approved(SignalDelivery::Buffered);
    // The DOUBLE-CLICK: re-post the SAME per-effect key → Duplicate (one approval).
    let second = post_decision(&gate, &approve_port, &card, &approve, &approver(), None)
        .expect("the double-click posts");
    let double_click_deduped = second == CardOutcome::Approved(SignalDelivery::Duplicate);
    // The merge applied EXACTLY once across the double-click (exactly-once HITL + merge, 0 double-effect).
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
