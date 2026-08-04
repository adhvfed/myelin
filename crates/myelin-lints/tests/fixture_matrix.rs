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

struct MatrixRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

fn matrix() -> Vec<MatrixRow> {
    vec![
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

const EB08_RED: &str = "no_cross_sync_cycle.eb08.red.rs.txt";
const EB08_GREEN: &str = "no_cross_sync_cycle.eb08.green.rs.txt";

#[test]
fn eb08_write_path_red_fixture_is_rejected() {
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
    let red = read_fixture(EB08_RED);
    let unmarked = red.replace("@write-path", "(removed-marker)");
    let lint = no_cross_sync_cycle();
    assert!(
        lint.run(&unmarked).is_empty(),
        "the EB-08 write-path leg must be INERT on an unmarked source (no `@write-path`), so the \
         lint admits the whole current workspace until producer write paths land"
    );
}

const EB09_RED: &str = "tenant_predicate.eb09.red.rs.txt";
const EB09_GREEN: &str = "tenant_predicate.eb09.green.rs.txt";

#[test]
fn eb09_stream_scope_red_fixture_is_rejected() {
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
    let rows = matrix();
    for (drop_idx, dropped) in rows.iter().enumerate() {
        let reduced: Vec<Lint> = all_twelve()
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != drop_idx)
            .map(|(_, l)| l)
            .collect();
        let red = read_fixture(dropped.red);
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
