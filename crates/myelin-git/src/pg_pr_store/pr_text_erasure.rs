use myelin_events::{Actor, EmitContextBase, Timestamp};
use myelin_storage::kms::PiiKeyRef;
use myelin_storage::with_tenant_tx_error;
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use super::{co_commit_event, pg_error, pg_query, PgPrStore};
use crate::core::RepoLoc;
use crate::dek::git_subject_key_class;
use crate::durable::DurableError;
use crate::events::GIT_PR_UPDATED;
use crate::pr_store::PrRecord;

pub const GIT_PR_TEXT_ERASURE_OPERATION_TABLE: &str = "git_pr_text_erasure_operation";
pub const ERASED_PR_TITLE: &str = "[erased pull request title]";
pub const ERASED_TITLE_KEY_REF: &str = "erased";
pub const ERASED_TITLE_NONCE: [u8; 12] = [0; 12];
pub const ERASED_TITLE_CIPHERTEXT: [u8; 1] = [0];
const PR_TEXT_ERASURE_BATCH_MAX: i64 = 100;

pub const EXPAND_GIT_PR_TEXT_ERASURE_DDL: &str = r#"
ALTER TABLE git_pr
  ADD COLUMN IF NOT EXISTS free_text_erased boolean NOT NULL DEFAULT false;

ALTER TABLE git_pr
  ADD CONSTRAINT git_pr_free_text_erasure_shape CHECK (
    NOT free_text_erased OR (
      title_nonce = decode(repeat('00', 12), 'hex') AND
      title_ciphertext = decode('00', 'hex') AND
      title_pii_key_ref = 'erased' AND
      body_nonce IS NULL AND body_ciphertext IS NULL AND body_pii_key_ref IS NULL
    )
  );
"#;

pub const CREATE_GIT_PR_TEXT_ERASURE_OPERATION_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS git_pr_text_erasure_operation (
  tenant_id                 text        NOT NULL,
  region                    text        NOT NULL,
  operation_id              text        NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 255),
  subject                   text        NOT NULL CHECK (length(subject) BETWEEN 1 AND 255),
  started_at                timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
  validation_repo_slug      text,
  validation_number         bigint,
  validation_completed_at   timestamptz,
  pull_requests_erased_so_far bigint     NOT NULL DEFAULT 0,
  events_emitted_so_far     bigint      NOT NULL DEFAULT 0,
  completed_at              timestamptz,
  pull_requests_erased      bigint,
  events_emitted            bigint,
  PRIMARY KEY (tenant_id, region, operation_id),
  CHECK ((validation_repo_slug IS NULL) = (validation_number IS NULL)),
  CHECK (
    pull_requests_erased_so_far >= 0 AND
    events_emitted_so_far = pull_requests_erased_so_far AND
    (validation_completed_at IS NOT NULL OR pull_requests_erased_so_far = 0) AND
    (
      completed_at IS NULL AND pull_requests_erased IS NULL AND events_emitted IS NULL
      OR completed_at IS NOT NULL
         AND validation_completed_at IS NOT NULL
         AND pull_requests_erased = pull_requests_erased_so_far
         AND events_emitted = events_emitted_so_far
    )
  )
);

CREATE UNIQUE INDEX IF NOT EXISTS git_pr_text_erasure_in_progress
  ON git_pr_text_erasure_operation (tenant_id, region, subject)
  WHERE completed_at IS NULL;

CREATE OR REPLACE FUNCTION myelin_guard_git_pr_text_erasure_progress()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id OR
     NEW.region IS DISTINCT FROM OLD.region OR
     NEW.operation_id IS DISTINCT FROM OLD.operation_id OR
     NEW.subject IS DISTINCT FROM OLD.subject OR
     NEW.started_at IS DISTINCT FROM OLD.started_at OR
     OLD.completed_at IS NOT NULL OR
     NEW.pull_requests_erased_so_far < OLD.pull_requests_erased_so_far OR
     NEW.events_emitted_so_far < OLD.events_emitted_so_far OR
     NEW.pull_requests_erased_so_far <> NEW.events_emitted_so_far OR
     (OLD.validation_repo_slug IS NOT NULL AND
       (NEW.validation_repo_slug, NEW.validation_number) IS DISTINCT FROM
         (OLD.validation_repo_slug, OLD.validation_number) AND
       (NEW.validation_repo_slug IS NULL OR
        (NEW.validation_repo_slug, NEW.validation_number) <=
          (OLD.validation_repo_slug, OLD.validation_number))) OR
     (OLD.validation_completed_at IS NOT NULL AND
       (NEW.validation_completed_at IS DISTINCT FROM OLD.validation_completed_at OR
        NEW.validation_repo_slug IS DISTINCT FROM OLD.validation_repo_slug OR
        NEW.validation_number IS DISTINCT FROM OLD.validation_number)) OR
     (NEW.validation_completed_at IS NULL AND NEW.pull_requests_erased_so_far <> 0) OR
     (NEW.completed_at IS NULL AND
       (NEW.pull_requests_erased IS NOT NULL OR NEW.events_emitted IS NOT NULL)) OR
     (NEW.completed_at IS NOT NULL AND
       (NEW.validation_completed_at IS NULL OR
        NEW.pull_requests_erased IS DISTINCT FROM NEW.pull_requests_erased_so_far OR
        NEW.events_emitted IS DISTINCT FROM NEW.events_emitted_so_far)) THEN
    RAISE EXCEPTION 'Git PR text erasure permits only forward validation, progress, and completion';
  END IF;
  RETURN NEW;
