//! # The SSE real-time convention — the backend stream the UI consumes via `EventSource`
//!
//! The frontend canon §6 subscribes to real-time over **`EventSource` (SSE)** (a SolidStart API
//! route proxies the backend stream to the client as SSE). This module is the backend half: the
//! **SSE endpoint shape** + **how a subsystem publishes to it**.
//!
//! - **The frame shape** ([`SseEvent`]) — the `event:`/`id:`/`data:` lines the browser `EventSource`
//!   parses. [`SseEvent::frame`] renders the wire form (a blank line terminates each event).
//! - **The hub** ([`SseHub`]) — a per-`(stream, scope)` broadcast. A subsystem PUBLISHES a frame to a
//!   `(stream, scope)` via [`SseHub::broadcast`]; every connected subscriber on that key receives it.
//!   This reuses the firehose/events surface SHAPE (substrate `firehose`/`firehose_selector`,
//!   contract 3.5): the **scope is bounded, never `*`** — the gateway derives it from the VERIFIED
//!   tenant (+ an optional bounded resource id), so a connection can never subscribe across tenants
//!   or to an unbounded selector (the §7.7 "never `*`" guarantee, the IDOR floor for streams).
//!
//! **Floor named (EI-01 §4):** the durable zero-loss-across-reconnect backbone (subscribe/resume/
//! `resync_required → *.snapshot`, the substrate `firehose` half + the Bus EB-21 half) is the
//! connection-tier deferred piece; here the hub is a REAL in-process broadcast that streams live
//! frames over the real SSE transport — enough to PROVE the edge streams; the resume-cursor durability
//! rides the firehose seam.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// The per-`(stream, scope)` broadcast capacity (bounded — a slow consumer that lags past this is
/// dropped by the broadcast channel, the bounded-and-sheds posture, contract §7.7). The slow-consumer
/// → `resync_required` snapshot fallback is the firehose seam's deferred half (named above).
const CHANNEL_CAPACITY: usize = 256;

/// One Server-Sent Event frame (the `EventSource` wire shape). `event` is the optional event type the
/// client `addEventListener`s on; `id` is the optional Last-Event-ID resume cursor; `data` is the
/// payload (a JSON view-model projection, references-not-payloads). Clone so one frame fans out to
/// every subscriber on the broadcast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The optional SSE `event:` type (e.g. `"notification"`, `"presence"`).
    pub event: Option<String>,
    /// The optional `id:` (the Last-Event-ID resume cursor — the firehose op_seq on the real seam).
    pub id: Option<String>,
    /// The `data:` payload — a JSON projection (never raw PII; a reference/short-form).
    pub data: String,
}

impl SseEvent {
    /// A bare data-only event.
    pub fn data(data: impl Into<String>) -> SseEvent {
        SseEvent { event: None, id: None, data: data.into() }
    }

    /// A typed event (`event:` + `data:`).
    pub fn typed(event: impl Into<String>, data: impl Into<String>) -> SseEvent {
        SseEvent { event: Some(event.into()), id: None, data: data.into() }
    }

    /// Render the SSE wire frame: `event: …\n` (if any), `id: …\n` (if any), one `data: …\n` per
    /// line of `data` (so a multi-line payload is legal), then the terminating blank line.
    pub fn frame(&self) -> String {
        let mut s = String::new();
        if let Some(e) = &self.event {
            s.push_str("event: ");
            s.push_str(e);
            s.push('\n');
        }
        if let Some(id) = &self.id {
            s.push_str("id: ");
            s.push_str(id);
            s.push('\n');
        }
        for line in self.data.split('\n') {
            s.push_str("data: ");
            s.push_str(line);
            s.push('\n');
        }
        s.push('\n');
        s
    }
}

/// A live subscription to a `(stream, scope)` — the receiving half handed to the SSE response body.
/// Not `Clone` (one receiver per connection).
pub struct SseSubscription {
    rx: broadcast::Receiver<SseEvent>,
}

impl SseSubscription {
    /// Consume into the underlying broadcast receiver (the server adapts it to a streaming body).
    pub fn into_receiver(self) -> broadcast::Receiver<SseEvent> {
        self.rx
    }
}

/// **The SSE hub — a per-`(stream, scope)` broadcast.** A subsystem PUBLISHES frames to a bounded
/// `(stream, scope)`; every connected subscriber on that key receives them. Cloneable (one hub shared
/// by the gateway + every publisher). The scope passed in is ALWAYS the gateway-derived bounded scope
/// (verified tenant + optional resource) — never a client-supplied selector.
#[derive(Clone, Default)]
pub struct SseHub {
    inner: Arc<Mutex<HashMap<(String, String), broadcast::Sender<SseEvent>>>>,
}

impl SseHub {
    /// A fresh, empty hub.
    pub fn new() -> SseHub {
        SseHub::default()
    }

    /// Subscribe a new connection to `(stream, scope)` — get-or-create the bounded broadcast channel
    /// and return its receiver. The `scope` MUST be the gateway-derived bounded scope (the verified
    /// tenant), never a raw client selector.
    pub fn subscribe(&self, stream: &str, scope: &str) -> SseSubscription {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = (stream.to_string(), scope.to_string());
        let tx = map
            .entry(key)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        SseSubscription { rx: tx.subscribe() }
    }

    /// **Publish a frame to `(stream, scope)`** (the subsystem→edge publish convention). Returns the
    /// number of live subscribers the frame reached (0 if none are connected — a frame with no
    /// listener is dropped, the ephemeral firehose posture; durability is the resume-cursor seam).
    /// NOTE: deliberately NOT named `publish` — that token is the `no-raw-publish` lint fingerprint
    /// for the DURABLE bus (`OutboxTx::emit`); this is the EPHEMERAL firehose surface (a different
    /// seam, §4.3), so it uses a distinct name to keep the durable-bus lint unambiguous.
    pub fn broadcast(&self, stream: &str, scope: &str, event: SseEvent) -> usize {
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(&(stream.to_string(), scope.to_string())) {
            Some(tx) => tx.send(event).unwrap_or(0),
            None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_renders_the_eventsource_wire_shape() {
        let ev = SseEvent { event: Some("notification".into()), id: Some("42".into()), data: "{\"n\":1}".into() };
        assert_eq!(ev.frame(), "event: notification\nid: 42\ndata: {\"n\":1}\n\n");
        // multi-line data → one `data:` per line.
        assert_eq!(SseEvent::data("a\nb").frame(), "data: a\ndata: b\n\n");
    }

    #[test]
    fn hub_fans_a_frame_out_to_subscribers_on_the_same_scope_only() {
        let hub = SseHub::new();
        let mut a = hub.subscribe("edge", "tenant:acme").into_receiver();
        let _b = hub.subscribe("edge", "tenant:globex").into_receiver();
        // A frame to acme reaches the acme subscriber (1), not globex.
        let reached = hub.broadcast("edge", "tenant:acme", SseEvent::data("hi"));
        assert_eq!(reached, 1, "only the acme subscriber is on tenant:acme");
        assert_eq!(a.try_recv().unwrap(), SseEvent::data("hi"));
        // A frame to a scope with no subscriber reaches 0 (dropped, ephemeral).
        assert_eq!(hub.broadcast("edge", "tenant:nobody", SseEvent::data("x")), 0);
    }
}
