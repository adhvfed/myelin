use std::collections::HashSet;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use myelin_events::ArtifactRef;
use myelin_storage::{with_tenant_tx_error, PgError};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPool;
use sqlx::{Postgres, QueryBuilder, Row};

use crate::list_inbox::{subsystem_of, InboxFilter, Subsystem};
use crate::migrations::{INBOX_ATTENTION_CASE_SQL, INBOX_PRIORITY_CASE_SQL};
use crate::prefs::{class_token, reason_token};
use crate::ranking::base_priority;
use crate::read_state::ReadState;
use crate::router::RoutedInboxItem;
use crate::{Class, Reason};

const MAX_PAGE_SIZE: u16 = 100;
const MAX_CURSOR_BYTES: usize = 1_024;
const MAX_CURSOR_FRAME_BYTES: usize = 768;
const MAX_KEY_BYTES: usize = 512;
const MAX_TEMPLATE_ARGS: usize = 32;
const MAX_TEMPLATE_ARGS_JSON_BYTES: usize = 16 * 1024;
const CURSOR_VERSION: u8 = 3;
const CURSOR_PREFIX: &str = "ni3_";
const SORT_ID: &str = "attention-desc:base-priority-desc:occurred-at-desc:item-id-asc:v3";

const UPSERT_SQL: &str = "INSERT INTO notif_inbox_item (
 tenant_id, region, item_id, recipient, subject, subject_root, reason, class, origin_event,
 template_key, template_args_json, dedup_key, coalesce_count, state, snooze_until, occurred_at,
 dek_ref
) VALUES (
 $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::jsonb, $12, 1, 'unread', NULL, $13, $14
)
ON CONFLICT (tenant_id, recipient, dedup_key) DO UPDATE
SET coalesce_count = notif_inbox_item.coalesce_count + 1
WHERE notif_inbox_item.region = EXCLUDED.region
  AND notif_inbox_item.item_id = EXCLUDED.item_id
  AND notif_inbox_item.subject = EXCLUDED.subject
  AND notif_inbox_item.subject_root = EXCLUDED.subject_root
  AND notif_inbox_item.reason = EXCLUDED.reason
  AND notif_inbox_item.class = EXCLUDED.class
  AND notif_inbox_item.template_key = EXCLUDED.template_key
  AND notif_inbox_item.template_args_json = EXCLUDED.template_args_json
  AND notif_inbox_item.dek_ref = EXCLUDED.dek_ref
  AND notif_inbox_item.coalesce_count >= 1
  AND notif_inbox_item.coalesce_count < 2147483647
RETURNING coalesce_count";

const ENSURE_SQL: &str = "INSERT INTO notif_inbox_item (
 tenant_id, region, item_id, recipient, subject, subject_root, reason, class, origin_event,
 template_key, template_args_json, dedup_key, coalesce_count, state, snooze_until, occurred_at,
 dek_ref
) VALUES (
 $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11::jsonb, $12, 1, 'unread', NULL, $13, $14
)
ON CONFLICT (tenant_id, recipient, dedup_key) DO UPDATE
SET item_id = notif_inbox_item.item_id
WHERE notif_inbox_item.region = EXCLUDED.region
  AND notif_inbox_item.item_id = EXCLUDED.item_id
  AND notif_inbox_item.subject = EXCLUDED.subject
  AND notif_inbox_item.subject_root = EXCLUDED.subject_root
  AND notif_inbox_item.reason = EXCLUDED.reason
  AND notif_inbox_item.class = EXCLUDED.class
  AND notif_inbox_item.origin_event = EXCLUDED.origin_event
  AND notif_inbox_item.template_key = EXCLUDED.template_key
  AND notif_inbox_item.template_args_json = EXCLUDED.template_args_json
  AND notif_inbox_item.occurred_at = EXCLUDED.occurred_at
  AND notif_inbox_item.dek_ref = EXCLUDED.dek_ref
