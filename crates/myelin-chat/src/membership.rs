//! # `membership` — membership → `write_tuples` → zookie in ONE transaction + the new-enemy guard
//! + the send/membership check gate (CHAT-P8 / P-402, M4-C1)
//!
//! The **ReBAC-write + zookie + gate slice** of milestone M4-C1 — the second committable unit of
//! the membership half (the Conversation/Membership entity is CHAT-P7 / [`crate::conversation`]).
//! Conformed to the frozen glue in
//! [`03-events-contracts-and-glue.md`](../../../../planning/04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md)
//! §5 (membership writes project tuples via `write_tuples` in the SAME tx as the membership row +
//! the `chat.channel.member_*` event, STAMPING the returned zookie — the new-enemy guard) + §1.1
//! (the `chat.channel.created/archived/member_added/member_removed/linked` durable events) and the
//! frozen Chat ReBAC fragment in
//! [`00-reconciliation-decisions.md`](../../../../planning/05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
//! §1 (`channel.read = member + parent_project->read`; the `watcher` read-fanout relation).
//!
//! It ships:
//!
//! - **[`MembershipTupleWriter`]** — the port the membership write is generic over (contract 4.6):
//!   `write_membership_tuples([Δtuple], precondition?) -> Zookie`. The PRODUCTION binding is
//!   Identity's `TupleStore::write_tuples` (which carries the verified `(tenant, region)` scope the
//!   names-only `IdentityService::write_tuples` ABI cannot — see the module note below); the CDC
//!   binds the REAL engine and proves the channel.read fragment resolves. Chat NEVER owns a second
//!   tuple-write path (one write primitive, EI-01 §7) — it writes the frozen [`TupleDelta`] against
//!   the frozen [`crate::rebac_fragment`] `channel`/`member` shape.
//! - **[`MembershipGate`]** — the send / edit / membership permission gate (contract 4.2): every
//!   send and every membership mutation is gated by `Id.check(subject, permission, channel, zookie?,
//!   caveat?)` and is **fail-closed** (a `Deny` / `Conditional` / Id-error ALL deny). Generic over
//!   the frozen [`IdentityService`] ABI carrier (chat is a consumer subsystem — it depends on the
//!   names-only `myelin-identity`, never the engine crate; the §2.9 DAG stays acyclic).
//! - **[`MembershipService`]** — the atomic co-commit driver. A membership change runs:
//!   `write_membership_tuples` → zookie → STAMP the zookie on the conversation
//!   ([`crate::conversation::ConversationStore::stamp_acl_zookie`]) → emit the
//!   `chat.channel.member_added` / `member_removed` event — all on the SAME [`OutboxTx`] + the same
//!   conversation-store mutation. The channel lifecycle events
//!   (`created`/`archived`/`linked`) ride the same outbox.
//!
//! ## The new-enemy guard (the load-bearing invariant; contract 4.6/4.10, §5)
//! A just-REVOKED grant cannot read stale on the next unfurl/read. The mechanism:
//! [`MembershipService::remove_member`] writes the `Remove(member)` tuple → Identity advances the
//! revision and returns the NEW zookie → we STAMP it on the conversation in the same transaction.
//! Every permission-sensitive read of the conversation (the unfurl/project gate, CHAT-P13) reads
//! AT-OR-AFTER that stamped watermark with [`ConsistencyMode::Strong`] — so the read resolves
//! against the POST-revoke tuple set (the removed member is gone), never the pre-revoke one. The
//! zookie-stamped strong read bypasses the fail-static cache (§8.7), so the revoke is visible
//! immediately (0 stale grants readable post-revoke). [`MembershipService::read_consistency`] is
//! the helper a reader composes the strong, stamped read from.
//!
//! ## The atomicity gate (one transaction; the GATE)
//! The membership row mutation + the `write_tuples` zookie stamp + the `chat.channel.member_*` event
//! commit in ONE transaction (BUS-2 co-commit, contract 2.2). [`MembershipService`] stages the
//! conversation-store mutation and the outbox emit onto the SAME [`OutboxTx`]; a kill between the
//! membership row and the tuple write commits NEITHER (0 partial membership) — the outbox
//! transaction is dropped, the staged conversation mutation is rolled back, and nothing is durable.
//! In the in-memory floor the conversation store mutation is applied eagerly (there is no real PG
//! transaction to roll back), so the ORDER is load-bearing: we write the tuple + stamp the zookie
//! FIRST (the part that can fail loudly), then mutate the membership row, then emit + return — a
//! tuple-write failure aborts BEFORE the membership row changes (0 partial membership). The REAL PG
//! binding (the named promotion) co-commits all three in one DB transaction.
//!
//! ## Why a port trait for the tuple write (the DAG; EI-01 §7 — one primitive, no third copy)
//! `myelin-chat` is a consumer SUBSYSTEM crate: it depends on the frozen contract surface
//! `myelin-identity` (the names-only `IdentityService` ABI), NOT on `myelin-identity-service` (the
//! engine). But the names-only `IdentityService::write_tuples` is deliberately SCOPE-LESS (it
//! carries no verified `(tenant, region)`), so the real write path is the engine's
//! `TupleStore::write_tuples` (which carries the scope). To consume the real write WITHOUT taking a
//! consumer→service edge, the membership service is generic over the [`MembershipTupleWriter`] port:
//! the production wiring adapts `TupleStore::write_tuples` to it (a thin scope-carrying closure),
//! and the CDC binds the REAL engine and proves the fragment resolves. Chat declares NO second
//! tuple-write surface — the port is a thin adapter over the ONE frozen `write_tuples` primitive.
//!
//! ## FLOORS named (VISION §3)
//! - **No new floor** — this completes the M4-C1 silent-data-loss floor's membership half. The
//!   per-viewer `project(ref, viewer)` unfurl gate that READS the stamped zookie is CHAT-P13
//!   (CHAT-D5, the no-leak proof); the ACL-filtered Search conjoin that reads it is CHAT-P20
//!   (CHAT-D11). Both DEPEND-ON this stamp; here the stamp + the strong-read recipe land.
//! - **The cross-org / federated channels follow-on (M5-C-X1 / CHAT-P30 / P-504)** rides the frozen
//!   cross-cell PII-free pointer bridge (contract 12.6, OQ-I) — a membership set that spans cells is
//!   non-foreclosed by [`crate::conversation::Membership::principal_id`] (a free principal id), and
//!   the tuple write here writes one home-cell's tuples; the cross-cell tuple bridge is CHAT-P30.

