//! # `unfurl::invalidation` — bus-driven cache invalidation + erasure-safe re-render + `#sub`
//! anchor stability (CHAT-P14 / P-408, M4-C4 — CHAT-D6 / D7 / D18)
//!
//! The **second committable unit of M4-C4** (the cache + per-viewer gate is CHAT-P13 / [`super`]).
//! Three properties land here, all over the **one shared cache** CHAT-P13 owns ([`UnfurlCache`] —
//! never a second cache, EI-01 §7):
//!
//! 1. **Bus-driven invalidation (§4.4, precise; TTL is the backstop).** A consumer
//!    ([`UnfurlInvalidator`]) over the artifact pointer events — `*.updated` / `ci.check.updated`
//!    (the frozen CheckStatus event, X-1) / `*.erased` (the cross-cutting erasure tombstone, contract
//!    2.7) — **busts the shared projection cache entry** for that `ArtifactRef` and pushes a **live
//!    firehose card update** (a frame, contract 3.5) to viewers currently showing the card. Precise
//!    (the matching event busts exactly the one ref's entry); the short TTL is only the backstop. The
//!    consumer is **whitelisted-subject, never `*`** (contract 2.4) and **idempotent** via the
//!    `consumer_dedup` ledger (contract 2.5 — the [`Consumer`](myelin_events::consumer::Consumer)
//!    runtime owns dedup; the handler is idempotent by construction — a bust of an absent entry is a
//!    no-op).
//!
//! 2. **Erasure-safe cards (§6; CHAT-D6).** Erasing a third party rendered in a card produces a
//!    **tombstone on next render, 0 recoverable PII, NO durable snapshot** — because **no rendered
//!    title/state/PII is ever stored** (§4.5: the only durable thing is the `artifact_ref` node + the
//!    post-time timestamp, a reference, never rendered content). The `*.erased` event busts the
//!    shared entry; the cache **re-resolves live → `Erased`** (the 4-step ladder erased outcome, 5.7).
//!    The cache holds ONLY a live/moved/outdated projection — a gone/erased outcome is never cached as
//!    content, so there is nothing to recover. [`erasure_safe_rerender`] is the proof helper.
//!
//! 3. **`#sub` anchor stability (§2; CHAT-D18).** A message id is immutable, so its
//!    `...#message-<id>` `#sub` is **stable across edits** — an EDIT is a new state on the SAME id, so
//!    a referencing embed stays **live** and the anchor never dangles. A DELETE degrades the embed to
//!    a **Tombstone carrying the root** (the `#sub`-stripped channel) — it **never dangles**. The
//!    `#sub` anchor logic lives in [`anchor`]; it consumes the frozen 5.7 ladder, it does not
//!    re-implement it (EI-01 §7).
//!
//! ## What this module does NOT own (the seam to CHAT-P13)
//! It does not own the cache, the gate, the resolver, or the ladder enum — those are CHAT-P13
//! ([`super::UnfurlCache`] / [`super::UnfurlService`] / [`super::RefsResolvePort`] /
//! [`super::LadderOutcome`]). This module is the **consumer + the live-bust wiring** over that one
//! cache. The bus-bust lever it drives is [`super::UnfurlCache::bust`] (the hook CHAT-P13 exposed for
//! exactly this).
//!
//! ## The FLOOR (R-C4 — measured, not predicted)
//! No NEW milestone floor. The unfurl cache **TTL** (the backstop) + the membership-class **refresh
//! cadence** are **measured-not-predicted tunables** (R-C4): the bus-bust is the precise path, the TTL
//! is the staleness ceiling, and both are tuned against TELEMETRY (the p99 staleness + the bus-lag),
//! never guessed in this prompt and never a separate milestone. The default TTL ([`DEFAULT_CACHE_TTL_SECONDS`])
//! is NAMED, generous, and is the backstop only — the bus-bust makes the common case immediate.
//!
//! ## The mutation floor (mandatory-core — the no-recoverable-PII property)
//! The erasure-safe re-render ([`erasure_safe_rerender`]) + the invalidation match
//! ([`UnfurlInvalidator::should_bust`]) are the **mandatory-core** no-recoverable-PII surface: the
//! cargo-mutants floor is **0 surviving mutants on the `*.erased`/`*.updated` match + the bust-then-
//! re-resolve path** (a survived mutant that fails to bust on `*.erased`, or that re-serves a stale
//! cached title after an erase, is recoverable PII in a card). Asserted by the chained drill
//! (`drill_chat_d6_d7_d18_invalidation.rs`: resolve → erase third party → re-resolve → tombstone, 0
//! recoverable PII).

