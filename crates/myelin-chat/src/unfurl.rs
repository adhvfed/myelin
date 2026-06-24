//! # `unfurl` — the Unfurl Service: the shared per-ref projection cache + the per-viewer
//! `list_objects`/`check` gate (the no-leak floor) (CHAT-P13 / P-407, M4-C4)
//!
//! The cache + per-viewer-gate slice of milestone **M4-C4** ("the unfurl service: cheap per-viewer
//! permission-aware unfurls" — planning/06-roadmaps/subsystems/chat.md §4). This is the **no-leak
//! core**: a confidential artifact referenced in a chat message renders a **tombstone** to a viewer
//! who lacks access — the **title NEVER leaks** (CHAT-D5). The erasure-safe + bus-invalidation +
//! anchor-stability slice is CHAT-P14; the `project(ref, viewer)` + edge-producer slice is CHAT-P15.
//!
//! ## The architecture this conforms to (02-internals-and-algorithms.md §4)
//! The Unfurl Service is a **Chat-owned cache + orchestration layer in FRONT of Refs `resolve`** — it
//! does NOT re-implement permission-aware resolution (contract 5.2 is the non-leaking chokepoint; EI-01
//! §7 — chat never re-implements the third copy). Chat's job is to make the per-viewer call CHEAP at
//! chat density. The layered cheapening (§4):
//!
//! 1. **Lazy-on-viewport (§4.1).** A virtualised timeline resolves unfurls ONLY for messages in the
//!    viewport ([`UnfurlService::unfurl_viewport`]). A scroll-back of 10 000 messages resolves a
//!    handful of cards, not 10 000 — the naïve "resolve every ref in the channel" trap is defeated.
//! 2. **Split the cache by viewer-varying vs. viewer-independent, over the 4-step ladder (§4.2).** The
//!    PER-VIEWER part is the `check`/`list_objects` gate (the platform's fast primitive); the
//!    VIEWER-INDEPENDENT part is the projection CONTENT — cached **ONCE per `ArtifactRef`, never per
//!    `(ref, viewer)`** ([`UnfurlCache`]). Content is returned ONLY after the per-viewer gate passes,
//!    so there is **one shared cache entry per ref, no leak** ([`UnfurlCache::entry_count`]).
//! 3. **Membership-as-permission class precompute (§4.3, OQ-E).** The per-viewer gate lowers the
//!    frozen `list_objects` `SetExpr` (contract 4.3) to a SQL predicate / JOIN over the unfurl
//!    candidate id column ([`gate::lower_over_unfurl_candidate`]) — **no N+1, no post-filter**. For a
//!    public channel "can a member see this project artifact?" is often ONE coarse class, not N checks.
//!
//! ## The 4-step tombstone ladder (contract 5.7) — chat's outcomes
//! Every ref resolves through the ONE frozen 4-step ladder (5.7): permission → root → sub-resolve →
//! erased. For CHAT refs (a message is content-addressed by a stable id minted at send) the ladder
//! outcomes are **live / gone / erased** — a message has no `moved`/`outdated` (it is immutable; an
//! edit is a NEW state on the SAME id, a tombstone is a `gone`/`erased`). The outcome enum
//! [`LadderOutcome`] carries the full ladder so a NON-chat ref (an Issue/PR/KN page embedded in a chat
//! message) still degrades through `Moved`/`Outdated` — chat consumes the ladder, it does not narrow it.
//!
//! ## The no-leak property (05-hard-problems.md §4 — the subtlety that separates real from demo)
//! The cache is keyed by the `ArtifactRef` ALONE — there is structurally NO `(ref, viewer)` key, so a
//! viewer's permission decision can never be baked into a shared cache entry. The gate runs BEFORE the
//! cache is touched ([`UnfurlService::resolve_one`]): a `Deny` returns a [`Card::Tombstone`] WITHOUT
//! reading the cache (the title is never even fetched for a denied viewer), and a cache HIT after an
//! `Allow` returns the SAME viewer-independent content every allowed viewer sees. The title of a
//! confidential artifact is therefore unreachable for a denied viewer (CHAT-D5; 0 title leak).
//!
//! ## FLOORS named (VISION §3 — name-your-floors)
//! - **The Refs `resolve` chokepoint (contract 5.2; REF-P10).** The production binding of
//!   [`RefsResolvePort`] is Refs' `resolve(ref, viewer, mode) -> Projection | Tombstone` over the
//!   resilient client (contract 1.9). It does not yet exist in `myelin-refs` (the resolver half is the
//!   named Refs floor). Here the port is the seam; the in-memory test resolver models its EXACT
//!   `Projection | Tombstone` contract so the no-leak PROPERTY is proven structurally (the SAME
//!   pattern `drill_chat_d5_humanise_leak.rs` uses for the Notif seam). The wire-up to the real
//!   resolver + the resilient-client degradation (§4.5) is CHAT-P15 (the `project()` slice).
//! - **Bus-driven cache invalidation (§4.4) + erasure-safe re-render (§4.6) + #sub anchor stability**
//!   is **CHAT-P14** (CHAT-D6/D7/D18). Here the cache has a short TTL backstop only; the precise
//!   bus-bust consumer lands in CHAT-P14.
//! - **THE CANVAS = AN EMBEDDED/PINNED KNOWLEDGE PAGE, NOT A CHAT EDITOR (M4-C4 lean, firm).** A chat
//!   "canvas" is an [`InlineNode::Embed`](myelin_content::InlineNode::Embed) `ArtifactRef` to a
//!   Knowledge page — it unfurls through THIS service like any other ref. Chat ships NO canvas EDITOR
//!   (no chat-side block model, no chat-side collab transport). The lean is **embed, not editor**
//!   (M4/M5, M5-C-X2-adjacent). Stated here so no agent builds a chat-side canvas editor.
//!
//! ## The mutation floor (the no-leak property; cargo-mutants mandatory-core)
//! The per-viewer gate ([`gate`]) + the gate-before-cache order ([`UnfurlService::resolve_one`]) are
//! the **mandatory-core** no-leak surface: the cargo-mutants floor for this module is **0 surviving
//! mutants on the gate decision + the deny-skips-cache path** (a survived mutant that flips a `Deny`
//! to an `Allow`, or reads the cache before the gate, is a title leak). Asserted by the chained drill
//! (`drill_chat_d5_unfurl_no_leak.rs`: resolve as member → revoke → re-resolve → tombstone, 0 leak).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    SetExpr, Zookie,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

