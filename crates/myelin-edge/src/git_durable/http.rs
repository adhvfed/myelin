use super::*;

struct DRepoList {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoList {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if repo_summary_requested(&ctx.request.query) {
            let query = parse_repo_summary_query(&ctx.request.query)?;
            let envelope = self.be.list_repositories(
                ctx.principal,
                u32::try_from(query.limit).expect("bounded repository summary limit"),
                query.cursor,
            )?;
            return repo_summary_response(&envelope);
        }
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let (page, has_more) = self
            .be
            .list_repos_visible(tenant_of(ctx), region_of(ctx), ctx.principal, offset, limit)
            .map_err(map_durable_err)?;
        let items: Vec<Value> = page.iter().map(|repo| repo.to_json()).collect();
        let next = if has_more {
            Some(offset.saturating_add(limit).to_string())
        } else {
            None
        };
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit),
        ))
    }
}

struct DRepoCreate {
    be: Arc<DurableGitBackend>,
}
impl Handler for DRepoCreate {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let slug = body
            .get("slug")
            .or_else(|| body.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("create-repo body missing `slug`".into()))?;
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

fn commit_log_offset(cursor: Option<&str>) -> Result<usize, EdgeError> {
    match cursor {
        None => Ok(0),
        Some(cursor) => cursor
            .parse::<usize>()
            .ok()
            .filter(|offset| *offset <= COMMIT_LOG_MAX_OFFSET)
            .ok_or_else(|| EdgeError::BadRequest("invalid commit-log cursor".into())),
    }
}

#[cfg(test)]
mod commit_log_cursor_tests {
    use super::*;

    #[test]
    fn commit_log_cursor_is_strict_and_bounded() {
        assert_eq!(commit_log_offset(None).unwrap(), 0);
        assert_eq!(
            commit_log_offset(Some(&COMMIT_LOG_MAX_OFFSET.to_string())).unwrap(),
            COMMIT_LOG_MAX_OFFSET
        );
        let maximum = usize::MAX.to_string();
        for cursor in ["", "-1", "1.5", "not-a-cursor", maximum.as_str()] {
            assert!(matches!(
                commit_log_offset(Some(cursor)),
                Err(EdgeError::BadRequest(message)) if message == "invalid commit-log cursor"
            ));
        }
    }
}

impl Handler for DCommitLog {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let offset = commit_log_offset(ctx.page.cursor.as_deref())?;
        let limit = ctx.page.limit;
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
mod refs_query_tests {
    use super::*;

    #[test]
    fn refs_query_defaults_and_exact_decoding_are_stable() {
        assert_eq!(parse_refs_query("").unwrap(), RefsPageRequest::default());
        let parsed = parse_refs_query(
            "limit=%37&q=Feature%2FOne&current=refs%2Fheads%2Fmain&cursor=gr1_abc",
        )
        .expect("strict decoded query");
        assert_eq!(parsed.limit, 7);
        assert_eq!(parsed.query.as_deref(), Some("Feature/One"));
        assert_eq!(parsed.current_ref.as_deref(), Some("refs/heads/main"));
        assert_eq!(parsed.cursor.as_deref(), Some("gr1_abc"));
        assert_eq!(parse_refs_query("q=").unwrap().query.as_deref(), Some(""));
    }

    #[test]
    fn refs_query_rejects_noncanonical_and_ambiguous_inputs() {
        for query in [
            "limit",
            "=1",
            "unknown=1",
            "q=a&q=b",
            "q=a&%71=b",
            "limit=",
            "cursor=",
            "current=",
            "limit=01",
            "limit=+1",
            "limit=%2B1",
            "limit=0",
            "limit=101",
            "limit=-1",
            "limit=1.0",
            "q=%00",
            "q=%FF",
            "q=%",
            "current=main",
            "current=refs%2Fremotes%2Forigin%2Fmain",
            "q=a&&limit=1",
        ] {
            assert!(
                matches!(parse_refs_query(query), Err(EdgeError::BadRequest(_))),
                "query must fail closed: {query}"
            );
        }
    }

    #[test]
    fn refs_query_component_and_total_byte_limits_are_exact() {
        parse_refs_query(&format!("q={}", "x".repeat(REFS_PAGE_MAX_QUERY_BYTES)))
            .expect("exact q bound");
        parse_refs_query(&format!("cursor={}", "x".repeat(REFS_MAX_CURSOR_BYTES)))
            .expect("exact cursor bound");
        let current = format!(
            "refs/heads/{}",
            "x".repeat(WIRE_MAX_REF_NAME_BYTES - "refs/heads/".len())
        );
        parse_refs_query(&format!("current={current}")).expect("exact current bound");

        for query in [
            format!("q={}", "x".repeat(REFS_PAGE_MAX_QUERY_BYTES + 1)),
            format!("cursor={}", "x".repeat(REFS_MAX_CURSOR_BYTES + 1)),
            format!("current={current}x"),
            "x".repeat(REFS_MAX_QUERY_BYTES + 1),
        ] {
            assert!(matches!(
                parse_refs_query(&query),
                Err(EdgeError::BadRequest(_))
            ));
        }
    }

    #[test]
    fn refs_cursor_errors_map_to_scoped_statuses() {
        for error in [
            RefsPageError::MalformedCursor,
            RefsPageError::CursorScopeMismatch,
            RefsPageError::InvalidCurrentRef,
            RefsPageError::InvalidLimit { supplied: 0 },
        ] {
            assert_eq!(map_refs_page_err(error).status(), 400);
        }
        assert_eq!(map_refs_page_err(RefsPageError::CursorStale).status(), 409);
        assert_eq!(
            map_refs_page_err(RefsPageError::Durable(DurableError::NotFound(
                "missing".into()
            )))
            .status(),
            404
        );
    }
}

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
mod tree_query_tests {
    use super::*;

    #[test]
    fn tree_query_defaults_and_exact_decoding_are_stable() {
        assert_eq!(parse_tree_query("").unwrap(), TreePageRequest::default());
        let parsed = parse_tree_query("limit=%37&q=Readme+File&cursor=gt1_abc")
            .expect("strict decoded query");
        assert_eq!(parsed.limit, 7);
        assert_eq!(parsed.query.as_deref(), Some("Readme File"));
        assert_eq!(parsed.cursor.as_deref(), Some("gt1_abc"));
        assert_eq!(parse_tree_query("q=").unwrap().query.as_deref(), Some(""));
    }

    #[test]
    fn tree_query_rejects_noncanonical_ambiguous_and_unknown_inputs() {
        for query in [
            "limit",
            "=1",
            "unknown=1",
            "q=a&q=b",
            "q=a&%71=b",
            "limit=",
            "cursor=",
            "limit=01",
            "limit=+1",
            "limit=%2B1",
            "limit=0",
            "limit=101",
            "limit=-1",
            "limit=1.0",
            "q=%00",
            "q=%FF",
            "q=%",
            "q=a&&limit=1",
        ] {
            assert!(
                matches!(parse_tree_query(query), Err(EdgeError::BadRequest(_))),
                "query must fail closed: {query}"
            );
        }
    }

    #[test]
    fn tree_query_component_and_total_byte_limits_are_exact() {
        parse_tree_query(&format!("q={}", "x".repeat(TREE_PAGE_MAX_QUERY_BYTES)))
            .expect("exact q bound");
        parse_tree_query(&format!("cursor={}", "x".repeat(TREE_MAX_CURSOR_BYTES)))
            .expect("exact cursor bound");
        for query in [
            format!("q={}", "x".repeat(TREE_PAGE_MAX_QUERY_BYTES + 1)),
            format!("cursor={}", "x".repeat(TREE_MAX_CURSOR_BYTES + 1)),
            "x".repeat(TREE_MAX_QUERY_BYTES + 1),
        ] {
            assert!(matches!(
                parse_tree_query(&query),
                Err(EdgeError::BadRequest(_))
            ));
        }
    }

    #[test]
    fn tree_cursor_errors_map_to_scoped_statuses() {
        for error in [
            TreePageError::MalformedCursor,
            TreePageError::CursorScopeMismatch,
            TreePageError::InvalidQuery,
            TreePageError::InvalidLimit { supplied: 0 },
        ] {
            assert_eq!(map_tree_page_err(error).status(), 400);
        }
        assert_eq!(map_tree_page_err(TreePageError::CursorStale).status(), 409);
        assert_eq!(
            map_tree_page_err(TreePageError::Durable(DurableError::NotFound(
                "missing".into()
            )))
            .status(),
            404
        );
    }

    #[test]
    fn tree_capacity_errors_are_sanitized_payload_too_large_responses() {
        for private in [
            "tree page limit exceeded: tree object is larger than 8388608 bytes",
            "tree page limit exceeded: scanned entry count",
            "tree page limit exceeded: one entry name",
            "tree page limit exceeded: name bytes",
        ] {
            let mapped =
                map_tree_page_err(TreePageError::Durable(DurableError::Git(private.into())));
            assert_eq!(mapped.status(), 413);
            assert_eq!(
                mapped.to_string(),
                "413 (payload_too_large): repository tree exceeds the interactive browse limit"
            );
        }
    }

    #[test]
    fn non_capacity_tree_errors_keep_their_existing_classification() {
        assert_eq!(
            map_tree_page_err(TreePageError::Durable(DurableError::Git(
                "tree path segment is invalid".into(),
            )))
            .status(),
            400
        );
        assert_eq!(
            map_tree_page_err(TreePageError::Durable(DurableError::Git(
                "tree object has the wrong kind".into(),
            )))
            .status(),
            500
        );
    }
}

#[cfg(test)]
mod tree_page_backend_tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    use base64::Engine as _;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdRegion, TenantId};

    use super::*;
    use crate::catalogue::{test_request_identity, Page};
    use crate::repo_authz::GrantBackedRepos;
    use crate::request::EdgeRequest;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const TENANT: &str = "tree-page-tenant";
    const REGION: &str = "eu-north";

