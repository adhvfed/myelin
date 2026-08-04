use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiReplayKind {
    Run,
    Deployment,
    Pipeline,
}

impl CiReplayKind {
    fn snapshot_type(self) -> EventType {
        EventType(
            match self {
                CiReplayKind::Run => events::CI_RUN_SNAPSHOT,
                CiReplayKind::Deployment => events::CI_DEPLOYMENT_SNAPSHOT,
                CiReplayKind::Pipeline => events::CI_PIPELINE_SNAPSHOT,
            }
            .to_string(),
        )
    }

    fn from_selector(selector: &str) -> Option<CiReplayKind> {
        match selector.split(':').next() {
            Some("run") => Some(CiReplayKind::Run),
            Some("deployment") => Some(CiReplayKind::Deployment),
            Some("pipeline") => Some(CiReplayKind::Pipeline),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CiTruthRow {
    kind: CiReplayKind,
    version: u64,
    payload: serde_json::Value,
    subject: ArtifactRef,
}

#[derive(Debug, Default)]
pub struct CiReindexSource {
    truth: BTreeMap<String, CiTruthRow>,
}

impl CiReindexSource {
    pub fn new() -> CiReindexSource {
        CiReindexSource::default()
    }

    pub fn upsert(
        &mut self,
        kind: CiReplayKind,
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
            CiTruthRow {
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

impl ReindexSource for CiReindexSource {
    fn owner_token(&self) -> &str {
        "ci"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let kind = match CiReplayKind::from_selector(&scope.selector) {
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
    use myelin_events::{
        reindex, snapshot_event_id, Actor, EmitContextBase, OutboxStore, Region, TenantId,
        Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("ci".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn source_with_two_runs() -> CiReindexSource {
        let mut src = CiReindexSource::new();
        src.upsert(
            CiReplayKind::Run,
            "myelin://acme/ci/run/r1",
            1,
            "myelin://acme/ci/run/r1",
            serde_json::json!({ "overall": "success", "commit": "abc" }),
        );
        src.upsert(
            CiReplayKind::Run,
            "myelin://acme/ci/run/r2",
            3,
            "myelin://acme/ci/run/r2",
            serde_json::json!({ "overall": "failure", "commit": "def" }),
        );
        src
    }

    #[test]
    fn one_run_granular_replay() {
        let src = source_with_two_runs();
        let scope = SnapshotScope::new("ci", "run:r1");
        let drafts = src.replay(&scope, None);
        assert_eq!(drafts.len(), 1, "exactly the one run");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r1");
        assert_eq!(drafts[0].type_.0, "ci.run.snapshot");
    }

    #[test]
    fn cold_equals_live_idempotent_rerun() {
        let src = source_with_two_runs();
        let scope = SnapshotScope::new("ci", "run:all");

        let first = src.replay(&scope, None);
        let second = src.replay(&scope, None);
        assert_eq!(first, second, "the replay is deterministic (cold == live)");

        let id_a = snapshot_event_id(&first[0].aggregate, first[0].version);
        let id_b = snapshot_event_id(&second[0].aggregate, second[0].version);
        assert_eq!(id_a, id_b, "the snapshot event_id is deterministic");
    }

    #[test]
    fn erased_run_is_skipped() {
        let mut src = source_with_two_runs();
        assert!(src.erase("myelin://acme/ci/run/r1"));
        let drafts = src.replay(&SnapshotScope::new("ci", "run:all"), None);
        assert_eq!(drafts.len(), 1, "only the non-erased run replays");
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r2");
    }

    #[test]
    fn since_cursor_replays_only_newer() {
        let src = source_with_two_runs();
        let drafts = src.replay(&SnapshotScope::new("ci", "run:all"), Some(1));
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].aggregate.0, "myelin://acme/ci/run/r2");
    }

    #[test]
    fn bus_reindex_dispatches_to_ci_owner() {
        let src = source_with_two_runs();
        let sources: Vec<&dyn ReindexSource> = vec![&src];
        let mut outbox = OutboxStore::new();
        let receipt = reindex(
            &SnapshotScope::new("ci", "run:all"),
            None,
            &sources,
            &mut outbox,
            ctx_base(),
        )
        .expect("reindex dispatches to the ci owner");
        assert_eq!(receipt.owners_replayed, vec!["ci".to_string()]);
        assert_eq!(
            receipt.snapshots_emitted, 2,
            "both runs re-emitted through the outbox"
        );
    }
}
