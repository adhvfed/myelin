use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitReplayKind {
    Repo,
    Blob,
    Pr,
}

impl GitReplayKind {
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                GitReplayKind::Repo => events::GIT_REPO_SNAPSHOT,
                GitReplayKind::Blob => events::GIT_BLOB_SNAPSHOT,
                GitReplayKind::Pr => events::GIT_PR_SNAPSHOT,
            }
            .to_string(),
        )
    }

    fn from_selector(selector: &str) -> Option<GitReplayKind> {
        match selector.split(':').next() {
            Some("repo") => Some(GitReplayKind::Repo),
            Some("blob") => Some(GitReplayKind::Blob),
            Some("pr") => Some(GitReplayKind::Pr),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitTruthRow {
    kind: GitReplayKind,
    version: u64,
    payload: serde_json::Value,
    subject: ArtifactRef,
}

#[derive(Debug, Default)]
pub struct GitReindexSource {
    truth: BTreeMap<String, GitTruthRow>,
}

impl GitReindexSource {
    pub fn new() -> GitReindexSource {
        GitReindexSource::default()
    }

    pub fn upsert(
        &mut self,
        kind: GitReplayKind,
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
            GitTruthRow {
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

impl ReindexSource for GitReindexSource {
    fn owner_token(&self) -> &str {
        "git"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let kind = match GitReplayKind::from_selector(&scope.selector) {
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
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        reindex, snapshot_event_id, Actor, DerivedStore, EmitContextBase, OutboxStore, Region,
        TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn source() -> GitReindexSource {
        let mut s = GitReindexSource::new();
        s.upsert(
            GitReplayKind::Repo,
            "myelin://acme/git/repo/core",
            3,
            "myelin://acme/git/repo/core",
            serde_json::json!({ "default_branch": "main", "visibility": "internal" }),
        );
        s.upsert(
            GitReplayKind::Repo,
            "myelin://acme/git/repo/docs",
            1,
            "myelin://acme/git/repo/docs",
            serde_json::json!({ "default_branch": "main", "visibility": "private" }),
        );
        s.upsert(
            GitReplayKind::Blob,
            "myelin://acme/git/repo/core#blob-abc",
            2,
            "myelin://acme/git/repo/core#blob-abc",
            serde_json::json!({ "path": "src/main.rs", "language": "rust" }),
        );
        s
    }

    #[test]
    fn replay_repo_scope_emits_git_repo_snapshots() {
        let s = source();
        let scope = SnapshotScope::new("git", "repo:all");
        let drafts = s.replay(&scope, None);
        assert_eq!(drafts.len(), 2, "two repos, not the blob");
        for d in &drafts {
            assert_eq!(d.type_.0, "git.repo.snapshot");
        }
    }

    #[test]
    fn replay_specific_blob_scope_is_sub_artifact_granular() {
        let s = source();
        let scope = SnapshotScope::new("git", "blob:myelin://acme/git/repo/core#blob-abc");
        let drafts = s.replay(&scope, None);
        assert_eq!(drafts.len(), 1, "exactly the one blob");
        assert_eq!(drafts[0].type_.0, "git.blob.snapshot");
        assert_eq!(drafts[0].version, 2);
    }

    #[test]
    fn replay_skips_an_erased_aggregate() {
        let mut s = source();
        assert!(s.erase("myelin://acme/git/repo/docs"));
        let drafts = s.replay(&SnapshotScope::new("git", "repo:all"), None);
        assert_eq!(drafts.len(), 1, "the erased repo is not re-snapshotted");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    #[test]
    fn replay_pr_selector_arm_resolves() {
        let mut s = source();
        s.upsert(
            GitReplayKind::Pr,
            "myelin://acme/git/repo/core#pr-1",
            7,
            "myelin://acme/git/repo/core#pr-1",
            serde_json::json!({ "title": "ref" }),
        );
        let drafts = s.replay(&SnapshotScope::new("git", "pr:all"), None);
        assert_eq!(
            drafts.len(),
            1,
            "the pr arm resolved (else the selector is unparseable → empty)"
        );
        assert_eq!(drafts[0].type_.0, "git.pr.snapshot");
    }

    #[test]
    fn replay_specific_aggregate_matches_exactly_one_not_all() {
        let s = source();
        let scope = SnapshotScope::new("git", "repo:myelin://acme/git/repo/core");
        let drafts = s.replay(&scope, None);
        assert_eq!(
            drafts.len(),
            1,
            "exactly the named repo (not all repos, not none)"
        );
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    #[test]
    fn replay_suffix_selector_matches_via_ends_with() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("git", "repo:core"), None);
        assert_eq!(
            drafts.len(),
            1,
            "the `core` suffix selector matched the full aggregate"
        );
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    #[test]
    fn replay_suffix_selector_is_segment_anchored_not_substring() {
        let mut s = source();
        s.upsert(
            GitReplayKind::Repo,
            "myelin://acme/git/repo/mycore",
            1,
            "myelin://acme/git/repo/mycore",
            serde_json::json!({ "default_branch": "main" }),
        );
        let drafts = s.replay(&SnapshotScope::new("git", "repo:core"), None);
        assert_eq!(
            drafts.len(),
            1,
            "`core` matches ONLY the segment `…/repo/core`, never `…/mycore`"
        );
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/git/repo/core");
    }

    #[test]
    fn matches_aggregate_exact_anchored_and_substring_reject() {
        assert!(
            matches_aggregate("myelin://acme/git/repo/core", "myelin://acme/git/repo/core"),
            "exact"
        );
        assert!(
            matches_aggregate("myelin://acme/git/repo/core", "core"),
            "segment-anchored `/core`"
        );
        assert!(
            matches_aggregate("myelin://acme/git/repo/core#blob-1", "blob-1"),
            "anchored at `#`"
        );
        assert!(
            !matches_aggregate("myelin://acme/git/repo/mycore", "core"),
            "substring is NOT a match"
        );
        assert!(
            !matches_aggregate("myelin://acme/git/repo/core", "other"),
            "a non-suffix is no match"
        );
    }

    #[test]
    fn replay_since_cursor_is_strictly_above() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("git", "repo:all"), Some(2));
        assert_eq!(
            drafts.len(),
            1,
            "only the version-3 repo replays past since=2"
        );
        assert_eq!(drafts[0].version, 3);
        assert!(
            s.replay(&SnapshotScope::new("git", "repo:all"), Some(3))
                .is_empty(),
            "since == the high-water version re-emits nothing (the cursor row is not re-applied)"
        );
        assert_eq!(
            s.replay(&SnapshotScope::new("git", "repo:all"), Some(0))
                .len(),
            2,
            "since=0 replays every repo (full rebuild)"
        );
    }

    #[test]
    fn git_replay_rebuilds_byte_identically_and_is_idempotent() {
        let s = source();
        let scope = SnapshotScope::new("git", "repo:all");

        let mut live = DerivedStore::new();
        for draft in s.replay(&scope, None) {
            live.ingest(&snapshot_envelope(&draft));
        }

        let mut cold = DerivedStore::new();
        let sources: &[&dyn ReindexSource] = &[&s];
        let mut outbox = OutboxStore::new();
        let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        assert_eq!(r1.snapshots_emitted, 2);
        for draft in s.replay(&scope, None) {
            let row = outbox.row(&draft.event_id()).expect("snapshot row present");
            cold.ingest(&row.envelope);
        }
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "cold == live (byte-identical)"
        );

        let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 new (idempotent)");
        assert_eq!(r2.snapshots_skipped_duplicate, 2);
    }

    fn snapshot_envelope(draft: &SnapshotDraft) -> myelin_events::EventEnvelope {
        use myelin_events::CorrelationId;
        myelin_events::EventEnvelope {
            event_id: draft.event_id(),
            type_: draft.type_.clone(),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: draft.subject.clone(),
            aggregate: draft.aggregate.clone(),
            causation_id: None,
            correlation_id: CorrelationId(draft.event_id().0),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: draft.data_role,
            visibility: draft.visibility,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            payload: draft.payload.clone(),
        }
    }

    #[test]
    fn git_snapshot_id_is_deterministic() {
        let a = AggregateKey("myelin://acme/git/repo/core".into());
        assert_eq!(snapshot_event_id(&a, 3), snapshot_event_id(&a, 3));
        assert_ne!(snapshot_event_id(&a, 3), snapshot_event_id(&a, 4));
    }
}
