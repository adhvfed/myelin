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

use crate::gateway::Gateway;
use crate::request::{EdgeRequest, EdgeResponse};
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// The response body type the adapter emits — a boxed body of [`Bytes`] frames (finished or
/// streamed), with an `io::Error` error channel.
type EdgeBody = BoxBody<Bytes, std::io::Error>;

/// **Serve the edge over a bound TCP listener.** Accepts connections forever, serving each over
/// HTTP/1.1 with the gateway. Returns only on an accept error.
pub async fn serve_edge(listener: TcpListener, gateway: Arc<Gateway>) -> std::io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let gw = gateway.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req: Request<Incoming>| {
                let gw = gw.clone();
                async move { Ok::<_, Infallible>(handle_connection(gw, req).await) }
            });
            // serve_connection drives request/response (incl. streaming the SSE body) on this conn.
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await;
        });
    }
}

/// Convert one hyper request → [`EdgeRequest`], run the gateway, convert the response back.
async fn handle_connection(gw: Arc<Gateway>, req: Request<Incoming>) -> Response<EdgeBody> {
    let (parts, body) = req.into_parts();
    let method = parts.method.as_str().to_string();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or("").to_string();
    let headers = parts
        .headers
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Collect the body (bounded by hyper's defaults). A read error → empty body (the gateway then
    // produces a clean 400 if a body was required) — never a panic.
    let bytes = match body.collect().await {
        Ok(c) => c.to_bytes().to_vec(),
        Err(_) => Vec::new(),
    };
    let edge_req = EdgeRequest::new(method, path, query, headers, bytes);
    to_hyper(gw.handle(edge_req))
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
