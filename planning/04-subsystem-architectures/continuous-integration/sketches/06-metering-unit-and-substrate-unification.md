# Sketch 06 — Metering unit (TE-32) + CI↔agent substrate unification depth (TE-31, realising UNIFY)

> Phase 4 — CI exploration. Two decisions: (1) the **metering unit** for the universal reserve/settle
> gate (TE-32) — what one CI cost event measures; and (2) **how deep CI and the agent substrate unify**
> (TE-31, already *resolved = UNIFY* by ADR-20/D5 — this realises it concretely and names where they
> stay distinct).

---

## Part 1 — Metering unit (TE-32)

### The constraint surface (already fixed by Phase 3)
- The **reserve/settle gate is the workflow's bookends** (sketch 02; Workflow §6.2; contract 11.7):
  reserve at run start (refuse-start-on-exhaustion), settle on completion, never interrupt in flight.
- **Cost is integer minor-units, never floats** (X-5; substrate §2.10); **wholesale ≠ markup kept
  separate** so a pricing change never rewrites history (D8; C-1 immutable pricing history).
- The **wallet/pricing model is Commercial's** (C-1); CI owns only the **metering *unit* + the cost
  events**, not the price.

### Candidate units (CI-DD §11 Q10)
- **A — build-minutes (wall-clock per runner class).** Familiar (Actions/CircleCI), simple to explain.
  But it hides resource cost (a 1-core job and a 32-core GPU job both bill "a minute"); it rewards
  hogging a big runner; and "minute" isn't agent-comparable.
- **B — credits (abstract unit, runner-class-weighted).** Flexible pricing knob, but opaque to users
  ("why 40 credits?") and decoupled from real cost.
- **C — resource-seconds per runner class (CPU-second, GB-second, GPU-second) — chosen as the base
  meter.** Meter the **actual resources held** (cpu-seconds, mem-GB-seconds, gpu-seconds, plus
  storage-GB-hours for artifacts/cache and egress-GB) as the **wholesale** measure. This is the honest
  cost basis, it bin-packs well (sketch 03 rewards density), and it is **directly comparable to an
  agent run's cost** (an agent `compute` tool call holds the same resources). Commercial maps
  resource-seconds → a **credit/price** at the markup layer (kept separate) for a human-legible bill;
  users *see* credits, the *meter* is resource-seconds.

```rust
// One cost event per metered unit (D8). Wholesale (resource-seconds) and markup (price) are SEPARATE rows.
pub struct CostEvent {
    pub run_id: RunId, pub job_id: Option<JobId>,
    pub meter: Meter,                  // CpuSeconds | MemGbSeconds | GpuSeconds | StorageGbHours | EgressGb
    pub wholesale_minor_units: i64,    // integer; the resource cost  (NEVER a float)
    pub markup_minor_units: i64,       // integer; the priced amount (Commercial's pricing table, immutable)
    pub kind: JobKind,                 // Ci | Agent — the SAME meter schema fronts both (Part 2)
}
```

### Why this satisfies "a runaway is self-limiting" (D8/CI-2)
Reserve checks the **prepaid balance** before start; **no balance → no run** (EI-03 §5.2). A runaway
agent-triggered CI storm **spends down the wallet and stops** — not a surprise infra bill. The meter
being resource-seconds means the reserve is sized to real cost, so the wallet bound is meaningful.

### Quotas + abuse (the free-tier magnet, CI-DD §5.9)
Per-tenant in-flight caps (sketch 03) + reserve/settle (no balance → no start) + abuse detection
(crypto-mining heuristics on sustained high-CPU-no-IO) compose. The economic control is the wallet;
the structural control is the bounded queue + sandbox limits.

## Part 2 — Realising UNIFY (TE-31 = UNIFY): where CI and the agent substrate are one, and where distinct

ADR-20/D5 already **resolved TE-31 = UNIFY**; the Phase-4 burden inverted to "justify in writing if you
diverge." We do **not** diverge. The realisation:

