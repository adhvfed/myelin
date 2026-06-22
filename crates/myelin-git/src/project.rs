//! # `project` — `project(ref, viewer)` for git artifacts + the `ArtifactRef` id grammar
//! (GIT-P18 / P-279, M3-G3)
//!
//! This is the M3-G3 **projection half** of Git hosting: `project(ref, viewer)` (contract 5.6) — the
//! **only** way Refs/Search/Notif read about a git artifact (no cross-DB read), **per-viewer,
//! pre-permission-checked**. A viewer WITHOUT access to the artifact gets a [`Tombstone`], **never
//! the title** (0 title leaks to an unauthorized viewer — this feeds the M3-G5/M5 leak drills
//! GIT-D11, SRCH-D1/D3). And the **`ArtifactRef` id grammar** (contract 5.1 / REF-3): git's stored
//! canonical key is the sha / PR-number (`commit/<repo>:<sha>`, `pr/<repo>:<n>`) — the `#1421`-style
//! display is **render-time only** (0 stored display keys).
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `03-events-contracts-and-glue.md` §2 (the `ArtifactRef` id grammar — `<id>` is the stable
//!   mintable canonical key git OWNS, the sha / PR-number / repo-id, NEVER a render-time display
//!   form) + §3 (`project(ref, viewer) -> {title, state, icon, render_hint, sub_anchor?}` — the only
//!   way Refs/Search/Notif read git artifacts: permission first, then load own DB; erasure-safe;
//!   restriction-safe; always cell-local).
//! - `00-overview.md` §0.1 Δ7 (`pr/<repo>:<n>`, `commit/<repo>:<sha>` are the stored canonical keys;
//!   `#1421` is render-time).
//! - `reference-graph.md` §4.8 / `00-reconciliation-decisions.md` REF-3 (display keys are render-time
//!   only, NEVER the stored link).
//! - `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it — a viewer without access
//!   gets a tombstone, never the title; 0 leak is quantified).
//!
//! **Contracts implemented (to the frozen shapes):**
//! - **5.1** the `ArtifactRef` id grammar — git's stable canonical keys ([`git_pr_ref`],
//!   [`git_commit_ref`], [`git_repo_ref`], [`git_review_ref`]) mint through the ONE
//!   [`myelin_refs`] codec (0 ungrammatical refs by construction); the `#1421` display is a
//!   render-time projection ([`display_key`]) NEVER a stored key.
//! - **5.2 / 5.6** `project(ref, viewer)` for git artifacts (PR/commit/review/repo) — the per-viewer
//!   permission-checked projection ([`Projector::project`]); a denied viewer gets a [`Tombstone`].
//!
//! ## What this prompt (GIT-P18 / P-279) ships — and what it deliberately does NOT (VISION §3)
//! **Ships:**
//! 1. [`Projection`] / [`Tombstone`] — the frozen §3 projection shape (`title`, `state`, `icon`,
//!    `render_hint`, optional `sub_anchor`) + the erasure-safe tombstone.
//! 2. [`Projector`] — the `project(ref, viewer)` entry point: **permission FIRST** (`Id.check(viewer,
//!    view, ref.acl_object())` — deny ⇒ [`Tombstone`], never leak), THEN load the artifact from the
//!    own (in-memory here) store and build the per-viewer projection (PR/commit/review/repo).
//! 3. The **`ArtifactRef` id grammar helpers** ([`git_pr_ref`] etc.) + [`display_key`] (the
//!    render-time `#n` / short-sha display, NEVER stored) + the round-trip the GATE asserts.
//!
//! **Does NOT ship (FLOORS named — VISION §3):**
//! - **The live OLTP store** the projector reads is the GIT-P20 store-wiring follow-on; here the
//!   projector reads an in-memory [`ArtifactStore`] of the GIT-P16 lifecycle entities (the SAME
//!   entity shapes the live store will hydrate — the projection logic is store-agnostic).
//! - **The `blob`/`#L<a>-L<b>` content-anchored sub-projection** (the 4-state `live/moved/outdated/
//!   gone` resolver) is **GIT-P24** — this projector handles the `#comment-`/`#thread-` PR sub-anchor
//!   (a comment excerpt) and the bare-root PR/commit/review/repo projections; a blob line-range ref
//!   is a documented FLOOR here.
//! - **Cross-cell projection** is single-home (a viewer in cell A resolving a PR homed in cell B has
//!   cell B run `project`; only the rendered projection crosses) — the named multi-cell floor
//!   (contract 5.2 / OQ-I).
//! - **The CI `details_ref` link render** (a `check-`/`step-` sub belongs to CI, never read here) —
//!   git renders a `details_ref` only as a link into CI's run view (§2), never by reading CI's DB; a
//!   `check-`/`step-` sub on a git ref is out of scope for this projector (it is CI's).
//!
//! ## Why permission-FIRST (the 0-leak invariant — EI-01 §3 prove-it)
//! The order is load-bearing: the permission check runs BEFORE the artifact is loaded into the
//! projection, so a denied viewer's projection is a [`Tombstone`] built with NO field of the
//! artifact ever read into it — the title cannot leak because it is never fetched on the deny path.
//! This is asserted directly (the unauthorized-viewer test + the chained e2e). The mutation-score
//! floor for this module is stated below.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / prove-it)
//! `project(ref, viewer)` is the **mandatory-core leak-surface** (a leak is the failure). The floor
//! for this module is **≥ 80% of viable mutants caught**
//! (`cargo mutants -p myelin-git -f crates/myelin-git/src/project.rs`). The load-bearing logic — the
//! permission-first gate (deny ⇒ tombstone), each artifact-type projection arm, the erased/restricted
//! tombstone, the id-grammar mint/parse — each has a test a mutation flips. Measured 2026-06-22:
//! the permission-deny mutant (replace the `Decision::Allow` guard with `true`) is caught by
//! `unauthorized_viewer_gets_a_tombstone_never_the_title`; the erased-tombstone mutant by
//! `an_erased_artifact_projects_to_a_tombstone`; each projection arm by its
//! `project_*_for_authorized_viewer` test; the id-grammar arms by `git_*_ref_round_trips` /
//! `display_key_is_render_time_only`.

use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_refs::ArtifactRef;
use std::collections::HashMap;

use crate::check_status::GateOutcome;
use crate::lifecycle::{PrState, PullRequest, Review, ReviewState, ReviewVerdict};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE ArtifactRef ID GRAMMAR — git's stable canonical keys (contract 5.1, REF-3)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The `view` permission the projector checks before reading a git artifact (§5.2 — `pull_request.view
/// = parent_repo->pull`; a commit/blob/repo view inherits `repo.pull`). Spelled once so the projector
/// keys the live fragment on the one canonical string (mirrors [`crate::live_check::perm`]).
pub const VIEW: &str = "view";

/// The frozen git artifact types `project` projects (the `<type>` token of the canonical
/// `ArtifactRef`). A closed set — git is the resolver-owner of exactly these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitArtifactType {
    /// `git/pr/<repo>:<n>` — a pull request.
    Pr,
    /// `git/commit/<repo>:<sha>` — a commit (the sha is immutable).
    Commit,
    /// `git/review/<repo>:<n>:<reviewer>` — a review.
    Review,
    /// `git/repo/<repo_id>` — a repository.
    Repo,
    /// `git/blob/<repo>:<ref>:<path>` — a content-anchored file (the `#L<a>-L<b>` sub resolver is the
    /// GIT-P24 floor; this projector does not build a blob projection — it is named here for the
    /// closed-set match, and projecting one returns the documented [`ProjectError::BlobFloor`]).
    Blob,
}

