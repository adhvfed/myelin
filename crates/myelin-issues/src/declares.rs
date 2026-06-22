//! # `declares` — the Issues `declare_indexable` IndexSpec + the `define_notif_rule` reason set (ISS-P04 / P-243, M2)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §6.3 (the Search projection Issues declares) + §10/§3.1 (the Trigger `on_resolve` → the ONE Notif
//! inbox reasons), and `01-tech-and-data-model.md` §6.1 (the typed-core hot facets a board/list/search
//! query pins on) + §"the projection feeder" (the derived indexable projection). **Contracts:**
//! - **6.3** `declare_indexable(IndexSpec{ subsystem, type, struct_fields, semantic, acl_object_type })`
//!   — the per-subsystem projection shape. Issues OWNS *what* its facets are; Search owns the engine.
//! - **7.6** `define_notif_rule(reason, dedup_tpl, default_class)` — Signal class → inbox
//!   reason/priority. Issues registers its set (SLA at-risk / unblocked / approval-requested) against
//!   Notif's ONE ranking table.
//!
//! ## What this prompt (ISS-P04 / P-243) ships — the DECLARATIONS only
//!
//! Two registrations so the M4 emitter/wiring prompts compile against an *already-declared* shape
//! (EI-01 §7 — declare the projection + the reasons at the plan layer so the consumers compile):
//!
//! 1. **The Issues `declare_indexable` IndexSpec** ([`issue_facets_projection_spec`]) — the
//!    `issue.*` facets projection: the structured/columnar facets a board/list/search query filters
//!    on ([`FACET_STATE_CATEGORY`]/[`FACET_PRIORITY`]/[`FACET_ASSIGNEE`]/[`FACET_TYPE_RANK`]/
//!    [`FACET_PROJECT_ID`]/[`FACET_CYCLE_ID`]/[`FACET_RANK`]), `type = "issue"`,
//!    `acl_object_type = "issue"` (an issue's reachability is decided by the issue object itself via
//!    the frozen ReBAC `issue` namespace + the `- confidential` set-difference, [`crate::rebac_fragment`]).
//!    The full-text body (`title`/props/comment-body free-text) is NOT a structured facet — it arrives
//!    at emit time in the index-time `SearchProjection.text` ([`myelin_search::SearchProjection`]), so
//!    it is deliberately absent from `struct_fields` (the spec is the columnar schema, the projection
//!    is the row). Issues is **not** vector-embedded in v1 (`semantic = false`): board/list facet
//!    filtering + trigram title text is the floor; semantic embedding is the post-v1 follow-on.
//!
//! 2. **The Issues `define_notif_rule` reason set** ([`issue_notif_rules`]) — the three reasons
//!    Issues' triggers/SLA timers route into the ONE Notif inbox (arch §10 the stateful Trigger
//!    `on_resolve` + §3.1 the ranking table): **SLA at-risk** ([`Reason::Sla`] → [`Class::Critical`]),
//!    **unblocked** ([`Reason::Unblocked`] → [`Class::Watching`], the flagship "remind me when
//!    unblocked" trigger), and **approval-requested** ([`Reason::ApprovalRequested`] →
//!    [`Class::Critical`], the HITL approval card). Each is built via the frozen
//!    [`define_notif_rule`] verb (so the supplied `default_class` is RECONCILED against Notif's §3.1
//!    table — Issues registers WHICH reason, the table owns the band) and keyed under a stable
//!    `rule_key` ([`RULE_KEY_SLA_AT_RISK`]/[`RULE_KEY_UNBLOCKED`]/[`RULE_KEY_APPROVAL_REQUESTED`]).
//!
//! ## Coherence (EI-01 §7) — Issues defines NO second shape
//!
//! Both deliverables construct the ONE frozen consumer-owned shape: the IndexSpec is
//! [`myelin_search::IndexSpec`] (Search owns it; the registration is "accepted" exactly when a live
//! [`IncrementalIndexer`](myelin_search::IncrementalIndexer) admits it without a schema mismatch —
//! the only honest definition of accepted, [`register_issue_facets_projection_spec`]); the notif
//! rules are [`myelin_notif::NotifRule`]s registered into the ONE
//! [`NotifRuleRegistry`](myelin_notif::NotifRuleRegistry) by CALLING `register` (the inverse-signal
//! seam — zero Notif change, [`register_issue_notif_rules`]). There is no parallel Issues indexing
//! contract and no second Issues reason vocabulary.
//!
//! ## FLOORS named (VISION §3 — these are DECLARATIONS, the live work follows on)
//!
//! - **The `issue.*` Search projection EMITTER** — the live `project(ref)` that walks an issue and
//!   builds the per-doc [`myelin_search::SearchProjection`] (title text + the typed facet values)
//!   and emits it through the outbox — lands in **ISS-P17** (the `issue.*` Search projection). Here
//!   the SPEC (the schema) is registered; no emitter, no index rows.
//! - **The notif WIRING** — the live route from an Issues trigger/SLA timer firing → a curated
//!   Signal carrying one of these `rule_key`s → the classified inbox item ("My Work") — lands in
//!   **ISS-P22** ("My Work" over the ONE Notif inbox). Here the reason set (the rules) is registered;
//!   no signal route.
//!
//! No mutation floor is required (these are DECLARATIONS, not core decision logic — the prompt's
//! TESTS line states so); the serialize-to-frozen-shape + accepted-registration tests are the gate.

