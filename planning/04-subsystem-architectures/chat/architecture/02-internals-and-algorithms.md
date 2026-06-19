# Chat — 02 · Internals & Algorithms

> See [`00-overview.md`](./00-overview.md) for framing, [`01-tech-and-data-model.md`](./01-tech-and-data-model.md)
> for the data model. This doc details the subsystem-specific algorithms, conformed to the frozen shapes: the
> connection tier over the **frozen firehose resume-cursor protocol** (§1); message-store tiering + the resume
> read (§2); the read-state hot path (§3); cheap per-viewer unfurls over the **frozen `SetExpr`** + the 4-step
> tombstone ladder (§4); the HITL bridge with the **frozen per-effect `idem_key`** + the Activity-as-view link
> (§5); the erasure cascade with the residual handled **by reference to the ONE platform posture** (§6); agent
> presence/streaming/explicit-first dispatch (§7). Prior art is cited in [`05-hard-problems.md`](./05-hard-problems.md);
> this doc is the *mechanism*.

---

## 1. The connection tier: the frozen firehose resume-cursor protocol (contract 3.5)

### 1.1 Topology

The gateway is **horizontally sharded and stateless** ([01 §1.1](./01-tech-and-data-model.md)). A client
connection (WS primary; SSE/long-poll fallback for restrictive networks + the CLI `chat tail --follow`) lands on
*any* gateway node (a near-edge node for a globally-distributed user; writes route home to the tenant's cell —
the single-home-cell FLOOR, [05 §1](./05-hard-problems.md)). The node:

1. **Authenticates the handshake** via Id `authenticate(credential) → Principal` — the **tenant comes from the
   verified token, never the path** (ID-3). The gateway trusts the injected identity for *identity* and
   re-authorizes nothing it isn't responsible for (per-action authz is the Rust services' job, contract 1.7).
2. **`subscribe`s the frozen firehose protocol** (contract 3.5) for the channels this connection is a member of:
   `subscribe(stream = fan.<tenant>.<channel>, scope = channel:<id>, cursor = last_seq?)`. The **`scope` is a
   bounded selector, never `*`** (the whitelist-not-`*` rule generalised to the firehose, BUS-3). Membership is
   resolved once via `list_objects(viewer, read, channel)` (the frozen `SetExpr`) at connect, refreshed on
   `chat.channel.member_*` events.
3. **Holds a per-connection bounded queue** (the per-connection in-flight frame cap; over-cap sheds in the
   firehose's own bounded queue, and a slow consumer is dropped to `resync_required` rather than buffering
   unboundedly — the OQ-J backpressure rule).

### 1.2 The fan-out backplane — the resume-cursor tier, subject-per-channel

When the Message Service persists a message, the **Indexing/Outbox feeder** also `firehose::publish`es the
**rendered message frame** to `fan.<tenant>.<channel>` (contract 3.5). The frame carries a per-`(stream, scope)`
monotonic `seq`. The transport routes the frame to exactly the gateway nodes holding a subscribed connection;
each node pushes it to the matching sockets. This is **read-fanout per channel** (one ordered log; readers
cursor in) — the backplane just nudges online sockets.

- **Allowed-to-drop by design.** The resume-cursor tier does not durably persist the *live* frame; a frame missed
  during a hiccup is **correct to drop** because the durable log is the source of truth and `resume` recovers it
  (§1.3). This is what makes the backplane cheap.
- **Per-view scope bounding (the head-of-line + cost discipline, OQ-J).** A subscription's `scope = channel:<id>`
  is the *bounded slice the client is looking at*. A huge announcement channel **paginates its scope** (the
  visible window + a margin), so a 100k-member channel does **not** stream 100k live frames to one client. The
  transport rejects an unbounded/over-broad scope.
- **Mega-channel escalation (FLOOR, R-5).** A 100k-member announcement channel = up to 100k gateway-side
  subscribers on one subject — workable under scope bounding, but the **channel-sharded home-node** is the named
  escalation: a measured-hot channel gets a dedicated fan-out home node that gateways pull from, rather than
  100k direct subject subscribers (the Phoenix/Discord guild model, but Rust + consistent-hash). Promotion
  trigger: measured subscriber count exceeding the subject-fan-out budget. Until measured, the subject model is
  the design (ADR-10 anti-premature-shard). [05 §1](./05-hard-problems.md).

