# Chat — Research Deep-Dive

> Phase 1 research. This maps the territory for the Chat subsystem; it is **not** the
> final architecture. It is written to be consumed by `02-holistic-architecture` and
> `04-subsystem-architectures/chat`. Uncertainty is flagged inline as **[UNCERTAIN]**,
> assumptions as **[ASSUMPTION]**, deferrals as **[DEFER]**.

---

## 1. Purpose & role in Myelin

Chat is the **synchronous and semi-synchronous conversation layer** of Myelin. Its job is
not to be "a Slack clone bolted onto a dev tool" — it is to be the place where the
*activity* of the other four subsystems becomes *conversation*, and where conversation
becomes *action* on those subsystems. It is the most natural home for the platform's
two strongest differentiators:

1. **Cross-artifact reference.** Any commit, PR, issue, knowledge doc, database row, or
   CI run can be referenced in a message and rendered as a rich, live **unfurl**. Chat is
   the densest consumer of the platform's **cross-artifact reference graph** (a shared
   system). Conversely, every reference made *in* chat is an edge that should be written
   *back* into that graph (e.g. "this incident channel discussed PR #482"), so the graph
   is bidirectional.
2. **Humans and agents in the same channels.** Agents are first-class participants, not
   webhooks posting into a #bots channel. An agent can be @-mentioned, can hold a
   conversation thread, can be a channel member, can react, and can act on artifacts it is
   discussing. This is the most visible surface of the "agent-native" principle, so the
   message/membership model must treat an agent identity and a human identity as the same
   kind of thing wherever possible (see §2, §6).

Role relative to the other subsystems:

- **Issue tracker / Git / CI / Knowledge** *emit* events; Chat is a primary *renderer* and
  *router* of those events into human/agent attention (channels, threads, notifications).
- Chat is also an *input device*: a message can trigger an action (create an issue from a
  message, kick a CI re-run, assign a PR) via slash commands or agent mediation.
- Chat is where **incident/coordination workflows** live: the natural locus for "war room"
  channels that thread together a failing CI run, the offending commit, the issue, and the
  doc post-mortem.

A useful framing: **Chat is the human-attention bus that sits in front of the machine event
bus.** Not every event becomes a message; the routing/filtering logic (what surfaces, to
whom, where) is a core part of this subsystem's value and difficulty.

### What Chat is explicitly NOT (scope boundaries)
- Not the notification *delivery* system itself (email/push/web) — that is a **shared
  Notifications system**. Chat *produces* notifiable events and *consumes* a delivery API.
- Not the artifact store for docs/code — it *references* them.
- Not the identity/permission authority — it *consumes* the shared identity & access model.
- **[ASSUMPTION]** Voice/video calls are **out of scope for v1** and likely permanently
  deferred or delegated to an embeddable EU-sovereign third party (e.g. self-hosted
  Jitsi/LiveKit). Flagged because corporate buyers will ask. **[DEFER]**

---

## 2. Core domain concepts & data model considerations

This section maps concepts, not final schemas. The hard tension throughout is between a
**simple mental model** (good UX) and **world-scale + GDPR + agents** (hard constraints).

### 2.1 Spaces / Workspaces / Tenancy
- Myelin is multi-tenant. **[ASSUMPTION]** Chat lives inside an **organization/tenant**
  boundary that is shared platform-wide (the same org that owns repos/issues/docs). A
  channel belongs to an org; cross-org channels (shared channels, "Slack Connect"-style)
  are a known-hard later feature — **[DEFER]** but call out that federation has deep
  identity, residency, and erasure implications, so the data model should not *assume*
  single-org membership forever.

### 2.2 Conversation containers
The taxonomy that virtually all serious chat products converge on:

- **Channel** — named, possibly topic-scoped, with a membership model. Sub-kinds:
  - *Public* (discoverable + joinable by anyone in the org/space).
  - *Private* (invite-only; membership is the access control).
  - *Shared/linked-to-artifact* — e.g. a channel auto-created for an incident, a release,
    a sprint, or a repo. This is a Myelin-specific lever: channels that are *born from*
    artifacts and carry a back-reference.
  - *Org-wide / announcement* — broadcast-style, restricted posting.
