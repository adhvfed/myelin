use std::collections::BTreeMap;

pub use myelin_ci_sandbox::TrustTier;

pub const CLAIM_QUERY: &str = "\
WITH eligible AS (
  SELECT q.tenant_id, q.region, q.job_id
  FROM job_queue q
  LEFT JOIN fair_deficit f
    ON f.tenant_id = q.tenant_id AND f.region = q.region AND f.fair_key = q.fair_key
  WHERE q.state = 'queued'
    AND q.region = $1
    AND EXISTS (
      SELECT 1 FROM workflow_run w
      WHERE w.tenant_id = q.tenant_id
        AND w.region = q.region
        AND w.run_id = q.run_id::text
        AND w.state IN ('running', 'waiting')
    )
    AND EXISTS (
      SELECT 1 FROM ci_run c
      WHERE c.tenant_id = q.tenant_id
        AND c.region = q.region
        AND c.wf_run_id = q.run_id
        AND c.state = 'running'
    )
    AND q.labels <@ $2
    AND q.trust_tier = ANY($3)
    AND (
      q.concurrency_group IS NULL
      OR q.concurrency_group NOT LIKE 'deploy:%'
      OR NOT EXISTS (
        SELECT 1 FROM job_queue r
        WHERE r.tenant_id = q.tenant_id
          AND r.concurrency_group = q.concurrency_group
          AND r.state = 'running'
      )
    )
  ORDER BY
    CASE q.lane WHEN 'interactive' THEN 2 WHEN 'batch' THEN 1 ELSE 0 END DESC,
    COALESCE(f.deficit, 0) DESC,
    q.enqueued_at ASC
  FOR UPDATE OF q SKIP LOCKED
  LIMIT 1
)
UPDATE job_queue j
SET state = 'leased',
    lease_owner = $4,
    lease_expires = statement_timestamp() + ($5 || ' seconds')::interval,
    lease_epoch = j.lease_epoch + 1,
    claim_nonce = gen_random_uuid(),
    claim_started_at = statement_timestamp(),
    claim_expires_at = statement_timestamp()
      + (COALESCE(j.claim_window_secs::text, $5) || ' seconds')::interval
FROM eligible e
WHERE j.tenant_id = e.tenant_id AND j.job_id = e.job_id
RETURNING j.tenant_id, j.job_id, j.run_id, j.lane, j.concurrency_group, j.fair_key, j.trust_tier,
          j.lease_epoch, j.claim_nonce::text AS claim_nonce, j.claim_window_secs,
          FLOOR(EXTRACT(EPOCH FROM j.claim_started_at))::bigint AS claim_started_at_epoch_secs,
          FLOOR(EXTRACT(EPOCH FROM j.claim_expires_at))::bigint AS claim_expires_at_epoch_secs";

pub const CANCEL_SUPERSEDED_QUERY: &str = "\
UPDATE job_queue
SET state = 'terminal', lease_owner = NULL, lease_expires = NULL
WHERE tenant_id = $1
  AND region = $2
  AND concurrency_group = $3
  AND state IN ('queued', 'leased')
  AND job_id <> $4
RETURNING job_id";

pub const REAP_QUERY: &str = "\
WITH candidates AS MATERIALIZED (
  SELECT tenant_id, region, job_id, state, lease_epoch, claim_nonce
  FROM job_queue
  WHERE region = $1
    AND state IN ('leased', 'running')
    AND lease_expires < now()
    AND EXISTS (
      SELECT 1 FROM workflow_run w
      WHERE w.tenant_id = job_queue.tenant_id
        AND w.region = job_queue.region
        AND w.run_id = job_queue.run_id::text
        AND w.state IN ('running', 'waiting')
    )
    AND EXISTS (
      SELECT 1 FROM ci_run c
      WHERE c.tenant_id = job_queue.tenant_id
        AND c.region = job_queue.region
        AND c.wf_run_id = job_queue.run_id
        AND c.state = 'running'
    )
  FOR UPDATE SKIP LOCKED
),
expired AS (
  SELECT tenant_id, job_id, lease_epoch, claim_nonce
  FROM candidates
  WHERE state = 'leased'
     OR (
       state = 'running'
       AND pg_try_advisory_xact_lock(
         hashtextextended(
           jsonb_build_array(
             tenant_id::text,
             region::text,
             job_id::text,
             lease_epoch::text,
             claim_nonce::text
           )::text,
           0
         )
       )
     )
)
UPDATE job_queue j
SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
FROM expired e
WHERE j.tenant_id = e.tenant_id AND j.job_id = e.job_id
RETURNING j.tenant_id, j.job_id, e.lease_epoch AS reaped_lease_epoch,
          e.claim_nonce::text AS reaped_claim_nonce";

pub const AUTHORIZE_JOB_LAUNCH_QUERY: &str = "\
WITH launched AS (
UPDATE job_queue
SET state = 'running',
    lease_expires = LEAST(
      claim_expires_at,
      statement_timestamp() + ($10 || ' seconds')::interval
    )
WHERE tenant_id = $1
  AND region = $2
  AND job_id = $3::uuid
  AND run_id = $4::uuid
  AND state = 'leased'
  AND lease_owner = $5
  AND lease_epoch = $6
  AND claim_nonce = $7::uuid
  AND FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint = $8
  AND FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint = $9
  AND claim_expires_at > statement_timestamp()
  AND completion_receipt IS NULL
RETURNING tenant_id, region, job_id
)
UPDATE ci_job AS surface
SET state = 'running'
FROM launched
WHERE surface.tenant_id = launched.tenant_id
  AND surface.region = launched.region
  AND surface.job_id = launched.job_id
  AND surface.state IN ('queued', 'leased')
RETURNING surface.job_id";

pub const AUTHORIZE_JOB_LAUNCH_V2_QUERY: &str = "\
WITH launched AS (
UPDATE job_queue
SET state = 'running',
    lease_expires = LEAST(
      claim_expires_at,
      statement_timestamp() + ($10 || ' seconds')::interval
    )
