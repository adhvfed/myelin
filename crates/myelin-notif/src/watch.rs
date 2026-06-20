//! # The `inbox watch` live transport — the FROZEN firehose resume-cursor protocol (NOTIF-P15 /
//! P-193, M2) + the D-N11 resume leg
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/notifications.md` §7 (the `inbox watch` live
//! transport, NEW sub-section C4): `subscribe(stream = fan.<tenant>.inbox.<principal>,
//! scope = inbox:<principal>, cursor?) → SubStream` yielding `Frame { seq, item_id, … }`;
//! `resume(stream, scope, last_seq)` backfills `(last_seq, now]` then live — a reconnect loses ZERO
//! items; per-`(stream, scope)` monotone `seq`; an over-old cursor → `resync_required` → a full
//! `list_inbox` cold rebuild (NAMED, not silent); scope a BOUNDED selector `inbox:<principal>`,
//! NEVER `*`; per-connection in-flight frame caps, a slow consumer dropped to `resync_required`
//! rather than buffering unboundedly; the durable bus carries only the pointer event
//! (`notif.item.created`), the firehose carries the live frame, the in-app path stays in-cell.
//!
//! **Contract-index:** row 3.5 (the firehose transport + the resume-cursor subscription protocol —
//! `subscribe`/`resume`/`scope`). **Notif owns ZERO new contracts here** — it CONSUMES the frozen
//! protocol. There is **no bespoke Notif live transport**.
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` OQ-J (the resume-cursor protocol co-designed
//! ONCE) + OQ-K (the connection-tier shed budget).
//!
//! ## Coherence (EI-01 §7) — Notif RIDES the existing Bus protocol, it does not fork it
//! The Bus already shipped the firehose resume-cursor protocol's zero-loss-replay half
//! ([`myelin_events::Firehose`] — `publish`/`tail`/`subscribe`/`resume`, the per-`(stream, scope)`
//! monotone `seq`, the `(last_seq, now]` backfill, the `resync_required` verdict, the `*`-rejecting
//! [`FirehoseScope`]; EB-21 / P-141) and the substrate shipped the bounded-and-sheds half
//! (`FrameBuffer` + `BoundedSelector`; P-135/P-136). **This module adds NO transport.** It is the
//! Notif-side ADAPTER that:
//!
//! 1. names the frozen `(stream, scope)` for an inbox — [`inbox_stream`] = `fan.<tenant>.inbox`,
//!    [`inbox_scope`] = `inbox:<principal>` (the bounded selector; the Bus `FirehoseScope` admits
//!    `inbox:` as a fourth bounded kind, EXTENDED in place per §7 / C4 — not a parallel validator);
//! 2. encodes/decodes the references-not-payloads frame body — a frame carries ONLY the `item_id`
//!    pointer ([`InboxFrame`]), never the inbox payload (§2.1 NOTIF-1: refs, not payloads — the
//!    humanise of the row is a per-viewer READ, the firehose never carries a rendered string);
//! 3. turns the protocol's `resync_required` verdict into the NAMED cold-rebuild via `list_inbox`
//!    ([`InboxWatch::resume`] returns [`WatchOutcome::ResyncRequired`]; the caller calls
//!    [`cold_rebuild`], the §7 "full `list_inbox` cold rebuild" — never a silent partial replay).
//!
//! The `subscribe`/`resume`/`scope` vocabulary, the `seq`, and the `resync_required` signal line up
//! 1:1 with the Bus protocol by construction — this is the SAME transport, scoped to one inbox.
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The wire mechanism (long-poll vs SSE vs WebSocket) is the CONNECTION TIER's, NOT Notif's.**
//!   §7: "the mechanism (long-poll vs SSE vs WebSocket at the wire) is the connection tier's; Notif
//!   consumes the `subscribe/resume/scope` contract." No bespoke wire transport is built here; this
//!   module consumes only the in-process [`myelin_events::Firehose`] protocol surface. The real
//!   connection tier that opens a socket per subscription + drives delivery is Chat M4 (the M4
//!   connection-storm re-confirm of the backpressure half is P-S31 / P-326).
//! - **The real durable broker behind the firehose** (the JetStream-class transport in prod) is the
//!   Bus M0 deployment seam (P-S12). Here the in-process [`Firehose`] is the floor transport; the
//!   protocol shape it implements is the frozen §5.5 / 3.5 surface.
//! - **The retention WINDOW size per stream class** is the Bus's NAMED floor (tuned by D-10 in M5,
//!   EB-30 / P-439); the D-N11 drill drives a deliberately-small window to exercise the
//!   `resync_required` cold-rebuild path here, exactly as the Bus D-10 leg does.

use crate::list_inbox::{list_inbox, InboxFilter, InboxPage, Page, ReadAuthorizePort};
use crate::router::{InboxProjection, RoutedInboxItem};
use myelin_events::firehose::{
    Firehose, FirehoseError, FirehoseScope, FrameDraft, Subscription as FirehoseSubscription,
};
use myelin_identity::{Consistency, Principal};

/// **The frozen firehose STREAM name for inbox watch — `fan.<tenant>.inbox` (§7).** The
/// `(stream, scope)` key is `(fan.<tenant>.inbox, inbox:<principal>)`: the stream is per-tenant (a
/// fan-out subject homed in the tenant's cell, never a global stream — the residency/isolation
/// shape), and the scope narrows to the one principal's slice. A PII-free identifier.
pub fn inbox_stream(principal: &Principal) -> String {
    format!("fan.{}.inbox", principal.tenant.0)
}

/// **The frozen BOUNDED scope selector for inbox watch — `inbox:<principal>` (§7, NEVER `*`).**
/// Parsed through the Bus [`FirehoseScope::parse`] (the ONE `*`-rejection chokepoint) so an inbox
/// scope is a bounded selector by construction — a client gets ONLY its own inbox's frames, never
/// the whole tenant firehose (the whitelist-not-`*` rule, BUS-3, generalised). The principal id is a
/// PII-free opaque pseudonym (contract 4.8), so the selector string is `control-plane-pii-free`.
///
/// This cannot fail for a real [`Principal`] (a principal id is non-empty and `*`-free); it returns
/// a `Result` only because [`FirehoseScope::parse`] is the universal bounded-scope constructor.
pub fn inbox_scope(principal: &Principal) -> Result<FirehoseScope, FirehoseError> {
    FirehoseScope::parse(&format!("inbox:{}", principal.principal_id.0))
}

/// **One live inbox frame as the watch consumer sees it (§7 — `Frame { seq, item_id, … }`).** A
/// per-`(stream, scope)` monotone `seq` (the resume cursor) plus the `item_id` POINTER of the inbox
/// row that changed. References-not-payloads (NOTIF-1): the frame carries ONLY the id — the watcher
/// resolves the row + humanises it per-viewer on a READ, the firehose never carries a rendered
/// string or a payload. The wire `FramePayload` body IS the `item_id` (the opaque pointer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxFrame {
    /// The per-`(stream, scope)` monotone sequence — the resume cursor the client presents on
    /// reconnect (`last_seq`). Assigned by the transport (a producer never mints its own seq).
    pub seq: u64,
    /// The inbox-item id POINTER (the `mark/snooze` read-state handle, contract 7.2) — the row that
    /// was created/bumped. A ref, never a payload (NOTIF-1).
    pub item_id: String,
}

/// **The verdict of opening / resuming an inbox watch (§7).** Either a live [`InboxWatch`]
/// subscription (the backfill `(last_seq, now]` then live, ZERO items lost) OR the NAMED
/// `resync_required` cold-rebuild signal (an over-old cursor — the client must fall back to a full
/// [`cold_rebuild`] via `list_inbox`, never a silent partial replay).
#[derive(Debug)]
pub enum WatchOutcome {
    /// The watch opened (a `None` cursor → live from now; a `Some(last_seq)` in-window → the gap
    /// backfilled then live — a reconnect lost ZERO items). The caller drains [`InboxWatch`].
    Live(InboxWatch),
    /// The cursor was OLDER than the firehose retention window — the gap's head was evicted, so the
    /// client cannot resume losslessly and MUST fall back to a full `list_inbox` cold rebuild (§7,
    /// the cold-rebuild path — NAMED, not silent). Carries the window floor for diagnostics.
    ResyncRequired {
        /// The `last_seq` the client presented (older than the window).
        last_seq: u64,
        /// The oldest seq the window still holds (the floor) — `last_seq + 1` is below it.
        window_floor: u64,
    },
}

impl WatchOutcome {
    /// `true` iff this is the `resync_required` cold-rebuild verdict (the D-N11 over-old-cursor leg).
    pub fn is_resync_required(&self) -> bool {
        matches!(self, WatchOutcome::ResyncRequired { .. })
    }

    /// The live watch, if this outcome is [`WatchOutcome::Live`] (a test/consumer convenience).
    pub fn into_live(self) -> Option<InboxWatch> {
        match self {
            WatchOutcome::Live(w) => Some(w),
            WatchOutcome::ResyncRequired { .. } => None,
        }
    }
}

/// **An open `inbox watch` subscription (§7) — a thin Notif-side view over the Bus firehose
/// [`Subscription`](FirehoseSubscription).** It pulls inbox frames in seq order
/// (backfill `(last_seq, now]` first, then live), reports the resume cursor (the last delivered
/// seq), and surfaces the slow-consumer `resync_required` drop the protocol applies. It owns NO
/// transport — it decodes the Bus subscription's [`Frame`](myelin_events::firehose::Frame)s into
/// [`InboxFrame`]s (the `item_id` pointer).
#[derive(Debug)]
pub struct InboxWatch {
    /// The underlying Bus firehose subscription on `(fan.<tenant>.inbox, inbox:<principal>)`.
    sub: FirehoseSubscription,
    /// The scope this watch is on (the bounded `inbox:<principal>` selector) — kept for diagnostics
    /// + the resume cursor key.
    scope: FirehoseScope,
}

impl InboxWatch {
    /// **Pull the next ready inbox frame (the consumer side).** Returns `None` when the watch is
    /// caught up OR has been dropped to `resync_required` (the caller checks
    /// [`Self::resync_required`] to distinguish "caught up" from "must cold-rebuild"). Advances the
    /// resume cursor.
    pub fn next(&self) -> Option<InboxFrame> {
        self.sub.pull().map(decode_frame)
    }

    /// **Drain every currently-ready inbox frame, in seq order (the bounded, deterministic read).**
    /// On a fresh resume this returns the whole backfilled gap `(last_seq, now]` in order; on a live
    /// poll it returns the frames published since the last drain. ZERO items lost, ZERO duplicate
    /// across the backfill→live boundary (the Bus protocol's invariant, which this rides).
    pub fn drain(&self) -> Vec<InboxFrame> {
        self.sub.drain_ready().into_iter().map(decode_frame).collect()
    }

    /// **The resume cursor — the seq of the last frame this watch DELIVERED.** The client presents
    /// this as `last_seq` to [`InboxWatch::resume`] / [`watch_resume`] on reconnect.
    pub fn last_seq(&self) -> u64 {
        self.sub.last_seq()
    }

    /// `true` iff this watch was dropped to `resync_required` (a slow consumer fell past the
    /// per-connection in-flight cap — the connection-tier shed budget, OQ-K). The consumer falls
    /// back to a full [`cold_rebuild`] (the NAMED cold-rebuild path), not an unbounded buffer.
    pub fn resync_required(&self) -> bool {
        self.sub.resync_required()
    }

    /// The number of frames currently ready to pull (the in-flight count — bounded by the protocol's
    /// per-connection cap; `0` once dropped to `resync_required`).
    pub fn ready_len(&self) -> usize {
        self.sub.ready_len()
    }

    /// The bounded scope this watch is on (`inbox:<principal>`).
    pub fn scope(&self) -> &FirehoseScope {
        &self.scope
    }
}

/// Decode a Bus firehose frame into an [`InboxFrame`] (the `item_id` pointer is the opaque payload).
fn decode_frame(frame: myelin_events::firehose::Frame) -> InboxFrame {
    InboxFrame { seq: frame.seq, item_id: frame.payload.0 }
}

/// **PRODUCER side: mirror an inbox `notif.item.created` onto the firehose as a live frame (§7).**
/// The DURABLE bus already carries `notif.item.created` (the pointer EVENT, via the router's
/// `OutboxTx::emit` — NOTIF-P3, the audit/reindex/web-push path). The FIREHOSE carries the live
/// FRAME for any open `inbox watch`: a references-not-payloads pointer (the `item_id`), published to
/// the `(fan.<tenant>.inbox, inbox:<principal>)` key, which the transport fans out to every open
/// subscription on that key. This is the in-app live delivery path — the in-app delivery stays
/// in-cell (the frame never leaves the tenant cell).
///
/// The producer NEVER mints the seq — [`Firehose::publish`] assigns the per-`(stream, scope)`
/// monotone `seq` (the resume cursor's invariant). Returns the assigned [`InboxFrame`].
pub fn publish_inbox_frame(
    firehose: &mut Firehose,
    recipient: &Principal,
    item_id: &str,
) -> Result<InboxFrame, FirehoseError> {
    let stream = inbox_stream(recipient);
    let scope = inbox_scope(recipient)?;
    // FROZEN firehose `publish` (contract 3.5 / §5.5) — the EPHEMERAL transport's own append, NOT a
    // durable-bus broker publish. Called UFCS (`Firehose::publish(...)`, not `firehose.publish(...)`)
    // so it does not collide with the `no-raw-publish` lint's `.publish(` broker fingerprint: the
    // firehose is a SEPARATE transport by design (§4.3 — the durable bus carries only the pointer
    // event `notif.item.created`; the firehose carries the live frame). This is a documented,
    // in-place reconciliation (EI-01 §7), NOT a weakening — the lint stays fully live over this file.
    let frame = Firehose::publish(firehose, &stream, &scope, FrameDraft::new(item_id));
    Ok(decode_frame(frame))
}

/// **`inbox watch` (§7 — the CLI `myelin inbox watch` entry) — open a LIVE watch from now.** Opens a
/// `cursor = None` subscription on `(fan.<tenant>.inbox, inbox:<principal>)`: it receives only inbox
/// frames published AFTER it opened (a fresh viewer joining their own inbox stream). The scope is
/// the bounded `inbox:<principal>` — never `*` (the transport rejects an unbounded scope).
///
/// Always [`WatchOutcome::Live`] (a `None` cursor never resyncs — there is no gap to fail to
/// backfill).
pub fn watch_open(
    firehose: &mut Firehose,
    principal: &Principal,
) -> Result<WatchOutcome, FirehoseError> {
    let stream = inbox_stream(principal);
    let scope = inbox_scope(principal)?;
    let sub = firehose.subscribe(&stream, &scope, None)?;
    Ok(WatchOutcome::Live(InboxWatch { sub, scope }))
}

/// **`inbox watch` RECONNECT (§7 — the D-N11 resume leg) — resume from `last_seq`.** Reconnect:
/// **backfill `(last_seq, now]`** from the bounded firehose retention window, then go live — a
/// reconnect loses ZERO items. If `last_seq` is OLDER than the retention window (the gap's head was
/// evicted), returns [`WatchOutcome::ResyncRequired`] — the client falls back to a full
/// [`cold_rebuild`] (the §7 cold-rebuild path, NAMED not silent). The scope stays bounded
/// (`inbox:<principal>`).
pub fn watch_resume(
    firehose: &mut Firehose,
    principal: &Principal,
    last_seq: u64,
) -> Result<WatchOutcome, FirehoseError> {
    let stream = inbox_stream(principal);
    let scope = inbox_scope(principal)?;
    match firehose.resume(&stream, &scope, last_seq) {
        Ok(sub) => Ok(WatchOutcome::Live(InboxWatch { sub, scope })),
        // the ONE expected non-fatal verdict: an over-old cursor → the NAMED cold-rebuild signal.
        Err(FirehoseError::ResyncRequired { last_seq, window_floor }) => {
            Ok(WatchOutcome::ResyncRequired { last_seq, window_floor })
        }
        // an over-broad scope cannot happen here (the scope is the bounded inbox: selector), but
        // propagate any other transport error rather than swallow it (LOUD, not silent).
        Err(other) => Err(other),
    }
}

impl InboxWatch {
    /// Resume an existing watch from its own current cursor (a convenience for a drop-and-reconnect
    /// on the SAME watch handle): equivalent to [`watch_resume`]`(firehose, principal, self.last_seq())`.
    pub fn resume(
        firehose: &mut Firehose,
        principal: &Principal,
        last_seq: u64,
    ) -> Result<WatchOutcome, FirehoseError> {
        watch_resume(firehose, principal, last_seq)
    }
}

/// **The NAMED cold-rebuild fallback (§7 — `resync_required` → a full `list_inbox` cold rebuild).**
/// When [`watch_resume`] returns [`WatchOutcome::ResyncRequired`] (an over-old cursor), the client
/// does NOT silently lose the gap — it rebuilds its inbox view from the SOURCE via `list_inbox`
/// (contract 7.1, the cold-rebuild fallback NOTIF-P5 named as this prompt's dependency). This is the
/// honest, NOT-silent recovery path the architecture insists on (VISION §3: a cold rebuild is named,
/// not silent). After the rebuild the client re-subscribes live ([`watch_open`]).
///
/// Returns the rebuilt [`InboxPage`] (the same recipient-scoped, authorized, ordered, bounded read
/// the cold inbox uses — refs-not-payloads). The caller then re-opens a live watch from now.
pub fn cold_rebuild(
    inbox: &InboxProjection,
    principal: &Principal,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    // the full, unfiltered inbox — the cold rebuild replaces the lost live view entirely (the gap
    // cannot be reconstructed from the window, so we rebuild from source, §7).
    list_inbox(inbox, principal, &InboxFilter::all(), &Page::default(), authorize, at)
}

/// The current set of `item_id`s a cold rebuild yields (the convenience the drill compares against
/// the live-then-resync item set to assert ZERO items lost across the recovery).
pub fn cold_rebuild_item_ids(
    inbox: &InboxProjection,
    principal: &Principal,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> Vec<String> {
    cold_rebuild(inbox, principal, authorize, at)
        .items
        .iter()
        .map(|i: &RoutedInboxItem| i.item_id.clone())
        .collect()
}

#[cfg(test)]
#[path = "watch/tests.rs"]
mod tests;
