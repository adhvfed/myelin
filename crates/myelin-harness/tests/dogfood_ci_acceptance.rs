//! Executable contract for the founder CI acceptance verifier.
//!
//! The script itself must consume authenticated production-shaped detail/archive responses, assemble
//! every bounded page exactly, compare the live/archive marker, and fail closed on hostile transport,
//! pagination, encoding, or evidence-file shapes.

use base64::Engine as _;
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

const RUN: &str = "81000000-0000-4000-8000-000000000021";
const JOB: &str = "85000000-0000-4000-8000-000000000021";
const MARKER: &str = "MYELIN-CI-0123456789abcdef0123456789abcdef";
const TOKEN: &str = "v4.public.c2lnbmVkLWJvZHk|W10|dGFpbA";
const PAGE_BYTES: usize = 262_144;

#[derive(Clone, Copy)]
enum Scenario {
    Green,
    DiscontinuousSecond,
    NoncanonicalSecond,
    ShortNonfinal,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("harness is under workspace/crates")
        .to_path_buf()
}

fn evidence_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("myelin-ci-acceptance-{}-{tag}", std::process::id()))
}

fn prepare_evidence(tag: &str) -> PathBuf {
    let directory = evidence_dir(tag);
    if directory.exists() {
        fs::remove_dir_all(&directory).expect("remove stale isolated evidence");
    }
    fs::create_dir_all(&directory).expect("create isolated evidence");
    fs::write(
        directory.join(format!("myelin-ci-live-{RUN}-{JOB}.log")),
        format!("{MARKER}\n"),
    )
    .expect("seed the independently captured live output");
    fs::write(directory.join(".curlrc"), b"verbose\n")
        .expect("seed a curlrc that would disclose headers unless --disable is first");
    directory
}

fn first_page() -> Vec<u8> {
    let mut page = vec![b'x'; PAGE_BYTES];
    let marker_line = format!("{MARKER}\n");
    page[..marker_line.len()].copy_from_slice(marker_line.as_bytes());
    page
}

fn read_request(socket: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while !request.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).expect("read HTTP request");
        request.push(byte[0]);
        assert!(request.len() <= 16 * 1024, "request headers stay bounded");
    }
    String::from_utf8(request).expect("request is HTTP text")
}

fn send_json(socket: &mut TcpStream, value: serde_json::Value) {
    let body = serde_json::to_vec(&value).expect("encode response");
    write!(
        socket,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write response headers");
    socket.write_all(&body).expect("write response body");
}

fn spawn_edge(scenario: Scenario) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Edge");
    listener
        .set_nonblocking(true)
        .expect("make mock Edge accept bounded");
    let address = listener.local_addr().expect("mock Edge address");
    let request_count = if matches!(scenario, Scenario::ShortNonfinal) {
        2
    } else {
        3
    };
    let server = thread::spawn(move || {
        for index in 0..request_count {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "verifier did not make expected request {index}"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept verifier request: {error}"),
                }
            };
            let request = read_request(&mut socket);
            assert!(
                request.contains(&format!("authorization: Bearer {TOKEN}")),
                "verifier authenticates every read"
            );
            assert!(
                request.contains("x-myelin-token-scheme: agent"),
                "verifier sends the configured token scheme"
            );
            match index {
                0 => {
                    assert!(request.starts_with(&format!("GET /v1/ci/runs/{RUN} HTTP/1.1")));
                    send_json(
                        &mut socket,
                        json!({
                            "run": {
                                "run_id": RUN,
                                "state": "succeeded",
                                "cost_settled": true,
                                "finished_at": "2026-07-24T14:00:00.123456Z"
                            },
                            "jobs": [{"job_id": JOB, "state": "succeeded"}],
                            "steps": []
                        }),
                    );
                }
                1 if matches!(scenario, Scenario::ShortNonfinal) => {
                    assert!(request.starts_with(&format!(
                        "GET /v1/ci/runs/{RUN}/jobs/{JOB}/log?start=0&limit={PAGE_BYTES} HTTP/1.1"
                    )));
                    send_json(
                        &mut socket,
                        json!({
                            "run_id": RUN,
                            "job_id": JOB,
                            "byte_start": 0,
                            "byte_end": 12,
                            "total_end": 17,
                            "next_offset": 12,
                            "encoding": "base64",
                            "data": "eHh4eHh4eHh4eHh4"
                        }),
                    );
                }
                1 => {
                    assert!(request.starts_with(&format!(
                        "GET /v1/ci/runs/{RUN}/jobs/{JOB}/log?start=0&limit={PAGE_BYTES} HTTP/1.1"
                    )));
                    send_json(
                        &mut socket,
                        json!({
                            "run_id": RUN,
                            "job_id": JOB,
                            "byte_start": 0,
                            "byte_end": PAGE_BYTES,
                            "total_end": PAGE_BYTES + 5,
                            "next_offset": PAGE_BYTES,
                            "encoding": "base64",
                            "data": base64::engine::general_purpose::STANDARD.encode(first_page())
                        }),
                    );
                }
                2 => {
                    assert!(request.starts_with(&format!(
                        "GET /v1/ci/runs/{RUN}/jobs/{JOB}/log?start={PAGE_BYTES}&limit={PAGE_BYTES} HTTP/1.1"
                    )));
                    let encoded = if matches!(scenario, Scenario::NoncanonicalSecond) {
                        "cmVzdAo"
                    } else {
                        "cmVzdAo="
                    };
                    send_json(
                        &mut socket,
                        json!({
                            "run_id": RUN,
                            "job_id": JOB,
                            "byte_start": if matches!(scenario, Scenario::DiscontinuousSecond) {
                                PAGE_BYTES + 1
                            } else {
                                PAGE_BYTES
                            },
                            "byte_end": PAGE_BYTES + 5,
                            "total_end": PAGE_BYTES + 5,
                            "next_offset": null,
                            "encoding": "base64",
                            "data": encoded
                        }),
                    );
                }
                _ => unreachable!(),
            }
        }
    });
    (address, server)
}