/// Classify a parsed git `ArtifactRef` to its [`GitArtifactType`], or reject a ref that is not a git
/// artifact (a non-`git` subsystem, or a git type this projector does not own). The classification
/// reads the `<subsystem>`/`<type>` segments of the canonical URN — never a render-time display form.
fn classify(r: &ArtifactRef) -> Result<GitArtifactType, ProjectError> {
    // The canonical URN is `myelin://<tenant>/git/<type>/<id>[#<sub>]`; split the scope segments.
    let rest = r
        .0
        .strip_prefix("myelin://")
        .ok_or_else(|| ProjectError::NotAGitArtifact { reference: r.0.clone() })?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != "git" {
        return Err(ProjectError::NotAGitArtifact { reference: r.0.clone() });
    }
    match segments[2] {
        "pr" => Ok(GitArtifactType::Pr),
        "commit" => Ok(GitArtifactType::Commit),
        "review" => Ok(GitArtifactType::Review),
        "repo" => Ok(GitArtifactType::Repo),
        "blob" => Ok(GitArtifactType::Blob),
        other => Err(ProjectError::UnknownGitType { ty: other.to_string() }),
    }
}

/// Mint the canonical **PR** `ArtifactRef` (contract 5.1 / REF-3): `myelin://<tenant>/git/pr/<repo>:<n>`.
/// The `<repo>:<n>` id is git's STABLE canonical key — the PR number, never the render-time `#n`
/// display. Goes through the ONE [`myelin_refs`] codec so it is grammatical by construction (a
/// malformed scope is rejected loudly, not emitted).
pub fn git_pr_ref(tenant: &str, repo: &str, number: u64) -> ArtifactRef {
    parse_git(&format!("myelin://{tenant}/git/pr/{repo}:{number}"))
}

/// Mint the canonical **commit** `ArtifactRef`: `myelin://<tenant>/git/commit/<repo>:<sha>`. The
/// `<sha>` is the immutable content-address — git's stable key, never a short-sha display.
pub fn git_commit_ref(tenant: &str, repo: &str, sha: &str) -> ArtifactRef {
    parse_git(&format!("myelin://{tenant}/git/commit/{repo}:{sha}"))
}

/// Mint the canonical **review** `ArtifactRef`: `myelin://<tenant>/git/review/<repo>:<n>:<reviewer>`.
/// Keyed by the parent PR number + the reviewer's opaque pseudonym (a review is a child of a PR; the
/// id is stable for that `(pr, reviewer)`).
pub fn git_review_ref(tenant: &str, repo: &str, pr_number: u64, reviewer_pseudonym: &str) -> ArtifactRef {
    parse_git(&format!(
        "myelin://{tenant}/git/review/{repo}:{pr_number}:{reviewer_pseudonym}"
    ))
}

/// Mint the canonical **repo** `ArtifactRef`: `myelin://<tenant>/git/repo/<repo_id>`. The `<repo_id>`
/// is the stable repo identifier (relocatable, never node-pinned — STOR-5).
pub fn git_repo_ref(tenant: &str, repo_id: &str) -> ArtifactRef {
    parse_git(&format!("myelin://{tenant}/git/repo/{repo_id}"))
}

/// Parse a git URN through the ONE refs codec. Git mints only well-formed scopes (the inputs here are
/// composed from validated segments), so a parse failure is an internal invariant break — `expect`
/// surfaces it loudly rather than silently emitting a malformed ref.
fn parse_git(urn: &str) -> ArtifactRef {
    myelin_refs::parse(urn).expect("git mints a grammatical canonical ArtifactRef (contract 5.1)")
}

/// The **render-time display key** (REF-3, §4.8) — `#1421` for a PR, the 7-char short-sha for a
/// commit. This is the human-facing projection a UI renders; it is **NEVER a stored link / scope**
/// (the stored canonical key is the full `pr/<repo>:<n>` / `commit/<repo>:<sha>` — [`git_pr_ref`]
/// etc.). The 0-stored-display-keys gate asserts this string never round-trips back to a scope: it is
/// not parseable as an `ArtifactRef`. Returns `None` for an artifact type with no short display form
/// (repo/review render their slug/handle, not a `#n`/short-sha).
pub fn display_key(r: &ArtifactRef) -> Option<String> {
    let ty = classify(r).ok()?;
    let id = canonical_id(r)?;
    match ty {
        // PR: the `<repo>:<n>` id renders to `#<n>` (the trailing number after the last `:`).
        GitArtifactType::Pr => id.rsplit(':').next().map(|n| format!("#{n}")),
        // Commit: the `<repo>:<sha>` id renders to the 7-char short-sha (the conventional display).
        GitArtifactType::Commit => id.rsplit(':').next().map(short_sha),
        // Repo/review/blob: no `#n`/short-sha display form (slug/handle is render-time text, not a key).
        GitArtifactType::Repo | GitArtifactType::Review | GitArtifactType::Blob => None,
    }
}

/// The canonical `<id>` segment of a git `ArtifactRef` (the part after `git/<type>/`, before any
/// `#sub`). The stored canonical key. `None` for a non-git / malformed ref.
fn canonical_id(r: &ArtifactRef) -> Option<String> {
    let rest = r.0.strip_prefix("myelin://")?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != "git" {
        return None;
    }
    Some(segments[3].to_string())
}

