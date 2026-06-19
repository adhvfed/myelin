# CI/CD — 01 Technology, Job Spec & Data Model

> Phase 5-B — CI detailed architecture (rewritten against the reconciled layer). The language/tools/DB
> choice with written justification (Rust default; any divergence justified), the runtime-agnostic **job
> spec** (the ADR-20 / X-6 seam), the **data model/schema** for every CI store including the **CheckStatus
> source columns** (X-1) and the **per-subject log DEK** (Storage C1), and the residency/encryption posture.
> Schema is illustrative-Rust/SQL-shaped (no code beyond schema/interface snippets).

---

## 1. Language / tools / database — and the written justification

### 1.1 Rust throughout, with **zero justified divergence** (carried forward, re-confirmed)

ADR-02 makes Rust the default and Phase-2 §3 already recorded "no divergence justified" for CI.
Reconciliation forced **no** language change (recon §0: "your Phase-4 language/DB choice stands"). Every CI
component is **either a latency-/correctness-critical hot path or a trust-boundary surface** — the two places
ADR-02 names Rust as load-bearing:

| Component | Why Rust (not a divergence) |
|---|---|
| **Scheduler / state machine** | The platform's heaviest scheduling hot path (claim-query at QPS, lease/heartbeat/reaper correctness). Latency + memory-safety + the shared `FOR UPDATE SKIP LOCKED` primitive all argue Rust. |
| **Runner agent** | A single small **attested binary at the trust boundary**, hosted + self-hosted from one artifact. Memory-safety here is security-load-bearing — a runner-agent overflow is a host compromise. |
| **Sandbox backend control** | Firecracker has a first-class Rust API (it *is* Rust); Cloud Hypervisor likewise. The VMM-control path is exactly where memory-safety matters most. |
| **Check emitter** | Stamps `CheckStatus` (state, `run_attempt`, `trust_tier`) into the outbox in the same tx as the run state change; consumes the Rust outbox glue directly. |
| **Trigger & dispatch, log-pipeline coordinator, secret broker, supply-chain verifier** | Contract surfaces (outbox emit, firehose publish + resume-cursor, KMS, sigstore verify) — they consume the Rust glue crates directly; a language boundary here would re-implement the outbox/consumer template. |
| **Workflow definitions** | The `ci.pipeline`/`ci.deploy` functions are registered on the Rust `myelin-flow` engine and guarded by the `flow-determinism` lint — they must be Rust. |

**The only "divergence" is by *constraint*, not by language:** ADR-11 forbids hyperscaler autoscaling, so CI
**builds** the fleet autoscaler on EU infra rather than renting one (02 §5). That is an extra component, not
a non-Rust component.

**Glue-contract implementability across a language boundary.** Even though CI is all-Rust, the self-hosted
runner and any future non-Rust step container interact only over the wire (the job spec as a protobuf/JSON
contract, logs as firehose frames, results as the `job.done` signal payload), so a future non-Rust runner or
backend remains implementable against the identical shapes (the cross-language harness parity note, contract
1.7). **EU-deployable / self-hostable** holds: Firecracker + Cloud Hypervisor + Postgres + the EU
`FleetProvider` adapters are all open and EU-runnable; nothing in CI requires a hyperscaler-proprietary
primitive.

### 1.2 Database & storage tiers (delegated to Storage; CI is a disciplined consumer)

CI owns **no novel storage** — it composes the frozen Storage tiers (contract group 11):

