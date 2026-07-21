//! # `project` — `project(ref, viewer)` for chat/{channel,message,thread} (CHAT-P15 / P-409, M4-C4)
//!
//! The **producer half** of contract 5.6: the ONLY way another subsystem reads about a chat artifact
//! (no cross-DB). Refs `resolve(ref, viewer, mode)` (the 5.2 chokepoint the
//! [`crate::unfurl::RefsResolvePort`] CONSUMES) calls back into THIS `project(ref, viewer)` to obtain
//! a per-viewer, pre-permission-checked `Projection | Tombstone` — **never the body**. So an issue or
//! a doc that references "discussed in #incidents" can unfurl a chat message **without reading Chat's
//! DB** (arch 03 §3); chat is a *participant* in the reference graph, not a silo.
//!
//! This is the third committable unit of milestone **M4-C4** (the per-viewer unfurl service): the
//! cache + per-viewer gate is [`crate::unfurl`] (CHAT-P13), the bus-driven invalidation + erasure-safe
//! re-render + `#sub` anchor stability is [`crate::unfurl::invalidation`] (CHAT-P14), and the
//! `project()` + edge-producer slice is THIS module (CHAT-P15). The `refs.edge.created` densest-producer
//! half (the three structured content nodes + the `chat.channel.linked` "discussed in" edge) is
//! ALREADY built — [`crate::content`] (the `mention`/`artifact_ref`/`embed` → edge mapping, co-committed
//! with the message content event) + [`crate::membership`] (the `chat.channel.linked` lifecycle event);
//! see [`densest_edge_producer`] for the chat-the-densest-producer assertion this prompt adds.
//!
//! ## Owning architecture (read in full before changing this)
//! - `04-subsystem-architectures/chat/architecture/03-events-contracts-and-glue.md` §3
//!   (`project(ref, viewer)` — the per-viewer pre-permission-checked projection, never the body; the
//!   frozen `{title, state, icon, render_hint, sub_anchor?}` shape; the per-type `render_hint` =
//!   `ChannelChip`/`MessageChip`/`ThreadChip`; the title humanised via `humanise`, 7.3) + §4 (chat the
//!   densest `refs.edge.created` producer) + §1.1 (the `chat.channel.linked` event).
//! - `05-refined-shared-systems-architecture/00-reconciliation-decisions.md` §OQ-I (cross-cell
//!   resolution is ALWAYS cell-local — `project()` never reaches across cells).
//! - `contract-index.md` rows 5.6 (project REQUIRED on every subsystem — chat implements it for
//!   chat/{channel,message,thread}), 4.2 (check — the per-viewer gate), 7.3 (humanise — the title).
//!
//! ## The frozen 4-step tombstone ladder (contract 5.7) — chat's outcomes
//! permission → root → (erased/restricted) → sub-resolve. For a CHAT ref the producible outcomes are
//! **live / gone / erased** (a message is content-addressed by a stable id, no moved/outdated — arch
//! §2): a denied viewer gets a `Denied` tombstone (the title NEVER read — the 0-leak gate); a gone
//! root/sub gets a `Gone` tombstone carrying the root; an erased (or restricted) subject gets an
//! `Erased` tombstone, "[erased]". A tombstone ALWAYS carries the `#sub`-stripped ROOT (so a broken
//! sub-anchor still points at the parent channel — arch §2).
//!
//! ## The no-body invariant (the project-leak gate; cargo-mutants mandatory-core)
//! [`Projection`] structurally carries NO body field — only `{title, state, icon, render_hint,
//! sub_anchor?}`. The title is a permission-gated one-line PREVIEW / channel label, NEVER the message
//! body bytes (those are per-subject-DEK ciphertext at rest, [`crate::dek`], and are never on this
//! path). The permission check runs FIRST (the deny path reads NO artifact field), so a denied viewer's
//! title is never even fetched. The mandatory-core mutation floor for this module is the **never-the-body
//! property**: a survived mutant that flips a `Deny` to an `Allow`, or returns a body where a title
//! belongs, is a leak — 0 surviving mutants on the gate decision + the projection build.
//!
//! ## FLOOR named (VISION §3 — name-your-floors)
//! - **`project()` resolution is ALWAYS cell-local (OQ-I).** A viewer in another cell resolving a chat
//!   ref homed here gets the already-permission-filtered projection, never raw rows; the cross-cell
//!   pointer follow-on (cross-org channels, CHAT-P30 / contract 12.6) consumes the bridge, NOT
//!   `project()` directly. `project()` reads ONLY the home-cell store ([`ChatProjectionSource`]); it
//!   never reaches across cells. NO new floor is introduced by this prompt.
//! - **The live-OLTP projection source.** [`ChatProjectionSource`] is the in-memory model of the
//!   `conversation` + `message` OLTP rows (CHAT-P7/P5) the live projector reads — the SAME entity shapes
//!   the stores hydrate. The wire-up to the live [`crate::conversation::ConversationStore`] +
//!   [`crate::store::MessageStore`] (decrypt-then-one-line-preview the body for the title) is the
//!   service-assembly step; here the source is the seam, so the no-body + per-viewer-gate PROPERTIES are
//!   proven over the EXACT contract shape (the SAME posture as Knowledge's `PageStore` projector floor).