RETURNING coalesce_count";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxUpsert {
    pub item: RoutedInboxItem,
    pub subject_root: ArtifactRef,
    pub template_key: String,
    pub template_args: Vec<ArtifactRef>,
    pub occurred_at: String,
    pub dek_ref: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboxUpsertOutcome {
    Inserted,
    Collapsed { coalesce_count: i32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxReadScope {
    pub tenant: TenantId,
    pub region: Region,
    pub recipient: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxReadRequest {
    pub scope: InboxReadScope,
    pub filter: InboxFilter,
    pub limit: u16,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableInboxItem {
    pub item: RoutedInboxItem,
    pub subject_root: ArtifactRef,
    pub template_key: String,
    pub template_args: Vec<ArtifactRef>,
    pub occurred_at: String,
    pub dek_ref: String,
    pub priority: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableInboxPage {
    pub items: Vec<DurableInboxItem>,
    pub next_cursor: Option<String>,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PgInboxError {
    InvalidInput,
    InvalidLimit,
    MalformedCursor,
    CursorScopeMismatch,
    NotFound,
    WriteConflict,
    NoCoCommitTx,
    CorruptStoredRow,
    Database,
}

impl core::fmt::Display for PgInboxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::InvalidInput => "invalid durable inbox input",
            Self::InvalidLimit => "inbox page limit must be between 1 and 100",
            Self::MalformedCursor => "malformed inbox cursor",
            Self::CursorScopeMismatch => "inbox cursor belongs to another query scope",
            Self::NotFound => "notification inbox item not found",
            Self::WriteConflict => "inbox collapse key conflicts with an existing row",
            Self::NoCoCommitTx => {
                "durable inbox co-commit requires a PostgreSQL handler transaction"
            }
            Self::CorruptStoredRow => "durable inbox row failed structural decoding",
            Self::Database => "durable inbox storage operation failed",
        };
        f.write_str(message)
    }
}

impl std::error::Error for PgInboxError {}

impl From<PgError> for PgInboxError {
    fn from(_: PgError) -> Self {
        Self::Database
    }
}

#[derive(Clone)]
pub struct PgInboxStore {
    pool: PgPool,
}

impl PgInboxStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn upsert(&self, input: &InboxUpsert) -> Result<InboxUpsertOutcome, PgInboxError> {
        let prepared = PreparedUpsert::try_from(input)?;
        let tenant = prepared.tenant_id.0.clone();
        let region = prepared.region.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { upsert_on_conn(conn, &prepared).await })
        })
        .await
    }

    /// Ensures one exact source event has one inbox row without treating a redelivery as a
    /// repeated notification. A conflicting reuse of the deduplication key is refused.
    pub async fn ensure(&self, input: &InboxUpsert) -> Result<(), PgInboxError> {
        let prepared = PreparedUpsert::try_from(input)?;
        let tenant = prepared.tenant_id.0.clone();
        let region = prepared.region.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { ensure_on_conn(conn, &prepared).await })
        })
        .await
    }

    pub fn co_commit_upsert(
        &self,
        tx: &mut myelin_events::HandlerTx<'_>,
        input: &InboxUpsert,
        runtime: &tokio::runtime::Handle,
    ) -> Result<InboxUpsertOutcome, PgInboxError> {
        let prepared = PreparedUpsert::try_from(input)?;
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(PgInboxError::NoCoCommitTx)?;
        tokio::task::block_in_place(|| runtime.block_on(upsert_on_conn(conn, &prepared)))
    }

    pub(crate) fn co_commit_contains(
        &self,
        tx: &mut myelin_events::HandlerTx<'_>,
        item: &RoutedInboxItem,
        runtime: &tokio::runtime::Handle,
    ) -> Result<bool, PgInboxError> {
        validate_routing_key(item)?;
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(PgInboxError::NoCoCommitTx)?;
        tokio::task::block_in_place(|| {
            runtime.block_on(async {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM notif_inbox_item \
                     WHERE tenant_id = $1 AND region = $2 AND recipient = $3 AND dedup_key = $4)",
                )
                .bind(&item.tenant.0)
                .bind(&item.region.0)
                .bind(&item.recipient)
                .bind(&item.dedup_key)
                .fetch_one(conn)
                .await
                .map_err(|_| PgInboxError::Database)
            })
        })
    }

    pub(crate) fn co_commit_mark_done(
        &self,
        tx: &mut myelin_events::HandlerTx<'_>,
        item: &RoutedInboxItem,
        runtime: &tokio::runtime::Handle,
    ) -> Result<bool, PgInboxError> {
        validate_routing_key(item)?;
        let conn = tx
            .connection::<sqlx::PgConnection>()
            .ok_or(PgInboxError::NoCoCommitTx)?;
        tokio::task::block_in_place(|| {
            runtime.block_on(async {
                let result = sqlx::query(
                    "UPDATE notif_inbox_item SET state = 'done', snooze_until = NULL \
                     WHERE tenant_id = $1 AND region = $2 AND recipient = $3 AND dedup_key = $4 \
                       AND state <> 'done'",
                )
                .bind(&item.tenant.0)
                .bind(&item.region.0)
                .bind(&item.recipient)
                .bind(&item.dedup_key)
                .execute(conn)
                .await
                .map_err(|_| PgInboxError::Database)?;
                Ok(result.rows_affected() == 1)
            })
        })
    }

    pub async fn list(&self, request: &InboxReadRequest) -> Result<DurableInboxPage, PgInboxError> {
        validate_request(request)?;
        let cursor = request
            .cursor
            .as_deref()
            .map(|value| decode_cursor(value, request))
            .transpose()?;
        let owned = request.clone();
        let tenant = owned.scope.tenant.0.clone();
        let region = owned.scope.region.0.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { list_on_conn(conn, &owned, cursor.as_ref()).await })
        })
        .await
    }

    pub async fn get(
        &self,
        scope: &InboxReadScope,
        item_id: &str,
    ) -> Result<DurableInboxItem, PgInboxError> {
        validate_scope(scope)?;
        if !valid_item_id(item_id) {
            return Err(PgInboxError::InvalidInput);
        }
        let owned_scope = scope.clone();
        let owned_item_id = item_id.to_string();
        let tenant = owned_scope.tenant.0.clone();
        let region = owned_scope.region.0.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { get_on_conn(conn, &owned_scope, &owned_item_id).await })
        })
        .await
    }

    pub async fn mark_read(
        &self,
        scope: &InboxReadScope,
        item_id: &str,
    ) -> Result<(), PgInboxError> {
        validate_scope(scope)?;
        if !valid_item_id(item_id) {
            return Err(PgInboxError::InvalidInput);
        }
        let owned_scope = scope.clone();
        let owned_item_id = item_id.to_string();
        let tenant = owned_scope.tenant.0.clone();
        let region = owned_scope.region.0.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move {
                let result = sqlx::query(
                    "UPDATE notif_inbox_item SET state = 'read', snooze_until = NULL \
                     WHERE tenant_id = $1 AND region = $2 AND recipient = $3 AND item_id = $4",
                )
                .bind(&owned_scope.tenant.0)
                .bind(&owned_scope.region.0)
                .bind(&owned_scope.recipient)
                .bind(&owned_item_id)
                .execute(conn)
                .await
                .map_err(|_| PgInboxError::Database)?;
                if result.rows_affected() == 1 {
                    Ok(())
                } else {
                    Err(PgInboxError::NotFound)
                }
            })
        })
        .await
    }

    /// Completes an actionable inbox item if it exists. Missing rows are valid for work created
    /// before the projection was introduced, and an already-completed row is retry-safe.
    pub async fn complete_if_present(
        &self,
        scope: &InboxReadScope,
        item_id: &str,
    ) -> Result<bool, PgInboxError> {
        validate_scope(scope)?;
        if !valid_item_id(item_id) {
            return Err(PgInboxError::InvalidInput);
        }
        let owned_scope = scope.clone();
        let owned_item_id = item_id.to_string();
        let tenant = owned_scope.tenant.0.clone();
        let region = owned_scope.region.0.clone();
        let store = self.clone();
        with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move {
                store
                    .complete_if_present_on_conn(conn, &owned_scope, &owned_item_id)
                    .await
            })
        })
        .await
    }

    pub async fn complete_if_present_on_conn(
        &self,
        conn: &mut sqlx::PgConnection,
        scope: &InboxReadScope,
        item_id: &str,
    ) -> Result<bool, PgInboxError> {
        validate_scope(scope)?;
        if !valid_item_id(item_id) {
            return Err(PgInboxError::InvalidInput);
        }
        let result = sqlx::query(
            "UPDATE notif_inbox_item SET state = 'done', snooze_until = NULL \
             WHERE tenant_id = $1 AND region = $2 AND recipient = $3 AND item_id = $4 \
               AND state <> 'done'",
        )
        .bind(&scope.tenant.0)
        .bind(&scope.region.0)
        .bind(&scope.recipient)
        .bind(item_id)
        .execute(conn)
        .await
        .map_err(|_| PgInboxError::Database)?;
        Ok(result.rows_affected() == 1)
    }
}

