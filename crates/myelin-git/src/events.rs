//! # `events` — the complete `git.*` event token registration (GIT-P2 / P-124)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/git-hosting/architecture/03-events-contracts-and-glue.md`
//! §1 (**the complete `git.*` event taxonomy this subsystem owns** — the v1 token list),
//! `00-overview.md` §4 (inherited non-negotiables 1, 2).
//!
//! **Contract-index rows (registered here — against the frozen Bus grammar):**
//! - **2.9** Event taxonomy + token table — `<subsystem>.<artifact_type>.<event_name>`. Bus owns
//!   the **grammar + the seed**; **each subsystem completes its own list** (contract 2.9 text). This
//!   module is Git COMPLETING its `git.*` list — git **registers**, it does **not** author the
//!   grammar. Every token below is validated against the one Bus validator
//!   ([`myelin_events::validate_event_type`], EB-02 / P-042) — there is no second token language.
//! - **2.1** `EventEnvelope` — the canonical envelope every `git.*` event aligns to (the `type`
//!   field is one of the [`GIT_EVENT_TOKENS`] below; the names/units anchor). Referenced, not
//!   re-defined (the envelope lives in `myelin-events`).
//!
//! ## What this prompt (GIT-P2 / P-124) ships — and what it deliberately does NOT
//! **Ships:** the complete v1 `git.*` event-token registration (arch §1) as named `&'static str`
//! constants + the [`GIT_EVENT_TOKENS`] table, each PROVEN grammatical against the Bus §6.2/§6.1
//! grammar by [`myelin_events::validate_event_type`] (0 ungrammatical tokens — the gate). Git
//! REGISTERS its list; the Bus owns the grammar.
//!
//! **Does NOT ship (floors named — VISION §3 name-your-floors):** these tokens are **registered**
//! here but **actually EMITTED only from the outbox** in later prompts — there is no emit body here:
//! - **`git.ref.updated`** (the core push event) → **GIT-P8 / P-S-…**: the receive-pack → one-tx
//!   ref-CAS + outbox emit (the silent-data-loss floor, GIT-D9). Also the `git.repo.*` /
//!   `git.branch.*` / `git.tag.*` lifecycle emits ride the GitCore seam built there.
//! - **`git.pr.*` / `git.review.*` / `git.comment.*` / `git.thread.*`** → **GIT-P16**: the
//!   PR/review/comment hosting layer (the metadata-store emit seam).
//! - the `*.erased` tombstones (`git.repo.erased`, …) emit from the H1 holder erasure path
//!   (GIT-P29 / the §6.1 algorithm); the `*.snapshot` reindex-from-source events emit from
//!   `replay(scope, since)` (the reindex seam) — both via the outbox, never a direct publish.
//!
//! ## Why this is data (a `&'static str` token table), not an emit seam
//! Registration at M1 is a **names freeze** so dependents (CI's check-seam consumer, Refs' edge
//! builder, Search's indexer, Notif's router) compile against the NAMED git tokens, never literals
//! (the names anchor, X-5). The emit paths attach to these constants in the later prompts above;
//! when they do, they assert against THESE names — one token language, no drift (EI-01 §7).

use myelin_events::validate_event_type;
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RuntimeRef};

/// Stable tenant-bound actor pseudonym for Git event envelopes. Production emitters must never put
/// the authenticated principal's raw subject locator in the durable outbox.
pub fn event_actor_pseudonym(tenant: &str, subject: &str) -> String {
    event_actor_field_pseudonym("principal", tenant, subject)
}

fn event_actor_field_pseudonym(field: &str, tenant: &str, subject: &str) -> String {
    let digest = blake3::hash(
        format!("myelin.git.event-actor.v1\0{field}\0{tenant}\0{subject}").as_bytes(),
    );
    format!("git-event:{}", &digest.to_hex()[..32])
}

