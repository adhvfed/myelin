//! **ISS-P12 / P-378 (M4) — the flow-determinism lint is GREEN over the Issues workflow module (the
//! GATE half), with a RED witness proving the gate still REJECTS a non-deterministic workflow body.**
//!
//! The prompt's GATE: "the flow-determinism lint holds on any workflow body that schedules a durable
//! activity (a post-action that arms a trigger) — CI, lint green." The arm-trigger post-action
//! ([`myelin_issues::PostAction::ArmTrigger`]) schedules a durable activity; its arming body
//! ([`myelin_issues::arm_trigger_body`], marked `// @workflow-body`) reads time ONLY through the
//! deterministic `WfCtx` clock (`ctx.now()` passed in) — NEVER a raw `SystemTime::now()` / `rand::` /
//! `tokio::time::sleep(` — so the workflow replays deterministically (contract 1.6 / index 9.2/OQ-F).
//!
//! This file runs the REAL shared lint ([`myelin_lints::lints::flow_determinism`]) over the live
//! `workflow.rs` source and asserts **0 violations**, plus a RED fixture proving the gate is not
//! vacuous. The lint + its engine are the SHARED substrate's (P-S10/P-S11); this file CONFIRMS the
//! gate in place over the Issues workflow source — it does not re-define the lint.

use myelin_lints::lints::flow_determinism;

/// **GREEN — the flow-determinism lint finds 0 violations in the live workflow module.** The
/// arm-trigger workflow body reads time through `WfCtx` (the passed-in `ctx_now_seconds` = `ctx.now()`),
/// never a raw clock/rng/IO call — the gate is green from ISS-P12.
#[test]
fn flow_determinism_is_green_over_the_workflow_module() {
    let src = include_str!("../src/workflow.rs");
    let violations = flow_determinism().run(src);
    assert!(
        violations.is_empty(),
        "flow-determinism MUST be GREEN over the Issues workflow body (time read through WfCtx): {violations:?}"
    );
}

/// **RED — the lint REJECTS a workflow body that reads the raw clock.** A `// @workflow-body`-marked
/// arm-trigger that calls `SystemTime::now()` directly bypasses `WfCtx` (the replay-divergence bug
/// class). The gate fires (it is not vacuous) — proving the GREEN result above is a real, earned green.
/// (The workspace live scan excludes `crates/*/tests/`, so this red fixture string does not turn the
/// workspace scan red.)
#[test]
fn flow_determinism_rejects_a_raw_clock_in_a_workflow_body() {
    let red = "\
// @workflow-body the arm-trigger durable activity\n\
fn arm_trigger_body() {\n\
    let now = std::time::SystemTime::now();\n\
    schedule(now);\n\
}\n";
    let violations = flow_determinism().run(red);
    assert!(
        !violations.is_empty(),
        "the flow-determinism lint MUST reject a raw SystemTime::now() in a workflow body"
    );
    assert!(
        violations.iter().all(|v| v.lint.0 == "flow-determinism"),
        "every violation carries the flow-determinism id"
    );
}
