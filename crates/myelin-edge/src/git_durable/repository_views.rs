use super::*;

pub(super) fn qualify_ref(gitref: &str) -> String {
    if gitref.starts_with("refs/") {
        gitref.to_string()
    } else {
        format!("refs/heads/{gitref}")
    }
}

pub(super) const LATEST_COMMIT_WALK_CAP: usize = 500;

pub(super) const REPO_SCAN_MAX_CANDIDATES: usize = 10_000;

pub(super) const PR_LIST_PER_REPO_MAX_RECORDS: usize = PR_LIST_OFFSET_MAX;
pub(super) const PR_LIST_PER_REPO_MAX_BYTES: usize = 64 * 1024 * 1024;

pub(super) const CROSS_PR_LIST_MAX_RECORDS: usize = 10_000;
pub(super) const CROSS_PR_LIST_MAX_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn checked_cross_pr_list_total(
    current: usize,
    addition: usize,
    maximum: usize,
    dimension: &'static str,
) -> Result<usize, DurableError> {
    current
        .checked_add(addition)
        .filter(|total| *total <= maximum)
        .ok_or_else(|| DurableError::Git(format!("pull request list limit exceeded: {dimension}")))
}

pub(super) fn serialized_pr_records_bytes(records: &[PrRecord]) -> Result<usize, DurableError> {
    records.iter().try_fold(0usize, |total, record| {
        let bytes = serde_json::to_vec(record)
            .map_err(|error| DurableError::Io(format!("serialize pull request record: {error}")))?;
        checked_cross_pr_list_total(
            total,
            bytes.len(),
            usize::MAX,
            "cross-repository serialized bytes",
        )
    })
}

pub(super) const BLOB_INLINE_CAP: usize = 512 * 1024;

pub(super) const README_MAX_BYTES: usize = 512 * 1024;

pub(super) const RAW_BLOB_MAX_BYTES: usize = 64 * 1024 * 1024;

pub(super) const CODE_SEARCH_MAX_REPOS: usize = 100;
pub(super) const CODE_SEARCH_MAX_ENTRIES: usize = 10_000;
pub(super) const CODE_SEARCH_MAX_BLOB_BYTES: usize = 512 * 1024;
pub(super) const CODE_SEARCH_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
pub(super) const CODE_SEARCH_MAX_RESULTS: usize = 100;
pub(super) const CODE_SEARCH_MAX_EXCERPT_CHARS: usize = 500;

pub(super) struct CodeSearchBudget {
    pub(super) entries: usize,
    pub(super) bytes: usize,
    pub(super) incomplete: bool,
    pub(super) exhausted: bool,
}

impl CodeSearchBudget {
    pub(super) fn new() -> Self {
        Self {
            entries: 0,
            bytes: 0,
            incomplete: false,
            exhausted: false,
        }
    }
}

pub(super) fn first_root_tree_page(
    repo: &DurableGitRepo,
    branch_ref: &str,
) -> Result<TreePage, DurableError> {
    match repo.tree_page(branch_ref, "", TreePageRequest::default()) {
        Ok(TreePageLookup::Dir(page)) => Ok(page),
        Ok(TreePageLookup::IsFile | TreePageLookup::Missing) => Err(DurableError::NotFound(
            format!("default branch `{branch_ref}` did not resolve to a root tree"),
        )),
        Err(TreePageError::Durable(error)) => Err(error),
        Err(error) => Err(DurableError::Git(format!(
            "default root tree page failed: {error}"
        ))),
    }
}

pub(super) fn read_text_blob_at_snapshot_bounded(
    repo: &DurableGitRepo,
    snapshot_oid: &CoreOid,
    path: &str,
    maximum_bytes: usize,
) -> Result<Option<String>, DurableError> {
    match repo.read_blob_at_commit_oid_bounded(snapshot_oid, path, maximum_bytes)? {
        BlobPathLookup::Found {
            bytes,
            is_binary: false,
            ..
        } => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
        BlobPathLookup::Found { .. }
        | BlobPathLookup::TooLarge { .. }
        | BlobPathLookup::IsDir
        | BlobPathLookup::Missing => Ok(None),
    }
}

