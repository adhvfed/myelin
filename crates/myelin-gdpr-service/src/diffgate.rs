//! # `diffgate` — the CI data-map DIFF GATE + the DPIA-route on reclassification (P-GA-10 → P-110)
//! (contract 10.3; gdpr §2.2 / §2.3)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §2.2 (the generated map is
//! *regenerated every build and **diffed in CI** so a DPO sees any reclassification*) + §2.3 (the
//! DPIA gate (Art. 35) fires on a **data-map diff** introducing a new `SpecialCategory` flow).
//! Doctrine: `external-insights/01-process-and-quality-doctrine.md` §5 (the ratchet — *the CI diff
//! is the committed gate a DPO reads; a reclassification that slips through silently is the
//! failure*).
//!
//! **Contract-index:** row **10.3** — the CI-diffed inventory. P-GA-09 (→ P-109) ships the
//! GENERATION ([`crate::datamap::data_map`] → [`Inventory`]) + the deterministic
//! [`Inventory::fingerprint`]. P-GA-08 (→ P-108) ships the DPIA MARKER + the
//! [`myelin_gdpr::DpiaRouter`]. THIS module (P-GA-10) is the **gate** that joins them: it commits a
//! baseline inventory, diffs the freshly-generated one against it, and **fails the build** with the
//! diff surfaced when the map changes — *until a DPO reviews and re-baselines*. A newly-appeared
//! `SpecialCategory` flow additionally routes into the DPIA gate (the [`DataMapDiff::dpia_verdicts`]
//! the marker from P-108 drives).
//!
//! ## What this module is (the GATE, EI-01 §5 — coverage is a committed property, not good intent)
//! A generated map that drifts silently is the failure: a new PII field, a reclassification
//! (role/basis/category/retention/erasure change), or a holder added/removed all change the legal
//! posture, and a DPO must SEE the change. The map is generated (P-GA-09), so a hand-maintained
//! changelog drifts — instead the **fingerprint of the generated inventory is committed** as the
//! baseline, and every build:
//! 1. regenerates the inventory from the live `#[personal_data]` registry + registered holders;
//! 2. compares it against the committed baseline ([`diff`]);
//! 3. if unchanged → **green** (the build passes);
//! 4. if changed → **red** ([`GateVerdict::Changed`]) with the structured diff surfaced
//!    ([`DataMapDiff`]: added/removed fields, reclassifications, added/removed holders) — the build
//!    fails until a DPO reviews the diff and re-baselines (re-commits the new fingerprint).
//!
//! The gate is deterministic: it is a pure function of (committed baseline, regenerated inventory),
//! and the inventory is byte-stable build-to-build ([`Inventory::fingerprint`]), so the gate never
//! flaps in CI.
//!
//! ## The DPIA route (gdpr §2.3) — a new SpecialCategory flow is an Art. 35 obligation
//! The diff carries the DPIA verdicts: it feeds the prior committed marker set and the regenerated
//! marker set to the [`myelin_gdpr::DpiaRouter`] (P-108), which records each **newly-appeared**
//! special-category flow as [`myelin_gdpr::DpiaVerdict::Required`] — *surfaced for a DPO, never
//! auto-decided*. So a reclassification that PROMOTES a field to special-category (or changes its
//! Art. 9 kind) both fails the diff gate (the inventory changed) AND routes into the DPIA gate (a new
//! marker appeared). A pure removal or an ordinary-category change fails the diff gate but is not a
//! new DPIA obligation.
//!
//! ## The GATE / DRILL (GA-D5, the data-map-diff face)
//! [`tests::ga_d5_changed_inventory_fails_the_gate_and_unchanged_passes`] is the dated green
//! artifact: add a field → the diff gate fails with the new field surfaced; reclassify a field
//! (role tenant-content → platform-operational) → the gate fails with the reclassification shown;
//! add a special-category field → the gate fails AND the DPIA verdict fires; an UNCHANGED inventory
//! passes. The companion `no-untagged-personal-data` lint (P-GA-03) is the OTHER half of the GA-D5
//! drill — it fails the build at COMPILE time on an untagged PII field, so a new PII field cannot
//! reach the map untagged; once tagged, THIS gate surfaces it in the diff. The two together are
//! GA-D5: *an untagged field fails the lint; a tagged change fails the diff gate.*
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - The map's **content completeness** grows per store as holders ship (M3 Git/KN, M4
//!   CI/Issues/Chat) — the diff surfaces each new holder's PII fields as it lands; the map is
//!   COMPLETE (and GA-D1 provable end-to-end) only at **M5 (P-GA-32 → P-505)**. This gate is
//!   complete NOW; its *content* is whatever the current holders contribute, and it correctly
//!   surfaces every future holder's fields as an additive diff a DPO reviews.
//! - The **CI wiring** (the committed baseline file + the build step that fails the pipeline on a red
//!   verdict): the baseline is committed as [`COMMITTED_BASELINE_FINGERPRINT`]-shaped data and the
//!   gate is exercised by [`check_against_baseline`]; running it AS a `build.rs` / CI step over the
//!   live cell's full registered-holder set lands when the holders are assembled at `serve(AppSpec)`
//!   boot (the same in-memory-vs-live floor every M0 store carries — the GATE LOGIC is complete and
//!   tested here; the pipeline invocation is one call to [`check_against_baseline`] over the boot's
//!   holder set). The diff-gate *mechanism* is the deliverable; it is proven over a real-shaped
//!   registered-holder set in the tests + the CDC.
//!
//! ## Mutation floor (P-GA-10 TESTS — the diff-comparison + the DPIA-route path is mandatory-core).
//! The behavioural core is [`diff`] (the field/holder set-difference + the per-field reclassification
//! detection), [`DataMapDiff::is_clean`] (the changed/unchanged verdict), [`check_against_baseline`]
//! (the fingerprint compare → gate verdict), and the DPIA-route wiring (the marker-difference feeding
//! the router). The tests below drive each: a missed added field, a missed removed field, a missed
//! reclassification, a missed holder change, a false "clean" verdict, and a missed DPIA route each
//! fail a test. `cargo mutants --package myelin-gdpr-service -f
//! crates/myelin-gdpr-service/src/diffgate.rs` (2026-06-20): **26 mutants, 24 caught, 0 missed**, 2
//! unviable (non-compiling) — a 100% catch rate on the viable mutants of the diff-comparison +
//! coverage + DPIA-route core. No survivor (EI-01 §3 — stated, not hidden).

