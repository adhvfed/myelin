//! Unit + redaction tests for the AG-P11 card-text path (C9/OQ-L): the HITL `risk_summary` + an
//! agent message resolve through Notif `humanise` to a per-viewer `HumanisedString`; a raw agent
//! string is REJECTED (0 raw-string surfaces); an unauthorised viewer sees the redacted card
//! (the SAME card renders differently for two viewers with different permissions). The consumer
//! CDC for 7.3 lives in `tests/cdc_7_3_humanise_card.rs`.

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

// ── Fixtures ──────────────────────────────────────────────────────────────────────────────

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
    Consistency { at_least: Zookie("z-1".into()), mode: ConsistencyMode::Strong }
}
/// The confidential PR the card is about — its TITLE must never leak to a denied viewer.
fn confidential_pr() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/pr/42".into())
}
/// The secret title an UNauthorised viewer must NEVER see in the card text.
const SECRET_TITLE: &str = "PR #42: rotate the production signing key";

/// A programmable synthetic Refs resolve chokepoint (the SAME `Projection | Tombstone` shape the
/// real chokepoint, REF-P10, returns). Per (viewer, ref) it allows or denies; a denied ref becomes a
/// TOMBSTONE carrying no title (the leak-free chokepoint humanise binds the slot to).
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
        self.allowed.lock().unwrap().push((viewer_id.into(), ref_.0.clone()));
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

/// A template store with BOTH the Notif platform defaults and the agent-fabric templates registered
/// into it (the ONE store — not a second engine).
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
        cost: EffectCost { unit: "git.merge", wholesale: 30, markup: 20 },
    };
    let gate = HitlGate::open(
        GateId("gate:git.merge:pr42".into()),
        "R1",
        &plan,
        risk(),
        vec![PrincipalId("psn:lead".into()), PrincipalId("psn:maintainer".into())],
        "card:R1:0",
    );
    crate::hitl::surface_card(&gate)
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  C9 — the risk_summary + an agent message resolve through humanise to a per-viewer HumanisedString
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The HITL `risk_summary` resolves through Notif `humanise` to a per-viewer `HumanisedString`
/// (C9).** An AUTHORISED viewer sees the title bound into the card body — through the ONE templating
/// surface, never a raw string.
#[test]
fn risk_summary_resolves_through_humanise_for_an_authorised_viewer() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let out = humanise_risk_summary(
        &r, &tenant(), &region(), &store(), &risk(), &viewer("lead"), "en", &at(), Channel::Cli,
    )
    .expect("a stable template key humanises");
    // the authorised viewer sees the title bound into the agent.hitl.merge_pr body.
    assert!(out.text.contains(SECRET_TITLE), "authorised viewer sees the title: {:?}", out.text);
    assert!(out.text.starts_with("Merge "), "the card text came from the agent template body");
    // a click-route to the resolved ref rode through (the allowed branch yields a link).
    assert_eq!(out.links, vec![confidential_pr().0]);
}

/// **An agent-authored message resolves through `humanise` per-viewer (C9).** The SAME render path
/// as the risk summary — one templating surface, never a second engine.
#[test]
fn agent_message_resolves_through_humanise() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let msg = AgentMessage::about("agent.msg.proposed_effect", &confidential_pr());
    let out = humanise_agent_message(
        &r, &tenant(), &region(), &store(), &msg, &viewer("lead"), "en", &at(), Channel::Cli,
    )
    .expect("a stable template key humanises");
    assert!(out.text.contains(SECRET_TITLE), "the agent message bound the title per-viewer");
    assert!(out.text.starts_with("The agent proposed an effect on"));
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  PER-VIEWER — the SAME card renders DIFFERENTLY for two viewers with different permissions
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The SAME card renders differently for two viewers with different permissions (per-viewer
/// proven; the redaction test).** An authorised viewer sees the title; an UNauthorised viewer sees a
/// TOMBSTONE (`a restricted pr`) — the title NEVER leaks (NOTIF-D4, inherited for free). This is the
/// AG-P11 GATE's per-viewer leg.
#[test]
fn same_card_renders_differently_for_two_viewers() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr()); // lead may view; bystander may NOT.
    let c = card();

    let lead_card = humanise_card(
        &r, &tenant(), &region(), &store(), &c, &viewer("lead"), "en", &at(), Channel::Cli,
    )
    .unwrap();
    let bystander_card = humanise_card(
        &r, &tenant(), &region(), &store(), &c, &viewer("bystander"), "en", &at(), Channel::Cli,
    )
    .unwrap();

    // the SAME card (same gate, same risk slot) renders DIFFERENT text per viewer.
    assert_ne!(
        lead_card.risk_text.text, bystander_card.risk_text.text,
        "the per-viewer gate makes the two renders differ"
    );
    // the authorised viewer sees the title.
    assert!(lead_card.risk_text.text.contains(SECRET_TITLE), "lead sees the title");
    // the UNauthorised viewer sees a tombstone, NEVER the title (0 leak).
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
    // the bystander's render carries NO click-route to the denied ref (no route leaks either).
    assert!(bystander_card.risk_text.links.is_empty(), "no link to a denied ref");
    // the structured (non-text) fields ride through unchanged for both viewers.
    assert_eq!(lead_card.cost_estimate, 50);
    assert_eq!(lead_card.action_tool, "git.merge");
    assert_eq!(bystander_card.cost_estimate, 50);
    assert_eq!(lead_card.approver_count, 2);
}

