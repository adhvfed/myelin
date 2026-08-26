use super::*;

pub(super) struct FileEditQuarantine {
    pub(super) repo: DurableGitRepo,
    pub(super) _directory: tempfile::TempDir,
}

impl FileEditQuarantine {
    pub(super) fn new(durable_repo: &DurableGitRepo) -> Result<Self, DurableError> {
        let directory = tempfile::Builder::new()
            .prefix("myelin-file-edit-quarantine-")
            .tempdir()
            .map_err(|error| {
                DurableError::Io(format!("create file-edit quarantine directory: {error}"))
            })?;
        let repo = DurableGitRepo::init_quarantine(
            directory.path(),
            &durable_repo.path().join("objects"),
        )?;
        Ok(Self {
            repo,
            _directory: directory,
        })
    }
}

pub(super) struct ObjectPromotion<'a> {
    pub(super) repo: &'a DurableGitRepo,
    pub(super) objects: &'a [(String, String, Vec<u8>)],
}

impl QuarantineMigration for ObjectPromotion<'_> {
    fn migrate(&self, _quarantine: &[QuarantineObject]) -> Result<(), String> {
        for (claimed_oid, ty, bytes) in self.objects {
            let written = self
                .repo
                .write_raw_object(ty, bytes)
                .map_err(|e| e.to_string())?;
            if &written.0 != claimed_oid {
                return Err(format!(
                    "refusing migration: object oid mismatch (claimed {claimed_oid}, git computed {})",
                    written.0
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn region_of<'a>(ctx: &'a HandlerCtx<'_>) -> &'a str {
    ctx.scope.region().0.as_str()
}

pub(super) fn clock_reading() -> Result<myelin_events::clock::ClockReading, DurableError> {
    myelin_events::clock::system_clock_reading()
        .map_err(|error| DurableError::Io(format!("Git clock unavailable: {error}")))
}

pub(super) fn now_unix() -> Result<i64, DurableError> {
    Ok(clock_reading()?.unix_seconds())
}

pub(super) fn branch_ref(gitref: &str) -> String {
    if gitref.starts_with("refs/heads/") {
        gitref.to_string()
    } else {
        format!("refs/heads/{gitref}")
    }
}

pub(super) fn file_write_request_hash(request: &FileCommit<'_>) -> String {
    let full_ref = branch_ref(request.gitref);
    let mut hasher = blake3::Hasher::new();
    hasher.update(if request.actor_is_agent {
        // Preserve the deployed agent-write identity so an in-flight retry still finds its receipt.
        b"myelin.git.file-write-request.v1\0".as_slice()
    } else {
        b"myelin.git.human-file-write-request.v1\0".as_slice()
    });
    for part in [
        request.target.tenant,
        request.target.principal.principal_id.0.as_str(),
        request.target.slug,
        full_ref.as_str(),
        request.path,
        request.expected_base,
        request.contents,
        request.start_ref.unwrap_or(""),
    ] {
        hasher.update(&(part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    if !request.actor_is_agent {
        hasher.update(&(request.message.len() as u64).to_be_bytes());
        hasher.update(request.message.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub(super) fn replayed_file_write(
    commit_oid: String,
    commit_message: &str,
    request_trailer: &str,
) -> Result<WebEditOutcome, DurableError> {
    if !commit_message.lines().any(|line| line == request_trailer) {
        return Err(DurableError::Conflict(
            "idempotency key is already bound to a different file write".into(),
        ));
    }
    Ok(WebEditOutcome::Committed {
        new_oid: commit_oid,
    })
}

pub(super) fn require_body_md(body: &Value) -> Result<String, DurableError> {
    body.get("body_md")
        .or_else(|| body.get("body"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DurableError::Git("comment body missing a non-empty `body_md`".into()))
}

pub(super) fn parse_anchor(body: &Value) -> Result<Option<ThreadAnchor>, DurableError> {
    let Some(value) = body.get("anchor") else {
        return Ok(None);
    };
    let anchor = value
        .as_object()
        .ok_or_else(|| DurableError::Git("anchor must be an object".into()))?;
    if anchor.len() != 3
        || !anchor.contains_key("path")
        || !anchor.contains_key("line")
        || !anchor.contains_key("side")
    {
        return Err(DurableError::Git(
            "anchor must contain exactly path, line, and side".into(),
        ));
    }
    let path = anchor
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| valid_anchor_path(path))
        .ok_or_else(|| DurableError::Git("anchor path is invalid".into()))?;
    let line = anchor
        .get("line")
        .and_then(Value::as_u64)
        .filter(|line| *line > 0 && *line <= u32::MAX as u64)
        .ok_or_else(|| DurableError::Git("anchor line is invalid".into()))?;
    let side = match anchor.get("side").and_then(Value::as_str) {
        Some("old") => AnchorSide::Old,
        Some("new") => AnchorSide::New,
        _ => return Err(DurableError::Git("anchor side is invalid".into())),
    };
    Ok(Some(ThreadAnchor {
        path: path.to_string(),
        line: Some(line),
        side: Some(side),
        base_oid: None,
        head_oid: None,
        anchor_state: AnchorState::Live,
    }))
}

pub(super) fn valid_anchor_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4 * 1024
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

pub(super) fn principal_role_token(role: PrincipalRole) -> &'static str {
    match role {
        PrincipalRole::Human => "human",
        PrincipalRole::Agent => "agent",
        PrincipalRole::Service => "service",
    }
}

pub(super) fn principal_json(p: &ThreadPrincipal) -> Value {
    json!({
        "kind": principal_role_token(p.kind),
        "display": p.display,
        "on_behalf_of": p.on_behalf_of,
        "trigger": p.trigger,
    })
}

pub(super) fn anchor_state_token(s: AnchorState) -> &'static str {
    match s {
        AnchorState::Live => "live",
        AnchorState::Moved => "moved",
        AnchorState::Outdated => "outdated",
    }
}

pub(super) fn comment_json(c: &CommentRecord) -> Value {
    json!({
        "id": c.id,
        "author": principal_json(&c.author),
        "body_md": match c.state { CommentState::Removed => Value::Null, _ => json!(c.body_md) },
        "created_at": c.created_at,
        "edited_at": c.edited_at,
        "state": match c.state { CommentState::Removed => "removed", _ => "visible" },
        "review_id": c.review_id,
        "pending": c.pending,
    })
}

pub(super) fn thread_json(t: &ThreadRecord) -> Value {
    json!({
        "id": t.id,
        "anchor": t.anchor.as_ref().map(|a| json!({
            "path": a.path,
            "line": a.line,
            "side": a.side.map(|side| match side { AnchorSide::Old => "old", AnchorSide::New => "new" }),
            "base_oid": a.base_oid,
            "head_oid": a.head_oid,
            "anchor_state": anchor_state_token(a.anchor_state),
        })),
        "resolved": t.resolved,
        "comments": t.comments.iter().map(comment_json).collect::<Vec<_>>(),
    })
}

pub(super) fn review_batch_json(r: &ReviewBatch) -> Value {
    json!({
        "id": r.id,
        "reviewer": principal_json(&r.reviewer),
        "verdict": r.verdict.as_str(),
        "advisory": r.advisory,
        "submitted_at": r.submitted_at,
        "summary_md": r.summary_md,
    })
}

pub(super) fn viewed_threads_json(v: &ViewedThreads) -> Value {
    let (anchored, discussion): (Vec<&ThreadRecord>, Vec<&ThreadRecord>) =
        v.threads.iter().partition(|t| t.anchor.is_some());
    json!({
        "discussion": discussion.iter().map(|t| thread_json(t)).collect::<Vec<_>>(),
        "anchored": anchored.iter().map(|t| thread_json(t)).collect::<Vec<_>>(),
        "threads": v.threads.iter().map(thread_json).collect::<Vec<_>>(),
        "reviews": v.reviews.iter().map(review_batch_json).collect::<Vec<_>>(),
        "durable": true,
    })
}
