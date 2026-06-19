# CI/CD — 03 Events, Contracts & Glue

> Phase 5-B — CI detailed architecture (rewritten against the reconciled layer). The **complete `ci.*` event
> taxonomy** CI owns (under the Bus §6 grammar, incl. the **frozen `ci.check.updated` + `ci.result`** tokens),
> the events CI **consumes**, and **how CI implements every glue contract against the FROZEN shapes**:
> `ArtifactRef` + the unified `#sub` scheme, `project(ref, viewer)`, `replay(scope, since)`, the envelope via
> the OUTBOX, Identity `check`/`list_objects` (the `SetExpr` push-down) + the ReBAC fragment,
> `PersonalDataHolder` (erasure by reference to the one posture), `IndexSpec`, the `ToolDef` registrations,
> reserve/settle, and the **`CheckStatus` seam** (contract 5.9).

---

## 1. The `ci.*` event taxonomy (CI OWNS this complete list)

Grammar (Bus §6 / contract 2.9): `<subsystem>.<artifact_type>.<event_name>` — **singular, past-tense,
dotted**. Subsystem token = `ci` (canonical, Bus §6.2). Every event is the canonical `EventEnvelope`
(contract 2.1), emitted **only via the transactional outbox** (`OutboxTx::emit(draft, cause)`, contract 2.2)
in the same tx as the state change; causality (`correlation_id`/`causation_id`/`depth`) is derived
correct-by-construction. The two **frozen new tokens** (`ci.check.updated`, `ci.result`) are registered under
contract 2.9.

### 1.1 The Git↔CI check tokens (the X-1 seam — FROZEN)

| Event / signal | Aggregate (ordering key) | Notes |
|---|---|---|
| **`ci.check.updated`** | `ci/run/<run_id>`, subject `repo#commit-<oid>/check-<context>` | **The frozen X-1 event.** Carries the `CheckStatus` struct (small, PII-free): `{repo, commit_oid, context, state, run, run_attempt, trust_tier, details_ref:#step-<n>, summary:(template_key,args), started_at, completed_at?, cost_settled}`. Per-context; **last-writer-wins by `run_attempt`** on Git's side. References-not-payloads (never log bytes). |
| **`ci.result`** *(signal, not a bus event)* | the merge-queue workflow run | **The frozen X-1 rollup signal.** `signal(merge_queue_run, "ci.result", {commit_oid, overall: success\|failure, contexts:[CheckContext], idem_token})`, idempotent on `idem_token`. Wakes Git's merge-queue durable workflow (contract 9.4). **Distinct from per-context `ci.check.updated`**: the events drive the PR checks UI; the one `ci.result` signal drives the merge-queue resume. |

`CheckContext = { provider: "ci"\|"external", name }` (e.g. `{ci,"build"}`, `{ci,"test/unit"}`).
`CheckState = queued \| in_progress \| success \| failure \| error \| neutral \| cancelled`.
`TrustTier = trusted \| untrusted_fork`. CI stamps `trust_tier` from run provenance (02 §1.3 / §4).

### 1.2 Lifecycle events (run / job / deployment)

| Event | Aggregate | Notes |
|---|---|---|
| `ci.run.started` | `ci/run/<run_id>` | a run began; carries trust_tier, trigger_kind, the CAS snapshot ref. |
| `ci.run.succeeded` / `ci.run.failed` / `ci.run.cancelled` / `ci.run.timed_out` | `ci/run/<run_id>` | terminal. **`ci.run.failed` carries *structured* failure** (which step, which test, log excerpt) — the agent-native triage hook (deliberate, not a blob). |
| `ci.run.reaped` | `ci/run/<run_id>` | a runner died mid-job and the run was re-queued/failed — **honest, never silent**. |
| `ci.job.started` / `ci.job.succeeded` / `ci.job.failed` / `ci.job.cancelled` | `ci/run/<run_id>` | per-job; job ordering is within the run aggregate. |
| `ci.deployment.requested` / `ci.deployment.approval_required` / `ci.deployment.approved` / `ci.deployment.rejected` | `ci/deployment/<dep_id>` | the protected-env HITL flow (`approval_required` opens the gate; `approved` is the durable signal landing, per-effect `idem_key`, OQ-F). |
| `ci.deployment.started` / `ci.deployment.succeeded` / `ci.deployment.failed` / `ci.deployment.rolled_back` | `ci/deployment/<dep_id>` | deploy lifecycle; `rolled_back` is first-class (reversibility). |

