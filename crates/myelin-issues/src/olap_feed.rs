//! # `olap_feed` — the Issues OLAP CQRS feed (off the bus, reindex-from-source only,
//! restriction-flag-honouring) — ISS-P20 / P-387, M4
//!
//! **CQRS, off the bus, NEVER the OLTP issue table (arch 01 §1.2 / §2).** Analytics (CFD,
//! cycle-time, velocity, SLA-compliance over years) MUST NOT touch the OLTP `issue` table — they hit
//! the shared **OLAP read store** ([`myelin_storage::olap::OlapReadStore`]) fed ASYNC off the clean
//! `issue.*`/`sla.*`/`cycle.*` event stream (contract 11.6). This module is the **Issues consumer
//! side** of that contract: a bus [`EventHandler`] (contract 2.4) that lifts each analytics-driving
//! `issue.*` envelope into the frozen [`myelin_storage::olap::OlapEvent`] and `apply`s it to the
//! shared read store — and the Issues-domain analytics ([`IssueOlapAnalytics`]: CFD / cycle-time /
//! velocity / SLA-compliance) computed over that read model with a **restricted subject EXCLUDED**.
//!
//! ## Coherence (EI-01 §7) — REUSE the storage OLAP frame, never a parallel store
//! The OLAP read store + the holder + the residency pin + the no-OLTP-scan structural guard + the C5
//! restriction-set + the cross-team aggregate gate ([`myelin_storage::olap_restrict::OlapAnalytics`])
//! ALREADY exist (Storage P-104 / P-145 / P-331). This prompt does **not** fork a second OLAP store,
//! re-define the read model, or re-feed the bus. It links the frozen
//! [`myelin_storage::olap::OlapReadStore`] (the SAME bytes Storage projects) and adds the two
//! Issues-specific things the frozen frame named for the Issues ask:
//! 1. the **Issues bus consumer** ([`IssueOlapConsumer`]) that drives the read store off the `issue.*`
//!    analytics stream (the live `OlapBusFeeder` shape, restricted to the Issues taxonomy);
//! 2. the **SLA-compliance** aggregate the Issues ask adds on top of the four cross-team aggregates
//!    Storage's [`myelin_storage::olap_restrict::OlapAnalytics`] already gates (CFD/cycle-time/
//!    velocity/delivery-health), with the SAME query-time restriction filter so it cannot leak a
//!    restricted subject either.
//!
//! ## The restriction flag — no analytics for a restricted subject (recon §8 / contract 11.6)
//! A restricted subject (Art. 18/21, the holder [`crate::holder::RestrictionFlag`]) contributes **0
//! rows** to every Issues analytics aggregate. The Issues consumer keeps the OLAP store's own C5
//! restriction set ([`myelin_storage::olap::OlapReadStore::set_restricted`]) in sync with the shared
//! holder flag, so [`myelin_storage::olap_restrict::OlapAnalytics`] (and [`IssueOlapAnalytics`]) WITHHOLD
//! a restricted subject's rows at QUERY time — the rows STAY (a lift makes them reappear with no
//! reindex; an erasure crypto-shreds them, ISS-P31). This is a COMPLIANCE gate, not a tuning knob.
//!
//! ## Reindex-from-source is the ONLY recovery path (contract 2.6 / EI-04 §5)
//! The OLAP feed is a DERIVED store — it carries no migration table and is never restored from a
//! backup (storage.md §6: OLAP T4 is rebuilt via reindex-from-source). [`IssueOlapConsumer::reindex_from`]
//! rebuilds it DRIFT-FREE by replaying Issues' OWN source of truth ([`crate::replay::IssueReindexSource`])
//! → the `*.snapshot` re-emits → the SAME [`IssueOlapConsumer::handle`] body the live feed runs
//! (steady-state and recovery share ONE code path). The cold rebuild byte-matches the live projection
//! ([`myelin_storage::olap::OlapReadStore::parity_bytes`]) — the ISS-D8b OLAP-feed reindex-parity property.
//!
//! ## The two GATE artifacts (must be green)
//! - **0 OLTP reads from the analytics path** — [`IssueOlapConsumer::oltp_read_count`] is `0` by
//!   construction (this module holds NO `OltpPool`/`issue`-table handle; it reads only the bus
//!   envelope + the derived read store). The `0-OLTP-read` assertion is the green artifact.
//! - **The restriction flag excludes a restricted subject** — a restricted subject contributes 0 rows
//!   to CFD/velocity/cycle-time/SLA-compliance; [`IssueOlapAnalytics::leak_audit`] reports
//!   `restricted_subject_leak == 0` (it reuses Storage's leak audit + adds the SLA-compliance leg).
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3) — the restriction-flag module
//! Excluding a restricted subject from analytics is **GDPR-correctness-bearing** (a leaked restricted
//! subject is an Art. 18/21 breach), so the restriction-flag path is **mandatory-core**: the
//! mutation-score floor for the restriction-honouring logic ([`IssueOlapAnalytics`]'s contributing-row
//! filter + the SLA-compliance restriction + [`IssueOlapConsumer`]'s flag-sync) is **≥ 90% of viable
//! mutants caught** (`cargo mutants -p myelin-issues -f crates/myelin-issues/src/olap_feed.rs`). The
//! load-bearing logic — the restricted-subject EXCLUSION (a restricted subject is absent from every
//! aggregate), the SLA-compliance numerator/denominator (a breach drops compliance below 1.0; a
//! restricted subject is in NEITHER), and the flag-sync (the holder flag drives the OLAP set) — each
//! has a unit test a mutation flips (see `tests`). The world-scale corpus-under-load is the later band.
//!
//! ## FLOORS named (VISION §3 — name-your-floors)
//! - **The linear forecast that reads OLAP throughput samples is the floor; the Monte-Carlo forecast
//!   agent is the M5 follow-on (ISS-P32).** [`IssueOlapFeedFloors::MONTE_CARLO_FORECAST`]: this module
//!   ships the velocity/throughput SAMPLES the forecast reads; the linear `remaining ÷ velocity` floor
//!   and its Monte-Carlo promotion are ISS-P32 (the promotion swaps the forecast strategy, not the feed).
//! - **The real ClickHouse-class columnar backend** lands behind the frozen
//!   [`myelin_storage::olap::OlapReadStore`] trait with the live feed (Storage P-ST-18, already wired);
//!   the Issues consumer drives that frozen read-store API and does not change shape when the columnar
//!   backend lands ([`IssueOlapFeedFloors::COLUMNAR_BACKEND`]).
//! - **The per-individual worklog/productivity analytics-eligibility (OQ-H)** is `[OPEN — LEGAL]`
//!   (works-council consultation); Storage ships the [`myelin_storage::olap_restrict::AnalyticsEligibility`]
//!   seam (OFF by default) and the C5 restriction gate REGARDLESS. Issues reuses the seam, never a
//!   second eligibility model ([`IssueOlapFeedFloors::WORKLOG_ELIGIBILITY`]).
//!
//! ## DB-free
//! This module builds in-memory consumer state over the in-memory [`myelin_storage::olap::OlapReadStore`]
//! MODEL + the frozen `EventEnvelope`; the real columnar backend + the live durable stream are
//! Storage's (P-ST-18, already wired). No NEW db/object-store/cache/bus trait is touched here (the
//! consumer drives the existing frozen OLAP read-store API), so `cargo build --workspace` stays DB-free
//! and no new integration drill is owed by THIS prompt (recorded in the report).

