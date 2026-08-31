use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use sqlx::types::Uuid;
use sqlx::Row;

use super::{by_id, AgentThreadState, DurableAgentThreadBacking};
use crate::{PgError, ProviderError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentThreadReviewHandoff {
    pub thread_id: Uuid,
    pub repository_slug: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_oid: String,
    pub pull_request_number: u64,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAgentThreadReviewHandoff {
    pub repository_slug: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_oid: String,
    pub pull_request_number: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindAgentThreadReviewHandoffOutcome {
    Bound(AgentThreadReviewHandoff),
    AlreadyBound(AgentThreadReviewHandoff),
    Conflict,
    NotFound,
    Unavailable,
}

impl DurableAgentThreadBacking {
    pub async fn review_handoff(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
    ) -> Result<Option<AgentThreadReviewHandoff>, ProviderError> {
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT thread_id, repository_slug, base_ref, head_ref, head_oid,
                                pull_request_number, created_at
                           FROM agent_thread_review_handoff
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3
                            AND owner_principal_id = $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(&owner)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("read agent thread review handoff"))?;
                    row.as_ref().map(handoff_from_row).transpose()
                })
            })
            .await
    }

    pub async fn bind_review_handoff(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        thread_id: Uuid,
        proposal: NewAgentThreadReviewHandoff,
        created_at: DateTime<Utc>,
    ) -> Result<BindAgentThreadReviewHandoffOutcome, ProviderError> {
        validate_proposal(&proposal)?;
        let created_at = created_at
            .with_nanosecond(0)
            .ok_or_else(|| query("agent thread review handoff timestamp is invalid"))?;
        let pull_request_number = i64::try_from(proposal.pull_request_number)
            .map_err(|_| query("agent thread review handoff PR number is too large"))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let thread = by_id(connection, &tenant, &region, thread_id).await?;
                    let Some(thread) = thread else {
                        return Ok(BindAgentThreadReviewHandoffOutcome::NotFound);
                    };
                    if thread.owner_principal_id != owner {
                        return Ok(BindAgentThreadReviewHandoffOutcome::NotFound);
                    }
                    let Some(git) = thread.git.as_ref() else {
                        return Ok(BindAgentThreadReviewHandoffOutcome::Unavailable);
                    };
                    if thread.state != AgentThreadState::Ready
                        || !thread.is_published()
                        || thread.review_ready_at.is_none()
                    {
                        return Ok(BindAgentThreadReviewHandoffOutcome::Unavailable);
                    }
                    if git.repository != proposal.repository_slug
                        || git.base_ref != proposal.base_ref
                        || format!("refs/heads/{}", git.branch) != proposal.head_ref
                    {
                        return Ok(BindAgentThreadReviewHandoffOutcome::Conflict);
                    }

                    let inserted = sqlx::query(
                        "INSERT INTO agent_thread_review_handoff (
                           tenant_id, region, thread_id, owner_principal_id, repository_slug,
                           base_ref, head_ref, head_oid, pull_request_number, created_at
                         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                         ON CONFLICT (tenant_id, region, thread_id) DO NOTHING",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .bind(&owner)
                    .bind(&proposal.repository_slug)
                    .bind(&proposal.base_ref)
                    .bind(&proposal.head_ref)
                    .bind(&proposal.head_oid)
                    .bind(pull_request_number)
                    .bind(created_at)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("bind agent thread review handoff"))?
                    .rows_affected()
                        == 1;
                    let row = sqlx::query(
                        "SELECT thread_id, repository_slug, base_ref, head_ref, head_oid,
                                pull_request_number, created_at
                           FROM agent_thread_review_handoff
                          WHERE tenant_id = $1 AND region = $2 AND thread_id = $3",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(thread_id)
                    .fetch_one(&mut *connection)
                    .await
                    .map_err(query_error("read bound agent thread review handoff"))?;
                    let handoff = handoff_from_row(&row)?;
                    if handoff.repository_slug != proposal.repository_slug
                        || handoff.base_ref != proposal.base_ref
                        || handoff.head_ref != proposal.head_ref
                        || handoff.head_oid != proposal.head_oid
                        || handoff.pull_request_number != proposal.pull_request_number
                    {
                        return Ok(BindAgentThreadReviewHandoffOutcome::Conflict);
                    }
                    Ok(if inserted {
                        BindAgentThreadReviewHandoffOutcome::Bound(handoff)
                    } else {
                        BindAgentThreadReviewHandoffOutcome::AlreadyBound(handoff)
                    })
                })
            })
            .await
    }
}

fn validate_proposal(proposal: &NewAgentThreadReviewHandoff) -> Result<(), ProviderError> {
    let clean = |value: &str, maximum: usize| {
        !value.is_empty()
            && value.len() <= maximum
            && value.trim() == value
            && !value.chars().any(char::is_control)
    };
    if !clean(&proposal.repository_slug, 255)
        || !clean(&proposal.base_ref, 1024)
        || !clean(&proposal.head_ref, 1024)
        || !proposal.base_ref.starts_with("refs/heads/")
        || !proposal.head_ref.starts_with("refs/heads/")
        || proposal.head_oid.len() != 40
        || !proposal
            .head_oid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || proposal.pull_request_number == 0
    {
        return Err(query("agent thread review handoff proposal is invalid"));
    }
    Ok(())
}

fn handoff_from_row(row: &sqlx::postgres::PgRow) -> Result<AgentThreadReviewHandoff, PgError> {
    let number = row.get::<i64, _>("pull_request_number");
    Ok(AgentThreadReviewHandoff {
        thread_id: row.get("thread_id"),
        repository_slug: row.get("repository_slug"),
        base_ref: row.get("base_ref"),
        head_ref: row.get("head_ref"),
        head_oid: row.get("head_oid"),
        pull_request_number: u64::try_from(number)
            .map_err(|_| PgError::Query("stored review handoff PR number is invalid".into()))?,
        created_at: row
            .get::<DateTime<Utc>, _>("created_at")
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    })
}

fn query(message: impl Into<String>) -> ProviderError {
    ProviderError::Pg(PgError::Query(message.into()))
}

fn query_error(context: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{context}: {error}"))
}
