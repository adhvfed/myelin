use myelin_identity::{Consistency, Principal};
use myelin_notif::{
    list_inbox, AllowAllAuthorize, HumaniseTemplate, InboxFilter, InboxPage, InboxProjection,
    NotifRuleRegistry, Page, ReadAuthorizePort, TemplateStore, DEFAULT_LOCALE,
    PLATFORM_DEFAULT_TENANT,
};

use crate::declares::register_issue_notif_rules;

pub fn my_work_filter() -> InboxFilter {
    InboxFilter::issues_my_work()
}

pub fn list_my_work(
    inbox: &InboxProjection,
    principal: &Principal,
    page: &Page,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    list_inbox(inbox, principal, &my_work_filter(), page, authorize, at)
}

pub fn list_my_work_default(
    inbox: &InboxProjection,
    principal: &Principal,
    at: &Consistency,
) -> InboxPage {
    list_my_work(inbox, principal, &Page::default(), &AllowAllAuthorize, at)
}

pub const TPL_SLA_AT_RISK: &str = "issue.sla.at_risk";
pub const TPL_UNBLOCKED: &str = "issue.unblocked";
pub const TPL_APPROVAL_REQUESTED: &str = "issue.approval_requested";

pub const ISSUE_HUMANISE_TEMPLATES: &[(&str, &str, &str)] = &[
    (
        TPL_SLA_AT_RISK,
        "SLA at risk on {0} - respond before the deadline",
        "sla",
    ),
    (TPL_UNBLOCKED, "{0} is now unblocked", "unblocked"),
    (
        TPL_APPROVAL_REQUESTED,
        "Approval requested on {0}",
        "approval",
    ),
];

pub fn issue_humanise_templates() -> Vec<HumaniseTemplate> {
    ISSUE_HUMANISE_TEMPLATES
        .iter()
        .map(|(key, body, icon)| HumaniseTemplate {
            tenant: PLATFORM_DEFAULT_TENANT.to_string(),
            template_key: (*key).to_string(),
            locale: DEFAULT_LOCALE.to_string(),
            body: (*body).to_string(),
            icon: (*icon).to_string(),
        })
        .collect()
}

pub fn register_issue_humanise_templates(store: &mut TemplateStore) -> &mut TemplateStore {
    for tpl in issue_humanise_templates() {
        store.put(tpl);
    }
    store
}

