# Phase 2b — Doctrine Integration Decision Record

> Phase: `02b-doctrine-integration` (inserted between Phase 2 holistic-architecture and Phase 3
> shared-systems-architecture). Canonical brief: [`VISION.md`](../../VISION.md). Doctrine source:
> [`external-insights/`](../../external-insights/) (README + docs 01–05), treated as **DEFAULT we
> follow unless we write down why not** — the same canonical status as VISION (per
> [`external-insights/README.md`](../../external-insights/README.md)). Integrated against the
> Phase-2 spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
> (ADR-01…ADR-15), [`design-language.md`](../02-holistic-architecture/design-language.md),
> [`open-questions-and-risks.md`](../01-research/open-questions-and-risks.md).
>
> Built on the five per-doc analyses in [`analysis/`](./analysis/):
> [`platform-substrate.md`](./analysis/platform-substrate.md),
> [`agent-native-fabric.md`](./analysis/agent-native-fabric.md),
> [`hard-problems.md`](./analysis/hard-problems.md),
> [`process-quality.md`](./analysis/process-quality.md),
> [`ux-design.md`](./analysis/ux-design.md).
>
> Companion deliverable: [`integration-directives.md`](./integration-directives.md) — the binding,
> per-destination directives that make these decisions bite downstream.

---

## (a) Headline — the degree of convergence

**The substrate doctrine overwhelmingly CONFIRMS the Myelin spine.** Our 14-ADR register was
drawn from the same prior art the doctrine distills, so the bulk of docs 02 (platform substrate),
03 (agent fabric), and 04 (hard problems) is *validation, not work*: tenant-first partitioning
(ADR-11), one polymorphic principal for humans+agents (ADR-03/08/13), the event log + transactional
outbox as the nervous system (ADR-04/13), causality as a first-class envelope primitive (ADR-13.2),
one canonical URN reference graph (ADR-13.1), minimal justified storage (ADR-10/14), crypto-shred /
references-not-payloads (ADR-12), plan-then-apply + the strategy boundary + one trigger engine + HITL
in the tool layer (ADR-08). **There are no genuine CONFLICTS** anywhere in the five docs — every
divergence is a *sharpening*, a *net-new discipline at a lower altitude than Phase 2 chose to operate*,
or a *concrete default-to-beat for an open question*.

The doctrine's *value* is concentrated in a handful of real deltas. Docs 01 (process/quality) and 05
(UX) barely overlap our spine at all — they are *how to build it without it rotting* and *the concrete
day-one UX mandates our principles implied but didn't pin* — so they bind almost entirely **forward**
to Phase 5 (testing), Phase 6 (roadmap), Phase 8 (execution), and the design language. The integration
job is therefore mostly **routing**: planting each item as a named, non-skippable input to the phase
where it becomes real, plus a small number of committed-decision back-patches (five new ADRs, a
design-language §11, two VISION clauses) where a delta deserves canonical status *now* so it can't be
silently dropped later.

The few deltas that earned committed-decision status are listed in §(c). Everything routed downstream
without an ADR is in [`integration-directives.md`](./integration-directives.md).

---

## (b) Confirmations — evidence the architecture is sound

A representative (not exhaustive) map of doctrine ↔ the ADR/doc it validates. Full per-item tables
live in the five `analysis/` docs.