/// **An ERASED subject humanises to `[erased user]` for everyone (erasure-safe for free).** Even an
/// otherwise-authorised viewer sees the erased display, never PII, never a 500.
#[test]
fn erased_subject_humanises_to_erased_user() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    r.mark_erased(&confidential_pr()); // erased takes precedence over the allow.
    let out = humanise_risk_summary(
        &r, &tenant(), &region(), &store(), &risk(), &viewer("lead"), "en", &at(), Channel::Cli,
    )
    .unwrap();
    assert!(out.text.contains("[erased user]"), "erased subject → [erased user]: {:?}", out.text);
    assert!(!out.text.contains(SECRET_TITLE), "an erased subject leaks no title");
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  0-RAW-STRING-SURFACES — a raw agent string is REJECTED (the GATE)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **A raw agent string is REJECTED (0 raw-string surfaces, the GATE).** A free-text sentence, a key
/// with spaces, a Capitalised key, sentence punctuation, or an empty key is refused — the agent loop
/// has NO raw-string path to a card or a chat message.
#[test]
fn a_raw_agent_string_is_rejected() {
    // raw human sentences (the thing an LLM brain would try to emit) are all rejected.
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
    // the rejection carries the offending candidate (observable in the audit).
    assert_eq!(
        assert_no_raw_agent_surface("Please review this PR!"),
        Err(RawAgentString { offered: "Please review this PR!".into() })
    );
}

/// **A stable taxonomy `template_key` is ACCEPTED.** The legitimate agent-fabric keys pass the gate
/// (dot/underscore/hyphen-segmented, lowercase) — they are the ONLY shape that reaches `humanise`.
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
        assert!(assert_no_raw_agent_surface(key).is_ok(), "the stable key `{key}` is accepted");
    }
    // every agent-fabric platform-default key passes the gate (no raw-string surface ships).
    for (key, _, _) in AGENT_PLATFORM_DEFAULT_TEMPLATES {
        assert!(assert_no_raw_agent_surface(key).is_ok(), "platform-default key `{key}` is a token");
    }
}

/// **The render path itself REFUSES a raw key (no raw string smuggled in as a "key").** Even if a
/// caller hand-builds a `RiskSummary`/`AgentMessage` with a free-text key, the render rejects it —
/// the gate runs on EVERY humanise call, so 0 raw strings reach the templating surface.
#[test]
fn the_render_path_refuses_a_raw_key() {
    let r = SyntheticResolver::new();
    r.allow("lead", &confidential_pr());
    let raw_risk = RiskSummary {
        template_key: "Merge this PR now!".into(), // a raw string masquerading as a key.
        args: vec![("object".into(), confidential_pr())],
    };
    let res = humanise_risk_summary(
        &r, &tenant(), &region(), &store(), &raw_risk, &viewer("lead"), "en", &at(), Channel::Cli,
    );
    assert!(res.is_err(), "a raw-string key is refused at the render boundary");

    let raw_msg = AgentMessage {
        template_key: "The agent did a thing.".into(),
        args: vec![],
    };
    assert!(
        humanise_agent_message(
            &r, &tenant(), &region(), &store(), &raw_msg, &viewer("lead"), "en", &at(),
            Channel::Cli,
        )
        .is_err(),
        "a raw-string agent message is refused at the render boundary"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  CHANNEL PROJECTIONS — the leak invariant holds across every channel projection
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The leak invariant holds across every channel projection (CLI / Email / Markdown).** A denied
/// viewer sees the tombstone in EVERY channel — the per-viewer slot binding happened before the
/// channel lowering, so no channel re-introduces the title.
#[test]
fn the_tombstone_holds_across_every_channel() {
    let r = SyntheticResolver::new(); // bystander is denied on every channel.
    for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
        let out = humanise_risk_summary(
            &r, &tenant(), &region(), &store(), &risk(), &viewer("bystander"), "en", &at(), channel,
        )
        .unwrap();
        assert!(!out.text.contains(SECRET_TITLE), "{channel:?} leaks no title");
        assert!(out.text.contains("a restricted pr"), "{channel:?} shows the tombstone");
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
//  THE ONE STORE — the agent templates register into the SAME Notif TemplateStore (no second engine)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The agent-fabric templates register into the SAME Notif `TemplateStore` (the ONE surface).**
/// After `register_agent_templates`, the agent keys resolve in the store the Notif defaults live in
/// — there is no second template engine and no frontend string map.
#[test]
fn agent_templates_register_into_the_one_store() {
    let s = store();
    // an agent key and a Notif platform-default key both resolve in the SAME store.
    assert!(
        s.lookup(myelin_notif::PLATFORM_DEFAULT_TENANT, "agent.hitl.merge_pr", "en").is_some(),
        "the agent template is in the ONE store"
    );
    assert!(
        s.lookup(myelin_notif::PLATFORM_DEFAULT_TENANT, "approval_requested", "en").is_some(),
        "the Notif default is in the SAME store"
    );
}