use crate::membership::{channel_object, permissions};

pub mod gate;
pub mod invalidation;

pub use invalidation::{
    erasure_safe_rerender, invalidates_card, CardUpdatePush, LiveUnfurlInvalidator,
    UnfurlInvalidator, DEFAULT_CACHE_TTL_SECONDS, UNFURL_INVALIDATION_SUBJECTS,
};

pub use gate::{
    lower_over_unfurl_candidate, unfurl_candidate_colref, AuthzJoin, AuthzVisibleIndex, BoundParam,
    FilterMode, LoweredFilter,
};

// ───────────────────────────── the 4-step tombstone ladder (contract 5.7) ────────────────────────

/// **A resolved unfurl projection (contract 5.2 `Projection`) — the VIEWER-INDEPENDENT content cached
/// ONCE per `ArtifactRef`.** The fields are exactly the cross-subsystem `project(ref, viewer)` shape
/// (contract 5.6: `{title, state, icon, render_hint, sub_anchor?}`) — Refs returns this from `resolve`
/// after the OWNER's `project()` (the only way to read another subsystem's artifact). It carries NO
/// viewer identity: the SAME projection is what every ALLOWED viewer sees (the per-viewer decision is
/// the gate, NOT the content). This is the thing the cache holds — and it is only ever returned AFTER
/// the per-viewer gate passes, so caching it shared is leak-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The artifact's display title (e.g. an issue summary, a PR title, a channel name). This is the
    /// field that must NEVER reach a denied viewer (the leak-test payload, CHAT-D5).
    pub title: String,
    /// The artifact's state token (`open`/`merged`/`closed`/…) — render-time, viewer-independent.
    pub state: String,
    /// The icon/type hint the card renders (`issue`/`pr`/`channel`/`page`).
    pub icon: String,
    /// The optional `#sub` anchor the projection resolved (e.g. a specific message/line range) — the
    /// stable opaque id from the ref's sub-URN (5.7); `None` for a bare-root ref.
    pub sub_anchor: Option<String>,
}

/// The reason a ref degraded to a tombstone — the 4-step ladder outcomes (contract 5.7). The
/// **tombstone always carries the root** (a broken sub-anchor still resolves to the parent — §4.2), so
/// every reason names the parent the card points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// Ladder step 1 — the PERMISSION gate denied the viewer. The title was NEVER fetched (the
    /// leak-free chokepoint): the gate returns this WITHOUT reading the projection. Renders "a
    /// restricted <type>".
    Denied,
    /// Ladder step 4 — the referenced ROOT artifact is gone (deleted, not erased). Renders "this
    /// referenced <parent> (the specific part is no longer available)".
    Gone,
    /// Ladder step 4 (erased) — the referenced artifact (or a third party in it) was erased
    /// (crypto-shred / pseudonym shred). Renders "[erased]". Erasure-safe re-render is CHAT-P14.
    Erased,
}

/// A tombstone (contract 5.2 `Tombstone`) — the leak-free degradation of a ref. Carries the ROOT
/// (never the leaked title) + the reason, so the card renders "a restricted <type>" / "[erased]"
/// WITHOUT ever holding the confidential content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// The ROOT artifact ref (the `#sub`-stripped parent — the tombstone always carries the root, so a
    /// broken sub-anchor still points at the parent, §4.2). NEVER the title.
    pub root: ArtifactRef,
    /// Why the ref degraded (permission / gone / erased).
    pub reason: TombstoneReason,
}

