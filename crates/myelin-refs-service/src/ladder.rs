use myelin_events::ArtifactRef;
use myelin_refs::{strip_sub, sub_kind, Sub};

use crate::resolve::{OwnerProjection, ProjectOutcome, ProjectionFlag};

pub const TOMBSTONE_COUNT_SIGNAL: &str = "refs.tombstone_count";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubState {
    Live(OwnerProjection),
    Moved(OwnerProjection),
    Outdated(OwnerProjection),
    Gone,
    Erased,
}

impl SubState {
    pub fn into_outcome(self) -> ProjectOutcome {
        match self {
            SubState::Live(p) => ProjectOutcome::Live(OwnerProjection { flag: None, ..p }),
            SubState::Moved(p) => ProjectOutcome::Live(OwnerProjection {
                flag: Some(ProjectionFlag::Moved),
                ..p
            }),
            SubState::Outdated(p) => ProjectOutcome::Live(OwnerProjection {
                flag: Some(ProjectionFlag::Outdated),
                ..p
            }),
            SubState::Gone => ProjectOutcome::SubGone,
            SubState::Erased => ProjectOutcome::Erased,
        }
    }
}

pub trait SubAnchorResolver: Send + Sync {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LineRangeState {
    Exact,
    Rebased {
        new_start: u64,
        new_end: u64,
    },
    Partial {
        surviving_start: u64,
        surviving_end: u64,
    },
    ContentGone,
}

impl LineRangeState {
    pub fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            LineRangeState::Exact => SubState::Live(projection),
            LineRangeState::Rebased { .. } => SubState::Moved(projection),
            LineRangeState::Partial { .. } => SubState::Outdated(projection),
            LineRangeState::ContentGone => SubState::Gone,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintedLineRange {
    pub blob_oid: String,
    pub anchored: Vec<String>,
}

impl MintedLineRange {
    pub fn fingerprint(line: &str) -> String {
        format!(
            "blake3:{}",
            hex::encode(blake3::hash(line.as_bytes()).as_bytes())
        )
    }

    pub fn mint(blob_oid: &str, lines: &[&str], start: u64, end: u64) -> MintedLineRange {
        let anchored = lines
            .iter()
            .skip(start.saturating_sub(1) as usize)
            .take((end.saturating_sub(start) + 1) as usize)
            .map(|l| Self::fingerprint(l))
            .collect();
        MintedLineRange {
            blob_oid: blob_oid.to_string(),
            anchored,
        }
    }
}

pub fn resolve_line_range(
    minted: &MintedLineRange,
    current_oid: &str,
    current_lines: &[&str],
) -> LineRangeState {
    if minted.blob_oid == current_oid {
        return LineRangeState::Exact;
    }
    if minted.anchored.is_empty() {
        return LineRangeState::ContentGone;
    }

    let current_fps: Vec<String> = current_lines
        .iter()
        .map(|l| MintedLineRange::fingerprint(l))
        .collect();

    if let Some(offset) = find_subsequence(&current_fps, &minted.anchored) {
        let new_start = (offset + 1) as u64;
        let new_end = (offset + minted.anchored.len()) as u64;
        return LineRangeState::Rebased { new_start, new_end };
    }

    for keep in (1..minted.anchored.len()).rev() {
        if let Some(offset) = find_subsequence(&current_fps, &minted.anchored[..keep]) {
            let surviving_start = (offset + 1) as u64;
            let surviving_end = (offset + keep) as u64;
            return LineRangeState::Partial {
                surviving_start,
                surviving_end,
            };
        }
    }

    LineRangeState::ContentGone
}

fn find_subsequence(haystack: &[String], needle: &[String]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .find(|&start| haystack[start..start + needle.len()] == *needle)
}

pub fn resolve_sub_outcome(resolver: &dyn SubAnchorResolver, ref_: &ArtifactRef) -> ProjectOutcome {
    let sub = sub_kind(ref_);
    resolver.resolve_sub(ref_, sub.as_ref()).into_outcome()
}

pub fn ladder_root(ref_: &ArtifactRef) -> ArtifactRef {
    strip_sub(ref_)
}
