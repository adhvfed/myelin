use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use myelin_chat::store::pg_conversation::visible_public_conversations_cte;
use myelin_identity::Principal;
use myelin_refs::{parse_scoped, strip_sub, ArtifactRef, ParsedArtifactRef};
use myelin_refs_service::{PgEdgeStore, StoredEdge};
use myelin_storage::with_tenant_tx;
use percent_encoding::percent_decode_str;
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool};
use tokio::runtime::Handle;

use crate::catalogue::{Handler, HandlerCtx, MAX_PAGE_LIMIT};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::repo_authz::RepoAuthorizer;
use crate::request::EdgeResponse;
use crate::runtime::drive_result_on_runtime;
use crate::Method;

const DEFAULT_BACKLINK_LIMIT: usize = 50;
const MAX_AUTHORIZATION_SCAN: u32 = 1_000;

#[derive(Clone)]
pub struct DurableRefsReadApi {
    graph: PgEdgeStore,
    visibility: PgArtifactVisibility,
    runtime: Handle,
}

impl DurableRefsReadApi {
    pub fn new(pool: PgPool, repos: Arc<dyn RepoAuthorizer>, runtime: Handle) -> Self {
        Self {
            graph: PgEdgeStore::new(pool.clone()),
            visibility: PgArtifactVisibility::new(pool, repos),
            runtime,
        }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, EdgeError>
    where
        F: std::future::Future<Output = Result<T, EdgeError>>,
    {
        drive_result_on_runtime(
            &self.runtime,
            future,
            EdgeError::Unavailable("References require the Edge multi-thread runtime".into()),
        )
    }

    pub fn backlinks(
        &self,
        principal: &Principal,
        reference: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Value, EdgeError> {
        self.related(principal, reference, limit, cursor, EdgeDirection::Inbound)
    }

    pub fn links(
        &self,
        principal: &Principal,
        reference: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Value, EdgeError> {
        self.related(principal, reference, limit, cursor, EdgeDirection::Outbound)
    }

    fn related(
        &self,
        principal: &Principal,
        reference: &str,
        limit: usize,
        cursor: Option<&str>,
        direction: EdgeDirection,
    ) -> Result<Value, EdgeError> {
        let parsed = parse_scoped(reference)
            .map_err(|error| EdgeError::BadRequest(format!("invalid reference: {error}")))?;
        if parsed.tenant != principal.tenant {
            return Err(EdgeError::NotFound("reference not found".into()));
        }
        let target_root = strip_sub(&parsed.artifact_ref);
        self.drive(async {
            if !self
                .visibility
                .readable_roots(principal, std::slice::from_ref(&target_root))
                .await?
                .contains(&target_root.0)
            {
                return Err(EdgeError::NotFound("reference not found".into()));
            }

            let candidates = match direction {
                EdgeDirection::Inbound => {
                    self.graph
                        .inbound_live_after(
                            &principal.tenant,
                            &principal.region,
                            &target_root,
                            cursor,
                            MAX_AUTHORIZATION_SCAN + 1,
                        )
                        .await
                }
                EdgeDirection::Outbound => {
                    self.graph
                        .outbound_live_after(
                            &principal.tenant,
                            &principal.region,
                            &target_root,
                            cursor,
                            MAX_AUTHORIZATION_SCAN + 1,
                        )
                        .await
                }
            }
            .map_err(|error| EdgeError::Internal(error.to_string()))?;
            let scan_has_more = candidates.len() > MAX_AUTHORIZATION_SCAN as usize;
            let mut candidates = candidates;
            candidates.truncate(MAX_AUTHORIZATION_SCAN as usize);

            let roots = candidates
                .iter()
                .map(|edge| ArtifactRef(direction.related_root(edge).to_string()))
                .collect::<Vec<_>>();
            let visible = self.visibility.readable_roots(principal, &roots).await?;
            let page =
                paginate_authorized_scan(candidates, &visible, limit, scan_has_more, direction);
            let items = page
                .items
                .into_iter()
                .map(|edge| edge_json(edge, direction))
                .collect::<Vec<_>>();
            Ok(json!({
                "ref": parsed.artifact_ref.0,
                "root_ref": target_root.0,
                "items": items,
                "page": { "next_cursor": page.next_cursor, "limit": limit },
            }))
        })
    }
}

#[derive(Clone, Copy)]
enum EdgeDirection {
    Inbound,
    Outbound,
}

impl EdgeDirection {
    fn related_ref(self, edge: &StoredEdge) -> &str {
        match self {
            Self::Inbound => &edge.source,
            Self::Outbound => &edge.target,
        }
    }

    fn related_root(self, edge: &StoredEdge) -> &str {
        match self {
            Self::Inbound => &edge.source_root,
            Self::Outbound => &edge.target_root,
        }
    }
}

struct AuthorizedEdgePage {
    items: Vec<StoredEdge>,
    next_cursor: Option<String>,
}

fn paginate_authorized_scan(
    candidates: Vec<StoredEdge>,
    visible_roots: &BTreeSet<String>,
    limit: usize,
    scan_has_more: bool,
    direction: EdgeDirection,
) -> AuthorizedEdgePage {
    let scanned_through = candidates.last().map(|edge| edge.edge_id.clone());
    let mut items = candidates
        .into_iter()
        .filter(|edge| visible_roots.contains(direction.related_root(edge)))
        .collect::<Vec<_>>();
    let authorized_has_more = items.len() > limit;
    items.truncate(limit);
    let next_cursor = if authorized_has_more {
        items.last().map(|edge| edge.edge_id.clone())
    } else if scan_has_more {
        scanned_through
    } else {
        None
    };
    AuthorizedEdgePage { items, next_cursor }
}

fn edge_json(edge: StoredEdge, direction: EdgeDirection) -> Value {
    let related_ref = direction.related_ref(&edge).to_string();
    let related_root = direction.related_root(&edge).to_string();
    json!({
        "ref": related_ref,
        "root_ref": related_root,
        "source_ref": edge.source,
        "source_root_ref": edge.source_root,
        "target_ref": edge.target,
        "target_root_ref": edge.target_root,
        "relation": edge.rel,
        "relation_class": edge.rel_class,
        "origin_actor": edge.origin_actor,
    })
}

#[derive(Clone)]
struct PgArtifactVisibility {
    pool: PgPool,
    repos: Arc<dyn RepoAuthorizer>,
}

impl PgArtifactVisibility {
    fn new(pool: PgPool, repos: Arc<dyn RepoAuthorizer>) -> Self {
        Self { pool, repos }
    }

    async fn readable_roots(
        &self,
        principal: &Principal,
        roots: &[ArtifactRef],
    ) -> Result<BTreeSet<String>, EdgeError> {
        let mut by_class: BTreeMap<(String, String), Vec<ParsedArtifactRef>> = BTreeMap::new();
        for root in roots {
            let Ok(parsed) = parse_scoped(&root.0) else {
                continue;
            };
            if parsed.tenant != principal.tenant || parsed.sub.is_some() {
                continue;
            }
            by_class
                .entry((parsed.subsystem.clone(), parsed.type_.clone()))
                .or_default()
                .push(parsed);
        }

        let tenant = principal.tenant.0.clone();
        let region = principal.region.0.clone();
        let subject = principal.principal_id.0.clone();
        let viewer = myelin_knowledge::event_actor_pseudonym(&tenant, &subject);
        let issue_keys = ids(&by_class, "issue", "issue");
        let message_ids = ids(&by_class, "chat", "message");
        let channel_ids = ids(&by_class, "chat", "channel");
        let page_ids = ids(&by_class, "knowledge", "page");
        let run_ids = ids(&by_class, "ci", "run");
        let query = DatabaseVisibilityQuery {
            tenant: tenant.clone(),
            region: region.clone(),
            subject,
            viewer,
            issue_keys,
            message_ids,
            channel_ids,
            page_ids,
            run_ids,
        };
        let database_visible = with_tenant_tx(&self.pool, &tenant, &region, move |connection| {
            Box::pin(read_database_visibility(connection, query))
        })
        .await
        .map_err(|error| EdgeError::Internal(error.to_string()))?;

        let mut visible = database_visible.roots;
        let mut repo_roots: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for parsed in by_class.values().flatten() {
            if let Some(repo) = git_repo_slug(parsed) {
                repo_roots
                    .entry(repo.to_string())
                    .or_default()
                    .push(parsed.artifact_ref.0.clone());
            }
        }
        for (root, repo) in database_visible.ci_repos {
            repo_roots.entry(repo).or_default().push(root);
        }
        let repos = repo_roots.keys().cloned().collect::<Vec<_>>();
        let allowed = self
            .repos
            .visible_repos(principal, &tenant, &region, &repos)
            .into_iter()
            .collect::<BTreeSet<_>>();
        for (repo, roots) in repo_roots {
            if allowed.contains(&repo) {
                visible.extend(roots);
            }
        }
        Ok(visible)
    }
}

fn ids(
    classes: &BTreeMap<(String, String), Vec<ParsedArtifactRef>>,
    subsystem: &str,
    type_: &str,
) -> Vec<String> {
    classes
        .get(&(subsystem.to_string(), type_.to_string()))
        .into_iter()
        .flatten()
        .map(|parsed| parsed.id.clone())
        .collect()
}

fn git_repo_slug(parsed: &ParsedArtifactRef) -> Option<&str> {
    if parsed.subsystem != "git" {
        return None;
    }
    match parsed.type_.as_str() {
        "repo" => Some(parsed.id.as_str()),
        "pr" | "review" | "comment" | "commit" | "blob" | "ref" => parsed.id.split(':').next(),
        _ => None,
    }
}

#[derive(Default)]
struct DatabaseVisibility {
    roots: BTreeSet<String>,
    ci_repos: Vec<(String, String)>,
}

struct DatabaseVisibilityQuery {
    tenant: String,
    region: String,
    subject: String,
    viewer: String,
    issue_keys: Vec<String>,
    message_ids: Vec<String>,
    channel_ids: Vec<String>,
    page_ids: Vec<String>,
    run_ids: Vec<String>,
}

async fn read_database_visibility(
    connection: &mut PgConnection,
    query: DatabaseVisibilityQuery,
) -> Result<DatabaseVisibility, myelin_storage::PgError> {
    let DatabaseVisibilityQuery {
        tenant,
        region,
        subject,
        viewer,
        issue_keys,
        message_ids,
        channel_ids,
        page_ids,
        run_ids,
    } = query;
    let mut result = DatabaseVisibility::default();
    let visible_issues = sqlx::query_scalar::<_, String>(
        "SELECT i.key
           FROM authz_projection_state projection
           JOIN issue_authz_visible visible
             ON visible.tenant_id = projection.tenant_id
            AND visible.region = projection.region
            AND visible.projection = projection.projection
            AND visible.revision = projection.applied_revision
           JOIN issue i
             ON i.tenant_id = visible.tenant_id AND i.region = visible.region
            AND i.id::text = visible.object_id
          WHERE projection.tenant_id = $1 AND projection.region = $2
            AND projection.projection = 'issue:view' AND projection.status = 'ready'
            AND projection.applied_revision = projection.source_revision
            AND visible.subject = $3 AND visible.permission = 'view'
            AND visible.object_type = 'issue' AND i.key = ANY($4)
            AND i.deleted_at IS NULL AND NOT i.archived",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&subject)
    .bind(&issue_keys)
    .fetch_all(&mut *connection)
    .await
    .map_err(database_error("authorize referenced issues"))?;
    result.roots.extend(
        visible_issues
            .into_iter()
            .map(|id| canonical_root(&tenant, "issue", "issue", &id)),
    );

    result.roots.extend(
        read_visible_chat_roots(
            connection,
            &tenant,
            &region,
            &subject,
            &message_ids,
            &channel_ids,
        )
        .await?,
    );

    let visible_pages = sqlx::query_scalar::<_, String>(
        "SELECT page_id FROM knowledge_page
          WHERE tenant_id = $1 AND region = $2 AND page_id = ANY($3) AND NOT archived
            AND (owner = $4 OR visibility = 'team')",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&page_ids)
    .bind(&viewer)
    .fetch_all(&mut *connection)
    .await
    .map_err(database_error("authorize referenced knowledge pages"))?;
    result.roots.extend(
        visible_pages
            .into_iter()
            .map(|id| canonical_root(&tenant, "knowledge", "page", &id)),
    );

    let visible_runs = sqlx::query_as::<_, (String, String)>(
        "SELECT run_id::text, repo_ref FROM ci_run
          WHERE tenant_id = $1 AND region = $2 AND run_id::text = ANY($3)
            AND repo_ref IS NOT NULL",
    )
    .bind(&tenant)
    .bind(&region)
    .bind(&run_ids)
    .fetch_all(connection)
    .await
    .map_err(database_error("authorize referenced CI runs"))?;
    result
        .ci_repos
        .extend(visible_runs.into_iter().filter_map(|(run_id, repo_ref)| {
            let repo = parse_scoped(&repo_ref).ok()?;
            (repo.subsystem == "git" && repo.type_ == "repo")
                .then(|| (canonical_root(&tenant, "ci", "run", &run_id), repo.id))
        }));
    Ok(result)
}

async fn read_visible_chat_roots(
    connection: &mut PgConnection,
    tenant: &str,
    region: &str,
    subject: &str,
    message_ids: &[String],
    channel_ids: &[String],
) -> Result<Vec<String>, myelin_storage::PgError> {
    let query = format!(
        "{}
         SELECT 'message' AS artifact_type, message.message_id AS artifact_id
           FROM chat_message message
           JOIN visible_conversation conversation
             ON conversation.conversation_id = message.conversation_id
          WHERE message.tenant_id = $1 AND message.region = $2
            AND message.message_id = ANY($4)
         UNION ALL
         SELECT 'channel' AS artifact_type, conversation_id AS artifact_id
           FROM visible_conversation
          WHERE conversation_id = ANY($5)",
        visible_public_conversations_cte(),
    );
    let rows = sqlx::query_as::<_, (String, String)>(&query)
        .bind(tenant)
        .bind(region)
        .bind(subject)
        .bind(message_ids)
        .bind(channel_ids)
        .fetch_all(connection)
        .await
        .map_err(database_error("authorize referenced Chat artifacts"))?;
    Ok(rows
        .into_iter()
        .map(|(artifact_type, artifact_id)| {
            canonical_root(tenant, "chat", &artifact_type, &artifact_id)
        })
        .collect())
}

fn database_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> myelin_storage::PgError {
    move |error| myelin_storage::PgError::Query(format!("{context}: {error}"))
}

fn canonical_root(tenant: &str, subsystem: &str, type_: &str, id: &str) -> String {
    format!("myelin://{tenant}/{subsystem}/{type_}/{id}")
}

fn is_canonical_edge_cursor(value: &str) -> bool {
    fn is_lower_hex(value: &str, expected_len: usize) -> bool {
        value.len() == expected_len
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    is_lower_hex(value, 32)
        || value
            .strip_prefix("blake3:")
            .is_some_and(|digest| is_lower_hex(digest, 64))
}

fn parse_query(query: &str) -> Result<(String, usize, Option<String>), EdgeError> {
    let mut reference = None;
    let mut limit = None;
    let mut cursor = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, encoded) = pair
            .split_once('=')
            .ok_or_else(|| EdgeError::BadRequest("malformed References query".into()))?;
        let value = percent_decode_str(encoded)
            .decode_utf8()
            .map_err(|_| EdgeError::BadRequest("References query is not UTF-8".into()))?;
        match name {
            "ref" if reference.is_none() => reference = Some(value.into_owned()),
            "limit" if limit.is_none() => {
                let parsed = value.parse::<usize>().map_err(|_| {
                    EdgeError::BadRequest("References limit must be an integer".into())
                })?;
                if !(1..=MAX_PAGE_LIMIT).contains(&parsed) {
                    return Err(EdgeError::BadRequest(format!(
                        "References limit must be within 1..={MAX_PAGE_LIMIT}"
                    )));
                }
                limit = Some(parsed);
            }
            "cursor" if cursor.is_none() => {
                if !is_canonical_edge_cursor(&value) {
                    return Err(EdgeError::BadRequest(
                        "References cursor is not a canonical edge cursor".into(),
                    ));
                }
                cursor = Some(value.into_owned());
            }
            "ref" | "limit" | "cursor" => {
                return Err(EdgeError::BadRequest(format!(
                    "duplicate References query parameter `{name}`"
                )))
            }
            _ => {
                return Err(EdgeError::BadRequest(format!(
                    "unknown References query parameter `{name}`"
                )))
            }
        }
    }
    let reference = reference
        .filter(|value| !value.is_empty())
        .ok_or_else(|| EdgeError::BadRequest("References query requires `ref`".into()))?;
    Ok((reference, limit.unwrap_or(DEFAULT_BACKLINK_LIMIT), cursor))
}

struct RelatedRefsHandler {
    api: DurableRefsReadApi,
    direction: EdgeDirection,
}

impl Handler for RelatedRefsHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "References reads do not accept a request body".into(),
            ));
        }
        let (reference, limit, cursor) = parse_query(&ctx.request.query)?;
        let response = match self.direction {
            EdgeDirection::Inbound => {
                self.api
                    .backlinks(ctx.principal, &reference, limit, cursor.as_deref())?
            }
            EdgeDirection::Outbound => {
                self.api
                    .links(ctx.principal, &reference, limit, cursor.as_deref())?
            }
        };
        Ok(EdgeResponse::json(200, &response).with_header("Cache-Control", "no-store"))
    }
}