/// **The 4-step tombstone ladder outcome (contract 5.7).** The ONE frozen ladder every ref degrades
/// through: permission → root → sub-resolve {live/moved/outdated/gone} → erased. Chat CONSUMES the
/// ladder (it does not re-implement it — EI-01 §7); for a chat-own ref the producible outcomes are
/// `Live`/`Gone`/`Erased`, but a NON-chat ref embedded in a chat message (an Issue/PR/KN page) can
/// also degrade `Moved`/`Outdated` — so the enum carries the full ladder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LadderOutcome {
    /// The ref resolved to a live projection (the card renders title/state/icon/actions).
    Live(Projection),
    /// The referenced KN block / row moved — the card renders + a `moved` flag (NON-chat refs only).
    Moved(Projection),
    /// The projection is partial / stale — the card renders + an `outdated` flag (NON-chat refs only).
    Outdated(Projection),
    /// The root is gone (deleted) — a tombstone carrying the root.
    Gone(Tombstone),
    /// The artifact (or a third party in it) was erased — a tombstone, "[erased]".
    Erased(Tombstone),
}

/// **The leak-free card the unfurl service renders for one ref + viewer.** Either the
/// viewer-independent projection (the gate passed AND the ladder resolved live/moved/outdated) or a
/// tombstone (the gate denied, OR the ladder degraded gone/erased). NEVER a title for a denied viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Card {
    /// A live card — the projection content (the gate passed). Carries the ladder flag so the renderer
    /// shows the `moved`/`outdated` badge.
    Live {
        /// The viewer-independent projection (the SAME for every allowed viewer).
        projection: Projection,
        /// `true` iff the ladder degraded `Moved` (a NON-chat ref whose anchor moved).
        moved: bool,
        /// `true` iff the ladder degraded `Outdated` (a partial/stale projection).
        outdated: bool,
    },
    /// A tombstone — the leak-free degradation (denied / gone / erased). NEVER a title.
    Tombstone(Tombstone),
}

impl Card {
    /// `true` iff this card is a tombstone (the leak-free degradation). A drill asserts a denied
    /// viewer's card is a tombstone.
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Card::Tombstone(_))
    }

    /// The title the card exposes to the viewer, if any — `None` for a tombstone (the leak-free
    /// invariant: a tombstone exposes NO title). A drill asserts this is `None` for a denied viewer.
    pub fn exposed_title(&self) -> Option<&str> {
        match self {
            Card::Live { projection, .. } => Some(&projection.title),
            Card::Tombstone(_) => None,
        }
    }
}

// ───────────────────────────── the Refs resolve chokepoint port (contract 5.2) ───────────────────

/// **The Refs `resolve(ref, viewer, mode) -> Projection | Tombstone` chokepoint (contract 5.2) — the
/// port the unfurl service calls on a cache MISS.** The PRODUCTION binding is Refs' `resolve` over the
/// resilient client (contract 1.9), which calls the owner's `project(ref, viewer)` (contract 5.6).
/// Chat NEVER re-implements permission-aware resolution (EI-01 §7) — it CALLS this chokepoint. The
/// real Refs resolver is a NAMED FLOOR (REF-P10 / CHAT-P15); here the port is the seam.
///
/// NOTE: the resolver itself is permission-aware (it returns a `Tombstone` for a denied viewer too),
/// but the unfurl service ALSO runs the per-viewer gate FIRST — so a denied viewer's title is never
/// even fetched (the gate-before-resolve order is the structural no-leak guarantee, not a trust in the
/// downstream resolver). Defence in depth: even a buggy resolver that leaked a title would be unreached
/// for a denied viewer.
pub trait RefsResolvePort {
    /// Resolve a ref for a viewer in `Display` mode (contract 5.2 / 5.6) over the 4-step ladder. The
    /// `at` zookie bounds staleness (the conversation's stamped `acl_zookie` — the new-enemy guard).
    /// Returns the full [`LadderOutcome`]; the unfurl service caches the viewer-INDEPENDENT projection.
    fn resolve(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        at: &Consistency,
    ) -> LadderOutcome;
}

// ───────────────────────────── the shared per-ref projection cache (§4.2) ────────────────────────

/// **The shared, per-`ArtifactRef` projection cache — ONE entry per ref, NEVER per `(ref, viewer)`
/// (§4.2; the no-leak invariant).** Keyed by the ref string ALONE (there is structurally no viewer in
/// the key), so a viewer's permission decision can never be baked into a shared entry. The content is
/// VIEWER-INDEPENDENT (the SAME projection every allowed viewer sees) and is only ever STORED/RETURNED
/// after the per-viewer gate passes (the gate is the per-viewer part; the content is the shared part).
///
/// Short-TTL, bus-busted (the precise bus-bust consumer is CHAT-P14; here the TTL backstop only). The
/// in-memory model is the dev floor; the REAL cache is Valkey (`unfurl:proj:<ref>`) — a config swap,
/// not a code change (the binding policy). The cache holds ONLY a live/moved/outdated projection: a
/// gone/erased outcome is NOT cached as content (it is the absence of content — re-resolved each time
/// until the bus-bust, CHAT-P14).
#[derive(Clone, Default)]
pub struct UnfurlCache {
    /// `ref_string -> projection` — the key is the REF ALONE (no viewer; the no-leak invariant is
    /// structural: a `(ref, viewer)` key is unconstructable).
    entries: Arc<Mutex<HashMap<String, Projection>>>,
}