### 1.3 The resume-cursor resync — the correctness backbone (zero-loss-across-reconnect)

The per-conversation total-order + no-lost-messages guarantee comes **not** from the backplane but from the
**durable per-conversation log + the frozen `resume` protocol** (the EI-04 §2.2 discipline, now the platform's
one protocol, contract 3.5):

```
on (re)connect, per subscribed conversation:
  client sends its cursor = last_seq it has for (stream, scope=channel:<id>)
  gateway: resume(stream, scope, last_seq)
     → if last_seq is WITHIN the firehose retention window:
          backfill (last_seq, now] from the bounded firehose window, then resume live   # zero loss
     → if last_seq is OLDER than the retention window:
          emit resync_required → the gateway falls back to a *.snapshot replay
          (MessageStore::resync_from(conversation, cursor) — a clustering-range read, ordered, gap-free)
          then resume live
  gateway: stream the gap to the client  (idempotent on message_id — a re-streamed message is a client-side no-op)
```

- A frame lost during a hiccup is recovered by the next `resume` — so the ephemeral backplane is *allowed to
  drop*, which is why it can be cheap. The `resync_required` → `*.snapshot` fallback is the cold-rebuild path,
  **named, not silent** (OQ-J).
- **Idempotency on `message_id`** makes the gap stream + the resumed live stream overlap-safe: a message seen in
  both is deduped client-side. The **send** path is idempotent on `client_nonce` ([01 §3](./01-tech-and-data-model.md))
  so a retried send (flaky mobile/agent) is one message.
- **Drill (the build-order item-0 gate):** sever the gateway↔backplane mid-publish; assert `resume` from the
  retention window (and `resync_required` → snapshot when the window is exceeded) recovers the gap → **0 lost, 0
  duplicate** ([07](./07-drills-and-open-questions.md) D-C1).

### 1.4 The protected-human-lane shed order (ADR-16 / OQ-K; the gateway is *the* edge)

The gateway is where the worst load manifests: connection storms (a deploy reconnect thundering-herd),
mega-channels, agent-generated fan-out (EI-03 §5.4). The protected-human-lane shed order (contract 1.11) applies
here as Chat's **per-surface shed budget**, now a named floor in the OQ-K budget table:

- **Connection-tier (connection-storm) budget:** per-tenant connection cap + per-connection frame cap; reserved
  connection slots for interactive humans; **speculative/presence shed first; message delivery last** (OQ-K row
  "Connection tier (CHAT)").
- **Agent-mention-storm budget:** per-tenant agent-run in-flight cap (reserve/settle refuses over-cap); humans
  never queue behind agent runs (the lane); the agent lane sheds with `429 + Retry-After`, which the agent
  runtime honours (OQ-K row "Agent-mention (CHAT/all)").
- **The gateway honours `Retry-After`** on its own RPC calls (else shedding becomes a retry storm — the
  protected-human-lane defeat).
- **Telemetry survival signals** (contract 1.8): connection count, per-tenant in-flight, shed counts per lane,
  firehose `(stream,scope)` lag, resume-gap size, breaker state — the Phase-5 drills assert against these.
- **Drill:** 30× agent message/connection surge on one tenant → **human latency in budget; agent lane sheds
  (429+Retry-After honoured); other tenants unaffected** ([07](./07-drills-and-open-questions.md) D-C3).

The concrete budget numbers are Chat's P6 call (OQ-K names the floor: "every one of these is bounded, has a
reserved human lane, and applies the shed order"), asserted by the drills.

---

## 2. Message-store tiering + the resume read

### 2.1 The hot/cold lifecycle

The hot tier is the recent tail in Postgres `(tenant, region)` partitions, time-sub-partitioned (monthly RANGE
on the time embedded in `message_id`). A background detach job seals an old sub-partition's range to a
**content-addressed object segment** (encrypted, BLAKE3-addressed, via `BlobStore`) and records the
`(conversation, message_id-range → segment+offset)` mapping in a PG index. Reads are transparent:

```
MessageStore::range(conv, cursor, limit):
  read the hot PG partition first (recent-N / scroll-back — the index range)
  if the cursor falls into a DETACHED range:
     look up the cold segment(s), fetch + decrypt (per-tenant DEK), range-read within → merge ordered
```

