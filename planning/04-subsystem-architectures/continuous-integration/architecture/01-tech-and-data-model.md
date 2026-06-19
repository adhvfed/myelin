# CI/CD — 01 Technology, Job Spec & Data Model

> Phase 4 — CI Stage-2. The language/tools/DB choice with written justification (Rust default; any
> divergence justified), the runtime-agnostic **job spec** (the ADR-20 seam), the **data model/schema**
> for every CI store, and the residency/encryption posture. Schema is illustrative-Rust/SQL-shaped (no
> code beyond schema/interface snippets, per the stage rules).

---

## 1. Language / tools / database — and the written justification

### 1.1 Rust throughout, with **zero justified divergence**

ADR-02 makes Rust the default and Phase-2 §3 already recorded "no divergence justified" for CI. Stage-2
confirms it component-by-component, because every CI component is **either a latency-/correctness-critical
hot path or a trust-boundary surface** — the two places ADR-02 names Rust as load-bearing:

| Component | Why Rust (not a divergence) |
|---|---|
| **Scheduler / state machine** | The platform's heaviest scheduling hot path (claim-query at QPS, lease/heartbeat/reaper correctness). Latency + memory-safety + the shared `FOR UPDATE SKIP LOCKED` primitive all argue Rust. |
| **Runner agent** | A single small **attested binary at the trust boundary**, hosted + self-hosted from one artifact. Memory-safety here is security-load-bearing — a runner-agent overflow is a host compromise. |
| **Sandbox backend control** | Firecracker has a first-class Rust API (it *is* Rust); Cloud Hypervisor likewise. The VMM-control path is exactly where memory-safety matters most. |
| **Trigger & dispatch, log-pipeline coordinator, secret broker, supply-chain verifier** | Contract surfaces (outbox emit, firehose publish, KMS, sigstore verify) — they consume the Rust glue crates directly; a language boundary here would re-implement the outbox/consumer template. |
| **Workflow definitions** | The `ci.pipeline`/`ci.deploy` functions are registered on the Rust `myelin-flow` engine and guarded by the `flow-determinism` lint — they must be Rust. |

**The only "divergence" is by *constraint*, not by language:** ADR-11 forbids hyperscaler autoscaling, so
CI **builds** the fleet autoscaler on EU infra rather than renting one (02 §2). That is an extra component,
not a non-Rust component.

**Glue-contract implementability across a language boundary.** Even though CI is all-Rust, the
self-hosted runner and any future non-Rust step container interact only over the wire (the job
spec as a protobuf/JSON contract, logs as firehose frames, results as the activity-completion payload),
so a future non-Rust runner or backend remains implementable against the identical shapes (the
cross-language harness parity note, contract 1.7). **EU-deployable / self-hostable** holds: Firecracker +
Cloud Hypervisor + Postgres + the EU `FleetProvider` adapters are all open and EU-runnable; nothing in CI
requires a hyperscaler-proprietary primitive.

### 1.2 Database & storage tiers (delegated to Storage; CI is a disciplined consumer)

CI owns **no novel storage** — it composes the Phase-3 tiers (contract group 11):

| CI data | Tier | Notes |
|---|---|---|
| Run/job/step state, `job_queue`, leases, runners, deployments, environments, the log **range index**, artifact/cache **indices**, the dedup ledger, the cost-event log | **OLTP** (one Postgres per service, RLS, `(tenant,region)` first key) — contract 11.1 | The scheduler's hot tables; `FOR UPDATE SKIP LOCKED` claim. |
| Log **segment bytes** | **T3 log/firehose tier** (append-mostly object-backed segments; Storage §3.3) | Sealed segments → T2 content-addressed blobs; a small OLTP range index points in. |
| **Artifacts + caches** | **T2 `BlobStore`** (BLAKE3, plaintext-hash-within-tenant-keyspace → per-tenant dedup; Storage §3.2) | Residency-pinned; crypto-shred-capable. |
| Cross-repo "release readiness" / usage rollups | **OLAP read store** (CQRS, bus-fed, reindex-from-source; Storage §3.4) | Read-only; never a second write path. |
| The pipeline-run **journal** | `myelin-flow`'s Postgres (Workflow §3) | CI does not own it; it owns the workflow *definition*. |

