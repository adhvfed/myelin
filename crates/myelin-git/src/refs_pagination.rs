use std::cmp::Ordering;

use base64::Engine as _;

use crate::core::Oid;
use crate::durable::{DurableError, DurableGitRepo, WIRE_MAX_REFS};

pub const REFS_PAGE_DEFAULT_LIMIT: usize = 100;
pub const REFS_PAGE_MAX_LIMIT: usize = 100;
pub const REFS_PAGE_MAX_QUERY_BYTES: usize = 256;
pub const WIRE_MAX_REF_NAME_BYTES: usize = 4 * 1024;
pub const WIRE_MAX_REF_NAMES_TOTAL_BYTES: usize = 32 * 1024 * 1024;

const CURSOR_PREFIX: &str = "gr1_";
const CURSOR_VERSION: u8 = 1;
const CURSOR_FIXED_BYTES: usize = 1 + 32 + 32 + 1 + 2;
const CURSOR_MAX_DECODED_BYTES: usize = CURSOR_FIXED_BYTES + WIRE_MAX_REF_NAME_BYTES;
const MAX_PINS: usize = 2;

#[derive(Clone, Copy)]
struct ScanLimits {
    refs: usize,
    one_name_bytes: usize,
    total_name_bytes: usize,
}

const WIRE_SCAN_LIMITS: ScanLimits = ScanLimits {
    refs: WIRE_MAX_REFS,
    one_name_bytes: WIRE_MAX_REF_NAME_BYTES,
    total_name_bytes: WIRE_MAX_REF_NAMES_TOTAL_BYTES,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Branch,
    Tag,
}

impl RefKind {
    fn byte(self) -> u8 {
        match self {
            Self::Branch => 0,
            Self::Tag => 1,
        }
    }

