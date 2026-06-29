//! # The edge HTTP client — present the Bearer capability token, parse the envelopes.
//!
//! The CLI is an edge CLIENT: it calls the MR-014 gateway's `/v1/...` routes over HTTP/1.1 (hyper
//! 1.x directly — the SAME minimal transport stack the edge uses; no reqwest), presenting the
//! capability token as `Authorization: Bearer <token>` + the `x-myelin-token-scheme` the gateway
//! reads. It parses the uniform `{items,page}` list envelope and the `{error:{message,code}}` error
//! envelope, and maps a `401` to a clean [`CliError::Unauthorized`] (NOT a panic).
//!
//! **The token is NEVER logged.** It is placed only in the `Authorization` header; no error path
//! interpolates it, and the header is not printed.

use crate::config::EdgeConfig;
use crate::dispatch::EdgeCall;
use crate::error::CliError;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpStream;

/// `(host:port, origin-form target)` parsed from the base URL + the call's path/query. Rejects
/// `https://` with a clean error (the CLI client speaks `http` to the dev edge; TLS termination is a
/// deployment concern — a named seam, never a silent downgrade).
fn target(config: &EdgeConfig, call: &EdgeCall) -> Result<(String, String), CliError> {
    let base = config.url.trim_end_matches('/');
    let authority = if let Some(rest) = base.strip_prefix("http://") {
        rest
    } else if base.starts_with("https://") {
        return Err(CliError::Transport(
            "the CLI edge client speaks http only (https/TLS termination is a deployment concern — a \
             named seam)".into(),
        ));
    } else {
        return Err(CliError::Transport(format!("edge URL must start with http:// (got `{base}`)")));
    };
    // The authority is everything before the first '/'; any base path is folded ahead of the route.
    let (host, base_path) = match authority.find('/') {
        Some(i) => (authority[..i].to_string(), &authority[i..]),
        None => (authority.to_string(), ""),
    };
    if host.is_empty() {
        return Err(CliError::Transport("edge URL has no host".into()));
    }
    let mut origin = format!("{base_path}{}", call.path);
    if let Some(q) = &call.query {
        origin.push('?');
        origin.push_str(q);
    }
    Ok((host, origin))
}

/// **Run an [`EdgeCall`] against the edge** with the Bearer capability token, returning the parsed
/// JSON body on success. Total: a connect/transport failure is [`CliError::Transport`]; a `401` is
/// [`CliError::Unauthorized`]; any other non-2xx with the `{error:{message}}` envelope is
/// [`CliError::Edge`]. No panic, ever.
pub async fn execute(config: &EdgeConfig, token: &str, call: &EdgeCall) -> Result<Value, CliError> {
    let (host, origin) = target(config, call)?;

    let stream = TcpStream::connect(&host)
        .await
        .map_err(|e| CliError::Transport(format!("connect {host}: {e}")))?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|e| CliError::Transport(format!("http handshake: {e}")))?;
    // Drive the connection in the background; it ends when the response is read.
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut builder = Request::builder()
        .method(call.method.as_str())
        .uri(&origin)
        .header("host", &host)
        .header("accept", "application/json")
        // The Bearer capability token + the scheme the gateway reads. The token is presented ONLY
        // here; it is never logged.
        .header("authorization", format!("Bearer {token}"))
        .header("x-myelin-token-scheme", &config.scheme);
    let payload = call.payload.clone().unwrap_or_default();
    if call.payload.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Full::new(Bytes::from(payload)))
        .map_err(|e| CliError::Transport(format!("build request: {e}")))?;

    let response = sender
        .send_request(request)
        .await
        .map_err(|e| CliError::Transport(format!("send request: {e}")))?;
    let status = response.status().as_u16();
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|e| CliError::Transport(format!("read response body: {e}")))?
        .to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    interpret(status, body)
}

/// Map an HTTP `(status, body)` to a success value or a typed [`CliError`], parsing the
/// `{error:{message,code}}` envelope. Separated so it is unit-testable without a socket.
pub fn interpret(status: u16, body: Value) -> Result<Value, CliError> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let message = body
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("(no message)")
        .to_string();
    let code = body
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if status == 401 {
        return Err(CliError::Unauthorized(message));
    }
    Err(CliError::Edge { status, code, message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{EdgeCall, HttpMethod};
    use serde_json::json;

    fn cfg(url: &str) -> EdgeConfig {
        EdgeConfig { url: url.into(), scheme: "agent".into() }
    }
    fn get(path: &str, query: Option<&str>) -> EdgeCall {
        EdgeCall { method: HttpMethod::Get, path: path.into(), query: query.map(str::to_string), payload: None }
    }

    #[test]
    fn target_builds_origin_form_and_rejects_https() {
        let (host, origin) = target(&cfg("http://127.0.0.1:8080"), &get("/v1/git/repos", None)).unwrap();
        assert_eq!(host, "127.0.0.1:8080");
        assert_eq!(origin, "/v1/git/repos");
        let (_, origin_q) =
            target(&cfg("http://h:1/"), &get("/v1/git/search/code", Some("q=x"))).unwrap();
        assert_eq!(origin_q, "/v1/git/search/code?q=x");
        // https is a clean transport error, not a silent downgrade.
        assert!(target(&cfg("https://h:1"), &get("/v1/git/repos", None)).is_err());
    }

    #[test]
    fn interpret_maps_status_to_typed_errors() {
        // 2xx → the body.
        let ok = interpret(200, json!({"items":[]})).unwrap();
        assert_eq!(ok["items"], json!([]));
        // 401 → Unauthorized (exit 3), carrying the envelope message (no token in it).
        let e = interpret(401, json!({"error":{"message":"authentication required","code":"unauthorized"}}))
            .unwrap_err();
        assert_eq!(e.code(), 3);
        // a 404 → Edge with the parsed code/message (exit 1).
        let e = interpret(404, json!({"error":{"message":"no such pull request","code":"not_found"}}))
            .unwrap_err();
        match e {
            CliError::Edge { status, code, message } => {
                assert_eq!(status, 404);
                assert_eq!(code, "not_found");
                assert_eq!(message, "no such pull request");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
        // a non-2xx with no envelope does not panic.
        assert!(interpret(500, Value::Null).is_err());
    }
}
