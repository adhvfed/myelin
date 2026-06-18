# Subsystem Deep-Dive: Continuous Integration / CD

> Phase: `01-research`. This is **research, not architecture**. It maps the territory
> the later architecture phases (`02`, `03`, `04/continuous-integration`) will build on.
> It does not commit to final designs. It is explicit about uncertainty and assumptions.
> Canonical brief: `../../VISION.md`. Sibling deep-dives (git hosting, issue tracker,
> knowledge, chat) and the shared-systems research are cross-referenced where relevant.

---

## 1. Purpose & role in Myelin

CI/CD is the subsystem that **executes work in response to events** — most commonly
repository events (push, PR/MR opened, tag created) but, because Myelin is one platform
with a shared event bus, also from *any* platform event (an issue transitioning to "Ready
to deploy", a knowledge-base "runbook" page being published, a chat slash-command, a
scheduled cron, an agent-fabric trigger). It is the subsystem where **untrusted, customer-
supplied code runs**, which makes it simultaneously the most operationally heavy and the
most security-sensitive subsystem in the platform.

Its role in the Myelin thesis:

- **The execution arm of the event fabric.** Git hosting, issue tracker, knowledge, and
  chat mostly *store and present* artifacts; CI *acts* on them. It is the canonical
  example of "an event causes work to happen," so it is the proving ground for the
  agent-native event model (a CI pipeline and an agent trigger are nearly the same shape:
  an event arrives → a sandboxed unit of work runs → results + events come back).
- **Where "agent-native" gets real.** A CI job and a mock agent both consume an event,
  do isolated work, and emit a result. The same runner/sandbox substrate that runs `cargo
  test` can run an agent that proposes a fix. Designing the executor abstraction so a
  "step" can be a shell command *or* a strategy-pattern agent invocation is a first-class
  goal. (Decision deferred to architecture; flagged here as a shaping constraint.)
- **EU-sovereign build infrastructure.** A major reason EU customers want this: their
  build/test/deploy compute — which sees all their source code, secrets, and artifacts —
  must run on EU-controlled infrastructure, not US hyperscaler CI SaaS (GitHub Actions
  hosted runners, CircleCI, etc.). CI is arguably the strongest data-residency argument
  in the whole platform because of what build compute touches.

**Scope boundary (assumption):** "CD" here means the *pipeline mechanics* of deployment
(running deploy steps, environment gating, approvals, promotion, rollback orchestration).
It does **not** mean Myelin ships a hosting/PaaS runtime for customer apps in Phase 1.
Deploy steps call out to customer-owned or third-party targets. Whether Myelin later
offers an EU-sovereign deploy target is an open product question (§11). I am flagging this
because "CI/CD" is ambiguous and the brief does not resolve it.

---

## 2. Core domain concepts & data model considerations

### 2.1 Conceptual model (vocabulary)

Terminology is deliberately neutral here; naming is an architecture/UX decision. The model
is informed by GitHub Actions, GitLab CI, Buildkite, Tekton, Drone, Concourse, Argo
Workflows, and Dagger.

- **Pipeline definition** — the config-as-code declaration (file in the repo, or possibly
  a platform-managed definition). Defines triggers, stages/jobs, steps, matrix, env,
  caching, secrets references, concurrency rules, artifacts.
- **Workflow / Pipeline** — a named, triggerable graph of work attached to a project/repo.
  A repo may have many.
- **Trigger** — the binding between an event (or schedule/manual/api) and a workflow,
  including filters (branch globs, path filters, event type, label conditions).
- **Run (a.k.a. pipeline run / build)** — one execution instance of a workflow caused by
  one triggering event. The top-level unit users watch. Has a status, a cause, a commit/
  ref context, a correlation id back to the triggering event.
- **Stage** — optional ordered grouping of jobs (e.g. build → test → deploy) with
  gating between stages.
- **Job** — a unit scheduled onto a single runner/executor, in one isolation boundary.
  Jobs within a run can be parallel (subject to `needs`/dependency edges → a DAG) .
- **Step / Task** — an ordered command (or action/plugin/agent invocation) inside a job,
  sharing the job's workspace and filesystem.
- **Matrix** — a parameterized fan-out of a job over a set of variable combinations
  (os × version × feature-flag), producing N sibling jobs.
- **Runner / Executor / Agent(overloaded term — avoid "agent" here to not collide with
  the agent fabric; prefer "runner")** — the process/host that claims and executes a job
  inside an isolation boundary.
- **Workspace** — the per-job filesystem (checked-out source + working files), ephemeral.
- **Artifact** — a file/blob output of a job, retained and addressable (build outputs,
  test reports, coverage, binaries, SBOMs). Distinct from cache.
- **Cache** — a *reconstructible*, keyed blob used to speed up future runs (dependency
  caches, compiler caches). Loss is a perf hit, not a correctness hit. Must be treated
  as untrusted/poisoning-prone (§7).
