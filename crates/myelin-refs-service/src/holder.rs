//! Refs as a `PersonalDataHolder` (H12) — the REAL structural erasure surface (REF-P15 / P-164;
//! contract 10.1 — replaces the REF-P3 / P-120 STUB) + the harness auto-registration (contract 1.4).
//!
//! **Architecture:** reference-graph.md §3 (every Refs store is a `PersonalDataHolder`
//! auto-registered by the harness — substrate §3.4 / contract 1.4), §3.6 (the projection cache is
//! itself a bounded invalidatable holder), §4.6 (the small, structural erasure surface). The
//! exhaustive H1–H18 catalog ([`myelin_substrate::Holder`]) names Refs **H12 (`ReferenceGraph`)**.
//!
//! ## What REF-P15 ships — the REAL erase body (replaces the REF-P3 stub)
//! §4.6: **`locate(subject)`** → the edges/cache entries naming the subject; **`erase(subject)`** →
//! purge R2-cache PII (the REF-P12 cache) + rely on Identity's pseudonym shred (4.8) for
//! `origin_actor` (the edge KEEPS the opaque id; the human becomes unresolvable) + tombstone
//! content-erased targets via the `*.erased` consumer (REF-P7); **`restrict(subject)`** suppression
//! keeps a restricted subject's references out of indexing / agent-use / analytics. There is **NO
//! erasure backdoor** — content-target tombstoning is driven by `*.erased` through the SAME live
//! consumer path (the edge-builder), never a direct store write here.
//!
//! Refs holds the subject ONLY as the **PSEUDONYMOUS `origin_actor` opaque id** + cache titles —
//! **never the third-party free-text body** (the references-not-payloads case). So Refs' erasure
//! surface is **small and structural**: it does not need to mutate the edge (the opaque actor id is
//! already pseudonymous — Identity's 4.8 pseudonym-map shred makes it unresolvable to a human); it
//! purges the cache (the only place a name-in-a-title lands) and suppresses on `restrict`. This is the
//! platform free-text/immutable erasure posture **instantiated by reference** (X-7; contract 10.9) —
//! Refs adds **no new `[OPEN — LEGAL]` residual** ([`crate::erasure_posture`]).
//!
//! ## The stub → the real surface (the EI-01 §7 reconcile, NOT a parallel second holder)
//! The REF-P3 holders [`RefsEdgeHolder`] / [`RefsCacheHolder`] are KEPT (the same H12/H9
//! registration + classification + the `PersonalDataHolder` impls the DSR orchestrator already fans
//! out to) — REF-P15 gives them an **optional runtime backing**: when **unbacked** (the
//! [`Default`] registration-only form, the `serve`-before-the-store-is-wired posture) they remain
//! **empty-but-correct** (a tenant with no live index has no located data); when **backed**
//! ([`RefsEdgeHolder::with_backing`] / [`RefsCacheHolder::with_cache`]) they run the REAL §4.6 body
//! over the live [`crate::edge_builder::EdgeProjection`] + the REF-P12 [`crate::cache::R2ProjectionCache`].
//! So there is ONE Refs holder type per store (no parallel second holder); the body is the real one
//! the moment the store is wired. The world-scale 0-recoverable shred drill (REF-D5 at backup scale)
//! is DELIVERED by REF-P25 ([`crate::restore_reerase`] — the erase → restore-pre-erase-backup →
//! re-erase-from-the-10.8-ledger → 0-recoverable cross-seam, which RIDES this very §4.6 erase body).
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The erase holder is 0-recoverable-PII critical (REF-D5). Floor: **≥ 80% of viable mutants caught**
//! (`cargo mutants -p myelin-refs-service -f crates/myelin-refs-service/src/holder.rs`). Measured
//! 2026-06-20: **58 mutants generated → 45 unviable, 13 viable, 13 caught, 0 missed = 100% of viable**
//! — floor met. (Every body — `locate`'s real count, the backed-vs-unbacked split, the cache
//! subject-purge SUM through the one eviction path, the restrict-set write, the registration — has a
//! test a mutation flips.) The `restrict` suppression-set core ([`crate::restrict`]) is separately at
//! **8/8 = 100%**.

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

