# Sketch 10 — The `chat.*` taxonomy, `ToolDef`s, ReBAC namespace, and the glue obligations

> Exploration note. Chat's Phase-3 README §5 obligation list made concrete: own the **complete `chat.*`
> taxonomy** (Bus §10.1), register `ToolDef`s, declare the ReBAC namespace fragment + `watcher` relation,
> implement `project`/`replay`/`declare_indexable`/`PersonalDataHolder`. Not a deep design — a checklist
> with the leaned shapes, so the architecture stage builds to a known surface.

---

## A — The `chat.*` event taxonomy (Bus §6 grammar: `<subsystem>.<type>.<event>`, singular, past-tense)

Canonical subsystem token = **`chat`**; types = `channel`, `message`, `thread`, `reaction` (Bus §6.2).
Seed list (Chat owns the *complete* list as a P4 deliverable, validated against the §6 grammar):

**Durable bus** (via outbox; the contract):
- `chat.message.created` / `chat.message.edited` / `chat.message.deleted` — carry conversation, author
  `Principal`, thread root, **`mention` + `artifact_ref` + `embed` nodes** (ADR-05), tenant, region,
  visibility, `contains_personal_data`.
- `chat.message.mentioned` — a specific principal was `@`-mentioned (the **write-fanout / notify-reason
  producer**, Sketch 03; the agent **notify**-not-dispatch signal, Sketch 07; the AG-6 reference gate).
- `chat.reaction.added` / `chat.reaction.removed` — lightweight signals (a ✅ can be an *explicit*
  approve-action, Sketch 07 — never an implicit auto-trigger).
- `chat.thread.created` / `chat.thread.replied`.
- `chat.channel.created` / `chat.channel.archived` / `chat.channel.member_added` /
  `chat.channel.member_removed` — drive membership/visibility/unfurl recomputation + ReBAC tuple writes.
- `chat.channel.linked` — an artifact-linked channel created → a `refs.edge.created` ("discussed in").
- `chat.read_state.updated` — **coarse summary only** on the durable bus (high-volume; mostly firehose).
- `chat.message.erased` (`*.erased` tombstone) + `chat.*.snapshot` (reindex-from-source) — cross-cutting.

**Firehose ONLY (never durable bus)** — Phase-2 Chat §7.1; ADR-04.5:
- `chat.presence.*` (incl. **agent presence** classes, Sketch 07), `chat.typing.*`, fine-grained
  `chat.read_state.*`, `agent.message.partial` (streaming, Sketch 07).

**Pointer events Chat CONSUMES** (to invalidate unfurls / route — Sketch 04; Phase-2 Chat §7.2):
`issue.*.updated`, `git.pr.*` + `git.pr.checks_completed`, `ci.run.*` + `ci.log.available`,
`knowledge.doc.updated`, `identity.human.erased` / `identity.permission.revoked` /
`identity.member.added`, and the Agent Fabric `agent.approval.requested` / `agent.status_changed`.

---

## B — `ToolDef` registrations (into the shared `ToolSurface`; Agent §6)

Each is `ToolDef{ name, input_schema, required_caps, effect_kind, side_effecting, requires_approval,
exposed_over_mcp }` (Agent §4.2). Chat's actions (humans and agents act through the **same** catalogue —
design-language §5.2):

| Tool | effect_kind / side_effecting | requires_approval (default) | Notes |
|---|---|---|---|
| `chat.post` | mutate / true | **false** | post a message; goes through `EffectApi` (plan-then-apply); the agent's chat output path |
| `chat.reply_in_thread` | mutate / true | false | reply to a thread |
| `chat.react` | mutate / true | false | add a reaction |
| `chat.create_channel` | mutate / true | **true** (consequential) | creating a channel is governance-shaped |
| `chat.invite` | mutate / true | **true** | adding a member changes who-can-see — sensitive |
| `chat.start_dm` | mutate / true | false | open a DM |
| `chat.archive_channel` | mutate / true | **true** | destructive-ish lifecycle |

