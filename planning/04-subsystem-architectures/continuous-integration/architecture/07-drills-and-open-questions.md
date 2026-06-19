# CI/CD — 07 Drills & Open Questions

> Phase 5-B — CI detailed architecture (rewritten against the reconciled layer). The **quantified drills** CI
> owes (PROVE-IT: each failable property named with a measurable gate, feeding the Phase-5/6 testing
> strategy), and the **open questions** handed to Phase 6 / Legal, tagged by resolver. A property not drilled
> is a claim, not a fact (EI-04 §5.1).

---

## 1. Drills owed (quantified)

| # | Drill | The property under test | Quantified gate |
|---|---|---|---|
| **T-1 (escape drill, X-6)** | **Sandbox escape drill — the single hard go/no-go that gates ALL agent execution.** Run an adversarial corpus inside a production-backend sandbox on a **real kernel**: kernel-exploit primitives, cloud-metadata SSRF (169.254.169.254) → cred theft, control-plane/internal-RPC reach, cross-tenant network/storage, fork bomb, disk fill, secret exfil via egress. | Untrusted code (CI **and** agent `ToolHands::exec`) cannot escape the boundary. | **Zero escapes.** Green attestation artifact or **CI is no-go for untrusted code**. Re-run on every backend/image/kernel change. |
| **D-1** | **Crash-recovery / effectively-once** — kill the runner mid-job; kill the control plane mid-run. | A run resumes (workflow replay + `SCHEDULE_AND_RUN_JOB` idempotent re-dispatch on `idem_token`) with **no double-effect**. | **Effectively-once job execution; zero lost runs; zero double-deploys; zero duplicate artifact publishes.** |
| **D-2** | **CI surge / fairness** — 30× CI surge on one tenant. | Lanes + DRR fair-share + the per-surface shed budget (OQ-K) hold; the reaper recovers dead leases. | Interactive lane holds its latency budget; batch lane sheds (429 + Retry-After honoured by `myelin ci`); **other tenants unaffected**; reserve/settle refuses over-budget runs; a killed runner's jobs re-queue **within the lease TTL**, zero orphans. |
| **D-3** | **Erasure-reaches-every-holder** — `erase(subject)` fans out to CI. | PII in logs/artifacts/caches/run-state is destroyed; attribution falls back to the opaque pseudonym; unfurls degrade to tombstones via the OQ-D ladder. | **Subject PII unrecoverable** (per-subject DEK destroyed where isolable; per-tenant fallback) across logs/artifacts/caches/run-state **incl. backups**; run *structure* survives for audit; zero dangling leaks in any unfurl/embed. |
| **D-4 (supply-chain)** | **Supply-chain fail-closed** — a pipeline references a floating tag (image or component); a tampered/unsigned component. | Digest-pin + sign-verify hold at plan/run time. | **Zero un-pinned executions; zero unsigned-component runs.** The floating tag fails closed at `plan`; the unsigned component is refused; `ci.supply_chain.verification_failed` emitted (audit). |
| **D-5** | **Reserve/settle parity (CI ↔ agent)** — exhaust the wallet, then start a CI run and an agent `compute` job; replay across a pricing change. | The one metering path refuses-start (never interrupts in flight); wholesale ≠ markup. | **Zero starts past exhaustion** for either kind; in-flight runs finish; one cost event per metered unit; **wholesale ≠ markup invariant holds across a pricing-change replay**. |
| **D-6** | **Fork-cannot-poison-trusted-cache** — an adversarial `UntrustedFork` run attempts to write the default-branch cache scope. | The trust-tier/branch-scoped cache namespace (Storage C4) holds **structurally**. | **Zero trusted-cache writes** from a fork-tier run. |
| **D-7** | **Fork-gets-no-secrets** — an adversarial fork run attempts to read protected secrets. | The `read & !is_untrusted_fork` ABAC edge holds. | **Zero secret reads** by a fork-tier run; protected-env secrets require explicit grant/approval. |
| **R-3** | **Residency** — an EU-resident tenant's run. | No global pool; logs/artifacts/caches/state stay in-region. | Job claimed **only** by an in-region runner; **logs/artifacts/caches never leave the region** (CDN edge within-EU only); `residency_verify` attests the runner pool + log/artifact/cache region; the `residency-pin` lint passes on every CI write. |
| **D-8 (CheckStatus seam, X-1)** | **Git↔CI merge-gate seam** — a push → `ci.check.updated` per context → required-checks-green → merge; an out-of-order / re-delivered `ci.check.updated`; a fork-tier success; a re-run. | The frozen `CheckStatus` contract is correct + idempotent. | Git's projection holds the correct current row per `(commit_oid, context)`; a **lower-`run_attempt`** arrival is dropped, a higher one supersedes (monotonic, not wall-clock); an **`untrusted_fork` success is neutral for gating** until endorsed/re-run-trusted; **the merge-queue workflow wakes on the `ci.result` signal** (idempotent on `idem_token`); zero spurious unblocks. |
| **D-9** | **Determinism guard** — the `ci.pipeline` workflow body. | No clock/RNG/IO outside `WfCtx`; the scheduler's non-determinism is journaled inside the dispatch, not the body. | The `flow-determinism` lint passes; **replay is bit-identical**; only the journaled `job.done` signal result feeds the body. |
| **D-10** | **Self-hosted runner trust boundary** — a compromised self-hosted runner. | The scoped job token (tenant-`SelfHosted`-scoped) bounds blast radius. | The runner can read **only** its own tenant's `SelfHosted`-tier job; **zero cross-tenant job/secret reads**; attestation failure → the runner cannot claim. |
| **D-11 (NEW, OQ-J)** | **Live-log reconnect-loses-zero-ops** — drop the live-tail connection mid-run, reconnect with `last_seq`. | The firehose resume-cursor protocol backfills `(last_seq, now]`. | **Zero log lines lost** on reconnect; a `last_seq` past the retention window → `resync_required` → clean range-read fallback; scope stays bounded (never `*`). |

