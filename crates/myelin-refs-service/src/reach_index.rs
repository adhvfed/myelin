//! # `reach_index` — the **hot-artifact Leopard-style flattened reach index R4** (REF-P23 / P-454, M5)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §6.3 (the hot-artifact backlink scale — the "viral PR / referenced-by-50,000" case: the BUILT floor
//! is the read-time CTE + `list_objects` filter + pagination + replica; the **FOLLOW-ON** is the
//! Leopard-style flattened reach index **R4**, derived/rebuildable from R1, incrementally maintained
//! from `refs.edge.*`, gated by the SAME `list_objects` filter, **promoted only when MEASURED
//! hot-fanout exceeds the read budget (R5), not predicted**), §6.2 (*measure before you shard* — R4 is
//! the third move, after the read replica + the read-time CTE), §3.7 (R4 is a derived/ephemeral FLOOR
//! component, rebuildable from R1). **External insight:** `02-platform-substrate.md` §7 (the
//! Leopard/Zanzibar set-flattened reverse index, measured-trigger); `01-process-and-quality-doctrine.md`
//! §3 (prove-it; measured-not-predicted; the `hot_artifact_fanout` telemetry is part of the pass).
//! **Contract-index:** row **5.3** at scale (the backlink read at scale — the R4 path), row **4.3** (R4
//! gated by the same `list_objects` filter), row **1.8** (the `hot_artifact_fanout` telemetry).
//!
//! ## What REF-P23 ships — and what it deliberately does NOT (VISION §3 / EI-01 §1)
//! REF-P11 ([`crate::backlinks`]) shipped the read-time **CTE floor**: the permission-filtered
//! backlink read paginates a hot target's inbound edges (you never materialise 50,000 backlinks, you
//! PAGE them). That floor NAMED R4 as its follow-on (`backlinks.rs` "Floors named"). **This module IS
//! that follow-on** — the Leopard-style flattened reach index R4:
//!
//! - **derived/rebuildable from R1** ([`crate::edge_builder::EdgeProjection`]): R4 holds a flattened
//!   `(tenant, region, target_root) → [inbound source edges]` reach set, precomputed from R1's inbound
//!   adjacency (§3.7 — R4 is derived, the log/R1 is the source of truth; a wipe + rebuild reproduces
//!   it);
//! - **incrementally maintained from `refs.edge.*`** ([`R4ReachIndex::on_edge_upsert`] /
//!   [`R4ReachIndex::on_edge_tombstone`]): a live edge created/removed updates the flattened set, so R4
//!   stays in lock-step with R1 (the steady-state == cold-rebuild discipline — the SAME code path feeds
//!   both, never a drift);
//! - **gated by the SAME `list_objects` filter** ([`crate::backlinks::set_expr_admits`]): R4 serves a
//!   read by conjoining the lowered `SetExpr` over each flattened candidate's `source_root` — the
//!   IDENTICAL leak-free predicate the CTE floor applies (there is NO second permission algebra; R4
//!   reuses the FROZEN [`set_expr_admits`] the backlink read lowers — the REF-P11 leak invariant cannot
//!   regress on the R4 path, since both paths run the same admit);
//! - **promoted ONLY on a MEASURED trigger (R5)** ([`R4ReachIndex::is_promoted`]): R4 serves a target
//!   ONLY when that target's MEASURED inbound fanout EXCEEDS the read budget
//!   ([`R4_READ_BUDGET_FANOUT`], read from the thresholds file `[refs_hot_artifact]`), not predicted —
//!   a cold target below budget still serves from the CTE floor (the §6.3 "promotion trigger = measured
//!   hot-fanout exceeding the read budget, not predicted"; ADR-10 measure-before-shard).
//!
//! ## The `hot_artifact_fanout` telemetry (contract 1.8 / §5.1 — observability is part of the pass)
//! Every R4-considered read samples the target's MEASURED inbound fanout
//! ([`R4ReachIndex::HOT_ARTIFACT_FANOUT_SIGNAL`] = `refs.hot_artifact_fanout`) — the signal the
//! measured-trigger keys on, and the loud artifact REF-D3 reads (a target whose fanout crosses the
//! budget is observably hot BEFORE R4 is promoted for it). The signal is a named constant — drills
//! assert against the NAME, never a literal (EI-01 §3).
//!
//! ## R4 parity with the CTE floor (REF-D3 — the prompt's TESTS clause)
//! The cardinal R4 property: **once promoted, R4 returns the SAME leak-free, paginated result set as
//! the CTE floor** ([`R4ReachIndex::backlinks`] vs [`crate::backlinks::BacklinkRead::backlinks`]). R4 is
//! a faster PATH to the same answer, never a different answer — same admit, same pagination order, same
//! tenant predicate. [`R4ReachIndex::backlinks`] returns the IDENTICAL [`BacklinkPage`] shape the CTE
//! floor returns (the parity the drill asserts byte-for-byte on the edge list). A read that is NOT
//! promotion-eligible refuses with [`R4Verdict::ServeFromCte`] — R4 never serves a cold target (the
//! measured-trigger discipline: no predicted promotion).
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **The WORLD-SCALE fleet-hardware re-measure of the real R5 crossover is the ONE remaining floor.**
//!   The read budget here ([`R4_READ_BUDGET_FANOUT`] = 1000, from the thresholds file) is the v1
//!   default-to-beat seeded at the synthetic REF-D3 corpus scale (the "referenced-by-50,000" case); the
//!   real crossover where the CTE p99 falls over its budget is re-measured on real fleet hardware (the
//!   master M5 30× world-scale load floor). The PROPERTY (R4 promotes only above the measured budget,
//!   serves the same leak-free paginated set, the fanout telemetry fires) is complete + testable now and
//!   does not change shape when the real PgStore-backed `edge` table + the read replica carry the load.
//! - **The flattened reach set is MODELLED in-memory here** ([`R4ReachIndex`]) — the §3.7 R4 component's
//!   semantics (derived from R1, incrementally maintained, gated by the same filter) byte-for-byte. The
//!   REAL R4 — a per-tenant-DEK-encrypted materialised reach table on the read replica, incrementally
//!   maintained off `refs.edge.*` — replaces this when the OLTP store is wired into `serve` (the seam
//!   shape — derive-from-R1, the same-filter conjoin, the measured-promotion gate — does not change).
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The leak-critical paths are (1) the SAME-filter admit on the R4 path (a mutant that drops the
//! `set_expr_admits` conjoin would leak a confidential referrer through R4 — the REF-P11 leak invariant
//! on the new path) and (2) the measured-promotion boundary (a mutant that flips `>` to `>=` or serves a
//! cold target would promote R4 by prediction, not measurement). Both are pinned by the unit + parity +
//! drill tests below: R4's admitted set EQUALS the CTE floor's (no leak through the flattened path), and
//! a target AT the budget serves from the CTE while one OVER it serves from R4 (the strict `>` boundary).

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

/// **R5 — the read-budget fanout above which R4 is promoted to serve a hot target (§6.3).** The seed
/// default-to-beat (1000): a `target_root` with MORE than this many live inbound edges is the
/// hot-artifact case the read-time CTE scan is measured to fall over its p99 budget for, so the
/// flattened reach index R4 is promoted to serve it. STRICTLY greater-than (a target AT the budget
/// still serves from the CTE floor). This MIRRORS the thresholds file `[refs_hot_artifact]
/// read_budget_fanout` row (the versioned source of truth, P-038) — a drill reads the budget from the
/// FILE through [`myelin_substrate::thresholds::Thresholds`], never this literal; this constant is the
/// seed the file is kept in lock-step with (a CDC test asserts the two agree). MEASURED-not-predicted:
/// the world-scale fleet re-measure of the real crossover is the named M5 floor.
pub const R4_READ_BUDGET_FANOUT: u64 = 1000;

/// **The measured-trigger verdict for an R4-considered read (§6.3 — promotion only on measured
/// hot-fanout).** Either the target's MEASURED inbound fanout exceeds the read budget and R4 serves the
/// flattened path, or it is at-or-under the budget and the read falls back to the CTE floor (R4 never
/// serves a cold target — no predicted promotion). Carries the measured fanout + the budget so the
/// drill reads the boundary off the verdict (the loud observability artifact).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum R4Verdict {
    /// The target's measured fanout EXCEEDS the read budget — R4 is promoted; it serves the flattened
    /// reach set (gated by the same `list_objects` filter). Carries `(measured_fanout, budget)`.
    ServeFromR4 {
        /// The target's MEASURED live-inbound fanout (the `hot_artifact_fanout` sample).
        measured_fanout: u64,
        /// The read budget R5 the fanout exceeded.
        budget: u64,
    },
    /// The target's measured fanout is at-or-under the read budget — R4 is NOT promoted for it; the
    /// read serves from the CTE floor ([`crate::backlinks::BacklinkRead`]). Carries
    /// `(measured_fanout, budget)` so the cold-path branch is observable.
    ServeFromCte {
        /// The target's MEASURED live-inbound fanout.
        measured_fanout: u64,
        /// The read budget R5 the fanout did NOT exceed.
        budget: u64,
    },
}

