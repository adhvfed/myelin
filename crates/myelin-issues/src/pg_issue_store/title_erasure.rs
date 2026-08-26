use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventType, IdMinter,
    Timestamp, Visibility,
};
use myelin_storage::kms::PiiKeyRef;
use myelin_storage::pgrelay::PgRelay;
use myelin_storage::with_tenant_tx_error;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
use sqlx::types::Uuid;
use sqlx::Row;

use super::{IssueAuthorizer, IssueStoreError, PgIssueStore};
use crate::dek::issue_subject_key_class;
use crate::events::ISSUE_UPDATED;

const TITLE_ERASURE_BATCH_MAX: i64 = 100;
const ERASED_TITLE: &str = "[erased issue title]";

#[derive(Clone, Debug)]
pub struct IssueTitleErasureAttempt {
    operation_id: String,
    actor: Actor,
    occurred_at: Timestamp,
    recorded_at: Timestamp,
    unix_seconds: i64,
}

impl IssueTitleErasureAttempt {
    pub fn new(
        operation_id: impl Into<String>,
        actor: Actor,
        observed: myelin_events::clock::ClockReading,
    ) -> Result<Self, IssueStoreError> {
        let operation_id = operation_id.into();
        validate_operation_id(&operation_id)?;
        Ok(Self {
            operation_id,
            actor,
            occurred_at: observed.timestamp(),
            recorded_at: observed.timestamp(),
            unix_seconds: observed.unix_seconds(),
        })
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn completed_at_offset(&self) -> Result<u64, IssueStoreError> {
        u64::try_from(self.unix_seconds)
            .map_err(|_| IssueStoreError::Clock(myelin_events::clock::ClockError::BeforeUnixEpoch))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredIssueTitleEraseReceipt {
    pub titles_tombstoned: u64,
    pub erasure_events_co_committed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoredIssueTitleErasureState {
    Pending,
    Completed(AuthoredIssueTitleEraseReceipt),
}

pub(crate) struct VerifiedIssueTitleErasureAttempt {
    subject: String,
    attempt: IssueTitleErasureAttempt,
}

impl VerifiedIssueTitleErasureAttempt {
    pub(crate) fn after_key_destruction(
        subject: impl Into<String>,
        attempt: IssueTitleErasureAttempt,
    ) -> Self {
        Self {
            subject: subject.into(),
            attempt,
        }
    }
}

#[derive(Debug)]
struct TombstonedTitle {
    id: Uuid,
    key: String,
    type_id: Uuid,
    version: i64,
}

impl<A: IssueAuthorizer> PgIssueStore<A> {
    pub async fn prepare_title_erasure(
        &self,
        tenant: &str,
        subject: &str,
        operation_id: &str,
    ) -> Result<AuthoredIssueTitleErasureState, IssueStoreError> {
        validate_erasure_identity(tenant, subject, operation_id)?;
        let region = self.provider.config().region.clone();
        let tenant_owned = tenant.to_string();
        let subject_owned = subject.to_string();
        let operation_owned = operation_id.to_string();
        with_tenant_tx_error(
            self.provider.db_pool(),
            tenant,
            &region.clone(),
            move |connection| {
                Box::pin(async move {
                    lock_title_subject(
                        connection,
                        &tenant_owned,
                        &subject_owned,
                        SubjectLock::Exclusive,
                    )
                    .await?;

                    if let Some(row) =
                        load_marker_optional(connection, &tenant_owned, &region, &operation_owned)
                            .await?
                    {
                        require_same_subject(&row, &subject_owned)?;
                        return erasure_state_from_row(&row);
                    }

                    let another_operation: Option<String> = sqlx::query_scalar(
                        "SELECT operation_id FROM issue_title_erasure_operation \
                         WHERE tenant_id = $1 AND region = $2 AND subject = $3 \
                           AND completed_at IS NULL",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject_owned)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(storage_error("inspect pending Issue-title erasure"))?;
                    if another_operation.is_some() {
                        return Err(IssueStoreError::Conflict(
                            "another Issue-title erasure is already in progress for this subject"
                                .into(),
                        ));
                    }

                    sqlx::query(
                        "INSERT INTO issue_title_erasure_operation \
                         (tenant_id, region, operation_id, subject) VALUES ($1, $2, $3, $4)",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&operation_owned)
                    .bind(&subject_owned)
                    .execute(&mut *connection)
                    .await
                    .map_err(storage_error("prepare Issue-title erasure"))?;
                    Ok(AuthoredIssueTitleErasureState::Pending)
                })
            },
        )
        .await
    }

    pub async fn verify_title_erasure_ready(
        &self,
        tenant: &str,
        subject: &str,
        operation_id: &str,
    ) -> Result<(), IssueStoreError> {
        validate_erasure_identity(tenant, subject, operation_id)?;
        loop {
            let region = self.provider.config().region.clone();
            let tenant_owned = tenant.to_string();
            let subject_owned = subject.to_string();
            let operation_owned = operation_id.to_string();
            let complete = with_tenant_tx_error(
                self.provider.db_pool(),
                tenant,
                &region.clone(),
                move |connection| {
                    Box::pin(async move {
                        lock_title_subject(
                            connection,
                            &tenant_owned,
                            &subject_owned,
                            SubjectLock::Exclusive,
                        )
                        .await?;
                        let marker =
                            load_marker(connection, &tenant_owned, &region, &operation_owned)
                                .await?;
                        require_same_subject(&marker, &subject_owned)?;
                        if matches!(
                            erasure_state_from_row(&marker)?,
                            AuthoredIssueTitleErasureState::Completed(_)
                        ) || marker
                            .try_get::<bool, _>("validation_completed")
                            .map_err(row_decode)?
                        {
                            return Ok(true);
                        }
                        validate_next_title_batch(
                            connection,
                            &tenant_owned,
                            &region,
                            &subject_owned,
                            &operation_owned,
                            &marker,
                        )
                        .await
                    })
                },
            )
            .await?;
            if complete {
                return Ok(());
            }
        }
    }

    pub(crate) async fn tombstone_titles_co_commit(
        &self,
        tenant: &str,
        verified: VerifiedIssueTitleErasureAttempt,
    ) -> Result<AuthoredIssueTitleEraseReceipt, IssueStoreError> {
        let VerifiedIssueTitleErasureAttempt { subject, attempt } = verified;
        validate_erasure_identity(tenant, &subject, &attempt.operation_id)?;
        let expected_region = self.provider.config().region.as_str();
        if attempt.actor.0.tenant.as_str() != tenant
            || attempt.actor.0.region.as_str() != expected_region
        {
            return Err(IssueStoreError::BadInput(
                "Issue-title erasure actor must belong to the affected tenant and region".into(),
            ));
        }
        loop {
            if let Some(receipt) = self
                .tombstone_next_title_batch(tenant, &subject, &attempt)
                .await?
            {
                return Ok(receipt);
            }
        }
    }

    async fn tombstone_next_title_batch(
        &self,
        tenant: &str,
        subject: &str,
        attempt: &IssueTitleErasureAttempt,
    ) -> Result<Option<AuthoredIssueTitleEraseReceipt>, IssueStoreError> {
        let region = self.provider.config().region.clone();
        let tenant_owned = tenant.to_string();
        let subject_owned = subject.to_string();
        let attempt = attempt.clone();
        let event_ids = self.minter.clone();
        with_tenant_tx_error(
            self.provider.db_pool(),
            tenant,
            &region.clone(),
            move |connection| {
                Box::pin(async move {
                    lock_title_subject(
                        connection,
                        &tenant_owned,
                        &subject_owned,
                        SubjectLock::Exclusive,
                    )
                    .await?;
                    let marker =
                        load_marker(connection, &tenant_owned, &region, &attempt.operation_id)
                            .await?;
                    require_same_subject(&marker, &subject_owned)?;
                    if let AuthoredIssueTitleErasureState::Completed(receipt) =
                        erasure_state_from_row(&marker)?
                    {
                        return Ok(Some(receipt));
                    }
                    if !marker
                        .try_get::<bool, _>("validation_completed")
                        .map_err(row_decode)?
                    {
                        return Err(IssueStoreError::Storage(
                            "Issue titles cannot be erased before key-scope validation".into(),
                        ));
                    }

                    let rows = sqlx::query(
                        "WITH batch AS ( \
                           SELECT tenant_id, region, id FROM issue \
                           WHERE tenant_id = $1 AND region = $2 AND title_subject = $3 \
                             AND NOT title_erased ORDER BY id LIMIT $4 FOR UPDATE \
                         ) \
                         UPDATE issue AS item \
                            SET title = $5, title_nonce = NULL, title_ciphertext = NULL, \
                                pii_key_ref = NULL, title_subject = NULL, \
                                created_by_principal = NULL, title_erased = true, \
                                version = item.version + 1, updated_at = to_timestamp($6) \
                           FROM batch \
                          WHERE item.tenant_id = batch.tenant_id \
                            AND item.region = batch.region AND item.id = batch.id \
                         RETURNING item.id, item.key, item.type_id, item.version",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject_owned)
                    .bind(TITLE_ERASURE_BATCH_MAX)
                    .bind(ERASED_TITLE)
                    .bind(attempt.unix_seconds)
                    .fetch_all(&mut *connection)
                    .await
                    .map_err(storage_error("tombstone Issue-title batch"))?;
                    let tombstones = rows
                        .into_iter()
                        .map(|row| {
                            Ok(TombstonedTitle {
                                id: row.try_get("id").map_err(row_decode)?,
                                key: row.try_get("key").map_err(row_decode)?,
                                type_id: row.try_get("type_id").map_err(row_decode)?,
                                version: row.try_get("version").map_err(row_decode)?,
                            })
                        })
                        .collect::<Result<Vec<_>, IssueStoreError>>()?;

                    for title in &tombstones {
                        let envelope = title_erasure_event(
                            &tenant_owned,
                            &region,
                            title,
                            event_ids.as_ref(),
                            &attempt,
                        );
                        PgRelay::co_commit_in_tx(
                            &mut *connection,
                            &envelope.aggregate.0,
                            &envelope,
                        )
                        .await?;
                    }

                    let batch_count = i64::try_from(tombstones.len()).map_err(|_| {
                        IssueStoreError::Storage("Issue-title erasure count overflowed".into())
                    })?;
                    let progress: i64 = sqlx::query_scalar(
                        "UPDATE issue_title_erasure_operation \
                            SET titles_erased_so_far = titles_erased_so_far + $4, \
                                events_emitted_so_far = events_emitted_so_far + $4 \
                          WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
                            AND completed_at IS NULL \
                        RETURNING titles_erased_so_far",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&attempt.operation_id)
                    .bind(batch_count)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(storage_error("advance Issue-title erasure progress"))?;
                    let more_titles: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM issue \
                         WHERE tenant_id = $1 AND region = $2 AND title_subject = $3 \
                           AND NOT title_erased)",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&subject_owned)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(storage_error("check remaining Issue titles"))?;
                    if more_titles {
                        return Ok(None);
                    }

                    let completed = sqlx::query(
                        "UPDATE issue_title_erasure_operation \
                            SET completed_at = to_timestamp($4), \
                                titles_erased = titles_erased_so_far, \
                                events_emitted = events_emitted_so_far \
                          WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
                            AND completed_at IS NULL",
                    )
                    .bind(&tenant_owned)
                    .bind(&region)
                    .bind(&attempt.operation_id)
                    .bind(attempt.unix_seconds)
                    .execute(&mut *connection)
                    .await
                    .map_err(storage_error("complete Issue-title erasure"))?;
                    if completed.rows_affected() != 1 {
                        return Err(IssueStoreError::Storage(
                            "Issue-title erasure marker did not complete exactly once".into(),
                        ));
                    }
                    let titles_tombstoned = u64::try_from(progress).map_err(|_| {
                        IssueStoreError::Storage("Issue-title erasure count is negative".into())
                    })?;
                    Ok(Some(AuthoredIssueTitleEraseReceipt {
                        titles_tombstoned,
                        erasure_events_co_committed: titles_tombstoned,
                    }))
                })
            },
        )
        .await
    }
}

pub(super) async fn refuse_title_creation_during_erasure(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    subject: &str,
) -> Result<(), IssueStoreError> {
    lock_title_subject(connection, tenant, subject, SubjectLock::Shared).await?;
    let erasing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM issue_title_erasure_operation \
         WHERE tenant_id = $1 AND region = $2 AND subject = $3 AND completed_at IS NULL)",
    )
    .bind(tenant)
    .bind(region)
    .bind(subject)
    .fetch_one(connection)
    .await
    .map_err(storage_error("check Issue-title erasure fence"))?;
    if erasing {
        return Err(IssueStoreError::Conflict(
            "an Issue title cannot be created while its subject erasure is in progress".into(),
        ));
    }
    Ok(())
}

async fn validate_next_title_batch(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    subject: &str,
    operation_id: &str,
    marker: &sqlx::postgres::PgRow,
) -> Result<bool, IssueStoreError> {
    let cursor = marker
        .try_get::<Option<Uuid>, _>("validation_cursor")
        .map_err(row_decode)?;
    let rows = sqlx::query(
        "SELECT id, pii_key_ref FROM issue \
         WHERE tenant_id = $1 AND region = $2 AND title_subject = $3 AND NOT title_erased \
           AND ($4::uuid IS NULL OR id > $4) ORDER BY id LIMIT $5 FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(subject)
    .bind(cursor)
    .bind(TITLE_ERASURE_BATCH_MAX)
    .fetch_all(&mut *connection)
    .await
    .map_err(storage_error("inspect Issue-title encryption batch"))?;
    for row in &rows {
        let id: Uuid = row.try_get("id").map_err(row_decode)?;
        let encoded: String = row.try_get("pii_key_ref").map_err(row_decode)?;
        let key_ref = PiiKeyRef::parse(&encoded).ok_or_else(|| {
            IssueStoreError::Storage(format!(
                "Issue-title erasure cannot prove the encryption boundary of {id}"
            ))
        })?;
        if key_ref.tenant.as_str() != tenant || key_ref.class != issue_subject_key_class(subject) {
            return Err(IssueStoreError::Storage(format!(
                "Issue-title erasure refuses legacy or foreign key scope on {id}"
            )));
        }
    }
    let next_cursor = rows
        .last()
        .map(|row| row.try_get::<Uuid, _>("id"))
        .transpose()
        .map_err(row_decode)?
        .or(cursor);
    let validation_completed = rows.len() < TITLE_ERASURE_BATCH_MAX as usize;
    let advanced = sqlx::query(
        "UPDATE issue_title_erasure_operation \
            SET validation_cursor = $4, \
                validation_completed_at = CASE WHEN $5 THEN CURRENT_TIMESTAMP ELSE NULL END \
          WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 \
            AND completed_at IS NULL",
    )
    .bind(tenant)
    .bind(region)
    .bind(operation_id)
    .bind(next_cursor)
    .bind(validation_completed)
    .execute(&mut *connection)
    .await
    .map_err(storage_error("advance Issue-title erasure validation"))?;
    if advanced.rows_affected() != 1 {
        return Err(IssueStoreError::Storage(
            "Issue-title erasure validation marker did not advance exactly once".into(),
        ));
    }
    Ok(validation_completed)
}

fn title_erasure_event(
    tenant: &str,
    region: &str,
    title: &TombstonedTitle,
    event_ids: &dyn IdMinter,
    attempt: &IssueTitleErasureAttempt,
) -> myelin_events::EventEnvelope {
    let subject = ArtifactRef(format!("myelin://{tenant}/issue/issue/{}", title.key));
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_UPDATED.into()),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("issue:{}", title.id)),
            payload: serde_json::json!({
                "issue": subject.0,
                "issue_local_id": title.key,
                "type_id": title.type_id.to_string(),
                "changed_facets": ["title"],
                "change_kind": "title_erased",
                "version": title.version,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id: event_ids.mint().into(),
            tenant: TenantId(tenant.to_string()),
            region: Region(region.to_string()),
            actor: attempt.actor.clone(),
            schema_ver: 1,
            occurred_at: attempt.occurred_at.clone(),
            recorded_at: attempt.recorded_at.clone(),
            caused_by: None,
        },
        None,
    )
}

async fn load_marker(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    operation_id: &str,
) -> Result<sqlx::postgres::PgRow, IssueStoreError> {
    load_marker_optional(connection, tenant, region, operation_id)
        .await?
        .ok_or_else(|| {
            IssueStoreError::Storage(
                "Issue-title erasure was not durably prepared before mutation".into(),
            )
        })
}

async fn load_marker_optional(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    operation_id: &str,
) -> Result<Option<sqlx::postgres::PgRow>, IssueStoreError> {
    sqlx::query(
        "SELECT subject, completed_at IS NOT NULL AS completed, \
                validation_cursor, validation_completed_at IS NOT NULL AS validation_completed, \
                titles_erased, events_emitted \
           FROM issue_title_erasure_operation \
          WHERE tenant_id = $1 AND region = $2 AND operation_id = $3 FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(operation_id)
    .fetch_optional(connection)
    .await
    .map_err(storage_error("lock Issue-title erasure marker"))
}

fn require_same_subject(row: &sqlx::postgres::PgRow, subject: &str) -> Result<(), IssueStoreError> {
    if row.try_get::<String, _>("subject").map_err(row_decode)? != subject {
        return Err(IssueStoreError::Conflict(
            "Issue-title erasure operation is already bound to another subject".into(),
        ));
    }
    Ok(())
}

fn erasure_state_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AuthoredIssueTitleErasureState, IssueStoreError> {
    if !row.try_get::<bool, _>("completed").map_err(row_decode)? {
        return Ok(AuthoredIssueTitleErasureState::Pending);
    }
    let titles = row
        .try_get::<Option<i64>, _>("titles_erased")
        .map_err(row_decode)?
        .ok_or_else(|| IssueStoreError::Storage("completed erasure has no title count".into()))?;
    let events = row
        .try_get::<Option<i64>, _>("events_emitted")
        .map_err(row_decode)?
        .ok_or_else(|| IssueStoreError::Storage("completed erasure has no event count".into()))?;
    Ok(AuthoredIssueTitleErasureState::Completed(
        AuthoredIssueTitleEraseReceipt {
            titles_tombstoned: u64::try_from(titles).map_err(|_| {
                IssueStoreError::Storage("Issue-title erasure count is negative".into())
            })?,
            erasure_events_co_committed: u64::try_from(events).map_err(|_| {
                IssueStoreError::Storage("Issue-title erasure event count is negative".into())
            })?,
        },
    ))
}

#[derive(Clone, Copy)]
enum SubjectLock {
    Shared,
    Exclusive,
}

async fn lock_title_subject(
    connection: &mut sqlx::PgConnection,
    tenant: &str,
    subject: &str,
    mode: SubjectLock,
) -> Result<(), IssueStoreError> {
    let function = match mode {
        SubjectLock::Shared => "pg_advisory_xact_lock_shared",
        SubjectLock::Exclusive => "pg_advisory_xact_lock",
    };
    sqlx::query(&format!(
        "SELECT {function}(hashtextextended( \
         'myelin.issues.title-erasure.v1:' || length($1)::text || ':' || $1 || ':' || $2, 0))"
    ))
    .bind(tenant)
    .bind(subject)
    .execute(connection)
    .await
    .map_err(storage_error("lock Issue-title subject lifecycle"))?;
    Ok(())
}

fn validate_erasure_identity(
    tenant: &str,
    subject: &str,
    operation_id: &str,
) -> Result<(), IssueStoreError> {
    if tenant.is_empty() || subject.is_empty() || subject.len() > 255 {
        return Err(IssueStoreError::BadInput(
            "Issue-title erasure requires a bounded tenant and subject".into(),
        ));
    }
    validate_operation_id(operation_id)
}

fn validate_operation_id(operation_id: &str) -> Result<(), IssueStoreError> {
    if operation_id.is_empty()
        || operation_id.len() > 255
        || operation_id.trim() != operation_id
        || operation_id.chars().any(char::is_control)
    {
        return Err(IssueStoreError::BadInput(
            "Issue-title erasure requires a clean 1-255 byte operation identity".into(),
        ));
    }
    Ok(())
}

fn storage_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> IssueStoreError {
    move |error| IssueStoreError::Storage(format!("{context}: {error}"))
}

fn row_decode(error: sqlx::Error) -> IssueStoreError {
    IssueStoreError::Storage(format!("decode Issue-title erasure row: {error}"))
}
