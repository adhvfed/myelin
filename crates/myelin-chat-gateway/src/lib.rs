//! # `myelin-chat-gateway` — the stateless Rust connection-tier gateway
//! (CHAT-P9 / P-403, M4-C2): subscribe / resume / resync_required — the zero-loss-across-reconnect
//! backbone.
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/02-internals-and-algorithms.md` §1 (the
//! connection tier over the FROZEN firehose resume-cursor protocol) + §1.3 (the resume-cursor
//! resync — the correctness backbone, zero-loss-across-reconnect) and `01-tech-and-data-model.md`
//! (the gateway is **stateless**: live sockets + presence + resume cursors only — NO durable store,
//! NO outbox of its own) + `03-events-contracts-and-glue.md` §9 (the gateway has **no emit path** —
//! it calls the Message Service). Reconciliation: `00-reconciliation-decisions.md` OQ-J (the one
//! resume-cursor protocol: `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)`
//! backfills `(last_seq, now]`; `resync_required -> *.snapshot`; scope a BOUNDED selector, never `*`).
//!
//! ## What this crate is (the gateway-shell + the resume-cursor slice of M4-C2)
//! The gateway is the platform's connection tier — where the worst load manifests (connection
//! storms, mega-channels, agent-generated fan-out, arch §1.4). It is **STATELESS**: it holds live
//! sockets + presence + resume cursors only, and it COMPOSES the frozen pieces rather than
//! re-implementing any of them (EI-01 §7 — one shape, no second copy):
//!
//! - **The firehose resume-cursor transport** ([`myelin_events::Firehose`], contract 3.5) — the
//!   gateway opens a per-channel subscription on a BOUNDED `channel:<id>` scope and resumes the gap
//!   `(last_seq, now]` on a reconnect, losing ZERO ops. The firehose owns the seq + the window + the
//!   `*`-rejection; the gateway never mints a seq and never invents a second scope validator.
//! - **The `resync_required -> *.snapshot` fallback** ([`myelin_chat::MessageStore::resync_from`],
//!   contract 2.6) — when a reconnect's `last_seq` is OLDER than the firehose retention window the
//!   transport raises `resync_required`; the gateway falls back to the durable store's
//!   `resync_from(conversation, cursor)` snapshot (a gap-free clustering-range read) and then
//!   resumes live. The cold-rebuild path is **NAMED, not silent**.
//! - **The connect-time read gate** ([`myelin_chat::MembershipGate`], contract 4.2) — a connection
//!   may only subscribe to channels it is a member of; the gate is `Id.check`-backed + fail-closed.
//! - **`authenticate(credential) -> Principal`** (contract 4.1) — the handshake resolves a
//!   Principal; **tenant comes from the verified token, never the path** (ID-3). The gateway trusts
//!   the injected identity for IDENTITY and re-authorizes nothing it is not responsible for.
//! - **The readiness gate** ([`myelin_substrate::metrics_health`], contract 1.3) — a connection is
//!   admitted only when the gateway is `Ready`; a dead critical dependency reports `NotReady` and
//!   the gateway SHEDS new connections (liveness never restart-storms under a connection storm).
//! - **The TE-21 cross-language harness shim** (contract 1.7) — satisfied as a NO-OP (the connection
//!   tier is in-process Rust); the gateway speaks the Rust `EventEnvelope`/`Frame` on the wire
//!   regardless. See [`myelin_chat::glue::Te21LanguagePin`].
//!
//! ## FLOORS named (VISION §3 name-your-floors)
//! - **Connection-tier language = Rust** (TE-21 default). The BEAM/Phoenix hatch is
//!   written-but-CLOSED, bounded by the 1.7 harness shim ([`te21_pin`]); it is opened only if
//!   CHAT-D3/D4 prove Rust presence-at-scale intractable — a gateway-process swap, not a platform
//!   rewrite (**CHAT-P26**).
//! - **Mega-channel live delivery = firehose subject fan-out with per-view scope bounding.** The
//!   channel-sharded home-node (the Phoenix/Discord guild model, Rust + consistent-hash) is the
//!   named M5 escalation (**M5-C-S3 / CHAT-P29 / P-503**), promoted on a measured subscriber count
//!   exceeding the subject-fan-out budget. Until measured, the subject model is the design (ADR-10).
//!   The floor's measurable trigger predicate ([`SubjectFanOutBudget::exceeded_by`]) + its dated
//!   gap-report row ([`home_node_floor_gap_report`]) live in [`home_node`]; at this prompt's execution
//!   the trigger has NOT fired (the surge family measured shed budgets, not subscriber fan-out), so
//!   the home-node is a named floor, not built.
//! - **The firehose-only LIVE delivery body + the protected-human-lane shed order is CHAT-P10**
//!   (**P-404**) — this crate is the gateway SHELL + the resume-cursor live tier (subscribe / resume
//!   / resync); the message/presence/typing/read-state/partials delivery surface + the per-surface
//!   shed budgets land there. The readiness/shed-order surface here is the gating primitive that
//!   CHAT-P10's shed order rides.
//! - **The real broker binding** (the JetStream-class firehose transport in prod) is the Bus M0
//!   deployment seam (`relay::BusTransport`, P-S12) — here the in-process [`myelin_events::Firehose`]
//!   is the unit/drill transport; the protocol shape it implements is the frozen §5.5 surface.

#![forbid(unsafe_code)]

pub mod delivery;
pub mod home_node;
pub mod shed;
pub mod surge;

pub use delivery::{DeliveryOutcome, LiveDelivery, LiveFrame};
pub use home_node::{
    home_node_floor_gap_report, MeasuredFanOut, SubjectFanOutBudget, BEAM_GATEWAY_SIBLING_FLOOR,
    GATEWAY_MEASURED_TRIGGER_FLOORS, HOME_NODE_FLOOR,
};
pub use shed::{LiveSurface, ShedGovernor, ShedVerdict};
pub use surge::{
    run_chat_surge, surge_governor_from_thresholds, ChatSurgeReport, CHAT_SHED_BUDGETS_TUNED,
    CHAT_SURGE_MULTIPLIER, COMMENT_CONSOLIDATION_FOLLOW_ON, CROSS_ORG_FOLLOW_ON,
    HOME_NODE_FOLLOW_ON, SCYLLA_HOT_TIER_FOLLOW_ON,
};

use myelin_chat::glue::{chat_channel_scope, Te21LanguagePin, CHAT_FIREHOSE_STREAM_PREFIX};
use myelin_chat::membership::permissions;
// The chat subsystem's PUBLIC contract surface (the top-level re-exports — never the private
// `::store` data path; the gateway is chat's OWN connection tier reading chat's OWN store through
// its public API, the proper subsystem-internal coupling, ADR-01).
use myelin_chat::{ConversationId, MembershipGate, MessageId, MessageStore};
use myelin_events::{Firehose, FirehoseError, FirehoseScope, FirehoseSubscription, Frame};
use myelin_identity::{Credential, IdentityService, Principal};
use myelin_substrate::metrics_health::{DependencyHealth, MetricsHealthSurface, Readiness};

/// **The firehose `stream` a tenant's channels live-deliver over (contract 3.5 / arch §1.2:
/// `fan.<tenant>.<channel>`).** The per-tenant stream prefix is the frozen
/// [`CHAT_FIREHOSE_STREAM_PREFIX`] (`fan`); the gateway fills the `<tenant>` from the connection's
/// verified Principal (ID-3 — tenant from the token, never the path), and the `channel:<id>` SCOPE
/// narrows the stream to one channel's slice. Built here so the stream NAME is derived, never a
/// literal (X-5 — names anchor).
pub fn channel_stream(tenant: &str) -> String {
    format!("{CHAT_FIREHOSE_STREAM_PREFIX}.{tenant}")
}

/// **Why a connection / subscribe / resume failed at the gateway (the typed, LOUD verdicts — never a
/// silent fail).** The gateway is a chokepoint; every refusal is a value the connection tier turns
/// into a close/`4xx`, never a silent admit (EI-01 §3 prove-it).
#[derive(Debug)]
pub enum GatewayError {
    /// The gateway is `NotReady` (a dead critical dependency or an incomplete boot) — the connection
    /// is SHED, never served on a not-ready instance (liveness != readiness, contract 1.3). The
    /// client retries against a ready instance.
    NotReady,
    /// `authenticate` refused the credential (an unresolved / disabled / revoked principal, contract
    /// 4.1). The handshake is rejected; no socket opens. Carries the underlying Id error string.
    Unauthenticated(String),
    /// The principal is not a member of the channel it tried to subscribe to (the connect-time read
    /// gate, contract 4.2 — fail-closed). NO field of the channel is read on a denial (the leak-free
    /// chokepoint). Carries the channel id.
    NotAMember(String),
    /// The subscription scope was over-broad / `*` / unbounded — the transport REJECTS it (the
    /// whitelist-not-`*` rule, BUS-3, generalised, contract 3.5). The gateway never opens an
    /// unbounded subscription.
    OverBroadScope(FirehoseError),
    /// The reconnect cursor is OLDER than the firehose retention window — the gap cannot be
    /// backfilled from the window, so the client MUST fall back to the `*.snapshot` resync. This is
    /// NOT a terminal error: it is the **named** signal the gateway turns into a [`ResumeOutcome::Resync`]
    /// (a snapshot resync via [`MessageStore::resync_from`]). It only surfaces as an error when the
    /// caller asked for a raw firehose-only resume without the snapshot fallback wired.
    ResyncRequired {
        /// The `last_seq` the client presented.
        last_seq: u64,
        /// The oldest seq the retention window still holds (the window floor).
        window_floor: u64,
    },
    /// The durable store's `resync_from` snapshot read failed (the cold-rebuild path itself errored).
    /// Carries the store error string. LOUD — a snapshot failure is never a silent partial replay.
    SnapshotFailed(String),
}

impl core::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GatewayError::NotReady => write!(
                f,
                "gateway not ready — shedding the new connection (liveness != readiness, 1.3)"
            ),
            GatewayError::Unauthenticated(e) => write!(f, "authenticate refused the credential: {e}"),
            GatewayError::NotAMember(ch) => {
                write!(f, "principal is not a member of channel `{ch}` (read gate, fail-closed)")
            }
            GatewayError::OverBroadScope(e) => write!(f, "over-broad subscription scope: {e}"),
            GatewayError::ResyncRequired { last_seq, window_floor } => write!(
                f,
                "resync_required: last_seq={last_seq} older than the retention window (floor={window_floor}) \
                 → *.snapshot resync (resync_from)"
            ),
            GatewayError::SnapshotFailed(e) => write!(f, "resync_from snapshot read failed: {e}"),
        }
    }
}

impl std::error::Error for GatewayError {}

/// **A live connection's resolved identity (the stateless gateway holds only this + its open
/// subscriptions).** The gateway resolves a [`Principal`] from the handshake credential
/// (`authenticate`, ID-3 — tenant from the token); it persists NOTHING durable. The connection's
/// `tenant` is the firehose stream key half (`fan.<tenant>.…`), taken from the VERIFIED principal,
/// never a path/header the client controls.
#[derive(Clone, Debug)]
pub struct Connection {
    /// The resolved principal (the identity the gateway trusts for IDENTITY only — per-action authz
    /// is the membership gate's job, never the gateway's).
    pub principal: Principal,
    /// The firehose stream this connection's subscriptions ride (`fan.<tenant>`), derived from the
    /// principal's verified tenant (ID-3).
    pub stream: String,
}

impl Connection {
    /// The verified tenant (the firehose stream key half) — taken from the principal, never the path.
    pub fn tenant(&self) -> &str {
        self.principal.tenant.0.as_str()
    }
}

/// **The outcome of a `resume` (arch §1.3 — the resume-cursor resync ladder).** A reconnect either
/// recovers the gap from the bounded firehose window (the common, cheap path — ZERO ops lost) OR,
/// when the cursor is older than the window, falls back to the durable `*.snapshot` resync (the
/// NAMED cold-rebuild path, contract 2.6). Either way the client loses nothing — the property the
/// CHAT-D1 drill pins (0 lost, 0 dup).
pub enum ResumeOutcome {
    /// The gap was recovered from the firehose retention window: the backfilled frames `(last_seq,
    /// now]` followed by a live [`FirehoseSubscription`] (contiguous — no gap, no duplicate).
    Live {
        /// The backfilled gap frames `(last_seq, now]`, oldest-first.
        backfill: Vec<Frame>,
        /// The live subscription that continues from the gap's head (strictly-newer frames only).
        sub: FirehoseSubscription,
    },
    /// The cursor was older than the window — the gateway fell back to the durable `*.snapshot`
    /// resync ([`MessageStore::resync_from`]): the gap-free, ordered snapshot of everything in the
    /// conversation strictly after the snapshot cursor, followed by a fresh LIVE subscription from
    /// the firehose head. The client re-renders from the snapshot then resumes live (idempotent on
    /// `message_id` — a re-streamed message is a client-side no-op).
    Resync {
        /// The durable snapshot (everything after `snapshot_cursor`, gap-free, ordered) — the
        /// cold-rebuild the over-window client re-renders from.
        snapshot: Vec<myelin_chat::Message>,
        /// A fresh LIVE subscription from the firehose head (live continues after the snapshot).
        sub: FirehoseSubscription,
    },
}

