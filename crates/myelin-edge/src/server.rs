//! # The real hyper listener — the thin transport adapter over [`Gateway`]
//!
//! This is the REAL HTTP listener the prompt calls for: it binds a TCP listener and, per connection,
//! converts `hyper::Request` → [`EdgeRequest`], runs the gateway lifecycle, and converts the
//! [`EdgeResponse`] back to a `hyper::Response` — rendering a finished JSON/error body OR streaming an
//! SSE response. hyper 1.x is used directly (no axum); every type here is already in `Cargo.lock`.
//!
//! The adapter is deliberately thin: ALL the policy (auth, tenant-from-token, IDOR reject,
//! authorization, error envelope, versioning, pagination, SSE scoping) lives in [`Gateway`], so the
//! security properties hold identically whether driven over this socket or in-process.

use crate::error::EdgeError;
use crate::gateway::Gateway;
use crate::request::{EdgeRequest, EdgeResponse};
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Body, Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// The response body type the adapter emits — a boxed body of [`Bytes`] frames (finished or
/// streamed), with an `io::Error` error channel.
type EdgeBody = BoxBody<Bytes, std::io::Error>;

/// **The front-door request-body size ceiling (R0.5 / DELTA N3).**
///
/// `body.collect().await` on a hyper `Incoming` buffers the ENTIRE body into host RAM with no size
/// limit — the comment that it was "bounded by hyper's defaults" was false, so a single large POST
/// (e.g. to the `git-receive-pack` wire route, or ANY route) is a trivial memory-exhaustion DoS. The
/// sandbox's 64 MiB stdin bound in `myelin-ci-sandbox` only applies AFTER this collection, so it
/// never protects the edge process. We therefore bound the body AS IT STREAMS and reject oversize
/// with a `413 Payload Too Large` without buffering past the cap.
///
/// **Tradeoff / choice of 100 MiB:** git pushes ship packfiles that can be large, so the ceiling must
/// be generous enough for a legitimate `git-receive-pack` — but finite, so the front door has a hard
/// DoS ceiling. 100 MiB is that compromise: comfortably above ordinary pushes, far below "exhaust the
/// host". Only `git-receive-pack` receives that budget; every other route has a 1 MiB ceiling.
const MAX_REQUEST_BODY_BYTES: usize = 100 * 1024 * 1024;
const MAX_JSON_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 1024;
const MAX_CONCURRENT_GIT_PUSHES: usize = 8;
const MAX_REQUEST_HEADERS: usize = 64;
const MAX_HTTP_BUFFER_BYTES: usize = 64 * 1024;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Result of a requested listener shutdown. A forced result means the grace deadline expired and
/// the remaining connection tasks were aborted; the count is the number active when draining began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Graceful { connections: usize },
    Forced { connections: usize },
}

/// Boxed asynchronous readiness check. The probe reports only a verdict; transport responses never
/// expose connection strings, filesystem paths, or dependency error details.
pub type ReadinessCheck<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Critical-dependency readiness seam used by the unauthenticated orchestration probe.
pub trait ReadinessProbe: Send + Sync {
    fn check(&self) -> ReadinessCheck<'_>;
}

#[derive(Debug)]
struct AlwaysReady;

impl ReadinessProbe for AlwaysReady {
    fn check(&self) -> ReadinessCheck<'_> {
        Box::pin(std::future::ready(true))
    }
}

/// Why a bounded collect stopped short of returning the full body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundedCollectError {
    /// The accumulated data frames exceeded [`MAX_REQUEST_BODY_BYTES`]; reading stopped at the cap
    /// (the oversize buffer is never allocated) — maps to a `413` at the edge.
    TooLarge,
    /// The transport yielded a read error mid-body — the caller falls back to an empty body (the
    /// gateway then produces a clean `400` if a body was required), preserving prior behavior.
    Read,
}

