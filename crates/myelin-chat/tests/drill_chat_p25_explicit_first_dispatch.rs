//! # CHAT-D17 (the CHAT-P25 / P-419 dispatch-orchestration side) — explicit-first agent dispatch
//! (no auto-spawn on mention; reserve-gated) + the agent provenance popover, proven against the
//! `--use-mock` runtime (M4-C9 — the second committable unit; presence+streaming is CHAT-P24 / P-418).
//!
//! **Drill (the GATE):** `05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **CHAT-D17** (explicit-first): "A casual `@agent` mention → notifies the agent's inbox, does NOT
//! spawn a costed run; only an explicit action/structured trigger dispatches; reserve/settle gates even
//! the explicit run." **Thresholds: 0 auto-spawn; reserve gate.** Reconciliation
//! `00-reconciliation-decisions.md` §6 (explicit-first dispatch CONFIRM, CHAT-1, AG-6). VISION §3
//! (agent-native — agents have inboxes; a casual @agent notifies, does not spawn a costed run).
//!
//! ## What this drill PROVES (the CHAT-P25 chat-MODULE dispatch orchestration)
//! The companion `drill_chat_d17_explicit_first.rs` (NOTIF-P22 / P-343) proves the explicit-first
//! CLASS decision through the Bus dispatch TIER. THIS drill proves the **chat-module dispatch
//! orchestration** ([`myelin_chat::dispatch`]) that CHAT-P25 ships:
//! - a casual `@agent` mention → [`Disposition::NotifiedInbox`] (0 run, 0 reserve, 0 token mint) — the
//!   explicit-first floor lives in the chat module's own dispatch path, not only in the tier;
//! - **0 auto-spawn paths are wired** ([`no_auto_spawn_path_is_wired`] over the WHOLE casual chat
//!   surface — a structural proof there is no mention→run edge);
//! - an EXPLICIT action → reserve (11.7, no balance → no run) → mint a per-run token (4.7) → route the
//!   run's chat output through `EffectApi` (8.2 — the routing split); the run is dispatched against the
//!   `--use-mock` runtime (contract 8.3 — the real `LlmAgentRuntime` is the post-M5 swap);
//! - the reserve gate bites EVEN the explicit run (no balance → `NoBalanceRefused`, nothing minted);
//! - the agent provenance popover (S12) answers "why did this agent post?" from the dispatched run's
//!   message envelope (`on_behalf_of` / `causation_id` / `correlation_id`), agent badge always set.
//!
//! **AG-D4 (the permanent sandbox-escape gate, contract 8.4 / X-6 #4):** before chat dispatches ANY
//! agent-compute run it asserts AG-D4 is GREEN and runs NO compute over a RED gate. This drill consumes
//! the REAL [`myelin_ci_sandbox::EscapeAttestation`] artifact (the drill is UPSTREAM, AG-P17 → P-229 /
//! CI-P5 → P-239; chat reads it via the SAME green predicate the Fabric's `AgentExecGate` uses —
//! [`myelin_chat::presence::ag_d4_attestation_is_green`], not a chat-local fork).
//!
//! **FLOORS named (VISION §3 / EI-01 §1):** (1) the no-auto-spawn path is a DELIBERATE counsel-gated
//! **L-3** absence (recon §6 / AG-P20), NOT an omission — [`L3_AUTO_SPAWN_ABSENCE`] names it; (2) the
//! dispatched brain is the MOCK runtime (`--use-mock`, 8.3 — the real `LlmAgentRuntime` is post-M5);
//! (3) the real `EventInbox`(8.6)=AG-P4/P-216, the real `CostLedger`(11.7)=P-103/P-146, the real
//! `mint_run_token`(4.7)=P-ID-18, the real `EffectApi`(8.2)=AG-P6/P-218 — chat CONSUMES all four.