impl UnfurlCache {
    /// A fresh, empty cache.
    pub fn new() -> UnfurlCache {
        UnfurlCache::default()
    }

    /// Look up the shared projection for a ref (the cache HIT path) — `None` on a miss (the caller
    /// resolves via the [`RefsResolvePort`] and inserts). The lookup key is the ref ALONE.
    pub fn get(&self, ref_: &ArtifactRef) -> Option<Projection> {
        self.entries.lock().unwrap().get(&ref_.0).cloned()
    }

    /// Insert the shared, VIEWER-INDEPENDENT projection for a ref (the cache FILL after a resolve). The
    /// key is the ref ALONE — a second viewer resolving the SAME ref reuses THIS entry (one entry per
    /// ref). Returns the prior entry if any (idempotent fill).
    pub fn put(&self, ref_: &ArtifactRef, projection: Projection) -> Option<Projection> {
        self.entries
            .lock()
            .unwrap()
            .insert(ref_.0.clone(), projection)
    }

    /// **Bust the shared entry for a ref (the bus-bust hook, §4.4).** Drops the cached projection so
    /// the next resolve re-fetches. The PRECISE bus-driven invalidation consumer (the matching
    /// `*.updated`/`*.erased` event → bust) is CHAT-P14; here the lever is exposed so CHAT-P14 wires
    /// the consumer to it (no second cache, EI-01 §7). Returns `true` iff an entry was busted.
    pub fn bust(&self, ref_: &ArtifactRef) -> bool {
        self.entries.lock().unwrap().remove(&ref_.0).is_some()
    }

    /// **The one-entry-per-ref invariant probe (the GATE): the number of CACHE ENTRIES.** A confidential
    /// ref resolved by N viewers has exactly ONE entry (never N) — the cache is keyed by ref, not
    /// `(ref, viewer)`. A drill asserts `entry_count() == 1` after N viewers resolve the SAME ref (0
    /// per-viewer cache entries; 0 viewer-content baked into the cache).
    pub fn entry_count(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Whether the cache holds an entry for a ref (the bus-bust assertion helper).
    pub fn contains(&self, ref_: &ArtifactRef) -> bool {
        self.entries.lock().unwrap().contains_key(&ref_.0)
    }

    /// **Clear EVERY entry (the GDPR erase-fan-out purge, CHAT-P22 / CHAT-D8).** A subject's erasure
    /// busts the whole short-TTL projection cache so the next render re-resolves LIVE — the cache
    /// holds no durable PII snapshot (each entry is a viewer-independent, content-addressed
    /// projection re-fetched from source), so clearing it is the correct, fail-safe purge (the
    /// erasure-safe re-render, CHAT-D6: the next render gets a tombstone, never stale PII). Returns
    /// the count cleared.
    pub fn clear(&self) -> usize {
        let mut entries = self.entries.lock().unwrap();
        let n = entries.len();
        entries.clear();
        n
    }
}

// ───────────────────────────── the Unfurl Service (the orchestration; §4) ────────────────────────

/// One unfurl candidate the service resolves — an `ArtifactRef` referenced by a VISIBLE message (an
/// [`InlineNode::ArtifactRefNode`](myelin_content::InlineNode::ArtifactRefNode) /
/// [`InlineNode::Embed`](myelin_content::InlineNode::Embed) node from the message body, CHAT-P11), plus
/// the conversation it is anchored in (the `acl_zookie` the gate reads — the new-enemy guard).
#[derive(Clone, Debug)]
pub struct UnfurlCandidate {
    /// The artifact ref to unfurl (the cache key + the gate object).
    pub ref_: ArtifactRef,
    /// The conversation the referencing message is in — the gate checks `channel.read` against it and
    /// reads its stamped `acl_zookie` (the new-enemy watermark). `None` for a ref outside any channel
    /// context (the gate then checks the ref's own object directly).
    pub channel_id: Option<String>,
}

/// **The Unfurl Service (CHAT-P13) — the shared cache + per-viewer gate orchestration in FRONT of Refs
/// `resolve` (§4).** Generic over the frozen [`IdentityService`] ABI (the per-viewer `check`/
/// `list_objects` gate) + the [`RefsResolvePort`] (the 5.2 resolve chokepoint) — chat is a CONSUMER
/// subsystem (it depends on the names-only contract surfaces, never the engines; the §2.9 DAG stays
/// acyclic). Holds the shared [`UnfurlCache`].
///
/// The resolve order per ref (the no-leak structure):
/// 1. **The per-viewer gate (§4.2/§4.3).** `Id.check(viewer, channel.read, channel, zookie)` — the
///    per-viewer part. A `Deny`/`Conditional`/Id-error returns a [`Card::Tombstone`] WITHOUT touching
///    the cache or the resolver (the title is never fetched — the leak-free chokepoint).
/// 2. **The cache (§4.2).** On `Allow`, the shared projection is looked up by ref ALONE (one entry per
///    ref). A HIT returns the viewer-independent content; a MISS calls the resolver and FILLS the
///    shared entry.
/// 3. **The 4-step ladder (5.7).** The resolver's [`LadderOutcome`] maps to a [`Card`] — live/moved/
///    outdated render content; gone/erased render a tombstone.
pub struct UnfurlService<I: IdentityService, R: RefsResolvePort> {
    id: I,
    resolver: R,
    cache: UnfurlCache,
}

impl<I: IdentityService, R: RefsResolvePort> UnfurlService<I, R> {
    /// Compose the service over Identity (the gate) + the Refs resolve port (the chokepoint) + a fresh
    /// shared cache.
    pub fn new(id: I, resolver: R) -> UnfurlService<I, R> {
        UnfurlService {
            id,
            resolver,
            cache: UnfurlCache::new(),
        }
    }

