use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_identity::{
    DataRole as IdentityDataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
    PseudonymHandle,
};
use myelin_notif::{define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason};
use myelin_query::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

use crate::lifecycle::ReviewState;
use crate::pr_store::PrRecord;
use crate::rebac_fragment::object_types;

pub const GIT_REVIEW_REQUESTED_RULE: &str = "git.review_requested";

pub const GIT_MENTIONED_RULE: &str = "git.mentioned";

pub const GIT_WATCHED_RULE: &str = "git.watched";

fn review_requested_rule() -> Result<NotifRule, myelin_notif::DefineRuleError> {
    define_notif_rule(
        Reason::ReviewRequested,
        DedupTpl("git-review:{subject}".into()),
        Class::Direct,
    )
}

pub fn git_notif_rules() -> Result<Vec<(&'static str, NotifRule)>, myelin_notif::DefineRuleError> {
    Ok(vec![
        (GIT_REVIEW_REQUESTED_RULE, review_requested_rule()?),
        (
            GIT_MENTIONED_RULE,
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("git-mention:{recipient}:{subject}".into()),
                Class::Direct,
            )?,
        ),
        (
            GIT_WATCHED_RULE,
            define_notif_rule(
                Reason::Watched,
                DedupTpl("git-watched:{subject}".into()),
                Class::Watching,
            )?,
        ),
    ])
}

pub fn register_git_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in git_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

pub(crate) fn review_request_opened_signal_drafts(
    tenant: &TenantId,
    region: &Region,
    repo: &str,
    record: &PrRecord,
    recorded_at: &str,
) -> Result<Vec<EventDraft>, ReviewSignalError> {
    let rule = review_requested_rule()?;
    let subject = ArtifactRef(format!(
        "myelin://{}/git/pr/{repo}:{}",
        tenant.0, record.number
    ));
    let mut drafts = Vec::new();
    for review in &record.reviews {
        if !matches!(review.state, ReviewState::Requested) {
            continue;
        }
        if let Some(draft) = review_request_signal_draft(
            tenant,
            region,
            &rule,
            &subject,
            &review.reviewer_pseudonym,
            recorded_at,
            SignalState::Open,
        )? {
            drafts.push(draft);
        }
    }
    Ok(drafts)
}

pub(crate) fn review_request_resolved_signal_draft(
    tenant: &TenantId,
    region: &Region,
    repo: &str,
    record: &PrRecord,
    recorded_at: &str,
) -> Result<Option<EventDraft>, ReviewSignalError> {
    let rule = review_requested_rule()?;
    let Some(review) = record
        .reviews
        .last()
        .filter(|review| matches!(review.state, ReviewState::Submitted(_)))
    else {
        return Ok(None);
    };
    let subject = ArtifactRef(format!(
        "myelin://{}/git/pr/{repo}:{}",
        tenant.0, record.number
    ));
    review_request_signal_draft(
        tenant,
        region,
        &rule,
        &subject,
        &review.reviewer_pseudonym,
        recorded_at,
        SignalState::Resolved,
    )
}

#[derive(Debug)]
pub(crate) enum ReviewSignalError {
    Rule(myelin_notif::DefineRuleError),
    Encode(serde_json::Error),
}

impl std::fmt::Display for ReviewSignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rule(error) => write!(f, "define review notification rule: {error}"),
            Self::Encode(error) => write!(f, "encode review notification signal: {error}"),
        }
    }
}

impl std::error::Error for ReviewSignalError {}

impl From<myelin_notif::DefineRuleError> for ReviewSignalError {
    fn from(error: myelin_notif::DefineRuleError) -> Self {
        Self::Rule(error)
    }
}

impl From<serde_json::Error> for ReviewSignalError {
    fn from(error: serde_json::Error) -> Self {
        Self::Encode(error)
    }
}

