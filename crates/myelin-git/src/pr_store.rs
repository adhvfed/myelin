//! # `pr_store` — the DURABLE PR/review store + the repo-owned branch-protection policy + the gated,
//! durable merge (GT-003 / E1.2)
//!
//! The hosting-layer entities Git owns ([`crate::lifecycle`] — PR, Review, branch-protection ruleset)
//! become **durable** here, so open-PR / review / merge survive a restart and drive the product-API write
//! path. Per the GT-003 prompt's offered option, the durable medium is **on-disk repo metadata** under
//! the bare repo dir, resolved through the SAME validated [`RepoPathResolver`] the durable git store uses
//! (tenant/region path-isolated + traversal-safe). The arch's long-term home for PR/review/policy rows is
//! the control-plane OLTP (PG via the MR-022 provider) — the named **GT-003b** follow-on.
//!
//! ## The security boundary (GT-003 verifier fix): policy is REPO-OWNED, never author-settable
//! Branch-protection POLICY — the required check set, the approval threshold, the CODEOWNERS/conversation
//! rules, the fork-endorsement requirement — lives in a **repo-owned** [`BranchProtectionConfig`]
//! (`<repo>.git/myelin/branch-protection.json`), set ONLY by an authorized repo-admin operation. A PR
//! record carries only FACTS (the head, the submitted reviews, the CI-reported green contexts, the
//! maintainer endorsements). The merge sources the required set + thresholds from the repo policy for the
//! target ref — NEVER from author-supplied PR fields — and a **protected ref defaults CLOSED** (an
//! unconfigured protected ref still requires a non-author approval). So a tenant member cannot merge by
//! supplying loose/null policy at PR-open (the proven bypass — closed).
//!
//! ## Anti-duplication
//! - The lifecycle STATE MACHINE / ruleset entity is [`crate::lifecycle`] — reused, not reimplemented.
//! - The merge GATE is [`crate::merge_gate`] (required-set + fork-trust) AND
//!   [`crate::lifecycle::evaluate_ruleset`] (approvals / CODEOWNERS / conversations) — reused verbatim. A
//!   merge advances the target ref via the durable per-ref CAS ([`crate::receive_pack::RefStore`]) ONLY
//!   after both admit AND the head is a valid fast-forward target — never a policy bypass, never an
//!   arbitrary oid.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::check_status::{
    CheckContext, CheckState, CheckStatus, CheckStatusProjection, GitOid, HumanisedRef, Timestamp,
    TrustTier,
};
use crate::core::{Oid as CoreOid, RepoLoc};
use crate::durable::{DurableError, DurableGitRepo};
use crate::gix_backend::{RepoPathResolver, RootedResolver};
use crate::lifecycle::{
    evaluate_ruleset, BranchProtectionRuleset, MergeContext, PrState, PrTransition, PullRequest,
    ReviewState, ReviewVerdict, RulesetOutcome,
};
use crate::merge_gate::{evaluate_merge_gate, MergeGateOutcome, MergeGatePolicy};
use crate::receive_pack::{
    CrashPoint, InMemoryObjectDb, Oid as PushOid, ProposedRefUpdate, PushOutcome, PushSession,
    Pusher, RefName, RefStore,
};

// ───────────────────────────── repo-owned branch-protection policy ────────────────────────────────

/// **The repo-owned branch-protection config** — the durable POLICY, set ONLY by an authorized repo
/// admin (never author input). A list of [`BranchProtectionRuleset`]s (reused entity), each protecting a
/// ref pattern; the first whose `matches(base_ref)` wins. Persisted at
/// `<repo>.git/myelin/branch-protection.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchProtectionConfig {
    /// The rulesets, in precedence order (first match wins).
    pub rulesets: Vec<BranchProtectionRuleset>,
}

impl BranchProtectionConfig {
    /// The first configured ruleset protecting `base_ref` (`None` if none matches).
    pub fn resolve(&self, base_ref: &str) -> Option<&BranchProtectionRuleset> {
        self.rulesets.iter().find(|r| r.matches(base_ref))
    }
}

/// The **effective** ruleset the merge enforces for `base_ref`:
/// - a CONFIGURED ruleset wins (the repo admin's policy — unbypassable);
/// - else if `base_ref` is a PROTECTED ref (the default branch / `release/*`), a **default-CLOSED**
///   built-in ruleset (require 1 non-author approval) — so an unconfigured protected ref is NOT a free
///   merge; the repo admin must opt into anything looser;
/// - else (an unprotected ref like a feature branch) no requirements.
///
/// The required set + thresholds therefore NEVER come from author input.
pub fn effective_ruleset(
    config: Option<&BranchProtectionConfig>,
    base_ref: &str,
) -> BranchProtectionRuleset {
    if let Some(rs) = config.and_then(|c| c.resolve(base_ref)) {
        return rs.clone();
    }
    if RefName::new(base_ref).is_protected() {
        // Default-CLOSED for a protected ref: at minimum a non-author approval is required.
        return BranchProtectionRuleset {
            ref_pattern: base_ref.to_string(),
            required_contexts: Vec::new(),
            required_approvals: 1,
            require_codeowner_review: false,
            require_conversation_resolution: false,
            allow_force_push: false,
        };
    }
    // An unprotected ref — no branch-protection requirements (still head-validated at merge).
    BranchProtectionRuleset {
        ref_pattern: base_ref.to_string(),
        required_contexts: Vec::new(),
        required_approvals: 0,
        require_codeowner_review: false,
        require_conversation_resolution: false,
        allow_force_push: false,
    }
}

// ───────────────────────────── the durable PR/review record (FACTS only) ──────────────────────────

