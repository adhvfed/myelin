# Chat — 03 · Event Taxonomy, Contracts & Glue

> See [`00-overview.md`](./00-overview.md) for framing. This doc owns the **complete `chat.*` event taxonomy**
> (durable vs firehose) under the Bus §6 grammar, the events Chat **consumes**, and **how Chat implements every
> glue contract against the FROZEN shapes**: `ArtifactRef` + the frozen `#sub` grammar + `project(ref, viewer)` +
> `replay(scope, since)`; the envelope via the OUTBOX; Id `check`/`list_objects` (the frozen `SetExpr`) + the
> ReBAC namespace fragment; `PersonalDataHolder`; `ToolDef` registrations (the frozen `requires_approval`
> defaults); `declare_indexable`; reserve/settle. Aligned to the **frozen
> [`contract-index.md`](../../../05-refined-shared-systems-architecture/contract-index.md)**; nothing here drifts.

---

## 1. The `chat.*` event taxonomy (Bus §6 grammar: `<subsystem>.<type>.<event>`, singular, past-tense)

Canonical subsystem token = **`chat`**; canonical types = `channel`, `message`, `thread` (the frozen token table,
event-bus §6.2), plus `reaction`, `presence`, `typing`, `read_state` (Chat owns its complete list under the
grammar — contract 2.9: "each subsystem owns + completes its dotted-name list"). The `ArtifactRef` form is
`myelin://<tenant>/chat/<type>/<id>[#sub]`.

### 1.1 Durable bus (via the OUTBOX — the only emit path, BUS-2 / contract 2.2)

| Event | Payload (references-not-payloads; PII via `pii_key_ref`) | Drives |
|---|---|---|
| `chat.message.created` | conversation ref, author `Principal`, thread_root, **`mention`/`artifact_ref`/`embed` nodes** (the frozen inline nodes), `contains_personal_data`, visibility, causation/correlation | live delivery, unfurl edges (`refs.edge.created`), Search index, read-fanout |
| `chat.message.edited` | message ref, new `edited_seq` (the per-message CAS), changed nodes | re-index, re-unfurl, live update |
| `chat.message.deleted` | message ref, reason | timeline removal, derived-store update |
| `chat.message.erased` | message ref (tombstone; `*.erased` cross-cutting) | Search/Refs/Notif erasure cascade |
| `chat.message.mentioned` | message ref, mentioned `Principal` | **the write-fanout / notify-reason producer** (§4 / Notif); the agent **notify**-not-dispatch signal (CHAT-1); the explicit-first reference gate |
| `chat.reaction.added` / `chat.reaction.removed` | message ref, reactor `Principal`, emoji | lightweight signals; a ✅ may be an *explicit* approve-action (never an implicit trigger) |
| `chat.thread.created` / `chat.thread.replied` | thread root ref (the frozen `thread-<root>` `#sub` kind), reply ref | thread watch/read-fanout |
| `chat.channel.created` / `chat.channel.archived` | channel ref, kind, parent_project | lifecycle; ReBAC tuple write |
| `chat.channel.member_added` / `chat.channel.member_removed` | channel ref, `Principal`, role | membership/visibility/unfurl recompute; **ReBAC tuple write** (`write_tuples`, returns zookie) |
| `chat.channel.linked` | channel ref, linked `ArtifactRef` | → `refs.edge.created` ("discussed in") |
| `chat.read_state.updated` | **coarse summary only** (high-volume; the fine grain is firehose) | optional cross-device coarse sync |
| `chat.channel.snapshot` / `chat.message.snapshot` / `chat.thread.snapshot` | the reindex-from-source projection (sub-artifact-granular) | Search/Refs/Notif/OLAP rebuild (via `replay`, §6) |

Per-aggregate ordering is `aggregate = (conversation_id)` so all events for one conversation are per-aggregate
ordered (contract 2.3 / the D-9 drill: per-conversation total order at production QPS).

### 1.2 Firehose ONLY (never the durable bus — ADR-04.5; over the frozen resume-cursor protocol, contract 3.5)

`chat.presence.*` (incl. **agent presence** classes available/busy/rate-limited/offline), `chat.typing.*`,
fine-grained `chat.read_state.*`, `agent.message.partial` (streaming), and the **live message delivery frame**.
These ride the frozen firehose `subscribe/resume/scope` tier (contract 3.5), ephemeral, allowed-to-drop; if lost,
the durable record (or the final message) is the truth (`resume`, [02 §1.3](./02-internals-and-algorithms.md)).
The `no-raw-publish` lint + the firehose seam keep these off the durable bus structurally; the firehose retention
window is a measured-not-predicted tunable (event-bus §OQ-J — too short forces `resync_required`).

