//! # `projection_feeder` — the MEASURED generated-index promotion (ISS-P15 / P-381).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §3 (*The query planner — AST→store compiler + `SetExpr` push-down + cost-bounding + the projection
//! feeder*): "**The projection feeder** (the measured-promotion path): a bus consumer watches
//! `issue.updated` deltas and a per-`(tenant, type, field_id)` **filter/sort frequency counter**; when
//! a custom facet crosses the **measured** threshold (contract 6.3, OQ-C — the default-to-beat is a
//! facet appearing in `> 5%` of a collection's view executions over a rolling window; a Search-owned
//! tunable, not a contract constant), the feeder provisions a generated/expression index via a
//! forward-only online migration (expand→backfill→contract; no blocking `ALTER` on the flagged-hot
//! `issue` table). Promotion is **measured, never predicted** (EI-02 §8)."
//!
//! Reconciliation: `00-reconciliation-decisions.md` OQ-C (the `> 5%` measured default-to-beat — a
//! Search-owned tunable, not a contract constant). VISION §3 / EI-01 §7 (name-your-floors; promote a
//! floor ONLY on a measured trigger, never premature).
//!
//! ## What ISS-P15 ships here (the measured-promotion consumer ON TOP of ISS-P14's catalog)
//!
//! The cost-bounder ([`crate::cost_bounder`], ISS-P14) reads a [`crate::cost_bounder::FacetCatalog`]
//! to decide Tier 2 (a promoted generated index) vs Tier 2b (the default GIN probe). This module is
//! the bus CONSUMER that FEEDS that catalog — the only thing that may move a facet from GIN to a
//! generated index, and ONLY on a measured trigger:
//!
//! 1. [`FrequencyCounter`] — the per-`(tenant, type, field_id)` filter/sort frequency counter over a
//!    rolling window of a collection's VIEW EXECUTIONS (the denominator) and the per-facet appearances
//!    in those executions (the numerator). [`FrequencyCounter::share`] is the measured facet share
//!    (`appearances / executions`); a facet that never appears in a view has share 0.
//! 2. [`PromotionThreshold`] — the OQ-C measured threshold (`> 5%` of a collection's view executions —
//!    [`PromotionThreshold::OQ_C_DEFAULT_TO_BEAT`]). The gate is `share > threshold` (strict — exactly
//!    at the threshold is NOT promoted; the OQ-C wording is `> 5%`).
//! 3. [`IndexProvisioning`] — the forward-only ONLINE migration the feeder runs to provision the
//!    generated/expression index: a `CREATE INDEX CONCURRENTLY` over the `props` JSONB tail's hot facet
//!    expression, asserted NON-BLOCKING on the declared-hot `issue` table (the expand→backfill→contract
//!    discipline; contract 1.5). 0 downtime by construction (CONCURRENTLY takes no exclusive lock).
//! 4. [`ProjectionFeeder`] — the consumer proper ([`myelin_events::EventHandler`], contract 2.4): its
//!    `subjects()` whitelist is `issue.issue.updated` (NEVER `*`); `handle` records each updated
//!    facet's appearance, then PROMOTES iff the facet crosses the threshold — idempotent on
//!    `event_id` AND idempotent on promotion (a facet already promoted is not re-provisioned).
//!
//! ## Promotion is MEASURED, never predicted (EI-02 §8; the gate)
//! The feeder NEVER provisions an index speculatively. A facet stays on the GIN index (Tier 2b) until
//! the rolling-window share STRICTLY exceeds the OQ-C threshold; only then does it provision the
//! generated index (Tier 2). [`ProjectionFeeder::should_promote`] is the gate; a below-threshold facet
//! is structurally NOT promoted. The promotion is the green artifact, calibrated under ISS-D2.
//!
//! ## FLOOR named (per the prompt — DELIVERABLE)
//! - **The promotion threshold is the OQ-C default-to-beat** (`> 5%` of a collection's view executions
//!   over a rolling window), a **Search-owned tunable, NOT a contract constant** — calibrated by the
//!   ISS-D2 `<1s` drill. Named: [`PromotionThreshold::OQ_C_DEFAULT_TO_BEAT`] /
//!   [`ProjectionFeederFloors::OQ_C_THRESHOLD`]. The rolling-window WIDTH + the demotion-on-cooling
//!   half (a facet that goes cold again) is the at-scale calibration follow-on (ISS-P32 / M5 — the
//!   surge family), named: [`ProjectionFeederFloors::WINDOW_CALIBRATION`].
//!
//! The live wall-clock proof that the promoted index moves the facet under the `<1s` budget is the
//! ISS-D2 `--features integration` drill against the dev-stack Postgres
//! (`tests/integration_iss_p14_cost_bounding.rs`, ISS-P14) — this module's CI gate is the
//! threshold-gated promotion + the 0-downtime online-migration signal.

