//! # `taxonomy` — the event-type grammar validator + the seed token table (EB-02 / P-042)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/event-bus.md`
//! §6.1 (the dotted-name grammar — the AUTHORITY), §6.2 (the `ArtifactRef` subsystem/type token
//! table + the new `initiative` type token), §6.3 (the new `ci.check.updated` / `ci.result`
//! check-seam tokens), §6.4 (the representative seed event names).
//! **Contract:** `contract-index.md` row 2.9 (Event taxonomy + token table — Bus owns the
//! grammar + the seed; **+ new tokens** `ci.check.updated`, `ci.result`, type token `initiative`).
//! **Reconciliation:** `00-reconciliation-decisions.md` §2 (the new tokens registered; the
//! `initiative` type is a sanctioned §6.2 extension, no new subsystem token).
//!
//! ## What this is (the grammar + seed; NOT the per-subsystem completion)
//! This module is the **one grammar** the whole platform validates a `type` field against, plus
//! the **seed** token table (the §6.4 representative names + the three new check-seam/`initiative`
//! tokens). It is the names anchor (X-5): one grammar, no per-subsystem drift (EI-01 §7).
//!
//! The grammar (§6.1), exactly:
//!
//! - `type = <subsystem>.<artifact_type>.<event_name>` — lowercase, dot-separated tokens;
//! - each token matches `[a-z][a-z0-9_]*` (lowercase ASCII start, then lowercase / digit /
//!   underscore — `break_glass`, `read_state` are well-formed; `Foo`, `1bad`, `a-b` are not);
//! - **two segments minimum, three when an artifact type clarifies** (`ci.result` is a valid
//!   two-segment name; `ci.check.updated` is the three-segment form);
//! - the leading token is a **known subsystem** (`git`/`ci`/`issue`/`knowledge`/`chat`/
//!   `identity`/`refs`, §6.2 — the canonical singular set; CLI aliases are render-time only);
//! - the verb tokens are **singular, past-tense** — the validator rejects the obvious
//!   present-tense / plural mistakes (`open` vs `opened`, `comments` vs `comment`) by a
//!   conservative deny-list (see [`looks_present_tense`] / [`looks_plural`]); the full
//!   morphology is not re-derived — the deny-list catches the seed-relevant mistakes and the
//!   admit-fixture proves every real seed name passes.
//!
//! ## Floor named (EI-01 §1) — the per-subsystem token LIST is EB-24, not here
//! EB-02 ships the **grammar + the §6.4 seed + the three new check-seam/`initiative` tokens**.
//! Each subsystem owns and COMPLETES its full dotted-name list as a 5-B / M3-M4 deliverable
//! (**EB-24**), validated against THIS grammar. So [`SEED_EVENT_NAMES`] is the representative
//! seed, not an exhaustive registry; [`validate`] is the gate every later list is checked by.
//! The token set Identity ships (`identity.tuple.written`, `identity.role.granted`,
//! `identity.break_glass.invoked`, `myelin-identity::iam_events`) uses the already-canonical
//! `identity.*` §6.2 subsystem prefix — the SAME prefix the §6.4 Bus seed
//! (`identity.permission.granted`, …) uses. **Corrected cross-crate contract note:** these
//! tokens were originally minted with an `iam.` prefix, which this grammar has never admitted
//! (see [`tests::subsystem_token_set_is_the_frozen_canonical_set`] — the exclusion of `iam` is
//! deliberate, not an oversight); every real `iam.*` outbox row hit
//! [`TaxonomyError::UnknownSubsystem`] at the elected relay and was permanently quarantined.
//! `identity` was already the canonical token for this crate's subsystem, so the fix was a
//! rename to `identity.*`, not a second subsystem token — see
//! [`tests::identity_tuple_written_and_siblings_are_admitted_by_the_grammar`].

/// The canonical singular **subsystem** tokens (Bus §6.2 — the names anchor, UNCHANGED). The
/// leading segment of every well-formed event `type` is one of these. CLI aliases (`repo`/`doc`
/// /…) are a render-time projection only and are intentionally NOT here — Refs is the validator,
/// not a second authority.
pub const SUBSYSTEM_TOKENS: &[&str] = &[
    "git",
    "ci",
    "issue",
    "knowledge",
    "chat",
    "identity",
    "refs",
];