/// A persisted review (the [`crate::lifecycle::Review`] entity, durable). The verdict lives in the
/// lifecycle [`ReviewState`]; the reviewer pseudonym + agent-legibility ride alongside (GIT-1 / ADR-08).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRecord {
    /// The reviewer's OPAQUE pseudonym (GIT-1).
    pub reviewer_pseudonym: String,
    /// The review lifecycle state (`Requested` / `Submitted(verdict)` / `Dismissed`).
    pub state: ReviewState,
    /// Agent legibility (ADR-08) — `true` iff the reviewer is an agent.
    pub is_agent: bool,
}

impl ReviewRecord {
    fn is_current_approval(&self) -> bool {
        matches!(self.state, ReviewState::Submitted(ReviewVerdict::Approve))
    }
    fn is_blocking(&self) -> bool {
        matches!(self.state, ReviewState::Submitted(ReviewVerdict::RequestChanges))
    }
}

/// The **durable pull-request record — FACTS ONLY** (the [`PullRequest`] entity + its reviews + the
/// CI/endorsement facts). It carries NO branch-protection policy: the required set + thresholds come from
/// the repo-owned [`BranchProtectionConfig`] at merge time, never from these fields. The check facts
/// (`green_contexts` / `fork_unendorsed_contexts` / `endorsed_contexts`) are produced by authorized
/// producers (the CI check-report path — the real producer is M4; the maintainer endorsement —
/// [`crate::fork_gate`]; the reviews — the review op), NOT by the PR author at open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRecord {
    /// The per-repo PR number (git's stable canonical key).
    pub number: u64,
    /// **The human title (R3.1 — required at create through the edge).** `#[serde(default)]` so a
    /// PR record persisted BEFORE the title store existed deserializes with an EMPTY title — the list
    /// renders such a legacy PR as its `#number` (an honest fallback, never a fabricated title). The
    /// on-disk JSON store's additive-field schema evolution is the durable equivalent of a migration
    /// (there is no `pr` SQL table to `ALTER` — the PG home is the named GT-003b follow-on).
    #[serde(default)]
    pub title: String,
    /// **The optional Markdown body (R3.1).** `None` for a legacy record or a PR opened without one.
    #[serde(default)]
    pub body_md: Option<String>,
    /// **Whether the PR AUTHOR is an agent (ADR-08 legibility).** Set at open from the opener's
    /// [`PrincipalKind`]; drives the row's four-channel agent badge (`is_agent` is REQUIRED, never
    /// disguised as a human). `#[serde(default)]` = `false` for a legacy record.
    #[serde(default)]
    pub author_is_agent: bool,
    /// **Last-touched wall-clock (unix seconds), for the list's "updated" column + `sort=updated`.**
    /// Set at open and bumped on each authored mutation through the edge. `None` for a legacy record
    /// (the row omits the timestamp rather than fabricating one).
    #[serde(default)]
    pub updated_at: Option<i64>,
    /// The lifecycle state.
    pub state: PrState,
    /// The base ref the PR targets (the ref a merge advances; the policy keys on it).
    pub base_ref: String,
    /// The head ref the PR proposes to land.
    pub head_ref: String,
    /// The head commit oid a merge advances `base_ref` to (validated as a real FF target at merge).
    pub head_oid: String,
    /// The PR author's OPAQUE pseudonym (GIT-1) — a self-approval by this pseudonym does NOT count.
    pub author_pseudonym: String,
    /// The reviews on this PR (durable; submitted via the authorized review op).
    pub reviews: Vec<ReviewRecord>,
    /// Contexts with a CURRENT TRUSTED success for `head_oid` (the CI check-report fact; producer M4).
    pub green_contexts: Vec<String>,
    /// Contexts with a CURRENT `untrusted_fork` success NOT yet endorsed — neutral-for-gating (Δ3).
    pub fork_unendorsed_contexts: Vec<String>,
    /// Contexts a maintainer endorsed via `approve_untrusted_ci` ([`crate::fork_gate`]).
    pub endorsed_contexts: Vec<String>,
    /// Whether the CODEOWNERS review requirement is satisfied (resolved by the owner Expand; default
    /// `false` — safe: a repo requiring CODEOWNERS blocks until genuinely satisfied).
    pub codeowner_review_satisfied: bool,
    /// The count of OUTSTANDING (unresolved) conversation threads.
    pub outstanding_conversations: u32,
}

impl PrRecord {
    /// Open a new PR record (FACTS empty) from the [`PullRequest`] entity. Carries NO policy — the merge
    /// reads the repo-owned ruleset.
    pub fn open(pr: &PullRequest, head_oid: impl Into<String>) -> PrRecord {
        PrRecord {
            number: pr.number,
            title: String::new(),
            body_md: None,
            author_is_agent: false,
            updated_at: None,
            state: pr.state,
            base_ref: pr.base_ref.clone(),
            head_ref: pr.head_ref.clone(),
            head_oid: head_oid.into(),
            author_pseudonym: pr.author_pseudonym.clone(),
            reviews: Vec::new(),
            green_contexts: Vec::new(),
            fork_unendorsed_contexts: Vec::new(),
            endorsed_contexts: Vec::new(),
            codeowner_review_satisfied: false,
            outstanding_conversations: 0,
        }
    }

    fn as_pull_request(&self) -> PullRequest {
        let mut pr = PullRequest::open(
            self.number,
            self.base_ref.clone(),
            self.head_ref.clone(),
            self.author_pseudonym.clone(),
            matches!(self.state, PrState::Draft),
        );
        pr.state = self.state;
        pr
    }

    /// Current approvals that COUNT toward the threshold — EXCLUDES the author's own review (no
    /// self-approval lets a PR meet its approval requirement).
    fn counting_approvals(&self) -> u32 {
        self.reviews
            .iter()
            .filter(|r| r.is_current_approval() && r.reviewer_pseudonym != self.author_pseudonym)
            .count() as u32
    }

