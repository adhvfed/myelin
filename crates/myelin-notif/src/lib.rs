//! # `myelin-notif` — the Notifications service shell + the §4.1 contract carriers (NOTIF-P1 → P-127, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/notifications.md` §1 (purpose, the C-9
//! resolution — there is exactly ONE inbox; every "my X" surface is a `filter` over it, never a
//! second store), §4.1 (the EXPOSED contract table — STABLE, matching contract-index §7), §5.1
//! (cell-local, tenant-partitioned, bus-driven — the router is a stateless replicable consumer).
//!
//! **Contract-index cluster:** 7 — Notifications (`myelin-notif`), rows 7.1–7.8. This crate
//! carries those rows' **type SHAPES** as compile-time carriers (the glue role) and ships the
//! bootable service shell (the impl role). Consumed/wired here: 1.1 `serve(AppSpec)`, 1.2 the
//! three ports, 1.3 liveness≠readiness, 1.5 forward-only migrations.
//!
//! ## What this prompt (NOTIF-P1) ships — the SHELL + the CARRIERS, nothing else
//!
//! 1. **The contract carriers (the glue role, ADR-01).** Notif owns no new contracts vs Phase 3,
//!    but the §4.1 EXPOSED shapes — [`InboxItem`], [`HumanisedString`], the [`Reason`] +
//!    [`Class`] enums, the [`DeliveryAdapter`] trait — are exported as **compile-time carrier
//!    types** so a contract change breaks every consumer's build NOW, never silently in prod.
//!    NO bodies (the data model is NOTIF-P2; the router NOTIF-P3; the holder NOTIF-P4); just the
//!    frozen signatures every later Notif prompt and every consumer compiles against.
//!
//! 2. **The service shell (the impl role).** [`notif_app_spec`] assembles an [`AppSpec`] the
//!    harness wires (boot → migrate → outbox relay → consumer-seam → three ports → graceful
//!    drain, liveness≠readiness), passed to [`serve`](myelin_substrate::serve) by `main` — NOT a
//!    hand-rolled lifecycle (contract 1.1). The migration set is empty (the nine tables land in
//!    NOTIF-P2); there are no consumers yet (the Signal-consumer router is NOTIF-P3); holders
//!    auto-register (the references-not-payloads holder is NOTIF-P4).
//!
//! ## FLOORS named (this shell is explicitly NOT the working inbox)
//!
//! - **The data model** (the nine tenant-partitioned tables: `inbox_item`, `notif_pref`,
//!   `quiet_hours`, `delivery`, `oncall_schedule`, `escalation_policy`, `escalation_run`,
//!   `humanise_template`, `mute`) → **landed at NOTIF-P2** (P-180), see [`migrations`] + [`schema`].
//!   The migration set is now the nine `(tenant, region)`-first RLS tables (no longer empty).
//! - **The Signal-consumer router** (the EventHandler that consumes curated Signals, UPSERTs
//!   inbox items, and emits `notif.*` via the outbox — the ONLY emit path) → **NOTIF-P3** (P-181).
//!   The `consumers` slot is an empty seam here.
//! - **The `PersonalDataHolder` registration** (references-not-payloads → tombstone-for-free) →
//!   **landed at NOTIF-P4** (P-182), see [`holder`]. Notif is the H13 `NotificationHistory` holder:
//!   `register_notif_holder` opens the OLTP store through the one door (1.4); [`NotifHistoryHolder`]
//!   implements the 7.7 holder half (locate/export/rectify/restrict + the structural
//!   references-not-payloads erase — 0 PII-column mutation on refs-stored items). FLOORS still open:
//!   the reindex/replay half of 7.7 (NOTIF-P17); the off-cell-payload erasure residual (X-7 / 10.9,
//!   NOTIF-P27).
//! - **The contract BODIES** behind every carrier here — `list_inbox` (NOTIF-P5, landed),
//!   `mark/snooze/mark_all_read` (NOTIF-P6, landed — see [`read_state`]), `humanise` (NOTIF-P9),
//!   `define_notif_rule` (NOTIF-P8, landed — see [`define_rule`]: the registration seam + the
//!   stubbed Notif-owned default reason set), `DeliveryAdapter` delivery fabric (NOTIF-P16). The
//!   carriers are SHAPES; the algorithms are the follow-ons.
//!
//! The shell carries NO mandatory-core algorithm module (it is the boot lifecycle + frozen type
//! shapes), so there is no mutation-score floor on this prompt — stated explicitly per the
//! template's TESTS field.

