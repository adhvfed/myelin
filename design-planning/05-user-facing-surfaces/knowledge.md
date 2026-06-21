# Surface group: Knowledge platform (§7.4)

> Phase 5 surface map · group **K** · maps [`design-language §7.4`](../../planning/02-holistic-architecture/design-language.md)
> against the [§2 template](./README.md#2-the-per-surface-map-template). Pointer map; PROVEN / HOUSE
> STYLE tagged; date 2026-06-20. Cross-cutting obligations ([README §3](./README.md#3)) inherited.
> **K-2 (database views) is the D2 reuse seam** — the *same* §5.6 views component as Issues; **K-1 (the
> block editor) is the one editor** used by every body across the platform.

---

## K-1 — The block editor (page)
1. **Jobs:** M2 (spec *is* linked to delivery, not a dead copy), E5 (read code/decision context). 2. **IA + shell:** `Knowledge → <space> → <page>`; content; contextual sidebar = space→page tree (K-3).
3. **Components:** **it IS the §5.9 / R-10 §3 editor** — the *same* editor used for issue bodies, PR descriptions, chat composition; chip/unfurl as first-class inline nodes; embedded views (R-10 §2 db-view embeds).
4. **Density:** 0.4 — earns via J1 (long-form prose canvas) + J3 (slash-menu blocks); "page-ness" is layout, not a fork (R-07 §2).
5. **Agent:** **DocBot** proposes edits (M9 — "this doc likely went stale") as an agent-pending card; suggest-not-auto, never silent edit (R-14 doctrine). 6. **Sovereignty:** visibility chip; export-safe (open formats, K-8); erased blocks tombstone.
7. **State set (R-21 §2e — collab editor **owns** conflict + reconnect, cols 10/8):** block-skeleton (ghost heading + paragraph lines, R-13 §A.2); saving/pending (optimistic, content **never lost** on error); **conflict** (collab — CAS→CRDT shown, never silent overwrite, OPT-3); offline/reconnecting.
8. **A11y (R-17 §5.4 block-editor hard component):** keyboard (slash-insert, Enter-split, Backspace-merge, mark toggles, block move, no trap on chips, Tab exits); SR (block type announced, **markdown round-trip as fallback**, mention/ref chip announces humanised name+type, **IME/composition for CJK + accented EU**). **PROVEN day-one mandates (§8b.2 / R-10 §3.1):** one render path (read+edit same parser); `render(parse(md)) === md` CI gate; markdown-subset string for inline (survives copy/paste/export); controlled contenteditable + char-offset caret. **G2:** per-block `dir="auto"`, block handles to inline-start, slash-menu mirrors; line-height diacritic headroom (Greek/Cyrillic, R-18 §3.2).
9. **Device (MOB-1):** backlink/handle hover-actions touch-reachable; **contenteditable on mobile is the top variance risk** (R-10 §6.3) — slash-menu becomes a bottom sheet; read-friendly, authoring usable.
10. **Wedge/motion:** `motion.settle` on block commit; born-linked artifact (a doc created from a fix lands already chipped on the PR, R-20 D-S2). 11. **DoD + switch:** "Enter splits a block" works (the #1 "not a real editor" tell, §8b.2); open-format export means no Notion-style lock-in (R-01 §2.1 trap); a team writes a PRD that *is* linked to its epics (M2), not a dead Confluence copy.

## K-2 — Database views *(D2 — same component as Issues)*
1. **Jobs:** D2 (same records, engineer table ↔ PM board/gallery/timeline). 2. **IA:** `Knowledge → <space> → <db>`; content; **same views tree shape** as Issues (R-06 §3.2 reuse seam).
3. **Components:** **the SAME §5.6 views component (R-10 §2) as Issues** — over `db-row` instead of `issue`. This is the biggest reuse boundary (ADR-06); distinctness here would re-fracture it (R-07 §2).
4. **Density:** 0.5. **Persona lenses (R-16 D2):** L1 engineer table/compact, L2 PM/designer board-gallery/comfortable — *config delta, not forked code*. 5. **Agent:** same as Issues views. 6. **Sovereignty:** permission-aware rows by construction (ADR-03 — a view can never show rows the viewer can't see).
7. **State set (R-21 §2e):** same as Issues views (R-10 §2.2 — empty/loading/error/permission/erased-cell/optimistic/live-update/conflict). 8. **A11y:** **same R-17 §5.3 views-inline-edit hard component** (grid `role=grid`, Tab-stop, F2 edit). **G2:** columns mirror in RTL, frozen-first-column → inline-start. 9. **Device:** same as Issues views (MOB-1 cell-overflow; horizontal scroll). 10. **Wedge/motion:** same view-projection motion (`motion.move` on board-drag). 11. **DoD + switch:** a knowledge DB and an issue board are *visibly the same component tuned by projection* (D4); two DBs never drift (the reference graph stays whole, R-03 D2 "endangered if forked").

## K-4 — Backlinks & references panel *(wedge)*
1. **Jobs:** E5 (trace why a line exists, live), M2/M8 (intent↔delivery↔design trail). Flow **F-ENG-2 / F-PM-1**. 2. **IA:** `Knowledge → <page> → Backlinks` (and on every artifact); context pane. 3. **Components:** chip/unfurl (R-09 — the panel *is* a list of live chips).
4. **Density:** 0.3. 5. **Agent:** agent-authored refs appear with treatment. 6. **Sovereignty:** **leak-free by construction** — a backlink the viewer can't access → no-access card, never the title (R-09, R-02 R-LEAK-1).
7. **State set (R-21 §2e):** empty ("no linked references yet"); cross-cell backlink → projection-or-tombstone; moved/outdated. 8. **A11y/i18n:** panel landmark; chips keyboard-navigable, hover-peek = focus-peek (WCAG 1.4.13). **G2:** humanised chip titles. 9. **Device (MOB-1):** backlink peek hover → tap; panel → drawer.
10. **Wedge/motion:** **W5 (backlinks appear automatically — a reverse trail no one curated)** — event-sourced "Linked references / Mentioned in"; `motion.enter` on hovercard (300ms intent-delay, R-12 §3). 11. **DoD + switch:** the reverse trail is automatic and live across all five subsystems (no manual cross-tool hunt, the stitched-stack regression W5 dissolves).

## K-3 / K-5 / K-7 / K-9 — sidebar tree · page history · sharing/permissions · search palette
- **K-3 Navigation / sidebar tree**: `Knowledge → <space>` (sidebar); spaces→pages→sub-pages, favorites/pins, breadcrumb, quick-switcher (palette S-2). Density 0.3. **G2:** tree-disclosure chevrons mirror in RTL (R-18 §4.2). MOB-2: sidebar → drawer.
- **K-5 Page history UI**: `Knowledge → <page> → History`; version timeline, diff, restore. Density 0.4. Diff reuses G-7-class rendering. Reversibility-over-confirmation (restore, not "are you sure", R-13 OPT-2).
- **K-7 Sharing & permissions UI**: `Knowledge → <page> → Sharing`; page-tree ACL inheritance with overrides, share-with-link, guest, public-publish. Density 0.4. **No-leak is the trap (R-02 R-LEAK)**: visibility chip (R-19 §1.2) shows effective access; privacy-by-default = Private. Desktop-mainly admin.
- **K-9 Search palette**: knowledge-scoped + cross-artifact (R-08, ADR-03 pre-filtered). Density 0.3. Lives at `[G] Search` scoped.

## K-6 — Templates UI *(admin-ish; flow-orphaned — job-linked)*
1. **Job link ([README §4.2](./README.md#42)):** **M2** (P6/P10 — spec/PRD/runbook standardisation) + **R-20 onboarding** ("new from template" is the empty-state CTA at first-doc, startup rung 0 / scale-up). Used at first-doc creation and when a team standardises PRDs/runbooks. 2. **IA:** `Knowledge → <space> → Templates`; template gallery. 3. **Components:** views (gallery), editor (template body).
4. **Density:** 0.3. 5. **Agent:** n/a. 6. **Sovereignty:** template export-safe. 7. **State set (R-21 §2e):** empty gallery = onboarding-forward (R-20 Law A — teach by doing, no tour). 8. **A11y/i18n:** gallery keyboard-navigable; humanised template names. 9. **Device:** read + pick on mobile; authoring desktop-mainly.
10. **Motion:** `motion.enter` on "new from template". 11. **DoD + switch:** "new from template" is the onboarding-forward empty action, not a patronising tour (R-20 Law A); a team standardises docs without a config-maze.

## K-8 — Export UI *(admin; portability — buyer-decisive)*
1. **Job link:** **G9** (P14/P13/P15 — data-portability/exit, the anti-lock-in promise). 2. **IA:** `Knowledge → <space> → Export`. 3. **Components:** overlays (export dialog). 4. **Density:** 0.3. 5. **Agent:** n/a. 6. **Sovereignty (PROVEN — P14/ADR-12):** per-page/space/workspace export to **Markdown/open formats**; exports stay in-region. 7. **State (R-21 §2g):** export progress; partial-failure isolated. 8. **A11y/i18n:** dialog a11y; locale-aware. 9. **Device:** trigger on mobile, download desktop. 10. **Motion:** progress, not spinner-on-blank. 11. **DoD + switch:** clean open-format export means a team can *leave* without losing their knowledge — the anti-lock-in promise made operable (R-01 §2.1 Notion-export trap dissolved).

---

**Group invariants reminder:** K-1 is the *one editor* (the same node taxonomy as issue/PR/chat bodies,
ADR-05) and K-2 is the *one views component* (the same as Issues, ADR-06). The reuse is the coherence
guarantee — a knowledge DB that grew its own table UI, or a page editor that diverged from the issue
editor, would fork the product (R-07 §3 invariants 6).