use std::collections::{HashMap, HashSet};

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_notif::{render_message, TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT};
use myelin_refs::ArtifactRef;

use crate::glue::{TPL_CHAT_PROJECT_CHANNEL, TPL_CHAT_PROJECT_MESSAGE, TPL_CHAT_PROJECT_THREAD};
use crate::membership::{channel_object, permissions};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE FROZEN PROJECTION SHAPE (contract 5.6 — {title, state, icon, render_hint, sub_anchor?})
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The frozen `render_hint` chip a chat projection renders (arch 03 §3 — `ChannelChip`/`MessageChip`/
/// `ThreadChip`). The cross-subsystem card renderer picks the chip from this token; a PII-free
/// `&'static str` token, never a literal at the call sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderHint {
    /// `chat/channel` → `ChannelChip` (a channel name/topic card).
    ChannelChip,
    /// `chat/message` → `MessageChip` (a one-line message preview card).
    MessageChip,
    /// `chat/thread` → `ThreadChip` (a thread root + reply-count card).
    ThreadChip,
}

impl RenderHint {
    /// The frozen render-hint token (arch §3 vocabulary).
    pub fn as_str(self) -> &'static str {
        match self {
            RenderHint::ChannelChip => "ChannelChip",
            RenderHint::MessageChip => "MessageChip",
            RenderHint::ThreadChip => "ThreadChip",
        }
    }

    /// The frozen `icon` token (the kind glyph the card draws — arch §3 `icon`). Chat keeps the icon
    /// per-TYPE (channel/message/thread); the author-kind glyph is the renderer's per-author overlay.
    pub fn icon(self) -> &'static str {
        match self {
            RenderHint::ChannelChip => "channel",
            RenderHint::MessageChip => "message",
            RenderHint::ThreadChip => "thread",
        }
    }
}

/// **A per-viewer chat projection (contract 5.6) — `{title, state, icon, render_hint, sub_anchor?}`.**
/// Built ONLY after the per-viewer permission check passes (the deny path returns a [`Tombstone`]
/// instead, never this). Structurally carries **NO body field** — the no-body invariant is by
/// construction: a projection is a title (a permission-gated one-line preview / channel label) + render
/// metadata, NEVER the message body bytes. This is the shape Refs returns from `resolve` after calling
/// chat's `project()`; every ALLOWED viewer of a NON-`#sub` ref sees the SAME projection (the per-viewer
/// decision is the gate, not the content).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The humanised display title (contract 7.3 — the channel name/topic, the message one-line
    /// preview, or the thread root preview + reply-count). PERMISSION-GATED: read only AFTER the check
    /// passes; NEVER the body. Routed through the ONE templating surface (the registered
    /// `chat.project.*` keys), so a tenant brands/localises it.
    pub title: String,
    /// The artifact's state token (`active`/`archived` for a channel; `active`/`edited`/`deleted` for a
    /// message; the thread root's state). Render-time, viewer-independent.
    pub state: String,
    /// The icon/type glyph token ([`RenderHint::icon`]).
    pub icon: String,
    /// The frozen per-type render hint (arch §3 — ChannelChip/MessageChip/ThreadChip).
    pub render_hint: RenderHint,
    /// The stable opaque `#sub` anchor the projection resolved (the `message-<id>` / `thread-<root>`
    /// opaque body, contract 5.7) — `Some` for a `#sub`-precise message/thread ref, `None` for a
    /// bare-root channel ref.
    pub sub_anchor: Option<String>,
}

