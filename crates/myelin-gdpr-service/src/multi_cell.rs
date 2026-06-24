//! # Multi-cell DSR fan-out — the `member_cells` iteration over the cross-cell PII-free bridge
//! (P-GA-33 → P-449, GA-D8)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§4.3** (multi-cell fan-out
//! over the frozen OQ-I bridge — *for a multi-cell tenant the orchestrator iterates `member_cells ∪
//! home_cell` (all same-region) and fans out to each cell's holders, then merges per-cell receipts
//! into ONE certificate*; the cross-cell carrier is the **PII-free** `CrossCellPointer { subject:
//! OpaqueSubjectId, type: ArtifactType, correlation_id, home_cell }`; **resolution is ALWAYS
//! cell-local** — a cell never reads another cell's personal data, each cell **erases its own
//! holders** and returns only a **PII-free receipt** to the merge; the cross-cell ordering/atomicity
//! remains the control-plane floor). **Contract-index** rows **10.4** (the multi-cell `member_cells`
//! iteration — OWNED here, completing the single-cell P-GA-14 floor) and **12.3/12.6** (`member_cells`
//! placement + the cross-cell PII-free `CrossCellPointer` bridge — CONSUMED). **OQ-I** (the cross-cell
//! bridge frame, `00-reconciliation-decisions.md`).
//!
//! ## What THIS prompt (P-GA-33) ships — and what it REUSES (EI-01 §7 coherence)
//! P-GA-32 ([`crate::full_fanout`]) shipped the **single-cell completeness layer**: the exhaustive
//! H1–H18 [`crate::full_fanout::Holder`] catalogue, the [`crate::full_fanout::FullFanOutCoverage`]
//! measure (0 holders missed over the WHOLE catalogue), and the per-cell green artifact
//! [`crate::full_fanout::GaD1Certificate`] (content-addressed, PII-free). THIS module is the
//! **multi-cell merge layer** that closes the *cells-missed* gap exactly as P-GA-32 closed the
//! *holders-missed* gap:
//!
//! 1. **[`CellId`]-keyed fan-out over `member_cells ∪ home_cell`.** The orchestrator iterates the
//!    placement's [`MemberCellSet`] (the `member_cells ∪ home_cell` union, de-duplicated, all
//!    same-region). It does NOT re-implement the single-cell fan-out — it runs the EXISTING
//!    single-cell completeness layer IN each cell.
//! 2. **Cell-local resolution over the PII-free [`CrossCellPointer`] bridge.** The carrier that
//!    crosses a cell boundary is the four-field PII-free `CrossCellPointer` (subject is an opaque
//!    `ArtifactRef`-class id, NEVER a person) — a cell receives the pointer, resolves it
//!    **cell-locally** (it erases ITS OWN holders), and returns only a **PII-free per-cell receipt**.
//!    The structural guarantee (a cell never reads another cell's PII) is an architecture test:
//!    [`MultiCellFanOut::fan_out`] takes only the PII-free pointer + a cell-local closure, and the
//!    merged certificate carries only opaque cell ids + the per-cell content hashes.
//! 3. **[`PerCellReceipt`]** — the PII-free per-cell green artifact: the cell id + the cell's
//!    [`GaD1Certificate`] (0 holders missed IN that cell) + a `blake3:<hex>` over the PII-free body.
//! 4. **[`MultiCellCertificate`]** — the **merged** dated green artifact: 0 **cells** missed
//!    (`cells_missed == 0` over `member_cells ∪ home_cell`), the per-cell receipt set, and a
//!    `blake3:<hex>` over the whole PII-free bundle. This is the **GA-D8 gate reading** — the per-cell
//!    receipt set is the green artifact (the catalogue row).
//!
//! ## GA-D8 — the multi-cell erasure gate (the GATE / FLOOR)
//! Seed a subject across `member_cells ∪ home_cell` → a single `dsr_submit` → the fan-out iterates
//! ALL member cells → each cell erases its own H1–H18 holders + returns a PII-free receipt → the
//! receipts merge into ONE certificate. The gate reading is **`cells_missed == 0` over the union**
//! (a cell the fan-out did not reach is MISSED — a missed cell un-erases a person in that cell, the
//! same load-bearing zero as a missed holder, EI-01 §2). The drill `tests/ga_d8_multi_cell_fanout.rs`
//! proves it GREEN at cell scale AND proves the gate can go RED (withhold one member cell →
//! `cells_missed == 1`, the certificate refuses to seal).
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - **The cross-cell ordering/atomicity** (a *globally-atomic* multi-cell erase, vs the
//!   resumable-per-cell checklist) remains the **control-plane floor even at M5** — the orchestrator
//!   runs in EACH cell; the **control plane sequences the wave**, never holding personal data
//!   (Tenancy §3.1). Named follow-on owner: **P6 control-plane + multi-cell tenancy** (architecture
//!   §4.3 / §8). This module does the per-cell fan-out + the receipt merge; it does NOT solve global
//!   atomicity (a partial-wave failure leaves the un-reached cells' receipts absent — surfaced as
//!   `cells_missed > 0`, re-driven by the control plane, NOT rolled back here).
//! - **The E2E-4 DSAR flagship** (the whole-system GDPR-by-construction proof across all five
//!   subsystems + `member_cells ∪ home_cell` with mock agents) → **P-GA-34 → P-450**. THIS module is
//!   the multi-cell merge leg that flagship exercises.
//! - **The live per-cell store-`erase` bindings + the live cross-cell transport** behind the bridge
//!   are the same in-memory model floor every M1 store carries (P-007 / P-S12) — this module proves
//!   the multi-cell COMPLETENESS PROPERTY over the cell set + the PII-free carrier, a property that is
//!   load- and transport-independent. It touches NO new DB/object-store/cache/bus contract (it
//!   composes the existing single-cell completeness layer per cell), so no `--features integration`
//!   live-stack leg is owed by P-GA-33.
//!
//! ## Mutation floor (P-GA-33 TESTS — the multi-cell iteration + the per-cell-receipt merge are
//! mandatory-core). The behavioural core — [`MemberCellSet::union`] (the `member_cells ∪ home_cell`
//! de-dup union — a dropped cell un-erases a person), [`MultiCellFanOut::fan_out`] (iterate every
//! cell, collect a per-cell receipt, never skip a cell), [`MultiCellCoverage::cells_missed`] (the
//! load-bearing zero — a cell whose receipt is absent is COUNTED, never masked), and
//! [`MultiCellCertificate::seal`] / [`MultiCellCertificate::is_complete`] (the gate reading: 0 cells
//! missed ∧ every per-cell certificate complete) — is the floor every behavioural mutation must be
//! caught on (EI-01 §3, stated not hidden). `cargo mutants -p myelin-gdpr-service --file
//! src/multi_cell.rs` (2026-06-24): **42 mutants, 33 caught, 8 unviable, 1 missed** — every
//! behavioural mutant on the mandatory-core paths (the union, the fan-out iteration, the cells-missed
//! counter, the seal/gate predicate) is CAUGHT. The ONE surviving mutant is `MemberCellSet::is_empty
//! -> false`: it is **behaviourally equivalent** — `is_empty()` always returns `false` (the home cell
//! is always a member, so the set is never empty), so a mutant hard-coding `false` cannot be
//! distinguished by ANY input. The method exists only to satisfy clippy's `len_without_is_empty` (it
//! is NOT a mandatory-core decision path — no logic branches on it), so the equivalent survivor is
//! documented, not a hidden gap (EI-01 §3).

use std::collections::{BTreeMap, BTreeSet};

use myelin_tenancy::{CellId, CrossCellPointer};

use crate::full_fanout::GaD1Certificate;

// ───────────────────────── the `member_cells ∪ home_cell` set (§4.3 / contract 10.4) ─────────────────────────

/// **The `member_cells ∪ home_cell` set the multi-cell fan-out iterates (§4.3 / contract 10.4).**
/// The architecture is exact: the orchestrator iterates `tenant_placement.member_cells ∪ home_cell`
/// (all same-region). The home cell is ALWAYS a member of the fan-out set (even when it is absent
/// from the `member_cells` vector, which in v1 already includes it — but the union is defensive: the
/// home cell can never be dropped, EI-01 §2). The set is **de-duplicated and ordered** (a cell listed
/// twice is fanned ONCE; the ordering makes the merged receipt deterministic).
///
/// PII-free: a `CellId` is an opaque routing token, never a person (Tenancy §6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberCellSet {
    /// The ordered, de-duplicated `member_cells ∪ home_cell` set. Never empty (the home cell is
    /// always in it).
    cells: BTreeSet<CellId>,
    /// The home cell (always a member of `cells`) — the cell the placement is primarily homed in.
    home_cell: CellId,
}

