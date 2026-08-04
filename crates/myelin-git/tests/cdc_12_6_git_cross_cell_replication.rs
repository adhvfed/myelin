use myelin_git::cross_cell::{CrossCellReplicaSet, ReplicaCell, ReplicaFreshness};
use myelin_storage::is_cell_local;
use myelin_tenancy::{ArtifactRef, ArtifactType, CellId, CorrelationId};

fn repo() -> ArtifactRef {
    ArtifactRef("myelin://acme/git/repo/core".into())
}

#[test]
fn replica_set_compiles_the_frozen_pointer_and_resolution_is_cell_local() {
    let set = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 10);
    let pointer = set.pointer_for(CorrelationId("01J0CORR".into()));

    assert_eq!(
        pointer.subject().artifact_ref().0,
        "myelin://acme/git/repo/core"
    );
    assert_eq!(pointer.artifact_type(), &ArtifactType::Repo);
    assert_eq!(pointer.home_cell(), &CellId::from_token("cell-fr-par"));

    assert!(is_cell_local(&pointer, &CellId::from_token("cell-fr-par")));
    assert!(!is_cell_local(&pointer, &CellId::from_token("cell-de-fra")));
}

#[test]
fn cross_cell_replication_is_within_eu_for_an_eu_tenant() {
    let mut set = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 10);
    set.add_replica(ReplicaCell::new(
        CellId::from_token("cell-de-fra"),
        true,
        10,
    ))
    .expect("within-EU replica admitted");
    set.add_replica(ReplicaCell::new(
        CellId::from_token("cell-us-east"),
        false,
        10,
    ))
    .expect_err("an extra-EU replica is refused (residency invariant)");
}

#[test]
fn update_seq_is_the_cross_cell_fence() {
    let mut set = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 12);
    set.add_replica(ReplicaCell::new(CellId::from_token("cell-de-fra"), true, 9))
        .unwrap();
    assert_eq!(
        set.freshness(&CellId::from_token("cell-de-fra")),
        ReplicaFreshness::Stale { behind_by: 3 }
    );
    set.stream_into(&CellId::from_token("cell-de-fra"));
    assert!(set
        .freshness(&CellId::from_token("cell-de-fra"))
        .is_current());
}
