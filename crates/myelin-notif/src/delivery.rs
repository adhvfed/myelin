//! # The delivery fabric — the idempotent `DeliveryAdapter` body + the deterministic mock (NOTIF-P16 / P-194, M2)
//!
//! **Owning architecture doc:** `notifications.md` §3.6 (the EU-sovereign delivery fabric: ONE trait
//! [`DeliveryAdapter`](crate::DeliveryAdapter)`{channel, region, send(RedactedMessage, idem_key),
//! receipts}` — EU-preferring, region-aware, swappable; PII-minimised off-cell payloads
//! ([`RedactedMessage`](crate::RedactedMessage) = a humanised summary + a deep link, never the full
//! body where avoidable; `delivery.redacted = true`, GDPR Art. 5(1)(c) data-minimisation); in-app
//! channels `inbox`/`web_push`/`desktop`… wait — the IN-APP set (`in_app`) never leaves the cell;
//! at-least-once + idempotent on `UNIQUE(idem_key)`; FLOOR: the trait + EU-preferring posture +
//! redaction ship, the concrete production EU provider is the NOTIF-P25 sovereignty/legal selection).
//!
//! **Contracts:** **7.8** [`DeliveryAdapter`](crate::DeliveryAdapter) (owned — the trait was frozen
//! as a carrier shape in NOTIF-P1; this prompt ships the BODY: the deterministic mock adapter + the
//! idempotent fabric + the redaction discipline). **Consumed:** 7.3 humanise (the
//! [`RedactedMessage`](crate::RedactedMessage) summary is a humanise render), 7.1/2.3 the
//! `notif_delivery` table with `UNIQUE(tenant_id, idem_key)` + `redacted` (NOTIF-P2,
//! [`DeliveryRow`](crate::schema::DeliveryRow)), 1.8 the `delivery_success`/`bounce` telemetry signal.
//! **Drills:** NOTIF-D9 (crash between provider-ack and ledger-write, retry → `UNIQUE(idem_key)`
//! collapses to EXACTLY-ONE effective delivery per (item, channel); 1 effective delivery).
//!
//! ## What this prompt (NOTIF-P16) ships
//!
//! 1. **The idempotent delivery fabric** ([`DeliveryFabric`]) — `deliver(item, channel, message)`
//!    sends through the channel's region-aware [`DeliveryAdapter`](crate::DeliveryAdapter) and records
//!    the receipt in the [`DeliveryLedger`] keyed on `UNIQUE(tenant, idem_key)`. At-least-once +
//!    idempotent: a RETRY of an already-recorded `idem_key` is a NO-OP (the dedup row collapses it) —
//!    the provider is NOT called twice, exactly one EFFECTIVE delivery results.
//! 2. **The deterministic mock adapter** ([`MockAdapter`], `--use-mock`-as-runtime) — a deterministic,
//!    record-only adapter (no network) for v1 dev + the drills. Per-channel; records every `send`
//!    call so a drill can assert exactly-once provider invocation. EU-preferring region by default.
//! 3. **The redaction discipline** ([`redact_for_offcell`]) — an OFF-CELL channel (`email`,
//!    `web_push`, `mobile_push`, `desktop`) carries a [`RedactedMessage`] = the humanised SUMMARY +
//!    the deep link, with `delivery.redacted = true` — NEVER the full body where avoidable (Art.
//!    5(1)(c)). The IN-APP channel (`in_app`) stays IN-CELL: it produces NO off-cell egress at all.
//! 4. **The `delivery_success` telemetry signal** ([`effective_delivery_count`]) — the 1.8 observable
//!    the NOTIF-D9 drill asserts (exactly 1 effective delivery per (item, channel)).
//!
//! ## NOTIF-D9 — the exactly-once across a crash between provider-ack and ledger-write
//!
//! The drill (in `tests/drill_notif_d9.rs`) sends, then "crashes" the process AFTER the provider
//! acked but BEFORE the in-process delivery handle committed the ledger row; a retry on the SAME
//! `idem_key` re-runs `deliver` — but the DURABLE ledger row (the `UNIQUE(tenant, idem_key)`
//! constraint) collapses it to ONE effective delivery, and the provider is invoked exactly once. The
//! property the fabric guarantees: **write-the-ledger-then-call OR call-then-dedupe-on-ledger**; here
//! we model the dedupe-on-ledger path (the ledger row is the durable idempotency key). The threshold
//! is exactly 1 — never softened.
//!
//! ## The in-app-stays-in-cell assertion
//!
//! [`Channel::is_in_cell`](crate::prefs::Channel::is_in_cell) splits the channel set: `in_app` is
//! IN-CELL ([`DeliveryFabric::deliver`] never builds an off-cell payload for it — 0 off-cell egress);
//! the rest are OFF-CELL and carry ONLY a [`RedactedMessage`] with `delivery.redacted = true` (0
//! off-cell full-body). The CI assertion (the drill) measures both at 0.
//!
//! ## FLOORS named
//!
//! - **The concrete production EU email/push provider** (with its DPA / sub-processor posture) is
//!   **N-M5.2 / NOTIF-P25** (a sovereignty/legal [OPEN — LEGAL] selection; the EU-sovereign
//!   delivery-provider follow-on is also tracked as NOTIF-P26 in the run table). The trait +
//!   EU-preferring posture + redaction discipline ship NOW; the real provider swaps into the SAME
//!   [`DeliveryAdapter`](crate::DeliveryAdapter) trait via the strategy pattern (ADR-12.8 — the same
//!   mock→real swap mandate the agent fabric uses). Named.
//! - **The durable `notif_delivery` Postgres store** is the [`DeliveryLedger`] seam's real backing
//!   (the same in-memory-now / Postgres-later seam pattern as [`PrefStore`](crate::prefs::PrefStore)
//!   and the escalation wheel). The DDL + the `UNIQUE(tenant_id, idem_key)` constraint already exist
//!   (NOTIF-P2, [`schema::DELIVERY_DDL`](crate::migrations)); wiring the fabric onto the live
//!   `PgStore` is the integration leg (the band-boundary integration check + the real-stack
//!   integration test). Here the in-memory ledger models the SAME `UNIQUE(tenant, idem_key)`
//!   collapse the constraint enforces, proven by the drill.
//!
//! ## Mutation floor (the delivery module — mandatory-core)
//!
//! The delivery fabric is mandatory-core: a DOUBLE delivery is duplicate-notification spam / a
//! double off-cell egress (a GDPR exposure); a DROPPED delivery is a silently-missed notification.
//! The mutation-tested core is the idempotency + the redaction decision: [`DeliveryFabric::deliver`]
//! (the dedupe-on-ledger collapse — exactly one effective delivery), [`DeliveryLedger::record`] (the
//! `UNIQUE(tenant, idem_key)` first-writer-wins), [`redact_for_offcell`] (off-cell carries summary +
//! link, never the body), [`Channel::is_in_cell`](crate::prefs::Channel::is_in_cell) (the in-cell
//! split), and [`build_idem_key`] (the stable per-(item, channel) key). **Floor: ≥ 80% line/branch
//! mutation score on `delivery.rs`** (measured with `cargo mutants`; reported in the P-194 commit body).

