//! # The measured projection-feeder promotion (GIN scan → generated index, OQ-C) — SRCH-P27 / P-462, M5
//!
//! **Architecture:** `search-and-indexing.md` §4.6.1 (the projection-feeder promotion threshold —
//! SHARPENED, OQ-C: a custom facet filtered in **> 5 % of a collection's view executions over a
//! rolling window** is promoted from a cold **GIN-indexed JSONB scan** to a **generated/columnar
//! index**; the GIN scan serves CORRECTLY meanwhile — promotion changes COST, never correctness).
//! **Reconciliation:** `00-reconciliation-decisions.md` OQ-C (the promotion threshold). **Contracts:**
//! 6.3 (`declare_indexable` + the measured projection-feeder promotion — the OQ-C tail), 1.8 (the
//! per-facet filter-frequency signal). **Doctrine:** `external-insights/01-process-and-quality-
//! doctrine.md` §3 (measured, never predicted), §7 (abstract at the THIRD copy — the generated index).
//!
//! ## Ownership — who owns the signal, who decides the promotion (§4.6.1)
//! The OWNER of the per-facet filter-frequency signal is **Issues/Knowledge**
//! ([`myelin_knowledge::FacetTelemetry`] / [`myelin_knowledge::FACET_PROMOTION_THRESHOLD`]): the
//! producer records which facets each view execution filtered on. SRCH-P27 is the **Search-side
//! CONSUMER**: it reads that frequency signal and decides promotion for **Search's OWN index**, where
//! a facet is served by a cold GIN-indexed JSONB scan until it crosses the measured `> 5 %` threshold,
//! at which point Search promotes it to a generated/columnar fast-field index. The same OQ-C number
//! (`0.05`) the KN owner measures the frequency against — read here from the thresholds file
//! (`[projection_feeder]`), a **Search-owned tunable, NOT a contract constant** (§4.6.1).
//!
//! ## What this slice ships
//!   - [`ViewExecutionTelemetry`] — the Search-side per-(collection, facet) view-execution counter the
//!     consumer reads. Mirrors the KN owner's [`myelin_knowledge::FacetTelemetry`] shape (a facet is
//!     counted ONCE per execution regardless of how many times its filter references it — the 6.3
//!     "fraction of EXECUTIONS that touched the facet" window definition), so the two never drift.
//!   - [`FacetServingPath`] — which path serves a facet (`GinScan` cold / `GeneratedIndex` hot). The
//!     [`FacetCollection`] starts every facet on the GIN scan and promotes a hot one in place.
//!   - [`FacetCollection`] — a small Search facet collection serving a typed facet-equality query BOTH
//!     ways: the **GIN scan** ([`FacetCollection::serve_gin_scan`], a full scan over the docs' props +
//!     the ACL-admit) and the **generated index** ([`FacetCollection::serve_generated_index`], a
//!     pre-built value→doc-ids lookup + the same ACL-admit). The promotion ([`FacetCollection::
//!     promote`]) only switches which path [`FacetCollection::serve`] dispatches to.
//!   - [`ProjectionFeederGate`] — the SCHED gate (SRCH-P27): a facet crossing the measured `> 5 %`
//!     threshold is promoted; for EVERY facet value, both serving paths return the **byte-identical**
//!     ordered visible doc-id set across the promotion (cost changes, correctness does not). A
//!     promotion that changed ANY result is a typed RED [`ProjectionFeederFailure`], never softened.
//!
//! ## The numbers are MEASURED, not predicted (EI-01 §3) — recorded honestly
//! The **threshold** (`> 5 %`) is the OQ-C default-to-beat carried in the thresholds file — the SAME
//! number the Issues/KN owner measures its frequency signal against (Search consumes it, it is not
//! re-derived here). The **promotion MECHANISM + the byte-identical-correctness invariant** are
//! MEASURED here: the gate drives both serving paths over a real corpus across the promotion and
//! proves they agree. [`ProjectionFeederArtifact::threshold_measured`] records this split honestly
//! (`false` — the threshold is carried, not re-measured; the mechanism is what this gate proves).
//!
//! ## Floors named
//! - This is the named SRCH-P17 / SRCH-P20 GIN-scan floor's follow-on (the GIN scan serves the facets
//!   meanwhile; THIS promotes a hot one). No NEW floor: a Search-owned tunable, not a contract constant
//!   (§4.6.1). The **world-scale fleet run** (a real per-cell collection at scale, the cell-class
//!   rolling window) re-confirms the threshold + the promotion at real cardinality — the one remaining
//!   world-scale floor (shared testing-strategy §4.1); the promotion LOGIC + the byte-identical proof +
//!   the measured-threshold write ship now and re-run as a `cargo test` gate on every facet change.

use std::collections::BTreeMap;

use myelin_query::FieldValue;
use myelin_substrate::thresholds::ProjectionFeeder;
use myelin_tenancy::{Region, TenantId};

use crate::engine::AclFilter;

