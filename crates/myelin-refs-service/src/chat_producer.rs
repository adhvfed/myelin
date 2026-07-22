//! # `chat_producer` — Chat unfurls: the MAXIMAL consumer + cross-subsystem traversal COMPLETE
//! (REF-P21 / P-337, M4).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! - §4.2 (Chat unfurls via `resolve` + the shared per-ref cache busting on `*.updated` — Chat is
//!   THE consumer of the resolution chokepoint: every mention/artifact_ref/embed it produces is
//!   rendered by asking Refs to `resolve(ref, viewer)`, so a confidential issue / private channel /
//!   fork-scoped CI run degrades to a tombstone, never a leak),
//! - §4.6 (the ONE 4-step ladder on REAL Chat sub-anchors — a tombstone ALWAYS carries the root; a
//!   Chat `message-`/`thread-` anchor is content-addressed by a STABLE opaque id, so it has only
//!   LIVE / GONE / ERASED states — never MOVED/OUTDATED: an edited message keeps its `message_id` and
//!   still resolves LIVE; only a DELETED message is GONE),
//! - §4.4 (the Chat channel ReBAC fragment — `channel.read = member + parent_project->read` — flows
//!   through `list_objects`, so a non-member search/backlink returns 0; the leak-free backlink read
//!   reuses the REF-P11 [`crate::backlinks::BacklinkRead`] over `edge.source_root`).
//!
//! **Contracts implemented (consumer side, to the FROZEN shapes; the Refs engine is UNCHANGED — fixed
//! at M2):**
//! - **5.4** consume the REAL Chat producer reference edges — a Chat message body IS a `myelin-content`
//!   document whose structured inline nodes ([`myelin_content::InlineNode::Mention`] for an `@`-mention,
//!   [`myelin_content::InlineNode::ArtifactRefNode`] for an artifact link, [`myelin_content::InlineNode::Embed`]
//!   for an inline unfurl) are the edges, extracted through the SAME [`crate::extract_edges`] /
//!   [`crate::emit_edges`] seam every other producer uses ([`ChatEdgeProducer`]). Chat is the FINAL,
//!   MAXIMAL producer: it unfurls EVERY artifact class (commit / issue / doc / CI run / another chat
//!   message), so its edge corpus exercises the whole grammar.
//! - **5.6** resolve Chat's `project(ref, viewer)` for the `message-`/`thread-` sub-anchors —
//!   [`ChatOwner`] is a REAL [`crate::ProjectApi`] + [`crate::SubAnchorResolver`] over Chat's content;
//!   its `project` classifies a Chat `#sub` (a `message-<message_id>` message, a `thread-<thread_root_id>`
//!   thread root) into the frozen [`crate::ProjectOutcome`].
//! - **5.7** the `message-`/`thread-` `#sub` kinds on REAL Chat sub-anchors — Refs resolves Chat's
//!   `message-`/`thread-` mints (minted by [`myelin_chat::subs::mint_message`] /
//!   [`myelin_chat::subs::mint_thread`], the CHAT producer half) through the ONE [`crate::ladder`]:
//!   immutable → LIVE, deleted → GONE, crypto-shredded → ERASED. The SAME vocabulary a Git line-range /
//!   a KN block / a CI check / an Issues field degrades through (one ladder).
//! - **4.9** the Chat channel ReBAC fragment flows through `list_objects` — the backlink read
//!   ([`crate::BacklinkRead`]) lowers the FROZEN `SetExpr` over `edge.source_root`, reusing REF-P11; a
//!   non-member (no `channel.read = member + parent_project->read`) sees 0 chat-sourced backlinks (the
//!   non-member-returns-0 property, the GATE of this prompt — supports CHAT-D5).
//!
//! ## Why this is a CONSUMER module, not a new engine (EI-01 §7 coherence — the engine is UNCHANGED)
//! Exactly like the REF-P17 [`crate::git_producer`], the REF-P18 [`crate::kn_producer`], the REF-P19
//! [`crate::ci_producer`], and the REF-P20 [`crate::issues_producer`], REF-P21's deliverable is to WIRE
//! Refs to the REAL Chat producer — the FINAL one — and RE-CONFIRM the invariants on the most
//! adversarial corpus (confidential issues, private channels, fork-scoped CI). It does NOT build a
//! second resolver / ladder / backlink read. So this module:
//! - reuses [`crate::extract_edges`] / [`crate::emit::emit_edges`] for ingest (the §4.1 producer #1 seam),
//! - reuses [`crate::resolve::ResolveService`] + [`crate::ladder`] for resolution (the ONE chokepoint),
//! - reuses [`crate::backlinks::BacklinkRead`] for the leak-free list (the ONE `SetExpr` lowering, REF-P11),
//! - reuses Chat's canonical `message-`/`thread-` mint codecs ([`myelin_chat::subs::mint_message`] /
//!   [`myelin_chat::subs::mint_thread`]) + its frozen ReBAC fragment names
//!   ([`myelin_chat::rebac_fragment`]) by NAME (X-5), never a literal.
//!
//! It adds ONLY the Chat-specific glue: the Chat source-URN construction for the message/thread roots
//! the unfurl edges hang off ([`ChatEdgeProducer`]), and the Chat `ProjectApi`/sub-anchor resolution
//! body ([`ChatOwner`]) the engine calls. No Refs type is re-defined; no parallel second ladder /
//! backlink read is minted.
//!
//! ## Cross-subsystem traversal is now COMPLETE (the milestone — R-M4 closes)
//! With Chat wired, ALL FIVE producers (Git, CI, Knowledge, Issues, Chat) emit the structured inline
//! nodes uniformly (X-2 — mention/artifact_ref/embed) AND Issues/KN own both typed-relation tables, so
//! mention/ref/lifecycle edges are dependable across the WHOLE platform. The spec-to-ship lineage
//! (`initiative → child issues → PRs → commits → CI → deploy → chat decision`) is ONE Refs traverse —
//! Chat is the terminal `chat decision` node, and it can unfurl every prior node in the lineage.
//!
//! ## Floors named (VISION §3 / EI-01 §1 — the prompt's named floors)
//! - **No new Refs floor — the engine is FIXED at M2.** The ladder, the grammar, the chokepoint, the
//!   backlink read are all M2-frozen; this prompt adds ONLY the Chat sub-anchor resolution + the Chat
//!   edge-producer wiring. This is the FINAL producer — the five-producer corpus is now complete.
//! - **In-cell single-home-cell graph build.** Cross-cell fan-out is **R-M5 (REF-P26)** — this wiring
//!   builds + resolves the Chat graph in the artifact's home cell; the C-5 cross-cell semantics are
//!   already frozen in [`crate::resolve`] (a cross-cell unfurl resolves in the target's home cell, only
//!   the filtered projection/tombstone crosses).
//! - **The recorded ACL stands in for the LIVE Chat ReBAC check.** [`ChatOwner::check_view`] and the
//!   non-member backlink test model the frozen `channel.read = member + parent_project->read` fragment
//!   via a recorded grant / the `authz_visible` reverse index; production is Identity's `check` /
//!   `list_objects` over the Chat fragment (REF-P323 / the Chat M4 spine). The leak-free SHAPE is what
//!   Refs owns and proves here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_chat::rebac_fragment::object_types::CHANNEL;
use myelin_chat::subs::{mint_message, mint_thread, CHAT_SUBSYSTEM};
use myelin_content::InlineNode;
use myelin_events::{ArtifactRef, EventEnvelope, EventId, OutboxTx, Result as BusResult};
use myelin_identity::{Decision, Permission, Principal};
use myelin_refs::{sub_kind, ParseError, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::emit::{emit_edges, EdgeDraft};
use crate::ladder::SubState;
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

/// The canonical Chat subsystem token — re-exported from Chat's durable taxonomy
/// ([`myelin_chat::subs::CHAT_SUBSYSTEM`]) so a Chat drill asserts against the ONE token Chat owns,
/// never a literal. (Chat's subsystem prefix on every `chat.*` token + every `chat/...` URN.)
pub const CHAT_OWNER_TOKEN: &str = CHAT_SUBSYSTEM;

/// The frozen Chat channel object type the ReBAC fragment keys on ([`myelin_chat::rebac_fragment`] —
/// `channel.read = member + parent_project->read`). Re-exported so a non-member backlink drill names
/// the relation against Chat's fragment, never a literal. The `view` permission flows through
/// `list_objects` over `edge.source_root` (§4.4) — a non-member's `list_objects` admits 0 channels, so
/// 0 chat-sourced backlinks are returned.
pub const CHAT_CHANNEL_TYPE: &str = CHANNEL;

/// **The Chat reference-edge producer (contract 5.4 EMIT side — the REAL, MAXIMAL Chat producer).** A
/// Chat message body IS a `myelin-content` document: its structured inline nodes
/// ([`InlineNode::Mention`] for an `@`-mention, [`InlineNode::ArtifactRefNode`] for an artifact link,
/// [`InlineNode::Embed`] for an inline unfurl of a commit / issue / doc / CI run / another message) are
/// the edges. This producer constructs the Chat SOURCE URN (the message/thread root) and drives the
/// SAME [`emit_edges`] seam every other producer used — so the WIRING is unchanged; only the CALLER is
/// now the FINAL real surface (Chat, the maximal consumer that unfurls every artifact class).
///
/// References-not-payloads: the source/target are opaque Chat/Git/Issue/CI/KN/Identity URNs; an
/// `@`-mention's target is the PSEUDONYMOUS `member` URN (erasure-safe, §4.6). No chat-message free
/// text is held — only the structured ref nodes are read (the reliability guarantee, EI-04 §2.4:
/// structured-node extraction, never a regex over the message prose).
pub struct ChatEdgeProducer;

impl ChatEdgeProducer {
    /// The canonical Chat **message root** `myelin://<tenant>/chat/message/<message_id>` — the source
    /// of an unfurl/mention edge produced by a chat message. Built through Chat's OWN canonical mint
    /// codec ([`myelin_chat::subs::mint_message`]) so the root is grammatical BY CONSTRUCTION and Refs
    /// names the SAME root Chat mints (one mint, never a parallel literal). The `message_id` is the
    /// immutable ULID (a stable opaque id, §2 — never a positional index).
    ///
    /// # Errors
    ///
    /// Returns a grammar error when the tenant or message id cannot form a canonical reference.
    pub fn message_root(tenant: &str, message_id: &str) -> Result<ArtifactRef, ParseError> {
        // mint_message attaches a `message-<id>` #sub; the EDGE SOURCE is the #sub-stripped root (the
        // message artifact itself), so we strip back to the canonical root Refs stores edges against.
        mint_message(tenant, message_id).map(|minted| myelin_refs::strip_sub(&minted))
    }

    /// The canonical Chat **thread root** `myelin://<tenant>/chat/thread/<thread_root_id>` — the source
    /// of an unfurl edge produced within a thread. Built through Chat's OWN mint codec
    /// ([`myelin_chat::subs::mint_thread`]); the `thread_root_id` is the immutable ULID root.
    ///
    /// # Errors
    ///
    /// Returns a grammar error when the tenant or thread id cannot form a canonical reference.
    pub fn thread_root(tenant: &str, thread_root_id: &str) -> Result<ArtifactRef, ParseError> {
        mint_thread(tenant, thread_root_id).map(|minted| myelin_refs::strip_sub(&minted))
    }

    /// **Emit the reference (unfurl) edges of a real Chat message body, in the SAME outbox transaction
    /// as the Chat content write (contract 5.4 — emit-iff-committed, REF-D7 producer half).** `source`
    /// is the Chat message/thread root (built above); `body` is the structured `myelin-content`
    /// document of the message (its mention/artifact_ref/embed nodes); `content_event` is the
    /// `chat.message.created` / `chat.message.edited` event emitted in the SAME transaction (the CAUSE —
    /// the correlation root carries, `depth +1`, the loop-guard stamp). One `refs.edge.created` per
    /// structured ref node. Returns the minted ids.
    ///
    /// The edges become durable IFF the caller commits the Chat content transaction — an aborted /
    /// rolled-back message-send drops the buffered edge rows with it (no unfurl edge without its chat
    /// message). This is the SAME guarantee every producer proves; the only change is the real Chat
    /// caller + source URN — and that Chat is the MAXIMAL consumer (it unfurls every artifact class).
    pub fn emit_chat_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        body: &[InlineNode],
        content_event: &EventEnvelope,
    ) -> BusResult<Vec<EventId>> {
        // The ONE sanctioned producer seam (§4.1 producer #1; no standalone edge-write API). Refs
        // extracts one edge per structured node and emits via OutboxTx::emit — unchanged from REF-P8.
        emit_edges(tx, source, body, content_event)
    }

    /// The extracted (un-emitted) Chat unfurl edges of a message body — exposed so a drill can assert
    /// the edge SET a real Chat message produces (the leak/IDOR re-confirmation corpus, REF-D1/D2)
    /// without driving the outbox. Reuses the ONE [`crate::extract_edges`] seam.
    pub fn chat_edges(&self, source: &ArtifactRef, body: &[InlineNode]) -> Vec<EdgeDraft> {
        crate::extract_edges(source, body)
    }
}

