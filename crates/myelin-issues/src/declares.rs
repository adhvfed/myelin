use std::collections::BTreeMap;

use myelin_notif::{define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason};
use myelin_query::FieldType;
use myelin_search::IndexSpec;

use crate::rebac_fragment::object_types;

pub const ISSUE_SUBSYSTEM: &str = "issue";

pub const ISSUE_TYPE: &str = "issue";

pub const FACET_STATE_CATEGORY: &str = "state_category";
pub const FACET_PRIORITY: &str = "priority";
pub const FACET_ASSIGNEE: &str = "assignee";
pub const FACET_TYPE_RANK: &str = "type_rank";
pub const FACET_PROJECT_ID: &str = "project_id";
pub const FACET_CYCLE_ID: &str = "cycle_id";
pub const FACET_RANK: &str = "rank";

pub fn issue_facets_projection_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    struct_fields.insert(FACET_STATE_CATEGORY.to_string(), FieldType::Select);
    struct_fields.insert(FACET_PRIORITY.to_string(), FieldType::Int);
    struct_fields.insert(FACET_ASSIGNEE.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_TYPE_RANK.to_string(), FieldType::Int);
    struct_fields.insert(FACET_PROJECT_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_CYCLE_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_RANK.to_string(), FieldType::OrderKey);

    IndexSpec::new(ISSUE_SUBSYSTEM, ISSUE_TYPE, struct_fields)
        .with_acl_object_type(object_types::ISSUE)
}

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
            .expect("Reason::Assigned reconciles to Class::Direct in the §3.1 table"),
        ),
        (
            RULE_KEY_BLOCKED,
            define_notif_rule(
                Reason::Blocked,
                DedupTpl("issue.blocked:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("Reason::Blocked reconciles to Class::Watching in the §3.1 table"),
        ),
        (
            RULE_KEY_SLA_AT_RISK,
            define_notif_rule(
                Reason::Sla,
                DedupTpl("issue.sla:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("Reason::Sla reconciles to Class::Critical in the §3.1 table"),
        ),
        (
            RULE_KEY_UNBLOCKED,
            define_notif_rule(
                Reason::Unblocked,
                DedupTpl("issue.unblocked:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("Reason::Unblocked reconciles to Class::Watching in the §3.1 table"),
        ),
        (
            RULE_KEY_APPROVAL_REQUESTED,
            define_notif_rule(
                Reason::ApprovalRequested,
                DedupTpl("issue.approval:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("Reason::ApprovalRequested reconciles to Class::Critical in the §3.1 table"),
        ),
    ]
}

pub fn register_issue_notif_rules(registry: &mut NotifRuleRegistry) -> &mut NotifRuleRegistry {
    for (key, rule) in issue_notif_rules() {
        registry.register(key, rule);
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_issues_owned_6_3_shape() {
        let s = issue_facets_projection_spec();
        assert_eq!(
            s.subsystem, "issue",
            "Issues owns the `issue` subsystem projection"
        );
        assert_eq!(s.type_, "issue", "the indexed artifact type is an issue");
        assert_eq!(
            s.acl_object_type, "issue",
            "an issue's reachability is its own ReBAC `view` permission (no parent ACL object)"
        );
        assert_eq!(
            s.acl_object_type,
            object_types::ISSUE,
            "the acl_object_type is exactly Issues' frozen ReBAC `issue` object type"
        );
        assert!(
            !s.semantic,
            "Issues is trigram-title + facet filter in v1, not vector-embedded"
        );
        assert_eq!(
            s.struct_fields.len(),
            7,
            "exactly the seven structured issue facets"
        );
        assert_eq!(
            s.struct_fields.get("state_category"),
            Some(&FieldType::Select)
        );
        assert_eq!(s.struct_fields.get("priority"), Some(&FieldType::Int));
        assert_eq!(s.struct_fields.get("assignee"), Some(&FieldType::Principal));
        assert_eq!(s.struct_fields.get("type_rank"), Some(&FieldType::Int));
        assert_eq!(
            s.struct_fields.get("project_id"),
            Some(&FieldType::Relation)
        );
        assert_eq!(s.struct_fields.get("cycle_id"), Some(&FieldType::Relation));
        assert_eq!(s.struct_fields.get("rank"), Some(&FieldType::OrderKey));
    }

    #[test]
    fn fulltext_body_is_not_a_struct_facet() {
        let s = issue_facets_projection_spec();
        for absent in ["title", "body", "props", "comment", "description"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    #[test]
    fn spec_serializes_to_the_6_3_wire_shape() {
        let s = issue_facets_projection_spec();
        let json = serde_json::to_value(&s).expect("the spec serializes");
        let obj = json.as_object().expect("the spec is a JSON object");

        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "acl_object_type",
                "semantic",
                "struct_fields",
                "subsystem",
                "type"
            ],
            "the 6.3 wire key set"
        );

        assert_eq!(obj["subsystem"], serde_json::json!("issue"));
        assert_eq!(obj["type"], serde_json::json!("issue"));
        assert_eq!(obj["semantic"], serde_json::json!(false));
        assert_eq!(obj["acl_object_type"], serde_json::json!("issue"));
        assert_eq!(
            obj["struct_fields"],
            serde_json::json!({
                "state_category": "Select",
                "priority": "Int",
                "assignee": "Principal",
                "type_rank": "Int",
                "project_id": "Relation",
                "cycle_id": "Relation",
                "rank": "OrderKey",
            }),
            "the structured facets serialize to the typed columnar shape (13.3)"
        );
    }

    #[test]
    fn notif_rules_are_the_issues_reasons_at_their_bands() {
        let rules = issue_notif_rules();
        assert_eq!(
            rules.len(),
            5,
            "the five distinct Issues consumer reasons (assigned / blocked / SLA / unblocked / approval)"
        );

        let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();

        let asg = by_key
            .get(RULE_KEY_ASSIGNED)
            .expect("assigned rule registered");
        assert_eq!(asg.reason, Reason::Assigned);
        assert_eq!(
            asg.default_class,
            Class::Direct,
            "assigned is a direct target"
        );

        let blk = by_key
            .get(RULE_KEY_BLOCKED)
            .expect("blocked rule registered");
        assert_eq!(blk.reason, Reason::Blocked);
        assert_eq!(
            blk.default_class,
            Class::Watching,
            "blocked re-surfaces calmly"
        );

        let sla = by_key
            .get(RULE_KEY_SLA_AT_RISK)
            .expect("SLA rule registered");
        assert_eq!(sla.reason, Reason::Sla);
        assert_eq!(
            sla.default_class,
            Class::Critical,
            "SLA at-risk pierces (critical)"
        );

        let unb = by_key
            .get(RULE_KEY_UNBLOCKED)
            .expect("unblocked rule registered");
        assert_eq!(unb.reason, Reason::Unblocked);
        assert_eq!(
            unb.default_class,
            Class::Watching,
            "unblocked re-surfaces calmly (watching)"
        );

        let appr = by_key
            .get(RULE_KEY_APPROVAL_REQUESTED)
            .expect("approval rule registered");
        assert_eq!(appr.reason, Reason::ApprovalRequested);
        assert_eq!(
            appr.default_class,
            Class::Critical,
            "approval is a HITL interrupt (critical)"
        );
    }

    #[test]
    fn notif_rules_register_and_classify_through_notif() {
        let subject = myelin_refs::ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());

        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_issue_notif_rules(&mut reg);
        assert_eq!(
            reg.len(),
            before + 5,
            "the five Issues rules accreted (no Notif change)"
        );

        let c = reg.classify(RULE_KEY_ASSIGNED, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Assigned);
        assert_eq!(c.default_class, Class::Direct);
        assert!(
            c.from_registered_rule,
            "the registered Issues rule took effect"
        );

        let c = reg.classify(RULE_KEY_BLOCKED, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Blocked);
        assert_eq!(c.default_class, Class::Watching);
        assert!(c.from_registered_rule);

        let c = reg.classify(RULE_KEY_SLA_AT_RISK, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Sla);
        assert_eq!(c.default_class, Class::Critical);
        assert!(
            c.from_registered_rule,
            "the registered Issues rule took effect"
        );
        assert_eq!(
            c.dedup_key,
            "issue.sla:psn:alice:myelin://acme/issue/issue/ENG-1421"
        );

        let c = reg.classify(RULE_KEY_UNBLOCKED, "psn:bob", &subject);
        assert_eq!(c.reason, Reason::Unblocked);
        assert_eq!(c.default_class, Class::Watching);
        assert!(c.from_registered_rule);

        let c = reg.classify(RULE_KEY_APPROVAL_REQUESTED, "psn:carol", &subject);
        assert_eq!(c.reason, Reason::ApprovalRequested);
        assert_eq!(c.default_class, Class::Critical);
        assert!(c.from_registered_rule);
    }

    #[test]
    fn issues_cannot_smuggle_a_reason_into_the_wrong_band() {
        let err = define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Watching)
            .expect_err("SLA must register at the critical band the §3.1 table owns");
        assert!(matches!(
            err,
            myelin_notif::DefineRuleError::ClassMismatch { .. }
        ));
    }
}
