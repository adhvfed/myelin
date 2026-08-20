use super::*;

struct DRepoList {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoList {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let query = parse_repo_list_query(&ctx.request.query)?;
        let envelope = self.be.list_repositories(
            ctx.principal,
            u32::try_from(query.limit).expect("bounded repository list limit"),
            query.cursor,
        )?;
        repo_list_response(&envelope)
    }
}

struct DRepoCreate {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoCreate {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request: CreateRepositoryBody =
            required_json(&ctx.request.body, "repository creation")?;
        let slug = request.slug.as_str();
        let created = self
            .be
            .create_repo_as(tenant_of(ctx), region_of(ctx), slug, ctx.principal)
            .map_err(map_durable_err)?;
        if !created {
            let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), slug);
            if self.be.repo_authorizer().authorize_repo_permission(
                ctx.principal,
                &loc,
                RepoPermission::Push,
            ) {
                return Ok(EdgeResponse::json(
                    200,
                    &json!({
                        "applied": { "action": "git.repo.create", "slug": slug },
                        "created": false,
                        "durable": true,
                    }),
                ));
            }
            return Err(EdgeError::Conflict(format!("repo `{slug}` already exists")));
        }
        Ok(EdgeResponse::json(
            201,
            &json!({
                "applied": { "action": "git.repo.create", "slug": slug },
                "created": true,
                "durable": true,
            }),
        ))
    }
}

struct DRepoHome {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoHome {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let home = self
            .be
            .repo_home_json(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &home))
    }
}

struct DCommitLog {
    be: Arc<DurableGitBackend>,
}

#[cfg(test)]
#[path = "http/commit_log_cursor_tests.rs"]
mod commit_log_cursor_tests;

impl Handler for DCommitLog {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let page = Page::parse(&ctx.request.query, "commit log")?;
        let offset = page.offset(COMMIT_LOG_MAX_OFFSET, "commit-log")?;
        let limit = page.limit;
        let (rows, has_more) = self
            .be
            .commit_log(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                offset,
                limit,
            )
            .map_err(map_durable_err)?;
        let items: Vec<Value> = rows.iter().map(CommitRow::to_json).collect();
        let next = if has_more {
            Some(offset.saturating_add(limit).to_string())
        } else {
            None
        };
        let prev = if offset > 0 {
            Some(offset.saturating_sub(limit).to_string())
        } else {
            None
        };
        let range_from = if items.is_empty() { 0 } else { offset + 1 };
        let range_to = offset + items.len();
        let page = json!({
            "next_cursor": next,
            "prev_cursor": prev,
            "limit": limit,
            "offset": offset,
            "range": { "from": range_from, "to": range_to },
        });
        Ok(EdgeResponse::json(
            200,
            &json!({ "items": items, "page": page }),
        ))
    }
}

struct DCommitDiff {
    be: Arc<DurableGitBackend>,
}
impl Handler for DCommitDiff {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let diff = self
            .be
            .commit_diff(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "oid")?,
            )
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such commit".into()))?;
        Ok(EdgeResponse::json(200, &diff.to_json()))
    }
}

struct DBlobView {
    be: Arc<DurableGitBackend>,
}
impl Handler for DBlobView {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self.be.read_file(
            ctx.principal,
            param(ctx, "repo")?,
            param(ctx, "ref")?,
            param(ctx, "path")?,
        )?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DBlameView {
    be: Arc<DurableGitBackend>,
}

impl Handler for DBlameView {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self
            .be
            .blame_json(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                param(ctx, "path")?,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DRefs {
    be: Arc<DurableGitBackend>,
}

const REFS_MAX_QUERY_BYTES: usize = 16 * 1024;
const REFS_MAX_CURSOR_BYTES: usize = 8 * 1024;

fn decode_refs_query_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "git refs query contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    let decoded = percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map_err(|_| EdgeError::BadRequest("git refs query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "git refs query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

fn parse_refs_query(query: &str) -> Result<RefsPageRequest, EdgeError> {
    if query.len() > REFS_MAX_QUERY_BYTES {
        return Err(EdgeError::BadRequest("git refs query is too large".into()));
    }
    if query.is_empty() {
        return Ok(RefsPageRequest::default());
    }

    let mut limit = None;
    let mut cursor = None;
    let mut q = None;
    let mut current = None;
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
            EdgeError::BadRequest("malformed git refs query parameter (missing `=`)".into())
        })?;
        let name = decode_refs_query_component(raw_name)?;
        let value = decode_refs_query_component(raw_value)?;
        if name.is_empty() {
            return Err(EdgeError::BadRequest(
                "empty git refs query parameter name".into(),
            ));
        }
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!("duplicate git refs query parameter `{field}`"))
        };
        match name.as_str() {
            "limit" => {
                if limit.is_some() {
                    return Err(duplicate("limit"));
                }
                let parsed = value.parse::<usize>().ok().filter(|parsed| {
                    value == parsed.to_string() && (1..=REFS_PAGE_MAX_LIMIT).contains(parsed)
                });
                limit = Some(parsed.ok_or_else(|| {
                    EdgeError::BadRequest(format!(
                        "git refs limit must be canonical and within 1..={REFS_PAGE_MAX_LIMIT}"
                    ))
                })?);
            }
            "cursor" => {
                if cursor.is_some() {
                    return Err(duplicate("cursor"));
                }
                if value.is_empty() || value.len() > REFS_MAX_CURSOR_BYTES {
                    return Err(EdgeError::BadRequest(
                        "git refs cursor is empty or exceeds its byte limit".into(),
                    ));
                }
                cursor = Some(value);
            }
            "q" => {
                if q.is_some() {
                    return Err(duplicate("q"));
                }
                if value.len() > REFS_PAGE_MAX_QUERY_BYTES {
                    return Err(EdgeError::BadRequest(
                        "git refs q exceeds its byte limit".into(),
                    ));
                }
                q = Some(value);
            }
            "current" => {
                if current.is_some() {
                    return Err(duplicate("current"));
                }
                if value.len() > WIRE_MAX_REF_NAME_BYTES {
                    return Err(EdgeError::BadRequest(
                        "git refs current exceeds its byte limit".into(),
                    ));
                }
                if !value.starts_with("refs/heads/") && !value.starts_with("refs/tags/") {
                    return Err(EdgeError::BadRequest(
                        "git refs current must be a fully-qualified branch or tag ref".into(),
                    ));
                }
                current = Some(value);
            }
            other => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown git refs query parameter `{other}`"
                )))
            }
        }
    }
    Ok(RefsPageRequest {
        limit: limit.unwrap_or(REFS_PAGE_DEFAULT_LIMIT),
        query: q,
        current_ref: current,
        cursor,
    })
}

