use crate::ci_job_result::CiJobResultSummary;
use crate::ci_run_store::{CiRunStore, CiRunStoreError};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use myelin_storage::{with_tenant_repeatable_read_tx, with_tenant_tx, PgError};
use serde_json::Value;
use sqlx::types::Uuid;
use sqlx::Row;

pub const CI_RUN_PAGE_DEFAULT: u32 = 50;
pub const CI_RUN_PAGE_MAX: u32 = 100;
pub const CI_RUN_VISIBLE_REPO_MAX: usize = 4_096;
pub const CI_RUN_SUMMARY_BATCH_MAX: usize = 10_000;
pub const CI_RUN_CURSOR_PREFIX: &str = "cr1_";
pub const CI_LOG_RANGE_DEFAULT: u32 = 64 * 1024;
pub const CI_LOG_RANGE_MAX: u32 = 256 * 1024;
pub const CI_LOG_SEGMENT_REF_MAX: usize = 256;
pub const CI_LOG_TAIL_BATCH_MAX: usize = 64;

const CURSOR_TIMESTAMP_BYTES: usize = 27;
const CURSOR_UUID_BYTES: usize = 16;
const CURSOR_SCOPE_BYTES: usize = 16;
const CURSOR_FRAME_BYTES: usize =
    1 + CURSOR_TIMESTAMP_BYTES + CURSOR_UUID_BYTES + CURSOR_SCOPE_BYTES;

pub const LIST_CI_RUNS_QUERY: &str = "\
SELECT
  candidate.run_id,
  candidate.pipeline_id,
  candidate.repo_ref,
  candidate.source_ref,
  candidate.commit_oid,
  candidate.trigger_kind,
  candidate.trust_tier,
  candidate.state,
  candidate.cost_settled,
  candidate.created_at,
  candidate.finished_at
FROM unnest($3::text[]) AS visible(repo_ref)
CROSS JOIN LATERAL (
  SELECT
    run_id::text AS run_id,
    run_id AS sort_run_id,
    pipeline_id::text AS pipeline_id,
    repo_ref,
    source_ref,
    commit_oid,
    trigger_kind,
    trust_tier,
    state,
    cost_settled,
    created_at AS sort_created_at,
    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,
    CASE WHEN finished_at IS NULL THEN NULL
         ELSE to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
    END AS finished_at
  FROM ci_run
  WHERE tenant_id = $1
    AND region = $2
    AND repo_ref = visible.repo_ref
    AND ($4::text IS NULL OR state = $4)
    AND (
      $5::timestamptz IS NULL
      OR created_at < $5::timestamptz
      OR (created_at = $5::timestamptz AND run_id < $6::uuid)
    )
  ORDER BY created_at DESC, run_id DESC
  LIMIT $7
) AS candidate
ORDER BY candidate.sort_created_at DESC, candidate.sort_run_id DESC
LIMIT $7";

pub const SELECT_CI_SURFACE_JOBS_QUERY: &str = "\
SELECT
  job_id::text AS job_id,
  stage,
  name,
  ARRAY(SELECT need::text FROM unnest(needs) AS need) AS needs,
  matrix_key,
  state,
  attempt,
  result_summary
FROM ci_job
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
ORDER BY stage ASC, name ASC, job_id ASC";

pub const SELECT_CI_SURFACE_RUN_QUERY: &str = "\
SELECT
  run_id::text AS run_id,
  pipeline_id::text AS pipeline_id,
  repo_ref,
  source_ref,
  commit_oid,
  trigger_kind,
  trust_tier,
  state,
  cost_settled,
  to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,
  CASE WHEN finished_at IS NULL THEN NULL
       ELSE to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
  END AS finished_at
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid AND repo_ref = $4";

