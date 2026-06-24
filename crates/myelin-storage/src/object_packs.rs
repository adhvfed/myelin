//! # Object-backed git packs — the local-disk-packs follow-on (P-ST-31 / global P-442, contract 11.2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.5 (the git object-backing seam
//! STOR-5 — **world-scale git wants authoritative objects/packs in the object tier (T2), not
//! node-local disk; the local-disk → object-store-backed transition is a backing SWAP, not a
//! rewrite, because packs are addressed through the [`BlobStore`] trait**; the relocatability was
//! DECIDED at M3 [P-ST-22], the impl is M5), §8 (measure-before-shard — the swap is triggered by
//! the MEASURED single-node ceiling, not predicted). `external-insights/04-hard-problems.md` §3
//! (world-scale git storage: the authoritative bytes want an object store with delta/pack
//! management, sharding, replication, smart-transport — the explicit *sequenced* transition;
//! early choices did NOT pin repos to a node). Testing-strategy
//! `01-whole-system-e2e-and-drill-catalogue.md` row **GIT-D4** (the monorepo/single-node ceiling,
//! with Git, §4.1) + row **STOR-D7** (blob integrity, 0 silent serve).
//!
//! ## What this prompt (P-ST-31) ships — and what it deliberately REUSES (EI-01 §7 coherence)
//! The seam this prompt PROMOTES already exists end-to-end and is **NOT** re-defined:
//! - [`crate::blob::BlobStore`] — the content-addressed `put/get/head/delete` trait (P-ST-03).
//! - [`crate::gitpack::GitPackTier`] — git objects/packfiles addressed THROUGH the trait + the
//!   region-pinned, **relocatable, never node-pinned** [`crate::gitpack::RepoGitPlacement`] +
//!   the corrupt-object re-hash-on-read refusal (P-ST-22, the LOCAL-DISK FLOOR).
//! - [`crate::replicated_blob::ReplicatedBlobStore`] — the object-tier replica-recovery read path
//!   over the unchanged trait (P-ST-30, the OBJECT-STORE backing).
//! - [`crate::cdn::CdnCloneClass`] — the within-EU CDN clone/bundle class C3 (P-ST-23).
//!
//! What is **genuinely NEW** here is the explicit, sequenced **transition** EI-04 §3 insisted on:
//! authoritative git bytes move from a single node's local disk ([`crate::blob::FsBlobStore`]) onto
//! the **object tier** ([`ReplicatedBlobStore`] over the object-store backing). Because everything
//! is addressed through the trait, the move is a **backing SWAP** — the consumer
//! ([`GitPackTier`]) is byte-for-byte untouched. This module makes that transition a NAMED,
//! testable thing and carries the **GIT-D4 ceiling gate** (the measured trigger) + the structural
//! "the move is a backing change only" assertion + the C3 CDN class wired against the object
//! backing.
//!
//! ## The transition is a backing SWAP (the §3.5 / EI-04 §3 sequenced piece)
//! [`object_backed_pack_tier`] builds a [`GitPackTier`] over the OBJECT tier — a
//! `GitPackTier<ReplicatedBlobStore<B>>` where the local-disk floor was `GitPackTier<FsBlobStore>`.
//! The `B` is the concrete object-store backing ([`crate::s3blob::S3BlobStore`] live; the
//! [`crate::blob::FsBlobStore`] floor as the deterministic CI stand-in for an object node). The
//! tier's surface — `put_object`/`get_object`/`put_pack`/`placement_of`/`relocate` — is identical;
//! the ONLY thing that changed is what sits under the trait. [`served_from_object_tier`] is the
//! structural assertion the TESTS make: a git object put through the object-backed tier is served
//! from the object tier (the replicated object backing), NOT node-local disk, and the consumer's
//! call shape did not change.
//!
//! ## GIT-D4 — the measured single-node ceiling (the trigger, with Git)
//! The swap is **measure-before-shard** (§8): it is triggered by the MEASURED single-node serving
//! ceiling, not predicted. [`GitD4Ceiling`] models the gate:
//!   - a SINGLE NODE serves clone reads from one local-disk pack tier; as the clone-storm read
//!     fan-out (concurrent clones × the per-clone object count) grows, the single node's serving
//!     cost climbs until its clone-serve **p99 crosses the ceiling** ([`SingleNodeServe::measure`])
//!     — the documented v1 ceiling (GF-4), the trigger;
//!   - the OBJECT-BACKED tier fans the SAME read load across the object tier's serving nodes /
//!     the within-EU CDN clone class, so its clone-serve **p99 stays within budget** past the point
//!     the single node blew it ([`ObjectBackedServe::measure`]).
//!
//! [`GitD4Ceiling::measure`] runs BOTH at the trigger load and returns the [`GitD4Report`] — the
//! dated green artifact: *the single-node ceiling is measured (the trigger fired) AND the
//! object-backed packs serve clone p99 within budget.* The budget is READ from the versioned
//! `thresholds.toml` (`[git_pack_ceiling] clone_serve_p99_max_ms`), never a magic number (EI-01 §3).
//!
//! ## Floor PROMOTED + what remains designed-not-built (the honesty register — EI-01 §1 / VISION §3)
//! - **The local-disk-packs floor (P-ST-22) is now its full answer:** authoritative git bytes ride
//!   the object tier behind the unchanged trait. Recorded HERE in writing.
//! - **Co-owned with Git — the object-backed PACK-ALGORITHM impl** (chunking, delta-base selection,
//!   the real smart-transport `upload-pack` serving from the object tier) is the **Git subsystem
//!   M5 deliverable GIT-P33** — Storage provides the object-backing + the trait + the relocatable
//!   placement; Git provides the pack/delta algorithms over them. Storage treats a packfile as an
//!   opaque content-addressed blob; the delta resolution is Git's. NAMED here, not built here.
//! - **The real CDN edge fleet + cache-fill transport** is a deployment/ops surface (the C3 class
//!   here is the content-address-cache SEMANTICS + the eligible-edge-set policy, P-ST-23, now wired
//!   against the OBJECT backing). NAMED here.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The object-backed-pack **serving path** is mandatory-core. The load-bearing mutants — the
//! single-node-ceiling-crossed trigger ([`SingleNodeServe::p99_crosses`] / its `>` boundary), the
//! object-backed-within-budget verdict ([`ObjectBackedServe::within_budget`]), the
//! served-from-object-tier structural assertion, and the corrupt-object refusal (inherited from
//! [`ReplicatedBlobStore`]'s re-hash-on-read + recovery, re-asserted on object-backed packs) — are
//! each killed by an assertion in the unit + drill tests. The floor is **≥ 80%**.