fn map_refs_page_err(error: RefsPageError) -> EdgeError {
    match error {
        RefsPageError::Durable(error) => map_durable_err(error),
        RefsPageError::CursorStale => {
            EdgeError::Conflict("git refs cursor is stale; restart pagination".into())
        }
        RefsPageError::InvalidLimit { .. } => EdgeError::BadRequest(format!(
            "git refs limit must be canonical and within 1..={REFS_PAGE_MAX_LIMIT}"
        )),
        RefsPageError::QueryTooLong { .. } => {
            EdgeError::BadRequest("git refs q exceeds its byte limit".into())
        }
        RefsPageError::InvalidCurrentRef => EdgeError::BadRequest(
            "git refs current must be a fully-qualified branch or tag ref".into(),
        ),
        RefsPageError::MalformedCursor | RefsPageError::CursorScopeMismatch => {
            EdgeError::BadRequest("git refs cursor is invalid for this request".into())
        }
    }
}

#[cfg(test)]
#[path = "http/refs_query_tests.rs"]
mod refs_query_tests;

impl Handler for DRefs {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request = parse_refs_query(&ctx.request.query)?;
        let vm = self
            .be
            .refs_json(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?, request)
            .map_err(map_refs_page_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DTree {
    be: Arc<DurableGitBackend>,
}

const TREE_MAX_QUERY_BYTES: usize = 16 * 1024;
const TREE_MAX_CURSOR_BYTES: usize = 8 * 1024;

fn decode_tree_query_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "git tree query contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    let decoded = percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map_err(|_| EdgeError::BadRequest("git tree query is not valid UTF-8".into()))?
        .into_owned();
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "git tree query contains a control character".into(),
        ));
    }
    Ok(decoded)
}

fn parse_tree_query(query: &str) -> Result<TreePageRequest, EdgeError> {
    if query.len() > TREE_MAX_QUERY_BYTES {
        return Err(EdgeError::BadRequest("git tree query is too large".into()));
    }
    if query.is_empty() {
        return Ok(TreePageRequest::default());
    }
    let mut limit = None;
    let mut cursor = None;
    let mut q = None;
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
            EdgeError::BadRequest("malformed git tree query parameter (missing `=`)".into())
        })?;
        let name = decode_tree_query_component(raw_name)?;
        let value = decode_tree_query_component(raw_value)?;
        if name.is_empty() {
            return Err(EdgeError::BadRequest(
                "empty git tree query parameter name".into(),
            ));
        }
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!("duplicate git tree query parameter `{field}`"))
        };
        match name.as_str() {
            "limit" => {
                if limit.is_some() {
                    return Err(duplicate("limit"));
                }
                let parsed = value.parse::<usize>().ok().filter(|parsed| {
                    value == parsed.to_string() && (1..=TREE_PAGE_MAX_LIMIT).contains(parsed)
                });
                limit = Some(parsed.ok_or_else(|| {
                    EdgeError::BadRequest(format!(
                        "git tree limit must be canonical and within 1..={TREE_PAGE_MAX_LIMIT}"
                    ))
                })?);
            }
            "cursor" => {
                if cursor.is_some() {
                    return Err(duplicate("cursor"));
                }
                if value.is_empty() || value.len() > TREE_MAX_CURSOR_BYTES {
                    return Err(EdgeError::BadRequest(
                        "git tree cursor is empty or exceeds its byte limit".into(),
                    ));
                }
                cursor = Some(value);
            }
            "q" => {
                if q.is_some() {
                    return Err(duplicate("q"));
                }
                if value.len() > TREE_PAGE_MAX_QUERY_BYTES {
                    return Err(EdgeError::BadRequest(
                        "git tree q exceeds its byte limit".into(),
                    ));
                }
                q = Some(value);
            }
            other => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown git tree query parameter `{other}`"
                )))
            }
        }
    }
    Ok(TreePageRequest {
        limit: limit.unwrap_or(TREE_PAGE_DEFAULT_LIMIT),
        query: q,
        cursor,
    })
}

fn map_tree_page_err(error: TreePageError) -> EdgeError {
    match error {
        TreePageError::Durable(error) => map_durable_err(error),
        TreePageError::CursorStale => {
            EdgeError::Conflict("git tree cursor is stale; restart pagination".into())
        }
        TreePageError::InvalidLimit { .. } => EdgeError::BadRequest(format!(
            "git tree limit must be canonical and within 1..={TREE_PAGE_MAX_LIMIT}"
        )),
        TreePageError::QueryTooLong { .. } | TreePageError::InvalidQuery => {
            EdgeError::BadRequest("git tree q is invalid or exceeds its byte limit".into())
        }
        TreePageError::MalformedCursor | TreePageError::CursorScopeMismatch => {
            EdgeError::BadRequest("git tree cursor is invalid for this request".into())
        }
    }
}

#[cfg(test)]
#[path = "http/tree_query_tests.rs"]
mod tree_query_tests;

#[cfg(test)]
#[path = "http/tree_page_backend_tests.rs"]
mod tree_page_backend_tests;