use myelin_events::{AggregateKey, DataRole, EventDraft, EventType, OutboxTx as _, Visibility};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Precondition, Principal,
    RelName, RelationTuple, TupleDelta, Zookie,
};

use crate::conversation::{
    Conversation, ConversationError, ConversationStore, Membership, MembershipRole,
};
use crate::events::{
    CHAT_CHANNEL_ARCHIVED, CHAT_CHANNEL_CREATED, CHAT_CHANNEL_LINKED, CHAT_CHANNEL_MEMBER_ADDED,
    CHAT_CHANNEL_MEMBER_REMOVED,
};
use crate::rebac_fragment::object_types;
use crate::store::{ConversationId, OutboxTx};

/// The frozen Chat `channel` permission names the send/membership gate checks (the
/// [`crate::rebac_fragment`] `channel` permissions — names verbatim from §5). A gate that checked a
/// name not in this fragment would be a contract drift; the constants anchor the gate to the frozen
/// vocabulary.
pub mod permissions {
    /// `channel.post = member` — the SEND gate (a non-member cannot post).
    pub const POST: &str = "post";
    /// `channel.read = member + parent_project->read` — the READ/view gate.
    pub const READ: &str = "read";
    /// `channel.manage = member & parent_project->admin` — the membership-MUTATION gate
    /// (invite / archive / settings — the consequential, visibility-changing mutations, §8).
    pub const MANAGE: &str = "manage";
}

