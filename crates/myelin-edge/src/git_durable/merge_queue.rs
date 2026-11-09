//! The merge queue. Pull requests that asked to merge when ready land one at a time per base
//! branch: the oldest one that people have approved is brought up to date with its base, waits
//! for its required checks on that exact head, and merges. Then the next one starts from the new
//! base. A pull request still waiting for review does not hold up the ones behind it.

use super::check_projection::canonical_required_context_tokens;
use super::*;
use myelin_git::check_status::CheckState;
use myelin_git::lifecycle::{BlockReason, RulesetOutcome};
use myelin_git::pg_pr_store::QueuedPullRequest;
use myelin_git::pr_store::{evaluate_merge, AutoMerge};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

/// Pull requests one pass considers per tenant, read in pages; the rest wait for the next pass.
const MAX_QUEUED_PER_PASS: usize = 5_000;
const QUEUE_PAGE: usize = 500;
/// How long a scheduled pull request may hold its base waiting for required checks.
const MAX_CHECK_WAIT_SECONDS: i64 = 6 * 60 * 60;
const MAX_REASON_CHARS: usize = 300;
const MERGE_QUEUE_PRINCIPAL: &str = "service:git-merge-queue";
const QUEUE_UPDATE_DOMAIN: &str = "myelin.git.merge-queue.update.v1";
const QUEUE_STOP_DOMAIN: &str = "myelin.git.merge-queue.stop.v1";

/// What one pass did with a queued pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueStep {
    /// It merged, so the next pull request on its base can start.
    Merged,
    /// It is being brought up to date or waits for required checks; later ones wait behind it.
    Holding,
    /// It waits for people (approval, requested changes, open conversations) and holds nothing.
    WaitingForReview,
    /// The queue gave up on it and recorded why.
    Stopped(String),
}

impl DurableGitBackend {
    pub fn request_auto_merge_with_operation(
        &self,
        target: PrActorContext<'_>,
        operation_id: &PrOperationId,
        method: MergeMethod,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext {
            repo: actor,
            number,
        } = target;
        if Self::is_agent(actor.principal) {
            return Err(DurableError::Forbidden(
                "an agent cannot schedule a merge; a person decides when work merges".into(),
            ));
        }
        let loc = Self::loc(actor.tenant, actor.region, actor.slug);
        let record = self
            .pr_get(&loc, number, actor.principal)?
            .ok_or_else(|| DurableError::NotFound("pull request not found".into()))?;
        if record.head_repo_slug != actor.slug {
            return Err(DurableError::Conflict(
                "a pull request from a fork cannot merge automatically; merge it directly".into(),
            ));
        }
        let record = self.pr_mutate(
            &loc,
            number,
            PrMutation::RequestAutoMerge {
                requested_by: actor.principal.principal_id.0.clone(),
                method,
            },
            operation_id,
            actor.principal,
        )?;
        self.wake_merge_queue();
        Ok(record)
    }

    pub fn cancel_auto_merge_with_operation(
        &self,
        target: PrActorContext<'_>,
        operation_id: &PrOperationId,
    ) -> Result<PrRecord, DurableError> {
        let PrActorContext {
            repo: actor,
            number,
        } = target;
        let loc = Self::loc(actor.tenant, actor.region, actor.slug);
        self.pr_mutate(
            &loc,
            number,
            PrMutation::CancelAutoMerge {
                reason: None,
                requested_at: None,
            },
            operation_id,
            actor.principal,
        )
    }