impl Handler for DTree {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request = parse_tree_query(&ctx.request.query)?;
        let path = ctx.params.get("path").map(String::as_str).unwrap_or("");
        let vm = self
            .be
            .tree_json(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                param(ctx, "ref")?,
                path,
                request,
            )
            .map_err(map_tree_page_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DRawFile {
    be: Arc<DurableGitBackend>,
    attachment: bool,
}

#[derive(Clone, Copy)]
pub(super) struct RawResponseOptions {
    pub(super) attachment: bool,
    pub(super) maximum_bytes: usize,
}

#[derive(Clone, Copy)]
pub(super) struct BlobViewOptions {
    pub(super) maximum_preview_bytes: usize,
    pub(super) maximum_transfer_bytes: usize,
}
impl Handler for DRawFile {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let path = ctx.params.get("path").map(String::as_str).unwrap_or("");
        self.be.raw_response(
            tenant_of(ctx),
            region_of(ctx),
            param(ctx, "repo")?,
            param(ctx, "ref")?,
            path,
            self.attachment,
        )
    }
}

struct DWebEditCommit {
    be: Arc<DurableGitBackend>,
}
impl Handler for DWebEditCommit {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request: WebEditCommitBody = required_json(&ctx.request.body, "file commit")?;
        let message = request.commit_message()?;
        let outcome = self
            .be
            .web_edit_commit(WebFileEdit {
                target: RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                ),
                gitref: param(ctx, "ref")?,
                path: param(ctx, "path")?,
                expected_base: &request.base_oid,
                contents: &request.contents,
                start_ref: request.start_ref.as_deref(),
                message,
            })
            .map_err(map_durable_err)?;
        match outcome {
            WebEditOutcome::Denied => Err(EdgeError::Forbidden("no write permission for this ref".into())),
            WebEditOutcome::StaleBase { .. } => Err(EdgeError::Conflict(
                "the file changed since you opened it - refused so nothing is silently overwritten \
                 (GF-6: no 3-way editor in v1)"
                    .into(),
            )),
            committed @ WebEditOutcome::Committed { .. } => Ok(EdgeResponse::json(
                200,
                &json!({ "applied": committed.to_json(), "durable": true }),
            )),
        }
    }
}

struct DOpenPr {
    be: Arc<DurableGitBackend>,
}
impl Handler for DOpenPr {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = canonical_json(required_json::<OpenPullRequestBody>(
            &ctx.request.body,
            "pull request creation",
        )?)?;
        let operation_id = self.be.request_operation_id(ctx.request, ctx.principal)?;
        let rec = self
            .be
            .open_pr_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                &body,
                ctx.principal,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.open", "pr": DurableGitBackend::pr_json(tenant_of(ctx), param(ctx, "repo")?, &rec) }, "durable": true }),
        ))
    }
}

struct DPrOverview {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrOverview {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
        let rec = self
            .be
            .pr_get(&loc, pull_request_number_param(ctx, "n")?, ctx.principal)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let mut vm = DurableGitBackend::pr_json(tenant_of(ctx), param(ctx, "repo")?, &rec);
        if let Some(obj) = vm.as_object_mut() {
            match self.be.commits_in_pr_count(&loc, &rec) {
                Some((count, has_more)) => {
                    obj.insert("commits_count".into(), json!(count));
                    obj.insert("commits_count_capped".into(), json!(has_more));
                }
                None => {
                    obj.insert("commits_count".into(), Value::Null);
                }
            }
        }
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DPrCommits {
    be: Arc<DurableGitBackend>,
}

const PR_COMMIT_QUERY_MAX_BYTES: usize = 16 * 1024;

struct PrCommitQuery {
    limit: usize,
    cursor: Option<PrCommitCursor>,
}

fn parse_pr_commit_query(query: &str) -> Result<PrCommitQuery, EdgeError> {
    if query.len() > PR_COMMIT_QUERY_MAX_BYTES {
        return Err(EdgeError::BadRequest(
            "pull request commit query is too large".into(),
        ));
    }
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("pull request commit query is malformed".into())
            })?;
            match name {
                "limit" => {
                    if limit.is_some() {
                        return Err(EdgeError::BadRequest(
                            "duplicate pull request commit query parameter `limit`".into(),
                        ));
                    }
                    let parsed = value.parse::<usize>().ok().filter(|parsed| {
                        value == parsed.to_string() && (1..=MAX_PAGE_LIMIT).contains(parsed)
                    });
                    limit = Some(parsed.ok_or_else(|| {
                        EdgeError::BadRequest(format!(
                            "pull request commit limit must be canonical and within 1..={MAX_PAGE_LIMIT}"
                        ))
                    })?);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(EdgeError::BadRequest(
                            "duplicate pull request commit query parameter `cursor`".into(),
                        ));
                    }
                    cursor = Some(PrCommitCursor::parse(value).map_err(|_| {
                        EdgeError::BadRequest("pull request commit cursor is malformed".into())
                    })?);
                }
                _ => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown pull request commit query parameter `{name}`"
                    )))
                }
            }
        }
    }
    Ok(PrCommitQuery {
        limit: limit.unwrap_or(DEFAULT_PAGE_LIMIT),
        cursor,
    })
}

fn pr_commit_cursor_scope(tenant: &str, region: &str, repo: &str, number: u64) -> [u8; 32] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"myelin.git.pr-commit-cursor.scope.v1\0");
    for component in [tenant, region, repo] {
        hash.update(&(component.len() as u64).to_be_bytes());
        hash.update(component.as_bytes());
    }
    hash.update(&number.to_be_bytes());
    *hash.finalize().as_bytes()
}

fn map_pr_commit_page_error(error: PrCommitPageError) -> EdgeError {
    match error {
        PrCommitPageError::InvalidPagination => {
            EdgeError::BadRequest("pull request commit pagination is invalid".into())
        }
        PrCommitPageError::CapacityExceeded => EdgeError::PayloadTooLarge(
            "pull request commit history exceeds the interactive walk limit".into(),
        ),
        PrCommitPageError::SnapshotExpired => {
            EdgeError::Conflict("pull request commit cursor expired".into())
        }
        PrCommitPageError::Durable(error) => map_durable_err(error),
    }
}