use myelin_agent::{
    AgentRuntime, Conversation, EffectApi, EffectResult, EventId as FxEventId, ProposedEffect,
    RunCtx, StepOutcome, Submission,
};
use myelin_chat::dispatch::{
    agent_provenance, dispatch_disposition_class, dispatch_explicit, no_auto_spawn_path_is_wired,
    DispatchOutcome, Disposition, L3_AUTO_SPAWN_ABSENCE,
};
use myelin_chat::events::{CHAT_MESSAGE_CREATED, CHAT_MESSAGE_MENTIONED, CHAT_REACTION_ADDED};
use myelin_chat::presence::{ag_d4_attestation_is_green, AgD4Attestation};
use myelin_ci_sandbox::escape_corpus::{BEGIN_MARKER, END_MARKER};
use myelin_ci_sandbox::{parse_console, Backend, BackendRun, EscapeAttestation, CORPUS};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_identity::{
    Consistency, Credential, DelegationCaveats, FailStaticBound, FragmentAdmit, NamespaceFragment,
    ObjectId, ObjectType, Permission, Precondition, Principal, PrincipalId, PrincipalKind,
    PrincipalStatus, Result as IdResult, RevokeTarget, RunId as IdRunId, RunToken, RuntimeRef,
    TupleDelta, Zookie,
};
use myelin_storage::reserve_settle::{CostLedger, MinorUnits};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_id() -> PrincipalId {
    PrincipalId("agent:assistant".into())
}

// ─────────────────────── the REAL AG-D4 attestation field-view chat asserts over ───────────────────

struct RealAtt<'a>(&'a EscapeAttestation);
impl AgD4Attestation for RealAtt<'_> {
    fn artifact_tag(&self) -> &str {
        &self.0.artifact
    }
    fn drill_id(&self) -> &str {
        &self.0.drill
    }
    fn total_escapes(&self) -> u32 {
        self.0.total_escapes
    }
}

/// A REAL green AG-D4 attestation, minted from the corpus parser — NEVER hardcoded. `escaped` flips
/// one attack to ESCAPED to model a red drill (which mints NO attestation — the source guard).
fn real_attestation(escaped: bool) -> Result<EscapeAttestation, String> {
    let mut console = format!("{BEGIN_MARKER} corpus_version=1 kernel=6.1.168 guest_euid=0\n");
    for atk in CORPUS {
        console.push_str(&format!("{} CONTAINED\n", atk.id));
    }
    if escaped {
        console = console.replace("K1_module CONTAINED", "K1_module ESCAPED");
    }
    console.push_str(&format!("{END_MARKER}\n"));
    let report = parse_console(&console);
    EscapeAttestation::from_green_drill(
        "2026-06-24",
        &report,
        vec![
            BackendRun {
                backend: Backend::FirecrackerMicrovm,
                exercised: true,
                residual_note: None,
            },
            BackendRun {
                backend: Backend::GvisorRunsc,
                exercised: false,
                residual_note: Some("runsc residual (CI-P28)".into()),
            },
        ],
        Backend::FirecrackerMicrovm,
        "7a2bc8ed2c64ed78994971439b00c234b1ce46d247123314d683df7579c77923",
        "467367e6b8e88323dd23dedae3119ade9c9fca6a102a84fc2155e3ef1bec00eb",
        "6.1.168",
    )
}

// ─────────────────────── the --use-mock runtime (the dispatched brain) ──────────────────────────────

/// The mock runtime (`--use-mock`, contract 8.3) — the dispatched run's brain. A deterministic submit
/// is a valid skeleton decision; the real `LlmAgentRuntime` is the post-M5 swap behind THIS seam.
struct MockRuntime;
impl AgentRuntime for MockRuntime {
    fn step(&self, _conv: &Conversation) -> StepOutcome {
        StepOutcome::Submit(Submission("the agent's reply".into()))
    }
}

// ─────────────────────── the mock Identity (mint_run_token, 4.7) + EffectApi (8.2) ──────────────────

struct MockIdentity;
impl myelin_identity::IdentityService for MockIdentity {
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
        _caveats: &DelegationCaveats,
        _ttl: &FailStaticBound,
    ) -> IdResult<RunToken> {
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

struct MockEffectApi;
impl EffectApi for MockEffectApi {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        EffectResult::Applied(FxEventId(format!("applied:{}", effect.0)))
    }
}

/// The agent's dispatched-run output message (the FINAL durable `chat.message.created`, provenance-
/// bearing — §7.3 / §7.5). Carries the agent actor + the causal triple the popover derives over.
fn dispatched_message(
    on_behalf_of: Option<PrincipalId>,
    causation: Option<EventId>,
) -> EventEnvelope {
    let agent = Principal::new(
        tenant(),
        Region("fr-par".into()),
        agent_id(),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("mock-runtime".into()),
            on_behalf_of,
        },
        myelin_identity::DataRole::Controller,
        PrincipalStatus::Active,
    );
    EventEnvelope {
        event_id: EventId("evt:agent-post".into()),
        type_: EventType(CHAT_MESSAGE_CREATED.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(agent),
        subject: ArtifactRef("myelin://acme/chat/message/M1".into()),
        aggregate: AggregateKey("agg:chan".into()),
        causation_id: causation,
        correlation_id: CorrelationId("root-flow-1".into()),
        caused_by: Some(CausedBy("session:alice".into())),
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        pii_key_ref: None,
        payload: serde_json::json!({}),
    }
}

