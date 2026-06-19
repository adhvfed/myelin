# Knowledge Platform — 04 · Views, CLI & API / Agent-Tool Surface

> See [`00-overview.md`](./00-overview.md) for framing. This doc enumerates the **primary screens** (with
> their first-class empty/loading/error states), the **CLI** surface, and the **API / agent-tool** surface.
> Per VISION §3, **design sketches (IA + flows + wireframes incl. empty/loading/error states) precede UI
> code** — and they exist (the PRESERVED design record):
> [`../design/information-architecture.md`](../design/information-architecture.md) (the one-shell fit + nav
> model), [`../design/user-flows.md`](../design/user-flows.md) (the task flows), and
> [`../design/wireframes.md`](../design/wireframes.md) (the per-screen ASCII wireframes S1–S12, each with
> empty/loading/error + permission-denied/erased states and the §8b primitives applied). This doc is the
> architectural screen checklist; the design sketches are the visual ground truth those screens build to.
>
> All screens consume the **shared design-system package** (DL §8.1): one shell, the overlay primitives
> (portal-always, one z-index scale, centralized focus-trap/ARIA — DL §8b.1), the `ArtifactRef` chip/unfurl,
> the identity/agent badge, the shared views component, and the **one editor render path** (KN-4). No subsystem
> ships its own design system.

---

## 1. The views / primary screens (DL §7.4)

> **States are first-class** (EI-05 §4): empty *explains + offers create*; loading shows *structure*
> (skeletons matching the final layout, never a spinner on blank); error *blames the system in one quiet line
> + a path*; a degraded surface fails *static* ("temporarily unavailable" for that surface). Latency budgets
> are hard: keyboard < ~100ms, suppress spinner-flash < ~1s, pages render not animate-in.

> **Wireframe map** (the ASCII ground truth in [`../design/wireframes.md`](../design/wireframes.md)): §1.1
> editor = **S1**; §1.2 db views = **S2**; §1.3 nav tree = **S3**; §1.4 backlinks = **S4**; §1.5 comments =
> **S5**; §1.6 history = **S6**; §1.7 sharing = **S7**; §1.8 templates = **S8**; §1.9 search = **S9**; §1.10
> agent affordances + HITL card = **S10**; (§11 export = **S11**); §1.11 mobile = **S12**.

### 1.1 The block editor (page view) — the core surface

Block-based WYSIWYG over the **one editor render path** (KN-4): slash-command (`/`) insert menu (the frozen
`myelin-content` Block variants), drag-handles to move/reorder (the LexoRank `order_key`), nested/indentable
lists, inline `@`-mention & `ArtifactRef` autocomplete (the structured-node picker — the *only* re-trigger
source for agents, AG-6), markdown shortcuts, paste fidelity (web/Word/MD → blocks), image/file upload &
embed, code blocks with highlighting, tables, and **live presence** (cursors/avatars over the awareness tier).

*States:* **empty** (new page — explains + offers a first block / template); **loading** (lazy block load for
huge docs — skeleton block structure); **read-only** (no `edit` permission); **offline/syncing** (the
resume-cursor transport reconnecting via `firehose::resume` — a non-blocking "reconnecting, your edits are
queued" indicator, [02 §2](./02-internals-and-algorithms.md)); **conflict-resolving** (the CAS floor surfaced a
same-block conflict — shows both versions, never a silent overwrite); **agent-suggesting** (agent-authored
edits shown distinctly with accept/reject — agents look like agents, no sparkle iconography, DL §8b.3);
**tombstoned-reference placeholder** (a referenced artifact was erased — neutral "(deleted)" chip via the
4-step tombstone ladder, never a crash).

### 1.2 Database views — the shared views component (the frozen `ViewSpec`)

Table (inline-edit cells, resize/reorder columns, add property), Board (drag cards between status columns —
LexoRank `order_key`), Calendar (drag to reschedule), List, Gallery, Timeline (the frozen `ViewSpec.kind` set).
Per-view filter/sort/group UI (the frozen `QueryAst`); row "peek" / open-row-as-page (a row is itself a page
with a body).

*States:* **empty database** (explains + offers a first row/import); **schema-editing**; **filtered-empty**;
**loading large result set** (skeleton rows); **permission-filtered** (rows the viewer can't see are simply
**absent — never post-filtered**, the `SetExpr` conjoin, [02 §4.1](./02-internals-and-algorithms.md));
**formula-recomputing** (a read-time rollup over a large set shows a brief computing state).

### 1.3 Navigation sidebar — the tree

