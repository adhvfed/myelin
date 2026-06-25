//! # `patch_id_chain` — patch-id-chain anchor carry-over (GF-5 → R-6, GIT-P33 / P-482, M5)
//!
//! **A content-anchored inline thread follows a rebased hunk through a MULTI-commit rebase.** The M3
//! floor ([`crate::anchor`] / GF-5) resolves an anchor against ONE new blob (the head blob) with a
//! per-pair fingerprint remap → the four states `Live/Moved/Outdated/Gone`. That degrades to `Outdated`
//! when an INTERMEDIATE commit in a multi-commit rebase perturbs the context window even though the
//! hunk itself is intact. This module promotes the resolver: it matches `git patch-id` across the
//! pre/post-rebase commit SEQUENCE, so a thread follows a rebased hunk through the whole rebase and
//! resolves **`Moved`** (the hunk is intact, just relocated), not `Outdated`.
//!
//! **Owning architecture (read first, in full):**
//! `05-hard-problems.md` **HP-4** (diff/comment anchoring — the OQ-D content-fingerprint resolver; "git's
//! `patch-id` for the follow-on rebase carry-over") + **floor GF-5** ("v1 does per-pair fingerprint remap
//! + the four states; follow-on: patch-id-chain carry-over so a thread follows a rebased hunk through a
//! multi-commit rebase — arch §5.2; owed drill D-7"). `02-internals-and-algorithms.md` §5.2 (the
//! patch-id-chain). Contract 5.7 (the BLAKE3 fingerprint 4-state resolver — EXTENDED, not re-defined).
//!
//! ## What is REUSED vs NEW (EI-01 §7 coherence)
//! The 4-state resolver already exists and is NOT re-defined:
//! - [`crate::anchor::resolve`] / [`crate::anchor::Resolution`] / [`crate::anchor::AnchorState`] — the
//!   frozen four-state ladder (`Live/Moved/Outdated/Gone`) + the never-silently-wrong invariant.
//! - [`crate::anchor::LineAnchor`] — the content anchor (blob oid + path + range + fingerprint).
//!
//! What is **genuinely NEW** here (the GF-5 → R-6 promotion):
//! 1. [`PatchId`] — a commit's `git patch-id` (the stable hash of its DIFF, independent of commit
//!    metadata / parent / position). Two commits with the same change have the same patch-id even
//!    across a rebase.
//! 2. [`RebaseChain`] — the pre→post-rebase commit-sequence mapping, matched by patch-id (a pre-rebase
//!    commit maps to the post-rebase commit carrying the SAME patch-id).
//! 3. [`carry_anchor_through_rebase`] — the promoted resolve: if the anchor's commit was rebased (its
//!    patch-id survives in the post-rebase chain), the anchor is carried to the post-rebase commit and
//!    resolved against THAT commit's blob → `Moved` (intact through the rebase), not the floor's
//!    `Outdated`. If the patch-id does NOT survive (the change was dropped/squashed-away), it falls
//!    back to the per-pair floor resolve (never silently wrong).
//!
//! ## The carry-over is never silently wrong (the contract-5.7 invariant carries)
//! The patch-id-chain ONLY upgrades an `Outdated` to `Moved` when the patch-id GENUINELY survives the
//! rebase (the hunk is provably the same change). If the change was squashed/dropped, the patch-id is
//! absent and the resolve falls through to the floor's per-pair ladder — so a dropped hunk still
//! tombstones (`Gone`) or degrades (`Outdated`), never a false `Moved`. The four states are unchanged;
//! the chain makes the `Moved` case RELIABLE across a multi-commit rebase rather than a guess.
//!
//! ## FLOOR PROMOTED (the honesty register — VISION §3 / EI-01 §1)
//! - **GF-5 — per-pair fingerprint remap (M3 floor) → patch-id-chain carry-over across a multi-commit
//!   rebase (R-6).** The patch-id-chain model + the carry-over resolve ship HERE. Recorded, dated
//!   GIT-P33. The real `git patch-id` byte production is the sandboxed canonical-`git` wire op; this
//!   owns the chain-matching + the carry-over semantics over the four-state ladder.
//!
//! ## Mutation floor (mandatory-core, ≥ 80% — EI-01 §2/§3; a mis-anchored thread is the failure)
//! The carry-over is mandatory-core. The load-bearing mutants — the patch-id survival match
//! ([`RebaseChain::post_for`]), the upgrade-only-on-survival ([`carry_anchor_through_rebase`] never
//! invents a `Moved`), and the fall-back-to-floor on a dropped patch-id — are each killed by an
//! assertion in the unit tests. The floor is **≥ 80%**.