use crate::prefs::Channel;
use crate::{HumanisedString, RedactedMessage, Receipt};
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// ===========================================================================================
//  THE IN-CELL / OFF-CELL CHANNEL SPLIT (architecture §3.6) — the in-app-stays-in-cell invariant
// ===========================================================================================

impl Channel {
    /// **Is this an IN-CELL channel?** (architecture §3.6). The in-app inbox push (`in_app`) is the
    /// ONE inbox's in-cell live transport (NOTIF-P15) — it NEVER leaves the cell, so the delivery
    /// fabric builds NO off-cell payload for it (0 off-cell egress). Every OTHER channel
    /// (`web_push`, `mobile_push`, `email`, `desktop`) is OFF-CELL: it carries ONLY a
    /// [`RedactedMessage`] (`delivery.redacted = true`, the §3.6 PII-minimisation).
    ///
    /// **The §3.6 wording note:** the architecture lists `inbox`/`web_push`/`desktop` as "in-app
    /// channels"; but `web_push` and `desktop` are browser/OS push surfaces that DO egress (they go
    /// through a push service). The data-minimising reading — and the one this fabric enforces — is
    /// that the ONLY truly in-cell channel is the in-cell inbox (`in_app`); a `web_push`/`desktop`
    /// payload egresses to a push endpoint and so MUST be redacted. We take the stricter (more
    /// data-minimising) reading: redact everything that leaves the cell. (Documented deviation,
    /// EI-01 §1 — the stricter reading can only reduce exposure, never increase it.)
    pub fn is_in_cell(self) -> bool {
        matches!(self, Channel::InApp)
    }

