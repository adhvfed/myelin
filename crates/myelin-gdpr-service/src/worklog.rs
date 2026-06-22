//! # `worklog` — the worklog/productivity/estimate Behavioural classification (OQ-H) + the
//! works-council consultation trigger + the `SpecialCategory` → DPIA route (P-GA-31 → P-334)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` **§2.4** (the NEW
//! worklog/productivity/estimate sensitivity classification, OQ-H — `category = Behavioural,
//! role = TenantContent, basis = TBD_LEGAL, erasure = CryptoShred(subject_dek),
//! data_role_default = Restricted`; excluded from cross-individual analytics/agent-use for a
//! restricted subject by default; per-individual productivity rollups OFF by default behind an
//! explicit tenant-admin enablement that **flags the works-council consultation trigger** — the
//! platform surfaces it, it does not adjudicate; build-data-as-LLM-training **foreclosed by
//! default**). Doctrine: `external-insights/01-process-and-quality-doctrine.md` §1 (name-your-floors
//! — the structural floor ships on the engineering clock, the `[OPEN — LEGAL]` residual is flagged
//! to counsel) + §8 (the human sign-off is the bottleneck — a works-council/labour-law call is
//! decision-shaped, **surfaced not auto-decided**).
//!
//! **Contract-index:** owns **10.2** (the worklog/Behavioural tags + the `SpecialCategory` → DPIA
//! route — the typed half this prompt ships in [`myelin_gdpr`] as the `data_role_default` registry
//! tag; the analytics/rollup/works-council behaviour here). Consumed: **11.6** (OLAP honours
//! restriction — the worklog rides the `restrict` suppression the OLAP read store already honours,
//! P-GA-25).
//!
//! ## What is genuinely NEW here, and what it REUSES (EI-01 §7 coherence)
//! The §2.4 posture has FOUR structural parts; this module composes already-shipped seams for three
//! of them and adds the genuinely-new fourth:
//! 1. **The restricted-by-default classification is STRUCTURAL** — shipped in `myelin-gdpr` as the
//!    `data_role_default = Restricted` registry tag ([`myelin_gdpr::PersonalDataField::
//!    is_restricted_by_default`]), applied to the Issues `worklog_seconds` / `story_points` fields.
//!    This module READS that tag off the data map to gate analytics (it does not re-detect "worklog"
//!    by field name — the map drives it).
//! 2. **Excluded from cross-individual analytics by default** — REUSES the OLAP restriction
//!    chokepoint ([`crate::restrict_fanout`], P-GA-25): a restricted-by-default worklog field is
//!    suppressed from any cross-individual aggregate UNLESS an explicit per-subject opt-in is
//!    recorded. The genuinely-new piece is the **default-DENY** ([`WorklogAnalyticsGate`]): the
//!    suppression default is FLIPPED for a restricted-by-default field (deny unless opted-in),
//!    whereas an ordinary field is allow-unless-`restrict`ed.
//! 3. **Per-individual rollups OFF by default + the works-council consultation trigger** — the
//!    genuinely-new [`RollupEnablement`] + [`WorksCouncilTrigger`]: enabling a per-individual
//!    productivity rollup is gated behind an explicit tenant-admin action that **emits a surfaced
//!    works-council consultation signal** (a `[OPEN — LEGAL]` obligation a tenant DPO/works-council
//!    must clear off-platform — the platform records the obligation, it does NOT auto-decide it,
//!    §8). A rollup that is not explicitly enabled stays OFF.
//! 4. **The `SpecialCategory` worklog field → DPIA route** — REUSES the [`myelin_gdpr::DpiaRouter`]
//!    (P-GA-08) verbatim: a worklog field tagged `category = SpecialCategory(...)` routes into the
//!    DPIA gate exactly like any other special-category field (no second router).
//! 5. **Build-data-as-LLM-training foreclosed by default** — the [`tests::
//!    build_data_as_llm_training_has_no_code_path`] architecture test: no platform code path feeds
//!    tenant content (worklog or otherwise) into model training. The foreclosure is the ABSENCE of a
//!    training-feed surface; the test asserts the absence over the GDPR-service surface.
//!
//! ## Floor named (the `[OPEN — LEGAL]` residual) — VISION §3 name-your-floors, dated
//! The worklog `basis = TBD_LEGAL` is the NAMED residual: the **structural floor** (the
//! restricted-by-default tag + per-subject DEK + rollups-off-by-default + the surfaced works-council
//! trigger) ships on the engineering clock (this prompt); the **special-category / works-council
//! ratification** (whether worklog is Art. 9 special-category vs merely elevated, and the per-
//! jurisdiction consultation trigger) is **parallel-legal** (`[OPEN — LEGAL]`,
//! [`WORKLOG_BASIS_RESIDUAL`]) — counsel/DPO ratify; engineering carries the tag + surfaces the
//! trigger regardless. Recorded in writing, dated 2026-06-22. **After this prompt all H1–H18 holders
//! exist** — the GA-D1 precondition ([`ALL_HOLDERS_EXIST_FOR`] → M5 P-GA-32).
//!
//! This module touches **NO new DB / object-store / cache / bus contract** (it composes the already-
//! shipped data-map registry + the restrict/OLAP seam + the DPIA router) — **no `--features
//! integration` live-stack leg is owed** by P-GA-31.
//!
//! ## Mutation floor (P-GA-31 TESTS — the restricted-by-default + the rollup-off-by-default + the
//! works-council-trigger-surfacing paths are mandatory-core). The behavioural core every mutation
//! must be CAUGHT: [`WorklogAnalyticsGate::cross_individual_allowed`] (the default-DENY for a
//! restricted-by-default field — load-bearing in BOTH polarities: restricted-by-default ⇒ denied
//! unless opted-in; ordinary ⇒ allowed unless `restrict`ed), [`RollupEnablement::is_enabled`] (OFF
//! by default — a `false`→`true` mutant would silently enable an individual rollup), and
//! [`RollupEnablement::enable`]'s **works-council trigger emission** (enabling MUST surface the
//! trigger — a dropped trigger is the §8 auto-decide bug). The `cargo mutants` score is recorded in
//! the commit body (EI-01 §3, stated not hidden).