/// **The stateless connection-tier gateway (arch §1; the M4-C2 gateway shell + resume-cursor slice,
/// CHAT-P9).** It is generic over the three frozen dependencies it COMPOSES — the
/// [`IdentityService`] (`authenticate` + the gate's `check`), the [`MessageStore`] (the
/// `resync_from` snapshot fallback), and the [`DependencyHealth`] probe the readiness gate reads —
/// and holds the [`Firehose`] transport + the metrics-health surface. It owns **no durable store**
/// and **no outbox**: it subscribes/resumes over the firehose and falls back to the store's snapshot;
/// any WRITE is the Message Service's job (the gateway has no emit path, arch §9).
pub struct ChatGateway<I, S, H>
where
    I: IdentityService + Clone,
    S: MessageStore,
    H: DependencyHealth,
{
    /// The membership read gate (fail-closed; the connect-time `channel.read` check, 4.2). Holds the
    /// Id dependency (`check`).
    gate: MembershipGate<I>,
    /// The Id dependency for `authenticate` (4.1) — the handshake principal resolution. (Held
    /// separately from the gate's `id` because `MembershipGate` does not expose its inner Id.)
    id: I,
    /// The durable store the `resync_required -> *.snapshot` fallback reads `resync_from` against
    /// (contract 2.6). The gateway holds it by reference-semantics; it never WRITES it.
    store: S,
    /// The firehose resume-cursor transport (contract 3.5). Owned so a `subscribe`/`resume`/`publish`
    /// can mutate the window + fan out to open subscriptions.
    firehose: Firehose,
    /// The liveness != readiness surface (contract 1.3) — the gateway readiness-gates new
    /// connections off this. Liveness is never checked here (it is the orchestrator's restart input;
    /// a dependency outage flips READINESS, never liveness — no restart-storm).
    health: MetricsHealthSurface<H>,
    /// **The per-tenant protected-human-lane shed governor (CHAT-P10; ADR-16 / OQ-K / contract
    /// 1.11).** The connection-storm + agent-mention-storm budget chat OWNS: under storm pressure it
    /// sheds speculative/presence first and the human message lane LAST (humans never queue behind
    /// agent runs, VISION §3). Consulted by the live-delivery surface ([`Self::live_delivery`]) before
    /// every firehose publish. Holds NO durable state.
    shed: ShedGovernor,
}

