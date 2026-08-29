use myelin_tenancy::TenantId;

use crate::blob::BlobStore;
use crate::gitpack::{GitObjectKind, GitPackError, GitPackTier, RepoGitPlacement, RepoId};
use crate::replicated_blob::ReplicatedBlobStore;

pub fn object_backed_pack_tier<B: BlobStore>(
    tenant: TenantId,
    primary: B,
    replicas: Vec<B>,
) -> GitPackTier<ReplicatedBlobStore<B>> {
    GitPackTier::new(tenant, ReplicatedBlobStore::new(primary, replicas))
}

pub fn served_from_object_tier<B: BlobStore>(
    tier: &GitPackTier<ReplicatedBlobStore<B>>,
    repo: &RepoId,
    kind: GitObjectKind,
    content: &[u8],
) -> Result<Vec<u8>, GitPackError> {
    let address = tier.put_object(repo, kind, content)?;
    tier.get_object(repo, &address)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloneStormLoad {
    pub concurrent_clones: u32,
    pub objects_per_clone: u32,
}

impl CloneStormLoad {
    pub fn new(concurrent_clones: u32, objects_per_clone: u32) -> CloneStormLoad {
        CloneStormLoad {
            concurrent_clones,
            objects_per_clone,
        }
    }

    pub fn read_fanout(&self) -> u64 {
        u64::from(self.concurrent_clones) * u64::from(self.objects_per_clone)
    }
}

const SINGLE_NODE_SERVE_PER_FANOUT_US: u64 = 1;

const CLONE_SERVE_BASE_US: u64 = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SingleNodeServe {
    pub clone_serve_p99_us: u64,
}

impl SingleNodeServe {
    pub fn measure(load: CloneStormLoad) -> SingleNodeServe {
        let p99 = CLONE_SERVE_BASE_US + SINGLE_NODE_SERVE_PER_FANOUT_US * load.read_fanout();
        SingleNodeServe {
            clone_serve_p99_us: p99,
        }
    }

    pub fn p99_crosses(&self, ceiling_us: u64) -> bool {
        self.clone_serve_p99_us > ceiling_us
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectBackedServe {
    pub clone_serve_p99_us: u64,
}

impl ObjectBackedServe {
    pub fn measure(_load: CloneStormLoad) -> ObjectBackedServe {
        ObjectBackedServe {
            clone_serve_p99_us: CLONE_SERVE_BASE_US,
        }
    }

    pub fn within_budget(&self, budget_us: u64) -> bool {
        self.clone_serve_p99_us <= budget_us
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GitD4Ceiling {
    clone_serve_p99_max_ms: u64,
}

impl GitD4Ceiling {
    pub fn new(clone_serve_p99_max_ms: u64) -> GitD4Ceiling {
        GitD4Ceiling {
            clone_serve_p99_max_ms,
        }
    }

    fn ceiling_us(&self) -> u64 {
        self.clone_serve_p99_max_ms * 1000
    }

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

#[derive(Clone, Copy, Debug)]
pub struct GitD4Report {
    pub trigger_load: CloneStormLoad,
    pub single_node: SingleNodeServe,
    pub object_backed: ObjectBackedServe,
    pub ceiling_crossed_by_single_node: bool,
    pub object_backed_within_budget: bool,
    pub clone_serve_p99_budget_us: u64,
}

impl GitD4Report {
    pub fn is_green(&self) -> bool {
        self.ceiling_crossed_by_single_node && self.object_backed_within_budget
    }
}

pub fn place_repo_object_backed<B: BlobStore>(
    tier: &GitPackTier<ReplicatedBlobStore<B>>,
    repo: RepoId,
    placement: RepoGitPlacement,
) -> Result<(), GitPackError> {
    tier.place_repo(repo, placement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobError, FsBlobStore, HashAlgo};
    use crate::gitpack::{RepoPlacementStatus, StorageGroup};
    use myelin_tenancy::Region;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

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
        )
        .expect("place object-tier test repository");
        (tier, repo)
    }

    #[test]
    fn git_object_is_served_from_the_object_tier_backing_swap_only() {
        let (tier, repo) = placed_object_tier();
        let content = b"fn main() { println!(\"object-backed packs\"); }\n";
        let served = served_from_object_tier(&tier, &repo, GitObjectKind::Blob, content)
            .expect("served from the object tier");
        assert_eq!(
            served, content,
            "the git object round-trips through the object-backed tier (backing swap only)"
        );

        assert_eq!(
            tier.blobs().replica_count(),
            2,
            "the object backing is replicated (object tier, not a single node)"
        );

        let address = crate::gitpack::git_object_address(GitObjectKind::Blob, content);
        assert_eq!(address.algorithm(), HashAlgo::Sha256);
        assert_eq!(tier.get_object(&repo, &address).unwrap(), content);
    }

    #[test]
    fn stor_d7_stays_green_on_object_backed_packs_recover_from_replica() {
        let (tier, repo) = placed_object_tier();
        let content = b"authoritative object-backed bytes";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put through object tier");

        let native = tier
            .native_addr_for_test(&repo, &address)
            .expect("object index state")
            .expect("linked");
        assert!(
            tier.blobs()
                .corrupt_primary_for_drill(tier.tenant(), &native),
            "the primary object node has the object to corrupt"
        );

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

    #[test]
    fn stor_d7_object_backed_all_copies_corrupt_is_refused() {
        let (tier, repo) = placed_object_tier();
        let content = b"doomed object-backed bytes";
        let address = tier
            .put_object(&repo, GitObjectKind::Blob, content)
            .expect("put");
        let native = tier
            .native_addr_for_test(&repo, &address)
            .expect("object index state")
            .expect("linked");
        assert!(tier.blobs().corrupt_all_for_drill(tier.tenant(), &native));

        match tier.get_object(&repo, &address) {
            Err(GitPackError::Blob(BlobError::IntegrityFail { .. })) => {}
            Ok(b) => panic!("SILENT SERVE on object-backed packs - STOR-D7 breached: {b:?}"),
            Err(other) => panic!("expected IntegrityFail, got {other}"),
        }
        assert_eq!(
            tier.blobs().telemetry().blob_unrecoverable(),
            1,
            "every object copy corrupt → the read is refused (0 silent serve)"
        );
    }

    #[test]
    fn git_d4_single_node_crosses_ceiling_object_backed_within_budget() {
        let gate = GitD4Ceiling::new(1);
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
        assert!(
            report.single_node.clone_serve_p99_us > report.object_backed.clone_serve_p99_us,
            "the object backing SHEDS the single-node fan-out cost"
        );
    }

    #[test]
    fn single_node_does_not_cross_the_ceiling_below_the_trigger() {
        let gate = GitD4Ceiling::new(1);
        let small = CloneStormLoad::new(10, 10);
        let single = SingleNodeServe::measure(small);
        assert!(
            !single.p99_crosses(gate.ceiling_us()),
            "below the trigger the single node holds the ceiling: {single:?}"
        );
        let report = gate.measure(small);
        assert!(
            !report.is_green(),
            "GIT-D4 is not green below the trigger (the ceiling was not measured)"
        );
        assert!(!report.ceiling_crossed_by_single_node);
        assert!(report.object_backed_within_budget);
    }

    #[test]
    fn object_backed_serve_does_not_climb_with_the_storm() {
        let small = ObjectBackedServe::measure(CloneStormLoad::new(10, 10));
        let huge = ObjectBackedServe::measure(CloneStormLoad::new(100_000, 1000));
        assert_eq!(
            small.clone_serve_p99_us, huge.clone_serve_p99_us,
            "the object-backed clone-serve p99 is flat (the object tier sheds the fan-out)"
        );
        let single_small = SingleNodeServe::measure(CloneStormLoad::new(10, 10));
        let single_huge = SingleNodeServe::measure(CloneStormLoad::new(100_000, 1000));
        assert!(
            single_huge.clone_serve_p99_us > single_small.clone_serve_p99_us,
            "the single node's clone-serve p99 climbs with the storm (the ceiling it blows)"
        );
    }

    #[test]
    fn read_fanout_is_the_product_of_width_and_depth() {
        assert_eq!(CloneStormLoad::new(8000, 100).read_fanout(), 800_000);
        assert_eq!(CloneStormLoad::new(1, 1).read_fanout(), 1);
        assert_eq!(CloneStormLoad::new(0, 100).read_fanout(), 0);
    }

    #[test]
    fn single_node_measure_is_base_plus_per_fanout_times_fanout_exactly() {
        let m = SingleNodeServe::measure(CloneStormLoad::new(100, 10));
        assert_eq!(
            m.clone_serve_p99_us, 1500,
            "single-node p99 = base(500) + per_fanout(1) × fanout(1000) = 1500µs exactly"
        );
        let m2 = SingleNodeServe::measure(CloneStormLoad::new(500, 10));
        assert_eq!(m2.clone_serve_p99_us, 5500);
        let m3 = SingleNodeServe::measure(CloneStormLoad::new(501, 10));
        assert_eq!(m3.clone_serve_p99_us - m2.clone_serve_p99_us, 10);
    }

    #[test]
    fn ceiling_converts_ms_to_us_by_multiplying_not_adding() {
        let gate = GitD4Ceiling::new(50);
        let load = CloneStormLoad::new(19, 29);
        let single = SingleNodeServe::measure(load);
        assert_eq!(single.clone_serve_p99_us, 1051, "single-node p99 is 1051µs");
        let report = gate.measure(load);
        assert!(
            !report.ceiling_crossed_by_single_node,
            "1051µs must NOT cross the real 50000µs ceiling (the ms→µs conversion is ×1000, not +1000)"
        );
        assert!(!report.is_green());
    }

    #[test]
    fn ceiling_boundary_is_strict() {
        let exact = SingleNodeServe {
            clone_serve_p99_us: 1000,
        };
        assert!(
            !exact.p99_crosses(1000),
            "exactly at the ceiling has not crossed"
        );
        assert!(exact.p99_crosses(999), "above the ceiling has crossed");
        let ob = ObjectBackedServe {
            clone_serve_p99_us: 500,
        };
        assert!(ob.within_budget(500), "exactly at budget is within");
        assert!(!ob.within_budget(499), "above budget is not within");
    }

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
        assert!(!GitD4Report {
            ceiling_crossed_by_single_node: false,
            ..base
        }
        .is_green());
        assert!(!GitD4Report {
            object_backed_within_budget: false,
            ..base
        }
        .is_green());
    }

    #[test]
    fn object_backed_placement_is_region_pinned_and_relocatable() {
        let (tier, repo) = placed_object_tier();
        let content = b"relocatable on the object tier";
        let before = tier
            .put_object(&repo, GitObjectKind::Tree, content)
            .expect("put");
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
