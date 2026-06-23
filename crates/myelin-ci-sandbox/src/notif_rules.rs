//! # `notif_rules` — CI registers its status-summary reasons + the `CheckStatus.summary`
//! `HumanisedRef` template set (CI-side of NOTIF-P23 / P-344, M4)
//!
//! **Consumer accretion (architecture
//! `05-refined-shared-systems-architecture/notifications.md` §3.1 / §3.3, X-1).** This is the **CI
//! half** of N-M4 "Issues + Chat + CI register" — the FINAL M4 consumer (Issues = NOTIF-P21/P-342,
//! Chat = NOTIF-P22/P-343, CI here). CI **registers** its status-summary `define_notif_rule` set
//! (contract 7.6) AND its `CheckStatus.summary` `HumanisedRef` template set on the ONE humanise
//! surface (contract 7.3); **Notif reads them, it never invents them**.
//!
//! ## The X-1 invariant this closes: `CheckStatus.summary` is a `HumanisedRef`, NEVER a raw string
//! The frozen 5.9 / X-1 `CheckStatus.summary` field is a `(template_key, args)` pair (a
//! `HumanisedRef`), never a raw `"build failed"` string (`00-reconciliation-decisions.md` X-1, line
//! 79). That is **structural** at the seam — the `summary` field is TYPED `HumanisedRef`, not
//! `String`, so a producer CANNOT smuggle a raw string through the seam (it would not compile). What
//! THIS module adds is the other half: CI registers the `template_key`s the summary points at ON
//! Notif's ONE templating surface, so the `HumanisedRef` actually **resolves through humanise**
//! (NOTIF-P9) — per-viewer, permission-safe — rather than degrading to the generic fallback display.
//! [`ci_summary`] is the producer-side constructor that builds the `(template_key, args)` pair from a
//! check verdict — every CI summary is built through it, so a raw-string summary has no code path.
//!
//! ## The inverse-signal property (EI-01 §1) — ZERO Notif code change
//! CI registers its reasons + its summary templates using ONLY the **public, frozen** Notif seams —
//! [`myelin_notif::define_notif_rule`] (7.6), [`myelin_notif::NotifRuleRegistry::register`], and
//! [`myelin_notif::HumaniseTemplate`] / [`myelin_notif::TemplateStore::put`] (7.3). **No Notif enum
//! variant was added, no Notif match arm, no Notif recompile** — the registration is a *call into a
//! data registry* and a *put into a template store*, both from THIS producer crate. If accepting
//! CI's set had required editing Notif, THIS MODULE COULD NOT COMPILE WITHOUT TOUCHING `myelin-notif`
//! — it does not (the seam is right; the third+ consumer is no harder than the first, EI-01 §1). This
//! is the SAME accretion shape Git ([`myelin_git::notif_rules`]), Knowledge, Issues
//! ([`myelin_issues::declares`]) and Chat already use.
//!
//! ## What CI registers (contract 7.6 — the status-summary reason)
//! CI's notifiable Signal class is a **check status change on a commit/PR** — the §3.1 ambient
//! `state_changed` reason ([`myelin_notif::Reason::StateChanged`] → the `watching` band): a check
//! went green/red/errored on a subject the recipient watches (a PR author / a watcher of the repo).
//! This is the bounded AMBIENT set (read-fanout / watching, §3.5) — a check outcome is not a direct
//! address, it is ambient activity on a watched subject. The Notif router classifies a CI Signal
//! carrying [`CI_CHECK_STATUS_RULE`] through CI's registered rule; the dedup template collapses a
//! storm of per-context check updates on one commit into ONE inbox row (§3.2).
//!
//! ## What CI registers on the ONE humanise surface (contract 7.3 — the summary templates)
//! Every [`CheckState`](crate::events) verdict has a stable, PII-free `template_key`
//! ([`summary_template_key`]) that maps to an ICU-subset render body
//! ([`CI_SUMMARY_TEMPLATES`]) — `{0}` is the SUBJECT slot (the `repo#commit-<oid>/check-<context>`
//! sub-anchor), resolved PER-VIEWER through Refs `resolve(Display)` so a check on a private repo
//! humanises to a tombstone for a viewer who lacks access (NOTIF-D4, 0 title/PII leak — the SAME
//! leak floor every other consumer's summary inherits, because the summary rides the ONE humanise
//! pipeline). CI [`register_ci_summary_templates`]s these as platform-default
//! ([`myelin_notif::PLATFORM_DEFAULT_TENANT`]) `en` rows; a tenant brands/localises by putting its
//! own `(tenant, key, locale)` override (the §2.5 fallback ladder is Notif's).
//!
//! ## Named floors (VISION §3)
//! - **This is the LAST M4 consumer accretion.** Issues = NOTIF-P21 (P-342), Chat = NOTIF-P22
//!   (P-343), CI = here. Each accreted the SAME way (a `register` call + template `put`s), no Notif
//!   edit.
//! - **Cross-cell inbox aggregation is NOTIF-P24 (N-M5.1)** — single-home-cell still: a CI summary
//!   resolves in the subject's home cell (the ref resolves cell-local, OQ-I); the multi-cell ambient
//!   aggregation is NOTIF-P24. Named.
//! - **Surge / erasure hardening is NOTIF-P25 / P26 / P27.** The CI-summary erasure posture (a check
//!   on an erased subject → the erased tombstone) rides the SAME references-not-payloads humanise
//!   path; the Notif erasure-residual instancing is NOTIF-P27. Named.
//! - **The live CI Signal-curation emitter** that turns a real `ci.check.updated` into a curated
//!   Signal carrying [`CI_CHECK_STATUS_RULE`] + the [`ci_summary`] `HumanisedRef` is the CI emit
//!   follow-on; this module registers the rule + the templates that emit's Signal classifies +
//!   humanises through (the producer emit body is the named CI floor — see `events.rs`).

