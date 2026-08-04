use super::*;
use crate::effect_api::EffectCost;
use crate::hitl::{HitlGate, RiskSummary};
use myelin_agent::GateId;
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    Channel, RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone,
    TombstoneReason,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Mutex;

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
        at_least: Zookie("z-1".into()),
        mode: ConsistencyMode::Strong,
    }
}
fn confidential_pr() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/42".into())
}
const SECRET_TITLE: &str = "PR #42: rotate the production signing key";

#[derive(Default)]
struct SyntheticResolver {
    allowed: Mutex<Vec<(String, String)>>,
    erased: Mutex<Vec<String>>,
}
impl SyntheticResolver {
    fn new() -> SyntheticResolver {
        SyntheticResolver::default()
    }
    fn allow(&self, viewer_id: &str, ref_: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((viewer_id.into(), ref_.0.clone()));
    }
    fn mark_erased(&self, ref_: &ArtifactRef) {
        self.erased.lock().unwrap().push(ref_.0.clone());
    }
}
impl RefResolvePort for SyntheticResolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self.erased.lock().unwrap().iter().any(|r| r == &ref_.0) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
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

fn store() -> TemplateStore {
    let mut s = TemplateStore::with_platform_defaults();
    register_agent_templates(&mut s);
    s
}

fn risk() -> RiskSummary {
    RiskSummary::for_action("agent.hitl.merge_pr", &confidential_pr())
}

fn card() -> HitlCard {
    let plan = crate::effect_api::PlannedEffect {
        tool: myelin_agent::ToolName("git.merge".into()),
        object: confidential_pr(),
        input_json: r#"{"pr":42}"#.into(),
        field: None,
        transition: None,
        cost: EffectCost {
            unit: "git.merge",
            wholesale: 30,
            markup: 20,
        },
    };
    let gate = HitlGate::open(
        GateId("gate:git.merge:pr42".into()),
        "R1",
        &plan,
        risk(),
        vec![
            PrincipalId("psn:lead".into()),
            PrincipalId("psn:maintainer".into()),
        ],
        "card:R1:0",
    );
    crate::hitl::surface_card(&gate)
}