- `requires_approval` defaults are a **per-subsystem call** (Agent §5.2; README §5) — Chat's lean: gate
  the *membership/lifecycle* mutations (invite/create/archive — they change visibility), leave
  post/reply/react ungated (an agent posting is its core function; the agent badge + provenance + calm-
  volume handle trust, Sketch 07). All consequential gates render the **HITL card in chat** (Sketch 06).
- All side-effecting tools route through **`EffectApi`** (governed mutation via the public endpoint, no
  carve-out — Agent §5.0), **never** `ToolHands::exec` (that's for sandboxed compute, not chat mutation).
- `exposed_over_mcp` defaults **false**; the external MCP endpoint is a deferred platform floor (README §6).

---

## C — ReBAC namespace fragment (Chat's contribution to the one cell schema; Id §5/§4.9)

```
definition channel {
  relation parent_project: project
  relation member:  user | agent | service        // membership IS the ACL for private kinds
  relation watcher: user | agent                   // the Notif read-fanout relation (README §5 obligation)
  permission read   = member + parent_project->read
  permission post   = member
  permission manage = member & parent_project->admin   // invite/archive/settings
}
definition message {
  relation parent_channel: channel
  permission view = parent_channel->read
}
```
(verbatim-aligned to identity §5 Chat clause: `channel.read = member + parent_project->read`;
`message.view = parent_channel->read`.) **The per-viewer unfurl is NOT a chat permission** — Chat asks
Refs, Refs asks Id `check(viewer, view, target)` (identity §5; Sketch 04). The **`watcher` relation** is
the Notif read-fanout declaration every watchable type owes (Notif §8.3) — Chat declares it on `channel`
(and per-thread watch derives from it).

---

## D — The other required glue contracts (README §5; one line each, leaned)

- **`project(ref, viewer) → {title, state, icon, render_hint, sub_anchor?}`** (ADR-13.1; Refs §5.6): Chat
  implements it for `chat/channel`, `chat/message`, `chat/thread` — so *other* subsystems can unfurl a
  chat message (e.g. an issue referencing "discussed in #incidents"). Per-viewer, pre-permission-checked.
- **`replay(scope, since) → emits *.snapshot`** (Bus §5.6): Chat re-emits message/channel/membership
  snapshots through the outbox so Search/Refs/Notif/OLAP **reindex-from-source** — Chat is never read
  directly. Must support **sub-artifact-granular** snapshots (a single message, a thread).
- **`declare_indexable(IndexSpec)`** (Search §6.3): the message → index-doc projection (ft_fields = body
  text via the markdown-subset string, struct_fields = channel/author/thread/timestamp, `acl_object_type
  = message`) — Search **always conjoins `list_objects(viewer, read, message)`** so you only find messages
  in channels you're in (Phase-1 §5.5; the `search-requires-acl-filter` lint).
- **Sub-artifact `#sub` scheme** (Refs §3.5; substrate §13 Q4): Chat mints stable opaque sub-ids —
  `#message-<id>` for a message within a thread/channel, `#thread-<root>` — **stable across edits** so
  embeds don't dangle (an edited message keeps its `#sub`).
- **`PersonalDataHolder`** (Sketch 05): `locate/export/rectify/restrict/erase` over every Chat store.
- **Hot-table flagging** (substrate §13 Q2): the `message` table + the read-state store are hot →
  forward-only expand→backfill→contract.
- **Per-surface shed budgets** (substrate §13 Q3): Chat's are the **connection-storm** profile (a deploy
  reconnect thundering-herd; an agent mention storm) — the protected-human-lane reservation + per-tenant
  in-flight caps tuned for connection churn (Sketch 07).

---

## What this sketch hands forward

A concrete glue checklist the architecture stage builds to: the `chat.*` taxonomy (durable vs firehose
split), the `ToolDef` set + `requires_approval` defaults (gate membership/lifecycle, not post/react), the
ReBAC fragment (`channel.read = member + parent_project->read`, the `watcher` relation), and the
`project`/`replay`/`declare_indexable`/`#sub`/`PersonalDataHolder`/hot-table/shed-budget obligations — all
aligned to the already-decided Phase-3 contracts, none re-invented.
