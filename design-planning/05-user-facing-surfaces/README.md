# Phase 5 — The User-Facing Surface Map

> Phase: `design-planning/05-user-facing-surfaces`. **This is a MAP, not per-screen visual design.**
> It synthesises the Phase-4 research corpus ([`04-research/**`](../04-research/), 22 files + the
> [`_completeness-critic.md`](../04-research/_completeness-critic.md)) into one navigable overview of
> every user-facing surface in Myelin, so Phase 6 (the [sketch funnel](../02-research-roadmap/sketch-funnel.md))
> and the final human decision have a single, pointer-dense target. It does **not** re-derive the
> corpus — it points into it. **Status date: 2026-06-20.**
>
> **What this map is built on (never re-derived):** the view catalogue
> [`design-language.md §7`](../../planning/02-holistic-architecture/design-language.md) (+ §1–§6
> principles/components/agent contract, §8b day-one primitives); the IA backbone
> [`R-06 platform-ia`](../04-research/ia/platform-ia.md) §2 (one tree) / §3 (shell regions) / §4
> (global surfaces) / §5 (`ArtifactRef` address space); the unification ruling
> [`R-07 unification-study`](../04-research/ia/unification-study.md) §2 (per-surface Axis-3 position) /
> §3 (the eight invariants); the jobs [`R-03 jtbd-catalogue`](../04-research/jtbd-flows/jtbd-catalogue.md)
> (E1–E12 / M1–M10 / G1–G10 + the D1–D5 dual-audience pairs); the flows
> [`R-04 cross-surface-flows`](../04-research/jtbd-flows/cross-surface-flows.md) (F-ENG-1/2, F-PM-1/2,
> F-GOV-1, F-AGT-1); and the component/craft/a11y/agent/sovereignty specs R-08…R-22.
>
> **Honesty (VISION §3 rule, carried from the corpus):** every claim below is **PROVEN** (a cited
> standard / an existing architecture contract surfaced) or **HOUSE STYLE** (our taste/synthesis).
> The corpus is **expert-led, not user-validated**; the eight `[DEFERRED-UNTIL-USERS]` flags
> (R-03 ODI ranking, R-05 personas, R-07 card-sort, R-15 PAIR trust, R-16 both-audience, R-17 AT-user,
> R-19 regulated-buyer, RITE-on-sketches) are **carried forward, not substituted**. This map adds **no
> new validation** — it organises what exists and resolves the four critic seams.

---

## 0. How to read this map

| § | What it gives you |
|---|---|
| §1 | **The surface inventory table** — every primary surface from §7, grouped, with ID · audience · IA placement · density (R-07 Axis-3) · research files behind it (the **0-orphans proof**). |
| §2 | **The per-surface map template** — the consistent 11-item schema each surface entry uses (applied in the per-group files). |
| §3 | **Cross-cutting obligations** — what *every* surface inherits (shell · gates G1/G2 · the state set · perf budgets · motion · agent legibility · sovereignty). |
| §4 | **The critic-seam resolutions** — the four [`_completeness-critic.md`](../04-research/_completeness-critic.md) fixes (touch/mobile · flow-orphaned admin · chat threading · vocabulary fracturing). |
| §5 | **The highest-leverage surfaces for the sketch funnel** — the recommended comparable-screen set for Phase 6, with rationale. |
| §6 | **Completeness-critic mini-pass** — what *this* map glossed. |
| — | **Per-group files:** [`git.md`](./git.md) · [`ci.md`](./ci.md) · [`issues.md`](./issues.md) · [`knowledge.md`](./knowledge.md) · [`chat.md`](./chat.md) · [`shared-admin-sovereignty.md`](./shared-admin-sovereignty.md) · [`cli.md`](./cli.md) — each surface mapped against the §2 template. |

---

## 1. The surface inventory table (every primary surface from §7 — 0 orphans)

**Reading the columns.** **ID** = this map's stable handle (`G-…` git, `C-…` CI, `I-…` issues, `K-…`
knowledge, `H-…` chat, `S-…` shared/admin/sovereignty, `X-…` CLI). **Audience** = R-03 cluster(s)
(Eng / PM / Gov; *shell* = all). **IA placement** = R-06 §2 tree node. **Density (R-07)** = the
Axis-3 position from [`unification-study §2`](../04-research/ia/unification-study.md#2) (`0.0` maximally
unified ↔ `1.0` maximally distinct; the number tunes the **content region only** — all eight §3
invariants hold regardless). **Research behind it** = the files that spec/place/state-craft it (the
orphan check: every row has ≥1 flow **or** pattern spec **or** R-21 state-matrix row, per
[`_completeness-critic §3`](../04-research/_completeness-critic.md)).

### 1.1 Git hosting & code review — [`git.md`](./git.md) (§7.1)

