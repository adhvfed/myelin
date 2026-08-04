pub const SUBSYSTEM_TOKENS: &[&str] = &[
    "git",
    "ci",
    "issue",
    "knowledge",
    "chat",
    "identity",
    "refs",
];

pub const ARTIFACT_TYPE_TOKENS: &[&str] = &[
    "pr",
    "ref",
    "review",
    "comment",
    "repo",
    "commit",
    "blob",
    "run",
    "check",
    "log",
    "artifact",
    "deployment",
    "pipeline",
    "runner",
    "issue",
    "initiative",
    "relation",
    "page",
    "doc",
    "row",
    "channel",
    "message",
    "thread",
    "read_state",
    "permission",
    "member",
    "edge",
];

pub mod new_tokens {
    pub const CI_CHECK_UPDATED: &str = "ci.check.updated";
    pub const CI_RESULT: &str = "ci.result";
    pub const ISSUE_INITIATIVE_CREATED: &str = "issue.initiative.created";
}

pub const SEED_EVENT_NAMES: &[&str] = &[
    "git.pr.opened",
    "git.pr.updated",
    "git.pr.closed",
    "git.pr.merged",
    "git.pr.reopened",
    "git.pr.marked_ready",
    "git.ref.updated",
    "git.review.submitted",
    "git.comment.created",
    "ci.run.started",
    "ci.run.passed",
    "ci.run.failed",
    "ci.run.cancelled",
    new_tokens::CI_CHECK_UPDATED,
    new_tokens::CI_RESULT,
    "ci.log.available",
    "ci.artifact.published",
    "issue.issue.created",
    "issue.issue.updated",
    "issue.issue.transitioned",
    "issue.issue.closed",
    new_tokens::ISSUE_INITIATIVE_CREATED,
    "issue.initiative.updated",
    "issue.relation.created",
    "knowledge.page.created",
    "knowledge.page.updated",
    "knowledge.doc.updated",
    "knowledge.row.updated",
    "chat.message.created",
    "chat.read_state.updated",
    "identity.permission.granted",
    "identity.permission.revoked",
    "identity.member.added",
    "refs.edge.created",
    "refs.edge.removed",
    "git.repo.erased",
    "knowledge.page.snapshot",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaxonomyError {
    TooFewSegments { name: String, segments: usize },
    TooManySegments { name: String, segments: usize },
    EmptySegment { name: String },
    BadToken { name: String, token: String },
    UnknownSubsystem { name: String, token: String },
    PresentTenseVerb { name: String, token: String },
    PluralToken { name: String, token: String },
}

impl std::fmt::Display for TaxonomyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaxonomyError::TooFewSegments { name, segments } => write!(
                f,
                "`{name}`: {segments} segment(s) - the grammar needs ≥2 (<subsystem>.<event_name>)"
            ),
            TaxonomyError::TooManySegments { name, segments } => write!(
                f,
                "`{name}`: {segments} segments - the grammar is at most 3 (<sub>.<type>.<event>)"
            ),
            TaxonomyError::EmptySegment { name } => {
                write!(
                    f,
                    "`{name}`: has an empty segment (a leading/trailing/doubled dot)"
                )
            }
            TaxonomyError::BadToken { name, token } => write!(
                f,
                "`{name}`: token `{token}` does not match [a-z][a-z0-9_]* (lowercase, no \
                 leading digit, no hyphen)"
            ),
            TaxonomyError::UnknownSubsystem { name, token } => write!(
                f,
                "`{name}`: `{token}` is not a canonical subsystem token \
                 (git/ci/issue/knowledge/chat/identity/refs)"
            ),
            TaxonomyError::PresentTenseVerb { name, token } => write!(
                f,
                "`{name}`: verb `{token}` looks present-tense - event verbs are past-tense \
                 (e.g. `opened`, not `open`)"
            ),
            TaxonomyError::PluralToken { name, token } => write!(
                f,
                "`{name}`: token `{token}` looks plural - taxonomy tokens are singular \
                 (e.g. `comment`, not `comments`)"
            ),
        }
    }
}