| CI data | Tier | Notes |
|---|---|---|
| Run/job/step state, `job_queue`, leases, runners, deployments, environments, the **check-attempt counter**, the log **range index**, artifact/cache **indices**, the dedup ledger, the cost-event log | **OLTP** (one Postgres per service, RLS, `(tenant,region)` first key) — contract 11.1 | The scheduler's hot tables; `FOR UPDATE SKIP LOCKED` claim. Free-text columns keyed per-subject. |
| Log **segment bytes** | **T3 log tier** (append-mostly object-backed segments; Storage §3.3, **frozen `(job,step,byte-range)` index** 11.8) | Sealed segments → T2 content-addressed blobs; **per-subject DEK for isolable inline PII** (Storage C1 / 11.4), per-tenant DEK otherwise. |
| **Artifacts + caches** | **T2 `BlobStore`** (BLAKE3, per-tenant dedup; Storage §3.2) with **trust-tier/branch-scoped cache namespaces** (Storage C4 / 11.2) | Residency-pinned; crypto-shred-capable. A fork write cannot reach the trusted cache scope (structural). |
| Cross-repo "release readiness" / usage rollups | **OLAP read store** (CQRS, bus-fed, reindex-from-source; Storage §3.4) — honours the **restriction flag** (11.6) | Read-only; never a second write path; no analytics for a restricted subject. |
| The pipeline-run **journal** | `myelin-flow`'s Postgres (Workflow §3) | CI does not own it; it owns the workflow *definition*. The `SCHEDULE_AND_RUN_JOB` dispatch + `job.done` wait live in the journal. |

The pipeline definition **snapshot** (the resolved, digest-pinned config) is itself a content-addressed
**T2 blob** referenced by the run, so a run is reproducible down to which bytes it ran (05 §HP-3/HP-4).

## 2. The job spec — the runtime-agnostic seam (ADR-20 / X-6 / CI-1)

`JobSpec` is the **one struct, two kinds** seam. `SandboxBackend` hides Firecracker vs gVisor vs
self-hosted; `FleetProvider` hides the EU provider. These three traits are what the whole subsystem is built
on. `ToolHands::exec(Command)` (Agent contract 8.4) **is** `launch(JobSpec{ kind: Agent, .. })` — the same
runner, the same hardening, the same drill, inheriting the **four uniform guarantees** (X-6: cost gate,
per-run-token attribution, HITL withhold, isolation floor; 02 §4/§5).

```rust
pub struct JobSpec {
    pub kind: JobKind,                 // Ci | Agent — the UNIFY point (TE-31 = UNIFY; X-6)
    pub image: ImageRef,               // MUST be digest-pinned; an un-digested tag is rejected (fail-closed, CI-1)
    pub command: Vec<String>,
    pub env: Vec<EnvVar>,              // secrets are NAMES here, resolved inside the boundary (CI-1)
    pub secret_refs: Vec<SecretRef>,   // resolved by the in-boundary broker, scoped to THIS job only
    pub egress: EgressPolicy,          // default-deny; allowlist opt-in; metadata/control-plane/cross-tenant always blocked
    pub limits: ResourceLimits,        // cpu, mem, disk, pids_max, timeout, zero-swap
    pub workspace: WorkspaceSpec,      // checkout via the scoped job-token git wire; read-only root + tmpfs scratch
    pub trust_tier: TrustTier,         // Trusted | UntrustedFork | SelfHosted — gates secrets/cache-scope/egress;
                                       //   the SAME value CI stamps onto CheckStatus.trust_tier (X-1)
    pub run_token: RunTokenRef,        // the per-job attenuated token (Id::mint_run_token, contract 4.7)
    pub meter_to: MeterTarget,         // the reserve this job settles against (run-level / agent-run-level)
    pub idem_token: IdemToken,         // minted by the workflow at SCHEDULE_AND_RUN_JOB dispatch (OQ-F);
                                       //   the runner stamps it on the job.done signal — producer/consumer agree, no round-trip
}
pub enum JobKind { Ci, Agent }
pub enum TrustTier { Trusted, UntrustedFork, SelfHosted }

pub trait SandboxBackend {             // Firecracker (default) | Gvisor (named 2nd) | SelfHosted (delegated)
    fn launch(&self, spec: &JobSpec, hooks: &RunnerHooks) -> Result<SandboxHandle>;
    fn kill(&self, h: &SandboxHandle) -> Result<()>;            // whole-guest kill on teardown
}

pub trait FleetProvider {              // Hetzner | OVH | Scaleway | BareMetal-PXE | K8s(customer) | SelfHosted
    fn provision(&self, class: RunnerClass, n: u32, region: Region) -> Result<Vec<RunnerHost>>;
    fn deprovision(&self, hosts: &[RunnerHost]) -> Result<()>;
    fn capacity(&self, region: Region) -> Result<Capacity>;
}
```

