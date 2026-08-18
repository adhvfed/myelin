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
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_stream::wrappers::BroadcastStream;

type EdgeBody = BoxBody<Bytes, std::io::Error>;

const MAX_REQUEST_BODY_BYTES: usize = myelin_ci_sandbox::gvisor::WIRE_STDIN_BOUND;
const MAX_JSON_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 1024;
const MAX_CONCURRENT_GIT_WIRE_OPERATIONS: usize = 4;
const MAX_CONCURRENT_REQUEST_BODIES: usize = 64;
const MAX_CONCURRENT_GIT_PUSH_BODIES: usize = 2;
const MAX_CONCURRENT_GATEWAY_DISPATCHES: usize = 64;
// machine-classified requests (CI, agents, deploy keys, or anything carrying
// x-myelin-run-class) draw from this smaller pool FIRST, so the remaining
// dispatch slots stay available to interactive humans during a machine storm.
// a caller that lies about its class gets past this pool but is then held to
// its verified principal class by the per-tenant shed lane in the gateway.
const MAX_CONCURRENT_MACHINE_DISPATCHES: usize = 48;
const MAX_CONCURRENT_LARGE_RESPONSES: usize = 2;
const MAX_RESPONSE_FRAME_BYTES: usize = 64 * 1024;
const MAX_REQUEST_HEADERS: usize = 64;
const MAX_HTTP_BUFFER_BYTES: usize = 64 * 1024;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const API_BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_PUSH_BODY_READ_TIMEOUT: Duration = Duration::from_secs(300);
const GIT_WIRE_RETRY_AFTER_SECONDS: &str = "1";
const READINESS_RETRY_AFTER_SECONDS: &str = "5";
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CONNECTIONS_SHED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct AdmissionSlots {
    git_wire: Arc<Semaphore>,
    request_body: Arc<Semaphore>,
    git_push_body: Arc<Semaphore>,
    gateway_dispatch: Arc<Semaphore>,
    machine_dispatch: Arc<Semaphore>,
    large_response: Arc<Semaphore>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Graceful { connections: usize },
    Forced { connections: usize },
}

pub type ReadinessCheck<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundedCollectError {
    TooLarge,
    Read,
    TimedOut,
}

async fn collect_bounded<B>(
    mut body: B,
    cap: usize,
    deadline: Duration,
) -> Result<Vec<u8>, BoundedCollectError>
where
    B: Body<Data = Bytes> + Unpin,
{
    tokio::time::timeout(deadline, async {
        let mut acc: Vec<u8> = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| BoundedCollectError::Read)?;
            if let Ok(data) = frame.into_data() {
                if acc.len() + data.len() > cap {
                    return Err(BoundedCollectError::TooLarge);
                }
                acc.extend_from_slice(&data);
            }
        }
        Ok(acc)
    })
    .await
    .map_err(|_| BoundedCollectError::TimedOut)?
}

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

