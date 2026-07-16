//! # `pr_threads` — the DURABLE review-thread / comment / review-batch store (R3.3 · shared by R3.2)
//!
//! The ONE canonical conversation store both R3 packs consume (the `_gate.md` §02/§03 cross-pack
//! reconciliation, BINDING):
//!
//! - the canonical model is **threads** — a [`ThreadRecord`] has an OPTIONAL content
//!   [`ThreadAnchor`] (a diff-line anchor); comments belong to threads. The PR overview's
//!   "discussion" is exactly the set of threads with `anchor == None`; the diff pack's line-anchored
//!   threads are the ones with `Some(anchor)`. One store, one shape, two faces.
//! - **review batching** layers on via a `review_id` carried on each comment + a [`ReviewBatch`]
//!   lifecycle ([`start_review`](DurablePrThreadStore::start_review) →
//!   [`add_pending_comment`](DurablePrThreadStore::add_pending_comment) →
//!   [`submit_review`](DurablePrThreadStore::submit_review) / [`discard_review`]). A batch's pending
//!   comments are **visible ONLY to their author** until submit ([`SubjectThreads::view_for`] filters
//!   by construction — a non-author never sees another reviewer's unsubmitted draft).
//! - **submit emits ONE batch event** (R-BATCH-1): [`submit_review`] returns `Some(SubmittedBatch)`
//!   exactly once (the first submit); a re-submit is idempotent `None` — so the wire emits one event
//!   per batch, never one-per-comment and never a double on retry. (The wire `git.review.submitted`
//!   emit itself is the GIT-P16 outbox-emit floor; this store produces the single authoritative batch
//!   the emit carries.)
//!
//! ## Storage — keyed by the canonical type-qualified `object_key` (R2.2 grammar)
//! Threads are persisted per SUBJECT, keyed by the canonical [`myelin_refs::object_key`] tuple key
//! (`pr:<slug>:<n>` today; `issue:<id>` / `doc:<id>` when those surfaces mount the SAME store later —
//! the reconciliation's "build once, generalize by key"). The durable medium is JSON-on-disk under the
//! bare repo dir (the [`crate::pr_store`] pattern — there is no `thread` SQL table yet; the PG home is
//! the GT-003b follow-on). Schema evolution is additive `#[serde(default)]` fields (the honest
//! JSON-store analogue of a migration), exactly as `PrRecord` does it.
//!
//! ## Authz (enforced at the edge, named here)
//! Thread READ = the PR's own view permission (the `Pull` object guard — a viewer who may view the PR
//! may read its threads). Comment / review WRITE = a real write permission (the `Push` object guard,
//! backed in production by the deny-by-default `CheckBackedRepoAuthorizer`, never a permissive
//! authorizer). This store holds no authorizer — it is the durable medium; the edge is the chokepoint.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::RepoLoc;
use crate::durable::DurableError;
use crate::gix_backend::{RepoPathResolver, RootedResolver};

// ───────────────────────────── the shared atoms (VM shapes) ────────────────────────────────────────

/// The identity/agent badge atom (the `PrincipalVM` shape). `display` arrives pre-collapsed —
/// `[erased user]` / `Restricted` are set by the caller, never reconstructed here (non-leak by
/// construction). `on_behalf_of` / `trigger` are the agent attribution channels (ADR-08 legibility).
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
    /// A plain human/agent/service principal with only a display pseudonym (the common case).
    pub fn plain(kind: PrincipalRole, display: impl Into<String>) -> Self {
        ThreadPrincipal {
            kind,
            display: display.into(),
            on_behalf_of: None,
            trigger: None,
        }
    }
}

/// The principal role on a comment/review — `human` / `agent` / `service` (glyph + label, never
/// colour alone at the UI). An agent review is always advisory (never gates).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    Human,
    Agent,
    Service,
}

