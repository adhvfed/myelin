use super::*;

#[test]
fn catalogue_contains_only_durable_permission_checked_reads() {
    let definitions = ci_tool_defs();
    assert_eq!(
        definitions
            .iter()
            .map(ToolDef::canonical_name)
            .collect::<Vec<_>>(),
        ["ci.read_log", "ci.read_run"]
    );
    for definition in definitions {
        definition.validate().unwrap();
        assert_eq!(definition.required_caps, [CAP_RUN_VIEW]);
        assert_eq!(definition.effect_kind, EffectKind::Read);
        assert!(!definition.side_effecting);
        assert!(!definition.requires_approval);
        assert!(definition.exposed_over_mcp);
    }
}

#[test]
fn unavailable_ci_agent_actions_cannot_be_turned_into_definitions() {
    for unavailable in [
        "run",
        "run_pipeline",
        "cancel_run",
        "retry_run",
        "validate",
        "plan",
        "deploy",
        "approve_deploy",
        "rollback",
        "write_secret",
    ] {
        assert!(ci_tool_def(unavailable).is_none());
    }
}

#[test]
fn read_schemas_match_the_durable_handler_inputs() {
    let read_run = ci_tool_def("read_run").unwrap();
    assert!(read_run.input_schema.contains("run_id"));
    assert!(!read_run.input_schema.contains("job_id"));

    let read_log = ci_tool_def("read_log").unwrap();
    assert!(read_log.input_schema.contains("run_id"));
    assert!(read_log.input_schema.contains("job_id"));
    assert!(read_log.input_schema.contains("262144"));
}