use std::collections::BTreeMap;

use myelin_notif::{define_notif_rule, Class, DedupTpl, NotifRule, NotifRuleRegistry, Reason};
use myelin_query::FieldType;
use myelin_search::IndexSpec;

use crate::rebac_fragment::object_types;

// ===========================================================================
// §1 — the declare_indexable IndexSpec (contract 6.3): the issue.* facets projection
// ===========================================================================

/// The subsystem token Issues declares its projection under (`issue`, the Bus §6.2 / arch §2
/// canonical singular token — the same token [`crate::events`] anchors every `issue.*` event on).
pub const ISSUE_SUBSYSTEM: &str = "issue";

/// The artifact type Issues' facets projection indexes — an `issue` (the canonical ENG-1421 row;
/// the projection feeder's derived indexable doc, `01-tech-and-data-model.md` §6.1).
pub const ISSUE_TYPE: &str = "issue";

/// The structured-facet key for the FIXED state category (`backlog`/`started`/`done`/… — the
/// cross-project reporting invariant the board scan keys on, `01` §6.1; an exact-match `Select`).
pub const FACET_STATE_CATEGORY: &str = "state_category";
/// The structured-facet key for the issue priority (the typed-core `priority smallint`; an ordered
/// `Int` facet so `priority >= P2` is a structured comparison).
pub const FACET_PRIORITY: &str = "priority";
/// The structured-facet key for the assignee (a *pseudonymous* principal id — erasure-safe, EI-04
/// §1; a `Principal` facet compared by equality only, never ordered).
pub const FACET_ASSIGNEE: &str = "assignee";
/// The structured-facet key for the denormalised type rank (sub-task=0 … initiative=3 — the
/// board↔roadmap partitioning facet, `01` §6.1; an ordered `Int`).
pub const FACET_TYPE_RANK: &str = "type_rank";
/// The structured-facet key for the parent project (the Identity `project` authz scope + the
/// per-project board filter; a `Relation` ref facet).
pub const FACET_PROJECT_ID: &str = "project_id";
/// The structured-facet key for the current cycle membership (the time-axis/burndown filter; a
/// `Relation` ref facet — nullable, the row carries the denormalised cache).
pub const FACET_CYCLE_ID: &str = "cycle_id";
/// The structured-facet key for the board rank (the frozen `order_key` LexoRank string, contract
/// 13.3 — an [`FieldType::OrderKey`] facet whose byte order IS the board sort order).
pub const FACET_RANK: &str = "rank";