    /// How many pull requests were scheduled for the same base before this one, if it is queued.
    pub(super) fn auto_merge_ahead(
        &self,
        loc: &RepoLoc,
        record: &PrRecord,
        principal: &Principal,
    ) -> Result<Option<u64>, DurableError> {
        let Some(request) = record
            .auto_merge
            .as_ref()
            .filter(|_| record.state == PrState::Open)
        else {
            return Ok(None);
        };
        if let Some(store) = &self.pg_prs {
            return store
                .auto_merge_ahead(
                    &Self::verified_pr_scope(principal, loc)?,
                    &loc.repo,
                    &record.base_ref,
                    request.requested_at,
                    record.number,
                )
                .map(Some);
        }
        let earlier = self
            .prs
            .list_bounded(
                loc,
                PR_LIST_PER_REPO_MAX_RECORDS,
                PR_LIST_PER_REPO_MAX_BYTES,
            )?
            .into_iter()
            .filter(|other| other.state == PrState::Open && other.base_ref == record.base_ref)
            .filter_map(|other| {
                other
                    .auto_merge
                    .map(|queued| (queued.requested_at, other.number))
            })
            .filter(|position| *position < (request.requested_at, record.number))
            .count();
        Ok(Some(earlier as u64))
    }

    /// Asks the merge queue to look again soon, after anything that can make a pull request ready.
    pub(crate) fn wake_merge_queue(&self) {
        self.merge_queue_wakeup.notify_one();
    }

    pub(crate) fn merge_queue_wakeup(&self) -> Arc<tokio::sync::Notify> {
        self.merge_queue_wakeup.clone()
    }

