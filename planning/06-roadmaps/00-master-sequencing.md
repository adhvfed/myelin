# Phase 6 — Master Sequencing (the keystone roadmap)

> Phase: `06-roadmaps`. **The global build sequence** every per-system roadmap slots into.
> Canonical brief: [`VISION.md`](../../VISION.md) §6 (a roadmap is milestones with the work, the
> floor-then-full progression, the dependencies, and the quantified gates/drills that call a milestone done) —
> never contradicted. Binding doctrine:
> [`external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md)
> (§2 order-by-non-negotiability, §3 prove-it-or-it-isn't-real + the failure-injection harness, §5 the ratchet /
> committed gates, §1 code-wins-over-docs + name-your-floors) and
> [`external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (erasure-vs-immutability,
> CRDT-after-CAS, world-scale git, untrusted-code-execution, reindex-from-source).
> Frozen architecture (FROZEN — this roadmap sequences, it does not redesign):
> [`../05-refined-shared-systems-architecture/contract-index.md`](../05-refined-shared-systems-architecture/contract-index.md)
> (the dependency structure) + [`../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (X-1..X-7, OQ-A..OQ-L) + the 11 refined shared docs + the 5 rewritten subsystem `architecture/` folders.
> Testing strategy: [`../05-refined-shared-systems-architecture/testing-strategy/README.md`](../05-refined-shared-systems-architecture/testing-strategy/README.md)
> (the must-be-early gates §5) + [`../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
> (the 178 proof obligations). Spine: [`../02-holistic-architecture/architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
> (ADR-01..ADR-20). Date: 2026-06-19.
>
> **What this is.** The master order-of-operations for building Myelin. It states the ordering thesis, defines
> seven milestone bands (M0..M6), gives the critical path + the dependency DAG across the 11 shared systems and
> 5 subsystems, names the drill gates that bound each band, schedules every floor-then-full progression, and
> places the dogfooding milestone. Plain text identifiers throughout (no backticks-as-emphasis). Markdown only;
> no commits. This document is the spine; the per-system roadmaps (`06-roadmaps/<system>/`) refine the work
> inside each band and must not contradict the band ordering or the gate invariant here.

---

## 1. The ordering thesis — order by non-negotiability, applied to Myelin

The doctrine (EI-01 §2) is binding: **sequence the roadmap by what kills you first, not by architectural layer
or convenience.** For Myelin that resolves to a strict precedence. Each tier below must **exist and be drilled
green** before anything that sits on top of it is claimed done — the gate invariant (EI-01 §2, R-2): *no later
milestone is done over a red earlier gate.*

**Tier 0 — the unit of proof itself.** Nothing in the catalogue is drillable until the failure-injection
harness exists (R-3, testing-strategy §5.1). The 1×/10×/30× load generator with mixed principal kinds, the
scoped-reversible dependency-break primitive, and the telemetry-assertion library that reads the metrics port
(contract 1.8) are the **first thing built** — before the systems they will drill. A property is not real until
a drill forces the failure and observability watches it survive; the harness is the machine that does that.

**Tier 1 — silent data loss (RPO/RTO + cross-seam integrity).** EI-01 §2 #1: silent data loss outranks every
feature. The substrate's transactional outbox + idempotent-consumer template (so no event is ever lost or
ghosted, F5), and the backup/restore-verify gate (ADR-18: a CI job, not an aspiration — RPO ≤ 5 min, RTO ≤
1h/tenant ≤ 4h/cell, 0 loss, F3/STOR-D1/D2) must be green **before any surface writes real data on top of
them.** A beautiful feature surface over a substrate that silently corrupts is discovered the day a real tenant
loses real data.

**Tier 2 — RCE / sandbox escape.** EI-01 §2 #1 (the other half) + EI-04 §5: one escape is catastrophic, and a
property not drilled on a real kernel is a claim. The real-kernel sandbox-escape drill (AG-D4 / CI-T1, the
single hard GATE) must be **green on the production backend before any untrusted customer code runs** — any CI
step or any agent `ToolHands::exec`, one unified runner (ADR-20). It re-runs on every backend/image/kernel
change and gates everything downstream of untrusted execution.

**Tier 3 — the committed ratchet (the lints).** EI-01 §5: an uncommitted gate is no gate. The twelve
compile-time architecture lints (contract 1.6) make whole bug-classes impossible to compile — most load-bearing
of all, `tenant-predicate` (no cross-tenant leak, F2), `no-raw-publish` (no event escapes the outbox, F5),
`no-host-exec` (no sandbox bypass), `no-untagged-personal-data` (no PII escapes the data map, GDPR). Each ships
with a red-fixture + a green-fixture so the lint is proven to reject. They are the cheapest ratchet and come
early and stay green.

**Tier 4 — the dependency root: Identity, fail-static.** Identity is the dependency root of the whole platform
(every read path calls `check`/`list_objects`; ADR-03). It must exist, be correct (zero cross-tenant, zero
leak, disabled-user-denied-in-N-min), and **fail-static not fail-closed** (ADR-17, F7) so an Identity hiccup
degrades rather than cascading the platform down — before any permission-aware surface is built on it.

**Tier 5 — tenant partitioning + residency pin.** `(tenant, region)` is the first-class partition key (ADR-11,
contract 12.1); residency is region-pinning with no cross-region query path (EI-04 §1). The control-plane
routing + the `residency-pin` lint + cross-tenant bulkhead must exist before real tenant data is placed, so
"one tenant's region is immutable" is true by construction, not bolted on.

**Tier 6 — backup/restore-verification before real tenant data; the breadth; the scale; the polish.** Only
once Tiers 0–5 are green do we build the keystone shared systems (bus already in Tier 1, then refs/search/notif/
agents/workflow/GDPR), then the producer subsystems, then the consumer subsystems, then world-scale hardening.

**Why this exact order (the compounding-payoff test, EI-01 closing).** When the substrate and contracts are
right, each new surface is *smaller* than the last because it is a projection of capabilities that already
exist. The inverse signal — features getting *harder* to add — means the substrate is wrong; stop and repair
the foundation rather than building more feature surface. The band ordering below is this thesis made concrete.

---

## 2. The milestone bands (M0..M6)

Seven bands. Each band states its **work**, its **entry dependency** (what must be green to start it), and its
**exit gate** (the named drills that must be green to call it done and unblock the next band). Bands are mostly
sequential by the gate invariant; within a band, the per-system roadmaps parallelise the work. The band
boundaries are *gates*, not calendar dates — a band is done when its drills emit green artifacts (the catalogue
definition of PROVEN, testing-strategy §4), never when the code merely "looks done."

This banding is **my sequencing call**, justified against the frozen architecture: it follows the dependency DAG
(§3), front-loads the four order-by-non-negotiability keystones (testing-strategy §5), and respects that the bus
+ outbox + restore-verify are a Tier-1 substrate concern that cannot wait for a "foundations" band — they ship
*inside* M0/M1 because every later write depends on them.

### M0 — Substrate, harness, and the committed gates (the floor under everything)

**Thesis:** build the machine that proves things, the lints that forbid whole bug-classes, and the service shell
every system boots from. Nothing here is a feature; everything here is a precondition for honestly claiming any
feature later.

**Work:**
- **The Cargo workspace + glue-crate skeleton** (ADR-01): `myelin-events`, `myelin-identity`, `myelin-refs`,
  `myelin-agent`, `myelin-gdpr`, `myelin-content`, `myelin-query`, `myelin-tenancy` as compile-time contract
  carriers. A change to a glue contract breaks every consumer's build *now*, never silently in prod (ADR-01).
- **The substrate bootstrap harness** (contract 1.1 `serve(AppSpec)`): boot → migrate → outbox relay →
  consumers → three ports (public/internal/metrics-health, contract 1.2) → graceful drain; liveness ≠ readiness
  (1.3); forward-only online migrations (1.5); `ResilientClient` (1.9, timeout+breaker+bulkhead+jittered-retry,
  honours Retry-After); `FailStatic<T>` bounded-staleness cache (1.10); the protected-human-lane shed order
  (1.11, ADR-16). The cross-language harness shim (1.7) is specified now (Chat may diverge, TE-21) but only its
  contract, not an implementation.
- **The transactional outbox + the idempotent-consumer template** (contracts 2.2 `OutboxTx::emit`, 2.3 `outbox`
  table with `UNIQUE(aggregate, seq)` per-aggregate ordering + `FOR UPDATE SKIP LOCKED` relay, 2.4
  `EventHandler` with `subjects()` whitelist never `*`, ack-after-enqueue, 2.5 `consumer_dedup` ledger). This is
  the Tier-1 silent-data-loss floor: the **only** sanctioned emit path, causality correct-by-construction, no
  `publish_now`. The `EventEnvelope` (2.1) is frozen here as the names/units anchor every later contract aligns
  to.
- **The failure-injection harness** (testing-strategy §3, R-3): the 1×/10×/30× load generator (mixed
  human/agent/service/CI/external-MCP principal kinds, per-surface storm profiles OQ-K), the scoped-reversible
  dependency-break injector, and the telemetry-assertion library reading the survival-signal set (contract 1.8:
  RED/USE per principal-kind, consumer-lag, outbox-depth, breaker-state, fail-static ratios, shed-counts,
  causal-depth). The every-incident-adds-a-drill loop is wired (T-3).
- **The twelve committed architecture lints** (contract 1.6, the ratchet floor): `no-cross-db`, `no-raw-publish`,
  `tenant-predicate`, `no-host-exec`, `forward-only-migration`, `no-cross-sync-cycle`, `residency-pin`,
  `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`, `no-untagged-personal-data`,
  `flow-determinism` — each with a red-fixture (proves it rejects) + a green-fixture (proves it admits), wired
  into CI loud-never-swallowed (no `|| true`).
- **The contract-coverage scanner** (testing-strategy §5): CI fails the workspace if any contract-index row
  lacks provider + consumer CDC coverage — an uncommitted contract test is no contract test.
- **The shared overlay/state primitives** (testing-strategy §5, R-3): built before any feature consumes them so
  the off-screen-picker / clipped-dialog / focus-leak bug-classes are foreclosed at the design-system layer.
- **The thresholds file** (testing-strategy §3): one versioned file holds every Q32 default-to-beat; a red gate
  becomes a "claimed, not proven" scorecard row, never edited green.

**Entry dependency:** none (this is the root).

**Exit gate (must be green to start M1):**
- **SUB-D1** (kill service between commit and publish → exactly-once-in-effect, 0 ghost, 0 lost) — CI.
- **SUB-D2** (drop broker mid-stream → 0 lost across reconnect; slow subject doesn't block others) — CI.
- **BUS-D4** (crash producer between state-commit and publish → event delivered, never without state; outbox
  emit-iff-committed) — CI.
- **All twelve lints green** with both fixtures; the contract-coverage scanner passes on the (still-small)
  contract set; the harness can inject a fault and read a telemetry assertion (a self-test drill of the harness).

### M1 — Identity + storage durability + tenancy (the dependency root + the data-loss floor)

**Thesis:** stand up the dependency root (Identity, fail-static) and prove the silent-data-loss floor
(restore-verify) and the tenant/residency partition, before any subsystem writes a row. These are Tiers 1, 4,
and 5 of the thesis.

**Work:**
- **Identity & access** (`myelin-identity`, contracts 4.1–4.11): `authenticate` (all credential kinds incl.
  machine-identity SSH/deploy-key/PAT/per-job, 4.1); `check` with `CaveatContext` (4.2); **`list_objects` with
  the `SetExpr` push-down** (4.3 — the single most load-bearing inter-system contract, the leak-free pre-filter
  via the per-tenant authz reverse index; built early because Search/Refs/every board depends on it);
  `write_tuples`/zookie (4.6, 4.10); `mint_run_token`/`revoke` (4.7); `resolve_pseudonym`/`erase` (4.8); the
  per-subsystem ReBAC namespace engine (4.9, fragments slot in per subsystem later). **Fail-static** (4.11,
  ADR-17): an Identity hiccup degrades, doesn't cascade.
- **Tenancy & control plane** (`myelin-tenancy`, contracts 12.1–12.6): `(tenant, region)` partition key (12.1);
  `discover`/`place`/`placement_of` PII-free routing (12.2, 12.3); `residency_verify` (12.4); isolation-tier
  contract (12.5); the cross-cell PII-free pointer bridge **frame** (12.6 — the contract, multi-cell is the
  named floor for M5). The control-plane schema is PII-free by the `control-plane-pii-free` lint.
- **Storage durability** (contracts 11.1–11.8): OLTP tier client + RLS + encrypted columns + the outbox lives
  here (11.1); `BlobStore` content-addressed (11.2, fs-backed floor — object-backed is M3/M5); KMS hierarchy +
  `KeyOrigin` (11.3, per-cell root → per-tenant KEK → per-subject DEK, the crypto-shred substrate);
  **backup/restore/cross-seam + restore-verify** (11.5, ADR-18 — the headline durability gate, CI-wired); the
  reserve/settle cost gate (11.7, fronts every agent + CI run).
- **GDPR/Audit spine, structural half** (`myelin-gdpr`, contracts 10.1–10.3, 10.8): `PersonalDataHolder`
  trait + harness auto-registration (10.1, 1.4); the `#[personal_data]` classify-derive + the
  `no-untagged-personal-data` lint targets (10.2); `data_map()/ropa` (10.3); the erasure ledger (10.8). The
  full DSR state machine + the `[OPEN — LEGAL]` posture (10.9) lands incrementally as holders come online; the
  structural floor (per-subject DEK crypto-shred + pseudonym-map shred) is built now (X-7).

**Entry dependency:** M0 green (the outbox, the harness, the lints — Identity emits `write_tuples` via the
outbox; every store auto-registers as a holder via the harness).

**Exit gate (must be green to start M2):**
- **ID-D3** (cross-tenant check/list/read via path spoof → 0 cross-tenant tuples) — CI.
- **ID-D2** (break Identity dependency → authenticated traffic survives on coarse cache; just-revoked still
  denied; fail-static) — CI.
- **ID-D1** (SCIM-disable → every surface denies within **N=5 min**) — SCHED.
- **CP-D2 / CP-D3** (misroute rejection 0 cross-cell read; residency-pin rejects out-of-region write) — CI.
- **STOR-D1 / STOR-D2** (restore from backups → 0 loss, OLTP↔blob↔index↔offset one consistent point; **RPO ≤
  5 min, RTO ≤ 1h/tenant ≤ 4h/cell**) — SCHED. **This is the silent-data-loss floor; M2 does not start over a
  red STOR-D1.**
- **STOR-D4 / GA-D5** (per-subject crypto-shred unrecoverable in backups; `no-untagged-personal-data` lint red
  on an untagged PII field) — SCHED/CI.

### M2 — The reactive shared layer (refs, search, notif, agents, workflow) + the safety drills

**Thesis:** build the connective tissue every subsystem projects onto — the reference graph, search, the one
inbox, the agent fabric, and the durable workflow engine — and **green the sandbox-escape gate (Tier 2)** so
untrusted code can run in later bands. This is the band where the "agent-native from the ground up" promise gets
its substrate.

**Work:**
- **Reference graph** (`myelin-refs`, contracts 5.1–5.8): `ArtifactRef` parse/format (5.1); `resolve` per-viewer
  unfurl/embed → projection|tombstone (5.2, cell-local); `backlinks/edges/traverse` leak-free depth-16 CTE (5.3);
  `refs.edge.created` consumed from producers (5.4); the TE-7 typed-edge mirror (5.5); `project(ref, viewer)`
  REQUIRED-on-every-subsystem (5.6); the **unified `#sub` grammar + the 4-step tombstone ladder** (5.7, X-4);
  reindex-from-source (5.8). The Git↔CI `CheckStatus` seam (5.9) is *declared* here as a contract but lands with
  its producers in M3/M4.
- **Search** (`myelin-search`, contracts 6.1–6.5): `query` AST→FT/structured/vector **always conjoining the
  `list_objects` Filter** before scoring (6.1, `search-requires-acl-filter` lint); `semantic` ACL-filtered k-NN
  (6.2); `declare_indexable` per-subsystem projection (6.3); `reindex(scope)` the only rebuild path (6.4,
  reindex-from-source). Search is a derived store — it never reads owner DBs, it replays through the live
  consumer (EI-04 §5, F4).
- **Notifications** (`myelin-notif`, contracts 7.1–7.8): the ONE inbox `list_inbox` ranked (7.1); read-state
  truth (7.2); **`humanise` the ONE templating surface** resolving each ref per-viewer (7.3, OQ-L);
  prefs/quiet-hours (7.4); `oncall_now`/`page` escalation workflow (7.5); the delivery adapter region-aware
  EU-preferring (7.8).
- **Durable workflow** (`myelin-flow`, contracts 9.1–9.6): `DurableExecutor{start, signal, describe, cancel}`
  with idempotent `signal` per-effect `idem_key` (9.1, OQ-F); `WfCtx` deterministic surface + the
  `flow-determinism` lint + the `SCHEDULE_AND_RUN_JOB` long-park idiom (9.2, OQ-F); the durable timer wheel
  (9.3, millions of timers as an indexed range read); the durable signal for multi-day HITL (9.4). This is the
  engine under agent runs, CI pipelines, the merge queue, SLA timers, and HITL.
- **Agent fabric** (`myelin-agent`, contracts 8.1–8.8): `ToolSurface::register_tool` with the frozen
  `requires_approval` defaults (8.1, X-6); **`EffectApi::apply` plan-then-apply** schema→capability→delegation→
  tenant→budget→HITL→apply→meter (8.2); `AgentRuntime::step` strategy seam with `--use-mock` (8.3, mock agents
  only during development per VISION §3); **`ToolHands::exec` the unified sandbox = the CI runner's `kind=agent`
  job** (8.4, ADR-20, the four uniform guarantees); `Agent::handle` bounded loop (8.5); explicit-first dispatch
  (8.6); `run --dry-run` (8.7). The agent-trace holder seam (8.8) lands with Knowledge in M3.
- **Signals / automations / triggers / firehose** (contracts 3.1–3.6): `define_signal_rule` (3.1);
  `register_automation` (3.2); `arm_trigger` with the `QueryAst` condition (3.3); the `EventMatcher` = the frozen
  `myelin-query` `QueryAst` (3.4); **the firehose transport + the resume-cursor subscription protocol** (3.5,
  OQ-J — `subscribe(stream, scope, cursor?)` + `resume(stream, scope, last_seq)` backfills the gap then live,
  `resync_required` → snapshot fallback; the durable real-time transport the CRDT later slots into, EI-04 §2);
  the reactive/dispatch tier with reserve/settle-before-run (3.6).
- **Shared content + query crates frozen** (`myelin-content` 13.1, `myelin-query` 13.3): the block taxonomy,
  the markdown-subset inline grammar with the three structured ref nodes, the WASM compile target
  (`render(parse(md)) === md`), the `QueryAst`/`ViewSpec`/field-type enum, and the `order_key`/LexoRank encoding
  — frozen byte-identical so Issues/Knowledge/Chat cannot drift (X-2, X-3).

**Entry dependency:** M1 green (every system here calls `list_objects`/`check`, emits via the outbox, registers
as a holder, and is residency-pinned).

**Exit gate (must be green to start M3 — note AG-D4 is the hard go/no-go for all untrusted code):**
- **AG-D4 / CI-T1** (real-kernel sandbox escape → **ZERO escapes**) — **GATE**. *This is the single hard
  go/no-go: until green on the production backend, no untrusted CI step and no agent compute call runs in M3+.*
- **AG-D1 / AG-D2 / AG-D3** (no write outside `EffectApi`; effect outside the ∩ denied; confined to
  `agent.policy ∩ delegation ∩ tenant.policy`) — CI.
- **AG-D5** (HITL gated tool withheld → 0 mutation pre-approval, 1 apply) + **AG-D9** (mock determinism →
  identical effect sequences; mutation score) — CI.
- **FLOW-D1 / FLOW-D2 / FLOW-D5** (worker kill mid-run → resume exactly-once 0 double-effect; divergence guard
  halts nondeterministic; journal+outbox co-commit) — CI.
- **REF-D1 / REF-D2 / REF-D8** (confidential-via-public 0 leak; cross-tenant edge 0; traversal depth-16 bound) —
  CI.
- **SRCH-D1 / SRCH-D3** (confidential never in any result incl. counts/IDF/RAG; cross-tenant 0) — CI.
- **NOTIF-D4 / NOTIF-D7** (confidential subject → humanised tombstone, title never leaks; escalation resumes
  exactly once across a kill) — CI.
- **BUS-D1 / BUS-D3 / BUS-D6** (kill consumer + sever broker → 0 lost 0 dup; replay deterministic+idempotent;
  self-trigger → depth-ceiling + tripwire + breaker) — CI.

### M3 — The producer subsystems (Git hosting + Knowledge platform)

**Thesis:** build the two subsystems that *produce* the artifacts everything else references and gates on — git
repositories/commits/PRs and knowledge docs/databases. They are producers in the reference graph and the
heaviest to scale, so they come before the consumers that gate on them. Each ships its **floor first** (see §5).

**Work — Git hosting** (subsystem `git-hosting/architecture/`):
- Repository hosting on the **local-disk floor** (EI-04 §3: authoritative bytes on a node's local disk first;
  object-backed packs are the named M5 follow-on); content-addressed objects + Merkle history + pack/delta
  (proven structures, not invented). Code browsing; pull/merge requests; code review with content-anchored line
  ranges (`#L<a>-L<b>`, BLAKE3 fingerprint + 3-way context match, the 5.7 tombstone ladder).
- The merge gate reads the Git-owned `check_status` projection table (5.9 consumer half); the merge queue is a
  durable workflow waking on the `ci.result` signal (the producer is CI in M4 — Git ships the gate + projection
  now, the seam goes live when CI lands).
- **Pseudonymous-by-default commit identities** (X-7, 4.8 — the immutable bytes never bake in erasable PII in
  the first place; this is the erasure-vs-immutability answer and **must be decided before the git data model is
  fixed**, EI-04 §1). The history-rewrite erasure path is the named, audited follow-on (10.6).
- The Git ReBAC namespace fragment (4.9: ref-glob + CODEOWNERS-as-relations + `approve_untrusted_ci`); the
  `list_objects` `SetExpr` conjoin for PR/repo lists (no N+1); `project(ref, viewer)` for unfurls; the indexable
  `git.*` projection for code search (6.5).

**Work — Knowledge platform** (subsystem `knowledge-platform/architecture/`):
- The Notion-class block editor over the frozen `myelin-content` taxonomy (13.1); in-document databases as a
  property bag per row with rollups/formulas **computed at read time, never stored** (EI-04 §2.4, field-type
  enum 13.3); inline content as the **markdown-subset string** (survives copy/paste/export/diff/
  reference-extraction).
- **Real-time collaborative editing — the CAS floor first** (EI-04 §2, KN-1): per-block optimistic
  compare-and-swap guarding each write on the block's last-modified token; on a precondition miss, reject the
  loser and return current server state — **guarantees no *silent* overwrite, does not merge.** Shipped *named
  as a floor* over the resume-cursor firehose transport (3.5, built in M2). The CRDT is the named M5 follow-on
  that slots into that same transport.
- The agent-trace holder (8.8, AG-7: Knowledge accepts a content-addressed agent-trace write, registers it as
  an erasable holder). The KN ReBAC fragment (4.9: page-tree inherit-with-overrides + row + field caveat).

**Entry dependency:** M2 green (Refs/Search/Notif/Agents/Workflow exist; the `#sub` grammar, the firehose
resume-cursor transport, the content + query crates are frozen; **AG-D4 is green** so any agent edit / CI-bound
compute can run).

**Exit gate (must be green to start M4):**
- **GIT-D9** (crash serving tier mid-push → `git.ref.updated` iff the ref move committed; quarantine objects
  discarded on abort; outbox emit-iff-committed) — CI.
- **GIT-D8 / GIT-D11** (cross-tenant repo access denied at the front door; partial-visibility PR list →
  `SetExpr` JOIN returns only visible rows, 0 leak, one query, revoke reflected) — CI/SCHED.
- **GIT-D7** (force-push/rebase a PR with open inline threads → anchors resolve LIVE/MOVED/OUTDATED/GONE; 0
  mis-anchored) — CI.
- **GIT-D2** (erase a commit author → pseudonymous-by-default residual == the one platform posture) — SCHED.
- **KN-D3** (CAS floor: two clients edit the same block → loser rejected with current state, **0 silent
  overwrites**; different blocks parallel) — CI.
- **KN-D1** (kill collab client mid-edit + sever → resume(scope=doc, last_seq) → **0 ops lost, 0 duplicate**;
  re-run across the engine_promote boundary so it stays green when the CRDT lands) — CI.
- **KN-D2** (`render(parse(md)) === md` 100% round-trip) + **KN-D7** (block commit ↔ relay-publish outbox
  emit-iff-committed) + **KN-D5 / KN-D13** (confidential page/row/field 0 leak incl. COUNT; cross-tenant 0) — CI.

### M4 — The consumer subsystems (CI + Issues + Chat)

**Thesis:** build the three subsystems that *consume* and *react to* the producers' artifacts — CI gates on git
pushes, Issues references commits/PRs/docs, Chat unfurls everything. CI lands first within the band because it
closes the `CheckStatus` seam (5.9) Git already gates on, and because its sandbox runner is the same hardened
runner the agent fabric already drilled (AG-D4 / CI-T1 is the same gate).

**Work — Continuous Integration** (subsystem `continuous-integration/architecture/`):
- Pipelines as durable workflows (`myelin-flow`, the `ci.pipeline` workflow body, `flow-determinism` lint, no
  clock/RNG/IO outside `WfCtx`); triggered by repository + platform events. The unified sandbox runner (= the
  agent `ToolHands::exec` runner, ADR-20) — **AG-D4 / CI-T1 already green from M2**, re-run on every backend/
  image/kernel change.
- The **`CheckStatus` producer half** (5.9, X-1): emit `ci.check.updated` per `(commit_oid, context)` with
  `run_attempt` monotonic supersession + `trust_tier` (trusted | untrusted_fork); emit the rollup `ci.result`
  signal the merge queue waits on; stamp `details_ref` as `#step-<n>` (jump-to-failure). The trust-tier gate:
  an `untrusted_fork` success is **neutral for gating** until endorsed (the poisoned-pipeline defense).
- Reserve/settle on every run (11.7, the same wallet as agent runs); the T3 log tier (firehose frames sealed
  into content-addressed segments + the `(job, step, byte-range)` index, per-subject-DEK CI-log segments, 11.8);
  trust-tier/branch-scoped cache namespaces (11.2, a fork write cannot reach the trusted cache); the CI ReBAC
  fragment + `read & !is_untrusted_fork` ABAC; residency-pinned runners (CI-R3).

**Work — Issue tracker** (subsystem `issue-tracker/architecture/`):
- Issues for engineers + PMs: roadmaps, sprints, hierarchies, custom fields (the `myelin-query` field-type enum
  13.3), SLAs, reporting, audit. The board/backlog scan via the **`list_objects` `SetExpr` JOIN** (no N+1, <1s
  keyboard budget at 1M+ issues); co-equal board/roadmap views over the same `ViewSpec`/table; the human-key
  `<PROJECTKEY>-<seqno>` (5.1); drag-reorder via `order_key`/LexoRank (13.3); SLA timers + triggers on the
  durable wheel (9.3); the ADF→`myelin-content` import (13.2). Issue descriptions/comments single-author CAS over
  the content subset; `render(parse(md)) === md`.
- The Issues ReBAC fragment (4.9: issue + field/transition caveats); guard transitions ("can't mark Done while
  CI red" reads `CheckStatus`; "can't close while blocked_by open").

**Work — Chat** (subsystem `chat/architecture/`):
- Conversation that **references any artifact** (commit/issue/doc/CI run) and lets humans + agents talk in the
  same channels. The gateway↔firehose resume-cursor transport (3.5, OQ-J — Chat may diverge to a non-Rust
  connection tier per the 1.7 shim, TE-21); per-conversation total order (ULID); idempotent send
  (`UNIQUE(conv, client_nonce)`); unfurls via Refs `resolve` (the 4-step tombstone ladder); the shared per-ref
  cache busting on `*.updated`; explicit-first agent dispatch (a casual `@agent` mention notifies, does not spawn
  a costed run; only an explicit action dispatches; reserve/settle gates even the explicit run, 8.6). Batch HITL
  approval cards with per-effect `idem_key` (OQ-F). The Chat ReBAC fragment (4.9: `channel.read = member +
  parent_project->read`); search-as-non-member returns 0 results.

**Entry dependency:** M3 green (Git produces the commits CI checks and Issues/Chat reference; Knowledge produces
the docs they embed; **AG-D4 green**; the `CheckStatus` consumer/projection exists in Git awaiting CI's
producer).

**Exit gate (must be green to start M5):**
- **CI-T1 / AG-D4** re-confirmed green on the production runner (the hard GATE; re-run on the CI image) — GATE.
- **GIT-D10 / CI-D8** (the X-1 check seam end-to-end: out-of-order/dup `ci.check.updated` → run_attempt
  supersession; fork self-green neutral; doubly-delivered `ci.result` → merge-queue wakes exactly once; **0
  double-merge**) — CI.
- **CI-D1 / CI-D4 / CI-D6 / CI-D7** (runner+control-plane kill → effectively-once, 0 double-deploy; floating
  tag/unsigned → fail-closed; fork→trusted-cache write 0; fork→secret read 0) — CI.
- **ISS-D1 / ISS-D2 / ISS-D3** (board↔roadmap same-row 0 drift; 50+ fields × 1M issues board query <1s no full
  scan; cross-tenant + confidential IDOR 0 leak) — CI/SCHED.
- **ISS-D5 / ISS-D6 / ISS-D12** (N humans + agent reorder 0 clobber, converges; SLA fires to-the-second across
  restart; "can't mark Done while CI red" guard blocks) — CI.
- **CHAT-D1 / CHAT-D13 / CHAT-D14** (sever gateway↔firehose → resume 0 lost/0 dup; message-persist↔event co-commit;
  idempotent send 1 message) — CI.
- **CHAT-D5 / CHAT-D11 / CHAT-D17** (confidential unfurl → tombstone 0 title leak; search-as-non-member 0
  results + lint; casual @agent → 0 auto-spawn, reserve gate) — CI.

### M5 — World-scale hardening + the floor follow-ons + the cross-subsystem E2E wedge

**Thesis:** with all five subsystems on one substrate and the deterministic correctness drills green, now prove
the system **as a whole** under world-scale load, ship the named floor follow-ons (CRDT, object-backed git
packs, multi-cell), and green the four whole-system chained-mutation E2E scenarios that prove the differentiator.

**Work — the floor follow-ons** (each was named in its band; here is its scheduled follow-on, §5):
- **The CRDT, after the CAS floor** (EI-04 §2, KN-1): an Automerge-/Yjs-class engine slotting into the M2
  resume-cursor firehose transport; the first true concurrent-edit conflict is its trigger. KN-D1 re-runs across
  the engine_promote boundary (it was written to survive the swap).
- **Object-backed git packs, after the local-disk floor** (EI-04 §3, STOR-5): authoritative bytes move from
  node-local disk to the object store (delta/pack management, sharding, replication, smart-transport, the
  within-EU CDN clone/bundle blob class, 11.2). The transition is the explicit sequenced piece of work EI-04 §3
  insisted on; early choices did not pin repositories to a single node (12.2 repo-granular relocatable
  placement).
- **Multi-cell, after single-cell** (the named floor, OQ-I, 12.6): the cross-cell PII-free pointer bridge goes
  live (ISS cross-cell portfolio rollup, KN cross-cell collab, CHAT cross-org channels); DSR fan-out iterates
  `member_cells` (10.4); the FLOOR drills GA-D8 / CP-D7 / CP-D8 are now owed and run.
- **The full DSR / erasure fan-out across all H1–H18 holders** (10.4, GA-D1): every holder now exists, so the
  fan-out is complete; the `[OPEN — LEGAL]` posture (10.9) is instantiated per subsystem by reference.
- **Event-volume column-store seam** (EI-04 §5): a seam for the highest-volume streams — added only once volume
  is *measured*, not before.

**Work — world-scale hardening (the F6 surge family + the scheduled scale drills):** the 30× surge drills across
every owner (protected human lane holds, agent lane sheds 429+Retry-After, cross-tenant impact 0); the
prod-scale benchmarks (1M+ timers, 100k-PR list, the monorepo-ceiling); online-migration-under-load; restore-
verify at cell scale; the cell bulkhead (a fatal fault in one cell unaffects others).

**Work — the whole-system E2E wedge** (testing-strategy §2, the four chained-mutation scenarios against a full
cell with mock agents):
- **E2E-1 PR context pane** (Git+CI+Issues+Knowledge+Refs+Search+Id+Notif).
- **E2E-2 CI-fail → triage agent → issue → chat → fix-PR** (the agent-native flagship).
- **E2E-3 Spec-to-ship traceability** (cold-reindex == live; audit tamper detected).
- **E2E-4 DSAR fan-out** (0 holders missed; 0 recoverable PII incl. vectors incl. backups; certificate sealed).

**Entry dependency:** M4 green (all five subsystems exist; the deterministic correctness drills are green; the
floors are in place to be promoted).

**Exit gate (must be green to declare world-scale readiness):**
- **The full F6 surge family** across all owners (SUB-D3, ID-D9, BUS-D7, REF-D10, SRCH-D6, NOTIF-D5, AG-D6,
  FLOW-D8, GIT-D6, CI-D2, CHAT-D3/D4) — SCHED: human lane within budget, agent sheds, cross-tenant impact 0.
- **GIT-D4 / GIT-D5** (monorepo ceiling documented + clone p99 held; concurrent merges + failover → linearizable
  on ref CAS, no split-brain, 0 lost merge) — SCHED.
- **KN-D1 re-green across the CRDT boundary**; **KN-D8** (all-hands doc thousands of concurrent editors → caps
  hold) — CI/SCHED.
- **GA-D1 / GA-D8 / CP-D7 / CP-D8** (DSR fan-out 0 holders missed; multi-cell erasure per-cell receipt set;
  cell→cell migration 0 loss; cross-cell ref PII-free bridge) — SCHED.
- **The four E2E scenarios green** (E2E-1..E2E-4, each emitting its named green artifact, testing-strategy §3.4).
- **STOR-D2 at cell scale** re-confirmed (RPO/RTO under world-scale load).

### M6 — Dogfooding: Myelin hosts itself

**Thesis:** the cheapest, most honest load generator is the platform's own development (testing-strategy §1).
Myelin hosts its own git repositories, runs its own CI (one CI graph, dogfooded), tracks its own issues,
documents itself in its own Knowledge platform, and the team talks in its own Chat — so the gates run on the
platform's own commits and the switch test is reached by the builders themselves driving the real UI.

**Work:**
- Migrate the Myelin monorepo onto Myelin git hosting; the build/test/lint/mutation pipeline becomes a Myelin CI
  pipeline (the lints and the mandatory-core mutation gate now run as Myelin CI jobs on every Myelin commit).
- The roadmap + gap report + scorecard live as Myelin issues + a Myelin Knowledge space; the every-incident-
  adds-a-drill loop files a Myelin issue and a reproducing drill.
- Drive the real UI of all five subsystems for the **switch test** (EI-01 §4, the frontend done-bar L5): could a
  GitHub/Jira/Linear/Notion/Slack user move to Myelin without hitting a wall the old tool didn't have? — reached
  by *driving it in a browser*, not by reading the feature list; measured contrast + latency budgets +
  `render(parse(md)) === md` + overlays against the real anchor (design-language §8b).

**Entry dependency:** M5 green (the platform is world-scale-ready and the E2E wedge is proven; you do not
dogfood real team data onto a substrate whose restore-verify and DSAR fan-out are not green — Tier 1 + Tier 6 of
the thesis: backup/restore-verification before real tenant data, and the team's data is real tenant data).

**Exit gate (the done-bar for the platform):**
- **ISS-D14 / CHAT-D19 / Git OQ-12 switch tests pass** (driven in a browser; measured contrast + latency) — SCHED.
- **The Myelin self-hosting CI graph is green** on the platform's own commits (the dogfood loop is live).
- **No later-band gate is red** (the gate invariant holds end-to-end: a truth-up pass confirms every PROVEN row
  rests on a dated green artifact, never a doc claim; code-wins-over-docs, EI-01 §1).

### M7 — Production readiness & security hardening (fill the floors; the fail-closed release gate)

**Thesis:** M0..M6 deliberately shipped several production mechanisms as **documented EI-01 §1 structural
floors** — correct in shape, honestly `Floor named:`, but not production-real — and the M6 dogfood loop is the
cheapest, most honest place to surface that the named follow-ons ("P5/P6") were never ledger prompts. M7 is the
post-dogfood band that **fills those floors with real implementations, proves each on real infrastructure with a
SEPARATE verification prompt (a mock/model/dogfood never proves a production mechanism — EI-01 §3), and gates the
platform's first production release fail-closed.** It runs AFTER M6 and BEFORE any real customer tenant data is
admitted. The full per-finding disposition is the audit
[`../07-prompts/production-readiness-audit.md`](../07-prompts/production-readiness-audit.md); the prompt bodies
are [`../07-prompts/by-system/production-readiness.md`](../07-prompts/by-system/production-readiness.md)
(P-522..P-546). The M7 vetting overlay is recorded in
[`../system-reviews/2026-06-26/00-m7-hardening-strategy.md`](../system-reviews/2026-06-26/00-m7-hardening-strategy.md),
[`../system-reviews/2026-06-26/01-m7-vetting-gate-matrix.md`](../system-reviews/2026-06-26/01-m7-vetting-gate-matrix.md),
and
[`../system-reviews/2026-06-26/02-blackbox-security-persistence-drills.md`](../system-reviews/2026-06-26/02-blackbox-security-persistence-drills.md);
those review docs do not add product scope, but they make the proof obligations concrete: production-graph
absence scanners, blackbox security/persistence drills, evidence-integrity checks, and external-review records
are part of the M7 done-bar.

**Work (the floors filled, each with its filling prompt):**
- **Durable persistence** — bind the live OLTP/cache pool under Identity's in-memory principal/tuple/revocation
  stores (P-522), verify crash/restart + multi-instance + no-in-memory-store-in-prod-graph (P-523).
- **KMS / HSM** — back the L0 cell root with a durable HSM-class adapter; root never in process; destruction
  permanent; rotation O(keys) (P-524); verify zeroization + no-resurrection-across-restore + no-key-leak (P-525).
- **Real authentication cryptography** — OIDC JWKS / SAML XML-DSig / WebAuthn / SSH (P-526); signed
  capability/machine tokens + DPoP proof + TPM attestation (P-527); verify no structural verifier in the prod
  graph + expired-grants-cannot-authorize (P-528). These REMOVE `Structural{Verifier,TokenVerifier,TokenSigner,
  AttestationVerifier}` from every production path.
- **Real backup/restore** — WAL shipping + base backups + PITR + destructive clean-target restore (P-529);
  verify MEASURED RPO ≤ 5min / RTO ≤ 1h-tenant / 4h-cell over real data at cell scale (P-530).
- **Tenant isolation** — transaction-local `SET LOCAL` RLS + reset-on-release + identifier validation + mTLS +
  region fail-fast on the live pool (P-531).
- **Secret handling** — redacted Debug + secrecy/zeroize + no-Serialize-on-bearer sweep (P-532); verify
  no-credential/key-in-any-sink (P-533).
- **Supply chain & governance** — SHA-pin actions / digest-pin images / pin toolchain (P-534); cargo-deny
  advisory+license (P-535); SBOM + signed provenance (P-536); reproducible builds (P-537); SECURITY.md +
  CODEOWNERS + vuln-response (P-538).
- **Production runtime** — real OS-signal drain + OpenTelemetry export + trace propagation (P-539).
- **Gate integrity + truth-up** — required jobs fail-not-skip + mandatory mutation + immutable attestations
  (P-540); re-run every band scorecard against the M7 production graph (P-541).
- **External human blockers (recorded, not asserted)** — independent crypto + sandbox reviews + prod-image escape
  drill (P-542); third-party penetration test + findings register (P-543).
- **Sandbox production exec path** (corrected on re-audit, F4) — neither committed backend ran `JobSpec.command`
  through its production `launch()` (Firecracker hardcoded `oneshot=true` → `init=/bin/true`; gVisor only probed
  `runsc --version`). Implement the real microVM/runsc job runner so both backends execute `spec.command`,
  enforce limits + timeout-whole-guest-kill, capture exit code + stdout/stderr, and meter only post-completion
  (P-544); verify a real command runs end-to-end on BOTH backends AND re-run the AG-D4 corpus THROUGH the
  production exec path (not the special harness) → 0 escapes on a real kernel (P-545).

**Entry dependency:** M6 green (the platform is dogfooded and the M0..M6 gate invariant holds end-to-end).

**Vetting overlay:** M7 must be executed under the review matrix in
`planning/system-reviews/2026-06-26/`: every implementation prompt has a paired verification artifact that
would fail on the old floor; every security or persistence claim has a blackbox/adversarial drill; every static
scanner has a red fixture proving it bites; every DB/KVM/runsc/KMS dependency is required, not skipped; every
scorecard row is generated/attested or treated as non-evidence. These review docs are the tactical test plan
for the M7 prompts, and P-546 reads their required scorecards as release evidence.

**Exit gate (the production-release done-bar — P-546, FAIL-CLOSED, RED by default):** the single release gate
goes green if and ONLY IF all hold, computed mechanically from dated green artifacts (never a self-claim):
**no structural/mock impl in the production dependency graph**; durable persistence + real crypto + real KMS +
tenant-isolation + secret-handling + production-runtime gates all green; **the sandbox production exec path runs
the real `spec.command` with no `oneshot`/`init=/bin/true`-only launch in the production graph, and the
production-path escape drill is green (0 escapes) on both committed backends**; **a destructive production
restore has been performed and RPO/RTO MEASURED over real data**; current dependency/advisory + license scans
pass at release time; gates mechanically enforced + the truth-up pass has 0 red rows; **independent cryptography
+ sandbox + penetration reviews completed with 0 critical/high open, OR each open item explicitly recorded as a
named external/human blocker with owner + rationale + sign-off**. Any single condition red ⇒ no production
release; a threshold is never weakened to flip it.

**The M7 gate invariant:** M7 is the only band whose exit gate is a release authorization, not a build
boundary. The two permanent gates (AG-D4/CI-T1, STOR-D1/D2) still ratchet through M7 — and P-529 re-arms
STOR-D1/D2 over the REAL restore driver, P-542 re-runs AG-D4/CI-T1 on the committed prod image, and **P-545
re-arms AG-D4/CI-T1 over the production exec path** (the corpus run through the real `launch()`, not the special
harness, now that P-544 makes the sandbox actually execute `spec.command`) — so M7 does not relax either
permanent gate; it raises the bar from "green on the floor" to "green on the production mechanism" and from
"green on a harness" to "green on the path real jobs use."

---

## 3. The critical path + the dependency DAG

### 3.1 The critical path (the longest chain of must-precede dependencies)

The single longest dependency chain — the spine that determines the minimum number of sequential gates — is:

> **harness + outbox + lints (M0)** → **Identity `list_objects`/`check` + restore-verify + tenancy (M1)** →
> **agent fabric + workflow + the firehose resume-cursor transport + AG-D4 sandbox-escape GATE (M2)** →
> **Git (pseudonymous commits, the merge gate + `check_status` projection) (M3)** → **CI (the `CheckStatus`
> producer closing the X-1 seam) (M4)** → **the X-1 check-seam end-to-end (GIT-D10/CI-D8) + the E2E-2 flagship
> (M5)** → **dogfood the self-hosting CI graph (M6)**.

Everything else hangs off branches of this spine. The two hardest single seams on the path are **AG-D4** (the
sandbox-escape GATE, which blocks all untrusted execution and therefore both CI and any agent compute) and
**X-1 / 5.9** (the Git↔CI check seam, the most load-bearing cross-subsystem contract, split producer/consumer
across M4/M3).

### 3.2 The dependency DAG (what unblocks what)

**Substrate root (M0) — depended on by everything:**
- `serve(AppSpec)` + three-surface + liveness≠readiness → every service boots from it.
- `OutboxTx::emit` + `outbox` table + `EventHandler` template + `consumer_dedup` → every state-changing handler
  and every consumer.
- The `EventEnvelope` (2.1) + the `ArtifactRef` token table → the names/units anchor every contract aligns to.
- The twelve lints + the failure-injection harness + the contract-coverage scanner → the gate under every drill.

**Identity (M1) — the dependency root of every read path:**
- `list_objects` `SetExpr` push-down (4.3) → **Search** (conjoin the Filter), **Refs** (leak-free traverse),
  **every subsystem board/list** (Git PR list, CI runs, Issues board, KN db view, Chat channel list). The single
  highest-fan-in inter-system contract.
- `check` + `CaveatContext` (4.2) → every write path, `EffectApi`, every gateway, field/transition ABAC.
- `mint_run_token` (4.7) → Agent runs, CI dispatch, workflow activities.
- `write_tuples`/zookie (4.6/4.10) → every permission-aware read's consistency.
- fail-static (4.11) → Notif and every critical-dep caller degrade rather than cascade.

**Storage + Tenancy (M1) — the durability + partition floor:**
- restore-verify (11.5) → **gates every subsystem that writes data** (the silent-data-loss floor).
- KMS hierarchy + per-subject DEK (11.3/11.4) → crypto-shred for every erasable holder.
- `(tenant, region)` partition + `residency_verify` (12.1/12.4) → every store, every CI runner/log/cache.
- reserve/settle (11.7) → every agent run + every CI run.

**The reactive layer (M2):**
- The firehose resume-cursor transport (3.5) → **Knowledge collab** (the CRDT slots in later), **Chat**
  presence/live, **CI** log live-tail. The durable real-time transport built *first* (EI-04 §2).
- `EffectApi` plan-then-apply (8.2) + `ToolHands::exec` unified sandbox (8.4) → **every agent-authoring
  subsystem** (Issues/Knowledge/Chat tools) and **CI** (the same runner). **AG-D4 gates them all.**
- `DurableExecutor` + timer wheel + durable signal (9.x) → agent runs, CI pipelines, the merge queue, SLA
  timers, multi-day HITL, Notif escalation, KN living-doc automations.
- `project(ref, viewer)` (5.6) + the `#sub` grammar + tombstone ladder (5.7) → every unfurl/embed/backlink in
  every subsystem.
- `humanise` (7.3) → every channel renderer, every agent HITL card, every status message.
- `myelin-content` + `myelin-query` frozen (13.x) → Knowledge, Issues, Chat (no drift).

**The producers (M3) → the consumers (M4):**
- **Git** produces commits/PRs/refs → **CI** checks them, **Issues** references them, **Chat** unfurls them.
- **Knowledge** produces docs/databases → **Issues** (spec→issue lineage), **Chat** (embeds), **Search**
  (indexes), **agent-trace** holder.
- **CI** produces `CheckStatus`/`ci.result` (5.9) → **Git** merge gate consumes (the X-1 seam, producer in M4
  closing the consumer Git built in M3).
- **Issues** produces typed lifecycle edges (TE-7, 5.5) → Refs traversal, the spec-to-ship lineage.
- **Chat** consumes everything (the maximal consumer): refs unfurl, the per-ref cache, agent dispatch.

**M5 floor follow-ons depend on their floors:** CRDT ⟸ CAS floor + resume-cursor transport; object-backed packs
⟸ local-disk floor + relocatable placement; multi-cell ⟸ single-cell + the bridge frame; full DSR fan-out ⟸ all
H1–H18 holders existing.

**The acyclicity rule (EI-02 §3, `no-cross-sync-cycle` lint):** the DAG is kept acyclic by construction — Git
never synchronously calls CI to ask "is it green," it reads its own `check_status` projection fed by CI's
events. Every cross-subsystem dependency is an async event/projection, never a synchronous call cycle. The
`no-cross-sync-cycle` lint (M0) enforces this at compile time.

---

## 4. The drill gates that bound each band (the gate invariant, quantified)

The gate invariant (EI-01 §2, R-2): **no later band is done over a red earlier-band gate.** Each band's exit
gate (§2) is a set of catalogue drills that must emit a green artifact (PROVEN, not CLAIMED). Restated as the
band-boundary go/no-go, with the must-be-green-first ordering:

| Band boundary | The hard go/no-go (must be green to proceed) | Family |
|---|---|---|
| **M0 → M1** | SUB-D1, SUB-D2, BUS-D4 (outbox 0-loss/0-ghost); all 12 lints green w/ fixtures; harness self-test. | F5 + the ratchet |
| **M1 → M2** | **STOR-D1/STOR-D2** (restore-verify, RPO ≤ 5min / RTO ≤ 1h-tenant 4h-cell, 0 loss — *the silent-data-loss floor*); ID-D3 (cross-tenant 0); ID-D2 (fail-static); ID-D1 (disabled-user N=5min); CP-D2/CP-D3 (misroute 0 + residency-pin). | F3, F2, F7, F8 |
| **M2 → M3** | **AG-D4 / CI-T1** (real-kernel escape = **0** — *the single hard GATE before any untrusted code*); AG-D1/D2/D3/D5/D9; FLOW-D1/D2/D5; REF/SRCH/NOTIF/BUS leak + loss drills. | escape, F9, F5, F1 |
| **M3 → M4** | GIT-D9 (push outbox emit-iff-committed); GIT-D8/D11 (cross-tenant 0 + SetExpr leak-free); KN-D3 (**CAS 0 silent overwrites**); KN-D1 (resume 0 lost/dup); KN-D2 (md round-trip 100%). | F5, F2, F1, CAS floor |
| **M4 → M5** | GIT-D10/CI-D8 (**X-1 check seam, 0 double-merge**); CI-T1 re-green on the prod runner; CI-D4/D6/D7 (supply-chain + fork isolation); ISS-D2/D3 (board <1s + IDOR 0); CHAT-D1/D13/D14 (resume + co-commit + idempotent send). | X-1, escape, F1, F5 |
| **M5 → M6** | The full F6 surge family (human lane holds, agent sheds, cross-tenant 0); GA-D1/CP-D7/CP-D8 (DSR fan-out 0-missed + multi-cell floors); **E2E-1..E2E-4 green**; STOR-D2 at cell scale. | F6, F3, the whole-system wedge |
| **M6 done** | ISS-D14/CHAT-D19 switch tests; the self-hosting CI graph green; the truth-up pass confirms 0 red earlier gates. | the done-bar |
| **M7 done (release)** | **P-546 fail-closed release gate green:** 0 structural/mock impl in the prod graph; durable-persistence + real-crypto + real-KMS + restore + tenant-isolation + secret + runtime gates green; **sandbox production exec path runs `spec.command` (no `init=/bin/true`-only launch) + AG-D4 corpus through the prod path = 0 escapes on both backends**; destructive restore performed + RPO/RTO MEASURED over real data; advisory+license scans pass at release; truth-up 0 red; independent crypto/sandbox/pentest reviews 0 critical/high open or recorded as named human blockers. | the production-release done-bar |

**The two permanent gates** (re-run forever, never "done"): **AG-D4 / CI-T1** (re-run on every backend/image/
kernel change — one escape is catastrophic) and **STOR-D1/STOR-D2** (the restore-verify CI job runs on every
change touching a store — silent data loss outranks every feature). These are not band-local; they ratchet
across the whole build.

---

## 5. Where floors ship and when their follow-on is scheduled (name-your-floors)

The discipline (VISION §3, EI-04 §4): **name the floor and name the follow-on.** A floor masquerading as done is
the failure; a named floor with a scheduled follow-on is correct. Every floor in the build, with its ship-band
and its follow-on-band:

| Floor (shipped) | Band | The full answer (follow-on) | Band | The trigger |
|---|---|---|---|---|
| **Per-block CAS** (no silent overwrite, no merge) | M3 (KN) | **CRDT** (Automerge-/Yjs-class, slots into the resume-cursor transport) | M5 | the first true concurrent-edit conflict (EI-04 §2) |
| **Local-disk git storage** (authoritative bytes node-local) | M3 (Git) | **Object-backed packs** (delta/pack/sharding/replication/smart-transport + CDN clone class) | M5 | the single-node ceiling measured (GIT-D4); never pin repos to one node (EI-04 §3) |
| **Single-cell** (one home cell per tenant) | M1/M4 | **Multi-cell** (the cross-cell PII-free bridge live; DSR iterates member_cells) | M5 | cross-cell rollup/collab/cross-org demand (OQ-I); FLOOR drills GA-D8/CP-D7/CP-D8 owed |
| **fs-backed `BlobStore`** | M1 | **Object-store `BlobStore`** (one-line swap, 11.2) | M5 | with object-backed packs |
| **Mock agent runtime** (`--use-mock`, scripted-deterministic) | M2 | **`LlmAgentRuntime`** (the real adapter, region-aware EU-hostable sub-processor) | post-M5 / execution | after the safety drills (AG-D4/D2/D3/D5) are green; a config/impl swap, not a rewrite (VISION §3) |
| **Pseudonymous-by-default commits** (immutable bytes never bake erasable PII) | M3 (Git) | **Audited history-rewrite erasure path** (10.6, with the changed-hash consequence) | M5 / on-demand | a body must be expunged (EI-04 §1); decided *before* the git data model froze |
| **GIN-indexed JSONB facet scan** (Issues/KN custom fields) | M3/M4 | **Generated projection-feeder index** (promoted per facet) | M5 | a facet in > 5% of view executions, *measured* (OQ-C) |
| **Single-region event log** (general-purpose DB) | M0 | **Column-store/time-series seam** for highest-volume streams | post-M5 | event volume *measured* to outgrow the DB (EI-04 §5); not before |
| **The `[OPEN — LEGAL]` erasure residual posture** (structural floor built, residual flagged) | M1→M5 | **Counsel/DPO ratification** of the one residual lawful-basis statement (10.9, X-7) | parallel (legal) | the structural floor ships regardless; the residual is one ratified statement, not five |
| **In-memory principal/tuple/revocation stores** (model the SQL S1/S3/S7 tables; the executed-code floor, audit F6) | M1 (Identity) | **Live OLTP/cache pool binding** (real Postgres + Valkey behind the unchanged store surface) | **M7 (P-522, verify P-523)** | first production deployment; the floor note ("until the driver lands P-S15") was never a real follow-on prompt |
| **`StructuralVerifier`/`StructuralTokenVerifier`/`StructuralTokenSigner`/`StructuralAttestationVerifier`** (parse envelopes, no crypto; audit F2) | M1 (Identity) | **Real credential + token cryptography** (OIDC JWKS / SAML XML-DSig / WebAuthn / SSH; signed PASETO/biscuit tokens + DPoP proof + TPM attestation) | **M7 (P-526/P-527, verify P-528)** | first production deployment; the "P5/P6 follow-on" in the code/roadmaps was a planning-phase label, not a ledger prompt |
| **Software-floor KMS root** (`CellRoot` process-held; audit F7) | M1 (Storage) | **Durable HSM-class KMS adapter** (root never in process, Shamir-split recovery, destruction permanent) + zeroization | **M7 (P-524, verify P-525)** | first production deployment; real-HSM physical keying ceremony is a named external/human blocker on P-544 |
| **Modeled-WAL backup/restore** (abstract `WalOffset`; modeled clean target; audit F8) | M1 (Storage) | **Real WAL shipping + base backups + PITR + destructive clean-target restore** (`pg_basebackup`/`pg_restore`) + MEASURED RPO/RTO | **M7 (P-529, verify P-530)** | first production deployment; re-arms STOR-D1/D2 over the real driver |
| **Unpinned supply chain + absent governance** (tag-pinned actions/images, no cargo-deny/SBOM/SECURITY.md; audit F11) | (pre-M7) | **SHA/digest pinning + cargo-deny + SBOM/provenance + reproducible builds + SECURITY.md/CODEOWNERS** | **M7 (P-534..P-538)** | first production deployment; the license allowlist + staffed security owners are named human prerequisites |
| **Sandbox boot-smoke launch, no job exec** (Firecracker `launch()` hardcodes `oneshot=true` → `init=/bin/true`, `firecracker.rs:327-328`; gVisor `spawn_real_runsc` probes `runsc --version` only, `gvisor.rs:227-237`; neither runs `JobSpec.command` in production; escape drills pass only on special harnesses; audit F4) | M2 (CI/agent, the hardening posture + drill harness) | **Real microVM/runsc job runner** (both backends execute `spec.command`, enforce limits + timeout-whole-guest-kill, capture exit code + stdout/stderr, meter only post-completion) + **production-path escape drill** (AG-D4 corpus through the real `launch()`, 0 escapes on both backends) | **M7 (P-544, verify P-545)** | first production deployment; the boot self-test keeps its `oneshot` path; re-arms AG-D4/CI-T1 over the production exec path |
| **Mock agent runtime** (re-stated: `--use-mock`) | M2 | **`LlmAgentRuntime`** (real adapter) | post-M5 / execution (P-481 names the swap; the swap itself is a config/impl change, gated by the M7 release gate's "no mock in the prod graph" condition) | after AG-D4/D2/D3/D5 green |

**The honest-floor rule binds all of these:** each floor is tracked in the gap report with its claimed/proven
status and its linked follow-on; the gap being *invisible* is the only failure (EI-04 §4). KN-D1 is deliberately
written to **re-run green across the CAS→CRDT engine_promote boundary** so the floor's promotion is itself
drilled.

---

## 6. Digest

**The milestone bands (M0..M6):**
- **M0 — Substrate + harness + gates:** the workspace + glue crates, `serve(AppSpec)`, the transactional outbox
  + idempotent-consumer template, the failure-injection harness, the twelve committed lints (red+green
  fixtures), the contract-coverage scanner, the overlay/state primitives.
- **M1 — Identity + storage durability + tenancy:** Identity (the dependency root, `list_objects` SetExpr
  push-down, fail-static), Storage (restore-verify = the silent-data-loss floor, KMS/per-subject-DEK, reserve/
  settle), Tenancy (partition key + residency-pin), the GDPR structural spine.
- **M2 — The reactive shared layer + the safety drills:** Refs, Search, Notif, Workflow, Agent fabric, the
  Signals/firehose resume-cursor transport, the content+query crates frozen — and **AG-D4, the sandbox-escape
  GATE.**
- **M3 — The producer subsystems:** Git (pseudonymous commits, local-disk floor, the merge gate + check_status
  projection) and Knowledge (the CAS collab floor, in-doc databases, agent-trace).
- **M4 — The consumer subsystems:** CI (the CheckStatus producer closing the X-1 seam, the unified runner),
  Issues (board/SLA/roadmap), Chat (unfurl-everything, explicit-first agents).
- **M5 — World-scale hardening + the floor follow-ons + the E2E wedge:** CRDT, object-backed packs, multi-cell,
  full DSR fan-out; the 30× surge family; the four whole-system chained-mutation E2E scenarios.
- **M6 — Dogfooding:** Myelin hosts itself (one self-hosting CI graph; the switch tests driven in a browser).
- **M7 — Production readiness & security hardening:** fill the executed-code floors (live OLTP/cache stores, real
  credential+token cryptography, HSM-class KMS, real WAL/PITR backup-restore, transaction-local RLS, secret
  zeroization, supply-chain pinning/cargo-deny/SBOM/SECURITY.md, OS-signal runtime, AND — corrected on re-audit —
  the **real sandbox JobSpec.command execution path** for both Firecracker + gVisor, neither of which ran
  `spec.command` in production), each impl + a separate verification prompt; the truth-up pass; the recorded
  external/human blockers (HSM ceremony, independent crypto/sandbox audits, third-party pentest); and the single
  FAIL-CLOSED production-release gate (P-546).

**The critical path:** harness+outbox+lints (M0) → Identity+restore-verify+tenancy (M1) → agent fabric+workflow+
firehose transport+**AG-D4 sandbox-escape GATE** (M2) → Git pseudonymous commits + merge gate (M3) → CI
**CheckStatus producer closing the X-1 seam** (M4) → the X-1 seam end-to-end + the E2E-2 flagship (M5) → dogfood
the self-hosting CI graph (M6). The two hardest single seams on the path: **AG-D4** (blocks all untrusted
execution) and **X-1 / contract 5.9** (the Git↔CI check seam, split producer/consumer across M4/M3).

**The must-be-green-first drills (order-by-non-negotiability, the band gates):**
1. **The failure-injection harness self-test (M0)** — the unit of proof; nothing else is drillable first.
2. **SUB-D1 / SUB-D2 / BUS-D4 + the twelve lints (M0)** — the transactional outbox 0-loss/0-ghost floor + the
   ratchet.
3. **STOR-D1 / STOR-D2 (M1)** — restore-verify, RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell, 0 loss: the
   silent-data-loss floor, before any surface writes real data (the permanent gate).
4. **ID-D3 / ID-D2 / ID-D1 + CP-D2 / CP-D3 (M1)** — cross-tenant 0, fail-static, disabled-user-denied-in-5-min,
   misroute 0 + residency-pin: the dependency root made safe.
5. **AG-D4 / CI-T1 (M2)** — real-kernel sandbox escape = **0**: the single hard go/no-go before any untrusted CI
   step or agent compute call runs (the permanent gate, re-run on every backend/image/kernel change).
6. **KN-D3 (M3)** — the CAS floor: 0 silent overwrites (the named-floor proof before the CRDT).
7. **GIT-D10 / CI-D8 (M4)** — the X-1 check seam: 0 double-merge, fork-success-neutral.
8. **E2E-1..E2E-4 (M5)** — the whole-system wedge: 0 leak, exactly-once HITL+merge, cold-reindex == live, DSAR 0
   holders missed.
