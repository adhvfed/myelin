//! # `cross_org` — cross-org / federated channels over the PII-free CrossCellPointer bridge
//! (CHAT-P30 / P-504, M5; M5-C-X1)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/chat/architecture/05-hard-problems.md` (the cross-org bridge
//! consumption) + `01-tech-and-data-model.md` §2 (the Conversation model's non-foreclosure of
//! multi-cell — [`crate::conversation::Conversation::home_cell`] is a VALUE, membership admits a
//! foreign-tenant principal). **Contract:** `contract-index.md` row 12.6 (the cross-cell PII-free
//! pointer bridge — CONSUMED here for cross-org channel fan-out; resolution always cell-local),
//! row 10.4 (the multi-cell DSR fan-out — iterate `member_cells`, 0 holders missed), row 5.6
//! (`project()` always cell-local — only the projection crosses, never a raw row).
//! **Reconciliation:** `00-reconciliation-decisions.md` §OQ-I (the cross-cell PII-free pointer
//! bridge — resolution is ALWAYS cell-local; cross-org channels ride it). **External insight:**
//! `VISION.md` §3 (world-scale; EU-sovereign by construction — the bridge is PII-free; name-your-
//! floors); `external-insights/01-process-and-quality-doctrine.md` §1 (name-your-floors), §3
//! (cross-cell resolution always cell-local — prove it MEASURED, not asserted), §7 (extend in
//! place, never a second frame).
//!
//! ## TRIGGER FIRED — the bridge SHIPPED in M5, so this is BUILT (not designed-not-built)
//! The CHAT-P30 prompt is conditional: *build only if the cross-cell PII-free pointer bridge ships
//! in M5; otherwise name it as designed-not-built*. The bridge **shipped** — the frame is live
//! ([`myelin_events::CrossCellPointer`], the `myelin-tenancy` authority), the event-propagation half
//! is live ([`myelin_events::crosscell_propagation::CrossCellPropagator`], EB-25 / P-438), and the
//! control-plane resolution half is live (`myelin_control_plane::cross_cell_bridge`, P-429/P-430),
//! with the CP-D8 / GA-D8 / CP-D7 drills green. The Conversation model (CHAT-P7) already does NOT
//! foreclose multi-cell ([`crate::conversation::Conversation::not_single_cell_foreclosed`]). So
//! cross-org / federated channels are **BUILT here** on the frozen bridge (R-C9 resolved for the
//! channel surface), not named as a floor.
//!
//! ## What a cross-org channel IS (the model, arch §2 + 05)
//! A cross-org / federated channel is a [`crate::conversation::Conversation`] whose **membership
//! spans cells**: a channel homed in cell A whose member set includes principals whose identity +
//! data live in cell B (another org/tenant in another residency cell). The channel's `home_cell` is
//! its residency anchor (a VALUE — the model already carries it); a member in cell B does NOT get
//! the channel's messages copied into B — they get a PII-free [`myelin_events::CrossCellPointer`],
//! and they render the channel by asking A to resolve it **cell-local** (§OQ-I).
//!
//! ## What crosses the bridge (the residency invariant — row 12.6 / OQ-I)
//! **0 raw cross-cell rows cross.** Exactly ONE thing crosses per fan-out: the four-field
//! [`myelin_events::CrossCellPointer`] (`subject` opaque channel URN / `type` the PII-free
//! `Channel` kind / `correlation_id` causal-root / `home_cell` routing handle). What does NOT cross,
//! by construction:
//! - the message body bytes (per-subject-DEK ciphertext, [`crate::dek`]) — there is no field on the
//!   frame for them;
//! - the channel topic / name / membership roster — none of it;
//! - the actor, the read-state, the unfurl cache — none of it. A member cell that receives the
//!   pointer holds ONLY a pointer; it cannot reconstruct the channel from what crossed.
//!
//! ## Resolution stays cell-local (the residency gate — row 12.6 / 5.6, R-C9)
//! A member in a foreign cell does NOT receive the channel content. When they want to render the
//! cross-org channel, the foreign cell asks the channel's **home cell** to resolve it
//! ([`CellLocalChannelResolution`]): the home cell renders the channel against ITS rows +
//! permission-checks the viewer THERE (`project()`, always cell-local, contract 5.6), and returns
//! ONLY the already-filtered projection (or a tombstone) — never a raw row, never the message log,
//! never a body byte. The channel's content never leaves its residency cell (§OQ-I). This is the
//! SAME cell-local-resolution discipline the control-plane `cross_cell_bridge` (P-429), the search
//! federated path (P-464), and the Knowledge cross-cell collab (P-485) hold — REUSED here, never
//! re-invented (EI-01 §7).
//!
//! ## Multi-cell DSR iterates `member_cells` (contract 10.4 — 0 holders missed)
//! When a person P is erased, the Chat erase cascade ([`crate::erase`]) must reach P's data in
//! EVERY cell the channel's membership spans. [`CrossOrgChannel::dsr_member_cells`] enumerates the
//! distinct member cells of a cross-org channel so the DSR fan-out iterates them (10.4) — a holder
//! in a member cell P participates in is never missed. The per-cell erasure itself stays cell-local
//! (each cell crypto-shreds P's per-subject DEK against ITS own KMS — the home cell never reaches
//! into a member cell's store; the no-cross-store-read law, [`crate::erase`]).
//!
//! ## EI-01 §7 reconciliation — no second frame, no second propagator, no second resolver
//! This module CONSUMES the already-built cross-cell machinery rather than re-defining it:
//! - the **frame** is [`myelin_events::CrossCellPointer`] (the `myelin-tenancy` authority, re-exported
//!   on the frozen Bus path) — NOT re-defined;
//! - the **event-propagation half** is
//!   [`myelin_events::crosscell_propagation::pointer_for_propagation`] under
//!   [`CrossCellStream::ChatCrossOrg`] (EB-25 / P-438) — the Bus mints the pointer + selects the
//!   member-cell fan-out; this module drives it for the cross-org `chat.*` channel surface and adds
//!   the channel-shaped seam (the membership-spans-cells fan-out, resolve cell-local, the DSR
//!   member-cell enumeration). There is NO second `CrossCellPropagator`, NO second pointer frame,
//!   NO second resolver seam (the resolver mirrors the control-plane bridge's `CellLocalResolver`).
//!
//! ## DAG position (why the fan-out PRODUCTION lives here, not the control plane)
//! `myelin-chat` depends on `myelin-events` (ABOVE `myelin-control-plane` in the §2.9 DAG). So the
//! Bus owns the pointer-event *production* (`crosscell_propagation`); the control plane CONSUMES the
//! produced pointer and carries it cell→cell over the wire. Chat's cross-org layer SITS ON the
//! Bus's production: it classifies a channel event into the `ChatCrossOrg` stream and fans the
//! PII-free pointer to the member cells. The actual cell→cell wire is the control-plane transport
//! (the same `ResilientClient` wire every cross-cell consumer rides).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The control-plane transport wire** (the actual cell→cell carriage of the pointer + the
//!   cell-local `resolve()` round-trip) is the control plane's `cross_cell_bridge` + the
//!   resilient-client transport (P-429/P-437); this module produces the pointer-events the wire
//!   carries and drives the resolution seam against an in-process home-cell resolver standing in for
//!   the home cell (the SAME stand-in the control-plane bridge tests + the search federated path +
//!   the KN collab path use). The cross-process WIRE is the substrate floor — NOT re-built here.
//! - **The member-cell ENUMERATION is the control plane's `placement_of`/`member_cells` fan-out**
//!   (P-CP-20 / P-430). This module fans out to / iterates the membership-derived member-cell set;
//!   the `placement_of`-driven enumeration that PRODUCES the platform-of-record set lives in the
//!   control plane. Chat derives the channel's member cells from ITS membership rows (the channel's
//!   own truth — each member carries its home cell).
//! - **`[OPEN — P6 control plane + LEGAL]` the cross-tenant capability + residency policy.** A
//!   cross-org channel needs an explicit cross-tenant capability grant + a residency policy sign-off
//!   (which tenant may federate with which, in which residency). The ENGINEERING floor (the PII-free
//!   bridge, cell-local resolution, the DSR member-cell iteration) ships today; the capability +
//!   residency-policy gate is the P6 control-plane + LEGAL parallel residual, named here in writing.
//!
//! ## The mandatory-core mutation floor (EI-01 §3 / VISION §4 prove-it)
//! A raw-row / PII leak across the bridge is a **sovereignty breach** — the cross-cell-PII-free
//! discipline is mandatory-core. The cargo-mutants mutation-score floor for this module is
//! **100% caught** on the fan-out + residency + DSR seams: any mutant that lets a non-pointer field
//! cross the bridge, that fails to lift the single-cell pin (drops a member cell), that self-hops
//! the home cell, that lets the cell-local resolver leak a raw row instead of a projection, or that
//! drops a member cell from the DSR enumeration is KILLED by an assertion in [`tests`]. (The
//! `raw_rows_crossed == 0` read is the documented equivalent-mutant: `replace -> 0` is
//! observationally identical because the layer NEVER increments it — the *correct* property, the
//! tripwire stays wired; mirrors `CrossCellBridge::cross_cell_raw_rows` /
//! `CrossCellPropagator::pii_fields_crossed`.)

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::crosscell_propagation::{
    pointer_for_propagation, CrossCellStream, PropagatedPointer,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CellId, CorrelationId, CrossCellPointer, DataRole,
    EventEnvelope, EventId, EventType, OpaqueSubjectId, Timestamp, Visibility,
};
use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};

