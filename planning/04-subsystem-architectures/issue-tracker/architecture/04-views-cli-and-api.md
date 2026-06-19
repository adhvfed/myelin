# Issue Tracker — 04 · Views, CLI & API / Tool Surface

> See [`00-overview.md`](./00-overview.md) for the role and [`../design/`](../design/) for the full IA, user
> flows, and wireframes (PRESERVED) this doc maps to the data model. This doc fixes the **view catalogue** (each
> view = a frozen `myelin-query` `ViewSpec` projection over the one `issue` table — the structural proof of
> board↔roadmap co-equality), the **`myelin issue` CLI** (a peer surface against the same API the UI uses), and
> the **public/agent API + tool surface**. The UI, CLI, and agent ToolDefs all hit **one API** — no privileged
> back-channel (design-language §7.7; ADR-08).

---

## 1. The views — one component, many `ViewSpec` projections (design-language §5.6/§7.3)

Every Issues view is a **saved frozen `myelin-query` `ViewSpec`** (contract 13.3 — `{kind, filter:QueryAst,
group_by, sort, visible, order_field:order_key}`) over the **one `issue` table** — the same shared view primitive
Knowledge uses (`db_view`). This is the structural core of the platform bet: the board and the roadmap are **not
two object graphs**, they are **two `ViewSpec`s over the same rows** (sketch 01; [05 §1](./05-hard-problems.md)).
Editing an issue on the board patches the roadmap live because they read the same `issue` rows — **no parallel
reality**.

| View (screen) | The `ViewSpec` projection | `kind` | Persona default |
|---|---|---|---|
| **Board / Kanban** (S3) | `filter type_rank ≤ 1 ∧ cycle:current`, `group_by state_category`, `sort order_key` | board | engineer |
| **Roadmap / Timeline** (S5) | `filter type_rank ≥ 2`, on a date axis (`earliest_start … latest_due` from the rollup), dependency overlay | timeline | PM/exec |
| **Backlog** (S6) | `filter state_category:unstarted`, `order_field order_key` (drag-to-rank, frozen `order_key` + CAS) | list | engineer/PM |
| **List** (S2) | any `QueryAst` filter, `sort` | list (`j/k`, single-key actions) | engineer |
| **Table / spreadsheet** (S4) | any `QueryAst` filter, `visible`/hidden fields, inline-edit | table | PM/corporate |
| **Calendar** (S7) | `group_by` a date field | calendar | PM |
| **Cycle / Sprint** (S8) | `filter cycle:N`, burndown chart (OLAP-fed) | board + charts | engineer/PM |
| **Triage** (S9) | `filter state:triage`, agent-suggestion strip | list + agent card | engineer |
| **My Work** (S10) | **a `list_inbox(principal, filter)` over the ONE Notif inbox** (C-9, contract 7.1) — assigned/blocked/needs-approval/overdue | inbox groups | all |
| **Dashboards / Reports** (S11) | OLAP queries (CFD, cycle-time, velocity, SLA-compliance), restriction-flag-honouring | charts | PM/exec |

**Persona-adaptive default landing** (design-language §2): an engineer lands on My Work / Active cycle (board); a
PM lands on Roadmap; the lens is a **per-role preference, never a lock** — switching is a view change, not a fork.
Vocabulary translates per space ("issue" ↔ "work item" ↔ "deliverable") as a presentation choice over one model.
Density (engineer = compact/keyboard-forward; PM/exec = spacious/chart-forward) is a density-token choice over one
component.