impl Handler for DPrCommits {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let query = parse_pr_commit_query(&ctx.request.query)?;
        let repo_slug = param(ctx, "repo")?;
        let number = pull_request_number_param(ctx, "n")?;
        let scope = pr_commit_cursor_scope(tenant_of(ctx), region_of(ctx), repo_slug, number);
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), repo_slug);
        let rec = self
            .be
            .pr_get(&loc, number, ctx.principal)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let repo = self.be.store.open_repo(&loc).map_err(map_durable_err)?;
        let (snapshot, position) = match query.cursor.as_ref() {
            Some(cursor) => {
                if cursor.scope() != scope {
                    return Err(EdgeError::BadRequest(
                        "pull request commit cursor scope mismatch".into(),
                    ));
                }
                (
                    PrCommitSnapshot {
                        base_oid: cursor.base_oid().map(str::to_string),
                        head_oid: cursor.head_oid().to_string(),
                    },
                    cursor.position(),
                )
            }
            None => match repo
                .pr_commit_snapshot(&rec.base_ref, &rec.head_oid)
                .map_err(map_durable_err)?
            {
                Some(snapshot) => (snapshot, 0),
                None => {
                    return Ok(EdgeResponse::json(
                        200,
                        &page_envelope(json!([]), None, query.limit),
                    ))
                }
            },
        };
        let (metas, has_more) = repo
            .commits_in_pr_snapshot(
                snapshot.base_oid.as_deref(),
                &snapshot.head_oid,
                position,
                query.limit,
            )
            .map_err(map_pr_commit_page_error)?;
        let items: Vec<Value> = metas.into_iter().map(|m| commit_row(m).to_json()).collect();
        let next = if has_more {
            let next_position = position
                .checked_add(items.len())
                .filter(|position| (1..=PR_COMMIT_CURSOR_MAX_POSITION).contains(position));
            let next_position = next_position.ok_or_else(|| {
                EdgeError::PayloadTooLarge(
                    "pull request commit history exceeds the interactive pagination limit".into(),
                )
            })?;
            Some(
                PrCommitCursor::new(
                    scope,
                    snapshot.base_oid.as_deref(),
                    &snapshot.head_oid,
                    next_position,
                )
                .map_err(|error| {
                    EdgeError::Internal(format!("mint pull request commit cursor failed: {error}"))
                })?
                .encode(),
            )
        } else {
            None
        };
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, query.limit),
        ))
    }
}

#[cfg(test)]
#[path = "http/pr_commit_pagination_tests.rs"]
mod pr_commit_pagination_tests;

struct DPrDiff {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrDiff {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let page = Page::parse(&ctx.request.query, "pull request diff")?;
        let offset = page.offset(myelin_git::durable::PR_DIFF_MAX_FILES, "pull request diff")?;
        let limit = page.limit;
        let vm = self
            .be
            .pr_diff(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                offset,
                limit,
            )
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        Ok(EdgeResponse::json(200, &vm.to_json()))
    }
}

struct DFileLines {
    be: Arc<DurableGitBackend>,
}

struct FileLinesQuery {
    path: String,
    start: usize,
    end: usize,
}

const FILE_LINES_MAX_QUERY_BYTES: usize = 16 * 1024;

fn parse_file_lines_query(query: &str) -> Result<FileLinesQuery, EdgeError> {
    if query.len() > FILE_LINES_MAX_QUERY_BYTES {
        return Err(EdgeError::BadRequest(
            "file-lines query is too large".into(),
        ));
    }
    let mut path = None;
    let mut start = None;
    let mut end = None;
    for pair in query.split('&') {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| EdgeError::BadRequest("malformed file-lines query parameter".into()))?;
        let duplicate = |field: &str| {
            EdgeError::BadRequest(format!("duplicate file-lines query parameter `{field}`"))
        };
        match name {
            "path" => {
                if path.is_some() {
                    return Err(duplicate("path"));
                }
                let decoded = decode_form_query_component(value, "file-lines")?;
                if !valid_anchor_path(&decoded) {
                    return Err(EdgeError::BadRequest("file-lines path is invalid".into()));
                }
                path = Some(decoded);
            }
            "start" | "end" => {
                let slot = if name == "start" {
                    &mut start
                } else {
                    &mut end
                };
                if slot.is_some() {
                    return Err(duplicate(name));
                }
                let number =
                    myelin_git::coordinate::parse_positive_decimal(value).ok_or_else(|| {
                        EdgeError::BadRequest(format!(
                            "file-lines `{name}` must be a canonical positive line number"
                        ))
                    })?;
                if number > u32::MAX as u64 {
                    return Err(EdgeError::BadRequest(format!(
                        "file-lines `{name}` must be a canonical positive line number"
                    )));
                }
                *slot = Some(number as usize);
            }
            "" => {
                return Err(EdgeError::BadRequest(
                    "empty file-lines query parameter".into(),
                ))
            }
            other => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown file-lines query parameter `{other}`"
                )))
            }
        }
    }
    let path = path.ok_or_else(|| EdgeError::BadRequest("file-lines path is required".into()))?;
    let start =
        start.ok_or_else(|| EdgeError::BadRequest("file-lines start is required".into()))?;
    let end = end.ok_or_else(|| EdgeError::BadRequest("file-lines end is required".into()))?;
    if end < start || end - start + 1 > FILE_LINES_MAX_RANGE {
        return Err(EdgeError::BadRequest(format!(
            "file-lines range must be ordered and no larger than {FILE_LINES_MAX_RANGE} lines"
        )));
    }
    Ok(FileLinesQuery { path, start, end })
}

