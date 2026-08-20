use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use myelin_events::ArtifactRef;
use myelin_identity::{
    AuthzError, ColRef, Consistency, ConsistencyMode, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr,
};
use myelin_tenancy::TenantId;

use crate::storm_control::subject_root_of;

pub const WATCH_PERMISSION: &str = "watch";

pub const WATCHER_RELATION: &str = "watcher";

pub const SUBJECT_ROOT_TYPE: &str = "subject_root";

pub fn subject_root_col() -> ColRef {
    ColRef {
        table: "notif_inbox_item".into(),
        column: "subject_root".into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadFanoutMarker {
    pub tenant: TenantId,
    pub subject_root: String,
    pub subject: ArtifactRef,
    pub reason: crate::Reason,
    pub count: u64,
    pub latest_origin: ArtifactRef,
}

#[derive(Clone, Default)]
pub struct AmbientMarkerStore {
    inner: Arc<Mutex<HashMap<(String, String), ReadFanoutMarker>>>,
}

impl AmbientMarkerStore {
    pub fn new() -> AmbientMarkerStore {
        AmbientMarkerStore::default()
    }

    pub fn record(
        &self,
        tenant: &TenantId,
        subject: &ArtifactRef,
        reason: crate::Reason,
        origin: &ArtifactRef,
    ) {
        let root = subject_root_of(&subject.0);
        let key = (tenant.0.clone(), root.clone());
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match g.get_mut(&key) {
            Some(existing) => {
                existing.count += 1;
                existing.latest_origin = origin.clone();
            }
            None => {
                g.insert(
                    key,
                    ReadFanoutMarker {
                        tenant: tenant.clone(),
                        subject_root: root,
                        subject: subject.clone(),
                        reason,
                        count: 1,
                        latest_origin: origin.clone(),
                    },
                );
            }
        }
    }

    pub fn marker_count(&self, tenant: &TenantId) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .filter(|(t, _)| t == &tenant.0)
            .count()
    }

    pub fn get(&self, tenant: &TenantId, subject_root: &str) -> Option<ReadFanoutMarker> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(tenant.0.clone(), subject_root.to_string()))
            .cloned()
    }

    fn snapshot_for_tenant(&self, tenant: &TenantId) -> Vec<ReadFanoutMarker> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|m| m.tenant == *tenant)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RevisionWatermark(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseIndexAnswer {
    pub subject_roots: BTreeSet<String>,
    pub revision: RevisionWatermark,
}

impl ReverseIndexAnswer {
    pub fn honours(&self, required: RevisionWatermark) -> bool {
        self.revision >= required
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationalLeaf {
    InRelation {
        relation: RelName,
        via_column: ColRef,
    },
    TupleSet {
        index: myelin_identity::AuthzIndexRef,
    },
}

pub trait WatcherResolvePort {
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> AuthzResult<myelin_identity::ListObjectsResult>;

    fn resolve_relation(
        &self,
        _subject: &Principal,
        _leaf: &RelationalLeaf,
        _required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        Err(AuthzError::Unavailable(
            "the authz reverse index is not wired for this read-fanout path - a relational watcher \
             SetExpr leaf cannot be resolved (deny-when-unsure, ADR-03; the ambient item is HELD, \
             not leaked, §5.3)"
                .into(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadFanoutError {
    Unavailable(String),
    InvalidRevision,
    StaleReverseIndex {
        required: RevisionWatermark,
        served: RevisionWatermark,
    },
}

pub fn read_fanout(
    viewer: &Principal,
    markers: &AmbientMarkerStore,
    resolver: &dyn WatcherResolvePort,
    at: &Consistency,
) -> std::result::Result<Vec<ReadFanoutMarker>, ReadFanoutError> {
    let permission = Permission(WATCH_PERMISSION.into());
    let ty = ObjectType(SUBJECT_ROOT_TYPE.into());
    let result = resolver
        .list_objects(viewer, &permission, &ty, at)
        .map_err(|e| ReadFanoutError::Unavailable(format!("{e:?}")))?;

    let (set_expr, zookie) = match result {
        myelin_identity::ListObjectsResult::Ids { ids, .. } => {
            let reachable: BTreeSet<String> = ids.into_iter().map(|o| o.0).collect();
            return Ok(project_with(
                markers,
                &viewer.tenant,
                &Reachable::Some(reachable),
            ));
        }
        myelin_identity::ListObjectsResult::Filter { set_expr, zookie } => (set_expr, zookie),
    };

    let required = watermark_for(&zookie, at)?;
    let reachable = lower(viewer, &set_expr, resolver, required)?;
    Ok(project_with(markers, &viewer.tenant, &reachable))
}

fn lower(
    viewer: &Principal,
    expr: &SetExpr,
    resolver: &dyn WatcherResolvePort,
    required: RevisionWatermark,
) -> std::result::Result<Reachable, ReadFanoutError> {
    match expr {
        SetExpr::All => Ok(Reachable::All),
        SetExpr::None => Ok(Reachable::Some(BTreeSet::new())),
        SetExpr::Ids(ids) => Ok(Reachable::Some(ids.iter().map(|o| o.0.clone()).collect())),
        SetExpr::NotIds(ids) => Ok(Reachable::AllExcept(
            ids.iter().map(|o| o.0.clone()).collect(),
        )),
        SetExpr::InRelation {
            relation,
            via_column,
        } => {
            let leaf = RelationalLeaf::InRelation {
                relation: relation.clone(),
                via_column: via_column.clone(),
            };
            resolve_leaf(viewer, &leaf, resolver, required)
        }
        SetExpr::TupleSet { index } => {
            let leaf = RelationalLeaf::TupleSet {
                index: index.clone(),
            };
            resolve_leaf(viewer, &leaf, resolver, required)
        }
        SetExpr::Union(parts) => {
            let mut acc = Reachable::Some(BTreeSet::new());
            for p in parts {
                acc = acc.union(lower(viewer, p, resolver, required)?);
            }
            Ok(acc)
        }
        SetExpr::Intersect(parts) => {
            let mut acc = Reachable::All;
            for p in parts {
                acc = acc.intersect(lower(viewer, p, resolver, required)?);
            }
            Ok(acc)
        }
        SetExpr::Difference(a, b) => {
            let left = lower(viewer, a, resolver, required)?;
            let right = lower(viewer, b, resolver, required)?;
            Ok(left.difference(right))
        }
    }
}

fn resolve_leaf(
    viewer: &Principal,
    leaf: &RelationalLeaf,
    resolver: &dyn WatcherResolvePort,
    required: RevisionWatermark,
) -> std::result::Result<Reachable, ReadFanoutError> {
    let answer = resolver
        .resolve_relation(viewer, leaf, required)
        .map_err(|e| ReadFanoutError::Unavailable(format!("{e:?}")))?;
    if !answer.honours(required) {
        return Err(ReadFanoutError::StaleReverseIndex {
            required,
            served: answer.revision,
        });
    }
    Ok(Reachable::Some(answer.subject_roots))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Reachable {
    All,
    AllExcept(BTreeSet<String>),
    Some(BTreeSet<String>),
}

impl Reachable {
    fn contains(&self, root: &str) -> bool {
        match self {
            Reachable::All => true,
            Reachable::AllExcept(deny) => !deny.contains(root),
            Reachable::Some(set) => set.contains(root),
        }
    }

    fn union(self, other: Reachable) -> Reachable {
        match (self, other) {
            (Reachable::All, _) | (_, Reachable::All) => Reachable::All,
            (Reachable::AllExcept(a), Reachable::AllExcept(b)) => {
                Reachable::AllExcept(a.intersection(&b).cloned().collect())
            }
            (Reachable::AllExcept(a), Reachable::Some(s))
            | (Reachable::Some(s), Reachable::AllExcept(a)) => {
                Reachable::AllExcept(a.difference(&s).cloned().collect())
            }
            (Reachable::Some(a), Reachable::Some(b)) => {
                Reachable::Some(a.union(&b).cloned().collect())
            }
        }
    }

    fn intersect(self, other: Reachable) -> Reachable {
        match (self, other) {
            (Reachable::All, x) | (x, Reachable::All) => x,
            (Reachable::AllExcept(a), Reachable::AllExcept(b)) => {
                Reachable::AllExcept(a.union(&b).cloned().collect())
            }
            (Reachable::AllExcept(a), Reachable::Some(s))
            | (Reachable::Some(s), Reachable::AllExcept(a)) => {
                Reachable::Some(s.difference(&a).cloned().collect())
            }
            (Reachable::Some(a), Reachable::Some(b)) => {
                Reachable::Some(a.intersection(&b).cloned().collect())
            }
        }
    }

    fn difference(self, other: Reachable) -> Reachable {
        match (self, other) {
            (_, Reachable::All) => Reachable::Some(BTreeSet::new()),
            (Reachable::All, Reachable::Some(s)) => Reachable::AllExcept(s),
            (Reachable::All, Reachable::AllExcept(b)) => Reachable::Some(b),
            (Reachable::AllExcept(a), Reachable::Some(s)) => {
                Reachable::AllExcept(a.union(&s).cloned().collect())
            }
            (Reachable::AllExcept(a), Reachable::AllExcept(b)) => {
                Reachable::Some(b.difference(&a).cloned().collect())
            }
            (Reachable::Some(s), Reachable::Some(t)) => {
                Reachable::Some(s.difference(&t).cloned().collect())
            }
            (Reachable::Some(s), Reachable::AllExcept(b)) => {
                Reachable::Some(s.intersection(&b).cloned().collect())
            }
        }
    }
}

fn project_with(
    markers: &AmbientMarkerStore,
    tenant: &TenantId,
    reachable: &Reachable,
) -> Vec<ReadFanoutMarker> {
    let mut out: Vec<ReadFanoutMarker> = markers
        .snapshot_for_tenant(tenant)
        .into_iter()
        .filter(|m| reachable.contains(&m.subject_root))
        .collect();
    out.sort_by(|a, b| a.subject_root.cmp(&b.subject_root));
    out
}

fn watermark_for(
    zookie: &myelin_identity::Zookie,
    at: &Consistency,
) -> Result<RevisionWatermark, ReadFanoutError> {
    match at.mode {
        ConsistencyMode::Strong => parse_revision(&zookie.0)
            .map(RevisionWatermark)
            .ok_or(ReadFanoutError::InvalidRevision),
        ConsistencyMode::BoundedStale => Ok(RevisionWatermark(0)),
    }
}

fn parse_revision(zookie: &str) -> Option<u64> {
    zookie
        .strip_prefix("zk-")
        .and_then(|revision| revision.parse::<u64>().ok())
}

#[derive(Clone, Default)]
pub struct SyntheticReverseIndex {
    inner: Arc<Mutex<SyntheticState>>,
}

#[derive(Default)]
struct SyntheticState {
    watches: HashMap<(String, String), BTreeSet<String>>,
    revision: u64,
    unavailable: bool,
    served_revision_override: Option<u64>,
}

impl SyntheticReverseIndex {
    pub fn new() -> SyntheticReverseIndex {
        SyntheticReverseIndex::default()
    }

    pub fn grant_watch(
        &self,
        tenant: &TenantId,
        principal: &str,
        subject_root: &str,
    ) -> myelin_identity::Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        g.watches
            .entry((tenant.0.clone(), principal.to_string()))
            .or_default()
            .insert(subject_root.to_string());
        myelin_identity::Zookie(format!("zk-{}", g.revision))
    }

    pub fn revoke_watch(
        &self,
        tenant: &TenantId,
        principal: &str,
        subject_root: &str,
    ) -> myelin_identity::Zookie {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.revision += 1;
        if let Some(set) = g
            .watches
            .get_mut(&(tenant.0.clone(), principal.to_string()))
        {
            set.remove(subject_root);
        }
        myelin_identity::Zookie(format!("zk-{}", g.revision))
    }

    pub fn current_zookie(&self) -> myelin_identity::Zookie {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        myelin_identity::Zookie(format!("zk-{}", g.revision))
    }

    pub fn set_unavailable(&self, on: bool) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unavailable = on;
    }

    pub fn pin_served_revision(&self, revision: Option<u64>) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .served_revision_override = revision;
    }
}

impl WatcherResolvePort for SyntheticReverseIndex {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<myelin_identity::ListObjectsResult> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable(
                "synthetic reverse index unavailable".into(),
            ));
        }
        Ok(myelin_identity::ListObjectsResult::Filter {
            set_expr: SetExpr::InRelation {
                relation: RelName(WATCHER_RELATION.into()),
                via_column: subject_root_col(),
            },
            zookie: myelin_identity::Zookie(format!("zk-{}", g.revision)),
        })
    }

    fn resolve_relation(
        &self,
        subject: &Principal,
        leaf: &RelationalLeaf,
        _required: RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.unavailable {
            return Err(AuthzError::Unavailable(
                "synthetic reverse index unavailable".into(),
            ));
        }
        let watched = match leaf {
            RelationalLeaf::InRelation { relation, .. } if relation.0 == WATCHER_RELATION => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            RelationalLeaf::TupleSet { .. } => g
                .watches
                .get(&(subject.tenant.0.clone(), subject.principal_id.0.clone()))
                .cloned()
                .unwrap_or_default(),
            _ => BTreeSet::new(),
        };
        let served = g.served_revision_override.unwrap_or(g.revision);
        Ok(ReverseIndexAnswer {
            subject_roots: watched,
            revision: RevisionWatermark(served),
        })
    }
}

#[cfg(test)]
mod tests;
