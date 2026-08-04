use myelin_agent::{
    DryRun, EffectApi, EffectKind, EffectResult, EventId, GateId, InboxEvent, ProposedEffect,
    RunCtx, ToolDef, ToolName, ToolSurface,
};
use myelin_chat::tools::{
    chat_tool_def, dry_run_chat_tools, register_chat_tools, requires_approval_default,
    reserve_spend_bearing_post, route_chat_effect_through_effect_api, settle_spend_bearing_post,
    ChatDryRun, PostCostEstimate, ARCHIVE_CHANNEL_TOOL, CHAT_TOOL_NAMES, CREATE_CHANNEL_TOOL,
    INVITE_TOOL, POST_TOOL, REACT_TOOL, REPLY_IN_THREAD_TOOL, START_DM_TOOL,
};
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MicroUsd, ReserveError, RunId};
use myelin_tenancy::TenantId;

struct FabricCatalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for FabricCatalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

struct FabricEffectApi {
    applies: std::cell::Cell<u32>,
}
impl EffectApi for FabricEffectApi {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        let tool = effect.0.strip_prefix("chat.").unwrap_or(&effect.0);
        if requires_approval_default(tool) {
            EffectResult::Gated(GateId(format!("gate:{tool}")))
        } else {
            self.applies.set(self.applies.get() + 1);
            EffectResult::Applied(EventId(format!("evt:{tool}")))
        }
    }
}

#[test]
fn cdc_8_1_chat_tool_set_registers_into_the_real_tool_surface() {
    let mut cat = FabricCatalogue { defs: vec![] };
    let registered = register_chat_tools(&mut cat).expect("the frozen set always admits");
    assert_eq!(registered.len(), 7, "the seven frozen chat tools");

    let expected_gated = [
        (POST_TOOL, false),
        (REPLY_IN_THREAD_TOOL, false),
        (REACT_TOOL, false),
        (START_DM_TOOL, false),
        (CREATE_CHANNEL_TOOL, true),
        (INVITE_TOOL, true),
        (ARCHIVE_CHANNEL_TOOL, true),
    ];
    for (tool, gated) in expected_gated {
        let def = cat
            .resolve(&ToolName(tool.to_string()))
            .unwrap_or_else(|| panic!("chat.{tool} registered"));
        assert_eq!(def.subsystem, "chat");
        assert_eq!(
            def.requires_approval, gated,
            "chat.{tool} frozen §8 default"
        );
        assert_eq!(
            def.effect_kind,
            EffectKind::Mutate,
            "chat.{tool} is a Mutate"
        );
        assert!(def.side_effecting);
        assert!(!def.exposed_over_mcp);
    }
}

#[test]
fn cdc_8_1_routing_split_rejects_a_tool_hands_routed_chat_mutation() {
    let mut bad = chat_tool_def(POST_TOOL);
    bad.effect_kind = EffectKind::Compute;
    assert!(
        myelin_chat::tools::assert_routes_through_effect_api(&bad).is_err(),
        "a side-effecting chat tool that routes through ToolHands::exec is REJECTED (the routing split)"
    );
    assert!(
        myelin_chat::tools::assert_routes_through_effect_api(&chat_tool_def(POST_TOOL)).is_ok()
    );
}

#[test]
fn cdc_8_2_chat_mutation_routes_through_effect_api_gated_withholds() {
    let api = FabricEffectApi {
        applies: std::cell::Cell::new(0),
    };
    let run = RunCtx::default();

    let post = route_chat_effect_through_effect_api(&api, &run, POST_TOOL);
    assert!(matches!(post, EffectResult::Applied(_)));

    let invite = route_chat_effect_through_effect_api(&api, &run, INVITE_TOOL);
    assert!(matches!(invite, EffectResult::Gated(_)));

    assert_eq!(api.applies.get(), 1, "0 mutation on the gated effect");
}

#[test]
fn cdc_8_7_dry_run_returns_the_plan_without_applying() {
    let bridge = ChatDryRun::new(vec![POST_TOOL.to_string(), INVITE_TOOL.to_string()]);
    let effects: Vec<ProposedEffect> = bridge.dry_run(InboxEvent("trigger".into()));
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0], ProposedEffect("chat.post".into()));
    assert_eq!(effects[1], ProposedEffect("chat.invite".into()));

    let plan = dry_run_chat_tools(&[POST_TOOL, INVITE_TOOL]);
    assert!(!plan[0].would_gate);
    assert!(plan[1].would_gate);

    let full = dry_run_chat_tools(CHAT_TOOL_NAMES);
    assert_eq!(full.iter().filter(|e| !e.would_gate).count(), 4);
    assert_eq!(full.iter().filter(|e| e.would_gate).count(), 3);
}

#[test]
fn cdc_11_7_reserve_settle_fronts_a_spend_bearing_post() {
    let tenant = TenantId("acme".into());
    let mut ledger = CostLedger::new();
    let estimate = PostCostEstimate(MicroUsd(50));

    let refused = reserve_spend_bearing_post(
        &mut ledger,
        tenant.clone(),
        RunId::new("run-broke"),
        estimate,
        MicroUsd(0),
    );
    assert!(matches!(
        refused,
        Err(ReserveError::InsufficientBalance { .. })
    ));

    let run = RunId::new("run-ok");
    reserve_spend_bearing_post(
        &mut ledger,
        tenant.clone(),
        run.clone(),
        estimate,
        MicroUsd(100),
    )
    .expect("a funded post reserves");
    ledger.begin(&tenant, &run).expect("begin in-flight");
    let units = [MeteredUnit {
        unit: "agent.effect",
        wholesale: MicroUsd(30),
        markup: MicroUsd(5),
    }];
    let outcome = settle_spend_bearing_post(&mut ledger, &tenant, &run, &units).expect("settle");
    assert_eq!(
        outcome.billed_total,
        MicroUsd(35),
        "billed = wholesale + markup"
    );
    assert!(
        outcome.billed_total <= MicroUsd(50),
        "settle never exceeds the reserve (the gate's whole point)"
    );
    assert_eq!(ledger.inflight_interrupt_count(), 0);
}