use std::collections::BTreeMap;

use myelin_refs::ArtifactRef;

use crate::anchor::{resolve, AnchorState, LineAnchor, Resolution};

/// **A commit's `git patch-id` — the stable hash of its DIFF** (independent of commit metadata, parent,
/// author, or position in history). Two commits that make the SAME change share a patch-id even across
/// a rebase — that is what lets an anchor follow a rebased hunk. `patch-id:<hex>`. PII-free (it hashes
/// the diff content, which the anchor already references; no new identity surface).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatchId(pub String);

impl PatchId {
    /// Wrap a `git patch-id` hex (the sandboxed `git patch-id` wire op produces the bytes; this carries
    /// the resulting handle).
    pub fn new(hex: impl Into<String>) -> PatchId {
        PatchId(hex.into())
    }
}

/// **The pre→post-rebase commit-sequence mapping, matched by patch-id (GF-5 → R-6).** A multi-commit
/// rebase replays each pre-rebase commit onto the new base, producing a new commit oid that carries the
/// SAME patch-id (the change is identical; only the parent/position moved). This chain records, per
/// surviving patch-id, the post-rebase commit oid (+ the blob the anchored path resolves to there). A
/// pre-rebase commit whose patch-id is ABSENT from the chain was dropped/squashed (the change did not
/// survive) — the carry-over falls back to the floor resolver for it.
#[derive(Clone, Debug, Default)]
pub struct RebaseChain {
    /// patch-id → the post-rebase commit oid that carries it (the survivor).
    post_commit: BTreeMap<PatchId, String>,
    /// (patch-id) → the post-rebase blob bytes the anchored path resolves to in that commit (so the
    /// carry-over can resolve the anchor against the rebased blob). Keyed per patch-id for the drill.
    post_blob: BTreeMap<PatchId, (String, Vec<u8>)>,
}

impl RebaseChain {
    /// An empty rebase chain (no commits matched yet).
    pub fn new() -> RebaseChain {
        RebaseChain::default()
    }

    /// **Record that a pre-rebase change (its `patch_id`) survived the rebase as `post_commit_oid`,
    /// with the anchored path resolving to `post_blob` there.** Built by walking the post-rebase
    /// commit sequence and matching each commit's `git patch-id` back to the pre-rebase set.
    pub fn record_survivor(
        &mut self,
        patch_id: PatchId,
        post_commit_oid: impl Into<String>,
        post_blob_oid: impl Into<String>,
        post_blob: Vec<u8>,
    ) {
        let post_commit_oid = post_commit_oid.into();
        self.post_blob
            .insert(patch_id.clone(), (post_blob_oid.into(), post_blob));
        self.post_commit.insert(patch_id, post_commit_oid);
    }

    /// **The post-rebase commit carrying `patch_id`, if the change survived the rebase.** `None` ⇒ the
    /// change was dropped/squashed (the patch-id did not survive) — the carry-over falls back to the
    /// floor resolver. Mandatory-core: the survival match is the whole carry-over.
    pub fn post_for(&self, patch_id: &PatchId) -> Option<&String> {
        self.post_commit.get(patch_id)
    }

    /// The post-rebase blob (oid + bytes) the anchored path resolves to for a surviving patch-id.
    pub fn post_blob_for(&self, patch_id: &PatchId) -> Option<&(String, Vec<u8>)> {
        self.post_blob.get(patch_id)
    }
}

