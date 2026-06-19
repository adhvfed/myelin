//! The restore-verify cross-seam half (SUB-D6, the silent-data-loss floor) — P-S26.
//!
//! See the crate-level docs for the doctrine / architecture / testing-strategy anchors. This
//! module is the **substrate's half of the restore-verify gate**: the failure-injection +
//! telemetry-assertion machinery that DRIVES SUB-D6 / STOR-D1 / STOR-D2 and makes "rebuild from
//! backups → no loss, one consistent cross-seam point" PROVABLE. Storage owns the WAL+PITR
//! restore and the CI-wired `restore-verify` job (contract 11.5; its M1 follow-ons are P-ST-11/
//! P-ST-12/P-ST-13 → globals P-059/P-060/P-061); **this** is the cross-seam consistency
//! assertion + the RPO/RTO measurement + the drill scenario every restore-touching change
//! re-runs.
//!
//! ## The four seams the restore must land at ONE consistent point (architecture §11 D-6)
//! A rebuild-from-backups is correct only when the four stores restore to a **single consistent
//! cross-seam point** — the event-log offset is the cross-seam cursor (storage.md §7.3; contract
//! 11.5 "event-log offset = the cross-seam cursor"):
//! 1. **OLTP rows** — the authoritative domain state (each row carries the offset it was last
//!    written at, and may reference a blob by its content hash).
//! 2. **Blob store** — content-addressed objects an OLTP row points at by hash. *No row may point
//!    at a missing blob* (the headline D-6 invariant).
//! 3. **Search index** — derived docs (rebuilt-not-restored: reindexed from source). *No index doc
//!    may outlive the OLTP row it projects* (a resurrected/orphan doc is a mismatch).
//! 4. **Event-log offsets** — the cross-seam cursor. *No store may hold data past the restored
//!    offset* (data beyond the consistency point is a row the restore should not have — a
//!    forward-inconsistency, the shape a sloppy PITR produces).
//!
//! The assertion [`RestoredSnapshot::verify_cross_seam`] walks all four and returns a typed
//! [`CrossSeamReport`] enumerating EVERY mismatch (never a swallowed pass): a consistent rebuild
//! has zero mismatches; a rebuild with a row→missing-blob (or an orphan index doc, or a
//! past-offset row) is REJECTED — the assertion MUST reject an inconsistent rebuild (the unit
//! test the prompt names).
//!
//! ## Why an abstract snapshot model, not a dependency on `myelin-storage`
//! The harness is test-support that sits ABOVE `myelin-substrate` as a leaf consumer (crate-level
//! docs); `myelin-storage` sits BELOW the substrate. The cross-seam assertion is therefore
//! modelled over an abstract [`RestoredSnapshot`] (the four seams as plain value data keyed by the
//! event-log offset) rather than a concrete `BlobStore`/OLTP handle — so the harness does not pull
//! a below-substrate crate up into test-support, and the assertion surface does not change when
//! Storage's real WAL/PITR restore lands (P-059..P-061): that restore will *populate* a
//! [`RestoredSnapshot`] off the real stores; this assertion reads it identically. The contract is
//! the cross-seam INVARIANT, not a wire format.
//!
//! ## The RPO/RTO half (architecture §11 D-6 / STOR-D2; thresholds 11.5)
//! [`RestoreOutcome`] carries the MEASURED RPO (seconds of tail data lost) and RTO (wall-clock to
//! a consistent, ready copy, per grain). The drill reads its bounds from the thresholds file
//! (`rpo_max_mins` / `rto_tenant_max_mins` / `rto_cell_max_mins`) — NEVER a hardcoded number — and
//! [`RestoreOutcome::record_into`] writes the measured numbers onto the telemetry source so the
//! drill asserts them green via the SAME [`crate::telemetry::SignalSource`] every other drill uses.
//! **Never weaken RPO/RTO to pass** (EI-01 §3): a red is a dated claimed-not-proven scorecard row.
//!
//! ## Floors named (deferred + filling prompt)
//! - **No real WAL/PITR restore yet.** Storage's WAL archiving + base backups + PITR (P-059), the
//!   `restore(to_offset T)` consistency-point rebuild (P-060), and the CI-wired restore-verify gate
//!   (P-061, STOR-D1 — the permanent gate) are the M1 follow-ons that land AFTER this prompt. This
//!   module is the **assertion + RPO/RTO + drill machinery** the substrate owes; when P-059..P-061
//!   land they drive a real rebuild into a [`RestoredSnapshot`] and re-point the drill at it — the
//!   cross-seam invariant + the RPO/RTO bound do not change. The drill here runs at the M1
//!   single-tenant scale against a modelled rebuild (the dated green artifact the DoD names).
//! - **Cell-scale re-confirm is the M5 follow-on (P-S35 → P-435 family).** This prompt proves
//!   SUB-D6/STOR-D1/STOR-D2 at single-tenant scale; the world-scale-load cell-scale re-drive is
//!   P-S35 (named in the prompt). The same [`RestoredSnapshot`] machinery is re-driven there at
//!   cell scale; nothing in the assertion shape changes.
//! - **Post-restore RE-ERASURE** (the key stays destroyed across a restore — STOR-D3 / GA-16) is
//!   the GDPR/Storage follow-on (P-100 / P-115), not this prompt: the cross-seam invariant here is
//!   "no row → missing blob, no orphan doc, no past-offset row"; the "no resurrected erased
//!   subject" invariant rides the erasure-ledger seam those prompts own. Named, not silent.

