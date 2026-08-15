use std::sync::Arc;

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
    ) -> Result<bool, PgInboxError> {
        let issue_object =
            if subsystem_of(&row.item.subject) == myelin_notif::list_inbox::Subsystem::Issue {
                self.issue_authorization_object(principal, &row.item.subject)?
            } else {
                None
            };
        Ok(can_read_subject(
            &self.identity,
            Some(&self.git),
            principal,
            row,
            issue_object,
        ))
    }

    fn issue_authorization_object(
        &self,
        principal: &Principal,
        subject: &ArtifactRef,
    ) -> Result<Option<ArtifactRef>, PgInboxError> {
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
        Ok(issue_id.map(|id| ArtifactRef(format!("issue:{id}"))))
    }
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
        if !self
            .api
            .can_read_subject(ctx.principal, &row)
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
        if !ctx.request.query.is_empty() {
            return Err(EdgeError::BadRequest(
                "notification inbox writes accept no query parameters".into(),
            ));
        }
        if ctx.request.body.len() > MAX_INBOX_MUTATION_BYTES {
            return Err(EdgeError::PayloadTooLarge(
                "notification inbox request body is too large".into(),
            ));
        }
        if !ctx.request.body.is_empty() {
            let value = ctx.request.json_body()?;
            if value.as_object().is_none_or(|object| !object.is_empty()) {
                return Err(EdgeError::BadRequest(
                    "mark-read body must be an empty JSON object".into(),
                ));
            }
        }
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
        for row in &page.items {
            if self
                .api
                .can_read_subject(ctx.principal, row)
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

fn can_read_subject(
    identity: &StoreBackedCheck,
    git: Option<&DurableGitBackend>,
    principal: &Principal,
    row: &DurableInboxItem,
    issue_object: Option<ArtifactRef>,
) -> bool {
    let (permission, object) = match subsystem_of(&row.item.subject) {
        myelin_notif::list_inbox::Subsystem::Issue => {
            let Some(object) = issue_object else {
                return false;
            };
            ("view", object)
        }
        myelin_notif::list_inbox::Subsystem::Chat
        | myelin_notif::list_inbox::Subsystem::Knowledge
        | myelin_notif::list_inbox::Subsystem::Ci => ("read", row.item.subject.clone()),
        myelin_notif::list_inbox::Subsystem::Git => {
            if row.item.reason == myelin_notif::Reason::ReviewRequested
                && row.item.recipient == principal.principal_id.0
            {
                if let (Some(git), Some((repo, number))) = (
                    git,
                    git_pr_coordinate(&row.item.subject, &principal.tenant.0),
                ) {
                    if git.authorize_pr_review(
                        &principal.tenant.0,
                        &principal.region.0,
                        &repo,
                        number,
                        principal,
                    ) {
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
    myelin_git::gix_backend::validate_repo_slug(repo).ok()?;
    let parsed = number.parse::<u64>().ok()?;
    (parsed > 0 && parsed.to_string() == number).then_some((repo, parsed))
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
    myelin_git::gix_backend::validate_repo_slug(slug).ok()?;
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
            Arc::new(InboxMarkReadHandler { api }),
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
    fn storage_failures_never_leak_database_detail() {
        let error = map_inbox_error(PgInboxError::Database);
        assert_eq!(error.status(), 503);
        assert_eq!(
            error.envelope()["error"]["message"],
            "notification inbox is temporarily unavailable"
        );
    }

    #[test]
    fn missing_items_are_indistinguishable_from_inaccessible_items() {
        let error = map_inbox_error(PgInboxError::NotFound);
        assert_eq!(error.status(), 404);
        assert_eq!(
            error.envelope()["error"]["message"],
            "notification not found"
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
        ] {
            assert_eq!(git_repo_subject(&ArtifactRef(subject.into()), "acme"), None);
            assert_eq!(
                git_pr_coordinate(&ArtifactRef(subject.into()), "acme"),
                None
            );
        }
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