The pipeline definition **snapshot** (the resolved, digest-pinned config) is itself a content-addressed
**T2 blob** referenced by the run, so a run is reproducible down to which bytes it ran (05 §HP-3/HP-4).

## 2. The job spec — the runtime-agnostic seam (ADR-20 / CI-1)

`JobSpec` is the **one struct, two kinds** seam. `SandboxBackend` hides Firecracker vs gVisor vs
self-hosted; `FleetProvider` hides the EU provider. These three traits are what the whole subsystem is
built on (Stage-1 commitment).

```rust
pub struct JobSpec {
    pub kind: JobKind,                 // Ci | Agent — the UNIFY point (TE-31 resolved=UNIFY)
    pub image: ImageRef,               // MUST be digest-pinned; an un-digested tag is rejected (fail-closed, CI-1)
    pub command: Vec<String>,
    pub env: Vec<EnvVar>,              // secrets are NAMES here, resolved inside the boundary (CI-1)
    pub secret_refs: Vec<SecretRef>,   // resolved by the in-boundary broker, scoped to THIS job only
    pub egress: EgressPolicy,          // default-deny; allowlist opt-in; metadata/control-plane/cross-tenant always blocked
    pub limits: ResourceLimits,        // cpu, mem, disk, pids_max, timeout, zero-swap
    pub workspace: WorkspaceSpec,      // checkout via the scoped job-token git wire; read-only root + tmpfs scratch
    pub trust_tier: TrustTier,         // Trusted | UntrustedFork | SelfHosted — gates secrets/cache-write/egress
    pub run_token: RunTokenRef,        // the per-job attenuated token (Id::mint_run_token, contract 4.7)
    pub meter_to: MeterTarget,         // the reserve this job settles against (run-level / agent-run-level)
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

`ToolHands::exec(Command)` (Agent contract 8.4) is realised as `launch(JobSpec{ kind: Agent, .. })` —
**the same runner, the same hardening, the same drill** (02 §5).

## 3. The data model (per-service, `(tenant, region)`-first, RLS)

Every table below carries `tenant uuid` + `region text` as its leading columns (the partition key,
contract 12.1; the `residency-pin` lint asserts `row.region == cell.region` on write). Every personal-data
field carries a `#[personal_data(...)]` tag (the `no-untagged-personal-data` lint, S-5).
**Identity is stored as a *reference* (pseudonym), never copied PII** (the references-not-payloads rule) —
`triggered_by`, `approved_by`, commit-author are pseudonym subjects resolvable through Id, erasable there.

### 3.1 Run / job / step (the run-state model)

```sql
-- The run is a thin index over the myelin-flow workflow (the journal is the workflow's; this is CI's view).
CREATE TABLE ci_run (
  tenant uuid, region text, run_id uuid,
  project_id     uuid NOT NULL,
  repo_ref       text,                          -- ArtifactRef of the triggering repo (reference, not data)
  pipeline_id    uuid NOT NULL,
  wf_run_id      uuid NOT NULL,                 -- the myelin-flow durable run (the lifecycle owner)
  cause_event_id text,                          -- the triggering event_id (causation; dedup anchor)
  definition_snapshot blob_ref NOT NULL,        -- content-addressed (CAS) resolved config — reproducibility/audit
  trigger_kind   text NOT NULL,                 -- push | pull_request | issue_transition | manual | agent | schedule
  triggered_by   text,  -- #[personal_data(actor, role=controller, erasure=pseudonym_shred)]  pseudonym subject
  trust_tier     text NOT NULL,                 -- trusted | untrusted_fork | self_hosted
  state          text NOT NULL,                 -- queued | running | succeeded | failed | cancelled | timed_out | reaped
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

### 3.2 The scheduler tables (the hot path — 02 §2)

```sql
CREATE TABLE job_queue (                          -- one row per schedulable job; the claim surface
  tenant uuid, region text, job_id uuid, run_id uuid,
  lane         text NOT NULL,                      -- interactive | batch | deploy  (priority lanes)
  labels       text[] NOT NULL,                    -- affinity: gpu, arm64, large, linux ...
  trust_tier   text NOT NULL,                      -- gates which runner pools may claim
  concurrency_group text,                          -- "deploy:prod" serialize | "pr:web:42" cancel-superseded
  fair_key     text NOT NULL,                       -- = tenant (or tenant:project) — the DRR fairness bucket
  enqueued_at  timestamptz NOT NULL,
  lease_owner  text, lease_expires timestamptz,    -- the lease (runner identity + TTL)
  state        text NOT NULL,                       -- queued | leased | running | terminal
  PRIMARY KEY (tenant, job_id)
);
CREATE INDEX jq_claimable ON job_queue (region, lane, enqueued_at) WHERE state='queued';
CREATE UNIQUE INDEX jq_serialize ON job_queue (concurrency_group) WHERE state='running' AND concurrency_group LIKE 'deploy:%';