use std::collections::BTreeMap;

use myelin_notif::{
    define_notif_rule, Class, DedupTpl, HumaniseTemplate, NotifRule, NotifRuleRegistry, Reason,
    TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};

// ===========================================================================
// Contract 7.6 — CI's define_notif_rule status-summary reason (the registration)
// ===========================================================================

/// **The `rule_key` CI's curated check-status-change Signal carries** (the `<rule>` segment of the
/// engine subject the Signal-curation emitter publishes). The Notif router classifies a CI Signal
/// under THIS key through CI's registered [`NotifRule`]. Named so the CI emit follow-on and the
/// router agree by NAME (X-5), never a literal.
pub const CI_CHECK_STATUS_RULE: &str = "ci.check.status_changed";

/// **CI's `define_notif_rule` reason set (contract 7.6) — the registration value CI hands Notif.**
/// CI's one notifiable Signal class is a **check status change** ([`Reason::StateChanged`] → the
/// ambient `watching` band, §3.1): a check went green/red/errored on a watched commit/PR. Built
/// through the FROZEN [`define_notif_rule`] verb, so the supplied `default_class` is RECONCILED
/// against Notif's §3.1 ranking table (CI registers WHICH reason; the table owns the band) — a band
/// that disagreed would fail LOUDLY here (a `Result`, surfaced not `unwrap`ped), never silently
/// mis-rank in prod.
///
/// The dedup template `ci.check:{recipient}:{subject}` collapses a storm of per-context check
/// updates on one commit into ONE inbox row per recipient (§3.2) — five contexts flipping on one
/// PR is one "checks changed on PR-9" row with `coalesce_count = N`, not five interrupts.
pub fn ci_notif_rules() -> Result<Vec<(&'static str, NotifRule)>, myelin_notif::DefineRuleError> {
    Ok(vec![(
        CI_CHECK_STATUS_RULE,
        define_notif_rule(
            Reason::StateChanged,
            DedupTpl("ci.check:{recipient}:{subject}".into()),
            Class::Watching,
        )?,
    )])
}

