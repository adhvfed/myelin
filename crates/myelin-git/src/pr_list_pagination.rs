use base64::Engine as _;

use crate::coordinate::{RepositorySlug, MAX_REPOSITORY_SLUG_BYTES};
use crate::pr_store::{PrListBucket, PrListSort, PrListState, PR_LIST_PAGE_MAX};

pub const PR_LIST_CURSOR_PREFIX: &str = "pl1_";
pub const PR_LIST_CURSOR_MAX_BYTES: usize = 512;
pub const PR_LIST_CURSOR_MAX_REPO_SLUG_BYTES: usize = MAX_REPOSITORY_SLUG_BYTES;

const VERSION: u8 = 1;
const FIXED_BYTES: usize = 94;
const FLAG_UPDATED_AT_PRESENT: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrListCursorEndpoint {
    Repository(PrListState),
    CrossRepository(PrListBucket),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrListDirection {
    Older,
    Newer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrListKey {
    pub updated_at: Option<i64>,
    pub number: u64,
    pub repo_slug: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrListCursor {
    endpoint: PrListCursorEndpoint,
    direction: PrListDirection,
    sort: PrListSort,
    limit: usize,
    display_offset: u32,
    key: PrListKey,
    static_scope: [u8; 32],
    visible_scope: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrListPage {
    Initial,
    LegacyOffset(usize),
    Keyset(PrListCursor),
}

impl PrListPage {
    pub fn display_offset(&self) -> usize {
        match self {
            Self::Initial => 0,
            Self::LegacyOffset(offset) => *offset,
            Self::Keyset(cursor) => cursor.display_offset() as usize,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrListCursorError;

impl std::fmt::Display for PrListCursorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("pull-request list cursor is malformed")
    }
}

impl std::error::Error for PrListCursorError {}

impl PrListCursor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: PrListCursorEndpoint,
        direction: PrListDirection,
        sort: PrListSort,
        limit: usize,
        display_offset: u32,
        key: PrListKey,
        static_scope: [u8; 32],
        visible_scope: [u8; 32],
    ) -> Result<Self, PrListCursorError> {
        if !(1..=PR_LIST_PAGE_MAX).contains(&limit)
            || key.number == 0
            || key.number > i64::MAX as u64
        {
            return Err(PrListCursorError);
        }
        match endpoint {
            PrListCursorEndpoint::Repository(_) => {
                if key.repo_slug.is_some() || visible_scope != [0; 32] {
                    return Err(PrListCursorError);
                }
            }
            PrListCursorEndpoint::CrossRepository(_) => {
                let slug = key.repo_slug.as_deref().ok_or(PrListCursorError)?;
                if slug.len() > PR_LIST_CURSOR_MAX_REPO_SLUG_BYTES
                    || RepositorySlug::parse(slug).is_err()
                {
                    return Err(PrListCursorError);
                }
            }
        }
        if sort == PrListSort::Created && key.updated_at.is_some() {
            return Err(PrListCursorError);
        }
        Ok(Self {
            endpoint,
            direction,
            sort,
            limit,
            display_offset,
            key,
            static_scope,
            visible_scope,
        })
    }

    pub fn parse(value: &str) -> Result<Self, PrListCursorError> {
        let encoded = value
            .strip_prefix(PR_LIST_CURSOR_PREFIX)
            .ok_or(PrListCursorError)?;
        if encoded.is_empty() || value.len() > PR_LIST_CURSOR_MAX_BYTES {
            return Err(PrListCursorError);
        }
        let frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| PrListCursorError)?;
        if frame.len() < FIXED_BYTES
            || frame[0] != VERSION
            || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&frame) != encoded
        {
            return Err(PrListCursorError);
        }
        let endpoint = decode_endpoint(frame[1], frame[3])?;
        let direction = match frame[2] {
            0 => PrListDirection::Older,
            1 => PrListDirection::Newer,
            _ => return Err(PrListCursorError),
        };
        let sort = match frame[4] {
            0 => PrListSort::Updated,
            1 => PrListSort::Created,
            _ => return Err(PrListCursorError),
        };
        let flags = frame[5];
        if flags & !FLAG_UPDATED_AT_PRESENT != 0 {
            return Err(PrListCursorError);
        }
        let limit = usize::from(u16::from_be_bytes([frame[6], frame[7]]));
        let display_offset =
            u32::from_be_bytes(frame[8..12].try_into().map_err(|_| PrListCursorError)?);
        let raw_updated_at =
            i64::from_be_bytes(frame[12..20].try_into().map_err(|_| PrListCursorError)?);
        let updated_at = match flags & FLAG_UPDATED_AT_PRESENT {
            0 if raw_updated_at == 0 => None,
            FLAG_UPDATED_AT_PRESENT => Some(raw_updated_at),
            _ => return Err(PrListCursorError),
        };
        let number = u64::from_be_bytes(frame[20..28].try_into().map_err(|_| PrListCursorError)?);
        let mut static_scope = [0; 32];
        static_scope.copy_from_slice(&frame[28..60]);
        let mut visible_scope = [0; 32];
        visible_scope.copy_from_slice(&frame[60..92]);
        let slug_len = usize::from(u16::from_be_bytes([frame[92], frame[93]]));
        if frame.len() != FIXED_BYTES.saturating_add(slug_len) {
            return Err(PrListCursorError);
        }
        let repo_slug = if slug_len == 0 {
            None
        } else {
            Some(
                std::str::from_utf8(&frame[FIXED_BYTES..])
                    .map_err(|_| PrListCursorError)?
                    .to_string(),
            )
        };
        Self::new(
            endpoint,
            direction,
            sort,
            limit,
            display_offset,
            PrListKey {
                updated_at,
                number,
                repo_slug,
            },
            static_scope,
            visible_scope,
        )
    }

    pub fn encode(&self) -> String {
        let slug = self.key.repo_slug.as_deref().unwrap_or("").as_bytes();
        let mut frame = Vec::with_capacity(FIXED_BYTES + slug.len());
        frame.push(VERSION);
        frame.push(match self.endpoint {
            PrListCursorEndpoint::Repository(_) => 0,
            PrListCursorEndpoint::CrossRepository(_) => 1,
        });
        frame.push(match self.direction {
            PrListDirection::Older => 0,
            PrListDirection::Newer => 1,
        });
        frame.push(encode_filter(self.endpoint));
        frame.push(match self.sort {
            PrListSort::Updated => 0,
            PrListSort::Created => 1,
        });
        frame.push(u8::from(self.key.updated_at.is_some()));
        frame.extend_from_slice(&(self.limit as u16).to_be_bytes());
        frame.extend_from_slice(&self.display_offset.to_be_bytes());
        frame.extend_from_slice(&self.key.updated_at.unwrap_or(0).to_be_bytes());
        frame.extend_from_slice(&self.key.number.to_be_bytes());
        frame.extend_from_slice(&self.static_scope);
        frame.extend_from_slice(&self.visible_scope);
        frame.extend_from_slice(&(slug.len() as u16).to_be_bytes());
        frame.extend_from_slice(slug);
        debug_assert_eq!(frame.len(), FIXED_BYTES + slug.len());
        format!(
            "{PR_LIST_CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    pub fn endpoint(&self) -> PrListCursorEndpoint {
        self.endpoint
    }
    pub fn direction(&self) -> PrListDirection {
        self.direction
    }
    pub fn sort(&self) -> PrListSort {
        self.sort
    }
    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn display_offset(&self) -> u32 {
        self.display_offset
    }
    pub fn key(&self) -> &PrListKey {
        &self.key
    }
    pub fn static_scope(&self) -> [u8; 32] {
        self.static_scope
    }
    pub fn visible_scope(&self) -> [u8; 32] {
        self.visible_scope
    }
}

fn encode_filter(endpoint: PrListCursorEndpoint) -> u8 {
    match endpoint {
        PrListCursorEndpoint::Repository(PrListState::Open) => 0,
        PrListCursorEndpoint::Repository(PrListState::Merged) => 1,
        PrListCursorEndpoint::Repository(PrListState::Closed) => 2,
        PrListCursorEndpoint::Repository(PrListState::All) => 3,
        PrListCursorEndpoint::CrossRepository(PrListBucket::Yours) => 4,
        PrListCursorEndpoint::CrossRepository(PrListBucket::NeedsReview) => 5,
    }
}

fn decode_endpoint(endpoint: u8, filter: u8) -> Result<PrListCursorEndpoint, PrListCursorError> {
    match (endpoint, filter) {
        (0, 0) => Ok(PrListCursorEndpoint::Repository(PrListState::Open)),
        (0, 1) => Ok(PrListCursorEndpoint::Repository(PrListState::Merged)),
        (0, 2) => Ok(PrListCursorEndpoint::Repository(PrListState::Closed)),
        (0, 3) => Ok(PrListCursorEndpoint::Repository(PrListState::All)),
        (1, 4) => Ok(PrListCursorEndpoint::CrossRepository(PrListBucket::Yours)),
        (1, 5) => Ok(PrListCursorEndpoint::CrossRepository(
            PrListBucket::NeedsReview,
        )),
        _ => Err(PrListCursorError),
    }
}

fn hash_field(hash: &mut blake3::Hasher, value: &[u8]) {
    hash.update(&(value.len() as u64).to_be_bytes());
    hash.update(value);
}

pub fn pr_list_static_scope(
    tenant: &str,
    region: &str,
    viewer: &str,
    endpoint: PrListCursorEndpoint,
    repo: Option<&str>,
    sort: PrListSort,
    limit: usize,
) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.git.pr-list-cursor.static-scope.v1\0");
    for field in [tenant, region, viewer, repo.unwrap_or("")] {
        hash_field(&mut hash, field.as_bytes());
    }
    hash.update(&[match endpoint {
        PrListCursorEndpoint::Repository(_) => 0,
        PrListCursorEndpoint::CrossRepository(_) => 1,
    }]);
    hash.update(&[encode_filter(endpoint)]);
    hash.update(&[match sort {
        PrListSort::Updated => 0,
        PrListSort::Created => 1,
    }]);
    hash.update(&(limit as u16).to_be_bytes());
    *hash.finalize().as_bytes()
}

pub fn pr_list_visible_scope(slugs: &[String]) -> [u8; 32] {
    let mut slugs = slugs.to_vec();
    slugs.sort();
    slugs.dedup();
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.git.pr-list-cursor.visible-scope.v1\0");
    hash.update(&(slugs.len() as u64).to_be_bytes());
    for slug in slugs {
        hash_field(&mut hash, slug.as_bytes());
    }
    *hash.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(endpoint: PrListCursorEndpoint, sort: PrListSort) -> PrListCursor {
        PrListCursor::new(
            endpoint,
            PrListDirection::Older,
            sort,
            25,
            u32::MAX,
            PrListKey {
                updated_at: (sort == PrListSort::Updated).then_some(-7),
                number: 42,
                repo_slug: matches!(endpoint, PrListCursorEndpoint::CrossRepository(_))
                    .then(|| "team/core".to_string()),
            },
            [3; 32],
            if matches!(endpoint, PrListCursorEndpoint::CrossRepository(_)) {
                [4; 32]
            } else {
                [0; 32]
            },
        )
        .unwrap()
    }

    #[test]
    fn canonical_round_trip_covers_endpoints_sorts_null_and_direction() {
        for endpoint in [
            PrListCursorEndpoint::Repository(PrListState::All),
            PrListCursorEndpoint::CrossRepository(PrListBucket::NeedsReview),
        ] {
            for sort in [PrListSort::Updated, PrListSort::Created] {
                let mut value = cursor(endpoint, sort);
                value.direction = PrListDirection::Newer;
                if sort == PrListSort::Updated {
                    value.key.updated_at = None;
                }
                let encoded = value.encode();
                assert!(encoded.starts_with(PR_LIST_CURSOR_PREFIX));
                assert_eq!(PrListCursor::parse(&encoded).unwrap(), value);
            }
        }
    }

    #[test]
    fn malformed_and_noncanonical_frames_are_rejected() {
        let valid = cursor(
            PrListCursorEndpoint::CrossRepository(PrListBucket::Yours),
            PrListSort::Updated,
        )
        .encode();
        for invalid in [
            "",
            "1",
            "pl1_",
            "pl1_@@",
            &format!("{valid}="),
            &format!("pl1_{}", "a".repeat(513)),
        ] {
            assert!(
                PrListCursor::parse(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        let encoded = valid.strip_prefix(PR_LIST_CURSOR_PREFIX).unwrap();
        let mut frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        for (index, byte) in [(0, 2), (2, 7), (3, 3), (4, 9), (5, 2)] {
            let old = frame[index];
            frame[index] = byte;
            let bad = format!(
                "{PR_LIST_CURSOR_PREFIX}{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&frame)
            );
            assert!(PrListCursor::parse(&bad).is_err());
            frame[index] = old;
        }
    }

    #[test]
    fn scopes_bind_query_viewer_and_sorted_visibility() {
        let endpoint = PrListCursorEndpoint::Repository(PrListState::Open);
        let base = pr_list_static_scope(
            "t",
            "r",
            "viewer",
            endpoint,
            Some("core"),
            PrListSort::Updated,
            20,
        );
        assert_ne!(
            base,
            pr_list_static_scope(
                "other",
                "r",
                "viewer",
                endpoint,
                Some("core"),
                PrListSort::Updated,
                20
            )
        );
        assert_ne!(
            base,
            pr_list_static_scope(
                "t",
                "r",
                "other",
                endpoint,
                Some("core"),
                PrListSort::Updated,
                20
            )
        );
        assert_ne!(
            base,
            pr_list_static_scope(
                "t",
                "r",
                "viewer",
                endpoint,
                Some("other"),
                PrListSort::Updated,
                20
            )
        );
        assert_eq!(
            pr_list_visible_scope(&["b".into(), "a".into()]),
            pr_list_visible_scope(&["a".into(), "b".into()])
        );
        assert_ne!(
            pr_list_visible_scope(&["a".into()]),
            pr_list_visible_scope(&["a".into(), "b".into()])
        );
    }
}