use myelin_events::{EventEnvelope, EventHandler, HandleOutcome, Reason, SubjectPattern};
use myelin_storage::migration::is_blocking_alter;
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::cost_bounder::FacetCatalog;
use crate::events::ISSUE_UPDATED;

// ───────────────────────────── the per-(tenant, type, field_id) key (§3) ─────────────────────────

/// **The frequency-counter key (§3): per-`(tenant, type, field_id)`.** A facet's hotness is measured
/// WITHIN a collection (a `(tenant, type)` pair — e.g. `acme`'s `bug` issues), so the same custom
/// field can be hot for one type and cold for another. The promotion is per-collection; the generated
/// index it provisions is scoped to the collection's rows. Ordered so the counter is deterministic.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FacetKey {
    /// The tenant the facet belongs to (residency + isolation — never cross-tenant).
    pub tenant: String,
    /// The issue `type` the collection is scoped to (e.g. `bug`/`story`/`epic`).
    pub type_: String,
    /// The opaque custom `field_id` (the `props` JSONB key the facet filters/sorts on).
    pub field_id: String,
}

impl FacetKey {
    /// Build a facet key from its three parts.
    pub fn new(
        tenant: impl Into<String>,
        type_: impl Into<String>,
        field_id: impl Into<String>,
    ) -> FacetKey {
        FacetKey {
            tenant: tenant.into(),
            type_: type_.into(),
            field_id: field_id.into(),
        }
    }

    /// The collection key (`(tenant, type)`) this facet belongs to — the denominator scope for the
    /// share computation (a facet's share is over ITS collection's view executions, not globally).
    pub fn collection(&self) -> CollectionKey {
        CollectionKey {
            tenant: self.tenant.clone(),
            type_: self.type_.clone(),
        }
    }
}

/// **A collection — a `(tenant, type)` pair (§3).** The denominator of the measured share: a facet's
/// hotness is `appearances-in-this-collection's-views / this-collection's-view-executions`. View
/// executions are counted per collection so a facet on a rarely-viewed type is not spuriously promoted.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CollectionKey {
    /// The tenant.
    pub tenant: String,
    /// The issue `type` (the collection).
    pub type_: String,
}

impl CollectionKey {
    /// Build a collection key.
    pub fn new(tenant: impl Into<String>, type_: impl Into<String>) -> CollectionKey {
        CollectionKey {
            tenant: tenant.into(),
            type_: type_.into(),
        }
    }
}

// ───────────────────────────── the OQ-C measured threshold ───────────────────────────────────────

/// **The OQ-C measured promotion threshold (recon OQ-C).** A facet is promoted iff its rolling-window
/// share STRICTLY exceeds this fraction of a collection's view executions. The default-to-beat is
/// `> 5%` (a Search-owned TUNABLE, NOT a contract constant) — promotion is MEASURED, never predicted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PromotionThreshold {
    /// The fraction (`0.0..=1.0`) of a collection's view executions a facet must STRICTLY exceed to be
    /// promoted to a generated index. The OQ-C default is `0.05` (`> 5%`).
    pub share: f64,
}

impl PromotionThreshold {
    /// **The OQ-C default-to-beat: `> 5%` of a collection's view executions over a rolling window.** A
    /// Search-owned tunable, NOT a contract constant — named here so the feeder gate reads the SAME
    /// threshold the cost-bounder's classification documents
    /// ([`crate::cost_bounder::CostBounderFloors::OQ_C_DEFAULT_TO_BEAT`]).
    pub const OQ_C_DEFAULT_TO_BEAT: f64 = 0.05;

    /// The OQ-C default threshold (`> 5%`).
    pub const DEFAULT: PromotionThreshold = PromotionThreshold {
        share: Self::OQ_C_DEFAULT_TO_BEAT,
    };

