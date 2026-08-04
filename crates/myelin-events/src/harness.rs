use crate::taxonomy::{self, TaxonomyError};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PayloadShape {
    ReferencesOnly,
    InlinePersonalData,
    EphemeralFirehose,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisteredToken {
    pub name: String,
    pub current_schema_ver: u32,
    pub shape: PayloadShape,
}

impl RegisteredToken {
    pub fn references_only(name: impl Into<String>) -> RegisteredToken {
        RegisteredToken {
            name: name.into(),
            current_schema_ver: 1,
            shape: PayloadShape::ReferencesOnly,
        }
    }

    pub fn inline_personal_data(name: impl Into<String>) -> RegisteredToken {
        RegisteredToken {
            name: name.into(),
            current_schema_ver: 1,
            shape: PayloadShape::InlinePersonalData,
        }
    }

    pub fn firehose(name: impl Into<String>) -> RegisteredToken {
        RegisteredToken {
            name: name.into(),
            current_schema_ver: 1,
            shape: PayloadShape::EphemeralFirehose,
        }
    }

    pub fn at_schema_ver(mut self, v: u32) -> RegisteredToken {
        self.current_schema_ver = v;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubsystemTokenList {
    pub subsystem: String,
    pub tokens: Vec<RegisteredToken>,
}

impl SubsystemTokenList {
    pub fn new(subsystem: impl Into<String>, tokens: Vec<RegisteredToken>) -> SubsystemTokenList {
        SubsystemTokenList {
            subsystem: subsystem.into(),
            tokens,
        }
    }

    pub fn references_only(subsystem: impl Into<String>, names: &[&str]) -> SubsystemTokenList {
        SubsystemTokenList {
            subsystem: subsystem.into(),
            tokens: names
                .iter()
                .map(|n| RegisteredToken::references_only(*n))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HarnessError {
    UngrammaticalToken {
        subsystem: String,
        token: String,
        cause: TaxonomyError,
    },
    ForeignPrefix {
        subsystem: String,
        token: String,
    },
    UnknownSubsystem {
        subsystem: String,
    },
    DuplicateToken {
        subsystem: String,
        token: String,
    },
    ZeroSchemaVer {
        subsystem: String,
        token: String,
    },
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarnessError::UngrammaticalToken {
                subsystem,
                token,
                cause,
            } => write!(
                f,
                "subsystem `{subsystem}`: registered token `{token}` is ungrammatical: {cause}"
            ),
            HarnessError::ForeignPrefix { subsystem, token } => write!(
                f,
                "subsystem `{subsystem}`: token `{token}` does not carry the `{subsystem}.` prefix \
                 (a subsystem may only register names under its own §6.2 prefix)"
            ),
            HarnessError::UnknownSubsystem { subsystem } => write!(
                f,
                "`{subsystem}` is not a canonical §6.2 subsystem token \
                 (git/ci/issue/knowledge/chat/identity/refs)"
            ),
            HarnessError::DuplicateToken { subsystem, token } => write!(
                f,
                "subsystem `{subsystem}`: token `{token}` is registered more than once \
                 (each name is minted exactly once)"
            ),
            HarnessError::ZeroSchemaVer { subsystem, token } => write!(
                f,
                "subsystem `{subsystem}`: token `{token}` has schema_ver 0 (a live version is ≥ 1)"
            ),
        }
    }
}

impl std::error::Error for HarnessError {}

#[derive(Debug, Default)]
pub struct TokenListHarness {
    by_name: BTreeMap<String, (String, RegisteredToken)>,
}

impl TokenListHarness {
    pub fn new() -> TokenListHarness {
        TokenListHarness::default()
    }

    pub fn register(&mut self, list: &SubsystemTokenList) -> Result<usize, HarnessError> {
        if !taxonomy::SUBSYSTEM_TOKENS.contains(&list.subsystem.as_str()) {
            return Err(HarnessError::UnknownSubsystem {
                subsystem: list.subsystem.clone(),
            });
        }
        let prefix = format!("{}.", list.subsystem);
        let mut seen_in_list: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for tok in &list.tokens {
            taxonomy::validate(&tok.name).map_err(|cause| HarnessError::UngrammaticalToken {
                subsystem: list.subsystem.clone(),
                token: tok.name.clone(),
                cause,
            })?;
            if !tok.name.starts_with(&prefix) {
                return Err(HarnessError::ForeignPrefix {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
            if tok.current_schema_ver == 0 {
                return Err(HarnessError::ZeroSchemaVer {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
            if !seen_in_list.insert(tok.name.as_str()) {
                return Err(HarnessError::DuplicateToken {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
            if self.by_name.contains_key(&tok.name) {
                return Err(HarnessError::DuplicateToken {
                    subsystem: list.subsystem.clone(),
                    token: tok.name.clone(),
                });
            }
        }
        for tok in &list.tokens {
            self.by_name
                .insert(tok.name.clone(), (list.subsystem.clone(), tok.clone()));
        }
        Ok(list.tokens.len())
    }

    pub fn add(&mut self, subsystem: &str, tok: RegisteredToken) -> Result<(), HarnessError> {
        let list = SubsystemTokenList {
            subsystem: subsystem.to_string(),
            tokens: vec![tok],
        };
        self.register(&list).map(|_| ())
    }

    pub fn is_registered(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub fn lookup(&self, name: &str) -> Option<(&str, &RegisteredToken)> {
        self.by_name.get(name).map(|(sub, tok)| (sub.as_str(), tok))
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    pub fn names_for(&self, subsystem: &str) -> Vec<&str> {
        self.by_name
            .iter()
            .filter(|(_, (sub, _))| sub == subsystem)
            .map(|(name, _)| name.as_str())
            .collect()
    }

    pub fn registered_subsystems(&self) -> Vec<&str> {
        let mut subs: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (sub, _) in self.by_name.values() {
            subs.insert(sub.as_str());
        }
        subs.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_list() -> SubsystemTokenList {
        SubsystemTokenList::references_only(
            "git",
            &[
                "git.ref.updated",
                "git.pr.opened",
                "git.pr.merged",
                "git.repo.snapshot",
            ],
        )
    }

    fn kn_list() -> SubsystemTokenList {
        SubsystemTokenList::references_only(
            "knowledge",
            &[
                "knowledge.page.created",
                "knowledge.page.updated",
                "knowledge.page.snapshot",
            ],
        )
    }

    #[test]
    fn harness_admits_a_full_well_formed_list() {
        let mut h = TokenListHarness::new();
        assert_eq!(
            h.register(&git_list()).unwrap(),
            4,
            "the whole git list is admitted"
        );
        assert_eq!(
            h.register(&kn_list()).unwrap(),
            3,
            "the whole KN list is admitted"
        );
        assert!(h.is_registered("git.ref.updated"));
        assert!(h.is_registered("knowledge.page.snapshot"));
        assert_eq!(h.len(), 7);
        assert_eq!(h.registered_subsystems(), vec!["git", "knowledge"]);
    }

    #[test]
    fn harness_rejects_a_malformed_addition() {
        let mut h = TokenListHarness::new();
        h.register(&git_list()).unwrap();

        assert!(matches!(
            h.add("git", RegisteredToken::references_only("git.pr.open")),
            Err(HarnessError::UngrammaticalToken {
                cause: TaxonomyError::PresentTenseVerb { .. },
                ..
            })
        ));
        assert!(matches!(
            h.add("git", RegisteredToken::references_only("git.PR.opened")),
            Err(HarnessError::UngrammaticalToken {
                cause: TaxonomyError::BadToken { .. },
                ..
            })
        ));
        assert_eq!(
            h.len(),
            4,
            "a rejected addition leaves the harness unchanged"
        );
        assert!(!h.is_registered("git.pr.open"));
    }

    #[test]
    fn a_subsystem_cannot_register_a_foreign_prefix_name() {
        let mut h = TokenListHarness::new();
        let bad = SubsystemTokenList::references_only("git", &["ci.check.updated"]);
        assert!(matches!(
            h.register(&bad),
            Err(HarnessError::ForeignPrefix { subsystem, token })
                if subsystem == "git" && token == "ci.check.updated"
        ));
        assert!(
            h.is_empty(),
            "a list with a foreign-prefix name registers NOTHING (all-or-nothing)"
        );
    }

    #[test]
    fn an_unknown_subsystem_is_rejected() {
        let mut h = TokenListHarness::new();
        let bad = SubsystemTokenList::references_only("billing", &["billing.invoice.created"]);
        assert!(matches!(
            h.register(&bad),
            Err(HarnessError::UnknownSubsystem { .. })
        ));
    }

    #[test]
    fn a_cross_subsystem_name_collision_is_rejected() {
        let mut h = TokenListHarness::new();
        h.register(&SubsystemTokenList::references_only(
            "git",
            &["git.ref.updated"],
        ))
        .unwrap();
        assert!(matches!(
            h.register(&SubsystemTokenList::references_only(
                "git",
                &["git.ref.updated"]
            )),
            Err(HarnessError::DuplicateToken { .. })
        ));
        let dup = SubsystemTokenList::references_only(
            "knowledge",
            &["knowledge.page.created", "knowledge.page.created"],
        );
        assert!(matches!(
            h.register(&dup),
            Err(HarnessError::DuplicateToken { .. })
        ));
        assert!(
            h.names_for("knowledge").is_empty(),
            "the duplicate list registered nothing"
        );
    }

    #[test]
    fn schema_ver_lineage_is_held_and_must_be_at_least_one() {
        let mut h = TokenListHarness::new();
        let list = SubsystemTokenList::new(
            "knowledge",
            vec![RegisteredToken::references_only("knowledge.page.updated").at_schema_ver(3)],
        );
        h.register(&list).unwrap();
        let (sub, tok) = h.lookup("knowledge.page.updated").unwrap();
        assert_eq!(sub, "knowledge");
        assert_eq!(
            tok.current_schema_ver, 3,
            "the schema_ver lineage tip is held"
        );

        let bad = SubsystemTokenList::new(
            "knowledge",
            vec![RegisteredToken::references_only("knowledge.row.updated").at_schema_ver(0)],
        );
        assert!(matches!(
            h.register(&bad),
            Err(HarnessError::ZeroSchemaVer { .. })
        ));
    }

    #[test]
    fn payload_shape_descriptor_is_held_per_name() {
        let mut h = TokenListHarness::new();
        let list = SubsystemTokenList::new(
            "chat",
            vec![
                RegisteredToken::references_only("chat.message.created"),
                RegisteredToken::firehose("chat.message.typing"),
            ],
        );
        h.register(&list).unwrap();
        assert_eq!(
            h.lookup("chat.message.created").unwrap().1.shape,
            PayloadShape::ReferencesOnly
        );
        assert_eq!(
            h.lookup("chat.message.typing").unwrap().1.shape,
            PayloadShape::EphemeralFirehose
        );
    }
}
