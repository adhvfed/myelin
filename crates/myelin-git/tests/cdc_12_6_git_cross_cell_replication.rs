//! Contract 12.6 CDC pair — git's CONSUMER half of the cross-cell PII-free pointer bridge
//! (GIT-P33 / global P-482, M5).
//!
//! Contract 12.6: "Cross-cell PII-free pointer bridge — `CrossCellPointer{subject (opaque), type,
//! correlation_id, home_cell}`; resolution always cell-local (the home cell renders + permission-
//! checks; only the projection crosses)." Git's cross-cell active replica sets (GF-2 → GIT-P33) RIDE
//! this frozen bridge.
//!
//! - **PROVIDER:** `myelin-tenancy` (the frozen [`myelin_tenancy::CrossCellPointer`] frame) +
//!   `myelin-storage` ([`myelin_storage::is_cell_local`] — the cell-local-resolution discriminant).
//! - **CONSUMER:** `myelin-git` — [`myelin_git::cross_cell::CrossCellReplicaSet`] composes the bridge
//!   frame for a repo's authoritative tip and defers a foreign-homed read to the home cell.
//!
//! The load-bearing contract: the pointer carries ONLY the four PII-free fields (subject is an opaque
//! repo ref, never PII), resolution is ALWAYS cell-local (a foreign cell defers, never reads foreign
//! PII locally), and `update_seq` is the fence (a stale replica is never served as authoritative).

use myelin_git::cross_cell::{CrossCellReplicaSet, ReplicaCell, ReplicaFreshness};
use myelin_storage::is_cell_local;
use myelin_tenancy::{ArtifactRef, ArtifactType, CellId, CorrelationId};

fn repo() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/repo/core".into())
}

/// **The replica set compiles the frozen four-field PII-free pointer frame; resolution is cell-local.**
/// The subject is the opaque repo ref (never PII), the type is `Repo`, the home cell is where
/// resolution happens. A foreign cell defers (storage's `is_cell_local` returns false).
#[test]
fn replica_set_compiles_the_frozen_pointer_and_resolution_is_cell_local() {
    let set = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 10);
    let pointer = set.pointer_for(CorrelationId("01J0CORR".into()));

    // Exactly the four frozen fields, subject opaque (never PII).
    assert_eq!(
        pointer.subject().artifact_ref().0,
        "myelin://acme/git/repo/core"
    );
    assert_eq!(pointer.artifact_type(), &ArtifactType::Repo);
    assert_eq!(pointer.home_cell(), &CellId::from_token("cell-fr-par"));

    // Resolution is cell-local: the home resolves locally; a foreign cell defers (no foreign PII read).
    assert!(is_cell_local(&pointer, &CellId::from_token("cell-fr-par")));
    assert!(!is_cell_local(&pointer, &CellId::from_token("cell-de-fra")));
}

/// **The within-EU residency invariant holds across the bridge: a repo never replicates extra-EU.** An
/// EU tenant's repo refuses a non-within-EU replica cell.
#[test]
fn cross_cell_replication_is_within_eu_for_an_eu_tenant() {
    let mut set = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 10);
    // A within-EU replica is admitted.
    set.add_replica(ReplicaCell::new(
        CellId::from_token("cell-de-fra"),
        true,
        10,
    ))
    .expect("within-EU replica admitted");
    // An extra-EU replica is REFUSED.
    set.add_replica(ReplicaCell::new(
        CellId::from_token("cell-us-east"),
        false,
        10,
    ))
    .expect_err("an extra-EU replica is refused (residency invariant)");
}

/// **`update_seq` is the fence: a replica behind the home is stale, never served as authoritative.**
/// The frozen recovery-tiebreaker property (HP-6) carried to the cross-cell layer.
#[test]
fn update_seq_is_the_cross_cell_fence() {
    let mut set = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 12);
    set.add_replica(ReplicaCell::new(CellId::from_token("cell-de-fra"), true, 9))
        .unwrap();
    // The replica is stale (9 < 12).
    assert_eq!(
        set.freshness(&CellId::from_token("cell-de-fra")),
        ReplicaFreshness::Stale { behind_by: 3 }
    );
    // After streaming the home's move, the replica is current (the fence is honoured).
    set.stream_into(&CellId::from_token("cell-de-fra"));
    assert!(set
        .freshness(&CellId::from_token("cell-de-fra"))
        .is_current());
}
