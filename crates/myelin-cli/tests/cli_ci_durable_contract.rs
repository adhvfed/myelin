//! CT-005d — the compiled CLI consumes the same durable CI contract artifact as Rust Edge and the
//! dev Edge. This is a client contract proof, not a second Edge implementation: it asserts the exact
//! request target emitted by the binary and renders the artifact's already-gated response.

use base64::Engine as _;
use serde_json::Value;
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: &str = "ci-cli-contract-token";

fn canonical_cursor() -> String {
    let mut frame = [0_u8; 60];
    frame[0] = 1;
    format!(
        "cr1_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
    )
}

fn golden_expected(id: &str) -> Value {
    let contract: Value = serde_json::from_str(include_str!(
        "../../../contracts/ci-read-dev-edge.golden.json"
    ))
    .expect("CI contract artifact is JSON");
    let mut expected = contract["vectors"]
        .as_array()
        .expect("vectors")
        .iter()
        .find(|vector| vector["id"] == id)
        .unwrap_or_else(|| panic!("missing CI vector {id}"))["expected"]
        .clone();
    expected
        .as_object_mut()
        .expect("expected response is an object")
        .remove("status");
    expected
}

async fn run_cli_against_response(
    expected_target: &str,
    response: Value,
    args: &[&str],
) -> (i32, String, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let expected_target = expected_target.to_string();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 2048];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "CLI closed before completing HTTP headers");
            request.extend_from_slice(&buffer[..read]);
            assert!(
                request.len() <= 16 * 1024,
                "request headers are unexpectedly large"
            );
        }
        let request = String::from_utf8(request).expect("CLI request headers are UTF-8");
        let first_line = request.lines().next().expect("request line");
        assert_eq!(first_line, format!("GET {expected_target} HTTP/1.1"));
        assert!(
            request
                .lines()
                .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {TOKEN}"))),
            "compiled CLI presents the configured bearer credential"
        );

        let body = serde_json::to_vec(&response).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(headers.as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
    });

    let mut command = Command::new(env!("CARGO_BIN_EXE_myelin"));
    command
        .env("MYELIN_EDGE", format!("http://{address}"))
        .env("MYELIN_TOKEN_SCHEME", "agent")
        .env("MYELIN_TOKEN", TOKEN)
        .env(
            "MYELIN_CONFIG_DIR",
            std::env::temp_dir().join("myelin-cli-ci-contract-empty"),
        )
        .args(args);
    let output = command.output().expect("run compiled myelin CLI");
    server.await.unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_lists_views_and_reads_archived_ci_output_from_the_shared_contract() {
    let mut list = golden_expected("runs-first-page-keyset");
    let cursor = canonical_cursor();
    list["page"]["next_cursor"] = Value::String(cursor.clone());
    let (code, stdout, stderr) = run_cli_against_response(
        "/v1/ci/runs?state=all&limit=1",
        list,
        &["ci", "list", "--limit", "1"],
    )
    .await;
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("✗ failed"));
    assert!(stdout.contains(&format!(
        "myelin ci list --status all --limit 1 --cursor {cursor}"
    )));

    let run = "91000000-0000-4000-8000-000000000001";
    let job = "92000000-0000-4000-8000-000000000001";
    let (code, stdout, stderr) = run_cli_against_response(
        &format!("/v1/ci/runs/{run}"),
        golden_expected("failed-run-detail"),
        &["ci", "view", run],
    )
    .await;
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("test/contract"));
    assert!(stdout.contains(&format!("myelin ci logs {run} --job {job}")));

    let (code, stdout, stderr) = run_cli_against_response(
        &format!("/v1/ci/runs/{run}/jobs/{job}/log?start=9&limit=7"),
        golden_expected("archived-log-byte-range"),
        &[
            "ci", "logs", run, "--job", job, "--start", "9", "--limit", "7",
        ],
    )
    .await;
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("archived log bytes 9..16 of 18"));
    assert!(stdout.contains("\\xa9\nfail"));
    assert!(stdout.contains(&format!(
        "myelin ci logs {run} --job {job} --start 16 --limit 7"
    )));
    assert!(!stdout.contains("watch"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_rejects_a_mismatched_success_before_rendering_it() {
    let requested_run = "91000000-0000-4000-8000-000000000001";
    let requested_job = "92000000-0000-4000-8000-000000000001";
    let mut response = golden_expected("archived-log-byte-range");
    response["job_id"] = Value::String("92000000-0000-4000-8000-000000000002".into());

    let (code, stdout, stderr) = run_cli_against_response(
        &format!("/v1/ci/runs/{requested_run}/jobs/{requested_job}/log?start=9&limit=7"),
        response,
        &[
            "ci",
            "logs",
            requested_run,
            "--job",
            requested_job,
            "--start",
            "9",
            "--limit",
            "7",
        ],
    )
    .await;

    assert_ne!(code, 0);
    assert!(
        stdout.is_empty(),
        "a malformed success must not be rendered"
    );
    assert!(stderr.contains("malformed CI success response"));
    assert!(!stderr.contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_accepts_the_production_beyond_end_empty_range() {
    let run = "91000000-0000-4000-8000-000000000001";
    let job = "92000000-0000-4000-8000-000000000001";
    let (code, stdout, stderr) = run_cli_against_response(
        &format!("/v1/ci/runs/{run}/jobs/{job}/log?start=100&limit=7"),
        serde_json::json!({
            "run_id": run,
            "job_id": job,
            "byte_start": 100,
            "byte_end": 100,
            "total_end": 18,
            "next_offset": null,
            "encoding": "base64",
            "data": ""
        }),
        &[
            "ci", "logs", run, "--job", job, "--start", "100", "--limit", "7",
        ],
    )
    .await;

    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("archived log bytes 100..100 of 18"));
    assert!(!stdout.contains("more — run:"));
}

#[test]
fn compiled_cli_refuses_to_claim_live_ci_watch() {
    let output = Command::new(env!("CARGO_BIN_EXE_myelin"))
        .args(["ci", "watch", "91000000-0000-4000-8000-000000000001"])
        .output()
        .expect("run compiled myelin CLI");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cross-service resumable live logs"));
}
