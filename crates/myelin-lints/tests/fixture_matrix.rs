//! THE fixture matrix (the P-S10 → P-017 required test, EXTENDED to twelve in P-S11 → P-018).
//!
//! For each of the TWELVE architecture lints: run its RED fixture and assert the lint REJECTS it
//! (≥1 violation, fired by THAT lint), run its GREEN fixture and assert the lint ADMITS it (0
//! violations). The full 12×(red-reject + green-admit) matrix is the dated green artifact.
//!
//! Plus the ratchet regression test: removing any one of the twelve lints' wiring must make the
//! matrix fail (the gate cannot be silently un-wired — EI-01 §5).

use myelin_lints::engine::run;
use myelin_lints::lints::{
    all_twelve, control_plane_pii_free, flow_determinism, forward_only_migration, no_cross_db,
    no_cross_sync_cycle, no_host_exec, no_llm_in_platform, no_raw_publish,
    no_untagged_personal_data, residency_pin, search_requires_acl_filter, tenant_predicate,
};
use myelin_lints::{Lint, LintId};

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_fixture(name: &str) -> String {
    let path = format!("{FIXTURES_DIR}/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
}

/// One row of the matrix: a lint + its red fixture file + its green fixture file.
struct MatrixRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

/// The full TWELVE-row matrix, in §2.11 table order (the four load-bearing then the eight).
fn matrix() -> Vec<MatrixRow> {
    vec![
        // ---- the four load-bearing (P-S10 → P-017) ----
        MatrixRow {
            lint: tenant_predicate,
            id: LintId("tenant-predicate"),
            red: "tenant_predicate.red.rs.txt",
            green: "tenant_predicate.green.rs.txt",
        },
        MatrixRow {
            lint: no_raw_publish,
            id: LintId("no-raw-publish"),
            red: "no_raw_publish.red.rs.txt",
            green: "no_raw_publish.green.rs.txt",
        },
        MatrixRow {
            lint: no_host_exec,
            id: LintId("no-host-exec"),
            red: "no_host_exec.red.rs.txt",
            green: "no_host_exec.green.rs.txt",
        },
        MatrixRow {
            lint: no_untagged_personal_data,
            id: LintId("no-untagged-personal-data"),
            red: "no_untagged_personal_data.red.rs.txt",
            green: "no_untagged_personal_data.green.rs.txt",
        },
        // ---- the remaining eight (P-S11 → P-018) ----
        MatrixRow {
            lint: no_cross_db,
            id: LintId("no-cross-db"),
            red: "no_cross_db.red.rs.txt",
            green: "no_cross_db.green.rs.txt",
        },
        MatrixRow {
            lint: forward_only_migration,
            id: LintId("forward-only-migration"),
            red: "forward_only_migration.red.rs.txt",
            green: "forward_only_migration.green.rs.txt",
        },
        MatrixRow {
            lint: no_cross_sync_cycle,
            id: LintId("no-cross-sync-cycle"),
            red: "no_cross_sync_cycle.red.rs.txt",
            green: "no_cross_sync_cycle.green.rs.txt",
        },
        MatrixRow {
            lint: residency_pin,
            id: LintId("residency-pin"),
            red: "residency_pin.red.rs.txt",
            green: "residency_pin.green.rs.txt",
        },
        MatrixRow {
            lint: control_plane_pii_free,
            id: LintId("control-plane-pii-free"),
            red: "control_plane_pii_free.red.rs.txt",
            green: "control_plane_pii_free.green.rs.txt",
        },
        MatrixRow {
            lint: search_requires_acl_filter,
            id: LintId("search-requires-acl-filter"),
            red: "search_requires_acl_filter.red.rs.txt",
            green: "search_requires_acl_filter.green.rs.txt",
        },
        MatrixRow {
            lint: no_llm_in_platform,
            id: LintId("no-llm-in-platform"),
            red: "no_llm_in_platform.red.rs.txt",
            green: "no_llm_in_platform.green.rs.txt",
        },
        MatrixRow {
            lint: flow_determinism,
            id: LintId("flow-determinism"),
            red: "flow_determinism.red.rs.txt",
            green: "flow_determinism.green.rs.txt",
        },
    ]
}

#[test]
fn the_matrix_has_a_row_per_lint() {
    // 12/12: the matrix covers exactly the twelve lints, in §2.11 order.
    let rows = matrix();
    assert_eq!(rows.len(), 12, "the matrix must cover all twelve lints");
    let row_ids: Vec<LintId> = rows.iter().map(|r| r.id).collect();
    let lint_ids: Vec<LintId> = all_twelve().iter().map(|l| l.id).collect();
    assert_eq!(
        row_ids, lint_ids,
        "matrix rows must match the twelve lints in order"
    );
}

#[test]
fn every_red_fixture_is_rejected() {
    // 12/12 REJECT: each lint's red fixture produces ≥1 violation, fired by THAT lint.
    for row in matrix() {
        let lint = (row.lint)();
        let src = read_fixture(row.red);
        let violations = lint.run(&src);
        assert!(
            !violations.is_empty(),
            "lint `{}` MUST reject its red fixture `{}`, but found 0 violations",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "lint `{}`'s violations must all carry its own id",
            row.id
        );
    }
}

#[test]
fn every_green_fixture_is_admitted() {
    // 12/12 ADMIT: each lint's green fixture produces 0 violations.
    for row in matrix() {
        let lint = (row.lint)();
        let src = read_fixture(row.green);
        let violations = lint.run(&src);
        assert!(
            violations.is_empty(),
            "lint `{}` MUST admit its green fixture `{}`, but found: {:?}",
            row.id,
            row.green,
            violations
        );
    }
}

#[test]
fn the_full_twelve_lint_set_run_rejects_each_red_and_admits_each_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on each red
    // fixture and Ok on each green fixture. This is the loud-never-swallowed surface.
    //
    // NOTE: each red fixture is crafted to trip EXACTLY ONE lint, and each GREEN fixture is
    // crafted so NO lint fires over it (not just the row's own lint) — so the whole-set run is
    // Ok. This is asserted explicitly below so a cross-lint false-positive is caught loudly.
    let all = all_twelve();
    for row in matrix() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT green fixture `{}` (no lint may false-positive on \
             another lint's green fixture)",
            row.green
        );
    }
}

