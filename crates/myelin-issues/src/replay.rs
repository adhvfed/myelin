use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueReplayKind {
    Issue,
    Relation,
    Comment,
    Rollup,
}

impl IssueReplayKind {
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                IssueReplayKind::Issue => events::ISSUE_SNAPSHOT,
                IssueReplayKind::Relation => events::RELATION_SNAPSHOT,
                IssueReplayKind::Comment => events::COMMENT_SNAPSHOT,
                IssueReplayKind::Rollup => events::ROLLUP_SNAPSHOT,
            }
            .to_string(),
        )
    }

    fn from_selector(selector: &str) -> Option<IssueReplayKind> {
        match selector.split(':').next() {
            Some("issue") => Some(IssueReplayKind::Issue),
            Some("relation") => Some(IssueReplayKind::Relation),
            Some("comment") => Some(IssueReplayKind::Comment),
            Some("rollup") => Some(IssueReplayKind::Rollup),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IssueTruthRow {
    kind: IssueReplayKind,
    version: u64,
    payload: serde_json::Value,
    subject: ArtifactRef,
}

#[derive(Debug, Default)]
pub struct IssueReindexSource {
    truth: BTreeMap<String, IssueTruthRow>,
}

impl IssueReindexSource {
    pub fn new() -> IssueReindexSource {
        IssueReindexSource::default()
    }

    pub fn upsert(
        &mut self,
        kind: IssueReplayKind,
        aggregate: &str,
        version: u64,
        subject: &str,
        mut payload: serde_json::Value,
    ) {
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert("version".into(), serde_json::json!(version));
        }
        self.truth.insert(
            aggregate.to_string(),
            IssueTruthRow {
                kind,
                version,
                payload,
                subject: ArtifactRef(subject.to_string()),
            },
        );
    }

    pub fn erase(&mut self, aggregate: &str) -> bool {
        self.truth.remove(aggregate).is_some()
    }
}

fn matches_aggregate(agg: &str, target: &str) -> bool {
    if agg == target {
        return true;
    }
    agg.strip_suffix(target)
        .and_then(|head| head.chars().next_back())
        .is_some_and(|boundary| boundary == '/' || boundary == '#')
}

impl ReindexSource for IssueReindexSource {
    fn owner_token(&self) -> &str {
        "issue"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let kind = match IssueReplayKind::from_selector(&scope.selector) {
            Some(k) => k,
            None => return Vec::new(),
        };
        let target = scope
            .selector
            .split_once(':')
            .map(|(_, rest)| rest)
            .unwrap_or("");
        self.truth
            .iter()
            .filter(|(_, row)| row.kind == kind)
            .filter(|(agg, _)| target == "all" || matches_aggregate(agg, target))
            .filter(|(_, row)| since.is_none_or(|s| row.version > s))
            .map(|(agg, row)| SnapshotDraft {
                aggregate: AggregateKey(agg.clone()),
                version: row.version,
                type_: kind.snapshot_type(),
                subject: row.subject.clone(),
                payload: row.payload.clone(),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::snapshot_event_id;

    fn source() -> IssueReindexSource {
        let mut s = IssueReindexSource::new();
        s.upsert(
            IssueReplayKind::Issue,
            "myelin://acme/issue/CORE-1",
            2,
            "myelin://acme/issue/CORE-1",
            serde_json::json!({ "state": "open", "title_ref": "ref" }),
        );
        s.upsert(
            IssueReplayKind::Issue,
            "myelin://acme/issue/CORE-2",
            5,
            "myelin://acme/issue/CORE-2",
            serde_json::json!({ "state": "closed" }),
        );
        s
    }

    #[test]
    fn issue_granular_replay() {
        let drafts = source().replay(&SnapshotScope::new("issue", "issue:CORE-1"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/issue/CORE-1");
        assert_eq!(drafts[0].type_.0, "issue.issue.snapshot");
    }

    #[test]
    fn cold_equals_live_idempotent() {
        let src = source();
        let scope = SnapshotScope::new("issue", "issue:all");
        let a = src.replay(&scope, None);
        let b = src.replay(&scope, None);
        assert_eq!(a, b);
        assert_eq!(
            snapshot_event_id(
                &myelin_events::TenantId("acme".into()),
                &a[0].aggregate,
                a[0].version,
            ),
            snapshot_event_id(
                &myelin_events::TenantId("acme".into()),
                &b[0].aggregate,
                b[0].version,
            )
        );
    }

    #[test]
    fn erased_issue_is_skipped() {
        let mut src = source();
        assert!(src.erase("myelin://acme/issue/CORE-1"));
        let drafts = src.replay(&SnapshotScope::new("issue", "issue:all"), None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/issue/CORE-2");
    }

    #[test]
    fn owner_token_is_issue() {
        assert_eq!(source().owner_token(), "issue");
    }
}