END
$myelin$;

CREATE OR REPLACE TRIGGER git_pr_text_erasure_guard_update
BEFORE UPDATE ON git_pr_text_erasure_operation
FOR EACH ROW EXECUTE FUNCTION myelin_guard_git_pr_text_erasure_progress();

SELECT myelin_make_tenant_scoped('git_pr_text_erasure_operation');
"#;

pub const CREATE_GIT_PR_TEXT_ERASURE_BATCH_INDEX_DDL: &str =
    "CREATE INDEX CONCURRENTLY IF NOT EXISTS git_pr_text_erasure_batch \
     ON git_pr (tenant_id, region, author_subject_id, repo_slug, number) \
     WHERE NOT free_text_erased";

#[derive(Clone, Debug)]
pub struct PrTextErasureAttempt {
    operation_id: String,
    actor: Actor,
    occurred_at: Timestamp,
    recorded_at: Timestamp,
    unix_seconds: i64,
}

impl PrTextErasureAttempt {
    pub fn new(
        operation_id: impl Into<String>,
        actor: Actor,
        observed: myelin_events::clock::ClockReading,
    ) -> Result<Self, DurableError> {
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

    pub fn completed_at_offset(&self) -> Result<u64, DurableError> {
        u64::try_from(self.unix_seconds)
            .map_err(|_| DurableError::Io("Git PR text erasure clock predates Unix time".into()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredPrTextEraseReceipt {
    pub pull_requests_tombstoned: u64,
    pub erasure_events_co_committed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoredPrTextErasureState {
    Pending,
    Completed(AuthoredPrTextEraseReceipt),
}

pub(crate) struct VerifiedPrTextErasureAttempt {
    subject: String,
    attempt: PrTextErasureAttempt,
}

impl VerifiedPrTextErasureAttempt {
    pub(crate) fn after_key_destruction(
        subject: impl Into<String>,
        attempt: PrTextErasureAttempt,
    ) -> Self {
        Self {
            subject: subject.into(),
            attempt,
        }
    }
}

#[derive(Clone, Copy)]
enum SubjectLock {
    Shared,
    Exclusive,
}

impl PgPrStore {
    pub fn prepare_pr_text_erasure(
        &self,
        tenant: &str,
        subject: &str,
        operation_id: &str,
    ) -> Result<AuthoredPrTextErasureState, DurableError> {
        validate_erasure_identity(tenant, subject, operation_id)?;
        let provider = self.provider.clone();
        let region = self.provider.config().region.clone();
        let tenant = tenant.to_owned();
        let subject = subject.to_owned();
        let operation_id = operation_id.to_owned();
        self.block_on(async move {
            with_tenant_tx_error(
                provider.db_pool(),
                &tenant.clone(),
                &region.clone(),
                move |conn| {
                    Box::pin(async move {
                        lock_pr_text_subject(conn, &tenant, &subject, SubjectLock::Exclusive)
                            .await?;
                        if let Some(row) =
                            load_marker_optional(conn, &tenant, &region, &operation_id).await?
                        {
                            require_same_subject(&row, &subject)?;
                            return erasure_state_from_row(&row);
                        }
                        let pending: Option<String> = sqlx::query_scalar(
                            "SELECT operation_id FROM git_pr_text_erasure_operation \
                         WHERE tenant_id=$1 AND region=$2 AND subject=$3 AND completed_at IS NULL",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&subject)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| pg_query("inspect pending PR text erasure"))?;
                        if pending.is_some() {
                            return Err(pg_query("another PR text erasure is already in progress"));
                        }
                        sqlx::query(
                            "INSERT INTO git_pr_text_erasure_operation \
                         (tenant_id,region,operation_id,subject) VALUES ($1,$2,$3,$4)",
                        )
                        .bind(&tenant)
                        .bind(&region)
                        .bind(&operation_id)
                        .bind(&subject)
                        .execute(&mut *conn)
                        .await
                        .map_err(|_| pg_query("prepare PR text erasure"))?;
                        Ok(AuthoredPrTextErasureState::Pending)
                    })
                },
            )
            .await
        })
        .map_err(pg_error)
    }

    pub fn verify_pr_text_erasure_ready(
        &self,
        tenant: &str,
        subject: &str,
        operation_id: &str,
    ) -> Result<(), DurableError> {
        validate_erasure_identity(tenant, subject, operation_id)?;
        loop {
            let provider = self.provider.clone();
            let region = self.provider.config().region.clone();
            let tenant = tenant.to_owned();
            let subject = subject.to_owned();
            let operation_id = operation_id.to_owned();
            let complete = self
                .block_on(async move {
                    with_tenant_tx_error(
                        provider.db_pool(),
                        &tenant.clone(),
                        &region.clone(),
                        move |conn| {
                            Box::pin(async move {
                                lock_pr_text_subject(
                                    conn,
                                    &tenant,
                                    &subject,
                                    SubjectLock::Exclusive,
                                )
                                .await?;
                                let marker =
                                    load_marker(conn, &tenant, &region, &operation_id).await?;
                                require_same_subject(&marker, &subject)?;
                                if matches!(
                                    erasure_state_from_row(&marker)?,
                                    AuthoredPrTextErasureState::Completed(_)
                                ) || marker
                                    .try_get::<bool, _>("validation_completed")
                                    .map_err(|_| pg_query("decode PR text erasure marker"))?
                                {
                                    return Ok(true);
                                }
                                validate_next_batch(
                                    conn,
                                    &tenant,
                                    &region,
                                    &subject,
                                    &operation_id,
                                    &marker,
                                )
                                .await
                            })
                        },
                    )
                    .await
                })
                .map_err(pg_error)?;
            if complete {
                return Ok(());
            }
        }
    }

    pub(crate) fn tombstone_pr_text_co_commit(
        &self,
        tenant: &str,
        verified: VerifiedPrTextErasureAttempt,
    ) -> Result<AuthoredPrTextEraseReceipt, DurableError> {
        let VerifiedPrTextErasureAttempt { subject, attempt } = verified;
        validate_erasure_identity(tenant, &subject, &attempt.operation_id)?;
        if attempt.actor.0.tenant.as_str() != tenant
            || attempt.actor.0.region.as_str() != self.provider.config().region
        {
            return Err(DurableError::Git(
                "PR text erasure actor must belong to the affected tenant and region".into(),
            ));
        }
        loop {
            if let Some(receipt) = self.tombstone_next_batch(tenant, &subject, &attempt)? {
                return Ok(receipt);
            }
        }
    }

    fn tombstone_next_batch(
        &self,
        tenant: &str,
        subject: &str,
        attempt: &PrTextErasureAttempt,
    ) -> Result<Option<AuthoredPrTextEraseReceipt>, DurableError> {
        let provider = self.provider.clone();
        let region = self.provider.config().region.clone();
        let tenant = tenant.to_owned();
        let subject = subject.to_owned();
        let attempt = attempt.clone();
        let minter = self.minter.clone();
        self.block_on(async move {
            with_tenant_tx_error(provider.db_pool(), &tenant.clone(), &region.clone(), move |conn| {
                Box::pin(async move {
                    lock_pr_text_subject(conn, &tenant, &subject, SubjectLock::Exclusive).await?;
                    let marker = load_marker(conn, &tenant, &region, &attempt.operation_id).await?;
                    require_same_subject(&marker, &subject)?;
                    if let AuthoredPrTextErasureState::Completed(receipt) = erasure_state_from_row(&marker)? {
                        return Ok(Some(receipt));
                    }
                    if !marker.try_get::<bool, _>("validation_completed").map_err(|_| pg_query("decode PR text erasure marker"))? {
                        return Err(pg_query("PR text cannot be erased before key-scope validation"));
                    }

                    let rows = sqlx::query(
                        "WITH batch AS ( \
                           SELECT tenant_id,region,repo_slug,number FROM git_pr \
                           WHERE tenant_id=$1 AND region=$2 AND author_subject_id=$3 \
                             AND NOT free_text_erased \
                           ORDER BY repo_slug,number LIMIT $4 FOR UPDATE \
                         ) \
                         UPDATE git_pr AS pr SET \
                           free_text_erased=true, title_nonce=$5, title_ciphertext=$6, \
                           title_pii_key_ref=$7, body_nonce=NULL, body_ciphertext=NULL, \
                           body_pii_key_ref=NULL, \
                           record=jsonb_set(pr.record,'{updated_at}',to_jsonb($8::bigint),false), \
                           version=pr.version+1, updated_at=to_timestamp($8) \
                         FROM batch WHERE pr.tenant_id=batch.tenant_id AND pr.region=batch.region \
                           AND pr.repo_slug=batch.repo_slug AND pr.number=batch.number \
                         RETURNING pr.repo_slug,pr.record",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(&subject)
                    .bind(PR_TEXT_ERASURE_BATCH_MAX)
                    .bind(ERASED_TITLE_NONCE.to_vec())
                    .bind(ERASED_TITLE_CIPHERTEXT.to_vec())
                    .bind(ERASED_TITLE_KEY_REF)
                    .bind(attempt.unix_seconds)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|_| pg_query("tombstone PR text batch"))?;

                    for row in &rows {
                        let repo: String = row.try_get("repo_slug").map_err(|_| pg_query("decode erased PR repository"))?;
                        let value: serde_json::Value = row.try_get("record").map_err(|_| pg_query("decode erased PR projection"))?;
                        let mut record: PrRecord = serde_json::from_value(value).map_err(|_| pg_query("decode erased PR projection"))?;
                        record.title = ERASED_PR_TITLE.into();
                        record.body_md = None;
                        co_commit_event(
                            conn,
                            minter.clone(),
                            EmitContextBase {
                                tenant: TenantId(tenant.clone()),
                                region: Region(region.clone()),
                                actor: attempt.actor.clone(),
                                schema_ver: 1,
                                occurred_at: attempt.occurred_at.clone(),
                                recorded_at: attempt.recorded_at.clone(),
                                caused_by: None,
                            },
                            &RepoLoc::new(&tenant, &region, repo),
                            &record,
                            GIT_PR_UPDATED,
                            None,
                        )
                        .await?;
                    }

                    let batch_count = i64::try_from(rows.len()).map_err(|_| pg_query("count erased PR text batch"))?;
                    let progress: i64 = sqlx::query_scalar(
                        "UPDATE git_pr_text_erasure_operation SET \
                           pull_requests_erased_so_far=pull_requests_erased_so_far+$4, \
                           events_emitted_so_far=events_emitted_so_far+$4 \
                         WHERE tenant_id=$1 AND region=$2 AND operation_id=$3 AND completed_at IS NULL \
                         RETURNING pull_requests_erased_so_far",
                    )
                    .bind(&tenant).bind(&region).bind(&attempt.operation_id).bind(batch_count)
                    .fetch_one(&mut *conn).await.map_err(|_| pg_query("advance PR text erasure progress"))?;
                    let more: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM git_pr WHERE tenant_id=$1 AND region=$2 \
                         AND author_subject_id=$3 AND NOT free_text_erased)",
                    )
                    .bind(&tenant).bind(&region).bind(&subject)
                    .fetch_one(&mut *conn).await.map_err(|_| pg_query("check remaining PR text"))?;
                    if more {
                        return Ok(None);
                    }
                    let completed = sqlx::query(
                        "UPDATE git_pr_text_erasure_operation SET completed_at=to_timestamp($4), \
                           pull_requests_erased=pull_requests_erased_so_far, \
                           events_emitted=events_emitted_so_far \
                         WHERE tenant_id=$1 AND region=$2 AND operation_id=$3 AND completed_at IS NULL",
                    )
                    .bind(&tenant).bind(&region).bind(&attempt.operation_id).bind(attempt.unix_seconds)
                    .execute(&mut *conn).await.map_err(|_| pg_query("complete PR text erasure"))?;
                    if completed.rows_affected() != 1 {
                        return Err(pg_query("PR text erasure completion was not unique"));
                    }
                    Ok(Some(AuthoredPrTextEraseReceipt {
                        pull_requests_tombstoned: u64::try_from(progress).map_err(|_| pg_query("decode PR text erasure count"))?,
                        erasure_events_co_committed: u64::try_from(progress).map_err(|_| pg_query("decode PR text erasure event count"))?,
                    }))
                })
            })
            .await
        })
        .map_err(pg_error)
    }
}

pub(super) async fn lock_pr_text_subject_write(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    subject: &str,
) -> Result<(), myelin_storage::PgError> {
    lock_pr_text_subject(conn, tenant, subject, SubjectLock::Shared).await?;
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM git_pr_text_erasure_operation \
         WHERE tenant_id=$1 AND region=$2 AND subject=$3 AND completed_at IS NULL)",
    )
    .bind(tenant)
    .bind(region)
    .bind(subject)
    .fetch_one(conn)
    .await
    .map_err(|_| pg_query("inspect PR text subject lifecycle"))?;
    if pending {
        return Err(pg_query(
            "PR text cannot be written while subject erasure is in progress",
        ));
    }
    Ok(())
}

async fn lock_pr_text_subject(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    subject: &str,
    mode: SubjectLock,
) -> Result<(), myelin_storage::PgError> {
    let function = match mode {
        SubjectLock::Shared => "pg_advisory_xact_lock_shared",
        SubjectLock::Exclusive => "pg_advisory_xact_lock",
    };
    sqlx::query(&format!(
        "SELECT {function}(hashtextextended( \
         'myelin.git.pr-text-erasure.v1:' || length($1)::text || ':' || $1 || ':' || $2, 0))"
    ))
    .bind(tenant)
    .bind(subject)
    .execute(conn)
    .await
    .map_err(|_| pg_query("lock PR text subject lifecycle"))?;
    Ok(())
}

async fn validate_next_batch(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    subject: &str,
    operation_id: &str,
    marker: &sqlx::postgres::PgRow,
) -> Result<bool, myelin_storage::PgError> {
    let cursor_repo: Option<String> = marker
        .try_get("validation_repo_slug")
        .map_err(|_| pg_query("decode PR text validation cursor"))?;
    let cursor_number: Option<i64> = marker
        .try_get("validation_number")
        .map_err(|_| pg_query("decode PR text validation cursor"))?;
    let rows = sqlx::query(
        "SELECT repo_slug,number,title_pii_key_ref,body_pii_key_ref FROM git_pr \
         WHERE tenant_id=$1 AND region=$2 AND author_subject_id=$3 AND NOT free_text_erased \
           AND ($4::text IS NULL OR (repo_slug,number) > ($4,$5)) \
         ORDER BY repo_slug,number LIMIT $6 FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(subject)
    .bind(&cursor_repo)
    .bind(cursor_number)
    .bind(PR_TEXT_ERASURE_BATCH_MAX)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| pg_query("inspect PR text encryption batch"))?;
    let expected = git_subject_key_class(subject);
    for row in &rows {
        for column in ["title_pii_key_ref", "body_pii_key_ref"] {
            let encoded: Option<String> = row
                .try_get(column)
                .map_err(|_| pg_query("decode PR text key reference"))?;
            if let Some(encoded) = encoded {
                let key_ref = PiiKeyRef::parse(&encoded).ok_or_else(|| {
                    pg_query("PR text erasure cannot prove an encryption boundary")
                })?;
                if key_ref.tenant.as_str() != tenant || key_ref.class != expected {
                    return Err(pg_query(
                        "PR text erasure refuses legacy or foreign key scope",
                    ));
                }
            } else if column == "title_pii_key_ref" {
                return Err(pg_query(
                    "PR text erasure found a missing title key reference",
                ));
            }
        }
    }
    let next_repo = rows
        .last()
        .map(|row| row.try_get::<String, _>("repo_slug"))
        .transpose()
        .map_err(|_| pg_query("decode PR text validation cursor"))?
        .or(cursor_repo);
    let next_number = rows
        .last()
        .map(|row| row.try_get::<i64, _>("number"))
        .transpose()
        .map_err(|_| pg_query("decode PR text validation cursor"))?
        .or(cursor_number);
    let complete = rows.len() < PR_TEXT_ERASURE_BATCH_MAX as usize;
    let advanced = sqlx::query(
        "UPDATE git_pr_text_erasure_operation SET validation_repo_slug=$4,validation_number=$5, \
         validation_completed_at=CASE WHEN $6 THEN CURRENT_TIMESTAMP ELSE NULL END \
         WHERE tenant_id=$1 AND region=$2 AND operation_id=$3 AND completed_at IS NULL",
    )
    .bind(tenant)
    .bind(region)
    .bind(operation_id)
    .bind(next_repo)
    .bind(next_number)
    .bind(complete)
    .execute(conn)
    .await
    .map_err(|_| pg_query("advance PR text erasure validation"))?;
    if advanced.rows_affected() != 1 {
        return Err(pg_query(
            "PR text erasure validation did not advance exactly once",
        ));
    }
    Ok(complete)
}

async fn load_marker(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    operation_id: &str,
) -> Result<sqlx::postgres::PgRow, myelin_storage::PgError> {
    load_marker_optional(conn, tenant, region, operation_id)
        .await?
        .ok_or_else(|| pg_query("PR text erasure was not durably prepared"))
}

async fn load_marker_optional(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    region: &str,
    operation_id: &str,
) -> Result<Option<sqlx::postgres::PgRow>, myelin_storage::PgError> {
    sqlx::query(
        "SELECT subject,completed_at IS NOT NULL AS completed,validation_repo_slug,validation_number, \
         validation_completed_at IS NOT NULL AS validation_completed,pull_requests_erased,events_emitted \
         FROM git_pr_text_erasure_operation WHERE tenant_id=$1 AND region=$2 AND operation_id=$3 FOR UPDATE",
    )
    .bind(tenant).bind(region).bind(operation_id).fetch_optional(conn).await
    .map_err(|_| pg_query("lock PR text erasure marker"))
}

fn require_same_subject(
    row: &sqlx::postgres::PgRow,
    subject: &str,
) -> Result<(), myelin_storage::PgError> {
    if row
        .try_get::<String, _>("subject")
        .map_err(|_| pg_query("decode PR text erasure marker"))?
        != subject
    {
        return Err(pg_query(
            "PR text erasure operation is bound to another subject",
        ));
    }
    Ok(())
}

fn erasure_state_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<AuthoredPrTextErasureState, myelin_storage::PgError> {
    if !row
        .try_get::<bool, _>("completed")
        .map_err(|_| pg_query("decode PR text erasure marker"))?
    {
        return Ok(AuthoredPrTextErasureState::Pending);
    }
    let erased: i64 = row
        .try_get::<Option<i64>, _>("pull_requests_erased")
        .map_err(|_| pg_query("decode PR text erasure marker"))?
        .ok_or_else(|| pg_query("completed PR text erasure has no count"))?;
    let events: i64 = row
        .try_get::<Option<i64>, _>("events_emitted")
        .map_err(|_| pg_query("decode PR text erasure marker"))?
        .ok_or_else(|| pg_query("completed PR text erasure has no event count"))?;
    Ok(AuthoredPrTextErasureState::Completed(
        AuthoredPrTextEraseReceipt {
            pull_requests_tombstoned: u64::try_from(erased)
                .map_err(|_| pg_query("PR text erasure count is negative"))?,
            erasure_events_co_committed: u64::try_from(events)
                .map_err(|_| pg_query("PR text erasure event count is negative"))?,
        },
    ))
}

fn validate_erasure_identity(
    tenant: &str,
    subject: &str,
    operation_id: &str,
) -> Result<(), DurableError> {
    if tenant.is_empty() || subject.is_empty() || subject.len() > 255 {
        return Err(DurableError::Git(
            "PR text erasure requires a bounded tenant and subject".into(),
        ));
    }
    validate_operation_id(operation_id)
}

fn validate_operation_id(operation_id: &str) -> Result<(), DurableError> {
    if operation_id.is_empty()
        || operation_id.len() > 255
        || operation_id.trim() != operation_id
        || operation_id.chars().any(char::is_control)
    {
        return Err(DurableError::Git(
            "PR text erasure requires a clean operation identity".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_declares_a_bounded_forward_only_erasure_shape() {
        assert!(EXPAND_GIT_PR_TEXT_ERASURE_DDL.contains("free_text_erased"));
        assert!(CREATE_GIT_PR_TEXT_ERASURE_OPERATION_DDL.contains("OLD.completed_at IS NOT NULL"));
        assert!(CREATE_GIT_PR_TEXT_ERASURE_BATCH_INDEX_DDL.contains("WHERE NOT free_text_erased"));
        assert_eq!(ERASED_TITLE_NONCE.len(), 12);
        assert_eq!(ERASED_TITLE_CIPHERTEXT.len(), 1);
    }

    #[test]
    fn erasure_operation_id_is_bounded_and_clean() {
        assert!(validate_operation_id("privacy-request-42").is_ok());
        assert!(validate_operation_id("").is_err());
        assert!(validate_operation_id(" dirty ").is_err());
        assert!(validate_operation_id(&"x".repeat(256)).is_err());
    }
}
