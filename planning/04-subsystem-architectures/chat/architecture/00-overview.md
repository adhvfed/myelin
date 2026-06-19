# Chat — 00 · Overview, Role & Responsibilities

> Phase: `04-subsystem-architectures/chat`, **Stage 2 of 2 — detailed architecture**. Canonical brief:
> [`VISION.md`](../../../../VISION.md) (never contradicted). Doctrine (binding):
> [`EI-03`](../../../../external-insights/03-agent-native-fabric.md) (agent-native),
> [`EI-04`](../../../../external-insights/04-hard-problems.md) (the hard-problem ladder),
> [`EI-05`](../../../../external-insights/05-ux-and-design.md) (UX is non-negotiable). Binding directives:
> [`integration-directives.md`](../../../02b-doctrine-integration/integration-directives.md) Phase-4 **CHAT-1**
> + SUB-X + X-1…X-5; [`decision-record.md`](../../../02b-doctrine-integration/decision-record.md) §(f).
> Phase-3 build-to surface: [`contract-index.md`](../../../03-shared-systems-architecture/contract-index.md) +
> the three foundational docs ([`00-platform-substrate`](../../../03-shared-systems-architecture/00-platform-substrate.md),
> [`identity-and-access`](../../../03-shared-systems-architecture/identity-and-access.md),
> [`event-bus`](../../../03-shared-systems-architecture/event-bus.md)). Wave-A dependency:
> [`knowledge-platform/architecture`](../../knowledge-platform/architecture/) (the shared `myelin-content`
> block/inline model my messages reuse — "share the AST, not the editor"). My Stage-1:
> [`../sketches/00-findings.md`](../sketches/00-findings.md) + the per-problem sketches + [`../design/`](../design/).
>
> **Status convention** (VISION §3, name-your-floors): *DECIDED* = committed for build/test; *FLOOR* = a
> partial answer shipped with a named follow-on; *[OPEN → P5]* = handed forward. Every property that can fail
> names its **drill** ([`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md)). Dated 2026-06-19.

---

## 0. Reading map (the document split)

| # | Doc | What it owns |
|---|---|---|
| 00 | **this** | Role, responsibilities, owns-vs-delegates, the floors named up front, the component map, the build-order law. |
| 01 | [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) | The language/runtime/DB choice + written justification (the TE-21 connection-tier call); the full data model (conversation, message, read-state, unfurl cache, membership). |
| 02 | [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) | The hard-problem algorithms in depth: the connection tier + NATS backplane + resume-cursor resync; the fanout boundary; the read-state hot path; cheap per-viewer unfurls; erasure. |
| 03 | [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) | The complete `chat.*` event taxonomy (durable vs firehose); every glue contract (`ArtifactRef`/`project`/`replay`, the envelope via the outbox, Id `check`/`list_objects` + the ReBAC fragment, `PersonalDataHolder`, `ToolDef`s, `declare_indexable`, reserve/settle). |
| 04 | [`04-views-cli-and-api.md`](./04-views-cli-and-api.md) | The views (S1–S13, ref the design docs), the CLI surface, the API / agent-tool surface. |
| 05 | [`05-hard-problems.md`](./05-hard-problems.md) | Each subsystem-specific hard problem resolved, with cited prior art and the named floor. |
| 06 | [`06-shared-system-change-requests.md`](./06-shared-system-change-requests.md) | The itemized list of shared-system changes Chat needs from Phase 5 reconciliation. |
| 07 | [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md) | The quantified drills owed + open questions for Phase 5. |

**Floors named up front** (the honest state of v1 — VISION §3):

1. **Message hot tier = Postgres-partitioned; ScyllaDB is the measured promotion** (R-5). The `MessageStore`
   trait makes the hot engine a swap, not a redesign; the cold tier (object segments) is identical either way.
   [01 §3](./01-tech-and-data-model.md), [05 §2](./05-hard-problems.md).
2. **Connection tier = Rust; the BEAM/Phoenix divergence (TE-21) is written-but-disfavoured.** Either way the
   gateway speaks the Rust `EventEnvelope` on the wire and implements `PersonalDataHolder`. The escape hatch is
   a gateway-process swap (the wire contract makes it so), not a platform rewrite. [01 §1](./01-tech-and-data-model.md), [05 §1](./05-hard-problems.md).
3. **Free-text third-party mention erasure is best-effort.** Structured `mention(Principal)` nodes erase for
   free (pseudonym-shred); a person's name typed into the *free text* of someone else's un-erased body is **not**
   surgically erasable — retention + access-control + a documented lawful-basis limit (the chat analogue of
   GD-1). → P4 + LEGAL. [05 §5](./05-hard-problems.md).
4. **Mega-channel live delivery = NATS subject fan-out; the channel-sharded home-node is the measured
   escalation** (R-5). [02 §2](./02-internals-and-algorithms.md), [05 §1](./05-hard-problems.md).
5. **Canvas = an embedded/pinned Knowledge page (`ArtifactRef`), not a Chat-native editor.** Chat references;
   Knowledge authors. Flagged for the joint Chat↔Knowledge review; the lean is firm. [05 §6](./05-hard-problems.md).
6. **Cross-org / federated channels are designed-not-built.** The `Conversation` model does not foreclose them
   (membership may span tenants); they ride the platform's deferred cross-cell PII-free bridge + an explicit
   cross-tenant capability + multi-cell DSR. → P4 control plane + LEGAL. [05 §7](./05-hard-problems.md).
7. **Single home-cell for a tenant's chat.** A globally-distributed user connects to a near edge gateway, but
   writes route to the tenant's home cell. Multi-region edge + the cross-cell bridge is the follow-on. [05 §1](./05-hard-problems.md).

---

## 1. Role & responsibilities

Chat is Myelin's **real-time conversation surface** (VISION §2, subsystem #5 of five) — and, distinctively,
**the most visible surface of the agent-native principle** (Phase-2 Chat §1): the place where humans and
agents converse over the *same* references, where a CI failure becomes a triage thread, where an agent posts a
proposed fix behind a **human-in-the-loop approval gate**, and where the live, permission-aware **unfurl** —
the platform's differentiator (Phase-1 §2.4) — is densest. Chat is not a silo; it is a *participant* in the
reference graph and the one inbox, built almost entirely on the Phase-3 chokepoints.

**The one-paragraph thesis.** *Chat is a careful **consumer** of the Phase-3 shared layer plus four
genuinely-owned hot parts. It owns (a) the **real-time connection tier** — millions of long-lived sockets, a
NATS-core fan-out backplane, and a resume-cursor resync that makes the backplane allowed-to-drop; (b) the
**durable message store** — a per-conversation append log whose body is per-subject-DEK-encrypted (because a
chat body **is** the PII, not a reference to it), partitioned by `(tenant, region)`, tiered to object segments,
behind a `MessageStore` trait; (c) the **read-state hot path** — Valkey hot markers + a Postgres durable record,
eventually-consistent, never authoritative in cache; and (d) **cheap per-viewer unfurls** — a shared-per-ref
projection cache gated by a per-viewer `list_objects`/`check`, lazy-on-viewport, bus-invalidated, calling Refs
`resolve` (it never re-implements permission-aware resolution). On top of those it **is the HITL approval-card
surface** (`Id.check(approve)` → `DurableExecutor::signal(idem_key=card_id)` — the bridge), its
"Activity/Mentions" is a scoped **view** into the one Notif inbox (C-9, never a second store), agent dispatch is
**explicit-first** (CHAT-1 — a mention notifies, it does not auto-spawn a costed run), and every body, mention,
embed, and reaction is a structured `myelin-content` node + an event through the outbox. Chat invents no auth,
reads no other store, and is fully rebuildable from its own source via `replay` — which is what makes it
recoverable and erasure-correct.*

### 1.1 What Chat OWNS (its core competency + its Phase-3 handoff obligations)

From the Phase-3 handoff ([README §5](../../../03-shared-systems-architecture/README.md)) and the Phase-2
ownership table, Chat owns:

- **The `chat.*` taxonomy** (under the Bus §6 grammar) — the complete event list, the durable-vs-firehose
  split ([03 §1](./03-events-contracts-and-glue.md)).
- **The connection tier** — the WebSocket/SSE gateway, the NATS-core fan-out backplane, the
  resume-cursor resync from the durable log (the zero-loss-across-reconnect property). The **most likely Rust
  divergence (TE-21)** — the call is made in writing in [01 §1](./01-tech-and-data-model.md).
- **The durable message store + hot/cold tiering** — the per-conversation append log, k-sortable message ids
  (intrinsic per-conversation order), object-segment cold tier, behind the `MessageStore` trait ([01 §3](./01-tech-and-data-model.md)).
- **The read-state hot path** — per-`(user × conversation)` and per-thread last-read markers + derived unread
  counts; firehose-only events; a `PersonalDataHolder` ([02 §3](./02-internals-and-algorithms.md)).
- **The unfurl card UX, lifecycle, and the shared-per-ref projection cache** — Chat owns the *card* and the
  *cache + invalidation + lazy-on-viewport orchestration*; Refs/Id own the permission decision and the
  projection content ([02 §4](./02-internals-and-algorithms.md)).
- **The HITL approval-card surface** — the withhold→approve→resume bridge renders here; the approval signal
  posts to the durable workflow (`DurableExecutor::signal`, idempotent on `card_id`) ([02 §5](./02-internals-and-algorithms.md)).
- **The conversation model** — one `Conversation` entity, many `kind`s (channel pub/priv, dm, group-dm,
  artifact-linked, announcement); membership-is-the-ACL via ReBAC tuples ([01 §2](./01-tech-and-data-model.md)).
- **Threads** (threads-first with explicit broadcast), agent presence/streaming semantics, the composer over
  `myelin-content`, the `#sub` scheme (`#message-<id>`, `#thread-<root>`).
- **Its half of the glue contracts** — `project(ref, viewer)` for `chat/{channel,message,thread}`,
  `replay(scope, since)`, `declare_indexable`, the ReBAC namespace fragment + the `watcher` relation,
  `PersonalDataHolder`, `ToolDef` registrations, per-surface shed budgets ([03](./03-events-contracts-and-glue.md)).

### 1.2 What Chat DELEGATES to the shared systems (reads no other store — ADR-01)

Chat implements the three glue contracts (ADR-13) and delegates everything cross-cutting. It **reads no other
subsystem's store** (`no-cross-db` lint, ADR-01); it interacts only through the contracts.

| Concern | Delegated to | The contract Chat calls / implements |
|---|---|---|
| Identity, channel-membership ACL, agent delegation, approver-set | **Identity** (`myelin-identity`) | `authenticate` / `check` / `list_objects` / `list_subjects` / `delegation`; Chat **declares its ReBAC fragment** (Id §5) — `channel.read = member + parent_project->read`; no bespoke ACL. |
| Event emission/consumption | **Event Bus** (`myelin-events`) | `OutboxTx::emit(draft, cause)` (the only emit path); the `EventHandler` consumer template; `events::reindex` + Chat's `replay`. |
| Live delivery / presence / typing / read-state transport | **Bus firehose seam** (NATS core) | `firehose::publish/tail`; the durable bus carries only the coarse `chat.read_state.updated` summary (Bus §4.3). |
| **Per-viewer permission-aware unfurls** | **Reference Graph** (`myelin-refs`) | `resolve(ref, viewer, mode) → Projection \| Tombstone` (the non-leaking chokepoint, Refs §4.2). Chat is the **densest producer** of `refs.edge.created` ("discussed in"). |
| Full-text + structured + vector message search | **Search** (`myelin-search`) | `declare_indexable(IndexSpec)`; `query`/`semantic` always conjoined with `list_objects` (the `search-requires-acl-filter` lint). |
| The one inbox (mentions/replies/approvals) | **Notifications** (`myelin-notif`) | declares its `watcher` relation; produces the `chat.message.mentioned` Signal; "Activity/Mentions" is a **filter** over `list_inbox`, **never a second store** (C-9). `humanise` for card/agent-message strings (NOTIF-1). |
| Agent authors/readers/triggers; the cost gate | **Agent Fabric** (`myelin-agent`) | registers `ToolDef`s; agent posts flow through **`EffectApi`** (plan-then-apply, reserves); explicit-first dispatch (CHAT-1). |
| Message bodies / drafts durable storage + erasure | **Storage** (`BlobStore` + KMS) | OLTP message log; object store for cold segments; **per-subject DEK crypto-shred** (GD-4) for bodies/drafts — Chat is the canonical GD-4 case. |
| DSR / erasure / audit / retention | **GDPR/Audit** (`myelin-gdpr`) | implements `PersonalDataHolder`; Chat is the **most PII-dense holder** in Myelin (Phase-2 Chat §8.5). |
| The HITL durable wait/timer/resume | **Durable-workflow** (`myelin-flow`) | `DurableExecutor::signal` (the approval bridge); Chat owns the *card*, not the wait/timer/budget. |
| Canvas content/editor | **Knowledge** | a pinned `knowledge/page` `ArtifactRef` embed; Chat references, Knowledge authors (ADR-05; one editor render path). |
| The message content model | **Knowledge (`myelin-content`)** | Chat **consumes** the shared block/inline AST (markdown-subset string + structured `mention`/`artifact_ref`/`embed` nodes); Knowledge **leads** the taxonomy. |

---

## 2. The internal component architecture (at altitude)

A set of mostly-Rust services inside a region-pinned cell (ADR-11), each a thin shell over
`myelin_substrate::serve(AppSpec)` (substrate §3) — **except** the connection-tier gateway, which is Rust by
default but is the single most-justified candidate for a BEAM/Phoenix divergence (TE-21, [01 §1](./01-tech-and-data-model.md));
either way it speaks the Rust envelope on the wire and runs the cross-language harness parity (Sketch 09).

```
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│  CHAT SUBSYSTEM  (Rust services; one region-pinned cell; serve(AppSpec) each)              │
│                                                                                            │
│   clients (WS / SSE / CLI `chat tail`)                                                     │
│        │  (public surface: identity-injected; tenant from token, never path — ID-3)        │
│        ▼                                                                                    │
│  ┌──────────────────────────────────────────────┐                                          │
│  │ CONNECTION-TIER GATEWAY  (TE-21 — Rust default,│  stateless: live sockets + presence +   │
│  │   BEAM escape hatch; wire-envelope either way) │  resume cursors only; NO durable store, │
│  │ · WS/SSE termination · per-conn bounded queue  │  NO outbox of its own (Sketch 09)       │
│  │ · NATS-core subscribe (fan.<tenant>.<channel>) │◄────── firehose (NATS core) ──────┐     │
│  │ · resync from the durable log on (re)connect   │  presence/typing/read/partials    │     │
│  │ · protected-human-lane shed (ADR-16)           │                                   │     │
│  │ · PersonalDataHolder over ephemeral state      │                                   │     │
│  └───────┬──────────────────────────────┬─────────┘                                   │     │
│          │ internal RPC (mTLS)          │ resync range-read                           │     │
│          ▼                              ▼                                             │     │
│  ┌────────────────────┐   ┌──────────────────────┐   ┌────────────────────────┐      │     │
│  │ MESSAGE SERVICE    │   │ READ-STATE SERVICE   │   │ UNFURL SERVICE         │      │     │
│  │ · MessageStore     │   │ · Valkey hot markers │   │ · shared per-ref cache │      │     │
│  │   trait (PG hot →  │   │   + PG durable record│   │ · lazy-on-viewport     │      │     │
│  │   Scylla floor)    │   │ · batched flush      │   │ · refs.resolve caller  │──────┘     │
│  │ · append + range + │   │ · derived unread     │   │ · bus-invalidation     │            │
│  │   tombstone + resync│  │ · firehose-only evts │   │   consumer             │            │
│  │ · OUTBOX in same tx│   └──────────┬───────────┘   └───────────┬────────────┘            │
│  │ · #sub minting     │              │                           │                          │
│  └───────┬────────────┘              │                           │                          │
│          │                           │                           │                          │
│  ┌───────▼────────────┐   ┌──────────▼───────────┐   ┌───────────▼────────────┐            │
│  │ CONVERSATION /     │   │ HITL CARD SERVICE    │   │ INDEXING / OUTBOX feeder│            │
│  │ MEMBERSHIP SERVICE │   │ · render approval card│  │ · events → Bus/Search/  │            │
│  │ · Conversation/kind│   │ · Id.check(approve)  │   │   Refs/Notif (coalesced)│            │
│  │ · membership→ReBAC │   │ · DurableExecutor::  │   │ · declare_indexable     │            │
│  │   tuple writes     │   │   signal(idem=card)  │   │ · project / replay      │            │
│  │ · retention hook   │   │ · also → Notif inbox │   │                         │            │
│  └───────┬────────────┘   └──────────────────────┘   └─────────────────────────┘            │
│          │                                                                                  │
│  ┌───────▼───────────┐  ┌────────────────────┐  ┌──────────────────────────────┐           │
│  │ GDPR holder       │  │ Storage adapter    │  │ Agent / Tool adapter          │           │
│  │ (locate/export/   │  │ (OLTP log; object  │  │ (ToolDefs via EffectApi;      │           │
│  │ rectify/restrict/ │  │ cold segments; KMS │  │ explicit-first dispatch;      │           │
│  │ erase; per-subject│  │ per-subject DEK)   │  │ streaming partials; presence) │           │
│  │ crypto-shred)     │  │                    │  │                               │           │
│  └───────────────────┘  └────────────────────┘  └──────────────────────────────┘           │
└──────────────────────────────────────────────────────────────────────────────────────────┘
     │ authz       │ events/outbox   │ refs        │ search    │ notif   │ gdpr   │ flow   │ agent
     ▼             ▼                 ▼             ▼           ▼         ▼        ▼        ▼
  Identity      Event Bus      Reference Graph  Search    Notif    GDPR/Audit Workflow Agent Fabric
```

**The components, one line each** (detail in [01](./01-tech-and-data-model.md)/[02](./02-internals-and-algorithms.md)):

1. **Connection-tier gateway** — holds the live WebSocket/SSE sockets, subscribes to NATS-core
   `fan.<tenant>.<channel>` subjects for the channels its connections are in, resyncs the gap from the durable
   log on (re)connect, enforces the protected-human-lane shed order (ADR-16). **Stateless** (no durable store,
   no outbox); the most-justified TE-21 divergence; speaks the Rust envelope on the wire regardless ([01 §1](./01-tech-and-data-model.md)).
2. **Message Service** — authority for the durable per-conversation log behind the `MessageStore` trait
   (`append`/`range`/`tombstone`/`resync_from`); persists the message **and** the `chat.message.created` outbox
   row in **one transaction** (BUS-2 — no dual-write); mints `#sub` ids stable across edits ([01 §3](./01-tech-and-data-model.md)).
3. **Read-state Service** — the churny hot path: Valkey hot markers + counters, batched eventually-consistent
   flush to the PG durable record (STOR-3: Valkey never authoritative); derives unread as a bounded range read;
   emits only firehose `chat.read_state.updated` ([02 §3](./02-internals-and-algorithms.md)).
4. **Unfurl Service** — the Chat-owned cache + orchestration in front of Refs `resolve`: a shared, per-`ArtifactRef`
   projection cache (viewer-independent) gated by a per-viewer `list_objects`/`check`, lazy-on-viewport,
   bus-invalidated on `*.updated`/`*.erased` pointer events ([02 §4](./02-internals-and-algorithms.md)).
5. **Conversation / Membership Service** — the one `Conversation` entity (+`kind`), membership compiled to
   ReBAC tuples (`write_tuples`), the retention-policy hook, the artifact-linked-channel `refs.edge.created`.
6. **HITL Card Service** — renders the approval card, gates the click with `Id.check(human, approve, run)`,
   posts `DurableExecutor::signal(run, name, payload, idem_key=card_id)` (the bridge), and lands the card in the
   Notif inbox too (C-9) ([02 §5](./02-internals-and-algorithms.md)).
7. **Indexing / Outbox feeder** — writes events to the transactional outbox in the same DB transaction as the
   state change; coalesces; implements `declare_indexable`, `project`, `replay`.
8. **GDPR holder** — `locate/export/rectify/restrict/erase` over every Chat store; per-subject DEK crypto-shred
   for bodies/drafts; structured-mention pseudonym-shred; honours the restriction flag ([02 §6](./02-internals-and-algorithms.md)).
9. **Storage adapter** — OLTP message log + outbox; object store for cold segments; residency-pinned;
   per-tenant envelope-encryption with **per-subject DEKs for free-text bodies** (GD-4).
10. **Agent / Tool adapter** — registers Chat `ToolDef`s; routes agent posts through `EffectApi` (reserves);
    explicit-first dispatch; streams partials on the firehose; derives agent presence from fabric health.

The **channel-membership ReBAC tuples are NOT Chat's component** — they live in Id's tuple store; Chat only
*projects* into them via `write_tuples`. All derived state (read-state counts, the unfurl cache, the Search
index) is rebuildable by reindex-from-source.

---

## 3. The build-order law (R1 / R3 — what is sequenced first)

Per the roadmap sequencing law (R1: "order by what kills you first — silent data-loss floors before any feature
surface") and the doctrine floor for any real-time relay (EI-04 §2.2: "a relay *without* resume cursors will
silently lose the gap on a reconnect"):

1. **The durable message store + the outbox co-commit (BUS-2).** Before any live delivery, before any UI: a
   message and its `chat.message.created` event commit in one transaction. The no-dual-write guarantee is item 0
   — getting it wrong is the silent-data-loss class. The `MessageStore` trait + the cold-tier seam ship here.
2. **The resume-cursor resync from the durable log.** The fan-out backplane is *allowed to drop* only because
   the resume cursor recovers the gap (Sketch 01). The **zero-loss-across-reconnect drill** is this layer's gate
   ([07](./07-drills-and-open-questions.md)). Built before the backplane is trusted.
3. **The connection-tier gateway + the NATS-core backplane** — live delivery on top of (1)+(2). The
   protected-human-lane shed order (ADR-16) and the per-surface shed budgets ship with it.
4. **The unfurl service** — the wedge differentiator, on top of the Refs `resolve` chokepoint; the
   **unfurl-no-leak** and **unfurl-erasure-safe** drills are its gates.
5. **The read-state hot path** — Valkey + PG batched flush; eventually-consistent; STOR-3-honouring.
6. **The HITL approval-card bridge + the Activity-as-view** — Chat's two named platform obligations, on top of
   `DurableExecutor::signal` and `Notif.list_inbox`.
7. **Agent presence/streaming + explicit-first dispatch** — built and proven against the mock runtime (D6).

This file is the map; the substance is in [01](./01-tech-and-data-model.md)–[07](./07-drills-and-open-questions.md).