    /// **The row-level review posture (R3.1 list VM).** `changes` if any current review requests
    /// changes; else `approved` if a non-author approval counts; else `requested` if any reviewer is
    /// still in the requested state; else `none`. Drives the quiet review marker on the list row.
    pub fn review_state_label(&self) -> &'static str {
        if self.reviews.iter().any(|r| r.is_blocking()) {
            "changes"
        } else if self.counting_approvals() > 0 {
            "approved"
        } else if self
            .reviews
            .iter()
            .any(|r| matches!(r.state, ReviewState::Requested))
        {
            "requested"
        } else {
            "none"
        }
    }

    /// **Is `viewer_pseudonym` a REQUESTED reviewer on this PR?** (the cross-repo "needs your review"
    /// bucket predicate + the row's "review requested" marker.) A requested review whose reviewer is
    /// the viewer — never leaks another reviewer's request.
    pub fn is_review_requested_of(&self, viewer_pseudonym: &str) -> bool {
        self.reviews.iter().any(|r| {
            matches!(r.state, ReviewState::Requested) && r.reviewer_pseudonym == viewer_pseudonym
        })
    }

    /// **The checks-summary rollup for the list row (R3.1 / gate Q4 — rolled up IN the list pass, no
    /// N+1).** Derived from the durable check FACTS on this record (the CI-reported `green_contexts`)
    /// against the REPO-OWNED `ruleset`'s required set — the same facts [`evaluate_merge`] reads, so
    /// the row never contradicts the merge gate. **Honest floor:** the record persists only SUCCESS
    /// facts (greens), so a required context that is not yet green is `pending` (verdict `running`) —
    /// this projection cannot distinguish a genuine FAILURE from a still-running check (`failing`
    /// stays 0 until the per-commit `check_status` projection is joined here; a named follow-on). The
    /// `verdict` is the load-bearing, leak-free signal; the counts refine it.
    pub fn checks_summary(&self, ruleset: &BranchProtectionRuleset) -> ChecksSummary {
        let total = ruleset.required_contexts.len() as u32;
        let passing = ruleset
            .required_contexts
            .iter()
            .filter(|c| self.green_contexts.iter().any(|g| g == *c))
            .count() as u32;
        let verdict = if total == 0 {
            // No required checks: "pass" if the record carries greens (a merged/green PR), else none.
            if self.green_contexts.is_empty() {
                ChecksVerdict::None
            } else {
                ChecksVerdict::Pass
            }
        } else if passing >= total {
            ChecksVerdict::Pass
        } else {
            ChecksVerdict::Running
        };
        ChecksSummary {
            verdict,
            passing,
            failing: 0,
            total,
        }
    }
}

/// The list-row checks verdict (glyph+label; the ring stays reserved for this CI trio). `None` = no
/// checks reported; `Unavailable` = the projection could not be read (the row FAILS STATIC — it still
/// lists, the checks glyph shows a neutral "checks unavailable", never a blanked row; ux-git #5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksVerdict {
    /// Every required check is green (or a green-carrying merged PR).
    Pass,
    /// A required check reported a failure (reserved — the record cannot yet witness this; see the
    /// [`PrRecord::checks_summary`] floor).
    Fail,
    /// A required check is not yet green (running/pending — the record cannot distinguish these).
    Running,
    /// No required checks and no greens reported.
    None,
    /// The checks projection could not be read for this row (degraded — fail static, still lists).
    Unavailable,
}

impl ChecksVerdict {
    /// The stable wire token the row VM emits (the frontend maps it to a glyph+label).
    pub fn as_str(self) -> &'static str {
        match self {
            ChecksVerdict::Pass => "pass",
            ChecksVerdict::Fail => "fail",
            ChecksVerdict::Running => "running",
            ChecksVerdict::None => "none",
            ChecksVerdict::Unavailable => "unavailable",
        }
    }
}

/// The rolled-up checks posture for one list row — `verdict` + the refining counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChecksSummary {
    /// The load-bearing, leak-free verdict.
    pub verdict: ChecksVerdict,
    /// Required contexts currently green.
    pub passing: u32,
    /// Required contexts witnessed as FAILED (0 until the check_status projection is joined — floor).
    pub failing: u32,
    /// Required contexts in total.
    pub total: u32,
}

impl ChecksSummary {
    /// The degraded summary a row shows when the checks projection could not be read (fail static).
    pub fn unavailable() -> ChecksSummary {
        ChecksSummary {
            verdict: ChecksVerdict::Unavailable,
            passing: 0,
            failing: 0,
            total: 0,
        }
    }
}

// ───────────────────────────── the merge-gate evaluation (reused logic) ───────────────────────────

/// The combined merge-gate decision over the repo-owned ruleset + the durable PR facts — the required-set
/// + fork-trust half ([`crate::merge_gate`]) AND the approvals / CODEOWNERS / conversations half
/// ([`crate::lifecycle::evaluate_ruleset`]). A merge is admitted ONLY when BOTH admit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeEval {
    /// The required-set + fork-trust posture outcome.
    pub gate: MergeGateOutcome,
    /// The approvals / CODEOWNERS / conversations outcome.
    pub ruleset: RulesetOutcome,
}

impl MergeEval {
    /// `true` iff BOTH halves admit — the only state that lets a merge advance the ref (0 policy bypass).
    pub fn admitted(&self) -> bool {
        self.gate.is_admitted() && self.ruleset.is_satisfied()
    }
}

/// A malformed required-context string in the repo-owned ruleset (the merge gate must never silently
/// treat an unparseable required context as absent — that would be an under-gated merge).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateInputError(pub String);