impl R4Verdict {
    /// `true` iff R4 is promoted for this read (the measured fanout exceeded the budget). A drill reads
    /// this to assert R4 served the hot target (and NOT a cold one).
    pub fn is_promoted(&self) -> bool {
        matches!(self, R4Verdict::ServeFromR4 { .. })
    }

    /// The MEASURED inbound fanout this verdict sampled (the `hot_artifact_fanout` value — the signal
    /// the measured-trigger keys on, contract 1.8).
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

/// One flattened reach entry — an inbound edge to a hot `target_root`, precomputed into R4's reach set
/// (§6.3 / the Leopard flattened reverse index). Holds exactly the fields the backlink read serves +
/// the `source_root` the `SetExpr` filters over (so R4's admit is the SAME conjoin as the CTE floor's).
/// References-not-payloads: every field is an opaque ref/token; `origin_actor` is the PSEUDONYMOUS
/// Principal ref (erasure-safe; never the name) — an R4 entry holds no PII the edge row does not.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReachEntry {
    /// The deterministic `edge_id` (the dedup key — an incremental upsert of the SAME edge is one
    /// flattened entry; mirrors R1's `ON CONFLICT` idempotency).
    edge_id: String,
    /// The flattened inbound edge as the backlink read serves it (the `#sub`-precise source, the
    /// `source_root` the filter targets, the rel/class/actor).
    backlink: Backlink,
}