use myelin_events::{DedupLedger, InProcessBus, OutboxStore};
use myelin_refs::ArtifactRef;
use myelin_substrate::{
    boot, serve, AppSpec, Config, ConsumerReg, CriticalDependencies, HotTables, InternalRpc,
    Migrations, OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};

// The Notif data model — the nine tenant-partitioned tables (NOTIF-P2 / P-180). `schema` carries the
// row types + the `#[personal_data(...)]` classification tags (contract 10.2); `migrations` carries
// the nine forward-only `(tenant, region)`-first RLS migrations (contract 1.5). The committed-ratchet
// lint-fixture proof (the three schema gates bite) lives in `tests/lint_fixtures.rs` over RED/GREEN
// fixtures under `tests/fixtures/` (which the lint-gate excludes by the `/fixtures/` convention).
pub mod cli;
pub mod define_rule;
pub mod holder;
// The ONE platform templating surface (NOTIF-P9 / P-187 — contract 7.3): `humanise` + the
// `humanise_template` store + the per-viewer resolve seam + the ONE myelin-content render path +
// the NOTIF-D4 (0 title/PII leak) gate. See [`humanise`].
pub mod humanise;
pub mod list_inbox;
pub mod migrations;
// prefs / quiet-hours over the frozen QueryAst (NOTIF-P10 / P-188 — contract 7.4): get_prefs /
// set_prefs, the per-channel matcher (the frozen `myelin-query` QueryAst — Notif invents no second
// predicate language), quiet-hours in the recipient tz, and `pierce_classes` (critical pierces by
// default — you cannot silence an on-call page). See [`prefs`].
pub mod prefs;
pub mod ranking;
pub mod read_state;
pub mod router;
pub mod schema;
// The five write-time storm-control mechanisms (NOTIF-P11 / P-189 — §3.2): self-suppression,
// dedup-key collapse, thread/subject coalescing, per-(recipient, subject_root) token-bucket rate
// damping, and mute/DND honoring. Storm-control suppresses DELIVERY and RANKING only — NEVER the
// audit/history (Notif is a projection, EI-04 §5.3). The router runs it between classify and UPSERT.
pub mod storm_control;
// Write-fanout for the bounded high-signal set (NOTIF-P12 / P-190 — §3.5/§3.2.4): the router reads
// the frozen `mention(Principal)` STRUCTURED node (contract 13.1) — NEVER free text (AG-6) — and
// materialises one inbox_item per mentioned recipient, bounded by the hot-subject cap so a
// mention-storm cannot write-amplify. The read-fanout for the unbounded ambient set is NOTIF-P13.
pub mod write_fanout;

pub use cli::{
    inbox_list, inbox_read, inbox_show, inbox_snooze, notify_prefs, notify_prefs_set, notify_test,
    render_prefs, CliView, InboxShow,
};
pub use define_rule::{
    define_notif_rule, platform_default_reason, platform_default_rules, Classification, DedupTpl,
    DefineRuleError, NotifRule, NotifRuleRegistry,
};
pub use holder::{
    notif_history_holder, notif_store_classifier, register_notif_holder, NotifBacking,
    NotifHistoryHolder, NotifHolderRegistration, RestrictSet, NOTIF_OLTP_STORE,
};
pub use humanise::{
    humanise, humanise_item, parse_markdown, reason_template_key, render_html, render_markdown,
    render_message, render_plain, shared_platform_templates, tombstone_display, Channel, ContentDoc,
    HumaniseTemplate, RefProjection, RefResolution, RefResolvePort, Span, TemplateStore, Tombstone,
    TombstoneReason, DEFAULT_LOCALE, HUMANISE_RESOLVE_MODE, PLATFORM_DEFAULT_TEMPLATES,
    PLATFORM_DEFAULT_TENANT,
};
pub use list_inbox::{
    list_inbox, list_inbox_ranked, subsystem_of, AllowAllAuthorize, Cursor, InboxFilter, InboxPage,
    Page, RankedPage, ReadAuthorizePort, Subsystem,
};
pub use prefs::{
    build_routing_matcher, get_prefs, route, route_context, set_prefs, Channel as PrefChannel,
    DigestConfig, NotifPrefs, PrefStore, PrefView, QuietHours, QuietWindow, RoutingRule, Tz,
    PREFS_MAX_PREDICATE_DEPTH, PREFS_MAX_PREDICATE_NODES,
};
pub use ranking::{
    band_ceiling, band_floor, base_priority, class_for, rank_and_order, reason_base_class,
    AffinitySource, DeterministicV1, ExplainTrace, NeutralAffinity, RankStrategy, RankedItem,
    PRIORITY_MAX, PRIORITY_MIN,
};
pub use read_state::{
    active_inbox, mark, mark_all_read, snooze, ReadState, ReadStateError,
};
pub use router::{
    build_router, signal_subject_prefix, InboxProjection, RoutedInboxItem, SignalRouter,
    NOTIF_ESCALATION_ACKED, NOTIF_ITEM_CREATED, ROUTER_CONSUMER_NAME, SIGNAL_MENTIONS_KEY,
};
pub use storm_control::{
    dedup_collapse_ratio_bps, is_self_notification, subject_root_of, Coalescer, RateConfig,
    StormContext, StormControl, StormDecision, StormPrefs, SuppressReason, TokenBucket,
};
pub use write_fanout::{
    extract_mentions, CapVerdict, HotSubjectCap, DEFAULT_HOT_SUBJECT_WRITE_CAP,
};