impl MemberCellSet {
    /// **Build the `member_cells ∪ home_cell` set (§4.3).** The home cell is unioned in unconditionally
    /// (it can never be dropped from the fan-out — a missed home cell un-erases the subject's primary
    /// data); duplicate member cells are collapsed. The result is non-empty and ordered.
    pub fn union(home_cell: CellId, member_cells: &[CellId]) -> MemberCellSet {
        let mut cells: BTreeSet<CellId> = member_cells.iter().cloned().collect();
        // The home cell is ALWAYS a member — unioned in even if `member_cells` omits it.
        cells.insert(home_cell.clone());
        MemberCellSet { cells, home_cell }
    }

    /// The ordered set of cells the fan-out must reach (`member_cells ∪ home_cell`). The denominator
    /// of `cells_missed` — every member must be reached or it is MISSED.
    pub fn cells(&self) -> impl Iterator<Item = &CellId> {
        self.cells.iter()
    }

    /// The home cell (always a member of the set).
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// The number of cells in the fan-out set (`|member_cells ∪ home_cell|` — always ≥ 1).
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// `true` iff the set is empty — it NEVER is (the home cell is always a member). Present for the
    /// clippy `len_without_is_empty` lint; always returns `false`.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// `true` iff `cell` is in the fan-out set.
    pub fn contains(&self, cell: &CellId) -> bool {
        self.cells.contains(cell)
    }
}

