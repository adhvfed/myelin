use crate::project::Tombstone;
use myelin_refs::{strip_sub, ArtifactRef, Sub};

pub const CONTEXT_WINDOW: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSide {
    Old,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorState {
    Live,
    Moved,
    Outdated,
    Gone,
}

impl AnchorState {
    pub fn token(self) -> &'static str {
        match self {
            AnchorState::Live => "live",
            AnchorState::Moved => "moved",
            AnchorState::Outdated => "outdated",
            AnchorState::Gone => "gone",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, AnchorState::Live)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRange {
    pub start: u64,
    pub end: u64,
}

impl LineRange {
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

    pub fn len(self) -> u64 {
        self.end - self.start + 1
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAnchor {
    pub anchor_blob_oid: String,
    pub path: String,
    pub side: DiffSide,
    pub range: LineRange,
    pub anchored_commit_oid: String,
    pub anchor_fingerprint: String,
    pub anchored_lines: Vec<String>,
}

impl LineAnchor {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    pub state: AnchorState,
    pub resolved_range: Option<LineRange>,
    pub original_range: LineRange,
    pub original_blob_oid: String,
    pub original_commit_oid: String,
    pub tombstone: Option<Tombstone>,
}

impl Resolution {
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

    pub fn state_token(&self) -> &'static str {
        self.state.token()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginalContext<'a> {
    pub blob_oid: &'a str,
    pub commit_oid: &'a str,
    pub range: LineRange,
    pub state: AnchorState,
}

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

    if anchor.anchor_blob_oid == new_blob_oid {
        return base(AnchorState::Live, Some(anchor.range), None);
    }
    if let Some((s, e)) = range_to_indices(anchor.range, new_lines.len()) {
        if fingerprint_block(&new_lines, s, e) == anchor.anchor_fingerprint {
            return base(AnchorState::Live, Some(anchor.range), None);
        }
    }

    let block_len = anchor.range.len() as usize;
    if let Some(new_start_idx) =
        find_fingerprint_match(&new_lines, &anchor.anchor_fingerprint, block_len)
    {
        let start_line = (new_start_idx + 1) as u64;
        let end_line = start_line + block_len as u64 - 1;
        return base(
            AnchorState::Moved,
            Some(LineRange::new(start_line, end_line)),
            None,
        );
    }

    match surviving_subrange(&anchor.anchored_lines, &new_lines) {
        Some(range) => base(AnchorState::Outdated, Some(range), None),
        None => base(
            AnchorState::Gone,
            None,
            Some(content_gone_tombstone(pr_root)),
        ),
    }
}

fn split_lines(blob: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(blob);
    let mut lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
    if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

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

fn fingerprint_block(lines: &[String], start_idx: usize, end_idx: usize) -> String {
    let ctx_start = start_idx.saturating_sub(CONTEXT_WINDOW);
    let ctx_end = (end_idx + CONTEXT_WINDOW).min(lines.len().saturating_sub(1));
    let mut hasher = blake3::Hasher::new();
    for line in &lines[ctx_start..=ctx_end] {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    format!("blake3:{}", hex::encode(hasher.finalize().as_bytes()))
}

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

fn surviving_subrange(anchored_lines: &[String], new_lines: &[String]) -> Option<LineRange> {
    let mut first_hit: Option<usize> = None;
    let mut last_hit: Option<usize> = None;
    let mut search_from = 0usize;
    for anchored in anchored_lines {
        if !is_survival_evidence(anchored) {
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

fn is_survival_evidence(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.chars().any(|c| c.is_alphanumeric())
}

fn content_gone_tombstone(_pr_root: &ArtifactRef) -> Tombstone {
    Tombstone {
        reason: crate::project::TombstoneReason::ContentGone,
    }
}

pub fn line_range_of(reference: &ArtifactRef) -> Option<LineRange> {
    match myelin_refs::sub_kind(reference)? {
        Sub::LineRange { start, end } => Some(LineRange::new(start, end)),
        _ => None,
    }
}

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

    fn fixture() -> (Vec<u8>, LineAnchor) {
        let old = blob(&[
            "use crate::ledger;",
            "",
            "fn charge(amount: u64) {",
            "    let fee = amount / 10;",
            "    ledger::debit(fee);",
            "}",
            "",
            "fn refund() {}",
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

    #[test]
    fn same_position_identical_content_is_live_even_with_a_new_oid() {
        let (old, anchor) = fixture();
        let r = resolve(&anchor, &old, &oid("a-different-oid"), &pr());
        assert_eq!(r.state, AnchorState::Live);
        assert_eq!(r.resolved_range, Some(LineRange::new(4, 5)));
    }

    #[test]
    fn shifted_block_is_moved_to_the_new_position() {
        let (_, anchor) = fixture();
        let new = blob(&[
            "// new top-of-file license header",
            "// SPDX: MIT",
            "",
            "use crate::ledger;",
            "",
            "fn charge(amount: u64) {",
            "    let fee = amount / 10;",
            "    ledger::debit(fee);",
            "}",
            "",
            "fn refund() {}",
        ]);
        let r = resolve(&anchor, &new, &oid("new-blob"), &pr());
        assert_eq!(
            r.state,
            AnchorState::Moved,
            "a shifted intact block must be MOVED, not outdated"
        );
        assert_eq!(r.resolved_range, Some(LineRange::new(7, 8)));
        let ctx = r
            .original_context()
            .expect("a Moved anchor offers original context");
        assert_eq!(ctx.range, LineRange::new(4, 5));
        assert_eq!(ctx.state, AnchorState::Moved);
        assert_eq!(ctx.blob_oid, anchor.anchor_blob_oid);
        assert_eq!(r.state_token(), "moved");
    }

    #[test]
    fn partial_survival_is_outdated_with_the_surviving_subrange() {
        let (_, anchor) = fixture();
        let new = blob(&[
            "use crate::ledger;",
            "",
            "fn charge_v2(amount: u64) {",
            "    ledger::debit(amount);",
            "    ledger::debit(fee);",
            "    audit();",
            "}",
        ]);
        let r = resolve(&anchor, &new, &oid("new-blob"), &pr());
        assert_eq!(
            r.state,
            AnchorState::Outdated,
            "partial survival is OUTDATED"
        );
        assert_eq!(r.resolved_range, Some(LineRange::new(5, 5)));
        let ctx = r
            .original_context()
            .expect("an Outdated anchor offers original context");
        assert_eq!(ctx.range, LineRange::new(4, 5));
        assert_eq!(r.state_token(), "outdated");
    }

    #[test]
    fn entirely_gone_content_is_a_pr_rooted_tombstone() {
        let (_, anchor) = fixture();
        let new = blob(&[
            "use crate::ledger;",
            "",
            "fn refund() {}",
        ]);
        let r = resolve(&anchor, &new, &oid("new-blob"), &pr());
        assert_eq!(r.state, AnchorState::Gone);
        assert_eq!(r.resolved_range, None);
        assert!(
            r.tombstone.is_some(),
            "a Gone anchor carries a content_gone tombstone"
        );
        let ctx = r
            .original_context()
            .expect("a Gone anchor still offers original context");
        assert_eq!(ctx.range, LineRange::new(4, 5));
        assert_eq!(ctx.commit_oid, anchor.anchored_commit_oid);
        assert_eq!(r.state_token(), "gone");
    }

    #[test]
    fn fingerprint_is_position_independent_but_context_sensitive() {
        let a = vec![
            "ctxA".to_string(),
            "x".to_string(),
            "y".to_string(),
            "ctxB".to_string(),
        ];
        let fp1 = fingerprint_block(&a, 1, 2);
        let fp2 = fingerprint_block(&a, 1, 2);
        assert_eq!(fp1, fp2, "deterministic");

        let b = vec![
            "DIFFERENT".to_string(),
            "x".to_string(),
            "y".to_string(),
            "ctxB".to_string(),
        ];
        assert_ne!(fingerprint_block(&a, 1, 2), fingerprint_block(&b, 1, 2));
    }

    #[test]
    fn find_fingerprint_match_locates_the_shifted_block() {
        let lines: Vec<String> = vec!["h1", "h2", "h3", "TARGET1", "TARGET2", "t1", "t2", "t3"]
            .into_iter()
            .map(String::from)
            .collect();
        let fp = fingerprint_block(&lines, 3, 4);
        let shifted: Vec<String> = vec![
            "x1", "x2", "x3", "x4", "h1", "h2", "h3", "TARGET1", "TARGET2", "t1", "t2", "t3",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(find_fingerprint_match(&shifted, &fp, 2), Some(7));
        let other: Vec<String> = vec!["q", "r", "s"].into_iter().map(String::from).collect();
        assert_eq!(find_fingerprint_match(&other, &fp, 2), None);
    }

    #[test]
    fn surviving_subrange_spans_first_to_last_survivor() {
        let anchored = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let new: Vec<String> = vec!["x", "A", "y", "C", "z"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            surviving_subrange(&anchored, &new),
            Some(LineRange::new(2, 4))
        );
        let gone: Vec<String> = vec!["x", "y", "z"].into_iter().map(String::from).collect();
        assert_eq!(surviving_subrange(&anchored, &gone), None);
        let blanks = vec!["".to_string(), "".to_string()];
        let newblanks: Vec<String> = vec!["", "", ""].into_iter().map(String::from).collect();
        assert_eq!(surviving_subrange(&blanks, &newblanks), None);
    }

    #[test]
    fn minted_sub_round_trips_to_the_resolver_range() {
        let r = mint_blob_line_range("acme", "repo7", "main", "src/charge.rs", 4, 5).unwrap();
        assert_eq!(line_range_of(&r), Some(LineRange::new(4, 5)));
        let root = blob_root(&r);
        assert!(!myelin_refs::format(&root).contains('#'));
        assert_eq!(decode_path_segment("src%2Fcharge.rs"), "src/charge.rs");
    }

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

    #[test]
    fn range_to_indices_is_loud_at_every_boundary() {
        assert_eq!(
            range_to_indices(LineRange { start: 2, end: 4 }, 5),
            Some((1, 3))
        );
        assert_eq!(range_to_indices(LineRange { start: 0, end: 2 }, 5), None);
        assert_eq!(range_to_indices(LineRange { start: 0, end: 0 }, 5), None);
        assert_eq!(range_to_indices(LineRange { start: 5, end: 6 }, 5), None);
        assert_eq!(
            range_to_indices(LineRange { start: 5, end: 5 }, 5),
            Some((4, 4))
        );
        assert_eq!(
            range_to_indices(LineRange { start: 1, end: 1 }, 1),
            Some((0, 0))
        );
    }

    #[test]
    fn find_fingerprint_match_guards_and_end_of_file_block() {
        let lines: Vec<String> = vec!["a", "b", "c", "d", "e", "f", "g", "B1", "B2"]
            .into_iter()
            .map(String::from)
            .collect();
        let fp = fingerprint_block(&lines, 7, 8);
        assert_eq!(find_fingerprint_match(&lines, &fp, 2), Some(7));
        assert_eq!(find_fingerprint_match(&lines, &fp, 99), None);
        assert_eq!(find_fingerprint_match(&lines, &fp, 0), None);
        let whole = fingerprint_block(&lines, 0, 8);
        assert_eq!(find_fingerprint_match(&lines, &whole, 9), Some(0));
    }

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
        assert_eq!(
            surviving_subrange(&anchored, &new),
            Some(LineRange { start: 2, end: 4 })
        );
        let one = vec!["solo".to_string()];
        let newone: Vec<String> = vec!["solo", "z"].into_iter().map(String::from).collect();
        assert_eq!(
            surviving_subrange(&one, &newone),
            Some(LineRange { start: 1, end: 1 })
        );
    }

    #[test]
    fn is_survival_evidence_rejects_blank_and_structural_only_lines() {
        assert!(is_survival_evidence("    ledger::debit(fee);"));
        assert!(is_survival_evidence("x"));
        assert!(!is_survival_evidence(""));
        assert!(!is_survival_evidence("   "));
        assert!(!is_survival_evidence("}"));
        assert!(!is_survival_evidence("    });"));
        assert!(!is_survival_evidence("||"));
    }

    #[test]
    fn line_range_len_is_empty_and_normalisation() {
        assert_eq!(LineRange::new(4, 8).len(), 5);
        assert_eq!(LineRange::new(7, 7).len(), 1);
        assert!(!LineRange::new(4, 8).is_empty());
        assert!(!LineRange::new(1, 1).is_empty());
        assert_eq!(LineRange::new(8, 4), LineRange { start: 4, end: 8 });
    }

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