- **Why object segments for cold (not a third engine):** infinite per-conversation growth is the case that
  melts a single PG table. Sealing cold ranges to immutable encrypted segments keeps the hot tier bounded; the
  segment is still range-readable and still crypto-shreddable (destroy the key). This is the cold tier only, not
  the v1 hot store.
- **Crypto-shred inside a segment:** a segment is encrypted under the per-tenant DEK; the *bodies* inside it are
  additionally per-subject-DEK-encrypted (the body ciphertext is double-wrapped). Erasing a person destroys their
  per-subject DEK → their bodies inside any segment become unrecoverable **without rewriting the immutable
  segment** (§6). This is the whole reason crypto-shred (not delete) is the erasure mechanism (contract 11.4).

### 2.2 Per-conversation order at scale

`message_id` ULID gives intrinsic order; appends are tail-writes; the `UNIQUE(conv, client_nonce)` constraint
plus the ULID make burst-sends + edits to a hot channel preserve per-conversation total order (the per-aggregate
ordering the bus assumes, contract 2.3 / the D-9 drill). Causal anomalies on the client (an edit/reaction
arriving before its target message — possible over the live backplane) are reconciled to the stable `message_id`
order: the client buffers an out-of-order op until its target arrives, or on `resume` re-orders to the durable
sequence. **Drill:** burst sends + edits to one hot channel → **per-conversation total order preserved; resume
gap-free** ([07](./07-drills-and-open-questions.md) D-C2).

---

## 3. The read-state hot path (Valkey + PG, eventually-consistent; Sketch 03)

```
on scroll/open (the high-frequency write):
  Valkey HSET read:<t>:<p>:<conv> = last_read_message_id        # debounced (coalesce rapid scrolls), in-memory
  firehose::publish chat.read_state.updated (coarse)            # ephemeral; NOT the durable bus (ADR-04.5)
batched flush (cadence ~seconds, debounced):
  UPSERT read_state(...)                                        # PG durable record (this is the truth)
on cache loss (reconstruction):
  the PG record is authoritative; a marker is at-worst slightly stale → you re-see a few read messages (benign, bounded)
unread count (DERIVED, never authoritative):
  count(message_id > read_state.last_read[conv])               # a bounded range read against the log, cached
```

- **Valkey is never the source of truth.** The PG `read_state` record is; Valkey is a write-back cache. A cache
  loss never loses correctness, only freshness, and the failure is *benign and bounded*.
- **Eventually-consistent is acceptable** for read-state: "delivered/seen on one device → eventually seen
  everywhere" (cross-device truth).
- The link to Notif's inbox read-state is at the **mention** (§5.3), not duplicated.

---

## 4. Cheap per-viewer unfurls (the wedge; Sketch 04)

The unfurl service is a **Chat-owned cache + orchestration layer in front of Refs `resolve`** — it does **not**
re-implement permission-aware resolution. Contract 5.2 is the non-leaking chokepoint; Chat's job is to make the
per-viewer call *cheap* at chat density. The layered cheapening, in order:

### 4.1 Lazy-on-viewport (the single biggest cost-killer)

A virtualised timeline (design wireframes S2) resolves unfurls **only for messages currently in the viewport**.
A scroll-back of 10,000 messages resolves a handful of cards, not 10,000. The naïve "resolve every ref in the
channel" is the trap; lazy-on-viewport defeats it.

### 4.2 Split the cache by what varies per-viewer vs. what doesn't, over the 4-step tombstone ladder

```
for each artifact_ref / embed node in a VISIBLE message, per viewer:
  decision = Id.check(viewer, view, ref)         # the PER-VIEWER part: fast, cached, authz-reverse-index-prefiltered
  if Deny:  render TOMBSTONE ("a restricted <type>")   # the title NEVER leaks (contract 5.2; ADR-03; ladder step 1)
  else:
    proj = unfurl:proj:<ref>  (Valkey)           # the VIEWER-INDEPENDENT projection content — cached ONCE per ref
            ?? refs.resolve(ref, viewer, Display) # cache miss → Refs → owner.project(ref, viewer) via resilient client
    render per the 4-step ladder (contract 5.7):
       LIVE      → live card (title/state/icon/actions)
       MOVED     → card + `moved` flag        (e.g. a referenced KN block moved)
       OUTDATED  → partial card + `outdated` flag
       GONE      → Tombstone{root} ("this referenced <parent> (the specific part is no longer available)")
       ERASED    → Tombstone{erased} ("[erased]")
```