/// **Collect a request body frame-by-frame, bounded by `cap` (R0.5 / DELTA N3).**
///
/// Iterates the body one [`Frame`] at a time via [`BodyExt::frame`], accumulating ONLY data frames
/// (trailers do not count toward the body) into a `Vec`. The moment the running total would exceed
/// `cap`, it STOPS reading and returns [`BoundedCollectError::TooLarge`] — it does NOT keep buffering,
/// so an oversize body never grows host memory past ~`cap`. A transport read error returns
/// [`BoundedCollectError::Read`]. Generic over any `Body<Data = Bytes>` so it is unit-testable with an
/// in-memory body (no socket needed).
async fn collect_bounded<B>(mut body: B, cap: usize) -> Result<Vec<u8>, BoundedCollectError>
where
    B: Body<Data = Bytes> + Unpin,
{
    let mut acc: Vec<u8> = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BoundedCollectError::Read)?;
        // Only DATA frames count toward the body; a trailers frame is skipped.
        if let Ok(data) = frame.into_data() {
            // Reject BEFORE extending past the cap — never allocate the full oversize buffer.
            if acc.len() + data.len() > cap {
                return Err(BoundedCollectError::TooLarge);
            }
            acc.extend_from_slice(&data);
        }
    }
    Ok(acc)
}

/// **Serve the edge over a bound TCP listener.** Accepts connections forever, serving each over
/// HTTP/1.1 with the gateway. Returns only on an accept error.
pub async fn serve_edge(listener: TcpListener, gateway: Arc<Gateway>) -> std::io::Result<()> {
    serve_edge_until_shutdown(
        listener,
        gateway,
        std::future::pending::<()>(),
        Duration::from_secs(20),
    )
    .await?;
    Ok(())
}

/// Serve until `shutdown` resolves, then close the listener, gracefully finish active HTTP/1
/// connections, and abort only those still open after `grace`. The shutdown future's output is
/// returned to the owner so signal-handler failures remain distinguishable from transport failures.
pub async fn serve_edge_until_shutdown<F, T>(
    listener: TcpListener,
    gateway: Arc<Gateway>,
    shutdown: F,
    grace: Duration,
) -> std::io::Result<(ShutdownOutcome, T)>
where
    F: Future<Output = T>,
{
    serve_edge_until_shutdown_with_probe(
        listener,
        gateway,
        Arc::new(AlwaysReady),
        shutdown,
        grace,
    )
    .await
}

/// Production variant of [`serve_edge_until_shutdown`] with an explicit dependency-readiness probe.
pub async fn serve_edge_until_shutdown_with_probe<F, T>(
    listener: TcpListener,
    gateway: Arc<Gateway>,
    readiness: Arc<dyn ReadinessProbe>,
    shutdown: F,
    grace: Duration,
) -> std::io::Result<(ShutdownOutcome, T)>
where
    F: Future<Output = T>,
{
    tokio::pin!(shutdown);
    let graceful = GracefulShutdown::new();
    let mut connections = JoinSet::new();
    let connection_slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let git_push_slots = Arc::new(Semaphore::new(MAX_CONCURRENT_GIT_PUSHES));
    let mut accept_error = None;

    let shutdown_output = loop {
        tokio::select! {
            biased;
            output = &mut shutdown => break Some(output),
            accepted = listener.accept() => {
                let (stream, _peer) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        accept_error = Some(error);
                        break None;
                    }
                };
                let Ok(connection_permit) = connection_slots.clone().try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let io = TokioIo::new(stream);
                let gw = gateway.clone();
                let readiness = readiness.clone();
                let git_push_slots = git_push_slots.clone();
                let watcher = graceful.watcher();
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    let service = service_fn(move |req: Request<Incoming>| {
                        let gw = gw.clone();
                        let readiness = readiness.clone();
                        let git_push_slots = git_push_slots.clone();
                        async move {
                            Ok::<_, Infallible>(
                                handle_connection(gw, readiness, git_push_slots, req).await,
                            )
                        }
                    });
                    let mut http = hyper::server::conn::http1::Builder::new();
                    http.timer(TokioTimer::new())
                        .header_read_timeout(HEADER_READ_TIMEOUT)
                        .max_headers(MAX_REQUEST_HEADERS)
                        .max_buf_size(MAX_HTTP_BUFFER_BYTES);
                    let connection = http.serve_connection(io, service);
                    let _ = watcher.watch(connection).await;
                });
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                let _ = completed;
            }
        }
    };

    // Dropping the listener happens here, before any drain wait, so no new socket can enter.
    drop(listener);
    let active = graceful.count();
    let outcome = if tokio::time::timeout(grace, graceful.shutdown())
        .await
        .is_ok()
    {
        ShutdownOutcome::Graceful {
            connections: active,
        }
    } else {
        connections.abort_all();
        ShutdownOutcome::Forced {
            connections: active,
        }
    };
    while connections.join_next().await.is_some() {}

    if let Some(error) = accept_error {
        return Err(error);
    }
    Ok((
        outcome,
        shutdown_output.expect("shutdown output exists when the accept loop did not fail"),
    ))
}

