use myelin_agent::{GateId, ToolName};
use myelin_agent_service::{assert_no_raw_agent_surface, humanise_agent_message};
use myelin_agent_service::{
    humanise_card, humanise_risk_summary, register_agent_templates, surface_card, AgentMessage,
    EffectCost, HitlGate, PlannedEffect, RenderCtx, RiskSummary,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    Channel, RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone,
    TombstoneReason,
};
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use std::sync::Mutex;

const SECRET_TITLE: &str = "PR #99: ship the GDPR erasure fan-out";

#[derive(Default)]
struct ProviderResolve {
    allowed: Mutex<Vec<(String, String)>>,
}
impl ProviderResolve {
    fn allow(&self, viewer_id: &str, ref_: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((viewer_id.into(), ref_.0.clone()));
    }
}
impl RefResolvePort for ProviderResolve {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        let allowed = self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|(v, r)| v == &viewer.principal_id.0 && r == &ref_.0);
        if allowed {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: SECRET_TITLE.into(),
                icon: "lock".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn at() -> Consistency {
    Consistency {
        at_least: Zookie("z-9".into()),
        mode: ConsistencyMode::Strong,
    }
}
fn pr() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/99".into())
}
fn store() -> TemplateStore {
    let mut s = TemplateStore::with_platform_defaults();
    register_agent_templates(&mut s);
    s
}
fn a_card() -> myelin_agent_service::HitlCard {
    let plan = PlannedEffect {
        tool: ToolName("git.merge".into()),
        object: pr(),
        input_json: r#"{"pr":99}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "git.merge",
            wholesale: 40,
            markup: 10,
        },
    };
    let gate = HitlGate::open(
        GateId("gate:git.merge:pr99".into()),
        "R9",
        &plan,
        RiskSummary::for_action("agent.hitl.merge_pr", &pr()),
        vec![PrincipalId("psn:lead".into())],
        "card:R9:0",
    );
    surface_card(&gate)
}

#[test]
fn cdc_card_renders_per_viewer_through_humanise() {
    let prov = ProviderResolve::default();
    prov.allow("lead", &pr());
    let card = a_card();

    let lead = humanise_card(
        &RenderCtx {
            resolver: &prov,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("lead"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &card,
    )
    .expect("a stable key renders");
    let other = humanise_card(
        &RenderCtx {
            resolver: &prov,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("bystander"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &card,
    )
    .expect("a stable key renders");

    assert!(
        lead.risk_text.text.contains(SECRET_TITLE),
        "authorised viewer sees the title"
    );
    assert!(
        !other.risk_text.text.contains(SECRET_TITLE),
        "denied viewer: 0 title leak"
    );
    assert!(
        other.risk_text.text.contains("a restricted pr"),
        "denied viewer sees a tombstone"
    );
    assert_ne!(
        lead.risk_text.text, other.risk_text.text,
        "per-viewer renders differ"
    );
    assert_eq!(lead.cost_estimate, 50);
    assert_eq!(lead.action_tool, "git.merge");
}

#[test]
fn cdc_agent_message_renders_through_humanise() {
    let prov = ProviderResolve::default();
    prov.allow("lead", &pr());
    let msg = AgentMessage::about("agent.msg.completed", &pr());
    let out = humanise_agent_message(
        &RenderCtx {
            resolver: &prov,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("lead"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &msg,
    )
    .unwrap();
    assert!(
        out.text.contains(SECRET_TITLE),
        "the agent message bound the title per-viewer"
    );
}

#[test]
fn cdc_no_raw_agent_string_reaches_humanise() {
    assert!(
        assert_no_raw_agent_surface("agent.hitl.merge_pr").is_ok(),
        "a token key crosses"
    );
    assert!(
        assert_no_raw_agent_surface("Merge this PR now!").is_err(),
        "a raw string never crosses"
    );

    let prov = ProviderResolve::default();
    prov.allow("lead", &pr());
    let raw = RiskSummary {
        template_key: "Please approve!".into(),
        args: vec![("o".into(), pr())],
    };
    assert!(
        humanise_risk_summary(
            &RenderCtx {
                resolver: &prov,
                tenant: &tenant(),
                region: &region(),
                templates: &store(),
                viewer: &viewer("lead"),
                locale: "en",
                at: &at(),
                channel: Channel::Cli,
            },
            &raw,
        )
        .is_err(),
        "the render boundary refuses a raw-string key (0 raw-string surfaces)"
    );
}
