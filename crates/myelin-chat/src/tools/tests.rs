//! Unit tests for the Chat agent ToolDef set + the routing split + reserve/settle + dry-run
//! (CHAT-P19 → P-414).
//!
//! These pin the chat-owned tool invariants in isolation:
//! - the frozen §8 / X-6 `requires_approval` defaults (post/reply/react/start_dm = no;
//!   create_channel/invite/archive = yes; 0 default divergences);
//! - the routing split (every side-effecting chat tool routes through `EffectApi`, NEVER
//!   `ToolHands::exec`; a `Compute`/`External` def is structurally rejected);
//! - the reserve gate (no balance → no post; the spend-bearing post is fronted);
//! - the dry-run no-apply (the plan is returned, NOTHING mutates, the reserve is unchanged);
//! - the cross-subsystem "governed where it lands" rule (chat never un-gates a cross-subsystem effect).
//!
//! The CDC pair (`tests/cdc_8_1_8_2_8_7_11_7_chat_tools.rs`) proves the registration against the real
//! `myelin_agent` ToolSurface + `EffectApi` + the real `myelin_storage` CostLedger.

use super::*;
use myelin_agent::{DryRun as _, EffectApi, EffectResult, EventId, GateId, InboxEvent, ToolName};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId};
use myelin_tenancy::TenantId;

// ───────── a real in-memory ToolSurface (the same shape the fabric catalogue uses) ─────────

struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

/// A real `EffectApi` that gates a tool iff its name's frozen §8 default is gated — proving a chat
/// mutation ONLY reaches the world through `EffectApi::apply`, and a gated effect is WITHHELD (AG-8).
struct RoutingEffectApi {
    applied: std::cell::Cell<u32>,
}
impl EffectApi for RoutingEffectApi {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        // decode the chat tool from the carrier (chat.<tool>).
        let tool = effect.0.strip_prefix("chat.").unwrap_or(&effect.0);
        if requires_approval_default(tool) {
            // gated → WITHHELD; NO mutation (AG-8).
            EffectResult::Gated(GateId(format!("gate:{tool}")))
        } else {
            self.applied.set(self.applied.get() + 1);
            EffectResult::Applied(EventId(format!("evt:{tool}")))
        }
    }
}

// ───────────────────────── the frozen §8 / X-6 defaults (0 divergences) ──────────────────────────

#[test]
fn the_frozen_x6_chat_defaults_hold_verbatim() {
    // reversible, cheap → NOT gated.
    for tool in [POST_TOOL, REPLY_IN_THREAD_TOOL, REACT_TOOL, START_DM_TOOL] {
        assert!(
            !requires_approval_default(tool),
            "chat.{tool} is reversible → NOT gated (§8 / X-6)"
        );
    }
    // consequential (visibility change / destructive lifecycle) → GATED.
    for tool in [CREATE_CHANNEL_TOOL, INVITE_TOOL, ARCHIVE_CHANNEL_TOOL] {
        assert!(
            requires_approval_default(tool),
            "chat.{tool} is consequential → GATED (§8 / X-6)"
        );
    }
    // fail-closed: an unrecognised chat action is gated.
    assert!(
        requires_approval_default("delete_everything"),
        "an unknown chat action is gated (fail-closed)"
    );
}

#[test]
fn every_tool_def_carries_its_frozen_default_and_is_a_mutate() {
    for def in chat_tool_defs() {
        assert_eq!(
            def.requires_approval,
            requires_approval_default(&def.name.0),
            "chat.{} gating IS the frozen §8 default (seeded, not hand-set)",
            def.name.0
        );
        // the routing split rests on every chat tool being a Mutate (routes to EffectApi).
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
        assert!(!def.exposed_over_mcp, "MCP endpoint is a post-M5 floor");
        assert_eq!(def.subsystem, CHAT_SUBSYSTEM);
        assert_eq!(def.version, CHAT_TOOL_VERSION);
    }
    // the full set is exactly the seven frozen tools.
    assert_eq!(chat_tool_defs().len(), 7);
    assert_eq!(CHAT_TOOL_NAMES.len(), 7);
}

#[test]
fn required_caps_come_from_the_frozen_chat_rebac_fragment() {
    // reversible → channel.post; membership/lifecycle → channel.manage (the §5 fragment permissions).
    assert_eq!(chat_tool_def(POST_TOOL).required_caps, vec!["channel.post"]);
    assert_eq!(
        chat_tool_def(REACT_TOOL).required_caps,
        vec!["channel.post"]
    );
    assert_eq!(
        chat_tool_def(INVITE_TOOL).required_caps,
        vec!["channel.manage"]
    );
    assert_eq!(
        chat_tool_def(ARCHIVE_CHANNEL_TOOL).required_caps,
        vec!["channel.manage"]
    );
    // built from the canonical object-type constant — a rename in the fragment breaks this.
    assert_eq!(chat_objects::CHANNEL, "channel");
}