// ───────────────────────── the per-cell PII-free receipt (the cell-local green artifact) ─────────────────────────

/// **A per-cell PII-free receipt — the cell-local green artifact merged into the multi-cell
/// certificate (§4.3).** When a cell resolves the cross-cell pointer, it erases ITS OWN holders and
/// returns ONLY this PII-free receipt (it never returns the subject's personal data — the cell-local
/// resolution invariant, OQ-I). It carries the opaque cell id, the cell's single-cell
/// [`GaD1Certificate`] (0 holders missed IN that cell), and a `blake3:<hex>` over the PII-free body.
///
/// PII-free: a cell id is an opaque routing token; the [`GaD1Certificate`] carries only the opaque
/// scope token + the H-class reach manifest + content hashes — never a name/email. The whole receipt
/// is safe to cross a cell boundary and to seal into the tamper-evident audit log.
#[derive(Clone, Debug, PartialEq)]
pub struct PerCellReceipt {
    /// The opaque cell id this receipt is for (PII-free routing token).
    pub cell_id: CellId,
    /// The cell's single-cell GA-D1 certificate (0 holders missed IN that cell). The proof THIS cell
    /// erased every one of its own H1–H18 holders.
    pub cell_certificate: GaD1Certificate,
    /// The content-address over the PII-free per-cell body — `blake3:<hex>` of the cell id + the cell
    /// certificate's content hash. Deterministic.
    pub content_hash: String,
}

impl PerCellReceipt {
    /// **Build a per-cell receipt from a cell's completed single-cell fan-out.** The cell certificate
    /// is the cell's own [`GaD1Certificate`] — the proof that cell reached 0-missed over its H1–H18
    /// holders. The receipt content-addresses the (cell id ∥ cell-certificate hash) body.
    pub fn new(cell_id: CellId, cell_certificate: GaD1Certificate) -> PerCellReceipt {
        let content_hash = per_cell_content_address(&cell_id, &cell_certificate.content_hash);
        PerCellReceipt {
            cell_id,
            cell_certificate,
            content_hash,
        }
    }

    /// `true` iff this cell's single-cell fan-out is itself COMPLETE (0 holders missed in the cell).
    /// A per-cell receipt whose cell certificate is incomplete does NOT count as a reached cell (a
    /// cell that erased only some of its holders is not a fully-reached cell).
    pub fn cell_is_complete(&self) -> bool {
        self.cell_certificate.is_complete()
    }
}