Spaces → pages → sub-pages; favorites/pins; recent; breadcrumb; quick-switcher / command palette (full-text +
the universal reference graph — one keystroke to any entity). *States:* empty workspace (guided first-run);
search-no-results; deep-tree virtualised loading.

### 1.4 Backlinks / references panel — the graph made visible

"Linked references / mentioned in" on every page; hover-preview (peek a doc/issue/commit/run without leaving —
the system assembles context, DL §8b.6). Permission-filtered at read time via `Refs.backlinks` (contract 5.3).
*States:* no backlinks; permission-filtered (only refs from things you can read); referenced-artifact-erased
(graceful tombstone via the ladder); live update on referenced-artifact change. **Mobile:** backlinks
row-actions surfaced by default (hover-is-not-touch).

### 1.5 Comments & discussion

Inline comments anchored to a text range or block (the `comment-`/`thread-` `#sub` grammar, **shared with
Chat** — OQ-L). v1 ships a **KB-native comment thread** (its own store) using the **shared design-system thread
render component** (CR-G: KB-native anchor data + shared thread render). The consolidation onto the Chat
threading primitive + firehose transport is the **named follow-on** (promote when doc-anchored comments need
real-time multi-party presence) — because they already share `#sub` + content + refs, that promotion swaps the
store/transport, not the data model ([05 §10](./05-hard-problems.md)).

### 1.6 History UI

Version timeline, diff view, restore-to-version (the snapshot/restore floor). *States:* no history;
**crypto-shredded/erased segment shown as redacted** (an erased range renders "content erased" — the
erasure-reaches-history property made visible); restore-confirm.

### 1.7 Sharing / permissions dialog (the overlay primitive)

