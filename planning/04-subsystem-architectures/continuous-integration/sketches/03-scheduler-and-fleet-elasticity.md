# Sketch 03 — The distributed scheduler + runner-fleet elasticity on EU infra (TE-29)

> Phase 4 — CI exploration. The hardest distributed-systems core CI owns (CI-DD §5.2): matching queued
> jobs to a heterogeneous, elastic runner fleet with fairness, lanes, concurrency groups, affinity,
> leases, and exactly-once assignment — and doing it **without hyperscaler autoscaling primitives**, on
> **EU-controlled infra** (TE-29; ADR-11). Sits behind the `SCHEDULE_AND_RUN_JOB` activity (sketch 02).

---

## Part 1 — Assignment model: pull-leasing vs push-assignment

### Candidate A — Push (control plane assigns job → runner)
Central scheduler picks a runner and pushes. **Pro:** global optimality possible. **Con:** the control
plane must track live runner capacity/health precisely; a stale view mis-assigns; scaling the scheduler
is harder; it fights the elastic/ephemeral fleet (runners come and go). Borg/Kubernetes-style; heavy.

### Candidate B — Pull-leasing (runners claim work) — **chosen, the directional model already (Phase-2 §2.1)**
Runners long-poll / claim the next eligible job for their labels via `FOR UPDATE SKIP LOCKED` over a
queue table, take a **lease** (`lease_owner` + `lease_expires`), and **heartbeat** to extend it. A
**dead-runner reaper** re-queues jobs whose lease expired (the run's `SCHEDULE_AND_RUN_JOB` activity
retries — sketch 02). **Pros:** the runner pulls only what it can run (capacity is self-evident); no
central live-capacity tracking; the same `FOR UPDATE SKIP LOCKED` + lease primitive the platform
already uses for the outbox relay and the timer wheel (event-bus §4.1; Workflow §4.2/§4.7) — proven,
not novel; trivially horizontally scalable (more runners = more pulls). **Con:** "fair-share" and
"affinity" must be encoded in the *claim query*, not in a central planner — harder to express but
cheaper to operate. This is the Buildkite-agent / Nomad-pull model.

**Decision: pull-leasing.** It is the lowest-novelty, highest-operability fit for an ephemeral,
self-hostable, EU-bare-metal fleet, and it reuses the platform's existing lease primitive.

## Part 2 — Fairness, lanes, concurrency, affinity (the claim semantics)

The scheduler's intelligence lives in **which job a runner is allowed to claim next**:

```sql
-- The job queue (CI's own Postgres; (tenant,region) first key; RLS). One row per schedulable job.
CREATE TABLE job_queue (
  tenant uuid, region text, job_id uuid, run_id uuid,
  lane         job_lane NOT NULL,        -- interactive | batch | deploy  (priority lanes)
  labels       text[]   NOT NULL,        -- affinity: gpu, arm64, large, linux ...
  trust_tier   trust_tier NOT NULL,      -- gates which pools may claim (untrusted → no self-hosted-trusted pool)
  concurrency_group text,                -- e.g. "deploy:prod" — serialize; "pr:web:42" — cancel-superseded
  enqueued_at  timestamptz NOT NULL,
  fair_key     text NOT NULL,            -- = tenant (or tenant:project) — the fairness bucket
  lease_owner  text, lease_expires timestamptz,
  state        text NOT NULL,            -- queued | leased | running | terminal
  PRIMARY KEY (tenant, job_id)
);
CREATE INDEX jq_claimable ON job_queue (lane, enqueued_at) WHERE state='queued';
```

- **Fair-share across tenants (no starvation):** the claim query implements **weighted-fair / deficit
  round-robin over `fair_key`** — a runner claims the oldest job of the *least-recently-served* tenant
  eligible for its labels, not simply the globally-oldest job. This prevents one tenant's 10k-job
  matrix from starving every other tenant (CI-DD §5.2). Borrowed from DRR (Shreedhar & Varghese, *Deficit
  Round Robin*, 1996) and Linux CFS's fairness intuition, applied at claim time.
- **Priority lanes:** `interactive` (PR checks) > `batch` (nightly) > `deploy` is a strict lane order in
  the claim query; this is the **protected-human-lane analogue inside CI** — interactive PR feedback
  must not queue behind nightly batch (ties to the platform shed order: speculative → batch/CI → agent →
  human-last, ADR-16; here it is intra-CI lane priority).
- **Concurrency groups:** `deploy:prod` is a **serialization key** (one running at a time — a partial
  unique index on `(concurrency_group) WHERE state='running'`); `pr:web:42` is **cancel-superseded** (a
  new push cancels the in-flight run for the same group — CI-DD §6.5). Both are claim-time predicates.