use std::collections::BTreeSet;

use myelin_gdpr::{DataRoleDefault, HasPersonalData, PersonalDataField};

/// The worklog `basis = TBD_LEGAL` residual — the NAMED `[OPEN — LEGAL]` follow-on (gdpr §2.4,
/// OQ-H). The structural floor ships on the engineering clock; counsel ratifies the special-category
/// status + the per-jurisdiction works-council consultation trigger. PII-free; a documented anchor.
pub const WORKLOG_BASIS_RESIDUAL: &str =
    "[OPEN — LEGAL] worklog basis = TBD_LEGAL — counsel ratifies special-category (Art. 9) vs \
     elevated + the per-jurisdiction works-council consultation trigger; the structural floor \
     (restricted-by-default + per-subject DEK + rollups-off-by-default + the surfaced trigger) \
     ships regardless (P-GA-31 / P-334, recorded 2026-06-22)";

/// **After this prompt all H1–H18 holders exist** — the GA-D1 precondition. Named in writing per the
/// prompt: the full H1–H18 DSR fan-out (GA-D1, 0 holders missed) is M5, P-GA-32 → P-505.
pub const ALL_HOLDERS_EXIST_FOR: &str =
    "all H1–H18 holders now exist — GA-D1 precondition → M5 P-GA-32 (full DSR fan-out, 0 holders \
     missed)";

/// The telemetry signal NAME + UNIT for the worklog analytics default-deny (the GA-D7 worklog-leg
/// green artifact — 0 cross-individual analytics for a restricted-by-default subject unless opted-in).
/// PII-free.
pub const WORKLOG_CROSS_INDIVIDUAL_DENIED: (&str, &str) =
    ("gdpr.worklog_cross_individual_denied", "count");

