use base64::Engine as _;
use serde_json::Value;
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

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

async fn read_request(socket: &mut TcpStream) -> String {
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
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("authorization: Bearer {TOKEN}"))),
        "compiled CLI presents the configured bearer credential"
    );
    request
}

async fn send_json(socket: &mut TcpStream, status: &str, value: Value) {
    let body = serde_json::to_vec(&value).unwrap();
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
}

fn sse_wire(events: &[Value]) -> Vec<u8> {
    let mut body = String::from(": connected\n\n");
    for event in events {
        body.push_str("event: ");
        body.push_str(event["event"].as_str().unwrap());
        body.push('\n');
        if let Some(id) = event["id"].as_str() {
            body.push_str("id: ");
            body.push_str(id);
            body.push('\n');
        }
        body.push_str("data: ");
        body.push_str(&event["data"].to_string());
        body.push_str("\n\n");
    }
    body.into_bytes()
}

async fn send_sse(socket: &mut TcpStream, events: &[Value]) {
    let body = sse_wire(events);
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
}

async fn send_abrupt_sse(socket: &mut TcpStream, events: &[Value]) {
    let body = sse_wire(events);
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
              Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    socket
        .write_all(format!("{:x}\r\n", body.len()).as_bytes())
        .await
        .unwrap();
    socket.write_all(&body).await.unwrap();
    socket.write_all(b"\r\n10\r\npartial").await.unwrap();
}