/// The service name (a PII-free label, the telemetry / trace / deployable identifier). The
/// `notif` binary (`src/main.rs`) and the `AppSpec::name` both read this so the deployable
/// matches the trace identifier.
pub const SERVICE_NAME: &str = "notif";

// ===========================================================================================
//  THE §4.1 EXPOSED CONTRACT CARRIERS (the glue role — frozen SHAPES, no bodies yet, ADR-01)
// ===========================================================================================

/// The **inbox item** — the unit of the ONE inbox (contract 7.1 / architecture §2.1).
///
/// **The load-bearing invariant (NOTIF-1, §2.1):** `template_args` holds [`ArtifactRef`]s,
/// **never rendered strings**. The human string is produced at *read* time by
/// [`humanise`](DeliveryAdapter)-time resolution through Refs `resolve(ref, viewer, Display)`,
/// so a renamed PR / a retitled issue / an *erased* author all reflect correctly, and a viewer
/// who lost access sees a tombstone — not a stale title. This is what makes the ONE erasure
/// posture (X-7 / contract 10.9) apply to Notif "for free": the inbox stores refs, not payloads,
/// so erasing a person tombstones their appearance with no mutation (§3.9, C7).
///
/// This is the carrier SHAPE only — the columns/UPSERT/dedup live in the `inbox_item` table
/// (NOTIF-P2); the ranking + the `list_inbox` body are NOTIF-P5/P8.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    /// The stable inbox-item id (PII-free; the `mark/snooze` read-state key, contract 7.2).
    pub item_id: String,
    /// The structured **why it fired** provenance (NOTIF-2) — the basis for scoped-view filters
    /// (the C-9 resolution: a "my X" surface is a `filter` over `reason` + `subject`).
    pub reason: Reason,
    /// The routing/quiet-hours class (critical/direct/participating/watching/fyi) — drives the
    /// channel set and whether the item pierces quiet-hours (`pierce_classes`).
    pub class: Class,
    /// The artifact the item is *about* (the `subject` ref the scoped-view filter pins on). A
    /// ref, never a payload.
    pub subject: ArtifactRef,
    /// The humanise template key (the ONE templating surface, contract 7.3) — resolved per-viewer
    /// at read time. The template store is NOTIF-P9.
    pub template_key: String,
    /// The template arguments — **`ArtifactRef`s, never rendered strings** (NOTIF-1). Each is
    /// resolved per-viewer through Refs `resolve(Display)` at humanise time.
    pub template_args: Vec<ArtifactRef>,
    /// The originating event ref (the NOTIF-2 provenance: "why am I seeing this?").
    pub origin_event: ArtifactRef,
}