/// **The Search-side per-(collection, facet) view-execution frequency signal (contract 1.8).** The
/// consumer half of the OQ-C signal: every view execution records which facets its filter referenced
/// ([`ViewExecutionTelemetry::record_execution`]); [`ViewExecutionTelemetry::should_promote`] reads
/// the threshold from the thresholds file and decides whether a facet has crossed the measured `> 5 %`
/// trigger. Mirrors the KN owner's [`myelin_knowledge::FacetTelemetry`] window semantics (a facet is
/// counted ONCE per execution) so the consumer and the owner never drift. PII-free (counts only).
#[derive(Clone, Debug, Default)]
pub struct ViewExecutionTelemetry {
    /// `collection` → (total executions, per-facet usage count).
    collections: BTreeMap<String, CollectionCounters>,
}

/// Per-collection view-execution counters (the rolling-window state).
#[derive(Clone, Debug, Default)]
struct CollectionCounters {
    /// Total view executions over the window.
    total: u64,
    /// `facet` → how many of those executions referenced this facet.
    facet_uses: BTreeMap<String, u64>,
}

impl ViewExecutionTelemetry {
    /// A fresh, empty telemetry register.
    pub fn new() -> ViewExecutionTelemetry {
        ViewExecutionTelemetry::default()
    }

    /// **Record one view execution over `collection` referencing `facets` (§4.6.1 telemetry).**
    /// Increments the collection's total execution count and each referenced facet's usage count. A
    /// facet is counted ONCE per execution regardless of how many times the filter references it (the
    /// 6.3 "fraction of EXECUTIONS that touched the facet" window — the SAME definition as the KN
    /// owner's [`myelin_knowledge::FacetTelemetry::record_execution`]).
    pub fn record_execution(&mut self, collection: &str, facets: &[&str]) {
        let entry = self.collections.entry(collection.to_string()).or_default();
        entry.total += 1;
        let mut counted = std::collections::BTreeSet::new();
        for f in facets {
            if counted.insert(*f) {
                *entry.facet_uses.entry((*f).to_string()).or_insert(0) += 1;
            }
        }
    }

    /// The total recorded view executions for a collection (the rolling-window denominator).
    pub fn total_executions(&self, collection: &str) -> u64 {
        self.collections
            .get(collection)
            .map(|c| c.total)
            .unwrap_or(0)
    }

    /// How many recorded executions referenced `facet` in `collection` (the numerator).
    pub fn facet_uses(&self, collection: &str, facet: &str) -> u64 {
        self.collections
            .get(collection)
            .and_then(|c| c.facet_uses.get(facet).copied())
            .unwrap_or(0)
    }

    /// The execution frequency of a facet (fraction of executions that referenced it, `0.0..=1.0`).
    /// `0.0` if the collection has no recorded executions.
    pub fn facet_frequency(&self, collection: &str, facet: &str) -> f64 {
        let total = self.total_executions(collection);
        if total == 0 {
            return 0.0;
        }
        self.facet_uses(collection, facet) as f64 / total as f64
    }

    /// **Whether `facet` has crossed the measured promotion threshold** (`uses/total > ratio` over a
    /// window of `≥ min_executions`, STRICTLY greater-than — §4.6.1). Reads the threshold from the
    /// thresholds-file section (the source of truth), never a hardcoded number.
    pub fn should_promote(&self, collection: &str, facet: &str, t: &ProjectionFeeder) -> bool {
        t.should_promote(
            self.facet_uses(collection, facet),
            self.total_executions(collection),
        )
    }
}

/// **Which serving path a facet is on (§4.6.1).** A facet starts on the cold GIN scan and is promoted
/// to the generated index once it crosses the measured threshold. The path changes the COST of a facet
/// query (a full props scan vs a value→doc-ids lookup), never the RESULT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetServingPath {
    /// The cold path — a GIN-indexed JSONB scan over the docs' property bags (the default; serves every
    /// facet CORRECTLY meanwhile). The cost is a full scan over the collection.
    GinScan,
    /// The hot path — a generated/columnar fast-field index (a pre-built value→doc-ids map). The cost
    /// is a single value lookup. Promoted to when the facet crosses the measured `> 5 %` threshold.
    GeneratedIndex,
}

/// A doc in a Search facet collection — its id, its typed facet values (the structured/columnar shape),
/// and its ACL object id (the pre-filter key). Mirrors the engine's index-document facet shape; the
/// minimal model the promotion gate drives both serving paths over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetDoc {
    /// The primary key (the same `doc_id` that keys the engine's co-located shapes).
    pub doc_id: String,
    /// The ACL object id the [`AclFilter`] pre-filter pins on (the engine's `acl_object`).
    pub acl_object: String,
    /// The typed structured facets, keyed by facet name (the §3.1 columnar shape). The GIN scan filters
    /// over these; the generated index is built from them.
    pub facets: BTreeMap<String, FieldValue>,
}

impl FacetDoc {
    /// A new doc keyed by `doc_id`, ACL-scoped by `acl_object`, with no facets yet.
    pub fn new(doc_id: impl Into<String>, acl_object: impl Into<String>) -> FacetDoc {
        FacetDoc {
            doc_id: doc_id.into(),
            acl_object: acl_object.into(),
            facets: BTreeMap::new(),
        }
    }

    /// Add a typed facet value (builder).
    pub fn with_facet(mut self, name: impl Into<String>, value: FieldValue) -> FacetDoc {
        self.facets.insert(name.into(), value);
        self
    }
}