The insight (mirroring contract 5.2): **projection content is viewer-independent** — cache it once per
`ArtifactRef`, short-TTL, bus-busted. **The permission decision is per-viewer** — but it is a `check`/`list_objects`,
the platform's fast primitive — and content is returned **only after the per-viewer check passes**, so there is
**one shared cache entry per ref, never one per `(ref, viewer)`**, with no leak. A sub-anchored ref
(`...#message-<id>`, `...#b<id>`, `...#L42-L88`) degrades through the **one frozen 4-step ladder** (contract
5.7) — the tombstone **always carries the root**, so a broken sub-anchor still resolves to the parent.

### 4.3 Membership-as-permission class precompute (the frozen `SetExpr` push-down, OQ-E)

For a **public** channel in a project, "can a channel member see this project artifact?" is often a single coarse
class, not 500 checks: `list_objects(viewer, view, type)` returns a **`Filter{set_expr, zookie}`** (contract
4.3). The unfurl service **lowers the `SetExpr`** (`Ids`/`InRelation{relation, via_column}`/`TupleSet`) to a SQL
predicate / JOIN over the unfurl candidate-id column against Id's **per-tenant authz reverse index** — no N+1,
no post-filter. For a **private** channel whose membership ≈ the visibility class, often *all members see the
same artifacts* — one class decision, not N. The returned `zookie` bounds staleness; a `member_*` event triggers
a refresh of the class.

### 4.4 Bus-driven invalidation (precise; TTL is the backstop)

The Unfurl Service runs a consumer (substrate template, whitelisted subjects, contract 2.4) on the artifact
pointer events — `issue.*.updated`, `git.pr.*` + **`ci.check.updated`** (the frozen CheckStatus event, X-1),
`ci.run.*`, `knowledge.*.updated`, and crucially `*.erased` and `identity.human.erased`/`identity.permission.revoked`.
A matching event **busts the shared projection cache entry** for that `ArtifactRef`; the gateway pushes a live
card update (a firehose frame) to viewers currently showing it. A permission-revoked event also drops the
viewer's cached `check` (honouring the zookie revision watermark, contract 4.10).

### 4.5 Resilient-client degradation

Every `project(ref, viewer)` call (Refs → owner) goes through the shared resilient client (timeout/breaker/
bulkhead; contract 1.9) — a slow/down owning subsystem degrades the *card* to "couldn't load — retry" (fails
static), **never** stalls the message render. The message renders; the card retries.

**Live-vs-snapshot policy (decided: live, with an audit "as-of").** The card renders live per-viewer; the **only
durable thing stored is the `artifact_ref` node + the post-time timestamp** (the audit "as-of" — a reference,
not rendered content). No rendered title/state/PII is ever stored → **erasure is free** (§6). Drills owed:
**unfurl-no-leak** (a viewer lacking access → tombstone, never the title) and **unfurl-erasure-safe** (an erased
third party in a card → tombstone on next render) ([07](./07-drills-and-open-questions.md) D-C5/D-C6).

---

## 5. The HITL approval-card bridge + the Activity-as-view link (Sketch 06)

### 5.1 The round-trip (Chat's seat marked)

The platform warns the bridge is "easy to ship the card but forget" (EI-03 §5.1). The full bridge spans three
owners (durable-workflow §4.4); **Chat is the surface** (steps 2 + 3, below):