/// The **humanised render** of an inbox item or a `(template_key, args)` pair (contract 7.3) —
/// the output shape of the ONE platform templating surface. Permission/erasure-safe: produced by
/// resolving each [`ArtifactRef`] per-viewer via Refs `resolve(Display)` (a denied/erased ref
/// renders as a tombstone, never a stale or leaked title). The render pipeline (ICU
/// MessageFormat) is NOTIF-P9 — this is the carrier SHAPE.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanisedString {
    /// The rendered, viewer-safe text (per-viewer, per-locale; ICU MessageFormat).
    pub text: String,
    /// The resolved links (artifact routes per-viewer; a denied ref drops to a tombstone link).
    pub links: Vec<String>,
    /// The icon key for the item's reason/class (the inbox UI affordance).
    pub icon: String,
}

/// The **why-it-fired** taxonomy (architecture §3.1 / §1.3) — the structured `reason` every
/// inbox item carries and every scoped-view `filter` pins on (the C-9 resolution). Each
/// subsystem registers its set via `define_notif_rule` (contract 7.6, NOTIF-P8); these sixteen
/// are the frozen platform reason vocabulary the §1.3 view table and the §3.1 ranking table read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// An approval is requested of the recipient (the Agent HITL card, AG-8; high priority).
    ApprovalRequested,
    /// An escalation reached the recipient (the on-call/escalation chain, contract 7.5).
    Escalated,
    /// An SLA timer fired (the durable-timer wheel, contract 9.3).
    Sla,
    /// A review is requested of the recipient (Git "Review requests", §1.3).
    ReviewRequested,
    /// The recipient was assigned the subject (Issues "My Work", §1.3).
    Assigned,
    /// The recipient was @-mentioned (the `mention(Principal)` write-fanout node, contract 13.1).
    Mentioned,
    /// Someone replied to the recipient (Chat "Activity", §1.3).
    Replied,
    /// An agent proposed an effect to the recipient (the agent-native inbox, §1.4).
    AgentProposal,
    /// The recipient watches the subject (the read-fanout watcher set, contract 4.3/4.4).
    Watched,
    /// The subject changed state (the ambient "participating" stream).
    StateChanged,
    /// A low-priority for-your-information item (the `fyi` class default).
    Fyi,
    /// The subject became blocked (Issues blocking signal, §1.3).
    Blocked,
    /// The subject became unblocked.
    Unblocked,
    /// The recipient watches a thread (Chat `thread_watched`, §1.3).
    ThreadWatched,
    /// Something was shared with the recipient.
    Shared,
    /// New comments on the subject the recipient participates in.
    Comments,
}

/// The **routing class** (architecture §3.1 / §2.2) — five levels driving the channel set and the
/// quiet-hours pierce decision. `critical`/`escalated` items pierce quiet-hours by default
/// (`pierce_classes`); the rest are gated by prefs ∩ ¬quiet_hours. The frozen five, highest →
/// lowest signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// On-call / escalated / SLA-breach — pierces quiet-hours by default (cannot be silenced).
    Critical,
    /// Directly addressed to the recipient (assigned / mentioned / approval-requested).
    Direct,
    /// The recipient is actively participating (replied / commented).
    Participating,
    /// The recipient watches the subject (ambient).
    Watching,
    /// For-your-information (lowest priority; digestible).
    Fyi,
}

/// A PII-minimised, off-cell-safe message handed to a [`DeliveryAdapter`] (contract 7.8). The
/// body is already redacted (no titles / no PII cross the cell boundary); the carrier is the
/// rendered, viewer-safe [`HumanisedString`] plus the routing metadata. The redaction pipeline
/// is NOTIF-P16 — this is the carrier SHAPE.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedMessage {
    /// The already-redacted, viewer-safe rendered content.
    pub rendered: HumanisedString,
    /// The class (drives the pierce decision at the channel).
    pub class: Class,
}

/// The **delivery adapter** trait SHAPE (contract 7.8) — `{channel, region, send(RedactedMessage,
/// idem_key), receipts}`. Region-aware, EU-preferring, swappable; PII-minimised off-cell;
/// at-least-once **+ idempotent** (the `idem_key` is the at-least-once dedup key). The in-app
/// adapter stays in-cell; email/push/web/mobile/desktop adapters register against this shape.
///
/// This is the carrier TRAIT SHAPE only — the deterministic mock + the real fabric land in
/// NOTIF-P16 (the idempotent `DeliveryAdapter` body). A `Receipt` is the at-least-once delivery
/// outcome the fabric records.
pub trait DeliveryAdapter {
    /// The channel this adapter delivers to (`email`/`push`/`web`/`mobile`/`desktop`/`in_app`).
    fn channel(&self) -> &str;

