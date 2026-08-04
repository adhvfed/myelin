use std::sync::Mutex;

use myelin_chat::glue::{chat_hitl_card_facets, chat_humanise_templates, TPL_CHAT_CARD};
use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, TemplateStore, Tombstone,
    TombstoneReason, DEFAULT_LOCALE,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn strong(zk: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zk.into()),
        mode: ConsistencyMode::Strong,
    }
}

fn confidential_channel() -> ArtifactRef {
    ArtifactRef("myelin://acme/chat/channel/board-secret".into())
}
const SECRET_TITLE: &str = "#board-leadership-comp";

#[derive(Default)]
struct SyntheticResolver {
    allowed: Mutex<Vec<(String, String)>>,
}
impl SyntheticResolver {
    fn allow(&self, viewer_id: &str, ref_: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((viewer_id.into(), ref_.0.clone()));
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
                icon: "channel".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

fn store_with_chat_keys() -> TemplateStore {
    let mut store = TemplateStore::with_platform_defaults();
    for row in chat_humanise_templates() {
        store.put(row);
    }
    store
}

#[test]
fn chat_d5_confidential_unfurl_tombstones_zero_title_leak() {
    let resolver = SyntheticResolver::default();
    let subject = confidential_channel();
    let store = store_with_chat_keys();

    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &store,
        TPL_CHAT_CARD,
        std::slice::from_ref(&subject),
        &viewer("intruder"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );

    assert!(
        !h.text.contains(SECRET_TITLE)
            && !h.text.contains("leadership")
            && !h.text.contains("comp"),
        "CHAT-D5: the title must NEVER appear for a denied viewer, got text=`{}`",
        h.text
    );
    assert!(
        h.text.contains("a restricted channel"),
        "the denied subject slot renders the tombstone display, got `{}`",
        h.text
    );
    assert!(
        h.links.is_empty(),
        "a denied ref yields no link, got {:?}",
        h.links
    );
}

#[test]
fn chat_d5_allowed_approver_sees_the_title() {
    let resolver = SyntheticResolver::default();
    let subject = confidential_channel();
    resolver.allow("approver", &subject);
    let store = store_with_chat_keys();

    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &store,
        TPL_CHAT_CARD,
        std::slice::from_ref(&subject),
        &viewer("approver"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_TITLE),
        "the permitted approver DOES see the channel title, got `{}`",
        h.text
    );
    assert_eq!(
        h.links,
        vec![subject.0.clone()],
        "allowed ref yields its link"
    );
}

#[test]
fn hitl_card_surfaces_action_risk_cost_and_is_leak_safe() {
    let resolver = SyntheticResolver::default();
    let store = store_with_chat_keys();

    let subject_line = humanise(
        &resolver,
        &tenant(),
        &region(),
        &store,
        TPL_CHAT_CARD,
        &[confidential_channel()],
        &viewer("intruder"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    let facets = chat_hitl_card_facets(&store, "archive-channel", "irreversible", "0.10 USD");
    let card = format!("{} - {}", subject_line.text, facets);

    assert!(card.contains("archive-channel"), "action renders: `{card}`");
    assert!(card.contains("irreversible"), "risk renders: `{card}`");
    assert!(card.contains("0.10 USD"), "cost renders: `{card}`");
    assert!(
        card.contains("a restricted channel") && !card.contains(SECRET_TITLE),
        "the HITL card is leak-safe in the subject slot: `{card}`"
    );
}
