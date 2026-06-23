//! Unit tests for [`crate::surfacing_tools`] — the FROZEN X-6 `requires_approval` defaults
//! (deploy/secret = yes; run/read = no), the effect-kind/side-effecting split, and the complete-set
//! registration into the ONE ToolSurface (8.1).

use super::*;

struct Catalogue {
    defs: Vec<ToolDef>,
}
impl ToolSurface for Catalogue {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

/// **THE X-6 GATE (arch 04 §3): the `requires_approval` defaults are frozen-correct — deploy / secret
/// / rollback / approve_deploy = YES; run / read / validate / plan / cancel / retry = NO.** A
/// loosening of any `yes` or a gating of any read would flip a value here.
#[test]
fn x6_requires_approval_defaults_are_frozen_correct() {
    // gated (yes) — consequential / privileged.
    for gated in ["deploy", "approve_deploy", "rollback", "write_secret"] {
        assert!(
            ci_requires_approval_default(gated),
            "ci.{gated} is HITL-gated (X-6 — consequential/privileged)"
        );
    }
    // not gated (no) — cheap / reversible / read.
    for not_gated in [
        "run",
        "run_pipeline",
        "cancel_run",
        "retry_run",
        "read_log",
        "read_run",
        "validate",
        "plan",
    ] {
        assert!(
            !ci_requires_approval_default(not_gated),
            "ci.{not_gated} is cheap/reversible/read → NOT gated (X-6)"
        );
    }
}

/// **An UNKNOWN CI action fails CLOSED to gated (a new consequential action is added to the frozen
/// table, never silently un-gated).**
#[test]
fn an_unknown_ci_action_fails_closed_to_gated() {
    assert!(
        ci_requires_approval_default("nuke_prod"),
        "an unrecognised CI action is gated until the frozen table is extended"
    );
}

/// **The effect-kind split (arch 04 §3): the reads are `Read` (not side-effecting); everything else
/// is `Mutate` (routed through EffectApi::apply, side-effecting).**
#[test]
fn effect_kind_and_side_effecting_split() {
    for read in ["read_log", "read_run", "validate", "plan"] {
        assert_eq!(
            ci_effect_kind(read),
            EffectKind::Read,
            "ci.{read} is a read"
        );
        assert!(
            !ci_side_effecting(read),
            "ci.{read} is not side-effecting (a read)"
        );
    }
    for mutate in [
        "run",
        "run_pipeline",
        "cancel_run",
        "retry_run",
        "deploy",
        "approve_deploy",
        "rollback",
        "write_secret",
    ] {
        assert_eq!(
            ci_effect_kind(mutate),
            EffectKind::Mutate,
            "ci.{mutate} is a mutate (plan-then-apply)"
        );
        assert!(ci_side_effecting(mutate), "ci.{mutate} is side-effecting");
    }
}

/// **The `required_caps` are the frozen CI ReBAC permissions (4.9), not invented here.** deploy /
/// approve / rollback → `environment.deploy`; write_secret → `ci_project.administer`; reads →
/// `run.view`; run lifecycle → `run.trigger`.
#[test]
fn required_caps_are_the_ci_rebac_fragment_permissions() {
    assert_eq!(ci_required_caps("deploy"), vec!["environment.deploy"]);
    assert_eq!(
        ci_required_caps("approve_deploy"),
        vec!["environment.deploy"]
    );
    assert_eq!(ci_required_caps("rollback"), vec!["environment.deploy"]);
    assert_eq!(
        ci_required_caps("write_secret"),
        vec!["ci_project.administer"]
    );
    assert_eq!(ci_required_caps("read_log"), vec!["run.view"]);
    assert_eq!(ci_required_caps("read_run"), vec!["run.view"]);
    assert_eq!(ci_required_caps("run"), vec!["run.trigger"]);
    assert_eq!(ci_required_caps("validate"), vec!["run.trigger"]);
    // the caps name the canonical CI fragment permissions (4.9), keyed on the run/env/project objects.
    assert_eq!(CAP_RUN_TRIGGER, "run.trigger");
    assert_eq!(CAP_RUN_VIEW, "run.view");
    assert_eq!(CAP_ENVIRONMENT_DEPLOY, "environment.deploy");
    assert_eq!(CAP_CI_PROJECT_ADMINISTER, "ci_project.administer");
}

/// **A built `ToolDef` SEEDS its `requires_approval` from the frozen X-6 table (never hand-set).**
#[test]
fn tool_def_seeds_the_frozen_x6_gating() {
    for tool in CI_TOOL_NAMES {
        let def = ci_tool_def(tool);
        assert_eq!(
            def.requires_approval,
            ci_requires_approval_default(tool),
            "ci.{tool}'s gating IS the frozen X-6 default (seeded, not hand-set)"
        );
        assert_eq!(def.subsystem, "ci");
        assert_eq!(def.version, CI_TOOL_VERSION);
        assert!(!def.exposed_over_mcp, "not MCP-exposed at v1");
    }
}

/// **`register_ci_tools` registers the COMPLETE set into the ONE catalogue (8.1) — every X-6 row
/// resolves by name with its frozen gating.** And the exactly-four gated set is the privileged CI
/// gates; everything else is not gated.
#[test]
fn register_ci_tools_registers_the_complete_x6_set() {
    let mut cat = Catalogue { defs: vec![] };
    let registered = register_ci_tools(&mut cat);
    assert_eq!(
        registered.len(),
        CI_TOOL_NAMES.len(),
        "the complete X-6 CI tool set (12 rows)"
    );

    // deploy resolves and is gated; read_run resolves and is not gated.
    let deploy = cat
        .resolve(&ToolName("deploy".into()))
        .expect("deploy registered");
    assert!(deploy.requires_approval, "the registered deploy is gated");
    let read_run = cat
        .resolve(&ToolName("read_run".into()))
        .expect("read_run registered");
    assert!(
        !read_run.requires_approval,
        "the registered read_run is NOT gated"
    );

    // the gated set is exactly the four privileged CI gates.
    let gated: Vec<&str> = registered
        .iter()
        .filter(|d| d.requires_approval)
        .map(|d| d.name.0.as_str())
        .collect();
    assert_eq!(
        gated,
        vec!["deploy", "approve_deploy", "rollback", "write_secret"],
        "the four privileged/consequential CI gates; everything else is not gated"
    );
}

/// **`ToolHands::exec` (the runner) is DELIBERATELY ABSENT from the table (X-6 / 05 §HP-5).** The
/// runner is never a side-effecting tool — there is no `exec` ToolDef.
#[test]
fn tool_hands_exec_is_not_a_tool_def() {
    assert!(
        !CI_TOOL_NAMES.contains(&"exec"),
        "ToolHands::exec is the runner itself, never a side-effecting ToolDef (X-6)"
    );
    let mut cat = Catalogue { defs: vec![] };
    register_ci_tools(&mut cat);
    assert!(
        cat.resolve(&ToolName("exec".into())).is_none(),
        "no exec ToolDef is registered (the runner is not a tool)"
    );
}
