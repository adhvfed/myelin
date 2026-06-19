# CI/CD — 04 Views, CLI & API (the agent-tool surface)

> Phase 4 — CI Stage-2. The view inventory (referencing `../design/`), the CLI command surface (a peer
> rendering, not a second-class afterthought), and the API / agent-tool surface (the `ToolDef`
> registrations with their `requires_approval` defaults). The design sketches (`../design/`) are the
> authoritative wireframes; this doc grounds them in the data model + contracts and fixes the tool surface.

---

## 1. Views (the §7.2 catalogue, placed in the one shell)

CI is one entry in the **primary rail** (`Code · CI · Issues · Knowledge · Chat · Inbox · Search`). It
composes into the existing one-shell skeleton (rail + CI-owned contextual sidebar + main content + a
**pre-fetched cross-artifact context pane**); it invents no shell. Every view designs **empty / loading /
error / permission-denied / erased** states (design §5.10) and applies the §8b day-one primitives
(skeletons-not-spinners, system-blamed errors, fail-static degradation, glyph+label status never colour
alone, the `◆ agent` badge with no sparkle/magic iconography, overlays portalled + focus-trapped, each
region its own `min-height:0` scroller). Full wireframes: `../design/wireframes.md`.

| Sidebar entry | View(s) | Backed by | Primary persona |
|---|---|---|---|
| **Runs** | Run list / dashboard ("is main green?"); **Single-run view** (DAG + jump-to-failure); **Live log view** (firehose, secret-masked, collapsible per step); Matrix view | `ci_run`/`ci_job` + `log_anchor`; `project`; `list_objects` pre-filter; firehose `tail` | Engineer |
| **Environments** | Environments & deployments; **Approvals queue** (a HITL surface) | `environment`/`deployment`; `list_subjects(env, approve)`; the durable-signal gate | PM/release-mgr + Engineer |
| **Pipelines** | Pipeline **editor + validator** (schema, lint, `plan` — no runner spend) | the JSON-Schema validator + the resolver (one code path with `myelin ci validate/plan`) | Engineer |
| **Runners** | Runner fleet / self-hosted mgmt (health, capacity, **attestation**) | `runner`; the attestation surface | Admin/platform |
| **Caches & Artifacts** | Caches & artifacts browser (retention, GC, download) | `artifact`/`cache_entry`; `ArtifactRef` | Engineer/admin |
| **Secrets & Variables** | Secrets mgmt (scoped, **audited**, rotation; **never echoes** a value) | `secret_binding` (names + scope only) | Security/admin |
| **Triggers** | Triggers mgmt (the cross-subsystem subscription model) | the armed `EventMatcher`s | Engineer/admin |
| **Usage & Quota** | Usage / quota / billing (**resource-seconds → credits**) | `cost_event` (wholesale) + Commercial markup; OLAP rollup | Admin/finance |
| **(surfaced, not a tab)** | **Agent-surfaced triage** (structured failure + proposed plan-then-apply fix) | `ci.run.failed` structured payload; `EffectApi` plan; provenance/causal-depth | Engineer + agent |
| **Cross-repo** | All Runs ("is everything green?"); **Release Readiness** (PM lens) | OLAP read store; the same data, persona-adaptive lens | Engineer / PM/exec |

**The cross-subsystem surfaces CI *feeds* (never owns):** the Git PR **checks badge** (the merge gate —
the most load-bearing seam; `ci.status.updated`), the issue status ("deployed by RUN-X closes ISSUE-123"),
the chat **run unfurl** + the **HITL approval card**, the knowledge **embed** (a runbook showing a live
deploy), the one **notifications inbox** (failure-on-my-work / deploy-awaiting-me / quota-alert, each
humanised at the backend with "why it fired"), and **search** (ACL-pre-filtered run/log/artifact search).
All via events + `project` + refs.

**Key flows** (`../design/user-flows.md`): push→checks→merge gate; the **agent-triage flagship**
(`ci.run.failed` structured → MockTriageAgent plan → issue/refs/chat → FixAgent → HITL gate → approve →
resume → PR); deploy-gated-on-issue-transition; shift-left validate/plan; self-hosted runner attestation;
GDPR erasure reaching CI; the engineer-vs-PM dual-audience release-readiness split. **The strategy-pattern
payoff:** the exact same UI works whether the runtime is `MockAgentRuntime` today or `LlmAgentRuntime`
later — swapping the runtime changes nothing in the frontend.

## 2. The CLI (a peer surface — design §7.7)

Every primary CI capability has a `myelin ci <verb>`; `--json` on everything (agent/automation use); the
same `ArtifactRef` scheme as the UI (`myelin://…`); routed to the tenant's cell (residency, via
`discover`). The CLI and web view are **two renderings of one design surface** — the run id you watch in
`myelin ci watch` is the chip you click in chat. The CLI is the `ResilientClient` (contract 1.9): it
honours `429 + Retry-After` under shed (02 §2.4).

