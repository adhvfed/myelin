# Phase 5-C — Whole-System: Cross-Subsystem E2E + the Failure-Injection Drill Catalogue

> Phase: `05-refined-shared-systems-architecture/testing-strategy`. Canonical brief:
> [`VISION.md`](../../../VISION.md) (never contradicted). Philosophy source (this doc IS the testing
> philosophy made concrete): [`external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md)
> §3 (prove-it-or-it-isnt-real), §4 (drive-the-real-UI + chained-mutation E2E), §5 (the ratchet / committed
> gates), §2 (order-by-non-negotiability). Hard problems:
> [`external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md).
> Binding directives: [`integration-directives.md`](../../02b-doctrine-integration/integration-directives.md)
> Phase-5 T-1..T-9. Spine: [`architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md)
> (ADR-16 backpressure+human-lane, ADR-17 fail-static, ADR-18 backup/restore-verify, ADR-20 one sandbox).
> Frozen contracts under test: [`contract-index.md`](../contract-index.md) +
> [`00-reconciliation-decisions.md`](../00-reconciliation-decisions.md). Design-QA gates:
> [`design-language.md`](../../02-holistic-architecture/design-language.md) §8b. Drill inventories consolidated:
> [`03 drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md)
> Part A (101 drills, 9 families) + the five subsystems' `architecture/07-drills-and-open-questions.md`
> (Git D-1..D-11, CI T-1/D-1..D-11, Issues D1..D14, Knowledge KD-1..KD-13, Chat D-C1..D-C19). Date: 2026-06-19.
>
> **What this is.** The strategy for testing Myelin **as a whole**. Three parts: **(§2)** the cross-subsystem
> **chained-mutation E2E scenarios** that prove the wedge, driven against a full cell; **(§3)** the
> **failure-injection / drill harness** (doc-01 §3) — a 1×/10×/30× load generator with mixed principal types, a
> scoped reversible dependency-break, assertions read from production telemetry, cheap-in-CI vs scheduled, and
> the every-incident-adds-a-drill loop; **(§4)** the **consolidated quantified drill catalogue** — one master
> table merging Phase-3 Part A + the five subsystems' `07` drills, each with id, owner, the **quantified
> threshold**, the **green artifact** it emits, and **CI-vs-scheduled** frequency.
>
> **The five non-negotiable invariants of every gate in this doc** (T-1..T-4):
> 1. **Quantified.** Every gate resolves to a measured number (RPO/RTO seconds, zero-escape, zero-loss,
>    N-min revocation, 30×-surge-holds, recall@k, p99 budget). A target you cannot measure is not a gate.
> 2. **Forced.** The property is not real until the drill **forces the failure** and the system survives it.
> 3. **Observed.** Observability is part of the pass condition — a drill that survives but emits no survival
>    signal (contract 1.8) has **failed**. The assertion is read from production telemetry, not test scaffolding.
> 4. **Committed.** An uncommitted gate is no gate. Every CI-tier drill is wired into the pipeline; every
>    scheduled drill is a cron job with a paging owner. `... || true` and silent filters are banned (T-2).
> 5. **Green-artifact-or-claimed.** A capability is **proven** only when its drill emits a dated green
>    artifact; until then it is **claimed** (T-4). The scorecard is source-verified, never doc-verified.

---

## 1. Test pyramid, sourced from the doctrine (so the whole-system layer is not asked to do the parts' job)

The whole-system strategy sits on top of a per-part base. Naming the full pyramid keeps each layer honest
about what it does and does not prove (EI-01 §4 — "automated tests prove the parts; they routinely miss what
only appears when a real user drives the whole thing").

| Tier | What it proves | Where it lives | Owner | This doc's concern |
|---|---|---|---|---|
| **L0 — Unit + property + mutation** | A function/type does what it says; `cargo-mutants` ≥ the mutation threshold over the event→trigger→effect→event core (AG-D9). | every crate | each system | out of scope (named for completeness) |
| **L1 — Architecture lints (the ratchet)** | A class of bug is **structurally impossible** at compile time (12 lints, contract 1.6): `tenant-predicate`, `no-cross-db`, `no-raw-publish`, `no-host-exec`, `forward-only-migration`, `no-cross-sync-cycle`, `residency-pin`, `control-plane-pii-free`, `search-requires-acl-filter`, `no-llm-in-platform`, `no-untagged-personal-data`, `flow-determinism`. | CI, every crate | substrate/CI | the **floor under** every drill; a lint that goes red blocks merge (R-2 gate invariant) |
| **L2 — Per-system integration drills** | One system survives one forced failure, asserted against its own telemetry. | each system's test suite + CI | each system | enumerated in §4 (the master catalogue) |
| **L3 — Cross-subsystem chained-mutation E2E** | The **wedge** works end-to-end: real sessions chain mutations mid-flight across ≥3 subsystems against a full cell, with mock agents + plan-then-apply + HITL. | the cross-system harness | platform/testing | **§2 — the heart of this doc** |
| **L4 — Whole-cell failure-injection drills** | A whole-cell property (RPO/RTO, 30× surge, restore-verify, escape) survives a forced fault under load. | the drill harness (§3) | platform/testing + owner | **§3 + §4** |
| **L5 — Frontend done-bar** | The switch test, reached by **driving the real UI in a browser**; measured-contrast + latency budgets (T-7/T-8, design-language §8b). | per-subsystem UI E2E | each subsystem | §2.6 (folded into the E2E scenarios) |

**The gate invariant (R-2, EI-01 §2).** No later tier is "done" while an earlier tier is red. A red L1 lint
blocks the L3 E2E from being claimed; a red L4 escape drill (AG-D4) blocks **all** untrusted-code execution
(the single hard go/no-go, §3.5). Ordering is enforced, not aspirational.

---

## 2. Cross-subsystem chained-mutation E2E scenarios (L3 — the wedge)

These are the scenarios that prove Myelin's differentiator: **one identity model, one event bus, one agent
fabric, one reference graph** make work flow across subsystems without friction, with agents as first-class
citizens. Each is a **chained-mutation E2E** (EI-01 §4 / T-6): a real session chains mutations and updates
state **mid-flight** — exactly where the bugs live — not an isolated single-handler call against a fresh DB.
Each runs **against a full cell** (all shared systems + all five subsystems booted via `serve(AppSpec)`, real
Postgres/blob/bus/search, the gateway in front), with **mock agents** (`--use-mock`, contract 8.3) so the
agent path is exercised deterministically (AG-D9: a scripted mock run twice → identical proposed-effect
sequences).

**Common harness obligations for every scenario below:**
- **Real sessions, chained.** Drive through the public gateway with real tokens (human + agent + service
  principals), chaining mutation N on the state produced by mutation N−1, asserting intermediate state
  between steps (not just final state).
- **Mock agents + plan-then-apply + HITL.** Every agent step goes `AgentRuntime::step` → `run --dry-run` →
  `EffectApi::apply` → `{Applied | Gated | Denied}`. A `Gated` effect **withholds** (does not mutate) until
  an `approval` signal arrives; the scenario asserts the withheld tool **never mutated** before approval
  (AG-D5/AG-8) and that a double-click is **one** approval (per-effect `idem_key`, OQ-F).
- **Telemetry assertions.** Each scenario asserts against the survival-signal set (contract 1.8) — RED/USE per
  principal-kind, consumer-lag, outbox-depth, causal-depth, reserve/settle ledger — not just HTTP 200s.
- **Green artifact.** A dated JUnit-class E2E report + the per-step telemetry snapshot + (for UI steps) a
  browser trace/screenshot set. Stored as the scorecard artifact for that scenario (T-4).

### E2E-1 — The PR context pane (UC-X-3) — the cross-artifact reference-graph proof

**Wedge proven:** one reference graph + one permission model means a PR pane unfurls *every* connected artifact
(issue, doc, CI checks, chat thread) per-viewer, leak-free, live.

**Chain (each step mutates; the pane re-resolves mid-flight):**
1. Open a PR whose description contains `Closes ENG-1421`, an `embed` of a Knowledge design doc, and a
   `@mention`. The three inline ref nodes (contract 13.1) emit `refs.edge.created` (5.4).
2. Assert the PR context pane resolves: the issue projection (`project(ref, viewer)`, 5.6), the doc embed, the
   `CheckStatus` rows per `(commit_oid, context)` (5.9), and backlinks (`traverse`, depth-16, 5.3) — **all
   per-viewer**.
3. **Mid-flight mutation A:** CI emits `ci.check.updated` (build → success, test → failure) via outbox. Assert
   the pane's checks panel live-updates (firehose, the shared per-ref cache busts, Chat D-C7 analog) and the
   merge gate shows blocked.
4. **Mid-flight mutation B:** a second viewer **without** access to the linked confidential issue opens the
   same PR. Assert the issue unfurls to a **tombstone** ("a restricted issue"), title never present (the
   4-step ladder step 1, 5.7) — **zero leak, incl. count/backlink leak** (REF-D1, SRCH-D1, ID-D4).
5. **Mid-flight mutation C:** the linked issue is transitioned to Done; assert the pane backlink projection
   updates and the typed edge (`closes`, TE-7) reconverges.

**Gate:** every connected artifact resolves correctly per-viewer; zero leak to the unauthorized viewer; the
live check-update lands within the freshness budget; the tombstone carries the root. **Crosses:** Git, CI,
Issues, Knowledge, Refs, Search, Identity, Notif (humanise). **Green artifact:** the pane-resolution trace +
zero-leak counter at 0 + per-viewer projection diff.

### E2E-2 — CI-fail → triage agent → issue → chat → fix-PR (the agent-native flagship)

**Wedge proven:** agents are first-class — a failing CI run wakes a (mock) triage agent that plans, gets HITL
approval, files an issue, discusses in chat, and opens a fix-PR, all metered through one wallet and one
plan-then-apply gate. This is the flagship; it exercises the full agent loop + durable workflow + HITL across
five subsystems.

**Chain:**
1. A push triggers a CI pipeline (a durable workflow, `myelin-flow`); a test step fails. CI emits
   `ci.check.updated` (state=failure) + the rollup `ci.result` signal.
2. A **Signal rule** matches the failure; the dispatch tier (3.6) delivers an `InboxEvent` to a **mock triage
   agent** (explicit-first dispatch, CHAT-1 — a Signal-driven automation, not a casual mention). Reserve at
   dispatch (11.7): **no wallet balance → no run** (asserted with an exhausted-wallet variant → refuse-start).
3. The agent loops: `step` → `run --dry-run` → proposes effects `[create_issue, post_chat_message,
   open_pr]`. Assert the proposed-effect sequence is **deterministic across two runs** (AG-D9).
4. `create_issue` applies (no approval needed, Issues `triage` default = no, 8.1); the issue is filed with the
   failing-run `details_ref` (`#step-<n>`, jump-to-failure).
5. **HITL gate:** `open_pr`/`git.merge` path — the fix-PR open is `no`, but a `git.merge` proposal is
   **`requires_approval=yes`** (8.1). Assert the merge tool is **withheld** (returns `Denied`, does NOT mutate
   — AG-8/AG-D5); a Notif HITL card (`reason=approval_requested`) is posted showing action + risk + cost.
6. **Mid-flight:** kill the Agent + Workflow services mid-`ack_window`. Approve **days later** (double-click).
   Assert the durable workflow resumes (FLOW-D4), consumes the approval **exactly once**, re-mints the run
   token on resume (4.7), and the merge applies **once** (FLOW-D1, no double-effect).
7. The fix-PR's CI goes green; the merge-queue workflow wakes on `ci.result` (idempotent on `idem_token`,
   X-1/D-10) and merges; `git.pr.merged` closes the issue via the `Closes` trailer.