    /// The region this adapter delivers from (residency-aware, EU-preferring; the off-cell PII
    /// minimisation is enforced before `send`).
    fn region(&self) -> &myelin_tenancy::Region;

    /// Deliver a redacted message at-least-once and idempotently. `idem_key` collapses retries to
    /// one delivery (the at-least-once dedup key). Returns the delivery [`Receipt`].
    ///
    /// **Floor:** the body (the deterministic mock + the real region-aware fabric) is NOTIF-P16;
    /// the trait shape is frozen here so every channel adapter compiles against it now.
    fn send(&self, message: &RedactedMessage, idem_key: &str) -> Receipt;
}

/// The delivery outcome a [`DeliveryAdapter::send`] records (contract 7.8 `receipts`). The carrier
/// SHAPE — the receipt store + the bounce/retry accounting are NOTIF-P16.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    /// The idempotency key this receipt acknowledges (the at-least-once dedup key).
    pub idem_key: String,
    /// Whether the delivery was accepted by the channel (a bounce is `false`).
    pub accepted: bool,
}

// ===========================================================================================
//  THE SERVICE SHELL (the impl role — an AppSpec the harness wires, contract 1.1; NOT a main)
// ===========================================================================================

/// The Notif forward-only migration set (contract 1.5). **The nine tenant-partitioned tables land
/// here at NOTIF-P2 (P-180)** — `notif_inbox_item`, `notif_pref`, `notif_quiet_hours`,
/// `notif_delivery`, `notif_oncall_schedule`, `notif_escalation_policy`, `notif_escalation_run`,
/// `notif_humanise_template`, `notif_mute` — each `(tenant, region)`-first, RLS-scoped,
/// encrypted-from-birth (see [`migrations`]). The boot lifecycle runs migrate → ready over this set.
fn notif_migrations() -> Migrations {
    migrations::migrations()
}

/// Assemble the Notif service [`AppSpec`] (contract 1.1; architecture §5.1) the harness wires.
/// The spec declares Notif's (empty) migration set, the in-process outbox, and holder
/// auto-registration; the harness opens the three ports (public / internal-RPC / metrics-health)
/// around it and starts the outbox relay (the ONLY emit path the router, NOTIF-P3, uses).
///
/// `config` is the validated, env-first config (§3.2). Notif's OLTP store is implicitly critical
/// (the harness adds it). Notif declares its further critical downstreams (Identity `check`, the
/// bus) as those consumer call-sites land (NOTIF-P3+) — at the shell, a healthy boot is ready
/// once migrations apply.
///
/// **Floors wired as empty seams:** the `consumers` slot is empty (the Signal-consumer router is
/// NOTIF-P3); the migration set is empty (the data model is NOTIF-P2); holders auto-register but
/// the references-not-payloads store-holder lands with the data model (NOTIF-P4).
pub fn notif_app_spec(config: Config) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: notif_migrations(),
        hot_tables: HotTables::none(),
        // The public surface (gateway-fronted, tenant-from-token) — the `inbox list|show|read`
        // route bodies are NOTIF-P5/P6. The harness opens the live tenant-from-token surface.
        public: PublicRoutes::default(),
        // The internal-RPC surface — Notif exposes list_inbox/humanise/define_notif_rule to
        // sibling subsystems on this surface; the bodies are the follow-ons.
        internal: InternalRpc::default(),
        // No consumers yet — the Signal-consumer router (the EventHandler) is NOTIF-P3 (P-181).
        consumers: Vec::new(),
        // Every opened store auto-registers as a PersonalDataHolder (§3.4, GD-3) — the Notif OLTP
        // store auto-registers at boot as the H13 NotificationHistory holder (NOTIF-P4 landed; see
        // `holder::register_notif_holder` + `holder::notif_store_classifier`). The references-not-
        // payloads structural erase tombstones an erased person's appearance for free.
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: myelin_substrate::OutboxSpec::default(),
        // Notif declares no further critical downstream at the shell (its consumer call-sites —
        // Identity check, the bus — land with the router, NOTIF-P3); the OLTP store is implicit.
        critical: CriticalDependencies::default(),
    }
}