| Doctrine insight | Validates |
|---|---|
| Tenant is the unit of everything; tenant id first column / partition key (02 §1) | ADR-11.2 |
| One polymorphic principal for humans+agents; authority in one place (02 §2, 03 §4) | ADR-03, ADR-08.1, ADR-13.3 |
| Per-run, narrowly-scoped, short-lived, auto-revoked agent identity (02 §2) | ADR-08, Id §1.1 |
| Sync calls for queries / async events for reactions; one event per state change (02 §3) | ADR-04, ADR-13.2 |
| Per-subject ordering only; no global order (02 §3) | ADR-04.2 |
| At-least-once + idempotent + dedup ≈ effectively-once; don't chase exactly-once (02 §4) | ADR-04.1 |
| Transactional outbox is the sanctioned emit path; dual-write hazard (02 §4) | ADR-04.3, ADR-01 (`myelin-events`) |
| Causality carried on every event (caused-by, correlation, causation, depth) (02 §6) | ADR-13.2, ADR-08.6 |
| Canonical stable URN, one library owns parse/format/resolve (02 §7) | ADR-13.1 (`myelin-refs`) |
| Backlinks are event-sourced projections, rebuilt from the log (02 §7) | ADR-13 §Rationale |
| The graph that cascades an action is the graph a human traverses — the moat (02 §7) | VISION §1, ADR-13 |
| Minimal storage stack; one DB per service; no cross-service joins; S3-compatible object store (02 §8) | ADR-10, ADR-01 |
| Region-pinning; immutable tenant region; no cross-region query path (04 §1) | ADR-11.1/.2 |
| Content-addressed objects, Merkle history, pack/delta storage for git (04 §3) | git-hosting §3, VISION §5.4 |
| Mock→real is a single trait swap; no model name in platform code (03 §1) | ADR-08.2 |
| Agents act through the same gateway as humans; no carve-out; denial = ordinary tool error (03 §4) | ADR-08.1/.3/.4, ADR-13.3 |
| HITL approval lives in the tool layer; suggest-by-default (03 §5) | ADR-08.6, ADR-09 |
| Loop prevention is structural (depth caps, cycle detection, idempotent tools, breakers) (03 §5) | ADR-08.6 |
| Erasure: tombstone/pseudonymise; references-not-payloads; crypto-shred (04 §1) | ADR-12.1/.3/.4 |
| Per-tenant / customer-managed keys for crypto-shred (04 §1) | ADR-12.3 |
| Keep a seam for a column-store/time-series engine; don't add before measured (04 §5) | ADR-10/14 (log/firehose tier) |
| Command palette over the reference graph; same graph powers automation + navigation (05 §4) | DL §5.2, ADR-13 |
| One shell everywhere; the system assembles context (05 §4) | DL §5.1/§5.3 |
| Status never by colour alone; borders over shadow (05 §3) | DL §4, DL §3.5 |
| Empty/loading/error first-class; optimistic with honest rollback (05 §4) | DL §5.10, DL P2 |
| Design-first: IA + flows + wireframes incl. empty/loading/error before UI (05 §7) | VISION §3/§5.2, DL §7/§8.3 |
| Specificity contract (settled named directly / open left to you) (01 README) | the ADR DECIDED / DECIDED(directional) / OPEN→PN ladder |

---

## (c) The DELTAS we adopt

Each delta: the change · where it binds · the rationale. Items that earned a committed ADR are marked
**[ADR-NN]**; the rest are routed via [`integration-directives.md`](./integration-directives.md).

### D1 — Backpressure + protected-human-lane + shed order **[ADR-16]**
**Change.** Every queue and pool is bounded; fast-fail on saturation; statement timeouts. A
**protected interactive-human lane**; an explicit shed order **speculative → batch/CI → agent →
human-last**; agents/CI receive `429 + Retry-After`; **our own clients (CLI, agent runtime) MUST
honour Retry-After** or shedding becomes a retry storm. A **shared resilient inter-service client**
(timeout / circuit breaker / bounded-concurrency bulkhead / jittered-retry-idempotent-only;
never retry through a tripped breaker) becomes a named substrate crate.
**Binds.** ADR-16 (Phase-2 back-patch) · Phase 3 (Bus + Id rate-limiting + the shared client) ·
Phase 5 (the 30× agent-surge drill) · Phase 6 (sequence the client early).
**Rationale.** We govern *agent* load (ADR-08.6) but never reserved a human lane or ordered the
shedding, and the dominant client is now a fleet of agents. Without this, an agent surge degrades the
human experience first. (02 §5; 03 §5.4.)

