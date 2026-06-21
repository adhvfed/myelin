# Icon usage map — components & surfaces ↔ the 42-icon registry

> **Phase 8 · `04-icons/`.** The two-way binding between the 42-icon library
> ([`ICONS-README.md`](./ICONS-README.md) §2 registry · [`dist/manifest.json`](./dist/manifest.json) ·
> [`dist/sprite.svg`](./dist/sprite.svg)) and the things that consume it — the
> [`02-components/`](../02-components/) specs and the [`05-user-facing-surfaces/`](../../05-user-facing-surfaces/)
> (§7) subsystems. Names here are the **registry contract keys** (the filename *is* the contract — §2). One
> canonical glyph per meaning, identical across every subsystem (design-language §3.7). **File date: 2026-06-21.**

> **Design-language rails honoured here:**
> - **Agents look like agents.** `agent` is the **plain geometric mark** (rounded square + centered dot) — the
>   shape channel of the four-channel agent signature ([identity-and-agent-badge](../02-components/identity-and-agent-badge.md) §3).
>   **Never** sparkle/shimmer/magic-wand/star/emoji.
> - **Status pairs glyph + colour, never colour-alone** (WCAG 1.4.1). The CI verdict trio (`check-pass` /
>   `check-fail` / `check-pending`) is the ring-enclosed status set; bare ✓/✗ (`approve` / `reject` / `close`)
>   is **action/chrome, not a verdict** (§2 glyph-family conventions).
> - **`chevron` is one glyph, rotated by CSS** for up/down/left/right and next/prev — never four files.
> - Icons inherit **`currentColor`** from the token-driven text/icon colour — no per-theme or per-direction files.

---

## A. Per component / surface → the canonical icons it uses

### Shell & nav — [`shell-and-nav.md`](../02-components/shell-and-nav.md)
The rail subsystem switcher + constant topbar chrome.

| Slot | Icons |
|---|---|
| Rail subsystem switcher | `nav-code` · `nav-ci` · `nav-issues` · `nav-knowledge` · `nav-chat` |
| Topbar constant chrome | `search` (global search) · `inbox` (notifications entry) · `settings` (admin/settings) · `human` (identity-menu fallback) |
| Sidebar tree / disclosure | `chevron` (expand/collapse, rotated) · `folder` · `repo` · `file` · `doc` · `channel` · `database` |
| Sidebar status hints | `check-pass` · `check-fail` · `check-pending` (a PR going green, a check flipping — glyph+colour) |
| Drawer / overlay dismiss (mobile) | `close` |

### Command palette — [`command-palette.md`](../02-components/command-palette.md)
The row atom is `[type icon] label · status hint`; the type icon disambiguates type for colour-blind users (§3).

| Slot | Icons |
|---|---|
| Navigate-mode type icons (jump to any `ArtifactRef`) | `repo` · `pull-request` · `issue` · `doc` · `channel` · `run` · `human` · `agent` · `settings` (admin) |
| Act-mode row icons | the verb's `ToolDef` glyph (e.g. `merge`, `run`, `rerun`, `edit`, `tag`) |
| Build-query / Search affordances | `search` · `priority` (a filterable field) |
| Agent-armable row | `agent` (the [Agent] badge mark) |
| Consequential-verb gate marker | `gate` (routes into the HITL/Confirm gate; pairs with `--warning` + the word "GATE") |
| Footer / chrome | `chevron` (↑↓ move hint) · `external-link` ("Open in Search view") |

### Reference chip + unfurl — [`reference-chip-and-unfurl.md`](../02-components/reference-chip-and-unfurl.md)
**The wedge.** The `type-icon` is the persistent identity, known from the URN `<type>` *before* resolution (§2.1)
— it is identical across every surface (board cell, editor mention, inbox subject, PR pane, chat unfurl).

| Artifact type | type-icon | status-glyph (stateful types, glyph+label) |
|---|---|---|
| PR | `pull-request` | `check-pass` / `check-fail` / `check-pending` (checks roll-up); `merge` (merged) |
| Issue / sub-issue | `issue` / `sub-issue` | state via `priority` + status glyph |
| Doc / block | `doc` | — |
| CI run / step | `run` | `check-pass` / `check-fail` / `check-pending`; `rerun` (re-run affordance) |
| Commit | `commit` | — |
| Thread / message | `message` | — |
| Repo / file / folder | `repo` / `file` / `folder` | — |
| Branch / tag | `branch` / `tag` | — |
| Person / Agent | `human` / `agent` | — (agent → the four-channel treatment) |
| Database view | `database` | — |
| Backlink / reference | `link` | — |
| Card action bar | `external-link` (open ↗) · `kebab` (overflow `···`) · `edit` (edit affordance) | — |

