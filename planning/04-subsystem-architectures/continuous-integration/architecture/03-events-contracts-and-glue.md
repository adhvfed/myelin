# CI/CD — 03 Events, Contracts & Glue

> Phase 4 — CI Stage-2. The **complete `ci.*` event taxonomy** CI owns (under the Bus §6 grammar), the
> events CI **consumes**, and **how CI implements every glue contract**: `ArtifactRef` + the `#sub`
> scheme, `project(ref, viewer)`, `replay(scope, since)`, the envelope via the OUTBOX, Identity
> `check`/`list_objects` + the ReBAC namespace fragment, `PersonalDataHolder`, `IndexSpec`, the `ToolDef`
> registrations, and reserve/settle. This is CI's Bus-§6 completion obligation + the README §5 "every
> subsystem must implement" list, made concrete.

---

## 1. The `ci.*` event taxonomy (CI OWNS this complete list)

Grammar (Bus §6 / contract 2.9): `<subsystem>.<artifact_type>.<event_name>` — **singular, past-tense,
dotted**. Subsystem token = `ci` (canonical, Bus §6.2). Every event is the canonical `EventEnvelope`
(contract 2.1), emitted **only via the transactional outbox** (`OutboxTx::emit(draft, cause)`, contract
2.2) in the same tx as the state change; causality (`correlation_id`/`causation_id`/`depth`) is derived
correct-by-construction.

### 1.1 Lifecycle events (run / job / deployment)

| Event | Aggregate (ordering key) | Notes |
|---|---|---|
| `ci.run.started` | `ci/run/<run_id>` | a run began; carries trust_tier, trigger_kind, the CAS snapshot ref. |
| `ci.run.succeeded` / `ci.run.failed` / `ci.run.cancelled` / `ci.run.timed_out` | `ci/run/<run_id>` | terminal. **`ci.run.failed` carries *structured* failure** (which step, which test, log excerpt) — the agent-native triage hook (deliberate, not a blob). |
| `ci.run.reaped` | `ci/run/<run_id>` | a runner died mid-job and the run was re-queued/failed — **honest, never silent**. |
| `ci.job.started` / `ci.job.succeeded` / `ci.job.failed` / `ci.job.cancelled` | `ci/run/<run_id>` | per-job; job ordering is within the run aggregate. |
| **`ci.status.updated`** | `ci/run/<run_id>` | the **commit-status / check** event Git consumes for the merge gate — carries `{commit_oid, context, state ∈ pending/success/failure/error, run_ref, target_url}`. **The Git↔CI seam** (§4). |
| `ci.run.passed` / `ci.run.failed` (check semantics) | `ci/run/<run_id>` | the check-result shape Git's merge-queue/merge-gate consumes (jointly owned — git `03 §1.1`, `06 §CR-CI`). |
| `ci.deployment.requested` / `ci.deployment.approval_required` / `ci.deployment.approved` / `ci.deployment.rejected` | `ci/deployment/<dep_id>` | the protected-env HITL flow (`approval_required` opens the gate; `approved` is the durable signal landing). |
| `ci.deployment.started` / `ci.deployment.succeeded` / `ci.deployment.failed` / `ci.deployment.rolled_back` | `ci/deployment/<dep_id>` | deploy lifecycle; `rolled_back` is first-class (reversibility). |

### 1.2 Pointer & resource events

| Event | Aggregate | Notes |
|---|---|---|
| **`ci.log.available`** | `ci/run/<run_id>` | **the ONLY log-related durable event** — a *pointer* ("lines N..M of `run/job/step` ready at `<ArtifactRef>`"). Logs themselves ride the **firehose** (`ci.log.appended` frames, NOT the durable bus). **CI owns this pointer taxonomy.** Coalesced, never per-line (02 §7.1). |
| `ci.artifact.published` | `ci/run/<run_id>` | a retained artifact (binary/SBOM/report/SCIP-LSIF) is available at an `ArtifactRef`; carries SLSA provenance ref. Git/Search consume (find-usages, GF-3 follow-on). |
| `ci.cost.metered` | `ci/run/<run_id>` | one metered unit (resource-seconds); wholesale + markup separate. Commercial/OLAP consume for usage rollups. |

### 1.3 Fleet / config / supply-chain events