```
1. Agent workflow hits a gated tool → ctx.wait_for_signal("approval:<card>", timeout=window)        [Workflow]
   → emits agent.approval.requested via OUTBOX { tool, args(ArtifactRefs), RISK, LIVE COST ESTIMATE } [Agent Fabric]
   → Bus Signal tier routes (reason=approval_requested, priority=critical)                            [Bus/Notif]
   ┌──────────────────────────────────────────────────────────────────────────────────────────────┐
   │ 2. CHAT renders the APPROVAL CARD in the thread/channel where the run is anchored               │ ← Chat
   │    (correlation_id threads the run to the originating message — the anchoring rule),            │
   │    AND lands it as a Notif inbox item (reason=approval_requested) so it's never missed (C-9).   │
   │    Card is HUMANISED by Notif `humanise` (contract 7.3, the SOLE templating surface — the agent │
   │    does no string work); args resolve per-viewer via Refs resolve (a restricted arg → tombstone)│
   │    The workflow is now state=waiting, holding NO runtime, for up to `window` (may be DAYS).      │
   ├──────────────────────────────────────────────────────────────────────────────────────────────┤
   │ 3. A human clicks Approve / Edit / Reject (in Chat).                                             │ ← Chat
   │    Chat: Id.check(human, approve, run)                — approval authority gate.                 │
   │    Chat: DurableExecutor::signal(run, "approval:<card>",                                         │
   │            {approved|denied|edited, by:ref}, idem_key = <PER-EFFECT KEY>)   — THE BRIDGE.        │
   └──────────────────────────────────────────────────────────────────────────────────────────────┘
4. Signal lands (idempotent on idem_key — a double-click is ONE approval). Waiting wf re-leases, replays    [Workflow]
   to the wait, consumes the signal:                                                                       [Agent Fabric]
     approved → gated TOOL_EXEC runs (the step re-runs with the tool now allowed)
     denied   → tool WITHHELD (returns a Denied tool error, no mutation); agent continues   (AG-8)
     edited   → human-amended effect applied
     timeout  → the durable timer fired first → auto-deny + notify
   → outcome announced back in the SAME thread (an agent message, humanised).                              [Chat]
```

- **The frozen per-effect `idem_key` (contract 9.1/9.4, OQ-F):**
  ```
  idem_key = card_id                 // single-effect card: one approval, double-click idempotent
  idem_key = card_id ":" effect_idx  // multi-effect card: each effect approved independently and idempotently
  ```
  `DurableExecutor::signal` is idempotent on `idem_key`, so a double-click / retried click is **one** approval.
- **Per-viewer-safe:** the card's args are `ArtifactRef`s resolved per-viewer via `resolve` — a restricted arg
  renders a tombstone, never leaks.
- **Chat owns:** the card UI, the Approve/Edit/Reject affordance, the `Id.check(approve)` gate, the `signal`
  post. **Chat does NOT own:** the durable wait, the timer, the budget/cost, the withhold/resume logic (Workflow
  + Agent Fabric). On resume the workflow re-mints its attenuated agent token (contract 4.7, callable
  mid-workflow), so a days-later approval runs under a fresh token, not a stale one.
- **Drill:** request an approval, kill Chat + Workflow mid-wait, approve days later → **the gated tool runs
  exactly once; a double-click is one approval; deny withholds with no mutation** ([07](./07-drills-and-open-questions.md) D-C9).

### 5.2 Batch / partial approval (the frozen per-effect scheme, OQ-F)

One card **can present a multi-effect plan** ("open PR #88, link issue ENG-412, post to #incidents") with
**per-effect Approve/Reject** plus an "approve all". A **partial approval** (approve effects 0 and 2, decline 1)
sends three signals `{card_id:0 = approve}`, `{card_id:1 = decline}`, `{card_id:2 = approve}`; each is idempotent
on its own key, **each maps to exactly one `EffectApi::apply`** (a declined effect is **withheld** — AG-8,
returns a `Denied` tool error, never mutates). A double-click on "approve all" re-sends the same keys → no
double-apply. "A double-click is one approval" and "a partial approval is well-defined" are both true by
construction. The card anchors to the thread/channel that triggered the run (`correlation_id`) + the Notif inbox.
**Drill:** a multi-effect card approved 2-of-3 → **the 2 resume approved, the 1 withheld, each independent; no
effect runs twice** ([07](./07-drills-and-open-questions.md) D-C10).

### 5.3 "Activity/Mentions" is a scoped VIEW into the one Notif inbox (C-9 — binding)

Chat's "Activity/Mentions" is **a filtered query into Notif's one inbox — not a second store** (contract 7.1):

```
Chat "Activity / Mentions" = Notif.list_inbox(me,
    filter = subsystem ∈ {chat} ∧ reason ∈ {mentioned, replied, thread_watched, approval_requested})
    ranked by priority DESC
```