impl PrincipalRole {
    /// Is this an agent principal? An agent review is ADVISORY — it never counts toward the gate.
    pub fn is_agent(self) -> bool {
        matches!(self, PrincipalRole::Agent)
    }
}

/// The resolved content anchor for a diff-line-anchored thread — `None` on a thread means a PR-level
/// (Overview "discussion") thread. `line == None` = a detached (outdated) anchor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadAnchor {
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    pub anchor_state: AnchorState,
}

/// The anchor's resolved state (reference-chip §5 vocabulary). Never a silent wrong-line re-anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorState {
    Live,
    Moved,
    Outdated,
}

/// A comment's visibility state — `Removed` renders "Comment removed", the thread tree preserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommentState {
    Visible,
    Removed,
}

/// One durable comment. `pending == true` iff it belongs to an un-submitted review batch — visible
/// ONLY to its author until submit ([`SubjectThreads::view_for`] enforces this).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRecord {
    pub id: String,
    pub author: ThreadPrincipal,
    /// The Markdown body (ONE render path — the BlockEditor read path at the UI).
    pub body_md: String,
    pub created_at: i64,
    #[serde(default)]
    pub edited_at: Option<i64>,
    #[serde(default = "visible")]
    pub state: CommentState,
    /// Batch membership — `Some(review_id)` iff this comment was authored inside a review batch.
    #[serde(default)]
    pub review_id: Option<String>,
    /// True ONLY while the owning batch is un-submitted (drives the "Pending · only you" pill AND the
    /// read-side visibility filter). Submit flips this to `false` for every comment in the batch.
    #[serde(default)]
    pub pending: bool,
}

fn visible() -> CommentState {
    CommentState::Visible
}

/// One durable thread — a content anchor (or `None` for a PR-level discussion thread) + its comments.
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
    /// Is this a PR-level (Overview "discussion") thread? (anchor == None)
    pub fn is_discussion(&self) -> bool {
        self.anchor.is_none()
    }
}

/// The verdict on a review batch. `InProgress` = an un-submitted draft; the terminal verdicts are set
/// at submit. `Approved` / `ChangesRequested` on a NON-advisory (human) batch feed the merge gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchVerdict {
    InProgress,
    Approved,
    ChangesRequested,
    Commented,
}

impl BatchVerdict {
    /// The stable wire token (the frontend maps it to a glyph + label).
    pub fn as_str(self) -> &'static str {
        match self {
            BatchVerdict::InProgress => "in_progress",
            BatchVerdict::Approved => "approved",
            BatchVerdict::ChangesRequested => "changes_requested",
            BatchVerdict::Commented => "commented",
        }
    }
}

/// One durable review batch (the `PrReviewVM` lifecycle). `advisory == true` for an agent reviewer —
/// it NEVER counts toward the gate. `submitted_at == None` while the batch is a draft.
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
    /// A draft (un-submitted) batch belongs to its reviewer alone — hidden from other viewers.
    pub fn is_draft(&self) -> bool {
        self.submitted_at.is_none()
    }
}

// ───────────────────────────── the per-subject durable document ────────────────────────────────────

/// The whole conversation for ONE subject (`object_key`), persisted as one JSON file. Additive
/// `#[serde(default)]` fields are the JSON-store schema-evolution path (no `thread` SQL table yet).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectThreads {
    /// The canonical type-qualified object key this document keys on (`pr:<slug>:<n>`, `issue:<id>`…).
    #[serde(default)]
    pub object_key: String,
    #[serde(default)]
    pub threads: Vec<ThreadRecord>,
    #[serde(default)]
    pub reviews: Vec<ReviewBatch>,
    /// A monotonic per-subject id counter (thread/comment/review ids are `t-<n>` / `c-<n>` / `r-<n>`).
    #[serde(default)]
    pub seq: u64,
}

