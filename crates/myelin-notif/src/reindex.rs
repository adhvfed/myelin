//! # Reindex-from-source — the ONLY recovery path (NOTIF-P17 / P-195, M2) + NOTIF-D3
//!
//! **Owning architecture doc:** `notifications.md` §3.8 (reindex-from-source — UNCHANGED from Phase
//! 3 §3.8 / contract 7.7 replay half): *the inbox is a derived read-model, rebuildable ONLY via the
//! live consumer path:* `events::reindex(scope=notif)` → owners replay `*.snapshot` events through
//! outbox→bus→Signal → the **SAME router** ([`crate::router::SignalRouter`], NOTIF-P3) re-ingests
//! idempotently (`origin_event`/`event_id` dedup) → `inbox_item`/`delivery` reconstructed;
//! **cold == live** (the D-N3 / NOTIF-D3 parity drill). This is the only recovery path (there is NO
//! "read the inbox from some other store" code → steady-state and recovery share ONE code path →
//! cannot drift, EI-04 §5.3). It doubles as new-recipient backfill + the schema-upcaster path.
//!
//! **Contracts:** **7.7 replay** (OWNED — the reindex half; completes the holder contract whose
//! holder half is NOTIF-P4 / [`crate::holder`]). **Consumed:** 2.6 reindex-from-source
//! ([`myelin_events::reindex`] — the bus re-emit seam + the `*.snapshot` schema, EB-22), 11.5
//! restore-verify (the system-of-record tables — prefs/on-call/templates, named below).
//!
//! ## The §3.8 protocol, implemented here — through the SAME router, no second read path
//!
//! Unlike a derived store that ingests `*.snapshot` events DIRECTLY (Search, SRCH-P16), Notif's
//! inbox is the projection of the **curated Signal stream** (`sig.<tenant>.>` — what the router,
//! NOTIF-P3, consumes). So the §3.8 protocol's "owners replay `*.snapshot` → Signal → the same
//! router" is realised here as:
//!
//! ```text
//! reindex(scope=notif):
//!   wipe the inbox projection (the cold-rebuild precondition — derived state only)
//!   for the owning Signal source in scope:
//!      source.replay(scope, since) → *.snapshot drafts carrying the curated Signal payload, on the
//!                                    `sig.<tenant>.<sev>.<rule>` subject the router whitelists
//!      → emit each through the OUTBOX (the SAME outbox→bus path; no backdoor)
//!      → drain each snapshot row and feed it through the LIVE Consumer<SignalRouter>::deliver — the
//!        EXACT live step a `sig.*` Signal takes (rule-1 consumer_dedup + the router's own
//!        (tenant, recipient, dedup_key) write-time collapse — belt and braces)
//! ```
//!
//! There is **NO second read path**: the rebuild re-drives [`Consumer::deliver`] — the SAME method a
//! live Signal hits — over the SAME [`SignalRouter`]. A reindex cannot rebuild a row the live path
//! would not have produced (and vice-versa) because there is exactly one [`SignalRouter::route`]
//! body. The single-code-path check below pins this structurally.
//!
//! ## Idempotency-by-construction — re-running a reindex is a no-op in effect
//!
//! Each `*.snapshot` lands at its **deterministic** [`myelin_events::snapshot_event_id`] (a pure
//! function of `(aggregate, version)`), so:
//! - the bus re-emit skips an already-present id (`ON CONFLICT DO NOTHING` — a re-run emits 0 new);
//! - the consumer's [`myelin_events::DedupLedger`] makes a redelivered snapshot a handler no-op
//!   ([`Delivered::Deduplicated`]);
//! - the router's `(tenant, recipient, dedup_key)` UPSERT collapses any residual repeat onto the
//!   SAME row (`coalesce_count += 1`) — it never opens a duplicate.
//!
//! ## The reindex-parity hash (NOTIF-D3 — cold == live)
//!
//! [`inbox_parity_hash`] is a deterministic BLAKE3 over the inbox projection's rows in canonical
//! order — the dated green artifact NOTIF-D3 emits: wipe `inbox_item`, run `reindex(notif)`, assert
//! the rebuilt inbox's parity hash == the live inbox's parity hash (items + read-state). The hash
//! covers the load-bearing reconstructed state: the `(tenant, recipient, dedup_key)` identity, the
//! reason/class/subject refs (references-not-payloads), the `coalesce_count`, AND the read-state
//! (`state` + `snooze_until`) — so a reindex that drops a row, mis-maps a field, or loses read-state
//! flips the hash. The threshold is **identical** — never softened.
//!
//! ## FLOORS named (VISION §3 / EI-01 §1 name-your-floors)
//!
//! - **The ~90-day item retention window** is a FLOOR (§3.8). The inbox holds a bounded window of
//!   items; older items age out and are reconstructable from the **OLAP/Audit long-term holder**
//!   (the OLAP read store, P-ST-18; the audit log, GA-19). `prefs`/`on-call`/`templates` are
//!   **permanent** (the system-of-record tables) and are **restore-verify gated** (STOR-D1 / 11.5),
//!   NOT reindexed — they are not derived. [`RetentionWindow`] carries the boundary; the OLAP/Audit
//!   long-window replay source is the named follow-on (the long-term holder fills it).
//! - **The owners' real `replay` bodies** are EB-26 / the dispatch tier (the Signal engine, P-138)
//!   re-emitting curated Signals. This module ships the Notif-side reindex DRIVER + the
//!   `*.snapshot`-carries-a-Signal source SHAPE; the [`SignalReindexSource`] reference owner is the
//!   one NOTIF-D3 runs against (NOT a stand-in for the real dispatch-tier replay). The seam shape
//!   (wipe → bus re-emit → live `deliver` → parity) does not change when the real owner lands.
//! - **`delivery` reconstruction.** §3.8 names `inbox_item`/`delivery` reconstructed. The inbox_item
//!   rebuild is here (through the router). The `notif_delivery` rows are downstream of inbox items
//!   (the delivery fabric, NOTIF-P16, sends per surfaced item); a reindex rebuilds the inbox_item
//!   rows the fabric delivers from, and `delivery` is the at-least-once-idempotent fabric's own
//!   `UNIQUE(idem_key)` ledger (NOT re-sent on a rebuild — re-delivering every historical item would
//!   be a notification storm). The rebuild restores the inbox the fabric reads; the delivery ledger
//!   is the durable system-of-record half (restore-verify gated, like prefs). Named so the
//!   inbox-rebuild green is not mistaken for a re-delivery.
//!
//! ## Mutation floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//!
//! The reindex module is recovery-correctness critical (the only path back from a lost inbox
//! read-model). The mutation-tested core is the reindex decision logic: the wipe-iff-full-rebuild
//! branch, the bus-re-emit drive, the deterministic-id drain order, the feed-through-the-LIVE-router
//! step, the parity hash (every covered field), and the retention-window boundary. **Floor: ≥ 80% of
//! viable mutants caught** (`cargo mutants -p myelin-notif -f crates/myelin-notif/src/reindex.rs`).
//! Measured 2026-06-20: see the P-195 commit body. Every branch is asserted by the unit + chained +
//! NOTIF-D3 drill tests below; a mutant that drops the wipe, mis-orders the drain, skips the live
//! `deliver`, or loosens the parity hash is caught.