/// Why a projection degraded to a [`Tombstone`] — the 4-step ladder reasons (contract 5.7 / arch §2).
/// The tombstone always carries the root; the reason is the audit fact + the card's render mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// Ladder step 1 — the per-viewer PERMISSION gate denied. The title was NEVER fetched (the 0-leak
    /// chokepoint). Renders "a restricted <type>".
    Denied,
    /// Ladder step 2/4 — the referenced root/sub is gone (deleted, not erased). Renders "this referenced
    /// conversation (the specific part is no longer available)".
    Gone,
    /// Ladder step 4 (erased) — the artifact (or a third party in it) was erased (crypto-shred), OR the
    /// subject is GDPR-restricted (the suppression window degrades to the SAME content-free tombstone).
    /// Renders "[erased]".
    Erased,
}

/// **A tombstone (contract 5.2 `Tombstone`) — the leak-free degradation of a chat ref.** Carries the
/// `#sub`-stripped ROOT (so a broken sub-anchor still points at the parent channel — arch §2) + the
/// reason. NEVER the title, NEVER the body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// The ROOT artifact ref (the `#sub`-stripped parent). NEVER the title.
    pub root: ArtifactRef,
    /// Why the ref degraded (permission / gone / erased).
    pub reason: TombstoneReason,
}

/// **The result of [`Projector::project`]: a per-viewer [`Projection`] or a [`Tombstone`] (contract
/// 5.6 — `Projection | Tombstone`).** A projector NEVER returns a bare title to an unauthorised viewer:
/// the two-variant shape IS the contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    /// Authorised + present — the per-viewer projection (the gate passed; the ladder resolved live).
    Visible(Projection),
    /// Denied / gone / erased — the content-free tombstone carrying the root.
    Tombstoned(Tombstone),
}

impl Projected {
    /// `true` iff this is a tombstone (the leak-free degradation). A drill asserts a denied viewer's
    /// result is a tombstone.
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    /// The title the projection exposes, if any — `None` for a tombstone (the leak-free invariant: a
    /// tombstone exposes NO title). A drill asserts this is `None` for a denied viewer (0 title leak).
    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

/// A loud, typed projection error — a malformed / non-chat ref (distinct from a [`Tombstone`], which is
/// the leak-free degradation of a VALID chat ref). A non-chat ref is a programming error in the caller
/// (Refs routes a ref to its owner's `project()`); it is surfaced LOUDLY, never silently tombstoned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The ref is not a `chat/*` artifact (the wrong owner was asked to project it).
    NotChat {
        /// The subsystem token parsed from the ref (`issue`/`git`/…).
        subsystem: String,
    },
    /// The ref's `<type>` is not one of chat's projectable types (channel/message/thread).
    UnknownChatType {
        /// The type token parsed from the ref.
        ty: String,
    },
    /// The ref is not a parseable canonical `myelin://<tenant>/chat/<type>/<id>` URN.
    Malformed {
        /// The offending ref string.
        reference: String,
    },
}

impl core::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectError::NotChat { subsystem } => {
                write!(f, "not a chat artifact (subsystem `{subsystem}`)")
            }
            ProjectError::UnknownChatType { ty } => {
                write!(
                    f,
                    "`{ty}` is not a projectable chat type (channel/message/thread)"
                )
            }
            ProjectError::Malformed { reference } => {
                write!(f, "malformed chat ref `{reference}`")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// The projectable chat artifact type (the `<type>` segment of a `chat/<type>/<id>` ref).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatType {
    Channel,
    Message,
    Thread,
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE PROJECTION SOURCE (the live-OLTP-store FLOOR — in-memory here, keyed by canonical ROOT ref)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A channel's projectable metadata (the `conversation` row's projection inputs, CHAT-P7). PII-free
/// label + state; the body bytes are never here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMeta {
    /// The channel display label — `name` if present, else `"#" + topic` (arch §3). The projection
    /// title is humanised FROM this label; it is read only AFTER the per-viewer check passes.
    pub label: String,
    /// `true` iff the channel is archived (the `chat.channel.archived` end-state → state `archived`).
    pub archived: bool,
}

