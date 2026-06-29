//! # Integration: the `myelin-mcp` BINARY as a real JSON-RPC stdio peer (subprocess).
//!
//! Spawns the actual `myelin-mcp` binary, writes newline-delimited JSON-RPC to its stdin, and reads
//! its stdout — proving the protocol shell works end-to-end over real pipes (the framing local Claude
//! / Claude Code drives it over). Covers `initialize`, `tools/list` (the git tools sourced from
//! `agent_tools()`), the no-panic-over-malformed rule, and the HONEST "governed routing not wired"
//! response for `tools/call` (the standalone binary constructs no minter/crypto — the governed
//! `mint_run_token → EffectApi::apply` path is proven in `governed_routing.rs`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// Send the given JSON-RPC request lines to the binary and collect the response lines.
fn run_peer(requests: &[&str]) -> Vec<serde_json::Value> {
    let exe = env!("CARGO_BIN_EXE_myelin-mcp");
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn myelin-mcp");

    {
        let mut stdin = child.stdin.take().unwrap();
        for r in requests {
            writeln!(stdin, "{r}").unwrap();
        }
        // Drop stdin → EOF → the server's run loop returns.
    }

    let stdout = child.stdout.take().unwrap();
    let responses: Vec<serde_json::Value> = BufReader::new(stdout)
        .lines()
        .map(|l| l.unwrap())
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(&l).expect("response line is JSON"))
        .collect();
    child.wait().unwrap();
    responses
}

#[test]
fn binary_handshakes_lists_tools_and_stays_total_over_malformed() {
    let resps = run_peer(&[
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "{ this is not valid json",
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"git.merge"}}"#,
    ]);

    // initialize, tools/list, the malformed-error, the tools/call error = 4 responses (the
    // notification produced none).
    assert_eq!(resps.len(), 4, "the notification yields no response");

    // initialize → capabilities + server info.
    assert_eq!(resps[0]["id"], 1);
    assert_eq!(resps[0]["result"]["serverInfo"]["name"], "myelin-mcp");
    assert!(resps[0]["result"]["capabilities"]["tools"].is_object());

    // tools/list → the git tools sourced from agent_tools(), with the frozen requiresApproval flags.
    let tools = resps[1]["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"git.merge"));
    assert!(names.contains(&"git.open_pr"));
    let merge = tools.iter().find(|t| t["name"] == "git.merge").unwrap();
    assert_eq!(merge["annotations"]["requiresApproval"], true);

    // The malformed line is a JSON-RPC parse error — the server did NOT panic / crash (it kept
    // serving the subsequent request).
    assert_eq!(resps[2]["error"]["code"], -32700);

    // tools/call on the standalone binary is honestly "governed routing not wired" (-32004): the
    // per-run minter + the EffectApi body are injected by the composition root, not the protocol shell.
    assert_eq!(resps[3]["error"]["code"], -32004);
}