WHERE tenant_id = $1
  AND region = $2
  AND job_id = $3::uuid
  AND run_id = $4::uuid
  AND state = 'leased'
  AND lease_owner = $5
  AND lease_epoch = $6
  AND claim_nonce = $7::uuid
  AND FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint = $8
  AND FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint = $9
  AND claim_expires_at > statement_timestamp()
  AND lease_expires > statement_timestamp()
  AND completion_receipt IS NULL
  AND EXISTS (
    SELECT 1
    FROM ci_job_credential_generation AS generation
    WHERE generation.tenant_id = job_queue.tenant_id
      AND generation.region = job_queue.region
      AND generation.job_id = job_queue.job_id
      AND generation.lease_epoch = job_queue.lease_epoch
      AND generation.claim_nonce = job_queue.claim_nonce
      AND generation.purpose = 'workload'
      AND generation.binding_version = $11
      AND generation.generation_id = $12
      AND generation.jti = $13
      AND generation.issued_at_epoch_secs = $14
      AND generation.expires_at_epoch_secs = $15
      AND generation.wf_run_id = job_queue.run_id
      AND generation.ci_run_id = $16::uuid
      AND generation.token_authority_handle = $17
      AND generation.idem_token = $18
      AND generation.lease_owner = job_queue.lease_owner
      AND generation.claim_started_at_epoch_secs = $8
      AND generation.claim_expires_at_epoch_secs = $9
      AND generation.expires_at_epoch_secs >
          FLOOR(EXTRACT(EPOCH FROM statement_timestamp()))::bigint
      AND NOT EXISTS (
        SELECT 1
        FROM ci_job_credential_generation AS successor
        WHERE successor.tenant_id = generation.tenant_id
          AND successor.region = generation.region
          AND successor.job_id = generation.job_id
          AND successor.lease_epoch = generation.lease_epoch
          AND successor.claim_nonce = generation.claim_nonce
          AND successor.phase_ordinal > generation.phase_ordinal
      )
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job_spec AS launch
    WHERE launch.tenant_id = job_queue.tenant_id
      AND launch.region = job_queue.region
      AND launch.job_id = job_queue.job_id
      AND launch.run_id = job_queue.run_id
      AND (launch.spec #>> '{spec,workspace,commit}') IS NOT DISTINCT FROM $19::text
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = job_queue.tenant_id
      AND parent.region = job_queue.region
      AND parent.job_id = job_queue.job_id
      AND parent.wf_run_id = job_queue.run_id
      AND parent.ci_run_id = $16::uuid
      AND parent.lease_owner = job_queue.lease_owner
      AND parent.lease_epoch = job_queue.lease_epoch
      AND parent.claim_nonce = job_queue.claim_nonce
      AND parent.claim_started_at_epoch_secs = $8
      AND parent.claim_expires_at_epoch_secs = $9
  )
  AND NOT EXISTS (
    SELECT 1
    FROM ci_job_prelaunch_usage AS unresolved
    WHERE unresolved.tenant_id = job_queue.tenant_id
      AND unresolved.region = job_queue.region
      AND unresolved.job_id = job_queue.job_id
      AND unresolved.lease_epoch = job_queue.lease_epoch
      AND unresolved.claim_nonce = job_queue.claim_nonce
      AND unresolved.status <> 'measured'
  )
RETURNING tenant_id, region, job_id
)
UPDATE ci_job AS surface
SET state = 'running'
FROM launched
WHERE surface.tenant_id = launched.tenant_id
  AND surface.region = launched.region
  AND surface.job_id = launched.job_id
  AND surface.state IN ('queued', 'leased')
RETURNING surface.job_id";

pub const VERIFY_JOB_LAUNCH_LIVE_QUERY: &str = "\
SELECT 1
FROM job_queue
WHERE tenant_id = $1
  AND region = $2
  AND job_id = $3::uuid
  AND run_id = $4::uuid
  AND state = 'leased'
  AND lease_owner = $5
  AND lease_epoch = $6
  AND claim_nonce = $7::uuid
  AND FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint = $8
  AND FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint = $9
  AND claim_expires_at > statement_timestamp()
  AND completion_receipt IS NULL
  AND EXISTS (
    SELECT 1 FROM ci_job AS surface
    WHERE surface.tenant_id = job_queue.tenant_id
      AND surface.region = job_queue.region
      AND surface.job_id = job_queue.job_id
      AND surface.state IN ('queued', 'leased')
  )";

pub const RENEW_PREPARATION_LEASE_QUERY: &str = "\
UPDATE job_queue AS q
SET lease_expires = LEAST(
      q.claim_expires_at,
      statement_timestamp() + ($10 || ' seconds')::interval
    )
WHERE q.tenant_id = $1
  AND q.region = $2
  AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid
  AND q.state = 'leased'
  AND q.lease_owner = $5
  AND q.lease_epoch = $6
  AND q.claim_nonce = $7::uuid
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $8
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $9
  AND q.claim_expires_at > statement_timestamp()
  AND q.completion_receipt IS NULL
  AND EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = q.tenant_id
      AND parent.region = q.region
      AND parent.job_id = q.job_id
      AND parent.wf_run_id = q.run_id
      AND parent.lease_owner = $5
      AND parent.lease_epoch = q.lease_epoch
      AND parent.claim_nonce = q.claim_nonce
      AND parent.claim_started_at_epoch_secs = $8
      AND parent.claim_expires_at_epoch_secs = $9
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
RETURNING q.job_id";

pub const INSERT_JOB_QUEUE_QUERY: &str = "\
INSERT INTO job_queue
  (tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group, fair_key, idem_token, stage, claim_window_secs, reservation_write_version, state)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 'queued')
ON CONFLICT (tenant_id, idem_token) DO NOTHING
RETURNING job_id";

pub const COMPLETE_JOB_QUERY: &str = "\
UPDATE job_queue
SET state = 'terminal', lease_owner = NULL, lease_expires = NULL
WHERE tenant_id = $1
  AND job_id = $2
  AND state <> 'terminal'
RETURNING job_id";

pub const HEARTBEAT_QUERY: &str = "\
UPDATE job_queue
SET lease_expires = now() + ($4 || ' seconds')::interval
WHERE tenant_id = $1
  AND job_id = $2
  AND state IN ('leased', 'running')
  AND lease_owner = $3
RETURNING job_id";

pub const CONSUME_CLAIM_QUERY: &str = "\
UPDATE job_queue
SET state = 'terminal', completion_receipt = $6, lease_owner = NULL, lease_expires = NULL
WHERE tenant_id = $1
  AND job_id = $2
  AND lease_owner = $3
  AND lease_epoch = $4
  AND claim_nonce = $5::uuid
  AND stage = $7
  AND state = 'running'
  AND completion_receipt IS NULL
RETURNING job_id";