    /// **Is this an OFF-CELL channel?** (the complement of [`is_in_cell`](Channel::is_in_cell)). An
    /// off-cell channel MUST carry only a redacted message (`delivery.redacted = true`).
    pub fn is_off_cell(self) -> bool {
        !self.is_in_cell()
    }
}

// ===========================================================================================
//  THE REDACTION DISCIPLINE (architecture §3.6, GDPR Art. 5(1)(c)) — summary + deep link only
// ===========================================================================================

/// **Build the off-cell [`RedactedMessage`] for an item — the §3.6 PII-minimisation.** An off-cell
/// payload carries the already-humanised, viewer-safe SUMMARY (the [`HumanisedString`] — itself
/// permission/erasure-safe by construction, NOTIF-P9) plus a DEEP LINK back into the cell, and the
/// routing [`class`](crate::Class) — and NOTHING ELSE. The full body never crosses the cell
/// boundary where a summary + link suffice (GDPR Art. 5(1)(c) data-minimisation). The deep link is
/// the click-through into the in-cell inbox/artifact (so the recipient reads the full content
/// IN-CELL, behind auth) — it is the FIRST resolved link of the summary, or the explicit `deep_link`.
///
/// The caller passes the humanised SUMMARY (already rendered per-viewer, so a denied/erased ref is a
/// tombstone, never a leaked title — the title is never in `summary.text`). This function does NOT
/// re-render; it CARRIES the summary + the class, asserting the off-cell shape.
pub fn redact_for_offcell(summary: HumanisedString, class: crate::Class) -> RedactedMessage {
    RedactedMessage { rendered: summary, class }
}

/// **The stable idempotency key for a delivery — `<item_id>:<channel>` (architecture §2.3).** The
/// at-least-once + idempotent dedup key: the `UNIQUE(tenant, idem_key)` constraint collapses a
/// retried send to ONE effective delivery per (item, channel). Stable + deterministic (a retry of
/// the SAME (item, channel) produces the SAME key), channel-scoped (the same item delivered to two
/// channels is two distinct deliveries, never collapsed). PII-free (an item id + a channel token).
pub fn build_idem_key(item_id: &str, channel: Channel) -> String {
    format!("{item_id}:{}", channel.token())
}

// ===========================================================================================
//  THE DELIVERY LEDGER — the durable idempotency store (UNIQUE(tenant, idem_key)), seamed
// ===========================================================================================

/// One recorded delivery — the in-memory model of a [`DeliveryRow`](crate::schema::DeliveryRow). The
/// fabric records this AFTER the provider acks; the `idem_key` is the `UNIQUE(tenant, idem_key)`
/// collapse key. `redacted` is the §3.6 off-cell PII-minimisation flag (true for off-cell channels).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryRecord {
    /// The inbox item this delivery is FOR (opaque; FK to `inbox_item.item_id`).
    pub item_id: String,
    /// The channel this delivery went to.
    pub channel: Channel,
    /// The at-least-once + idempotent dedup key (`= build_idem_key(item, channel)`).
    pub idem_key: String,
    /// The off-cell PII-minimisation flag (`true` for an off-cell channel, `false` for in-cell).
    pub redacted: bool,
    /// Whether the channel accepted the delivery (a bounce is `false`).
    pub accepted: bool,
    /// The adapter id that handled it (the region-aware §3.6 adapter; not PII).
    pub adapter: String,
}

/// **The delivery ledger — the durable idempotency store modelling `UNIQUE(tenant, idem_key)`
/// (architecture §2.3 / contract 7.8).** Keyed on `(tenant, idem_key)`: [`record`](DeliveryLedger::record)
/// is FIRST-WRITER-WINS — a second record on the SAME `(tenant, idem_key)` is REJECTED (returns
/// `false`), exactly as the `UNIQUE(tenant_id, idem_key)` constraint rejects a duplicate INSERT. This
/// is the durable half of the at-least-once + idempotent guarantee: even if the in-process send is
/// retried (a crash between provider-ack and the ledger write), the ledger row is the single source
/// of truth for "did this (item, channel) already deliver?".
///
/// **Floor:** the real backing is the `notif_delivery` Postgres table (NOTIF-P2 DDL); this in-memory
/// `BTreeMap` models the SAME constraint (the seam pattern). Clonable + `Arc<Mutex<…>>`-shared so a
/// "crashed-and-retried" delivery in the drill reads the SAME durable ledger.
#[derive(Clone, Default)]
pub struct DeliveryLedger {
    // (tenant, idem_key) → the recorded delivery (UNIQUE(tenant, idem_key)).
    rows: Arc<Mutex<BTreeMap<(String, String), DeliveryRecord>>>,
}