use crate::telemetry::{Label, SignalName, SignalSource};
use std::collections::{BTreeMap, BTreeSet};

/// The event-log offset — the cross-seam cursor (storage.md §7.3; contract 11.5). A monotone
/// per-aggregate sequence number; the restore lands every store at ONE such offset (the
/// consistency point). Modelled as a `u64` here (the substrate-neutral cursor shape; the real
/// per-aggregate `seq` the OLTP co-location establishes is `myelin-storage`'s `ColocatedTx`,
/// P-016 — re-stated as a scalar cursor here, not imported, since the harness does not depend on
/// storage).
pub type Offset = u64;

/// A content-address (the blob hash an OLTP row references). Modelled as a `String` (the
/// self-describing multihash form `<algo>:<hex>` `myelin-storage::ContentHash` produces, P-047) —
/// re-stated as an opaque address here so the cross-seam assertion does not depend on the storage
/// crate. Two rows referencing the same address point at the same blob.
pub type BlobAddr = String;

/// One restored OLTP row — the authoritative domain state a rebuild lands. Carries the offset it
/// was last written at (so the assertion can reject a row PAST the restored consistency point) and
/// the blob address it references, if any (so the assertion can reject a row → MISSING blob).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OltpRow {
    /// A stable row id (PII-free in tests — a synthetic key; the real id is the aggregate key).
    pub id: String,
    /// The event-log offset this row was last written at. A row whose offset exceeds the restored
    /// consistency point is a forward-inconsistency (the restore should not have it).
    pub written_at: Offset,
    /// The blob this row references by content address, if any. `Some(addr)` MUST resolve in the
    /// restored blob store (no row → missing blob — the headline D-6 invariant).
    pub blob_ref: Option<BlobAddr>,
}

/// One restored search-index doc — derived state (reindexed-from-source, not restored). Keyed to
/// the OLTP row it projects; a doc whose source row is absent from the restored OLTP set is an
/// ORPHAN (a resurrected/leftover projection — a cross-seam mismatch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDoc {
    /// The id of the OLTP row this doc projects (the join key). MUST exist in the restored OLTP
    /// set — an index doc must never outlive its source row.
    pub source_row_id: String,
}

/// A snapshot of the four restore seams after a rebuild-from-backups, all at ONE event-log offset
/// (the [`Self::restored_to_offset`] consistency point). The cross-seam consistency assertion
/// [`Self::verify_cross_seam`] walks all four and reports every mismatch.
///
/// In tests the rig populates this directly (the modelled rebuild). When Storage's real WAL/PITR
/// restore lands (P-059..P-061) it populates the SAME shape off the real stores; the assertion
/// does not change. Built via [`RestoredSnapshot::builder`].
#[derive(Clone, Debug, Default)]
pub struct RestoredSnapshot {
    /// The consistency point: every store is restored to THIS offset. A store holding data past it
    /// is a forward-inconsistency the assertion rejects.
    pub restored_to_offset: Offset,
    /// The restored OLTP rows (the authoritative domain state).
    pub oltp_rows: Vec<OltpRow>,
    /// The content addresses present in the restored blob store (a row's `blob_ref` must be one
    /// of these — else it is a row → missing blob).
    pub blob_addrs: BTreeSet<BlobAddr>,
    /// The restored/reindexed search-index docs (each must project a present OLTP row).
    pub index_docs: Vec<IndexDoc>,
}

