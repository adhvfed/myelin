#![forbid(unsafe_code)]

pub mod delivery;
pub mod shed;
pub mod surge;

pub use delivery::{DeliveryOutcome, LiveDelivery, LiveFrame};
pub use shed::{LiveSurface, ShedGovernor, ShedVerdict};
pub use surge::{
    run_chat_surge, surge_governor_from_thresholds, ChatSurgeReport, CHAT_SURGE_MULTIPLIER,
};

use myelin_chat::glue::{chat_channel_scope, Te21LanguagePin, CHAT_FIREHOSE_STREAM_PREFIX};
use myelin_chat::membership::permissions;
use myelin_chat::{ConversationId, MembershipGate, MessageId, MessageStore};
use myelin_events::{Firehose, FirehoseError, FirehoseScope, FirehoseSubscription, Frame};
use myelin_identity::{Credential, IdentityService, Principal};
use myelin_substrate::metrics_health::{DependencyHealth, MetricsHealthSurface, Readiness};

pub fn channel_stream(tenant: &str) -> String {
    format!("{CHAT_FIREHOSE_STREAM_PREFIX}.{tenant}")
}

#[derive(Debug)]
pub enum GatewayError {
    NotReady,
    Unauthenticated(String),
    NotAMember(String),
    Firehose(FirehoseError),
    SnapshotFailed(String),
}

impl core::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GatewayError::NotReady => write!(
                f,
                "gateway not ready - shedding the new connection (liveness != readiness, 1.3)"
            ),
            GatewayError::Unauthenticated(e) => {
                write!(f, "authenticate refused the credential: {e}")
            }
            GatewayError::NotAMember(ch) => {
                write!(
                    f,
                    "principal is not a member of channel `{ch}` (read gate, fail-closed)"
                )
            }
            GatewayError::Firehose(error) => {
                write!(f, "live transport refused the request: {error}")
            }
            GatewayError::SnapshotFailed(e) => write!(f, "resync_from snapshot read failed: {e}"),
        }
    }
}

impl std::error::Error for GatewayError {}

#[derive(Clone, Debug)]
pub struct Connection {
    pub principal: Principal,
    pub stream: String,
}

impl Connection {
    pub fn tenant(&self) -> &str {
        self.principal.tenant.0.as_str()
    }
}

pub enum ResumeOutcome {
    Live {
        backfill: Vec<Frame>,
        sub: FirehoseSubscription,
    },
    Resync {
        snapshot: Vec<myelin_chat::Message>,
        sub: FirehoseSubscription,
    },
}

pub struct ChatGateway<I, S, H>
where
    I: IdentityService + Clone,
    S: MessageStore,
    H: DependencyHealth,
{
    gate: MembershipGate<I>,
    id: I,
    store: S,
    firehose: Firehose,
    health: MetricsHealthSurface<H>,
    shed: ShedGovernor,
}

impl<I, S, H> ChatGateway<I, S, H>
where
    I: IdentityService + Clone,
    S: MessageStore,
    H: DependencyHealth,
{
    pub fn new(id: I, store: S, firehose: Firehose, health: MetricsHealthSurface<H>) -> Self {
        ChatGateway {
            gate: MembershipGate::new(id.clone()),
            id,
            store,
            firehose,
            health,
            shed: ShedGovernor::new(),
        }
    }

    pub fn shed(&self) -> &ShedGovernor {
        &self.shed
    }

    pub fn shed_mut(&mut self) -> &mut ShedGovernor {
        &mut self.shed
    }

    pub fn set_shed_governor(&mut self, shed: ShedGovernor) {
        self.shed = shed;
    }

    pub fn live_delivery(&mut self) -> LiveDelivery<'_> {
        LiveDelivery::new(&mut self.firehose, &mut self.shed)
    }

    pub fn readiness(&self) -> Readiness {
        self.health.readiness().verdict
    }

    pub fn health(&self) -> &MetricsHealthSurface<H> {
        &self.health
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn firehose_mut(&mut self) -> &mut Firehose {
        &mut self.firehose
    }

    pub fn connect(&self, credential: &Credential) -> Result<Connection, GatewayError> {
        if self.readiness().sheds() {
            return Err(GatewayError::NotReady);
        }
        let principal = self
            .id
            .authenticate(credential)
            .map_err(|e| GatewayError::Unauthenticated(format!("{e:?}")))?;
        let stream = channel_stream(principal.tenant.0.as_str());
        Ok(Connection { principal, stream })
    }

    pub fn subscribe(
        &mut self,
        conn: &Connection,
        channel: &ConversationId,
        at_zookie: Option<&str>,
        cursor: Option<u64>,
    ) -> Result<FirehoseSubscription, GatewayError> {
        self.gate
            .check_channel(&conn.principal, permissions::READ, channel, at_zookie)
            .map_err(|_| GatewayError::NotAMember(channel.conversation_id.clone()))?;
        let scope = self.bounded_scope(channel)?;
        self.firehose
            .subscribe(&conn.stream, &scope, cursor)
            .map_err(GatewayError::Firehose)
    }

    pub fn resume(
        &mut self,
        conn: &Connection,
        channel: &ConversationId,
        at_zookie: Option<&str>,
        last_seq: u64,
        snapshot_cursor: &MessageId,
    ) -> Result<ResumeOutcome, GatewayError> {
        self.gate
            .check_channel(&conn.principal, permissions::READ, channel, at_zookie)
            .map_err(|_| GatewayError::NotAMember(channel.conversation_id.clone()))?;
        let scope = self.bounded_scope(channel)?;
        match self.firehose.resume(&conn.stream, &scope, last_seq) {
            Ok(sub) => {
                let backfill = sub.drain_ready();
                Ok(ResumeOutcome::Live { backfill, sub })
            }
            Err(FirehoseError::ResyncRequired { .. } | FirehoseError::TailLimitExceeded) => {
                self.snapshot_resync(conn, channel, snapshot_cursor, &scope)
            }
            Err(error) => Err(GatewayError::Firehose(error)),
        }
    }

    fn snapshot_resync(
        &mut self,
        conn: &Connection,
        channel: &ConversationId,
        snapshot_cursor: &MessageId,
        scope: &FirehoseScope,
    ) -> Result<ResumeOutcome, GatewayError> {
        let snapshot = self
            .store
            .resync_from(channel, snapshot_cursor)
            .map_err(|e| GatewayError::SnapshotFailed(e.to_string()))?;
        let sub = self
            .firehose
            .subscribe(&conn.stream, scope, None)
            .map_err(GatewayError::Firehose)?;
        Ok(ResumeOutcome::Resync { snapshot, sub })
    }

    fn bounded_scope(&self, channel: &ConversationId) -> Result<FirehoseScope, GatewayError> {
        chat_channel_scope(&channel.conversation_id).map_err(GatewayError::Firehose)
    }
}

pub fn te21_pin() -> Te21LanguagePin {
    let pin = Te21LanguagePin::PINNED;
    debug_assert!(
        pin.is_no_op(),
        "the gateway connection tier is Rust - the 1.7 cross-language harness shim is a NO-OP (the BEAM hatch is closed)"
    );
    pin
}
