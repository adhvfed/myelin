# Phase 2b — Integration Directives (the binding hand-off)

> Phase: `02b-doctrine-integration`. These are the **binding directives** downstream phases must
> follow, derived from [`decision-record.md`](./decision-record.md) and the five
> [`analysis/`](./analysis/) docs. Each directive is **one crisp imperative + the external-insights
> citation + any adopted default**. They have the same canonical status as the doctrine itself
> (default-you-follow-unless-you-write-down-why, per
> [`external-insights/README.md`](../../external-insights/README.md)).
>
> Citation key: `EI-NN §x` = `external-insights/NN-*.md §x`. ADR-NN refers to
> [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) (ADR-16…20
> appended this phase). DL = [`design-language.md`](../02-holistic-architecture/design-language.md).
>
> **Reading rule (carried from VISION §3 / EI README):** read the relevant external-insights doc
> *before* the phase it binds to — EI-02 before shared/subsystem architecture, EI-03 before the agent
> fabric, EI-04 before committing any subsystem design, EI-01 before any build phase, EI-05 before any
> frontend design or build.

---

## Phase 3 — Shared systems

### Identity & Access
- **ID-1 — Serve a bounded-staleness cached answer on an Id-dependency hiccup (fail-static), not
  fail-closed.** Already-authenticated traffic survives on a cached "actor active / coarse grants"
  answer; the staleness window is **bounded by the deprovision/revocation SLA** and must contain the
  short-lived agent-token TTL. Fail-closed remains correct for *authz decisions*; fail-static is the
  *availability* default. *(EI-02 §10; ADR-17.) DPO ratifies the bound.*
- **ID-2 — Mint per-run agent identity at dispatch; token life == run life.** Scrub any shared platform
  token from the child environment (anti-leak); revoke on teardown idempotently **even on crash**.
  *(EI-03 §4.3; EI-02 §2.)*
- **ID-3 — Trust the token's tenant, never the URL-path tenant.** A cross-tenant access is an IDOR;
  there is no cross-tenant query path. *(EI-02 §1.)*
- **ID-4 — Plan a dedicated read replica for the authn/authz hot path first; measure before sharding.**
  *(EI-02 §8.)*

### Event Bus (`myelin-events`)
- **BUS-1 — Default the durable transport to a JetStream-class durable streaming log with durable PULL
  consumers + consumer groups.** Non-durable fire-and-forget pub/sub is wrong; PG-outbox is acceptable
  only if it provides the same durable-pull/consumer-group semantics; Kafka/Redpanda are same-class.
  Beat the default only in writing. *(EI-02 §3; default-to-beat narrowing ADR-04.)*
- **BUS-2 — The outbox is the ONLY sanctioned emit path; `myelin-events` exposes no fire-and-forget
  emit.** Relay claims with `FOR UPDATE SKIP LOCKED`, stable message id for dedup, dead-letter after
  bounded retries. *(EI-02 §4.)*
- **BUS-3 — The shared consumer template: whitelist the subjects you handle (never `*`); bind to a
  durable consumer by name and never re-declare its start policy on reconnect; acknowledge only after
  the work is enqueued; terminate non-retryable (malformed) messages immediately.** Expose consumer-lag
  / pending-count for monitoring. *(EI-03 §6.1–6.3.)*
- **BUS-4 — Refine the reactive vocabulary into four primitives — Event / Signal / Automation-rule /
  Trigger.** Consumers subscribe to **curated, deduplicated, severity-ranked Signals**, not the raw
  Event firehose (infra indexers/refs-builders excepted). The reactive/dispatch tier gets an explicit,
  separately-reviewed design. *(EI-03 §2, §6; ADR-19.)*
- **BUS-5 — Carry causality nested, not flat:** `causation_id` is the *immediate parent* (root carries,
  parent = cause, depth+1, derived from the cause); propagate in headers alongside trace context; keep a
  distinct `caused-by` human-action/session reference. *(EI-02 §6; EI-03 §6.4.)*
- **BUS-6 — Keep a seam for a column-store/time-series engine on the highest-volume streams; do not add
  it before the volume is measured.** *(EI-04 §5.2.)*

### Reference Graph (`myelin-refs`)
- **REF-1 — Backlinks stay event-sourced projections; lifecycle/semantic edges (closes/blocks/depends/
  assigns) are *also* mirrored to a typed relation table owned by the authoritative subsystem — the
  typed edge, not the URN string, is source of truth.** This resolves **TE-7**; hand the typed tables
  to Phase 4 (Issues + Knowledge). *(EI-02 §7; ADR-15 note.)*
