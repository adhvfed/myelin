use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_refs::{strip_sub, ArtifactRef};
use myelin_tenancy::{Region, TenantId};

pub static EDGE_BUILDER_SUBJECTS: &[SubjectPattern] = &[];

pub const EDGE_BUILDER_CONSUMER: &str = "refs-edge-builder";

pub const EDGE_BUILDER_SUBJECT_PREFIXES: &[&str] =
    &["refs.edge.", "issue.relation.", "knowledge.page."];

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
    let mut h: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h ^= b as u128;
            h = h.wrapping_mul(PRIME);
        }
        h ^= 0x00;
        h = h.wrapping_mul(PRIME);
    };
    feed(tenant.0.as_bytes());
    feed(source.as_bytes());
    feed(target.as_bytes());
    feed(rel.as_bytes());
    format!("{h:032x}")
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
    Upsert(EdgeRow),
    Tombstone { edge_id: String },
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

    let source = required_field(ev, "source")?;
    let target = required_field(ev, "target")?;
    let rel = required_field(ev, "rel")?;
    let source_ref = ArtifactRef(source.clone());
    let target_ref = ArtifactRef(target.clone());
    Ok(EdgeMutation::Upsert(EdgeRow {
        edge_id: edge_id(&ev.tenant, &source, &target, &rel),
        source_root: strip_sub(&source_ref),
        target_root: strip_sub(&target_ref),
        source: source_ref,
        target: target_ref,
        rel,
        rel_class: if ev.type_.0.starts_with("refs.edge.") {
            RelClass::Reference
        } else {
            RelClass::Lifecycle
        },
        origin_event: ev.event_id.0.clone(),
        origin_actor: str_field(payload, "origin_actor")
            .unwrap_or_else(|| ev.actor.0.principal_id.0.clone()),
        zookie: str_field(payload, "zookie"),
        tombstoned: false,
    }))
}

fn removed_edge_mutation(ev: &EventEnvelope) -> Result<EdgeMutation, ProjectError> {
    let edge_id = match str_field(&ev.payload, "edge_id") {
        Some(edge_id) => edge_id,
        None => {
            let source = required_removal_field(ev, "source")?;
            let target = required_removal_field(ev, "target")?;
            let rel = required_removal_field(ev, "rel")?;
            edge_id(&ev.tenant, &source, &target, &rel)
        }
    };
    Ok(EdgeMutation::Tombstone { edge_id })
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

#[derive(Clone, Default)]
pub struct EdgeProjection {
    inner: Arc<Mutex<HashMap<PartKey, HashMap<String, EdgeRow>>>>,
}

impl EdgeProjection {
    pub fn new() -> EdgeProjection {
        EdgeProjection::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PartKey, HashMap<String, EdgeRow>>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn upsert(&self, tenant: &TenantId, region: &Region, row: EdgeRow) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut inner = self.lock();
        inner
            .entry(pk)
            .or_default()
            .insert(row.edge_id.clone(), row);
    }

    pub fn tombstone(&self, tenant: &TenantId, region: &Region, edge_id: &str, origin_event: &str) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut inner = self.lock();
        if let Some(part) = inner.get_mut(&pk) {
            if let Some(row) = part.get_mut(edge_id) {
                row.tombstoned = true;
                row.origin_event = origin_event.to_string();
            }
        }
    }

    pub fn get(&self, tenant: &TenantId, region: &Region, edge_id: &str) -> Option<EdgeRow> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().get(&pk).and_then(|p| p.get(edge_id).cloned())
    }

    pub fn live_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock()
            .get(&pk)
            .map(|p| p.values().filter(|r| !r.tombstoned).count())
            .unwrap_or(0)
    }

    pub fn total_count(&self, tenant: &TenantId, region: &Region) -> usize {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock().get(&pk).map(|p| p.len()).unwrap_or(0)
    }

    pub fn parity_bytes(&self, tenant: &TenantId, region: &Region) -> Vec<u8> {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let mut rows: Vec<EdgeRow> = self
            .lock()
            .get(&pk)
            .map(|p| p.values().cloned().collect())
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
                p.values()
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
                p.values()
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
                p.values()
                    .filter(|r| !r.tombstoned && &r.source_root == source_root)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        rows.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
        rows
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
            EdgeMutation::Upsert(row) => self.projection.upsert(&ev.tenant, &ev.region, row),
            EdgeMutation::Tombstone { edge_id } => {
                self.projection
                    .tombstone(&ev.tenant, &ev.region, &edge_id, &ev.event_id.0)
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
            "field boundaries are unambiguous (NUL-separated)"
        );
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
    fn tombstone_of_absent_edge_is_a_noop() {
        let b = RefsEdgeBuilder::new(EdgeProjection::new());
        let rm = edge_event("01J-r", "refs.edge.removed", "s", "t", "mentions");
        assert_eq!(
            b.handle(&rm, &mut myelin_events::HandlerTx::none()),
            HandleOutcome::Done,
            "removal of absent edge is a no-op"
        );
        assert_eq!(b.projection().total_count(&tenant(), &region()), 0);
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
