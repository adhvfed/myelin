//! # CDC — Chat agent ToolDef set + EffectApi routing + reserve/settle + dry-run (CHAT-P19 → P-414)
//!
//! Pins the Chat tool slice against the FROZEN agent-fabric + storage contracts (no local divergence):
//! - **8.1** `ToolSurface::register_tool(ToolDef)` — PROVIDER: chat's `register_chat_tools` registers
//!   the FULL frozen X-6 set into the REAL `myelin_agent::ToolSurface`; resolve-by-name carries every
//!   frozen field (the §8 `requires_approval` split + the 4.9 `required_caps` + `effect_kind = Mutate`).
//! - **8.2** `EffectApi::apply` — CONSUMER: every chat MUTATION routes through the REAL
//!   `myelin_agent::EffectApi` (the routing split: `effect_kind = Mutate`, NEVER `ToolHands::exec`); a
//!   gated chat effect is WITHHELD (`Gated`, 0 mutation, AG-8).
//! - **8.7** `run --dry-run` — PROVIDER: chat's `ChatDryRun` satisfies the frozen
//!   `DryRun::dry_run(InboxEvent) -> Vec<ProposedEffect>`; the plan is returned, NOTHING applies.
//! - **11.7** reserve/settle — CONSUMER: chat fronts a spend-bearing post through the REAL
//!   `myelin_storage::reserve_settle::CostLedger` (no balance → no post; settle ≤ reserve).
//!
//! The routing split is the safety boundary (X-6): a chat tool registered as `Compute`/`External`
//! (which would route to `ToolHands::exec`) is structurally REJECTED at registration.

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
use myelin_storage::reserve_settle::{CostLedger, MeteredUnit, MinorUnits, ReserveError, RunId};
use myelin_tenancy::TenantId;

/// The REAL fabric catalogue shape (a `myelin_agent::ToolSurface` impl) — the PROVIDER the 8.1 CDC
/// registers into. This is the same trait the fabric's persisted catalogue implements.
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

/// A REAL `myelin_agent::EffectApi` (the plan-then-apply seam, 8.2): gates iff the chat tool's frozen
/// §8 default is gated; a gated effect is WITHHELD (returns `Gated`, 0 mutation, AG-8).
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

/// **8.1 — chat registers the FULL frozen X-6 set into the REAL ToolSurface; every field round-trips.**
#[test]
fn cdc_8_1_chat_tool_set_registers_into_the_real_tool_surface() {
    let mut cat = FabricCatalogue { defs: vec![] };
    let registered = register_chat_tools(&mut cat).expect("the frozen set always admits");
    assert_eq!(registered.len(), 7, "the seven frozen chat tools");

    // the frozen §8 split — post/reply/react/start_dm = no; create_channel/invite/archive = yes.
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
        // the routing split rests on Mutate (never Compute/External → never ToolHands::exec).
        assert_eq!(
            def.effect_kind,
            EffectKind::Mutate,
            "chat.{tool} is a Mutate"
        );
        assert!(def.side_effecting);
        assert!(!def.exposed_over_mcp);
    }
}

/// **8.1 routing split — a side-effecting chat tool registered as Compute/External is REJECTED.** A
/// `Compute`/`External` def would route to `ToolHands::exec` (the sandbox); the registration guard
/// forbids a chat MUTATION on that seam (the safety boundary, X-6).
#[test]
fn cdc_8_1_routing_split_rejects_a_tool_hands_routed_chat_mutation() {
    // a hand-built bad def (post, but routed as Compute) — register_chat_tools only ships Mutate, so
    // we assert the guard directly via the public structural check.
    let mut bad = chat_tool_def(POST_TOOL);
    bad.effect_kind = EffectKind::Compute;
    assert!(
        myelin_chat::tools::assert_routes_through_effect_api(&bad).is_err(),
        "a side-effecting chat tool that routes through ToolHands::exec is REJECTED (the routing split)"
    );
    // sanity: the real frozen def passes.
    assert!(
        myelin_chat::tools::assert_routes_through_effect_api(&chat_tool_def(POST_TOOL)).is_ok()
    );
}