impl std::fmt::Display for GateInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "merge-gate input error: {}", self.0)
    }
}
impl std::error::Error for GateInputError {}

fn synthetic_fact(head: &GitOid, ctx: CheckContext, state: CheckState, trust: TrustTier) -> CheckStatus {
    use myelin_tenancy::{ArtifactRef, TenantId};
    CheckStatus {
        tenant: TenantId("_gate".into()),
        repo: ArtifactRef("myelin://_gate/git/repo/_".into()),
        commit_oid: head.clone(),
        context: ctx,
        state,
        required: true,
        run: ArtifactRef("myelin://_gate/ci/run/_".into()),
        run_attempt: 1,
        trust_tier: trust,
        details_ref: ArtifactRef("myelin://_gate/ci/run/_#s".into()),
        summary: HumanisedRef {
            template_key: "ci.check.updated".into(),
            args: Default::default(),
        },
        started_at: Timestamp("2026-06-29T00:00:00Z".into()),
        completed_at: Some(Timestamp("2026-06-29T00:01:00Z".into())),
        cost_settled: true,
    }
}

/// **Evaluate the merge gate over a REPO-OWNED ruleset + the durable PR facts.** The required set +
/// thresholds come from `ruleset` (repo policy — never author input); the facts (greens, endorsements,
/// approvals, conversations) come from the durable record. Reuses [`crate::merge_gate`] (required-set +
/// fork-trust) + [`crate::lifecycle::evaluate_ruleset`] (approvals/CODEOWNERS/conversations).
pub fn evaluate_merge(
    ruleset: &BranchProtectionRuleset,
    rec: &PrRecord,
) -> Result<MergeEval, GateInputError> {
    let policy = MergeGatePolicy::from_required_contexts(&ruleset.required_contexts)
        .map_err(|e| GateInputError(e.to_string()))?;
    let head = GitOid(rec.head_oid.clone());

    let mut proj = CheckStatusProjection::new();
    let parse = |s: &str| {
        crate::merge_gate::parse_required_context(s).map_err(|e| GateInputError(e.to_string()))
    };
    for c in &rec.green_contexts {
        proj.apply(&synthetic_fact(&head, parse(c)?, CheckState::Success, TrustTier::Trusted));
    }
    for c in &rec.fork_unendorsed_contexts {
        proj.apply(&synthetic_fact(&head, parse(c)?, CheckState::Success, TrustTier::UntrustedFork));
    }
    let endorsed: Vec<CheckContext> = rec
        .endorsed_contexts
        .iter()
        .map(|c| parse(c))
        .collect::<Result<_, _>>()?;

    // The required-set + fork-trust half (merge_gate owns it) — required set from REPO policy.
    let gate = evaluate_merge_gate(&policy, &proj, &head, &endorsed);

    // The approvals / CODEOWNERS / conversations half (lifecycle ruleset owns it). `required_contexts`
    // is intentionally EMPTY here — merge_gate owns the required-set check above (no duplication/drift).
    let ruleset_def = BranchProtectionRuleset {
        ref_pattern: ruleset.ref_pattern.clone(),
        required_contexts: Vec::new(),
        required_approvals: ruleset.required_approvals,
        require_codeowner_review: ruleset.require_codeowner_review,
        require_conversation_resolution: ruleset.require_conversation_resolution,
        allow_force_push: ruleset.allow_force_push,
    };
    let mctx = MergeContext {
        green_contexts: Vec::new(),
        current_approvals: rec.counting_approvals(),
        codeowner_review_satisfied: rec.codeowner_review_satisfied,
        has_blocking_review: rec.reviews.iter().any(|r| r.is_blocking()),
        outstanding_conversations: rec.outstanding_conversations,
    };
    let ruleset_outcome = evaluate_ruleset(&ruleset_def, &mctx);

    Ok(MergeEval {
        gate,
        ruleset: ruleset_outcome,
    })
}

// ───────────────────────────── the durable on-disk PR + policy store ───────────────────────────────

/// **The durable on-disk PR/review + branch-protection store.** PR records live as JSON under
/// `<root>/<tenant>/<region>/<repo>.git/myelin/prs/<n>.json`; the repo-owned branch-protection config
/// lives at `…/<repo>.git/myelin/branch-protection.json` — both resolved through the SAME validated
/// [`RepoPathResolver`] (tenant/region path-isolated + traversal-safe).
pub struct DurablePrStore<P: RepoPathResolver = RootedResolver> {
    resolver: P,
}

impl DurablePrStore<RootedResolver> {
    /// Root the store at the SAME on-disk root the durable git store uses.
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            resolver: RootedResolver::new(root),
        }
    }
}

impl<P: RepoPathResolver> DurablePrStore<P> {
    /// Build over a resolver (the placement resolver swaps in here behind the same port).
    pub fn new(resolver: P) -> Self {
        Self { resolver }
    }

    fn meta_dir(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        let repo_path = self
            .resolver
            .repo_path(repo)
            .map_err(|e| DurableError::Git(e.to_string()))?;
        Ok(repo_path.join("myelin"))
    }

    fn prs_dir(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        Ok(self.meta_dir(repo)?.join("prs"))
    }

    fn pr_path(&self, repo: &RepoLoc, number: u64) -> Result<PathBuf, DurableError> {
        Ok(self.prs_dir(repo)?.join(format!("{number}.json")))
    }

    fn protection_path(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        Ok(self.meta_dir(repo)?.join("branch-protection.json"))
    }

