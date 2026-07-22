//! # The edge HTTPS client — present the Bearer capability token, parse the envelopes.
//!
//! The CLI is an edge CLIENT: it calls the MR-014 gateway's `/v1/...` routes over HTTP/1.1 (hyper
//! 1.x directly — the SAME minimal transport stack the edge uses; no reqwest), presenting the
//! capability token as `Authorization: Bearer <token>` + the `x-myelin-token-scheme` the gateway
//! reads. It parses the uniform `{items,page}` list envelope and the `{error:{message,code}}` error
//! envelope, and maps a `401` to a clean [`CliError::Unauthorized`] (NOT a panic).
//!
//! Remote endpoints require certificate-verified HTTPS. Plain HTTP is allowed only for loopback
//! development (`localhost`, `127.0.0.0/8`, or `::1`), so a configuration typo cannot send a bearer
//! capability over a clear-text network. The token is NEVER logged: it is placed only in the
//! `Authorization` header; no error path interpolates it, and the header is not printed.

use crate::config::EdgeConfig;
use crate::dispatch::EdgeCall;
use crate::error::CliError;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Request, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::time::Duration;

const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
struct Target {
    uri: Uri,
    authority: String,
}

/// Parse the base URL + call into an absolute request target. HTTPS is the production path. HTTP is
/// intentionally limited to loopback so bearer material never crosses a clear-text network.
fn target(config: &EdgeConfig, call: &EdgeCall) -> Result<Target, CliError> {
    let base = config.url.trim_end_matches('/');
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err(CliError::Transport(
            "edge URL must start with https:// (or loopback http:// for development)".into(),
        ));
    }
    if base.contains('#') {
        return Err(CliError::Transport(
            "edge URL must not contain a fragment".into(),
        ));
    }
    let base_uri: Uri = base
        .parse()
        .map_err(|e| CliError::Transport(format!("invalid edge URL: {e}")))?;
    let base_authority = base_uri
        .authority()
        .ok_or_else(|| CliError::Transport("edge URL has no host".into()))?;
    if base_authority.as_str().contains('@') {
        return Err(CliError::Transport(
            "edge URL must not contain user information".into(),
        ));
    }
    if base_uri.path_and_query().is_some_and(|value| value.query().is_some()) {
        return Err(CliError::Transport(
            "edge URL must not contain a query string".into(),
        ));
    }

    let mut absolute = format!("{base}{}", call.path);
    if let Some(q) = &call.query {
        absolute.push('?');
        absolute.push_str(q);
    }
    let uri: Uri = absolute
        .parse()
        .map_err(|e| CliError::Transport(format!("invalid edge URL: {e}")))?;
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| CliError::Transport("edge URL has no scheme".into()))?;
    let authority = uri
        .authority()
        .ok_or_else(|| CliError::Transport("edge URL has no host".into()))?;
    let host = authority.host();
    if scheme == "http" && !is_loopback_host(host) {
        return Err(CliError::Transport(format!(
            "refusing clear-text bearer transport to non-loopback host `{host}`; use https://"
        )));
    }
    let authority = authority.as_str().to_string();
    Ok(Target { uri, authority })
}

fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// **Run an [`EdgeCall`] against the edge** with the Bearer capability token, returning the parsed
/// JSON body on success. Total: a connect/transport failure is [`CliError::Transport`]; a `401` is
/// [`CliError::Unauthorized`]; any other non-2xx with the `{error:{message}}` envelope is
/// [`CliError::Edge`]. No panic, ever.
pub async fn execute(config: &EdgeConfig, token: &str, call: &EdgeCall) -> Result<Value, CliError> {
    execute_with_limits(
        config,
        token,
        call,
        REQUEST_DEADLINE,
        MAX_RESPONSE_BYTES,
    )
    .await
}