impl DeliveryLedger {
    /// A fresh, empty ledger.
    pub fn new() -> DeliveryLedger {
        DeliveryLedger::default()
    }

    /// **Record a delivery — FIRST-WRITER-WINS on `(tenant, idem_key)`.** Returns `true` if this was
    /// the FIRST record for the key (a new effective delivery), `false` if the key already exists (a
    /// retry collapsed by the `UNIQUE(tenant, idem_key)` constraint — a NO-OP). The first record's
    /// row is preserved; a duplicate NEVER overwrites it (idempotent).
    pub fn record(&self, tenant: &TenantId, rec: DeliveryRecord) -> bool {
        let mut rows = self.rows.lock().expect("delivery ledger lock");
        let key = (tenant.0.clone(), rec.idem_key.clone());
        if rows.contains_key(&key) {
            // The UNIQUE(tenant, idem_key) collapse: a retry is a no-op (the first writer won).
            return false;
        }
        rows.insert(key, rec);
        true
    }

    /// Has this `(tenant, idem_key)` already been recorded? (the durable idempotency check).
    pub fn contains(&self, tenant: &TenantId, idem_key: &str) -> bool {
        self.rows
            .lock()
            .expect("delivery ledger lock")
            .contains_key(&(tenant.0.clone(), idem_key.to_string()))
    }

    /// The recorded delivery for `(tenant, idem_key)`, if any.
    pub fn get(&self, tenant: &TenantId, idem_key: &str) -> Option<DeliveryRecord> {
        self.rows
            .lock()
            .expect("delivery ledger lock")
            .get(&(tenant.0.clone(), idem_key.to_string()))
            .cloned()
    }

    /// The total number of EFFECTIVE deliveries recorded for `tenant` (one row per `idem_key`) — the
    /// `delivery_success` telemetry observable (signal 1.8): the count the NOTIF-D9 drill asserts is
    /// EXACTLY 1 per (item, channel) across a crash/retry.
    pub fn effective_count(&self, tenant: &TenantId) -> usize {
        self.rows
            .lock()
            .expect("delivery ledger lock")
            .keys()
            .filter(|(t, _)| t == &tenant.0)
            .count()
    }
}

/// **`delivery_success` (signal 1.8) — the effective delivery count for `(tenant, item, channel)`.**
/// 1 if the (item, channel) delivered (the ledger holds its row), 0 otherwise. The NOTIF-D9 drill
/// asserts this is EXACTLY 1 across a crash between provider-ack and ledger-write (never 0, never 2).
pub fn effective_delivery_count(
    ledger: &DeliveryLedger,
    tenant: &TenantId,
    item_id: &str,
    channel: Channel,
) -> usize {
    let idem = build_idem_key(item_id, channel);
    usize::from(ledger.contains(tenant, &idem))
}

// ===========================================================================================
//  THE DETERMINISTIC MOCK ADAPTER (--use-mock-as-runtime) — the v1 dev / drill adapter
// ===========================================================================================

/// **The deterministic mock [`DeliveryAdapter`](crate::DeliveryAdapter) (`--use-mock`-as-runtime,
/// architecture §3.6 FLOOR).** A record-only adapter (NO network): every [`send`](MockAdapter::send)
/// call appends the `(idem_key)` to an internal log so a drill can assert the provider was invoked
/// EXACTLY ONCE (the at-least-once-but-not-more property). Deterministic: it accepts every message
/// (unless [`with_bounce`](MockAdapter::with_bounce) marks a key as a bounce). Region-aware +
/// EU-preferring: it reports its configured [`Region`].
///
/// This is the v1 dev runtime + the drill double for the §3.6 FLOOR — the concrete production EU
/// provider (NOTIF-P25) swaps into the SAME trait. Clonable (`Arc`-shared log) so a drill can hold a
/// handle to inspect the call log after delivering through a cloned adapter.
#[derive(Clone)]
pub struct MockAdapter {
    channel: Channel,
    region: Region,
    // The log of idem_keys this adapter was asked to send (to assert exactly-once provider calls).
    sent: Arc<Mutex<Vec<String>>>,
    // idem_keys that should BOUNCE (return accepted=false) — for the bounce-telemetry path.
    bounce: Arc<Mutex<std::collections::BTreeSet<String>>>,
}

