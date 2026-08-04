use myelin_ci_sandbox::{
    ci_notif_rules, ci_summary, register_ci_notif_rules, register_ci_summary_templates,
    CheckVerdict, CI_CHECK_STATUS_RULE, CI_SUMMARY_TEMPLATES,
};
use myelin_notif::{
    reason_base_class, Class, NotifRuleRegistry, Reason, TemplateStore, DEFAULT_LOCALE,
    PLATFORM_DEFAULT_TENANT,
};
use myelin_refs::ArtifactRef;

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
    assert_eq!(rule.default_class, reason_base_class(rule.reason).1);
}

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

#[test]
fn cdc_ci_producer_summary_is_git_consumer_humanised_ref() {
    let s = ci_summary(CheckVerdict::Failure, "test/unit");
    let opaque = serde_json::to_value(&s).expect("CI serialises the summary");
    let git_view: myelin_git::check_status::HumanisedRef =
        serde_json::from_value(opaque).expect("the Git consumer decodes CI's HumanisedRef");
    assert_eq!(git_view.template_key, "ci.check.failure");
    assert_eq!(git_view.args.get("context"), Some(&"test/unit".to_string()));
}