/// The canonical singular **artifact-type** tokens that appear in the seed (Bus §6.2 + §6.4),
/// **including the new `initiative`** type token (the sanctioned §6.2 extension, recon §2 — a
/// ranked `issue`-family type `myelin://<tenant>/issue/initiative/<id>`; no new subsystem token).
/// This is the seed set the grammar admits; subsystems extend it under EB-24.
pub const ARTIFACT_TYPE_TOKENS: &[&str] = &[
    // git (EB-24: git's full canonical-root type list — `repo`/`commit`/`blob` complete the
    // architecture §2 root table the #sub mints attach to: pr/comment+thread, blob/L<a>-L<b>,
    // commit/review canonical roots; GIT-P4 / P-230)
    "pr",
    "ref",
    "review",
    "comment",
    "repo",
    "commit",
    "blob", // ci
    "run",
    "check",
    "log",
    "artifact",
    // CI's remaining canonical-root type tokens (CI-P25 / P-368): the cross-fabric `ArtifactRef`
    // mints `project(ref, viewer)` resolves are `myelin://<t>/ci/{run|deployment|pipeline|runner|
    // artifact}/<id>` (continuous-integration §7.1). `run`/`artifact` were already present (the
    // `#step-<n>`/`L<a>-L<b>` mints attach to `run`; `check`/`log` are the X-1 + log roots); the
    // deployment/pipeline/runner canonical roots are added here so CI's run-unfurl / PR-context-pane
    // / deploy-card refs parse to a grammatical URN root — the SAME EB-24 root-type extension git
    // (`repo`/`commit`/`blob`) and chat (`channel`/`thread`) carry for the same reason.
    "deployment",
    "pipeline",
    "runner", // issue
    "issue",
    "initiative", // NEW type token (recon §2 / §6.2)
    "relation",   // knowledge
    "page",
    "doc",
    "row", // chat
    // chat's three canonical-root type tokens (architecture chat §2:
    // `myelin://<tenant>/chat/{channel|message|thread}/<id>`). `message` was already present (it
    // is also an identity type); `channel`/`thread` are added here so chat's `#sub` mints
    // (`message-`/`thread-`) attach to a parsing URN root (CHAT-P2 / P-244, mirroring git's
    // EB-24 `repo`/`commit`/`blob` extension for the same reason).
    "channel",
    "message",
    "thread",
    "read_state", // identity
    "permission",
    "member", // refs
    "edge",
];

/// The two **new** check-seam tokens registered by EB-02 (Bus §6.3, X-1). Pinned as constants so
/// the Git merge-gate consumer (the `CheckStatus` projection) and the merge-queue durable
/// workflow assert against NAMED tokens, never literals.
pub mod new_tokens {
    /// A `(commit_oid, context)` `CheckStatus` fact; `aggregate = (repo, commit_oid)` (§4.12).
    /// The producer of the Git merge-gate projection (three-segment form).
    pub const CI_CHECK_UPDATED: &str = "ci.check.updated";
    /// The CI-derived rollup the merge-queue durable workflow waits on as a signal (§4.12,
    /// contract 9.4) — a valid **two-segment** name (`<subsystem>.<event_name>`).
    pub const CI_RESULT: &str = "ci.result";
    /// The new `initiative` artifact-type token, demonstrated in an `issue`-family event name
    /// (`issue.initiative.created`) — the third "new token" EB-02 must ADMIT.
    pub const ISSUE_INITIATIVE_CREATED: &str = "issue.initiative.created";
}

/// The §6.4 representative **seed** event-name table (UNCHANGED + the new tokens). Each name is
/// admitted by [`validate`]; subsystems complete their full lists under EB-24. This is the
/// admit-fixture's positive corpus — every entry here, plus the three [`new_tokens`], must pass.
pub const SEED_EVENT_NAMES: &[&str] = &[
    // git (§6.4)
    "git.pr.opened",
    "git.pr.updated",
    "git.pr.closed",
    "git.pr.merged",
    "git.pr.reopened",
    "git.pr.marked_ready",
    "git.ref.updated",
    "git.review.submitted",
    "git.comment.created",
    // ci (§6.4) — incl. the two NEW check-seam tokens (§6.3)
    "ci.run.started",
    "ci.run.passed",
    "ci.run.failed",
    "ci.run.cancelled",
    new_tokens::CI_CHECK_UPDATED, // NEW (§6.3)
    new_tokens::CI_RESULT,        // NEW (§6.3)
    "ci.log.available",
    "ci.artifact.published",
    // issue (§6.4) — incl. the NEW `initiative` type token
    "issue.issue.created",
    "issue.issue.updated",
    "issue.issue.transitioned",
    "issue.issue.closed",
    new_tokens::ISSUE_INITIATIVE_CREATED, // NEW type token (§6.2/recon §2)
    "issue.initiative.updated",
    "issue.relation.created",
    // knowledge (§6.4)
    "knowledge.page.created",
    "knowledge.page.updated",
    "knowledge.doc.updated",
    "knowledge.row.updated",
    // chat (§6.4)
    "chat.message.created",
    "chat.read_state.updated",
    // identity (§6.4 — the canonical `identity.*` subsystem prefix; Identity's own
    // `iam_events` token set — `identity.tuple.written`, `identity.role.granted`,
    // `identity.break_glass.invoked` — uses this SAME canonical prefix, corrected from an
    // earlier `iam.*` naming that this grammar never admitted, see the module doc)
    "identity.permission.granted",
    "identity.permission.revoked",
    "identity.member.added",
    // refs (§6.4)
    "refs.edge.created",
    "refs.edge.removed",
    // cross-cutting (§6.4): the `*.erased` tombstone + `*.snapshot` reindex events, here
    // instantiated for a representative subsystem (the `*` is per-subsystem at emit time).
    "git.repo.erased",
    "knowledge.page.snapshot",
];

