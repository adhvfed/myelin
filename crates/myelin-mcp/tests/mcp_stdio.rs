//! The production binary must never fall back to the former catalogue-only server. Protocol framing
//! is covered in `server`/`governed_routing`; this subprocess pin proves missing durable production
//! configuration fails before stdin is served.

use std::process::Command;

#[test]
fn binary_refuses_to_serve_without_explicit_durable_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_myelin-mcp"))
        .arg("serve")
        .env_remove("DATABASE_URL")
        .env_remove("DATABASE_MIGRATION_URL")
        .env_remove("MYELIN_MCP_CREDENTIAL_FILE")
        .output()
        .expect("spawn myelin-mcp");

    assert!(
        !output.status.success(),
        "catalogue-only fallback must stay dead"
    );
    assert!(
        output.stdout.is_empty(),
        "stdout is reserved for MCP JSON-RPC"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("database bootstrap refused"),
        "missing durable configuration fails loudly: {stderr}"
    );
}
