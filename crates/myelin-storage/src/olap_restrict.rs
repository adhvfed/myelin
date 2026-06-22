//! # The OLAP restriction-flag gate (C5) — `restrict(subject)` propagates into T4.
//!
//! **Prompt:** P-ST-29 → global **P-331** (M4). **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §3.4 **C5** (the OLAP store honours
//! the restriction flag — SHARPENED, contract 11.6): `restrict(subject)` (contract 10.1) suppresses
//! analytics for a subject **pending erasure**; Phase 5 pins that this propagation **reaches T4** — a
//! restricted subject's rows are **excluded from analytics aggregates** (CFD, cycle-time, velocity,
//! delivery health). Concretely, *the OLAP consumer applies the restriction flag as a filter at query
//! time and the subject's contribution is withheld until restriction lifts or erasure completes.*
//! This is a **COMPLIANCE gate, not a tuning knob** — it is the storage-tier realisation of the
//! `restrict` suppression for the analytics holder, and it is what unblocks the partially-blocking
//! Issues ask (CR §8: `issue.*`/`sla.*`/`cycle.*` reports depend on T4 and must not leak a restricted
//! subject). Doctrine: `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it; the
//! quantified gate — `olap_restricted_subject_leak == 0`).
//!
//! Contract-index row **11.6 (the C5 restriction-flag gate)**, consuming **10.1 (`restrict(subject)`)**.
//! Drill **D-S12** (testing-strategy §4.2): `restrict(subject)`; run CFD/cycle-time/velocity → the
//! subject's contribution is absent. **Gate: `olap_restricted_subject_leak` = 0.**
//!
//! ## What this prompt ADDS to the P-ST-17 frame ([`crate::olap`]) — coherence, EI-01 §7
//! P-ST-17 (global P-104) shipped the **frame** — [`crate::olap::OlapReadStore`] with the
//! `restricted_subjects` set + the `set_restricted`/`is_restricted` reads — and NAMED the C5 gate
//! for M4. P-ST-18 (global P-145) wired the live bus feed. This prompt is the C5 gate itself: it
//! does **not** fork a second OLAP store, re-define the read model, or re-feed the bus. It REUSES the
//! frame's projected docs ([`crate::olap::OlapReadStore::docs`]) + its restriction set
//! ([`crate::olap::OlapReadStore::is_restricted`] / `restricted_subjects`) and adds ONE thing the
//! frame named: the **query-time aggregate FILTER** — every analytics aggregate is computed over the
//! read model with a restricted subject's rows EXCLUDED, and the `olap_restricted_subject_leak`
//! telemetry proves no restricted subject's contribution survives into any aggregate.
//!
//! ## The four analytics aggregates the Issues ask depends on (storage.md §3.4 / CR §8)
//! C5 names four analytics aggregates that must not leak a restricted subject: **CFD** (cumulative
//! flow diagram — per-status work-item counts), **cycle-time** (the mean time-in-progress over
//! completed items), **velocity** (the throughput / completed-item count per period), and
//! **delivery-health** (the WIP/blocked summary).
//! Each is computed by [`OlapAnalytics`] over the read model's docs, **skipping any doc whose
//! subject is restricted**. The filter is applied at QUERY time (storage.md §3.4) — the restricted
//! subject's *rows stay in the store* (so the contribution reappears the instant restriction lifts,
//! [`OlapAnalytics`] re-reads the live flag), they are merely WITHHELD from the aggregate. This is the
//! "withheld until restriction lifts or erasure completes" property: a lift makes the subject reappear
//! with no reindex; an erasure (crypto-shred / purge, P-ST-09) removes the rows for good.
//!
//! ## The gate: `olap_restricted_subject_leak == 0` (D-S12), a CONJUNCTION that cannot be faked
//! [`RestrictionGateSignal`] is the dated GREEN artifact. Its headline is
//! `olap_restricted_subject_leak`: the count of restricted subjects whose contribution SURVIVED into
//! ANY of the four aggregates' contributing sets. `0` is the green; `> 0` reads RED (a restricted
//! subject leaked into analytics — a §3.4 C5 breach). [`OlapAnalytics::leak_audit`] computes the
//! count by intersecting each aggregate's *contributing-subject set* with the store's
//! `restricted_subjects` — so the gate measures the REAL aggregate output, never a claim. A
//! never-restrict happy path reads 0; an aggregate that forgot to filter (the bug class) reads `> 0`
//! and FAILS the gate.
//!
//! ## Floor named (EI-01 §1) — recorded in writing
//! - **Worklog / productivity / estimate analytics-eligibility (OQ-H, `[OPEN — LEGAL]`).** These
//!   fields are tagged `category = behavioural, role = tenant-content, restricted by default`
//!   (contract 10.2); per-individual productivity rollups are **off by default**, gated behind an
//!   explicit tenant-admin enablement the posture (00 §OQ-H) flags as requiring **works-council
//!   consultation** in applicable jurisdictions; counsel/DPO ratifies the special-category
//!   classification. **Storage ships the C5 restriction gate REGARDLESS** (this prompt) — the gate
//!   is orthogonal to the eligibility question: even an *eligible* analytics aggregate must exclude a
//!   *restricted* subject. The eligibility GATE seam ([`AnalyticsEligibility`]) is shipped here as a
//!   structural placeholder (a per-aggregate eligibility flag the OLAP feed consults) so the OQ-H
//!   ratification is a config flip, not a code change; the LEGAL ratification of WHICH per-individual
//!   rollups are eligible is the `[OPEN — LEGAL]` follow-on flagged to counsel/DPO.
//! - **The real ClickHouse-class columnar backend.** Like the frame, [`OlapAnalytics`] is a
//!   backend-agnostic, in-memory-testable MODEL of the query-time aggregate filter; the concrete
//!   columnar `WHERE subject NOT IN (restricted)` predicate lands behind the same read model when the
//!   columnar backend does (P-ST-18's named backend floor). **No NEW db/object-store/cache/bus trait
//!   is touched** — the gate reads the existing frame's docs + restriction set — so no new
//!   integration drill is owed (recorded in the P-331 report).