/// Return the event-safe projection of an authenticated principal. Every identifier nested in an
/// Agent principal is independently tenant-bound and domain-separated so neither the subject, the
/// runtime locator, nor the delegating human can leak into the durable event envelope or be joined
/// across fields by comparing equal pseudonyms.
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

// ===========================================================================
// §1 — the complete git.* event token constants (the registration; dotted grammar)
//
// Names are taken verbatim from arch 03 §1 (the owned taxonomy table). Every constant is asserted
// grammatical against the Bus validator in `tests` below (0 ungrammatical). The trailing comment on
// each row is the Aggregate (ordering key) from arch §1 — the per-aggregate ordering contract (2.3)
// the emit prompts wire.
// ===========================================================================

// --- repo lifecycle (aggregate: git/repo/<id>) -----------------------------

/// A repository was created. Aggregate `git/repo/<id>`.
pub const GIT_REPO_CREATED: &str = "git.repo.created";
/// A repository was deleted. Aggregate `git/repo/<id>`.
pub const GIT_REPO_DELETED: &str = "git.repo.deleted";
/// A repository was archived (read-only). Aggregate `git/repo/<id>`.
pub const GIT_REPO_ARCHIVED: &str = "git.repo.archived";
/// A repository was transferred to another owner. Aggregate `git/repo/<id>`.
pub const GIT_REPO_TRANSFERRED: &str = "git.repo.transferred";
/// A repository's visibility changed — drives Search/Refs ACL recompute. Aggregate `git/repo/<id>`.
pub const GIT_REPO_VISIBILITY_CHANGED: &str = "git.repo.visibility_changed";
/// A repository was forked (fork-network creation). Aggregate `git/repo/<child>`.
pub const GIT_REPO_FORKED: &str = "git.repo.forked";

// --- branch / ref / tag (aggregate: git/repo/<id>, except ref) -------------

/// A branch was created (derivable from `ref.updated`, emitted for convenience). `git/repo/<id>`.
pub const GIT_BRANCH_CREATED: &str = "git.branch.created";
/// A branch was deleted. Aggregate `git/repo/<id>`.
pub const GIT_BRANCH_DELETED: &str = "git.branch.deleted";
/// A branch ruleset changed (incl. the `required_contexts` policy). Aggregate `git/repo/<id>`.
pub const GIT_BRANCH_PROTECTION_CHANGED: &str = "git.branch.protection_changed";

/// **The core push event** — `{repo, ref, old_oid, new_oid, forced, commit_oids[], pusher_pseudonym}`.
/// **Per-ref aggregate** `git/ref/<repo>:<ref>` (contract 2.3); CI/Search/Refs/Agents consume.
/// Emit body: **GIT-P8** (receive-pack → one-tx ref-CAS + outbox, GIT-D9).
pub const GIT_REF_UPDATED: &str = "git.ref.updated";

/// A tag was created. Aggregate `git/repo/<id>`.
pub const GIT_TAG_CREATED: &str = "git.tag.created";
/// A tag was deleted. Aggregate `git/repo/<id>`.
pub const GIT_TAG_DELETED: &str = "git.tag.deleted";

// --- pull-request lifecycle (aggregate: git/pr/<n>) ------------------------

/// A pull request was opened. Aggregate `git/pr/<n>`.
pub const GIT_PR_OPENED: &str = "git.pr.opened";
/// A pull request was updated. Aggregate `git/pr/<n>`.
pub const GIT_PR_UPDATED: &str = "git.pr.updated";
/// A draft pull request was marked ready for review. Aggregate `git/pr/<n>`.
pub const GIT_PR_MARKED_READY: &str = "git.pr.marked_ready";
/// A pull request was closed. Aggregate `git/pr/<n>`.
pub const GIT_PR_CLOSED: &str = "git.pr.closed";
/// A pull request was reopened. Aggregate `git/pr/<n>`.
pub const GIT_PR_REOPENED: &str = "git.pr.reopened";
/// A pull request was merged. Aggregate `git/pr/<n>`.
pub const GIT_PR_MERGED: &str = "git.pr.merged";
/// A pull request head moved (re-anchor + re-gate). Aggregate `git/pr/<n>`.
pub const GIT_PR_SYNCHRONIZED: &str = "git.pr.synchronized";