/// The conventional 7-char short-sha display (or the whole sha if shorter). A `blake3:<hex>` oid
/// renders the first 7 hex chars after the `blake3:` algorithm prefix; a bare hex renders its first 7.
fn short_sha(sha: &str) -> String {
    let hex = sha.split_once(':').map(|(_, h)| h).unwrap_or(sha);
    hex.chars().take(7).collect()
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE PROJECTION SHAPE (contract 5.6, §3) — {title, state, icon, render_hint, sub_anchor?}
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A **per-viewer projection** of a git artifact (contract 5.6, §3). The humanisation projection
/// Refs/Search/Notif consume — `title`, `state`, an `icon` token, an optional `render_hint` (the
/// PR checks/approvals summary), and an optional `sub_anchor` (a `#comment-`/`#thread-` excerpt). The
/// projection is built ONLY after the per-viewer permission check passes ([`Projector::project`]); a
/// denied viewer gets a [`Tombstone`] instead, never this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The artifact title (PR title, commit subject line, review verdict label, repo slug). NEVER
    /// rendered for an unauthorized viewer (the 0-leak invariant — the deny path returns a tombstone
    /// that never reads this field).
    pub title: String,
    /// The artifact state token (`open`/`merged`/`draft`/`closed` for a PR; `verified`/`unverified`
    /// for a commit; the review verdict; the repo visibility).
    pub state: String,
    /// The icon token (`pr`/`commit`/`review`/`repo`) the UI renders. A frozen vocabulary.
    pub icon: String,
    /// An optional render hint — for a PR the checks/approvals summary the PR checks panel renders
    /// (humanised by Notif; the raw gate-state is carried, never a CI-supplied raw string). `None`
    /// for artifact types with no extra render context.
    pub render_hint: Option<RenderHint>,
    /// An optional sub-anchor projection — set when the projected ref carried a `#comment-`/`#thread-`
    /// sub (a comment excerpt). `None` for a bare-root projection.
    pub sub_anchor: Option<SubAnchor>,
}

/// The PR render hint (§3 — the `render_hint.checks/approvals/is_draft/trust`). The checks summary is
/// the Git-OWNED gate state ([`GateOutcome`] → a coarse green/red/neutral), never a CI-supplied raw
/// string (the PR checks panel humanises it; Git decides which facts gate, CI reports them — X-1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderHint {
    /// The coarse checks summary — `green` (all required green), `red` (≥1 required unmet), or
    /// `neutral` (no required contexts gate this PR). Humanised at render; never a raw CI string.
    pub checks: ChecksSummary,
    /// `n/m` approvals — `(current, required)`.
    pub approvals: (u32, u32),
    /// `true` iff the PR is a draft (not review-ready).
    pub is_draft: bool,
}

/// The coarse PR checks summary (§3 `render_hint.checks` — green/red/neutral). A humanisable enum,
/// never a raw CI string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksSummary {
    /// Every required context is currently green-and-acceptable (the gate is satisfied).
    Green,
    /// At least one required context is missing / not-green / un-endorsed-fork (the gate blocks).
    Red,
    /// No required contexts gate this PR (nothing to summarise green/red).
    Neutral,
}

/// A projected sub-anchor (a `#comment-`/`#thread-` excerpt, §3 `sub_anchor`). The line-range
/// (`#L<a>-L<b>`) content-anchored 4-state resolver is the GIT-P24 floor; this carries the
/// comment/thread excerpt only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    /// The sub kind label (`comment`/`thread`).
    pub kind: String,
    /// A short excerpt of the anchored sub-artifact (the comment's leading text).
    pub excerpt: String,
}

/// A **tombstone** — the projection of an artifact the viewer may NOT see, or that has been
/// erased/restricted (contract 5.6, §3 — erasure-safe / restriction-safe). It carries NO title and NO
/// content of the artifact (the 0-leak invariant): a denied viewer learns only "(this artifact is not
/// available to you)" — never the title, never the state. The optional `root` lets a backlink degrade
/// to "(deleted)" while still pointing at the (permission-checked) parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// Why the projection is a tombstone (denied / erased / restricted) — for the AUDIT log, NEVER
    /// rendered to the viewer (the viewer sees only the generic "(not available)" text).
    pub reason: TombstoneReason,
}

/// Why a projection degraded to a [`Tombstone`] (the audit reason; never leaked to the viewer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// The viewer is not authorised to view the artifact (`Id.check` denied) — the deny path. The
    /// projection NEVER reads the artifact's title (0 leak).
    Unauthorized,
    /// The artifact has been ERASED (a `git.*.erased` tombstone) — the content is gone; a tombstone
    /// is returned, never the erased content (contract 5.2).
    Erased,
    /// The viewer's subject is RESTRICTED (the GDPR `restrict` flag, §6) — the projection omits the
    /// restricted content (a tombstone for the restricted artifact).
    Restricted,
    /// The anchored CONTENT IS GONE — a content-anchored `#L<a>-L<b>` line-range resolved to the GONE
    /// state (the anchored lines are entirely absent from the new blob; X-4 §"Git line-ranges" /
    /// contract 5.7). The tombstone is rooted at the parent PR ("this referenced PR #N, that line is
    /// no longer present"), never a silent drop. Produced by [`crate::anchor::resolve`] (GIT-P24); the
    /// distinct reason keeps a content-gone line range separable from an ERASED artifact in the audit.
    ContentGone,
}

impl Tombstone {
    /// The generic, content-free text the VIEWER sees (never the title/state/reason). The same string
    /// regardless of reason — a denied viewer cannot distinguish "denied" from "erased" (no
    /// information leaks through the tombstone text either).
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

/// The result of [`Projector::project`]: either a per-viewer [`Projection`] (authorised + present) or
/// a [`Tombstone`] (denied / erased / restricted). The two-variant shape IS the §3 contract
/// (`Projection | Tombstone`) — a projector NEVER returns a bare title to a denied viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    /// The authorised, present projection.
    Visible(Projection),
    /// The denied / erased / restricted tombstone (no leaked content).
    Tombstoned(Tombstone),
}

impl Projected {
    /// `true` iff this is a visible projection (authorised + present).
    pub fn is_visible(&self) -> bool {
        matches!(self, Projected::Visible(_))
    }