- **REF-2 — Default to Postgres + recursive CTEs for the shallow graph; a dedicated graph DB must beat
  this default with a measured reason.** *(EI-02 §8; default-to-beat narrowing ADR-14 Refs row.)*
- **REF-3 — Persist cross-entity links as URNs, never display keys; the URN library rejects scope-less /
  ambiguous refs and never guesses scope; `#42`/`@alice`/`~general` are render-time projections.**
  *(EI-02 §7.)*
- **REF-4 — Reindex-from-source is a first-class capability:** on rebuild, Refs asks each owner to
  re-emit through the live consumer path; it never reads owner DBs. *(EI-04 §5.3.)*

### Search & Indexing
- **SEARCH-1 — Reindex-from-source is the only recovery path:** the index never reads owner databases;
  owners re-emit through the live consumer used in steady state, so recovery uses one code path and
  cannot drift. *(EI-04 §5.3.)*
- **SEARCH-2 — Treat Search and Refs as easy-to-under-budget; budget the reindex capability up front.**
  *(EI-04 §5.3.)*

### Notifications
- **NOTIF-1 — Humanise machine strings at the backend, paired with a routable `ArtifactRef`.**
  `"merge_request merged"`, raw ids, and unrendered markdown are the #1 "unfinished" tell; humanisation
  lives at the source (Notifications templating + Refs display-name resolution), not in a frontend
  string map, so every consumer **and every agent-authored message** inherits it. *(EI-05 §6.)*
- **NOTIF-2 — Every notification carries "why it fired" provenance.** *(EI-05 §4.)*
- **NOTIF-3 — Reindex/rebuild notification read-models from source via the live consumer.**
  *(EI-04 §5.3.)*

### Agent Fabric (`myelin-agent`)
- **AG-1 — The brain is a stateless one-method provider `step(conversation) -> {use_tools | submit}`;
  the platform-side agent loop owns conversation history.** This is the default-to-beat for the open
  `Agent::handle` shape (AG-3); plan-then-apply must survive. *(EI-03 §1.2.)*