    /// A threshold with an explicit share fraction (the drill calibrates this against the live ISS-D2
    /// `<1s` artifact — a Search-owned tunable).
    pub fn new(share: f64) -> PromotionThreshold {
        PromotionThreshold { share }
    }
}

impl Default for PromotionThreshold {
    fn default() -> PromotionThreshold {
        PromotionThreshold::DEFAULT
    }
}

// ───────────────────────────── the rolling-window frequency counter (§3) ─────────────────────────

/// **The per-`(tenant, type, field_id)` filter/sort frequency counter (§3).** A rolling window of a
/// collection's VIEW EXECUTIONS (the denominator) and the per-facet appearances in those executions
/// (the numerator). A view execution records WHICH facets it filtered/sorted on; the counter
/// accumulates per-facet appearances + per-collection total executions, so [`FrequencyCounter::share`]
/// is the measured share the OQ-C gate reads. (The window WIDTH + the demotion-on-cooling half are the
/// ISS-P32 / M5 calibration follow-on — named in [`ProjectionFeederFloors`].)
#[derive(Clone, Debug, Default)]
pub struct FrequencyCounter {
    /// Per-facet appearance count (the numerator — how often this facet was filtered/sorted on).
    appearances: BTreeMap<FacetKey, u64>,
    /// Per-collection total view executions (the denominator — how many views ran for this collection).
    executions: BTreeMap<CollectionKey, u64>,
}

impl FrequencyCounter {
    /// A fresh counter (no observations yet — every facet's share is 0).
    pub fn new() -> FrequencyCounter {
        FrequencyCounter::default()
    }

    /// **Record one VIEW EXECUTION over a collection (the denominator bump).** Every board/list/report
    /// run over `collection` calls this once; `filtered_facets` are the custom `field_id`s that
    /// execution filtered/sorted on (the numerator bump for each). A view filtering on a typed-core
    /// field (no custom facet) bumps only the denominator — exactly right, since a custom facet that
    /// never appears in a view stays cold.
    pub fn record_view_execution(&mut self, collection: &CollectionKey, filtered_facets: &[&str]) {
        *self.executions.entry(collection.clone()).or_insert(0) += 1;
        for &field_id in filtered_facets {
            let key = FacetKey {
                tenant: collection.tenant.clone(),
                type_: collection.type_.clone(),
                field_id: field_id.to_string(),
            };
            *self.appearances.entry(key).or_insert(0) += 1;
        }
    }

    /// The number of view executions recorded for a collection (the denominator).
    pub fn executions(&self, collection: &CollectionKey) -> u64 {
        self.executions.get(collection).copied().unwrap_or(0)
    }

    /// The number of times a facet appeared in its collection's views (the numerator).
    pub fn appearances(&self, facet: &FacetKey) -> u64 {
        self.appearances.get(facet).copied().unwrap_or(0)
    }

    /// **The measured share of a facet (§3 / OQ-C): `appearances / collection-view-executions`.** The
    /// OQ-C gate reads THIS. A facet in a collection with 0 view executions has share 0 (no division by
    /// zero — a never-viewed collection promotes nothing, exactly right: promotion is MEASURED).
    pub fn share(&self, facet: &FacetKey) -> f64 {
        let execs = self.executions(&facet.collection());
        if execs == 0 {
            return 0.0;
        }
        self.appearances(facet) as f64 / execs as f64
    }
}

// ───────────────────────────── the forward-only online migration (1.5) ───────────────────────────

/// **The hot table the generated index is provisioned on (arch 01 §2 / §8.1).** The custom-field
/// facets live in the `issue` table's `props` JSONB tail; the generated/expression index is built on
/// `issue` (declared HOT) — which is exactly why it MUST be `CREATE INDEX CONCURRENTLY` (no blocking
/// `ALTER`, contract 1.5 / §9.4).
pub const ISSUE_HOT_TABLE: &str = "issue";