    fn write_atomic(
        &self,
        dir: &std::path::Path,
        file: &std::path::Path,
        bytes: &[u8],
    ) -> Result<(), DurableError> {
        std::fs::create_dir_all(dir)
            .map_err(|e| DurableError::Io(format!("create dir {}: {e}", dir.display())))?;
        let tmp = dir.join(format!(
            ".{}.tmp",
            file.file_name().and_then(|s| s.to_str()).unwrap_or("x")
        ));
        std::fs::write(&tmp, bytes)
            .map_err(|e| DurableError::Io(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, file)
            .map_err(|e| DurableError::Io(format!("rename {}: {e}", file.display())))?;
        Ok(())
    }

    // ── branch-protection policy (repo-owned) ──

    /// Persist (overwrite) the repo-owned branch-protection config. The CALLER must have authorized this
    /// as a repo-admin operation (the edge gates the distinct `git.repo.branch_protection.set` action;
    /// the production authorizer resolves `Id.check(repo_admin)`). Atomic temp-file + rename.
    pub fn put_protection(
        &self,
        repo: &RepoLoc,
        config: &BranchProtectionConfig,
    ) -> Result<(), DurableError> {
        let dir = self.meta_dir(repo)?;
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|e| DurableError::Io(format!("serialize branch-protection: {e}")))?;
        self.write_atomic(&dir, &self.protection_path(repo)?, &bytes)
    }

    /// Read the repo-owned branch-protection config (`None` if the repo has none configured).
    pub fn get_protection(
        &self,
        repo: &RepoLoc,
    ) -> Result<Option<BranchProtectionConfig>, DurableError> {
        let path = self.protection_path(repo)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes)
                    .map_err(|e| DurableError::Io(format!("parse {}: {e}", path.display())))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DurableError::Io(format!("read {}: {e}", path.display()))),
        }
    }

    /// The EFFECTIVE ruleset enforced for `base_ref` (repo policy, or default-closed for a protected
    /// ref) — the required set + thresholds the merge uses, never author input.
    pub fn effective_ruleset_for(
        &self,
        repo: &RepoLoc,
        base_ref: &str,
    ) -> Result<BranchProtectionRuleset, DurableError> {
        let config = self.get_protection(repo)?;
        Ok(effective_ruleset(config.as_ref(), base_ref))
    }

    // ── PR records (facts) ──

    /// Persist (create or overwrite) a PR record durably (atomic temp-file + rename).
    pub fn put(&self, repo: &RepoLoc, rec: &PrRecord) -> Result<(), DurableError> {
        let dir = self.prs_dir(repo)?;
        let bytes = serde_json::to_vec_pretty(rec)
            .map_err(|e| DurableError::Io(format!("serialize PR {}: {e}", rec.number)))?;
        self.write_atomic(&dir, &self.pr_path(repo, rec.number)?, &bytes)
    }

    /// Read a PR record from disk (`None` if absent). A FRESH store over the same root reads it back.
    pub fn get(&self, repo: &RepoLoc, number: u64) -> Result<Option<PrRecord>, DurableError> {
        let path = self.pr_path(repo, number)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes)
                    .map_err(|e| DurableError::Io(format!("parse {}: {e}", path.display())))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DurableError::Io(format!("read {}: {e}", path.display()))),
        }
    }

    /// Open a NEW PR durably — conflict if the number already exists.
    pub fn open_pr(&self, repo: &RepoLoc, rec: &PrRecord) -> Result<(), DurableError> {
        if self.get(repo, rec.number)?.is_some() {
            return Err(DurableError::Git(format!(
                "PR #{} already exists (conflict)",
                rec.number
            )));
        }
        self.put(repo, rec)
    }

    /// List every PR record under a repo (durable — loaded from disk).
    pub fn list(&self, repo: &RepoLoc) -> Result<Vec<PrRecord>, DurableError> {
        let dir = self.prs_dir(repo)?;
        let mut out = Vec::new();
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(DurableError::Io(format!("read_dir {}: {e}", dir.display()))),
        };
        for entry in rd {
            let entry = entry.map_err(|e| DurableError::Io(format!("dir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(bytes) = std::fs::read(&path) {
                    if let Ok(rec) = serde_json::from_slice::<PrRecord>(&bytes) {
                        out.push(rec);
                    }
                }
            }
        }
        out.sort_by_key(|r| r.number);
        Ok(out)
    }
}

// ───────────────────────────── the gated, durable merge ───────────────────────────────────────────

/// The outcome of a [`merge_pr`] attempt — loud + typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeAttempt {
    /// The gate admitted, the head is a valid FF target, the base ref advanced durably, the PR is
    /// `Merged`. Carries the new tip + the durable `update_seq`.
    Merged {
        /// The base ref that advanced.
        base_ref: String,
        /// The new tip (the head the PR landed).
        new_oid: String,
        /// The post-update durable generation.
        update_seq: u64,
    },
    /// The merge gate BLOCKED — no ref advance. Carries the evaluation.
    Blocked(MergeEval),
    /// The head_oid is not a valid merge target (non-existent / non-commit / not a fast-forward of base)
    /// — refused, no ref advance (never advance a protected ref to an arbitrary oid).
    InvalidHead(String),
    /// The ref CAS was refused by the push policy (a raced base, or a protected-ref rule) — no advance.
    RefRefused(crate::receive_pack::RejectReason),
}