**Gate:** the agent never mutates outside `agent.policy ∩ delegation ∩ tenant.policy` (AG-D2/AG-D3); zero
mutation before approval; exactly-once approval + merge across the kill; reserve/settle balanced (one cost
event per metered unit, never interrupts in-flight). **Crosses:** CI, Agent Fabric, Workflow, Issues, Chat,
Git, Identity, Notif, Storage (wallet). **Green artifact:** the run trace (deterministic), the HITL
withhold→approve→apply ledger, the reserve/settle parity report, merge-count == 1.

### E2E-3 — Spec-to-ship traceability — the reference-graph + audit proof

**Wedge proven:** an artifact's full causal lineage (doc spec → issue → PR → commits → CI runs → deploy →
chat decision) is reconstructable per-viewer from the reference graph + the tamper-evident audit log, and
survives a reindex-from-cold.

**Chain:**
1. Author a Knowledge spec doc; create an `initiative` (new type token, 2.9) referencing it; break it into
   child issues (typed `parent` edges, TE-7).
2. Open PRs that `Closes` each issue; land commits with `Co-authored-by` trailers (→ `refs.edge.created`);
   CI runs attach `CheckStatus`; a protected-env deploy (HITL-gated, CI deploy default = yes) ships it; a chat
   thread records the go/no-go decision and references the deploy.
3. Assert `traverse(spec_doc, viewer)` walks the **entire lineage** (depth-bounded 16, cycle-safe), each hop
   permission-checked per-viewer; the `explain` trace is coherent.
4. **Mid-flight mutation:** wipe the Refs edge index + the Search index; `reindex(scope)` via the live
   consumer path (`*.snapshot` replay, 2.6). Assert the rebuilt lineage **byte-matches** the live lineage
   (F4 / REF-D4 / SRCH-D5) — **no bespoke recovery reader**.
5. Assert the audit log for the deploy is tamper-evident: retroactively edit one entry → the hash-chain breaks
   + the consistency proof against the published STH fails (GA-D3).

**Gate:** complete lineage per-viewer; cold-reindex == live; audit tamper detected 100%. **Crosses:**
Knowledge, Issues, Git, CI, Chat, Refs, Search, GDPR/Audit, Identity. **Green artifact:** the lineage diff
(live vs cold) at zero drift + the tamper-detection proof.

### E2E-4 — A DSAR fan-out (the GDPR-by-construction proof)

**Wedge proven:** a single `dsr_submit` reaches **every** holder across all subsystems (the data-map-driven
fan-out), erases reliably (crypto-shred + pseudonym-shred + restrict), survives a backup-restore, and emits a
Merkle-proven certificate — GDPR-by-construction, not bolted-on.

**Chain:**
1. Seed one subject's PII into **all** H1–H18 holders: a git commit author + PR comments, CI log lines,
   issue free-text + change-log, Knowledge blocks + agent-trace, chat message bodies + drafts, notif inbox
   items, search docs + **embeddings**, refs edges, workflow history, OLAP read store.
2. `dsr_submit(subject)`. Assert the `data_map()`-driven fan-out (10.3/10.4) iterates every holder
   (GA-D1: **0 holders missed**); a 1-month durable timer is armed (GA-D4).
3. **Erase.** Assert: structured PII pseudonymised (`<pseudonym>@<tenant>.noreply`), self-authored free-text
   crypto-shredded (per-subject DEK destroyed → unrecoverable in DBs **and backups**, STOR-D4/D-2/D-3/KD-4),
   embeddings **purged not hidden** (GA-D2/SRCH-D4: 0 embedding re-identification), git author pseudonymous,
   inbox items humanise to `[erased user]` (NOTIF-D6), unfurls degrade to tombstones (D-C6/REF-D5).
4. **Mid-flight:** restore an **older** backup (pre-erasure). Assert post-restore re-erasure runs from the
   erasure ledger (GD-14, 10.8) → the subject is **still erased** (STOR-D3/ID-D8: 0 resurrected subjects).
5. Assert the **residual** is exactly the platform-posture residual (contract 10.9): third-party free-text
   PII authored by others is `restrict`-suppressed (never indexed/agent-read/in-analytics, GA-D7) and the
   `[OPEN — LEGAL]` documented limit — **nothing more**.
6. `dsr_certificate` seals a `MerkleProvenBundle` on completion (GA-D4: 0 silent misses).

**Gate:** 0 holders missed; 0 recoverable PII (incl. vectors, incl. backups); residual == the one documented
posture; certificate sealed. **Crosses:** GDPR/Audit, Storage, Identity, all five subsystems, Search, Refs,
Notif, Workflow, Bus. **Green artifact:** the holder-coverage receipt set (all H1–H18) + a post-erase
`locate` returning 0 recoverable PII + the Merkle certificate.

### 2.5 The agent-native cross-cutting assertions (apply to E2E-1..4)

Because agents are first-class, every E2E that includes an agent step also asserts: the agent's effects are
**attributed** (per-run attenuated token, nested causality, BUS-5); a consequential effect is **HITL-gated**
by the frozen `requires_approval` default; `is_agent`/`agent_run` is rendered **distinctly** (AI-Act
labelling, ADR-08); and the run is **metered** into the same wallet as a human/CI action (reserve/settle, one
path). These are the AG-D1/D2/D3/D5 family assertions, exercised in-context rather than only in isolation.

### 2.6 The frontend done-bar, folded in (L5 / T-7, T-8)

Each E2E with a user-facing surface ends by **driving the real UI in a browser** (not reading the feature
list): the **switch test** — could a Jira/Linear/Notion/GitHub user complete the loop without hitting a wall
the old tool didn't have? (Issues D14, Chat D-C19, Git OQ-12.) Plus the measured frontend gates (design-language
§8b): **measured-contrast** over the token table (never a stated ratio — WCAG 2.2 AA, AAA where feasible);
**hard latency budgets** (keyboard < ~100ms, no spinner-flash < ~1s, pages render not animate-in);
**`render(parse(md)) === md`** round-trip over a corpus (the one editor path, KD-2/D10/D-C-editor); and
**overlays/popovers tested against the real anchor** (the bottom-pinned composer, the off-screen-picker bugs).
Empty/loading/error/permission/erased/agent-pending states are each driven.

---

## 3. The failure-injection / drill harness (doc-01 §3)