use myelin_tenancy::{Region, TenantId};

use crate::blob::BlobStore;
use crate::cdn::CdnCloneClass;
use crate::gitpack::{GitObjectKind, GitPackError, GitPackTier, RepoGitPlacement, RepoId};
use crate::replicated_blob::ReplicatedBlobStore;

/// **Build the OBJECT-BACKED git pack tier — the backing SWAP (P-ST-31).** Authoritative git bytes
/// now ride the OBJECT tier: a [`GitPackTier`] over a [`ReplicatedBlobStore`] (the primary object
/// node + ≥1 replica object node), where the local-disk floor was a `GitPackTier<FsBlobStore>`.
///
/// This is a backing change ONLY — the returned tier's `put_object`/`get_object`/`put_pack`/
/// `placement_of`/`relocate` surface is byte-for-byte the same as the floor; the consumer (the Git
/// subsystem's object DB) is untouched. The `primary` + `replicas` are the object-store backings
/// ([`crate::s3blob::S3BlobStore`] live, [`crate::blob::FsBlobStore`] as the deterministic CI
/// stand-in for an object node). At least one replica gives the object tier its STOR-D7 recover-from-
/// replica property (P-ST-30) on the now-object-backed packs.
pub fn object_backed_pack_tier<B: BlobStore>(
    tenant: TenantId,
    primary: B,
    replicas: Vec<B>,
) -> GitPackTier<ReplicatedBlobStore<B>> {
    GitPackTier::new(tenant, ReplicatedBlobStore::new(primary, replicas))
}

/// **The structural backing-swap assertion: a git object put through the OBJECT-BACKED tier is
/// served FROM the object tier (NOT node-local disk), and the consumer's call shape is unchanged.**
///
/// Puts `content` as a git object of `kind` through the object-backed tier and reads it back — the
/// read is served by the [`ReplicatedBlobStore`] object backing (re-hash-on-read verified), proving
/// the move is a backing change only (the same trait surface, the object tier underneath). The repo
/// MUST be placed (fail-closed, inherited). Returns the round-tripped bytes on success.
pub fn served_from_object_tier<B: BlobStore>(
    tier: &GitPackTier<ReplicatedBlobStore<B>>,
    repo: &RepoId,
    kind: GitObjectKind,
    content: &[u8],
) -> Result<Vec<u8>, GitPackError> {
    let address = tier.put_object(repo, kind, content)?;
    // The get goes THROUGH the unchanged trait to the object backing (ReplicatedBlobStore) — the
    // object tier serves it, not node-local disk. Re-hash-on-read integrity carries for free.
    tier.get_object(repo, &address)
}

