use std::sync::Arc;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EmitContextBase, EventDraft, EventType, IdMinter,
    OutboxTransaction, OutboxTx, Visibility,
};
use myelin_storage::pgrelay::PgRelay;

use crate::core::RepoLoc;
use crate::events::{
    GIT_PR_HEAD_TRIGGER_SCHEMA_V2, GIT_PR_OPENED, GIT_PR_SYNCHRONIZED, GIT_REVIEW_SUBMITTED,
};
use crate::notif_rules::{
    outstanding_review_request_signal_drafts, review_request_resolved_signal_draft,
};
use crate::pr_store::PrRecord;
use crate::typed_edges::{closes_issue_targets, emit_lifecycle_edges};

fn pg_query(action: &'static str) -> myelin_storage::PgError {
    myelin_storage::PgError::Query(action.into())
}

pub(crate) async fn co_commit_event(
    conn: &mut sqlx::PgConnection,
    minter: Arc<dyn IdMinter>,
    mut ctx: EmitContextBase,
    loc: &RepoLoc,
    record: &PrRecord,
    event_type: &'static str,
    operation: Option<&serde_json::Value>,
) -> Result<(), myelin_storage::PgError> {
    let signal_ctx = EmitContextBase {
        schema_ver: 1,
        ..ctx.clone()
    };
    let safe_operation = operation.map(|value| {
        serde_json::json!({
            "operation_id": value.get("operation_id"),
            "base_ref": value.get("base_ref"),
            "expected_old_oid": value.get("expected_old_oid"),
            "head_oid": value.get("head_oid"),
        })
    });
    let is_head_trigger = matches!(event_type, GIT_PR_OPENED | GIT_PR_SYNCHRONIZED);
    let head_generation = if is_head_trigger {
        let number =
            i64::try_from(record.number).map_err(|_| pg_query("encode PR event generation"))?;
        let generation: i64 = sqlx::query_scalar(
            "SELECT version FROM git_pr \
              WHERE tenant_id=$1 AND region=$2 AND repo_slug=$3 AND number=$4",
        )
        .bind(&loc.tenant)
        .bind(&loc.region)
        .bind(&loc.repo)
        .bind(number)
        .fetch_one(&mut *conn)
        .await
        .map_err(|_| pg_query("read PR event generation"))?;
        if generation <= 0 {
            return Err(pg_query("invalid PR event generation"));
        }
        ctx.schema_ver = GIT_PR_HEAD_TRIGGER_SCHEMA_V2;
        Some(generation)
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "repo": loc.repo,
        "number": record.number,
        "base_ref": record.base_ref,
        "head_ref": record.head_ref,
        "head_oid": record.head_oid,
        // Anyone who can read the target repository may observe this event; a fork's own name
        // can be private, so only the fact that it is a fork travels.
        "is_fork": record.head_repo_slug != loc.repo,
        "state": format!("{:?}", record.state).to_ascii_lowercase(),
        "operation": safe_operation,
    });
    if let Some(generation) = head_generation {
        payload["head_generation"] = generation.into();
    }
    if event_type == GIT_REVIEW_SUBMITTED {
        // Automations act on the decision itself, such as an agent addressing requested changes.
        // Automation filters see top-level scalars, so the decision is flat like the rest.
        if let Some(review) = record.reviews.last() {
            payload["review_verdict"] = review_verdict_token(&review.state).into();
            payload["reviewer_is_agent"] = review.is_agent.into();
        }
    }
    let mut tx = OutboxTransaction::detached(minter.clone(), ctx);
    tx.emit(
        EventDraft {
            type_: EventType(event_type.into()),
            subject: ArtifactRef(format!(
                "myelin://{}/git/pr/{}:{}",
                loc.tenant, loc.repo, record.number
            )),
            aggregate: AggregateKey(format!("git/pr/{}:{}", loc.repo, record.number)),
            payload,
            data_role: DataRole::Processor,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        None,
    )
    .map_err(|_| pg_query("stage PR lifecycle event"))?;
    let mut rows = tx
        .into_staged_rows()
        .map_err(|_| pg_query("encode PR lifecycle event"))?;
    let lifecycle = rows
        .first()
        .map(|row| row.envelope.clone())
        .ok_or_else(|| pg_query("missing PR envelope"))?;
    let signal_drafts = if matches!(event_type, GIT_PR_OPENED | GIT_PR_SYNCHRONIZED) {
        outstanding_review_request_signal_drafts(
            &signal_ctx.tenant,
            &signal_ctx.region,
            &loc.repo,
            record,
            &signal_ctx.recorded_at.0,
        )
        .map_err(|_| pg_query("derive outstanding PR review-request signals"))?
    } else if event_type == GIT_REVIEW_SUBMITTED {
        review_request_resolved_signal_draft(
            &signal_ctx.tenant,
            &signal_ctx.region,
            &loc.repo,
            record,
            &signal_ctx.recorded_at.0,
        )
        .map_err(|_| pg_query("derive resolved PR review-request signal"))?
        .into_iter()
        .collect()
    } else {
        crate::notif_rules::auto_merge_signal_drafts(
            &signal_ctx.tenant,
            &signal_ctx.region,
            &loc.repo,
            record,
            event_type,
            &signal_ctx.recorded_at.0,
        )
        .map_err(|_| pg_query("derive automatic-merge signals"))?
    };
    let closes_targets = if event_type == GIT_PR_OPENED {
        closes_issue_targets(&loc.tenant, record.body_md.as_deref().unwrap_or_default())
            .map_err(|_| pg_query("parse PR lifecycle trailers"))?
    } else {
        Vec::new()
    };
    if !signal_drafts.is_empty() || !closes_targets.is_empty() {
        let mut derived_tx = OutboxTransaction::detached(minter, signal_ctx);
        for draft in signal_drafts {
            derived_tx
                .emit(draft, Some(&lifecycle))
                .map_err(|_| pg_query("stage PR review-request signal"))?;
        }
        emit_lifecycle_edges(
            &mut derived_tx,
            &lifecycle.subject,
            &closes_targets,
            &[],
            &lifecycle,
        )
        .map_err(|_| pg_query("stage PR lifecycle reference edges"))?;
        rows.extend(
            derived_tx
                .into_staged_rows()
                .map_err(|_| pg_query("encode PR derived events"))?,
        );
    }
    PgRelay::co_commit_rows_in_tx(conn, &rows).await
}

fn review_verdict_token(state: &crate::lifecycle::ReviewState) -> &'static str {
    use crate::lifecycle::{ReviewState, ReviewVerdict};
    match state {
        ReviewState::Submitted(ReviewVerdict::Approve) => "approve",
        ReviewState::Submitted(ReviewVerdict::RequestChanges) => "request_changes",
        ReviewState::Submitted(ReviewVerdict::Comment) => "comment",
        ReviewState::Requested => "requested",
        ReviewState::Dismissed => "dismissed",
    }
}

#[cfg(test)]
mod review_payload_tests {
    use super::review_verdict_token;
    use crate::lifecycle::{ReviewState, ReviewVerdict};

    #[test]
    fn a_review_event_names_the_decision_automations_filter_on() {
        assert_eq!(
            review_verdict_token(&ReviewState::Submitted(ReviewVerdict::RequestChanges)),
            "request_changes"
        );
        assert_eq!(
            review_verdict_token(&ReviewState::Submitted(ReviewVerdict::Approve)),
            "approve"
        );
        assert_eq!(
            review_verdict_token(&ReviewState::Submitted(ReviewVerdict::Comment)),
            "comment"
        );
    }
}