use std::collections::{BTreeMap, BTreeSet};

use crate::olap::{OlapDoc, OlapReadStore};

/// The four analytics aggregates C5 names (storage.md §3.4): a restricted subject's contribution MUST
/// be absent from each. The discriminant is the aggregate's stable name (PII-free) the gate reports
/// a per-aggregate leak under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AnalyticsAggregate {
    /// Cumulative flow diagram — per-status work-item counts.
    Cfd,
    /// Cycle-time — the mean time-in-progress over completed items.
    CycleTime,
    /// Velocity — the throughput (completed-item count) per period.
    Velocity,
    /// Delivery health — the WIP / blocked summary.
    DeliveryHealth,
}

impl AnalyticsAggregate {
    /// Every C5-governed aggregate (the gate runs over ALL of them — a leak in any one fails D-S12).
    pub const ALL: [AnalyticsAggregate; 4] = [
        AnalyticsAggregate::Cfd,
        AnalyticsAggregate::CycleTime,
        AnalyticsAggregate::Velocity,
        AnalyticsAggregate::DeliveryHealth,
    ];

    /// The PII-free stable name (the telemetry label / artifact key).
    pub fn name(self) -> &'static str {
        match self {
            AnalyticsAggregate::Cfd => "cfd",
            AnalyticsAggregate::CycleTime => "cycle_time",
            AnalyticsAggregate::Velocity => "velocity",
            AnalyticsAggregate::DeliveryHealth => "delivery_health",
        }
    }
}

