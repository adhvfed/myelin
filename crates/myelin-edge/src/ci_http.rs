//! Authenticated CT-005 CI run read routes.
//!
//! CI owns the durable row/query authority; Edge supplies only the verified principal, derives the
//! tenant/region from it, and reuses Git's live parent-repository Pull decision. Lists prefilter by
//! the bounded visible repository set before pagination. Object reads resolve only the parent
//! repository identity, authorize it, and then load the DAG/job/step detail. Denied and absent runs
//! are the same 404.

use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::git_durable::DurableGitBackend;
use crate::request::EdgeResponse;
use crate::Method;
use base64::Engine as _;
use myelin_ci_controlplane::ci_run_store::{CiRunStore, CiRunStoreError};
use myelin_ci_controlplane::surfacing_store::{
    CiJobSurface, CiLogArchive, CiLogRangeRequest, CiRunPageRequest, CiRunStateFilter,
    CiRunSummary, CiRunSurfaceError, CiStepSurface, CI_LOG_RANGE_DEFAULT, CI_RUN_PAGE_DEFAULT,
};
use myelin_storage::{BlobStore, ContentHash};
use myelin_tenancy::TenantId;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

#[derive(Clone)]
struct DurableCiHttpApi {
    runs: CiRunStore,
    git: Arc<DurableGitBackend>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    runtime: Handle,
}

impl DurableCiHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, CiRunSurfaceError>
    where
        F: Future<Output = Result<T, CiRunSurfaceError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(CiRunSurfaceError::Storage(
                "CI HTTP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => self.runtime.block_on(future),
        }
    }

    fn drive_run<F, T>(&self, future: F) -> Result<T, CiRunStoreError>
    where
        F: Future<Output = Result<T, CiRunStoreError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(CiRunStoreError::Db(
                "CI HTTP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => self.runtime.block_on(future),
        }
    }
}

fn map_surface_error(error: CiRunSurfaceError) -> EdgeError {
    match error {
        CiRunSurfaceError::BadInput(reason) => EdgeError::BadRequest(reason),
        CiRunSurfaceError::CursorStale => {
            EdgeError::Conflict("CI run cursor is stale; restart pagination".into())
        }
        CiRunSurfaceError::Storage(_) => {
            EdgeError::Unavailable("CI run data is temporarily unavailable".into())
        }
    }
}

fn map_run_error(_: CiRunStoreError) -> EdgeError {
    EdgeError::Unavailable("CI run data is temporarily unavailable".into())
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

fn parse_list_query(query: &str) -> Result<CiRunPageRequest, EdgeError> {
    let mut state = None;
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed CI query parameter".into()))?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!("duplicate CI query parameter `{field}`"))
            };
            match name {
                "state" => {
                    if state.is_some() {
                        return Err(duplicate("state"));
                    }
                    state = Some(CiRunStateFilter::parse(value).ok_or_else(|| {
                        EdgeError::BadRequest(
                            "state must be all, queued, running, succeeded, failed, cancelled, \
                             timed_out, or reaped"
                                .into(),
                        )
                    })?);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(duplicate("limit"));
                    }
                    let parsed = value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("CI run limit must be a canonical integer".into())
                    })?;
                    if value != parsed.to_string() {
                        return Err(EdgeError::BadRequest(
                            "CI run limit must be a canonical integer".into(),
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
                            "CI run cursor must be non-empty".into(),
                        ));
                    }
                    cursor = Some(value.to_string());
                }
                "" => return Err(EdgeError::BadRequest("empty CI query parameter".into())),
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown CI query parameter `{other}`"
                    )))
                }
            }
        }
    }
    CiRunPageRequest::new(
        state.unwrap_or(CiRunStateFilter::All),
        limit.unwrap_or(CI_RUN_PAGE_DEFAULT),
        cursor,
    )
    .map_err(map_surface_error)
}

fn parse_log_query(query: &str) -> Result<CiLogRangeRequest, EdgeError> {
    let mut start = None;
    let mut limit = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed CI query parameter".into()))?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!("duplicate CI query parameter `{field}`"))
            };
            match name {
                "start" => {
                    if start.is_some() {
                        return Err(duplicate("start"));
                    }
                    let parsed = value.parse::<i64>().map_err(|_| {
                        EdgeError::BadRequest("CI log start must be a canonical integer".into())
                    })?;
                    if value != parsed.to_string() {
                        return Err(EdgeError::BadRequest(
                            "CI log start must be a canonical integer".into(),
                        ));
                    }
                    start = Some(parsed);
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(duplicate("limit"));
                    }
                    let parsed = value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("CI log limit must be a canonical integer".into())
                    })?;
                    if value != parsed.to_string() {
                        return Err(EdgeError::BadRequest(
                            "CI log limit must be a canonical integer".into(),
                        ));
                    }
                    limit = Some(parsed);
                }
                "" => return Err(EdgeError::BadRequest("empty CI query parameter".into())),
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown CI query parameter `{other}`"
                    )))
                }
            }
        }
    }
    CiLogRangeRequest::new(start.unwrap_or(0), limit.unwrap_or(CI_LOG_RANGE_DEFAULT))
        .map_err(map_surface_error)
}