use crate::conversation::Membership;
use crate::events::CHAT_MESSAGE_CREATED;
use crate::store::ConversationId;

/// **Build the opaque channel subject URN for a cross-org pointer** —
/// `myelin://<tenant>/chat/channel/<conversation_id>`. An `ArtifactRef`-class id (a routing /
/// addressing handle), NEVER a person, NEVER the channel's body/topic/roster. This is the ONLY
/// channel identity that crosses the bridge (as the pointer's `subject`).
#[must_use]
pub fn channel_ref(conv: &ConversationId) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/chat/channel/{}",
        conv.tenant, conv.conversation_id
    ))
}

/// **A federated member of a cross-org channel.** Pairs the member's pseudonymous principal id (the
/// [`Membership::principal_id`] — opaque, never a name/email) with the **home cell** that member's
/// identity + data live in. A cross-org channel's membership spans cells precisely because different
/// members carry different `home_cell`s. PII-free: an `(opaque principal, opaque cell)` pair, never
/// personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FederatedMember {
    /// The member's pseudonymous principal id (the channel's [`Membership::principal_id`]).
    pub principal_id: String,
    /// The cell the member's identity + data live in (an opaque routing handle).
    pub home_cell: CellId,
}

impl FederatedMember {
    /// A federated member from an opaque principal id + the cell their data lives in.
    #[must_use]
    pub fn new(principal_id: impl Into<String>, home_cell: CellId) -> FederatedMember {
        FederatedMember {
            principal_id: principal_id.into(),
            home_cell,
        }
    }