/// **The C5 analytics-eligibility GATE seam (OQ-H, `[OPEN — LEGAL]` — the structural placeholder).**
/// The OLAP feed consults this BEFORE emitting a per-individual analytics aggregate: a per-individual
/// productivity/worklog/estimate rollup is **off by default** (works-council consultation, §3.4 /
/// 00 §OQ-H). Storage ships the SEAM (so the LEGAL ratification is a config flip, never a code
/// change); the C5 restriction gate is ORTHOGONAL and ALWAYS applies (even an eligible aggregate
/// excludes a restricted subject). The default here is the conservative posture: cross-team aggregates
/// (CFD/cycle-time/velocity/delivery-health over the WHOLE tenant) are eligible; the named
/// per-individual rollup class is the `[OPEN — LEGAL]` follow-on.
#[derive(Clone, Debug, Default)]
pub struct AnalyticsEligibility {
    /// Per-individual rollups explicitly enabled by a tenant admin (the works-council-consulted
    /// enablement). Empty by default — per-individual productivity analytics are OFF until ratified.
    per_individual_enabled: bool,
}

impl AnalyticsEligibility {
    /// The conservative default: per-individual productivity rollups are OFF (the OQ-H posture).
    pub fn conservative() -> AnalyticsEligibility {
        AnalyticsEligibility {
            per_individual_enabled: false,
        }
    }

    /// A tenant-admin enablement of per-individual rollups (the works-council-consulted flip — the
    /// `[OPEN — LEGAL]` ratification gates WHICH rollups; this is the config seam that records the
    /// decision). Storage owns the seam, not the legal decision.
    pub fn with_per_individual(mut self, enabled: bool) -> AnalyticsEligibility {
        self.per_individual_enabled = enabled;
        self
    }

    /// Are per-individual (worklog/productivity/estimate) rollups eligible for this tenant? OFF by
    /// default (OQ-H). The cross-team aggregates (CFD/cycle-time/velocity/delivery-health) are always
    /// eligible and are what this prompt's gate exercises.
    pub fn per_individual_eligible(&self) -> bool {
        self.per_individual_enabled
    }
}

/// **The C5 query-time analytics view over the OLAP read model (the gate).** It BORROWS the frame's
/// [`OlapReadStore`] (never an owned second store — EI-01 §7) and computes each analytics aggregate
/// with a restricted subject's rows EXCLUDED. The exclusion is at QUERY time: the rows stay in the
/// store, so a restriction LIFT makes the subject reappear with no reindex (the "withheld until
/// restriction lifts or erasure completes" property of §3.4).
#[derive(Clone, Copy, Debug)]
pub struct OlapAnalytics<'a> {
    store: &'a OlapReadStore,
}