pub async fn serve_edge_until_shutdown<F, T>(
    listener: TcpListener,
    gateway: Arc<Gateway>,
    shutdown: F,
    grace: Duration,
) -> std::io::Result<(ShutdownOutcome, T)>
where
    F: Future<Output = T>,
{
    serve_edge_until_shutdown_with_probe(listener, gateway, Arc::new(AlwaysReady), shutdown, grace)
        .await
}

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
    let admission = AdmissionSlots {
        git_wire: Arc::new(Semaphore::new(MAX_CONCURRENT_GIT_WIRE_OPERATIONS)),
        request_body: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUEST_BODIES)),
        git_push_body: Arc::new(Semaphore::new(MAX_CONCURRENT_GIT_PUSH_BODIES)),
        gateway_dispatch: Arc::new(Semaphore::new(MAX_CONCURRENT_GATEWAY_DISPATCHES)),
        machine_dispatch: Arc::new(Semaphore::new(MAX_CONCURRENT_MACHINE_DISPATCHES)),
        large_response: Arc::new(Semaphore::new(MAX_CONCURRENT_LARGE_RESPONSES)),
    };
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
                    log_connection_shed();
                    drop(stream);
                    continue;
                };
                let io = TokioIo::new(stream);
                let gw = gateway.clone();
                let readiness = readiness.clone();
                let admission = admission.clone();
                let watcher = graceful.watcher();
                connections.spawn(async move {
                    let _connection_permit = connection_permit;
                    let service = service_fn(move |req: Request<Incoming>| {
                        let gw = gw.clone();
                        let readiness = readiness.clone();
                        let admission = admission.clone();
                        async move {
                            Ok::<_, Infallible>(
                                handle_connection(
                                    gw,
                                    readiness,
                                    admission,
                                    req,
                                )
                                .await,
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

async fn handle_connection(
    gw: Arc<Gateway>,
    readiness: Arc<dyn ReadinessProbe>,
    admission: AdmissionSlots,
    req: Request<Incoming>,
) -> Response<EdgeBody> {
    let request_id = next_request_id();
    let method = observable_method(req.method());
    let route_class = route_class(req.uri().path());
    let started = std::time::Instant::now();
    let mut response = handle_connection_inner(gw, readiness, admission, req).await;
    harden_response_headers(&mut response);
    response.headers_mut().insert(
        hyper::header::HeaderName::from_static("x-request-id"),
        hyper::header::HeaderValue::from_str(&request_id).expect("generated request id is ASCII"),
    );
    eprintln!(
        "{}",
        access_log_record(
            &request_id,
            method,
            route_class,
            response.status().as_u16(),
            started.elapsed(),
        )
    );
    response
}

fn harden_response_headers(response: &mut Response<EdgeBody>) {
    let headers = response.headers_mut();
    if !headers.contains_key(hyper::header::CACHE_CONTROL) {
        headers.insert(
            hyper::header::CACHE_CONTROL,
            hyper::header::HeaderValue::from_static("no-store"),
        );
    }
    headers.insert(
        hyper::header::HeaderName::from_static("x-content-type-options"),
        hyper::header::HeaderValue::from_static("nosniff"),
    );
}

async fn handle_connection_inner(
    gw: Arc<Gateway>,
    readiness: Arc<dyn ReadinessProbe>,
    admission: AdmissionSlots,
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
    let is_git_wire = is_git_wire_path(&path);
    let is_git_push = path.ends_with("/git-receive-pack");
    let body_cap = request_body_cap(&path);
    let body_deadline = request_body_deadline(&path);
    let query = parts.uri.query().unwrap_or("").to_string();
    let headers = parts
        .headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    if content_length_over_cap(&parts.headers, body_cap) {
        return payload_too_large(body_cap);
    }
    let request_body_permit = if request_has_body(&parts.headers) {
        match admission.request_body.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return unread_body_overloaded(
                    "the edge request-body service is at capacity; retry later",
                )
            }
        }
    } else {
        None
    };
    let git_push_body_permit = if is_git_push {
        match admission.git_push_body.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => {
                return unread_body_overloaded(
                    "the Git push upload service is at capacity; retry later",
                )
            }
        }
    } else {
        None
    };
    let bytes = match collect_bounded(body, body_cap, body_deadline).await {
        Ok(b) => b,
        Err(BoundedCollectError::TooLarge) => return payload_too_large(body_cap),
        Err(BoundedCollectError::Read) => return request_body_read_error(),
        Err(BoundedCollectError::TimedOut) => return request_timeout(body_deadline),
    };
    let edge_req = EdgeRequest::new(method, path, query, headers, bytes);
    let large_response_permit = if requires_large_response_budget(&edge_req.method, &edge_req.path)
    {
        match admission.large_response.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return overloaded("the large response service is at capacity; retry later"),
        }
    } else {
        None
    };
    let git_wire_permit = if is_git_wire {
        match admission.git_wire.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return overloaded("the Git wire service is at capacity; retry later"),
        }
    } else {
        None
    };
    let machine_permit = if is_machine_classified(&edge_req) {
        match admission.machine_dispatch.clone().try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return machine_dispatch_shed(),
        }
    } else {
        None
    };
    let gateway_permit = match admission.gateway_dispatch.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return overloaded("the edge request service is at capacity; retry later"),
    };

    let (edge_response, large_response_permit) = match tokio::task::spawn_blocking(move || {
        let _machine_permit = machine_permit;
        let _gateway_permit = gateway_permit;
        let _git_wire_permit = git_wire_permit;
        let _request_body_permit = request_body_permit;
        let _git_push_body_permit = git_push_body_permit;
        (handle_gateway_safely(&gw, edge_req), large_response_permit)
    })
    .await
    {
        Ok(response) => response,
        Err(_) => (
            EdgeResponse::error(&EdgeError::Internal(
                "gateway dispatch task did not complete".into(),
            )),
            None,
        ),
    };
    to_hyper_with_permit(edge_response, large_response_permit)
}