fn token_is_well_formed(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn looks_present_tense(token: &str) -> bool {
    const PRESENT_VERBS: &[&str] = &[
        "create",
        "update",
        "delete",
        "merge",
        "close",
        "reopen",
        "open",
        "start",
        "pass",
        "fail",
        "cancel",
        "submit",
        "transition",
        "grant",
        "revoke",
        "add",
        "remove",
        "erase",
        "publish",
        "snapshot",
        "invoke",
    ];
    const LEGIT_NON_VERB: &[&str] = &["snapshot", "published", "available"];
    if LEGIT_NON_VERB.contains(&token) {
        return false;
    }
    PRESENT_VERBS.contains(&token)
}

fn looks_plural(token: &str) -> bool {
    const LEGIT_S_TOKENS: &[&str] = &["pass", "status", "read_state", "progress"];
    if LEGIT_S_TOKENS.contains(&token)
        || SUBSYSTEM_TOKENS.contains(&token)
        || ARTIFACT_TYPE_TOKENS.contains(&token)
    {
        return false;
    }
    let Some(singular) = token.strip_suffix('s') else {
        return false;
    };
    if singular.is_empty() {
        return false;
    }
    SUBSYSTEM_TOKENS.contains(&singular) || ARTIFACT_TYPE_TOKENS.contains(&singular)
}

pub fn validate(name: &str) -> Result<(), TaxonomyError> {
    let segments: Vec<&str> = name.split('.').collect();
    if segments.len() < 2 {
        return Err(TaxonomyError::TooFewSegments {
            name: name.to_string(),
            segments: segments.len(),
        });
    }
    if segments.len() > 3 {
        return Err(TaxonomyError::TooManySegments {
            name: name.to_string(),
            segments: segments.len(),
        });
    }
    for seg in &segments {
        if seg.is_empty() {
            return Err(TaxonomyError::EmptySegment {
                name: name.to_string(),
            });
        }
        if !token_is_well_formed(seg) {
            return Err(TaxonomyError::BadToken {
                name: name.to_string(),
                token: (*seg).to_string(),
            });
        }
    }
    let subsystem = segments[0];
    if !SUBSYSTEM_TOKENS.contains(&subsystem) {
        return Err(TaxonomyError::UnknownSubsystem {
            name: name.to_string(),
            token: subsystem.to_string(),
        });
    }
    let verb = *segments.last().expect("≥2 segments");
    if looks_present_tense(verb) {
        return Err(TaxonomyError::PresentTenseVerb {
            name: name.to_string(),
            token: verb.to_string(),
        });
    }
    for seg in &segments {
        if looks_plural(seg) {
            return Err(TaxonomyError::PluralToken {
                name: name.to_string(),
                token: (*seg).to_string(),
            });
        }
    }
    Ok(())
}

pub fn is_seed_token(name: &str) -> bool {
    SEED_EVENT_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admit_fixture_every_seed_name_and_the_three_new_tokens_pass() {
        for name in SEED_EVENT_NAMES {
            assert!(
                validate(name).is_ok(),
                "seed name `{name}` was wrongly REJECTED: {:?}",
                validate(name)
            );
        }
        assert!(validate(new_tokens::CI_CHECK_UPDATED).is_ok());
        assert!(validate(new_tokens::CI_RESULT).is_ok());
        assert!(validate(new_tokens::ISSUE_INITIATIVE_CREATED).is_ok());
        assert_eq!(new_tokens::CI_RESULT.split('.').count(), 2);
    }

    #[test]
    fn reject_fixture_malformed_names_are_rejected_with_their_rule() {
        assert!(matches!(
            validate("git.PR.opened"),
            Err(TaxonomyError::BadToken { .. })
        ));
        assert!(matches!(
            validate("git.pr.open"),
            Err(TaxonomyError::PresentTenseVerb { .. })
        ));
        assert!(matches!(
            validate("git.comments.created"),
            Err(TaxonomyError::PluralToken { .. })
        ));
        assert!(matches!(
            validate("opened"),
            Err(TaxonomyError::TooFewSegments { .. })
        ));
        assert!(matches!(
            validate("billing.invoice.created"),
            Err(TaxonomyError::UnknownSubsystem { .. })
        ));
        assert!(matches!(
            validate("git.pull-request.opened"),
            Err(TaxonomyError::BadToken { .. })
        ));
        assert!(matches!(
            validate("git.1pr.opened"),
            Err(TaxonomyError::BadToken { .. })
        ));
        assert!(matches!(
            validate("git..opened"),
            Err(TaxonomyError::EmptySegment { .. })
        ));
        assert!(matches!(
            validate("git.pr.review.opened"),
            Err(TaxonomyError::TooManySegments { .. })
        ));
    }

    #[test]
    fn two_and_three_segment_forms_both_valid() {
        assert!(validate("ci.result").is_ok());
        assert!(validate("ci.check.updated").is_ok());
    }

    #[test]
    fn underscored_tokens_are_well_formed() {
        assert!(token_is_well_formed("break_glass"));
        assert!(token_is_well_formed("read_state"));
        assert!(token_is_well_formed("marked_ready"));
        assert!(validate("chat.read_state.updated").is_ok());
        assert!(validate("git.pr.marked_ready").is_ok());
        assert!(validate("identity.break_glass.invoked").is_ok());
    }

    #[test]
    fn identity_tuple_written_and_siblings_are_admitted_by_the_grammar() {
        for tok in myelin_identity::iam_events::IDENTITY_EVENT_TOKENS {
            assert!(validate(tok).is_ok(), "token `{tok}` must be admitted by this grammar");
        }
        assert!(matches!(
            validate("iam.tuple.written"),
            Err(TaxonomyError::UnknownSubsystem { .. })
        ));
    }

    #[test]
    fn the_three_new_tokens_are_registered_in_the_seed() {
        assert!(is_seed_token("ci.check.updated"));
        assert!(is_seed_token("ci.result"));
        assert!(is_seed_token("issue.initiative.created"));
        assert!(ARTIFACT_TYPE_TOKENS.contains(&"initiative"));
    }

    #[test]
    fn subsystem_token_set_is_the_frozen_canonical_set() {
        assert_eq!(
            SUBSYSTEM_TOKENS,
            &[
                "git",
                "ci",
                "issue",
                "knowledge",
                "chat",
                "identity",
                "refs"
            ]
        );
    }
}
