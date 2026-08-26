use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::outbox::{EmitContextBase, IdMinter, OutboxStore, Ulid};
use crate::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    TenantId, Visibility,
};

pub const SNAPSHOT_EVENT_NAME: &str = "snapshot";

pub fn snapshot_event_id(tenant: &TenantId, aggregate: &AggregateKey, version: u64) -> EventId {
    let keyed = format!("{}:{}:{}@{version}", tenant.0.len(), tenant.0, aggregate.0);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in keyed.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    EventId(format!("snap-{hash:016x}"))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SnapshotScope {
    pub owner: String,
    pub selector: String,
}

impl SnapshotScope {
    pub fn new(owner: impl Into<String>, selector: impl Into<String>) -> SnapshotScope {
        SnapshotScope {
            owner: owner.into(),
            selector: selector.into(),
        }
    }

    pub fn as_key(&self) -> String {
        format!("{}:{}", self.owner, self.selector)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotDraft {
    pub aggregate: AggregateKey,
    pub version: u64,
    pub type_: EventType,
    pub subject: ArtifactRef,
    pub payload: serde_json::Value,
    pub data_role: DataRole,
    pub visibility: Visibility,
}

impl SnapshotDraft {
    pub fn event_id(&self, tenant: &TenantId) -> EventId {
        snapshot_event_id(tenant, &self.aggregate, self.version)
    }

    fn to_event_draft(&self) -> EventDraft {
        EventDraft {
            type_: self.type_.clone(),
            subject: self.subject.clone(),
            aggregate: self.aggregate.clone(),
            payload: self.payload.clone(),
            data_role: self.data_role,
            visibility: self.visibility,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
}

pub trait ReindexSource {
    fn owner_token(&self) -> &str;

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft>;
}

struct PresetMinter {
    ids: std::sync::Mutex<std::collections::VecDeque<Ulid>>,
}

impl PresetMinter {
    fn new(ids: impl IntoIterator<Item = Ulid>) -> PresetMinter {
        PresetMinter {
            ids: std::sync::Mutex::new(ids.into_iter().collect()),
        }
    }
}

impl IdMinter for PresetMinter {
    fn mint(&self) -> Ulid {
        self.ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
            .expect("reindex preset minter underflow - one id per snapshot draft")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ReindexReceipt {
    pub snapshots_emitted: usize,
    pub snapshots_skipped_duplicate: usize,
    pub owners_replayed: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    NoSourceForOwner(String),
    SubjectTenantMismatch {
        subject: String,
        expected_tenant: String,
    },
    OutboxFailed(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::NoSourceForOwner(o) => {
                write!(f, "reindex: no registered owner for scope owner `{o}`")
            }
            ReindexError::SubjectTenantMismatch {
                subject,
                expected_tenant,
            } => write!(
                f,
                "reindex: snapshot subject `{subject}` does not belong to tenant `{expected_tenant}`"
            ),
            ReindexError::OutboxFailed(e) => write!(f, "reindex: outbox emit failed: {e}"),
        }
    }
}

impl std::error::Error for ReindexError {}

pub fn reindex(
    scope: &SnapshotScope,
    since: Option<u64>,
    sources: &[&dyn ReindexSource],
    outbox: &mut OutboxStore,
    ctx_base: EmitContextBase,
) -> Result<ReindexReceipt, ReindexError> {
    let source = sources
        .iter()
        .find(|s| s.owner_token() == scope.owner)
        .ok_or_else(|| ReindexError::NoSourceForOwner(scope.owner.clone()))?;

    let drafts = source.replay(scope, since);
    let tenant = &ctx_base.tenant;

    for draft in &drafts {
        validate_subject_tenant(&draft.subject, tenant)?;
    }

    let mut to_emit: Vec<SnapshotDraft> = Vec::new();
    let mut skipped_duplicate = 0usize;
    for draft in drafts {
        let id = draft.event_id(tenant);
        if outbox
            .try_row(&id)
            .map_err(|error| ReindexError::OutboxFailed(error.0))?
            .is_some()
        {
            skipped_duplicate += 1;
        } else {
            to_emit.push(draft);
        }
    }

    let snapshots_emitted = to_emit.len();
    if !to_emit.is_empty() {
        let ids: Vec<Ulid> = to_emit
            .iter()
            .map(|draft| Ulid(draft.event_id(tenant).0))
            .collect();
        let minter: Arc<dyn IdMinter> = Arc::new(PresetMinter::new(ids));
        let mut tx = outbox.begin(minter, ctx_base);
        for draft in &to_emit {
            tx.emit(draft.to_event_draft(), None)
                .map_err(|e| ReindexError::OutboxFailed(e.0))?;
        }
        tx.stage_state_change(format!(
            "reindex owner={} scope={} emitted={snapshots_emitted}",
            scope.owner,
            scope.as_key()
        ));
        tx.commit().map_err(|e| ReindexError::OutboxFailed(e.0))?;
    }

    Ok(ReindexReceipt {
        snapshots_emitted,
        snapshots_skipped_duplicate: skipped_duplicate,
        owners_replayed: vec![scope.owner.clone()],
    })
}

fn validate_subject_tenant(subject: &ArtifactRef, expected: &TenantId) -> Result<(), ReindexError> {
    let Some(rest) = subject.0.strip_prefix("myelin://") else {
        return Ok(());
    };
    let actual = rest.split('/').next().unwrap_or_default();
    if actual == expected.0 {
        Ok(())
    } else {
        Err(ReindexError::SubjectTenantMismatch {
            subject: subject.0.clone(),
            expected_tenant: expected.0.clone(),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivedStore {
    projection: BTreeMap<String, (u64, serde_json::Value)>,
    applied: std::collections::BTreeSet<String>,
}

impl DerivedStore {
    pub fn new() -> DerivedStore {
        DerivedStore::default()
    }

    pub fn ingest(&mut self, env: &EventEnvelope) -> bool {
        if !self.applied.insert(env.event_id.0.clone()) {
            return false;
        }
        let version = env
            .payload
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let agg = env.aggregate.0.clone();
        match self.projection.get(&agg) {
            Some((existing_v, _)) if *existing_v >= version => false,
            _ => {
                self.projection.insert(agg, (version, env.payload.clone()));
                true
            }
        }
    }

    pub fn parity_bytes(&self) -> Vec<u8> {
        let view: BTreeMap<&String, &serde_json::Value> =
            self.projection.iter().map(|(k, (_, v))| (k, v)).collect();
        serde_json::to_vec(&view).expect("projection serializes")
    }

    pub fn len(&self) -> usize {
        self.projection.len()
    }

    pub fn is_empty(&self) -> bool {
        self.projection.is_empty()
    }
}

pub struct ReferenceReindexSource {
    tenant: TenantId,
    owner: String,
    artifact: String,
    truth: BTreeMap<String, (u64, serde_json::Value)>,
}

impl ReferenceReindexSource {
    pub fn new(
        tenant: TenantId,
        owner: impl Into<String>,
        artifact: impl Into<String>,
    ) -> ReferenceReindexSource {
        ReferenceReindexSource {
            tenant,
            owner: owner.into(),
            artifact: artifact.into(),
            truth: BTreeMap::new(),
        }
    }

    pub fn upsert(&mut self, aggregate: &str, version: u64, payload: serde_json::Value) {
        let mut payload = payload;
        if let serde_json::Value::Object(map) = &mut payload {
            map.insert("version".into(), serde_json::json!(version));
        }
        self.truth.insert(aggregate.to_string(), (version, payload));
    }

    pub fn snapshot_type(&self) -> EventType {
        EventType(format!(
            "{}.{}.{}",
            self.owner, self.artifact, SNAPSHOT_EVENT_NAME
        ))
    }
}

impl ReindexSource for ReferenceReindexSource {
    fn owner_token(&self) -> &str {
        &self.owner
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.truth
            .iter()
            .filter(|(_, (v, _))| since.is_none_or(|s| *v > s))
            .map(|(agg, (v, payload))| SnapshotDraft {
                aggregate: AggregateKey(agg.clone()),
                version: *v,
                type_: self.snapshot_type(),
                subject: ArtifactRef(format!(
                    "myelin://{}/{}/{}/{agg}",
                    self.tenant.0, self.owner, self.artifact
                )),
                payload: payload.clone(),
                data_role: DataRole::Processor,
                visibility: Visibility::Internal,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, CorrelationId, Region, TenantId, Timestamp};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn ctx_base() -> EmitContextBase {
        ctx_base_for(tenant())
    }

    fn ctx_base_for(tenant: TenantId) -> EmitContextBase {
        EmitContextBase {
            tenant: tenant.clone(),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant,
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            caused_by: None,
        }
    }

    fn envelope_of(row: &crate::OutboxRow) -> EventEnvelope {
        row.envelope.clone()
    }

    #[test]
    fn snapshot_event_id_is_deterministic_from_tenant_aggregate_and_version() {
        let a = AggregateKey("ci.run:42".into());
        let b = AggregateKey("ci.run:43".into());
        assert_eq!(
            snapshot_event_id(&tenant(), &a, 1),
            snapshot_event_id(&tenant(), &a, 1),
            "same inputs → same id"
        );
        assert_ne!(
            snapshot_event_id(&tenant(), &a, 1),
            snapshot_event_id(&tenant(), &a, 2),
            "version bumps the id"
        );
        assert_ne!(
            snapshot_event_id(&tenant(), &a, 1),
            snapshot_event_id(&tenant(), &b, 1),
            "aggregate bumps the id"
        );
        assert_ne!(
            snapshot_event_id(&tenant(), &a, 1),
            snapshot_event_id(&TenantId("other".into()), &a, 1),
            "tenant bumps the id so shared outboxes cannot deduplicate across tenants"
        );
        assert!(
            snapshot_event_id(&tenant(), &a, 1).0.starts_with("snap-"),
            "snapshot ids are prefixed"
        );
    }

    #[test]
    fn reindex_is_idempotent_a_rerun_emits_zero_new_snapshots() {
        let mut source = ReferenceReindexSource::new(tenant(), "ci", "run");
        source.upsert("ci.run:1", 1, serde_json::json!({ "status": "success" }));
        source.upsert("ci.run:2", 1, serde_json::json!({ "status": "failure" }));
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("ci", "run:all");

        let mut outbox = OutboxStore::new();
        let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("first reindex");
        assert_eq!(r1.snapshots_emitted, 2, "first run emits both snapshots");
        assert_eq!(r1.snapshots_skipped_duplicate, 0);
        assert_eq!(outbox.committed_count(), 2);

        let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("second reindex");
        assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 NEW (idempotent)");
        assert_eq!(r2.snapshots_skipped_duplicate, 2);
        assert_eq!(
            outbox.committed_count(),
            2,
            "still only 2 rows - no duplicate effect"
        );
    }

    #[test]
    fn reindex_is_isolated_by_tenant_in_a_shared_outbox() {
        let globex = TenantId("globex".into());
        let mut acme_source = ReferenceReindexSource::new(tenant(), "ci", "run");
        acme_source.upsert("ci.run:1", 1, serde_json::json!({ "status": "success" }));
        let mut globex_source = ReferenceReindexSource::new(globex.clone(), "ci", "run");
        globex_source.upsert("ci.run:1", 1, serde_json::json!({ "status": "failure" }));
        let scope = SnapshotScope::new("ci", "run:all");
        let mut outbox = OutboxStore::new();

        let acme =
            reindex(&scope, None, &[&acme_source], &mut outbox, ctx_base()).expect("acme rebuild");
        let other = reindex(
            &scope,
            None,
            &[&globex_source],
            &mut outbox,
            ctx_base_for(globex.clone()),
        )
        .expect("globex rebuild");

        assert_eq!(acme.snapshots_emitted, 1);
        assert_eq!(other.snapshots_emitted, 1);
        assert_eq!(
            outbox.committed_count(),
            2,
            "neither tenant masks the other"
        );
        assert_ne!(
            snapshot_event_id(&tenant(), &AggregateKey("ci.run:1".into()), 1),
            snapshot_event_id(&globex, &AggregateKey("ci.run:1".into()), 1),
            "the same aggregate version has a distinct identity per tenant"
        );
    }

    #[test]
    fn reindex_rejects_a_cross_tenant_subject_before_staging_anything() {
        let mut source =
            ReferenceReindexSource::new(TenantId("globex".into()), "knowledge", "page");
        source.upsert("home", 1, serde_json::json!({ "title": "private" }));
        let mut outbox = OutboxStore::new();

        let error = reindex(
            &SnapshotScope::new("knowledge", "page:all"),
            None,
            &[&source],
            &mut outbox,
            ctx_base(),
        )
        .expect_err("a globex subject cannot enter acme's rebuild");

        assert!(matches!(
            error,
            ReindexError::SubjectTenantMismatch {
                expected_tenant,
                ..
            } if expected_tenant == "acme"
        ));
        assert_eq!(outbox.committed_count(), 0, "validation is atomic");
    }

    #[test]
    fn emitted_snapshots_carry_the_deterministic_event_id() {
        let mut source = ReferenceReindexSource::new(tenant(), "knowledge", "page");
        source.upsert(
            "knowledge.page:home",
            3,
            serde_json::json!({ "title_ref": "r1" }),
        );
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("knowledge", "page:home");

        let mut outbox = OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");

        let expected = snapshot_event_id(&tenant(), &AggregateKey("knowledge.page:home".into()), 3);
        let row = outbox
            .row(&expected)
            .expect("snapshot lands at its deterministic id");
        assert_eq!(row.envelope.type_.0, "knowledge.page.snapshot");
        assert_eq!(
            row.envelope.depth, 0,
            "a snapshot is a ROOT re-emit (depth resets)"
        );
    }

    #[test]
    fn reference_consumer_rebuilds_byte_identically_cold_equals_live() {
        let mut source = ReferenceReindexSource::new(tenant(), "ci", "run");
        source.upsert("ci.run:1", 1, serde_json::json!({ "status": "success" }));
        source.upsert("ci.run:2", 2, serde_json::json!({ "status": "failure" }));
        source.upsert("ci.run:3", 1, serde_json::json!({ "status": "running" }));

        let mut live = DerivedStore::new();
        let scope = SnapshotScope::new("ci", "run:all");
        for draft in source.replay(&scope, None) {
            let env = snapshot_envelope(&draft);
            live.ingest(&env);
        }

        let mut cold = DerivedStore::new();
        let sources: &[&dyn ReindexSource] = &[&source];
        let mut outbox = OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        for draft in source.replay(&scope, None) {
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row present");
            cold.ingest(&envelope_of(&row));
        }

        assert_eq!(live.len(), 3);
        assert_eq!(cold.len(), 3);
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "cold == live (byte-identical)"
        );
    }

    #[test]
    fn reindex_of_unknown_owner_is_a_loud_error() {
        let source = ReferenceReindexSource::new(tenant(), "ci", "run");
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("refs", "edge:all");
        let mut outbox = OutboxStore::new();
        let err = reindex(&scope, None, sources, &mut outbox, ctx_base()).unwrap_err();
        assert_eq!(err, ReindexError::NoSourceForOwner("refs".into()));
    }

    #[test]
    fn reindex_since_cursor_replays_only_newer_versions() {
        let mut source = ReferenceReindexSource::new(tenant(), "ci", "run");
        source.upsert("ci.run:1", 1, serde_json::json!({ "status": "old" }));
        source.upsert("ci.run:2", 5, serde_json::json!({ "status": "new" }));
        let sources: &[&dyn ReindexSource] = &[&source];
        let scope = SnapshotScope::new("ci", "run:all");

        let mut outbox = OutboxStore::new();
        let r = reindex(&scope, Some(3), sources, &mut outbox, ctx_base()).expect("incremental");
        assert_eq!(
            r.snapshots_emitted, 1,
            "only the version-5 aggregate replays past since=3"
        );
    }

    #[test]
    fn derived_store_ingest_is_idempotent_and_lww() {
        let mut store = DerivedStore::new();
        let draft_v2 = SnapshotDraft {
            aggregate: AggregateKey("a:1".into()),
            version: 2,
            type_: EventType("x.a.snapshot".into()),
            subject: ArtifactRef("myelin://acme/x/a/1".into()),
            payload: serde_json::json!({ "version": 2, "v": "new" }),
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
        };
        let env_v2 = snapshot_envelope(&draft_v2);
        assert!(store.ingest(&env_v2), "first ingest applies");
        assert!(
            !store.ingest(&env_v2),
            "redelivery is a no-op (idempotent on event_id)"
        );

        let draft_v1 = SnapshotDraft {
            version: 1,
            payload: serde_json::json!({ "version": 1, "v": "old" }),
            ..draft_v2.clone()
        };
        let env_v1 = snapshot_envelope(&draft_v1);
        assert!(
            !store.ingest(&env_v1),
            "an older-version snapshot is LWW-rejected (no resurrection)"
        );
        let bytes = store.parity_bytes();
        assert!(
            String::from_utf8_lossy(&bytes).contains("new"),
            "the newer version wins (LWW)"
        );
    }

    fn snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
        let event_id = draft.event_id(&tenant());
        EventEnvelope {
            event_id: event_id.clone(),
            type_: draft.type_.clone(),
            schema_ver: 1,
            tenant: tenant(),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("platform".into()),
                PrincipalKind::Service,
                tenant(),
            )),
            subject: draft.subject.clone(),
            aggregate: draft.aggregate.clone(),
            causation_id: None,
            correlation_id: CorrelationId(event_id.0),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: draft.data_role,
            visibility: draft.visibility,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload: draft.payload.clone(),
        }
    }
}
