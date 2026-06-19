//! # `dpia` — the `SpecialCategory` → DPIA router (the data-map-diff marker into the DPIA gate)
//! (contract 10.2; gdpr §2.3 / §2.4) — P-GA-08 / P-108
//!
//! The DPIA gate (Art. 35) fires on a **data-map diff** that introduces a new `SpecialCategory`
//! flow (gdpr §2.3). P-107 made the *detection* structural: a `category = SpecialCategory(<kind>)`
//! tag is parsed off a [`crate::PersonalDataField`] into a [`crate::SpecialCategoryFlag`]. THIS
//! module is the *routing* layer on top: it emits a [`DpiaMarker`] into the generated inventory for
//! every special-category field, and the [`DpiaRouter`] records a newly-appeared marker as a
//! **DPIA-required change** the data-map diff gate (P-GA-10) surfaces to a human/DPO.
//!
//! ## What is genuinely new here (the reconciliation, EI-01 §7)
//! P-107 (in [`crate::__registry`]) already ships [`crate::SpecialCategoryFlag`] +
//! [`crate::PersonalDataField::is_special_category`] — the *flag*. This module REUSES that detection
//! verbatim (it does not re-detect special-category off raw text) and adds the two things P-GA-08
//! owns that did not exist:
//! 1. [`DpiaMarker`] — the **marker shape that lives IN the generated inventory** (field path +
//!    special-category kind). The marker is what the data-map diff compares build-to-build; a marker
//!    that appears in the new map but not the old is "a new `SpecialCategory` flow".
//! 2. [`DpiaRouter`] — the **router** that adjudicates the *posture* (not the decision): it diffs the
//!    prior marker set against the current one and records each newly-appeared marker as
//!    [`DpiaVerdict::Required`] (a DPIA is required, **surfaced for a human/DPO call, never
//!    auto-decided** — gdpr §2.3). An unchanged marker set yields no new obligation.
//!
//! ## The adjudication is surfaced, not auto-decided (gdpr §2.3)
//! The router NEVER returns "DPIA passed" / "DPIA not needed for this flow". It returns "a DPIA is
//! **required** for this newly-introduced special-category flow" — the mechanical, deterministic
//! half. Whether the DPIA itself clears is a DPO judgement made off-platform; the router's job is to
//! make the obligation **impossible to miss** (the data-map diff cannot ship a new special-category
//! flow silently). That is the §2.3 gate.

use crate::{HasPersonalData, PersonalDataField};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A **DPIA marker** emitted into the generated data-map inventory for one `SpecialCategory` field
/// (gdpr §2.3; contract 10.2). The marker is the diff substrate: the data-map diff gate (P-GA-10)
/// compares the marker SET of the prior committed inventory against the current build's; a marker
/// present now but absent before is "a new `SpecialCategory` flow" the DPIA gate fires on.
///
/// References-not-payloads: the marker carries the field PATH (`owning_struct.field`) + the
/// special-category KIND reference (`"health"`), never any subject's actual special-category value.
/// It is safe to commit into the data map + surface to a DPO.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DpiaMarker {
    /// The field path the special-category flow lives at (`owning_struct ∥ "." ∥ field`), e.g.
    /// `"ProfileRow.health_note"`. The stable identity the diff keys on.
    pub field_path: String,
    /// The Art. 9 special-category kind reference the `SpecialCategory(<kind>)` tag named, e.g.
    /// `"health"`. A change of KIND at the same path is also a new flow (a re-classification).
    pub special_category_kind: String,
}

impl DpiaMarker {
    /// Emit a DPIA marker for a registry field **iff** it is `category = SpecialCategory(...)`
    /// (gdpr §2.3). Returns `None` for an ordinary-category field (no DPIA obligation). This is the
    /// ONE place a marker is minted — it reuses P-107's [`PersonalDataField::is_special_category`]
    /// detection verbatim, so the marker can never disagree with the flag.
    pub fn from_field(field: &PersonalDataField) -> Option<DpiaMarker> {
        field.is_special_category().map(|flag| DpiaMarker {
            field_path: format!("{}.{}", field.owning_struct, field.field),
            special_category_kind: flag.kind.to_string(),
        })
    }
}