pub const SELECT_CI_RUN_SUMMARIES_QUERY: &str = "\
SELECT
  run_id::text AS run_id,
  pipeline_id::text AS pipeline_id,
  repo_ref,
  source_ref,
  commit_oid,
  trigger_kind,
  trust_tier,
  state,
  cost_settled,
  to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS created_at,
  CASE WHEN finished_at IS NULL THEN NULL
       ELSE to_char(finished_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')
  END AS finished_at
FROM ci_run
WHERE tenant_id = $1 AND region = $2 AND run_id = ANY($3::uuid[])
ORDER BY run_id ASC";

pub const SELECT_CI_SURFACE_STEPS_QUERY: &str = "\
SELECT
  job_id::text AS job_id,
  step_id,
  byte_start,
  byte_end,
  status
FROM log_anchor
WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid
ORDER BY job_id ASC, byte_start ASC, step_id ASC";

pub const SELECT_CI_LOG_ARCHIVE_HEAD_QUERY: &str = "\
SELECT COALESCE(MAX(segment.byte_end), 0) AS total_end
FROM ci_run run
JOIN ci_job job
  ON job.tenant_id = run.tenant_id
 AND job.region = run.region
 AND job.run_id = run.run_id
LEFT JOIN log_segment segment
  ON segment.tenant_id = job.tenant_id
 AND segment.region = job.region
 AND segment.run_id = job.run_id
 AND segment.job_id = job.job_id
 AND segment.blob_ref IS NOT NULL
WHERE run.tenant_id = $1
  AND run.region = $2
  AND run.run_id = $3::uuid
  AND run.repo_ref = $4
  AND job.job_id = $5::uuid
GROUP BY run.run_id, job.job_id";

pub const SELECT_CI_LOG_ARCHIVE_SEGMENTS_QUERY: &str = "\
SELECT segment.blob_ref, segment.byte_start, segment.byte_end
FROM ci_run run
JOIN ci_job job
  ON job.tenant_id = run.tenant_id
 AND job.region = run.region
 AND job.run_id = run.run_id
JOIN log_segment segment
  ON segment.tenant_id = job.tenant_id
 AND segment.region = job.region
 AND segment.run_id = job.run_id
 AND segment.job_id = job.job_id
WHERE run.tenant_id = $1
  AND run.region = $2
  AND run.run_id = $3::uuid
  AND run.repo_ref = $4
  AND job.job_id = $5::uuid
  AND segment.blob_ref IS NOT NULL
  AND segment.byte_end > $6
  AND segment.byte_start < $7
ORDER BY segment.byte_start ASC, segment.segment_seq ASC
LIMIT $8";

pub const SELECT_CI_LOG_TAIL_HEAD_QUERY: &str = "\
SELECT
  job.state,
  floor.segment_seq AS floor_sequence,
  head.segment_seq AS head_sequence,
  COALESCE(head.byte_end, 0) AS total_end
FROM ci_run run
JOIN ci_job job
  ON job.tenant_id = run.tenant_id
 AND job.region = run.region
 AND job.run_id = run.run_id
LEFT JOIN LATERAL (
  SELECT segment.segment_seq
  FROM log_segment segment
  WHERE segment.tenant_id = job.tenant_id
    AND segment.region = job.region
    AND segment.run_id = job.run_id
    AND segment.job_id = job.job_id
    AND segment.blob_ref IS NOT NULL
  ORDER BY segment.segment_seq ASC
  LIMIT 1
) floor ON TRUE
LEFT JOIN LATERAL (
  SELECT segment.segment_seq, segment.byte_end
  FROM log_segment segment
  WHERE segment.tenant_id = job.tenant_id
    AND segment.region = job.region
    AND segment.run_id = job.run_id
    AND segment.job_id = job.job_id
    AND segment.blob_ref IS NOT NULL
  ORDER BY segment.segment_seq DESC
  LIMIT 1
) head ON TRUE
WHERE run.tenant_id = $1
  AND run.region = $2
  AND run.run_id = $3::uuid
  AND run.repo_ref = $4
  AND job.job_id = $5::uuid";

pub const SELECT_CI_LOG_TAIL_SEGMENTS_QUERY: &str = "\
SELECT segment.segment_seq, segment.byte_start, segment.byte_end
FROM log_segment segment
WHERE segment.tenant_id = $1
  AND segment.region = $2
  AND segment.run_id = $3::uuid
  AND segment.job_id = $4::uuid
  AND segment.blob_ref IS NOT NULL
  AND segment.segment_seq >= $5
ORDER BY segment.segment_seq ASC
LIMIT $6";

pub const SELECT_CI_LOG_TAIL_PREDECESSOR_QUERY: &str = "\
SELECT segment.byte_end
FROM log_segment segment
WHERE segment.tenant_id = $1
  AND segment.region = $2
  AND segment.run_id = $3::uuid
  AND segment.job_id = $4::uuid
  AND segment.blob_ref IS NOT NULL
  AND segment.segment_seq = $5
LIMIT 1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunStateFilter {
    All,
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Reaped,
}

impl CiRunStateFilter {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            "timed_out" => Some(Self::TimedOut),
            "reaped" => Some(Self::Reaped),
            _ => None,
        }
    }

    pub const fn token(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Reaped => "reaped",
        }
    }

    fn query_token(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            other => Some(other.token()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiRunPageRequest {
    pub state: CiRunStateFilter,
    pub limit: u32,
    pub cursor: Option<String>,
}

impl CiRunPageRequest {
    pub fn new(
        state: CiRunStateFilter,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<Self, CiRunSurfaceError> {
        if !(1..=CI_RUN_PAGE_MAX).contains(&limit) {
            return Err(CiRunSurfaceError::BadInput(format!(
                "CI run page limit must be between 1 and {CI_RUN_PAGE_MAX}"
            )));
        }
        Ok(Self {
            state,
            limit,
            cursor,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CiRunSummary {
    pub run_id: String,
    pub pipeline_id: String,
    pub repo_ref: String,
    pub source_ref: Option<String>,
    pub commit_oid: Option<String>,
    pub trigger_kind: String,
    pub trust_tier: String,
    pub state: String,
    pub cost_settled: bool,
    pub created_at: String,
    pub finished_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CiJobSurface {
    pub job_id: String,
    pub stage: String,
    pub name: String,
    pub needs: Vec<String>,
    pub matrix_key: Option<Value>,
    pub state: String,
    pub attempt: i32,
    pub result_summary: Option<CiJobResultSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiStepSurface {
    pub job_id: String,
    pub step_id: String,
    pub byte_start: i64,
    pub byte_end: Option<i64>,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CiRunSurface {
    pub run: CiRunSummary,
    pub jobs: Vec<CiJobSurface>,
    pub steps: Vec<CiStepSurface>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiLogRangeRequest {
    pub start: i64,
    pub limit: u32,
}

impl CiLogRangeRequest {
    pub fn new(start: i64, limit: u32) -> Result<Self, CiRunSurfaceError> {
        if start < 0 {
            return Err(CiRunSurfaceError::BadInput(
                "CI log start must be non-negative".into(),
            ));
        }
        if !(1..=CI_LOG_RANGE_MAX).contains(&limit) {
            return Err(CiRunSurfaceError::BadInput(format!(
                "CI log range limit must be between 1 and {CI_LOG_RANGE_MAX}"
            )));
        }
        start
            .checked_add(i64::from(limit))
            .ok_or_else(|| CiRunSurfaceError::BadInput("CI log range overflows".into()))?;
        Ok(Self { start, limit })
    }

    pub fn end(self) -> i64 {
        self.start + i64::from(self.limit)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogSegmentRef {
    pub blob_ref: String,
    pub byte_start: i64,
    pub byte_end: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogArchive {
    pub total_end: i64,
    pub segments: Vec<CiLogSegmentRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogTailSegmentRef {
    pub cursor: u64,
    pub byte_start: i64,
    pub byte_end: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogTailBatch {
    pub resume_from: u64,
    pub segments: Vec<CiLogTailSegmentRef>,
    pub total_end: i64,
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CiRunPage {
    pub items: Vec<CiRunSummary>,
    pub next_cursor: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiRunSurfaceError {
    BadInput(String),
    CursorStale,
    Storage(String),
}

impl std::fmt::Display for CiRunSurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadInput(reason) => write!(f, "{reason}"),
            Self::CursorStale => write!(f, "CI run cursor scope is stale"),
            Self::Storage(reason) => write!(f, "CI run surface storage error: {reason}"),
        }
    }
}

impl std::error::Error for CiRunSurfaceError {}

impl From<PgError> for CiRunSurfaceError {
    fn from(value: PgError) -> Self {
        Self::Storage(value.to_string())
    }
}

impl From<CiRunStoreError> for CiRunSurfaceError {
    fn from(value: CiRunStoreError) -> Self {
        Self::Storage(value.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CiRunCursor {
    created_at: String,
    run_id: String,
}

impl CiRunStore {
    pub async fn list_surface_runs(
        &self,
        tenant_id: &str,
        region: &str,
        visible_repo_refs: &[String],
        request: CiRunPageRequest,
    ) -> Result<CiRunPage, CiRunSurfaceError> {
        let cursor_key = self.surface_cursor_key().ok_or_else(|| {
            CiRunSurfaceError::Storage("CI run cursor authority is not configured".into())
        })?;
        let visible = canonical_visible_repo_refs(visible_repo_refs)?;
        let scope = cursor_scope(tenant_id, region, request.state, &visible);
        let cursor = request
            .cursor
            .as_deref()
            .map(|value| decode_cursor(value, scope, cursor_key))
            .transpose()?;
        if visible.is_empty() {
            return Ok(CiRunPage {
                items: Vec::new(),
                next_cursor: None,
                limit: request.limit,
            });
        }
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let state = request.state.query_token().map(str::to_string);
        let cursor_created = cursor.as_ref().map(|value| value.created_at.clone());
        let cursor_run = cursor.as_ref().map(|value| value.run_id.clone());
        let fetch_limit = i64::from(request.limit) + 1;
        let rows = with_tenant_tx(self.pool(), tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(LIST_CI_RUNS_QUERY)
                    .bind(tenant_id_owned)
                    .bind(region_owned)
                    .bind(visible)
                    .bind(state)
                    .bind(cursor_created)
                    .bind(cursor_run)
                    .bind(fetch_limit)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))
            })
        })
        .await?;
        let mut items = rows.into_iter().map(run_from_row).collect::<Vec<_>>();
        let has_more = items.len() > request.limit as usize;
        items.truncate(request.limit as usize);
        let next_cursor = if has_more {
            items
                .last()
                .map(|run| {
                    encode_cursor(
                        &CiRunCursor {
                            created_at: run.created_at.clone(),
                            run_id: run.run_id.clone(),
                        },
                        scope,
                        cursor_key,
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(CiRunPage {
            items,
            next_cursor,
            limit: request.limit,
        })
    }

    pub async fn get_surface_run(
        &self,
        tenant_id: &str,
        region: &str,
        run_id: &str,
        expected_repo_ref: &str,
    ) -> Result<Option<CiRunSurface>, CiRunSurfaceError> {
        let run_id = canonical_uuid("run id", run_id)?;
        if expected_repo_ref.is_empty() || expected_repo_ref.len() > 1_024 {
            return Err(CiRunSurfaceError::BadInput(
                "expected repository ref must be non-empty and bounded".into(),
            ));
        }
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let run_for_query = run_id.clone();
        let repo_for_query = expected_repo_ref.to_string();
        #[cfg(any(test, feature = "integration"))]
        let detail_test_barrier = self.surface_detail_test_barrier();
        let rows = with_tenant_repeatable_read_tx(self.pool(), tenant_id, region, move |conn| {
            Box::pin(async move {
                let run = sqlx::query(SELECT_CI_SURFACE_RUN_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&region_owned)
                    .bind(&run_for_query)
                    .bind(&repo_for_query)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                let Some(run) = run else {
                    return Ok(None);
                };
                #[cfg(any(test, feature = "integration"))]
                if let Some(barrier) = detail_test_barrier {
                    barrier.wait().await;
                    barrier.wait().await;
                }
                let jobs = sqlx::query(SELECT_CI_SURFACE_JOBS_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&region_owned)
                    .bind(&run_for_query)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                let steps = sqlx::query(SELECT_CI_SURFACE_STEPS_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&region_owned)
                    .bind(&run_for_query)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                Ok(Some((run, jobs, steps)))
            })
        })
        .await?;
        let Some((run, jobs, steps)) = rows else {
            return Ok(None);
        };
        Ok(Some(CiRunSurface {
            run: run_from_row(run),
            jobs: jobs
                .into_iter()
                .map(job_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            steps: steps.into_iter().map(step_from_row).collect(),
        }))
    }

    /// Read an exact bounded set of run summaries without materializing jobs, steps, or logs.
    /// Repository authorization remains the caller's responsibility.
    pub async fn get_run_summaries(
        &self,
        tenant_id: &str,
        region: &str,
        run_ids: &[String],
    ) -> Result<Vec<CiRunSummary>, CiRunSurfaceError> {
        let run_ids = canonical_run_id_batch(run_ids)?;
        if run_ids.is_empty() {
            return Ok(Vec::new());
        }
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let rows = with_tenant_tx(self.pool(), tenant_id, region, move |conn| {
            Box::pin(async move {
                sqlx::query(SELECT_CI_RUN_SUMMARIES_QUERY)
                    .bind(tenant_id_owned)
                    .bind(region_owned)
                    .bind(run_ids)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))
            })
        })
        .await?;
        Ok(rows.into_iter().map(run_from_row).collect())
    }

    pub async fn get_surface_log_archive(
        &self,
        tenant_id: &str,
        region: &str,
        run_id: &str,
        job_id: &str,
        expected_repo_ref: &str,
        request: CiLogRangeRequest,
    ) -> Result<Option<CiLogArchive>, CiRunSurfaceError> {
        let request = CiLogRangeRequest::new(request.start, request.limit)?;
        let run_id = canonical_uuid("run id", run_id)?;
        let job_id = canonical_uuid("job id", job_id)?;
        if expected_repo_ref.is_empty() || expected_repo_ref.len() > 1_024 {
            return Err(CiRunSurfaceError::BadInput(
                "expected repository ref must be non-empty and bounded".into(),
            ));
        }
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let repo = expected_repo_ref.to_string();
        let query_run = run_id.clone();
        let query_job = job_id.clone();
        let fetch_limit = CI_LOG_SEGMENT_REF_MAX as i64 + 1;
        let result = with_tenant_repeatable_read_tx(self.pool(), tenant_id, region, move |conn| {
            Box::pin(async move {
                let head = sqlx::query(SELECT_CI_LOG_ARCHIVE_HEAD_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&region_owned)
                    .bind(&query_run)
                    .bind(&repo)
                    .bind(&query_job)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                let Some(head) = head else {
                    return Ok(None);
                };
                let total_end: i64 = head.get("total_end");
                if total_end < 0 {
                    return Err(PgError::Query(
                        "CI log archive has a negative total byte offset".into(),
                    ));
                }
                let range_end = if request.start < total_end {
                    request.end().min(total_end)
                } else {
                    request.start
                };
                let segments = if range_end > request.start {
                    sqlx::query(SELECT_CI_LOG_ARCHIVE_SEGMENTS_QUERY)
                        .bind(&tenant_id_owned)
                        .bind(&region_owned)
                        .bind(&query_run)
                        .bind(&repo)
                        .bind(&query_job)
                        .bind(request.start)
                        .bind(range_end)
                        .bind(fetch_limit)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|error| PgError::Query(error.to_string()))?
                } else {
                    Vec::new()
                };
                Ok(Some((total_end, segments)))
            })
        })
        .await?;
        let Some((total_end, rows)) = result else {
            return Ok(None);
        };
        if rows.len() > CI_LOG_SEGMENT_REF_MAX {
            return Err(CiRunSurfaceError::Storage(
                "CI log archive range is too fragmented to serve".into(),
            ));
        }
        let segments = rows
            .into_iter()
            .map(|row| CiLogSegmentRef {
                blob_ref: row.get("blob_ref"),
                byte_start: row.get("byte_start"),
                byte_end: row.get("byte_end"),
            })
            .collect();
        Ok(Some(CiLogArchive {
            total_end,
            segments,
        }))
    }

    pub async fn get_surface_log_tail(
        &self,
        tenant_id: &str,
        region: &str,
        run_id: &str,
        job_id: &str,
        expected_repo_ref: &str,
        last_cursor: Option<u64>,
    ) -> Result<Option<CiLogTailBatch>, CiRunSurfaceError> {
        let run_id = canonical_uuid("run id", run_id)?;
        let job_id = canonical_uuid("job id", job_id)?;
        if expected_repo_ref.is_empty() || expected_repo_ref.len() > 1_024 {
            return Err(CiRunSurfaceError::BadInput(
                "expected repository ref must be non-empty and bounded".into(),
            ));
        }
        if last_cursor.is_some_and(|cursor| i64::try_from(cursor).is_err()) {
            return Err(CiRunSurfaceError::BadInput(
                "CI log resume cursor is malformed".into(),
            ));
        }
        let tenant_id_owned = tenant_id.to_string();
        let region_owned = region.to_string();
        let repo = expected_repo_ref.to_string();
        let query_run = run_id.clone();
        let query_job = job_id.clone();
        let fetch_limit = CI_LOG_TAIL_BATCH_MAX as i64;
        let has_explicit_cursor = last_cursor.is_some();
        let result = with_tenant_repeatable_read_tx(self.pool(), tenant_id, region, move |conn| {
            Box::pin(async move {
                let head = sqlx::query(SELECT_CI_LOG_TAIL_HEAD_QUERY)
                    .bind(&tenant_id_owned)
                    .bind(&region_owned)
                    .bind(&query_run)
                    .bind(&repo)
                    .bind(&query_job)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|error| PgError::Query(error.to_string()))?;
                let Some(head) = head else {
                    return Ok(None);
                };
                let state: String = head.get("state");
                let floor_sequence: Option<i32> = head.get("floor_sequence");
                let head_sequence: Option<i32> = head.get("head_sequence");
                let total_end: i64 = head.get("total_end");
                if floor_sequence.is_some_and(|sequence| sequence < 0)
                    || head_sequence.is_some_and(|sequence| sequence < 0)
                    || total_end < 0
                {
                    return Err(PgError::Query(
                        "CI log archive contains an invalid tail coordinate".into(),
                    ));
                }
                let floor_cursor = log_sequence_cursor(floor_sequence)?;
                let head_cursor = log_sequence_cursor(head_sequence)?;
                let resume_from = last_cursor.unwrap_or_else(|| head_cursor.unwrap_or(0));
                let resume_from_db = i64::try_from(resume_from).map_err(|_| {
                    PgError::Query("CI log resume cursor exceeds the storage range".into())
                })?;
                let cursor_in_window = floor_cursor
                    .is_none_or(|floor| resume_from.saturating_add(1) >= floor)
                    && head_cursor.is_none_or(|head| resume_from <= head)
                    && (head_cursor.is_some() || resume_from == 0);
                let rows = if cursor_in_window {
                    sqlx::query(SELECT_CI_LOG_TAIL_SEGMENTS_QUERY)
                        .bind(&tenant_id_owned)
                        .bind(&region_owned)
                        .bind(&query_run)
                        .bind(&query_job)
                        .bind(resume_from_db)
                        .bind(fetch_limit)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(|error| PgError::Query(error.to_string()))?
                } else {
                    Vec::new()
                };
                let predecessor_byte_end: Option<i64> = if cursor_in_window && resume_from > 0 {
                    sqlx::query_scalar(SELECT_CI_LOG_TAIL_PREDECESSOR_QUERY)
                        .bind(&tenant_id_owned)
                        .bind(&region_owned)
                        .bind(&query_run)
                        .bind(&query_job)
                        .bind(resume_from_db - 1)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|error| PgError::Query(error.to_string()))?
                } else {
                    None
                };
                Ok(Some((
                    state,
                    floor_cursor,
                    head_cursor,
                    total_end,
                    resume_from,
                    predecessor_byte_end,
                    rows,
                )))
            })
        })
        .await?;
        let Some((
            state,
            floor_cursor,
            head_cursor,
            total_end,
            resume_from,
            predecessor_byte_end,
            rows,
        )) = result
        else {
            return Ok(None);
        };
        if floor_cursor.is_some_and(|floor| resume_from.saturating_add(1) < floor) {
            return Err(CiRunSurfaceError::CursorStale);
        }
        if head_cursor.is_none() && has_explicit_cursor {
            return Err(CiRunSurfaceError::CursorStale);
        }
        if head_cursor.is_some_and(|head| resume_from > head) {
            return Err(CiRunSurfaceError::BadInput(
                "CI log resume cursor is ahead of the durable stream".into(),
            ));
        }
        if resume_from > 0
            && predecessor_byte_end.is_none()
            && floor_cursor != resume_from.checked_add(1)
        {
            return Err(CiRunSurfaceError::Storage(
                "CI log archive is missing the resume predecessor".into(),
            ));
        }
        let mut segments = Vec::with_capacity(rows.len());
        let mut expected_cursor = resume_from.checked_add(1).ok_or_else(|| {
            CiRunSurfaceError::Storage("CI log resume cursor cannot advance".into())
        })?;
        let mut previous_byte_end = predecessor_byte_end;
        for row in rows {
            let sequence: i32 = row.get("segment_seq");
            let cursor = u64::try_from(sequence)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| {
                    CiRunSurfaceError::Storage(
                        "CI log archive contains an invalid segment sequence".into(),
                    )
                })?;
            let byte_start: i64 = row.get("byte_start");
            let byte_end: i64 = row.get("byte_end");
            if cursor != expected_cursor
                || byte_start < 0
                || byte_end <= byte_start
                || previous_byte_end.is_some_and(|previous| byte_start != previous)
            {
                return Err(CiRunSurfaceError::Storage(
                    "CI log archive contains a discontinuous tail".into(),
                ));
            }
            segments.push(CiLogTailSegmentRef {
                cursor,
                byte_start,
                byte_end,
            });
            expected_cursor = cursor.checked_add(1).ok_or_else(|| {
                CiRunSurfaceError::Storage("CI log resume cursor cannot advance".into())
            })?;
            previous_byte_end = Some(byte_end);
        }
        let batch_head = segments
            .last()
            .map_or(resume_from, |segment| segment.cursor);
        Ok(Some(CiLogTailBatch {
            resume_from,
            segments,
            total_end,
            terminal: matches!(
                state.as_str(),
                "succeeded" | "failed" | "cancelled" | "reaped"
            ) && head_cursor.is_none_or(|head| batch_head >= head),
        }))
    }
}

pub fn canonical_visible_repo_refs(values: &[String]) -> Result<Vec<String>, CiRunSurfaceError> {
    if values.len() > CI_RUN_VISIBLE_REPO_MAX {
        return Err(CiRunSurfaceError::BadInput(format!(
            "visible repository set exceeds {CI_RUN_VISIBLE_REPO_MAX}"
        )));
    }
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    if values
        .iter()
        .any(|value| value.is_empty() || value.len() > 1_024)
    {
        return Err(CiRunSurfaceError::BadInput(
            "visible repository refs must be non-empty and bounded".into(),
        ));
    }
    Ok(values)
}

fn cursor_scope(
    tenant: &str,
    region: &str,
    state: CiRunStateFilter,
    visible: &[String],
) -> [u8; CURSOR_SCOPE_BYTES] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin:ci-run-cursor:v1\0");
    for value in [tenant, region, state.token()] {
        hasher.update(&(value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    for value in visible {
        hasher.update(&(value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    let mut scope = [0; CURSOR_SCOPE_BYTES];
    scope.copy_from_slice(&digest.as_bytes()[..CURSOR_SCOPE_BYTES]);
    scope
}

fn encode_cursor(
    cursor: &CiRunCursor,
    scope: [u8; CURSOR_SCOPE_BYTES],
    key: &[u8; 32],
) -> Result<String, CiRunSurfaceError> {
    let run_id = Uuid::parse_str(&cursor.run_id).map_err(|_| {
        CiRunSurfaceError::Storage("CI run listing returned a malformed run id".into())
    })?;
    let timestamp = cursor.created_at.as_bytes();
    if !canonical_timestamp(&cursor.created_at) {
        return Err(CiRunSurfaceError::Storage(
            "CI run listing returned a malformed creation timestamp".into(),
        ));
    }
    let mut frame = Vec::with_capacity(CURSOR_FRAME_BYTES);
    frame.push(1);
    frame.extend_from_slice(timestamp);
    frame.extend_from_slice(run_id.as_bytes());
    frame.extend_from_slice(&cursor_tag(key, scope, timestamp, run_id.as_bytes()));
    Ok(format!(
        "{CI_RUN_CURSOR_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(frame)
    ))
}

fn log_sequence_cursor(sequence: Option<i32>) -> Result<Option<u64>, PgError> {
    sequence
        .map(|sequence| {
            u64::try_from(sequence)
                .ok()
                .and_then(|sequence| sequence.checked_add(1))
                .ok_or_else(|| {
                    PgError::Query("CI log archive contains an invalid tail sequence".into())
                })
        })
        .transpose()
}

fn decode_cursor(
    value: &str,
    expected_scope: [u8; CURSOR_SCOPE_BYTES],
    key: &[u8; 32],
) -> Result<CiRunCursor, CiRunSurfaceError> {
    if value.len() > 256 {
        return Err(CiRunSurfaceError::BadInput(
            "CI run cursor is malformed".into(),
        ));
    }
    let encoded = value
        .strip_prefix(CI_RUN_CURSOR_PREFIX)
        .ok_or_else(|| CiRunSurfaceError::BadInput("CI run cursor is malformed".into()))?;
    let frame = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| CiRunSurfaceError::BadInput("CI run cursor is malformed".into()))?;
    if frame.len() != CURSOR_FRAME_BYTES
        || frame[0] != 1
        || URL_SAFE_NO_PAD.encode(&frame) != encoded
    {
        return Err(CiRunSurfaceError::BadInput(
            "CI run cursor is malformed".into(),
        ));
    }
    let timestamp_end = 1 + CURSOR_TIMESTAMP_BYTES;
    let uuid_end = timestamp_end + CURSOR_UUID_BYTES;
    let timestamp = std::str::from_utf8(&frame[1..timestamp_end])
        .map_err(|_| CiRunSurfaceError::BadInput("CI run cursor is malformed".into()))?;
    if !canonical_timestamp(timestamp) {
        return Err(CiRunSurfaceError::BadInput(
            "CI run cursor is malformed".into(),
        ));
    }
    let run_id = Uuid::from_slice(&frame[timestamp_end..uuid_end])
        .map_err(|_| CiRunSurfaceError::BadInput("CI run cursor is malformed".into()))?
        .to_string();
    let expected_tag = cursor_tag(
        key,
        expected_scope,
        &frame[1..timestamp_end],
        &frame[timestamp_end..uuid_end],
    );
    if frame[uuid_end..] != expected_tag {
        return Err(CiRunSurfaceError::CursorStale);
    }
    Ok(CiRunCursor {
        created_at: timestamp.to_string(),
        run_id,
    })
}

fn cursor_tag(
    key: &[u8; 32],
    scope: [u8; CURSOR_SCOPE_BYTES],
    timestamp: &[u8],
    run_id: &[u8],
) -> [u8; CURSOR_SCOPE_BYTES] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(b"myelin:ci-run-cursor-coordinate:v1\0");
    hasher.update(&scope);
    hasher.update(timestamp);
    hasher.update(run_id);
    let digest = hasher.finalize();
    let mut tag = [0; CURSOR_SCOPE_BYTES];
    tag.copy_from_slice(&digest.as_bytes()[..CURSOR_SCOPE_BYTES]);
    tag
}

fn canonical_timestamp(value: &str) -> bool {
    value.len() == CURSOR_TIMESTAMP_BYTES
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value.as_bytes()[19] == b'.'
        && value.ends_with('Z')
        && value.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            10 => byte == b'T',
            13 | 16 => byte == b':',
            19 => byte == b'.',
            26 => byte == b'Z',
            _ => byte.is_ascii_digit(),
        })
        && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

fn canonical_uuid(field: &str, value: &str) -> Result<String, CiRunSurfaceError> {
    parse_canonical_uuid(field, value)?;
    Ok(value.to_string())
}

fn parse_canonical_uuid(field: &str, value: &str) -> Result<Uuid, CiRunSurfaceError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|_| CiRunSurfaceError::BadInput(format!("{field} must be a canonical UUID")))?;
    if parsed.to_string() != value {
        return Err(CiRunSurfaceError::BadInput(format!(
            "{field} must be a canonical UUID"
        )));
    }
    Ok(parsed)
}

fn canonical_run_id_batch(run_ids: &[String]) -> Result<Vec<Uuid>, CiRunSurfaceError> {
    if run_ids.len() > CI_RUN_SUMMARY_BATCH_MAX {
        return Err(CiRunSurfaceError::BadInput(format!(
            "at most {CI_RUN_SUMMARY_BATCH_MAX} CI run ids may be read at once"
        )));
    }
    let mut run_ids = run_ids
        .iter()
        .map(|run_id| parse_canonical_uuid("run id", run_id))
        .collect::<Result<Vec<_>, _>>()?;
    run_ids.sort_unstable();
    run_ids.dedup();
    Ok(run_ids)
}

fn run_from_row(row: sqlx::postgres::PgRow) -> CiRunSummary {
    CiRunSummary {
        run_id: row.get("run_id"),
        pipeline_id: row.get("pipeline_id"),
        repo_ref: row.get("repo_ref"),
        source_ref: row.get("source_ref"),
        commit_oid: row.get("commit_oid"),
        trigger_kind: row.get("trigger_kind"),
        trust_tier: row.get("trust_tier"),
        state: row.get("state"),
        cost_settled: row.get("cost_settled"),
        created_at: row.get("created_at"),
        finished_at: row.get("finished_at"),
    }
}

fn job_from_row(row: sqlx::postgres::PgRow) -> Result<CiJobSurface, CiRunSurfaceError> {
    let stored: Option<Value> = row.get("result_summary");
    let result_summary = match stored {
        Some(value) => CiJobResultSummary::parse(&value).map_err(|error| {
            CiRunSurfaceError::Storage(format!(
                "CI job contains an invalid result summary: {error}"
            ))
        })?,
        None => None,
    };
    Ok(CiJobSurface {
        job_id: row.get("job_id"),
        stage: row.get("stage"),
        name: row.get("name"),
        needs: row.get("needs"),
        matrix_key: row.get("matrix_key"),
        state: row.get("state"),
        attempt: row.get("attempt"),
        result_summary,
    })
}

fn step_from_row(row: sqlx::postgres::PgRow) -> CiStepSurface {
    CiStepSurface {
        job_id: row.get("job_id"),
        step_id: row.get("step_id"),
        byte_start: row.get("byte_start"),
        byte_end: row.get("byte_end"),
        status: row.get("status"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURSOR_KEY: [u8; 32] = [0x71; 32];

    fn refs() -> Vec<String> {
        vec![
            "myelin://acme/git/repo/api".into(),
            "myelin://acme/git/repo/web".into(),
        ]
    }

    #[test]
    fn cursor_is_canonical_keyset_and_filter_visibility_bound() {
        let scope = cursor_scope("acme", "eu-north", CiRunStateFilter::All, &refs());
        let cursor = CiRunCursor {
            created_at: "2026-07-24T12:34:56.123456Z".into(),
            run_id: "11111111-1111-4111-8111-111111111111".into(),
        };
        let encoded = encode_cursor(&cursor, scope, &CURSOR_KEY).unwrap();
        assert!(encoded.starts_with(CI_RUN_CURSOR_PREFIX));
        assert_eq!(decode_cursor(&encoded, scope, &CURSOR_KEY).unwrap(), cursor);
        assert_eq!(
            decode_cursor(
                &encoded,
                cursor_scope("acme", "eu-north", CiRunStateFilter::Failed, &refs()),
                &CURSOR_KEY,
            ),
            Err(CiRunSurfaceError::CursorStale)
        );
        let mut changed = refs();
        changed.push("myelin://acme/git/repo/secret".into());
        assert_eq!(
            decode_cursor(
                &encoded,
                cursor_scope("acme", "eu-north", CiRunStateFilter::All, &changed),
                &CURSOR_KEY,
            ),
            Err(CiRunSurfaceError::CursorStale)
        );
        assert_eq!(
            decode_cursor(
                &encoded,
                cursor_scope("other", "eu-north", CiRunStateFilter::All, &refs()),
                &CURSOR_KEY,
            ),
            Err(CiRunSurfaceError::CursorStale)
        );
    }

    #[test]
    fn cursor_rejects_noncanonical_and_malformed_frames() {
        let scope = cursor_scope("acme", "eu-north", CiRunStateFilter::All, &refs());
        for malformed in ["", "cr1_", "cr1_***", "cr2_AA", &"x".repeat(257)] {
            assert!(matches!(
                decode_cursor(malformed, scope, &CURSOR_KEY),
                Err(CiRunSurfaceError::BadInput(_))
            ));
        }

        let cursor = CiRunCursor {
            created_at: "2026-07-24T12:34:56.123456Z".into(),
            run_id: "11111111-1111-4111-8111-111111111111".into(),
        };
        let encoded = encode_cursor(&cursor, scope, &CURSOR_KEY).unwrap();
        let mut frame = URL_SAFE_NO_PAD
            .decode(encoded.strip_prefix(CI_RUN_CURSOR_PREFIX).unwrap())
            .unwrap();
        frame[9] = b'1';
        let tampered = format!("{CI_RUN_CURSOR_PREFIX}{}", URL_SAFE_NO_PAD.encode(&frame));
        assert_eq!(
            decode_cursor(&tampered, scope, &CURSOR_KEY),
            Err(CiRunSurfaceError::CursorStale),
            "coordinates cannot be changed while retaining a valid cursor tag"
        );

        let forged_key = [0x72; 32];
        let timestamp_end = 1 + CURSOR_TIMESTAMP_BYTES;
        let uuid_end = timestamp_end + CURSOR_UUID_BYTES;
        let forged_tag = cursor_tag(
            &forged_key,
            scope,
            &frame[1..timestamp_end],
            &frame[timestamp_end..uuid_end],
        );
        frame[uuid_end..].copy_from_slice(&forged_tag);
        let forged = format!("{CI_RUN_CURSOR_PREFIX}{}", URL_SAFE_NO_PAD.encode(&frame));
        assert_eq!(
            decode_cursor(&forged, scope, &CURSOR_KEY),
            Err(CiRunSurfaceError::CursorStale),
            "a client cannot recompute the coordinate tag without the cell-derived key"
        );

        let mut impossible = frame;
        impossible[6] = b'9';
        impossible[7] = b'9';
        let impossible = format!(
            "{CI_RUN_CURSOR_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(&impossible)
        );
        assert!(matches!(
            decode_cursor(&impossible, scope, &CURSOR_KEY),
            Err(CiRunSurfaceError::BadInput(_))
        ));
    }

    #[test]
    fn cursor_encoding_fails_closed_on_malformed_storage_coordinates() {
        let scope = cursor_scope("acme", "eu-north", CiRunStateFilter::All, &refs());
        let malformed_run = CiRunCursor {
            created_at: "2026-07-24T12:34:56.123456Z".into(),
            run_id: "not-a-uuid".into(),
        };
        assert!(matches!(
            encode_cursor(&malformed_run, scope, &CURSOR_KEY),
            Err(CiRunSurfaceError::Storage(_))
        ));

        let malformed_timestamp = CiRunCursor {
            created_at: "1970-01-01T00:00:00Z".into(),
            run_id: "11111111-1111-4111-8111-111111111111".into(),
        };
        assert!(matches!(
            encode_cursor(&malformed_timestamp, scope, &CURSOR_KEY),
            Err(CiRunSurfaceError::Storage(_))
        ));
    }

    #[test]
    fn state_and_bounds_are_strict() {
        assert_eq!(
            CiRunStateFilter::parse("timed_out"),
            Some(CiRunStateFilter::TimedOut)
        );
        assert_eq!(CiRunStateFilter::parse("passed"), None);
        assert!(CiRunPageRequest::new(CiRunStateFilter::All, 1, None).is_ok());
        assert!(CiRunPageRequest::new(CiRunStateFilter::All, CI_RUN_PAGE_MAX, None).is_ok());
        assert!(CiRunPageRequest::new(CiRunStateFilter::All, 0, None).is_err());
        assert!(CiRunPageRequest::new(CiRunStateFilter::All, CI_RUN_PAGE_MAX + 1, None).is_err());
    }

    #[test]
    fn exact_run_summary_batches_are_canonical_bounded_and_deduplicated() {
        let first = "71000000-0000-4000-8000-000000000001".to_string();
        let second = "71000000-0000-4000-8000-000000000002".to_string();
        assert!(canonical_run_id_batch(&[]).unwrap().is_empty());
        assert_eq!(
            canonical_run_id_batch(&[second.clone(), first.clone(), second]).unwrap(),
            [
                Uuid::parse_str(&first).unwrap(),
                Uuid::parse_str("71000000-0000-4000-8000-000000000002").unwrap()
            ]
        );
        assert!(canonical_run_id_batch(&["NOT-A-UUID".into()]).is_err());
        assert!(canonical_run_id_batch(&vec![first; CI_RUN_SUMMARY_BATCH_MAX + 1]).is_err());
    }
}