/// Why a `type` name is malformed (Bus §6.1). Each variant is a distinct, LOUD reason — the
/// validator never silently coerces; a bad name is rejected with the rule it broke (EI-01 §5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaxonomyError {
    /// Fewer than two dotted segments (`opened`, `git`) — the minimum is `<subsystem>.<event>`.
    TooFewSegments { name: String, segments: usize },
    /// More than three dotted segments — the grammar is at most `<sub>.<type>.<event>`.
    TooManySegments { name: String, segments: usize },
    /// An empty segment (a leading/trailing/doubled dot: `git..opened`, `.ci.run`).
    EmptySegment { name: String },
    /// A token that does not match `[a-z][a-z0-9_]*` (uppercase, leading digit, a hyphen, …).
    BadToken { name: String, token: String },
    /// The leading token is not one of the canonical [`SUBSYSTEM_TOKENS`].
    UnknownSubsystem { name: String, token: String },
    /// A verb token that looks present-tense where past-tense is required (`open` vs `opened`).
    PresentTenseVerb { name: String, token: String },
    /// A token that looks plural where singular is required (`comments` vs `comment`).
    PluralToken { name: String, token: String },
}

impl std::fmt::Display for TaxonomyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaxonomyError::TooFewSegments { name, segments } => write!(
                f,
                "`{name}`: {segments} segment(s) — the grammar needs ≥2 (<subsystem>.<event_name>)"
            ),
            TaxonomyError::TooManySegments { name, segments } => write!(
                f,
                "`{name}`: {segments} segments — the grammar is at most 3 (<sub>.<type>.<event>)"
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
                "`{name}`: verb `{token}` looks present-tense — event verbs are past-tense \
                 (e.g. `opened`, not `open`)"
            ),
            TaxonomyError::PluralToken { name, token } => write!(
                f,
                "`{name}`: token `{token}` looks plural — taxonomy tokens are singular \
                 (e.g. `comment`, not `comments`)"
            ),
        }
    }
}

/// One dotted token is well-formed iff it matches `[a-z][a-z0-9_]*` (§6.1): a lowercase ASCII
/// letter, then lowercase ASCII letters / ASCII digits / underscores. Empty is NOT well-formed.
fn token_is_well_formed(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// A conservative present-tense deny-list (§6.1: verbs are past-tense). We do NOT re-derive
/// English morphology; we reject the bare/present forms of the seed lifecycle verbs so the
/// red-fixture (`git.pr.open`, `ci.run.start`) is caught while every real past-tense seed name
/// passes. The full per-subsystem list is each subsystem's EB-24 responsibility.
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
    // `snapshot`/`publish` are the noun-form seed names; only treat a token as present-tense if
    // it is EXACTLY a bare verb AND not one of the legitimate noun/past seed tokens. The seed
    // admit-fixture is the cross-check that no real name is rejected.
    const LEGIT_NON_VERB: &[&str] = &["snapshot", "published", "available"];
    if LEGIT_NON_VERB.contains(&token) {
        return false;
    }
    PRESENT_VERBS.contains(&token)
}