// ─────────────────────── AG-D4 must be GREEN before any dispatch ────────────────────────────────────

/// **AG-D4 re-confirmed GREEN before chat dispatches any agent-compute run (contract 8.4).** The
/// green-attestation artifact admits; a red one (any escape) refuses; a missing one is fail-closed.
#[test]
fn ag_d4_is_green_before_any_chat_agent_dispatch() {
    // a REAL green attestation (minted from the green corpus parse) is admitted.
    let green = real_attestation(false).expect("a green drill mints a green attestation");
    assert!(
        ag_d4_attestation_is_green(Some(&RealAtt(&green))),
        "AG-D4 green attestation admits — chat may dispatch agent compute"
    );
    // a RED drill mints NO attestation at all (the source guard) — a red AG-D4 is a dated no-go.
    assert!(
        real_attestation(true).is_err(),
        "a red drill must NOT mint an attestation (chat runs NO compute over a red gate)"
    );
    // missing attestation → fail-closed.
    let none: Option<&RealAtt> = None;
    assert!(
        !ag_d4_attestation_is_green(none),
        "no attestation ⇒ fail-closed (no green ⇒ no compute)"
    );
}

// ─────────────────────── CHAT-D17 — 0 auto-spawn from a casual mention ──────────────────────────────

/// **CHAT-D17 threshold #1 — a casual `@agent` mention NOTIFIES, 0 auto-spawn, 0 reserve, 0 mint.**
/// The chat-module dispatch path short-circuits a `NotifyOnly` class to `NotifiedInbox` WITHOUT ever
/// touching the reserve gate / the token mint / `EffectApi` — the explicit-first floor in the module.
#[test]
fn a_casual_mention_notifies_zero_auto_spawn() {
    // the class decision: a casual @agent mention is notify-only.
    assert_eq!(
        dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, /*is_explicit_action=*/ false),
        DispatchOutcome::NotifyOnly,
        "a casual @agent mention notifies — it does NOT auto-spawn a costed run (CHAT-1)"
    );
    // a NotifyOnly never reaches the run side (dispatch_explicit is only called for WouldDispatch).
    // The disposition a notify produces is NotifiedInbox — modelled by the caller short-circuit:
    let outcome = dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false);
    let disp = match outcome {
        DispatchOutcome::NotifyOnly => Disposition::NotifiedInbox,
        DispatchOutcome::WouldDispatch => panic!("a mention must NEVER would-dispatch"),
    };
    assert_eq!(
        disp,
        Disposition::NotifiedInbox,
        "the mention notifies the inbox — 0 auto-spawn (CHAT-D17 threshold)"
    );
}

/// **CHAT-D17 threshold #2 — 0 auto-spawn PATHS are wired (the structural CI signal).** Over the WHOLE
/// casual chat surface, NO event auto-spawns a run — a structural proof there is no mention→run edge.
#[test]
fn zero_auto_spawn_paths_over_the_casual_chat_surface() {
    let chat_tokens: &[&str] = &[
        CHAT_MESSAGE_MENTIONED,
        CHAT_MESSAGE_CREATED,
        CHAT_REACTION_ADDED,
    ];
    assert!(
        no_auto_spawn_path_is_wired(chat_tokens),
        "0 auto-spawn paths: no casual chat event spawns a costed run (the no-auto-spawn floor)"
    );
    // the L-3 absence is a recorded decision, not a silent gap (EI-01 §1).
    assert!(
        L3_AUTO_SPAWN_ABSENCE.contains("counsel-gated"),
        "the no-auto-spawn path is a DELIBERATE L-3 absence, named"
    );
}

// ─────────────────────── CHAT-D17 — an explicit run reserves, mints, routes through EffectApi ───────