#[test]
fn each_red_fixture_trips_exactly_its_own_lint() {
    // Cross-lint isolation: a red fixture for lint X must be caught by X and by NO OTHER lint.
    // This keeps the ratchet-regression test below sound (dropping X leaves X's red un-caught).
    for row in matrix() {
        let all = all_twelve();
        let red = read_fixture(row.red);
        let mut firing: Vec<LintId> = Vec::new();
        for lint in &all {
            if !lint.run(&red).is_empty() {
                firing.push(lint.id);
            }
        }
        assert_eq!(
            firing,
            vec![row.id],
            "red fixture `{}` must trip exactly its own lint `{}`, but tripped: {:?}",
            row.red,
            row.id,
            firing
        );
    }
}

// ================================================================================================
// EB-08 → P-044: the Bus's OWNED slice of `no-cross-sync-cycle` — the WRITE-PATH leg.
//
// The P-S11 → P-018 form (the matrix rows above) is the Identity-sink half (the `@identity-sink`
// fixtures). EB-08 is the Bus's owned slice of the SAME contract-1.6 lint, keyed to the broader
// canonical rule (refined-arch event-bus §7.1 + 00-reconciliation §X-1): a SYNCHRONOUS
// cross-subsystem call in a WRITE PATH (the "is it green?" sync call) is rejected; reading the local
// projection / reacting over the bus is admitted. These tests are the EB-08 red+green fixture
// obligation — BOTH fixtures are the pass condition (a lint that only rejects, or only admits, is
// not proven). The fixtures are scoped by the loud, named `// @write-path` marker, so they exercise
// a genuinely NEW bug fingerprint the Identity-sink leg does not catch.
// ================================================================================================

const EB08_RED: &str = "no_cross_sync_cycle.eb08.red.rs.txt";
const EB08_GREEN: &str = "no_cross_sync_cycle.eb08.green.rs.txt";

#[test]
fn eb08_write_path_red_fixture_is_rejected() {
    // The EB-08 red fixture (a sync `call_sync` cross-subsystem RPC in an `@write-path` merge-gate)
    // MUST be rejected by `no-cross-sync-cycle`, fired by THAT lint.
    let lint = no_cross_sync_cycle();
    let violations = lint.run(&read_fixture(EB08_RED));
    assert!(
        !violations.is_empty(),
        "no-cross-sync-cycle MUST reject the EB-08 write-path red fixture (the \"is it green?\" \
         sync call), but found 0 violations"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint == LintId("no-cross-sync-cycle")),
        "every EB-08 red-fixture violation must carry the no-cross-sync-cycle id"
    );
}

#[test]
fn eb08_write_path_green_fixture_is_admitted() {
    // The EB-08 green fixture (the merge gate reads its OWN cell-local projection, no sync RPC) MUST
    // be admitted — proving the lint does not over-reject (both fixtures are the EB-08 pass cond.).
    let lint = no_cross_sync_cycle();
    let violations = lint.run(&read_fixture(EB08_GREEN));
    assert!(
        violations.is_empty(),
        "no-cross-sync-cycle MUST admit the EB-08 write-path green fixture (the projection read), \
         but found: {violations:?}"
    );
}