    /// One pass over a tenant's merge queues, oldest request first within each base branch.
    /// `stopping` ends the pass early between pull requests.
    pub fn advance_merge_queues(
        &self,
        tenant: &str,
        region: &str,
        stopping: &AtomicBool,
    ) -> Result<Vec<(QueuedPullRequest, QueueStep)>, DurableError> {
        let Some(store) = &self.pg_prs else {
            return Ok(Vec::new());
        };
        let queue = merge_queue_principal(tenant, region);
        let scope = TenantScope::from_verified_token(&queue, queue.region.clone());
        let mut bases: BTreeMap<(String, String), Vec<QueuedPullRequest>> = BTreeMap::new();
        let mut after: Option<QueuedPullRequest> = None;
        let mut seen = 0;
        while seen < MAX_QUEUED_PER_PASS {
            let page = store.auto_merge_queue(&scope, after.as_ref(), QUEUE_PAGE)?;
            let full = page.len() == QUEUE_PAGE;
            seen += page.len();
            after = page.last().cloned();
            for queued in page {
                bases
                    .entry((queued.repo.clone(), queued.base_ref.clone()))
                    .or_default()
                    .push(queued);
            }
            if !full {
                break;
            }
        }
        let mut steps = Vec::new();
        for queued in bases.into_values() {
            for entry in queued {
                if stopping.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(steps);
                }
                let loc = Self::loc(tenant, region, &entry.repo);
                // Only an outage gets here; the next pass retries.
                let step = self
                    .advance_queued(&loc, entry.number, &queue)
                    .unwrap_or_else(|error| {
                        eprintln!(
                            "edge: merge queue could not advance {}#{}: {error}",
                            entry.repo, entry.number
                        );
                        QueueStep::Holding
                    });
                let holds = step == QueueStep::Holding;
                steps.push((entry, step));
                if holds {
                    break;
                }
            }
        }
        Ok(steps)
    }

    /// Takes the next step for one queued pull request. Errors are outages worth retrying;
    /// everything a person can act on stops the automatic merge with a reason instead, so no
    /// pull request holds its base indefinitely.
    fn advance_queued(
        &self,
        loc: &RepoLoc,
        number: u64,
        queue: &Principal,
    ) -> Result<QueueStep, DurableError> {
        let Some(record) = self.pr_get(loc, number, queue)? else {
            return Ok(QueueStep::WaitingForReview);
        };
        let Some(request) = record.auto_merge.clone() else {
            return Ok(QueueStep::WaitingForReview);
        };
        if record.state != PrState::Open {
            return Ok(QueueStep::WaitingForReview);
        }
        let stop = |reason: String| self.stop_auto_merge(loc, &record, queue, reason, &request);
        if record.head_oid != request.head_oid {
            return stop(
                "new commits arrived after the merge was scheduled; schedule it again to merge \
                 them"
                    .into(),
            );
        }
        let Some(requester) = self.active_person(&request.requested_by, queue)? else {
            return stop("the person who scheduled it is no longer active".into());
        };
        if !self.repo_authz.authorize_repo_permission(
            &requester,
            loc,
            RepoPermission::ProtectedPush,
        ) {
            return stop("the person who scheduled it can no longer merge here".into());
        }
        let base = record.base_ref.trim_start_matches("refs/heads/").to_owned();
        let Some(base_tip) = self
            .store
            .open_repo(loc)?
            .read_ref(&branch_ref(&record.base_ref))?
        else {
            return stop(format!("{base} no longer exists"));
        };

        let projected = self.record_with_projected_checks(loc, &record, queue)?;
        let ruleset = self.effective_ruleset_for(loc, &record.base_ref)?;
        let evaluation = match evaluate_merge(&ruleset, &projected) {
            Ok(evaluation) => evaluation,
            Err(error) => return stop(format!("its branch protection could not be read: {error}")),
        };
        if let RulesetOutcome::Blocked { reasons } = &evaluation.ruleset {
            if reasons
                .iter()
                .any(|reason| !matches!(reason, BlockReason::MissingRequiredContext(_)))
            {
                return Ok(QueueStep::WaitingForReview);
            }
        }

        let actor = RepoActorContext::new(&loc.tenant, &loc.region, &loc.repo, &requester);
        let up_to_date = self
            .store
            .open_repo(loc)?
            .is_fast_forward(Some(&base_tip), &CoreOid::new(record.head_oid.clone()))?;
        if !up_to_date {
            let requested_at = request.requested_at.to_string();
            let operation = queue_operation(
                QUEUE_UPDATE_DOMAIN,
                loc,
                number,
                &[&record.head_oid, &base_tip.0, &requested_at],
            )?;
            return match self.update_pr_branch_with_operation(
                actor.for_pr(number),
                Some(&record.head_oid),
                &operation,
            ) {
                Ok(BranchUpdate::Conflicted { paths }) => stop(format!(
                    "{base} and this branch both changed {}",
                    name_some(&paths)
                )),
                Ok(_) => Ok(QueueStep::Holding),
                Err(DurableError::Io(error)) => Err(DurableError::Io(error)),
                Err(error) => stop(format!(
                    "the branch could not be brought up to date: {}",
                    map_durable_err(error).client_message()
                )),
            };
        }

        let failed = self.failed_required_checks(loc, &record, &ruleset, queue)?;
        if !failed.is_empty() {
            return stop(format!("a required check failed: {}", name_some(&failed)));
        }
        if !evaluation.admitted() {
            let waited = record
                .updated_at
                .map(|since| now_unix().map(|now| now - since))
                .transpose()?;
            if waited.is_some_and(|seconds| seconds > MAX_CHECK_WAIT_SECONDS) {
                return stop("its required checks did not finish within six hours".into());
            }
            return Ok(QueueStep::Holding);
        }

        // A person may have cancelled while this pass looked; never merge past that.
        if self
            .pr_get(loc, number, queue)?
            .and_then(|current| current.auto_merge)
            != Some(request.clone())
        {
            return Ok(QueueStep::WaitingForReview);
        }
        // A merge that lost a race is recorded as a finished attempt, so every attempt is new.
        let attempt = self.merge_with_operation(
            actor.for_pr(number),
            &self.fresh_operation_id()?,
            request.method,
        );
        match attempt {
            Ok(MergeAttempt::Merged { .. }) => Ok(QueueStep::Merged),
            // The base moved under the merge; the next pass brings the branch up to date.
            Ok(MergeAttempt::BehindBase { .. } | MergeAttempt::RefRefused(_)) => {
                Ok(QueueStep::Holding)
            }
            Ok(MergeAttempt::Blocked(_)) => {
                stop("branch protection refused the merge when it was tried".into())
            }
            Ok(MergeAttempt::InvalidHead(reason)) => {
                stop(format!("the branch could not merge: {reason}"))
            }
            Err(DurableError::Io(error)) => Err(DurableError::Io(error)),
            Err(DurableError::Conflict(message)) if message == MERGE_IN_PROGRESS => {
                Ok(QueueStep::Holding)
            }
            Err(error) => stop(format!(
                "the merge failed: {}",
                map_durable_err(error).client_message()
            )),
        }
    }

    /// The person behind a request, if they are still an active human in the directory.
    fn active_person(
        &self,
        subject: &str,
        queue: &Principal,
    ) -> Result<Option<Principal>, DurableError> {
        let Some(directory) = &self.principals else {
            return Ok(Some(Principal::new(
                queue.tenant.clone(),
                queue.region.clone(),
                PrincipalId(subject.into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
            )));
        };
        let scope = TenantScope::from_verified_token(queue, queue.region.clone());
        match directory.get_principal(&scope, &PrincipalId(subject.into())) {
            Ok(Some(row))
                if row.status == PrincipalStatus::Active
                    && matches!(row.kind, PrincipalKind::Human) =>
            {
                Ok(Some(Principal::new(
                    row.tenant,
                    row.region,
                    row.principal_id,
                    row.kind,
                    row.data_role,
                    row.status,
                )))
            }
            Ok(_) => Ok(None),
            Err(error) => Err(DurableError::Io(format!(
                "the principal directory is unavailable: {error}"
            ))),
        }
    }

    /// Required checks on the pull request's current head that finished without passing.
    fn failed_required_checks(
        &self,
        loc: &RepoLoc,
        record: &PrRecord,
        ruleset: &BranchProtectionRuleset,
        queue: &Principal,
    ) -> Result<Vec<String>, DurableError> {
        let Some(checks) = &self.checks else {
            return Ok(Vec::new());
        };
        let required = canonical_required_context_tokens(&ruleset.required_contexts)?;
        let repo_ref = format!("myelin://{}/git/repo/{}", loc.tenant, loc.repo);
        let rows = checks
            .rows_for_commit(
                &Self::verified_pr_scope(queue, loc)?,
                &repo_ref,
                &GitOid(record.head_oid.clone()),
            )
            .map_err(|_| DurableError::Io("Git check projection is unavailable".into()))?;
        let mut failed: Vec<String> = rows
            .iter()
            .filter(|row| {
                matches!(
                    row.state,
                    CheckState::Failure | CheckState::Error | CheckState::Cancelled
                )
            })
            .map(|row| row.context.policy_token())
            .filter(|token| required.contains(token))
            .collect();
        failed.sort();
        failed.dedup();
        Ok(failed)
    }

    fn stop_auto_merge(
        &self,
        loc: &RepoLoc,
        record: &PrRecord,
        queue: &Principal,
        reason: String,
        request: &AutoMerge,
    ) -> Result<QueueStep, DurableError> {
        let reason: String = reason.chars().take(MAX_REASON_CHARS).collect();
        let requested_at = request.requested_at.to_string();
        let operation = queue_operation(
            QUEUE_STOP_DOMAIN,
            loc,
            record.number,
            &[&record.head_oid, &reason, &requested_at],
        )?;
        match self.pr_mutate(
            loc,
            record.number,
            PrMutation::CancelAutoMerge {
                reason: Some(reason.clone()),
                requested_at: Some(request.requested_at),
            },
            &operation,
            queue,
        ) {
            Ok(_) => Ok(QueueStep::Stopped(reason)),
            // Someone is merging it by hand; the base waits for that outcome.
            Err(DurableError::Conflict(message)) if message == MERGE_IN_PROGRESS => {
                Ok(QueueStep::Holding)
            }
            // Someone cancelled or scheduled again first; their decision stands.
            Err(DurableError::Conflict(_)) => Ok(QueueStep::WaitingForReview),
            Err(error) => Err(error),
        }
    }
}