pub(super) fn search_repo_code(
    repo: &DurableGitRepo,
    slug: &str,
    branch_ref: &str,
    query: &str,
    hits: &mut Vec<Value>,
    budget: &mut CodeSearchBudget,
) -> Result<(), DurableError> {
    let root = match repo.tree_page(
        branch_ref,
        "",
        TreePageRequest {
            limit: TREE_PAGE_MAX_LIMIT,
            query: None,
            cursor: None,
        },
    ) {
        Ok(TreePageLookup::Dir(page)) => page,
        Ok(TreePageLookup::IsFile | TreePageLookup::Missing) => return Ok(()),
        Err(TreePageError::Durable(error)) => return Err(error),
        Err(error) => return Err(DurableError::Git(format!("code search tree read: {error}"))),
    };
    let snapshot = root.snapshot_oid;
    let snapshot_ref = snapshot.as_str().to_string();
    let mut directories = vec![String::new()];
    while let Some(directory) = directories.pop() {
        let mut cursor = None;
        loop {
            let page = match repo.tree_page(
                &snapshot_ref,
                &directory,
                TreePageRequest {
                    limit: TREE_PAGE_MAX_LIMIT,
                    query: None,
                    cursor,
                },
            ) {
                Ok(TreePageLookup::Dir(page)) => page,
                Ok(TreePageLookup::IsFile | TreePageLookup::Missing) => break,
                Err(TreePageError::Durable(error)) => return Err(error),
                Err(error) => {
                    return Err(DurableError::Git(format!("code search tree read: {error}")))
                }
            };
            for entry in &page.entries {
                budget.entries = budget.entries.saturating_add(1);
                if budget.entries > CODE_SEARCH_MAX_ENTRIES {
                    budget.incomplete = true;
                    budget.exhausted = true;
                    return Ok(());
                }
                let path = if directory.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{directory}/{}", entry.name)
                };
                if entry.is_dir {
                    directories.push(path);
                    continue;
                }
                let blob = match repo.read_blob_at_commit_oid_bounded(
                    &snapshot,
                    &path,
                    CODE_SEARCH_MAX_BLOB_BYTES,
                )? {
                    BlobPathLookup::Found {
                        bytes,
                        is_binary: false,
                        ..
                    } => bytes,
                    BlobPathLookup::TooLarge { .. } => {
                        budget.incomplete = true;
                        continue;
                    }
                    BlobPathLookup::Found { .. }
                    | BlobPathLookup::IsDir
                    | BlobPathLookup::Missing => continue,
                };
                budget.bytes = budget.bytes.saturating_add(blob.len());
                if budget.bytes > CODE_SEARCH_MAX_TOTAL_BYTES {
                    budget.incomplete = true;
                    budget.exhausted = true;
                    return Ok(());
                }
                let text = String::from_utf8_lossy(&blob);
                for (index, line) in text.lines().enumerate() {
                    if !line.contains(query) {
                        continue;
                    }
                    hits.push(json!({
                        "repo": slug,
                        "ref": branch_ref,
                        "snapshot_oid": snapshot.as_str(),
                        "path": path,
                        "line": index + 1,
                        "excerpt": line.chars().take(CODE_SEARCH_MAX_EXCERPT_CHARS).collect::<String>(),
                    }));
                    if hits.len() >= CODE_SEARCH_MAX_RESULTS {
                        budget.incomplete = true;
                        budget.exhausted = true;
                        return Ok(());
                    }
                }
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
    }
    Ok(())
}

pub(super) fn short_oid12(oid: &str) -> String {
    oid.chars().take(12).collect()
}

pub(super) fn sanitize_fork_import_error(_error: DurableError) -> DurableError {
    DurableError::Git("fork commit import could not be completed".into())
}

pub(super) fn commit_brief_json(m: &CommitMeta) -> Value {
    json!({
        "short_oid": short_oid12(&m.oid),
        "oid": m.oid,
        "summary": m.summary,
        "author": m.author_name,
        "committed_at": m.time,
    })
}

pub(super) fn tree_entries_json(
    entries: &[myelin_git::durable::TreeEntryInfo],
    base: &str,
    per_entry: &std::collections::BTreeMap<String, CommitMeta>,
) -> Vec<Value> {
    entries
        .iter()
        .map(|e| {
            let full = if base.is_empty() {
                e.name.clone()
            } else {
                format!("{base}/{}", e.name)
            };
            let mut o = json!({ "name": e.name, "path": full, "is_dir": e.is_dir });
            if let Some(sz) = e.size {
                o["size"] = json!(sz);
            }
            if let Some(m) = per_entry.get(&e.name) {
                o["latest_commit"] = commit_brief_json(m);
            }
            o
        })
        .collect()
}

pub(super) fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\' && *c != '/')
        .collect();
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

pub(super) fn commit_row(m: CommitMeta) -> CommitRow {
    CommitRow {
        oid: m.oid,
        summary: m.summary,
        author: m.author_name,
        committed_at: m.time,
        parents: m.parents,
    }
}

pub(super) fn commit_diff_vm(d: CommitDetail) -> CommitDiff {
    CommitDiff {
        commit: commit_row(d.meta),
        message: d.message,
        files: d
            .files
            .into_iter()
            .map(|f| DiffFile {
                path: f.path,
                old_path: f.old_path,
                status: f.status,
                lines: f
                    .lines
                    .into_iter()
                    .map(|(origin, content)| DiffLineView { origin, content })
                    .collect(),
            })
            .collect(),
    }
}

pub(super) const PR_DIFF_PER_FILE_LINE_CAP: usize = 4000;

pub(super) fn pr_diff_line(l: myelin_git::durable::DiffLineDelta) -> PrDiffLine {
    PrDiffLine {
        origin: l.origin,
        content: l.content,
        old_no: l.old_no,
        new_no: l.new_no,
    }
}

pub(super) fn pr_diff_vm(
    number: u64,
    base_ref: &str,
    diff: PrDiff,
    offset: usize,
    limit: usize,
) -> PrDiffVM {
    let total_files = diff.total_files;
    let files: Vec<PrDiffFile> = diff
        .files
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|f| PrDiffFile {
            path: f.path,
            old_path: f.old_path,
            new_blob_oid: f.new_blob_oid,
            status: f.status,
            kind: f.kind.as_str().to_string(),
            additions: f.additions,
            deletions: f.deletions,
            size_bytes: f.size_bytes,
            hunks: f
                .hunks
                .into_iter()
                .map(|h| PrDiffHunk {
                    header: h.header,
                    old_start: h.old_start,
                    old_lines: h.old_lines,
                    new_start: h.new_start,
                    new_lines: h.new_lines,
                    lines: h.lines.into_iter().map(pr_diff_line).collect(),
                })
                .collect(),
            deleted_body_available: f.deleted_body_available,
            truncated: f.truncated,
        })
        .collect();
    let next_cursor = if offset.saturating_add(limit) < total_files {
        Some(offset.saturating_add(limit).to_string())
    } else {
        None
    };
    PrDiffVM {
        number,
        base_ref: base_ref.to_string(),
        base_oid: diff.base_oid,
        head_oid: diff.head_oid,
        three_dot: diff.three_dot,
        files,
        restricted_files: 0,
        total_files,
        total_additions: diff.total_additions,
        total_deletions: diff.total_deletions,
        next_cursor,
        limit,
    }
}