/// The state of a Chat message / thread sub-anchor (§4.6). The owner ([`ChatOwner`]) records these; the
/// resolver maps them onto the frozen [`SubState`] so a real Chat anchor degrades through the SAME
/// ladder as a Git line-range / a KN block / a CI check / an Issues field.
///
/// A Chat `message-`/`thread-` id is the **immutable `message_id` / `thread_root_id` ULID** (the
/// stability obligation is Chat's, [`myelin_chat::subs`] §2 — content-addressed by a stable opaque id,
/// NEVER a positional index). So unlike a Git line-range (positional → MOVED/OUTDATED) or an Issues
/// field (editable value → OUTDATED), a Chat message has ONLY:
/// - **LIVE** — the message/thread exists (an EDITED message keeps its id → still LIVE; there is no
///   MOVED/OUTDATED arm for Chat, §4.6: "Chat message/thread anchors (immutable → LIVE; deleted →
///   GONE)"),
/// - **GONE** — the message/thread was DELETED (the root channel still resolves → `Tombstone{ sub_gone,
///   root }`; the embed shows the parent),
/// - **ERASED** — the message was crypto-shredded (the per-subject/per-tenant DEK destroyed) → ERASED.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatAnchorState {
    /// The message/thread resolves exactly — it exists (LIVE). An EDITED message keeps its immutable
    /// `message_id` and stays LIVE (no OUTDATED arm — Chat is content-addressed by stable id).
    Live,
    /// The message/thread was DELETED — the root still resolves, the sub anchor is gone (GONE ⇒
    /// `Tombstone{ sub_gone, root }`; the embed shows the parent channel/thread).
    Deleted,
    /// The message was ERASED (crypto-shred of the message's per-subject/per-tenant DEK) — unrenderable
    /// (ERASED ⇒ `Tombstone{ erased }`; supports CHAT-D5 confidential-unfurl → tombstone, 0 title leak).
    Erased,
}