/// **8.2 — every chat mutation routes through the REAL EffectApi; a gated effect is WITHHELD (0
/// mutation, AG-8).** A reversible post APPLIES; a consequential invite is GATED (withheld, no mutate).
#[test]
fn cdc_8_2_chat_mutation_routes_through_effect_api_gated_withholds() {
    let api = FabricEffectApi {
        applies: std::cell::Cell::new(0),
    };
    let run = RunCtx::default();

    // post (ungated) → Applied via EffectApi (the ONLY mutation path).
    let post = route_chat_effect_through_effect_api(&api, &run, POST_TOOL);
    assert!(matches!(post, EffectResult::Applied(_)));

    // invite (gated) → Gated (withheld); NOTHING mutates (AG-8).
    let invite = route_chat_effect_through_effect_api(&api, &run, INVITE_TOOL);
    assert!(matches!(invite, EffectResult::Gated(_)));

    // exactly one apply happened (the post); the gated invite did NOT mutate.
    assert_eq!(api.applies.get(), 1, "0 mutation on the gated effect");
}

/// **8.7 — `run --dry-run` returns the proposed-effect plan WITHOUT applying any (0 mutation).** The
/// frozen `DryRun::dry_run(InboxEvent) -> Vec<ProposedEffect>` bridge returns the plan; each effect
/// carries its frozen would-gate verdict.
#[test]
fn cdc_8_7_dry_run_returns_the_plan_without_applying() {
    let bridge = ChatDryRun::new(vec![POST_TOOL.to_string(), INVITE_TOOL.to_string()]);
    let effects: Vec<ProposedEffect> = bridge.dry_run(InboxEvent("trigger".into()));
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0], ProposedEffect("chat.post".into()));
    assert_eq!(effects[1], ProposedEffect("chat.invite".into()));

    // the verdict-carrying form: post WOULD apply, invite WOULD gate.
    let plan = dry_run_chat_tools(&[POST_TOOL, INVITE_TOOL]);
    assert!(!plan[0].would_gate);
    assert!(plan[1].would_gate);

    // a dry-run over the whole set: 4 would-apply + 3 would-gate (the frozen split), 0 mutation.
    let full = dry_run_chat_tools(CHAT_TOOL_NAMES);
    assert_eq!(full.iter().filter(|e| !e.would_gate).count(), 4);
    assert_eq!(full.iter().filter(|e| e.would_gate).count(), 3);
}

/// **11.7 — reserve/settle fronts a spend-bearing post through the REAL CostLedger: no balance → no
/// post; settle ≤ reserve.**
#[test]
fn cdc_11_7_reserve_settle_fronts_a_spend_bearing_post() {
    let tenant = TenantId("acme".into());
    let mut ledger = CostLedger::new();
    let estimate = PostCostEstimate(MinorUnits(50));

    // no balance → no post (the reserve REFUSES; nothing is written).
    let refused = reserve_spend_bearing_post(
        &mut ledger,
        tenant.clone(),
        RunId::new("run-broke"),
        estimate,
        MinorUnits(0),
    );
    assert!(matches!(
        refused,
        Err(ReserveError::InsufficientBalance { .. })
    ));

    // funded → reserves, then settles within the reserve.
    let run = RunId::new("run-ok");
    reserve_spend_bearing_post(
        &mut ledger,
        tenant.clone(),
        run.clone(),
        estimate,
        MinorUnits(100),
    )
    .expect("a funded post reserves");
    ledger.begin(&tenant, &run).expect("begin in-flight");
    let units = [MeteredUnit {
        unit: "agent.effect",
        wholesale: MinorUnits(30),
        markup: MinorUnits(5),
    }];
    let outcome = settle_spend_bearing_post(&mut ledger, &tenant, &run, &units).expect("settle");
    assert_eq!(
        outcome.billed_total,
        MinorUnits(35),
        "billed = wholesale + markup"
    );
    assert!(
        outcome.billed_total <= MinorUnits(50),
        "settle never exceeds the reserve (the gate's whole point)"
    );
    // chat never interrupts in-flight.
    assert_eq!(ledger.inflight_interrupt_count(), 0);
}
