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
use myelin_storage::reserve_settle::{CostLedger, MicroUsd};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent_id() -> PrincipalId {
    PrincipalId("agent:assistant".into())
}

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

struct MockRuntime;
impl AgentRuntime for MockRuntime {
    fn step(&self, _conv: &Conversation) -> StepOutcome {
        StepOutcome::Submit(Submission("the agent's reply".into()))
    }
}

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

#[test]
fn ag_d4_is_green_before_any_chat_agent_dispatch() {
    let green = real_attestation(false).expect("a green drill mints a green attestation");
    assert!(
        ag_d4_attestation_is_green(Some(&RealAtt(&green))),
        "AG-D4 green attestation admits - chat may dispatch agent compute"
    );
    assert!(
        real_attestation(true).is_err(),
        "a red drill must NOT mint an attestation (chat runs NO compute over a red gate)"
    );
    let none: Option<&RealAtt> = None;
    assert!(
        !ag_d4_attestation_is_green(none),
        "no attestation ⇒ fail-closed (no green ⇒ no compute)"
    );
}

#[test]
fn a_casual_mention_notifies_zero_auto_spawn() {
    assert_eq!(
        dispatch_disposition_class(CHAT_MESSAGE_MENTIONED,  false),
        DispatchOutcome::NotifyOnly,
        "a casual @agent mention notifies - it does NOT auto-spawn a costed run (CHAT-1)"
    );
    let outcome = dispatch_disposition_class(CHAT_MESSAGE_MENTIONED, false);
    let disp = match outcome {
        DispatchOutcome::NotifyOnly => Disposition::NotifiedInbox,
        DispatchOutcome::WouldDispatch => panic!("a mention must NEVER would-dispatch"),
    };
    assert_eq!(
        disp,
        Disposition::NotifiedInbox,
        "the mention notifies the inbox - 0 auto-spawn (CHAT-D17 threshold)"
    );
}

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
    assert!(
        L3_AUTO_SPAWN_ABSENCE.contains("counsel-gated"),
        "the no-auto-spawn path is a DELIBERATE L-3 absence, named"
    );
}

#[test]
fn an_explicit_action_reserves_mints_and_routes_through_effect_api_against_the_mock() {
    let runtime = MockRuntime;
    let _decision = runtime.step(&Conversation::default());

    let id = MockIdentity;
    let fx = MockEffectApi;
    let mut ledger = CostLedger::new();

    assert_eq!(
        dispatch_disposition_class(CHAT_REACTION_ADDED,  true),
        DispatchOutcome::WouldDispatch
    );

    let (disp, applied) = dispatch_explicit(
        &id,
        &fx,
        &mut ledger,
        tenant(),
        &agent_id(),
        "run:explicit:1",
        MicroUsd(5),
        MicroUsd(10),
        ProposedEffect("chat.post".into()),
    );

    assert_eq!(
        disp,
        Disposition::Dispatched {
            run_token_jti: "jti:run:explicit:1".into()
        },
        "an explicit action dispatches a costed run (reserve → mint → EffectApi)"
    );
    assert_eq!(
        applied,
        Some(EffectResult::Applied(FxEventId("applied:chat.post".into()))),
        "the run's chat output routed through EffectApi (8.2)"
    );
}

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
        "no balance → no run: reserve/settle gates even the explicit run (CHAT-D17)"
    );
    assert_eq!(
        applied, None,
        "a refused dispatch mints nothing + applies nothing"
    );
}

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