    /// `true` iff this is a tombstone (denied / erased / restricted).
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    /// The projected title IF visible, else `None`. The 0-leak helper: a tombstone has no title to
    /// return, so a caller asserting "an unauthorized viewer never gets the title" reads `None` here.
    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

/// A loud, typed projection error (a malformed / non-git ref, or the named blob floor) — distinct from
/// a [`Tombstone`] (which is a SUCCESSFUL projection of a hidden artifact). An error means the ref is
/// not projectable AT ALL; a tombstone means it is projectable but hidden from this viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The ref is not a git artifact (wrong subsystem / malformed scope) — git's projector does not
    /// own it.
    NotAGitArtifact {
        /// The offending reference string.
        reference: String,
    },
    /// The `<type>` token is not a git type git's projector owns.
    UnknownGitType {
        /// The rejected type token.
        ty: String,
    },
    /// The artifact does not exist in the store (a dangling ref). Distinct from a tombstone: the ref
    /// is well-formed and the viewer MAY be authorised, but there is nothing to project.
    NotFound {
        /// The reference that resolved to nothing.
        reference: String,
    },
    /// A `blob`/`#L<a>-L<b>` ref — the content-anchored 4-state resolver now lives in
    /// [`crate::anchor`] (GIT-P24 / P-286, shipped). This projector does NOT build a blob projection
    /// inline: the L-range sub-resolve needs the OLD + NEW blob BYTES (read via the git2 object
    /// backend at the resolved head) which the per-viewer `project(ref, viewer)` seam does not carry —
    /// the Refs ladder calls [`crate::anchor::resolve`] directly for the sub-resolve step. So a bare
    /// `blob` ref handed to THIS projector returns this variant pointing the caller at the resolver
    /// (the projector handles PR/commit/review/repo + the `#comment-`/`#thread-` sub; the L-range is
    /// the anchor resolver's). The live OLTP blob-byte wiring that lets a projection embed a resolved
    /// L-range excerpt rides the GIT-P20 store wiring.
    BlobFloor {
        /// The blob reference deferred to GIT-P24.
        reference: String,
    },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAGitArtifact { reference } => {
                write!(f, "not a git artifact: `{reference}` — git's projector does not own this ref")
            }
            ProjectError::UnknownGitType { ty } => {
                write!(f, "unknown git artifact type `{ty}`")
            }
            ProjectError::NotFound { reference } => {
                write!(f, "no git artifact found for `{reference}` (dangling ref)")
            }
            ProjectError::BlobFloor { reference } => write!(
                f,
                "blob projection for `{reference}` is the GIT-P24 content-anchored resolver floor"
            ),
        }
    }
}

impl std::error::Error for ProjectError {}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE ARTIFACT STORE (the GIT-P20 live-store floor — in-memory GIT-P16 entities here)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A repository's projectable metadata (the `repo` artifact — slug + visibility). The live `repo`
/// OLTP row is GIT-P20; here it is the projection input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoMeta {
    /// The human-readable repo slug (e.g. `acme/payments`) — the repo projection title.
    pub slug: String,
    /// The repo visibility (`private`/`internal`/`public`) — the repo projection state.
    pub visibility: String,
}

/// A commit's projectable metadata (subject line + verified flag). The commit bytes live in the object
/// DB ([`crate::commit::Commit`]); the projector needs only the subject + verification posture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitMeta {
    /// The commit subject — the first line of the message (the projection title is `short_sha +
    /// subject`).
    pub subject: String,
    /// `true` iff the commit signature verified (the `verified`/`unverified` state).
    pub verified: bool,
}

/// The in-memory **artifact store** the projector reads (the GIT-P20 live-OLTP-store FLOOR). Keyed by
/// the canonical `ArtifactRef` string — the SAME entity shapes the live store will hydrate, so the
/// projection logic ([`Projector::project`]) is store-agnostic. Carries the erased/restricted flags
/// the §3 erasure-/restriction-safe tombstone reads.
#[derive(Clone, Debug, Default)]
pub struct ArtifactStore {
    /// PRs by canonical ref string.
    prs: HashMap<String, PullRequest>,
    /// The PR render-hint inputs (gate outcome, required-context count) by canonical PR ref string —
    /// the merge gate (GIT-P20) produces these; here they are seeded for the projection.
    pr_render: HashMap<String, (GateOutcome, u32, u32)>,
    /// Reviews by canonical ref string.
    reviews: HashMap<String, Review>,
    /// Commits by canonical ref string.
    commits: HashMap<String, CommitMeta>,
    /// Repos by canonical ref string.
    repos: HashMap<String, RepoMeta>,
    /// Comment excerpts by canonical `#comment-<id>` sub-URN string (the `sub_anchor` projection).
    comments: HashMap<String, String>,
    /// The set of canonical ref strings that have been ERASED (a `git.*.erased` tombstone) — projecting
    /// one returns an `Erased` tombstone, never the (gone) content.
    erased: std::collections::HashSet<String>,
    /// The set of canonical ref strings whose subject is RESTRICTED (the GDPR `restrict` flag) —
    /// projecting one returns a `Restricted` tombstone.
    restricted: std::collections::HashSet<String>,
}

impl ArtifactStore {
    /// A fresh empty store.
    pub fn new() -> ArtifactStore {
        ArtifactStore::default()
    }

    /// Insert a PR keyed by its canonical ref (with its render-hint inputs — the gate outcome + the
    /// `(current_approvals, required_approvals)` pair).
    pub fn put_pr(
        &mut self,
        canonical_ref: &ArtifactRef,
        pr: PullRequest,
        gate: GateOutcome,
        current_approvals: u32,
        required_approvals: u32,
    ) {
        self.pr_render.insert(
            canonical_ref.0.clone(),
            (gate, current_approvals, required_approvals),
        );
        self.prs.insert(canonical_ref.0.clone(), pr);
    }

    /// Insert a review keyed by its canonical ref.
    pub fn put_review(&mut self, canonical_ref: &ArtifactRef, review: Review) {
        self.reviews.insert(canonical_ref.0.clone(), review);
    }

    /// Insert a commit's projectable metadata keyed by its canonical ref.
    pub fn put_commit(&mut self, canonical_ref: &ArtifactRef, meta: CommitMeta) {
        self.commits.insert(canonical_ref.0.clone(), meta);
    }