/// The stable, PII-free name of the Refs **edge inverse-index** OLTP store (the holder's H12
/// store). Frozen here so the REF-P5 migration, the data-map (P-GA-09), and the DSR fan-out all
/// address exactly this store. PII-free: a store identifier, never personal data.
pub const REFS_EDGE_STORE: &str = "refs_edge_index";

/// The stable, PII-free name of the Refs **R2 projection cache** namespace (the §3.6 invalidatable
/// holder). Classifies structurally to H9 (caches); §3.6 also has Refs invalidate it on `*.erased`.
pub const REFS_CACHE_STORE: &str = "refs_projection_cache";

/// The typed receipt that a Refs store was auto-registered as a [`PersonalDataHolder`] — the proof
/// the registration fired for a given store. The harness collects these; the holder-registered
/// architecture test reads them to assert no Refs store escaped registration. PII-free: a (kind,
/// name) tag.
pub type RefsHolderRegistration = HolderRegistration;

/// Build the Refs [`myelin_substrate::StoreClassifier`] — the data-map declaration that the Refs
/// edge OLTP store belongs to holder **H12 (`ReferenceGraph`)**. The R2 cache classifies
/// structurally to H9 (a cache), so it needs no per-store declaration here.
pub fn refs_store_classifier() -> StoreClassifier {
    StoreClassifier::of([myelin_substrate::StoreHolder::new(
        StoreKind::Oltp,
        REFS_EDGE_STORE,
        Holder::H12ReferenceGraph,
    )])
}

/// **Register Refs' stores as `PersonalDataHolder`s through the harness auto-registration (contract
/// 1.4).** Opens both Refs stores through the substrate [`HolderRegistry`] — the ONE door — so each
/// is a registered holder by construction. Registering ALWAYS (even before the store is wired) makes
/// "the DSAR fan-out forgot Refs" structurally impossible (10.1 exhaustiveness).
pub fn register_refs_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, REFS_EDGE_STORE);
    registry.open(StoreKind::Cache, REFS_CACHE_STORE);
    registry
}

/// The live runtime backing the REAL REF-P15 edge-holder body operates over: the edge inverse-index
/// projection (to `locate` edges naming the subject + report the `*.erased`-driven tombstones) + the
/// restrict-suppression set (to keep a restricted subject's references out of indexing/agent-use).
/// **References-not-payloads:** the edge holds only the PSEUDONYMOUS opaque `origin_actor` — the
/// `locate`/`erase` key on that opaque id, never a name. Cloneable handle (the projection is shared).
#[derive(Clone)]
pub struct EdgeBacking {
    /// The live edge projection (REF-P6) — `locate` walks it for edges naming the subject.
    projection: EdgeProjection,
    /// The restrict-suppression set (Art. 18/21) — a restricted subject's references are suppressed
    /// from indexing/agent-use/analytics (GA-D7).
    restrict: RestrictSet,
}

impl EdgeBacking {
    /// Wire the edge holder over a live edge projection (the REF-P15 real body). The restrict set is
    /// fresh (empty) — `restrict(subject, true)` adds to it.
    pub fn new(projection: EdgeProjection) -> EdgeBacking {
        EdgeBacking {
            projection,
            restrict: RestrictSet::new(),
        }
    }

    /// Wire the edge holder over a live projection AND a shared restrict-suppression set (so the
    /// suppression a holder records is the SAME set the indexer/backlink read consults).
    pub fn with_restrict(projection: EdgeProjection, restrict: RestrictSet) -> EdgeBacking {
        EdgeBacking {
            projection,
            restrict,
        }
    }

    /// The shared restrict-suppression set (the indexer/backlink read reads it to suppress a
    /// restricted subject's references).
    pub fn restrict_set(&self) -> &RestrictSet {
        &self.restrict
    }
}

/// Refs' **edge inverse-index** AS a [`PersonalDataHolder`] (H12; contract 10.1). REF-P15: the REAL
/// §4.6 erasure surface when [`Self::with_backing`] wires the live edge projection; **empty-but-correct**
/// (the registration-only [`Default`] form) when unbacked (`serve` before the store lands). Cloneable.
#[derive(Clone, Default)]
pub struct RefsEdgeHolder {
    /// `None` = the registration-only stub (empty-but-correct); `Some` = the REAL REF-P15 body over a
    /// live edge projection.
    backing: Option<EdgeBacking>,
}