// --- review (aggregate: git/pr/<n>) ----------------------------------------

/// A review was requested. Aggregate `git/pr/<n>`.
pub const GIT_REVIEW_REQUESTED: &str = "git.review.requested";
/// A review was submitted (carries verdict + `is_agent`). Aggregate `git/pr/<n>`.
pub const GIT_REVIEW_SUBMITTED: &str = "git.review.submitted";
/// A review was dismissed. Aggregate `git/pr/<n>`.
pub const GIT_REVIEW_DISMISSED: &str = "git.review.dismissed";

// --- comment / thread (aggregate: git/pr/<n>) ------------------------------

/// An inline/thread comment was created (`#comment-<id>`). Aggregate `git/pr/<n>`.
pub const GIT_COMMENT_CREATED: &str = "git.comment.created";
/// A comment was resolved. Aggregate `git/pr/<n>`.
pub const GIT_COMMENT_RESOLVED: &str = "git.comment.resolved";
/// A review thread was resolved (`#thread-<id>`). Aggregate `git/pr/<n>`.
pub const GIT_THREAD_RESOLVED: &str = "git.thread.resolved";

// --- merge-queue surfacing + the Git-owned merge gate (aggregate: git/pr/<n>) ---

/// A PR's merge was blocked (merge-queue surfacing). Aggregate `git/pr/<n>`.
pub const GIT_PR_MERGE_BLOCKED: &str = "git.pr.merge_blocked";
/// A PR was placed on the merge queue. Aggregate `git/pr/<n>`.
pub const GIT_PR_MERGE_QUEUED: &str = "git.pr.merge_queued";
/// The Git-owned merge-gate outcome (NOT a CI fact — Git emits this off its own projection).
/// Aggregate `git/pr/<n>`.
pub const GIT_CHECK_GATE_EVALUATED: &str = "git.check.gate_evaluated";
/// CODEOWNERS review is required on a PR. Aggregate `git/pr/<n>`.
pub const GIT_CODEOWNERS_REVIEW_REQUIRED: &str = "git.codeowners.review_required";

// --- audit-critical (aggregate: git/repo/<id> or git/pr/<n>) ---------------

/// **Audit-critical** — a branch-protection bypass was used (contract 10.6). Aggregate
/// `git/repo/<id>`.
pub const GIT_PROTECTION_BYPASS_USED: &str = "git.protection.bypass_used";
/// A maintainer endorsed an `untrusted_fork` CI run via `approve_untrusted_ci` (X-1,
/// audit-relevant). Aggregate `git/pr/<n>`.
pub const GIT_FORK_CI_ENDORSED: &str = "git.fork.ci_endorsed";

// --- governed MCP intent/outcome audit (aggregate: mcp run) ----------------

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

// --- cross-cutting *.erased tombstones (contract 2.7) ----------------------

/// The `*.erased` tombstone for a repo (contract 2.7); consumers drop derived state. Emitted from
/// the H1 holder erasure path (§6.1 algorithm).
pub const GIT_REPO_ERASED: &str = "git.repo.erased";
/// The `*.erased` tombstone for a PR (contract 2.7).
pub const GIT_PR_ERASED: &str = "git.pr.erased";
/// The `*.erased` tombstone for a comment (contract 2.7).
pub const GIT_COMMENT_ERASED: &str = "git.comment.erased";

/// **The blob-removal tombstone — the REAL Search removal operation for a code-projection doc.**
///
/// A `git.blob.snapshot` whose payload carries `op = "delete"` is NOT a Search tombstone: Search
/// drives removal off the event TYPE's trailing verb (`deleted`/`removed`/`erased`) or an owner
/// `project` resolving `Gone` — it never reads a payload `op` field. A delete emitted as a
/// `*.snapshot` therefore fell through to Search's UPSERT path and the stale doc survived. This
/// token is the removal verb the indexer actually honours, so a blob that leaves the indexed ref —
/// or whose subject is `restrict`ed (`03 §6`: the restricted body must not remain queryable, and a
/// body-suppressed upsert still leaves a path/oid-queryable doc) — is genuinely removed from the
/// index rather than downgraded to an empty document.
pub const GIT_BLOB_REMOVED: &str = "git.blob.removed";