    /// Insert a repo's projectable metadata keyed by its canonical ref.
    pub fn put_repo(&mut self, canonical_ref: &ArtifactRef, meta: RepoMeta) {
        self.repos.insert(canonical_ref.0.clone(), meta);
    }

    /// Insert a comment excerpt keyed by its canonical `#comment-<id>` sub-URN (the `sub_anchor`).
    pub fn put_comment_excerpt(&mut self, comment_ref: &ArtifactRef, excerpt: impl Into<String>) {
        self.comments.insert(comment_ref.0.clone(), excerpt.into());
    }

    /// Mark a canonical ref ERASED (a `git.*.erased` tombstone) — projecting it returns an `Erased`
    /// tombstone (erasure-safe, §3).
    pub fn mark_erased(&mut self, canonical_ref: &ArtifactRef) {
        self.erased.insert(canonical_ref.0.clone());
    }

    /// Mark a canonical ref's subject RESTRICTED (the GDPR `restrict` flag) — projecting it returns a
    /// `Restricted` tombstone (restriction-safe, §3).
    pub fn mark_restricted(&mut self, canonical_ref: &ArtifactRef) {
        self.restricted.insert(canonical_ref.0.clone());
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE PROJECTOR — project(ref, viewer): permission FIRST, then the per-viewer projection
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The git `project(ref, viewer)` projector (contract 5.6 — the GIT-P18 deliverable).** The ONLY
/// way Refs/Search/Notif read a git artifact (no cross-DB). Holds the [`IdentityService`] dependency
/// (the per-viewer permission source) + the [`ArtifactStore`] (the own-DB read — the GIT-P20 store
/// floor in-memory here). Generic over `I: IdentityService` so the front door wires the real Id
/// resolver and tests wire a deterministic one.
pub struct Projector<I: IdentityService> {
    /// The Identity dependency — the per-viewer `check(viewer, view, acl_object)` source.
    id: I,
    /// The own-DB read — the artifact entities the projection is built from (GIT-P20 live store floor).
    store: ArtifactStore,
}

impl<I: IdentityService> Projector<I> {
    /// Compose the projector over the Id dependency + the artifact store.
    pub fn new(id: I, store: ArtifactStore) -> Projector<I> {
        Projector { id, store }
    }

    /// A borrow of the underlying store (for the front door / drills to seed or inspect).
    pub fn store_mut(&mut self) -> &mut ArtifactStore {
        &mut self.store
    }

    /// **`project(ref, viewer) -> Projection | Tombstone` (contract 5.6, §3).**
    ///
    /// The order is the load-bearing invariant (the 0-leak gate):
    /// 1. **PERMISSION FIRST** — `Id.check(viewer, view, ref.acl_object())`. A `Deny` (or any
    ///    non-`Allow`, fail-closed) returns a [`Tombstone`] built with **NO field of the artifact
    ///    read into it** — the title cannot leak because it is never fetched on the deny path. An Id
    ///    transport error fails CLOSED (a tombstone, never a leak).
    /// 2. **ERASURE-/RESTRICTION-SAFE** — if the artifact is erased (a `git.*.erased` tombstone) or the
    ///    viewer's subject is restricted, return the corresponding tombstone (never the gone/restricted
    ///    content), even though the permission passed.
    /// 3. **LOAD THE OWN DB + BUILD THE PER-VIEWER PROJECTION** — only now read the artifact and build
    ///    the §3 `{title, state, icon, render_hint, sub_anchor?}` projection.
    ///
    /// `zookie` is the read-consistency fence (a strong zookie-stamped read for a security-sensitive
    /// projection; bounded-stale for an availability-tolerant unfurl). The `acl_object` is the
    /// artifact's own ref (the Id engine resolves `view` → `parent_repo->pull` via the §5.2 fragment).
    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        // Classify FIRST so a non-git / unknown-type ref is a loud error (not a tombstone — a
        // tombstone is for a HIDDEN git artifact, an error is for a non-projectable ref).
        let ty = classify(reference)?;
        if ty == GitArtifactType::Blob {
            // The blob `#L<a>-L<b>` content-anchored resolver is the named GIT-P24 floor.
            return Err(ProjectError::BlobFloor { reference: reference.0.clone() });
        }

        // ── 1. PERMISSION FIRST (the 0-leak gate). The acl_object is the artifact's own ref; the
        //    engine resolves `view` → `parent_repo->pull`. We check the ROOT (the `#sub`-stripped
        //    artifact) so a comment/thread sub inherits the parent PR's `view` (a sub is never more
        //    visible than its parent). A Deny / Conditional / Id-error all fail CLOSED to a tombstone.
        let acl_object = myelin_refs::strip_sub(reference);
        let at = Consistency { at_least: zookie, mode: ConsistencyMode::Strong };
        let permission = Permission(VIEW.to_string());
        let decision = self.id.check(viewer, &permission, &acl_object, &at, None);
        match decision {
            Ok(Decision::Allow) => { /* authorised — fall through to the erasure/restriction guards */ }
            // Deny, Conditional (no caveat satisfied at the projection seam), or an Id transport error
            // ALL fail closed: a tombstone with NO artifact field read (0 leak).
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Unauthorized,
                }));
            }
        }

        // ── 2. ERASURE-/RESTRICTION-SAFE (§3). Keyed on the ROOT (an erased PR tombstones its comments
        //    too). The permission passed, but the content is gone / restricted — a tombstone, never
        //    the content.
        if self.store.erased.contains(&acl_object.0) || self.store.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone { reason: TombstoneReason::Erased }));
        }
        if self.store.restricted.contains(&acl_object.0) || self.store.restricted.contains(&reference.0)
        {
            return Ok(Projected::Tombstoned(Tombstone { reason: TombstoneReason::Restricted }));
        }

        // ── 3. LOAD THE OWN DB + BUILD THE PER-VIEWER PROJECTION (§3).
        let sub_anchor = self.project_sub_anchor(reference);
        let projection = match ty {
            GitArtifactType::Pr => self.project_pr(&acl_object, sub_anchor)?,
            GitArtifactType::Commit => self.project_commit(&acl_object)?,
            GitArtifactType::Review => self.project_review(&acl_object)?,
            GitArtifactType::Repo => self.project_repo(&acl_object)?,
            // Blob is handled above (the floor); unreachable here.
            GitArtifactType::Blob => unreachable!("blob handled as the GIT-P24 floor above"),
        };
        Ok(Projected::Visible(projection))
    }

    /// Build the PR projection (§3 — `title`, `state` open|merged|draft|closed, `icon: pr`, the
    /// checks/approvals/is_draft render hint).
    fn project_pr(
        &self,
        root: &ArtifactRef,
        sub_anchor: Option<SubAnchor>,
    ) -> Result<Projection, ProjectError> {
        let pr = self
            .store
            .prs
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound { reference: root.0.clone() })?;
        let (gate, current, required) = self
            .store
            .pr_render
            .get(&root.0)
            .cloned()
            .unwrap_or((GateOutcome::AllRequiredGreen, 0, 0));
        let checks = match (&gate, required) {
            // No required contexts gate this PR → neutral (nothing to summarise).
            (_, 0) => ChecksSummary::Neutral,
            (GateOutcome::AllRequiredGreen, _) => ChecksSummary::Green,
            (GateOutcome::Blocked { .. }, _) => ChecksSummary::Red,
        };
        Ok(Projection {
            title: pr_title(pr),
            state: pr_state_token(pr.state).to_string(),
            icon: "pr".to_string(),
            render_hint: Some(RenderHint {
                checks,
                approvals: (current, required),
                is_draft: pr.state == PrState::Draft,
            }),
            sub_anchor,
        })
    }

    /// Build the commit projection (§3 — `title: short_sha + subject`, `state: verified?`,
    /// `icon: commit`).
    fn project_commit(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .commits
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound { reference: root.0.clone() })?;
        let short = canonical_id(root)
            .and_then(|id| id.rsplit(':').next().map(short_sha))
            .unwrap_or_default();
        Ok(Projection {
            title: format!("{short} {}", meta.subject),
            state: if meta.verified { "verified" } else { "unverified" }.to_string(),
            icon: "commit".to_string(),
            render_hint: None,
            sub_anchor: None,
        })
    }

    /// Build the review projection (§3 — the verdict label as title, the review state, `icon: review`).
    fn project_review(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let review = self
            .store
            .reviews
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound { reference: root.0.clone() })?;
        Ok(Projection {
            title: review_title(review),
            state: review_state_token(&review.state).to_string(),
            icon: "review".to_string(),
            render_hint: None,
            sub_anchor: None,
        })
    }

    /// Build the repo projection (§3 — `title: slug`, `state: visibility`, `icon: repo`).
    fn project_repo(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .repos
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound { reference: root.0.clone() })?;
        Ok(Projection {
            title: meta.slug.clone(),
            state: meta.visibility.clone(),
            icon: "repo".to_string(),
            render_hint: None,
            sub_anchor: None,
        })
    }

    /// Build the `#comment-`/`#thread-` sub-anchor projection (§3 `sub_anchor`) IF the ref carried a
    /// comment/thread sub. The `#L<a>-L<b>` line-range resolver is the GIT-P24 floor (returns `None`
    /// here — a blob ref is rejected before this is reached anyway). The comment excerpt is read from
    /// the store keyed on the full sub-URN.
    fn project_sub_anchor(&self, reference: &ArtifactRef) -> Option<SubAnchor> {
        let sub = myelin_refs::sub_kind(reference)?;
        match sub {
            myelin_refs::Sub::Comment(_) => Some(SubAnchor {
                kind: "comment".to_string(),
                excerpt: self.store.comments.get(&reference.0).cloned().unwrap_or_default(),
            }),
            myelin_refs::Sub::Thread(_) => Some(SubAnchor {
                kind: "thread".to_string(),
                excerpt: self.store.comments.get(&reference.0).cloned().unwrap_or_default(),
            }),
            // A line-range / other sub on a non-blob ref carries no comment excerpt here.
            _ => None,
        }
    }
}