    /// Compose over an EXISTING shared cache (so the bus-bust consumer, CHAT-P14, and the service share
    /// the one cache — never a second cache, EI-01 §7).
    pub fn with_cache(id: I, resolver: R, cache: UnfurlCache) -> UnfurlService<I, R> {
        UnfurlService {
            id,
            resolver,
            cache,
        }
    }

    /// The shared cache (so a test / the CHAT-P14 bus-bust consumer reads the one-entry invariant +
    /// busts the shared entry — never a second cache).
    pub fn cache(&self) -> &UnfurlCache {
        &self.cache
    }

    /// The Refs resolve chokepoint (so a CHAT-P14 erasure/live-update drill can drive the resolver's
    /// outcome and assert a LIVE re-resolve after a bus-bust — never a stale cache read).
    pub fn resolver(&self) -> &R {
        &self.resolver
    }

    /// **Resolve ONE ref for a viewer — the gate-then-cache-then-ladder path (the no-leak core).** The
    /// per-viewer gate runs FIRST; a denial returns a tombstone WITHOUT reading the cache or the
    /// resolver (the title is never fetched for a denied viewer — CHAT-D5, 0 title leak). On allow, the
    /// shared cache is consulted (one entry per ref); a miss resolves via the chokepoint and fills the
    /// shared entry.
    pub fn resolve_one(&self, candidate: &UnfurlCandidate, viewer: &Principal) -> Card {
        let tenant = TenantId(viewer.tenant.0.clone());
        let region = Region(viewer.region.0.clone());

        // STEP 1 — the PER-VIEWER gate (§4.2). The object the viewer must hold `channel.read` on is the
        // conversation the referencing message is in (a member of the channel may see the channel's own
        // refs; a non-member is fail-closed). A ref outside any channel context gates on the ref's own
        // object. The check is at-or-after the conversation's stamped `acl_zookie` with Strong
        // consistency — the new-enemy guard: a just-revoked grant is denied immediately (§5 / 4.10).
        let (object, at) = self.gate_object(candidate);
        let decision = self.id.check(
            viewer,
            &Permission(permissions::READ.to_string()),
            &object,
            &at,
            None,
        );
        match decision {
            Ok(Decision::Allow) => {}
            // FAIL-CLOSED: Deny / Conditional / Id-error ALL deny → a tombstone, the title NEVER
            // fetched (the cache + the resolver are NOT touched — the leak-free chokepoint). The
            // tombstone carries the ROOT, never the title.
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Card::Tombstone(Tombstone {
                    root: myelin_refs::strip_sub(&candidate.ref_),
                    reason: TombstoneReason::Denied,
                });
            }
        }

        // STEP 2 — the SHARED cache (§4.2), keyed by the REF ALONE (one entry per ref). A HIT returns
        // the viewer-independent content (the SAME for every allowed viewer); a MISS resolves + fills.
        if let Some(projection) = self.cache.get(&candidate.ref_) {
            return Card::Live {
                projection,
                moved: false,
                outdated: false,
            };
        }

        // STEP 3 — the cache MISS path: the Refs resolve chokepoint (5.2) over the 4-step ladder (5.7).
        let outcome = self
            .resolver
            .resolve(&tenant, &region, &candidate.ref_, viewer, &at);
        self.outcome_to_card(&candidate.ref_, outcome)
    }

    /// **Resolve the VIEWPORT — lazy-on-viewport (§4.1, the single biggest cost-killer).** Resolves
    /// ONLY the candidates currently on screen (the caller passes the viewport slice, never the whole
    /// channel). A scroll-back of 10 000 messages resolves a handful of cards, not 10 000. Returns one
    /// [`Card`] per candidate, in order.
    pub fn unfurl_viewport(&self, viewport: &[UnfurlCandidate], viewer: &Principal) -> Vec<Card> {
        viewport
            .iter()
            .map(|c| self.resolve_one(c, viewer))
            .collect()
    }