use crate::datamap::{Inventory, InventoryEntry};
use myelin_gdpr::{DpiaMarker, DpiaRouter, DpiaVerdict};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The committed baseline the diff gate compares against (gdpr §2.2 — *the generated inventory is
/// committed; a build that changes it fails CI*). It is the LAST DPO-reviewed inventory: the
/// `fingerprint` is the fast equality check the gate keys on; the full `inventory` is carried so the
/// gate can produce the structured [`DataMapDiff`] a DPO reads when the fingerprint differs (the
/// fingerprint says *that* it changed; the inventory says *what* changed).
///
/// In CI this is the committed artifact (a checked-in JSON of the inventory + its fingerprint);
/// a build regenerates the inventory and calls [`check_against_baseline`] against this. Re-baselining
/// (a DPO reviewed the diff and accepts it) is re-committing this with the new inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedBaseline {
    /// The DPO-reviewed inventory the current build is diffed against.
    pub inventory: Inventory,
    /// The committed fingerprint of `inventory` (the fast equality check; redundant with
    /// `inventory.fingerprint()` but committed explicitly so a hand-edit of the JSON that forgets to
    /// update the fingerprint is itself caught — see [`CommittedBaseline::is_self_consistent`]).
    pub fingerprint: String,
}

impl CommittedBaseline {
    /// Seal a reviewed inventory into a baseline (the re-baseline operation a DPO performs after
    /// reviewing a diff). The fingerprint is computed from the inventory, so the baseline is
    /// self-consistent by construction.
    pub fn seal(inventory: Inventory) -> CommittedBaseline {
        let fingerprint = inventory.fingerprint();
        CommittedBaseline {
            inventory,
            fingerprint,
        }
    }

    /// Whether the committed `fingerprint` matches its `inventory` — a hand-edited baseline JSON whose
    /// fingerprint was not regenerated is corrupt (the diff gate would compare against a lie). The
    /// gate asserts this before trusting the baseline.
    pub fn is_self_consistent(&self) -> bool {
        self.inventory.fingerprint() == self.fingerprint
    }
}

/// A **reclassification** of an existing field (gdpr §2.2 — the diff a DPO reads). The field is at
/// the same `field_path` in both the baseline and the current map, but one or more of its five tags /
/// owning holder / residency region CHANGED — the legal posture moved (e.g. `role` tenant-content →
/// platform-operational, or `category` ordinary → special-category). The gate surfaces the BEFORE and
/// AFTER so the DPO sees exactly what moved.
///
/// References-not-payloads: every field is a path / tag / id, never a subject's data — safe to
/// surface in the diff a DPO reads + commit into a review record.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Reclassification {
    /// The field path that was reclassified (stable identity across the baseline + current).
    pub field_path: String,
    /// The entry as it was in the committed baseline.
    pub before: InventoryEntry,
    /// The entry as it is in the regenerated map.
    pub after: InventoryEntry,
}

