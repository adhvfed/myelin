use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use myelin_events::ArtifactRef;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_identity_service::StoreBackedCheck;
use myelin_notif::cli::CliView;
use myelin_notif::list_inbox::subsystem_of;
use myelin_notif::pg_inbox::{
    DurableInboxItem, InboxReadRequest, InboxReadScope, PgInboxError, PgInboxStore,
};
use myelin_notif::prefs::{class_token, reason_token, subsystem_token};
use myelin_notif::{agent_effect_approval_action, automation_approval_action};
use myelin_storage::with_tenant_tx_error;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;
use tokio::runtime::Handle;

use crate::catalogue::{page_envelope, Handler, HandlerCtx, Method};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_durable::DurableGitBackend;
use crate::request::EdgeResponse;
use crate::runtime::drive_result_on_runtime;

const DEFAULT_INBOX_LIMIT: u16 = 50;
const MAX_INBOX_MUTATION_BYTES: usize = 1_024;

#[derive(Clone)]
struct DurableNotifHttpApi {
    store: Arc<PgInboxStore>,
    identity: StoreBackedCheck,
    git: Arc<DurableGitBackend>,
    runtime: Handle,
}

impl DurableNotifHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, PgInboxError>
    where
        F: std::future::Future<Output = Result<T, PgInboxError>>,
    {
        drive_result_on_runtime(&self.runtime, future, PgInboxError::Database)
    }

    fn can_read_subject(
        &self,
        principal: &Principal,
        row: &DurableInboxItem,
        cache: &mut SubjectReadCache,
    ) -> Result<bool, PgInboxError> {
        let issue_object =
            if subsystem_of(&row.item.subject) == myelin_notif::list_inbox::Subsystem::Issue {
                self.issue_authorization_object(principal, &row.item.subject, cache)?
            } else {
                None
            };
        Ok(can_read_subject_cached(
            &self.identity,
            Some(&self.git),
            principal,
            row,
            issue_object,
            cache,
        ))
    }

    fn issue_authorization_object(
        &self,
        principal: &Principal,
        subject: &ArtifactRef,
        cache: &mut SubjectReadCache,
    ) -> Result<Option<ArtifactRef>, PgInboxError> {
        if let Some(cached) = cache.issue_objects.get(&subject.0) {
            return Ok(cached.clone());
        }
        let Some(key) = myelin_refs::object_key(subject) else {
            return Ok(None);
        };
        if key.tenant.as_deref() != Some(principal.tenant.as_str())
            || key.subsystem.as_deref() != Some("issue")
            || key.object_type.as_deref() != Some("issue")
        {
            return Ok(None);
        }
        let tenant = principal.tenant.0.clone();
        let region = principal.region.0.clone();
        let issue_key = key.id;
        let query_tenant = tenant.clone();
        let query_region = region.clone();
        let issue_id = self.drive(with_tenant_tx_error(
            self.store.pool(),
            &tenant,
            &region,
            move |conn| {
                Box::pin(async move {
                    sqlx::query_scalar::<_, Uuid>(
                        "SELECT id FROM issue \
                         WHERE tenant_id = $1 AND region = $2 AND key = $3 \
                           AND deleted_at IS NULL",
                    )
                    .bind(&query_tenant)
                    .bind(&query_region)
                    .bind(&issue_key)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|_| PgInboxError::Database)
                })
            },
        ))?;
        let object = issue_id.map(|id| ArtifactRef(format!("issue:{id}")));
        cache
            .issue_objects
            .insert(subject.0.clone(), object.clone());
        Ok(object)
    }
}

