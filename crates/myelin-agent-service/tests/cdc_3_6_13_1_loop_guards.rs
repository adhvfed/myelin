use myelin_agent_service::{AgentLoopGuards, GuardRefusal, GuardVerdict};
use myelin_content::InlineNode;
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn agent(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        },
        tenant(),
    )
}

fn human(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn delivered_dispatch(actor: Principal, correlation: &str, depth: u32) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("evt-{depth}")),
        type_: EventType("issues.comment.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(actor),
        subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
        aggregate: AggregateKey("agg-1".into()),
        causation_id: None,
        correlation_id: CorrelationId(correlation.into()),
        caused_by: None,
        depth,
        contains_personal_data: false,
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

#[test]
fn cdc_3_6_self_guard_reads_actor_principal_off_envelope() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    let ref_node =
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()));

    let own = delivered_dispatch(agent("agent-alice"), "corr", 0);
    let v = guards.admit_dispatch(&own.actor, &ref_node, &own.correlation_id.0, own.depth);
    assert_eq!(v, GuardVerdict::Drop(GuardRefusal::SelfTrigger));

    let human_ev = delivered_dispatch(human("user-bob"), "corr", 0);
    let v = guards.admit_dispatch(
        &human_ev.actor,
        &ref_node,
        &human_ev.correlation_id.0,
        human_ev.depth,
    );
    assert_eq!(
        v,
        GuardVerdict::Admit,
        "a human's dispatch is admitted (bounded by depth/tripwire)"
    );
}

#[test]
fn cdc_3_6_depth_ceiling_reads_envelope_depth() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    let ceiling = guards.ceiling();
    let ref_node =
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()));
    let other = Actor(human("user-bob"));

    let deep = delivered_dispatch(human("user-bob"), "corr", ceiling);
    let v = guards.admit_dispatch(&deep.actor, &ref_node, &deep.correlation_id.0, deep.depth);
    assert_eq!(
        v,
        GuardVerdict::Drop(GuardRefusal::DepthCeiling),
        "the depth ceiling reads the envelope's depth (a child past {ceiling} is dropped)"
    );

    let ok = delivered_dispatch(human("user-bob"), "corr", ceiling - 1);
    let v = guards.admit_dispatch(&other, &ref_node, &ok.correlation_id.0, ok.depth);
    assert_eq!(
        v,
        GuardVerdict::Admit,
        "a child exactly AT the ceiling is admitted"
    );
}

#[test]
fn cdc_13_1_reference_gate_keys_on_inline_node_taxonomy() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    let other = Actor(human("user-bob"));

    let ref_node =
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/knowledge/page/7".into()));
    assert_eq!(
        guards.admit_dispatch(&other, &ref_node, "corr", 0),
        GuardVerdict::Admit,
        "a structured artifact_ref node (13.1) re-triggers",
    );

    let mention = InlineNode::Mention(agent("agent-alice"));
    assert_eq!(
        guards.admit_dispatch(&other, &mention, "corr", 0),
        GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        "a mention (13.1) is explicit-dispatch, not a loop re-trigger",
    );

    let embed = InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/9".into()));
    assert_eq!(
        guards.admit_dispatch(&other, &embed, "corr", 0),
        GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
        "an embed (13.1) is display, not a re-trigger",
    );
}

#[test]
fn cdc_13_1_raw_text_is_not_an_inline_node_never_re_triggers() {
    let guards = AgentLoopGuards::new(PrincipalId("agent-alice".into()));
    for raw in [
        "@agent-alice loop",
        "myelin://acme/issues/issue/PROJ-1",
        "plain text",
        "",
    ] {
        assert_eq!(
            guards.reference_gate().admit_raw_text(raw),
            GuardVerdict::Drop(GuardRefusal::RawTextNotAReference),
            "raw text {raw:?} produces no 13.1 node → never re-triggers",
        );
    }
}
