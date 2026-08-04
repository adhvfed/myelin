use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{
    all_twelve, forward_only_migration, no_untagged_personal_data, tenant_predicate,
};
use myelin_lints::{Lint, LintId};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

struct AgentRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

fn agent_matrix() -> Vec<AgentRow> {
    vec![
        AgentRow {
            lint: tenant_predicate,
            id: LintId("tenant-predicate"),
            red: "tenant_predicate.agent.red.rs.txt",
            green: "tenant_predicate.agent.green.rs.txt",
        },
        AgentRow {
            lint: forward_only_migration,
            id: LintId("forward-only-migration"),
            red: "forward_only_migration.agent.red.rs.txt",
            green: "forward_only_migration.agent.green.rs.txt",
        },
        AgentRow {
            lint: no_untagged_personal_data,
            id: LintId("no-untagged-personal-data"),
            red: "no_untagged_personal_data.agent.red.rs.txt",
            green: "no_untagged_personal_data.agent.green.rs.txt",
        },
    ]
}

#[test]
fn the_agent_lints_reject_their_agent_red_fixtures() {
    for row in agent_matrix() {
        let lint = (row.lint)();
        let violations = lint.run(&read_fixture(row.red));
        assert!(
            !violations.is_empty(),
            "agent lint `{}` MUST reject its red fixture `{}`",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "agent lint `{}`'s violations must all carry its own id",
            row.id
        );
    }
}

#[test]
fn the_agent_lints_admit_their_agent_green_fixtures() {
    for row in agent_matrix() {
        let lint = (row.lint)();
        let violations = lint.run(&read_fixture(row.green));
        assert!(
            violations.is_empty(),
            "agent lint `{}` MUST admit its green fixture `{}`, but found: {:?}",
            row.id,
            row.green,
            violations
        );
    }
}

#[test]
fn each_agent_red_fixture_trips_exactly_its_own_lint() {
    for row in agent_matrix() {
        let mut firing: Vec<LintId> = Vec::new();
        let red = read_fixture(row.red);
        for lint in all_twelve() {
            if !lint.run(&red).is_empty() {
                firing.push(lint.id);
            }
        }
        assert_eq!(
            firing,
            vec![row.id],
            "agent red fixture `{}` must trip exactly its own lint `{}`, but tripped: {:?}",
            row.red,
            row.id,
            firing
        );
    }
}

#[test]
fn the_full_twelve_set_rejects_each_agent_red_and_admits_each_agent_green() {
    let all = all_twelve();
    for row in agent_matrix() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT agent red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT agent green fixture `{}` (no lint may false-positive)",
            row.green
        );
    }
}

#[test]
fn ci_gate_exits_non_zero_on_an_agent_red_fixture_and_zero_on_green() {
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
    assert_ne!(
        run_over("tenant_predicate.agent.red.rs.txt"),
        0,
        "lint-gate MUST exit non-zero on the agent tenant-predicate red fixture"
    );
    assert_ne!(
        run_over("forward_only_migration.agent.red.rs.txt"),
        0,
        "lint-gate MUST exit non-zero on the agent forward-only-migration red fixture"
    );
    assert_eq!(
        run_over("tenant_predicate.agent.green.rs.txt"),
        0,
        "lint-gate MUST exit zero on the agent tenant-predicate green fixture"
    );
    assert_eq!(
        run_over("forward_only_migration.agent.green.rs.txt"),
        0,
        "lint-gate MUST exit zero on the agent forward-only-migration green fixture"
    );
    assert_ne!(
        run_over("no_untagged_personal_data.agent.red.rs.txt"),
        0,
        "lint-gate MUST exit non-zero on the agent no-untagged-personal-data red fixture"
    );
    assert_eq!(
        run_over("no_untagged_personal_data.agent.green.rs.txt"),
        0,
        "lint-gate MUST exit zero on the agent no-untagged-personal-data green fixture"
    );
}

#[test]
fn the_agent_no_untagged_red_names_the_untagged_field_and_green_is_admitted() {
    let red = read_fixture("no_untagged_personal_data.agent.red.rs.txt");
    let violations = no_untagged_personal_data().run(&red);
    assert_eq!(
        violations.len(),
        1,
        "exactly the untagged Fabric body field must fire, got: {violations:?}"
    );
    assert_eq!(
        violations[0].lint,
        LintId("no-untagged-personal-data"),
        "the agent red-fixture violation must carry the no-untagged-personal-data id"
    );
    assert!(
        violations[0].reason.contains("message_body"),
        "the violation must name the untagged Fabric body field, got: {:?}",
        violations[0]
    );

    let green = read_fixture("no_untagged_personal_data.agent.green.rs.txt");
    assert!(
        no_untagged_personal_data().run(&green).is_empty(),
        "the fully-tagged Fabric row must be admitted (no false positive)"
    );
}