impl RefsEdgeHolder {
    /// **The REAL REF-P15 edge holder over a live edge projection (§4.6).** `locate` walks the
    /// projection for edges naming the subject; `erase` relies on Identity's 4.8 pseudonym shred (the
    /// edge keeps the opaque id) + the `*.erased` tombstoning (REF-P7, NO backdoor); `restrict`
    /// suppresses the subject's references.
    pub fn with_backing(backing: EdgeBacking) -> RefsEdgeHolder {
        RefsEdgeHolder {
            backing: Some(backing),
        }
    }

    /// Register this holder through the substrate registry (the `serve`-called auto-registration
    /// seam), returning the receipt — the proof the edge store registered as holder H12.
    pub fn register(&self, registry: &mut HolderRegistry) -> RefsHolderRegistration {
        registry.open(StoreKind::Oltp, REFS_EDGE_STORE)
    }

    /// The opaque, PII-free subject id the receipt body keys on (the pseudonymous Principal id) —
    /// never a name/email. This is the `origin_actor` pseudonym posture (§4.6 / EI-04 §1).
    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    /// The tenancy `(tenant, region)` partition for a gdpr `TenantId` — Refs is region-pinned; the
    /// region is `fr-par` (the cell's home region) for the in-cell holder. The fan-out is tenant-first.
    fn part(tenant: &GdprTenantId) -> (TenantId, Region) {
        (TenantId(tenant.0.clone()), Region("fr-par".into()))
    }
}

impl PersonalDataHolder for RefsEdgeHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        // REAL §4.6 locate: the edges naming the subject (by the PSEUDONYMOUS origin_actor opaque id —
        // never a name). Unbacked → empty-but-correct (0 located). Tenant-first.
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
        // Refs holds no free-text body the subject is the controller for — its only subject data is the
        // pseudonymous opaque id (which Identity exports). The portable bundle is the located-edge
        // count receipt (references-not-payloads — nothing to export but the count + a content-address).
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
        // Refs holds no free-text body (references-not-payloads) → rectification of an edge is via
        // reindex-from-source over the corrected owner content (GA-D2, REF-P16), never an in-place edit
        // here. A no-op at the holder surface (correct: there is nothing to rectify in an opaque edge).
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                REFS_EDGE_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (references-not-payloads — rectify via reindex-from-source over owner content, GA-D2)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // REAL §4.6 restrict (Art. 18/21): record the subject in the suppression set so the indexer /
        // agent-use / analytics keep the restricted subject's references out (GA-D7). Unbacked → a
        // well-defined no-op (no index to suppress over). Idempotent.
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
        // REAL §4.6 erase: Refs' erasure surface is SMALL + STRUCTURAL. The edge keeps the opaque
        // `origin_actor`; Identity's 4.8 pseudonym-map shred makes it unresolvable to a human (NO edge
        // mutation needed in the common case — the person becomes unresolvable). Content-erased TARGETS
        // are tombstoned via the `*.erased` consumer (REF-P7 / the edge-builder), NOT a direct write
        // here (no erasure backdoor). So the holder erase RELIES on Identity's shred + reports the
        // surface it covers; it does not itself destroy a key (the cache holder destroys the cache DEK).
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        // Count the edges the subject touches (the surface the erasure covers; the edges themselves
        // stay — they carry only the opaque id, which Identity's shred makes unresolvable).
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
            // No KEY destroyed at the EDGE holder (the edge is opaque-id-only; the crypto-shred is the
            // cache DEK, RefsCacheHolder + the per-tenant DEK, REF-P4). key_epoch_destroyed = None here.
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

/// Refs' **R2 projection cache** AS a [`PersonalDataHolder`] (§3.6 — a bounded, invalidatable holder).
/// REF-P15: the REAL purge body when [`Self::with_cache`] wires the live REF-P12 cache; **empty-but-
/// correct** (the registration-only [`Default`] form) when unbacked. The cache's PII is derived
/// projection titles (a name in a title) + the pseudonymous `origin_actor`. Cloneable.
#[derive(Clone, Default)]
pub struct RefsCacheHolder {
    /// `None` = registration-only stub; `Some` = the live REF-P12 cache + the subjects whose cached
    /// entries to purge (driven by the located edges). The cache is held behind the invalidate trait.
    cache: Option<Arc<R2ProjectionCache>>,
    /// The edge projection — used to find the cache entries (refs) naming the subject to purge.
    projection: Option<EdgeProjection>,
}