// --- cross-cutting *.snapshot reindex-from-source events (contract 2.6) ----

/// The `*.snapshot` reindex-from-source event for a repo (contract 2.6 — `replay`). Cold == live.
pub const GIT_REPO_SNAPSHOT: &str = "git.repo.snapshot";
/// The `*.snapshot` reindex event for a PR (contract 2.6).
pub const GIT_PR_SNAPSHOT: &str = "git.pr.snapshot";
/// The `*.snapshot` reindex event for an indexed blob/code-projection (contract 2.6).
pub const GIT_BLOB_SNAPSHOT: &str = "git.blob.snapshot";
/// The `*.snapshot` reindex event for a comment (contract 2.6).
pub const GIT_COMMENT_SNAPSHOT: &str = "git.comment.snapshot";

/// The complete v1 `git.*` event-token list this subsystem registers (arch 03 §1). The Bus taxonomy
/// (contract 2.9) admits exactly these under the §6.1 grammar; each is PROVEN grammatical by
/// [`tests::every_git_token_parses_the_bus_grammar`]. **Git registers; it does not author the
/// grammar** — the validator is [`myelin_events::validate_event_type`] (one grammar, no drift).
///
/// Note: `key.added` / `token.created` are **echoed from Identity** (Id owns them) and git does NOT
/// originate them; and **git does NOT emit `ci.*`** (the check facts are CI's — the dependency is
/// acyclic: CI emits, Git reads). Neither appears in this list (arch §1).
pub const GIT_EVENT_TOKENS: &[&str] = &[
    // repo lifecycle
    GIT_REPO_CREATED,
    GIT_REPO_DELETED,
    GIT_REPO_ARCHIVED,
    GIT_REPO_TRANSFERRED,
    GIT_REPO_VISIBILITY_CHANGED,
    GIT_REPO_FORKED,
    // branch / ref / tag
    GIT_BRANCH_CREATED,
    GIT_BRANCH_DELETED,
    GIT_BRANCH_PROTECTION_CHANGED,
    GIT_REF_UPDATED,
    GIT_TAG_CREATED,
    GIT_TAG_DELETED,
    // pull-request lifecycle
    GIT_PR_OPENED,
    GIT_PR_UPDATED,
    GIT_PR_MARKED_READY,
    GIT_PR_CLOSED,
    GIT_PR_REOPENED,
    GIT_PR_MERGED,
    GIT_PR_SYNCHRONIZED,
    // review
    GIT_REVIEW_REQUESTED,
    GIT_REVIEW_SUBMITTED,
    GIT_REVIEW_DISMISSED,
    // comment / thread
    GIT_COMMENT_CREATED,
    GIT_COMMENT_RESOLVED,
    GIT_THREAD_RESOLVED,
    // merge-queue + merge gate
    GIT_PR_MERGE_BLOCKED,
    GIT_PR_MERGE_QUEUED,
    GIT_CHECK_GATE_EVALUATED,
    GIT_CODEOWNERS_REVIEW_REQUIRED,
    // audit-critical
    GIT_PROTECTION_BYPASS_USED,
    GIT_FORK_CI_ENDORSED,
    // governed MCP intent/outcome audit
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
    // cross-cutting *.erased tombstones (contract 2.7)
    GIT_REPO_ERASED,
    GIT_PR_ERASED,
    GIT_COMMENT_ERASED,
    // the code-projection removal tombstone (the verb Search's indexer honours)
    GIT_BLOB_REMOVED,
    // cross-cutting *.snapshot reindex events (contract 2.6)
    GIT_REPO_SNAPSHOT,
    GIT_PR_SNAPSHOT,
    GIT_BLOB_SNAPSHOT,
    GIT_COMMENT_SNAPSHOT,
];