| Event | Aggregate | Notes |
|---|---|---|
| `ci.runner.registered` / `ci.runner.attested` / `ci.runner.degraded` / `ci.runner.offline` | `ci/runner/<runner_id>` | fleet health + the self-hosted attestation surface (the runner-fleet view). |
| `ci.pipeline.created` / `ci.pipeline.updated` / `ci.pipeline.validated` | `ci/pipeline/<pipeline_id>` | config-as-code lifecycle (`validated` = a `plan` succeeded). |
| `ci.supply_chain.verification_failed` | `ci/run/<run_id>` | a floating-tag / unsigned-component / failed-signature was **refused** (audit-critical; the fail-closed proof). |

### 1.4 Cross-cutting (erasure + reindex) — required of every subsystem

| Event | Aggregate | Notes |
|---|---|---|
| `ci.run.erased` / `ci.deployment.erased` / `ci.runner.erased` | the erased aggregate | the `*.erased` tombstone (Bus §6.3) — degrades unfurls to a tombstone, never a dangling leak (§6). |
| `ci.run.snapshot` / `ci.deployment.snapshot` / `ci.pipeline.snapshot` | the snapshotted aggregate | the `*.snapshot` reindex-from-source events for `replay` (Search/Refs/OLAP cold rebuild; §5.3). Must be **sub-artifact-granular** (contract 2.6). |

## 2. Events CONSUMED (idempotent on `event_id`, the `consumer_dedup` ledger)

| Event | From | Effect |
|---|---|---|
| `git.ref.updated` | Git | match `on: push` triggers → resolve + start a run. |
| `git.pull_request.synchronized` / `git.pr.opened` | Git | match `on: pull_request` triggers; trust-tier = fork→`UntrustedFork`. |
| `issue.transitioned` / `issue.issue.closed` | Issues | match `on: issue.transitioned` triggers (the deploy-gated-on-issue flow). |
| `identity.permission.granted\|revoked` / `identity.member.*` | Identity | invalidate the approver/secret-scope resolution caches (who can approve a deploy / read a secret). |
| `*.erased` (subject) | GDPR/Bus DSR fan-out | the erasure path → crypto-shred + tombstone (§6). |
| Agent `ProposedEffect`s (re-run / cancel / deploy / approve) | Agent Fabric | arrive via `EffectApi` (plan-then-apply), **never as direct writes** (§7). |
| (schedule timers) | `myelin-flow` | cron-style `on: schedule` triggers fire as durable timers. |

## 3. The envelope via the OUTBOX (contract 2.1 / 2.2)

CI is a state-changing emitter: **every** `ci.*` event is drafted into the per-service `outbox` table in
the **same transaction** as the run/job/deploy state change, drained by the relay (`FOR UPDATE SKIP
LOCKED`), deduped broker-side on `event_id` (ULID). There is **no `publish_now`** anywhere in CI (the
`no-raw-publish` lint). The activity-completion / terminal-signal path threads the `idem_token` into any
downstream emit so an at-least-once activity retry never double-emits (`ci.deployment.succeeded` is
emitted at-most-once-in-effect). The **audit append** is itself a bus consumer of CI's events — CI does not
write the audit log directly.

## 4. The Git↔CI checks/merge-gate contract (the tightest seam — jointly owned)

This is the single most load-bearing cross-subsystem seam (git `03 §1.1`; `06 §CR-CI`). The shape:

- **CI emits** `ci.run.started` → `ci.status.updated{state: pending}`; on terminal,
  `ci.run.passed`/`ci.run.failed` + `ci.status.updated{state: success|failure, commit_oid, context}`.
