# Knowledge Platform — Information Architecture

> Phase 4, Knowledge, **design sketch** (REQUIRED before architecture; VISION §3/§5.2). Fits the
> ONE-SHELL design language (design-language §5.1 rail + contextual nav + header) and the §7 view
> catalogue (§7.4 Knowledge). Inherits the shared components (§5), tokens (§3), accessibility (§4),
> and the agent surfaces (§6). Day-one UX primitives (§8b) are applied in `wireframes.md`.

---

## 1. Where Knowledge sits in the one shell

Knowledge is one area in the persistent shell every subsystem composes into (design-language §5.1) —
switching to Knowledge must never feel like switching apps (P1). The shell has three constant regions;
Knowledge owns only its **secondary nav** + **content**, never the rail or header.

```
┌────────────────────────────────────────────────────────────────────────────────────┐
│ HEADER:  ⌘K command palette · global search · Inbox · [Principal] · space/tenant scope│  ← shell-owned
├──────┬───────────────────────────────┬─────────────────────────────────────────────┤
│ RAIL │  SECONDARY NAV (Knowledge)    │  CONTENT                        │ CONTEXT PANE│
│ Code │  ┌─────────────────────────┐  │                                 │ (right,     │
│ CI   │  │ Spaces ▸ Pages ▸ Subpages│  │   The block editor / a DB view  │  optional)  │
│ Issue│  │  · Favorites / Pins      │  │   / history / settings          │  Backlinks  │
│▸Know │  │  · Recent                │  │                                 │  Outline    │
│ Chat │  │  · Trash                 │  │                                 │  Comments   │
│ Inbox│  │  [+ New page]            │  │                                 │  Page info  │
│      │  └─────────────────────────┘  │                                 │             │
└──────┴───────────────────────────────┴─────────────────────────────────────────────┘
  ▲ shell-owned       ▲ Knowledge-owned secondary nav   ▲ Knowledge content   ▲ shell context-pane slot
```

- **Rail** (shell-owned): the subsystem switcher (Code · CI · Issues · **Knowledge** · Chat · Inbox ·
  Search). Knowledge highlights when active. On mobile the rail collapses to a toggled drawer
  (design-language §8b.4 drawer pattern).
- **Secondary nav** (Knowledge-owned): the **space → page → sub-page tree** (the navigation sidebar,
  §7.4) — favorites/pins, recent, trash, `+ New page`. The tree is virtualised for deep trees
  (loading state in wireframes). A page is both a doc and a folder (pure-pages model, sketch 04), so
  nesting in the tree *is* the folder hierarchy.
- **Content**: the active surface — the block editor (default), a database view, history, or a
  settings/sharing dialog (overlay).
- **Context pane** (shell context-pane slot, §5.1/§5.5): right-hand, optional, tabbed —
  **Backlinks** ("linked references", §5.3 — the reference graph made visible, P6), **Outline**
  (page headings), **Comments** (the discussion thread), **Page info** (residency/visibility/lawful-
  basis cue, P9). The pane is where the system *assembles context* the user never has to (§8b.6).

## 2. The Knowledge object hierarchy (navigation model)

```
Tenant
 └─ Space (teamspace; maps to platform org→team→project; permission + membership boundary)
     └─ Page  (a doc AND a folder — pure-pages; the addressable, permissioned, referenceable unit)
         ├─ Blocks (the ordered tree; stable block_id = #sub ref target; partial/lazy load)
         ├─ Sub-pages (nesting = the folder hierarchy)
         └─ Database (lives in a page)
             ├─ Schema (typed field definitions — shared ADR-06 primitive)
             ├─ Rows (property bag per row; row-as-page peek)
             └─ Views (table/board/calendar/list/gallery/timeline — shared §5.6 component)
```

Every node is an `ArtifactRef` down to sub-artifact granularity: `myelin://<t>/knowledge/page/<id>`,
`…/knowledge/block/<page>#b<id>`, `…/knowledge/database/<id>`, `…/knowledge/row/<id>`,
`…/knowledge/view/<id>` (sketch 07). Deep-linkable URLs for every one (the shell rule, §5.1) — a chat
message / issue / PR references a *block* precisely.

## 3. Primary screens (the §7.4 catalogue, mapped to this IA)

Each gets empty/loading/error/permission-denied/erased states (wireframes); each is keyboard-operable +
accessible + i18n/RTL-ready (§3–§4).

