use myelin_events::{
    validate_event_type, RegisteredToken, SubsystemTokenList,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};

pub fn event_actor_pseudonym(tenant: &str, subject: &str) -> String {
    event_actor_field_pseudonym("principal", tenant, subject)
}

fn event_actor_field_pseudonym(field: &str, tenant: &str, subject: &str) -> String {
    let digest = blake3::hash(
        format!("myelin.git.event-actor.v1\0{field}\0{tenant}\0{subject}").as_bytes(),
    );
    format!("git-event:{}", &digest.to_hex()[..32])
}

pub fn pseudonymized_event_principal(tenant: &str, principal: &Principal) -> Principal {
    let mut projected = principal.clone();
    projected.principal_id = PrincipalId(event_actor_pseudonym(
        tenant,
        &principal.principal_id.0,
    ));
    if let PrincipalKind::Agent {
        runtime_ref,
        on_behalf_of,
    } = &principal.kind
    {
        projected.kind = PrincipalKind::Agent {
            runtime_ref: RuntimeRef(event_actor_field_pseudonym(
                "runtime-ref",
                tenant,
                &runtime_ref.0,
            )),
            on_behalf_of: on_behalf_of.as_ref().map(|delegator| {
                PrincipalId(event_actor_field_pseudonym(
                    "on-behalf-of",
                    tenant,
                    &delegator.0,
                ))
            }),
        };
    }
    projected
}

pub const GIT_REPO_CREATED: &str = "git.repo.created";
pub const GIT_REPO_DELETED: &str = "git.repo.deleted";
pub const GIT_REPO_ARCHIVED: &str = "git.repo.archived";
pub const GIT_REPO_TRANSFERRED: &str = "git.repo.transferred";
pub const GIT_REPO_VISIBILITY_CHANGED: &str = "git.repo.visibility_changed";
pub const GIT_REPO_FORKED: &str = "git.repo.forked";

pub const GIT_BRANCH_CREATED: &str = "git.branch.created";
pub const GIT_BRANCH_DELETED: &str = "git.branch.deleted";
pub const GIT_BRANCH_PROTECTION_CHANGED: &str = "git.branch.protection_changed";

pub const GIT_REF_UPDATED: &str = "git.ref.updated";

pub const GIT_TAG_CREATED: &str = "git.tag.created";
pub const GIT_TAG_DELETED: &str = "git.tag.deleted";

pub const GIT_PR_OPENED: &str = "git.pr.opened";
pub const GIT_PR_UPDATED: &str = "git.pr.updated";
pub const GIT_PR_MARKED_READY: &str = "git.pr.marked_ready";
pub const GIT_PR_CLOSED: &str = "git.pr.closed";
pub const GIT_PR_REOPENED: &str = "git.pr.reopened";
pub const GIT_PR_MERGED: &str = "git.pr.merged";
pub const GIT_PR_SYNCHRONIZED: &str = "git.pr.synchronized";

pub const GIT_PR_HEAD_TRIGGER_SCHEMA_V2: u32 = 2;

pub const GIT_REVIEW_REQUESTED: &str = "git.review.requested";
pub const GIT_REVIEW_SUBMITTED: &str = "git.review.submitted";
pub const GIT_REVIEW_DISMISSED: &str = "git.review.dismissed";

pub const GIT_COMMENT_CREATED: &str = "git.comment.created";
pub const GIT_COMMENT_RESOLVED: &str = "git.comment.resolved";
pub const GIT_THREAD_RESOLVED: &str = "git.thread.resolved";

pub const GIT_PR_MERGE_BLOCKED: &str = "git.pr.merge_blocked";
pub const GIT_PR_MERGE_QUEUED: &str = "git.pr.merge_queued";
pub const GIT_CHECK_GATE_EVALUATED: &str = "git.check.gate_evaluated";
pub const GIT_CODEOWNERS_REVIEW_REQUIRED: &str = "git.codeowners.review_required";

pub const GIT_PROTECTION_BYPASS_USED: &str = "git.protection.bypass_used";
pub const GIT_FORK_CI_ENDORSED: &str = "git.fork.ci_endorsed";

