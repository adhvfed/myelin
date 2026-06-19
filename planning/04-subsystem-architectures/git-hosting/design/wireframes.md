# Git hosting — Wireframes (primary screens)

> Phase-4 design sketch. ASCII wireframes of the PRIMARY screens from the design-language §7.1
> catalogue, each showing **happy / empty / loading / error** (and permission/erased/agent-pending
> where they apply, §5.10). Applies the §8b day-one UX primitives: overlay/portal rules, ONE editor
> render path, measured tokens, layout-containment, humanised strings. Wireframes are structural, not
> visual — tokens/colours/values are §3 + the P4 design-system build. Date: 2026-06-19.

Shell legend (every screen sits inside it; design-language §5.1):
```
RAIL: Code▸ CI Issues Know Chat Inbox ⌘K     HEADER: [acme-eu ▾ 🇪🇺] [repo ▾]  ⌘K  inbox②  ◐me
```
Primitive rules applied throughout: overlays **portal to document root** with the one z-index scale
(§8b.1); loading shows **skeletons matching final layout**, never a blank spinner (§8b.3/§8b.6); errors
**blame the system in one quiet line + a path** (§8b.6); all strings **humanised at the backend**
(§8b.5 — no `merge_request merged`, no raw ids); status is **glyph+label+position, never colour alone**
(§8b.3).

---

## Screen A — Repository home

HAPPY:
```
┌ SIDEBAR (file tree) ─┬ MAIN ─────────────────────────────────────────┬ CONTEXT ──────┐
│ acme/app             │ acme / app   ⌖private 🇪🇺EU  [Clone ▾][Fork][⋆]│ Activity      │
│ ▾ src                │ ──────────────────────────────────────────────│ • main pushed │
│   ▸ auth             │ [main ▾]  142 branches · 37 tags   24 MB · Rust│   2m ago ◐ana │
│   app.rs             │ ┌ README.md ───────────────────────────────┐  │ • PR #88 open │
│ ▾ tests              │ │ # app                                    │  │ • tag v2.1    │
│   auth_test.rs       │ │ The Acme platform service…               │  │               │
│ Cargo.toml           │ │ ## Getting started …                     │  │ Languages     │
│ README.md            │ └──────────────────────────────────────────┘  │ Rust 91% …    │
└──────────────────────┴───────────────────────────────────────────────┴───────────────┘
```
EMPTY (no commits): MAIN shows an onboarding card — "This repository is empty." + the exact clone/push
commands (copy buttons) + "or create a file" — onboarding-forward (§5.10 empty; P1 startup persona).
LOADING: file-tree skeleton rows + a README skeleton block (structure, not a spinner; cold-repo
hydration, §7.1).
ERROR: one quiet line "We couldn't load this repository. — /repo/acme/app" + Retry; never a dead end.
PERMISSION: a private repo the viewer can't see resolves to the §5.3 "no access" card, never a leaked
name (the repo picker already `list_objects`-filters it out).
OVER-QUOTA: "This repository is over its residency storage quota" with the admin path (§7.1 error case).

---

## Screen B — File view (with blame)

HAPPY:
```
│ src/auth/sso.rs   [main ▾]   [Raw][Blame◀][History][⋆permalink-by-SHA]    LFS? n   1.2KB │
│ ┌────────────────────────────────────────────────────────────────────────────────────┐ │
│ │ 1  use crate::id;                              ◐ana  3mo  a1b2c3 "wire SSO"          │ │
│ │ 2  pub fn login(req: Req) -> Resp {            ◐ana  3mo  a1b2c3                      │ │
│ │ …  (syntax-highlighted; blame gutter w/ ignore-rev; click line# → permalink #L2)     │ │
│ └────────────────────────────────────────────────────────────────────────────────────┘ │
```
EMPTY: binary/image → inline preview; LFS pointer → "Stored with Git LFS (4.2 MB) — Download"; huge
file → "File too large to display — Raw / Download" (graceful degradation, §7.1).
LOADING: line-numbered skeleton lines (final layout), blame gutter greyed.
ERROR: "We couldn't render this file. — /blob/…" + Raw fallback (always offer raw).
ERASED: a file whose containing history was redacted → tombstone "This content was removed" (§5.10
erased), never a dangling render.

---

## Screen C — Pull request — Overview (the centrepiece + context pane + agent surface)

