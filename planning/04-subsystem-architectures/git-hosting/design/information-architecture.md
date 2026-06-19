# Git hosting — Information Architecture

> Phase-4 design sketch (precedes the architecture stage, VISION §3/§5.4). Fits the ONE-SHELL design
> language: primary rail + contextual sidebar + header + optional right-hand context pane
> (design-language §5.1). Maps the §7.1 view catalogue into a navigable structure. Every screen
> inherits the shared components (§5), tokens (§3), a11y (§4), agent surfaces (§6), and the
> empty/loading/error/permission/erased states (§5.10). Date: 2026-06-19.

## 1. Where git hosting sits in the shell

The persistent navigation shell (design-language §5.1) is platform-wide; git hosting is the **"Code"**
area on the primary rail:

```
┌──────┬──────────────────────────────────────────────────────────────────────────────┐
│ RAIL │  HEADER:  [tenant/org ▾]  [repo-scope ▾]  ⌘K search   inbox●   identity ▾      │
│      ├──────────────────────────────────────────────────────────────────────────────┤
│ Code◀│  CONTEXTUAL SIDEBAR        │  MAIN CONTENT AREA              │  CONTEXT PANE    │
│ CI   │  (repo tree / PR list /    │  (the active view)             │  (cross-artifact │
│ Issue│   branch list / settings)  │                                │   refs, live)    │
│ Know │                            │                                │   — collapsible  │
│ Chat │                            │                                │                  │
│ Inbox│                            │                                │                  │
│ ─────│                            │                                │                  │
│ ⌘K   │                            │                                │                  │
└──────┴────────────────────────────┴────────────────────────────────┴──────────────────┘
```

- **Rail** (platform-owned): Code · CI · Issues · Knowledge · Chat · Inbox · Search. Switching areas
  never feels like switching apps (P1). The **agent presence indicator** and identity menu live here/header.
- **Header** (platform-owned): the org/tenant switcher (doubles as a **residency/visibility cue**, P9),
  the **repo scope selector**, the command palette trigger (⌘K), global search, the notifications inbox
  entry, the current `Principal` menu.
- **Contextual sidebar** (git-hosting-owned): changes by sub-area — the repo file tree, the PR list, the
  branch/tag list, or the settings nav.
- **Context pane** (git-hosting-composes, Refs/projection-driven): the right-hand pane where
  cross-artifact references surface — the **PR context pane** is the flagship (the wedge made concrete).

## 2. The navigation tree (git hosting's surface)

```
Code (area root)
│
├─ Repository list / picker            [repo scope selector in header + a list view]
│
└─ Repository  «owner/name»  (the repo is the primary unit; tabs across the top of MAIN)
   │  sidebar = file tree (Code tab) | PR list (Pull requests) | branch list (Branches)
   │
   ├─ Code            ▸ Repo home (README, branch/tag switcher, language/size, quick actions)
   │                  ▸ File tree & file view (syntax, blame w/ ignore-rev, raw, LFS-aware, permalink-by-SHA)
   │                  ▸ History / commit list (per-path) → Commit detail (diff, parents, verified-sig)
   │                  ▸ Compare view (arbitrary ref/SHA ↔ ref/SHA)
   │                  ▸ Code search (path/symbol/literal/trigram — permission-pre-filtered)
   │
   ├─ Pull requests   ▸ PR list (filter: open/draft/merged/closed, reviewer, author, agent-authored)
   │                  └─ PR detail  (tabs: Overview · Files changed · Commits · Checks)
   │                        Overview      = description + linked issues/docs/runs (CONTEXT PANE) +
   │                                        participants + required-checks summary + merge-readiness +
   │                                        timeline + AGENT-AWARE review surface
   │                        Files changed = diff (unified/split) + inline + batched review comments
   │                        Commits       = per-commit / "changes since you last reviewed"
   │                        Checks        = live CI status, required vs optional, re-run, log deep-link
   │
   ├─ Branches/Tags   ▸ Branch list (ahead/behind, protection badge, last-update) · Tag list
   │
   └─ Settings        ▸ General (visibility, default branch, merge methods, residency display)
                      ▸ Collaborators & teams (→ authz; compiled to ReBAC relations)
                      ▸ Branch protection / ruleset editor (ref patterns, approvals, checks,
                        signed/linear, force/delete bans, BYPASS lists, AGENT rules)
                      ▸ Event subscriptions (Myelin term for webhooks) / triggers
                      ▸ Keys & tokens (SSH/deploy keys — delegated to Identity)
                      ▸ Fork / network · Mirror config (residency-gated) · Archive/Transfer/Delete
                      ▸ Erasure / redaction admin (history-rewrite + crypto-shred — destructive, audited)
```