impl ChatAnchorState {
    /// Map a Chat message/thread state onto the frozen §4.6 [`SubState`], carrying the owner projection
    /// on the LIVE arm. The ONE vocabulary — a Chat anchor degrades identically to every other
    /// sub-anchor; Chat simply does not use the MOVED/OUTDATED arms (immutable → LIVE; deleted → GONE).
    fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            ChatAnchorState::Live => SubState::Live(projection),
            ChatAnchorState::Deleted => SubState::Gone,
            ChatAnchorState::Erased => SubState::Erased,
        }
    }
}

/// **The REAL Chat owner — a [`ProjectApi`] + [`SubAnchorResolver`] over Chat's content (contracts
/// 5.6 / 5.7).** This is the producer half Refs' chokepoint calls when resolving a Chat `#sub`:
///
/// - a `message-<message_id>` message / `thread-<thread_root_id>` thread → the recorded
///   [`ChatAnchorState`] (live → LIVE, deleted → GONE, erased → ERASED),
/// - a bare root (no `#sub`) → LIVE (the channel/message/thread artifact itself).
///
/// Refs NEVER reads Chat's DB — it only calls this seam. The leak invariant is the chokepoint's: this
/// owner is reached ONLY on the permission-allowed branch (the chokepoint gates it). The `check_view`
/// verdict is Chat's authoritative permission decision (4.2 — production is Identity's `check` over the
/// Chat ReBAC fragment's `channel.read = member + parent_project->read`, REF-P323; here the recorded
/// ACL). So a non-member of a private channel is tombstoned, never leaked (the REF-D1 leak invariant on
/// the Chat corpus; supports CHAT-D5).
///
/// Cloneable: every map is held behind an [`Arc`] so a clone shares the SAME recorded state (the
/// resolve chokepoint holds the owner behind an `Arc<dyn ProjectApi>`; a clone lets the test record
/// into the same owner the service resolves through). Tenant-first; no cross-tenant key.
#[derive(Clone, Default)]
pub struct ChatOwner {
    /// `(tenant|region|principal|root)` → the authoritative `view` decision (4.2). The recorded ACL the
    /// production wire replaces with Identity's `check` over the resilient client (the Chat
    /// `channel.read = member + parent_project->read` fragment, REF-P323). Default-deny.
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    /// `full-ref-urn` → the Chat message/thread anchor state (the §4.6 sub-anchor state).
    anchors: Arc<Mutex<BTreeMap<String, ChatAnchorState>>>,
}