- **Secret** — a named, access-controlled sensitive value injected into a job at runtime,
  scoped (org / project / environment), never persisted into logs/artifacts/caches.
- **Environment / Deployment target** — a named target (staging, prod) with protection
  rules (required approvals, allowed branches, wait timers) and its own scoped secrets.
- **Log stream** — the live + archived, line-addressable output of a step/job.

### 2.2 Data model considerations

These are *considerations*, not a schema. The schema is later-phase work.

- **Multi-tenancy is in the primary key.** Per the world-scale + multi-tenant mandate,
  every entity is tenant/org-scoped from the outset (org_id is not an afterthought column;
  it is part of partitioning/sharding strategy). CI generates the *highest write volume*
  of any subsystem (log lines, step events, job state transitions), so its storage model
  drives shared-storage requirements more than any other subsystem.
- **Run state is a state machine, and it must be durable + crash-recoverable.** A run/job
  has a lifecycle (queued → assigned → running → succeeded/failed/cancelled/timed-out/
  skipped, plus a "neutral"/manual-gate-waiting state). Because runners can die mid-job,
  the source of truth for state cannot live only on the runner; it lives in a durable
  store with leases/heartbeats so a dead runner's job can be detected and re-queued or
  failed. This is essentially a distributed job scheduler — the hardest part of the data
  model (§5).
- **Hot vs. cold separation.** Run *metadata/state* (small, transactional, frequently
  queried) vs. *logs* (huge, append-mostly, time-series-ish, range-read) vs. *artifacts/
  caches* (large blobs) have radically different storage profiles. Almost certainly three
  different storage tiers (transactional DB / log store-object store / object store). This
  is a strong dependency on shared **storage** + **search**.
- **Definition snapshotting / pinning.** A run must record *exactly which definition* it
  executed (the config file content at the triggering commit, plus referenced reusable
  workflows/actions and their resolved versions). Otherwise reproducing/auditing a run is
  impossible. Implication: store a content-addressed snapshot of the effective resolved
  pipeline per run.
- **Idempotency & correlation.** Each run carries the triggering event id; re-delivery of
  the same event must not double-trigger (exactly-once *effect* even over at-least-once
  delivery). The dedupe key (event id + workflow + ref) needs to be a first-class field.
- **Append-only audit overlay.** Who triggered, who approved a deployment, who cancelled,
  who changed a protected secret/environment — corporate/GDPR auditability (cross-cuts
  with issue-tracker audit needs). Audit events are immutable and likely live in the
  shared audit log, not just CI's own tables.
- **Retention policy as a field, not a cron hack.** Logs, artifacts, and caches each need
  TTL/retention policy attached at the org/project level, both for cost and for GDPR
  storage-limitation (§9).
- **Reference-graph participation.** A run links to: a commit/PR (git hosting), possibly
  an issue ("deployed by run #X closes ISSUE-123"), a chat message, a knowledge runbook.
  These edges go in the shared cross-artifact reference graph, not as ad-hoc CI columns.

---

## 3. Pipeline definition (config-as-code)

The single most consequential design axis. Findings:

- **Config-as-code in the repo is table stakes.** A versioned file (e.g. `.myelin/ci.yml`
  or a directory of them) that travels with the code, is reviewed in PRs, and is pinned
  per-commit. This is the universal expectation (Actions, GitLab, Drone, Buildkite-as-
  code, Tekton-via-git). Anything platform-only (define pipeline in a UI DB) is at best a
  secondary mode.
- **Format spectrum — a real, unresolved tension:**
  - *Pure declarative YAML* (Actions/GitLab): approachable, diffable, AI/agent-friendly to
    generate, but suffers "YAML programming" pain (anchors, templating, expression mini-
    languages, copy-paste matrices).
  - *Programmable / typed config* (Dagger, Earthly, Pulumi-style, CDK-for-pipelines,
    Buildkite dynamic pipelines): real language, real abstraction, testable; but higher
    barrier, supply-chain surface (you run arbitrary code just to *compute* the pipeline),
    and harder for non-engineers/PMs to read.
  - *Hybrid* (declarative core + an expression language + optional dynamic generation
    step): the pragmatic middle that most mature systems converge on.
  - **Uncertainty/opinion:** Given "top-tier UX" + "agent-native" + "serves PMs too in
    the issue tracker (CI status surfaces to them)", a *declarative, strongly-schema'd,
    JSON-schema-validated* format with an *escape hatch to dynamic generation* is the
    likely sweet spot — but this is an architecture decision, explicitly deferred. The
    research point is: **agent-generatability and human-diffability should be first-class
    selection criteria**, which biases away from pure-programmatic.