- **AG-2 — The hands is one method `exec(command) -> result` with NO host-execution path that bypasses
  it.** Elevate to an architecture-test/lint obligation (sibling to ADR-01's no-cross-DB lint); a
  simulation impl emits a channel-proof marker. *(EI-03 §1.5.)*
- **AG-3 — Stand up skeleton mode first (no model, no tools, zero spend, zero effects): authenticate,
  fetch task, print summary, exit.** Roadmap order is skeleton → mock → real. *(EI-03 §1.6.)*
- **AG-4 — Ship the deterministic mock as a real `--use-mock` runtime flag on the same code path**, not
  only a test harness. *(EI-03 §1.3.)*
- **AG-5 — A denied effect (403/503) returns an ordinary `Denied` tool error to the loop; there is no
  privileged fallback path.** *(EI-03 §4.2.)*
- **AG-6 — Add the structural loop guards explicitly: self-guard (skip the agent's own output) + a
  reference gate (only a structured picker-produced reference can re-trigger, never raw typed text) +
  causal-depth ceiling + shared-causal-root-within-a-window tripwire + bounded dispatch worker pool that
  drops over-cap (never forks).** Wire to ADR-05 (only `artifact_ref` nodes emit `ref.created`).
  *(EI-03 §5.3–5.4; EI-02 §6.)*
- **AG-7 — Represent an agent's execution trace as a content-addressed, immutable Knowledge document**
  (reusing `myelin-content`), distinct from the tamper-evident audit log; keep it a `PersonalDataHolder`.
  *(EI-03 §4.4.)*
- **AG-8 — Wire approve → resume end-to-end:** a gated write tool whose name is not in "approved" is
  *withheld* (returns an error, does not mutate); approval re-runs the step; the approval card shows the
  pending action + a live cost estimate. *(EI-03 §5.1.)*

### Storage
- **STOR-1 — Provide a narrow `put/get/head/delete` content-addressed (hash-on-write) blob trait** so
  filesystem-vs-object-store is a one-line swap. *(EI-02 §8; ADR-12.8 alignment.)*
- **STOR-2 — Migrations are forward-only and online: no rollback migrations; expand→backfill→contract;
  never a blocking `ALTER` on a hot table; measure lock time against a restore first.** *(EI-02 §8.)*
- **STOR-3 — The cache/coordination store (Redis/Valkey-class) is NEVER a source of truth.**
  *(EI-02 §8.)*
- **STOR-4 — Define the cross-seam restore-consistency point (OLTP rows ↔ object/blob ↔ search index ↔
  event-log offsets restore to one mutually consistent point).** *(EI-02 §11; ADR-18.)*
- **STOR-5 — Plan the git local-disk → object-store-backed packs transition as explicit sequenced work;
  the v1 data model must keep repos relocatable, never node-pinned.** *(EI-04 §3.2.)*

### GDPR / Audit
- **GD-1 — Produce a named "Erasure vs. Immutability reconciliation" write-up** (co-owned with the Git
  P4 agent + Legal/DPO): pseudonymous-by-default commit identities as a *commit-time prerequisite*, the
  history-rewrite path and its blast radius, crypto-shred reach into reflogs/bitmaps/backups, and the
  documented residual limit. The spine does **not** silently solve git-history erasure. *(EI-04 §1.2,
  §1.7.)*
- **GD-2 — Add a tamper-evident eDiscovery/legal-hold export** alongside DSR export receipts; the
  retention engine is tightest-policy-wins + legal-hold-aware. *(EI-04 §1.5, §1.1.)*
- **GD-3 — The fail-static staleness bound (ID-1) must be ≤ the deprovision/revocation SLA; ratify the
  chosen bound.** *(EI-02 §10.)*

### Cross-cutting (every shared system)
- **X-1 — Expose the telemetry the Phase-5 drills read as a survival signal** (an observability/
  telemetry contract every shared system implements). *(EI-01 P3.)*
- **X-2 — Adopt the shared service bootstrap harness and three-surface topology** (public gateway that
  authenticates + injects trusted identity headers / internal RPC inside the trust boundary only /
  metrics-health); the public/internal split is a **security boundary**; **liveness ≠ readiness** (a
  dead critical dependency → not-ready → shed traffic). *(EI-02 §9.)*
- **X-3 — Every queue and pool is bounded; fast-fail on saturation; statement timeouts; per-tenant
  in-flight caps.** Honour `Retry-After` in our own outbound client. *(EI-02 §5; ADR-16.)*
- **X-4 — Maintain a per-shared-system stateful-component register + blast-radius note** (enumerate
  every stateful component, give each a shared-state/sharding plan; everything else is stateless).
  *(EI-02 §10.)*
- **X-5 — Reconcile cross-component contract field names AND units at the plan layer before either side
  ships** (envelope fields, `ArtifactRef` shape, authz `list-objects` results, budget/quota/metering/SLA
  units). *(EI-01 P7.)*

---

## Phase 4 — Subsystems

- **CI-1 — Build on the ONE unified sandbox (job spec `kind ∈ {ci, agent}`); justify in writing if you
  diverge.** Isolation floor = gVisor-class userspace-kernel or microVM; plain shared-kernel containers
  rejected for untrusted code. Adopt the named hardening profile (egress default-deny, read-only root +
  tmpfs, caps dropped, no-new-privileges, seccomp, digest-pinned images with fail-closed on un-digested
  tags, whole-guest kill on teardown, `pids.max` + zero swap, secrets resolved inside the boundary and
  never forwarded via the agent runtime). *(EI-03 §3; ADR-20.)*
- **CI-2 — Pass every CI run through the universal reserve/settle cost gate** (reserve at dispatch,
  settle on completion, refuse-start-on-exhaustion, never interrupt in flight; meter one cost event per
  unit, wholesale ≠ markup). *(EI-03 §5.2.)*
- **KN-1 — Collaboration default-to-beat: resume-cursor durable transport → CAS floor → CRDT.** Build the
  resume-cursor durable transport FIRST (reconnect loses zero ops, idempotent apply); ship per-block
  optimistic compare-and-swap + soft-locks + snapshot/restore as the **named v1 floor that does not
  merge**; promote to a CRDT on the **first true concurrent-edit conflict**. *(EI-04 §2.1–2.3.)*
- **KN-2 — Store inline content as a markdown-subset string** (block structure stays AST; keep
  `mention`/`artifact_ref`/`embed` as structured nodes). *(EI-04 §2.4; EI-05 §2.)*
- **KN-3 — Model in-document databases as a property bag per row with rollups/formulas computed at READ
  TIME, never stored; materialise only when read-time recompute is measured too slow.** *(EI-04 §2.5.)*
- **KN-4 — One editor render path: read and edit run the same inline parser; controlled
  `contenteditable` (not `<textarea>`); caret = char offset into serialised markdown.** Enter-splits-
  block / caret-after-split and the serializer/offset/DOM-surgery primitives are shipped + unit-tested
  standalone before the integrated editor. *(EI-05 §2.)*
- **ISS-1 — Own the typed relation tables backing the TE-7 resolution (ties to ADR-06's relation field
  type); surface the stateful Trigger ("unblock me when…") UX.** *(EI-02 §7; EI-03 §2.4.)*
- **GIT-1 — Pseudonymous commit identities by default (commit to a stable opaque author id; person
  mapping lives in the erasable store); decide this BEFORE the git data model is fixed.** The data model
  keeps an object-backing migration seam (repos relocatable, never node-pinned). *(EI-04 §1.2, §3.2.)*
- **CHAT-1 — Explicit "run an agent here" is the v1 default; implicit auto-dispatch on casual mention is
  a separately-decided product feature.** *(EI-03 §7.)*
- **SUB-X — Every subsystem produces a blast-radius note; inherits the overlay catalogue + DL §11;
  Chat/Issues own the hover-action and width-takeover responsive cases.** *(EI-02 §10; EI-05 §1, §5.)*

---

## Phase 5 — Testing strategy (EI-01 is essentially its philosophy)

- **T-1 — Organising thesis: prove it or it isn't real.** A property does not exist until a
  failure-injection drill forces the failure and observability watches the system survive; **observability
  is part of the pass condition.** *(EI-01 P3.)*
- **T-2 — Every top risk gets a quantified gate**: RPO/RTO; "zero sandbox escapes"; "zero messages lost
  across a reconnect"; "zero cross-tenant read"; "disabled user → zero access within N minutes." Never
  weaken a threshold or invert an assertion to make a check pass; a red gate is information. *(EI-01 P3.)*
- **T-3 — Build the failure-injection harness early** (load 1×/10×/30×, mixed principal types,
  scoped-reversible dependency break, assertions from prod telemetry; cheap drills in CI, expensive
  scheduled; every incident adds a drill). *(EI-01 P3.)*
- **T-4 — The source-verified scorecard: a capability is "proven" only when a drill emits a green
  artifact; otherwise it is "claimed."** *(EI-04 §4.3.)*
- **T-5 — Named required drills** (each a scorecard item):
  - 30× agent-surge: human lane holds, agent lane sheds, other tenants unaffected. *(EI-02 §5.)*
  - Id-hiccup / fail-static: platform stays up for authenticated traffic. *(EI-02 §10.)*
  - Restore-verification + cross-seam (row↔blob↔index↔offset) integrity. *(EI-02 §11; ADR-18.)*
  - Sandbox-escape on a real kernel — **the single hard gate before any customer code runs.** *(EI-03 §3.5.)*
  - Cross-tenant IDOR. *(EI-02 §1.)*
  - Causal-loop tripwire (adversarial, AG-4). *(EI-03 §5.3.)*
  - Reconnect-loses-zero-ops (collab transport). *(EI-04 §2.3.)*
  - Reindex-from-cold parity (Search/Refs). *(EI-04 §5.3.)*
  - Editor round-trip `render(parse(md)) === md` over a markdown corpus. *(EI-05 §2.)*
  - Erasure-reaches-search / erasure-reaches-every-holder. *(EI-04 §1; ADR-12.)*
- **T-6 — Chained-mutation E2E tests** (real sessions chain mutations and update state mid-flight), not
  just isolated single-handler tests. *(EI-01 P4.)*
- **T-7 — Frontend done-bar = the switch test, reached by driving the REAL UI in a browser**: done means
  a team could move to it without hitting a wall the old tool didn't have. *(EI-05 §7; EI-01 P4.)*
- **T-8 — Frontend gates: measured-contrast over the token table; hard latency budgets (keyboard <
  ~100ms, no spinner-flash < ~1s, pages render not animate-in); test popovers/overlays against the real
  anchor.** *(EI-05 §3, §4, §5.)*
- **T-9 — Pre-ship contract reconciliation (names AND units) across every glue contract.** *(EI-01 P7.)*

---

## Phase 6 — Roadmaps ("order by non-negotiability")

- **R-1 — Sequence by what kills you first: silent data-loss and RCE/sandbox-escape floors before any
  feature surface.** *(EI-01 P2.)*
- **R-2 — Gate invariant: no later phase is "done" while an earlier phase's gate is red.** *(EI-01 P2.)*
- **R-3 — Sequence the platform-capability keystones early**: the failure-injection harness; the shared
  resilient client; the bootstrap harness; the ratchet gates; the overlay primitives and editor
  primitives (before any feature consumes them); the sandbox-escape drill as a milestone. *(EI-01 P3;
  EI-02 §5/§9; EI-05 §1.)*
- **R-4 — Every roadmap item that ships a floor names its explicit, linked follow-on** (CAS floor → CRDT;
  node-backed git → object-backed; read-time rollups → materialised; single-region → multi-cell).
  *(EI-04 §4.2.)*
- **R-5 — Named promotion triggers, not vague "v2":** the CRDT is triggered by the first true
  concurrent-edit conflict; the column-store by measured volume. *(EI-04 §2.2, §5.2.)*
- **R-6 — Budget periodic doc/code truth-up passes.** *(EI-01 P1.)*

---

## Phase 8 — Execution discipline

- **E-1 — Code wins over docs: when a doc and the code disagree, the code wins; fix the doc, then
  proceed.** *(EI-01 P1.)*
- **E-2 — Date every status/capability note with the commit/verification it was true at; report
  capabilities as "claimed" until a drill makes them "proven."** *(EI-01 P1; EI-04 §4.3.)*
- **E-3 — Maintain a durable gap report** (shipped floors + follow-ons + claimed/proven status), seeded
  with: CAS floor, single-region, pseudonymous-commit residual limit, read-time-rollup floor,
  node-backed git. *(EI-04 §4.4.)*
- **E-4 — The ratchet: every quality invariant becomes a committed CI check; an uncommitted gate is no
  gate; wire `cargo-mutants` into CI; make violations loud (ban `|| true` and swallowed errors).**
  *(EI-01 P5.)*
- **E-5 — Named lints/gates:** no-cross-tenant-predicate; no direct bus-publish outside the outbox
  helper; no host-execution path bypassing the tool trait; no blocking `ALTER` on hot tables /
  forward-only migrations; no inline colour on interactive elements; restore-verify wired into CI;
  liveness/readiness operational rule; consumer-lag monitoring. *(EI-02 §1/§4/§8/§9/§11; EI-03 §1.5;
  EI-05 §3.)*
- **E-6 — Investigate before you build: test the hypothesis before fixing (use event-replay — the log
  makes this first-class); follow the chain to root cause; triage.** *(EI-01 P6.)*
- **E-7 — Abstract at the third copy; spawn a cleanup pass the moment a workaround threatens to go
  load-bearing (the between-agents trigger).** *(EI-01 P7.)*
- **E-8 — Reserve the human for decision-shaped calls; don't churn a document while a human is reading it
  for sign-off; tag each step "just build it" vs "needs a decision first."** *(EI-01 P8.)*
- **E-9 — The sandbox-escape drill is an explicit go/no-go before any run executes untrusted customer
  code (CI or agent).** *(EI-03 §3.5.)*

---

## Legal / DPO

- **L-1 — Ratify the fail-static staleness bound (≤ deprovision/revocation SLA).** *(EI-02 §10.)*
- **L-2 — Own the erasure-vs-immutability reconciliation:** the residual limit on non-pseudonymised git
  history (history-rewrite vs documented lawful-basis limit), and Art. 17 scope into immutable history
  (GD-1/GD-2). *(EI-04 §1.)*
- **L-3 — Ratify implicit auto-dispatch on casual mention before it ships** (GDPR Art. 22 / EU AI Act
  human-oversight). *(EI-03 §7.)*
- **L-4 — Confirm the EU AI Act angle for any agent that processes personal data; design-safe minimums
  (labelling, HITL, logging) hold now.** *(EI-04 §1.6.)*

---

## Commercial

- **C-1 — Own the wallet/pricing model behind the universal reserve/settle gate; pricing history is
  immutable; wholesale ≠ markup metered separately.** *(EI-03 §5.2.)*
- **C-2 — Casual-mention auto-spawn is a product decision with intent/cost detection; ship explicit "run
  an agent here" first.** *(EI-03 §7.)*

---

## Cross-references
- [`decision-record.md`](./decision-record.md) — the consolidated decision behind these directives.
- [`analysis/`](./analysis/) — the five per-doc analyses (full per-item routing tables).
- [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md) — ADR-16…ADR-20.
- [`design-language.md`](../02-holistic-architecture/design-language.md) — §11 day-one UX mandates.
- [`external-insights/`](../../external-insights/) — the doctrine (cited inline as EI-NN).