pub fn register_refs(builder: GatewayBuilder, api: DurableRefsReadApi) -> GatewayBuilder {
    builder
        .route(
            Method::Get,
            "/v1/refs/backlinks",
            "refs.backlinks.list",
            Arc::new(RelatedRefsHandler {
                api: api.clone(),
                direction: EdgeDirection::Inbound,
            }),
        )
        .route(
            Method::Get,
            "/v1/refs/links",
            "refs.links.list",
            Arc::new(RelatedRefsHandler {
                api,
                direction: EdgeDirection::Outbound,
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(edge_id: &str, source_root: &str, target_root: &str) -> StoredEdge {
        StoredEdge {
            edge_id: edge_id.into(),
            source: source_root.into(),
            source_root: source_root.into(),
            target: target_root.into(),
            target_root: target_root.into(),
            rel: "links".into(),
            rel_class: "reference".into(),
            origin_actor: "psn:alice".into(),
        }
    }

    #[test]
    fn backlink_query_is_strict_and_decodes_one_canonical_ref() {
        assert_eq!(
            parse_query("ref=myelin%3A%2F%2Facme%2Fissue%2Fissue%2FENG-41&limit=7").unwrap(),
            ("myelin://acme/issue/issue/ENG-41".into(), 7, None)
        );
        assert_eq!(
            parse_query("ref=myelin%3A%2F%2Facme%2Fissue%2Fissue%2FENG-41")
                .unwrap()
                .1,
            DEFAULT_BACKLINK_LIMIT
        );
        for query in [
            "",
            "limit=10",
            "ref=x&ref=y",
            "ref=x&limit=0",
            "ref=x&limit=101",
            "ref=x&cursor=y",
        ] {
            assert!(parse_query(query).is_err(), "accepted `{query}`");
        }
    }

    #[test]
    fn backlink_cursors_accept_both_durable_identity_generations() {
        let legacy = "0123456789abcdef0123456789abcdef";
        let strong = format!("blake3:{}", "0123456789abcdef".repeat(4));
        for cursor in [legacy, strong.as_str()] {
            let query = format!("ref=x&cursor={cursor}");
            assert_eq!(
                parse_query(&query).unwrap().2.as_deref(),
                Some(cursor),
                "the cursor remains an opaque handle across the identity migration"
            );
        }
        for cursor in [
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcde",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "blake3:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef",
            "blake3:0123456789abcdef",
        ] {
            assert!(
                parse_query(&format!("ref=x&cursor={cursor}")).is_err(),
                "non-canonical cursor `{cursor}` was accepted"
            );
        }
    }

    #[test]
    fn repository_visibility_is_derived_without_guessing_other_subsystems() {
        let repo = parse_scoped("myelin://acme/git/repo/core").unwrap();
        let pr = parse_scoped("myelin://acme/git/pr/core:42").unwrap();
        let issue = parse_scoped("myelin://acme/issue/issue/ENG-41").unwrap();
        assert_eq!(git_repo_slug(&repo), Some("core"));
        assert_eq!(git_repo_slug(&pr), Some("core"));
        assert_eq!(git_repo_slug(&issue), None);
    }

    #[test]
    fn backlink_pages_advance_without_duplicates_or_unauthorized_rows() {
        let visible = BTreeSet::from([
            "myelin://acme/knowledge/page/visible-1".into(),
            "myelin://acme/knowledge/page/visible-2".into(),
            "myelin://acme/knowledge/page/visible-3".into(),
        ]);
        let first = paginate_authorized_scan(
            vec![
                edge(
                    "01",
                    "myelin://acme/knowledge/page/hidden",
                    "myelin://acme/issue/issue/ENG-41",
                ),
                edge(
                    "02",
                    "myelin://acme/knowledge/page/visible-1",
                    "myelin://acme/issue/issue/ENG-41",
                ),
                edge(
                    "03",
                    "myelin://acme/knowledge/page/visible-2",
                    "myelin://acme/issue/issue/ENG-41",
                ),
                edge(
                    "04",
                    "myelin://acme/knowledge/page/visible-3",
                    "myelin://acme/issue/issue/ENG-41",
                ),
            ],
            &visible,
            2,
            false,
            EdgeDirection::Inbound,
        );
        assert_eq!(
            first
                .items
                .iter()
                .map(|edge| edge.edge_id.as_str())
                .collect::<Vec<_>>(),
            vec!["02", "03"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("03"));

        let second = paginate_authorized_scan(
            vec![edge(
                "04",
                "myelin://acme/knowledge/page/visible-3",
                "myelin://acme/issue/issue/ENG-41",
            )],
            &visible,
            2,
            false,
            EdgeDirection::Inbound,
        );
        assert_eq!(second.items[0].edge_id, "04");
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn an_empty_authorized_page_still_advances_its_bounded_scan() {
        let page = paginate_authorized_scan(
            vec![
                edge(
                    "01",
                    "myelin://acme/knowledge/page/hidden-1",
                    "myelin://acme/issue/issue/ENG-41",
                ),
                edge(
                    "02",
                    "myelin://acme/knowledge/page/hidden-2",
                    "myelin://acme/issue/issue/ENG-41",
                ),
            ],
            &BTreeSet::new(),
            2,
            true,
            EdgeDirection::Inbound,
        );
        assert!(page.items.is_empty());
        assert_eq!(page.next_cursor.as_deref(), Some("02"));
    }

    #[test]
    fn outgoing_pages_authorize_targets_not_the_already_authorized_source() {
        let visible = BTreeSet::from(["myelin://acme/issue/issue/ENG-42".into()]);
        let page = paginate_authorized_scan(
            vec![
                edge(
                    "01",
                    "myelin://acme/git/pr/platform:7",
                    "myelin://acme/issue/issue/SECRET-1",
                ),
                edge(
                    "02",
                    "myelin://acme/git/pr/platform:7",
                    "myelin://acme/issue/issue/ENG-42",
                ),
            ],
            &visible,
            10,
            false,
            EdgeDirection::Outbound,
        );

        assert_eq!(page.items.len(), 1);
        assert_eq!(
            page.items[0].target_root,
            "myelin://acme/issue/issue/ENG-42"
        );
        let rendered = edge_json(
            page.items.into_iter().next().unwrap(),
            EdgeDirection::Outbound,
        );
        assert_eq!(rendered["ref"], "myelin://acme/issue/issue/ENG-42");
        assert_eq!(rendered["root_ref"], "myelin://acme/issue/issue/ENG-42");
    }
}