**The `trust_tier` is one value, stamped once.** CI's Trigger & Dispatch evaluates the tier from run
provenance (member push vs fork PR vs self-hosted target) and the ReBAC ABAC edge `read & !is_untrusted_fork`
(contract 4.9). The **same** value gates secrets/cache-scope/egress on the `JobSpec` **and** is stamped onto
the `CheckStatus.trust_tier` Git reads (X-1). Git never recomputes trust; it reads the fact.

## 3. The data model (per-service, `(tenant, region)`-first, RLS)

Every table carries `tenant uuid` + `region text` as its leading columns (the partition key, contract 12.1;
the `residency-pin` lint asserts `row.region == cell.region` on write). Every personal-data field carries a
`#[personal_data(...)]` tag (the `no-untagged-personal-data` lint). **Identity is stored as a *reference*
(pseudonym), never copied PII** — `triggered_by`, `approved_by`, commit-author are pseudonym subjects
resolvable through Id (`resolve_pseudonym`/`erase`, contract 4.8), erasable there.

### 3.1 Run / job / step (the run-state model)

```sql
-- The run is a thin index over the myelin-flow workflow (the journal is the workflow's; this is CI's view).
CREATE TABLE ci_run (
  tenant uuid, region text, run_id uuid,
  project_id     uuid NOT NULL,
  repo_ref       text,                          -- ArtifactRef of the triggering repo (reference, not data)
  commit_oid     text,                          -- the content-addressed commit the run ran against (CheckStatus key half)
  pipeline_id    uuid NOT NULL,
  wf_run_id      uuid NOT NULL,                 -- the myelin-flow durable run (the lifecycle owner)
  cause_event_id text,                          -- the triggering event_id (causation; dedup anchor)
  definition_snapshot blob_ref NOT NULL,        -- content-addressed (CAS) resolved config — reproducibility/audit
  trigger_kind   text NOT NULL,                 -- push | pull_request | issue_transition | manual | agent | schedule
  triggered_by   text,  -- #[personal_data(actor, role=controller, erasure=pseudonym_shred)]  pseudonym subject
  trust_tier     text NOT NULL,                 -- trusted | untrusted_fork | self_hosted  (stamped onto every CheckStatus)
  state          text NOT NULL,                 -- queued | running | succeeded | failed | cancelled | timed_out | reaped
  cost_settled   bool NOT NULL DEFAULT false,   -- reserve/settle bookend closed (11.7) — a check is not "final" until settled (X-1)
  correlation_id text NOT NULL,                 -- threads the whole causal chain (agent triage, etc.)
  created_at timestamptz, finished_at timestamptz,
  PRIMARY KEY (tenant, run_id)
);

CREATE TABLE ci_job (
  tenant uuid, region text, job_id uuid, run_id uuid,
  stage          text NOT NULL,
  name           text NOT NULL,
  needs          uuid[] NOT NULL,               -- DAG edges (job dependencies)
  matrix_key     jsonb,                          -- the resolved matrix cell (deterministic fan-out)
  spec_ref       blob_ref NOT NULL,             -- CAS of this job's resolved JobSpec
  state          text NOT NULL,                 -- queued | leased | running | succeeded | failed | cancelled | reaped
  attempt        int NOT NULL DEFAULT 1,        -- activity-retry attempt (idempotent on idem_token)
  result_summary jsonb,                          -- structured terminal report (for the agent-native triage hook)
  PRIMARY KEY (tenant, job_id),
  FOREIGN KEY (tenant, run_id) REFERENCES ci_run(tenant, run_id)
);
-- Steps are NOT a journaled row each (firehose volume). Step boundaries live in the log range index (§3.4);
-- step structure is recovered by re-running the job on retry, not persisted per-step.
```

### 3.2 The check-status source (the X-1 CheckStatus producer side — NEW)