/// A membership-service error — a typed, LOUD surface (a failure is a value, never a silent
/// fallthrough; EI-01 §3 prove-it). Distinct from [`ConversationError`] (the store half) so the
/// caller can tell a tuple-write/zookie failure from a missing-conversation.
#[derive(Debug, PartialEq, Eq)]
pub enum MembershipError {
    /// The conversation the membership change targets does not exist (a phantom membership is LOUD).
    NotFound(String),
    /// The `write_tuples` call failed (the tuple write did NOT commit, so NOTHING else does — the
    /// atomicity guarantee: a tuple-write failure aborts the WHOLE change before the membership row
    /// or the event commit). Carries the writer's error string.
    TupleWrite(String),
    /// The send / membership permission gate DENIED (fail-closed): the subject lacks the required
    /// `channel.post` / `channel.manage` permission. The gate is fail-closed — a `Deny`,
    /// `Conditional`, OR an Id-error all surface as this `Denied`.
    Denied {
        /// The permission that was denied.
        permission: String,
        /// The channel the denial was on.
        channel: String,
    },
    /// The outbox co-commit emit failed (the event could not be staged). Carries the cause.
    Emit(String),
    /// A conversation-store mutation failed (the zookie stamp / row write). Carries the cause.
    Store(String),
}

impl core::fmt::Display for MembershipError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MembershipError::NotFound(id) => write!(f, "conversation {id} not found"),
            MembershipError::TupleWrite(e) => write!(f, "write_tuples failed: {e}"),
            MembershipError::Denied {
                permission,
                channel,
            } => write!(f, "permission `{permission}` denied on channel {channel}"),
            MembershipError::Emit(e) => write!(f, "outbox emit failed: {e}"),
            MembershipError::Store(e) => write!(f, "conversation store failed: {e}"),
        }
    }
}

impl std::error::Error for MembershipError {}

impl From<ConversationError> for MembershipError {
    fn from(e: ConversationError) -> MembershipError {
        match e {
            ConversationError::NotFound(id) => MembershipError::NotFound(id),
            other => MembershipError::Store(other.to_string()),
        }
    }
}

/// The membership-service result alias.
pub type Result<T> = core::result::Result<T, MembershipError>;

/// **The tuple-write port (contract 4.6).** The membership write is generic over THIS so chat
/// consumes the REAL `write_tuples` primitive (returning the zookie to stamp) without taking a
/// consumer→service DAG edge: the production binding adapts Identity's scope-carrying
/// `TupleStore::write_tuples` to it; the CDC binds the live engine. The port carries EXACTLY the
/// frozen 4.6 shape — `[TupleDelta]` + an optional [`Precondition`] → [`Zookie`] — and nothing else
/// (no second write language, no chat-private tuple shape).
pub trait MembershipTupleWriter {
    /// Write the membership tuple deltas atomically; return the advanced [`Zookie`] to stamp on the
    /// conversation (the new-enemy watermark). On failure NOTHING changed and NOTHING emitted
    /// (emit-iff-committed) — the membership change aborts before the row mutates.
    fn write_membership_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
    ) -> core::result::Result<Zookie, String>;
}

/// **The send / edit / membership permission gate (contract 4.2).** Generic over the frozen
/// [`IdentityService`] ABI carrier. The gate is **fail-closed**: only an explicit
/// `Ok(Decision::Allow)` permits; a `Deny`, a `Conditional` (a caveat needing context — never a
/// silent allow), OR an Id-error ALL deny. The check is at the conversation's stamped `acl_zookie`
/// with [`ConsistencyMode::Strong`] so the gate honours a just-revoked grant (the new-enemy guard
/// applies to the gate too — a removed member cannot post in the window between revoke and cache
/// expiry).
pub struct MembershipGate<I: IdentityService> {
    id: I,
}

impl<I: IdentityService> MembershipGate<I> {
    /// Compose the gate over the Id dependency.
    pub fn new(id: I) -> MembershipGate<I> {
        MembershipGate { id }
    }

