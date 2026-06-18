# Phase 2 — Subsystem Architecture: **Chat**

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../../VISION.md)
> (single source of truth; never contradicted). Phase-2 spine:
> [`architecture-decisions.md`](../architecture-decisions.md) (the ADR register) and
> [`system-overview.md`](../system-overview.md) (the holistic narrative). Phase-1 deep-dive this
> builds on: [`01-research/subsystem-deep-dives/chat.md`](../../01-research/subsystem-deep-dives/chat.md);
> structural foundation: [`01-research/technical-structuring.md`](../../01-research/technical-structuring.md).
>
> **Altitude.** This is the *high-level* Chat architecture (VISION §5.2): role, internal
> structure, tech direction, the views/CLI it needs, usage examples, how it interacts with the
> rest of the platform, the shared-system changes it implies, and the open questions for Phase 4.
> Concrete schemas, the connection-tier transport choice, fan-out internals, and the erasure
> mechanism are **deferred to Phase 4 (Chat)** and **Phase 3 (shared systems)**, and each section
> names what it defers.

---

## 1. Role & responsibilities — what Chat OWNS vs delegates

**Role (Phase-1 §1).** Chat is the **synchronous/semi-synchronous conversation layer** and,
more importantly for Myelin, the place where *activity of the other four subsystems becomes
conversation, and conversation becomes action on those subsystems*. Two framings drive the
whole design:

