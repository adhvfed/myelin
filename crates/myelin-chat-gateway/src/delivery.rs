//! # The firehose-ONLY live-delivery surface (CHAT-P10 / P-404).
//!
//! **Owning architecture doc:** `chat/architecture/02-internals-and-algorithms.md` §7 (agent
//! presence §7.2, streaming partials §7.3, typing/read-state over the firehose) +
//! `03-events-contracts-and-glue.md` §1.2 (the FIREHOSE-ONLY set: `chat.presence.*` / `chat.typing.*`
//! / fine `chat.read_state.*` / `agent.message.partial` / the live message-delivery frame — NEVER the
//! durable bus, ADR-04.5). Contract 3.5 (the firehose resume-cursor transport the live frames ride).
//!
//! ## The no-raw-publish seam (why this is the gateway's ONE publish site)
//! The live-delivery surface is the ONE place in the gateway that calls
//! [`myelin_events::Firehose::publish`] — the EPHEMERAL references-not-payloads transport, a
//! DIFFERENT seam from the durable bus the `no-raw-publish` lint guards (the durable bus carries only
//! pointer EVENTS via `OutboxTx::emit`; the firehose carries ephemeral allowed-to-drop frames over
//! its own publish/subscribe/resume API, refined-arch §4.3 / ADR-04.5). This module is a NAMED, LOUD
//! exclusion in `myelin-lints/tests/workspace_clean.rs` — exactly the posture of
//! `myelin-knowledge/src/transport.rs` (collab op-stream) and `myelin-ci-controlplane/src/
//! log_pipeline.rs` (CI log live-tail). Every OTHER gateway module stays fully linted: the
//! shed-order governor ([`crate::shed`]) and the frame builders here that do NOT publish carry no
//! `.publish(` and are scanned.
//!
//! ## Firehose-ONLY — structurally off the durable bus
//! There is NO `OutboxTx`, NO `BusTransport`, NO durable-bus handle in this module — the live frames
//! CANNOT reach the durable bus because the only transport in scope is the ephemeral [`Firehose`].
//! The `no-raw-publish` gate stays green (0 firehose frames on the durable bus): the seam is
//! enforced by construction, not by convention. If lost, the durable record (the final
//! `chat.message.created` for a message, the durable coarse read-state for a fine marker) is the
//! truth (resume-on-reconnect, arch §1.3).

// The chat subsystem's PUBLIC contract surface (the top-level re-exports — NEVER the private
// `::store` data path; the gateway is chat's OWN connection tier reading chat's OWN types through
// its public API, the proper subsystem-internal coupling, ADR-01 / no-cross-db).
use myelin_chat::{AuthorKind, ConversationId, Message};
use myelin_events::{Firehose, FirehoseScope, Frame, FrameDraft};
use myelin_identity::PrincipalId;
use myelin_tenancy::TenantId;

use crate::shed::{LiveSurface, ShedGovernor, ShedVerdict};

/// **A live frame the connection tier delivers over the FIREHOSE (never the durable bus).** Each
/// variant maps to a firehose-only `chat.*` token (arch §1.2) + the [`LiveSurface`] the shed order
/// keys on. The payload is an OPAQUE references-not-payloads pointer (a message id, a presence
/// summary, an op id) — never an inline PII body (the firehose carries a pointer, the durable record
/// carries the truth).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveFrame {
    /// The live delivery of a human's message — the PROTECTED human lane ([`LiveSurface::HumanMessage`]).
    /// The frame is a POINTER to the durable `chat.message.created` (its `message_id`); the body is
    /// resolved by the connection tier from the durable record, never carried inline. Shed LAST.
    HumanMessage {
        /// The durable message id the live frame points at (the references-not-payloads pointer).
        message_id: String,
    },
    /// An agent's streaming partial (`agent.message.partial`) — the AGENT lane
    /// ([`LiveSurface::AgentPartial`]). Live-only; if lost the FINAL durable message is the truth
    /// (arch §7.3). Carries the run's `correlation_id` (the partial→final reconciliation key) — a
    /// pointer, not the streamed text.
    AgentPartial {
        /// The run correlation id the final durable message reconciles the partial against.
        correlation_id: String,
    },
    /// A presence change (`chat.presence.changed`, incl. agent-presence classes) —
    /// [`LiveSurface::Speculative`]. Shed FIRST; re-derived on the next heartbeat.
    Presence {
        /// The principal whose presence changed.
        principal: String,
        /// The presence class glyph/label (`online`/`away`/`offline`; or the agent classes
        /// `available`/`busy`/`rate-limited`/`offline`) — status by glyph+label, never colour alone.
        class: String,
    },
    /// A typing start/stop (`chat.typing.*`) — [`LiveSurface::Typing`]. Self-heals on TTL.
    Typing {
        /// The principal typing.
        principal: String,
        /// `true` for a typing-started frame, `false` for typing-stopped.
        started: bool,
    },
    /// A fine-grained read-state marker (`chat.read_state.viewed`) — [`LiveSurface::ReadState`]. The
    /// durable COARSE summary is the recovery truth if lost.
    ReadState {
        /// The principal whose viewed-marker advanced.
        principal: String,
        /// The message id the marker advanced to (the pointer).
        up_to_message_id: String,
    },
}

