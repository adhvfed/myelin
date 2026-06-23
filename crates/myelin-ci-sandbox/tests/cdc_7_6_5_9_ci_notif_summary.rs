//! # The CDC pair for contracts 7.6 + 5.9 — CI's status-summary registration (CI-side, NOTIF-P23 / P-344)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md`
//! - **7.6** `define_notif_rule` — CI registers its status-summary reason (a check status change →
//!   the §3.1 `state_changed → watching` band) via the FROZEN verb; Notif's registry ADMITS +
//!   CLASSIFIES it (zero Notif change — the inverse-signal seam, EI-01 §1).
//! - **5.9 / 7.3** the Git↔CI `CheckStatus` seam — CI's `CheckStatus.summary` is a `HumanisedRef`
//!   `(template_key, args)` pair (X-1, never a raw string) that resolves THROUGH Notif's ONE
//!   templating surface (7.3 `humanise`); CI registers its summary templates on that surface.
//!
//! **Reconciliation:** `00-reconciliation-decisions.md` X-1 (the most load-bearing cross-subsystem
//! seam — `CheckStatus.summary` is a `(template_key, args)` pair humanised by Notif, never a raw
//! string). This pair is the **M4 PRODUCER half** of the X-1 cross-band seam (provider CI ↔ consumer
//! Notif): CI declares its reason set + its summary templates at their table-correct bands /
//! registered bodies, and Notif admits + classifies + humanises them.
//!
//! ## What this pair pins (provider CI; consumer Notif)
//! - **PROVIDER (CI):** [`ci_notif_rules`](myelin_ci_sandbox::ci_notif_rules) declares CI's
//!   status-summary reason via the frozen `define_notif_rule` verb (table-correct band); CI's
//!   summary templates ([`CI_SUMMARY_TEMPLATES`](myelin_ci_sandbox::CI_SUMMARY_TEMPLATES)) carry the
//!   `{0}` subject slot; [`ci_summary`](myelin_ci_sandbox::ci_summary) builds a `(template_key,
//!   args)` pair, never a raw string.
//! - **CONSUMER (Notif):** the `NotifRuleRegistry` admits CI's rule + classifies a CI Signal through
//!   it (zero Notif change); the `TemplateStore` admits CI's summary templates; the
//!   `CheckStatus.summary` resolves through `humanise` (the end-to-end render is the companion
//!   integration test `integration_ci_summary_humanises.rs`).
//!
//! ## FLOOR (named, not silent)
//! No cargo-mutants floor on THIS CDC: it is a REGISTRATION + shape pin (the registration accretes
//! with zero Notif change; the summary is structurally a `HumanisedRef`). The render-pipeline
//! mutation floor is Notif's `humanise.rs` (NOTIF-P9 / P-187, ≥80% measured). The live CI
//! Signal-curation emitter that turns a real `ci.check.updated` into the curated Signal carrying the
//! registered rule_key + the `ci_summary` HumanisedRef is the CI emit follow-on (named in
//! `notif_rules.rs`).

use myelin_ci_sandbox::{
    ci_notif_rules, ci_summary, register_ci_notif_rules, register_ci_summary_templates,
    CheckVerdict, CI_CHECK_STATUS_RULE, CI_SUMMARY_TEMPLATES,
};
use myelin_notif::{
    reason_base_class, Class, NotifRuleRegistry, Reason, TemplateStore, DEFAULT_LOCALE,
    PLATFORM_DEFAULT_TENANT,
};
use myelin_refs::ArtifactRef;

/// **PROVIDER side of 7.6 — CI declares its status-summary reason at its table-correct band.** CI's
/// rule is built through the frozen `define_notif_rule` verb, so the `default_class` is RECONCILED
/// against Notif's §3.1 table (CI registers WHICH reason; the table owns the band). The reason is
/// `state_changed → watching` (a check status change is ambient activity on a watched subject).
#[test]
fn provider_ci_declares_status_summary_reason_at_its_table_band() {
    let rules = ci_notif_rules().expect("CI's set is table-correct by construction");
    assert_eq!(
        rules.len(),
        1,
        "CI declares exactly its status-summary reason"
    );
    let (key, rule) = &rules[0];
    assert_eq!(*key, CI_CHECK_STATUS_RULE);
    assert_eq!(rule.reason, Reason::StateChanged);
    assert_eq!(rule.default_class, Class::Watching);
    // the band is EXACTLY the §3.1 ranking-table band for the reason (CI cannot smuggle a band).
    assert_eq!(rule.default_class, reason_base_class(rule.reason).1);
}