pub const CONSUME_PREPARATION_CLAIM_QUERY: &str = "\
UPDATE job_queue AS q
SET state = 'terminal', completion_receipt = $14, lease_owner = NULL, lease_expires = NULL
WHERE q.tenant_id = $1
  AND q.region = $2
  AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid
  AND q.idem_token = $5
  AND q.lease_owner = $6
  AND q.lease_epoch = $7
  AND q.claim_nonce = $8::uuid
  AND q.stage = $9
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $10
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $11
  AND q.claim_expires_at > statement_timestamp()
  AND q.state = 'leased'
  AND q.completion_receipt IS NULL
  AND EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = q.tenant_id
      AND parent.region = q.region
      AND parent.job_id = q.job_id
      AND parent.wf_run_id = q.run_id
      AND parent.ci_run_id = $12::uuid
      AND parent.reserve_handle = $13
      AND parent.lease_owner = $6
      AND parent.lease_epoch = q.lease_epoch
      AND parent.claim_nonce = q.claim_nonce
      AND parent.claim_started_at_epoch_secs = $10
      AND parent.claim_expires_at_epoch_secs = $11
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
RETURNING q.job_id";

pub const CONSUME_SECRET_WITHHELD_CLAIM_QUERY: &str = "\
UPDATE job_queue AS q
SET state = 'terminal', completion_receipt = $14, lease_owner = NULL, lease_expires = NULL
WHERE q.tenant_id = $1
  AND q.region = $2
  AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid
  AND q.idem_token = $5
  AND q.lease_owner = $6
  AND q.lease_epoch = $7
  AND q.claim_nonce = $8::uuid
  AND q.stage = $9
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $10
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $11
  AND q.claim_expires_at > statement_timestamp()
  AND q.state = 'leased'
  AND q.completion_receipt IS NULL
  AND NOT EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = q.tenant_id
      AND parent.region = q.region
      AND parent.job_id = q.job_id
      AND parent.wf_run_id = q.run_id
      AND parent.ci_run_id = $12::uuid
      AND parent.reserve_handle = $13
      AND parent.lease_owner = $6
      AND parent.lease_epoch = q.lease_epoch
      AND parent.claim_nonce = q.claim_nonce
      AND parent.claim_started_at_epoch_secs = $10
      AND parent.claim_expires_at_epoch_secs = $11
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
RETURNING q.job_id";

pub const CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY: &str = "\
UPDATE job_queue AS q
SET state = 'terminal', completion_receipt = $14, lease_owner = NULL, lease_expires = NULL
WHERE q.tenant_id = $1
  AND q.region = $2
  AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid
  AND q.idem_token = $5
  AND q.lease_owner = $6
  AND q.lease_epoch = $7
  AND q.claim_nonce = $8::uuid
  AND q.stage = $9
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $10
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $11
  AND q.claim_expires_at > statement_timestamp()
  AND q.state = 'leased'
  AND q.completion_receipt IS NULL
  AND EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = q.tenant_id
      AND parent.region = q.region
      AND parent.job_id = q.job_id
      AND parent.wf_run_id = q.run_id
      AND parent.ci_run_id = $12::uuid
      AND parent.reserve_handle = $13
    GROUP BY parent.budget_revision, parent.max_parent_attempts
    HAVING count(*) = parent.max_parent_attempts
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
RETURNING q.job_id";

pub const REQUEUE_PREPARATION_CLAIM_QUERY: &str = "\
UPDATE job_queue AS q
SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
WHERE q.tenant_id = $1
  AND q.region = $2
  AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid
  AND q.idem_token = $5
  AND q.lease_owner = $6
  AND q.lease_epoch = $7
  AND q.claim_nonce = $8::uuid
  AND q.stage = $9
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $10
  AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $11
  AND q.claim_expires_at > statement_timestamp()
  AND q.state = 'leased'
  AND q.completion_receipt IS NULL
  AND EXISTS (
    SELECT 1
    FROM ci_job_parent_attempt AS parent
    WHERE parent.tenant_id = q.tenant_id
      AND parent.region = q.region
      AND parent.job_id = q.job_id
      AND parent.wf_run_id = q.run_id
      AND parent.ci_run_id = $12::uuid
      AND parent.reserve_handle = $13
      AND parent.lease_owner = $6
      AND parent.lease_epoch = q.lease_epoch
      AND parent.claim_nonce = q.claim_nonce
      AND parent.claim_started_at_epoch_secs = $10
      AND parent.claim_expires_at_epoch_secs = $11
  )
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
RETURNING q.job_id";

pub const RESET_REQUEUED_PREPARATION_CI_JOB_SURFACE_QUERY: &str = "\
UPDATE ci_job SET state = 'queued'
WHERE tenant_id = $1 AND job_id = $2::uuid AND state = 'leased'";

pub const READ_COMPLETION_DISPOSITION_QUERY: &str = "\
SELECT state, completion_receipt FROM job_queue WHERE tenant_id = $1 AND job_id = $2";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Interactive,
    Batch,
    Deploy,
}

impl Lane {
    pub fn priority(self) -> i32 {
        match self {
            Lane::Interactive => 2,
            Lane::Batch => 1,
            Lane::Deploy => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Interactive => "interactive",
            Lane::Batch => "batch",
            Lane::Deploy => "deploy",
        }
    }