fn canonical_blob_oid(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Handler for DFileLines {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let FileLinesQuery { path, start, end } = parse_file_lines_query(&ctx.request.query)?;
        debug_assert!(!path.is_empty());
        let oid = param(ctx, "oid")?;
        if !canonical_blob_oid(oid) {
            return Err(EdgeError::BadRequest(
                "file-lines oid must be a canonical lowercase object id".into(),
            ));
        }
        let lookup = self
            .be
            .file_lines(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                oid,
                start,
                end,
            )
            .map_err(map_durable_err)?;
        let lines: Vec<PrDiffLine> = match lookup {
            FileLinesLookup::Found(lines) => lines.into_iter().map(pr_diff_line).collect(),
            FileLinesLookup::Binary | FileLinesLookup::Missing => Vec::new(),
            FileLinesLookup::TooLarge { maximum, .. } => {
                return Err(EdgeError::PayloadTooLarge(format!(
                    "file is too large for context expansion (maximum {maximum} bytes)"
                )))
            }
        };
        let items: Vec<Value> = lines.iter().map(PrDiffLine::to_json).collect();
        Ok(EdgeResponse::json(200, &json!({ "lines": items })))
    }
}

struct DPrThreads {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreads {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let vm = self
            .be
            .list_threads(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                pull_request_number_param(ctx, "n")?,
                ctx.principal,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

struct DPrThreadCreate {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreadCreate {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = canonical_json(required_json::<ThreadCreateBody>(
            &ctx.request.body,
            "review thread creation",
        )?)?;
        let operation_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let vm = self
            .be
            .create_thread(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                &operation_nonce,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.thread.create", "thread": vm }, "durable": true }),
        ))
    }
}

struct DPrThreadComment {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreadComment {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = canonical_json(required_json::<CommentBody>(
            &ctx.request.body,
            "review thread comment",
        )?)?;
        let operation_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let vm = self
            .be
            .add_thread_comment(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                param(ctx, "tid")?,
                &operation_nonce,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.comment.create", "comment": vm }, "durable": true }),
        ))
    }
}

struct DPrThreadResolve {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrThreadResolve {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = canonical_json(optional_json::<ResolveThreadBody>(
            &ctx.request.body,
            "review thread resolution",
        )?)?;
        let operation = self
            .be
            .required_request_operation(ctx.request, ctx.principal)?;
        let vm = self
            .be
            .resolve_thread(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                param(ctx, "tid")?,
                &operation.nonce,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.thread.resolve", "result": vm }, "durable": true }),
        ))
    }
}

struct DPrReviewStart {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewStart {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        optional_json::<EmptyMutationBody>(&ctx.request.body, "review start")?;
        let operation = self
            .be
            .required_request_operation(ctx.request, ctx.principal)?;
        let vm = self
            .be
            .start_review_batch(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                &operation.nonce,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.review.start", "review": vm }, "durable": true }),
        ))
    }
}

struct DPrReviewComment {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewComment {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = canonical_json(required_json::<ThreadCreateBody>(
            &ctx.request.body,
            "pending review comment",
        )?)?;
        let operation_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let vm = self
            .be
            .add_pending_comment(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                param(ctx, "rid")?,
                &operation_nonce,
                &body,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.pr.review.comment", "comment": vm }, "durable": true }),
        ))
    }
}

struct DPrReviewSubmit {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewSubmit {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = canonical_json(optional_json::<SubmitReviewBody>(
            &ctx.request.body,
            "review submission",
        )?)?;
        let operation = self
            .be
            .required_request_operation(ctx.request, ctx.principal)?;
        let vm = self
            .be
            .submit_review_batch(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                param(ctx, "rid")?,
                &body,
                &operation.nonce,
                &operation.pr_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review.submit", "result": vm }, "durable": true }),
        ))
    }
}

struct DPrReviewDiscard {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReviewDiscard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        optional_json::<EmptyMutationBody>(&ctx.request.body, "review discard")?;
        let operation = self
            .be
            .required_request_operation(ctx.request, ctx.principal)?;
        let vm = self
            .be
            .discard_review_batch(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                param(ctx, "rid")?,
                &operation.nonce,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review.discard", "result": vm }, "durable": true }),
        ))
    }
}

struct DPrChecks {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrChecks {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
        let rec = self
            .be
            .pr_get(&loc, pull_request_number_param(ctx, "n")?, ctx.principal)
            .map_err(map_durable_err)?
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        let vm = self
            .be
            .pr_checks_json(&loc, &rec, ctx.principal)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &vm))
    }
}

