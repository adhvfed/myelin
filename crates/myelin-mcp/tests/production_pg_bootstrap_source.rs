//! Deployment guard for the MCP binary's privileged migration-to-runtime handoff.

#[test]
fn production_mcp_requires_both_git_pr_indexes_before_runtime_handoff() {
    let source = include_str!("../src/main.rs");

    let migrations = source
        .find("&myelin_git::pg_pr_store::git_pr_migrations()")
        .expect("Git PR migrations must run through PgBootstrap");
    let head_index = source
        .find("verify_index_ready(\"git_pr_head_repo_idx\")")
        .expect("Git PR provenance index must be ready before serving");
    let operation_index = source
        .find("verify_index_ready(\"git_pr_command_operation_scope_uidx\")")
        .expect("Git PR operation namespace index must be ready before serving");
    let handoff = source
        .find("bootstrap\n        .into_runtime()")
        .expect("bootstrap must be consumed by the runtime handoff");

    assert!(migrations < head_index);
    assert!(head_index < operation_index);
    assert!(operation_index < handoff);
}