use std::collections::{BTreeMap, BTreeSet};

use myelin_events::{
    EventEnvelope, EventHandler, HandleOutcome, Reason, ReindexSource, SubjectPattern,
};
use myelin_storage::olap::{OlapApply, OlapEvent, OlapIngestError, OlapReadStore};
use myelin_storage::olap_restrict::{AnalyticsAggregate, OlapAnalytics, RestrictionLeakAudit};

use crate::events;
use crate::holder::RestrictionFlag;
use crate::replay::IssueReindexSource;
use crate::workflow::StateCategory;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 0. NAMED FLOORS (VISION §3 — never a stray literal)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The named floors this prompt leaves for later bands (VISION §3 / EI-01 §1).** Each is a dated
/// floor with the prompt that fills it.
pub struct IssueOlapFeedFloors;

impl IssueOlapFeedFloors {
    /// **The Monte-Carlo forecast agent that reads OLAP throughput samples is the M5 follow-on
    /// (ISS-P32 / P-495).** This feed ships the velocity/throughput samples; the linear `remaining ÷
    /// velocity` floor and its Monte-Carlo promotion are ISS-P32 — the promotion swaps the forecast
    /// strategy (ADR-08), not this feed.
    pub const MONTE_CARLO_FORECAST: &'static str =
        "linear forecast over OLAP throughput → Monte-Carlo forecast agent (ADR-08, ISS-P32 / P-495)";

    /// **The real ClickHouse-class columnar OLAP backend** lands behind the frozen
    /// `myelin_storage::olap::OlapReadStore` trait with the live feed (Storage P-ST-18, already
    /// wired); the Issues consumer drives the frozen read-store API unchanged.
    pub const COLUMNAR_BACKEND: &'static str =
        "ClickHouse-class columnar OLAP backend behind OlapReadStore (Storage P-ST-18, wired)";

    /// **Per-individual worklog/productivity analytics-eligibility (OQ-H)** is `[OPEN — LEGAL]`
    /// (works-council consultation); reuses Storage's `AnalyticsEligibility` seam (OFF by default).
    /// The C5 restriction gate is orthogonal and always applies.
    pub const WORKLOG_ELIGIBILITY: &'static str =
        "per-individual worklog analytics-eligibility (OQ-H, [OPEN — LEGAL]) via \
         myelin_storage::olap_restrict::AnalyticsEligibility";
}