Member/guest management, link sharing, **publish-to-web** (with an **explicit personal-data warning +
lawful-basis prompt** — a high-risk export). The per-database **row-restriction opt-in** ("restrict rows by a
person/team field" → declares the `row_reader` grant rule, [01 §5.1](./01-tech-and-data-model.md)). *States:*
inherited vs overridden ACL (the page-tree override made legible); public-published warning; guest/link-only.
Reversibility over confirmation **except** publish (a consequential/GDPR action confirms).

### 1.8 Templates UI

Insert-from-template, new-from-template, org template gallery. Living-doc/daily-note templates and status
strings register into the **ONE `humanise`/ICU-MessageFormat surface** (contract 7.3, OQ-L) — no second
template engine. *States:* empty gallery; **template that pre-seeds personal-data fields (GDPR-flagged)**.

### 1.9 Search / quick-switcher palette

Cross-type (page/db/row) results, **permission-filtered** (the `Filter` conjoin, contract 6.1), multilingual.
*States:* loading; no-results; semantic-vs-keyword toggle.

### 1.10 Agent affordances (woven through, not a standalone screen)

Agent presence in a doc; "suggested by agent" attribution; accept/reject of agent edits; "ask an agent" on a
doc ("summarise", "turn this into issues"); **HITL approval cards surfaced via Chat** (contract 8.2) for
consequential agent actions — the card resumes the run via a durable signal with the per-effect `idem_key`
rule (OQ-F). Must work with **mock agents** in dev (`--use-mock`, contract 8.3). Agents look like agents — no
sparkle/shimmer/magic-wand iconography, no emoji as UI (DL §8b.3).

### 1.11 Mobile / responsive read + light edit

Read + light edit, queued offline on the CAS floor (full offline-first arrives with the CRDT,
[05 §8](./05-hard-problems.md)). The shell pins to the viewport (`100vh`, `overflow:hidden`, scrollers own
`min-height:0`); `width:100%` is not a takeover (collapse the sidebar at the breakpoint); popovers flip when
off-screen, tested against the real anchor.

---

## 2. The CLI surface (DL §7.7 — a peer surface, machine-output-friendly)

Namespace `myelin kb …` (the `knowledge` token's CLI noun alias is `doc`). Every command authorizes via one
`Principal` (ADR-13), is auditable (contract 10.6), and supports `--format json` everywhere (agents + CI call
it). The CLI uses the **same `ArtifactRef` scheme** (`myelin://…`, incl. the frozen `#sub` kinds) and humanised
rendering as the UI.

**Pages / docs**
```
myelin kb page list   [--space <s>] [--parent <id>] [--format json]
myelin kb page get    <id> [--format md|json|html]          # render(parse(md))===md round-trips the md form
myelin kb page create [--space <s>] [--parent <id>] [--title ...] [--from-file <md>] [--template <t>]
myelin kb page edit   <id> [--from-file <md>]               # replace body (CAS-guarded; --expect-version optional)
myelin kb page append <id> --from-file <md>
myelin kb page move   <id> --to <parent>
myelin kb page archive|delete <id>
myelin kb page history <id> [--format json]
myelin kb page restore <id> --version <v>
myelin kb page export  <id|--space <s>|--all> --format md|json|pdf --out <dir>
myelin kb page publish|unpublish <id>                       # publish warns on personal-data export
myelin kb import --from adf <file>                          # ADF→myelin-content lossy-map (contract 13.2); writes an import report
```

**Databases**
```
myelin kb db create --space <s> --name ... [--schema <file>]
myelin kb db schema get <db>
myelin kb db schema add-property <db> --type <FieldType> --name ...   # the frozen myelin-query FieldType set
myelin kb db row add    <db> --set key=value …              # incl. --set ref=myelin://...  (relation / artifact ref)
myelin kb db row list   <db> [--view <v>] [--filter <QueryAst>] [--sort <prop>] [--format json]
myelin kb db row get|update|delete <db> <row>
myelin kb db view create <db> --kind table|board|calendar|list|gallery|timeline --group-by <prop>   # a ViewSpec
myelin kb db import <db> --csv <file>
myelin kb db export <db> --csv|--json
```

**Permissions / references / GDPR / agents**
```
myelin kb share <page-or-db> --grant <principal>=<role>     # role ∈ read|comment|edit|manage (compiles to tuples, 4.9)
myelin kb share <page-or-db> --restrict-rows <field>        # the per-db row_reader opt-in (CR-D)
myelin kb backlinks <id> [--format json]                    # what references this (permission-filtered, Refs)
myelin kb refs      <id> [--format json]                    # what this references (incl. #b/#h/#row/#field sub-anchors)
myelin kb export-subject --principal <user> --out <dir>     # DSR: lossless JSON of all KB by/about subject
myelin kb erase-subject  --principal <user> [--dry-run]     # erasure (pseudonym-map shred + per-subject crypto-shred)
myelin kb watch <id> [--format json]                        # stream events (powers triggers/agents) — pointer + semantic
myelin kb template list|apply
```

---

## 3. The API / agent-tool surface

### 3.1 The public API (behind the gateway, contract 1.2)

Knowledge exposes its mutations as **public endpoints** behind the stateless gateway (identity-injected,
tenant-from-token). Agents call the **same endpoints** as humans (no carve-out): create/edit/append a page,
upsert a row, share, publish — each authorized by `Id.check`, each emitting via the outbox. The collab
op-stream is a separate WebSocket-class surface over the firehose (`subscribe/resume(scope=doc:<id>)`,
[02 §2](./02-internals-and-algorithms.md)), authorized per-connection.

### 3.2 The agent-tool surface (the `ToolDef` registry — [03 §5](./03-events-contracts-and-glue.md))

The `ToolDef`s (`knowledge.search`, `knowledge.page.read|create|append|summarise`, `knowledge.row.upsert`,
`knowledge.page.publish`, `knowledge.page.turn_into_issues`) are the one catalogue consumed internally and
exposable over MCP. Properties:

- **`knowledge.search`** = RAG over the corpus, permission-filtered (the `Filter` conjoin) — an agent never
  retrieves a doc its delegated principal can't see.
- **Side-effecting tools** route through `EffectApi::apply` → public endpoint → collab apply with agent
  attribution; consequential ones (publish, confidential edit, turn-into-issues) are HITL-gated by the **frozen
  `requires_approval` defaults** (X-6) — the Chat approval card with the per-effect `idem_key` resume.
- **`run --dry-run`** (contract 8.7): plan-then-apply testability — shows the proposed Knowledge effects
  without applying.
- **The agent-trace write path** ([03 §5.2](./03-events-contracts-and-glue.md)): a content-addressed Knowledge
  document holds each run's execution trace (AG-7).

### 3.3 The shared content/query crates the API speaks (the FROZEN shapes)

- **`myelin-content`** (contract 13.1, Knowledge leads): the block/inline taxonomy + the markdown-subset
  serialization is the wire shape for page bodies; the WASM core gives client/server parity (KN-4). Chat/Issues
  consume strict subsets.
- **`myelin-query`** (contract 13.3, Knowledge co-owns): the `FieldType` + `ViewSpec` + `QueryAst` + LexoRank
  `order_key` is the wire shape for database schemas, views, filters — the *same* (byte-identical) shapes the
  UI, CLI, automations, agents, and the bus `EventMatcher` speak.

Continue to [`05-hard-problems.md`](./05-hard-problems.md).