    /// **Gate a `channel.<permission>` action (fail-closed).** `subject` must hold `permission` on
    /// the `channel` at-or-after `at_zookie` (the conversation's stamped acl watermark, or the empty
    /// zookie for a never-stamped channel). Returns `Ok(())` iff `Allow`; otherwise a `Denied`
    /// error. NO field of the channel is read on a denial — the gate is the leak-free chokepoint.
    pub fn check_channel(
        &self,
        subject: &Principal,
        permission: &str,
        channel_id: &ConversationId,
        at_zookie: Option<&str>,
    ) -> Result<()> {
        // The ReBAC `check` object is the Id-side `channel:<id>` ObjectId form the membership tuples
        // key on (the SAME spelling [`MembershipService::member_tuples`] writes against). This is
        // distinct from the `myelin://…/chat/channel/<id>` `ArtifactRef` the Refs unfurl resolution
        // uses — both name the same channel; the authz `check` resolves the object id, so the gate
        // checks against the object-id form so it joins the membership tuple (one object-id language).
        let object = myelin_tenancy::ArtifactRef(channel_object(&channel_id.conversation_id));
        let at = Consistency {
            at_least: Zookie(at_zookie.unwrap_or("").to_string()),
            // Strong: bypass the fail-static cache, honour the new-enemy guard (a revoked grant is
            // denied immediately, not after the cache window — §8.7).
            mode: ConsistencyMode::Strong,
        };
        let permission_tok = Permission(permission.to_string());
        match self.id.check(subject, &permission_tok, &object, &at, None) {
            Ok(Decision::Allow) => Ok(()),
            // Fail-closed: Deny / Conditional / Id-error ALL deny (no leak, no silent allow).
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                Err(MembershipError::Denied {
                    permission: permission.to_string(),
                    channel: channel_id.conversation_id.clone(),
                })
            }
        }
    }

    /// The SEND gate ([`permissions::POST`]) — every send is gated; a non-member is fail-closed.
    pub fn check_send(
        &self,
        subject: &Principal,
        channel_id: &ConversationId,
        at_zookie: Option<&str>,
    ) -> Result<()> {
        self.check_channel(subject, permissions::POST, channel_id, at_zookie)
    }

    /// The MEMBERSHIP-MUTATION gate ([`permissions::MANAGE`]) — invite/remove/archive are gated by
    /// `channel.manage` (the visibility-changing mutations, §8); a plain member is fail-closed.
    pub fn check_manage(
        &self,
        subject: &Principal,
        channel_id: &ConversationId,
        at_zookie: Option<&str>,
    ) -> Result<()> {
        self.check_channel(subject, permissions::MANAGE, channel_id, at_zookie)
    }
}

/// The `channel:<id>` ReBAC object id the membership tuples key on AND the gate's `check` resolves
/// against (the same object id the [`crate::rebac_fragment`] `channel` type + the CDC provider use —
/// one object-id language, never a second spelling). NOTE: this is the Id-side `ObjectId` string,
/// distinct from the `myelin://…/chat/channel/<id>` `ArtifactRef` the Refs unfurl resolution uses;
/// both name the same channel. The authz tuple store + `check` key on THIS `channel:<id>` form.
pub fn channel_object(channel_id: &str) -> String {
    format!("{}:{}", object_types::CHANNEL, channel_id)
}

/// **The membership co-commit driver (CHAT-P8).** Drives the atomic
/// membership-change → `write_tuples` → zookie-stamp → `chat.channel.member_*` event on the SAME
/// transaction, and the channel lifecycle events (`created`/`archived`/`linked`). Generic over the
/// [`MembershipTupleWriter`] port (the tuple write) — the conversation store + the outbox are
/// passed per call so the service stays a thin, dependency-injected orchestrator (no hidden state).
pub struct MembershipService<W: MembershipTupleWriter> {
    writer: W,
}

impl<W: MembershipTupleWriter> MembershipService<W> {
    /// Compose the service over the tuple-write port.
    pub fn new(writer: W) -> MembershipService<W> {
        MembershipService { writer }
    }