#[test]
fn eb08_write_path_red_trips_exactly_its_own_lint() {
    // Cross-lint isolation: the EB-08 write-path red fixture must be caught by no-cross-sync-cycle
    // and by NO OTHER of the twelve lints (so the whole-set CI gate rejects it for the right reason).
    let red = read_fixture(EB08_RED);
    let mut firing: Vec<LintId> = Vec::new();
    for lint in all_twelve() {
        if !lint.run(&red).is_empty() {
            firing.push(lint.id);
        }
    }
    assert_eq!(
        firing,
        vec![LintId("no-cross-sync-cycle")],
        "the EB-08 write-path red fixture must trip exactly no-cross-sync-cycle, but tripped: {firing:?}"
    );
}

#[test]
fn eb08_write_path_green_is_admitted_by_the_full_twelve_set() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on the EB-08 red and
    // Ok on the EB-08 green (no lint false-positives on the projection-read green).
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(EB08_RED)).is_err(),
        "the twelve-lint set must REJECT the EB-08 write-path red fixture"
    );
    assert!(
        run(&all, &read_fixture(EB08_GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the EB-08 write-path green fixture (the projection read)"
    );
}

#[test]
fn eb08_write_path_leg_is_inert_without_the_marker() {
    // The write-path leg is scoped by the loud, named `// @write-path` marker (EI-01 §4): a sync
    // cross-subsystem call OUTSIDE a marked write path is NOT this lint's concern (it admits the
    // whole current no-write-path-yet workspace until the producer write paths land). Strip the
    // marker from the red fixture → the leg goes inert (0 violations). This proves the gate does not
    // over-reach: it fires only where a write path is actually being scanned.
    let red = read_fixture(EB08_RED);
    let unmarked = red.replace("@write-path", "(removed-marker)");
    let lint = no_cross_sync_cycle();
    assert!(
        lint.run(&unmarked).is_empty(),
        "the EB-08 write-path leg must be INERT on an unmarked source (no `@write-path`), so the \
         lint admits the whole current workspace until producer write paths land"
    );
}

// ================================================================================================
// EB-09 → P-045: the Bus's OWNED slice of `tenant-predicate` — the SUBSCRIBE/STREAM-SCOPE leg.
//
// The P-S10 → P-017 form (the `tenant_predicate.{red,green}` matrix rows above) is the DATA-STORE
// half (a query-builder call that is not tenant-bound). EB-09 is the Bus's owned slice of the SAME
// contract-1.6 lint, keyed to the canonical stream rule (refined-arch event-bus §4.2 "whitelist
// subjects, never `*`" + §7.1 "a stream is provisioned per (tenant, subsystem)" + §4.3 "scope is a
// bounded selector, never `*`"): an UNSCOPED subscribe (no (tenant, subsystem) scope) or a WILDCARD
// subscribe (`scope = *`, an `evt.>`/`*` wildcard subject, an "all streams" scope) is rejected; a
// bounded (tenant, subsystem) StreamScope is admitted. These tests are the EB-09 red+green fixture
// obligation — BOTH fixtures are the pass condition (a lint that only rejects, or only admits, is
// not proven). The fixtures are scoped by the loud, named `// @bus-stream` marker, so they exercise
// a genuinely NEW bug fingerprint the data-store leg does not catch.
// ================================================================================================

const EB09_RED: &str = "tenant_predicate.eb09.red.rs.txt";
const EB09_GREEN: &str = "tenant_predicate.eb09.green.rs.txt";

#[test]
fn eb09_stream_scope_red_fixture_is_rejected() {
    // The EB-09 red fixture (an unscoped, wildcard-subject `subscribe` in a `@bus-stream` consumer)
    // MUST be rejected by `tenant-predicate`, fired by THAT lint.
    let lint = tenant_predicate();
    let violations = lint.run(&read_fixture(EB09_RED));
    assert!(
        !violations.is_empty(),
        "tenant-predicate MUST reject the EB-09 stream-scope red fixture (the unscoped/wildcard \
         subscribe), but found 0 violations"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint == LintId("tenant-predicate")),
        "every EB-09 red-fixture violation must carry the tenant-predicate id"
    );
}