/// A message's projectable metadata (the `message` row's projection inputs, CHAT-P5). The `preview` is
/// the DECRYPTED one-line preview of the body the live projector derives (the per-subject-DEK unseal +
/// the first-line truncate is the service step; here it is the input) — it is NEVER the full body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageMeta {
    /// The home channel id (the conversation this message lives in) — the `channel.read` gate object a
    /// message inherits (a message is never more visible than its channel).
    pub channel_id: String,
    /// The one-line body PREVIEW (already truncated/decrypted by the live projector) — the humanised
    /// title source. NEVER the full body bytes (the no-body invariant).
    pub preview: String,
    /// The message lifecycle state token (`active`/`edited`/`deleted`).
    pub state: String,
}

/// A thread root's projectable metadata (the thread-root `message` row + the reply tally). The
/// `root_preview` is the decrypted one-line preview of the thread's root message; `reply_count` is the
/// reply tally (pluralised in the title through the ICU subset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadMeta {
    /// The home channel id — the `channel.read` gate object the thread inherits.
    pub channel_id: String,
    /// The one-line preview of the thread root message (already decrypted/truncated). NEVER the body.
    pub root_preview: String,
    /// The reply tally (the read-fanout count, arch §4). Pluralised in the title.
    pub reply_count: u32,
    /// The thread root state token.
    pub state: String,
}

/// **The chat projection source — the live-OLTP read (the CHAT-P5/P7 store FLOOR, in-memory here).**
/// Keyed by the canonical ROOT `ArtifactRef` string of each chat artifact — the SAME entity shapes the
/// live `conversation`/`message` stores hydrate, so the projection logic is store-agnostic (the live
/// wire-up is the service-assembly step; here the source is the seam). Carries the erased/restricted
/// sets the §2.1 erasure-/restriction-safe tombstone reads. **Cell-local by construction (OQ-I): the
/// source holds ONLY this cell's rows; `project()` never reaches across cells.**
#[derive(Clone, Debug, Default)]
pub struct ChatProjectionSource {
    /// Channel metadata by canonical channel ROOT ref string.
    channels: HashMap<String, ChannelMeta>,
    /// Message metadata by canonical message ROOT ref string.
    messages: HashMap<String, MessageMeta>,
    /// Thread metadata by canonical thread ROOT ref string.
    threads: HashMap<String, ThreadMeta>,
    /// Canonical ref strings (root OR sub-URN) that are ERASED (a `*.erased` tombstone) — projecting one
    /// returns an `Erased` tombstone, never the shredded content (§2.1 step 4).
    erased: HashSet<String>,
    /// Canonical ref strings whose subject is GDPR-RESTRICTED — projecting one returns a tombstone (the
    /// restriction-window suppression, arch §10 / §6).
    restricted: HashSet<String>,
}

impl ChatProjectionSource {
    /// A fresh, empty source.
    pub fn new() -> ChatProjectionSource {
        ChatProjectionSource::default()
    }

    /// Insert a channel's projectable metadata keyed by its canonical channel ROOT ref.
    pub fn put_channel(&mut self, root: &ArtifactRef, meta: ChannelMeta) {
        self.channels.insert(root.0.clone(), meta);
    }

    /// Insert a message's projectable metadata keyed by its canonical message ROOT ref.
    pub fn put_message(&mut self, root: &ArtifactRef, meta: MessageMeta) {
        self.messages.insert(root.0.clone(), meta);
    }

    /// Insert a thread root's projectable metadata keyed by its canonical thread ROOT ref.
    pub fn put_thread(&mut self, root: &ArtifactRef, meta: ThreadMeta) {
        self.threads.insert(root.0.clone(), meta);
    }

    /// Mark a canonical ref (root or sub-URN) ERASED (a `*.erased` tombstone, §2.1 step 4).
    pub fn mark_erased(&mut self, reference: &ArtifactRef) {
        self.erased.insert(reference.0.clone());
    }