- **Direct Message (DM)** — 1:1. Special-case of a group conversation with exactly 2
  members; worth modeling as a "conversation" so DMs and group-DMs share machinery.
- **Group DM** — small (N ≤ ~8–10) ad-hoc multi-party, no name, membership == the set of
  people. The boundary between "group DM" and "private channel" is a product decision;
  **[UNCERTAIN]** whether to even keep both (Slack keeps both; some products collapse them).
- **Thread** — a reply-tree hanging off a root message inside a channel/DM. Critical for
  keeping agent verbosity and incident detail out of the main timeline. See §2.5.

**[ASSUMPTION]** Internally these should likely all be one entity ("conversation") with a
`kind` and a membership strategy, rather than separate tables, to avoid duplicating
read/write/fan-out logic.

### 2.3 Message model (the core record)
A message minimally needs:
- `id` — globally sortable, time-ordered ID. **[ASSUMPTION]** Use a Snowflake/ULID-style
  k-sortable ID so ordering is intrinsic and cursoring/pagination is cheap. **Do not**
  rely on wall-clock timestamps for ordering (clock skew at scale). Note ULID/UUIDv7 leak
  creation time — fine here, but relevant to GDPR minimization discussions.
- `conversation_id`, `author_id` (human **or** agent — same field, polymorphic actor),
  `thread_root_id` (nullable), `created_at`, `edited_at`, `deleted_at` (soft-delete /
  tombstone — see GDPR §7).
- **Rich content body.** Strong recommendation: store a **structured document** (a JSON/AST
  block model), not a raw markdown string or HTML. Reasons: consistent rendering across
  web/CLI/mobile/agents; safe; lets mentions, artifact-refs, code blocks, and unfurl
  placeholders be *first-class nodes* rather than regex-parsed text. This aligns the
  message model with the Knowledge platform's block model — **[ASSUMPTION]** they should
  share a content/block representation where possible (shared system candidate: a
  "rich content" library). Worth a cross-subsystem alignment note.
- `attachments` (references to shared Storage blobs), `reactions`, `pinned`, `edit_history`
  (audit), `metadata` (e.g. "posted by agent X on behalf of rule Y").
- **Idempotency key / client nonce** — to dedupe retried sends (mobile/flaky networks,
  and *especially* agents that may retry). Non-negotiable at scale.
- **Authorship provenance for agents.** When an agent posts, the message must carry: which
  agent identity, on whose authority/lawful basis, triggered by which event, and a clear
  visual "agent" badge. This provenance is both a UX requirement and an audit/GDPR one.

### 2.4 Mentions, references & unfurls (the differentiator)
- **Mentions:** `@user`, `@agent`, `@channel`/`@here` (broadcast classes), `@role`/`@team`
  (resolve via shared identity groups). Each mention is a structured node carrying a stable
  ID, so it survives rename and drives notification routing.
- **Artifact references:** a message can embed a reference to *any* artifact via a stable
  URI/handle (e.g. `myelin://issue/ABC-123`, `myelin://repo/x/pr/482`,
  `myelin://ci/run/...`, `myelin://doc/...`, `myelin://doc/db/row/...`). Rendering rules:
  - The reference node stores the **canonical artifact handle**, not a copy of its content.
  - An **unfurl** is a *projection* of that artifact, fetched at render time (or cached),
    showing live state: PR title + status checks + reviewers; issue title + state +
    assignee; CI run status + duration + failing step; doc title + breadcrumb; commit
    message + author + diffstat.
  - **Live vs snapshot tension:** does the unfurl show *current* state or *state at time of
    posting*? Slack/GitHub show current state and it's often confusing. **[UNCERTAIN]** —
    likely answer: show live state but record the snapshot for audit/erasure; let the user
    see "as of" on hover. This needs a product decision and has GDPR implications (a snapshot
    may embed personal data of someone since erased — see §7).
  - **Permission-aware unfurls:** the unfurl must be rendered *per viewer*. If viewer A can
    see the PR and viewer B cannot, B must see a redacted/"no access" card, not the title.
    This means unfurl resolution is an **authorization-sensitive, per-recipient** operation —
    a real performance and correctness problem at scale (see §5). This is THE subtlety
    that separates a real implementation from a demo.
