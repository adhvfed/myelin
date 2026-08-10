use myelin_events::validate_event_type;

pub const ISSUE_CREATED: &str = "issue.issue.created";
pub const ISSUE_UPDATED: &str = "issue.issue.updated";
pub const ISSUE_TRANSITIONED: &str = "issue.issue.transitioned";
pub const ISSUE_CLOSED: &str = "issue.issue.closed";
pub const ISSUE_REOPENED: &str = "issue.issue.reopened";
pub const ISSUE_DELETED: &str = "issue.issue.deleted";
pub const ISSUE_RESTORED: &str = "issue.issue.restored";
pub const ISSUE_ASSIGNED: &str = "issue.issue.assigned";
pub const ISSUE_PRIORITY_CHANGED: &str = "issue.issue.priority_changed";
pub const ISSUE_TYPE_CHANGED: &str = "issue.issue.type_changed";
pub const ISSUE_PARENT_CHANGED: &str = "issue.issue.parent_changed";
pub const ISSUE_ARCHIVED: &str = "issue.issue.archived";
pub const ISSUE_REORDERED: &str = "issue.issue.reordered";
pub const ISSUE_AUTHORIZATION_REQUESTED: &str = "issue.issue.authorization_requested";

pub const ISSUE_CREATE_ATTEMPTED: &str = "issue.create.attempted";
pub const ISSUE_CREATE_APPLIED: &str = "issue.create.applied";
pub const ISSUE_CREATE_GATED: &str = "issue.create.gated";
pub const ISSUE_CREATE_DENIED: &str = "issue.create.denied";
pub const ISSUE_CREATE_INDETERMINATE: &str = "issue.create.indeterminate";
pub const ISSUE_CREATE_GOVERNANCE_AUDIT_EVENT_TOKENS: &[&str] = &[
    ISSUE_CREATE_ATTEMPTED,
    ISSUE_CREATE_APPLIED,
    ISSUE_CREATE_GATED,
    ISSUE_CREATE_DENIED,
    ISSUE_CREATE_INDETERMINATE,
];

pub const ISSUE_TRIAGED: &str = "issue.issue.triaged";
pub const ISSUE_DUPLICATE_SUSPECTED: &str = "issue.issue.duplicate_suspected";
pub const ISSUE_LABELLED_BY_AGENT: &str = "issue.issue.labelled_by_agent";

pub const RELATION_CREATED: &str = "issue.relation.created";
pub const RELATION_REMOVED: &str = "issue.relation.removed";

pub const FIELD_DEFINED: &str = "issue.field.defined";
pub const FIELD_UPDATED: &str = "issue.field.updated";
pub const FIELD_REMOVED: &str = "issue.field.removed";

pub const COMMENT_CREATED: &str = "issue.comment.created";
pub const COMMENT_UPDATED: &str = "issue.comment.updated";
pub const COMMENT_DELETED: &str = "issue.comment.deleted";

pub const ROLLUP_RECOMPUTED: &str = "issue.rollup.recomputed";

pub const CYCLE_STARTED: &str = "issue.cycle.started";
pub const CYCLE_COMPLETED: &str = "issue.cycle.completed";
pub const CYCLE_ISSUE_ADDED: &str = "issue.cycle.issue_added";
pub const CYCLE_ISSUE_REMOVED: &str = "issue.cycle.issue_removed";

pub const MILESTONE_RELEASED: &str = "issue.milestone.released";
pub const MILESTONE_ISSUE_ADDED: &str = "issue.milestone.issue_added";
pub const MILESTONE_ISSUE_REMOVED: &str = "issue.milestone.issue_removed";

pub const ATTACHMENT_ADDED: &str = "issue.attachment.added";
pub const ATTACHMENT_REMOVED: &str = "issue.attachment.removed";

pub const SLA_STARTED: &str = "issue.sla.started";
pub const SLA_PAUSED: &str = "issue.sla.paused";
pub const SLA_RESUMED: &str = "issue.sla.resumed";
pub const SLA_AT_RISK: &str = "issue.sla.at_risk";
pub const SLA_BREACHED: &str = "issue.sla.breached";
pub const SLA_MET: &str = "issue.sla.met";

pub const APPROVAL_REQUESTED: &str = "issue.approval.requested";
pub const APPROVAL_GRANTED: &str = "issue.approval.granted";
pub const APPROVAL_REJECTED: &str = "issue.approval.rejected";
pub const APPROVAL_TIMED_OUT: &str = "issue.approval.timed_out";

pub const INITIATIVE_HEALTH_CHANGED: &str = "issue.initiative.health_changed";