| # | Screen | Shell region | Notes |
|---|---|---|---|
| S1 | **Block editor (page view)** | content | the core surface; one editor over `myelin-content` (§5.9); live presence; agent-suggesting state (§6). |
| S2 | **Database views** | content | table/board/calendar/list/gallery/timeline (shared §5.6); row peek → row-as-page. |
| S3 | **Navigation sidebar / tree** | secondary nav | spaces→pages→subpages, favorites, recent, trash; quick-switcher via ⌘K. |
| S4 | **Backlinks / references pane** | context pane | "linked references / mentioned in"; permission-filtered; hover-peek; tombstone-aware. |
| S5 | **Comments / discussion** | context pane + inline | anchored comments (range/block/`#sub`); shared comment/thread primitive. |
| S6 | **Page history** | content (overlay/full) | version timeline + diff + restore; erased-segment redacted state. |
| S7 | **Sharing / permissions dialog** | overlay (portal) | inherited-vs-overridden ACL; publish-to-web with lawful-basis prompt. |
| S8 | **Templates** | overlay / gallery | insert-from-template; new-from-template; GDPR-flagged PII templates. |
| S9 | **Search / quick-switcher** | header overlay (palette) + full search view | cross-type, permission-filtered, multilingual, semantic toggle. |
| S10 | **Agent affordances** (woven) | inline + context pane + Chat | agent presence, "suggested by agent" attribution, accept/reject, "ask an agent", HITL cards (surface in Chat + Inbox). |
| S11 | **Export** | overlay | per-page/space/workspace → MD/JSON/PDF/CSV; lossless JSON = portability + DSAR spine. |
| S12 | **Mobile read + light-edit** | full-screen, drawer nav | read-anywhere + optimistic light-edit-online (offline depth = CRDT-promotion follow-on, sketch 01). |

## 4. Navigation flows (how a user moves through the IA)

- **Find → open**: ⌘K (palette, §5.2) → type → jump to any page/db/row/block (permission-pre-filtered
  via `list_objects`, §5.7 — you only find what you may see). OR sidebar tree → click. OR a reference
  chip anywhere on the platform → opens the page/block (the wedge, P6).
- **Read → reference**: open a page → context pane Backlinks shows "what references this" → hover-peek
  a referenced issue/PR/run without leaving (the system assembles context, §8b.6).
- **Edit → collaborate**: type in the editor → optimistic local apply → live presence cursors of
  co-editors → coalesced semantic event to the bus (agents/Search/Refs react). CAS floor: a
  same-block conflict surfaces as a soft-lock "Bob is editing" cue (sketch 01).
- **Structure → database**: `/database` inserts an inline DB → add typed fields → switch view
  (table↔board↔calendar) → filter/sort/group → row peek opens row-as-page.
- **Govern → share/publish/erase**: sharing dialog (inherited vs override) → publish-to-web (lawful-
  basis prompt) → DSAR export/erase via the GDPR console (shell §7.6) for a subject.
- **Agent**: "Ask agent → turn this into issues" → agent proposes effects (plan-then-apply) → the
  approval card (Chat + Inbox) → approve → effects apply, page edited *through the collab protocol*
  with "suggested by agent" attribution the author can accept/reject.

## 5. Overlay inventory (portal-always, §8b.1)

Every overlay portals to the document root, shares the one z-index scale (chrome < popover < modal <
toast), and inherits focus-trap/return-focus/scroll-lock/Escape/ARIA from the shared primitive
(§8b.1). Knowledge's overlays: the **slash-command menu** (popover, anchored to caret — must flip when
near the viewport bottom, §8b.4), the **`@`/`#` mention-ref autocomplete** (popover, anchored to
caret), the **block drag-handle menu** (dropdown), the **sharing dialog** (modal), the **export
dialog** (modal), the **template gallery** (modal), the **history diff** (full-surface or modal), the
**row peek** (modal/side-panel), **link/embed unfurl hovercards** (popover), **confirm-restore /
confirm-delete** (confirm dialog — but per §8b.6 prefer undo over confirm for reversible deletes;
confirm only irreversible/erase/publish).

## 6. The one editor render path (the IA consequence of KN-4)

The block editor (S1), the comment composer (S5), and database row free-text cells all use **one editor
component over `myelin-content`** (§5.9) running **one `parseInline` pipeline for read and edit** (KN-4).
Concurrency differs (collab for pages, single-author for comments/cells) but the editor *component +
AST + the md-subset string* are shared (ADR-05). This is the IA-level guarantee that "writing feels the
same everywhere" — and it's why the editor primitives (serializer / offset model / DOM-surgery) ship +
unit-test standalone before any screen consumes them (§8b.2).

## 7. Cross-references

- design-language §5.1 (shell), §5.3 (reference chip/unfurl), §5.5 (comments), §5.6 (views
  component), §5.9 (editor), §6 (agent surfaces), §7.4 (Knowledge view catalogue), §8b (day-one
  primitives).
- sketches 01 (collab/transport), 02 (block tree/content), 03 (db/formula), 04 (permissions), 05
  (transclusion/embed liveness), 06 (GDPR/agent trace), 07 (taxonomy/search/refs/multi-region).
- `knowledge-platform.md` §4 (views/screens), §7 (interactions).
