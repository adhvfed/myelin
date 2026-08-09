use super::{
    canonical_issue_ref, is_canonical_uuid, map_store_error, no_store, DurableIssueHttpApi,
};
use crate::catalogue::{Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;
pub use myelin_issues::api::{MAX_ISSUE_IMPORT_JSON_BYTES, MAX_ISSUE_IMPORT_RECORDS};
use myelin_issues::{CreateIssue, ImportIssue, SourceSystem};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportIssueBody {
    source_id: String,
    project_id: String,
    type_id: String,
    prefix: String,
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBatchBody {
    source: String,
    records: Vec<ImportIssueBody>,
}

struct ImportBatch {
    job_id: String,
    source: SourceSystem,
    records: Vec<ImportIssue>,
}

fn parse_import_batch(ctx: &HandlerCtx<'_>) -> Result<ImportBatch, EdgeError> {
    let job_id = import_job_param(ctx)?.to_string();
    parse_import_batch_bytes(job_id, &ctx.request.body)
}

fn parse_import_batch_bytes(job_id: String, bytes: &[u8]) -> Result<ImportBatch, EdgeError> {
    if bytes.len() > MAX_ISSUE_IMPORT_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "Issues import request body exceeds {MAX_ISSUE_IMPORT_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected an Issues import batch)".into(),
        ));
    }
    let body: ImportBatchBody = serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid issue import body: {error}")))?;
    let source = SourceSystem::parse(&body.source).ok_or_else(|| {
        EdgeError::BadRequest("import source must be jira, linear, github, or csv".into())
    })?;
    if body.records.is_empty() || body.records.len() > MAX_ISSUE_IMPORT_RECORDS {
        return Err(EdgeError::BadRequest(format!(
            "issue import must contain 1..={MAX_ISSUE_IMPORT_RECORDS} records"
        )));
    }

    let mut source_ids = BTreeSet::new();
    let mut records = Vec::with_capacity(body.records.len());
    for record in body.records {
        if !source_ids.insert(record.source_id.clone()) {
            return Err(EdgeError::BadRequest(
                "issue import contains a duplicate source_id".into(),
            ));
        }
        records.push(ImportIssue {
            import_job_id: job_id.clone(),
            source,
            source_id: record.source_id,
            issue: CreateIssue {
                project_id: record.project_id,
                type_id: record.type_id,
                prefix: record.prefix,
                title: record.title,
            },
        });
    }
    Ok(ImportBatch {
        job_id,
        source,
        records,
    })
}

fn import_job_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("import_job")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an import job id".into()))?;
    if !is_canonical_uuid(value) {
        return Err(EdgeError::BadRequest(
            "import job id must be a canonical UUID".into(),
        ));
    }
    Ok(value)
}

struct IssueImportDryRunHandler {
    api: DurableIssueHttpApi,
}

impl Handler for IssueImportDryRunHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let batch = parse_import_batch(ctx)?;
        for record in &batch.records {
            self.api
                .store
                .validate_import(ctx.principal, record)
                .map_err(map_store_error)?;
        }
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "import": {
                    "job_id": batch.job_id,
                    "source": batch.source.token(),
                    "mode": "dry_run",
                },
                "reconciliation": {
                    "received": batch.records.len(),
                    "ready": batch.records.len(),
                    "lossy": 0,
                    "dropped": 0,
                },
                "losses": [],
            }),
        )))
    }
}

struct IssueImportRunHandler {
    api: DurableIssueHttpApi,
}

impl Handler for IssueImportRunHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let batch = parse_import_batch(ctx)?;
        for record in &batch.records {
            self.api
                .store
                .validate_import(ctx.principal, record)
                .map_err(map_store_error)?;
        }

        let job_id = batch.job_id;
        let source = batch.source;
        let received = batch.records.len();
        let store = Arc::clone(&self.api.store);
        let outcomes = self
            .api
            .drive(async {
                let mut outcomes = Vec::with_capacity(received);
                for record in batch.records {
                    let source_id = record.source_id.clone();
                    let receipt = store.import_issue(ctx.principal, record).await?;
                    outcomes.push((source_id, receipt));
                }
                Ok(outcomes)
            })
            .map_err(map_store_error)?;
        let created = outcomes
            .iter()
            .filter(|(_, receipt)| receipt.created)
            .count();
        let issues = outcomes
            .into_iter()
            .map(|(source_id, receipt)| {
                let issue_ref = canonical_issue_ref(&ctx.principal.tenant.0, &receipt.issue.key);
                json!({
                    "source_id": source_id,
                    "created": receipt.created,
                    "issue": {
                        "id": receipt.issue.id,
                        "key": receipt.issue.key,
                        "ref": issue_ref,
                        "project_id": receipt.issue.project_id,
                    },
                    "authorization": {
                        "status": "requested",
                        "request_event_id": receipt.issue.authorization_request_event_id,
                    },
                })
            })
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            202,
            &json!({
                "import": {
                    "job_id": job_id,
                    "source": source.token(),
                    "mode": "run",
                    "resumable": true,
                },
                "summary": {
                    "received": received,
                    "created": created,
                    "resumed": received - created,
                    "lossy": 0,
                    "dropped": 0,
                },
                "issues": issues,
                "losses": [],
            }),
        )))
    }
}

pub(super) fn register(builder: GatewayBuilder, api: DurableIssueHttpApi) -> GatewayBuilder {
    builder
        .route(
            Method::Post,
            "/v1/issues/imports/{import_job}/dry-run",
            "issues.import.dry_run",
            Arc::new(IssueImportDryRunHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/issues/imports/{import_job}/run",
            "issues.import.run",
            Arc::new(IssueImportRunHandler { api }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_batches_are_bounded_strict_and_source_id_unique() {
        let job = "33333333-3333-3333-3333-333333333333";
        let record = json!({
            "source_id": "JIRA-41",
            "project_id": "11111111-1111-1111-1111-111111111111",
            "type_id": "22222222-2222-2222-2222-222222222222",
            "prefix": "ENG",
            "title": "Imported without a provider token",
        });
        let valid = serde_json::to_vec(&json!({
            "source": "jira",
            "records": [record.clone()],
        }))
        .unwrap();
        let parsed = parse_import_batch_bytes(job.into(), &valid).unwrap();
        assert_eq!(parsed.job_id, job);
        assert_eq!(parsed.source, SourceSystem::Jira);
        assert_eq!(parsed.records.len(), 1);

        for invalid in [
            json!({"source": "JIRA", "records": [record.clone()]}),
            json!({"source": "jira", "records": []}),
            json!({"source": "jira", "records": [record.clone(), record.clone()]}),
            json!({"source": "jira", "records": [record.clone()], "tenant": "other"}),
        ] {
            assert!(
                parse_import_batch_bytes(job.into(), &serde_json::to_vec(&invalid).unwrap())
                    .is_err()
            );
        }
        assert!(matches!(
            parse_import_batch_bytes(job.into(), &vec![b'x'; MAX_ISSUE_IMPORT_JSON_BYTES + 1]),
            Err(EdgeError::PayloadTooLarge(_))
        ));
    }
}