/// Convert one hyper request → [`EdgeRequest`], run the gateway, convert the response back.
async fn handle_connection(
    gw: Arc<Gateway>,
    readiness: Arc<dyn ReadinessProbe>,
    git_push_slots: Arc<Semaphore>,
    req: Request<Incoming>,
) -> Response<EdgeBody> {
    let (parts, body) = req.into_parts();
    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();
    if matches!(path.as_str(), "/livez" | "/readyz") {
        let status = match method.as_str() {
            "GET" | "HEAD" if path == "/livez" => 200,
            "GET" | "HEAD" if readiness.check().await => 200,
            "GET" | "HEAD" => 503,
            _ => 405,
        };
        let body = if method == "HEAD" {
            Vec::new()
        } else if status == 200 {
            br#"{"status":"ok"}"#.to_vec()
        } else if status == 503 {
            br#"{"status":"not_ready"}"#.to_vec()
        } else {
            br#"{"status":"method_not_allowed"}"#.to_vec()
        };
        return probe_response(status, body);
    }
    let body_cap = request_body_cap(&path);
    let _git_push_permit = if body_cap == MAX_REQUEST_BODY_BYTES {
        match git_push_slots.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return overloaded(),
        }
    } else {
        None
    };
    let query = parts.uri.query().unwrap_or("").to_string();
    let headers = parts
        .headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Front-door body bound (R0.5 / DELTA N3): fast-path reject on a Content-Length header that
    // already declares more than the cap — refuse before reading a single body byte.
    if content_length_over_cap(&parts.headers, body_cap) {
        return payload_too_large(body_cap);
    }
    // Collect the body bounded by MAX_REQUEST_BODY_BYTES, streaming frame-by-frame. Oversize → a 413
    // WITHOUT buffering past the cap; a read error → empty body (the gateway then produces a clean 400
    // if a body was required) — never a panic.
    let bytes = match collect_bounded(body, body_cap).await {
        Ok(b) => b,
        Err(BoundedCollectError::TooLarge) => return payload_too_large(body_cap),
        Err(BoundedCollectError::Read) => Vec::new(),
    };
    let edge_req = EdgeRequest::new(method, path, query, headers, bytes);
    to_hyper(gw.handle(edge_req))
}

fn request_body_cap(path: &str) -> usize {
    if path.ends_with("/git-receive-pack") {
        MAX_REQUEST_BODY_BYTES
    } else {
        MAX_JSON_REQUEST_BODY_BYTES
    }
}

fn overloaded() -> Response<EdgeBody> {
    let err = EdgeError::Unavailable("the Git push intake is at capacity; retry later".into());
    to_hyper(EdgeResponse::error(&err))
}

fn probe_response(status: u16, body: Vec<u8>) -> Response<EdgeBody> {
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store");
    if status == 405 {
        response = response.header("allow", "GET, HEAD");
    }
    response
        .body(full_body(body))
        .unwrap_or_else(|_| Response::new(full_body(b"{}".to_vec())))
}

/// True iff a `Content-Length` header is present, parseable, and DECLARES more than `cap` bytes — the
/// front-door fast-reject signal (R0.5 / DELTA N3). An absent/unparseable header is NOT over-cap here
/// (the streaming bound in [`collect_bounded`] is the real enforcement; this is only the cheap
/// pre-read shortcut for a client that honestly declares an oversize body).
fn content_length_over_cap(headers: &hyper::HeaderMap, cap: usize) -> bool {
    headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_some_and(|declared| declared > cap as u64)
}