fn validate_routing_key(item: &RoutedInboxItem) -> Result<(), PgInboxError> {
    for field in [
        item.tenant.0.as_str(),
        item.region.0.as_str(),
        item.recipient.as_str(),
        item.dedup_key.as_str(),
    ] {
        if field.is_empty() || field.len() > MAX_KEY_BYTES || field.chars().any(char::is_control) {
            return Err(PgInboxError::InvalidInput);
        }
    }
    Ok(())
}

#[derive(Clone)]
struct PreparedUpsert {
    tenant_id: TenantId,
    region: String,
    item_id: String,
    recipient: String,
    subject: String,
    subject_root: String,
    reason: &'static str,
    class: &'static str,
    origin_event: String,
    template_key: String,
    template_args_json: String,
    dedup_key: String,
    occurred_at: DateTime<Utc>,
    dek_ref: String,
}

impl TryFrom<&InboxUpsert> for PreparedUpsert {
    type Error = PgInboxError;

    fn try_from(value: &InboxUpsert) -> Result<Self, Self::Error> {
        let item = &value.item;
        for field in [
            item.tenant.0.as_str(),
            item.region.0.as_str(),
            item.item_id.as_str(),
            item.recipient.as_str(),
            item.subject.0.as_str(),
            value.subject_root.0.as_str(),
            item.origin_event.0.as_str(),
            item.dedup_key.as_str(),
            value.template_key.as_str(),
            value.dek_ref.as_str(),
        ] {
            if field.is_empty()
                || field.len() > MAX_KEY_BYTES
                || field.chars().any(char::is_control)
            {
                return Err(PgInboxError::InvalidInput);
            }
        }
        if item.coalesce_count != 1 || item.state != "unread" || item.snooze_until.is_some() {
            return Err(PgInboxError::InvalidInput);
        }
        validate_ref_scope(&item.subject.0, &item.tenant.0)?;
        validate_ref_scope(&value.subject_root.0, &item.tenant.0)?;
        validate_ref_scope(&item.origin_event.0, &item.tenant.0)?;
        if value.subject_root.0 != crate::storm_control::subject_root_of(&item.subject.0) {
            return Err(PgInboxError::InvalidInput);
        }
        validate_dek_ref(&value.dek_ref, &item.tenant.0)?;
        if value.template_args.len() > MAX_TEMPLATE_ARGS {
            return Err(PgInboxError::InvalidInput);
        }
        for arg in &value.template_args {
            validate_ref_scope(&arg.0, &item.tenant.0)?;
        }
        let occurred_at = DateTime::parse_from_rfc3339(&value.occurred_at)
            .map_err(|_| PgInboxError::InvalidInput)?
            .with_timezone(&Utc);
        let args: Vec<&str> = value
            .template_args
            .iter()
            .map(|arg| arg.0.as_str())
            .collect();
        let template_args_json =
            serde_json::to_string(&args).map_err(|_| PgInboxError::InvalidInput)?;
        if template_args_json.len() > MAX_TEMPLATE_ARGS_JSON_BYTES {
            return Err(PgInboxError::InvalidInput);
        }
        Ok(Self {
            tenant_id: item.tenant.clone(),
            region: item.region.0.clone(),
            item_id: item.item_id.clone(),
            recipient: item.recipient.clone(),
            subject: item.subject.0.clone(),
            subject_root: value.subject_root.0.clone(),
            reason: reason_token(item.reason),
            class: class_token(item.class),
            origin_event: item.origin_event.0.clone(),
            template_key: value.template_key.clone(),
            template_args_json,
            dedup_key: item.dedup_key.clone(),
            occurred_at,
            dek_ref: value.dek_ref.clone(),
        })
    }
}

async fn upsert_on_conn(
    conn: &mut sqlx::PgConnection,
    row: &PreparedUpsert,
) -> Result<InboxUpsertOutcome, PgInboxError> {
    let count = sqlx::query_scalar::<_, i32>(UPSERT_SQL)
        .bind(&row.tenant_id.0)
        .bind(&row.region)
        .bind(&row.item_id)
        .bind(&row.recipient)
        .bind(&row.subject)
        .bind(&row.subject_root)
        .bind(row.reason)
        .bind(row.class)
        .bind(&row.origin_event)
        .bind(&row.template_key)
        .bind(&row.template_args_json)
        .bind(&row.dedup_key)
        .bind(row.occurred_at)
        .bind(&row.dek_ref)
        .fetch_optional(&mut *conn)
        .await
        .map_err(map_upsert_database_error)?
        .ok_or(PgInboxError::WriteConflict)?;
    if count == 1 {
        Ok(InboxUpsertOutcome::Inserted)
    } else if count > 1 {
        Ok(InboxUpsertOutcome::Collapsed {
            coalesce_count: count,
        })
    } else {
        Err(PgInboxError::CorruptStoredRow)
    }
}