### Agent / HITL approval card — [`agent-hitl-card.md`](../02-components/agent-hitl-card.md)

| Slot | Icons |
|---|---|
| Agent treatment (header) | `agent` (the plain mark — **never** sparkle/star) |
| Per-effect gate marker | `gate` (consequential effect; pairs with `--warning` + the word "GATE") |
| Per-effect targets | the `<ReferenceChip>` type-icons (above) |
| Controls | `approve` (bare ✓) · `edit` (pencil) · `reject` (bare ✗) |
| Provenance / chain / audit | `link` (the clickable `correlation_id` thread / audit deep-link) · `external-link` |

### Comments / mentions / reactions — [`comments-mentions.md`](../02-components/comments-mentions.md)

| Slot | Icons |
|---|---|
| Comment header identity | `human` / `agent` (agent author → four-channel treatment) |
| Inline mention / ref nodes | `<ReferenceChip>` type-icons (`message`, `issue`, `doc`, …) |
| Anchored-thread anchor chip | the anchored target's type-icon + `link` |
| Per-comment actions | `message` (reply) · `kebab` (overflow: edit/delete/quote/copy-ref) · `link` (copy-ref) · `edit` |
| Review verdict (approve/request-changes/comment) | `approve` · `reject` · `message` — **always glyph + label** |

### Views (table · board · calendar · list · gallery · timeline) — [`views.md`](../02-components/views.md)

| Slot | Icons |
|---|---|
| Projection switcher | `roadmap` (timeline/Gantt lens) · `cycle` (calendar/cycle) · (board/table/list/gallery use generic projection glyphs — see gap list) |
| View-bar query controls | `search` (filter) · `priority` (group/sort field) · `settings` (visible fields ⊞) |
| Ref-typed cells | `<ReferenceChip>` type-icons |
| Status cells | `check-pass` / `check-fail` / `check-pending` · `priority` — **glyph + label** |
| Row disclosure / group collapse | `chevron` (rotated) |
| Agent-suggested/pending row | `agent` |
| Row overflow | `kebab` |

### Block / rich-text editor — [`block-editor.md`](../02-components/block-editor.md)

| Slot | Icons |
|---|---|
| Block drag handle | `kebab` (the six-dot handle exposes delete/duplicate/convert — the overflow mark) |
| Slash-menu insertable blocks | `doc` · `database` (`/database` live board) · `file` · `link` (`/embed`) · `issue` (`/issue`) |
| Inline mention / ref / embed nodes | `<ReferenceChip>` type-icons; `@agent` → `agent` |
| Disclosure (toggle blocks) | `chevron` |

### Notifications inbox — [`notifications-inbox.md`](../02-components/notifications-inbox.md)

| Slot | Icons |
|---|---|
| Surface entry (topbar) | `inbox` |
| Filter tabs tune | `settings` (⚙ tune) |
| Item kind glyphs | `message` (mention/reply) · `pull-request` (review request) · `issue` (assignment) · `check-fail` (CI failure on my work) · `gate` / `agent` (HITL approval requested) · `priority` (SLA / escalated) |
| Item subject | `<ReferenceChip>` (live, permission-aware) |
| Deduped group expand | `chevron` |
| Agent-activity group / HITL row | `agent` · `approve` · `edit` · `reject` (docked `<AgentHitlCard>`) |
| Triage actions | `check-pass` (done) · `external-link` (go/open) · `close` (mute/dismiss) |

### Identity & agent badge — [`identity-and-agent-badge.md`](../02-components/identity-and-agent-badge.md)
Specced once, inherited everywhere (the atom inside chip, HITL card, comments, views cells, editor node, inbox rows).

| `Principal.kind` | Icon |
|---|---|
| human | `human` (avatar fallback) |
| agent | `agent` (the plain geometric mark — the **shape** channel of the four-channel signature) |
| team | `team` (two people) |
| service | *(plain service glyph — see gap list)* |

### Forms & controls — [`forms-and-controls.md`](../02-components/forms-and-controls.md)

| Control | Icons |
|---|---|
| Button (leading/trailing) | any registry glyph, inheriting `currentColor` (e.g. `search`, `edit`, `external-link`) |
| Input (search variant / clear / validate) | `search` · `close` (clear) · `check-pass` (valid) |
| Select / Combobox | `chevron` (trigger) · `check-pass` (selected option) · `search` (type-to-filter) · `close` (remove multi-select chip) |
| Checkbox / radio / switch | `approve` (the checked ✓ glyph) — checked by glyph+position, not colour-alone |
| Field validation (error) | a danger glyph — **`check-fail`** ✗ pattern + `--danger` + message (never colour alone) |

