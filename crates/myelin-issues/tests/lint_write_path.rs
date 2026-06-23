//! **ISS-P06 / P-372 (M4) — the `no-raw-publish` lint is GREEN over the Issues write path (the GATE
//! half), with a RED witness proving the gate still REJECTS a raw publish.**
//!
//! The silent-data-loss-safe write path ([`myelin_issues::write_path`]) emits its `issue.*` events
//! through the ONE sanctioned verb `OutboxTx::emit` — there is NO fire-and-forget publish path. This
//! file runs the REAL shared lint ([`myelin_lints::lints::no_raw_publish`], contract 1.6 / P-019)
//! over the live write-path source and asserts **0 raw-publish call sites** (the prompt's GATE: "the
//! no-raw-publish lint is GREEN — 0 publish_now call sites; the emit is the only path"), plus a RED
//! fixture proving the lint is not vacuous.
//!
//! **Reconciliation (EI-01 §7).** The lint + its engine are the SHARED substrate's (P-S10 / EB-07).
//! This file CONFIRMS the gate in place over the Issues write-path source — it does not re-define the
//! lint. (The workspace live scan excludes `crates/*/tests/`, so the RED fixture below — a string the
//! lint must reject — does not turn the workspace scan red.)

use myelin_lints::lints::no_raw_publish;

/// **GREEN — the lint finds 0 raw-publish call sites in the live write-path source.** Every `issue.*`
/// event the write path emits goes through `OutboxTx::emit`; there is no `.publish_now(` / `.publish(`
/// / `transport.put(` / `bus.put(` CALL site. The gate is green from ISS-P06.
#[test]
fn no_raw_publish_is_green_over_the_write_path_source() {
    let src = include_str!("../src/write_path.rs");
    let violations = no_raw_publish().run(src);
    assert!(
        violations.is_empty(),
        "no-raw-publish MUST be GREEN over the Issues write path (emit is the only path): {violations:?}"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "no-raw-publish"),
        "every violation (none expected) carries the no-raw-publish id"
    );
}

/// **RED — the lint REJECTS a deliberately raw-published Issues write.** A write-path handler that
/// calls `bus.publish(..)` directly bypasses the outbox (the lost-event / causality-break bug class).
/// The gate fires (it is not vacuous) — proving the GREEN result above is a real, earned green.
#[test]
fn no_raw_publish_rejects_a_raw_published_issue_write() {
    let red = "\
fn create_issue_BAD(bus: &Bus, ev: IssueEvent) {
    // BUG: a fire-and-forget publish OUTSIDE the outbox transaction — the event can be lost if the
    // state commit fails, or ghost-published if it never committed. This is exactly what ISS-P06
    // forbids.
    bus.publish(ev);
}";
    let violations = no_raw_publish().run(red);
    assert!(
        !violations.is_empty(),
        "no-raw-publish MUST reject a raw bus.publish( in an Issues write path (the lost-event bug)"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "no-raw-publish"),
        "every violation carries the no-raw-publish id (no false attribution)"
    );
}

/// **The residency-pin lint stays GREEN over the write-path source** (no request-derived region
/// write — the write path threads `ctx_base.region` from the cell, never `req.region`). Every Issues
/// write pins `row.region == cell.region` (contract 1.6); the source-scan half confirms no
/// request-derived region write was introduced.
#[test]
fn residency_pin_is_green_over_the_write_path_source() {
    use myelin_lints::lints::residency_pin;
    let src = include_str!("../src/write_path.rs");
    assert!(
        residency_pin().run(src).is_empty(),
        "residency-pin MUST be GREEN over the Issues write-path source (no request-derived region write)"
    );
}