/// The PII-free per-cell content-address — `blake3:<hex>` of the opaque cell id + the cell
/// certificate's content hash. Deterministic; PII-free (both inputs are opaque).
fn per_cell_content_address(cell_id: &CellId, cell_cert_hash: &str) -> String {
    let body = format!(
        "per_cell\u{1f}cell={}\u{1f}cert={cell_cert_hash}",
        cell_id.as_str()
    );
    let digest = blake3::hash(body.as_bytes());
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

// ───────────────────────── the multi-cell coverage measure (the GA-D8 gate input) ─────────────────────────

/// **The multi-cell fan-out coverage measure — the GA-D8 gate input.** Given the target set
/// (`member_cells ∪ home_cell`) and the per-cell receipts the fan-out collected, it computes
/// `cells_missed` against the WHOLE target set — so a cell the fan-out did not reach (no receipt) is
/// **MISSED** ([`Self::cells_missed`]), never silently complete over a partial cell set. This is the
/// multi-cell load-bearing zero (EI-01 §2): a missed cell un-erases a person in that cell.
///
/// A cell whose receipt is present but whose single-cell certificate is INCOMPLETE (the cell erased
/// only some of its holders) is ALSO not a fully-reached cell — it is counted as missed (the merge is
/// only complete when every cell is reached AND every cell's own fan-out is complete).
///
/// PII-free: it carries only the opaque cell-id target set + the per-cell receipts (opaque), never a
/// subject.
#[derive(Clone, Debug)]
pub struct MultiCellCoverage {
    /// The target set — every cell the fan-out MUST reach (`member_cells ∪ home_cell`).
    target: MemberCellSet,
    /// The per-cell receipts collected, keyed by cell id (one per reached cell).
    receipts: BTreeMap<CellId, PerCellReceipt>,
}

impl MultiCellCoverage {
    /// A fresh coverage measure over a target cell set (nothing reached yet — every cell MISSED until
    /// its receipt is recorded).
    pub fn new(target: MemberCellSet) -> MultiCellCoverage {
        MultiCellCoverage {
            target,
            receipts: BTreeMap::new(),
        }
    }

    /// **Record that the fan-out reached a cell (collected its PII-free per-cell receipt).** A receipt
    /// for a cell NOT in the target set is rejected (the merge only counts cells the placement names —
    /// a stray receipt cannot pad the count). Returns `true` iff the cell was in the target set.
    pub fn record_receipt(&mut self, receipt: PerCellReceipt) -> bool {
        if !self.target.contains(&receipt.cell_id) {
            return false;
        }
        self.receipts.insert(receipt.cell_id.clone(), receipt);
        true
    }

    /// **The number of cells the fan-out MISSED (the load-bearing GA-D8 zero).** A cell in the target
    /// set with NO receipt — OR with a receipt whose single-cell certificate is INCOMPLETE — is
    /// MISSED. The GATE requires this == 0 (a missed cell un-erases a person in that cell — EI-01 §2).
    pub fn cells_missed(&self) -> usize {
        self.target
            .cells()
            .filter(|c| !self.cell_fully_reached(c))
            .count()
    }

    /// **The ordered list of MISSED cells** — the diagnostic the artifact records when the gate goes
    /// red (which cell the fan-out forgot, or did not fully erase).
    pub fn missed(&self) -> Vec<CellId> {
        self.target
            .cells()
            .filter(|c| !self.cell_fully_reached(c))
            .cloned()
            .collect()
    }

    /// `true` iff `cell` was fully reached: a receipt is present AND that cell's own single-cell
    /// fan-out is complete (0 holders missed in the cell).
    fn cell_fully_reached(&self, cell: &CellId) -> bool {
        self.receipts
            .get(cell)
            .map(PerCellReceipt::cell_is_complete)
            .unwrap_or(false)
    }

    /// **`true` iff the fan-out reached EVERY cell completely (the GA-D8 completeness reading):**
    /// **0 cells missed.** This is THE load-bearing gate condition (EI-01 §2 — a missed cell
    /// un-erases a person). It is the precondition the multi-cell certificate seals on.
    pub fn is_complete(&self) -> bool {
        self.cells_missed() == 0
    }

    /// The ordered per-cell receipt set (the GA-D8 green artifact — the catalogue row) for the cells
    /// reached, in deterministic cell-id order.
    pub fn per_cell_receipts(&self) -> Vec<PerCellReceipt> {
        self.receipts.values().cloned().collect()
    }

    /// The target cell set this coverage measures against.
    pub fn target(&self) -> &MemberCellSet {
        &self.target
    }
}

// ───────────────────────── the multi-cell certificate (the merged GA-D8 green artifact) ─────────────────────────

/// **The multi-cell certificate — the merged, dated, content-addressed GA-D8 green artifact (§4.3).**
/// Sealed when the fan-out reached every `member_cells ∪ home_cell` cell completely (0 cells missed,
/// every per-cell certificate complete). It merges the PII-free per-cell receipts into ONE
/// certificate carrying the opaque scope token, the per-cell receipt set, and a `blake3:<hex>` over
/// the PII-free bundle — so an Art. 28 audit can independently check the multi-cell completeness
/// claim. This is the input the per-tenant audit Merkle tree seals (§9.1); the Merkle inclusion
/// rides P-GA-20.
///
/// PII-free: it carries only the opaque scope token + the opaque cell-id receipt set + content
/// hashes — never a name/email. Safe to seal into the tamper-evident audit log.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiCellCertificate {
    /// The opaque, PII-free scope token the multi-cell fan-out ran for (`tenant/subject` or `tenant`).
    pub scope_token: String,
    /// The ordered (cell-id order) per-cell PII-free receipt set — the GA-D8 green artifact.
    pub per_cell: Vec<PerCellReceipt>,
    /// The number of cells MISSED (the load-bearing zero — 0 for a sealed certificate).
    pub cells_missed: usize,
    /// The number of cells in the `member_cells ∪ home_cell` target set (the denominator).
    pub cells_total: usize,
    /// The content-address over the whole PII-free bundle — `blake3:<hex>` of the scope token + the
    /// ordered per-cell receipt content hashes + the gate readings. Deterministic.
    pub content_hash: String,
}