/// Register the complete `git.*` list against the Bus grammar (contract 2.9). Returns `Ok(())` iff
/// **every** registered token parses the §6.1 grammar via the one Bus validator
/// ([`myelin_events::validate_event_type`]); otherwise the first offending token + its
/// [`myelin_events::TaxonomyError`] (LOUD, never silently coerced). This is the registration check
/// the GATE asserts (0 ungrammatical tokens) — git REGISTERS its list against the grammar it does
/// not own.
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

    /// **THE GATE (contract 2.9): 0 ungrammatical tokens.** Every registered `git.*` token parses
    /// the Bus §6.1/§6.2 grammar via the one Bus validator — git registers against the grammar it
    /// does not author. The parse is the green artifact.
    #[test]
    fn every_git_token_parses_the_bus_grammar() {
        for &tok in GIT_EVENT_TOKENS {
            assert!(
                validate_event_type(tok).is_ok(),
                "registered git token `{tok}` is UNGRAMMATICAL: {:?}",
                validate_event_type(tok)
            );
        }
        // The whole-list registration helper agrees (0 ungrammatical).
        assert!(
            register_git_tokens().is_ok(),
            "register_git_tokens() must succeed: {:?}",
            register_git_tokens()
        );
    }

    /// Every registered token carries the canonical `git` subsystem prefix (§6.2 — git is the
    /// canonical subsystem token; CLI aliases like `repo` are render-time only and never the
    /// stored/registered token).
    #[test]
    fn every_git_token_carries_the_git_subsystem_prefix() {
        for &tok in GIT_EVENT_TOKENS {
            let head = tok.split('.').next().expect("non-empty token");
            assert_eq!(
                head, "git",
                "token `{tok}` must carry the `git` subsystem prefix"
            );
            // ...and `git` is the canonical subsystem token the Bus knows.
            assert!(
                myelin_events::SUBSYSTEM_TOKENS.contains(&"git"),
                "`git` must be a canonical Bus subsystem token"
            );
        }
    }

    /// The list has **no duplicates** — a token registered twice is a contract smell (each name is
    /// minted once; the set is the authoritative registry).
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

    /// The core push event + the cross-subsystem-consumed tokens are present under their NAMED
    /// constants (the names anchor X-5 — CI/Search/Refs/Notif consume these by name, never by
    /// literal). A rename/drop here is a contract change every consumer must reconcile.
    #[test]
    fn the_load_bearing_git_tokens_are_registered() {
        // the core push event (per-ref aggregate, contract 2.3 — CI/Search/Refs/Agents consume)
        assert!(GIT_EVENT_TOKENS.contains(&GIT_REF_UPDATED));
        // the PR/review/comment family (GIT-P16 emit follow-on)
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
        // the audit-critical tokens (contract 10.6)
        assert!(GIT_EVENT_TOKENS.contains(&GIT_PROTECTION_BYPASS_USED));
        assert!(GIT_EVENT_TOKENS.contains(&GIT_FORK_CI_ENDORSED));
        for token in GIT_GOVERNANCE_AUDIT_EVENT_TOKENS {
            assert!(GIT_EVENT_TOKENS.contains(token));
        }
        // the cross-cutting *.erased + *.snapshot tokens (contracts 2.7 / 2.6)
        assert!(GIT_EVENT_TOKENS.contains(&GIT_REPO_ERASED));
        assert!(GIT_EVENT_TOKENS.contains(&GIT_BLOB_SNAPSHOT));
    }

    /// Git does NOT register `ci.*` (the dependency is acyclic: CI emits, Git reads) nor the
    /// Identity-owned `key.*` / `token.*` echoes (arch §1) — no registered token leaves the `git`
    /// prefix. This is the in-crate proof of the acyclic-producer invariant (EI-02 §3).
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