/// Render the `413 Payload Too Large` response through the existing error-envelope path, so its
/// `{error:{message,code}}` shape matches every other edge error (R0.5 / DELTA N3).
fn payload_too_large(cap: usize) -> Response<EdgeBody> {
    let err = EdgeError::PayloadTooLarge(format!(
        "request body exceeds the {cap}-byte route limit"
    ));
    to_hyper(EdgeResponse::error(&err))
}

/// Render an [`EdgeResponse`] as a hyper response (finished body or streamed SSE).
fn to_hyper(resp: EdgeResponse) -> Response<EdgeBody> {
    match resp {
        EdgeResponse::Bytes { status, content_type, headers, body } => {
            let mut builder = Response::builder()
                .status(status)
                .header("content-type", content_type);
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
            builder
                .body(full_body(body))
                .unwrap_or_else(|_| Response::new(full_body(b"{}".to_vec())))
        }
        EdgeResponse::Sse { headers, sub } => {
            let mut builder = Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive");
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
            builder
                .body(sse_body(sub.into_receiver()))
                .unwrap_or_else(|_| Response::new(full_body(b"{}".to_vec())))
        }
    }
}

/// A finished body of `Bytes` (the JSON view-model / error envelope).
fn full_body(bytes: Vec<u8>) -> EdgeBody {
    Full::new(Bytes::from(bytes))
        .map_err(|never: Infallible| match never {})
        .boxed()
}

/// A streaming SSE body fed by the subscription's broadcast receiver: each frame is written as one
/// SSE event; a lagged-receiver error is skipped (the bounded-and-sheds posture).
fn sse_body(rx: tokio::sync::broadcast::Receiver<crate::sse::SseEvent>) -> EdgeBody {
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(ev) => Some(Ok::<Frame<Bytes>, std::io::Error>(Frame::data(Bytes::from(ev.frame())))),
        // A lagged/closed receiver yields an error frame we skip (the connection is dropped to
        // resync on the real firehose seam; here we simply stop emitting on close).
        Err(_) => None,
    });
    StreamBody::new(stream).boxed()
}

#[cfg(test)]
mod tests {
    //! R0.5 / DELTA N3 — the front-door body bound. These prove: (a) an under-cap body collects fully
    //! and correctly; (b) an over-cap body returns `TooLarge` and stops accumulating NEAR the cap (the
    //! full oversize buffer is never allocated); (c) a Content-Length header over the cap fast-rejects.
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// (a) A body under the cap collects fully and byte-for-byte.
    #[tokio::test]
    async fn under_cap_collects_full_body() {
        let payload = b"the quick brown fox jumps over the lazy dog".to_vec();
        let body = Full::new(Bytes::from(payload.clone()));
        let out = collect_bounded(body, MAX_REQUEST_BODY_BYTES).await.expect("under cap");
        assert_eq!(out, payload, "an under-cap body is returned exactly");
    }

    /// A body exactly AT the cap is accepted (the boundary is inclusive).
    #[tokio::test]
    async fn exactly_at_cap_is_accepted() {
        let cap = 4096;
        let body = Full::new(Bytes::from(vec![7u8; cap]));
        let out = collect_bounded(body, cap).await.expect("at cap is accepted");
        assert_eq!(out.len(), cap);
    }

    /// (b) A body over the cap returns `TooLarge` AND stops reading near the cap — it does NOT pull the
    /// whole oversize input. We count bytes actually pulled from the stream; only consumed frames
    /// increment it, so the counter proves accumulation halted a frame past the cap, not at input end.
    #[tokio::test]
    async fn over_cap_rejects_without_buffering_full_input() {
        let cap = 4096usize;
        let chunk = 1024usize;
        let total_chunks = 4096usize; // 4 MiB of input — 1000x the cap.
        let pulled = Arc::new(AtomicUsize::new(0));
        let counter = pulled.clone();
        let chunks: Vec<Bytes> = (0..total_chunks).map(|_| Bytes::from(vec![0u8; chunk])).collect();
        let stream = tokio_stream::iter(chunks).map(move |b| {
            counter.fetch_add(b.len(), Ordering::SeqCst);
            Ok::<Frame<Bytes>, std::io::Error>(Frame::data(b))
        });
        let body = StreamBody::new(stream);

        let res = collect_bounded(body, cap).await;
        assert_eq!(res, Err(BoundedCollectError::TooLarge), "over-cap body is rejected");

        let consumed = pulled.load(Ordering::SeqCst);
        assert!(
            consumed <= cap + chunk,
            "reading stopped near the cap (consumed {consumed}), not the full {} input bytes",
            total_chunks * chunk
        );
    }