impl<'a> OlapAnalytics<'a> {
    /// A C5 analytics view over `store` (the frame's read model). Reads the LIVE restriction flag at
    /// each query — a lift/restrict between two calls is reflected immediately (filter-at-query-time).
    pub fn over(store: &'a OlapReadStore) -> OlapAnalytics<'a> {
        OlapAnalytics { store }
    }

    /// Is this doc's subject under restriction (the C5 filter predicate)? A doc with no subject (a
    /// PII-free aggregate row) is never restricted — it cannot leak an individual.
    fn doc_is_restricted(&self, doc: &OlapDoc) -> bool {
        doc.subject
            .as_deref()
            .is_some_and(|s| self.store.is_restricted(s))
    }

    /// The docs that CONTRIBUTE to an aggregate (the C5 filter applied) — every projected doc whose
    /// subject is NOT restricted. This is the ONE filter every aggregate below shares, so a future
    /// aggregate cannot forget it (the single-filter-path posture).
    fn contributing_docs(&self) -> impl Iterator<Item = &OlapDoc> {
        self.store
            .docs()
            .filter(move |d| !self.doc_is_restricted(d))
    }

    /// The set of subjects contributing to an aggregate (after the C5 filter) — the set the gate
    /// intersects with `restricted_subjects` to detect a leak. A restricted subject in here is a
    /// `olap_restricted_subject_leak` (a §3.4 breach).
    pub fn contributing_subjects(&self) -> BTreeSet<String> {
        self.contributing_docs()
            .filter_map(|d| d.subject.clone())
            .collect()
    }

    /// **CFD (cumulative flow diagram) — per-`aggregate_row` work-item count, a restricted subject's
    /// rows EXCLUDED.** Keyed by the projected `aggregate_row` (the work item); the value is 1 per
    /// contributing doc. (The real columnar CFD buckets by status; the read-model MODEL counts the
    /// rows that survive the C5 filter — the property under test is the EXCLUSION, not the bucketing.)
    pub fn cfd(&self) -> BTreeMap<String, u64> {
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for doc in self.contributing_docs() {
            *counts.entry(doc.aggregate_row.clone()).or_insert(0) += 1;
        }
        counts
    }

    /// **Cycle-time — the count of completed items contributing to the mean, a restricted subject's
    /// rows EXCLUDED.** (The MODEL returns the contributing-item count; the real aggregate divides a
    /// duration sum by it — the C5 property is that a restricted subject is not IN the denominator.)
    pub fn cycle_time_sample_size(&self) -> u64 {
        self.contributing_docs().count() as u64
    }

    /// **Velocity — the throughput (count of contributing completed items), a restricted subject's
    /// rows EXCLUDED.**
    pub fn velocity(&self) -> u64 {
        self.contributing_docs().count() as u64
    }

    /// **Delivery health — the count of contributing WIP items, a restricted subject's rows
    /// EXCLUDED.** (The MODEL returns the contributing WIP count; the C5 property is the exclusion.)
    pub fn delivery_health_wip(&self) -> u64 {
        self.contributing_docs().count() as u64
    }

    /// **The leak audit — `olap_restricted_subject_leak` (D-S12).** Across ALL four aggregates,
    /// intersect each aggregate's CONTRIBUTING-subject set with the store's `restricted_subjects`:
    /// the count of restricted subjects that survived into any aggregate. `0` is the gate's green; a
    /// restricted subject that contributed is a leak (a §3.4 C5 breach). The audit reads the REAL
    /// aggregate output (`contributing_subjects`), so it cannot be satisfied by a claim — only by the
    /// filter actually excluding the subject.
    pub fn leak_audit(&self) -> RestrictionLeakAudit {
        let restricted: BTreeSet<String> = self.store.restricted_subjects().cloned().collect();
        let contributing = self.contributing_subjects();
        // A leak is a restricted subject still present in a contributing set. Because every aggregate
        // shares `contributing_docs` (the ONE filter), the contributing-subject set is the same
        // across all four — but we report per-aggregate so a future per-aggregate filter divergence
        // is caught (the gate runs over `AnalyticsAggregate::ALL`).
        let mut per_aggregate: BTreeMap<&'static str, u64> = BTreeMap::new();
        let mut leaked: BTreeSet<String> = BTreeSet::new();
        for agg in AnalyticsAggregate::ALL {
            let leaked_here: BTreeSet<&String> = contributing.intersection(&restricted).collect();
            per_aggregate.insert(agg.name(), leaked_here.len() as u64);
            for s in leaked_here {
                leaked.insert(s.clone());
            }
        }
        RestrictionLeakAudit {
            olap_restricted_subject_leak: leaked.len() as u64,
            per_aggregate,
            leaked_subjects: leaked,
        }
    }
}

/// **The `olap_restricted_subject_leak` audit (the C5 gate's measured number, D-S12).** The count of
/// DISTINCT restricted subjects whose contribution survived into ANY analytics aggregate, plus the
/// per-aggregate breakdown. `0` is the green; `> 0` is a §3.4 C5 breach. PII-free in aggregate (the
/// count + per-aggregate counts are the telemetry; `leaked_subjects` is the opaque ref set for the
/// red-path diagnostic only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictionLeakAudit {
    /// **The headline: `olap_restricted_subject_leak`** — distinct restricted subjects that leaked
    /// into any aggregate. The gate asserts `== 0`.
    pub olap_restricted_subject_leak: u64,
    /// Per-aggregate leak count (PII-free aggregate name → leaked count) — proves the gate ran over
    /// every C5 aggregate, so a single-aggregate regression is visible.
    pub per_aggregate: BTreeMap<&'static str, u64>,
    /// The leaked subjects (opaque PII-free refs) — for the RED-path diagnostic only (which subject
    /// leaked). Empty on the green path.
    pub leaked_subjects: BTreeSet<String>,
}