use myelin_events::consumer::{Consumer, ConsumerName, PrefetchBound, Subscription};
use myelin_events::firehose::FirehoseScope;
use myelin_events::taxonomy::new_tokens::CI_CHECK_UPDATED;
use myelin_events::{DedupLedger, EventEnvelope, EventHandler, HandleOutcome, SubjectPattern};
use myelin_refs::ArtifactRef;

use super::{Card, LadderOutcome, RefsResolvePort, Tombstone, TombstoneReason, UnfurlCache};

// ───────────────────────────── the cache-TTL tunable (R-C4, the backstop) ─────────────────────────

/// **The default unfurl cache TTL — the BACKSTOP, not the precise path (R-C4, measured-not-predicted).**
/// The bus-bust ([`UnfurlInvalidator`]) is the precise invalidation; this TTL is the ceiling on
/// staleness if a bus event is missed/delayed. It is a NAMED tunable (a generous default), tuned per
/// stream class against telemetry (the p99 staleness vs the bus-lag), never a guessed production number.
/// 60 seconds is generous enough that the common case is always served by the precise bus-bust and the
/// TTL never fires in practice — it exists so a missed event cannot pin a stale card indefinitely.
pub const DEFAULT_CACHE_TTL_SECONDS: u64 = 60;

// ───────────────────────────── the invalidation event-name match (§4.4) ───────────────────────────

/// **The whitelisted unfurl-invalidation subject prefixes (contract 2.4 — never `*`).** The consumer
/// subscribes to these BOUNDED subject prefixes (the artifact pointer events whose change can invalidate
/// a card); the per-event-name decision ([`UnfurlInvalidator::should_bust`]) then matches the precise
/// `*.updated`/`*.erased`/`ci.check.updated` set within them. Each is a concrete prefix — there is no
/// `*` here (an over-broad subscription head-of-line-blocks everything, BUS-3); the
/// [`Subscription::bind`] `*`-rejection enforces it at registration.
///
/// (`issue.` / `git.` / `ci.` / `knowledge.` / `chat.` / `identity.` are the producing subsystems of
/// the artifacts a chat card can embed — arch 03 §1.3.)
pub const UNFURL_INVALIDATION_SUBJECTS: &[&str] =
    &["issue.", "git.", "ci.", "knowledge.", "chat.", "identity."];

/// **Does this event TYPE invalidate an unfurl card (the precise `*.updated`/`*.erased`/
/// `ci.check.updated` match, §4.4)?** The mandatory-core decision: a matching pointer event busts the
/// shared cache entry for the event's subject ref. The match is on the **event-name** segment of the
/// dotted token `<subsystem>.<artifact>.<event_name>` (Bus §6 grammar):
/// - the event name is `erased` → `*.erased` (the cross-cutting erasure tombstone, contract 2.7);
/// - the event name is `updated` → `*.updated` (the artifact changed);
/// - OR the token is exactly `ci.check.updated` (the frozen CheckStatus event, X-1 — already covered by
///   the `updated` name, asserted explicitly so a CheckStatus change always busts);
/// - the event name is `revoked` → `identity.permission.revoked` (a permission change can flip a
///   viewer's gate; the entry is busted so the next resolve re-gates, §4.4).
///
/// A NON-matching event (a `created`/`member_added`/…) does NOT bust (precision — only a change to the
/// rendered artifact, or an erase/revoke, invalidates a card). Returns `false` for an ungrammatical
/// token (no segment to match) — fail-closed-to-no-bust (a malformed token is not a real invalidation).
pub fn invalidates_card(event_type: &str) -> bool {
    if event_type == CI_CHECK_UPDATED {
        return true;
    }
    match event_type.rsplit_once('.') {
        Some((_, event_name)) => {
            matches!(event_name, "updated" | "erased" | "revoked")
        }
        None => false,
    }
}