pub const GIT_MERGE_ATTEMPTED: &str = "git.merge.attempted";
pub const GIT_MERGE_APPLIED: &str = "git.merge.applied";
pub const GIT_MERGE_GATED: &str = "git.merge.gated";
pub const GIT_MERGE_DENIED: &str = "git.merge.denied";
pub const GIT_MERGE_INDETERMINATE: &str = "git.merge.indeterminate";
pub const GIT_MERGE_APPROVED: &str = "git.merge.approved";
pub const GIT_MERGE_REJECTED: &str = "git.merge.rejected";
pub const GIT_MERGE_EXPIRED: &str = "git.merge.expired";
pub const GIT_OPEN_PR_ATTEMPTED: &str = "git.open_pr.attempted";
pub const GIT_OPEN_PR_APPLIED: &str = "git.open_pr.applied";
pub const GIT_OPEN_PR_GATED: &str = "git.open_pr.gated";
pub const GIT_OPEN_PR_DENIED: &str = "git.open_pr.denied";
pub const GIT_OPEN_PR_INDETERMINATE: &str = "git.open_pr.indeterminate";
pub const GIT_WRITE_FILE_ATTEMPTED: &str = "git.write_file.attempted";
pub const GIT_WRITE_FILE_APPLIED: &str = "git.write_file.applied";
pub const GIT_WRITE_FILE_GATED: &str = "git.write_file.gated";
pub const GIT_WRITE_FILE_DENIED: &str = "git.write_file.denied";
pub const GIT_WRITE_FILE_INDETERMINATE: &str = "git.write_file.indeterminate";
pub const GIT_SUBMIT_REVIEW_ATTEMPTED: &str = "git.submit_review.attempted";
pub const GIT_SUBMIT_REVIEW_APPLIED: &str = "git.submit_review.applied";
pub const GIT_SUBMIT_REVIEW_GATED: &str = "git.submit_review.gated";
pub const GIT_SUBMIT_REVIEW_DENIED: &str = "git.submit_review.denied";
pub const GIT_SUBMIT_REVIEW_INDETERMINATE: &str = "git.submit_review.indeterminate";
pub const GIT_ENDORSE_FORK_CI_ATTEMPTED: &str = "git.endorse_fork_ci.attempted";
pub const GIT_ENDORSE_FORK_CI_APPLIED: &str = "git.endorse_fork_ci.applied";
pub const GIT_ENDORSE_FORK_CI_GATED: &str = "git.endorse_fork_ci.gated";
pub const GIT_ENDORSE_FORK_CI_DENIED: &str = "git.endorse_fork_ci.denied";
pub const GIT_ENDORSE_FORK_CI_INDETERMINATE: &str = "git.endorse_fork_ci.indeterminate";

pub const GIT_GOVERNANCE_AUDIT_EVENT_TOKENS: &[&str] = &[
    GIT_MERGE_ATTEMPTED,
    GIT_MERGE_APPLIED,
    GIT_MERGE_GATED,
    GIT_MERGE_DENIED,
    GIT_MERGE_INDETERMINATE,
    GIT_MERGE_APPROVED,
    GIT_MERGE_REJECTED,
    GIT_MERGE_EXPIRED,
    GIT_OPEN_PR_ATTEMPTED,
    GIT_OPEN_PR_APPLIED,
    GIT_OPEN_PR_GATED,
    GIT_OPEN_PR_DENIED,
    GIT_OPEN_PR_INDETERMINATE,
    GIT_WRITE_FILE_ATTEMPTED,
    GIT_WRITE_FILE_APPLIED,
    GIT_WRITE_FILE_GATED,
    GIT_WRITE_FILE_DENIED,
    GIT_WRITE_FILE_INDETERMINATE,
    GIT_SUBMIT_REVIEW_ATTEMPTED,
    GIT_SUBMIT_REVIEW_APPLIED,
    GIT_SUBMIT_REVIEW_GATED,
    GIT_SUBMIT_REVIEW_DENIED,
    GIT_SUBMIT_REVIEW_INDETERMINATE,
    GIT_ENDORSE_FORK_CI_ATTEMPTED,
    GIT_ENDORSE_FORK_CI_APPLIED,
    GIT_ENDORSE_FORK_CI_GATED,
    GIT_ENDORSE_FORK_CI_DENIED,
    GIT_ENDORSE_FORK_CI_INDETERMINATE,
];