**Views are per-user-overridable vs shared** (matching Knowledge's `db_view` + `db_view_override`): a shared base
`ViewSpec` with optional personal tweaks layered on top. Every view query **always conjoins
`list_objects(viewer, 'view', 'issue')`** (the `SetExpr` push-down, [03 §6.2](./03-events-contracts-and-glue.md))
so a viewer sees only rows they may read — never post-filtered. A confidential issue is simply absent — **no "N
hidden" leak** (S3 wireframe; D3).

### 1.1 The admin/governance views (the schemes made editable)

| View (screen) | Edits | Primitive |
|---|---|---|
| **Workflow / scheme editor** (S13) | states + transitions + guards + post-actions; the **fixed category mapping** validated | state-graph editor + the **frozen `QueryAst` guard builder** (no scripting); unreachable-state flagged inline |
| **SLA policy editor** (S14) | policy (metric/target/calendar/pause/escalation) + the **calendar editor** + breach-simulation preview | calendar editor + the frozen escalation-chain builder (contract 7.5) |
| **Team / Project settings** (S15) | members, prefix, scheme assignments, the **permission inspector** | forms + `list_subjects`/`explain` (the ReBAC "why", contract 4.4) |
| **Automation / trigger builder** (S16) | automations (stateless reflex) + triggers (stateful promise, the frozen `QueryAst` condition) + agent-handler picker + HITL config | `QueryAst` builder + the ToolDef picker |
| **Import wizard** (S17) | connect → map → dry-run → run; the **reconciliation report** (lossy/dropped named per the frozen ADF map, contract 13.2) | stepper + mapping preview |
| **Audit / change-history** (S18) | the per-issue change-log + the tamper-evident audit log | timeline + actor/agent badges (Issues contributes attribution, not the log — contract 10.6) |

---

## 2. The CLI — `myelin issue …` (a peer surface; design-language §7.7)

Every primary capability is a CLI verb against the **same API the UI uses** (no privileged back-channel), sharing
the `QueryAst` and the canonical `ArtifactRef`. `--json` everywhere for agents/scripts.

```
# create / read / update
myelin issue create --title "Login 500 on SSO" --type bug --project ENG --assignee @alice
myelin issue show ENG-1421 [--json]                       # = project(ref, me) + relations + activity
myelin issue update ENG-1421 --priority high --field severity=S1
myelin issue transition ENG-1421 --to "In Review"         # runs the workflow interpreter (QueryAst guards, category)
myelin issue assign ENG-1421 @bob
myelin issue comment ENG-1421 --body "repro on staging @bob #ENG-1390"

# relations (the TE-7 typed table)
myelin issue link ENG-1421 --blocked-by ENG-1490
myelin issue link ENG-1421 --parent ENG-1390              # containment (rank-monotonic)
myelin issue link ENG-1421 --closes myelin://…/git/pr/88

# ranking / cycles / roadmap
myelin issue reorder ENG-1441 --after ENG-1440            # frozen order_key + CAS
myelin cycle plan --cycle 14 --add ENG-1440,ENG-1441
myelin cycle complete 14                                  # carry-over prompt; emits cycle.completed → OLAP

# queries / views (the shared QueryAst)
myelin issue list 'state:open assignee:me cycle:current'  # the same AST as ⌘K (S19) and saved views
myelin view create "Open bugs" 'type:bug state:open' --as board
myelin report cycle-time --project ENG --since 2026-Q1    # OLAP-fed

# triggers / SLA / governance
myelin issue remind ENG-1421 --when unblocked --stale-after 30d   # the stateful Trigger flagship (QueryAst condition)
myelin sla show ENG-1455
myelin issue scheme assign --workflow Support --type bug --project PAY

# import / export (the round-trip; the frozen ADF map)
myelin issue import --from jira --dry-run                 # → the reconciliation report (lossy/dropped named)
myelin issue import --from jira --run --resume            # idempotent, resumable
myelin export --format canonical --scope project:ENG      # round-trips with import
```

The CLI noun alias is render-time (`issue` is the canonical token; index §14). The chip you see and the handle you
paste are the same `ArtifactRef` (`myelin://…/issue/issue/ENG-1421`); `#1421` is the render-time display form
(Δ3).

---

## 3. The public / agent API + tool surface

One public RPC surface (gateway-fronted, identity-injected — the three-surface topology, contract 1.2), consumed
identically by the UI, the CLI, and agents (via the `ToolDef` catalogue + MCP, contract 8.1). The shape:

| Method | Contract tie |
|---|---|
| `create / update / transition / assign / comment / link / reorder / estimate / close` | the write path ([02 §2/§5](./02-internals-and-algorithms.md)); each = a `ToolDef` ([03 §8](./03-events-contracts-and-glue.md)) |
| `query(ast, viewer, page)` | the planner ([02 §3](./02-internals-and-algorithms.md)); ACL-pre-filtered via the `SetExpr` push-down |
| `show(ref, viewer)` | = `project(ref, viewer)` + relations + activity (contract 5.6) |
| `remind(subject, condition, stale_after)` / `unarm(trigger)` | the stateful Trigger (the frozen `QueryAst` condition, [03 §10](./03-events-contracts-and-glue.md)) |
| `scheme.assign / scheme.define` | the governance config ([01 §3](./01-tech-and-data-model.md)) |
| `import.dry_run / import.run(resume) / export` | the import engine (the frozen ADF map, sketch 09) |
| `report(metric, scope)` | OLAP read store (contract 11.6), restriction-flag-honouring |

**Agent surface:** agents act through the **same** `ToolDef`s + `EffectApi` (plan-then-apply; no carve-out, the
frozen `requires_approval` defaults). `run --dry-run` returns proposed effects without applying (the
triage/forecast agents' proposals — S9/S5). Side-effecting tools pass schema → capability → delegation → tenant →
budget → HITL gate before applying via the public endpoint. Every applied effect is attributed to the agent +
audit-linked (one `correlation_id` threading the flow; loop-depth capped — AG-6); agent vs human is rendered
distinctly (AI-Act labelling, contract 8.x).

**The latency budget** (design-language §8b.6): the command palette / quick-create responds < ~100ms; results
stream in under the input (no blank flash < 1s); a board/list query is under the <1s keyboard budget (the Tier-1
index range + the `SetExpr` JOIN; D2). Every overlay (quick-create, guard popover, assign dropdown, palette,
approval card) portals to root, shares one z-index scale, centralises focus-trap/Escape/ARIA, and flips when
off-screen.

Continue to [`05-hard-problems.md`](./05-hard-problems.md).
