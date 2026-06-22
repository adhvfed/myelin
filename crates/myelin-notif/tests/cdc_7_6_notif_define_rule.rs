//! # CDC — contract 7.6 `define_notif_rule` (the registration seam) (P-186 / NOTIF-P8)
//!
//! **Architecture:** `notifications.md` §3.1 (the `reason → base → class` table the ranking reads —
//! a rule's `default_class` is the band a Signal of that reason ranks in), §3.4 step-2 (the router
//! `classify reason → score → dedup/storm-control collapse`: it classifies a curated Signal's reason
//! **through a registered rule**). **Contract:** **7.6** `define_notif_rule(reason, dedup_tpl,
//! default_class)` — Signal class → inbox reason/priority; **each subsystem registers its set**
//! (Issues SLA/unblocked/approval; KN mentions/comments/shares/watched; Chat
//! mentioned/replied/thread_watched/approval). Owned by Notif. **Reconciliation:** OQ1 (the default
//! set is CONFIRM — its content is the M3/M4 per-subsystem enumeration, NOTIF-P19..P23).
//!
//! This CDC pins the 7.6 seam from BOTH sides:
//!
//! - **PROVIDER (Notif owns 7.6):** `define_notif_rule` constructs a rule; the `NotifRuleRegistry`
//!   classifies a Signal carrying a registered `rule_key` into the rule's reason + `default_class` +
//!   rendered §3.2 dedup key; an unregistered key falls back to the platform default (never a panic /
//!   never a silent drop). The `default_class` is RECONCILED against the §3.1 ranking table (a
//!   subsystem cannot smuggle a reason into the wrong band).
//! - **CONSUMER (a subsystem — Git/KN/Issues/Chat/CI, NOTIF-P19..P23):** a subsystem registers its
//!   rule set by CALLING `define_notif_rule` + `register` — with ZERO Notif code change (no new enum
//!   variant, no new match arm, no Notif recompile). The inverse-signal property (EI-01 §1) is the
//!   wire: if accepting a registration required a Notif change, the consumer half could not compile
//!   without editing Notif. It does not.
//!
//! The two halves agree on the WIRE: the `(reason, dedup_tpl, default_class)` rule shape + the
//! `register(rule_key, rule)` / `classify(rule_key, recipient, subject)` seam. A drift on either side
//! (a verb that stops reconciling the band, a registry that becomes a hard-coded match) breaks THIS
//! build.

use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::define_rule::{
    define_notif_rule, platform_default_reason, DedupTpl, DefineRuleError, NotifRuleRegistry,
};
use myelin_notif::ranking::{reason_base_class, DeterministicV1, RankStrategy};
use myelin_notif::router::RoutedInboxItem;
use myelin_notif::{Class, Reason};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn viewer() -> Principal {
    Principal::stub(PrincipalId("u1".into()), PrincipalKind::Human, tenant())
}
fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())
}

// ===========================================================================================
//  PROVIDER (Notif owns 7.6): the verb constructs a rule; the registry classifies a Signal.
// ===========================================================================================

/// **PROVIDER: `define_notif_rule(reason, dedup_tpl, default_class)` constructs the rule with the
/// frozen three fields and renders the §3.2 dedup key.** The verb is the seam's construction half.
#[test]
fn provider_define_notif_rule_constructs_the_rule_shape() {
    let rule = define_notif_rule(
        Reason::Mentioned,
        DedupTpl("mention:{recipient}:{subject}".into()),
        Class::Direct,
    )
    .expect("mentioned → direct is the §3.1 table band");
    assert_eq!(rule.reason, Reason::Mentioned);
    assert_eq!(rule.default_class, Class::Direct);
    assert_eq!(
        rule.dedup_key("psn:bob", &subject()),
        "mention:psn:bob:myelin://acme/issues/issue/PROJ-1",
        "the dedup_tpl renders the §3.2 collapse key for (recipient, subject)"
    );
}

/// **PROVIDER: the registry classifies a registered Signal through its rule, and an unregistered
/// key onto the platform default (never a panic / never a silent drop).** The router step-2
/// `classify reason` seam.
#[test]
fn provider_registry_classifies_registered_then_default() {
    let mut reg = NotifRuleRegistry::new();
    reg.register(
        "iss_sla_breach",
        define_notif_rule(
            Reason::Sla,
            DedupTpl("sla:{subject}".into()),
            Class::Critical,
        )
        .unwrap(),
    );

    let c = reg.classify("iss_sla_breach", "psn:bob", &subject());
    assert_eq!(c.reason, Reason::Sla);
    assert_eq!(c.default_class, Class::Critical);
    assert_eq!(c.dedup_key, "sla:myelin://acme/issues/issue/PROJ-1");
    assert!(c.from_registered_rule);

    // an unregistered key → the platform default (ambient watching), never a panic.
    let (default_reason, default_class) = platform_default_reason();
    let d = reg.classify("not.registered", "psn:bob", &subject());
    assert_eq!(d.reason, default_reason);
    assert_eq!(d.default_class, default_class);
    assert!(!d.from_registered_rule);
}