    /// Trailers frames do not count toward the body: a data frame under the cap followed by trailers
    /// still collects successfully and yields only the data bytes.
    #[tokio::test]
    async fn trailers_frame_is_not_counted_as_body() {
        let mut trailers = hyper::HeaderMap::new();
        trailers.insert("x-checksum", hyper::header::HeaderValue::from_static("abc"));
        let frames: Vec<Result<Frame<Bytes>, std::io::Error>> = vec![
            Ok(Frame::data(Bytes::from_static(b"hello"))),
            Ok(Frame::trailers(trailers)),
        ];
        let body = StreamBody::new(tokio_stream::iter(frames));
        let out = collect_bounded(body, 1024).await.expect("data under cap");
        assert_eq!(out, b"hello", "only the data frame contributes to the body");
    }

    /// A transport read error surfaces as `Read` (the caller falls back to an empty body → clean 400).
    #[tokio::test]
    async fn read_error_surfaces_as_read_not_too_large() {
        let frames: Vec<Result<Frame<Bytes>, std::io::Error>> = vec![
            Ok(Frame::data(Bytes::from_static(b"partial"))),
            Err(std::io::Error::other("connection reset")),
        ];
        let body = StreamBody::new(tokio_stream::iter(frames));
        let res = collect_bounded(body, 1024).await;
        assert_eq!(res, Err(BoundedCollectError::Read), "a mid-body read error is Read, not TooLarge");
    }

    /// (c) A Content-Length header declaring more than the cap fast-rejects; at/under the cap, or
    /// absent/unparseable, does not.
    #[test]
    fn content_length_over_cap_fast_rejects() {
        let cap = 4096usize;
        let mk = |val: &str| {
            let mut h = hyper::HeaderMap::new();
            h.insert(hyper::header::CONTENT_LENGTH, hyper::header::HeaderValue::from_str(val).unwrap());
            h
        };
        assert!(content_length_over_cap(&mk("4097"), cap), "declared > cap rejects");
        assert!(!content_length_over_cap(&mk("4096"), cap), "declared == cap does not reject");
        assert!(!content_length_over_cap(&mk("10"), cap), "declared < cap does not reject");
        assert!(!content_length_over_cap(&mk("not-a-number"), cap), "unparseable does not fast-reject");
        assert!(
            !content_length_over_cap(&hyper::HeaderMap::new(), cap),
            "absent Content-Length does not fast-reject (streaming bound enforces)"
        );
    }

    /// The 413 response carries the canonical `{error:{message,code}}` envelope with code
    /// `payload_too_large` and HTTP status 413.
    #[test]
    fn payload_too_large_uses_the_canonical_413_envelope() {
        let resp = payload_too_large(MAX_REQUEST_BODY_BYTES);
        assert_eq!(resp.status(), 413);
        let err = EdgeError::PayloadTooLarge("x".into());
        assert_eq!(err.status(), 413);
        assert_eq!(err.code(), "payload_too_large");
    }

    #[test]
    fn only_git_receive_pack_gets_the_large_body_budget() {
        assert_eq!(
            request_body_cap("/acme/eu-west/widgets.git/git-receive-pack"),
            MAX_REQUEST_BODY_BYTES
        );
        for path in [
            "/v1/issues",
            "/v1/git/repos/widgets",
            "/acme/eu-west/widgets.git/git-upload-pack",
            "/livez",
        ] {
            assert_eq!(request_body_cap(path), MAX_JSON_REQUEST_BODY_BYTES, "{path}");
        }
    }
}