// ───────────────────────────── the live-bust card-update seam (§4.4, contract 3.5) ────────────────

/// **The live card-update push seam — the port the gateway's CHAT-P10 live-delivery surface implements
/// (§4.4, arch §9).** After the shared cache entry is busted, a viewer with the card on screen needs a
/// LIVE update frame on the channel's firehose scope (contract 3.5: `channel:<id>` — a bounded
/// selector, never `*`) so the re-resolved card replaces the stale one WITHIN BUDGET (no stale title;
/// an erased third party tombstones live).
///
/// **Why a port, not a `firehose.publish` here (EI-01 §7 — one transport).** The firehose transport is
/// the **gateway's** (arch §9: "the gateway has no emit path" — its ONLY output is the firehose; the
/// invalidation module owns NO transport handle). The gateway already owns the ONE excluded
/// `firehose.publish` call site (`myelin-chat-gateway/src/delivery.rs`, CHAT-P10) — this module does
/// NOT re-implement a second firehose publish. It hands the invalidated **root ref** to this port; the
/// gateway's [`CardUpdatePush`] impl publishes the bust frame on the bounded scope. The frame is a
/// **references-not-payloads pointer** (the ref, never rendered content), so the bust frame is leak-free
/// even for a card a viewer can no longer see (the per-viewer re-resolve happens at the connection tier).
pub trait CardUpdatePush {
    /// Push a live card-update (a bust) for `invalidated` on the bounded channel `scope` (contract 3.5).
    /// Returns the assigned frame seq (the resume cursor a viewer backfills from). The push is
    /// allowed-to-drop (firehose semantics): if lost, the TTL backstop + the next viewport resolve still
    /// re-fetch. The implementation is the gateway's firehose publish (CHAT-P10); a test models it.
    fn push_card_update(&self, scope: &FirehoseScope, invalidated: &ArtifactRef) -> u64;
}

// ───────────────────────────── the bus-driven invalidation consumer (§4.4) ────────────────────────

/// **The unfurl-invalidation consumer (CHAT-P14, §4.4) — the bus-driven bust over the ONE shared
/// cache.** An [`EventHandler`] (contract 2.4) matching the whitelisted `*.updated`/`*.erased`/
/// `ci.check.updated` set ([`invalidates_card`]); on a match it **busts the shared cache entry** for the
/// event's subject ref ([`UnfurlCache::bust`] — the CHAT-P13 hook, never a second cache) so the next
/// resolve re-fetches live. It holds a CLONE of the one shared [`UnfurlCache`] (the cache is an
/// `Arc`-backed handle, so the consumer and the [`super::UnfurlService`] bust/read the SAME entries).
///
/// The bust is keyed by the **root-stripped** ref ([`myelin_refs::strip_sub`]): the cache is keyed by
/// the root projection (a `#sub`-anchored card and its parent share the one projection entry), so an
/// `issue.updated` busts the issue card whether or not the embed carried a `#sub` (§4.2 — the tombstone
/// always carries the root).
///
/// **Idempotent (contract 2.5):** the [`Consumer`] runtime owns `consumer_dedup` (a redelivered event
/// is absorbed by the ledger before `handle`); and the handler is idempotent BY CONSTRUCTION — busting
/// an already-busted (absent) entry is a no-op ([`UnfurlCache::bust`] returns `false`, the handler still
/// returns [`HandleOutcome::Done`]). So an at-least-once redelivery is effectively-once.
///
/// **The live push** is OPTIONAL: bind it via [`UnfurlInvalidator::with_firehose`] so the consumer also
/// pushes a live card-update frame (CHAT-D7); without it the consumer only busts (the next viewport
/// resolve re-fetches — the bust is still correct, just not pushed live). The firehose seam is the
/// CHAT-P10 transport (a named floor); here the in-memory [`Firehose`] models it.
pub struct UnfurlInvalidator {
    /// The ONE shared cache (an `Arc` handle — busting here busts what the service reads).
    cache: UnfurlCache,
    /// The whitelisted subject prefixes (never `*`) — the `EventHandler::subjects` whitelist.
    subjects: &'static [SubjectPattern],
}