The harness is the machine that makes a drill **forced + observed**. It is a **platform-capability keystone**
built early (R-3) — before the features that depend on it being drillable. Four components.

### 3.1 The load generator (1× / 10× / 30×, mixed principal types)

A traffic multiplier that replays a representative workload at **1× (baseline), 10× (stress), 30× (surge)**
and **mixes principal types** — human, agent, service, CI runner, external-MCP — in configurable ratios. The
30× agent-skewed mix is the input to every F6 surge drill (ADR-16): it proves the **protected human lane
holds** while the **agent lane sheds** (`429 + Retry-After`, honoured by the resilient client) and **other
tenants are unaffected** (per-tenant bulkhead, EI-02 §5). The generator tags every request with its
principal-kind + tenant so the assertion can read RED/USE **per principal-kind per tenant** off the metrics
port (contract 1.8). Per-surface storm profiles (OQ-K): CI-surge, collab op-stream, connection-storm,
agent-mention-storm — each asserts against its named v1 shed-budget floor.

### 3.2 The scoped, reversible dependency-break

A controlled fault injector that **breaks one dependency in a scoped, reversible way** — kill a service
between commit and publish, sever the broker mid-stream, fail-over a DB replica mid-merge, hard-down Identity,
hard-down KMS, drop a firehose connection, trip a downstream breaker, corrupt one blob object. "Scoped" =
blast-radius-limited to the drill's tenant/cell; "reversible" = the break is lifted and the system observed
recovering. This is the engine behind the F3/F5/F7 families and every "kill X mid-flight" drill in §4.

### 3.3 Assertions read from production telemetry

A drill **does not pass by not failing** — it passes by **asserting against the telemetry** that the system
survived (T-1). Every assertion reads the survival-signal set (contract 1.8): RED/USE per principal-kind +
tenant, consumer-lag, outbox-depth/dead-letter, breaker-state + Retry-After issuance, fail-static
fresh/stale/closed ratios, shed-counts per lane, causal-depth histogram + tripwire firings, reindex-parity
hash, erase-receipts, residency-attestation, misroute-count, and the system-specific signals (timer-wheel lag
+ replay-rate + nondeterministic-halt for Workflow; important-buried-rate + dedup-collapse-ratio +
delivery-success for Notif; erasure-fanout-coverage + audit-append-lag + STH-publish-age for GDPR/Audit;
backup-RPO-seconds + restore-verify-pass + crypto-shred-lag for Storage). **A system that survives a drill but
emits no signal that it survived has failed the drill** (X-1 directive: a Phase-3 doc that omits its signals
fails; a drill that doesn't read them isn't a proof).

### 3.4 Cheap-in-CI vs scheduled; and the every-incident-adds-a-drill loop

- **CI tier (cheap, every change):** the lints (L1); the deterministic correctness drills (idempotency,
  outbox emit-iff-committed, dedup, supersession, `#sub` ladder, editor round-trip, the `SetExpr` leak-free
  JOIN at moderate scale, reconnect-loses-zero-ops at small scale, the fail-static degrade, single-tenant IDOR).
  These gate **every merge** (R-2). Their failure is loud, never `|| true`.
- **Scheduled tier (expensive, nightly/weekly):** the prod-scale surge drills (1M+ timers, 100k-PR list,
  30× multi-tenant surge), restore-verify at cell scale, the monorepo-ceiling benchmark, the full
  erasure-reaches-every-holder fan-out, the audit-tamper proof, the online-migration-under-load drill, and
  **the real-kernel escape drill** (re-run on every backend/image/kernel change — see §3.5).
- **The incident loop (T-3 tail).** Every real incident **ends by adding a drill that reproduces it** — the
  custom scanner built from the fingerprint of a recurring failure (EI-01 §5). The drill is committed; the
  incident is not closed until its reproducing drill is green. This is how the catalogue grows.

### 3.5 The one hard gate — the real-kernel sandbox-escape drill (AG-D4 / CI T-1 / E-9)

This is the **single hard go/no-go** before **any** untrusted customer code runs (CI step **or** agent
`ToolHands::exec` — one unified runner, ADR-20). An adversarial corpus runs inside a production-backend
sandbox on a **real kernel**: kernel-exploit primitives, cloud-metadata SSRF (169.254.169.254) → cred theft,
control-plane/internal-RPC reach, cross-tenant network/storage, fork bomb, disk fill, secret exfil via egress.
**Gate: zero escapes.** It emits a green attestation artifact **or CI is no-go for untrusted code**. It is
**not** a CI-cheap drill nor a once-and-done — it is **re-run on every backend/image/kernel change** and is the
sequencing spine: until it is green on the production backend, no untrusted CI step and no agent compute call
runs (R-1: sequence RCE/escape floors before any feature). A property not drilled on a real kernel is a
**claim**, and one escape is catastrophic (EI-04 §5).

---

## 4. The consolidated quantified drill catalogue (the master list)

The master merge of **Phase-3 Part A** (101 drills across 11 shared systems, 9 families) + the **five
subsystems' `architecture/07`** drills. Each row: **id**, **owner**, **family** (F1–F9 from Part A §A.1, or a
unique tag), the **quantified threshold**, the **green artifact** it emits, and **freq** (CI = cheap/every
change; SCHED = expensive/scheduled; GATE = the one hard go/no-go). Subsystem rows that are *instances* of a
shared family name the family so Phase 5 runs the family across owners with one harness + one scorecard column.

### 4.1 The nine reusable families (the recurring prove-its, contract-anchored)

| Family | Property | Quantified gate (default-to-beat) | Spine |
|---|---|---|---|
| **F1 — Zero-escape / no-leak** | A viewer never finds/reads what they can't access. | 0 leaked docs/edges/backlinks/notifications/results, **0 count/IDF/ranking leak**, across an adversarial corpus, incl. under zookie staleness. | ADR-03, SC-1 |
| **F2 — Cross-tenant IDOR** | No cross-tenant/cross-cell read via path-tenant spoof. | 0 cross-tenant rows; `tenant-predicate` lint catches a tenant-less query at compile. | EI-02 §1 |
| **F3 — Restore + cross-seam integrity** | Rebuild from backups lands at one consistent point. | 0 loss; OLTP↔blob↔index↔offset mutually consistent; post-restore re-erasure runs. | ADR-18 |
| **F4 — Reindex-from-cold parity** | A derived store rebuilds to match live, via the live consumer path only. | cold == live (docs/ACL/ranking/edges/vectors/inbox); no bespoke recovery reader. | EI-04 §5.3 |
| **F5 — Zero-loss-across-reconnect / outbox no-ghost** | No event lost/ghosted across a broker drop or producer/worker crash. | 0 lost, 0 ghost, 0 duplicate effect. | BUS-2/3, ADR-04 |
| **F6 — 30× surge + protected human lane** | A human request survives a machine-speed surge; other tenants unaffected. | human-lane latency within budget; agent lane sheds (429 + Retry-After); cross-tenant impact = 0. | ADR-16 |
| **F7 — Id-hiccup / fail-static** | A transient Id/CP hiccup degrades, doesn't cascade; a revoked actor still denied. | authenticated traffic survives within W; staleness ≤ `static_max` ≤ revocation SLA; zookie reads bypass cache. | ADR-17 |
| **F8 — Disabled-user → zero-access-in-N-min** | A disabled/revoked principal loses all access within N min. | every surface denies within **N = 5 min** (default-to-beat); token TTL + denylist + cache expiry ≤ W; stale re-grant = 0. | Zanzibar §2.4.4 |
| **F9 — Loop/runaway adversarial** | An agent→agent loop/storm is structurally halted. | loop halts ≤ depth ceiling (agent 12 / traversal 16); tripwire trips the per-tenant breaker; bounded pool drops over-cap; runaway stops at the wallet. | AG-6 |

**The named thresholds (Q32 defaults-to-beat; Phase 6 measures and sets the final numbers):** N = **5 min**
revocation; surge = **30×**; fail-static W = **5 min** (DPO-ratified, L-1); **RPO ≤ 5 min**, **RTO ≤ 1h/tenant,
≤ 4h/cell**; freshness p99 = seconds-grade; depth ceilings 12 (agent loop) / 16 (graph traversal); `order_key`
rebalance at 48 chars; projection-feeder promotion at > 5% of view executions; recall@k under filter ≥
threshold; timer-wheel fire-latency at 1M+ outstanding within the tick budget. Each is a **measured** number
in Phase 6, not a predicted one (EI-02 §8).

### 4.2 Shared-system drills (Phase-3 Part A — 101 drills)

> Owner abbreviations: SUB = Substrate, ID = Identity, BUS = Event Bus, REF = Reference Graph, SRCH = Search,
> NOTIF = Notifications, AG = Agent Fabric, FLOW = Durable Workflow, STOR = Storage, GA = GDPR/Audit,
> CP = Tenancy/Control-Plane. The green artifact for every drill is the named telemetry assertion (contract
> 1.8) passing; the "artifact" column names the *specific* signal that must read green.

