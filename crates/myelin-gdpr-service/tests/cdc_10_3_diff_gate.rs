//! # CDC 10.3 (diff-gate leg) — the CI data-map DIFF GATE + the DPIA-route (P-GA-10 → P-110)
//!
//! **Contract:** index row 10.3 (the generated inventory is committed; *a build that changes it
//! fails CI with the diff surfaced until a DPO reviews*; a new `SpecialCategory` flow routes into
//! the DPIA gate). This is the consumer-driven contract test the coverage scanner (P-S21) reads both
//! halves of:
//!
//! - **provider** = the data-map GENERATOR ([`data_map`] → [`Inventory`], P-GA-09) — it emits the
//!   machine-readable inventory the gate diffs.
//! - **consumer** = the **CI diff gate** ([`check_against_baseline`], P-GA-10) — it commits a
//!   DPO-reviewed [`CommittedBaseline`], regenerates the inventory each build, and FAILS the build
//!   ([`GateVerdict::Changed`]) with the structured [`DataMapDiff`] surfaced when the map changes; a
//!   newly-appeared special-category flow additionally routes into the DPIA gate (the marker from
//!   P-GA-08). An UNCHANGED map passes ([`GateVerdict::Unchanged`]).
//!
//! The dated green artifact: the generator emits an inventory over a real-shaped registered-holder
//! set; the gate is GREEN against the sealed baseline and RED with the diff surfaced when a holder /
//! field / classification changes; a new special-category flow routes into the DPIA gate. If 10.3's
//! diff-gate shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{DpiaVerdict, PersonalData};
use myelin_gdpr_service::{
    check_against_baseline, data_map, CommittedBaseline, GateVerdict, HolderSchema,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

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
    row_version: u64,
}

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

fn region() -> Region {
    Region("fr-par".into())
}

fn principal() -> HolderSchema {
    HolderSchema::from_schema::<PrincipalRow>(
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "identity_oltp",
        },
        Holder::H15Identity,
        region(),
    )
}

fn profile() -> HolderSchema {
    HolderSchema::from_schema::<ProfileRow>(
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "profile_oltp",
        },
        Holder::H15Identity,
        region(),
    )
}

/// PROVIDER (the generator) + CONSUMER (the diff gate): the gate is GREEN against the sealed baseline
/// for an unchanged map, and RED with the diff surfaced when the map changes (a new PII field + a new
/// holder). The committed baseline is the DPO-reviewed artifact; a build regenerates + compares.
#[test]
fn cdc_10_3_diff_gate_passes_unchanged_and_fails_a_changed_map_with_the_diff() {
    // The DPO-reviewed baseline: just the principal store.
    let baseline = CommittedBaseline::seal(data_map(&[principal()]));
    assert!(baseline.is_self_consistent());

    // ── unchanged: regenerate the SAME map → green (build passes). ───────────────────────────────
    assert_eq!(
        check_against_baseline(&baseline, &data_map(&[principal()])),
        GateVerdict::Unchanged,
    );

    // ── changed: the Profile store (a new special-category holder) is now registered → red. ───────
    let current = data_map(&[principal(), profile()]);
    let verdict = check_against_baseline(&baseline, &current);
    assert!(!verdict.is_green(), "a changed map fails the gate");
    let d = verdict.diff().expect("the diff is surfaced for a DPO");

    // the new health field + the new holder are surfaced.
    assert_eq!(
        d.added_fields
            .iter()
            .map(|e| e.field_path.as_str())
            .collect::<Vec<_>>(),
        vec!["ProfileRow.health_note"],
    );
    assert_eq!(d.added_holders, vec!["oltp:profile_oltp".to_string()]);

    // …and the new special-category flow routes into the DPIA gate (Art. 35 obligation).
    assert!(d.requires_dpia());
    assert!(matches!(
        &d.dpia_verdicts[0],
        DpiaVerdict::Required { marker, .. } if marker.field_path == "ProfileRow.health_note"
    ));
    assert!(d
        .summary()
        .contains("! DPIA REQUIRED ProfileRow.health_note"));
}

/// Re-baselining (the ratchet moves forward only with a DPO re-seal): after the gate fails, sealing
/// the reviewed inventory makes the next build green. The gate cannot self-clear.
#[test]
fn cdc_10_3_re_sealing_after_dpo_review_makes_the_gate_green() {
    let baseline = CommittedBaseline::seal(data_map(&[principal()]));
    let changed = data_map(&[principal(), profile()]);
    assert!(!check_against_baseline(&baseline, &changed).is_green());

    // a DPO reviews + re-seals the new inventory → the next build passes.
    let re_sealed = CommittedBaseline::seal(changed.clone());
    assert_eq!(
        check_against_baseline(&re_sealed, &changed),
        GateVerdict::Unchanged
    );
}
