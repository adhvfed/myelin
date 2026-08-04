use std::collections::HashMap;

use myelin_refs::ArtifactRef;

use crate::ranking::reason_base_class;
use crate::{Class, Reason};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DedupTpl(pub String);

impl DedupTpl {
    pub fn render(&self, recipient: &str, subject: &ArtifactRef, reason: Reason) -> String {
        let src = &self.0;
        let mut out = String::with_capacity(src.len());
        let mut chars = src.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push('}');
                }
                '{' => {
                    let mut name = String::new();
                    for nc in chars.by_ref() {
                        if nc == '}' {
                            break;
                        }
                        name.push(nc);
                    }
                    out.push_str(&render_field(name.trim(), recipient, subject, reason));
                }
                other => out.push(other),
            }
        }
        out
    }
}

fn render_field(name: &str, recipient: &str, subject: &ArtifactRef, reason: Reason) -> String {
    match name {
        "subject" => subject.0.clone(),
        "recipient" => recipient.to_string(),
        "reason" => reason_token(reason).to_string(),
        _ => "<missing>".to_string(),
    }
}

fn reason_token(reason: Reason) -> &'static str {
    match reason {
        Reason::ApprovalRequested => "approval_requested",
        Reason::Escalated => "escalated",
        Reason::Sla => "sla",
        Reason::ReviewRequested => "review_requested",
        Reason::Assigned => "assigned",
        Reason::Mentioned => "mentioned",
        Reason::Replied => "replied",
        Reason::AgentProposal => "agent_proposal",
        Reason::Watched => "watched",
        Reason::StateChanged => "state_changed",
        Reason::Fyi => "fyi",
        Reason::Blocked => "blocked",
        Reason::Unblocked => "unblocked",
        Reason::ThreadWatched => "thread_watched",
        Reason::Shared => "shared",
        Reason::Comments => "comments",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifRule {
    pub reason: Reason,
    pub dedup_tpl: DedupTpl,
    pub default_class: Class,
}

impl NotifRule {
    pub fn dedup_key(&self, recipient: &str, subject: &ArtifactRef) -> String {
        self.dedup_tpl.render(recipient, subject, self.reason)
    }
}

pub fn define_notif_rule(
    reason: Reason,
    dedup_tpl: DedupTpl,
    default_class: Class,
) -> Result<NotifRule, DefineRuleError> {
    let (_base, table_class) = reason_base_class(reason);
    if default_class != table_class {
        return Err(DefineRuleError::ClassMismatch {
            reason,
            supplied: default_class,
            table: table_class,
        });
    }
    Ok(NotifRule {
        reason,
        dedup_tpl,
        default_class,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefineRuleError {
    ClassMismatch {
        reason: Reason,
        supplied: Class,
        table: Class,
    },
}

impl std::fmt::Display for DefineRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefineRuleError::ClassMismatch {
                reason,
                supplied,
                table,
            } => write!(
                f,
                "define_notif_rule: reason {reason:?} must register default_class {table:?} \
                 (the §3.1 ranking-table band), not {supplied:?}"
            ),
        }
    }
}

impl std::error::Error for DefineRuleError {}

#[derive(Clone, Debug, Default)]
pub struct NotifRuleRegistry {
    rules: HashMap<String, NotifRule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Classification {
    pub reason: Reason,
    pub default_class: Class,
    pub dedup_key: String,
    pub from_registered_rule: bool,
}

impl NotifRuleRegistry {
    pub fn new() -> NotifRuleRegistry {
        NotifRuleRegistry::default()
    }

    pub fn platform_default() -> NotifRuleRegistry {
        let mut reg = NotifRuleRegistry::new();
        for (key, rule) in platform_default_rules() {
            reg.rules.insert(key, rule);
        }
        reg
    }

    pub fn register(&mut self, rule_key: impl Into<String>, rule: NotifRule) -> &mut Self {
        self.rules.insert(rule_key.into(), rule);
        self
    }

    pub fn rule(&self, rule_key: &str) -> Option<&NotifRule> {
        self.rules.get(rule_key)
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn classify(
        &self,
        rule_key: &str,
        recipient: &str,
        subject: &ArtifactRef,
    ) -> Classification {
        match self.rules.get(rule_key) {
            Some(rule) => Classification {
                reason: rule.reason,
                default_class: rule.default_class,
                dedup_key: rule.dedup_key(recipient, subject),
                from_registered_rule: true,
            },
            None => {
                let (reason, default_class) = platform_default_reason();
                let tpl = default_dedup_tpl();
                Classification {
                    reason,
                    default_class,
                    dedup_key: format!("{rule_key}:{}", tpl.render(recipient, subject, reason)),
                    from_registered_rule: false,
                }
            }
        }
    }
}

pub fn platform_default_reason() -> (Reason, Class) {
    let reason = Reason::StateChanged;
    (reason, reason_base_class(reason).1)
}

fn default_dedup_tpl() -> DedupTpl {
    DedupTpl("{recipient}:{subject}".to_string())
}

pub fn platform_default_rules() -> Vec<(String, NotifRule)> {
    let (reason, class) = platform_default_reason();
    let rule = define_notif_rule(reason, default_dedup_tpl(), class)
        .expect("the platform-default rule is table-correct by construction");
    vec![(reason_token(reason).to_string(), rule)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject() -> ArtifactRef {
        ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())
    }

    #[test]
    fn dedup_tpl_renders_placeholders_escapes_and_missing() {
        let tpl = DedupTpl("issue:{subject}|to:{recipient}|why:{reason}".into());
        assert_eq!(
            tpl.render("psn:alice", &subject(), Reason::Mentioned),
            "issue:myelin://acme/issues/issue/PROJ-1|to:psn:alice|why:mentioned"
        );
        assert_eq!(
            DedupTpl("{nope}".into()).render("r", &subject(), Reason::Fyi),
            "<missing>"
        );
        assert_eq!(
            DedupTpl("{{literal}}".into()).render("r", &subject(), Reason::Fyi),
            "{literal}"
        );
        assert_eq!(
            DedupTpl("a}b".into()).render("r", &subject(), Reason::Fyi),
            "a}b",
            "a lone `}}` is literal (the escape guard fires only on `}}}}`)"
        );
        assert_eq!(
            DedupTpl("{subject}}".into()).render("r", &subject(), Reason::Fyi),
            "myelin://acme/issues/issue/PROJ-1}"
        );
        assert_eq!(
            DedupTpl("static-key".into()).render("r", &subject(), Reason::Fyi),
            "static-key"
        );
    }

    #[test]
    fn reason_token_is_the_snake_case_wire_form() {
        assert_eq!(
            reason_token(Reason::ApprovalRequested),
            "approval_requested"
        );
        assert_eq!(reason_token(Reason::ReviewRequested), "review_requested");
        assert_eq!(reason_token(Reason::ThreadWatched), "thread_watched");
        assert_eq!(reason_token(Reason::StateChanged), "state_changed");
        let json = serde_json::to_string(&Reason::Mentioned).unwrap();
        assert_eq!(json, "\"mentioned\"");
        assert_eq!(reason_token(Reason::Mentioned), "mentioned");
    }

    #[test]
    fn define_notif_rule_reconciles_default_class_against_the_table() {
        let rule = define_notif_rule(
            Reason::Mentioned,
            DedupTpl("{recipient}:{subject}".into()),
            Class::Direct,
        )
        .expect("mentioned → direct is the table band");
        assert_eq!(rule.reason, Reason::Mentioned);
        assert_eq!(rule.default_class, Class::Direct);

        let err = define_notif_rule(Reason::Mentioned, DedupTpl("{subject}".into()), Class::Fyi)
            .expect_err("a class that disagrees with the table band is rejected");
        assert_eq!(
            err,
            DefineRuleError::ClassMismatch {
                reason: Reason::Mentioned,
                supplied: Class::Fyi,
                table: Class::Direct,
            }
        );
        let msg = err.to_string();
        assert!(
            msg.contains("Mentioned") && msg.contains("Direct") && msg.contains("Fyi"),
            "{msg}"
        );
        assert!(
            define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Critical).is_ok()
        );
        assert!(
            define_notif_rule(Reason::Sla, DedupTpl("{subject}".into()), Class::Direct).is_err()
        );
    }

    #[test]
    fn notif_rule_dedup_key_renders_for_recipient_and_subject() {
        let rule = define_notif_rule(
            Reason::Mentioned,
            DedupTpl("mention:{subject}".into()),
            Class::Direct,
        )
        .unwrap();
        assert_eq!(
            rule.dedup_key("psn:bob", &subject()),
            "mention:myelin://acme/issues/issue/PROJ-1"
        );
    }

    #[test]
    fn registry_classifies_registered_then_falls_back_to_default() {
        let mut reg = NotifRuleRegistry::new();
        reg.register(
            "issue_mentioned",
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("mention:{recipient}:{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );

        let c = reg.classify("issue_mentioned", "psn:bob", &subject());
        assert_eq!(c.reason, Reason::Mentioned);
        assert_eq!(c.default_class, Class::Direct);
        assert_eq!(
            c.dedup_key,
            "mention:psn:bob:myelin://acme/issues/issue/PROJ-1"
        );
        assert!(
            c.from_registered_rule,
            "a registered key classifies through its rule"
        );

        let d = reg.classify("never_registered", "psn:bob", &subject());
        assert_eq!(
            d.reason,
            Reason::StateChanged,
            "the platform-default ambient reason"
        );
        assert_eq!(d.default_class, Class::Watching);
        assert!(
            !d.from_registered_rule,
            "an unregistered key uses the platform default"
        );
        assert!(
            d.dedup_key.starts_with("never_registered:"),
            "the default key namespaces by rule_key so distinct unregistered rules do not collide"
        );
    }

    #[test]
    fn registered_default_class_is_the_ranking_band_for_the_reason() {
        for (key, reason, class) in [
            ("git_review", Reason::ReviewRequested, Class::Direct),
            ("iss_sla", Reason::Sla, Class::Critical),
            ("chat_replied", Reason::Replied, Class::Participating),
            ("kn_watched", Reason::Watched, Class::Watching),
        ] {
            let mut reg = NotifRuleRegistry::new();
            reg.register(
                key,
                define_notif_rule(reason, DedupTpl("{subject}".into()), class).unwrap(),
            );
            let c = reg.classify(key, "psn:x", &subject());
            assert_eq!(c.default_class, reason_base_class(reason).1);
            assert_eq!(
                c.default_class, class,
                "the registered band drives the rank"
            );
        }
    }

    #[test]
    fn platform_default_is_stubbed_and_accretes_without_notif_change() {
        assert!(
            NotifRuleRegistry::new().is_empty(),
            "a fresh registry is empty"
        );
        assert_eq!(NotifRuleRegistry::new().len(), 0);

        let reg = NotifRuleRegistry::platform_default();
        assert_eq!(
            reg.len(),
            1,
            "the stubbed default set is exactly the one platform-default rule"
        );
        assert!(!reg.is_empty(), "the seeded default set is NOT empty");
        let seed = reg
            .rule("state_changed")
            .expect("the state_changed default rule is seeded");
        assert_eq!(seed.reason, Reason::StateChanged);
        assert_eq!(seed.default_class, Class::Watching);

        let mut reg = reg;
        reg.register(
            "git_review_requested",
            define_notif_rule(
                Reason::ReviewRequested,
                DedupTpl("{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );
        assert_eq!(
            reg.len(),
            2,
            "the per-subsystem rule accreted (no Notif enum/match edit)"
        );
    }

    #[test]
    fn synthetic_subsystem_registers_with_zero_notif_change() {
        let mut reg = NotifRuleRegistry::platform_default();
        reg.register(
            "synthetic.thing_happened",
            define_notif_rule(
                Reason::Assigned,
                DedupTpl("synthetic:{recipient}:{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );

        let c = reg.classify("synthetic.thing_happened", "psn:carol", &subject());
        assert_eq!(c.reason, Reason::Assigned);
        assert_eq!(c.default_class, Class::Direct);
        assert!(
            c.from_registered_rule,
            "the synthetic registration took effect (0 Notif change)"
        );
        assert_eq!(
            c.dedup_key,
            "synthetic:psn:carol:myelin://acme/issues/issue/PROJ-1"
        );
    }

    #[test]
    fn re_registration_is_last_write_wins() {
        let mut reg = NotifRuleRegistry::new();
        reg.register(
            "k",
            define_notif_rule(
                Reason::Assigned,
                DedupTpl("v1:{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );
        reg.register(
            "k",
            define_notif_rule(
                Reason::Mentioned,
                DedupTpl("v2:{subject}".into()),
                Class::Direct,
            )
            .unwrap(),
        );
        assert_eq!(
            reg.len(),
            1,
            "the same key is one rule (last-write-wins, idempotent)"
        );
        assert_eq!(
            reg.rule("k").unwrap().reason,
            Reason::Mentioned,
            "the latest registration wins"
        );
    }
}