```
# Run lifecycle
myelin ci run     [--ref <ref>] [--pipeline <id>] [--json]      # manual trigger (Id.check run_pipeline)
myelin ci list    [--branch] [--status] [--actor] [--json]      # list_objects pre-filtered
myelin ci watch   <run>                                          # live, firehose-backed
myelin ci logs    <run> [--job] [--step] [--range L42-L88]       # range-read; archived = T2 range read
myelin ci cancel  <run>                                          # Id.check cancel
myelin ci retry   <run> [--failed-only]

# Shift-left (no runner spend — the cost-saving path)
myelin ci validate [<file>]                                      # JSON-Schema + lint
myelin ci plan     [--ref <ref>]                                 # resolved DAG + matrix + referenced secrets (the CAS snapshot)

# Deployments / HITL
myelin ci deploy          <env> [--ref <ref>]
myelin ci deploy approve  <dep>                                  # = posting the durable `approval` signal
myelin ci deploy rollback <dep>                                  # first-class reversibility (not "are you sure?")

# Fleet / self-hosted (the EU-enterprise path)
myelin ci runner register --pool <p> --labels <l,...>           # → attest → scoped job token (mint_run_token)
myelin ci runner list

# Secrets / usage
myelin ci secret set <name> --scope <env|project>               # never echoes; untrusted_fork resolves to none
myelin ci usage [--period <m>]                                  # resource-seconds → credits; reserve-gate honesty
```

`myelin ci local` (laptop execution) is a **named floor** — not built v1 (UX-win-vs-fidelity-cost; 07 §open).

## 3. The agent-tool surface — `ToolDef` registrations (contract 8.1)

CI registers its actions into the one permissioned `ToolSurface` (`register_tool`), MCP-exposable. Each
declares `required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`. **The
`requires_approval` defaults are CI's per-subsystem HITL product call** (jointly with the Agent Fabric;
Stage-1 open Q5):

| ToolDef | effect_kind | side-effecting | `requires_approval` default | Notes |
|---|---|---|---|---|
| `ci.run` | mutate | yes | **no** (spends budget, gated by reserve) | start a run; reserve gate bounds spend. |
| `ci.cancel_run` | mutate | yes | no | cancel; low-risk, reversible. |
| `ci.retry_run` | mutate | yes | no | re-run; reserve-gated. |
| `ci.read_log` / `ci.read_run` | read | no | no | ACL-checked read (RAG/triage input). |
| `ci.validate` / `ci.plan` | read | no | no | shift-left; **no runner spend**. |
| `ci.deploy` | mutate | yes | **yes on a protected env** (HITL-gated) | a protected-env deploy is a consequential action → the approval card. |
| `ci.approve_deploy` | mutate | yes | **yes — always** (an approval IS the human-in-the-loop) | an agent cannot self-approve its own deploy unless delegation explicitly allows. |
| `ci.rollback` | mutate | yes | **yes on prod** | reversibility, but prod rollback is consequential. |
| `ci.secret_write` | mutate | yes | **yes — always; sensitive** | secret writes are audit-critical; never auto-applied by an agent. |

The approver set for each gate resolves via `Id.list_subjects(<object>, approve)` (contract 4.4). Every
agent-authored CI message/card inherits **NOTIF-1 backend humanisation** (routable `ArtifactRef`s, never
raw ids) and carries the agent label `◆`, the on-behalf-of attribution, the live cost estimate, and the
**causal-depth ceiling** (the loop guard visible). **`ToolHands::exec` is *not* in this table** — it is the
runner itself (the `kind=agent` job), the deepest unification, never a side-effecting tool (05 §HP-5).

## 4. The public/internal API split (contract 1.2)

- **Public surface** (gateway-fronted, identity-injected): the CLI/web/MCP entrypoints above; tenant from
  the verified token, never the path; every call runs `Id.check`; under load the public surface sheds in
  the protected-human-lane order (interactive human-last).
- **Internal RPC** (the trust boundary): the scheduler ↔ runner-agent lease/heartbeat/report, the
  fleet-autoscaler ↔ `FleetProvider`, the secret broker ↔ the shared secret store, the log-pipeline ↔
  firehose/Storage. The sandbox is the **hard isolation boundary** below even the internal RPC — a job
  inside a sandbox reaches *nothing* on the internal RPC (egress default-deny; the escape drill proves it).
- **Metrics/health** (contract 1.3): liveness must not check deps; readiness gates on DB pool + broker +
  authz reachability + at-least-one-healthy-runner-pool. The telemetry set (contract 1.8) includes
  scheduler queue-depth, claim latency, lease-reap count, fleet capacity, shed counts, and causal-depth —
  the Phase-5 drill survival signals.