## 3. Deep-linking & ArtifactRef granularity (the wedge substrate)

Every node above is deep-linkable down to **sub-artifact granularity** (design-language §5.1 rule;
contract 5.7), because those links are what Chat/Issues/Docs reference:

- repo → `myelin://<t>/git/repo/<id>`
- file line → `myelin://<t>/git/blob/<repo>/<path>#L42` (permalink-by-SHA in the URL)
- commit → `myelin://<t>/git/commit/<repo>:<sha>`
- PR → `myelin://<t>/git/pr/<id>` ; PR comment → `…/git/pr/<id>#comment-<id>`

These render everywhere as the **reference chip / unfurl** (§5.3) — live, permission-aware per viewer,
tombstoning gracefully. The git surface is both the **densest producer** of these refs and a heavy
**consumer** (the PR context pane).

## 4. The context pane — cross-subsystem composition (no cross-DB)

The right-hand context pane on a PR is assembled by: Git checks viewer authz → asks **Refs** for the
PR's edges → Refs pre-filters targets via **Id.list_objects** → Git resolves each surviving
`ArtifactRef` via the **owning subsystem's projection API** (Issues/Knowledge/CI/Chat) → renders only
what the viewer may see, kept live by bus events (Phase-2 §6.3). A reference the viewer can't see
degrades to a **permission-stub**, never a leak (§5.3 hard rule).

## 5. Persona-adaptive density (P5 / §2)

- **Engineer (default):** dense PR/diff surfaces, keyboard-first (P3), the file tree and diff are the
  centre of gravity. Density is *earned* on the diff/review surfaces.
- **Reviewer:** the review surface (verdicts, CODEOWNERS "who must still approve", incremental review)
  is foregrounded; the agent-aware review surface is legible and distinct.
- **Admin/maintainer:** the ruleset editor and erasure/redaction admin are **progressive-disclosure**
  surfaces (P4) — simple defaults, power revealed on demand.

## 6. CLI as a co-equal "view" (one API, three consumers)

The Myelin CLI (`myelin repo|pr|branch|search|agent …`) is a thin client over the **same APIs** the UI
and agents use (Phase-2 §5; design-language §7.7 — the CLI is a first-class view). Plain `git` over the
wire (SSH/smart-HTTP v2) is the other surface and must just-work with stock clients. The CLI surface is
designed alongside the screens, not bolted on.

## 7. Mobile / responsive (DL §8b.4)

- The shell is **pinned to the viewport** (`100vh`/`overflow:hidden`); each region owns its scroller
  (`min-height:0` on scrolling flex children — the composer-below-the-fold bug class).
- On narrow widths the **sidebar and context pane collapse to toggled overlays** (backdrop + Escape +
  route-change auto-close — the mobile drawer pattern); `width:100%` is not a takeover (collapse the
  other column at the breakpoint).
- Diff view degrades to **unified-only** on narrow widths; hover-only row actions (PR list, comment
  actions) get an explicit mobile affordance (hover is not touch-reachable).

## Cross-references
- design-language §5.1 (shell), §5.3 (chip/unfurl), §7.1 (git view catalogue), §8b.4 (layout/mobile).
- `user-flows.md` (the flows over this IA), `wireframes.md` (the primary screens).
- Sketches 05 (authz/context pane), 06 (code search), 07 (diff anchoring), 09 (erasure admin).
