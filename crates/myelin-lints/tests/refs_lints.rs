//! The four Refs lints wired with Refs-specific red+green fixtures — the REFS ownership prompt
//! (REF-P2 → global P-053). The M0 ratchet half of R-M0.
//!
//! `myelin-refs` (REF-P1 / P-052) ships the `ArtifactRef` value type; this prompt wires the FOUR
//! architecture lints Refs leans on (contract-index 1.6) into CI with Refs-specific fixtures, each a
//! red sample the lint MUST reject + a green sample it MUST admit (refined-arch reference-graph §3):
//!   - `tenant-predicate` — every `edge`-table query carries the tenant predicate (no cross-tenant
//!     query path; ID-3). RED: a tenant-less backlink read.
//!   - `no-raw-publish` — no edge escapes the outbox; there is no standalone edge-write API (5.4,
//!     emit-iff-committed). RED: a fire-and-forget broker publish.
//!   - `no-cross-db` — Refs never reads an owner's DB, only project/events (ADR-01). RED: a reach
//!     into Issues' internal store.
//!   - `no-cross-sync-cycle` — every cross-subsystem edge is async (event/projection), never a sync
//!     call (acyclicity; EI-02 §3). RED: a sync RPC in the resolution path.
//!
//! **Coherence note (EI-01 §7 — reconcile, never duplicate; coherence/survey rule).** The four
//! lints, the engine, the `lint-gate` CI binary, and the central fixtures were already shipped
//! CENTRALLY by the substrate prompts P-S10 → P-017 (the four load-bearing lints) and the Bus's
//! owned slices EB-07/EB-08/EB-09 (P-019/P-044/P-045). The REF-P2 DELIVERABLE names this exact case:
//! *"If the M0 substrate prompt already ships these four lints centrally, this prompt instead adds
//! the Refs-specific red+green fixtures … and confirms they are wired loud."* So this prompt adds NO
//! new lint id, re-defines NO scanner or type, and creates NO parallel implementation. It REUSES the
//! in-place scanners, attaching Refs-shaped fixtures + this verdict test — exactly mirroring the
//! `tenancy_control_plane_lints.rs` / `identity_lints.rs` / `storage_lints.rs` / `gdpr_audit_lints.rs`
//! precedent. CASE: the lints are CENTRAL; Refs adds fixtures + confirms loud CI wiring.
//!
//! These tests ARE the REF-P2 fixtures (the TESTS field: the red+green fixture pair for each of the
//! four lints, each proven to reject its red and admit its green). They run loud over the Refs
//! fixtures and assert the exact verdict; the CI-wiring proof (a Refs red fixture ⇒ the `lint-gate`
//! binary exits non-zero, no `|| true` swallow) is the last test. No threshold is weakened.
//!
//! **FLOOR — none (permanent ratchet, not a feature; EI-01 §1).** These four lints are permanent M0
//! ratchet gates, not a deliverable that later tightens: every later Refs prompt's DEFINITION OF DONE
//! requires these four green. (The genuinely-new Refs ENGINE the lints will guard live — the resolver
//! / backlink crux REF-P9..REF-P11, the outbox-only edge emit REF-P9, the per-viewer chokepoint
//! REF-P10 — lands in R-M2; when it does, these same gates fire on its real code, no new lint needed.)

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{
    all_twelve, no_cross_db, no_cross_sync_cycle, no_raw_publish, tenant_predicate,
};
use myelin_lints::{Lint, LintId};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

/// One Refs lint row: the lint, its id, and its Refs-specific red + green fixture files.
struct RefsRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

/// The FOUR Refs lints, each with its Refs-specific red + green fixture (contract-index 1.6).
fn refs_rows() -> Vec<RefsRow> {
    vec![
        RefsRow {
            lint: tenant_predicate,
            id: LintId("tenant-predicate"),
            red: "tenant_predicate.refs.red.rs.txt",
            green: "tenant_predicate.refs.green.rs.txt",
        },
        RefsRow {
            lint: no_raw_publish,
            id: LintId("no-raw-publish"),
            red: "no_raw_publish.refs.red.rs.txt",
            green: "no_raw_publish.refs.green.rs.txt",
        },
        RefsRow {
            lint: no_cross_db,
            id: LintId("no-cross-db"),
            red: "no_cross_db.refs.red.rs.txt",
            green: "no_cross_db.refs.green.rs.txt",
        },
        RefsRow {
            lint: no_cross_sync_cycle,
            id: LintId("no-cross-sync-cycle"),
            red: "no_cross_sync_cycle.refs.red.rs.txt",
            green: "no_cross_sync_cycle.refs.green.rs.txt",
        },
    ]
}