### D2 — Fail-static vs fail-closed for availability **[ADR-17]**
**Change.** Distinguish **fail-closed (correct for authorization — deny when unsure)** from
**fail-static (correct for availability)**. On an Identity-dependency hiccup, Id serves a
**bounded-staleness cached answer** (actor still active / coarse grants) so *already-authenticated*
traffic survives; the staleness window is **bounded by the deprovision/revocation SLA** (and must
contain the short-lived agent-token TTL).
**Binds.** ADR-17 (Phase-2 back-patch; cross-references ADR-03 and ADR-11) · Phase 3 (Id cache +
consistency-token interplay) · Phase 5 (Id-hiccup drill) · **DPO ratifies the staleness bound**.
**Rationale.** This is the single highest blast-radius gap: without a bounded-staleness Id cache, one
Id-dependency hiccup is a whole-platform cascade because every request fails closed. The staleness ≤
revocation-SLA bound is a deliberate, counsel-aware trade-off, not a conflict. (02 §10.)

### D3 — Backup/restore-verification as a durability gate **[ADR-18]**
**Change.** Continuous log archiving + periodic base backups for a tight RPO, plus **automated
periodic restore-verification that rebuilds and asserts no loss**, wired into CI. Restore must be
**consistent across the seam** — OLTP rows ↔ object/blob ↔ search index ↔ event-log offsets restore to
a mutually consistent point; a row pointing at a missing blob is silent corruption.
**Binds.** ADR-18 (Phase-2 back-patch) · Phase 5 (the restore drill owns the assertion) · Phase 8
(CI gate) · Phase 3 (Storage + GDPR/Audit define the cross-tier consistency point; interacts with
post-restore re-erasure GD-14).
**Rationale.** "A backup that has never been restored is not a backup." ADR-12 lists backups as a
holder but never gates *restorability* or cross-seam integrity — a silent durability gap. (02 §11.)

### D4 — The Event / Signal / Automation-rule / Trigger four-primitive refinement **[ADR-19]**
**Change.** Refine our coarse "event → matcher → {subscription, automation, agent}" into four distinct
primitives: **Event** (a fact, every state change, over the durable log via the outbox); **Signal** (a
curated, deduplicated, severity-ranked *subset* of events actors should react to — the trigger
substrate); **Automation rule** (a stateless, per-event reflex the project owns: "when X, do Y");
**Trigger** (a *stateful promise a person owns*: armed → resolved / stale / disarmed, fires once per
arming). This resolves a genuine **vocabulary collision**: our existing "Trigger" (the matcher→target
*binding*) is renamed to a **subscription/automation binding**, freeing "Trigger" for the doctrine's
stateful per-person promise.
**Binds.** ADR-19 (Phase-2 back-patch; addendum to ADR-04/ADR-08, the shared reactive vocabulary) ·
Phase 3 (Event Bus / trigger engine implements the tiers; consumers subscribe to **curated Signals**,
not the raw firehose) · Phase 4 (Issues — task-unblock UX for the stateful Trigger) · DL/admin UX copy.
**Rationale.** The split is strictly richer than what we collapsed, and the Signal tier is the upstream
defence against the head-of-line-blocking gotcha (D7). It changes shared vocabulary every downstream
phase uses, so it deserves committed status now. (03 §2.)

### D5 — ONE sandbox for CI + agents (resolves TE-31) **[ADR-20]**
**Change.** CI steps and agent tool calls are the *same problem* (running untrusted code). **Unify**
them behind **one job spec with a `kind` field (`ci | agent`)** feeding one hardened runner.
Default-to-beat = **UNIFY**; Phase-4 CI must justify in writing if it diverges (inverts the prior
"prove it's worth unifying" burden). Isolation floor broadened to **gVisor-class userspace-kernel
*or* microVM**; plain shared-kernel containers rejected by default for untrusted code. A named
**hardening profile** (no host network / egress default-deny, read-only root + tmpfs, all caps
dropped, no-new-privileges, seccomp, images pinned by digest with fail-closed on un-digested tags,
whole-guest kill on teardown, cgroup `pids.max` + zero swap; secrets resolved *inside* the boundary,
never forwarded via the agent runtime). The **sandbox-escape drill on a real kernel is the single
hard gate before any customer code runs** (CI *or* agent).
**Binds.** ADR-20 (Phase-2 back-patch; resolves TE-31) · Phase 4 (CI, owns the runner + TE-28 threat
model) · Phase 3 (Agent Fabric) · Phase 5 (escape drill) · Phase 6 (the drill is a roadmap milestone) ·
Phase 8 (go/no-go).
**Rationale.** Build the isolation primitive once and harden it once; an undrilled isolation property is
a claim, not a fact. (03 §3; 04 §5.1.)