### 1.3 Events Chat CONSUMES (to invalidate unfurls / route / cascade)

`issue.*.updated`; `git.pr.*` + **`ci.check.updated`** (the frozen CheckStatus event, X-1) + `ci.run.*`;
`knowledge.*.updated` (and the pinned-canvas page); `identity.human.erased` / `identity.permission.revoked` /
`identity.member.added`; the Agent Fabric `agent.approval.requested` / `agent.status_changed`. Each is a
**whitelisted subject** in the consumer (never `*`; contract 2.4); the unfurl-invalidation consumer matches the
`*.updated`/`*.erased`/`ci.check.updated` set ([02 §4.4](./02-internals-and-algorithms.md)); the erasure
consumer matches `identity.human.erased`. Consumers are idempotent via the `consumer_dedup` ledger (contract 2.5).

---

## 2. `ArtifactRef` + the frozen `#sub` scheme (contract 5.1 / 5.7)

- **Chat mints `ArtifactRef`s** `myelin://<tenant>/chat/{channel|message|thread}/<id>`. The gateway never
  *parses* scope — it carries the string and calls Refs to resolve.
- **The frozen `#sub` grammar** (contract 5.7, X-4): Chat mints from the **frozen vocabulary** —
  `message-<opaqueid>` for a single message, `thread-<opaqueid>` for a thread root (these are the Chat kinds in
  the frozen `#sub` list). The `<opaqueid>` is the immutable `message_id` / `thread_root_id` ULID — **a stable
  opaque id, not a positional index**. **Stable across edits** — an edited message keeps its `message_id` and
  thus its `#sub`, so embeds/references don't dangle.
- **The stability obligation is Chat's** (contract 5.7): a message id is immutable; a thread id is immutable.
  Refs stores the **full sub-URN AND the `#sub`-stripped root**, so a broken sub-anchor still resolves to the
  parent channel via the **one 4-step tombstone ladder** (permission → root → sub-resolve {live/moved/outdated/gone}
  → erased). For Chat a message has no "moved/outdated" state (it is content-addressed by stable id), so its
  ladder outcomes are **live** (active), **gone** (deleted → tombstone with root), or **erased** (crypto-shred →
  tombstone).

---

## 3. `project(ref, viewer)` — Chat's projection API (ADR-13.1; contract 5.6)

**The only way another subsystem reads about a chat artifact (no cross-DB).** REQUIRED on every subsystem;
per-viewer, pre-permission-checked:

```rust
fn project(ref: ArtifactRef, viewer: Principal) -> Projection | Tombstone {
    // 1. PER-VIEWER permission gate first (no leak): Id.check(viewer, view, ref) → Deny ⇒ Tombstone
    // 2. project by type, returning { title, state, icon, render_hint, sub_anchor? } (contract 5.6):
    //    chat/channel  → { title: name|"#"+topic, state: archived?, icon: kind-glyph, render_hint: ChannelChip }
    //    chat/message  → { title: humanised one-line preview, state: edited/deleted, icon: author-kind,
    //                      render_hint: MessageChip, sub_anchor: message-<id> }
    //    chat/thread   → { title: root preview + reply-count, state, icon, render_hint: ThreadChip,
    //                      sub_anchor: thread-<root> }
    //    a tombstoned/erased message → Tombstone ("[deleted]"/"[erased]") — never the body
}
```

So an issue or doc that references "discussed in #incidents" can unfurl a chat message **without reading Chat's
DB** — it calls Refs `resolve`, Refs calls Chat's `project`. The title is humanised via `humanise` (contract 7.3,
the sole templating surface); a restricted message → tombstone, never the body. **Cross-cell resolution is
cell-local** (contract 5.2, OQ-I): a viewer in another cell resolving a chat ref homed here gets the
already-permission-filtered projection, never raw rows.

---

## 4. The fanout boundary Chat owns (which class each event is; Sketch 03)

Chat decides, **per event, which attention class it is** (the obligation Notif hands every subsystem); Notif owns
the routing/inbox/priority/delivery (C-9):