### ONE thing (unified)
| Surface | Unification |
|---|---|
| **The job spec** | `JobSpec{ kind ∈ {Ci, Agent} }` — one spec, one struct (sketch 01). |
| **The sandbox runner + hardening profile** | identical for both kinds; `ToolHands::exec` (Agent contract 8.4) **is** `SandboxBackend::launch(JobSpec{kind:Agent})` on CI's runner. |
| **The escape drill (AG-D4)** | one drill, on the one runner, gates **both** CI and agent untrusted code — the single hard gate (Agent §11.2: "CI owns the runner + the escape drill"). |
| **The reserve/settle gate** | one gate, one `CostEvent` schema, fronts CI runs *and* agent runs (Part 1; contract 11.7). |
| **Metering** | one meter (resource-seconds), `kind` distinguishes for billing reporting, not for the mechanism. |
| **Secrets-inside-the-boundary** | identical broker + rule for both kinds (CI-1). |

### Distinct things (deliberately NOT collapsed)
| Concern | CI | Agent fabric | Why distinct |
|---|---|---|---|
| **Orchestration** | the `ci.pipeline` workflow (stages/DAG) | the `agent_run` plan-then-apply loop (sketch 02; Agent §6.1) | different deterministic workflow definitions over the same engine |
| **Side-effecting mutation** | n/a (CI steps run code) | **`EffectApi::apply`** (plan-then-apply, governed mutation via public endpoint) — **NOT `ToolHands::exec`** (Agent §2.2/§5.0) | the runner only runs *untrusted computation*; governed mutation is a separate path |
| **The "brain"** | none (CI runs declared steps) | `AgentRuntime::step` (the LLM/mock seam) | CI has no model; the brain is agent-only |
| **Scheduler** | CI's fair-share/lanes/affinity pull-leasing (sketch 03) | `myelin-flow` partition lease + the dispatch tier | CI's multi-tenant fleet scheduling is not a durable-execution concern |

**The clean one-liner:** *CI and the agent fabric share **the hands and the hardening** (the sandbox,
the job spec, the drill, the cost gate, the secret broker); they differ in **the head and the
governance** (the orchestration workflow, the brain, and the `EffectApi` mutation path).* That is the
exact depth of UNIFY — enough that the catastrophic surface (untrusted execution) is built and drilled
**once**, not enough to conflate sandboxed *computation* with governed *mutation* (which would be a
security regression — Agent §2.2 is explicit that side-effecting tools never touch `ToolHands::exec`).

### A `kind=agent` job that CI runs (the concrete seam)
When an agent's brain returns `UseTools` with a `compute` tool (Agent §5.0 routing), the loop calls
`ToolHands::exec(cmd)`; the real impl builds `JobSpec{ kind: Agent, image: <digest>, command: cmd,
egress: default-deny, secret_refs: <this run's scope>, limits: <agent budget> }` and submits it to
**CI's runner** — same launch path, same hardening, same drill, metered into the same wallet under the
agent's reserve. CI doesn't know or care it's an agent vs a CI step at the sandbox layer; only the
`kind` tag and the owning workflow differ.

## Floors & follow-ons
- **FLOOR:** v1 meters CPU/mem/GPU-seconds + storage-GB-hours + egress-GB; finer-grained meters
  (network shaping, cache-hit credits) are measured follow-ons.
- **FLOOR:** the gVisor second backend (sketch 01) is the likely first home for short agent `compute`
  calls where microVM start-latency dominates a sub-second tool call — a measured economics decision,
  not a v1 commitment.
- **Drill owed (PROVE-IT):** reserve/settle parity — an exhausted wallet **refuses to start** both a CI
  run and an agent `compute` job (never interrupts in flight); one cost event per metered unit;
  wholesale and markup never co-mingled. Gate: zero starts past exhaustion; wholesale≠markup invariant
  holds across a pricing change replay.