/// **Register CI's `define_notif_rule` set into a [`NotifRuleRegistry`] (the inverse-signal seam,
/// EI-01 §1).** A `serve` boot path that wires the Notif router holds the ONE registry; CI (like
/// every other producer subsystem) calls THIS to register its set — a data insertion, ZERO Notif
/// code change. Returns `&mut` for fluent chaining with the other producers' registrations.
/// Last-write-wins on a re-registration (idempotent on a reconnect — the rule set is declarative).
/// Surfaces a [`myelin_notif::DefineRuleError`] if CI's set ever drifts off the §3.1 table.
pub fn register_ci_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in ci_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

// ===========================================================================
// Contract 7.3 — the CheckStatus.summary HumanisedRef template set (the ONE humanise surface)
// ===========================================================================

/// **A CI check verdict — the closed set the `CheckStatus.summary` template key is selected by.**
/// Mirrors the frozen 5.9 `CheckState` (`queued | in_progress | success | failure | error | neutral
/// | cancelled`) WITHOUT depending on the Git consumer crate (X-1 acyclic: CI is the producer, it
/// must not depend on Git's consumer view). The producer maps its own check verdict onto THIS to
/// pick the summary template key — the wire `CheckState` decode is Git's consumer concern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckVerdict {
    /// The check is queued (not yet running) — pending.
    Queued,
    /// The check is running — pending.
    InProgress,
    /// The check passed.
    Success,
    /// The check failed (a test/build failure).
    Failure,
    /// The check errored (infra/runner fault, distinct from a clean failure).
    Error,
    /// Explicitly neutral — recorded, does not satisfy and does not block.
    Neutral,
    /// The check was cancelled.
    Cancelled,
}

/// **The stable, PII-free `template_key` a [`CheckVerdict`] summary points at** — the `template_key`
/// half of the `CheckStatus.summary` [`HumanisedRef`]. Keyed `ci.check.<verdict>`; resolves through
/// Notif's humanise against the [`CI_SUMMARY_TEMPLATES`] body. A stable identifier, NEVER PII and
/// NEVER a raw summary string (the raw "build failed" the X-1 decision forbids — the verdict picks a
/// KEY, humanise renders the per-viewer body).
pub fn summary_template_key(verdict: CheckVerdict) -> &'static str {
    match verdict {
        CheckVerdict::Queued => "ci.check.queued",
        CheckVerdict::InProgress => "ci.check.in_progress",
        CheckVerdict::Success => "ci.check.success",
        CheckVerdict::Failure => "ci.check.failure",
        CheckVerdict::Error => "ci.check.error",
        CheckVerdict::Neutral => "ci.check.neutral",
        CheckVerdict::Cancelled => "ci.check.cancelled",
    }
}

/// **The `CheckStatus.summary` `HumanisedRef` shape CI produces (the `(template_key, args)` pair,
/// X-1 / 5.9).** This is the PRODUCER-side mirror of the consumer's `myelin_git::check_status::
/// HumanisedRef` — CI builds the `(template_key, args)` pair WITHOUT depending on the Git consumer
/// crate (X-1 acyclic). It serialises to EXACTLY `{template_key, args}` (the frozen 5.9 field set
/// the Bus carries opaque + the Git consumer decodes), so the producer half and the consumer half
/// agree on the ONE shape (proven by the CDC). It is structurally a `(template_key, args)` pair —
/// there is NO `String` variant, so a raw-string summary cannot be built.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CiSummary {
    /// The humanisation template key (e.g. `ci.check.failure`) — resolves through Notif's humanise.
    pub template_key: String,
    /// The template args (`name → value`) the humanisation fills. PII-free identifiers/labels.
    pub args: BTreeMap<String, String>,
}