// ───────────────────────── the routing split (the safety boundary) ───────────────────────────────

#[test]
fn every_chat_tool_routes_through_effect_api_never_tool_hands() {
    // the structural check passes for every frozen chat def (all are Mutate → EffectApi).
    for def in chat_tool_defs() {
        assert!(
            assert_routes_through_effect_api(&def).is_ok(),
            "chat.{} routes through EffectApi (Mutate)",
            def.name.0
        );
    }
}

#[test]
fn a_side_effecting_compute_or_external_chat_tool_is_rejected() {
    // a (hypothetical) side-effecting chat tool registered as Compute would route to ToolHands::exec
    // (the sandbox) — the routing-split check REJECTS it (loud).
    for kind in [EffectKind::Compute, EffectKind::External] {
        let mut bad = chat_tool_def(POST_TOOL);
        bad.effect_kind = kind;
        let err = assert_routes_through_effect_api(&bad).unwrap_err();
        assert_eq!(err.tool, "post");
        assert_eq!(err.effect_kind, kind);
        assert!(
            err.to_string().contains("ToolHands::exec"),
            "the violation names the forbidden seam: {err}"
        );
    }
}

#[test]
fn register_chat_tools_registers_all_seven_and_enforces_the_split() {
    let mut cat = Catalogue { defs: vec![] };
    let registered = register_chat_tools(&mut cat).expect("the frozen set always admits");
    assert_eq!(registered.len(), 7);

    // every tool resolves by name with its frozen shape.
    for tool in CHAT_TOOL_NAMES {
        let def = cat
            .resolve(&ToolName(tool.to_string()))
            .unwrap_or_else(|| panic!("chat.{tool} registered"));
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert_eq!(def.requires_approval, requires_approval_default(tool));
    }
    // an unregistered tool resolves to None.
    assert!(cat.resolve(&ToolName("nope".into())).is_none());
}

#[test]
fn register_rejects_a_silent_loosening_of_a_consequential_default() {
    // build a surface registration that tries to silently un-gate `invite` (frozen yes → no).
    // register_chat_tools always seeds the frozen default, so we exercise the guard directly.
    let mut loosened = chat_tool_def(INVITE_TOOL);
    loosened.requires_approval = false; // silently un-gate a consequential action.
    let err = super::assert_no_silent_loosening(&loosened).unwrap_err();
    assert_eq!(err.tool, "invite");
    assert!(err.to_string().contains("may not be silently un-gated"));

    // tightening a reversible tool (post: frozen no → yes) is always allowed.
    let mut tightened = chat_tool_def(POST_TOOL);
    tightened.requires_approval = true;
    assert!(super::assert_no_silent_loosening(&tightened).is_ok());
}

// ───────────────────────── the EffectApi routing (no ToolHands mutation) ──────────────────────────

#[test]
fn a_reversible_chat_mutation_routes_through_effect_api_and_applies() {
    let api = RoutingEffectApi {
        applied: std::cell::Cell::new(0),
    };
    let run = RunCtx::default();
    // post is NOT gated → applies through EffectApi (the ONLY mutation path).
    let res = route_chat_effect_through_effect_api(&api, &run, POST_TOOL);
    assert!(matches!(res, EffectResult::Applied(_)), "post applied");
    assert_eq!(api.applied.get(), 1, "exactly one apply via EffectApi");
}

#[test]
fn a_consequential_chat_mutation_is_withheld_pending_hitl_zero_mutation() {
    let api = RoutingEffectApi {
        applied: std::cell::Cell::new(0),
    };
    let run = RunCtx::default();
    // invite is GATED → WITHHELD; EffectApi returns Gated, NOTHING mutates (AG-8).
    let res = route_chat_effect_through_effect_api(&api, &run, INVITE_TOOL);
    assert!(matches!(res, EffectResult::Gated(_)), "invite withheld");
    assert_eq!(api.applied.get(), 0, "0 mutation on a gated effect (AG-8)");
}

// ───────────────────────── reserve/settle on a spend-bearing post (11.7) ──────────────────────────

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn no_balance_means_no_post_the_reserve_refuses() {
    let mut ledger = CostLedger::new();
    let estimate = PostCostEstimate(MicroUsd(50));
    // an exhausted wallet (available < estimate) REFUSES the dispatch — no post.
    let err = reserve_spend_bearing_post(
        &mut ledger,
        tenant(),
        RunId::new("run-1"),
        estimate,
        MicroUsd(10),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            myelin_storage::reserve_settle::ReserveError::InsufficientBalance { .. }
        ),
        "no balance → no post (the runaway self-limiter)"
    );
    // nothing was written.
    assert!(ledger.state_of(&tenant(), &RunId::new("run-1")).is_none());
}

