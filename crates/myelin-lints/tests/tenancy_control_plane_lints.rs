//! The `control-plane-pii-free` data-map slice — the TENANCY ownership prompt (P-CP-04 → global P-028).
//!
//! `control-plane-pii-free` is one of the TWO lints Tenancy owns (contract-index 1.6). P-S11 → P-018
//! first shipped it as the substrate-side scanner keyed to the `@control-plane` marker + a PII
//! NAME-fingerprint (`name`/`email`/`body`/…). P-CP-04 is the TENANCY slice — it SHARPENS that
//! scanner with the genuinely-NEW property the canonical §4.3 rule names: *no control-plane registry
//! column is classified `is_personal=true` — run through the generated data-map*. The authoritative
//! `is_personal` signal is the GDPR `#[personal_data(...)]` classify-derive (contract 10.2), NOT a
//! name guess: a control-plane field tagged `#[personal_data]` fires the lint regardless of its name.
//! This is the **CP-D1 lint leg**.
//!
//! **Coherence note (EI-01 §7 — reconcile, never duplicate).** This prompt adds NO new lint id and
//! re-defines NO type. It EXTENDS the in-place [`myelin_lints::lints::control_plane_pii_free`]
//! scanner with the data-map leg (a `#[personal_data]`-tagged field on a control-plane struct), and
//! adds the TENANCY-shaped red+green fixtures over the REAL frozen `CrossCellPointer` frame (P-CP-02
//! / P-027) + this verdict test — exactly mirroring the `tenancy_lints.rs` / `storage_lints.rs` /
//! `identity_lints.rs` precedent. The lint is SHARPENED, never weakened (EI-01 §5): a control-plane
//! field classified `is_personal=true` fails the build, even when its NAME would slip the
//! name-fingerprint leg.
//!
//! These tests ARE the P-CP-04 fixtures (the TESTS field: "the two fixtures (1 red + 1 green) ARE
//! the tests"). They run loud over the Tenancy fixtures and assert the exact verdict; the CI-wiring
//! proof (the Tenancy red fixture ⇒ the `lint-gate` binary exits non-zero, no `|| true` swallow) is
//! the last test. No threshold is weakened.
//!
//! **Floor named.** The live RUNTIME CP-D1 drill (the `cell`/`tenant_placement`/`cell_provisioning`
//! registry schema asserted at 0 `is_personal=true` columns via the generated data-map over the LIVE
//! tables) lands once the registry exists in **P-CP-05 / P-080**. This is the lint leg (the
//! compile-time / source-scan rejection) only.

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, control_plane_pii_free, no_untagged_personal_data};
use myelin_lints::LintId;

const RED: &str = "control_plane_pii_free.tenancy.red.rs.txt";
const GREEN: &str = "control_plane_pii_free.tenancy.green.rs.txt";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

#[test]
fn control_plane_pii_free_rejects_the_tenancy_frame_red_fixture() {
    // REJECT: the frozen CrossCellPointer frame with an `is_personal=true`-tagged fifth field
    // produces >= 1 violation, fired by THE control-plane-pii-free lint and no other.
    let violations = control_plane_pii_free().run(&read_fixture(RED));
    assert!(
        !violations.is_empty(),
        "control-plane-pii-free MUST reject its tenancy frame red fixture `{RED}`"
    );
    assert!(
        violations.iter().all(|v| v.lint == LintId("control-plane-pii-free")),
        "every violation on the tenancy red fixture must carry the control-plane-pii-free id"
    );
}

#[test]
fn control_plane_pii_free_admits_the_tenancy_frame_green_fixture() {
    // ADMIT: the frozen four-field CrossCellPointer frame (opaque ids only) produces 0 violations.
    let violations = control_plane_pii_free().run(&read_fixture(GREEN));
    assert!(
        violations.is_empty(),
        "control-plane-pii-free MUST admit its tenancy green fixture `{GREEN}`, but found: {violations:?}"
    );
}