CI is the **producer** of the frozen `CheckStatus` fact; Git owns the projection table + the merge gate. CI
holds the **monotonic attempt counter** per `(commit_oid, context)` so re-run supersession is on `run_attempt`,
never on wall-clock (clocks are not authority — X-1).

```sql
-- The per-(commit_oid, context) attempt counter. CI's source of run_attempt for the CheckStatus fact.
CREATE TABLE check_attempt (
  tenant uuid, region text,
  repo_ref     text NOT NULL,
  commit_oid   text NOT NULL,
  context      text NOT NULL,                     -- "{provider}:{name}", e.g. "ci:build", "ci:test/unit"
  next_attempt int  NOT NULL DEFAULT 1,           -- bumped on each (re-)dispatch of this context; the supersession key
  current_run  uuid,                              -- the run that most recently produced this context's status
  PRIMARY KEY (tenant, repo_ref, commit_oid, context)
);
-- On a new run/re-run for (commit_oid, context): UPDATE ... next_attempt = next_attempt + 1 RETURNING (attempt-1);
-- the returned attempt is stamped into the emitted CheckStatus.run_attempt. Higher attempt supersedes lower (Git's rule).
```

The emitted `CheckStatus` (the frozen struct, X-1) is assembled from `ci_run` + `check_attempt`:
`{ repo, commit_oid, context, state, required(?), run, run_attempt, trust_tier, details_ref (#step-<n>),
summary (HumanisedRef), started_at, completed_at?, cost_settled }`. **`required` is Git's decision, not CI's**
— CI may leave it absent or echo a hint; Git's branch-protection policy is authoritative. CI emits the fact;
it does not store a projection table (that is Git's, contract 5.9).

### 3.3 The scheduler tables (the hot path — 02 §2)

```sql
CREATE TABLE job_queue (                          -- one row per schedulable job; the claim surface
  tenant uuid, region text, job_id uuid, run_id uuid,
  lane         text NOT NULL,                      -- interactive | batch | deploy  (priority lanes)
  labels       text[] NOT NULL,                    -- affinity: gpu, arm64, large, linux ...
  trust_tier   text NOT NULL,                      -- gates which runner pools may claim
  concurrency_group text,                          -- "deploy:prod" serialize | "pr:web:42" cancel-superseded
  fair_key     text NOT NULL,                       -- = tenant (or tenant:project) — the DRR fairness bucket
  idem_token   text NOT NULL,                       -- = the workflow's SCHEDULE_AND_RUN_JOB idem_token (OQ-F); enqueue is idempotent on it
  enqueued_at  timestamptz NOT NULL,
  lease_owner  text, lease_expires timestamptz,    -- the lease (runner identity + TTL)
  state        text NOT NULL,                       -- queued | leased | running | terminal
  PRIMARY KEY (tenant, job_id)
);
CREATE INDEX jq_claimable ON job_queue (region, lane, enqueued_at) WHERE state='queued';
CREATE UNIQUE INDEX jq_serialize ON job_queue (concurrency_group) WHERE state='running' AND concurrency_group LIKE 'deploy:%';
CREATE UNIQUE INDEX jq_idem ON job_queue (tenant, idem_token);    -- enqueue dedup (reaper re-dispatch is one row)

-- DRR fairness accounting: per fair_key deficit counter, advanced at claim time (02 §2).
CREATE TABLE fair_deficit (
  tenant uuid, region text, fair_key text,
  deficit bigint NOT NULL, last_served timestamptz NOT NULL,
  PRIMARY KEY (tenant, region, fair_key)
);
```

### 3.4 Runners & the fleet

```sql
CREATE TABLE runner (
  tenant uuid, region text, runner_id uuid,
  pool          text NOT NULL,                     -- eu-west, gpu-pool ...
  labels        text[] NOT NULL,
  ownership     text NOT NULL,                     -- hosted | self_hosted
  trust_tier    text NOT NULL,                     -- hosted runners claim trusted/untrusted; self-hosted only its own tenant's SelfHosted jobs
  attestation   jsonb,                              -- TPM quote / provisioning-signed token (self-hosted)
  attest_state  text NOT NULL,                      -- pending | attested | failed
  health        text NOT NULL,                      -- healthy | degraded | offline
  capacity      jsonb NOT NULL,                      -- slots, cpu/mem
  last_heartbeat timestamptz NOT NULL,
  PRIMARY KEY (tenant, runner_id)
);
-- Hosted runners are tenant-agnostic provisioned capacity; "tenant" on a hosted runner row is the cell-owner.
```

### 3.5 The log range index (the firehose archive seam — Storage 11.8, frozen)

```sql
-- Logs are NOT rows. Bytes live as T3 segments → T2 blobs. This index makes (job, step, byte-range) addressable.
CREATE TABLE log_segment (
  tenant uuid, region text, run_id uuid, job_id uuid,
  segment_seq  int NOT NULL,
  blob_ref     blob_ref,                            -- content-addressed sealed segment (T2); NULL while open (in firehose)
  byte_start   bigint NOT NULL, byte_end bigint NOT NULL,
  pii_key_ref  text NOT NULL,                       -- kms://<tenant>/<dek-epoch>/<class> — per-tenant OR per-subject (Storage C1)
  PRIMARY KEY (tenant, run_id, job_id, segment_seq)
);
CREATE TABLE log_anchor (                            -- (job, step) → byte offset, for collapsible-per-step + jump-to-failure
  tenant uuid, region text, run_id uuid, job_id uuid,
  step_id   text NOT NULL,                           -- stable across retries (the #step-<n> sub-anchor; resolves CheckStatus.details_ref)
  byte_start bigint NOT NULL, byte_end bigint,
  status    text NOT NULL,                            -- running | passed | failed | skipped
  PRIMARY KEY (tenant, run_id, job_id, step_id)
);
-- This is the frozen Storage 11.8 (job, step, byte-range) index. The CheckStatus.details_ref = myelin://.../ci/run/<id>#step-<n>
-- resolves through log_anchor → log_segment → the byte range (the X-1 / OQ-D jump-to-failure path).
```

**Per-subject DEK for inline log PII (Storage C1 / 11.4, now built).** Where a subject's inline PII in a log
segment is **isolable** (e.g. a redaction-tagged span, a structured field), that segment's `pii_key_ref`
names a **per-subject** DEK (`<class> = subject:<id>`); erasing the subject crypto-shreds exactly their
reachable log content **including backups** without touching the run's other bytes. Where it is not isolable,
the segment falls back to the per-tenant DEK. The *residual* (third-party PII typed into someone else's log
line, encrypted under no isolable subject key) is handled by reference per the **one platform erasure
posture** (X-7, 06 §erasure) — CI does not author a CI-local residual statement.

### 3.6 Artifacts & caches (trust-scoped — Storage C4)

```sql
CREATE TABLE artifact (                              -- retained job output; ArtifactRef-addressable
  tenant uuid, region text, artifact_id uuid, run_id uuid,
  name        text NOT NULL,
  blob_ref    blob_ref NOT NULL,                     -- T2 content-addressed
  size_bytes  bigint NOT NULL,
  provenance  jsonb,                                  -- SLSA attestation ref + SBOM ref
  pii_key_ref text NOT NULL,                          -- per-tenant DEK (or per-subject where isolable)
  retain_until timestamptz NOT NULL,                  -- explicit TTL (Art. 5 storage-limitation) → GC
  PRIMARY KEY (tenant, artifact_id)
);
CREATE TABLE cache_entry (                            -- reconstructible perf optimization; TRUST-SCOPED (Storage C4)
  tenant uuid, region text, run_id uuid,
  cache_key   text NOT NULL,                          -- = hash(lockfile + os + toolchain + ...) — the subtle part
  scope       text NOT NULL,                          -- the trust-tier/branch scope key (Storage C4 namespace):
                                                      --   an UntrustedFork write CANNOT reach the trusted (default-branch) scope
  blob_ref    blob_ref NOT NULL,
  last_used   timestamptz NOT NULL,                   -- LRU eviction
  PRIMARY KEY (tenant, scope, cache_key)
);
-- The scope key is derived from the run's trust_tier (which CI stamps); Storage enforces the write-scope rule structurally
-- (the poisoned-cache defence, the storage half of the X-1 trust-tier story).
```

### 3.7 Deployments, environments, secrets, cost

```sql
CREATE TABLE environment (
  tenant uuid, region text, env_id uuid, project_id uuid,
  name        text NOT NULL,                          -- prod | staging ...
  protected   bool NOT NULL,                          -- protected → HITL gate on deploy
  PRIMARY KEY (tenant, env_id)
);  -- approver set resolves via Id.list_subjects(env, approve) at gate time (contract 4.4), not stored here.
CREATE TABLE deployment (
  tenant uuid, region text, dep_id uuid, env_id uuid, run_id uuid,
  version     text NOT NULL,
  state       text NOT NULL,                          -- awaiting_approval | deploying | deployed | failed | rolled_back
  approved_by text,  -- #[personal_data(actor, role=controller, erasure=pseudonym_shred)]
  PRIMARY KEY (tenant, dep_id)
);
CREATE TABLE secret_binding (                          -- NAMES + scope only; VALUES live in the shared secret store (Id/GDPR-placed)
  tenant uuid, region text, project_id uuid, name text,
  scope       text NOT NULL,                           -- env / project; untrusted_fork resolves to NONE by default (ABAC edge)
  value_ref   text NOT NULL,                            -- a reference into the shared secret capability (never the value)
  PRIMARY KEY (tenant, project_id, name, scope)
);
CREATE TABLE cost_event (                              -- one row per metered unit (D8); wholesale & markup separate columns
  tenant uuid, region text, cost_id uuid, run_id uuid, job_id uuid,
  meter       text NOT NULL,                            -- cpu_seconds | mem_gb_seconds | gpu_seconds | storage_gb_hours | egress_gb
  amount      bigint NOT NULL,                           -- integer quantity of the meter unit (NEVER a float)
  wholesale_minor_units bigint NOT NULL,                 -- the resource cost
  markup_minor_units    bigint NOT NULL,                 -- Commercial's priced amount (immutable pricing table)
  kind        text NOT NULL,                             -- ci | agent — same schema fronts both (UNIFY / X-6)
  PRIMARY KEY (tenant, cost_id)
);
```

### 3.8 The dedup ledger (exactly-once *effect*)

```sql
CREATE TABLE consumer_dedup (                          -- the platform consumer template's ledger (contract 2.5)
  tenant uuid, region text, consumer text, event_id text,
  PRIMARY KEY (consumer, event_id)
);
-- Trigger & Dispatch dedups on the triggering event_id so one push = one run (exactly-once effect).
```

## 4. Encryption, residency & GDPR posture (the data-model invariants)

- **Per-tenant envelope encryption + crypto-shred** on every blob/log/artifact store
  (`pii_key_ref = kms://<tenant>/<dek-epoch>/<class>`, contract 11.3/11.4). **Inline log PII keyed
  per-subject where isolable** (Storage C1 / 11.4) — `<class> = subject:<id>` — so an individual's erasure
  crypto-shreds exactly their reachable log content incl. backups; per-tenant DEK is the fallback.
- **Residency by construction** — no global pool; the `residency-pin` lint + the harness
  `row.region == cell.region` assertion on every CI write; `residency_verify` attests the runner pool +
  log/artifact/cache region (07 drill R-3, contract 12.4).
- **Trust-scoped cache namespaces** (Storage C4) make "a fork cannot reach the trusted cache scope" a
  **structural** property, not a check — the storage half of the X-1 trust-tier defence.
- **Identity is referenced, never copied** — actors are pseudonym subjects; erasure flows through Id's
  `resolve_pseudonym`/`erase` (contract 4.8) + CI's crypto-shred of the incidental-PII stores (03 §6).
- **Auto-registration** — every table above is opened through `serve(AppSpec)`, so the harness
  auto-registers each as a `PersonalDataHolder` (contract 1.4); "we forgot the cache table" is structurally
  impossible.
- **The erasure residual is by reference** — third-party free-text PII follows the **one platform posture**
  (X-7, contract 10.9); CI does not restate it (06 §erasure).