/// **A forward-only ONLINE migration the feeder runs to provision a generated/expression index (1.5).**
/// Unlike the boot-time [`myelin_storage::migration::Migration`] (which carries a `&'static str` DDL
/// for the frozen schema), a feeder promotion is RUNTIME-shaped (per `(tenant, type, field_id)`), so it
/// carries an OWNED DDL string. It is provisioned via `CREATE INDEX CONCURRENTLY` over the hot facet's
/// `props` expression on the declared-HOT `issue` table — asserted NON-BLOCKING
/// ([`IndexProvisioning::is_non_blocking`], contract 1.5 / §9.4: a hot-table change is
/// expand→backfill→contract, never one blocking `ALTER`). A `CREATE INDEX CONCURRENTLY` takes no
/// exclusive lock → **0 downtime by construction** (the prompt's gate). The migration is FORWARD-ONLY:
/// `IF NOT EXISTS` makes it idempotent/re-runnable and there is no `DROP`/down step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexProvisioning {
    /// The facet this index serves (the promoted `(tenant, type, field_id)`).
    pub facet: FacetKey,
    /// The generated index name (`issue_facet_<field_id>` — the Tier-2 index the cost-bounder reads).
    pub index_name: String,
    /// The owned `CREATE INDEX CONCURRENTLY` DDL (runtime-shaped — the expression index over the hot
    /// facet's `props` key). Owned (not `&'static`) because the facet is dynamic.
    pub ddl: String,
    /// The table the index is built on (the declared-HOT [`ISSUE_HOT_TABLE`]).
    pub table: &'static str,
}

impl IndexProvisioning {
    /// **Build the forward-only online migration that provisions the generated/expression index for a
    /// promoted facet (1.5 / §3).** `CREATE INDEX CONCURRENTLY IF NOT EXISTS` over the `props` JSONB
    /// expression for the hot facet — tenant-scoped via the partial predicate, non-blocking on the hot
    /// `issue` table, idempotent (`IF NOT EXISTS`), forward-only (no down). The expression index lets
    /// the cost-bounder's Tier 2 serve the facet from a generated index instead of the GIN probe.
    pub fn for_facet(facet: &FacetKey) -> IndexProvisioning {
        // The Tier-2 index name the cost-bounder reads — one per facet (the promoted-hot set).
        let index_name = format!("issue_facet_{}", sanitize_ident(&facet.field_id));
        // CONCURRENTLY → no exclusive lock → 0 downtime on the declared-HOT `issue` table (§9.4). The
        // expression `(props ->> '<field_id>')` is the generated/expression index over the JSONB tail;
        // the partial predicate scopes it to the collection's tenant + type (no cross-tenant index).
        let ddl = format!(
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS {index_name} \
             ON {ISSUE_HOT_TABLE} ((props ->> '{field}')) \
             WHERE tenant_id = '{tenant}' AND type_id::text = '{type_}' AND deleted_at IS NULL",
            field = facet.field_id,
            tenant = facet.tenant,
            type_ = facet.type_,
        );
        IndexProvisioning {
            facet: facet.clone(),
            index_name,
            ddl,
            table: ISSUE_HOT_TABLE,
        }
    }

    /// **The 0-downtime gate (§9.4 / contract 1.5): the provisioning is NON-BLOCKING.** `true` iff the
    /// DDL is a `CREATE INDEX CONCURRENTLY` (takes no exclusive lock on the hot table) — the structural
    /// guard the CI gate asserts for EVERY promotion. A blocking `ALTER` on the declared-hot `issue`
    /// table would be REJECTED (the feeder never provisions one). Reads the SAME
    /// [`myelin_storage::migration::is_blocking_alter`] the boot-time runner + the
    /// `forward-only-migration` lint use (one discipline, no second copy).
    pub fn is_non_blocking(&self) -> bool {
        !is_blocking_alter(&self.ddl)
    }

    /// **The forward-only gate (§9.1): no down/destructive step.** `true` iff the DDL carries no
    /// `DROP`/down (forward-only is structural; a "rollback" is a NEW forward migration, never a down).
    pub fn is_forward_only(&self) -> bool {
        let up = self.ddl.to_ascii_uppercase();
        !up.contains("DROP ") && !up.contains("DROP\t")
    }
}