async fn ensure_on_conn(
    conn: &mut sqlx::PgConnection,
    row: &PreparedUpsert,
) -> Result<(), PgInboxError> {
    sqlx::query_scalar::<_, i32>(ENSURE_SQL)
        .bind(&row.tenant_id.0)
        .bind(&row.region)
        .bind(&row.item_id)
        .bind(&row.recipient)
        .bind(&row.subject)
        .bind(&row.subject_root)
        .bind(row.reason)
        .bind(row.class)
        .bind(&row.origin_event)
        .bind(&row.template_key)
        .bind(&row.template_args_json)
        .bind(&row.dedup_key)
        .bind(row.occurred_at)
        .bind(&row.dek_ref)
        .fetch_optional(conn)
        .await
        .map_err(map_upsert_database_error)?
        .ok_or(PgInboxError::WriteConflict)?;
    Ok(())
}

fn map_upsert_database_error(error: sqlx::Error) -> PgInboxError {
    match error.as_database_error().and_then(|error| error.code()) {
        Some(code) if matches!(code.as_ref(), "23505" | "42501") => PgInboxError::WriteConflict,
        _ => PgInboxError::Database,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CursorFrame {
    version: u8,
    sort: String,
    scope: String,
    limit: u16,
    attention: u8,
    priority: u8,
    occurred_at: DateTime<Utc>,
    item_id: String,
}

fn validate_request(request: &InboxReadRequest) -> Result<(), PgInboxError> {
    if !(1..=MAX_PAGE_SIZE).contains(&request.limit) {
        return Err(PgInboxError::InvalidLimit);
    }
    validate_scope(&request.scope)
}

fn validate_scope(scope: &InboxReadScope) -> Result<(), PgInboxError> {
    for value in [
        scope.tenant.0.as_str(),
        scope.region.0.as_str(),
        scope.recipient.as_str(),
    ] {
        if value.is_empty() || value.len() > MAX_KEY_BYTES || value.chars().any(char::is_control) {
            return Err(PgInboxError::InvalidInput);
        }
    }
    Ok(())
}

fn valid_item_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_KEY_BYTES && !value.chars().any(char::is_control)
}

fn encode_cursor(
    request: &InboxReadRequest,
    item: &DurableInboxItem,
) -> Result<String, PgInboxError> {
    let attention = ReadState::parse(&item.item.state)
        .map(ReadState::attention_rank)
        .ok_or(PgInboxError::CorruptStoredRow)?;
    if !valid_cursor_item_id(&item.item.item_id)
        || !valid_cursor_attention(attention)
        || !valid_cursor_priority(item.priority)
    {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let occurred_at = DateTime::parse_from_rfc3339(&item.occurred_at)
        .map_err(|_| PgInboxError::CorruptStoredRow)?
        .with_timezone(&Utc);
    let frame = CursorFrame {
        version: CURSOR_VERSION,
        sort: SORT_ID.into(),
        scope: cursor_scope(request),
        limit: request.limit,
        attention,
        priority: item.priority,
        occurred_at,
        item_id: item.item.item_id.clone(),
    };
    let bytes = serde_json::to_vec(&frame).map_err(|_| PgInboxError::MalformedCursor)?;
    if bytes.len() > MAX_CURSOR_FRAME_BYTES {
        return Err(PgInboxError::MalformedCursor);
    }
    let token = format!("{CURSOR_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    if token.len() > MAX_CURSOR_BYTES {
        return Err(PgInboxError::MalformedCursor);
    }
    Ok(token)
}

fn decode_cursor(value: &str, request: &InboxReadRequest) -> Result<CursorFrame, PgInboxError> {
    if value.len() > MAX_CURSOR_BYTES {
        return Err(PgInboxError::MalformedCursor);
    }
    let encoded = value
        .strip_prefix(CURSOR_PREFIX)
        .ok_or(PgInboxError::MalformedCursor)?;
    if encoded.is_empty() || encoded.len() > MAX_CURSOR_BYTES - CURSOR_PREFIX.len() {
        return Err(PgInboxError::MalformedCursor);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| PgInboxError::MalformedCursor)?;
    if bytes.len() > MAX_CURSOR_FRAME_BYTES {
        return Err(PgInboxError::MalformedCursor);
    }
    if URL_SAFE_NO_PAD.encode(&bytes) != encoded {
        return Err(PgInboxError::MalformedCursor);
    }
    let frame: CursorFrame =
        serde_json::from_slice(&bytes).map_err(|_| PgInboxError::MalformedCursor)?;
    if frame.version != CURSOR_VERSION
        || frame.sort != SORT_ID
        || frame.limit != request.limit
        || !valid_cursor_item_id(&frame.item_id)
        || !valid_cursor_attention(frame.attention)
        || !valid_cursor_priority(frame.priority)
    {
        return Err(PgInboxError::MalformedCursor);
    }
    if frame.scope != cursor_scope(request) {
        return Err(PgInboxError::CursorScopeMismatch);
    }
    Ok(frame)
}

fn valid_cursor_item_id(value: &str) -> bool {
    valid_item_id(value)
}

fn valid_cursor_priority(value: u8) -> bool {
    matches!(value, 15 | 35 | 55 | 70 | 90)
}

fn valid_cursor_attention(value: u8) -> bool {
    value <= ReadState::Unread.attention_rank()
}

fn cursor_scope(request: &InboxReadRequest) -> String {
    let canonical = canonical_filter(&request.filter);
    let input = format!(
        "{CURSOR_VERSION}\0{SORT_ID}\0{}\0{}\0{}\0{}\0{canonical}",
        request.limit, request.scope.tenant.0, request.scope.region.0, request.scope.recipient
    );
    blake3::hash(input.as_bytes()).to_hex()[..32].to_string()
}

fn canonical_filter(filter: &InboxFilter) -> String {
    fn sorted<T: Ord>(values: impl Iterator<Item = T>) -> Vec<T> {
        let mut values = values.collect::<Vec<_>>();
        values.sort();
        values
    }
    let subsystems = filter.subsystems.as_ref().map_or_else(
        || "*".into(),
        |values| sorted(values.iter().map(|value| subsystem_cursor_token(*value))).join(","),
    );
    let reasons = filter.reasons.as_ref().map_or_else(
        || "*".into(),
        |values| sorted(values.iter().map(|value| reason_token(*value))).join(","),
    );
    format!("subsystems={subsystems};reasons={reasons}")
}

fn subsystem_cursor_token(value: Subsystem) -> &'static str {
    match value {
        Subsystem::Issue => "issue",
        Subsystem::Chat => "chat",
        Subsystem::Git => "git",
        Subsystem::Knowledge => "knowledge",
        Subsystem::Ci => "ci",
        Subsystem::Unknown => "unknown",
    }
}

async fn list_on_conn(
    conn: &mut sqlx::PgConnection,
    request: &InboxReadRequest,
    cursor: Option<&CursorFrame>,
) -> Result<DurableInboxPage, PgInboxError> {
    let mut query = build_list_query(request, cursor);
    let rows = query
        .build()
        .fetch_all(&mut *conn)
        .await
        .map_err(|_| PgInboxError::Database)?;
    let has_more = rows.len() > usize::from(request.limit);
    let items = rows
        .into_iter()
        .take(usize::from(request.limit))
        .map(|row| decode_row(&row, &request.scope))
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if has_more {
        let last = items.last().ok_or(PgInboxError::CorruptStoredRow)?;
        Some(encode_cursor(request, last)?)
    } else {
        None
    };
    Ok(DurableInboxPage { items, next_cursor })
}

async fn get_on_conn(
    conn: &mut sqlx::PgConnection,
    scope: &InboxReadScope,
    item_id: &str,
) -> Result<DurableInboxItem, PgInboxError> {
    let mut query = inbox_row_query();
    query.push(" WHERE tenant_id = ");
    query.push_bind(&scope.tenant.0);
    query.push(" AND region = ");
    query.push_bind(&scope.region.0);
    query.push(" AND recipient = ");
    query.push_bind(&scope.recipient);
    query.push(" AND item_id = ");
    query.push_bind(item_id);
    let row = query
        .build()
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| PgInboxError::Database)?
        .ok_or(PgInboxError::NotFound)?;
    decode_row(&row, scope)
}