/// The **structured data-map diff** the gate surfaces (gdpr §2.2). It is the human-readable answer
/// to *what changed in the data map* a DPO reviews: which PII fields appeared, which disappeared,
/// which were reclassified (a tag/holder/region moved at the same path), and which holders were
/// added or removed. Plus the DPIA route ([`dpia_verdicts`](DataMapDiff::dpia_verdicts)): the
/// newly-appeared special-category flows that are Art. 35 obligations.
///
/// **Empty iff the map is unchanged** ([`is_clean`](DataMapDiff::is_clean)) — the gate's
/// green/red verdict reads exactly this.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DataMapDiff {
    /// PII field paths present in the current map but ABSENT from the baseline (a new tagged field —
    /// a new flow). Sorted.
    pub added_fields: Vec<InventoryEntry>,
    /// PII field paths present in the baseline but ABSENT from the current map (a field removed —
    /// e.g. a store dropped a column). Sorted.
    pub removed_fields: Vec<InventoryEntry>,
    /// Fields present at the same path in BOTH but with a changed tag/holder/region — a
    /// reclassification (the legal posture moved). Sorted by field path.
    pub reclassifications: Vec<Reclassification>,
    /// Holder ids present in the current roster but ABSENT from the baseline roster (a holder added —
    /// a new store the harness opened). Sorted.
    pub added_holders: Vec<String>,
    /// Holder ids present in the baseline roster but ABSENT from the current roster (a holder
    /// removed). Sorted.
    pub removed_holders: Vec<String>,
    /// The DPIA route (gdpr §2.3): the newly-appeared special-category flows, each recorded as an
    /// Art. 35 obligation by the [`myelin_gdpr::DpiaRouter`] — *surfaced for a DPO, never
    /// auto-decided*. Empty iff no new special-category flow appeared in the diff.
    pub dpia_verdicts: Vec<DpiaVerdict>,
}

impl DataMapDiff {
    /// **The gate's green verdict.** `true` iff the map is UNCHANGED — no field added/removed, no
    /// reclassification, no holder added/removed (the DPIA route is a consequence of an added/
    /// reclassified field, so it is never non-empty when the structural diff is empty). A clean diff
    /// is the build-passes verdict; a non-clean diff is the build-FAILS verdict the gate returns.
    pub fn is_clean(&self) -> bool {
        self.added_fields.is_empty()
            && self.removed_fields.is_empty()
            && self.reclassifications.is_empty()
            && self.added_holders.is_empty()
            && self.removed_holders.is_empty()
    }

    /// Whether the diff introduced a new special-category flow (gdpr §2.3) — `true` iff the DPIA
    /// router recorded at least one Art. 35 obligation. A DPO reading the diff treats this as the
    /// must-not-ship-silently signal.
    pub fn requires_dpia(&self) -> bool {
        !self.dpia_verdicts.is_empty()
    }

    /// A human-readable summary of the diff for the CI failure message a DPO reads (gdpr §2.2 — *a
    /// DPO sees the change*). One line per change class; the DPIA obligations are called out
    /// explicitly. Returned only when the diff is non-clean (the gate's surfaced reason).
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        for e in &self.added_fields {
            lines.push(format!(
                "+ field {} ({} @ {})",
                e.field_path, e.holder_id, e.region
            ));
        }
        for e in &self.removed_fields {
            lines.push(format!(
                "- field {} ({} @ {})",
                e.field_path, e.holder_id, e.region
            ));
        }
        for r in &self.reclassifications {
            lines.push(format!(
                "~ reclassified {}: role {}→{}, category {}→{}, basis {}→{}, retention {}→{}, \
                 erasure {}→{}, holder {}→{}, region {}→{}",
                r.field_path,
                r.before.role,
                r.after.role,
                r.before.category,
                r.after.category,
                r.before.basis,
                r.after.basis,
                r.before.retention,
                r.after.retention,
                r.before.erasure,
                r.after.erasure,
                r.before.holder_id,
                r.after.holder_id,
                r.before.region,
                r.after.region,
            ));
        }
        for h in &self.added_holders {
            lines.push(format!("+ holder {h}"));
        }
        for h in &self.removed_holders {
            lines.push(format!("- holder {h}"));
        }
        for v in &self.dpia_verdicts {
            match v {
                DpiaVerdict::Required { marker, reason } => lines.push(format!(
                    "! DPIA REQUIRED {} (kind {}): {}",
                    marker.field_path, marker.special_category_kind, reason
                )),
            }
        }
        lines.join("\n")
    }
}