impl SubjectThreads {
    fn next_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}-{}", self.seq)
    }

    /// A find for a batch by id.
    fn review(&self, review_id: &str) -> Option<&ReviewBatch> {
        self.reviews.iter().find(|r| r.id == review_id)
    }

    /// **The viewer-scoped projection (BINDING non-leak rule).** Returns the threads + reviews a
    /// `viewer` (by pseudonym `display`) may see: every PENDING comment authored by ANOTHER principal
    /// is dropped, a thread left with no visible comments is dropped, and a DRAFT (un-submitted) review
    /// batch owned by another reviewer is hidden. A viewer's own pending drafts stay visible to them.
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
                    return None; // a thread of only-others'-pending comments does not exist for this viewer
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

/// A viewer-scoped projection of a subject's conversation (the output of [`SubjectThreads::view_for`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewedThreads {
    pub threads: Vec<ThreadRecord>,
    pub reviews: Vec<ReviewBatch>,
}

/// The single authoritative batch a submit produces — the ONE notification event's payload
/// (R-BATCH-1). `comment_ids` are the comments the submit made visible (0..N — the event carries the
/// whole batch, never one event per comment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedBatch {
    pub review: ReviewBatch,
    pub comment_ids: Vec<String>,
}

// ───────────────────────────── the durable on-disk store ───────────────────────────────────────────

/// **The durable on-disk thread/comment/review store.** One JSON document per subject at
/// `<repo>.git/myelin/threads/<safe(object_key)>.json`, resolved through the SAME validated
/// [`RepoPathResolver`] the durable git + PR stores use (tenant/region path-isolated + traversal-safe).
pub struct DurablePrThreadStore<P: RepoPathResolver = RootedResolver> {
    resolver: P,
}

impl DurablePrThreadStore<RootedResolver> {
    /// Root the store at the SAME on-disk root the durable git + PR stores use.
    pub fn rooted(root: impl Into<PathBuf>) -> Self {
        Self {
            resolver: RootedResolver::new(root),
        }
    }
}