/// Build the Issues **`declare_indexable` facets-projection spec** (contract 6.3) — the deliverable
/// of ISS-P04 / P-243. The returned [`IndexSpec`] is the frozen Search-owned shape Issues registers:
/// `subsystem = "issue"`, `type = "issue"`, the seven structured board/list/search facets, non-
/// semantic (Issues is trigram-title + facet filter in v1, not vector-embedded — `01` §6.1),
/// `acl_object_type = "issue"` (an issue's reachability is the issue object's own ReBAC `view`
/// permission with the `- confidential` set-difference, [`crate::rebac_fragment`]).
///
/// The full-text body (`title`/props/comment free-text) is NOT in the spec — it arrives at emit time
/// in the index-time [`SearchProjection.text`](myelin_search::SearchProjection) (the ISS-P17
/// emitter). This function registers the SCHEMA; the emitter ships the rows.
pub fn issue_facets_projection_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The structured/columnar facets a board/list/search query filters on. The full-text body
    // (title / props free-text / comment bodies) is delivered via SearchProjection.text at emit
    // time (ISS-P17), NOT here — the spec is the columnar schema, the projection is the row.
    struct_fields.insert(FACET_STATE_CATEGORY.to_string(), FieldType::Select);
    struct_fields.insert(FACET_PRIORITY.to_string(), FieldType::Int);
    struct_fields.insert(FACET_ASSIGNEE.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_TYPE_RANK.to_string(), FieldType::Int);
    struct_fields.insert(FACET_PROJECT_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_CYCLE_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_RANK.to_string(), FieldType::OrderKey);

    // The default constructor pins `acl_object_type == type_` (== "issue"), which is exactly what
    // Issues wants: an issue's reachability is decided by the issue object's own frozen ReBAC `view`
    // permission (with the `- confidential` set-difference) — there is no parent ACL object here,
    // UNLIKE git's blob→repo. We make the equality explicit so a future facet rename can't silently
    // drift the ACL anchor off the `issue` namespace.
    IndexSpec::new(ISSUE_SUBSYSTEM, ISSUE_TYPE, struct_fields)
        .with_acl_object_type(object_types::ISSUE)
}

/// **Register Issues' facets-projection spec WITH Search (the GATE).** Builds the
/// [`issue_facets_projection_spec`] and proves Search **accepts** it by admitting it into a live
/// [`IncrementalIndexer`](myelin_search::IncrementalIndexer)'s per-tenant facet union — the only
/// honest definition of "accepted" (Search is the authority that admits; Issues does not get to
/// assert acceptance). Returns the spec that was accepted so a caller can assert the registered shape.
///
/// In production the spec is handed to Search's `declare_indexable` registration at subsystem boot;
/// here we exercise that admission directly (the indexer constructor IS the build-time
/// declare_indexable registration surface — `IndexSpec` doc §4.1). No fetcher/embedder is needed for
/// the SPEC registration; this proves the schema is admitted without a schema mismatch.
pub fn register_issue_facets_projection_spec() -> IndexSpec {
    let spec = issue_facets_projection_spec();
    // Admit it into a real indexer's facet union (the build-time declare_indexable surface). If the
    // facet types collided or the shape were malformed this would panic at construction; it does not.
    let _accepted = myelin_search::IncrementalIndexer::new(
        vec![spec.clone()],
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(myelin_search::MockEmbeddingAdapter::new(8)),
    );
    spec
}

/// A do-nothing [`ProjectFetcher`](myelin_search::ProjectFetcher) used ONLY to admit the spec into a
/// live indexer for the registration GATE (the SPEC half ships here; the real owner-`project` fetch
/// is the ISS-P17 emitter). It never fetches — registration does not index.
struct NullProjectFetcher;

impl myelin_search::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here — ISS-P17). A row that
        // is never asked-for projects to nothing; this is the registration GATE, not the index path.
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

// ===========================================================================
// §2 — the define_notif_rule reason set (contract 7.6): SLA / unblocked / approval
// ===========================================================================