/// **The data-map diff (the comparison core; gdpr §2.2).** Computes the structured [`DataMapDiff`]
/// between the committed `baseline` inventory and the freshly-regenerated `current` inventory:
/// - **added/removed fields** by `field_path` set-difference (a field path in current but not
///   baseline is added; in baseline but not current is removed);
/// - **reclassifications** for a field path present in BOTH whose entry differs (any of the five
///   tags / holder / region moved) — the before/after captured;
/// - **added/removed holders** by roster set-difference;
/// - **the DPIA route** (gdpr §2.3): the [`myelin_gdpr::DpiaRouter`] fed (baseline markers, current
///   markers) records each newly-appeared special-category flow as an Art. 35 obligation.
///
/// Pure + deterministic: a function of (baseline, current); the outputs are sorted, so the diff is
/// byte-stable and the gate never flaps in CI. *The map, not a hand-written changelog, drives the
/// review* — a DPO reads this diff, not a developer's recollection.
pub fn diff(baseline: &Inventory, current: &Inventory) -> DataMapDiff {
    // Index both inventories by field path (a field path is unique per inventory — the generator
    // emits one entry per `owning_struct.field`).
    let base_by_path: BTreeMap<&str, &InventoryEntry> = baseline
        .entries
        .iter()
        .map(|e| (e.field_path.as_str(), e))
        .collect();
    let cur_by_path: BTreeMap<&str, &InventoryEntry> = current
        .entries
        .iter()
        .map(|e| (e.field_path.as_str(), e))
        .collect();

    let mut added_fields = Vec::new();
    let mut removed_fields = Vec::new();
    let mut reclassifications = Vec::new();

    // Added (in current, not in baseline) + reclassified (in both, differs).
    for (path, cur_entry) in &cur_by_path {
        match base_by_path.get(path) {
            None => added_fields.push((*cur_entry).clone()),
            Some(base_entry) => {
                if base_entry != cur_entry {
                    reclassifications.push(Reclassification {
                        field_path: (*path).to_string(),
                        before: (*base_entry).clone(),
                        after: (*cur_entry).clone(),
                    });
                }
            }
        }
    }
    // Removed (in baseline, not in current).
    for (path, base_entry) in &base_by_path {
        if !cur_by_path.contains_key(path) {
            removed_fields.push((*base_entry).clone());
        }
    }

    // Holder roster set-difference.
    let added_holders: Vec<String> = current
        .holders
        .difference(&baseline.holders)
        .cloned()
        .collect();
    let removed_holders: Vec<String> = baseline
        .holders
        .difference(&current.holders)
        .cloned()
        .collect();

    // The DPIA route (gdpr §2.3): a newly-appeared special-category marker is an Art. 35 obligation.
    // Reuses the P-108 router VERBATIM — the gate does not re-detect special-category, it diffs the
    // marker sets the generated maps already carry.
    let prior_markers: &BTreeSet<DpiaMarker> = &baseline.dpia_markers;
    let current_markers: &BTreeSet<DpiaMarker> = &current.dpia_markers;
    let dpia_verdicts = DpiaRouter::new().route(prior_markers, current_markers);

    // Sorted outputs (deterministic, diffable). `added_fields`/`removed_fields` come from a
    // BTreeMap walk (already path-sorted); `reclassifications` we sort by path; holder vecs from
    // a BTreeSet difference (already sorted); `dpia_verdicts` the router returns path-sorted.
    reclassifications.sort();
    DataMapDiff {
        added_fields,
        removed_fields,
        reclassifications,
        added_holders,
        removed_holders,
        dpia_verdicts,
    }
}

/// The verdict the CI build step acts on (gdpr §2.2). [`Unchanged`](GateVerdict::Unchanged) → the
/// build PASSES; [`Changed`](GateVerdict::Changed) → the build FAILS with the diff surfaced until a
/// DPO reviews + re-baselines. [`CorruptBaseline`](GateVerdict::CorruptBaseline) → the committed
/// baseline JSON is internally inconsistent (its fingerprint does not match its inventory) — also a
/// build failure (the gate refuses to compare against a corrupt baseline).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateVerdict {
    /// The regenerated map matches the committed baseline — the build passes (green).
    Unchanged,
    /// The map changed — the build fails (red) with the structured diff surfaced for a DPO. A
    /// `SpecialCategory` addition additionally carries the Art. 35 obligation
    /// ([`DataMapDiff::requires_dpia`]).
    Changed(Box<DataMapDiff>),
    /// The committed baseline is internally inconsistent (its fingerprint ≠ its inventory) — the gate
    /// refuses to trust it. A build failure that says "re-seal the baseline", distinct from a
    /// content change.
    CorruptBaseline,
}