| Class | Strategy (Notif owns the store) | What it is in chat | Producer |
|---|---|---|---|
| **DIRECT / write-fanout** | materialise a per-recipient inbox item | an `@mention(Principal)` of you, a DM to you, a reply in *your* thread, an HITL approval awaiting you, a keyword-alert match | the frozen structured `mention(Principal)` node → `chat.message.mentioned` → a Signal → Notif write-fanout |
| **AMBIENT / read-fanout** | one ordered per-conversation log; per-watcher unread computed lazily | "#general has 40 new", "thread got 12 replies you watch", unread counts | the per-conversation log + read-state; watchers via `list_subjects(channel, watcher)` against the authz reverse index (performant at 50k-member density, contract 4.4) |

Two load-bearing rules: (1) **the mention is the canonical write-fanout producer** and it is a *frozen structured
node* identical across Chat/Issues/Knowledge (contract 13.1), not parsed free text — the same node that drives
write-fanout makes agent dispatch safe (the reference gate). (2) **The unbounded ambient set never
write-amplifies** — a 100k-member announcement does **zero** per-member inbox writes on a post (the
celebrity-fanout mitigation; Silberstein et al. *Feeding Frontier* VLDB 2010; Facebook TAO). Chat registers the
default Signal/notify-reason rules via `define_notif_rule(reason, dedup_tpl, default_class)` (contract 7.6) —
`mentioned`/`replied`/`thread_watched`/`approval_requested` with their priority classes.

---

## 5. The ReBAC namespace fragment + the `watcher` relation (contract 4.9)

Declared in [01 §5](./01-tech-and-data-model.md) (verbatim-aligned to the **frozen** Chat clause: `channel.read =
member + parent_project->read`). The load-bearing points for the glue: (a) **membership writes project tuples**
via `write_tuples([Δtuple], precondition) → zookie` in the **same transaction** as the membership row + the
`chat.channel.member_*` event, **stamping the returned zookie** on the conversation (the new-enemy guard, contract
4.6/4.10 — a just-revoked grant cannot read stale on the next unfurl/read); (b) the **`watcher` relation** is
Chat's Notif read-fanout declaration (contract 4.9: "`watcher` relation per watchable type") —
`list_subjects(channel, watcher)` resolves who gets read-fanout, served by the same authz reverse index as
`list_objects`. **The per-viewer unfurl permission is NOT a chat permission** — it is a `check` against the
*target artifact's* namespace, asked via Refs.

---

## 6. `replay(scope, since)` — reindex-from-source (contract 2.6)

The **only recovery path** for every derived store (Search, Refs, Notif read-models, OLAP) — Chat is **never read
directly**:

```rust
fn replay(scope: ReplayScope, since: Cursor) -> impl Stream<Item = SnapshotEvent> {
    // re-emit chat.{message,channel,thread}.snapshot through the OUTBOX, via the live consumer path.
    // MUST support sub-artifact granularity: a single message, a thread, a channel, a tenant (contract 2.6).
    // bodies in a snapshot are decrypted-then-re-encrypted-per-consumer-need; erased subjects emit tombstones.
}
```

Steady-state indexing and recovery share **one code path** (the outbox → consumer template), so they cannot drift
(EI-04 §5.3). Sub-artifact granularity (a single message/thread) is required (contract 2.6); it is also the
`resync_required` cold-rebuild path the firehose protocol falls back to (contract 3.5). A reindexing consumer
composes the frozen `list_objects` `Filter` so a rebuild stays ACL-correct.

---

## 7. `declare_indexable(IndexSpec)` — Search projection (contract 6.3) + the frozen ACL filter

```rust
declare_indexable(IndexSpec {
    subsystem: "chat",
    type: "message",
    ft_fields: ["body"],                          // the markdown-subset string (decrypted per-index, GD-4-aware)
    struct_fields: ["channel", "author", "thread_root", "created_at", "kind"],
    semantic: Some(EmbeddingSpec { .. }),         // vector for RAG/dedup (embeddings ARE personal data → erasure-aware)
    acl_object_type: "message",                   // Search ALWAYS conjoins the frozen list_objects Filter over message.id
});
```

**Search always conjoins the frozen `list_objects` `Filter{set_expr, zookie}` over the `message.id` column**
before scoring (the `search-requires-acl-filter` lint, contract 6.1) — the `SetExpr` lowers to a JOIN against
Id's per-tenant authz reverse index, so you only find messages in channels you're in, with no N+1, no
post-filter. **Drill:** search as a non-member → **0 results from channels you're not in** ([07](./07-drills-and-open-questions.md)
D-C11). On erasure, Search **purges + reindexes** (incl. embeddings) — never hides. An HYOK tenant whose
`can_derive_plaintext_index()=false` structurally skips message indexing (contract 11.3).