### D6 — Brain + hands boundary + skeleton mode (no ADR; sharpens ADR-08)
**Change.** Two strategy boundaries, each one method: the **brain** is a *stateless* provider
`step(conversation) -> {use_tools | submit}` with the platform-side agent loop owning conversation
history; the **hands** is `exec(command) -> result` with **no host-execution path that bypasses the
trait** (a simulation impl emits a channel-proof marker). Add **skeleton mode** — no model, no tools:
authenticate, fetch task, print summary, exit — proving the whole gateway/identity/dispatch path with
**zero spend and zero effects**; it is the *first* runtime to stand up (skeleton → mock → real). The
deterministic mock doubles as a **shipped `--use-mock` runtime flag** on the same code path, not just a
test harness.
**Binds.** Phase 3 (Agent Fabric — answers the open `Agent::handle` shape, AG-3) · Phase 6 (roadmap:
skeleton → mock → real) · Phase 8. (No ADR: ADR-08's plan-then-apply core is unchanged; this is the
concrete trait shape Phase 3 was already to decide.)
**Rationale.** Skeleton mode proves the wiring before any model spend; the stateless-`step` shape is the
concrete answer AG-3 was waiting for; `--use-mock`-as-runtime is the dogfooding/demo lever VISION §3
implied. (03 §1.)

