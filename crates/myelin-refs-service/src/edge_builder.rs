use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_refs::{strip_sub, ArtifactRef};
use myelin_tenancy::{Region, TenantId};

use crate::mirror::{mirror_edges, LifecycleRel, SyntheticTypedEvent};

pub static EDGE_BUILDER_SUBJECTS: &[SubjectPattern] = &[];

pub const EDGE_BUILDER_CONSUMER: &str = "refs-edge-builder";

pub const EDGE_BUILDER_SUBJECT_PREFIXES: &[&str] =
    &["refs.edge.", "issue.relation.", "knowledge.page."];

const EDGE_ID_DOMAIN: &[u8] = b"myelin.refs.edge.v2";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelClass {
    Reference,
    Lifecycle,
}

impl RelClass {
    pub fn as_str(self) -> &'static str {
        match self {
            RelClass::Reference => "reference",
            RelClass::Lifecycle => "lifecycle",
        }
    }
}

pub fn edge_id(tenant: &TenantId, source: &str, target: &str, rel: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(EDGE_ID_DOMAIN);
    for field in [
        tenant.0.as_bytes(),
        source.as_bytes(),
        target.as_bytes(),
        rel.as_bytes(),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeRow {
    pub edge_id: String,
    pub source: ArtifactRef,
    pub source_root: ArtifactRef,
    pub target: ArtifactRef,
    pub target_root: ArtifactRef,
    pub rel: String,
    pub rel_class: RelClass,
    pub origin_event: String,
    pub origin_actor: String,
    pub zookie: Option<String>,
    pub tombstoned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeMutation {
    Apply(Vec<EdgeRow>),
    TombstoneIds(Vec<String>),
    Ignore,
}

/// Interpret an event once, independently of the projection backing.
///
/// The memory projection, PostgreSQL consumer, and reindex path all use this function so a live
/// event and a replay cannot acquire different edge semantics.
pub fn edge_mutation(ev: &EventEnvelope) -> Result<EdgeMutation, ProjectError> {
    let event_name = ev.type_.0.rsplit('.').next().unwrap_or("");
    match event_name {
        "erased" | "removed" => removed_edge_mutation(ev),
        "created" | "set" | "parent_set" | "updated" | "snapshot" => created_edge_mutation(ev),
        _ => Ok(EdgeMutation::Ignore),
    }
}

fn created_edge_mutation(ev: &EventEnvelope) -> Result<EdgeMutation, ProjectError> {
    let payload = &ev.payload;
    let has_edge_payload = payload.get("source").is_some() || payload.get("target").is_some();
    let is_edge_subject = ev.type_.0.starts_with("refs.edge.");
    if !has_edge_payload {
        return if is_edge_subject {
            Err(ProjectError(format!(
                "{} carries no edge payload (source/target/rel)",
                ev.type_.0
            )))
        } else {
            Ok(EdgeMutation::Ignore)
        };
    }

    Ok(EdgeMutation::Apply(edge_rows(ev, false)?))
}

fn removed_edge_mutation(ev: &EventEnvelope) -> Result<EdgeMutation, ProjectError> {
    if let Some(edge_id) = str_field(&ev.payload, "edge_id") {
        return Ok(EdgeMutation::TombstoneIds(vec![edge_id]));
    }
    Ok(EdgeMutation::Apply(edge_rows(ev, true)?))
}

fn edge_rows(ev: &EventEnvelope, tombstoned: bool) -> Result<Vec<EdgeRow>, ProjectError> {
    let field = |name| {
        if tombstoned {
            required_removal_field(ev, name)
        } else {
            required_field(ev, name)
        }
    };
    let source = field("source")?;
    let target = field("target")?;
    let rel = field("rel")?;
    let source_ref = ArtifactRef(source.clone());
    let target_ref = ArtifactRef(target.clone());
    let rel_class = rel_class(ev)?;
    let origin_actor =
        str_field(&ev.payload, "origin_actor").unwrap_or_else(|| ev.actor.0.principal_id.0.clone());
    let zookie = str_field(&ev.payload, "zookie");
    if rel_class == RelClass::Lifecycle {
        let rel = LifecycleRel::parse(&rel)
            .ok_or_else(|| ProjectError(format!("unknown lifecycle relation `{rel}`")))?;
        let mut rows = mirror_edges(
            &ev.tenant,
            &SyntheticTypedEvent {
                source: source_ref,
                target: target_ref,
                rel,
                origin_event: ev.event_id.0.clone(),
                origin_actor,
                zookie,
            },
        );
        for row in &mut rows {
            row.tombstoned = tombstoned;
        }
        return Ok(rows);
    }
    let rel = reference_rel(&rel)?;
    let row = EdgeRow {
        edge_id: edge_id(&ev.tenant, &source, &target, &rel),
        source_root: strip_sub(&source_ref),
        target_root: strip_sub(&target_ref),
        source: source_ref,
        target: target_ref,
        rel,
        rel_class,
        origin_event: ev.event_id.0.clone(),
        origin_actor,
        zookie,
        tombstoned,
    };
    Ok(vec![row])
}

fn reference_rel(rel: &str) -> Result<String, ProjectError> {
    match rel {
        "mentions" | "links" | "embeds" => Ok(rel.to_string()),
        "references" => Ok("links".into()),
        other => Err(ProjectError(format!(
            "unknown reference relation `{other}`"
        ))),
    }
}

fn rel_class(ev: &EventEnvelope) -> Result<RelClass, ProjectError> {
    match ev.payload.get("rel_class").and_then(|value| value.as_str()) {
        Some("reference") => Ok(RelClass::Reference),
        Some("lifecycle") => Ok(RelClass::Lifecycle),
        Some(other) => Err(ProjectError(format!(
            "{} carries unknown edge relation class `{other}`",
            ev.type_.0
        ))),
        None if is_legacy_lifecycle_event(&ev.type_.0) => Ok(RelClass::Lifecycle),
        None => Ok(RelClass::Reference),
    }
}

fn is_legacy_lifecycle_event(event_type: &str) -> bool {
    event_type.starts_with("issue.relation.")
        || event_type.starts_with("knowledge.relation.")
        || event_type == "knowledge.page.parent_set"
}

fn required_field(ev: &EventEnvelope, field: &str) -> Result<String, ProjectError> {
    str_field(&ev.payload, field)
        .ok_or_else(|| ProjectError(format!("{} edge payload is missing `{field}`", ev.type_.0)))
}

fn required_removal_field(ev: &EventEnvelope, field: &str) -> Result<String, ProjectError> {
    str_field(&ev.payload, field).ok_or_else(|| {
        ProjectError(format!(
            "{} removal is missing `edge_id`/`{field}`",
            ev.type_.0
        ))
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartKey {
    tenant: TenantId,
    region: Region,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EventOrder {
    recorded_at: String,
    event_id: String,
}

#[derive(Default)]
struct EdgePartition {
    rows: HashMap<String, EdgeRow>,
    event_orders: HashMap<String, EventOrder>,
}

#[derive(Clone, Default)]
pub struct EdgeProjection {
    inner: Arc<Mutex<HashMap<PartKey, EdgePartition>>>,
}

impl EdgeProjection {
    pub fn new() -> EdgeProjection {
        EdgeProjection::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PartKey, EdgePartition>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn upsert(&self, tenant: &TenantId, region: &Region, row: EdgeRow) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut inner = self.lock();
        let part = inner.entry(pk).or_default();
        part.event_orders.remove(&row.edge_id);
        part.rows.insert(row.edge_id.clone(), row);
    }

    fn apply_event(&self, ev: &EventEnvelope, row: EdgeRow) {
        let pk = PartKey {
            tenant: ev.tenant.clone(),
            region: ev.region.clone(),
        };
        let order = event_order(ev);
        let mut inner = self.lock();
        let part = inner.entry(pk).or_default();
        if part
            .event_orders
            .get(&row.edge_id)
            .is_some_and(|applied| order <= *applied)
        {
            return;
        }
        part.event_orders.insert(row.edge_id.clone(), order);
        part.rows.insert(row.edge_id.clone(), row);
    }

    fn tombstone_event(&self, ev: &EventEnvelope, edge_id: &str) {
        let pk = PartKey {
            tenant: ev.tenant.clone(),
            region: ev.region.clone(),
        };
        let order = event_order(ev);
        let mut inner = self.lock();
        let Some(part) = inner.get_mut(&pk) else {
            return;
        };
        if part
            .event_orders
            .get(edge_id)
            .is_some_and(|applied| order <= *applied)
        {
            return;
        }
        let Some(row) = part.rows.get_mut(edge_id) else {
            return;
        };
        row.tombstoned = true;
        row.origin_event = ev.event_id.0.clone();
        part.event_orders.insert(edge_id.to_string(), order);
    }

    pub fn tombstone(&self, tenant: &TenantId, region: &Region, edge_id: &str, origin_event: &str) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut inner = self.lock();
        if let Some(part) = inner.get_mut(&pk) {
            if let Some(row) = part.rows.get_mut(edge_id) {
                row.tombstoned = true;
                row.origin_event = origin_event.to_string();
                part.event_orders.remove(edge_id);
            }
        }
    }

    pub fn get(&self, tenant: &TenantId, region: &Region, edge_id: &str) -> Option<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock()
            .get(&pk)
            .and_then(|p| p.rows.get(edge_id).cloned())
    }

    pub fn live_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock()
            .get(&pk)
            .map(|p| p.rows.values().filter(|r| !r.tombstoned).count())
            .unwrap_or(0)
    }

    pub fn total_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().get(&pk).map(|p| p.rows.len()).unwrap_or(0)
    }

    pub fn parity_bytes(&self, tenant: &TenantId, region: &Region) -> Vec<u8> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| p.rows.values().cloned().collect())
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        let mut out = Vec::new();
        for r in &rows {
            let rec = format!(
                "{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
                r.edge_id,
                r.source.0,
                r.source_root.0,
                r.target.0,
                r.target_root.0,
                r.rel,
                r.rel_class.as_str(),
                r.origin_actor,
                r.zookie.as_deref().unwrap_or(""),
                r.tombstoned,
            );
            out.extend_from_slice(rec.as_bytes());
            out.push(b'\n');
        }
        out
    }

    pub fn parity_hash(&self, tenant: &TenantId, region: &Region) -> String {
        let bytes = self.parity_bytes(tenant, region);
        format!("blake3:{}", blake3::hash(&bytes).to_hex())
    }

    pub fn wipe_partition(&self, tenant: &TenantId, region: &Region) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().remove(&pk);
    }

    pub fn edges_by_actor(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject_id: &str,
    ) -> Vec<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| {
                p.rows
                    .values()
                    .filter(|r| r.origin_actor == subject_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
    }

    pub fn count_by_actor(&self, tenant: &TenantId, region: &Region, subject_id: &str) -> usize {
        self.edges_by_actor(tenant, region, subject_id).len()
    }

    pub fn inbound_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> Vec<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| {
                p.rows
                    .values()
                    .filter(|r| !r.tombstoned && &r.target_root == target_root)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
    }

    pub fn outbound_live(
        &self,
        tenant: &TenantId,
        region: &Region,
        source_root: &ArtifactRef,
    ) -> Vec<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| {
                p.rows
                    .values()
                    .filter(|r| !r.tombstoned && &r.source_root == source_root)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
    }
}

fn event_order(ev: &EventEnvelope) -> EventOrder {
    EventOrder {
        recorded_at: ev.recorded_at.0.clone(),
        event_id: ev.event_id.0.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectError(pub String);

#[derive(Clone)]
pub struct RefsEdgeBuilder {
    projection: EdgeProjection,
    index_lag: Arc<AtomicU64>,
}

impl RefsEdgeBuilder {
    pub const INDEX_LAG_SIGNAL: &'static str = "refs.index_lag";

    pub fn new(projection: EdgeProjection) -> RefsEdgeBuilder {
        RefsEdgeBuilder {
            projection,
            index_lag: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn projection(&self) -> &EdgeProjection {
        &self.projection
    }

    pub fn index_lag(&self) -> u64 {
        self.index_lag.load(Ordering::SeqCst)
    }

    pub fn project(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        self.index_lag.fetch_add(1, Ordering::SeqCst);
        let result = self.project_inner(ev);
        self.index_lag.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn project_inner(&self, ev: &EventEnvelope) -> Result<(), ProjectError> {
        match edge_mutation(ev)? {
            EdgeMutation::Apply(rows) => {
                for row in rows {
                    self.projection.apply_event(ev, row);
                }
            }
            EdgeMutation::TombstoneIds(edge_ids) => {
                for edge_id in edge_ids {
                    self.projection.tombstone_event(ev, &edge_id);
                }
            }
            EdgeMutation::Ignore => {}
        }
        Ok(())
    }
}

fn str_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

impl EventHandler for RefsEdgeBuilder {
    fn subjects(&self) -> &'static [SubjectPattern] {
        EDGE_BUILDER_SUBJECTS
    }

    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        match self.project(ev) {
            Ok(()) => HandleOutcome::Done,
            Err(ProjectError(reason)) => HandleOutcome::NonRetryable(Reason(reason)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    fn edge_event(id: &str, type_: &str, source: &str, target: &str, rel: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            subject: ArtifactRef(source.into()),
            aggregate: AggregateKey(format!("edge:{source}->{target}")),
            causation_id: None,
            correlation_id: CorrelationId(id.into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
            payload: serde_json::json!({ "source": source, "target": target, "rel": rel, "zookie": "zk-1" }),
        }
    }

    #[test]
    fn edge_id_is_deterministic_and_field_unambiguous() {
        let t = tenant();
        let a = edge_id(&t, "s", "t", "mentions");
        assert_eq!(
            a,
            edge_id(&t, "s", "t", "mentions"),
            "the same tuple → the same id (idempotent)"
        );
        assert_ne!(
            a,
            edge_id(&t, "s", "t", "embeds"),
            "a different rel → a different id"
        );
        assert_ne!(
            a,
            edge_id(&t, "s2", "t", "mentions"),
            "a different source → a different id"
        );
        assert_ne!(
            a,
            edge_id(&TenantId("other".into()), "s", "t", "mentions"),
            "tenant-scoped id"
        );
        assert_ne!(
            edge_id(&t, "ab", "c", "mentions"),
            edge_id(&t, "a", "bc", "mentions"),
            "length-prefixed field boundaries are unambiguous"
        );
        assert!(
            a.starts_with("blake3:"),
            "the durable identity names its digest"
        );
        assert_eq!(a.len(), 71, "the full 256-bit digest is retained");
    }

    #[test]
    fn created_upserts_one_row_and_derives_roots() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/chat/message/m1#block-9";
        let tgt = "myelin://acme/knowledge/page/7c2#block-3";
        let ev = edge_event("01J-1", "refs.edge.created", src, tgt, "embeds");

        assert_eq!(
            b.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            b.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            1,
            "idempotent: one row"
        );

        let id = edge_id(&tenant(), src, tgt, "embeds");
        let row = b
            .projection()
            .get(&tenant(), &region(), &id)
            .expect("the edge row exists");
        assert_eq!(
            row.source_root.0, "myelin://acme/chat/message/m1",
            "source_root strips #sub"
        );
        assert_eq!(
            row.target_root.0, "myelin://acme/knowledge/page/7c2",
            "target_root strips #sub"
        );
        assert_eq!(row.source.0, src);
        assert_eq!(row.target.0, tgt);
        assert_eq!(row.rel, "embeds");
        assert_eq!(
            row.rel_class,
            RelClass::Reference,
            "refs.edge.* is reference-class"
        );
        assert_eq!(row.origin_actor, "p-opaque-1");
        assert_eq!(row.zookie.as_deref(), Some("zk-1"));
        assert!(!row.tombstoned);
    }

    #[test]
    fn typed_lifecycle_event_projects_a_lifecycle_class_edge() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/issue/issue/ENG-1";
        let tgt = "myelin://acme/issue/issue/ENG-2";
        let ev = edge_event("01J-rel", "issue.relation.created", src, tgt, "blocks");
        assert_eq!(
            b.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        let id = edge_id(&tenant(), src, tgt, "blocks");
        let row = b
            .projection()
            .get(&tenant(), &region(), &id)
            .expect("lifecycle edge exists");
        assert_eq!(
            row.rel_class,
            RelClass::Lifecycle,
            "issue.relation.* is lifecycle-class (TE-7)"
        );
    }

    #[test]
    fn lifecycle_event_with_no_edge_payload_is_a_noop_not_poison() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let mut ev = edge_event("01J-pg", "knowledge.page.created", "x", "y", "z");
        ev.payload = serde_json::json!({ "title_ref": "r1" });
        assert_eq!(
            b.handle(&ev, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "no edge payload → no-op, not poison"
        );
        assert_eq!(
            b.projection().total_count(&tenant(), &region()),
            0,
            "no edge projected"
        );
    }

    #[test]
    fn malformed_edge_event_is_a_nonretryable_poison() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let mut ev = edge_event("01J-bad", "refs.edge.created", "s", "t", "mentions");
        ev.payload = serde_json::json!({ "target": "t", "rel": "mentions" });
        match b.handle(&ev, &mut myelin_events::HandlerTx::none()) {
            HandleOutcome::NonRetryable(Reason(r)) => {
                assert!(r.contains("source"), "names the field: {r}")
            }
            other => panic!("a malformed edge event must be a non-retryable poison, got {other:?}"),
        }
        assert_eq!(b.projection().total_count(&tenant(), &region()), 0);
    }

    #[test]
    fn removed_tombstones_and_is_idempotent() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/chat/message/m1";
        let tgt = "myelin://acme/issue/issue/ENG-1";
        b.handle(
            &edge_event("01J-c", "refs.edge.created", src, tgt, "mentions"),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(b.projection().live_count(&tenant(), &region()), 1);

        let rm = edge_event("01J-r", "refs.edge.removed", src, tgt, "mentions");
        assert_eq!(
            b.handle(&rm, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            0,
            "tombstoned → hidden from live"
        );
        assert_eq!(
            b.projection().total_count(&tenant(), &region()),
            1,
            "row retained for audit"
        );
        assert_eq!(
            b.handle(&rm, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(b.projection().total_count(&tenant(), &region()), 1);

        let id = edge_id(&tenant(), src, tgt, "mentions");
        assert!(
            b.projection()
                .get(&tenant(), &region(), &id)
                .unwrap()
                .tombstoned
        );
    }

    #[test]
    fn removal_arriving_first_leaves_a_fence_for_its_delayed_creation() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let mut created = edge_event("01J-1", "refs.edge.created", "s", "t", "mentions");
        created.recorded_at = Timestamp("2026-06-20T00:00:01Z".into());
        let mut removed = edge_event("01J-2", "refs.edge.removed", "s", "t", "mentions");
        removed.recorded_at = Timestamp("2026-06-20T00:00:02Z".into());

        assert_eq!(
            b.handle(&removed, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "a removal can be projected before its creation"
        );
        assert_eq!(b.projection().live_count(&tenant(), &region()), 0);
        assert_eq!(b.projection().total_count(&tenant(), &region()), 1);

        assert_eq!(
            b.handle(&created, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "the delayed broker delivery is harmless"
        );
        let row = b
            .projection()
            .get(
                &tenant(),
                &region(),
                &edge_id(&tenant(), "s", "t", "mentions"),
            )
            .expect("the removal fence remains inspectable");
        assert!(row.tombstoned, "old creation cannot resurrect the edge");
        assert_eq!(row.origin_event, "01J-2");
    }

    #[test]
    fn a_lifecycle_relation_appears_both_ways_and_disappears_both_ways() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let planning = "myelin://acme/issue/issue/PLAN-1";
        let delivery = "myelin://acme/issue/issue/SHIP-1";
        let mut created = edge_event("01J-1", "refs.edge.created", planning, delivery, "blocks");
        created.payload["rel_class"] = serde_json::json!("lifecycle");

        assert_eq!(
            b.handle(&created, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        let forward = b
            .projection()
            .get(
                &tenant(),
                &region(),
                &edge_id(&tenant(), planning, delivery, "blocks"),
            )
            .expect("planning blocks delivery");
        let inverse = b
            .projection()
            .get(
                &tenant(),
                &region(),
                &edge_id(&tenant(), delivery, planning, "blocked_by"),
            )
            .expect("delivery is blocked by planning");
        assert_eq!(forward.rel_class, RelClass::Lifecycle);
        assert_eq!(inverse.rel_class, RelClass::Lifecycle);
        assert_eq!(b.projection().live_count(&tenant(), &region()), 2);

        let mut removed = edge_event("01J-2", "refs.edge.removed", planning, delivery, "blocks");
        removed.recorded_at = Timestamp("2026-06-20T00:00:02Z".into());
        removed.payload["rel_class"] = serde_json::json!("lifecycle");
        assert_eq!(
            b.handle(&removed, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            0,
            "removing the typed relation removes both navigable directions"
        );
        assert_eq!(b.projection().total_count(&tenant(), &region()), 2);
    }

    #[test]
    fn erased_tombstones_the_edge() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let src = "myelin://acme/chat/message/m1";
        let tgt = "myelin://acme/issue/issue/ENG-1";
        b.handle(
            &edge_event("01J-c", "refs.edge.created", src, tgt, "mentions"),
            &mut myelin_events::HandlerTx::none(),
        );
        let er = edge_event("01J-e", "chat.message.erased", src, tgt, "mentions");
        assert_eq!(
            b.handle(&er, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done
        );
        assert_eq!(
            b.projection().live_count(&tenant(), &region()),
            0,
            "erased → tombstoned"
        );
    }

    #[test]
    fn index_lag_is_zero_in_steady_state_and_named() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        assert_eq!(b.index_lag(), 0, "a fresh builder has no lag");
        b.handle(
            &edge_event("01J-1", "refs.edge.created", "s", "t", "mentions"),
            &mut myelin_events::HandlerTx::none(),
        );
        assert_eq!(b.index_lag(), 0, "index_lag returns to 0 after projection");
        assert_eq!(
            RefsEdgeBuilder::INDEX_LAG_SIGNAL,
            "refs.index_lag",
            "the contract-1.8 signal name"
        );
    }
}