/// Walk a holder/schema type's generated registry and emit the DPIA marker set — the
/// special-category slice of its data-map contribution (gdpr §2.3; the substrate the diff gate
/// P-GA-10 commits + compares). **100% of `SpecialCategory`-tagged fields emit a marker** (the
/// quantified gate): the iteration is total over `personal_data_fields()`, and a field emits a
/// marker iff `is_special_category()` is `Some` — so the marker count equals the special-category
/// field count exactly.
///
/// Returned as a sorted set (deduplicated, stable order) so the data-map diff is deterministic
/// build-to-build (no spurious diff from field-order churn).
pub fn dpia_markers<T: HasPersonalData>() -> BTreeSet<DpiaMarker> {
    T::personal_data_fields()
        .iter()
        .filter_map(DpiaMarker::from_field)
        .collect()
}

/// The same walk over an already-materialised registry slice (the shape the data-map generator
/// P-GA-09 holds after it unions every registered holder's `personal_data_fields()`). The diff gate
/// (P-GA-10) calls THIS over the generated inventory's fields — `dpia_markers::<T>()` is the
/// single-holder convenience over the same logic.
pub fn dpia_markers_of(fields: &[PersonalDataField]) -> BTreeSet<DpiaMarker> {
    fields.iter().filter_map(DpiaMarker::from_field).collect()
}

/// The router's verdict for a marker the data-map diff surfaced (gdpr §2.3). The router emits ONLY
/// `Required` — it adjudicates the *obligation* (deterministic), never the DPIA *outcome* (a DPO
/// call). The variant exists as an enum (not a bare bool) so the surfaced reason is explicit in the
/// diff-gate output a DPO reads, and so a later band can add posture variants (a new agent
/// capability over personal data, large-scale monitoring — gdpr §2.3) without a shape change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DpiaVerdict {
    /// A DPIA is **required** for this newly-introduced special-category flow (Art. 35; gdpr §2.3).
    /// Surfaced for a human/DPO call — the platform records the obligation, it does not clear it.
    Required {
        /// the marker the obligation attaches to (the field path + special-category kind).
        marker: DpiaMarker,
        /// the human-readable reason surfaced into the data-map diff a DPO reads.
        reason: String,
    },
}

impl DpiaVerdict {
    /// The field path this verdict attaches to (the diff-gate row a DPO acts on).
    pub fn field_path(&self) -> &str {
        match self {
            DpiaVerdict::Required { marker, .. } => &marker.field_path,
        }
    }
}

/// The **DPIA router** (gdpr §2.3, contract 10.2). It is fed the prior committed data-map's DPIA
/// marker set and the current build's, and it records each **newly-appeared** marker as a
/// DPIA-required change. This is the routing half the data-map diff gate (P-GA-10) drives: the diff
/// surfaces *that* the map changed; the router classifies a special-category addition as a DPIA
/// obligation a DPO must adjudicate.
///
/// **Stateless + deterministic:** `route` is a pure function of (prior, current) — the SAME diff
/// always routes to the SAME verdict set, so the gate is reproducible in CI. The "state" (the prior
/// committed marker set) lives in the committed data map (P-GA-09/P-GA-10), not here.
#[derive(Clone, Copy, Debug, Default)]
pub struct DpiaRouter;

impl DpiaRouter {
    /// A new router.
    pub fn new() -> DpiaRouter {
        DpiaRouter
    }

