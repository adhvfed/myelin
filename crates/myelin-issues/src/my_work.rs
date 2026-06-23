//! # `my_work` — Issues "My Work" over the ONE Notif inbox + the humanise templates (ISS-P22 / P-389, M4)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! ("My Work" over the ONE inbox; the reason/subject filters) +
//! `planning/05-refined-shared-systems-architecture/notifications.md` §1.3 (the C-9 resolution — there
//! is exactly ONE cross-subsystem inbox; Issues "My Work" is a *scoped, filtered query INTO this one
//! inbox*, NEVER a second store — one store → one read-state truth) + §3.3 (the ONE platform
//! templating surface). **Reconciliation:** `00-reconciliation-decisions.md` OQ-L (the ONE templating
//! surface — Issues SLA strings register into `humanise`/ICU-MessageFormat; there is no second
//! template engine). **External insight:** `01-process-and-quality-doctrine.md` §7 ("My Work" is a
//! FILTER over the ONE inbox, not a new store; the ONE templating surface).
//!
//! **Contracts CONSUMED (implemented to the FROZEN Notif-owned shapes — escalate a needed change,
//! never diverge):**
//! - **7.1** `list_inbox(principal, filter?, page?)` — the ONE inbox. "My Work" is
//!   [`myelin_notif::InboxFilter::issues_my_work`] (a `filter` over `subsystem ∈ {issue} ∧ reason ∈
//!   {assigned, mentioned, review_requested, sla, watched, blocked, approval_requested}`), NEVER a
//!   second store. [`my_work_filter`] is the Issues-side canonical accessor — it RETURNS the ONE Notif
//!   filter (it does not define a parallel filter shape).
//! - **7.2** `mark / snooze / mark_all_read` — one read-state truth across all views. A mark/snooze on
//!   a "My Work" item reflects in the underlying inbox because there is ONE row (the C-9 read-state
//!   truth). Proven by the chained e2e ([`tests`]) + the integration drill
//!   (`tests/integration_issues_my_work_c9.rs`).
//! - **7.3** `humanise(item | (template_key, args), viewer, locale)` — the ONE templating surface.
//!   [`register_issue_humanise_templates`] registers the Issues SLA-at-risk / unblocked /
//!   approval-requested strings into the ONE [`myelin_notif::TemplateStore`] (no second template
//!   engine) — keyed by the Issues template keys ([`TPL_SLA_AT_RISK`] / [`TPL_UNBLOCKED`] /
//!   [`TPL_APPROVAL_REQUESTED`]), ICU-MessageFormat-subset bodies with `{0}` the per-viewer-bound
//!   subject (so a confidential subject still tombstones — NOTIF-D4 holds for an Issues string too).
//! - **7.6** `define_notif_rule` — the Issues reason set, declared at ISS-P04 ([`crate::declares`]),
//!   now WIRED: [`wire_issues_my_work`] registers the rule set into the ONE
//!   [`myelin_notif::NotifRuleRegistry`] AND the templates into the ONE [`myelin_notif::TemplateStore`]
//!   in one call — the build-time wiring point the run table places at ISS-P22.
//!
//! ## What this prompt (ISS-P22 / P-389) ships — the WIRING, never a new store/engine
//!
//! The Notif platform already owns every mechanism: the ONE inbox + the `issues_my_work` filter
//! (NOTIF-P5), the one-read-state `mark`/`snooze`/`mark_all_read` (NOTIF-P6), the ONE `humanise`
//! templating surface + platform-default reason templates (NOTIF-P8), and Issues' reason set is
//! DECLARED (ISS-P04, [`crate::declares::issue_notif_rules`]) and its watcher read-fanout + real SLA
//! chain are WIRED (NOTIF-P21, [`crate::sla_escalation`]). ISS-P22 is the *completion of M4-I5*: the
//! Issues-side wiring that
//! 1. exposes "My Work" as the canonical Issues accessor over the ONE filter ([`my_work_filter`] /
//!    [`list_my_work`]) — a filter, never a fourth inbox; and
//! 2. registers the Issues humanise template strings (the SLA at-risk / unblocked / approval-requested
//!    surface) into the ONE templating surface ([`register_issue_humanise_templates`]); and
//! 3. ties both into the ONE Notif surfaces in one call ([`wire_issues_my_work`]).
//!
//! There is **0 second store** (My Work reads `list_inbox`) and **0 second template engine** (the
//! strings register into the ONE `TemplateStore` and render through the ONE `humanise`). These are the
//! green artifacts the prompt's GATE names.
//!
//! ## FLOOR named (VISION §3 — name-your-floors)
//!
//! - **The SLA-breach escalation ENGINE lands in ISS-P26 (the SLA engine).** The SLA at-risk *string*
//!   registers HERE (the templating-surface half); the live SLA timer that DECIDES when an issue is
//!   "at risk" and fires the curated Signal carrying [`crate::declares::RULE_KEY_SLA_AT_RISK`] is the
//!   ISS-P26 SLA engine on the `myelin-flow` durable timer wheel (the SAME wheel ISS-P22's escalation
//!   chain — [`crate::sla_escalation::issue_sla_escalation_policy`] — arms). Named so the at-risk
//!   string here is not mistaken for the SLA engine. No NEW floor beyond it (the prompt states so).
//!
//! ## Mutation floor (yes/no — stated)
//!
//! Per the prompt's TESTS line: "My Work" is a FILTER over `list_inbox` (it adds no decision logic of
//! its own — it delegates to the frozen, already-mutation-tested `InboxFilter::matches` +
//! `list_inbox`) and the template registration is a data insertion into the ONE store. It is **not
//! data-loss-bearing** — so **no `cargo-mutants` floor is mandatory** for this module. The
//! load-bearing decision logic (the filter narrowing, the one-read-state mark/snooze, the
//! humanise-leak chokepoint) carries its mutation floor in `myelin-notif` (list_inbox.rs / read_state.rs
//! / humanise.rs, each ≥ 80% measured). The gate here is the one-read-state + 0-second-store +
//! ONE-template assertions (the unit + e2e + the C-9 integration drill).