fn command_for(directory: &Path, edge_url: &str) -> Command {
    let mut command = Command::new("bash");
    command
        .arg(workspace_root().join("scripts/dogfood.sh"))
        .args(["verify-ci", RUN, JOB, MARKER])
        .arg(directory)
        .env("MYELIN_TOKEN", TOKEN)
        .env("MYELIN_TOKEN_SCHEME", "agent")
        .env("MYELIN_EDGE_URL", edge_url)
        .env("CURL_HOME", directory);
    command
}

fn assert_token_hidden(output: &Output) {
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
}

fn run_verifier(tag: &str, scenario: Scenario) -> (Output, PathBuf) {
    let directory = prepare_evidence(tag);
    let (address, server) = spawn_edge(scenario);
    let output = command_for(&directory, &format!("http://{address}"))
        .output()
        .expect("execute founder CI verifier");
    if server.join().is_err() {
        panic!(
            "mock Edge did not receive the complete verifier flow\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_token_hidden(&output);
    (output, directory)
}

fn receipt_path(directory: &Path) -> PathBuf {
    directory.join(format!("myelin-ci-acceptance-{RUN}-{JOB}.json"))
}

fn assert_red_without_receipt(output: &Output, directory: &Path, message: &str) {
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "stderr did not contain {message:?}:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_token_hidden(output);
    assert!(
        !receipt_path(directory).exists(),
        "a rejected proof cannot produce a green receipt"
    );
}

#[test]
fn verifier_assembles_every_page_and_emits_a_checksum_receipt() {
    let (output, directory) = run_verifier("green", Scenario::Green);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut expected = first_page();
    expected.extend_from_slice(b"rest\n");
    assert_eq!(
        fs::read(directory.join(format!("myelin-ci-archive-{RUN}-{JOB}.log")))
            .expect("read assembled archive"),
        expected
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(receipt_path(&directory)).expect("read receipt"))
            .expect("receipt JSON");
    assert_eq!(receipt["verified"], true);
    assert_eq!(receipt["archive_bytes"], PAGE_BYTES + 5);
    assert_eq!(receipt["marker_count"], json!({"live": 1, "archive": 1}));
    fs::remove_dir_all(directory).expect("remove isolated green evidence");
}

#[test]
fn verifier_refuses_a_discontinuous_archive_without_a_green_receipt() {
    let (output, directory) = run_verifier("discontinuous", Scenario::DiscontinuousSecond);
    assert_red_without_receipt(
        &output,
        &directory,
        "archive page is malformed or cross-scope",
    );
    fs::remove_dir_all(directory).expect("remove isolated red evidence");
}

#[test]
fn verifier_refuses_a_short_nonfinal_page() {
    let (output, directory) = run_verifier("short-nonfinal", Scenario::ShortNonfinal);
    assert_red_without_receipt(
        &output,
        &directory,
        "archive page is malformed or cross-scope",
    );
    fs::remove_dir_all(directory).expect("remove isolated red evidence");
}

#[test]
fn verifier_refuses_noncanonical_base64() {
    let (output, directory) = run_verifier("noncanonical-base64", Scenario::NoncanonicalSecond);
    assert_red_without_receipt(&output, &directory, "archive page is not canonical base64");
    fs::remove_dir_all(directory).expect("remove isolated red evidence");
}

#[test]
fn verifier_refuses_cleartext_nonloopback_before_sending_the_token() {
    let directory = prepare_evidence("cleartext-nonloopback");
    let output = command_for(&directory, "http://192.0.2.1:8080")
        .output()
        .expect("execute transport refusal");
    assert_red_without_receipt(
        &output,
        &directory,
        "must be an HTTPS origin or an HTTP loopback origin",
    );
    fs::remove_dir_all(directory).expect("remove isolated transport evidence");
}

#[test]
fn verifier_refuses_a_preexisting_evidence_file() {
    let directory = prepare_evidence("preexisting");
    let detail = directory.join(format!("myelin-ci-run-{RUN}.json"));
    fs::write(&detail, b"preserve me").expect("seed pre-existing evidence");
    let output = command_for(&directory, "http://127.0.0.1:9")
        .output()
        .expect("execute overwrite refusal");
    assert_red_without_receipt(
        &output,
        &directory,
        "refusing to overwrite existing or linked",
    );
    assert_eq!(
        fs::read(detail).expect("read preserved evidence"),
        b"preserve me"
    );
    fs::remove_dir_all(directory).expect("remove isolated overwrite evidence");
}

#[cfg(unix)]
#[test]
fn verifier_refuses_a_dangling_evidence_symlink() {
    use std::os::unix::fs::symlink;

    let directory = prepare_evidence("dangling-symlink");
    let receipt = receipt_path(&directory);
    symlink(directory.join("missing-target"), &receipt).expect("seed dangling evidence symlink");
    let output = command_for(&directory, "http://127.0.0.1:9")
        .output()
        .expect("execute symlink refusal");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("refusing to overwrite existing or linked"));
    assert_token_hidden(&output);
    assert!(fs::symlink_metadata(&receipt)
        .expect("preserve dangling link")
        .file_type()
        .is_symlink());
    fs::remove_dir_all(directory).expect("remove isolated symlink evidence");
}