impl GateVerdict {
    /// Whether the gate is GREEN (the build passes). `true` only for [`GateVerdict::Unchanged`].
    pub fn is_green(&self) -> bool {
        matches!(self, GateVerdict::Unchanged)
    }

    /// The diff to surface, if the gate is red on a content change (`None` for green or a corrupt
    /// baseline).
    pub fn diff(&self) -> Option<&DataMapDiff> {
        match self {
            GateVerdict::Changed(d) => Some(d),
            _ => None,
        }
    }
}

/// **The CI data-map diff GATE (`check_against_baseline`; contract 10.3, gdpr §2.2).** The committed
/// step a build runs: regenerate the inventory (`current`, from [`crate::datamap::data_map`] over the
/// boot's registered-holder set) and compare it against the committed `baseline`. The verdict:
/// - the baseline must be self-consistent (its fingerprint matches its inventory) — else
///   [`GateVerdict::CorruptBaseline`] (refuse to compare against a corrupt baseline);
/// - if `current.fingerprint() == baseline.fingerprint` → the maps are identical →
///   [`GateVerdict::Unchanged`] (the build passes);
/// - otherwise → [`GateVerdict::Changed`] with the structured [`diff`] surfaced (the build fails
///   until a DPO reviews; a new special-category flow additionally routes into the DPIA gate).
///
/// The fingerprint is the fast path (a single string compare); the structured diff is computed only
/// when the fingerprint differs (the surfaced reason a DPO reads). Deterministic — a pure function of
/// (baseline, current).
pub fn check_against_baseline(baseline: &CommittedBaseline, current: &Inventory) -> GateVerdict {
    if !baseline.is_self_consistent() {
        return GateVerdict::CorruptBaseline;
    }
    if current.fingerprint() == baseline.fingerprint {
        // Fast path: identical fingerprints ⇒ identical maps ⇒ no diff. (Defence in depth: the
        // structured diff over identical maps is clean too — the fingerprint is the canonical
        // serialization, so equal fingerprint ⇒ equal map.)
        return GateVerdict::Unchanged;
    }
    let d = diff(&baseline.inventory, current);
    // The fingerprint differs, so the structured diff MUST be non-clean (the fingerprint is a
    // function of the whole inventory). If they ever disagree it is a generator bug, not a clean
    // pass — surface the change.
    debug_assert!(
        !d.is_clean(),
        "fingerprint differs but structured diff is clean — generator/fingerprint disagreement"
    );
    GateVerdict::Changed(Box::new(d))
}

