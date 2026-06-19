# CI/CD — Information Architecture

> Phase 4 — CI design sketch (REQUIRED before architecture; VISION §3/§5.2). The screen/navigation
> structure for the CI subsystem, fitting the **one-shell** design language (rail + contextual nav +
> header — design-language §5.1) and the **§7.2 view catalogue**. Every screen here inherits the shared
> components (§5), tokens (§3), accessibility baseline (§4), the agent surfaces (§6), and must design
> its **empty / loading / error / permission-denied / erased** states (§5.10).

---

## 1. Where CI lives in the one shell (design-language §5.1)

CI is one entry in the **primary rail** — `Code · CI · Issues · Knowledge · Chat · Inbox · Search`. CI
does **not** invent its own shell; it composes into the existing skeleton:

```
┌──────────────────────────────────────────────────────────────────────────────────┐
│ HEADER:  [tenant/space ▾]   ⌘K command palette   [global search]   inbox •  [you ▾]│  ← shell-owned
├────────┬─────────────────────────────────────────────────────────┬───────────────┤
│ RAIL   │ CONTEXTUAL SIDEBAR (CI-owned)                            │ CONTEXT PANE   │
│ Code   │  ── Repo scope: acme/web ▾ ──                            │  (cross-artifact│
│ ▸CI    │   Runs            (run list / dashboard)                 │   references,   │
│ Issues │   Environments    (deploys, approvals queue)             │   pre-fetched   │
│ Know.. │   Pipelines       (definitions / editor)                │   per §5.3 /    │
│ Chat   │   Runners         (fleet / self-hosted)                  │   "the system   │
│ Inbox  │   Caches & Artifacts                                     │   assembles     │
│ Search │   Secrets & Variables                                    │   context")     │
│        │   Triggers                                               │                 │
│        │   Usage & Quota                                          │                 │
│        │  ── Cross-repo ──                                        │                 │
│        │   All Runs   ·   Release Readiness                       │                 │
│        │                                                          │                 │
│        │  MAIN CONTENT AREA (the active view)                     │                 │
└────────┴──────────────────────────────────────────────────────────┴───────────────┘
```

