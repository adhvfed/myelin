use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::define_rule::{
    define_notif_rule, platform_default_reason, DedupTpl, DefineRuleError, NotifRuleRegistry,
};
use myelin_notif::ranking::{reason_base_class, DeterministicV1, RankStrategy};
use myelin_notif::router::RoutedInboxItem;
use myelin_notif::{Class, Reason};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn viewer() -> Principal {
    Principal::stub(PrincipalId("u1".into()), PrincipalKind::Human, tenant())
}
fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())
}

#[test]
fn provider_define_notif_rule_constructs_the_rule_shape() {
    let rule = define_notif_rule(
        Reason::Mentioned,
        DedupTpl("mention:{recipient}:{subject}".into()),
        Class::Direct,
    )
    .expect("mentioned → direct is the §3.1 table band");
    assert_eq!(rule.reason, Reason::Mentioned);
    assert_eq!(rule.default_class, Class::Direct);
    assert_eq!(
        rule.dedup_key("psn:bob", &subject()),
        "mention:psn:bob:myelin://acme/issues/issue/PROJ-1",
        "the dedup_tpl renders the §3.2 collapse key for (recipient, subject)"
    );
}

#[test]
fn provider_registry_classifies_registered_then_default() {
    let mut reg = NotifRuleRegistry::new();
    reg.register(
        "iss_sla_breach",
        define_notif_rule(
            Reason::Sla,
            DedupTpl("sla:{subject}".into()),
            Class::Critical,
        )
        .unwrap(),
    );

    let c = reg.classify("iss_sla_breach", "psn:bob", &subject());
    assert_eq!(c.reason, Reason::Sla);
    assert_eq!(c.default_class, Class::Critical);
    assert_eq!(c.dedup_key, "sla:myelin://acme/issues/issue/PROJ-1");
    assert!(c.from_registered_rule);

    let (default_reason, default_class) = platform_default_reason();
    let d = reg.classify("not.registered", "psn:bob", &subject());
    assert_eq!(d.reason, default_reason);
    assert_eq!(d.default_class, default_class);
    assert!(!d.from_registered_rule);
}

#[test]
fn provider_define_notif_rule_reconciles_class_against_the_ranking_table() {
    assert!(define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Critical).is_ok());
    let err = define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Fyi)
        .expect_err("sla → fyi disagrees with the §3.1 table (sla is critical)");
    assert_eq!(
        err,
        DefineRuleError::ClassMismatch {
            reason: Reason::Sla,
            supplied: Class::Fyi,
            table: Class::Critical,
        }
    );
}

#[test]
fn provider_registered_class_drives_the_ranking_band() {
    let mut reg = NotifRuleRegistry::new();
    reg.register(
        "git_review",
        define_notif_rule(
            Reason::ReviewRequested,
            DedupTpl("{subject}".into()),
            Class::Direct,
        )
        .unwrap(),
    );
    let c = reg.classify("git_review", "psn:bob", &subject());

    let item = RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: "itm-1".into(),
        recipient: "psn:bob".into(),
        subject: subject(),
        reason: c.reason,
        class: c.default_class,
        origin_event: ArtifactRef("myelin://acme/bus/event/e1".into()),
        dedup_key: c.dedup_key.clone(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    };
    let (priority, trace) = DeterministicV1::default().score(&viewer(), &item);
    assert_eq!(trace.class, c.default_class);
    assert_eq!(trace.class, reason_base_class(c.reason).1);
    assert_eq!(
        priority,
        reason_base_class(c.reason).0,
        "the base from the table drives the rank"
    );
}

#[test]
fn consumer_subsystem_registers_a_whole_set_with_zero_notif_change() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    reg.register(
        "widgets.assigned",
        define_notif_rule(
            Reason::Assigned,
            DedupTpl("w:assign:{subject}".into()),
            Class::Direct,
        )
        .unwrap(),
    )
    .register(
        "widgets.escalated",
        define_notif_rule(
            Reason::Escalated,
            DedupTpl("w:esc:{subject}".into()),
            Class::Critical,
        )
        .unwrap(),
    )
    .register(
        "widgets.watched",
        define_notif_rule(
            Reason::Watched,
            DedupTpl("w:watch:{recipient}".into()),
            Class::Watching,
        )
        .unwrap(),
    );
    assert_eq!(
        reg.len(),
        before + 3,
        "three subsystem rules accreted (no Notif enum/match edit)"
    );

    let assigned = reg.classify("widgets.assigned", "psn:dana", &subject());
    assert_eq!(assigned.reason, Reason::Assigned);
    assert_eq!(assigned.default_class, Class::Direct);
    assert!(assigned.from_registered_rule);

    let escalated = reg.classify("widgets.escalated", "psn:dana", &subject());
    assert_eq!(escalated.reason, Reason::Escalated);
    assert_eq!(escalated.default_class, Class::Critical);

    let watched = reg.classify("widgets.watched", "psn:dana", &subject());
    assert_eq!(watched.reason, Reason::Watched);
    assert_eq!(watched.dedup_key, "w:watch:psn:dana");
}

#[test]
fn consumer_provider_round_trip_on_the_rule_wire() {
    let mut reg = NotifRuleRegistry::new();
    let rule = define_notif_rule(
        Reason::ApprovalRequested,
        DedupTpl("approval:{recipient}:{subject}".into()),
        Class::Critical,
    )
    .unwrap();
    reg.register("agent.approval", rule.clone());

    let c = reg.classify("agent.approval", "psn:agent-owner", &subject());
    assert_eq!(
        c.reason, rule.reason,
        "the classified reason is the registered reason"
    );
    assert_eq!(
        c.default_class, rule.default_class,
        "the classified band is the registered band"
    );
    assert_eq!(
        c.dedup_key,
        rule.dedup_key("psn:agent-owner", &subject()),
        "the classified dedup key is the registered template rendered for (recipient, subject)"
    );
}