    pub fn from_token(token: &str) -> Option<Lane> {
        match token {
            "interactive" => Some(Lane::Interactive),
            "batch" => Some(Lane::Batch),
            "deploy" => Some(Lane::Deploy),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobState {
    Queued,
    Leased,
    Running,
    Terminal,
}

impl JobState {
    fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Leased => "leased",
            JobState::Running => "running",
            JobState::Terminal => "terminal",
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueuedJob {
    pub tenant_id: String,
    pub region: String,
    pub job_id: String,
    pub run_id: String,
    pub lane: Lane,
    pub labels: Vec<String>,
    pub trust_tier: TrustTier,
    pub concurrency_group: Option<String>,
    pub fair_key: String,
    pub idem_token: String,
    pub enqueued_seq: u64,
    pub state: JobState,
    pub lease_owner: Option<String>,
    pub lease_expires: Option<u64>,
}

impl QueuedJob {
    #[allow(clippy::too_many_arguments)]
    pub fn enqueued(
        tenant_id: impl Into<String>,
        region: impl Into<String>,
        job_id: impl Into<String>,
        run_id: impl Into<String>,
        lane: Lane,
        trust_tier: TrustTier,
        fair_key: impl Into<String>,
        idem_token: impl Into<String>,
        enqueued_seq: u64,
    ) -> Self {
        QueuedJob {
            tenant_id: tenant_id.into(),
            region: region.into(),
            job_id: job_id.into(),
            run_id: run_id.into(),
            lane,
            labels: Vec::new(),
            trust_tier,
            concurrency_group: None,
            fair_key: fair_key.into(),
            idem_token: idem_token.into(),
            enqueued_seq,
            state: JobState::Queued,
            lease_owner: None,
            lease_expires: None,
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

#[derive(Clone, Debug)]
pub struct ClaimRequest {
    pub cell_region: String,
    pub runner_labels: Vec<String>,
    pub runner_allowed_tiers: Vec<TrustTier>,
    pub lease_owner: String,
    pub lease_ttl: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub tenant_id: String,
    pub job_id: String,
    pub run_id: String,
    pub lane: Lane,
    pub concurrency_group: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SchedulerState {
    jobs: Vec<QueuedJob>,
    fair_deficit: BTreeMap<(String, String, String), i64>,
    now: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Inserted,
    DuplicateIdem,
}

impl SchedulerState {
    pub fn new() -> Self {
        SchedulerState::default()
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn advance(&mut self, ticks: u64) {
        self.now += ticks;
    }

    pub fn set_deficit(&mut self, tenant_id: &str, region: &str, fair_key: &str, deficit: i64) {
        self.fair_deficit.insert(
            (
                tenant_id.to_string(),
                region.to_string(),
                fair_key.to_string(),
            ),
            deficit,
        );
    }

    fn deficit_of(&self, job: &QueuedJob) -> i64 {
        self.fair_deficit
            .get(&(
                job.tenant_id.clone(),
                job.region.clone(),
                job.fair_key.clone(),
            ))
            .copied()
            .unwrap_or(0)
    }

    pub fn jobs(&self) -> &[QueuedJob] {
        &self.jobs
    }

    fn find(&self, tenant_id: &str, job_id: &str) -> Option<usize> {
        self.jobs
            .iter()
            .position(|j| j.tenant_id == tenant_id && j.job_id == job_id)
    }

    pub fn enqueue(&mut self, job: QueuedJob) -> EnqueueOutcome {
        let dup = self
            .jobs
            .iter()
            .any(|j| j.tenant_id == job.tenant_id && j.idem_token == job.idem_token);
        if dup {
            return EnqueueOutcome::DuplicateIdem;
        }
        self.jobs.push(job);
        EnqueueOutcome::Inserted
    }

    pub fn enqueue_superseding(&mut self, job: QueuedJob) -> EnqueueOutcome {
        let group = job.concurrency_group.clone();
        let new_job_id = job.job_id.clone();
        let tenant = job.tenant_id.clone();
        let region = job.region.clone();
        let outcome = self.enqueue(job);
        if outcome == EnqueueOutcome::Inserted {
            if let Some(group) = group {
                self.cancel_superseded(&tenant, &region, &group, &new_job_id);
            }
        }
        outcome
    }

    pub fn cancel_superseded(
        &mut self,
        tenant_id: &str,
        region: &str,
        group: &str,
        keep_job_id: &str,
    ) -> Vec<String> {
        let mut cancelled = Vec::new();
        for j in &mut self.jobs {
            if j.tenant_id == tenant_id
                && j.region == region
                && j.concurrency_group.as_deref() == Some(group)
                && j.job_id != keep_job_id
                && matches!(j.state, JobState::Queued | JobState::Leased)
            {
                j.state = JobState::Terminal;
                j.lease_owner = None;
                j.lease_expires = None;
                cancelled.push(j.job_id.clone());
            }
        }
        cancelled
    }

    fn deploy_group_running(&self, tenant_id: &str, group: &str) -> bool {
        self.jobs.iter().any(|j| {
            j.tenant_id == tenant_id
                && j.concurrency_group.as_deref() == Some(group)
                && j.state == JobState::Running
        })
    }

    fn eligible(&self, job: &QueuedJob, req: &ClaimRequest) -> bool {
        if job.state != JobState::Queued {
            return false;
        }
        if job.region != req.cell_region {
            return false;
        }
        if !job
            .labels
            .iter()
            .all(|l| req.runner_labels.iter().any(|rl| rl == l))
        {
            return false;
        }
        if !req.runner_allowed_tiers.contains(&job.trust_tier) {
            return false;
        }
        if let Some(group) = &job.concurrency_group {
            if group.starts_with("deploy:") && self.deploy_group_running(&job.tenant_id, group) {
                return false;
            }
        }
        true
    }

    pub fn claim(&mut self, req: &ClaimRequest) -> Option<Claimed> {
        let mut best: Option<usize> = None;
        for (i, job) in self.jobs.iter().enumerate() {
            if !self.eligible(job, req) {
                continue;
            }
            best = Some(match best {
                None => i,
                Some(b) => {
                    let cur = &self.jobs[b];
                    let key_new = (
                        job.lane.priority(),
                        self.deficit_of(job),
                        -(job.enqueued_seq as i64),
                    );
                    let key_cur = (
                        cur.lane.priority(),
                        self.deficit_of(cur),
                        -(cur.enqueued_seq as i64),
                    );
                    if key_new > key_cur {
                        i
                    } else {
                        b
                    }
                }
            });
        }
        let idx = best?;
        let now = self.now;
        let ttl = req.lease_ttl;
        let owner = req.lease_owner.clone();
        let job = &mut self.jobs[idx];
        job.state = JobState::Leased;
        job.lease_owner = Some(owner);
        job.lease_expires = Some(now + ttl);
        Some(Claimed {
            tenant_id: job.tenant_id.clone(),
            job_id: job.job_id.clone(),
            run_id: job.run_id.clone(),
            lane: job.lane,
            concurrency_group: job.concurrency_group.clone(),
        })
    }

    pub fn mark_running(&mut self, tenant_id: &str, job_id: &str) -> bool {
        if let Some(i) = self.find(tenant_id, job_id) {
            if self.jobs[i].state == JobState::Leased {
                self.jobs[i].state = JobState::Running;
                return true;
            }
        }
        false
    }

    pub fn reap(&mut self) -> Vec<(String, String)> {
        let now = self.now;
        let mut reaped = Vec::new();
        for j in &mut self.jobs {
            if matches!(j.state, JobState::Leased | JobState::Running)
                && j.lease_expires.is_some_and(|e| e < now)
            {
                j.state = JobState::Queued;
                j.lease_owner = None;
                j.lease_expires = None;
                reaped.push((j.tenant_id.clone(), j.job_id.clone()));
            }
        }
        reaped
    }

    pub fn heartbeat(&mut self, tenant_id: &str, job_id: &str, owner: &str, ttl: u64) -> bool {
        let now = self.now;
        if let Some(i) = self.find(tenant_id, job_id) {
            let j = &mut self.jobs[i];
            if matches!(j.state, JobState::Leased | JobState::Running)
                && j.lease_owner.as_deref() == Some(owner)
            {
                j.lease_expires = Some(now + ttl);
                return true;
            }
        }
        false
    }

    pub fn complete_job(&mut self, tenant_id: &str, job_id: &str) -> bool {
        if let Some(i) = self.find(tenant_id, job_id) {
            let j = &mut self.jobs[i];
            if j.state != JobState::Terminal {
                j.state = JobState::Terminal;
                j.lease_owner = None;
                j.lease_expires = None;
                return true;
            }
        }
        false
    }

    pub fn state_of(&self, tenant_id: &str, job_id: &str) -> Option<JobState> {
        self.find(tenant_id, job_id).map(|i| self.jobs[i].state)
    }

    pub fn queue_depth(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Queued)
            .count()
    }
}

pub fn lane_token(lane: Lane) -> &'static str {
    lane.as_str()
}

pub fn state_token(state: JobState) -> &'static str {
    state.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(region: &str, owner: &str) -> ClaimRequest {
        ClaimRequest {
            cell_region: region.into(),
            runner_labels: vec!["linux".into(), "arm64".into(), "gpu".into()],
            runner_allowed_tiers: vec![TrustTier::Trusted, TrustTier::UntrustedFork],
            lease_owner: owner.into(),
            lease_ttl: 30,
        }
    }

    fn job(id: &str, lane: Lane, seq: u64) -> QueuedJob {
        QueuedJob::enqueued(
            "tenantA",
            "fr-par",
            id,
            format!("run-{id}"),
            lane,
            TrustTier::Trusted,
            "tenantA",
            format!("idem-{id}"),
            seq,
        )
    }

    #[test]
    fn claim_residency_in_region_only() {
        let mut s = SchedulerState::new();
        let mut j = job("j1", Lane::Batch, 0);
        j.region = "us-east".into();
        s.enqueue(j);
        assert!(
            s.claim(&req("fr-par", "r1")).is_none(),
            "an out-of-region job is NOT claimable (residency by construction)"
        );
        s.enqueue(job("j2", Lane::Batch, 1));
        let c = s
            .claim(&req("fr-par", "r1"))
            .expect("the in-region job claims");
        assert_eq!(c.job_id, "j2");
    }

    #[test]
    fn claim_affinity_labels_subset() {
        let mut s = SchedulerState::new();
        s.enqueue(job("jneed", Lane::Batch, 0).with_labels(["windows"]));
        assert!(
            s.claim(&req("fr-par", "r1")).is_none(),
            "a job needing a label the runner lacks is NOT claimable"
        );
        s.enqueue(job("jok", Lane::Batch, 1).with_labels(["linux", "gpu"]));
        let c = s
            .claim(&req("fr-par", "r1"))
            .expect("the label-eligible job claims");
        assert_eq!(c.job_id, "jok", "labels ⊆ runner_labels claims");
    }

    #[test]
    fn claim_trust_tier_membership() {
        let mut s = SchedulerState::new();
        let mut j = job("jself", Lane::Batch, 0);
        j.trust_tier = TrustTier::SelfHosted;
        s.enqueue(j);
        assert!(
            s.claim(&req("fr-par", "r1")).is_none(),
            "a SelfHosted job is NOT claimable by a runner that doesn't allow that tier"
        );
        let mut r = req("fr-par", "r1");
        r.runner_allowed_tiers = vec![TrustTier::SelfHosted];
        let c = s.claim(&r).expect("the self-hosted-allowed runner claims");
        assert_eq!(c.job_id, "jself");
    }

    #[test]
    fn claim_lane_priority_strict() {
        let mut s = SchedulerState::new();
        s.enqueue(job("jbatch", Lane::Batch, 0));
        s.enqueue(job("jinteractive", Lane::Interactive, 1));
        let c = s.claim(&req("fr-par", "r1")).expect("a job claims");
        assert_eq!(
            c.job_id, "jinteractive",
            "interactive is claimed before an OLDER batch job (lane priority is strict)"
        );
    }

    #[test]
    fn claim_fairness_deficit_orders() {
        let mut s = SchedulerState::new();
        let mut ja = job("ja", Lane::Batch, 0);
        ja.fair_key = "tenantA".into();
        let mut jb = job("jb", Lane::Batch, 1);
        jb.fair_key = "tenantB".into();
        jb.tenant_id = "tenantA".into();
        s.enqueue(ja);
        s.enqueue(jb);
        s.set_deficit("tenantA", "fr-par", "tenantB", 100);
        s.set_deficit("tenantA", "fr-par", "tenantA", 1);
        let c = s.claim(&req("fr-par", "r1")).expect("a job claims");
        assert_eq!(
            c.job_id, "jb",
            "the higher-deficit fair_key is claimed first (the DRR ORDER BY term)"
        );
    }

    #[test]
    fn claim_oldest_first_within_equal_key() {
        let mut s = SchedulerState::new();
        s.enqueue(job("jnew", Lane::Batch, 5));
        s.enqueue(job("jold", Lane::Batch, 1));
        let c = s.claim(&req("fr-par", "r1")).expect("a job claims");
        assert_eq!(
            c.job_id, "jold",
            "the oldest job (lowest enqueued_seq) claims"
        );
    }

    #[test]
    fn concurrency_deploy_serialize_one_at_a_time() {
        let mut s = SchedulerState::new();
        s.enqueue(job("d1", Lane::Deploy, 0).with_concurrency_group("deploy:prod"));
        s.enqueue(job("d2", Lane::Deploy, 1).with_concurrency_group("deploy:prod"));

        let c1 = s
            .claim(&req("fr-par", "r1"))
            .expect("the first deploy claims");
        assert_eq!(c1.job_id, "d1");
        assert!(
            s.mark_running("tenantA", "d1"),
            "the first deploy is running"
        );

        assert!(
            s.claim(&req("fr-par", "r2")).is_none(),
            "a second deploy:prod is NOT claimable while the first runs (serialize)"
        );

        let d1_idx = s.find("tenantA", "d1").unwrap();
        s.jobs[d1_idx].state = JobState::Terminal;
        let c2 = s
            .claim(&req("fr-par", "r2"))
            .expect("the second deploy now claims");
        assert_eq!(
            c2.job_id, "d2",
            "the second deploy:prod claims once the first is done"
        );
    }

    #[test]
    fn concurrency_non_deploy_group_does_not_serialize() {
        let mut s = SchedulerState::new();
        s.enqueue(job("p1", Lane::Interactive, 0).with_concurrency_group("pr:web:42"));
        let c1 = s.claim(&req("fr-par", "r1")).expect("first claims");
        s.mark_running("tenantA", &c1.job_id);
        s.enqueue(job("p2", Lane::Interactive, 1).with_concurrency_group("pr:web:99"));
        assert!(
            s.claim(&req("fr-par", "r2")).is_some(),
            "a non-deploy group does not serialize"
        );
    }

    #[test]
    fn concurrency_cancel_superseded_keeps_latest_head() {
        let mut s = SchedulerState::new();
        s.enqueue(job("head1", Lane::Interactive, 0).with_concurrency_group("pr:web:42"));
        let out = s.enqueue_superseding(
            job("head2", Lane::Interactive, 1).with_concurrency_group("pr:web:42"),
        );
        assert_eq!(out, EnqueueOutcome::Inserted);
        assert_eq!(
            s.state_of("tenantA", "head1"),
            Some(JobState::Terminal),
            "the prior head is cancelled (cancel-superseded)"
        );
        let c = s
            .claim(&req("fr-par", "r1"))
            .expect("the latest head claims");
        assert_eq!(c.job_id, "head2", "only the latest PR head is tested");
        assert!(
            s.claim(&req("fr-par", "r2")).is_none(),
            "no other head remains schedulable"
        );
    }

    #[test]
    fn cancel_superseded_cancels_a_leased_prior_head() {
        let mut s = SchedulerState::new();
        s.enqueue(job("h1", Lane::Interactive, 0).with_concurrency_group("pr:web:7"));
        let c = s.claim(&req("fr-par", "r1")).expect("h1 leases");
        assert_eq!(c.job_id, "h1");
        s.enqueue_superseding(job("h2", Lane::Interactive, 1).with_concurrency_group("pr:web:7"));
        assert_eq!(
            s.state_of("tenantA", "h1"),
            Some(JobState::Terminal),
            "a leased prior head is also cancelled by a new push"
        );
    }

    #[test]
    fn reaper_recovery_within_lease_ttl_zero_orphans_zero_dup_enqueue() {
        let mut s = SchedulerState::new();
        s.enqueue(job("j1", Lane::Batch, 0));
        let total_before = s.jobs().len();

        let c = s
            .claim(&req("fr-par", "dead-runner"))
            .expect("the job is claimed");
        assert_eq!(c.job_id, "j1");
        assert_eq!(s.state_of("tenantA", "j1"), Some(JobState::Leased));

        s.advance(31);

        let reaped = s.reap();
        assert_eq!(
            reaped,
            vec![("tenantA".into(), "j1".into())],
            "the dead lease is reaped"
        );
        assert_eq!(
            s.state_of("tenantA", "j1"),
            Some(JobState::Queued),
            "the reaped job is re-queued (claimable again) - 0 orphans"
        );

        assert_eq!(
            s.jobs().len(),
            total_before,
            "the reaper inserts no new row"
        );
        let retry = s.enqueue(job("j1", Lane::Batch, 0));
        assert_eq!(
            retry,
            EnqueueOutcome::DuplicateIdem,
            "the re-dispatch is idempotent on idem_token - ONE enqueue row, never a duplicate"
        );
        assert_eq!(
            s.jobs().len(),
            total_before,
            "still ONE row after the idempotent retry"
        );

        let c2 = s
            .claim(&req("fr-par", "live-runner"))
            .expect("the re-queued job re-claims");
        assert_eq!(c2.job_id, "j1", "a live runner picks up the recovered job");
    }

    #[test]
    fn heartbeat_keeps_a_live_lease_off_the_reaper() {
        let mut s = SchedulerState::new();
        s.enqueue(job("j1", Lane::Batch, 0));
        s.claim(&req("fr-par", "live")).expect("claimed");
        s.advance(20);
        assert!(
            s.heartbeat("tenantA", "j1", "live", 30),
            "the live runner extends its lease"
        );
        s.advance(11);
        let reaped = s.reap();
        assert!(
            reaped.is_empty(),
            "a heart-beating runner's lease is NOT reaped (0 swept)"
        );
        assert_eq!(
            s.state_of("tenantA", "j1"),
            Some(JobState::Leased),
            "the live job stays leased"
        );
    }

    #[test]
    fn reaper_is_idempotent_across_sweeps() {
        let mut s = SchedulerState::new();
        s.enqueue(job("j1", Lane::Batch, 0));
        s.claim(&req("fr-par", "dead")).expect("claimed");
        s.advance(31);
        assert_eq!(s.reap().len(), 1, "first sweep re-queues the dead lease");
        assert!(
            s.reap().is_empty(),
            "a second sweep finds nothing (idempotent)"
        );
    }

    #[test]
    fn the_live_claim_sql_matches_the_model_predicates() {
        assert!(
            CLAIM_QUERY.contains("FOR UPDATE OF q SKIP LOCKED"),
            "the claim is non-blocking + locks only the job_queue row (not the read-only fairness join)"
        );
        assert!(
            CLAIM_QUERY.contains("q.region = $1"),
            "RESIDENCY: in-region only"
        );
        assert!(
            CLAIM_QUERY.contains("q.labels <@ $2"),
            "AFFINITY: labels ⊆ runner_labels"
        );
        assert!(
            CLAIM_QUERY.contains("q.trust_tier = ANY($3)"),
            "TRUST: trust_tier ∈ runner_allowed_tiers"
        );
        assert!(
            CLAIM_QUERY.contains("LIKE 'deploy:%'") && CLAIM_QUERY.contains("NOT EXISTS"),
            "CONCURRENCY: the deploy:% serialize NOT EXISTS"
        );
        assert!(
            CLAIM_QUERY.contains("WHEN 'interactive' THEN 2")
                && CLAIM_QUERY.contains("COALESCE(f.deficit, 0) DESC")
                && CLAIM_QUERY.contains("q.enqueued_at ASC"),
            "ORDER BY lane DESC, deficit DESC, enqueued_at ASC"
        );
        assert!(
            CLAIM_QUERY.contains("SET state = 'leased'"),
            "on claim → leased"
        );
        for active_owner in [
            "w.state IN ('running', 'waiting')",
            "c.state = 'running'",
            "c.wf_run_id = q.run_id",
        ] {
            assert!(
                CLAIM_QUERY.contains(active_owner),
                "claim refuses a queue row without active owner predicate `{active_owner}`"
            );
        }
        assert!(
            REAP_QUERY.contains("SET state = 'queued'")
                && REAP_QUERY.contains("lease_expires < now()")
                && REAP_QUERY.contains("FOR UPDATE SKIP LOCKED")
                && REAP_QUERY.contains("pg_try_advisory_xact_lock")
                && REAP_QUERY.contains("w.state IN ('running', 'waiting')")
                && REAP_QUERY.contains("c.state = 'running'")
                && REAP_QUERY.contains("c.wf_run_id = job_queue.run_id")
                && REAP_QUERY
                    .trim_start()
                    .starts_with("WITH candidates AS MATERIALIZED"),
            "the reaper UPDATEs an expired lease in place (no INSERT)"
        );
        assert!(
            CANCEL_SUPERSEDED_QUERY.contains("SET state = 'terminal'")
                && CANCEL_SUPERSEDED_QUERY.contains("job_id <> $4"),
            "cancel-superseded terminalises prior heads, keeps the new head"
        );
        assert_eq!(lane_token(Lane::Interactive), "interactive");
        assert_eq!(state_token(JobState::Leased), "leased");
    }

    #[test]
    fn the_claim_sizes_the_execution_lease_and_the_claim_ceiling_separately() {
        assert!(
            CLAIM_QUERY
                .contains("lease_expires = statement_timestamp() + ($5 || ' seconds')::interval"),
            "the execution lease is still one execution slot from $5"
        );
        assert!(
            CLAIM_QUERY.contains("COALESCE(j.claim_window_secs::text, $5)"),
            "the immutable claim ceiling comes from the durable window, legacy-NULL back to $5"
        );
        assert!(
            CLAIM_QUERY.contains("j.claim_window_secs,"),
            "the claim returns the durable window so the resolver can refuse a legacy checkout row"
        );
    }

    #[test]
    fn the_reaper_returns_the_exact_generation_it_requeued() {
        assert!(REAP_QUERY
            .contains("SELECT tenant_id, job_id, lease_epoch, claim_nonce\n  FROM candidates"));
        assert!(REAP_QUERY.contains("e.lease_epoch AS reaped_lease_epoch"));
        assert!(REAP_QUERY.contains("e.claim_nonce::text AS reaped_claim_nonce"));
    }

    #[test]
    fn every_installed_lease_is_capped_at_the_immutable_claim_expiry() {
        for (name, query) in [
            ("launch", AUTHORIZE_JOB_LAUNCH_QUERY),
            ("preparation renewal", RENEW_PREPARATION_LEASE_QUERY),
        ] {
            assert!(
                query.contains("LEAST(") && query.contains("claim_expires_at"),
                "the {name} lease must be capped at the immutable claim expiry"
            );
        }
        const FROZEN_LAUNCH_PREDICATES: &str = "\
WHERE tenant_id = $1
  AND region = $2
  AND job_id = $3::uuid
  AND run_id = $4::uuid
  AND state = 'leased'
  AND lease_owner = $5
  AND lease_epoch = $6
  AND claim_nonce = $7::uuid
  AND FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint = $8
  AND FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint = $9
  AND claim_expires_at > statement_timestamp()
  AND completion_receipt IS NULL
RETURNING tenant_id, region, job_id
)
UPDATE ci_job AS surface
SET state = 'running'
FROM launched
WHERE surface.tenant_id = launched.tenant_id
  AND surface.region = launched.region
  AND surface.job_id = launched.job_id
  AND surface.state IN ('queued', 'leased')
RETURNING surface.job_id";
        let predicates_start = AUTHORIZE_JOB_LAUNCH_QUERY
            .find("WHERE tenant_id = $1")
            .expect("the launch CAS opens its predicate block with the tenant bind");
        assert_eq!(
            &AUTHORIZE_JOB_LAUNCH_QUERY[predicates_start..],
            FROZEN_LAUNCH_PREDICATES,
            "the launch CAS admits EXACTLY what it admitted before this slice: only the lease \
             assignment above the WHERE may change"
        );
    }

    #[test]
    fn the_preparation_renewal_binds_the_full_generation_and_refuses_a_launched_job() {
        for predicate in [
            "q.tenant_id = $1",
            "q.region = $2",
            "q.job_id = $3::uuid",
            "q.run_id = $4::uuid",
            "q.state = 'leased'",
            "q.lease_owner = $5",
            "q.lease_epoch = $6",
            "q.claim_nonce = $7::uuid",
            "FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $8",
            "FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $9",
            "q.claim_expires_at > statement_timestamp()",
            "q.completion_receipt IS NULL",
            "FROM ci_job_parent_attempt AS parent",
            "parent.claim_started_at_epoch_secs = $8",
            "parent.claim_expires_at_epoch_secs = $9",
            "surface.state IN ('queued', 'leased')",
        ] {
            assert!(
                RENEW_PREPARATION_LEASE_QUERY.contains(predicate),
                "the preparation renewal must bind `{predicate}`"
            );
        }
        assert!(!RENEW_PREPARATION_LEASE_QUERY.contains("q.state = 'running'"));
        assert!(!RENEW_PREPARATION_LEASE_QUERY.contains("q.state IN"));
        assert!(
            !RENEW_PREPARATION_LEASE_QUERY.contains("SET state"),
            "a renewal is a lease extension only, never a lifecycle transition"
        );
        assert!(HEARTBEAT_QUERY.contains("state IN ('leased', 'running')"));
    }

    #[test]
    fn the_v2_launch_cas_folds_the_current_workload_generation_into_the_same_statement() {
        for predicate in [
            "SET state = 'running'",
            "WHERE tenant_id = $1",
            "AND region = $2",
            "AND job_id = $3::uuid",
            "AND run_id = $4::uuid",
            "AND state = 'leased'",
            "AND lease_owner = $5",
            "AND lease_epoch = $6",
            "AND claim_nonce = $7::uuid",
            "AND FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint = $8",
            "AND FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint = $9",
            "AND claim_expires_at > statement_timestamp()",
            "AND completion_receipt IS NULL",
            "UPDATE ci_job AS surface",
            "AND surface.state IN ('queued', 'leased')",
        ] {
            assert!(
                AUTHORIZE_JOB_LAUNCH_V2_QUERY.contains(predicate),
                "the V2 launch CAS must still bind `{predicate}`"
            );
        }
        assert!(
            AUTHORIZE_JOB_LAUNCH_V2_QUERY.contains("AND lease_expires > statement_timestamp()"),
            "the V2 launch CAS requires a LIVE execution lease, like every other V2 boundary"
        );
        assert!(
            !AUTHORIZE_JOB_LAUNCH_QUERY.contains("lease_expires > statement_timestamp()"),
            "the legacy launch CAS stays byte-frozen; the lease predicate is V2-only"
        );
        for predicate in [
            "FROM ci_job_credential_generation AS generation",
            "generation.purpose = 'workload'",
            "generation.binding_version = $11",
            "generation.generation_id = $12",
            "generation.jti = $13",
            "generation.issued_at_epoch_secs = $14",
            "generation.expires_at_epoch_secs = $15",
            "generation.ci_run_id = $16::uuid",
            "generation.token_authority_handle = $17",
            "generation.idem_token = $18",
            "FROM ci_job_spec AS launch",
            "(launch.spec #>> '{spec,workspace,commit}') IS NOT DISTINCT FROM $19::text",
            "generation.claim_started_at_epoch_secs = $8",
            "generation.claim_expires_at_epoch_secs = $9",
            "successor.phase_ordinal > generation.phase_ordinal",
            "FROM ci_job_parent_attempt AS parent",
            "unresolved.status <> 'measured'",
        ] {
            assert!(
                AUTHORIZE_JOB_LAUNCH_V2_QUERY.contains(predicate),
                "the V2 launch CAS must bind `{predicate}`"
            );
        }
        let generation_predicate_position = AUTHORIZE_JOB_LAUNCH_V2_QUERY
            .find("FROM ci_job_credential_generation AS generation")
            .expect("the generation predicate exists");
        let cte_close = AUTHORIZE_JOB_LAUNCH_V2_QUERY
            .find("RETURNING tenant_id, region, job_id")
            .expect("the launching CTE closes");
        assert!(
            generation_predicate_position < cte_close,
            "the generation predicate must live INSIDE the launching UPDATE, not after it"
        );
        assert!(
            !AUTHORIZE_JOB_LAUNCH_QUERY.contains("ci_job_credential_generation"),
            "the production launch CAS stays byte-unchanged: production is still V1-pinned"
        );
        assert!(
            !VERIFY_JOB_LAUNCH_LIVE_QUERY.contains("ci_job_credential_generation")
                && !RENEW_PREPARATION_LEASE_QUERY.contains("ci_job_credential_generation")
                && !CONSUME_CLAIM_QUERY.contains("ci_job_credential_generation")
                && !CONSUME_PREPARATION_CLAIM_QUERY.contains("ci_job_credential_generation"),
            "no previously shipped query gains a credential-generation dependency in this slice"
        );
        assert!(
            !CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY.contains("ci_job_credential_generation"),
            "the exhausted-variant preparation CAS must not depend on credential generations"
        );
    }

    #[test]
    fn the_exhausted_cas_shares_the_shipped_predicate_block_byte_for_byte() {
        const PARENT_MARKER: &str =
            "  AND EXISTS (\n    SELECT 1\n    FROM ci_job_parent_attempt AS parent";
        const SURFACE_MARKER: &str = "  AND EXISTS (\n    SELECT 1\n    FROM ci_job AS surface";

        let shipped_parent = CONSUME_PREPARATION_CLAIM_QUERY
            .find(PARENT_MARKER)
            .expect("the shipped CAS opens a parent-attempt EXISTS block");
        let exhausted_parent = CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY
            .find(PARENT_MARKER)
            .expect("the exhausted CAS opens a parent-attempt EXISTS block");
        assert_eq!(
            &CONSUME_PREPARATION_CLAIM_QUERY[..shipped_parent],
            &CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY[..exhausted_parent],
            "the exhausted CAS must share the shipped queue-identity/live-claim block byte-for-byte"
        );

        let shipped_surface = CONSUME_PREPARATION_CLAIM_QUERY
            .find(SURFACE_MARKER)
            .expect("the shipped CAS has a ci_job surface guard");
        let exhausted_surface = CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY
            .find(SURFACE_MARKER)
            .expect("the exhausted CAS has a ci_job surface guard");
        assert_eq!(
            &CONSUME_PREPARATION_CLAIM_QUERY[shipped_surface..],
            &CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY[exhausted_surface..],
            "the exhausted CAS must share the shipped ci_job surface guard + RETURNING byte-for-byte"
        );

        let shipped_parent_block = &CONSUME_PREPARATION_CLAIM_QUERY[shipped_parent..shipped_surface];
        let exhausted_parent_block =
            &CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY[exhausted_parent..exhausted_surface];
        assert!(
            shipped_parent_block.contains("parent.claim_nonce = q.claim_nonce"),
            "the shipped CAS binds the exact current generation's parent row"
        );
        assert!(!shipped_parent_block.contains("GROUP BY"));
        assert!(
            exhausted_parent_block
                .contains("GROUP BY parent.budget_revision, parent.max_parent_attempts")
                && exhausted_parent_block.contains("HAVING count(*) = parent.max_parent_attempts"),
            "the exhausted CAS recovers on the policy-exhaustion group"
        );
        assert!(
            !exhausted_parent_block.contains("q.claim_nonce"),
            "the exhausted parent predicate must NOT bind the current generation (that is the row \
             that does not exist for a refused generation)"
        );
    }

    #[test]
    fn the_requeue_cas_shares_the_shipped_where_block_byte_for_byte() {
        const WHERE_MARKER: &str = "WHERE q.tenant_id = $1";
        let shipped_where = CONSUME_PREPARATION_CLAIM_QUERY
            .find(WHERE_MARKER)
            .expect("the shipped preparation CAS opens its WHERE block");
        let requeue_where = REQUEUE_PREPARATION_CLAIM_QUERY
            .find(WHERE_MARKER)
            .expect("the requeue CAS opens its WHERE block");
        assert_eq!(
            &CONSUME_PREPARATION_CLAIM_QUERY[shipped_where..],
            &REQUEUE_PREPARATION_CLAIM_QUERY[requeue_where..],
            "the requeue CAS must admit EXACTLY what the shipped preparation CAS admits"
        );
        let requeue_set = &REQUEUE_PREPARATION_CLAIM_QUERY[..requeue_where];
        assert_eq!(
            requeue_set,
            "UPDATE job_queue AS q\n\
             SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL\n",
            "the requeue action releases the lease and clears the nonce, in full"
        );
        assert!(!REQUEUE_PREPARATION_CLAIM_QUERY.contains("completion_receipt = $14"));
        assert!(
            !REQUEUE_PREPARATION_CLAIM_QUERY.contains("retry_attempts"),
            "a preparation requeue NEVER touches the workload retry counter"
        );
    }
}