- **Rail** = shell-owned (subsystem switch). **Contextual sidebar** = CI-owned (the CI nav tree).
  **Main content** = the active CI view. **Right context pane** = the cross-artifact references for the
  focused artifact (a run's PR/issue/chat/commit edges, §5.3) — pre-fetched, not a tab the user assembles.
- **Two scopes** in the sidebar: **repo-scoped** (the common case — runs/pipelines for `acme/web`) and
  **cross-repo** (the "is everything green?" + PM "release readiness" rollup). The scope selector at the
  sidebar top mirrors Code's repo selector for muscle-memory (P1 coherence).

## 2. The CI view inventory → §7.2 catalogue mapping

Every §7.2 view, placed in the IA, with its primary purpose and the personas it serves:

| Sidebar entry | View(s) | §7.2 catalogue item | Primary persona |
|---|---|---|---|
| **Runs** | Run list / dashboard; **Single-run view** (DAG); **Live log view**; **Matrix view** | Run list, Single-run, Live log, Matrix | Engineer (P1–P5) |
| **Environments** | Environments & deployments; **Approvals queue** (HITL surface) | Environments & deployments | PM/release-mgr (P6/P8) + Engineer |
| **Pipelines** | Pipeline / definition **editor + validator** (schema, lint, `plan`) | Pipeline editor + validator | Engineer |
| **Runners** | Runner fleet / self-hosted mgmt (health, capacity, attestation) | Runner fleet | Admin/platform (P15) |
| **Caches & Artifacts** | Caches & artifacts browser (retention, GC, download) | Caches & artifacts browser | Engineer/admin |
| **Secrets & Variables** | Secrets mgmt (scoped, **audited**, rotation; never echoes) | Secrets management | Security/admin (P12/P15) |
| **Triggers** | Triggers mgmt (the cross-subsystem subscription model) | Triggers management | Engineer/admin |
| **Usage & Quota** | Usage / quota / billing (resource-seconds → credits) | Usage / quota / billing | Admin/finance (P11/P15) |
| **(surfaced, not a sidebar tab)** | **Agent-surfaced triage** (failure-structured + proposed fix) | Agent-surfaced triage view | Engineer + agent |
| **Cross-repo: Release Readiness** | PM-friendly deploy/health rollup | Environments (PM lens, §2) | PM/exec (P6/P8/P11) |

## 3. Navigation depth & deep-linking (ADR-13 `ArtifactRef` down to sub-artifact)

Every CI artifact is deep-linkable down to **sub-artifact granularity** (design-language §5.1; contract
5.7) so chat/issues/docs can reference it and the command palette can jump to it:

```
myelin://<tenant>/ci/run/<run-id>                      → Single-run view
myelin://<tenant>/ci/run/<run-id>#job-<job-id>         → run view, that job focused
myelin://<tenant>/ci/run/<run-id>#step-<n>             → live-log view, that step expanded
myelin://<tenant>/ci/run/<run-id>#step-<n>#L42-L88     → log view, scrolled + range-highlighted (jump-to-failure)
myelin://<tenant>/ci/deployment/<dep-id>               → Environments view, that deploy focused
myelin://<tenant>/ci/pipeline/<pipeline-id>            → Pipeline editor
```

The `#sub` ids (`#job-…`, `#step-…`, `#L…`) are **stable across edits/retries** (contract 5.7 — CI's
obligation) so an embed in a runbook or a chat unfurl never dangles. These are the same anchors the
**`project(ref, viewer)` projection** returns (`sub_anchor`), so a `#step-3` reference unfurls to the
right step everywhere.

## 4. Cross-subsystem surfacing (CI feeds views CI doesn't own)

CI is the platform's connective hub for "did the work pass?". It **feeds** (never owns) these
surfaces, via events + the `project` projection + refs:

- **Git PR view** — the **checks badge** / required-checks summary (the merge gate); `ci.status.updated`
  drives it. *The single most load-bearing cross-subsystem seam* (CI ↔ Git, Phase-2 §10).
- **Issue detail** — CI status on a linked issue ("deployed by RUN-X closes ISSUE-123").
- **Chat** — run **unfurls** (live, permission-aware, with inline re-run/cancel actions, §5.3); the
  **HITL approval card** for deploy gates renders in chat (the approval-card surface, §6.3).
- **Knowledge** — a runbook embeds a live CI run / deploy status (reference node, §5.9).
- **Notifications inbox** — run failure on my work, deploy-approval-awaiting-me, quota alert → the **one
  inbox** (§5.8); humanised at the backend (NOTIF-1), each carrying "why it fired".
- **Search** — runs/logs/artifacts, ACL-pre-filtered ("find the run where test X first failed", §5.7).

## 5. The CLI as a peer surface (design-language §7.7)

Every primary CI capability has a `myelin ci <verb>` (Phase-2 §5), `--json` on everything for agent/
automation use, same `ArtifactRef` scheme as the UI (`myelin://…`), routed to the tenant's cell
(residency). The CLI and web view are **two renderings of one design surface** — the run id you watch
in `myelin ci watch` is the chip you click in chat.

## 6. Density, responsive & persona-adaptivity

- **Density (P5):** the run list, matrix grid, and log view are density-sensitive — **compact mode**
  for engineers, comfortable default; the Release Readiness rollup is spacious + chart-forward for PMs
  (the same persona-adaptive principle as §2, applied to CI's dual audience: engineer "is main green?"
  vs PM "are we ready to ship?").
- **Responsive (design-language §8b.4):** the rail + contextual sidebar collapse to toggled drawers on
  mobile (backdrop + Escape + route-change auto-close); the live-log and matrix views own their own
  scroller (`min-height:0` flex children) so the log never pushes the page below the fold; row actions
  on the run list are surfaced by default (hover-is-not-touch). Primary mobile target is **read/watch**
  (watch a run, approve a deploy from the inbox), not authoring.
