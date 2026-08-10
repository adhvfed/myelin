use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use myelin_identity::{Consistency, ListObjectsResult, Principal, SetExpr};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::backlinks::{
    set_expr_admits, AuthzVisibleIndex, Backlink, BacklinkError, BacklinkPage, FilterMode,
};
use crate::edge_builder::{EdgeProjection, EdgeRow};

pub const R4_READ_BUDGET_FANOUT: u64 = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum R4Verdict {
    ServeFromR4 { measured_fanout: u64, budget: u64 },
    ServeFromCte { measured_fanout: u64, budget: u64 },
}

impl R4Verdict {
    pub fn is_promoted(&self) -> bool {
        matches!(self, R4Verdict::ServeFromR4 { .. })
    }

    pub fn measured_fanout(&self) -> u64 {
        match self {
            R4Verdict::ServeFromR4 {
                measured_fanout, ..
            }
            | R4Verdict::ServeFromCte {
                measured_fanout, ..
            } => *measured_fanout,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReachEntry {
    edge_id: String,
    backlink: Backlink,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartKey {
    tenant: TenantId,
    region: Region,
}

type TargetReach = HashMap<String, Vec<ReachEntry>>;
type ReachMap = HashMap<PartKey, TargetReach>;

#[derive(Clone)]
pub struct R4ReachIndex {
    reach: Arc<Mutex<ReachMap>>,
    authz: AuthzVisibleIndex,
    read_budget_fanout: u64,
    last_fanout_sample: Arc<AtomicU64>,
    r4_served_count: Arc<AtomicU64>,
}

impl R4ReachIndex {
    pub const HOT_ARTIFACT_FANOUT_SIGNAL: &'static str = "refs.hot_artifact_fanout";

    pub fn new(authz: AuthzVisibleIndex, read_budget_fanout: u64) -> R4ReachIndex {
        R4ReachIndex {
            reach: Arc::new(Mutex::new(HashMap::new())),
            authz,
            read_budget_fanout,
            last_fanout_sample: Arc::new(AtomicU64::new(0)),
            r4_served_count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ReachMap> {
        self.reach.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn rebuild_from_r1(
        &self,
        r1: &EdgeProjection,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) {
        let live = r1.inbound_live(tenant, region, target_root);
        let entries: Vec<ReachEntry> = live.iter().map(Self::entry_from_row).collect();
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock()
            .entry(pk)
            .or_default()
            .insert(target_root.0.clone(), entries);
    }

    pub fn on_edge_upsert(&self, tenant: &TenantId, region: &Region, row: &EdgeRow) {
        if row.tombstoned {
            self.on_edge_tombstone(tenant, region, &row.edge_id, &row.target_root);
            return;
        }
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let entry = Self::entry_from_row(row);
        let mut guard = self.lock();
        let bucket = guard
            .entry(pk)
            .or_default()
            .entry(row.target_root.0.clone())
            .or_default();
        match bucket.binary_search_by(|e| e.edge_id.cmp(&row.edge_id)) {
            Ok(pos) => bucket[pos] = entry,
            Err(pos) => bucket.insert(pos, entry),
        }
    }

    pub fn on_edge_tombstone(
        &self,
        tenant: &TenantId,
        region: &Region,
        edge_id: &str,
        target_root: &ArtifactRef,
    ) {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        if let Some(part) = self.lock().get_mut(&pk) {
            if let Some(bucket) = part.get_mut(&target_root.0) {
                bucket.retain(|e| e.edge_id != edge_id);
            }
        }
    }

    pub fn measured_fanout(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> u64 {
        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        self.lock()
            .get(&pk)
            .and_then(|p| p.get(&target_root.0))
            .map(|b| b.len() as u64)
            .unwrap_or(0)
    }

    pub fn promotion_verdict(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> R4Verdict {
        let measured_fanout = self.measured_fanout(tenant, region, target_root);
        self.last_fanout_sample
            .store(measured_fanout, Ordering::SeqCst);
        if measured_fanout > self.read_budget_fanout {
            R4Verdict::ServeFromR4 {
                measured_fanout,
                budget: self.read_budget_fanout,
            }
        } else {
            R4Verdict::ServeFromCte {
                measured_fanout,
                budget: self.read_budget_fanout,
            }
        }
    }

    pub fn is_promoted(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> bool {
        self.promotion_verdict(tenant, region, target_root)
            .is_promoted()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn backlinks(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
        viewer: &Principal,
        list_objects: &ListObjectsResult,
        _at: &Consistency,
        page: usize,
    ) -> Result<BacklinkPage, BacklinkError> {
        if page == 0 {
            return Err(BacklinkError::InvalidPage);
        }
        self.r4_served_count.fetch_add(1, Ordering::SeqCst);

        let (set_expr, mode) = match list_objects {
            ListObjectsResult::Ids { ids, .. } => (SetExpr::Ids(ids.clone()), FilterMode::Ids),
            ListObjectsResult::Filter { set_expr, .. } => {
                (set_expr.clone(), FilterMode::PushedDown)
            }
        };

        let pk = PartKey {
            tenant: tenant.clone(),
            region: region.clone(),
        };
        let guard = self.lock();
        let admitted: Vec<Backlink> = guard
            .get(&pk)
            .and_then(|p| p.get(&target_root.0))
            .map(|bucket| {
                bucket
                    .iter()
                    .filter(|e| {
                        set_expr_admits(
                            &set_expr,
                            &self.authz,
                            viewer,
                            tenant,
                            region,
                            &e.backlink.source_root,
                        )
                    })
                    .take(page)
                    .map(|e| e.backlink.clone())
                    .collect()
            })
            .unwrap_or_default();
        drop(guard);

        Ok(BacklinkPage {
            edges: admitted,
            mode,
            fell_back_to_check: false,
        })
    }

    pub fn authz_index(&self) -> &AuthzVisibleIndex {
        &self.authz
    }

    pub fn last_fanout_sample(&self) -> u64 {
        self.last_fanout_sample.load(Ordering::SeqCst)
    }

    pub fn r4_served_count(&self) -> u64 {
        self.r4_served_count.load(Ordering::SeqCst)
    }

    pub fn read_budget_fanout(&self) -> u64 {
        self.read_budget_fanout
    }

    fn entry_from_row(row: &EdgeRow) -> ReachEntry {
        ReachEntry {
            edge_id: row.edge_id.clone(),
            backlink: Backlink {
                source: row.source.clone(),
                source_root: row.source_root.clone(),
                rel: row.rel.clone(),
                rel_class: row.rel_class.as_str().into(),
                origin_actor: row.origin_actor.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests;