/// **Assemble the Notif [`AppSpec`] WITH the Signal-consumer router wired into the consumer seam
/// (NOTIF-P3 / P-181).** Unlike the bare [`notif_app_spec`] (whose `consumers` slot is empty at the
/// shell, awaiting the homed-tenant binding), this builds the [`SignalRouter`](router::SignalRouter)
/// — the [`EventHandler`](myelin_events::EventHandler) consumer of curated Signals (`sig.<tenant>.>`)
/// that UPSERTs inbox items and emits `notif.item.created` via the outbox — for each tenant in
/// `tenants`, registers them into the AppSpec `consumers` slot, and supplies the SAME
/// [`OutboxStore`] the routers emit into to the [`OutboxSpec`] the relay drains (so the emit →
/// relay → bus path is the ONE sanctioned outbox path, BUS-2/2.2).
///
/// Returns the spec + the shared [`InboxProjection`] the routers UPSERT into (so a drill / the
/// integration test can read the routed inbox). One [`DedupLedger`] is shared across the tenant
/// routers (the `(consumer, event_id)` PK keeps them isolated; rule 1/4).
///
/// **Floor named:** the LIVE homed-tenant set a cell's router pool binds is the control plane's
/// (`placement_of`, CP-15, M3) — at M2 the caller passes the tenant set explicitly (the drill / the
/// integration / a single-cell self-host). The bare [`notif_app_spec`] keeps the empty seam for the
/// shell-boot property; this is the wired path.
pub fn notif_app_spec_with_router(
    config: Config,
    tenants: &[TenantId],
) -> (AppSpec, router::InboxProjection) {
    let inbox = router::InboxProjection::new();
    // The ONE outbox the routers emit into AND the relay drains (BUS-2): supply it to the
    // OutboxSpec so the emit → relay → bus path is the sanctioned one (no second store).
    let outbox = OutboxStore::new();
    let dedup = DedupLedger::new();
    let mut consumers: Vec<ConsumerReg> = Vec::with_capacity(tenants.len());
    for tenant in tenants {
        // `build_router` binds the `sig.<tenant>.` whitelist through the sanctioned `consume`
        // (rule 3: rejects `*`/empty). A malformed/over-broad tenant is skipped loudly (it never
        // silently narrows to an over-broad subscription) — the shell still boots without it.
        if let Ok(router) = router::build_router(tenant, inbox.clone(), outbox.clone(), dedup.clone())
        {
            consumers.push(ConsumerReg::new(router));
        }
    }
    let mut spec = notif_app_spec(config);
    spec.consumers = consumers;
    // The routers emit into `outbox`; the relay must drain THAT store (not a fresh default one).
    spec.outbox = OutboxSpec::new(outbox, InProcessBus::new());
    (spec, inbox)
}

/// Boot the Notif service shell under the harness (contract 1.1) up to the pre-serve state,
/// returning the [`ServeHandle`] the lifecycle drives. A thin wrapper over
/// [`boot`](myelin_substrate::boot) of [`notif_app_spec`] — separated so a test/drill can boot,
/// inspect the three ports + the liveness≠readiness state, and drive the drain deterministically.
///
/// Returns `Err` (the non-zero exit) on a failed boot (§3.1).
pub fn boot_notif(config: Config) -> Result<ServeHandle, ServeError> {
    boot(notif_app_spec(config))
}