| Drill | Owner | Fam | Quantified threshold | Green artifact | Freq |
|---|---|---|---|---|---|
| SUB-D1 | SUB | F5 | Kill service between commit & publish → outbox delivers every committed event exactly-once-in-effect (0 ghost, 0 lost). | outbox-depth drains; dedup ledger | CI |
| SUB-D2 | SUB | F5 | Drop broker mid-stream → 0 lost across reconnect (bind-by-name, dedup); slow subject doesn't block others. | consumer-lag; no HoL stall | CI |
| SUB-D3 | SUB | F6 | 30× agent surge one tenant → human lane holds, agent sheds, others unaffected. | shed-counts/lane; per-tenant RED | SCHED |
| SUB-D4 | SUB | F7 | Id-hiccup → already-authenticated survives within W; revoked denied when window closes. | fail-static fresh/stale/closed | CI |
| SUB-D5 | SUB | retry-storm | Trip a downstream breaker → callers fail fast + honour Retry-After; no amplification. | breaker-state; Retry-After issuance | CI |
| SUB-D6 | SUB+STOR | F3 | Rebuild from backups → no loss; OLTP↔blob↔index↔offsets one consistent point. | restore-verify-pass | SCHED |
| SUB-D7 | SUB | F2 | Cross-tenant read via path≠token tenant → 0; lint catches tenant-less query at compile. | misroute-count 0; lint green | CI |
| SUB-D8 | SUB | F9 | Adversarial agent→agent loop → depth ceiling + tripwire + bounded pool halt it. | causal-depth histogram; tripwire | CI |
| SUB-D9 | SUB | liveness | Kill a critical dependency → instance not-ready + sheds; no restart-storm. | readiness flips; no liveness churn | CI |
| SUB-D10 | SUB | migration | expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime. | lock-wait p99; 0 errored writes | SCHED |
| ID-D1 | ID | F8 | SCIM-disable → every surface (UI/API/git wire/agent) denies within **N=5 min**; cache+token+denylist ≤ W. | deny-latency histogram | SCHED |
| ID-D2 | ID | F7 | Break Id dependency → authenticated survives on coarse cache; just-revoked still denied (zookie bypass). | fail-static ratios | CI |
| ID-D3 | ID | F2 | Cross-tenant check/list/read via path spoof → 0 cross-tenant tuples readable. | cross-tenant count 0 | CI |
| ID-D4 | ID | F1 | Confidential issue/overridden page/private channel absent from any `list_objects`/search/refs for an unauthorized viewer. | zero-escape counter | CI |
| ID-D5 | ID | F9 | Adversarial delegation: agent confined to `agent.policy ∩ delegation ∩ tenant.policy`, incl. via a delegator who lost the right. | denial counter; intersection proof | CI |
| ID-D6 | ID | F8 | Kill a run mid-flight → per-run token revoked (teardown) AND auto-expires (`expires_at`) within run-life ≤ W. | token-revocation lag | CI |
| ID-D7 | ID | F8 | Revoke then re-read with post-revoke zookie → no stale allow ("new enemy"). | zookie-watermark honoured | CI |
| ID-D8 | ID | F3 | Restore to a consistent point → no resurrected grants past an erasure; post-restore re-erasure runs. | re-erasure receipt | SCHED |
| ID-D9 | ID | F6 | 30× agent surge on the authz hot path → human lane holds, agent sheds. | shed-counts; authz p99 | SCHED |
| BUS-D1 | BUS | F5 | Kill consumer + sever broker during sustained publish → 0 lost, 0 duplicate effects on reconnect. | lost/dup = 0; lag drains | CI |
| BUS-D2 | BUS | F5/HoL | Flood unhandled types at a `*`-subscribed consumer → whitelist consumer doesn't stall; lag alarm fires. | lag alarm; no stall | CI |
| BUS-D3 | BUS | replay | Replay a `correlation_id` tree → deterministic re-drive, idempotent, causality preserved (replay == original). | replay-equals-original hash | CI |
| BUS-D4 | BUS | F5 | Crash producer between state-commit and publish → event still delivered (outbox), never without state. | outbox emit-iff-committed | CI |
| BUS-D5 | BUS | F4 | Wipe a derived store, `reindex(scope)` → rebuilt store byte-matches live. | reindex-parity hash | SCHED |
| BUS-D6 | BUS | F9 | Self-triggering automation → depth ceiling + shared-root tripwire trip the per-tenant breaker. | tripwire firing; breaker trip | CI |
| BUS-D7 | BUS | F6 | 30× agent publish surge one tenant → human/control lane holds, agent sheds, others unaffected. | shed-counts/lane | SCHED |
| BUS-D8 | BUS | erasure | Erase a subject → inline-PII events unrecoverable (key destroyed); `*.erased` tombstones emitted; consumers degrade. | erase-receipt; tombstone count | SCHED |
| BUS-D9 | BUS | per-ref order | Burst force-pushes to one hot ref → `git.ref.updated` in push order per ref, parallel across refs, at target QPS. | per-aggregate order preserved | SCHED |
| REF-D1 | REF | F1 | Confidential artifact referencing a public one absent from backlinks/traverse for an unauthorized viewer (incl. filter-mode, zookie staleness). | zero-escape counter | CI |
| REF-D2 | REF | F2 | Cross-tenant edge read via path spoof / crafted cross-tenant URN → 0 cross-tenant edge. | cross-tenant edge 0 | CI |
| REF-D3 | REF | F6 | "Referenced-by-50,000" under concurrent permission-filtered reads → paginated p99 within budget; R4 serves post-promotion. | fan-out read p99 | SCHED |
| REF-D4 | REF | F4 | Wipe `edge` index, `reindex` → byte-matches live; a TE-7 drift reconverges to the typed table (typed wins). | reindex-parity hash | SCHED |
| REF-D5 | REF | erasure | Erase a subject + a referenced artifact → references tombstone, person unresolvable, 0 recoverable PII, no 500 on resolve. | erase-receipt; 0 resolve-error | SCHED |
| REF-D6 | REF | F8 | Revoke access, re-read backlinks with post-revoke zookie → no stale allow (bypasses fail-static). | zookie-bypass honoured | CI |
| REF-D7 | REF | F5 | Crash producer between content/relation commit and publish → edge event still delivered, never an edge without content. | outbox emit-iff-committed | CI |
| REF-D8 | REF | traversal bound | Cycle + 1000-deep chain → CTE terminates (visited-set + depth 16), cycle surfaced, statement timeout respected. | depth-bound honoured | CI |
| REF-D9 | REF | sub-tombstone | Delete an embedded block / PR comment → embed degrades to partial/relocated, not 404; 0 dangling embed. | tombstone ladder state dist. | CI |
| REF-D10 | REF | F6 | 30× agent ref-creation + backlink-read surge → human read lane holds, agent sheds. | shed-counts; read p99 | SCHED |
| SRCH-D1 | SRCH | F1 | Confidential/overridden/private artifact never in any `query`/`semantic` result (incl. counts, IDF, "more results", RAG). | zero-escape counter | CI |
| SRCH-D2 | SRCH | F1/F8 | Revoke, re-search with post-revoke zookie → excluded; default-consistency search excludes within W. | exclusion within W | CI |
| SRCH-D3 | SRCH | F2 | Search scoped to another tenant via path spoof → 0 cross-tenant results. | cross-tenant results 0 | CI |
| SRCH-D4 | SRCH | erasure | Erase a subject → every doc/field/**vector/embedding** purged (not hidden), unrecoverable; 0 orphan embedding. | embedding-purge receipt | SCHED |
| SRCH-D5 | SRCH | F4 | Wipe index, `reindex(scope)` → rebuilt index matches live (docs, ACL, ranking, vectors), live consumer path only. | reindex-parity hash | SCHED |
| SRCH-D6 | SRCH | F6 | 30× agent/CI query surge → human search lane holds, agent sheds, others unaffected. | shed-counts; search p99 | SCHED |
| SRCH-D7 | SRCH | freshness | Under load, event→searchable p99 within seconds-grade budget; index-lag alarms before user-visible staleness. | index-lag alarm; freshness p99 | SCHED |
| SRCH-D8 | SRCH | filtered-ANN | Selective ACL/structured filter → k nearest **visible** neighbours (filter-during-traversal), recall@k ≥ threshold; no leak. | recall@k; zero-escape | SCHED |
| SRCH-D9 | SRCH | F3 | Restore index with OLTP/blob/offsets → no resurrected erased docs (re-erasure runs); no row↔doc↔vector mismatch. | restore-verify; re-erasure | SCHED |
| SRCH-D10 | SRCH+STOR+AG | HYOK | Mark a content class HYOK → Search/Agents skip it (`can_derive_plaintext_index()=false`); 0 HYOK plaintext in any derived store. | 0 HYOK plaintext indexed | SCHED |
| NOTIF-D1 | NOTIF | ranking | Replay a mixed week → every `critical`/`direct` ranks above every `fyi`; first-important latency within budget; explain-trace per rank. | important-buried-rate 0 | SCHED |
| NOTIF-D2 | NOTIF | storm-control | 1000 near-identical CI failures + a 30-comment PR burst → bounded items (`coalesce_count` correct); self-notifications suppressed. | dedup-collapse-ratio; 0 self | CI |
| NOTIF-D3 | NOTIF | F4 | Wipe `inbox_item`, `reindex(notif)` → rebuilt inbox matches live (items + read-state from source events). | reindex-parity hash | SCHED |
| NOTIF-D4 | NOTIF | F1 | Notify on a confidential issue/private channel to a viewer lacking access → humanised tombstone; title never appears; item suppressed if recipient can't see subject. | 0 title/PII leak | CI |
| NOTIF-D5 | NOTIF | F6 | 30× agent-generated notification surge → human inbox-read lane holds, agent sheds, delivery-adapter bulkhead bounds provider load. | shed-counts; delivery-success | SCHED |
| NOTIF-D6 | NOTIF | erasure | Erase a user → every inbox item humanises to `[erased user]`; 0 recoverable PII; off-cell-sent payload crypto-shredded/erasure-requested. | erase-receipt; 0 recoverable | SCHED |
| NOTIF-D7 | NOTIF | F5 | Start escalation; kill Notif mid-`ack_window` → durable workflow resumes, pages next step exactly once; an ack stops the chain. | exactly-once page; ack-halt | CI |
| NOTIF-D8 | NOTIF | quiet-hours | Set DND; fire a `critical` escalation → it pierces quiet-hours; a `watching` item is suppressed. | critical pierces; non-crit suppressed | CI |
| NOTIF-D9 | NOTIF | delivery idempotency | Crash between provider-ack and ledger-write, retry → `UNIQUE(idem_key)` collapses to exactly-one delivery per (item, channel). | 1 effective delivery | CI |
| NOTIF-D10 | NOTIF | F5/HoL | Inject a slow/poison Signal type → whitelisted-template router doesn't stall, terminates poison, lag-alarm fires. | no stall; lag alarm | CI |
| AG-D1 | AG | plan-then-apply | A tool tries to write outside `EffectApi` → structurally impossible (`no-host-exec` + `no-cross-db` lints green). | lints green | CI |
| AG-D2 | AG | F9 | Effect outside the `∩` → `Denied` returns to the loop, no privileged fallback fires. | denial counter; 0 fallback | CI |
| AG-D3 | AG | F9 | Agent attempts an effect policy allows but delegation/tenant forbids (and vice-versa) → confined to the intersection. | intersection proof | CI |
| **AG-D4** | **AG (CI owns)** | **escape** | **`compute` tool attempts a kernel escape on a real kernel → ZERO escapes. The single hard gate before any untrusted code runs.** | **green escape attestation** | **GATE** |
| AG-D5 | AG | HITL | Gated tool → withheld (returns error, does NOT mutate); card shows action+risk+cost; approval resumes + applies once; rejection halts. | 0 mutation pre-approval; 1 apply | CI |
| AG-D6 | AG | F6/F9 | 30× agent dispatch surge → human lane holds, agent sheds, reserve/settle refuses over-budget runs, others unaffected. | shed-counts; reserve refusals | SCHED |
| AG-D7 | AG | F9 | Adversarial agent→agent self-trigger → depth ceiling (12) + tripwire + bounded pool halt ≤ ceiling; per-tenant breaker trips. | causal-depth; tripwire; breaker | CI |
| AG-D8 | AG | F8 | Kill a run mid-flight → token revoked on teardown AND auto-expires ≤ W; 0 shared token leaked into the child env. | token-revocation lag; 0 leak | CI |
| AG-D9 | AG | determinism | Run a scripted mock twice → identical proposed-effect sequences; `cargo-mutants` over event→trigger→effect→event ≥ mutation threshold. | identical effect seq; mutation score | CI |
| AG-D10 | AG | erasure | Erase a subject → run trace + agent memory/embeddings crypto-shredded/purged; attribution → opaque pseudonym. | erase-receipt; 0 recoverable | SCHED |
| AG-D11 | AG | F9 | Runaway loop vs an exhausted wallet → reserve refuses new runs (never interrupts in-flight); loop stops at the wallet. | reserve refusals; 0 interrupt | CI |
| FLOW-D1 | FLOW | F5 | Kill a worker at activity 5/10 mid-run → another re-leases, replays, resumes at step 6 with 0 re-executed side effects, 0 lost progress, exactly-once-in-effect. | replay-rate; 0 double-effect | CI |
| FLOW-D2 | FLOW | determinism | Replay against a divergent/wrong-version definition → divergence guard halts as `nondeterministic` + dead-letters; 0 silent divergence. | nondeterministic-halt count | CI |
| FLOW-D3 | FLOW | timer scale | Arm 1M+ durable timers + a burst due in one minute → due timers fire within the tick budget; far-future ~free; a crash re-fires unfired. 0 lost/0 double-fire. | timer-wheel lag; 0 lost/dup | SCHED |
| FLOW-D4 | FLOW | multi-day HITL | A gated workflow waits across a worker restart + a deploy; deliver `approval` days later (double-click) → resumes, consumes once, runs/withholds correctly. | 1 consume; withhold = 0 mutation | CI |
| FLOW-D5 | FLOW | F5 | Crash between journaling an activity's DB write and emitting its event → journal + outbox committed together (one txn); 0 ghost, 0 lost. | co-commit proof | CI |
| FLOW-D6 | FLOW | F9 | Runaway agent loop vs a depleting wallet → a new spend-bearing activity refused at reserve; an in-flight one never interrupted. | reserve refusals; 0 interrupt | CI |
| FLOW-D7 | FLOW | F9 | Adversarial workflow→event→workflow loop → depth ceiling + bus tripwire + bounded activity pool stop it (drops/parks, never forks). | causal-depth; 0 fork | CI |
| FLOW-D8 | FLOW | F6 | 30× surge of agent-initiated workflows → human-initiated lane holds, agent sheds, others unaffected. | shed-counts/lane | SCHED |
| FLOW-D9 | FLOW | erasure | Erase a subject with inline-PII history/signal rows → keys destroyed (unrecoverable incl. backups), references tombstoned, structure preserved. | crypto-shred-lag; 0 recoverable | SCHED |
| FLOW-D10 | FLOW | F3 | Restore `myelin-flow` PG to a consistent point → in-flight runs resume; store↔outbox offsets↔referenced rows at one consistent point; no run pointing at a vanished result. | restore-verify; consistent point | SCHED |
| STOR-D1 | STOR | F3 | Rebuild from backups to offset T → 0 loss (checksum parity); OLTP↔blob↔index↔offset at one consistent point. **The headline durability gate.** | restore-verify-pass | SCHED |
| STOR-D2 | STOR | RPO/RTO | Kill a cell; restore → **RPO ≤ 5 min** (WAL tail); **RTO ≤ 1h/tenant, ≤ 4h/cell**. | backup-RPO-seconds; restore-time | SCHED |
| STOR-D3 | STOR+GA | F3 | Erase a subject; restore an *older* backup → the erased subject is still erased (post-restore re-erasure ran). 0 resurrected. | re-erasure receipt | SCHED |
| STOR-D4 | STOR | crypto-shred reach | Erase a subject; attempt recovery from backups → per-subject ciphertext unrecoverable (key destroyed, excluded from backup). 0 recoverable PII in any backup. | crypto-shred-lag; 0 recoverable | SCHED |
| STOR-D5 | STOR | residency | Read/replicate a tenant's data outside its region → impossible (region in partition key; `residency-pin` rejects out-of-region writes). 0 cross-region PII egress. | residency-attestation; 0 egress | SCHED |
| STOR-D6 | STOR | KMS degrade | Transient KMS outage → resolved-DEK reads survive (bounded TTL); hard-down → not-ready+shed (not fail-open). 0 plaintext-without-key. | fail-static; 0 fail-open | CI |
| STOR-D7 | STOR | blob integrity | Corrupt an object → re-hash-on-read detects it (content-address mismatch); recover from replica/backup. 0 silent serve. | integrity-check; 0 silent serve | CI |
| STOR-D8 | STOR | migration | expand→backfill→contract on a restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime. | lock-wait p99; 0 downtime | SCHED |
| GA-D1 | GA | erasure-fanout | Erase a subject seeded into all H1–H18 → data-map fan-out hit every holder; post-erase `locate` returns 0 recoverable PII. **0 holders missed.** | erasure-fanout-coverage = 100% | SCHED |
| GA-D2 | GA+SRCH | erasure-search | The subject's docs **and embeddings** purged+reindexed out (not hidden). 0 hits, 0 embedding re-identification. | embedding-purge receipt | SCHED |
| GA-D3 | GA | audit tamper | Retroactively edit/delete an audit entry → the chain breaks + a consistency proof against the published STH fails + the external witness mismatches. Tamper detected 100%. | tamper-detection proof | SCHED |
| GA-D4 | GA | DSR deadline | Open a DSR → the durable timer fires a warning Signal before the 1-month deadline; the certificate seals on completion. 0 silent misses. | DSR-timer fire; sealed cert | SCHED |
| GA-D5 | GA | data-map drift | Add an untagged personal-data field → `no-untagged-personal-data` lint fails the build; the data-map diff surfaces it. Build red on untagged PII. | lint red on untagged PII | CI |
| GA-D6 | GA | legal-hold | Set a hold over a subject; submit an erase → erasure deferred-by-hold (not run), resumes on hold-lift. 0 held-scope deletions. | hold-defer receipt | SCHED |
| GA-D7 | GA+BUS/AG | restriction | Restrict a subject → no indexing/agent-use/analytics/notification while storage retained; reversible. 0 processing of a restricted subject. | restriction-suppression proof | CI |
| GA-D8 | GA+CP | F2 (**FLOOR**) | Multi-cell erasure: fan-out iterates all `member_cells ∪ home_cell`; merged a complete receipt set. 0 cells missed. | per-cell receipt set | SCHED |
| CP-D1 | CP | PII-free | Data-map over the control-plane schema → 0 `is_personal=true` columns; writing a name/email → build fails (`control-plane-pii-free`). | lint green; 0 PII columns | CI |
| CP-D2 | CP | F2 | Request to a cell for a `tenant_id` it doesn't host → misroute rejection, 0 cross-tenant/cross-cell read, audited. | misroute-count; audit entry | CI |
| CP-D3 | CP | residency | Write where `row.region ≠ cell.region` → `residency-pin` rejects; `residency_verify` attestation passes. | residency-attestation | CI |
| CP-D4 | CP | F7 | Hard-down the control plane → already-placed tenants keep serving; only signup/provisioning degrades. | serving-uptime; degrade scope | SCHED |
| CP-D5 | CP | bulkhead | Fatal fault / 30× surge in one cell → other cells unaffected; noisy tenant contained to its cell. | cross-cell impact 0 | SCHED |
| CP-D6 | CP | F3 | Provision a fresh cell → passes restore-verify + readiness before accepting any tenant; failing cell stays `provisioning`. | restore-verify + readiness gate | SCHED |
| CP-D7 | CP | F3 (**FLOOR**) | Migrate a tenant cell→cell (same region) → 0 loss across-seam, lands in-region, source crypto-shredded. | migration receipt; 0 loss | SCHED |
| CP-D8 | CP | F1 (**FLOOR**) | Cross-cell ref (multi-cell) → bridge carries only `subject`/`type`/`correlation_id`; target resolves per-viewer; unauthorized → tombstone. | PII-free bridge proof | SCHED |

### 4.3 Subsystem drills (the five `architecture/07` sets, mapped to families)

> These are instances of the shared families on subsystem surfaces, plus subsystem-specific drills. The
> green artifact is again the named telemetry assertion. Many are the **per-subsystem face** of an E2E
> scenario in §2.

| Drill | Owner | Fam | Quantified threshold | Green artifact | Freq |
|---|---|---|---|---|---|
| GIT-D1 | Git | F5/per-ref | Burst force-pushes + rapid pushes to one hot ref (1×/10×/30×) → `git.ref.updated` in push order per ref; refs parallel; 0 lost/ghost. | per-aggregate order; outbox depth | SCHED |
| GIT-D2 | Git | erasure | Erase a subject who authored commits/PRs/comments + LFS → every holder hit; residual == the ONE platform-posture residual (10.9); crypto-shred reaches backups. | DSR receipt set; ledger entry | SCHED |
| GIT-D3 | Git | F4 | Wipe Search code index + Refs edges + `check_status` projection; `reindex`/`replay` → cold rebuild byte-matches live; no cross-DB read. | reindex-parity hash | SCHED |
| GIT-D4 | Git | ceiling | Grow a synthetic monorepo until partial-clone/sparse/bitmaps degrade → documented v1 ceiling (GF-4); clone/fetch p99 held below it. | ceiling numbers; clone p99 | SCHED |
| GIT-D5 | Git | linearizable | Concurrent merges + force-push to one protected `base_ref` + DB-replica failover + node recovery mid-merge → linearizable on the ref CAS; no split-brain; 0 lost merge; `update_seq` monotonic. | 0 conflicting tips; reconcile log | SCHED |
| GIT-D6 | Git | F6 | 30× agent/CI clone surge on a hot repo → human fetch p99 held; agent/CI sheds (429 + Retry-After); 0 cross-tenant starvation. | shed-counts; fetch p99; CDN hit | SCHED |
| GIT-D7 | Git | sub-anchor | Force-push/rebase a PR with open inline threads → anchors resolve LIVE/MOVED/OUTDATED/GONE correctly; 0 mis-anchored; never silently wrong. | per-anchor state distribution | CI |
| GIT-D8 | Git | F2 | Cross-tenant repo access via token tenant ≠ URL-path tenant → tenant from token; 0 cross-tenant read; rejected at front door. | authz deny; lint green | CI |
| GIT-D9 | Git | F5 | Crash serving tier mid-push (after policy, before/after commit) → `git.ref.updated` emitted iff the ref move committed; no ghost/lost; quarantine objects discarded on abort. | outbox emit-iff-committed | CI |
| GIT-D10 | Git+CI | X-1 check seam | (a) out-of-order/dup `ci.check.updated` → `run_attempt`-monotonic supersession holds, drops stale; (b) fork PR self-greens → **neutral for gating**; (c) maintainer endorses → green; (d) doubly-delivered `ci.result` → workflow wakes **exactly once**; 0 double-merge. | 1 current row/key; merge-count == 1 | CI |
| GIT-D11 | Git | F1 | Viewer with partial repo/PR visibility lists a 100k-PR tenant → `SetExpr` JOIN returns only visible rows (0 leak), **one query** (no N+1/post-filter); just-revoked grant reflected (zookie). | 0 leak; 1 SQL query; revoke latency | SCHED |
| CI-T1 | CI | **escape** | **= AG-D4.** Real-kernel adversarial corpus → **ZERO escapes** or CI is no-go for untrusted code. Re-run on every backend/image/kernel change. | **green escape attestation** | **GATE** |
| CI-D1 | CI | F5 | Kill the runner mid-job; kill the control plane mid-run → run resumes (replay + `SCHEDULE_AND_RUN_JOB` idempotent re-dispatch); effectively-once; 0 lost runs/double-deploys/duplicate publishes. | replay-rate; 0 double-effect | CI |
| CI-D2 | CI | F6 | 30× CI surge one tenant → interactive lane holds; batch sheds (429 + Retry-After honoured); others unaffected; reserve/settle refuses over-budget; killed-runner jobs re-queue within lease TTL, 0 orphans. | shed-counts; reaper; lease TTL | SCHED |
| CI-D3 | CI | erasure | `erase(subject)` fans to CI → PII in logs/artifacts/caches/run-state destroyed (per-subject DEK where isolable; per-tenant fallback) incl. backups; structure survives; 0 dangling leak. | DSR receipt; 0 recoverable | SCHED |
| CI-D4 | CI | supply-chain | Floating tag / tampered-unsigned component → digest-pin + sign-verify fail closed at plan/run; `ci.supply_chain.verification_failed` emitted. 0 un-pinned/unsigned executions. | 0 un-pinned runs; audit event | CI |
| CI-D5 | CI | reserve/settle | Exhaust the wallet, start a CI run + an agent `compute` job; replay across a pricing change → refuse-start (never interrupt in-flight); 0 starts past exhaustion; wholesale ≠ markup holds. | 0 over-exhaustion starts; cost parity | CI |
| CI-D6 | CI | cache-poison | Adversarial `UntrustedFork` run writes the default-branch cache scope → trust-tier/branch-scoped namespace holds structurally. 0 trusted-cache writes from a fork. | 0 fork→trusted writes | CI |
| CI-D7 | CI | F1/secrets | Adversarial fork run reads protected secrets → `read & !is_untrusted_fork` ABAC holds. 0 secret reads by a fork-tier run. | 0 fork secret reads | CI |
| CI-R3 | CI | residency | An EU-resident tenant's run → claimed only by an in-region runner; logs/artifacts/caches never leave region (CDN within-EU); `residency_verify` attests; `residency-pin` passes on every write. | residency-attestation; lint green | SCHED |
| CI-D8 | CI | X-1 (= GIT-D10) | push → `ci.check.updated` per context → green → merge; out-of-order/re-delivered; fork success; re-run → projection holds correct current row; lower `run_attempt` dropped; fork success neutral; merge-queue wakes on `ci.result` idempotently; 0 spurious unblocks. | correct row; 0 double-merge | CI |
| CI-D9 | CI | determinism | The `ci.pipeline` workflow body → no clock/RNG/IO outside `WfCtx`; `flow-determinism` lint passes; replay bit-identical; only journaled `job.done` feeds the body. | lint green; bit-identical replay | CI |
| CI-D10 | CI | F2 | A compromised self-hosted runner → scoped job token bounds it to its own tenant's `SelfHosted` jobs; 0 cross-tenant job/secret reads; attestation failure → cannot claim. | 0 cross-tenant reads | SCHED |
| CI-D11 | CI | F5/OQ-J | Drop the live-tail connection mid-run, reconnect with `last_seq` → firehose backfills `(last_seq, now]`; 0 log lines lost; `last_seq` past window → `resync_required` → range-read; scope bounded (never `*`). | 0 lost lines; resync fallback | CI |
| ISS-D1 | Issues | co-equal view | Edit an issue's date/scope on the board → roadmap reflects the **same row**, 0 drift, and vice-versa (same `ViewSpec`/table, asserted by row id). | same-row-id assertion | CI |
| ISS-D2 | Issues | flex-field latency | 50+ custom fields, 1M+ issues board query → under the **<1s keyboard budget** with the `SetExpr` JOIN; a cold ad-hoc query escalates to Search (same `Filter`); planner never emits a full JSONB scan. | query p99 < 1s; no full scan | SCHED |
| ISS-D3 | Issues | F1 | Cross-tenant + confidential-issue IDOR → not in any board/`SetExpr` JOIN/search/backlink/context-pane for an unauthorized viewer, incl. under zookie staleness. 0 leak. | zero-escape counter | CI |
| ISS-D4 | Issues | human-key | Create-storm (import + incident burst on one hot prefix, N workers) → no duplicate key, monotonic per prefix, gaps benign, per-prefix isolation, key == stored canonical `<id>`. | 0 dup key; monotonic | SCHED |
| ISS-D5 | Issues | reorder | N humans + an agent re-ranking the same backlog region → 0 silent clobber, bounded re-base churn, converges with `order_key` (2-char jitter), 48-char rebalance never reorders displayed order. | 0 clobber; converged order | CI |
| ISS-D6 | Issues | SLA durability | (a) breach fires after a restart; (b) business-calendar corpus (DST, multi-day, holiday, pause/resume) → computed `fire_at` matches wall-clock to the second; (c) breach starts the escalation chain. | fire-at accuracy; chain start | CI |
| ISS-D7 | Issues | trigger | Arm "remind me when unblocked" (`QueryAst`); resolve last blocker across a restart → fires **exactly once** into the one inbox; after `stale_after`, stale nudge fires once, trigger goes stale. | 1 fire; stale-once | CI |
| ISS-D8 | Issues | F4/rollup | (a) rollup freshness under a 10k-issue import (bounded ancestor recomputes, debounce); (b) `replay` rebuilds rollup + Refs edge projection drift-free vs live. | reindex-parity; debounce bound | SCHED |
| ISS-D9 | Issues | import | (a) `export→import→export` round-trips (ADF lossy-map nodes named, never silent); (b) a large import resumes after a crash, no duplicate creates; (c) doesn't starve other tenants. | round-trip oracle; 0 dup; lane p99 | SCHED |
| ISS-D10 | Issues | editor round-trip | `render(parse(md)) === md` over a corpus for issue bodies + comments (consumed subset; read+edit use the identical WASM parser). | 100% round-trip | CI |
| ISS-D11 | Issues | erasure | Erase a subject → PII gone from `issue` row (per-subject DEK), change-log, comments, attachments, OLAP (+restriction), Search (incl. embeddings), Refs; post-restore re-erasure catches a restore; third-party residual is the `[OPEN — LEGAL]` limit. | holder receipts; re-erasure | SCHED |
| ISS-D12 | Issues | guard | "Can't mark Done while CI red on the linked PR" (reads `CheckStatus` + trust posture) + "can't close while `blocked_by` open" → transition blocked with a reason; an agent hitting a governed transition is HITL-gated (withheld, no mutation) until approval. | transition blocked; 0 pre-approval mutation | CI |
| ISS-D13 | Issues | F5/OQ-J | A board at `scope = board:<id>` drops mid-edit-storm → `resume` backfill then live loses **zero ops**; `last_seq` past window → `resync_required` → `*.snapshot`. | 0 ops lost; resync fallback | CI |
| ISS-D14 | Issues | switch-test/UI | Can a Jira/Linear user complete create→triage→plan→board→done without a manual? + measured contrast/latency on primary screens, incl. empty/loading/error/permission/erased/agent-pending states. | switch-test pass; contrast/latency | SCHED |
| KN-D1 | Knowledge | F5/OQ-J (headline) | Kill a collab client mid-edit + sever the connection during multi-author edit; on `resume(scope=doc:<id>, last_seq)` → **0 ops lost, 0 duplicate** (`UNIQUE(op_id)`). Re-run across the CAS→CRDT `engine_promote` boundary. | 0 lost/dup; resume-gap size | CI |
| KN-D2 | Knowledge | editor round-trip | `render(parse(md)) === md` over a markdown-subset corpus (3 structured nodes `U+FFFC`-anchored, nesting, code, IME/paste). **100% round-trip; 0 regressions.** | corpus pass rate 100% | CI |
| KN-D3 | Knowledge | CAS floor | Two clients edit the same block concurrently → the loser is rejected with current state (never silently overwritten); different blocks edit in parallel, no false conflict. | 0 silent overwrites | CI |
| KN-D4 | Knowledge | erasure | Erase a subject → structured PII purged/pseudonymised, free-text under a per-subject DEK crypto-shredded (unrecoverable in op-log/snapshots/backups), embeddings purged, backlinks tombstoned. 0 recoverable incl. vectors; residual per 10.9. | holder receipts; key-shred count | SCHED |
| KN-D5 | Knowledge | F1 | A confidential page / overridden sub-page / row-restricted db / field-hidden column never in any view/backlink/search/embed/RAG result for an unauthorized viewer, incl. an aggregate `COUNT`. 0 leaked; 0 count-leak. | zero-escape counters | CI |
| KN-D6 | Knowledge | F4 | Wipe Knowledge's derived state; `replay(scope)` (block-granular `*.snapshot`) → rebuilt matches live; live consumer path only. | reindex-parity hash | SCHED |
| KN-D7 | Knowledge | F5 | Crash between the block/row commit and relay-publish → event still delivered (outbox), never without the state change. 0 ghost, 0 lost. | outbox emit-iff-committed | CI |
| KN-D8 | Knowledge | F6 | An all-hands doc with thousands of concurrent readers/editors → per-doc op cap + read-fanout bound + active-editor lane reservation hold within budget, others unaffected; incl. a same-gap LexoRank insert storm (0 reorder, bounded rebalance). | per-tenant in-flight; op fan-out | SCHED |
| KN-D9 | Knowledge | flex-DB latency | Filter/sort/group a large multi-tenant database (JSONB + projection + `SetExpr` conjoin) → read-time p99 within budget; measure the >5% facet-promotion trigger. | db-query p99; facet frequency | SCHED |
| KN-D10 | Knowledge | rollup latency | A rollup over a large related set, computed at read time (permission-filtered) → p99 within budget; measure when incremental materialisation is needed. | rollup p99 | SCHED |
| KN-D11 | Knowledge | HITL | An agent edits a doc via `EffectApi` → attributed "suggested by agent"; a consequential edit (publish/confidential) is HITL-withheld (returns `Denied`, no mutation) until approval; double-click is one approval; reserve/settle passed. 0 ungoverned/0 pre-approval/0 double-apply. | gate-state; idem-key dedup | CI |
| KN-D12 | Knowledge | erasure (trace) | Erase a subject → content-addressed agent traces crypto-shredded/purged; attribution falls back to the pseudonym. 0 recoverable PII; attribution intact. | trace holder receipts | SCHED |
| KN-D13 | Knowledge | F2 | Read a page/db/row across tenants via path-tenant spoofing → 0 cross-tenant read; `tenant-predicate` lint catches a tenant-less query at compile. | 0 cross-tenant; lint green | CI |
| CHAT-D1 | Chat | F5/OQ-J | Sever the gateway↔firehose mid-publish → `resume(stream, scope, last_seq)` recovers the gap (0 lost, 0 dup); `last_seq` past window → `resync_required` → `*.snapshot` (`MessageStore::resync_from`), still 0 lost. | 0 lost/dup; resync fallback | CI |
| CHAT-D2 | Chat | per-conv order | Burst sends + edits to one hot channel from many gateways → per-conversation total order (ULID `message_id`/`aggregate=conversation_id`); resume gap-free; out-of-order client ops reconcile. | total order; gap-free | SCHED |
| CHAT-D3 | Chat | F6 | 30× agent message/connection surge one tenant → human connection/read latency in budget; agent lane sheds (429 + Retry-After honoured); others unaffected. **(TE-21 build-gate drill.)** | shed-counts; connection p99 | SCHED |
| CHAT-D4 | Chat | deploy herd | Roll the gateway fleet under a connection storm → bounded reconnect rate; `resume` completes for all; no message loss; readiness gates new connections, liveness no restart-storm. **(TE-21 build-gate drill.)** | reconnect rate; 0 loss | SCHED |
| CHAT-D5 | Chat | F1 | Notify/unfurl a confidential artifact to a viewer lacking access → tombstone rendered, title never present (the 4-step ladder step 1). | 0 title leak | CI |
| CHAT-D6 | Chat | erasure (unfurl) | Erase a third party rendered in a card → tombstone on next render, 0 recoverable PII (no durable snapshot; cache re-resolves live → `erased`). | 0 recoverable; live re-resolve | CI |
| CHAT-D7 | Chat | live-update | An artifact's `ci.check.updated`/`*.updated` → the shared per-ref cache busts; viewers showing the card get a live firehose update within budget. | cache-bust; update latency | CI |
| CHAT-D8 | Chat | erasure | Erase a person → bodies crypto-shred in hot+cold segments+backups; mentions → `[erased user]` (pseudonym shred); read-state/drafts/unfurl-cache purged; Search (incl. embeddings)/Refs/Notif cascade. 0 recoverable PII. | holder receipts; 0 recoverable | SCHED |
| CHAT-D9 | Chat | HITL bridge | Request approval, kill Chat + Workflow mid-wait, approve days later → the gated tool runs exactly once; double-click is one approval (`idem_key=card_id`); deny withholds with no mutation; timeout auto-denies; resume under a fresh token. | 1 apply; 0 pre-approval mutation | CI |
| CHAT-D10 | Chat | batch HITL | A multi-effect card approved 2-of-3 → the 2 resume approved, the 1 withheld, each independent (`idem_key=card_id:<idx>`); no effect runs twice; the withheld never mutates. | per-effect idempotency | CI |
| CHAT-D11 | Chat | F1 | Search as a non-member → 0 results from channels you're not in; the `search-requires-acl-filter` lint fails any query path reaching the index without the `Filter` conjoined over `message.id`. | 0 leak; lint green | CI |
| CHAT-D12 | Chat | cache-loss | Flush + drop Valkey mid-session → the PG record is authoritative; a marker is at-worst slightly stale; unread counts recompute correctly. | PG authoritative; counts correct | CI |
| CHAT-D13 | Chat | F5 | Crash between message persist and event emit → either both committed or neither; the message and `chat.message.created` are atomic; no orphan/phantom. | co-commit proof | CI |
| CHAT-D14 | Chat | idempotent send | Retry a send (flaky mobile/agent) with the same `client_nonce` → one message (`UNIQUE(conv, client_nonce)`). | 1 message | CI |
| CHAT-D15 | Chat | F4 | Wipe + `replay(scope, since)` → Search/Refs/Notif read-models rebuild from `chat.*.snapshot`; steady-state and recovery share one path; erased subjects emit tombstones (no PII resurrected). | reindex-parity hash | SCHED |
| CHAT-D16 | Chat | agent mock | Drive the streaming UX against the mock runtime (`--use-mock`) → partials stream on the firehose; final replaces partial; a mid-stream reconnect `resume`s the final, never a half-message. | partial→final; 0 half-message | CI |
| CHAT-D17 | Chat | explicit-first | A casual `@agent` mention → notifies the agent's inbox, does NOT spawn a costed run; only an explicit action/structured trigger dispatches; reserve/settle gates even the explicit run. | 0 auto-spawn; reserve gate | CI |
| CHAT-D18 | Chat | sub-anchor | Edit a message referenced by another artifact → the `message-<id>` anchor stays stable (live); delete it → the embed degrades to a Tombstone carrying the root (channel), never dangles. | anchor stability; tombstone | CI |
| CHAT-D19 | Chat | switch-test/UI | Drive the real Chat UI → a team could move without hitting a wall the old tool didn't have; measured-contrast tokens; latency budgets (optimistic send < ~100ms perceived); flip-popovers against the real bottom-pinned composer anchor. | switch-test; contrast/latency | SCHED |

### 4.4 The four whole-system E2E scenarios as scorecard rows

| Scenario | Crosses | Quantified gate | Green artifact | Freq |
|---|---|---|---|---|
| **E2E-1 PR context pane** (UC-X-3) | Git, CI, Issues, Knowledge, Refs, Search, Id, Notif | every connected artifact resolves per-viewer; **0 leak** to the unauthorized viewer; live check-update within freshness budget; tombstone carries root. | pane-resolution trace + zero-leak = 0 + per-viewer diff | SCHED |
| **E2E-2 CI-fail → triage → issue → chat → fix-PR** (flagship) | CI, Agent, Workflow, Issues, Chat, Git, Id, Notif, Storage | 0 effect outside the `∩`; 0 mutation before approval; exactly-once approval + merge across a kill; reserve/settle balanced. | deterministic run trace + HITL withhold→approve→apply ledger + reserve/settle parity + merge-count == 1 | SCHED |
| **E2E-3 Spec-to-ship traceability** | Knowledge, Issues, Git, CI, Chat, Refs, Search, GDPR/Audit, Id | complete lineage per-viewer; **cold-reindex == live**; audit tamper detected 100%. | lineage diff (live vs cold) at 0 drift + tamper-detection proof | SCHED |
| **E2E-4 DSAR fan-out** | GDPR/Audit, Storage, Id, all 5 subsystems, Search, Refs, Notif, Workflow, Bus | **0 holders missed**; 0 recoverable PII (incl. vectors, incl. backups); residual == the one documented posture; certificate sealed. | H1–H18 coverage receipt set + post-erase `locate` = 0 + Merkle certificate | SCHED |

---

## 5. Honesty register (floors, open items, what this strategy does and does not yet prove)

Per the deferral discipline (VISION §3, EI-04 §4) — named, not silent:

- **FLOOR drills (owed when the follow-on is built, named here so the gap is visible):** GA-D8 (multi-cell
  erasure), CP-D7 (cell→cell migration), CP-D8 (cross-cell ref) — all gated on multi-cell shipping
  (single-home-cell is v1; cross-cell is designed-not-built, contract 12.6/OQ-I). KN-D1's CRDT-boundary leg,
  ISS-D5's move-CRDT promotion, and KN-D9/D10/ISS-D2 materialisation triggers are **measured-promotion**
  drills (the trigger is measured, not predicted, EI-02 §8).
- **The one hard gate (AG-D4 / CI-T1):** the real-kernel escape drill is the single go/no-go before any
  untrusted code runs — it is **GATE** frequency (re-run on every backend/image/kernel change), not CI-cheap,
  and it sequences everything (R-1). Until green, no untrusted CI step and no agent compute call runs.
- **`[OPEN — LEGAL]` (the structural floor ships regardless, the residual is flagged to counsel/DPO):** the
  ONE free-text/immutable erasure posture (10.9, X-7, L-2); worklog special-category classification (OQ-H);
  build-data-as-LLM-training basis (OQ-H); Art. 17 reach into immutable git bytes; the fail-static W
  ratification (L-1, the value behind every F7/F8 "≤ W" threshold). The drills assert the **structural floor**
  (crypto-shred + pseudonym-shred + restrict); the residual lawful-basis is not an engineering gate.
- **Thresholds that Phase 6 measures and sets (Q32):** every number in §4.1 is a **default-to-beat**, not a
  contract constant. Phase 5 enumerates the obligation + proposes the default; Phase 6 measures load + cell
  telemetry and sets the final value (N-min, 30×, W, RPO/RTO, p99 budgets, recall@k, timer-wheel latency,
  promotion points, cell sizing-bands Q33). A drill is not "proven" against a guessed number.
- **What this strategy does NOT yet prove (named):** L0 unit/property/mutation coverage per crate (each
  system owns it); the exact escape-drill adversarial corpus + green-attestation format (CI P6, `[OPEN → P6]`);
  the cross-cell drills (FLOOR, above); the real-LLM agent runtime path (v1 drills run against the mock
  runtime, `--use-mock`; the real adapter is post the escape drill). These are **claimed** until their drills
  emit green artifacts in Phase 6.

---

## 6. Cross-references

- [`contract-index.md`](../contract-index.md) — the frozen build-to surface every drill tests (owner/consumer/site).
- [`00-reconciliation-decisions.md`](../00-reconciliation-decisions.md) — the rationale + the X-1..X-7 / OQ-A..OQ-L shapes the E2E scenarios exercise.
- [`../../03-shared-systems-architecture/drills-and-open-questions.md`](../../03-shared-systems-architecture/drills-and-open-questions.md) — Part A (the 101-drill inventory + the 9 families + the survival-signal set) this consolidates.
- The five subsystems' `architecture/07-drills-and-open-questions.md` (git-hosting, continuous-integration, issue-tracker, knowledge-platform, chat) — the subsystem drill sets consolidated in §4.3.
- Spine: [`../../02-holistic-architecture/architecture-decisions.md`](../../02-holistic-architecture/architecture-decisions.md) (ADR-16/17/18/20); [`../../02b-doctrine-integration/integration-directives.md`](../../02b-doctrine-integration/integration-directives.md) (T-1..T-9, R-1..R-2, E-9); [`../../02-holistic-architecture/design-language.md`](../../02-holistic-architecture/design-language.md) §8b (frontend QA gates).
- Doctrine: [`../../../external-insights/01-process-and-quality-doctrine.md`](../../../external-insights/01-process-and-quality-doctrine.md) (the philosophy); [`../../../external-insights/04-hard-problems.md`](../../../external-insights/04-hard-problems.md).