    struct Fixture {
        root: PathBuf,
        be: DurableGitBackend,
        repo: DurableGitRepo,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "myelin-edge-tree-page-{label}-{}-{sequence}",
                std::process::id()
            ));
            let be = DurableGitBackend::rooted_inmem_for_test(&root);
            let loc = DurableGitBackend::loc(TENANT, REGION, label);
            let repo = be.store.create_repo(&loc).expect("create repo");
            Self { root, be, repo }
        }

        fn commit_shared_files(
            &self,
            count: usize,
            parents: &[&CoreOid],
            message: &str,
        ) -> (CoreOid, CoreOid) {
            let blob = self.repo.write_blob(b"page\n").expect("blob");
            let names = (0..count)
                .map(|index| format!("file-{index:04}.txt"))
                .collect::<Vec<_>>();
            let entries = names
                .iter()
                .map(|name| (name.as_str(), &blob))
                .collect::<Vec<_>>();
            let tree = self.repo.write_tree(&entries).expect("tree");
            let commit = self
                .repo
                .write_commit(
                    &tree,
                    parents,
                    message,
                    "psn@tenant.noreply",
                    "psn@tenant.noreply",
                )
                .expect("commit");
            (tree, commit)
        }

        fn commit_named_files(
            &self,
            files: &[(&str, &[u8])],
            parents: &[&CoreOid],
            message: &str,
        ) -> (CoreOid, CoreOid) {
            let blobs = files
                .iter()
                .map(|(name, bytes)| ((*name).to_string(), self.repo.write_blob(bytes).unwrap()))
                .collect::<Vec<_>>();
            let entries = blobs
                .iter()
                .map(|(name, oid)| (name.as_str(), oid))
                .collect::<Vec<_>>();
            let tree = self.repo.write_tree(&entries).expect("tree");
            let commit = self
                .repo
                .write_commit(
                    &tree,
                    parents,
                    message,
                    "psn@tenant.noreply",
                    "psn@tenant.noreply",
                )
                .expect("commit");
            (tree, commit)
        }

        fn create_main(&self, commit: &CoreOid) {
            self.repo
                .update_ref_cas(
                    "refs/heads/main",
                    None,
                    Some(commit),
                    "create main",
                    "psn@tenant.noreply",
                )
                .expect("create main");
        }

        fn move_main(&self, old: &CoreOid, new: &CoreOid) {
            self.repo
                .update_ref_cas(
                    "refs/heads/main",
                    Some(old),
                    Some(new),
                    "move main",
                    "psn@tenant.noreply",
                )
                .expect("move main");
        }

        fn tree_json(&self, request: TreePageRequest) -> Result<Value, TreePageError> {
            self.be
                .tree_json(TENANT, REGION, self.label(), "refs/heads/main", "", request)
        }

        fn label(&self) -> &str {
            self.repo
                .path()
                .file_stem()
                .and_then(|name| name.to_str())
                .expect("repo slug")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn oid_bytes(oid: &CoreOid) -> [u8; 20] {
        let mut bytes = [0_u8; 20];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&oid.as_str()[index * 2..index * 2 + 2], 16).expect("hex oid");
        }
        bytes
    }

    fn forge_cursor_oids(cursor: &str, snapshot: &CoreOid, tree: &CoreOid) -> String {
        let encoded = cursor.strip_prefix("gt1_").expect("tree cursor prefix");
        let mut frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .expect("tree cursor frame");
        frame[1..21].copy_from_slice(&oid_bytes(snapshot));
        frame[21..41].copy_from_slice(&oid_bytes(tree));
        format!(
            "gt1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            IdRegion(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn serve_tree(
        handler: &dyn Handler,
        viewer: &Principal,
        slug: &str,
        query: &str,
    ) -> Result<EdgeResponse, EdgeError> {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let params = BTreeMap::from([
            ("repo".to_string(), slug.to_string()),
            ("ref".to_string(), "refs/heads/main".to_string()),
            ("path".to_string(), String::new()),
        ]);
        let request = EdgeRequest::new(
            "GET",
            "/v1/git/repos/core/tree/refs%2Fheads%2Fmain/",
            query,
            vec![],
            vec![],
        );
        let page = Page::from_request(&request);
        let identity = test_request_identity(viewer, &scope);
        handler.handle(&HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &params,
            page: &page,
            request: &request,
        })
    }

    #[test]
    fn tree_pull_guard_denies_before_malformed_query_or_cursor_parsing() {
        let fixture = Fixture::new("guard-order");
        let tree = fixture.repo.write_tree(&[]).expect("empty tree");
        let commit = fixture
            .repo
            .write_commit(
                &tree,
                &[],
                "guarded tree",
                "psn@tenant.noreply",
                "psn@tenant.noreply",
            )
            .expect("commit");
        fixture.create_main(&commit);

        let reader = human("u:reader");
        let stranger = human("u:stranger");
        let authorizer = GrantBackedRepos::new().grant_read("u:reader", TENANT, fixture.label());
        let be = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&fixture.root)
                .with_repo_authorizer(Arc::new(authorizer)),
        );
        let handler = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DTree { be: be.clone() }),
        );

        for query in ["cursor=%", "cursor=not-a-tree-cursor"] {
            let denied = match serve_tree(&*handler, &stranger, fixture.label(), query) {
                Err(error) => error,
                Ok(_) => panic!("the pull guard must deny before DTree parses the query"),
            };
            assert!(
                matches!(denied, EdgeError::NotFound(_)) && denied.status() == 404,
                "ungranted malformed tree request must be 0-leak 404: {denied:?}"
            );

            let admitted = match serve_tree(&*handler, &reader, fixture.label(), query) {
                Err(error) => error,
                Ok(_) => panic!("a granted reader must reach DTree's strict parser"),
            };
            assert!(
                matches!(admitted, EdgeError::BadRequest(_)) && admitted.status() == 400,
                "granted malformed tree request must reach DTree and return 400: {admitted:?}"
            );
        }
    }

    #[test]
    fn repo_home_pages_more_than_one_thousand_rows_with_its_qualified_continuation_ref() {
        let fixture = Fixture::new("wide");
        let (_, commit) = fixture.commit_shared_files(1_001, &[], "wide root");
        fixture.create_main(&commit);

        let home = fixture
            .be
            .repo_home_json(TENANT, REGION, fixture.label())
            .expect("repo home");
        assert_eq!(home["state"], "populated");
        assert_eq!(home["entries"].as_array().unwrap().len(), 100);
        assert_eq!(home["entries_page"]["limit"], 100);
        assert_eq!(home["entries_page"]["ref"], "refs/heads/main");
        assert_eq!(home["entries_page"]["snapshot_oid"], commit.as_str());
        let mut names = home["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        let mut cursor = home["entries_page"]["next_cursor"]
            .as_str()
            .map(str::to_string);
        while let Some(next) = cursor {
            let page = fixture
                .tree_json(TreePageRequest {
                    limit: 100,
                    cursor: Some(next),
                    ..TreePageRequest::default()
                })
                .expect("qualified continuation");
            names.extend(
                page["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| entry["name"].as_str().unwrap().to_string()),
            );
            cursor = page["page"]["next_cursor"].as_str().map(str::to_string);
        }
        assert_eq!(names.len(), 1_001);
        assert_eq!(names.first().unwrap(), "file-0000.txt");
        assert_eq!(names.last().unwrap(), "file-1000.txt");
    }

    #[test]
    fn branch_movement_is_a_typed_stale_tree_cursor() {
        let fixture = Fixture::new("stale");
        let (_, first) = fixture.commit_shared_files(3, &[], "first");
        fixture.create_main(&first);
        let first_page = fixture
            .tree_json(TreePageRequest {
                limit: 1,
                ..TreePageRequest::default()
            })
            .expect("first page");
        let cursor = first_page["page"]["next_cursor"]
            .as_str()
            .expect("cursor")
            .to_string();
        let (_, second) = fixture.commit_shared_files(4, &[&first], "second");
        fixture.move_main(&first, &second);

        let error = fixture
            .tree_json(TreePageRequest {
                limit: 1,
                cursor: Some(cursor),
                ..TreePageRequest::default()
            })
            .expect_err("moved branch must stale");
        assert_eq!(error, TreePageError::CursorStale);
        assert_eq!(map_tree_page_err(error).status(), 409);
    }

    #[test]
    fn forged_cursor_oids_cannot_select_unreachable_tree_objects() {
        let fixture = Fixture::new("forged");
        let (_, visible) = fixture.commit_shared_files(3, &[], "visible");
        fixture.create_main(&visible);
        let first_page = fixture
            .tree_json(TreePageRequest {
                limit: 1,
                ..TreePageRequest::default()
            })
            .expect("first page");
        let cursor = first_page["page"]["next_cursor"].as_str().unwrap();
        let (secret_tree, secret_commit) = fixture.commit_named_files(
            &[("secret.txt", b"unreachable secret\n")],
            &[],
            "unreachable",
        );
        let forged = forge_cursor_oids(cursor, &secret_commit, &secret_tree);

        let error = fixture
            .tree_json(TreePageRequest {
                limit: 1,
                cursor: Some(forged),
                ..TreePageRequest::default()
            })
            .expect_err("forged object ids are consistency-only");
        assert_eq!(error, TreePageError::CursorStale);
    }

    #[test]
    fn readme_is_present_only_on_the_first_unfiltered_tree_page() {
        let fixture = Fixture::new("readme");
        let (_, commit) = fixture.commit_named_files(
            &[("README.md", b"# snapshot readme\n"), ("z.txt", b"z\n")],
            &[],
            "readme",
        );
        fixture.create_main(&commit);
        let first = fixture
            .tree_json(TreePageRequest {
                limit: 1,
                ..TreePageRequest::default()
            })
            .expect("first page");
        assert_eq!(first["readme"], "# snapshot readme\n");
        let cursor = first["page"]["next_cursor"].as_str().unwrap().to_string();
        let continuation = fixture
            .tree_json(TreePageRequest {
                limit: 1,
                cursor: Some(cursor),
                ..TreePageRequest::default()
            })
            .expect("continuation");
        assert!(continuation.get("readme").is_none());
        let search = fixture
            .tree_json(TreePageRequest {
                query: Some("readme".into()),
                ..TreePageRequest::default()
            })
            .expect("search");
        assert!(search.get("readme").is_none());
    }

    #[test]
    fn committed_empty_tree_is_populated_and_exposes_a_terminal_entries_page() {
        let fixture = Fixture::new("empty-tree");
        let tree = fixture.repo.write_tree(&[]).expect("empty tree");
        let commit = fixture
            .repo
            .write_commit(
                &tree,
                &[],
                "empty snapshot",
                "psn@tenant.noreply",
                "psn@tenant.noreply",
            )
            .expect("empty commit");
        fixture.create_main(&commit);

        let home = fixture
            .be
            .repo_home_json(TENANT, REGION, fixture.label())
            .expect("repo home");
        assert_eq!(home["state"], "populated");
        assert_eq!(home["entries"], json!([]));
        assert!(home["entries_page"]["next_cursor"].is_null());
        assert!(matches!(
            fixture
                .be
                .repo_home(TENANT, REGION, fixture.label(), &fixture.repo)
                .expect("catalogue home"),
            RepoHome::Populated { entries, .. } if entries.is_empty()
        ));
    }

    #[test]
    fn repo_and_tree_entry_metadata_share_the_selected_snapshot() {
        let fixture = Fixture::new("snapshot-meta");
        let (_, first) = fixture.commit_named_files(&[("file.txt", b"first\n")], &[], "first");
        fixture.create_main(&first);
        let (_, second) =
            fixture.commit_named_files(&[("file.txt", b"second\n")], &[&first], "second");
        fixture.move_main(&first, &second);

        let tree = fixture
            .tree_json(TreePageRequest::default())
            .expect("tree page");
        assert_eq!(tree["snapshot_oid"], second.as_str());
        assert_eq!(tree["entries"][0]["latest_commit"]["oid"], second.as_str());
        let home = fixture
            .be
            .repo_home_json(TENANT, REGION, fixture.label())
            .expect("repo home");
        assert_eq!(home["snapshot_oid"], second.as_str());
        assert_eq!(home["entries_page"]["snapshot_oid"], second.as_str());
        assert_eq!(home["latest_commit"]["oid"], second.as_str());
        assert_eq!(home["entries"][0]["latest_commit"]["oid"], second.as_str());
    }
}

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
        let body = ctx.request.json_body()?;
        let expected_base = body
            .get("base_oid")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("commit body missing `base_oid`".into()))?;
        let contents = body
            .get("contents")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("commit body missing `contents`".into()))?;
        let start_ref = body
            .get("start_ref")
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    EdgeError::BadRequest("commit body `start_ref` must be a string".into())
                })
            })
            .transpose()?;
        let outcome = self
            .be
            .web_edit_commit(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                ),
                param(ctx, "ref")?,
                param(ctx, "path")?,
                expected_base,
                contents,
                start_ref,
            )
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
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
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
            .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
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
        let number = num_param(ctx, "n")?;
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
mod pr_commit_pagination_tests {
    use super::*;
    use crate::catalogue::Page;
    use crate::request::EdgeRequest;
    use base64::Engine as _;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_tenancy::{Region as IdentityRegion, TenantId};
    use std::collections::BTreeMap;

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";
    const REPO: &str = "core";