impl RestoredSnapshot {
    /// Start building a snapshot restored to `offset` (the consistency point).
    pub fn builder(restored_to_offset: Offset) -> RestoredSnapshotBuilder {
        RestoredSnapshotBuilder {
            snapshot: RestoredSnapshot {
                restored_to_offset,
                ..Default::default()
            },
        }
    }

    /// **The cross-seam consistency assertion (the substrate's half of SUB-D6 / STOR-D1).** Walk
    /// all four seams and return a typed [`CrossSeamReport`] enumerating EVERY mismatch — never a
    /// swallowed pass. A consistent rebuild has zero mismatches ([`CrossSeamReport::is_consistent`]
    /// is `true`); ANY of the three cross-seam invariant violations rejects the rebuild:
    ///
    /// 1. **row → missing blob** — an OLTP row references a blob address absent from the restored
    ///    blob store (the headline D-6 invariant: *no row pointing at a missing blob*).
    /// 2. **orphan index doc** — a search doc projects an OLTP row absent from the restored OLTP
    ///    set (a resurrected/leftover projection — an index doc must not outlive its source row).
    /// 3. **past-offset row** — an OLTP row was written PAST the restored consistency point (the
    ///    restore should not hold data beyond the offset it restored to — a forward-inconsistency).
    pub fn verify_cross_seam(&self) -> CrossSeamReport {
        let mut mismatches = Vec::new();
        let row_ids: BTreeSet<&str> = self.oltp_rows.iter().map(|r| r.id.as_str()).collect();

        for row in &self.oltp_rows {
            // (1) no row → missing blob.
            if let Some(addr) = &row.blob_ref {
                if !self.blob_addrs.contains(addr) {
                    mismatches.push(CrossSeamMismatch::RowMissingBlob {
                        row_id: row.id.clone(),
                        blob_addr: addr.clone(),
                    });
                }
            }
            // (3) no store holds data past the restored consistency point.
            if row.written_at > self.restored_to_offset {
                mismatches.push(CrossSeamMismatch::RowPastOffset {
                    row_id: row.id.clone(),
                    written_at: row.written_at,
                    restored_to_offset: self.restored_to_offset,
                });
            }
        }
        for doc in &self.index_docs {
            // (2) no index doc outlives its source OLTP row.
            if !row_ids.contains(doc.source_row_id.as_str()) {
                mismatches.push(CrossSeamMismatch::OrphanIndexDoc {
                    source_row_id: doc.source_row_id.clone(),
                });
            }
        }
        CrossSeamReport { mismatches }
    }
}

/// A builder for a [`RestoredSnapshot`] (the rig's modelled rebuild). Fluent so a test reads as
/// the cross-seam story it tells (a row with a blob, the blob present/absent, an index doc on a
/// present/absent row).
#[derive(Debug)]
pub struct RestoredSnapshotBuilder {
    snapshot: RestoredSnapshot,
}

impl RestoredSnapshotBuilder {
    /// Add an OLTP row written at `written_at`, optionally referencing `blob_ref`.
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

    /// Record that a blob with content address `addr` is present in the restored blob store.
    pub fn blob(mut self, addr: impl Into<BlobAddr>) -> Self {
        self.snapshot.blob_addrs.insert(addr.into());
        self
    }

    /// Add a search-index doc projecting the OLTP row `source_row_id`.
    pub fn index_doc(mut self, source_row_id: impl Into<String>) -> Self {
        self.snapshot.index_docs.push(IndexDoc {
            source_row_id: source_row_id.into(),
        });
        self
    }

    /// Finish, yielding the snapshot.
    pub fn build(self) -> RestoredSnapshot {
        self.snapshot
    }
}