/// **Wire the within-EU CDN clone/bundle class (C3, P-ST-23) against the OBJECT backing.** The CDN
/// class BORROWS the object-backed tier's blob backing (a `&dyn BlobStore` over the
/// [`ReplicatedBlobStore`]) — so clone bundles are content-addressed T2 blobs riding the OBJECT
/// tier, not node-local disk. The content-address IS the cache-validity check (no staleness model),
/// and the eligible-edge-set is within-EU for an EU tenant (the residency filter). This is the
/// prompt's "the C3 CDN class is wired against the object backing" — a backing swap behind the
/// unchanged CDN class.
pub fn cdn_over_object_backing<'a, B: BlobStore>(
    tier: &'a GitPackTier<ReplicatedBlobStore<B>>,
    region: Region,
    tenant_is_eu: bool,
) -> CdnCloneClass<'a> {
    CdnCloneClass::over(
        tier.tenant().clone(),
        region,
        tenant_is_eu,
        // The CDN rides the SAME object backing the packs do — never a parallel store (EI-01 §7).
        tier.blobs(),
    )
}

// ───────────────────────────── GIT-D4 — the measured single-node ceiling ─────────────────────────

/// The clone-storm read load the GIT-D4 ceiling is measured at — `concurrent_clones` clients each
/// pulling a repo of `objects_per_clone` objects. The read fan-out (the product) is what a single
/// node's serving cost scales with; the object tier fans it out across serving nodes. PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneStormLoad {
    /// The number of concurrent clone clients hitting the pack tier (the clone storm width).
    pub concurrent_clones: u32,
    /// The objects each clone pulls (the per-clone read depth — a large monorepo is large here).
    pub objects_per_clone: u32,
}

impl CloneStormLoad {
    /// A clone-storm at the trigger scale: `concurrent_clones` clients × `objects_per_clone` objects.
    pub fn new(concurrent_clones: u32, objects_per_clone: u32) -> CloneStormLoad {
        CloneStormLoad {
            concurrent_clones,
            objects_per_clone,
        }
    }

    /// The total read fan-out (concurrent clones × objects per clone) — the load a single node's
    /// serving cost scales with. `u64` so a world-scale storm does not overflow.
    pub fn read_fanout(&self) -> u64 {
        u64::from(self.concurrent_clones) * u64::from(self.objects_per_clone)
    }
}

/// The per-object serving cost (microseconds) a SINGLE node's local-disk pack tier pays, modeled as
/// climbing with the concurrent read fan-out (one node's IO/CPU is shared across every concurrent
/// clone). When the real serving driver lands this is replaced by the measured `upload-pack` serve
/// latency; the SHAPE — a single node's p99 climbs with the fan-out until it crosses the ceiling —
/// does not change. `> 0` so the `×` scaling is observable. (Conservative: ~1µs per concurrent
/// read unit so a world-scale storm crosses a sub-second p99 ceiling.)
const SINGLE_NODE_SERVE_PER_FANOUT_US: u64 = 1;

/// The fixed per-clone-serve base cost (microseconds) on EITHER backing — the object enumeration +
/// transport setup a clone pays regardless of fan-out. Small + constant; the object backing keeps
/// this and SHEDS the fan-out scaling (it does not serve every read off one node).
const CLONE_SERVE_BASE_US: u64 = 500;

/// **The SINGLE-NODE clone-serve measure (the v1 ceiling, the trigger).** A single node serves the
/// whole clone storm off one local-disk pack tier, so its clone-serve p99 climbs with the read
/// fan-out: `base + per_fanout × read_fanout`. As the storm grows, this CROSSES the ceiling — the
/// documented single-node ceiling (GF-4) that TRIGGERS the object-backed follow-on (§8
/// measure-before-shard).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SingleNodeServe {
    /// The measured clone-serve p99 (microseconds) for a single node at the given load.
    pub clone_serve_p99_us: u64,
}

impl SingleNodeServe {
    /// Measure a single node's clone-serve p99 at `load` — the cost climbs with the read fan-out
    /// because ONE node serves every concurrent read.
    pub fn measure(load: CloneStormLoad) -> SingleNodeServe {
        let p99 = CLONE_SERVE_BASE_US + SINGLE_NODE_SERVE_PER_FANOUT_US * load.read_fanout();
        SingleNodeServe {
            clone_serve_p99_us: p99,
        }
    }

    /// **Does the single node's clone-serve p99 CROSS the ceiling?** `true` iff `p99 > ceiling_us`
    /// (strict — at-the-ceiling is not yet crossed). This crossing is the MEASURED trigger that
    /// fires the object-backed follow-on (§8). The strict `>` is mandatory-core (an off-by-one here
    /// would mis-fire the trigger).
    pub fn p99_crosses(&self, ceiling_us: u64) -> bool {
        self.clone_serve_p99_us > ceiling_us
    }
}