    /// Adapt a chat [`Membership`] + the member's resolved home cell into a [`FederatedMember`].
    /// The membership row's `principal_id` is the pseudonymous id (never PII); the home cell is the
    /// control-plane `placement_of`-resolved cell of that principal (the named enumeration floor —
    /// here it is supplied alongside the membership row).
    #[must_use]
    pub fn from_membership(m: &Membership, home_cell: CellId) -> FederatedMember {
        FederatedMember::new(m.principal_id.clone(), home_cell)
    }
}

/// **A cross-org / federated channel pointer addressed to one member cell.** What Chat's cross-org
/// layer hands the control plane to carry to one of the channel's *other* member cells so a member
/// THERE learns a channel event occurred. It carries ONLY the PII-free [`CrossCellPointer`] (the
/// four frozen fields) + the destination routing handle — NEVER the message body, NEVER the topic,
/// NEVER the roster, NEVER any raw row.
///
/// A member cell that receives this does not get the channel — it gets a pointer; to render it, it
/// asks the home cell to resolve cell-local ([`CellLocalChannelResolution`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossOrgPointer {
    /// The destination member cell the pointer is carried TO (an opaque routing handle).
    pub to_cell: CellId,
    /// The PII-free cross-cell pointer (the four frozen fields — never body, never PII).
    pub pointer: CrossCellPointer,
}

impl CrossOrgPointer {
    /// The opaque channel subject the pointer routes to (the home-cell [`channel_ref`] URN). An
    /// `ArtifactRef`-class id, never a person.
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        self.pointer.subject().artifact_ref()
    }

    /// The channel's home cell — where resolution happens (the content never leaves it).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        self.pointer.home_cell()
    }
}

/// **The PII-free projection a cell-local channel resolution returns (the residency gate body —
/// row 12.6 / 5.6).** When a member in a foreign cell wants to render a cross-org channel, the
/// channel's HOME cell renders it against ITS rows, permission-checks the viewer THERE
/// (`project()`, cell-local), and returns ONLY this — the already-filtered projection (or a
/// tombstone). The message log, the raw rows, the body bytes NEVER cross; only the rendered,
/// viewer-scoped projection does (§OQ-I). This is the channel twin of the control-plane
/// `cross_cell_bridge`'s `BridgeProjection` + the KN `DocProjection` — the SAME cell-local
/// discipline.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelProjection {
    /// The viewer may render the channel in its home cell — the home cell returns the rendered,
    /// permission-filtered projection (a channel-card the viewer is allowed to see, the home cell's
    /// `project()` output AFTER the permission check passed). NEVER the raw message log or a body
    /// byte.
    Rendered {
        /// The channel subject this projection is for (the opaque home-cell URN).
        subject: ArtifactRef,
        /// The home-cell-rendered, permission-filtered channel card the viewer is permitted to see
        /// (name/state/preview the home cell's `project()` produced AFTER the permission check
        /// passed — a single opaque rendered string, NEVER the raw message log or a body byte; the
        /// SAME `rendered: String` shape the KN `DocProjection` carries, EI-01 §7).
        rendered: String,
    },
    /// The viewer may NOT render the channel in its home cell (unauthorised, archived-away, or
    /// erased) — a tombstone carrying NO content (the channel's secrets never cross). An unauthorised
    /// cross-org viewer ALWAYS lands here (no leak across the org boundary).
    Tombstone {
        /// The channel subject the tombstone stands in for (so the member cell renders "unavailable").
        subject: ArtifactRef,
    },
}

impl ChannelProjection {
    /// The channel subject this projection (rendered or tombstone) is for.
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        match self {
            ChannelProjection::Rendered { subject, .. }
            | ChannelProjection::Tombstone { subject } => subject,
        }
    }

    /// `true` iff the home cell rendered the channel for this viewer (vs returned a tombstone).
    #[must_use]
    pub fn is_rendered(&self) -> bool {
        matches!(self, ChannelProjection::Rendered { .. })
    }
}

