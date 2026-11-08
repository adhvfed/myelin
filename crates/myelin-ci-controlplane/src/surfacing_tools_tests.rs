use super::*;

#[test]
fn catalogue_contains_only_durable_permission_checked_tools() {
    let definitions = ci_tool_defs();
    assert_eq!(
        definitions
            .iter()
            .map(ToolDef::canonical_name)
            .collect::<Vec<_>>(),
        [
            "ci.cancel_run",
            "ci.read_log",
            "ci.read_run",
            "ci.retry_run"
        ]
    );
    for definition in definitions {
        definition.validate().unwrap();
        if matches!(
            definition.canonical_name().as_str(),
            "ci.cancel_run" | "ci.retry_run"
        ) {
            assert_eq!(definition.required_caps, ["repo.push", CAP_RUN_VIEW]);
            assert_eq!(definition.effect_kind, EffectKind::Mutate);
            assert!(definition.side_effecting);
            assert!(definition.requires_approval);
        } else {
            assert_eq!(definition.required_caps, [CAP_RUN_VIEW]);
            assert_eq!(definition.effect_kind, EffectKind::Read);
            assert!(!definition.side_effecting);
            assert!(!definition.requires_approval);
        }
        assert!(definition.exposed_over_mcp);
    }
}

#[test]
fn unavailable_ci_agent_actions_cannot_be_turned_into_definitions() {
    for unavailable in [
        "run",
        "run_pipeline",
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

    let retry_run = ci_tool_def("retry_run").unwrap();
    assert!(retry_run.input_schema.contains("run_id"));
    assert!(!retry_run.input_schema.contains("job_id"));

    let cancel_run = ci_tool_def("cancel_run").unwrap();
    assert!(cancel_run.input_schema.contains("run_id"));
    assert!(!cancel_run.input_schema.contains("job_id"));
}