### §7 user-facing surfaces — [`05-user-facing-surfaces/`](../../05-user-facing-surfaces/)
Each subsystem composes the components above; its rail entry + characteristic objects are listed.

| Surface | Rail icon | Characteristic object / status icons |
|---|---|---|
| **Git hosting & code review** (§7.1, [`git.md`](../../05-user-facing-surfaces/git.md)) | `nav-code` | `repo` · `branch` · `commit` · `pull-request` · `merge` · `tag` · `file` · `folder` · `link` (PR↔issue) |
| **CI/CD** (§7.2, [`ci.md`](../../05-user-facing-surfaces/ci.md)) | `nav-ci` | `run` · `rerun` · `check-pass` · `check-fail` · `check-pending` (the ring verdict set) |
| **Issue tracker** (§7.3, [`issues.md`](../../05-user-facing-surfaces/issues.md)) | `nav-issues` | `issue` · `sub-issue` · `priority` · `cycle` · `roadmap` (the Views projections) |
| **Knowledge platform** (§7.4, [`knowledge.md`](../../05-user-facing-surfaces/knowledge.md)) | `nav-knowledge` | `doc` · `folder` · `file` · `database` · `link` (backlinks) · `external-link` |
| **Chat** (§7.5, [`chat.md`](../../05-user-facing-surfaces/chat.md)) | `nav-chat` | `channel` · `message` · `human` · `agent` · `team` |
| **Shared / identity / admin / GDPR & sovereignty** (§7.6, [`shared-admin-sovereignty.md`](../../05-user-facing-surfaces/shared-admin-sovereignty.md)) | `settings` | `human` · `team` · `agent` · `gate` (HITL/authority) · `settings` · `link` (audit chain) |
| **CLI** (§7.7, [`cli.md`](../../05-user-facing-surfaces/cli.md)) | — (text surface) | none — the CLI renders the registry's *meanings* as text/Unicode, not the SVGs |

---

## B. Per icon → where it is used

| Icon | Group | Consumers |
|---|---|---|
| `nav-code` | nav | shell rail · git surface |
| `nav-ci` | nav | shell rail · CI surface |
| `nav-issues` | nav | shell rail · issues surface |
| `nav-knowledge` | nav | shell rail · knowledge surface |
| `nav-chat` | nav | shell rail · chat surface |
| `inbox` | nav | shell topbar · notifications-inbox (entry) |
| `search` | nav | shell topbar · palette · views view-bar · forms (search input/combobox) |
| `settings` | nav | shell topbar · palette (admin) · views (visible-fields) · inbox (tune) · admin surface |
| `branch` | git/ci | reference-chip (branch) · git surface · HITL edit-diff (branch rename) |
| `merge` | git/ci | reference-chip (PR merged) · palette (Act) · git surface |
| `commit` | git/ci | reference-chip (commit) · git surface |
| `pull-request` | git/ci | reference-chip (PR) · palette · inbox (review request) · git surface |
| `tag` | git/ci | reference-chip (tag) · palette (Act) · git surface |
| `run` | git/ci | reference-chip (CI run) · palette · CI surface |
| `rerun` | git/ci | reference-chip (re-run affordance) · palette · CI surface |
| `check-pass` | git/ci | shell status · chip status · views status cells · inbox (triage done) · forms (valid/selected) · CI surface |
| `check-fail` | git/ci | shell status · chip status · views status cells · inbox (CI failure) · forms (error pattern) · CI surface |
| `check-pending` | git/ci | shell status · chip status · views status cells · CI surface |
| `issue` | issue/work | reference-chip (issue) · palette · editor (`/issue`) · inbox (assignment) · issues surface |
| `sub-issue` | issue/work | reference-chip (sub-issue) · issues surface |
| `priority` | issue/work | palette (filter) · views (group/sort/status) · inbox (SLA/escalated) · issues surface |
| `cycle` | issue/work | views (calendar/cycle) · issues surface |
| `roadmap` | issue/work | views (timeline/Gantt projection) · issues surface |
| `repo` | objects | shell sidebar · reference-chip · palette · git surface |
| `file` | objects | shell sidebar · reference-chip · editor slash-menu · git/knowledge surfaces |
| `folder` | objects | shell sidebar · reference-chip · git/knowledge surfaces |
| `doc` | objects | shell sidebar · reference-chip · editor slash-menu · knowledge surface |
| `database` | objects | shell sidebar · reference-chip · editor (`/database`) · knowledge surface |
| `channel` | objects | shell sidebar · reference-chip · chat surface |
| `message` | objects | reference-chip (thread) · comments (reply/verdict) · inbox (mention) · chat surface |
| `link` | objects | reference-chip (backlink/copy-ref) · HITL (correlation/audit) · comments (anchor/copy-ref) · editor (`/embed`) · knowledge backlinks · admin audit chain |
| `human` | principals | identity badge · palette · comments · inbox · chat/admin surfaces |
| `agent` | principals | identity badge (the mark) · palette (armable) · HITL card · comments · views · editor · inbox · chat/admin surfaces |
| `team` | principals | identity badge · chat/admin surfaces |
| `approve` | agent/HITL | HITL card (bare ✓) · comments (review verdict) · inbox (HITL row) · forms (checkbox checked glyph) |
| `edit` | agent/HITL | HITL card (pencil) · reference-chip action bar · comments (edit) · inbox (HITL row) |
| `reject` | agent/HITL | HITL card (bare ✗) · comments (review verdict) · inbox (HITL row) |
| `gate` | agent/HITL | palette (consequential-verb gate) · HITL card (per-effect gate) · inbox (approval-requested) · admin/authority surface |
| `chevron` | chrome | shell sidebar · palette · views (disclosure/group) · editor (toggle) · forms (select trigger) — **one glyph, CSS-rotated** |
| `kebab` | chrome | reference-chip (overflow) · comments (overflow) · editor (block handle) · views (row overflow) |
| `close` | chrome | shell (mobile drawer) · inbox (mute/dismiss) · forms (input clear / chip remove) |
| `external-link` | chrome | reference-chip (open ↗) · palette (open in view) · HITL (audit) · inbox (go/open) · knowledge surface |

