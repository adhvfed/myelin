use myelin_notif::{define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason};

pub const RULE_KEY_SLA_AT_RISK: &str = "issue.sla.at_risk";
pub const RULE_KEY_UNBLOCKED: &str = "issue.trigger.unblocked";
pub const RULE_KEY_APPROVAL_REQUESTED: &str = "issue.approval.requested";
pub const RULE_KEY_ASSIGNED: &str = "issue.assigned";
pub const RULE_KEY_BLOCKED: &str = "issue.blocked";

pub fn issue_notif_rules() -> Vec<(&'static str, NotifRule)> {
    vec![
        (
            RULE_KEY_ASSIGNED,
            define_notif_rule(
                Reason::Assigned,
                DedupTpl("issue.assigned:{recipient}:{subject}".to_string()),
                Class::Direct,
            )
            .expect("assigned reconciles to the direct notification class"),
        ),
        (
            RULE_KEY_BLOCKED,
            define_notif_rule(
                Reason::Blocked,
                DedupTpl("issue.blocked:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("blocked reconciles to the watching notification class"),
        ),
        (
            RULE_KEY_SLA_AT_RISK,
            define_notif_rule(
                Reason::Sla,
                DedupTpl("issue.sla:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("SLA reconciles to the critical notification class"),
        ),
        (
            RULE_KEY_UNBLOCKED,
            define_notif_rule(
                Reason::Unblocked,
                DedupTpl("issue.unblocked:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("unblocked reconciles to the watching notification class"),
        ),
        (
            RULE_KEY_APPROVAL_REQUESTED,
            define_notif_rule(
                Reason::ApprovalRequested,
                DedupTpl("issue.approval:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("approval requests reconcile to the critical notification class"),
        ),
    ]
}

pub fn register_issue_notif_rules(registry: &mut NotifRuleRegistry) -> &mut NotifRuleRegistry {
    for (key, rule) in issue_notif_rules() {
        registry.register(key, rule);
    }
    registry
}