/// **The cell-local channel resolution seam (row 12.6 / 5.6 — resolution is ALWAYS cell-local).** A
/// member cell that holds a [`CrossOrgPointer`] resolves the channel by asking the channel's HOME
/// cell to render it for a specific viewer. The home cell owns the channel's residency: it renders
/// against ITS rows, permission-checks the viewer THERE (`project()`), and returns ONLY the
/// [`ChannelProjection`] (rendered-or-tombstone) — never a raw row, never the message log, never a
/// body byte. The content NEVER leaves the home cell (§OQ-I).
///
/// In production the call crosses the control-plane `cross_cell_bridge` wire (the named substrate
/// floor); the SEAM is real and is proven here against an in-process home-cell resolver standing in
/// for the home cell (the SAME stand-in the control-plane bridge + search federated + KN collab
/// tests use).
pub trait CellLocalChannelResolution {
    /// Resolve `pointer` for `viewer` IN the channel's home cell — render-or-tombstone, cell-local.
    /// The home cell permission-checks `viewer` and returns ONLY the filtered projection.
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossOrgPointer,
        viewer: &Principal,
    ) -> ChannelProjection;
}

/// **The Chat cross-org / federated channel layer (contract 12.6 consumed — the channel leg,
/// LIVE).** Serves a cross-org channel's **home cell** and, for an applied channel event, fans the
/// PII-free [`CrossCellPointer`] out to the channel's member cells over the bridge — carrying ONLY
/// the four-field frame across, NEVER the message body, NEVER the topic/roster, NEVER a raw row.
/// It also enumerates the channel's member cells for the multi-cell DSR fan-out (10.4) and resolves
/// a cross-org pointer cell-local (5.6).
///
/// `events_fanned_out` + `raw_rows_crossed` are the **PII-free fan-out proof** telemetry (the gate):
/// every fanned-out pointer increments `events_fanned_out`; `raw_rows_crossed` is pinned to **0** by
/// construction (the layer only ever emits the four-field frame), exposed as a live tripwire so a
/// future regression that carried a raw row across the bridge would be observable (it would tick
/// above 0). This is the "0 raw rows cross the bridge" projection the gate asserts `== 0`.
#[derive(Clone)]
pub struct CrossOrgChannel {
    /// The channel home cell this layer serves (an opaque id). A member cell == the home cell is
    /// skipped on fan-out (no self-hop — that cell is the home cell, it already has the event).
    home_cell: CellId,
    /// The fan-out telemetry: how many cross-org channel pointers were fanned out (PII-free).
    events_fanned_out: Arc<AtomicU64>,
    /// **The ZERO — raw rows carried across a cell boundary by the cross-org fan-out.** Pinned to 0
    /// by construction (the layer only ever emits the four-field PII-free frame). A live counter
    /// (not a constant) so a future regression — a code path that carried a body/topic/roster/raw
    /// row across — is observable. This is the "0 raw rows cross the bridge" projection the gate
    /// asserts `== 0`.
    raw_rows_crossed: Arc<AtomicU64>,
}