/// Sanitise a canonical object key (`pr:core:42`) into a safe single-segment filename stem
/// (`pr-core-42`). Any non-`[A-Za-z0-9._-]` byte becomes `_` so a key with `:` / `/` never escapes the
/// threads dir (the resolver already isolates the repo dir; this keeps ONE file per subject inside it).
/// **Floor:** two keys differing only in a sanitised byte would collide — object keys are
/// `type:id`-structured and workspace-unique per subsystem, so within one repo `pr:<slug>:<n>` stems
/// are distinct; a cross-type mount (issue/doc) carries its own `type` prefix, keeping stems disjoint.
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
    /// Build over a resolver (the placement resolver swaps in here behind the same port).
    pub fn new(resolver: P) -> Self {
        Self { resolver }
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

    /// Load the subject document (an empty, `object_key`-stamped default if none persisted yet).
    pub fn load(&self, repo: &RepoLoc, object_key: &str) -> Result<SubjectThreads, DurableError> {
        let path = self.subject_path(repo, object_key)?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mut doc: SubjectThreads = serde_json::from_slice(&bytes)
                    .map_err(|e| DurableError::Io(format!("parse {}: {e}", path.display())))?;
                if doc.object_key.is_empty() {
                    doc.object_key = object_key.to_string();
                }
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
        let dir = self.threads_dir(repo)?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| DurableError::Io(format!("create dir {}: {e}", dir.display())))?;
        let file = self.subject_path(repo, &doc.object_key)?;
        let bytes = serde_json::to_vec_pretty(doc)
            .map_err(|e| DurableError::Io(format!("serialize threads {}: {e}", doc.object_key)))?;
        let tmp = dir.join(format!(
            ".{}.tmp",
            file.file_name().and_then(|s| s.to_str()).unwrap_or("x")
        ));
        std::fs::write(&tmp, &bytes)
            .map_err(|e| DurableError::Io(format!("write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &file)
            .map_err(|e| DurableError::Io(format!("rename {}: {e}", file.display())))?;
        Ok(())
    }

    // ── thread + comment ops (single, un-batched — the Overview discussion + the diff line threads) ──

    /// Create a NEW thread with its first comment. `anchor == None` = a PR-level discussion thread
    /// (the Overview renders those); `Some` = a diff-line-anchored thread (the diff pack renders those).
    pub fn create_thread(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        anchor: Option<ThreadAnchor>,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> Result<ThreadRecord, DurableError> {
        let mut doc = self.load(repo, object_key)?;
        let tid = doc.next_id("t");
        let cid = doc.next_id("c");
        let thread = ThreadRecord {
            id: tid,
            anchor,
            resolved: false,
            comments: vec![CommentRecord {
                id: cid,
                author,
                body_md: body_md.into(),
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

    /// Reply to an existing thread (a single, non-batched comment). `NotFound` if the thread is absent.
    pub fn add_comment(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        thread_id: &str,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> Result<CommentRecord, DurableError> {
        let mut doc = self.load(repo, object_key)?;
        let cid = doc.next_id("c");
        let thread = doc
            .threads
            .iter_mut()
            .find(|t| t.id == thread_id)
            .ok_or_else(|| DurableError::NotFound(format!("thread {thread_id}")))?;
        let comment = CommentRecord {
            id: cid,
            author,
            body_md: body_md.into(),
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

    /// Resolve / unresolve a thread. `NotFound` if the thread is absent.
    pub fn resolve_thread(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), DurableError> {
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

    // ── review-batch lifecycle (G-8) ──

    /// Start a review batch (verdict `InProgress`, `submitted_at == None`). `advisory` is derived from
    /// the reviewer role (an agent batch is advisory — it never gates). Returns the fresh batch.
    pub fn start_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        reviewer: ThreadPrincipal,
    ) -> Result<ReviewBatch, DurableError> {
        let mut doc = self.load(repo, object_key)?;
        let rid = doc.next_id("r");
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

    /// Add a PENDING comment to an un-submitted batch (its own thread, anchored or discussion). The
    /// comment is `pending == true` → visible only to its author until submit. `NotFound` if the batch
    /// is absent; a `Forbidden` if the batch is already submitted (no appending to a closed batch).
    pub fn add_pending_comment(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        review_id: &str,
        anchor: Option<ThreadAnchor>,
        author: ThreadPrincipal,
        body_md: impl Into<String>,
        now: i64,
    ) -> Result<CommentRecord, DurableError> {
        let mut doc = self.load(repo, object_key)?;
        match doc.review(review_id) {
            None => return Err(DurableError::NotFound(format!("review {review_id}"))),
            Some(r) if !r.is_draft() => {
                return Err(DurableError::Forbidden(format!(
                    "review {review_id} is already submitted — cannot append pending comments"
                )))
            }
            Some(_) => {}
        }
        let tid = doc.next_id("t");
        let cid = doc.next_id("c");
        let comment = CommentRecord {
            id: cid,
            author,
            body_md: body_md.into(),
            created_at: now,
            edited_at: None,
            state: CommentState::Visible,
            review_id: Some(review_id.to_string()),
            pending: true,
        };
        doc.threads.push(ThreadRecord {
            id: tid,
            anchor,
            resolved: false,
            comments: vec![comment.clone()],
        });
        self.save(repo, &doc)?;
        Ok(comment)
    }

    /// **Submit a review batch — the ONE-event operation (R-BATCH-1).** Sets the terminal verdict +
    /// `submitted_at` + optional summary, flips every pending comment of the batch to `pending ==
    /// false` (now visible to all), and returns `Some(SubmittedBatch)` — the single event payload —
    /// EXACTLY ONCE. A re-submit of an already-submitted batch is idempotent `None` (no double event,
    /// no verdict flip). `NotFound` if the batch is absent.
    pub fn submit_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        review_id: &str,
        verdict: BatchVerdict,
        summary_md: Option<String>,
        now: i64,
    ) -> Result<Option<SubmittedBatch>, DurableError> {
        let mut doc = self.load(repo, object_key)?;
        let already = match doc.review(review_id) {
            None => return Err(DurableError::NotFound(format!("review {review_id}"))),
            Some(r) => !r.is_draft(),
        };
        if already {
            return Ok(None); // idempotent — one event per batch, never a double on retry.
        }
        // Flip the batch's pending comments visible.
        let mut comment_ids = Vec::new();
        for t in &mut doc.threads {
            for c in &mut t.comments {
                if c.review_id.as_deref() == Some(review_id) && c.pending {
                    c.pending = false;
                    comment_ids.push(c.id.clone());
                }
            }
        }
        // Set the terminal verdict on the batch.
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
        self.save(repo, &doc)?;
        Ok(Some(SubmittedBatch {
            review,
            comment_ids,
        }))
    }

    /// Discard an un-submitted batch: remove the batch AND every pending comment/thread it owns (the
    /// text was the reviewer's private draft — discarding it leaves no trace). A submitted batch is
    /// NOT discardable (its comments are public record) — `Forbidden`. `NotFound` if absent.
    pub fn discard_review(
        &self,
        repo: &RepoLoc,
        object_key: &str,
        review_id: &str,
    ) -> Result<(), DurableError> {
        let mut doc = self.load(repo, object_key)?;
        match doc.review(review_id) {
            None => return Err(DurableError::NotFound(format!("review {review_id}"))),
            Some(r) if !r.is_draft() => {
                return Err(DurableError::Forbidden(format!(
                    "review {review_id} is already submitted — its comments are public record"
                )))
            }
            Some(_) => {}
        }
        // Drop this batch's pending comments, then any thread left empty.
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

    /// A discussion thread (anchor null) round-trips durably; a fresh store over the same root reads it.
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

    /// A legacy/partial subject document (missing the additive fields) still deserializes — the JSON
    /// store's schema-evolution analogue of a migration.
    #[test]
    fn a_legacy_subject_doc_deserializes_with_defaults() {
        let legacy = serde_json::json!({ "object_key": "pr:core:1", "threads": [
            { "id": "t-1", "comments": [
                { "id": "c-1", "author": { "kind": "human", "display": "psn:a@acme" },
                  "body_md": "hi", "created_at": 1 } ] } ] });
        let doc: SubjectThreads = serde_json::from_value(legacy).expect("legacy doc deserializes");
        assert_eq!(doc.reviews.len(), 0);
        assert_eq!(doc.threads[0].resolved, false);
        assert_eq!(doc.threads[0].anchor, None);
        let c = &doc.threads[0].comments[0];
        assert_eq!(c.state, CommentState::Visible);
        assert_eq!(c.pending, false);
        assert_eq!(c.review_id, None);
    }

    /// **BINDING: a pending review comment is visible ONLY to its author.** A second viewer's
    /// projection does not contain the draft; the author's does.
    #[test]
    fn a_pending_comment_is_invisible_to_others_until_submit() {
        let root = temp_root("pending");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:reviewer@acme"))
            .unwrap();
        store
            .add_pending_comment(
                &loc(),
                KEY,
                &batch.id,
                None,
                human("psn:reviewer@acme"),
                "draft note",
                200,
            )
            .unwrap();

        let doc = store.load(&loc(), KEY).unwrap();
        // The author sees their draft (comment + the draft batch).
        let mine = doc.view_for("psn:reviewer@acme");
        assert_eq!(mine.threads.len(), 1, "author sees their own pending draft");
        assert_eq!(mine.reviews.len(), 1);
        // A DIFFERENT viewer sees neither the pending comment nor the draft batch.
        let other = doc.view_for("psn:other@acme");
        assert_eq!(other.threads.len(), 0, "a pending comment must be invisible to others");
        assert_eq!(other.reviews.len(), 0, "a draft batch is hidden from others");

        // After submit, the comment is visible to everyone.
        store
            .submit_review(&loc(), KEY, &batch.id, BatchVerdict::ChangesRequested, None, 300)
            .unwrap();
        let doc = store.load(&loc(), KEY).unwrap();
        let other = doc.view_for("psn:other@acme");
        assert_eq!(other.threads.len(), 1, "submit makes the batch's comments public");
        assert_eq!(other.reviews[0].verdict, BatchVerdict::ChangesRequested);
        assert!(!other.reviews[0].advisory, "a human batch is not advisory");
        std::fs::remove_dir_all(&root).ok();
    }

    /// **BINDING R-BATCH-1: submit emits exactly ONE batch event, regardless of comment count, and is
    /// idempotent on retry (no double event).**
    #[test]
    fn submit_emits_one_batch_event_and_is_idempotent() {
        let root = temp_root("onevent");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store
            .start_review(&loc(), KEY, human("psn:reviewer@acme"))
            .unwrap();
        for i in 0..3 {
            store
                .add_pending_comment(
                    &loc(),
                    KEY,
                    &batch.id,
                    None,
                    human("psn:reviewer@acme"),
                    format!("note {i}"),
                    200 + i,
                )
                .unwrap();
        }
        // First submit → ONE event carrying all three comments.
        let first = store
            .submit_review(&loc(), KEY, &batch.id, BatchVerdict::Approved, Some("LGTM".into()), 400)
            .unwrap();
        let ev = first.expect("first submit yields exactly one batch event");
        assert_eq!(ev.comment_ids.len(), 3, "the ONE event carries the whole batch");
        assert_eq!(ev.review.verdict, BatchVerdict::Approved);
        assert_eq!(ev.review.summary_md.as_deref(), Some("LGTM"));
        // Re-submit → idempotent None (no second event, no verdict change).
        let second = store
            .submit_review(&loc(), KEY, &batch.id, BatchVerdict::Commented, None, 500)
            .unwrap();
        assert!(second.is_none(), "a re-submit must NOT emit a second event");
        std::fs::remove_dir_all(&root).ok();
    }

    /// An agent review batch is ADVISORY (never gates).
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

    /// Discarding a draft batch leaves no trace; a submitted batch is not discardable.
    #[test]
    fn discard_removes_a_draft_but_not_a_submitted_batch() {
        let root = temp_root("discard");
        let store = DurablePrThreadStore::rooted(&root);
        let batch = store.start_review(&loc(), KEY, human("psn:r@acme")).unwrap();
        store
            .add_pending_comment(&loc(), KEY, &batch.id, None, human("psn:r@acme"), "draft", 1)
            .unwrap();
        store.discard_review(&loc(), KEY, &batch.id).unwrap();
        let doc = store.load(&loc(), KEY).unwrap();
        assert_eq!(doc.threads.len(), 0, "discard removes the draft's threads");
        assert_eq!(doc.reviews.len(), 0);

        // A submitted batch cannot be discarded.
        let b2 = store.start_review(&loc(), KEY, human("psn:r@acme")).unwrap();
        store
            .submit_review(&loc(), KEY, &b2.id, BatchVerdict::Commented, None, 2)
            .unwrap();
        assert!(matches!(
            store.discard_review(&loc(), KEY, &b2.id),
            Err(DurableError::Forbidden(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Resolve toggles the thread flag durably.
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

    /// The object-key stem sanitiser keeps one file per subject and never escapes the dir.
    #[test]
    fn key_stem_sanitises_separators() {
        assert_eq!(key_stem("pr:core:42"), "pr_core_42");
        assert_eq!(key_stem("issue:PROJ-1"), "issue_PROJ-1");
        assert_eq!(key_stem("repo:team/app"), "repo_team_app");
        assert!(!key_stem("pr:../../etc").contains('/'));
    }
}
