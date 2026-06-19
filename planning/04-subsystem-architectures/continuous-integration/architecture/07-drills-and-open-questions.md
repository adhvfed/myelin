# CI/CD — 07 Drills & Open Questions

> Phase 4 — CI Stage-2. The **quantified drills** CI owes (PROVE-IT: each failable property is named with a
> measurable gate, feeding the Phase-5 testing strategy), and the **open questions** handed to Phase 5 /
> Legal, tagged by resolver. A property not drilled is a claim, not a fact (EI-03 §3.5).

---

## 1. Drills owed (quantified)

| # | Drill | The property under test | Quantified gate |
|---|---|---|---|
| **T-1 (AG-D4)** | **Sandbox escape drill** — the single hard go/no-go. Run an adversarial corpus inside a production-backend sandbox on a **real kernel**: kernel-exploit primitives, cloud-metadata SSRF (169.254.169.254) → cred theft, control-plane/internal-RPC reach, cross-tenant network/storage, fork bomb, disk fill, secret exfil via egress. | Untrusted code (CI **and** agent) cannot escape the boundary. | **Zero escapes.** Green attestation artifact or **CI is no-go for untrusted code**. Re-run on every backend/image/kernel change. |
| **D-1** | **Crash-recovery / effectively-once** — kill the runner mid-job; kill the control plane mid-run. | A run resumes (workflow replay + activity retry) with **no double-effect** (no double-deploy, no duplicate artifact publish). | **Effectively-once job execution; zero lost runs; zero double-deploys.** |
| **D-2** | **CI surge / fairness** — 30× CI surge on one tenant. | Lanes + DRR fair-share hold; the reaper recovers dead leases. | Interactive lane holds its latency budget; batch lane sheds (429 + Retry-After honoured by `myelin ci`); **other tenants unaffected**; reserve/settle refuses over-budget runs; a killed runner's jobs re-queue **within the lease TTL**, zero orphans. |
| **D-3** | **Erasure-reaches-every-holder** — `erase(subject)` fans out to CI. | PII in logs/artifacts/caches/run-state is destroyed; attribution falls back to the opaque pseudonym; unfurls degrade to tombstones. | **Subject PII unrecoverable** (key destroyed) across logs/artifacts/caches/run-state; run *structure* survives for audit; zero dangling leaks in any unfurl/embed. |
| **D-4** | **Supply-chain fail-closed** — a pipeline references a floating tag (image or component); a tampered/unsigned component. | Digest-pin + sign-verify hold at plan/run time. | **Zero un-pinned executions; zero unsigned-component runs.** The floating-tag reference fails closed at `plan`; the unsigned component is refused; `ci.supply_chain.verification_failed` emitted (audit). |
| **D-5** | **Reserve/settle parity (CI ↔ agent)** — exhaust the wallet, then start a CI run and an agent `compute` job; replay across a pricing change. | The universal gate refuses-start (never interrupts in flight); wholesale ≠ markup. | **Zero starts past exhaustion** for either kind; in-flight runs finish; one cost event per metered unit; **wholesale ≠ markup invariant holds across a pricing-change replay**. |
| **D-6** | **Fork-cannot-poison-trusted-cache** — an adversarial `UntrustedFork` run attempts to write the default-branch cache scope. | Cache scope boundary holds. | **Zero trusted-cache writes** from a fork-tier run. |
| **D-7** | **Fork-gets-no-secrets** — an adversarial fork run attempts to read protected secrets. | Trust-tier secret gate holds. | **Zero secret reads** by a fork-tier run; protected-env secrets require explicit grant/approval. |
| **R-3** | **Residency** — an EU-resident tenant's run. | No global pool; logs/artifacts/caches/state stay in-region. | Job claimed **only** by an in-region runner; **logs/artifacts/caches never leave the region**; `residency_verify` attests; the `residency-pin` lint passes on every CI write. |
| **D-8** | **Git↔CI merge-gate seam** — a push → checks → required-checks-green → merge; an out-of-order / re-delivered `ci.status.updated`. | The checks contract is correct + idempotent. | The merge gate sees the correct `check_status` per `(commit_oid, context)`; a re-delivered/out-of-order status is last-writer-wins per context; **the merge-queue workflow wakes on the `ci.result` signal**; zero spurious unblocks. |
| **D-9** | **Determinism guard** — the `ci.pipeline` workflow body. | No clock/RNG/IO outside `WfCtx`; the scheduler's non-determinism is journaled inside the activity, not the body. | The `flow-determinism` lint passes; **replay is bit-identical**; only the journaled terminal-signal result feeds the body. |
| **D-10** | **Self-hosted runner trust boundary** — a compromised self-hosted runner. | The scoped job token bounds blast radius. | The runner can read **only** its own tenant's `SelfHosted`-tier job; **zero cross-tenant job/secret reads**; attestation failure → the runner cannot claim. |