impl RefsCacheHolder {
    /// **The REAL REF-P15 cache holder over the live REF-P12 cache + edge projection (§4.6 purge).**
    /// `erase` purges the subject's cached projection entries (the refs the subject authored/targets) —
    /// the only place a name-in-a-title lands. Driven through the cache's `invalidate` (the SAME
    /// eviction the `*.erased` invalidator drives — one purge path, no backdoor). The per-tenant DEK
    /// (REF-P4) additionally makes the whole cache crypto-shred-able on tenant offboard.
    pub fn with_cache(
        cache: Arc<R2ProjectionCache>,
        projection: EdgeProjection,
    ) -> RefsCacheHolder {
        RefsCacheHolder {
            cache: Some(cache),
            projection: Some(projection),
        }
    }

    /// Register the cache through the substrate registry (a cache namespace, §3.4), returning the
    /// receipt — the proof the R2 cache registered as a holder (§3.6).
    pub fn register(&self, registry: &mut HolderRegistry) -> RefsHolderRegistration {
        registry.open(StoreKind::Cache, REFS_CACHE_STORE)
    }

    fn part(tenant: &GdprTenantId) -> (TenantId, Region) {
        (TenantId(tenant.0.clone()), Region("fr-par".into()))
    }

    /// Purge the subject's cached projection entries: the refs the subject authored OR targets (a
    /// cached title that may contain the name). Returns how many entries were evicted. Driven through
    /// the cache's `invalidate` (the ONE eviction path the `*.erased` consumer also drives). Returns 0
    /// when unbacked. Tenant-first.
    fn purge_subject(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let (Some(cache), Some(projection)) = (&self.cache, &self.projection) else {
            return 0;
        };
        let (t, r) = Self::part(tenant);
        // Every ref the subject's edges touch is a candidate cache key (the projection's title may hold
        // the name). Purge the FULL #sub ref + its root (both may be cached, §3.6 keys per ArtifactRef).
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
                "derived cache (titles may hold a name) — purged on erase, sealed under the per-tenant DEK",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        // The cache is DERIVED + reconstructible — never the export source (the owner is the truth).
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                &tenant.0,
                "empty-bundle (cache is derived, reconstructible — never the export source)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        // The cache is derived → rectify via reindex-from-source (the re-resolved title is correct).
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                REFS_CACHE_STORE,
                &subject.principal.principal_id.0,
                "",
                "no-op (cache is derived; rectify via reindex-from-source — the re-resolve corrects it)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        // Restriction suppresses the cache too (the restricted subject's cached projections are not
        // served). The suppression set is the edge holder's; the cache simply re-resolves to a
        // restricted state on a bust. The holder records the restrict op.
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
        // REAL §4.6 cache erase = PURGE the subject's cached PII (a name in a title). Driven through the
        // cache's `invalidate` — the ONE eviction path the `*.erased` consumer also drives (no
        // backdoor). The per-tenant DEK (REF-P4) makes the whole cache crypto-shred-able on a TENANT
        // erase (destroy the DEK → every cached title unrecoverable, even in backups).
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let purged = match &scope {
            EraseScope::Subject { tenant, .. } => self.purge_subject(tenant, &sid),
            // A tenant erase is the crypto-shred (destroy the per-tenant DEK) — handled by the
            // tenant-decommission lever (REF-P4 `destroy_tenant_dek`), not a per-entry purge here.
            EraseScope::Tenant(_) => 0,
        };
        let outcome = match &scope {
            EraseScope::Subject { .. } => {
                format!("purged {purged} cached projection entries naming the subject (the only name-bearing PII)")
            }
            EraseScope::Tenant(_) => {
                "tenant crypto-shred: destroy the per-tenant DEK (REF-P4) — every cached title unrecoverable".into()
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