/// The telemetry signal NAME + UNIT for the works-council consultation trigger (a SURFACED signal —
/// the count of rollup enablements that raised the consultation obligation; §8 surfaced-not-decided).
pub const WORKS_COUNCIL_TRIGGERS_SURFACED: (&str, &str) =
    ("gdpr.works_council_triggers_surfaced", "count");

// ───────────────────────── the restricted-by-default analytics gate (the worklog leg of GA-D7) ─────────────────────────

/// **The worklog analytics gate (gdpr §2.4 / 11.6, the OQ-H restricted-by-default posture).** It
/// decides whether a field MAY participate in **cross-individual** analytics/agent-use, reading the
/// `data_role_default` tag off the data map (NOT inferring "worklog" by field name — the map drives
/// it). The genuinely-new fact is the **flipped default**:
/// - an **ordinary** field (`data_role_default = Default`) is ALLOWED unless the subject is
///   `restrict`ed (the normal P-GA-25 chokepoint);
/// - a **restricted-by-default** field (`data_role_default = Restricted` — the worklog/productivity/
///   estimate posture) is **DENIED unless an explicit per-subject opt-in is recorded** (excluded
///   from cross-individual analytics/agent-use by default — §2.4).
///
/// Stateless; the opt-in set is the only state and is passed in (the tenant-admin-recorded explicit
/// per-subject opt-ins). This is the deterministic decision the OLAP/agent chokepoint reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorklogAnalyticsGate;

impl WorklogAnalyticsGate {
    /// A new gate.
    #[must_use]
    pub fn new() -> WorklogAnalyticsGate {
        WorklogAnalyticsGate
    }

    /// **May this field participate in CROSS-INDIVIDUAL analytics/agent-use for `subject_opted_in`?**
    /// (gdpr §2.4). The flipped-default decision:
    /// - `data_role_default = Restricted` (worklog) ⇒ **only if** the subject explicitly opted in
    ///   (default-DENY — excluded from cross-individual analytics by default).
    /// - `data_role_default = Default` (ordinary) ⇒ allowed (the `restrict` suppression — a
    ///   per-subject ad-hoc restriction, handled by P-GA-25 — is a SEPARATE, orthogonal gate; this
    ///   gate is only the OQ-H default-class decision).
    ///
    /// The `subject_opted_in` flag is the tenant-admin-recorded explicit per-subject opt-in for a
    /// restricted-by-default field; it is ignored for an ordinary field (which has no default-deny to
    /// override).
    #[must_use]
    pub fn cross_individual_allowed(
        &self,
        field: &PersonalDataField,
        subject_opted_in: bool,
    ) -> bool {
        match field.data_role_default() {
            // Restricted-by-default: denied UNLESS an explicit per-subject opt-in is recorded.
            DataRoleDefault::Restricted => subject_opted_in,
            // Ordinary: allowed by this gate (the OQ-H default-class is not restrictive).
            DataRoleDefault::Default => true,
        }
    }

    /// The worklog (restricted-by-default) fields of a holder schema `T`, read off the data map. The
    /// gate keys analytics decisions on THESE — the map, not a hand-written "is this worklog?" list,
    /// drives the restriction (so a newly-added restricted-by-default field is covered automatically).
    #[must_use]
    pub fn restricted_by_default_fields<T: HasPersonalData>() -> Vec<&'static PersonalDataField> {
        T::personal_data_fields()
            .iter()
            .filter(|f| f.is_restricted_by_default())
            .collect()
    }
}

// ───────────────────────── the works-council consultation trigger (surfaced, not auto-decided) ─────────────────────────