    fn from_byte(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Branch),
            1 => Some(Self::Tag),
            _ => None,
        }
    }

    pub fn from_qualified_name(full_name: &str) -> Option<(Self, &str)> {
        full_name
            .strip_prefix("refs/heads/")
            .map(|name| (Self::Branch, name))
            .or_else(|| {
                full_name
                    .strip_prefix("refs/tags/")
                    .map(|name| (Self::Tag, name))
            })
            .filter(|(_, name)| !name.is_empty())
    }

    fn full_name(self, short_name: &str) -> String {
        match self {
            Self::Branch => format!("refs/heads/{short_name}"),
            Self::Tag => format!("refs/tags/{short_name}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefPageItem {
    pub kind: RefKind,
    pub qualified_name: String,
    pub name: String,
    pub tip: Oid,
}

impl RefPageItem {
    fn key(&self) -> RefKey<'_> {
        RefKey {
            kind: self.kind,
            name: &self.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsSummary {
    pub branch_count: usize,
    pub tag_count: usize,
    pub head_symbolic_target: Option<String>,
    pub default_branch: String,
    pub default_tip: Option<Oid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogueRepoState {
    Populated,
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsPageRequest {
    pub limit: usize,
    pub query: Option<String>,
    pub current_ref: Option<String>,
    pub cursor: Option<String>,
}

impl Default for RefsPageRequest {
    fn default() -> Self {
        Self {
            limit: REFS_PAGE_DEFAULT_LIMIT,
            query: None,
            current_ref: None,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefsPage {
    pub summary: RefsSummary,
    pub items: Vec<RefPageItem>,
    pub pins: Vec<RefPageItem>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefsPageError {
    Durable(DurableError),
    InvalidLimit { supplied: usize },
    QueryTooLong { maximum: usize },
    InvalidCurrentRef,
    MalformedCursor,
    CursorScopeMismatch,
    CursorStale,
}

impl std::fmt::Display for RefsPageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Durable(error) => error.fmt(f),
            Self::InvalidLimit { supplied } => write!(
                f,
                "ref page limit {supplied} is outside 1..={REFS_PAGE_MAX_LIMIT}"
            ),
            Self::QueryTooLong { maximum } => {
                write!(f, "ref query exceeds the {maximum}-byte limit")
            }
            Self::InvalidCurrentRef => write!(f, "current ref must be fully qualified"),
            Self::MalformedCursor => write!(f, "malformed ref-page cursor"),
            Self::CursorScopeMismatch => write!(f, "ref-page cursor belongs to another scope"),
            Self::CursorStale => write!(f, "ref-page cursor is stale"),
        }
    }
}

impl std::error::Error for RefsPageError {}

impl From<DurableError> for RefsPageError {
    fn from(value: DurableError) -> Self {
        Self::Durable(value)
    }
}

#[derive(Clone, Copy)]
struct RefKey<'a> {
    kind: RefKind,
    name: &'a str,
}

impl PartialEq for RefKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.name.as_bytes() == other.name.as_bytes()
    }
}

impl Eq for RefKey<'_> {}

impl PartialOrd for RefKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RefKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kind
            .cmp(&other.kind)
            .then_with(|| self.name.as_bytes().cmp(other.name.as_bytes()))
    }
}

#[derive(Clone, Debug)]
struct Cursor {
    namespace: [u8; 32],
    scope: [u8; 32],
    kind: RefKind,
    name: String,
}

impl Cursor {
    fn encode(&self) -> String {
        let name = self.name.as_bytes();
        let mut frame = Vec::with_capacity(CURSOR_FIXED_BYTES + name.len());
        frame.push(CURSOR_VERSION);
        frame.extend_from_slice(&self.namespace);
        frame.extend_from_slice(&self.scope);
        frame.push(self.kind.byte());
        frame.extend_from_slice(&(name.len() as u16).to_be_bytes());
        frame.extend_from_slice(name);
        format!(
            "{CURSOR_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    fn parse(value: &str) -> Result<Self, RefsPageError> {
        let encoded = value
            .strip_prefix(CURSOR_PREFIX)
            .ok_or(RefsPageError::MalformedCursor)?;
        if encoded.is_empty() || encoded.len() > encoded_len(CURSOR_MAX_DECODED_BYTES) {
            return Err(RefsPageError::MalformedCursor);
        }
        let frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| RefsPageError::MalformedCursor)?;
        if frame.len() < CURSOR_FIXED_BYTES || frame.len() > CURSOR_MAX_DECODED_BYTES {
            return Err(RefsPageError::MalformedCursor);
        }
        if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&frame) != encoded {
            return Err(RefsPageError::MalformedCursor);
        }
        if frame[0] != CURSOR_VERSION {
            return Err(RefsPageError::MalformedCursor);
        }
        let mut namespace = [0_u8; 32];
        namespace.copy_from_slice(&frame[1..33]);
        let mut scope = [0_u8; 32];
        scope.copy_from_slice(&frame[33..65]);
        let kind = RefKind::from_byte(frame[65]).ok_or(RefsPageError::MalformedCursor)?;
        let name_len = usize::from(u16::from_be_bytes([frame[66], frame[67]]));
        if name_len == 0 || name_len > WIRE_MAX_REF_NAME_BYTES || frame.len() != 68 + name_len {
            return Err(RefsPageError::MalformedCursor);
        }
        let name = std::str::from_utf8(&frame[68..])
            .map_err(|_| RefsPageError::MalformedCursor)?
            .to_string();
        let full_name = kind.full_name(&name);
        if full_name.len() > WIRE_MAX_REF_NAME_BYTES
            || RefKind::from_qualified_name(&full_name).is_none()
            || !git2::Reference::is_valid_name(&full_name)
        {
            return Err(RefsPageError::MalformedCursor);
        }
        Ok(Self {
            namespace,
            scope,
            kind,
            name,
        })
    }
}

const fn encoded_len(decoded: usize) -> usize {
    decoded.div_ceil(3) * 4
}

#[derive(Default)]
struct NamespaceFingerprint {
    count: u64,
    xor: [u8; 32],
    sum: [u8; 32],
}

impl NamespaceFingerprint {
    fn add(&mut self, full_name: &str, oid: git2::Oid) {
        let mut entry = blake3::Hasher::new();
        entry.update(b"myelin.git.refs.namespace.entry.v1\0");
        entry.update(&(full_name.len() as u64).to_be_bytes());
        entry.update(full_name.as_bytes());
        entry.update(oid.as_bytes());
        let digest = *entry.finalize().as_bytes();
        self.count += 1;
        let mut carry = 0_u16;
        for (index, byte) in digest.into_iter().enumerate() {
            self.xor[index] ^= byte;
            let total = u16::from(self.sum[index]) + u16::from(byte) + carry;
            self.sum[index] = total as u8;
            carry = total >> 8;
        }
    }

    fn finish(&self, head_target: Option<&str>) -> [u8; 32] {
        let mut final_hash = blake3::Hasher::new();
        final_hash.update(b"myelin.git.refs.namespace.v1\0");
        final_hash.update(&self.count.to_be_bytes());
        final_hash.update(&self.xor);
        final_hash.update(&self.sum);
        if let Some(target) = head_target {
            final_hash.update(&[1]);
            final_hash.update(&(target.len() as u64).to_be_bytes());
            final_hash.update(target.as_bytes());
        } else {
            final_hash.update(&[0]);
        }
        *final_hash.finalize().as_bytes()
    }
}

struct ScanResult {
    summary: RefsSummary,
    rows: Vec<RefPageItem>,
    pins: Vec<RefPageItem>,
    namespace: [u8; 32],
    cursor_key_seen: bool,
}

impl DurableGitRepo {
    pub fn catalogue_repo_state(&self) -> Result<CatalogueRepoState, DurableError> {
        self.catalogue_repo_state_with_limits(WIRE_SCAN_LIMITS)
    }

    fn catalogue_repo_state_with_limits(
        &self,
        limits: ScanLimits,
    ) -> Result<CatalogueRepoState, DurableError> {
        let repo = self.open_git()?;
        if let Some(head_target) = read_head_target(&repo)? {
            if head_target.starts_with("refs/heads/") && direct_ref_has_target(&repo, &head_target)?
            {
                return Ok(CatalogueRepoState::Populated);
            }
        }
        if direct_ref_has_target(&repo, "refs/heads/main")? {
            return Ok(CatalogueRepoState::Populated);
        }

        let branches = repo
            .branches(Some(git2::BranchType::Local))
            .map_err(|error| DurableError::Git(format!("catalogue branches: {error}")))?;
        let mut total_name_bytes = 0_usize;
        for (scanned, branch) in branches.enumerate() {
            let (branch, _) = branch
                .map_err(|error| DurableError::Git(format!("catalogue branch iter: {error}")))?;
            if scanned == limits.refs {
                return Err(DurableError::Git(
                    "wire ref limit exceeded: catalogue branch count".into(),
                ));
            }
            let reference = branch.get();
            let full_name = reference.name().map_err(|_| {
                DurableError::Git("catalogue branch name is not valid UTF-8".into())
            })?;
            if full_name.len() > limits.one_name_bytes {
                return Err(DurableError::Git(
                    "wire ref limit exceeded: one ref name".into(),
                ));
            }
            total_name_bytes = total_name_bytes
                .checked_add(full_name.len())
                .ok_or_else(|| {
                    DurableError::Git("wire ref limit exceeded: ref name bytes".into())
                })?;
            if total_name_bytes > limits.total_name_bytes {
                return Err(DurableError::Git(
                    "wire ref limit exceeded: ref name bytes".into(),
                ));
            }
            if reference.target().is_some() {
                return Ok(CatalogueRepoState::Populated);
            }
        }
        Ok(CatalogueRepoState::Empty)
    }

    pub fn refs_summary(&self) -> Result<RefsSummary, DurableError> {
        let repo = self.open_git()?;
        let head_target = read_head_target(&repo)?;
        Ok(scan_refs(&repo, head_target, None, None, None, 0, WIRE_SCAN_LIMITS)?.summary)
    }

    pub fn default_branch_ref(&self) -> Result<String, DurableError> {
        let repo = self.open_git()?;
        let head_target = read_head_target(&repo)?;
        if let Some(target) = head_target.as_deref() {
            if target.starts_with("refs/heads/") && direct_ref_has_target(&repo, target)? {
                return Ok(target.to_string());
            }
        }
        if direct_ref_has_target(&repo, "refs/heads/main")? {
            return Ok("refs/heads/main".to_string());
        }
        let summary = scan_refs(&repo, head_target, None, None, None, 0, WIRE_SCAN_LIMITS)?.summary;
        Ok(format!("refs/heads/{}", summary.default_branch))
    }

    pub fn refs_page(&self, request: RefsPageRequest) -> Result<RefsPage, RefsPageError> {
        if request.limit == 0 || request.limit > REFS_PAGE_MAX_LIMIT {
            return Err(RefsPageError::InvalidLimit {
                supplied: request.limit,
            });
        }
        let query = normalize_query(request.query.as_deref())?;
        let current_ref = match request.current_ref.as_deref() {
            Some(value)
                if RefKind::from_qualified_name(value).is_some()
                    && value.len() <= WIRE_MAX_REF_NAME_BYTES
                    && git2::Reference::is_valid_name(value) =>
            {
                Some(value)
            }
            Some(_) => return Err(RefsPageError::InvalidCurrentRef),
            None => None,
        };
        let scope = self.refs_scope_hash(query.as_deref())?;
        let cursor = request.cursor.as_deref().map(Cursor::parse).transpose()?;
        if cursor.as_ref().is_some_and(|value| value.scope != scope) {
            return Err(RefsPageError::CursorScopeMismatch);
        }

        let repo = self.open_git()?;
        let head_target = read_head_target(&repo)?;
        let scan = scan_refs(
            &repo,
            head_target,
            query.as_deref(),
            current_ref,
            cursor.as_ref(),
            request.limit + 1,
            WIRE_SCAN_LIMITS,
        )?;
        if cursor
            .as_ref()
            .is_some_and(|value| value.namespace != scan.namespace)
        {
            return Err(RefsPageError::CursorStale);
        }
        if cursor.is_some() && !scan.cursor_key_seen {
            return Err(RefsPageError::MalformedCursor);
        }

        let mut items = scan.rows;
        let has_more = items.len() > request.limit;
        items.truncate(request.limit);
        let next_cursor = if has_more {
            items.last().map(|last| {
                Cursor {
                    namespace: scan.namespace,
                    scope,
                    kind: last.kind,
                    name: last.name.clone(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok(RefsPage {
            summary: scan.summary,
            items,
            pins: scan.pins,
            next_cursor,
        })
    }

    fn refs_scope_hash(&self, query: Option<&str>) -> Result<[u8; 32], DurableError> {
        let verified_path = std::fs::canonicalize(self.path()).map_err(|error| {
            DurableError::Io(format!(
                "canonicalize repository {} for ref cursor: {error}",
                self.path().display()
            ))
        })?;
        let path = verified_path.as_os_str().as_encoded_bytes();
        let mut hash = blake3::Hasher::new();
        hash.update(b"myelin.git.refs.scope.v1\0");
        hash.update(&(path.len() as u64).to_be_bytes());
        hash.update(path);
        match query {
            Some(value) => {
                hash.update(&[1]);
                hash.update(&(value.len() as u64).to_be_bytes());
                hash.update(value.as_bytes());
            }
            None => {
                hash.update(&[0]);
            }
        }
        Ok(*hash.finalize().as_bytes())
    }
}

fn direct_ref_has_target(repo: &git2::Repository, name: &str) -> Result<bool, DurableError> {
    match repo.find_reference(name) {
        Ok(reference) => Ok(reference.target().is_some()),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(error) => Err(DurableError::Git(format!("find catalogue branch: {error}"))),
    }
}

fn normalize_query(query: Option<&str>) -> Result<Option<String>, RefsPageError> {
    let Some(raw) = query else { return Ok(None) };
    if raw.len() > REFS_PAGE_MAX_QUERY_BYTES {
        return Err(RefsPageError::QueryTooLong {
            maximum: REFS_PAGE_MAX_QUERY_BYTES,
        });
    }
    let normalized = raw.trim().to_lowercase();
    if normalized.len() > REFS_PAGE_MAX_QUERY_BYTES {
        return Err(RefsPageError::QueryTooLong {
            maximum: REFS_PAGE_MAX_QUERY_BYTES,
        });
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}

fn read_head_target(repo: &git2::Repository) -> Result<Option<String>, DurableError> {
    match repo.find_reference("HEAD") {
        Ok(head) => head
            .symbolic_target()
            .map_err(|_| DurableError::Git("HEAD target is not valid UTF-8".into()))
            .and_then(|target| {
                if target.is_some_and(|value| value.len() > WIRE_MAX_REF_NAME_BYTES) {
                    Err(DurableError::Git(
                        "wire ref limit exceeded: HEAD target name".into(),
                    ))
                } else {
                    Ok(target.map(str::to_string))
                }
            }),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
        Err(error) => Err(DurableError::Git(format!("find HEAD: {error}"))),
    }
}

fn scan_refs(
    repo: &git2::Repository,
    head_target: Option<String>,
    query: Option<&str>,
    current_ref: Option<&str>,
    cursor: Option<&Cursor>,
    retain: usize,
    limits: ScanLimits,
) -> Result<ScanResult, DurableError> {
    let mut total_name_bytes = 0_usize;
    let mut branch_count = 0_usize;
    let mut tag_count = 0_usize;
    let mut namespace = NamespaceFingerprint::default();
    let mut rows = Vec::with_capacity(retain);
    let mut current_pin = None;
    let mut head_pin = None;
    let mut main_pin = None;
    let mut first_branch = None;
    let mut cursor_key_seen = cursor.is_none();
    let references = repo
        .references()
        .map_err(|error| DurableError::Git(format!("references: {error}")))?;
    for (scanned, reference) in references.enumerate() {
        let reference =
            reference.map_err(|error| DurableError::Git(format!("reference iter: {error}")))?;
        if scanned == limits.refs {
            return Err(DurableError::Git(
                "wire ref limit exceeded: scanned ref count".into(),
            ));
        }
        let full_name = reference
            .name()
            .map_err(|_| DurableError::Git("reference name is not valid UTF-8".into()))?;
        if full_name.len() > limits.one_name_bytes {
            return Err(DurableError::Git(
                "wire ref limit exceeded: one ref name".into(),
            ));
        }
        total_name_bytes = total_name_bytes
            .checked_add(full_name.len())
            .ok_or_else(|| DurableError::Git("wire ref limit exceeded: ref name bytes".into()))?;
        if total_name_bytes > limits.total_name_bytes {
            return Err(DurableError::Git(
                "wire ref limit exceeded: ref name bytes".into(),
            ));
        }
        let Some(target) = reference.target() else {
            continue;
        };
        namespace.add(full_name, target);
        let Some((kind, short_name)) = RefKind::from_qualified_name(full_name) else {
            continue;
        };
        match kind {
            RefKind::Branch => branch_count += 1,
            RefKind::Tag => tag_count += 1,
        }
        let item = RefPageItem {
            kind,
            qualified_name: full_name.to_string(),
            name: short_name.to_string(),
            tip: Oid::new(target.to_string()),
        };
        if current_ref == Some(full_name) {
            current_pin = Some(item.clone());
        }
        if head_target.as_deref() == Some(full_name) && kind == RefKind::Branch {
            head_pin = Some(item.clone());
        }
        if full_name == "refs/heads/main" {
            main_pin = Some(item.clone());
        }
        if kind == RefKind::Branch
            && first_branch
                .as_ref()
                .is_none_or(|first: &RefPageItem| item.key() < first.key())
        {
            first_branch = Some(item.clone());
        }
        let matches_query = query.is_none_or(|needle| item.name.to_lowercase().contains(needle));
        if cursor.as_ref().is_some_and(|value| {
            value.kind == item.kind
                && value.name.as_bytes() == item.name.as_bytes()
                && matches_query
        }) {
            cursor_key_seen = true;
        }
        let after_cursor = cursor.as_ref().is_none_or(|value| {
            item.key()
                > RefKey {
                    kind: value.kind,
                    name: &value.name,
                }
        });
        if retain != 0 && matches_query && after_cursor {
            insert_bounded(&mut rows, item, retain);
        }
    }
    let default = head_pin.or(main_pin).or(first_branch);
    let default_branch = default
        .as_ref()
        .map(|item| item.name.clone())
        .unwrap_or_else(|| "main".to_string());
    let default_tip = default.as_ref().map(|item| item.tip.clone());
    let mut pins = Vec::with_capacity(MAX_PINS);
    if let Some(pin) = current_pin {
        pins.push(pin);
    }
    if let Some(pin) = default {
        if pins
            .iter()
            .all(|item| item.qualified_name != pin.qualified_name)
            && pins.len() < MAX_PINS
        {
            pins.push(pin);
        }
    }
    Ok(ScanResult {
        summary: RefsSummary {
            branch_count,
            tag_count,
            head_symbolic_target: head_target.clone(),
            default_branch,
            default_tip,
        },
        rows,
        pins,
        namespace: namespace.finish(head_target.as_deref()),
        cursor_key_seen,
    })
}

fn insert_bounded(rows: &mut Vec<RefPageItem>, item: RefPageItem, retain: usize) {
    let index = rows
        .binary_search_by(|existing| existing.key().cmp(&item.key()))
        .unwrap_or_else(|index| index);
    rows.insert(index, item);
    if rows.len() > retain {
        rows.pop();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::core::RepoLoc;
    use crate::durable::DurableGitStore;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: std::path::PathBuf,
        repo: DurableGitRepo,
        first_tip: Oid,
        second_tip: Oid,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "myelin-ref-page-{label}-{}-{sequence}",
                std::process::id()
            ));
            let store = DurableGitStore::rooted(&root);
            let repo = store
                .create_repo(&RepoLoc::new("tenant", "eu-north", label))
                .expect("create bare repository");
            let blob = repo.write_blob(b"page\n").expect("blob");
            let tree = repo.write_tree(&[("page.txt", &blob)]).expect("tree");
            let first_tip = repo
                .write_commit(
                    &tree,
                    &[],
                    "first",
                    "psn@tenant.noreply",
                    "psn@tenant.noreply",
                )
                .expect("first commit");
            let second_tip = repo
                .write_commit(
                    &tree,
                    &[&first_tip],
                    "second",
                    "psn@tenant.noreply",
                    "psn@tenant.noreply",
                )
                .expect("second commit");
            Self {
                root,
                repo,
                first_tip,
                second_tip,
            }
        }

        fn git(&self) -> git2::Repository {
            self.repo.open_git().expect("open repository")
        }

        fn add_ref(&self, name: &str, tip: &Oid) {
            let oid = git2::Oid::from_str(tip.as_str()).expect("oid");
            self.git()
                .reference(name, oid, true, "test ref")
                .expect("write ref");
        }

        fn delete_ref(&self, name: &str) {
            self.git()
                .find_reference(name)
                .expect("find ref")
                .delete()
                .expect("delete ref");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn request(limit: usize) -> RefsPageRequest {
        RefsPageRequest {
            limit,
            ..RefsPageRequest::default()
        }
    }

    #[test]
    fn summary_and_default_page_limits_are_bounded() {
        let fixture = Fixture::new("summary-limits");
        for index in 0..1_005 {
            fixture.add_ref(&format!("refs/heads/branch-{index:04}"), &fixture.first_tip);
        }
        fixture.add_ref("refs/tags/release", &fixture.first_tip);
        fixture
            .git()
            .set_head("refs/heads/branch-0999")
            .expect("set HEAD");

        let summary = fixture.repo.refs_summary().expect("summary");
        assert_eq!(summary.branch_count, 1_005);
        assert_eq!(summary.tag_count, 1);
        assert_eq!(summary.default_branch, "branch-0999");
        assert_eq!(summary.default_tip, Some(fixture.first_tip.clone()));
        assert_eq!(
            summary.head_symbolic_target.as_deref(),
            Some("refs/heads/branch-0999")
        );

        let default_page = fixture
            .repo
            .refs_page(RefsPageRequest::default())
            .expect("default page");
        assert_eq!(default_page.items.len(), REFS_PAGE_DEFAULT_LIMIT);
        assert!(default_page.next_cursor.is_some());
        let exact_page = fixture
            .repo
            .refs_page(request(REFS_PAGE_MAX_LIMIT))
            .expect("exact maximum page");
        assert_eq!(exact_page.items.len(), REFS_PAGE_MAX_LIMIT);
        assert!(matches!(
            fixture.repo.refs_page(request(0)),
            Err(RefsPageError::InvalidLimit { supplied: 0 })
        ));
        assert!(matches!(
            fixture.repo.refs_page(request(REFS_PAGE_MAX_LIMIT + 1)),
            Err(RefsPageError::InvalidLimit { supplied: 101 })
        ));
    }

    #[test]
    fn summary_rejects_a_corrupt_head_instead_of_guessing_main() {
        let fixture = Fixture::new("corrupt-head");
        fixture.add_ref("refs/heads/main", &fixture.first_tip);
        std::fs::write(fixture.repo.path().join("HEAD"), b"ref: refs/heads/\xff\n")
            .expect("corrupt HEAD fixture");

        assert!(
            matches!(fixture.repo.refs_summary(), Err(DurableError::Git(_))),
            "an unreadable HEAD must not be represented as a valid main default"
        );
    }

    #[test]
    fn catalogue_state_uses_direct_branch_presence_only() {
        let unborn = Fixture::new("catalogue-unborn");
        assert_eq!(
            unborn.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Empty
        );

        let tag_only = Fixture::new("catalogue-tag-only");
        tag_only.add_ref("refs/tags/v1", &tag_only.first_tip);
        assert_eq!(
            tag_only.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Empty,
            "tags do not populate a repository catalogue row"
        );
        let tag_oid = git2::Oid::from_str(tag_only.first_tip.as_str()).unwrap();
        tag_only
            .git()
            .set_head_detached(tag_oid)
            .expect("detached HEAD");
        assert_eq!(
            tag_only.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Empty,
            "a detached HEAD is not a direct branch"
        );

        let live_head = Fixture::new("catalogue-live-head");
        live_head.add_ref("refs/heads/feature", &live_head.first_tip);
        live_head
            .git()
            .set_head("refs/heads/feature")
            .expect("live HEAD");
        assert_eq!(
            live_head.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Populated
        );

        let stale_main = Fixture::new("catalogue-stale-main");
        stale_main.add_ref("refs/heads/main", &stale_main.first_tip);
        stale_main
            .git()
            .set_head("refs/heads/missing")
            .expect("stale HEAD");
        assert_eq!(
            stale_main.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Populated
        );

        let stale_arbitrary = Fixture::new("catalogue-stale-arbitrary");
        stale_arbitrary.add_ref("refs/heads/zeta", &stale_arbitrary.first_tip);
        stale_arbitrary
            .git()
            .set_head("refs/heads/missing")
            .expect("stale HEAD");
        assert_eq!(
            stale_arbitrary.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Populated
        );
    }

    #[test]
    fn catalogue_state_ignores_symbolic_non_direct_branch_refs() {
        let fixture = Fixture::new("catalogue-symbolic-branch");
        fixture.add_ref("refs/tags/v1", &fixture.first_tip);
        fixture
            .git()
            .reference_symbolic("refs/heads/alias", "refs/tags/v1", true, "symbolic branch")
            .expect("symbolic branch");
        fixture
            .git()
            .set_head("refs/heads/alias")
            .expect("symbolic HEAD");

        assert_eq!(
            fixture.repo.catalogue_repo_state().unwrap(),
            CatalogueRepoState::Empty,
            "only direct refs/heads targets count as populated"
        );
    }

    #[test]
    fn catalogue_branch_scan_enforces_exact_count_and_name_bounds() {
        let fixture = Fixture::new("catalogue-scan-bounds");
        let branch = "refs/heads/arbitrary";
        fixture.add_ref(branch, &fixture.first_tip);
        let exact = ScanLimits {
            refs: 1,
            one_name_bytes: branch.len(),
            total_name_bytes: branch.len(),
        };
        assert_eq!(
            fixture
                .repo
                .catalogue_repo_state_with_limits(exact)
                .unwrap(),
            CatalogueRepoState::Populated
        );
        for limits in [
            ScanLimits { refs: 0, ..exact },
            ScanLimits {
                one_name_bytes: branch.len() - 1,
                ..exact
            },
            ScanLimits {
                total_name_bytes: branch.len() - 1,
                ..exact
            },
        ] {
            assert!(matches!(
                fixture.repo.catalogue_repo_state_with_limits(limits),
                Err(DurableError::Git(message)) if message.starts_with("wire ref limit exceeded:")
            ));
        }
    }

    #[test]
    fn more_than_one_thousand_refs_page_without_duplicates_or_skips() {
        let fixture = Fixture::new("large-namespace");
        let mut expected = Vec::new();
        for index in 0..1_003 {
            let name = format!("branch-{index:04}");
            fixture.add_ref(&format!("refs/heads/{name}"), &fixture.first_tip);
            expected.push((RefKind::Branch, name));
        }
        for index in 0..7 {
            let name = format!("tag-{index:02}");
            fixture.add_ref(&format!("refs/tags/{name}"), &fixture.first_tip);
            expected.push((RefKind::Tag, name));
        }

        let mut actual = Vec::new();
        let mut cursor = None;
        loop {
            let page = fixture
                .repo
                .refs_page(RefsPageRequest {
                    limit: 73,
                    cursor,
                    ..RefsPageRequest::default()
                })
                .expect("page");
            actual.extend(page.items.into_iter().map(|item| (item.kind, item.name)));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn raw_name_order_query_normalization_and_pins_are_independent() {
        let fixture = Fixture::new("order-query-pins");
        fixture.add_ref("refs/heads/z-current", &fixture.first_tip);
        fixture.add_ref("refs/heads/main", &fixture.first_tip);
        fixture.add_ref("refs/heads/Alpha", &fixture.first_tip);
        fixture.add_ref("refs/heads/feature-One", &fixture.first_tip);
        fixture.add_ref("refs/heads/feature-two", &fixture.first_tip);
        fixture.add_ref("refs/heads/åland", &fixture.first_tip);
        fixture.add_ref("refs/tags/A-tag", &fixture.first_tip);
        fixture.git().set_head("refs/heads/main").expect("set HEAD");

        let ordered = fixture.repo.refs_page(request(100)).expect("ordered page");
        assert_eq!(
            ordered
                .items
                .iter()
                .map(|item| (item.kind, item.name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (RefKind::Branch, "Alpha"),
                (RefKind::Branch, "feature-One"),
                (RefKind::Branch, "feature-two"),
                (RefKind::Branch, "main"),
                (RefKind::Branch, "z-current"),
                (RefKind::Branch, "åland"),
                (RefKind::Tag, "A-tag"),
            ]
        );

        let filtered = fixture
            .repo
            .refs_page(RefsPageRequest {
                limit: 1,
                query: Some("  FeAtUrE-  ".into()),
                current_ref: Some("refs/heads/z-current".into()),
                ..RefsPageRequest::default()
            })
            .expect("filtered page");
        assert_eq!(filtered.items[0].name, "feature-One");
        let filtered_next = fixture
            .repo
            .refs_page(RefsPageRequest {
                limit: 1,
                query: Some("feature-".into()),
                cursor: filtered.next_cursor.clone(),
                ..RefsPageRequest::default()
            })
            .expect("normalized-query continuation");
        assert_eq!(filtered_next.items[0].name, "feature-two");
        assert!(filtered_next.next_cursor.is_none());
        assert_eq!(
            filtered
                .pins
                .iter()
                .map(|pin| pin.qualified_name.as_str())
                .collect::<Vec<_>>(),
            vec!["refs/heads/z-current", "refs/heads/main"]
        );
        assert!(matches!(
            fixture.repo.refs_page(RefsPageRequest {
                current_ref: Some("z-current".into()),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::InvalidCurrentRef)
        ));
        let missing = fixture
            .repo
            .refs_page(RefsPageRequest {
                current_ref: Some("refs/heads/missing".into()),
                ..RefsPageRequest::default()
            })
            .expect("missing current is not guessed");
        assert_eq!(missing.pins.len(), 1);
        assert_eq!(missing.pins[0].qualified_name, "refs/heads/main");
        let deduplicated = fixture
            .repo
            .refs_page(RefsPageRequest {
                current_ref: Some("refs/heads/main".into()),
                ..RefsPageRequest::default()
            })
            .expect("current/default deduplication");
        assert_eq!(deduplicated.pins.len(), 1);
        assert_eq!(deduplicated.pins[0].qualified_name, "refs/heads/main");
    }

    fn first_cursor(fixture: &Fixture, query: Option<&str>) -> String {
        fixture
            .repo
            .refs_page(RefsPageRequest {
                limit: 1,
                query: query.map(str::to_string),
                ..RefsPageRequest::default()
            })
            .expect("first page")
            .next_cursor
            .expect("continuation")
    }

    #[test]
    fn malformed_noncanonical_and_wrong_scope_cursors_are_typed() {
        let fixture = Fixture::new("cursor-errors");
        fixture.add_ref("refs/heads/aa", &fixture.first_tip);
        fixture.add_ref("refs/heads/bb", &fixture.first_tip);
        let cursor = first_cursor(&fixture, None);

        for malformed in [
            "not-a-cursor".to_string(),
            format!("{cursor}="),
            "gr1_AA".to_string(),
        ] {
            assert!(matches!(
                fixture.repo.refs_page(RefsPageRequest {
                    limit: 1,
                    cursor: Some(malformed),
                    ..RefsPageRequest::default()
                }),
                Err(RefsPageError::MalformedCursor)
            ));
        }

        let mut noncanonical = cursor.clone();
        let final_char = noncanonical.pop().expect("cursor char");
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let index = alphabet
            .iter()
            .position(|byte| *byte == final_char as u8)
            .expect("base64url char");
        let replacement = alphabet[(index & !0x0f) | ((index + 1) & 0x0f)] as char;
        noncanonical.push(replacement);
        assert!(matches!(
            fixture.repo.refs_page(RefsPageRequest {
                limit: 1,
                cursor: Some(noncanonical),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::MalformedCursor)
        ));

        assert!(matches!(
            fixture.repo.refs_page(RefsPageRequest {
                limit: 1,
                query: Some("different".into()),
                cursor: Some(cursor.clone()),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::CursorScopeMismatch)
        ));
        let other = Fixture::new("other-repository");
        other.add_ref("refs/heads/aa", &other.first_tip);
        other.add_ref("refs/heads/bb", &other.first_tip);
        assert!(matches!(
            other.repo.refs_page(RefsPageRequest {
                limit: 1,
                cursor: Some(cursor),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::CursorScopeMismatch)
        ));
    }

    #[test]
    fn add_delete_and_retarget_each_stale_a_continuation() {
        let fixture = Fixture::new("cursor-stale");
        fixture.add_ref("refs/heads/a", &fixture.first_tip);
        fixture.add_ref("refs/heads/b", &fixture.first_tip);

        let before_add = first_cursor(&fixture, None);
        fixture.add_ref("refs/tags/v1", &fixture.first_tip);
        assert_stale(&fixture, before_add);

        let before_delete = first_cursor(&fixture, None);
        fixture.delete_ref("refs/tags/v1");
        assert_stale(&fixture, before_delete);

        let before_retarget = first_cursor(&fixture, None);
        fixture.add_ref("refs/heads/b", &fixture.second_tip);
        assert_stale(&fixture, before_retarget);
    }

    fn assert_stale(fixture: &Fixture, cursor: String) {
        assert!(matches!(
            fixture.repo.refs_page(RefsPageRequest {
                limit: 1,
                cursor: Some(cursor),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::CursorStale)
        ));
    }

    #[test]
    fn forged_last_key_with_valid_fences_is_malformed() {
        let fixture = Fixture::new("cursor-key");
        fixture.add_ref("refs/heads/a", &fixture.first_tip);
        fixture.add_ref("refs/heads/b", &fixture.first_tip);
        let token = first_cursor(&fixture, None);
        let mut cursor = Cursor::parse(&token).expect("parse cursor");
        cursor.name = "not-in-namespace".into();
        assert!(matches!(
            fixture.repo.refs_page(RefsPageRequest {
                limit: 1,
                cursor: Some(cursor.encode()),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::MalformedCursor)
        ));
    }

    #[test]
    fn scan_enforces_exact_ref_count_and_aggregate_name_bounds() {
        assert_eq!(WIRE_SCAN_LIMITS.refs, WIRE_MAX_REFS);
        let fixture = Fixture::new("scan-bounds");
        fixture.add_ref("refs/heads/a", &fixture.first_tip);
        fixture.add_ref("refs/heads/b", &fixture.first_tip);
        fixture.add_ref("refs/tags/c", &fixture.first_tip);
        let git = fixture.git();
        let head = read_head_target(&git).expect("HEAD");

        scan_refs(
            &git,
            head.clone(),
            None,
            None,
            None,
            1,
            ScanLimits {
                refs: 3,
                one_name_bytes: WIRE_MAX_REF_NAME_BYTES,
                total_name_bytes: WIRE_MAX_REF_NAMES_TOTAL_BYTES,
            },
        )
        .expect("exact ref limit succeeds");
        assert!(matches!(
            scan_refs(
                &git,
                head.clone(),
                None,
                None,
                None,
                1,
                ScanLimits {
                    refs: 2,
                    one_name_bytes: WIRE_MAX_REF_NAME_BYTES,
                    total_name_bytes: WIRE_MAX_REF_NAMES_TOTAL_BYTES,
                },
            ),
            Err(DurableError::Git(message))
                if message == "wire ref limit exceeded: scanned ref count"
        ));
        assert!(matches!(
            scan_refs(
                &git,
                head,
                None,
                None,
                None,
                1,
                ScanLimits {
                    refs: WIRE_MAX_REFS,
                    one_name_bytes: WIRE_MAX_REF_NAME_BYTES,
                    total_name_bytes: "refs/heads/a".len() - 1,
                },
            ),
            Err(DurableError::Git(message))
                if message == "wire ref limit exceeded: ref name bytes"
        ));
    }

    #[test]
    fn query_bytes_are_bounded_before_and_after_normalization() {
        let fixture = Fixture::new("query-bound");
        fixture.add_ref("refs/heads/a", &fixture.first_tip);
        assert!(matches!(
            fixture.repo.refs_page(RefsPageRequest {
                query: Some("x".repeat(REFS_PAGE_MAX_QUERY_BYTES + 1)),
                ..RefsPageRequest::default()
            }),
            Err(RefsPageError::QueryTooLong {
                maximum: REFS_PAGE_MAX_QUERY_BYTES
            })
        ));
        fixture
            .repo
            .refs_page(RefsPageRequest {
                query: Some("x".repeat(REFS_PAGE_MAX_QUERY_BYTES)),
                ..RefsPageRequest::default()
            })
            .expect("exact query bound succeeds");
    }
}