---

## 8. `ToolDef` registrations (the agent-tool surface; contract 8.1, the frozen defaults)

Humans and agents act through the **same** catalogue. Each `ToolDef{ name, input_schema, required_caps,
effect_kind, side_effecting, requires_approval, exposed_over_mcp }`, with the **frozen `requires_approval`
defaults** (recon §X-6: "Chat post = no; a cross-subsystem effect inherits the target's default"):

| Tool | side_effecting | requires_approval (frozen default) | Notes |
|---|---|---|---|
| `chat.post` / `post_message` | true | **false** (X-6: "Chat post = no; reversible, cheap") | post a message; the agent's chat output path; routes through `EffectApi` |
| `chat.reply_in_thread` | true | false | reply to a thread |
| `chat.react` | true | false (X-6: "`react` = no") | add a reaction |
| `chat.start_dm` | true | false | open a DM |
| `chat.create_channel` | true | **true** | creating a channel is governance-shaped (changes visibility) |
| `chat.invite` | true | **true** | adding a member changes who-can-see — sensitive |
| `chat.archive_channel` | true | **true** | destructive lifecycle |
| any `EffectApi` tool that mutates **another subsystem** | true | **inherits THAT subsystem's default** (X-6) | "the effect is governed where it lands, not where it's invoked" |

- **The frozen X-6 rule:** `post`/`react`/`reply` ungated (reversible, cheap); membership/lifecycle mutations
  gated (they change visibility); a cross-subsystem effect (e.g. an agent in chat invoking `git.merge`) inherits
  the **target** subsystem's default (merge = yes). Every consequential gate renders the **HITL card in chat**
  with the per-effect `idem_key` ([02 §5](./02-internals-and-algorithms.md)).
- **All side-effecting tools route through `EffectApi`** (contract 8.2 — plan-then-apply:
  schema→capability→delegation→tenant→budget→HITL→apply-via-public-endpoint→meter), **never** `ToolHands::exec`
  (that is for sandboxed compute, not chat mutation — the routing split is the safety boundary, X-6). `EffectApi`
  **reserves** (contract 11.7) — Chat does not own a private spend path. The tool inherits the **four uniform
  guarantees** (X-6): cost gate, per-run attenuated token attribution, HITL withhold, isolation floor+drill.
- **`exposed_over_mcp` defaults false** (the external MCP endpoint is a deferred platform floor).

---

## 9. The envelope via the OUTBOX, and reserve/settle

- **`OutboxTx::emit(draft, cause)` is the only emit path** (contract 2.2; `no-raw-publish` lint). Every state
  change — a message, an edit, a membership change, a reaction — commits its row **and** its event in one PG
  transaction. The **gateway has no emit path of its own**; it calls the Message Service, which does the outbox
  co-commit (contract 1.7 — the gateway must not regress to fire-and-forget even if it diverges to BEAM).
- **Causality correct-by-construction:** `emit(draft, cause)` derives `correlation_id`/`causation_id`/`depth`
  from the causing envelope (the canonical envelope, contract 2.1), so audit, the "why" view, tracing, and the
  agent loop guard are **one mechanism** (BUS-5). A human cannot typo into a loop.
- **Reserve/settle (where Chat runs spend-bearing work):** Chat dispatches agent work through `EffectApi`, which
  **reserves at dispatch (no balance → no start) and settles on completion** (contract 11.7), never interrupting
  in-flight. Chat surfaces the cost (the HITL card's live estimate) but never holds the wallet (Commercial owns
  it).

---

## 10. `PersonalDataHolder` + the restriction flag

Implemented over every Chat store ([02 §6.5](./02-internals-and-algorithms.md)); auto-registered by the harness
(contract 1.4). The **restriction flag** (Art. 18) is honoured at every read path: for a restricted subject, Chat
**stops indexing, agent-use, new notification routing, and analytics** (the message remains stored but is excluded
from those processings) — a distinct state from erasure. The **free-text residual is handled per the ONE platform
posture** (contract 10.9, recon §X-7) **by reference**, not restated. Hot tables (`message`, `read_state`,
`reaction`) are flagged for the `forward-only-migration` lint (contract 1.5); personal-data fields carry
`#[personal_data(...)]` tags (the `no-untagged-personal-data` lint, contract 10.2) feeding the generated data map.

Continue to [`04-views-cli-and-api.md`](./04-views-cli-and-api.md).