/// **Build the `CheckStatus.summary` `HumanisedRef` for a check verdict (the ONLY summary path).**
/// Every CI summary is built through THIS — it returns a `(template_key, args)` pair (the
/// [`CiSummary`]), NEVER a raw string. The `template_key` is [`summary_template_key`] for the
/// verdict (registered on the ONE humanise surface by [`register_ci_summary_templates`]); the `args`
/// carry the PII-free context label (the check context name, e.g. `build`) the template body binds.
/// Because there is no raw-string summary constructor anywhere in CI, a raw-string summary has no
/// code path — the X-1 "never a raw string" invariant is enforced by construction.
pub fn ci_summary(verdict: CheckVerdict, context_name: &str) -> CiSummary {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context_name.to_string());
    CiSummary {
        template_key: summary_template_key(verdict).to_string(),
        args,
    }
}

/// **CI's status-summary render bodies (contract 7.3 — the ICU-subset templates CI registers on the
/// ONE humanise surface).** `(template_key, body, icon)`. `{0}` is the SUBJECT slot (the
/// `repo#commit-<oid>/check-<context>` sub-anchor, resolved PER-VIEWER → the check's title or a
/// tombstone). ICU-subset bodies; a tenant overrides by putting its own `(tenant, key, locale)`
/// row. These are the bodies a `CheckStatus.summary` [`HumanisedRef`] resolves to through humanise —
/// permission-safe by construction (a check on a private repo → a tombstone, the title never leaks,
/// NOTIF-D4).
pub const CI_SUMMARY_TEMPLATES: &[(&str, &str, &str)] = &[
    ("ci.check.queued", "Checks queued on {0}", "ci-queued"),
    (
        "ci.check.in_progress",
        "Checks running on {0}",
        "ci-running",
    ),
    ("ci.check.success", "Checks passed on {0}", "ci-success"),
    ("ci.check.failure", "Checks failed on {0}", "ci-failure"),
    ("ci.check.error", "Checks errored on {0}", "ci-error"),
    ("ci.check.neutral", "Checks neutral on {0}", "ci-neutral"),
    (
        "ci.check.cancelled",
        "Checks cancelled on {0}",
        "ci-cancelled",
    ),
];