#[derive(Default)]
struct SubjectReadCache {
    issue_objects: HashMap<String, Option<ArtifactRef>>,
    decisions: HashMap<SubjectAccess, bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum SubjectAccess {
    Permission {
        permission: &'static str,
        object: String,
    },
    PullRequestReview {
        repository: String,
        number: u64,
    },
}

struct InboxListHandler {
    api: DurableNotifHttpApi,
}

struct InboxGetHandler {
    api: DurableNotifHttpApi,
}

struct InboxMarkReadHandler {
    api: DurableNotifHttpApi,
}

struct InboxSnoozeHandler {
    api: DurableNotifHttpApi,
}

struct InboxMarkAllReadHandler {
    api: DurableNotifHttpApi,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxSnoozeBody {
    until: String,
}

fn inbox_scope(principal: &Principal) -> InboxReadScope {
    InboxReadScope {
        tenant: principal.tenant.clone(),
        region: principal.region.clone(),
        recipient: principal.principal_id.0.clone(),
    }
}

impl Handler for InboxGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "notification item reads accept no query parameters or request body".into(),
            ));
        }
        let row = self
            .api
            .drive(
                self.api
                    .store
                    .get(&inbox_scope(ctx.principal), inbox_item_param(ctx)?),
            )
            .map_err(map_inbox_error)?;
        let mut cache = SubjectReadCache::default();
        if !self
            .api
            .can_read_subject(ctx.principal, &row, &mut cache)
            .map_err(map_inbox_error)?
        {
            return Err(EdgeError::NotFound("notification not found".into()));
        }
        Ok(
            EdgeResponse::json(200, &inbox_item_json(&row))
                .with_header("Cache-Control", "no-store"),
        )
    }
}

impl Handler for InboxMarkReadHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_inbox_write_query(ctx)?;
        require_optional_empty_inbox_body(ctx, "mark-read")?;
        let item_id = inbox_item_param(ctx)?;
        let scope = inbox_scope(ctx.principal);
        self.api
            .drive(self.api.store.mark_read(&scope, item_id))
            .map_err(map_inbox_error)?;
        Ok(
            EdgeResponse::json(200, &json!({ "id": item_id, "state": "read" }))
                .with_header("Cache-Control", "no-store"),
        )
    }
}

impl Handler for InboxSnoozeHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_inbox_write_query(ctx)?;
        if ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "notification snooze request body is empty".into(),
            ));
        }
        if ctx.request.body.len() > MAX_INBOX_MUTATION_BYTES {
            return Err(EdgeError::PayloadTooLarge(
                "notification inbox request body is too large".into(),
            ));
        }
        let body: InboxSnoozeBody = serde_json::from_slice(&ctx.request.body)
            .map_err(|error| EdgeError::BadRequest(format!("invalid snooze request: {error}")))?;
        let until = DateTime::parse_from_rfc3339(&body.until)
            .map_err(|_| {
                EdgeError::BadRequest("snooze `until` must be an RFC 3339 timestamp".into())
            })?
            .with_timezone(&Utc);
        if until <= Utc::now() {
            return Err(EdgeError::BadRequest(
                "snooze `until` must be in the future".into(),
            ));
        }
        let item_id = inbox_item_param(ctx)?;
        self.api
            .drive(
                self.api
                    .store
                    .snooze(&inbox_scope(ctx.principal), item_id, until),
            )
            .map_err(map_inbox_error)?;
        Ok(EdgeResponse::json(
            200,
            &json!({
                "id": item_id,
                "state": "snoozed",
                "snooze_until": until.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            }),
        )
        .with_header("Cache-Control", "no-store"))
    }
}

impl Handler for InboxMarkAllReadHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_optional_empty_inbox_body(ctx, "mark-all-read")?;
        let view = parse_inbox_view_query(&ctx.request.query)?;
        let updated = self
            .api
            .drive(
                self.api
                    .store
                    .mark_all_read(&inbox_scope(ctx.principal), &view.filter()),
            )
            .map_err(map_inbox_error)?;
        Ok(EdgeResponse::json(
            200,
            &json!({
                "state": "read",
                "view": inbox_view_name(view),
                "updated": updated,
            }),
        )
        .with_header("Cache-Control", "no-store"))
    }
}

fn require_inbox_write_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "notification inbox writes accept no query parameters".into(),
        ))
    }
}