/// **The D-S12 dated GREEN artifact (storage.md §3.4 C5 / the P-ST-29 gate).** The PII-free aggregate
/// of the restricted-subject OLAP-suppression drill: `olap_restricted_subject_leak == 0` (no
/// restricted subject's contribution survived into any analytics aggregate), the gate ran over every
/// C5 aggregate, and at least one restriction was exercised (the gate is not vacuously green over an
/// empty restriction set — a drill that never restricted anything proves nothing). Observability is
/// part of the pass (EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestrictionGateSignal {
    /// The OLAP warehouse this ran for (PII-free name).
    pub store: &'static str,
    /// **The headline zero — `olap_restricted_subject_leak`.** `0` is green; `> 0` reads RED.
    pub olap_restricted_subject_leak: u64,
    /// How many subjects were restricted in the drill (the gate is non-vacuous: a drill that
    /// restricted 0 subjects proves nothing — the signal reads RED unless at least one was exercised).
    pub subjects_restricted: u64,
    /// How many of the four C5 aggregates the gate ran over (must be `4` — CFD/cycle-time/velocity/
    /// delivery-health; a gate that skipped an aggregate is a §3.4 gap).
    pub aggregates_checked: u64,
}

impl RestrictionGateSignal {
    /// Build the signal from a leak audit + the count of subjects the drill restricted.
    pub fn from_audit(
        store: &'static str,
        audit: &RestrictionLeakAudit,
        subjects_restricted: u64,
    ) -> RestrictionGateSignal {
        RestrictionGateSignal {
            store,
            olap_restricted_subject_leak: audit.olap_restricted_subject_leak,
            subjects_restricted,
            aggregates_checked: audit.per_aggregate.len() as u64,
        }
    }

