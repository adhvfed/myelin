//! # `anchor` — content-anchored inline-thread line ranges: the `#sub` 4-state resolver
//! (GIT-P24 / P-286, M3-G4, GIT-D7)
//!
//! This is the M3-G4 **content-anchored line-range resolver** half of Git hosting (contract 5.7,
//! reconciliation X-4 / OQ-D). A PR inline thread is anchored to "line `a`..`b` of file X"; that
//! anchor MUST survive force-push, rebase, amend and base-branch movement, and degrade **legibly**
//! when it cannot — **never silently wrong** (EI-01 §3 prove-it: 0 mis-anchored is quantified).
//!
//! The mint half ([`crate::subs::mint_blob_line_range`], GIT-P4) ships the grammatical
//! `…/git/blob/<repo>:<ref>:<path>#L<a>-L<b>` sub-URN. The [`crate::project`] projector defers a
//! `blob`/`#L<a>-L<b>` ref to this module (`ProjectError::BlobFloor`). **This module is the resolver
//! the Refs ladder calls** in step 3 (the owner's sub-resolve step): Refs handles permission → root →
//! erased; Git answers the sub-resolve for an `L`-range.
//!
//! **Owning architecture doc (read in full before changing this):**
//! `04-subsystem-architectures/git-hosting/architecture/02-internals-and-algorithms.md` §5 (the
//! diff-anchoring 4-state resolver, OQ-D — `(anchor_blob_oid, path, side, line-range,
//! anchored_commit_oid, anchor_fingerprint = BLAKE3(anchored lines + context window))`, resolving
//! into exactly one of `exact(live) / rebased(moved) / partial(outdated) / tombstone(content_gone)`);
//! `00-overview.md` §0.1 Δ4 (the frozen `#sub` resolver); `01-tech-and-data-model.md` §1 (imara-diff
//! + BLAKE3 named as the diff + fingerprint primitives). **Reconciliation:**
//! `00-reconciliation-decisions.md` X-4 §"Git line-ranges" (the four states) + §"the one resolution
//! ladder" (Git is the owner's sub-anchor resolver the ladder calls in step 3). **Contract:**
//! `contract-index.md` row 5.7 (BLAKE3 fingerprint + 3-way context match → exact/rebased/partial/
//! tombstone).
//!
//! ## The 4-state ladder (contract 5.7 / X-4 / arch §5.1) — never silently wrong
//! Given a [`LineAnchor`] minted against `old_blob` and the file's `new_blob` at the resolved head:
//!
//! 1. **EXACT → [`AnchorState::Live`]** — the mint-time blob oid is still present at `path` (the file
//!    was untouched), OR the fingerprinted lines sit byte-identical at the same position. Return the
//!    exact range.
//! 2. **REBASED → [`AnchorState::Moved`]** — the blob changed but the fingerprinted lines are found at
//!    a SHIFTED position (a 3-way context match: the anchored lines + the surrounding context window
//!    matched as a contiguous run elsewhere in `new_blob`). Return the shifted range, flagged `moved`.
//! 3. **PARTIAL → [`AnchorState::Outdated`]** — SOME anchored lines survive, some are gone (no
//!    contiguous full-block match). Return the surviving sub-range, flagged `outdated` (Git's named
//!    "outdated-line-range" case).
//! 4. **GONE → [`AnchorState::Gone`]** — the anchored content is ENTIRELY absent from `new_blob`.
//!    Return a tombstone whose `root` is the PR (`reason: content_gone`), never silently dropping the
//!    thread.
//!
//! ## "View in original context" (never silently wrong)
//! For every NON-`Live` resolution the [`Resolution`] carries the ORIGINAL anchored position + the
//! mint-time blob oid so the UI can always offer **"view in original context"** ([`Resolution::
//! original_context`]) — a Moved/Outdated/Gone anchor is always RENDERED with its resolution state and
//! a link back to where the comment was originally written. The render state is on the resolution; a
//! consumer can never read a relocated range without also reading that it relocated (the 0-silent
//! invariant).
//!
//! ## FLOOR named (GF-5 — EI-01 §1)
//! v1 (this prompt) does **per-pair blob-diff fingerprint remap** + the four states: it resolves an
//! anchor against ONE new blob (the head blob). The named follow-on is **GF-5 — patch-id-chain
//! carry-over** across a MULTI-COMMIT rebase (matching `git patch-id` across the pre/post-rebase
//! commit sequence so a thread follows a *rebased* hunk through the whole rebase rather than degrading
//! to Outdated when an intermediate commit perturbs context). That is **GIT-P33 / M5 (R-6)** —
//! architecture §5.2. v1 uses the single-pair fingerprint remap; GF-5 hardens the `rebased→Moved` case
//! across the chain.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / prove-it)
//! The 4-state resolver is **mandatory-core** — a SILENT mis-anchor (a relocated/lost range reported
//! with the wrong state) is THE failure mode GIT-D7 quantifies. The floor for this module is
//! **≥ 90% of viable mutants caught** (`cargo mutants -p myelin-git -f
//! crates/myelin-git/src/anchor.rs`). Each load-bearing rung has a test a mutation flips: the EXACT
//! fast path + the at-position fingerprint (`exact_match_*`, `same_position_*`); the REBASED
//! fingerprint scan + its guards (`shifted_block_*`, `find_fingerprint_match_*`); the PARTIAL survival
//! scan + the structural-line filter (`partial_survival_*`, `surviving_subrange_*`,
//! `is_survival_evidence_*`); the GONE tombstone (`entirely_gone_*`); the boundary guards
//! (`range_to_indices_is_loud_*`); and the whole rebase corpus at 0 mis-anchored
//! (`e2e_git_d7_anchor_resolution`). Measured 2026-06-22: **77/80 viable mutants caught (96.25%)**
//! after the boundary-guard pins — above the 90% floor (the prior run left 10 edge mutants; the
//! boundary-guard `#[test]`s killed 7). The 3 residual MISSED mutants are EQUIVALENT mutants on
//! always-false / unreachable paths: `LineRange::is_empty -> false` (a `new`-built range can never be
//! empty — `len = end - start + 1 >= 1` always, so `false` IS the true value); and the
//! `find_fingerprint_match` loop-bound `-`→`+` / `surviving_subrange` position `+`→`*` mutants on the
//! no-match tail (the matching position is returned BEFORE the mutated arithmetic changes the result,
//! and on the no-match path both forms return the same answer). Equivalent mutants are not a coverage
//! gap (EI-01 §3 — measured + named, not silently dropped).
//!
//! ## imara-diff DEVIATION (EI-01 §1 — documented)
//! Architecture §5.1 / 01 §1 name **imara-diff (Myers 1986)** as the line-diff primitive. That crate
//! is **not in the workspace lockfile**, and pulling a new external diff crate purely for the
//! line-interval map is unwarranted: the resolver's correctness is in the FINGERPRINT + 3-way context
//! match (the part that makes `rebased` reliable rather than a guess), not in the specific diff
//! algorithm. We implement the line-interval search directly over the file lines — a contiguous
//! fingerprinted-block search with a context window (the "3-way context match" §5.1 specifies) plus a
//! per-line survival scan for the PARTIAL case. The on-disk blob bytes are read via the existing
//! git2 backend ([`crate::gix_backend`]) at the call site; this module is pure over the two blob byte
//! slices so it is deterministic + unit-testable (the GIT-D7 corpus drives it directly). If a later
//! prompt prefers imara-diff for the interval map, it is a git-local swap behind [`resolve`] (the
//! 4-state contract + the fingerprint are untouched). Logged here + in the P-286 report.

