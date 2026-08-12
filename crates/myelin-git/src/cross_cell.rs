use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaCell {
    pub cell: CellId,
    pub within_eu: bool,
    pub update_seq: u64,
}

impl ReplicaCell {
    pub fn new(cell: CellId, within_eu: bool, update_seq: u64) -> ReplicaCell {
        ReplicaCell {
            cell,
            within_eu,
            update_seq,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplicaFreshness {
    Current,
    Stale { behind_by: u64 },
}

impl ReplicaFreshness {
    pub fn of(home_seq: u64, replica_seq: u64) -> ReplicaFreshness {
        if replica_seq >= home_seq {
            ReplicaFreshness::Current
        } else {
            ReplicaFreshness::Stale {
                behind_by: home_seq - replica_seq,
            }
        }
    }

    pub fn is_current(self) -> bool {
        matches!(self, ReplicaFreshness::Current)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyViolation {
    pub cell: CellId,
}

impl std::fmt::Display for ResidencyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cross-cell replication REFUSED: cell {:?} is not within the EU residency class - an \
             EU tenant's repo NEVER replicates outside its residency (contract 12.6 / STOR-5)",
            self.cell
        )
    }
}

impl std::error::Error for ResidencyViolation {}

#[derive(Clone, Debug)]
pub struct CrossCellReplicaSet {
    repo: ArtifactRef,
    home_cell: CellId,
    tenant_is_eu: bool,
    home_update_seq: u64,
    replicas: Vec<ReplicaCell>,
}

impl CrossCellReplicaSet {
    pub fn new(
        repo: ArtifactRef,
        home_cell: CellId,
        tenant_is_eu: bool,
        home_update_seq: u64,
    ) -> CrossCellReplicaSet {
        CrossCellReplicaSet {
            repo,
            home_cell,
            tenant_is_eu,
            home_update_seq,
            replicas: Vec::new(),
        }
    }

    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn home_update_seq(&self) -> u64 {
        self.home_update_seq
    }

    pub fn add_replica(&mut self, replica: ReplicaCell) -> Result<(), ResidencyViolation> {
        if self.tenant_is_eu && !replica.within_eu {
            return Err(ResidencyViolation { cell: replica.cell });
        }
        self.replicas.push(replica);
        Ok(())
    }

    pub fn replicas(&self) -> &[ReplicaCell] {
        &self.replicas
    }

    pub fn advance_home(&mut self, new_update_seq: u64) {
        if new_update_seq > self.home_update_seq {
            self.home_update_seq = new_update_seq;
        }
    }

    pub fn freshness(&self, cell: &CellId) -> ReplicaFreshness {
        match self.replicas.iter().find(|r| &r.cell == cell) {
            Some(r) => ReplicaFreshness::of(self.home_update_seq, r.update_seq),
            None => ReplicaFreshness::Stale {
                behind_by: self.home_update_seq,
            },
        }
    }

    pub fn stream_into(&mut self, cell: &CellId) {
        let home_seq = self.home_update_seq;
        if let Some(r) = self.replicas.iter_mut().find(|r| &r.cell == cell) {
            r.update_seq = home_seq;
        }
    }

    pub fn is_home(&self, this_cell: &CellId) -> bool {
        &self.home_cell == this_cell
    }

    pub fn pointer_for(&self, correlation_id: CorrelationId) -> CrossCellPointer {
        pointer_for(&self.repo, &self.home_cell, correlation_id)
    }
}

pub fn pointer_for(
    repo: &ArtifactRef,
    home_cell: &CellId,
    correlation_id: CorrelationId,
) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(repo.clone()),
        ArtifactType::Repo,
        correlation_id,
        home_cell.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::is_cell_local;

    fn repo() -> ArtifactRef {
        ArtifactRef("myelin://acme/git/repo/core".into())
    }

    fn set() -> CrossCellReplicaSet {
        CrossCellReplicaSet::new(repo(), CellId::from_token("cell-fr-par"), true, 10)
    }

    #[test]
    fn within_eu_replica_admitted_extra_eu_refused() {
        let mut s = set();
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-de-fra"),
            true,
            10,
        ))
        .expect("within-EU replica admitted");
        assert_eq!(s.replicas().len(), 1);