impl LiveFrame {
    /// The [`LiveSurface`] this frame rides — the rung the protected-human-lane shed order keys on.
    pub fn surface(&self) -> LiveSurface {
        match self {
            LiveFrame::HumanMessage { .. } => LiveSurface::HumanMessage,
            LiveFrame::AgentPartial { .. } => LiveSurface::AgentPartial,
            LiveFrame::Presence { .. } => LiveSurface::Speculative,
            LiveFrame::Typing { .. } => LiveSurface::Typing,
            LiveFrame::ReadState { .. } => LiveSurface::ReadState,
        }
    }

    /// The firehose-only `chat.*` / `agent.*` token this frame is (arch §1.2). NAMED so a drill
    /// asserts the live frames map onto the firehose-only set, never a durable token.
    pub fn token(&self) -> &'static str {
        match self {
            // the live message-delivery frame is a pointer to the durable chat.message.created — on
            // the firehose it is the live-delivery frame (not the durable event itself).
            LiveFrame::HumanMessage { .. } => myelin_chat::events::CHAT_MESSAGE_CREATED,
            LiveFrame::AgentPartial { .. } => "agent.message.partial",
            LiveFrame::Presence { .. } => myelin_chat::events::CHAT_PRESENCE_CHANGED,
            LiveFrame::Typing { started: true, .. } => myelin_chat::events::CHAT_TYPING_STARTED,
            LiveFrame::Typing { started: false, .. } => myelin_chat::events::CHAT_TYPING_STOPPED,
            LiveFrame::ReadState { .. } => myelin_chat::events::CHAT_READ_STATE_VIEWED,
        }
    }

    /// The OPAQUE firehose payload pointer (references-not-payloads). A compact `token|pointer`
    /// string — the transport never reads its body; the connection tier resolves it. NEVER an inline
    /// PII body.
    pub fn payload_pointer(&self) -> String {
        match self {
            LiveFrame::HumanMessage { message_id } => format!("{}|{message_id}", self.token()),
            LiveFrame::AgentPartial { correlation_id } => {
                format!("{}|{correlation_id}", self.token())
            }
            LiveFrame::Presence { principal, class } => {
                format!("{}|{principal}:{class}", self.token())
            }
            LiveFrame::Typing { principal, .. } => format!("{}|{principal}", self.token()),
            LiveFrame::ReadState {
                principal,
                up_to_message_id,
            } => format!("{}|{principal}:{up_to_message_id}", self.token()),
        }
    }

    /// Build the live-delivery POINTER frame for a durable [`Message`] (arch §7.3 / §1.2). A human
    /// message → the protected human lane; an agent message → the agent lane (a partial's final
    /// replacement still delivers as the agent author class). The body is NOT carried — the frame is
    /// the `message_id` pointer the connection tier resolves from the durable record.
    pub fn from_message(msg: &Message) -> LiveFrame {
        match msg.author_kind {
            // a human's message AND a service/system message ride the PROTECTED lane (a system
            // message is not an agent run — it is never shed before a human's message; both deliver
            // last). Only an agent-authored message rides the shed-before-humans agent lane.
            AuthorKind::Human | AuthorKind::Service => LiveFrame::HumanMessage {
                message_id: msg.message_id.0.clone(),
            },
            // an agent's FINAL durable message delivers live too; on the firehose it rides the agent
            // lane (it replaces the partial, reconciled on the correlation/message id — arch §7.3).
            AuthorKind::Agent => LiveFrame::AgentPartial {
                correlation_id: msg.message_id.0.clone(),
            },
        }
    }
}