    struct Fixture {
        root: PathBuf,
        be: DurableGitBackend,
        viewer: Principal,
        base: CoreOid,
        second: CoreOid,
        third: CoreOid,
        head: CoreOid,
    }

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-pr-commit-pages-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn human(tenant: &str, region: &str, id: &str) -> Principal {
        Principal::new(
            TenantId(tenant.into()),
            IdentityRegion(region.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn fixture(tag: &str) -> Fixture {
        let root = temp_root(tag);
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let viewer = human(TENANT, REGION, "u:viewer");
        be.create_repo_as(TENANT, REGION, REPO, &viewer).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, REPO);
        let repo = be.store.open_repo(&loc).unwrap();
        let blob = repo.write_blob(b"snapshot\n").unwrap();
        let tree = repo.write_tree(&[("file.txt", &blob)]).unwrap();
        let base = repo
            .write_commit(&tree, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "base",
            "psn@acme.noreply",
        )
        .unwrap();
        let second = repo
            .write_commit(
                &tree,
                &[&base],
                "second",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        let third = repo
            .write_commit(
                &tree,
                &[&second],
                "third",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        let head = repo
            .write_commit(
                &tree,
                &[&third],
                "head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        be.open_pr(
            TENANT,
            REGION,
            REPO,
            &json!({
                "title": "Snapshot PR",
                "base_ref": "refs/heads/main",
                "head_ref": "refs/heads/feature",
                "head_oid": head.0,
            }),
            &viewer,
        )
        .unwrap();
        Fixture {
            root,
            be,
            viewer,
            base,
            second,
            third,
            head,
        }
    }

    fn serve(
        handler: &dyn Handler,
        viewer: &Principal,
        repo: &str,
        number: u64,
        query: &str,
    ) -> Result<EdgeResponse, EdgeError> {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let params = BTreeMap::from([
            ("repo".to_string(), repo.to_string()),
            ("n".to_string(), number.to_string()),
        ]);
        let request = EdgeRequest::new(
            "GET",
            format!("/v1/git/repos/{repo}/prs/{number}/commits"),
            query,
            vec![],
            vec![],
        );
        let page = Page::from_request(&request);
        let identity = crate::catalogue::test_request_identity(viewer, &scope);
        handler.handle(&HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &params,
            page: &page,
            request: &request,
        })
    }

    fn json(response: EdgeResponse) -> Value {
        response.json_body().expect("JSON response")
    }

    fn cursor_from(body: &Value) -> String {
        body["page"]["next_cursor"]
            .as_str()
            .expect("next cursor")
            .to_string()
    }

    fn mutate_cursor(cursor: &str, mutation: impl FnOnce(&mut [u8])) -> String {
        let encoded = cursor.strip_prefix("pc1_").unwrap();
        let mut frame = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        mutation(&mut frame);
        format!(
            "pc1_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(frame)
        )
    }

    #[test]
    fn strict_query_accepts_only_canonical_bounded_parameters() {
        let cursor = PrCommitCursor::new([4; 32], Some(&"1".repeat(40)), &"2".repeat(40), 1)
            .unwrap()
            .encode();
        for valid in [
            "".to_string(),
            "limit=1".to_string(),
            format!("cursor={cursor}"),
            format!("limit=100&cursor={cursor}"),
            format!("cursor={cursor}&limit=2"),
        ] {
            assert!(
                parse_pr_commit_query(&valid).is_ok(),
                "valid query: {valid}"
            );
        }

        let wrong_version = mutate_cursor(&cursor, |frame| frame[0] = 2);
        let overflow = mutate_cursor(&cursor, |frame| {
            frame[74..78].copy_from_slice(&u32::MAX.to_be_bytes())
        });
        for malformed in [
            "limit".to_string(),
            "cursor".to_string(),
            "limit=".to_string(),
            "cursor=".to_string(),
            "limit=0".to_string(),
            "limit=01".to_string(),
            "limit=101".to_string(),
            "limit=1&limit=2".to_string(),
            format!("cursor={cursor}&cursor={cursor}"),
            "unknown=x".to_string(),
            "limit=1&".to_string(),
            "=1".to_string(),
            format!("cursor={cursor}="),
            format!("cursor={wrong_version}"),
            format!("cursor={overflow}"),
            format!("cursor=pc1_{}", "a".repeat(256)),
            "x".repeat(PR_COMMIT_QUERY_MAX_BYTES + 1),
        ] {
            assert!(
                matches!(
                    parse_pr_commit_query(&malformed),
                    Err(EdgeError::BadRequest(_))
                ),
                "malformed query must be rejected: {malformed}"
            );
        }
    }

    #[test]
    fn capacity_errors_are_a_sanitized_payload_too_large_response() {
        assert!(matches!(
            map_pr_commit_page_error(PrCommitPageError::CapacityExceeded),
            EdgeError::PayloadTooLarge(message)
                if message == "pull request commit history exceeds the interactive walk limit"
        ));
    }

    #[test]
    fn exact_pages_repeat_deterministically_and_keep_the_pinned_snapshot() {
        let fixture = fixture("pages");
        let handler = DPrCommits {
            be: Arc::new(fixture.be),
        };
        let first = json(serve(&handler, &fixture.viewer, REPO, 1, "limit=1").unwrap());
        assert_eq!(
            first
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["items", "page"]
        );
        assert_eq!(
            first["page"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["limit", "next_cursor"]
        );
        assert_eq!(first["items"].as_array().unwrap().len(), 1);
        assert_eq!(first["items"][0]["oid"], fixture.head.0);
        assert_eq!(first["page"]["limit"], 1);
        let first_cursor = cursor_from(&first);
        assert!(first_cursor.starts_with("pc1_"));

        let second_query = format!("cursor={first_cursor}&limit=1");
        let second = json(serve(&handler, &fixture.viewer, REPO, 1, &second_query).unwrap());
        let repeated = json(serve(&handler, &fixture.viewer, REPO, 1, &second_query).unwrap());
        assert_eq!(second, repeated, "the same cursor is deterministic");
        let second_oid = second["items"][0]["oid"].as_str().unwrap().to_string();

        let loc = DurableGitBackend::loc(TENANT, REGION, REPO);
        let repo = handler.be.store.open_repo(&loc).unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            Some(&fixture.base),
            Some(&fixture.second),
            "advance live base",
            "psn@acme.noreply",
        )
        .unwrap();
        let live_head = repo
            .write_commit(
                &repo.write_tree(&[]).unwrap(),
                &[&fixture.head],
                "new live head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        handler
            .be
            .prs
            .update(&loc, 1, |record| {
                record.head_oid = live_head.0.clone();
                Ok(())
            })
            .unwrap();

        let still_second = json(serve(&handler, &fixture.viewer, REPO, 1, &second_query).unwrap());
        assert_eq!(still_second["items"][0]["oid"], second_oid);
        let third_cursor = cursor_from(&second);
        let third = json(
            serve(
                &handler,
                &fixture.viewer,
                REPO,
                1,
                &format!("limit=1&cursor={third_cursor}"),
            )
            .unwrap(),
        );
        let third_oid = third["items"][0]["oid"].as_str().unwrap().to_string();
        assert!(third["page"]["next_cursor"].is_null());
        assert_eq!(
            std::collections::BTreeSet::from([fixture.head.0.clone(), second_oid, third_oid,]),
            std::collections::BTreeSet::from([fixture.head.0, fixture.third.0, fixture.second.0,]),
            "the pinned pages contain each PR-owned commit exactly once"
        );
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    #[test]
    fn scope_replay_and_expired_snapshots_fail_cleanly() {
        let fixture = fixture("scope");
        let handler = DPrCommits {
            be: Arc::new(fixture.be),
        };
        handler
            .be
            .open_pr(
                TENANT,
                REGION,
                REPO,
                &json!({
                    "title": "Second PR",
                    "base_ref": "refs/heads/main",
                    "head_ref": "refs/heads/other",
                    "head_oid": fixture.head.0,
                }),
                &fixture.viewer,
            )
            .unwrap();
        let first = json(serve(&handler, &fixture.viewer, REPO, 1, "limit=1").unwrap());
        let cursor = cursor_from(&first);

        assert!(matches!(
            serve(
                &handler,
                &fixture.viewer,
                REPO,
                2,
                &format!("cursor={cursor}"),
            ),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));

        let other_repo = "other";
        handler
            .be
            .create_repo_as(TENANT, REGION, other_repo, &fixture.viewer)
            .unwrap();
        handler
            .be
            .open_pr(
                TENANT,
                REGION,
                other_repo,
                &json!({
                    "title": "Other repo",
                    "base_ref": "refs/heads/main",
                    "head_ref": "refs/heads/feature",
                    "head_oid": "0".repeat(40),
                }),
                &fixture.viewer,
            )
            .unwrap();
        assert!(matches!(
            serve(
                &handler,
                &fixture.viewer,
                other_repo,
                1,
                &format!("cursor={cursor}"),
            ),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));

        for (tenant, region) in [("other-tenant", REGION), (TENANT, "other-region")] {
            let viewer = human(tenant, region, "u:viewer");
            handler
                .be
                .create_repo_as(tenant, region, REPO, &viewer)
                .unwrap();
            handler
                .be
                .open_pr(
                    tenant,
                    region,
                    REPO,
                    &json!({
                        "title": "Other scope",
                        "base_ref": "refs/heads/main",
                        "head_ref": "refs/heads/feature",
                        "head_oid": "0".repeat(40),
                    }),
                    &viewer,
                )
                .unwrap();
            assert!(matches!(
                serve(&handler, &viewer, REPO, 1, &format!("cursor={cursor}")),
                Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
            ));
        }

        let expired = PrCommitCursor::new(
            pr_commit_cursor_scope(TENANT, REGION, REPO, 1),
            None,
            &"f".repeat(40),
            1,
        )
        .unwrap()
        .encode();
        assert!(matches!(
            serve(
                &handler,
                &fixture.viewer,
                REPO,
                1,
                &format!("cursor={expired}"),
            ),
            Err(EdgeError::Conflict(message)) if message == "pull request commit cursor expired"
        ));
        let expired_base = PrCommitCursor::new(
            pr_commit_cursor_scope(TENANT, REGION, REPO, 1),
            Some(&"e".repeat(40)),
            &fixture.head.0,
            1,
        )
        .unwrap()
        .encode();
        assert!(matches!(
            serve(
                &handler,
                &fixture.viewer,
                REPO,
                1,
                &format!("cursor={expired_base}"),
            ),
            Err(EdgeError::Conflict(message)) if message == "pull request commit cursor expired"
        ));
        std::fs::remove_dir_all(&fixture.root).ok();
    }

    #[test]
    fn pull_denial_precedes_malformed_cursor_parsing() {
        let fixture = fixture("guard-order");
        let denied = Arc::new(fixture.be.with_repo_authorizer(Arc::new(DenyAllRepos)));
        let handler = guarded(
            &denied,
            RepoPermission::Pull,
            Arc::new(DPrCommits { be: denied.clone() }),
        );
        assert!(matches!(
            serve(&*handler, &fixture.viewer, REPO, 1, "cursor=malformed"),
            Err(EdgeError::NotFound(message)) if message == "repository not found"
        ));
        std::fs::remove_dir_all(&fixture.root).ok();
    }
}

struct DPrDiff {
    be: Arc<DurableGitBackend>,
}
impl Handler for DPrDiff {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let vm = self
            .be
            .pr_diff(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
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

fn decode_form_query_component(raw: &str, subject: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(format!(
                    "{subject} query contains malformed percent encoding"
                )));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    let form_value = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&form_value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|_| EdgeError::BadRequest(format!("{subject} query is not valid UTF-8")))
}

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
                let number = value.parse::<u64>().map_err(|_| {
                    EdgeError::BadRequest(format!(
                        "file-lines `{name}` must be a positive line number"
                    ))
                })?;
                if number == 0 || number > u32::MAX as u64 {
                    return Err(EdgeError::BadRequest(format!(
                        "file-lines `{name}` must be a positive line number"
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
                num_param(ctx, "n")?,
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
        let body = ctx.request.json_body()?;
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
                .for_pr(num_param(ctx, "n")?),
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
        let body = ctx.request.json_body()?;
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
                .for_pr(num_param(ctx, "n")?),
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
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let vm = self
            .be
            .resolve_thread(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                param(ctx, "tid")?,
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
        let vm = self
            .be
            .start_review_batch(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                ctx.principal,
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
        let body = ctx.request.json_body()?;
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
                .for_pr(num_param(ctx, "n")?),
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
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let vm = self
            .be
            .submit_review_batch(
                RepoActorContext::new(
                    tenant_of(ctx),
                    region_of(ctx),
                    param(ctx, "repo")?,
                    ctx.principal,
                )
                .for_pr(num_param(ctx, "n")?),
                param(ctx, "rid")?,
                &body,
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
        let vm = self
            .be
            .discard_review_batch(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
                param(ctx, "rid")?,
                ctx.principal,
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
            .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
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
        let body = ctx.request.json_body()?;
        let verdict = body
            .get("verdict")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("review body missing `verdict`".into()))?;
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
                .for_pr(num_param(ctx, "n")?),
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
                .for_pr(num_param(ctx, "n")?),
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
        let operation_id = self.be.request_operation_id(ctx.request, ctx.principal)?;
        let attempt = self
            .be
            .merge_with_operation(
                tenant_of(ctx),
                region_of(ctx),
                param(ctx, "repo")?,
                num_param(ctx, "n")?,
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
                    .pr_get(&loc, num_param(ctx, "n")?, ctx.principal)
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
                .for_pr(num_param(ctx, "n")?),
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
mod code_search_boundary_tests {
    use super::*;
    use crate::catalogue::Page;
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdentityRegion, TenantId as IdentityTenantId};
    use std::collections::BTreeMap;

    fn search_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "myelin-code-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn principal() -> Principal {
        Principal::new(
            IdentityTenantId("acme".into()),
            IdentityRegion("eu-west".into()),
            PrincipalId("u:searcher".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn serve(be: Arc<DurableGitBackend>, query: &str) -> Result<EdgeResponse, EdgeError> {
        let principal = principal();
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        let params = BTreeMap::new();
        let request = EdgeRequest::new("GET", "/v1/git/search/code", query, vec![], vec![]);
        let page = Page::from_request(&request);
        let identity = crate::catalogue::test_request_identity(&principal, &scope);
        DCodeSearch { be }.handle(&HandlerCtx {
            identity: &identity,
            principal: &principal,
            scope: &scope,
            params: &params,
            page: &page,
            request: &request,
        })
    }

    #[test]
    fn code_search_decodes_form_components_without_splitting_encoded_injection() {
        assert_eq!(
            parse_code_search_query(
                "q=two%20words%20%26%20100%25%20%3D%20na%C3%AFve&repo=team%2Fcore"
            )
            .unwrap(),
            ("two words & 100% = naïve".into(), Some("team/core".into()))
        );
        assert_eq!(
            parse_code_search_query("q=x%26limit%3D100").unwrap(),
            ("x&limit=100".into(), None),
            "an encoded ampersand stays inside q rather than becoming a limit parameter"
        );
        assert!(parse_code_search_query("q=x&limit=100").is_err());

        let maximum = format!(
            "q={}",
            "x".repeat(myelin_git::api::CODE_SEARCH_QUERY_MAX_BYTES)
        );
        assert!(parse_code_search_query(&maximum).is_ok());
    }

    #[test]
    fn code_search_rejects_every_malformed_or_unbounded_coordinate() {
        for query in [
            "",
            "q",
            "=x",
            "repo=core",
            "q=",
            "q=+++",
            "q=x&q=y",
            "q=x&%71=y",
            "q=x&repo=core&repo=other",
            "q=x&unknown=value",
            "q=x&limit=100",
            "q=x&cursor=opaque",
            "q=%",
            "q=%0",
            "q=%GG",
            "q=%FF",
            "q=%00",
            "q=x&repo=",
            "q=x&repo=..%2Fsecret",
            "q=x&repo=team%2F%2Fcore",
            "q=x&repo=team%5Ccore",
        ] {
            assert!(
                matches!(
                    parse_code_search_query(query),
                    Err(EdgeError::BadRequest(_))
                ),
                "malformed code-search query should be a 400: {query:?}"
            );
        }

        let oversized_search = format!(
            "q={}",
            "x".repeat(myelin_git::api::CODE_SEARCH_QUERY_MAX_BYTES + 1)
        );
        assert!(parse_code_search_query(&oversized_search).is_err());
        let oversized_repo = format!(
            "q=x&repo={}",
            "r".repeat(myelin_git::api::CODE_SEARCH_REPO_MAX_BYTES + 1)
        );
        assert!(parse_code_search_query(&oversized_repo).is_err());
        let oversized_raw = format!("q=x&{}", "a".repeat(CODE_SEARCH_MAX_RAW_QUERY_BYTES));
        assert!(parse_code_search_query(&oversized_raw).is_err());
    }

    #[test]
    fn code_search_reads_the_authorized_default_branch_without_repo_probing() {
        let root = search_root();
        let be = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
        let principal = principal();
        be.create_repo_as("acme", "eu-west", "core", &principal)
            .unwrap();
        let repo = be
            .store
            .open_repo(&DurableGitBackend::loc("acme", "eu-west", "core"))
            .unwrap();
        let blob = repo.write_blob(b"first line\nneedle in code\n").unwrap();
        let tree = repo.write_tree(&[("app.rs", &blob)]).unwrap();
        let commit = repo
            .write_commit(&tree, &[], "seed", "searcher", "searcher")
            .unwrap();
        repo.update_ref_cas("refs/heads/main", None, Some(&commit), "seed", "searcher")
            .unwrap();

        let response = serve(be.clone(), "repo=core&q=needle").unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json_body().unwrap()["items"],
            json!([{
                "repo": "core",
                "ref": "refs/heads/main",
                "snapshot_oid": commit.as_str(),
                "path": "app.rs",
                "line": 2,
                "excerpt": "needle in code",
            }])
        );

        let missing = serve(be, "repo=missing-but-valid&q=needle").unwrap();
        assert_eq!(missing.status(), 200);
        assert_eq!(missing.json_body().unwrap()["items"], json!([]));
        std::fs::remove_dir_all(root).ok();
    }
}

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
        let number = num_param(ctx, "n")?;
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
        let number = num_param(ctx, "n")?;
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
        let pattern = match (ep.method, ep.path) {
            (GitMethod::Get, "/api/git/repos/{repo}/blob/{ref}/{path}") => {
                reroot("/api/git/repos/{repo}/blob/{ref}/{...path}")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/blame/{ref}/{path}") => {
                reroot("/api/git/repos/{repo}/blame/{ref}/{...path}")
            }
            (GitMethod::Post, "/api/git/repos/{repo}/blob/{ref}/{path}") => {
                reroot("/api/git/repos/{repo}/blob/{ref}/{...path}")
            }
            _ => reroot(ep.path),
        };
        let method = map_method(ep.method);
        let (handler, action): (Arc<dyn Handler>, &'static str) = match (ep.method, ep.path) {
            (GitMethod::Get, "/api/git/repos") => {
                (Arc::new(DRepoList { be: be.clone() }), "git.repos.list")
            }
            (GitMethod::Post, "/api/git/repos") => {
                (Arc::new(DRepoCreate { be: be.clone() }), "git.repo.create")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}") => (
                pr_read_guarded(&be, Arc::new(DPrOverview { be: be.clone() })),
                "git.pr.view",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}/checks") => (
                pr_read_guarded(&be, Arc::new(DPrChecks { be: be.clone() })),
                "git.pr.checks",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/blob/{ref}/{path}") => {
                (Arc::new(DBlobView { be: be.clone() }), "git.blob.view")
            }
            (GitMethod::Get, "/api/git/repos/{repo}/blame/{ref}/{path}") => (
                guarded(&be, Pull, Arc::new(DBlameView { be: be.clone() })),
                "git.blame.view",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/blob/{ref}/{path}") => (
                guarded(&be, Push, Arc::new(DWebEditCommit { be: be.clone() })),
                "git.blob.commit",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs") => (
                guarded(&be, Push, Arc::new(DOpenPr { be: be.clone() })),
                "git.pr.open",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/reviews") => (
                pr_review_guarded(&be, Arc::new(DPrReview { be: be.clone() })),
                "git.pr.review",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci") => (
                guarded(
                    &be,
                    ApproveUntrustedCi,
                    Arc::new(DEndorse { be: be.clone() }),
                ),
                "git.pr.endorse_fork_ci",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/merge") => (
                guarded(&be, ProtectedPush, Arc::new(DMerge { be: be.clone() })),
                "git.pr.merge",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/branch-protection") => (
                guarded(
                    &be,
                    ProtectedPush,
                    Arc::new(DSetBranchProtection { be: be.clone() }),
                ),
                "git.repo.branch_protection.set",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/checks") => (
                guarded(&be, Push, Arc::new(DReportChecks { be: be.clone() })),
                "git.checks.report",
            ),
            (GitMethod::Get, "/api/git/search/code") => {
                (Arc::new(DCodeSearch { be: be.clone() }), "git.search.code")
            }
            (_, other) => (
                Arc::new(DCodeSearch { be: be.clone() }),
                Box::leak(format!("git.unmapped:{other}").into_boxed_str()),
            ),
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
mod event_privacy_tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::Region as IdRegion;

    #[test]
    fn production_refstore_context_scrubs_all_raw_agent_identifiers() {
        let principal = Principal::new(
            myelin_tenancy::TenantId("acme".into()),
            IdRegion("fr-par".into()),
            PrincipalId("agent:raw@example.test".into()),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("runtime://raw-machine/session".into()),
                on_behalf_of: Some(PrincipalId("person@example.test".into())),
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let first = DurableGitBackend::emit_ctx("acme", "fr-par", &principal);
        let second = DurableGitBackend::emit_ctx("acme", "fr-par", &principal);
        assert_eq!(first.actor, second.actor, "the tenant pseudonym is stable");
        assert_ne!(first.actor.0.principal_id, principal.principal_id);
        let serialized = serde_json::to_string(&first.actor).unwrap();
        for raw in [
            "agent:raw@example.test",
            "runtime://raw-machine/session",
            "person@example.test",
        ] {
            assert!(
                !serialized.contains(raw),
                "raw Agent identifier leaked: {raw}"
            );
        }

        let request = RepoActorContext::new("acme", "fr-par", "core", &principal).for_pr(42);
        let debug = format!("{request:?}");
        assert!(!debug.contains("agent:raw@example.test"));
        assert!(debug.contains("principal: \"<redacted>\""));
    }

    #[test]
    fn fork_import_diagnostics_are_sanitized_before_public_boundaries() {
        let raw = DurableError::Git(
            "failed to index /srv/tenants/acme/private-fork.git: object secretdeadbeef".into(),
        );
        let public = sanitize_fork_import_error(raw).to_string();
        assert_eq!(
            public,
            "durable git op failed: fork commit import could not be completed"
        );
        assert!(!public.contains("/srv/tenants"));
        assert!(!public.contains("secretdeadbeef"));
    }
}

#[cfg(test)]
mod create_claim_tests {

    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::sync::{mpsc, Barrier, Mutex};
    use std::time::Duration;

    fn principal(id: &str, tenant: &str) -> Principal {
        Principal::new(
            TenantId(tenant.into()),
            IdRegion("fr-par".into()),
            PrincipalId(id.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-create-claim-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[derive(Default)]
    struct RecordingBootstrap {
        grants: Mutex<Vec<(String, String)>>,
    }

    impl RepoBootstrapGrants for RecordingBootstrap {
        fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
            self.grants
                .lock()
                .unwrap()
                .push((creator.principal_id.0.clone(), repo.repo.clone()));
            Ok(())
        }
    }

    struct CommitThenDisconnectBootstrap {
        grants: Mutex<Vec<(String, String)>>,
    }

    impl RepoBootstrapGrants for CommitThenDisconnectBootstrap {
        fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
            self.grants
                .lock()
                .unwrap()
                .push((creator.principal_id.0.clone(), repo.repo.clone()));
            Err("the durable grant committed, but its response was lost".into())
        }
    }

    struct PausingBootstrap {
        grants: Mutex<Vec<(String, String)>>,
        first_grant_entered: mpsc::Sender<()>,
        release_first_grant: Mutex<mpsc::Receiver<()>>,
    }

    impl RepoBootstrapGrants for PausingBootstrap {
        fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
            let is_first = {
                let mut grants = self.grants.lock().unwrap();
                grants.push((creator.principal_id.0.clone(), repo.repo.clone()));
                grants.len() == 1
            };
            if is_first {
                self.first_grant_entered.send(()).unwrap();
                self.release_first_grant
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .expect("the test releases the first durable grant");
            }
            Ok(())
        }
    }

    #[test]
    fn successful_create_grants_its_creator_once() {
        let root = temp_root("ok");
        let boot = Arc::new(RecordingBootstrap::default());
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");

        let created = be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect("create succeeds");
        assert!(created);
        assert_eq!(boot.grants.lock().unwrap().len(), 1, "granted once");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_ambiguous_grant_response_keeps_the_slug_with_its_original_creator() {
        let root = temp_root("resume");
        let creator = principal("svc:creator", "acme");
        let stranger = principal("svc:stranger", "acme");
        let interrupted_grant = Arc::new(CommitThenDisconnectBootstrap {
            grants: Mutex::new(Vec::new()),
        });
        let interrupted = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(interrupted_grant.clone());

        let interrupted_error = interrupted
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect_err("the caller lost the committed grant response");
        assert!(interrupted_error
            .to_string()
            .contains("owner-bound repository claim remains retryable"));
        assert_eq!(
            *interrupted_grant.grants.lock().unwrap(),
            vec![("svc:creator".to_string(), "widgets".to_string())]
        );

        let stranger_grants = Arc::new(RecordingBootstrap::default());
        let after_restart = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(stranger_grants.clone());
        let conflict = after_restart
            .create_repo_as("acme", "fr-par", "widgets", &stranger)
            .expect_err("another principal cannot adopt the interrupted slug");
        assert!(matches!(conflict, DurableError::Conflict(_)));
        assert_eq!(
            stranger_grants.grants.lock().unwrap().len(),
            0,
            "the stranger never reaches authorization"
        );

        let resumed_grants = Arc::new(RecordingBootstrap::default());
        let resumed = DurableGitBackend::rooted_inmem_for_test(&root)
            .with_repo_bootstrap(resumed_grants.clone());
        assert!(resumed
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .expect("the original creator resumes after restart"));
        assert_eq!(
            *resumed_grants.grants.lock().unwrap(),
            vec![("svc:creator".to_string(), "widgets".to_string())]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn simultaneous_creators_cannot_both_claim_the_same_repository() {
        let root = temp_root("concurrent");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let bootstrap = Arc::new(PausingBootstrap {
            grants: Mutex::new(Vec::new()),
            first_grant_entered: entered_tx,
            release_first_grant: Mutex::new(release_rx),
        });
        let backend = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(bootstrap.clone()),
        );
        let starting_line = Arc::new(Barrier::new(3));

        let spawn_creator = |id: &'static str| {
            let backend = backend.clone();
            let starting_line = starting_line.clone();
            std::thread::spawn(move || {
                let creator = principal(id, "acme");
                starting_line.wait();
                backend.create_repo_as("acme", "fr-par", "widgets", &creator)
            })
        };
        let first = spawn_creator("svc:alice");
        let second = spawn_creator("svc:bob");
        starting_line.wait();
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("one creator enters the grant while holding the claim");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            bootstrap.grants.lock().unwrap().len(),
            1,
            "the other creator waits outside the authorization boundary"
        );
        release_tx.send(()).unwrap();

        let mut outcomes = vec![
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        ];
        outcomes.sort_unstable();
        assert_eq!(outcomes, vec![false, true]);
        assert_eq!(
            bootstrap.grants.lock().unwrap().len(),
            1,
            "exactly the creator that initialized the repository received admin"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_existing_repo_does_not_grant_again() {
        let root = temp_root("exists");
        let boot = Arc::new(RecordingBootstrap::default());
        let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
        let creator = principal("svc:creator", "acme");
        assert!(be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .unwrap());
        assert!(!be
            .create_repo_as("acme", "fr-par", "widgets", &creator)
            .unwrap());
        assert_eq!(
            boot.grants.lock().unwrap().len(),
            1,
            "granted only on the first create"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod repo_summary_tests {
    use super::*;
    use crate::catalogue::Page;
    use crate::repo_authz::GrantBackedRepos;
    use crate::request::EdgeRequest;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::collections::BTreeMap;

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-repo-summary-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            IdRegion(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn serve(
        handler: &dyn Handler,
        viewer: &Principal,
        query: &str,
    ) -> Result<EdgeResponse, EdgeError> {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let params = BTreeMap::new();
        let request = EdgeRequest::new("GET", "/v1/git/repos", query, vec![], vec![]);
        let page = Page::from_request(&request);
        let identity = crate::catalogue::test_request_identity(viewer, &scope);
        handler.handle(&HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &params,
            page: &page,
            request: &request,
        })
    }

    fn json(response: EdgeResponse) -> Value {
        response.json_body().expect("JSON response")
    }

    fn create_repo(be: &DurableGitBackend, slug: &str, creator: &Principal) {
        be.create_repo_as(TENANT, REGION, slug, creator)
            .expect("create repository");
    }

    fn add_non_commit_main_target(be: &DurableGitBackend, slug: &str) {
        let loc = DurableGitBackend::loc(TENANT, REGION, slug);
        let repo = be.store.open_repo(&loc).expect("open repository");
        let tree = repo.write_tree(&[]).expect("write bare tree object");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&tree),
            "create deliberately non-commit main target",
            "psn@tenant.noreply",
        )
        .expect("create direct branch target");
    }

    #[test]
    fn summary_rows_have_exact_shapes_and_skip_legacy_home_reads() {
        let root = temp_root("exact-rows");
        let viewer = human("u:viewer");
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "empty")
            .grant_read("u:viewer", TENANT, "populated");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        create_repo(&be, "empty", &viewer);
        create_repo(&be, "populated", &viewer);
        add_non_commit_main_target(&be, "populated");
        let handler = DRepoList { be: Arc::new(be) };

        let body = json(serve(&handler, &viewer, "view=summary").expect("summary response"));
        assert_eq!(
            body,
            json!({
                "items": [
                    { "state": "empty", "slug": "acme/empty" },
                    {
                        "state": "populated",
                        "slug": "acme/populated",
                        "clone_url": "/acme/eu-west/populated.git",
                    },
                ],
                "page": { "next_cursor": null, "limit": DEFAULT_PAGE_LIMIT },
            })
        );
        assert!(
            serve(&handler, &viewer, "").is_err(),
            "the deliberately non-commit branch breaks legacy RepoHome, proving summary did not use it"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn authorization_precedes_keyset_paging_and_continuation_has_no_gaps_or_duplicates() {
        let root = temp_root("auth-before-page");
        let viewer = human("u:viewer");
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "gamma")
            .grant_read("u:viewer", TENANT, "omega");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        for slug in ["alpha", "beta", "gamma", "omega"] {
            create_repo(&be, slug, &viewer);
        }
        let handler = DRepoList { be: Arc::new(be) };

        let mut query = "view=summary&limit=1".to_string();
        let mut seen = Vec::new();
        loop {
            let body = json(serve(&handler, &viewer, &query).expect("summary page"));
            let items = body["items"].as_array().expect("items");
            seen.extend(
                items
                    .iter()
                    .map(|item| item["slug"].as_str().expect("slug").to_string()),
            );
            let Some(cursor) = body["page"]["next_cursor"].as_str() else {
                break;
            };
            assert!(cursor.starts_with(REPO_LIST_CURSOR_PREFIX));
            query = format!("view=summary&limit=1&cursor={cursor}");
        }
        assert_eq!(seen, ["acme/alpha", "acme/gamma", "acme/omega"]);
        assert!(!seen.iter().any(|slug| slug == "acme/beta"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn summary_query_is_strict_and_limits_are_canonical_and_bounded() {
        assert!(repo_summary_requested("limit=2&%76iew=summary"));
        assert_eq!(
            parse_repo_summary_query("%76iew=summary&limit=100")
                .unwrap()
                .limit,
            100
        );
        for query in [
            "view",
            "view=other",
            "view=summary&view=summary",
            "view=summary&unknown=x",
            "view=summary&unknown=%GG",
            "view=summary%0A",
            "view=summary&limit=0",
            "view=summary&limit=01",
            "view=summary&limit=101",
            "view=summary&cursor=x",
        ] {
            assert!(
                matches!(
                    parse_repo_summary_query(query),
                    Err(EdgeError::BadRequest(_))
                ),
                "query should be rejected: {query}"
            );
        }
        assert!(matches!(
            parse_repo_summary_query(&format!(
                "view=summary&cursor=rl1_{}",
                "a".repeat(REPO_LIST_CURSOR_MAX_BYTES)
            )),
            Err(EdgeError::BadRequest(_))
        ));
    }

    #[test]
    fn cursor_is_canonical_bounded_and_scoped_to_verified_tenant_region() {
        let cursor = RepoListCursor::new(repo_summary_cursor_scope(TENANT, REGION), "alpha")
            .unwrap()
            .encode();
        let parsed = parse_repo_summary_cursor(&cursor, TENANT, REGION).expect("canonical cursor");
        assert_eq!(parsed.last_slug(), "alpha");
        for malformed in [
            "rl1_".to_string(),
            "rl1_not-base64!".to_string(),
            format!("{cursor}="),
        ] {
            assert!(matches!(
                parse_repo_summary_cursor(&malformed, TENANT, REGION),
                Err(EdgeError::BadRequest(_))
            ));
        }
        assert!(matches!(
            parse_repo_summary_cursor(&cursor, "other-tenant", REGION),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));
        assert!(matches!(
            parse_repo_summary_cursor(&cursor, TENANT, "other-region"),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));

        let root = temp_root("empty-scope");
        let viewer = human("u:viewer");
        let handler = DRepoList {
            be: Arc::new(DurableGitBackend::rooted_inmem_for_test(&root)),
        };
        let empty = json(serve(&handler, &viewer, "view=summary").expect("empty tenant summary"));
        assert_eq!(empty["items"], json!([]));
        let wrong_scope =
            RepoListCursor::new(repo_summary_cursor_scope("other-tenant", REGION), "alpha")
                .unwrap()
                .encode();
        assert!(matches!(
            serve(
                &handler,
                &viewer,
                &format!("view=summary&cursor={wrong_scope}")
            ),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn summary_response_and_candidate_cardinality_are_bounded() {
        let small = page_envelope(json!([]), None, DEFAULT_PAGE_LIMIT);
        assert_eq!(repo_summary_response(&small).unwrap().status(), 200);
        assert!(matches!(
            repo_summary_response(&json!({
                "items": ["x".repeat(REPO_SUMMARY_RESPONSE_MAX_BYTES)],
                "page": { "next_cursor": null, "limit": 1 },
            })),
            Err(EdgeError::PayloadTooLarge(_))
        ));

        let root = temp_root("candidate-cap");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let viewer = human("u:viewer");
        for slug in ["alpha", "beta"] {
            create_repo(&be, slug, &viewer);
        }
        assert!(matches!(
            be.scan_repo_slugs_bounded(TENANT, REGION, 1),
            Err(DurableError::Git(message))
                if message == "browse response limit exceeded: repository candidate count"
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn summary_capacity_errors_use_catalogue_specific_sanitized_text() {
        let mapped = map_repo_summary_durable_err(DurableError::Git(
            "wire ref limit exceeded: private branch detail".into(),
        ));
        assert_eq!(mapped.status(), 413);
        assert_eq!(
            mapped.to_string(),
            "413 (payload_too_large): repository catalogue exceeds the interactive list limit"
        );

        let delegated = map_repo_summary_durable_err(DurableError::NotFound("missing".into()));
        assert_eq!(
            delegated,
            map_durable_err(DurableError::NotFound("missing".into())),
            "non-capacity errors retain the shared durable mapping"
        );
        assert_eq!(
            map_durable_err(DurableError::Git(
                "wire ref limit exceeded: private wire detail".into()
            ))
            .to_string(),
            "413 (payload_too_large): repository exceeds the smart-HTTP ref limit",
            "actual wire callers retain the established sanitized message"
        );
    }

    #[test]
    fn no_view_keeps_the_legacy_repo_home_projection_and_offset_cursor() {
        let root = temp_root("legacy-compat");
        let viewer = human("u:viewer");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "legacy");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        create_repo(&be, "legacy", &viewer);
        let handler = DRepoList { be: Arc::new(be) };

        let body = json(serve(&handler, &viewer, "limit=1").expect("legacy response"));
        assert_eq!(
            body,
            json!({
                "items": [{
                    "state": "empty",
                    "slug": "acme/legacy",
                    "clone_url": "/acme/eu-west/legacy.git",
                }],
                "page": { "next_cursor": null, "limit": 1 },
            })
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod pr_list_tests {

    use super::*;
    use crate::catalogue::Page;
    use crate::repo_authz::GrantBackedRepos;
    use crate::request::EdgeRequest;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::collections::BTreeMap;

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-prlist-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            IdRegion(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn open_pr(be: &DurableGitBackend, slug: &str, title: &str, opener: &Principal) {
        be.create_repo_as(TENANT, REGION, slug, opener).ok();
        let body = json!({
            "title": title,
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "0".repeat(40),
            "draft": false,
        });
        be.open_pr(TENANT, REGION, slug, &body, opener)
            .unwrap_or_else(|e| panic!("open PR in {slug}: {e:?}"));
    }

    fn repo_pr_bytes(be: &DurableGitBackend, slug: &str, viewer: &Principal) -> usize {
        let loc = DurableGitBackend::loc(TENANT, REGION, slug);
        let records = be.pr_list(&loc, viewer).expect("read seeded PR records");
        serialized_pr_records_bytes(&records).expect("measure seeded PR records")
    }

    fn serve(handler: &dyn Handler, viewer: &Principal, repo: Option<&str>, query: &str) -> Value {
        serve_result(handler, viewer, repo, query)
            .unwrap_or_else(|e| panic!("handler errored: {e:?}"))
    }

    fn serve_result(
        handler: &dyn Handler,
        viewer: &Principal,
        repo: Option<&str>,
        query: &str,
    ) -> Result<Value, EdgeError> {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let mut params = BTreeMap::new();
        if let Some(r) = repo {
            params.insert("repo".to_string(), r.to_string());
        }
        let req = EdgeRequest::new("GET", "/v1/git/prs", query, vec![], vec![]);
        let page = Page::from_request(&req);
        let identity = crate::catalogue::test_request_identity(viewer, &scope);
        let ctx = HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &params,
            page: &page,
            request: &req,
        };
        handler
            .handle(&ctx)
            .map(|response| response.json_body().expect("json body"))
    }

    #[test]
    fn forged_or_over_cap_pr_list_cursor_is_a_clean_bad_request() {
        let root = temp_root("forged-cursor");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        open_pr(&be, "core", "Only PR", &viewer);
        let handler = DRepoPrList { be: Arc::new(be) };
        for cursor in [
            usize::MAX.to_string(),
            "10001".into(),
            "01".into(),
            "+1".into(),
        ] {
            let error = serve_result(
                &handler,
                &viewer,
                Some("core"),
                &format!("state=all&cursor={cursor}"),
            )
            .expect_err("a noncanonical or over-cap cursor must be rejected");
            assert_eq!(
                error,
                EdgeError::BadRequest("invalid pull request cursor".into())
            );
        }
        for query in [
            "state=unknown",
            "sort=oldest",
            "state=open&state=all",
            "sort=updated&sort=created",
            "cursor=1&cursor=2",
            "limit=1&limit=2",
            "unknown=value",
            "limit",
            "=value",
            "limit=01",
            "limit=0",
            "limit=101",
            "limit=%ZZ",
            "cursor=",
        ] {
            assert!(matches!(
                serve_result(&handler, &viewer, Some("core"), query),
                Err(EdgeError::BadRequest(_))
            ));
        }
        let oversized = format!("state=open&x={}", "a".repeat(PR_LIST_QUERY_MAX_BYTES));
        assert!(matches!(
            serve_result(&handler, &viewer, Some("core"), &oversized),
            Err(EdgeError::BadRequest(_))
        ));
        let body = serve(&handler, &viewer, Some("core"), "state=all&cursor=10000");
        assert_eq!(
            body["items"].as_array().unwrap().len(),
            0,
            "the capped out-of-range coordinate is an empty page"
        );
        assert_eq!(body["counts"]["all"], 1, "empty pages retain exact badges");
        assert_eq!(body["page"]["total"], 1);
        assert!(
            body["page"]["next_cursor"].is_null(),
            "no next past the end"
        );

        let capped = repo_pr_list_envelope(EnrichedPrSlice {
            rows: Vec::new(),
            counts: PrListCounts::default(),
            total: PR_LIST_OFFSET_MAX + 1,
            offset: PR_LIST_OFFSET_MAX,
            limit: 100,
            next_cursor: None,
            prev_cursor: None,
        });
        assert!(
            capped["page"]["next_cursor"].is_null(),
            "the transitional ceiling never emits a cursor its strict parser will reject"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repository_list_pages_visible_slugs_before_building_view_models() {
        let root = temp_root("repo-page");
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "beta")
            .grant_read("u:viewer", TENANT, "gamma");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        for slug in ["alpha", "beta", "gamma"] {
            be.create_repo_as(TENANT, REGION, slug, &viewer).unwrap();
        }

        let handler = DRepoList { be: Arc::new(be) };
        let body = serve(&handler, &viewer, None, "limit=1&cursor=1");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["slug"], "acme/beta");
        assert_eq!(body["page"]["next_cursor"], "2");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn repository_candidate_scan_stops_before_unbounded_materialization() {
        let root = temp_root("repo-scan-bound");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let viewer = human("u:viewer");
        for slug in ["alpha", "beta"] {
            be.create_repo_as(TENANT, REGION, slug, &viewer).unwrap();
        }

        let error = be
            .scan_repo_slugs_bounded(TENANT, REGION, 1)
            .expect_err("the second repository must trip the candidate ceiling");
        assert!(matches!(
            error,
            DurableError::Git(message)
                if message == "browse response limit exceeded: repository candidate count"
        ));
        assert_eq!(
            be.scan_repo_slugs_bounded(TENANT, REGION, 2).unwrap(),
            vec!["alpha".to_string(), "beta".to_string()]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversized_title_is_rejected_at_create() {
        let root = temp_root("title-cap");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "core", &author).unwrap();
        let body = json!({
            "title": "x".repeat(513),
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "0".repeat(40),
        });
        let err = be.open_pr(TENANT, REGION, "core", &body, &author);
        assert!(err.is_err(), "513-byte title must be rejected");
        let ok_body = json!({
            "title": "x".repeat(512),
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
            "head_oid": "0".repeat(40),
        });
        assert!(be
            .open_pr(TENANT, REGION, "core", &ok_body, &author)
            .is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cross_repo_bucket_never_leaks_a_forbidden_repos_pr() {
        let root = temp_root("cross-leak");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let viewer = human("u:viewer");
        open_pr(&be, "alpha", "Alpha change", &viewer);
        open_pr(&be, "beta", "Beta change (forbidden repo)", &viewer);
        let be = be.with_repo_authorizer(Arc::new(authz));

        let handler = DMyPrs { be: Arc::new(be) };
        let body = serve(&handler, &viewer, None, "bucket=yours");
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "only the visible repo's PR is listed");
        assert_eq!(items[0]["repo"], "alpha");
        assert_eq!(items[0]["title"], "Alpha change");
        assert_eq!(body["counts"]["bucket"], 1);
        assert_eq!(body["page"]["total"], 1);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cross_repo_query_is_strict_and_fs_order_has_repo_tie_breaker() {
        let root = temp_root("cross-strict-query");
        let viewer = human("u:viewer");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        open_pr(&be, "beta", "Beta", &viewer);
        open_pr(&be, "alpha", "Alpha", &viewer);
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "beta");
        let handler = DMyPrs {
            be: Arc::new(be.with_repo_authorizer(Arc::new(authz))),
        };
        for query in [
            "bucket=unknown",
            "bucket=yours&bucket=needs-review",
            "sort=oldest",
            "sort=updated&sort=created",
            "cursor=01",
            "cursor=10001",
            "cursor=1&cursor=2",
            "limit=0",
            "limit=01",
            "limit=101",
            "limit=1&limit=2",
            "unknown=value",
            "limit",
            "limit=%ZZ",
        ] {
            assert!(matches!(
                serve_result(&handler, &viewer, None, query),
                Err(EdgeError::BadRequest(_))
            ));
        }

        let first = serve(&handler, &viewer, None, "bucket=yours&sort=created&limit=1");
        assert_eq!(first["counts"]["bucket"], 2);
        assert_eq!(first["items"][0]["repo"], "alpha");
        let next = first["page"]["next_cursor"].as_str().unwrap();
        assert!(next.starts_with(PR_LIST_CURSOR_PREFIX));
        let second = serve(&handler, &viewer, None, &format!("cursor={next}"));
        assert_eq!(second["items"][0]["repo"], "beta");
        assert!(second["page"]["next_cursor"].is_null());

        let capped = cross_pr_list_envelope(EnrichedCrossPrSlice {
            rows: Vec::new(),
            total: PR_LIST_OFFSET_MAX + 1,
            offset: PR_LIST_OFFSET_MAX,
            limit: 100,
            next_cursor: None,
            prev_cursor: None,
        });
        assert!(capped["page"]["next_cursor"].is_null());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_visible_cross_repo_set_is_exact_empty() {
        let root = temp_root("cross-empty-visible");
        let viewer = human("u:viewer");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        open_pr(&be, "hidden", "Hidden", &viewer);
        let handler = DMyPrs {
            be: Arc::new(be.with_repo_authorizer(Arc::new(GrantBackedRepos::new()))),
        };
        let body = serve(&handler, &viewer, None, "bucket=yours");
        assert!(body["items"].as_array().unwrap().is_empty());
        assert_eq!(body["counts"]["bucket"], 0);
        assert_eq!(body["page"]["total"], 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cross_repo_record_ceiling_applies_across_visible_repositories() {
        let root = temp_root("cross-record-cap");
        let viewer = human("u:viewer");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        for slug in ["alpha", "beta"] {
            open_pr(&be, slug, &format!("{slug} one"), &viewer);
            open_pr(&be, slug, &format!("{slug} two"), &viewer);
        }
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "beta");
        let be = be.with_repo_authorizer(Arc::new(authz));

        let error = be
            .list_prs_cross_bounded(
                TENANT,
                REGION,
                &viewer,
                CrossPrListLimits {
                    maximum_records: 3,
                    maximum_bytes: usize::MAX,
                },
            )
            .err()
            .expect("four collectively visible PRs must exceed a three-record request cap");
        assert!(matches!(
            &error,
            DurableError::Git(message)
                if message == "pull request list limit exceeded: cross-repository record count"
        ));
        assert_eq!(map_durable_err(error).status(), 413);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cross_repo_byte_ceiling_is_exact_and_aggregate() {
        let root = temp_root("cross-byte-cap");
        let viewer = human("u:viewer");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        open_pr(&be, "alpha", "Alpha payload", &viewer);
        open_pr(&be, "beta", "Beta payload", &viewer);
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "beta");
        let be = be.with_repo_authorizer(Arc::new(authz));
        let exact_bytes = repo_pr_bytes(&be, "alpha", &viewer)
            .checked_add(repo_pr_bytes(&be, "beta", &viewer))
            .unwrap();

        let error = be
            .list_prs_cross_bounded(
                TENANT,
                REGION,
                &viewer,
                CrossPrListLimits {
                    maximum_records: 2,
                    maximum_bytes: exact_bytes - 1,
                },
            )
            .err()
            .expect("the aggregate byte limit applies across repositories");
        assert!(matches!(
            error,
            DurableError::Git(message)
                if message == "pull request list limit exceeded: cross-repository serialized bytes"
        ));

        let exact = be
            .list_prs_cross_bounded(
                TENANT,
                REGION,
                &viewer,
                CrossPrListLimits {
                    maximum_records: 2,
                    maximum_bytes: exact_bytes,
                },
            )
            .expect("exactly-at-cap records and bytes remain available");
        assert_eq!(exact.len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn forbidden_oversized_repo_contributes_neither_work_nor_capacity() {
        let root = temp_root("cross-forbidden-cap");
        let viewer = human("u:viewer");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        open_pr(&be, "alpha", "Visible", &viewer);
        for index in 0..3 {
            open_pr(&be, "hidden", &format!("Hidden {index}"), &viewer);
        }
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha");
        let be = be.with_repo_authorizer(Arc::new(authz));
        let visible_bytes = repo_pr_bytes(&be, "alpha", &viewer);

        let rows = be
            .list_prs_cross_bounded(
                TENANT,
                REGION,
                &viewer,
                CrossPrListLimits {
                    maximum_records: 1,
                    maximum_bytes: visible_bytes,
                },
            )
            .expect("an oversized forbidden repository is excluded before PR reads");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].repo_slug.as_deref(), Some("alpha"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cross_repo_capacity_accounting_is_overflow_safe() {
        assert_eq!(
            checked_cross_pr_list_total(7, 3, 10, "cross-repository record count").unwrap(),
            10,
            "the exact ceiling is admitted"
        );
        assert!(matches!(
            checked_cross_pr_list_total(
                usize::MAX,
                1,
                usize::MAX,
                "cross-repository record count",
            ),
            Err(DurableError::Git(message))
                if message == "pull request list limit exceeded: cross-repository record count"
        ));
    }

    #[test]
    fn per_repo_list_rows_titles_and_counts() {
        let root = temp_root("per-repo");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        open_pr(&be, "core", "First PR", &viewer);
        open_pr(&be, "core", "Second PR", &viewer);

        let handler = DRepoPrList { be: Arc::new(be) };
        let body = serve(&handler, &viewer, Some("core"), "");
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"First PR") && titles.contains(&"Second PR"));
        assert_eq!(body["counts"]["open"], 2);
        assert_eq!(body["counts"]["all"], 2);
        assert_eq!(body["counts"]["merged"], 0);
        assert_eq!(body["counts"]["yours"], 2, "the viewer authored both");
        let merged = serve(&handler, &viewer, Some("core"), "state=merged");
        assert_eq!(merged["items"].as_array().unwrap().len(), 0);
        assert_eq!(
            merged["counts"]["open"], 2,
            "the Open badge still reads 2 on the Merged tab"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn per_repo_list_cursor_is_stable_and_bidirectional() {
        let root = temp_root("cursor");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        let viewer = human("u:viewer");
        for i in 1..=5 {
            open_pr(&be, "core", &format!("PR {i}"), &viewer);
        }
        let handler = DRepoPrList { be: Arc::new(be) };

        let p1 = serve(&handler, &viewer, Some("core"), "state=all&limit=2");
        assert_eq!(p1["items"].as_array().unwrap().len(), 2);
        assert_eq!(p1["page"]["total"], 5);
        assert!(p1["page"]["prev_cursor"].is_null(), "head has no Newer");
        let c2 = p1["page"]["next_cursor"].as_str().unwrap();
        assert!(c2.starts_with(PR_LIST_CURSOR_PREFIX));

        let p2 = serve(&handler, &viewer, Some("core"), &format!("cursor={c2}"));
        let c1 = p2["page"]["prev_cursor"].as_str().unwrap();
        let c3 = p2["page"]["next_cursor"].as_str().unwrap();
        assert!(c1.starts_with(PR_LIST_CURSOR_PREFIX));
        assert!(c3.starts_with(PR_LIST_CURSOR_PREFIX));
        let back = serve(&handler, &viewer, Some("core"), &format!("cursor={c1}"));
        assert_eq!(back["items"], p1["items"], "Newer returns the prior page");

        let p3 = serve(&handler, &viewer, Some("core"), &format!("cursor={c3}"));
        assert_eq!(p3["items"].as_array().unwrap().len(), 1);
        assert!(p3["page"]["next_cursor"].is_null(), "tail has no Older");

        let mut seen: Vec<u64> = Vec::new();
        for pg in [&p1, &p2, &p3] {
            for it in pg["items"].as_array().unwrap() {
                seen.push(it["number"].as_u64().unwrap());
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3, 4, 5]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn live_keyset_survives_anchor_removal_and_newer_insert_but_does_not_claim_snapshot_history() {
        let root = temp_root("cursor-live-mutation");
        let authz = GrantBackedRepos::new().grant_read("u:viewer", TENANT, "core");
        let be = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
        );
        let viewer = human("u:viewer");
        for i in 1..=5 {
            open_pr(&be, "core", &format!("PR {i}"), &viewer);
        }
        let handler = DRepoPrList { be: be.clone() };
        let first = serve(
            &handler,
            &viewer,
            Some("core"),
            "state=open&sort=created&limit=2",
        );
        let cursor = first["page"]["next_cursor"].as_str().unwrap().to_string();

        be.prs
            .update(
                &DurableGitBackend::loc(TENANT, REGION, "core"),
                4,
                |record| {
                    record.state = PrState::Closed;
                    Ok(())
                },
            )
            .unwrap();
        open_pr(&be, "core", "PR 6", &viewer);
        let second = serve(&handler, &viewer, Some("core"), &format!("cursor={cursor}"));
        assert_eq!(
            second["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["number"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            [3, 2]
        );
        assert_eq!(
            second["page"]["total"], 5,
            "total is the current live total"
        );

        for number in [1_u64, 2, 3, 5, 6] {
            be.prs
                .update(
                    &DurableGitBackend::loc(TENANT, REGION, "core"),
                    number,
                    |record| {
                        record.updated_at = Some((number * 10) as i64);
                        Ok(())
                    },
                )
                .unwrap();
        }
        let updated_first = serve(
            &handler,
            &viewer,
            Some("core"),
            "state=open&sort=updated&limit=2",
        );
        let updated_cursor = updated_first["page"]["next_cursor"]
            .as_str()
            .unwrap()
            .to_string();
        be.prs
            .update(
                &DurableGitBackend::loc(TENANT, REGION, "core"),
                2,
                |record| {
                    record.updated_at = Some(i64::MAX - 1);
                    Ok(())
                },
            )
            .unwrap();
        let repeated = serve(
            &handler,
            &viewer,
            Some("core"),
            &format!("cursor={updated_cursor}"),
        );
        assert!(repeated["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["number"] != 2));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cursor_scope_replay_and_cross_visible_set_changes_are_typed() {
        let root = temp_root("cursor-scopes");
        let viewer = human("u:viewer");
        let authz = GrantBackedRepos::new()
            .grant_read("u:viewer", TENANT, "alpha")
            .grant_read("u:viewer", TENANT, "beta");
        let be = Arc::new(DurableGitBackend::rooted_inmem_for_test(&root));
        open_pr(&be, "alpha", "Alpha", &viewer);
        open_pr(&be, "alpha", "Alpha two", &viewer);
        open_pr(&be, "beta", "Beta", &viewer);
        let be = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
        );

        let repo_handler = DRepoPrList { be: be.clone() };
        let alpha = serve(
            &repo_handler,
            &viewer,
            Some("alpha"),
            "state=all&sort=created&limit=1",
        );
        let repo_cursor = alpha["page"]["next_cursor"].as_str().unwrap();
        assert!(matches!(
            serve_result(
                &repo_handler,
                &viewer,
                Some("beta"),
                &format!("cursor={repo_cursor}")
            ),
            Err(EdgeError::BadRequest(message)) if message.contains("scope mismatch")
        ));

        let cross_handler = DMyPrs { be: be.clone() };
        let first = serve(
            &cross_handler,
            &viewer,
            None,
            "bucket=yours&sort=created&limit=1",
        );
        let cross_cursor = first["page"]["next_cursor"].as_str().unwrap();
        let narrowed = DMyPrs {
            be: Arc::new(
                DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(
                    GrantBackedRepos::new().grant_read("u:viewer", TENANT, "alpha"),
                )),
            ),
        };
        assert!(matches!(
            serve_result(&narrowed, &viewer, None, &format!("cursor={cross_cursor}")),
            Err(EdgeError::Conflict(message)) if message.contains("stale")
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pull_denial_precedes_pr_list_cursor_decoding() {
        let root = temp_root("cursor-auth-order");
        let be = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root)
                .with_repo_authorizer(Arc::new(GrantBackedRepos::new())),
        );
        let guarded_handler = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DRepoPrList { be: be.clone() }),
        );
        let error = serve_result(
            guarded_handler.as_ref(),
            &human("u:denied"),
            Some("hidden"),
            "cursor=pl1_malformed",
        )
        .unwrap_err();
        assert_eq!(error, EdgeError::NotFound("repository not found".into()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn row_vm_title_null_and_checks_unavailable_are_honest() {
        let pr = myelin_git::lifecycle::PullRequest::open(
            9,
            "refs/heads/main",
            "refs/heads/feature",
            "psn:old@acme",
            false,
        );
        let rec = PrRecord::open(&pr, "abc");
        assert_eq!(rec.title, "");
        let enriched = EnrichedPr {
            rec,
            summary: ChecksSummary::unavailable(),
            you_requested: false,
            repo_slug: Some("core".into()),
        };
        let row = DurableGitBackend::pr_list_row_json(&enriched);
        assert!(
            row["title"].is_null(),
            "empty title → null (the #number fallback is honest)"
        );
        assert_eq!(row["number"], 9);
        assert_eq!(
            row["checks_summary"]["verdict"], "unavailable",
            "fails static, still lists"
        );
        assert_eq!(row["updated_at"], Value::Null);
    }

    #[test]
    fn f3_clone_url_is_http_wire_shape_never_ssh() {
        let root = temp_root("f3-clone-url");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let url = be.clone_url(TENANT, REGION, "widgets");
        assert!(
            url.ends_with("/acme/eu-west/widgets.git"),
            "the wire path grammar is /{{tenant}}/{{region}}/{{repo}}.git - got {url}"
        );
        assert!(
            !url.contains("ssh://"),
            "no ssh scheme (there is no SSH server): {url}"
        );
        assert!(!url.contains("git@myelin"), "no fabricated ssh host: {url}");

        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "widgets", &author)
            .unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, "widgets");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let advertised = match be
            .repo_home(TENANT, REGION, "widgets", &repo)
            .expect("repo home reads")
        {
            RepoHome::Empty { clone_url, .. } | RepoHome::Populated { clone_url, .. } => clone_url,
            other => panic!("a fresh repo projects an Empty/Populated home, got {other:?}"),
        };
        assert!(
            advertised.ends_with("/acme/eu-west/widgets.git"),
            "got {advertised}"
        );
        assert!(
            !advertised.contains("ssh://"),
            "no ssh in the projection: {advertised}"
        );

        let namespaced = be.clone_url(TENANT, REGION, "team/widgets");
        assert!(
            namespaced.ends_with("/acme/eu-west/team%2Fwidgets.git"),
            "a namespaced slug stays in the wire route's repo segment: {namespaced}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn pr_number_allocation_never_resets_or_wraps() {
        assert_eq!(DurableGitBackend::next_pr_number_after(None).unwrap(), 1);
        assert_eq!(
            DurableGitBackend::next_pr_number_after(Some(41)).unwrap(),
            42
        );
        let err = DurableGitBackend::next_pr_number_after(Some(u64::MAX))
            .expect_err("an exhausted namespace must fail instead of wrapping");
        assert!(err.to_string().contains("number space exhausted"));
    }

    #[test]
    fn f8_open_pr_resolves_head_oid_from_head_ref_tip() {
        let root = temp_root("f8-resolve-head");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "core", &author).unwrap();

        let loc = DurableGitBackend::loc(TENANT, REGION, "core");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let blob = repo.write_blob(b"hello\n").expect("blob");
        let tree = repo.write_tree(&[("f.txt", &blob)]).expect("tree");
        let tip = repo
            .write_commit(&tree, &[], "seed", "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit");
        repo.update_ref_cas(
            "refs/heads/feature",
            None,
            Some(&tip),
            "create",
            "psn@acme.noreply",
        )
        .expect("create feature ref");

        let body = json!({
            "title": "resolve my head",
            "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature",
        });
        let rec = be
            .open_pr(TENANT, REGION, "core", &body, &author)
            .expect("open PR");
        assert_eq!(
            rec.head_oid, tip.0,
            "F8: an omitted head_oid is resolved from head_ref's current tip"
        );

        let body_bare = json!({ "title": "bare head_ref", "head_ref": "feature" });
        let rec2 = be
            .open_pr(TENANT, REGION, "core", &body_bare, &author)
            .expect("open PR");
        assert_eq!(
            rec2.head_oid, tip.0,
            "F8: a bare branch name also resolves to the tip"
        );

        let bad = json!({ "title": "ghost branch", "head_ref": "refs/heads/does-not-exist" });
        let err = be
            .open_pr(TENANT, REGION, "core", &bad, &author)
            .expect_err("must refuse");
        assert_eq!(
            map_durable_err(err).status(),
            400,
            "F8: a non-existent head_ref is a 400 at open, not a merge-time surprise"
        );
        let oversized = be.raw_response_bounded(
            TENANT,
            REGION,
            "core",
            "refs/heads/feature",
            "f.txt",
            RawResponseOptions {
                attachment: true,
                maximum_bytes: 1,
            },
        );
        assert!(matches!(oversized, Err(error) if error.status() == 413));
        assert_eq!(
            read_text_blob_at_snapshot_bounded(&repo, &tip, "f.txt", 1).unwrap(),
            None,
            "an oversized README-style preview must stop at the object header",
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn blob_view_stops_at_the_object_header_for_oversized_previews() {
        let root = temp_root("blob-preview-bound");
        let be = DurableGitBackend::rooted_inmem_for_test(&root);
        let author = human("u:author");
        be.create_repo_as(TENANT, REGION, "core", &author).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, "core");
        let repo = be.store.open_repo(&loc).expect("open repo");
        let blob = repo.write_blob(b"hello\n").expect("blob");
        let tree = repo.write_tree(&[("large.txt", &blob)]).expect("tree");
        let tip = repo
            .write_commit(&tree, &[], "seed", "psn@acme.noreply", "psn@acme.noreply")
            .expect("commit");
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&tip),
            "create",
            "psn@acme.noreply",
        )
        .expect("create main");

        let metadata = be
            .blob_json_bounded(
                TENANT,
                REGION,
                "core",
                "main",
                "large.txt",
                BlobViewOptions {
                    maximum_preview_bytes: 1,
                    maximum_transfer_bytes: 4,
                },
            )
            .expect("metadata-only blob view");
        assert_eq!(metadata["contents"], "");
        assert_eq!(metadata["base_oid"], blob.as_str());
        assert_eq!(metadata["size_bytes"], 6);
        assert_eq!(metadata["preview_unavailable"], true);
        assert_eq!(metadata["download_available"], false);
        assert_eq!(metadata["viewer_may_edit"], false);

        let inline = be
            .blob_json_bounded(
                TENANT,
                REGION,
                "core",
                "main",
                "large.txt",
                BlobViewOptions {
                    maximum_preview_bytes: 6,
                    maximum_transfer_bytes: 6,
                },
            )
            .expect("inline blob view");
        assert_eq!(inline["contents"], "hello\n");
        assert_eq!(inline["preview_unavailable"], false);
        assert_eq!(inline["download_available"], true);
        assert_eq!(inline["viewer_may_edit"], false);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn oversized_interactive_reads_map_to_bounded_public_responses() {
        for (private, public) in [
            (
                "browse response limit exceeded: private repository detail",
                "repository view exceeds the interactive browse limit",
            ),
            (
                "pr diff computation limit exceeded: private repository detail",
                "pull request diff exceeds the interactive file limit",
            ),
            (
                "commit diff computation limit exceeded: private repository detail",
                "commit diff exceeds the interactive content limit",
            ),
            (
                "pull request list limit exceeded: private repository detail",
                "pull request list exceeds the interactive record limit",
            ),
            (
                "pull request record limit exceeded: private repository detail",
                "pull request record exceeds the storage limit",
            ),
            (
                "branch protection limit exceeded: private repository detail",
                "branch protection policy exceeds the storage limit",
            ),
            (
                "wire ref limit exceeded: private repository detail",
                "repository exceeds the smart-HTTP ref limit",
            ),
        ] {
            let mapped = map_durable_err(DurableError::Git(private.into()));
            assert_eq!(mapped.status(), 413);
            assert_eq!(
                mapped.to_string(),
                format!("413 (payload_too_large): {public}")
            );
        }
    }
}

#[cfg(test)]
mod file_lines_boundary_tests {
    use super::*;

    #[test]
    fn file_lines_query_is_exact_decoded_and_bounded() {
        let parsed = parse_file_lines_query("path=src%2Fmain+file.rs&start=2&end=4")
            .expect("canonical bounded query");
        assert_eq!(parsed.path, "src/main file.rs");
        assert_eq!((parsed.start, parsed.end), (2, 4));

        let exact_end = 17 + FILE_LINES_MAX_RANGE - 1;
        let exact = parse_file_lines_query(&format!("path=x&start=17&end={exact_end}"))
            .expect("the exact line-range cap must remain valid");
        assert_eq!((exact.start, exact.end), (17, exact_end));

        for query in [
            "",
            "path=x&start=1",
            "path=x&start=1&end=1&extra=x",
            "path=x&path=y&start=1&end=1",
            "path=..%2Fsecret&start=1&end=1",
            "path=x&start=0&end=1",
            "path=x&start=2&end=1",
            &format!("path=x&start=1&end={}", FILE_LINES_MAX_RANGE + 1),
            "path=x%ZZ&start=1&end=1",
        ] {
            assert!(
                matches!(parse_file_lines_query(query), Err(EdgeError::BadRequest(_))),
                "query must fail closed: {query}"
            );
        }
        assert!(matches!(
            parse_file_lines_query(&"x".repeat(FILE_LINES_MAX_QUERY_BYTES + 1)),
            Err(EdgeError::BadRequest(_))
        ));
    }

    #[test]
    fn file_lines_oid_requires_the_full_lowercase_content_address() {
        assert!(canonical_blob_oid(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!canonical_blob_oid("01234567"));
        assert!(!canonical_blob_oid(
            "0123456789ABCDEF0123456789ABCDEF01234567"
        ));
        assert!(!canonical_blob_oid(
            "g123456789abcdef0123456789abcdef01234567"
        ));
    }
}

#[cfg(test)]
mod pr_thread_tests {

    use super::*;
    use crate::catalogue::Page;
    use crate::repo_authz::GrantBackedRepos;
    use crate::request::EdgeRequest;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region as IdRegion, TenantId};
    use std::collections::BTreeMap;

    const TENANT: &str = "acme";
    const REGION: &str = "eu-west";
    const SLUG: &str = "core";

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "myelin-prthread-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn human(id: &str) -> Principal {
        Principal::new(
            TenantId(TENANT.into()),
            IdRegion(REGION.into()),
            PrincipalId(id.into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn serve(
        handler: &dyn Handler,
        method: &str,
        viewer: &Principal,
        params: &[(&str, &str)],
        body: Value,
    ) -> Result<Value, EdgeError> {
        let scope = TenantScope::from_verified_token(viewer, viewer.region.clone());
        let pmap: BTreeMap<String, String> = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let bytes = if body.is_null() {
            vec![]
        } else {
            serde_json::to_vec(&body).unwrap()
        };
        let headers = if method == "GET" {
            vec![]
        } else {
            let mut retry_key = blake3::Hasher::new();
            retry_key.update(method.as_bytes());
            for (name, value) in params {
                retry_key.update(name.as_bytes());
                retry_key.update(value.as_bytes());
            }
            retry_key.update(&bytes);
            vec![(
                "idempotency-key".into(),
                format!("pr-thread-test-{}", retry_key.finalize().to_hex()),
            )]
        };
        let req = EdgeRequest::new(method, "/v1/git/x", "", headers, bytes);
        let page = Page::from_request(&req);
        let identity = crate::catalogue::test_request_identity(viewer, &scope);
        let ctx = HandlerCtx {
            identity: &identity,
            principal: viewer,
            scope: &scope,
            params: &pmap,
            page: &page,
            request: &req,
        };
        handler.handle(&ctx).map(|r| r.json_body().expect("json"))
    }

    fn setup(tag: &str, head_oid: &str) -> (Arc<DurableGitBackend>, Principal, Principal) {
        let root = temp_root(tag);
        let authz = GrantBackedRepos::new()
            .grant_write("u:writer", TENANT, SLUG)
            .grant_read("u:reader", TENANT, SLUG);
        let writer = human("u:writer");
        let reader = human("u:reader");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        let body = json!({
            "title": "R3.3 flagship", "base_ref": "refs/heads/main",
            "head_ref": "refs/heads/feature", "head_oid": head_oid, "draft": false,
        });
        be.open_pr(TENANT, REGION, SLUG, &body, &writer).unwrap();
        (Arc::new(be), writer, reader)
    }

    #[test]
    fn thread_write_admits_requested_reviewer_or_repo_pusher_only() {
        let (be, writer, reader) = setup("authz", &"0".repeat(40));
        let list = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DPrThreads { be: be.clone() }),
        );
        let v = serve(
            &*list,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect("reader may read threads");
        assert!(v["threads"].is_array());
        let create = pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() }));
        let err = serve(
            &*create,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "hi" }),
        )
        .expect_err("an unrelated read-only viewer must be forbidden from commenting");
        assert!(matches!(err, EdgeError::Forbidden(_)), "got {err:?}");

        serve(
            &*create,
            "POST",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "writer comment" }),
        )
        .expect("a repo pusher may review");

        let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
        be.prs
            .update(&loc, 1, |record| {
                record.reviews.push(ReviewRecord {
                    reviewer_pseudonym: DurableGitBackend::pseudonym(TENANT, &reader),
                    state: ReviewState::Requested,
                    is_agent: false,
                });
                Ok(())
            })
            .expect("request review");
        serve(
            &*create,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            json!({ "body_md": "requested-reviewer comment" }),
        )
        .expect("a directly requested reviewer may review without repo Push");
    }

    #[test]
    fn opening_a_pull_request_records_unique_requested_reviewers() {
        let (be, writer, reader) = setup("requested-reviewers", &"0".repeat(40));
        let opened = be
            .open_pr(
                TENANT,
                REGION,
                SLUG,
                &json!({
                    "title": "Request review while opening",
                    "base_ref": "refs/heads/main",
                    "head_ref": "refs/heads/second-feature",
                    "head_oid": "1".repeat(40),
                    "reviewers": ["u:reader", "u:reader", "u:writer"],
                }),
                &writer,
            )
            .expect("open pull request");

        assert_eq!(
            opened.reviews.len(),
            1,
            "duplicates and the author are omitted"
        );
        assert_eq!(
            opened.reviews[0].reviewer_pseudonym,
            "u:reader@acme.noreply"
        );
        assert_eq!(opened.reviews[0].state, ReviewState::Requested);
        assert!(
            be.authorize_pr_review(TENANT, REGION, SLUG, opened.number, &reader),
            "a requested reader may participate in the review"
        );

        let malformed = be
            .open_pr(
                TENANT,
                REGION,
                SLUG,
                &json!({
                    "title": "Reject malformed reviewer",
                    "base_ref": "refs/heads/main",
                    "head_ref": "refs/heads/third-feature",
                    "head_oid": "2".repeat(40),
                    "reviewers": [" u:reader"],
                }),
                &writer,
            )
            .expect_err("reviewer ids must be canonical");
        assert!(
            matches!(malformed, DurableError::Git(message) if message.contains("reviewer ids"))
        );
    }

    #[test]
    fn oversized_comment_is_rejected_before_conversation_storage() {
        let (be, writer, _reader) = setup("body-limit", &"0".repeat(40));
        let create = pr_review_guarded(&be, Arc::new(DPrThreadCreate { be: be.clone() }));

        let error = serve(
            &*create,
            "POST",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            json!({
                "body_md": "x".repeat(myelin_git::pr_threads::MAX_COMMENT_BODY_BYTES + 1),
            }),
        )
        .expect_err("oversized comment must fail before persistence");

        assert!(matches!(error, EdgeError::BadRequest(_)), "got {error:?}");
        assert!(be
            .threads
            .load(&DurableGitBackend::loc(TENANT, REGION, SLUG), "pr:core:1")
            .unwrap()
            .threads
            .is_empty());
    }

    #[test]
    fn pending_comment_is_private_and_submit_emits_one_event() {
        let (be, writer, reader) = setup("pending", &"0".repeat(40));
        let threads = Arc::new(DPrThreads { be: be.clone() });
        let start = Arc::new(DPrReviewStart { be: be.clone() });
        let pending = Arc::new(DPrReviewComment { be: be.clone() });
        let submit = Arc::new(DPrReviewSubmit { be: be.clone() });

        let batch = serve(
            &*start,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        let rid = batch["applied"]["review"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        serve(
            &*pending,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "body_md": "draft note" }),
        )
        .unwrap();

        let seen = serve(
            &*threads,
            "GET",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        assert_eq!(
            seen["threads"].as_array().unwrap().len(),
            0,
            "pending comment is private"
        );
        assert_eq!(
            seen["reviews"].as_array().unwrap().len(),
            0,
            "draft batch is hidden"
        );

        let first = serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "commented" }),
        )
        .unwrap();
        assert_eq!(first["applied"]["result"]["emitted"], true);
        let again = serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "commented" }),
        )
        .unwrap();
        assert_eq!(
            again["applied"]["result"]["emitted"], false,
            "no double event"
        );

        let seen = serve(
            &*threads,
            "GET",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        assert_eq!(
            seen["threads"].as_array().unwrap().len(),
            1,
            "submit makes it public"
        );
    }

    #[test]
    fn a_changes_requested_batch_blocks_the_gate() {
        let (be, _writer, reader) = setup("blockgate", &"0".repeat(40));
        let start = Arc::new(DPrReviewStart { be: be.clone() });
        let submit = Arc::new(DPrReviewSubmit { be: be.clone() });
        let checks = Arc::new(DPrChecks { be: be.clone() });

        let batch = serve(
            &*start,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        let rid = batch["applied"]["review"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        serve(
            &*submit,
            "POST",
            &reader,
            &[("repo", SLUG), ("n", "1"), ("rid", &rid)],
            json!({ "verdict": "changes_requested" }),
        )
        .unwrap();
        let ck = serve(
            &*checks,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .unwrap();
        assert_eq!(
            ck["changes_requested"], true,
            "the gate ingests changes_requested"
        );
        assert_eq!(
            ck["gate_admitted"], false,
            "a live request-changes blocks the merge"
        );
    }

    #[test]
    fn a_blocked_merge_returns_409_with_rerendered_checks() {
        let root = temp_root("merge409");
        let authz = GrantBackedRepos::new().grant_admin("u:writer", TENANT, SLUG);
        let writer = human("u:writer");
        let be = Arc::new(
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz)),
        );
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        be.open_pr(
            TENANT,
            REGION,
            SLUG,
            &json!({ "title": "N6", "base_ref": "refs/heads/main", "head_ref": "refs/heads/feature",
                     "head_oid": "0".repeat(40), "draft": false }),
            &writer,
        )
        .unwrap();
        let merge = guarded(
            &be,
            RepoPermission::ProtectedPush,
            Arc::new(DMerge { be: be.clone() }),
        );
        let resp = serve(
            &*merge,
            "POST",
            &writer,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect("merge handler returns a body (409 is an Ok EdgeResponse, not an Err)");
        assert_eq!(resp["error"]["code"], "merge_blocked");
        assert_eq!(
            resp["checks"]["gate_admitted"], false,
            "the 409 carries the fresh gate state"
        );
    }

    fn setup_diff(tag: &str) -> (Arc<DurableGitBackend>, Principal, String) {
        let root = temp_root(tag);
        let authz = GrantBackedRepos::new()
            .grant_write("u:writer", TENANT, SLUG)
            .grant_read("u:reader", TENANT, SLUG);
        let writer = human("u:writer");
        let reader = human("u:reader");
        let be =
            DurableGitBackend::rooted_inmem_for_test(&root).with_repo_authorizer(Arc::new(authz));
        be.create_repo_as(TENANT, REGION, SLUG, &writer).unwrap();
        let loc = DurableGitBackend::loc(TENANT, REGION, SLUG);
        let repo = be.store.open_repo(&loc).unwrap();
        let b0 = repo.write_blob(b"a\nb\nc\n").unwrap();
        let t0 = repo.write_tree(&[("file.txt", &b0)]).unwrap();
        let base = repo
            .write_commit(&t0, &[], "base", "psn@acme.noreply", "psn@acme.noreply")
            .unwrap();
        repo.update_ref_cas(
            "refs/heads/main",
            None,
            Some(&base),
            "c",
            "psn@acme.noreply",
        )
        .unwrap();
        let bh = repo.write_blob(b"a\nB\nc\nd\n").unwrap();
        let th = repo.write_tree(&[("file.txt", &bh)]).unwrap();
        let head = repo
            .write_commit(
                &th,
                &[&base],
                "head",
                "psn@acme.noreply",
                "psn@acme.noreply",
            )
            .unwrap();
        be.open_pr(
            TENANT,
            REGION,
            SLUG,
            &json!({ "title": "diff pr", "base_ref": "refs/heads/main",
                     "head_ref": "refs/heads/feature", "head_oid": head.0, "draft": false }),
            &writer,
        )
        .unwrap();
        (Arc::new(be), reader, head.0)
    }

    #[test]
    fn line_anchors_are_strictly_validated_and_revision_bound() {
        let (be, reviewer, head) = setup_diff("anchors");
        let new_side = be
            .create_thread(
                RepoActorContext::new(TENANT, REGION, SLUG, &reviewer).for_pr(1),
                "new-side-anchor",
                &json!({
                    "body_md": "new-side note",
                    "anchor": { "path": "file.txt", "line": 4, "side": "new" },
                }),
            )
            .expect("a displayed new-side line resolves");
        assert_eq!(new_side["anchor"]["side"], "new");
        assert_eq!(new_side["anchor"]["head_oid"], head);
        assert_eq!(new_side["anchor"]["base_oid"].as_str().unwrap().len(), 40);

        let old_side = be
            .create_thread(
                RepoActorContext::new(TENANT, REGION, SLUG, &reviewer).for_pr(1),
                "old-side-anchor",
                &json!({
                    "body_md": "old-side note",
                    "anchor": { "path": "file.txt", "line": 2, "side": "old" },
                }),
            )
            .expect("a displayed old-side line resolves");
        assert_eq!(old_side["anchor"]["side"], "old");

        for (index, invalid) in [
            json!({ "body_md": "missing side", "anchor": { "path": "file.txt", "line": 2 } }),
            json!({ "body_md": "stale line", "anchor": { "path": "file.txt", "line": 99, "side": "new" } }),
            json!({ "body_md": "unsafe path", "anchor": { "path": "../secret", "line": 1, "side": "new" } }),
        ]
        .into_iter()
        .enumerate()
        {
            let error = be
                .create_thread(
                    RepoActorContext::new(TENANT, REGION, SLUG, &reviewer).for_pr(1),
                    &format!("invalid-anchor-{index}"),
                    &invalid,
                )
                .expect_err("malformed or stale anchor must be rejected");
            assert!(error.to_string().contains("anchor"), "got {error:?}");
        }

        let stored = be
            .threads
            .load(&DurableGitBackend::loc(TENANT, REGION, SLUG), "pr:core:1")
            .unwrap();
        assert_eq!(stored.threads.len(), 2, "invalid anchors persisted nothing");
    }

    #[test]
    fn pr_diff_is_pull_guarded_zero_leak_and_three_dot() {
        let (be, reader, head) = setup_diff("diffauthz");
        let guard = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DPrDiff { be: be.clone() }),
        );
        let v = serve(
            &*guard,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect("a reader may view the PR diff");
        assert_eq!(v["number"], 1);
        assert_eq!(
            v["three_dot"], true,
            "durable repos are libgit2-backed → merge-base"
        );
        assert_eq!(v["total_files"], 1);
        assert_eq!(v["files"][0]["path"], "file.txt");
        assert_eq!(v["files"][0]["status"], "M");
        assert_eq!(v["files"][0]["kind"], "text");
        let new_blob_oid = v["files"][0]["new_blob_oid"]
            .as_str()
            .expect("visible text files carry their immutable new-side blob oid");
        assert_eq!(new_blob_oid.len(), 40);
        assert_ne!(
            new_blob_oid, head,
            "the blob oid must never be the PR head commit oid"
        );
        let lines = v["files"][0]["hunks"][0]["lines"].as_array().unwrap();
        assert!(lines
            .iter()
            .any(|l| l["origin"] == "+" && l["content"] == "d" && l["new_no"] == 4));
        assert_eq!(
            v["restricted_files"], 0,
            "count-only; 0 under the repo-level Pull guard"
        );
        assert!(v["restricted_files"].is_number());

        let stranger = human("u:stranger");
        let err = serve(
            &*guard,
            "GET",
            &stranger,
            &[("repo", SLUG), ("n", "1")],
            Value::Null,
        )
        .expect_err("a stranger must not view the diff");
        assert!(
            matches!(err, EdgeError::NotFound(_)),
            "0-leak 404, got {err:?}"
        );
    }

    #[test]
    fn pr_diff_absent_pr_is_not_found() {
        let (be, reader, _head) = setup_diff("diffabsent");
        let guard = guarded(
            &be,
            RepoPermission::Pull,
            Arc::new(DPrDiff { be: be.clone() }),
        );
        let err = serve(
            &*guard,
            "GET",
            &reader,
            &[("repo", SLUG), ("n", "999")],
            Value::Null,
        )
        .expect_err("absent PR");
        assert!(matches!(err, EdgeError::NotFound(_)), "got {err:?}");
    }
}