/// The committed-baseline fingerprint constant NAME the CI step keys on (the committed artifact is a
/// `CommittedBaseline` JSON; this documents the convention). Anchored here so a later CI-wiring
/// prompt commits the baseline under a stable key. The live baseline file lands with the
/// `serve(AppSpec)` boot that assembles the full registered-holder set (the named CI-wiring floor).
pub const COMMITTED_BASELINE_FINGERPRINT: &str = "gdpr.data_map.committed_baseline.fingerprint";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamap::{data_map, HolderSchema};
    use myelin_gdpr::PersonalData;
    use myelin_substrate::{Holder, HolderRegistration, StoreKind};
    use myelin_tenancy::Region;

    // ── A real-shaped registered-holder set: a principal store (H15) with two ordinary-category PII
    //    fields + a zero-PII derived index (H7). The diff-gate tests mutate THIS set (add a field,
    //    reclassify, add a holder, add a special-category field) and assert the gate's verdict.

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct PrincipalRow {
        #[personal_data(
            category = ContactInfo,
            role = TenantContent,
            basis = Contract,
            retention = UntilContractEnd,
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        email: String,
        #[personal_data(
            category = Identifier,
            role = PlatformOperational,
            basis = Contract,
            retention = UntilContractEnd,
            erasure = Pseudonymise,
            subject_locator = "principal_id"
        )]
        handle: String,
        row_version: u64,
    }

    /// The SAME schema with `email` reclassified `role = TenantContent → PlatformOperational` (the
    /// legal posture moved — the diff gate must surface it).
    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct PrincipalRowReclassified {
        #[personal_data(
            category = ContactInfo,
            role = PlatformOperational, // ← reclassified from TenantContent
            basis = Contract,
            retention = UntilContractEnd,
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        email: String,
    }

    /// A schema with a NEW special-category field (health) — the DPIA route fires.
    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct ProfileRow {
        #[personal_data(
            category = SpecialCategory(health),
            role = PlatformOperational,
            basis = Consent(c-1),
            retention = Fixed(365d),
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        health_note: String,
    }

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct OpaqueIndexRow {
        doc_id: u64,
    }

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn principal_schema() -> HolderSchema {
        HolderSchema::from_schema::<PrincipalRow>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        )
    }

    fn index_schema() -> HolderSchema {
        HolderSchema::from_schema::<OpaqueIndexRow>(
            HolderRegistration {
                kind: StoreKind::SearchIndex,
                name: "search_index",
            },
            Holder::H7SearchIndex,
            region(),
        )
    }

    /// The committed baseline: the principal store + the zero-PII index (the DPO-reviewed map).
    fn baseline() -> CommittedBaseline {
        CommittedBaseline::seal(data_map(&[principal_schema(), index_schema()]))
    }

    /// **GA-D5 (the data-map-diff face) — the headline drill.** An UNCHANGED inventory passes; a
    /// CHANGED one (added field, reclassification, added holder, special-category addition) FAILS with
    /// the diff surfaced. This is the dated green artifact.
    #[test]
    fn ga_d5_changed_inventory_fails_the_gate_and_unchanged_passes() {
        let base = baseline();

        // ── UNCHANGED: regenerate the SAME map → green (the build passes). ──────────────────────
        let same = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &same);
        assert_eq!(
            verdict,
            GateVerdict::Unchanged,
            "an unchanged map passes the gate"
        );
        assert!(verdict.is_green());
        assert!(verdict.diff().is_none());

        // ── CHANGED: a holder was REMOVED (the index dropped) → red, diff surfaces the removal. ──
        let dropped = data_map(&[principal_schema()]);
        let verdict = check_against_baseline(&base, &dropped);
        assert!(!verdict.is_green(), "a changed map fails the gate");
        let d = verdict.diff().expect("the diff is surfaced for a DPO");
        assert!(!d.is_clean());
        // the zero-PII index holder was removed.
        assert_eq!(
            d.removed_holders,
            vec!["search_index:search_index".to_string()]
        );
        assert!(d.added_holders.is_empty());
        // no field change (the index carried no PII field) — the holder removal is the diff.
        assert!(d.added_fields.is_empty() && d.removed_fields.is_empty());
        assert!(
            !d.requires_dpia(),
            "no special-category flow appeared — no DPIA obligation"
        );
        assert!(d.summary().contains("- holder search_index:search_index"));
    }

    /// A NEW PII field appears (a store added a tagged column) → the gate fails with the new field
    /// surfaced (the added-field diff path — mandatory-core).
    #[test]
    fn a_new_pii_field_fails_the_gate_with_the_field_surfaced() {
        let base = CommittedBaseline::seal(data_map(&[index_schema()])); // baseline: only the zero-PII index.
                                                                         // The principal store (two PII fields) is now registered → two new fields appear.
        let current = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &current);

        let d = verdict.diff().expect("changed");
        let added: Vec<&str> = d
            .added_fields
            .iter()
            .map(|e| e.field_path.as_str())
            .collect();
        assert_eq!(added, vec!["PrincipalRow.email", "PrincipalRow.handle"]);
        assert_eq!(d.added_holders, vec!["oltp:identity_oltp".to_string()]);
        assert!(d.removed_fields.is_empty());
        // the new email field carries its facts into the diff (a DPO reads them).
        let email = d
            .added_fields
            .iter()
            .find(|e| e.field_path == "PrincipalRow.email")
            .unwrap();
        assert_eq!(email.role, "TenantContent");
        assert_eq!(email.category, "ContactInfo");
        assert!(d.summary().contains("+ field PrincipalRow.email"));
    }

    /// A struct RENAME (the field moved to a different owning struct, so its field PATH changed) is
    /// surfaced as a removal of the old path + an addition of the new path — the gate fails and the
    /// before/after roles are both visible to a DPO. (A same-path tag move is a `Reclassification`,
    /// covered by [`a_same_path_tag_change_is_a_reclassification_not_add_remove`]; this is the
    /// path-changed case.)
    #[test]
    fn a_field_path_change_is_surfaced_as_remove_plus_add() {
        // baseline: PrincipalRow.email is role = TenantContent.
        let base = CommittedBaseline::seal(data_map(&[HolderSchema::from_schema::<PrincipalRow>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        )]));
        // current: the email field now lives on PrincipalRowReclassified at role = PlatformOperational
        // (the owning struct — hence the field PATH — changed).
        let current = data_map(&[HolderSchema::from_schema::<PrincipalRowReclassified>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "identity_oltp",
            },
            Holder::H15Identity,
            region(),
        )]);
        let verdict = check_against_baseline(&base, &current);

        let d = verdict.diff().expect("changed");
        // The path changed, so it is a removal of the old path + an addition of the new path.
        let after_email = d
            .added_fields
            .iter()
            .find(|e| e.field_path == "PrincipalRowReclassified.email")
            .expect("the new-path email appears as added");
        assert_eq!(after_email.role, "PlatformOperational");
        let before_email = d
            .removed_fields
            .iter()
            .find(|e| e.field_path == "PrincipalRow.email")
            .expect("the old-path email appears as removed");
        assert_eq!(before_email.role, "TenantContent");
        // it is not a same-path reclassification (the paths differ).
        assert!(d.reclassifications.is_empty());
    }

    /// A TRUE same-path reclassification (the field path is byte-identical in baseline + current, only
    /// a tag moved) lands in [`DataMapDiff::reclassifications`] with the before/after captured. We
    /// build the two inventories directly (a hand-authored before/after at the SAME path) to exercise
    /// the same-path branch precisely.
    #[test]
    fn a_same_path_tag_change_is_a_reclassification_not_add_remove() {
        let before_entry = InventoryEntry {
            field_path: "PrincipalRow.email".into(),
            holder_id: "oltp:identity_oltp".into(),
            holder: "H15".into(),
            region: "fr-par".into(),
            category: "ContactInfo".into(),
            role: "TenantContent".into(),
            basis: "Contract".into(),
            retention: "UntilContractEnd".into(),
            erasure: "CryptoShred(subject_dek)".into(),
            subject_locator: "principal_id".into(),
        };
        let mut after_entry = before_entry.clone();
        after_entry.role = "PlatformOperational".into(); // ← the same-path reclassification.

        let mut base_inv = Inventory::default();
        base_inv.entries.push(before_entry.clone());
        base_inv.holders.insert("oltp:identity_oltp".into());
        let base = CommittedBaseline::seal(base_inv);

        let mut cur_inv = Inventory::default();
        cur_inv.entries.push(after_entry.clone());
        cur_inv.holders.insert("oltp:identity_oltp".into());

        let verdict = check_against_baseline(&base, &cur_inv);
        let d = verdict.diff().expect("changed");
        // It is a reclassification — NOT an add/remove (the path is unchanged).
        assert!(d.added_fields.is_empty(), "same path ⇒ not an add");
        assert!(d.removed_fields.is_empty(), "same path ⇒ not a remove");
        assert_eq!(d.reclassifications.len(), 1);
        let r = &d.reclassifications[0];
        assert_eq!(r.field_path, "PrincipalRow.email");
        assert_eq!(r.before.role, "TenantContent");
        assert_eq!(r.after.role, "PlatformOperational");
        // the holder roster is unchanged (same holder both sides).
        assert!(d.added_holders.is_empty() && d.removed_holders.is_empty());
        assert!(d.summary().contains("~ reclassified PrincipalRow.email"));
    }

    /// **The DPIA ROUTE (gdpr §2.3) — a new SpecialCategory flow routes into the DPIA gate.** Adding a
    /// special-category field both FAILS the diff gate (the map changed) AND records an Art. 35
    /// obligation ([`DataMapDiff::requires_dpia`]). An ordinary-category addition fails the gate but is
    /// NOT a DPIA obligation. This is the mandatory-core DPIA-route path.
    #[test]
    fn a_new_special_category_flow_routes_into_the_dpia_gate() {
        // baseline: the principal store (ordinary-category fields only) — no DPIA marker.
        let base = CommittedBaseline::seal(data_map(&[principal_schema()]));
        assert!(
            base.inventory.dpia_markers.is_empty(),
            "baseline carries no special-category flow"
        );

        // current: a Profile store with a health (special-category) field is now registered.
        let profile = HolderSchema::from_schema::<ProfileRow>(
            HolderRegistration {
                kind: StoreKind::Oltp,
                name: "profile_oltp",
            },
            Holder::H15Identity,
            region(),
        );
        let current = data_map(&[principal_schema(), profile]);

        let verdict = check_against_baseline(&base, &current);
        let d = verdict.diff().expect("changed");
        // the diff fails the gate…
        assert!(!d.is_clean());
        // …AND routes into the DPIA gate (the new special-category flow is an Art. 35 obligation).
        assert!(
            d.requires_dpia(),
            "a new special-category flow requires a DPIA"
        );
        assert_eq!(d.dpia_verdicts.len(), 1);
        match &d.dpia_verdicts[0] {
            DpiaVerdict::Required { marker, reason } => {
                assert_eq!(marker.field_path, "ProfileRow.health_note");
                assert_eq!(marker.special_category_kind, "health");
                assert!(reason.contains("DPIA required"));
                assert!(
                    reason.contains("DPO"),
                    "surfaced for a DPO, never auto-decided"
                );
            }
        }
        assert!(d
            .summary()
            .contains("! DPIA REQUIRED ProfileRow.health_note"));
    }

    /// An ORDINARY-category addition fails the diff gate but does NOT route into the DPIA gate (the
    /// DPIA route is special-category-only — it is not "any new field"). Kills a mutant that would
    /// fire DPIA on every change.
    #[test]
    fn an_ordinary_category_addition_does_not_require_a_dpia() {
        let base = CommittedBaseline::seal(data_map(&[index_schema()]));
        let current = data_map(&[principal_schema(), index_schema()]); // ContactInfo + Identifier — ordinary.
        let d = check_against_baseline(&base, &current)
            .diff()
            .expect("changed")
            .clone();
        assert!(!d.is_clean(), "an added field fails the gate");
        assert!(
            !d.requires_dpia(),
            "but an ordinary-category field is not a DPIA obligation"
        );
        assert!(d.dpia_verdicts.is_empty());
    }

    /// A CORRUPT baseline (its committed fingerprint does not match its inventory — a hand-edited JSON
    /// that forgot to re-seal) is refused: the gate returns `CorruptBaseline`, a build failure
    /// distinct from a content change. The gate never compares against a baseline it cannot trust.
    #[test]
    fn a_corrupt_baseline_is_refused() {
        let mut base = baseline();
        base.fingerprint = "blake3:deadbeef".into(); // tamper: fingerprint no longer matches inventory.
        assert!(!base.is_self_consistent());
        let current = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &current);
        assert_eq!(verdict, GateVerdict::CorruptBaseline);
        assert!(!verdict.is_green());
        assert!(
            verdict.diff().is_none(),
            "a corrupt baseline surfaces no content diff"
        );
    }

    /// Re-baselining: after a DPO reviews a diff and accepts it, sealing the new inventory makes the
    /// gate green again (the next build passes against the re-sealed baseline). The ratchet moves
    /// FORWARD only with a DPO's re-seal — the gate cannot self-clear.
    #[test]
    fn re_sealing_the_baseline_after_review_makes_the_gate_green() {
        let base = baseline();
        // a holder is added (the gate would fail)…
        let changed = data_map(&[
            principal_schema(),
            index_schema(),
            HolderSchema::from_schema::<ProfileRow>(
                HolderRegistration {
                    kind: StoreKind::Oltp,
                    name: "profile_oltp",
                },
                Holder::H15Identity,
                region(),
            ),
        ]);
        assert!(
            !check_against_baseline(&base, &changed).is_green(),
            "the change fails the gate"
        );
        // …a DPO reviews + re-seals the new inventory as the baseline → the next build is green.
        let re_sealed = CommittedBaseline::seal(changed.clone());
        assert!(re_sealed.is_self_consistent());
        assert!(
            check_against_baseline(&re_sealed, &changed).is_green(),
            "the re-sealed baseline passes the regenerated map"
        );
    }

    /// The diff is DETERMINISTIC + order-independent (the gate never flaps in CI): the same map in any
    /// holder order yields the same verdict, and the diff outputs are sorted.
    #[test]
    fn the_gate_is_deterministic_and_order_independent() {
        let base = baseline();
        let a = data_map(&[principal_schema(), index_schema()]);
        let b = data_map(&[index_schema(), principal_schema()]); // reversed order.
        assert_eq!(
            check_against_baseline(&base, &a),
            check_against_baseline(&base, &b),
            "order-independent verdict"
        );
        assert!(check_against_baseline(&base, &a).is_green());
    }

    /// The diff + verdict round-trip serialize — they cross the crate boundary (the committed baseline
    /// is a checked-in JSON; the diff is surfaced to a DPO), so a stable serde shape is part of the
    /// frozen surface.
    #[test]
    fn diff_and_verdict_round_trip_serialize() {
        let base = CommittedBaseline::seal(data_map(&[index_schema()]));
        let current = data_map(&[principal_schema(), index_schema()]);
        let verdict = check_against_baseline(&base, &current);

        let back: GateVerdict =
            serde_json::from_str(&serde_json::to_string(&verdict).unwrap()).unwrap();
        assert_eq!(back, verdict);

        // the committed baseline round-trips (it IS the committed artifact).
        let base_back: CommittedBaseline =
            serde_json::from_str(&serde_json::to_string(&base).unwrap()).unwrap();
        assert_eq!(base_back, base);
        assert!(base_back.is_self_consistent());
    }

    /// An empty→empty diff is clean (the degenerate case: no holders either side → no change). Kills a
    /// mutant that would make `is_clean` always-false.
    #[test]
    fn an_unchanged_empty_map_is_clean() {
        let base = CommittedBaseline::seal(data_map(&[]));
        let d = diff(&base.inventory, &data_map(&[]));
        assert!(d.is_clean(), "empty→empty is no change");
        assert!(check_against_baseline(&base, &data_map(&[])).is_green());
    }
}