fn run_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("run")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a CI run id".into()))?;
    if !canonical_uuid(value) {
        return Err(EdgeError::BadRequest(
            "CI run id must be a canonical UUID".into(),
        ));
    }
    Ok(value)
}

fn job_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("job")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a CI job id".into()))?;
    if !canonical_uuid(value) {
        return Err(EdgeError::BadRequest(
            "CI job id must be a canonical UUID".into(),
        ));
    }
    Ok(value)
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn repo_slug_from_ref<'a>(tenant: &str, repo_ref: &'a str) -> Option<&'a str> {
    let prefix = format!("myelin://{tenant}/git/repo/");
    let slug = repo_ref.strip_prefix(&prefix)?;
    if slug.is_empty()
        || slug.len() > 512
        || slug.contains('#')
        || slug.bytes().any(|byte| byte.is_ascii_control())
    {
        return None;
    }
    Some(slug)
}

fn run_json(run: &CiRunSummary) -> Value {
    json!({
        "run_id": run.run_id,
        "pipeline_id": run.pipeline_id,
        "repo_ref": run.repo_ref,
        "commit_oid": run.commit_oid,
        "trigger_kind": run.trigger_kind,
        "trust_tier": run.trust_tier,
        "state": run.state,
        "cost_settled": run.cost_settled,
        "created_at": run.created_at,
        "finished_at": run.finished_at,
    })
}

fn job_json(job: &CiJobSurface) -> Value {
    json!({
        "job_id": job.job_id,
        "stage": job.stage,
        "name": job.name,
        "needs": job.needs,
        "matrix_key": job.matrix_key,
        "state": job.state,
        "attempt": job.attempt,
        "result_summary": job.result_summary,
    })
}

fn step_json(step: &CiStepSurface) -> Value {
    json!({
        "job_id": step.job_id,
        "step_id": step.step_id,
        "byte_start": step.byte_start,
        "byte_end": step.byte_end,
        "status": step.status,
        "details_ref": format!("#step-{}", step.step_id),
    })
}

fn authorized_repo_ref(
    api: &DurableCiHttpApi,
    ctx: &HandlerCtx<'_>,
    run_id: &str,
) -> Result<String, EdgeError> {
    let tenant = ctx.principal.tenant.as_str();
    let region = ctx.principal.region.as_str();
    let record = api
        .drive_run(api.runs.get_ci_run(tenant, region, run_id))
        .map_err(map_run_error)?
        .ok_or_else(|| EdgeError::NotFound("CI run not found".into()))?;
    let repo_ref = record
        .repo_ref
        .ok_or_else(|| EdgeError::NotFound("CI run not found".into()))?;
    let repo_slug = repo_slug_from_ref(tenant, &repo_ref)
        .ok_or_else(|| EdgeError::NotFound("CI run not found".into()))?;
    if !api.git.may_view_ci_repo(ctx.principal, repo_slug) {
        return Err(EdgeError::NotFound("CI run not found".into()));
    }
    Ok(repo_ref)
}