### D7 — Orchestrator gotchas (no ADR; binds the consumer template + ops)
**Change.** Four concrete, expensive traps the reactive/dispatch tier must design around:
**(i) whitelist the subjects you handle** (no consumer subscribes to `*`) **+ monitor consumer lag**
(an over-broad subscription head-of-line-blocks everything); **(ii) bind to a durable consumer by name,
never re-declare its start policy on reconnect** (re-asserting start position can wedge the broker);
**(iii) acknowledge only after the work is enqueued, and terminate non-retryable (malformed) messages
immediately** rather than burning the redelivery budget; **(iv) thread `causation_id` nested
(immediate parent), not flat (everything → root)** or depth-capping and provenance both break. The
reactive/dispatch tier gets an **explicit, separately-reviewed design**, not folded into the bus.
**Binds.** Phase 3 (the `myelin-events` consumer template + the dispatch tier) · Phase 8 (consumer-lag
ops gate). (No ADR: these are design + ops disciplines on top of ADR-04's committed semantics.)
**Rationale.** Each is a specific, costly failure that is cheap to design around now and brutal to
retrofit. (03 §6.)

### D8 — Reserve/settle cost gate (no ADR; generalises ADR-08.6)
**Change.** A **universal reserve/settle cost gate in front of EVERY run, CI included**: reserve at
dispatch, settle on completion, **refuse to start when balance is exhausted, never interrupt one in
flight**. Meter **one cost event per model call, wholesale ≠ markup kept separate**. "No balance → no
execution" is uniformly true; this unifies agent budgets (ADR-08.6) and CI metering (TE-32) under one
substrate.
**Binds.** Phase 3 (shared metering capability) · Phase 4 (CI) · **Commercial** (wallet/pricing model) ·
generalises ADR-08.6 (note added there). (No ADR: it generalises an existing committed decision.)
**Rationale.** A cost pre-flight makes a runaway loop self-limiting; unifying CI + agent spend under one
gate closes part of TE-32 with a concrete default. (03 §5.2.)

### D9 — The process/quality doctrine (no ADRs; binds VISION + P5/P6/P8)
**Change.** Adopt doc 01 as the philosophy of Phases 5/6/8:
- **Prove it or it isn't real** — a property does not exist until a failure-injection drill forces it
  and observability watches the system survive; gates resolve to **quantified thresholds** (RPO/RTO,
  "zero sandbox escapes," "zero messages lost across a reconnect," "disabled user → zero access in N
  min"). *(→ Phase 5 organising thesis.)*
- **The ratchet** — every assumed discipline becomes a committed mechanical gate; **an uncommitted gate
  is no gate**; violations are loud, never swallowed (`|| true` banned). *(→ Phase 6 build / Phase 8.)*
- **Name your floors** — shipping a floor is fine; a floor masquerading as done is the failure; every
  floor names its follow-on; a capability is **proven only when a drill emits a green artifact** (else
  "claimed"); a durable **gap report** carries the real state between agents. *(→ VISION §3 + Phase 5 +
  Phase 8.)*
- **Code wins over docs** — when a doc and the code disagree, the code wins; fix the doc, then proceed;
  **date every status/capability note** (a claim that outlives its verification misleads the next
  agent). *(→ VISION §3 + Phase 7 prompt template + Phase 8.)*
- **Drive the real UI** — the **switch test**: a surface is done only when a team could move to it in a
  real browser without hitting a wall the old tool didn't have; write **chained-mutation E2E** tests.
  *(→ Phase 5 + Phase 8.)*
- **Order by non-negotiability** — the roadmap is ordered by *what kills you first*: silent data loss
  and RCE/sandbox-escape floors before any feature surface; **no later phase is "done" over a red
  earlier gate**. *(→ Phase 6 sequencing law + Phase 8.)*

**Binds.** VISION §3 (code-wins; name-your-floors; human-sign-off-is-the-bottleneck) · Phase 5
(organising thesis + gate table + failure-injection harness) · Phase 6 (sequencing law + gate
invariant + build gates early) · Phase 7 (prompt template fields) · Phase 8 (execution discipline).
**Rationale.** Our *entire build is sequential agents reading each other's notes*; stale/over-claimed
status is our sharpest self-deception risk, so the honesty + proof disciplines deserve canonical and
forward-binding status. (01 all.)

### D10 — Day-one UX primitives (no ADR; binds DL §11)
**Change.** Pin the concrete UX mandates our principles implied but didn't fix, in a new design-language
§11 (see [`design-language.md`](../02-holistic-architecture/design-language.md)):
overlay primitives (portal-always to root, centralized focus-trap / return-focus / scroll-lock with
scrollbar compensation / Escape+backdrop dismiss / ARIA *in the primitive*, one documented z-index
scale, single-purpose-by-shape); **one editor render path** where read and edit run the same inline
parser with **`render(parse(md)) === md`** as a hard round-trip gate and inline content stored as a
**markdown-subset string** (block structure stays AST; `mention`/`artifact_ref`/`embed` stay structured
nodes); measured-not-claimed tokens (**focus-token ≠ identity-token** because a brand accent can fail
AA; **status never by colour alone**; measured-contrast gate); the layout-containment bug checklist
(`min-height:0` on flex scrollers, `width:100%`-isn't-a-takeover, flip-popovers-off-screen,
hover-isn't-touch); **humanise machine strings at the backend** (paired with a routable `ArtifactRef`,
not a frontend string map — so every consumer *and every agent-authored message* inherits it); and
**"build these first."**
**Binds.** DL §11 (Phase-2 back-patch) · Phase 3 (Notifications templating + Refs display-name
resolution for backend humanisation) · Phase 4 (Knowledge owns the editor; Chat/Issues the responsive
cases) · Phase 5 (round-trip / contrast / latency / switch-test gates) · Phase 6 (overlay + editor
primitives sequenced before consumers).
**Rationale.** These are the *specific bugs and correctness gates* that turn a principle into a thing you
can fail a CI check on — and the most expensive retrofits if skipped. (05 all.)

### D11 — Resolved open questions (default-to-beat handed downstream)
| Open Q | Resolution (default-to-beat) | Binds |
|---|---|---|
| **TE-7** (Refs vs subsystems own relations) | **Backlinks stay event-sourced projections in Refs; lifecycle/semantic edges (closes/blocks/depends/assigns) are *also* mirrored to a typed relation table owned by the authoritative subsystem — the typed edge, not the URN string, is source of truth.** The hybrid both halves wanted. | ADR-15 note → Phase 3 (Refs) → Phase 4 (Issues + Knowledge own the typed tables) |
| **TE-31** (CI ↔ agent substrate) | **UNIFY** — one job spec, `kind ∈ {ci, agent}`, one hardened runner. | **[ADR-20]** |
| **TE-15** (collab engine) | **CAS-floor → CRDT**, and **resume-cursor durable transport built FIRST**: per-block optimistic compare-and-swap on a last-modified token + soft-locks + snapshot/restore as the named v1 *floor that does not merge*; the resume-cursor durable transport (idempotent apply, reconnect loses zero ops) is item 0; the CRDT is the scheduled next step, triggered by the *first true concurrent-edit conflict*. The editor round-trip gate (D10) is TE-15's correctness bar regardless of engine. | Phase 4 (Knowledge) + Phase 6 (sequencing) + Phase 3 (firehose transport) + Phase 5 (reconnect + round-trip drills) |
| **TE-16/TE-18** (content + rollups) | **Markdown-subset inline string** (structured ref/mention/embed nodes preserved) + **read-time-only rollups/formulas, never stored** (materialise only when measured too slow) — floor-first, inverting our current materialised-first lean. | ADR-05/ADR-06 directional notes → Phase 4 (Knowledge) |
| **reindex-from-source** | Elevate to a **first-class resilience primitive**: every derived store (Search, Refs, OLAP, Notifications) is reconstructible by asking each owner to **re-emit through the live consumer path**; the index never reads owner DBs; no bespoke recovery path. | ADR-13 note → Phase 3 (Search/Refs) + Phase 5 (reindex-from-cold drill) |

---

## (d) Conflicts and their resolution

**No genuine CONFLICTS across all five docs.** Three near-tensions, all resolved without overriding any
committed ADR:

1. **fail-static (D2) vs GDPR revocation latency.** Resolved by bounding the staleness window to ≤ the
   deprovision/revocation SLA, and containing the short-lived agent-token TTL inside it. A deliberate,
   written, DPO-ratified trade-off — internal to the doctrine, not a conflict with an ADR. (02 §10.)
2. **read-time-never-stored rollups (D11/TE-18) vs our current Knowledge lean to async materialised
   dataflow.** Both are admissible; ADR-06 leaves TE-18 open to P4. Resolved as a **floor-first**
   inversion (read-time → materialised only when measured too slow), consistent with the doctrine's own
   "don't add the engine before the volume is measured." A SHARPENS, not a conflict. (04 §2.5.)
3. **markdown-subset string (D10/D11) vs the `myelin-content` AST (ADR-05).** A *representation seam*,
   not a disagreement: **AST for block structure, markdown-subset string for inline runs**, with
   `mention`/`artifact_ref`/`embed` kept as structured nodes so reference-extraction stays reliable. No
   ADR reopened; a refinement note to ADR-05 and the Knowledge P4 sketch. (04 §2.4; 05 #9.)

The only other friction is **vocabulary** (the "Trigger" collision), resolved by the four-primitive
rename in **D4 / [ADR-19]** — nothing committed was *wrong*, only coarser than it should be.

A note flagged to Phase 7/8: doc 01's **"human sign-off is the bottleneck"** (about *our phased-agent
build process*) must **not** be conflated with ADR-08's **runtime HITL gates** (about *runtime agents
acting on tenant data*). Two distinct HITL notions; keep them separate downstream.

---

## (e) Stronger priors we carry as defaults-to-beat

These narrow an ADR's even-handed option list to a reference default a downstream agent must either
adopt or write down why it deviated (they do **not** foreclose the alternatives in the same class):

- **JetStream-class durable streaming log with durable PULL consumers + consumer groups** is the
  reference default for the durable bus transport (02 §3). Narrows ADR-04's even-handed
  Kafka/JetStream/PG-outbox list to *"a durable streaming log with durable pull consumers"* as the
  class; non-durable fire-and-forget pub/sub is wrong; PG-outbox is acceptable only if it provides the
  same durable-pull/consumer-group semantics. Does not foreclose Kafka/Redpanda (same class). → Phase 3
  (Bus).
- **Postgres + recursive CTEs over a dedicated graph DB** for the shallow graphs Refs produces (02 §8).
  Narrows ADR-14's "PG **or** graph-index" to PG-by-default; a graph DB must beat this with a measured
  reason (it buys dual-write sync pain + fragile cross-store transactions — the exact hazard ADR-04
  warns of). Reinforces the TE-7 typed-table-in-PG resolution. → Phase 3 (Refs).
- **The outbox is the ONLY sanctioned emit path** (02 §4): `myelin-events` exposes no fire-and-forget
  emit; a shortcut that exists will be used and lose data. Hardens ADR-04.3's "by default" → "only."
  → Phase 3 + Phase 8 lint.
- **Measure before you shard; read-replicas + pooling first; the authn/authz hot path is the likely
  first replica** (02 §8). Names the first concrete scaling need on top of ADR-10's anti-premature-shard
  lean. → Phase 3 (Storage + Id).
- **Thin, visible SQL over a heavy ORM** ("an ORM hides the data model you most need to see") (02 §8). A
  low-controversy data-access default. → Phase 3 bootstrap convention.
- **Forward-only, online migrations** (no rollback migrations; expand→backfill→contract; no blocking
  `ALTER` on a hot table; measure lock time against a restore first) (02 §8). Net-new substrate law.
  → ADR-10 note + Phase 3 + Phase 8 CI check.
- **Three-surface service topology** (public gateway / internal RPC / metrics-health) with the
  public/internal split as a *security boundary*, **liveness ≠ readiness**, and a **shared bootstrap
  harness** (config/DB/migrations/outbox-publisher/telemetry/the three ports in one call) so a new
  service is a thin shell over identical plumbing (02 §9). → ADR-01 crate table + Phase 3 + Phase 6
  (early platform-capability).

---

## (f) What we deliberately do NOT change, and why

- **The 14-ADR spine stands unaltered.** No existing ADR's *decision* is reversed; the doctrine
  confirms it. We only **append** ADR-16…ADR-20 and add cross-reference notes. (Honours the phase
  rule: do not alter committed decisions, only append and cross-reference.)
- **No new VISION non-negotiable principle.** Substrate "how" does not rise to VISION-principle status;
  VISION already states the "what" (world-scale, GDPR-by-construction, agent-native, top-tier UX). We
  add only: (a) external-insights as canonical doctrine, and (b) the new phase 2b in the §5 list, plus
  the small §3 honesty clauses (code-wins, name-your-floors) that are the *same class* as the existing
  §3 invariants.
- **We do not pre-decide the genuinely-open design spaces** the doctrine itself leaves open (the
  collaborative editor's final engine, per-subsystem storage, UX token *values*, the chat connection
  tier). The doctrine's specificity contract matches our DECIDED / OPEN→PN ladder; we hand
  *defaults-to-beat*, not foreclosures, into Phase 3/4.
- **We do not collapse the two HITL notions** (build-time human sign-off vs runtime agent gates) — see
  §(d).
- **Casual-mention auto-spawn stays a product/cost decision, not an engineering default.** Ship the
  explicit "run an agent here" action first; gate implicit auto-dispatch behind a deliberate,
  intent/cost-aware, DPO-aware (Art. 22) decision. Our flagship walkthroughs assumed auto-wake; that
  assumption is corrected to *explicit-first*. (03 §7 → Commercial/Product + Phase 4 Chat + Phase 6.)
- **Git-history erasure is NOT treated as solved.** We explicitly stop ADR-12 §4 from *implying* the
  spine solves erasure: pseudonymous-by-default commit identities are a **commit-time prerequisite**,
  not a retrofit; the only levers for non-pseudonymised history are history-rewrite (changed hashes,
  disruptive) or a documented lawful-basis limit — none free. This becomes a named Phase-3 reconciliation
  deliverable, co-owned with Legal/DPO, gating the Git P4 data model. (04 §1 → ADR-12 note +
  directives.)

---

## Cross-references
- Doctrine: [`external-insights/`](../../external-insights/) (README + 01–05).
- Spine: [`architecture-decisions.md`](../02-holistic-architecture/architecture-decisions.md)
  (ADR-01…ADR-20 after this phase's append),
  [`design-language.md`](../02-holistic-architecture/design-language.md) (now with §11).
- Analyses: [`analysis/`](./analysis/) (five per-doc integration analyses).
- Hand-off: [`integration-directives.md`](./integration-directives.md) — the binding per-destination
  directives. [`README.md`](./README.md) — this phase's index.
