# Chat — 02 · Internals & Algorithms

> See [`00-overview.md`](./00-overview.md) for framing, [`01-tech-and-data-model.md`](./01-tech-and-data-model.md)
> for the data model. This doc details the subsystem-specific algorithms: the connection tier + NATS backplane +
> resume-cursor resync (§1); message-store tiering + the resync read (§2); the read-state hot path (§3); cheap
> per-viewer unfurls (§4); the HITL bridge + the Activity-as-view link (§5); the erasure cascade (§6); the agent
> presence/streaming/dispatch semantics (§7). Each hard problem is resolved with cited prior art in
> [`05-hard-problems.md`](./05-hard-problems.md); this doc is the *mechanism*.

---

## 1. The connection tier: backplane + the correctness backbone

### 1.1 Topology

The gateway is **horizontally sharded and stateless** ([01 §1.1](./01-tech-and-data-model.md)). A client
connection (WS primary; SSE/long-poll fallback for restrictive networks + the CLI `chat tail --follow`) lands on
*any* gateway node (a near-edge node for a globally-distributed user; writes route home to the tenant's cell —
the single-home-cell FLOOR, [05 §1](./05-hard-problems.md)). The node:

1. **Authenticates the handshake** via Id `authenticate(credential) → Principal` — the **tenant comes from the
   verified token, never the path** (ID-3). The gateway trusts the injected identity for *identity* and
   re-authorizes nothing it isn't responsible for (per-action authz is the Rust services' job, Sketch 09).
2. **Subscribes to NATS-core subjects** for the channels this connection is a member of — `fan.<tenant>.<channel>`
   per channel (membership resolved once via `list_objects(viewer, read, channel)` at connect, refreshed on
   `chat.channel.member_*` events).
3. **Holds a per-connection bounded queue** (drop-and-resync on overflow — the "one slow connection stalls
   others" failure is prevented by bounded queues, not unbounded buffering; Phase-2 §2).

### 1.2 The fan-out backplane — NATS-core subject-per-channel

When the Message Service persists a message, the **Indexing/Outbox feeder** also `firehose::publish`es the
**rendered message frame** to `fan.<tenant>.<channel>` (NATS core; Bus §4.3). NATS routes the frame to exactly
the gateway nodes holding a subscribed connection; each node pushes it to the matching sockets. This is
**read-fanout per channel** (one ordered log; readers cursor in) — the backplane just nudges online sockets.

- **Non-durable / at-most-once by design.** NATS core does not persist; a frame missed during a hiccup is
  **correct to drop** because the durable log is the source of truth and the resume cursor recovers it (§1.3).
  This is what makes the backplane cheap.
- **Mega-channel escalation (FLOOR, R-5).** A 100k-member announcement channel = up to 100k gateway-side
  subscribers on one subject — workable, but the **channel-sharded home-node** is the named escalation: a
  measured-hot channel gets a dedicated fan-out home node that gateways pull from, rather than 100k direct subject
  subscribers (the Phoenix/Discord guild model, but Rust + consistent-hash). Promotion trigger: measured
  subscriber count exceeding the subject-fan-out budget. Until measured, the subject model is the design
  (ADR-10 anti-premature-shard). [05 §1](./05-hard-problems.md).

### 1.3 Resume-cursor resync — the correctness backbone (zero-loss-across-reconnect)

The per-conversation total-order + no-lost-messages guarantee (Phase-1 §5.7) comes **not** from the backplane
but from the **durable per-conversation log + a resume cursor** (the EI-04 §2.2 discipline applied to chat):

```
on (re)connect, per subscribed conversation:
  client sends its cursor = last message_id it has for that conversation
  gateway: gap = MessageStore::resync_from(conversation, cursor)        # a clustering-range read, ordered, gap-free
  gateway: stream the gap to the client   (idempotent on message_id — a re-streamed message is a client-side no-op)
  THEN resume live NATS delivery          (any frame that arrived during the gap is already in the gap → no dup, no loss)
```

- A frame lost during a NATS hiccup is recovered by the **next resync** — so the ephemeral backplane is *allowed
  to drop*, which is why it can be cheap and non-durable. This decision is what lets §1.2 be a non-durable
  transport without violating Phase-1 §5.7.
- **Idempotency on `message_id`** makes the gap stream + the resumed live stream overlap-safe: a message seen in
  both is deduped client-side. The **send** path is idempotent on `client_nonce` ([01 §3](./01-tech-and-data-model.md))
  so a retried send (flaky mobile/agent) is one message.
- **Drill (the build-order item-0 gate):** sever the gateway↔backplane mid-publish; assert resync from the
  durable log recovers the gap → **0 lost, 0 duplicate** ([07](./07-drills-and-open-questions.md)).

### 1.4 The protected-human-lane shed order (ADR-16; the gateway is *the* edge)

The gateway is where the worst load manifests: connection storms (a deploy reconnect thundering-herd),
mega-channels, agent-generated fan-out (EI-03 §5.4). The substrate's protected-human-lane shed order (substrate
§7) applies here as Chat's **per-surface shed budget** (a Chat P4 deliverable, Sketch 10 §D):