/// Sanitise a `field_id` into a safe SQL identifier fragment for the index NAME (the index name is an
/// identifier, not a value — only `[a-z0-9_]` survive; everything else collapses to `_`). The field
/// VALUE inside the expression rides the normal bound/quoted path; this is only the index name.
fn sanitize_ident(field_id: &str) -> String {
    field_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

// ───────────────────────────── the projection feeder consumer (2.4) ──────────────────────────────

/// **The result of a promotion decision (the feeder's per-facet outcome).** Either the facet crossed
/// the measured threshold and an online migration provisioned its generated index, or it stayed on the
/// GIN probe (below threshold OR already promoted — idempotent).
#[derive(Clone, Debug, PartialEq)]
pub enum PromotionDecision {
    /// PROMOTED — the facet crossed the OQ-C threshold; the [`IndexProvisioning`] online migration ran
    /// (0 downtime) and the facet is now in the [`FacetCatalog`] (Tier 2).
    Promoted(IndexProvisioning),
    /// NOT promoted — the facet's measured share is at-or-below the threshold (still Tier 2b, the GIN
    /// probe). Carries the measured share so telemetry can see how close it is.
    StayedOnGin { share: f64 },
    /// Already promoted — the facet is already in the catalog; the migration is NOT re-run (idempotent).
    AlreadyPromoted,
}

impl PromotionDecision {
    /// `true` iff this decision promoted the facet (ran the online migration).
    pub fn is_promoted(&self) -> bool {
        matches!(self, PromotionDecision::Promoted(_))
    }
}

/// **THE PROJECTION FEEDER (§3 — the measured generated-index promotion consumer; contract 2.4).** A
/// bus [`EventHandler`] that watches `issue.issue.updated` deltas + the per-`(tenant, type, field_id)`
/// [`FrequencyCounter`], and PROMOTES a facet to a generated index — via a 0-downtime forward-only
/// online migration ([`IndexProvisioning`]) — ONLY when the facet crosses the OQ-C measured threshold.
/// The promoted set is held in a [`FacetCatalog`] (the SAME type the cost-bounder's Tier 2 reads,
/// ISS-P14) so a promotion immediately moves the facet from Tier 2b (GIN) to Tier 2 (generated index).
///
/// Interior mutability ([`Mutex`]) because [`EventHandler::handle`] takes `&self` (the consumer runtime
/// holds the handler immutably; the counter + catalog + dedup mutate per event). The feeder is
/// idempotent on `event_id` (the same `issue.updated` is not double-counted) AND idempotent on
/// promotion (a facet already in the catalog is not re-provisioned).
pub struct ProjectionFeeder {
    threshold: PromotionThreshold,
    state: Mutex<FeederState>,
}

#[derive(Default)]
struct FeederState {
    counter: FrequencyCounter,
    /// The promoted-hot set the cost-bounder reads (Tier 2) — fed by promotions off the bus.
    catalog: FacetCatalog,
    /// The `field_id`s already promoted (so a promotion is not re-provisioned). Mirrors the catalog;
    /// kept here so the migration is run at-most-once per facet.
    promoted: std::collections::BTreeSet<FacetKey>,
    /// The `event_id`s already handled (idempotent on `event_id`, contract 2.4 / ADR-04.1).
    seen_events: std::collections::BTreeSet<String>,
}

/// The whitelist subjects the feeder binds — `issue.issue.updated` ONLY, NEVER `*` (BUS-3 / 2.4 — an
/// over-broad subscription head-of-line-blocks everything).
fn feeder_subjects() -> &'static [SubjectPattern] {
    use std::sync::OnceLock;
    static SUBJECTS: OnceLock<Vec<SubjectPattern>> = OnceLock::new();
    SUBJECTS
        .get_or_init(|| vec![SubjectPattern(ISSUE_UPDATED.to_string())])
        .as_slice()
}

impl ProjectionFeeder {
    /// A feeder with the OQ-C default threshold (`> 5%`).
    pub fn new() -> ProjectionFeeder {
        ProjectionFeeder::with_threshold(PromotionThreshold::DEFAULT)
    }

    /// A feeder with an explicit threshold (the drill calibrates it against the live ISS-D2 artifact).
    pub fn with_threshold(threshold: PromotionThreshold) -> ProjectionFeeder {
        ProjectionFeeder {
            threshold,
            state: Mutex::new(FeederState::default()),
        }
    }