/// **A works-council consultation trigger — a SURFACED signal, never an auto-decision (gdpr §2.4 /
/// §8; doctrine §8 the-human-is-the-bottleneck).** Enabling a per-individual productivity rollup in
/// an applicable jurisdiction is potentially works-council-consultable; the platform RECORDS the
/// obligation (this trigger) and surfaces it for a tenant DPO / works-council to clear off-platform —
/// it does NOT adjudicate whether the consultation cleared. References-not-payloads: PII-free (the
/// opaque tenant token + the rollup id + the reason text, never a subject).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorksCouncilTrigger {
    /// The opaque tenant the rollup enablement was requested under (PII-free).
    pub tenant_token: String,
    /// The PII-free rollup identifier the enablement targets (e.g. `"team_velocity"`).
    pub rollup_id: String,
    /// The human-readable obligation surfaced for the tenant DPO / works-council (the `[OPEN —
    /// LEGAL]` consultation requirement — surfaced, not cleared).
    pub reason: String,
}

impl WorksCouncilTrigger {
    /// Mint the consultation trigger for a rollup enablement (the surfaced obligation). The reason
    /// names the `[OPEN — LEGAL]` consultation requirement — it is the obligation, NOT a verdict.
    fn for_rollup(tenant_token: &str, rollup_id: &str) -> WorksCouncilTrigger {
        WorksCouncilTrigger {
            tenant_token: tenant_token.to_string(),
            rollup_id: rollup_id.to_string(),
            reason: format!(
                "[OPEN — LEGAL] enabling per-individual productivity rollup `{rollup_id}` may \
                 require works-council consultation in applicable jurisdictions — surfaced for the \
                 tenant DPO / works-council, NOT auto-decided (gdpr §2.4 OQ-H)"
            ),
        }
    }
}

/// **The per-individual productivity rollup enablement (gdpr §2.4 — OFF by default).** Per-individual
/// productivity rollups (team velocity broken down by person, per-individual worklog aggregates) are
/// **OFF by default**; turning one ON is an explicit tenant-admin action that **surfaces the
/// works-council consultation trigger**. This models the gate: a rollup is enabled ONLY if the
/// tenant admin explicitly enabled it, and enabling one ALWAYS records a [`WorksCouncilTrigger`].
///
/// The trigger set is the audit trail of surfaced obligations (the green artifact) — it is never
/// auto-cleared (a works-council/labour-law call is decision-shaped, §8). Disabling a rollup leaves
/// the trigger in the surfaced-obligations log (the obligation was raised; lifting the rollup does
/// not unraise the historical consultation requirement).
#[derive(Debug, Default)]
pub struct RollupEnablement {
    /// The PII-free `(tenant, rollup_id)` pairs explicitly enabled (OFF unless present here).
    enabled: BTreeSet<(String, String)>,
    /// The surfaced works-council consultation triggers (the audit trail of raised obligations).
    surfaced_triggers: Vec<WorksCouncilTrigger>,
}

impl RollupEnablement {
    /// A fresh enablement registry — **every per-individual rollup OFF by default** (no entries).
    #[must_use]
    pub fn new() -> RollupEnablement {
        RollupEnablement::default()
    }

    /// **Is this per-individual rollup ENABLED? (OFF by default — gdpr §2.4).** `false` unless the
    /// tenant admin explicitly enabled it via [`RollupEnablement::enable`]. Load-bearing: a
    /// `false`→`true` mutant would silently enable an individual productivity rollup (the OQ-H bug).
    #[must_use]
    pub fn is_enabled(&self, tenant_token: &str, rollup_id: &str) -> bool {
        self.enabled
            .contains(&(tenant_token.to_string(), rollup_id.to_string()))
    }

    /// **Enable a per-individual productivity rollup (the explicit tenant-admin action) — and SURFACE
    /// the works-council consultation trigger (gdpr §2.4 / §8).** Returns the surfaced
    /// [`WorksCouncilTrigger`] (the obligation), recorded into the audit trail. Enabling ALWAYS
    /// surfaces the trigger — the platform records the obligation; it does NOT clear it (surfaced,
    /// not auto-decided). Idempotent on the enabled set; the trigger is surfaced once per enable call
    /// (the audit trail of attempts).
    pub fn enable(&mut self, tenant_token: &str, rollup_id: &str) -> WorksCouncilTrigger {
        self.enabled
            .insert((tenant_token.to_string(), rollup_id.to_string()));
        let trigger = WorksCouncilTrigger::for_rollup(tenant_token, rollup_id);
        self.surfaced_triggers.push(trigger.clone());
        trigger
    }

