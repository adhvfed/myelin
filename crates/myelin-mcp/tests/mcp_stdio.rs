use std::process::Command;

#[test]
fn legacy_binary_points_operators_to_the_edge_owned_cli_bridge() {
    let output = Command::new(env!("CARGO_BIN_EXE_myelin-mcp"))
        .arg("serve")
        .output()
        .expect("spawn myelin-mcp");

    assert!(
        !output.status.success(),
        "the legacy process must never become a second composition root"
    );
    assert!(
        output.stdout.is_empty(),
        "stdout is reserved for MCP JSON-RPC"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("myelin mcp serve --as <agent-id>"),
        "operators get one actionable migration path: {stderr}"
    );
}