- **Lanes:** human connections in the protected lane; agent/CI connections in the shed-able lane (get `429 +
  Retry-After`). The gateway honours `Retry-After` on its own RPC calls (else shedding becomes a retry storm —
  the protected-human-lane defeat).
- **Per-tenant fairness:** a 30× agent surge on one tenant sheds *that tenant's agent lane*, never other
  tenants' humans (per-tenant in-flight caps).
- **Telemetry survival signals** (substrate §10.2, X-1): connection count, per-tenant in-flight, shed counts per
  lane, NATS-subject lag, resync-gap size, breaker state — the Phase-5 drills assert against these.
- **Drill:** 30× agent message/connection surge on one tenant → **human latency in budget; agent lane sheds
  (429+Retry-After honoured); other tenants unaffected** ([07](./07-drills-and-open-questions.md)).

---

## 2. Message-store tiering + the resync read

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
  melts a single PG table (Phase-1 §5.3 "strongly suggest tiering"). Sealing cold ranges to immutable encrypted
  segments keeps the hot tier bounded; the segment is still range-readable and still crypto-shreddable (destroy
  the key). This is Candidate C from Sketch 02 used *only* as the cold tier, not as the v1 hot store.
- **Crypto-shred inside a segment:** a segment is encrypted under the per-tenant DEK; the *bodies* inside it are
  additionally per-subject-DEK-encrypted (the body ciphertext is double-wrapped). Erasing a person destroys their
  per-subject DEK → their bodies inside any segment become unrecoverable **without rewriting the immutable
  segment** (§6). This is the whole reason crypto-shred (not delete) is the erasure mechanism.

### 2.2 Per-conversation order at scale

`message_id` ULID gives intrinsic order; appends are tail-writes; the `UNIQUE(conv, client_nonce)` constraint
plus the ULID make burst-sends + edits to a hot channel preserve per-conversation total order. Causal anomalies
on the client (an edit/reaction arriving before its target message — possible over the live backplane) are
reconciled to the stable `message_id` order: the client buffers an out-of-order op until its target arrives, or
on resync re-orders to the durable sequence (Phase-1 §5.7). **Drill:** burst sends + edits to one hot channel →
**per-conversation total order preserved; resync gap-free** ([07](./07-drills-and-open-questions.md)).

---

## 3. The read-state hot path (Valkey + PG, eventually-consistent; Sketch 03)

```
on scroll/open (the high-frequency write):
  Valkey HSET read:<t>:<p>:<conv> = last_read_message_id        # debounced (coalesce rapid scrolls), in-memory
  firehose::publish chat.read_state.updated (coarse)            # ephemeral; NOT the durable bus (ADR-04.5)
batched flush (cadence ~seconds, debounced):
  UPSERT read_state(...)                                        # PG durable record (STOR-3: this is the truth)
on cache loss (STOR-3 reconstruction):
  the PG record is authoritative; a marker is at-worst slightly stale → you re-see a few read messages (benign, bounded)
unread count (DERIVED, never authoritative):
  count(message_id > read_state.last_read[conv])               # a bounded range read against the log, cached
```