impl ChatOwner {
    /// A fresh Chat owner (default-deny ACL; no anchors recorded — an unscripted bare root is LIVE).
    pub fn new() -> ChatOwner {
        ChatOwner::default()
    }

    fn acl_key(
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        root: &ArtifactRef,
    ) -> String {
        format!(
            "{}|{}|{}|{}",
            tenant.0, region.0, viewer.principal_id.0, root.0
        )
    }

    /// Grant a viewer the `view` permission on a Chat root (the recorded ACL — the Chat ReBAC
    /// fragment's `channel.read = member + parent_project->read` grant, modelled here; production is
    /// Identity's `check`, REF-P323).
    pub fn grant_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        root: &ArtifactRef,
    ) {
        self.acl
            .lock()
            .unwrap()
            .insert(Self::acl_key(tenant, region, viewer, root), Decision::Allow);
    }

    /// Record a Chat message/thread anchor state (`message-`/`thread-`) — the §4.6 sub-anchor state. An
    /// edit that keeps the immutable id records [`ChatAnchorState::Live`] (no OUTDATED arm for Chat); a
    /// delete records [`ChatAnchorState::Deleted`]; a crypto-shred records [`ChatAnchorState::Erased`].
    pub fn record_anchor(&self, ref_: &ArtifactRef, state: ChatAnchorState) {
        self.anchors.lock().unwrap().insert(ref_.0.clone(), state);
    }

    /// The default owner projection a renderable Chat anchor carries (a render-safe title — the leak
    /// invariant already gates this; the owner is reached only on the allowed branch). PII-free.
    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "a chat message".into(),
            state: "live".into(),
            icon: "chat".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    /// Resolve a Chat `#sub` anchor on `ref_` into the frozen [`SubState`] (the §4.6 step-3 owner
    /// resolver — the SUB-ANCHOR RESOLUTION REF-P21 ships). Dispatched by the Chat `#sub` KIND through
    /// the ONE Refs grammar (`sub_kind`):
    /// - `message-<id>` message / `thread-<id>` thread → the recorded [`ChatAnchorState`] (live → LIVE,
    ///   deleted → GONE, erased → ERASED),
    /// - bare root → LIVE.
    fn resolve_chat_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            // A bare root (no #sub) is LIVE — the channel/message/thread itself, no sub-anchor to
            // degrade.
            None => SubState::Live(projection),
            // The Chat sub-anchor kinds — the recorded §4.6 state. A message/thread id is an IMMUTABLE
            // opaque ULID, so an edited message stays LIVE (no OUTDATED arm); only a deleted message is
            // GONE, a crypto-shredded one ERASED.
            Some(Sub::Message(_)) | Some(Sub::Thread(_)) => {
                self.anchors
                    .lock()
                    .unwrap()
                    .get(&ref_.0)
                    .copied()
                    // No recorded state for a sub Refs minted → defensively GONE (a real owner always
                    // has the mint-time state for a ref it minted; an unscripted anchor is treated as
                    // gone rather than guessed LIVE — REF-3, never resolve to content it cannot back).
                    .map(|s| s.into_sub_state(projection.clone()))
                    .unwrap_or(SubState::Gone)
            }
            // Any other kind on a Chat ref is not a Chat-owned mint — Chat renders the bare root LIVE
            // rather than guess (REF-3 — never guess scope; the grammar already rejected unknowns).
            Some(_) => SubState::Live(projection),
        }
    }
}