    /// Route a data-map diff: every marker in `current` that is **not** in `prior` is a newly-
    /// introduced special-category flow and yields a [`DpiaVerdict::Required`] (gdpr §2.3). A marker
    /// present in both (unchanged flow) yields nothing — a DPIA was already adjudicated when it first
    /// appeared. A marker removed (`prior` minus `current`) yields nothing here (removal is not a new
    /// processing risk; it is surfaced by the diff itself, P-GA-10).
    ///
    /// Returned sorted by field path (deterministic gate output).
    pub fn route(
        &self,
        prior: &BTreeSet<DpiaMarker>,
        current: &BTreeSet<DpiaMarker>,
    ) -> Vec<DpiaVerdict> {
        current
            .difference(prior)
            .map(|marker| DpiaVerdict::Required {
                marker: marker.clone(),
                reason: format!(
                    "Art. 35 DPIA required: new special-category flow `{}` (kind: {}) — \
                     awaiting DPO adjudication (surfaced, not auto-decided)",
                    marker.field_path, marker.special_category_kind
                ),
            })
            .collect()
    }

    /// The **initial-introduction** route: routing the current marker set against an EMPTY prior
    /// (a fresh data map, or a holder newly added to the map). Every special-category flow is new, so
    /// every marker yields a `Required` verdict. This is the form the FIRST commit of a holder's map
    /// takes (P-GA-10 commits the baseline; this records the obligations the baseline introduces).
    pub fn route_all_new(&self, current: &BTreeSet<DpiaMarker>) -> Vec<DpiaVerdict> {
        self.route(&BTreeSet::new(), current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PersonalData;

    // A schema with a SpecialCategory field (the DPIA route), a CryptoShred-but-ordinary field (NOT a
    // DPIA route — proves the marker is special-category-only, not "any sensitive field"), and a
    // non-PII field (no entry at all).
    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct Subject {
        #[personal_data(
            category = SpecialCategory(health),
            role = PlatformOperational,
            basis = Consent(c-1),
            retention = Fixed(365d),
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        health_note: String,
        #[personal_data(
            category = ContactInfo,
            role = TenantContent,
            basis = Contract,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        email: String,
        row_version: u64,
    }

    /// GATE: a `SpecialCategory`-tagged field emits the DPIA marker into the inventory; a
    /// non-SpecialCategory tag (even a crypto-shred ContactInfo) does NOT. 100% of special-category
    /// fields emit the marker (the quantified gate), and ONLY they do.
    #[test]
    fn special_category_field_emits_the_dpia_marker_and_ordinary_fields_do_not() {
        let markers = dpia_markers::<Subject>();
        // Exactly one marker — the health_note field. The crypto-shred `email` is ordinary-category
        // (ContactInfo); it carries no DPIA obligation.
        assert_eq!(markers.len(), 1, "100% of (and ONLY) special-category fields emit a marker");
        let marker = markers.iter().next().unwrap();
        assert_eq!(marker.field_path, "Subject.health_note");
        assert_eq!(marker.special_category_kind, "health");

        // The count of markers equals the count of special-category fields, field-for-field (the
        // 100% property, proven structurally not by spot-check).
        let special_field_count = Subject::personal_data_fields()
            .iter()
            .filter(|f| f.is_special_category().is_some())
            .count();
        assert_eq!(markers.len(), special_field_count);
    }

    /// The marker minting reuses P-107's detection verbatim — `DpiaMarker::from_field` agrees with
    /// `is_special_category()` on every field (no second, drifting detector).
    #[test]
    fn marker_minting_agrees_with_the_p107_special_category_flag() {
        for f in Subject::personal_data_fields() {
            assert_eq!(
                DpiaMarker::from_field(f).is_some(),
                f.is_special_category().is_some(),
                "the marker fires iff (and only iff) the P-107 flag does, for {}",
                f.field
            );
        }
    }

    /// GATE / DRILL (the headline P-GA-08 drill — "the DPIA marker fires on a SpecialCategory
    /// addition"): the CI data-map diff routes a NEW special-category flow into the DPIA gate. The
    /// router records the newly-appeared marker as DPIA-required; an unchanged flow does not re-fire.
    #[test]
    fn router_fires_dpia_required_on_a_new_special_category_flow_only() {
        let router = DpiaRouter::new();
        let prior: BTreeSet<DpiaMarker> = BTreeSet::new();
        let current = dpia_markers::<Subject>();

        // First appearance: the new flow routes to DPIA-required (the §2.3 gate fires).
        let verdicts = router.route(&prior, &current);
        assert_eq!(verdicts.len(), 1, "a new special-category flow fires the DPIA gate");
        match &verdicts[0] {
            DpiaVerdict::Required { marker, reason } => {
                assert_eq!(marker.field_path, "Subject.health_note");
                assert!(reason.contains("DPIA required"));
                assert!(
                    reason.contains("DPO"),
                    "the adjudication is surfaced for a DPO, not auto-decided"
                );
            }
        }
        assert_eq!(verdicts[0].field_path(), "Subject.health_note");

        // No change build-to-build: the SAME marker set in prior and current routes to NOTHING (the
        // gate does not re-fire on an already-adjudicated flow — only a diff fires it).
        let no_change = router.route(&current, &current);
        assert!(no_change.is_empty(), "an unchanged flow does not re-fire the DPIA gate");
    }

    /// A reclassification (the kind changes at the same field path) is a NEW flow — the marker
    /// differs, so the router fires again. This is the data-map-diff semantics: the gate keys on the
    /// (path, kind) pair, not the path alone, so promoting a field to a different special-category
    /// kind cannot slip through.
    #[test]
    fn a_reclassification_to_a_new_special_category_kind_re_fires_the_gate() {
        let router = DpiaRouter::new();
        let prior: BTreeSet<DpiaMarker> = [DpiaMarker {
            field_path: "Subject.health_note".into(),
            special_category_kind: "health".into(),
        }]
        .into_iter()
        .collect();
        let current: BTreeSet<DpiaMarker> = [DpiaMarker {
            field_path: "Subject.health_note".into(),
            special_category_kind: "biometric".into(),
        }]
        .into_iter()
        .collect();
        let verdicts = router.route(&prior, &current);
        assert_eq!(verdicts.len(), 1, "a kind reclassification is a new flow");
        assert_eq!(verdicts[0].field_path(), "Subject.health_note");
    }

    /// `route_all_new` (the baseline-commit form) records every special-category flow as a new
    /// obligation — routing the current set against an empty prior.
    #[test]
    fn route_all_new_records_every_flow_as_a_fresh_obligation() {
        let router = DpiaRouter::new();
        let current = dpia_markers::<Subject>();
        let all = router.route_all_new(&current);
        assert_eq!(all.len(), current.len());
    }

    /// The marker + verdict round-trip serialize — they cross the crate boundary (the data map
    /// commits the marker set; the diff gate P-GA-10 surfaces the verdict to a DPO), so a stable
    /// serde shape is part of the frozen surface.
    #[test]
    fn marker_and_verdict_round_trip_serialize() {
        let marker = DpiaMarker {
            field_path: "S.f".into(),
            special_category_kind: "health".into(),
        };
        let back: DpiaMarker =
            serde_json::from_str(&serde_json::to_string(&marker).unwrap()).unwrap();
        assert_eq!(back, marker);

        let verdict = DpiaVerdict::Required {
            marker,
            reason: "r".into(),
        };
        let v_back: DpiaVerdict =
            serde_json::from_str(&serde_json::to_string(&verdict).unwrap()).unwrap();
        assert_eq!(v_back, verdict);
    }

    /// `dpia_markers_of` (the materialised-slice form P-GA-09's generator calls over the unioned
    /// inventory) agrees with `dpia_markers::<T>()` (the single-holder convenience).
    #[test]
    fn dpia_markers_of_a_slice_matches_the_typed_walk() {
        let from_slice = dpia_markers_of(Subject::personal_data_fields());
        let from_type = dpia_markers::<Subject>();
        assert_eq!(from_slice, from_type);
    }
}