pub const ISSUE_ERASED: &str = "issue.issue.erased";
pub const COMMENT_ERASED: &str = "issue.comment.erased";

pub const ISSUE_SNAPSHOT: &str = "issue.issue.snapshot";
pub const RELATION_SNAPSHOT: &str = "issue.relation.snapshot";
pub const COMMENT_SNAPSHOT: &str = "issue.comment.snapshot";
pub const ROLLUP_SNAPSHOT: &str = "issue.rollup.snapshot";

pub const ISSUE_EVENT_TOKENS: &[&str] = &[
    ISSUE_CREATED,
    ISSUE_UPDATED,
    ISSUE_TRANSITIONED,
    ISSUE_CLOSED,
    ISSUE_REOPENED,
    ISSUE_DELETED,
    ISSUE_RESTORED,
    ISSUE_ASSIGNED,
    ISSUE_PRIORITY_CHANGED,
    ISSUE_TYPE_CHANGED,
    ISSUE_PARENT_CHANGED,
    ISSUE_ARCHIVED,
    ISSUE_REORDERED,
    ISSUE_AUTHORIZATION_REQUESTED,
    ISSUE_CREATE_ATTEMPTED,
    ISSUE_CREATE_APPLIED,
    ISSUE_CREATE_GATED,
    ISSUE_CREATE_DENIED,
    ISSUE_CREATE_INDETERMINATE,
    ISSUE_TRIAGED,
    ISSUE_DUPLICATE_SUSPECTED,
    ISSUE_LABELLED_BY_AGENT,
    RELATION_CREATED,
    RELATION_REMOVED,
    FIELD_DEFINED,
    FIELD_UPDATED,
    FIELD_REMOVED,
    COMMENT_CREATED,
    COMMENT_UPDATED,
    COMMENT_DELETED,
    ROLLUP_RECOMPUTED,
    CYCLE_STARTED,
    CYCLE_COMPLETED,
    CYCLE_ISSUE_ADDED,
    CYCLE_ISSUE_REMOVED,
    MILESTONE_RELEASED,
    MILESTONE_ISSUE_ADDED,
    MILESTONE_ISSUE_REMOVED,
    ATTACHMENT_ADDED,
    ATTACHMENT_REMOVED,
    SLA_STARTED,
    SLA_PAUSED,
    SLA_RESUMED,
    SLA_AT_RISK,
    SLA_BREACHED,
    SLA_MET,
    APPROVAL_REQUESTED,
    APPROVAL_GRANTED,
    APPROVAL_REJECTED,
    APPROVAL_TIMED_OUT,
    INITIATIVE_HEALTH_CHANGED,
    ISSUE_ERASED,
    COMMENT_ERASED,
    ISSUE_SNAPSHOT,
    RELATION_SNAPSHOT,
    COMMENT_SNAPSHOT,
    ROLLUP_SNAPSHOT,
];