HAPPY:
```
│ PR #88  Add SSO login   ◐ana → main   ✔Open    [Reviewers ▾][⋆][Merge ▾]                 │
│ tabs: ‹Overview› Files(7) Commits(4) Checks(2)                          │ CONTEXT PANE   │
│ ┌ description (myelin-content render) ───────────────────────────────┐ │ Linked         │
│ │ Implements SSO. Closes #ISSUE-412.                                 │ │ ▸#ISSUE-412 ◔  │
│ └───────────────────────────────────────────────────────────────────┘ │  "SSO login"   │
│ ── Required checks ───────────────────────────────────────────────────│  ▸doc: Auth §3 │
│  ✔ ci/build   passed      ⟳ ci/test   running…                        │  ▸run #4412 ✔  │
│ ── Reviews ────────────────────────────────────────────────────────── │ (each per-     │
│  ◐sarah  approved ✔        🤖code-review  requested changes ⚠         │  viewer perm-  │
│   └ 🤖 AGENT · why: 2 findings · scope: comment-only · [audit↗][dismiss]│ filtered,live)│
│ ── Merge readiness ─────────────────────────────────────────────────  │                │
│  ⚠ Blocked: 1 check running · CODEOWNERS @sec must approve             │                │
│ ── Timeline ──────────────────────────────────────────────────────────│                │
│  ◐ana opened · 🤖 reviewed · ◐sarah approved …                         │                │
```
- The **agent review** row uses the **agent treatment** (distinct, labelled `🤖`, never human-disguised;
  no sparkle iconography §8b.3) with **why / scope / audit-link / dismiss** (Flow 3; P7/AI-Act §6.1).
- **Merge readiness** names *which* gate is unmet in humanised text (§8b.5), with the next action.
- Context-pane refs are **live, per-viewer permission-filtered** (§5.3 hard rules); an unseeable target
  → permission-stub.

EMPTY: a draft PR with no description → "Add a description" editor prompt; no linked artifacts →
context pane shows "No linked issues, docs, or runs yet."
LOADING: skeleton of the checks/reviews/timeline blocks; context pane shows ref-chip skeletons.
ERROR: per-block degradation — if the checks panel fails, it shows "Checks unavailable — retry" while
the rest renders (fail-static per surface, §8b.6), not a whole-page error.
AGENT-PENDING: if an agent action awaits HITL → an inline **approval card stub** (§5.4) "🤖 code-review
proposes 3 changes — review in Chat / Inbox" (the card itself lives in Chat/Inbox, Screen G).

---

## Screen D — Pull request — Files changed (diff + inline/batched review)

HAPPY:
```
│ Files changed (7)   [Unified|Split]  [Whitespace ▾]  [⊟collapse viewed]   Review ▶ (2) │
│ ▾ src/auth/sso.rs   +24 −3   ☐viewed                                                    │
│ ┌──────────────────────────────────────────────────────────────────────────────────┐  │
│ │ 41   pub fn login(req)                                                            │  │
│ │ 42 + let p = id::resolve(req)?;            ⊕ ◀ comment here                        │  │
│ │   └─ ◐sarah: "handle the None case?"  [Resolve][Reply]   ··· thread (Current)      │  │
│ │ 43 + …                                                                            │  │
│ └──────────────────────────────────────────────────────────────────────────────────┘  │
│ ▸ tests/auth_test.rs  +12   ☐viewed       ▸ Cargo.toml +1                              │
│                                          [ Review: 2 pending ▾  Approve|Request|Comment ]│
```
- Inline comments use the **shared comment/editor surface** (§5.5, ONE editor render path §8b.2 —
  markdown-subset string + structured mention/ref nodes); **batched review** (start → batch → submit
  verdict) is the default.
- **Comment thread states** (sketch 07): `Current` (anchored), **`Outdated`** (collapses to a badge +
  "show in original context"), `Resolved`. Suggestions are committable from the UI.
EMPTY: no changes → "No file changes"; all files viewed → "All 7 files reviewed ✔".
LOADING: large diff → **virtualized** skeleton hunks (large-diff handling, §7.1); per-file lazy.
ERROR: a file's diff fails → "Couldn't compute this diff — view raw blobs" inline, other files render.
OUTDATED (force-push): on `pr.synchronized`, threads remap (sketch 07); unrelocatable threads show the
**Outdated** badge — never mispointed, never lost.

---

## Screen E — Code search results

HAPPY:
```
│ ⌕ "resolve(" in acme/*        [type: code|path|symbol]  [repo ▾]  [lang ▾]   213 results │
│ ┌──────────────────────────────────────────────────────────────────────────────────┐  │
│ │ acme/app · src/auth/sso.rs:42   let p = id::resolve(req)?;        ◐ symbol: resolve │  │
│ │ acme/lib · src/id.rs:8          pub fn resolve(r: Req) -> …                          │  │
│ └──────────────────────────────────────────────────────────────────────────────────┘  │
```
- Results are **permission-pre-filtered** via `list_objects` (P9 — "you can only find what you may
  see"); facets by type/lang/repo (sketch 06).
EMPTY: "No matches in the repositories you can access." (honest about the ACL scope, not "no results").
LOADING: skeleton result rows.
ERROR: "Search is temporarily unavailable — retry" (fail-static surface).
RESTRICTED/HYOK: a note when a scope is excluded — "Some content is not searchable (restricted or
customer-keyed)." (storage §6.1 honesty requirement).

---

## Screen F — Branch protection / ruleset editor (admin, progressive disclosure)

