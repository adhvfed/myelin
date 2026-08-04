use myelin_storage::blob::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;

use crate::transport::{DocOpLog, PageSnapshot, PersistedOp};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocSnapshot {
    pub page_id: String,
    pub snap_seq: u64,
    pub blob_hash: ContentHash,
    pub named_label: Option<String>,
}

impl DocSnapshot {
    pub fn as_page_snapshot(&self) -> PageSnapshot {
        PageSnapshot {
            snap_seq: self.snap_seq,
            blob_hash: self.blob_hash.to_multihash_string(),
        }
    }
}

pub fn materialize(ops: &[PersistedOp]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in ops {
        out.extend_from_slice(&p.op_seq.to_be_bytes());
        push_lp(&mut out, p.op.op_id.wire().as_bytes());
        push_lp(&mut out, p.op.kind.as_str().as_bytes());
        push_lp(&mut out, &p.op.payload);
    }
    out
}

fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

pub fn content_address(materialized: &[u8]) -> ContentHash {
    ContentHash::blake3(materialized)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactionError {
    BeyondHead {
        requested: u64,
        head: u64,
    },
    UnreconstructableGap {
        target: u64,
        lowest_available: u64,
    },
    Blob(String),
}

impl core::fmt::Display for CompactionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CompactionError::BeyondHead { requested, head } => write!(
                f,
                "cannot compact up to op_seq {requested}: beyond the op-log head {head}"
            ),
            CompactionError::UnreconstructableGap {
                target,
                lowest_available,
            } => write!(
                f,
                "cannot reconstruct version {target}: ops below {lowest_available} were GC'd \
                 (the snapshot the target needs was pruned) - refusing a non-exact reconstruction"
            ),
            CompactionError::Blob(e) => write!(f, "snapshot blob error: {e}"),
        }
    }
}

impl std::error::Error for CompactionError {}

pub struct SnapshotCompactor<'b, B: BlobStore> {
    tenant: TenantId,
    page_id: String,
    blobs: &'b B,
}