    /// The gate object + the consistency the per-viewer `check` runs at. A ref anchored in a channel
    /// gates on the `channel:<id>` object (the membership-tuple object the gate joins) at the
    /// conversation's stamped `acl_zookie`; a ref with no channel context gates on the ref's own object
    /// at default consistency. Strong consistency bypasses the fail-static cache (the new-enemy guard,
    /// §8.7).
    fn gate_object(
        &self,
        candidate: &UnfurlCandidate,
    ) -> (myelin_tenancy::ArtifactRef, Consistency) {
        match &candidate.channel_id {
            Some(channel_id) => (
                myelin_tenancy::ArtifactRef(channel_object(channel_id)),
                Consistency {
                    // The new-enemy watermark is stamped on the conversation; the caller passes the
                    // candidate already resolved against the live conversation, so the gate reads at
                    // Strong consistency with the empty floor (the conversation's stamped zookie is
                    // read by the caller's `MembershipService::read_consistency` recipe when it has the
                    // Conversation in hand; here the candidate is channel-scoped and the gate keys on
                    // the channel object with Strong consistency).
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
            ),
            None => (
                myelin_tenancy::ArtifactRef(candidate.ref_.0.clone()),
                Consistency {
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
            ),
        }
    }

    /// Map a [`LadderOutcome`] (the resolver's 4-step ladder result, 5.7) to a [`Card`], FILLING the
    /// shared cache with the viewer-independent projection on a live/moved/outdated outcome (the
    /// content is shared; the gate already passed). A gone/erased outcome is a tombstone — NOT cached
    /// as content (the absence of content is re-resolved each time until the bus-bust, CHAT-P14).
    fn outcome_to_card(&self, ref_: &ArtifactRef, outcome: LadderOutcome) -> Card {
        match outcome {
            LadderOutcome::Live(projection) => {
                self.cache.put(ref_, projection.clone());
                Card::Live {
                    projection,
                    moved: false,
                    outdated: false,
                }
            }
            LadderOutcome::Moved(projection) => {
                self.cache.put(ref_, projection.clone());
                Card::Live {
                    projection,
                    moved: true,
                    outdated: false,
                }
            }
            LadderOutcome::Outdated(projection) => {
                self.cache.put(ref_, projection.clone());
                Card::Live {
                    projection,
                    moved: false,
                    outdated: true,
                }
            }
            LadderOutcome::Gone(tombstone) | LadderOutcome::Erased(tombstone) => {
                Card::Tombstone(tombstone)
            }
        }
    }
}

/// **Precompute the membership-as-permission class for the unfurl candidate set (§4.3, OQ-E).** Lowers
/// the `list_objects(viewer, channel.read, channel)` `SetExpr` (contract 4.3) to a SQL predicate / JOIN
/// over the unfurl candidate id column — **one class decision, not N checks**. For a public channel
/// "can a member see this project artifact?" is often a SINGLE coarse class; the lowered [`LoweredFilter`]
/// is conjoined into the candidate scan (no N+1, no post-filter). Returns the lowered filter the
/// candidate scan ANDs into its `WHERE`; the `viewer` is the `av.subject` binding.
///
/// This is the cheap path for a CHANNEL full of refs to the SAME project: one `list_objects` returns
/// the class, lowered ONCE, applied to every candidate in the scan — never one `check` per candidate.
pub fn precompute_visibility_class(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over_unfurl_candidate(set_expr, viewer)
}

/// Evaluate the precomputed visibility class against a candidate set (the in-memory model of the SQL
/// JOIN the live scan runs, §4.3) — the candidate refs the viewer may see, leak-free, NO N+1. Returns
/// the subset of `candidates` that survive the lowered JOIN/predicate. The REAL path is the database
/// evaluating the SAME predicate against the `authz_visible` reverse index (the `--features integration`
/// proof); the in-memory index models the SAME semantics + the new-enemy watermark.
pub fn filter_candidates_by_class(
    index: &AuthzVisibleIndex,
    tenant: &TenantId,
    region: &Region,
    viewer: &Principal,
    lowered: &LoweredFilter,
    candidates: &[ObjectId],
) -> Vec<ObjectId> {
    index.evaluate(tenant, region, viewer, lowered, candidates)
}

// ───────────────────────────── unit tests (the no-leak core: cache + gate + ladder) ──────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    use myelin_identity::{AuthzError, ListObjectsResult, Precondition};
    use myelin_identity::{
        Credential, EffectivePolicy, FailStaticBound, FragmentAdmit, NamespaceFragment, ObjectType,
        PrincipalId, PrincipalKind, Result as IdResult, RevokeTarget, RewriteTrace, RunId,
        RunToken, SubjectTree, TupleDelta,
    };

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn confidential_ref() -> ArtifactRef {
        ArtifactRef("myelin://acme/chat/channel/board-secret".into())
    }
    const SECRET_TITLE: &str = "#board-leadership-comp";

    /// A synthetic `IdentityService` whose `check` returns Allow only for an allow-listed
    /// `(subject, object)` — the per-viewer gate the unfurl service runs FIRST. All other methods are
    /// the names-only fail-closed defaults (chat only consumes `check` here).
    #[derive(Default)]
    struct GateId {
        allow: StdMutex<Vec<(String, String)>>,
    }
    impl GateId {
        fn allow(&self, subject: &str, object: &str) {
            self.allow
                .lock()
                .unwrap()
                .push((subject.into(), object.into()));
        }
    }
    impl IdentityService for GateId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn check(
            &self,
            subject: &Principal,
            _permission: &Permission,
            object: &myelin_tenancy::ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&myelin_identity::CaveatContext>,
        ) -> IdResult<Decision> {
            let allowed = self
                .allow
                .lock()
                .unwrap()
                .iter()
                .any(|(s, o)| s == &subject.principal_id.0 && o == &object.0);
            Ok(if allowed {
                Decision::Allow
            } else {
                Decision::Deny
            })
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _at: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn delegation(&self, _a: &Principal, _t: &Principal) -> IdResult<EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn write_tuples(&self, _d: &[TupleDelta], _p: Option<&Precondition>) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &FailStaticBound,
        ) -> IdResult<RunToken> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn revoke(&self, _t: &RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("test"))
        }
        fn admit_fragment(&self, _f: &NamespaceFragment) -> IdResult<FragmentAdmit> {
            Ok(FragmentAdmit::Admitted {
                fragment_id: "test".into(),
            })
        }
    }

    /// A synthetic Refs resolve chokepoint (5.2; REF-P10 floor) — returns a live projection carrying
    /// the SECRET_TITLE (so a leak would be observable) or a programmable ladder outcome. Counts how
    /// many times it is CALLED (so a test proves the gate-before-resolve order: a denied viewer never
    /// reaches the resolver).
    #[derive(Default)]
    struct SyntheticResolver {
        outcome: StdMutex<Option<LadderOutcome>>,
        calls: StdMutex<usize>,
    }
    impl SyntheticResolver {
        fn live() -> SyntheticResolver {
            let r = SyntheticResolver::default();
            *r.outcome.lock().unwrap() = Some(LadderOutcome::Live(Projection {
                title: SECRET_TITLE.into(),
                state: "active".into(),
                icon: "channel".into(),
                sub_anchor: None,
            }));
            r
        }
        fn set(&self, o: LadderOutcome) {
            *self.outcome.lock().unwrap() = Some(o);
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }
    impl RefsResolvePort for SyntheticResolver {
        fn resolve(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            _ref_: &ArtifactRef,
            _viewer: &Principal,
            _at: &Consistency,
        ) -> LadderOutcome {
            *self.calls.lock().unwrap() += 1;
            self.outcome.lock().unwrap().clone().expect("outcome set")
        }
    }

    fn channel_candidate() -> UnfurlCandidate {
        UnfurlCandidate {
            ref_: confidential_ref(),
            channel_id: Some("board-secret".into()),
        }
    }

    /// **CHAT-D5 core — a denied viewer's card is a TOMBSTONE; the title is NEVER fetched (the
    /// gate-before-resolve order).** The resolver is NOT called for a denied viewer (0 title leak,
    /// structurally — the secret is never read).
    #[test]
    fn denied_viewer_tombstones_title_never_fetched() {
        let id = GateId::default(); // nobody allowed.
        let resolver = SyntheticResolver::live();
        let svc = UnfurlService::new(id, resolver);

        let card = svc.resolve_one(&channel_candidate(), &viewer("intruder"));
        assert!(card.is_tombstone(), "a denied viewer sees a tombstone");
        assert_eq!(
            card.exposed_title(),
            None,
            "0 title leak — no title exposed"
        );
        // the title was NEVER fetched: the resolver was not called (gate-before-resolve).
        assert_eq!(
            svc.resolver.calls(),
            0,
            "the resolver is unreached for a denied viewer"
        );
        // and the cache holds NOTHING for the denied ref (no content baked in).
        assert!(!svc.cache().contains(&confidential_ref()));
    }

    /// **The one-cache-entry-per-ref invariant: N viewers resolving the SAME ref → exactly ONE cache
    /// entry (never per (ref, viewer)).** The content is viewer-independent; the cache is keyed by the
    /// ref alone (0 per-viewer cache entries).
    #[test]
    fn one_cache_entry_per_ref_never_per_viewer() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        // three distinct viewers ALL allowed to read the same channel.
        id.allow("alice", &object);
        id.allow("bob", &object);
        id.allow("carol", &object);
        let resolver = SyntheticResolver::live();
        let svc = UnfurlService::new(id, resolver);

        for who in ["alice", "bob", "carol"] {
            let card = svc.resolve_one(&channel_candidate(), &viewer(who));
            assert_eq!(
                card.exposed_title(),
                Some(SECRET_TITLE),
                "{who} sees the shared title"
            );
        }
        // ONE entry for the ref — never three (the no-leak invariant: keyed by ref, not (ref, viewer)).
        assert_eq!(
            svc.cache().entry_count(),
            1,
            "exactly one cache entry per ref"
        );
        // and the resolver was called ONCE (the cache served the other two — viewer-independent).
        assert_eq!(
            svc.resolver.calls(),
            1,
            "resolve once, cache serves the rest"
        );
    }

    /// **The chained no-leak property: resolve as member → revoke → re-resolve → tombstone, 0 leak.**
    /// An allowed viewer sees the title; after the gate flips to Deny (the revoke), the SAME viewer
    /// sees a tombstone — the title is gone (the gate is the per-viewer chokepoint, not the cache).
    #[test]
    fn chained_member_then_revoke_then_tombstone_zero_leak() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("dave", &object);
        let resolver = SyntheticResolver::live();
        let svc = UnfurlService::new(id, resolver);

        // member resolve → sees the title.
        let before = svc.resolve_one(&channel_candidate(), &viewer("dave"));
        assert_eq!(before.exposed_title(), Some(SECRET_TITLE));

        // REVOKE: dave loses read (the gate now denies).
        svc.id.allow.lock().unwrap().clear();

        // re-resolve → tombstone, 0 title leak (even though the cache still holds the shared content,
        // the per-viewer gate denies BEFORE the cache is read).
        let after = svc.resolve_one(&channel_candidate(), &viewer("dave"));
        assert!(after.is_tombstone(), "post-revoke the card is a tombstone");
        assert_eq!(after.exposed_title(), None, "0 leak post-revoke");
    }

    /// **The 4-step ladder outcomes (5.7) map to the right card: live / gone / erased.** A chat ref's
    /// producible outcomes; the tombstone carries the ROOT.
    #[test]
    fn ladder_outcomes_map_to_cards() {
        let object = channel_object("board-secret");

        // LIVE → a live card with the projection.
        let id = GateId::default();
        id.allow("e", &object);
        let svc = UnfurlService::new(id, SyntheticResolver::live());
        let card = svc.resolve_one(&channel_candidate(), &viewer("e"));
        assert!(matches!(
            card,
            Card::Live {
                moved: false,
                outdated: false,
                ..
            }
        ));

        // GONE → a tombstone carrying the root, reason Gone.
        let id2 = GateId::default();
        id2.allow("f", &object);
        let resolver2 = SyntheticResolver::default();
        resolver2.set(LadderOutcome::Gone(Tombstone {
            root: confidential_ref(),
            reason: TombstoneReason::Gone,
        }));
        let svc2 = UnfurlService::new(id2, resolver2);
        match svc2.resolve_one(&channel_candidate(), &viewer("f")) {
            Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Gone),
            other => panic!("expected Gone tombstone, got {other:?}"),
        }
        // a gone outcome is NOT cached as content (re-resolved until the bus-bust, CHAT-P14).
        assert!(!svc2.cache().contains(&confidential_ref()));

        // ERASED → a tombstone, reason Erased.
        let id3 = GateId::default();
        id3.allow("g", &object);
        let resolver3 = SyntheticResolver::default();
        resolver3.set(LadderOutcome::Erased(Tombstone {
            root: confidential_ref(),
            reason: TombstoneReason::Erased,
        }));
        let svc3 = UnfurlService::new(id3, resolver3);
        match svc3.resolve_one(&channel_candidate(), &viewer("g")) {
            Card::Tombstone(t) => assert_eq!(t.reason, TombstoneReason::Erased),
            other => panic!("expected Erased tombstone, got {other:?}"),
        }
    }

    /// **Lazy-on-viewport: only the candidates passed are resolved (the caller passes the viewport
    /// slice).** The service maps one card per candidate in order.
    #[test]
    fn unfurl_viewport_resolves_only_the_slice() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("h", &object);
        let svc = UnfurlService::new(id, SyntheticResolver::live());
        let viewport = vec![channel_candidate(), channel_candidate()];
        let cards = svc.unfurl_viewport(&viewport, &viewer("h"));
        assert_eq!(cards.len(), 2);
        assert!(cards
            .iter()
            .all(|c| c.exposed_title() == Some(SECRET_TITLE)));
        // two candidates, same ref → ONE cache entry, ONE resolve (lazy + shared).
        assert_eq!(svc.cache().entry_count(), 1);
        assert_eq!(svc.resolver.calls(), 1);
    }

    /// **The bus-bust lever (CHAT-P14 hook): busting the shared entry drops the cached content.** The
    /// next resolve re-fetches (the precise bus-driven invalidation consumer lands in CHAT-P14).
    #[test]
    fn cache_bust_drops_the_shared_entry() {
        let id = GateId::default();
        let object = channel_object("board-secret");
        id.allow("i", &object);
        let svc = UnfurlService::new(id, SyntheticResolver::live());
        svc.resolve_one(&channel_candidate(), &viewer("i"));
        assert_eq!(svc.cache().entry_count(), 1);
        assert!(
            svc.cache().bust(&confidential_ref()),
            "the entry was busted"
        );
        assert_eq!(svc.cache().entry_count(), 0);
        // re-resolve re-fetches (calls the resolver again).
        svc.resolve_one(&channel_candidate(), &viewer("i"));
        assert_eq!(svc.resolver.calls(), 2);
    }
}