fn require_optional_empty_inbox_body(
    ctx: &HandlerCtx<'_>,
    operation: &str,
) -> Result<(), EdgeError> {
    if ctx.request.body.len() > MAX_INBOX_MUTATION_BYTES {
        return Err(EdgeError::PayloadTooLarge(
            "notification inbox request body is too large".into(),
        ));
    }
    if ctx.request.body.is_empty() {
        return Ok(());
    }
    let value = ctx.request.json_body()?;
    if value.as_object().is_some_and(|object| object.is_empty()) {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(format!(
            "{operation} body must be an empty JSON object"
        )))
    }
}

impl Handler for InboxListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "notification inbox reads accept no request body".into(),
            ));
        }
        let query = parse_inbox_query(&ctx.request.query)?;
        let request = InboxReadRequest {
            scope: inbox_scope(ctx.principal),
            filter: query.view.filter(),
            limit: query.limit,
            cursor: query.cursor,
        };
        let page = self
            .api
            .drive(self.api.store.list(&request))
            .map_err(map_inbox_error)?;
        let mut items = Vec::with_capacity(page.items.len());
        let mut cache = SubjectReadCache::default();
        for row in &page.items {
            if self
                .api
                .can_read_subject(ctx.principal, row, &mut cache)
                .map_err(map_inbox_error)?
            {
                items.push(inbox_item_json(row));
            }
        }
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!(items), page.next_cursor, usize::from(query.limit)),
        )
        .with_header("Cache-Control", "no-store"))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct InboxQuery {
    view: CliView,
    limit: u16,
    cursor: Option<String>,
}

fn parse_inbox_query(query: &str) -> Result<InboxQuery, EdgeError> {
    let mut view = None;
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=').ok_or_else(|| {
                EdgeError::BadRequest("malformed notification inbox query parameter".into())
            })?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!(
                    "duplicate notification inbox query parameter `{field}`"
                ))
            };
            match name {
                "view" => {
                    if view.is_some() {
                        return Err(duplicate("view"));
                    }
                    view = Some(CliView::parse(Some(value)).map_err(EdgeError::BadRequest)?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(duplicate("limit"));
                    }
                    let parsed = value.parse::<u16>().map_err(|_| {
                        EdgeError::BadRequest("limit must be an integer between 1 and 100".into())
                    })?;
                    if !(1..=100).contains(&parsed) {
                        return Err(EdgeError::BadRequest(
                            "limit must be an integer between 1 and 100".into(),
                        ));
                    }
                    limit = Some(parsed);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(duplicate("cursor"));
                    }
                    if value.is_empty() {
                        return Err(EdgeError::BadRequest(
                            "notification inbox cursor must not be empty".into(),
                        ));
                    }
                    cursor = Some(value.to_string());
                }
                "" => {
                    return Err(EdgeError::BadRequest(
                        "empty notification inbox query parameter".into(),
                    ))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown notification inbox query parameter `{other}`"
                    )))
                }
            }
        }
    }
    Ok(InboxQuery {
        view: view.unwrap_or(CliView::All),
        limit: limit.unwrap_or(DEFAULT_INBOX_LIMIT),
        cursor,
    })
}

fn parse_inbox_view_query(query: &str) -> Result<CliView, EdgeError> {
    if query.is_empty() {
        return Ok(CliView::All);
    }
    let (name, value) = query
        .split_once('=')
        .ok_or_else(|| EdgeError::BadRequest("malformed notification inbox view query".into()))?;
    if name != "view" || value.contains('&') || value.is_empty() {
        return Err(EdgeError::BadRequest(
            "mark-all-read accepts only one `view` query parameter".into(),
        ));
    }
    CliView::parse(Some(value)).map_err(EdgeError::BadRequest)
}

fn inbox_view_name(view: CliView) -> &'static str {
    match view {
        CliView::All => "all",
        CliView::MyWork => "my-work",
        CliView::Activity => "activity",
        CliView::ReviewRequests => "review-requests",
    }
}

