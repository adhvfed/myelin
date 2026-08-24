use std::collections::{BTreeMap, BTreeSet};
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
pub const MAX_CONVERSATION_COMMANDS_PER_SUBJECT: usize = 16_384;
pub const MAX_REVIEW_COMMANDS_PER_SUBJECT: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome<T> {
    pub value: T,
    pub applied: bool,
}

impl<T> CommandOutcome<T> {
    fn applied(value: T) -> Self {
        Self {
            value,
            applied: true,
        }
    }

    fn replayed(value: T) -> Self {
        Self {
            value,
            applied: false,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CommentWrite {
    author: ThreadPrincipal,
    body_md: String,
    operation_id: String,
    now: i64,
}

impl CommentWrite {
    pub fn new(
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        operation_nonce: &str,
        now: i64,
    ) -> Result<Self, DurableError> {
        Ok(Self {
            author,
            body_md: validate_markdown(body_md.into(), MAX_COMMENT_BODY_BYTES, "comment body")?,
            operation_id: operation_digest(operation_nonce)?,
            now,
        })
    }
}

impl core::fmt::Debug for CommentWrite {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CommentWrite")
            .field("author", &"<redacted>")
            .field("body_md", &"<redacted>")
            .field("operation_id", &self.operation_id)
            .field("now", &self.now)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PendingCommentRequest {
    repo: RepoLoc,
    object_key: String,
    review_id: String,
    anchor: Option<ThreadAnchor>,
    author: ThreadPrincipal,
    body_md: String,
    operation_id: String,
    request_hash: String,
    now: i64,
}

impl PendingCommentRequest {
    pub fn new(
        repo: RepoLoc,
        object_key: impl Into<String>,
        review_id: impl Into<String>,
        anchor: Option<ThreadAnchor>,
        comment: CommentWrite,
    ) -> Result<Self, DurableError> {
        let object_key = object_key.into();
        let review_id = review_id.into();
        let CommentWrite {
            author,
            body_md,
            operation_id,
            now,
        } = comment;
        validate_review_target(&repo, &object_key, &review_id)?;
        let request_hash = pending_comment_request_hash(&review_id, &anchor, &author, &body_md)?;
        Ok(Self {
            repo,
            object_key,
            review_id,
            anchor,
            author,
            body_md,
            operation_id,
            request_hash,
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
            .field("operation_id", &self.operation_id)
            .field("now", &self.now)
            .finish()
    }
}

fn operation_digest(value: &str) -> Result<String, DurableError> {
    if value.is_empty() || value.len() > 256 || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(DurableError::Git(
            "conversation idempotency key must contain 1..=256 printable ASCII bytes".into(),
        ));
    }
    Ok(digest_parts(
        "myelin.git.review-comment.operation.v1",
        &[value.as_bytes()],
    ))
}

fn thread_request_hash(
    anchor: &Option<ThreadAnchor>,
    author: &ThreadPrincipal,
    body_md: &str,
) -> Result<String, DurableError> {
    let anchor = encode_anchor_intent(anchor)?;
    let author = encode_author(author)?;
    Ok(digest_parts(
        "myelin.git.thread-create.request.v1",
        &[anchor.as_slice(), author.as_slice(), body_md.as_bytes()],
    ))
}

fn reply_request_hash(
    thread_id: &str,
    author: &ThreadPrincipal,
    body_md: &str,
) -> Result<String, DurableError> {
    let author = encode_author(author)?;
    Ok(digest_parts(
        "myelin.git.thread-reply.request.v1",
        &[thread_id.as_bytes(), author.as_slice(), body_md.as_bytes()],
    ))
}

fn resolve_thread_request_hash(thread_id: &str, resolved: bool) -> String {
    digest_parts(
        "myelin.git.thread-resolve.request.v1",
        &[thread_id.as_bytes(), &[u8::from(resolved)]],
    )
}

fn pending_comment_request_hash(
    review_id: &str,
    anchor: &Option<ThreadAnchor>,
    author: &ThreadPrincipal,
    body_md: &str,
) -> Result<String, DurableError> {
    let anchor = encode_anchor_intent(anchor)?;
    let author = encode_author(author)?;
    Ok(digest_parts(
        "myelin.git.review-comment.request.v1",
        &[
            review_id.as_bytes(),
            anchor.as_slice(),
            author.as_slice(),
            body_md.as_bytes(),
        ],
    ))
}

fn review_start_request_hash(reviewer: &ThreadPrincipal) -> Result<String, DurableError> {
    let reviewer = encode_author(reviewer)?;
    Ok(digest_parts(
        "myelin.git.review-start.request.v1",
        &[reviewer.as_slice()],
    ))
}

fn review_submit_request_hash(
    review_id: &str,
    actor: &ThreadPrincipal,
    verdict: BatchVerdict,
    summary_md: &Option<String>,
) -> Result<String, DurableError> {
    let actor = encode_author(actor)?;
    let summary = serde_json::to_vec(summary_md)
        .map_err(|_| DurableError::Git("encode review summary".into()))?;
    Ok(digest_parts(
        "myelin.git.review-submit.request.v1",
        &[
            review_id.as_bytes(),
            actor.as_slice(),
            verdict.as_str().as_bytes(),
            summary.as_slice(),
        ],
    ))
}

fn review_discard_request_hash(
    review_id: &str,
    actor: &ThreadPrincipal,
) -> Result<String, DurableError> {
    let actor = encode_author(actor)?;
    Ok(digest_parts(
        "myelin.git.review-discard.request.v1",
        &[review_id.as_bytes(), actor.as_slice()],
    ))
}

fn encode_anchor_intent(anchor: &Option<ThreadAnchor>) -> Result<Vec<u8>, DurableError> {
    let intent = anchor
        .as_ref()
        .map(|anchor| (&anchor.path, anchor.line, anchor.side));
    serde_json::to_vec(&intent).map_err(|_| DurableError::Git("encode comment anchor".into()))
}

fn encode_author(author: &ThreadPrincipal) -> Result<Vec<u8>, DurableError> {
    serde_json::to_vec(author).map_err(|_| DurableError::Git("encode comment author".into()))
}

fn digest_parts(domain: &str, parts: &[&[u8]]) -> String {
    let mut digest = blake3::Hasher::new();
    digest.update(domain.as_bytes());
    for part in parts {
        digest.update(&(part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    digest.finalize().to_hex().to_string()
}

fn canonical_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReviewDecision {
    verdict: BatchVerdict,
    summary_md: Option<String>,
}

impl core::fmt::Debug for ReviewDecision {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReviewDecision")
            .field("verdict", &self.verdict)
            .field(
                "summary_md",
                &self.summary_md.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl ReviewDecision {
    pub fn new(verdict: BatchVerdict, summary_md: Option<String>) -> Result<Self, DurableError> {
        let summary_md = summary_md
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| validate_markdown(summary, MAX_REVIEW_SUMMARY_BYTES, "review summary"))
            .transpose()?;
        Ok(Self {
            verdict,
            summary_md,
        })
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
    operation_id: String,
    request_hash: String,
    now: i64,
}

impl SubmitReviewRequest {
    pub fn new(
        repo: RepoLoc,
        object_key: impl Into<String>,
        review_id: impl Into<String>,
        actor: ThreadPrincipal,
        decision: ReviewDecision,
        operation_nonce: &str,
        now: i64,
    ) -> Result<Self, DurableError> {
        let object_key = object_key.into();
        let review_id = review_id.into();
        let ReviewDecision {
            verdict,
            summary_md,
        } = decision;
        validate_review_target(&repo, &object_key, &review_id)?;
        let operation_id = operation_digest(operation_nonce)?;
        let request_hash = review_submit_request_hash(&review_id, &actor, verdict, &summary_md)?;
        Ok(Self {
            repo,
            object_key,
            review_id,
            actor,
            verdict,
            summary_md,
            operation_id,
            request_hash,
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
            .field("operation_id", &self.operation_id)
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
    #[serde(default, alias = "pending_comment_commands")]
    conversation_commands: BTreeMap<String, ConversationCommand>,
    #[serde(default)]
    review_commands: BTreeMap<String, ReviewCommand>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ConversationCommand {
    request_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved: Option<bool>,
}

impl ConversationCommand {
    fn created_thread(request_hash: String, thread_id: String) -> Self {
        Self {
            request_hash,
            thread_id: Some(thread_id),
            comment_id: None,
            resolved: None,
        }
    }

    fn created_comment(request_hash: String, comment_id: String) -> Self {
        Self {
            request_hash,
            thread_id: None,
            comment_id: Some(comment_id),
            resolved: None,
        }
    }

    fn resolved_thread(request_hash: String, thread_id: String, resolved: bool) -> Self {
        Self {
            request_hash,
            thread_id: Some(thread_id),
            comment_id: None,
            resolved: Some(resolved),
        }
    }

    fn has_valid_result(&self) -> bool {
        matches!(
            (
                self.thread_id.is_some(),
                self.comment_id.is_some(),
                self.resolved.is_some(),
            ),
            (true, false, false) | (false, true, false) | (true, false, true)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewCommandKind {
    Start,
    DiscardedStart,
    Submit,
    Discard,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReviewCommand {
    request_hash: String,
    kind: ReviewCommandKind,
    review_id: String,
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

    fn validate(&self) -> Result<(), DurableError> {
        if self.threads.len() > MAX_THREADS_PER_SUBJECT
            || self.comment_count() > MAX_COMMENTS_PER_SUBJECT
            || self.reviews.len() > MAX_REVIEWS_PER_SUBJECT
            || self.conversation_commands.len() > MAX_CONVERSATION_COMMANDS_PER_SUBJECT
            || self.review_commands.len() > MAX_REVIEW_COMMANDS_PER_SUBJECT
        {
            return Err(DurableError::Git(
                "PR conversation exceeds its cardinality limit".into(),
            ));
        }
        if self
            .conversation_commands
            .iter()
            .any(|(operation_id, command)| {
                !canonical_digest(operation_id)
                    || !canonical_digest(&command.request_hash)
                    || !command.has_valid_result()
            })
        {
            return Err(DurableError::Git(
                "PR conversation contains a malformed command receipt".into(),
            ));
        }
        if self.review_commands.iter().any(|(operation_id, command)| {
            let target_exists = self.review(&command.review_id).is_some();
            !canonical_digest(operation_id)
                || !canonical_digest(&command.request_hash)
                || match command.kind {
                    ReviewCommandKind::Start | ReviewCommandKind::Submit => !target_exists,
                    ReviewCommandKind::DiscardedStart | ReviewCommandKind::Discard => target_exists,
                }
        }) {
            return Err(DurableError::Git(
                "PR conversation contains a malformed review command receipt".into(),
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

    /// Returns only the requested comments visible to this viewer. A pending review comment is
    /// private to its author until submission; removed comments retain their address but not useful
    /// content. The containing thread state travels with the comment so callers do not have to
    /// reconstruct conversation semantics.
    pub fn comments_for(
        &self,
        viewer_display: &str,
        requested_ids: &BTreeSet<String>,
    ) -> Vec<ViewedComment> {
        if requested_ids.is_empty() {
            return Vec::new();
        }
        self.threads
            .iter()
            .flat_map(|thread| {
                thread
                    .comments
                    .iter()
                    .filter(|comment| {
                        requested_ids.contains(&comment.id)
                            && (!comment.pending || comment.author.display == viewer_display)
                    })
                    .map(|comment| ViewedComment {
                        comment: comment.clone(),
                        thread_id: thread.id.clone(),
                        anchor: thread.anchor.clone(),
                        resolved: thread.resolved,
                    })
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewedThreads {
    pub threads: Vec<ThreadRecord>,
    pub reviews: Vec<ReviewBatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewedComment {
    pub comment: CommentRecord,
    pub thread_id: String,
    pub anchor: Option<ThreadAnchor>,
    pub resolved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedBatch {
    pub review: ReviewBatch,
    pub comment_ids: Vec<String>,
}

fn submitted_batch(
    document: &SubjectThreads,
    review_id: &str,
) -> Result<SubmittedBatch, DurableError> {
    let review = document.review(review_id).cloned().ok_or_else(|| {
        DurableError::Io("review-submit idempotency receipt references a missing review".into())
    })?;
    if review.is_draft() {
        return Err(DurableError::Io(
            "review-submit idempotency receipt references a draft review".into(),
        ));
    }
    let comment_ids = document
        .threads
        .iter()
        .flat_map(|thread| thread.comments.iter())
        .filter(|comment| comment.review_id.as_deref() == Some(review_id))
        .map(|comment| comment.id.clone())
        .collect();
    Ok(SubmittedBatch {
        review,
        comment_ids,
    })
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
                doc.validate().map_err(|error| {
                    DurableError::Io(format!("invalid stored PR conversation: {error}"))
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
        doc.validate()?;
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
        comment: CommentWrite,
    ) -> Result<CommandOutcome<ThreadRecord>, DurableError> {
        let CommentWrite {
            author,
            body_md,
            operation_id,
            now,
        } = comment;
        let request_hash = thread_request_hash(&anchor, &author, &body_md)?;
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if let Some(command) = doc.conversation_commands.get(&operation_id) {
            if command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different conversation write".into(),
                ));
            }
            let thread_id = command.thread_id.as_deref().ok_or_else(|| {
                DurableError::Io("thread idempotency receipt contains the wrong result type".into())
            })?;
            let thread = doc
                .threads
                .iter()
                .find(|thread| thread.id == thread_id)
                .cloned()
                .ok_or_else(|| {
                    DurableError::Io(
                        "thread idempotency receipt references a missing thread".into(),
                    )
                })?;
            return Ok(CommandOutcome::replayed(thread));
        }
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
        doc.conversation_commands.insert(
            operation_id,
            ConversationCommand::created_thread(request_hash, thread.id.clone()),
        );
        self.save(repo, &doc)?;
        Ok(CommandOutcome::applied(thread))
    }

    pub fn add_comment(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        thread_id: &str,
        comment: CommentWrite,
    ) -> Result<CommandOutcome<CommentRecord>, DurableError> {
        let CommentWrite {
            author,
            body_md,
            operation_id,
            now,
        } = comment;
        let request_hash = reply_request_hash(thread_id, &author, &body_md)?;
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if let Some(command) = doc.conversation_commands.get(&operation_id) {
            if command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different conversation write".into(),
                ));
            }
            let comment_id = command.comment_id.as_deref().ok_or_else(|| {
                DurableError::Io(
                    "comment idempotency receipt contains the wrong result type".into(),
                )
            })?;
            let comment = doc
                .threads
                .iter()
                .flat_map(|thread| thread.comments.iter())
                .find(|comment| comment.id == comment_id)
                .cloned()
                .ok_or_else(|| {
                    DurableError::Io(
                        "comment idempotency receipt references a missing comment".into(),
                    )
                })?;
            return Ok(CommandOutcome::replayed(comment));
        }
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
        doc.conversation_commands.insert(
            operation_id,
            ConversationCommand::created_comment(request_hash, comment.id.clone()),
        );
        self.save(repo, &doc)?;
        Ok(CommandOutcome::applied(comment))
    }

    pub fn resolve_thread(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        thread_id: &str,
        resolved: bool,
        operation_nonce: &str,
    ) -> Result<CommandOutcome<bool>, DurableError> {
        let operation_id = operation_digest(operation_nonce)?;
        let request_hash = resolve_thread_request_hash(thread_id, resolved);
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if let Some(command) = doc.conversation_commands.get(&operation_id) {
            if command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different conversation write".into(),
                ));
            }
            let receipt_thread_id = command.thread_id.as_deref().ok_or_else(|| {
                DurableError::Io("thread-resolution receipt contains the wrong result type".into())
            })?;
            if receipt_thread_id != thread_id {
                return Err(DurableError::Io(
                    "thread-resolution receipt references the wrong thread".into(),
                ));
            }
            let receipt = command.resolved.ok_or_else(|| {
                DurableError::Io("thread-resolution receipt is missing its state".into())
            })?;
            return Ok(CommandOutcome::replayed(receipt));
        }
        if doc.conversation_commands.len() >= MAX_CONVERSATION_COMMANDS_PER_SUBJECT {
            return Err(DurableError::Git(
                "PR conversation exceeds its command limit".into(),
            ));
        }
        let thread = doc
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| DurableError::NotFound(format!("thread {thread_id}")))?;
        thread.resolved = resolved;
        doc.conversation_commands.insert(
            operation_id,
            ConversationCommand::resolved_thread(request_hash, thread_id.to_string(), resolved),
        );
        self.save(repo, &doc)?;
        Ok(CommandOutcome::applied(resolved))
    }

    pub fn start_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        reviewer: ThreadPrincipal,
        operation_nonce: &str,
    ) -> Result<ReviewBatch, DurableError> {
        let operation_id = operation_digest(operation_nonce)?;
        let request_hash = review_start_request_hash(&reviewer)?;
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if let Some(command) = doc.review_commands.get(&operation_id) {
            if command.kind != ReviewCommandKind::Start || command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different review command".into(),
                ));
            }
            let review = doc.review(&command.review_id).cloned().ok_or_else(|| {
                DurableError::Io(
                    "review-start idempotency receipt references a missing review".into(),
                )
            })?;
            return Ok(review);
        }
        // Leave room for the terminal discard receipt. Otherwise a caller could fill the ledger
        // with equivalent start keys and make its own private draft impossible to remove safely.
        if doc.review_commands.len() >= MAX_REVIEW_COMMANDS_PER_SUBJECT - 1 {
            return Err(DurableError::Git(
                "PR conversation exceeds its review-command limit".into(),
            ));
        }
        if let Some(existing) = doc
            .reviews
            .iter()
            .find(|review| review.is_draft() && review.reviewer.display == reviewer.display)
            .cloned()
        {
            doc.review_commands.insert(
                operation_id,
                ReviewCommand {
                    request_hash,
                    kind: ReviewCommandKind::Start,
                    review_id: existing.id.clone(),
                },
            );
            self.save(repo, &doc)?;
            return Ok(existing);
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
        doc.review_commands.insert(
            operation_id,
            ReviewCommand {
                request_hash,
                kind: ReviewCommandKind::Start,
                review_id: batch.id.clone(),
            },
        );
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
            operation_id,
            request_hash,
            now,
        } = request;
        let lock = self.subject_lock(&repo, &object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(&repo, &object_key)?;
        if let Some(command) = doc.conversation_commands.get(&operation_id) {
            if command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different review comment".into(),
                ));
            }
            let comment_id = command.comment_id.as_deref().ok_or_else(|| {
                DurableError::Io(
                    "review comment idempotency receipt contains the wrong result type".into(),
                )
            })?;
            return doc
                .threads
                .iter()
                .flat_map(|thread| thread.comments.iter())
                .find(|comment| comment.id == comment_id)
                .cloned()
                .ok_or_else(|| {
                    DurableError::Io(
                        "review comment idempotency receipt references a missing comment".into(),
                    )
                });
        }
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
        doc.conversation_commands.insert(
            operation_id,
            ConversationCommand::created_comment(request_hash, comment.id.clone()),
        );
        self.save(&repo, &doc)?;
        Ok(comment)
    }

    pub fn submit_review(
        &self,
        request: SubmitReviewRequest,
    ) -> Result<CommandOutcome<Option<SubmittedBatch>>, DurableError> {
        let SubmitReviewRequest {
            repo,
            object_key,
            review_id,
            actor,
            verdict,
            summary_md,
            operation_id,
            request_hash,
            now,
        } = request;
        let lock = self.subject_lock(&repo, &object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(&repo, &object_key)?;
        if let Some(command) = doc.review_commands.get(&operation_id) {
            if command.kind != ReviewCommandKind::Submit || command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different review command".into(),
                ));
            }
            let submitted = submitted_batch(&doc, &command.review_id)?;
            return Ok(CommandOutcome::replayed(Some(submitted)));
        }
        if doc.review_commands.len() >= MAX_REVIEW_COMMANDS_PER_SUBJECT {
            return Err(DurableError::Git(
                "PR conversation exceeds its review-command limit".into(),
            ));
        }
        let review_index = doc
            .reviews
            .iter()
            .position(|review| review.id == review_id)
            .ok_or_else(|| DurableError::NotFound(format!("review {review_id}")))?;
        let owner_display = doc.reviews[review_index].reviewer.display.clone();
        let already = !doc.reviews[review_index].is_draft();
        if owner_display != actor.display {
            return Err(DurableError::Forbidden(format!(
                "review {review_id} belongs to another reviewer"
            )));
        }
        if already {
            return Ok(CommandOutcome::replayed(None));
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
            let r = &mut doc.reviews[review_index];
            r.verdict = verdict;
            r.summary_md = summary_md;
            r.submitted_at = Some(now);
            r.clone()
        };
        let submitted = SubmittedBatch {
            review,
            comment_ids,
        };
        doc.review_commands.insert(
            operation_id,
            ReviewCommand {
                request_hash,
                kind: ReviewCommandKind::Submit,
                review_id,
            },
        );
        self.save(&repo, &doc)?;
        Ok(CommandOutcome::applied(Some(submitted)))
    }

    pub fn discard_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        review_id: &str,
        actor: &ThreadPrincipal,
        operation_nonce: &str,
    ) -> Result<CommandOutcome<String>, DurableError> {
        let operation_id = operation_digest(operation_nonce)?;
        let request_hash = review_discard_request_hash(review_id, actor)?;
        let lock = self.subject_lock(repo, object_key)?;
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut doc = self.load(repo, object_key)?;
        if let Some(command) = doc.review_commands.get(&operation_id) {
            if command.kind != ReviewCommandKind::Discard || command.request_hash != request_hash {
                return Err(DurableError::Conflict(
                    "idempotency key is already bound to a different review command".into(),
                ));
            }
            return Ok(CommandOutcome::replayed(command.review_id.clone()));
        }
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
        let discarded_comment_ids: BTreeSet<String> = doc
            .threads
            .iter()
            .flat_map(|thread| thread.comments.iter())
            .filter(|comment| comment.review_id.as_deref() == Some(review_id))
            .map(|comment| comment.id.clone())
            .collect();
        for t in &mut doc.threads {
            t.comments
                .retain(|c| c.review_id.as_deref() != Some(review_id));
        }
        doc.threads.retain(|t| !t.comments.is_empty());
        doc.reviews.retain(|r| r.id != review_id);
        doc.conversation_commands.retain(|_, command| {
            command
                .comment_id
                .as_ref()
                .is_none_or(|comment_id| !discarded_comment_ids.contains(comment_id))
        });
        doc.review_commands.retain(|_, command| {
            command.review_id != review_id || command.kind == ReviewCommandKind::Start
        });
        for command in doc
            .review_commands
            .values_mut()
            .filter(|command| command.review_id == review_id)
        {
            command.kind = ReviewCommandKind::DiscardedStart;
        }
        doc.review_commands.insert(
            operation_id,
            ReviewCommand {
                request_hash,
                kind: ReviewCommandKind::Discard,
                review_id: review_id.to_string(),
            },
        );
        self.save(repo, &doc)?;
        Ok(CommandOutcome::applied(review_id.to_string()))
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

    fn comment(
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        operation_nonce: &str,
        now: i64,
    ) -> CommentWrite {
        CommentWrite::new(author, body_md, operation_nonce, now).unwrap()
    }

    fn pending_comment(
        review_id: &str,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> PendingCommentRequest {
        PendingCommentRequest::new(
            loc(),
            KEY,
            review_id,
            None,
            comment(author, body_md, &format!("operation-{now}"), now),
        )
        .unwrap()
    }

    fn submitted_review(
        review_id: &str,
        actor: ThreadPrincipal,
        verdict: BatchVerdict,
        summary_md: Option<String>,
        now: i64,
    ) -> SubmitReviewRequest {
        SubmitReviewRequest::new(
            loc(),
            KEY,
            review_id,
            actor,
            ReviewDecision::new(verdict, summary_md).unwrap(),
            &format!("submit-{now}"),
            now,
        )
        .unwrap()
    }

    #[test]
    fn review_requests_validate_targets_and_redact_authors_and_content() {
        let pending = PendingCommentRequest::new(
            loc(),
            KEY,
            "r-secret",
            None,
            comment(
                human("psn:secret-author@acme"),
                "sensitive draft body",
                "secret-operation",
                1,
            ),
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
            ReviewDecision::new(BatchVerdict::Approved, Some("sensitive summary".into())).unwrap(),
            "secret-submit-operation",
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
            comment(human("psn:r@acme"), "draft", "incomplete-target", 3),
        )
        .is_err());
        assert!(SubmitReviewRequest::new(
            loc(),
            " ",
            "r-1",
            human("psn:r@acme"),
            ReviewDecision::new(BatchVerdict::Commented, None).unwrap(),
            "incomplete-submit-target",
            4,
        )
        .is_err());
        assert!(CommentWrite::new(
            human("psn:r@acme"),
            "x".repeat(MAX_COMMENT_BODY_BYTES + 1),
            "oversized-comment",
            5,
        )
        .is_err());
        assert!(ReviewDecision::new(
            BatchVerdict::Commented,
            Some("x".repeat(MAX_REVIEW_SUMMARY_BYTES + 1)),
        )
        .is_err());
    }

    #[test]
    fn repeated_review_start_reuses_the_reviewers_active_draft() {
        let root = temp_root("one-active-draft");
        let store = DurablePrThreadStore::rooted(&root);
        let reviewer = human("psn:reviewer@acme");

        let first = store
            .start_review(&loc(), KEY, reviewer.clone(), "start-once")
            .unwrap();
        let retry = store
            .start_review(&loc(), KEY, reviewer, "start-once")
            .unwrap();

        assert_eq!(retry.id, first.id);
        assert_eq!(store.load(&loc(), KEY).unwrap().reviews.len(), 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn retried_pending_comment_returns_its_original_receipt_without_a_duplicate() {
        let root = temp_root("pending-comment-retry");
        let store = DurablePrThreadStore::rooted(&root);
        let reviewer = human("psn:reviewer@acme");
        let review = store
            .start_review(&loc(), KEY, reviewer.clone(), "pending-review")
            .unwrap();

        let request = |body: &str, now| {
            PendingCommentRequest::new(
                loc(),
                KEY,
                &review.id,
                None,
                comment(reviewer.clone(), body, "private-retry-key", now),
            )
            .unwrap()
        };
        let first = store
            .add_pending_comment(request("One durable observation.", 100))
            .unwrap();
        let replayed = DurablePrThreadStore::rooted(&root)
            .add_pending_comment(request("One durable observation.", 999))
            .unwrap();

        assert_eq!(
            replayed, first,
            "a retry returns the first timestamp and id"
        );
        let stored = store.load(&loc(), KEY).unwrap();
        assert_eq!(stored.threads.len(), 1);
        assert_eq!(stored.comment_count(), 1, "one intent leaves one comment");
        assert!(matches!(
            store.add_pending_comment(request("Different work.", 1_000)),
            Err(DurableError::Conflict(_))
        ));
        let document = std::fs::read_to_string(store.subject_path(&loc(), KEY).unwrap()).unwrap();
        assert!(
            !document.contains("private-retry-key"),
            "raw caller keys never enter the review document"
        );
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
            .create_thread(
                &loc(),
                KEY,
                None,
                comment(human("psn:a@acme"), "first post", "round-trip", 100),
            )
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
    fn discussion_writes_converge_on_the_first_durable_result() {
        let root = temp_root("discussion-retries");
        let store = DurablePrThreadStore::rooted(&root);
        let author = human("psn:a@acme");
        let first = store
            .create_thread(
                &loc(),
                KEY,
                None,
                comment(
                    author.clone(),
                    "Can we make this invariant explicit?",
                    "private-thread-key",
                    100,
                ),
            )
            .unwrap();
        let retried = store
            .create_thread(
                &loc(),
                KEY,
                None,
                comment(
                    author.clone(),
                    "Can we make this invariant explicit?",
                    "private-thread-key",
                    999,
                ),
            )
            .unwrap();
        assert!(first.applied);
        assert!(!retried.applied);
        assert_eq!(retried.value, first.value);
        assert!(matches!(
            store.create_thread(
                &loc(),
                KEY,
                None,
                comment(
                    author.clone(),
                    "A different thread.",
                    "private-thread-key",
                    1_000,
                ),
            ),
            Err(DurableError::Conflict(_))
        ));

        let first_reply = store
            .add_comment(
                &loc(),
                KEY,
                &first.value.id,
                comment(
                    author.clone(),
                    "Yes; the durable boundary is the right place.",
                    "private-reply-key",
                    101,
                ),
            )
            .unwrap();
        let retried_reply = DurablePrThreadStore::rooted(&root)
            .add_comment(
                &loc(),
                KEY,
                &first.value.id,
                comment(
                    author,
                    "Yes; the durable boundary is the right place.",
                    "private-reply-key",
                    1_001,
                ),
            )
            .unwrap();
        assert!(first_reply.applied);
        assert!(!retried_reply.applied);
        assert_eq!(retried_reply.value, first_reply.value);

        let document = std::fs::read_to_string(store.subject_path(&loc(), KEY).unwrap()).unwrap();
        assert!(!document.contains("private-thread-key"));
        assert!(!document.contains("private-reply-key"));
        let stored = store.load(&loc(), KEY).unwrap();
        assert_eq!(stored.threads.len(), 1, "one question leaves one thread");
        assert_eq!(
            stored.comment_count(),
            2,
            "one answer leaves one additional comment"
        );
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
                    comment(
                        human(&format!("psn:writer-{writer}@acme")),
                        format!("comment-{writer}"),
                        &format!("writer-{writer}"),
                        writer as i64,
                    ),
                )
            }));
        }
        for handle in handles {
            handle
                .join()
                .expect("writer must not panic")
                .expect("writer must persist");
        }

        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(
            doc.threads.len(),
            WRITERS,
            "no concurrent write may be lost"
        );
        assert_eq!(
            doc.seq,
            (WRITERS * 2) as u64,
            "thread and comment ids stay monotonic"
        );
        let bodies: std::collections::BTreeSet<_> = doc
            .threads
            .iter()
            .map(|thread| thread.comments[0].body_md.as_str())
            .collect();
        assert_eq!(
            bodies.len(),
            WRITERS,
            "every writer remains distinguishable"
        );
        drop(store);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn subject_lock_registry_prunes_inactive_subjects() {
        let root = temp_root("subject-lock-registry");
        let store = DurablePrThreadStore::rooted(&root);

        let first = store.subject_lock(&loc(), "pr:core:1").unwrap();
        let same = store.subject_lock(&loc(), "pr:core:1").unwrap();
        assert!(
            Arc::ptr_eq(&first, &same),
            "overlapping operations share one lock"
        );
        drop(first);
        drop(same);

        for number in 2..=2_000 {
            let lock = store
                .subject_lock(&loc(), &format!("pr:core:{number}"))
                .unwrap();
            drop(lock);
        }

        let locks = store
            .subject_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assert_eq!(locks.len(), 1, "dead subject keys must not accumulate");
        assert_eq!(
            locks
                .values()
                .filter(|weak| weak.strong_count() > 0)
                .count(),
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
    fn pending_comment_receipts_upgrade_into_the_shared_command_ledger() {
        let stored = serde_json::json!({
            "object_key": "pr:core:1",
            "threads": [{
                "id": "t-1",
                "comments": [{
                    "id": "c-1",
                    "author": { "kind": "human", "display": "psn:a@acme" },
                    "body_md": "A pending observation.",
                    "created_at": 1,
                    "review_id": "r-1",
                    "pending": true
                }]
            }],
            "pending_comment_commands": {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": {
                    "request_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "comment_id": "c-1"
                }
            }
        });

        let document: SubjectThreads =
            serde_json::from_value(stored).expect("the previous receipt shape remains readable");
        document.validate().expect("the migrated receipt is valid");
        let upgraded = serde_json::to_value(document).unwrap();
        assert!(upgraded.get("pending_comment_commands").is_none());
        assert_eq!(
            upgraded["conversation_commands"]
                ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["comment_id"],
            "c-1"
        );
    }

    #[test]
    fn a_pending_comment_is_invisible_to_others_until_submit() {
        let root = temp_root("pending");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:reviewer@acme"), "private-review")
            .unwrap();
        let pending = store
            .add_pending_comment(pending_comment(
                &batch.id,
                human("psn:reviewer@acme"),
                "draft note",
                200,
            ))
            .unwrap();

        let doc = store.load(&loc(), KEY).unwrap();
        let requested = BTreeSet::from([pending.id.clone(), "c-absent".into()]);
        assert_eq!(
            doc.comments_for("psn:reviewer@acme", &requested),
            vec![ViewedComment {
                comment: pending.clone(),
                thread_id: doc.threads[0].id.clone(),
                anchor: doc.threads[0].anchor.clone(),
                resolved: false,
            }],
            "an exact owner lookup returns the pending comment without scanning it into the result"
        );
        assert!(doc.comments_for("psn:other@acme", &requested).is_empty());
        let mine = doc.view_for("psn:reviewer@acme");
        assert_eq!(mine.threads.len(), 1, "author sees their own pending draft");
        assert_eq!(mine.reviews.len(), 1);
        let other = doc.view_for("psn:other@acme");
        assert_eq!(
            other.threads.len(),
            0,
            "a pending comment must be invisible to others"
        );
        assert_eq!(
            other.reviews.len(),
            0,
            "a draft batch is hidden from others"
        );

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
        assert_eq!(
            doc.comments_for("psn:other@acme", &requested).len(),
            1,
            "submission makes the exact comment visible"
        );
        let other = doc.view_for("psn:other@acme");
        assert_eq!(
            other.threads.len(),
            1,
            "submit makes the batch's comments public"
        );
        assert_eq!(other.reviews[0].verdict, BatchVerdict::ChangesRequested);
        assert!(!other.reviews[0].advisory, "a human batch is not advisory");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn review_commands_return_their_original_receipts_after_submission() {
        let root = temp_root("onevent");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:reviewer@acme"), "one-event-review")
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
        let submission = |verdict, summary_md, now| {
            SubmitReviewRequest::new(
                loc(),
                KEY,
                &batch.id,
                human("psn:reviewer@acme"),
                ReviewDecision::new(verdict, summary_md).unwrap(),
                "submit-once",
                now,
            )
            .unwrap()
        };
        let first = store
            .submit_review(submission(BatchVerdict::Approved, Some("LGTM".into()), 400))
            .unwrap();
        assert!(first.applied);
        let ev = first
            .value
            .expect("first submit yields exactly one batch event");
        assert_eq!(
            ev.comment_ids.len(),
            3,
            "the ONE event carries the whole batch"
        );
        assert_eq!(ev.review.verdict, BatchVerdict::Approved);
        assert_eq!(ev.review.summary_md.as_deref(), Some("LGTM"));
        let replayed = DurablePrThreadStore::rooted(&root)
            .submit_review(submission(BatchVerdict::Approved, Some("LGTM".into()), 999))
            .unwrap();
        assert!(!replayed.applied);
        assert_eq!(replayed.value, Some(ev.clone()));
        assert!(matches!(
            store.submit_review(submission(BatchVerdict::Commented, None, 500)),
            Err(DurableError::Conflict(_))
        ));
        assert_eq!(
            store
                .start_review(&loc(), KEY, human("psn:reviewer@acme"), "one-event-review",)
                .unwrap()
                .id,
            batch.id,
            "a response-loss retry must not open a new draft after submit"
        );
        assert_eq!(store.load(&loc(), KEY).unwrap().reviews.len(), 1);
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
                "agent-review",
            )
            .unwrap();
        assert!(batch.advisory, "an agent batch must be advisory");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discard_returns_its_receipt_after_removing_a_private_draft() {
        let root = temp_root("discard");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:r@acme"), "discarded-review")
            .unwrap();
        store
            .add_pending_comment(pending_comment(&batch.id, human("psn:r@acme"), "draft", 1))
            .unwrap();
        let first = store
            .discard_review(&loc(), KEY, &batch.id, &human("psn:r@acme"), "discard-once")
            .unwrap();
        assert!(first.applied);
        assert_eq!(first.value, batch.id);
        let replayed = DurablePrThreadStore::rooted(&root)
            .discard_review(&loc(), KEY, &batch.id, &human("psn:r@acme"), "discard-once")
            .unwrap();
        assert!(!replayed.applied);
        assert_eq!(replayed.value, batch.id);
        assert!(matches!(
            store.start_review(&loc(), KEY, human("psn:r@acme"), "discarded-review",),
            Err(DurableError::Conflict(_))
        ));
        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(doc.threads.len(), 0, "discard removes the draft's threads");
        assert_eq!(doc.reviews.len(), 0);

        let b2 = store
            .start_review(&loc(), KEY, human("psn:r@acme"), "submitted-review")
            .unwrap();
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
            store.discard_review(
                &loc(),
                KEY,
                &b2.id,
                &human("psn:r@acme"),
                "cannot-discard-submitted",
            ),
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
        let batch = store
            .start_review(&loc(), KEY, author.clone(), "owned-review")
            .unwrap();
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
            store.discard_review(&loc(), KEY, &batch.id, &attacker, "attacker-discard"),
            Err(DurableError::Forbidden(_))
        ));
        assert!(matches!(
            store.add_pending_comment(pending_comment(&batch.id, attacker.clone(), "injected", 3,)),
            Err(DurableError::Forbidden(_))
        ));

        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(
            doc.view_for("psn:attacker@acme").reviews.len(),
            0,
            "still hidden from the attacker"
        );
        assert_eq!(
            doc.view_for("psn:author@acme").reviews.len(),
            1,
            "author still owns their draft"
        );
        let submitted = store
            .submit_review(submitted_review(
                &batch.id,
                author,
                BatchVerdict::Approved,
                None,
                4,
            ))
            .unwrap();
        assert!(
            submitted.value.is_some(),
            "the real author can still submit their own batch"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_thread_persists_and_replays_its_original_receipt() {
        let root = temp_root("resolve");
        let store = DurablePrThreadStore::rooted(&root);
        let t = store
            .create_thread(
                &loc(),
                KEY,
                None,
                comment(human("psn:a@acme"), "q?", "resolve-thread", 1),
            )
            .unwrap()
            .value;
        let first = store
            .resolve_thread(&loc(), KEY, &t.id, true, "resolve-once")
            .unwrap();
        assert_eq!(first, CommandOutcome::applied(true));
        let replayed = DurablePrThreadStore::rooted(&root)
            .resolve_thread(&loc(), KEY, &t.id, true, "resolve-once")
            .unwrap();
        assert_eq!(replayed, CommandOutcome::replayed(true));
        assert!(matches!(
            store.resolve_thread(&loc(), KEY, &t.id, false, "resolve-once"),
            Err(DurableError::Conflict(_))
        ));
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
        assert_eq!(
            key_stem(first),
            key_stem(colliding),
            "the regression needs a collision"
        );

        store
            .create_thread(
                &loc(),
                first,
                None,
                comment(human("psn:a@acme"), "private", "filename-collision", 1),
            )
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
        assert_eq!(
            doc.seq,
            u64::MAX,
            "a refused allocation leaves state unchanged"
        );
    }
}