- **STOR-3 is law: Valkey is never the source of truth.** The PG `read_state` record is; Valkey is a write-back
  cache. A cache loss never loses correctness, only freshness, and the failure is *benign and bounded* (you
  re-see a few already-read messages).
- **Eventually-consistent is acceptable** for read-state (Phase-1 §5.6): "delivered/seen on one device →
  eventually seen everywhere" (cross-device truth, Notif §9 Q7).
- The link to Notif's inbox read-state is at the **mention** (§5.3), not duplicated.

---

## 4. Cheap per-viewer unfurls (the wedge; Sketch 04)

The unfurl service is a **Chat-owned cache + orchestration layer in front of Refs `resolve`** — it does **not**
re-implement permission-aware resolution. Refs §4.2 is the non-leaking chokepoint; Chat's job is to make the
per-viewer call *cheap* at chat density. The layered cheapening, in order:

### 4.1 Lazy-on-viewport (the single biggest cost-killer)

A virtualised timeline (design wireframes S2) resolves unfurls **only for messages currently in the viewport**.
A scroll-back of 10,000 messages resolves a handful of cards, not 10,000. The naïve "resolve every ref in the
channel" is the trap; lazy-on-viewport defeats it.

### 4.2 Split the cache by what varies per-viewer vs. what doesn't

```
for each artifact_ref node in a VISIBLE message, per viewer:
  decision = Id.check(viewer, view, ref)         # the PER-VIEWER part: fast, cached, Leopard-prefiltered (Id §8)
  if Deny:  render TOMBSTONE ("a restricted <type>")   # the title NEVER leaks (Refs §4.2; ADR-03)
  else:
    proj = unfurl:proj:<ref>  (Valkey)           # the VIEWER-INDEPENDENT projection content — cached ONCE per ref
            ?? refs.resolve(ref, viewer, Display) # cache miss → Refs → owner.project(ref, viewer) via resilient client
    render: live card (title/state/icon/actions) | "deleted/erased" tombstone | "couldn't load — retry"
```

The insight (mirroring Refs §4.2): **projection content is viewer-independent** — cache it once per `ArtifactRef`,
short-TTL, bus-busted. **The permission decision is per-viewer** — but it is a `check`/`list_objects`, the
platform's fast primitive — and content is returned **only after the per-viewer check passes**, so there is
**one shared cache entry per ref, never one per `(ref, viewer)`**, with no leak. A popular doc embedded in 500
messages resolves its content once; the per-viewer cost is a cheap cached `check`.

### 4.3 Membership-as-permission class precompute

For a **public** channel in a project, "can a channel member see this project artifact?" is often a single coarse
class, not 500 checks: channel membership compiles to a ReBAC tuple, and `list_objects(viewer, view, type)`
returns a **`Filter{set_expr, zookie}`** (Id §8.2) the unfurl service applies once. For a **private** channel
whose membership ≈ the visibility class, often *all members see the same artifacts* — one class decision, not N
(Phase-1 §5.4; the `list_objects` push-down, S-10).

### 4.4 Bus-driven invalidation (precise; TTL is the backstop)

The Unfurl Service runs a consumer (substrate template, whitelisted subjects) on the artifact pointer events —
`issue.*.updated`, `git.pr.*` + `git.pr.checks_completed`, `ci.run.*` + `ci.log.available`, `knowledge.doc.updated`,
and crucially `*.erased` and `identity.human.erased`/`permission.revoked`. A matching event **busts the shared
projection cache entry** for that `ArtifactRef`; the gateway pushes a live card update to viewers currently
showing it (Phase-2 Chat §6.1.4). A permission-revoked event also drops the viewer's cached `check`.

### 4.5 Resilient-client degradation

Every `project(ref, viewer)` call (Refs → owner) goes through the shared resilient client (timeout/breaker/
bulkhead; substrate §6) — a slow/down owning subsystem degrades the *card* to "couldn't load — retry" (fails
static), **never** stalls the message render. The message renders; the card retries.