// ────────────────────────────── projection text helpers (one place each) ──────────────────────────

/// The PR projection title — the first non-empty line of the PR body, else a stable `PR #<n>` label.
/// (The PR `title` field is the body's leading text in the GIT-P16/P17 model; a future explicit title
/// column overrides this — the projector reads whatever the live store hydrates.)
fn pr_title(pr: &PullRequest) -> String {
    let first_line = pr.body.md.lines().find(|l| !l.trim().is_empty());
    match first_line {
        Some(line) => line.trim().to_string(),
        None => format!("PR #{}", pr.number),
    }
}

/// The PR state token (§3 `state` — `open`/`merged`/`draft`/`closed`).
fn pr_state_token(state: PrState) -> &'static str {
    match state {
        PrState::Draft => "draft",
        PrState::Open => "open",
        PrState::Merged => "merged",
        PrState::Closed => "closed",
    }
}

/// The review projection title — the verdict label (`approved`/`changes requested`/`commented`/
/// `review requested`/`dismissed`).
fn review_title(review: &Review) -> String {
    match review.state {
        ReviewState::Requested => "review requested".to_string(),
        ReviewState::Submitted(ReviewVerdict::Approve) => "approved".to_string(),
        ReviewState::Submitted(ReviewVerdict::RequestChanges) => "changes requested".to_string(),
        ReviewState::Submitted(ReviewVerdict::Comment) => "commented".to_string(),
        ReviewState::Dismissed => "dismissed".to_string(),
    }
}

