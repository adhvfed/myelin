# CI/CD — 04 Views, CLI & API (the agent-tool surface)

> Phase 5-B — CI detailed architecture (rewritten against the reconciled layer). The view inventory
> (referencing `../design/`), the CLI command surface (a peer rendering, not a second-class afterthought),
> and the API / agent-tool surface (the `ToolDef` registrations with the **frozen X-6 `requires_approval`
> defaults**). The design sketches (`../design/`) are the authoritative, **preserved** wireframes; this doc
> grounds them in the data model + the frozen contracts and fixes the tool surface.

---

## 1. Views (the §7.2 catalogue, placed in the one shell)

CI is one entry in the **primary rail** (`Code · CI · Issues · Knowledge · Chat · Inbox · Search`). It
composes into the existing one-shell skeleton (rail + CI-owned contextual sidebar + main content + a
**pre-fetched cross-artifact context pane**); it invents no shell. Every view designs **empty / loading /
error / permission-denied / erased** states (design §5.10) and applies the day-one primitives
(skeletons-not-spinners, system-blamed errors, fail-static degradation, glyph+label status never colour
alone, the `◆ agent` badge with no sparkle/magic iconography, overlays portalled + focus-trapped, each region
its own `min-height:0` scroller). Full wireframes: `../design/wireframes.md`.

| Sidebar entry | View(s) | Backed by | Primary persona |
|---|---|---|---|
| **Runs** | Run list / dashboard ("is main green?"); **Single-run view** (DAG + jump-to-failure via `#step-<n>`); **Live log view** (firehose + **resume-cursor**, secret-masked, collapsible per step); Matrix view | `ci_run`/`ci_job` + `log_anchor`; `project`; **`list_objects` `SetExpr` push-down over `run_id`**; firehose `subscribe/resume` (OQ-J) | Engineer |
| **Environments** | Environments & deployments; **Approvals queue** (a HITL surface) | `environment`/`deployment`; `list_subjects(env, approve)`; the durable-signal gate (per-effect `idem_key`) | PM/release-mgr + Engineer |
| **Pipelines** | Pipeline **editor + validator** (schema, lint, `plan` — no runner spend) | the JSON-Schema validator + the resolver (one code path with `myelin ci validate/plan`) | Engineer |
| **Runners** | Runner fleet / self-hosted mgmt (health, capacity, **attestation**) | `runner`; the attestation surface | Admin/platform |
| **Caches & Artifacts** | Caches & artifacts browser (retention, GC, download; **trust-scoped namespaces visible**) | `artifact`/`cache_entry`; `ArtifactRef` | Engineer/admin |
| **Secrets & Variables** | Secrets mgmt (scoped, **audited**, rotation; **never echoes** a value) | `secret_binding` (names + scope only) | Security/admin |
| **Triggers** | Triggers mgmt (the cross-subsystem subscription model) | the armed `QueryAst` matchers | Engineer/admin |
| **Usage & Quota** | Usage / quota / billing (**resource-seconds → credits**) | `cost_event` (wholesale) + Commercial markup; OLAP rollup | Admin/finance |
| **(surfaced, not a tab)** | **Agent-surfaced triage** (structured failure + proposed plan-then-apply fix) | `ci.run.failed` structured payload; `EffectApi` plan; provenance/causal-depth | Engineer + agent |
| **Cross-repo** | All Runs ("is everything green?"); **Release Readiness** (PM lens) | OLAP read store; the same data, persona-adaptive lens | Engineer / PM/exec |