fn review_request_signal_draft(
    tenant: &TenantId,
    region: &Region,
    rule: &NotifRule,
    subject: &ArtifactRef,
    reviewer_pseudonym: &str,
    recorded_at: &str,
    state: SignalState,
) -> Result<Option<EventDraft>, ReviewSignalError> {
    let Some(handle) = PseudonymHandle::parse(reviewer_pseudonym) else {
        return Ok(None);
    };
    if handle.tenant() != tenant.0 {
        return Ok(None);
    }
    let recipient_id = handle.pseudonym();
    let dedup_key = rule.dedup_key(recipient_id, subject);
    let signal = Signal {
        rule_id: RuleId(GIT_REVIEW_REQUESTED_RULE.into()),
        tenant: tenant.clone(),
        severity: Severity::Notice,
        dedup_key: DedupKey(dedup_key.clone()),
        subject: subject.clone(),
        count: 1,
        state,
        first_seen: recorded_at.to_string(),
        last_seen: recorded_at.to_string(),
    };
    let recipient = Principal::new(
        tenant.clone(),
        region.clone(),
        PrincipalId(recipient_id.to_string()),
        PrincipalKind::Human,
        IdentityDataRole::Controller,
        PrincipalStatus::Active,
    );
    let aggregate_id = &blake3::hash(dedup_key.as_bytes()).to_hex()[..32];
    let mut payload = serde_json::to_value(signal)?;
    payload["mentions"] = serde_json::json!([{ "Mention": recipient }]);
    payload["notification_reason"] = serde_json::to_value(rule.reason)?;
    Ok(Some(EventDraft {
        type_: EventType(
            match state {
                SignalState::Open => "signal.opened",
                SignalState::Resolved => "signal.resolved",
            }
            .into(),
        ),
        subject: ArtifactRef(format!(
            "sig.{}.{}.{}",
            tenant.0,
            Severity::Notice.token(),
            GIT_REVIEW_REQUESTED_RULE
        )),
        aggregate: AggregateKey(format!("signal:{aggregate_id}")),
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }))
}

pub const GIT_WATCHER_RELATION: &str = myelin_notif::WATCHER_RELATION;