-- DRR fairness accounting: per fair_key deficit counter, advanced at claim time (02 §2).
CREATE TABLE fair_deficit (
  tenant uuid, region text, fair_key text,
  deficit bigint NOT NULL, last_served timestamptz NOT NULL,
  PRIMARY KEY (tenant, region, fair_key)
);
```

### 3.3 Runners & the fleet

```sql
CREATE TABLE runner (
  tenant uuid, region text, runner_id uuid,
  pool          text NOT NULL,                     -- eu-west, gpu-pool ...
  labels        text[] NOT NULL,
  ownership     text NOT NULL,                     -- hosted | self_hosted
  trust_tier    text NOT NULL,                     -- hosted runners claim trusted/untrusted; self-hosted only its own
  attestation   jsonb,                              -- TPM quote / provisioning-signed token (self-hosted) — sketch 05
  attest_state  text NOT NULL,                      -- pending | attested | failed
  health        text NOT NULL,                      -- healthy | degraded | offline
  capacity      jsonb NOT NULL,                      -- slots, cpu/mem
  last_heartbeat timestamptz NOT NULL,
  PRIMARY KEY (tenant, runner_id)
);
-- Hosted runners are tenant-agnostic provisioned capacity; "tenant" on a hosted runner row is the cell-owner.
```

### 3.4 The log range index (the firehose archive seam — 02 §4)

```sql
-- Logs are NOT rows. Bytes live as T3 segments → T2 blobs. This index makes a (job,step,byte-range) addressable.
CREATE TABLE log_segment (
  tenant uuid, region text, run_id uuid, job_id uuid,
  segment_seq  int NOT NULL,
  blob_ref     blob_ref,                            -- content-addressed sealed segment (T2); NULL while open (in firehose)
  byte_start   bigint NOT NULL, byte_end bigint NOT NULL,
  pii_key_ref  text NOT NULL,                       -- kms://<tenant>/<dek-epoch>/blob — per-tenant DEK (crypto-shred)
  PRIMARY KEY (tenant, run_id, job_id, segment_seq)
);
CREATE TABLE log_anchor (                            -- (job, step) → byte offset, for collapsible-per-step + jump-to-failure
  tenant uuid, region text, run_id uuid, job_id uuid,
  step_id   text NOT NULL,                           -- stable across retries (the #step-<id> sub-anchor)
  byte_start bigint NOT NULL, byte_end bigint,
  status    text NOT NULL,                            -- running | passed | failed | skipped
  PRIMARY KEY (tenant, run_id, job_id, step_id)
);
```

### 3.5 Artifacts & caches

```sql
CREATE TABLE artifact (                              -- retained job output; ArtifactRef-addressable
  tenant uuid, region text, artifact_id uuid, run_id uuid,
  name        text NOT NULL,
  blob_ref    blob_ref NOT NULL,                     -- T2 content-addressed
  size_bytes  bigint NOT NULL,
  provenance  jsonb,                                  -- SLSA attestation ref + SBOM ref (sketch 05)
  pii_key_ref text NOT NULL,                          -- per-tenant DEK
  retain_until timestamptz NOT NULL,                  -- explicit TTL (Art. 5 storage-limitation) → GC
  PRIMARY KEY (tenant, artifact_id)
);
CREATE TABLE cache_entry (                            -- reconstructible perf optimization; trust-scoped
  tenant uuid, region text, run_id uuid,
  cache_key   text NOT NULL,                          -- = hash(lockfile + os + toolchain + ...) — the subtle part
  scope       text NOT NULL,                          -- trust-tier/branch scope: a fork CANNOT write the trusted scope
  blob_ref    blob_ref NOT NULL,
  last_used   timestamptz NOT NULL,                   -- LRU eviction
  PRIMARY KEY (tenant, scope, cache_key)
);
```

### 3.6 Deployments, environments, secrets, cost

```sql
CREATE TABLE environment (
  tenant uuid, region text, env_id uuid, project_id uuid,
  name        text NOT NULL,                          -- prod | staging ...
  protected   bool NOT NULL,                          -- protected → HITL gate on deploy
  required_approvers text,                            -- resolves via Id::list_subjects(env, approve) at gate time
  PRIMARY KEY (tenant, env_id)
);
CREATE TABLE deployment (
  tenant uuid, region text, dep_id uuid, env_id uuid, run_id uuid,
  version     text NOT NULL,
  state       text NOT NULL,                          -- awaiting_approval | deploying | deployed | failed | rolled_back
  approved_by text,  -- #[personal_data(actor, role=controller, erasure=pseudonym_shred)]
  PRIMARY KEY (tenant, dep_id)
);
CREATE TABLE secret_binding (                          -- NAMES + scope only; VALUES live in the shared secret store (Id/GDPR-placed)
  tenant uuid, region text, project_id uuid, name text,
  scope       text NOT NULL,                           -- env / project; untrusted_fork resolves to NONE by default
  value_ref   text NOT NULL,                            -- a reference into the shared secret capability (never the value)
  PRIMARY KEY (tenant, project_id, name, scope)
);
CREATE TABLE cost_event (                              -- one row per metered unit (D8); wholesale & markup separate columns
  tenant uuid, region text, cost_id uuid, run_id uuid, job_id uuid,
  meter       text NOT NULL,                            -- cpu_seconds | mem_gb_seconds | gpu_seconds | storage_gb_hours | egress_gb
  amount      bigint NOT NULL,                           -- integer quantity of the meter unit (NEVER a float)
  wholesale_minor_units bigint NOT NULL,                 -- the resource cost
  markup_minor_units    bigint NOT NULL,                 -- Commercial's priced amount (immutable pricing table)
  kind        text NOT NULL,                             -- ci | agent — same schema fronts both (UNIFY)
  PRIMARY KEY (tenant, cost_id)
);
```

### 3.7 The dedup ledger (exactly-once *effect*)

```sql
CREATE TABLE consumer_dedup (                          -- the platform consumer template's ledger (contract 2.5)
  tenant uuid, region text, consumer text, event_id text,
  PRIMARY KEY (consumer, event_id)
);
-- Trigger & Dispatch dedups on the triggering event_id so one push = one run (exactly-once effect).
```

## 4. Encryption, residency & GDPR posture (the data-model invariants)

- **Per-tenant envelope encryption + crypto-shred** on every blob/log/artifact store (`pii_key_ref =
  kms://<tenant>/<dek-epoch>/<class>`, contract 11.3/11.4; the S-8 grammar). Bulk content is per-tenant DEK;
  the per-subject-DEK case for free-text PII in logs is the named GD-6 floor (05 §HP-7).
- **Residency by construction** — no global pool; the `residency-pin` lint (S-1) + the harness
  `row.region == cell.region` assertion (S-7) on every CI write; `residency_verify` attests it (07 drill R-3).
- **Identity is referenced, never copied** — actors are pseudonym subjects; erasure flows through Id's
  `resolve_pseudonym`/`erase` (contract 4.8) + CI's crypto-shred of the incidental-PII stores (03 §6).
- **Auto-registration** — every table above is opened through `serve(AppSpec)`, so the harness
  auto-registers each as a `PersonalDataHolder` (contract 1.4); "we forgot the cache table" is structurally
  impossible (GD-3).
