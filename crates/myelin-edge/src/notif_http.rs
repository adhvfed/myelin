use std::future::Future;
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
use serde_json::{json, Value};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::catalogue::{page_envelope, Handler, HandlerCtx, Method};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;

const DEFAULT_INBOX_LIMIT: u16 = 50;

#[derive(Clone)]
struct DurableNotifHttpApi {
    store: Arc<PgInboxStore>,
    identity: StoreBackedCheck,
    runtime: Handle,
}

impl DurableNotifHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, PgInboxError>
    where
        F: Future<Output = Result<T, PgInboxError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(PgInboxError::Database),
            Err(_) => self.runtime.block_on(future),
        }
    }
}

struct InboxListHandler {
    api: DurableNotifHttpApi,
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
            scope: InboxReadScope {
                tenant: ctx.principal.tenant.clone(),
                region: ctx.principal.region.clone(),
                recipient: ctx.principal.principal_id.0.clone(),
            },
            filter: query.view.filter(),
            limit: query.limit,
            cursor: query.cursor,
        };
        let page = self
            .api
            .drive(self.api.store.list(&request))
            .map_err(map_inbox_error)?;
        let items = page
            .items
            .iter()
            .filter(|row| can_read_subject(&self.api.identity, ctx.principal, row))
            .map(inbox_item_json)
            .collect::<Vec<_>>();
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
    })
}

fn can_read_subject(
    identity: &StoreBackedCheck,
    principal: &Principal,
    row: &DurableInboxItem,
) -> bool {
    let (permission, object) = match subsystem_of(&row.item.subject) {
        myelin_notif::list_inbox::Subsystem::Issue => ("view", row.item.subject.clone()),
        myelin_notif::list_inbox::Subsystem::Chat
        | myelin_notif::list_inbox::Subsystem::Knowledge
        | myelin_notif::list_inbox::Subsystem::Ci => ("read", row.item.subject.clone()),
        myelin_notif::list_inbox::Subsystem::Git => {
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

fn git_repo_subject(subject: &ArtifactRef, expected_tenant: &str) -> Option<ArtifactRef> {
    let rest = subject
        .0
        .strip_prefix(&format!("myelin://{expected_tenant}/git/"))?;
    let (kind, id) = rest.split_once('/')?;
    if id.is_empty() || id.contains('/') || id.contains('#') {
        return None;
    }
    let slug = match kind {
        "repo" => id.strip_prefix("repo:").unwrap_or(id),
        "pr" => id.split_once(':')?.0,
        _ => return None,
    };
    if slug.is_empty() || slug.chars().any(char::is_control) {
        return None;
    }
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
    runtime: Handle,
) -> GatewayBuilder {
    builder.route(
        Method::Get,
        "/v1/notif/inbox",
        "notif.inbox.list",
        Arc::new(InboxListHandler {
            api: DurableNotifHttpApi {
                store,
                identity,
                runtime,
            },
        }),
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
    fn git_subjects_reduce_only_to_their_same_tenant_parent_repo() {
        assert_eq!(
            git_repo_subject(&ArtifactRef("myelin://acme/git/pr/core:42".into()), "acme"),
            Some(ArtifactRef("repo:core".into()))
        );
        for subject in [
            "myelin://other/git/pr/core:42",
            "myelin://acme/git/pr/no-repo-coordinate",
            "myelin://acme/git/blob/core:deadbeef",
            "myelin://acme/git/pr/team/core:42",
        ] {
            assert_eq!(git_repo_subject(&ArtifactRef(subject.into()), "acme"), None);
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
        assert!(!can_read_subject(&identity, &principal, &row));

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
        assert!(can_read_subject(&identity, &principal, &row));
    }
}