    /// Mark a canonical ref's subject RESTRICTED (the GDPR `restrict` flag, arch §10).
    pub fn mark_restricted(&mut self, reference: &ArtifactRef) {
        self.restricted.insert(reference.0.clone());
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE PROJECTOR — project(ref, viewer): the per-viewer 4-step ladder, permission FIRST, no body
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The chat `project(ref, viewer)` projector (contract 5.6 — the CHAT-P15 deliverable).** The ONLY
/// way Refs/Search/Notif read a chat artifact (no cross-DB). Holds the [`IdentityService`] (the
/// per-viewer permission source — the SAME `check` the unfurl gate runs) + the [`ChatProjectionSource`]
/// (the own-DB read, the CHAT-P5/P7 store floor in-memory here) + the [`TemplateStore`] (the ONE
/// templating surface the title is humanised through, contract 7.3). Generic over `I: IdentityService`
/// so the service wires the real Id resolver and tests wire a deterministic one (chat is a CONSUMER —
/// the §2.9 DAG stays acyclic).
pub struct Projector<I: IdentityService> {
    id: I,
    source: ChatProjectionSource,
    templates: TemplateStore,
}

impl<I: IdentityService> Projector<I> {
    /// Compose the projector over Identity (the per-viewer gate) + the projection source + the
    /// templating surface (with chat's `chat.project.*` title keys registered — the caller seeds it via
    /// [`crate::glue::register_chat_humanise_templates`]).
    pub fn new(id: I, source: ChatProjectionSource, templates: TemplateStore) -> Projector<I> {
        Projector {
            id,
            source,
            templates,
        }
    }

    /// A mutable borrow of the underlying source (for the service / drills to seed or inspect).
    pub fn source_mut(&mut self) -> &mut ChatProjectionSource {
        &mut self.source
    }

    /// **`project(ref, viewer) -> Projection | Tombstone` (contract 5.6 / 5.7, the per-viewer 4-step
    /// tombstone ladder).** The load-bearing invariant (the project-leak gate):
    ///
    /// 1. **classify** — a non-chat / unknown-type ref is a LOUD [`ProjectError`] (never a tombstone —
    ///    the wrong owner was asked to project).
    /// 2. **permission FIRST** — `check(viewer, read, channel-object)` against the channel the artifact
    ///    lives in (a message/thread inherits its channel's `channel.read`; a channel gates on itself).
    ///    Deny / Conditional / Id-error ALL fail CLOSED to a `Denied` tombstone carrying the ROOT — NO
    ///    artifact field read (0 leak — the title is never even fetched).
    /// 3. **erased / restricted (any level)** — checked BEFORE the body/preview is read so a shredded or
    ///    suppressed subject never renders ⇒ an `Erased` tombstone.
    /// 4. **root resolve** — the channel/message/thread exists in this cell's source? No ⇒ a `Gone`
    ///    tombstone carrying the root.
    ///
    /// A tombstone ALWAYS carries the `#sub`-stripped root (arch §2). The `zookie` is the
    /// read-consistency fence — a STRONG, zookie-stamped read (the conversation's stamped `acl_zookie`
    /// the new-enemy guard advances), so a just-revoked grant is denied immediately. **Cell-local
    /// (OQ-I): the source is this cell's; `project()` never reaches across cells.**
    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        let ty = classify(reference)?;
        let root = myelin_refs::strip_sub(reference);

        // ── STEP 2: the gate object — the CHANNEL the artifact lives in (a message/thread is never more
        //    visible than its home channel; a channel gates on itself). Resolved BEFORE the permission
        //    check so the deny path reads no projectable field of the artifact — only the (opaque)
        //    channel id the message/thread's metadata names. A message/thread whose home channel is not
        //    in this cell's source is GONE (the root resolve at step 4 cannot find it either).
        let channel_id = match self.gate_channel_id(ty, &root) {
            Some(c) => c,
            None => {
                // The artifact's home channel is unknown here → the root is gone (carry the root, never
                // a title). The same outcome step 4 would reach; resolved here so the gate has an object.
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Gone,
                    root,
                }));
            }
        };

        // ── STEP 2 (cont.): PERMISSION FIRST (the 0-leak gate). check(viewer, read, channel:<id>) at a
        //    STRONG, zookie-stamped read. A Deny / Conditional / Id-error ALL fail closed to a `Denied`
        //    tombstone carrying the ROOT, with NO artifact field read.
        let object = myelin_tenancy::ArtifactRef(channel_object(&channel_id));
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(permissions::READ.to_string());
        match self.id.check(viewer, &permission, &object, &at, None) {
            Ok(Decision::Allow) => { /* authorised — descend the ladder */ }
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Denied,
                    root,
                }));
            }
        }

        // ── STEP 3: ERASED / RESTRICTED (checked early so a shredded/suppressed subject never renders).
        //    Keyed on BOTH the root and the full ref — an erased channel tombstones its messages/threads
        //    too. Restriction (the GDPR suppression window, arch §10) degrades to the SAME content-free
        //    tombstone.
        if self.source.erased.contains(&root.0) || self.source.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
                root,
            }));
        }
        if self.source.restricted.contains(&root.0) || self.source.restricted.contains(&reference.0)
        {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
                root,
            }));
        }

        // ── STEP 4: ROOT RESOLVE + BUILD THE PROJECTION (only now read the title/preview — the deny path
        //    never did). The `#sub` opaque body is carried as the sub_anchor for a sub-precise ref.
        let sub_anchor = sub_opaque(reference);
        match ty {
            ChatType::Channel => {
                let meta = match self.source.channels.get(&root.0) {
                    Some(m) => m.clone(),
                    None => return Ok(self.gone(root)),
                };
                let title = self.humanise_title(TPL_CHAT_PROJECT_CHANNEL, vec![meta.label], viewer);
                Ok(Projected::Visible(Projection {
                    title,
                    state: channel_state(meta.archived).to_string(),
                    icon: RenderHint::ChannelChip.icon().to_string(),
                    render_hint: RenderHint::ChannelChip,
                    sub_anchor,
                }))
            }
            ChatType::Message => {
                let meta = match self.source.messages.get(&root.0) {
                    Some(m) => m.clone(),
                    None => return Ok(self.gone(root)),
                };
                let title =
                    self.humanise_title(TPL_CHAT_PROJECT_MESSAGE, vec![meta.preview], viewer);
                Ok(Projected::Visible(Projection {
                    title,
                    state: meta.state,
                    icon: RenderHint::MessageChip.icon().to_string(),
                    render_hint: RenderHint::MessageChip,
                    sub_anchor,
                }))
            }
            ChatType::Thread => {
                let meta = match self.source.threads.get(&root.0) {
                    Some(m) => m.clone(),
                    None => return Ok(self.gone(root)),
                };
                let title = self.humanise_title(
                    TPL_CHAT_PROJECT_THREAD,
                    vec![meta.root_preview, meta.reply_count.to_string()],
                    viewer,
                );
                Ok(Projected::Visible(Projection {
                    title,
                    state: meta.state,
                    icon: RenderHint::ThreadChip.icon().to_string(),
                    render_hint: RenderHint::ThreadChip,
                    sub_anchor,
                }))
            }
        }
    }

    /// The `channel:<id>` the per-viewer gate checks for a ref of type `ty` rooted at `root`. A channel
    /// gates on its OWN id (the `<id>` segment of the root ref); a message/thread inherits its home
    /// channel's id from its metadata (a message/thread is never more visible than its channel). Returns
    /// `None` iff the artifact's home channel cannot be resolved in this cell's source (→ a Gone
    /// tombstone, the artifact does not exist here).
    fn gate_channel_id(&self, ty: ChatType, root: &ArtifactRef) -> Option<String> {
        match ty {
            ChatType::Channel => ref_id(root),
            ChatType::Message => self
                .source
                .messages
                .get(&root.0)
                .map(|m| m.channel_id.clone()),
            ChatType::Thread => self
                .source
                .threads
                .get(&root.0)
                .map(|m| m.channel_id.clone()),
        }
    }

    /// Humanise a projection title through the ONE templating surface (contract 7.3): look up the
    /// registered `chat.project.*` body for the viewer's locale and ICU-format it with the PII-free,
    /// already-permission-gated slots ([`render_message`]). An unregistered key degrades to the raw
    /// first slot (never a panic, never a chat-local `format!`) — the honest fallback.
    fn humanise_title(&self, key: &str, slots: Vec<String>, _viewer: &Principal) -> String {
        // The viewer's locale selects the branded/localised row; v1 uses the platform-default locale
        // (the per-viewer locale plumb is the live-render step — the SAME locale the unfurl card uses).
        let body = self
            .templates
            .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
            .map(|t| t.body.clone())
            // An unregistered key degrades to the first slot verbatim — never a panic, never a leak
            // (the slot is already permission-gated).
            .unwrap_or_else(|| slots.first().cloned().unwrap_or_default());
        render_message(&body, &slots)
    }

    /// A `Gone` tombstone carrying the root (the root does not resolve in this cell's source).
    fn gone(&self, root: ArtifactRef) -> Projected {
        Projected::Tombstoned(Tombstone {
            reason: TombstoneReason::Gone,
            root,
        })
    }
}