| ID | Surface | Audience | IA placement (R-06 §2) | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **G-1** | Repository home | Eng | `Code → <repo>` | 0.3 | R-21 §2b · R-06 §8 · R-01 §4 |
| **G-2** | File tree & file view (+ blame, permalink-by-SHA, LFS) | Eng | `Code → <repo> → Code @ref` | 0.4 | R-04 F-ENG-2 (E5) · R-21 §2b · R-22 W5 |
| **G-3** | History / commit views (signature verify) | Eng | `Code → <repo> → history` | 0.4 | R-04 F-ENG-2 · R-21 §2b |
| **G-4** | Compare view (arbitrary ref/SHA diff) | Eng | `Code → <repo> → Compare` | 0.85 | R-09 §5.9 · R-21 §2b |
| **G-5** | Code search | Eng | `Code → <repo> → Code search` | 0.4 | R-03 E6 · R-08 (search) · R-21 §2g |
| **G-6** | **PR overview / context pane** *(wedge flagship)* | Eng | `Code → <repo> → PR → Overview` | 0.4 | R-04 F-ENG-1 (E1/E2) · R-22 **W1** · R-13 CA-2 · R-21 §2b |
| **G-7** | **Diff / files-changed** *(densest engineer surface)* | Eng | `Code → <repo> → PR → Diff` | **0.85** | R-04 F-ENG-1 · R-07 §2.1 · R-09 §5.9 · R-17 §5.1 · R-22 W4 · R-21 §2b |
| **G-8** | Review surface (verdicts, batched, agent-aware) | Eng | `Code → <repo> → PR → Review` | 0.6 | R-03 E4 · R-01 §4.3 · R-02 R-BATCH · R-14 §6.1 · R-21 §2b |
| **G-9** | Checks / CI integration panel | Eng | `Code → <repo> → PR → Checks` | 0.6 | R-04 F-ENG-1 (E3) · R-22 W4 · R-21 §2b |
| **G-10** | **Branch-protection / ruleset editor** *(admin)* | Eng/Gov | `Code → <repo> → [A] settings` | 0.4 | R-02 §1.1 · R-21 §2b · §4.2 job-link below |
| **G-11** | Repo settings (collaborators, webhooks, keys) | Eng/Gov | `Code → <repo> → [A] settings` | 0.4 | R-03 E12 · R-21 §2b |

### 1.2 CI/CD — [`ci.md`](./ci.md) (§7.2)

| ID | Surface | Audience | IA placement | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **C-1** | Run list / dashboard ("is main green?") | Eng | `CI → <pipeline\|repo runs>` | 0.6 | R-03 E10 · R-21 §2c |
| **C-2** | Single-run view (DAG, jobs, steps) | Eng | `CI → <run> → DAG` | 0.8 | R-04 F-ENG-1 (E3) · R-21 §2c |
| **C-3** | **Live log view** (streaming, masked) | Eng | `CI → <run> → Logs` | **0.8** | R-04 F-ENG-1 · R-07 §2 (log 0.8) · R-17/R-21 §2c |
| **C-4** | Matrix view (fan-out grid) | Eng | `CI → <run> → Matrix` | 0.8 | R-21 §2c · §7.2 |
| **C-5** | **Pipeline / definition editor + validator** *(admin)* | Eng | `CI → <…> → Pipeline editor` | 0.5 | R-03 E8 · R-02 §1.1 · R-21 §2c · §4.2 job-link |
| **C-6** | Environments & deployments (+ approvals queue) | Eng/Gov | `CI → Environments` | 0.5 | R-04 F-AGT-1 (HITL) · R-14 §6.3 · R-21 §2c |
| **C-7** | Secrets management *(admin)* | Eng/Gov | `CI → [A] Secrets` | 0.4 | R-03 E12 · R-21 §2c |
| **C-8** | Usage / quota / billing view | Gov | `CI → [A] Usage` | 0.45 | R-03 E10/G1 · R-21 §2g |
| **C-9** | Agent-surfaced triage view | Eng/cross | `CI → <run> → triage` | 0.5 | R-04 F-AGT-1 · R-14 §6.4 · R-21 §2c |

### 1.3 Issue tracker — [`issues.md`](./issues.md) (§7.3)

| ID | Surface | Audience | IA placement | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **I-1** | Issue detail view | Eng/PM | `Issues → <project> → <issue>` | 0.45 | R-03 E1/M2 · R-09 (refs) · R-21 §2d |
| **I-2** | **Issue board / cycle** *(engineer lens, D1)* | Eng | `Issues → <project> → view` | **0.55** | R-03 D1 · R-07 §2.1 · R-16 D1/L1 · R-21 §2d |
| **I-3** | **Roadmap / timeline** *(PM lens, D1)* | PM | `Issues → <project> → roadmap` | **0.5** | R-04 F-PM-2 (M1) · R-07 §2.1 · R-16 D1/L2 · R-21 §2d |
| **I-4** | Portfolio / exec rollup *(exec lens, D1)* | Gov/PM | `Issues → portfolio` | 0.45 | R-03 G1 · R-16 D1/L3 · R-21 §2d |
| **I-5** | List / table / calendar views | Eng/PM | `Issues → <project> → view` | 0.55 | R-10 §2 · R-16 · R-21 §2d |
| **I-6** | Cycle (sprint) view (capacity, burndown) | Eng/PM | `Issues → <project> → Cycle` | 0.5 | R-03 M3 · R-21 §2d |
| **I-7** | Triage inbox (agent-assisted dedup/label) | Eng/PM | `Issues → <project> → Triage` | 0.55 | R-03 E11/M6 · R-14 §6.2 · R-21 §2d |
| **I-8** | "My Work" hub | Eng/PM | `[G] Home` | 0.3 | R-03 M5 · R-06 §7 · R-21 §2d |
| **I-9** | Dashboards (charts, SLA gauges) *(D5)* | PM/Gov | `Issues → Dashboards` | 0.4 | R-03 M4/D5 · R-16 D5 · R-21 §2d |
| **I-10** | Saved views management | Eng/PM | `Issues → Saved views` | 0.3 | R-10 §2 · R-21 §2d |
| **I-11** | **Workflow / SLA / field-scheme admin** *(admin)* | Gov | `Issues → [A] admin` | 0.4 | R-03 G8 · R-02 §1.1 · R-21 §2d · §4.2 job-link |
| **I-12** | Team page (team-scoped work, health) | PM | `Issues → Team` | 0.4 | R-03 M4 · R-21 §2d |