fn cli_command(address: std::net::SocketAddr, args: &[&str]) -> Command {
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
    command
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
    assert!(!stdout.contains("more - run:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_watches_a_terminal_log_from_the_shared_contract() {
    let run = "91000000-0000-4000-8000-000000000001";
    let job = "92000000-0000-4000-8000-000000000001";
    let archive = base64::engine::general_purpose::STANDARD.encode("prep\ncafé\nfailed\n");
    let expected_archive = archive.clone();
    let terminal = golden_expected("live-terminal-job-checkpoints-and-completes");
    let events = terminal["events"].as_array().unwrap().clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for index in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let first = request.lines().next().unwrap();
            match index {
                0 => {
                    assert_eq!(
                        first,
                        format!(
                            "GET /v1/ci/runs/{run}/jobs/{job}/log?start=9007199254740991&limit=1 HTTP/1.1"
                        )
                    );
                    send_json(
                        &mut socket,
                        "200 OK",
                        serde_json::json!({
                            "run_id": run,
                            "job_id": job,
                            "byte_start": 9007199254740991_i64,
                            "byte_end": 9007199254740991_i64,
                            "total_end": 18,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": ""
                        }),
                    )
                    .await;
                }
                1 => {
                    assert_eq!(
                        first,
                        format!("GET /v1/ci/runs/{run}/jobs/{job}/log?start=0&limit=18 HTTP/1.1")
                    );
                    send_json(
                        &mut socket,
                        "200 OK",
                        serde_json::json!({
                            "run_id": run,
                            "job_id": job,
                            "byte_start": 0,
                            "byte_end": 18,
                            "total_end": 18,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": archive
                        }),
                    )
                    .await;
                }
                2 => {
                    assert_eq!(
                        first,
                        format!("GET /v1/ci/runs/{run}/jobs/{job}/log/live HTTP/1.1")
                    );
                    assert!(
                        !request
                            .lines()
                            .any(|line| line.to_ascii_lowercase().starts_with("last-event-id:")),
                        "fresh watch must not invent a resume cursor"
                    );
                    send_sse(&mut socket, &events).await;
                }
                _ => unreachable!(),
            }
        }
    });

    let output = cli_command(address, &["--json", "ci", "watch", run, "--job", job])
        .output()
        .expect("run compiled myelin CLI");
    server.await.unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "watch --json is newline-delimited JSON");
    let range: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(range["run_id"], run);
    assert_eq!(range["job_id"], job);
    assert_eq!(range["byte_start"], 0);
    assert_eq!(range["byte_end"], 18);
    assert_eq!(range["data"], expected_archive);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_stale_resume_catches_up_archive_then_subscribes_fresh() {
    let run = "91000000-0000-4000-8000-000000000002";
    let job = "92000000-0000-4000-8000-000000000002";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for index in 0..9 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            let first = request.lines().next().unwrap();
            match index {
                0 | 6 => {
                    assert!(first.contains("start=9007199254740991&limit=1"));
                    let total = if index == 0 { 5 } else { 16 };
                    send_json(
                        &mut socket,
                        "200 OK",
                        serde_json::json!({
                            "run_id": run,
                            "job_id": job,
                            "byte_start": 9007199254740991_i64,
                            "byte_end": 9007199254740991_i64,
                            "total_end": total,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": ""
                        }),
                    )
                    .await;
                }
                1 => {
                    assert!(first.contains("log?start=0&limit=5"));
                    send_json(
                        &mut socket,
                        "200 OK",
                        serde_json::json!({
                            "run_id": run,
                            "job_id": job,
                            "byte_start": 0,
                            "byte_end": 5,
                            "total_end": 5,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": base64::engine::general_purpose::STANDARD.encode("boot\n")
                        }),
                    )
                    .await;
                }
                2 => {
                    assert!(first.ends_with("/log/live HTTP/1.1"));
                    assert!(!request
                        .lines()
                        .any(|line| line.to_ascii_lowercase().starts_with("last-event-id:")));
                    send_abrupt_sse(
                        &mut socket,
                        &[serde_json::json!({
                            "event": "ci.log.ready",
                            "id": "1",
                            "data": {
                                "run_id": run,
                                "job_id": job,
                                "byte_end": 5
                            }
                        })],
                    )
                    .await;
                }
                3 => {
                    assert!(first.ends_with("/log/live HTTP/1.1"));
                    assert!(request
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("last-event-id: 1")));
                    send_sse(
                        &mut socket,
                        &[serde_json::json!({
                            "event": "ci.log.appended",
                            "id": "2",
                            "data": {
                                "run_id": run,
                                "job_id": job,
                                "byte_start": 5,
                                "byte_end": 11
                            }
                        })],
                    )
                    .await;
                }
                4 => {
                    assert!(first.contains("log?start=5&limit=6"));
                    send_json(
                        &mut socket,
                        "200 OK",
                        serde_json::json!({
                            "run_id": run,
                            "job_id": job,
                            "byte_start": 5,
                            "byte_end": 11,
                            "total_end": 11,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": base64::engine::general_purpose::STANDARD.encode("build\n")
                        }),
                    )
                    .await;
                }
                5 => {
                    assert!(first.ends_with("/log/live HTTP/1.1"));
                    assert!(request
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case("last-event-id: 2")));
                    send_json(
                        &mut socket,
                        "409 Conflict",
                        serde_json::json!({
                            "error": {
                                "code": "conflict",
                                "message": "CI live-log cursor is stale; reload the archive"
                            }
                        }),
                    )
                    .await;
                }
                7 => {
                    assert!(first.contains("log?start=11&limit=5"));
                    send_json(
                        &mut socket,
                        "200 OK",
                        serde_json::json!({
                            "run_id": run,
                            "job_id": job,
                            "byte_start": 11,
                            "byte_end": 16,
                            "total_end": 16,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": base64::engine::general_purpose::STANDARD.encode("test\n")
                        }),
                    )
                    .await;
                }
                8 => {
                    assert!(first.ends_with("/log/live HTTP/1.1"));
                    assert!(!request
                        .lines()
                        .any(|line| line.to_ascii_lowercase().starts_with("last-event-id:")));
                    send_sse(
                        &mut socket,
                        &[
                            serde_json::json!({
                                "event": "ci.log.ready",
                                "id": "3",
                                "data": {
                                    "run_id": run,
                                    "job_id": job,
                                    "byte_end": 16
                                }
                            }),
                            serde_json::json!({
                                "event": "ci.log.complete",
                                "id": "3",
                                "data": {
                                    "run_id": run,
                                    "job_id": job,
                                    "byte_end": 16
                                }
                            }),
                        ],
                    )
                    .await;
                }
                _ => unreachable!(),
            }
        }
    });

    let output = cli_command(address, &["ci", "watch", run, "--job", job])
        .output()
        .expect("run compiled myelin CLI");
    server.await.unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.matches("boot").count(), 1);
    assert_eq!(stdout.matches("build").count(), 1);
    assert_eq!(stdout.matches("test").count(), 1);
    assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_refuses_an_unexpected_live_success_status_without_panicking() {
    let run = "91000000-0000-4000-8000-000000000002";
    let job = "92000000-0000-4000-8000-000000000002";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut probe, _) = listener.accept().await.unwrap();
        let request = read_request(&mut probe).await;
        assert!(request
            .lines()
            .next()
            .unwrap()
            .contains("start=9007199254740991&limit=1"));
        send_json(
            &mut probe,
            "200 OK",
            serde_json::json!({
                "run_id": run,
                "job_id": job,
                "byte_start": 9007199254740991_i64,
                "byte_end": 9007199254740991_i64,
                "total_end": 0,
                "next_offset": null,
                "encoding": "base64",
                "data": ""
            }),
        )
        .await;

        let (mut live, _) = listener.accept().await.unwrap();
        let request = read_request(&mut live).await;
        assert!(request
            .lines()
            .next()
            .unwrap()
            .ends_with("/log/live HTTP/1.1"));
        live.write_all(
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    });

    let output = cli_command(address, &["ci", "watch", run, "--job", job])
        .output()
        .expect("run compiled myelin CLI");
    server.await.unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected success status 204"));
    assert!(!stderr.contains("panicked"));
    assert!(!stderr.contains("backtrace"));
    assert!(!stderr.contains(TOKEN));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compiled_cli_refuses_409_on_a_fresh_subscription_without_retrying() {
    let run = "91000000-0000-4000-8000-000000000002";
    let job = "92000000-0000-4000-8000-000000000002";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut probe, _) = listener.accept().await.unwrap();
        let request = read_request(&mut probe).await;
        assert!(request
            .lines()
            .next()
            .unwrap()
            .contains("start=9007199254740991&limit=1"));
        send_json(
            &mut probe,
            "200 OK",
            serde_json::json!({
                "run_id": run,
                "job_id": job,
                "byte_start": 9007199254740991_i64,
                "byte_end": 9007199254740991_i64,
                "total_end": 0,
                "next_offset": null,
                "encoding": "base64",
                "data": ""
            }),
        )
        .await;

        let (mut live, _) = listener.accept().await.unwrap();
        let request = read_request(&mut live).await;
        assert!(request
            .lines()
            .next()
            .unwrap()
            .ends_with("/log/live HTTP/1.1"));
        assert!(!request
            .lines()
            .any(|line| line.to_ascii_lowercase().starts_with("last-event-id:")));
        send_json(
            &mut live,
            "409 Conflict",
            serde_json::json!({
                "error": {
                    "code": "conflict\u{1b}",
                    "message": "fresh subscriptions cannot be stale\r\n\u{1b}]52;clipboard"
                }
            }),
        )
        .await;
    });

    let output = cli_command(address, &["ci", "watch", run, "--job", job])
        .output()
        .expect("run compiled myelin CLI");
    server.await.unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1);
    assert!(!stderr.contains('\r'));
    assert!(!stderr.contains('\u{1b}'));
    assert!(stderr.contains("409 conflict\\x1b"));
    assert!(stderr.contains("fresh subscriptions cannot be stale\\r\\n\\x1b]52;clipboard"));
    assert!(!stderr.contains(TOKEN));
}