The drills feed the Phase-5/6 testing strategy; **T-1 (the escape drill) is the gating milestone** — per X-6
it precedes every other CI capability that runs untrusted code (CI **or** agent).

## 2. Open questions (by resolver)

### To CI's own Phase-6 build

1. **Exact DRR weights + replenishment cadence + the per-`fair_key` starvation histogram threshold** that
   promotes from flat DRR to a hierarchical scheduler (02 §2.2 floor) — tuned against measured load.
2. **The pre-warm buffer sizing function** (warm microVM pool size vs recent arrival rate vs the per-VM
   memory floor) — the cold-start-vs-cost trade-off, measured per (region, label-class) (02 §5.4).
3. **The full escape-drill (T-1) adversarial corpus** — CI enumerates the obligation; the concrete exploit
   set + the green-attestation format are built/executed in Phase 6 (`[OPEN → P6]`). The gate.
4. **The gVisor-second-backend promotion trigger** — the measured density/latency economics (esp. sub-second
   agent `compute` calls) that justify adding the second backend behind the trait (HP-1/HP-5 floor).
5. **The per-surface shed budget concrete numbers** (OQ-K names the floor: bounded run-queue per tenant,
   runners pull-bounded; the numbers are CI's P6 budget call, asserted by D-2).

### Resolved by Phase-5 reconciliation (closed — recorded for traceability)

- **The `SCHEDULE_AND_RUN_JOB` activity-vs-signal shape** — **CLOSED** as the frozen OQ-F idiom (`job.done`
  signal, workflow-minted `idem_token`); the merge queue uses `ci.result` (contracts 9.2/9.4).
- **The Git↔CI `CheckStatus` keying + ordering** — **CLOSED** as the frozen contract 5.9 (X-1): the
  `(commit_oid, context)` key, monotonic-`run_attempt` supersession, `trust_tier` gating, the `ci.result`
  merge-queue signal.
- **The `list_objects` push-down over `run_id`** — **CLOSED** as the frozen `SetExpr` JOIN (OQ-E,
  contract 4.3).
- **The four uniform sandbox guarantees + `requires_approval` defaults** — **CLOSED** (X-6, contracts 8.1/8.4).

### To Legal / DPO (`[OPEN — LEGAL]`)

6. **The residual third-party / non-isolable free-text crypto-shred basis in logs** — the isolable case
   **ships built** (per-subject DEK, Storage C1); the residual third-party span follows the **one platform
   posture** (X-7, contract 10.9), counsel-ratified.
7. **Build-data-as-LLM-training lawful basis** (AG-8) — foreclosed by default; no platform path feeds tenant
   build data to training; flagged (OQ-H).
8. **CD-as-PaaS product scope** (PR-5) — a product + legal scope question, flagged to Commercial; the CI
   sandbox + reserve/settle + residency primitives already support it (OQ-H).

### Deferred (named floors, not built v1)

9. **`myelin ci local`** (laptop execution) — a UX win vs a fidelity cost; deferred (04 §2).
10. **Cross-cell-spanning pipelines** — a pipeline whose jobs span cells of a multi-cell tenant;
    designed-not-built, inherits the cross-cell PII-free pointer bridge (contract 12.6 / OQ-I).

---

## 3. The single most important thing in this whole subsystem

**T-1, the escape drill, is the one gate everything else waits behind.** CI's value is composition of the
frozen contracts plus two genuinely-hard cores (the scheduler, the EU fleet autoscaler) — but its *risk*
concentrates entirely in the one place untrusted code runs. Hardware-isolated microVMs + the mandatory
hardening profile + the four uniform guarantees (X-6) + a real-kernel adversarial drill, re-run on every
change, is the architecture's non-negotiable. Per X-6, **until T-1 is green on the production backend, no
untrusted CI step and no agent `ToolHands::exec` call runs.** That sequencing — drill first, capability
second — is the spine of this design.