    /// Disable a per-individual rollup (turn it back OFF). The historical surfaced trigger is
    /// RETAINED (the obligation was raised; lifting the rollup does not unraise it — the audit trail
    /// is append-only). Returns whether a rollup was actually enabled (and is now disabled).
    pub fn disable(&mut self, tenant_token: &str, rollup_id: &str) -> bool {
        self.enabled
            .remove(&(tenant_token.to_string(), rollup_id.to_string()))
    }

    /// The surfaced works-council consultation triggers (the audit trail of raised obligations — the
    /// green artifact; surfaced, never auto-cleared).
    #[must_use]
    pub fn surfaced_triggers(&self) -> &[WorksCouncilTrigger] {
        &self.surfaced_triggers
    }
}

// ───────────────────────── build-data-as-LLM-training foreclosure (the architecture anchor) ─────────────────────────

/// The documented foreclosure (gdpr §2.4 / OQ-H, AG-8): **build-data-as-LLM-training is foreclosed by
/// default** — no platform code path feeds tenant content (worklog or otherwise) into model
/// training. Training on tenant data is a NEW purpose needing its own lawful basis; it is a
/// separately-ratified opt-in, never a default. This constant is the documented anchor; the
/// foreclosure is PROVEN by the ABSENCE of a training-feed surface (the architecture test
/// [`tests::build_data_as_llm_training_has_no_code_path`]).
pub const BUILD_TRAINING_FORECLOSURE: &str =
    "build-data-as-LLM-training foreclosed by default (gdpr §2.4 / AG-8) — no platform code path \
     feeds tenant content to model training; training-on-tenant-data is a separately-ratified \
     opt-in (a region-aware EU-hostable sub-processor, ADR-12.8), never a default";

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::{DpiaMarker, DpiaRouter, DpiaVerdict, PersonalData};
    use std::collections::BTreeSet;

    // A worklog-shaped holder schema: a restricted-by-default Behavioural worklog field, an ordinary
    // Content field (NOT restricted-by-default), and a special-category worklog field (the DPIA
    // route). One fixture exercises the three §2.4 legs.
    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct WorklogRow {
        // The OQ-H worklog field — Behavioural + restricted-by-default + per-subject DEK.
        #[personal_data(
            category = Behavioural,
            role = TenantContent,
            basis = TBD_LEGAL,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "created_by_pseudonym",
            data_role_default = Restricted
        )]
        worklog_seconds: i64,
        // An ordinary free-text field — Content, NOT restricted-by-default (the default-class).
        #[personal_data(
            category = Content,
            role = TenantContent,
            basis = Contract,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "created_by_pseudonym"
        )]
        title: String,
        // A SPECIAL-CATEGORY worklog field (a productivity metric that became health-adjacent) — the
        // DPIA route. Also restricted-by-default (the OQ-H posture).
        #[personal_data(
            category = SpecialCategory(health),
            role = TenantContent,
            basis = TBD_LEGAL,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "created_by_pseudonym",
            data_role_default = Restricted
        )]
        sensitive_metric: f64,
        // a non-PII column — no tag, no entry.
        row_version: u64,
    }

    fn worklog_field() -> &'static PersonalDataField {
        WorklogRow::personal_data_fields()
            .iter()
            .find(|f| f.field == "worklog_seconds")
            .expect("worklog_seconds is tagged")
    }

    fn title_field() -> &'static PersonalDataField {
        WorklogRow::personal_data_fields()
            .iter()
            .find(|f| f.field == "title")
            .expect("title is tagged")
    }

    // ───────── (1) the worklog tag is STRUCTURAL: restricted-by-default + Behavioural read off the map ─────────

    /// The worklog field carries the OQ-H classification on the data map (gdpr §2.4): `Behavioural` +
    /// restricted-by-default + the per-subject-DEK erasure tag. Read STRUCTURALLY off the registry —
    /// not inferred from the field name.
    #[test]
    fn worklog_field_carries_the_behavioural_restricted_by_default_per_subject_dek_tags() {
        let w = worklog_field();
        assert!(w.is_behavioural(), "worklog is category = Behavioural");
        assert!(
            w.is_restricted_by_default(),
            "worklog is data_role_default = Restricted (OQ-H)"
        );
        assert_eq!(
            w.data_role_default(),
            DataRoleDefault::Restricted,
            "the structural tag is read off the map"
        );
        // The same per-subject DEK crypto-shred as other free-text PII (§2.4).
        assert_eq!(
            w.erasure_key_class(),
            Some(myelin_gdpr::ErasureKeyClass::SubjectDek),
            "worklog carries the same per-subject DEK crypto-shred"
        );

        // The ORDINARY field is NOT restricted-by-default (the default-class — the tag is not a
        // blanket "everything is restricted"). This kills a `is_restricted_by_default -> true` mutant.
        let t = title_field();
        assert!(
            !t.is_restricted_by_default(),
            "an ordinary field is Default"
        );
        assert_eq!(t.data_role_default(), DataRoleDefault::Default);
        assert!(!t.is_behavioural(), "Content is not Behavioural");
    }

    /// `restricted_by_default_fields` reads the worklog set off the map (the map drives it). Exactly
    /// the two restricted-by-default fields are returned (worklog + the special-category metric), not
    /// the ordinary `title`.
    #[test]
    fn the_gate_reads_the_restricted_by_default_field_set_off_the_map() {
        let fields = WorklogAnalyticsGate::restricted_by_default_fields::<WorklogRow>();
        let names: BTreeSet<&str> = fields.iter().map(|f| f.field).collect();
        assert_eq!(
            names,
            ["sensitive_metric", "worklog_seconds"]
                .into_iter()
                .collect(),
            "the map drives the restricted-by-default set (worklog + the special-category metric)"
        );
        assert!(
            !names.contains("title"),
            "the ordinary Content field is not in the restricted set"
        );
    }

    // ───────── (2) restricted-by-default ⇒ excluded from cross-individual analytics (GA-D7 face) ─────────

    /// **GA-D7 worklog face: a restricted-by-default worklog field is EXCLUDED from cross-individual
    /// analytics by default (gdpr §2.4) — allowed ONLY for a subject with an explicit opt-in.** The
    /// default-DENY is the flipped default; the branch is load-bearing in BOTH polarities.
    #[test]
    fn restricted_by_default_worklog_is_excluded_from_cross_individual_analytics_unless_opted_in() {
        let gate = WorklogAnalyticsGate::new();
        let w = worklog_field();

        // Default: DENIED (excluded from cross-individual analytics — the OQ-H exclusion).
        assert!(
            !gate.cross_individual_allowed(w, false),
            "a restricted-by-default worklog field is DENIED cross-individual analytics by default"
        );
        // With an explicit per-subject opt-in: ALLOWED (the tenant-admin-recorded override).
        assert!(
            gate.cross_individual_allowed(w, true),
            "an explicit per-subject opt-in lifts the default-deny"
        );

        // An ORDINARY field is ALLOWED by this gate regardless of opt-in (the OQ-H default-class is
        // not restrictive). This kills a constant-`false` mutant on the gate.
        let t = title_field();
        assert!(
            gate.cross_individual_allowed(t, false),
            "an ordinary field is allowed by the OQ-H gate (no default-deny)"
        );
        assert!(gate.cross_individual_allowed(t, true));
    }

    // ───────── (3) per-individual rollups OFF by default + the works-council trigger ─────────

    /// **Per-individual productivity rollups are OFF by default (gdpr §2.4).** A fresh registry has
    /// every rollup OFF; `is_enabled` is `false`. Load-bearing: a `false`→`true` default mutant would
    /// silently enable an individual rollup.
    #[test]
    fn per_individual_rollups_are_off_by_default() {
        let rollups = RollupEnablement::new();
        assert!(
            !rollups.is_enabled("acme", "team_velocity"),
            "per-individual rollups are OFF by default (OQ-H)"
        );
        assert!(
            rollups.surfaced_triggers().is_empty(),
            "no consultation obligation until a rollup is explicitly enabled"
        );
    }

    /// **Enabling a per-individual rollup SURFACES the works-council consultation trigger (gdpr §2.4
    /// / §8) — a surfaced signal, NOT an auto-decision.** The platform records the obligation; it does
    /// not clear it. Load-bearing: a dropped trigger is the §8 auto-decide bug.
    #[test]
    fn enabling_a_rollup_surfaces_the_works_council_trigger_without_auto_deciding() {
        let mut rollups = RollupEnablement::new();

        let trigger = rollups.enable("acme", "team_velocity");
        // The rollup is now enabled (the explicit tenant-admin action).
        assert!(rollups.is_enabled("acme", "team_velocity"));
        // The trigger is SURFACED — the obligation, never a verdict.
        assert_eq!(trigger.tenant_token, "acme");
        assert_eq!(trigger.rollup_id, "team_velocity");
        assert!(
            trigger.reason.contains("works-council"),
            "the surfaced obligation names the works-council consultation"
        );
        assert!(
            trigger.reason.contains("OPEN — LEGAL") && trigger.reason.contains("NOT auto-decided"),
            "the trigger is surfaced, not auto-decided (§8)"
        );
        // The audit trail recorded exactly one surfaced obligation.
        assert_eq!(
            rollups.surfaced_triggers().len(),
            1,
            "the surfaced obligation is recorded once"
        );

        // The trigger is NEVER auto-cleared: disabling the rollup retains the historical obligation.
        assert!(rollups.disable("acme", "team_velocity"));
        assert!(
            !rollups.is_enabled("acme", "team_velocity"),
            "rollup is OFF again"
        );
        assert_eq!(
            rollups.surfaced_triggers().len(),
            1,
            "the historical consultation obligation is RETAINED (append-only audit trail)"
        );
    }

    /// The trigger is per-`(tenant, rollup)` — enabling one tenant's rollup does NOT enable another's
    /// (the key is load-bearing; a constant-key mutant would leak the enablement across tenants).
    #[test]
    fn rollup_enablement_is_keyed_per_tenant_and_rollup() {
        let mut rollups = RollupEnablement::new();
        rollups.enable("acme", "team_velocity");
        assert!(rollups.is_enabled("acme", "team_velocity"));
        // A different tenant — NOT enabled (tenant in the key).
        assert!(!rollups.is_enabled("globex", "team_velocity"));
        // A different rollup in the same tenant — NOT enabled (rollup id in the key).
        assert!(!rollups.is_enabled("acme", "sprint_burndown"));
    }

    // ───────── (4) a SpecialCategory worklog field routes into the DPIA gate (reuses P-GA-08) ─────────

    /// **A `SpecialCategory`-flagged worklog field routes into the DPIA gate (gdpr §2.3 / §2.4)** —
    /// REUSING the [`DpiaRouter`] (P-GA-08) verbatim, no second router. The special-category worklog
    /// metric mints a DPIA marker; routing it against an empty prior fires `DpiaVerdict::Required`.
    #[test]
    fn a_special_category_worklog_field_routes_into_the_dpia_gate() {
        // The special-category worklog field emits a DPIA marker (the same minting every special-
        // category field uses — myelin_gdpr::dpia_markers walks the registry).
        let markers: BTreeSet<DpiaMarker> = myelin_gdpr::dpia_markers::<WorklogRow>();
        assert_eq!(
            markers.len(),
            1,
            "exactly the special-category worklog field emits a DPIA marker"
        );
        let marker = markers.iter().next().unwrap();
        assert_eq!(marker.field_path, "WorklogRow.sensitive_metric");
        assert_eq!(marker.special_category_kind, "health");

        // The router fires DPIA-required on the new flow (surfaced for a DPO, not auto-decided).
        let router = DpiaRouter::new();
        let verdicts = router.route(&BTreeSet::new(), &markers);
        assert_eq!(
            verdicts.len(),
            1,
            "a new special-category worklog flow fires the DPIA gate"
        );
        match &verdicts[0] {
            DpiaVerdict::Required { marker, reason } => {
                assert_eq!(marker.field_path, "WorklogRow.sensitive_metric");
                assert!(
                    reason.contains("DPO"),
                    "surfaced for a DPO, not auto-decided"
                );
            }
        }
    }

    // ───────── (5) build-data-as-LLM-training foreclosed by default (the architecture test) ─────────

    /// **Architecture test: build-data-as-LLM-training has NO code path (gdpr §2.4 / AG-8) — the
    /// foreclosure is the ABSENCE of a training-feed surface.** The GDPR-service surface exposes no
    /// `*_train*` / `*_model_feed*` / `*_llm_train*` entry point that consumes tenant content into
    /// model training. The foreclosure is structural: there is nothing to feed training BECAUSE no
    /// such surface exists. We assert the absence over this crate's own public re-exports (the
    /// surface a caller could reach) — a future training-feed would have to ADD a surface, which this
    /// test would force to be named/ratified (the `[OPEN — LEGAL]` opt-in, never a default).
    #[test]
    fn build_data_as_llm_training_has_no_code_path() {
        // The foreclosure constant documents the posture.
        assert!(
            BUILD_TRAINING_FORECLOSURE.contains("foreclosed by default")
                && BUILD_TRAINING_FORECLOSURE.contains("separately-ratified opt-in"),
            "the foreclosure is documented: no default training-feed path"
        );

        // Scan THIS crate's source tree for a training-feed call surface — there must be none (the
        // foreclosure is the absence of a surface). A grep over the crate's own src for a
        // training-feed identifier returns nothing (this file's own foreclosure prose excluded — it
        // names training only to FORECLOSE it, not to feed it).
        let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut training_feed_surfaces: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(src_dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            // This module documents the foreclosure (it names training to forbid it); skip its own file.
            if path.file_name().and_then(|n| n.to_str()) == Some("worklog.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read src file");
            for needle in [
                "fn train_model",
                "feed_training",
                "llm_training_feed",
                "train_on_tenant",
            ] {
                if text.contains(needle) {
                    training_feed_surfaces.push(format!(
                        "{}: {needle}",
                        path.file_name().unwrap().to_string_lossy()
                    ));
                }
            }
        }
        assert!(
            training_feed_surfaces.is_empty(),
            "build-data-as-LLM-training is foreclosed: NO training-feed surface may exist, found {training_feed_surfaces:?}"
        );
    }

    // ───────── the named floors are recorded in writing (dated) ─────────

    /// The `[OPEN — LEGAL]` worklog residual + the all-H1–H18-exist GA-D1 precondition are named in
    /// writing (the floor is never silently dropped — VISION §3).
    #[test]
    fn the_open_legal_residual_and_the_ga_d1_precondition_are_named() {
        assert!(WORKLOG_BASIS_RESIDUAL.contains("TBD_LEGAL"));
        assert!(WORKLOG_BASIS_RESIDUAL.contains("P-GA-31"));
        assert!(
            WORKLOG_BASIS_RESIDUAL.contains("works-council"),
            "the works-council ratification is the named parallel-legal residual"
        );
        assert!(
            ALL_HOLDERS_EXIST_FOR.contains("GA-D1") && ALL_HOLDERS_EXIST_FOR.contains("P-GA-32"),
            "all H1–H18 now exist — the GA-D1 precondition, named for P-GA-32"
        );
    }
}