/// The `(tenant, region)` partition key — every R4 read/write is tenant-first (the
/// no-cross-tenant-query-path floor; §3 / EI-02 §1). PII-free opaque partition tokens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PartKey {
    tenant: TenantId,
    region: Region,
}

/// `target_root` → the flattened, edge_id-sorted inbound reach set for that target (one Leopard
/// reverse-index bucket). Named so the [`R4ReachIndex`] field type stays legible.
type TargetReach = HashMap<String, Vec<ReachEntry>>;
/// `(tenant, region)` → the per-tenant flattened reach partition (tenant-first; no cross-tenant key).
type ReachMap = HashMap<PartKey, TargetReach>;

/// **The hot-artifact Leopard-style flattened reach index R4 (REF-P23; §6.3 / §3.7).** A cloneable
/// handle over shared, tenant-partitioned flattened reach state keyed `(tenant, region, target_root) →
/// [inbound reach entries]`, derived from R1, incrementally maintained from `refs.edge.*`, and SHARING
/// the REF-P11 [`AuthzVisibleIndex`] (so R4's `InRelation`/`TupleSet` admit reads the SAME reverse index
/// the CTE floor's JOIN does — one source of truth, no second permission state). R4 serves a target's
/// backlinks ONLY when its MEASURED fanout exceeds the read budget (the measured-trigger, §6.3).
#[derive(Clone)]
pub struct R4ReachIndex {
    /// `(tenant, region) → (target_root → [reach entries])` — the flattened inbound reach set (R4 is
    /// derived from R1; this is the materialised Leopard reverse index modelled in-memory). Tenant-
    /// first; no cross-tenant key.
    reach: Arc<Mutex<ReachMap>>,
    /// The SAME §4.4 `authz_visible` reverse index the CTE floor JOINs (REF-P11) — R4's
    /// `InRelation`/`TupleSet` admit reads THIS, so the R4 path and the CTE path admit the identical
    /// leak-free set. NOT a second authz state.
    authz: AuthzVisibleIndex,
    /// The read budget R5 (the measured-trigger fanout — read from the thresholds file by the caller,
    /// seeded to [`R4_READ_BUDGET_FANOUT`]). A target's measured fanout must EXCEED this for R4 to serve.
    read_budget_fanout: u64,
    /// The live `hot_artifact_fanout` telemetry sample (contract 1.8): the most-recent measured inbound
    /// fanout R4 considered (the signal the measured-trigger keys on; a drill reads it to assert the hot
    /// artifact was observably hot before promotion).
    last_fanout_sample: Arc<AtomicU64>,
    /// The count of reads R4 SERVED (the flattened path was taken — the measured-trigger fired). A drill
    /// reads this to assert R4 served the hot target (and the cold target did not bump it).
    r4_served_count: Arc<AtomicU64>,
}