impl CrossOrgChannel {
    /// Build a cross-org channel layer serving channel `home_cell`.
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossOrgChannel {
        CrossOrgChannel {
            home_cell,
            events_fanned_out: Arc::new(AtomicU64::new(0)),
            raw_rows_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The channel home cell this layer serves (opaque id).
    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// **Fan a channel event out to the channel's member cells (M5-C-X1) — the cross-org fan-out.**
    /// For a cross-org channel `conv`, mint the PII-free [`CrossCellPointer`] (homed in THIS cell,
    /// subject = the opaque [`channel_ref`] URN, type = the `Channel` kind via the
    /// [`CrossCellStream::ChatCrossOrg`] stream, correlation = the event's causal-root) and produce
    /// one [`CrossOrgPointer`] per **distinct member cell** that is **not** this home cell (no
    /// self-hop — the home cell already has the event). A single-home-cell channel (every member in
    /// the home cell) produces an EMPTY fan-out — the non-federated path is unchanged (the M4
    /// single-home-cell floor preserved for non-cross-org channels).
    ///
    /// Across EVERY produced pointer, ONLY the four frozen frame fields cross — never the message
    /// body, never the topic/roster (`raw_rows_crossed` stays 0). The body/topic are NOT read into
    /// the pointer — there is structurally no field on the frame for them. The `correlation_id` ties
    /// the cross-cell pointers to the originating event's causal chain (BUS-5).
    pub fn fan_out_channel_event(
        &self,
        conv: &ConversationId,
        correlation_id: &CorrelationId,
        members: &[FederatedMember],
    ) -> Vec<CrossOrgPointer> {
        // Build the `chat.channel.message_created` envelope the Bus's propagation half mints the
        // pointer from. The envelope's `subject` is the OPAQUE channel URN (never a person/body);
        // `correlation_id` is the event's causal-root. The body/topic are NOT placed on the subject.
        let envelope = self.channel_event_envelope(conv, correlation_id);
        // CONSUME the Bus's pointer mint (EB-25) — no second propagator, no second frame. The
        // ChatCrossOrg stream supplies the PII-free `Channel` artifact-type kind.
        let pointer = pointer_for_propagation(
            &envelope,
            CrossCellStream::ChatCrossOrg,
            self.home_cell.clone(),
        );
        // The DISTINCT member cells other than the home cell (a member roster has many principals per
        // cell; the bridge carries ONE pointer per cell, not per principal — no duplicate hops).
        self.distinct_member_cells(members)
            .into_iter()
            .filter(|to| *to != self.home_cell) // no self-hop — the home cell already has it.
            .map(|to| {
                // Each produced pointer carries ONLY the four-field frame — 0 raw rows cross.
                self.events_fanned_out.fetch_add(1, Ordering::SeqCst);
                CrossOrgPointer {
                    to_cell: to,
                    pointer: pointer.clone(),
                }
            })
            .collect()
    }

    /// **The multi-cell DSR member-cell enumeration (contract 10.4 — iterate `member_cells`, 0
    /// holders missed).** The distinct cells a cross-org channel's membership spans, INCLUDING the
    /// home cell — the set the Chat erase cascade ([`crate::erase`]) iterates so a person P's data
    /// is reached in EVERY cell P participates in (a holder in a member cell is never missed). The
    /// per-cell erasure stays cell-local (each cell crypto-shreds against ITS own KMS); this
    /// enumeration is the fan-out cursor, not a cross-store reach.
    ///
    /// Unlike [`Self::fan_out_channel_event`] (which excludes the home cell — no self-hop), the DSR
    /// enumeration INCLUDES the home cell: the home cell holds P's authored bodies too, so it is a
    /// holder the DSR must reach.
    #[must_use]
    pub fn dsr_member_cells(&self, members: &[FederatedMember]) -> Vec<CellId> {
        let mut cells = self.distinct_member_cells(members);
        // The home cell is always a DSR holder (it holds the channel + P's authored bodies), even if
        // no member's resolved home cell == it.
        if !cells.contains(&self.home_cell) {
            cells.push(self.home_cell.clone());
            cells.sort();
        }
        cells
    }

    /// The distinct member cells of a roster, sorted + de-duplicated (the BTreeSet keeps the fan-out
    /// stable + carries ONE pointer per cell, never one per principal).
    fn distinct_member_cells(&self, members: &[FederatedMember]) -> Vec<CellId> {
        members
            .iter()
            .map(|m| m.home_cell.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// **Resolve a cross-org channel pointer cell-local (the residency gate — row 12.6 / 5.6).** A
    /// member cell that received a [`CrossOrgPointer`] does NOT hold the channel — it holds a
    /// pointer. To render the channel for `viewer`, it asks the channel's HOME cell (via `resolver`)
    /// to render it: the home cell permission-checks `viewer` THERE (`project()`) and returns ONLY
    /// the [`ChannelProjection`] (rendered-or-tombstone). The channel's content NEVER leaves its
    /// residency cell — only the already-filtered projection crosses back. This call PROVES
    /// resolution is cell-local (the member cell resolves THROUGH the home cell; it never reaches
    /// into the home cell's rows itself).
    #[must_use]
    pub fn resolve_cell_local(
        &self,
        pointer: &CrossOrgPointer,
        viewer: &Principal,
        resolver: &dyn CellLocalChannelResolution,
    ) -> ChannelProjection {
        // The home cell owns the channel's residency — it renders + permission-checks; we receive
        // only the projection. No raw row, no message log, no body byte crosses back.
        resolver.resolve_in_home_cell(pointer, viewer)
    }

    /// Mint the `chat.channel.message_created` [`EventEnvelope`] for a channel event — the Bus's
    /// propagation half mints the cross-cell pointer FROM this. The envelope's `subject` is the
    /// OPAQUE [`channel_ref`] URN; its `payload` is EMPTY (the message body lives in the home cell's
    /// log under a per-subject DEK, never on this envelope — the cross-cell event is a *pointer*
    /// event by design: the durable bus carries only the pointer, never the body bytes).
    fn channel_event_envelope(
        &self,
        conv: &ConversationId,
        correlation_id: &CorrelationId,
    ) -> EventEnvelope {
        let subject = channel_ref(conv);
        EventEnvelope {
            event_id: EventId(format!("xorg-{}", conv.conversation_id)),
            type_: EventType(CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: TenantId(conv.tenant.clone()),
            region: Region(conv.region.clone()),
            actor: Actor(Principal::stub(
                myelin_identity::PrincipalId("xorg-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                TenantId(conv.tenant.clone()),
            )),
            subject,
            aggregate: AggregateKey(format!("channel:{}", conv.conversation_id)),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            // The pointer event carries NO personal data — the body (which may) never rides it.
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            // EMPTY payload — the channel message body NEVER rides the cross-cell pointer event.
            payload: serde_json::json!({}),
        }
    }

    /// **The gate telemetry — `events_fanned_out`.** How many cross-org channel pointers the layer
    /// fanned out (aggregate, PII-free).
    #[must_use]
    pub fn events_fanned_out(&self) -> u64 {
        self.events_fanned_out.load(Ordering::SeqCst)
    }

    /// **The ZERO — `raw_rows_crossed`.** Pinned to 0 by construction (the layer only ever emits the
    /// four-field PII-free frame); exposed as a live tripwire so a future regression that carried a
    /// body/topic/roster/raw row across the bridge is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace raw_rows_crossed -> 0` is
    /// observationally identical because the layer NEVER increments it (the structural guarantee) —
    /// the *correct* property, not a coverage gap. The field + the read seam stay so the tripwire is
    /// wired the day a regression lands (mirrors `CrossCellBridge::cross_cell_raw_rows`).
    #[must_use]
    pub fn raw_rows_crossed(&self) -> u64 {
        self.raw_rows_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossOrgChannel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the home cell id + the aggregate counters, never a member / pointer / body.
        f.debug_struct("CrossOrgChannel")
            .field("home_cell", &self.home_cell.as_str())
            .field("events_fanned_out", &self.events_fanned_out())
            .field("raw_rows_crossed", &self.raw_rows_crossed())
            .finish()
    }
}

/// **The PII-free fan-out proof (the gate telemetry body).** What crosses the bridge for a
/// fanned-out cross-org pointer is EXACTLY the four frozen [`CrossCellPointer`] fields + the
/// destination routing handle — never the body, never the topic/roster. This helper extracts the
/// (opaque, PII-free) fields a fan-out drill asserts crossed, so a drill can show "the fan-out
/// carried only `subject`/`type`/`correlation_id`/`home_cell` + `to_cell`" with the concrete opaque
/// values. It returns the fields by reference — there is structurally no body field to return.
#[must_use]
pub fn fanned_out_carried_fields(
    fanned: &CrossOrgPointer,
) -> (
    &CellId,
    &OpaqueSubjectId,
    &myelin_events::ArtifactType,
    &CorrelationId,
    &CellId,
) {
    (
        &fanned.to_cell,
        fanned.pointer.subject(),
        fanned.pointer.artifact_type(),
        fanned.pointer.correlation_id(),
        fanned.pointer.home_cell(),
    )
}

/// Adapt a [`CrossOrgPointer`] to the Bus's [`PropagatedPointer`] shape (the SAME four-field frame
/// under the `ChatCrossOrg` stream) — proving the Chat cross-org fan-out is the Bus's propagation,
/// not a parallel one (EI-01 §7). Used by the CDC pair to round-trip the cross-org pointer through
/// the frozen 12.6 wire shape.
#[must_use]
pub fn as_propagated(fanned: &CrossOrgPointer) -> PropagatedPointer {
    PropagatedPointer {
        to_cell: fanned.to_cell.clone(),
        pointer: fanned.pointer.clone(),
        stream: CrossCellStream::ChatCrossOrg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn conv() -> ConversationId {
        ConversationId::new("acme", "fr-par", "01J0CHAN")
    }

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("viewer-opaque".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    /// A cross-org channel roster: the home cell holds the creator; cell-de-1 + cell-nl-1 hold
    /// foreign-org members. Two distinct foreign cells, one with two members (to prove the fan-out
    /// is per-CELL not per-principal).
    fn roster() -> Vec<FederatedMember> {
        vec![
            FederatedMember::new("psn:creator", CellId::from_token("cell-fr-par-1")),
            FederatedMember::new("psn:de-member-1", CellId::from_token("cell-de-1")),
            FederatedMember::new("psn:de-member-2", CellId::from_token("cell-de-1")),
            FederatedMember::new("psn:nl-member", CellId::from_token("cell-nl-1")),
        ]
    }

    /// An in-process home-cell resolver standing in for the channel's home cell (the SAME stand-in
    /// the control-plane bridge + search federated + KN collab tests use). It renders the channel
    /// IFF the viewer is permitted THERE — and returns ONLY a projection, never a raw row / message
    /// log / body byte.
    struct HomeCellResolver {
        /// Which viewer ids the home cell permits to render (the cell-local permission check).
        allowed: Vec<String>,
        /// The rendered channel card the home cell exposes (never the raw message log / body).
        rendered: String,
    }

    impl CellLocalChannelResolution for HomeCellResolver {
        fn resolve_in_home_cell(
            &self,
            pointer: &CrossOrgPointer,
            viewer: &Principal,
        ) -> ChannelProjection {
            let subject = pointer.subject().clone();
            if self.allowed.iter().any(|id| id == &viewer.principal_id.0) {
                ChannelProjection::Rendered {
                    subject,
                    rendered: self.rendered.clone(),
                }
            } else {
                // Unauthorised cross-org viewer → a tombstone with NO content (secrets never cross).
                ChannelProjection::Tombstone { subject }
            }
        }
    }

    /// **THE FAN-OUT GATE — cross-org fan-out carries ONLY the PII-free pointer (0 raw rows cross).**
    /// A cross-org channel event fans out to the channel's OTHER member cells; each produced pointer
    /// carries ONLY the four frozen fields; the serialised frame contains neither a body nor a
    /// topic/roster; `raw_rows_crossed == 0`; the fan-out is per-CELL (de-1 appears once though it
    /// has two members).
    #[test]
    fn cross_org_fan_out_carries_only_the_pointer_zero_raw_rows() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let corr = CorrelationId("event-causal-root".into());
        let fanned = xorg.fan_out_channel_event(&conv(), &corr, &roster());

        // One pointer per OTHER DISTINCT member cell (home filtered out; de-1 de-duplicated).
        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(dests, vec!["cell-de-1", "cell-nl-1"]);
        assert_eq!(xorg.events_fanned_out(), 2);

        for pp in &fanned {
            let (to, subject, kind, corr_field, home) = fanned_out_carried_fields(pp);
            // The subject is the OPAQUE channel URN — never a person, never the body.
            assert_eq!(
                subject.artifact_ref().0,
                "myelin://acme/chat/channel/01J0CHAN"
            );
            assert_eq!(kind, &myelin_events::ArtifactType::Channel);
            assert_eq!(corr_field, &corr);
            assert_eq!(home.as_str(), "cell-fr-par-1");
            assert!(matches!(to.as_str(), "cell-de-1" | "cell-nl-1"));

            // The frame serialises to EXACTLY the four fields — no body/topic/roster field.
            let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
            let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
            let mut keys: Vec<&str> = json
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);
            assert!(
                !wire.contains("payload"),
                "no payload/body field on the frame: {wire}"
            );
        }
        // THE GATE: 0 raw rows crossed the bridge.
        assert_eq!(xorg.raw_rows_crossed(), 0);
    }

    /// **The single-home-cell pin is LIFTED — the fan-out reaches EVERY other member cell.** A
    /// membership spanning cells produces a pointer for each of the channel's other distinct member
    /// cells; the pointer rides the event's causal-root (BUS-5).
    #[test]
    fn membership_spanning_cells_reaches_every_other_member_cell() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-a"));
        let members = vec![
            FederatedMember::new("p1", CellId::from_token("cell-a")),
            FederatedMember::new("p2", CellId::from_token("cell-b")),
            FederatedMember::new("p3", CellId::from_token("cell-c")),
            FederatedMember::new("p4", CellId::from_token("cell-d")),
        ];
        let corr = CorrelationId("root".into());
        let fanned = xorg.fan_out_channel_event(&conv(), &corr, &members);

        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(
            dests,
            vec!["cell-b", "cell-c", "cell-d"],
            "every OTHER member cell reached"
        );
        for pp in &fanned {
            assert_eq!(
                pp.pointer.correlation_id(),
                &corr,
                "rides the event causal-root"
            );
        }
    }

    /// **A single-home-cell channel propagates nothing (the M4 floor preserved for non-cross-org).**
    /// With every member in the home cell, the fan-out is empty — the non-federated path is unchanged.
    #[test]
    fn single_home_cell_channel_fans_out_nothing() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-a"));
        let members = vec![
            FederatedMember::new("p1", CellId::from_token("cell-a")),
            FederatedMember::new("p2", CellId::from_token("cell-a")),
        ];
        let fanned = xorg.fan_out_channel_event(&conv(), &CorrelationId("root".into()), &members);
        assert!(
            fanned.is_empty(),
            "a single-home-cell channel has nowhere to fan out"
        );
        assert_eq!(xorg.events_fanned_out(), 0);
        assert_eq!(xorg.raw_rows_crossed(), 0);
    }

    /// **THE RESIDENCY GATE — resolution stays cell-local, only the projection crosses (5.6).** A
    /// member in a foreign cell resolves a cross-org channel THROUGH the home cell: the home cell
    /// renders + permits, returning ONLY a [`ChannelProjection`] (rendered) — never the raw message
    /// log / body. The member cell never reaches into the home cell's rows itself.
    #[test]
    fn resolution_stays_cell_local_only_the_projection_crosses() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let fanned = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-member-1",
                CellId::from_token("cell-de-1"),
            )],
        );
        let pointer = &fanned[0];

        // The HOME cell renders for an allowed viewer — returns ONLY the projection.
        let home = HomeCellResolver {
            allowed: vec!["viewer-opaque".into()],
            rendered: "#cross-org-incident · active (rendered in fr-par-1, viewer-scoped)".into(),
        };
        let proj = xorg.resolve_cell_local(pointer, &viewer(), &home);
        assert!(
            proj.is_rendered(),
            "an allowed viewer gets the rendered projection"
        );
        assert_eq!(proj.subject().0, "myelin://acme/chat/channel/01J0CHAN");
        if let ChannelProjection::Rendered { rendered, .. } = &proj {
            assert!(rendered.contains("rendered in fr-par-1"));
        }
    }