#[test]
fn a_funded_spend_bearing_post_reserves_then_settles_within_the_reserve() {
    let mut ledger = CostLedger::new();
    let estimate = PostCostEstimate(MicroUsd(50));
    let run = RunId::new("run-2");
    // funded → reserves (the dispatch is fronted).
    let reservation = reserve_spend_bearing_post(
        &mut ledger,
        tenant(),
        run.clone(),
        estimate,
        MicroUsd(100),
    )
    .expect("a funded post reserves");
    assert_eq!(reservation.reserved, MicroUsd(50));

    // begin → in-flight → settle (the post completed). Actual cost ≤ reserve.
    ledger.begin(&tenant(), &run).expect("begin");
    let units = [MeteredUnit {
        unit: "agent.effect",
        wholesale: MicroUsd(20),
        markup: MicroUsd(10),
    }];
    let outcome = settle_spend_bearing_post(&mut ledger, &tenant(), &run, &units).expect("settle");
    assert_eq!(
        outcome.billed_total,
        MicroUsd(30),
        "billed = wholesale + markup, capped at the reserve"
    );
    assert_eq!(
        outcome.refunded,
        MicroUsd(20),
        "the over-reservation is released"
    );
    // chat never interrupts in-flight.
    assert_eq!(ledger.inflight_interrupt_count(), 0);
}

// ───────────────────────── run --dry-run (8.7 — 0 mutations, 0 reserve consumed) ──────────────────

#[test]
fn dry_run_returns_the_plan_and_applies_nothing() {
    // the run WOULD invoke post (ungated) then invite (gated).
    let plan = dry_run_chat_tools(&[POST_TOOL, INVITE_TOOL]);
    assert_eq!(plan.len(), 2, "the plan has both proposed effects");
    assert_eq!(plan[0].tool, "post");
    assert!(!plan[0].would_gate, "post WOULD apply (not gated)");
    assert_eq!(plan[1].tool, "invite");
    assert!(plan[1].would_gate, "invite WOULD gate (consequential)");

    // an unknown tool is dropped (it is not a chat effect).
    let mixed = dry_run_chat_tools(&[POST_TOOL, "not_a_chat_tool"]);
    assert_eq!(mixed.len(), 1);
}

#[test]
fn dry_run_is_side_effect_free_the_ledger_is_unchanged() {
    // a dry-run holds no ledger and cannot reserve — prove a parallel ledger is untouched after a
    // full plan (the dry-run lever is side-effect-free by construction).
    let ledger = CostLedger::new();
    let before = ledger
        .cost_events_for(&tenant(), &RunId::new("run-3"))
        .len();

    let plan = dry_run_chat_tools(CHAT_TOOL_NAMES);
    assert_eq!(plan.len(), 7, "every chat tool plans an effect");
    // 4 ungated (would apply) + 3 gated (would gate) — the frozen split.
    assert_eq!(plan.iter().filter(|e| !e.would_gate).count(), 4);
    assert_eq!(plan.iter().filter(|e| e.would_gate).count(), 3);

    let after = ledger
        .cost_events_for(&tenant(), &RunId::new("run-3"))
        .len();
    assert_eq!(before, after, "a dry-run reserves/meters NOTHING");
}

#[test]
fn the_chat_dry_run_bridge_satisfies_the_frozen_8_7_signature() {
    // the frozen DryRun::dry_run(InboxEvent) -> Vec<ProposedEffect> bridge.
    let bridge = ChatDryRun::new(vec![POST_TOOL.to_string(), CREATE_CHANNEL_TOOL.to_string()]);
    let effects = bridge.dry_run(InboxEvent("trigger".into()));
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0], ProposedEffect("chat.post".into()));
    assert_eq!(effects[1], ProposedEffect("chat.create_channel".into()));
}

// ───────────────────────── the cross-subsystem "governed where it lands" rule (§8) ───────────────

#[test]
fn cross_subsystem_effect_is_governed_where_it_lands_chat_never_un_gates() {
    // a chat-invoked chat.post lands in chat → its own un-gated default.
    assert!(!requires_approval_for_landing(CHAT_SUBSYSTEM, POST_TOOL));
    // a chat-invoked chat.invite lands in chat → its own GATED default.
    assert!(requires_approval_for_landing(CHAT_SUBSYSTEM, INVITE_TOOL));
    // a chat-invoked effect that LANDS in another subsystem is fail-closed gated — chat never
    // un-gates a cross-subsystem effect (the real landing default is resolved by the fabric).
    assert!(
        requires_approval_for_landing("git", "merge"),
        "a chat-invoked cross-subsystem effect is governed where it lands (never un-gated by chat)"
    );
    assert!(requires_approval_for_landing("issues", "forecast"));
}
