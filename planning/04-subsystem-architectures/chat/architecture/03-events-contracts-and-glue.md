# Chat — 03 · Event Taxonomy, Contracts & Glue

> See [`00-overview.md`](./00-overview.md) for framing. This doc owns the **complete `chat.*` event taxonomy**
> (durable vs firehose) under the Bus §6 grammar, the events Chat **consumes**, and **how Chat implements every
> glue contract**: `ArtifactRef` + `project(ref, viewer)` + `replay(scope, since)`; the envelope via the OUTBOX;
> Id `check`/`list_objects` + the ReBAC namespace fragment; `PersonalDataHolder`; `ToolDef` registrations;
> `declare_indexable`; reserve/settle. Aligned to the Phase-3 [`contract-index.md`](../../../03-shared-systems-architecture/contract-index.md);
> nothing here is re-invented.

---

## 1. The `chat.*` event taxonomy (Bus §6 grammar: `<subsystem>.<type>.<event>`, singular, past-tense)

Canonical subsystem token = **`chat`**; canonical types = `channel`, `message`, `thread` (Bus §6.2 token table),
plus `reaction`, `presence`, `typing`, `read_state` (Chat owns its complete list under the grammar — Bus §6 / the
README §5 obligation). The `ArtifactRef` form is `myelin://<tenant>/chat/<type>/<id>[#sub]`.

### 1.1 Durable bus (via the OUTBOX — the only emit path, BUS-2)

| Event | Payload (references-not-payloads; PII via `pii_key_ref`) | Drives |
|---|---|---|
| `chat.message.created` | conversation ref, author `Principal`, thread_root, **`mention`/`artifact_ref`/`embed` nodes**, `contains_personal_data`, visibility, causation/correlation | live delivery, unfurl edges, Search index, read-fanout |
| `chat.message.edited` | message ref, new `edited_seq`, changed nodes | re-index, re-unfurl, live update |
| `chat.message.deleted` | message ref, reason | timeline removal, derived-store update |
| `chat.message.erased` | message ref (tombstone; `*.erased` cross-cutting) | Search/Refs/Notif erasure cascade |
| `chat.message.mentioned` | message ref, mentioned `Principal` | **the write-fanout / notify-reason producer** (§4 / Notif); the agent **notify**-not-dispatch signal (CHAT-1); the AG-6 reference gate |
| `chat.reaction.added` / `chat.reaction.removed` | message ref, reactor `Principal`, emoji | lightweight signals; a ✅ may be an *explicit* approve-action (never an implicit trigger) |
| `chat.thread.created` / `chat.thread.replied` | thread root ref, reply ref | thread watch/read-fanout |
| `chat.channel.created` / `chat.channel.archived` | channel ref, kind, parent_project | lifecycle; ReBAC tuple write |
| `chat.channel.member_added` / `chat.channel.member_removed` | channel ref, `Principal`, role | membership/visibility/unfurl recompute; **ReBAC tuple write** (`write_tuples`) |
| `chat.channel.linked` | channel ref, linked `ArtifactRef` | → `refs.edge.created` ("discussed in") |
| `chat.read_state.updated` | **coarse summary only** (high-volume; the fine grain is firehose) | optional cross-device coarse sync |
| `chat.channel.snapshot` / `chat.message.snapshot` | the reindex-from-source projection (sub-artifact-granular) | Search/Refs/Notif/OLAP rebuild (§via `replay`) |

### 1.2 Firehose ONLY (never the durable bus — ADR-04.5; Phase-2 Chat §7.1)

`chat.presence.*` (incl. **agent presence** classes available/busy/rate-limited/offline), `chat.typing.*`,
fine-grained `chat.read_state.*`, `agent.message.partial` (streaming). These ride NATS core, ephemeral,
at-most-once; if lost, the durable record (or the final message) is the truth (resync, [02 §1.3](./02-internals-and-algorithms.md)).
The `no-raw-publish` lint + the firehose seam keep these off the durable bus structurally.

### 1.3 Events Chat CONSUMES (to invalidate unfurls / route / cascade)

`issue.*.updated`; `git.pr.*` + `git.pr.checks_completed`; `ci.run.*` + `ci.log.available`;
`knowledge.doc.updated` (and the pinned-canvas page); `identity.human.erased` / `identity.permission.revoked` /
`identity.member.added`; the Agent Fabric `agent.approval.requested` / `agent.status_changed`. Each is a
**whitelisted subject** in the consumer (never `*`); the unfurl-invalidation consumer matches the `*.updated`/
`*.erased` set ([02 §4.4](./02-internals-and-algorithms.md)); the erasure consumer matches `identity.human.erased`.

---

## 2. `ArtifactRef` + the `#sub` scheme (Refs §3.1/§3.5)