> **Rename note (Δ1, vs Phase-4).** Phase-4 used `ci.status.updated` + `ci.run.passed/failed` for the Git
> seam. Those are **superseded** by the single frozen `ci.check.updated` (carrying `CheckStatus`) + the
> `ci.result` rollup signal. The preserved design record (`../design/`) still references the old names in
> prose; the architecture (this doc) is authoritative — the code emits `ci.check.updated` / `ci.result`.

### 1.3 Pointer & resource events

| Event | Aggregate | Notes |
|---|---|---|
| **`ci.log.available`** | `ci/run/<run_id>` | **the ONLY log-related durable event** — a *pointer* ("lines N..M of `run/job/step` ready at `<ArtifactRef>`"). Logs themselves ride the **firehose** (`ci.log.appended` frames + the resume-cursor protocol, contract 3.5, NOT the durable bus). Coalesced, never per-line (02 §7.1). |
| `ci.artifact.published` | `ci/run/<run_id>` | a retained artifact (binary/SBOM/report/SCIP-LSIF) is available at an `ArtifactRef`; carries SLSA provenance ref. Git/Search consume (find-usages, the SCIP/LSIF follow-on, contract 6.5). |
| `ci.cost.metered` | `ci/run/<run_id>` | one metered unit (resource-seconds); wholesale + markup separate. Commercial/OLAP consume for usage rollups. |

### 1.4 Fleet / config / supply-chain events

| Event | Aggregate | Notes |
|---|---|---|
| `ci.runner.registered` / `ci.runner.attested` / `ci.runner.degraded` / `ci.runner.offline` | `ci/runner/<runner_id>` | fleet health + the self-hosted attestation surface (the runner-fleet view). |
| `ci.pipeline.created` / `ci.pipeline.updated` / `ci.pipeline.validated` | `ci/pipeline/<pipeline_id>` | config-as-code lifecycle (`validated` = a `plan` succeeded). |
| `ci.supply_chain.verification_failed` | `ci/run/<run_id>` | a floating-tag / unsigned-component / failed-signature was **refused** (audit-critical; the fail-closed proof). |

### 1.5 Cross-cutting (erasure + reindex) — required of every subsystem

| Event | Aggregate | Notes |
|---|---|---|
| `ci.run.erased` / `ci.deployment.erased` / `ci.runner.erased` | the erased aggregate | the `*.erased` tombstone (Bus §6.3) — degrades unfurls to a tombstone via the OQ-D ladder, never a dangling leak (§6). |
| `ci.run.snapshot` / `ci.deployment.snapshot` / `ci.pipeline.snapshot` | the snapshotted aggregate | the `*.snapshot` reindex-from-source events for `replay` (Search/Refs/OLAP cold rebuild; §7.3). **Sub-artifact-granular** (contract 2.6). |

## 2. Events CONSUMED (idempotent on `event_id`, the `consumer_dedup` ledger)

| Event | From | Effect |
|---|---|---|
| `git.ref.updated` | Git | match `on: push` triggers → resolve + start a run. |
| `git.pull_request.synchronized` / `git.pr.opened` | Git | match `on: pull_request` triggers; trust-tier = fork → `UntrustedFork` (stamped, 02 §1.3). |
| `issue.transitioned` / `issue.issue.closed` | Issues | match `on: issue.transitioned` triggers (the deploy-gated-on-issue flow). |
| `identity.permission.granted\|revoked` / `identity.member.*` | Identity | invalidate the approver/secret-scope resolution caches (who can approve a deploy / read a secret). |
| `*.erased` (subject) | GDPR/Bus DSR fan-out | the erasure path → crypto-shred + tombstone (§6). |
| Agent `ProposedEffect`s (re-run / cancel / deploy / approve) | Agent Fabric | arrive via `EffectApi` (plan-then-apply), **never as direct writes** (§7). |
| `job.done` / `ci.result` signals | the runner / CI | **signals, not bus events** — they wake parked workflows (02 §3, OQ-F); idempotent on `idem_token`. |
| (schedule timers) | `myelin-flow` | cron-style `on: schedule` triggers fire as durable timers (contract 9.3). |

## 3. The envelope via the OUTBOX (contract 2.1 / 2.2)