(verbatim from the frozen Notif C-9 table, notifications.md §1.) **Chat does NOT build a mentions inbox.** One
store → one read-state truth: marking a mention read in Chat's Activity view is the *same row* as the unified
inbox. The link to Chat's own read-state (§3): opening a channel and scrolling past a mentioned message calls
`Notif.mark(item, read)`; an item snoozed in the unified inbox doesn't re-badge in Chat. The two read-states
(Chat's per-channel scroll position; Notif's per-item state) are **linked at the mention, not duplicated**. If
Chat built its own mentions store it would recreate the "three inboxes fragment attention" disease the platform
exists to fix; C-9 forbids it, Chat honours it by being a view.

---

## 6. The erasure cascade (Chat is the hardest holder; Sketch 05)

Chat bodies are pervasive, unstructured free-text PII, **often about other people**, replicated into derived
stores. The platform's answer is the **references-not-payloads + crypto-shred + tombstone triad** (EI-04 §1;
contract 11.4) — *delete the identity, not the fact*. The crucial honesty: **a chat body IS the PII** (not a
reference), so Chat leans hard on crypto-shred. Two erasure subjects, kept distinct.

### 6.1 Role 1 — P authored the message (their own content)

**Crypto-shred the body + tombstone the record (the structural floor of contract 10.9).** Bodies (and drafts)
are envelope-encrypted under P's **per-subject DEK** (contract 11.4, the canonical GD-4 case). `erase(P)` =
**destroy P's DEK** → every body P authored becomes unrecoverable ciphertext **in the hot store, the cold
segments, AND backups simultaneously** (the crypto-shred property; Boneh & Lipton 1996; NIST SP 800-88r1) —
without rewriting the immutable log. The *record* survives as a **tombstone** ("message deleted") so the
conversation's structure/order/causality stays intact for others (`state = tombstoned`). Per-subject (not
per-tenant) DEK is exactly the granularity rule — a per-tenant key would force erasing P to destroy *everyone's*
bodies.

### 6.2 Role 2 — P is mentioned in others' messages

**Structured-node neutralisation via the pseudonym-map shred (free because of ADR-05; contract 10.9 step 2).** A
`mention(Principal)` is a structured node pointing at P's **pseudonymous principal_id** (never inline PII).
`erase(P)` needs **no message mutation** in the common case: Id's pseudonym-map shred (contract 4.8;
`<pseudonym>@<tenant>.noreply` grammar) makes the id unresolvable, and the mention **renders to `[erased user]`**
on next render — the same references-not-payloads lever Refs/Notif use. The mention being *structured* is the
whole reason this is tractable.

### 6.3 The residual is handled BY REFERENCE to the ONE platform posture (contract 10.9, recon §X-7)

P's name typed into the **free-text body** of someone else's un-erased message ("I talked to Alice Smith about
X") is encrypted under the **author's** DEK, not the subject's, so the subject's erasure does not crypto-shred
it. **This is the platform's named residual, NOT a chat-specific one.** Per recon §X-7, no subsystem doc
restates the posture; Chat instantiates it by reference:

> The residual third-party free-text PII in Chat message bodies is handled **per the platform posture in
> 00-reconciliation §X-7 / contract 10.9**: best-effort on-request `rectify`/tombstone of the specific span, plus
> the standing structural guarantee that the residual is **never indexed, never agent-readable, never in
> analytics for a restricted subject** (the `restrict` suppression). `[OPEN — LEGAL]`: counsel/DPO ratify the
> residual lawful basis as ONE platform statement.

Chat supplies only the **structural floor** the posture relies on (per-subject DEK shred + pseudonym-map shred +
`restrict` suppression at every read path); the lawful-basis statement is the platform's single ratified
posture, not a fifth chat residual.

### 6.4 The cascade reaches every Chat-owned store (the holder enumeration)

`PersonalDataHolder` auto-registration (contract 1.4) makes "we forgot store X" structurally impossible — every
store the harness opens is registered. The cascade is triggered by `identity.human.erased` (consumed) + the DSR
orchestrator fan-out (contract 10.4), **never** a Chat-private backdoor:

| Chat store | Holds | Erasure mechanism |
|---|---|---|
| Message log (hot) | bodies (per-subject-DEK), mention nodes (pseudonymous), tombstones | crypto-shred P's DEK (author) + pseudonym-map shred (mention); tombstone the record |
| Cold segments + backups | sealed encrypted ranges | crypto-shred (key destruction reaches cold + backups for free — the point) |
| Unfurl projection cache | short-TTL projections (may hold a name in a title) | purge entries naming P; they re-resolve live → tombstone (no durable snapshot exists) |
| Read-state | P's last-read markers (P's own data) | delete P's Valkey keys + PG rows |
| Membership / prefs / drafts | P's memberships, prefs, pins/bookmarks, **drafts** (drafts are PII) | delete P's rows; **drafts crypto-shred** (P-authored free text) |
| Gateway ephemeral state | P's live sockets, presence, resume cursors | drop on erase (ephemeral; TTL'd anyway) |
| Search index (Search-owned, Chat triggers) | indexed message terms + **embeddings** | Search **purges + reindexes** on the erasure event (embeddings are personal data) |
| Refs edges (Refs-owned) | pseudonymous `origin_actor` | Refs pseudonym-map shred (no Chat action beyond the event) |
| Notif inbox items (Notif-owned) | chat-originated items referencing P | Notif references-not-payloads → tombstone |