- **Chat mints `ArtifactRef`s** `myelin://<tenant>/chat/{channel|message|thread}/<id>`. The gateway never
  *parses* scope — it carries the string and calls Refs to resolve.
- **Stable `#sub` scheme** (Refs §3.5; substrate §13 Q4): `#message-<id>` for a message within a thread/channel,
  `#thread-<root>` for a thread. **Stable across edits** — an edited message keeps its `message_id` and thus its
  `#sub`, so embeds/references don't dangle ([01 §3](./01-tech-and-data-model.md)).

---

## 3. `project(ref, viewer)` — Chat's projection API (ADR-13.1; Refs §5.6)

**The only way another subsystem reads about a chat artifact (no cross-DB).** Per-viewer, pre-permission-checked:

```rust
fn project(ref: ArtifactRef, viewer: Principal) -> Projection | Tombstone {
    // 1. PER-VIEWER permission gate first (no leak): Id.check(viewer, view, ref) → Deny ⇒ Tombstone
    // 2. project by type:
    //    chat/channel  → { title: name|"#"+topic, state: archived?, icon: kind-glyph, render_hint: ChannelChip }
    //    chat/message  → { title: humanised one-line preview, state: edited/deleted, icon: author-kind,
    //                      render_hint: MessageChip, sub_anchor: #message-<id> }
    //    chat/thread   → { title: root preview + reply-count, state, icon, render_hint: ThreadChip }
    //    a tombstoned/erased message → Tombstone ("[deleted]"/"[erased]") — never the body
}
```

So an issue or doc that references "discussed in #incidents" can unfurl a chat message **without reading Chat's
DB** — it calls Refs `resolve`, Refs calls Chat's `project`. The title is humanised at the backend (NOTIF-1); a
restricted message → tombstone, never the body.

---

## 4. The fanout boundary Chat owns (which class each event is; Sketch 03)

Chat decides, **per event, which attention class it is** (the §8 obligation Notif §8 hands every subsystem);
Notif owns the routing/inbox/priority/delivery (C-9):

| Class | Strategy (Notif owns the store) | What it is in chat | Producer |
|---|---|---|---|
| **DIRECT / write-fanout** | materialise a per-recipient inbox item | an `@mention(Principal)` of you, a DM to you, a reply in *your* thread, an HITL approval awaiting you, a keyword-alert match | the structured `mention(Principal)` node → `chat.message.mentioned` → a Signal → Notif write-fanout |
| **AMBIENT / read-fanout** | one ordered per-conversation log; per-watcher unread computed lazily | "#general has 40 new", "thread got 12 replies you watch", unread counts | the per-conversation log + read-state; watchers via `list_subjects(channel, watch)` |

Two load-bearing rules: (1) **the mention is the canonical write-fanout producer** and it is a *structured node*,
not parsed free text (ADR-05) — the same node that drives write-fanout makes agent dispatch safe (AG-6). (2)
**The unbounded ambient set never write-amplifies** — a 100k-member announcement does **zero** per-member inbox
writes on a post (the celebrity-fanout mitigation; Silberstein et al. *Feeding Frontier* VLDB 2010; Facebook
TAO). Chat registers the default Signal/notify-reason rules via `define_notif_rule(reason, dedup_tpl,
default_class)` — `mentioned`/`replied`/`thread_watched`/`approval_requested` with their priority classes.

---

## 5. The ReBAC namespace fragment + the `watcher` relation (Id §5)

