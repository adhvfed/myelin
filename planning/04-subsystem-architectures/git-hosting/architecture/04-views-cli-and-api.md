# 04 — Views, CLI & API / Agent-Tool Surface

> The views (IA + flows + per-screen empty/loading/error states + the Myelin-specific
> agent-aware/per-viewer states), the two CLI surfaces (plain git + `myelin`), and the HTTP/RPC +
> agent-tool API. References the shared design language (`design-language.md` §7 view catalogue, §8b
> overlay primitives, §11 day-one UX mandates) and the design folder
> ([`../design/`](../design/)). Date: 2026-06-19.
>
> **UX is non-negotiable (EI-05).** Every screen specifies empty/loading/error. The shared overlay
> primitives (Dialog/Confirm/Popover/Dropdown/Tooltip/Toast) and the `myelin-content` editor (rich text
> for PR/review bodies + comments, so mentions and `ArtifactRef`s render consistently, ADR-05/KN-2) are
> reused, never re-built. Latency budgets (T-8): keyboard < ~100ms, no spinner-flash < ~1s, pages render
> (not animate-in). The frontend done-bar is the **switch test** driven through the real UI (T-7).

---

## 1. Information architecture

```
Repo  ──► Code (tree/file/blame/history/compare/search)
      ──► Pull Requests (list ──► PR detail: overview / files / checks / context-pane)
      ──► Branches & Tags
      ──► Settings (rulesets / collaborators / keys / subscriptions / erasure-admin)
      ──► Insights (throughput / review-latency — OLAP-fed)
```

The PR detail is the centrepiece; the **PR context pane** (the cross-artifact wedge) and the
**agent-aware review surface** are the Myelin-specific load-bearing affordances.

---

## 2. The views (each with states)

### 2.1 Repository & code browsing

- **Repo home** — README render, branch/tag switcher, language/size stats, default-branch file tree,
  quick actions (clone URL, "open in CLI", create branch/PR). *Empty:* no commits yet → clone/push
  instructions. *Loading:* cold-repo hydration skeleton (no spinner-flash < 1s). *Error:*
  over-residency-quota / repo unavailable / region-mismatch.