**The cross-subsystem surfaces CI *feeds* (never owns):** the Git PR **checks badge** (the merge gate — the
most load-bearing seam; CI emits **`ci.check.updated`** with `CheckStatus`, **Git** owns the projection +
gate, X-1), the issue status ("deployed by RUN-X closes ISSUE-123"), the chat **run unfurl** + the **HITL
approval card**, the knowledge **embed** (a runbook showing a live deploy), the one **notifications inbox**
(failure-on-my-work / deploy-awaiting-me / quota-alert, each humanised at the backend via NOTIF-1 with "why
it fired"), and **search** (ACL-pre-filtered run/log/artifact search). All via events + `project` + refs.

**Key flows** (`../design/user-flows.md`): push→checks→merge gate; the **agent-triage flagship**
(`ci.run.failed` structured → MockTriageAgent plan → issue/refs/chat → FixAgent → HITL gate → approve →
resume → PR); deploy-gated-on-issue-transition; shift-left validate/plan; self-hosted runner attestation;
GDPR erasure reaching CI; the engineer-vs-PM dual-audience release-readiness split. **The strategy-pattern
payoff:** the exact same UI works whether the runtime is `MockAgentRuntime` today or `LlmAgentRuntime` later
— swapping the runtime changes nothing in the frontend.

## 2. The CLI (a peer surface — design §7.7)

Every primary CI capability has a `myelin ci <verb>`; `--json` on everything (agent/automation use); the same
`ArtifactRef` scheme as the UI (`myelin://…`); routed to the tenant's cell (residency, via `discover`). The
CLI and web view are **two renderings of one design surface** — the run id you watch in `myelin ci watch` is
the chip you click in chat. The CLI is the `ResilientClient` (contract 1.9): it honours `429 + Retry-After`
under shed (02 §2.4).

```
# Run lifecycle
myelin ci run     [--ref <ref>] [--pipeline <id>] [--json]      # manual trigger (Id.check run_pipeline)
myelin ci list    [--branch] [--status] [--actor] [--json]      # list_objects SetExpr push-down over run_id
myelin ci watch   <run>                                          # live, firehose + resume-cursor (loses zero lines on reconnect)
myelin ci logs    <run> [--job] [--step] [--range L42-L88]       # range-read; archived = T2 (job,step,byte-range) read
myelin ci cancel  <run>                                          # Id.check cancel
myelin ci retry   <run> [--failed-only]                          # bumps run_attempt per re-run context (supersedes in Git's gate)

# Shift-left (no runner spend — the cost-saving path)
myelin ci validate [<file>]                                      # JSON-Schema + lint
myelin ci plan     [--ref <ref>]                                 # resolved DAG + matrix + referenced secrets (the CAS snapshot)

# Deployments / HITL
myelin ci deploy          <env> [--ref <ref>]
myelin ci deploy approve  <dep>                                  # = posting the durable `approval` signal (idem_key = card_id[:effect_idx])
myelin ci deploy rollback <dep>                                  # first-class reversibility (not "are you sure?")

# Fleet / self-hosted (the EU-enterprise path)
myelin ci runner register --pool <p> --labels <l,...>           # → attest → scoped job token (mint_run_token, tenant-SelfHosted-scoped)
myelin ci runner list

# Secrets / usage
myelin ci secret set <name> --scope <env|project>               # never echoes; untrusted_fork resolves to none (ABAC edge)
myelin ci usage [--period <m>]                                  # resource-seconds → credits; reserve-gate honesty
```

`myelin ci local` (laptop execution) is a **named floor** — not built v1 (UX-win-vs-fidelity-cost; 07 §open).

## 3. The agent-tool surface — `ToolDef` registrations (contract 8.1, X-6 defaults FROZEN)

CI registers its actions into the one permissioned `ToolSurface` (`register_tool`), MCP-exposable. Each
declares `required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`. **The
`requires_approval` defaults are now the FROZEN X-6 table** (deploy/secret = yes; reversible/cheap = no) —
no longer a Phase-4 CI-local product call but a frozen contract-index value (contract 8.1):

| ToolDef | effect_kind | side-effecting | `requires_approval` (X-6 frozen) | Notes |
|---|---|---|---|---|
| `ci.run` / `run_pipeline` (non-prod) | mutate | yes | **no** (cheap, reversible, metered) | start a run; reserve gate bounds spend. |
| `ci.cancel_run` | mutate | yes | no | cancel; low-risk, reversible. |
| `ci.retry_run` | mutate | yes | no | re-run; reserve-gated; bumps `run_attempt`. |
| `ci.read_log` / `ci.read_run` | read | no | no | ACL-checked read (RAG/triage input). |
| `ci.validate` / `ci.plan` | read | no | no | shift-left; **no runner spend**. |
| `ci.deploy` (protected env) | mutate | yes | **yes** | protected-env deploy is consequential → the approval card (CI HITL gate). |
| `ci.approve_deploy` | mutate | yes | **yes** | secret-write + approval are privileged; an agent cannot self-approve unless delegation explicitly allows. |
| `ci.rollback` (prod) | mutate | yes | **yes** | reversibility, but prod rollback is consequential. |
| `ci.write_secret` | mutate | yes | **yes** | secret writes are audit-critical; never auto-applied by an agent. |

The approver set for each gate resolves via `Id.list_subjects(<object>, approve)` (contract 4.4). Every
agent-authored CI message/card inherits **NOTIF-1 backend humanisation** (the one templating surface,
contract 7.3 — routable `ArtifactRef`s, never raw ids) and carries the agent label `◆`, the on-behalf-of
attribution, the live cost estimate, and the **causal-depth ceiling** (the loop guard visible). A batch
approval card (e.g. "approve these 3 deploys") uses the **per-effect `idem_key`** rule
(`card_id:<effect_idx>`, OQ-F): a double-click is one approval; a partial approval is well-defined; a declined
effect is **withheld** (returns `Denied`, never mutates). **`ToolHands::exec` is *not* in this table** — it is
the runner itself (the `kind=agent` job), the deepest unification, never a side-effecting tool (05 §HP-5).

## 4. The public/internal API split (contract 1.2)

- **Public surface** (gateway-fronted, identity-injected): the CLI/web/MCP entrypoints above; tenant from the
  verified token, never the path; every call runs `Id.check`; under load the public surface sheds in the
  protected-human-lane order (interactive human-last; the CI-dispatch per-surface shed budget, OQ-K).
- **Internal RPC** (the trust boundary): the scheduler ↔ runner-agent lease/heartbeat/`job.done`-report, the
  fleet-autoscaler ↔ `FleetProvider`, the secret broker ↔ the shared secret store, the log-pipeline ↔
  firehose/Storage, the check-emitter ↔ outbox. The sandbox is the **hard isolation boundary** below even the
  internal RPC — a job inside a sandbox reaches *nothing* on the internal RPC (egress default-deny; the escape
  drill proves it).
- **Metrics/health** (contract 1.3): liveness must not check deps; readiness gates on DB pool + broker +
  authz reachability + at-least-one-healthy-runner-pool. The telemetry set (contract 1.8) includes scheduler
  queue-depth, claim latency, lease-reap count, fleet capacity, shed counts, consumer-lag, outbox-depth, and
  causal-depth — the Phase-5 drill survival signals.