pub(crate) fn map_durable_err(e: DurableError) -> EdgeError {
    match e {
        DurableError::NotFound(m) => EdgeError::NotFound(m),
        DurableError::InvalidInput(m) => EdgeError::BadRequest(m),
        DurableError::Git(m) if m == "pull request list cursor visible set changed" => {
            EdgeError::Conflict("pull request list cursor is stale; restart pagination".into())
        }
        DurableError::Git(m) if m == "PR operation id conflicts with durable state" => {
            EdgeError::Conflict(
                "idempotency key is already bound to a different pull request operation".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("browse response limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository view exceeds the interactive browse limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("tree page limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "repository tree exceeds the interactive browse limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("pr diff computation limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "pull request diff exceeds the interactive file limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("commit diff computation limit exceeded:") => {
            EdgeError::PayloadTooLarge("commit diff exceeds the interactive content limit".into())
        }
        DurableError::Git(m) if m.starts_with("blame limit exceeded:") => {
            EdgeError::PayloadTooLarge("file exceeds the interactive blame limit".into())
        }
        DurableError::Git(m) if m.starts_with("blame unavailable:") => EdgeError::BadRequest(m),
        DurableError::Git(m) if m.starts_with("pull request list limit exceeded:") => {
            EdgeError::PayloadTooLarge(
                "pull request list exceeds the interactive record limit".into(),
            )
        }
        DurableError::Git(m) if m.starts_with("pull request record limit exceeded:") => {
            EdgeError::PayloadTooLarge("pull request record exceeds the storage limit".into())
        }
        DurableError::Git(m) if m.starts_with("branch protection limit exceeded:") => {
            EdgeError::PayloadTooLarge("branch protection policy exceeds the storage limit".into())
        }
        DurableError::Git(m) if m.starts_with("wire ref limit exceeded:") => {
            EdgeError::PayloadTooLarge("repository exceeds the smart-HTTP ref limit".into())
        }
        DurableError::Git(m)
            if m.contains("traversal")
                || m.contains("segment")
                || m.contains("slug")
                || m.contains("missing")
                || m.contains("exceeds")
                || m.contains("anchor") =>
        {
            EdgeError::BadRequest(m)
        }
        DurableError::CasMismatch { .. } => EdgeError::Conflict(e.to_string()),
        DurableError::Conflict(m) => EdgeError::Conflict(m),
        DurableError::Forbidden(m) => EdgeError::Forbidden(m),
        other => EdgeError::Internal(other.to_string()),
    }
}