impl<'b, B: BlobStore> SnapshotCompactor<'b, B> {
    pub fn new(
        tenant: TenantId,
        page_id: impl Into<String>,
        blobs: &'b B,
    ) -> SnapshotCompactor<'b, B> {
        SnapshotCompactor {
            tenant,
            page_id: page_id.into(),
            blobs,
        }
    }

    pub fn compact(
        &self,
        log: &DocOpLog,
        up_to_seq: u64,
        named_label: Option<String>,
    ) -> Result<DocSnapshot, CompactionError> {
        if up_to_seq > log.head_seq() {
            return Err(CompactionError::BeyondHead {
                requested: up_to_seq,
                head: log.head_seq(),
            });
        }
        let prefix = log.ops_up_to(up_to_seq);
        let materialized = materialize(&prefix);
        let blob_hash = self
            .blobs
            .put(&self.tenant, &materialized)
            .map_err(|e| CompactionError::Blob(e.to_string()))?;
        Ok(DocSnapshot {
            page_id: self.page_id.clone(),
            snap_seq: up_to_seq,
            blob_hash,
            named_label,
        })
    }

    pub fn load_snapshot_state(&self, snapshot: &DocSnapshot) -> Result<Vec<u8>, CompactionError> {
        self.blobs
            .get(&self.tenant, &snapshot.blob_hash)
            .map_err(|e| CompactionError::Blob(e.to_string()))
    }

    pub fn gc(&self, log: &mut DocOpLog, snap_seq: u64, open_cursors: &[u64]) -> usize {
        let watermark = match open_cursors.iter().copied().min() {
            Some(lowest_cursor) => snap_seq.min(lowest_cursor),
            None => snap_seq,
        };
        log.gc_below(watermark)
    }

    pub fn reconstruct_at(
        &self,
        log: &DocOpLog,
        snapshots: &[DocSnapshot],
        target: u64,
    ) -> Result<Vec<u8>, CompactionError> {
        let seed = snapshots
            .iter()
            .filter(|s| s.snap_seq <= target)
            .max_by_key(|s| s.snap_seq);

        match seed {
            Some(snapshot) => {
                let seed_state = self.load_snapshot_state(snapshot)?;
                let tail = log.ops_in_range(snapshot.snap_seq, target);
                self.guard_no_gap(log, snapshot.snap_seq, target)?;
                let mut state = seed_state;
                state.extend_from_slice(&materialize(&tail));
                Ok(state)
            }
            None => {
                self.guard_no_gap(log, 0, target)?;
                let prefix = log.ops_up_to(target);
                Ok(materialize(&prefix))
            }
        }
    }

    fn guard_no_gap(&self, log: &DocOpLog, from: u64, target: u64) -> Result<(), CompactionError> {
        if target <= from {
            return Ok(());
        }
        let lowest_available = log.lowest_seq();
        let needed_first = from + 1;
        let gap = match lowest_available {
            0 => true,
            lowest => lowest > needed_first,
        };
        if gap {
            return Err(CompactionError::UnreconstructableGap {
                target,
                lowest_available,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{DocOp, OpId, OpKind};
    use myelin_storage::blob::FsBlobStore;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn op(client: &str, lamport: u64, kind: OpKind, payload: &str) -> DocOp {
        DocOp::cas(
            OpId::new(client, lamport),
            "actor-1",
            kind,
            payload.as_bytes().to_vec(),
        )
    }

    fn log_with(n: u64) -> DocOpLog {
        let mut log = DocOpLog::new();
        for i in 1..=n {
            log.persist(op("c1", i, OpKind::Insert, &format!("edit-{i}")));
        }
        log
    }

    #[test]
    fn materialize_is_deterministic() {
        let log = log_with(5);
        let prefix = log.ops_up_to(5);
        let a = materialize(&prefix);
        let b = materialize(&prefix);
        assert_eq!(
            a, b,
            "the same op sequence materialises to the same bytes (deterministic)"
        );
        assert!(
            !a.is_empty(),
            "a non-empty doc materialises to non-empty state"
        );
    }

    #[test]
    fn distinct_states_materialize_distinctly() {
        let log = log_with(5);
        let s3 = materialize(&log.ops_up_to(3));
        let s5 = materialize(&log.ops_up_to(5));
        assert_ne!(
            s3, s5,
            "distinct versions materialise to distinct state bytes"
        );
    }

    #[test]
    fn materialize_carries_op_content_not_just_seq() {
        let mut a = DocOpLog::new();
        a.persist(op("c1", 1, OpKind::Insert, "alpha"));
        let mut b = DocOpLog::new();
        b.persist(op("c1", 1, OpKind::Insert, "omega"));
        let ma = materialize(&a.ops_up_to(1));
        let mb = materialize(&b.ops_up_to(1));
        assert_ne!(
            ma, mb,
            "same op_seq + different content must materialise differently (the content is framed)"
        );

        let mut c = DocOpLog::new();
        c.persist(op("ab", 1, OpKind::Insert, "cd"));
        let mut d = DocOpLog::new();
        d.persist(op("ab", 1, OpKind::Insert, "cd"));
        d.persist(op("e", 1, OpKind::Insert, ""));
        assert_ne!(
            materialize(&c.ops_up_to(2)),
            materialize(&d.ops_up_to(2)),
            "length-prefixed framing is injective - no field-boundary collision"
        );
    }

    #[test]
    fn snapshot_determinism_same_state_same_content_address() {
        let log = log_with(6);
        let blobs_a = FsBlobStore::new();
        let blobs_b = FsBlobStore::new();
        let comp_a = SnapshotCompactor::new(tenant(), "page-1", &blobs_a);
        let comp_b = SnapshotCompactor::new(tenant(), "page-1", &blobs_b);

        let snap_a = comp_a.compact(&log, 4, None).expect("compact a");
        let snap_b = comp_b.compact(&log, 4, None).expect("compact b");

        assert_eq!(
            snap_a.blob_hash, snap_b.blob_hash,
            "the same state (page-1 up to op_seq 4) mints the SAME content-address (determinism gate)"
        );
        assert_eq!(
            snap_a.blob_hash,
            content_address(&materialize(&log.ops_up_to(4))),
            "the content-address is BLAKE3(materialised state)"
        );
    }

    #[test]
    fn different_versions_get_different_content_addresses() {
        let log = log_with(6);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let s3 = comp.compact(&log, 3, None).expect("compact 3");
        let s5 = comp.compact(&log, 5, None).expect("compact 5");
        assert_ne!(
            s3.blob_hash, s5.blob_hash,
            "different versions of the doc mint different content-addresses"
        );
    }

    #[test]
    fn compaction_round_trip_is_byte_identical_after_gc() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);

        let pre_compaction_v4 = materialize(&log.ops_up_to(4));

        let snapshot = comp.compact(&log, 4, None).expect("compact up to 4");
        assert_eq!(snapshot.snap_seq, 4);
        assert_eq!(
            log.head_seq(),
            8,
            "compaction did NOT touch the op_seq counter"
        );
        assert_eq!(
            log.len(),
            8,
            "compaction did NOT prune the op-log (that is GC)"
        );

        let pruned = comp.gc(&mut log, 4, &[]);
        assert_eq!(pruned, 4, "the 4 compacted rows (op_seq 1..=4) were GC'd");
        assert_eq!(log.len(), 4, "only the live tail (op_seq 5..=8) remains");
        assert_eq!(
            log.head_seq(),
            8,
            "the op_seq counter SURVIVED the prune (monotone)"
        );

        let reconstructed = comp
            .reconstruct_at(&log, std::slice::from_ref(&snapshot), 4)
            .expect("reconstruct version 4 from snapshot + tail");
        let mismatches = if reconstructed == pre_compaction_v4 {
            0
        } else {
            1
        };
        assert_eq!(
            mismatches, 0,
            "COMPACTION-ROUND-TRIP: reconstructed version 4 is byte-identical to pre-compaction \
             (0 mismatches)"
        );
    }

    #[test]
    fn reconstruct_a_tail_version_from_snapshot_plus_tail() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let pre_v7 = materialize(&log.ops_up_to(7));

        let snapshot = comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);

        let v7 = comp
            .reconstruct_at(&log, std::slice::from_ref(&snapshot), 7)
            .expect("reconstruct version 7");
        assert_eq!(
            v7, pre_v7,
            "version 7 = snapshot(4) + tail(5..=7), byte-identical"
        );
    }

    #[test]
    fn a_gcd_range_is_reconstructable_from_the_snapshot() {
        let mut log = log_with(6);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let pre_v4 = materialize(&log.ops_up_to(4));

        let snapshot = comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);
        assert!(
            log.ops_up_to(4).iter().all(|p| p.op_seq > 4),
            "the range ≤ 4 was pruned from the log"
        );

        let v4 = comp
            .reconstruct_at(&log, std::slice::from_ref(&snapshot), 4)
            .expect("the GC'd range is reconstructable from the snapshot");
        assert_eq!(
            v4, pre_v4,
            "a GC'd range reconstructs byte-identically from the snapshot"
        );
    }

    #[test]
    fn gc_watermark_retains_rows_an_open_cursor_still_trails() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");

        let pruned = comp.gc(&mut log, 4, &[2]);
        assert_eq!(
            pruned, 2,
            "only rows ≤ 2 (below the open cursor) are pruned"
        );
        let remaining: Vec<u64> = log.ops_up_to(8).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            remaining,
            vec![3, 4, 5, 6, 7, 8],
            "rows the open cursor trails are retained"
        );

        let resumed: Vec<u64> = log.ops_since(2).iter().map(|p| p.op_seq).collect();
        assert_eq!(
            resumed,
            vec![3, 4, 5, 6, 7, 8],
            "the open client resumes with 0 ops lost (KD-1)"
        );
    }

    #[test]
    fn gc_watermark_is_the_lowest_open_cursor() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");
        let pruned = comp.gc(&mut log, 4, &[5, 2, 6]);
        assert_eq!(
            pruned, 2,
            "the lowest cursor (2) is the watermark - the most-behind client wins"
        );
    }

    #[test]
    fn gc_with_no_open_clients_prunes_the_whole_compacted_range() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");
        let pruned = comp.gc(&mut log, 4, &[]);
        assert_eq!(
            pruned, 4,
            "no open client → the whole compacted range ≤ 4 is pruned"
        );
        assert_eq!(log.len(), 4, "the live tail 5..=8 remains");
    }

    #[test]
    fn compact_beyond_head_errors_loudly() {
        let log = log_with(3);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);

        let r = comp.compact(&log, 9, None);
        assert!(matches!(
            r,
            Err(CompactionError::BeyondHead {
                requested: 9,
                head: 3
            })
        ));

        let at_head = comp
            .compact(&log, 3, None)
            .expect("compacting exactly at head succeeds");
        assert_eq!(
            at_head.snap_seq, 3,
            "the at-head snapshot covers the whole doc"
        );
        assert_eq!(
            at_head.blob_hash,
            content_address(&materialize(&log.ops_up_to(3))),
            "the at-head snapshot is the BLAKE3 of the whole materialised state"
        );
    }

    #[test]
    fn reconstruct_into_a_pruned_gap_errors_loudly() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);
        let r = comp.reconstruct_at(&log, &[], 3);
        assert!(
            matches!(r, Err(CompactionError::UnreconstructableGap { target: 3, .. })),
            "a pruned version with no covering snapshot refuses LOUDLY (0 silent wrong-version serve)"
        );
    }

    #[test]
    fn reconstruct_refuses_a_corrupt_snapshot_blob() {
        let mut log = log_with(8);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let snapshot = comp.compact(&log, 4, None).expect("compact 4");
        comp.gc(&mut log, 4, &[]);

        assert!(
            blobs.corrupt_for_drill(&tenant(), &snapshot.blob_hash),
            "snapshot blob present"
        );

        let r = comp.reconstruct_at(&log, std::slice::from_ref(&snapshot), 4);
        assert!(
            matches!(r, Err(CompactionError::Blob(_))),
            "a corrupt snapshot blob is refused (0 silent corrupt restore)"
        );
    }

    #[test]
    fn snapshot_lowers_to_the_transport_resync_seed() {
        let log = log_with(6);
        let blobs = FsBlobStore::new();
        let comp = SnapshotCompactor::new(tenant(), "page-1", &blobs);
        let snapshot = comp
            .compact(&log, 4, Some("v1.0".into()))
            .expect("named compact");
        assert_eq!(
            snapshot.named_label.as_deref(),
            Some("v1.0"),
            "a named version (restore point)"
        );

        let seed = snapshot.as_page_snapshot();
        assert_eq!(
            seed.snap_seq, 4,
            "the resync seed carries the snapshot's snap_seq"
        );
        assert_eq!(
            seed.blob_hash,
            snapshot.blob_hash.to_multihash_string(),
            "the resync seed points at the SAME content-addressed blob (one format)"
        );
    }

    #[test]
    fn errors_display_loud_and_specific() {
        assert!(CompactionError::BeyondHead {
            requested: 9,
            head: 3
        }
        .to_string()
        .contains("beyond the op-log head"));
        assert!(CompactionError::UnreconstructableGap {
            target: 3,
            lowest_available: 5
        }
        .to_string()
        .contains("refusing a non-exact reconstruction"));
        assert!(CompactionError::Blob("boom".into())
            .to_string()
            .contains("snapshot blob error"));
    }
}