/// Run the Notif service to completion under the harness (boot → migrate → relay → consumers →
/// three ports → graceful drain). The `notif` binary calls this; a failed boot / incomplete
/// drain returns `Err` (the non-zero process exit, §3.1).
pub fn run_notif(config: Config) -> Result<(), ServeError> {
    serve(notif_app_spec(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{Readiness, Startup, Surface};

    /// **The shell boots under the harness and the three ports bind (contracts 1.1/1.2).** The
    /// Notif AppSpec runs the boot → migrate → relay → ports lifecycle; the public / internal /
    /// metrics-health surfaces are all opened (3/3 ports up); no hand-rolled main.
    #[test]
    fn notif_shell_boots_and_three_ports_bind() {
        let handle = boot_notif(Config::default()).expect("the notif shell boots");
        assert_eq!(handle.name(), "notif");
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    /// **Liveness ≠ readiness (contract 1.3): readiness is false *before* migrations apply.** The
    /// metrics-health surface opens in the `Booting` startup state — not-ready (it cannot serve
    /// correct traffic before its schema exists) but not-killed (liveness stays Up). Even with an
    /// EMPTY migration set, the migrate-complete gate is what lifts readiness.
    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface = myelin_substrate::MetricsHealthSurface::new(
            CriticalDependencies::new(["oltp"]),
            myelin_substrate::HealthTable::new(),
        );
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until the migrate-complete gate lifts"
        );
        assert!(r.startup_incomplete, "the not-ready reason names the startup (pre-migrate) gate");
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            myelin_substrate::Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );
        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    /// **A booted instance reports ready once migrations have applied.** The harness flips the
    /// startup gate to Complete at the end of a successful boot — even over the EMPTY migration
    /// set (the data model is NOTIF-P2), so the bootable-shell property is exercised now.
    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_notif(Config::default()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted (empty migration set still completes)"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted notif instance (migrations applied, deps up) is ready"
        );
    }

    /// **The shell ships ZERO consumers but now the NINE-table data model (NOTIF-P2 landed).** The
    /// Signal-consumer router is still the NOTIF-P3 floor (empty `consumers`); the migration set is
    /// no longer empty — it carries the nine `(tenant, region)`-first tables (the data model this
    /// prompt, NOTIF-P2 / P-180, ships). This asserts the shell still has NO writer (the inbox is not
    /// working until NOTIF-P3 UPSERTs items), but the SCHEMA exists.
    #[test]
    fn shell_carries_empty_consumer_seam_and_the_nine_table_data_model() {
        let spec = notif_app_spec(Config::default());
        assert!(spec.consumers.is_empty(), "the Signal-consumer router is the NOTIF-P3 floor");
        assert_eq!(
            spec.migrations.0.len(),
            9,
            "the nine-table data model landed at NOTIF-P2 (P-180): the migration set is non-empty"
        );
        assert_eq!(spec.migrations, migrations::migrations(), "the AppSpec wires the NOTIF-P2 set");
        assert_eq!(spec.name, SERVICE_NAME);
    }

    /// **NOTIF-P3: `notif_app_spec_with_router` REGISTERS the Signal-consumer router into the
    /// AppSpec consumer seam (no longer empty).** For a homed-tenant set, the routers are wired as
    /// `ConsumerReg`s and the SAME outbox they emit into is supplied to the relay (BUS-2). The bare
    /// `notif_app_spec` keeps the empty seam for the shell-boot property; this is the wired path.
    #[test]
    fn notif_app_spec_with_router_registers_the_router_consumer() {
        let tenants = [TenantId("acme".into()), TenantId("globex".into())];
        let (spec, _inbox) = notif_app_spec_with_router(Config::default(), &tenants);
        assert_eq!(
            spec.consumers.len(),
            2,
            "the Signal-consumer router is registered for each homed tenant (the seam is wired)"
        );
        assert_eq!(spec.name, SERVICE_NAME);
        // the wired spec STILL carries the nine-table data model + boots.
        assert_eq!(spec.migrations.0.len(), 9, "the NOTIF-P2 data model is still wired");
        let handle = boot(spec).expect("the router-wired notif spec boots under the harness");
        assert_eq!(
            handle.surfaces(),
            &[
                myelin_substrate::Surface::Public,
                myelin_substrate::Surface::Internal,
                myelin_substrate::Surface::MetricsHealth
            ],
            "the three ports bind around the router-wired spec"
        );
    }

    /// **An over-broad / malformed tenant is skipped loudly (never silently narrowed) — the shell
    /// still boots with the valid routers.** A malformed tenant cannot bind an over-broad `sig.>`
    /// subscription; it is dropped, the valid tenant's router is wired.
    #[test]
    fn notif_app_spec_with_router_skips_overbroad_tenant_but_boots() {
        let tenants = [TenantId("acme".into()), TenantId("".into())]; // the empty tenant is invalid.
        let (spec, _inbox) = notif_app_spec_with_router(Config::default(), &tenants);
        assert_eq!(spec.consumers.len(), 1, "only the valid tenant's router is wired (the empty one is skipped)");
        boot(spec).expect("the shell still boots with the valid router");
    }
}

