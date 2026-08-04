use std::collections::BTreeMap;

use myelin_refs::ArtifactRef;

use crate::anchor::{resolve, AnchorState, LineAnchor, Resolution};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatchId(pub String);

impl PatchId {
    pub fn new(hex: impl Into<String>) -> PatchId {
        PatchId(hex.into())
    }
}

#[derive(Clone, Debug, Default)]
pub struct RebaseChain {
    post_commit: BTreeMap<PatchId, String>,
    post_blob: BTreeMap<PatchId, (String, Vec<u8>)>,
}

impl RebaseChain {
    pub fn new() -> RebaseChain {
        RebaseChain::default()
    }

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

    pub fn post_for(&self, patch_id: &PatchId) -> Option<&String> {
        self.post_commit.get(patch_id)
    }

    pub fn post_blob_for(&self, patch_id: &PatchId) -> Option<&(String, Vec<u8>)> {
        self.post_blob.get(patch_id)
    }
}

pub fn carry_anchor_through_rebase(
    anchor: &LineAnchor,
    anchor_patch_id: &PatchId,
    chain: &RebaseChain,
    head_blob: &[u8],
    head_blob_oid: &str,
    pr_root: &ArtifactRef,
) -> Resolution {
    if chain.post_for(anchor_patch_id).is_some() {
        if let Some((post_blob_oid, post_blob)) = chain.post_blob_for(anchor_patch_id) {
            let at_rebased = resolve(anchor, post_blob, post_blob_oid, pr_root);
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
            if at_rebased.state != AnchorState::Gone {
                return at_rebased;
            }
        }
    }
    resolve(anchor, head_blob, head_blob_oid, pr_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::{DiffSide, LineRange};

    fn pr_root() -> ArtifactRef {
        ArtifactRef("myelin://acme/git/pr/core:42".into())
    }

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

    #[test]
    fn rebased_hunk_resolves_moved_via_patch_id_chain() {
        let anchor = mint_anchor(PRE);
        let pid = PatchId::new("patch-id:abc123");

        let post_blob = b"new preamble 0\nnew preamble 1\nline 1\nthe anchored A\nthe anchored B\nline 4\nline 5\n".to_vec();
        let mut chain = RebaseChain::new();
        chain.record_survivor(
            pid.clone(),
            "commit-post",
            "blake3:post-blob",
            post_blob.clone(),
        );

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
        assert!(res.resolved_range.is_some());
        assert!(res.original_context().is_some());
        assert_eq!(res.original_range, LineRange::new(2, 3));
    }

    #[test]
    fn dropped_patch_id_falls_back_to_floor_resolve_no_false_moved() {
        let anchor = mint_anchor(PRE);
        let pid = PatchId::new("patch-id:dropped");
        let chain = RebaseChain::new();
        let head_blob = b"line 1\nunrelated\nunrelated 2\nline 4\nline 5\n";

        let res = carry_anchor_through_rebase(
            &anchor,
            &pid,
            &chain,
            head_blob,
            "blake3:head-blob",
            &pr_root(),
        );
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

    #[test]
    fn unchanged_file_resolves_live_chain_does_not_over_fire() {
        let anchor = mint_anchor(PRE);
        let pid = PatchId::new("patch-id:untouched");
        let chain = RebaseChain::new();
        let res =
            carry_anchor_through_rebase(&anchor, &pid, &chain, PRE, "blake3:pre-blob", &pr_root());
        assert_eq!(res.state, AnchorState::Live, "an untouched file is Live");
    }

    #[test]
    fn patch_id_survival_match_is_exact() {
        let mut chain = RebaseChain::new();
        chain.record_survivor(
            PatchId::new("patch-id:other"),
            "commit-other",
            "blake3:other",
            b"whatever".to_vec(),
        );
        assert!(chain.post_for(&PatchId::new("patch-id:mine")).is_none());
        assert_eq!(
            chain.post_for(&PatchId::new("patch-id:other")),
            Some(&"commit-other".to_string())
        );
    }
}