pub const GIT_REPO_ERASED: &str = "git.repo.erased";
pub const GIT_PR_ERASED: &str = "git.pr.erased";
pub const GIT_COMMENT_ERASED: &str = "git.comment.erased";

pub const GIT_REPO_SNAPSHOT: &str = "git.repo.snapshot";
pub const GIT_PR_SNAPSHOT: &str = "git.pr.snapshot";
pub const GIT_BLOB_SNAPSHOT: &str = "git.blob.snapshot";
pub const GIT_COMMENT_SNAPSHOT: &str = "git.comment.snapshot";

pub const GIT_EVENT_TOKENS: &[&str] = &[
    GIT_REPO_CREATED,
    GIT_REPO_DELETED,
    GIT_REPO_ARCHIVED,
    GIT_REPO_TRANSFERRED,
    GIT_REPO_VISIBILITY_CHANGED,
    GIT_REPO_FORKED,
    GIT_BRANCH_CREATED,
    GIT_BRANCH_DELETED,
    GIT_BRANCH_PROTECTION_CHANGED,
    GIT_REF_UPDATED,
    GIT_TAG_CREATED,
    GIT_TAG_DELETED,
    GIT_PR_OPENED,
    GIT_PR_UPDATED,
    GIT_PR_MARKED_READY,
    GIT_PR_CLOSED,
    GIT_PR_REOPENED,
    GIT_PR_MERGED,
    GIT_PR_SYNCHRONIZED,
    GIT_REVIEW_REQUESTED,
    GIT_REVIEW_SUBMITTED,
    GIT_REVIEW_DISMISSED,
    GIT_COMMENT_CREATED,
    GIT_COMMENT_RESOLVED,
    GIT_THREAD_RESOLVED,
    GIT_PR_MERGE_BLOCKED,
    GIT_PR_MERGE_QUEUED,
    GIT_CHECK_GATE_EVALUATED,
    GIT_CODEOWNERS_REVIEW_REQUIRED,
    GIT_PROTECTION_BYPASS_USED,
    GIT_FORK_CI_ENDORSED,
    GIT_MERGE_ATTEMPTED,
    GIT_MERGE_APPLIED,
    GIT_MERGE_GATED,
    GIT_MERGE_DENIED,
    GIT_MERGE_INDETERMINATE,
    GIT_MERGE_APPROVED,
    GIT_MERGE_REJECTED,
    GIT_MERGE_EXPIRED,
    GIT_OPEN_PR_ATTEMPTED,
    GIT_OPEN_PR_APPLIED,
    GIT_OPEN_PR_GATED,
    GIT_OPEN_PR_DENIED,
    GIT_OPEN_PR_INDETERMINATE,
    GIT_WRITE_FILE_ATTEMPTED,
    GIT_WRITE_FILE_APPLIED,
    GIT_WRITE_FILE_GATED,
    GIT_WRITE_FILE_DENIED,
    GIT_WRITE_FILE_INDETERMINATE,
    GIT_SUBMIT_REVIEW_ATTEMPTED,
    GIT_SUBMIT_REVIEW_APPLIED,
    GIT_SUBMIT_REVIEW_GATED,
    GIT_SUBMIT_REVIEW_DENIED,
    GIT_SUBMIT_REVIEW_INDETERMINATE,
    GIT_ENDORSE_FORK_CI_ATTEMPTED,
    GIT_ENDORSE_FORK_CI_APPLIED,
    GIT_ENDORSE_FORK_CI_GATED,
    GIT_ENDORSE_FORK_CI_DENIED,
    GIT_ENDORSE_FORK_CI_INDETERMINATE,
    GIT_REPO_ERASED,
    GIT_PR_ERASED,
    GIT_COMMENT_ERASED,
    GIT_REPO_SNAPSHOT,
    GIT_PR_SNAPSHOT,
    GIT_BLOB_SNAPSHOT,
    GIT_COMMENT_SNAPSHOT,
];