pub(super) fn cross_pr_before_key(row: &EnrichedPr, key: &PrListKey, sort: PrListSort) -> bool {
    let repo = row.repo_slug.as_deref().unwrap_or("");
    let key_repo = key.repo_slug.as_deref().unwrap_or("");
    if sort == PrListSort::Created {
        return row.rec.number > key.number || (row.rec.number == key.number && repo < key_repo);
    }
    match (row.rec.updated_at, key.updated_at) {
        (Some(row_time), Some(key_time)) if row_time != key_time => row_time > key_time,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        _ => row.rec.number > key.number || (row.rec.number == key.number && repo < key_repo),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mint_pr_list_cursors(
    rows: &[EnrichedPr],
    endpoint: PrListCursorEndpoint,
    sort: PrListSort,
    limit: usize,
    offset: usize,
    has_older: bool,
    has_newer: bool,
    static_scope: [u8; 32],
    visible_scope: [u8; 32],
    source_page: &PrListPage,
) -> Result<(Option<String>, Option<String>), DurableError> {
    let mint = |row: &EnrichedPr, direction, display_offset: usize| {
        let display_offset = u32::try_from(display_offset).map_err(|_| {
            DurableError::Git("pull request list cursor display offset exceeds u32".into())
        })?;
        PrListCursor::new(
            endpoint,
            direction,
            sort,
            limit,
            display_offset,
            PrListKey {
                updated_at: (sort == PrListSort::Updated)
                    .then_some(row.rec.updated_at)
                    .flatten(),
                number: row.rec.number,
                repo_slug: row.repo_slug.clone(),
            },
            static_scope,
            visible_scope,
        )
        .map(|cursor| cursor.encode())
        .map_err(|_| DurableError::Git("mint pull request list cursor failed".into()))
    };
    let next_cursor = if has_older {
        rows.last()
            .zip(offset.checked_add(rows.len()))
            .map(|(row, next_offset)| mint(row, PrListDirection::Older, next_offset))
            .transpose()?
    } else {
        None
    };
    let prev_cursor = if has_newer {
        rows.first()
            .map(|row| {
                mint(
                    row,
                    PrListDirection::Newer,
                    offset.saturating_sub(rows.len()),
                )
            })
            .transpose()?
    } else if rows.is_empty() {
        match source_page {
            PrListPage::LegacyOffset(legacy) if *legacy > 0 => {
                Some(legacy.saturating_sub(limit).to_string())
            }
            _ => None,
        }
    } else {
        None
    };
    Ok((next_cursor, prev_cursor))
}

fn cross_pr_list_envelope(page: EnrichedCrossPrSlice) -> Value {
    json!({
        "items": page.rows.iter().map(DurableGitBackend::pr_list_row_json).collect::<Vec<_>>(),
        "page": {
            "next_cursor": page.next_cursor,
            "prev_cursor": page.prev_cursor,
            "limit": page.limit,
            "offset": page.offset,
            "total": page.total,
        },
        "counts": { "bucket": page.total },
    })
}

fn repo_pr_list_envelope(page: EnrichedPrSlice) -> Value {
    let counts = json!({
        "open": page.counts.open,
        "merged": page.counts.merged,
        "closed": page.counts.closed,
        "all": page.counts.all,
        "yours": page.counts.yours,
        "needs_review": page.counts.needs_review,
    });
    json!({
        "items": page.rows.iter().map(DurableGitBackend::pr_list_row_json).collect::<Vec<_>>(),
        "page": {
            "next_cursor": page.next_cursor,
            "prev_cursor": page.prev_cursor,
            "limit": page.limit,
            "offset": page.offset,
            "total": page.total,
        },
        "counts": counts,
    })
}

struct DRepoPrList {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoPrList {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let viewer = DurableGitBackend::pseudonym(tenant_of(ctx), ctx.principal);
        let repo_slug = param(ctx, "repo")?;
        let query = repo_pr_list_query(ctx, viewer, repo_slug)?;
        let page = self
            .be
            .list_prs_for_repo(
                tenant_of(ctx),
                region_of(ctx),
                repo_slug,
                ctx.principal,
                &query,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &repo_pr_list_envelope(page)))
    }
}

struct DMyPrs {
    be: Arc<DurableGitBackend>,
}
impl Handler for DMyPrs {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let viewer = DurableGitBackend::pseudonym(tenant_of(ctx), ctx.principal);
        let query = cross_pr_list_query(ctx, viewer)?;
        let page = self
            .be
            .list_prs_cross_page(tenant_of(ctx), region_of(ctx), ctx.principal, &query)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(200, &cross_pr_list_envelope(page)))
    }
}

struct DPrReview {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrReview {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request: ReviewBody = required_json(&ctx.request.body, "pull request review")?;
        let verdict = request.verdict.as_str();
        let operation_id = self.be.request_operation_id(ctx.request, ctx.principal)?;
        let rec = self
            .be
            .submit_review_with_operation(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                verdict,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.review", "verdict": verdict, "reviews": rec.reviews.len() }, "durable": true }),
        ))
    }
}

struct DEndorse {
    be: Arc<DurableGitBackend>,
}
impl Handler for DEndorse {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let operation_id = self.be.request_operation_id(ctx.request, ctx.principal)?;
        let rec = self
            .be
            .endorse_fork_ci_with_operation(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                &body,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.pr.endorse_fork_ci", "endorsed": rec.endorsed_contexts }, "durable": true }),
        ))
    }
}

struct DMerge {
    be: Arc<DurableGitBackend>,
}
impl Handler for DMerge {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        optional_json::<EmptyMutationBody>(&ctx.request.body, "pull request merge")?;
        let operation_id = self.be.request_operation_id(ctx.request, ctx.principal)?;
        let attempt = self
            .be
            .merge_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                pull_request_number_param(ctx, "n")?,
                ctx.principal,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        match attempt {
            MergeAttempt::Merged {
                base_ref,
                new_oid,
                update_seq,
            } => Ok(EdgeResponse::json(
                200,
                &json!({
                    "applied": { "action": "git.pr.merge", "merged": true, "base_ref": base_ref,
                                 "new_oid": new_oid, "update_seq": update_seq },
                    "durable": true,
                }),
            )),
            MergeAttempt::Blocked(_eval) => {
                let loc =
                    DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?);
                let checks = self
                    .be
                    .pr_get(&loc, pull_request_number_param(ctx, "n")?, ctx.principal)
                    .ok()
                    .flatten()
                    .and_then(|rec| self.be.pr_checks_json(&loc, &rec, ctx.principal).ok());
                Ok(EdgeResponse::json(
                    409,
                    &json!({
                        "error": {
                            "code": "merge_blocked",
                            "message": "merge blocked by policy: branch protection requirements are unmet",
                        },
                        "checks": checks,
                        "durable": true,
                    }),
                ))
            }
            MergeAttempt::RefRefused(reason) => Err(EdgeError::Conflict(format!(
                "merge ref advance refused: {reason:?}"
            ))),
            MergeAttempt::InvalidHead(why) => {
                Err(EdgeError::BadRequest(format!("invalid merge head: {why}")))
            }
        }
    }
}

struct DSetBranchProtection {
    be: Arc<DurableGitBackend>,
}
impl Handler for DSetBranchProtection {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let n = self
            .be
            .set_branch_protection(tenant_of(ctx), region_of(ctx), param(ctx, "repo")?, &body)
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.repo.branch_protection.set", "rulesets": n }, "durable": true }),
        ))
    }
}

struct DReportChecks {
    be: Arc<DurableGitBackend>,
}
impl Handler for DReportChecks {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = ctx.request.json_body()?;
        let operation_id = self.be.request_operation_id(ctx.request, ctx.principal)?;
        let rec = self
            .be
            .report_checks_with_operation(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(pull_request_number_param(ctx, "n")?),
                &body,
                &operation_id,
            )
            .map_err(map_durable_err)?;
        Ok(EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.checks.report", "green_contexts": rec.green_contexts }, "durable": true }),
        ))
    }
}