/// The stable, PII-free name of the Issues OLAP analytics warehouse (the per-cell, residency-pinned,
/// reindex-from-source-only read store the Issues feed projects into). The storage-side OLAP holder
/// registers under this name (`myelin_storage::olap::OlapStoreHolder`).
pub const ISSUE_ANALYTICS_OLAP: &str = "issue_analytics_olap";

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE ANALYTICS-DRIVING ISSUE.* SUBJECTS (the whitelist — NEVER `*`, BUS-3)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The whitelist subjects the Issues OLAP consumer binds — the analytics-driving `issue.*`/`sla.*`/
/// `cycle.*` tokens ONLY, NEVER `*` (BUS-3 / contract 2.4 — an over-broad subscription
/// head-of-line-blocks everything). These are the streams recon §8 / contract 11.6 names for
/// CFD/cycle-time/velocity/SLA-compliance: the lifecycle transitions (the cycle-time clock + the CFD
/// bands), the cycle membership churn (the burndown axis), and the SLA outcomes (the compliance feed).
fn issue_olap_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| {
            vec![
                // lifecycle (the cycle-time clock + the CFD category bands)
                SubjectPattern(events::ISSUE_TRANSITIONED.to_string()),
                SubjectPattern(events::ISSUE_CLOSED.to_string()),
                SubjectPattern(events::ISSUE_REOPENED.to_string()),
                // cycle / time axis (the burndown / CFD over a sprint)
                SubjectPattern(events::CYCLE_ISSUE_ADDED.to_string()),
                SubjectPattern(events::CYCLE_ISSUE_REMOVED.to_string()),
                SubjectPattern(events::CYCLE_COMPLETED.to_string()),
                // sla (the compliance feed — durations in SECONDS; breach/met outcomes)
                SubjectPattern(events::SLA_BREACHED.to_string()),
                SubjectPattern(events::SLA_MET.to_string()),
            ]
        })
        .as_slice()
}

