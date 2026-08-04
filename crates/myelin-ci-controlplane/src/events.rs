use myelin_events::validate_event_type;

pub use myelin_ci_sandbox::events::{
    ci_event_tokens, is_durable, register_ci_tokens, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
};

pub const CI_SUBSYSTEM_TOKEN: &str = "ci";

pub const CI_TYPE_TOKENS: &[&str] = &[
    "run",
    "job",
    "check",
    "deployment",
    "runner",
    "pipeline",
    "log",
    "artifact",
    "cost",
    "supply_chain",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiTypeTokenError {
    NotCiSubsystem { token: String, head: String },
    UnregisteredTypeToken { token: String, type_seg: String },
}

impl std::fmt::Display for CiTypeTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CiTypeTokenError::NotCiSubsystem { token, head } => write!(
                f,
                "`{token}`: leading segment `{head}` is not the canonical `ci` subsystem token (§6.2)"
            ),
            CiTypeTokenError::UnregisteredTypeToken { token, type_seg } => write!(
                f,
                "`{token}`: artifact-type segment `{type_seg}` is not a registered CI type token \
                 (§6.2 - CI registers its type list, it does not author a new type at emit time)"
            ),
        }
    }
}

pub fn validate_ci_type_token(name: &str) -> Result<(), CiTypeTokenError> {
    let segments: Vec<&str> = name.split('.').collect();
    let head = segments.first().copied().unwrap_or("");
    if head != CI_SUBSYSTEM_TOKEN {
        return Err(CiTypeTokenError::NotCiSubsystem {
            token: name.to_string(),
            head: head.to_string(),
        });
    }
    if segments.len() == 3 {
        let type_seg = segments[1];
        if !CI_TYPE_TOKENS.contains(&type_seg) {
            return Err(CiTypeTokenError::UnregisteredTypeToken {
                token: name.to_string(),
                type_seg: type_seg.to_string(),
            });
        }
    }
    Ok(())
}

pub fn validate_ci_type_tokens() -> Result<(), (&'static str, CiTypeTokenError)> {
    for tok in ci_event_tokens() {
        validate_ci_type_token(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

pub fn register_ci_taxonomy() -> Result<(), (&'static str, String)> {
    for tok in ci_event_tokens() {
        if let Err(e) = validate_event_type(tok) {
            return Err((tok, format!("§6.1 grammar: {e}")));
        }
        if let Err(e) = validate_ci_type_token(tok) {
            return Err((tok, format!("§6.2 token table: {e}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::SUBSYSTEM_TOKENS;
    use std::collections::BTreeSet;

    #[test]
    fn register_ci_taxonomy_passes_grammar_and_token_table() {
        assert_eq!(
            register_ci_taxonomy(),
            Ok(()),
            "the CI control-plane taxonomy registration must be GREEN: {:?}",
            register_ci_taxonomy()
        );
        for tok in ci_event_tokens() {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered ci token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert_eq!(
            validate_ci_type_tokens(),
            Ok(()),
            "every ci token must conform to the §6.2 token table: {:?}",
            validate_ci_type_tokens()
        );
    }

    #[test]
    fn ci_is_the_canonical_subsystem_token_and_every_token_carries_it() {
        assert!(
            SUBSYSTEM_TOKENS.contains(&CI_SUBSYSTEM_TOKEN),
            "`ci` must be a canonical Bus subsystem token (§6.2)"
        );
        for tok in ci_event_tokens() {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "ci",
                "token `{tok}` must carry the `ci` subsystem prefix"
            );
        }
    }

    #[test]
    fn every_artifact_type_segment_is_a_registered_singular_type_token() {
        for named in ["run", "deployment", "pipeline", "runner", "artifact"] {
            assert!(
                CI_TYPE_TOKENS.contains(&named),
                "the CI-P7-named type token `{named}` must be registered"
            );
        }
        for tok in ci_event_tokens() {
            let segs: Vec<&str> = tok.split('.').collect();
            if segs.len() == 3 {
                assert!(
                    CI_TYPE_TOKENS.contains(&segs[1]),
                    "token `{tok}`: type segment `{}` is not a registered CI type token",
                    segs[1]
                );
            }
        }
        for ty in CI_TYPE_TOKENS {
            let probe = format!("ci.{ty}.created");
            assert!(
                validate_event_type(&probe).is_ok(),
                "CI type token `{ty}` is not a well-formed singular §6.2 token (probe `{probe}`): {:?}",
                validate_event_type(&probe)
            );
        }
    }

    #[test]
    fn the_token_table_check_rejects_foreign_and_unregistered_loudly() {
        assert!(matches!(
            validate_ci_type_token("git.pr.opened"),
            Err(CiTypeTokenError::NotCiSubsystem { .. })
        ));
        assert!(matches!(
            validate_ci_type_token("ci.widget.created"),
            Err(CiTypeTokenError::UnregisteredTypeToken { type_seg, .. }) if type_seg == "widget"
        ));
        assert_eq!(validate_ci_type_token("ci.result"), Ok(()));
    }

    #[test]
    fn the_registration_reuses_the_one_canonical_source() {
        assert_eq!(
            CI_DURABLE_TOKENS,
            myelin_ci_sandbox::events::CI_DURABLE_TOKENS
        );
        assert_eq!(
            CI_FIREHOSE_TOKENS,
            myelin_ci_sandbox::events::CI_FIREHOSE_TOKENS
        );
        assert!(
            register_ci_tokens().is_ok(),
            "the §6.1 source registration is green"
        );
        assert!(
            register_ci_taxonomy().is_ok(),
            "the §6.1+§6.2 control-plane registration is green"
        );
    }

    #[test]
    fn the_registered_list_has_no_duplicates() {
        let mut seen = BTreeSet::new();
        for tok in ci_event_tokens() {
            assert!(
                seen.insert(tok),
                "ci token `{tok}` is registered more than once"
            );
        }
    }

    #[test]
    fn the_superseded_legacy_tokens_are_absent_from_the_registration() {
        for tok in ci_event_tokens() {
            assert_ne!(
                tok, "ci.status.updated",
                "superseded by ci.check.updated (Δ1)"
            );
            assert_ne!(tok, "ci.run.passed", "superseded by ci.check.updated (Δ1)");
        }
        assert!(validate_event_type("ci.status.updated").is_ok());
        assert!(validate_event_type("ci.run.passed").is_ok());
    }
}