- **Live unfurls + ephemeral caches are favoured over durable snapshots** *precisely* so a later-erased third
  party isn't frozen in a card (§4) — the erasure design and the unfurl design are the same decision twice.
- **`restrict` (Art. 18)** suppresses indexing/agent-use/new-notification-routing/analytics for a restricted
  subject — a distinct state from erasure, honoured at every read path (contract 10.1).
- **Retention** (per-channel auto-delete after N days) is a bulk erasure path on the same cascade;
  tightest-policy-wins + legal-hold-aware (contract 10.5). Chat owns the *policy hook*
  (`conversation.retention_days`); GDPR owns the engine.
- **Audit:** an agent reading a channel **is processing personal data** → audited with lawful basis (the
  tamper-evident log is GDPR's, contract 10.6, distinct from chat history).
- **Drill:** erase a person → assert bodies crypto-shred in hot + cold + **backups**; mentions → `[erased user]`;
  Search/Refs/Notif cascade → **0 recoverable PII** ([07](./07-drills-and-open-questions.md) D-C8).

### 6.5 `PersonalDataHolder` shape (illustrative; contract 10.1)

```rust
impl PersonalDataHolder for Chat {
  fn locate(subject)   -> messages authored-by | mentioning subject; memberships; read-state; drafts/pins;
                          unfurl-cache entries naming subject; gateway live state.
  fn export(subject)   -> subject's messages (decrypted with their DEK), mentions OF them, DMs, reactions,
                          memberships — the Art. 15/20 DSR bundle (cross-refs resolved via owners).
  fn rectify(subject)  -> profile rectification is Id's; chat stores no rectifiable profile copy (refs only);
                          a best-effort span rectify on residual free-text is the contract-10.9 path.
  fn restrict(subject) -> stop indexing / agent-use / new notification routing / analytics for the restricted
                          subject (the platform restriction flag; honoured at every read path).
  fn erase(subject)    -> crypto-shred subject's per-subject DEK (bodies + drafts) → unrecoverable in
                          hot/cold/backups; tombstone the records; pseudonym-map shred handles mentions;
                          purge unfurl-cache + read-state; drop gateway state; cascade to Search/Refs/Notif via the bus.
}
```

---

## 7. Agent presence, streaming, and explicit-first dispatch (Sketch 07; CHAT-1)

### 7.1 Explicit-first dispatch (CHAT-1 — pinned in reconciliation)

**Runtime agent dispatch is EXPLICIT-FIRST** (contract 8.6, recon §6 — pinned). A casual `@triage-bot` mention
does **not** auto-spawn a costed autonomous run; v1's trigger surface is an **explicit "run an agent here"
action** (a slash command `/run <agent> <task>`, a "Run agent" action on a message/thread, or a
reaction-as-explicit-invoke on an agent's offered action). A structured `mention(Principal)` *displays* an agent
and *notifies* it (an item in the agent's inbox — agents have inboxes), but **notifying ≠ dispatching a run.**
Implicit auto-wake is **L-3** (counsel-gated, intent/cost-detection + DPO-aware Art. 22 / EU AI Act), **not
built in v1**; no auto-spawn path is wired until counsel ratifies the human-oversight basis (agent-fabric §3.4).

- **The reference gate:** only a structured, picker-produced reference (the explicit action / the `mention`
  node) can re-trigger an agent — **never raw typed text** (Bus dispatch tier, contract 3.6). Explicit-first
  dispatch and the loop-safety reference gate are the **same mechanism**.
- **Cost backstop (reserve/settle):** even an explicit run passes the universal reserve/settle gate (contract
  11.7) before the Agent Fabric/CI runner starts it — *no balance → no execution*; a runaway is self-limiting.
  Chat does not own the wallet; it surfaces the cost (the HITL card's live estimate) and dispatches through
  **`EffectApi`** (which reserves), never a Chat-private path. The agent run inherits the **four uniform
  sandbox guarantees** (X-6): cost gate, per-run attenuated token attribution, HITL withhold, isolation
  floor+drill.

### 7.2 Agent presence (its own fabric-health-derived class)

"What does *online* mean for an agent?" An agent is not a human with an idle timer. **Agent presence is its own
class, tied to agent-fabric health:** `available` (runtime healthy + within budget/quota), `busy` (in a run /
streaming), `rate-limited` (shed by the protected-human-lane / per-tenant caps — OQ-K), `offline` (runtime
unavailable). These map to `chat.presence.*` / consume `agent.status_changed`. Presence rides the **firehose,
never the durable bus** (ADR-04.5), same ephemeral transport as human presence/typing (§1.2), with TTLs. Status
is shown by **glyph + label + position, never colour alone** (design-language §3.2/§4), with **no
sparkle/shimmer/magic-wand iconography** ("agents look like agents, not magic").

### 7.3 Streaming partial output

```
agent.message.partial frames → FIREHOSE (ephemeral; high-freq; low-value-if-lost)  — NOT the durable bus
the thread shows a "working…" affordance updating as partials arrive
on the agent's Submit:
  the FINAL durable chat.message.created (agent-attributed, provenance-bearing) replaces the partial,
  reconciled by the run's correlation_id / message_id
a reconnect mid-stream re-fetches the FINAL / in-progress marker from the durable log + live firehose (resume) — never a half-message
```

- The partial is live-only; if lost, the **final message is the truth** (resume-on-reconnect, §1.3).
- **Built and proven against mocks** (D6 `--use-mock`): the mock runtime streams scripted partials on the same
  path, so the streaming UX is proven without a real LLM (VISION §3; the strategy seam, contract 8.3).
- **Calm volume:** streaming/agent verbosity lives in **threads + collapsible summaries**, out of the main
  timeline by default (threads-first with explicit broadcast, §7.4). This matters *more* in Myelin than in Slack
  precisely because agents raise volume.

### 7.4 Threads-first with explicit broadcast (Sketch 08)

A reply goes to its **thread by default**; "also send to channel" is an *explicit, deliberate* broadcast (not the
Slack inverse). A thread = messages sharing a `thread_root_id`, addressable by the frozen `thread-<root>` `#sub` kind
(contract 5.7); per-thread read-state + unread/mention counts (§3). The **thread pane** hosts agent detail +
streaming. This keeps agent verbosity and incident detail out of the main timeline by default (calm-by-default;
Zulip-style topic threading considered specifically because agent participation raises volume). The threading
primitive is **shared with Knowledge/Issues comment threads at the `#sub`/content/refs level** (OQ-L), so the
named consolidation is a merge, not a rewrite ([05 §6](./05-hard-problems.md)).

### 7.5 Attribution, provenance & loop guards

Every agent message carries (and the UI surfaces): the **agent badge** (AI-Act legibility — agents are never
disguised as humans); a **provenance popover** — *which* agent, **on whose authority/lawful basis**
(`actor.on_behalf_of`), **triggered by which event** (`causation_id` / the explicit action), `correlation_id`
threading the flow — answering "why did this agent post?" inline with an audit-log link. **Loop/abuse guards are
the platform's structural ones** (the dispatch tier, contract 3.6), which Chat *honours* via `OutboxTx::emit(draft,
cause)` (causality correct-by-construction so a human cannot typo into a loop): self-guard (an agent never
re-triggers on its own output), the reference gate (= explicit dispatch), the causal-depth ceiling +
shared-root tripwire, bounded dispatch + per-tenant caps + reserve/settle (no balance → no run).

Continue to [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) for the taxonomy + glue.