/// The channel `state` token for a channel projection (`active`/`archived`).
fn channel_state(archived: bool) -> &'static str {
    if archived {
        "archived"
    } else {
        "active"
    }
}

/// Classify a canonical chat ref's `<type>` (channel/message/thread) — a non-chat subsystem or an
/// unknown chat type is a LOUD [`ProjectError`] (the wrong owner / a type chat does not project). Parses
/// the `myelin://<tenant>/chat/<type>/<id>` scope from the `#sub`-stripped root.
fn classify(reference: &ArtifactRef) -> Result<ChatType, ProjectError> {
    let root = myelin_refs::strip_sub(reference);
    let segments = scope_segments(&root).ok_or_else(|| ProjectError::Malformed {
        reference: reference.0.clone(),
    })?;
    let (subsystem, ty) = (segments.1, segments.2);
    if subsystem.as_str() != crate::subs::CHAT_SUBSYSTEM {
        return Err(ProjectError::NotChat { subsystem });
    }
    match ty.as_str() {
        "channel" => Ok(ChatType::Channel),
        "message" => Ok(ChatType::Message),
        "thread" => Ok(ChatType::Thread),
        _ => Err(ProjectError::UnknownChatType { ty }),
    }
}

/// The `(tenant, subsystem, type, id)` scope segments of a canonical `myelin://…` root ref, or `None`
/// if the string is not a four-segment canonical scope. A pure string split (the ref came from
/// [`myelin_refs::parse`], so it is canonical; this reads its segments without re-validating the
/// grammar). The `#sub` (if any) must already be stripped by the caller.
fn scope_segments(root: &ArtifactRef) -> Option<(String, String, String, String)> {
    let rest = root.0.strip_prefix(myelin_refs::SCHEME)?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 4 || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    Some((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
        parts[3].to_string(),
    ))
}