async fn execute_with_limits(
    config: &EdgeConfig,
    token: &str,
    call: &EdgeCall,
    deadline: Duration,
    max_response_bytes: usize,
) -> Result<Value, CliError> {
    let target = target(config, call)?;
    let connector = HttpsConnectorBuilder::new()
        // The workspace also carries ring through unrelated consumers. Select this client's
        // provider explicitly so rustls never has to guess a process-global default from unified
        // Cargo features (which would panic before either an HTTP or HTTPS request is sent).
        .with_provider_and_native_roots(rustls::crypto::aws_lc_rs::default_provider())
        .map_err(|e| CliError::Transport(format!("load native TLS trust roots: {e}")))?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let mut builder = Request::builder()
        .method(call.method.as_str())
        .uri(&target.uri)
        .header("host", &target.authority)
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

    let (status, bytes) = tokio::time::timeout(deadline, async {
        let response = client
            .request(request)
            .await
            .map_err(|e| CliError::Transport(format!("edge request failed: {e}")))?;
        let status = response.status().as_u16();
        let bytes = Limited::new(response.into_body(), max_response_bytes)
            .collect()
            .await
            .map_err(|_| {
                CliError::Transport(format!(
                    "edge response exceeded the {max_response_bytes}-byte limit or could not be read"
                ))
            })?
            .to_bytes();
        Ok::<_, CliError>((status, bytes))
    })
    .await
    .map_err(|_| {
        CliError::Transport(format!(
            "edge request exceeded the {}-second deadline",
            deadline.as_secs()
        ))
    })??;
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
    Err(CliError::Edge {
        status,
        code,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{EdgeCall, HttpMethod};
    use serde_json::json;

    fn cfg(url: &str) -> EdgeConfig {
        EdgeConfig {
            url: url.into(),
            scheme: "agent".into(),
        }
    }
    fn get(path: &str, query: Option<&str>) -> EdgeCall {
        EdgeCall {
            method: HttpMethod::Get,
            path: path.into(),
            query: query.map(str::to_string),
            payload: None,
        }
    }

    #[test]
    fn target_admits_https_and_only_loopback_http() {
        let local = target(&cfg("http://127.0.0.1:8080"), &get("/v1/git/repos", None)).unwrap();
        assert_eq!(local.authority, "127.0.0.1:8080");
        assert_eq!(local.uri.to_string(), "http://127.0.0.1:8080/v1/git/repos");

        let secure = target(
            &cfg("https://edge.example.com/platform"),
            &get("/v1/git/search/code", Some("q=x")),
        )
        .unwrap();
        assert_eq!(secure.authority, "edge.example.com");
        assert_eq!(
            secure.uri.to_string(),
            "https://edge.example.com/platform/v1/git/search/code?q=x"
        );

        for local in [
            "http://localhost:8080",
            "http://127.42.0.1:8080",
            "http://[::1]:8080",
        ] {
            assert!(target(&cfg(local), &get("/v1/whoami", None)).is_ok());
        }
        assert!(target(&cfg("http://edge.example.com"), &get("/v1/whoami", None)).is_err());
        assert!(target(&cfg("ftp://edge.example.com"), &get("/v1/whoami", None)).is_err());
        assert!(target(&cfg("https://user@edge.example.com"), &get("/v1/whoami", None)).is_err());
        assert!(target(&cfg("https://edge.example.com?route=other"), &get("/v1/whoami", None)).is_err());
        assert!(target(&cfg("https://edge.example.com/#fragment"), &get("/v1/whoami", None)).is_err());
    }

    #[tokio::test]
    async fn response_body_is_bounded_before_json_parsing() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"items\":[1,2]}")
                .await
                .unwrap();
        });

        let error = execute_with_limits(
            &cfg(&format!("http://{address}")),
            "sensitive-token",
            &get("/v1/whoami", None),
            Duration::from_secs(1),
            8,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("8-byte limit"));
        assert!(!error.to_string().contains("sensitive-token"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn one_deadline_bounds_connect_headers_and_body() {
        use tokio::io::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            std::future::pending::<()>().await;
        });

        let error = execute_with_limits(
            &cfg(&format!("http://{address}")),
            "sensitive-token",
            &get("/v1/whoami", None),
            Duration::from_millis(20),
            1024,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
        assert!(!error.to_string().contains("sensitive-token"));
        server.abort();
    }

    #[test]
    fn interpret_maps_status_to_typed_errors() {
        // 2xx → the body.
        let ok = interpret(200, json!({"items":[]})).unwrap();
        assert_eq!(ok["items"], json!([]));
        // 401 → Unauthorized (exit 3), carrying the envelope message (no token in it).
        let e = interpret(
            401,
            json!({"error":{"message":"authentication required","code":"unauthorized"}}),
        )
        .unwrap_err();
        assert_eq!(e.code(), 3);
        // a 404 → Edge with the parsed code/message (exit 1).
        let e = interpret(
            404,
            json!({"error":{"message":"no such pull request","code":"not_found"}}),
        )
        .unwrap_err();
        match e {
            CliError::Edge {
                status,
                code,
                message,
            } => {
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