fn inbox_item_json(row: &DurableInboxItem) -> Value {
    let action = agent_effect_approval_action(row)
        .map(|action| {
            json!({
                "kind": "agent_effect_approval",
                "gate_id": action.gate_id,
                "run_id": action.run_id,
            })
        })
        .or_else(|| {
            automation_approval_action(row).map(|action| {
                json!({
                    "kind": "automation_firing_approval",
                    "automation_id": action.automation_id,
                    "event_id": action.event_id,
                })
            })
        });
    json!({
        "id": row.item.item_id,
        "reason": reason_token(row.item.reason),
        "class": class_token(row.item.class),
        "subsystem": subsystem_token(subsystem_of(&row.item.subject)),
        "subject": row.item.subject.0,
        "subject_root": row.subject_root.0,
        "coalesce_count": row.item.coalesce_count,
        "state": row.item.state,
        "snooze_until": row.item.snooze_until,
        "occurred_at": row.occurred_at,
        "priority": row.priority,
        "action": action,
    })
}

fn inbox_item_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("item")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an inbox item id".into()))?;
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "invalid notification inbox item id".into(),
        ));
    }
    Ok(value)
}

#[cfg(test)]
fn can_read_subject(
    identity: &StoreBackedCheck,
    git: Option<&DurableGitBackend>,
    principal: &Principal,
    row: &DurableInboxItem,
    issue_object: Option<ArtifactRef>,
) -> bool {
    can_read_subject_cached(
        identity,
        git,
        principal,
        row,
        issue_object,
        &mut SubjectReadCache::default(),
    )
}

fn can_read_subject_cached(
    identity: &StoreBackedCheck,
    git: Option<&DurableGitBackend>,
    principal: &Principal,
    row: &DurableInboxItem,
    issue_object: Option<ArtifactRef>,
    cache: &mut SubjectReadCache,
) -> bool {
    let (permission, object) = match subsystem_of(&row.item.subject) {
        myelin_notif::list_inbox::Subsystem::Issue => {
            let Some(object) = issue_object else {
                return false;
            };
            ("view", object)
        }
        myelin_notif::list_inbox::Subsystem::Chat => {
            let Some(access) = chat_subject_access(&row.item.subject, &principal.tenant.0) else {
                return false;
            };
            access
        }
        myelin_notif::list_inbox::Subsystem::Knowledge
        | myelin_notif::list_inbox::Subsystem::Ci => ("read", row.item.subject.clone()),
        myelin_notif::list_inbox::Subsystem::Git => {
            if row.item.reason == myelin_notif::Reason::ReviewRequested
                && row.item.recipient == principal.principal_id.0
            {
                if let (Some(git), Some((repo, number))) = (
                    git,
                    git_pr_coordinate(&row.item.subject, &principal.tenant.0),
                ) {
                    let access = SubjectAccess::PullRequestReview {
                        repository: repo.clone(),
                        number,
                    };
                    let allowed = *cache.decisions.entry(access).or_insert_with(|| {
                        git.authorize_pr_review(
                            &principal.tenant.0,
                            &principal.region.0,
                            &repo,
                            number,
                            principal,
                        )
                    });
                    if allowed {
                        return true;
                    }
                }
            }
            let Some(repo) = git_repo_subject(&row.item.subject, &principal.tenant.0) else {
                return false;
            };
            ("pull", repo)
        }
        myelin_notif::list_inbox::Subsystem::Unknown => return false,
    };
    let access = SubjectAccess::Permission {
        permission,
        object: object.0.clone(),
    };
    *cache.decisions.entry(access).or_insert_with(|| {
        matches!(
            identity.check(
                principal,
                &Permission(permission.into()),
                &object,
                &Consistency {
                    at_least: Zookie(String::new()),
                    mode: ConsistencyMode::Strong,
                },
                None,
            ),
            Ok(Decision::Allow)
        )
    })
}