#[test]
fn risk_summary_resolves_through_humanise_for_an_authorised_viewer() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let out = humanise_risk_summary(
        &RenderCtx {
            resolver: &r,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("lead"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &risk(),
    )
    .expect("a stable template key humanises");
    assert!(
        out.text.contains(SECRET_TITLE),
        "authorised viewer sees the title: {:?}",
        out.text
    );
    assert!(
        out.text.starts_with("Merge "),
        "the card text came from the agent template body"
    );
    assert_eq!(out.links, vec![confidential_pr().0]);
}

#[test]
fn agent_message_resolves_through_humanise() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let msg = AgentMessage::about("agent.msg.proposed_effect", &confidential_pr());
    let out = humanise_agent_message(
        &RenderCtx {
            resolver: &r,
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
    .expect("a stable template key humanises");
    assert!(
        out.text.contains(SECRET_TITLE),
        "the agent message bound the title per-viewer"
    );
    assert!(out.text.starts_with("The agent proposed an effect on"));
}

#[test]
fn same_card_renders_differently_for_two_viewers() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let c = card();

    let lead_card = humanise_card(
        &RenderCtx {
            resolver: &r,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("lead"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &c,
    )
    .unwrap();
    let bystander_card = humanise_card(
        &RenderCtx {
            resolver: &r,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("bystander"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &c,
    )
    .unwrap();

    assert_ne!(
        lead_card.risk_text.text, bystander_card.risk_text.text,
        "the per-viewer gate makes the two renders differ"
    );
    assert!(
        lead_card.risk_text.text.contains(SECRET_TITLE),
        "lead sees the title"
    );
    assert!(
        !bystander_card.risk_text.text.contains(SECRET_TITLE),
        "bystander must NOT see the title (0 leak): {:?}",
        bystander_card.risk_text.text
    );
    assert!(
        bystander_card.risk_text.text.contains("a restricted pr"),
        "bystander sees the kind-shaped tombstone: {:?}",
        bystander_card.risk_text.text
    );
    assert!(
        bystander_card.risk_text.links.is_empty(),
        "no link to a denied ref"
    );
    assert_eq!(lead_card.cost_estimate, 50);
    assert_eq!(lead_card.action_tool, "git.merge");
    assert_eq!(bystander_card.cost_estimate, 50);
    assert_eq!(lead_card.approver_count, 2);
}

#[test]
fn erased_subject_humanises_to_erased_user() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    r.mark_erased(&confidential_pr());
    let out = humanise_risk_summary(
        &RenderCtx {
            resolver: &r,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("lead"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &risk(),
    )
    .unwrap();
    assert!(
        out.text.contains("[erased user]"),
        "erased subject → [erased user]: {:?}",
        out.text
    );
    assert!(
        !out.text.contains(SECRET_TITLE),
        "an erased subject leaks no title"
    );
}

#[test]
fn a_raw_agent_string_is_rejected() {
    for raw in [
        "Please review this PR!",
        "Merge the pull request now.",
        "Approve, then deploy",
        "Hello world",
        "Capitalised",
        "",
        "trailing.",
        "has space",
    ] {
        assert!(
            assert_no_raw_agent_surface(raw).is_err(),
            "a raw agent string `{raw}` must be rejected (0 raw-string surfaces)"
        );
    }
    assert_eq!(
        assert_no_raw_agent_surface("Please review this PR!"),
        Err(RawAgentString {
            offered: "Please review this PR!".into()
        })
    );
}

#[test]
fn a_stable_template_key_is_accepted() {
    for key in [
        "agent.hitl.merge_pr",
        "agent.msg.proposed_effect",
        "agent.hitl.deploy",
        "agent.hitl.generic",
        "git.pr.merged",
        "approval_requested",
        "agent-fabric.note",
    ] {
        assert!(
            assert_no_raw_agent_surface(key).is_ok(),
            "the stable key `{key}` is accepted"
        );
    }
    for (key, _, _) in AGENT_PLATFORM_DEFAULT_TEMPLATES {
        assert!(
            assert_no_raw_agent_surface(key).is_ok(),
            "platform-default key `{key}` is a token"
        );
    }
}

#[test]
fn the_render_path_refuses_a_raw_key() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let raw_risk = RiskSummary {
        template_key: "Merge this PR now!".into(),
        args: vec![("object".into(), confidential_pr())],
    };
    let res = humanise_risk_summary(
        &RenderCtx {
            resolver: &r,
            tenant: &tenant(),
            region: &region(),
            templates: &store(),
            viewer: &viewer("lead"),
            locale: "en",
            at: &at(),
            channel: Channel::Cli,
        },
        &raw_risk,
    );
    assert!(
        res.is_err(),
        "a raw-string key is refused at the render boundary"
    );

    let raw_msg = AgentMessage {
        template_key: "The agent did a thing.".into(),
        args: vec![],
    };
    assert!(
        humanise_agent_message(
            &RenderCtx {
                resolver: &r,
                tenant: &tenant(),
                region: &region(),
                templates: &store(),
                viewer: &viewer("lead"),
                locale: "en",
                at: &at(),
                channel: Channel::Cli,
            },
            &raw_msg,
        )
        .is_err(),
        "a raw-string agent message is refused at the render boundary"
    );
}

#[test]
fn the_tombstone_holds_across_every_channel() {
    let r = SyntheticResolver::new();
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let out = humanise_risk_summary(
            &RenderCtx {
                resolver: &r,
                tenant: &tenant(),
                region: &region(),
                templates: &store(),
                viewer: &viewer("bystander"),
                locale: "en",
                at: &at(),
                channel,
            },
            &risk(),
        )
        .unwrap();
        assert!(
            !out.text.contains(SECRET_TITLE),
            "{channel:?} leaks no title"
        );
        assert!(
            out.text.contains("a restricted pr"),
            "{channel:?} shows the tombstone"
        );
    }
}

#[test]
fn agent_templates_register_into_the_one_store() {
    let s = store();
    assert!(
        s.lookup(
            myelin_notif::PLATFORM_DEFAULT_TENANT,
            "agent.hitl.merge_pr",
            "en"
        )
        .is_some(),
        "the agent template is in the ONE store"
    );
    assert!(
        s.lookup(
            myelin_notif::PLATFORM_DEFAULT_TENANT,
            "approval_requested",
            "en"
        )
        .is_some(),
        "the Notif default is in the SAME store"
    );
}
