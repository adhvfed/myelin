//! The KNOWLEDGE slice of the `no-untagged-personal-data` ratchet (KN-P25 → P-315).
//!
//! Knowledge is `PersonalDataHolder` holder H4 (architecture knowledge-platform 03 §6) — the
//! "hardest GDPR surface in Myelin" (§6). KN-P25 applies the `#[personal_data(...)]` classification
//! tags on the Knowledge schema (author/edit attribution + mention nodes + free-text body + db-row
//! person props + agent-trace authorship — `myelin_knowledge::gdpr::KnowledgePersonRecord`) so the
//! `no-untagged-personal-data` lint (contract 10.2 / 1.6) is GREEN on the Knowledge schema. These
//! tests are the red+green fixture pair proving the lint REJECTS a deliberately-untagged Knowledge
//! PII field and ADMITS the fully-tagged Knowledge row — both verdicts are the GATE (a lint that only
//! rejects, or only admits, is not proven). The live workspace scan (`workspace_clean.rs`)
//! additionally runs all twelve lints over `myelin-knowledge/src`, so the actual shipped Knowledge
//! schema is held green by the same gate.

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, no_untagged_personal_data};
use myelin_lints::LintId;

const KN_RED: &str = "no_untagged_personal_data.knowledge.red.rs.txt";
const KN_GREEN: &str = "no_untagged_personal_data.knowledge.green.rs.txt";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))
}

#[test]
fn the_knowledge_red_fixture_is_rejected() {
    // Reject leg: the Knowledge person record's untagged `comment_text` body escapes the per-subject
    // DEK crypto-shred fan-out (§6.1; KN-P26) — the lint MUST reject it, fired by THAT lint, on
    // exactly the untagged body field.
    let violations = no_untagged_personal_data().run(&read_fixture(KN_RED));
    assert_eq!(
        violations.len(),
        1,
        "exactly the untagged Knowledge body field must fire, got: {violations:?}"
    );
    assert!(
        violations[0].lint == LintId("no-untagged-personal-data"),
        "the Knowledge red-fixture violation must carry the no-untagged-personal-data id"
    );
    assert!(
        violations[0].reason.contains("comment_text"),
        "the violation must name the untagged `comment_text` body field, got: {:?}",
        violations[0]
    );
}

#[test]
fn the_knowledge_green_fixture_is_admitted() {
    // Admit leg: every PII field of the Knowledge person record carries the canonical six-tag tag
    // (attribution ⇒ Pseudonymise, body ⇒ CryptoShred), so the lint MUST admit it.
    let violations = no_untagged_personal_data().run(&read_fixture(KN_GREEN));
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST admit the fully-tagged Knowledge green fixture, found: {violations:?}"
    );
}

#[test]
fn the_knowledge_red_trips_exactly_its_own_lint() {
    // Cross-lint isolation: the Knowledge red fixture is caught by no-untagged-personal-data and by
    // NO OTHER of the twelve lints, so the whole-set CI gate rejects it for the right reason.
    let red = read_fixture(KN_RED);
    let firing: Vec<LintId> = all_twelve()
        .into_iter()
        .filter(|lint| !lint.run(&red).is_empty())
        .map(|lint| lint.id)
        .collect();
    assert_eq!(
        firing,
        vec![LintId("no-untagged-personal-data")],
        "the Knowledge red fixture must trip exactly no-untagged-personal-data, but tripped: {firing:?}"
    );
}

#[test]
fn the_full_twelve_set_rejects_the_knowledge_red_and_admits_the_knowledge_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on the Knowledge red
    // and Ok on the Knowledge green (no lint false-positives on the canonical six-tag Knowledge row).
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(KN_RED)).is_err(),
        "the twelve-lint set must REJECT the Knowledge red fixture (untagged PII body)"
    );
    assert!(
        run(&all, &read_fixture(KN_GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the Knowledge green fixture (canonical six-tag tags)"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_the_knowledge_red_fixture_and_zero_on_green() {
    // The CI-wiring proof (loud, never swallowed — EI-01 §5): the `lint-gate` binary the CI
    // `architecture-lints` job runs exits NON-ZERO over the Knowledge red fixture and ZERO over the
    // Knowledge green fixture. `--no-exclude` disables the by-design `/fixtures/` exclusion so the
    // fixture is actually scanned.
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
        run_over(KN_RED),
        0,
        "lint-gate MUST exit non-zero on the Knowledge red fixture (untagged PII body fails the build)"
    );
    assert_eq!(
        run_over(KN_GREEN),
        0,
        "lint-gate MUST exit zero on the Knowledge green fixture (canonical six-tag tags admitted)"
    );
}