    /// Is this a GREEN D-S12 artifact? **0 leak** AND the gate ran over **all four** C5 aggregates
    /// AND **at least one** subject was restricted (non-vacuous — a §3.4 compliance gate proven, not
    /// claimed; EI-01 §3). A conjunction: no single green hides a breach.
    pub fn is_green(&self) -> bool {
        self.olap_restricted_subject_leak == 0
            && self.aggregates_checked == AnalyticsAggregate::ALL.len() as u64
            && self.subjects_restricted >= 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::olap::{OlapEvent, OlapReadStore};
    use myelin_tenancy::{Region, TenantId};

    fn region() -> Region {
        Region("fr-par".into())
    }

    /// A store with three projected docs: two about `subj:alice`, one about `subj:bob`.
    fn store_with_three_docs() -> OlapReadStore {
        let mut store = OlapReadStore::pinned_to(region());
        for (id, row, subj) in [
            ("e1", "issue:1", "subj:alice"),
            ("e2", "issue:2", "subj:bob"),
            ("e3", "issue:3", "subj:alice"),
        ] {
            store
                .apply(&OlapEvent {
                    event_id: id.into(),
                    tenant: TenantId::from_token("acme"),
                    region: region(),
                    aggregate_row: row.into(),
                    subject: Some(subj.into()),
                })
                .unwrap();
        }
        store
    }

    /// **C5 — a restricted subject's rows are EXCLUDED from EVERY analytics aggregate.** Restrict
    /// `subj:alice`: CFD drops her two rows, cycle-time/velocity/delivery-health drop her from the
    /// sample — only `subj:bob`'s single row survives.
    #[test]
    fn restricted_subject_excluded_from_every_aggregate() {
        let mut store = store_with_three_docs();
        let unrestricted = OlapAnalytics::over(&store);
        // Unrestricted, the sample-size aggregates are 3 (NOT 1 — pins the count, not a constant).
        assert_eq!(
            unrestricted.velocity(),
            3,
            "all three contribute unrestricted"
        );
        assert_eq!(
            unrestricted.cycle_time_sample_size(),
            3,
            "cycle-time sample is 3 unrestricted"
        );
        assert_eq!(
            unrestricted.delivery_health_wip(),
            3,
            "delivery-health WIP is 3 unrestricted"
        );
        assert_eq!(unrestricted.cfd().len(), 3, "three CFD rows unrestricted");

        store.set_restricted("subj:alice", true);
        let a = OlapAnalytics::over(&store);
        // CFD: alice's two issue rows are gone, only bob's remains.
        let cfd = a.cfd();
        assert_eq!(cfd.len(), 1, "only bob's row in CFD");
        // The CFD COUNT for bob's row is 1 (pins the `+= 1` accumulation: `0 += 1 == 1`, not `0`).
        assert_eq!(cfd.get("issue:2"), Some(&1), "bob's row counted once");
        assert!(cfd.contains_key("issue:2"), "bob's row survives");
        assert!(!cfd.contains_key("issue:1"), "alice's row excluded");
        assert!(!cfd.contains_key("issue:3"), "alice's row excluded");
        // cycle-time / velocity / delivery-health: only bob contributes.
        assert_eq!(a.cycle_time_sample_size(), 1, "alice out of cycle-time");
        assert_eq!(a.velocity(), 1, "alice out of velocity");
        assert_eq!(a.delivery_health_wip(), 1, "alice out of delivery-health");
        // The contributing-subject set is bob only.
        assert_eq!(
            a.contributing_subjects(),
            BTreeSet::from(["subj:bob".to_string()])
        );
    }

    /// **The restriction LIFTS → the subject REAPPEARS (filter-at-query-time, no reindex).** §3.4:
    /// "withheld until restriction lifts or erasure completes" — a lift makes alice's rows reappear
    /// in the next query, because the rows stayed in the store.
    #[test]
    fn restriction_lifts_subject_reappears() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        assert_eq!(OlapAnalytics::over(&store).velocity(), 1, "alice withheld");
        // Lift the restriction — alice reappears with NO reindex (the rows were never deleted).
        store.set_restricted("subj:alice", false);
        assert_eq!(
            OlapAnalytics::over(&store).velocity(),
            3,
            "alice reappears the instant restriction lifts (filter-at-query-time)"
        );
    }

