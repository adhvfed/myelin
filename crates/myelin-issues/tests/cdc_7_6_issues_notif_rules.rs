use std::collections::BTreeMap;

use myelin_issues::declares::{
    issue_notif_rules, register_issue_notif_rules, RULE_KEY_APPROVAL_REQUESTED, RULE_KEY_ASSIGNED,
    RULE_KEY_BLOCKED, RULE_KEY_SLA_AT_RISK, RULE_KEY_UNBLOCKED,
};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, DefineRuleError, NotifRule, NotifRuleRegistry, Reason,
};
use myelin_refs::ArtifactRef;

fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/issue/issue/ENG-1421".into())
}

#[test]
fn producer_issues_declares_the_three_reasons_at_their_bands() {
    let rules = issue_notif_rules();
    assert_eq!(
        rules.len(),
        5,
        "the full Issues consumer reason set (NOTIF-P21)"
    );
    let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();

    let asg = by_key.get(RULE_KEY_ASSIGNED).expect("assigned rule");
    assert_eq!(asg.reason, Reason::Assigned);
    assert_eq!(asg.default_class, Class::Direct);

    let blk = by_key.get(RULE_KEY_BLOCKED).expect("blocked rule");
    assert_eq!(blk.reason, Reason::Blocked);
    assert_eq!(blk.default_class, Class::Watching);

    let sla = by_key.get(RULE_KEY_SLA_AT_RISK).expect("SLA rule");
    assert_eq!(sla.reason, Reason::Sla);
    assert_eq!(sla.default_class, Class::Critical);

    let unb = by_key.get(RULE_KEY_UNBLOCKED).expect("unblocked rule");
    assert_eq!(unb.reason, Reason::Unblocked);
    assert_eq!(unb.default_class, Class::Watching);

    let appr = by_key
        .get(RULE_KEY_APPROVAL_REQUESTED)
        .expect("approval rule");
    assert_eq!(appr.reason, Reason::ApprovalRequested);
    assert_eq!(appr.default_class, Class::Critical);
}

#[test]
fn producer_issues_cannot_re_band_a_reason() {
    let err = define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Watching)
        .expect_err("SLA must register at the critical band the §3.1 table owns");
    assert!(matches!(err, DefineRuleError::ClassMismatch { .. }));
}

#[test]
fn consumer_notif_admits_and_routes_the_reason_set() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_issue_notif_rules(&mut reg);
    assert_eq!(
        reg.len(),
        before + 5,
        "Notif admits the full Issues reason set (zero Notif change)"
    );

    let c = reg.classify(RULE_KEY_ASSIGNED, "psn:alice", &subject());
    assert_eq!(c.reason, Reason::Assigned);
    assert_eq!(c.default_class, Class::Direct);
    assert!(c.from_registered_rule);

    let c = reg.classify(RULE_KEY_BLOCKED, "psn:alice", &subject());
    assert_eq!(c.reason, Reason::Blocked);
    assert_eq!(c.default_class, Class::Watching);
    assert!(c.from_registered_rule);

    let c = reg.classify(RULE_KEY_SLA_AT_RISK, "psn:alice", &subject());
    assert_eq!(c.reason, Reason::Sla);
    assert_eq!(c.default_class, Class::Critical);
    assert!(c.from_registered_rule);
    assert_eq!(
        c.dedup_key,
        "issue.sla:psn:alice:myelin://acme/issue/issue/ENG-1421"
    );

    let c = reg.classify(RULE_KEY_UNBLOCKED, "psn:bob", &subject());
    assert_eq!(c.reason, Reason::Unblocked);
    assert_eq!(c.default_class, Class::Watching);
    assert!(c.from_registered_rule);

    let c = reg.classify(RULE_KEY_APPROVAL_REQUESTED, "psn:carol", &subject());
    assert_eq!(c.reason, Reason::ApprovalRequested);
    assert_eq!(c.default_class, Class::Critical);
    assert!(c.from_registered_rule);
}