// The `subjects()` whitelist must be a `&'static [SubjectPattern]`; build it once.
use std::sync::OnceLock;
fn invalidation_subject_patterns() -> &'static [SubjectPattern] {
    static PATTERNS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            UNFURL_INVALIDATION_SUBJECTS
                .iter()
                .map(|s| SubjectPattern((*s).to_string()))
                .collect()
        })
        .as_slice()
}

impl UnfurlInvalidator {
    /// Compose the invalidator over the ONE shared cache (the cache the [`super::UnfurlService`] reads;
    /// pass `service.cache().clone()`). The subject whitelist is the frozen
    /// [`UNFURL_INVALIDATION_SUBJECTS`] set (never `*`).
    pub fn new(cache: UnfurlCache) -> UnfurlInvalidator {
        UnfurlInvalidator {
            cache,
            subjects: invalidation_subject_patterns(),
        }
    }

    /// The whitelisted subjects (the `EventHandler::subjects` whitelist — never `*`).
    pub fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    /// **Should this event bust a card?** The precise `*.updated`/`*.erased`/`ci.check.updated`/
    /// `*.revoked` decision (mandatory-core; [`invalidates_card`]). Exposed so a test asserts the
    /// match precision (a `created` does NOT bust; an `updated`/`erased`/`ci.check.updated` does).
    pub fn should_bust(&self, ev: &EventEnvelope) -> bool {
        invalidates_card(&ev.type_.0)
    }

    /// **Apply the invalidation: bust the shared entry for the event's subject (the root-stripped
    /// ref).** Returns `true` iff an entry was actually busted (the next resolve re-fetches). A no-op
    /// (returns `false`) if the event does not match OR no entry was cached — idempotent, fail-safe.
    pub fn invalidate(&self, ev: &EventEnvelope) -> bool {
        if !self.should_bust(ev) {
            return false;
        }
        // The envelope subject is a tenancy `ArtifactRef`; the cache keys on the refs `ArtifactRef`
        // (the same canonical string). Bust the ROOT-stripped ref (the cache keys the root projection).
        let subject_ref = ArtifactRef(ev.subject.0.clone());
        let root = myelin_refs::strip_sub(&subject_ref);
        self.cache.bust(&root)
    }

    /// Bind a live card-update push (CHAT-D7): the consumer also pushes a live card-update frame on the
    /// channel scope after busting, THROUGH the gateway's [`CardUpdatePush`] port (never a second
    /// firehose handle here — the transport is the gateway's, arch §9). Returns a
    /// [`LiveUnfurlInvalidator`]. Without this the consumer only busts (correct, but the live update
    /// waits for the next viewport resolve). The push impl is the CHAT-P10 gateway firehose (a floor).
    pub fn with_push<P: CardUpdatePush>(self, push: P) -> LiveUnfurlInvalidator<P> {
        LiveUnfurlInvalidator { inner: self, push }
    }