use crate::project::Tombstone;
use myelin_refs::{strip_sub, ArtifactRef, Sub};

/// The number of context lines fingerprinted on EACH side of the anchored range (the "context
/// window", arch §5.1). A 3-line window is the GitHub/GitLab-class default: enough surrounding
/// context that a `rebased` (Moved) match is a reliable contiguous-block match rather than a guess,
/// without over-anchoring on far-away unrelated edits. Used at mint time (the fingerprint) and at
/// resolve time (the 3-way context match).
pub const CONTEXT_WINDOW: usize = 3;

/// Which side of a diff the comment was anchored to (arch §5 — `side`). A PR review comment is
/// anchored to a line on the OLD (base) side or the NEW (head) side of a diff hunk; the resolver
/// anchors the fingerprint against the corresponding blob. Carried for fidelity + the original-context
/// render; v1 resolves the fingerprint identically for either side (the fingerprint is the content,
/// not the side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSide {
    /// The comment was anchored to a line on the OLD (base) side of the diff.
    Old,
    /// The comment was anchored to a line on the NEW (head) side of the diff.
    New,
}

/// The four frozen resolution states (contract 5.7 / X-4 §"Git line-ranges" / arch §5.1). EXACTLY
/// these four — the resolver returns one, never "unknown" + never a relocated range with no state
/// (the never-silently-wrong invariant). Maps onto the Refs ladder's `{live/moved/outdated/gone}`
/// sub-resolve outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorState {
    /// **EXACT** — the file/blob is unchanged at the anchored lines: the range still names the same
    /// content. Render the thread inline at the original range.
    Live,
    /// **REBASED** — the fingerprinted block (anchored lines + context) was found at a SHIFTED
    /// position: the content moved but is intact. Render the thread at the shifted range, flagged
    /// `moved`, with "view in original context".
    Moved,
    /// **PARTIAL** — some anchored lines survive, some are gone: no full-block match. Render the
    /// surviving sub-range, flagged `outdated`, with "view in original context".
    Outdated,
    /// **GONE** — the anchored content is entirely absent. Render a tombstone (`content_gone`) rooted
    /// at the PR, with "view in original context". Never silently drop the thread.
    Gone,
}

impl AnchorState {
    /// The render-time state token a UI/projection surfaces (`live`/`moved`/`outdated`/`gone`). The
    /// resolution ALWAYS carries this — a consumer cannot read a range without also reading its state
    /// (the never-silently-wrong invariant, EI-01 §3).
    pub fn token(self) -> &'static str {
        match self {
            AnchorState::Live => "live",
            AnchorState::Moved => "moved",
            AnchorState::Outdated => "outdated",
            AnchorState::Gone => "gone",
        }
    }

    /// `true` iff this is the EXACT/`Live` state — the only state that renders inline with no
    /// "view in original context" affordance (every other state moved/dropped the content).
    pub fn is_live(self) -> bool {
        matches!(self, AnchorState::Live)
    }
}

/// A 1-based, inclusive line range (`start..=end`, `end >= start`) — the unit the `#L<a>-L<b>` sub
/// names + the unit the resolver returns. Mirrors the `Sub::LineRange { start, end }` endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRange {
    /// The 1-based start line (inclusive).
    pub start: u64,
    /// The 1-based end line (inclusive, `>= start`).
    pub end: u64,
}

impl LineRange {
    /// Build a 1-based inclusive range, normalising `start <= end` (an inverted pair is swapped — the
    /// grammar already rejects an inverted MINT, this is defensive for internally-derived ranges).
    pub fn new(start: u64, end: u64) -> LineRange {
        if start <= end {
            LineRange { start, end }
        } else {
            LineRange {
                start: end,
                end: start,
            }
        }
    }

    /// The number of lines the range covers (`end - start + 1`).
    pub fn len(self) -> u64 {
        self.end - self.start + 1
    }