/// **Register CI's status-summary templates into a Notif [`TemplateStore`] (the 7.3 accretion seam,
/// EI-01 §1).** A `serve` boot path that wires the humaniser holds the ONE template store; CI calls
/// THIS to register its summary templates as platform-default ([`PLATFORM_DEFAULT_TENANT`]) `en`
/// rows — a template `put`, ZERO Notif code change. Returns `&mut` for fluent chaining. Idempotent
/// on a re-registration (last-write-wins per key). After this, every CI `CheckStatus.summary`
/// `HumanisedRef` resolves through humanise to a registered body (not the generic fallback).
pub fn register_ci_summary_templates(store: &mut TemplateStore) -> &mut TemplateStore {
    for (key, body, icon) in CI_SUMMARY_TEMPLATES {
        store.put(HumaniseTemplate {
            tenant: PLATFORM_DEFAULT_TENANT.to_string(),
            template_key: (*key).to_string(),
            locale: DEFAULT_LOCALE.to_string(),
            body: (*body).to_string(),
            icon: (*icon).to_string(),
        });
    }
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_notif::reason_base_class;

    /// **CI's `define_notif_rule` set is built through the FROZEN verb + reconciles against Notif's
    /// §3.1 table (the registration is table-correct).** The rule's `default_class` is EXACTLY the
    /// band Notif's ranking table assigns the reason — CI registers the reason, Notif owns the band.
    #[test]
    fn ci_rule_is_table_correct_status_changed_watching() {
        let rules = ci_notif_rules().expect("CI's set is table-correct by construction");
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![CI_CHECK_STATUS_RULE]);
        let (_key, rule) = &rules[0];
        assert_eq!(rule.reason, Reason::StateChanged);
        assert_eq!(rule.default_class, Class::Watching);
        // the registered default_class is EXACTLY the §3.1 ranking-table band for the reason.
        assert_eq!(rule.default_class, reason_base_class(rule.reason).1);
    }

    /// **THE INVERSE-SIGNAL PROPERTY (EI-01 §1): CI registers + the router classifies a CI Signal —
    /// with ZERO Notif code change.** CI uses ONLY the public seam; the platform-default registry
    /// accretes CI's set with no Notif edit, and a Signal carrying CI's rule_key classifies through
    /// CI's rule. If accepting CI's registration required a Notif change, this test could not
    /// compile without editing `myelin-notif` — it does not.
    #[test]
    fn ci_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        let before = reg.len();
        register_ci_notif_rules(&mut reg).expect("CI's set registers");
        assert_eq!(
            reg.len(),
            before + 1,
            "CI's rule accreted (no Notif enum/match edit)"
        );

        // the router classifies a CI check-status Signal through CI's registered rule.
        let subject = myelin_refs::ArtifactRef("myelin://acme/git/pr/9".into());
        let c = reg.classify(CI_CHECK_STATUS_RULE, "psn:author", &subject);
        assert_eq!(c.reason, Reason::StateChanged);
        assert_eq!(c.default_class, Class::Watching);
        assert!(
            c.from_registered_rule,
            "the CI registration took effect (0 Notif change)"
        );
        assert_eq!(c.dedup_key, "ci.check:psn:author:myelin://acme/git/pr/9");
    }

    /// **Re-registration is idempotent (last-write-wins) — a reconnecting CI declaring its set does
    /// not double it.**
    #[test]
    fn ci_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_ci_notif_rules(&mut reg).unwrap();
        register_ci_notif_rules(&mut reg).unwrap();
        assert_eq!(reg.len(), 1, "re-registering CI's set keeps one rule");
    }

    /// **Every check verdict maps to a stable, PII-free summary template key** — the closed set, no
    /// raw string. The keys are exactly the `ci.check.<verdict>` vocabulary the templates register.
    #[test]
    fn every_verdict_has_a_summary_template_key() {
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
            let key = summary_template_key(v);
            assert!(key.starts_with("ci.check."));
            // the key is one of the registered template bodies.
            assert!(
                CI_SUMMARY_TEMPLATES.iter().any(|(k, _, _)| *k == key),
                "verdict {v:?} key `{key}` must have a registered template body"
            );
        }
    }

    /// **`ci_summary` builds a `(template_key, args)` pair — NEVER a raw string (the X-1
    /// invariant).** The summary is structurally a [`CiSummary`] (template_key + args); there is no
    /// raw-string constructor. The serialised shape is exactly `{template_key, args}` — the frozen
    /// 5.9 `HumanisedRef` field set the Bus carries + the Git consumer decodes.
    #[test]
    fn ci_summary_is_a_humanised_ref_never_raw() {
        let s = ci_summary(CheckVerdict::Failure, "build");
        assert_eq!(s.template_key, "ci.check.failure");
        assert_eq!(s.args.get("context"), Some(&"build".to_string()));
        // serialises to exactly the frozen 5.9 HumanisedRef field set.
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["template_key"], "ci.check.failure");
        assert_eq!(v["args"]["context"], "build");
        assert!(
            v.get("text").is_none(),
            "no raw-string summary field exists"
        );
        // round-trips.
        let back: CiSummary = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    /// **CI registers its summary templates with ZERO Notif change (the 7.3 accretion seam).** The
    /// platform-default store accretes CI's seven verdict bodies as a data `put`, no Notif edit.
    #[test]
    fn ci_summary_templates_register_on_the_one_surface() {
        let mut store = TemplateStore::with_platform_defaults();
        register_ci_summary_templates(&mut store);
        // every CI summary key now resolves to a registered body (not the generic fallback).
        for (key, body, _icon) in CI_SUMMARY_TEMPLATES {
            let t = store
                .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
                .expect("CI's summary template registered");
            assert_eq!(&t.body, body);
            // the `{0}` subject slot is present so the per-viewer resolve binds it (NOTIF-D4).
            assert!(t.body.contains("{0}"), "the subject slot must be present");
        }
    }
}