const CODE_SEARCH_MAX_RAW_QUERY_BYTES: usize = 16 * 1024;

fn parse_code_search_query(query: &str) -> Result<(String, Option<String>), EdgeError> {
    if query.len() > CODE_SEARCH_MAX_RAW_QUERY_BYTES {
        return Err(EdgeError::BadRequest(
            "code search request query is too large".into(),
        ));
    }

    let mut search = None;
    let mut repo = None;
    for pair in query.split('&') {
        let (raw_name, raw_value) = pair.split_once('=').ok_or_else(|| {
            EdgeError::BadRequest("code search query parameter is malformed".into())
        })?;
        let name = decode_form_query_component(raw_name, "code search")?;
        let value = decode_form_query_component(raw_value, "code search")?;
        match name.as_str() {
            "q" => {
                if search.is_some() {
                    return Err(EdgeError::BadRequest(
                        "duplicate code search query parameter".into(),
                    ));
                }
                if !valid_code_search_query(&value) {
                    return Err(EdgeError::BadRequest("code search query is invalid".into()));
                }
                search = Some(value);
            }
            "repo" => {
                if repo.is_some() {
                    return Err(EdgeError::BadRequest(
                        "duplicate code search repository filter".into(),
                    ));
                }
                if !valid_code_search_repo(&value) {
                    return Err(EdgeError::BadRequest(
                        "code search repository filter is invalid".into(),
                    ));
                }
                repo = Some(value);
            }
            _ => {
                return Err(EdgeError::BadRequest(
                    "unknown code search query parameter".into(),
                ));
            }
        }
    }
    Ok((
        search.ok_or_else(|| EdgeError::BadRequest("code search query is required".into()))?,
        repo,
    ))
}

struct DCodeSearch {
    be: Arc<DurableGitBackend>,
}
impl Handler for DCodeSearch {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let (query, repo) = parse_code_search_query(&ctx.request.query)?;
        let response = self
            .be
            .search_code(ctx.principal, &query, repo.as_deref())?;
        Ok(EdgeResponse::json(200, &response))
    }
}

#[cfg(test)]
#[path = "http/code_search_boundary_tests.rs"]
mod code_search_boundary_tests;

struct RepoObjectGuard {
    be: Arc<DurableGitBackend>,
    permission: RepoPermission,
    inner: Arc<dyn Handler>,
}

impl Handler for RepoObjectGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let slug = param(ctx, "repo")?;
        let loc = RepoLoc::new(tenant_of(ctx), region_of(ctx), slug);
        if !self.be.repo_authorizer().authorize_repo_permission(
            ctx.principal,
            &loc,
            self.permission,
        ) {
            return Err(match self.permission {
                RepoPermission::Pull => EdgeError::NotFound("repository not found".into()),
                RepoPermission::Push => {
                    EdgeError::Forbidden("no write grant for this repository".into())
                }
                RepoPermission::ProtectedPush => EdgeError::Forbidden(
                    "no admin (protected_push) grant for this repository".into(),
                ),
                RepoPermission::ApproveUntrustedCi => EdgeError::Forbidden(
                    "no fork-CI endorsement grant (approve_untrusted_ci) for this repository"
                        .into(),
                ),
            });
        }
        self.inner.handle(ctx)
    }
}

fn guarded(
    be: &Arc<DurableGitBackend>,
    permission: RepoPermission,
    inner: Arc<dyn Handler>,
) -> Arc<dyn Handler> {
    Arc::new(RepoObjectGuard {
        be: be.clone(),
        permission,
        inner,
    })
}

struct PrReviewGuard {
    be: Arc<DurableGitBackend>,
    inner: Arc<dyn Handler>,
}

struct PrReadGuard {
    be: Arc<DurableGitBackend>,
    inner: Arc<dyn Handler>,
}

impl Handler for PrReadGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let slug = param(ctx, "repo")?;
        let number = pull_request_number_param(ctx, "n")?;
        let loc = DurableGitBackend::loc(tenant_of(ctx), region_of(ctx), slug);
        let can_pull = self.be.repo_authorizer().authorize_repo_permission(
            ctx.principal,
            &loc,
            RepoPermission::Pull,
        );
        if !can_pull
            && !self.be.authorize_pr_review(
                tenant_of(ctx),
                region_of(ctx),
                slug,
                number,
                ctx.principal,
            )
        {
            return Err(EdgeError::NotFound("pull request not found".into()));
        }
        self.inner.handle(ctx)
    }
}

fn pr_read_guarded(be: &Arc<DurableGitBackend>, inner: Arc<dyn Handler>) -> Arc<dyn Handler> {
    Arc::new(PrReadGuard {
        be: be.clone(),
        inner,
    })
}

impl Handler for PrReviewGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let slug = param(ctx, "repo")?;
        let number = pull_request_number_param(ctx, "n")?;
        if !self
            .be
            .authorize_pr_review(tenant_of(ctx), region_of(ctx), slug, number, ctx.principal)
        {
            return Err(EdgeError::Forbidden(
                "no review grant for this pull request".into(),
            ));
        }
        self.inner.handle(ctx)
    }
}

fn pr_review_guarded(be: &Arc<DurableGitBackend>, inner: Arc<dyn Handler>) -> Arc<dyn Handler> {
    Arc::new(PrReviewGuard {
        be: be.clone(),
        inner,
    })
}