/// The review state token (§3 `state`).
fn review_state_token(state: &ReviewState) -> &'static str {
    match state {
        ReviewState::Requested => "requested",
        ReviewState::Submitted(_) => "submitted",
        ReviewState::Dismissed => "dismissed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::Body;
    use crate::check_status::{CheckContext, GateOutcome};
    use myelin_identity::{
        AuthzError, CaveatContext, Credential, ListObjectsResult, ObjectId, ObjectType, PrincipalId,
        PrincipalKind, Result as IdResult, RewriteTrace, SubjectTree, TupleDelta,
    };
    use myelin_tenancy::{Region, TenantId};
    use std::collections::HashSet;

    // ── a deterministic Id stub: a `view@object` allow-list (absent ⇒ Deny, fail-closed); a toggle to
    //    force a transport hiccup (the projector must then fail CLOSED to a tombstone). ──
    struct StubId {
        allow: HashSet<String>,
        hiccup: bool,
    }

    impl StubId {
        fn new() -> Self {
            Self { allow: HashSet::new(), hiccup: false }
        }
        fn allow_view(mut self, object: &ArtifactRef) -> Self {
            self.allow.insert(format!("view@{}", object.0));
            self
        }
        fn with_hiccup(mut self) -> Self {
            self.hiccup = true;
            self
        }
    }

    impl IdentityService for StubId {
        fn authenticate(&self, _c: &Credential) -> IdResult<Principal> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn check(
            &self,
            _s: &Principal,
            permission: &Permission,
            object: &ArtifactRef,
            _at: &Consistency,
            _caveat: Option<&CaveatContext>,
        ) -> IdResult<Decision> {
            if self.hiccup {
                return Err(AuthzError::Unavailable("forced Id break".into()));
            }
            let key = format!("{}@{}", permission.0, object.0);
            Ok(if self.allow.contains(&key) { Decision::Allow } else { Decision::Deny })
        }
        fn list_objects(
            &self,
            _s: &Principal,
            _p: &Permission,
            _t: &ObjectType,
            _at: &Consistency,
        ) -> IdResult<ListObjectsResult> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn list_subjects(
            &self,
            _o: &ObjectId,
            _p: &Permission,
            _at: &Consistency,
        ) -> IdResult<SubjectTree> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn explain(
            &self,
            _s: &Principal,
            _p: &Permission,
            _o: &ObjectId,
            _at: &Consistency,
        ) -> IdResult<RewriteTrace> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn delegation(
            &self,
            _a: &Principal,
            _t: &Principal,
        ) -> IdResult<myelin_identity::EffectivePolicy> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn write_tuples(
            &self,
            _d: &[TupleDelta],
            _p: Option<&myelin_identity::Precondition>,
        ) -> IdResult<Zookie> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn mint_run_token(
            &self,
            _a: &PrincipalId,
            _r: &myelin_identity::RunId,
            _d: &myelin_identity::DelegationCaveats,
            _t: &myelin_identity::FailStaticBound,
        ) -> IdResult<myelin_identity::RunToken> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn revoke(&self, _t: &myelin_identity::RevokeTarget) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn resolve_pseudonym(&self, _s: &PrincipalId, _t: &TenantId) -> IdResult<String> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn erase(&self, _s: &PrincipalId) -> IdResult<()> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
        fn admit_fragment(
            &self,
            _f: &myelin_identity::NamespaceFragment,
        ) -> IdResult<myelin_identity::FragmentAdmit> {
            Err(AuthzError::NotYetImplemented("n/a"))
        }
    }

    fn viewer(id: &str) -> Principal {
        Principal::new(
            TenantId("acme".into()),
            Region("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        )
    }

    fn a_pr() -> PullRequest {
        let mut pr = PullRequest::open(42, "refs/heads/main", "refs/heads/feature", "psn:alice", false);
        pr.body = Body::new("Fix the charge race condition\n\nmore detail", vec![]);
        pr
    }

    fn z() -> Zookie {
        Zookie("z0".into())
    }

    // ════════ 1. THE ArtifactRef ID GRAMMAR (contract 5.1 / REF-3) ════════

    #[test]
    fn git_pr_and_commit_refs_round_trip_canonical_keys() {
        let pr = git_pr_ref("acme", "repo7", 4291);
        assert_eq!(myelin_refs::format(&pr), "myelin://acme/git/pr/repo7:4291");
        // re-parse is a fixed point (the stored key is stable).
        assert_eq!(myelin_refs::parse(&myelin_refs::format(&pr)).unwrap(), pr);

        let c = git_commit_ref("acme", "repo7", "blake3:deadbeefcafe0000");
        assert_eq!(myelin_refs::format(&c), "myelin://acme/git/commit/repo7:blake3:deadbeefcafe0000");

        let repo = git_repo_ref("acme", "repo7");
        assert_eq!(myelin_refs::format(&repo), "myelin://acme/git/repo/repo7");

        let rv = git_review_ref("acme", "repo7", 4291, "psn:bob");
        assert_eq!(myelin_refs::format(&rv), "myelin://acme/git/review/repo7:4291:psn:bob");
    }

    #[test]
    fn display_key_is_render_time_only_never_a_stored_scope() {
        // PR `#1421` is render-time only — it does NOT round-trip back to a scope (0 stored display keys).
        let pr = git_pr_ref("acme", "repo7", 1421);
        assert_eq!(display_key(&pr).as_deref(), Some("#1421"));
        // the display key is NOT parseable as an ArtifactRef (REF-3 — it is never a stored link).
        assert!(myelin_refs::parse("#1421").is_err());

        // commit short-sha is the 7-char display, also not a scope.
        let c = git_commit_ref("acme", "repo7", "blake3:deadbeefcafef00d");
        assert_eq!(display_key(&c).as_deref(), Some("deadbee"));
        assert!(myelin_refs::parse("deadbee").is_err());

        // repo/review have no `#n`/short-sha display form.
        assert_eq!(display_key(&git_repo_ref("acme", "r")), None);
        assert_eq!(display_key(&git_review_ref("acme", "r", 1, "psn:x")), None);
    }

    #[test]
    fn classify_rejects_a_non_git_ref() {
        let issue = myelin_refs::parse("myelin://acme/issue/issue/ENG-1").unwrap();
        assert!(matches!(classify(&issue), Err(ProjectError::NotAGitArtifact { .. })));
    }

    // ════════ 2. project(ref, viewer) — PERMISSION FIRST (the 0-leak gate) ════════

    #[test]
    fn authorized_viewer_gets_the_pr_projection() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let mut store = ArtifactStore::new();
        store.put_pr(
            &pr_ref,
            a_pr(),
            GateOutcome::Blocked { unmet: vec![CheckContext::ci("ci/build")] },
            1,
            2,
        );
        let id = StubId::new().allow_view(&pr_ref);
        let p = Projector::new(id, store);

        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_visible());
        assert_eq!(got.title(), Some("Fix the charge race condition"));
        if let Projected::Visible(proj) = got {
            assert_eq!(proj.state, "open");
            assert_eq!(proj.icon, "pr");
            let hint = proj.render_hint.expect("a PR carries a render hint");
            assert_eq!(hint.checks, ChecksSummary::Red); // a required context is unmet → red.
            assert_eq!(hint.approvals, (1, 2));
            assert!(!hint.is_draft);
        }
    }

    #[test]
    fn unauthorized_viewer_gets_a_tombstone_never_the_title() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 2, 0);
        // the Id stub allows NOBODY (the allow-list is empty) → every check denies.
        let p = Projector::new(StubId::new(), store);

        let got = p.project(&pr_ref, &viewer("mallory"), z()).unwrap();
        assert!(got.is_tombstone(), "an unauthorized viewer must get a tombstone");
        // THE 0-LEAK INVARIANT: the title is NEVER returned to the denied viewer.
        assert_eq!(got.title(), None, "0 title leak — the denied viewer never gets the title");
        if let Projected::Tombstoned(t) = got {
            assert_eq!(t.reason, TombstoneReason::Unauthorized);
            assert_eq!(t.display_text(), "(not available)"); // content-free.
        }
    }

    #[test]
    fn an_id_hiccup_fails_closed_to_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        // the Id ALLOWS alice, but the transport hiccups — the projector must fail CLOSED (no leak).
        let id = StubId::new().allow_view(&pr_ref).with_hiccup();
        let p = Projector::new(id, store);

        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_tombstone(), "an Id hiccup fails closed to a tombstone (never a leak)");
        assert_eq!(got.title(), None);
    }

    #[test]
    fn an_erased_artifact_projects_to_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.mark_erased(&pr_ref);
        // the viewer IS authorised, but the artifact is erased → an erasure-safe tombstone (§3).
        let id = StubId::new().allow_view(&pr_ref);
        let p = Projector::new(id, store);

        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_tombstone());
        assert_eq!(got.title(), None, "an erased artifact never leaks its (gone) title");
        if let Projected::Tombstoned(t) = got {
            assert_eq!(t.reason, TombstoneReason::Erased);
        }
    }

    #[test]
    fn a_restricted_subject_projects_to_a_tombstone() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.mark_restricted(&pr_ref);
        let p = Projector::new(StubId::new().allow_view(&pr_ref), store);
        let got = p.project(&pr_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_tombstone());
        if let Projected::Tombstoned(t) = got {
            assert_eq!(t.reason, TombstoneReason::Restricted);
        }
    }

    #[test]
    fn project_commit_review_and_repo_for_authorized_viewer() {
        let commit_ref = git_commit_ref("acme", "repo7", "blake3:deadbeefcafe");
        let review_ref = git_review_ref("acme", "repo7", 42, "psn:bob");
        let repo_ref = git_repo_ref("acme", "repo7");
        let mut store = ArtifactStore::new();
        store.put_commit(
            &commit_ref,
            CommitMeta { subject: "Fix the leak".into(), verified: true },
        );
        let mut review = Review::request("psn:bob", false);
        review.submit(ReviewVerdict::Approve).unwrap();
        store.put_review(&review_ref, review);
        store.put_repo(
            &repo_ref,
            RepoMeta { slug: "acme/payments".into(), visibility: "private".into() },
        );
        let id = StubId::new()
            .allow_view(&commit_ref)
            .allow_view(&review_ref)
            .allow_view(&repo_ref);
        let p = Projector::new(id, store);

        // commit: `short_sha + subject`, verified.
        let c = p.project(&commit_ref, &viewer("alice"), z()).unwrap();
        assert_eq!(c.title(), Some("deadbee Fix the leak"));
        if let Projected::Visible(proj) = &c {
            assert_eq!(proj.state, "verified");
            assert_eq!(proj.icon, "commit");
        }

        // review: the verdict label + the review state.
        let r = p.project(&review_ref, &viewer("alice"), z()).unwrap();
        assert_eq!(r.title(), Some("approved"));
        if let Projected::Visible(proj) = &r {
            assert_eq!(proj.state, "submitted");
            assert_eq!(proj.icon, "review");
        }

        // repo: slug + visibility.
        let repo = p.project(&repo_ref, &viewer("alice"), z()).unwrap();
        assert_eq!(repo.title(), Some("acme/payments"));
        if let Projected::Visible(proj) = &repo {
            assert_eq!(proj.state, "private");
            assert_eq!(proj.icon, "repo");
        }
    }

    #[test]
    fn a_pr_comment_sub_anchor_projects_an_excerpt_and_inherits_the_parent_permission() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let comment_ref =
            myelin_refs::mint(&pr_ref, myelin_refs::Sub::Comment("c9".into())).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.put_comment_excerpt(&comment_ref, "this looks risky");
        // the Id allows `view` on the ROOT PR — the comment sub inherits it (a sub is never more
        // visible than its parent; the projector checks the stripped root).
        let p = Projector::new(StubId::new().allow_view(&pr_ref), store);

        let got = p.project(&comment_ref, &viewer("alice"), z()).unwrap();
        assert!(got.is_visible());
        if let Projected::Visible(proj) = got {
            let anchor = proj.sub_anchor.expect("a comment sub carries a sub_anchor");
            assert_eq!(anchor.kind, "comment");
            assert_eq!(anchor.excerpt, "this looks risky");
        }
    }

    #[test]
    fn a_comment_sub_is_tombstoned_when_the_parent_pr_is_denied() {
        let pr_ref = git_pr_ref("acme", "repo7", 42);
        let comment_ref =
            myelin_refs::mint(&pr_ref, myelin_refs::Sub::Comment("c9".into())).unwrap();
        let mut store = ArtifactStore::new();
        store.put_pr(&pr_ref, a_pr(), GateOutcome::AllRequiredGreen, 0, 0);
        store.put_comment_excerpt(&comment_ref, "secret excerpt");
        // the Id allows NOBODY → the comment's parent PR is denied → the comment is tombstoned, the
        // excerpt never leaks.
        let p = Projector::new(StubId::new(), store);
        let got = p.project(&comment_ref, &viewer("mallory"), z()).unwrap();
        assert!(got.is_tombstone());
        assert_eq!(got.title(), None);
    }

    #[test]
    fn a_blob_ref_is_the_named_git_p24_floor() {
        let blob = myelin_refs::parse("myelin://acme/git/blob/repo7:main:lib.rs").unwrap();
        let p = Projector::new(StubId::new().allow_view(&blob), ArtifactStore::new());
        assert!(matches!(
            p.project(&blob, &viewer("alice"), z()),
            Err(ProjectError::BlobFloor { .. })
        ));
    }

    #[test]
    fn a_dangling_ref_is_not_found_not_a_tombstone() {
        // authorised, but nothing in the store → NotFound (distinct from a tombstone).
        let pr_ref = git_pr_ref("acme", "repo7", 999);
        let p = Projector::new(StubId::new().allow_view(&pr_ref), ArtifactStore::new());
        assert!(matches!(
            p.project(&pr_ref, &viewer("alice"), z()),
            Err(ProjectError::NotFound { .. })
        ));
    }
}