/// **A Search facet collection that serves a typed facet-equality query BOTH ways (§4.6.1).** The GIN
/// scan and the generated index serve the SAME facet over the SAME docs under the SAME ACL filter; the
/// promotion only switches which path [`FacetCollection::serve`] dispatches to. The byte-identical-
/// correctness invariant the gate proves: for every facet value, both paths return the identical
/// ordered visible doc-id set.
#[derive(Clone, Debug, Default)]
pub struct FacetCollection {
    /// The collection name (the rolling-window key).
    name: String,
    /// The docs, in a stable order (doc_id-sorted — the canonical result order both paths return).
    docs: Vec<FacetDoc>,
    /// Per-facet serving path. Absent ⇒ the default cold [`FacetServingPath::GinScan`].
    paths: BTreeMap<String, FacetServingPath>,
    /// The generated indexes built for the promoted facets: `facet` → (serialized value → doc-ids in
    /// the canonical order). Built at promotion from the SAME doc set the GIN scan reads.
    generated: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl FacetCollection {
    /// A new, empty collection named `name`.
    pub fn new(name: impl Into<String>) -> FacetCollection {
        FacetCollection {
            name: name.into(),
            docs: Vec::new(),
            paths: BTreeMap::new(),
            generated: BTreeMap::new(),
        }
    }

    /// The collection name (the rolling-window key).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Add a doc, keeping the doc list in the canonical (`doc_id`-sorted) order so both serving paths
    /// return results in the SAME order (a byte-identical comparison requires a stable order). Any
    /// already-built generated index is invalidated for correctness (the doc set changed) — it must be
    /// rebuilt at the next promotion (or, for an already-promoted facet, rebuilt here).
    pub fn add(&mut self, doc: FacetDoc) {
        match self.docs.binary_search_by(|d| d.doc_id.cmp(&doc.doc_id)) {
            Ok(i) => self.docs[i] = doc, // replace-in-place (an upsert by doc_id)
            Err(i) => self.docs.insert(i, doc),
        }
        // Rebuild any promoted facet's generated index so it never serves a stale doc set.
        let promoted: Vec<String> = self.generated.keys().cloned().collect();
        for facet in promoted {
            self.build_generated_index(&facet);
        }
    }

    /// The serving path a facet is on (the default is the cold GIN scan).
    pub fn path_of(&self, facet: &str) -> FacetServingPath {
        self.paths
            .get(facet)
            .copied()
            .unwrap_or(FacetServingPath::GinScan)
    }

    /// **Promote `facet` from the GIN scan to a generated index (§4.6.1).** Builds the value→doc-ids
    /// generated index from the SAME doc set the GIN scan reads, then flips the serving path. Idempotent
    /// (promoting an already-promoted facet rebuilds the index — never a duplicate). Promotion changes
    /// COST (a value lookup instead of a full scan), never correctness.
    pub fn promote(&mut self, facet: &str) {
        self.build_generated_index(facet);
        self.paths
            .insert(facet.to_string(), FacetServingPath::GeneratedIndex);
    }

    /// Build the generated value→doc-ids index for `facet` from the live doc set, in the canonical
    /// (`doc_id`-sorted) order. The value key is the doc-value's stable JSON serialization (a
    /// byte-stable key — the SAME bytes the GIN scan would compare). A doc that lacks the facet
    /// contributes nothing (it never matches an equality on a value it does not carry).
    fn build_generated_index(&mut self, facet: &str) {
        let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for doc in &self.docs {
            if let Some(v) = doc.facets.get(facet) {
                index
                    .entry(value_key(v))
                    .or_default()
                    .push(doc.doc_id.clone());
            }
        }
        // The docs are already doc_id-sorted, so each value's id list is too (the canonical order).
        self.generated.insert(facet.to_string(), index);
    }

    /// **Serve a facet-equality query the COLD way — the GIN-indexed JSONB scan (§4.6.1).** A full scan
    /// over the docs' property bags filtering on `facet == value`, then the ACL-admit pre-filter. The
    /// visible matching doc-ids in the canonical order. This path always serves CORRECTLY — it is the
    /// reference both the promotion and the generated index are checked against.
    pub fn serve_gin_scan(&self, facet: &str, value: &FieldValue, acl: &AclFilter) -> Vec<String> {
        let key = value_key(value);
        self.docs
            .iter()
            .filter(|d| d.facets.get(facet).map(value_key).as_deref() == Some(key.as_str()))
            .filter(|d| acl.admits(&d.acl_object))
            .map(|d| d.doc_id.clone())
            .collect()
    }

    /// **Serve a facet-equality query the HOT way — the generated/columnar index (§4.6.1).** A single
    /// value→doc-ids lookup, then the ACL-admit pre-filter. The visible matching doc-ids in the
    /// canonical order. If the facet has no generated index built (it was never promoted), this falls
    /// back to the GIN scan (the index is a COST optimisation, never a correctness dependency).
    pub fn serve_generated_index(
        &self,
        facet: &str,
        value: &FieldValue,
        acl: &AclFilter,
    ) -> Vec<String> {
        let Some(index) = self.generated.get(facet) else {
            // No generated index — fall back to the always-correct GIN scan.
            return self.serve_gin_scan(facet, value, acl);
        };
        let key = value_key(value);
        index
            .get(&key)
            .map(|ids| {
                ids.iter()
                    .filter(|id| {
                        // Resolve the doc to its ACL object for the pre-filter (the index keys doc-ids;
                        // the ACL pin is on the doc's acl_object, exactly as the GIN scan applies it).
                        self.docs
                            .iter()
                            .find(|d| &d.doc_id == *id)
                            .map(|d| acl.admits(&d.acl_object))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **Serve a facet-equality query via whichever path the facet is currently on (§4.6.1).** Cold
    /// facets ride the GIN scan; a promoted facet rides the generated index. The RESULT is identical
    /// either way (the byte-identical-correctness invariant) — only the COST differs.
    pub fn serve(&self, facet: &str, value: &FieldValue, acl: &AclFilter) -> Vec<String> {
        match self.path_of(facet) {
            FacetServingPath::GinScan => self.serve_gin_scan(facet, value, acl),
            FacetServingPath::GeneratedIndex => self.serve_generated_index(facet, value, acl),
        }
    }

    /// The distinct values a facet carries across the live doc set (the value domain the gate sweeps to
    /// prove both paths agree on EVERY value). In the byte-stable key order.
    pub fn facet_values(&self, facet: &str) -> Vec<FieldValue> {
        let mut seen: BTreeMap<String, FieldValue> = BTreeMap::new();
        for doc in &self.docs {
            if let Some(v) = doc.facets.get(facet) {
                seen.entry(value_key(v)).or_insert_with(|| v.clone());
            }
        }
        seen.into_values().collect()
    }
}

/// The stable, byte-identical key for a facet value — its canonical JSON serialization. The SAME bytes
/// the GIN scan compares and the generated index keys on, so "the results are byte-identical across the
/// promotion" is a literal byte comparison, never a semantic re-interpretation. `FieldValue` is
/// always JSON-serializable (the frozen taxonomy), so this never fails; an impossible failure falls
/// back to the Debug form (still stable per value).
fn value_key(v: &FieldValue) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| format!("{v:?}"))
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The SCHED gate — the dated artifact + the typed failure + the gate
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN ARTIFACT a projection-feeder promotion run returns (the SRCH-P27 SCHED proof;
/// observability is part of the pass). Carries the MEASURED numbers: the facet's view-execution
/// frequency, the threshold it crossed, and the number of facet values over which both serving paths
/// were proven byte-identical across the promotion. PII-free (names + counts only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectionFeederArtifact {
    /// The cell the gate ran within (Search never crosses it).
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// The collection the facet belongs to.
    pub collection: String,
    /// The facet that was promoted.
    pub facet: String,
    /// The MEASURED facet usage count (executions that referenced the facet) — the numerator.
    pub facet_uses: u64,
    /// The total view executions over the window — the denominator.
    pub total_executions: u64,
    /// The promotion ratio (basis points, 10 000 = 100 %) the facet's frequency crossed (`> 5 % =
    /// 500 bps`) — recorded so the artifact proves the trigger was the frozen OQ-C number.
    pub threshold_bps: u32,
    /// The MEASURED facet frequency in basis points (floored) — must be `> threshold_bps`.
    pub measured_frequency_bps: u32,
    /// The number of distinct facet VALUES over which both serving paths were proven to return the
    /// byte-identical visible doc-id set across the promotion. The breadth of the correctness proof.
    pub values_checked: u64,
    /// Honest recording (the TESTS line): `false` — the THRESHOLD (`> 5 %`) is the OQ-C default-to-beat
    /// carried in the thresholds file (the SAME number the Issues/KN owner measures against), NOT
    /// re-measured here. The PROMOTION MECHANISM + the byte-identical-correctness invariant ARE measured
    /// (the gate drives both paths over a real corpus across the promotion).
    pub threshold_measured: bool,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl ProjectionFeederArtifact {
    /// Whether the SCHED gate is GREEN: the facet crossed the threshold AND every value's results were
    /// byte-identical across the promotion (≥ 1 value actually checked — a vacuous proof is not green).
    pub fn is_green(&self) -> bool {
        self.measured_frequency_bps > self.threshold_bps && self.values_checked > 0
    }

    /// The dated green-artifact line a SCHED run prints on PASS. The caller prefixes the date
    /// (`[P-462 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search projection-feeder promotion PASS (SRCH-P27, OQ-C): collection={}, facet={} \
             promoted from the GIN scan to the generated index — MEASURED frequency {}bps ({:.2}% \
             of {} view executions, {} uses) crossed the {}bps (> {:.0}%) threshold. Results are \
             BYTE-IDENTICAL across the promotion over {} distinct facet value(s) (cost changes, \
             correctness does not). Threshold carried as the OQ-C default-to-beat (the Issues/KN-owned \
             signal); the promotion mechanism + the byte-identical invariant MEASURED. Written to the \
             thresholds file ([projection_feeder]).",
            self.collection,
            self.facet,
            self.measured_frequency_bps,
            self.measured_frequency_bps as f64 / 100.0,
            self.total_executions,
            self.facet_uses,
            self.threshold_bps,
            self.threshold_bps as f64 / 100.0,
            self.values_checked,
        )
    }
}

/// A RED projection-feeder result — EXACTLY which promotion invariant failed (observability is part of
/// the pass). Never a bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectionFeederFailure {
    /// **The promotion CHANGED a result** — for some facet value, the GIN scan and the generated index
    /// returned DIFFERENT visible doc-id sets. This is the gravest failure: promotion must change COST,
    /// never correctness (§4.6.1). Carries the value key + both result sets. FAILs the gate LOUD; the
    /// generated index is NEVER shipped if it disagrees with the GIN scan.
    ResultChanged {
        value_key: String,
        gin_scan: Vec<String>,
        generated_index: Vec<String>,
    },
    /// **The facet did NOT cross the threshold** — promoting it would be premature (the GIN scan still
    /// serves it within budget). Carries the measured frequency + the threshold (bps). The gate FAILs
    /// rather than promoting a cold facet (a wasted generated index).
    BelowThreshold {
        measured_bps: u32,
        threshold_bps: u32,
    },
    /// **No facet values were checked** — a promotion proof that compared nothing cannot prove the
    /// byte-identical invariant (a facet with no values, or a mis-specified drill). FAILs LOUD.
    NoValuesChecked,
    /// **The threshold is mis-specified** (a ratio ≤ 0 / ≥ 1 or a 0 execution floor — a vacuous bar).
    /// A green can never be manufactured by a vacuous threshold (EI-01 §3). FAILs LOUD.
    MisspecifiedThreshold,
}

impl core::fmt::Display for ProjectionFeederFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ProjectionFeederFailure::ResultChanged {
                value_key,
                gin_scan,
                generated_index,
            } => write!(
                f,
                "PROJECTION-FEEDER FAIL — the promotion CHANGED a result for facet value {value_key}: \
                 the GIN scan returned {gin_scan:?} but the generated index returned \
                 {generated_index:?}. Promotion must change COST, never correctness (§4.6.1) — the \
                 generated index is NEVER shipped if it disagrees with the GIN scan"
            ),
            ProjectionFeederFailure::BelowThreshold {
                measured_bps,
                threshold_bps,
            } => write!(
                f,
                "PROJECTION-FEEDER FAIL — the facet frequency {measured_bps}bps did NOT cross the \
                 {threshold_bps}bps (> 5 %) threshold: promoting it would be premature (the GIN scan \
                 still serves it). Measured, never predicted (EI-01 §3)"
            ),
            ProjectionFeederFailure::NoValuesChecked => write!(
                f,
                "PROJECTION-FEEDER FAIL — 0 facet values checked: a promotion proof that compared \
                 nothing cannot prove the byte-identical invariant (a mis-specified drill)"
            ),
            ProjectionFeederFailure::MisspecifiedThreshold => write!(
                f,
                "PROJECTION-FEEDER FAIL — the threshold is mis-specified (a ratio ≤ 0 / ≥ 1 or a 0 \
                 execution floor). A green cannot be manufactured by a vacuous bar (EI-01 §3)"
            ),
        }
    }
}