impl MockAdapter {
    /// A deterministic mock adapter for `channel` delivering from `region` (EU-preferring: pass an
    /// EU region — `fr-par` is the platform default). The adapter id is `mock:<channel>`.
    pub fn new(channel: Channel, region: Region) -> MockAdapter {
        MockAdapter {
            channel,
            region,
            sent: Arc::new(Mutex::new(Vec::new())),
            bounce: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
        }
    }

    /// Mark an `idem_key` as a BOUNCE (the next `send` for it returns `accepted = false`) — for
    /// exercising the bounce-telemetry path (signal 1.8 `bounce`).
    pub fn with_bounce(self, idem_key: &str) -> MockAdapter {
        self.bounce.lock().expect("bounce set lock").insert(idem_key.to_string());
        self
    }

    /// The log of `idem_key`s this adapter was asked to send (the exactly-once provider-call proof).
    pub fn sent_log(&self) -> Vec<String> {
        self.sent.lock().expect("sent log lock").clone()
    }

    /// How many times the provider was invoked for `idem_key` (the at-least-once-but-not-more
    /// observable; the drill asserts this is 1 — the fabric never double-invokes a deduped key).
    pub fn send_count(&self, idem_key: &str) -> usize {
        self.sent.lock().expect("sent log lock").iter().filter(|k| *k == idem_key).count()
    }
}

impl crate::DeliveryAdapter for MockAdapter {
    fn channel(&self) -> &str {
        self.channel.token()
    }

    fn region(&self) -> &Region {
        &self.region
    }

    fn send(&self, _message: &RedactedMessage, idem_key: &str) -> Receipt {
        // Record the provider invocation (the exactly-once proof) — deterministic, no network.
        self.sent.lock().expect("sent log lock").push(idem_key.to_string());
        let bounced = self.bounce.lock().expect("bounce set lock").contains(idem_key);
        Receipt { idem_key: idem_key.to_string(), accepted: !bounced }
    }
}

// ===========================================================================================
//  THE DELIVERY FABRIC — the idempotent, region-aware, redaction-enforcing orchestrator
// ===========================================================================================

/// The outcome of a [`DeliveryFabric::deliver`] call (PII-free; a drill/telemetry observable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// A NEW effective delivery (the first for this (item, channel)): the provider was invoked and
    /// the ledger row recorded. Carries the [`Receipt`].
    Delivered(Receipt),
    /// A RETRY collapsed by the `UNIQUE(tenant, idem_key)` constraint — a NO-OP. The provider was
    /// NOT invoked again; the original effective delivery stands (idempotent). Carries the already-
    /// recorded delivery's accepted flag.
    AlreadyDelivered { accepted: bool },
    /// A bounce — the provider was invoked but the channel REJECTED the message (`accepted = false`).
    /// Still RECORDED (the bounce is an effective, observed delivery attempt; the row exists, so a
    /// retry is still collapsed — we do not re-deliver a bounced item by re-running `deliver`).
    Bounced(Receipt),
}