/// **The gated, durable merge (GT-003).** Sources the required set + thresholds from the REPO-OWNED
/// `ruleset` (never author input), validates the head is a real FF target via the on-disk `repo`, and
/// ONLY on a fully-admitted gate advances `base_ref` to `head_oid` via the durable per-ref CAS + the
/// reused [`PullRequest::transition`]. A blocked gate / invalid head advances NOTHING.
pub fn merge_pr<P: RepoPathResolver>(
    store: &DurablePrStore<P>,
    repo_loc: &RepoLoc,
    number: u64,
    ref_store: &RefStore,
    repo: &DurableGitRepo,
    merger_pseudonym: &str,
) -> Result<MergeAttempt, DurableError> {
    let mut rec = store
        .get(repo_loc, number)?
        .ok_or_else(|| DurableError::NotFound(format!("PR #{number}")))?;

    // The REPO-OWNED policy for the target ref (never author input).
    let ruleset = store.effective_ruleset_for(repo_loc, &rec.base_ref)?;

    let eval = evaluate_merge(&ruleset, &rec).map_err(|e| DurableError::Git(e.to_string()))?;
    if !eval.admitted() {
        return Ok(MergeAttempt::Blocked(eval)); // 0 policy bypass.
    }

    // Validate the head is a REAL fast-forward target of the base (never advance to an arbitrary oid).
    let base = RefName::new(rec.base_ref.clone());
    let cur_tip: Option<CoreOid> = ref_store.tip(&base).map(|o| CoreOid::new(o.0));
    let head_core = CoreOid::new(rec.head_oid.clone());
    if !repo.object_is_commit(&head_core) {
        return Ok(MergeAttempt::InvalidHead(format!(
            "head_oid {} is not a commit in the repo",
            rec.head_oid
        )));
    }
    if !repo.is_fast_forward(cur_tip.as_ref(), &head_core)? {
        return Ok(MergeAttempt::InvalidHead(format!(
            "head_oid {} is not a fast-forward of {}",
            rec.head_oid, rec.base_ref
        )));
    }

    // The gate admitted + the head is valid — advance via the durable per-ref CAS.
    let expected_old = cur_tip
        .map(|o| PushOid::new(o.0))
        .unwrap_or_else(PushOid::zero);
    let head = PushOid::new(rec.head_oid.clone());
    let push = PushSession {
        updates: vec![ProposedRefUpdate {
            ref_name: base,
            expected_old,
            new_oid: head.clone(),
            forced: false,
            commit_oids: vec![head.clone()],
        }],
        quarantine: Vec::new(),
        pusher: Pusher {
            pseudonym: merger_pseudonym.to_string(),
            is_agent: false,
        },
    };
    let outcome = ref_store
        .receive(&push, &InMemoryObjectDb::new(), CrashPoint::None)
        .map_err(|e| DurableError::Git(format!("merge ref advance failed: {e:?}")))?;

    match outcome {
        PushOutcome::Accepted { moved, .. } => {
            let (_, new_oid, update_seq) = moved.into_iter().next().expect("one moved ref");
            let mut pr = rec.as_pull_request();
            pr.transition(PrTransition::Merge, true)
                .map_err(|e| DurableError::Git(format!("PR merge transition: {e}")))?;
            rec.state = pr.state;
            // Merge is a mutation: bump `updated_at` so `sort=updated` doesn't rank a merged
            // PR by its pre-merge activity (verifier note, R3.1).
            rec.updated_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or_default(),
            );
            store.put(repo_loc, &rec)?;
            Ok(MergeAttempt::Merged {
                base_ref: rec.base_ref,
                new_oid: new_oid.0,
                update_seq,
            })
        }
        PushOutcome::Rejected(reason) => Ok(MergeAttempt::RefRefused(reason)),
        PushOutcome::Crashed(_) => Err(DurableError::Git("merge ref advance crashed".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::DurableGitStore;
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Region, TenantId,
        Timestamp as EvTimestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::Arc;

    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("myelin-prstore-{tag}-{nanos}"));
        p
    }

    fn loc() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "core")
    }

    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            schema_ver: 1,
            occurred_at: EvTimestamp("2026-06-29T00:00:00Z".into()),
            recorded_at: EvTimestamp("2026-06-29T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:merge".into())),
        }
    }

    /// Seed main at c1, return (c1, c2) where c2 is a real descendant of c1 (a valid FF target). main is
    /// left pointing at c1.
    fn seed_main_then_descendant(repo: &DurableGitRepo) -> (CoreOid, CoreOid) {
        let (c1, _b1, _p1) = repo
            .build_file_commit("refs/heads/main", "a.txt", b"v1\n", "c1", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&c1), "create", "psn@acme.noreply")
            .unwrap();
        let (c2, _b2, _p2) = repo
            .build_file_commit("refs/heads/main", "a.txt", b"v2\n", "c2", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        (c1, c2)
    }

    fn open_record(number: u64, base: &str, head_oid: &str, author: &str) -> PrRecord {
        let pr = PullRequest::open(number, base, "refs/heads/feature", author, false);
        PrRecord::open(&pr, head_oid)
    }

    fn durable_ref_store(repo: Arc<DurableGitRepo>) -> RefStore {
        let outbox = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        RefStore::open_durable(repo, "core", ctx_base(), outbox, minter)
    }

    /// A repo-owned ruleset survives a FRESH store over the same root (durable, path-isolated).
    #[test]
    fn branch_protection_config_survives_a_fresh_store() {
        let root = temp_root("prot");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let cfg = BranchProtectionConfig {
            rulesets: vec![BranchProtectionRuleset {
                ref_pattern: "refs/heads/main".into(),
                required_contexts: vec!["ci/build".into()],
                required_approvals: 1,
                require_codeowner_review: false,
                require_conversation_resolution: false,
                allow_force_push: false,
            }],
        };
        store.put_protection(&loc(), &cfg).unwrap();
        let store2 = DurablePrStore::rooted(&root);
        assert_eq!(store2.get_protection(&loc()).unwrap(), Some(cfg));
        std::fs::remove_dir_all(&root).ok();
    }

    /// **A CORRUPT protection file is `Err`, NOT `Ok(None)` (the R0.2 fail-closed precondition).** The
    /// wire push gate (`git_durable.rs` step 3b) relies on this distinction: a MISSING file means "no
    /// protection configured" (`Ok(None)`, proceed), but an UNREADABLE/garbage `branch-protection.json`
    /// must surface as `Err` so the push path can fail CLOSED (reject) rather than silently disable the
    /// branch-protection gate. If this ever regressed to `Ok(None)` on corruption, a corrupt policy would
    /// re-open force-push/delete/un-CI'd-push on every protected ref.
    #[test]
    fn a_corrupt_protection_file_is_an_error_not_a_silent_none() {
        let root = temp_root("prot-corrupt");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        // Write a valid config so the file exists at the canonical path, then overwrite it with garbage
        // (the path helper is private, so locate the file by name under the store root).
        store
            .put_protection(&loc(), &BranchProtectionConfig::default())
            .unwrap();
        let mut path = None;
        for entry in walkdir(&root) {
            if entry.file_name().and_then(|s| s.to_str()) == Some("branch-protection.json") {
                path = Some(entry);
                break;
            }
        }
        let path = path.expect("branch-protection.json was written by put_protection");
        std::fs::write(&path, b"{ this is not valid json").unwrap();

        let result = store.get_protection(&loc());
        assert!(
            result.is_err(),
            "a corrupt branch-protection.json must be Err (fail-closed), got {result:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R3.1 — the title/body store round-trips durably** (a fresh store over the same root reads
    /// the title + body back).
    #[test]
    fn title_and_body_round_trip_durably() {
        let root = temp_root("title");
        let gitstore = DurableGitStore::rooted(&root);
        gitstore.create_repo(&loc()).unwrap();
        let store = DurablePrStore::rooted(&root);
        let mut rec = open_record(1, "refs/heads/main", &"a".repeat(40), "psn:author@acme");
        rec.title = "R2.4 MCP HITL server-side verdicts".into();
        rec.body_md = Some("The gate withholds until a human approves.".into());
        rec.author_is_agent = true;
        rec.updated_at = Some(1_752_000_000);
        store.open_pr(&loc(), &rec).unwrap();

        let back = DurablePrStore::rooted(&root)
            .get(&loc(), 1)
            .unwrap()
            .unwrap();
        assert_eq!(back.title, "R2.4 MCP HITL server-side verdicts");
        assert_eq!(back.body_md.as_deref(), Some("The gate withholds until a human approves."));
        assert!(back.author_is_agent);
        assert_eq!(back.updated_at, Some(1_752_000_000));
        std::fs::remove_dir_all(&root).ok();
    }

    /// **R3.1 — the on-disk "migration": a PR record written BEFORE the title store existed (no
    /// `title`/`body_md`/`author_is_agent`/`updated_at` keys) still deserializes** — the additive
    /// `#[serde(default)]` fields default (empty title → the list's honest `#number` fallback; not an
    /// error, not a fabricated title). This is the durable-schema-evolution analogue of a boot
    /// migration for the JSON store (there is no `pr` SQL table to `ALTER` — GT-003b).
    #[test]
    fn a_legacy_record_without_title_deserializes_with_defaults() {
        // A pre-R3.1 record shape: only the fields that existed before the title store.
        let legacy = serde_json::json!({
            "number": 7,
            "state": "Open",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "deadbeef",
            "author_pseudonym": "psn:old@acme",
            "reviews": [],
            "green_contexts": [],
            "fork_unendorsed_contexts": [],
            "endorsed_contexts": [],
            "codeowner_review_satisfied": false,
            "outstanding_conversations": 0
        });
        let rec: PrRecord = serde_json::from_value(legacy).expect("legacy record deserializes");
        assert_eq!(rec.number, 7);
        assert_eq!(rec.title, "", "no title → empty (the list renders #number, honest)");
        assert_eq!(rec.body_md, None);
        assert!(!rec.author_is_agent);
        assert_eq!(rec.updated_at, None);
    }

    /// **R3.1 — the checks-summary rollup matches the merge-gate facts** (pass when all required are
    /// green; running when a required one is not yet green; none when nothing is required/reported).
    #[test]
    fn checks_summary_rolls_up_from_greens_and_required_set() {
        let ruleset = BranchProtectionRuleset {
            ref_pattern: "refs/heads/main".into(),
            required_contexts: vec!["ci/build".into(), "ci/test".into()],
            required_approvals: 0,
            require_codeowner_review: false,
            require_conversation_resolution: false,
            allow_force_push: false,
        };
        let mut rec = open_record(1, "refs/heads/main", "abc", "psn:a@acme");

        // Nothing green yet → running (2 required, 0 passing).
        let s = rec.checks_summary(&ruleset);
        assert_eq!(s.verdict, ChecksVerdict::Running);
        assert_eq!((s.passing, s.failing, s.total), (0, 0, 2));

        // One green → still running.
        rec.green_contexts = vec!["ci/build".into()];
        assert_eq!(rec.checks_summary(&ruleset).verdict, ChecksVerdict::Running);

        // Both green → pass.
        rec.green_contexts = vec!["ci/build".into(), "ci/test".into()];
        let s = rec.checks_summary(&ruleset);
        assert_eq!(s.verdict, ChecksVerdict::Pass);
        assert_eq!(s.passing, 2);

        // No required contexts + no greens → none.
        let empty_rs = BranchProtectionRuleset {
            required_contexts: vec![],
            ..ruleset.clone()
        };
        let mut fresh = open_record(2, "refs/heads/main", "abc", "psn:a@acme");
        assert_eq!(fresh.checks_summary(&empty_rs).verdict, ChecksVerdict::None);
        // …but greens on an unrequired ref still read as pass (a green merged PR).
        fresh.green_contexts = vec!["ci/build".into()];
        assert_eq!(fresh.checks_summary(&empty_rs).verdict, ChecksVerdict::Pass);
    }

    /// Minimal recursive file walk (test-only) — returns every file path under `dir`.
    fn walkdir(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(walkdir(&p));
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    /// (a) **The bypass is CLOSED.** A protected ref defaults CLOSED: a PR carrying NO policy (the author
    /// cannot set any) is blocked — the repo-owned default requires a non-author approval.
    #[test]
    fn protected_ref_defaults_closed_no_author_policy_can_open_it() {
        let root = temp_root("closed");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (_c1, c2) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        store
            .open_pr(&loc(), &open_record(1, "refs/heads/main", &c2.0, "psn:author@acme"))
            .unwrap();
        let rs = durable_ref_store(repo.clone());
        let attempt = merge_pr(&store, &loc(), 1, &rs, &repo, "psn:author@acme").unwrap();
        assert!(matches!(attempt, MergeAttempt::Blocked(_)), "default-closed blocks: {attempt:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// (b) The SAME merge succeeds only with GENUINE greens (repo requires ci/build) + a NON-author
    /// approval — facts from the durable record, required set from REPO policy. A self-approval does NOT
    /// count.
    #[test]
    fn merge_admits_only_with_genuine_repo_required_checks_and_nonauthor_approval() {
        let root = temp_root("genuine");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (_c1, c2) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        store
            .put_protection(
                &loc(),
                &BranchProtectionConfig {
                    rulesets: vec![BranchProtectionRuleset {
                        ref_pattern: "refs/heads/main".into(),
                        required_contexts: vec!["ci/build".into()],
                        required_approvals: 1,
                        require_codeowner_review: false,
                        require_conversation_resolution: false,
                        allow_force_push: false,
                    }],
                },
            )
            .unwrap();
        store
            .open_pr(&loc(), &open_record(1, "refs/heads/main", &c2.0, "psn:author@acme"))
            .unwrap();
        let rs = durable_ref_store(repo.clone());

        // No greens, no approval → blocked.
        assert!(matches!(
            merge_pr(&store, &loc(), 1, &rs, &repo, "psn:m@acme").unwrap(),
            MergeAttempt::Blocked(_)
        ));

        // A self-approval by the AUTHOR + greens → still blocked (self-approval does not count).
        let mut rec = store.get(&loc(), 1).unwrap().unwrap();
        rec.green_contexts = vec!["ci/build".into()];
        rec.reviews.push(ReviewRecord {
            reviewer_pseudonym: "psn:author@acme".into(),
            state: ReviewState::Submitted(ReviewVerdict::Approve),
            is_agent: false,
        });
        store.put(&loc(), &rec).unwrap();
        assert!(
            matches!(merge_pr(&store, &loc(), 1, &rs, &repo, "psn:m@acme").unwrap(), MergeAttempt::Blocked(_)),
            "a self-approval must NOT satisfy the approval threshold"
        );

        // A genuine non-author approval + genuine green → admitted, ref advances to c2.
        let mut rec = store.get(&loc(), 1).unwrap().unwrap();
        rec.reviews.push(ReviewRecord {
            reviewer_pseudonym: "psn:reviewer@acme".into(),
            state: ReviewState::Submitted(ReviewVerdict::Approve),
            is_agent: false,
        });
        store.put(&loc(), &rec).unwrap();
        match merge_pr(&store, &loc(), 1, &rs, &repo, "psn:m@acme").unwrap() {
            MergeAttempt::Merged { new_oid, .. } => assert_eq!(new_oid, c2.0),
            other => panic!("expected Merged, got {other:?}"),
        }
        assert_eq!(rs.tip(&RefName::new("refs/heads/main")), Some(PushOid::new(c2.0)));
        std::fs::remove_dir_all(&root).ok();
    }

    /// (c) A non-existent or non-descendant head_oid is refused — the protected ref is never advanced to
    /// an arbitrary oid.
    #[test]
    fn arbitrary_or_nondescendant_head_oid_is_refused() {
        let root = temp_root("head");
        let gitstore = DurableGitStore::rooted(&root);
        let repo = Arc::new(gitstore.create_repo(&loc()).unwrap());
        let (c1, c2) = seed_main_then_descendant(&repo);
        let store = DurablePrStore::rooted(&root);
        let rs = durable_ref_store(repo.clone());
        // Create an UNPROTECTED base ref `feat` at c2 (so the gate admits — isolating the head check).
        rs.receive(
            &PushSession {
                updates: vec![ProposedRefUpdate {
                    ref_name: RefName::new("refs/heads/feat"),
                    expected_old: PushOid::zero(),
                    new_oid: PushOid::new(c2.0.clone()),
                    forced: false,
                    commit_oids: vec![PushOid::new(c2.0.clone())],
                }],
                quarantine: vec![],
                pusher: Pusher { pseudonym: "psn:m@acme".into(), is_agent: false },
            },
            &InMemoryObjectDb::new(),
            CrashPoint::None,
        )
        .unwrap();

        // A non-existent head → InvalidHead.
        let bogus = "0".repeat(40);
        store
            .open_pr(&loc(), &open_record(1, "refs/heads/feat", &bogus, "psn:author@acme"))
            .unwrap();
        assert!(matches!(
            merge_pr(&store, &loc(), 1, &rs, &repo, "psn:m@acme").unwrap(),
            MergeAttempt::InvalidHead(_)
        ));

        // A real commit that is an ANCESTOR (not descendant) of feat's tip c2 → not a fast-forward.
        store
            .open_pr(&loc(), &open_record(2, "refs/heads/feat", &c1.0, "psn:author@acme"))
            .unwrap();
        assert!(
            matches!(
                merge_pr(&store, &loc(), 2, &rs, &repo, "psn:m@acme").unwrap(),
                MergeAttempt::InvalidHead(_)
            ),
            "an ancestor (non-descendant) head is refused"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