pub fn git_watchable_object_types() -> [&'static str; 2] {
    [object_types::REPO, object_types::PULL_REQUEST]
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_notif::reason_base_class;
    use myelin_query::Signal;

    use crate::lifecycle::PullRequest;
    use crate::pr_store::ReviewRecord;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    #[test]
    fn git_rules_are_table_correct_review_mention_watched() {
        let rules = git_notif_rules().expect("git's set is table-correct by construction");
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                GIT_REVIEW_REQUESTED_RULE,
                GIT_MENTIONED_RULE,
                GIT_WATCHED_RULE
            ]
        );
        for (key, rule) in &rules {
            assert_eq!(
                rule.default_class,
                reason_base_class(rule.reason).1,
                "rule `{key}` must register the §3.1 band for its reason"
            );
        }
        assert_eq!(rules[0].1.reason, Reason::ReviewRequested);
        assert_eq!(rules[0].1.default_class, Class::Direct);
        assert_eq!(rules[1].1.reason, Reason::Mentioned);
        assert_eq!(rules[1].1.default_class, Class::Direct);
        assert_eq!(rules[2].1.reason, Reason::Watched);
        assert_eq!(rules[2].1.default_class, Class::Watching);
    }

    #[test]
    fn review_request_signals_are_recipient_scoped_and_rule_classified() {
        let pr = PullRequest::open(
            7,
            "refs/heads/main",
            "refs/heads/feature",
            "author@acme.noreply",
            false,
        );
        let mut record = PrRecord::open(&pr, "1".repeat(40));
        record.reviews = vec![
            ReviewRecord {
                reviewer_pseudonym: "alice@acme.noreply".into(),
                state: ReviewState::Requested,
                is_agent: false,
            },
            ReviewRecord {
                reviewer_pseudonym: "bob@acme.noreply".into(),
                state: ReviewState::Submitted(crate::lifecycle::ReviewVerdict::Approve),
                is_agent: false,
            },
            ReviewRecord {
                reviewer_pseudonym: "mallory@other.noreply".into(),
                state: ReviewState::Requested,
                is_agent: false,
            },
            ReviewRecord {
                reviewer_pseudonym: "not-a-pseudonym".into(),
                state: ReviewState::Requested,
                is_agent: false,
            },
        ];

        let drafts = review_request_opened_signal_drafts(
            &tenant(),
            &Region("fr-par".into()),
            "core",
            &record,
            "2026-08-09T12:00:00Z",
        )
        .unwrap();
        assert_eq!(drafts.len(), 1, "only the outstanding request is signalled");
        let draft = &drafts[0];
        assert_eq!(draft.type_.0, "signal.opened");
        assert_eq!(draft.subject.0, "sig.acme.notice.git.review_requested");
        assert!(draft.aggregate.0.starts_with("signal:"));
        assert!(!draft.aggregate.0.contains('.'));
        assert_eq!(draft.payload["notification_reason"], "review_requested");
        assert_eq!(
            draft.payload["mentions"][0]["Mention"]["principal_id"],
            "alice"
        );
        let signal: Signal = serde_json::from_value(draft.payload.clone()).unwrap();
        assert_eq!(signal.rule_id.0, GIT_REVIEW_REQUESTED_RULE);
        assert_eq!(signal.subject.0, "myelin://acme/git/pr/core:7");
        assert_eq!(signal.first_seen, "2026-08-09T12:00:00Z");

        record.reviews.push(ReviewRecord {
            reviewer_pseudonym: "alice@acme.noreply".into(),
            state: ReviewState::Submitted(crate::lifecycle::ReviewVerdict::Approve),
            is_agent: false,
        });
        let resolved = review_request_resolved_signal_draft(
            &tenant(),
            &Region("fr-par".into()),
            "core",
            &record,
            "2026-08-09T12:05:00Z",
        )
        .unwrap()
        .expect("the submitted review resolves its request signal");
        assert_eq!(resolved.type_.0, "signal.resolved");
        assert_eq!(resolved.aggregate, draft.aggregate);
        let signal: Signal = serde_json::from_value(resolved.payload).unwrap();
        assert_eq!(signal.state, SignalState::Resolved);
        assert_eq!(signal.last_seen, "2026-08-09T12:05:00Z");
    }

    #[test]
    fn git_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_git_notif_rules(&mut reg).expect("git's set registers");
        assert_eq!(
            reg.len(),
            before + 3,
            "git's three rules accreted (no Notif enum/match edit)"
        );

        let subject = myelin_refs::ArtifactRef("myelin://acme/git/pr/9".into());
        let c = reg.classify(GIT_REVIEW_REQUESTED_RULE, "psn:reviewer", &subject);
        assert_eq!(c.reason, Reason::ReviewRequested);
        assert_eq!(c.default_class, Class::Direct);
        assert!(
            c.from_registered_rule,
            "the Git registration took effect (0 Notif change)"
        );
        assert_eq!(c.dedup_key, "git-review:myelin://acme/git/pr/9");

        let m = reg.classify(GIT_MENTIONED_RULE, "psn:bob", &subject);
        assert_eq!(m.reason, Reason::Mentioned);
        assert_eq!(m.dedup_key, "git-mention:psn:bob:myelin://acme/git/pr/9");
    }

    #[test]
    fn git_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_git_notif_rules(&mut reg).unwrap();
        register_git_notif_rules(&mut reg).unwrap();
        assert_eq!(
            reg.len(),
            3,
            "re-registering Git's set keeps three rules (idempotent)"
        );
    }

    #[test]
    fn git_watcher_relation_matches_the_frozen_fragment() {
        assert_eq!(GIT_WATCHER_RELATION, "watcher");
        assert_eq!(GIT_WATCHER_RELATION, myelin_notif::WATCHER_RELATION);
        assert_eq!(git_watchable_object_types(), ["repo", "pull_request"]);
        let repo_rels: Vec<String> = crate::rebac_fragment::repo_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(repo_rels.contains(&"watcher".to_string()));
        let pr_rels: Vec<String> = crate::rebac_fragment::pull_request_fragment()
            .relations
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert!(pr_rels.contains(&"watcher".to_string()));
    }
}