fn chat_subject_access(
    subject: &ArtifactRef,
    expected_tenant: &str,
) -> Option<(&'static str, ArtifactRef)> {
    let parsed = myelin_refs::parse_scoped(&subject.0).ok()?;
    if parsed.tenant.as_str() != expected_tenant
        || parsed.subsystem != "chat"
        || parsed.id.is_empty()
    {
        return None;
    }
    match parsed.type_.as_str() {
        "channel" if parsed.sub.is_none() => Some((
            "read",
            ArtifactRef(myelin_chat::membership::channel_object(&parsed.id)),
        )),
        "message" => Some((
            "view",
            ArtifactRef(format!(
                "{}:{}",
                myelin_chat::rebac_fragment::object_types::MESSAGE,
                parsed.id
            )),
        )),
        // A thread is rooted in a message and inherits that message's channel.
        "thread" => Some((
            "view",
            ArtifactRef(format!(
                "{}:{}",
                myelin_chat::rebac_fragment::object_types::MESSAGE,
                parsed.id
            )),
        )),
        _ => None,
    }
}

fn git_pr_coordinate(subject: &ArtifactRef, expected_tenant: &str) -> Option<(String, u64)> {
    let parsed = myelin_refs::parse_scoped(&subject.0).ok()?;
    if parsed.tenant.as_str() != expected_tenant
        || parsed.subsystem != "git"
        || parsed.type_ != "pr"
        || parsed.sub.is_some()
    {
        return None;
    }
    let (repo, number) = git_pr_parts(&parsed.id)?;
    Some((repo.to_string(), number))
}

fn git_pr_parts(id: &str) -> Option<(&str, u64)> {
    let (repo, number) = id.rsplit_once(':')?;
    myelin_git::coordinate::RepositorySlug::parse(repo).ok()?;
    myelin_git::coordinate::parse_positive_decimal(number).map(|parsed| (repo, parsed))
}

fn git_repo_subject(subject: &ArtifactRef, expected_tenant: &str) -> Option<ArtifactRef> {
    let parsed = myelin_refs::parse_scoped(&subject.0).ok()?;
    if parsed.tenant.as_str() != expected_tenant
        || parsed.subsystem != "git"
        || parsed.sub.is_some()
    {
        return None;
    }
    let slug = match parsed.type_.as_str() {
        "repo" => parsed.id.strip_prefix("repo:").unwrap_or(&parsed.id),
        "pr" => git_pr_parts(&parsed.id)?.0,
        _ => return None,
    };
    myelin_git::coordinate::RepositorySlug::parse(slug).ok()?;
    Some(ArtifactRef(format!("repo:{slug}")))
}

fn map_inbox_error(error: PgInboxError) -> EdgeError {
    match error {
        PgInboxError::InvalidInput
        | PgInboxError::InvalidLimit
        | PgInboxError::MalformedCursor
        | PgInboxError::CursorScopeMismatch => {
            EdgeError::BadRequest("invalid notification inbox page request".into())
        }
        PgInboxError::NotFound => EdgeError::NotFound("notification not found".into()),
        PgInboxError::InvalidState => {
            EdgeError::Conflict("notification is no longer active".into())
        }
        PgInboxError::Database => {
            EdgeError::Unavailable("notification inbox is temporarily unavailable".into())
        }
        PgInboxError::CorruptStoredRow
        | PgInboxError::WriteConflict
        | PgInboxError::NoCoCommitTx => EdgeError::Internal(error.to_string()),
        _ => EdgeError::Internal("unrecognized notification inbox failure".into()),
    }
}