#[test]
fn there_are_exactly_four_refs_lints() {
    // The REF-P2 obligation is the FOUR lints Refs leans on — no more, no less.
    assert_eq!(
        refs_rows().len(),
        4,
        "REF-P2 wires exactly the four Refs lints"
    );
}

#[test]
fn every_refs_red_fixture_is_rejected_by_its_own_lint() {
    // REJECT: each Refs red fixture produces >= 1 violation, fired by THAT lint and no other id.
    for row in refs_rows() {
        let violations = (row.lint)().run(&read_fixture(row.red));
        assert!(
            !violations.is_empty(),
            "lint `{}` MUST reject its Refs red fixture `{}`, but found 0 violations",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "lint `{}`'s violations on `{}` must all carry its own id",
            row.id,
            row.red
        );
    }
}

#[test]
fn every_refs_green_fixture_is_admitted_by_its_own_lint() {
    // ADMIT: each Refs green fixture produces 0 violations from its own lint (no over-rejection).
    for row in refs_rows() {
        let violations = (row.lint)().run(&read_fixture(row.green));
        assert!(
            violations.is_empty(),
            "lint `{}` MUST admit its Refs green fixture `{}`, but found: {violations:?}",
            row.id,
            row.green
        );
    }
}

#[test]
fn each_refs_red_fixture_trips_exactly_its_own_lint() {
    // Cross-lint isolation: a Refs red fixture for lint X is caught by X and by NO OTHER of the
    // twelve — so the whole-set CI gate rejects it for the RIGHT reason (and the per-system gate is
    // attributed correctly). Mirrors the central matrix's `each_red_fixture_trips_exactly_its_own_lint`.
    for row in refs_rows() {
        let red = read_fixture(row.red);
        let mut firing: Vec<LintId> = Vec::new();
        for lint in all_twelve() {
            if !lint.run(&red).is_empty() {
                firing.push(lint.id);
            }
        }
        assert_eq!(
            firing,
            vec![row.id],
            "the Refs red fixture `{}` must trip exactly `{}`, but tripped: {firing:?}",
            row.red,
            row.id
        );
    }
}

#[test]
fn the_full_twelve_set_rejects_each_refs_red_and_admits_each_refs_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on each Refs red
    // fixture and Ok on each Refs green fixture — loud, never swallowed (EI-01 §5). No Refs green
    // fixture may false-positive on ANY of the twelve lints.
    let all = all_twelve();
    for row in refs_rows() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT the Refs red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT the Refs green fixture `{}` (no lint may false-positive)",
            row.green
        );
    }
}

#[test]
fn the_marker_scoped_refs_legs_are_inert_without_their_marker() {
    // The two marker-keyed legs Refs exercises — `no-raw-publish` rides no marker (it is the
    // always-on broker-publish fingerprint), but `no-cross-sync-cycle` uses the `@write-path` marker
    // (EI-01 §4) so it fires ONLY where a write path is being scanned. Stripping the marker from the
    // Refs `no-cross-sync-cycle` red fixture makes that leg go inert (0 violations) — proving the gate
    // does not over-reach into ordinary Refs code that legitimately calls other services off the
    // write path. (`tenant-predicate`'s data-store leg and `no-cross-db` are structural, no marker.)
    let red = read_fixture("no_cross_sync_cycle.refs.red.rs.txt");
    let unmarked = red.replace("@write-path", "(removed-marker)");
    assert!(
        no_cross_sync_cycle().run(&unmarked).is_empty(),
        "the no-cross-sync-cycle write-path leg must be INERT on an unmarked Refs source, so the lint \
         admits the whole current workspace until the Refs resolution write path lands (REF-P9..P11)"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_each_refs_red_fixture_and_zero_on_each_green() {
    // THE CI-WIRING PROOF (loud, never swallowed — EI-01 §5; the REF-P2 GATE row): the `lint-gate`
    // binary the CI `architecture-lints` job runs exits NON-ZERO over every Refs red fixture and ZERO
    // over every Refs green fixture. A process whose exit code IS the gate cannot be `|| true`-
    // swallowed. `--no-exclude` disables the by-design `/fixtures/` exclusion so the fixture is
    // actually scanned. This is the "wired into CI, loud, never swallowed" obligation, proven per
    // fixture for all four Refs lints.
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
    for row in refs_rows() {
        assert_ne!(
            run_over(row.red),
            0,
            "lint-gate MUST exit non-zero on the Refs red fixture `{}` (loud, never swallowed)",
            row.red
        );
        assert_eq!(
            run_over(row.green),
            0,
            "lint-gate MUST exit zero on the Refs green fixture `{}`",
            row.green
        );
    }
}