impl R4ReachIndex {
    /// The `hot_artifact_fanout` telemetry signal name (contract 1.8 / §5.1). A named constant — drills
    /// assert against the NAME, never a literal (EI-01 §3 observability).
    pub const HOT_ARTIFACT_FANOUT_SIGNAL: &'static str = "refs.hot_artifact_fanout";

    /// Build an R4 reach index over the SAME [`AuthzVisibleIndex`] the CTE floor uses, with the read
    /// budget R5 (read from the thresholds file by the caller; pass [`R4_READ_BUDGET_FANOUT`] for the
    /// seed). The reach set starts empty — [`R4ReachIndex::rebuild_from_r1`] derives it from R1, or the
    /// incremental [`R4ReachIndex::on_edge_upsert`]/[`R4ReachIndex::on_edge_tombstone`] maintain it.
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

    /// **Derive R4 from R1 (§3.7 — R4 is rebuildable from R1, the source of truth).** Flattens every
    /// LIVE inbound edge to `target_root` in the `(tenant, region)` partition of `r1` into R4's reach
    /// set. This is the cold-build path (and the rebuild-after-wipe path): R4 is NEVER its own source of
    /// truth — it is recomputed from R1, so a drift/corruption is repaired by re-deriving. Idempotent:
    /// re-deriving the same R1 state yields the same flattened set (keyed on the deterministic
    /// `edge_id`). Tenant-first.
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