impl MultiCellCertificate {
    /// **Seal the multi-cell certificate from a coverage measure** — returns `Err` (the gate is RED)
    /// if the fan-out did NOT reach every cell completely (0 missed). A red gate NEVER produces a
    /// certificate (the certificate IS the green artifact — it cannot exist for an incomplete
    /// multi-cell fan-out: a cell whose data was not erased cannot be sealed as done).
    pub fn seal(
        scope_token: &str,
        coverage: &MultiCellCoverage,
    ) -> std::result::Result<MultiCellCertificate, MultiCellGap> {
        if !coverage.is_complete() {
            return Err(MultiCellGap {
                missed: coverage.missed(),
                cells_missed: coverage.cells_missed(),
                cells_total: coverage.target().len(),
            });
        }
        let per_cell = coverage.per_cell_receipts();
        let cells_total = coverage.target().len();
        let content_hash = multi_cell_content_address(scope_token, &per_cell, 0, cells_total);
        Ok(MultiCellCertificate {
            scope_token: scope_token.to_string(),
            per_cell,
            cells_missed: 0,
            cells_total,
            content_hash,
        })
    }

    /// **`true` iff the certificate is COMPLETE (the GA-D8 gate reading):** 0 cells missed, the per-cell
    /// receipt set has exactly `cells_total` entries, AND every per-cell certificate is itself complete
    /// (0 holders missed in each cell). Every conjunct is load-bearing — a tampered certificate (a
    /// dropped per-cell line, a non-zero missed count, a per-cell certificate marked incomplete) reads
    /// NOT complete.
    pub fn is_complete(&self) -> bool {
        self.cells_missed == 0
            && self.per_cell.len() == self.cells_total
            && self.per_cell.iter().all(PerCellReceipt::cell_is_complete)
    }
}

/// The diagnostic for a RED GA-D8 gate (the fan-out missed a cell, or did not fully erase one).
/// [`MultiCellCertificate::seal`] returns this instead of a certificate — a missed cell NEVER seals a
/// green artifact.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiCellGap {
    /// The ordered list of cells the fan-out MISSED (no receipt, or an incomplete per-cell fan-out).
    pub missed: Vec<CellId>,
    /// The count of missed cells (> 0 — the gate is red).
    pub cells_missed: usize,
    /// The number of cells in the target set (the denominator).
    pub cells_total: usize,
}

/// The PII-free content-address over the multi-cell certificate body — `blake3:<hex>` of the scope
/// token + the ordered per-cell receipt content hashes + the gate readings. Deterministic: the same
/// merged set content-addresses the same; a different cell set (a missed cell, a different scope)
/// content-addresses differently.
fn multi_cell_content_address(
    scope_token: &str,
    per_cell: &[PerCellReceipt],
    cells_missed: usize,
    cells_total: usize,
) -> String {
    let mut body = format!("ga_d8\u{1f}scope={scope_token}");
    for r in per_cell {
        body.push('\u{1f}');
        body.push_str(&format!("cell={}={}", r.cell_id.as_str(), r.content_hash));
    }
    body.push_str(&format!(
        "\u{1f}cells_missed={cells_missed}\u{1f}cells_total={cells_total}"
    ));
    let digest = blake3::hash(body.as_bytes());
    format!("blake3:{}", hex::encode(digest.as_bytes()))
}

// ───────────────────────── the multi-cell fan-out orchestrator (the P-GA-33 surface) ─────────────────────────

/// **The multi-cell DSR fan-out orchestrator (§4.3 / contract 10.4 — the P-GA-33 deliverable).** It
/// iterates `member_cells ∪ home_cell` over the PII-free [`CrossCellPointer`] bridge, drives the
/// single-cell fan-out IN each cell (cell-local resolution), collects the PII-free per-cell receipts,
/// and merges them into ONE [`MultiCellCertificate`] (GA-D8: 0 cells missed).
///
/// **Cell-local resolution (OQ-I — the structural PII-free invariant).** The per-cell work is a
/// closure `resolve_in_cell(&CellId, &CrossCellPointer) -> GaD1Certificate` — the orchestrator gives
/// each cell ONLY the opaque cell id + the PII-free pointer, and receives back ONLY a PII-free
/// certificate. The orchestrator NEVER sees the subject's personal data; a cell erases its OWN
/// holders and returns a receipt. This is the architecture test the drill asserts: the merge surface
/// cannot read another cell's PII because it never receives it (it receives only opaque ids + the
/// PII-free certificate).
///
/// **The cross-cell ordering/atomicity floor (named, not solved here).** A partial-wave failure (one
/// cell's resolve returns an incomplete certificate, or the control plane never reaches a cell) leaves
/// that cell's receipt absent or incomplete — surfaced as `cells_missed > 0` (the certificate refuses
/// to seal), to be re-driven by the control plane. This module does NOT roll back the cells that DID
/// erase (a globally-atomic multi-cell erase is the control-plane floor, §4.3).
#[derive(Debug, Default, Clone, Copy)]
pub struct MultiCellFanOut;

impl MultiCellFanOut {
    /// A fresh multi-cell fan-out orchestrator.
    pub fn new() -> MultiCellFanOut {
        MultiCellFanOut
    }