fn next_request_id() -> String {
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    format!("{:08x}{nanos:016x}{sequence:016x}", std::process::id())
}

fn observable_method(method: &hyper::Method) -> &'static str {
    match *method {
        hyper::Method::GET => "GET",
        hyper::Method::HEAD => "HEAD",
        hyper::Method::POST => "POST",
        hyper::Method::PUT => "PUT",
        hyper::Method::PATCH => "PATCH",
        hyper::Method::DELETE => "DELETE",
        hyper::Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

fn route_class(path: &str) -> &'static str {
    if matches!(path, "/livez" | "/readyz") {
        "health"
    } else if is_git_wire_path(path) {
        "git_wire"
    } else if path.starts_with("/v1/auth/") {
        "auth"
    } else if path.starts_with("/v1/") {
        "api"
    } else {
        "unknown"
    }
}

fn access_log_record(
    request_id: &str,
    method: &str,
    route_class: &str,
    status: u16,
    elapsed: Duration,
) -> serde_json::Value {
    serde_json::json!({
        "event": "edge.http.request",
        "request_id": request_id,
        "method": method,
        "route_class": route_class,
        "status": status,
        "duration_us": elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
    })
}

fn log_connection_shed() {
    let count = CONNECTIONS_SHED.fetch_add(1, Ordering::Relaxed) + 1;
    if count.is_power_of_two() {
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "edge.connection.shed",
                "shed_total": count,
                "reason": "connection_limit",
            })
        );
    }
}

fn handle_gateway_safely(gw: &Gateway, request: EdgeRequest) -> EdgeResponse {
    catch_unwind(AssertUnwindSafe(|| gw.handle(request))).unwrap_or_else(|_| {
        EdgeResponse::error(&EdgeError::Internal("gateway handler panicked".into()))
    })
}

fn is_git_wire_path(path: &str) -> bool {
    path.ends_with("/git-upload-pack")
        || path.ends_with("/git-receive-pack")
        || path.ends_with("/info/refs")
}

fn requires_large_response_budget(method: &str, path: &str) -> bool {
    if is_git_wire_path(path) {
        return true;
    }
    if method != "GET" {
        return false;
    }
    let mut segments = path.strip_prefix('/').unwrap_or(path).split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ),
        (
            Some("v1"),
            Some("git"),
            Some("repos"),
            Some(_),
            Some("raw" | "download"),
            Some(_),
            Some(_),
        )
    )
}

fn request_body_cap(path: &str) -> usize {
    if path.ends_with("/git-receive-pack") {
        MAX_REQUEST_BODY_BYTES
    } else {
        MAX_JSON_REQUEST_BODY_BYTES
    }
}

fn request_body_deadline(path: &str) -> Duration {
    if path.ends_with("/git-receive-pack") {
        GIT_PUSH_BODY_READ_TIMEOUT
    } else {
        API_BODY_READ_TIMEOUT
    }
}

fn overloaded(message: &str) -> Response<EdgeBody> {
    let err = EdgeError::Unavailable(message.into());
    let mut response = to_hyper(EdgeResponse::error(&err));
    response.headers_mut().insert(
        hyper::header::RETRY_AFTER,
        hyper::header::HeaderValue::from_static(GIT_WIRE_RETRY_AFTER_SECONDS),
    );
    response
}

// pre-auth classification for the machine dispatch pool: an explicit
// x-myelin-run-class header, or one of the machine-only credential schemes.
// "session" and "pat" stay in the general pool - they are human-operated.
fn is_machine_classified(req: &EdgeRequest) -> bool {
    if req.header(crate::shed_governor::RUN_CLASS_HEADER).is_some() {
        return true;
    }
    matches!(
        req.header("x-myelin-token-scheme").map(str::trim),
        Some("ci" | "agent" | "deploy_key" | "per_job")
    )
}

fn machine_dispatch_shed() -> Response<EdgeBody> {
    let err = EdgeError::TooManyRequests(
        "the machine dispatch pool is at capacity; retry later".into(),
    );
    let mut response = to_hyper(EdgeResponse::error(&err));
    response.headers_mut().insert(
        hyper::header::RETRY_AFTER,
        hyper::header::HeaderValue::from_static(GIT_WIRE_RETRY_AFTER_SECONDS),
    );
    response
}

fn unread_body_overloaded(message: &str) -> Response<EdgeBody> {
    let mut response = overloaded(message);
    response.headers_mut().insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("close"),
    );
    response
}