    /// Bind this invalidator into the ONE frozen consumer runtime (contract 2.4 — the seven rules +
    /// `consumer_dedup`). Builds the [`Subscription`] over the whitelisted subjects (the `*`-rejection
    /// is structural) + a fresh [`DedupLedger`] (contract 2.5 — idempotent redelivery). The durable
    /// `name` is stable so a reconnect re-binds the SAME cursor + ledger.
    pub fn into_consumer(self, name: &str) -> Consumer<UnfurlInvalidator> {
        let subscription = Subscription::bind(
            ConsumerName(name.into()),
            UNFURL_INVALIDATION_SUBJECTS,
            PrefetchBound::DEFAULT,
        )
        .expect("the unfurl-invalidation subjects are a `*`-free whitelist (never over-broad)");
        Consumer::new(self, subscription, DedupLedger::new())
    }
}

impl EventHandler for UnfurlInvalidator {
    fn subjects(&self) -> &'static [SubjectPattern] {
        self.subjects
    }

    /// Idempotent on `event_id` (the runtime's `consumer_dedup` absorbs a redelivery before this).
    /// Busts the shared entry on a matching `*.updated`/`*.erased`/`ci.check.updated` event; a no-op
    /// otherwise. Always [`HandleOutcome::Done`] — a bust is total (it cannot poison): an absent entry
    /// is a no-op, never an error, so there is no NonRetryable/Retry path here.
    fn handle(&self, ev: &EventEnvelope) -> HandleOutcome {
        self.invalidate(ev);
        HandleOutcome::Done
    }
}

/// **The invalidator + the live card-update push (CHAT-D7).** Wraps [`UnfurlInvalidator`] with the
/// gateway's [`CardUpdatePush`] port (contract 3.5): on a matching event it busts the shared entry AND
/// pushes a live card-update frame on the channel scope THROUGH the gateway's firehose (never a second
/// transport here — arch §9), so a viewer showing the card gets the re-resolved card WITHIN BUDGET.
pub struct LiveUnfurlInvalidator<P: CardUpdatePush> {
    inner: UnfurlInvalidator,
    push: P,
}

impl<P: CardUpdatePush> LiveUnfurlInvalidator<P> {
    /// The underlying bust-only invalidator (so a test reads the shared cache / the match decision).
    pub fn inner(&self) -> &UnfurlInvalidator {
        &self.inner
    }

    /// **Invalidate + push live (CHAT-D7): bust the shared entry, then push a live card-update frame on
    /// the channel scope through the gateway port.** Returns `(busted, Some(frame_seq))` iff the event
    /// matched; the frame seq is the resume cursor the viewer backfills from. A non-matching event
    /// returns `(false, None)` (no bust, no push). The scope is the `channel:<id>` of the conversation
    /// showing the card (a bounded selector, never `*`).
    pub fn invalidate_and_push(
        &self,
        ev: &EventEnvelope,
        scope: &FirehoseScope,
    ) -> (bool, Option<u64>) {
        let busted = self.inner.invalidate(ev);
        if !self.inner.should_bust(ev) {
            return (false, None);
        }
        // Push the live update even if the entry was already absent (a viewer may be showing a card the
        // entry for which expired by TTL — the live frame tells them to re-resolve regardless). The
        // push goes through the gateway port (the bust frame carries the ROOT ref, never a title).
        let subject_ref = ArtifactRef(ev.subject.0.clone());
        let root = myelin_refs::strip_sub(&subject_ref);
        let seq = self.push.push_card_update(scope, &root);
        (busted, Some(seq))
    }
}

// ───────────────────────────── erasure-safe re-render (§6 / §4.5; CHAT-D6) ─────────────────────────