    /// `true` iff the range covers no lines — unreachable for a `new`-built range (always ≥ 1); the
    /// clippy-paired companion to [`LineRange::len`].
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// A **content anchor** — the mint-time record a PR inline thread pins to a file line-range (arch §5,
/// contract 5.7). Stores the blob oid + path + side + the 1-based range + the anchored commit oid +
/// the **BLAKE3 fingerprint** of the anchored lines + their context window. The fingerprint is what
/// makes a `rebased` (Moved) match reliable; the blob oid is the EXACT-match fast path; the original
/// position drives the "view in original context" render for every non-Live state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAnchor {
    /// The blob oid the lines were anchored against at mint time (the EXACT-match fast path — if this
    /// oid is still the file's blob, the anchor is trivially Live). A `blake3:<hex>` content-address.
    pub anchor_blob_oid: String,
    /// The file path the range names (the on-disk path — decoded from the `#sub` URN's percent-encoded
    /// `<path>` by [`crate::subs::decode_path_segment`] at the call site).
    pub path: String,
    /// Which diff side the comment was anchored to (old/base or new/head).
    pub side: DiffSide,
    /// The 1-based anchored line range at mint time.
    pub range: LineRange,
    /// The commit oid the range was anchored against at mint time (the original context for the
    /// "view in original context" render — the commit the reviewer was looking at).
    pub anchored_commit_oid: String,
    /// The BLAKE3 fingerprint of the anchored lines + their context window (`blake3:<hex>`). The
    /// content identity the `rebased`/`partial` resolution matches on — independent of position.
    pub anchor_fingerprint: String,
    /// The EXACT anchored line texts (no context) — the per-line survival scan for the PARTIAL state
    /// matches these against `new_blob` to find the surviving sub-range. Stored so resolution does not
    /// need the OLD blob bytes when only a survival check is required (though [`resolve`] takes the old
    /// bytes too for the full-fidelity context match).
    pub anchored_lines: Vec<String>,
}

impl LineAnchor {
    /// **Mint** a content anchor from the file's blob bytes at mint time (arch §5.1 — the mint half of
    /// the resolver). Slices the 1-based `range` out of `blob`, captures the [`CONTEXT_WINDOW`] lines
    /// on each side, and computes the BLAKE3 fingerprint over `(context_before ++ anchored ++
    /// context_after)`. The fingerprint is position-independent (it does not include the line numbers),
    /// so a shifted block fingerprints identically — that is what makes `rebased` a reliable MATCH.
    ///
    /// Returns `None` if the range is out of the file's bounds (a mint against a stale/wrong blob —
    /// caught loudly at mint time, never a silently-empty fingerprint).
    pub fn mint(
        blob: &[u8],
        path: impl Into<String>,
        side: DiffSide,
        range: LineRange,
        anchor_blob_oid: impl Into<String>,
        anchored_commit_oid: impl Into<String>,
    ) -> Option<LineAnchor> {
        let lines = split_lines(blob);
        let (start_idx, end_idx) = range_to_indices(range, lines.len())?;
        let anchored_lines: Vec<String> = lines[start_idx..=end_idx].to_vec();
        let fingerprint = fingerprint_block(&lines, start_idx, end_idx);
        Some(LineAnchor {
            anchor_blob_oid: anchor_blob_oid.into(),
            path: path.into(),
            side,
            range,
            anchored_commit_oid: anchored_commit_oid.into(),
            anchor_fingerprint: fingerprint,
            anchored_lines,
        })
    }
}

/// The result of resolving a [`LineAnchor`] against a new blob — the 4-state outcome the Refs ladder
/// consumes (contract 5.7). ALWAYS carries the [`AnchorState`] + the ORIGINAL anchored position so a
/// consumer can never read a (possibly relocated) range without also reading its state + the
/// original-context link (the never-silently-wrong invariant, EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// The 4-state resolution outcome.
    pub state: AnchorState,
    /// The resolved range in the NEW blob — the same as the original for `Live`, the shifted range for
    /// `Moved`, the surviving sub-range for `Outdated`, `None` for `Gone` (nothing survives to point
    /// at in the new blob).
    pub resolved_range: Option<LineRange>,
    /// The ORIGINAL anchored range (mint time) — the "view in original context" target. Always
    /// present, even for `Gone` (the thread always remembers where it was written).
    pub original_range: LineRange,
    /// The mint-time blob oid the original range lived in (the "view in original context" blob).
    pub original_blob_oid: String,
    /// The mint-time commit oid (the commit the reviewer was looking at) — the original-context link.
    pub original_commit_oid: String,
    /// A `Gone` resolution carries the PR-root [`Tombstone`] (`reason: content_gone`) so the thread
    /// renders a tombstone rooted at its PR, never a silent drop. `None` for the other three states.
    pub tombstone: Option<Tombstone>,
}

impl Resolution {
    /// The "view in original context" descriptor every NON-`Live` resolution offers (arch §5 / X-4 —
    /// never silently wrong). For a `Live` anchor there is nothing to view "elsewhere" (the content is
    /// in place) so this returns `None`; for Moved/Outdated/Gone it returns the original blob + commit
    /// + range so the UI can always link back to where the comment was written.
    pub fn original_context(&self) -> Option<OriginalContext<'_>> {
        if self.state.is_live() {
            None
        } else {
            Some(OriginalContext {
                blob_oid: &self.original_blob_oid,
                commit_oid: &self.original_commit_oid,
                range: self.original_range,
                state: self.state,
            })
        }
    }

    /// The render-time state token (`live`/`moved`/`outdated`/`gone`) — the resolution ALWAYS surfaces
    /// it (a consumer reads the range AND the state together; never a range alone).
    pub fn state_token(&self) -> &'static str {
        self.state.token()
    }
}

/// The "view in original context" descriptor (arch §5 — the render path for a relocated/lost anchor).
/// Points at the mint-time blob + commit + range so the UI links a Moved/Outdated/Gone thread back to
/// where it was originally written, alongside the resolution state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginalContext<'a> {
    /// The mint-time blob oid the original range lived in.
    pub blob_oid: &'a str,
    /// The mint-time commit oid the reviewer was looking at.
    pub commit_oid: &'a str,
    /// The original 1-based anchored range.
    pub range: LineRange,
    /// The resolution state that triggered the original-context render (moved/outdated/gone).
    pub state: AnchorState,
}