/// One cross-seam inconsistency found in a restored snapshot. Each names EXACTLY what is wrong
/// (observability is part of the pass condition, EI-01 §3) so a red report points at the precise
/// row/blob/doc, never a bare "inconsistent".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossSeamMismatch {
    /// An OLTP row references a blob address absent from the restored blob store — the headline
    /// D-6 invariant violation (*no row pointing at a missing blob*).
    RowMissingBlob {
        /// The offending row.
        row_id: String,
        /// The content address it points at, which is absent from the restored blob store.
        blob_addr: BlobAddr,
    },
    /// A search-index doc projects an OLTP row absent from the restored OLTP set — a resurrected /
    /// leftover projection (an index doc must not outlive its source row).
    OrphanIndexDoc {
        /// The source row id the doc projects, which is absent from the restored OLTP set.
        source_row_id: String,
    },
    /// An OLTP row was written PAST the restored consistency point — the restore holds data beyond
    /// the offset it restored to (a forward-inconsistency).
    RowPastOffset {
        /// The offending row.
        row_id: String,
        /// The offset it was written at (greater than the restored offset).
        written_at: Offset,
        /// The restored consistency point it exceeds.
        restored_to_offset: Offset,
    },
}

/// The typed result of [`RestoredSnapshot::verify_cross_seam`] — every mismatch found (empty ⇒
/// the rebuild landed at ONE consistent cross-seam point). Never a bare `bool`: a red carries the
/// precise mismatches so the drill (and a later real restore) can name what broke.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a cross-seam verification result must be checked — a dropped inconsistency is a swallowed data-loss bug (EI-01 §3)"]
pub struct CrossSeamReport {
    /// Every cross-seam inconsistency found (empty ⇒ consistent).
    pub mismatches: Vec<CrossSeamMismatch>,
}

impl CrossSeamReport {
    /// `true` iff the rebuild landed at ONE consistent cross-seam point (zero mismatches). The
    /// ONLY way to read a pass — a non-empty report is never silently a pass.
    pub fn is_consistent(&self) -> bool {
        self.mismatches.is_empty()
    }

    /// The number of cross-seam mismatches — the value the drill asserts `== 0` via
    /// [`SignalName::RestoreCrossSeamMismatch`]. `0` ⇒ 0 loss, one consistent point.
    pub fn mismatch_count(&self) -> i64 {
        self.mismatches.len() as i64
    }
}

/// The grain an RTO objective is read at (architecture §11 D-6 / STOR-D2; thresholds 11.5):
/// per-tenant (≤ 1 h) or per-cell (≤ 4 h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtoGrain {
    /// Per-tenant recovery (the `rto_tenant_max_mins` bound — ≤ 1 h default-to-beat).
    Tenant,
    /// Per-cell recovery (the `rto_cell_max_mins` bound — ≤ 4 h default-to-beat).
    Cell,
}

impl RtoGrain {
    /// The `{grain}` telemetry label value for [`SignalName::RestoreRtoSecs`].
    pub fn label_value(self) -> &'static str {
        match self {
            RtoGrain::Tenant => "tenant",
            RtoGrain::Cell => "cell",
        }
    }
}

/// The measured outcome of a rebuild-from-backups: the cross-seam consistency report + the
/// MEASURED RPO (tail seconds lost) and per-grain RTO (wall-clock to a consistent, ready copy).
/// The drill reads its bounds from the thresholds file and asserts these green; never weaken
/// RPO/RTO to pass (EI-01 §3).
#[derive(Clone, Debug)]
pub struct RestoreOutcome {
    /// The cross-seam consistency report (0 mismatches ⇒ one consistent point).
    pub cross_seam: CrossSeamReport,
    /// The MEASURED recovery-POINT objective: seconds of committed tail data the restore lost
    /// (the gap between the last durably-backed offset and the crash point). Asserted
    /// `<= rpo_max_mins * 60`.
    pub rpo_secs: u64,
    /// The MEASURED recovery-TIME objective per grain, in seconds (wall-clock from "begin restore"
    /// to "consistent, ready copy"). Asserted against the per-grain bound from the thresholds file.
    pub rto_secs: BTreeMap<&'static str, u64>,
}

impl RestoreOutcome {
    /// Build an outcome from a verified snapshot + the measured RPO and per-grain RTO numbers.
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

    /// The measured RTO for a grain, if recorded.
    pub fn rto_for(&self, grain: RtoGrain) -> Option<u64> {
        self.rto_secs.get(grain.label_value()).copied()
    }