fn inbox_row_query<'a>() -> QueryBuilder<'a, Postgres> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT tenant_id, region, item_id, recipient, subject, subject_root, reason, class, \
         origin_event, template_key, CASE WHEN octet_length(template_args_json::text) <= 16384 \
         THEN template_args_json::text END AS template_args_json, dedup_key, \
         coalesce_count, state, snooze_until, occurred_at, dek_ref, ",
    );
    query.push(INBOX_PRIORITY_CASE_SQL);
    query.push(" AS priority FROM notif_inbox_item");
    query
}

fn build_list_query<'a>(
    request: &'a InboxReadRequest,
    cursor: Option<&'a CursorFrame>,
) -> QueryBuilder<'a, Postgres> {
    let mut query = inbox_row_query();
    query.push(" WHERE tenant_id = ");
    query.push_bind(&request.scope.tenant.0);
    query.push(" AND region = ");
    query.push_bind(&request.scope.region.0);
    query.push(" AND recipient = ");
    query.push_bind(&request.scope.recipient);
    push_filter(&mut query, &request.filter);
    if let Some(cursor) = cursor {
        query.push(" AND (");
        push_attention_expression(&mut query);
        query.push(" < ");
        query.push_bind(i16::from(cursor.attention));
        query.push(" OR (");
        push_attention_expression(&mut query);
        query.push(" = ");
        query.push_bind(i16::from(cursor.attention));
        query.push(" AND ");
        push_priority_expression(&mut query);
        query.push(" < ");
        query.push_bind(i16::from(cursor.priority));
        query.push(") OR (");
        push_attention_expression(&mut query);
        query.push(" = ");
        query.push_bind(i16::from(cursor.attention));
        query.push(" AND ");
        push_priority_expression(&mut query);
        query.push(" = ");
        query.push_bind(i16::from(cursor.priority));
        query.push(" AND occurred_at < ");
        query.push_bind(cursor.occurred_at);
        query.push(") OR (");
        push_attention_expression(&mut query);
        query.push(" = ");
        query.push_bind(i16::from(cursor.attention));
        query.push(" AND ");
        push_priority_expression(&mut query);
        query.push(" = ");
        query.push_bind(i16::from(cursor.priority));
        query.push(" AND occurred_at = ");
        query.push_bind(cursor.occurred_at);
        query.push(" AND item_id > ");
        query.push_bind(&cursor.item_id);
        query.push("))");
    }
    query.push(" ORDER BY ");
    push_attention_expression(&mut query);
    query.push(" DESC, ");
    push_priority_expression(&mut query);
    query.push(" DESC, occurred_at DESC, item_id ASC LIMIT ");
    query.push_bind(i64::from(request.limit) + 1);
    query
}

fn push_attention_expression(query: &mut QueryBuilder<'_, Postgres>) {
    query.push("(");
    query.push(INBOX_ATTENTION_CASE_SQL);
    query.push(")");
}

fn push_priority_expression(query: &mut QueryBuilder<'_, Postgres>) {
    query.push("(");
    query.push(INBOX_PRIORITY_CASE_SQL);
    query.push(")");
}