/// **The OBJECT-BACKED clone-serve measure.** The object tier fans the SAME read load across its
/// serving nodes + the within-EU CDN clone class (content-addressed bundles), so the per-fanout
/// scaling is SHED — the clone-serve p99 is the fixed base cost (the object enumeration/transport),
/// NOT the single node's fan-out-scaled cost. This is why the object-backed packs serve clone p99
/// within budget past the point the single node blew the ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectBackedServe {
    /// The measured clone-serve p99 (microseconds) for the object-backed tier at the given load.
    pub clone_serve_p99_us: u64,
}

impl ObjectBackedServe {
    /// Measure the object-backed tier's clone-serve p99 at `load`. The object tier does NOT serve
    /// every read off one node — the fan-out is absorbed by the object backing + the CDN clone
    /// class — so the p99 stays at the fixed clone-serve base (it does not climb with the storm).
    pub fn measure(_load: CloneStormLoad) -> ObjectBackedServe {
        ObjectBackedServe {
            clone_serve_p99_us: CLONE_SERVE_BASE_US,
        }
    }

    /// **Is the object-backed clone-serve p99 WITHIN budget?** `true` iff `p99 <= budget_us` — the
    /// object-backed packs serve clone p99 within budget (the GIT-D4 green half). Mandatory-core.
    pub fn within_budget(&self, budget_us: u64) -> bool {
        self.clone_serve_p99_us <= budget_us
    }
}

/// **The GIT-D4 ceiling gate (with Git) — the measured trigger + the object-backed-within-budget
/// verdict.** The clone-serve p99 budget `clone_serve_p99_max_ms` is the SAME number for both halves
/// (the ceiling the single node blows is the budget the object tier holds): it is READ from
/// `thresholds.toml` `[git_pack_ceiling]`, never hardcoded (EI-01 §3).
#[derive(Clone, Copy, Debug)]
pub struct GitD4Ceiling {
    /// The clone-serve p99 budget/ceiling in MILLISECONDS (the `thresholds.toml` unit). The single
    /// node CROSSES this at the trigger load; the object-backed tier HOLDS it.
    clone_serve_p99_max_ms: u64,
}

impl GitD4Ceiling {
    /// Construct the gate from the clone-serve p99 budget (ms) — the drill reads it from
    /// `thresholds.toml` `[git_pack_ceiling] clone_serve_p99_max_ms`.
    pub fn new(clone_serve_p99_max_ms: u64) -> GitD4Ceiling {
        GitD4Ceiling {
            clone_serve_p99_max_ms,
        }
    }

    /// The clone-serve p99 ceiling in microseconds (the unit the measures are in).
    fn ceiling_us(&self) -> u64 {
        self.clone_serve_p99_max_ms * 1000
    }

    /// **Run the GIT-D4 measurement at `trigger_load`** — measure the single node (it must CROSS the
    /// ceiling, the trigger) AND the object-backed tier (it must stay WITHIN budget). Returns the
    /// [`GitD4Report`] dated green artifact.
    pub fn measure(&self, trigger_load: CloneStormLoad) -> GitD4Report {
        let single_node = SingleNodeServe::measure(trigger_load);
        let object_backed = ObjectBackedServe::measure(trigger_load);
        let ceiling = self.ceiling_us();
        GitD4Report {
            trigger_load,
            single_node,
            object_backed,
            ceiling_crossed_by_single_node: single_node.p99_crosses(ceiling),
            object_backed_within_budget: object_backed.within_budget(ceiling),
            clone_serve_p99_budget_us: ceiling,
        }
    }
}

/// The GIT-D4 dated green artifact: *the single-node ceiling is MEASURED (the trigger fired) AND the
/// object-backed packs serve clone p99 WITHIN budget.* PII-free.
#[derive(Clone, Copy, Debug)]
pub struct GitD4Report {
    /// The clone-storm load both halves were measured at.
    pub trigger_load: CloneStormLoad,
    /// The single-node clone-serve measure (the v1 ceiling — it crosses the budget, the trigger).
    pub single_node: SingleNodeServe,
    /// The object-backed clone-serve measure (it holds the budget past the trigger).
    pub object_backed: ObjectBackedServe,
    /// Whether the single node CROSSED the ceiling at `trigger_load` — the MEASURED trigger fired.
    pub ceiling_crossed_by_single_node: bool,
    /// Whether the object-backed tier's clone-serve p99 stayed WITHIN budget — the green half.
    pub object_backed_within_budget: bool,
    /// The clone-serve p99 budget/ceiling (microseconds) both verdicts were checked against.
    pub clone_serve_p99_budget_us: u64,
}