fn request_timeout(deadline: Duration) -> Response<EdgeBody> {
    let err = EdgeError::RequestTimeout(format!(
        "request body was not received within {} seconds",
        deadline.as_secs()
    ));
    let mut response = to_hyper(EdgeResponse::error(&err));
    response.headers_mut().insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("close"),
    );
    response
}

fn request_body_read_error() -> Response<EdgeBody> {
    let err = EdgeError::BadRequest("request body could not be read".into());
    let mut response = to_hyper(EdgeResponse::error(&err));
    response.headers_mut().insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("close"),
    );
    response
}

fn probe_response(status: u16, body: Vec<u8>) -> Response<EdgeBody> {
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .header("cache-control", "no-store");
    if status == 405 {
        response = response.header("allow", "GET, HEAD");
    } else if status == 503 {
        response = response.header("retry-after", READINESS_RETRY_AFTER_SECONDS);
    }
    response
        .body(full_body(body))
        .unwrap_or_else(|_| response_render_failure())
}

fn content_length_over_cap(headers: &hyper::HeaderMap, cap: usize) -> bool {
    headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .is_some_and(|declared| declared > cap as u64)
}

fn request_has_body(headers: &hyper::HeaderMap) -> bool {
    headers
        .get(hyper::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|length| length > 0)
        || headers.contains_key(hyper::header::TRANSFER_ENCODING)
}

fn payload_too_large(cap: usize) -> Response<EdgeBody> {
    let err =
        EdgeError::PayloadTooLarge(format!("request body exceeds the {cap}-byte route limit"));
    let mut response = to_hyper(EdgeResponse::error(&err));
    response.headers_mut().insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("close"),
    );
    response
}

fn to_hyper(resp: EdgeResponse) -> Response<EdgeBody> {
    to_hyper_with_permit(resp, None)
}

fn to_hyper_with_permit(
    resp: EdgeResponse,
    large_response_permit: Option<OwnedSemaphorePermit>,
) -> Response<EdgeBody> {
    match resp {
        EdgeResponse::Bytes {
            status,
            content_type,
            headers,
            body,
        } => {
            if !response_content_type_is_safe(&content_type)
                || !handler_response_headers_are_safe(&headers)
            {
                return response_render_failure();
            }
            let mut builder = Response::builder()
                .status(status)
                .header("content-type", content_type);
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
            let body = match large_response_permit {
                Some(permit) => budgeted_bytes_body(body, permit),
                None => full_body(body),
            };
            builder
                .body(body)
                .unwrap_or_else(|_| response_render_failure())
        }
        EdgeResponse::Sse {
            headers,
            sub,
            expires_at_unix,
        } => {
            drop(large_response_permit);
            if !handler_response_headers_are_safe(&headers) {
                return response_render_failure();
            }
            let mut builder = Response::builder()
                .status(200)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive");
            for (k, v) in headers {
                builder = builder.header(k, v);
            }
            builder
                .body(sse_body(sub.into_receiver(), expires_at_unix))
                .unwrap_or_else(|_| response_render_failure())
        }
    }
}

fn response_content_type_is_safe(content_type: &str) -> bool {
    matches!(
        content_type,
        "application/json"
            | "application/octet-stream"
            | "text/plain; charset=utf-8"
            | "application/x-git-upload-pack-advertisement"
            | "application/x-git-upload-pack-result"
            | "application/x-git-receive-pack-advertisement"
            | "application/x-git-receive-pack-result"
    )
}

fn handler_response_headers_are_safe(headers: &[(String, String)]) -> bool {
    headers.iter().all(|(name, value)| {
        let Ok(name) = hyper::header::HeaderName::from_bytes(name.as_bytes()) else {
            return false;
        };
        if hyper::header::HeaderValue::from_str(value).is_err() {
            return false;
        }
        !matches!(
            name.as_str(),
            "connection"
                | "content-length"
                | "content-type"
                | "keep-alive"
                | "proxy-connection"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
                | "x-request-id"
        )
    })
}