/// Names the first few of a list and counts the rest, keeping stop reasons short.
fn name_some(items: &[String]) -> String {
    const NAMED: usize = 5;
    let mut named = items
        .iter()
        .take(NAMED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > NAMED {
        named.push_str(&format!(" and {} more", items.len() - NAMED));
    }
    named
}

fn merge_queue_principal(tenant: &str, region: &str) -> Principal {
    service_principal(tenant, region, MERGE_QUEUE_PRINCIPAL)
}

/// The same step on the same state is the same operation, so a repeated pass replays rather
/// than repeating it.
fn queue_operation(
    domain: &str,
    loc: &RepoLoc,
    number: u64,
    state: &[&str],
) -> Result<PrOperationId, DurableError> {
    let number = number.to_string();
    let mut parts: Vec<&[u8]> = vec![loc.repo.as_bytes(), number.as_bytes()];
    parts.extend(state.iter().map(|part| part.as_bytes()));
    PrOperationId::derive(domain, &parts)
}

/// Open pull requests one branch-follow sweep reads per tenant, in pages.
const MAX_FOLLOWED_PER_SWEEP: usize = 10_000;
const FOLLOW_PAGE: usize = 500;
const BRANCH_FOLLOWER_PRINCIPAL: &str = "service:git-branch-follower";
/// How often the background loop checks that pull requests sit on their branch's tip.
const BRANCH_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

impl DurableGitBackend {
    /// Brings open pull requests back onto their branch's tip when a crash or an outage
    /// interrupted the follow-up of a branch update. Returns how many branches it settled.
    pub fn reconcile_followed_branches(
        &self,
        tenant: &str,
        region: &str,
        stopping: &AtomicBool,
    ) -> Result<usize, DurableError> {
        let Some(store) = &self.pg_prs else {
            return Ok(0);
        };
        let follower = service_principal(tenant, region, BRANCH_FOLLOWER_PRINCIPAL);
        let scope = TenantScope::from_verified_token(&follower, follower.region.clone());
        let mut heads = Vec::new();
        let mut after: Option<(String, u64)> = None;
        while heads.len() < MAX_FOLLOWED_PER_SWEEP {
            let page = store.open_pull_request_heads(&scope, after.as_ref(), FOLLOW_PAGE)?;
            let full = page.len() == FOLLOW_PAGE;
            after = page.last().map(|head| (head.repo.clone(), head.number));
            heads.extend(page);
            if !full {
                break;
            }
        }
        Ok(self.settle_stale_heads(tenant, region, &heads, &follower, stopping))
    }

    /// Follows every branch whose tip differs from a pull request's recorded head.
    fn settle_stale_heads(
        &self,
        tenant: &str,
        region: &str,
        heads: &[myelin_git::pg_pr_store::FollowedHead],
        follower: &Principal,
        stopping: &AtomicBool,
    ) -> usize {
        let mut stale = std::collections::BTreeSet::new();
        for head in heads {
            let loc = Self::loc(tenant, region, &head.repo);
            let branch = branch_ref(&head.head_ref);
            let tip = self
                .store
                .open_repo(&loc)
                .and_then(|repo| repo.read_ref(&branch));
            if let Ok(Some(tip)) = tip {
                if tip.0 != head.head_oid {
                    stale.insert((head.repo.clone(), branch));
                }
            }
        }
        let mut settled = 0;
        for (repo, branch) in stale {
            if stopping.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            self.follow_branch(&Self::loc(tenant, region, &repo), follower, &branch);
            settled += 1;
        }
        settled
    }
}

fn service_principal(tenant: &str, region: &str, id: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(region.into()),
        PrincipalId(id.into()),
        PrincipalKind::Service,
        DataRole::Processor,
        PrincipalStatus::Active,
    )
}