    /// **Record one view execution over a collection (the denominator + numerator bump).** A
    /// board/list/report run calls this with the custom facets it filtered/sorted on; it feeds the
    /// frequency counter the promotion gate reads. (The `issue.updated` deltas tell the feeder which
    /// facets EXIST/were written; the view executions tell it which are HOT — the §3 filter/sort
    /// frequency counter. Both feed the same counter.)
    pub fn record_view_execution(&self, collection: &CollectionKey, filtered_facets: &[&str]) {
        let mut state = self.state.lock().expect("feeder state lock");
        state
            .counter
            .record_view_execution(collection, filtered_facets);
    }

    /// **The promotion gate (§3 / OQ-C — MEASURED, never predicted).** `true` iff the facet's
    /// rolling-window share STRICTLY exceeds the threshold AND it is not already promoted. A facet at or
    /// below the threshold is NOT promoted (it stays on the GIN probe — Tier 2b). The strict `>` matches
    /// the OQ-C `> 5%` wording.
    pub fn should_promote(&self, facet: &FacetKey) -> bool {
        let state = self.state.lock().expect("feeder state lock");
        Self::should_promote_locked(&state, self.threshold, facet)
    }

    fn should_promote_locked(
        state: &FeederState,
        threshold: PromotionThreshold,
        facet: &FacetKey,
    ) -> bool {
        if state.promoted.contains(facet) {
            return false;
        }
        state.counter.share(facet) > threshold.share
    }

    /// **Evaluate a facet for promotion + run the online migration if it crosses the threshold.** The
    /// measured-promotion path: if the facet's share strictly exceeds the OQ-C threshold (and it is not
    /// already promoted), provision the generated/expression index via a 0-downtime forward-only online
    /// migration, record it in the [`FacetCatalog`] the cost-bounder reads, and return the
    /// [`IndexProvisioning`]. Otherwise the facet stays on the GIN probe (Tier 2b). Idempotent: a facet
    /// already promoted returns [`PromotionDecision::AlreadyPromoted`] and re-runs nothing.
    pub fn evaluate_facet(&self, facet: &FacetKey) -> PromotionDecision {
        let mut state = self.state.lock().expect("feeder state lock");
        Self::evaluate_facet_locked(&mut state, self.threshold, facet)
    }

    fn evaluate_facet_locked(
        state: &mut FeederState,
        threshold: PromotionThreshold,
        facet: &FacetKey,
    ) -> PromotionDecision {
        if state.promoted.contains(facet) {
            return PromotionDecision::AlreadyPromoted;
        }
        let share = state.counter.share(facet);
        if share <= threshold.share {
            return PromotionDecision::StayedOnGin { share };
        }
        // MEASURED-HOT → provision the generated index via a 0-downtime forward-only online migration.
        let provisioning = IndexProvisioning::for_facet(facet);
        // The structural 0-downtime guard: the feeder NEVER provisions a blocking migration on the hot
        // `issue` table (it would lock writes at QPS). This holds by construction (CONCURRENTLY).
        debug_assert!(
            provisioning.is_non_blocking() && provisioning.is_forward_only(),
            "the feeder must only provision a non-blocking, forward-only online migration"
        );
        state.catalog.promote(facet.field_id.clone());
        state.promoted.insert(facet.clone());
        PromotionDecision::Promoted(provisioning)
    }

    /// **A snapshot of the promoted-hot [`FacetCatalog`] (the Tier-2 set the cost-bounder reads).** The
    /// cost-bounder ([`crate::cost_bounder::plan_board_query`]) reads a `FacetCatalog`; the feeder feeds
    /// it. A caller wires the feeder's catalog into the cost-bounder so a promotion immediately moves
    /// the facet from Tier 2b (GIN) to Tier 2 (the generated index).
    pub fn catalog_snapshot(&self) -> FacetCatalog {
        let state = self.state.lock().expect("feeder state lock");
        state.catalog.clone()
    }

    /// Whether a facet has been promoted (is in the catalog) — for telemetry / the gate assertion.
    pub fn is_promoted(&self, facet: &FacetKey) -> bool {
        let state = self.state.lock().expect("feeder state lock");
        state.promoted.contains(facet)
    }