impl ProjectApi for ChatOwner {
    fn check_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        _permission: &Permission,
    ) -> std::result::Result<Decision, ProjectApiError> {
        // The authoritative `view` verdict on the ROOT (the chokepoint passes the #sub-stripped root).
        // Default-deny: an unrecorded grant is a Deny (so a viewer with no Chat read — e.g. a private
        // channel's non-member — is tombstoned, never leaked — the REF-D1 leak invariant on the Chat
        // corpus; supports the `channel.read = member + parent_project->read` fragment, REF-P323, and
        // CHAT-D5 confidential-unfurl → tombstone).
        let key = Self::acl_key(tenant, region, viewer, object);
        Ok(self
            .acl
            .lock()
            .unwrap()
            .get(&key)
            .copied()
            .unwrap_or(Decision::Deny))
    }

    fn project(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _mode: ResolveMode,
    ) -> std::result::Result<ProjectOutcome, ProjectApiError> {
        // The owner classifies the Chat #sub into the frozen ProjectOutcome through the ONE ladder
        // mapping (SubState::into_outcome). Called ONLY on the permission-allowed branch.
        let sub = sub_kind(ref_);
        Ok(self.resolve_chat_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for ChatOwner {
    /// The §4.6 step-3 sub-anchor resolver (the SAME logic `project` runs) — exposed so a drill can
    /// drive the ladder directly (REF-D9) through [`crate::resolve_sub_outcome`] without the full
    /// chokepoint. ONE source of truth: it delegates to [`ChatOwner::resolve_chat_sub`].
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_chat_sub(ref_, sub)
    }
}

#[cfg(test)]
mod tests;