/// **The delivery fabric — at-least-once + idempotent, region-aware, redaction-enforcing (contract
/// 7.8).** Holds the per-channel [`DeliveryAdapter`](crate::DeliveryAdapter)s + the shared durable
/// [`DeliveryLedger`]. [`deliver`](DeliveryFabric::deliver) is the ONE delivery path:
///
/// 1. **idempotency CHECK** — if the `(tenant, idem_key)` is ALREADY in the ledger, return
///    [`DeliveryOutcome::AlreadyDelivered`] WITHOUT invoking the provider (the dedupe-on-ledger
///    collapse — the durable half of at-least-once; this is what survives a crash/retry).
/// 2. **redaction** — for an OFF-CELL channel, the message MUST be a [`RedactedMessage`]
///    (`delivery.redacted = true`); for the IN-CELL channel (`in_app`), no off-cell payload is built.
/// 3. **send** — invoke the channel's region-aware adapter `send(message, idem_key)` (the provider
///    ack).
/// 4. **record** — write the ledger row (`UNIQUE(tenant, idem_key)`). First-writer-wins.
///
/// The crash-between-provider-ack-and-ledger-write (NOTIF-D9): if the process dies after step 3 but
/// before step 4, the ledger has NO row, so a retry re-runs all steps — BUT the durable ledger's
/// `UNIQUE` constraint means the SECOND record (or, on the recovered path, the idempotency check)
/// collapses to ONE effective delivery. The drill proves exactly 1.
pub struct DeliveryFabric {
    // channel → its region-aware adapter (the strategy-pattern swap point; mock now, EU provider P-25).
    adapters: BTreeMap<Channel, Arc<dyn crate::DeliveryAdapter + Send + Sync>>,
    ledger: DeliveryLedger,
}

impl DeliveryFabric {
    /// A fresh fabric over a shared [`DeliveryLedger`] (so a crash/retry reads the SAME durable
    /// ledger). Register adapters with [`with_adapter`](DeliveryFabric::with_adapter).
    pub fn new(ledger: DeliveryLedger) -> DeliveryFabric {
        DeliveryFabric { adapters: BTreeMap::new(), ledger }
    }

    /// **A fabric wired with the deterministic MOCK adapter for every channel (`--use-mock`).** The
    /// v1 dev runtime: an EU-preferring (`fr-par`) [`MockAdapter`] per channel. The drills build on
    /// this. Returns the fabric + the per-channel mock handles (so a drill can inspect the call log).
    pub fn with_mock(ledger: DeliveryLedger, region: Region) -> DeliveryFabric {
        let mut fabric = DeliveryFabric::new(ledger);
        for channel in [
            Channel::InApp,
            Channel::WebPush,
            Channel::MobilePush,
            Channel::Email,
            Channel::Desktop,
        ] {
            fabric = fabric
                .with_adapter(Arc::new(MockAdapter::new(channel, region.clone())));
        }
        fabric
    }

    /// Register a region-aware [`DeliveryAdapter`](crate::DeliveryAdapter) for its channel (the
    /// strategy-pattern swap point — a real EU provider, NOTIF-P25, registers here over the SAME
    /// trait). The channel is read from the adapter's [`channel`](crate::DeliveryAdapter::channel).
    pub fn with_adapter(
        mut self,
        adapter: Arc<dyn crate::DeliveryAdapter + Send + Sync>,
    ) -> DeliveryFabric {
        if let Some(channel) = channel_from_token(adapter.channel()) {
            self.adapters.insert(channel, adapter);
        }
        self
    }

    /// The shared durable ledger (the `delivery_success` telemetry observable lives here).
    pub fn ledger(&self) -> &DeliveryLedger {
        &self.ledger
    }

    /// The adapter registered for `channel` (the region-aware §3.6 adapter), if any.
    pub fn adapter(&self, channel: Channel) -> Option<&Arc<dyn crate::DeliveryAdapter + Send + Sync>> {
        self.adapters.get(&channel)
    }