/// The background merge queue: a pass after anything that can make a pull request ready, and at
/// least every `interval` for check results that another service records.
pub struct MergeQueueHandle {
    shutdown: tokio::sync::watch::Sender<bool>,
    /// Ends a running pass between pull requests.
    halt: Arc<AtomicBool>,
    join: tokio::task::JoinHandle<()>,
}

/// Bounds a pass that keeps waking itself, such as a merge followed by the next update.
const MIN_PASS_GAP: std::time::Duration = std::time::Duration::from_millis(500);

impl MergeQueueHandle {
    pub async fn shutdown(self) -> Result<(), String> {
        self.halt.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.shutdown.send(true);
        match tokio::time::timeout(std::time::Duration::from_secs(10), self.join).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err("merge queue task join failed".into()),
            Err(_) => Err("merge queue task did not stop within 10 seconds".into()),
        }
    }
}

/// The tenants a cell runs merge queues and branch sweeps for.
pub enum QueueTenants {
    /// Every active tenant placed in this cell, read again for each pass, so a tenant placed
    /// later joins without a restart.
    Placed {
        placements: myelin_storage::DurablePlacementBacking,
        cell_id: String,
    },
    Fixed(Vec<String>),
}

impl QueueTenants {
    async fn current(&self) -> Vec<String> {
        match self {
            Self::Fixed(tenants) => tenants.clone(),
            Self::Placed {
                placements,
                cell_id,
            } => match placements.local_tenants(cell_id).await {
                Ok(local) => local
                    .into_iter()
                    .filter(|entry| entry.active && entry.cell_id == *cell_id)
                    .map(|entry| entry.tenant_id)
                    .collect(),
                Err(error) => {
                    eprintln!("edge: the merge queue could not read this cell's tenants: {error}");
                    Vec::new()
                }
            },
        }
    }
}