/// Compile-time CARRIER conformance (ADR-01): the §4.1 EXPOSED shapes match the contract-index
/// §7 signatures, so a wrong shape fails the build NOW (never silently in prod). This is the
/// "the glue crate carries the frozen 7.1–7.8 contract types" proof — a const/type-level assertion
/// that the carrier fields/variants exist with the frozen shape every consumer compiles against.
#[cfg(test)]
mod carrier_conformance {
    use super::*;

    /// 7.1 `InboxItem` carries the refs-not-payloads shape (subject + template_args are
    /// `ArtifactRef`s, NOT strings — the NOTIF-1 invariant the holder, NOTIF-P4, leans on).
    #[test]
    fn inbox_item_stores_refs_not_payloads() {
        let item = InboxItem {
            item_id: "itm-1".into(),
            reason: Reason::Mentioned,
            class: Class::Direct,
            subject: ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            template_key: "issue.mentioned".into(),
            template_args: vec![ArtifactRef("myelin://acme/identity/principal/u1".into())],
            origin_event: ArtifactRef("myelin://acme/bus/event/e1".into()),
        };
        // The shape is refs, never rendered strings (a *compile* check: these fields ARE
        // `ArtifactRef`, so a future drift to `String` breaks this build).
        let _subject: &ArtifactRef = &item.subject;
        let _args: &Vec<ArtifactRef> = &item.template_args;
        assert_eq!(item.subject.0, "myelin://acme/issues/issue/PROJ-1");
    }

    /// 7.3 `HumanisedString{text, links[], icon}` carries the frozen three-field render shape.
    #[test]
    fn humanised_string_has_text_links_icon() {
        let h = HumanisedString {
            text: "you were mentioned".into(),
            links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
            icon: "mention".into(),
        };
        assert_eq!(h.text, "you were mentioned");
        assert_eq!(h.links.len(), 1);
        assert_eq!(h.icon, "mention");
    }

    /// 7.8 `DeliveryAdapter{channel, region, send(RedactedMessage, idem_key) → receipt}` — the
    /// trait shape is satisfiable (a consumer can implement it now, body is NOTIF-P16). The
    /// at-least-once `idem_key` collapses a retry to one accepted delivery.
    #[test]
    fn delivery_adapter_shape_is_implementable() {
        struct InAppAdapter {
            region: myelin_tenancy::Region,
        }
        impl DeliveryAdapter for InAppAdapter {
            fn channel(&self) -> &str {
                "in_app"
            }
            fn region(&self) -> &myelin_tenancy::Region {
                &self.region
            }
            fn send(&self, _message: &RedactedMessage, idem_key: &str) -> Receipt {
                Receipt { idem_key: idem_key.to_string(), accepted: true }
            }
        }
        let adapter = InAppAdapter { region: myelin_tenancy::Region("fr-par".into()) };
        let msg = RedactedMessage {
            rendered: HumanisedString {
                text: "redacted".into(),
                links: vec![],
                icon: "fyi".into(),
            },
            class: Class::Fyi,
        };
        let r = adapter.send(&msg, "idem-1");
        assert_eq!(adapter.channel(), "in_app");
        assert_eq!(adapter.region().0, "fr-par");
        assert!(r.accepted);
        assert_eq!(r.idem_key, "idem-1");
    }

    /// The frozen reason vocabulary (§3.1 / §1.3) has the sixteen platform reasons the C-9 view
    /// filters pin on; the class ordering (§2.2) is critical > … > fyi (the pierce ordering).
    #[test]
    fn reason_and_class_vocabularies_are_frozen() {
        // The C-9 scoped views (§1.3) filter on these reasons — a rename breaks every view.
        let _issues_my_work = [
            Reason::Assigned,
            Reason::Mentioned,
            Reason::ReviewRequested,
            Reason::Sla,
            Reason::Watched,
            Reason::Blocked,
            Reason::ApprovalRequested,
        ];
        let _chat_activity = [
            Reason::Mentioned,
            Reason::Replied,
            Reason::ThreadWatched,
            Reason::ApprovalRequested,
        ];
        // The class pierce-ordering: critical is the highest signal (pierces quiet-hours).
        assert!(Class::Critical < Class::Fyi, "critical outranks fyi (the pierce ordering)");
        assert!(Class::Direct < Class::Watching);
    }
}