use myelin_identity::{Consistency, Principal};
use myelin_notif::{
    list_inbox, AllowAllAuthorize, HumaniseTemplate, InboxFilter, InboxPage, InboxProjection,
    NotifRuleRegistry, Page, ReadAuthorizePort, TemplateStore, DEFAULT_LOCALE,
    PLATFORM_DEFAULT_TENANT,
};

use crate::declares::register_issue_notif_rules;

// ===========================================================================
// §1 — "My Work" over the ONE inbox (contract 7.1 — a FILTER, never a second store)
// ===========================================================================

/// **The Issues "My Work" filter (contract 7.1 — the C-9 scoped view).** RETURNS the ONE frozen Notif
/// filter [`InboxFilter::issues_my_work`] — `subsystem ∈ {issue} ∧ reason ∈ {assigned, mentioned,
/// review_requested, sla, watched, blocked, approval_requested}`. Issues does NOT define a parallel
/// filter shape; "My Work" is a saved filter over the ONE inbox, never a fourth store (the C-9
/// invariant). Every reason in this set is backed by an Issues-registered notif rule
/// ([`crate::declares::issue_notif_rules`]) plus the cross-subsystem `mentioned`/`review_requested`
/// reasons an issue can also carry.
pub fn my_work_filter() -> InboxFilter {
    InboxFilter::issues_my_work()
}