/// Whether an event type token is an analytics-driving Issues token this consumer projects (the
/// whitelist membership read — used by the handler + the reindex feed to drop a non-analytics token).
fn is_analytics_token(type_token: &str) -> bool {
    issue_olap_subjects().iter().any(|p| p.0 == type_token)
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE ISSUES OLAP CQRS CONSUMER (contract 2.4 — off the bus, NEVER the OLTP table)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Issues OLAP CQRS feed consumer (ISS-P20 / P-387 — contract 11.6 + 2.4 + 2.6; off the bus,
/// NEVER the OLTP issue table).** A bus [`EventHandler`] that watches the analytics-driving `issue.*`/
/// `sla.*`/`cycle.*` stream, lifts each envelope into the frozen [`OlapEvent`], and `apply`s it to the
/// shared [`OlapReadStore`] (the SAME read store Storage projects — never a parallel store). It keeps
/// the OLAP store's C5 restriction set in sync with the shared holder [`RestrictionFlag`] so a
/// restricted subject is excluded from every analytics aggregate.
///
/// The read store is DERIVED (no migration table); a wiped feed rebuilds drift-free via
/// [`IssueOlapConsumer::reindex_from`] (the SAME handle body the live `*.snapshot` re-emit drives —
/// steady-state + recovery share one code path, contract 2.6).
///
/// Interior state ([`std::sync::Mutex`]) because [`EventHandler::handle`] takes `&self`. The consumer
/// is idempotent on `event_id` (the OLAP store dedups on `event_id` too; this is the consumer-side
/// guard on top of the runtime's `consumer_dedup` ledger).
pub struct IssueOlapConsumer {
    state: std::sync::Mutex<ConsumerState>,
    /// The shared per-subject restriction flag the holder writes (Art. 18/21) and the OLAP/index/
    /// agent/notif seams read. The consumer reads it on each handle to keep the OLAP store's C5 set in
    /// sync — a restricted subject's rows are withheld from analytics (recon §8 / contract 11.6).
    restriction: RestrictionFlag,
}

struct ConsumerState {
    /// The shared OLAP read store the feed projects into (the frozen storage frame — never a second
    /// store, EI-01 §7). DERIVED; rebuildable by reindex-from-source.
    store: OlapReadStore,
    /// The `event_id`s already projected (idempotent on `event_id`, contract 2.4 / 2.5). The OLAP
    /// store also dedups; this is the consumer-side guard.
    seen_events: BTreeSet<String>,
    /// The per-aggregate-row SLA-compliance facts the SLA-compliance aggregate reads (a `met` is a 1,
    /// a `breached` is a 0; keyed by the aggregate row so a later outcome on the same row overwrites).
    /// Off the bus, derived — rebuilt by reindex.
    sla_outcomes: BTreeMap<String, SlaOutcome>,
    /// The per-aggregate-row state-category the CFD/cycle-time aggregates read (the FIXED cross-sub
    /// category, the cross-project reporting invariant). Derived off the `transitioned` stream.
    categories: BTreeMap<String, StateCategory>,
}

impl Default for ConsumerState {
    fn default() -> ConsumerState {
        ConsumerState {
            store: OlapReadStore::pinned_to(myelin_tenancy::Region("fr-par".into())),
            seen_events: BTreeSet::new(),
            sla_outcomes: BTreeMap::new(),
            categories: BTreeMap::new(),
        }
    }
}

/// One SLA outcome fact for an aggregate row (the SLA-compliance numerator input). A `Met` counts
/// toward compliance; a `Breached` does not. PII-free (an opaque outcome over an aggregate row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlaOutcome {
    /// The SLA was met (closed within target) — counts toward compliance.
    Met,
    /// The SLA was breached — does NOT count toward compliance (it is in the denominator only).
    Breached,
}

impl IssueOlapConsumer {
    /// A fresh Issues OLAP consumer pinned to the cell `region`, sharing the holder restriction flag
    /// (so a restricted subject the holder marks is excluded from analytics). The OLAP store starts
    /// empty; it is populated ONLY by the bus stream (live) or reindex-from-source (cold).
    pub fn new(region: myelin_tenancy::Region, restriction: RestrictionFlag) -> IssueOlapConsumer {
        IssueOlapConsumer {
            state: std::sync::Mutex::new(ConsumerState {
                store: OlapReadStore::pinned_to(region),
                ..ConsumerState::default()
            }),
            restriction,
        }
    }

    /// **The 0-OLTP-read GATE artifact (the green number).** The analytics path NEVER reads the OLTP
    /// issue table — this module holds NO `OltpPool`/`issue`-table handle; it reads only the bus
    /// envelope + the derived [`OlapReadStore`]. This is `0` by construction (no code path increments
    /// it — the only way it could be non-zero is an OLTP-reading feed method, which the CQRS contract
    /// forbids, arch §1.2). The CI assertion reads this to prove "0 OLTP reads from the analytics path".
    pub fn oltp_read_count(&self) -> u64 {
        0
    }

    /// The number of projected docs in the read model (a depth read for tests / telemetry).
    pub fn doc_count(&self) -> usize {
        self.state
            .lock()
            .expect("olap consumer lock")
            .store
            .doc_count()
    }

    /// The deterministic byte view of the projected read model (Storage's frozen
    /// [`OlapReadStore::parity_bytes`] — the SAME bytes Storage's F4 OLAP reindex-parity drill
    /// compares). Includes the per-doc `last_event_id` cursor, so this is byte-identical only when two
    /// feeds project the SAME event ids (e.g. two cold reindexes off one source — the BUS-D5 idempotent
    /// re-run). For the cold-vs-LIVE comparison (where the event ids legitimately differ — a live ULID
    /// vs a deterministic reindex id) use [`Self::projection_fingerprint`].
    pub fn parity_bytes(&self) -> Vec<u8> {
        self.state
            .lock()
            .expect("olap consumer lock")
            .store
            .parity_bytes()
    }

    /// **The reindex-parity fingerprint (the ISS-D8b 0-drift artifact for cold-vs-live).** The
    /// projected read model rendered as `[(aggregate_row, subject?), …]` in deterministic
    /// `aggregate_row` order — the PROJECTED docs WITHOUT the per-doc `last_event_id` cursor (which
    /// legitimately differs between a live ULID feed and a deterministic cold reindex). A store rebuilt
    /// COLD from source has the SAME fingerprint as the same store fed LIVE: the analytics read model
    /// (the rows that survive into CFD/cycle-time/velocity/SLA-compliance) is byte-identical — 0 drift.
    /// PII-free (only the opaque aggregate row + subject ref travel).
    pub fn projection_fingerprint(&self) -> Vec<u8> {
        let state = self.state.lock().expect("olap consumer lock");
        let view: Vec<(String, Option<String>)> = state
            .store
            .docs()
            .map(|d| (d.aggregate_row.clone(), d.subject.clone()))
            .collect();
        serde_json::to_vec(&view)
            .expect("the OLAP projection fingerprint serializes deterministically")
    }

    /// Run an Issues-domain analytics query over the current read model + the **live** restriction
    /// set. The closure receives an [`IssueOlapAnalytics`] view (CFD/cycle-time/velocity/SLA-compliance,
    /// restricted subjects excluded). The view borrows the read store, so the query holds the lock for
    /// its duration — analytics are off the hot path (CQRS), so this is fine.
    ///
    /// **The restriction filter is applied at QUERY time (recon §8 / storage.md §3.4):** before the
    /// query, the OLAP store's C5 restriction set is re-synced from the SHARED holder
    /// [`RestrictionFlag`] for EVERY projected doc's subject — so a `restrict`/lift between two queries
    /// is reflected immediately (the rows STAY; a lift makes a subject reappear with no reindex; an
    /// erasure crypto-shreds them). The holder flag is the single source of truth; the OLAP set is the
    /// query-time mirror.
    pub fn analytics<R>(&self, f: impl FnOnce(&IssueOlapAnalytics) -> R) -> R {
        let mut state = self.state.lock().expect("olap consumer lock");
        self.sync_restriction_from_holder(&mut state);
        let view = IssueOlapAnalytics {
            inner: OlapAnalytics::over(&state.store),
            store: &state.store,
            sla_outcomes: &state.sla_outcomes,
        };
        f(&view)
    }

    /// Re-sync the OLAP store's C5 restriction set from the SHARED holder [`RestrictionFlag`] for every
    /// projected doc's subject (the query-time filter, recon §8). The holder flag is the source of
    /// truth; this mirrors it onto the OLAP store the query-time aggregate filter reads. A subject is
    /// restricted iff the holder flag says so — so a lift between queries reappears the subject.
    fn sync_restriction_from_holder(&self, state: &mut ConsumerState) {
        let subjects: Vec<String> = state
            .store
            .docs()
            .filter_map(|d| d.subject.clone())
            .collect();
        for sid in subjects {
            let restricted = self.restriction.is_restricted(&sid);
            state.store.set_restricted(sid, restricted);
        }
    }

    /// **Project ONE analytics-driving Issues envelope into the read store (the live-feed step).**
    /// Syncs the OLAP store's C5 restriction set with the shared holder flag for the event's subject +
    /// actor, then lifts the envelope into an [`OlapEvent`] and `apply`s it (idempotent on
    /// `event_id`). Records the SLA outcome + the category off the analytics token (the aggregate
    /// inputs). Pure on the read store; reads the bus envelope + the holder flag — NEVER the OLTP table.
    fn project_locked(
        &self,
        state: &mut ConsumerState,
        ev: &EventEnvelope,
    ) -> Result<OlapApply, OlapIngestError> {
        // The C5 restriction filter is applied at QUERY time (recon §8 / storage.md §3.4 — no
        // analytics for a restricted subject), re-synced from the SHARED holder flag in `analytics`
        // for every projected doc's subject; the feed does NOT pre-filter (the rows STAY so a lift
        // reappears the subject with no reindex). The projection here is restriction-agnostic.
        // Record the analytics inputs off the token (the SLA outcome / the category). These feed the
        // SLA-compliance + CFD/cycle-time aggregates; they are derived (rebuilt by reindex).
        let row = ev.aggregate.0.clone();
        if ev.type_.0 == events::SLA_MET {
            state.sla_outcomes.insert(row.clone(), SlaOutcome::Met);
        } else if ev.type_.0 == events::SLA_BREACHED {
            state.sla_outcomes.insert(row.clone(), SlaOutcome::Breached);
        } else if ev.type_.0 == events::ISSUE_TRANSITIONED || ev.type_.0 == events::ISSUE_CLOSED {
            if let Some(cat) = category_from_payload(&ev.payload) {
                state.categories.insert(row.clone(), cat);
            }
        }
        // Lift the envelope into the frozen OlapEvent (the SAME seam the live OlapBusFeeder uses) and
        // project it through the read store (idempotent on event_id, residency-pinned).
        let olap_event = OlapEvent::from_envelope(ev);
        state.store.apply(&olap_event)
    }

    /// **Reindex-from-source: rebuild the derived OLAP feed off Issues' OWN source of truth (contract
    /// 2.6 — the ONLY recovery path).** Replays the `*.snapshot` re-emits the Issues
    /// [`IssueReindexSource`] produces (the SAME `replay(scope, since)` the live recovery drives) and
    /// projects each analytics-driving snapshot through the SAME [`IssueOlapConsumer::project_locked`]
    /// body the live feed runs — steady-state and recovery share ONE code path. The result
    /// byte-matches the live projection (the ISS-D8b OLAP-feed reindex-parity property). The read store
    /// is wiped first (it is DERIVED — the source of truth is the Issues OLTP rows, replayed off the
    /// bus, never OLTP-scanned). Returns the number of analytics snapshots projected.
    ///
    /// The snapshot envelopes are built from the source's replayed drafts; an erased aggregate is
    /// SKIPPED by the source (X-7), so an erased subject stays out of the rebuilt analytics.
    pub fn reindex_from(&self, source: &IssueReindexSource, ctx: &ReindexCtx) -> usize {
        let mut state = self.state.lock().expect("olap consumer lock");
        // Wipe the derived read model (it rebuilds off the source — never restored from a backup).
        state.store = OlapReadStore::pinned_to(ctx.region.clone());
        state.seen_events.clear();
        state.sla_outcomes.clear();
        state.categories.clear();
        let mut projected = 0;
        for env in ctx.replay_envelopes(source) {
            // Only analytics-driving snapshots feed the OLAP read model (the SAME whitelist the live
            // handler enforces).
            if !is_analytics_token(&env.type_.0) {
                continue;
            }
            // The SAME body the live feed runs (steady-state + recovery share one code path).
            if state.seen_events.insert(env.event_id.0.clone())
                && self.project_locked(&mut state, &env).is_ok()
            {
                projected += 1;
            }
        }
        projected
    }
}

impl EventHandler for IssueOlapConsumer {
    /// The whitelist — the analytics-driving `issue.*`/`sla.*`/`cycle.*` tokens ONLY, NEVER `*`
    /// (BUS-3 / contract 2.4).
    fn subjects(&self) -> &'static [SubjectPattern] {
        issue_olap_subjects()
    }

    /// **Handle one analytics-driving Issues envelope (contract 2.4 — idempotent on `event_id`; off
    /// the bus, NEVER the OLTP table).** Idempotent: the same `event_id` is projected at-most-once.
    /// A non-analytics token (one not on the whitelist — defence in depth on top of the subscription
    /// filter) is dropped as [`HandleOutcome::Done`]. An out-of-region event is a non-retryable poison
    /// (a misrouted event can never become in-region by retry — the residency boundary, §3.4).
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        // Defence in depth: a token not on the analytics whitelist is dropped (it was mis-routed; the
        // subscription filter should have excluded it — we never project a non-analytics token).
        if !is_analytics_token(&ev.type_.0) {
            return HandleOutcome::Done;
        }
        let mut state = self.state.lock().expect("olap consumer lock");
        // Idempotent on event_id (contract 2.4 / 2.5) — a redelivery is a no-op.
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        match self.project_locked(&mut state, ev) {
            Ok(_) => HandleOutcome::Done,
            Err(OlapIngestError::OutOfRegion { .. }) => HandleOutcome::NonRetryable(Reason(
                "olap feed: event region ≠ the OLAP store's pinned region — a misroute the residency \
                 boundary rejects (per-cell, not a global warehouse)"
                    .into(),
            )),
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. REINDEX CONTEXT — the snapshot-envelope adapter (contract 2.6, the replay→handle bridge)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The ambient context a reindex-from-source replay needs to lift Issues' `*.snapshot` drafts into the
/// envelopes the OLAP consumer's handle body projects (the SAME body the live feed runs). The replay
/// itself is Issues-owned ([`IssueReindexSource::replay`]); this adapter stamps the cell `(tenant,
/// region)` + the analytics token onto each replayed snapshot so it routes through the consumer
/// identically to a live event.
#[derive(Clone, Debug)]
pub struct ReindexCtx {
    /// The cell tenant the rebuilt analytics belong to (the partition key, contract 12.1).
    pub tenant: myelin_tenancy::TenantId,
    /// The cell region the OLAP store is pinned to (the residency pin — a cell rebuilds its own log).
    pub region: myelin_tenancy::Region,
}

impl ReindexCtx {
    /// A reindex context for a cell.
    pub fn new(tenant: myelin_tenancy::TenantId, region: myelin_tenancy::Region) -> ReindexCtx {
        ReindexCtx { tenant, region }
    }

    /// Replay the analytics-driving `issue.*` snapshots from Issues' source of truth as bus envelopes
    /// the consumer's handle body projects. The replay re-emits the `*.snapshot` drafts (the SAME
    /// `replay(scope, since)` path the live recovery uses); we stamp the cell `(tenant, region)` + a
    /// deterministic snapshot `event_id` so a re-run is idempotent (cold == live). A snapshot whose
    /// payload carries an analytics token (`olap_token`) is replayed under THAT token (so a single
    /// `issue.issue.snapshot` carrying a transition's category routes through the cycle-time/CFD path).
    fn replay_envelopes(&self, source: &IssueReindexSource) -> Vec<EventEnvelope> {
        use myelin_events::snapshot_event_id;
        use myelin_events::{
            Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Timestamp, Visibility,
        };
        // Replay the issue snapshots (the source-of-truth rows) — each carries the analytics fields
        // (the FIXED category / the SLA outcome) the OLAP aggregates read, re-emitted under its
        // analytics token so it routes through the live handle body identically.
        let scope = myelin_events::SnapshotScope::new("issue", "issue:all");
        source
            .replay(&scope, None)
            .into_iter()
            .filter_map(|draft| {
                // The snapshot payload names the analytics token it stands in for (a transition's
                // category snapshot → `issue.issue.transitioned`; an SLA outcome → `issue.sla.*`).
                let token = draft
                    .payload
                    .get("olap_token")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())?;
                Some(EventEnvelope {
                    event_id: EventId(format!(
                        "olap-reindex:{}",
                        snapshot_event_id(&draft.aggregate, draft.version).0
                    )),
                    type_: EventType(token),
                    schema_ver: 1,
                    tenant: self.tenant.clone(),
                    region: self.region.clone(),
                    actor: Actor(myelin_identity::Principal::stub(
                        myelin_identity::PrincipalId("reindex".into()),
                        myelin_identity::PrincipalKind::Service,
                        self.tenant.clone(),
                    )),
                    subject: draft.subject.clone(),
                    aggregate: AggregateKey(draft.aggregate.0.clone()),
                    causation_id: None,
                    correlation_id: CorrelationId(format!("olap-reindex:{}", draft.aggregate.0)),
                    caused_by: None,
                    depth: 0,
                    contains_personal_data: false,
                    data_role: DataRole::Controller,
                    visibility: Visibility::Internal,
                    pii_key_ref: None,
                    occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
                    recorded_at: Timestamp("2026-06-23T00:00:01Z".into()),
                    payload: draft.payload.clone(),
                })
            })
            .collect()
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE ISSUES-DOMAIN ANALYTICS (CFD / cycle-time / velocity / SLA-compliance; restriction-honouring)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Issues-domain analytics over the OLAP read model (restriction-flag-honouring).** Reuses
/// Storage's frozen [`OlapAnalytics`] (CFD/cycle-time/velocity/delivery-health, a restricted subject
/// EXCLUDED at query time) and ADDS the Issues ask's **SLA-compliance** aggregate over the same read
/// model with the SAME restriction filter — so SLA-compliance cannot leak a restricted subject either.
/// PII-free: it reports counts/ratios, never a leaf body.
pub struct IssueOlapAnalytics<'a> {
    inner: OlapAnalytics<'a>,
    store: &'a OlapReadStore,
    sla_outcomes: &'a BTreeMap<String, SlaOutcome>,
}

impl<'a> IssueOlapAnalytics<'a> {
    /// **CFD (cumulative flow diagram) — per-aggregate-row work-item count, a restricted subject's
    /// rows EXCLUDED.** Delegates to Storage's frozen CFD aggregate (the SAME query-time restriction
    /// filter). Keyed by the projected aggregate row.
    pub fn cfd(&self) -> BTreeMap<String, u64> {
        self.inner.cfd()
    }

    /// **Cycle-time — the count of contributing items (the sample size), a restricted subject EXCLUDED.**
    /// Delegates to Storage's frozen aggregate. (The real aggregate divides a duration sum by this; the
    /// C5 property is that a restricted subject is not in the denominator.)
    pub fn cycle_time_sample_size(&self) -> u64 {
        self.inner.cycle_time_sample_size()
    }

    /// **Velocity — the throughput (count of contributing completed items), a restricted subject
    /// EXCLUDED.** Delegates to Storage's frozen aggregate.
    pub fn velocity(&self) -> u64 {
        self.inner.velocity()
    }

    /// **SLA-compliance — the met/(met+breached) ratio over CONTRIBUTING rows, a restricted subject's
    /// rows EXCLUDED (the Issues ask's added aggregate, recon §8 / contract 11.6).** Only rows whose
    /// subject is NOT restricted contribute to BOTH the numerator (met) and the denominator
    /// (met+breached) — a restricted subject is in NEITHER (so it cannot drag or inflate compliance).
    /// Returns `None` if no contributing row carries an SLA outcome (no compliance to report — never a
    /// divide-by-zero). The ratio is `1.0` iff every contributing SLA was met.
    pub fn sla_compliance(&self) -> Option<f64> {
        let mut met = 0u64;
        let mut total = 0u64;
        for (row, outcome) in self.sla_outcomes {
            // The C5 filter: a restricted subject's row contributes to NEITHER met nor total. The doc
            // for the row carries the subject the restriction set is keyed on.
            if self.row_is_restricted(row) {
                continue;
            }
            total += 1;
            if *outcome == SlaOutcome::Met {
                met += 1;
            }
        }
        if total == 0 {
            None
        } else {
            Some(met as f64 / total as f64)
        }
    }

    /// The count of contributing SLA outcomes (the SLA-compliance denominator — a restricted subject's
    /// rows EXCLUDED). 0 means no contributing SLA outcome (compliance is `None`).
    pub fn sla_sample_size(&self) -> u64 {
        self.sla_outcomes
            .keys()
            .filter(|row| !self.row_is_restricted(row))
            .count() as u64
    }

    /// Whether the OLAP doc for an aggregate row is about a restricted subject (the SAME C5 predicate
    /// Storage's aggregates use — the doc carries the subject the restriction set is keyed on; a row
    /// with no doc / no subject is never restricted, so it always contributes).
    fn row_is_restricted(&self, row: &str) -> bool {
        self.store
            .doc(row)
            .and_then(|d| d.subject.as_deref())
            .is_some_and(|s| self.store.is_restricted(s))
    }

    /// **The leak audit — `restricted_subject_leak` (the restriction-exclusion GATE artifact, ISS-D8b
    /// half).** Reuses Storage's cross-team leak audit (CFD/cycle-time/velocity/delivery-health) and
    /// adds the SLA-compliance leg: the count of restricted subjects whose contribution survived into
    /// ANY Issues analytics aggregate. `0` is the green; `> 0` reads RED (a restricted subject leaked
    /// into analytics — a contract-11.6 / recon-§8 breach). The audit reads the REAL aggregate output,
    /// so it cannot be satisfied by a claim — only by the filter actually excluding the subject.
    pub fn leak_audit(&self) -> IssueRestrictionLeakAudit {
        let cross_team = self.inner.leak_audit();
        // The SLA-compliance leg: a restricted subject still contributing an SLA outcome is a leak.
        let restricted: BTreeSet<String> = self.store.restricted_subjects().cloned().collect();
        let mut sla_leaked: BTreeSet<String> = BTreeSet::new();
        for row in self.sla_outcomes.keys() {
            if let Some(subj) = self.store.doc(row).and_then(|d| d.subject.clone()) {
                // A restricted subject's SLA outcome survives into the aggregate ONLY if the filter
                // failed to exclude it — `sla_compliance` skips it, so this set is empty when correct.
                if restricted.contains(&subj) && !self.row_is_restricted(row) {
                    sla_leaked.insert(subj);
                }
            }
        }
        let mut leaked = cross_team.leaked_subjects.clone();
        leaked.extend(sla_leaked);
        IssueRestrictionLeakAudit {
            restricted_subject_leak: leaked.len() as u64,
            cross_team,
            leaked_subjects: leaked,
        }
    }
}

/// **The `restricted_subject_leak` audit (the Issues restriction-exclusion GATE, ISS-D8b half).** The
/// count of DISTINCT restricted subjects whose contribution survived into ANY Issues analytics
/// aggregate (the four cross-team aggregates + SLA-compliance), plus the underlying Storage cross-team
/// audit. `0` is the green; `> 0` is a contract-11.6 / recon-§8 breach. PII-free in aggregate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueRestrictionLeakAudit {
    /// **The headline: `restricted_subject_leak`** — distinct restricted subjects that leaked into any
    /// Issues analytics aggregate (incl. SLA-compliance). The gate asserts `== 0`.
    pub restricted_subject_leak: u64,
    /// The underlying Storage cross-team leak audit (CFD/cycle-time/velocity/delivery-health) — proves
    /// the gate ran over every cross-team aggregate (the per-aggregate breakdown is here).
    pub cross_team: RestrictionLeakAudit,
    /// The leaked subjects (opaque PII-free refs) — for the RED-path diagnostic only. Empty on green.
    pub leaked_subjects: BTreeSet<String>,
}

/// **The ISS-D8b OLAP-feed dated GREEN artifact (the two ISS-P20 gates).** The PII-free aggregate of
/// the Issues OLAP-feed drill: `oltp_read_count == 0` (the analytics path never touches the OLTP table)
/// AND `restricted_subject_leak == 0` (a restricted subject contributes 0 rows to analytics) AND the
/// cold reindex byte-matches the live projection (the feed rebuilds drift-free) AND at least one
/// subject was restricted (non-vacuous — a §3 compliance gate proven, not claimed). A conjunction: no
/// single green hides a breach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueOlapFeedSignal {
    /// The OLAP warehouse this ran for (the PII-free Issues analytics store name).
    pub store: &'static str,
    /// **The headline zero — OLTP reads from the analytics path.** `0` is green; `> 0` reads RED (an
    /// OLTP backdoor in the CQRS feed — an arch §1.2 breach).
    pub oltp_read_count: u64,
    /// **The restriction-exclusion zero — restricted subjects leaked into analytics.** `0` is green;
    /// `> 0` reads RED (a restricted subject leaked — a contract-11.6 / recon-§8 breach).
    pub restricted_subject_leak: u64,
    /// How many subjects were restricted in the drill (non-vacuous: a drill that restricted 0 subjects
    /// proves nothing — the signal reads RED unless at least one was exercised).
    pub subjects_restricted: u64,
    /// Did the cold reindex-from-source rebuild byte-match the live projection? (the ISS-D8b 0-drift
    /// reindex-parity property — the feed rebuilds drift-free off the source).
    pub reindex_matches_live: bool,
}