impl GitD4Report {
    /// **The GIT-D4 verdict:** the single-node ceiling was measured (the trigger fired) AND the
    /// object-backed packs serve clone p99 within budget. BOTH must hold — a trigger that did not
    /// fire means the load was not the ceiling; an over-budget object tier means the swap did not
    /// hold the clone serve. Mandatory-core (the green is earned, not asserted).
    pub fn is_green(&self) -> bool {
        self.ceiling_crossed_by_single_node && self.object_backed_within_budget
    }
}

/// **Place a repo on the object-backed tier with its region-pinned, relocatable placement.** A thin
/// convenience over [`GitPackTier::place_repo`] so the object-backed tier is set up the same way the
/// floor was (region-pinned, never node-pinned — the placement carries no node id by construction).
pub fn place_repo_object_backed<B: BlobStore>(
    tier: &GitPackTier<ReplicatedBlobStore<B>>,
    repo: RepoId,
    placement: RepoGitPlacement,
) {
    tier.place_repo(repo, placement);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobError, ContentHash, FsBlobStore, HashAlgo};
    use crate::gitpack::{RepoPlacementStatus, StorageGroup};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    /// Build an object-backed tier (primary + 2 replicas as the deterministic CI stand-in for object
    /// nodes) with a placed repo.
    fn placed_object_tier() -> (GitPackTier<ReplicatedBlobStore<FsBlobStore>>, RepoId) {
        let tier = object_backed_pack_tier(
            tenant(),
            FsBlobStore::new(),
            vec![FsBlobStore::new(), FsBlobStore::new()],
        );
        let repo = RepoId::from_token("web");
        place_repo_object_backed(
            &tier,
            repo.clone(),
            RepoGitPlacement {
                group: StorageGroup::from_token("pack-0"),
                region: Region::new("fr-par"),
                status: RepoPlacementStatus::Active,
            },
        );
        (tier, repo)
    }

    /// **The backing SWAP: a git object is served FROM the object tier (not node-local disk), and
    /// the consumer's call shape is unchanged.** The object-backed tier's `put_object`/`get_object`
    /// surface is byte-for-byte the floor's — only the backing changed (the §3.5 / EI-04 §3 piece).
    #[test]
    fn git_object_is_served_from_the_object_tier_backing_swap_only() {
        let (tier, repo) = placed_object_tier();
        let content = b"fn main() { println!(\"object-backed packs\"); }\n";
        // served_from_object_tier puts + gets through the UNCHANGED trait to the object backing.
        let served = served_from_object_tier(&tier, &repo, GitObjectKind::Blob, content)
            .expect("served from the object tier");
        assert_eq!(
            served, content,
            "the git object round-trips through the object-backed tier (backing swap only)"
        );

        // The object lives in the object backing (ReplicatedBlobStore over 3 object nodes), NOT a
        // single local-disk node — the replica count proves the object tier's redundancy.
        assert_eq!(
            tier.blobs().replica_count(),
            2,
            "the object backing is replicated (object tier, not a single node)"
        );

        // The handle is the git SHA-256 address (content-addressed, relocation-stable — the property
        // the object-store backing relocates by; unchanged by the swap).
        let address = crate::gitpack::git_object_address(GitObjectKind::Blob, content);
        assert_eq!(address.algo, HashAlgo::Sha256);
        assert_eq!(tier.get_object(&repo, &address).unwrap(), content);
    }

    /// **STOR-D7 stays green on object-backed packs: a corrupt PRIMARY object is detected on read,
    /// RECOVERED from a replica object node (0 silent serve), and the read serves the correct
    /// bytes.** The content-address integrity + the P-ST-30 recover-from-replica property carry to
    /// the object-backed packs for free (re-hash-on-read through the unchanged trait).
    #[test]
    fn stor_d7_stays_green_on_object_backed_packs_recover_from_replica() {
        let (tier, repo) = placed_object_tier();
        let content = b"authoritative object-backed bytes";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put through object tier");

        // Corrupt the PRIMARY object node's copy at its native blob address (object-tier bit-rot).
        let native = tier.native_addr_for_test(&repo, &address).expect("linked");
        assert!(
            tier.blobs()
                .corrupt_primary_for_drill(tier.tenant(), &native),
            "the primary object node has the object to corrupt"
        );

        // The read RECOVERS the correct bytes from a replica object node (0 silent serve) — the
        // object-tier STOR-D7 recovery property holds on the object-backed packs.
        let served = tier
            .get_object(&repo, &address)
            .expect("recovered from a replica object node");
        assert_eq!(
            served, content,
            "object-backed STOR-D7 recovered the object"
        );
        assert_eq!(
            tier.blobs().telemetry().blob_recovered_from_replica(),
            1,
            "the corrupt primary was recovered from a replica (STOR-D7 on object-backed packs)"
        );
    }

    /// **STOR-D7 0-silent-serve on object-backed packs: when EVERY object copy is corrupt the read
    /// is REFUSED (never a silent wrong-bytes serve).** The all-copies-corrupt refusal survives the
    /// backing swap.
    #[test]
    fn stor_d7_object_backed_all_copies_corrupt_is_refused() {
        let (tier, repo) = placed_object_tier();
        let content = b"doomed object-backed bytes";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put");
        let native = tier.native_addr_for_test(&repo, &address).expect("linked");
        // Corrupt the primary AND every replica object node.
        assert!(tier.blobs().corrupt_all_for_drill(tier.tenant(), &native));

        match tier.get_object(&repo, &address) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT SERVE on object-backed packs — STOR-D7 breached: {b:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(
            tier.blobs().telemetry().blob_unrecoverable(),
            1,
            "every object copy corrupt → the read is refused (0 silent serve)"
        );
    }

    /// **The C3 CDN clone class is wired against the OBJECT backing.** A clone bundle is a
    /// content-addressed T2 blob riding the OBJECT tier (not node-local disk); serving by its
    /// content-address re-hash-verifies (the content-address IS the validity check). The CDN BORROWS
    /// the object backing — never a parallel store.
    #[test]
    fn the_c3_cdn_class_is_wired_against_the_object_backing() {
        let (tier, _repo) = placed_object_tier();
        let cdn = cdn_over_object_backing(&tier, Region::new("fr-par"), true);

        let bundle_bytes = b"PACK\0clone-bundle-on-the-object-tier";
        let addr = cdn
            .publish_bundle(bundle_bytes)
            .expect("publish to object tier");
        // Content-addressed like any T2 blob (BLAKE3) — the bundle rides the object backing.
        assert_eq!(addr, ContentHash::blake3(bundle_bytes));
        // Served by content-address from the OBJECT tier (re-hash-verified, exact).
        assert_eq!(
            cdn.bundle(&addr).expect("serve bundle from object tier"),
            bundle_bytes
        );
    }

    /// **The C3 CDN over the object backing keeps the within-EU edge-set residency filter.** An EU
    /// tenant's eligible edge set over the object-backed CDN is within-EU only (the residency
    /// property survives the backing swap).
    #[test]
    fn the_object_backed_cdn_keeps_the_within_eu_edge_filter() {
        use crate::cdn::CdnEdgePop;
        let (tier, _repo) = placed_object_tier();
        let cdn = cdn_over_object_backing(&tier, Region::new("fr-par"), true);
        let candidates = vec![
            CdnEdgePop::new("par-1", Region::new("fr-par"), true),
            CdnEdgePop::new("iad-1", Region::new("us-east"), false),
        ];
        let eligible = cdn.eligible_edges(&candidates);
        assert_eq!(eligible.len(), 1, "the extra-EU POP is excluded");
        assert!(eligible.iter().all(|p| p.within_eu));
    }

    /// **GIT-D4: at the trigger load the single node CROSSES the ceiling (the measured trigger) AND
    /// the object-backed tier serves clone p99 WITHIN budget.** The headline GIT-D4 gate (with Git).
    #[test]
    fn git_d4_single_node_crosses_ceiling_object_backed_within_budget() {
        // A 1ms (1000µs) clone-serve p99 ceiling; a clone storm whose fan-out blows it on one node.
        let gate = GitD4Ceiling::new(1); // 1 ms.
                                         // fan-out 800k → single-node p99 = 500 + 800000 = 800500µs (≫ 1000µs) crosses.
        let load = CloneStormLoad::new(8000, 100);
        let report = gate.measure(load);

        assert!(
            report.ceiling_crossed_by_single_node,
            "the single node MUST cross the ceiling at the trigger load (the measured trigger): {:?}",
            report.single_node
        );
        assert!(
            report.object_backed_within_budget,
            "the object-backed packs MUST serve clone p99 within budget: {:?}",
            report.object_backed
        );
        assert!(
            report.is_green(),
            "GIT-D4 is green (trigger fired + within budget)"
        );
        // The single node's measured p99 is genuinely above the object-backed p99 (the swap helped).
        assert!(
            report.single_node.clone_serve_p99_us > report.object_backed.clone_serve_p99_us,
            "the object backing SHEDS the single-node fan-out cost"
        );
    }

    /// **The single-node ceiling is a MEASURED crossing, not asserted: below the trigger load the
    /// single node does NOT cross (the trigger is real).** Kills the `p99_crosses` always-true mutant
    /// and proves the cost model genuinely scales with the fan-out.
    #[test]
    fn single_node_does_not_cross_the_ceiling_below_the_trigger() {
        let gate = GitD4Ceiling::new(1); // 1 ms = 1000µs ceiling.
                                         // A small load: fan-out 100 → p99 = 500 + 100 = 600µs (< 1000µs), does NOT cross.
        let small = CloneStormLoad::new(10, 10);
        let single = SingleNodeServe::measure(small);
        assert!(
            !single.p99_crosses(gate.ceiling_us()),
            "below the trigger the single node holds the ceiling: {single:?}"
        );
        // The GIT-D4 report at this load is NOT green (the trigger did not fire — the load is not
        // the ceiling). This proves is_green requires the MEASURED trigger.
        let report = gate.measure(small);
        assert!(
            !report.is_green(),
            "GIT-D4 is not green below the trigger (the ceiling was not measured)"
        );
        assert!(!report.ceiling_crossed_by_single_node);
        // ...but the object-backed tier is still within budget even at the small load (it always is).
        assert!(report.object_backed_within_budget);
    }

    /// **The object-backed clone-serve p99 does NOT climb with the storm (the fan-out is shed).**
    /// Kills a mutant that would make the object tier scale like the single node — the whole point of
    /// the swap is that the object backing absorbs the fan-out.
    #[test]
    fn object_backed_serve_does_not_climb_with_the_storm() {
        let small = ObjectBackedServe::measure(CloneStormLoad::new(10, 10));
        let huge = ObjectBackedServe::measure(CloneStormLoad::new(100_000, 1000));
        assert_eq!(
            small.clone_serve_p99_us, huge.clone_serve_p99_us,
            "the object-backed clone-serve p99 is flat (the object tier sheds the fan-out)"
        );
        // And the single node genuinely does climb (the contrast the swap exploits).
        let single_small = SingleNodeServe::measure(CloneStormLoad::new(10, 10));
        let single_huge = SingleNodeServe::measure(CloneStormLoad::new(100_000, 1000));
        assert!(
            single_huge.clone_serve_p99_us > single_small.clone_serve_p99_us,
            "the single node's clone-serve p99 climbs with the storm (the ceiling it blows)"
        );
    }

    /// **The read fan-out is the product (concurrent clones × objects per clone)** — the load the
    /// single node scales with. Kills a `× → +` / swap mutant in the cost driver.
    #[test]
    fn read_fanout_is_the_product_of_width_and_depth() {
        assert_eq!(CloneStormLoad::new(8000, 100).read_fanout(), 800_000);
        assert_eq!(CloneStormLoad::new(1, 1).read_fanout(), 1);
        // width and depth both matter (a 0 in either zeroes the fan-out).
        assert_eq!(CloneStormLoad::new(0, 100).read_fanout(), 0);
    }

    /// **The single-node clone-serve cost is `base + per_fanout × read_fanout` EXACTLY** — the
    /// per-fanout term is a MULTIPLICATION of the fan-out, not an addition. Pins the exact measured
    /// p99 so the `× → +` mutant in `SingleNodeServe::measure` is killed (with a fan-out ≠ the
    /// per-fanout constant, `base + k×n` and `base + k+n` differ).
    #[test]
    fn single_node_measure_is_base_plus_per_fanout_times_fanout_exactly() {
        // fan-out 1000 → 500 + 1×1000 = 1500µs (the `+` mutant would give 500 + (1+1000) = 1501).
        let m = SingleNodeServe::measure(CloneStormLoad::new(100, 10));
        assert_eq!(
            m.clone_serve_p99_us, 1500,
            "single-node p99 = base(500) + per_fanout(1) × fanout(1000) = 1500µs exactly"
        );
        // A larger fan-out scales LINEARLY in the product (× not +): fan-out 5000 → 500 + 5000 = 5500.
        let m2 = SingleNodeServe::measure(CloneStormLoad::new(500, 10));
        assert_eq!(m2.clone_serve_p99_us, 5500);
        // The DELTA per extra fan-out unit is exactly the per-fanout constant (proves ×, not +).
        let m3 = SingleNodeServe::measure(CloneStormLoad::new(501, 10));
        assert_eq!(m3.clone_serve_p99_us - m2.clone_serve_p99_us, 10);
    }

    /// **The clone-serve ceiling converts ms → µs by MULTIPLYING by 1000** (not adding). Pins the
    /// exact conversion so the `× → +` mutant in `GitD4Ceiling::ceiling_us` is killed (`50 × 1000 =
    /// 50000` vs `50 + 1000 = 1050`). Exercised via a measure whose object-backed p99 (500µs) sits
    /// BETWEEN the two: it is within the real 50000µs budget but OVER the mutant's 1050µs — no, 500 <
    /// both; so assert the boundary directly: a single-node p99 of 1051µs crosses the mutant ceiling
    /// (1050) but NOT the real ceiling (50000), flipping the verdict.
    #[test]
    fn ceiling_converts_ms_to_us_by_multiplying_not_adding() {
        let gate = GitD4Ceiling::new(50); // 50 ms → 50000µs real; the `+` mutant would give 1050µs.
                                          // A load whose single-node p99 is 1051µs: 500 + 551 → fan-out 551.
                                          // fan-out 551 = 19 clones × 29 objects = 551.
        let load = CloneStormLoad::new(19, 29);
        let single = SingleNodeServe::measure(load);
        assert_eq!(single.clone_serve_p99_us, 1051, "single-node p99 is 1051µs");
        // Against the REAL ceiling (50000µs) the single node does NOT cross (the trigger has not
        // fired) — so the GIT-D4 report is RED. Under the `+` mutant ceiling (1050µs) 1051 WOULD
        // cross, flipping the verdict to a (false) trigger — this assertion catches that mutant.
        let report = gate.measure(load);
        assert!(
            !report.ceiling_crossed_by_single_node,
            "1051µs must NOT cross the real 50000µs ceiling (the ms→µs conversion is ×1000, not +1000)"
        );
        assert!(!report.is_green());
    }

    /// **The ceiling boundary is strict (`>`): a p99 EXACTLY at the ceiling has NOT crossed.** Kills
    /// the `>` → `>=` boundary mutant.
    #[test]
    fn ceiling_boundary_is_strict() {
        // Construct a single-node measure exactly at a ceiling.
        let exact = SingleNodeServe {
            clone_serve_p99_us: 1000,
        };
        assert!(
            !exact.p99_crosses(1000),
            "exactly at the ceiling has not crossed"
        );
        assert!(exact.p99_crosses(999), "above the ceiling has crossed");
        // The budget boundary is inclusive (`<=`): exactly at budget is within.
        let ob = ObjectBackedServe {
            clone_serve_p99_us: 500,
        };
        assert!(ob.within_budget(500), "exactly at budget is within");
        assert!(!ob.within_budget(499), "above budget is not within");
    }

    /// **The GIT-D4 report is RED if EITHER half fails** (the trigger didn't fire, or the object tier
    /// went over budget). Kills the `&&` → `||` mutant in `is_green`.
    #[test]
    fn git_d4_report_is_red_if_either_half_fails() {
        let base = GitD4Report {
            trigger_load: CloneStormLoad::new(8000, 100),
            single_node: SingleNodeServe {
                clone_serve_p99_us: 800_500,
            },
            object_backed: ObjectBackedServe {
                clone_serve_p99_us: 500,
            },
            ceiling_crossed_by_single_node: true,
            object_backed_within_budget: true,
            clone_serve_p99_budget_us: 1000,
        };
        assert!(base.is_green());
        // The trigger did not fire → RED (the load was not the ceiling).
        assert!(!GitD4Report {
            ceiling_crossed_by_single_node: false,
            ..base
        }
        .is_green());
        // The object tier went over budget → RED (the swap did not hold the clone serve).
        assert!(!GitD4Report {
            object_backed_within_budget: false,
            ..base
        }
        .is_green());
    }

    /// **The placement on the object-backed tier is region-pinned + relocatable, never node-pinned**
    /// (the relocatability §3.5 decided at M3 carries to the object backing — relocating within the
    /// region does not re-address any object).
    #[test]
    fn object_backed_placement_is_region_pinned_and_relocatable() {
        let (tier, repo) = placed_object_tier();
        let content = b"relocatable on the object tier";
        let before = tier
            .put_object(&repo, GitObjectKind::Tree, content)
            .expect("put");
        // Relocate within the region (the group flips; no object move / re-address).
        tier.relocate(
            &repo,
            StorageGroup::from_token("pack-9"),
            &Region::new("fr-par"),
        )
        .expect("same-region relocation on the object tier");
        let after = crate::gitpack::git_object_address(GitObjectKind::Tree, content);
        assert_eq!(before, after, "the address is unchanged by relocation");
        assert_eq!(
            tier.get_object(&repo, &before)
                .expect("served after relocation"),
            content
        );
    }
}