/// **`list_my_work(inbox, principal, page, authorize, at)` — read "My Work" over the ONE inbox
/// (contract 7.1).** A thin, intention-revealing wrapper over [`list_inbox`] with the
/// [`my_work_filter`] applied. It is the SAME read surface as the unified inbox — recipient-scoped,
/// step-0 read-authorized (ADR-03, a denied subject is held not leaked), ranked, paged — narrowed to
/// the Issues My-Work reasons. Because it calls `list_inbox`, "My Work" is structurally a SUBSET of
/// the ONE inbox (it can add no row the unfiltered inbox lacks) and shares the ONE read-state column
/// (a mark/snooze here reflects everywhere — one read-state truth). There is **0 second store**.
pub fn list_my_work(
    inbox: &InboxProjection,
    principal: &Principal,
    page: &Page,
    authorize: &dyn ReadAuthorizePort,
    at: &Consistency,
) -> InboxPage {
    list_inbox(inbox, principal, &my_work_filter(), page, authorize, at)
}

/// **`list_my_work_default(inbox, principal, at)` — the convenience entry (first page, allow-all
/// authorize).** For the single-cell self-host / a CLI default where the live Identity `check` is the
/// `AllowAllAuthorize` seam (the documented non-bypass; production substitutes the real `check`
/// behind the SAME [`ReadAuthorizePort`]). Returns the first [`Page`] of "My Work".
pub fn list_my_work_default(
    inbox: &InboxProjection,
    principal: &Principal,
    at: &Consistency,
) -> InboxPage {
    list_my_work(inbox, principal, &Page::default(), &AllowAllAuthorize, at)
}

// ===========================================================================
// §2 — the humanise templates (contract 7.3 — the ONE templating surface, OQ-L)
// ===========================================================================

/// The Issues template key for the **SLA at-risk / overdue** humanise string (the SLA-breach surface;
/// the *string* registers here, the SLA engine that fires it is ISS-P26). `{0}` is the per-viewer-
/// bound subject (the issue ref) — a confidential subject still tombstones (NOTIF-D4).
pub const TPL_SLA_AT_RISK: &str = "issue.sla.at_risk";
/// The Issues template key for the **unblocked** humanise string (the flagship "remind me when
/// unblocked" trigger re-surface). `{0}` is the per-viewer-bound subject.
pub const TPL_UNBLOCKED: &str = "issue.unblocked";
/// The Issues template key for the **approval-requested** humanise string (the HITL approval card the
/// human must act on). `{0}` is the per-viewer-bound subject.
pub const TPL_APPROVAL_REQUESTED: &str = "issue.approval_requested";

/// The Issues platform-default humanise template bodies (the §3.3 ICU-MessageFormat-SUBSET strings)
/// for the SLA at-risk / unblocked / approval-requested surface. `{0}` is the SUBJECT slot (resolved
/// per-viewer through Refs `resolve` → the title, or a tombstone display for a denied/erased viewer —
/// the leak invariant is the ONE surface's, structural). Frozen as the platform default; a tenant
/// brands/localises by registering its own `(tenant, key, locale)` override into the SAME store.
/// `(template_key, body, icon)`.
pub const ISSUE_HUMANISE_TEMPLATES: &[(&str, &str, &str)] = &[
    (
        TPL_SLA_AT_RISK,
        "SLA at risk on {0} — respond before the deadline",
        "sla",
    ),
    (TPL_UNBLOCKED, "{0} is now unblocked", "unblocked"),
    (
        TPL_APPROVAL_REQUESTED,
        "Approval requested on {0}",
        "approval",
    ),
];

/// Build the Issues humanise template ROWS (contract 7.3) — the SLA at-risk / unblocked /
/// approval-requested strings, each a NULL-tenant ([`PLATFORM_DEFAULT_TENANT`]) `en`
/// [`HumaniseTemplate`] (the platform default; a tenant overrides by `put`ting its own row). These are
/// the Issues SLA-surface strings the OQ-L "ONE templating surface" decision routes into `humanise`
/// — there is NO second template engine.
pub fn issue_humanise_templates() -> Vec<HumaniseTemplate> {
    ISSUE_HUMANISE_TEMPLATES
        .iter()
        .map(|(key, body, icon)| HumaniseTemplate {
            tenant: PLATFORM_DEFAULT_TENANT.to_string(),
            template_key: (*key).to_string(),
            locale: DEFAULT_LOCALE.to_string(),
            body: (*body).to_string(),
            icon: (*icon).to_string(),
        })
        .collect()
}