/// **The outcome of a live delivery — DELIVERED (published on the firehose) or SHED (the shed order
/// dropped it).** A shed frame is NOT an error: ephemeral surfaces are allowed-to-drop (arch §1.2);
/// the agent lane sheds with backpressure; the human lane is shed only as the absolute last resort
/// behind its reservation. The connection tier records the verdict (the shed-count drill signal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// The frame was published on the firehose at this assigned seq (the resume cursor).
    Delivered(Frame),
    /// The frame was SHED by the protected-human-lane shed order (allowed-to-drop / backpressure).
    Shed,
}

impl DeliveryOutcome {
    /// `true` iff the frame was delivered (published on the firehose).
    pub fn is_delivered(&self) -> bool {
        matches!(self, DeliveryOutcome::Delivered(_))
    }
    /// The assigned firehose frame iff delivered.
    pub fn frame(&self) -> Option<&Frame> {
        match self {
            DeliveryOutcome::Delivered(f) => Some(f),
            DeliveryOutcome::Shed => None,
        }
    }
}

/// **The firehose-ONLY live-delivery surface (CHAT-P10).** Holds the ephemeral [`Firehose`]
/// transport (the ONLY transport in scope — there is NO durable-bus handle, so a live frame CANNOT
/// reach the durable bus by construction) and applies the protected-human-lane shed order
/// ([`ShedGovernor`]) before every publish. Per-tenant: the governor is the connection-storm +
/// agent-mention-storm budget chat owns (contract 1.11).
///
/// This is composed INTO the gateway (it is the gateway's delivery seam); it owns no durable store
/// and emits no durable event (the gateway has no emit path — the durable `chat.message.created` is
/// the Message Service's outbox-co-committed write, arch §9). The live frame here is the EPHEMERAL
/// pointer that fans out to the open subscriptions.
pub struct LiveDelivery<'a> {
    /// The ephemeral firehose transport (the live frames ride contract 3.5; NEVER the durable bus).
    firehose: &'a mut Firehose,
    /// The per-tenant shed governor (the protected-human-lane shed order + per-surface budgets).
    governor: &'a mut ShedGovernor,
}