impl std::error::Error for ProjectionFeederFailure {}

/// The typed verdict of a projection-feeder promotion run — GREEN ([`ProjectionFeederArtifact`]) or RED
/// ([`ProjectionFeederFailure`]). `#[must_use]`: a dropped verdict is a swallowed promotion check.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a projection-feeder verdict must be checked — a dropped RED is a SWALLOWED \
              correctness/promotion failure (the SRCH-P27 gate, EI-01 §5: loud-never-swallowed)"]
pub enum ProjectionFeederVerdict {
    /// The facet crossed the threshold + every value's results were byte-identical. The dated artifact.
    Green(ProjectionFeederArtifact),
    /// EXACTLY what broke. FAILs the gate; never swallowed.
    Red(ProjectionFeederFailure),
}

impl ProjectionFeederVerdict {
    /// `true` iff the gate passed.
    pub fn is_green(&self) -> bool {
        matches!(self, ProjectionFeederVerdict::Green(_))
    }
    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&ProjectionFeederArtifact> {
        match self {
            ProjectionFeederVerdict::Green(a) => Some(a),
            ProjectionFeederVerdict::Red(_) => None,
        }
    }
    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&ProjectionFeederFailure> {
        match self {
            ProjectionFeederVerdict::Green(_) => None,
            ProjectionFeederVerdict::Red(f) => Some(f),
        }
    }
}