pub fn register_notif(
    builder: GatewayBuilder,
    store: Arc<PgInboxStore>,
    identity: StoreBackedCheck,
    git: Arc<DurableGitBackend>,
    runtime: Handle,
) -> GatewayBuilder {
    let api = DurableNotifHttpApi {
        store,
        identity,
        git,
        runtime,
    };
    builder
        .route(
            Method::Get,
            "/v1/notif/inbox",
            "notif.inbox.list",
            Arc::new(InboxListHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/notif/inbox/{item}",
            "notif.inbox.get",
            Arc::new(InboxGetHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/notif/inbox/{item}/read",
            "notif.inbox.mark_read",
            Arc::new(InboxMarkReadHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/notif/inbox/{item}/snooze",
            "notif.inbox.snooze",
            Arc::new(InboxSnoozeHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/notif/inbox/read",
            "notif.inbox.mark_all_read",
            Arc::new(InboxMarkAllReadHandler { api }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{OutboxStore, Timestamp};
    use myelin_identity::{
        DataRole, ObjectId, PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple,
        TupleDelta,
    };
    use myelin_identity_service::TupleStore;
    use myelin_notif::router::RoutedInboxItem;
    use myelin_notif::{Class, Reason};
    use myelin_storage::TenantScope;
    use myelin_tenancy::{Region, TenantId};

    #[test]
    fn inbox_query_is_strict_bounded_and_defaults_to_all() {
        assert_eq!(
            parse_inbox_query("").unwrap(),
            InboxQuery {
                view: CliView::All,
                limit: 50,
                cursor: None
            }
        );
        assert_eq!(
            parse_inbox_query("view=review-requests&limit=25&cursor=ni1_abc").unwrap(),
            InboxQuery {
                view: CliView::ReviewRequests,
                limit: 25,
                cursor: Some("ni1_abc".into()),
            }
        );
        for query in [
            "limit=0",
            "limit=101",
            "limit=banana",
            "cursor=",
            "view=everything",
            "tenant=acme",
            "recipient=alice",
            "region=eu-west",
            "limit=1&limit=2",
            "bare",
        ] {
            assert_eq!(
                parse_inbox_query(query).unwrap_err().status(),
                400,
                "{query}"
            );
        }
    }

    #[test]
    fn inbox_state_mutations_have_one_exact_input_shape() {
        assert_eq!(parse_inbox_view_query("").unwrap(), CliView::All);
        assert_eq!(
            parse_inbox_view_query("view=my-work").unwrap(),
            CliView::MyWork
        );
        for query in [
            "view=",
            "view=all&view=my-work",
            "limit=1",
            "cursor=ni1_abc",
            "bare",
        ] {
            assert_eq!(parse_inbox_view_query(query).unwrap_err().status(), 400);
        }

        let body: InboxSnoozeBody =
            serde_json::from_slice(br#"{"until":"2026-08-20T15:00:00Z"}"#).unwrap();
        assert_eq!(body.until, "2026-08-20T15:00:00Z");
        for body in [
            br#"{}"#.as_slice(),
            br#"{"until":42}"#.as_slice(),
            br#"{"until":"2026-08-20T15:00:00Z","item":"other"}"#.as_slice(),
        ] {
            assert!(serde_json::from_slice::<InboxSnoozeBody>(body).is_err());
        }
    }

    #[test]
    fn storage_failures_never_leak_database_detail() {
        let error = map_inbox_error(PgInboxError::Database);
        assert_eq!(error.status(), 503);
        assert_eq!(
            error.envelope()["error"]["message"],
            "notification inbox is temporarily unavailable"
        );
    }

    #[test]
    fn absence_and_terminal_state_are_distinct_safe_failures() {
        let error = map_inbox_error(PgInboxError::NotFound);
        assert_eq!(error.status(), 404);
        assert_eq!(
            error.envelope()["error"]["message"],
            "notification not found"
        );

        let error = map_inbox_error(PgInboxError::InvalidState);
        assert_eq!(error.status(), 409);
        assert_eq!(
            error.envelope()["error"]["message"],
            "notification is no longer active"
        );
    }

    #[test]
    fn git_subjects_reduce_only_to_their_same_tenant_parent_repo() {
        assert_eq!(
            git_repo_subject(&ArtifactRef("myelin://acme/git/pr/core:42".into()), "acme"),
            Some(ArtifactRef("repo:core".into()))
        );
        assert_eq!(
            git_pr_coordinate(&ArtifactRef("myelin://acme/git/pr/core:42".into()), "acme"),
            Some(("core".into(), 42))
        );
        assert_eq!(
            git_repo_subject(
                &ArtifactRef("myelin://acme/git/pr/team/core:42".into()),
                "acme"
            ),
            Some(ArtifactRef("repo:team/core".into()))
        );
        assert_eq!(
            git_repo_subject(
                &ArtifactRef("myelin://acme/git/repo/team/core".into()),
                "acme"
            ),
            Some(ArtifactRef("repo:team/core".into()))
        );
        assert_eq!(
            git_pr_coordinate(
                &ArtifactRef("myelin://acme/git/pr/team/core:42".into()),
                "acme"
            ),
            Some(("team/core".into(), 42))
        );
        for subject in [
            "myelin://other/git/pr/core:42",
            "myelin://acme/git/pr/no-repo-coordinate",
            "myelin://acme/git/blob/core:deadbeef",
            "myelin://acme/git/pr/team//core:42",
            "myelin://acme/git/pr/team/core:0",
            "myelin://acme/git/pr/team/core:01",
            "myelin://acme/git/pr/team.git/core:42",
        ] {
            assert_eq!(git_repo_subject(&ArtifactRef(subject.into()), "acme"), None);
            assert_eq!(
                git_pr_coordinate(&ArtifactRef(subject.into()), "acme"),
                None
            );
        }
    }

    #[test]
    fn chat_subjects_reduce_to_the_exact_rebac_object_that_owns_visibility() {
        let message_id = "01J00000000000000000000000";
        assert_eq!(
            chat_subject_access(
                &ArtifactRef(format!(
                    "myelin://acme/chat/message/{message_id}#message-{message_id}"
                )),
                "acme"
            ),
            Some(("view", ArtifactRef(format!("message:{message_id}"))))
        );
        assert_eq!(
            chat_subject_access(
                &ArtifactRef("myelin://acme/chat/channel/room-1".into()),
                "acme"
            ),
            Some(("read", ArtifactRef("channel:room-1".into())))
        );
        assert_eq!(
            chat_subject_access(
                &ArtifactRef(format!(
                    "myelin://acme/chat/thread/{message_id}#thread-{message_id}"
                )),
                "acme"
            ),
            Some(("view", ArtifactRef(format!("message:{message_id}"))))
        );
        assert_eq!(
            chat_subject_access(
                &ArtifactRef("myelin://other/chat/channel/room-1".into()),
                "acme"
            ),
            None
        );
    }

    #[test]
    fn subject_access_is_reconfirmed_and_denial_is_held() {
        let tuples = TupleStore::new(OutboxStore::new());
        let identity = StoreBackedCheck::new(tuples.clone());
        for result in identity.admit_git_fragment() {
            assert!(matches!(
                result,
                myelin_identity::FragmentAdmit::Admitted { .. }
            ));
        }
        let principal = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("psn:alice".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let row = DurableInboxItem {
            item: RoutedInboxItem {
                tenant: principal.tenant.clone(),
                region: principal.region.clone(),
                item_id: "item-1".into(),
                recipient: principal.principal_id.0.clone(),
                subject: ArtifactRef("myelin://acme/git/pr/core:42".into()),
                reason: Reason::ReviewRequested,
                class: Class::Direct,
                origin_event: ArtifactRef("myelin://acme/bus/event/event-1".into()),
                dedup_key: "review:core:42".into(),
                coalesce_count: 1,
                state: "unread".into(),
                snooze_until: None,
            },
            subject_root: ArtifactRef("myelin://acme/git/pr/core:42".into()),
            template_key: "git.review_requested".into(),
            template_args: Vec::new(),
            occurred_at: "2026-07-22T12:00:00Z".into(),
            dek_ref: "kms://acme/notif/inbox".into(),
            priority: 70,
        };
        assert!(!can_read_subject(&identity, None, &principal, &row, None));

        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        tuples
            .write_tuples(
                &scope,
                &principal,
                &[TupleDelta::Add(RelationTuple {
                    object: ObjectId("repo:core".into()),
                    relation: RelName("reader".into()),
                    subject: principal.principal_id.clone(),
                    caveat: None,
                })],
                None,
                None,
                Timestamp("2026-07-22T12:00:00Z".into()),
            )
            .unwrap();
        assert!(can_read_subject(&identity, None, &principal, &row, None));
    }
}