**Live-vs-snapshot policy (decided: live, with an audit "as-of").** The card renders live per-viewer; the **only
durable thing stored is the `artifact_ref` node + the post-time timestamp** (the audit "as-of" for lawful-basis
records — a reference, not rendered content). No rendered title/state/PII is ever stored → **erasure is free**
(§6). Drills owed: **unfurl-no-leak** (a viewer lacking access → tombstone, never the title) and
**unfurl-erasure-safe** (an erased third party in a card → tombstone on next render) ([07](./07-drills-and-open-questions.md)).

---

## 5. The HITL approval-card bridge + the Activity-as-view link (Sketch 06)

### 5.1 The round-trip (Chat's seat marked)

The platform warns the bridge is "easy to ship the card but forget" (EI-03 §5.1). The full bridge is designed
in Workflow §6.3 with three owners; **Chat is the surface** (steps 2 + 3, below):

```
1. Agent workflow hits a gated tool → ctx.wait_for_signal("approval:<call>", timeout=window)        [Workflow]
   → emits agent.approval.requested via OUTBOX { tool, args(ArtifactRefs), RISK, LIVE COST ESTIMATE } [Agent Fabric]
   → Bus Signal tier routes (reason=approval_requested, priority=critical)                            [Bus/Notif]
   ┌──────────────────────────────────────────────────────────────────────────────────────────────┐
   │ 2. CHAT renders the APPROVAL CARD in the thread/channel where the run is anchored               │ ← Chat
   │    (correlation_id threads the run to the originating message — the anchoring rule),            │
   │    AND lands it as a Notif inbox item (reason=approval_requested) so it's never missed (C-9).   │
   │    Card is HUMANISED at the backend (Notif §3.3; NOTIF-1 — the agent does no string work);      │
   │    args resolve per-viewer via Refs resolve (a restricted arg → tombstone, never leaks).        │
   │    The workflow is now state=waiting, holding NO runtime, for up to `window` (may be DAYS).      │
   ├──────────────────────────────────────────────────────────────────────────────────────────────┤
   │ 3. A human clicks Approve / Edit / Reject (in Chat).                                             │ ← Chat
   │    Chat: Id.check(human, approve, run)                — approval authority gate (Workflow §6.3). │
   │    Chat: DurableExecutor::signal(run, "approval:<call>",                                         │
   │            {approved|denied|edited, by:ref}, idem_key = card_id)   — THE BRIDGE.                 │
   └──────────────────────────────────────────────────────────────────────────────────────────────┘
4. Signal lands (idempotent on card_id — a double-click is ONE approval). Waiting wf re-leases, replays    [Workflow]
   to the wait, consumes the signal:                                                                       [Agent Fabric]
     approved → gated TOOL_EXEC runs (the step re-runs with the tool now allowed — AG-8)
     denied   → tool WITHHELD (ordinary error to the loop, no mutation); agent continues
     edited   → human-amended effect applied (design-language §6.3)
     timeout  → the durable timer fired first → auto-deny + notify
   → outcome announced back in the SAME thread (an agent message, humanised).                              [Chat]
```

- **Idempotency is non-negotiable** here: `idem_key = card_id` makes a double-click / retried click **one**
  approval (`DurableExecutor::signal` is idempotent on `idem_key`, Workflow §5.1).
- **Per-viewer-safe:** the card's args are `ArtifactRef`s resolved per-viewer via `resolve` — a restricted arg
  renders a tombstone, never leaks.
- **Chat owns:** the card UI (the §5.4 design-language shared component), the Approve/Edit/Reject affordance, the
  `Id.check(approve)` gate, the `signal` post. **Chat does NOT own:** the durable wait, the timer, the
  budget/cost, the withhold/resume logic (Workflow + Agent Fabric).
- **Drill:** request an approval, kill Chat + Workflow mid-wait, approve days later → **the gated tool runs
  exactly once; a double-click is one approval; deny withholds with no mutation** ([07](./07-drills-and-open-questions.md)).

### 5.2 Batch / multi-effect approval (the resolved open product question)

