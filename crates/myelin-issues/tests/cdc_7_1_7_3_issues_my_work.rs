use std::collections::BTreeSet;

use myelin_identity::{
    Consistency, ConsistencyMode, Principal, PrincipalId, PrincipalKind, Zookie,
};
use myelin_issues::{
    list_my_work, list_my_work_default, my_work_filter, register_issue_humanise_templates,
    wire_issues_my_work, TPL_APPROVAL_REQUESTED, TPL_SLA_AT_RISK, TPL_UNBLOCKED,
};
use myelin_notif::{
    active_inbox, humanise, list_inbox, mark, snooze, AllowAllAuthorize, Channel, Class,
    InboxFilter, InboxPage, InboxProjection, NotifRuleRegistry, Page, ReadState, Reason,
    RefProjection, RefResolution, RefResolvePort, RoutedInboxItem, TemplateStore, Tombstone,
    TombstoneReason, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn me() -> Principal {
    Principal::stub(PrincipalId("psn:me".into()), PrincipalKind::Human, tenant())
}
fn at() -> Consistency {
    Consistency {
        at_least: Zookie("zk-0".into()),
        mode: ConsistencyMode::Strong,
    }
}
fn item(item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: item_id.into(),
        recipient: "psn:me".into(),
        subject: ArtifactRef(subject.into()),
        reason,
        class: Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}
fn ids(page: &InboxPage) -> BTreeSet<String> {
    page.items.iter().map(|i| i.item_id.clone()).collect()
}

struct Resolver {
    allowed: Vec<(String, String)>,
}
impl RefResolvePort for Resolver {
    fn resolve_display(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        _at: &Consistency,
    ) -> RefResolution {
        if self
            .allowed
            .iter()
            .any(|(v, r)| v == &viewer.principal_id.0 && r == &ref_.0)
        {
            RefResolution::Projection(RefProjection {
                ref_: ref_.clone(),
                title: "ENG-1421: payments timeout".into(),
                icon: "issue".into(),
            })
        } else {
            RefResolution::Tombstone(Tombstone {
                root: ref_.clone(),
                reason: TombstoneReason::Denied,
            })
        }
    }
}

fn seeded() -> InboxProjection {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(
        "iss-assigned",
        "myelin://acme/issue/issue/E-1",
        Reason::Assigned,
    ));
    inbox.upsert_for_test(item(
        "iss-fyi",
        "myelin://acme/issue/issue/E-2",
        Reason::Fyi,
    ));
    inbox.upsert_for_test(item(
        "git-review",
        "myelin://acme/git/pr/9",
        Reason::ReviewRequested,
    ));
    inbox
}

#[test]
fn provider_my_work_is_the_one_notif_filter() {
    assert_eq!(my_work_filter(), InboxFilter::issues_my_work());
}

#[test]
fn consumer_my_work_is_a_strict_subset_of_the_one_inbox() {
    let inbox = seeded();
    let big = Page {
        after: None,
        limit: 1000,
    };
    let full = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::all(),
        &big,
        &AllowAllAuthorize,
        &at(),
    );
    let mine = list_my_work(&inbox, &me(), &big, &AllowAllAuthorize, &at());
    let full_ids = ids(&full);
    let my_ids = ids(&mine);
    assert!(my_ids.is_subset(&full_ids), "My Work ⊆ the ONE inbox");
    assert!(
        my_ids.len() < full_ids.len(),
        "STRICT subset (a view, not a copy)"
    );
    assert!(my_ids.contains("iss-assigned"));
    assert!(!my_ids.contains("iss-fyi"), "non-My-Work reason excluded");
    assert!(!my_ids.contains("git-review"), "other subsystem excluded");
}

#[test]
fn one_read_state_truth_across_my_work_and_the_inbox() {
    let inbox = seeded();
    mark(&inbox, &me(), "iss-assigned", ReadState::Read).expect("mark my own item");
    let full = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::all(),
        &Page::default(),
        &AllowAllAuthorize,
        &at(),
    );
    let in_full = full
        .items
        .iter()
        .find(|i| i.item_id == "iss-assigned")
        .unwrap();
    assert_eq!(
        in_full.state, "read",
        "the mark reflects in the unified inbox"
    );

    snooze(&inbox, &me(), "iss-assigned", "2026-07-01T09:00:00Z").expect("snooze my own item");
    let full = list_inbox(
        &inbox,
        &me(),
        &InboxFilter::all(),
        &Page::default(),
        &AllowAllAuthorize,
        &at(),
    );
    let row = full
        .items
        .iter()
        .find(|i| i.item_id == "iss-assigned")
        .unwrap();
    assert_eq!(row.state, "snoozed", "the snooze reflected on the ONE row");
    assert!(
        !active_inbox(list_my_work_default(&inbox, &me(), &at()).items)
            .iter()
            .any(|i| i.item_id == "iss-assigned"),
        "the snoozed item is suppressed from the active My Work view (one store)"
    );
}

#[test]
fn issue_templates_render_through_the_one_humanise_surface() {
    let mut store = TemplateStore::with_platform_defaults();
    register_issue_humanise_templates(&mut store);
    let subject = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    let resolver = Resolver {
        allowed: vec![("psn:me".into(), subject.0.clone())],
    };
    let region = Region("fr-par".into());

    for (key, expected) in [
        (
            TPL_SLA_AT_RISK,
            "SLA at risk on ENG-1421: payments timeout - respond before the deadline",
        ),
        (TPL_UNBLOCKED, "ENG-1421: payments timeout is now unblocked"),
        (
            TPL_APPROVAL_REQUESTED,
            "Approval requested on ENG-1421: payments timeout",
        ),
    ] {
        let h = humanise(
            &resolver,
            &tenant(),
            &region,
            &store,
            key,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(
            h.text, expected,
            "the Issues `{key}` string renders on the ONE surface"
        );
    }
}

#[test]
fn issue_template_is_leak_safe_for_a_denied_viewer() {
    let mut store = TemplateStore::with_platform_defaults();
    register_issue_humanise_templates(&mut store);
    let subject = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
    let resolver = Resolver { allowed: vec![] };
    let h = humanise(
        &resolver,
        &tenant(),
        &Region("fr-par".into()),
        &store,
        TPL_SLA_AT_RISK,
        &[subject],
        &me(),
        DEFAULT_LOCALE,
        &at(),
        Channel::Cli,
    );
    assert!(
        !h.text.contains("payments timeout"),
        "the title NEVER leaks (NOTIF-D4)"
    );
    assert!(
        h.text.contains("a restricted issue"),
        "the PII-free tombstone display"
    );
}

#[test]
fn wire_issues_my_work_registers_both_surfaces() {
    let mut reg = NotifRuleRegistry::platform_default();
    let mut store = TemplateStore::with_platform_defaults();
    let before = reg.len();
    wire_issues_my_work(&mut reg, &mut store);
    assert_eq!(reg.len(), before + 5, "the Issues reason set wired (7.6)");
    assert!(
        store
            .lookup(PLATFORM_DEFAULT_TENANT, TPL_SLA_AT_RISK, DEFAULT_LOCALE)
            .is_some(),
        "the SLA at-risk string is on the ONE templating surface (7.3)"
    );
}