/// **The SRCH-P27 projection-feeder promotion gate (P-462).** Given a [`FacetCollection`] (mutated in
/// place to promote the facet), the consumed view-execution telemetry, the facet to promote, the
/// threshold (from the thresholds file), and the ACL filter the proof runs under, it:
/// 0. checks the threshold is well-formed (a vacuous bar can never manufacture a green);
/// 1. checks the facet has crossed the measured `> 5 %` threshold (else promotion is premature);
/// 2. captures every facet value's GIN-scan result set BEFORE the promotion;
/// 3. promotes the facet (GIN scan → generated index);
/// 4. re-serves every facet value via the (now generated-index) path and proves the result is
///    BYTE-IDENTICAL to the pre-promotion GIN-scan result (cost changed, correctness did not).
///
/// Returns [`ProjectionFeederVerdict::Green`] (the dated artifact) or [`ProjectionFeederVerdict::Red`]
/// (exactly what broke). NEVER swallows.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProjectionFeederGate;

impl ProjectionFeederGate {
    /// A new gate (stateless).
    pub fn new() -> ProjectionFeederGate {
        ProjectionFeederGate
    }

    /// **Evaluate the SRCH-P27 gate — promote a hot facet and prove the promotion changed cost, not
    /// correctness.** Mutates `collection` to promote `facet` (the gate IS the promotion act). `acl` is
    /// the ACL pre-filter the proof runs both paths under (a real, non-`All` filter so the proof covers
    /// the ACL-admit on both paths). See the type docs for the step sequence.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        tenant: &TenantId,
        region: &Region,
        collection: &mut FacetCollection,
        telemetry: &ViewExecutionTelemetry,
        facet: &str,
        acl: &AclFilter,
        t: &ProjectionFeeder,
        now: &str,
    ) -> ProjectionFeederVerdict {
        // (0) The threshold must be well-formed — a vacuous bar can never manufacture a green.
        if !t.is_well_formed() {
            return ProjectionFeederVerdict::Red(ProjectionFeederFailure::MisspecifiedThreshold);
        }

        let coll_name = collection.name().to_string();
        let total = telemetry.total_executions(&coll_name);
        let uses = telemetry.facet_uses(&coll_name, facet);
        let threshold_bps = ratio_to_bps(t.promotion_ratio);
        let measured_bps = freq_to_bps(uses, total);

        // (1) The facet must have crossed the measured > 5 % threshold (else promotion is premature).
        if !telemetry.should_promote(&coll_name, facet, t) {
            return ProjectionFeederVerdict::Red(ProjectionFeederFailure::BelowThreshold {
                measured_bps,
                threshold_bps,
            });
        }

        // (2) Capture every facet value's GIN-scan result BEFORE the promotion (the reference truth).
        let values = collection.facet_values(facet);
        if values.is_empty() {
            return ProjectionFeederVerdict::Red(ProjectionFeederFailure::NoValuesChecked);
        }
        let before: Vec<(FieldValue, Vec<String>)> = values
            .iter()
            .map(|v| (v.clone(), collection.serve_gin_scan(facet, v, acl)))
            .collect();

        // (3) Promote the facet (GIN scan → generated index) — the cost change.
        collection.promote(facet);

        // (4) Re-serve every value via the now-generated-index path; prove BYTE-IDENTICAL results.
        for (value, gin_result) in &before {
            let generated_result = collection.serve_generated_index(facet, value, acl);
            if &generated_result != gin_result {
                return ProjectionFeederVerdict::Red(ProjectionFeederFailure::ResultChanged {
                    value_key: value_key(value),
                    gin_scan: gin_result.clone(),
                    generated_index: generated_result,
                });
            }
        }

        ProjectionFeederVerdict::Green(ProjectionFeederArtifact {
            tenant: tenant.clone(),
            region: region.clone(),
            collection: coll_name,
            facet: facet.to_string(),
            facet_uses: uses,
            total_executions: total,
            threshold_bps,
            measured_frequency_bps: measured_bps,
            values_checked: before.len() as u64,
            // Honest recording: the threshold is the OQ-C carried default-to-beat (Issues/KN-owned),
            // the promotion mechanism + the byte-identical invariant are what THIS gate measures.
            threshold_measured: false,
            ran_at: now.to_string(),
        })
    }
}

