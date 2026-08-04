use myelin_identity::{
    Consistency, ConsistencyMode, Decision, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_notif::humanise::{
    humanise, Channel, RefProjection, RefResolution, RefResolvePort, Tombstone, TombstoneReason,
    DEFAULT_LOCALE,
};
use myelin_notif::list_inbox::{list_inbox_ranked, InboxFilter, Page, ReadAuthorizePort};
use myelin_notif::ranking::DeterministicV1;
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::{Class, Reason, TemplateStore};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::sync::Mutex;

const SECRET_TITLE: &str = "PROJECT NIGHTFALL - confidential acquisition terms";

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
fn confidential_issue() -> ArtifactRef {
    ArtifactRef("myelin://acme/issue/issue/ENG-secret".into())
}

#[derive(Default)]
struct DrillResolver {
    allowed: Mutex<Vec<(String, String)>>,
    erased: Mutex<Vec<String>>,
}
impl DrillResolver {
    fn allow(&self, viewer_id: &str, r: &ArtifactRef) {
        self.allowed
            .lock()
            .unwrap()
            .push((viewer_id.into(), r.0.clone()));
    }
    fn erase(&self, r: &ArtifactRef) {
        self.erased.lock().unwrap().push(r.0.clone());
    }
}
impl RefResolvePort for DrillResolver {
    fn resolve_display(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self.erased.lock().unwrap().iter().any(|x| x == &ref_.0) {
            return RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Erased,
            });
        }
        if self
            .allowed
            .lock()
            .unwrap()
            .iter()
            .any(|(v, x)| v == &viewer.principal_id.0 && x == &ref_.0)
        {
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

const ALL_REASONS: &[Reason] = &[
    Reason::ApprovalRequested,
    Reason::Escalated,
    Reason::Sla,
    Reason::ReviewRequested,
    Reason::Assigned,
    Reason::Mentioned,
    Reason::Replied,
    Reason::AgentProposal,
    Reason::Watched,
    Reason::StateChanged,
    Reason::Fyi,
    Reason::Blocked,
    Reason::Unblocked,
    Reason::ThreadWatched,
    Reason::Shared,
    Reason::Comments,
];

fn contains_leak(text: &str) -> bool {
    let lc = text.to_lowercase();
    text.contains(SECRET_TITLE)
        || lc.contains("nightfall")
        || lc.contains("acquisition")
        || lc.contains("confidential")
}

#[test]
fn notif_d4_zero_title_leak_across_viewers_channels_reasons() {
    let resolver = DrillResolver::default();
    let templates = TemplateStore::with_platform_defaults();
    let subject = confidential_issue();
    let denied_viewers = ["intruder-a", "intruder-b", "ex-employee"];

    let mut renders = 0u64;
    let mut leak_count = 0u64;
    let mut tombstone_present = 0u64;

    for v in denied_viewers {
        for &reason in ALL_REASONS {
            let key = myelin_notif::reason_template_key(reason);
            for channel in [Channel::Cli, Channel::Email, Channel::Markdown] {
                let h = humanise(
                    &resolver,
                    &tenant(),
                    &region(),
                    &templates,
                    key,
                    std::slice::from_ref(&subject),
                    &viewer(v),
                    DEFAULT_LOCALE,
                    &strong("z1"),
                    channel,
                );
                renders += 1;
                if contains_leak(&h.text) {
                    leak_count += 1;
                }
                if h.text.contains("a restricted issue") {
                    tombstone_present += 1;
                }
                assert!(
                    h.links.is_empty(),
                    "a denied subject yields no link (reason={key}, channel={channel:?})"
                );
            }
        }
    }

    assert_eq!(
        leak_count, 0,
        "NOTIF-D4: title-leak-count MUST be 0 over {renders} renders (the F1 floor); never weakened"
    );
    assert_eq!(
        tombstone_present, renders,
        "every denied render shows the PII-free tombstone display (the embed degrades, never vanishes)"
    );
    eprintln!(
        "NOTIF-D4 GREEN (2026-06-20): {renders} denied renders, title-leak-count = {leak_count} (threshold 0), \
         tombstone-present = {tombstone_present}/{renders}"
    );
}

#[test]
fn notif_d4_permitted_viewer_sees_the_title() {
    let resolver = DrillResolver::default();
    let subject = confidential_issue();
    resolver.allow("insider", &subject);
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "review_requested",
        std::slice::from_ref(&subject),
        &viewer("insider"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Cli,
    );
    assert!(
        h.text.contains(SECRET_TITLE),
        "the permitted viewer sees the title (the gate is real)"
    );
    assert_eq!(
        h.links,
        vec![subject.0],
        "the allowed branch yields the click-route link"
    );
}

#[test]
fn notif_d4_erased_actor_is_erased_user_zero_pii() {
    let resolver = DrillResolver::default();
    let actor = ArtifactRef("myelin://acme/identity/user/u-77".into());
    resolver.allow("colleague", &actor);
    resolver.erase(&actor);
    let h = humanise(
        &resolver,
        &tenant(),
        &region(),
        &TemplateStore::with_platform_defaults(),
        "mentioned",
        &[actor],
        &viewer("colleague"),
        DEFAULT_LOCALE,
        &strong("z1"),
        Channel::Email,
    );
    assert!(
        h.text.contains("[erased user]"),
        "an erased actor renders [erased user], got `{}`",
        h.text
    );
    assert!(h.links.is_empty(), "an erased ref is not routable");
}

#[test]
fn notif_d4_inbox_read_suppresses_unseeable_item() {
    struct DenyConfidential;
    impl ReadAuthorizePort for DenyConfidential {
        fn can_read(&self, _v: &Principal, subject: &ArtifactRef, _at: &Consistency) -> Decision {
            if subject == &confidential_issue() {
                Decision::Deny
            } else {
                Decision::Allow
            }
        }
    }

    let inbox = InboxProjection::new();
    let visible = ArtifactRef("myelin://acme/issue/issue/ENG-public".into());
    inbox.upsert_for_test(row("c", confidential_issue(), Reason::ReviewRequested));
    inbox.upsert_for_test(row("p", visible.clone(), Reason::Assigned));

    let page = list_inbox_ranked(
        &inbox,
        &viewer("recipient"),
        &InboxFilter::all(),
        &Page::default(),
        &DenyConfidential,
        &strong("z1"),
        &DeterministicV1::default(),
    );

    let subjects: Vec<&ArtifactRef> = page.items.iter().map(|r| &r.item.subject).collect();
    assert!(
        !subjects.contains(&&confidential_issue()),
        "the unseeable confidential item is SUPPRESSED from the read (held, not leaked)"
    );
    assert!(subjects.contains(&&visible), "the visible item remains");
}

fn row(id: &str, subject: ArtifactRef, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: region(),
        item_id: id.into(),
        recipient: "recipient".into(),
        subject,
        reason,
        class: Class::Direct,
        origin_event: ArtifactRef("myelin://acme/issue/event/e".into()),
        dedup_key: id.into(),
        coalesce_count: 0,
        state: "unread".into(),
        snooze_until: None,
    }
}