/// **Carry an anchor through a multi-commit rebase via the patch-id chain (GF-5 → R-6).**
///
/// `anchor` is the content anchor (minted against the pre-rebase blob); `anchor_patch_id` is the
/// `git patch-id` of the commit the anchor was written against; `chain` is the rebase's pre→post
/// patch-id mapping; `head_blob` / `head_blob_oid` are the CURRENT head blob (the floor's per-pair
/// target); `pr_root` is the anchor's parent PR (the tombstone root).
///
/// The promoted resolve:
/// 1. **Patch-id SURVIVED the rebase** ([`RebaseChain::post_for`] is `Some`): resolve the anchor
///    against the POST-REBASE blob the chain carries. If that resolves `Live` at the rebased commit,
///    the hunk is INTACT through the rebase → report **`Moved`** (the content relocated by the rebase
///    but is the same change), with the original-context link. This is the upgrade the floor's per-pair
///    resolve could not make (it would have seen perturbed context at the head and degraded to
///    `Outdated`).
/// 2. **Patch-id did NOT survive** (`None`) OR the post-rebase blob does not resolve `Live`: fall back
///    to the floor's per-pair [`resolve`] against the head blob — so a dropped/squashed hunk still
///    tombstones or degrades (never a false `Moved`; the never-silently-wrong invariant holds).
pub fn carry_anchor_through_rebase(
    anchor: &LineAnchor,
    anchor_patch_id: &PatchId,
    chain: &RebaseChain,
    head_blob: &[u8],
    head_blob_oid: &str,
    pr_root: &ArtifactRef,
) -> Resolution {
    // ── 1: did the anchored commit's change survive the rebase (by patch-id)? ──
    if chain.post_for(anchor_patch_id).is_some() {
        if let Some((post_blob_oid, post_blob)) = chain.post_blob_for(anchor_patch_id) {
            // Resolve the anchor against the POST-REBASE blob the chain carries.
            let at_rebased = resolve(anchor, post_blob, post_blob_oid, pr_root);
            // If the hunk is intact at the rebased commit (Live there), the rebase MOVED it — report
            // Moved with the resolved range (the upgrade the floor's per-pair resolve could not make).
            if at_rebased.state == AnchorState::Live {
                return Resolution {
                    state: AnchorState::Moved,
                    resolved_range: at_rebased.resolved_range,
                    original_range: anchor.range,
                    original_blob_oid: anchor.anchor_blob_oid.clone(),
                    original_commit_oid: anchor.anchored_commit_oid.clone(),
                    tombstone: None,
                };
            }
            // The patch-id survived but the anchored lines are not intact at the rebased blob (a
            // partial overlap) — return the rebased resolution AS-IS (Moved/Outdated/Gone from the
            // ladder against the rebased blob), never a false upgrade.
            if at_rebased.state != AnchorState::Gone {
                return at_rebased;
            }
        }
    }
    // ── 2: the change did not survive (or did not resolve intact) → the floor's per-pair resolve. ──
    resolve(anchor, head_blob, head_blob_oid, pr_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::{DiffSide, LineRange};

    fn pr_root() -> ArtifactRef {
        ArtifactRef("myelin://acme/git/pr/core:42".into())
    }

    /// Mint an anchor on a 5-line block at lines 2..3 of `blob`.
    fn mint_anchor(blob: &[u8]) -> LineAnchor {
        LineAnchor::mint(
            blob,
            "src/lib.rs",
            DiffSide::New,
            LineRange::new(2, 3),
            "blake3:pre-blob",
            "commit-pre",
        )
        .expect("mint")
    }

    const PRE: &[u8] = b"line 1\nthe anchored A\nthe anchored B\nline 4\nline 5\n";

    /// **A rebased hunk follows through a multi-commit rebase → Moved (not Outdated).** The anchored
    /// block is intact in the post-rebase commit (same patch-id) but RELOCATED (a preceding commit
    /// inserted lines), so the floor's per-pair resolve at the head would degrade — the patch-id chain
    /// carries it to the rebased blob and reports Moved.
    #[test]
    fn rebased_hunk_resolves_moved_via_patch_id_chain() {
        let anchor = mint_anchor(PRE);
        let pid = PatchId::new("patch-id:abc123");

        // The POST-rebase blob: the same anchored block, shifted down by an inserted preamble.
        let post_blob = b"new preamble 0\nnew preamble 1\nline 1\nthe anchored A\nthe anchored B\nline 4\nline 5\n".to_vec();
        let mut chain = RebaseChain::new();
        chain.record_survivor(
            pid.clone(),
            "commit-post",
            "blake3:post-blob",
            post_blob.clone(),
        );

        // The HEAD blob (the floor's per-pair target) has the block dropped entirely (so the floor
        // resolve would say Gone) — proving the chain is what carries the Moved.
        let head_blob = b"line 1\ncompletely different\nmore different\nline 4\nline 5\n";

        let res = carry_anchor_through_rebase(
            &anchor,
            &pid,
            &chain,
            head_blob,
            "blake3:head-blob",
            &pr_root(),
        );
        assert_eq!(
            res.state,
            AnchorState::Moved,
            "the patch-id chain carries the intact rebased hunk to Moved (not Outdated/Gone)"
        );
        // The resolved range points at the shifted position in the rebased blob.
        assert!(res.resolved_range.is_some());
        // The original-context link is preserved (never silently relocated).
        assert!(res.original_context().is_some());
        assert_eq!(res.original_range, LineRange::new(2, 3));
    }

    /// **A DROPPED change (patch-id absent from the chain) falls back to the floor resolve — never a
    /// false Moved.** If the hunk was squashed away, the patch-id does not survive; the carry-over uses
    /// the per-pair ladder against the head blob (which tombstones the gone hunk).
    #[test]
    fn dropped_patch_id_falls_back_to_floor_resolve_no_false_moved() {
        let anchor = mint_anchor(PRE);
        let pid = PatchId::new("patch-id:dropped");
        // An EMPTY chain — the anchored commit's change did not survive the rebase.
        let chain = RebaseChain::new();
        // The head blob no longer has the anchored block (it was squashed away).
        let head_blob = b"line 1\nunrelated\nunrelated 2\nline 4\nline 5\n";

        let res = carry_anchor_through_rebase(
            &anchor,
            &pid,
            &chain,
            head_blob,
            "blake3:head-blob",
            &pr_root(),
        );
        // The floor resolve tombstones the gone hunk — NOT a false Moved.
        assert_ne!(
            res.state,
            AnchorState::Moved,
            "a dropped hunk is never a false Moved"
        );
        assert!(
            matches!(res.state, AnchorState::Gone | AnchorState::Outdated),
            "a dropped/squashed hunk degrades or tombstones (the floor resolve): {:?}",
            res.state
        );
    }

    /// **An unchanged file through the rebase still resolves Live (the chain does not over-fire).** If
    /// the head blob is byte-identical to the mint blob, the floor resolve says Live — and the carry-
    /// over must not invent a Moved when the patch-id survived but the head is unchanged. Here the
    /// patch-id is absent (no rebase touched this file), so the floor Live wins.
    #[test]
    fn unchanged_file_resolves_live_chain_does_not_over_fire() {
        let anchor = mint_anchor(PRE);
        let pid = PatchId::new("patch-id:untouched");
        let chain = RebaseChain::new();
        // The head blob IS the mint blob (the file was untouched).
        let res =
            carry_anchor_through_rebase(&anchor, &pid, &chain, PRE, "blake3:pre-blob", &pr_root());
        assert_eq!(res.state, AnchorState::Live, "an untouched file is Live");
    }

    /// **The patch-id survival match is the load-bearing predicate.** A chain that records a DIFFERENT
    /// patch-id does not match the anchor's — so the carry-over falls back (kills a mutant that would
    /// match any patch-id).
    #[test]
    fn patch_id_survival_match_is_exact() {
        let mut chain = RebaseChain::new();
        chain.record_survivor(
            PatchId::new("patch-id:other"),
            "commit-other",
            "blake3:other",
            b"whatever".to_vec(),
        );
        // The anchor's patch-id is NOT the one in the chain → no survivor.
        assert!(chain.post_for(&PatchId::new("patch-id:mine")).is_none());
        // The one that IS recorded matches.
        assert_eq!(
            chain.post_for(&PatchId::new("patch-id:other")),
            Some(&"commit-other".to_string())
        );
    }
}