/// **Register Issues' humanise templates INTO the ONE templating surface (contract 7.3, the GATE).**
/// `put`s each [`issue_humanise_templates`] row into the supplied [`TemplateStore`] (the ONE store
/// every `humanise` call reads — ZERO second template engine, the OQ-L decision). Returns `&mut store`
/// for fluent chaining. The honest "the templates register on the ONE surface" is exactly this: a
/// subsequent `humanise((key, args), viewer, locale)` looks the body up in THIS store and renders it
/// through the ONE `humanise` pipeline (proven in [`tests`]).
pub fn register_issue_humanise_templates(store: &mut TemplateStore) -> &mut TemplateStore {
    for tpl in issue_humanise_templates() {
        store.put(tpl);
    }
    store
}

// ===========================================================================
// §3 — the wiring point (ISS-P22): both the reason set AND the templates onto the ONE surfaces
// ===========================================================================

/// **`wire_issues_my_work(registry, templates)` — wire Issues "My Work" into the ONE Notif surfaces
/// (the ISS-P22 deliverable).** In ONE call:
/// 1. registers the Issues `define_notif_rule` reason set ([`register_issue_notif_rules`], contract
///    7.6 — the rules ISS-P04 DECLARED, now WIRED) into the ONE [`NotifRuleRegistry`]; and
/// 2. registers the Issues humanise templates ([`register_issue_humanise_templates`], contract 7.3 —
///    the SLA at-risk / unblocked / approval-requested strings) into the ONE [`TemplateStore`].
///
/// Both surfaces are Notif's (the inverse-signal seam — ZERO Notif change). After this call "My Work"
/// ([`list_my_work`]) reads the ONE inbox with one read-state truth, and an Issues SLA/unblocked/
/// approval Signal classifies through the registered rule AND humanises through the registered string
/// — all on the ONE inbox + ONE templating surface, never a second store/engine. The `serve` boot path
/// calls this once when the Issues subsystem registers against Notif.
pub fn wire_issues_my_work(registry: &mut NotifRuleRegistry, templates: &mut TemplateStore) {
    register_issue_notif_rules(registry);
    register_issue_humanise_templates(templates);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    use myelin_notif::{
        humanise, Channel, ReadState, Reason, RefProjection, RefResolution, RefResolvePort,
        RoutedInboxItem, Tombstone, TombstoneReason,
    };
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};

    use myelin_identity::{ConsistencyMode, PrincipalId, PrincipalKind, Zookie};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn me() -> Principal {
        Principal::stub(PrincipalId("psn:me".into()), PrincipalKind::Human, tenant())
    }
    fn at() -> Consistency {
        Consistency {
            at_least: Zookie("zk-0".into()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn item(item_id: &str, subject: &str, reason: Reason) -> RoutedInboxItem {
        RoutedInboxItem {
            tenant: tenant(),
            region: Region("fr-par".into()),
            item_id: item_id.into(),
            recipient: "psn:me".into(),
            subject: ArtifactRef(subject.into()),
            reason,
            class: myelin_notif::Class::Direct,
            origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
            dedup_key: item_id.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        }
    }

    /// Seed the ONE inbox: an Issues assigned row (in My Work), an Issues fyi row (NOT in My Work),
    /// and a Git review row (other subsystem — never in Issues My Work).
    fn seeded() -> InboxProjection {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(item(
            "iss-assigned",
            "myelin://acme/issue/issue/E-1",
            Reason::Assigned,
        ));
        inbox.upsert_for_test(item(
            "iss-fyi",
            "myelin://acme/issue/issue/E-2",
            Reason::Fyi,
        ));
        inbox.upsert_for_test(item(
            "git-review",
            "myelin://acme/git/pr/9",
            Reason::ReviewRequested,
        ));
        inbox
    }

    fn ids(page: &InboxPage) -> BTreeSet<String> {
        page.items.iter().map(|i| i.item_id.clone()).collect()
    }

    // --- §1: "My Work" is a FILTER over list_inbox, not a separate store ---

    /// **The Issues "My Work" filter IS the ONE Notif filter (no parallel filter shape).**
    #[test]
    fn my_work_filter_is_the_one_notif_filter() {
        assert_eq!(my_work_filter(), InboxFilter::issues_my_work());
    }

    /// **"My Work" is a FILTER over the ONE inbox — a STRICT SUBSET, not a second store.** Every My
    /// Work row is in the unfiltered inbox; the non-My-Work + other-subsystem rows are excluded; the
    /// view is strictly smaller.
    #[test]
    fn my_work_is_a_strict_subset_of_the_one_inbox() {
        let inbox = seeded();
        let full = list_inbox(
            &inbox,
            &me(),
            &InboxFilter::all(),
            &Page {
                after: None,
                limit: 1000,
            },
            &AllowAllAuthorize,
            &at(),
        );
        let mine = list_my_work(
            &inbox,
            &me(),
            &Page {
                after: None,
                limit: 1000,
            },
            &AllowAllAuthorize,
            &at(),
        );
        let full_ids = ids(&full);
        let my_ids = ids(&mine);
        assert!(my_ids.is_subset(&full_ids), "My Work ⊆ the ONE inbox");
        assert!(
            my_ids.len() < full_ids.len(),
            "My Work is a STRICT subset (a view, not a copy)"
        );
        assert!(
            my_ids.contains("iss-assigned"),
            "the assigned issue is in My Work"
        );
        assert!(
            !my_ids.contains("iss-fyi"),
            "a non-My-Work Issues reason is excluded"
        );
        assert!(
            !my_ids.contains("git-review"),
            "another subsystem's row is excluded"
        );
    }

    /// **One read-state truth: a mark on a "My Work" item reflects in the underlying ONE inbox.** Mark
    /// the assigned issue read through the My Work surface row id; re-list both the My Work view AND
    /// the unified inbox — the SAME row reads `read` in BOTH (0 second store, 0 divergence).
    #[test]
    fn mark_on_my_work_reflects_in_the_one_inbox() {
        let inbox = seeded();
        // mark through the read-state verb (the ONE store); the row id came from My Work.
        let mine = list_my_work_default(&inbox, &me(), &at());
        let row_id = mine
            .items
            .iter()
            .find(|i| i.item_id == "iss-assigned")
            .expect("the assigned issue is in My Work")
            .item_id
            .clone();
        myelin_notif::mark(&inbox, &me(), &row_id, ReadState::Read).expect("mark my own item");

        // re-list My Work: the row reads `read`.
        let mine2 = list_my_work_default(&inbox, &me(), &at());
        let in_view = mine2.items.iter().find(|i| i.item_id == row_id).unwrap();
        assert_eq!(in_view.state, "read", "the My Work row reads `read`");

        // re-list the unified inbox: the SAME row reads `read` (no second store to diverge).
        let full = list_inbox(
            &inbox,
            &me(),
            &InboxFilter::all(),
            &Page::default(),
            &AllowAllAuthorize,
            &at(),
        );
        let in_full = full.items.iter().find(|i| i.item_id == row_id).unwrap();
        assert_eq!(
            in_full.state, "read",
            "the SAME row reads `read` in the unified inbox (one read-state truth)"
        );
    }

    // --- §2: the humanise templates register on the ONE templating surface ---

    /// A synthetic Refs `resolve` port — allows exactly the (viewer, ref) pairs given, else a denied
    /// tombstone (the leak-test seam; the production wire is the Refs resolve chokepoint).
    struct Resolver {
        allowed: Vec<(String, String)>,
    }
    impl RefResolvePort for Resolver {
        fn resolve_display(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            ref_: &ArtifactRef,
            viewer: &Principal,
            _at: &Consistency,
        ) -> RefResolution {
            if self
                .allowed
                .iter()
                .any(|(v, r)| v == &viewer.principal_id.0 && r == &ref_.0)
            {
                RefResolution::Projection(RefProjection {
                    ref_: ref_.clone(),
                    title: "ENG-1421: payments timeout".into(),
                    icon: "issue".into(),
                })
            } else {
                RefResolution::Tombstone(Tombstone {
                    root: ref_.clone(),
                    reason: TombstoneReason::Denied,
                })
            }
        }
    }

    /// **The Issues humanise templates register on the ONE surface and RENDER through the ONE
    /// `humanise` pipeline (0 second template engine).** Register the Issues strings into the ONE
    /// `TemplateStore`; a `humanise((key, [subject]), viewer, locale)` for an ALLOWED viewer renders
    /// the Issues body with the resolved title — proving the string lives on the ONE surface.
    #[test]
    fn issue_templates_register_and_render_on_the_one_surface() {
        let mut store = TemplateStore::with_platform_defaults();
        let before = issue_humanise_templates().len();
        register_issue_humanise_templates(&mut store);

        let subject = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
        let resolver = Resolver {
            allowed: vec![("psn:me".into(), subject.0.clone())],
        };

        // SLA at-risk renders the Issues body with the resolved title (the ONE pipeline).
        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_SLA_AT_RISK,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(
            h.text, "SLA at risk on ENG-1421: payments timeout — respond before the deadline",
            "the SLA at-risk string renders through the ONE humanise surface"
        );
        assert_eq!(h.icon, "issue", "slot-0 subject icon wins");

        // unblocked + approval render too (all three Issues strings are on the ONE surface).
        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_UNBLOCKED,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(h.text, "ENG-1421: payments timeout is now unblocked");

        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_APPROVAL_REQUESTED,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert_eq!(h.text, "Approval requested on ENG-1421: payments timeout");

        assert_eq!(before, 3, "the three Issues SLA-surface strings");
    }

    /// **The leak invariant holds for an Issues string too (NOTIF-D4 — 0 title leak).** A DENIED
    /// viewer humanising the SLA at-risk string gets the tombstone display, NEVER the title — the
    /// Issues string inherits the ONE surface's structural leak-safety (it is not a second engine).
    #[test]
    fn issue_template_is_leak_safe_for_a_denied_viewer() {
        let mut store = TemplateStore::with_platform_defaults();
        register_issue_humanise_templates(&mut store);
        let subject = ArtifactRef("myelin://acme/issue/issue/ENG-1421".into());
        // the resolver allows NOBODY → a denied tombstone.
        let resolver = Resolver { allowed: vec![] };
        let h = humanise(
            &resolver,
            &tenant(),
            &Region("fr-par".into()),
            &store,
            TPL_SLA_AT_RISK,
            std::slice::from_ref(&subject),
            &me(),
            DEFAULT_LOCALE,
            &at(),
            Channel::Cli,
        );
        assert!(
            !h.text.contains("payments timeout"),
            "the title NEVER leaks to a denied viewer (NOTIF-D4)"
        );
        assert!(
            h.text.contains("a restricted issue"),
            "the denied subject renders as the PII-free tombstone display"
        );
    }

    // --- §3: the wiring point ties both onto the ONE surfaces ---

    /// **`wire_issues_my_work` registers BOTH the reason set AND the templates onto the ONE surfaces
    /// (the ISS-P22 wiring).** After the call the registry admits the Issues rules and the store holds
    /// the Issues templates — all on the ONE Notif surfaces (zero Notif change).
    #[test]
    fn wire_registers_both_reason_set_and_templates() {
        let mut reg = NotifRuleRegistry::platform_default();
        let mut store = TemplateStore::with_platform_defaults();
        let rules_before = reg.len();

        wire_issues_my_work(&mut reg, &mut store);

        // the reason set registered (5 Issues rules, contract 7.6).
        assert_eq!(
            reg.len(),
            rules_before + 5,
            "the Issues reason set wired into the ONE registry"
        );
        // the templates registered (the lookup finds the Issues SLA string on the ONE surface).
        assert!(
            store
                .lookup(PLATFORM_DEFAULT_TENANT, TPL_SLA_AT_RISK, DEFAULT_LOCALE)
                .is_some(),
            "the SLA at-risk string is on the ONE templating surface"
        );
        assert!(store
            .lookup(PLATFORM_DEFAULT_TENANT, TPL_UNBLOCKED, DEFAULT_LOCALE)
            .is_some());
        assert!(store
            .lookup(
                PLATFORM_DEFAULT_TENANT,
                TPL_APPROVAL_REQUESTED,
                DEFAULT_LOCALE
            )
            .is_some());
    }

    // --- the chained-mutation e2e (EI-01 §4): assign → My Work → snooze → reflects in inbox ---

    /// **THE CHAINED e2e (prompt TESTS line): assign an issue → it appears in "My Work" → snooze it →
    /// it reflects in the inbox.** One store, one read-state truth across the whole chain.
    #[test]
    fn e2e_assign_appears_in_my_work_then_snooze_reflects_in_inbox() {
        let inbox = InboxProjection::new();
        // (1) "assign an issue" — the assignment Signal routed an Issues assigned row into the ONE
        // inbox (the router's job; here we model the routed row the assignment produced).
        inbox.upsert_for_test(item(
            "iss-assigned",
            "myelin://acme/issue/issue/ENG-1421",
            Reason::Assigned,
        ));

        // (2) "it appears in My Work" — the assigned row is in the Issues My Work view.
        let mine = list_my_work_default(&inbox, &me(), &at());
        assert!(
            mine.items.iter().any(|i| i.item_id == "iss-assigned"),
            "the assigned issue appears in My Work"
        );

        // (3) "snooze it" — through the ONE read-state verb (the row id from My Work).
        myelin_notif::snooze(&inbox, &me(), "iss-assigned", "2026-07-01T09:00:00Z")
            .expect("snooze my own item");

        // (4) "it reflects in the inbox" — the snooze is the SAME row in the unified inbox: the state
        // is `snoozed` with the until recorded, and the ACTIVE inbox suppresses it (one read-state
        // truth — the My Work snooze reflected everywhere; 0 second store).
        let full = list_inbox(
            &inbox,
            &me(),
            &InboxFilter::all(),
            &Page::default(),
            &AllowAllAuthorize,
            &at(),
        );
        let row = full
            .items
            .iter()
            .find(|i| i.item_id == "iss-assigned")
            .expect("the row is still in the ONE store (snooze does not delete)");
        assert_eq!(
            row.state, "snoozed",
            "the snooze reflected in the unified inbox"
        );
        assert_eq!(
            row.snooze_until.as_deref(),
            Some("2026-07-01T09:00:00Z"),
            "the until is recorded on the ONE row"
        );
        // the ACTIVE inbox (which suppresses parked rows) no longer shows it — both views agree.
        let active = myelin_notif::active_inbox(full.items.clone());
        assert!(
            !active.iter().any(|i| i.item_id == "iss-assigned"),
            "the snoozed item is suppressed from the active inbox (one read-state truth)"
        );
        // and it is suppressed from the ACTIVE My Work view too (the SAME read-state).
        let mine_active =
            myelin_notif::active_inbox(list_my_work_default(&inbox, &me(), &at()).items);
        assert!(
            !mine_active.iter().any(|i| i.item_id == "iss-assigned"),
            "the snooze reflected in My Work too (one store)"
        );
    }
}