/// The stable `rule_key` Issues' SLA-at-risk Signal carries (the `<rule>` segment of the curated
/// `sig.<tenant>.<severity>.<rule>` subject — arch §10 `sla.at_risk`). Notif classifies a Signal
/// carrying this key through the registered [`Reason::Sla`] rule.
pub const RULE_KEY_SLA_AT_RISK: &str = "issue.sla.at_risk";
/// The stable `rule_key` Issues' "remind me when unblocked" trigger Signal carries (the flagship
/// trigger, arch §10) → the registered [`Reason::Unblocked`] rule.
pub const RULE_KEY_UNBLOCKED: &str = "issue.trigger.unblocked";
/// The stable `rule_key` Issues' approval-requested HITL Signal carries (the §1 `approval.requested`
/// gate surface) → the registered [`Reason::ApprovalRequested`] rule.
pub const RULE_KEY_APPROVAL_REQUESTED: &str = "issue.approval.requested";
/// The stable `rule_key` Issues' assignment Signal carries (an issue assigned to a principal —
/// `issue.issue.assigned`, [`crate::events::ISSUE_ASSIGNED`]) → the registered [`Reason::Assigned`]
/// rule (the bounded DIRECT write-fanout set — an assignee is an explicit target, §3.5). The
/// flagship "My Work" reason: an assigned issue is the first row a person sees in their filtered view.
pub const RULE_KEY_ASSIGNED: &str = "issue.assigned";
/// The stable `rule_key` Issues' "your issue is now blocked" Signal carries (a `blocks` relation
/// edge landed on an issue you own/watch) → the registered [`Reason::Blocked`] rule (the ambient
/// WATCHING band — a calm "this is now blocked", the mirror of the flagship `unblocked` re-surface).
pub const RULE_KEY_BLOCKED: &str = "issue.blocked";