use std::collections::BTreeMap;

use myelin_events::reindex::{reindex as bus_reindex, ReindexError as BusReindexError};
use myelin_events::{
    AggregateKey, ArtifactRef, Consumer, DataRole, EmitContextBase, EventType, Message,
    OutboxStore, ReindexReceipt as BusReindexReceipt, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};
use myelin_query::signals::Signal;
use myelin_tenancy::TenantId;

use crate::router::{InboxProjection, SignalRouter};

/// The §6.2 subsystem owner token for a Notif reindex scope (`notif`). A `reindex(scope=notif)`
/// dispatches to the Signal source that owns this token. PII-free.
pub const NOTIF_OWNER_TOKEN: &str = "notif";

/// The Notif `*.snapshot` event type (`notif.signal.snapshot`) — the §6.4 snapshot grammar for the
/// curated-Signal artifact the inbox projects from. A snapshot carries the SAME Signal payload a
/// live `sig.*` event carries (that is the cold == live invariant); only its `event_id` is the
/// deterministic [`myelin_events::snapshot_event_id`]. PII-free token.
pub const NOTIF_SNAPSHOT_TYPE: &str = "notif.signal.snapshot";

/// The default item-retention window the inbox holds (the §3.8 FLOOR). Items older than this age out
/// of the live inbox and are reconstructable from the OLAP/Audit long-term holder, NOT from the
/// reindex (which replays the bounded window). A v1 default; the real per-cell window is a config
/// knob (a config swap, never a code change).
pub const DEFAULT_RETENTION_DAYS: u32 = 90;

