use myelin_lints::lints::flow_determinism;

#[test]
fn flow_determinism_is_green_over_the_workflow_module() {
    let src = include_str!("../src/workflow.rs");
    let violations = flow_determinism().run(src);
    assert!(
        violations.is_empty(),
        "flow-determinism MUST be GREEN over the Issues workflow body (time read through WfCtx): {violations:?}"
    );
}

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