/// The `<id>` segment of a canonical chat root ref (the channel id for a `chat/channel/<id>` ref).
fn ref_id(root: &ArtifactRef) -> Option<String> {
    scope_segments(root).map(|s| s.3)
}

/// The opaque `#sub` body of a chat ref (`message-<id>` → `<id>`, `thread-<root>` → `<root>`), or
/// `None` for a bare root. Chat resolves only the `message-`/`thread-` kinds (the kinds it mints,
/// [`crate::subs`]); a foreign sub on a chat ref carries its opaque body verbatim.
fn sub_opaque(reference: &ArtifactRef) -> Option<String> {
    use myelin_refs::Sub;
    match myelin_refs::sub_kind(reference)? {
        Sub::Message(id) | Sub::Thread(id) => Some(id),
        Sub::Comment(id)
        | Sub::Block(id)
        | Sub::Heading(id)
        | Sub::Row(id)
        | Sub::Field(id)
        | Sub::Check(id) => Some(id),
        Sub::CommitCheck {
            commit_oid,
            context,
        } => Some(format!("commit-{commit_oid}/check-{context}")),
        Sub::CommitCiResult { commit_oid } => Some(format!("commit-{commit_oid}/ci-result")),
        Sub::Step(n) => Some(n.to_string()),
        Sub::LineRange { start, end } => Some(format!("L{start}-L{end}")),
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. CHAT THE DENSEST refs.edge.created PRODUCER (contract 5.4) — the assertion this prompt adds
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **Chat as the densest `refs.edge.created` producer (contract 5.4 / arch §4) — the uniform producer
/// surface.** The edge-PRODUCTION machinery is ALREADY built and co-committed with the outbox:
///
/// - the three structured content nodes (`mention`/`artifact_ref`/`embed`) in EVERY message body emit
///   one `refs.edge.created` each ([`crate::content::extract_body_edges`] /
///   [`crate::content::emit_body_edges`]) — the dense per-message edge stream; and
/// - an artifact-linked channel emits `chat.channel.linked` → a "discussed in" `refs.edge.created`
///   ([`crate::membership::MembershipService::link_channel`]).
///
/// This function is the UNIFORMITY witness the CHAT-P15 gate asserts: for a fixture corpus of bodies,
/// chat produces an edge for EVERY structured node with **0 missing edges** (N structured nodes → N
/// edges, in body order) — extraction is structured (the enum variant), never a regex over prose, so a
/// `@alice` in a code span or a `myelin://…` URL written as text is NOT an edge. Returns the total edge
/// count across the corpus (the density measure the drill asserts equals the structured-node count).
pub fn densest_edge_producer(
    source: &ArtifactRef,
    corpus: &[crate::content::MessageBody],
) -> usize {
    corpus
        .iter()
        .map(|body| crate::content::extract_message_edges(source, body).len())
        .sum()
}

#[cfg(test)]
mod tests;