fn push_filter(query: &mut QueryBuilder<'_, Postgres>, filter: &InboxFilter) {
    if let Some(reasons) = &filter.reasons {
        let values = reasons
            .iter()
            .map(|reason| reason_token(*reason).to_string())
            .collect::<Vec<_>>();
        query.push(" AND reason = ANY(");
        query.push_bind(values);
        query.push(")");
    }
    if let Some(subsystems) = &filter.subsystems {
        let (known, unknown) = subsystem_sql_tokens(subsystems);
        query.push(" AND (");
        let mut wrote = false;
        if !known.is_empty() {
            query.push("split_part(subject, '/', 4) = ANY(");
            query.push_bind(known.clone());
            query.push(")");
            wrote = true;
        }
        if unknown {
            if wrote {
                query.push(" OR ");
            }
            query.push("split_part(subject, '/', 4) <> ALL(");
            query.push_bind(all_known_subsystem_tokens());
            query.push(")");
            wrote = true;
        }
        if !wrote {
            query.push("FALSE");
        }
        query.push(")");
    }
}

fn subsystem_sql_tokens(subsystems: &HashSet<Subsystem>) -> (Vec<String>, bool) {
    let mut known = Vec::new();
    let mut unknown = false;
    for subsystem in subsystems {
        match subsystem {
            Subsystem::Issue => known.extend(["issue".into(), "issues".into()]),
            Subsystem::Chat => known.push("chat".into()),
            Subsystem::Git => known.push("git".into()),
            Subsystem::Knowledge => known.extend(["kn".into(), "knowledge".into()]),
            Subsystem::Ci => known.push("ci".into()),
            Subsystem::Unknown => unknown = true,
        }
    }
    known.sort();
    known.dedup();
    (known, unknown)
}