        let err = s
            .add_replica(ReplicaCell::new(
                CellId::from_token("cell-us-east"),
                false,
                10,
            ))
            .expect_err("an extra-EU replica is refused for an EU tenant");
        assert_eq!(err.cell, CellId::from_token("cell-us-east"));
        assert!(err.to_string().contains("REFUSED"));
        assert_eq!(
            s.replicas().len(),
            1,
            "the extra-EU cell did not join the set"
        );
    }

    #[test]
    fn non_eu_tenant_not_bound_by_within_eu() {
        let mut s = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-us-east"), false, 5);
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-us-west"),
            false,
            5,
        ))
        .expect("non-EU tenant admits a non-EU replica");
        assert_eq!(s.replicas().len(), 1);
    }

    #[test]
    fn update_seq_fence_current_vs_stale() {
        let mut s = set();
        s.add_replica(ReplicaCell::new(CellId::from_token("cell-de-fra"), true, 7))
            .unwrap();
        assert_eq!(
            s.freshness(&CellId::from_token("cell-de-fra")),
            ReplicaFreshness::Stale { behind_by: 3 }
        );
        s.stream_into(&CellId::from_token("cell-de-fra"));
        assert_eq!(
            s.freshness(&CellId::from_token("cell-de-fra")),
            ReplicaFreshness::Current
        );
        assert!(s.freshness(&CellId::from_token("cell-de-fra")).is_current());
    }

    #[test]
    fn fence_boundary_is_inclusive() {
        assert_eq!(ReplicaFreshness::of(10, 10), ReplicaFreshness::Current);
        assert_eq!(ReplicaFreshness::of(10, 11), ReplicaFreshness::Current);
        assert_eq!(
            ReplicaFreshness::of(10, 9),
            ReplicaFreshness::Stale { behind_by: 1 }
        );
        let s = set();
        assert_eq!(
            s.freshness(&CellId::from_token("cell-unknown")),
            ReplicaFreshness::Stale { behind_by: 10 }
        );
    }

    #[test]
    fn home_cell_is_the_only_writer() {
        let mut s = set();
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-de-fra"),
            true,
            10,
        ))
        .unwrap();
        assert!(
            s.is_home(&CellId::from_token("cell-fr-par")),
            "the home is the writer"
        );
        assert!(
            !s.is_home(&CellId::from_token("cell-de-fra")),
            "a replica is NOT a writer (no split-brain)"
        );
    }

    #[test]
    fn advancing_the_home_makes_replicas_stale() {
        let mut s = set();
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-de-fra"),
            true,
            10,
        ))
        .unwrap();
        assert!(s.freshness(&CellId::from_token("cell-de-fra")).is_current());

        s.advance_home(11);
        assert_eq!(s.home_update_seq(), 11);
        assert_eq!(
            s.freshness(&CellId::from_token("cell-de-fra")),
            ReplicaFreshness::Stale { behind_by: 1 }
        );

        s.advance_home(9);
        assert_eq!(s.home_update_seq(), 11, "the fence never moves backwards");
    }

    #[test]
    fn pointer_is_pii_free_and_resolution_is_cell_local() {
        let s = set();
        let p = s.pointer_for(CorrelationId("01J0CORR".into()));
        assert_eq!(p.subject().artifact_ref().0, "myelin://acme/git/repo/core");
        assert_eq!(p.artifact_type(), &ArtifactType::Repo);
        assert_eq!(p.home_cell(), &CellId::from_token("cell-fr-par"));

        assert!(is_cell_local(&p, &CellId::from_token("cell-fr-par")));
        assert!(!is_cell_local(&p, &CellId::from_token("cell-de-fra")));
    }
}