    /// The ReBAC `member` relation name (the [`crate::rebac_fragment`] `channel.member` ACL arm).
    const MEMBER_REL: &'static str = "member";
    /// The ReBAC `watcher` relation name (the Notif read-fanout relation, contract 4.9).
    const WATCHER_REL: &'static str = "watcher";

    /// Build the `Add`/`Remove` membership tuple deltas for a `(channel, principal, role)` change.
    /// A member always gets the `member` ACL tuple; a watcher additionally gets the `watcher`
    /// read-fanout tuple (so `list_subjects(channel, watcher)` resolves the read-fanout set, §5).
    /// An admin's `manage` is the `member & parent_project->admin` intersection (the role is carried
    /// on the membership row; the tuple set is `member` either way — admin is not a separate
    /// relation in the frozen fragment, §5).
    fn member_tuples(channel_id: &str, m: &Membership, add: bool) -> Vec<TupleDelta> {
        let object = channel_object(channel_id);
        let mut deltas = Vec::with_capacity(2);
        let member_tuple = RelationTuple {
            object: myelin_identity::ObjectId(object.clone()),
            relation: RelName(Self::MEMBER_REL.to_string()),
            subject: myelin_identity::PrincipalId(m.principal_id.clone()),
            caveat: None,
        };
        let watcher_tuple = RelationTuple {
            object: myelin_identity::ObjectId(object),
            relation: RelName(Self::WATCHER_REL.to_string()),
            subject: myelin_identity::PrincipalId(m.principal_id.clone()),
            caveat: None,
        };
        if add {
            deltas.push(TupleDelta::Add(member_tuple));
            if m.is_watcher {
                deltas.push(TupleDelta::Add(watcher_tuple));
            }
        } else {
            deltas.push(TupleDelta::Remove(member_tuple));
            // On remove we always drop the watcher tuple too (a removed member watches nothing).
            deltas.push(TupleDelta::Remove(watcher_tuple));
        }
        deltas
    }

    /// **Add a member: `write_tuples(Add member)` → zookie → stamp → `chat.channel.member_added`
    /// (ONE transaction).** The atomic co-commit (the GATE):
    ///
    /// 1. Verify the conversation exists (a phantom membership is LOUD — `NotFound`).
    /// 2. `write_membership_tuples([Add member, Add watcher?])` → the advanced [`Zookie`]. A
    ///    tuple-write failure aborts HERE — NOTHING else changes (0 partial membership).
    /// 3. STAMP the zookie on the conversation ([`ConversationStore::stamp_acl_zookie`]) — the
    ///    new-enemy watermark.
    /// 4. Write the membership row + the `membership_by_principal` index (the [`ConversationStore`]
    ///    `join`).
    /// 5. Emit `chat.channel.member_added` on the SAME `tx` (the outbox co-commit, BUS-2).
    ///
    /// Returns the stamped [`Zookie`] so the caller can pass it on a read-your-writes read.
    pub fn add_member(
        &self,
        tx: &mut OutboxTx,
        store: &dyn ConversationStore,
        m: Membership,
    ) -> Result<Zookie> {
        let channel_id = m.conv.clone();
        // (1) The conversation must exist.
        store.get(&channel_id)?;
        // (2) write_tuples → zookie (the tuple write is the part that can fail loudly; do it FIRST
        //     so a failure aborts before the membership row mutates — 0 partial membership).
        let deltas = Self::member_tuples(&channel_id.conversation_id, &m, true);
        let zookie = self
            .writer
            .write_membership_tuples(&deltas, None)
            .map_err(MembershipError::TupleWrite)?;
        // (3) STAMP the new-enemy zookie on the conversation (same transaction in the PG binding).
        store
            .stamp_acl_zookie(&channel_id, &zookie.0)
            .map_err(MembershipError::from)?;
        // (4) Write the membership row + the index.
        store.join(m.clone()).map_err(MembershipError::from)?;
        // (5) CO-COMMIT the member_added event on the SAME tx.
        tx.stage_state_change(format!(
            "chat.channel.member_added:{}:{}",
            channel_id.conversation_id, m.principal_id
        ));
        self.emit_member_event(
            tx,
            CHAT_CHANNEL_MEMBER_ADDED,
            &channel_id,
            &m.principal_id,
            m.role,
        )?;
        Ok(zookie)
    }