/// **CONSUMER side of 7.6 — Notif's registry ADMITS + CLASSIFIES CI's reason (zero Notif change).**
/// The platform-default registry accretes CI's rule by a `register` call; a Signal carrying CI's
/// rule_key classifies through CI's rule into the right reason + band + rendered dedup key. If
/// admitting CI required a Notif edit, this CDC could not compile without touching `myelin-notif`.
#[test]
fn consumer_notif_admits_and_classifies_cis_reason_zero_change() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_ci_notif_rules(&mut reg).expect("CI's set registers");
    assert_eq!(reg.len(), before + 1, "CI's rule accreted, no Notif edit");

    let subject = ArtifactRef("myelin://acme/git/pr/9".into());
    let c = reg.classify(CI_CHECK_STATUS_RULE, "psn:author", &subject);
    assert_eq!(c.reason, Reason::StateChanged);
    assert_eq!(c.default_class, Class::Watching);
    assert!(c.from_registered_rule, "CI's registration took effect");
    assert_eq!(c.dedup_key, "ci.check:psn:author:myelin://acme/git/pr/9");
}

/// **PROVIDER side of 5.9 / X-1 — CI's `CheckStatus.summary` is a `(template_key, args)` pair, NEVER
/// a raw string.** Every verdict's summary is a `HumanisedRef` whose template_key is one of the
/// registered `ci.check.<verdict>` keys; the serialised shape is exactly `{template_key, args}` (no
/// raw-summary string field). A raw "build failed" has no code path through the seam.
#[test]
fn provider_ci_summary_is_a_humanised_ref_never_raw() {
    let verdicts = [
        CheckVerdict::Queued,
        CheckVerdict::InProgress,
        CheckVerdict::Success,
        CheckVerdict::Failure,
        CheckVerdict::Error,
        CheckVerdict::Neutral,
        CheckVerdict::Cancelled,
    ];
    for v in verdicts {
        let s = ci_summary(v, "build");
        // the template_key is a registered CI summary key.
        assert!(
            CI_SUMMARY_TEMPLATES
                .iter()
                .any(|(k, _, _)| *k == s.template_key),
            "verdict {v:?} → a registered summary key"
        );
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json.as_object().unwrap().len(),
            2,
            "{{template_key, args}} only"
        );
        assert!(json.get("text").is_none(), "no raw-string summary field");
    }
}

/// **CONSUMER side of 5.9 / 7.3 — Notif's template store ADMITS CI's summary templates (zero Notif
/// change).** The platform-default store accretes CI's seven verdict bodies by a `put`; each
/// resolves to its registered body with the `{0}` subject slot present (so the per-viewer resolve
/// binds it → NOTIF-D4). After this, a `CheckStatus.summary` HumanisedRef resolves through humanise
/// to a registered body, not the generic fallback.
#[test]
fn consumer_notif_admits_cis_summary_templates_zero_change() {
    let mut store = TemplateStore::with_platform_defaults();
    register_ci_summary_templates(&mut store);
    for (key, body, _icon) in CI_SUMMARY_TEMPLATES {
        let t = store
            .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
            .expect("Notif admits CI's summary template");
        assert_eq!(&t.body, body);
        assert!(t.body.contains("{0}"), "the subject slot binds per-viewer");
    }
}

/// **The X-1 cross-crate shape CDC — CI's producer `HumanisedRef` is byte-faithfully Git's consumer
/// `HumanisedRef`.** CI builds + serialises the summary (the opaque shape the Bus carries on the
/// CheckStatus fact); the Git consumer view decodes EXACTLY that shape. The producer half and BOTH
/// consumer halves (Notif's humanise + Git's projection) agree on the ONE frozen 5.9 `HumanisedRef`.
#[test]
fn cdc_ci_producer_summary_is_git_consumer_humanised_ref() {
    let s = ci_summary(CheckVerdict::Failure, "test/unit");
    let opaque = serde_json::to_value(&s).expect("CI serialises the summary");
    let git_view: myelin_git::check_status::HumanisedRef =
        serde_json::from_value(opaque).expect("the Git consumer decodes CI's HumanisedRef");
    assert_eq!(git_view.template_key, "ci.check.failure");
    assert_eq!(git_view.args.get("context"), Some(&"test/unit".to_string()));
}