One card **can present a multi-effect plan** (design-language §6.2 "open PR #88, link issue ENG-412, post to
#incidents") with **per-effect Approve/Reject** plus an "approve all". Each effect still resolves its **own** gate
signal (`idem_key` per effect = `card_id:<effect_idx>`), so a *partial* approval is well-defined: approving 2 of
3 signals the 2 gates approved and the 1 denied; the workflow consumes each independently. The card anchors to
the thread/channel that triggered the run (`correlation_id`) + the Notif inbox. (Resolves the Sketch 06 open
question, joint with Workflow §6.3.)

### 5.3 "Activity/Mentions" is a scoped VIEW into the one Notif inbox (C-9 — binding)

Chat's "Activity/Mentions" is **a filtered query into Notif's one inbox — not a second store**:

```
Chat "Activity / Mentions" = Notif.list_inbox(me,
    filter = subsystem ∈ {chat} ∧ reason ∈ {mentioned, replied, thread_watched, approval_requested})
    ranked by priority DESC
```

(verbatim from Notif §1.3's C-9 table.) **Chat does NOT build a mentions inbox.** One store → one read-state
truth: marking a mention read in Chat's Activity view is the *same row* as the unified inbox. The link to Chat's
own read-state (§3): opening a channel and scrolling past a mentioned message calls `Notif.mark(item, read)`;
conversely an item snoozed in the unified inbox doesn't re-badge in Chat. The two read-states (Chat's per-channel
scroll position; Notif's per-item state) are **linked at the mention, not duplicated**. If Chat built its own
mentions store it would recreate the exact "three inboxes fragment attention" disease the platform exists to fix
(P8); C-9 forbids it, Chat honours it by being a view.

---

## 6. The erasure cascade (Chat is the hardest holder; Sketch 05)

Chat bodies are pervasive, unstructured free-text PII, **often about other people**, replicated into derived
stores. The platform's decided answer is the **references-not-payloads + crypto-shred + tombstone triad** (EI-04
§1; Bus §4.8; Storage §5.1) — *delete the identity, not the fact*. The crucial honesty: **a chat body IS the PII**
(not a reference), so Chat leans hard on crypto-shred. Two erasure subjects, kept distinct:

### 6.1 Role 1 — P authored the message (their own content)

**Crypto-shred the body + tombstone the record.** Bodies (and drafts) are envelope-encrypted under P's
**per-subject DEK** (GD-4; the canonical case). `erase(P)` = **destroy P's DEK** → every body P authored becomes
unrecoverable ciphertext **in the hot store, the cold segments, AND backups simultaneously** (the crypto-shred
property; Boneh & Lipton 1996; NIST SP 800-88r1) — without rewriting the immutable log. The *record* survives as
a **tombstone** ("message deleted") so the conversation's structure/order/causality stays intact for others
(`state = tombstoned`). Per-subject (not per-tenant) DEK is exactly GD-4's granularity rule — a per-tenant key
would force erasing P to destroy *everyone's* bodies.

### 6.2 Role 2 — P is mentioned in others' messages

**Structured-node neutralisation, free because of ADR-05.** A `mention(Principal)` is a structured node pointing
at P's **pseudonymous principal_id** (never inline PII). `erase(P)` needs **no message mutation** in the common
case: Id's pseudonym-map shred (Id §11) makes the id unresolvable, and the mention **renders to `[erased user]`**
on next render — the same references-not-payloads lever Refs/Notif use. The mention being *structured* is the
whole reason this is tractable.

**The residual (FLOOR, named honestly):** P's name typed into the *free-text body* of someone else's un-erased
message ("I talked to Alice Smith about X") is **not** a structured node and cannot be surgically neutralised
without content analysis. Covered by **retention + access-control + a documented lawful-basis limit** (the chat
analogue of GD-1's git-history residual). We do **not** pretend free-text third-party mentions are perfectly
erasable. → P4 + LEGAL ([05 §5](./05-hard-problems.md), [06](./06-shared-system-change-requests.md)).

### 6.3 The cascade reaches every Chat-owned store (the holder enumeration)

`PersonalDataHolder` auto-registration (substrate §3.4) makes "we forgot store X" structurally impossible — every
store the harness opens is registered. The cascade is triggered by `identity.human.erased` (consumed) + the DSR
orchestrator fan-out (GDPR §4), **never** a Chat-private backdoor:

| Chat store | Holds | Erasure mechanism |
|---|---|---|
| Message log (hot) | bodies (per-subject-DEK), mention nodes (pseudonymous), tombstones | crypto-shred P's DEK (author) + pseudonym-shred (mention); tombstone the record |
| Cold segments + backups | sealed encrypted ranges | crypto-shred (key destruction reaches cold + backups for free — the point) |
| Unfurl projection cache | short-TTL projections (may hold a name in a title) | purge entries naming P; they re-resolve live → tombstone (no durable snapshot exists) |
| Read-state | P's last-read markers (P's own data) | delete P's Valkey keys + PG rows |
| Membership / prefs / drafts | P's memberships, prefs, pins/bookmarks, **drafts** (drafts are PII) | delete P's rows; **drafts crypto-shred** (P-authored free text) |
| Gateway ephemeral state | P's live sockets, presence, resync cursors | drop on erase (ephemeral; TTL'd anyway) |
| Search index (Search-owned, Chat triggers) | indexed message terms + embeddings | Search **purges + reindexes** on the erasure event |
| Refs edges (Refs-owned) | pseudonymous `origin_actor` | Refs pseudonym-shred (no Chat action beyond the event) |
| Notif inbox items (Notif-owned) | chat-originated items referencing P | Notif references-not-payloads → tombstone |