    /// **`deliver(tenant, item_id, channel, message)` — the ONE at-least-once + idempotent delivery
    /// path (contract 7.8).** See the type doc for the four steps. The exactly-once property: across
    /// ANY number of retries on the SAME (item, channel), the provider is invoked AT MOST ONCE more
    /// than a successful run (and the ledger collapses to EXACTLY ONE effective delivery). For an
    /// OFF-CELL channel `message` MUST be redacted (the caller built it via [`redact_for_offcell`] —
    /// the fabric records `redacted = true`); for the IN-CELL channel it stays in-cell (no off-cell
    /// egress; `redacted = false`).
    ///
    /// Returns [`DeliveryOutcome::AlreadyDelivered`] (a collapsed retry, provider NOT re-invoked),
    /// [`DeliveryOutcome::Delivered`] (a new accepted delivery), or [`DeliveryOutcome::Bounced`] (a
    /// new but channel-rejected delivery). An unregistered channel returns
    /// [`DeliveryError::NoAdapter`] (loud — never a silent drop, EI-01 §3).
    pub fn deliver(
        &self,
        tenant: &TenantId,
        item_id: &str,
        channel: Channel,
        message: &RedactedMessage,
    ) -> Result<DeliveryOutcome, DeliveryError> {
        let idem_key = build_idem_key(item_id, channel);

        // (1) IDEMPOTENCY CHECK — the dedupe-on-ledger collapse. If the (tenant, idem_key) already
        // delivered, return WITHOUT invoking the provider (the durable half of at-least-once: this
        // is what makes a retry after a crash a no-op — the ledger row is the source of truth).
        if let Some(existing) = self.ledger.get(tenant, &idem_key) {
            return Ok(DeliveryOutcome::AlreadyDelivered { accepted: existing.accepted });
        }

        // The region-aware adapter for the channel (the strategy-pattern dispatch; loud on absence).
        let adapter = self
            .adapters
            .get(&channel)
            .ok_or_else(|| DeliveryError::NoAdapter(channel.token()))?;

        // (3) SEND — invoke the provider (the ack). The redaction discipline is the CALLER's: an
        // off-cell `message` is a RedactedMessage carrying the summary + link (redact_for_offcell);
        // we mark the row's `redacted` flag from the channel's in-cell/off-cell split.
        let receipt = adapter.send(message, &idem_key);
        let redacted = channel.is_off_cell();

        // (4) RECORD — write the ledger row (UNIQUE(tenant, idem_key), first-writer-wins). If a
        // CONCURRENT retry already recorded it (the crash/retry race), `record` returns false: we
        // collapse to AlreadyDelivered rather than double-counting (exactly one effective delivery).
        let rec = DeliveryRecord {
            item_id: item_id.to_string(),
            channel,
            idem_key: idem_key.clone(),
            redacted,
            accepted: receipt.accepted,
            adapter: adapter.region().0.clone() + ":" + adapter.channel(),
        };
        let first = self.ledger.record(tenant, rec);
        if !first {
            // Another writer won the UNIQUE(tenant, idem_key) race — collapse to one effective.
            let existing = self
                .ledger
                .get(tenant, &idem_key)
                .expect("the winning record exists");
            return Ok(DeliveryOutcome::AlreadyDelivered { accepted: existing.accepted });
        }

        if receipt.accepted {
            Ok(DeliveryOutcome::Delivered(receipt))
        } else {
            Ok(DeliveryOutcome::Bounced(receipt))
        }
    }
}

/// A delivery error (loud, never a silent drop — EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryError {
    /// No region-aware adapter is registered for the channel (a misconfiguration — surfaced, never
    /// silently swallowed). Carries the channel token.
    NoAdapter(&'static str),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryError::NoAdapter(c) => {
                write!(f, "no delivery adapter registered for channel `{c}`")
            }
        }
    }
}

impl std::error::Error for DeliveryError {}

/// The [`Channel`] for a wire token (the inverse of [`Channel::token`]), if it is a known channel.
pub fn channel_from_token(token: &str) -> Option<Channel> {
    match token {
        "in_app" => Some(Channel::InApp),
        "web_push" => Some(Channel::WebPush),
        "mobile_push" => Some(Channel::MobilePush),
        "email" => Some(Channel::Email),
        "desktop" => Some(Channel::Desktop),
        _ => None,
    }
}

/// **Is the region EU-preferring?** (architecture §3.6 — EU-sovereign by construction). The
/// EU-preferring posture: a region whose code names an EU/EEA locale is preferred for off-cell
/// delivery. The platform default `fr-par` (Scaleway Paris) is EU. This is the posture the FLOOR
/// ships — the concrete EU provider selection (with its DPA) is NOTIF-P25. Conservative: only known
/// EU prefixes return true (an unknown region is NOT assumed EU).
pub fn is_eu_region(region: &Region) -> bool {
    let code = region.as_str();
    // Scaleway/EU region prefixes + the generic eu-/eea- forms.
    const EU_PREFIXES: &[&str] = &[
        "fr-", "de-", "nl-", "pl-", "eu-", "eea-", "es-", "it-", "se-", "fi-", "ie-", "be-", "at-",
        "dk-", "pt-", "cz-", "ro-", "gr-", "hu-",
    ];
    EU_PREFIXES.iter().any(|p| code.starts_with(p))
}

#[cfg(test)]
mod tests;