fn all_known_subsystem_tokens() -> Vec<String> {
    ["issue", "issues", "chat", "git", "kn", "knowledge", "ci"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn decode_row(
    row: &sqlx::postgres::PgRow,
    scope: &InboxReadScope,
) -> Result<DurableInboxItem, PgInboxError> {
    let tenant: String = get(row, "tenant_id")?;
    let region: String = get(row, "region")?;
    let recipient: String = get(row, "recipient")?;
    if tenant != scope.tenant.0 || region != scope.region.0 || recipient != scope.recipient {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let reason = parse_reason(&get::<String>(row, "reason")?)?;
    let class = parse_class(&get::<String>(row, "class")?)?;
    let state: String = get(row, "state")?;
    if !matches!(
        state.as_str(),
        "unread" | "seen" | "read" | "snoozed" | "archived" | "done"
    ) {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let subject = ArtifactRef(get(row, "subject")?);
    let subject_root = ArtifactRef(get(row, "subject_root")?);
    let origin_event = ArtifactRef(get(row, "origin_event")?);
    validate_ref_scope(&subject.0, &tenant).map_err(|_| PgInboxError::CorruptStoredRow)?;
    validate_ref_scope(&subject_root.0, &tenant).map_err(|_| PgInboxError::CorruptStoredRow)?;
    validate_ref_scope(&origin_event.0, &tenant).map_err(|_| PgInboxError::CorruptStoredRow)?;
    if subject_root.0 != crate::storm_control::subject_root_of(&subject.0) {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let template_args_json: Option<String> = get(row, "template_args_json")?;
    let template_args_json = template_args_json.ok_or(PgInboxError::CorruptStoredRow)?;
    if template_args_json.len() > MAX_TEMPLATE_ARGS_JSON_BYTES {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let template_arg_strings: Vec<String> =
        serde_json::from_str(&template_args_json).map_err(|_| PgInboxError::CorruptStoredRow)?;
    if template_arg_strings.len() > MAX_TEMPLATE_ARGS {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let mut template_args = Vec::with_capacity(template_arg_strings.len());
    for value in template_arg_strings {
        validate_ref_scope(&value, &tenant).map_err(|_| PgInboxError::CorruptStoredRow)?;
        template_args.push(ArtifactRef(value));
    }
    let occurred_at: DateTime<Utc> = get(row, "occurred_at")?;
    let snooze_until: Option<DateTime<Utc>> = get(row, "snooze_until")?;
    let coalesce_count: i32 = get(row, "coalesce_count")?;
    let priority: i32 = get(row, "priority")?;
    if coalesce_count < 1
        || !(0..=100).contains(&priority)
        || priority != i32::from(base_priority(reason))
    {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let item = RoutedInboxItem {
        tenant: TenantId(tenant),
        region: Region(region),
        item_id: get(row, "item_id")?,
        recipient,
        subject,
        reason,
        class,
        origin_event,
        dedup_key: get(row, "dedup_key")?,
        coalesce_count,
        state,
        snooze_until: snooze_until.map(format_timestamp),
    };
    if item.item_id.is_empty()
        || item.item_id.len() > MAX_KEY_BYTES
        || item.dedup_key.is_empty()
        || subsystem_of(&item.subject) == Subsystem::Unknown
            && !item.subject.0.starts_with("myelin://")
    {
        return Err(PgInboxError::CorruptStoredRow);
    }
    let dek_ref: String = get(row, "dek_ref")?;
    validate_dek_ref(&dek_ref, &item.tenant.0).map_err(|_| PgInboxError::CorruptStoredRow)?;
    Ok(DurableInboxItem {
        item,
        subject_root,
        template_key: get(row, "template_key")?,
        template_args,
        occurred_at: format_timestamp(occurred_at),
        dek_ref,
        priority: priority as u8,
    })
}

fn get<T>(row: &sqlx::postgres::PgRow, column: &str) -> Result<T, PgInboxError>
where
    for<'r> T: sqlx::Decode<'r, Postgres> + sqlx::Type<Postgres>,
{
    row.try_get(column)
        .map_err(|_| PgInboxError::CorruptStoredRow)
}

fn format_timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn validate_ref_scope(value: &str, tenant: &str) -> Result<(), PgInboxError> {
    let prefix = format!("myelin://{tenant}/");
    if value.len() > MAX_KEY_BYTES || !value.starts_with(&prefix) {
        return Err(PgInboxError::InvalidInput);
    }
    Ok(())
}

fn validate_dek_ref(value: &str, tenant: &str) -> Result<(), PgInboxError> {
    let prefix = format!("kms://{tenant}/");
    if value.len() > MAX_KEY_BYTES || !value.starts_with(&prefix) {
        return Err(PgInboxError::InvalidInput);
    }
    Ok(())
}

fn parse_reason(value: &str) -> Result<Reason, PgInboxError> {
    match value {
        "approval_requested" => Ok(Reason::ApprovalRequested),
        "escalated" => Ok(Reason::Escalated),
        "sla" => Ok(Reason::Sla),
        "review_requested" => Ok(Reason::ReviewRequested),
        "assigned" => Ok(Reason::Assigned),
        "mentioned" => Ok(Reason::Mentioned),
        "replied" => Ok(Reason::Replied),
        "agent_proposal" => Ok(Reason::AgentProposal),
        "watched" => Ok(Reason::Watched),
        "state_changed" => Ok(Reason::StateChanged),
        "fyi" => Ok(Reason::Fyi),
        "blocked" => Ok(Reason::Blocked),
        "unblocked" => Ok(Reason::Unblocked),
        "thread_watched" => Ok(Reason::ThreadWatched),
        "shared" => Ok(Reason::Shared),
        "comments" => Ok(Reason::Comments),
        _ => Err(PgInboxError::CorruptStoredRow),
    }
}

fn parse_class(value: &str) -> Result<Class, PgInboxError> {
    match value {
        "critical" => Ok(Class::Critical),
        "direct" => Ok(Class::Direct),
        "participating" => Ok(Class::Participating),
        "watching" => Ok(Class::Watching),
        "fyi" => Ok(Class::Fyi),
        _ => Err(PgInboxError::CorruptStoredRow),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(limit: u16, filter: InboxFilter) -> InboxReadRequest {
        InboxReadRequest {
            scope: InboxReadScope {
                tenant: TenantId("acme".into()),
                region: Region("fr-par".into()),
                recipient: "psn:alice".into(),
            },
            filter,
            limit,
            cursor: None,
        }
    }

    fn upsert_input() -> InboxUpsert {
        let subject = ArtifactRef("myelin://acme/git/pr/7".into());
        InboxUpsert {
            item: RoutedInboxItem {
                tenant: TenantId("acme".into()),
                region: Region("fr-par".into()),
                item_id: "itm-7".into(),
                recipient: "psn:alice".into(),
                subject: subject.clone(),
                reason: Reason::Mentioned,
                class: Class::Direct,
                origin_event: ArtifactRef("myelin://acme/bus/event/7".into()),
                dedup_key: "mention:7".into(),
                coalesce_count: 1,
                state: "unread".into(),
                snooze_until: None,
            },
            subject_root: subject.clone(),
            template_key: "git.pr.mentioned".into(),
            template_args: vec![subject],
            occurred_at: "2026-07-22T12:00:00Z".into(),
            dek_ref: "kms://acme/notif/inbox".into(),
        }
    }

    fn cursor_item(priority: u8, item_id: &str, occurred_at: &str) -> DurableInboxItem {
        let input = upsert_input();
        DurableInboxItem {
            item: RoutedInboxItem {
                item_id: item_id.into(),
                ..input.item
            },
            subject_root: input.subject_root,
            template_key: input.template_key,
            template_args: input.template_args,
            occurred_at: occurred_at.into(),
            dek_ref: input.dek_ref,
            priority,
        }
    }

    #[test]
    fn cursor_round_trip_is_bound_to_scope_filter_limit_and_sort() {
        let base = request(25, InboxFilter::git_review_requests());
        let item = cursor_item(70, "itm-7", "2026-07-22T12:00:00Z");
        let token = encode_cursor(&base, &item).unwrap();
        let decoded = decode_cursor(&token, &base).unwrap();
        assert_eq!(decoded.attention, 3);
        assert_eq!(decoded.priority, 70);
        assert_eq!(
            decoded.occurred_at,
            "2026-07-22T12:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
        assert_eq!(decoded.item_id, "itm-7");

        let mut other = base.clone();
        other.scope.recipient = "psn:bob".into();
        assert_eq!(
            decode_cursor(&token, &other),
            Err(PgInboxError::CursorScopeMismatch)
        );
        let mut other = base.clone();
        other.limit = 26;
        assert_eq!(
            decode_cursor(&token, &other),
            Err(PgInboxError::MalformedCursor)
        );
        let other = request(25, InboxFilter::issues_my_work());
        assert_eq!(
            decode_cursor(&token, &other),
            Err(PgInboxError::CursorScopeMismatch)
        );
    }

    #[test]
    fn cursor_and_limit_validation_are_strict_and_bounded() {
        for limit in [0, 101] {
            assert_eq!(
                validate_request(&request(limit, InboxFilter::all())),
                Err(PgInboxError::InvalidLimit)
            );
        }
        for token in ["", "ni2_legacy", "ni3_", "offset:20", "ni3_not-base64!"] {
            assert_eq!(
                decode_cursor(token, &request(10, InboxFilter::all())),
                Err(PgInboxError::MalformedCursor)
            );
        }
        let oversized = format!("ni3_{}", "A".repeat(MAX_CURSOR_BYTES));
        assert_eq!(
            decode_cursor(&oversized, &request(10, InboxFilter::all())),
            Err(PgInboxError::MalformedCursor)
        );
        for item in [
            cursor_item(71, "itm-7", "2026-07-22T12:00:00Z"),
            cursor_item(70, "itm\n7", "2026-07-22T12:00:00Z"),
            cursor_item(70, "itm-7", "not-a-timestamp"),
        ] {
            assert_eq!(
                encode_cursor(&request(10, InboxFilter::all()), &item),
                Err(PgInboxError::CorruptStoredRow)
            );
        }
        let mut invalid_state = cursor_item(70, "itm-7", "2026-07-22T12:00:00Z");
        invalid_state.item.state = "unknown".into();
        assert_eq!(
            encode_cursor(&request(10, InboxFilter::all()), &invalid_state),
            Err(PgInboxError::CorruptStoredRow)
        );
    }

    #[test]
    fn upsert_rejects_cross_tenant_and_oversized_template_arguments() {
        let mut input = upsert_input();
        input.template_args = vec![ArtifactRef("myelin://other/git/pr/7".into())];
        assert!(matches!(
            PreparedUpsert::try_from(&input),
            Err(PgInboxError::InvalidInput)
        ));

        let mut input = upsert_input();
        input.subject_root = ArtifactRef("myelin://acme/git/pr/not-the-subject-root".into());
        assert!(matches!(
            PreparedUpsert::try_from(&input),
            Err(PgInboxError::InvalidInput)
        ));

        let mut input = upsert_input();
        input.dek_ref = "kms://other/notif/inbox".into();
        assert!(matches!(
            PreparedUpsert::try_from(&input),
            Err(PgInboxError::InvalidInput)
        ));

        let mut input = upsert_input();
        input.template_args = (0..=MAX_TEMPLATE_ARGS)
            .map(|index| ArtifactRef(format!("myelin://acme/git/pr/{index}")))
            .collect();
        assert!(matches!(
            PreparedUpsert::try_from(&input),
            Err(PgInboxError::InvalidInput)
        ));
    }

    #[test]
    fn canonical_filter_is_order_independent() {
        let a = InboxFilter {
            subsystems: Some([Subsystem::Git, Subsystem::Issue].into_iter().collect()),
            reasons: Some([Reason::Mentioned, Reason::Assigned].into_iter().collect()),
        };
        let b = InboxFilter {
            subsystems: Some([Subsystem::Issue, Subsystem::Git].into_iter().collect()),
            reasons: Some([Reason::Assigned, Reason::Mentioned].into_iter().collect()),
        };
        assert_eq!(canonical_filter(&a), canonical_filter(&b));
        assert_eq!(cursor_scope(&request(10, a)), cursor_scope(&request(10, b)));
    }

    #[test]
    fn list_sql_is_always_fully_scoped_and_keyset_bounded() {
        let request = request(10, InboxFilter::git_review_requests());
        let cursor = CursorFrame {
            version: CURSOR_VERSION,
            sort: SORT_ID.into(),
            scope: cursor_scope(&request),
            limit: request.limit,
            attention: 3,
            priority: 70,
            occurred_at: "2026-07-22T12:00:00Z".parse().unwrap(),
            item_id: "itm-7".into(),
        };
        for sql in [
            build_list_query(&request, None).sql().to_string(),
            build_list_query(&request, Some(&cursor)).sql().to_string(),
        ] {
            assert!(sql.contains("WHERE tenant_id = "));
            assert!(sql.contains(" AND region = "));
            assert!(sql.contains(" AND recipient = "));
            assert!(sql.contains(" ORDER BY (CASE state "));
            assert!(sql.contains(") DESC, (CASE reason "));
            assert!(sql.contains(") DESC, occurred_at DESC, item_id ASC LIMIT "));
            assert!(sql.contains("octet_length(template_args_json::text) <= 16384"));
            assert!(!sql.to_ascii_uppercase().contains("OFFSET"));
        }
    }

    #[test]
    fn item_read_sql_is_fully_owner_scoped() {
        let scope = request(10, InboxFilter::all()).scope;
        let mut query = inbox_row_query();
        query.push(" WHERE tenant_id = ");
        query.push_bind(&scope.tenant.0);
        query.push(" AND region = ");
        query.push_bind(&scope.region.0);
        query.push(" AND recipient = ");
        query.push_bind(&scope.recipient);
        query.push(" AND item_id = ");
        query.push_bind("itm-7");
        let sql = query.sql();
        assert!(sql.contains("WHERE tenant_id = "));
        assert!(sql.contains(" AND region = "));
        assert!(sql.contains(" AND recipient = "));
        assert!(sql.contains(" AND item_id = "));
    }

    #[test]
    fn sql_priority_case_matches_every_frozen_reason() {
        let reasons = [
            Reason::ApprovalRequested,
            Reason::Escalated,
            Reason::Sla,
            Reason::ReviewRequested,
            Reason::Assigned,
            Reason::Mentioned,
            Reason::Replied,
            Reason::AgentProposal,
            Reason::Watched,
            Reason::StateChanged,
            Reason::Fyi,
            Reason::Blocked,
            Reason::Unblocked,
            Reason::ThreadWatched,
            Reason::Shared,
            Reason::Comments,
        ];
        for reason in reasons {
            let arm = format!(
                "WHEN '{}' THEN {}",
                reason_token(reason),
                base_priority(reason)
            );
            assert_eq!(
                INBOX_PRIORITY_CASE_SQL.matches(&arm).count(),
                1,
                "{reason:?}"
            );
            assert_eq!(
                crate::migrations::INBOX_RECENCY_KEYSET_INDEX_DDL
                    .matches(&arm)
                    .count(),
                1,
                "{reason:?}"
            );
        }
        assert!(UPSERT_SQL.contains("ON CONFLICT (tenant_id, recipient, dedup_key)"));
        assert!(crate::migrations::INBOX_RECENCY_KEYSET_INDEX_DDL
            .contains("DESC, occurred_at DESC, item_id ASC"));
        assert!(!UPSERT_SQL.contains("OFFSET"));
    }
}
