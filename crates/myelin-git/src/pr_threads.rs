use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};

use crate::core::RepoLoc;
use crate::durable::DurableError;
use crate::gix_backend::{RepoPathResolver, RootedResolver};

pub const MAX_COMMENT_BODY_BYTES: usize = 64 * 1024;
pub const MAX_REVIEW_SUMMARY_BYTES: usize = 64 * 1024;
pub const MAX_THREAD_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_THREADS_PER_SUBJECT: usize = 4_096;
pub const MAX_COMMENTS_PER_SUBJECT: usize = 8_192;
pub const MAX_REVIEWS_PER_SUBJECT: usize = 1_024;

#[derive(Clone, PartialEq, Eq)]
pub struct PendingCommentRequest {
    repo: RepoLoc,
    object_key: String,
    review_id: String,
    anchor: Option<ThreadAnchor>,
    author: ThreadPrincipal,
    body_md: String,
    now: i64,
}

impl PendingCommentRequest {
    pub fn new(
        repo: RepoLoc,
        object_key: impl Into<String>,
        review_id: impl Into<String>,
        anchor: Option<ThreadAnchor>,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> Result<Self, DurableError> {
        let object_key = object_key.into();
        let review_id = review_id.into();
        let body_md = validate_markdown(body_md.into(), MAX_COMMENT_BODY_BYTES, "comment body")?;
        validate_review_target(&repo, &object_key, &review_id)?;
        Ok(Self {
            repo,
            object_key,
            review_id,
            anchor,
            author,
            body_md,
            now,
        })
    }
}

impl core::fmt::Debug for PendingCommentRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PendingCommentRequest")
            .field("repo", &self.repo)
            .field("object_key", &self.object_key)
            .field("review_id", &self.review_id)
            .field("anchor", &self.anchor)
            .field("author", &"<redacted>")
            .field("body_md", &"<redacted>")
            .field("now", &self.now)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SubmitReviewRequest {
    repo: RepoLoc,
    object_key: String,
    review_id: String,
    actor: ThreadPrincipal,
    verdict: BatchVerdict,
    summary_md: Option<String>,
    now: i64,
}

impl SubmitReviewRequest {
    pub fn new(
        repo: RepoLoc,
        object_key: impl Into<String>,
        review_id: impl Into<String>,
        actor: ThreadPrincipal,
        verdict: BatchVerdict,
        summary_md: Option<String>,
        now: i64,
    ) -> Result<Self, DurableError> {
        let object_key = object_key.into();
        let review_id = review_id.into();
        let summary_md = summary_md
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| validate_markdown(summary, MAX_REVIEW_SUMMARY_BYTES, "review summary"))
            .transpose()?;
        validate_review_target(&repo, &object_key, &review_id)?;
        Ok(Self {
            repo,
            object_key,
            review_id,
            actor,
            verdict,
            summary_md,
            now,
        })
    }
}

fn validate_markdown(value: String, max_bytes: usize, field: &str) -> Result<String, DurableError> {
    if value.trim().is_empty() {
        return Err(DurableError::Git(format!("{field} is missing or blank")));
    }
    if value.len() > max_bytes {
        return Err(DurableError::Git(format!(
            "{field} exceeds the {max_bytes}-byte limit"
        )));
    }
    Ok(value)
}

impl core::fmt::Debug for SubmitReviewRequest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SubmitReviewRequest")
            .field("repo", &self.repo)
            .field("object_key", &self.object_key)
            .field("review_id", &self.review_id)
            .field("actor", &"<redacted>")
            .field("verdict", &self.verdict)
            .field(
                "summary_md",
                &self.summary_md.as_ref().map(|_| "<redacted>"),
            )
            .field("now", &self.now)
            .finish()
    }
}