/// **Erasure-safe re-render (CHAT-D6) — re-resolve a previously-cached card after the `*.erased` bust
/// and assert the next render is a tombstone with 0 recoverable PII.** The proof helper that models the
/// full CHAT-D6 path over the ONE shared cache + the CHAT-P13 resolver:
/// 1. the card was cached live (a third party's name in the title) — the shared entry exists;
/// 2. the `*.erased` event busts the entry ([`UnfurlInvalidator::invalidate`]) — the cached projection
///    is DROPPED (no durable snapshot: the cache is the only place rendered content ever lived, §4.5);
/// 3. the next resolve hits the resolver, which now returns [`LadderOutcome::Erased`] (the third party
///    was crypto-shredded) → a **Tombstone**, 0 recoverable PII.
///
/// This function performs step 3 (the re-resolve after the bust) and returns the re-rendered [`Card`],
/// asserting structurally that a busted entry re-resolves (it returns the resolver's outcome, never a
/// stale cached title). The caller (the drill) drives steps 1–2 and asserts the returned card is a
/// tombstone carrying no PII. The cache, after the bust, holds NO entry for the ref — there is **nothing
/// to recover** (the no-recoverable-PII property is structural: rendered content lives ONLY in the
/// cache, and the bust dropped it).
pub fn erasure_safe_rerender<R: RefsResolvePort>(
    cache: &UnfurlCache,
    resolver: &R,
    tenant: &myelin_tenancy::TenantId,
    region: &myelin_tenancy::Region,
    ref_: &ArtifactRef,
    viewer: &myelin_identity::Principal,
    at: &myelin_identity::Consistency,
) -> Card {
    // The entry MUST already be busted (the `*.erased` bust dropped it) — re-resolve live.
    debug_assert!(
        !cache.contains(ref_),
        "erasure_safe_rerender is called AFTER the *.erased bust — the entry must be gone (no durable \
         snapshot; the cache is the only place rendered content lived, §4.5)"
    );
    let outcome = resolver.resolve(tenant, region, ref_, viewer, at);
    match outcome {
        // The re-resolve returns Erased (the third party was shredded) → a tombstone, 0 recoverable PII.
        // It is NOT re-cached as content (a gone/erased outcome is the absence of content, §4.2).
        LadderOutcome::Erased(tombstone) | LadderOutcome::Gone(tombstone) => {
            Card::Tombstone(tombstone)
        }
        // If the resolver still returns live (the erase has not yet propagated to the owner), the card
        // re-fills — but with the FRESH projection, never the stale cached one. (In the CHAT-D6 path the
        // resolver returns Erased; this arm is the honest non-erased case.)
        LadderOutcome::Live(projection) => {
            cache.put(ref_, projection.clone());
            Card::Live {
                projection,
                moved: false,
                outdated: false,
            }
        }
        LadderOutcome::Moved(projection) => {
            cache.put(ref_, projection.clone());
            Card::Live {
                projection,
                moved: true,
                outdated: false,
            }
        }
        LadderOutcome::Outdated(projection) => {
            cache.put(ref_, projection.clone());
            Card::Live {
                projection,
                moved: false,
                outdated: true,
            }
        }
    }
}

// ───────────────────────────── the `#sub` anchor stability (§2; CHAT-D18) ──────────────────────────

/// **`#sub` anchor stability (CHAT-D18) — the anchor lifecycle of a `...#message-<id>` embed across the
/// referenced message's edit/delete (§2, contract 5.7).** A message id is immutable, so its `#sub`
/// (`message-<id>`) is **stable across edits** — an embed referencing it stays LIVE through any number of
/// edits (an edit is a new state on the SAME id, never a new id). A delete degrades the embed to a
/// **Tombstone carrying the root** (the `#sub`-stripped channel) — it NEVER dangles (the dangling-anchor
/// signal is structurally 0: the tombstone always carries the root, §4.2).
pub mod anchor {
    use super::*;