/// A conservative plural deny-list (§6.1: tokens are singular). A token ending in `s` is only
/// flagged if its singular (drop the trailing `s`) is itself a known singular token in the seed
/// vocabulary — so `comments`→`comment` is caught while `read_state`/`pass`/`status` are not.
fn looks_plural(token: &str) -> bool {
    // Tokens that legitimately end in `s` (not plurals): the hand-listed forms PLUS any token
    // that is ITSELF a canonical subsystem or artifact-type token (e.g. `refs` — a subsystem,
    // not the plural of `ref`; `status` is not in the seed but is hand-listed).
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
    // Flag iff the singular is a recognised subsystem or artifact-type token (i.e. the plural is
    // an obvious mistake of a known singular). This keeps the check precise, not greedy.
    SUBSYSTEM_TOKENS.contains(&singular) || ARTIFACT_TYPE_TOKENS.contains(&singular)
}

/// THE VALIDATOR (contract 2.9, the §6.1 grammar). Returns `Ok(())` for a well-formed canonical
/// `type` name, or the LOUD [`TaxonomyError`] for the first rule it broke. Pure + hermetic.
///
/// The grammar, in order: 2..=3 dotted segments, no empty segment, every token matches
/// `[a-z][a-z0-9_]*`, the leading token is a known subsystem, and the verb/noun tokens pass the
/// conservative singular + past-tense deny-lists (§6.1). This is the ONE gate every subsystem's
/// EB-24 token list is checked against (one grammar, no per-subsystem drift, EI-01 §7).
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
    // The leading token is a canonical subsystem (§6.2).
    let subsystem = segments[0];
    if !SUBSYSTEM_TOKENS.contains(&subsystem) {
        return Err(TaxonomyError::UnknownSubsystem {
            name: name.to_string(),
            token: subsystem.to_string(),
        });
    }
    // The trailing token is the event verb (past-tense, singular); the middle (if present) is
    // the artifact-type noun (singular). Apply the conservative deny-lists.
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