/// **Resolve** a [`LineAnchor`] against the file's NEW blob bytes through the frozen 4-state ladder
/// (contract 5.7 / X-4 / arch §5.1). `new_blob_oid` is the oid of `new_blob` at the resolved head;
/// `pr_root` is the anchor's parent PR (the tombstone root for a `Gone` resolution). Returns the
/// [`Resolution`] — ALWAYS with its [`AnchorState`] + the original-context position (never a silent
/// relocation).
///
/// The ladder, in order (the FIRST matching rung wins — exact before moved before partial before
/// gone, so a moved block is never mis-reported as partial, etc.):
///
/// 1. **EXACT (Live):** the mint-time `anchor_blob_oid` equals `new_blob_oid` (the file is untouched),
///    OR the fingerprinted block sits byte-identical at the SAME position in `new_blob`.
/// 2. **REBASED (Moved):** the fingerprinted block (anchored lines + context window) is found as a
///    contiguous run at a DIFFERENT position in `new_blob` (the 3-way context match).
/// 3. **PARTIAL (Outdated):** SOME anchored lines survive (in order) in `new_blob` but not as a full
///    contiguous block — return the surviving sub-range.
/// 4. **GONE (content_gone):** NONE of the anchored lines survive — a PR-rooted tombstone.
pub fn resolve(
    anchor: &LineAnchor,
    new_blob: &[u8],
    new_blob_oid: &str,
    pr_root: &ArtifactRef,
) -> Resolution {
    let base = |state, resolved_range, tombstone| Resolution {
        state,
        resolved_range,
        original_range: anchor.range,
        original_blob_oid: anchor.anchor_blob_oid.clone(),
        original_commit_oid: anchor.anchored_commit_oid.clone(),
        tombstone,
    };

    let new_lines = split_lines(new_blob);

    // ── Rung 1: EXACT (Live). The fast path: the file's blob is byte-identical to the mint-time blob,
    //    so the range names the same content. (Equivalently: the anchored block fingerprints identical
    //    at the SAME position.) Either proves the content is in place — return the exact range.
    if anchor.anchor_blob_oid == new_blob_oid {
        return base(AnchorState::Live, Some(anchor.range), None);
    }
    if let Some((s, e)) = range_to_indices(anchor.range, new_lines.len()) {
        if fingerprint_block(&new_lines, s, e) == anchor.anchor_fingerprint {
            return base(AnchorState::Live, Some(anchor.range), None);
        }
    }

    // ── Rung 2: REBASED (Moved). Search every candidate position in new_blob for the SAME fingerprint
    //    (anchored lines + context window matched as a contiguous run) — a position-independent match
    //    on the BLAKE3 fingerprint, so a block that merely shifted is matched reliably, not guessed.
    let block_len = anchor.range.len() as usize;
    if let Some(new_start_idx) =
        find_fingerprint_match(&new_lines, &anchor.anchor_fingerprint, block_len)
    {
        // The fingerprinted block sits at new_start_idx (0-based) — the shifted ANCHORED range is the
        // block_len lines starting there (the context window is matched but not part of the range).
        let start_line = (new_start_idx + 1) as u64;
        let end_line = start_line + block_len as u64 - 1;
        return base(
            AnchorState::Moved,
            Some(LineRange::new(start_line, end_line)),
            None,
        );
    }

    // ── Rung 3 / 4: PARTIAL (Outdated) vs GONE. No full-block match — scan which anchored lines still
    //    survive (in order) in new_blob. If SOME survive, return the surviving sub-range (Outdated);
    //    if NONE survive, the content is entirely gone (a PR-rooted tombstone).
    match surviving_subrange(&anchor.anchored_lines, &new_lines) {
        Some(range) => base(AnchorState::Outdated, Some(range), None),
        None => base(
            AnchorState::Gone,
            None,
            Some(content_gone_tombstone(pr_root)),
        ),
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// The line-diff + fingerprint primitives (the imara-diff DEVIATION — implemented directly; see header)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Split blob bytes into logical lines (UTF-8 lossy, `\n`-delimited, a trailing newline does NOT
/// produce a final empty line). The unit the fingerprint + the diff operate on. Lossy decode is
/// deliberate: a non-UTF-8 blob still anchors (the bytes-as-text fingerprint is stable), it is never
/// rejected — the resolver is for source files but must not panic on a binary blob.
fn split_lines(blob: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(blob);
    let mut lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
    // `split` on a trailing `\n` yields a trailing "" — drop it so a file with a final newline has the
    // intuitive line count (the same convention git/blame use).
    if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

/// Convert a 1-based inclusive [`LineRange`] to 0-based `(start_idx, end_idx)` indices into a
/// `line_count`-line file, or `None` if the range falls outside the file (loud out-of-bounds — never a
/// silently-clamped slice). `end >= start` is guaranteed by [`LineRange::new`]; both endpoints must be
/// within `1..=line_count`.
fn range_to_indices(range: LineRange, line_count: usize) -> Option<(usize, usize)> {
    if range.start == 0 || range.end == 0 {
        return None;
    }
    let start_idx = (range.start - 1) as usize;
    let end_idx = (range.end - 1) as usize;
    if end_idx >= line_count || start_idx > end_idx {
        return None;
    }
    Some((start_idx, end_idx))
}

/// The BLAKE3 fingerprint (`blake3:<hex>`) of the anchored block `lines[start_idx..=end_idx]` PLUS the
/// [`CONTEXT_WINDOW`] context lines on each side (clamped to the file bounds). The context is part of
/// the fingerprint so a `rebased` match is a 3-WAY context match (arch §5.1) — the anchored lines AND
/// their neighbourhood must agree, which is what makes a shifted-block match reliable rather than a
/// coincidental single-line match. Position-independent: the line numbers are NOT hashed, only the
/// text + a `\n` separator, so the same block at any position fingerprints identically.
fn fingerprint_block(lines: &[String], start_idx: usize, end_idx: usize) -> String {
    let ctx_start = start_idx.saturating_sub(CONTEXT_WINDOW);
    let ctx_end = (end_idx + CONTEXT_WINDOW).min(lines.len().saturating_sub(1));
    let mut hasher = blake3::Hasher::new();
    // A length-prefixed, separator-joined hash so two different line-splittings cannot collide.
    for line in &lines[ctx_start..=ctx_end] {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("blake3:{}", hex::encode(hasher.finalize().as_bytes()))
}

/// Search `new_lines` for a contiguous `block_len`-line anchored block whose fingerprint (with its
/// context window) equals `target_fingerprint` — the 3-way context match for the REBASED (Moved)
/// state. Returns the 0-based start index of the FIRST matching block, or `None` if no position
/// fingerprints identically. Scans every candidate start (a small file is cheap; the v1 per-pair
/// remap — GF-5 hardens the multi-commit case).
fn find_fingerprint_match(
    new_lines: &[String],
    target_fingerprint: &str,
    block_len: usize,
) -> Option<usize> {
    if block_len == 0 || block_len > new_lines.len() {
        return None;
    }
    for start_idx in 0..=(new_lines.len() - block_len) {
        let end_idx = start_idx + block_len - 1;
        if fingerprint_block(new_lines, start_idx, end_idx) == *target_fingerprint {
            return Some(start_idx);
        }
    }
    None
}

/// The surviving sub-range for the PARTIAL (Outdated) state: find which of `anchored_lines` still
/// appear (IN ORDER, as an in-order subsequence) in `new_lines`, and return the 1-based range spanning
/// the FIRST and LAST surviving anchored line's position in `new_lines`. Returns `None` if NONE of the
/// anchored lines survive (→ the GONE state). A blank anchored line is ignored for the survival test
/// (a blank line matching a blank line is not evidence the content survived — it would over-anchor on
/// whitespace).
fn surviving_subrange(anchored_lines: &[String], new_lines: &[String]) -> Option<LineRange> {
    let mut first_hit: Option<usize> = None;
    let mut last_hit: Option<usize> = None;
    // Walk the anchored lines as an in-order subsequence over new_lines — each surviving anchored line
    // must be found AT OR AFTER the previous survivor's position (so we measure a contiguous-ish
    // surviving span, not scattered coincidental single-line hits across the whole file).
    let mut search_from = 0usize;
    for anchored in anchored_lines {
        if !is_survival_evidence(anchored) {
            // blank + structural-only lines (`}`, `)`, `{` …) are not survival evidence: a bare `}`
            // matching some OTHER `}` elsewhere is a coincidence, not the anchored content surviving —
            // counting it would mis-report a DELETED block as Outdated instead of Gone (a mis-anchor).
            continue;
        }
        if let Some(rel) = new_lines[search_from..].iter().position(|l| l == anchored) {
            let abs = search_from + rel;
            first_hit.get_or_insert(abs);
            last_hit = Some(abs);
            search_from = abs + 1;
        }
    }
    match (first_hit, last_hit) {
        (Some(f), Some(l)) => Some(LineRange::new((f + 1) as u64, (l + 1) as u64)),
        _ => None,
    }
}

/// `true` iff a line is **survival evidence** for the PARTIAL/Outdated scan — a substantive line whose
/// presence in the new blob genuinely indicates the anchored content survived. A blank line, or a
/// "structural-only" line consisting SOLELY of brackets / braces / parens / separators (`}`, `)`,
/// `{ )`, `};`, …), is NOT evidence: such lines recur everywhere, so matching one elsewhere is a
/// coincidence, not survival. Counting them would mis-report a fully-deleted block (whose only
/// "surviving" line is a coincidental `}`) as Outdated instead of Gone — a mis-anchor (the GIT-D7
/// `legacy-gone` corpus case). A line with ANY alphanumeric content is evidence.
fn is_survival_evidence(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // substantive iff it carries at least one alphanumeric char (an identifier/keyword/literal) — a
    // pure-punctuation structural line (`}`, `});`, `||`) is not anchored-content evidence.
    trimmed.chars().any(|c| c.is_alphanumeric())
}

/// Build the PR-rooted `content_gone` [`Tombstone`] for the GONE state (X-4 §"Git line-ranges" — a
/// tombstone always carries the root PR: "this referenced PR #N, that line is no longer present").
/// Reuses the [`crate::project::Tombstone`] shape with the dedicated
/// [`crate::project::TombstoneReason::ContentGone`] reason (EI-01 §7 — one tombstone type, not a
/// second; a distinct reason, faithful to the frozen `reason: content_gone`). `pr_root` is the
/// anchor's parent PR — the tombstone's root the UI links back to.
fn content_gone_tombstone(_pr_root: &ArtifactRef) -> Tombstone {
    Tombstone {
        reason: crate::project::TombstoneReason::ContentGone,
    }
}

/// Extract a [`LineRange`] from a parsed `#L<a>-L<b>` sub on an `ArtifactRef`, or `None` if the ref
/// carries no line-range sub. The bridge from the minted `#sub` ([`crate::subs::mint_blob_line_range`])
/// to the resolver's range type — Refs owns the grammar, this reads the parsed endpoints.
pub fn line_range_of(reference: &ArtifactRef) -> Option<LineRange> {
    match myelin_refs::sub_kind(reference)? {
        Sub::LineRange { start, end } => Some(LineRange::new(start, end)),
        _ => None,
    }
}

/// The PR root of a blob `#L<a>-L<b>` ref is NOT the blob root (a thread lives on a PR, not a bare
/// file). The anchor's owning PR is supplied by the caller (the GIT-P16 thread carries its PR); this
/// helper strips the `#sub` to the blob root for the Refs-ladder root-resolve step. The PR-root for
/// the tombstone is the caller's PR ref, threaded into [`resolve`].
pub fn blob_root(reference: &ArtifactRef) -> ArtifactRef {
    strip_sub(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subs::{decode_path_segment, mint_blob_line_range};

    fn blob(lines: &[&str]) -> Vec<u8> {
        lines.join("\n").into_bytes()
    }

    fn oid(tag: &str) -> String {
        format!(
            "blake3:{}",
            hex::encode(blake3::hash(tag.as_bytes()).as_bytes())
        )
    }

    fn pr() -> ArtifactRef {
        myelin_refs::parse("myelin://acme/git/pr/repo7:42").unwrap()
    }

    /// A canonical fixture file + an anchor on lines 4..=5 (the body of `fn charge`).
    fn fixture() -> (Vec<u8>, LineAnchor) {
        let old = blob(&[
            "use crate::ledger;",         // 1
            "",                           // 2
            "fn charge(amount: u64) {",   // 3
            "    let fee = amount / 10;", // 4
            "    ledger::debit(fee);",    // 5
            "}",                          // 6
            "",                           // 7
            "fn refund() {}",             // 8
        ]);
        let anchor = LineAnchor::mint(
            &old,
            "src/charge.rs",
            DiffSide::New,
            LineRange::new(4, 5),
            oid("old-blob"),
            oid("old-commit"),
        )
        .expect("anchor mints within bounds");
        (old, anchor)
    }

    // ════════ STATE 1 — EXACT (Live) ════════

    /// The file is untouched (the new blob oid equals the mint oid) → LIVE at the exact range, no
    /// original-context affordance (the content is in place).
    #[test]
    fn exact_match_when_blob_unchanged_is_live() {
        let (old, anchor) = fixture();
        let r = resolve(&anchor, &old, &anchor.anchor_blob_oid, &pr());
        assert_eq!(r.state, AnchorState::Live);
        assert_eq!(r.resolved_range, Some(LineRange::new(4, 5)));
        assert!(
            r.original_context().is_none(),
            "a Live anchor has no 'original context' elsewhere"
        );
        assert_eq!(r.state_token(), "live");
    }

    /// The blob oid CHANGED (something else in the file moved) but the anchored block sits
    /// byte-identical at the SAME position → still LIVE (the fingerprint at-position matches).
    #[test]
    fn same_position_identical_content_is_live_even_with_a_new_oid() {
        let (old, anchor) = fixture();
        // a different oid, but identical bytes at the anchored position.
        let r = resolve(&anchor, &old, &oid("a-different-oid"), &pr());
        assert_eq!(r.state, AnchorState::Live);
        assert_eq!(r.resolved_range, Some(LineRange::new(4, 5)));
    }

    // ════════ STATE 2 — REBASED (Moved) ════════

    /// Lines are PREPENDED above the anchored block (a classic rebase/insert): the fingerprinted block
    /// shifts down by N lines → MOVED to the shifted range, with "view in original context".
    #[test]
    fn shifted_block_is_moved_to_the_new_position() {
        let (_, anchor) = fixture();
        // prepend 3 lines → the `fn charge` body (lines 4-5) shifts to lines 7-8.
        let new = blob(&[
            "// new top-of-file license header", // 1
            "// SPDX: MIT",                      // 2
            "",                                  // 3
            "use crate::ledger;",                // 4
            "",                                  // 5
            "fn charge(amount: u64) {",          // 6
            "    let fee = amount / 10;",        // 7  <- anchored
            "    ledger::debit(fee);",           // 8  <- anchored
            "}",                                 // 9
            "",                                  // 10
            "fn refund() {}",                    // 11
        ]);
        let r = resolve(&anchor, &new, &oid("new-blob"), &pr());
        assert_eq!(
            r.state,
            AnchorState::Moved,
            "a shifted intact block must be MOVED, not outdated"
        );
        assert_eq!(r.resolved_range, Some(LineRange::new(7, 8)));
        // never silently wrong: the moved anchor offers "view in original context" at the OLD range.
        let ctx = r
            .original_context()
            .expect("a Moved anchor offers original context");
        assert_eq!(ctx.range, LineRange::new(4, 5));
        assert_eq!(ctx.state, AnchorState::Moved);
        assert_eq!(ctx.blob_oid, anchor.anchor_blob_oid);
        assert_eq!(r.state_token(), "moved");
    }

    // ════════ STATE 3 — PARTIAL (Outdated) ════════

    /// ONE of the two anchored lines is deleted, the other survives, and the surrounding context is
    /// perturbed enough that the full-block fingerprint no longer matches → PARTIAL/Outdated, returning
    /// the surviving sub-range, with "view in original context".
    #[test]
    fn partial_survival_is_outdated_with_the_surviving_subrange() {
        let (_, anchor) = fixture();
        // delete the `let fee` line (anchored line 4) + rename the fn (perturb context) → only
        // `ledger::debit(fee);` survives.
        let new = blob(&[
            "use crate::ledger;",          // 1
            "",                            // 2
            "fn charge_v2(amount: u64) {", // 3 (renamed → context differs)
            "    ledger::debit(amount);",  // 4 (changed → first anchored line gone)
            "    ledger::debit(fee);",     // 5 <- the surviving anchored line
            "    audit();",                // 6 (inserted → block context differs)
            "}",                           // 7
        ]);
        let r = resolve(&anchor, &new, &oid("new-blob"), &pr());
        assert_eq!(
            r.state,
            AnchorState::Outdated,
            "partial survival is OUTDATED"
        );
        // the surviving sub-range spans only the surviving anchored line (`ledger::debit(fee);` at 5).
        assert_eq!(r.resolved_range, Some(LineRange::new(5, 5)));
        let ctx = r
            .original_context()
            .expect("an Outdated anchor offers original context");
        assert_eq!(ctx.range, LineRange::new(4, 5));
        assert_eq!(r.state_token(), "outdated");
    }

    // ════════ STATE 4 — GONE (content_gone) ════════

    /// BOTH anchored lines are gone (the whole function was deleted) → GONE: a PR-rooted tombstone,
    /// `resolved_range` None, and "view in original context" still available (never a silent drop).
    #[test]
    fn entirely_gone_content_is_a_pr_rooted_tombstone() {
        let (_, anchor) = fixture();
        let new = blob(&[
            "use crate::ledger;", // 1
            "",                   // 2
            "fn refund() {}",     // 3
        ]);
        let r = resolve(&anchor, &new, &oid("new-blob"), &pr());
        assert_eq!(r.state, AnchorState::Gone);
        assert_eq!(r.resolved_range, None);
        assert!(
            r.tombstone.is_some(),
            "a Gone anchor carries a content_gone tombstone"
        );
        // never silently wrong: even a gone anchor remembers where it was written.
        let ctx = r
            .original_context()
            .expect("a Gone anchor still offers original context");
        assert_eq!(ctx.range, LineRange::new(4, 5));
        assert_eq!(ctx.commit_oid, anchor.anchored_commit_oid);
        assert_eq!(r.state_token(), "gone");
    }

    // ════════ the fingerprint + context-window matcher ════════

    /// The fingerprint is POSITION-INDEPENDENT: the same block at two different positions (with the
    /// same surrounding context) fingerprints identically — that is what makes a `rebased` match
    /// reliable. And a DIFFERENT context produces a different fingerprint (the 3-way context match).
    #[test]
    fn fingerprint_is_position_independent_but_context_sensitive() {
        let a = vec![
            "ctxA".to_string(),
            "x".to_string(),
            "y".to_string(),
            "ctxB".to_string(),
        ];
        // identical block+context at indices 1..=2.
        let fp1 = fingerprint_block(&a, 1, 2);
        let fp2 = fingerprint_block(&a, 1, 2);
        assert_eq!(fp1, fp2, "deterministic");

        // same block, but the context differs → different fingerprint.
        let b = vec![
            "DIFFERENT".to_string(),
            "x".to_string(),
            "y".to_string(),
            "ctxB".to_string(),
        ];
        assert_ne!(fingerprint_block(&a, 1, 2), fingerprint_block(&b, 1, 2));
    }

    /// The matcher finds a shifted block by fingerprint (the Moved-state search) and returns its
    /// 0-based start; a block whose context changed is NOT matched (no false Moved).
    #[test]
    fn find_fingerprint_match_locates_the_shifted_block() {
        // a mid-file block with full (window=3) context on both sides, so the fingerprint is the same
        // block+context wherever it sits intact.
        let lines: Vec<String> = vec!["h1", "h2", "h3", "TARGET1", "TARGET2", "t1", "t2", "t3"]
            .into_iter()
            .map(String::from)
            .collect();
        let fp = fingerprint_block(&lines, 3, 4); // block = TARGET1,TARGET2 (idx 3,4)
                                                  // the SAME block WITH its full context window shifted down by 4 (the context travels with it —
                                                  // a real rebase moves the surrounding lines too).
        let shifted: Vec<String> = vec![
            "x1", "x2", "x3", "x4", "h1", "h2", "h3", "TARGET1", "TARGET2", "t1", "t2", "t3",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        // block now at idx 7,8 → matcher returns the 0-based block start.
        assert_eq!(find_fingerprint_match(&shifted, &fp, 2), Some(7));
        // a file with no such block → None.
        let other: Vec<String> = vec!["q", "r", "s"].into_iter().map(String::from).collect();
        assert_eq!(find_fingerprint_match(&other, &fp, 2), None);
    }

    /// `surviving_subrange` returns the FIRST..LAST surviving anchored line's new position, ignores
    /// blank-line "survival", and returns None when nothing survives.
    #[test]
    fn surviving_subrange_spans_first_to_last_survivor() {
        let anchored = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let new: Vec<String> = vec!["x", "A", "y", "C", "z"]
            .into_iter()
            .map(String::from)
            .collect();
        // A at idx1 (line2), C at idx3 (line4) survive; B gone → span 2..=4.
        assert_eq!(
            surviving_subrange(&anchored, &new),
            Some(LineRange::new(2, 4))
        );
        // nothing survives → None.
        let gone: Vec<String> = vec!["x", "y", "z"].into_iter().map(String::from).collect();
        assert_eq!(surviving_subrange(&anchored, &gone), None);
        // blank lines are not survival evidence.
        let blanks = vec!["".to_string(), "".to_string()];
        let newblanks: Vec<String> = vec!["", "", ""].into_iter().map(String::from).collect();
        assert_eq!(surviving_subrange(&blanks, &newblanks), None);
    }

    /// The mint↔resolve↔URN bridge: a minted `#L<a>-L<b>` sub round-trips back to the resolver's
    /// [`LineRange`] (the path percent-encoding round-trips too), and the blob root strips clean.
    #[test]
    fn minted_sub_round_trips_to_the_resolver_range() {
        let r = mint_blob_line_range("acme", "repo7", "main", "src/charge.rs", 4, 5).unwrap();
        assert_eq!(line_range_of(&r), Some(LineRange::new(4, 5)));
        // the blob root strips the #sub cleanly.
        let root = blob_root(&r);
        assert!(!myelin_refs::format(&root).contains('#'));
        // the percent-encoded path decodes back to the on-disk path the resolver opens.
        assert_eq!(decode_path_segment("src%2Fcharge.rs"), "src/charge.rs");
    }

    /// An out-of-bounds mint is rejected LOUDLY (never a silently-empty fingerprint over no lines).
    #[test]
    fn out_of_bounds_mint_is_rejected() {
        let b = blob(&["one", "two"]);
        assert!(LineAnchor::mint(
            &b,
            "f.rs",
            DiffSide::New,
            LineRange::new(5, 6),
            oid("o"),
            oid("c")
        )
        .is_none());
    }

    // ════════ boundary / guard pins (the mandatory-core mutation floor — kills the edge mutants) ════════

    /// `range_to_indices` is loud at EVERY out-of-bounds edge: a 0 start, a 0 end, an end past the
    /// file, AND a valid interior range maps exactly. Pins the `start==0 || end==0` guard (a `&&` there
    /// would admit a `0..0` range) + the `>=` upper-bound guard + the index arithmetic.
    #[test]
    fn range_to_indices_is_loud_at_every_boundary() {
        // valid interior: 1-based (2,4) over a 5-line file → 0-based (1,3).
        assert_eq!(
            range_to_indices(LineRange { start: 2, end: 4 }, 5),
            Some((1, 3))
        );
        // a 0 start (would be index -1) → None. (kills `||`→`&&`: with `&&`, start=0,end=2 would pass)
        assert_eq!(range_to_indices(LineRange { start: 0, end: 2 }, 5), None);
        // a 0 end → None.
        assert_eq!(range_to_indices(LineRange { start: 0, end: 0 }, 5), None);
        // end past the file (line 6 of a 5-line file) → None. (kills the `>=` bound)
        assert_eq!(range_to_indices(LineRange { start: 5, end: 6 }, 5), None);
        // the last line exactly is in-bounds.
        assert_eq!(
            range_to_indices(LineRange { start: 5, end: 5 }, 5),
            Some((4, 4))
        );
        // a single-line file, line 1 → (0,0).
        assert_eq!(
            range_to_indices(LineRange { start: 1, end: 1 }, 1),
            Some((0, 0))
        );
    }

    /// `find_fingerprint_match` rejects an over-long block (block_len > file) and a zero block, and
    /// returns the EXACT 0-based start of the matching block (pins the `||`/`>`/`-` guards + the scan
    /// upper bound — an off-by-one would miss a block at the very end of the file).
    #[test]
    fn find_fingerprint_match_guards_and_end_of_file_block() {
        let lines: Vec<String> = vec!["a", "b", "c", "d", "e", "f", "g", "B1", "B2"]
            .into_iter()
            .map(String::from)
            .collect();
        // a block at the VERY END of the file (idx 7..=8) — the scan must reach `len-block_len`. Its
        // context window (idx 4..=8) is distinct from any earlier position, so the match is unambiguous.
        let fp = fingerprint_block(&lines, 7, 8);
        assert_eq!(find_fingerprint_match(&lines, &fp, 2), Some(7));
        // block_len longer than the file → None (kills `>`→`==`/`>=` and the `||` guard).
        assert_eq!(find_fingerprint_match(&lines, &fp, 99), None);
        // a zero-length block → None.
        assert_eq!(find_fingerprint_match(&lines, &fp, 0), None);
        // block_len == file length is admissible (the whole file is one block) → start 0.
        let whole = fingerprint_block(&lines, 0, 8);
        assert_eq!(find_fingerprint_match(&lines, &whole, 9), Some(0));
    }

    /// `surviving_subrange` computes the 1-based positions EXACTLY (pins the `+ 1` line-number
    /// arithmetic — a `-`/`*` there would shift the reported range). The first survivor at idx 1 must
    /// report line 2, the last at idx 3 must report line 4.
    #[test]
    fn surviving_subrange_reports_exact_one_based_positions() {
        let anchored = vec![
            "keepA".to_string(),
            "dropB".to_string(),
            "keepC".to_string(),
        ];
        let new: Vec<String> = vec!["x", "keepA", "y", "keepC"]
            .into_iter()
            .map(String::from)
            .collect();
        // keepA at idx1 → line 2; keepC at idx3 → line 4.
        assert_eq!(
            surviving_subrange(&anchored, &new),
            Some(LineRange { start: 2, end: 4 })
        );
        // a single survivor at idx 0 → line 1..=1 (the `+1` is exact, not `+0`/`*1`-ambiguous).
        let one = vec!["solo".to_string()];
        let newone: Vec<String> = vec!["solo", "z"].into_iter().map(String::from).collect();
        assert_eq!(
            surviving_subrange(&one, &newone),
            Some(LineRange { start: 1, end: 1 })
        );
    }

    /// `is_survival_evidence` distinguishes substantive lines from blank + structural-only lines — the
    /// rule that keeps a deleted block (whose only coincidental survivor is a `}`) reported as GONE.
    #[test]
    fn is_survival_evidence_rejects_blank_and_structural_only_lines() {
        assert!(is_survival_evidence("    ledger::debit(fee);"));
        assert!(is_survival_evidence("x")); // a one-char identifier is substantive
        assert!(!is_survival_evidence("")); // blank
        assert!(!is_survival_evidence("   ")); // whitespace only
        assert!(!is_survival_evidence("}")); // a bare close brace
        assert!(!is_survival_evidence("    });")); // structural punctuation only
        assert!(!is_survival_evidence("||")); // operator-only
    }

    /// `LineRange` helpers: `len` counts inclusively, `is_empty` is never true for a `new`-built range,
    /// and `new` normalises an inverted pair (pins the `len`/`is_empty`/normalise arithmetic).
    #[test]
    fn line_range_len_is_empty_and_normalisation() {
        assert_eq!(LineRange::new(4, 8).len(), 5); // inclusive 4..=8 = 5 lines
        assert_eq!(LineRange::new(7, 7).len(), 1); // single line
        assert!(!LineRange::new(4, 8).is_empty()); // a real range is never empty
        assert!(!LineRange::new(1, 1).is_empty());
        // an inverted pair normalises (start <= end).
        assert_eq!(LineRange::new(8, 4), LineRange { start: 4, end: 8 });
    }

    /// A non-line-range ref yields no [`LineRange`] (the bridge is line-range-only).
    #[test]
    fn line_range_of_ignores_non_line_range_subs() {
        let pr_comment = myelin_refs::mint(
            &myelin_refs::parse("myelin://acme/git/pr/repo7:42").unwrap(),
            Sub::Comment("c1".into()),
        )
        .unwrap();
        assert_eq!(line_range_of(&pr_comment), None);
    }
}