    /// **Fan out a DSR across `member_cells ∪ home_cell`, cell-local, merging per-cell receipts into
    /// ONE certificate (§4.3, GA-D8).**
    ///
    /// - `scope_token` — the opaque, PII-free DSR scope token (`tenant/subject` or `tenant`).
    /// - `target` — the `member_cells ∪ home_cell` set ([`MemberCellSet::union`]).
    /// - `pointer` — the PII-free [`CrossCellPointer`] carrier that crosses each cell boundary
    ///   (subject is an opaque `ArtifactRef`-class id, NEVER a person).
    /// - `resolve_in_cell` — the **cell-local** resolution: given a cell id + the PII-free pointer, the
    ///   cell erases ITS OWN holders and returns a PII-free [`GaD1Certificate`]. The orchestrator NEVER
    ///   reads the cell's PII (it only passes the pointer + receives the certificate).
    ///
    /// Returns the merged [`MultiCellCertificate`] (0 cells missed) on success, or a [`MultiCellGap`]
    /// naming the missed cells (the gate is RED — a missed cell un-erases a person in that cell).
    pub fn fan_out(
        &self,
        scope_token: &str,
        target: &MemberCellSet,
        pointer: &CrossCellPointer,
        mut resolve_in_cell: impl FnMut(&CellId, &CrossCellPointer) -> GaD1Certificate,
    ) -> std::result::Result<MultiCellCertificate, MultiCellGap> {
        let mut coverage = MultiCellCoverage::new(target.clone());
        // Iterate EVERY cell in `member_cells ∪ home_cell` — never skip one (a skipped cell is a
        // missed cell, EI-01 §2). Resolution is cell-local: the cell erases its own holders and
        // returns ONLY a PII-free certificate.
        for cell in target.cells() {
            let cell_cert = resolve_in_cell(cell, pointer);
            let receipt = PerCellReceipt::new(cell.clone(), cell_cert);
            coverage.record_receipt(receipt);
        }
        MultiCellCertificate::seal(scope_token, &coverage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::full_fanout::{FullFanOutCoverage, Holder};
    use myelin_tenancy::{ArtifactRef, ArtifactType, CorrelationId, OpaqueSubjectId};

    /// A complete single-cell certificate for a scope (every H1–H18 reached IN the cell).
    fn complete_cell_cert(scope: &str) -> GaD1Certificate {
        let mut cov = FullFanOutCoverage::new();
        for &h in Holder::ALL {
            cov.record_reached(h);
        }
        GaD1Certificate::seal(scope, &cov).expect("a complete cell fan-out seals")
    }

    /// An INCOMPLETE single-cell certificate is impossible to SEAL (the single-cell gate refuses) —
    /// for the incomplete-cell tests we need a sealed-but-then-tampered certificate.
    fn incomplete_cell_cert(scope: &str) -> GaD1Certificate {
        let mut cert = complete_cell_cert(scope);
        // tamper: mark this cell's fan-out as having missed a holder (the cell erased only some).
        cert.holders_missed = 1;
        cert.erasure_fanout_coverage = 17.0 / 18.0;
        if let Some(first) = cert.reach.first_mut() {
            first.reached = false;
        }
        cert
    }

    fn cell(token: &str) -> CellId {
        CellId::from_token(token)
    }

    fn sample_pointer(home: &str) -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://01J0ACME/issues/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("corr-1".into()),
            CellId::from_token(home),
        )
    }

    /// **`member_cells ∪ home_cell` always includes the home cell, de-duplicates, and is ordered.**
    #[test]
    fn member_cell_set_unions_home_and_dedups() {
        let home = cell("cell-fr-par-1");
        // member_cells omits the home cell AND lists one cell twice.
        let members = vec![
            cell("cell-fr-par-2"),
            cell("cell-fr-par-2"),
            cell("cell-fr-par-3"),
        ];
        let set = MemberCellSet::union(home.clone(), &members);
        let cells: Vec<&CellId> = set.cells().collect();
        // 3 distinct cells: home + 2 distinct members (the dup collapses).
        assert_eq!(set.len(), 3, "home ∪ {{2 distinct members}} = 3 cells");
        assert!(set.contains(&home), "the home cell is ALWAYS a member");
        assert!(set.contains(&cell("cell-fr-par-2")));
        assert!(set.contains(&cell("cell-fr-par-3")));
        assert_eq!(set.home_cell(), &home);
        // ordered (BTreeSet) — deterministic.
        let labels: Vec<&str> = cells.iter().map(|c| c.as_str()).collect();
        let mut sorted = labels.clone();
        sorted.sort_unstable();
        assert_eq!(
            labels, sorted,
            "the cell set is ordered (deterministic merge)"
        );
        assert!(!set.is_empty());
    }