- **Back-references:** posting a reference should (optionally, per type) write an edge into
  the shared reference graph so the artifact "knows" it was discussed. e.g. an issue shows
  "mentioned in 3 conversations." Requires care to avoid notification spam and to respect
  visibility (a private-channel mention should not leak to users who can see the issue but
  not the channel).

### 2.5 Threads, ordering & read state
- **Threads** are essential here because agents are verbose and incidents are detailed.
  Model a thread as messages sharing a `thread_root_id`. Decide: are threaded replies *also*
  in the main timeline ("also send to channel")? Slack made this optional and it's a
  perennial UX wart. **[UNCERTAIN]** — recommend threads-first with explicit "broadcast"
  rather than the inverse.
- **Read state / unreads** is deceptively expensive: per-(user × conversation) last-read
  marker, plus per-thread read state, plus mention counts. At world scale this is a
  high-write, high-read hot path (every scroll updates it). Often a separate fast store.
  **[ASSUMPTION]** read-state and "unread counts" are a distinct, eventually-consistent
  subsystem from message storage.
- **Ordering & history** must be stable and gap-free for a given conversation. Editing and
  deletion produce *new versions/tombstones*, not in-place mutation, to keep replicas and
  clients consistent and to satisfy audit.

### 2.6 Presence & typing
- **Presence** (online/away/offline/dnd) and **typing indicators** are *ephemeral,
  high-frequency, low-value-if-lost* signals. They should **not** go through durable
  message storage or the durable event bus. **[ASSUMPTION]** Handle via a dedicated
  in-memory/pub-sub channel (e.g. Redis pub/sub, NATS, or gossip) with TTLs.
- Presence at world scale is a known fan-out nightmare: N users each subscribed to the
  presence of M others is O(N×M) updates. Mitigations: only compute presence for
  *currently-visible* members, subscribe lazily, coarsen state, debounce. Flag as a
  scale problem (§5), not solved here.