Declared in [01 §5](./01-tech-and-data-model.md) (verbatim-aligned to Id §5's Chat clause). The two load-bearing
points for the glue: (a) **membership writes project tuples** via `write_tuples([Δtuple], precondition)` in the
**same transaction** as the membership row + the `chat.channel.member_*` event, returning a zookie to stamp; (b)
the **`watcher` relation** is Chat's Notif read-fanout declaration (Notif §8.3) — `list_subjects(channel,
watcher)` resolves who gets read-fanout. **The per-viewer unfurl permission is NOT a chat permission** — it is a
`check` against the *target artifact's* namespace, asked via Refs.

---

## 6. `replay(scope, since)` — reindex-from-source (Bus §4.9/§5.6)

The **only recovery path** for every derived store (Search, Refs, Notif read-models, OLAP) — Chat is **never read
directly**:

```rust
fn replay(scope: ReplayScope, since: Cursor) -> impl Stream<Item = SnapshotEvent> {
    // re-emit chat.{message,channel,thread}.snapshot through the OUTBOX, via the live consumer path.
    // MUST support sub-artifact granularity: a single message, a thread, a channel, a tenant.
    // bodies in a snapshot are decrypted-then-re-encrypted-per-consumer-need; erased subjects emit tombstones.
}
```

Steady-state indexing and recovery share **one code path** (the outbox → consumer template), so they cannot drift
(EI-04 §5.3). Sub-artifact granularity (a single message/thread) is required (Bus §2.6; S-10 anchors the
`list_objects` push-down a Search reindex composes).

---

## 7. `declare_indexable(IndexSpec)` — Search projection (Search §6.3)

```rust
declare_indexable(IndexSpec {
    subsystem: "chat",
    type: "message",
    ft_fields: ["body"],                          // the markdown-subset string (decrypted per-index, GD-4-aware)
    struct_fields: ["channel", "author", "thread_root", "created_at", "kind"],
    semantic: Some(EmbeddingSpec { .. }),         // vector for RAG/dedup (embeddings ARE personal data → erasure-aware)
    acl_object_type: "message",                   // Search ALWAYS conjoins list_objects(viewer, read, message)
});
```

**Search always conjoins `list_objects(viewer, read, message)` before scoring** (the `search-requires-acl-filter`
lint, S-3) — so you only find messages in channels you're in (Phase-1 §5.5). **Drill:** search as a non-member →
**0 results from channels you're not in** ([07](./07-drills-and-open-questions.md)). On erasure, Search **purges +
reindexes** (incl. embeddings) — never hides. An HYOK tenant whose `can_derive_plaintext_index()=false`
structurally skips message indexing (Storage §6.2).

---

## 8. `ToolDef` registrations (the agent-tool surface; Agent §6)

Humans and agents act through the **same** catalogue (design-language §5.2). Each
`ToolDef{ name, input_schema, required_caps, effect_kind, side_effecting, requires_approval, exposed_over_mcp }`:

| Tool | side_effecting | requires_approval (default) | Notes |
|---|---|---|---|
| `chat.post` | true | **false** | post a message; the agent's chat output path; routes through `EffectApi` |
| `chat.reply_in_thread` | true | false | reply to a thread |
| `chat.react` | true | false | add a reaction |
| `chat.start_dm` | true | false | open a DM |
| `chat.create_channel` | true | **true** | creating a channel is governance-shaped |
| `chat.invite` | true | **true** | adding a member changes who-can-see — sensitive |
| `chat.archive_channel` | true | **true** | destructive lifecycle |

- **`requires_approval` defaults (Chat's per-subsystem call, Agent §5.2):** gate the **membership/lifecycle**
  mutations (invite/create/archive — they change visibility); leave **post/reply/react** ungated (an agent
  posting is its core function; the agent badge + provenance + calm-volume handle trust, [02 §7](./02-internals-and-algorithms.md)).
  Every consequential gate renders the **HITL card in chat** ([02 §5](./02-internals-and-algorithms.md)).
- **All side-effecting tools route through `EffectApi`** (governed mutation via the public endpoint — plan-then-
  apply, schema→capability→delegation→tenant→budget→HITL→apply→meter), **never** `ToolHands::exec` (that is for
  sandboxed compute, not chat mutation). `EffectApi` **reserves** — Chat does not own a private spend path.
- **`exposed_over_mcp` defaults false** (the external MCP endpoint is a deferred platform floor, README §6).

---

## 9. The envelope via the OUTBOX, and reserve/settle

- **`OutboxTx::emit(draft, cause)` is the only emit path** (BUS-2; `no-raw-publish` lint). Every state change —
  a message, an edit, a membership change, a reaction — commits its row **and** its event in one PG transaction.
  The **gateway has no emit path of its own**; it calls the Message Service, which does the outbox co-commit
  (Sketch 09 — the gateway must not regress to fire-and-forget even if it diverges to BEAM).
- **Causality correct-by-construction:** `emit(draft, cause)` derives `correlation_id`/`causation_id`/`depth`
  from the causing envelope, so audit, the "why" view, tracing, and the agent loop guard are **one mechanism**
  (BUS-5). A human cannot typo into a loop.
- **Reserve/settle (where Chat runs spend-bearing work):** Chat dispatches agent work through `EffectApi`, which
  **reserves at dispatch (no balance → no start) and settles on completion** (D8/CI-2; Storage §reserve-settle).
  Chat surfaces the cost (the HITL card's live estimate) but never holds the wallet (Commercial owns it, C-1).

---

## 10. `PersonalDataHolder` + the restriction flag

Implemented over every Chat store ([02 §6.4](./02-internals-and-algorithms.md)); auto-registered by the harness.
The **restriction flag** (Art. 18) is honoured at every read path: for a restricted subject, Chat **stops
indexing, agent-use, and new notification routing** (the message remains stored but is excluded from those
processings) — a distinct state from erasure. Hot tables (`message`, `read_state`, `reaction`) are flagged for
the `forward-only-migration` lint; personal-data fields carry `#[personal_data(...)]` tags (the
`no-untagged-personal-data` lint, S-5) feeding the generated data map.

Continue to [`04-views-cli-and-api.md`](./04-views-cli-and-api.md).