pub fn register_issue_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for &tok in ISSUE_EVENT_TOKENS {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

pub mod unit_check {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum UnitError {
        DurationNotSeconds { field: String },
        TimestampNotRfc3339Utc { field: String, value: String },
    }

    impl std::fmt::Display for UnitError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                UnitError::DurationNotSeconds { field } => write!(
                    f,
                    "`{field}`: a duration must be expressed in SECONDS (the frozen unit) - a \
                     `*_millis` key is the seconds-vs-millis drift the envelope anchor forbids"
                ),
                UnitError::TimestampNotRfc3339Utc { field, value } => write!(
                    f,
                    "`{field}`: timestamp `{value}` is not RFC-3339 UTC (it must be `T`-separated \
                     and `Z`-suffixed, e.g. `2026-06-21T10:00:00Z`)"
                ),
            }
        }
    }

    pub fn timestamp_is_rfc3339_utc(value: &str) -> bool {
        value.contains('T') && value.ends_with('Z')
    }

    pub fn validate_issue_payload_units(payload: &serde_json::Value) -> Result<(), UnitError> {
        let Some(obj) = payload.as_object() else {
            return Ok(());
        };
        for (key, val) in obj {
            if (key.ends_with("_millis") || key.ends_with("_ms")) && val.is_number() {
                return Err(UnitError::DurationNotSeconds { field: key.clone() });
            }
            if key.ends_with("_at") {
                match val.as_str() {
                    Some(s) if timestamp_is_rfc3339_utc(s) => {}
                    _ => {
                        return Err(UnitError::TimestampNotRfc3339Utc {
                            field: key.clone(),
                            value: val.to_string(),
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::unit_check::{timestamp_is_rfc3339_utc, validate_issue_payload_units, UnitError};
    use super::*;

    #[test]
    fn every_issue_token_parses_the_bus_grammar() {
        for &tok in ISSUE_EVENT_TOKENS {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered issue token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(
            register_issue_tokens().is_ok(),
            "register_issue_tokens() must succeed: {:?}",
            register_issue_tokens()
        );
    }

    #[test]
    fn every_issue_token_carries_the_issue_subsystem_prefix() {
        for &tok in ISSUE_EVENT_TOKENS {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "issue",
                "token `{tok}` must carry the `issue` subsystem prefix"
            );
        }
        assert!(
            myelin_events::SUBSYSTEM_TOKENS.contains(&"issue"),
            "`issue` must be a canonical Bus subsystem token"
        );
    }

    #[test]
    fn the_initiative_type_token_is_registered_and_grammatical() {
        assert!(ISSUE_EVENT_TOKENS.contains(&INITIATIVE_HEALTH_CHANGED));
        assert!(validate_event_type(INITIATIVE_HEALTH_CHANGED).is_ok());
        assert_eq!(
            INITIATIVE_HEALTH_CHANGED.split('.').nth(1),
            Some("initiative")
        );
        assert!(
            myelin_events::ARTIFACT_TYPE_TOKENS.contains(&"initiative"),
            "`initiative` must be a registered Bus artifact-type token (recon §2 / §6.2)"
        );
    }

    #[test]
    fn the_issue_token_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for &tok in ISSUE_EVENT_TOKENS {
            assert!(
                seen.insert(tok),
                "issue token `{tok}` is registered more than once"
            );
        }
        assert_eq!(seen.len(), ISSUE_EVENT_TOKENS.len());
    }

    #[test]
    fn the_load_bearing_issue_tokens_are_registered() {
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_UPDATED));
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_TRANSITIONED));
        assert!(ISSUE_EVENT_TOKENS.contains(&RELATION_CREATED));
        assert!(ISSUE_EVENT_TOKENS.contains(&SLA_AT_RISK));
        assert!(ISSUE_EVENT_TOKENS.contains(&INITIATIVE_HEALTH_CHANGED));
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_ERASED));
        assert!(ISSUE_EVENT_TOKENS.contains(&ISSUE_SNAPSHOT));
    }

    #[test]
    fn issues_registers_no_foreign_subsystem_tokens() {
        for &tok in ISSUE_EVENT_TOKENS {
            assert!(
                tok.starts_with("issue."),
                "issue must not register the foreign-subsystem token `{tok}`"
            );
        }
    }

    #[test]
    fn issue_payload_in_frozen_units_validates() {
        let ok = serde_json::json!({
            "issue": "myelin://acme/issue/issue/ENG-1421",
            "target_seconds": 86_400,
            "stale_after_seconds": 2_592_000,
            "started_at": "2026-06-21T10:00:00Z"
        });
        assert_eq!(
            validate_issue_payload_units(&ok),
            Ok(()),
            "an issue payload in the frozen units (seconds + RFC-3339 UTC) must validate"
        );
    }

    #[test]
    fn seconds_vs_millis_fixture_is_rejected() {
        let drifted = serde_json::json!({
            "issue": "myelin://acme/issue/issue/ENG-1421",
            "target_millis": 86_400_000,
            "started_at": "2026-06-21T10:00:00Z"
        });
        assert_eq!(
            validate_issue_payload_units(&drifted),
            Err(UnitError::DurationNotSeconds {
                field: "target_millis".into()
            }),
            "a millis-expressed duration must be REJECTED (the frozen unit is seconds)"
        );
        let drifted_ms = serde_json::json!({ "stale_after_ms": 2_592_000_000u64 });
        assert!(matches!(
            validate_issue_payload_units(&drifted_ms),
            Err(UnitError::DurationNotSeconds { .. })
        ));
    }

    #[test]
    fn non_rfc3339_timestamp_in_a_payload_is_rejected() {
        let epoch = serde_json::json!({ "occurred_at": 1_718_960_000_000u64 });
        assert!(matches!(
            validate_issue_payload_units(&epoch),
            Err(UnitError::TimestampNotRfc3339Utc { .. })
        ));
        let local = serde_json::json!({ "transitioned_at": "2026-06-21T10:00:00" });
        assert!(matches!(
            validate_issue_payload_units(&local),
            Err(UnitError::TimestampNotRfc3339Utc { .. })
        ));
        assert!(timestamp_is_rfc3339_utc("2026-06-21T10:00:00Z"));
        assert!(!timestamp_is_rfc3339_utc("2026-06-21T10:00:00"));
        assert!(!timestamp_is_rfc3339_utc("1718960000000"));
    }
}