pub fn spawn_merge_queue(
    backend: Arc<DurableGitBackend>,
    tenants: QueueTenants,
    region: String,
    interval: std::time::Duration,
) -> MergeQueueHandle {
    let wakeup = backend.merge_queue_wakeup();
    let (shutdown, mut stopping) = tokio::sync::watch::channel(false);
    let halt = Arc::new(AtomicBool::new(false));
    let pass_halt = halt.clone();
    let join = tokio::spawn(async move {
        // The first pass also settles anything a crash left behind.
        let mut swept_branches_at: Option<tokio::time::Instant> = None;
        loop {
            let sweep_branches =
                swept_branches_at.is_none_or(|at| at.elapsed() >= BRANCH_SWEEP_INTERVAL);
            if sweep_branches {
                swept_branches_at = Some(tokio::time::Instant::now());
            }
            let (pass_backend, pass_tenants, pass_region, halt) = (
                backend.clone(),
                tenants.current().await,
                region.clone(),
                pass_halt.clone(),
            );
            let pass = tokio::task::spawn_blocking(move || {
                for tenant in &pass_tenants {
                    if sweep_branches {
                        match pass_backend.reconcile_followed_branches(tenant, &pass_region, &halt)
                        {
                            Ok(0) => {}
                            Ok(settled) => eprintln!(
                                "edge: moved pull requests on {settled} branch(es) back onto \
                                 their tips"
                            ),
                            Err(error) => eprintln!("edge: a branch-follow sweep failed: {error}"),
                        }
                    }
                    if let Err(error) =
                        pass_backend.advance_merge_queues(tenant, &pass_region, &halt)
                    {
                        eprintln!("edge: a merge queue pass failed: {error}");
                    }
                }
            });
            if pass.await.is_err() {
                eprintln!("edge: a merge queue pass panicked");
            }
            tokio::select! {
                _ = stopping.changed() => return,
                _ = tokio::time::sleep(MIN_PASS_GAP) => {}
            }
            tokio::select! {
                _ = stopping.changed() => return,
                _ = tokio::time::sleep(interval) => {}
                _ = wakeup.notified() => {}
            }
        }
    });
    MergeQueueHandle {
        shutdown,
        halt,
        join,
    }
}

pub const MERGE_QUEUE_INTERVAL_MS_ENV: &str = "MYELIN_GIT_MERGE_QUEUE_INTERVAL_MS";
const DEFAULT_MERGE_QUEUE_INTERVAL_MS: u64 = 5_000;

pub fn merge_queue_interval_from_env() -> Result<std::time::Duration, String> {
    let milliseconds = match std::env::var(MERGE_QUEUE_INTERVAL_MS_ENV) {
        Ok(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|value| *value >= 100)
            .ok_or_else(|| format!("{MERGE_QUEUE_INTERVAL_MS_ENV} must be at least 100"))?,
        Err(_) => DEFAULT_MERGE_QUEUE_INTERVAL_MS,
    };
    Ok(std::time::Duration::from_millis(milliseconds))
}

#[cfg(test)]
#[path = "merge_queue/tests.rs"]
mod tests;