- **Reusable components.** Reusable/composable units (Actions' "actions"/"reusable
  workflows", GitLab `include`/components, Tekton tasks, Drone plugins) are essential for
  not-copy-pasting and for an ecosystem. This immediately raises **supply-chain trust**
  (pin by digest, not floating tag) and **a registry** question (does Myelin host a
  component registry? Probably yes, EU-sovereign — but that's an open product call, §11).
- **Validation & "shift left."** Schema validation, lint, and a *local dry-run / plan*
  (show the resolved DAG, matrix expansion, which secrets are referenced) before running
  are strong UX wins and reduce wasted runner spend. Worth treating as core, not nice-to-
  have, because runner compute is the cost center.
- **Determinism of resolution.** The act of turning a definition + event context into a
  concrete run (matrix expansion, conditional evaluation, secret/cache key resolution)
  must be deterministic and snapshotted (§2.2). Non-determinism here is an auditing and
  reproducibility nightmare.

---

## 4. Triggering from platform events

This is where Myelin diverges from a stock CI tool and leans into the brief.

- **Trigger sources to support (research-level enumeration):**
  - *Git events:* push, branch create/delete, tag, PR/MR opened/updated/merged/closed,
    review submitted, comment command (`/ci retry`).
  - *Schedule (cron):* with the well-known timezone, drift, and "don't run if repo idle"
    concerns; backed by the shared scheduling capability (deferred tools hint a `Cron*`
    capability exists at the platform level — CI should consume it, not reinvent cron).
  - *Manual / API / CLI:* human-dispatched runs, with input parameters.
  - *Cross-subsystem platform events:* issue transitions (issue tracker), knowledge page
    publish, chat slash-command, **agent-fabric triggers** (an agent decides to run a
    pipeline). These are the differentiator. A trigger is fundamentally "subscribe to an
    event-bus topic with a filter."
  - *CI-on-CI / chained:* a run completing triggers a downstream run (deployment after
    build, or fan-out to dependent repos). Must guard against trigger loops/storms.
- **Trigger = subscription + filter + mapping.** The model is: event topic + predicate
  (branch glob, path filter, label, author, event payload conditions) + a mapping from
  event payload → run inputs/context. This should be *uniform* across all event sources,
  not special-cased per source — that uniformity is the agent-native payoff.
- **Filtering must happen cheaply and close to the bus.** At world scale, every push to
  every repo is an event; you cannot wake a full pipeline planner for each. Need a cheap
  matching layer (path/branch filters evaluated before any heavy planning). Strong
  dependency on the event bus's filtering/subscription semantics.
- **Loop & storm protection.** A run that pushes a commit (e.g. auto-format bot) that
  triggers a run that pushes a commit… Need loop detection, `[skip ci]`-style markers,
  concurrency/dedup, and per-org rate limiting. This is a known footgun; flag early.
- **Exactly-once *effect*.** The bus is at-least-once (assumption — confirm with event-bus
  research); CI must dedupe on event id so a redelivery doesn't double-run.
- **Manual gates & approvals as event waits.** A deployment waiting on human approval is a
  run parked on an external event (approval event). This unifies nicely with the event
  model and with chat (approve from a chat message) and issues (approval as a work item).

---

## 5. Hardest technical problems for WORLD-SCALE

Ordered roughly by difficulty/risk.

1. **Secure execution of untrusted code (the #1 problem).** CI runs arbitrary customer
   code with network and filesystem access. At multi-tenant world scale this is a
   hostile-multi-tenant compute platform. Threats: container escape, kernel exploits,
   cross-tenant data theft, crypto-mining/abuse, SSRF into the platform's own control
   plane and cloud metadata endpoints, secret exfiltration, cache/artifact poisoning,
   resource exhaustion (fork bombs, disk fill), and **the special case of CI from forks/
   untrusted contributors** where the code author is not even a tenant member. Detail in
   §7. This problem alone justifies a dedicated security architecture track.
2. **The distributed job scheduler.** Matching millions of queued jobs to a heterogeneous,
   elastic runner fleet, with: fairness across tenants (no tenant starves others),
   priority, concurrency limits (per-org, per-pipeline, per-environment, `concurrency
   groups`), affinity (this job needs GPU / ARM / large), leases + heartbeats + dead-
   runner reaping, and exactly-once assignment. This is a known-hard distributed-systems
   problem (Borg/Kubernetes/Nomad/Buildkite-agent territory). Backpressure when the queue
   exceeds fleet capacity must be graceful (queue, not collapse).
3. **Runner fleet elasticity & cost.** Compute is the dominant cost. Need scale-to-zero
   for idle orgs, fast cold-start (pre-warmed pools vs. cost), bin-packing, autoscaling on
   queue depth, and spot/preemptible handling — all on **EU-controlled** infrastructure,
   which constrains the menu of providers and may mean bare-metal/Hetzner/OVH/Scaleway/
   sovereign-cloud rather than the usual hyperscaler autoscaling primitives. The EU-
   sovereignty constraint makes this materially harder than US-CI-SaaS faces.
4. **Log ingestion at scale.** Logs are the firehose: high-cardinality, high-volume,
   append-mostly, must support live tail (low latency, fan-out to many viewers) *and*
   durable archive *and* range reads *and* search *and* secret-redaction-in-flight *and*
   GDPR erasure. Live-tail (websocket/SSE fan-out) and cheap cold archive (object store)
   are different systems bridged by one API. This is a top-3 storage challenge for the
   whole platform.
5. **Cache & artifact storage at scale.** Content-addressed, deduplicated, tenant-isolated
   blob storage with locality (cache near the runner region), TTL/GC, and poisoning
   resistance. Cross-region cache placement vs. data-residency rules interact (a cache
   blob may contain source/secrets-adjacent data → residency applies).
6. **Multi-region / data-residency-aware execution.** A run for an EU-resident tenant must
   execute on EU runners, with logs/artifacts/caches stored in-region. This pins the
   scheduler, storage, and the event flow to a residency model. Cross-region only for
   tenants who opt in. This is a constraint US incumbents largely ignore and is a Myelin
   selling point — and a source of real complexity (no global runner pool; partitioned
   pools per residency zone).
7. **Reproducibility & determinism.** Snapshotting definitions, pinning action/component
   versions by digest, and ideally hermetic/cache-keyed builds so a run can be explained
   and re-run. Hard in the presence of network access and floating dependencies.
8. **Hot-path performance & queue latency.** "Time to first log line" and "time from push
   to job start" are the UX metrics users feel. At scale these fight against the cold-
   start and isolation costs (stronger isolation = slower start). Genuine tension.
9. **Noisy-neighbor & abuse at the platform edge.** Free-tier / trial orgs are a magnet
   for crypto-miners and abuse. Need abuse detection, quotas, and economic controls baked
   into the scheduler, not bolted on.

---

## 6. Runners / executors, isolation, caching, artifacts, secrets, concurrency

### 6.1 Isolation models (research survey, with trade-offs)

No single model wins; expect a **strategy/pluggable executor abstraction** (which also
aligns with the brief's strategy-pattern mandate and the agent-execution unification).

| Model | Isolation strength | Start latency | Density | Notes |
|---|---|---|---|---|
| Bare shell on shared host | none | instant | high | unacceptable for multi-tenant untrusted; only for trusted self-hosted |
| OS containers (namespaces/cgroups, runc) | weak-ish (shared kernel) | fast | high | the GitLab/Drone default; kernel is the trust boundary — risky for untrusted |
| Hardened containers (gVisor, Kata partial, seccomp+userns+no-new-privs) | medium | fast-ish | high | gVisor intercepts syscalls (perf cost); good default candidate |
| MicroVMs (Firecracker, Cloud Hypervisor) | strong (hardware virt) | ~100ms–sec | medium | Fly/Modal/CodeBuild-style; strong tenant boundary, the likely default for *untrusted* |
| Full VMs | strongest | slow (sec+) | low | needed for non-Linux (macOS/Windows) and nested-virt/Docker-in-CI |
| Remote/self-hosted runners (customer infra) | customer's problem | n/a | n/a | essential for enterprises: run on *their* network/secrets; major EU-sovereign appeal |

**Research conclusions:**
- For *untrusted/world-scale* execution, **microVM-class isolation (Firecracker/Cloud
  Hypervisor) is the conservative default**; shared-kernel containers alone are likely
  insufficient as the *only* tenant boundary. (Final call: architecture phase, after a
  security threat model.)
- **Self-hosted runners are non-negotiable for the EU-enterprise audience** (run builds
  inside their VPC, touch their internal artifacts/secrets, never leave their network).
  This adds a whole trust/registration/attestation surface (a self-hosted runner is a
  semi-trusted node calling into the control plane).
- **Docker-in-CI / nested virtualization** is a perennial real-world need (building
  container images, integration tests with service containers). It forces either
  privileged-ish modes (bad) or rootless/daemonless builders (Buildah/Kaniko/BuildKit
  rootless) or nested virt. Plan for it as a first-class, sandboxed capability.
- **Non-Linux targets** (macOS for Apple builds — license-constrained to Apple hardware;
  Windows) are an expensive but real demand. Likely out of earliest scope; flag.

### 6.2 Caching

- Two-tier: dependency/build caches (keyed, reconstructible) vs. compiler caches (sccache/
  ccache style). Key derivation (lockfile hash + os + toolchain) is the subtle part.
- **Locality matters more than at small scale:** cache must be near the runner (region/AZ)
  or the download dominates the savings.
- **Security:** caches restored into a job are *inputs* → a poisoned cache compromises
  builds. Cache writes from untrusted/fork runs must not poison the trusted cache (scope
  caches by trust level / branch; PR-from-fork runs get read-restricted or isolated
  caches). This is a known real exploit class.

### 6.3 Artifacts

- Retained job outputs with explicit retention. Addressable, downloadable, linkable into
  the reference graph (an artifact referenced from a chat message or release).
- Provenance/SBOM/attestation (SLSA-style signed provenance) is increasingly expected,
  especially for an EU-sovereign supply-chain-security pitch — worth flagging as a
  differentiator opportunity.
- Large-artifact handling (multi-GB) and dedup overlap with the cache/blob store.

### 6.4 Secrets

- **Never in the repo, never in logs, never in caches/artifacts.** Stored in (or
  brokered by) the shared secret-management capability; injected at runtime; scoped (org/
  project/environment); maskable so they're redacted from log streams (best-effort —
  masking is not a security boundary, defense in depth only).
- **Untrusted/fork runs must not get secrets** by default — the canonical CI vulnerability
  is "PR from fork exfiltrates prod secrets." Secret availability must be gated on trust
  (trusted branch vs. fork), and protected environments require explicit grants/approval.
- **OIDC / short-lived credentials over long-lived secrets.** Modern best practice: CI
  mints short-lived, audience-scoped tokens (OIDC federation) to talk to cloud/registries
  instead of storing static cloud keys. Strong fit for EU-sovereign + least-privilege;
  flag as a target capability.
- **Secret rotation, audit of access, and "which run read which secret"** are corporate/
  GDPR-audit requirements. Cross-cuts with the shared identity/audit systems.

### 6.5 Concurrency & scaling

- Concurrency controls at multiple scopes: per-org plan limits, per-pipeline concurrency
  groups (cancel-in-progress for superseded commits), per-environment serialization
  (only one prod deploy at a time), and global fairness.
- Autoscaling on queue depth; fair-share scheduling across tenants; priority lanes
  (interactive PR checks vs. nightly batch). See §5.2.

### 6.6 Logs / observability

- Live tail + durable archive + structured step boundaries + per-step timing + search +
  secret redaction (§6.4) + GDPR erasure (§9). Logs should be **structured around the
  step/job graph** (collapsible per step, jump-to-failure) — a top-tier-UX requirement,
  not just a text blob.
- The CI subsystem must itself be observable (metrics/traces on scheduler latency, queue
  depth, runner utilization, failure rates) — its *own* health, distinct from customer
  build logs. Depends on shared observability.

---

## 7. Security of untrusted code execution (dedicated section)

This is large enough to warrant its own architecture track later; here is the territory.

- **Trust tiers.** At minimum: (a) trusted runs (push to a branch by a tenant member with
  write access), (b) untrusted runs (PR from a fork / external contributor), (c)
  self-hosted (customer-trusted node). Capabilities (secrets, cache write, protected
  envs, network egress) gate on tier. The fork/untrusted case is where most real CVEs in
  competitor systems originate (`pull_request_target` footguns, secret exposure to forks).
- **The isolation boundary is the kernel — minimize trust in it.** Hence microVM lean
  (§6.1). Defense in depth: seccomp, no-new-privs, user namespaces, dropped capabilities,
  read-only rootfs where possible, ephemeral one-job-per-sandbox (never reuse a sandbox
  across tenants/jobs).
- **Network egress control.** Untrusted jobs should default to restricted egress: block
  the cloud metadata endpoint (169.254.169.254 — SSRF→credential-theft), block the
  platform's own internal control plane and other tenants, optionally allowlist registries
  /package mirrors. Egress policy is per-trust-tier and per-org-configurable.
- **Resource limits & abuse.** CPU/mem/disk/pids/time quotas; runaway/fork-bomb
  containment; crypto-mining detection; per-org spend caps. Free-tier abuse is a *certain*
  problem, not a hypothetical.
- **Supply-chain integrity.** Pin reusable components/actions by content digest; verify
  signatures; SBOM + signed provenance (SLSA) for produced artifacts; protect the
  component registry. The EU-sovereign pitch is strongest if Myelin is *better* than
  incumbents on supply-chain security.
- **Secret hygiene** (covered §6.4): no secrets to untrusted tiers, masking, OIDC short-
  lived creds, access audit.
- **Cache/artifact poisoning** (covered §6.2): scope writes by trust tier.
- **Control-plane hardening.** The runner→control-plane API is a privileged boundary; a
  compromised runner must not be able to read other tenants' jobs/secrets. Runner identity
  /attestation, least-privilege job tokens (scoped to the one job/repo), short TTLs.
- **The agent-execution overlap.** Because mock (and later real) agents run in the same
  sandbox substrate, the same untrusted-code threat model applies to agent execution —
  an agent that runs tool calls is, security-wise, untrusted code. Unifying the sandbox is
  efficient *and* means the threat model is shared. Flag as a deliberate design link.

---

## 8. Events: what CI must EMIT and CONSUME

This is the agent-fabric / event-bus contract. Names are illustrative (final taxonomy is
shared-systems work); the *shape* is the research output. Every event is tenant-scoped,
carries a correlation/causation id, an actor (human/agent/system), and a timestamp.

### 8.1 Events CI **consumes** (triggers + coordination)

- `git.push`, `git.branch.created/deleted`, `git.tag.created`
- `git.pull_request.opened/updated/synchronized/merged/closed`, `git.review.submitted`,
  `git.comment.command` (e.g. `/ci retry`)
- `schedule.fired` (from the shared cron/scheduling capability)
- `issue.transitioned` (e.g. → "Deploy approved"), `issue.labeled`
- `knowledge.page.published` (e.g. a runbook triggering a job) — niche but on-thesis
- `chat.command.invoked` (slash-command in chat: run/cancel/retry, approve a deploy)
- `agent.action.requested` — agent fabric asks CI to run a pipeline/job
- `ci.run.completed` (self/chained — downstream pipelines, with loop protection)
- `deployment.approval.granted/denied` (human or agent approval events)
- `identity.permission.changed` (may need to revalidate in-flight authz)
- `secret.rotated` / `secret.revoked` (invalidate cached creds; affect in-flight jobs)

### 8.2 Events CI **emits** (so the rest of the platform + agents react)

- Lifecycle: `ci.run.created`, `ci.run.started`, `ci.run.succeeded/failed/cancelled/
  timed_out`, and per-job/per-step equivalents (`ci.job.*`, `ci.step.*`).
- Status for surfacing: `ci.status.updated` (the "commit status / check" that git hosting
  shows on a PR; the gate that blocks merge). This is a critical cross-subsystem contract
  with git hosting.
- Logs/observability: `ci.log.appended` (likely a high-volume stream, possibly a separate
  channel/transport, not the main bus — flag as a scaling concern), `ci.artifact.
  published`, `ci.test_result.reported`, `ci.coverage.reported`.
- Deployment: `ci.deployment.started/succeeded/failed/rolled_back`, `ci.deployment.
  approval_required` (parks the run; chat/issue can surface and resolve it).
- Agent-relevant signals: `ci.run.failed` with structured failure (which step, which test,
  log excerpt) is the prime hook for a "fix-it" agent. Emitting *structured*, agent-
  consumable failure data (not just a log blob) is a deliberate agent-native design goal.
- Resource/abuse: `ci.quota.exceeded`, `ci.run.queued_too_long` (for notifications/SLOs).

### 8.3 Event-flow concerns (research-level)

- **Volume asymmetry:** lifecycle events are modest; `ci.log.appended` and per-step events
  are a firehose. They probably should *not* all traverse the general event bus the same
  way. Likely: control events on the bus; log streams on a dedicated high-throughput path
  with the bus carrying only "logs available / updated" pointers. **Open question for the
  event-bus architecture.**
- **Ordering & causation:** consumers (and agents) need causal ordering within a run
  (step 2 finished after step 1). Per-run ordering guarantee required; global ordering not.
- **Exactly-once effect** on consume (§4); **at-least-once with idempotent consumers** is
  the realistic contract — must be stated explicitly with the event-bus team.
- **Backpressure:** if a downstream (agent, notifier) is slow, CI emission must not stall
  builds. Decouple emission from execution.

---

## 9. GDPR / erasure considerations specific to CI

CI is GDPR-spicy because **personal data leaks into build artifacts incidentally**, not
just into obvious fields.

- **Where personal data hides in CI:**
  - Commit author/committer identity, PR author, "triggered by", "approved by" — *direct*
    personal data on every run.
  - **Logs** — the worst offender: build logs routinely print emails, usernames, IPs,
    tokens, and sometimes *test fixtures containing real personal data*. Logs are append-
    mostly and huge, making targeted erasure genuinely hard.
  - **Artifacts** — may embed personal data (a built app bundling a seeded DB, screenshots,
    test outputs).
  - **Caches** — may contain the same, transiently.
  - Self-hosted runner metadata, IP addresses, agent hostnames.
- **Right to erasure (Art. 17) is hard against append-only logs and content-addressed
  blobs.** Strategies to research:
  - *Crypto-shredding:* encrypt logs/artifacts per-subject or per-tenant with keys that
    can be destroyed → effective erasure without rewriting immutable stores. Strong
    candidate for log/artifact erasure at scale.
  - *Tombstoning + redaction* of identity fields in run metadata (replace author with a
    tombstone) while preserving aggregate/audit integrity.
  - *Pseudonymization* of actor references (store actor as an id resolved via the identity
    subsystem, so erasing the identity propagates) rather than denormalized PII copies in
    CI tables. **Strong design steer:** CI should reference identities, not copy PII.
  - *Retention/TTL by default:* short default log/artifact retention is both cost and
    storage-limitation (Art. 5) compliance; erasure burden shrinks if data expires.
- **Data residency (already in §5.6):** EU tenants' run data (logs/artifacts/caches/state)
  stays in-region. This is a *construction* constraint on the scheduler+storage, exactly
  per the brief's "by construction."
- **Lawful basis / purpose limitation:** build data is processed under contract/legitimate
  interest for the service; must not be repurposed (e.g. silently training models on
  customer build logs) without basis — relevant once *real* agents arrive. Flag now.
- **Audit vs. erasure tension:** corporate audit wants immutable "who deployed prod"
  forever; GDPR wants erasure. Resolution likely: audit retains pseudonymized actor +
  legal-basis-justified retention; PII resolves through identity and is erasable there.
  This tension is shared with the issue tracker; coordinate.
- **Right of access / portability (Art. 15/20):** "export all my runs/logs" must be
  feasible — argues for exportable, structured run records, not opaque blobs.
- **Processor obligations:** customers are controllers, Myelin is processor for their build
  data; DPA, sub-processor transparency, breach notification timelines all apply. Mostly a
  shared-platform concern but CI is a high-risk processing activity within it.

---

## 10. Dependencies

### 10.1 On shared systems

- **Identity & access:** authn/authz for who can trigger/cancel/approve/edit pipelines and
  secrets; *job tokens* (short-lived, scoped per-run identities); actor references for
  GDPR pseudonymization; permission-change events. Hard dependency.
- **Event bus:** the entire trigger + emit model (§8). Need its delivery guarantees,
  filtering semantics, ordering, and a story for the log firehose. Hardest shared
  dependency to pin down.
- **Agent fabric:** agents as trigger sources, agents as consumers of `ci.run.failed`,
  and *agents running inside the CI sandbox substrate* (strategy-pattern executor). The
  mock-agent-via-strategy-pattern mandate lands directly on the executor abstraction.
- **Storage:** three tiers — transactional (run state), log store (firehose, tail+archive),
  blob/object store (artifacts/caches, content-addressed, dedup, residency-aware). CI is
  the heaviest storage consumer; it will *drive* storage requirements.
- **Search:** searching logs, runs, artifacts; "find the run where test X first failed."
- **Notifications:** run failure/success, deploy approvals, quota alerts → email/chat/
  push. Uses the shared notification system, not a bespoke one.
- **Cross-artifact reference graph:** run ↔ commit/PR ↔ issue ↔ chat ↔ knowledge edges.
- **Scheduling/cron capability:** for scheduled triggers (don't reinvent cron).
- **Secret management:** storage/brokering of secrets, OIDC token minting, rotation, audit.
- **Observability:** CI's own metrics/traces/SLOs (distinct from customer build logs).
- **Audit log:** immutable record of triggers/approvals/secret-access/config changes.
- **Billing/quota/metering:** CI compute is the metered cost center; needs usage metering
  hooks (minutes/credits, per runner class), quota enforcement, abuse caps.

### 10.2 On / from other subsystems

- **Git hosting (tightest coupling):** source checkout, the commit-status/checks contract
  that gates merges, PR-trigger semantics, fork/trust-tier signals, branch protection
  integration ("require CI green to merge"). CI ↔ git hosting is the most load-bearing
  cross-subsystem relationship. Coordinate interfaces closely.
- **Issue tracker:** CI status on linked issues; deploy gates as work items; "run closed
  issue"; PM-visible build/deploy health (the brief's "serves PMs" angle — surface CI in
  PM-friendly form, e.g. release readiness, not raw logs).
- **Chat:** run notifications, slash-commands to control runs, approve deploys from chat,
  reference a run in a message; humans + agents in the same channel reacting to CI events.
- **Knowledge platform:** runbooks/CI docs; possibly a knowledge page as a trigger or as
  rendered run reports; linking a postmortem doc to a failed deploy.

---

## 11. Open questions & explicit uncertainties

Flagged honestly per the brief. These shape later phases.

1. **CD scope (biggest ambiguity).** Does Myelin provide an EU-sovereign *deploy target /
   PaaS* (so "CD" means real hosting), or only deploy *mechanics* that call out to
   customer/third-party targets? I assumed the latter for Phase 1 (§1). **Needs a product
   decision** — it dramatically changes scope.
2. **Config format.** Pure declarative YAML vs. programmable (Dagger-style) vs. hybrid.
   I lean hybrid/declarative-with-escape-hatch on agent-generatability + PM-readability
   grounds, but it's an architecture decision. Unresolved.
3. **Default isolation model.** MicroVM (Firecracker) vs. hardened containers (gVisor) as
   the *default* untrusted boundary. Leaning microVM for safety; needs a threat model +
   perf/cost study. The start-latency vs. isolation tension is unresolved.
4. **Runner ownership model & EU infra.** Which EU-sovereign infra underpins the hosted
   fleet (Hetzner/OVH/Scaleway/Exoscale/sovereign clouds/bare metal)? How are macOS/Windows
   targets handled (if at all)? Self-hosted runner trust/attestation model? Uncertain.
5. **Component/action registry.** Does Myelin host an EU-sovereign reusable-component
   registry, and what is its supply-chain trust model? Likely yes; unscoped.
6. **Log transport at scale.** Do logs traverse the general event bus or a dedicated
   high-throughput path? (I suspect dedicated.) This is a shared-event-bus architecture
   question CI must influence. Open.
7. **Event-bus guarantees.** Delivery semantics (at-least-once assumed), ordering (per-run
   assumed), dedupe support — must be confirmed with the event-bus research, not assumed.
8. **GDPR erasure mechanism for logs/artifacts.** Crypto-shredding vs. TTL-only vs.
   redaction. I lean crypto-shredding + short default retention + pseudonymized actors,
   but it needs validation against legal + storage-cost constraints.
9. **Multi-region/residency execution model.** Strictly partitioned per-residency runner
   pools assumed; cross-region opt-in. The exact residency boundaries (country vs. EU vs.
   per-tenant) are a platform-wide policy question.
10. **Metering/billing unit.** Build-minutes vs. credits vs. resource-seconds per runner
    class — affects scheduler and quota design. Deferred to billing/shared work.
11. **Agent-executor unification depth.** How far to merge the CI sandbox substrate and
    the agent execution substrate. Promising but a real architecture decision with
    security implications (§7). Flagged, not decided.
12. **Local/`act`-style local execution.** Whether to offer running pipelines locally
    (developer-laptop) for fast iteration. Strong UX win, real maintenance/fidelity cost.
    Uncertain whether in early scope.

---

### Assumptions made (summary)

- "CD" = deployment *mechanics*, not a hosted runtime, in Phase 1 (§1).
- Event bus is at-least-once with per-run ordering achievable; idempotent consumers (§8).
- Untrusted-multi-tenant execution is in scope from day 1 (world-scale ⇒ public/multi-
  tenant ⇒ untrusted), so security is foundational, not deferred.
- Self-hosted runners are required for the EU-enterprise audience.
- CI references shared identities rather than copying PII (GDPR steer).

### Deferred to later phases

- Concrete schema/storage engines; the actual scheduler design; the config-language
  specification; the security architecture/threat model; the executor strategy interface;
  UX visual design; the event taxonomy's final names; billing/metering specifics. All are
  `02`/`03`/`04`-phase work and are intentionally *not* decided here.

---

## Appendix A: Key UX / views required (research-level enumeration)

Not designs — the *set of views* the architecture/UX phases must cover.

- **Run list / dashboard:** per-repo and cross-repo, filterable (branch, status, actor,
  trigger), with live status. The "is main green?" view.
- **Single-run view:** the DAG/stage visualization, per-job/per-step status + timing,
  failure jump-to, retry/cancel controls, the triggering event/cause, the pinned
  definition snapshot.
- **Live log view:** streaming tail, collapsible per step, search-in-log, secret-masked,
  deep-linkable to a line, downloadable.
- **Matrix view:** the fan-out grid, partial-failure highlighting.
- **Pipeline/definition editor + validator:** edit config-as-code with schema validation,
  lint, and a dry-run/plan ("here's the DAG this would produce").
- **Environments & deployments view:** what's deployed where, history, approvals queue,
  rollback. PM/manager-friendly "release readiness" framing (serves the issue-tracker's
  PM audience).
- **Secrets & variables management:** scoped (org/project/env), audit of access, rotation.
- **Runner fleet / self-hosted runner management:** registration, health, capacity,
  job assignment visibility.
- **Caches & artifacts browser:** with retention/size, download, GC controls.
- **Usage/quota/billing view:** minutes/credits consumed, by repo/runner class.
- **Agent-surfaced views:** failures formatted for agent/human triage; an agent's proposed
  fix attached to a failed run (forward-looking, mock-agent era).

## Appendix B: CLI commands expected (research-level enumeration)

Shapes, not final spec. A first-class CLI is expected (the agent-friendly, scriptable
surface). Plausibly namespaced under the unified Myelin CLI.

- `myelin ci run [--workflow W] [--ref R] [--input k=v]` — manually dispatch a run.
- `myelin ci list [--repo] [--status] [--branch]` — list runs.
- `myelin ci watch <run-id>` / `myelin ci logs <run-id> [--job J] [--follow]` — tail logs.
- `myelin ci retry <run-id> [--failed-only]` / `myelin ci cancel <run-id>`.
- `myelin ci validate [file]` / `myelin ci plan` — schema-validate + show resolved DAG/
  matrix locally before pushing (shift-left).
- `myelin ci secret set/list/rm --scope org|project|env` (set never echoes value).
- `myelin ci env list` / `myelin ci deploy approve <id>` / `myelin ci deploy rollback`.
- `myelin ci runner register/list/status` — self-hosted runner management.
- `myelin ci artifact list/download/rm` / `myelin ci cache list/purge`.
- `myelin ci trigger create --on <event> --filter ... --workflow ...` — manage triggers
  (the cross-subsystem-event subscription model).
- (Possibly) `myelin ci local <workflow>` — run a pipeline locally (§11 open question).
- Machine-readable output (`--json`) on everything for agent/automation consumption.
