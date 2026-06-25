//! # `cross_cell` — cross-cell / multi-region git replication (GF-2 → GIT-P33 / P-482, M5)
//!
//! **The single-cell primary+quorum floor lifts to cross-cell active replica sets within-EU.** The M3
//! floor (GIT-P11 / GF-2) replicated a repo's packs within ONE cell (a primary + quorum-ack replica
//! set). This module promotes that to **cross-cell active replica sets**: a repo's authoritative tip
//! has a HOME cell (where the per-ref CAS — the linearisation point — lives) and ACTIVE geo read-
//! replicas in OTHER within-EU cells, addressed through the OQ-I [`CrossCellPointer`] bridge (contract
//! 12.6). `update_seq` is the fence: a replica is current iff its `update_seq` ≥ the home's.
//!
//! **Owning architecture (read first, in full):**
//! `05-hard-problems.md` **HP-1** (cross-cell active replica sets within-EU — "the single-cell floor's
//! primary+quorum lifts to multi-cell") + **HP-6** (the DB ref-store transaction is the linearisation
//! point; `update_seq` is the fence; NO bespoke per-repo consensus group; no split-brain).
//! `02-internals-and-algorithms.md` §4 (replication TE-24). Reconciliation
//! `00-reconciliation-decisions.md` OQ-I (the cross-cell bridge). Contract 12.6 (the cross-cell PII-free
//! pointer bridge — CONSUMED).
//!
//! ## What is REUSED vs NEW (EI-01 §7 coherence)
//! The cross-cell bridge frame already exists and is NOT re-defined:
//! - [`myelin_tenancy::CrossCellPointer`] — the frozen four-field PII-free bridge frame
//!   (`subject`/`type`/`correlation_id`/`home_cell`); resolution is always cell-local.
//! - [`myelin_storage::is_cell_local`] / [`myelin_storage::migrate_cell_to_cell`] — the storage-side
//!   cell-local-resolution discriminant + the cell→cell migration (P-443 / P-ST-30).
//! - The per-ref `update_seq` fence ([`crate::receive_pack::PushOutcome::Accepted`] carries the moved
//!   refs' `update_seq`) — the monotonic generation the recovery tiebreaker uses (HP-6).
//!
//! What is **genuinely NEW** here (the git-side cross-cell promotion):
//! 1. [`CrossCellReplicaSet`] — a repo's active replica set across within-EU cells: the HOME cell (the
//!    CAS linearisation point) + the geo read-replica cells, each with its observed `update_seq`.
//! 2. [`ReplicaFreshness`] — the `update_seq`-fenced freshness verdict: a replica is `Current` iff its
//!    `update_seq` ≥ the home's, else `Stale(behind_by)`. The fence honoured (HP-6).
//! 3. The within-EU residency invariant: a replica cell MUST be within-EU for an EU tenant (a repo
//!    NEVER replicates outside its residency class — [`CrossCellReplicaSet::add_replica`] refuses an
//!    extra-EU cell).
//! 4. [`pointer_for`] — the [`CrossCellPointer`] frame a geo read of a foreign-homed ref compiles
//!    against (resolution defers to the home cell; only the rendered projection crosses, never PII).
//!
//! ## The linearisation point stays the HOME cell's ref CAS (no split-brain — HP-6)
//! A protected-ref WRITE is admitted ONLY at the home cell (the per-ref CAS row lock there is THE
//! linearisation point). Geo replicas are READ replicas: they serve clones/fetches from their local
//! object tier but never accept a ref write (a write to a replica is routed to the home cell). So there
//! is no rival consensus log, no split-brain — the home cell's DB transaction is the single authority,
//! and `update_seq` is the fence the replicas catch up to. This is the GF-2 lift done the HP-6 way.
//!
//! ## FLOOR PROMOTED (the honesty register — VISION §3 / EI-01 §1)
//! - **GF-2 — single-cell primary+quorum (M3 floor) → cross-cell active replica sets within-EU.** The
//!   replica-set model + the `update_seq` fence + the within-EU residency invariant + the cell-local
//!   resolution ship HERE, riding the frozen [`CrossCellPointer`] bridge. Recorded, dated GIT-P33. The
//!   real WAL-streaming transport between cells is the storage replication layer (P-443); this owns the
//!   git-grain replica-set semantics over it.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; a lost/forked tip is the failure)
//! The replication path is mandatory-core. The load-bearing mutants — the `update_seq` freshness fence
//! ([`ReplicaFreshness::of`] / its `>=` boundary), the within-EU residency refusal
//! ([`CrossCellReplicaSet::add_replica`]), the home-cell-is-the-only-writer property, and the
//! cell-local resolution (a foreign-homed ref defers, never reads foreign PII locally) — are each killed
//! by an assertion in the unit tests. The floor is **≥ 80%**.