- **Agent presence** is a genuinely new design question: is an agent "online" when its
  worker pool is healthy? Should agents show a distinct presence class ("available /
  busy / offline / rate-limited")? **[UNCERTAIN]** — likely yes; agents need their own
  presence semantics tied to the agent fabric's health, not human-style idle timers.

### 2.7 Reactions, pins, bookmarks, saved items, drafts
Low-glory but expected. Reactions are high-cardinality writes (emoji storms) and need
aggregation. Drafts may sync across devices (privacy-relevant: drafts are personal data).
Saved/bookmarked items are per-user. All straightforward but must be in the model.

### 2.8 Actors: unified human/agent identity
The single most important modeling decision: **an "actor" (message author, channel member,
mention target, reactor) can be a human or an agent, and the model should treat them
uniformly** wherever access control and rendering allow. Differences (badge, provenance,
presence semantics, permission/lawful-basis) are *attributes of the actor*, not *separate
code paths*. This is the data-model expression of the VISION's agent-native principle and
the strategy-pattern steer: the message pipeline shouldn't branch on "is this a bot."

---

## 3. Key UX / views required

Breadth list; depth where it's load-bearing. UX is a first-class requirement per VISION.

- **Channel/conversation list (sidebar):** sections (channels, DMs, threads, mentions,
  saved), unread/mention badges, custom sections, drag-reorder, search-as-you-type jump
  (a "quick switcher" / command-K is table stakes for a top-tier product).
- **Message timeline view:** virtualized infinite scroll (must handle channels with
  millions of messages), date separators, grouped consecutive messages, jump-to-latest,
  jump-to-unread, "new messages" divider, scroll-to-reference.
- **Composer:** rich text with slash-commands, @-mention autocomplete (humans + agents +
  teams + artifacts), artifact-reference autocomplete (type `#ABC-` and get issue
  suggestions, paste a PR URL and it offers an unfurl), code-block with syntax, file
  upload/drag-drop, emoji, draft persistence, markdown shortcuts. The artifact-reference
  autocomplete is a *differentiating* UX surface and depends on shared search.
- **Thread pane:** side-by-side or overlay; the place most agent/incident detail lives.
- **Unfurl cards:** the rich previews — must be visually excellent, live, permission-aware,
  collapsible, and *actionable* (approve PR, change issue state, re-run CI directly from the
  card where permitted). Inline actions in unfurls are a major UX/agent integration point.
- **Mentions & reactions inbox / "Activity" view:** everything aimed at me across channels.
- **Search view:** unified search over messages + scoped to artifacts (see §5/§dependencies).
- **Presence & member roster** per channel; agent members visibly distinguished.
- **Channel detail / settings:** topic, description, membership, linked artifacts,
  notification prefs, retention policy (GDPR-relevant), agent rules attached to the channel.
- **Notification preferences:** per-channel/per-thread mute, keyword alerts, schedules
  (DND), mobile vs desktop routing. (Delivery is the shared Notifications system; *prefs*
  partly live here.)
- **Agent interaction affordances:** a way to see *why* an agent posted (provenance popover),
  to give an agent feedback/approval inline, and to see an agent's "thinking/working" state
  in a thread (streaming partial output — agents may stream like LLMs do).
- **Incident/"canvas" view [UNCERTAIN/DEFER]:** a pinned structured summary at the top of a
  channel (Slack "canvas"-like) that ties the artifacts together. Strong fit for Myelin but
  overlaps the Knowledge platform — flag the boundary.
- **Accessibility & i18n** (EU = many languages, legal accessibility requirements):
  keyboard-complete, screen-reader-correct, RTL, localized — first-class, not afterthought.

---

## 4. CLI commands expected

Myelin clearly intends a CLI (VISION + Rust steer). Chat over CLI is valuable for engineers
and *essential for scripting/agent-adjacent automation*. Indicative surface (`myelin chat …`):

- `myelin chat send <channel> "<message>"` — with `--thread <id>`, `--reply`, `--attach`,
  reading body from stdin (pipe CI logs into a channel), `--as <agent>` (with auth).
- `myelin chat tail <channel>` / `--follow` — stream messages live (websocket/SSE) to the
  terminal; the CLI analogue of an open channel. Great for ops.
- `myelin chat list` (channels), `myelin chat search "<query>"` (with artifact filters).
- `myelin chat read <channel>` / mark-read, `myelin chat history <channel> --since/--limit`.
- `myelin chat join/leave/create/archive <channel>`, `myelin chat invite/kick`.
- `myelin chat dm <user> "<msg>"`.
- `myelin chat ref <artifact-handle>` — produce the canonical reference string / unfurl
  preview for embedding (handy in scripts and other CLIs).
- `myelin chat react <message-id> :emoji:`, `pin`, `bookmark`.
- **Slash-commands inside the composer** (server-side, not OS CLI) — distinct surface:
  `/create-issue`, `/ci rerun`, `/assign`, `/remind`, `/poll`, `/giphy`-style, plus
  agent-invoking commands. These dispatch into other subsystems via the event bus.
- **[ASSUMPTION]** A consistent auth model: CLI uses platform tokens (incl. scoped agent
  tokens). `--as <agent>` requires the caller to be authorized to act as that agent.
- **[DEFER]** exact verb naming, output formats (`--json` everywhere for scripting),
  pagination cursors — architecture-phase concerns.

---

## 5. Hardest technical problems for WORLD-SCALE

This is the core of the research. Chat is one of the *hardest* systems to scale because it
combines high fan-out, low-latency real-time delivery, huge write volume, and rich
per-viewer rendering. Ordered roughly by difficulty/importance.

### 5.1 Real-time fan-out & the connection layer
- Every connected client holds a **persistent connection** (WebSocket; SSE/long-poll as
  fallback for restrictive networks). World scale = **millions of concurrent long-lived
  connections**. Each connection consumes memory + a file descriptor + must be routed to.
- The hard part is the **routing/fan-out tier**: when a message is posted to a channel with
  K online members spread across many connection-server nodes, the message must reach
  exactly those K connections, fast. This requires a **pub/sub backplane** between the
  stateless-ish connection nodes and a mapping of *which node holds which subscriptions*.
- Patterns to evaluate in architecture phase: connection gateways + an internal pub/sub
  (Redis cluster / NATS / Kafka-for-durable + lightweight bus for fan-out), or a
  channel-sharded actor model (Rust + something Elixir/Phoenix-Channels-like; Rust steer
  suggests building this on async runtime + actor crate or a NATS/Redis backplane).
  **[UNCERTAIN]** which backplane; this is a major architecture decision, explicitly
  deferred. Note: Rust gives a real edge for millions of connections (memory per conn,
  no GC pauses) — supports the VISION's Rust steer.
- **Hot channels:** an org-wide announcement channel with 100k members is a fan-out spike.
  Mega-fan-out needs special handling (read-fanout vs write-fanout decision, below).
- **Backpressure:** slow/abusive clients must not stall the node. Per-connection bounded
  queues, drop-and-resync semantics.

### 5.2 Write-fanout vs read-fanout (the timeline materialization question)
- **Write-fanout ("fan-out on write"):** copy each message into each recipient's inbox/feed.
  Cheap reads, expensive + amplifying writes; brutal for huge channels (the "celebrity"
  problem from social feeds).
- **Read-fanout ("fan-out on read"):** store the message once per channel; readers pull.
  Cheap writes, more expensive reads; needs good per-channel indexing and read-state.
- Chat is **channel-centric**, so **read-fanout per channel is the natural default**: one
  ordered log per conversation, readers cursor into it; *unread counts / mention inboxes*
  are the parts that may need selective write-fanout (materialize "you were mentioned").
  **[ASSUMPTION]** hybrid: read-fanout for message bodies, targeted write-fanout for
  mentions/notifications. Defer the exact split.

### 5.3 Storage model for messages
- Append-heavy, time-ordered, per-conversation. Needs: cheap appends, ordered range reads
  (recent N, scroll back), edits/tombstones, and *enormous* total volume that grows forever.
- Candidate shapes (architecture-phase): a partitioned/sharded log keyed by conversation
  (Cassandra/Scylla-style wide rows; or Postgres with partitioning per tenant/time; or a
  purpose-built log). Multi-tenant + EU-residency (§7) pushes toward **shardable by tenant
  with region pinning**. **[UNCERTAIN]** single store vs hot/cold tiering (recent in fast
  store, archived in cheap object storage). Strongly suggest **tiering** given infinite
  growth.
- **Tenancy + residency interact with sharding:** a German customer's messages may have to
  physically live in EU/Germany. Sharding key must encode residency. This constrains the
  storage choice more than raw scale does.

### 5.4 Per-viewer, permission-aware unfurl resolution at scale
- As noted in §2.4, an unfurl's content depends on *both* the artifact's live state *and*
  the viewing user's permissions on that artifact. Naïvely this is an N-recipients ×
  M-references authorization+fetch explosion on every render.
- Mitigations to research: cache artifact projections with short TTL + invalidate on the
  artifact's update events (the event bus helps here — Chat *consumes* "issue.updated" to
  bust unfurl caches); resolve permission via a fast authorization service / cached
  decision; render lazily (only unfurl what's on screen); precompute per-channel
  visibility classes where membership ≈ permission. This is a genuinely hard correctness
  *and* performance problem and a likely source of GDPR leaks if done wrong.

### 5.5 Search at scale
- Full-text search over an ever-growing, multi-tenant, permission-scoped message corpus,
  *plus* the ability to also surface referenced artifacts. Must be **permission-filtered at
  query time** (you can only find messages in channels you're in) — ACL-aware search is
  hard and a classic leak vector. Likely a shared Search system (Elastic/OpenSearch/Tantivy
  [Rust]/Meilisearch class) with per-tenant indices and residency-aware placement.
  **[DEFER]** engine choice to the shared-systems architecture; Chat just states its needs:
  ACL-filtered, near-real-time indexing, multilingual analyzers (EU languages), artifact
  cross-search.

### 5.6 Read-state / unread counts at scale
- Already noted (§2.5): extreme write rate (every view updates a marker), must be fast and
  may be eventually consistent. A bad design here melts the database. Often a separate
  key-value store with per-user batched writes.

### 5.7 Ordering, consistency & delivery guarantees
- Users expect **per-conversation total order** and **no lost messages** when online; "at
  least once" + client-side dedup (via the idempotency key) is the pragmatic target rather
  than exactly-once. Need **resync-on-reconnect**: client says "last id I have," server
  streams the gap (this is where the durable per-channel log pays off).
- **Causal anomalies** (edit arrives before the message it edits; reaction before message)
  must be handled gracefully on the client.

### 5.8 Abuse, rate-limiting & agent-induced load
- Agents can generate message volume far beyond humans. **Rate-limiting and quotas per
  actor (esp. agents)** are a first-class scaling AND safety concern: a misbehaving agent
  rule could fan-out-bomb a channel. Need circuit breakers, per-agent quotas, and "agent
  loop detection" (agent A replies to agent B replies to agent A…). **This is novel and
  important** given agent-native ambitions — flag strongly. (See also §6 event-loop risk.)

### 5.9 Multi-region, residency & latency
- EU-sovereign + world-scale = users in many regions, but data possibly pinned to EU. A
  user in another region talking in an EU-resident org's channel has a latency/residency
  tension. **[UNCERTAIN]** how aggressively to geo-replicate vs accept latency for
  residency. Likely: tenant's data has a home region; edge connection nodes everywhere;
  reads/writes routed home. This is one of the deepest open architecture questions and
  intersects every other subsystem.

---

## 6. Events EMITTED and CONSUMED (event bus / agent fabric)

Chat is both a heavy producer and heavy consumer on the shared event bus. **[ASSUMPTION]**
events are typed, versioned, carry actor + tenant + lawful-basis/provenance metadata, and
agent triggers subscribe to them via the agent fabric (strategy-pattern: a trigger is a
subscription + a handler; mock handlers in dev). Naming below is illustrative.

### 6.1 Events Chat EMITS (others / agents consume)
- `chat.message.created` / `.edited` / `.deleted` (carry conversation, author, thread,
  references, mentions, tenant, visibility/ACL context).
- `chat.message.mention` — a specific actor (human/agent/team) was mentioned. **This is the
  primary agent trigger:** "@deploy-bot ship this" → an agent subscribes to mentions of
  itself. Must carry enough context for the agent to act and to check authorization.
- `chat.reaction.added` / `.removed` (lightweight triggers, e.g. ✅ approves something).
- `chat.thread.created`, `chat.thread.reply`.
- `chat.channel.created` / `.archived` / `.member_added` / `.member_removed`.
- `chat.artifact.referenced` — a message referenced artifact X (feeds the reference graph
  and notifies the artifact's watchers). One of the most valuable cross-subsystem signals.
- `chat.slash_command.invoked` — dispatches into other subsystems / agents.
- `chat.presence.*` — **[ASSUMPTION]** NOT on the durable bus; ephemeral channel only.
- `chat.read.*` — likely NOT broadcast on the durable bus (too high volume).

### 6.2 Events Chat CONSUMES (to render / route / unfurl-invalidate)
- From **Issue tracker:** `issue.created/updated/state_changed/assigned/commented` → drive
  channel notifications, invalidate unfurl caches, optionally auto-post to a linked channel.
- From **Git:** `pr.opened/updated/review_requested/merged`, `commit.pushed`,
  `pr.checks_completed` → unfurl invalidation + channel posts.
- From **CI:** `ci.run.started/failed/succeeded` → post to linked channel, page an incident
  channel on failure, invalidate run-status unfurls.
- From **Knowledge:** `doc.updated/published/commented` → unfurl invalidation + notify.
- From **Identity/access:** `user.deactivated`, `user.erased`, `team.membership_changed`,
  `permission.changed` → membership/visibility/unfurl recomputation, **GDPR erasure
  cascade** (see §7), removing access on permission loss.
- From **Agent fabric:** `agent.message.partial` (streaming agent output into a thread),
  `agent.status_changed` (presence), `agent.action_proposed/awaiting_approval` (render an
  approval card in chat).
- From **Notifications system:** delivery receipts / preference updates (loosely).

### 6.3 Agent-fabric integration specifics
- **Mentions and reactions are the trigger surface.** An agent's "subscription" is a filter
  over emitted chat events (e.g. "messages mentioning me," "messages in #support,"
  "reactions of :rerun: on CI unfurls").
- **Agents post back as actors** (§2.8) via the same message-create path, carrying
  provenance. Streaming agent output should be a first-class capability (partial message
  updates), since real agents will be LLM-backed and stream.
- **Mock-first (VISION):** during development, agent handlers are **mock implementations
  behind the strategy interface** — e.g. a mock "@summarizer" that, when mentioned, posts a
  canned/randomized summary. The contract (subscribe → receive event → optionally act/post)
  must be identical for mock and real, so swapping is a config change. Chat's job is to make
  the *trigger + post-back contract* clean and stable; it should never hardcode agent logic.
- **Loop/abuse protection** (from §5.8) belongs partly here: the fabric and chat must
  cooperate to prevent agent↔agent infinite loops and runaway fan-out (hop counters on
  agent-originated message chains, per-agent quotas, kill switches).

---

## 7. GDPR / erasure considerations specific to Chat

Chat is one of the **most GDPR-sensitive** subsystems: messages are free-text personal data,
often *about other people*, hard to redact cleanly, and replicated widely. This needs deep
treatment in architecture; research-level concerns:

- **Personal data is pervasive and unstructured.** Message bodies contain names, opinions,
  and special-category data users type freely. Minimization is hard; the realistic posture
  is strong access control + retention + erasure, not pretending the data isn't personal.
- **Right to erasure (Art. 17) vs. conversation integrity.** Deleting a user must remove
  *their* personal data, but their messages are part of *others'* conversations. Options to
  research:
  - **Tombstone + crypto-shred:** replace authored content with a tombstone ("message
    deleted"); if message bodies are encrypted per-user/per-key, deleting the key renders
    them unrecoverable (**crypto-erasure**) — a strong, scalable technique for "delete
    everywhere including backups/replicas" without rewriting immutable logs. **[ASSUMPTION]**
    this is the leading approach; flag as a key architectural lever.
  - **Authorship anonymization:** detach author identity, keep content where content itself
    isn't the erasure subject. Risky if content names the person.
  - **Mentions of an erased user** must be neutralized/anonymized across all messages — a
    fan-out erasure across the corpus and search index. The mention being a *structured
    node with a stable ID* (§2.4) makes this tractable; free-text mentions would not be.
- **Erasure must cascade to derived data:** search indices, unfurl snapshot caches,
  read-state, notification logs, reference-graph edges, cold/archive tiers, and **backups**.
  Backups are the classic GDPR pain point — crypto-erasure or documented retention windows
  are the usual answers. **[DEFER]** the backup story to architecture but flag it as a known
  hard requirement.
- **Unfurl snapshots may embed third parties' personal data** captured at post time (a PR
  author's name, an issue reporter). If that person is later erased, snapshots must be
  invalidated. Favors *live* unfurls + ephemeral caches over durable snapshots (ties back to
  the live-vs-snapshot decision in §2.4).
- **DMs and private channels: confidentiality + lawful basis.** Who (incl. admins, agents)
  can access private content, under what lawful basis, with what audit trail. Agents reading
  a channel is *processing personal data* — needs lawful basis + records of processing.
- **Data residency:** message storage, search indices, caches, and backups must honor the
  tenant's region (§5.3, §5.9). Residency is a *storage-placement* constraint that the shard
  key must encode.
- **Retention policies:** per-channel/per-org retention (auto-delete after N days) is a
  common corporate + GDPR-minimization feature; must purge from *all* derived stores too.
- **Data Subject Access / portability (Art. 15/20):** "export everything about me in chat"
  — all my messages, mentions of me, reactions, DMs. Needs an export path; cross-cuts search
  and storage.
- **Auditability:** edits, deletions, admin access, agent processing — all logged
  immutably, which itself is personal-data-bearing (audit logs need their own retention/
  erasure reasoning). A known tension: audit immutability vs. erasure.
- **Lawful basis & provenance metadata on every message** (esp. agent-authored) supports
  records of processing (Art. 30) and DPIAs.

---

## 8. Dependencies

### 8.1 On shared systems
- **Identity & access (critical):** actors (human+agent), teams/roles, channel membership
  authority, per-artifact permission decisions for unfurls/search, tokens for CLI/agents,
  deactivation/erasure events. Chat does not own identity; it heavily consumes it.
- **Event bus (critical):** Chat's nervous system — both emit and consume (§6). Needs typed,
  versioned, tenant-/residency-aware, ordered-enough delivery; an ephemeral side-channel for
  presence/typing/read-state that must NOT pollute the durable bus.
- **Agent fabric (critical):** trigger subscriptions, agent identities, post-back contract,
  streaming, mock/real strategy swap, loop/quota safety.
- **Storage (critical):** blob storage for attachments; possibly the message-log substrate;
  residency-aware.
- **Search (critical):** ACL-filtered, multilingual, near-real-time, artifact-cross-search.
- **Notifications (critical):** delivery to email/push/web/mobile/desktop; Chat produces
  notifiable events + holds some preference state; respects DND/quiet hours/digests.
- **Cross-artifact reference graph (critical/differentiating):** resolve artifact handles →
  unfurl projections; write back "referenced in chat" edges; query "where is X discussed."
- **Cross-cutting:** rate-limiting/quota service, audit log, feature flags, observability,
  the shared rich-content/block model (§2.3) if adopted.

### 8.2 On other subsystems (as artifact sources for references/unfurls)
- **Git hosting:** PRs, commits, repos, reviews → unfurls + actionable cards.
- **CI:** runs, jobs, statuses → unfurls + incident posts + re-run actions.
- **Issue tracker:** issues, sprints, roadmap items → unfurls + create/transition actions.
- **Knowledge platform:** docs, databases, rows → unfurls; shared block/content model;
  possible "canvas in channel" overlap (§3) — needs a boundary decision.
- Each must expose: a stable handle, a permission-checkable "can user X see this," and a
  projection API for the unfurl, plus emit update events for cache invalidation.

---

## 9. Open questions (explicit uncertainty)

1. **Real-time transport backplane:** WebSocket gateway + which pub/sub (NATS vs Redis vs
   Kafka-hybrid vs actor-model)? Biggest single architecture decision. **[UNCERTAIN]**
2. **Message store substrate & tiering:** one store vs hot/cold; Postgres-partitioned vs
   Scylla/Cassandra vs custom log; how residency interacts with the shard key. **[UNCERTAIN]**
3. **Write-fanout vs read-fanout split** for bodies vs mentions/unreads — exact boundary.
4. **Unfurl live-vs-snapshot** semantics, and how to make per-viewer permission-aware unfurl
   resolution cheap without leaking. Likely *the* trickiest correctness problem.
5. **Erasure strategy:** crypto-shred vs tombstone vs anonymize; backup erasure story;
   audit-immutability vs erasure tension. Needs legal + architecture alignment.
6. **Group DM vs private channel** — keep both or unify? Product + model decision.
7. **Threads UX model** — threads-first vs channel-first; "also send to channel" or not.
8. **Agent presence & streaming semantics** — what does "online" mean for an agent; how does
   streaming partial agent output render in a thread.
9. **Agent loop/abuse prevention** — hop counters, quotas, kill switches; shared with the
   fabric. Novel; under-specified industry-wide.
10. **Multi-region/residency vs latency** — geo-replication policy for EU-pinned tenants
    with global users. Cross-subsystem; very deep. **[UNCERTAIN]**
11. **Shared rich-content/block model with Knowledge** — adopt one representation across
    Chat + Knowledge? Strong upside, coordination cost. Needs a cross-subsystem decision.
12. **"Canvas"/pinned-summary feature** — build in Chat, defer to Knowledge embed, or skip.
13. **Cross-org / federated channels** — deferred, but model should not foreclose it; deep
    identity/residency/erasure implications.
14. **Voice/video** — out for v1; if ever, embed an EU-sovereign third party. **[DEFER]**
15. **Read-state store** — separate KV with batched eventually-consistent writes? **[ASSUMPTION]**

### Things I did NOT verify (honesty)
- I relied on domain knowledge for competitor behavior (Slack threads/canvas, GitHub
  unfurls, social-feed fan-out patterns) rather than live web checks, to stay efficient.
  None of it is load-bearing for the *requirements* here, but specific competitor feature
  details should be re-verified in the competitive-landscape research doc, not trusted from
  this file.
- I did not benchmark or commit to any specific technology — all tech names are *candidates
  to evaluate*, consistent with the VISION's "steer, not mandate" stance.