**Every one of the 42 icons has at least one consumer** (no orphan glyphs).

---

## C. Gap list — surfaces that need an icon NOT in the 42

These are real surface needs that the 42-icon core set does not yet cover. They are **backlog icons** under the
*identical* spec (`ICONS-README.md` §6 / `00-requirements.md` §1.2) — more of the same set, never a second style.
Until they ship, the listed fallback applies.

| Gap | Needed by | Suggested name | Interim fallback |
|---|---|---|---|
| **Board / kanban projection** glyph | views projection switcher | `view-board` | text label "Board" + projection toggle |
| **Table projection** glyph | views projection switcher | `view-table` | text label "Table" |
| **List projection** glyph | views projection switcher | `view-list` | text label "List" |
| **Gallery projection** glyph | views projection switcher | `view-gallery` | text label "Gallery" |
| **Service principal** glyph | identity badge (`kind=service`) | `service` | `agent`-adjacent plain glyph + the word "Service" |
| **Reaction / emoji-picker** affordance (the `+` add-reaction control, distinct from emoji content) | comments ReactionBar | `react` (or reuse a `+`/`add`) | text "+" add-reaction button |
| **Snooze** triage action | notifications inbox | `snooze` (clock-variant) | could reuse `check-pending`'s clock motif; text "Snooze" interim |
| **Mute / bell-off** triage action | notifications inbox | `mute` | text "Mute" / reuse `close` interim |
| **Attachment / paperclip** | editor, comments, chat composer | `attachment` | text "Attach" interim |
| **Generic "add / new"** (`+`) | views (new row), editor, lists | `add` | native "+" character / button label interim |
| **Copy** (copy-to-clipboard, distinct from copy-*ref* which is `link`) | code blocks, refs | `copy` | text "Copy" interim |
| **Filter** (if a dedicated funnel is wanted vs reusing `search`) | views, palette Build-query | `filter` | **covered today by `search`** — only a gap if a distinct funnel glyph is desired |

**Notes**
- The **projection switcher** (4 missing glyphs) is the largest concrete gap; `roadmap` (timeline) and `cycle`
  (calendar) are the only projection glyphs in the core 42. Recommend a `view-*` mini-family next.
- `filter` is **arguably already covered** by `search` (the registry's `search` meaning is "global / scoped
  search" with `filter` in its tags) — promote only if a distinct funnel is wanted.
- No gap touches the **agent** or **status** rails: the agent mark and the CI verdict trio are complete.

---

*Names are registry contract keys; re-skin/recolor is via `currentColor` + the token table, never per-icon edits.
Backlog icons inherit the §3 visual spec exactly. Not committed.*