use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

/// **A within-EU cell in a repo's replica set, with the `update_seq` it has observed for the
/// replicated ref.** PII-free: a cell id + a monotonic generation, no person, no payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplicaCell {
    /// The cell's routing handle (opaque — [`CellId`]).
    pub cell: CellId,
    /// Whether this cell is within the EU residency class (an EU tenant's repo only replicates here).
    pub within_eu: bool,
    /// The `update_seq` (the per-ref monotonic generation, HP-6) this replica has caught up to. A geo
    /// read-replica lags the home until it streams the home's latest ref move.
    pub update_seq: u64,
}

impl ReplicaCell {
    /// A replica cell caught up to `update_seq`.
    pub fn new(cell: CellId, within_eu: bool, update_seq: u64) -> ReplicaCell {
        ReplicaCell {
            cell,
            within_eu,
            update_seq,
        }
    }
}

/// **A geo-replica's freshness against the home cell (the `update_seq` fence — HP-6).** A replica is
/// `Current` iff it has streamed the home's latest ref move (`update_seq` ≥ home's); else it is `Stale`
/// by how many generations it lags. The recovery tiebreaker is the DB ref index; `update_seq` is the
/// fence — a stale replica is never served as authoritative. PII-free closed enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplicaFreshness {
    /// The replica has caught up to (or past) the home's `update_seq` — current, safe to serve.
    Current,
    /// The replica lags the home by `behind_by` generations — stale (catch-up in flight).
    Stale {
        /// How many `update_seq` generations the replica is behind the home.
        behind_by: u64,
    },
}

impl ReplicaFreshness {
    /// **Compute a replica's freshness against the home `update_seq` (the fence).** `Current` IFF
    /// `replica_seq >= home_seq` (the replica streamed the home's latest move), else `Stale` by the
    /// gap. Mandatory-core: the `>=` boundary is the fence — an off-by-one would serve a stale tip.
    pub fn of(home_seq: u64, replica_seq: u64) -> ReplicaFreshness {
        if replica_seq >= home_seq {
            ReplicaFreshness::Current
        } else {
            ReplicaFreshness::Stale {
                behind_by: home_seq - replica_seq,
            }
        }
    }

    /// Is the replica current (safe to serve as the ref tip)?
    pub fn is_current(self) -> bool {
        matches!(self, ReplicaFreshness::Current)
    }
}

/// **A within-EU residency violation — a replica cell outside the tenant's residency class.** A repo
/// NEVER replicates outside its EU residency (the residency-pin invariant carried to the cross-cell
/// layer). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyViolation {
    /// The offending cell that is not within-EU.
    pub cell: CellId,
}

impl std::fmt::Display for ResidencyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cross-cell replication REFUSED: cell {:?} is not within the EU residency class — an \
             EU tenant's repo NEVER replicates outside its residency (contract 12.6 / STOR-5)",
            self.cell
        )
    }
}

impl std::error::Error for ResidencyViolation {}

/// **A repo's cross-cell active replica set (GF-2 → GIT-P33).** The HOME cell is where the per-ref CAS
/// (the linearisation point) lives — the ONLY cell that accepts a protected-ref WRITE. The replica
/// cells are within-EU geo READ-replicas that serve clones/fetches from their local object tier and
/// stream the home's ref moves (catching `update_seq` up). No replica is a writer (no split-brain).
#[derive(Clone, Debug)]
pub struct CrossCellReplicaSet {
    /// The repo's canonical artifact ref (the opaque subject of the cross-cell pointer — PII-free).
    repo: ArtifactRef,
    /// The HOME cell — the per-ref CAS linearisation point + the authoritative tip.
    home_cell: CellId,
    /// Whether the tenant is EU-residency-classed (an EU repo only replicates to within-EU cells).
    tenant_is_eu: bool,
    /// The home's current `update_seq` (the fence the replicas catch up to).
    home_update_seq: u64,
    /// The within-EU geo read-replica cells (each with its observed `update_seq`).
    replicas: Vec<ReplicaCell>,
}

impl CrossCellReplicaSet {
    /// Open a replica set for a repo homed in `home_cell` at `home_update_seq`. `tenant_is_eu` gates
    /// the within-EU residency invariant on every added replica.
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