impl<I, S, H> ChatGateway<I, S, H>
where
    I: IdentityService + Clone,
    S: MessageStore,
    H: DependencyHealth,
{
    /// Compose the gateway over its frozen dependencies + the firehose transport + the readiness
    /// surface. The `id` is cloned into the membership gate (the gate needs `check`; the gateway
    /// keeps a handle for `authenticate`) — one Id dependency, no second copy.
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

    /// **The per-tenant protected-human-lane shed governor (read; CHAT-P10 / contract 1.11).** A drill
    /// reads the in-flight depths / the under-pressure flag (the shed-count green artifact); the
    /// connection-storm signal flips pressure via [`Self::shed_mut`].
    pub fn shed(&self) -> &ShedGovernor {
        &self.shed
    }

    /// The shed governor (mutable) — the connection-storm signal flips storm pressure here; a drill
    /// drives the per-surface in-flight depths to prove the shed order holds.
    pub fn shed_mut(&mut self) -> &mut ShedGovernor {
        &mut self.shed
    }

    /// Replace the shed governor (the production form reads the OQ-K budgets from the thresholds file
    /// via [`ShedGovernor::from_thresholds`]; a drill installs a small deterministic budget to drive
    /// the storm boundary). The governor holds no durable state — swapping it is a config swap.
    pub fn set_shed_governor(&mut self, shed: ShedGovernor) {
        self.shed = shed;
    }

    /// **The firehose-ONLY live-delivery surface, shed-order-gated (CHAT-P10; arch §1.2 / §7;
    /// contracts 3.5 / 1.11).** Borrows the firehose transport + the shed governor as a
    /// [`LiveDelivery`]: every live frame (message/presence/typing/read-state/partial) is published on
    /// the EPHEMERAL firehose (never the durable bus) AFTER the protected-human-lane shed order admits
    /// it. The gateway has no emit path (arch §9) — this is the EPHEMERAL pointer fan-out, not a
    /// durable write; the durable `chat.message.created` is the Message Service's outbox-co-committed
    /// write.
    pub fn live_delivery(&mut self) -> LiveDelivery<'_> {
        LiveDelivery::new(&mut self.firehose, &mut self.shed)
    }

    /// The readiness verdict the gateway gates new connections on (contract 1.3). Exposed so a drill
    /// asserts the gateway sheds when not ready (and never restart-storms — liveness is independent).
    pub fn readiness(&self) -> Readiness {
        self.health.readiness().verdict
    }

    /// The metrics-health surface (so a drill marks boot complete / drives a dependency outage and
    /// reads the readiness/liveness-restart survival signals).
    pub fn health(&self) -> &MetricsHealthSurface<H> {
        &self.health
    }

    /// The durable store the gateway reads its `*.snapshot` resync against (contract 2.6). Exposed
    /// for READ (the snapshot fallback) — the gateway NEVER writes it (it has no emit path, arch §9);
    /// the Message Service owns the write path. (`MessageStore` mutators take `&self` via the store's
    /// own interior synchronisation, so this read-handle does not grant the gateway a write seam — a
    /// write still goes through the Message Service's outbox-co-committed `append`, never the gateway.)
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The firehose transport (so the Message Service's feeder side — or a drill — publishes the
    /// rendered message frames the gateway's subscriptions deliver). The gateway itself NEVER
    /// publishes a durable event (it has no emit path, arch §9); this exposes the live backplane the
    /// Message Service feeds, not a gateway write path.
    pub fn firehose_mut(&mut self) -> &mut Firehose {
        &mut self.firehose
    }

    /// **Connect — resolve the handshake credential to a Principal, readiness-gated (arch §1.1 step
    /// 1; contracts 4.1 / 1.3 / ID-3).** First the readiness gate: a `NotReady` gateway SHEDS the
    /// new connection ([`GatewayError::NotReady`]) — it never serves correct traffic on a not-ready
    /// instance (liveness != readiness; a connection storm against a degraded instance sheds, it does
    /// not restart-storm). Then `authenticate(credential) -> Principal`: **tenant comes from the
    /// VERIFIED token, never the path** (ID-3) — the connection's firehose stream key is derived from
    /// the resolved principal's tenant. A refused credential is [`GatewayError::Unauthenticated`]
    /// (no socket opens).
    pub fn connect(&self, credential: &Credential) -> Result<Connection, GatewayError> {
        // Readiness gate FIRST — shed before doing any work on a not-ready instance (1.3).
        if self.readiness().sheds() {
            return Err(GatewayError::NotReady);
        }
        // authenticate -> Principal; tenant from the verified token (ID-3), never the path.
        let principal = self
            .id
            .authenticate(credential)
            .map_err(|e| GatewayError::Unauthenticated(format!("{e:?}")))?;
        let stream = channel_stream(principal.tenant.0.as_str());
        Ok(Connection { principal, stream })
    }

    /// **Subscribe to a channel — bounded scope, membership-gated (arch §1.1 step 2; contracts 3.5 /
    /// 4.2).** The subscription scope is `channel:<id>`, a BOUNDED selector built through
    /// [`chat_channel_scope`] (the `*`-rejecting chokepoint — scope is NEVER `*`, the
    /// `0 unbounded subscriptions` gate). The connect-time READ gate
    /// ([`MembershipGate::check_channel`] at `channel.read`) is enforced FIRST: a non-member is
    /// fail-closed ([`GatewayError::NotAMember`]); NO field of the channel is read on a denial.
    ///
    /// `cursor = None` starts live from now (a fresh viewer joining a hot channel); `cursor =
    /// Some(seq)` is exactly a [`Self::resume`] (backfill the gap then live). Returns the live
    /// [`FirehoseSubscription`] the connection pumps to its socket.
    pub fn subscribe(
        &mut self,
        conn: &Connection,
        channel: &ConversationId,
        at_zookie: Option<&str>,
        cursor: Option<u64>,
    ) -> Result<FirehoseSubscription, GatewayError> {
        // The connect-time READ gate (4.2) — fail-closed; a non-member never gets a subscription.
        self.gate
            .check_channel(&conn.principal, permissions::READ, channel, at_zookie)
            .map_err(|_| GatewayError::NotAMember(channel.conversation_id.clone()))?;
        let scope = self.bounded_scope(channel)?;
        self.firehose
            .subscribe(&conn.stream, &scope, cursor)
            .map_err(GatewayError::OverBroadScope)
    }

    /// **Resume the gap on a reconnect — the zero-loss-across-reconnect backbone (arch §1.3;
    /// contracts 3.5 / 2.6).** Backfills `(last_seq, now]` from the bounded firehose retention window
    /// then goes live ([`ResumeOutcome::Live`]) — losing ZERO ops. If `last_seq` is OLDER than the
    /// window the firehose raises `resync_required`; the gateway falls back to the durable
    /// `*.snapshot` resync ([`MessageStore::resync_from`] from `snapshot_cursor`) and opens a fresh
    /// live subscription ([`ResumeOutcome::Resync`]) — the NAMED cold-rebuild path, never a silent
    /// partial replay. Membership is re-checked (a grant may have been revoked while disconnected —
    /// the new-enemy guard applies to a reconnect too).
    ///
    /// `snapshot_cursor` is the durable [`MessageId`] the client last rendered (the resume cursor in
    /// the message-id space the store's `resync_from` reads after); it is the snapshot anchor for the
    /// over-window case. Idempotency on `message_id` makes the snapshot + the resumed live stream
    /// overlap-safe (a re-streamed message is a client-side no-op).
    pub fn resume(
        &mut self,
        conn: &Connection,
        channel: &ConversationId,
        at_zookie: Option<&str>,
        last_seq: u64,
        snapshot_cursor: &MessageId,
    ) -> Result<ResumeOutcome, GatewayError> {
        // Re-gate on reconnect: a revoked member cannot resume (the new-enemy guard at the gateway).
        self.gate
            .check_channel(&conn.principal, permissions::READ, channel, at_zookie)
            .map_err(|_| GatewayError::NotAMember(channel.conversation_id.clone()))?;
        let scope = self.bounded_scope(channel)?;
        match self.firehose.resume(&conn.stream, &scope, last_seq) {
            Ok(sub) => {
                // The in-window path: the subscription carries the backfilled gap (drained here so
                // the caller sees the recovered gap explicitly) followed by live. ZERO ops lost.
                let backfill = sub.drain_ready();
                Ok(ResumeOutcome::Live { backfill, sub })
            }
            Err(FirehoseError::ResyncRequired { .. }) => {
                // The over-window path: fall back to the durable *.snapshot resync (2.6) — NAMED.
                let snapshot = self
                    .store
                    .resync_from(channel, snapshot_cursor)
                    .map_err(|e| GatewayError::SnapshotFailed(e.to_string()))?;
                // Open a fresh LIVE subscription from the firehose head (live continues after the
                // snapshot; the client de-dupes the overlap on message_id).
                let sub = self
                    .firehose
                    .subscribe(&conn.stream, &scope, None)
                    .map_err(GatewayError::OverBroadScope)?;
                Ok(ResumeOutcome::Resync { snapshot, sub })
            }
            Err(e @ FirehoseError::OverBroadScope { .. }) => Err(GatewayError::OverBroadScope(e)),
        }
    }

    /// Build the BOUNDED `channel:<id>` firehose scope for a channel — the `*`-rejecting chokepoint
    /// ([`chat_channel_scope`]). A subscription is bounded BY CONSTRUCTION (never `*`, never the
    /// tenant firehose) — the `0 unbounded subscriptions` gate. The conversation's `conversation_id`
    /// is the bounded resource id.
    fn bounded_scope(&self, channel: &ConversationId) -> Result<FirehoseScope, GatewayError> {
        chat_channel_scope(&channel.conversation_id).map_err(GatewayError::OverBroadScope)
    }
}

/// **The TE-21 cross-language harness shim pin (contract 1.7 — the connection-tier language call).**
/// The gateway connection tier is **Rust by default**; the BEAM/Phoenix hatch is written-but-CLOSED
/// (opened only if CHAT-D3/D4 prove the Rust connection tier intractable — the rewrite is CHAT-P26).
/// In the all-Rust default the cross-language harness shim is a NO-OP ([`Te21LanguagePin::is_no_op`]
/// is `true`): the gateway is the SAME in-process Rust subsystem, so there is no cross-language
/// boundary for the shim to enforce; it speaks the Rust `Frame`/`EventEnvelope` on the wire
/// regardless. Recorded here at the gateway (the site the pin actually governs), reusing the FROZEN
/// [`Te21LanguagePin`] from the chat M2-C0 glue — chat declares NO second TE-21 pin (EI-01 §7).
pub fn te21_pin() -> Te21LanguagePin {
    let pin = Te21LanguagePin::PINNED;
    debug_assert!(
        pin.is_no_op(),
        "the gateway connection tier is Rust — the 1.7 cross-language harness shim is a NO-OP (the BEAM hatch is closed)"
    );
    pin
}
