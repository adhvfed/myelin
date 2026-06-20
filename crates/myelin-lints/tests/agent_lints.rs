//! The TWO Agent-Fabric data-model lints, the agent-relevant slice (AG-P2 → global P-131).
//!
//! AG-P2 ships the Agent-Fabric data model — the five `(tenant, region)`-first RLS migrations
//! (`run`/`tool_def`/`proposed_effect`/`hitl_gate`/`trace`, architecture §4) in the
//! `myelin-agent-service` crate. The two CONSTRAINING lints (contract 1.6) are the
//! `tenant-predicate` (no cross-tenant query path) + `forward-only-migration` (online
//! expand→backfill→contract, no DROP/in-place rewrite) ratchet gates. Their generic scanners + the
//! engine were first shipped by the substrate prompt P-S10/P-S11 → P-017/P-018; rather than
//! duplicate a parallel scanner (EI-01 §7), this slice ships the AGENT-SHAPED red+green fixtures that
//! prove BOTH lints reject the agent-fabric bug fingerprint and admit the agent-fabric correct shape.
//!
//! These tests ARE the AG-P2 lint fixtures (the TESTS field: "the tenant-predicate red+green
//! fixtures"). They run loud over the agent fixtures and assert the exact verdict; the CI-wiring
//! proof (an agent red fixture ⇒ the `lint-gate` binary exits non-zero, no `|| true` swallow) is the
//! last test. No threshold is weakened: a lint is never softened to admit a red fixture (EI-01 §5).

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, forward_only_migration, tenant_predicate};
use myelin_lints::{Lint, LintId};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

/// One agent-lint row: the lint + its agent red fixture + its agent green fixture.
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
    ]
}

#[test]
fn the_two_agent_lints_reject_their_agent_red_fixtures() {
    // 2/2 REJECT: each agent red fixture produces >= 1 violation, fired by THAT lint.
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
fn the_two_agent_lints_admit_their_agent_green_fixtures() {
    // 2/2 ADMIT: each agent green fixture produces 0 violations from ITS lint.
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
    // Cross-lint isolation over the agent fixtures: a red fixture for lint X is caught by X and NO
    // OTHER of the twelve (so the whole-set gate attributes the failure correctly).
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
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on each agent red
    // fixture and Ok on each agent green fixture — loud, never swallowed.
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
    // THE CI-WIRING PROOF (loud, never swallowed — EI-01 §5): the `lint-gate` binary the CI job runs
    // exits NON-ZERO over an agent red fixture and ZERO over the agent green fixture. A process whose
    // exit code IS the gate cannot be `|| true`-swallowed. `--no-exclude` disables the by-design
    // `/fixtures/` exclusion so the fixture is actually scanned.
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
}