    /// The home cell (the per-ref CAS linearisation point — the only writer).
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// The home's current `update_seq` (the fence).
    pub fn home_update_seq(&self) -> u64 {
        self.home_update_seq
    }

    /// **Add a within-EU geo read-replica cell.** REFUSES an extra-EU cell for an EU tenant (the
    /// residency invariant — a repo never replicates outside its residency class). For a non-EU tenant
    /// the within-EU constraint does not apply (the residency class is the tenant's own region set).
    pub fn add_replica(&mut self, replica: ReplicaCell) -> Result<(), ResidencyViolation> {
        if self.tenant_is_eu && !replica.within_eu {
            return Err(ResidencyViolation { cell: replica.cell });
        }
        self.replicas.push(replica);
        Ok(())
    }

    /// The within-EU geo read-replica cells.
    pub fn replicas(&self) -> &[ReplicaCell] {
        &self.replicas
    }

    /// **Advance the home `update_seq` after a protected-ref CAS at the home cell** (a push moved the
    /// ref). The replicas are now behind until they stream the move. Monotonic (a lower seq is ignored
    /// — the fence never moves backwards).
    pub fn advance_home(&mut self, new_update_seq: u64) {
        if new_update_seq > self.home_update_seq {
            self.home_update_seq = new_update_seq;
        }
    }

    /// **A replica's freshness against the home `update_seq` (the fence — HP-6).** `Current` iff the
    /// replica streamed the home's latest move; else `Stale`. A `cell` not in the replica set is
    /// treated as maximally stale (it holds nothing).
    pub fn freshness(&self, cell: &CellId) -> ReplicaFreshness {
        match self.replicas.iter().find(|r| &r.cell == cell) {
            Some(r) => ReplicaFreshness::of(self.home_update_seq, r.update_seq),
            None => ReplicaFreshness::Stale {
                behind_by: self.home_update_seq,
            },
        }
    }

    /// **Stream the home's ref move into a replica (catch its `update_seq` up).** The geo read-replica
    /// has received the home's latest ref bytes; its `update_seq` advances to the home's. After this it
    /// is `Current`. A no-op if the cell is not in the set.
    pub fn stream_into(&mut self, cell: &CellId) {
        let home_seq = self.home_update_seq;
        if let Some(r) = self.replicas.iter_mut().find(|r| &r.cell == cell) {
            r.update_seq = home_seq;
        }
    }

    /// **Is `this_cell` the home (the only cell that may accept a protected-ref WRITE)?** A write at a
    /// replica cell is routed to the home — there is no rival writer (no split-brain — HP-6). The
    /// load-bearing single-writer predicate.
    pub fn is_home(&self, this_cell: &CellId) -> bool {
        &self.home_cell == this_cell
    }

    /// **The [`CrossCellPointer`] frame a geo read of this repo's ref compiles against (12.6).** When a
    /// cell other than the home wants the authoritative tip, it resolves THROUGH this pointer: the
    /// pointer's `home_cell` is where resolution happens (cell-local at the home), and only the
    /// already-rendered projection crosses back — never the repo's PII. `correlation_id` ties the read
    /// to its causal chain (BUS-5).
    pub fn pointer_for(&self, correlation_id: CorrelationId) -> CrossCellPointer {
        pointer_for(&self.repo, &self.home_cell, correlation_id)
    }
}