CI is a state-changing emitter: **every** `ci.*` bus event is drafted into the per-service `outbox` table in
the **same transaction** as the run/job/deploy/check state change, drained by the relay (`FOR UPDATE SKIP
LOCKED`), deduped broker-side on `event_id` (ULID). There is **no `publish_now`** anywhere in CI (the
`no-raw-publish` lint). The `job.done`/`ci.result` **signals** thread the `idem_token` so an at-least-once
delivery never double-advances a workflow (`ci.deployment.succeeded` is emitted at-most-once-in-effect). The
**audit append** is itself a bus consumer of CI's events — CI does not write the audit log directly.

## 4. The Git↔CI `CheckStatus` seam (the tightest seam — contract 5.9, FROZEN)

This is the single most load-bearing cross-subsystem seam, **jointly specified + frozen** in X-1. CI is the
**producer**; Git is the **gate**. The shape:

- **CI emits** `ci.check.updated` per `(commit_oid, context)` carrying the frozen `CheckStatus`:
  `queued → in_progress → success|failure|error|neutral|cancelled`, each with the monotonic `run_attempt` and
  the stamped `trust_tier`; `details_ref = #step-<n>` (jump-to-failure); `summary` is a `(template_key, args)`
  `HumanisedRef` (NOTIF-1), never a raw string; `cost_settled` flips true only when the reserve settles.
- **Git owns** (CI does not): the `check_status` **projection table** keyed `(commit_oid, context)` with
  **exactly one current row per key**; the **supersession rule** (incoming supersedes iff
  `run_attempt >= stored`; a lower attempt arriving late is dropped — mandatory under at-least-once); the
  **branch-protection `required`-set** policy; the **fork-endorsement** check (`approve_untrusted_ci`).
- **Fork trust gating (security-critical).** A `CheckStatus` with `trust_tier = untrusted_fork` is recorded
  faithfully but **cannot satisfy a `required` context by itself** — Git treats an `untrusted_fork` success as
  **neutral for gating** until a maintainer endorses the run or the context is re-run under `trust_tier =
  trusted`. CI **never** endorses; it only stamps the tier from provenance (the poisoned-pipeline-execution
  defence). **CI does not store `required`, does not recompute trust, does not merge.**
- **The merge queue (Git-owned durable workflow).** Per target ref, a `myelin-flow` workflow dispatches the
  required CI via `SCHEDULE_AND_RUN_JOB` (OQ-F) and `wait_for_signal("ci.result", idem_key = merge_attempt)`
  — holding **no runtime** while CI runs. CI emits the **`ci.result`** rollup once all required contexts for
  the commit reach terminal: `{commit_oid, overall, contexts, idem_token}`. On a `success` rollup Git merges
  and emits `git.pr.merged`; on `failure`/`error` it dequeues with a humanised reason.

The reciprocal **CI→Git** read is the trust-tier evaluation (CI calls `Id.check` to classify fork/member; §5)
and the workspace checkout (the runner uses a scoped job token over the git wire). CI does **not** read Git's
DB (the `no-cross-db` lint); it consumes events + calls `project`. This is the D-8 drill (07).

## 5. Identity: `check` / `list_objects` (the `SetExpr` push-down) + the ReBAC fragment

### 5.1 Where CI calls Identity

`Id.check(subject, permission, object, zookie?, caveat?)` runs at **every** CI write/read entrypoint
(fail-closed on uncertainty): start/cancel/retry a run, view a run/log/artifact, approve a deploy, read/write
a secret, register a runner, edit a pipeline. **`Id.list_objects(viewer, read, ci_run)` is the leak-free
pre-filter for every CI list/search** — and now uses the **frozen `SetExpr` push-down** (contract 4.3 /
OQ-E): Identity returns `Filter { set_expr, zookie }` for the large run space, and CI's query compiler
**lowers `set_expr` into a SQL predicate over its own `run_id` column** — concretely a JOIN against the
per-tenant authz reverse index:

```sql
-- The frozen OQ-E lowering for the CI run list (via_column = ci_run.run_id):
SELECT r.* FROM ci_run r
JOIN authz_visible av
  ON av.object_id = r.run_id            -- ColRef{ table:"ci_run", column:"run_id" }
 AND av.subject  = $viewer
 AND av.relation = 'view'
WHERE r.tenant = $tenant AND r.region = $region
  AND <the saved-view QueryAst filter, conjoined>
-- one query, NO N+1 per-row check, NO post-filter (the search-requires-acl-filter lint).
```