/// Convenience: is `name` a member of the registered [`SEED_EVENT_NAMES`] table? (The seed is
/// the representative set; subsystems complete it under EB-24.)
pub fn is_seed_token(name: &str) -> bool {
    SEED_EVENT_NAMES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **GREEN admit-fixture (the §6.1/§6.4 ratchet, GREEN half).** Every registered seed name
    /// — incl. the three NEW tokens `ci.check.updated`, `ci.result`, and the `initiative` type
    /// token (`issue.initiative.created`) — is admitted by the grammar validator. This is the
    /// dated proof the grammar admits the whole seed (0 false rejects).
    #[test]
    fn admit_fixture_every_seed_name_and_the_three_new_tokens_pass() {
        for name in SEED_EVENT_NAMES {
            assert!(
                validate(name).is_ok(),
                "seed name `{name}` was wrongly REJECTED: {:?}",
                validate(name)
            );
        }
        // The three NEW tokens, named explicitly (EB-02's headline deliverable).
        assert!(validate(new_tokens::CI_CHECK_UPDATED).is_ok());
        assert!(validate(new_tokens::CI_RESULT).is_ok());
        assert!(validate(new_tokens::ISSUE_INITIATIVE_CREATED).is_ok());
        // And `ci.result` really is the two-segment form (subsystem.event_name).
        assert_eq!(new_tokens::CI_RESULT.split('.').count(), 2);
    }

    /// **RED reject-fixture (the §6.1 ratchet, RED half).** A malformed `type` name is rejected
    /// with the specific rule it broke — 4+ distinct malformations: uppercase, plural,
    /// present-tense, single-segment, unknown subsystem, hyphen, leading digit, empty segment.
    #[test]
    fn reject_fixture_malformed_names_are_rejected_with_their_rule() {
        // uppercase token
        assert!(matches!(
            validate("git.PR.opened"),
            Err(TaxonomyError::BadToken { .. })
        ));
        // present-tense verb
        assert!(matches!(
            validate("git.pr.open"),
            Err(TaxonomyError::PresentTenseVerb { .. })
        ));
        // plural artifact-type token
        assert!(matches!(
            validate("git.comments.created"),
            Err(TaxonomyError::PluralToken { .. })
        ));
        // single segment (< 2)
        assert!(matches!(
            validate("opened"),
            Err(TaxonomyError::TooFewSegments { .. })
        ));
        // unknown subsystem token
        assert!(matches!(
            validate("billing.invoice.created"),
            Err(TaxonomyError::UnknownSubsystem { .. })
        ));
        // hyphen (not [a-z0-9_])
        assert!(matches!(
            validate("git.pull-request.opened"),
            Err(TaxonomyError::BadToken { .. })
        ));
        // leading digit
        assert!(matches!(
            validate("git.1pr.opened"),
            Err(TaxonomyError::BadToken { .. })
        ));
        // empty segment (doubled dot)
        assert!(matches!(
            validate("git..opened"),
            Err(TaxonomyError::EmptySegment { .. })
        ));
        // too many segments (> 3)
        assert!(matches!(
            validate("git.pr.review.opened"),
            Err(TaxonomyError::TooManySegments { .. })
        ));
    }

    /// The two-segment minimum is honoured (`ci.result`) AND the three-segment form
    /// (`ci.check.updated`) — both are valid per §6.1 ("two segments minimum, three when an
    /// artifact type clarifies").
    #[test]
    fn two_and_three_segment_forms_both_valid() {
        assert!(validate("ci.result").is_ok());
        assert!(validate("ci.check.updated").is_ok());
    }

    /// Underscored tokens (`break_glass`, `read_state`, `marked_ready`) are well-formed
    /// (`[a-z][a-z0-9_]*` admits the underscore) — the seed `chat.read_state.updated` and
    /// `git.pr.marked_ready` pass, and Identity's `identity.break_glass.invoked` obeys the
    /// grammar (see [`identity_tuple_written_and_siblings_are_admitted_by_the_grammar`] for
    /// the full corrected-naming proof).
    #[test]
    fn underscored_tokens_are_well_formed() {
        assert!(token_is_well_formed("break_glass"));
        assert!(token_is_well_formed("read_state"));
        assert!(token_is_well_formed("marked_ready"));
        assert!(validate("chat.read_state.updated").is_ok());
        assert!(validate("git.pr.marked_ready").is_ok());
        assert!(validate("identity.break_glass.invoked").is_ok());
    }

    /// **Regression (cross-crate contract gap, found & fixed post-hoc):** Identity's
    /// `myelin-identity::iam_events` tokens were originally minted with an `iam.` subsystem
    /// prefix. `SUBSYSTEM_TOKENS` has never admitted `iam` (see
    /// [`subsystem_token_set_is_the_frozen_canonical_set`] — the exclusion is deliberate), so
    /// every real outbox row of that shape hit [`TaxonomyError::UnknownSubsystem`] at the
    /// elected relay (`myelin-storage::pgrelay::validate_claimed_row`) and was quarantined
    /// (`outbox_quarantine` has `ON DELETE RESTRICT` back to `outbox`, so the relay has no
    /// automatic remediation path — an operator must act on the quarantine row directly; this
    /// rename stops new rows from being poisoned, it does not retroactively repair rows already
    /// quarantined). `identity` is ALREADY the canonical §6.2 subsystem token for
    /// this exact subsystem (the crate is `myelin-identity`), so the fix is a rename, not a
    /// second subsystem token: `iam_events` now emits `identity.tuple.written` /
    /// `identity.role.granted` / `identity.break_glass.invoked`. This proves each is admitted
    /// by THIS grammar (the real, load-bearing gate every outbox row is validated against
    /// before publish) — the concrete evidence that fixes
    /// `myelin-mcp/tests/git_effect_governed.rs::response_lost_retry_is_exactly_once_for_open_review_and_events`.
    #[test]
    fn identity_tuple_written_and_siblings_are_admitted_by_the_grammar() {
        // Exercise the ACTUAL cross-crate contract — myelin-identity's real token table,
        // not a copy-pasted literal that could silently drift from what iam_events.rs emits.
        for tok in myelin_identity::iam_events::IDENTITY_EVENT_TOKENS {
            assert!(validate(tok).is_ok(), "token `{tok}` must be admitted by this grammar");
        }
        // The old, never-admitted spelling is (and must remain) rejected — proving this is a
        // real fix, not a grammar loosening that would silently re-admit the bug.
        assert!(matches!(
            validate("iam.tuple.written"),
            Err(TaxonomyError::UnknownSubsystem { .. })
        ));
    }

    /// The two new check-seam tokens are registered in the seed table (§6.3) and the
    /// `initiative` type token appears in [`ARTIFACT_TYPE_TOKENS`] (§6.2 extension).
    #[test]
    fn the_three_new_tokens_are_registered_in_the_seed() {
        assert!(is_seed_token("ci.check.updated"));
        assert!(is_seed_token("ci.result"));
        assert!(is_seed_token("issue.initiative.created"));
        assert!(ARTIFACT_TYPE_TOKENS.contains(&"initiative"));
    }

    /// The subsystem token set is exactly the §6.2 canonical singular set (no `iam`, no aliases).
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