### 1.4 Knowledge platform — [`knowledge.md`](./knowledge.md) (§7.4)

| ID | Surface | Audience | IA placement | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **K-1** | **The block editor** (page) | PM/Eng | `Knowledge → <space> → <page>` | 0.4 | R-03 M2/E5 · R-10 §3 · R-21 §2e |
| **K-2** | Database views *(D2 — same §5.6 component as Issues)* | Eng/PM | `Knowledge → <space> → <db>` | 0.5 | R-03 D2 · R-10 §2 · R-16 D2 · R-21 §2e |
| **K-3** | Navigation / sidebar tree (spaces→pages) | PM/Eng | `Knowledge → <space>` (sidebar) | 0.3 | R-06 §3.2 · R-21 §2e |
| **K-4** | Backlinks & references panel *(wedge)* | PM/Eng | `Knowledge → <page> → Backlinks` | 0.3 | R-22 **W5** · R-04 F-ENG-2 · R-21 §2e |
| **K-5** | Page history UI (version, diff, restore) | PM/Eng | `Knowledge → <page> → History` | 0.4 | R-21 §2e · §7.4 |
| **K-6** | **Templates UI** *(admin-ish)* | PM | `Knowledge → <space> → Templates` | 0.3 | R-20 (template start) · R-21 §2e · §4.2 job-link |
| **K-7** | Sharing & permissions UI | PM/Gov | `Knowledge → <page> → Sharing` | 0.4 | R-02 §2.2 (no-leak) · R-21 §2e |
| **K-8** | Export UI (Markdown/open formats) | PM/Gov | `Knowledge → <space> → Export` | 0.3 | R-03 G9 · R-21 §2g · §4.2 job-link |
| **K-9** | Search palette (knowledge-scoped + cross) | PM/Eng | `[G] Search` (scoped) | 0.3 | R-08 · R-21 §2g |

### 1.5 Chat — [`chat.md`](./chat.md) (§7.5)

| ID | Surface | Audience | IA placement | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **H-1** | Channel / conversation list (sidebar) | all | `Chat` (sidebar) | 0.3 | R-06 §3.2 · R-21 §2f |
| **H-2** | **Message timeline view** | all | `Chat → <channel>` | **0.7** | R-04 F-PM-1 · R-07 §2 (chat 0.7) · R-21 §2f |
| **H-3** | Composer (rich, slash, @-mention, paste-unfurl) | all | `Chat → <channel>` (composer) | 0.5 | R-10 §3 · R-09 · R-21 §2f |
| **H-4** | Thread pane (where agent/incident detail lives) | all | `Chat → <channel> → <thread>` | 0.6 | R-15 §5.1 · R-06 §3.4 · R-21 §2f |
| **H-5** | Unfurl cards (live, permission-aware, inline-act) | all | (in H-2/H-3) | 0.05 | R-09 §3 · R-22 **W2** · R-21 §2a |
| **H-6** | Mentions / "Activity" inbox | all | `[G] Inbox` | 0.2 | R-03 M5 · R-10 §4 · R-21 §2f |
| **H-7** | Search view (messages + artifact-scoped) | all | `[G] Search` (scoped) | 0.3 | R-08 · R-21 §2g |
| **H-8** | **Incident / "canvas" view** `[UNCERTAIN/DEFER]` | PM/Eng | `Chat → <channel>` (pinned) | 0.6 | R-04 F-PM-1 · §4.3 below (resolution) · R-21 §2f |
| **H-9** | **HITL approval-card surface** *(agent flagship)* | cross | `Chat → <channel>` (card) + `[G] Inbox` | 0.3 | R-04 F-AGT-1 · R-14 §2/§3 · R-22 **W6** · R-21 §2f |

### 1.6 Shared, identity, admin, GDPR & sovereignty — [`shared-admin-sovereignty.md`](./shared-admin-sovereignty.md) (§7.6)

| ID | Surface | Audience | IA placement | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **S-1** | **The shell** (rail · sidebar · content · context pane) | all | the frame (R-06 §3) | n/a (frame) | R-06 §3 · R-07 §3 · design-language §5.1 |
| **S-2** | **Command palette** (`⌘K`) *(wedge)* | all | `[G]` (every screen) | **0.1** | R-08 · R-22 (palette) · R-17 §5.6 · R-21 §2a |
| **S-3** | Global / cross-artifact search view | all | `[G] Search` | 0.3 | R-08 · R-21 §2g |
| **S-4** | Unified notifications **inbox** ("what needs *me*") | all | `[G] Inbox` | **0.2** | R-10 §4 · R-13 §B.5 · R-22 W3 · R-21 §2g |
| **S-5** | Reference chip / unfurl *(the wedge component)* | all | (everywhere) | **0.05** | R-09 · R-22 W1/W2/W5 · R-21 §2a |
| **S-6** | Identity & scope selector / profile / prefs | all | top bar + `[A] Identity` | 0.0 / 0.4 | R-06 §4.4 · R-19 §1.1 · R-21 §2g |
| **S-7** | Org / team / project / space admin (SSO/SCIM) | Gov | `[A] Org admin` | 0.4 | R-03 G8 · R-20 enterprise arc · R-21 §2g |
| **S-8** | Permission / role management (RBAC over ReBAC) | Gov | `[A] RBAC` | 0.4 | R-03 G2 · R-02 §2.2 · R-19 §1.2 · R-21 §2g |
| **S-9** | **Agent governance console + kill-switch** | Gov | `[A] Agents` | 0.4 | R-03 G4 · R-14 §6.5 · R-15 §4 · R-02 §2.3 · R-21 §2g |
| **S-10** | Audit-log explorer | Gov | `[A] Audit` | 0.4 | R-03 G3 · R-15 §2 · R-21 §2g |
| **S-11** | **GDPR / data-rights (DSR) console** *(governance flagship)* | Gov | `[A] GDPR` | **0.4** | R-04 F-GOV-1 (G5/G6) · R-19 §2/§3 · R-21 §2g |
| **S-12** | Data-map / RoPA & residency console | Gov | `[A] Data-map` | 0.4 | R-03 G7 · R-19 §5.1 · R-21 §2g |
| **S-13** | Tenant / cell & residency settings | Gov | `[A] Tenant/cell` | 0.4 | R-19 §1.1 · R-20 A1 · R-21 §2g |
| **S-14** | Onboarding & empty-platform flows | all/Gov | `[A] Onboarding` | 0.3 | R-20 (3 archetypes) · R-21 §2g · design-language §5.10 |
| **S-15** | **Billing / usage / export & exit** *(admin)* | Gov | `[A] Billing` | 0.4 | R-03 G1/G9 · R-21 §2g · §4.2 job-link |