    /// **Record the measured restore signals onto the telemetry source** so the drill asserts them
    /// green via the SAME [`SignalSource`] every other drill uses (observability is part of the
    /// pass — a restore that lands consistent but emits no signal has failed the drill, EI-01 §3).
    /// Writes:
    /// - [`SignalName::RestoreCrossSeamMismatch`] = the mismatch count (asserted `== 0`),
    /// - [`SignalName::RestoreRpoSecs`] = the measured RPO seconds (asserted `<= rpo bound`),
    /// - [`SignalName::RestoreRtoSecs`]`{grain}` = the per-grain RTO seconds (asserted `<= rto bound`).
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

    /// A consistent rebuild — every row's blob present, every index doc on a present row, no row
    /// past the offset — reports ZERO mismatches (one consistent cross-seam point). The happy path
    /// SUB-D6 asserts green.
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

    /// **THE UNIT TEST THE PROMPT NAMES:** the cross-seam assertion CATCHES a deliberately-injected
    /// row → missing blob mismatch — the assertion MUST reject an inconsistent rebuild (never a
    /// silent pass). The headline D-6 invariant.
    #[test]
    fn assertion_rejects_a_row_pointing_at_a_missing_blob() {
        // r2 references blake3:bbbb, but the blob is NOT in the restored store (the injected
        // mismatch: a row → missing blob — the silent-data-loss shape a sloppy restore produces).
        let snap = RestoredSnapshot::builder(100)
            .blob("blake3:aaaa")
            .row("r1", 90, Some("blake3:aaaa".into()))
            .row("r2", 95, Some("blake3:bbbb".into())) // missing blob
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

    /// The assertion also catches an ORPHAN index doc (a projection whose source OLTP row is absent
    /// — a resurrected/leftover doc). An index doc must not outlive its source row.
    #[test]
    fn assertion_rejects_an_orphan_index_doc() {
        let snap = RestoredSnapshot::builder(100)
            .row("r1", 90, None)
            .index_doc("r1")
            .index_doc("r2") // no r2 row → orphan
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

    /// The assertion catches a row written PAST the restored consistency point (a
    /// forward-inconsistency — the restore holds data beyond the offset it restored to).
    #[test]
    fn assertion_rejects_a_row_past_the_restored_offset() {
        let snap = RestoredSnapshot::builder(100)
            .row("r1", 90, None)
            .row("r2", 140, None) // past the offset 100
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

    /// `record_into` writes the measured restore signals onto the telemetry source so a drill
    /// asserts them green via the SAME assertion surface every drill uses (observability is part of
    /// the pass). A consistent rebuild within RPO/RTO reads green.
    #[test]
    fn record_into_writes_the_restore_signals_for_assertion() {
        let snap = RestoredSnapshot::builder(100)
            .blob("blake3:aaaa")
            .row("r1", 100, Some("blake3:aaaa".into()))
            .build();
        let outcome = RestoreOutcome::new(
            snap.verify_cross_seam(),
            120,                                       // 2 min RPO
            &[(RtoGrain::Tenant, 1800), (RtoGrain::Cell, 7200)], // 30 min / 2 h
        );
        let mut signals = SignalSource::new();
        outcome.record_into(&mut signals);

        // 0 cross-seam mismatches
        signals
            .assert_signal(SignalName::RestoreCrossSeamMismatch, Predicate::Eq(0))
            .expect_green();
        // RPO within 5 min (300 s) — using the threshold value here as a literal; the DRILL reads
        // it from the thresholds file (myelin-substrate test).
        signals
            .assert_signal(SignalName::RestoreRpoSecs, Predicate::Lte(300))
            .expect_green();
        // per-tenant RTO within 1 h (3600 s)
        signals
            .assert_labelled(
                SignalName::RestoreRtoSecs,
                vec![Label::new("grain", "tenant")],
                Predicate::Lte(3600),
            )
            .expect_green();
        // per-cell RTO within 4 h (14400 s)
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

    /// An inconsistent rebuild's mismatch count is `> 0`, so the restore-verify telemetry assertion
    /// reads RED (`RestoreCrossSeamMismatch == 0` fails) — the silent-data-loss floor catches it.
    #[test]
    fn an_inconsistent_rebuild_reads_red_on_the_telemetry_assertion() {
        let snap = RestoredSnapshot::builder(100)
            .row("r1", 95, Some("blake3:missing".into())) // row → missing blob
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
