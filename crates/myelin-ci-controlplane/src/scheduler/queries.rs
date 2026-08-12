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

macro_rules! authorize_job_launch_query_with_guards {
    ($extra_guards:literal) => {
        concat!(
            "WITH launched AS (
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
  AND completion_receipt IS NULL",
            $extra_guards,
            "
RETURNING tenant_id, region, job_id
)
UPDATE ci_job AS surface
SET state = 'running'
FROM launched
WHERE surface.tenant_id = launched.tenant_id
  AND surface.region = launched.region
  AND surface.job_id = launched.job_id
  AND surface.state IN ('queued', 'leased')
RETURNING surface.job_id"
        )
    };
}

macro_rules! authorize_job_launch_query {
    (v1) => {
        authorize_job_launch_query_with_guards!("")
    };
    (v2) => {
        authorize_job_launch_query_with_guards!(
            "
  AND lease_expires > statement_timestamp()
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
  )"
        )
    };
}

pub const AUTHORIZE_JOB_LAUNCH_QUERY: &str = authorize_job_launch_query!(v1);
pub const AUTHORIZE_JOB_LAUNCH_V2_QUERY: &str = authorize_job_launch_query!(v2);

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

macro_rules! preparation_claim_mutation_with_parent_guard {
    ($mutation:literal, $parent_guard:literal) => {
        concat!(
            "UPDATE job_queue AS q
",
            $mutation,
            "
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
",
            $parent_guard,
            "
  AND EXISTS (
    SELECT 1
    FROM ci_job AS surface
    WHERE surface.tenant_id = q.tenant_id
      AND surface.region = q.region
      AND surface.job_id = q.job_id
      AND surface.state IN ('queued', 'leased')
  )
RETURNING q.job_id"
        )
    };
}

macro_rules! preparation_claim_mutation {
    ($mutation:literal, live_parent) => {
        preparation_claim_mutation_with_parent_guard!(
            $mutation,
            "  AND EXISTS (
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
  )"
        )
    };
    ($mutation:literal, missing_live_parent) => {
        preparation_claim_mutation_with_parent_guard!(
            $mutation,
            "  AND NOT EXISTS (
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
  )"
        )
    };
    ($mutation:literal, exhausted_parent_budget) => {
        preparation_claim_mutation_with_parent_guard!(
            $mutation,
            "  AND EXISTS (
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
  )"
        )
    };
}

pub const CONSUME_PREPARATION_CLAIM_QUERY: &str = preparation_claim_mutation!(
    "SET state = 'terminal', completion_receipt = $14, lease_owner = NULL, lease_expires = NULL",
    live_parent
);

pub const CONSUME_SECRET_WITHHELD_CLAIM_QUERY: &str = preparation_claim_mutation!(
    "SET state = 'terminal', completion_receipt = $14, lease_owner = NULL, lease_expires = NULL",
    missing_live_parent
);

pub const CONSUME_PREPARATION_CLAIM_EXHAUSTED_QUERY: &str = preparation_claim_mutation!(
    "SET state = 'terminal', completion_receipt = $14, lease_owner = NULL, lease_expires = NULL",
    exhausted_parent_budget
);

pub const REQUEUE_PREPARATION_CLAIM_QUERY: &str = preparation_claim_mutation!(
    "SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL",
    live_parent
);

macro_rules! preparation_completion_replay_query {
    ($parent_presence:literal) => {
        concat!(
            "SELECT 1
FROM job_queue q
WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid AND q.idem_token = $5 AND q.stage = $6
  AND q.state = 'terminal' AND q.completion_receipt = $7
  AND ",
            $parent_presence,
            "EXISTS (
    SELECT 1 FROM ci_job_parent_attempt p
    WHERE p.tenant_id = q.tenant_id AND p.region = q.region
      AND p.job_id = q.job_id AND p.wf_run_id = q.run_id
      AND p.ci_run_id = $8::uuid AND p.reserve_handle = $9
      AND p.lease_owner = $10 AND p.lease_epoch = $11
      AND p.claim_nonce = $12::uuid
      AND p.claim_started_at_epoch_secs = $13
      AND p.claim_expires_at_epoch_secs = $14
  )"
        )
    };
}

pub(crate) const READ_PREPARATION_COMPLETION_REPLAY_QUERY: &str =
    preparation_completion_replay_query!("");
pub(crate) const READ_SECRET_WITHHELD_COMPLETION_REPLAY_QUERY: &str =
    preparation_completion_replay_query!("NOT ");

pub(crate) const READ_EXHAUSTED_COMPLETION_REPLAY_QUERY: &str = "\
SELECT 1
FROM job_queue q
WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid
  AND q.run_id = $4::uuid AND q.idem_token = $5 AND q.stage = $6
  AND q.state = 'terminal' AND q.completion_receipt = $7
  AND EXISTS (
    SELECT 1 FROM ci_job_parent_attempt p
    WHERE p.tenant_id = q.tenant_id AND p.region = q.region
      AND p.job_id = q.job_id AND p.wf_run_id = q.run_id
      AND p.ci_run_id = $8::uuid AND p.reserve_handle = $9
    GROUP BY p.budget_revision, p.max_parent_attempts
    HAVING count(*) = p.max_parent_attempts
  )";

pub const RESET_REQUEUED_PREPARATION_CI_JOB_SURFACE_QUERY: &str = "\
UPDATE ci_job SET state = 'queued'
WHERE tenant_id = $1 AND job_id = $2::uuid AND state = 'leased'";

pub const READ_COMPLETION_DISPOSITION_QUERY: &str = "\
SELECT state, completion_receipt FROM job_queue WHERE tenant_id = $1 AND job_id = $2";