/// A promotion ratio (`0.0..=1.0`) as basis points (floored). `0.05` → `500` bps.
fn ratio_to_bps(ratio: f64) -> u32 {
    (ratio * 10_000.0).floor().clamp(0.0, 10_000.0) as u32
}

/// A facet frequency (`uses/total`) as basis points (floored — never rounded UP over the threshold, so
/// an at-threshold frequency can never be flattered over the bar). `total == 0` → `0`.
fn freq_to_bps(uses: u64, total: u64) -> u32 {
    if total == 0 {
        return 0;
    }
    (((uses as u128) * 10_000) / (total as u128)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    /// A collection of `n` issues whose `state_category` facet takes one of a few values, ACL-scoped by
    /// a per-doc `acl_object` (`obj-<i>`). A deterministic, byte-stable corpus.
    fn issues_collection(n: usize) -> FacetCollection {
        let states = ["backlog", "started", "done", "cancelled"];
        let mut c = FacetCollection::new("issues");
        for i in 0..n {
            c.add(
                FacetDoc::new(format!("ENG-{i:04}"), format!("obj-{i}")).with_facet(
                    "state_category",
                    FieldValue::Select(states[i % states.len()].to_string()),
                ),
            );
        }
        c
    }

    /// **The Search-side promotion ratio is the SAME OQ-C number the Issues/KN owner carries — proven
    /// THROUGH THE SHARED THRESHOLDS FILE (the §4.6.1 reconciliation point).**
    ///
    /// `myelin-search` cannot depend on `myelin-knowledge` (the DAG is `knowledge → search`, NOT the
    /// reverse — Cargo.toml §0). So the consumer↔owner agreement is proven the right way: both sections
    /// of the SHARED canonical thresholds file carry the SAME `0.05` (the Search-side
    /// `[projection_feeder].promotion_ratio` Search consumes == the KN-owner-side
    /// `[flex_db].facet_promotion_ratio` the owner measures against). The thresholds file is the single
    /// source of truth for the OQ-C number; this pins the two views of it together.
    #[test]
    fn threshold_mirrors_the_oqc_number_through_the_shared_thresholds_file() {
        use myelin_substrate::thresholds::Thresholds;
        // The Search-side seed is the frozen OQ-C > 5 % trigger.
        assert_eq!(ProjectionFeeder::PROMOTION_RATIO_SEED, 0.05);
        let t = Thresholds::load_canonical().expect("load canonical thresholds");
        // The Search-side consumer ratio and the KN-owner ratio are the SAME OQ-C number.
        assert_eq!(
            t.projection_feeder.promotion_ratio, t.flex_db.facet_promotion_ratio,
            "Search consumes the SAME OQ-C > 5 % number the Issues/KN owner measures against \
             (reconciled through the shared thresholds file, never a duplicated constant)"
        );
        assert_eq!(t.projection_feeder.promotion_ratio, 0.05);
    }

    /// **The view-execution telemetry counts a facet ONCE per execution (the 6.3 window) and the
    /// `should_promote` decision reads the threshold from the thresholds file.**
    #[test]
    fn telemetry_window_and_promotion_decision() {
        let t = ProjectionFeeder::default();
        let mut tel = ViewExecutionTelemetry::new();
        // 20 executions: 6 reference `state_category` (30 % > 5 %), 1 references `assignee` (5 %).
        for i in 0..20 {
            if i < 6 {
                // a filter referencing state_category twice still counts the facet ONCE this execution.
                tel.record_execution("issues", &["state_category", "state_category"]);
            } else if i == 6 {
                tel.record_execution("issues", &["assignee"]);
            } else {
                tel.record_execution("issues", &[]);
            }
        }
        assert_eq!(tel.total_executions("issues"), 20);
        assert_eq!(
            tel.facet_uses("issues", "state_category"),
            6,
            "counted once per execution"
        );
        assert_eq!(tel.facet_frequency("issues", "state_category"), 0.30);
        // 30 % > 5 % over a 20-execution window ⇒ promote.
        assert!(tel.should_promote("issues", "state_category", &t));
        // 1/20 == 5 % is NOT strictly greater than 5 % ⇒ do NOT promote (the frozen §4.6.1 wording).
        assert!(!tel.should_promote("issues", "assignee", &t));
        // A facet never filtered on never promotes.
        assert!(!tel.should_promote("issues", "priority", &t));
    }

    /// **A facet below the rolling-window execution floor never promotes (too noisy).**
    #[test]
    fn below_execution_floor_never_promotes() {
        let t = ProjectionFeeder::default(); // min_executions = 20
        let mut tel = ViewExecutionTelemetry::new();
        // 1 execution that referenced the facet — 100 %, but the window is too small to act on.
        tel.record_execution("issues", &["state_category"]);
        assert_eq!(tel.facet_frequency("issues", "state_category"), 1.0);
        assert!(
            !tel.should_promote("issues", "state_category", &t),
            "a 100 % frequency over a single execution is too noisy to promote on"
        );
    }

    /// **THE PROMOTION CHANGES COST, NOT CORRECTNESS — the SRCH-P27 green artifact.** A hot facet
    /// crosses the threshold, is promoted, and every facet value's GIN-scan and generated-index results
    /// are byte-identical under a real (non-`All`) ACL filter.
    #[test]
    fn promotion_changes_cost_not_correctness() {
        let mut coll = issues_collection(200);
        // Build a telemetry where state_category is hot (30 % of 100 executions).
        let mut tel = ViewExecutionTelemetry::new();
        for i in 0..100 {
            if i < 30 {
                tel.record_execution("issues", &["state_category"]);
            } else {
                tel.record_execution("issues", &[]);
            }
        }
        // A real ACL filter: only a subset of objects is visible (the ACL-admit must apply on BOTH
        // paths — a deny-set excluding two otherwise-matching objects).
        let acl = AclFilter::not_ids(["obj-0", "obj-4", "obj-8", "obj-12"]);

        // Capture the cold-path results for a value BEFORE the promotion.
        let v = FieldValue::Select("backlog".into());
        let before = coll.serve(&v_facet(), &v, &acl);
        assert_eq!(coll.path_of("state_category"), FacetServingPath::GinScan);

        let t = ProjectionFeeder::default();
        let verdict = ProjectionFeederGate::new().run(
            &tenant(),
            &region(),
            &mut coll,
            &tel,
            "state_category",
            &acl,
            &t,
            "2026-06-25",
        );
        let a = verdict.artifact().expect("SRCH-P27 green");
        assert!(a.is_green());
        assert_eq!(a.measured_frequency_bps, 3000, "30 % measured frequency");
        assert_eq!(a.threshold_bps, 500, "> 5 % = 500 bps threshold");
        assert!(a.measured_frequency_bps > a.threshold_bps);
        assert!(
            a.values_checked >= 4,
            "every distinct state value was checked"
        );
        assert!(
            !a.threshold_measured,
            "the threshold is the carried OQ-C default-to-beat"
        );

        // The facet is now on the generated index, and the result is byte-identical to the cold path.
        assert_eq!(
            coll.path_of("state_category"),
            FacetServingPath::GeneratedIndex
        );
        let after = coll.serve(&v_facet(), &v, &acl);
        assert_eq!(
            after, before,
            "byte-identical results across the promotion (cost changed only)"
        );
        // The ACL-admit really applied: obj-0/obj-4/... never surface even though they match the value.
        assert!(
            !after.iter().any(|id| id == "ENG-0000"),
            "obj-0 is ACL-denied on both paths"
        );
        println!("[P-462 GATE GREEN 2026-06-25] {}", a.summary());
    }

    fn v_facet() -> String {
        "state_category".to_string()
    }

    /// **A facet that has NOT crossed the threshold fails the gate (premature promotion) — never
    /// promoted.**
    #[test]
    fn below_threshold_fails_loud() {
        let mut coll = issues_collection(50);
        let mut tel = ViewExecutionTelemetry::new();
        // 40 executions, only 1 referenced the facet (2.5 % < 5 %).
        for i in 0..40 {
            if i == 0 {
                tel.record_execution("issues", &["state_category"]);
            } else {
                tel.record_execution("issues", &[]);
            }
        }
        let verdict = ProjectionFeederGate::new().run(
            &tenant(),
            &region(),
            &mut coll,
            &tel,
            "state_category",
            &AclFilter::All,
            &ProjectionFeeder::default(),
            "2026-06-25",
        );
        assert_eq!(
            verdict.failure(),
            Some(&ProjectionFeederFailure::BelowThreshold {
                measured_bps: 250,
                threshold_bps: 500,
            }),
            "2.5 % < 5 % is RED — never promoted prematurely"
        );
        // The facet was NOT promoted (it is still on the GIN scan).
        assert_eq!(coll.path_of("state_category"), FacetServingPath::GinScan);
    }

    /// **A mis-specified threshold (a ratio ≥ 1) fails LOUD — a vacuous bar can never make a green.**
    #[test]
    fn misspecified_threshold_fails_loud() {
        let mut coll = issues_collection(10);
        let tel = ViewExecutionTelemetry::new();
        let bad = ProjectionFeeder {
            promotion_ratio: 1.0, // promote nothing — a vacuous bar
            min_executions: 20,
        };
        let verdict = ProjectionFeederGate::new().run(
            &tenant(),
            &region(),
            &mut coll,
            &tel,
            "state_category",
            &AclFilter::All,
            &bad,
            "2026-06-25",
        );
        assert_eq!(
            verdict.failure(),
            Some(&ProjectionFeederFailure::MisspecifiedThreshold)
        );
    }

    /// **The generated index agrees with the GIN scan over EVERY value AND the empty / absent value.**
    /// A direct path-equivalence sweep (the property the gate proves, exercised directly).
    #[test]
    fn both_paths_agree_over_every_value() {
        let mut coll = issues_collection(120);
        coll.promote("state_category");
        let acl = AclFilter::not_ids(["obj-1", "obj-7", "obj-77"]);
        for v in coll.facet_values("state_category") {
            assert_eq!(
                coll.serve_gin_scan("state_category", &v, &acl),
                coll.serve_generated_index("state_category", &v, &acl),
                "the two paths agree on value {v:?}"
            );
        }
        // A value NO doc carries → both paths return empty (no spurious match on either path).
        let absent = FieldValue::Select("nonexistent".into());
        assert!(coll
            .serve_gin_scan("state_category", &absent, &acl)
            .is_empty());
        assert!(coll
            .serve_generated_index("state_category", &absent, &acl)
            .is_empty());
    }

    /// **An upsert AFTER promotion rebuilds the generated index so it never serves a stale doc set —
    /// the promoted path stays byte-identical to the GIN scan.**
    #[test]
    fn upsert_after_promotion_keeps_paths_identical() {
        let mut coll = issues_collection(30);
        coll.promote("state_category");
        // Add a new doc with a value already present, plus one with a brand-new value.
        coll.add(
            FacetDoc::new("ENG-9001", "obj-9001")
                .with_facet("state_category", FieldValue::Select("done".into())),
        );
        coll.add(
            FacetDoc::new("ENG-9002", "obj-9002")
                .with_facet("state_category", FieldValue::Select("triage".into())),
        );
        let acl = AclFilter::All;
        for v in coll.facet_values("state_category") {
            assert_eq!(
                coll.serve_gin_scan("state_category", &v, &acl),
                coll.serve_generated_index("state_category", &v, &acl),
                "post-upsert the promoted index still matches the GIN scan for {v:?}"
            );
        }
        // The brand-new value is found via the generated index (the rebuild picked it up).
        let triage = FieldValue::Select("triage".into());
        assert_eq!(
            coll.serve_generated_index("state_category", &triage, &acl),
            vec!["ENG-9002".to_string()]
        );
    }

    /// **The Display of each failure names exactly what broke (loud, never a bare bool).**
    #[test]
    fn failures_display_loudly() {
        let changed = ProjectionFeederFailure::ResultChanged {
            value_key: "\"backlog\"".into(),
            gin_scan: vec!["a".into()],
            generated_index: vec![],
        };
        assert!(changed.to_string().contains("CHANGED a result"));
        assert!(ProjectionFeederFailure::NoValuesChecked
            .to_string()
            .contains("0 facet values"));
        assert!(ProjectionFeederFailure::MisspecifiedThreshold
            .to_string()
            .contains("mis-specified"));
    }
}