    /// **Incrementally maintain R4 on a live `refs.edge.*` upsert (§6.3 — incrementally maintained from
    /// `refs.edge.*`).** A created/revived edge to `target_root` is flattened into R4's reach set,
    /// idempotent on the deterministic `edge_id` (a redelivered upsert is one entry — mirrors R1's `ON
    /// CONFLICT`). This keeps R4 in lock-step with R1 in steady state (the SAME edge feeds both, so they
    /// cannot drift). A tombstoned row is NOT added (R4 holds only live reach — see
    /// [`R4ReachIndex::on_edge_tombstone`]). Tenant-first.
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
        // Idempotent on edge_id, and KEPT SORTED by edge_id at maintenance time (a binary search +
        // in-place insert) so a paginated read takes only the first `page` admitted entries by
        // iterating in order — it NEVER re-sorts (let alone re-materialises) the whole flattened set at
        // read time (the Leopard flattened index keeps its order; the read is bounded by `page`, not by
        // the fanout — this is what avoids the §6.3 "falls over" case). Replace an existing entry (the
        // §4.1 ON CONFLICT shape), else insert at the sorted position.
        match bucket.binary_search_by(|e| e.edge_id.cmp(&row.edge_id)) {
            Ok(pos) => bucket[pos] = entry,
            Err(pos) => bucket.insert(pos, entry),
        }
    }

    /// **Incrementally maintain R4 on a `refs.edge.*` removal/erasure (§6.3 / §4.6).** Drops the
    /// flattened entry for `edge_id` from `target_root`'s reach set — a tombstoned edge is hidden from
    /// R4 exactly as it is from R1's `edge_inbound WHERE NOT tombstoned` scan (so the R4 path and the
    /// CTE path serve the SAME live set). Idempotent: dropping an absent entry is a no-op (a redelivered
    /// removal never errors). Tenant-first.
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

    /// The MEASURED inbound fanout R4 holds for `target_root` (the flattened reach-set size) — the
    /// `hot_artifact_fanout` value the measured-trigger keys on. This is what R4 PROMOTES against: a
    /// target whose flattened fanout exceeds the read budget is the hot artifact R4 serves. Tenant-first.
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

    /// **The measured-trigger promotion verdict (§6.3 — promote only when measured hot-fanout exceeds
    /// the read budget, not predicted).** Samples `target_root`'s MEASURED fanout (emitting the
    /// `hot_artifact_fanout` telemetry) and decides: if it EXCEEDS the read budget R5, R4 is promoted
    /// ([`R4Verdict::ServeFromR4`]); else the read serves from the CTE floor
    /// ([`R4Verdict::ServeFromCte`]). The boundary is STRICTLY greater-than (a target AT the budget
    /// serves from the CTE — the §6.3 "exceeding the read budget" wording; ADR-10 measure-before-shard).
    /// This is the single place the predicted-vs-measured discipline is enforced: R4 NEVER serves a cold
    /// target.
    pub fn promotion_verdict(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> R4Verdict {
        let measured_fanout = self.measured_fanout(tenant, region, target_root);
        // Emit the hot_artifact_fanout telemetry (1.8) — the signal the measured-trigger keys on; a
        // hot target is observably hot BEFORE R4 is promoted for it.
        self.last_fanout_sample
            .store(measured_fanout, Ordering::SeqCst);
        // STRICTLY greater-than: a target AT the budget serves from the CTE floor, one OVER it promotes
        // R4 (the measured-trigger, never predicted).
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

    /// `true` iff R4 is promoted for `target_root` (its measured fanout exceeds the read budget) — the
    /// caller routes a backlink read to [`R4ReachIndex::backlinks`] iff this holds, else to the CTE
    /// floor. The measured-trigger gate in one predicate.
    pub fn is_promoted(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
    ) -> bool {
        self.promotion_verdict(tenant, region, target_root)
            .is_promoted()
    }

    /// **The R4-served permission-filtered backlink read (contract 5.3 at scale — the R4 path).** Serves
    /// `target_root`'s backlinks from the FLATTENED reach set, admitting ONLY the entries whose
    /// `source_root` the `viewer` may `view`, paginated — the IDENTICAL leak-free, paginated result the
    /// CTE floor ([`crate::backlinks::BacklinkRead::backlinks`]) returns, via a faster path. The admit is
    /// the SAME FROZEN [`set_expr_admits`] the CTE floor lowers (ONE permission algebra — the REF-P11
    /// leak invariant cannot regress on the R4 path). Returns the SAME [`BacklinkPage`] shape.
    ///
    /// **Promotion-gated:** this serves only a PROMOTED target (the caller checks [`R4ReachIndex::is_promoted`]
    /// / routes by [`R4ReachIndex::promotion_verdict`] first); calling it on a cold target is well-defined
    /// (it returns the flattened set R4 holds, which for a cold target is small/absent) but is NOT the
    /// measured-trigger path — the gate is the caller's, kept explicit so the cold-vs-hot routing is
    /// observable. `page > 0` (always paginated — a 0 page is a malformed request, never an unbounded
    /// scan). Tenant-first (no cross-tenant path).
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

        // The frozen list_objects shape → the SetExpr to admit over source_root + the filter-mode split
        // (1.8). The SAME enum the CTE floor lowers — ONE source of truth, no second algebra.
        let (set_expr, mode) = match list_objects {
            ListObjectsResult::Ids { ids, .. } => (SetExpr::Ids(ids.clone()), FilterMode::Ids),
            ListObjectsResult::Filter { set_expr, .. } => {
                (set_expr.clone(), FilterMode::PushedDown)
            }
        };

        // The flattened reach set for this target — the candidate inbound edges, already precomputed +
        // KEPT SORTED by edge_id at maintenance time (so this read NEVER re-sorts, let alone
        // re-materialises, the whole set — the §6.3 hot-path discipline: the read is bounded by `page`,
        // not by the fanout). The bucket is iterated UNDER THE LOCK and the filter + `take(page)` short-
        // circuits as soon as `page` admitted entries are collected — for a hot artifact with a large
        // admitted prefix the read touches only ~`page` rows, NEVER all 50,000 (this is what avoids the
        // "falls over" case; cloning + sorting the full bucket per read WOULD fall over). The order is
        // the deterministic edge_id order the CTE floor returns (`EdgeProjection::inbound_live` sorts by
        // edge_id) → the IDENTICAL paginated order: the parity the drill asserts.
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
                        // The SAME conjoin the CTE floor runs (set_expr_admits), reading the SHARED
                        // authz_visible reverse index for the InRelation/TupleSet forms. NOT a per-edge
                        // check; NOT a post-filter; the REF-P11 leak invariant, reused verbatim.
                        set_expr_admits(
                            &set_expr,
                            &self.authz,
                            viewer,
                            tenant,
                            region,
                            &e.backlink.source_root,
                        )
                    })
                    .take(page) // short-circuit at LIMIT — the read is bounded by `page`, not the fanout.
                    .map(|e| e.backlink.clone())
                    .collect()
            })
            .unwrap_or_default();
        drop(guard);

        Ok(BacklinkPage {
            edges: admitted,
            mode,
            // R4 reads the SHARED reverse index already at-revision (the projecting grant/revoke
            // advanced its watermark); the flattened path never serves a stale grant. The new-enemy
            // fall-back BRANCH is the CTE floor's concern (R4 is the post-promotion hot path over an
            // already-fresh index); R4 itself does not re-decide the watermark — it admits over the
            // SAME fresh reverse index. `fell_back_to_check = false` on the R4 path (the hot path is
            // never the stale-index path).
            fell_back_to_check: false,
        })
    }

    /// The shared `authz_visible` reverse index R4 admits against (the SAME one the CTE floor JOINs) —
    /// exposed so the caller that wires R4 into `serve` (and the drill/CDC tests) projects grants/revokes
    /// into it. There is ONE reverse index; R4 does not own a second.
    pub fn authz_index(&self) -> &AuthzVisibleIndex {
        &self.authz
    }

    /// The live `hot_artifact_fanout` telemetry sample (contract 1.8) — the most-recent measured inbound
    /// fanout R4 considered. A drill reads this to assert the hot artifact was observably hot (its fanout
    /// crossed the budget) before R4 was promoted for it.
    pub fn last_fanout_sample(&self) -> u64 {
        self.last_fanout_sample.load(Ordering::SeqCst)
    }

    /// The count of reads R4 SERVED (the flattened path taken). A drill reads this to assert R4 served
    /// the hot target.
    pub fn r4_served_count(&self) -> u64 {
        self.r4_served_count.load(Ordering::SeqCst)
    }

    /// The read budget R5 this index promotes against (read from the thresholds file at construction).
    pub fn read_budget_fanout(&self) -> u64 {
        self.read_budget_fanout
    }

    /// Flatten one R1 edge row into an R4 reach entry (the §6.3 derivation — R4 holds exactly the
    /// backlink the read serves + its dedup key).
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