impl<'a> LiveDelivery<'a> {
    /// Compose the live-delivery surface over the firehose transport + the per-tenant shed governor.
    pub fn new(firehose: &'a mut Firehose, governor: &'a mut ShedGovernor) -> LiveDelivery<'a> {
        LiveDelivery { firehose, governor }
    }

    /// The shed governor (mutable) — the connection records a frame DRAINED (pumped to the socket /
    /// acked) here via [`ShedGovernor::on_drained`], releasing its per-tenant slot so the lane
    /// recovers. The interactive human lane is drained promptly (it keeps its budget free while the
    /// machine lanes back up under storm).
    pub fn governor_mut(&mut self) -> &mut ShedGovernor {
        self.governor
    }

    /// **Deliver a live frame FIREHOSE-ONLY, shed-order-gated (arch §1.2 / §7; contracts 3.5 / 1.11).**
    /// First the protected-human-lane shed order ([`ShedGovernor::admit`]): a frame over its
    /// surface's budget under storm pressure is SHED (allowed-to-drop / backpressure) — and the human
    /// lane is never shed while a lower-priority surface still has budget. If admitted, the frame is
    /// published on the EPHEMERAL firehose (the ONLY transport in scope — it cannot reach the durable
    /// bus) over the bounded `channel:<id>` scope.
    ///
    /// `stream` is the connection's `fan.<tenant>` stream; `scope` is the bounded `channel:<id>`
    /// slice (built through the `*`-rejecting chokepoint, never `*`). Returns the [`DeliveryOutcome`]
    /// — `Delivered(frame)` with the assigned seq, or `Shed`.
    pub fn deliver(
        &mut self,
        tenant: &TenantId,
        stream: &str,
        scope: &FirehoseScope,
        frame: &LiveFrame,
    ) -> DeliveryOutcome {
        let surface = frame.surface();
        // The shed gate is PER-TENANT (the substrate blast-radius guarantee): the tenant is the
        // connection's verified tenant (the `fan.<tenant>` stream key half, ID-3), never a path.
        match self.governor.admit(tenant, surface) {
            ShedVerdict::Shed { .. } => DeliveryOutcome::Shed,
            ShedVerdict::Deliver => {
                // The ONE firehose publish site in the gateway (the ephemeral references-not-payloads
                // transport — NAMED, LOUD lint exclusion; a DIFFERENT seam from the durable bus).
                let published =
                    self.firehose
                        .publish(stream, scope, FrameDraft::new(frame.payload_pointer()));
                DeliveryOutcome::Delivered(published)
            }
        }
    }

    /// Deliver a durable [`Message`]'s live POINTER frame (the live message-delivery surface) — the
    /// common human/agent send path. A convenience over [`Self::deliver`] + [`LiveFrame::from_message`].
    pub fn deliver_message(
        &mut self,
        tenant: &TenantId,
        stream: &str,
        scope: &FirehoseScope,
        _conv: &ConversationId,
        msg: &Message,
    ) -> DeliveryOutcome {
        let frame = LiveFrame::from_message(msg);
        self.deliver(tenant, stream, scope, &frame)
    }

    /// Deliver a presence change (`chat.presence.changed`) — the speculative lane (shed first).
    pub fn deliver_presence(
        &mut self,
        tenant: &TenantId,
        stream: &str,
        scope: &FirehoseScope,
        principal: &PrincipalId,
        class: &str,
    ) -> DeliveryOutcome {
        let frame = LiveFrame::Presence {
            principal: principal.0.clone(),
            class: class.to_string(),
        };
        self.deliver(tenant, stream, scope, &frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_chat::{MessageId, MessageState};
    use myelin_events::{ScopeKind, DEFAULT_INFLIGHT_CAP};

    fn scope() -> FirehoseScope {
        FirehoseScope::parse("channel:01J0CHANNEL").expect("bounded scope")
    }

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn human_msg(id: &str) -> Message {
        Message {
            message_id: MessageId(id.into()),
            conv: ConversationId::new("acme", "fr-par", "01J0CHANNEL"),
            thread_root_id: None,
            author: "alice".into(),
            author_kind: AuthorKind::Human,
            body_inline: b"hi".to_vec(),
            body_nodes: Vec::new(),
            client_nonce: "n1".into(),
            edited_seq: 0,
            state: MessageState::Active,
        }
    }

    /// **A live frame maps to the firehose-ONLY token set + the right shed surface (never a durable
    /// token).** Every variant's token is in the firehose-only set or `agent.message.partial`; the
    /// human message rides the protected lane.
    #[test]
    fn live_frame_maps_to_firehose_surface() {
        let m = LiveFrame::HumanMessage {
            message_id: "01J0MSG".into(),
        };
        assert_eq!(m.surface(), LiveSurface::HumanMessage);
        assert!(m.surface().is_protected_human_lane());

        let p = LiveFrame::Presence {
            principal: "alice".into(),
            class: "online".into(),
        };
        assert_eq!(p.surface(), LiveSurface::Speculative);
        assert!(myelin_chat::events::CHAT_FIREHOSE_TOKENS.contains(&p.token()));

        let r = LiveFrame::ReadState {
            principal: "alice".into(),
            up_to_message_id: "01J0MSG".into(),
        };
        assert_eq!(r.surface(), LiveSurface::ReadState);
        assert!(myelin_chat::events::CHAT_FIREHOSE_TOKENS.contains(&r.token()));

        let a = LiveFrame::AgentPartial {
            correlation_id: "run-1".into(),
        };
        assert_eq!(a.surface(), LiveSurface::AgentPartial);
        // the partial is the agent-owned firehose token (not a chat.* durable token).
        assert_eq!(a.token(), "agent.message.partial");
    }

    /// **A human message delivers on the firehose (the live pointer frame) — the durable record is
    /// the truth; the frame is the pointer.** And the in-flight depth advances on the human lane.
    #[test]
    fn deliver_message_publishes_a_human_lane_pointer_frame() {
        let mut firehose = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
        let mut gov = ShedGovernor::new();
        let conv = ConversationId::new("acme", "fr-par", "01J0CHANNEL");
        let scope = scope();
        let msg = human_msg("01J0MSG");

        let t = tenant();
        let mut delivery = LiveDelivery::new(&mut firehose, &mut gov);
        let outcome = delivery.deliver_message(&t, "fan.acme", &scope, &conv, &msg);
        assert!(outcome.is_delivered(), "the human message delivers");
        let frame = outcome.frame().unwrap();
        // references-not-payloads: the frame is a pointer to the durable message id, not the body.
        assert!(frame.payload.0.contains("01J0MSG"));
        assert!(
            !frame.payload.0.contains("hi"),
            "the body is NOT carried inline (references-not-payloads)"
        );
        assert_eq!(gov.in_flight(&t, LiveSurface::HumanMessage), 1);
        // and the scope stayed the bounded channel slice.
        assert_eq!(scope.kind(), ScopeKind::Channel);
    }

    /// **Under shed pressure a presence frame over budget is SHED, while a human message is
    /// DELIVERED — the protected-human-lane shed order, end-to-end through the delivery surface.**
    #[test]
    fn under_pressure_presence_shed_human_delivered() {
        use myelin_substrate::shed::SurfaceBudget;
        let mut firehose = Firehose::with_limits(8192, DEFAULT_INFLIGHT_CAP);
        // a small ConnectionTier budget so the speculative ceiling is quick to reach.
        let conn = SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 2,
            retry_after_secs: 3,
        };
        let agent = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 0,
            retry_after_secs: 10,
        };
        let mut gov = ShedGovernor::with_budgets(conn, agent);
        gov.set_under_pressure(true);
        let t = tenant();
        let scope = scope();

        {
            let mut d = LiveDelivery::new(&mut firehose, &mut gov);
            // fill presence until it sheds (the speculative lane is the first shed under pressure).
            let mut presence_shed = false;
            for _ in 0..16 {
                let o = d.deliver(
                    &t,
                    "fan.acme",
                    &scope,
                    &LiveFrame::Presence {
                        principal: "alice".into(),
                        class: "online".into(),
                    },
                );
                if o == DeliveryOutcome::Shed {
                    presence_shed = true;
                    break;
                }
            }
            assert!(presence_shed, "presence sheds first under pressure");

            // the human message STILL delivers (the protected lane holds, uses the reserved slots).
            let human = d.deliver(
                &t,
                "fan.acme",
                &scope,
                &LiveFrame::HumanMessage {
                    message_id: "01J0MSG".into(),
                },
            );
            assert!(
                human.is_delivered(),
                "the human lane holds while presence sheds"
            );
        }
    }

    /// **Firehose-ONLY by construction: the only transport in scope is the ephemeral firehose — there
    /// is no durable-bus handle, so a live frame cannot reach the durable bus.** (The structural
    /// no-raw-publish proof: the delivered frame is a firehose `Frame` with a `seq`, the firehose's
    /// resume cursor — never an outbox-emitted durable event.)
    #[test]
    fn delivered_frame_is_a_firehose_frame_not_a_durable_event() {
        let mut firehose = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
        let mut gov = ShedGovernor::new();
        let scope = scope();
        let t = tenant();
        let mut d = LiveDelivery::new(&mut firehose, &mut gov);
        let outcome = d.deliver(
            &t,
            "fan.acme",
            &scope,
            &LiveFrame::HumanMessage {
                message_id: "01J0MSG".into(),
            },
        );
        // the outcome carries a firehose Frame with a per-(stream,scope) seq (the resume cursor) —
        // the ephemeral transport's shape, never a durable EventEnvelope.
        let frame = outcome.frame().expect("delivered");
        assert!(
            frame.seq >= 1,
            "a firehose frame carries the resume-cursor seq"
        );
    }
}
