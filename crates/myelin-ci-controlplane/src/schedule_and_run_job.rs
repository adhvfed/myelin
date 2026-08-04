use crate::scheduler::{EnqueueOutcome, Lane, QueuedJob, SchedulerState, TrustTier};
use myelin_flow::{ActivityError, JobKind, JobRunner, JobSpec};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobScheduleTerms {
    pub tenant_id: String,
    pub region: String,
    pub run_id: String,
    pub lane: Lane,
    pub labels: Vec<String>,
    pub trust_tier: TrustTier,
    pub concurrency_group: Option<String>,
    pub fair_key: String,
}

impl JobScheduleTerms {
    pub fn new(
        tenant_id: impl Into<String>,
        region: impl Into<String>,
        run_id: impl Into<String>,
        lane: Lane,
        trust_tier: TrustTier,
        fair_key: impl Into<String>,
    ) -> JobScheduleTerms {
        JobScheduleTerms {
            tenant_id: tenant_id.into(),
            region: region.into(),
            run_id: run_id.into(),
            lane,
            labels: Vec::new(),
            trust_tier,
            concurrency_group: None,
            fair_key: fair_key.into(),
        }
    }

    pub fn with_labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_concurrency_group(mut self, group: impl Into<String>) -> Self {
        self.concurrency_group = Some(group.into());
        self
    }
}

#[derive(Clone)]
pub struct SchedulerJobRunner {
    scheduler: Arc<Mutex<SchedulerState>>,
    terms: JobScheduleTerms,
    next_seq: Arc<AtomicU64>,
}

impl SchedulerJobRunner {
    pub fn new(
        scheduler: Arc<Mutex<SchedulerState>>,
        terms: JobScheduleTerms,
    ) -> SchedulerJobRunner {
        SchedulerJobRunner {
            scheduler,
            terms,
            next_seq: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn scheduler(&self) -> &Arc<Mutex<SchedulerState>> {
        &self.scheduler
    }

    fn queued_job(&self, spec: &JobSpec) -> QueuedJob {
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
        let mut job = QueuedJob::enqueued(
            self.terms.tenant_id.clone(),
            self.terms.region.clone(),
            spec.idem_token.clone(),
            self.terms.run_id.clone(),
            self.terms.lane,
            self.terms.trust_tier,
            self.terms.fair_key.clone(),
            spec.idem_token.clone(),
            seq,
        );
        if !self.terms.labels.is_empty() {
            job = job.with_labels(self.terms.labels.clone());
        }
        if let Some(group) = &self.terms.concurrency_group {
            job = job.with_concurrency_group(group.clone());
        }
        job
    }
}

impl JobRunner for SchedulerJobRunner {
    fn dispatch(&self, spec: &JobSpec) -> Result<(), ActivityError> {
        if spec.kind != JobKind::Ci {
            return Err(ActivityError(format!(
                "SchedulerJobRunner dispatches kind=ci jobs into job_queue; got kind={} \
                 (an agent job dispatches into the agent runner, not CI's job_queue)",
                spec.kind.as_str()
            )));
        }
        let job = self.queued_job(spec);
        let mut scheduler = self
            .scheduler
            .lock()
            .map_err(|_| ActivityError("the CI scheduler lock was poisoned".into()))?;
        let is_pr_group = job
            .concurrency_group
            .as_deref()
            .is_some_and(|g| g.starts_with("pr:"));
        let outcome = if is_pr_group {
            scheduler.enqueue_superseding(job)
        } else {
            scheduler.enqueue(job)
        };
        debug_assert!(matches!(
            outcome,
            EnqueueOutcome::Inserted | EnqueueOutcome::DuplicateIdem
        ));
        let _ = outcome;
        Ok(())
    }
}

pub fn complete_job(
    scheduler: &Arc<Mutex<SchedulerState>>,
    tenant_id: &str,
    idem_token: &str,
) -> Result<bool, ActivityError> {
    let mut scheduler = scheduler
        .lock()
        .map_err(|_| ActivityError("the CI scheduler lock was poisoned".into()))?;
    Ok(scheduler.complete_job(tenant_id, idem_token))
}

#[cfg(test)]
#[path = "schedule_and_run_job_tests.rs"]
mod tests;