    /// **Remove a member: `write_tuples(Remove member)` → zookie → stamp → `chat.channel.member_removed`
    /// (ONE transaction) — the NEW-ENEMY GUARD.** Identical atomic shape to [`Self::add_member`] but
    /// the delta is `Remove`. The advanced zookie stamped here is the watermark a subsequent strong
    /// read reads at-or-after, so the removed member's read resolves against the POST-revoke tuple
    /// set (0 stale grants readable post-revoke). Returns the stamped [`Zookie`].
    pub fn remove_member(
        &self,
        tx: &mut OutboxTx,
        store: &dyn ConversationStore,
        channel_id: &ConversationId,
        principal_id: &str,
        role: MembershipRole,
        is_watcher: bool,
    ) -> Result<Zookie> {
        store.get(channel_id)?;
        // Build the Remove deltas from a synthetic membership carrying the watcher bit.
        let m = Membership {
            conv: channel_id.clone(),
            principal_id: principal_id.to_string(),
            role,
            is_watcher,
            notif_pref: serde_json::Value::Null,
        };
        let deltas = Self::member_tuples(&channel_id.conversation_id, &m, false);
        let zookie = self
            .writer
            .write_membership_tuples(&deltas, None)
            .map_err(MembershipError::TupleWrite)?;
        // STAMP the new-enemy watermark BEFORE dropping the row — the conversation's acl_zookie now
        // names the post-revoke revision.
        store
            .stamp_acl_zookie(channel_id, &zookie.0)
            .map_err(MembershipError::from)?;
        store
            .leave(channel_id, principal_id)
            .map_err(MembershipError::from)?;
        tx.stage_state_change(format!(
            "chat.channel.member_removed:{}:{}",
            channel_id.conversation_id, principal_id
        ));
        self.emit_member_event(
            tx,
            CHAT_CHANNEL_MEMBER_REMOVED,
            channel_id,
            principal_id,
            role,
        )?;
        Ok(zookie)
    }

    /// **Create a channel: persist the conversation row + `chat.channel.created` (ONE transaction).**
    /// The created channel has no members yet, so no `write_tuples` runs here (the first membership
    /// add stamps the acl_zookie). An artifact-linked channel ALSO emits `chat.channel.linked` (→
    /// `refs.edge.created` "discussed in", §1.1) for its `linked_ref`.
    pub fn create_channel(
        &self,
        tx: &mut OutboxTx,
        store: &dyn ConversationStore,
        conv: Conversation,
    ) -> Result<()> {
        let channel_id = conv.id.clone();
        let linked_ref = conv.linked_ref.clone();
        store.create(conv).map_err(MembershipError::from)?;
        tx.stage_state_change(format!(
            "chat.channel.created:{}",
            channel_id.conversation_id
        ));
        self.emit_channel_event(tx, CHAT_CHANNEL_CREATED, &channel_id, None)?;
        // An artifact-linked channel announces its link → refs.edge.created ("discussed in").
        if let Some(linked) = linked_ref {
            self.emit_channel_event(tx, CHAT_CHANNEL_LINKED, &channel_id, Some(linked))?;
        }
        Ok(())
    }

    /// **Archive a channel: `chat.channel.archived` (ONE transaction).** The lifecycle end-state.
    /// (The store-side `archived = true` flip is the conversation store's; here the durable event is
    /// emitted on the co-commit so the derived stores see the archive.)
    pub fn archive_channel(&self, tx: &mut OutboxTx, channel_id: &ConversationId) -> Result<()> {
        tx.stage_state_change(format!(
            "chat.channel.archived:{}",
            channel_id.conversation_id
        ));
        self.emit_channel_event(tx, CHAT_CHANNEL_ARCHIVED, channel_id, None)
    }