- **Live unfurls + ephemeral caches are favoured over durable snapshots** *precisely* so a later-erased third
  party isn't frozen in a card (§4) — the erasure design and the unfurl design are the same decision viewed
  twice.
- **Retention** (per-channel auto-delete after N days) is a bulk erasure path that rides the same cascade;
  tightest-policy-wins + legal-hold-aware (GD-2). Chat owns the *policy hook* (`conversation.retention_days`),
  GDPR owns the engine.
- **Audit:** an agent reading a channel **is processing personal data** → audited with lawful basis (Art. 30
  RoPA); the tamper-evident log is GDPR's, distinct from chat history.
- **Drill:** erase a person → assert bodies crypto-shred in hot + cold + **backups**; mentions → `[erased user]`;
  Search/Refs/Notif cascade → **0 recoverable PII** ([07](./07-drills-and-open-questions.md)).

### 6.4 `PersonalDataHolder` shape (illustrative)

```rust
impl PersonalDataHolder for Chat {
  fn locate(subject)   -> messages authored-by | mentioning subject; memberships; read-state; drafts/pins;
                          unfurl-cache entries naming subject; gateway live state.
  fn export(subject)   -> subject's messages (decrypted with their DEK), mentions OF them, DMs, reactions,
                          memberships — the Art. 15/20 DSR bundle (cross-refs resolved via owners).
  fn rectify(subject)  -> profile rectification is Id's; chat stores no rectifiable profile copy (refs only).
  fn restrict(subject) -> stop indexing / agent-use / new notification routing for the restricted subject
                          (the platform restriction flag — README §5 obligation; honoured at every read path).
  fn erase(subject)    -> crypto-shred subject's per-subject DEK (bodies + drafts) → unrecoverable in
                          hot/cold/backups; tombstone the records; pseudonym-shred handles mentions;
                          purge unfurl-cache + read-state; drop gateway state; cascade to Search/Refs/Notif via the bus.
}
```

---

## 7. Agent presence, streaming, and explicit-first dispatch (Sketch 07; CHAT-1)

### 7.1 Explicit-first dispatch (CHAT-1 — decided against the flashy default)