- **Affinity:** label-match (`gpu`, `arm64`, `large`, `linux`); a runner only claims jobs whose labels
  ⊆ its labels.
- **Per-tenant in-flight cap + backpressure:** bounded queue, statement timeouts, per-tenant in-flight
  ceiling (X-3); over-cap jobs **queue** (graceful), never collapse the scheduler. A 30× surge sheds the
  *agent/CI lane*, not the interactive human lane (the SRCH-D6-analogue CI surge drill).

## Part 3 — Fleet elasticity on EU infra (TE-29) — the divergence-by-constraint

ADR-11 **forbids hyperscaler autoscaling primitives**; the fleet runs on EU-controlled infra
(Hetzner / OVH / Scaleway / Exoscale / sovereign-cloud / bare-metal — the menu is a commercial pick,
flagged not foreclosed). So **we build the autoscaler** rather than renting one.

### Candidate fleet models
- **A — fixed bare-metal pools, no scale-to-zero.** Cheapest per-core, simplest, but pays for idle
  capacity and can't absorb spikes. Fine for a single-tenant self-host; wrong for the multi-tenant
  hosted edge.
- **B — autoscale-on-queue-depth over EU IaaS + bare-metal, with pre-warmed pools** — **chosen.** A
  pool manager watches `job_queue` depth per (region, label-class) and provisions/deprovisions runner
  hosts via the **EU provider's own API** (each provider gets a `FleetProvider` adapter behind a trait,
  mirroring the platform's swappable-backend ethos). Pre-warmed **microVM snapshot pools** (sketch 01)
  absorb the cold-start tension: a warm snapshot resumes in tens of ms; the autoscaler keeps a small
  warm buffer sized to the recent arrival rate. **Scale-to-zero** for idle tenants/regions (compute is
  the dominant cost, CI-DD §5.3). Bin-packing places jobs onto hosts to maximize density under the
  microVM memory floor.
- **C — Kubernetes-on-EU-infra as the substrate.** Tempting (autoscaler, bin-packer exist), but
  K8s-the-autoscaler is the very primitive we'd be leaning on, it adds a heavy operational universe per
  cell (against the self-host-parity ethos), and the security boundary is still ours to build. **Kept
  as a `FleetProvider` *option* for customers who already run K8s, not the default.**

```rust
pub trait FleetProvider {                 // Hetzner | OVH | Scaleway | BareMetal-PXE | K8s(customer) | SelfHosted
    fn provision(&self, class: RunnerClass, n: u32, region: Region) -> Result<Vec<RunnerHost>>;
    fn deprovision(&self, hosts: &[RunnerHost]) -> Result<()>;
    fn capacity(&self, region: Region) -> Result<Capacity>;
}
```

### Residency by construction (ADR-11; the platform's strongest data-residency argument)
There is **no global runner pool** — pools are **partitioned per residency zone**. An EU-resident
tenant's job is claimed only by a runner in its region; logs/artifacts/caches/state stay in-region
(CI-DD §5.6, §9). Cross-region is opt-in only. This is enforced at claim time (`region` predicate) and
by the residency-pin lint on every store CI writes (S-1).

### Self-hosted runners (TE-29 trust surface)
Customer-infra runners register, **attest** (sketch 05 covers attestation + supply-chain), receive a
**scoped job token** (Id `mint_run_token`-class, short-TTL, bound to one job/repo), and claim only
their tenant's `trust_tier ∈ {SelfHosted}` jobs. A compromised runner is bounded by its scoped token —
it cannot read other tenants' jobs/secrets (CI-DD §7 control-plane hardening). Non-negotiable for the
EU-enterprise audience (CI-DD §6.1).

## Why Rust (control plane + runner agent) — justified per ADR-02
The scheduler/state-machine is a **latency- and correctness-critical hot path** explicitly named to
stay Rust (ADR-02; Phase-2 §3 "no divergence justified"). The runner agent is a **single small attested
binary at the trust boundary** — memory-safety there is load-bearing; same artifact for hosted +
self-hosted. No divergence from the Rust default is justified anywhere in CI.

## Floors & follow-ons
- **FLOOR:** v1 ships one or two `FleetProvider` adapters (the commercial infra pick) + self-hosted;
  more providers are adapters, not redesigns.
- **FLOOR:** the fair-share algorithm ships as DRR-over-`fair_key`; a measured starvation signal is the
  promotion trigger to a richer hierarchical scheduler.
- **Drill owed (PROVE-IT):** 30× CI surge on one tenant → interactive lane holds latency budget, batch
  lane sheds (429+Retry-After honoured by our own `myelin ci` client), **other tenants unaffected**,
  reserve/settle refuses over-budget runs. Plus: kill a runner mid-lease → reaper re-queues within the
  lease TTL, zero orphaned jobs.
