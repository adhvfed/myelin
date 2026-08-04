use super::*;
use myelin_agent::{DryRun as _, EffectApi, EffectResult, EventId, GateId, InboxEvent, ToolName};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, RunId};
use myelin_tenancy::TenantId;

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

struct RoutingEffectApi {
    applied: std::cell::Cell<u32>,
}
impl EffectApi for RoutingEffectApi {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        let tool = effect.0.strip_prefix("chat.").unwrap_or(&effect.0);
        if requires_approval_default(tool) {
            EffectResult::Gated(GateId(format!("gate:{tool}")))
        } else {
            self.applied.set(self.applied.get() + 1);
            EffectResult::Applied(EventId(format!("evt:{tool}")))
        }
    }
}

#[test]
fn the_frozen_x6_chat_defaults_hold_verbatim() {
    for tool in [POST_TOOL, REPLY_IN_THREAD_TOOL, REACT_TOOL, START_DM_TOOL] {
        assert!(
            !requires_approval_default(tool),
            "chat.{tool} is reversible → NOT gated (§8 / X-6)"
        );
    }
    for tool in [CREATE_CHANNEL_TOOL, INVITE_TOOL, ARCHIVE_CHANNEL_TOOL] {
        assert!(
            requires_approval_default(tool),
            "chat.{tool} is consequential → GATED (§8 / X-6)"
        );
    }
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
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
        assert!(!def.exposed_over_mcp, "MCP endpoint is a post-M5 floor");
        assert_eq!(def.subsystem, CHAT_SUBSYSTEM);
        assert_eq!(def.version, CHAT_TOOL_VERSION);
    }
    assert_eq!(chat_tool_defs().len(), 7);
    assert_eq!(CHAT_TOOL_NAMES.len(), 7);
}

#[test]
fn required_caps_come_from_the_frozen_chat_rebac_fragment() {
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
    assert_eq!(chat_objects::CHANNEL, "channel");
}

#[test]
fn every_chat_tool_routes_through_effect_api_never_tool_hands() {
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

    for tool in CHAT_TOOL_NAMES {
        let def = cat
            .resolve(&ToolName(tool.to_string()))
            .unwrap_or_else(|| panic!("chat.{tool} registered"));
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert_eq!(def.requires_approval, requires_approval_default(tool));
    }
    assert!(cat.resolve(&ToolName("nope".into())).is_none());
}

#[test]
fn register_rejects_a_silent_loosening_of_a_consequential_default() {
    let mut loosened = chat_tool_def(INVITE_TOOL);
    loosened.requires_approval = false;
    let err = super::assert_no_silent_loosening(&loosened).unwrap_err();
    assert_eq!(err.tool, "invite");
    assert!(err.to_string().contains("may not be silently un-gated"));

    let mut tightened = chat_tool_def(POST_TOOL);
    tightened.requires_approval = true;
    assert!(super::assert_no_silent_loosening(&tightened).is_ok());
}

#[test]
fn a_reversible_chat_mutation_routes_through_effect_api_and_applies() {
    let api = RoutingEffectApi {
        applied: std::cell::Cell::new(0),
    };
    let run = RunCtx::default();
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
    let res = route_chat_effect_through_effect_api(&api, &run, INVITE_TOOL);
    assert!(matches!(res, EffectResult::Gated(_)), "invite withheld");
    assert_eq!(api.applied.get(), 0, "0 mutation on a gated effect (AG-8)");
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}

#[test]
fn no_balance_means_no_post_the_reserve_refuses() {
    let mut ledger = CostLedger::new();
    let estimate = PostCostEstimate(MicroUsd(50));
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
    assert!(ledger.state_of(&tenant(), &RunId::new("run-1")).is_none());
}

#[test]
fn a_funded_spend_bearing_post_reserves_then_settles_within_the_reserve() {
    let mut ledger = CostLedger::new();
    let estimate = PostCostEstimate(MicroUsd(50));
    let run = RunId::new("run-2");
    let reservation = reserve_spend_bearing_post(
        &mut ledger,
        tenant(),
        run.clone(),
        estimate,
        MicroUsd(100),
    )
    .expect("a funded post reserves");
    assert_eq!(reservation.reserved, MicroUsd(50));

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
    assert_eq!(ledger.inflight_interrupt_count(), 0);
}

#[test]
fn dry_run_returns_the_plan_and_applies_nothing() {
    let plan = dry_run_chat_tools(&[POST_TOOL, INVITE_TOOL]);
    assert_eq!(plan.len(), 2, "the plan has both proposed effects");
    assert_eq!(plan[0].tool, "post");
    assert!(!plan[0].would_gate, "post WOULD apply (not gated)");
    assert_eq!(plan[1].tool, "invite");
    assert!(plan[1].would_gate, "invite WOULD gate (consequential)");

    let mixed = dry_run_chat_tools(&[POST_TOOL, "not_a_chat_tool"]);
    assert_eq!(mixed.len(), 1);
}

#[test]
fn dry_run_is_side_effect_free_the_ledger_is_unchanged() {
    let ledger = CostLedger::new();
    let before = ledger
        .cost_events_for(&tenant(), &RunId::new("run-3"))
        .len();

    let plan = dry_run_chat_tools(CHAT_TOOL_NAMES);
    assert_eq!(plan.len(), 7, "every chat tool plans an effect");
    assert_eq!(plan.iter().filter(|e| !e.would_gate).count(), 4);
    assert_eq!(plan.iter().filter(|e| e.would_gate).count(), 3);

    let after = ledger
        .cost_events_for(&tenant(), &RunId::new("run-3"))
        .len();
    assert_eq!(before, after, "a dry-run reserves/meters NOTHING");
}

#[test]
fn the_chat_dry_run_bridge_satisfies_the_frozen_8_7_signature() {
    let bridge = ChatDryRun::new(vec![POST_TOOL.to_string(), CREATE_CHANNEL_TOOL.to_string()]);
    let effects = bridge.dry_run(InboxEvent("trigger".into()));
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0], ProposedEffect("chat.post".into()));
    assert_eq!(effects[1], ProposedEffect("chat.create_channel".into()));
}

#[test]
fn cross_subsystem_effect_is_governed_where_it_lands_chat_never_un_gates() {
    assert!(!requires_approval_for_landing(CHAT_SUBSYSTEM, POST_TOOL));
    assert!(requires_approval_for_landing(CHAT_SUBSYSTEM, INVITE_TOOL));
    assert!(
        requires_approval_for_landing("git", "merge"),
        "a chat-invoked cross-subsystem effect is governed where it lands (never un-gated by chat)"
    );
    assert!(requires_approval_for_landing("issues", "forecast"));
}
