# Chat — 00 · Overview, Role & Responsibilities

> Phase: `04-subsystem-architectures/chat`, **rewritten in Phase 5-B against the RECONCILED shared layer**.
> Canonical brief: [`VISION.md`](../../../../VISION.md) (never contradicted). Doctrine (binding):
> [`EI-03`](../../../../external-insights/03-agent-native-fabric.md) (agent-native),
> [`EI-04`](../../../../external-insights/04-hard-problems.md) (the hard-problem ladder),
> [`EI-05`](../../../../external-insights/05-ux-and-design.md) (UX is non-negotiable).
> **The FROZEN build-to surface this rewrite conforms to (no drift):**
> [`05/contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md) +
> [`05/00-reconciliation-decisions.md`](../../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
> (rationale for every shape). The refined shared docs most load-bearing for Chat:
> [`event-bus`](../../../05-refined-shared-systems-architecture/event-bus.md) (the firehose resume-cursor protocol),
> [`notifications`](../../../05-refined-shared-systems-architecture/notifications.md) (the one inbox),
> [`agent-fabric`](../../../05-refined-shared-systems-architecture/agent-fabric.md) (explicit-first + the four guarantees),
> [`durable-workflow`](../../../05-refined-shared-systems-architecture/durable-workflow.md) (per-effect `idem_key`),
> [`reference-graph`](../../../05-refined-shared-systems-architecture/reference-graph.md) (the `#sub` ladder).
> The Phase-4 **design record is PRESERVED**: [`../sketches/`](../sketches/) + [`../design/`](../design/).
>
> **Status convention** (VISION §3, name-your-floors): *DECIDED* = committed for build/test; *FLOOR* = a
> partial answer shipped with a named follow-on; *[OPEN → P6]* = handed forward. Every property that can fail
> names its **drill** ([`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md)). Dated 2026-06-19.

---

## 0. Changes vs the Phase-4 first pass (the reconciliation deltas absorbed)

This rewrite **carries forward the sound Phase-4 design** (the four owned hot parts, the `MessageStore` trait,
the crypto-shred erasure triad, explicit-first dispatch, the Activity-as-view) and **adjusts only where
reconciliation froze a contract shape Chat had assumed open**. Nothing here reverses an ADR; nothing drifts
from the frozen index. The exact deltas:

| # | What changed | Why (the reconciliation decision) | Where it lands |
|---|---|---|---|
| Δ1 | **The resume-cursor resync is now the FROZEN firehose `subscribe/resume/scope` protocol** (`subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)` / per-`(stream,scope)` monotonic `seq` / `resync_required` → `*.snapshot` fallback / **bounded scope, never `*`**). Phase 4 described an ad-hoc resync from the durable log; that is now the platform's one protocol, co-designed once for ISS boards / KN docs / Chat channels. | OQ-J, contract 3.5 (`event-bus.md` §C4/§OQ-J) | [02 §1](./02-internals-and-algorithms.md) rewritten to the frozen verbs; the per-view scope bounding is now explicit (a hot channel paginates its `scope = channel:<id>`). |
| Δ2 | **The `#sub` grammar + tombstone ladder are FROZEN.** Chat mints `message-<opaqueid>` and `thread-<opaqueid>` from the frozen vocabulary; Refs stores the full sub-URN **and** the `#sub`-stripped root; resolution degrades through the **one 4-step ladder** (permission → root → sub-resolve {live/moved/outdated/gone} → erased). Phase 4 used `#message-<id>`/`#thread-<root>`; the prefix is now the frozen `message-`/`thread-` kind. | X-4/OQ-D, contract 5.7 | [03 §2](./03-events-contracts-and-glue.md); the unfurl tombstone path ([02 §4](./02-internals-and-algorithms.md)) uses the frozen ladder. |
| Δ3 | **The `myelin-content` Chat subset is FROZEN explicitly.** Chat consumes `paragraph, heading(1..3), bullet_list, ordered_list, task_list, blockquote, code_block, callout, table, divider, image` + all three inline nodes (`mention`/`artifact_ref`/`embed`); it **excludes** `db_view, sync_block, toggle`; per-message CAS on edit, **no collaborative-edit engine**. Phase 4 said "consumes the AST"; the subset is now named to the node. | X-2/OQ-B, contract 13.1 | [01 §1.3](./01-tech-and-data-model.md), [04 §1](./04-views-cli-and-api.md). |
| Δ4 | **`list_objects` returns the FROZEN `SetExpr` push-down.** The unfurl membership-class precompute and the Search ACL conjoin lower `SetExpr` (`Ids`/`InRelation{relation, via_column}`/`TupleSet`) to a JOIN over Chat's own id column (`channel.id` / `message.id`) against Id's per-tenant authz reverse index — no N+1, no post-filter. Read-fanout uses `list_subjects(channel, watcher)` against the same index. | OQ-E, contracts 4.3/4.4 | [02 §4.3](./02-internals-and-algorithms.md), [03 §7](./03-events-contracts-and-glue.md). |
| Δ5 | **The per-effect `idem_key` rule is FROZEN.** A single-effect card signals `idem_key = card_id`; a multi-effect card signals `idem_key = card_id:<effect_idx>` per effect, each idempotent, each mapping to exactly one `EffectApi::apply` (a declined effect is **withheld**). | OQ-F, contracts 9.1/9.4 | [02 §5](./02-internals-and-algorithms.md). |
| Δ6 | **The erasure residual is now ONE platform posture, instantiated BY REFERENCE.** Chat no longer restates its own free-text-third-party residual; it points at the platform posture (contract **10.9**, recon §X-7) and supplies only the structural floor (per-subject DEK crypto-shred + pseudonym-map shred + `restrict`). | X-7/OQ-G, contract 10.9 | [02 §6](./02-internals-and-algorithms.md), [05 §5](./05-hard-problems.md), [06](./06-reconciliation-compliance.md). |
| Δ7 | **Threading consolidation is a NAMED floor (OQ-L).** Chat owns conversation-threads; Knowledge/Issues own document-anchored comment threads; they share the **same `#thread-`/`#comment-` `#sub` grammar + the same `myelin-content` AST + the same `refs.edge.created`**, so the future consolidation is a store/transport swap, not a rewrite. | OQ-L (incl. X-5), contract 5.7/13.1/7.3 | [05 §6](./05-hard-problems.md). |
| Δ8 | **File 06 is rewritten** from "shared-system change requests" to **`06-reconciliation-compliance.md`** — *how Chat now implements the frozen contracts* (CheckStatus-consumer posture, `myelin-content`, the `SetExpr` filter, the `#sub` grammar, the erasure posture by reference, per-effect `idem_key`, the firehose protocol) plus any RESIDUAL request for Phase 6. | the prompt's rename directive | [06](./06-reconciliation-compliance.md). |

Everything else — the Rust-default connection tier (TE-21), the Postgres-partitioned `MessageStore` with the
Scylla floor (R-5), the Valkey+PG read-state, cheap per-viewer unfurls, the conversation model, the ReBAC
fragment, the shed budgets, the cross-org/cross-cell floor — is **carried forward unchanged**; reconciliation
*confirmed* those Phase-4 calls (the per-surface shed budgets are now a named floor in OQ-K; the cross-cell
bridge frame is frozen in OQ-I; the explicit-first dispatch is pinned in CHAT-1).

---

## 1. Reading map (the document split)

| # | Doc | What it owns |
|---|---|---|
| 00 | **this** | Role, responsibilities, owns-vs-delegates, the reconciliation deltas, the floors named up front, the component map, the build-order law. |
| 01 | [`01-tech-and-data-model.md`](./01-tech-and-data-model.md) | The language/runtime/DB choice + justification (the TE-21 call, carried forward + confirmed); the full data model (conversation, message + `MessageStore` trait + tiering, read-state, unfurl cache, membership, the frozen `myelin-content` subset). |
| 02 | [`02-internals-and-algorithms.md`](./02-internals-and-algorithms.md) | The hard-problem algorithms: the connection tier + the **frozen firehose resume-cursor protocol**; message-store tiering; the read-state hot path; cheap per-viewer unfurls; the HITL bridge + the per-effect `idem_key` + Activity-as-view; the erasure cascade; agent presence/streaming/explicit-first dispatch. |
| 03 | [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) | The complete `chat.*` event taxonomy (durable vs firehose); every glue contract implemented against the **frozen shapes** (`ArtifactRef`/`project`/`replay`, the envelope via the outbox, Id `check`/`list_objects` `SetExpr` + the ReBAC fragment, `PersonalDataHolder`, `ToolDef`s, `declare_indexable`, reserve/settle). |
| 04 | [`04-views-cli-and-api.md`](./04-views-cli-and-api.md) | The views (S1–S13, ref [`../design/`](../design/)), the CLI surface, the API / agent-tool surface. |
| 05 | [`05-hard-problems.md`](./05-hard-problems.md) | Each subsystem-specific hard problem resolved, with cited prior art and the named floor. |
| 06 | [`06-reconciliation-compliance.md`](./06-reconciliation-compliance.md) | How Chat **implements** the frozen reconciled contracts + any residual request for Phase 6. |
| 07 | [`07-drills-and-open-questions.md`](./07-drills-and-open-questions.md) | The quantified drills owed + open questions for Phase 6. |

**Floors named up front** (the honest state of v1 — VISION §3):

1. **Message hot tier = Postgres-partitioned; ScyllaDB is the measured promotion** (R-5). The `MessageStore`
   trait makes the hot engine a swap, not a redesign; the cold tier (object segments) is identical either way.
   [01 §1.2](./01-tech-and-data-model.md), [05 §2](./05-hard-problems.md).
2. **Connection tier = Rust; the BEAM/Phoenix divergence (TE-21) is written-but-disfavoured.** Either way the
   gateway speaks the Rust `EventEnvelope` on the wire, implements `PersonalDataHolder`, and satisfies the
   **frozen cross-language harness shim** (contract 1.7). The escape hatch is a gateway-process swap (the wire
   contract makes it so), not a platform rewrite. [01 §1.1](./01-tech-and-data-model.md), [05 §1](./05-hard-problems.md).
3. **Free-text third-party mention erasure is best-effort, handled per the ONE platform posture** (contract
   10.9, recon §X-7) — **not restated here**. Structured `mention(Principal)` nodes erase for free
   (pseudonym-map shred); a person's name typed into the *free text* of someone else's un-erased body is the
   platform's named residual. [05 §5](./05-hard-problems.md), [06](./06-reconciliation-compliance.md).
4. **Mega-channel live delivery = the firehose resume-cursor tier (NATS); the channel-sharded home-node is the
   measured escalation** (R-5). [02 §2](./02-internals-and-algorithms.md), [05 §1](./05-hard-problems.md).
5. **Canvas = an embedded/pinned Knowledge page (`ArtifactRef`), not a Chat-native editor.** Chat references;
   Knowledge authors. Flagged for the joint Chat↔Knowledge review; the lean is firm. [05 §6](./05-hard-problems.md).
6. **Document-anchored comment threading consolidation is named, not built** (OQ-L): Chat owns conversation
   threads, Knowledge/Issues own anchored comment threads, **over one shared `#sub`/content/refs scheme**, so
   consolidation later is a merge, not a rewrite. [05 §6](./05-hard-problems.md).
7. **Cross-org / federated channels are designed-not-built.** The `Conversation` model does not foreclose them;
   they ride the **frozen cross-cell PII-free pointer bridge** (contract 12.6, OQ-I) + an explicit cross-tenant
   capability + multi-cell DSR. → P6 control plane + LEGAL. [05 §7](./05-hard-problems.md).
8. **Single home-cell for a tenant's chat.** A globally-distributed user connects to a near-edge gateway, but
   writes route to the tenant's home cell. Multi-region edge + the cross-cell bridge is the follow-on. [05 §1](./05-hard-problems.md).

---

## 2. Role & responsibilities

Chat is Myelin's **real-time conversation surface** (VISION §2, subsystem #5 of five) — and, distinctively,
**the most visible surface of the agent-native principle**: the place where humans and agents converse over the
*same* references, where a CI failure becomes a triage thread, where an agent posts a proposed fix behind a
**human-in-the-loop approval gate**, and where the live, permission-aware **unfurl** — the platform's
differentiator (Phase-1 §2.4) — is densest. Chat is not a silo; it is a *participant* in the reference graph
and the one inbox, built almost entirely on the reconciled chokepoints.

**The one-paragraph thesis.** *Chat is a careful **consumer** of the reconciled shared layer plus four
genuinely-owned hot parts. It owns (a) the **real-time connection tier** — millions of long-lived sockets over
the **frozen firehose resume-cursor protocol** (`subscribe/resume/scope`), a NATS backplane, and the
resume-cursor resync that makes the backplane allowed-to-drop; (b) the **durable message store** — a
per-conversation append log whose body is per-subject-DEK-encrypted (because a chat body **is** the PII, not a
reference to it), partitioned by `(tenant, region)`, tiered to object segments, behind a `MessageStore` trait;
(c) the **read-state hot path** — Valkey hot markers + a Postgres durable record, eventually-consistent, never
authoritative in cache; and (d) **cheap per-viewer unfurls** — a shared-per-ref projection cache gated by a
per-viewer `list_objects`/`check` (lowering the frozen `SetExpr`), lazy-on-viewport, bus-invalidated, calling
Refs `resolve` (it never re-implements permission-aware resolution). On top of those it **is the HITL
approval-card surface** (`Id.check(approve)` → `DurableExecutor::signal` with the **frozen per-effect
`idem_key`**), its "Activity/Mentions" is a scoped **view** into the one Notif inbox (C-9, never a second
store), agent dispatch is **explicit-first** (CHAT-1 — a mention notifies, it does not auto-spawn a costed run),
and every body, mention, embed, and reaction is a structured `myelin-content` node (the frozen Chat subset) + an
event through the outbox. Chat invents no auth, reads no other store, and is fully rebuildable from its own
source via `replay` — which is what makes it recoverable and erasure-correct.*

### 2.1 What Chat OWNS (its core competency + its handoff obligations)

- **The `chat.*` taxonomy** (under the Bus §6 grammar) — the complete event list, the durable-vs-firehose split
  ([03 §1](./03-events-contracts-and-glue.md)).
- **The connection tier** — the WebSocket/SSE gateway, the NATS backplane, the **frozen resume-cursor protocol**
  (`subscribe(stream, scope, cursor?)`/`resume`/`resync_required`) with **per-view scope bounding** (the
  zero-loss-across-reconnect property). The **most likely Rust divergence (TE-21)** — the call is carried forward
  in writing in [01 §1.1](./01-tech-and-data-model.md).
- **The durable message store + hot/cold tiering** — the per-conversation append log, k-sortable message ids
  (intrinsic per-conversation order), object-segment cold tier, behind the `MessageStore` trait ([01 §1.2/§3](./01-tech-and-data-model.md)).
- **The read-state hot path** — per-`(user × conversation)` and per-thread last-read markers + derived unread
  counts; firehose-only events; a `PersonalDataHolder` ([02 §3](./02-internals-and-algorithms.md)).
- **The unfurl card UX, lifecycle, and the shared-per-ref projection cache** — Chat owns the *card* and the
  *cache + invalidation + lazy-on-viewport orchestration*; Refs/Id own the permission decision and the
  projection content ([02 §4](./02-internals-and-algorithms.md)).
- **The HITL approval-card surface** — the withhold→approve→resume bridge renders here; the approval signal
  posts to the durable workflow (`DurableExecutor::signal`, the **frozen per-effect `idem_key`**) ([02 §5](./02-internals-and-algorithms.md)).
- **The conversation model** — one `Conversation` entity, many `kind`s (channel pub/priv, dm, group-dm,
  artifact-linked, announcement); membership-is-the-ACL via ReBAC tuples ([01 §2](./01-tech-and-data-model.md)).
- **Threads** (threads-first with explicit broadcast, over the frozen `#thread-` grammar), agent
  presence/streaming semantics, the composer over the **frozen `myelin-content` Chat subset**, the `#sub`
  scheme (`message-<opaqueid>`, `thread-<opaqueid>`).
- **Its half of the glue contracts** — `project(ref, viewer)` for `chat/{channel,message,thread}`,
  `replay(scope, since)`, `declare_indexable`, the ReBAC namespace fragment + the `watcher` relation,
  `PersonalDataHolder`, `ToolDef` registrations, per-surface shed budgets ([03](./03-events-contracts-and-glue.md)).

### 2.2 What Chat DELEGATES to the shared systems (reads no other store — ADR-01)

Chat implements the three glue contracts (ADR-13) and delegates everything cross-cutting. It **reads no other
subsystem's store** (`no-cross-db` lint, ADR-01); it interacts only through the frozen contracts.

| Concern | Delegated to | The contract Chat calls / implements |
|---|---|---|
| Identity, channel-membership ACL, agent delegation, approver-set | **Identity** (`myelin-identity`) | `authenticate` / `check` (with `CaveatContext` where needed) / `list_objects` (the **frozen `SetExpr`**) / `list_subjects` / `delegation`; Chat **declares its ReBAC fragment** (contract 4.9) — `channel.read = member + parent_project->read`; no bespoke ACL. |
| Event emission/consumption | **Event Bus** (`myelin-events`) | `OutboxTx::emit(draft, cause)` (the only emit path); the `EventHandler` consumer template; `events::reindex` + Chat's `replay`. |
| Live delivery / presence / typing / read-state transport | **Bus firehose seam** (the **frozen resume-cursor protocol**, contract 3.5) | `subscribe(stream, scope, cursor?)` / `resume(stream, scope, last_seq)`; the durable bus carries only the coarse `chat.read_state.updated` summary. |
| **Per-viewer permission-aware unfurls** | **Reference Graph** (`myelin-refs`) | `resolve(ref, viewer, mode) → Projection \| Tombstone` (the non-leaking chokepoint, contract 5.2; cross-cell resolution is cell-local, OQ-I). Chat is the **densest producer** of `refs.edge.created`. |
| Full-text + structured + vector message search | **Search** (`myelin-search`) | `declare_indexable(IndexSpec)`; `query`/`semantic` always conjoined with the **frozen `list_objects` `Filter`** (the `search-requires-acl-filter` lint). |
| The one inbox (mentions/replies/approvals) | **Notifications** (`myelin-notif`) | declares its `watcher` relation; produces the `chat.message.mentioned` Signal; "Activity/Mentions" is a **filter** over `list_inbox`, **never a second store** (C-9). `humanise` (the **sole templating surface**, OQ-L) for card/agent-message strings. |
| Agent authors/readers/triggers; the cost gate; the sandbox | **Agent Fabric** (`myelin-agent`) | registers `ToolDef`s (the frozen `requires_approval` defaults, X-6); agent posts flow through **`EffectApi`** (plan-then-apply, reserves); explicit-first dispatch (CHAT-1); inherits the four uniform sandbox guarantees. |
| Message bodies / drafts durable storage + erasure | **Storage** (`BlobStore` + KMS) | OLTP message log; object store for cold segments; **per-subject DEK crypto-shred** (contract 11.4, GD-4) for bodies/drafts — Chat is the canonical GD-4 case. |
| DSR / erasure / audit / retention / **the erasure posture** | **GDPR/Audit** (`myelin-gdpr`) | implements `PersonalDataHolder`; the free-text residual is handled per the **ONE platform posture** (contract 10.9, recon §X-7) **by reference**. Chat is the **most PII-dense holder** in Myelin. |
| The HITL durable wait/timer/resume | **Durable-workflow** (`myelin-flow`) | `DurableExecutor::signal` (the approval bridge, the **frozen per-effect `idem_key`**); Chat owns the *card*, not the wait/timer/budget. |
| Canvas content/editor | **Knowledge** | a pinned `knowledge/page` `ArtifactRef` embed; Chat references, Knowledge authors (ADR-05; one editor render path). |
| The message content model | **Knowledge (`myelin-content`)** | Chat **consumes** the **frozen Chat subset** of the block/inline AST (markdown-subset string + structured `mention`/`artifact_ref`/`embed` nodes); Knowledge **leads** the taxonomy (X-2). |

---

## 3. The internal component architecture (at altitude)

A set of mostly-Rust services inside a region-pinned cell (ADR-11), each a thin shell over
`myelin_substrate::serve(AppSpec)` — **except** the connection-tier gateway, which is Rust by default but is the
single most-justified candidate for a BEAM/Phoenix divergence (TE-21); either way it speaks the Rust envelope on
the wire and satisfies the **frozen cross-language harness shim** (contract 1.7).

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
│  │ · WS/SSE termination · per-conn bounded queue  │  NO outbox of its own (contract 1.7)    │
│  │ · firehose subscribe(stream, scope=channel:id) │◄────── firehose resume-cursor tier ┐    │
│  │ · resume(stream, scope, last_seq) on reconnect │  (contract 3.5; presence/typing/   │    │
│  │   → resync_required → *.snapshot fallback      │   read/partials + live delivery)   │    │
│  │ · protected-human-lane shed (ADR-16 / OQ-K)    │                                    │    │
│  │ · PersonalDataHolder over ephemeral state      │                                    │    │
│  └───────┬──────────────────────────────┬─────────┘                                    │    │
│          │ internal RPC (mTLS)          │ resume backfill (last_seq, now]              │    │
│          ▼                              ▼                                              │    │
│  ┌────────────────────┐   ┌──────────────────────┐   ┌────────────────────────┐       │    │
│  │ MESSAGE SERVICE    │   │ READ-STATE SERVICE   │   │ UNFURL SERVICE         │       │    │
│  │ · MessageStore     │   │ · Valkey hot markers │   │ · shared per-ref cache │       │    │
│  │   trait (PG hot →  │   │   + PG durable record│   │ · lazy-on-viewport     │       │    │
│  │   Scylla floor)    │   │ · batched flush      │   │ · refs.resolve caller  │───────┘    │
│  │ · append + range + │   │ · derived unread     │   │ · SetExpr-lowered ACL  │            │
│  │   tombstone + resume│  │ · firehose-only evts │   │ · bus-invalidation     │            │
│  │ · OUTBOX in same tx│   └──────────┬───────────┘   └───────────┬────────────┘            │
│  │ · #sub minting     │              │                           │                          │
│  └───────┬────────────┘              │                           │                          │
│          │                           │                           │                          │
│  ┌───────▼────────────┐   ┌──────────▼───────────┐   ┌───────────▼────────────┐            │
│  │ CONVERSATION /     │   │ HITL CARD SERVICE    │   │ INDEXING / OUTBOX feeder│            │
│  │ MEMBERSHIP SERVICE │   │ · render approval card│  │ · events → Bus/Search/  │            │
│  │ · Conversation/kind│   │ · Id.check(approve)  │   │   Refs/Notif (coalesced)│            │
│  │ · membership→ReBAC │   │ · DurableExecutor::  │   │ · declare_indexable     │            │
│  │   tuple writes     │   │   signal(per-effect  │   │ · project / replay      │            │
│  │ · retention hook   │   │   idem_key)          │   │                         │            │
│  └───────┬────────────┘   │ · also → Notif inbox │   └─────────────────────────┘            │
│          │                └──────────────────────┘                                          │
│  ┌───────▼───────────┐  ┌────────────────────┐  ┌──────────────────────────────┐           │
│  │ GDPR holder       │  │ Storage adapter    │  │ Agent / Tool adapter          │           │
│  │ (locate/export/   │  │ (OLTP log; object  │  │ (ToolDefs via EffectApi;      │           │
│  │ rectify/restrict/ │  │ cold segments; KMS │  │ explicit-first dispatch;      │           │
│  │ erase; per-subject│  │ per-subject DEK)   │  │ streaming partials; presence; │           │
│  │ crypto-shred;     │  │                    │  │ four uniform guarantees X-6)  │           │
│  │ residual → 10.9)  │  │                    │  │                               │           │
│  └───────────────────┘  └────────────────────┘  └──────────────────────────────┘           │
└──────────────────────────────────────────────────────────────────────────────────────────┘
     │ authz       │ events/outbox   │ refs        │ search    │ notif   │ gdpr   │ flow   │ agent
     ▼             ▼                 ▼             ▼           ▼         ▼        ▼        ▼
  Identity      Event Bus      Reference Graph  Search    Notif    GDPR/Audit Workflow Agent Fabric
```

**The components, one line each** (detail in [01](./01-tech-and-data-model.md)/[02](./02-internals-and-algorithms.md)):

1. **Connection-tier gateway** — holds the live WebSocket/SSE sockets, `subscribe`s the firehose at
   `scope = channel:<id>` (bounded, never `*`), `resume`s the gap `(last_seq, now]` on reconnect (falling back to
   `*.snapshot` on `resync_required`), enforces the protected-human-lane shed order (ADR-16/OQ-K). **Stateless**
   (no durable store, no outbox); the most-justified TE-21 divergence; speaks the Rust envelope on the wire and
   satisfies contract 1.7 regardless ([01 §1.1](./01-tech-and-data-model.md)).
2. **Message Service** — authority for the durable per-conversation log behind the `MessageStore` trait
   (`append`/`range`/`tombstone`/`resync_from`); persists the message **and** the `chat.message.created` outbox
   row in **one transaction** (BUS-2 — no dual-write); mints `#sub` ids stable across edits ([01 §3](./01-tech-and-data-model.md)).
3. **Read-state Service** — the churny hot path: Valkey hot markers + counters, batched eventually-consistent
   flush to the PG durable record (Valkey never authoritative); derives unread as a bounded range read; emits
   only firehose `chat.read_state.updated` ([02 §3](./02-internals-and-algorithms.md)).
4. **Unfurl Service** — the Chat-owned cache + orchestration in front of Refs `resolve`: a shared, per-`ArtifactRef`
   projection cache (viewer-independent) gated by a per-viewer `list_objects`/`check` (lowering the frozen
   `SetExpr`), lazy-on-viewport, bus-invalidated on `*.updated`/`*.erased` pointer events ([02 §4](./02-internals-and-algorithms.md)).
5. **Conversation / Membership Service** — the one `Conversation` entity (+`kind`), membership compiled to
   ReBAC tuples (`write_tuples`, returning the zookie to stamp), the retention-policy hook, the
   artifact-linked-channel `refs.edge.created`.
6. **HITL Card Service** — renders the approval card, gates the click with `Id.check(human, approve, run)`,
   posts `DurableExecutor::signal(run, name, payload, idem_key)` with the **frozen per-effect `idem_key`**
   (`card_id` single / `card_id:<effect_idx>` multi), and lands the card in the Notif inbox too (C-9) ([02 §5](./02-internals-and-algorithms.md)).
7. **Indexing / Outbox feeder** — writes events to the transactional outbox in the same DB transaction as the
   state change; coalesces; implements `declare_indexable`, `project`, `replay`; `firehose::publish`es the
   rendered frame to the resume-cursor tier.
8. **GDPR holder** — `locate/export/rectify/restrict/erase` over every Chat store; per-subject DEK crypto-shred
   for bodies/drafts; structured-mention pseudonym-map shred; the residual handled per contract 10.9; honours the
   restriction flag ([02 §6](./02-internals-and-algorithms.md)).
9. **Storage adapter** — OLTP message log + outbox; object store for cold segments; residency-pinned;
   per-tenant envelope-encryption with **per-subject DEKs for free-text bodies** (contract 11.4).
10. **Agent / Tool adapter** — registers Chat `ToolDef`s (the frozen defaults); routes agent posts through
    `EffectApi` (reserves); explicit-first dispatch; streams partials on the firehose; inherits the four uniform
    sandbox guarantees (X-6).

The **channel-membership ReBAC tuples are NOT Chat's component** — they live in Id's tuple store; Chat only
*projects* into them via `write_tuples`. All derived state (read-state counts, the unfurl cache, the Search
index) is rebuildable by reindex-from-source.

---

## 4. The build-order law (R1 / R3 — what is sequenced first)

Per the roadmap sequencing law (R1: "order by what kills you first — silent data-loss floors before any feature
surface") and the doctrine floor for any real-time relay (EI-04 §2.2: "build the durable resume-cursor transport
FIRST; a relay *without* resume cursors silently loses the gap on a reconnect"):

1. **The durable message store + the outbox co-commit (BUS-2).** Before any live delivery, before any UI: a
   message and its `chat.message.created` event commit in one transaction. The no-dual-write guarantee is item 0
   — getting it wrong is the silent-data-loss class. The `MessageStore` trait + the cold-tier seam ship here.
2. **The frozen firehose resume-cursor protocol** (`subscribe/resume/scope`, contract 3.5). The fan-out tier is
   *allowed to drop* only because `resume(stream, scope, last_seq)` recovers the gap `(last_seq, now]`. The
   **zero-loss-across-reconnect drill** is this layer's gate ([07](./07-drills-and-open-questions.md)). Built
   before the backplane is trusted.
3. **The connection-tier gateway** — live delivery on top of (1)+(2), with **per-view scope bounding** (a hot
   channel paginates its `scope`). The protected-human-lane shed order (ADR-16) and the per-surface shed budgets
   (OQ-K) ship with it.
4. **The unfurl service** — the wedge differentiator, on top of the Refs `resolve` chokepoint and the frozen
   `SetExpr` push-down; the **unfurl-no-leak** and **unfurl-erasure-safe** drills are its gates.
5. **The read-state hot path** — Valkey + PG batched flush; eventually-consistent; cache-never-authoritative.
6. **The HITL approval-card bridge + the Activity-as-view** — Chat's two named platform obligations, on top of
   `DurableExecutor::signal` (the per-effect `idem_key`) and `Notif.list_inbox`.
7. **Agent presence/streaming + explicit-first dispatch** — built and proven against the mock runtime (D6).

This file is the map; the substance is in [01](./01-tech-and-data-model.md)–[07](./07-drills-and-open-questions.md).