#[test]
fn the_data_map_leg_catches_an_is_personal_field_no_name_fingerprint_would_miss() {
    // THE GENUINELY-NEW P-CP-04 PROPERTY (the canonical §4.3 rule): the red fixture's PII field is
    // named `region_of_birth` — deliberately NOT in the name-fingerprint list — so it is caught
    // ONLY by the data-map leg (its `#[personal_data]` classification = `is_personal=true`). This
    // test proves the data-map leg, not the name leg, is what fires.
    let red = read_fixture(RED);
    assert!(
        red.contains("region_of_birth") && red.contains("#[personal_data"),
        "the red fixture's PII field is a #[personal_data]-tagged, non-name-fingerprinted column"
    );
    assert!(
        !red.contains("\n    email")
            && !red.contains("\n    name")
            && !red.contains("\n    display_name"),
        "the red fixture deliberately carries NO name-fingerprinted PII field — only the data-map \
         tag can catch it"
    );
    let violations = control_plane_pii_free().run(&red);
    assert!(
        violations.iter().any(|v| v.reason.contains("is_personal=true")),
        "the data-map leg must fire (reason names `is_personal=true`), proving the classification — \
         not the field name — caught the leak; got: {violations:?}"
    );
}

#[test]
fn the_tenancy_red_fixture_is_not_caught_by_no_untagged_personal_data() {
    // Cross-lint precision: the red fixture's PII field IS #[personal_data]-tagged, so the
    // `no-untagged-personal-data` lint (which fires only on UNTAGGED PII) does NOT fire — isolating
    // the control-plane rule. This is the OQ-I point: tagging satisfies the data-map but does NOT
    // make PII admissible on the control plane (control-plane-pii-free still fires).
    let red = read_fixture(RED);
    assert!(
        no_untagged_personal_data().run(&red).is_empty(),
        "the tagged field must satisfy no-untagged-personal-data (it is tagged), isolating the \
         control-plane rule"
    );
}

#[test]
fn the_tenancy_red_fixture_trips_exactly_control_plane_pii_free() {
    // Cross-lint isolation: the tenancy frame red fixture is caught by control-plane-pii-free and
    // NO OTHER of the twelve (so the whole-set gate attributes the failure correctly).
    let red = read_fixture(RED);
    let mut firing: Vec<LintId> = Vec::new();
    for lint in all_twelve() {
        if !lint.run(&red).is_empty() {
            firing.push(lint.id);
        }
    }
    assert_eq!(
        firing,
        vec![LintId("control-plane-pii-free")],
        "the tenancy red fixture must trip exactly control-plane-pii-free, but tripped: {firing:?}"
    );
}

#[test]
fn the_full_twelve_set_rejects_the_tenancy_red_and_admits_the_tenancy_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on the tenancy red
    // fixture and Ok on the tenancy green fixture — loud, never swallowed (EI-01 §5).
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(RED)).is_err(),
        "the twelve-lint set must REJECT the tenancy frame red fixture"
    );
    assert!(
        run(&all, &read_fixture(GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the tenancy frame green fixture"
    );
}

#[test]
fn the_lint_guards_the_real_frozen_cross_cell_pointer_frame() {
    // The P-CP-04 frame-guard assertion: the green fixture IS the frozen four-field CrossCellPointer
    // frame (P-CP-02 / P-027), the red fixture is that frame + a fifth is_personal field. The lint
    // admits the four-field frame and rejects the five-field one — guarding the frozen frame against
    // an is_personal=true field exactly as the prompt's GATE names.
    let green = read_fixture(GREEN);
    assert!(
        green.contains("struct CrossCellPointer")
            && green.contains("subject: OpaqueSubjectId")
            && green.contains("home_cell: CellId"),
        "the green fixture must be the real frozen four-field CrossCellPointer frame"
    );
    assert!(control_plane_pii_free().run(&green).is_empty(), "the four-field frame must admit");
    assert!(
        !control_plane_pii_free().run(&read_fixture(RED)).is_empty(),
        "the frame + an is_personal=true fifth field must be rejected"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_the_tenancy_red_fixture_and_zero_on_green() {
    // THE CI-WIRING PROOF (loud, never swallowed — EI-01 §5): the `lint-gate` binary the CI job runs
    // exits NON-ZERO over the tenancy red fixture and ZERO over the tenancy green fixture. A process
    // whose exit code IS the gate cannot be `|| true`-swallowed. `--no-exclude` disables the
    // by-design `/fixtures/` exclusion so the fixture is actually scanned.
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let run_over = |name: &str| -> i32 {
        Command::new(bin)
            .arg("--no-exclude")
            .arg(fixtures_dir().join(name))
            .status()
            .expect("the lint-gate binary must run")
            .code()
            .expect("lint-gate exits with a code, not a signal")
    };
    assert_ne!(run_over(RED), 0, "lint-gate MUST exit non-zero on the tenancy frame red fixture");
    assert_eq!(run_over(GREEN), 0, "lint-gate MUST exit zero on the tenancy frame green fixture");
}