The drills feed the Phase-5 testing strategy; T-1 (the escape drill) is the **gating milestone** — it
precedes every other CI capability that runs untrusted code (CI or agent).

## 2. Open questions (by resolver)

### To CI's own Phase-5 build

1. **Exact DRR weights + replenishment cadence + the per-`fair_key` starvation histogram threshold** that
   promotes from flat DRR to a hierarchical scheduler (02 §2.2 floor) — tuned against measured load.
2. **The pre-warm buffer sizing function** (warm microVM pool size vs recent arrival rate vs the per-VM
   memory floor) — the cold-start-vs-cost trade-off, measured per (region, label-class) (02 §4.4/§5).
3. **The full AG-D4 adversarial corpus** — CI enumerates the obligation (T-1); the concrete exploit set +
   the green-attestation format are built/executed in Phase 5 (`[OPEN → P5]`).
4. **The gVisor-second-backend promotion trigger** — the measured density/latency economics (esp. sub-second
   agent `compute` calls) that justify adding the second backend behind the trait (HP-1/HP-5 floor).

### To Phase-5 reconciliation (cross-subsystem)

5. **The `SCHEDULE_AND_RUN_JOB` activity-vs-signal shape** (CR-WF-1) co-finalised with Workflow §9 — the
   precise contract for a long-parked activity whose completion is a `ci.result` signal, and the
   `idem_token` threading on reaper retry.
6. **The resource-second meter ↔ Commercial credit/price table** reconciliation (CR-GDPR-adjacent; X-5) —
   the exact unit→credit mapping and the immutable-pricing-history guarantee (06 → C-1).
7. **The Git↔CI `ci.status.updated` keying + ordering** (D-8) co-finalised with git-hosting `02 §6` /
   `06 §CR-CI` — the `(commit_oid, context)` key, the re-run supersession, and the merge-queue signal shape.

### To Legal / DPO (`[OPEN → LEGAL]`)

8. **Per-subject free-text crypto-shred in logs** (GD-6; CR-STOR-3) — per-tenant vs per-subject DEK
   granularity for inline log PII; the per-subject case is the named floor.
9. **Build-data-as-LLM-training lawful basis** (AG-8) — whether/under-what-basis CI build data (logs,
   failures) may train models; flagged, not foreclosed (CR-GDPR-2).
10. **CD-as-PaaS product scope** (PR-5) — how far CI's deploy/CD surface extends into hosting customer
    workloads; a product + legal scope question, flagged.

### Deferred (named floors, not built v1)

11. **`myelin ci local`** (laptop execution) — a UX win vs a fidelity cost; deferred (04 §2).
12. **Cross-cell-spanning pipelines** — a pipeline whose jobs span cells of a multi-cell tenant;
    designed-not-built, inherits the Workflow §7.4 multi-cell floor.

---

## 3. The single most important thing in this whole subsystem

**T-1, the escape drill, is the one gate everything else waits behind.** CI's value is composition of the
Phase-3 contracts plus two genuinely-hard cores (the scheduler, the EU fleet autoscaler) — but its
*risk* concentrates entirely in the one place untrusted code runs. Hardware-isolated microVMs + the
mandatory hardening profile + a real-kernel adversarial drill, re-run on every change, is the architecture's
non-negotiable. Until T-1 is green on the production backend, no untrusted CI step and no agent `compute`
call runs. That sequencing — drill first, capability second — is the spine of this design.