fn materialize_log_archive(
    blobs: &dyn BlobStore,
    tenant: &TenantId,
    request: CiLogRangeRequest,
    archive: CiLogArchive,
) -> Result<(i64, Vec<u8>), EdgeError> {
    if archive.total_end < 0 {
        return Err(EdgeError::Unavailable(
            "CI log data is temporarily unavailable".into(),
        ));
    }
    let target_end = if request.start < archive.total_end {
        request.end().min(archive.total_end)
    } else {
        request.start
    };
    if target_end == request.start {
        if !archive.segments.is_empty() {
            return Err(EdgeError::Unavailable(
                "CI log data is temporarily unavailable".into(),
            ));
        }
        return Ok((target_end, Vec::new()));
    }

    let mut output = Vec::with_capacity((target_end - request.start) as usize);
    let mut covered_end = request.start;
    let mut previous_end = None;
    for segment in archive.segments {
        if segment.byte_start < 0 || segment.byte_end <= segment.byte_start {
            return Err(EdgeError::Unavailable(
                "CI log data is temporarily unavailable".into(),
            ));
        }
        if let Some(previous_end) = previous_end {
            if segment.byte_start != previous_end {
                return Err(EdgeError::Unavailable(
                    "CI log data is temporarily unavailable".into(),
                ));
            }
        } else if segment.byte_start > request.start || segment.byte_end <= request.start {
            return Err(EdgeError::Unavailable(
                "CI log data is temporarily unavailable".into(),
            ));
        }
        previous_end = Some(segment.byte_end);

        let hash = ContentHash::parse(&segment.blob_ref)
            .map_err(|_| EdgeError::Unavailable("CI log data is temporarily unavailable".into()))?;
        if hash.to_multihash_string() != segment.blob_ref || hash.digest_hex.len() != 64 {
            return Err(EdgeError::Unavailable(
                "CI log data is temporarily unavailable".into(),
            ));
        }
        let bytes = blobs
            .get_bounded(
                tenant,
                &hash,
                myelin_ci_controlplane::PRODUCTION_LOG_SEGMENT_MAX_BYTES,
            )
            .map_err(|_| EdgeError::Unavailable("CI log data is temporarily unavailable".into()))?;
        let expected_len = usize::try_from(segment.byte_end - segment.byte_start)
            .map_err(|_| EdgeError::Unavailable("CI log data is temporarily unavailable".into()))?;
        if bytes.len() != expected_len {
            return Err(EdgeError::Unavailable(
                "CI log data is temporarily unavailable".into(),
            ));
        }

        let slice_start = covered_end.max(segment.byte_start);
        let slice_end = target_end.min(segment.byte_end);
        if slice_end > slice_start {
            let local_start = usize::try_from(slice_start - segment.byte_start).map_err(|_| {
                EdgeError::Unavailable("CI log data is temporarily unavailable".into())
            })?;
            let local_end = usize::try_from(slice_end - segment.byte_start).map_err(|_| {
                EdgeError::Unavailable("CI log data is temporarily unavailable".into())
            })?;
            output.extend_from_slice(&bytes[local_start..local_end]);
            covered_end = slice_end;
        }
    }
    if covered_end != target_end {
        return Err(EdgeError::Unavailable(
            "CI log data is temporarily unavailable".into(),
        ));
    }
    Ok((target_end, output))
}

struct CiRunListHandler {
    api: DurableCiHttpApi,
}

impl Handler for CiRunListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "CI run list accepts no request body".into(),
            ));
        }
        let request = parse_list_query(&ctx.request.query)?;
        let slugs = self
            .api
            .git
            .visible_repo_slugs_for_ci(ctx.principal)
            .map_err(|_| {
                EdgeError::Unavailable("CI repository visibility is unavailable".into())
            })?;
        let tenant = ctx.principal.tenant.as_str();
        let refs = slugs
            .into_iter()
            .map(|slug| format!("myelin://{tenant}/git/repo/{slug}"))
            .collect::<Vec<_>>();
        let page = self
            .api
            .drive(self.api.runs.list_surface_runs(
                tenant,
                ctx.principal.region.as_str(),
                &refs,
                request,
            ))
            .map_err(map_surface_error)?;
        let items = page.items.iter().map(run_json).collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), page.next_cursor, page.limit as usize),
        )))
    }
}

struct CiRunViewHandler {
    api: DurableCiHttpApi,
}

impl Handler for CiRunViewHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "CI run view accepts no query parameters or request body".into(),
            ));
        }
        let run_id = run_param(ctx)?;
        let tenant = ctx.principal.tenant.as_str();
        let region = ctx.principal.region.as_str();
        let repo_ref = authorized_repo_ref(&self.api, ctx, run_id)?;
        let detail = self
            .api
            .drive(
                self.api
                    .runs
                    .get_surface_run(tenant, region, run_id, &repo_ref),
            )
            .map_err(map_surface_error)?
            .ok_or_else(|| EdgeError::NotFound("CI run not found".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "run": run_json(&detail.run),
                "jobs": detail.jobs.iter().map(job_json).collect::<Vec<_>>(),
                "steps": detail.steps.iter().map(step_json).collect::<Vec<_>>(),
            }),
        )))
    }
}

struct CiLogArchiveHandler {
    api: DurableCiHttpApi,
}

impl Handler for CiLogArchiveHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "CI log range accepts no request body".into(),
            ));
        }
        let request = parse_log_query(&ctx.request.query)?;
        let run_id = run_param(ctx)?;
        let job_id = job_param(ctx)?;
        let tenant = ctx.principal.tenant.as_str();
        let region = ctx.principal.region.as_str();
        let repo_ref = authorized_repo_ref(&self.api, ctx, run_id)?;
        let archive = self
            .api
            .drive(
                self.api
                    .runs
                    .get_surface_log_archive(tenant, region, run_id, job_id, &repo_ref, request),
            )
            .map_err(map_surface_error)?
            .ok_or_else(|| EdgeError::NotFound("CI run not found".into()))?;
        let total_end = archive.total_end;
        let (byte_end, bytes) = materialize_log_archive(
            self.api.blobs.as_ref(),
            &TenantId(tenant.to_string()),
            request,
            archive,
        )?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "run_id": run_id,
                "job_id": job_id,
                "byte_start": request.start,
                "byte_end": byte_end,
                "total_end": total_end,
                "next_offset": if byte_end < total_end { Some(byte_end) } else { None },
                "encoding": "base64",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )))
    }
}