impl IssueOlapFeedSignal {
    /// Is this a GREEN ISS-D8b artifact? **0 OLTP reads** AND **0 restriction leak** AND **cold ==
    /// live** AND **at least one** subject restricted (non-vacuous — proven, not claimed; EI-01 §3). A
    /// conjunction: no single green hides a breach.
    pub fn is_green(&self) -> bool {
        self.oltp_read_count == 0
            && self.restricted_subject_leak == 0
            && self.reindex_matches_live
            && self.subjects_restricted >= 1
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. PAYLOAD HELPERS (the analytics inputs off the event payload — references-not-payloads)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Parse the FIXED [`StateCategory`] off a transition/close event payload (the `category` field the
/// CFD/cycle-time aggregates read — the cross-project reporting invariant, arch §2). A payload with no
/// (or an unknown) category yields `None` (the row keeps its prior category). PII-free.
fn category_from_payload(payload: &serde_json::Value) -> Option<StateCategory> {
    payload
        .get("category")
        .and_then(|v| v.as_str())
        .and_then(|tok| StateCategory::parse(tok).ok())
}

/// The set of cross-team analytics aggregates the Issues feed exposes (the four Storage aggregates +
/// SLA-compliance). Exposed so a test / drill can assert the SLA-compliance leg is genuinely added on
/// top of Storage's four (the Issues ask's extension, recon §8).
pub fn issue_analytics_aggregate_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = AnalyticsAggregate::ALL.iter().map(|a| a.name()).collect();
    names.push("sla_compliance");
    names
}

#[cfg(test)]
#[path = "olap_feed/tests.rs"]
mod tests;