### 1.7 CLI — [`cli.md`](./cli.md) (§7.7)

| ID | Surface | Audience | IA placement | Density (R-07) | Research behind it |
|---|---|---|---|---|---|
| **X-1** | The CLI as a peer surface (same tree, textual render) | Eng | the §2 tree, textual | textual-density tier (R-07 J2) | R-06 §7.7/§8 · R-09 §7.1 · R-21 §2g (CLI row) · R-03 E1/E3/E7 |

**Orphan check (PROVEN against [`_completeness-critic §3`](../04-research/_completeness-critic.md)):**
**71 primary surfaces mapped (G:11 · C:9 · I:12 · K:9 · H:9 · S:15 · X:1, plus the shell S-1), 0
orphans.** Every row resolves to at least one of the three reachability paths — an R-04 flow, an
R-08/R-09/R-10 pattern spec, or an [`R-21 §2`](../04-research/craft/state-craft.md) per-surface state
matrix row (the exhaustive backstop). The four surfaces the critic flagged as *flow-orphaned* but
state-placed (G-10 branch-protection, C-5 pipeline editor, I-11 workflow/SLA admin, K-6 templates,
S-15 billing/export-&-exit) each get an explicit **job link** in §4.2 so Phase 6 does not sketch them
as decoration.

---

## 2. The per-surface map template (the schema each surface entry uses)

Every surface entry in the per-group files is a **tight map of pointers**, not a spec, using this
consistent 11-item schema. (The corpus owns the depth; this map only points + positions.)