    /// Extract the `(tenant, type, field_id)` facets an `issue.updated` event touched, from its
    /// payload's field deltas (references-not-payloads — the payload carries the changed field ids, not
    /// PII bodies). The payload shape: `{ "type": "<type_id>", "changed_fields": ["<field_id>", …] }`.
    /// An event without recognisable field deltas touches no facet (returns empty).
    fn facets_in_event(ev: &EventEnvelope) -> Vec<FacetKey> {
        let tenant = ev.tenant.0.clone();
        let Some(obj) = ev.payload.as_object() else {
            return Vec::new();
        };
        let type_ = obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let Some(changed) = obj.get("changed_fields").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        changed
            .iter()
            .filter_map(|v| v.as_str())
            .map(|field_id| FacetKey::new(tenant.clone(), type_.clone(), field_id))
            .collect()
    }
}

impl Default for ProjectionFeeder {
    fn default() -> ProjectionFeeder {
        ProjectionFeeder::new()
    }
}

impl EventHandler for ProjectionFeeder {
    /// The whitelist — `issue.issue.updated` ONLY, NEVER `*` (BUS-3 / 2.4).
    fn subjects(&self) -> &'static [SubjectPattern] {
        feeder_subjects()
    }

    /// **Handle one `issue.updated` delta (contract 2.4 — idempotent on `event_id`).** Record each
    /// touched facet's existence, then evaluate it for promotion against the measured threshold. The
    /// promotion (if any) runs a 0-downtime forward-only online migration and feeds the catalog. The
    /// feeder is idempotent: the same `event_id` is handled at-most-once (the dedup-within-handler
    /// guard, on top of the runtime's `consumer_dedup` ledger), and a facet already promoted is not
    /// re-provisioned. Always returns `Done` for a well-formed `issue.updated` (a non-`issue.updated`
    /// subject is a misroute → `NonRetryable`).
    fn handle(&self, ev: &EventEnvelope, _tx: &mut myelin_events::HandlerTx<'_>) -> HandleOutcome {
        if ev.type_.0 != ISSUE_UPDATED {
            return HandleOutcome::NonRetryable(Reason(format!(
                "projection feeder bound to `{ISSUE_UPDATED}` received `{}` — misroute",
                ev.type_.0
            )));
        }
        let mut state = self.state.lock().expect("feeder state lock");
        // Idempotent on event_id (ADR-04.1) — a redelivered event is a no-op.
        if !state.seen_events.insert(ev.event_id.0.clone()) {
            return HandleOutcome::Done;
        }
        for facet in Self::facets_in_event(ev) {
            // The issue.updated delta tells the feeder the facet exists/was written. The promotion
            // decision reads the frequency counter (fed by view executions) — promotion is MEASURED.
            let _ = Self::evaluate_facet_locked(&mut state, self.threshold, &facet);
        }
        HandleOutcome::Done
    }
}

// ───────────────────────────── the named floors (§3 — measured calibration) ──────────────────────

/// **FLOORS named (ISS-P15 DoD) — greppable markers for the measured calibration follow-ons.**
#[derive(Clone, Copy, Debug)]
pub struct ProjectionFeederFloors;

impl ProjectionFeederFloors {
    /// **The promotion threshold is the OQ-C default-to-beat (`> 5%` of a collection's view
    /// executions) — a Search-owned TUNABLE, not a contract constant.** Promotion is MEASURED, never
    /// predicted. Calibrated by the ISS-D2 `<1s` drill ([`PromotionThreshold::OQ_C_DEFAULT_TO_BEAT`]).
    pub const OQ_C_THRESHOLD: &'static str =
        "> 5% of a collection's view executions (OQ-C tunable)";
    /// **The rolling-window WIDTH + demotion-on-cooling — the at-scale calibration follow-on.** This
    /// module's counter accumulates over the process lifetime (the in-memory window); the calibrated
    /// rolling window (the time/count bound) + the demotion path (a facet that goes cold loses its
    /// generated index) is the surge-family follow-on. M5 / ISS-P32.
    pub const WINDOW_CALIBRATION: &'static str = "ISS-P32";
    /// **The live wall-clock `<1s` proof the promoted index serves Tier 2 under budget** — the ISS-D2
    /// `--features integration` drill against the dev-stack Postgres (ISS-P14's integration test). This
    /// CI gate is the threshold-gated promotion + the 0-downtime online-migration signal.
    pub const ISS_D2_LIVE_PROOF: &'static str = "ISS-P14 integration drill (ISS-D2)";
}

#[cfg(test)]
mod tests;