fn response_render_failure() -> Response<EdgeBody> {
    let mut response = Response::new(full_body(
        br#"{"error":{"message":"internal error","code":"internal"}}"#.to_vec(),
    ));
    *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

fn full_body(bytes: Vec<u8>) -> EdgeBody {
    Full::new(Bytes::from(bytes))
        .map_err(|never: Infallible| match never {})
        .boxed()
}

fn budgeted_bytes_body(bytes: Vec<u8>, permit: OwnedSemaphorePermit) -> EdgeBody {
    BudgetedBytesBody {
        bytes: Bytes::from(bytes),
        offset: 0,
        _permit: permit,
    }
    .boxed()
}

struct BudgetedBytesBody {
    bytes: Bytes,
    offset: usize,
    _permit: OwnedSemaphorePermit,
}

impl Body for BudgetedBytesBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.offset == self.bytes.len() {
            return Poll::Ready(None);
        }
        let end = self
            .offset
            .saturating_add(MAX_RESPONSE_FRAME_BYTES)
            .min(self.bytes.len());
        let frame = self.bytes.slice(self.offset..end);
        self.offset = end;
        Poll::Ready(Some(Ok(Frame::data(frame))))
    }

    fn is_end_stream(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        let remaining = self.bytes.len().saturating_sub(self.offset) as u64;
        hyper::body::SizeHint::with_exact(remaining)
    }
}

fn sse_body(
    rx: tokio::sync::broadcast::Receiver<crate::sse::SseEvent>,
    expires_at_unix: i64,
) -> EdgeBody {
    let expiry = u64::try_from(expires_at_unix)
        .ok()
        .and_then(|seconds| std::time::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)));
    let remaining = expiry
        .and_then(|instant| instant.duration_since(std::time::SystemTime::now()).ok())
        .unwrap_or_default();
    let stream = ExpiringSseStream {
        events: BroadcastStream::new(rx),
        expiry: Box::pin(tokio::time::sleep(remaining)),
        connected: false,
        done: false,
    };
    StreamBody::new(stream).boxed()
}

struct ExpiringSseStream {
    events: BroadcastStream<crate::sse::SseEvent>,
    expiry: Pin<Box<tokio::time::Sleep>>,
    connected: bool,
    done: bool,
}