1. **Audience(s) + the JTBD job(s) it finishes** — cite R-03 IDs (E#/M#/G#) and the R-04 flow (F-#)
   where it appears.
2. **IA placement + how it composes into the one shell** — the R-06 §2 tree node + which shell region
   (rail / contextual sidebar / content / context pane, R-06 §3) it fills.
3. **Shared components it is built from** — chip/unfurl (R-09), palette (R-08), views (R-10 §2),
   editor (R-10 §3), inbox (R-10 §4), overlays (R-10 §5) — the §5 building blocks it composes.
4. **Density position + persona-adaptive lenses** — the R-07 Axis-3 number + (if dual-audience) the
   R-16 lens bundle (projection / density / vocabulary / fields / landing).
5. **Agent touchpoints** — the R-14 treatment / plan-then-apply card / HITL states it hosts;
   R-15 attribution + audit affordances where relevant.
6. **Sovereignty / GDPR cues** — the R-19 residency cue tier (T0–T3), visibility chip, or console
   role, where the surface carries data.
7. **The state set it must implement** — cite the [`R-21 §2`](../04-research/craft/state-craft.md)
   per-surface matrix row (which of the 14 states are `●` required, `◐` context, `○` N/A, and which
   it **owns**).
8. **A11y + i18n/RTL obligations** — the R-17 hard-component checklist row (keyboard + SR) and the
   R-18 §7.2 i18n/RTL obligation (expansion / non-Latin / mirror / locale-format / humanised).
9. **Device / form-factor behaviour (incl. touch/mobile)** — grounded in design-language §8b.4: the
   hover-not-touch-reachable fix, `width:100%` panel clipping, popover flip, the mobile drawer pattern,
   named fixed-width assumptions. **Where a surface is desktop-mainly, say so + specify its mobile
   *read* behaviour.** *(Critic fix #1 — see §4.1.)*
10. **Wedge / delight moments + motion notes** — the R-22 wedge ID (W1–W7) it hosts + the R-12
    motion roles (e.g. `motion.liveUpdate`, `motion.settle`, the reserved agent signature).
11. **Per-surface definition-of-done incl. the switch test** — the concrete done-bar (states present,
    gates demonstrable) **+** the design-language §8b.7 switch test: *would a team move to this surface
    without hitting a wall the old tool didn't have?*

---

## 3. Cross-cutting obligations every surface inherits

These are **not** restated per surface — they are the floor every entry sits on. A finalist that
breaks any of them has regressed regardless of how the individual surface looks.

### 3.1 The one shell (R-06 §3 / R-07 §3 / design-language §5.1)
Every surface composes into **one skeleton**: top bar (scope selector · `⌘K` · search · Inbox ·
identity/agent badge · residency cue) + primary rail (`Code · CI · Issues · Knowledge · Chat` +
`Home · Inbox · Search` + `[A] Admin`) + contextual sidebar + content + collapsible context pane.
A surface owns **only** its sidebar + content; the rail, top bar, palette, inbox, identity badge, and
context pane are **shell-owned and identical everywhere**. The **eight invariants** that never vary
([`R-07 §3`](../04-research/ia/unification-study.md#3)): one shell · one identity/scope · one chip/unfurl ·
one palette+search · one inbox · one editor/views component · one token system · one agent treatment.
**The D4 reviewer test made concrete:** open the same chip, palette, inbox, and identity badge in the
diff (Axis-3 `0.85`) and the roadmap (Axis-3 `0.5`) — if they are the identical component, distinctness
was earned; if either is a bespoke clone, the product forked. *(PROVEN structure; HOUSE-STYLE synthesis.)*

### 3.2 The hard gates G1 / G2 (R-17 / R-18 — PROVEN, binding on every required screen)
- **G1 accessibility (WCAG 2.1 AA / EN 301 549 floor; 2.2 AA house target).** Every surface inherits
  the [`R-17`](../04-research/accessibility/audit-method.md) master checklist M1–M10: contrast
  **measured not claimed** (focus token ≠ identity token, AA-derived); visible focus in every theme;
  full keyboard operability + no traps; **status never by colour alone** (glyph + label + position);
  semantic roles/landmarks; live regions that announce without spamming; 200%/320px reflow;
  reduced-motion first-class; no-access never leaks to AT. The **seven hard components** (diff, board
  drag, views inline-edit, block editor, HITL card, command palette, nested overlays) each carry a
  keyboard + screen-reader row.
- **G2 i18n/RTL (PROVEN — requirement for an EU-sovereign product).** Every surface inherits the
  [`R-18 §7`](../04-research/accessibility/i18n-rtl-patterns.md) demonstration set: text expansion
  (German +30–40%, no truncation/clipping) · non-Latin (Greek/Cyrillic, self-hosted font, diacritic
  headroom) · **RTL via logical properties** (whole shell + editor + views + overlays mirror; real
  Arabic/Hebrew + a mixed-direction LTR run bidi-isolated) · locale-aware dates/numbers (`Intl.*`,
  load-bearing on SLA/deadline surfaces) · no machine strings (humanised at the backend, §8b.5).

### 3.3 The state set (R-21 — the 14 unglamorous states, per-surface matrix)
Every surface implements its [`R-21 §2`](../04-research/craft/state-craft.md) matrix row from the 14
states: **empty** (3 kinds: first-use / cleared / filtered) · **loading** (structure-skeleton, never
spinner) · **error** (blames system, one quiet line + path) · **permission-denied** (Restricted vs
Absent, no leak) · **erased/tombstoned** (`sub_gone`/`root_gone`/`erased`) · **agent-pending** ·
**degraded-surface** (fails static) · **stale/offline/reconnecting** · **optimistic-rollback** ·
**conflict** (CAS→CRDT) · **moved/outdated** · **cross-cell** · **storm/30×-surge** · **no-results**.
The "owns" cells: inbox owns storm; diff owns rebase-orphan; the collab editor owns conflict +
reconnect; live log + chat timeline own stream-drop/resume; the DSR console owns erasure outcome;
chip/unfurl own no-access/tombstone/moved/cross-cell.

### 3.4 Perceived-performance budgets (R-13 §A — PROVEN thresholds)
B1 keyboard response **<~100ms** · B2 optimistic action paints **<~100ms**, ack async · B3 suppress
flash-of-spinner **<~1s**, show **structure-skeleton** never blank · B4 **pages render, they don't
animate in** · B5 background live-update **noticed without interrupting** (in place, no scroll-jump) ·
B6 a degraded surface **fails static** (shell + other surfaces stay live). The three-state optimistic
contract (pending → settled → **rolled-back-visibly**, OPT-1: optimism never hides failure). Prefetch
patterns CA-1…4 warm the next hop (failing-check→line, PR-pane, notification next-hop, hover-peek).
**Residency caveat (PROVEN, ADR-11):** perceived speed is bought with optimistic UI + in-region edge +
prefetch, **never global replication** of personal data.

### 3.5 Motion (R-12 — functional, fast, interruptible, never decoration)
Five laws: L1 motion communicates state-change or doesn't ship · L2 fast + interruptible (120–200ms) ·
L3 **pages render, they don't animate in** · L4 reduced-motion first-class (every motion has a
no-movement spelling) · L5 one motion = one meaning. Semantic roles: `motion.feedback`/`settle`/`move`/
`enter`/`exit`/`liveUpdate` + the **reserved agent signature** (`motion.agentEnter`/`agentResolve`).
**Anti-list (binding):** no AI sparkle/shimmer/glow, no spring/bounce, no spinners-on-blank, no
confetti, no motion-as-only-signal.

### 3.6 Agent legibility (R-14 / R-15 — the §6 plan-then-apply / HITL contract)
Wherever an agent appears, the **four-channel treatment** (label + icon + reserved `agent` colour +
attribution string; **never colour-alone, never sparkle**); **plan-then-apply** (proposed effects
shown before they happen, each with target `ArtifactRef`, authority + gate-marker, scope, budget);
the **HITL card** with **Approve / Edit / Reject** (Edit re-runs the full pipeline within scope);
the 10 agent states (agent-pending → working → gate-awaiting → {approved/edited/rejected} + the
failure set: agent-error/budget-exceeded/loop-guard/denied/stale-approval); five-field provenance
(Who/What/On-behalf-of/Trigger/Correlation) with an inline **"Why?"** + audit-trail link; **calm
agent volume** (agent output routed out of the main timeline; one `correlation_id` threads a chain).
**Frozen consequential defaults (PROVEN):** merge/deploy/erase **gated**; triage/label suggest-no-gate;
suggest-by-default never loosened to autonomous without written deviation.

### 3.7 Sovereignty (R-19 — legible first-class, not fine print)
Where a surface carries data: the **residency cue ladder** (T0 ambient region token in the scope
indicator, always-on · T1 on-hover detail · T2 cross-boundary warning · T3 cross-cell provenance on a
chip) and the **per-artifact visibility chip** (`Private`/`Team`/`Org`/`Public`, privacy-by-default
**Private**, click → effective-access view). **All R-19 UX choices are `[UNDER-EVIDENCED]`** — the
legal/architectural floor is PROVEN, the legibility is HOUSE STYLE pending the deferred regulated-buyer
review.

---

## 4. The critic-seam resolutions (the four [`_completeness-critic.md`](../04-research/_completeness-critic.md) fixes)

### 4.1 Critic fix #1 — touch / mobile form-factor (the one uncovered gloss-risk; PROVEN-gap)
**Resolution: cover it as the per-surface template item 9**, grounded in
[`design-language §8b.4`](../../planning/02-holistic-architecture/design-language.md) (the PROVEN bug
set R-06 §3.5 carries for the shell). The map adds the device dimension to **every** surface; the
shared **mobile law set** (HOUSE STYLE application of the §8b.4 PROVEN bugs):

- **MOB-1 — hover is not touch-reachable.** Any action that lives on hover (issue-list row actions,
  chat message hover-actions, knowledge backlink peek, diff line-comment affordance, view-cell
  overflow) must be **default-visible or behind an explicit mobile affordance**. *(Affects G-7, G-8,
  H-2, I-1/I-5, K-4 most.)*
- **MOB-2 — `width:100%` is not a takeover.** A full-width mobile panel beside a still-present column
  is clipped off-screen; **collapse the other column at the breakpoint.** The contextual sidebar and
  context pane become **mobile drawers** (toggle + backdrop + Escape + route-change auto-close,
  R-06 §3.5).
- **MOB-3 — flip popovers when off-screen.** Test against the **real anchor** — a picker/unfurl under
  a bottom-pinned composer (chat, H-3) renders off-screen and must flip-above + cap height.
- **MOB-4 — pin the shell, each region scrolls itself** (`100vh`/`overflow:hidden`; a scrolling flex
  child needs `min-height:0` + overscroll-contain) so the composer never drops below the fold.
- **MOB-5 — name fixed-width assumptions before going responsive** (SLA timers, badges, the diff
  gutter, fixed-width buttons; compounds with G2 text expansion, R-18 §2.3).
- **MOB-6 — desktop-mainly surfaces declare a *read* behaviour.** The **diff (G-7)**, the **CI matrix
  (C-4)**, the **pipeline editor (C-5)**, the **DSR/RoPA consoles (S-11/S-12)**, and the **dashboards
  (I-9)** are **desktop-primary**: on mobile they are **read/triage**, not authoring surfaces. The
  diff renders **unified-only on narrow** (no side-by-side), with line-comment **read** but
  authoring deferred to a tap-to-expand sheet; the consoles render the **inventory/list read view**
  and defer destructive actions (erase, kill-switch) to desktop with a clear "open on a larger screen
  to act" affordance. *(HOUSE STYLE; the §8b.4 bugs are PROVEN.)*

**Carried flag:** this is a **reasoned coverage**, not user-validated; native-app scope stays
`[OPEN → P4]` (design-language §9). Phase 6 must *demonstrate* MOB-1…MOB-6 on at least the shell + one
dense surface (the diff or board) — it is now in scope, not silently dropped.

### 4.2 Critic fix #2 — flow-orphaned admin surfaces (give each an explicit job link)
The five admin surfaces that have an IA placement + R-21 states but **no R-04 narrative of use** —
so Phase 6 won't skip them or sketch them as decoration. Each gets a one-line **job link**
(R-03 job + persona):

| Surface | Job link (R-03 job · persona · the moment it's used) |
|---|---|
| **G-10 Branch-protection / ruleset editor** | Implied by **E12** (P5/P3): "accept outside contributions without leaking secrets" needs *required-reviewers / status-gates / fork-CI rules*; **G2** (P12/P15): least-privilege on protected refs. Used when a maintainer hardens `main` before opening a repo to contributors. |
| **C-5 Pipeline / definition editor + validator** | **E8** (P3/P15): "declare 'when X do Y' automation as a first-class, observable, *validated* trigger so I stop maintaining webhook glue that breaks silently." Used when a platform engineer authors/edits the paved-road pipeline; the schema-validator is the anti-YAML-sprawl differentiator. |
| **I-11 Workflow / SLA / field-scheme admin** | **G8** (P15): "one admin surface for … policy"; **M4** (P7): trustworthy SLA/flow needs the SLA + workflow defined. Used when an admin configures a team's workflow/SLA — *progressive disclosure, one layer down* (R-02 R-CFG: never imposed on the newcomer). |
| **K-6 Templates UI** | **M2** (P6/P10): "have my spec *be* linked …"; **R-20 startup/scale-up arcs**: "new from template" is the onboarding-forward empty-state CTA (rung 0 / P0). Used at first-doc creation and when a team standardises PRDs/runbooks. |
| **S-15 Billing / usage / export & exit** | **G9** (P14/P13/P15): "clean DPA, transparent sub-processor list, strong data-portability/exit so I avoid lock-in and control TCO"; **G1** (P11): one bill. Used by procurement/admin at evaluation and at contract exit — the anti-lock-in promise made operable. |

*(Also note **K-8 Export UI** shares the G9 anti-lock-in job; it is flow-touched lightly via R-03 G9
but is grouped here for the same reason — portability is a buyer-decisive surface Phase 6 must not skip.)*

### 4.3 Critic fix #3 — the chat-threading model (resolve explicitly)
**Recommendation (HOUSE STYLE, with a clear rationale): adopt Zulip-style *topics-within-channels* as
the threading model, NOT Slack-style flat-channel-with-opt-in-threads.** Rationale:

- **Agent volume is the deciding factor.** Myelin is agent-native; F-AGT-1 + R-15 §5 establish that
  agents generate review comments, triage updates, status posts, and chain chatter at machine speed —
  the exact volume that *pollutes a flat channel* (R-01 §3.3 Slack trap) and that *mandatory topics
  keep legible by construction* (Zulip). P8 (calm) + R-13 §B.4 (agent volume out of the main timeline)
  make topic-per-chain the structurally calmer default: **a topic = a `correlation_id` chain** (R-15
  §5.1 already recommends Zulip-style topics per chain). This is the single biggest reason.
- **It composes with the IA already.** R-06 §2 normalises **thread** as a first-class navigable L2
  node (`Chat → <channel> → <thread>`) and makes it `ArtifactRef`-able (`#thread-<id>`) — a Zulip
  topic *is* that node; Slack's ephemeral thread is a weaker fit for an addressable artifact graph.
- **The cost (carried honestly):** topics impose more up-front structure than Slack's zero-friction
  posting — a real approachability tax for the PM/casual audience (P6). **Mitigation (HOUSE STYLE):** a
  channel may offer a **default "general" topic** so posting never blocks on naming, and topic choice
  is a calm, optional affordance; the rigour is *earned where volume is high* (incident channels,
  agent-active channels), relaxed where it isn't. This keeps R-01's "discoverability-cliff" and P6
  approachability honoured.
- **`[DEFERRED-UNTIL-USERS]`:** whether the topic tax is acceptable to the PM/casual segment is the
  open question — it rides R-07's per-segment tree-test (does the PM segment co-locate a thread the
  way the engineer does?) and R-16's both-audience validation. **Carried, not closed.**

**The `[UNCERTAIN/DEFER]` incident / "canvas" view (H-8): settled as *adopt a thin version, defer the
heavy one*.** Recommendation: **adopt the incident "pinned structured summary atop an incident channel"
(the lightweight canvas)** — it is directly demanded by **F-PM-1 (E9: run an incident as one linked
timeline)** and **F-AGT-1** (the agent posts a summary), and it is cheaply expressible as a pinned
unfurl/summary block over the existing thread + Knowledge embed (R-10 §3 embeds; R-09 unfurl). **Defer
the *full* Notion-class collaborative "canvas"/whiteboard authoring surface** — that is the designer
"canvas" scope still `[OPEN → P4]` in design-language §9 (P9 designer-depth). **What decides the
deferral:** the P4 product-scope decision on native design/canvas authoring vs referencing external
tools (Figma) — same gate as the §9 open question. So Phase 6 *should* sketch the **incident pinned
summary** (it's job-backed); it should **not** invent a freeform canvas.

### 4.4 Critic fix #4 — vocabulary-fracturing (carry as an unvalidated HOUSE-STYLE bet)
**This map does NOT present persona-adaptive vocabulary as settled.** R-06/R-07/R-16 *agree on the
bound* but the *decisive card-sort is deferred to users* (R-07 Part 2 falsification #2 — "the single
most important result"). The map carries the **three-tier bound** as the explicit unvalidated bet:

- **T1 — schema / `ArtifactRef` / API / CLI / URL `<type>` / icon: FROZEN** (one canonical term; this
  is the line that keeps a chip/audit entry/search resolving identically regardless of lens). **PROVEN
  necessity** (R-06 §6.3 bounding rule).
- **T2 — lens label (per role/space): bounded curated synonyms** (`issue` ↔ `work item` ↔
  `deliverable`; `project` ↔ `initiative` ↔ `programme`; `roadmap` ↔ `portfolio`). **HOUSE-STYLE BET,
  unvalidated.**
- **T3 — free per-tenant rename: discouraged, audited opt-in** (bounded to a mapped synonym set, never
  arbitrary strings).

**Binding instruction to Phase 6:** treat T2 as a HOUSE-STYLE bet a finalist *demonstrates* (both
lenses sketched over the same data, R-16 §6.5), **never as validated**; keep labels **config/token-held**
so a failed card-sort is applied by re-mapping, not re-coding. **The falsifier (carried):** if the
two-label card-sort shows a PM and an engineer believe they are looking at *different objects* under
their own labels, the bet is broken and the §2 dual-product split returns. This is **the single most
important contradiction Phase 5 carries forward** (per the critic's "bottom line").

---

## 5. The highest-leverage surfaces for the sketch funnel (the recommended comparable screen set)

[`sketch-funnel.md §Part 4`](../02-research-roadmap/sketch-funnel.md) requires every 6c finalist to
include the comparable screen set (the shell + ≥1 dense engineer + ≥1 approachable PM/corporate + ≥1
agent/HITL + 1 wedge). Below is the **specific recommended target** — the exact surfaces that maximise
rubric coverage and decision-relevance, with rationale. Phase 6 should populate the funnel with these.

| Funnel slot | **Recommended surface** | Why this one (rubric / corpus rationale) |
|---|---|---|
| **The shell** | **S-1 the shell** with the **G-6 PR overview / context pane** as its content | The shell is the central-problem embodiment (D4); pairing it with the PR context-pane shows the **wedge (W1)** assembling itself in the one frame — the single screen that most proves "one product." Tests D4 + the eight invariants + the residency cue (Axis 6). |
| **Dense engineer surface** | **G-7 the diff** (split + inline-comment), at compact tier | The **highest-distinctness surface (Axis-3 `0.85`)** and the hardest D4 test (does it still feel like the same product while dense? R-07 §2.1). Also the hardest **G1** case (R-17 §5.1 diff checklist) and a strong **G2** case (RTL diff, mixed LTR code in RTL prose). Carries **W4** (check→line→fix) and the rebase-orphan state (R-21 owns). High D1/D7. |
| **Approachable PM/corporate surface** | **I-3 the roadmap** *paired with* **I-2 the board** (same §5.6 component, two lenses) | This **is** the D5 dual-audience proof and the funnel's binding Axis-3 spread point (R-07 §2.1, the board↔roadmap pair). Sketching both lenses over the **same issue records** is the "neither lens a degraded compromise" test (D5) and the vocabulary T2 bet demonstration (§4.4). Approachable + warm (D2). |
| **Agent / HITL moment** | **H-9 the HITL approval card** in chat (Approve/Edit/Reject) | The D6 flagship: plan-then-apply + the four-channel agent treatment + attribution + the gate states (R-14 §2/§3/§5). In chat (W6, one `correlation_id`) where F-AGT-1 puts it. Tests D6 + the agent-never-colour-alone G1 rule. |
| **Wedge moment** | **S-2 the command palette** (`⌘K`) **or** a live **H-5 unfurl with inline action** (W2) | The palette is the cross-artifact thesis as one keyboard surface (D1, Axis 2); the live unfurl-with-inline-action is the wedge made tangible at its most demonstrable (R-22 W2, R-09 §3). Either satisfies the slot; the palette also doubles as the Axis-2 navigation evidence. |

**Additional spread guidance (so the funnel covers the edges, per sketch-funnel §Part 1):**
- For **Axis 6 (sovereignty)**, at least one finalist should render the **S-11 DSR console** (or the
  S-12 residency console) as its PM/corporate surface instead of the roadmap — it is the D9 flagship
  and the regulated-buyer-decisive surface, and the funnel currently doesn't force it.
- The three R-11 visual directions (**A Instrument / B Civic / C Workshop**) map cleanly onto the
  Axis-1/Axis-4 spread and onto the three audiences — a finalist anchored in each is a natural way to
  achieve the binding Axis-3 + one-other-axis spread.

---

## 6. Completeness-critic mini-pass — what THIS map glossed

Honest about its own gloss (the discipline the corpus held; this map holds it too):

1. **Touch/mobile is *covered as a reasoned design law set* (§4.1), not user-tested.** MOB-1…MOB-6 are
   a HOUSE-STYLE application of PROVEN §8b.4 bugs; the diff/console *read-on-mobile* behaviour is a
   judgement call. Native-app scope stays `[OPEN → P4]`. **Risk:** a real mobile usability pass may
   reshape which surfaces are "read-only" — this map asserts the line, it doesn't prove it.
2. **The Axis-3 density numbers are carried from R-07 as *illustrative spacing*, not measured.** The
   *ordering* is well-grounded (diff/log/chat high; chip/palette/inbox/identity low; views-component
   mid); the specific values (`0.85` vs `0.8`) are spread-calibration for Phase-6, not precision.
3. **Surface *count* is a granularity choice.** "71 surfaces" depends on where a view ends and a state
   begins (e.g. list/table/calendar collapsed into I-5; the diff's split/unified are one surface). A
   finer cut would inflate the count; the orphan claim holds at any reasonable granularity because the
   R-21 §2 matrix is the exhaustive backstop.
4. **This map does not re-rank jobs or surfaces.** The R-03 ODI importance×satisfaction ranking is
   `[DEFERRED-UNTIL-USERS]`; the funnel recommendations in §5 lean on the corpus's *structural*
   leverage (wedge moments, dual-audience pairs, hard-gate stress), **not** on a validated
   "top surface" claim — which would be the exact taste-as-settled failure the honesty rule forbids.
5. **Two seams the critic raised are routed, not resolved here.** The R-10-silent **non-diff anchored
   comment relocation** (critic §4.3) is flagged in [`knowledge.md`](./knowledge.md) / [`issues.md`](./issues.md)
   as a question for Phase 6 (is anchored-comment relocation diff-only, or a general content-anchor the
   editor/views also owe?); and the **numeric AA token values** (critic §4.4) are correctly left to
   per-finalist DTCG token sets (rubric §5). Both are expected handoffs, named so they aren't forgotten.
6. **The map asserts a chat-threading *recommendation*, which is a real bet.** §4.3 recommends Zulip
   topics; the approachability tax on the PM segment is genuinely unvalidated (rides R-07/R-16). If
   the human disagrees, it is a single, cleanly-scoped reversal — flagged so.

---

*End of Phase 5 README. Date: 2026-06-20. Map structure HOUSE STYLE over the PROVEN corpus
(design-language §7 catalogue; R-06 IA; R-07 unification ruling; R-03/R-04 jobs+flows; R-08…R-22
components/craft/a11y/agent/sovereignty). Not user-validated — the eight `[DEFERRED-UNTIL-USERS]`
flags are carried, not substituted. Feeds Phase 6 (sketch funnel) and the final human decision.
Do not commit (orchestrator handles git).*