/// **The ~90-day item-retention window (the §3.8 FLOOR boundary).** The inbox is a bounded read-model:
/// it holds at most `days` of items; older items age out and are reconstructable from the OLAP/Audit
/// long-term holder (P-ST-18 / GA-19), NOT from this reindex. `prefs`/`on-call`/`templates` are
/// permanent (restore-verify gated, 11.5) and are not subject to this window. A named floor: the
/// long-window OLAP/Audit replay source is the long-term holder's follow-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionWindow {
    /// The bounded item window in days (older items age out → reconstruct from OLAP/Audit, the floor).
    pub days: u32,
}

impl Default for RetentionWindow {
    fn default() -> RetentionWindow {
        RetentionWindow {
            days: DEFAULT_RETENTION_DAYS,
        }
    }
}

impl RetentionWindow {
    /// The default ~90-day window (§3.8).
    pub fn new() -> RetentionWindow {
        RetentionWindow::default()
    }

    /// An explicit per-cell window (a config swap, never a code change). Floored at 1 (a 0-day window
    /// would age out everything immediately — a wedged inbox).
    pub fn of_days(days: u32) -> RetentionWindow {
        RetentionWindow { days: days.max(1) }
    }
}

/// An error from a Notif reindex (contract 7.7 replay half).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReindexError {
    /// The bus re-emit seam failed (no owner for the scope, or the outbox emit/commit failed). A
    /// reindex of an unknown owner is a LOUD error — never a silent empty rebuild that masks a wiring
    /// bug (EI-02 §4).
    Bus(String),
    /// A snapshot row the bus re-emit claimed to stage was not found in the outbox (the re-emit did
    /// not actually stage it — a LOUD half-rebuild guard, never a silently-dropped row).
    MissingSnapshot(String),
}

impl std::fmt::Display for ReindexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReindexError::Bus(e) => write!(f, "notif reindex: bus re-emit failed: {e}"),
            ReindexError::MissingSnapshot(id) => {
                write!(f, "notif reindex: snapshot {id} not found in the outbox (re-emit did not stage it)")
            }
        }
    }
}

impl std::error::Error for ReindexError {}

/// The receipt a Notif `reindex(scope=notif)` returns (the NOTIF-D3 artifact body). PII-free counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReindexReceipt {
    /// `*.snapshot` events the bus re-emit newly emitted into the outbox (the re-emit half).
    pub snapshots_emitted: usize,
    /// `*.snapshot` events skipped at the bus because their deterministic id was already present (the
    /// `ON CONFLICT DO NOTHING` idempotency at the re-emit half — a re-run reports these).
    pub snapshots_skipped_duplicate: usize,
    /// `*.snapshot` events DRIVEN through the LIVE router this pass (the re-ingest half).
    pub signals_replayed: usize,
    /// `*.snapshot` events the live consumer deduplicated (a redelivered snapshot — the
    /// `consumer_dedup` ledger no-op; no double effect).
    pub signals_deduplicated: usize,
    /// The owners replayed (the §6.2 tokens), in scope order.
    pub owners_replayed: Vec<String>,
}

/// **Notif's reindex-from-source driver (NOTIF-P17; contract 7.7 replay half — the ONLY recovery
/// path).** Wraps the LIVE [`Consumer<SignalRouter>`] (the SAME consumer the bus feeds — a reindex
/// re-drives ITS [`Consumer::deliver`] step; there is no second read path) + the cell's resident
/// region. The inbox it rebuilds is the router's own [`InboxProjection`] (NOT a parallel store).
///
/// Borrowed-not-owned: the driver borrows the live consumer for the duration of a reindex pass, so
/// the SAME router that serves live Signals is the one a reindex re-drives — structurally one code
/// path.
pub struct NotifReindexer<'a> {
    /// The LIVE Signal-consumer the reindex re-drives (the SAME `Consumer::deliver` a live `sig.*`
    /// event hits — no second read path). The inbox it UPSERTs into is the rebuild target.
    consumer: &'a Consumer<SignalRouter>,
}