`Id.list_subjects(env, approve)` resolves the HITL **approver set** for a protected deploy (contract 4.4).
`Id.mint_run_token(...)` (contract 4.7) mints the per-job attenuated token (life == job life; **callable
mid-workflow on resume**, S-11; self-hosted runner token scoped to one tenant's `SelfHosted` jobs).
`Id.delegation(agent, trigger_actor)` composes the effective policy when an agent triggers/approves CI work.
**Field/transition ABAC** (e.g. "may this principal read this secret's value") is a `CaveatContext` check at
`check`-time (contract 4.2), **off** the hot `list_objects` path.

### 5.2 CI's ReBAC namespace fragment (contract 4.9 — CI declares this, FROZEN)

CI contributes its namespace fragment (relations + permissions as union/intersect/exclude/TTU-rewrite); Id
owns the engine and the object-id minting. The frozen fragment is `ci_project / ci_environment / ci_secret /
ci_run` + the **`read & !is_untrusted_fork`** ABAC edge:

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
  permission read    = reader & !is_untrusted_fork                      // the FROZEN ABAC edge: fork tier never reads

namespace ci_run:
  relation project:  ci_project
  relation watcher:  user | team                                        // Notif read-fanout (contract 4.9)
  permission view    = project->view_runs
  permission cancel  = project->member | project->owner
  permission retry   = project->member | project->owner
```

The `watcher` relation (Notif read-fanout) is declared per watchable type (`ci_run`, `ci_environment`): a
run's watchers = its trigger-actor + project members who opted in; served by the same authz reverse index as
OQ-E (contract 4.4).

## 6. `PersonalDataHolder` — CI is a GDPR-spicy holder (contract 10.1) + the one erasure posture

CI leaks PII **incidentally** (not just in obvious fields), so it is a careful holder. The harness
auto-registers every CI store (contract 1.4). CI implements `locate / export / rectify / restrict / erase`
over **run-state, logs, artifacts, caches, deployments**:

- **Direct identity** (commit/PR author, "triggered by", "approved by") — stored as **pseudonym references,
  never copied PII** (`<pseudonym>@<tenant>.noreply` grammar, contract 4.8); erasure flows through Id's
  `resolve_pseudonym`/`erase` + CI tombstoning the identity field. Run *structure* survives for audit (delete
  the identity, not the fact).
- **Logs (worst offender)** — emails, usernames, IPs, tokens, fixtures. **Erasure = crypto-shred.** Where a
  subject's inline log PII is **isolable**, the `log_segment.pii_key_ref` names a **per-subject DEK**
  (`subject:<id>`, Storage C1 / 11.4); `erase(subject)` destroys that key, rendering the immutable append-only
  ciphertext — **incl. backups** — unrecoverable (NIST SP 800-88r1; Boneh & Lipton, *A Revocable Backup
  System*, 1996). Per-tenant DEK is the fallback where the PII is not isolable.
- **Artifacts/caches** — may embed personal data (seeded DBs, screenshots) — same per-tenant-DEK (or
  per-subject where isolable) crypto-shred + short default TTL (shrinks the erasure burden; Art. 5).
- **The `restrict` flag** — a restricted subject's CI data is **not indexed / agent-used / analytics-fed /
  notification-fanned**. `restrict` flips a per-subject flag checked at every index/agent/notif seam; the
  OLAP read store honours it (no analytics for a restricted subject, contract 11.6).
- **`export`** — returns the subject's CI footprint (their triggered runs, approvals) as references +
  decrypted-while-key-lives log excerpts, per-viewer-safe.

**The residual is by reference (X-7, contract 10.9).** Third-party free-text PII — a person's name/email
typed by *someone else* into a CI log line authored under that other person's DEK — is **not** restated as a
CI-local posture. CI follows the **one platform-wide erasure posture** in
[`05 §X-7`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md): the structural
floor (per-subject DEK + pseudonym shred + `restrict` suppression) erases self-authored content reliably; the
residual third-party span is handled under the documented lawful-basis limit (best-effort `rectify`/tombstone
+ the standing `restrict` guarantee), `[OPEN — LEGAL]` pending DPO ratification. CI ships the structural floor
regardless.

`ci.*.erased` tombstone events degrade every unfurl/embed of an affected run to a tombstone via the OQ-D
4-step ladder (the designed *erased* state in every CI view, design §5.10). This is the
erasure-reaches-every-holder drill (07 D-3).

## 7. The remaining "every subsystem must implement" contracts

### 7.1 `ArtifactRef` + the unified `#sub` scheme (contracts 5.1 / 5.7, FROZEN)

CI mints `myelin://<tenant>/ci/<type>/<id>[#sub]` using the canonical `ci` token (Refs validates the grammar,
never re-authors it). Types: `run`, `deployment`, `pipeline`, `runner`, `artifact`. CI uses the **frozen
`#sub` vocabulary** (contract 5.7): the CI-owned kinds are **`step-<n>`** (a run step, jump-to-failure) and
**`check-<context>`** (a check status on a commit), plus `L<a>-L<b>` line-ranges within a step's log:

| ArtifactRef | Meaning |
|---|---|
| `myelin://<t>/ci/run/<run_id>` | a run (the single-run view) |
| `myelin://<t>/ci/run/<run_id>#step-<n>` | a step (the live-log view, that step expanded) — **resolves `CheckStatus.details_ref`** |
| `myelin://<t>/ci/run/<run_id>#step-<n>#L42-L88` | a log line-range (jump-to-failure, the assembled-context path) |
| `myelin://<t>/git/repo/<id>#commit-<oid>/check-<context>` | the **check status** subject CI stamps on `ci.check.updated` (a Git-rooted ref, X-1 / OQ-D) |
| `myelin://<t>/ci/deployment/<dep_id>` | a deployment |
| `myelin://<t>/ci/pipeline/<pipeline_id>` | a pipeline definition |

`#step-<n>` ids are **opaque and stable across retries** (`log_anchor.step_id` is assigned deterministically
from the snapshot, not runtime order), so a runbook/chat embed of `#step-3` never dangles. Refs stores the
full sub-URN **and** the `#sub`-stripped root, so a broken sub-anchor still resolves to the parent run
(OQ-D); resolution degrades through the one 4-step tombstone ladder (permission → root → sub-resolve
{live/moved/outdated/gone} → erased). These are the same anchors `project` returns as `sub_anchor`.

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

This backs every cross-subsystem surface: the chat run unfurl, the PR context pane, the knowledge embed, the
inbox humanisation, the search result snippet. The `Display` mode is the Notif humanisation projection
(contract 5.2).

### 7.3 `replay(scope, since)` — reindex-from-source (contract 2.6)

CI implements `replay(scope, since)` emitting `ci.run.snapshot` / `ci.deployment.snapshot` /
`ci.pipeline.snapshot` through the **outbox → the live consumer path** (the only recovery path for derived
stores). It is **sub-artifact-granular** (a snapshot can scope to one run, one deployment, or a project) so
Search/Refs/OLAP rebuild without reading CI's DB. This is also the post-restore re-erasure path
(reindex-from-source replays the *current* state, which is already crypto-shredded for erased subjects;
contract 10.8).

### 7.4 `IndexSpec` — `declare_indexable` (contract 6.3)

CI declares how its artifacts project to a Search index doc (Search indexes implicitly off the bus, always
conjoining the OQ-E `list_objects` `Filter` before scoring — the `search-requires-acl-filter` lint):

```text
declare_indexable(IndexSpec {
  subsystem: "ci", type: "run",
  acl_object_type: "ci_run",                                  // Search pre-filters via list_objects(viewer, read, ci_run) → SetExpr
  ft_fields:    [pipeline_name, branch, trigger_kind, failed_test_name, log_excerpt_of_failure],
  struct_fields:[state, trust_tier, env, actor_pseudonym, created_at, repo_ref, commit_oid],
  projection:   project,                                       // reuses §7.2
  semantic:     [failure_summary],                             // "find the run where test X first failed" (RAG/dedup)
})
// restriction flag honoured: a restricted subject's runs are excluded from the index (§6).
// Named follow-on: consume CI-produced SCIP/LSIF for "find usages" (contract 6.5).
```

### 7.5 The `ToolDef` registrations (contract 8.1) → see 04 §3

CI registers its agent-facing actions into the one permissioned `ToolSurface` (`register_tool`), each with
`required_caps`, `effect_kind`, `side_effecting`, `requires_approval` (the **frozen X-6 defaults**),
`exposed_over_mcp`. The full set and the `requires_approval` defaults are in 04 §3. **`ToolHands::exec` is
CI's runner** (the `kind=agent` job) — the deepest unification; it inherits the four uniform guarantees and
is **never** a side-effecting tool in this table (05 §HP-5; X-6).

### 7.6 Reserve/settle (contract 11.7) → see 02 §6

CI passes every run + every `SCHEDULE_AND_RUN_JOB` dispatch through the universal gate as the workflow's
bookends; **no second metering path** (X-6.1). The meter is resource-seconds (02 §8). One `cost_event` per
metered unit; wholesale ≠ markup.