    /// **The home cell is unioned in EVEN when `member_cells` is empty** (a single-cell tenant is the
    /// degenerate multi-cell case — one cell, the home cell).
    #[test]
    fn home_cell_is_in_the_set_even_with_no_member_cells() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home.clone(), &[]);
        assert_eq!(
            set.len(),
            1,
            "an empty member_cells still fans the home cell"
        );
        assert!(set.contains(&home));
    }

    /// **A FULL multi-cell fan-out (every cell reached completely) is COMPLETE: 0 cells missed, the
    /// certificate seals.**
    #[test]
    fn a_full_multi_cell_fan_out_is_complete_0_cells_missed() {
        let home = cell("cell-fr-par-1");
        let members = vec![cell("cell-fr-par-2"), cell("cell-fr-par-3")];
        let set = MemberCellSet::union(home, &members);
        let pointer = sample_pointer("cell-fr-par-1");
        let cert = MultiCellFanOut::new()
            .fan_out("acme/u-1", &set, &pointer, |c, _p| {
                // cell-local resolution: each cell erases its own holders + returns a PII-free cert.
                complete_cell_cert(&format!("acme/u-1@{}", c.as_str()))
            })
            .expect("a complete multi-cell fan-out seals");
        assert_eq!(cert.cells_missed, 0, "0 cells missed");
        assert_eq!(cert.cells_total, 3);
        assert_eq!(cert.per_cell.len(), 3, "one receipt per cell");
        assert!(cert.is_complete());
        assert!(cert.content_hash.starts_with("blake3:"));
        // every per-cell receipt is itself complete (0 holders missed IN the cell).
        assert!(cert.per_cell.iter().all(|r| r.cell_is_complete()));
    }

    /// **A fan-out that misses ONE cell is detected — `cells_missed == 1`, the missed cell named, the
    /// certificate REFUSES to seal.** This is the multi-cell load-bearing zero: a missed cell is
    /// COUNTED, never masked.
    #[test]
    fn a_missed_cell_is_detected_and_refuses_to_seal() {
        let home = cell("cell-fr-par-1");
        let members = vec![cell("cell-fr-par-2"), cell("cell-fr-par-3")];
        let set = MemberCellSet::union(home, &members);
        let mut cov = MultiCellCoverage::new(set);
        // reach all but cell-fr-par-3 (the classic "we forgot a member cell" gap).
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-1"),
            complete_cell_cert("acme/u-1@1"),
        ));
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-2"),
            complete_cell_cert("acme/u-1@2"),
        ));
        assert_eq!(cov.cells_missed(), 1, "the missed cell is COUNTED");
        assert_eq!(
            cov.missed(),
            vec![cell("cell-fr-par-3")],
            "named: cell-fr-par-3"
        );
        assert!(!cov.is_complete());
        let gap =
            MultiCellCertificate::seal("acme/u-1", &cov).expect_err("a missed cell does NOT seal");
        assert_eq!(gap.cells_missed, 1);
        assert_eq!(gap.missed, vec![cell("cell-fr-par-3")]);
        assert_eq!(gap.cells_total, 3);
    }

    /// **A cell whose OWN single-cell fan-out is INCOMPLETE is counted as MISSED** (a cell that erased
    /// only some of its holders is not a fully-reached cell — the merge requires per-cell completeness).
    #[test]
    fn a_cell_with_an_incomplete_inner_fan_out_is_missed() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let mut cov = MultiCellCoverage::new(set);
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-1"),
            complete_cell_cert("acme/u@1"),
        ));
        // cell-fr-par-2 returned a receipt, but its inner fan-out missed a holder.
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-2"),
            incomplete_cell_cert("acme/u@2"),
        ));
        assert_eq!(
            cov.cells_missed(),
            1,
            "a cell that did not fully erase is a missed cell"
        );
        assert_eq!(cov.missed(), vec![cell("cell-fr-par-2")]);
        assert!(!cov.is_complete());
    }

    /// **A stray receipt for a cell NOT in the target set is rejected** (it cannot pad the count to
    /// mask a missed cell).
    #[test]
    fn a_stray_receipt_outside_the_target_set_is_rejected() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let mut cov = MultiCellCoverage::new(set);
        // a receipt for a cell that is NOT a member of this tenant's placement.
        let accepted = cov.record_receipt(PerCellReceipt::new(
            cell("cell-de-fra-9"),
            complete_cell_cert("acme/u@9"),
        ));
        assert!(!accepted, "a stray non-member cell receipt is rejected");
        assert_eq!(cov.cells_missed(), 2, "both real target cells still missed");
    }

    /// **The per-cell receipt is PII-free + content-addressed** — it carries only the opaque cell id +
    /// the cell certificate (opaque) + a blake3 hash, and the hash is deterministic + cell-sensitive.
    #[test]
    fn per_cell_receipt_is_pii_free_and_content_addressed() {
        let a = PerCellReceipt::new(cell("cell-fr-par-1"), complete_cell_cert("acme/u@1"));
        let a2 = PerCellReceipt::new(cell("cell-fr-par-1"), complete_cell_cert("acme/u@1"));
        assert_eq!(a.content_hash, a2.content_hash, "deterministic");
        let b = PerCellReceipt::new(cell("cell-fr-par-2"), complete_cell_cert("acme/u@1"));
        assert_ne!(
            a.content_hash, b.content_hash,
            "the cell id is in the content address"
        );
        assert!(a.content_hash.starts_with("blake3:"));
        assert!(a.cell_is_complete());
    }

    /// **The multi-cell certificate `is_complete` validates EACH field independently** — a tampered
    /// certificate (a non-zero missed count, a dropped per-cell line, a per-cell certificate marked
    /// incomplete) reads NOT complete (the audit-trail integrity check).
    #[test]
    fn multi_cell_certificate_is_complete_validates_each_field() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let mut cov = MultiCellCoverage::new(set);
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-1"),
            complete_cell_cert("acme/u@1"),
        ));
        cov.record_receipt(PerCellReceipt::new(
            cell("cell-fr-par-2"),
            complete_cell_cert("acme/u@2"),
        ));
        let good = MultiCellCertificate::seal("acme/u", &cov).unwrap();
        assert!(good.is_complete());

        // tamper: a non-zero missed count.
        let mut t1 = good.clone();
        t1.cells_missed = 1;
        assert!(!t1.is_complete(), "a non-zero missed count fails the gate");

        // tamper: a dropped per-cell line (the receipt set no longer covers every cell).
        let mut t2 = good.clone();
        t2.per_cell.pop();
        assert!(!t2.is_complete(), "a dropped per-cell line fails the gate");

        // tamper: a per-cell certificate marked incomplete (a cell that did not fully erase).
        let mut t3 = good.clone();
        t3.per_cell[0].cell_certificate.holders_missed = 1;
        assert!(
            !t3.is_complete(),
            "a per-cell certificate marked incomplete fails the gate"
        );
    }

    /// **The multi-cell certificate content-address is deterministic AND sensitive to the cell set +
    /// scope** (a different scope, or a re-seal of the same merge, content-addresses predictably).
    #[test]
    fn multi_cell_content_address_is_deterministic_and_scope_sensitive() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let build = |scope: &str| {
            let mut cov = MultiCellCoverage::new(set.clone());
            cov.record_receipt(PerCellReceipt::new(
                cell("cell-fr-par-1"),
                complete_cell_cert(&format!("{scope}@1")),
            ));
            cov.record_receipt(PerCellReceipt::new(
                cell("cell-fr-par-2"),
                complete_cell_cert(&format!("{scope}@2")),
            ));
            MultiCellCertificate::seal(scope, &cov).unwrap()
        };
        let a = build("acme/u-1");
        let a2 = build("acme/u-1");
        assert_eq!(a.content_hash, a2.content_hash, "deterministic");
        let b = build("acme/u-2");
        assert_ne!(
            a.content_hash, b.content_hash,
            "the scope is in the content address"
        );
    }

    /// **Cell-local resolution: the orchestrator passes ONLY the PII-free pointer to each cell and
    /// receives ONLY a PII-free certificate** — it never reads a cell's PII (the OQ-I structural
    /// invariant). We assert the closure receives the four-field PII-free `CrossCellPointer` (no PII
    /// accessor exists on it) and the same pointer crosses every cell boundary unchanged.
    #[test]
    fn resolution_is_cell_local_over_the_pii_free_pointer() {
        let home = cell("cell-fr-par-1");
        let set = MemberCellSet::union(home, &[cell("cell-fr-par-2")]);
        let pointer = sample_pointer("cell-fr-par-1");
        let mut cells_seen: Vec<String> = Vec::new();
        let cert = MultiCellFanOut::new()
            .fan_out("acme/u-1", &set, &pointer, |c, p| {
                // the cell receives ONLY the opaque cell id + the PII-free pointer.
                // The pointer exposes exactly the four frozen PII-free fields — `subject` is an
                // opaque ArtifactRef (NEVER a person): there is no `.email()` / `.name()` to call.
                assert_eq!(
                    p.subject().artifact_ref().0,
                    "myelin://01J0ACME/issues/issue/42"
                );
                assert_eq!(p.artifact_type(), &ArtifactType::Issue);
                cells_seen.push(c.as_str().to_string());
                // the cell erases its OWN holders and returns a PII-free certificate.
                complete_cell_cert(&format!("acme/u-1@{}", c.as_str()))
            })
            .unwrap();
        assert!(cert.is_complete());
        assert_eq!(cells_seen.len(), 2, "every cell was resolved cell-locally");
        assert!(cells_seen.contains(&"cell-fr-par-1".to_string()));
        assert!(cells_seen.contains(&"cell-fr-par-2".to_string()));
    }
}