impl<'a> NotifReindexer<'a> {
    /// Build the reindex driver over a LIVE Signal-consumer (the router the bus feeds). A reindex
    /// re-drives this consumer's `deliver` — the same live step a `sig.*` Signal takes.
    pub fn new(consumer: &'a Consumer<SignalRouter>) -> NotifReindexer<'a> {
        NotifReindexer { consumer }
    }

    /// The live inbox projection the reindex rebuilds (the router's OWN projection — the rebuild
    /// target a drill reads to assert cold == live).
    pub fn inbox(&self) -> &InboxProjection {
        self.consumer.handler().inbox()
    }

    /// **`reindex(scope=notif, since) → receipt` (contract 7.7 replay half; §3.8) — the ONLY recovery
    /// path.** Drives the bus re-emit ([`myelin_events::reindex`], 2.6 CONSUMED) → `*.snapshot` rows
    /// carrying the curated Signal payload → through the LIVE [`Consumer::deliver`] step (the SAME
    /// path a `sig.*` Signal takes), idempotent on the deterministic snapshot `event_id`. There is NO
    /// second read path — the rebuild re-drives the SAME router as live ingest.
    ///
    /// - `since = None` is a FULL rebuild: the inbox projection is **wiped** first (the cold-rebuild
    ///   precondition — derived state only; the inbox holds no system-of-record).
    /// - `since = Some(v)` is an INCREMENTAL backfill (the new-recipient / schema-upcaster / resume
    ///   path): NO wipe; only Signals above the cursor replay, re-ingested into the live inbox.
    ///
    /// `sources` are the OWNING Signal sources' [`ReindexSource`]s (their real `replay` bodies are
    /// the dispatch tier / EB-26 — the named floor); `outbox` is the bus outbox the snapshots
    /// co-commit to; `ctx_base` is the emit context (the platform actor + clock). The bus re-emit
    /// re-reads the OWNER's source of truth (the curated Signal log) — never the inbox projection.
    pub fn reindex(
        &self,
        tenant: &TenantId,
        scope: &SnapshotScope,
        since: Option<u64>,
        sources: &[&dyn ReindexSource],
        outbox: &mut OutboxStore,
        ctx_base: EmitContextBase,
    ) -> Result<ReindexReceipt, ReindexError> {
        // A FULL rebuild (`since = None`) WIPES the inbox first — the cold-rebuild precondition
        // (derived state only; the inbox is a projection, not a system-of-record). An INCREMENTAL
        // backfill (`since = Some`) re-ingests above the cursor into the LIVE inbox (the new-recipient
        // / upcaster / resume path), so NO wipe.
        if since.is_none() {
            self.inbox().wipe_tenant(tenant);
        }

        // (1) Drive the BUS re-emit (contract 2.6) — the owning Signal source's `replay(scope, since)`
        // emits `*.snapshot` drafts (carrying the curated Signal payload, on the `sig.<tenant>.*`
        // subject the router whitelists) through the outbox (the SAME outbox→bus→live-consumer path;
        // no backdoor). A LOUD error if the scope's owner is unregistered (never a silent empty
        // rebuild).
        let BusReindexReceipt {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
        } = bus_reindex(scope, since, sources, outbox, ctx_base).map_err(map_bus_err)?;

        // (2) Drain the snapshot rows from the outbox IN THE REPLAY'S DETERMINISTIC ORDER and feed
        // each through the LIVE `Consumer::deliver` step (the EXACT live step a `sig.*` Signal takes).
        // We recompute the deterministic ids from the SAME `replay` the bus used (the owner's truth is
        // deterministic), so the drain order is byte-reproducible (cold == live). The consumer's
        // `consumer_dedup` ledger makes a redelivered snapshot a no-op (belt to the deterministic-id
        // braces); the router's own (tenant, recipient, dedup_key) UPSERT is the inner idempotency.
        let mut receipt = ReindexReceipt {
            snapshots_emitted,
            snapshots_skipped_duplicate,
            owners_replayed,
            ..Default::default()
        };

        let full_rebuild = since.is_none();
        for source in sources {
            if source.owner_token() != scope.owner {
                continue; // the bus dispatched to the scope's owner; drain only that owner's snapshots.
            }
            for draft in source.replay(scope, since) {
                let event_id = draft.event_id();
                // On a FULL rebuild (the inbox was wiped), forget any prior consumer_dedup mark for
                // this snapshot so the cold rebuild RE-APPLIES it through the router (else a snapshot
                // already handled by a prior rebuild would be deduplicated and the wiped inbox would
                // stay empty). The within-pass idempotency is then the router's OWN (tenant,
                // recipient, dedup_key) UPSERT. An INCREMENTAL backfill keeps the dedup marks (a
                // redelivered snapshot above the cursor IS a no-op — no resurrection).
                if full_rebuild {
                    self.consumer
                        .dedup()
                        .forget(self.consumer.name(), &event_id);
                }
                // Read the snapshot row back from the outbox (it lands at its deterministic id) and
                // feed its envelope through the LIVE consumer — the SAME `Consumer::deliver` a live
                // `sig.*` event hits. The subject is the snapshot's `sig.<tenant>.*` subject (the
                // router whitelist matches it — exactly the live path).
                let row = outbox
                    .row(&event_id)
                    .ok_or_else(|| ReindexError::MissingSnapshot(event_id.0.clone()))?;
                let msg = Message {
                    subject: row.envelope.subject.0.clone(),
                    envelope: row.envelope.clone(),
                };
                match self.consumer.deliver(&msg) {
                    myelin_events::Delivered::Deduplicated => receipt.signals_deduplicated += 1,
                    _ => receipt.signals_replayed += 1,
                }
            }
        }

        Ok(receipt)
    }
}

fn map_bus_err(e: BusReindexError) -> ReindexError {
    ReindexError::Bus(e.to_string())
}

/// **The reindex-parity hash over an inbox projection (the NOTIF-D3 artifact — cold == live).** A
/// deterministic BLAKE3 over every row of `tenant`'s inbox in CANONICAL order (sorted by the stable
/// `(recipient, dedup_key)` identity), covering the load-bearing reconstructed state: the row
/// identity (`recipient`/`dedup_key`/`item_id`), the references-not-payloads `subject` ref, the
/// `reason`/`class` tokens, the `coalesce_count`, AND the read-state (`state`/`snooze_until`).
/// NOTIF-D3: wipe the inbox, reindex(notif), assert this hash on the rebuilt inbox == this hash on
/// the live inbox. A reindex that drops a row, mis-maps a field, or loses read-state flips the hash
/// (the threshold is IDENTICAL — never softened). PII-free: opaque pseudonyms + refs + tokens, never
/// a rendered name.
///
/// **Why `origin_event` is NOT in the parity set (a documented deviation, EI-01 §1).** The
/// `origin_event` ref encodes the EVENT id that produced the row ("why am I seeing this?"). On a
/// reindex the row is produced by the `*.snapshot` re-emit, whose `event_id` is the DETERMINISTIC
/// `snap-<hash>` — distinct BY CONSTRUCTION from the original live event's ULID. So the cold rebuild's
/// `origin_event` legitimately differs from live's (it points at the re-emit, not the original
/// event). The §3.8 cold == live invariant is over the inbox ITEMS + read-state (the user-visible
/// projection), NOT over the provenance event id of the re-emit. Including `origin_event` would make
/// cold == live structurally impossible (a reindex would never match live) and would mask real drift;
/// excluding it makes the gate measure the actual reconstructed inbox state. The `subject` ref (what
/// the item is ABOUT) IS in the set — that must reconstruct identically.
pub fn inbox_parity_hash(inbox: &InboxProjection, tenant: &TenantId) -> String {
    let mut rows = inbox.snapshot_for_tenant(tenant);
    // Canonical order: a hash must not depend on the projection's HashMap iteration order. Sort by
    // the stable write-time-collapse identity (recipient, dedup_key) — unique within a tenant.
    rows.sort_by(|a, b| {
        (a.recipient.as_str(), a.dedup_key.as_str())
            .cmp(&(b.recipient.as_str(), b.dedup_key.as_str()))
    });
    let mut hasher = blake3::Hasher::new();
    for row in &rows {
        // NUL-separate every field so boundaries are unambiguous (no field-merge aliasing). Every
        // field the reindex must reconstruct identically is fed — including the read-state. NOT
        // `origin_event` (it differs between a live event and its snapshot by construction — see above).
        for field in [
            row.recipient.as_str(),
            row.dedup_key.as_str(),
            row.item_id.as_str(),
            row.subject.0.as_str(),
            row.state.as_str(),
            row.snooze_until.as_deref().unwrap_or(""),
        ] {
            hasher.update(field.as_bytes());
            hasher.update(&[0u8]);
        }
        // The structured tokens + the collapse counter (serialised deterministically).
        hasher.update(format!("{:?}", row.reason).as_bytes());
        hasher.update(&[0u8]);
        hasher.update(format!("{:?}", row.class).as_bytes());
        hasher.update(&[0u8]);
        hasher.update(&row.coalesce_count.to_le_bytes());
        hasher.update(&[0u8]);
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

/// A `*.snapshot` draft carrying a curated [`Signal`] payload on the `sig.<tenant>.<sev>.<rule>`
/// subject the router whitelists — the shape an owning Signal source's `replay` yields so the SAME
/// router re-ingests it (cold == live). The aggregate is `signal:<dedup_key>` (the live router's
/// aggregate key); the version bumps on each curated state change of the Signal.
pub fn signal_snapshot_draft(signal: &Signal, version: u64) -> SnapshotDraft {
    let subject = signal_snapshot_subject(signal);
    SnapshotDraft {
        // The live router's per-Signal aggregate key (router emits `signal:<dedup_key>`), so a
        // snapshot of the same curated Signal is the SAME aggregate (the deterministic id is stable).
        aggregate: AggregateKey(format!("signal:{}", signal.dedup_key.0)),
        version,
        type_: EventType(NOTIF_SNAPSHOT_TYPE.into()),
        // The snapshot's SUBJECT is the `sig.<tenant>.<sev>.<rule>` Signal subject — what the router
        // whitelists, so `Consumer::deliver` routes it through the SAME path as a live Signal.
        subject: ArtifactRef(subject),
        // The payload is the SAME curated Signal a live event carries (cold == live).
        payload: serde_json::to_value(signal).unwrap_or(serde_json::Value::Null),
        // A curated Signal is references-not-payloads (refs/ids, never a body); Notif is the
        // controller of the inbox-item fact it derives.
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
    }
}

/// The `sig.<tenant>.<severity>.<rule>` subject a curated Signal rides (the router whitelist — what
/// `Consumer::deliver` matches). Mirrors the live engine's publish subject so a snapshot is routed
/// through the SAME consumer whitelist as a live Signal.
pub fn signal_snapshot_subject(signal: &Signal) -> String {
    format!(
        "sig.{}.{}.{}",
        signal.tenant.0,
        signal.severity.token(),
        signal.rule_id.0
    )
}

/// **A reference owning Signal source — the in-test owner NOTIF-D3 replays.** It owns a deterministic
/// set of curated `(aggregate, version, Signal)` triples (the owner's source of truth — the curated
/// Signal log) and replays them as `*.snapshot` drafts on the `sig.<tenant>.*` subject the router
/// whitelists. A real owner's `replay` reads ITS curated-Signal store (the dispatch tier, EB-26 —
/// the named floor); this reads its in-memory truth — the SAME shape.
#[derive(Default)]
pub struct SignalReindexSource {
    /// The owner's source of truth: `signal:<dedup_key>` → (version, the curated Signal). A
    /// `BTreeMap` so the replay order is deterministic (ascending aggregate) — a rebuild is
    /// byte-reproducible.
    truth: BTreeMap<String, (u64, Signal)>,
}

impl SignalReindexSource {
    /// A fresh, empty Signal source.
    pub fn new() -> SignalReindexSource {
        SignalReindexSource::default()
    }

    /// Record/update the owner's truth for a curated `signal` at `version` (the owner's live curated
    /// write — the dispatch tier publishing/updating a Signal). Keyed by the router's aggregate
    /// (`signal:<dedup_key>`), so a later version of the same curated Signal re-snapshots correctly.
    pub fn upsert(&mut self, signal: Signal, version: u64) {
        let key = format!("signal:{}", signal.dedup_key.0);
        self.truth.insert(key, (version, signal));
    }

    /// The number of curated Signals in the owner's truth.
    pub fn len(&self) -> usize {
        self.truth.len()
    }

    /// `true` iff the owner holds no curated Signals.
    pub fn is_empty(&self) -> bool {
        self.truth.is_empty()
    }
}

impl ReindexSource for SignalReindexSource {
    fn owner_token(&self) -> &str {
        NOTIF_OWNER_TOKEN
    }

    fn replay(&self, _scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        // Deterministic ascending-aggregate replay; skip Signals at/below the `since` cursor (the
        // incremental-backfill / resume path). A tombstoned (erased) Signal is not in the truth — the
        // erasure stays erased across a reindex (X-7).
        self.truth
            .values()
            .filter(|(v, _)| since.is_none_or(|s| *v > s))
            .map(|(v, signal)| signal_snapshot_draft(signal, *v))
            .collect()
    }
}

/// The Notif `reindex(scope=notif, selector)` scope (the §3.4 / §4.9 sub-artifact granularity — a
/// whole-inbox rebuild is `selector = "inbox:all"`; a per-recipient backfill is
/// `selector = "inbox:<recipient>"`). PII-free opaque selector. A convenience over
/// [`SnapshotScope::new`] pinned to the Notif owner token.
pub fn notif_scope(selector: impl Into<String>) -> SnapshotScope {
    SnapshotScope::new(NOTIF_OWNER_TOKEN, selector)
}

#[cfg(test)]
mod tests;