pub fn register_git_durable(mut b: GatewayBuilder, be: Arc<DurableGitBackend>) -> GatewayBuilder {
    use RepoPermission::{ApproveUntrustedCi, ProtectedPush, Pull, Push};
    for ep in http_catalogue() {
        let pattern = match ep.operation {
            GitOperation::ReadBlob | GitOperation::WriteBlob => {
                reroot("/api/git/repos/{repo}/blob/{ref}/{...path}")
            }
            GitOperation::BlameBlob => reroot("/api/git/repos/{repo}/blame/{ref}/{...path}"),
            _ => reroot(ep.path()),
        };
        let method = map_method(ep.method());
        let (handler, action): (Arc<dyn Handler>, &'static str) = match ep.operation {
            GitOperation::ListRepositories => {
                (Arc::new(DRepoList { be: be.clone() }), "git.repos.list")
            }
            GitOperation::CreateRepository => {
                (Arc::new(DRepoCreate { be: be.clone() }), "git.repo.create")
            }
            GitOperation::ViewPullRequest => (
                pr_read_guarded(&be, Arc::new(DPrOverview { be: be.clone() })),
                "git.pr.view",
            ),
            GitOperation::ViewPullRequestChecks => (
                pr_read_guarded(&be, Arc::new(DPrChecks { be: be.clone() })),
                "git.pr.checks",
            ),
            GitOperation::ReadBlob => (Arc::new(DBlobView { be: be.clone() }), "git.blob.view"),
            GitOperation::BlameBlob => (
                guarded(&be, Pull, Arc::new(DBlameView { be: be.clone() })),
                "git.blame.view",
            ),
            GitOperation::WriteBlob => (
                guarded(&be, Push, Arc::new(DWebEditCommit { be: be.clone() })),
                "git.blob.commit",
            ),
            GitOperation::OpenPullRequest => (
                guarded(&be, Push, Arc::new(DOpenPr { be: be.clone() })),
                "git.pr.open",
            ),
            GitOperation::ReviewPullRequest => (
                pr_review_guarded(&be, Arc::new(DPrReview { be: be.clone() })),
                "git.pr.review",
            ),
            GitOperation::EndorseForkCi => (
                guarded(
                    &be,
                    ApproveUntrustedCi,
                    Arc::new(DEndorse { be: be.clone() }),
                ),
                "git.pr.endorse_fork_ci",
            ),
            GitOperation::MergePullRequest => (
                guarded(&be, ProtectedPush, Arc::new(DMerge { be: be.clone() })),
                "git.pr.merge",
            ),
            GitOperation::SetBranchProtection => (
                guarded(
                    &be,
                    ProtectedPush,
                    Arc::new(DSetBranchProtection { be: be.clone() }),
                ),
                "git.repo.branch_protection.set",
            ),
            GitOperation::ReportPullRequestChecks => (
                guarded(&be, Push, Arc::new(DReportChecks { be: be.clone() })),
                "git.checks.report",
            ),
            GitOperation::SearchCode => {
                (Arc::new(DCodeSearch { be: be.clone() }), "git.search.code")
            }
        };
        b = b.route(method, &pattern, action, handler);
    }
    let get = map_method(GitMethod::Get);
    let post = map_method(GitMethod::Post);
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs"),
        "git.prs.list",
        guarded(&be, Pull, Arc::new(DRepoPrList { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs/{n}/commits"),
        "git.pr.commits",
        pr_read_guarded(&be, Arc::new(DPrCommits { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs/{n}/diff"),
        "git.pr.diff",
        pr_read_guarded(&be, Arc::new(DPrDiff { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/file-lines/{oid}"),
        "git.file.lines",
        guarded(&be, Pull, Arc::new(DFileLines { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads"),
        "git.pr.threads.list",
        pr_read_guarded(&be, Arc::new(DPrThreads { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads"),
        "git.pr.thread.create",
        pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads/{tid}/comments"),
        "git.pr.comment.create",
        pr_review_guarded(&be, Arc::new(DPrThreadComment { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/threads/{tid}/resolve"),
        "git.pr.thread.resolve",
        pr_review_guarded(&be, Arc::new(DPrThreadResolve { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/start"),
        "git.pr.review.start",
        pr_review_guarded(&be, Arc::new(DPrReviewStart { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/comments"),
        "git.pr.review.comment",
        pr_review_guarded(&be, Arc::new(DPrReviewComment { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/submit"),
        "git.pr.review.submit",
        pr_review_guarded(&be, Arc::new(DPrReviewSubmit { be: be.clone() })),
    );
    b = b.route(
        post,
        &reroot("/api/git/repos/{repo}/prs/{n}/reviews/{rid}/discard"),
        "git.pr.review.discard",
        pr_review_guarded(&be, Arc::new(DPrReviewDiscard { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/prs"),
        "git.prs.mine",
        Arc::new(DMyPrs { be: be.clone() }),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}"),
        "git.repo.view",
        guarded(&be, Pull, Arc::new(DRepoHome { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/commits/{ref}"),
        "git.commits.log",
        guarded(&be, Pull, Arc::new(DCommitLog { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/commit/{oid}"),
        "git.commit.diff",
        guarded(&be, Pull, Arc::new(DCommitDiff { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/refs"),
        "git.refs.list",
        guarded(&be, Pull, Arc::new(DRefs { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/tree/{ref}/{...path}"),
        "git.tree.view",
        guarded(&be, Pull, Arc::new(DTree { be: be.clone() })),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/raw/{ref}/{...path}"),
        "git.blob.raw",
        guarded(
            &be,
            Pull,
            Arc::new(DRawFile {
                be: be.clone(),
                attachment: false,
            }),
        ),
    );
    b = b.route(
        get,
        &reroot("/api/git/repos/{repo}/download/{ref}/{...path}"),
        "git.blob.download",
        guarded(
            &be,
            Pull,
            Arc::new(DRawFile {
                be: be.clone(),
                attachment: true,
            }),
        ),
    );
    b
}

#[cfg(test)]
#[path = "http/event_privacy_tests.rs"]
mod event_privacy_tests;

#[cfg(test)]
#[path = "http/create_claim_tests.rs"]
mod create_claim_tests;

#[cfg(test)]
#[path = "http/repository_list_tests.rs"]
mod repository_list_tests;

#[cfg(test)]
#[path = "http/pr_list_tests.rs"]
mod pr_list_tests;

#[cfg(test)]
#[path = "http/file_lines_boundary_tests.rs"]
mod file_lines_boundary_tests;

#[cfg(test)]
#[path = "http/pr_thread_tests.rs"]
mod pr_thread_tests;
