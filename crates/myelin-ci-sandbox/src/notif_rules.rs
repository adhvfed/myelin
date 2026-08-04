use std::collections::BTreeMap;

use myelin_notif::{
    define_notif_rule, Class, DedupTpl, HumaniseTemplate, NotifRule, NotifRuleRegistry, Reason,
    TemplateStore, DEFAULT_LOCALE, PLATFORM_DEFAULT_TENANT,
};

pub const CI_CHECK_STATUS_RULE: &str = "ci.check.status_changed";

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

pub fn register_ci_notif_rules(
    registry: &mut NotifRuleRegistry,
) -> Result<&mut NotifRuleRegistry, myelin_notif::DefineRuleError> {
    for (key, rule) in ci_notif_rules()? {
        registry.register(key, rule);
    }
    Ok(registry)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckVerdict {
    Queued,
    InProgress,
    Success,
    Failure,
    Error,
    Neutral,
    Cancelled,
}

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

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CiSummary {
    pub template_key: String,
    pub args: BTreeMap<String, String>,
}

pub fn ci_summary(verdict: CheckVerdict, context_name: &str) -> CiSummary {
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context_name.to_string());
    CiSummary {
        template_key: summary_template_key(verdict).to_string(),
        args,
    }
}

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

    #[test]
    fn ci_rule_is_table_correct_status_changed_watching() {
        let rules = ci_notif_rules().expect("CI's set is table-correct by construction");
        let keys: Vec<&str> = rules.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![CI_CHECK_STATUS_RULE]);
        let (_key, rule) = &rules[0];
        assert_eq!(rule.reason, Reason::StateChanged);
        assert_eq!(rule.default_class, Class::Watching);
        assert_eq!(rule.default_class, reason_base_class(rule.reason).1);
    }

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

    #[test]
    fn ci_re_registration_is_idempotent() {
        let mut reg = NotifRuleRegistry::new();
        register_ci_notif_rules(&mut reg).unwrap();
        register_ci_notif_rules(&mut reg).unwrap();
        assert_eq!(reg.len(), 1, "re-registering CI's set keeps one rule");
    }

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
            assert!(
                CI_SUMMARY_TEMPLATES.iter().any(|(k, _, _)| *k == key),
                "verdict {v:?} key `{key}` must have a registered template body"
            );
        }
    }

    #[test]
    fn ci_summary_is_a_humanised_ref_never_raw() {
        let s = ci_summary(CheckVerdict::Failure, "build");
        assert_eq!(s.template_key, "ci.check.failure");
        assert_eq!(s.args.get("context"), Some(&"build".to_string()));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["template_key"], "ci.check.failure");
        assert_eq!(v["args"]["context"], "build");
        assert!(
            v.get("text").is_none(),
            "no raw-string summary field exists"
        );
        let back: CiSummary = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn ci_summary_templates_register_on_the_one_surface() {
        let mut store = TemplateStore::with_platform_defaults();
        register_ci_summary_templates(&mut store);
        for (key, body, _icon) in CI_SUMMARY_TEMPLATES {
            let t = store
                .lookup(PLATFORM_DEFAULT_TENANT, key, DEFAULT_LOCALE)
                .expect("CI's summary template registered");
            assert_eq!(&t.body, body);
            assert!(t.body.contains("{0}"), "the subject slot must be present");
        }
    }
}