HAPPY:
```
│ Settings › Branch protection                                   [+ New rule]              │
│ ▾ Rule: refs matching  main, release/*                                                   │
│   ☑ Require approvals  [2]   ☑ from CODEOWNERS   ☑ dismiss stale on new push             │
│   ☑ Require checks:  ci/build  ci/test   [+ add]                                         │
│   ☑ Require signed commits   ☑ Linear history   ☑ Block force-push   ☑ Block deletion   │
│   ☑ Agent rule: agent-authored PRs require human approval                               │
│   ▸ Bypass list:  @release-admins        (use is audited → protection.bypass_used)       │
│                                                            [Cancel]  [Save rule]          │
```
EMPTY: "No protection rules — this repo's branches are unprotected. [Protect a branch]".
LOADING: form skeleton.
ERROR: save fails → inline "Couldn't save — your changes are kept; retry" (never lose the edit).
CONFIRM: enabling a destructive option (e.g. allow force-push on main) confirms (§6.3 carve-out).
Power is **progressively disclosed** (P4) — advanced gates behind a "▸" expander.

---

## Screen G — Agent / HITL approval card (shared overlay; surfaces in Chat / Inbox / inline)

HAPPY (the §5.4 card, portal-to-root overlay, one z-index scale §8b.1):
```
┌ 🤖 code-review · awaiting your approval ─────────────────────────────┐
│ Proposed effects (plan-then-apply):                                  │
│   • open PR on acme/app  (protected: requires human approval)        │
│   • comment ×2 on the diff                                           │
│ Acting as: code-review  · on behalf of ◐ana  · scope: open_pr,comment│
│ Estimated cost: 0.42 credits           Why: ci/test failed on main   │
│ [ Approve ]   [ Edit plan ]   [ Reject ]                [audit trail↗]│
└──────────────────────────────────────────────────────────────────────┘
```
- Shows **proposed effects** (plan before effect §6.2), **identity/scope/delegation** (P7/§6.4),
  **live cost estimate** (AG-8), **why-it-fired provenance** (NOTIF-2). Strings **humanised at backend**
  (§8b.5). **Approve → resume** re-runs the gated step (AG-8); **Reject** discards; **Edit** attenuates
  the plan.
PENDING/EXPIRED: if the human doesn't act → the §5.10 agent-pending state; a HITL timeout (timer wheel)
re-surfaces or lapses per policy. The card is **never missed** — it lives in the inbox too (§5.8).
ERROR: if resume fails post-approval → "Couldn't apply — the approval is preserved; retry" (the durable
signal is idempotent; no double-apply).

---

## Screen H — Erasure / redaction admin (destructive, audited)

HAPPY:
```
│ Settings › Erasure & redaction                                  ⚠ destructive · audited  │
│ Rewrite history to remove content (e.g. a leaked secret or PII in files):                │
│  [ Select paths / pattern ]   [ Range: all history ▾ ]                                   │
│  ⚠ This CHANGES every downstream commit hash. It will invalidate:                        │
│     • existing clones, forks, and signatures                                             │
│     • mirrors and CDN clone caches                                                       │
│  Reach: replicas ✔ reflogs ✔ bitmaps ✔ backups ✔ · foreign push-mirrors: policy-gated   │
│                                          [Cancel]   [ Type repo name to confirm ▢ ]      │
```
- Note: **author/email metadata is already pseudonymous** (sketch 09), so this tool is for **PII in file
  content / legacy history** — the honest residual. Emits `git.repo.history_rewritten` (audit-critical).
- The confirmation is **explicit + typed** (the §6.3 GDPR/irreversible carve-out from
  reversibility-over-confirmation). The blast-radius warning is **named, not hidden** (P9 honesty).
EMPTY/ERROR/IN-PROGRESS: a long-running rewrite shows a durable-progress state (it's a `[Flow]` job);
failure is recoverable and audited.

---

## Cross-cutting checklist applied to every screen (§5.10 / §8b)
- **Empty** — onboarding-forward, names the next action.
- **Loading** — skeletons matching final layout; no blank spinner; ⌨ < ~100ms, no spinner-flash < ~1s.
- **Error** — one quiet system-blaming line + a path + retry; never a dead end; fail-static per surface.
- **Permission-denied** — graceful "no access" card; never a leaked title (P9/ADR-03).
- **Erased/tombstoned** — GDPR-aware degraded state; never a dangling leak.
- **Agent-pending** — the "agent working / awaiting approval" state.
- Overlays portal-to-root, one z-index scale, focus-trap+return, scroll-lock, Escape/backdrop (§8b.1).
- Diff/comment editor = the ONE shared render path (§8b.2); code-file *editing* is a separate code
  surface (sketch 08), not the rich editor.

## Cross-references
design-language §5.1/§5.3/§5.4/§5.5/§5.10/§6/§7.1/§8b; `user-flows.md`; sketches 06/07/09.