/// **Build the cross-cell pointer frame for a repo's authoritative tip (12.6).** The subject is the
/// repo's opaque artifact ref (never PII); the type is [`ArtifactType::Repo`]; the home cell is where
/// resolution happens. A foreign cell resolving this pointer defers to the home cell — it never reads
/// the repo's PII into its own tier (resolution is cell-local; only the projection crosses).
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

    /// **A within-EU geo read-replica is admitted; an extra-EU cell is REFUSED for an EU tenant.** The
    /// residency invariant — a repo never replicates outside its residency class.
    #[test]
    fn within_eu_replica_admitted_extra_eu_refused() {
        let mut s = set();
        // A within-EU replica (another EU cell) is admitted.
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-de-fra"),
            true,
            10,
        ))
        .expect("within-EU replica admitted");
        assert_eq!(s.replicas().len(), 1);

        // An extra-EU cell is REFUSED for an EU tenant.
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

    /// **A non-EU tenant is not bound by the within-EU constraint** (its residency class is its own
    /// region set). Kills a mutant that would force within-EU on every tenant.
    #[test]
    fn non_eu_tenant_not_bound_by_within_eu() {
        let mut s = CrossCellReplicaSet::new(repo(), CellId::from_token("cell-us-east"), false, 5);
        // A non-within-EU replica is admitted for a non-EU tenant.
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-us-west"),
            false,
            5,
        ))
        .expect("non-EU tenant admits a non-EU replica");
        assert_eq!(s.replicas().len(), 1);
    }

    /// **The `update_seq` fence: a replica that streamed the home's latest move is Current; one behind
    /// is Stale by the gap.** The HP-6 fence — a stale replica is never served as authoritative.
    #[test]
    fn update_seq_fence_current_vs_stale() {
        let mut s = set(); // home at update_seq 10.
        s.add_replica(ReplicaCell::new(CellId::from_token("cell-de-fra"), true, 7))
            .unwrap();
        // The replica is behind the home (7 < 10) → stale by 3.
        assert_eq!(
            s.freshness(&CellId::from_token("cell-de-fra")),
            ReplicaFreshness::Stale { behind_by: 3 }
        );
        // Stream the home's move into the replica → it catches up → Current.
        s.stream_into(&CellId::from_token("cell-de-fra"));
        assert_eq!(
            s.freshness(&CellId::from_token("cell-de-fra")),
            ReplicaFreshness::Current
        );
        assert!(s.freshness(&CellId::from_token("cell-de-fra")).is_current());
    }

    /// **The fence boundary is `>=` (a replica exactly at the home seq is Current).** Kills the
    /// `>=` → `>` boundary mutant.
    #[test]
    fn fence_boundary_is_inclusive() {
        assert_eq!(ReplicaFreshness::of(10, 10), ReplicaFreshness::Current);
        assert_eq!(ReplicaFreshness::of(10, 11), ReplicaFreshness::Current);
        assert_eq!(
            ReplicaFreshness::of(10, 9),
            ReplicaFreshness::Stale { behind_by: 1 }
        );
        // A cell not in the set is maximally stale (it holds nothing).
        let s = set();
        assert_eq!(
            s.freshness(&CellId::from_token("cell-unknown")),
            ReplicaFreshness::Stale { behind_by: 10 }
        );
    }

    /// **The home cell is the ONLY writer (no split-brain — HP-6).** `is_home` is true only for the
    /// home; a write at a replica cell is routed to the home. Kills a mutant that would let a replica
    /// write.
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

    /// **Advancing the home `update_seq` (a CAS at the home) makes the replicas stale until they
    /// stream.** The fence moves forward on a home write; replicas catch up by streaming.
    #[test]
    fn advancing_the_home_makes_replicas_stale() {
        let mut s = set(); // home at 10.
        s.add_replica(ReplicaCell::new(
            CellId::from_token("cell-de-fra"),
            true,
            10,
        ))
        .unwrap();
        assert!(s.freshness(&CellId::from_token("cell-de-fra")).is_current());

        // A push moves the ref at the home (update_seq 10 → 11).
        s.advance_home(11);
        assert_eq!(s.home_update_seq(), 11);
        // The replica is now stale by 1 (it has not streamed the new move).
        assert_eq!(
            s.freshness(&CellId::from_token("cell-de-fra")),
            ReplicaFreshness::Stale { behind_by: 1 }
        );

        // advance_home is monotonic (a lower seq is ignored — the fence never moves backwards).
        s.advance_home(9);
        assert_eq!(s.home_update_seq(), 11, "the fence never moves backwards");
    }

    /// **The cross-cell pointer is PII-free + homed at the home cell; a foreign cell defers (cell-local
    /// resolution).** The pointer's subject is the repo's opaque ref; resolution happens at the home.
    #[test]
    fn pointer_is_pii_free_and_resolution_is_cell_local() {
        let s = set();
        let p = s.pointer_for(CorrelationId("01J0CORR".into()));
        // PII-free four-field frame: subject is the opaque repo ref, type is Repo, homed at fr-par.
        assert_eq!(p.subject().artifact_ref().0, "myelin://acme/git/repo/core");
        assert_eq!(p.artifact_type(), &ArtifactType::Repo);
        assert_eq!(p.home_cell(), &CellId::from_token("cell-fr-par"));

        // A read AT the home cell resolves locally (the home renders + permission-checks).
        assert!(is_cell_local(&p, &CellId::from_token("cell-fr-par")));
        // A read at a FOREIGN cell does NOT resolve locally — it defers to the home (no foreign PII
        // read into the local tier; only the rendered projection crosses).
        assert!(!is_cell_local(&p, &CellId::from_token("cell-de-fra")));
    }
}
