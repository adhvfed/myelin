use myelin_chat::{AuthorKind, ConversationId, Message};
use myelin_events::{Firehose, FirehoseError, FirehoseScope, Frame, FrameDraft};
use myelin_identity::PrincipalId;
use myelin_tenancy::TenantId;

use crate::shed::{LiveSurface, ShedGovernor, ShedVerdict};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveFrame {
    HumanMessage {
        message_id: String,
    },
    AgentPartial {
        correlation_id: String,
    },
    Presence {
        principal: String,
        class: String,
    },
    Typing {
        principal: String,
        started: bool,
    },
    ReadState {
        principal: String,
        up_to_message_id: String,
    },
}

impl LiveFrame {
    pub fn surface(&self) -> LiveSurface {
        match self {
            LiveFrame::HumanMessage { .. } => LiveSurface::HumanMessage,
            LiveFrame::AgentPartial { .. } => LiveSurface::AgentPartial,
            LiveFrame::Presence { .. } => LiveSurface::Speculative,
            LiveFrame::Typing { .. } => LiveSurface::Typing,
            LiveFrame::ReadState { .. } => LiveSurface::ReadState,
        }
    }

    pub fn token(&self) -> &'static str {
        match self {
            LiveFrame::HumanMessage { .. } => myelin_chat::events::CHAT_MESSAGE_CREATED,
            LiveFrame::AgentPartial { .. } => "agent.message.partial",
            LiveFrame::Presence { .. } => myelin_chat::events::CHAT_PRESENCE_CHANGED,
            LiveFrame::Typing { started: true, .. } => myelin_chat::events::CHAT_TYPING_STARTED,
            LiveFrame::Typing { started: false, .. } => myelin_chat::events::CHAT_TYPING_STOPPED,
            LiveFrame::ReadState { .. } => myelin_chat::events::CHAT_READ_STATE_VIEWED,
        }
    }

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

    pub fn from_message(msg: &Message) -> LiveFrame {
        match msg.author_kind {
            AuthorKind::Human | AuthorKind::Service => LiveFrame::HumanMessage {
                message_id: msg.message_id.0.clone(),
            },
            AuthorKind::Agent => LiveFrame::AgentPartial {
                correlation_id: msg.message_id.0.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Delivered(Frame),
    Shed,
}

impl DeliveryOutcome {
    pub fn is_delivered(&self) -> bool {
        matches!(self, DeliveryOutcome::Delivered(_))
    }
    pub fn frame(&self) -> Option<&Frame> {
        match self {
            DeliveryOutcome::Delivered(f) => Some(f),
            DeliveryOutcome::Shed => None,
        }
    }
}

pub struct LiveDelivery<'a> {
    firehose: &'a mut Firehose,
    governor: &'a mut ShedGovernor,
}

impl<'a> LiveDelivery<'a> {
    pub fn new(firehose: &'a mut Firehose, governor: &'a mut ShedGovernor) -> LiveDelivery<'a> {
        LiveDelivery { firehose, governor }
    }

    pub fn governor_mut(&mut self) -> &mut ShedGovernor {
        self.governor
    }

    pub fn deliver(
        &mut self,
        tenant: &TenantId,
        stream: &str,
        scope: &FirehoseScope,
        frame: &LiveFrame,
    ) -> Result<DeliveryOutcome, FirehoseError> {
        let surface = frame.surface();
        match self.governor.admit(tenant, surface) {
            ShedVerdict::Shed { .. } => Ok(DeliveryOutcome::Shed),
            ShedVerdict::Deliver => {
                let published =
                    self.firehose
                        .publish(stream, scope, FrameDraft::new(frame.payload_pointer()));
                let published = match published {
                    Ok(frame) => frame,
                    Err(error) => {
                        self.governor.on_drained(tenant, surface);
                        return Err(error);
                    }
                };
                Ok(DeliveryOutcome::Delivered(published))
            }
        }
    }

    pub fn deliver_message(
        &mut self,
        tenant: &TenantId,
        stream: &str,
        scope: &FirehoseScope,
        _conv: &ConversationId,
        msg: &Message,
    ) -> Result<DeliveryOutcome, FirehoseError> {
        let frame = LiveFrame::from_message(msg);
        self.deliver(tenant, stream, scope, &frame)
    }

    pub fn deliver_presence(
        &mut self,
        tenant: &TenantId,
        stream: &str,
        scope: &FirehoseScope,
        principal: &PrincipalId,
        class: &str,
    ) -> Result<DeliveryOutcome, FirehoseError> {
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
        assert_eq!(a.token(), "agent.message.partial");
    }

    #[test]
    fn deliver_message_publishes_a_human_lane_pointer_frame() {
        let mut firehose = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
        let mut gov = ShedGovernor::new();
        let conv = ConversationId::new("acme", "fr-par", "01J0CHANNEL");
        let scope = scope();
        let msg = human_msg("01J0MSG");

        let t = tenant();
        let mut delivery = LiveDelivery::new(&mut firehose, &mut gov);
        let outcome = delivery
            .deliver_message(&t, "fan.acme", &scope, &conv, &msg)
            .expect("the bounded frame publishes");
        assert!(outcome.is_delivered(), "the human message delivers");
        let frame = outcome.frame().unwrap();
        assert!(frame.payload.0.contains("01J0MSG"));
        assert!(
            !frame.payload.0.contains("hi"),
            "the body is NOT carried inline (references-not-payloads)"
        );
        assert_eq!(gov.in_flight(&t, LiveSurface::HumanMessage), 1);
        assert_eq!(scope.kind(), ScopeKind::Channel);
    }

    #[test]
    fn under_pressure_presence_shed_human_delivered() {
        use myelin_substrate::shed::SurfaceBudget;
        let mut firehose = Firehose::with_limits(8192, DEFAULT_INFLIGHT_CAP);
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
            let mut presence_shed = false;
            for _ in 0..16 {
                let o = d
                    .deliver(
                        &t,
                        "fan.acme",
                        &scope,
                        &LiveFrame::Presence {
                            principal: "alice".into(),
                            class: "online".into(),
                        },
                    )
                    .expect("the bounded frame publishes");
                if o == DeliveryOutcome::Shed {
                    presence_shed = true;
                    break;
                }
            }
            assert!(presence_shed, "presence sheds first under pressure");

            let human = d
                .deliver(
                    &t,
                    "fan.acme",
                    &scope,
                    &LiveFrame::HumanMessage {
                        message_id: "01J0MSG".into(),
                    },
                )
                .expect("the bounded frame publishes");
            assert!(
                human.is_delivered(),
                "the human lane holds while presence sheds"
            );
        }
    }

    #[test]
    fn delivered_frame_is_a_firehose_frame_not_a_durable_event() {
        let mut firehose = Firehose::with_limits(4096, DEFAULT_INFLIGHT_CAP);
        let mut gov = ShedGovernor::new();
        let scope = scope();
        let t = tenant();
        let mut d = LiveDelivery::new(&mut firehose, &mut gov);
        let outcome = d
            .deliver(
                &t,
                "fan.acme",
                &scope,
                &LiveFrame::HumanMessage {
                    message_id: "01J0MSG".into(),
                },
            )
            .expect("the bounded frame publishes");
        let frame = outcome.frame().expect("delivered");
        assert!(
            frame.seq >= 1,
            "a firehose frame carries the resume-cursor seq"
        );
    }

    #[test]
    fn a_refused_frame_releases_its_admission_slot() {
        let mut firehose = Firehose::new();
        let scope = scope();
        firehose
            .seed_head("fan.acme", &scope, u64::MAX - 1)
            .expect("the fixture leaves room for one live frame");
        let mut governor = ShedGovernor::new();
        let tenant = tenant();
        let frame = LiveFrame::HumanMessage {
            message_id: "01J0MSG".into(),
        };

        let mut delivery = LiveDelivery::new(&mut firehose, &mut governor);
        delivery
            .deliver(&tenant, "fan.acme", &scope, &frame)
            .expect("the final sequence publishes");
        assert_eq!(
            delivery
                .governor_mut()
                .in_flight(&tenant, LiveSurface::HumanMessage),
            1
        );

        let error = delivery
            .deliver(&tenant, "fan.acme", &scope, &frame)
            .expect_err("the cursor cannot wrap");
        assert_eq!(
            error,
            FirehoseError::SequenceExhausted { last_seq: u64::MAX }
        );
        assert_eq!(
            delivery
                .governor_mut()
                .in_flight(&tenant, LiveSurface::HumanMessage),
            1,
            "a frame that never entered the transport owns no admission slot"
        );
    }
}