pub fn register_ci(
    builder: GatewayBuilder,
    runs: CiRunStore,
    git: Arc<DurableGitBackend>,
    blobs: Arc<dyn BlobStore + Send + Sync>,
    runtime: Handle,
) -> GatewayBuilder {
    let api = DurableCiHttpApi {
        runs,
        git,
        blobs,
        runtime,
    };
    builder
        .route(
            Method::Get,
            "/v1/ci/runs",
            "ci.runs.list",
            Arc::new(CiRunListHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/ci/runs/{run}",
            "ci.run.view",
            Arc::new(CiRunViewHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/ci/runs/{run}/jobs/{job}/log",
            "ci.run.log.read",
            Arc::new(CiLogArchiveHandler { api }),
        )
}

#[cfg(test)]
mod log_tests {
    use super::*;
    use myelin_ci_controlplane::surfacing_store::CiLogSegmentRef;
    use myelin_storage::FsBlobStore;

    #[test]
    fn log_query_is_canonical_bounded_and_fail_closed() {
        assert_eq!(
            parse_log_query("").expect("defaults"),
            CiLogRangeRequest::new(0, CI_LOG_RANGE_DEFAULT).unwrap()
        );
        assert_eq!(
            parse_log_query("start=12&limit=34").expect("explicit range"),
            CiLogRangeRequest::new(12, 34).unwrap()
        );
        for query in [
            "start=-1",
            "start=00",
            "start=1&start=2",
            "limit=0",
            "limit=262145",
            "limit=01",
            "other=1",
            "start",
        ] {
            assert!(
                matches!(parse_log_query(query), Err(EdgeError::BadRequest(_))),
                "{query} must be rejected"
            );
        }
    }

    #[test]
    fn archive_materialization_requires_exact_contiguous_verified_bytes() {
        let blobs = FsBlobStore::new();
        let tenant = TenantId("ci-log-test".into());
        let first = b"alpha\n";
        let second = b"beta\n";
        let first_ref = blobs
            .put(&tenant, first)
            .expect("put first")
            .to_multihash_string();
        let second_ref = blobs
            .put(&tenant, second)
            .expect("put second")
            .to_multihash_string();
        let request = CiLogRangeRequest::new(3, 6).unwrap();
        let archive = CiLogArchive {
            total_end: 11,
            segments: vec![
                CiLogSegmentRef {
                    blob_ref: first_ref.clone(),
                    byte_start: 0,
                    byte_end: 6,
                },
                CiLogSegmentRef {
                    blob_ref: second_ref.clone(),
                    byte_start: 6,
                    byte_end: 11,
                },
            ],
        };
        assert_eq!(
            materialize_log_archive(&blobs, &tenant, request, archive).expect("verified range"),
            (9, b"ha\nbet".to_vec())
        );

        let gap = CiLogArchive {
            total_end: 12,
            segments: vec![
                CiLogSegmentRef {
                    blob_ref: first_ref,
                    byte_start: 0,
                    byte_end: 6,
                },
                CiLogSegmentRef {
                    blob_ref: second_ref,
                    byte_start: 7,
                    byte_end: 12,
                },
            ],
        };
        assert!(matches!(
            materialize_log_archive(&blobs, &tenant, request, gap),
            Err(EdgeError::Unavailable(_))
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_query_is_strict_and_bounded() {
        assert_eq!(
            parse_list_query("").unwrap(),
            CiRunPageRequest::new(CiRunStateFilter::All, CI_RUN_PAGE_DEFAULT, None).unwrap()
        );
        assert_eq!(
            parse_list_query("state=failed&limit=7&cursor=cr1_abc").unwrap(),
            CiRunPageRequest::new(CiRunStateFilter::Failed, 7, Some("cr1_abc".into())).unwrap()
        );
        for bad in [
            "state=passed",
            "limit=0",
            "limit=01",
            "limit=+1",
            "limit=101",
            "cursor=",
            "state=all&state=failed",
            "unknown=x",
        ] {
            assert!(parse_list_query(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repository_ref_parser_is_exact_and_tenant_bound() {
        assert_eq!(
            repo_slug_from_ref("acme", "myelin://acme/git/repo/team/app"),
            Some("team/app")
        );
        for bad in [
            "myelin://other/git/repo/team/app",
            "myelin://acme/git/repo/",
            "myelin://acme/git/repo/app#step-1",
            "repo:app",
        ] {
            assert_eq!(repo_slug_from_ref("acme", bad), None, "{bad}");
        }
    }
}