    /// **Link a channel to an `ArtifactRef`: `chat.channel.linked` (→ `refs.edge.created`).** The
    /// "discussed in" producer (§1.1) — Refs consumes `chat.channel.linked` and creates the edge;
    /// chat emits exactly the durable token (it does not emit the foreign `refs.*` token — the
    /// acyclic-producer invariant, EI-02 §3).
    pub fn link_channel(
        &self,
        tx: &mut OutboxTx,
        channel_id: &ConversationId,
        linked_ref: impl Into<String>,
    ) -> Result<()> {
        tx.stage_state_change(format!(
            "chat.channel.linked:{}",
            channel_id.conversation_id
        ));
        self.emit_channel_event(tx, CHAT_CHANNEL_LINKED, channel_id, Some(linked_ref.into()))
    }

    /// **The strong, zookie-stamped read recipe (the new-enemy guard's READ side).** A reader of a
    /// permission-sensitive view of the conversation composes its `check`/`list_objects` consistency
    /// from the conversation's stamped `acl_zookie` with [`ConsistencyMode::Strong`] — so the read
    /// resolves against the post-write tuple set (read-your-writes; a revoked member is gone). This
    /// is the recipe CHAT-P13 (the unfurl gate) + CHAT-P20 (the Search conjoin) consume.
    pub fn read_consistency(conv: &Conversation) -> Consistency {
        Consistency {
            at_least: Zookie(conv.acl_zookie.clone().unwrap_or_default()),
            mode: ConsistencyMode::Strong,
        }
    }

    /// Emit a `chat.channel.member_*` event (references-only payload: the channel ref + the member
    /// principal id + the role — NEVER a body) on the co-commit transaction.
    fn emit_member_event(
        &self,
        tx: &mut OutboxTx,
        event_type: &str,
        channel_id: &ConversationId,
        principal_id: &str,
        role: MembershipRole,
    ) -> Result<()> {
        let subject = crate::subs::mint_channel(&channel_id.tenant, &channel_id.conversation_id)
            .map_err(|e| MembershipError::Emit(format!("mint channel ref: {e}")))?;
        let draft = EventDraft {
            type_: EventType(event_type.to_string()),
            subject,
            // The per-conversation aggregate (contract 2.3) — every channel/membership event for a
            // conversation is per-aggregate ordered (CHAT-D2 total order), the SAME aggregate the
            // message events use (one ordered log per conversation).
            aggregate: AggregateKey(channel_id.conversation_id.clone()),
            // References-not-payloads: the channel ref + the member principal + the role. No PII.
            payload: serde_json::json!({
                "conversation_id": channel_id.conversation_id,
                "principal": principal_id,
                "role": role.as_token(),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        // A membership change is a causal ROOT for the member_* event (cause = None); a reaction to
        // an inbound event would pass that cause (the gateway's concern).
        tx.emit(draft, None)
            .map_err(|e| MembershipError::Emit(format!("emit {event_type}: {e:?}")))?;
        Ok(())
    }

    /// Emit a `chat.channel.{created,archived,linked}` lifecycle event on the co-commit transaction.
    /// `linked_ref` is `Some` only for `chat.channel.linked` (the `refs.edge.created` producer).
    fn emit_channel_event(
        &self,
        tx: &mut OutboxTx,
        event_type: &str,
        channel_id: &ConversationId,
        linked_ref: Option<String>,
    ) -> Result<()> {
        let subject = crate::subs::mint_channel(&channel_id.tenant, &channel_id.conversation_id)
            .map_err(|e| MembershipError::Emit(format!("mint channel ref: {e}")))?;
        let mut payload = serde_json::json!({
            "conversation_id": channel_id.conversation_id,
        });
        if let Some(linked) = linked_ref {
            payload["linked_ref"] = serde_json::Value::String(linked);
        }
        let draft = EventDraft {
            type_: EventType(event_type.to_string()),
            subject,
            aggregate: AggregateKey(channel_id.conversation_id.clone()),
            payload,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        tx.emit(draft, None)
            .map_err(|e| MembershipError::Emit(format!("emit {event_type}: {e:?}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