fn validate_review_target(
    repo: &RepoLoc,
    object_key: &str,
    review_id: &str,
) -> Result<(), DurableError> {
    if repo.tenant.trim().is_empty()
        || repo.region.trim().is_empty()
        || repo.repo.trim().is_empty()
        || object_key.trim().is_empty()
        || review_id.trim().is_empty()
    {
        return Err(DurableError::Git(
            "review operation requires a complete repository, object key, and review id".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadPrincipal {
    pub kind: PrincipalRole,
    pub display: String,
    #[serde(default)]
    pub on_behalf_of: Option<String>,
    #[serde(default)]
    pub trigger: Option<String>,
}

impl ThreadPrincipal {
    pub fn plain(kind: PrincipalRole, display: impl Into<String>) -> Self {
        ThreadPrincipal {
            kind,
            display: display.into(),
            on_behalf_of: None,
            trigger: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    Human,
    Agent,
    Service,
}

impl PrincipalRole {
    pub fn is_agent(self) -> bool {
        matches!(self, PrincipalRole::Agent)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadAnchor {
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub side: Option<AnchorSide>,
    #[serde(default)]
    pub base_oid: Option<String>,
    #[serde(default)]
    pub head_oid: Option<String>,
    pub anchor_state: AnchorState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorSide {
    Old,
    New,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorState {
    Live,
    Moved,
    Outdated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentState {
    Visible,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRecord {
    pub id: String,
    pub author: ThreadPrincipal,
    pub body_md: String,
    pub created_at: i64,
    #[serde(default)]
    pub edited_at: Option<i64>,
    #[serde(default = "visible")]
    pub state: CommentState,
    #[serde(default)]
    pub review_id: Option<String>,
    #[serde(default)]
    pub pending: bool,
}

fn visible() -> CommentState {
    CommentState::Visible
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub id: String,
    #[serde(default)]
    pub anchor: Option<ThreadAnchor>,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub comments: Vec<CommentRecord>,
}

impl ThreadRecord {
    pub fn is_discussion(&self) -> bool {
        self.anchor.is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchVerdict {
    InProgress,
    Approved,
    ChangesRequested,
    Commented,
}

impl BatchVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            BatchVerdict::InProgress => "in_progress",
            BatchVerdict::Approved => "approved",
            BatchVerdict::ChangesRequested => "changes_requested",
            BatchVerdict::Commented => "commented",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBatch {
    pub id: String,
    pub reviewer: ThreadPrincipal,
    pub verdict: BatchVerdict,
    pub advisory: bool,
    #[serde(default)]
    pub submitted_at: Option<i64>,
    #[serde(default)]
    pub summary_md: Option<String>,
}

impl ReviewBatch {
    pub fn is_draft(&self) -> bool {
        self.submitted_at.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectThreads {
    #[serde(default)]
    pub object_key: String,
    #[serde(default)]
    pub threads: Vec<ThreadRecord>,
    #[serde(default)]
    pub reviews: Vec<ReviewBatch>,
    #[serde(default)]
    pub seq: u64,
}

impl SubjectThreads {
    fn next_id(&mut self, prefix: &str) -> Result<String, DurableError> {
        self.seq = self.seq.checked_add(1).ok_or_else(|| {
            DurableError::Git("PR thread/comment/review id sequence exhausted at u64::MAX".into())
        })?;
        Ok(format!("{prefix}-{}", self.seq))
    }

    fn review(&self, review_id: &str) -> Option<&ReviewBatch> {
        self.reviews.iter().find(|r| r.id == review_id)
    }

    fn comment_count(&self) -> usize {
        self.threads
            .iter()
            .map(|thread| thread.comments.len())
            .sum()
    }

    fn validate_cardinality(&self) -> Result<(), DurableError> {
        if self.threads.len() > MAX_THREADS_PER_SUBJECT
            || self.comment_count() > MAX_COMMENTS_PER_SUBJECT
            || self.reviews.len() > MAX_REVIEWS_PER_SUBJECT
        {
            return Err(DurableError::Git(
                "PR conversation exceeds its cardinality limit".into(),
            ));
        }
        Ok(())
    }

    pub fn view_for(&self, viewer_display: &str) -> ViewedThreads {
        let threads = self
            .threads
            .iter()
            .filter_map(|t| {
                let comments: Vec<CommentRecord> = t
                    .comments
                    .iter()
                    .filter(|c| !c.pending || c.author.display == viewer_display)
                    .cloned()
                    .collect();
                if comments.is_empty() {
                    return None;
                }
                Some(ThreadRecord {
                    id: t.id.clone(),
                    anchor: t.anchor.clone(),
                    resolved: t.resolved,
                    comments,
                })
            })
            .collect();
        let reviews = self
            .reviews
            .iter()
            .filter(|r| !r.is_draft() || r.reviewer.display == viewer_display)
            .cloned()
            .collect();
        ViewedThreads { threads, reviews }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewedThreads {
    pub threads: Vec<ThreadRecord>,
    pub reviews: Vec<ReviewBatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedBatch {
    pub review: ReviewBatch,
    pub comment_ids: Vec<String>,
}

pub struct DurablePrThreadStore<P: RepoPathResolver = RootedResolver> {
    resolver: P,
    subject_locks: Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>,
}

impl DurablePrThreadStore<RootedResolver> {
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            resolver: RootedResolver::new(root),
            subject_locks: Mutex::new(BTreeMap::new()),
        }
    }
}

fn key_stem(object_key: &str) -> String {
    object_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl<P: RepoPathResolver> DurablePrThreadStore<P> {
    pub fn new(resolver: P) -> Self {
        Self {
            resolver,
            subject_locks: Mutex::new(BTreeMap::new()),
        }
    }

    fn threads_dir(&self, repo: &RepoLoc) -> Result<PathBuf, DurableError> {
        let repo_path = self
            .resolver
            .repo_path(repo)
            .map_err(|e| DurableError::Git(e.to_string()))?;
        Ok(repo_path.join("myelin").join("threads"))
    }

    fn subject_path(&self, repo: &RepoLoc, object_key: &str) -> Result<PathBuf, DurableError> {
        Ok(self
            .threads_dir(repo)?
            .join(format!("{}.json", key_stem(object_key))))
    }

    fn subject_lock(
        &self,
        repo: &RepoLoc,
        object_key: &str,
    ) -> Result<Arc<Mutex<()>>, DurableError> {
        let path = self.subject_path(repo, object_key)?;
        let mut locks = self.subject_locks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(lock) = locks.get(&path).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        locks.retain(|_, weak| weak.strong_count() > 0);
        let lock = Arc::new(Mutex::new(()));
        locks.insert(path, Arc::downgrade(&lock));
        Ok(lock)
    }

    pub fn load(&self, repo: &RepoLoc, object_key: &str) -> Result<SubjectThreads, DurableError> {
        let path = self.subject_path(repo, object_key)?;
        match std::fs::File::open(&path) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take((MAX_THREAD_DOCUMENT_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| DurableError::Io(format!("read {}: {e}", path.display())))?;
                if bytes.len() > MAX_THREAD_DOCUMENT_BYTES {
                    return Err(DurableError::Io(
                        "stored PR conversation exceeds its document limit".into(),
                    ));
                }
                let mut doc: SubjectThreads = serde_json::from_slice(&bytes)
                    .map_err(|e| DurableError::Io(format!("parse {}: {e}", path.display())))?;
                if doc.object_key.is_empty() {
                    doc.object_key = object_key.to_string();
                } else if doc.object_key != object_key {
                    return Err(DurableError::Git(format!(
                        "thread document subject mismatch: requested `{object_key}` but stored key is `{}`",
                        doc.object_key
                    )));
                }
                doc.validate_cardinality().map_err(|_| {
                    DurableError::Io("stored PR conversation exceeds its cardinality limit".into())
                })?;
                Ok(doc)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SubjectThreads {
                object_key: object_key.to_string(),
                ..Default::default()
            }),
            Err(e) => Err(DurableError::Io(format!("read {}: {e}", path.display()))),
        }
    }

    fn save(&self, repo: &RepoLoc, doc: &SubjectThreads) -> Result<(), DurableError> {
        doc.validate_cardinality()?;
        let dir = self.threads_dir(repo)?;
        let file = self.subject_path(repo, &doc.object_key)?;
        let bytes = serde_json::to_vec_pretty(doc)
            .map_err(|e| DurableError::Io(format!("serialize threads {}: {e}", doc.object_key)))?;
        if bytes.len() > MAX_THREAD_DOCUMENT_BYTES {
            return Err(DurableError::Git(format!(
                "PR conversation exceeds the {MAX_THREAD_DOCUMENT_BYTES}-byte document limit"
            )));
        }
        crate::durable::write_file_atomic(&dir, &file, &bytes)
    }

    pub fn create_thread(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        anchor: Option<ThreadAnchor>,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> Result<ThreadRecord, DurableError> {
        let body_md = validate_markdown(body_md.into(), MAX_COMMENT_BODY_BYTES, "comment body")?;
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if doc.threads.len() >= MAX_THREADS_PER_SUBJECT
            || doc.comment_count() >= MAX_COMMENTS_PER_SUBJECT
        {
            return Err(DurableError::Git(
                "PR conversation exceeds its thread or comment limit".into(),
            ));
        }
        let tid = doc.next_id("t")?;
        let cid = doc.next_id("c")?;
        let thread = ThreadRecord {
            id: tid,
            anchor,
            resolved: false,
            comments: vec![CommentRecord {
                id: cid,
                author,
                body_md,
                created_at: now,
                edited_at: None,
                state: CommentState::Visible,
                review_id: None,
                pending: false,
            }],
        };
        doc.threads.push(thread.clone());
        self.save(repo, &doc)?;
        Ok(thread)
    }

    pub fn add_comment(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        thread_id: &str,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> Result<CommentRecord, DurableError> {
        let body_md = validate_markdown(body_md.into(), MAX_COMMENT_BODY_BYTES, "comment body")?;
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if doc.comment_count() >= MAX_COMMENTS_PER_SUBJECT {
            return Err(DurableError::Git(
                "PR conversation exceeds its comment limit".into(),
            ));
        }
        let cid = doc.next_id("c")?;
        let thread = doc
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| DurableError::NotFound(format!("thread {thread_id}")))?;
        let comment = CommentRecord {
            id: cid,
            author,
            body_md,
            created_at: now,
            edited_at: None,
            state: CommentState::Visible,
            review_id: None,
            pending: false,
        };
        thread.comments.push(comment.clone());
        self.save(repo, &doc)?;
        Ok(comment)
    }

    pub fn resolve_thread(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), DurableError> {
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        let thread = doc
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| DurableError::NotFound(format!("thread {thread_id}")))?;
        thread.resolved = resolved;
        self.save(repo, &doc)?;
        Ok(())
    }

    pub fn start_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        reviewer: ThreadPrincipal,
    ) -> Result<ReviewBatch, DurableError> {
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if let Some(existing) = doc
            .reviews
            .iter()
            .find(|review| review.is_draft() && review.reviewer.display == reviewer.display)
        {
            return Ok(existing.clone());
        }
        if doc.reviews.len() >= MAX_REVIEWS_PER_SUBJECT {
            return Err(DurableError::Git(
                "PR conversation exceeds its review limit".into(),
            ));
        }
        let rid = doc.next_id("r")?;
        let batch = ReviewBatch {
            id: rid,
            advisory: reviewer.kind.is_agent(),
            reviewer,
            verdict: BatchVerdict::InProgress,
            submitted_at: None,
            summary_md: None,
        };
        doc.reviews.push(batch.clone());
        self.save(repo, &doc)?;
        Ok(batch)
    }

    pub fn add_pending_comment(
        &self,
        request: PendingCommentRequest,
    ) -> Result<CommentRecord, DurableError> {
        let PendingCommentRequest {
            repo,
            object_key,
            review_id,
            anchor,
            author,
            body_md,
            now,
        } = request;
        let lock = self.subject_lock(&repo, &object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(&repo, &object_key)?;
        if doc.threads.len() >= MAX_THREADS_PER_SUBJECT
            || doc.comment_count() >= MAX_COMMENTS_PER_SUBJECT
        {
            return Err(DurableError::Git(
                "PR conversation exceeds its thread or comment limit".into(),
            ));
        }
        match doc.review(&review_id) {
            None => return Err(DurableError::NotFound(format!("review {review_id}"))),
            Some(r) if !r.is_draft() => {
                return Err(DurableError::Forbidden(format!(
                    "review {review_id} is already submitted - cannot append pending comments"
                )))
            }
            Some(r) if r.reviewer.display != author.display => {
                return Err(DurableError::Forbidden(format!(
                    "review {review_id} belongs to another reviewer"
                )))
            }
            Some(_) => {}
        }
        let tid = doc.next_id("t")?;
        let cid = doc.next_id("c")?;
        let comment = CommentRecord {
            id: cid,
            author,
            body_md,
            created_at: now,
            edited_at: None,
            state: CommentState::Visible,
            review_id: Some(review_id),
            pending: true,
        };
        doc.threads.push(ThreadRecord {
            id: tid,
            anchor,
            resolved: false,
            comments: vec![comment.clone()],
        });
        self.save(&repo, &doc)?;
        Ok(comment)
    }

    pub fn submit_review(
        &self,
        request: SubmitReviewRequest,
    ) -> Result<Option<SubmittedBatch>, DurableError> {
        let SubmitReviewRequest {
            repo,
            object_key,
            review_id,
            actor,
            verdict,
            summary_md,
            now,
        } = request;
        let lock = self.subject_lock(&repo, &object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(&repo, &object_key)?;
        let (owner_display, already) = match doc.review(&review_id) {
            None => return Err(DurableError::NotFound(format!("review {review_id}"))),
            Some(r) => (r.reviewer.display.clone(), !r.is_draft()),
        };
        if owner_display != actor.display {
            return Err(DurableError::Forbidden(format!(
                "review {review_id} belongs to another reviewer"
            )));
        }
        if already {
            return Ok(None);
        }
        let mut comment_ids = Vec::new();
        for t in &mut doc.threads {
            for c in &mut t.comments {
                if c.review_id.as_deref() == Some(review_id.as_str()) && c.pending {
                    c.pending = false;
                    comment_ids.push(c.id.clone());
                }
            }
        }
        let review = {
            let r = doc
                .reviews
                .iter_mut()
                .find(|r| r.id == review_id)
                .expect("batch present (checked above)");
            r.verdict = verdict;
            r.summary_md = summary_md;
            r.submitted_at = Some(now);
            r.clone()
        };
        self.save(&repo, &doc)?;
        Ok(Some(SubmittedBatch {
            review,
            comment_ids,
        }))
    }

    pub fn discard_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        review_id: &str,
        actor: &ThreadPrincipal,
    ) -> Result<(), DurableError> {
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        match doc.review(review_id) {
            None => return Err(DurableError::NotFound(format!("review {review_id}"))),
            Some(r) if !r.is_draft() => {
                return Err(DurableError::Forbidden(format!(
                    "review {review_id} is already submitted - its comments are public record"
                )))
            }
            Some(r) if r.reviewer.display != actor.display => {
                return Err(DurableError::Forbidden(format!(
                    "review {review_id} belongs to another reviewer"
                )))
            }
            Some(_) => {}
        }
        for t in &mut doc.threads {
            t.comments
                .retain(|c| c.review_id.as_deref() != Some(review_id));
        }
        doc.threads.retain(|t| !t.comments.is_empty());
        doc.reviews.retain(|r| r.id != review_id);
        self.save(repo, &doc)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        p.push(format!("myelin-threads-{tag}-{nanos}"));
        p
    }

    fn loc() -> RepoLoc {
        RepoLoc::new("acme", "fr-par", "core")
    }

    const KEY: &str = "pr:core:42";

    fn human(name: &str) -> ThreadPrincipal {
        ThreadPrincipal::plain(PrincipalRole::Human, name)
    }

    fn pending_comment(
        review_id: &str,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> PendingCommentRequest {
        PendingCommentRequest::new(loc(), KEY, review_id, None, author, body_md, now).unwrap()
    }

    fn submitted_review(
        review_id: &str,
        actor: ThreadPrincipal,
        verdict: BatchVerdict,
        summary_md: Option<String>,
        now: i64,
    ) -> SubmitReviewRequest {
        SubmitReviewRequest::new(loc(), KEY, review_id, actor, verdict, summary_md, now).unwrap()
    }

    #[test]
    fn review_requests_validate_targets_and_redact_authors_and_content() {
        let pending = PendingCommentRequest::new(
            loc(),
            KEY,
            "r-secret",
            None,
            human("psn:secret-author@acme"),
            "sensitive draft body",
            1,
        )
        .unwrap();
        let pending_debug = format!("{pending:?}");
        assert!(!pending_debug.contains("psn:secret-author@acme"));
        assert!(!pending_debug.contains("sensitive draft body"));

        let submit = SubmitReviewRequest::new(
            loc(),
            KEY,
            "r-secret",
            human("psn:secret-reviewer@acme"),
            BatchVerdict::Approved,
            Some("sensitive summary".into()),
            2,
        )
        .unwrap();
        let submit_debug = format!("{submit:?}");
        assert!(!submit_debug.contains("psn:secret-reviewer@acme"));
        assert!(!submit_debug.contains("sensitive summary"));

        assert!(PendingCommentRequest::new(
            RepoLoc::new("acme", "fr-par", ""),
            KEY,
            "r-1",
            None,
            human("psn:r@acme"),
            "draft",
            3,
        )
        .is_err());
        assert!(SubmitReviewRequest::new(
            loc(),
            " ",
            "r-1",
            human("psn:r@acme"),
            BatchVerdict::Commented,
            None,
            4,
        )
        .is_err());
        assert!(PendingCommentRequest::new(
            loc(),
            KEY,
            "r-1",
            None,
            human("psn:r@acme"),
            "x".repeat(MAX_COMMENT_BODY_BYTES + 1),
            5,
        )
        .is_err());
        assert!(SubmitReviewRequest::new(
            loc(),
            KEY,
            "r-1",
            human("psn:r@acme"),
            BatchVerdict::Commented,
            Some("x".repeat(MAX_REVIEW_SUMMARY_BYTES + 1)),
            6,
        )
        .is_err());
    }

    #[test]
    fn repeated_review_start_reuses_the_reviewers_active_draft() {
        let root = temp_root("one-active-draft");
        let store = DurablePrThreadStore::rooted(&root);
        let reviewer = human("psn:reviewer@acme");

        let first = store.start_review(&loc(), KEY, reviewer.clone()).unwrap();
        let retry = store.start_review(&loc(), KEY, reviewer).unwrap();

        assert_eq!(retry.id, first.id);
        assert_eq!(store.load(&loc(), KEY).unwrap().reviews.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversized_stored_document_is_rejected_without_unbounded_reading() {
        let root = temp_root("oversized-doc");
        let store = DurablePrThreadStore::rooted(&root);
        let path = store.subject_path(&loc(), KEY).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b' '; MAX_THREAD_DOCUMENT_BYTES + 1]).unwrap();

        let error = store
            .load(&loc(), KEY)
            .expect_err("oversized document must fail closed");
        assert!(error.to_string().contains("document limit"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_discussion_thread_round_trips_durably() {
        let root = temp_root("rt");
        let store = DurablePrThreadStore::rooted(&root);
        store
            .create_thread(&loc(), KEY, None, human("psn:a@acme"), "first post", 100)
            .unwrap();

        let back = DurablePrThreadStore::rooted(&root)
            .load(&loc(), KEY)
            .unwrap();
        assert_eq!(back.threads.len(), 1);
        assert!(back.threads[0].is_discussion());
        assert_eq!(back.threads[0].comments[0].body_md, "first post");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn concurrent_subject_writes_preserve_every_thread() {
        const WRITERS: usize = 32;
        let root = temp_root("concurrent-writes");
        let store = Arc::new(DurablePrThreadStore::rooted(&root));
        let barrier = Arc::new(std::sync::Barrier::new(WRITERS));
        let mut handles = Vec::with_capacity(WRITERS);
        for writer in 0..WRITERS {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                store.create_thread(
                    &loc(),
                    KEY,
                    None,
                    human(&format!("psn:writer-{writer}@acme")),
                    format!("comment-{writer}"),
                    writer as i64,
                )
            }));
        }
        for handle in handles {
            handle.join().expect("writer must not panic").expect("writer must persist");
        }

        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(doc.threads.len(), WRITERS, "no concurrent write may be lost");
        assert_eq!(doc.seq, (WRITERS * 2) as u64, "thread and comment ids stay monotonic");
        let bodies: std::collections::BTreeSet<_> = doc
            .threads
            .iter()
            .map(|thread| thread.comments[0].body_md.as_str())
            .collect();
        assert_eq!(bodies.len(), WRITERS, "every writer remains distinguishable");
        drop(store);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn subject_lock_registry_prunes_inactive_subjects() {
        let root = temp_root("subject-lock-registry");
        let store = DurablePrThreadStore::rooted(&root);

        let first = store.subject_lock(&loc(), "pr:core:1").unwrap();
        let same = store.subject_lock(&loc(), "pr:core:1").unwrap();
        assert!(Arc::ptr_eq(&first, &same), "overlapping operations share one lock");
        drop(first);
        drop(same);

        for number in 2..=2_000 {
            let lock = store
                .subject_lock(&loc(), &format!("pr:core:{number}"))
                .unwrap();
            drop(lock);
        }

        let locks = store.subject_locks.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(locks.len(), 1, "dead subject keys must not accumulate");
        assert_eq!(
            locks.values().filter(|weak| weak.strong_count() > 0).count(),
            0,
            "the final inactive entry is only a weak cache placeholder"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_legacy_subject_doc_deserializes_with_defaults() {
        let legacy = serde_json::json!({ "object_key": "pr:core:1", "threads": [
            { "id": "t-1", "comments": [
                { "id": "c-1", "author": { "kind": "human", "display": "psn:a@acme" },
                  "body_md": "hi", "created_at": 1 } ] } ] });
        let doc: SubjectThreads = serde_json::from_value(legacy).expect("legacy doc deserializes");
        assert_eq!(doc.reviews.len(), 0);
        assert!(!doc.threads[0].resolved);
        assert_eq!(doc.threads[0].anchor, None);
        let c = &doc.threads[0].comments[0];
        assert_eq!(c.state, CommentState::Visible);
        assert!(!c.pending);
        assert_eq!(c.review_id, None);
    }

    #[test]
    fn a_pending_comment_is_invisible_to_others_until_submit() {
        let root = temp_root("pending");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:reviewer@acme"))
            .unwrap();
        store
            .add_pending_comment(pending_comment(
                &batch.id,
                human("psn:reviewer@acme"),
                "draft note",
                200,
            ))
            .unwrap();

        let doc = store.load(&loc(), KEY).unwrap();
        let mine = doc.view_for("psn:reviewer@acme");
        assert_eq!(mine.threads.len(), 1, "author sees their own pending draft");
        assert_eq!(mine.reviews.len(), 1);
        let other = doc.view_for("psn:other@acme");
        assert_eq!(other.threads.len(), 0, "a pending comment must be invisible to others");
        assert_eq!(other.reviews.len(), 0, "a draft batch is hidden from others");

        store
            .submit_review(submitted_review(
                &batch.id,
                human("psn:reviewer@acme"),
                BatchVerdict::ChangesRequested,
                None,
                300,
            ))
            .unwrap();
        let doc = store.load(&loc(), KEY).unwrap();
        let other = doc.view_for("psn:other@acme");
        assert_eq!(other.threads.len(), 1, "submit makes the batch's comments public");
        assert_eq!(other.reviews[0].verdict, BatchVerdict::ChangesRequested);
        assert!(!other.reviews[0].advisory, "a human batch is not advisory");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn submit_emits_one_batch_event_and_is_idempotent() {
        let root = temp_root("onevent");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:reviewer@acme"))
            .unwrap();
        for i in 0..3 {
            store
                .add_pending_comment(pending_comment(
                    &batch.id,
                    human("psn:reviewer@acme"),
                    format!("note {i}"),
                    200 + i,
                ))
                .unwrap();
        }
        let first = store
            .submit_review(submitted_review(
                &batch.id,
                human("psn:reviewer@acme"),
                BatchVerdict::Approved,
                Some("LGTM".into()),
                400,
            ))
            .unwrap();
        let ev = first.expect("first submit yields exactly one batch event");
        assert_eq!(ev.comment_ids.len(), 3, "the ONE event carries the whole batch");
        assert_eq!(ev.review.verdict, BatchVerdict::Approved);
        assert_eq!(ev.review.summary_md.as_deref(), Some("LGTM"));
        let second = store
            .submit_review(submitted_review(
                &batch.id,
                human("psn:reviewer@acme"),
                BatchVerdict::Commented,
                None,
                500,
            ))
            .unwrap();
        assert!(second.is_none(), "a re-submit must NOT emit a second event");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_agent_batch_is_advisory() {
        let root = temp_root("agent");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(
                &loc(),
                KEY,
                ThreadPrincipal::plain(PrincipalRole::Agent, "ReviewBot@acme"),
            )
            .unwrap();
        assert!(batch.advisory, "an agent batch must be advisory");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discard_removes_a_draft_but_not_a_submitted_batch() {
        let root = temp_root("discard");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store.start_review(&loc(), KEY, human("psn:r@acme")).unwrap();
        store
            .add_pending_comment(pending_comment(
                &batch.id,
                human("psn:r@acme"),
                "draft",
                1,
            ))
            .unwrap();
        store
            .discard_review(&loc(), KEY, &batch.id, &human("psn:r@acme"))
            .unwrap();
        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(doc.threads.len(), 0, "discard removes the draft's threads");
        assert_eq!(doc.reviews.len(), 0);

        let b2 = store.start_review(&loc(), KEY, human("psn:r@acme")).unwrap();
        store
            .submit_review(submitted_review(
                &b2.id,
                human("psn:r@acme"),
                BatchVerdict::Commented,
                None,
                2,
            ))
            .unwrap();
        assert!(matches!(
            store.discard_review(&loc(), KEY, &b2.id, &human("psn:r@acme")),
            Err(DurableError::Forbidden(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_batch_is_not_submittable_discardable_or_appendable_by_a_non_author() {
        let root = temp_root("batch-owner");
        let store = DurablePrThreadStore::rooted(&root);
        let author = human("psn:author@acme");
        let attacker = human("psn:attacker@acme");
        let batch = store.start_review(&loc(), KEY, author.clone()).unwrap();
        store
            .add_pending_comment(pending_comment(
                &batch.id,
                author.clone(),
                "secret draft",
                1,
            ))
            .unwrap();

        assert!(matches!(
            store.submit_review(submitted_review(
                &batch.id,
                attacker.clone(),
                BatchVerdict::Approved,
                Some("forged".into()),
                2,
            )),
            Err(DurableError::Forbidden(_))
        ));
        assert!(matches!(
            store.discard_review(&loc(), KEY, &batch.id, &attacker),
            Err(DurableError::Forbidden(_))
        ));
        assert!(matches!(
            store.add_pending_comment(pending_comment(
                &batch.id,
                attacker.clone(),
                "injected",
                3,
            )),
            Err(DurableError::Forbidden(_))
        ));

        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(doc.view_for("psn:attacker@acme").reviews.len(), 0, "still hidden from the attacker");
        assert_eq!(doc.view_for("psn:author@acme").reviews.len(), 1, "author still owns their draft");
        let submitted = store
            .submit_review(submitted_review(
                &batch.id,
                author,
                BatchVerdict::Approved,
                None,
                4,
            ))
            .unwrap();
        assert!(submitted.is_some(), "the real author can still submit their own batch");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_thread_persists() {
        let root = temp_root("resolve");
        let store = DurablePrThreadStore::rooted(&root);
        let t = store
            .create_thread(&loc(), KEY, None, human("psn:a@acme"), "q?", 1)
            .unwrap();
        store.resolve_thread(&loc(), KEY, &t.id, true).unwrap();
        let doc = store.load(&loc(), KEY).unwrap();
        assert!(doc.threads[0].resolved);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn key_stem_sanitises_separators() {
        assert_eq!(key_stem("pr:core:42"), "pr_core_42");
        assert_eq!(key_stem("issue:PROJ-1"), "issue_PROJ-1");
        assert_eq!(key_stem("repo:team/app"), "repo_team_app");
        assert!(!key_stem("pr:../../etc").contains('/'));
    }

    #[test]
    fn lossy_filename_collision_cannot_cross_object_boundaries() {
        let root = temp_root("subject-key-collision");
        let store = DurablePrThreadStore::rooted(&root);
        let first = "pr:a/b";
        let colliding = "pr:a:b";
        assert_eq!(key_stem(first), key_stem(colliding), "the regression needs a collision");

        store
            .create_thread(&loc(), first, None, human("psn:a@acme"), "private", 1)
            .unwrap();
        let err = store
            .load(&loc(), colliding)
            .expect_err("a colliding filename must not expose another object's threads");
        assert!(err.to_string().contains("subject mismatch"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn id_sequence_exhaustion_is_loud() {
        let mut doc = SubjectThreads {
            seq: u64::MAX,
            ..Default::default()
        };
        let err = doc
            .next_id("c")
            .expect_err("the sequence must not wrap and reuse an existing id");
        assert!(err.to_string().contains("sequence exhausted"));
        assert_eq!(doc.seq, u64::MAX, "a refused allocation leaves state unchanged");
    }
}