**Runtime agent dispatch is EXPLICIT-FIRST.** A casual `@triage-bot` mention does **not** auto-spawn a costed
autonomous run; v1's trigger surface is an **explicit "run an agent here" action** (a slash command `/run <agent>
<task>`, a "Run agent" action on a message/thread, or a reaction-as-explicit-invoke on an agent's offered
action). A structured `mention(Principal)` *displays* an agent and *notifies* it (an item in the agent's inbox —
agents have inboxes, Notif §1.4), but **notifying ≠ dispatching a run.** Implicit auto-wake is a separately-decided,
intent/cost-detection + DPO-aware (Art. 22 / EU AI Act) feature (C-2, L-3), **not** built in v1 (decision-record
§(f) explicitly flags this correction to the Phase-1/2 walkthroughs that assumed auto-wake).

- **This is also the AG-6 reference gate:** only a structured, picker-produced reference (the explicit action /
  the `mention` node) can re-trigger an agent — **never raw typed text** (AG-6; Bus §4.7). Explicit-first
  dispatch and the loop-safety reference gate are the **same mechanism**.
- **Cost backstop (reserve/settle):** even an explicit run passes the universal reserve/settle gate (D8/CI-2)
  before the Agent Fabric/CI runner starts it — *no balance → no execution*; a runaway is self-limiting. Chat
  does not own the wallet; it surfaces the cost (the HITL card's live estimate) and dispatches through
  **`EffectApi`** (which reserves), never a Chat-private path.

### 7.2 Agent presence (its own fabric-health-derived class)

"What does *online* mean for an agent?" An agent is not a human with an idle timer. **Agent presence is its own
class, tied to agent-fabric health:** `available` (runtime healthy + within budget/quota), `busy` (in a run /
streaming), `rate-limited` (shed by the protected-human-lane / per-tenant caps — ADR-16), `offline` (runtime
unavailable). These map to `chat.presence.*` / consume `agent.status_changed`. Presence rides the **firehose,
never the durable bus** (ADR-04.5), same ephemeral transport as human presence/typing (§1.2), with TTLs. Status
is shown by **glyph + label + position, never colour alone** (design-language §3.2/§4), with **no
sparkle/shimmer/magic-wand iconography** ("agents look like agents, not magic"; §8b.3).

### 7.3 Streaming partial output

```
agent.message.partial frames → FIREHOSE (ephemeral; high-freq; low-value-if-lost)  — NOT the durable bus
the thread shows a "working…" affordance updating as partials arrive
on the agent's Submit:
  the FINAL durable chat.message.created (agent-attributed, provenance-bearing) replaces the partial,
  reconciled by the run's correlation_id / message_id
a reconnect mid-stream re-fetches the FINAL / in-progress marker from the durable log + live firehose — never a half-message
```

- The partial is live-only; if lost, the **final message is the truth** (resync-on-reconnect, §1.3).
- **Built and proven against mocks** (D6 `--use-mock`): the mock runtime streams scripted partials on the same
  path, so the streaming UX is proven without a real LLM (VISION §3; agent-fabric §3).
- **Calm volume:** streaming/agent verbosity lives in **threads + collapsible summaries**, out of the main
  timeline by default (P8; threads-first with explicit broadcast, §below). This matters *more* in Myelin than in
  Slack precisely because agents raise volume.

### 7.4 Threads-first with explicit broadcast (Sketch 08)

A reply goes to its **thread by default**; "also send to channel" is an *explicit, deliberate* broadcast (not the
Slack inverse). A thread = messages sharing a `thread_root_id`; per-thread read-state + unread/mention counts
(§3). The **thread pane** hosts agent detail + streaming. This keeps agent verbosity and incident detail out of
the main timeline by default (the calm-by-default principle; Zulip-style topic threading considered specifically
because agent participation raises volume — competitive-landscape §5).

### 7.5 Attribution, provenance & loop guards

Every agent message carries (and the UI surfaces): the **agent badge** (AI-Act legibility — agents are never
disguised as humans); a **provenance popover** — *which* agent, **on whose authority/lawful basis**
(`actor.on_behalf_of`), **triggered by which event** (`causation_id` / the explicit action), `correlation_id`
threading the flow — answering "why did this agent post?" inline (NOTIF-2) with an audit-log link. **Loop/abuse
guards are the platform's structural ones** (AG-6; Bus §4.7), which Chat *honours* via `OutboxTx::emit(draft,
cause)` (causality correct-by-construction so a human cannot typo into a loop): self-guard (an agent never
re-triggers on its own output), the reference gate (= explicit dispatch), the causal-depth ceiling +
shared-root tripwire, bounded dispatch + per-tenant caps + reserve/settle (no balance → no run).

Continue to [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) for the taxonomy + glue.