#[test]
fn eb09_stream_scope_green_fixture_is_admitted() {
    // The EB-09 green fixture (a bounded (tenant, subsystem) StreamScope subscribe) MUST be admitted
    // — proving the lint does not over-reject (both fixtures are the EB-09 pass condition).
    let lint = tenant_predicate();
    let violations = lint.run(&read_fixture(EB09_GREEN));
    assert!(
        violations.is_empty(),
        "tenant-predicate MUST admit the EB-09 stream-scope green fixture (the bounded \
         (tenant, subsystem) StreamScope), but found: {violations:?}"
    );
}

#[test]
fn eb09_stream_scope_red_trips_exactly_its_own_lint() {
    // Cross-lint isolation: the EB-09 red fixture must be caught by tenant-predicate and by NO OTHER
    // of the twelve lints (so the whole-set CI gate rejects it for the right reason).
    let red = read_fixture(EB09_RED);
    let mut firing: Vec<LintId> = Vec::new();
    for lint in all_twelve() {
        if !lint.run(&red).is_empty() {
            firing.push(lint.id);
        }
    }
    assert_eq!(
        firing,
        vec![LintId("tenant-predicate")],
        "the EB-09 stream-scope red fixture must trip exactly tenant-predicate, but tripped: {firing:?}"
    );
}

#[test]
fn eb09_stream_scope_green_is_admitted_by_the_full_twelve_set() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on the EB-09 red and
    // Ok on the EB-09 green (no lint false-positives on the bounded-scope green).
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(EB09_RED)).is_err(),
        "the twelve-lint set must REJECT the EB-09 stream-scope red fixture"
    );
    assert!(
        run(&all, &read_fixture(EB09_GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the EB-09 stream-scope green fixture (the bounded scope)"
    );
}

#[test]
fn eb09_stream_scope_leg_is_inert_without_the_marker() {
    // The stream-scope leg is scoped by the loud, named `// @bus-stream` marker (EI-01 §4): an
    // unscoped/wildcard subscribe OUTSIDE a marked bus-stream surface is NOT this leg's concern (it
    // admits the whole current no-subscribe-surface-yet workspace until EB-05/EB-21 land). Strip the
    // marker from the red fixture → the leg goes inert (0 violations from THIS leg). This proves the
    // gate does not over-reach: it fires only where a bus subscribe surface is actually scanned.
    let red = read_fixture(EB09_RED);
    let unmarked = red.replace("@bus-stream", "(removed-marker)");
    let lint = tenant_predicate();
    assert!(
        lint.run(&unmarked).is_empty(),
        "the EB-09 stream-scope leg must be INERT on an unmarked source (no `@bus-stream`), so the \
         lint admits the whole current workspace until the bus subscribe surface lands"
    );
}

#[test]
fn eb09_unscoped_subscribe_without_wildcard_is_rejected() {
    // The unscoped-but-not-wildcard fingerprint: a subscribe with a concrete subject but NO
    // (tenant, subsystem) scope token. Proves the leg's "missing scope" branch (not only the
    // wildcard branch) fires — a bus subscribe must carry a (tenant, subsystem) scope (§7.1).
    let src = "// @bus-stream\nfn run(bus: &Bus) { bus.subscribe(my_subject(), cursor); }\n";
    let lint = tenant_predicate();
    let violations = lint.run(src);
    assert!(
        violations.iter().any(|v| v.reason.contains("no (tenant, subsystem) scope")),
        "an unscoped (non-wildcard) bus subscribe must trip the missing-scope branch, got: {violations:?}"
    );
}

#[test]
fn removing_any_lint_breaks_the_matrix() {
    // THE RATCHET REGRESSION TEST: if any one of the twelve lints is un-wired, that lint's red
    // fixture is no longer rejected by the remaining set — the matrix detects the missing gate.
    // Proves the gate cannot be silently un-wired (EI-01 §5).
    let rows = matrix();
    for (drop_idx, dropped) in rows.iter().enumerate() {
        // The set with lint `drop_idx` removed.
        let reduced: Vec<Lint> = all_twelve()
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != drop_idx)
            .map(|(_, l)| l)
            .collect();
        let red = read_fixture(dropped.red);
        // With its lint removed, the red fixture that ONLY that lint catches is now admitted by
        // the reduced set → the gate is missing. (Each red fixture is crafted to trip exactly
        // one lint, asserted by `each_red_fixture_trips_exactly_its_own_lint`.)
        let full_violations = run(&all_twelve(), &red);
        assert!(
            full_violations.is_err(),
            "full set must reject {}",
            dropped.red
        );
        let reduced_violations = run(&reduced, &red);
        assert!(
            reduced_violations.is_ok(),
            "removing lint `{}` must leave its red fixture `{}` UN-caught (the ratchet \
             regression: an un-wired gate is detectable)",
            dropped.id,
            dropped.red
        );
    }
}