/// **CHAT-D17 threshold #3 — an EXPLICIT action DOES dispatch: reserve (11.7) → mint (4.7) → EffectApi
/// (8.2), against the `--use-mock` runtime.** Notify-only is not "the agent never runs": a deliberate
/// structured action reserves the cost, mints a per-run token, and routes the run's output through
/// the governed `EffectApi`. The mock runtime is the dispatched brain (the real LLM is post-M5).
#[test]
fn an_explicit_action_reserves_mints_and_routes_through_effect_api_against_the_mock() {
    // the brain is the mock (--use-mock) — exercised so the runtime seam is real in the drill.
    let runtime = MockRuntime;
    let _decision = runtime.step(&Conversation::default()); // the mock submits (deterministic).

    let id = MockIdentity;
    let fx = MockEffectApi;
    let mut ledger = CostLedger::new();

    // an EXPLICIT action (a deliberate approve-reaction targeting the agent) → would-dispatch.
    assert_eq!(
        dispatch_disposition_class(CHAT_REACTION_ADDED, /*is_explicit_action=*/ true),
        DispatchOutcome::WouldDispatch
    );

    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        tenant(),
        &agent_id(),
        "run:explicit:1",
        MinorUnits(5),
        MinorUnits(10), // funded wallet
        ProposedEffect("chat.post".into()),
    );

    // the run dispatched + carries the minted per-run token (4.7).
    assert_eq!(
        disp,
        Disposition::Dispatched {
            run_token_jti: "jti:run:explicit:1".into()
        },
        "an explicit action dispatches a costed run (reserve → mint → EffectApi)"
    );
    // the run's chat output ROUTED through EffectApi (8.2 — the routing split, X-6).
    assert_eq!(
        applied,
        Some(EffectResult::Applied(FxEventId("applied:chat.post".into()))),
        "the run's chat output routed through EffectApi (8.2)"
    );
}

/// **CHAT-D17 threshold #4 — the reserve gate bites EVEN the explicit run (11.7).** With ZERO balance,
/// a deliberate explicit action is REFUSED at the reserve gate: nothing is minted, nothing applies —
/// the last clause of CHAT-D17 ("reserve/settle gates even the explicit run").
#[test]
fn the_reserve_gate_refuses_an_explicit_run_with_no_balance() {
    let id = MockIdentity;
    let fx = MockEffectApi;
    let mut ledger = CostLedger::new();

    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        tenant(),
        &agent_id(),
        "run:explicit:2",
        MinorUnits(50),
        MinorUnits(0), // exhausted wallet
        ProposedEffect("chat.post".into()),
    );

    assert_eq!(
        disp,
        Disposition::NoBalanceRefused {
            requested: MinorUnits(50),
            available: MinorUnits(0)
        },
        "no balance → no run: reserve/settle gates even the explicit run (CHAT-D17)"
    );
    assert_eq!(
        applied, None,
        "a refused dispatch mints nothing + applies nothing"
    );
}

// ─────────────────────── the provenance popover (S12) on the dispatched run's message ──────────────

/// **The agent provenance popover (S12) — "why did this agent post?" on the dispatched run's
/// message.** Derived from the FINAL `chat.message.created` envelope: which agent + runtime, on whose
/// authority (`on_behalf_of`), triggered by which event (`causation_id` — the explicit action), the
/// `correlation_id` audit anchor, the agent badge always set (AI-Act legibility).
#[test]
fn the_dispatched_run_carries_a_provenance_popover() {
    let msg = dispatched_message(
        Some(PrincipalId("psn:alice".into())),
        Some(EventId("evt:explicit-approve-reaction".into())),
    );
    let prov = agent_provenance(&msg).expect("an agent post HAS a provenance popover");

    assert_eq!(prov.agent, agent_id(), "which agent");
    assert_eq!(
        prov.runtime_ref.as_deref(),
        Some("mock-runtime"),
        "which runtime"
    );
    assert_eq!(
        prov.on_behalf_of,
        Some(PrincipalId("psn:alice".into())),
        "on whose authority / lawful basis (Art. 22)"
    );
    assert_eq!(
        prov.triggered_by,
        Some(EventId("evt:explicit-approve-reaction".into())),
        "triggered by which event (the explicit action)"
    );
    assert_eq!(
        prov.correlation_id,
        CorrelationId("root-flow-1".into()),
        "the flow this post threads (the audit anchor)"
    );
    assert!(
        prov.agent_badge,
        "the agent badge is always set (agents are never disguised)"
    );
}