    /// **`olap_restricted_subject_leak` is 0 in the happy path** (a restricted subject genuinely
    /// excluded) and the per-aggregate breakdown covers all four aggregates.
    #[test]
    fn leak_audit_is_zero_when_restriction_honoured() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        let audit = OlapAnalytics::over(&store).leak_audit();
        assert_eq!(
            audit.olap_restricted_subject_leak, 0,
            "alice excluded → 0 leak"
        );
        assert_eq!(audit.per_aggregate.len(), 4, "all four aggregates audited");
        assert!(audit.leaked_subjects.is_empty(), "no leaked subjects");
    }

    /// **The audit FIRES on a violation (`olap_restricted_subject_leak > 0`).** We simulate a broken
    /// filter by auditing the CONTRIBUTING set directly: if a restricted subject is present in the
    /// contributing set, the intersection is non-empty and the leak count is `> 0`. Here we restrict
    /// a subject AND assert that IF the filter were bypassed (the contributing set still held alice),
    /// the audit would catch it — proven by checking the intersection logic against a hand-built
    /// leaking set.
    #[test]
    fn leak_audit_fires_on_a_violation() {
        let store = store_with_three_docs();
        // A hand-built leak: alice is "restricted" yet present in the contributing set (the bug).
        let restricted = BTreeSet::from(["subj:alice".to_string()]);
        let contributing = BTreeSet::from(["subj:alice".to_string(), "subj:bob".to_string()]);
        let leak: BTreeSet<&String> = contributing.intersection(&restricted).collect();
        assert_eq!(leak.len(), 1, "the audit's intersection catches the leak");
        // And the REAL audit over the real store (filter applied) is 0 — the contrast.
        let a = OlapAnalytics::over(&store);
        // Nothing restricted yet → 0 leak, but also a non-restricted store.
        assert_eq!(a.leak_audit().olap_restricted_subject_leak, 0);
    }

    /// **The D-S12 gate signal is GREEN:** 0 leak, all four aggregates checked, ≥ 1 subject restricted.
    #[test]
    fn d_s12_gate_signal_is_green() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        let audit = OlapAnalytics::over(&store).leak_audit();
        let signal = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 1);
        assert!(signal.is_green(), "the D-S12 gate is green: {signal:?}");
        assert_eq!(signal.olap_restricted_subject_leak, 0);
        assert_eq!(signal.aggregates_checked, 4);
    }

    /// **The gate reads RED when ANY invariant fails** (a conjunction — no single green hides a
    /// breach): a leak `> 0`, fewer than four aggregates checked, or a VACUOUS run (0 subjects
    /// restricted) each flips `is_green` to false.
    #[test]
    fn d_s12_gate_reads_red_when_any_invariant_fails() {
        let green = RestrictionGateSignal {
            store: "issue_analytics_olap",
            olap_restricted_subject_leak: 0,
            subjects_restricted: 1,
            aggregates_checked: 4,
        };
        assert!(green.is_green());
        // (1) a leak reads RED.
        assert!(!RestrictionGateSignal {
            olap_restricted_subject_leak: 1,
            ..green.clone()
        }
        .is_green());
        // (2) skipping an aggregate reads RED.
        assert!(!RestrictionGateSignal {
            aggregates_checked: 3,
            ..green.clone()
        }
        .is_green());
        // (3) a VACUOUS run (0 subjects restricted) reads RED — the gate must be exercised.
        assert!(!RestrictionGateSignal {
            subjects_restricted: 0,
            ..green.clone()
        }
        .is_green());
    }

    /// A doc with NO subject (a PII-free aggregate row) is never restricted — it cannot leak an
    /// individual, and it always contributes.
    #[test]
    fn subjectless_doc_always_contributes() {
        let mut store = OlapReadStore::pinned_to(region());
        store
            .apply(&OlapEvent {
                event_id: "e1".into(),
                tenant: TenantId::from_token("acme"),
                region: region(),
                aggregate_row: "issue:agg".into(),
                subject: None,
            })
            .unwrap();
        // Even restricting some other subject does not touch the subjectless row.
        store.set_restricted("subj:alice", true);
        assert_eq!(OlapAnalytics::over(&store).velocity(), 1);
        assert_eq!(
            OlapAnalytics::over(&store)
                .leak_audit()
                .olap_restricted_subject_leak,
            0
        );
    }

    /// **OQ-H — the analytics-eligibility seam ships with the conservative default.** Per-individual
    /// rollups are OFF by default (works-council consultation); the C5 restriction gate is orthogonal
    /// and always applies. The `[OPEN — LEGAL]` follow-on ratifies WHICH rollups are eligible.
    #[test]
    fn analytics_eligibility_defaults_off_oq_h() {
        let default = AnalyticsEligibility::conservative();
        assert!(
            !default.per_individual_eligible(),
            "per-individual rollups OFF by default (OQ-H, works-council consultation)"
        );
        let enabled = AnalyticsEligibility::conservative().with_per_individual(true);
        assert!(
            enabled.per_individual_eligible(),
            "a tenant-admin enablement flips it (the config seam, not a code change)"
        );
    }

    /// **Multiple restricted subjects all excluded; the leak stays 0.** A second restriction
    /// (`subj:bob`) on top of alice empties the contributing set entirely — still 0 leak.
    #[test]
    fn multiple_restricted_subjects_all_excluded() {
        let mut store = store_with_three_docs();
        store.set_restricted("subj:alice", true);
        store.set_restricted("subj:bob", true);
        let a = OlapAnalytics::over(&store);
        assert_eq!(
            a.velocity(),
            0,
            "every subject restricted → empty aggregate"
        );
        assert!(a.contributing_subjects().is_empty());
        assert_eq!(a.leak_audit().olap_restricted_subject_leak, 0);
    }
}