/// Build Issues' **`define_notif_rule` reason set** (contract 7.6) — the **FULL** Issues consumer
/// set accreted at NOTIF-P21 (P-342): the six reasons the §1.3 "My Work" filtered view pins on —
/// **assigned** ([`Reason::Assigned`] → [`Class::Direct`]), **blocked** ([`Reason::Blocked`] →
/// [`Class::Watching`]), **needs-approval** ([`Reason::ApprovalRequested`] → [`Class::Critical`]),
/// **overdue/SLA** ([`Reason::Sla`] → [`Class::Critical`]), **unblocked** ([`Reason::Unblocked`] →
/// [`Class::Watching`], the flagship "remind me when unblocked" trigger). ISS-P04/P-243 shipped the
/// SLA/unblocked/approval slice; NOTIF-P21 completes the set with `assigned` + `blocked` so the
/// "My Work" view ([`InboxFilter::issues_my_work`](myelin_notif::InboxFilter::issues_my_work),
/// `reason∈{assigned, mentioned, review_requested, sla, watched, blocked, approval_requested}`) is
/// driven by a registered Issues rule for every reason Issues owns. Each rule is built via the frozen
/// [`define_notif_rule`] verb, so the supplied `default_class` is RECONCILED against Notif's §3.1
/// ranking table (Issues registers WHICH reason; the table owns the band) — a band that disagreed
/// would fail loudly here, never silently mis-rank in prod.
///
/// The dedup templates collapse a storm by `(recipient, subject)`: five SLA pings on one issue, or
/// repeated unblock checks, collapse into ONE inbox row (the §3.2 write-time collapse).
pub fn issue_notif_rules() -> Vec<(&'static str, NotifRule)> {
    vec![
        (
            RULE_KEY_ASSIGNED,
            // Assigned → the DIRECT band (the bounded write-fanout set — an assignee is an explicit
            // target). One row per (recipient, issue) — a re-assign churn on the same issue collapses.
            define_notif_rule(
                Reason::Assigned,
                DedupTpl("issue.assigned:{recipient}:{subject}".to_string()),
                Class::Direct,
            )
            .expect("Reason::Assigned reconciles to Class::Direct in the §3.1 table"),
        ),
        (
            RULE_KEY_BLOCKED,
            // Blocked → the WATCHING (ambient) band — the calm mirror of the `unblocked` re-surface.
            // One row per (recipient, issue) — repeated block edges on the same issue collapse.
            define_notif_rule(
                Reason::Blocked,
                DedupTpl("issue.blocked:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("Reason::Blocked reconciles to Class::Watching in the §3.1 table"),
        ),
        (
            RULE_KEY_SLA_AT_RISK,
            // SLA at-risk / overdue → the CRITICAL band (pierces quiet-hours; the SLA timer fired).
            // One row per (recipient, issue) — repeated at-risk pings on the same issue collapse.
            define_notif_rule(
                Reason::Sla,
                DedupTpl("issue.sla:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("Reason::Sla reconciles to Class::Critical in the §3.1 table"),
        ),
        (
            RULE_KEY_UNBLOCKED,
            // Unblocked → the WATCHING (ambient) band — the flagship "remind me when unblocked"
            // trigger re-surfaces calmly, not as a critical interrupt. One row per (recipient, issue).
            define_notif_rule(
                Reason::Unblocked,
                DedupTpl("issue.unblocked:{recipient}:{subject}".to_string()),
                Class::Watching,
            )
            .expect("Reason::Unblocked reconciles to Class::Watching in the §3.1 table"),
        ),
        (
            RULE_KEY_APPROVAL_REQUESTED,
            // Approval-requested (needs-approval) → the CRITICAL band (the HITL approval card; the
            // human must act). One row per (recipient, issue) — a re-requested approval collapses.
            define_notif_rule(
                Reason::ApprovalRequested,
                DedupTpl("issue.approval:{recipient}:{subject}".to_string()),
                Class::Critical,
            )
            .expect("Reason::ApprovalRequested reconciles to Class::Critical in the §3.1 table"),
        ),
    ]
}

/// **Register Issues' notif reason set WITH Notif (the GATE).** Registers the three
/// [`issue_notif_rules`] into the supplied [`NotifRuleRegistry`] via the inverse-signal seam
/// (`register` — a data insertion, ZERO Notif change). Returns `&mut` registry for fluent chaining.
///
/// This is the honest definition of "the reason set registers with Notif and is accepted": Notif's
/// registry admits each rule under its `rule_key`, and a later `classify(rule_key, …)` routes a
/// Signal through it (proven in this module's tests + the CDC). The live trigger/SLA → Signal route
/// that drives these is the ISS-P22 wiring.
pub fn register_issue_notif_rules(registry: &mut NotifRuleRegistry) -> &mut NotifRuleRegistry {
    for (key, rule) in issue_notif_rules() {
        registry.register(key, rule);
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- §1: the declare_indexable IndexSpec ---

    /// **The spec is Issues' owned 6.3 shape.** Pins every field of the frozen `IndexSpec` Issues
    /// registers (a rename of a Search field, or a facet drift, breaks this — the registrant catches it).
    #[test]
    fn spec_is_issues_owned_6_3_shape() {
        let s = issue_facets_projection_spec();
        assert_eq!(
            s.subsystem, "issue",
            "Issues owns the `issue` subsystem projection"
        );
        assert_eq!(s.type_, "issue", "the indexed artifact type is an issue");
        assert_eq!(
            s.acl_object_type, "issue",
            "an issue's reachability is its own ReBAC `view` permission (no parent ACL object)"
        );
        assert_eq!(
            s.acl_object_type,
            object_types::ISSUE,
            "the acl_object_type is exactly Issues' frozen ReBAC `issue` object type"
        );
        assert!(
            !s.semantic,
            "Issues is trigram-title + facet filter in v1, not vector-embedded"
        );
        // The seven structured board/list/search facets, each at its frozen FieldType.
        assert_eq!(
            s.struct_fields.len(),
            7,
            "exactly the seven structured issue facets"
        );
        assert_eq!(
            s.struct_fields.get("state_category"),
            Some(&FieldType::Select)
        );
        assert_eq!(s.struct_fields.get("priority"), Some(&FieldType::Int));
        assert_eq!(s.struct_fields.get("assignee"), Some(&FieldType::Principal));
        assert_eq!(s.struct_fields.get("type_rank"), Some(&FieldType::Int));
        assert_eq!(
            s.struct_fields.get("project_id"),
            Some(&FieldType::Relation)
        );
        assert_eq!(s.struct_fields.get("cycle_id"), Some(&FieldType::Relation));
        assert_eq!(s.struct_fields.get("rank"), Some(&FieldType::OrderKey));
    }

    /// **The full-text body is NOT a structured facet.** `title` / props free-text / comment bodies
    /// arrive at emit time in `SearchProjection.text` (ISS-P17), so they must be absent from
    /// `struct_fields` (the schema is the columnar half, not the body).
    #[test]
    fn fulltext_body_is_not_a_struct_facet() {
        let s = issue_facets_projection_spec();
        for absent in ["title", "body", "props", "comment", "description"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    /// **The spec serializes to the 6.3 wire shape (0 schema mismatches — the build-time gate).**
    /// Asserts the serialized JSON key set + values against the frozen contract-6.3 keys. A wire
    /// rename of any key (e.g. `type` → `type_`) or a facet-type drift is caught here.
    #[test]
    fn spec_serializes_to_the_6_3_wire_shape() {
        let s = issue_facets_projection_spec();
        let json = serde_json::to_value(&s).expect("the spec serializes");
        let obj = json.as_object().expect("the spec is a JSON object");

        // The exact frozen key set (the `type` rename, not `type_`, is the wire contract).
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "acl_object_type",
                "semantic",
                "struct_fields",
                "subsystem",
                "type"
            ],
            "the 6.3 wire key set"
        );

        assert_eq!(obj["subsystem"], serde_json::json!("issue"));
        assert_eq!(obj["type"], serde_json::json!("issue"));
        assert_eq!(obj["semantic"], serde_json::json!(false));
        assert_eq!(obj["acl_object_type"], serde_json::json!("issue"));
        // The typed columnar facets serialize to the frozen FieldType wire tokens (13.3).
        assert_eq!(
            obj["struct_fields"],
            serde_json::json!({
                "state_category": "Select",
                "priority": "Int",
                "assignee": "Principal",
                "type_rank": "Int",
                "project_id": "Relation",
                "cycle_id": "Relation",
                "rank": "OrderKey",
            }),
            "the structured facets serialize to the typed columnar shape (13.3)"
        );
    }

    /// **The registration is ACCEPTED by Search (the GATE).** Search admits the spec into a live
    /// indexer's per-tenant facet union without a schema mismatch — the returned accepted spec is
    /// byte-equal to the declared one (registration neither mutates nor rejects the shape).
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_issue_facets_projection_spec();
        assert_eq!(
            accepted,
            issue_facets_projection_spec(),
            "Search accepts the declared spec verbatim"
        );
    }

    // --- §2: the define_notif_rule reason set ---

    /// **The reason set IS the six frozen Issues reasons at their §3.1 bands.** assigned → direct,
    /// blocked → watching, SLA → critical, unblocked → watching, approval-requested → critical. A
    /// re-band (a `define_notif_rule` reconciliation drop) would have made the construction panic;
    /// this pins the accepted result.
    #[test]
    fn notif_rules_are_the_issues_reasons_at_their_bands() {
        let rules = issue_notif_rules();
        assert_eq!(
            rules.len(),
            5,
            "the five distinct Issues consumer reasons (assigned / blocked / SLA / unblocked / approval)"
        );

        let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();

        let asg = by_key
            .get(RULE_KEY_ASSIGNED)
            .expect("assigned rule registered");
        assert_eq!(asg.reason, Reason::Assigned);
        assert_eq!(
            asg.default_class,
            Class::Direct,
            "assigned is a direct target"
        );

        let blk = by_key
            .get(RULE_KEY_BLOCKED)
            .expect("blocked rule registered");
        assert_eq!(blk.reason, Reason::Blocked);
        assert_eq!(
            blk.default_class,
            Class::Watching,
            "blocked re-surfaces calmly"
        );

        let sla = by_key
            .get(RULE_KEY_SLA_AT_RISK)
            .expect("SLA rule registered");
        assert_eq!(sla.reason, Reason::Sla);
        assert_eq!(
            sla.default_class,
            Class::Critical,
            "SLA at-risk pierces (critical)"
        );

        let unb = by_key
            .get(RULE_KEY_UNBLOCKED)
            .expect("unblocked rule registered");
        assert_eq!(unb.reason, Reason::Unblocked);
        assert_eq!(
            unb.default_class,
            Class::Watching,
            "unblocked re-surfaces calmly (watching)"
        );

        let appr = by_key
            .get(RULE_KEY_APPROVAL_REQUESTED)
            .expect("approval rule registered");
        assert_eq!(appr.reason, Reason::ApprovalRequested);
        assert_eq!(
            appr.default_class,
            Class::Critical,
            "approval is a HITL interrupt (critical)"
        );
    }

    /// **The reason set registers with Notif and CLASSIFIES (the GATE).** Registering the set into a
    /// platform-default registry and classifying a Signal carrying each `rule_key` routes through the
    /// registered Issues rule (`from_registered_rule = true`) with the right reason + band + a dedup
    /// key that collapses by `(recipient, subject)`. This is the honest "accepted" — Notif's registry
    /// admits and routes the Issues rules with ZERO Notif change (the inverse-signal seam).
    #[test]
    fn notif_rules_register_and_classify_through_notif() {
        let subject = myelin_refs::ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());

        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_issue_notif_rules(&mut reg);
        assert_eq!(
            reg.len(),
            before + 5,
            "the five Issues rules accreted (no Notif change)"
        );

        // assigned → direct.
        let c = reg.classify(RULE_KEY_ASSIGNED, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Assigned);
        assert_eq!(c.default_class, Class::Direct);
        assert!(
            c.from_registered_rule,
            "the registered Issues rule took effect"
        );

        // blocked → watching.
        let c = reg.classify(RULE_KEY_BLOCKED, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Blocked);
        assert_eq!(c.default_class, Class::Watching);
        assert!(c.from_registered_rule);

        // SLA at-risk → critical.
        let c = reg.classify(RULE_KEY_SLA_AT_RISK, "psn:alice", &subject);
        assert_eq!(c.reason, Reason::Sla);
        assert_eq!(c.default_class, Class::Critical);
        assert!(
            c.from_registered_rule,
            "the registered Issues rule took effect"
        );
        assert_eq!(
            c.dedup_key,
            "issue.sla:psn:alice:myelin://acme/issue/issue/ENG-1421"
        );

        // unblocked → watching.
        let c = reg.classify(RULE_KEY_UNBLOCKED, "psn:bob", &subject);
        assert_eq!(c.reason, Reason::Unblocked);
        assert_eq!(c.default_class, Class::Watching);
        assert!(c.from_registered_rule);

        // approval-requested → critical.
        let c = reg.classify(RULE_KEY_APPROVAL_REQUESTED, "psn:carol", &subject);
        assert_eq!(c.reason, Reason::ApprovalRequested);
        assert_eq!(c.default_class, Class::Critical);
        assert!(c.from_registered_rule);
    }

    /// **Issues registers against the ONE Notif vocabulary — no second reason language.** The three
    /// rules are built by the frozen `define_notif_rule` verb (proven above by their non-panicking
    /// construction at the table-correct bands); an attempt to register a band that disagreed with
    /// the §3.1 table would be rejected loudly. This pins that Issues does not smuggle a reason into
    /// the wrong band (e.g. SLA into a non-critical band).
    #[test]
    fn issues_cannot_smuggle_a_reason_into_the_wrong_band() {
        // SLA registered at a NON-critical band is rejected by the frozen verb (the table owns the band).
        let err = define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Watching)
            .expect_err("SLA must register at the critical band the §3.1 table owns");
        // it is the loud class-mismatch (never a silent re-band).
        assert!(matches!(
            err,
            myelin_notif::DefineRuleError::ClassMismatch { .. }
        ));
    }
}