pub fn git_event_token_list() -> SubsystemTokenList {
    SubsystemTokenList::new(
        "git",
        GIT_EVENT_TOKENS
            .iter()
            .map(|name| {
                let token = RegisteredToken::references_only(*name);
                if matches!(*name, GIT_PR_OPENED | GIT_PR_SYNCHRONIZED) {
                    token.at_schema_ver(GIT_PR_HEAD_TRIGGER_SCHEMA_V2)
                } else {
                    token
                }
            })
            .collect(),
    )
}

pub fn register_git_tokens() -> Result<(), (&'static str, myelin_events::TaxonomyError)> {
    for &tok in GIT_EVENT_TOKENS {
        validate_event_type(tok).map_err(|e| (tok, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_projection_scrubs_every_nested_identifier_with_separate_domains() {
        let principal = Principal::stub(
            PrincipalId("agent:raw-subject".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("runtime://raw-host/session".into()),
                on_behalf_of: Some(PrincipalId("human:raw-delegator".into())),
            },
            myelin_tenancy::TenantId("acme".into()),
        );
        let projected = pseudonymized_event_principal("acme", &principal);
        let serialized = serde_json::to_string(&myelin_events::Actor(projected.clone())).unwrap();
        for raw in [
            "agent:raw-subject",
            "runtime://raw-host/session",
            "human:raw-delegator",
        ] {
            assert!(!serialized.contains(raw), "raw nested identifier leaked: {raw}");
        }
        let PrincipalKind::Agent {
            runtime_ref,
            on_behalf_of,
        } = projected.kind
        else {
            panic!("Agent discriminant must be preserved")
        };
        let delegator = on_behalf_of.expect("delegator projection");
        assert_ne!(projected.principal_id.0, runtime_ref.0);
        assert_ne!(projected.principal_id.0, delegator.0);
        assert_ne!(runtime_ref.0, delegator.0, "field domains must not correlate");
    }

    #[test]
    fn every_git_token_parses_the_bus_grammar() {
        for &tok in GIT_EVENT_TOKENS {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered git token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        assert!(
            register_git_tokens().is_ok(),
            "register_git_tokens() must succeed: {:?}",
            register_git_tokens()
        );
    }

    #[test]
    fn every_git_token_carries_the_git_subsystem_prefix() {
        for &tok in GIT_EVENT_TOKENS {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "git",
                "token `{tok}` must carry the `git` subsystem prefix"
            );
            assert!(
                myelin_events::SUBSYSTEM_TOKENS.contains(&"git"),
                "`git` must be a canonical Bus subsystem token"
            );
        }
    }

    #[test]
    fn the_git_token_list_has_no_duplicates() {
        let mut seen = std::collections::BTreeSet::new();
        for &tok in GIT_EVENT_TOKENS {
            assert!(
                seen.insert(tok),
                "git token `{tok}` is registered more than once"
            );
        }
        assert_eq!(seen.len(), GIT_EVENT_TOKENS.len());
    }

    #[test]
    fn the_load_bearing_git_tokens_are_registered() {
        assert!(GIT_EVENT_TOKENS.contains(&GIT_REF_UPDATED));
        for tok in [
            GIT_PR_OPENED,
            GIT_PR_MERGED,
            GIT_REVIEW_SUBMITTED,
            GIT_COMMENT_CREATED,
        ] {
            assert!(
                GIT_EVENT_TOKENS.contains(&tok),
                "`{tok}` must be registered"
            );
        }
        assert!(GIT_EVENT_TOKENS.contains(&GIT_PROTECTION_BYPASS_USED));
        assert!(GIT_EVENT_TOKENS.contains(&GIT_FORK_CI_ENDORSED));
        for token in GIT_GOVERNANCE_AUDIT_EVENT_TOKENS {
            assert!(GIT_EVENT_TOKENS.contains(token));
        }
        assert!(GIT_EVENT_TOKENS.contains(&GIT_REPO_ERASED));
        assert!(GIT_EVENT_TOKENS.contains(&GIT_BLOB_SNAPSHOT));
    }

    #[test]
    fn git_registers_no_foreign_subsystem_tokens() {
        for &tok in GIT_EVENT_TOKENS {
            assert!(
                !tok.starts_with("ci.")
                    && !tok.starts_with("identity.")
                    && !tok.starts_with("key.")
                    && !tok.starts_with("token."),
                "git must not register the foreign-subsystem token `{tok}`"
            );
        }
    }
}
