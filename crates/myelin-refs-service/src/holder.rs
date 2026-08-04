use std::sync::Arc;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId as GdprTenantId,
};
use myelin_refs::ArtifactRef;
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use myelin_tenancy::{Region, TenantId};

use crate::cache::R2ProjectionCache;
use crate::edge_builder::EdgeProjection;
use crate::invalidator::ProjectionCache;
use crate::restrict::RestrictSet;

pub const REFS_EDGE_STORE: &str = "refs_edge_index";

pub const REFS_CACHE_STORE: &str = "refs_projection_cache";

pub type RefsHolderRegistration = HolderRegistration;

pub fn refs_store_classifier() -> StoreClassifier {
    StoreClassifier::of([myelin_substrate::StoreHolder::new(
        StoreKind::Oltp,
        REFS_EDGE_STORE,
        Holder::H12ReferenceGraph,
    )])
}

pub fn register_refs_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, REFS_EDGE_STORE);
    registry.open(StoreKind::Cache, REFS_CACHE_STORE);
    registry
}

#[derive(Clone)]
pub struct EdgeBacking {
    projection: EdgeProjection,
    restrict: RestrictSet,
}

impl EdgeBacking {
    pub fn new(projection: EdgeProjection) -> EdgeBacking {
        EdgeBacking {
            projection,
            restrict: RestrictSet::new(),
        }
    }

    pub fn with_restrict(projection: EdgeProjection, restrict: RestrictSet) -> EdgeBacking {
        EdgeBacking {
            projection,
            restrict,
        }
    }

    pub fn restrict_set(&self) -> &RestrictSet {
        &self.restrict
    }
}

#[derive(Clone, Default)]
pub struct RefsEdgeHolder {
    backing: Option<EdgeBacking>,
}

impl RefsEdgeHolder {
    pub fn with_backing(backing: EdgeBacking) -> RefsEdgeHolder {
        RefsEdgeHolder {
            backing: Some(backing),
        }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> RefsHolderRegistration {
        registry.open(StoreKind::Oltp, REFS_EDGE_STORE)
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    fn part(tenant: &GdprTenantId) -> (TenantId, Region) {
        (TenantId(tenant.0.clone()), Region("fr-par".into()))
    }
}

impl PersonalDataHolder for RefsEdgeHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        let sid = Self::subject_id(subject);
        let count = match &self.backing {
            Some(b) => {
                let (t, r) = Self::part(&tenant);
                b.projection.count_by_actor(&t, &r, &sid)
            }
            None => 0,
        };
        let outcome = format!(
            "located {count} edges naming the pseudonymous origin_actor (references-not-payloads)"
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                REFS_EDGE_STORE,
                &sid,
                &tenant.0,
                &outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        let sid = Self::subject_id(subject);
        let count = match &self.backing {
            Some(b) => {
                let (t, r) = Self::part(&tenant);
                b.projection.count_by_actor(&t, &r, &sid)
            }
            None => 0,
        };
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                REFS_EDGE_STORE,
                &sid,
                &tenant.0,
                &format!(
                    "references-not-payloads bundle: {count} opaque-actor edges, no free-text body"
                ),
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                REFS_EDGE_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (references-not-payloads - rectify via reindex-from-source over owner content, GA-D2)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        let applied = match &self.backing {
            Some(b) => {
                b.restrict.set(&sid, on);
                true
            }
            None => false,
        };
        let outcome = if applied {
            format!("restrict={on} recorded in the suppression set (references suppressed from indexing/agent-use)")
        } else {
            format!("restrict={on} no-op (no live index; suppression GA-D7)")
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                REFS_EDGE_STORE,
                &sid,
                "",
                &outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let count = match (&self.backing, &scope) {
            (Some(b), EraseScope::Subject { tenant, .. }) => {
                let (t, r) = Self::part(tenant);
                b.projection.count_by_actor(&t, &r, &sid)
            }
            _ => 0,
        };
        let outcome = format!(
            "structural erase: {count} edges keep the opaque origin_actor (Identity 4.8 pseudonym-shred \
             makes it unresolvable); content-erased targets tombstoned via *.erased (no backdoor)"
        );
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                REFS_EDGE_STORE,
                &sid,
                &tenant,
                &outcome,
                None,
                0,
            ),
        })
    }
}

#[derive(Clone, Default)]
pub struct RefsCacheHolder {
    cache: Option<Arc<R2ProjectionCache>>,
    projection: Option<EdgeProjection>,
}

impl RefsCacheHolder {
    pub fn with_cache(
        cache: Arc<R2ProjectionCache>,
        projection: EdgeProjection,
    ) -> RefsCacheHolder {
        RefsCacheHolder {
            cache: Some(cache),
            projection: Some(projection),
        }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> RefsHolderRegistration {
        registry.open(StoreKind::Cache, REFS_CACHE_STORE)
    }

    fn part(tenant: &GdprTenantId) -> (TenantId, Region) {
        (TenantId(tenant.0.clone()), Region("fr-par".into()))
    }

    fn purge_subject(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let (Some(cache), Some(projection)) = (&self.cache, &self.projection) else {
            return 0;
        };
        let (t, r) = Self::part(tenant);
        let mut purged = 0usize;
        let mut seen: Vec<ArtifactRef> = Vec::new();
        for edge in projection.edges_by_actor(&t, &r, subject_id) {
            for ref_ in [
                edge.source.clone(),
                edge.source_root.clone(),
                edge.target.clone(),
                edge.target_root.clone(),
            ] {
                if !seen.contains(&ref_) {
                    cache.invalidate(&t, &r, &ref_);
                    seen.push(ref_);
                    purged += 1;
                }
            }
        }
        purged
    }
}

impl PersonalDataHolder for RefsCacheHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                &tenant.0,
                "derived cache (titles may hold a name) - purged on erase, sealed under the per-tenant DEK",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                &tenant.0,
                "empty-bundle (cache is derived, reconstructible - never the export source)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                "",
                "no-op (cache is derived; rectify via reindex-from-source - the re-resolve corrects it)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                "",
                &format!("restrict={on} (restricted subject's cached projections re-resolve suppressed; GA-D7)"),
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let purged = match &scope {
            EraseScope::Subject { tenant, .. } => self.purge_subject(tenant, &sid),
            EraseScope::Tenant(_) => 0,
        };
        let outcome = match &scope {
            EraseScope::Subject { .. } => {
                format!("purged {purged} cached projection entries naming the subject (the only name-bearing PII)")
            }
            EraseScope::Tenant(_) => {
                "tenant crypto-shred: destroy the per-tenant DEK (REF-P4) - every cached title unrecoverable".into()
            }
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                REFS_CACHE_STORE,
                &sid,
                &tenant,
                &outcome,
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests;