impl tokio_stream::Stream for ExpiringSseStream {
    type Item = Result<Frame<Bytes>, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done {
            return Poll::Ready(None);
        }
        if !this.connected {
            this.connected = true;
            return Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(
                b": connected\n\n",
            )))));
        }
        if this.expiry.as_mut().poll(cx).is_ready() {
            this.done = true;
            return Poll::Ready(None);
        }
        match tokio_stream::Stream::poll_next(Pin::new(&mut this.events), cx) {
            Poll::Ready(Some(Ok(event))) => {
                Poll::Ready(Some(Ok(Frame::data(Bytes::from(event.frame())))))
            }
            Poll::Ready(Some(Err(_))) | Poll::Ready(None) => {
                this.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn sse_body_opens_the_connection_before_the_first_product_event() {
        let (_sender, receiver) = tokio::sync::broadcast::channel(2);
        let future_expiry = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock after epoch")
                .as_secs(),
        )
        .expect("test clock fits i64")
            + 60;
        let mut body = sse_body(receiver, future_expiry);

        let frame = body
            .frame()
            .await
            .expect("connected comment frame")
            .expect("infallible SSE frame");
        assert_eq!(frame.into_data().expect("data frame"), ": connected\n\n");
    }

    #[tokio::test]
    async fn lagged_sse_body_terminates_instead_of_skipping_to_newer_events() {
        let (sender, receiver) = tokio::sync::broadcast::channel(2);
        for sequence in 1..=3 {
            sender
                .send(crate::sse::SseEvent::data(sequence.to_string()))
                .unwrap();
        }

        let future_expiry = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("test clock after epoch")
                .as_secs(),
        )
        .expect("test clock fits i64")
            + 60;
        let mut body = sse_body(receiver, future_expiry);
        let _connected = body.frame().await;
        assert!(
            body.frame().await.is_none(),
            "a lagged stream closes before exposing a newer frame after the invisible gap"
        );
        assert!(
            body.frame().await.is_none(),
            "termination is permanent, not a one-item filter"
        );
    }

    #[tokio::test]
    async fn expired_capability_deadline_terminates_sse_body() {
        let (sender, receiver) = tokio::sync::broadcast::channel(2);
        let mut body = sse_body(receiver, 0);

        tokio::task::yield_now().await;
        let _connected = body.frame().await;
        assert!(body.frame().await.is_none());
        drop(body);
        assert_eq!(sender.receiver_count(), 0);
    }

    #[test]
    fn only_bounded_large_response_routes_consume_large_response_capacity() {
        for path in [
            "/v1/git/repos/widgets/raw/main/src/lib.rs",
            "/v1/git/repos/widgets/download/deadbeef/release.tar",
        ] {
            assert!(requires_large_response_budget("GET", path), "{path}");
        }
        for (method, path) in [
            ("GET", "/acme/eu/widgets.git/info/refs"),
            ("POST", "/acme/eu/widgets.git/git-upload-pack"),
            ("POST", "/acme/eu/widgets.git/git-receive-pack"),
        ] {
            assert!(
                requires_large_response_budget(method, path),
                "{method} {path}"
            );
        }
        for (method, path) in [
            ("HEAD", "/v1/git/repos/widgets/raw/main/src/lib.rs"),
            ("POST", "/v1/git/repos/widgets/download/main/archive"),
            ("GET", "/v1/git/repos/widgets/raw/main"),
            ("GET", "/v1/git/repos/widgets/rawish/main/file"),
            ("GET", "/v1/git/raw/widgets/main/file"),
            ("GET", "/v2/git/repos/widgets/raw/main/file"),
        ] {
            assert!(
                !requires_large_response_budget(method, path),
                "{method} {path}"
            );
        }
    }

    #[tokio::test]
    async fn budgeted_response_is_chunked_and_holds_capacity_until_body_drop() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().expect("one slot");
        let payload = vec![7; MAX_RESPONSE_FRAME_BYTES * 2 + 7];
        let mut body = budgeted_bytes_body(payload.clone(), permit);

        assert_eq!(slots.available_permits(), 0);
        let mut received = Vec::new();
        let mut frame_lengths = Vec::new();
        while let Some(frame) = body.frame().await {
            let data = frame
                .expect("infallible response body")
                .into_data()
                .expect("data frame");
            frame_lengths.push(data.len());
            received.extend_from_slice(&data);
        }

        assert_eq!(received, payload);
        assert_eq!(
            frame_lengths,
            [MAX_RESPONSE_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES, 7]
        );
        assert_eq!(
            slots.available_permits(),
            0,
            "the live body retains its capacity permit"
        );
        drop(body);
        assert_eq!(
            slots.available_permits(),
            1,
            "cancellation or completion releases capacity"
        );
    }

    #[tokio::test]
    async fn under_cap_collects_full_body() {
        let payload = b"the quick brown fox jumps over the lazy dog".to_vec();
        let body = Full::new(Bytes::from(payload.clone()));
        let out = collect_bounded(body, MAX_REQUEST_BODY_BYTES, Duration::from_secs(1))
            .await
            .expect("under cap");
        assert_eq!(out, payload, "an under-cap body is returned exactly");
    }

    #[tokio::test]
    async fn exactly_at_cap_is_accepted() {
        let cap = 4096;
        let body = Full::new(Bytes::from(vec![7u8; cap]));
        let out = collect_bounded(body, cap, Duration::from_secs(1))
            .await
            .expect("at cap is accepted");
        assert_eq!(out.len(), cap);
    }

    #[tokio::test]
    async fn over_cap_rejects_without_buffering_full_input() {
        let cap = 4096usize;
        let chunk = 1024usize;
        let total_chunks = 4096usize;
        let pulled = Arc::new(AtomicUsize::new(0));
        let counter = pulled.clone();
        let chunks: Vec<Bytes> = (0..total_chunks)
            .map(|_| Bytes::from(vec![0u8; chunk]))
            .collect();
        let stream = tokio_stream::iter(chunks).map(move |b| {
            counter.fetch_add(b.len(), Ordering::SeqCst);
            Ok::<Frame<Bytes>, std::io::Error>(Frame::data(b))
        });
        let body = StreamBody::new(stream);

        let res = collect_bounded(body, cap, Duration::from_secs(1)).await;
        assert_eq!(
            res,
            Err(BoundedCollectError::TooLarge),
            "over-cap body is rejected"
        );

        let consumed = pulled.load(Ordering::SeqCst);
        assert!(
            consumed <= cap + chunk,
            "reading stopped near the cap (consumed {consumed}), not the full {} input bytes",
            total_chunks * chunk
        );
    }

    #[tokio::test]
    async fn trailers_frame_is_not_counted_as_body() {
        let mut trailers = hyper::HeaderMap::new();
        trailers.insert("x-checksum", hyper::header::HeaderValue::from_static("abc"));
        let frames: Vec<Result<Frame<Bytes>, std::io::Error>> = vec![
            Ok(Frame::data(Bytes::from_static(b"hello"))),
            Ok(Frame::trailers(trailers)),
        ];
        let body = StreamBody::new(tokio_stream::iter(frames));
        let out = collect_bounded(body, 1024, Duration::from_secs(1))
            .await
            .expect("data under cap");
        assert_eq!(out, b"hello", "only the data frame contributes to the body");
    }

    #[tokio::test]
    async fn read_error_surfaces_as_read_not_too_large() {
        let frames: Vec<Result<Frame<Bytes>, std::io::Error>> = vec![
            Ok(Frame::data(Bytes::from_static(b"partial"))),
            Err(std::io::Error::other("connection reset")),
        ];
        let body = StreamBody::new(tokio_stream::iter(frames));
        let res = collect_bounded(body, 1024, Duration::from_secs(1)).await;
        assert_eq!(
            res,
            Err(BoundedCollectError::Read),
            "a mid-body read error is Read, not TooLarge"
        );
    }

    #[tokio::test]
    async fn stalled_body_hits_the_absolute_read_deadline() {
        let body = StreamBody::new(tokio_stream::pending::<Result<Frame<Bytes>, std::io::Error>>());
        let res = collect_bounded(body, 1024, Duration::from_millis(10)).await;
        assert_eq!(res, Err(BoundedCollectError::TimedOut));
    }

    #[test]
    fn content_length_over_cap_fast_rejects() {
        let cap = 4096usize;
        let mk = |val: &str| {
            let mut h = hyper::HeaderMap::new();
            h.insert(
                hyper::header::CONTENT_LENGTH,
                hyper::header::HeaderValue::from_str(val).unwrap(),
            );
            h
        };
        assert!(
            content_length_over_cap(&mk("4097"), cap),
            "declared > cap rejects"
        );
        assert!(
            !content_length_over_cap(&mk("4096"), cap),
            "declared == cap does not reject"
        );
        assert!(
            !content_length_over_cap(&mk("10"), cap),
            "declared < cap does not reject"
        );
        assert!(
            !content_length_over_cap(&mk("not-a-number"), cap),
            "unparseable does not fast-reject"
        );
        assert!(
            !content_length_over_cap(&hyper::HeaderMap::new(), cap),
            "absent Content-Length does not fast-reject (streaming bound enforces)"
        );
    }

    #[test]
    fn payload_too_large_uses_the_canonical_413_envelope() {
        let resp = payload_too_large(MAX_REQUEST_BODY_BYTES);
        assert_eq!(resp.status(), 413);
        assert_eq!(resp.headers()[hyper::header::CONNECTION], "close");
        let err = EdgeError::PayloadTooLarge("x".into());
        assert_eq!(err.status(), 413);
        assert_eq!(err.code(), "payload_too_large");
    }

    #[test]
    fn only_git_receive_pack_gets_the_large_body_budget() {
        assert_eq!(
            MAX_REQUEST_BODY_BYTES,
            myelin_ci_sandbox::gvisor::WIRE_STDIN_BOUND,
            "the edge must never buffer bytes the sandbox will unconditionally reject"
        );
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
            assert_eq!(
                request_body_cap(path),
                MAX_JSON_REQUEST_BODY_BYTES,
                "{path}"
            );
        }
    }

    #[test]
    fn every_git_wire_endpoint_uses_the_bounded_blocking_pool() {
        for path in [
            "/acme/eu-west/widgets.git/info/refs",
            "/acme/eu-west/widgets.git/git-upload-pack",
            "/acme/eu-west/widgets.git/git-receive-pack",
        ] {
            assert!(is_git_wire_path(path), "{path}");
        }
        for path in ["/v1/git/repos", "/v1/issues", "/livez"] {
            assert!(!is_git_wire_path(path), "{path}");
        }
    }

    #[test]
    fn git_receive_pack_gets_a_longer_but_finite_body_deadline() {
        let push = "/acme/eu-west/widgets.git/git-receive-pack";
        assert_eq!(request_body_deadline(push), GIT_PUSH_BODY_READ_TIMEOUT);
        assert_eq!(request_body_deadline("/v1/issues"), API_BODY_READ_TIMEOUT);
        assert!(GIT_PUSH_BODY_READ_TIMEOUT > API_BODY_READ_TIMEOUT);
    }

    #[test]
    fn request_timeout_uses_the_canonical_408_envelope() {
        let resp = request_timeout(API_BODY_READ_TIMEOUT);
        assert_eq!(resp.status(), 408);
        assert_eq!(resp.headers()[hyper::header::CONNECTION], "close");
        let err = EdgeError::RequestTimeout("x".into());
        assert_eq!(err.code(), "request_timeout");
    }

    #[test]
    fn pre_read_body_overload_closes_the_connection() {
        let response =
            unread_body_overloaded("the Git push upload service is at capacity; retry later");
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()[hyper::header::RETRY_AFTER], "1");
        assert_eq!(response.headers()[hyper::header::CONNECTION], "close");
    }

    #[test]
    fn request_body_admission_recognizes_fixed_and_chunked_framing() {
        let mut headers = hyper::HeaderMap::new();
        assert!(!request_has_body(&headers));
        headers.insert(
            hyper::header::CONTENT_LENGTH,
            hyper::header::HeaderValue::from_static("0"),
        );
        assert!(!request_has_body(&headers));
        headers.insert(
            hyper::header::CONTENT_LENGTH,
            hyper::header::HeaderValue::from_static("1"),
        );
        assert!(request_has_body(&headers));
        headers.remove(hyper::header::CONTENT_LENGTH);
        headers.insert(
            hyper::header::TRANSFER_ENCODING,
            hyper::header::HeaderValue::from_static("chunked"),
        );
        assert!(request_has_body(&headers));
    }

    #[test]
    fn body_read_error_uses_a_canonical_400_and_closes_the_connection() {
        let resp = request_body_read_error();
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()[hyper::header::CONNECTION], "close");
    }

    #[test]
    fn invalid_response_metadata_fails_closed_as_a_canonical_500() {
        let response = to_hyper(
            EdgeResponse::json(200, &serde_json::json!({ "false": "success" }))
                .with_header("x-invalid", "line\nbreak"),
        );
        assert_eq!(response.status(), 500);
        assert_eq!(
            response.headers()[hyper::header::CONTENT_TYPE],
            "application/json"
        );
    }

    #[test]
    fn handler_cannot_control_transport_owned_response_headers() {
        for name in [
            "connection",
            "content-length",
            "content-type",
            "keep-alive",
            "proxy-connection",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "x-request-id",
        ] {
            assert!(
                !handler_response_headers_are_safe(&[(name.into(), "value".into())]),
                "{name} must remain transport-owned"
            );
        }
        assert!(handler_response_headers_are_safe(&[
            ("cache-control".into(), "no-store".into()),
            ("set-cookie".into(), "session=opaque; HttpOnly".into()),
            ("www-authenticate".into(), "Basic realm=\"Myelin\"".into()),
        ]));
    }

    #[test]
    fn response_content_types_are_a_closed_non_active_set() {
        for content_type in [
            "application/json",
            "application/octet-stream",
            "text/plain; charset=utf-8",
            "application/x-git-upload-pack-advertisement",
            "application/x-git-upload-pack-result",
            "application/x-git-receive-pack-advertisement",
            "application/x-git-receive-pack-result",
        ] {
            assert!(
                response_content_type_is_safe(content_type),
                "{content_type}"
            );
        }
        for content_type in ["text/html", "image/svg+xml", "text/javascript"] {
            assert!(
                !response_content_type_is_safe(content_type),
                "{content_type}"
            );
        }
    }

    #[test]
    fn request_ids_are_server_generated_ascii_and_unique() {
        let first = next_request_id();
        let second = next_request_id();
        assert_ne!(first, second);
        for id in [first, second] {
            assert_eq!(id.len(), 40);
            assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn access_records_use_bounded_classes_never_raw_paths() {
        assert_eq!(route_class("/livez"), "health");
        assert_eq!(
            route_class("/secret-tenant/eu-west/private.git/git-upload-pack"),
            "git_wire"
        );
        assert_eq!(route_class("/v1/auth/refresh"), "auth");
        assert_eq!(route_class("/v1/issues/secret-title"), "api");
        assert_eq!(route_class("/attacker-controlled"), "unknown");

        let record = access_log_record(
            "request-id",
            "GET",
            "git_wire",
            503,
            Duration::from_micros(42),
        );
        let encoded = record.to_string();
        assert_eq!(record["duration_us"], 42);
        assert!(!encoded.contains("secret-tenant"));
        assert!(!encoded.contains("private.git"));
        assert!(!encoded.contains("authorization"));
    }
}
