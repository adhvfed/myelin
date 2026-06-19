# Phase 5 — Testing IN PARTS: per-component suites, contract/seam tests, mock-agent determinism

> Phase: `05-refined-shared-systems-architecture/testing-strategy`. Canonical brief:
> [`VISION.md`](../../../VISION.md). Testing philosophy (THE source):
> [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> (prove-it-or-it-isnt-real · quantified thresholds · observability-as-pass-condition · the
> failure-injection harness · the ratchet/committed gates · name-your-floors · code-wins-over-docs ·
> drive-the-real-UI + chained-mutation E2E · order-by-non-negotiability). Hard problems:
> [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md). Phase-5 testing
> directives: [`02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
> §"Phase 5" (T-1..T-9) + §"Execution" (E-4/E-5 the ratchet) + §"Legal" (L-1..L-4). Frozen build-to surface:
> [`contract-index.md`](../contract-index.md) + [`00-reconciliation-decisions.md`](../00-reconciliation-decisions.md).
> Drill inventories consolidated here: [`03 drills-and-open-questions`](../../03-shared-systems-architecture/drills-and-open-questions.md)
> (Part A/B) + each subsystem's `architecture/07-drills-and-open-questions.md`. Spine: ADR-16/17/18/20.
> Design-QA gates: [`design-language.md` §8b](../../02-holistic-architecture/design-language.md). Date: 2026-06-19.
>
> **Companion.** This is the **IN PARTS** half of the Phase-5 testing strategy. The **AS A WHOLE** half
> (system-wide drill harness, the nine drill families F1–F9 run end-to-end across all systems, the
> source-verified scorecard, the chained-mutation real-UI E2E suite) is the sibling
> [`01-whole-system-strategy.md`](./01-whole-system-strategy.md). Where the two overlap, this doc owns the
> *unit/contract/seam* altitude and the *mock-agent determinism* layer; the whole-system doc owns the
> *cross-system drill* altitude. Every gate below names a **quantified threshold** and the **green artifact**
> it emits (T-4: a capability is "proven" only when a drill emits a green artifact; otherwise "claimed").
>
> **Reading rule for every gate in this doc (T-1/T-3):** a property does not exist until a test forces the
> failure AND the assertion is read **from the telemetry the system emits** (the survival-signal set,
> contract 1.8 / `00 §10.2`). A test that passes by *not erroring* is not a proof; it must assert against
> the emitted signal. An **uncommitted gate is no gate** (E-4): every gate below is a named CI job or a
> committed scanner/lint, not a paragraph.

---

## 0. How to read this document

Three layers, testing the system **in parts** (the rest mocked at the contract boundary):

- **§1–§2 — Per-component suites.** What each shared system and each subsystem **owns and proves in
  isolation**, with every cross-system dependency replaced by a **contract double** (the shared mock that
  implements the frozen contract). The unit/property/component altitude — fast, in CI on every change.
- **§3 — Contract / seam tests.** The **consumer-driven contract (CDC)** suite for every glue contract in
  the [contract-index](../contract-index.md): one **shared contract-test SUITE per contract**, run by every
  implementer (provider side) AND every consumer (against the double). A provider change that breaks a
  consumer **fails in CI now**, never silently in production (doctrine §7: reconcile at the plan layer; T-9:
  names AND units). The load-bearing reconciled seams (CheckStatus, the `list_objects` Filter, the
  `myelin-content` round-trip, the `#sub` tombstone ladder) get dedicated §3 sections.
- **§4 — Agent-native testing.** The mock brain's **determinism** (scripted step queue → byte-identical
  proposed-effect sequences), golden + `cargo-mutants` over the agent loop, skeleton-mode zero-spend gateway
  proof, the no-host-exec hands gate, and plan-then-apply validated against the
  `permissions ∩ delegation ∩ tenant` intersection.

**Test-double taxonomy (one vocabulary, used everywhere below).**

| Double | What it is | When used |
|---|---|---|
| **Contract double** | A shared in-repo fake that implements a frozen contract's trait/signature, ships in a `*-testkit` crate beside the real impl, and is **itself verified by the CDC suite (§3)** so it cannot drift from the provider. | Every per-component suite (§1/§2) that consumes another system. |
| **Mock runtime** | The deterministic `MockAgentRuntime` (`--use-mock`) — a **real runtime flag on the same code path** (AG-4), not a test-only stub. | §4 agent loop + golden + mutation tests; every per-component suite that needs an agent. |
| **Skeleton runtime** | `SkeletonAgentRuntime` — no model, no tools, zero spend, zero effects (AG-3). | §4.3 gateway/identity/dispatch/reserve/trace path proof. |
| **Replay fixture** | A recorded `correlation_id` event tree replayed deterministically (the event log makes this first-class, E-6). | Determinism, reindex-parity, and chained-mutation seam tests. |

**The committed-gate manifest (E-4).** Every gate in this doc resolves to one row of a single committed
`testing-manifest.toml` (the ratchet): `{ gate_id, layer, owner_crate, ci_job, threshold, green_artifact,
telemetry_signal }`. A red row blocks the workspace PR (contracts are stable per ADR-01: a break is one
whole-workspace PR that fails every consumer's build now). The manifest is the machine-readable index of
this document; CI fails if a contract in the contract-index has no manifest row (the **`contract-coverage`
scanner**, §3.0).

---

## 1. Per-SHARED-SYSTEM suites — what each owns and proves in isolation

Each shared system is testable **alone**, with every other system replaced by its contract double. The table
gives, per system: the **isolation boundary** (what is mocked), the **unit/property suite** it owns, and the
**in-isolation drills** it owns (the per-system rows from the Phase-3 inventory A.2, run here against
doubles; their *cross-system* instances run in the whole-system doc). Quantified gate + green artifact named
for each.

### 1.1 Substrate / service shell (`00`, `myelin-client`, harness)

- **Mocked at the boundary:** nothing below it (this is the floor); the consumers above are exercised via a
  **reference service** (`harness-probe`) that boots through `serve(AppSpec)`.
- **Owns (unit/property):**
  - `serve(AppSpec)` boot ordering property: **migrate → outbox relay → consumers → three ports → drain**
    happens in that order; **liveness ≠ readiness** (a dead critical dep ⇒ not-ready ⇒ sheds; liveness stays
    green so no restart-storm). Gate **SUB-G1**: the probe reports `ready=false` within **≤ 1 readiness
    tick** of a killed critical dep, `live=true` throughout; **0 restart-storms**. Artifact: `serve-order.json`.
  - `ResilientClient::call` property matrix: timeout fires at the configured ms; breaker opens after the
    threshold and **never retries through a tripped breaker**; bulkhead caps in-flight; **jittered retry only
    for idempotent calls**; `Retry-After` honoured. Gate **SUB-G2** (= drill SUB-D5): trip a downstream
    breaker ⇒ callers fail fast, **0 retry-through-breaker**, **0 amplification**; Retry-After obeyed to the
    second. Artifact: `resilient-client-matrix.json`.
  - `FailStatic<T>` property: serves a **bounded-staleness** answer while the source is down;
    `static_max ≤ revocation SLA` and `≥ agent-token TTL` is a **compile-time assertion** (the bound is a
    const, checked by a unit test that fails the build if violated). Gate **SUB-G3**: stale answers carry the
    `fail-static stale` signal; a zookie-stamped read **bypasses** the cache. Artifact: `failstatic-bound.json`.
  - **Architecture lints** (contract 1.6) each get a **fixture that must fail the build**: `no-cross-db`,
    `no-raw-publish`, `tenant-predicate`, `no-host-exec`, `forward-only-migration`, `no-cross-sync-cycle`,
    `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`,
    `no-untagged-personal-data`, `flow-determinism`. Gate **SUB-G4** (the ratchet, E-5): each lint ships with
    a **red-fixture** (code that MUST be rejected) and a **green-fixture**; CI asserts the lint fails the red
    one. A lint without a red-fixture is "claimed, not proven." Artifact: `lint-fixtures.json` (12 rows, all
    green = lint correctly rejects).
  - **Telemetry signal-set conformance** (contract 1.8): the probe exports RED/USE + consumer-lag +
    outbox-depth + breaker-state + fail-static ratios + shed-counts + causal-depth. Gate **SUB-G5** (X-1
    directive): a system that emits no survival signal **fails the harness conformance test** — observability
    is a pass condition. Artifact: `telemetry-conformance.json`.
- **In-isolation drills owned:** SUB-D1/D2 (outbox no-ghost + reconnect zero-loss, F5), SUB-D9 (liveness),
  SUB-D10 (online migration expand→backfill→contract under load, **0 blocking lock beyond budget, 0
  downtime**). The surge/IDOR/restore instances (SUB-D3/D6/D7) run cross-system in the whole-system doc.

### 1.2 Identity & Access (`myelin-identity`)

- **Mocked at the boundary:** the consuming subsystems (Git/CI/Issues/KN/Chat) replaced by a **namespace
  fixture** that loads each subsystem's frozen ReBAC fragment (contract 4.9) into the cell schema; the bus
  replaced by an outbox double.
- **Owns (unit/property):**
  - `authenticate` over **every credential kind** (SSO/SCIM/passkey/SSH/PAT/CI/agent/deploy-key + the
    machine-identity resolutions): produces a `Principal` whose **tenant comes from the credential, never the
    URL** (ID-3). Gate **ID-G1**: a path-tenant ≠ token-tenant request is an IDOR ⇒ **0 cross-tenant**
    (= drill ID-D3/F2). Artifact: `authn-credential-matrix.json`.
  - `check` fail-closed property + `CaveatContext{object, field?, transition?, attrs}` ABAC: a
    field/transition caveat is evaluated **off the hot `list_objects` path**. Gate **ID-G2**: an
    unauthorised field/transition is `Deny`; a malformed caveat is `Deny` (fail-closed), never `Allow`.
  - `list_objects` → `Ids | Filter{SetExpr, zookie}` — **the single most load-bearing contract** — gets a
    dedicated property suite: every `SetExpr` variant (`All/None/Ids/NotIds/InRelation/Union/Intersect/
    Difference/TupleSet`) **lowers to a SQL predicate/JOIN over an arbitrary consumer id column**, **no N+1,
    no post-filter**. Gate **ID-G3** (= the §3.5 seam): **0 leaked ids**, **0 count/IDF leak**, query plan
    has **0 post-filter rows discarded** (asserted by reading the plan's rows-removed-by-filter counter = 0).
    Artifact: `list-objects-pushdown.json`.
  - `list_subjects`/`explain` at **50k-member-channel density** (read-fanout): p99 within the (Phase-5
    measured) budget. Gate **ID-G4**.
  - `delegation` monotone-intersection property: `agent.policy ∩ delegation ∩ tenant.policy` is **monotone
    (attenuation, never up)** — a macaroon caveat can only narrow. Gate **ID-G5** (= drill ID-D5).
  - `mint_run_token`/`revoke` lifecycle: token life == run life; **re-mintable mid-workflow on resume**;
    teardown revocation is **idempotent even on crash**; **0 shared platform token leaked into the child
    env**. Gate **ID-G6** (= drills ID-D6/AG-D8, F8).
  - Zookie/consistency property: read-your-writes; a zookie-stamped read **bypasses the fail-static cache**;
    the authz reverse index honours the **revision watermark** (the "new enemy" property). Gate **ID-G7**
    (= drills ID-D7, F8): revoke then re-read with the post-revoke zookie ⇒ **0 stale allow**.
- **In-isolation drills owned:** ID-D1 (disabled-user → **zero access within N = 5 min** default, F8),
  ID-D2 (fail-static survives + just-revoked still denied, F7), ID-D3/D4 (IDOR + no-leak), ID-D5 (delegation
  confinement), ID-D6/D7 (token revocation + new-enemy). ID-D8/D9 (restore, surge) run cross-system.

### 1.3 Event Bus (`myelin-events`) + Signals/Triggers/firehose

- **Mocked at the boundary:** producers/consumers replaced by a **handler double**; the `QueryAst` evaluator
  is real (it is the shared `myelin-query` core, contract 3.4/13.3).
- **Owns (unit/property):**
  - `OutboxTx::emit(draft, cause)` is the **only** emit path: the `no-raw-publish` lint forbids any
    `publish_now`; causality is **correct-by-construction** (root carries, `causation_id` = immediate parent,
    `depth+1`). Gate **BUS-G1** (= drill BUS-D4/D5, F5): crash between state-commit and publish ⇒ event still
    delivered (outbox survived), **never delivered without the state change** (0 ghost, 0 lost). Artifact:
    `outbox-no-ghost.json`.
  - `EventHandler` template: `subjects()` whitelist (**never `*`** — a lint), durable-bind-by-name,
    ack-after-enqueue, dedup ledger, bounded prefetch. Gate **BUS-G2** (= drill BUS-D2, F5/HoL): flood
    unhandled types at a consumer ⇒ **does not stall**, terminates poison, **lag alarm fires**. Property:
    `consumer_dedup (consumer, event_id)` PK absorbs at-least-once redelivery ⇒ **0 duplicate effect**.
  - **Per-aggregate ordering** at production QPS (D-9): per-ref / per-conversation `UNIQUE(aggregate, seq)`,
    relay `FOR UPDATE SKIP LOCKED`. Gate **BUS-G3** (= BUS-D9): burst force-pushes to one hot ref ⇒
    `git.ref.updated` **in push order per ref, parallel across refs**, at target QPS; **0 lost/ghost**.
  - Schema-evolution upcasters `(type, from_ver) → to_ver` are **pure** (a property test: upcast is
    deterministic + forward-only). Gate **BUS-G4**.
  - `EventMatcher` = the frozen `QueryAst`: **bounded interpreter, no UDFs/loops/recursion, statically
    cost-bounded, permission-aware**. Gate **BUS-G5**: a matcher that cannot be statically cost-bounded is
    **rejected at registration** (not at runtime) — the cost bound is a parse-time property.
  - **Firehose resume-cursor protocol** (contract 3.5, NEW): `subscribe/resume(stream, scope, last_seq)`
    backfills `(last_seq, now]` then live; `resync_required → *.snapshot`; **scope is a bounded selector,
    never `*`**. Gate **BUS-G6** (= the reconnect-zero-loss family, owned-instance for the transport):
    sever mid-publish ⇒ resume recovers the gap, **0 lost, 0 duplicate**; over-window ⇒ snapshot fallback,
    still 0 lost. (Chat KD/D-C1 and Knowledge KD-1 are the subsystem instances of this — §2.)
- **In-isolation drills owned:** BUS-D1..D6 (the above), BUS-D8 (erasure: inline-PII events unrecoverable +
  `*.erased` tombstones). BUS-D7 (surge) runs cross-system.

### 1.4 Reference Graph (`myelin-refs`)

- **Mocked at the boundary:** each subsystem's `project(ref, viewer)` + `#sub` resolver replaced by a
  **projection double**; `list_objects` replaced by the Id `Filter` double; the typed-relation tables
  (Issues/KN) by a **typed-edge double**.
- **Owns (unit/property):**
  - `ArtifactRef` `parse`/`format` **rejects ambiguity / scope-less refs**, never guesses scope (REF-3);
    the Issues key grammar `<PROJECTKEY>-<seqno>` is the stored canonical, `#1421` is render-time only.
    Gate **REF-G1**: a fuzz corpus of malformed/ambiguous URNs ⇒ **100% rejected, 0 guessed scope**.
  - `resolve(ref, viewer, mode)` leak-free + **cell-local** (OQ-I): denied ⇒ tombstone, never the title.
    Gate **REF-G2** (= drill REF-D1/F1): a confidential artifact referencing a public one is **absent from
    backlinks/traverse/resolve** for an unauthorised viewer, incl. under zookie staleness. **0 leak.**
  - `traverse`/`backlinks` bounded cycle-safe recursive-CTE (depth **16**): a 1000-deep chain + a cycle ⇒
    CTE terminates (visited-set + depth ceiling), cycle surfaced as a diagnostic, statement-timeout
    respected. Gate **REF-G3** (= REF-D8).
  - The **`#sub` tombstone ladder** (the 4-step frozen ladder) — dedicated seam in §3.6.
  - `reindex(scope)` rebuilds the edge index + projection cache **via the live consumer path only, never
    reading owner DBs**; a TE-7 drift reconverges to the typed table (typed wins). Gate **REF-G4**
    (= REF-D4/F4): wipe `edge`, reindex ⇒ **byte-matches live**.
- **In-isolation drills owned:** REF-D1/D2/D4/D5/D6/D8/D9 (no-leak, IDOR, reindex parity, erasure tombstone,
  new-enemy, traversal bound, sub-artifact tombstone). REF-D3/D7/D10 cross-system.

### 1.5 Search (`myelin-search`)

- **Mocked at the boundary:** Id `list_objects` `Filter` double; each subsystem's `IndexSpec` projection
  feed via a **snapshot replay fixture**.
- **Owns (unit/property):**
  - `query(ast, viewer, zookie?, page)` **always conjoins the `list_objects` `Filter` before scoring** — the
    **`search-requires-acl-filter` lint** makes a non-conjoined query path a build failure. Gate **SRCH-G1**
    (= drill SRCH-D1/F1): a confidential/overridden/private artifact never appears in any result **incl.
    counts, IDF, "more results", RAG**. **0 leak, 0 count-leak.** Artifact: `search-acl-conjoin.json`.
  - `semantic` k-NN is **filter-during-traversal** (k visible NN), not k-then-filter: gate **SRCH-G2**
    (= SRCH-D8): `recall@k ≥ threshold` under a selective ACL filter AND **0 leak**.
  - `reindex(scope)` is the **only** rebuild path (SEARCH-1), sub-artifact-granular `*.snapshot` replay.
    Gate **SRCH-G3** (= SRCH-D5/F4): wipe index, reindex ⇒ matches live (docs, ACL, ranking, vectors),
    **live consumer path only**.
  - HYOK structural skip: `can_derive_plaintext_index()=false` ⇒ Search **never indexes** that class.
    Gate **SRCH-G4** (= SRCH-D10): **0 HYOK plaintext in any derived store**.
- **In-isolation drills owned:** SRCH-D1/D2/D4/D5/D7/D8/D10. SRCH-D3/D6/D9 cross-system.

### 1.6 Notifications (`myelin-notif`)

- **Mocked at the boundary:** Bus Signals via a **signal double**; Refs `resolve(Display)` via a projection
  double; Id `check`/`list_subjects` doubles; `DeliveryAdapter` via a **capture adapter** (records
  `RedactedMessage` + `idem_key`, asserts no PII off-cell).
- **Owns (unit/property):**
  - `list_inbox` is **the ONE inbox** (C-9): "My Work"/"Activity"/"Review requests" are **filters over
    reason/subject, never a second store**. Gate **NOTIF-G1**: a scoped view returns a **strict subset** of
    `list_inbox` for the same principal (property: `scoped ⊆ inbox`).
  - `humanise((template_key, args), viewer, locale)` is **the ONE templating surface** (OQ-L): resolves each
    `ArtifactRef` per-viewer via Refs `resolve(Display)`, **permission/erasure-safe**, ICU MessageFormat —
    **no agent-authored raw strings**. Gate **NOTIF-G2** (= drill NOTIF-D4/D6, F1): a notification about a
    confidential subject to a viewer lacking access humanises to **the tombstone** ("a restricted issue"),
    the title **never** appears; an erased user humanises to `[erased user]`. **0 title/PII leak.** This is
    the **`"merge_request merged"`/raw-id/unrendered-markdown #1-unfinished-tell gate** (NOTIF-1, design §8b.5).
  - Ranking property: every `critical`/`direct` ranks above every `fyi`, with an **explain-trace per rank**.
    Gate **NOTIF-G3** (= NOTIF-D1): **0 critical below an fyi**.
  - Storm-control/dedup: N near-identical events collapse to one item (`coalesce_count` correct, "+N more");
    self-notifications suppressed. Gate **NOTIF-G4** (= NOTIF-D2).
  - `DeliveryAdapter` idempotency: `UNIQUE(idem_key)` collapses a retry to **exactly-one effective delivery**
    per (item, channel). Gate **NOTIF-G5** (= NOTIF-D9).
- **In-isolation drills owned:** NOTIF-D1/D2/D4/D6/D8/D9/D10 + D7 (escalation resume, F5: page next step
  **exactly once**). NOTIF-D3/D5 cross-system.

### 1.7 Agent Fabric (`myelin-agent`) — see §4 for the full agent-native suite

- **Mocked at the boundary:** every subsystem effect via the **`EffectApi` public-endpoint double**; Id
  `check`/`delegation`/`mint_run_token` doubles; the sandbox via the **`ToolHands` channel-proof double**
  (emits a marker, no host exec); the brain via the **mock/skeleton runtime**.
- **Owns:** the full §4 suite (determinism, golden, `cargo-mutants`, skeleton zero-spend, no-host-exec,
  plan-then-apply intersection). The contract surface 8.1–8.8 is exercised here; its **seam** to every
  subsystem (ToolDef registration, `requires_approval` defaults) is §3.7.

### 1.8 Durable Workflow (`myelin-flow`)

- **Mocked at the boundary:** activities (`step`/`exec`/CI dispatch) replaced by **activity doubles**; the
  bus via outbox double; the timer wheel is **real** (it is the system under test).
- **Owns (unit/property):**
  - `WfCtx` determinism: non-determinism (`now`/`rand`/`emit`/IO) is **journaled**; the **`flow-determinism`
    lint** forbids un-journaled non-determinism in a workflow body. Gate **FLOW-G1** (= drill FLOW-D2):
    replay against a divergent/wrong-version definition ⇒ the divergence guard **halts as `nondeterministic`
    + dead-letters**; **0 silent divergence, 0 double-effect**.
  - Crash/resume exactly-once-in-effect: kill a worker at activity 5/10 ⇒ another re-leases, replays,
    resumes at step 6, **0 re-executed side effects, 0 lost progress**. Gate **FLOW-G2** (= FLOW-D1/F5).
  - Journal+outbox atomicity: crash between journaling an activity DB write and emitting its event ⇒ both in
    **one txn**, **0 ghost, 0 lost**. Gate **FLOW-G3** (= FLOW-D5).
  - Timer wheel at scale: arm **1M+ durable timers** + a burst due in one minute ⇒ due timers fire within
    the tick budget, far-future cost ~nothing, a crash re-fires unfired (**effectively-once, 0 lost, 0
    double-fire**). Gate **FLOW-G4** (= FLOW-D3, SC-11).
  - Multi-day HITL durable signal: a gated workflow waits across a worker restart + deploy; an
    `approval`/`ci.result`/`job.done` arrives hours/days later (double-click) ⇒ resumes, **consumes once**,
    runs/withholds correctly, **withheld tool does not mutate**. Gate **FLOW-G5** (= FLOW-D4) + the
    **per-effect `idem_key`** property (`card_id` single / `card_id:<idx>` multi/partial — a double-click is
    one approval, a partial approval is well-defined). **This is the `SCHEDULE_AND_RUN_JOB` idiom's
    correctness gate** (the workflow holds no runtime; completion is a durable signal keyed by `idem_token`).
- **In-isolation drills owned:** FLOW-D1..D5/D9. FLOW-D6/D7/D8/D10 cross-system.

### 1.9 Storage (tiers / `BlobStore` / KMS / reserve-settle / backup-restore)

- **Mocked at the boundary:** the OLTP pool is real (RLS under test); the blob backend swappable (fs double);
  KMS via a **`KeyOrigin` double** with a destroyable key.
- **Owns (unit/property):**
  - `BlobStore{put,get,head,delete}` content-addressed (BLAKE3, per-tenant dedup); **fs↔object one-line
    swap** is a property (same test passes against both backends). Gate **STOR-G1** (= STOR-D7): corrupt an
    object ⇒ **re-hash-on-read detects it** (content-address mismatch), recover from replica; **0 silent
    serve**. Plus the **trust-scoped cache namespace** property: an `UntrustedFork` write **cannot reach the
    trusted cache scope** (§3 ties to the CheckStatus trust tier).
  - KMS hierarchy + crypto-shred granularity (per-subject DEK incl. CI log segments): destroy the key ⇒
    ciphertext **unrecoverable incl. backups**. Gate **STOR-G2** (= STOR-D4): **0 recoverable PII in any
    backup** after a per-subject shred.
  - `residency-pin`: a write where `row.region ≠ cell.region` is **rejected by construction**. Gate
    **STOR-G3** (= STOR-D5): **0 cross-region personal-data egress**; `residency_verify` attestation passes.
  - **Backup/restore + cross-seam** (ADR-18): `restore(to_offset)` lands at a point where **OLTP rows ↔ blob
    ↔ index ↔ event-log offsets are mutually consistent**; `restore-verify` is **CI-gated**; `post_restore_
    reerase` runs. Gate **STOR-G4** (= STOR-D1/D2/D3, F3, the **headline durability gate**): **RPO ≤ 5 min**,
    **RTO ≤ 1h/tenant ≤ 4h/cell**, **0 loss (checksum parity)**, **0 row→missing-blob**, **0 resurrected
    erased subject**. Artifact: `restore-verify.json` (the green artifact ADR-18 demands; un-drilled = a
    claim). **This is the order-by-non-negotiability #1 (silent data loss outranks every feature).**
  - Reserve/settle cost gate property: reserve at dispatch (**no balance ⇒ no start**), settle on completion,
    **never interrupt in-flight**, integer minor-units, wholesale ≠ markup. Gate **STOR-G5** (fronts every
    agent run + every CI run + every `SCHEDULE_AND_RUN_JOB`).
- **In-isolation drills owned:** STOR-D1..D8.

### 1.10 GDPR / Audit / `PersonalDataHolder` (`myelin-gdpr`)

- **Mocked at the boundary:** each holder via a **holder double** that implements
  `PersonalDataHolder{locate, export, rectify, restrict, erase}`; the DSR orchestrator is real.
- **Owns (unit/property):**
  - **Holder-completeness**: a subject seeded into **all H1–H18** holders ⇒ the `data_map()`-driven fan-out
    hits **every** holder; post-erase `locate` returns **0 recoverable PII**. Gate **GDPR-G1** (= drill
    GA-D1): **0 holders missed**. (The CDC property in §3.3 makes every store's `PersonalDataHolder`
    implementation **provably present** — a store the harness opens auto-registers (contract 1.4) and the
    `no-untagged-personal-data` lint fails the build on an untagged field.)
  - `#[personal_data(...)]` classify-derive + the `no-untagged-personal-data` lint: add an untagged field ⇒
    **build red**; the data-map diff surfaces it. Gate **GDPR-G2** (= GA-D5).
  - `dsr_submit/status/certificate → MerkleProvenBundle`: the 1-month durable timer fires a warning Signal
    before deadline; the certificate seals on completion. Gate **GDPR-G3** (= GA-D4): **0 silent misses**.
  - Tamper-evident audit log: retroactively edit/delete an entry ⇒ the hash-chain breaks + the consistency
    proof against the published STH fails + the external witness mismatches. Gate **GDPR-G4** (= GA-D3):
    **tamper detected 100%**.
  - `restrict` suppression: a restricted subject ⇒ **no indexing/agent-use/analytics/notification** while
    storage is retained; reversible. Gate **GDPR-G5** (= GA-D7): **0 processing of a restricted subject**.
  - The **ONE free-text/immutable erasure posture** (contract 10.9, NEW): per-subject DEK crypto-shred +
    pseudonym-map shred + `restrict`; the residual (third-party/immutable free-text) is a **documented
    lawful-basis limit** + best-effort rectify/tombstone. Gate **GDPR-G6**: erase a subject ⇒ self-authored
    free-text **unrecoverable**, residual **named in the gap report** (`[OPEN — LEGAL]` ratification tracked,
    not silently green — never invert the assertion to make it pass, T-2).
- **In-isolation drills owned:** GA-D1..D7. GA-D8 (multi-cell, FLOOR) owed when built.

### 1.11 Tenancy & control plane (`myelin-tenancy`)

- **Mocked at the boundary:** cells via **cell doubles**; the partition-key injection is real.
- **Owns (unit/property):**
  - `control-plane-pii-free` lint: a name/email column in the control-plane schema ⇒ **build fails**. Gate
    **CP-G1** (= CP-D1): data-map over the CP schema = **0 `is_personal=true` columns**.
  - `discover`/`place`/`placement_of` PII-free routing + **repo-granular placement** (region-pinned,
    relocatable, never node-pinned). Gate **CP-G2** (= CP-D2): a request to a cell for a `tenant_id` it
    doesn't host ⇒ **misroute rejection, 0 cross-tenant/cross-cell read, audited**.
  - `residency_verify(tenant_id) → SignedAttestation` incl. **CI runner/log/artifact/cache region** (the
    no-global-pool property). Gate **CP-G3** (= CP-D3): the attestation passes; an out-of-region write is
    rejected.
  - **Cross-cell PII-free pointer bridge** `CrossCellPointer{subject(opaque), type, correlation_id,
    home_cell}` — resolution **always cell-local**. Gate **CP-G4** (= CP-D8, FLOOR): the bridge carries only
    the three fields, the target cell resolves per-viewer, unauthorised ⇒ tombstone. **0 PII crosses.**
- **In-isolation drills owned:** CP-D1/D2/D3/D6 + the FLOOR drills CP-D7/D8 (owed when multi-cell builds).

---

## 2. Per-SUBSYSTEM suites — what each owns and proves in isolation

Each subsystem is testable **alone**, the shared layer replaced by the §1 contract doubles and the **other
subsystems by their `project`/`#sub`/ToolDef doubles**. Each subsystem's `07-drills-and-open-questions.md` is
its **obligation register**; this section names the *in-isolation* gate per drill (the cross-system instances
run in the whole-system doc). The frozen-contract conformance each subsystem owes is in §3 (every subsystem
implements `project`, `replay`, `PersonalDataHolder`, the outbox emit, its ReBAC fragment, its ToolDefs).

### 2.1 Git hosting

- **Owns:** the `check_status` **projection table + `run_attempt` supersession + branch-protection
  `required`-set + fork-endorsement** (the Git half of the CheckStatus seam — §3.4); **pseudonymous commits
  by default** (GIT-1: the immutable bytes never contain erasable PII); **content-anchored line-range
  fingerprints** (the Git half of the `#sub` ladder — §3.6); the object-backing seam.
- **In-isolation gates (from git `07`):** **D-1** per-ref ordering at push QPS (0 lost/ghost; per-ref order
  == outbox order); **D-10** the X-1 check-seam correctness drill (a) `run_attempt`-monotonic supersession
  holds the correct current row + **drops stale lower attempts**, (b) a **fork cannot self-green** (untrusted
  success is neutral), (c) endorsement flips the gate, (d) a doubly-delivered `ci.result` wakes the workflow
  **exactly once** ⇒ **merge count == 1, 0 double-merge**. Green artifact: `git-check-seam.json`.

### 2.2 Continuous Integration

- **Owns:** the **unified sandbox** (`kind ∈ {ci, agent}`, ADR-20) + the four uniform guarantees; emits
  `ci.check.updated` + `ci.result` + `trust_tier` + `run_attempt` (the CI half of §3.4); the T3 log tier
  `(job, step, byte-range)` index that the jump-to-failure `details_ref` resolves through.
- **In-isolation gates (from CI `07`):** **T-1 the escape drill** — the **single hard go/no-go** that gates
  ALL untrusted execution (CI and agent `ToolHands::exec`): an adversarial corpus on a **real kernel**
  (kernel-exploit primitives, cloud-metadata SSRF to 169.254.169.254, control-plane/internal-RPC reach,
  cross-tenant network/storage, fork bomb, disk fill, secret exfil via egress) ⇒ **ZERO escapes**, or **CI
  is no-go for untrusted code**; re-run on every backend/image/kernel change. Green artifact:
  `sandbox-escape-attestation.json` (no artifact ⇒ untrusted code does not run — R-1/E-9). **D-8** the
  CheckStatus producer side (idempotent emit, monotonic `run_attempt`, neutral fork). **This is
  order-by-non-negotiability #1 alongside silent data loss: RCE/sandbox-escape before any feature surface.**

### 2.3 Issue tracker

- **In-isolation gates (from issues `07`):** **D3** permission-leak (confidential + cross-tenant IDOR) ⇒
  **0 leak** incl. under zookie staleness (the `SetExpr` JOIN, search, backlink, context-pane); **D8**
  rollup freshness under an import-storm (bounded ancestor recomputes via debounce) + reindex parity
  (drift-free vs live); **D10** editor round-trip `render(parse(md)) === md` over issue bodies + comments
  (the consumed `myelin-content` subset — §3.5); **D12** workflow-guard correctness ("can't mark Done while
  CI red" reads the frozen `CheckStatus` + trust posture; "can't close while `blocked_by` open") ⇒ the
  transition is **blocked with a pre-assembled reason**, and an agent at a governed transition is
  **HITL-gated** per the frozen `requires_approval` default (the tool withheld, **no mutation**); **D14** the
  **switch-test** (a Jira/Linear user completes create→triage→plan→board→done **without a manual**) + measured
  **contrast/latency** gates on the primary screens incl. empty/loading/error/permission/erased/agent-pending
  states (design §8b).

### 2.4 Knowledge platform

- **In-isolation gates (from knowledge `07`):** **KD-1** the headline **reconnect-loses-zero-ops** (Knowledge
  owns it): kill a collab client mid-edit + sever during a sustained multi-author edit; on
  `firehose::resume(scope=doc:<id>, last_seq)` ⇒ **0 ops lost, 0 duplicate** (`UNIQUE(op_id)` idempotent
  apply), re-run **across an `engine_promote` (CAS→CRDT) boundary**; **KD-2** editor round-trip
  `render(parse(md)) === md` over the corpus incl. the three structured nodes (`U+FFFC`-anchored × nesting in
  bold/lists/tables, code, IME/paste) ⇒ **100% round-trip, 0 regressions** (§3.5); **KD-3** CAS floor: a
  concurrent same-block edit ⇒ the loser is **rejected with current state**, never silently overwritten ⇒
  **0 silent overwrites** (and CAS conflict rate is the **CRDT-promotion trigger metric** — a named floor,
  KN-1); **KD-4** erasure reaches every holder (per-subject DEK crypto-shred + pseudonym shred + vector
  purge) ⇒ **0 recoverable structured PII incl. vectors**; **KD-5** permission-filtered reads incl.
  **count-leak** ⇒ **0 leaked artifacts, 0 count-leak** (the `SetExpr` conjoin is *inside* the query);
  **KD-11** agent edits governed + attributed (the four uniform guarantees) ⇒ **0 ungoverned agent mutation,
  0 mutation before approval, 0 double-apply**.

### 2.5 Chat

- **In-isolation gates (from chat `07`):** **D-C1** **zero messages lost across a reconnect** (the OQ-J pass
  condition): sever gateway↔firehose mid-publish ⇒ `resume(stream, scope, last_seq)` recovers the gap ⇒
  **0 lost, 0 duplicate**; over-window ⇒ `resync_required → *.snapshot`, still 0 lost; **D-C2**
  per-conversation total order at scale (ULID `message_id`, `aggregate=conversation_id`) ⇒ **order preserved,
  resume gap-free**; **D-C9** the HITL approve→resume bridge **exactly-once** (kill Chat + Workflow mid-wait,
  approve days later) ⇒ the gated tool runs **exactly once**, a double-click is one approval (`idem_key=
  card_id`), deny withholds with **no mutation**, timeout auto-denies; **D-C10** batch/partial approval
  well-defined (`idem_key=card_id:<idx>`) ⇒ 2-of-3 resumes the 2, withholds the 1, **no double-run**;
  **D-C17** explicit-first dispatch (CHAT-1) ⇒ a casual `@agent` mention **notifies, does NOT spawn a costed
  run**; reserve/settle gates even the explicit run (**no balance ⇒ no run**). **The TE-21 build-gate:**
  D-C3/D-C4 (presence-at-scale + reconnect thundering-herd) run **early against the Rust gateway** — if Rust
  holds, the cross-language divergence hatch (contract 1.7) stays closed.

---

## 3. CONTRACT / SEAM tests — one shared suite per glue contract (consumer-driven, CI-breaking)

> **The thesis (doctrine §7 + T-9 + ADR-01).** Two components that exchange data must agree on field names
> **and units** before either ships; a unit mismatch that calcifies is brutal to unwind. We enforce this
> with **consumer-driven contract (CDC) tests**: for each contract, the **consumers** publish the shape +
> behaviour they depend on as an executable expectation; the **provider** runs those expectations in its own
> CI. **A provider change that breaks a consumer fails the provider's CI** — never silently in production.
> Because a contract is stable (ADR-01), the break is one whole-workspace PR that fails every consumer's
> build *now*. The CDC suite **also verifies the contract double** (§0) so an in-isolation suite can never
> pass against a double that has drifted from the real provider.

### 3.0 The shared CDC harness + the contract-coverage scanner

- **Shape:** every contract in the [contract-index](../contract-index.md) gets a `contracts/<id>/` directory
  with (1) the **frozen wire/trait shape** (the names-and-units anchor — the `EventEnvelope` field list +
  units `00 §2.10`, the `ArtifactRef` token table Bus §6.2 are the two CONFIRMED-unchanged authorities every
  other contract aligns to), (2) the **consumer expectation set** (one file per consumer named in the
  contract-index row), (3) the **provider verification harness** the owner runs.
- **The ratchet gate (E-4/E-5) — `contract-coverage` scanner (committed):** CI **fails the workspace** if a
  contract-index row has **no** `contracts/<id>/` directory, OR a contract has a **consumer named in the
  index with no expectation file**, OR a provider has no verification harness. Gate **CDC-G0**: **every
  contract-index row has full provider+consumer coverage**; green artifact: `contract-coverage.json` (one
  row per contract, all green). An uncommitted contract test is no contract test.
- **Units gate (T-9):** the CDC shape carries the **frozen units** (timestamps RFC-3339 UTC; budgets/costs
  integer minor-units; TTLs/staleness/timers seconds; resilient-client timeouts ms; `pii_key_ref =
  kms://<tenant>/<dek-epoch>/<class>`). A consumer expectation that asserts a wrong unit (e.g. ms where
  seconds are frozen) **fails at compile** (the shape is a typed newtype, not a bare integer). Gate
  **CDC-G1**: **0 unit mismatches across all seams** — the doctrine §7 "100× scale difference" class is
  impossible by construction.

### 3.1 The universal per-subsystem contracts (every subsystem implements; one suite each)

Each of these is **REQUIRED on every subsystem** (the contract-index says so); the CDC suite is run by every
implementer and every consumer-double:

| Contract | Provider obligation (CDC asserts) | Consumer-side (the double must satisfy) | Gate |
|---|---|---|---|
| **`project(ref, viewer)`** (5.6) | returns `{title, state, icon, render_hint, sub_anchor?}` **per-viewer, pre-permission-checked**; a denied viewer ⇒ the projection is the **tombstone**, never the title. | Refs/Search/Notif read another subsystem **only** via `project` — never the owner DB. | **CDC-P** : 0 leak across a denied-viewer corpus; every subsystem's `project` round-trips the frozen shape. |
| **`replay(scope, since)`** (2.6) | re-emits `*.snapshot` via the outbox **through the live consumer**, **sub-artifact-granular** (CI one-run, KN page-subtree at block granularity). | Search/Refs/OLAP/Notif rebuild **only** via replay (reindex-from-source, F4). | **CDC-R** : reindex-from-cold == live (the F4 family); **0 bespoke recovery reader** (a code-path assertion). |
| **`PersonalDataHolder{locate,export,rectify,restrict,erase}`** (10.1) | every store the harness opens **auto-registers** (1.4); erase = purge/crypto-shred/pseudonymise, **never hide**. | the DSR orchestrator fans out to **every** registered holder (the GA-D1 completeness property). | **CDC-H** : a subject seeded into every holder ⇒ **0 recoverable PII** post-erase, **0 holders missed**. |
| **outbox `emit`** (2.2) | the **only** sanctioned emit path; same tx; **no `publish_now`** (the `no-raw-publish` lint). | every consumer dedups on `(consumer, event_id)`; causality nested. | **CDC-O** : crash between commit and publish ⇒ **0 ghost, 0 lost** (F5). |
| **ReBAC namespace fragment** (4.9) | each subsystem declares relations + permissions; compiled into **one** cell schema; frozen fragments (Git ref-glob+CODEOWNERS+`approve_untrusted_ci`; CI `read & !is_untrusted_fork`; Issues field/transition caveats; KN page-tree inherit; Chat `member + parent_project->read`). | Id compiles all fragments; a fragment that doesn't compile **fails the build**. | **CDC-N** : every fragment compiles + its no-leak property holds in `list_objects`. |
| **ToolDef registrations** (8.1) | each subsystem registers `ToolDef{name, input_schema, required_caps, effect_kind, side_effecting, requires_approval, exposed_over_mcp}` with the **frozen `requires_approval` defaults**. | the Agent `ToolSurface` resolves them; a missing/mismatched default **fails CDC**. | **CDC-T** : every subsystem's ToolDefs match the frozen §6.3 defaults table (§3.7). |

### 3.2 The names-and-units anchors (the two highest-fan-in, frozen)

- **`EventEnvelope` (2.1)** — every emitter + consumer. CDC asserts the **exact field list + units** (event_id
  ULID, type, schema_ver, tenant, region, actor, subject ArtifactRef, aggregate, correlation/causation/depth,
  contains_personal_data/data_role/visibility/pii_key_ref, occurred_at/recorded_at, payload =
  references-not-payloads). Gate **CDC-ENV**: a new emitter with a missing/renamed/wrong-unit field **fails
  CI**; an inline-PII event must carry an envelope-encrypted `pii_key_ref` (the `no-untagged-personal-data`
  intersection). **The single highest drift-risk contract — frozen and continuously verified.**
- **`list_objects` `SetExpr` push-down (4.3)** — Search + Refs + Notif + every permission-aware read. The
  dedicated seam is §3.5 below (it is "the single most load-bearing inter-system contract").

### 3.3 — through 3.7: the load-bearing reconciled seams (each frozen, each its own suite)

#### 3.4 The CheckStatus merge gate (X-1 / contract 5.9) — CI produces, Git gates

The hardest seam. CDC verifies **both halves** against one shared fixture (`contracts/5.9/`):

- **Keying + supersession (CI→Git).** `CheckStatus` keyed `(commit_oid, context)`; Git's projection holds
  **exactly one current row per key**; an incoming status supersedes iff `run_attempt >= stored` — **monotonic
  on `run_attempt`, NOT wall-clock `completed_at`** (clocks are not authority). Gate **SEAM-CHK-1** (= git
  D-10a / CI D-8): deliver `ci.check.updated` **out of order + duplicated** ⇒ exactly one current row, a
  **lower `run_attempt` arriving late is dropped**, idempotent on `event_id`. **0 stale-row wins.**
- **Fork / trust-tier gating (the security-critical half).** A `trust_tier = untrusted_fork` success is
  recorded but **`neutral` for gating** — it **cannot satisfy a `required` context by itself** until a
  trusted principal endorses via `check(subject, approve_untrusted_ci, repo)` OR the context is re-run
  `trusted`. CI stamps `trust_tier` from provenance + the ReBAC ABAC edge; **Git does not recompute trust, it
  reads the fact**. Gate **SEAM-CHK-2** (= git D-10b/c): a fork PR **cannot self-green** (the
  poisoned-pipeline-execution attack, EI-02 §1); endorsement flips the gate; **0 fork self-green**.
- **The merge-queue durable signal.** The queue is a durable workflow per target ref; it
  `wait_for_signal("ci.result", idem_key=<merge_attempt_id>)` (holds no runtime), wakes on the **rollup**
  `ci.result` (distinct from the per-context events — events drive the PR-checks UI, the one signal drives
  the resume). `DurableExecutor::signal` is **idempotent on `idem_key`**. Gate **SEAM-CHK-3** (= git D-10d):
  a **doubly-delivered `ci.result` wakes the workflow exactly once** ⇒ **merge count == 1, 0 double-merge, 0
  spurious unblocks**. **Acyclicity property (EI-02 §3):** CI emits, Git reads its own projection — **Git
  never synchronously calls CI** (a `no-cross-sync-cycle` lint instance). Green artifact: `seam-checkstatus.json`.

#### 3.5 The `list_objects` Filter pre-filter (OQ-E / contract 4.3) — no leak, no N+1

The single most load-bearing inter-system contract; one shared suite (`contracts/4.3/`) run by Id (provider)
+ Search + Refs + Issues + KN + every permission-aware read (consumers):

- **No leak.** Every `SetExpr` variant lowers to a SQL predicate/JOIN over the **consumer's own id column**
  via the per-tenant **authz reverse index**; the result conjoins **before** scoring/ranking. Gate
  **SEAM-LO-1** (= KD-5, D3, F1): a confidential/overridden/row-restricted/field-hidden artifact is **absent
  from every view/backlink/search/embed/RAG result incl. an aggregate `COUNT`** (the conjoin is *inside* the
  query) for an unauthorised viewer, **incl. under zookie staleness** (the revision watermark). **0 leaked
  artifacts, 0 count-leak.**
- **No N+1, no post-filter.** Gate **SEAM-LO-2**: the query plan shows **0 rows removed by a post-filter**
  and **1 authz JOIN, not one query per id** (asserted by reading the plan's rows-removed + the per-request
  query-count signal = bounded). The `search-requires-acl-filter` lint makes a non-conjoined Search path a
  **build failure**.
- **Composable over an arbitrary id column** (the C-B confirm): the same `Filter` composes into Search's
  posting-list predicate over `doc_id`, Refs' `WHERE source IN (…)` over the edge source column, and Issues'
  board JOIN — **one contract, many id columns**. Gate **SEAM-LO-3**: the CDC double proves all consumer id
  columns. Green artifact: `seam-listobjects.json`.

#### 3.6 The `#sub` tombstone resolution (X-4 / contract 5.7) — one ladder, never leak, never dangle

One shared suite (`contracts/5.7/`): Refs owns the grammar + the 4-step ladder; each subsystem mints **stable
opaque sub-ids** + implements its `project` sub-anchor resolver:

- **Grammar.** The frozen `#sub` kind vocabulary (`comment-`/`thread-`/`message-`/`b`/`h`/`row-`/`field-`/
  `L<a>-L<b>`/`check-`/`step-`); `<opaqueid>` is **stable across edits/moves** (each subsystem's obligation);
  Refs stores the **full sub-URN AND the stripped root** and **rejects ambiguity** (REF-3). Gate **SEAM-SUB-1**:
  a fuzz corpus of sub-URNs ⇒ **100% grammar-valid accepted, 100% ambiguous rejected, 0 guessed scope**.
- **The one ladder (frozen).** `resolve(ref, viewer, mode)`: **(1)** permission `check(viewer, read, root)`
  → Deny ⇒ `Tombstone{denied}` (**never leak**); **(2)** root resolve → No ⇒ `Tombstone{root_gone}`; **(3)**
  sub-resolve via the owner's `project` sub-anchor → `LIVE | MOVED | OUTDATED(partial) | GONE→Tombstone{sub_gone,
  root}`; **(4)** `ERASED` (any level) ⇒ `Tombstone{erased}`. **A tombstone always carries the root** so an
  embed degrades to "this referenced <parent> (the part is gone)", **never a 404, never a dangle**. Gate
  **SEAM-SUB-2** (= REF-D9): delete an embedded doc block / PR comment ⇒ the embed degrades to a
  partial/relocated projection, **0 dangling embed, 0 leak**.
- **Git content-anchored line ranges** (the new specificity): `#L42-L88` carries a **BLAKE3 fingerprint +
  context window + mint-time blob oid**; resolve against a newer blob ⇒ **exact | rebased(3-way context
  match, flag `moved`) | partial(surviving sub-range, flag `outdated`) | tombstone(content gone)**. Gate
  **SEAM-SUB-3**: edit/rebase the file ⇒ the range resolves to the correct one of the four states. Green
  artifact: `seam-sub-tombstone.json`.

#### 3.3/3.5b The `myelin-content` round-trip (X-2 / contract 13.1) — `render(parse(md)) === md`

One shared suite (`contracts/13.1/`): Knowledge leads, Chat + Issues consume **strict subsets**; the parser
is a **WASM compile target** so read and edit run the **same** path (KN-4):

- **The hard gate (design §8b.2, T-5).** `render(parse(md)) === md` over a **markdown-subset corpus** incl.
  the three structured nodes (`mention`/`artifact_ref`/`embed`) **`U+FFFC`-anchored × nesting in
  bold/lists/tables, code blocks, IME/paste edge cases**. Gate **SEAM-MDC-1** (= KD-2, D10): **100%
  round-trip, 0 corpus regressions** — read mode and edit mode use the **identical WASM parser** (a property:
  the two code paths are the same symbol). Green artifact: `seam-content-roundtrip.json`.
- **Subset conformance.** Chat & Issues consume **strict subsets**; a block outside the subset in a Chat/Issue
  body **fails the subset lint**. Gate **SEAM-MDC-2**: the three structured ref nodes produce
  `refs.edge.created` **uniformly** across all three subsystems (the CDC ties to §3.1 `project`/edge emit).
- **ADF lossy-map (13.2).** The Issues import conversion table is frozen; lossy nodes are **named + recorded
  in the import report**, never silently dropped. Gate **SEAM-MDC-3**: an import corpus ⇒ every lossy node
  appears in the report (name-your-floors, not silent loss).

#### 3.7 ToolDef registrations + the frozen `requires_approval` defaults (X-6 / contract 8.1)

One shared suite (`contracts/8.1/`): every subsystem registers; the Agent `ToolSurface` resolves. CDC asserts
the **frozen defaults table** — CI deploy/secret = **yes**; Git merge = **yes**, open_pr = **no**; Issues
forecast/triage = **no**, SLA transition = **caveat-gated**; KN publish/confidential = **yes**; Chat post =
**no**; a **cross-subsystem effect inherits the TARGET subsystem's default** ("governed where it lands").
Gate **SEAM-TOOL-1**: a ToolDef whose `requires_approval` diverges from the frozen default **fails CDC**; a
cross-subsystem effect's gate resolves to the target's default. Green artifact: `seam-tooldefs.json`.

---

## 4. AGENT-NATIVE testing — mock-brain determinism, golden + mutants, skeleton, no-host-exec, plan-then-apply

> The strategy seam (AG-1/AG-3/AG-4) is the whole point: **if the substrate is right, an agent needs almost
> no special code** — an agent is a `Principal{kind=agent}` through the same identity/gateway/event-log/
> sandbox/cost-gate. Testing the agent in parts means testing **the loop, deterministically, with a scripted
> brain**, then proving the brain swap (mock→real) cannot change the *governance* path.

### 4.1 The mock brain replays a scripted step queue → DETERMINISTIC (the foundation)

- **Mechanism.** `MockAgentRuntime` (`--use-mock` — a **real runtime flag on the same code path users hit**,
  AG-4, not a test-only stub) replays a **scripted queue of `StepOutcome`s** (`UseTools(calls) | Submit`).
  The platform owns conversation history; the brain is the only varying input. Gate **AGENT-G1** (= drill
  AG-D9 / D-9): run the same scripted queue **twice** ⇒ **byte-identical proposed-effect sequences** (the
  `proposed_effect` audit rows match exactly). Determinism is asserted from the **emitted trace + outbox**,
  not by re-reading memory. Green artifact: `mock-determinism.json`.
- **Replay-tree property.** A recorded `correlation_id` event tree replays to a **deterministic, idempotent,
  causality-preserving** re-drive (replay == original, exactly once) — the event log makes this first-class
  (E-6: replay before you fix). Gate **AGENT-G2** (= BUS-D3).

### 4.2 Golden tests + `cargo-mutants` over the agent loop

- **Golden.** A corpus of `(scripted-brain, inbox-event) → expected proposed-effect-plan` golden files; a
  change to the loop that alters a plan **fails the golden** (and must be a deliberate, reviewed re-baseline,
  never a silent overwrite). Gate **AGENT-G3**: **0 unreviewed golden drift**.
- **Mutation.** `cargo-mutants` over the **event→trigger→effect→event loop** (the load-bearing governance
  code: routing per `effect_kind`, the plan-then-apply pipeline, the loop guards). Gate **AGENT-G4** (= D-9):
  **mutation score ≥ threshold** (the Phase-5-measured floor; the repo `.gitignore` pre-seeds `cargo-mutants`
  — the expected quality bar). A surviving mutant in the governance path is a **missing test**, fixed before
  merge. Green artifact: `agent-mutants.json`. **Wired into CI (E-4) — an uncommitted mutation run is no gate.**

### 4.3 Skeleton mode proves the gateway/identity path with ZERO spend

- `SkeletonAgentRuntime` (no model, no tools, zero spend, zero effects, AG-3): **authenticate → fetch task →
  print summary → exit**, exercising the **whole gateway/identity/dispatch/reserve/trace path** at ~zero
  cost. Gate **AGENT-G5**: the skeleton run **mints a per-run token (life == run life), opens + settles a
  zero-cost reservation, writes a trace, scrubs the shared platform token from the child env** ⇒ proves the
  substrate path **before any model or tool exists**. **0 spend, 0 effect, 0 token-leak.** This is the
  build-order floor (skeleton → mock → real) made testable; green artifact: `skeleton-path.json`.

### 4.4 The hands trait has NO host-exec bypass

- `ToolHands::exec(Command) → ToolResult` is the **one** computation path; **no host-execution path bypasses
  it** (AG-2). The **`no-host-exec` lint** (sibling to `no-cross-db`) makes any direct host-exec a **build
  failure** — with a **red-fixture** (code that calls the host directly, which MUST be rejected) so the lint
  is *proven*, not claimed. The simulation/test impl emits a **channel-proof marker** (so a test can assert
  the call went through the trait, not around it). Gate **AGENT-G6** (= drill AG-D1): an adversarial corpus
  attempting a host/DB write outside `EffectApi`/`exec` ⇒ **structurally impossible** (`no-host-exec` +
  `no-cross-db` lints green on the red-fixtures), **0 direct mutation**. The sandbox escape drill (T-1, §2.2)
  gates `exec` itself on a **real kernel** — **the single hard go/no-go before any agent runs untrusted code**.

### 4.5 plan-then-apply validated against `permissions ∩ delegation ∩ tenant`

- The `EffectApi::apply` pipeline is **in order, fail-closed**: SCHEMA → CAPABILITY (`check` with the
  `CaveatContext{object, field?, transition?, attrs}` for field/transition ABAC, evaluated **off the hot
  `list_objects` path**) → DELEGATION (`agent.policy ∩ delegation ∩ tenant.policy`, **monotone — attenuation,
  never up**) → TENANT → BUDGET → HITL-GATE → APPLY (the **public endpoint as the agent principal — same
  gateway, no carve-out**) → METER. Gates:
  - **AGENT-G7** (= AG-D2/AG-D3, the intersection property): an effect **outside the intersection** ⇒
    `Denied` returns to the loop as an **ordinary tool error — no privileged fallback fires**; an agent can do
    **nothing no human role can** (union is forbidden; only the intersection applies). Tested both ways: a
    policy-allowed-but-delegation-forbidden effect AND a delegation-allowed-but-tenant-forbidden effect are
    **both confined to the intersection**; a delegator who **lost** the right ⇒ the agent loses it too.
  - **AGENT-G8** (= AG-D5/AG-8, HITL withhold): a gated tool whose name is not "approved" is **WITHHELD**
    (returns `Gated`, **does NOT mutate**); the approval card shows the pending action + risk + **live cost
    estimate**; approval **re-runs the step and applies once**; rejection halts; **per-effect `idem_key`** ⇒
    a double-click is one approval, a 2-of-3 partial approval is well-defined. **0 mutation before approval,
    0 double-apply.**
  - **AGENT-G9** (= AG-D11/AG-D6, the cost gate): a runaway loop vs an exhausted wallet ⇒ **reserve refuses
    to start** a new run, **never interrupts in-flight**; the loop **stops at the wallet**.
  - **AGENT-G10** (`--dry-run`, contract 8.7): `run --dry-run(InboxEvent) → Vec<ProposedEffect>` stops after
    the gate step and shows the plan **without applying** — plan-then-apply **testability** is a first-class
    surface (CLI + tests), **identical for mock and real**.
- **The loop guards** (AG-6): self-guard (skip the agent's own output) + a **reference gate** (only a
  **structured picker-produced** `artifact_ref` re-triggers, **never raw typed text**) + causal-depth ceiling
  (**12**) + shared-causal-root-within-a-window tripwire + bounded dispatch pool that **drops over-cap (never
  forks)**. Gate **AGENT-G11** (= AG-D7, F9, adversarial): an agent→agent self-trigger ⇒ **halts ≤ depth 12**,
  the tripwire trips the **per-tenant breaker**, the bounded pool drops over-cap. **The causal-loop tripwire
  is an adversarial drill** (T-5) — the agent **cannot typo into a loop** (causality is platform metadata,
  not convention). Green artifact: `agent-governance.json`.

---

## 5. The in-parts scorecard + how a gate becomes "proven"

Every gate above is **one committed row** in `testing-manifest.toml` and **one column** in the in-parts
scorecard. The status of a property (T-4, EI-04 §4):

- **PROVEN** — the gate's CI job is green AND the green artifact exists AND the assertion was read from the
  named telemetry signal (contract 1.8). Only then.
- **CLAIMED** — the code exists but no green artifact yet (a skeleton/spike; name the missing half).
- **FLOOR** — a partial answer shipped with a **named, linked follow-on** (CAS→CRDT KD-3; the free-text
  erasure residual GDPR-G6/10.9; node-backed→object-backed git; single-region→multi-cell CP-G4/GA-D8). A
  floor's drill is owed **when its follow-on is built**; named here so the gap is **visible, never invisible**
  (the failure is the gap masquerading as done — VISION §3, EI-04 §4).
- **`[OPEN — LEGAL]`** — a DPO/counsel ratification gates the green (L-1 fail-static bound W; L-2 git-history
  erasure residual; L-3 implicit auto-dispatch; L-4 EU AI Act). **Never invert the assertion or weaken the
  threshold to make it green** (T-2): a red/blocked gate is **information**, recorded as "needs human
  verification," not softened.

**The non-negotiability order this doc enforces (R-1/R-2, doctrine §2).** Within the in-parts suite, two
gates are the **stop-the-bleeding** floor that every later gate waits behind: **STOR-G4** (silent data loss —
the headline durability/restore-verify gate) and **CI T-1 + AGENT-G6** (RCE / sandbox-escape — the single
hard go/no-go before any untrusted CI step or agent `exec` runs). No subsystem gate is "done" while either is
red (the gate invariant). Everything else — keystone seams (§3.4/§3.5/§3.6/§3.5b), then breadth (§1/§2), then
the frontend switch-test + measured contrast/latency (design §8b, owned in the whole-system doc) — sequences
behind them.

---

## 6. Cross-references

- [`01-whole-system-strategy.md`](./01-whole-system-strategy.md) — the AS-A-WHOLE companion (the F1–F9 drill
  families run cross-system, the failure-injection harness 1×/10×/30×, the chained-mutation real-UI E2E
  suite, the frontend switch-test, the source-verified scorecard).
- [`../contract-index.md`](../contract-index.md) — the frozen build-to surface every §3 CDC suite verifies.
- [`../00-reconciliation-decisions.md`](../00-reconciliation-decisions.md) — the rationale for the load-bearing
  reconciled seams (X-1 CheckStatus, X-2 content, X-4 `#sub`, OQ-E `list_objects`, OQ-F `idem_key`, X-6 tools).
- [`../../03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
  — Part A (the 101-drill inventory, 9 families) + Part B (open questions, incl. Q32 drill thresholds → P5).
- Each subsystem's `architecture/07-drills-and-open-questions.md` (git/CI/issues/knowledge/chat) — the
  per-subsystem obligation registers consolidated in §2.
- [`../../02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
  — T-1..T-9 (Phase-5 testing), E-4/E-5 (the ratchet), L-1..L-4 (legal ratifications).
- [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
  — THE testing philosophy (prove-it / quantified gates / observability-as-pass-condition / the ratchet /
  name-your-floors / order-by-non-negotiability). [`04-hard-problems.md`](../../../external-insights/04-hard-problems.md)
  — the named-honestly hard problems each floor gate tracks.
- Spine: ADR-16 (backpressure + human lane), ADR-17 (fail-static), ADR-18 (restore-verify gate), ADR-20 (one
  sandbox); [`design-language.md` §8b](../../02-holistic-architecture/design-language.md) (render(parse(md)),
  measured contrast, latency budgets).