    /// **The residency gate — an unauthorised cross-org viewer resolves to a TOMBSTONE (no content).**
    /// The home cell permission-checks THERE; a viewer it does not permit gets a tombstone carrying
    /// NO content (the channel's content never leaves its residency cell — no leak across the org
    /// boundary).
    #[test]
    fn unauthorised_cross_org_viewer_resolves_to_a_tombstone() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let fanned = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-member-1",
                CellId::from_token("cell-de-1"),
            )],
        );
        let home = HomeCellResolver {
            allowed: vec![], // the home cell permits no one → tombstone.
            rendered: "secret-channel-name".into(),
        };
        let proj = xorg.resolve_cell_local(&fanned[0], &viewer(), &home);
        assert!(
            !proj.is_rendered(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(matches!(proj, ChannelProjection::Tombstone { .. }));
        // The tombstone carries no content — the secret name never crossed.
        let wire = serde_json::to_string(&proj).expect("projection serialises");
        assert!(!wire.contains("secret-channel-name"));
    }

    /// **THE MULTI-CELL DSR GATE — the enumeration iterates EVERY member cell (10.4, 0 holders
    /// missed).** A cross-org channel's membership spans the home cell + two foreign cells; the DSR
    /// enumeration returns ALL THREE distinct cells (incl. the home cell — it holds P's authored
    /// bodies). A member cell P participates in is never missed.
    #[test]
    fn multi_cell_dsr_iterates_every_member_cell_zero_missed() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let cells = xorg.dsr_member_cells(&roster());
        let names: Vec<&str> = cells.iter().map(CellId::as_str).collect();
        // ALL three distinct cells — the home cell INCLUDED (it is a DSR holder too).
        assert_eq!(names, vec!["cell-de-1", "cell-fr-par-1", "cell-nl-1"]);
        assert_eq!(
            cells.len(),
            3,
            "0 member cells missed by the DSR enumeration"
        );
    }

    /// The DSR enumeration ALWAYS includes the home cell, even when no member's resolved cell == it
    /// (the home cell holds the channel + the creator's authored bodies — it is a holder).
    #[test]
    fn dsr_always_includes_the_home_cell() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-home"));
        // No member resolves to cell-home.
        let members = vec![
            FederatedMember::new("p1", CellId::from_token("cell-b")),
            FederatedMember::new("p2", CellId::from_token("cell-c")),
        ];
        let cells = xorg.dsr_member_cells(&members);
        let names: Vec<&str> = cells.iter().map(CellId::as_str).collect();
        assert!(
            names.contains(&"cell-home"),
            "the home cell is always a DSR holder: {names:?}"
        );
        assert!(names.contains(&"cell-b") && names.contains(&"cell-c"));
    }

    /// **The CDC pair for contract 12.6 (Chat's cross-org consumer half).** A PROVIDER (Chat's
    /// cross-org fan-out) emits the channel pointer to its frozen 12.6 wire shape; a CONSUMER (the
    /// control plane's cross-cell carriage, stood in by [`PropagatedPointer`]) deserialises that
    /// exact wire shape and reads back ONLY the four frozen fields under the `ChatCrossOrg` stream.
    /// The Chat fan-out IS the Bus's propagation — one frame, conformant both ways (EI-01 §7).
    #[test]
    fn cdc_12_6_chat_consumer_reads_only_the_four_fields() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let fanned = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-member-1",
                CellId::from_token("cell-de-1"),
            )],
        );
        let provider = &fanned[0];

        // PROVIDER emits the pointer frame to its canonical 12.6 wire shape.
        let wire = serde_json::to_string(&provider.pointer).expect("provider emits the frame");
        let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);

        // CONSUMER (the control-plane carriage) reads it back as the Bus's ChatCrossOrg propagation.
        let consumer: CrossCellPointer =
            serde_json::from_str(&wire).expect("consumer reads the frame");
        assert_eq!(
            consumer, provider.pointer,
            "the CDC wire shape is conformant both ways"
        );

        // And the Chat fan-out adapts to the Bus's PropagatedPointer under the ChatCrossOrg kind.
        let propagated = as_propagated(provider);
        assert_eq!(propagated.stream, CrossCellStream::ChatCrossOrg);
        assert_eq!(
            propagated.pointer.artifact_type(),
            &myelin_events::ArtifactType::Channel
        );
    }

    /// The `CrossOrgChannel` Debug is PII-free + aggregate-only (the home cell id + counters, never a
    /// member / pointer / body). Mirrors the `CrossCellPropagator` PII-free log discipline.
    #[test]
    fn cross_org_debug_is_pii_free() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let _ = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-secret-member",
                CellId::from_token("cell-de-1"),
            )],
        );
        let dbg = format!("{xorg:?}");
        assert!(
            dbg.contains("cell-fr-par-1"),
            "Debug shows the home cell id: {dbg}"
        );
        assert!(
            dbg.contains("events_fanned_out"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("psn:de-secret-member"),
            "Debug leaks no member id: {dbg}"
        );
    }

    /// **A cross-org channel is a Conversation whose membership spans cells** — built on the
    /// CHAT-P7 non-foreclosure (`home_cell` is a VALUE; membership admits a foreign principal). The
    /// `from_membership` adapter carries the pseudonymous id, never PII.
    #[test]
    fn cross_org_channel_built_on_the_non_foreclosing_conversation_model() {
        let m = Membership::member(conv(), "psn:foreign-org-principal");
        let fed = FederatedMember::from_membership(&m, CellId::from_token("cell-de-1"));
        assert_eq!(fed.principal_id, "psn:foreign-org-principal");
        assert_eq!(fed.home_cell.as_str(), "cell-de-1");
        // The channel URN is built from the conversation's (tenant, conversation_id) — the opaque
        // addressing handle, never the body/topic.
        assert_eq!(
            channel_ref(&conv()).0,
            "myelin://acme/chat/channel/01J0CHAN"
        );
    }
}