/// **PROVIDER: the `default_class` a rule registers is RECONCILED against the §3.1 ranking table —
/// a mismatched class is rejected loudly.** Notif owns the ONE ranking table; a subsystem registers
/// the reason. This is the 0-leak of the NOTIF-D1 invariant onto the registration seam.
#[test]
fn provider_define_notif_rule_reconciles_class_against_the_ranking_table() {
    // table-correct passes.
    assert!(define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Critical).is_ok());
    // a class that disagrees with the table band is rejected (never silently re-banded).
    let err = define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Fyi)
        .expect_err("sla → fyi disagrees with the §3.1 table (sla is critical)");
    assert_eq!(
        err,
        DefineRuleError::ClassMismatch {
            reason: Reason::Sla,
            supplied: Class::Fyi,
            table: Class::Critical,
        }
    );
}

/// **PROVIDER↔NOTIF-P7: the registered `default_class` DRIVES the ranking band.** A Signal
/// classified through a rule ranks in EXACTLY the §3.1 band its reason maps to — the seam and the
/// ranking agree on the table. This is the `default_class → ranking` wiring the prompt names.
#[test]
fn provider_registered_class_drives_the_ranking_band() {
    let mut reg = NotifRuleRegistry::new();
    reg.register(
        "git_review",
        define_notif_rule(
            Reason::ReviewRequested,
            DedupTpl("{subject}".into()),
            Class::Direct,
        )
        .unwrap(),
    );
    let c = reg.classify("git_review", "psn:bob", &subject());

    // build the inbox row the classification would produce, and rank it.
    let item = RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: "itm-1".into(),
        recipient: "psn:bob".into(),
        subject: subject(),
        reason: c.reason,
        class: c.default_class,
        origin_event: ArtifactRef("myelin://acme/bus/event/e1".into()),
        dedup_key: c.dedup_key.clone(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    };
    let (priority, trace) = DeterministicV1::default().score(&viewer(), &item);
    // the rank's class is the registered default_class, and the §3.1 table agrees.
    assert_eq!(trace.class, c.default_class);
    assert_eq!(trace.class, reason_base_class(c.reason).1);
    assert_eq!(
        priority,
        reason_base_class(c.reason).0,
        "the base from the table drives the rank"
    );
}

// ===========================================================================================
//  CONSUMER (a subsystem): registers its set with ZERO Notif change (the inverse-signal seam).
// ===========================================================================================

/// **CONSUMER: a synthetic subsystem (the stand-in for any of NOTIF-P19..P23) registers a brand-new
/// rule set and the router classifies its Signals — with ZERO Notif code change.** This test uses
/// ONLY the public 7.6 seam (`define_notif_rule` + `register` + `classify`); it touches no Notif
/// internal type and extends no Notif enum. If accepting a registration required a Notif change,
/// this test could not compile without editing Notif. It does not — the inverse-signal property.
#[test]
fn consumer_subsystem_registers_a_whole_set_with_zero_notif_change() {
    // A synthetic "Acme Widgets" subsystem registers THREE reasons — exactly as Git/KN/Issues/Chat/CI
    // will in NOTIF-P19..P23 — by CALLING the seam. No Notif arm is added; this is data.
    let mut reg = NotifRuleRegistry::platform_default();
    let before = reg.len();
    reg.register(
        "widgets.assigned",
        define_notif_rule(
            Reason::Assigned,
            DedupTpl("w:assign:{subject}".into()),
            Class::Direct,
        )
        .unwrap(),
    )
    .register(
        "widgets.escalated",
        define_notif_rule(
            Reason::Escalated,
            DedupTpl("w:esc:{subject}".into()),
            Class::Critical,
        )
        .unwrap(),
    )
    .register(
        "widgets.watched",
        define_notif_rule(
            Reason::Watched,
            DedupTpl("w:watch:{recipient}".into()),
            Class::Watching,
        )
        .unwrap(),
    );
    assert_eq!(
        reg.len(),
        before + 3,
        "three subsystem rules accreted (no Notif enum/match edit)"
    );

    // the router classifies each of the synthetic Signals through the registered rules.
    let assigned = reg.classify("widgets.assigned", "psn:dana", &subject());
    assert_eq!(assigned.reason, Reason::Assigned);
    assert_eq!(assigned.default_class, Class::Direct);
    assert!(assigned.from_registered_rule);

    let escalated = reg.classify("widgets.escalated", "psn:dana", &subject());
    assert_eq!(escalated.reason, Reason::Escalated);
    assert_eq!(escalated.default_class, Class::Critical);

    let watched = reg.classify("widgets.watched", "psn:dana", &subject());
    assert_eq!(watched.reason, Reason::Watched);
    assert_eq!(watched.dedup_key, "w:watch:psn:dana");
}

/// **CONSUMER↔PROVIDER agree on the WIRE: the rule the consumer registers is the rule the provider
/// classifies (the round-trip).** The consumer's `(reason, dedup_tpl, default_class)` is exactly
/// what the provider returns from `classify` — no field is dropped or re-derived inconsistently.
#[test]
fn consumer_provider_round_trip_on_the_rule_wire() {
    let mut reg = NotifRuleRegistry::new();
    let rule = define_notif_rule(
        Reason::ApprovalRequested,
        DedupTpl("approval:{recipient}:{subject}".into()),
        Class::Critical,
    )
    .unwrap();
    reg.register("agent.approval", rule.clone());

    let c = reg.classify("agent.approval", "psn:agent-owner", &subject());
    assert_eq!(
        c.reason, rule.reason,
        "the classified reason is the registered reason"
    );
    assert_eq!(
        c.default_class, rule.default_class,
        "the classified band is the registered band"
    );
    assert_eq!(
        c.dedup_key,
        rule.dedup_key("psn:agent-owner", &subject()),
        "the classified dedup key is the registered template rendered for (recipient, subject)"
    );
}