- **Chat is the human-attention bus in front of the machine event bus.** The Event Bus (ADR-04)
  carries every state change; Chat is a primary *renderer and router* of a curated subset of
  that firehose into channels, threads, and notifications. The routing/filtering logic ("what
  surfaces, to whom, where") is core Chat value, not plumbing.
- **Chat is the most visible surface of the agent-native principle (VISION §3, ADR-08).** Humans
  and agents are the *same kind of actor* in the same channels — an agent is @-mentioned, holds a
  thread, is a channel member, reacts, and acts on artifacts it discusses. The message pipeline
  must **never branch on "is this a bot"** (Phase-1 §2.8).

### What Chat OWNS (its core competency — `system-overview.md §4`)

| Owned | Notes |
|---|---|
| **Conversation/message model** | One `Conversation` entity with a `kind` (channel public/private, DM, group-DM, artifact-linked, announcement) + a membership strategy; one `Message` record (k-sortable ID, polymorphic `author` = human **or** agent, thread root, content body, idempotency nonce, provenance) (Phase-1 §2.2, §2.3). |
| **Real-time fan-out & the connection tier** | The persistent-connection gateway (WebSocket; SSE/long-poll fallback) and the pub/sub backplane that routes a posted message to exactly the online members. The platform's hardest connection-scale problem (Phase-1 §5.1; ADR-11 hot-spot). |
| **Threads, ordering, read-state** | Per-conversation total order, gap-free history, resync-on-reconnect, per-(user×conversation) and per-thread read markers, unread/mention counts (Phase-1 §2.5, §5.6, §5.7). |
| **Unfurl rendering** | Composing the *per-viewer, permission-aware* unfurl card from an `ArtifactRef` (the projection comes from the target subsystem; Chat owns the *card UX*, lifecycle, and cache) (Phase-1 §2.4, §5.4). |
| **Reactions, pins, bookmarks, drafts, saved items** | Per-user and per-message metadata (Phase-1 §2.7). |
| **Slash-command dispatch surface** | `/create-issue`, `/ci rerun`, `/assign`, `/remind`, agent-invoking commands — Chat parses and dispatches via the Event Bus / `EffectApi`; it does **not** own the target subsystems' logic (Phase-1 §4). |
| **Channel-scoped notification *preferences*** | Mute, keyword alerts, DND schedules per channel/thread — the *prefs*; **delivery** is the Notifications shared system (Phase-1 §1, §3). |

### What Chat DELEGATES to the shared systems (`system-overview.md §4`, ADR-13)

| Delegated to | What Chat gets / owes |
|---|---|
| **Identity & Access (ADR-03)** | Actors (human+agent `Principal`s), channel membership authority, and — critically — **per-viewer `check`/`list-objects`** for unfurls and search. Chat owns *channel membership*; it does **not** implement auth (no parallel permission model). |
| **Event Bus (ADR-04)** | Heavy emit + consume via the canonical envelope + transactional outbox; per-aggregate (per-conversation) ordering; the **firehose split** for presence/typing/read-state. |
| **Reference Graph (ADR-13)** | Chat is the **densest consumer** of refs (every unfurl) and a heavy producer (every reference in a message → `ref.created` edge, "discussed in chat"). |
| **Search (ADR-03, ADR-10)** | ACL-filtered, near-real-time, multilingual message + artifact search; artifact-reference autocomplete in the composer. |
| **Notifications (ADR-12)** | Delivery to email/push/web/mobile/desktop; storm-control/dedup; Chat *produces* notifiable events and holds some preference state. |
| **Agent Fabric (ADR-08)** | Mentions/reactions are the **trigger surface**; agents post back as actors via the same message path; streaming partial output; loop/quota safety. |
| **Storage (ADR-10)** | Object tier for attachments; the **log/firehose tier** as the likely message-log substrate; residency-pinned. |
| **GDPR/Audit (ADR-12)** | Chat is a `PersonalDataHolder` (the most PII-dense one — free-text bodies *about other people*); crypto-shred + tombstone + mention-neutralization; audit of edits/deletes/agent processing. |
| **Durable-workflow (ADR-09)** | Chat is the **surface for HITL approval cards** (the agent approval card that waits minutes-to-days is rendered in chat; the wait lives in the workflow engine). |

### Explicit non-goals (Phase-1 §1)
Not the notification *delivery* fabric (that is Notifications). Not the artifact store for
docs/code (it references them). Not the identity/permission authority. **Voice/video is out of
scope for v1** — if ever, embed an EU-sovereign third party (self-hosted Jitsi/LiveKit). The
"canvas"/pinned-summary feature overlaps Knowledge and is a flagged boundary, not a v1 commit.

---

## 2. High-level internal structure

Chat decomposes into **tiers separated by their scaling profile** — this is the load-bearing
structural choice, because the connection/fan-out tier, the durable message store, and the
ephemeral presence path have *radically different* characteristics and must not share a substrate
(Phase-1 §5; ADR-04's firehose split). Architecture altitude only; component internals → Phase 4.

```
            ┌──────────────────────── CHAT SUBSYSTEM (inside a cell) ─────────────────────────┐
 clients    │                                                                                 │
 WS/SSE ───►│  ┌───────────────────────┐        ┌──────────────────────────────────────────┐ │
 CLI tail   │  │  CONNECTION GATEWAY    │  pub/  │  FAN-OUT / ROUTING TIER                  │ │
 ──────────►│  │  (stateful-ish edge:   │◄─sub──►│  channel→online-connection mapping;      │ │
            │  │   1 conn = 1 socket;   │backpl. │  read-fanout per channel; targeted       │ │
            │  │   backpressure, resync)│        │  write-fanout for mentions/unreads       │ │
            │  └───────────┬───────────┘         └───────────────┬──────────────────────────┘ │
            │              │ (presence/typing via FIREHOSE transport, never durable bus)       │
            │              ▼                                      ▼                             │
            │  ┌───────────────────────┐        ┌──────────────────────────────────────────┐ │
 HTTP/API ─►│  │  MESSAGE SERVICE       │───────►│  UNFURL SERVICE                          │ │
 (send,     │  │  (write path: validate,│        │  per-viewer permission-aware projection  │ │
 history,   │  │   authz, persist,      │        │  cache; invalidated by artifact events   │ │
 edit, …)   │  │   outbox-emit; content │        └───────────────┬──────────────────────────┘ │
            │  │   model = myelin-content)        ┌───────────────┴──────────────────────────┐ │
            │  └───────────┬───────────┘          │  READ-STATE SERVICE                      │ │
            │              │                       │  unread/mention counts, last-read marks; │ │
            │              ▼                       │  high-write, eventually-consistent       │ │
            │  ┌───────────────────────┐          └──────────────────────────────────────────┘ │
            │  │  DURABLE MESSAGE LOG    │   ┌──────────────────────────────────────────────┐  │
            │  │  per-conversation       │   │  MEMBERSHIP & CHANNEL METADATA (PG)          │  │
            │  │  append/range/tombstone │   │  channels, members, roles→tuples, prefs,     │  │
            │  │  (log/wide-column tier) │   │  linked-artifact backrefs, retention policy  │  │
            │  └───────────────────────┘   └──────────────────────────────────────────────┘  │
            └─────────────────────────────────────────────────────────────────────────────────┘
                │ outbox            ▲ consume                 ▲ check / list-objects
                ▼                   │                         │
            EVENT BUS ─────────► (Refs, Search, Notif, Agents, OLAP)        IDENTITY (ReBAC)
```

**The major components:**

1. **Connection Gateway.** Holds the millions of persistent client connections (WebSocket
   primary; SSE/long-poll fallback for restrictive networks). Responsibilities: auth handshake →
   resolve `Principal`; backpressure (per-connection bounded queues, drop-and-resync); resync-on-
   reconnect ("last id I have" → server streams the gap from the durable log). Stateful-ish but
   horizontally scalable; a connection can land on any node. *This is the component most likely
   to justify a language divergence — see §3.*

2. **Fan-out / Routing tier.** Maps `channel → online connections (across nodes)` and delivers a
   posted message to exactly those connections via a **pub/sub backplane** between gateway nodes.
   Implements the **read-fanout-per-channel default with targeted write-fanout for
   mentions/unreads** (Phase-1 §5.2): one ordered log per conversation that readers cursor into;
   "you were mentioned" is materialized. Handles hot/mega-channels specially (announcement
   channels with 100k members). The backplane choice (NATS/Redis-class vs channel-sharded actor
   model) is **[OPEN → P4]** (TE-21).

3. **Message Service (write path).** Validates and authorizes a send (`Id.check`), assigns a
   k-sortable ID + idempotency-dedupes on the client nonce, persists to the durable log, and
   **emits the event to the outbox in the same transaction** (ADR-04 §7.1). The body is the
   **shared `myelin-content` AST** (ADR-05) — `mention`/`artifact_ref`/`embed` are first-class
   nodes, not regex-parsed text. Attaches **agent authorship provenance** when the author is an
   agent (Phase-1 §2.3).

4. **Unfurl Service.** Resolves each `ArtifactRef` in a message to a **per-viewer** unfurl card by
   calling the target subsystem's projection API (never reading its DB), gated by `Id.check` for
   *that viewer* (Phase-1 §2.4, §5.4). Maintains a short-TTL projection cache **invalidated by the
   artifact's update events on the bus** (`issue.updated`, `pr.checks_completed`, …). Renders
   **live state** with a recorded "as of" snapshot for audit (the live-vs-snapshot resolution is
   §7; final call → P4). Unfurls are **actionable** (approve PR / change issue state / re-run CI
   from the card where permitted) — these actions dispatch through `EffectApi` like slash commands.

5. **Read-state Service.** Per-(user×conversation) and per-thread last-read + unread/mention
   counts. **High-write, eventually-consistent**, likely a separate fast KV store with batched
   writes (Phase-1 §2.5, §5.6) — a bad design here "melts the database."

6. **Durable Message Log.** Append-heavy, time-ordered, per-conversation, grows forever; supports
   range reads (recent N + scroll-back), tombstones, and resync. Wide-column (Cassandra/Scylla-
   class) is the directional candidate (ADR-14), with hot/cold tiering to object storage strongly
   suggested (Phase-1 §5.3). **The shard key encodes residency** (tenant+region — ADR-11).

7. **Membership & Channel Metadata.** Channels, members, roles, channel-scoped notification prefs,
   linked-artifact back-references, retention policy — OLTP (Postgres-class). Roles compile into
   **ReBAC tuples** (ADR-03): channel membership *is* the access-control relation for private
   channels.

8. **Presence/Typing path (not a durable component).** Ephemeral, high-frequency, low-value-if-
   lost; rides the **firehose transport** with TTLs, *never* the durable bus or durable storage
   (Phase-1 §2.6; ADR-04). Includes **agent presence** as its own class (available/busy/offline/
   rate-limited, tied to fabric health — Phase-1 §2.6).

---

## 3. Technology direction

**Default: Rust** (ADR-02, ADR-14), with **one flagged candidate divergence: the connection
tier**.

| Concern | Direction | Rationale / citation |
|---|---|---|
| **Message/unfurl/read-state services, fan-out core** | **Rust** | Hot paths stay Rust per ADR-02; memory-per-connection and no GC pauses are a *real* edge at millions of connections (Phase-1 §5.1). |
| **Connection tier language** | **Rust default, BEAM/Elixir (Phoenix Channels) a justified candidate** — **[OPEN → P4, TE-21]** | ADR-02 §directional names this as "the one language most likely to earn a divergence." Phoenix Channels is best-in-class for millions of soft-real-time connections; Rust async runtimes (tokio + an actor crate) are also strong. Phase 2 **does not pre-decide** — Chat's Phase-4 agent owns TE-21. **If it diverges, it still emits/consumes the Rust-defined envelope over the wire and implements `PersonalDataHolder`** (ADR-02 §consequences). |
| **Durable message log** | **Wide-column (Cassandra/Scylla-class)** candidate; PG-partitioned at smaller scale; hot/cold tiering to object store | ADR-10/ADR-14 log tier; Phase-1 §5.3. Residency in the shard key (ADR-11). |
| **Channel/membership metadata** | **Postgres-class** | ADR-10 OLTP tier; roles→ReBAC tuples (ADR-03). |
| **Pub/sub backplane (fan-out)** | NATS/Redis-class **or** channel-sharded actor model | Phase-1 §5.1; the firehose-class transport (ADR-04), distinct from the durable bus. **[OPEN → P4]**. |
| **Presence/typing transport** | Firehose transport (NATS/Redis-class), TTL'd | ADR-04 firehose split; Phase-1 §2.6. |
| **Read-state store** | Fast KV, batched writes, eventually consistent | Phase-1 §2.5, §5.6. **[OPEN → P4]**. |
| **Content model** | Shared **`myelin-content`** AST (ADR-05) | Mentions/refs/embeds as first-class nodes; one renderer across web/CLI/agents (Phase-1 §2.3). |
| **Frontend** | Open per VISION §4; TS/React-class baseline, against the **shared design language** | ADR-02 §frontend; design sketches precede UI (VISION §3). |

**Divergence justification posture (ADR-02 test).** A connection-tier divergence to BEAM would
be evaluated on: (a) does it buy a material capability Rust can't reasonably match for *this*
workload (massive soft-real-time fan-out — plausibly yes); (b) does it still implement the glue
across the language boundary (yes — it speaks the wire envelope + `ArtifactRef` + authz API, not
linked Rust crates); (c) does it preserve EU-deployability/self-hostability (yes, BEAM is self-
hostable). Phase 2 keeps this **open and honest**, not foreclosed.

---

## 4. Views / Screens the UI requires

Enumerated for the **shared design-language** catalogue and Phase-4 design sketches. Every screen
needs **empty / loading / error** states (VISION §3); a few state notes are called out where load-
bearing. UX is a first-class requirement; accessibility (keyboard-complete, screen-reader-correct,
RTL, localized for EU languages) is baseline, not afterthought (Phase-1 §3).

1. **App shell + Conversation list (sidebar).** Sections (channels, DMs, threads, mentions,
   saved), unread/mention badges, custom sections, drag-reorder, agent members visibly marked.
   *Empty:* "no channels yet — create or browse." *Loading:* skeleton list.
2. **Quick switcher / Command-K.** Jump-to-conversation + global command palette. Table stakes.
3. **Message timeline.** Virtualized infinite scroll (channels with millions of messages), date
   separators, grouped consecutive messages, jump-to-latest, **"new messages" divider**, jump-to-
   unread, scroll-to-reference. *Loading:* progressive backfill on scroll. *Error:* "couldn't load
   history — retry," with resync indicator on reconnect.
4. **Composer.** Rich-text over `myelin-content`; slash-command menu; **@-mention autocomplete**
   (humans + agents + teams + artifacts); **artifact-reference autocomplete** (`#ABC-` → issue
   suggestions; paste a PR URL → offer unfurl) — a *differentiating* surface backed by Search;
   code blocks w/ syntax; file upload/drag-drop; emoji; **draft persistence** (drafts are personal
   data — sync carefully). *Error:* send-failed with retry (idempotency-safe).
5. **Unfurl card.** The rich preview — visually excellent, **live, per-viewer permission-aware,
   collapsible, actionable**. *States:* loading skeleton; **"no access" redacted card** (viewer
   lacks permission — must *not* leak the title); "artifact deleted/erased" graceful degradation
   (tombstone); "as of" on hover.
6. **Thread pane.** Side-by-side/overlay; where agent and incident detail live; **agent
   "thinking/working" streaming state** rendered inline.
7. **Activity / Mentions inbox.** Everything aimed at *me* across channels (mentions, reactions,
   thread replies, agent approvals awaiting me).
8. **Search view.** Unified over messages + scoped to artifacts; ACL-filtered (you only find
   messages in channels you're in). *Empty:* zero-results vs no-query distinction.
9. **Member roster / presence.** Per channel; agents distinguished with their **agent presence
   class** (available/busy/offline/rate-limited).
10. **Channel detail / settings.** Topic, description, membership management, **linked artifacts**,
    notification prefs, **retention policy** (GDPR-relevant), **agent rules attached to the
    channel** (which triggers/agents act here).
11. **Notification preferences.** Per-channel/per-thread mute, keyword alerts, DND schedules,
    mobile vs desktop routing (*prefs* here; *delivery* is Notifications).
12. **Agent interaction affordances.** **Provenance popover** ("why did this agent post?" — which
    agent, on whose authority/lawful basis, triggered by which event); inline approve/edit/reject
    on **HITL approval cards** (the ADR-09 surface); feedback on agent output.
13. **DM / Group-DM views.** Reuse the conversation/timeline/composer machinery.
14. **Incident / "canvas" view** — **[UNCERTAIN / DEFER]**. Pinned structured summary tying
    artifacts together; overlaps Knowledge — boundary decision flagged, not a v1 commit.

---

## 5. CLI commands

Chat over CLI is valuable for engineers and **essential for scripting/agent-adjacent automation**
(VISION + Phase-1 §4). All authorize via the one `Principal`/token model (ADR-13); `--as <agent>`
requires the caller to be authorized to act as that agent. `--json` is available everywhere for
scripting. Surface (`myelin chat …`):

```bash
# Send (body from arg or stdin — pipe CI logs straight into a channel)
myelin chat send '#incidents' "main is red — investigating"
ci_output | myelin chat send '#ci-firehose' --thread 01HXY… --reply
myelin chat send '#deploys' "shipped v2.3" --as deploy-bot --attach ./release-notes.md

# Live tail — the CLI analogue of an open channel (WS/SSE stream to the terminal)
myelin chat tail '#incidents' --follow
myelin chat tail '#ci' --json | jq 'select(.mentions[]?.handle == "@me")'

# Read / history / read-state
myelin chat list                                  # channels I'm in
myelin chat history '#incidents' --since 2h --limit 200
myelin chat read '#incidents'                      # mark read

# Search (ACL-filtered; artifact-scoped filters)
myelin chat search "deploy failed" --in '#incidents' --refs issue,ci-run --since 7d

# Membership / lifecycle
myelin chat join '#design'      ;  myelin chat leave '#random'
myelin chat create '#release-2.4' --private --link myelin://…/issue/REL-240
myelin chat archive '#old-project'
myelin chat invite '#release-2.4' @alice @qa-team
myelin chat dm @bob "got a sec?"

# References, reactions, pins
myelin chat ref myelin://acme/issues/issue/ABC-123   # canonical ref string + unfurl preview
myelin chat react 01HXY… :white_check_mark:
myelin chat pin 01HXY… '#incidents'
myelin chat bookmark 01HXY…
```

**Slash-commands inside the composer** are a *distinct, server-side* surface (not the OS CLI):
`/create-issue`, `/ci rerun <run>`, `/assign <pr> <user>`, `/remind`, `/poll`, plus agent-invoking
commands. They dispatch into other subsystems **via the Event Bus / `EffectApi`** (Phase-1 §4),
authorized as the invoking `Principal`. Exact verb naming, output formats, and pagination cursors
are **[DEFER → P4]**.

---

## 6. Usage examples (end-to-end)

### 6.1 A reference becomes a live, permission-aware unfurl (the wedge, in chat)

**UI flow.** Alice types `Looking at ` then pastes a PR URL in `#release-2.4`. The composer's
artifact-reference autocomplete (Search-backed) offers an unfurl; she accepts. The message stores
an `artifact_ref(myelin://acme/git/pr/88)` node — **not a copy of the PR's content** (ADR-05).

**What happens.**
1. Message Service authorizes (`Id.check(alice, post, #release-2.4)`), persists, and **outbox-emits
   `chat.message.posted`** in the same transaction (ADR-04). The `artifact_ref` node also produces
   a **`ref.created` edge** (PR#88 ←"discussed in"→ #release-2.4) into Refs (ADR-13).
2. Fan-out delivers the message to online members of `#release-2.4`.
3. For **each viewer**, the Unfurl Service calls Git's projection API for PR#88 gated by *that
   viewer's* `Id.check`. Bob (a member who *can* see the PR) gets a live card: title, status
   checks, reviewers, an **"Approve" action**. Carol (a channel member who *cannot* see the
   private repo) gets a **redacted "no access" card** — the title never leaks (Phase-1 §2.4, §5.4;
   ADR-03 permission-aware reads).
4. Later, PR#88's checks turn green → Git emits `pr.checks_completed` → Chat **consumes it to bust
   the unfurl cache** → every viewer's card updates live (ADR-04; §7.2 of the overview).
5. Bob clicks **Approve** on the card → dispatched through `EffectApi` (validated vs permissions),
   exactly like an agent effect (ADR-08).

**Shared systems exercised:** Id (per-viewer authz + redaction), Refs (the back-edge), Bus (emit +
unfurl invalidation), Git projection API, Search (the autocomplete). *No subsystem read another's
DB.*

### 6.2 CI fails → agent triages → posts to chat → proposes a fix behind a HITL gate

This is the agent-native flagship (`system-overview.md §8.2`), seen from Chat's seat.

**Flow.**
1. `ci.pipeline.failed` on the bus wakes `MockTriageAgent`. Plan-then-apply: it **proposes**
   effects `issue.create`, `ref.create×2`, `chat.post` — performs no side effects (ADR-08).
2. `EffectApi` validates and applies; the `chat.post` lands as a message **authored by the agent
   `Principal`**, carrying full provenance (which agent, on-behalf-of the pusher, triggered by
   which event). The UI shows an **agent badge + provenance popover**: `🔴 main red — opened
   ISSUE-412, triaging`.
3. A second trigger wakes `FixAgent`, which proposes `git.open_pr` — **sensitive on a protected
   repo** → `Id` gates it → **HITL required**. The Durable-workflow engine (ADR-09) opens a gate
   and renders an **approval card in chat**: *"FixAgent proposes PR #88 — Approve / Edit / Reject."*
4. The card is a durable workflow **wait** — it can sit for minutes or days. A human clicks
   **Approve** in chat → workflow signal → `git.open_pr` applies → PR#88 opens, announced back in
   the thread.

**Chat's role:** it is the *attention surface and the human-decision surface* — it renders the
agent's posts (as a first-class actor) and the HITL approval card, and routes the human's decision
back as a workflow signal. **Loop/runaway protection** (hop counters on agent-originated chains,
per-agent quotas, kill switches) is enforced jointly by the fabric and Chat (Phase-1 §5.8, §6.3;
ADR-08 §safety).

### 6.3 Ops: pipe a failing run's logs into an incident channel from the CLI

```bash
# In a CI job's failure hook:
tail -n 40 build.log | myelin chat send '#incidents' \
  --thread "$INCIDENT_THREAD" --reply --as ci-bot
```
The body is structured (`code` block in `myelin-content`); `--as ci-bot` is authorized because the
job token is allowed to act as that agent. The message emits the same `chat.message.posted` event,
so an `@oncall`-mention trigger can page via Notifications.

---

## 7. Interactions with other subsystems & shared systems

### 7.1 Events EMITTED (canonical envelope, transactional outbox — ADR-04, ADR-13)
Exact dotted names are a **Phase-3 taxonomy deliverable**; the *envelope shape* is the contract.
Illustrative (Phase-1 §6.1):
- `chat.message.posted` / `.edited` / `.deleted` — carry conversation, author (`Principal`),
  thread, **mention + `artifact_ref` nodes**, `tenant`, `region`, `visibility`,
  `contains_personal_data`.
- `chat.message.mention` — **the primary agent trigger** ("@deploy-bot ship this"); carries enough
  context to act *and to authorize*.
- `chat.reaction.added/.removed` — lightweight triggers (✅ approves something).
- `chat.thread.created` / `.reply`; `chat.channel.created/.archived/.member_added/.member_removed`.
- `chat.artifact.referenced` — **feeds Refs** and notifies the artifact's watchers (a top cross-
  subsystem signal).
- `chat.slash_command.invoked` — dispatches into other subsystems/agents.
- **NOT on the durable bus:** `chat.presence.*`, `chat.typing.*`, `chat.read.*` — firehose only
  (ADR-04).

### 7.2 Events CONSUMED (to render / route / invalidate unfurls — Phase-1 §6.2)
- **Issues:** `issue.created/updated/state_changed/assigned/commented` → channel posts + unfurl
  invalidation.
- **Git:** `pr.opened/updated/review_requested/merged`, `commit.pushed`, `pr.checks_completed` →
  unfurl invalidation + posts.
- **CI:** `ci.run.started/failed/succeeded` → post to / page an incident channel; invalidate run-
  status unfurls.
- **Knowledge:** `doc.updated/published/commented` → unfurl invalidation + notify.
- **Identity:** `user.deactivated`, `user.erased`, `team.membership_changed`, `permission.changed`
  → membership/visibility/unfurl recomputation, access removal, and the **GDPR erasure cascade**.
- **Agent Fabric:** `agent.message.partial` (streaming output into a thread), `agent.status_changed`
  (presence), `agent.action_proposed/awaiting_approval` (render the approval card).

### 7.3 Authz (ADR-03)
Every send/read/unfurl/search resolves to `Id.check` or, for *lists/feeds/search/unfurls*,
`Id.list-objects` (pre-filter, never post-filter — no N+1, no leak). **Channel membership compiles
into ReBAC tuples**: private-channel membership *is* the visibility relation; a private-channel
mention must **not** leak to users who can see the referenced issue but not the channel (Phase-1
§2.4 back-reference caveat).

### 7.4 Refs (ADR-13) — densest consumer + heavy producer
Every `artifact_ref` node → a `ref.created` edge ("discussed in #channel"); every unfurl → a Refs
read, resolved **per viewer**. Edges tombstone gracefully on erasure.

### 7.5 Search (ADR-03, ADR-10)
Chat states its needs (engine choice is Search's): ACL-filtered at query time, near-real-time
indexing off the bus, multilingual analyzers (EU languages), artifact cross-search, and the
composer's artifact-reference autocomplete.

### 7.6 Notifications (ADR-12)
Chat *produces* notifiable events (mentions, DMs, keyword alerts) + holds channel-scoped *prefs*;
Notifications owns delivery, storm-control, dedup, DND/digests.

### 7.7 Agent Fabric (ADR-08) — tools & triggers Chat registers
- **`ToolDef`s** (typed, into the shared `ToolSurface`, MCP-exposable later): `chat.post`,
  `chat.reply_in_thread`, `chat.react`, `chat.create_channel`, `chat.invite`,
  `chat.start_dm` — each with required caps + side-effecting flag, applied only via `EffectApi`.
- **Trigger surface:** mentions of an agent, reactions on its messages, messages in a watched
  channel — all expressed as `EventMatcher` (query AST, ADR-07) over Chat events.
- **Streaming post-back** (`agent.message.partial`) and **agent presence** are first-class.
- **Loop/quota safety** is co-owned (hop counters on agent-originated chains, per-agent quotas,
  kill switches) — Chat is where agent↔agent fan-out bombs would manifest (Phase-1 §5.8).

### 7.8 GDPR / PersonalDataHolder duties (ADR-12) — Chat is the hardest holder
Free-text bodies are pervasive PII *about other people* (Phase-1 §7). Chat implements
`locate/export/rectify/restrict/erase`:
- **Crypto-shred + tombstone** for authored content (per-tenant, optionally per-subject keys);
  deleting the key renders bodies in the log/backups/cold tier unrecoverable without rewriting
  immutable structures (ADR-12 §3).
- **Mention neutralization across the corpus + search index** — tractable *because* mentions are
  structured nodes with stable IDs (Phase-1 §7; ADR-05).
- **Erasure cascades to derived data:** search indices, unfurl caches, read-state, notification
  history, Refs edges, cold/archive tiers, backups.
- **Live unfurls + ephemeral caches** are favored over durable snapshots precisely so a later-
  erased third party's data isn't frozen in a card (Phase-1 §7; §7 below).
- **Lawful-basis/provenance on every message** (esp. agent-authored) for Art. 30 RoPA; **agents
  reading a channel is processing** and is audited.
- **Per-channel/per-org retention** (auto-delete after N days) must purge from *all* derived
  stores.

---

## 8. Changes implied in the shared systems (flag for Phase 3)

These are Chat's asks; Phase 3 owns the mechanism. None contradict the ADRs — they *instantiate*
deferred items.

1. **Firehose transport must be a real, separate path (ADR-04).** Chat's presence/typing/read-state
   are the canonical firehose load. Phase 3 must select a low-latency fan-out transport with TTLs,
   distinct from the durable bus, EU-deployable. The fan-out backplane between gateway nodes is
   firehose-class. (TE-9/11.)
2. **Authz needs a *cheap, cached, per-viewer* `check`/`list-objects` for unfurls (ADR-03).** Per-
   viewer permission-aware unfurl resolution is "the subtlety that separates a real implementation
   from a demo" (Phase-1 §5.4). Phase 3's consistency-token + caching design must make N-viewers ×
   M-refs decisions affordable. **Channel-membership-as-tuple** and "membership ≈ permission"
   precompute classes are explicit asks.
3. **Bus must carry artifact `*.updated` events Chat can subscribe to for unfurl-cache busting**
   (ADR-04 + the projection contract, ADR-13). The "data available/updated pointer" semantics must
   include enough to invalidate a specific `ArtifactRef`'s cache.
4. **Refs must support per-viewer-filtered backlink reads and graceful tombstoning** (ADR-13) at
   the density Chat generates — and **not leak private-channel references** to users who can see
   the artifact but not the channel (cross-tenant/visibility gating, ADR-13 open item).
5. **GDPR/KMS must support crypto-shred granular enough for message bodies and per-subject mention
   neutralization** (ADR-12; GD-4 granularity), plus retention-engine hooks for per-channel
   retention. Chat is the stress test for the holder spine.
6. **Agent Fabric / durable-workflow must surface HITL approval cards *into chat* as the canonical
   approval UX** (ADR-08, ADR-09) — the data model for the approval card + signal round-trip is a
   joint Chat↔Fabric↔Workflow design.
7. **Notifications must accept channel-scoped preference state owned by Chat** and honor
   DND/keyword/mute (ADR-12).
8. **Storage log/firehose tier must be residency-pinned with the shard key encoding tenant+region**
   (ADR-10, ADR-11) and support hot/cold tiering for an infinitely-growing per-conversation log.

---

## 9. Open questions for Phase 4 (Chat detailed architecture)

Carried forward from Phase-1 §9 and ADR-15's `[OPEN → P4]`, scoped to Chat:

1. **Connection-tier transport + language (TE-21).** Rust async+actor vs BEAM/Phoenix Channels;
   which pub/sub backplane (NATS/Redis-class vs channel-sharded actor). *The biggest single Chat
   decision; Phase 2 leaves it open with a written candidate.*
2. **Message store substrate & tiering.** Wide-column vs PG-partitioned vs custom log; hot/cold
   split; exactly how the residency requirement shapes the shard key.
3. **Write-fanout vs read-fanout boundary.** Read-fanout for bodies is the default; the precise
   line for mentions/unreads (targeted write-fanout) is unsettled.
4. **Unfurl live-vs-snapshot semantics** and the concrete design that makes per-viewer permission-
   aware unfurl resolution cheap without leaking — *likely the trickiest correctness problem.*
5. **Erasure mechanism specifics.** Crypto-shred vs tombstone vs anonymize per case; the backup-
   erasure story; audit-immutability-vs-erasure tension (with `[OPEN — LEGAL]` GD-1/2/6).
6. **Group DM vs private channel** — unify or keep both (model + product decision).
7. **Threads UX model** — threads-first vs channel-first; "also send to channel" or not.
8. **Agent presence & streaming semantics** — what "online" means for an agent; how streaming
   partial output renders and reconciles in a thread.
9. **Agent loop/abuse prevention** — hop counters, quotas, kill switches; co-designed with the
   fabric (AG-4 adversarial validation is a P3/P5 concern; the chat-side mechanism is P4).
10. **Multi-region/residency vs latency** for EU-pinned tenants with global users — edge connection
    nodes everywhere, reads/writes routed home; intersects the multi-cell-tenant unknown (SC-2/3,
    `[OPEN → P3]`).
11. **"Canvas"/pinned-summary boundary with Knowledge** — build in Chat, embed Knowledge, or skip.
12. **Cross-org / federated channels** — deferred, but the conversation/membership model must not
    foreclose it (deep identity/residency/erasure implications).
13. **Voice/video** — out for v1; if ever, embed an EU-sovereign third party.

---

## 10. Cross-references
- [`VISION.md`](../../../VISION.md) — non-negotiables (agent-native, GDPR/EU-sovereign, world-
  scale, top-tier UX, Rust default).
- [`../architecture-decisions.md`](../architecture-decisions.md) — ADR-02 (language/TE-21),
  ADR-03 (ReBAC/unfurl authz), ADR-04 (bus + firehose split), ADR-05 (content model), ADR-08
  (agent fabric), ADR-09 (HITL/durable-workflow), ADR-10/14 (storage tiers), ADR-11 (cells/
  residency), ADR-12 (GDPR holder), ADR-13 (glue contracts).
- [`../system-overview.md`](../system-overview.md) — §4 (Chat owns/delegates), §8.1 (PR pane /
  unfurl wedge), §8.2 (CI→agent→chat HITL flagship), §8.3 (DSAR fan-out).
- [`../../01-research/subsystem-deep-dives/chat.md`](../../01-research/subsystem-deep-dives/chat.md)
  — the Phase-1 territory map this builds on.