- **Git consumes** these to **update `check_status`** (keyed on `commit_oid` + `context`), **feed the
  merge gate** (required-checks-green is one of the gate's conditions, git `02 §6.1` step 3), and **signal
  the merge-queue workflow** (the `ci.result` durable signal wakes Git's merge-queue `DurableExecutor`
  run, possibly after a multi-day HITL gate; git `02 §6.2`, CR-WF-1).
- **Idempotency / ordering:** `ci.status.updated` is keyed on `(commit_oid, context)` so a re-delivered
  or out-of-order status is last-writer-wins per context; Git dedups on `event_id`. A re-run of a check
  (new `run_id`, same `commit_oid`+`context`) supersedes the prior status.
- **Agent-vs-human at the gate is Git's, not CI's:** if an agent requests the *merge*, Git's merge gate
  enforces `agent_needs_human` (git `02 §6.1` step 5); CI only reports check results — it never merges.

The reciprocal **CI→Git** read is the trust-tier evaluation (CI calls `Id.check` to classify
fork/member; §5) and the workspace checkout (the runner uses a scoped job token over the git wire). CI
does **not** read Git's DB (the `no-cross-db` lint); it consumes events + calls `project`.

## 5. Identity: `check` / `list_objects` + the ReBAC namespace fragment (contracts 4.2/4.3/4.9)

### 5.1 Where CI calls Identity

`Id.check(subject, permission, object, zookie?)` runs at **every** CI write/read entrypoint
(fail-closed on uncertainty): start/cancel/retry a run, view a run/log/artifact, approve a deploy,
read/write a secret, register a runner, edit a pipeline. `Id.list_objects(viewer, read, ci_run)` is the
**leak-free pre-filter** for every CI list/search (the run list, "all runs", release readiness) — the
filter is composed *before* scoring/ranking (the `search-requires-acl-filter` lint; S-10 push-down over
the `run_id` column). `Id.list_subjects(env, approve)` resolves the HITL **approver set** for a protected
deploy. `Id.mint_run_token(...)` (contract 4.7) mints the per-job attenuated token (life == job life;
**callable mid-workflow on resume**, S-11). `Id.delegation(agent, trigger_actor)` composes the effective
policy when an agent triggers/approves CI work.

### 5.2 CI's ReBAC namespace fragment (contract 4.9 — CI declares this)

CI contributes its namespace fragment (relations + permissions as union/intersect/exclude/TTU-rewrite);
Id owns the engine and the object-id minting. The fragment (illustrative):

```text
namespace ci_project:
  relation owner:    user | team
  relation member:   user | team
  relation viewer:   user | team | (parent repo viewer via TTU)         // a repo viewer can view its CI
  permission view_runs   = viewer | member | owner
  permission run_pipeline = member | owner                              // manual trigger
  permission edit_pipeline = owner | (member & has_role:maintainer)

namespace ci_environment:
  relation project:  ci_project
  relation approver: user | team                                        // protected-env approval (list_subjects target)
  relation deployer: user | team
  permission deploy   = deployer | (project->member & !protected)
  permission approve  = approver                                        // the HITL approver set
  permission rollback = deployer | approver

namespace ci_secret:
  relation project:  ci_project
  relation reader:   (project->member & scope_grant)                    // gated; untrusted_fork resolves to NONE
  permission read    = reader & !is_untrusted_fork                      // ABAC edge: fork tier never reads

namespace ci_run:
  relation project:  ci_project
  permission view    = project->view_runs
  permission cancel  = project->member | project->owner
  permission retry   = project->member | project->owner
```

The `watcher` relation (for Notif read-fanout, README §5) is declared per watchable type
(`ci_run`, `ci_environment`): a run's watchers = its trigger-actor + project members who opted in.

## 6. `PersonalDataHolder` — CI is a GDPR-spicy holder (contract 10.1)

CI leaks PII **incidentally** (not just in obvious fields), so it is a careful holder. The harness
auto-registers every CI store (contract 1.4). CI implements `locate / export / rectify / restrict /
erase` over **run-state, logs, artifacts, caches, deployments**:

- **Direct identity** (commit/PR author, "triggered by", "approved by") — stored as **pseudonym
  references, never copied PII**; erasure flows through Id's `resolve_pseudonym`/`erase` (contract 4.8) +
  CI tombstoning the identity field. Run *structure* survives for audit (delete the identity, not the fact).
- **Logs (worst offender)** — emails, usernames, IPs, tokens, fixtures. **Erasure = crypto-shred** (the
  per-tenant-DEK envelope-encryption on `log_segment`; `erase(subject)` destroys the key, rendering the
  immutable append-only ciphertext — incl. backups — unrecoverable without rewriting; NIST SP 800-88r1;
  Boneh & Lipton 1996). Per-subject free-text shred is the named GD-6 floor (05 §HP-7).
- **Artifacts/caches** — may embed personal data (seeded DBs, screenshots) — same per-tenant-DEK
  crypto-shred + short default TTL (shrinks the erasure burden; Art. 5).
- **The `restrict` flag** — a restricted subject's CI data is **not indexed / agent-used / analytics-fed /
  notification-fanned** (the README §5 restriction obligation). `restrict` flips a per-subject flag checked
  at every index/agent/notif seam.
- **`export`** — returns the subject's CI footprint (their triggered runs, approvals) as references +
  decrypted-while-key-lives log excerpts, per-viewer-safe.

`ci.*.erased` tombstone events degrade every unfurl/embed of an affected run to a tombstone (the designed
*erased* state in every CI view, design §5.10). This is the erasure-reaches-every-holder drill (07 D-3).

## 7. The remaining "every subsystem must implement" contracts

### 7.1 `ArtifactRef` + the `#sub` scheme (contracts 5.1 / 5.7)

CI mints `myelin://<tenant>/ci/<type>/<id>[#sub]` using the canonical `ci` token. Types: `run`,
`deployment`, `pipeline`, `runner`, `artifact`. The **stable `#sub` scheme** (stable across edits/retries
so embeds never dangle — CI's obligation):

| ArtifactRef | Meaning |
|---|---|
| `myelin://<t>/ci/run/<run_id>` | a run (the single-run view) |
| `myelin://<t>/ci/run/<run_id>#job-<job_id>` | a job within the run |
| `myelin://<t>/ci/run/<run_id>#step-<step_id>` | a step (the live-log view, that step expanded) |
| `myelin://<t>/ci/run/<run_id>#step-<step_id>#L42-L88` | a log line-range (**jump-to-failure**, the assembled-context path) |
| `myelin://<t>/ci/deployment/<dep_id>` | a deployment |
| `myelin://<t>/ci/pipeline/<pipeline_id>` | a pipeline definition |

`#job-`/`#step-`/`#L…` ids are **opaque and stable across retries** (the `log_anchor.step_id` is assigned
deterministically from the snapshot, not from runtime order), so a runbook/chat embed of `#step-3` never
dangles. These are the same anchors the `project` projection returns as `sub_anchor`.

### 7.2 `project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}` (contract 5.6)

The **only** way Refs/Search/Notif read about a CI artifact (no cross-DB). Per-viewer,
pre-permission-checked:

```text
fn project(ref, viewer) -> Projection | Tombstone:
  if Id.check(viewer, view, ref.acl_object()) is Deny: return Tombstone     // never leak
  match ref.type:
    run        => { title: "Run #<n> · <pipeline>", state: passed|failed|running|queued|...,
                    icon: status_glyph, render_hint: { dag_summary, failed_step?, duration },
                    sub_anchor: ref.sub }                                    // #step-3 → that step
    deployment => { title: "Deploy <env> · <version>", state: deploying|deployed|awaiting_approval|...,
                    render_hint: { env, risk, rollback_available } }
    pipeline   => { title: "<name>", state: valid|invalid, render_hint: { last_run } }
```

This backs every cross-subsystem surface: the chat run unfurl, the PR context pane, the knowledge embed,
the inbox humanisation, the search result snippet.

### 7.3 `replay(scope, since)` — reindex-from-source (contract 2.6)

CI implements `replay(scope, since)` emitting `ci.run.snapshot` / `ci.deployment.snapshot` /
`ci.pipeline.snapshot` through the **outbox → the live consumer path** (the only recovery path for derived
stores). It is **sub-artifact-granular** (a snapshot can scope to one run, one deployment, or a project) so
Search/Refs/OLAP rebuild without reading CI's DB. This is also the post-restore re-erasure path
(reindex-from-source replays the *current* state, which is already crypto-shredded for erased subjects).

### 7.4 `IndexSpec` — `declare_indexable` (contract 6.3)

CI declares how its artifacts project to a Search index doc (Search indexes implicitly off the bus,
always conjoining `list_objects` before scoring — the `search-requires-acl-filter` lint):

```text
declare_indexable(IndexSpec {
  subsystem: "ci", type: "run",
  acl_object_type: "ci_run",                                  // Search pre-filters via list_objects(viewer, read, ci_run)
  ft_fields:    [pipeline_name, branch, trigger_kind, failed_test_name, log_excerpt_of_failure],
  struct_fields:[state, trust_tier, env, actor_pseudonym, created_at, repo_ref],
  projection:   project,                                       // reuses §7.2
  semantic:     [failure_summary],                             // "find the run where test X first failed" (RAG/dedup)
})
// restriction flag honoured: a restricted subject's runs are excluded from the index (§6).
```

### 7.5 The `ToolDef` registrations (contract 8.1) → see 04 §3

CI registers its agent-facing actions into the one permissioned `ToolSurface` (`register_tool`), each with
`required_caps`, `effect_kind`, `side_effecting`, `requires_approval`, `exposed_over_mcp`. The full set
(re-run, cancel, view-log, plan, deploy, approve-deploy, rollback, secret-write) and the
`requires_approval` defaults are in 04 §3. **`ToolHands::exec` is CI's runner** (the `kind=agent` job) — the
deepest unification (05 §HP-5).

### 7.6 Reserve/settle (contract 11.7) → see 02 §6

CI passes every run through the universal gate as the workflow's bookends; no second metering path. The
meter is resource-seconds (02 §8). One `cost_event` per metered unit; wholesale ≠ markup.
