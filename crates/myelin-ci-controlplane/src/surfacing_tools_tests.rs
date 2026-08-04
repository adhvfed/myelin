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

#[test]
fn x6_requires_approval_defaults_are_frozen_correct() {
    for gated in ["deploy", "approve_deploy", "rollback", "write_secret"] {
        assert!(
            ci_requires_approval_default(gated),
            "ci.{gated} is HITL-gated (X-6 - consequential/privileged)"
        );
    }
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

#[test]
fn an_unknown_ci_action_fails_closed_to_gated() {
    assert!(
        ci_requires_approval_default("nuke_prod"),
        "an unrecognised CI action is gated until the frozen table is extended"
    );
}

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
    assert_eq!(CAP_RUN_TRIGGER, "run.trigger");
    assert_eq!(CAP_RUN_VIEW, "run.view");
    assert_eq!(CAP_ENVIRONMENT_DEPLOY, "environment.deploy");
    assert_eq!(CAP_CI_PROJECT_ADMINISTER, "ci_project.administer");
}

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
        assert_eq!(
            def.exposed_over_mcp,
            matches!(*tool, "read_run" | "read_log"),
            "only the two durable permission-checked reads are MCP-exposed"
        );
    }
}

#[test]
fn register_ci_tools_registers_the_complete_x6_set() {
    let mut cat = Catalogue { defs: vec![] };
    let registered = register_ci_tools(&mut cat);
    assert_eq!(
        registered.len(),
        CI_TOOL_NAMES.len(),
        "the complete X-6 CI tool set (12 rows)"
    );

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
