use crate::telemetry::{Label, SignalName, SignalSource};
use std::collections::{BTreeMap, BTreeSet};

pub type Offset = u64;

pub type BlobAddr = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OltpRow {
    pub id: String,
    pub written_at: Offset,
    pub blob_ref: Option<BlobAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDoc {
    pub source_row_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct RestoredSnapshot {
    pub restored_to_offset: Offset,
    pub oltp_rows: Vec<OltpRow>,
    pub blob_addrs: BTreeSet<BlobAddr>,
    pub index_docs: Vec<IndexDoc>,
}

impl RestoredSnapshot {
    pub fn builder(restored_to_offset: Offset) -> RestoredSnapshotBuilder {
        RestoredSnapshotBuilder {
            snapshot: RestoredSnapshot {
                restored_to_offset,
                ..Default::default()
            },
        }
    }

    pub fn verify_cross_seam(&self) -> CrossSeamReport {
        let mut mismatches = Vec::new();
        let row_ids: BTreeSet<&str> = self.oltp_rows.iter().map(|r| r.id.as_str()).collect();

        for row in &self.oltp_rows {
            if let Some(addr) = &row.blob_ref {
                if !self.blob_addrs.contains(addr) {
                    mismatches.push(CrossSeamMismatch::RowMissingBlob {
                        row_id: row.id.clone(),
                        blob_addr: addr.clone(),
                    });
                }
            }
            if row.written_at > self.restored_to_offset {
                mismatches.push(CrossSeamMismatch::RowPastOffset {
                    row_id: row.id.clone(),
                    written_at: row.written_at,
                    restored_to_offset: self.restored_to_offset,
                });
            }
        }
        for doc in &self.index_docs {
            if !row_ids.contains(doc.source_row_id.as_str()) {
                mismatches.push(CrossSeamMismatch::OrphanIndexDoc {
                    source_row_id: doc.source_row_id.clone(),
                });
            }
        }
        CrossSeamReport { mismatches }
    }
}

#[derive(Debug)]
pub struct RestoredSnapshotBuilder {
    snapshot: RestoredSnapshot,
}

impl RestoredSnapshotBuilder {
    pub fn row(
        mut self,
        id: impl Into<String>,
        written_at: Offset,
        blob_ref: Option<BlobAddr>,
    ) -> Self {
        self.snapshot.oltp_rows.push(OltpRow {
            id: id.into(),
            written_at,
            blob_ref,
        });
        self
    }

    pub fn blob(mut self, addr: impl Into<BlobAddr>) -> Self {
        self.snapshot.blob_addrs.insert(addr.into());
        self
    }

    pub fn index_doc(mut self, source_row_id: impl Into<String>) -> Self {
        self.snapshot.index_docs.push(IndexDoc {
            source_row_id: source_row_id.into(),
        });
        self
    }

    pub fn build(self) -> RestoredSnapshot {
        self.snapshot
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossSeamMismatch {
    RowMissingBlob {
        row_id: String,
        blob_addr: BlobAddr,
    },
    OrphanIndexDoc {
        source_row_id: String,
    },
    RowPastOffset {
        row_id: String,
        written_at: Offset,
        restored_to_offset: Offset,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a cross-seam verification result must be checked - a dropped inconsistency is a swallowed data-loss bug (EI-01 §3)"]
pub struct CrossSeamReport {
    pub mismatches: Vec<CrossSeamMismatch>,
}

impl CrossSeamReport {
    pub fn is_consistent(&self) -> bool {
        self.mismatches.is_empty()
    }

    pub fn mismatch_count(&self) -> i64 {
        self.mismatches.len() as i64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtoGrain {
    Tenant,
    Cell,
}

impl RtoGrain {
    pub fn label_value(self) -> &'static str {
        match self {
            RtoGrain::Tenant => "tenant",
            RtoGrain::Cell => "cell",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RestoreOutcome {
    pub cross_seam: CrossSeamReport,
    pub rpo_secs: u64,
    pub rto_secs: BTreeMap<&'static str, u64>,
}

impl RestoreOutcome {
    pub fn new(
        cross_seam: CrossSeamReport,
        rpo_secs: u64,
        rto: &[(RtoGrain, u64)],
    ) -> RestoreOutcome {
        let rto_secs = rto
            .iter()
            .map(|(grain, secs)| (grain.label_value(), *secs))
            .collect();
        RestoreOutcome {
            cross_seam,
            rpo_secs,
            rto_secs,
        }
    }

    pub fn rto_for(&self, grain: RtoGrain) -> Option<u64> {
        self.rto_secs.get(grain.label_value()).copied()
    }

    pub fn record_into(&self, signals: &mut SignalSource) {
        signals.set_scalar(
            SignalName::RestoreCrossSeamMismatch,
            self.cross_seam.mismatch_count(),
        );
        signals.set_scalar(SignalName::RestoreRpoSecs, self.rpo_secs as i64);
        for (grain, secs) in &self.rto_secs {
            signals.set_labelled(
                SignalName::RestoreRtoSecs,
                vec![Label::new("grain", *grain)],
                *secs as i64,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::Predicate;

    #[test]
    fn a_consistent_rebuild_lands_at_one_cross_seam_point() {
        let snap = RestoredSnapshot::builder(100)
            .blob("blake3:aaaa")
            .blob("blake3:bbbb")
            .row("r1", 90, Some("blake3:aaaa".into()))
            .row("r2", 100, Some("blake3:bbbb".into()))
            .row("r3", 50, None)
            .index_doc("r1")
            .index_doc("r2")
            .build();

        let report = snap.verify_cross_seam();
        assert!(
            report.is_consistent(),
            "a consistent rebuild must report zero mismatches, got {:?}",
            report.mismatches
        );
        assert_eq!(report.mismatch_count(), 0);
    }

    #[test]
    fn assertion_rejects_a_row_pointing_at_a_missing_blob() {
        let snap = RestoredSnapshot::builder(100)
            .blob("blake3:aaaa")
            .row("r1", 90, Some("blake3:aaaa".into()))
            .row("r2", 95, Some("blake3:bbbb".into()))
            .build();

        let report = snap.verify_cross_seam();
        assert!(
            !report.is_consistent(),
            "a row pointing at a missing blob MUST be rejected, not pass silently"
        );
        assert_eq!(report.mismatch_count(), 1);
        assert_eq!(
            report.mismatches[0],
            CrossSeamMismatch::RowMissingBlob {
                row_id: "r2".into(),
                blob_addr: "blake3:bbbb".into(),
            }
        );
    }

    #[test]
    fn assertion_rejects_an_orphan_index_doc() {
        let snap = RestoredSnapshot::builder(100)
            .row("r1", 90, None)
            .index_doc("r1")
            .index_doc("r2")
            .build();

        let report = snap.verify_cross_seam();
        assert!(!report.is_consistent());
        assert_eq!(
            report.mismatches,
            vec![CrossSeamMismatch::OrphanIndexDoc {
                source_row_id: "r2".into(),
            }]
        );
    }

    #[test]
    fn assertion_rejects_a_row_past_the_restored_offset() {
        let snap = RestoredSnapshot::builder(100)
            .row("r1", 90, None)
            .row("r2", 140, None)
            .build();

        let report = snap.verify_cross_seam();
        assert!(!report.is_consistent());
        assert_eq!(
            report.mismatches,
            vec![CrossSeamMismatch::RowPastOffset {
                row_id: "r2".into(),
                written_at: 140,
                restored_to_offset: 100,
            }]
        );
    }

    #[test]
    fn record_into_writes_the_restore_signals_for_assertion() {
        let snap = RestoredSnapshot::builder(100)
            .blob("blake3:aaaa")
            .row("r1", 100, Some("blake3:aaaa".into()))
            .build();
        let outcome = RestoreOutcome::new(
            snap.verify_cross_seam(),
            120,
            &[(RtoGrain::Tenant, 1800), (RtoGrain::Cell, 7200)],
        );
        let mut signals = SignalSource::new();
        outcome.record_into(&mut signals);

        signals
            .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
            .expect_green();
        signals
            .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(300))
            .expect_green();
        signals
            .assert_labelled(
                SignalName::RestoreRtoSecs,
                vec![Label::new("grain", "tenant")],
                Predicate::Lte(3600),
            )
            .expect_green();
        signals
            .assert_labelled(
                SignalName::RestoreRtoSecs,
                vec![Label::new("grain", "cell")],
                Predicate::Lte(14400),
            )
            .expect_green();

        assert_eq!(outcome.rto_for(RtoGrain::Tenant), Some(1800));
        assert_eq!(outcome.rto_for(RtoGrain::Cell), Some(7200));
    }

    #[test]
    fn an_inconsistent_rebuild_reads_red_on_the_telemetry_assertion() {
        let snap = RestoredSnapshot::builder(100)
            .row("r1", 95, Some("blake3:missing".into()))
            .build();
        let outcome = RestoreOutcome::new(snap.verify_cross_seam(), 60, &[(RtoGrain::Tenant, 600)]);
        let mut signals = SignalSource::new();
        outcome.record_into(&mut signals);

        let verdict = signals.assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0));
        assert!(
            !verdict.is_green(),
            "an inconsistent rebuild MUST read RED on the cross-seam mismatch assertion"
        );
    }
}