pub fn wire_issues_my_work(registry: &mut NotifRuleRegistry, templates: &mut TemplateStore) {
    register_issue_notif_rules(registry);
    register_issue_humanise_templates(templates);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use myelin_notif::{
        humanise, Channel, ReadState, Reason, RefProjection, RefResolution, RefResolvePort,
        RoutedInboxItem, Tombstone, TombstoneReason,
    };
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};

    use myelin_identity::{ConsistencyMode, PrincipalId, PrincipalKind, Zookie};

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
            class: myelin_notif::Class::Direct,
            origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
            dedup_key: item_id.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
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

    fn ids(page: &InboxPage) -> BTreeSet<String> {
        page.items.iter().map(|i| i.item_id.clone()).collect()
    }


    #[test]
    fn my_work_is_a_strict_subset_of_the_one_inbox() {
        let inbox = seeded();
        let full = list_inbox(
            &inbox,
            &me(),
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 1000,
            },
            &AllowAllAuthorize,
            &at(),
        );
        let mine = list_my_work(
            &inbox,
            &me(),
            &Page {
                after: None,
                limit: 1000,
            },
            &AllowAllAuthorize,
            &at(),
        );
        let full_ids = ids(&full);
        let my_ids = ids(&mine);
        assert!(my_ids.is_subset(&full_ids), "My Work ⊆ the ONE inbox");
        assert!(
            my_ids.len() < full_ids.len(),
            "My Work is a STRICT subset (a view, not a copy)"
        );
        assert!(
            my_ids.contains("iss-assigned"),
            "the assigned issue is in My Work"
        );
        assert!(
            !my_ids.contains("iss-fyi"),
            "a non-My-Work Issues reason is excluded"
        );
        assert!(
            !my_ids.contains("git-review"),
            "another subsystem's row is excluded"
        );
    }

    #[test]
    fn mark_on_my_work_reflects_in_the_one_inbox() {
        let inbox = seeded();
        let mine = list_my_work_default(&inbox, &me(), &at());
        let row_id = mine
            .items
            .iter()
            .find(|i| i.item_id == "iss-assigned")
            .expect("the assigned issue is in My Work")
            .item_id
            .clone();
        myelin_notif::mark(&inbox, &me(), &row_id, ReadState::Read).expect("mark my own item");

        let mine2 = list_my_work_default(&inbox, &me(), &at());
        let in_view = mine2.items.iter().find(|i| i.item_id == row_id).unwrap();
        assert_eq!(in_view.state, "read", "the My Work row reads `read`");

        let full = list_inbox(
            &inbox,
            &me(),
            &InboxFilter::all(),
            &Page::default(),
            &AllowAllAuthorize,
            &at(),
        );
        let in_full = full.items.iter().find(|i| i.item_id == row_id).unwrap();
        assert_eq!(
            in_full.state, "read",
            "the SAME row reads `read` in the unified inbox (one read-state truth)"
        );
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

    #[test]
    fn issue_templates_register_and_render_on_the_one_surface() {
        let mut store = TemplateStore::with_platform_defaults();
        let before = issue_humanise_templates().len();
        register_issue_humanise_templates(&mut store);

        let subject = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
        let resolver = Resolver {
            allowed: vec![("psn:me".into(), subject.0.clone())],
        };

        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_SLA_AT_RISK,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(
            h.text, "SLA at risk on ENG-1421: payments timeout - respond before the deadline",
            "the SLA at-risk string renders through the ONE humanise surface"
        );
        assert_eq!(h.icon, "issue", "slot-0 subject icon wins");

        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_UNBLOCKED,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(h.text, "ENG-1421: payments timeout is now unblocked");

        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_APPROVAL_REQUESTED,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(h.text, "Approval requested on ENG-1421: payments timeout");

        assert_eq!(before, 3, "the three Issues SLA-surface strings");
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
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert!(
            !h.text.contains("payments timeout"),
            "the title NEVER leaks to a denied viewer (NOTIF-D4)"
        );
        assert!(
            h.text.contains("a restricted issue"),
            "the denied subject renders as the PII-free tombstone display"
        );
    }

    #[test]
    fn wire_registers_both_reason_set_and_templates() {
        let mut reg = NotifRuleRegistry::platform_default();
        let mut store = TemplateStore::with_platform_defaults();
        let rules_before = reg.len();

        wire_issues_my_work(&mut reg, &mut store);

        assert_eq!(
            reg.len(),
            rules_before + 5,
            "the Issues reason set wired into the ONE registry"
        );
        assert!(
            store
                .lookup(PLATFORM_DEFAULT_TENANT, TPL_SLA_AT_RISK, DEFAULT_LOCALE)
                .is_some(),
            "the SLA at-risk string is on the ONE templating surface"
        );
        assert!(store
            .lookup(PLATFORM_DEFAULT_TENANT, TPL_UNBLOCKED, DEFAULT_LOCALE)
            .is_some());
        assert!(store
            .lookup(
                PLATFORM_DEFAULT_TENANT,
                TPL_APPROVAL_REQUESTED,
                DEFAULT_LOCALE
            )
            .is_some());
    }

    #[test]
    fn e2e_assign_appears_in_my_work_then_snooze_reflects_in_inbox() {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item(
            "iss-assigned",
            "myelin://acme/issue/issue/ENG-1421",
            Reason::Assigned,
        ));

        let mine = list_my_work_default(&inbox, &me(), &at());
        assert!(
            mine.items.iter().any(|i| i.item_id == "iss-assigned"),
            "the assigned issue appears in My Work"
        );

        myelin_notif::snooze(&inbox, &me(), "iss-assigned", "2026-07-01T09:00:00Z")
            .expect("snooze my own item");

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
            .expect("the row is still in the ONE store (snooze does not delete)");
        assert_eq!(
            row.state, "snoozed",
            "the snooze reflected in the unified inbox"
        );
        assert_eq!(
            row.snooze_until.as_deref(),
            Some("2026-07-01T09:00:00Z"),
            "the until is recorded on the ONE row"
        );
        let active = myelin_notif::active_inbox(full.items.clone());
        assert!(
            !active.iter().any(|i| i.item_id == "iss-assigned"),
            "the snoozed item is suppressed from the active inbox (one read-state truth)"
        );
        let mine_active =
            myelin_notif::active_inbox(list_my_work_default(&inbox, &me(), &at()).items);
        assert!(
            !mine_active.iter().any(|i| i.item_id == "iss-assigned"),
            "the snooze reflected in My Work too (one store)"
        );
    }
}