    /// The lifecycle state of a referenced message (the producer of the `#sub` ladder outcome). Chat's
    /// message has no `moved`/`outdated` state — it is content-addressed by a stable id (§2): an edit is
    /// a new state on the SAME id (still `Live`), a delete is `Deleted`, an erase is `Erased`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum MessageLifecycle {
        /// The message exists (possibly edited any number of times — the id is immutable, so the
        /// `#sub` is stable; the embed stays LIVE).
        Live,
        /// The message was deleted (a tombstone with the root channel — the embed never dangles).
        Deleted,
        /// The message was erased (crypto-shred — a tombstone, "[erased]").
        Erased,
    }

    /// **Resolve a `...#message-<id>` embed to its ladder outcome (CHAT-D18).** Given the referenced
    /// message's lifecycle, returns the [`LadderOutcome`] the unfurl service maps to a card. The
    /// invariants this enforces (asserted by the drill):
    /// - **edit → the anchor stays stable/live.** Any number of edits keeps `MessageLifecycle::Live`
    ///   (the id, and thus the `#sub`, is unchanged) → [`LadderOutcome::Live`] with the SAME
    ///   `sub_anchor` (`message-<id>`). The embed does NOT dangle and does NOT degrade.
    /// - **delete → degrade to a Tombstone carrying the ROOT.** `MessageLifecycle::Deleted` →
    ///   [`LadderOutcome::Gone`] carrying [`myelin_refs::strip_sub`] of the embed (the channel) — never
    ///   a dangling anchor (the tombstone always carries the root, §4.2).
    /// - **erase → a Tombstone, "[erased]".** `MessageLifecycle::Erased` → [`LadderOutcome::Erased`]
    ///   carrying the root.
    ///
    /// `embed` is the full `...#message-<id>` ref; `projection_title` is the live message preview (the
    /// owner's `project()` output) used only for the `Live` outcome. The function NEVER puts a title in
    /// a tombstone (the tombstone carries the root only — leak-free, dangling-free).
    pub fn resolve_message_anchor(
        embed: &ArtifactRef,
        lifecycle: MessageLifecycle,
        projection_title: &str,
    ) -> LadderOutcome {
        let sub_anchor = message_sub_anchor(embed);
        let root = myelin_refs::strip_sub(embed);
        match lifecycle {
            MessageLifecycle::Live => LadderOutcome::Live(super::super::Projection {
                title: projection_title.to_string(),
                state: "live".to_string(),
                icon: "message".to_string(),
                // the STABLE `message-<id>` anchor — unchanged across edits.
                sub_anchor,
            }),
            MessageLifecycle::Deleted => LadderOutcome::Gone(Tombstone {
                root,
                reason: TombstoneReason::Gone,
            }),
            MessageLifecycle::Erased => LadderOutcome::Erased(Tombstone {
                root,
                reason: TombstoneReason::Erased,
            }),
        }
    }

    /// Extract the `message-<id>` `#sub` anchor from a `...#message-<id>` embed (the stable opaque id,
    /// §2). Returns the `#sub` segment (e.g. `message-01J...`) or `None` for a bare-root ref. The id is
    /// the immutable `message_id` ULID — stable across edits, so this NEVER changes for a given message.
    pub fn message_sub_anchor(embed: &ArtifactRef) -> Option<String> {
        embed.0.split_once('#').map(|(_, sub)| sub.to_string())
    }

    /// **The dangling-anchor probe (the GATE, dangling signal == 0): a tombstone for a deleted/erased
    /// embed ALWAYS carries the root, never a dangling `#sub`.** Returns `true` iff the outcome is
    /// dangling-free: a `Gone`/`Erased` outcome carries a non-empty root (the channel), and a `Live`
    /// outcome carries the stable `#sub`. A drill asserts this is `true` for every lifecycle (0 dangling
    /// anchors).
    pub fn is_dangle_free(outcome: &LadderOutcome) -> bool {
        match outcome {
            LadderOutcome::Live(p) | LadderOutcome::Moved(p) | LadderOutcome::Outdated(p) => {
                // a live anchor carries its stable `#sub` (or is a bare-root live card).
                p.sub_anchor.as_deref().map(str::is_empty) != Some(true)
            }
            LadderOutcome::Gone(t) | LadderOutcome::Erased(t) => {
                // a tombstone ALWAYS carries the root (never an empty/dangling anchor).
                !t.root.0.is_empty() && !t.root.0.contains('#')
            }
        }
    }
}

#[cfg(test)]
mod tests;
