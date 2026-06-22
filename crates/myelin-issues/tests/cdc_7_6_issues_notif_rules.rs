//! # The CDC pair for contract 7.6 — **Issues' owned `define_notif_rule` reason set** (ISS-P04 / P-243)
//!
//! **Contract-index row 7.6** (`define_notif_rule(reason, dedup_tpl, default_class)` — Signal class →
//! inbox reason/priority; each subsystem registers its set). The Notif seam + the registration verb +
//! the §3.1 ranking table is owned by Notif and frozen at NOTIF-P8 (`crates/myelin-notif/tests/
//! cdc_7_6_notif_define_rule.rs`); THIS file pins the **Issues slice** — the freeze ISS-P04 ships:
//! the Issues reason set (SLA at-risk / unblocked / approval-requested).
//!
//! (In contract-index terms the rule-set **PROVIDER**/producer is the subsystem — Issues here — and
//! the **CONSUMER** is Notif's registry; the two markers below carry the provider+consumer pair for
//! the coverage scanner.)
//!
//! - the **PRODUCER** (the provider side) is **Issues declaring its reason set at build time**
//!   ([`myelin_issues::declares::issue_notif_rules`]) — the three frozen Notif-owned
//!   [`myelin_notif::NotifRule`]s Issues registers, each built via the frozen
//!   [`define_notif_rule`](myelin_notif::define_notif_rule) verb so its `default_class` is RECONCILED
//!   against Notif's §3.1 table. The producer's promise: it registers exactly the three Issues
//!   reasons (arch §10 / §3.1) at their table-correct bands and NO second reason vocabulary (EI-01 §7).
//! - the **CONSUMER** is **Notif's [`NotifRuleRegistry`](myelin_notif::NotifRuleRegistry) admitting +
//!   routing** the rules: `register` accepts each under its `rule_key` (the inverse-signal seam, ZERO
//!   Notif change) and `classify(rule_key, …)` routes a Signal through the registered Issues rule.
//!
//! The two sides are pinned here so a drift on either (Issues re-bands a reason; Notif renames a
//! `NotifRule` field or changes the §3.1 table) fails this test in the same CI job. **The gate of
//! ISS-P04 is the build-time registration** — Notif admits + routes Issues' reasons; this CDC is the
//! mechanical evidence. The live trigger/SLA → Signal → inbox WIRING ("My Work") is the ISS-P22
//! follow-on.

use std::collections::BTreeMap;

use myelin_issues::declares::{
    issue_notif_rules, register_issue_notif_rules, RULE_KEY_APPROVAL_REQUESTED,
    RULE_KEY_SLA_AT_RISK, RULE_KEY_UNBLOCKED,
};
use myelin_notif::{
    define_notif_rule, Class, DedupTpl, DefineRuleError, NotifRule, NotifRuleRegistry, Reason,
};
use myelin_refs::ArtifactRef;

fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/issue/issue/ENG-1421".into())
}

/// **PRODUCER side — Issues declares the three frozen 7.6 reasons at their §3.1 bands.** Pins each
/// rule's reason + band (SLA → critical, unblocked → watching, approval-requested → critical). A
/// re-band on Issues' side would have panicked at construction (the verb reconciles); this pins the result.
#[test]
fn producer_issues_declares_the_three_reasons_at_their_bands() {
    let rules = issue_notif_rules();
    assert_eq!(rules.len(), 3, "exactly the three Issues reasons");
    let by_key: BTreeMap<&str, &NotifRule> = rules.iter().map(|(k, r)| (*k, r)).collect();

    let sla = by_key.get(RULE_KEY_SLA_AT_RISK).expect("SLA rule");
    assert_eq!(sla.reason, Reason::Sla);
    assert_eq!(sla.default_class, Class::Critical);

    let unb = by_key.get(RULE_KEY_UNBLOCKED).expect("unblocked rule");
    assert_eq!(unb.reason, Reason::Unblocked);
    assert_eq!(unb.default_class, Class::Watching);

    let appr = by_key
        .get(RULE_KEY_APPROVAL_REQUESTED)
        .expect("approval rule");
    assert_eq!(appr.reason, Reason::ApprovalRequested);
    assert_eq!(appr.default_class, Class::Critical);
}

/// **PRODUCER side — Issues uses the ONE Notif vocabulary (no second reason language).** Registering
/// an Issues reason at a band that disagrees with the §3.1 table is rejected loudly by the frozen
/// verb — Issues cannot smuggle SLA into a non-critical band.
#[test]
fn producer_issues_cannot_re_band_a_reason() {
    let err = define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Watching)
        .expect_err("SLA must register at the critical band the §3.1 table owns");
    assert!(matches!(err, DefineRuleError::ClassMismatch { .. }));
}

/// **CONSUMER side — Notif ADMITS + ROUTES Issues' reason set.** The registry accepts the three
/// rules under their `rule_key`s (the inverse-signal seam, zero Notif change) and `classify` routes a
/// Signal carrying each key through the registered Issues rule (right reason + band + a dedup key
/// that collapses by `(recipient, subject)`, `from_registered_rule = true`).
#[test]
fn consumer_notif_admits_and_routes_the_reason_set() {
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    register_issue_notif_rules(&mut reg);
    assert_eq!(
        reg.len(),
        before + 3,
        "Notif admits the three Issues rules (zero Notif change)"
    );

    let c = reg.classify(RULE_KEY_SLA_AT_RISK, "psn:alice", &subject());
    assert_eq!(c.reason, Reason::Sla);
    assert_eq!(c.default_class, Class::Critical);
    assert!(c.from_registered_rule);
    assert_eq!(
        c.dedup_key,
        "issue.sla:psn:alice:myelin://acme/issue/issue/ENG-1421"
    );

    let c = reg.classify(RULE_KEY_UNBLOCKED, "psn:bob", &subject());
    assert_eq!(c.reason, Reason::Unblocked);
    assert_eq!(c.default_class, Class::Watching);
    assert!(c.from_registered_rule);

    let c = reg.classify(RULE_KEY_APPROVAL_REQUESTED, "psn:carol", &subject());
    assert_eq!(c.reason, Reason::ApprovalRequested);
    assert_eq!(c.default_class, Class::Critical);
    assert!(c.from_registered_rule);
}