- **File tree & file view** — fast dir nav; syntax-highlighted view; permalink-by-SHA; **blame** (with
  ignore-rev); raw; image/binary/**LFS-aware** rendering; large-file graceful degradation. *Error:* LFS
  object missing / too large → degrade, never blank.
- **Commit list / commit detail** — per-path history; diff, parents, **signed/verified** status
  (commit/tag signatures; the repo's hash format is advertised — SHA-1+`sha1dc` default / SHA-256 opt-in,
  `01 §3`); lightweight commit-DAG viz.
- **Compare view** — arbitrary ref/SHA ↔ ref/SHA diff.
- **Code search results** — symbol/path/literal/trigram lexical (the GF-3 floor); *Empty:* no matches /
  index still building (honest "indexing…" state); *Error:* query too broad → bounded-cost reject.

### 2.2 Pull/Merge Request — the centrepiece

- **PR overview** — description (`myelin-content`), **linked issues/docs/runs via Refs**, participants,
  status, required-checks summary, **merge-readiness**, event timeline. *State:* a linked artifact the
  viewer can't see degrades to a **permission-stub** (tombstone), never leaks (ADR-03).
- **Diff / files-changed** — unified + split, syntax highlight, **per-file viewed/collapse state**,
  whitespace toggle, rename/move detection, **large-diff virtualization**, intra-line diff,
  expand-context. *State:* a thread **outdated** after force-push/rebase (the diff-anchor remap, `02
  §5`) renders with "view in original context".
- **Inline commenting & threads** — line/range/file comments; resolvable threads; **suggestions**
  (committable from UI); **multi-comment review batching** (start → batch → submit verdict).
- **Review verdicts** — approve / request-changes / comment; **CODEOWNERS + "who must still approve"**
  surfacing (from `list_subjects`).
- **Incremental review** — "changes since you last reviewed" (`review.head_oid_reviewed`).
- **Merge UX** — merge/squash/rebase, edit message, **auto-merge-when-green**, **merge-queue** position,
  conflict indication (+ single-file web edit; **no** 3-way conflict editor in v1, GF-6).
- **Checks/CI panel** — live status from CI events, required vs optional, re-run, logs deep-link.
- **Agent-aware review surface (load-bearing, Myelin-specific).** Agent reviewers/authors are **visually
  distinct and legible as agents** (which agent, why, provenance, the run) — never disguised as humans
  (ADR-08 AI-Act labelling). Humans can **request an agent review, dismiss/override**, and see the
  **audit trail**. An agent PR awaiting a HITL gate shows an approval state (the gate is a
  durable-workflow card, surfaced in Chat too — ADR-09/AG-8).
- **The PR context pane (the wedge).** The linked issue, doc section, CI run, and discussion inline,
  **each permission-filtered per viewer**, kept live by bus events. *Flow:* `GET pr` → git `Id.check`
  viewer → Refs `backlinks(pr, viewer)` (pre-filtered via `list_objects`) → resolve each surviving
  `ArtifactRef` via the owning subsystem's `project` API → assemble a pane showing only what the viewer
  may see. No subsystem touched another's DB.

### 2.3 Policy & settings

- **Ruleset / branch-protection editor** — ref patterns, required approvals (count / from CODEOWNERS /
  dismiss-stale), required checks, linear-history/signed-commits, force-push/deletion bans, **bypass
  lists** (audited), and **agent-specific rules** (`agent_needs_human`).
- **Repo/org settings** — collaborators & teams (compiled to ReBAC relations via `write_tuples`),
  visibility, default branch, allowed merge methods, **event subscriptions** (Myelin webhooks → Signals),
  keys/tokens (delegated to Id).
- **Fork / network**, **mirror config** (pull/push mirror, **residency-gated**: a push-mirror to a
  non-EU host is a residency boundary crossing, policy-gated, Phase-1 §9.3), **archive/transfer/delete**.
- **Erasure / redaction admin** — the destructive, audited **history-rewrite + crypto-shred** tool for
  secrets/PII incidents (GD-1 / GF-7), with explicit fork/mirror/clone-cache-invalidation warnings and a
  "this changes every downstream hash" confirm dialog.

### 2.4 Insights
- PR throughput / review-latency / merge-frequency — OLAP-fed off the bus (ADR-10), aggregate-only,
  residency-pinned.

---

## 3. CLI surfaces

### 3.1 Plain git (the server must support — stock clients just-work)

```
git clone <ssh|https-url>
git clone --filter=blob:none / --depth=N / --sparse     # partial / shallow / sparse
git fetch / pull / push
git push --force-with-lease                              # protection rules may reject
git lfs clone / pull / push                              # LFS batch protocol
git clone <bundle-uri> then incremental fetch            # accelerated clone (hot-repo)
```
Smart-HTTP **protocol v2** is the default. The repo's `object-format` (**sha1+`sha1dc` default / sha256
opt-in**, `01 §3`) is advertised; SHA-256 repos require a modern client (advertised via v2 capabilities).

### 3.2 Myelin CLI (`myelin …`) — a thin client over the SAME API the UI + agents use

```
myelin auth login                          # device/OAuth via shared Identity
myelin repo create|clone|fork|list|view|delete|transfer
myelin repo settings <repo> --visibility private|internal|public --default-branch main
                                            [--object-format sha256|sha1]
myelin pr create [--draft] [--base main] [--head feat] [--reviewer @u] [--agent-review code-review]
myelin pr list|view|checkout|diff
myelin pr review <pr> --approve|--request-changes|--comment [--inline path:line "…"]
myelin pr merge <pr> [--squash|--rebase|--merge] [--auto]   # --auto = merge-when-green (durable wait)
myelin pr ready <pr>
myelin branch protect <pattern> --require-approvals N --require-check ci/build --agent-needs-human
myelin codeowners validate
myelin key add|list|remove ; myelin token create           # delegated to shared Identity
myelin search code "<query>" [--repo …]
myelin subscription add <repo> --on git.pr.opened,git.ref.updated --to <target>
myelin agent review request <pr> --agent <name>            # invoke a (mock→real) agent reviewer
myelin repo erase-admin <repo> rewrite-history …           # the GD-1 destructive, audited path
```
The CLI noun alias `repo` maps to the canonical `git` ArtifactRef token (Bus §6.2; alias is render-time).

---

## 4. API / agent-tool surface

**One API surface, three consumers** (UI, CLI, agents). The HTTP/RPC API is the public gateway surface
(identity-injected, three-surface topology). The agent surface is the `ToolDef` set
([`03 §7`](./03-events-contracts-and-glue.md)): `git.read_file`, `git.read_diff`, `git.search_code`
(read-only), `git.open_pr`, `git.comment`, `git.submit_review`, `git.suggest_change`, `git.resolve_thread`,
`git.merge` (sensitive). Agents act through `EffectApi` (plan-then-apply), never direct writes; the same
endpoints the UI calls, no carve-out (ADR-08, `EffectApi` applies via the public endpoint).

Representative HTTP endpoints (illustrative):
```
GET  /api/git/repos                          # list_objects(viewer, pull, repo) pre-filtered
POST /api/git/repos
GET  /api/git/repos/{repo}/prs/{n}           # PR overview (calls project for the context pane)
POST /api/git/repos/{repo}/prs               # open PR
POST /api/git/repos/{repo}/prs/{n}/reviews   # submit review
POST /api/git/repos/{repo}/prs/{n}/merge     # merge gate + linearizable ref update
GET  /api/git/repos/{repo}/blob/{ref}/{path} # file view (project for unfurl)
GET  /api/git/search/code?q=…                # always conjoins list_objects (search-requires-acl-filter)
```

All write endpoints: `Id.check` → state change + `OutboxTx::emit` in one transaction (BUS-2). All
read endpoints that return cross-subsystem context use `project`/`resolve`, never cross-DB.
