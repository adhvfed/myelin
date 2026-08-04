use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

const CHANNEL_CAPACITY: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: String,
}

impl SseEvent {
    pub fn data(data: impl Into<String>) -> SseEvent {
        SseEvent {
            event: None,
            id: None,
            data: data.into(),
        }
    }

    pub fn typed(event: impl Into<String>, data: impl Into<String>) -> SseEvent {
        SseEvent {
            event: Some(event.into()),
            id: None,
            data: data.into(),
        }
    }

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

pub struct SseSubscription {
    rx: broadcast::Receiver<SseEvent>,
}

impl SseSubscription {
    pub fn into_receiver(self) -> broadcast::Receiver<SseEvent> {
        self.rx
    }

    pub(crate) fn bounded_channel(
        capacity: usize,
    ) -> (broadcast::Sender<SseEvent>, SseSubscription) {
        let (tx, rx) = broadcast::channel(capacity);
        (tx, SseSubscription { rx })
    }
}

type SseChannels = HashMap<(String, String), broadcast::Sender<SseEvent>>;

#[derive(Clone, Default)]
pub struct SseHub {
    inner: Arc<Mutex<SseChannels>>,
}

impl SseHub {
    pub fn new() -> SseHub {
        SseHub::default()
    }

    pub fn subscribe(&self, stream: &str, scope: &str) -> SseSubscription {
        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = (stream.to_string(), scope.to_string());
        let tx = map
            .entry(key)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        SseSubscription { rx: tx.subscribe() }
    }

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
        let ev = SseEvent {
            event: Some("notification".into()),
            id: Some("42".into()),
            data: "{\"n\":1}".into(),
        };
        assert_eq!(
            ev.frame(),
            "event: notification\nid: 42\ndata: {\"n\":1}\n\n"
        );
        assert_eq!(SseEvent::data("a\nb").frame(), "data: a\ndata: b\n\n");
    }

    #[test]
    fn hub_fans_a_frame_out_to_subscribers_on_the_same_scope_only() {
        let hub = SseHub::new();
        let mut a = hub.subscribe("edge", "tenant:acme").into_receiver();
        let _b = hub.subscribe("edge", "tenant:globex").into_receiver();
        let reached = hub.broadcast("edge", "tenant:acme", SseEvent::data("hi"));
        assert_eq!(reached, 1, "only the acme subscriber is on tenant:acme");
        assert_eq!(a.try_recv().unwrap(), SseEvent::data("hi"));
        assert_eq!(
            hub.broadcast("edge", "tenant:nobody", SseEvent::data("x")),
            0
        );
    }
}
