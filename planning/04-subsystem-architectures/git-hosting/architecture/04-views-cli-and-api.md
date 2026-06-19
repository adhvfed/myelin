# 04 — Views, CLI & API / Agent-Tool Surface

> The views (IA + flows + per-screen empty/loading/error states + the Myelin-specific agent-aware/per-viewer
> states), the two CLI surfaces (plain git + `myelin`), and the HTTP/RPC + agent-tool API. References the
> shared design language (`design-language.md` §7 view catalogue, §8b overlay primitives, §11 day-one UX
> mandates) and the **preserved** design folder ([`../design/`](../design/)). Updated for the X-1 checks/
> fork-endorsement UX and the merge-queue resume. Date: 2026-06-19.
>
> **UX is non-negotiable (EI-05).** Every screen specifies empty/loading/error. The shared overlay
> primitives (Dialog/Confirm/Popover/Dropdown/Tooltip/Toast) and the `myelin-content` editor (rich text for
> PR/review bodies + comments — the frozen `myelin-content` subset, contract 13.1, so mentions and
> `ArtifactRef`s render consistently, ADR-05/KN-2) are reused, never re-built. Latency budgets (T-8):
> keyboard < ~100ms, no spinner-flash < ~1s, pages render (not animate-in). The frontend done-bar is the
> **switch test** driven through the real UI (T-7).

---

## 1. Information architecture

```
Repo  ──► Code (tree/file/blame/history/compare/search)
      ──► Pull Requests (list ──► PR detail: overview / files / checks / context-pane)
      ──► Branches & Tags
      ──► Settings (rulesets / collaborators / keys / subscriptions / mirror / erasure-admin)
      ──► Insights (throughput / review-latency — OLAP-fed)
```

The PR detail is the centrepiece; the **PR context pane** (the cross-artifact wedge) and the **agent-aware
review surface** are the Myelin-specific load-bearing affordances.

---

## 2. The views (each with states)

### 2.1 Repository & code browsing

- **Repo home** — README render, branch/tag switcher, language/size stats, default-branch file tree, quick
  actions (clone URL incl. **bundle-URI for hot repos**, "open in CLI", create branch/PR). *Empty:* no
  commits → clone/push instructions. *Loading:* cold-repo hydration skeleton (no spinner-flash < 1s).
  *Error:* over-residency-quota / repo unavailable / region-mismatch.
- **File tree & file view** — fast dir nav; syntax-highlighted view; permalink-by-SHA; **blame** (ignore-rev);
  raw; image/binary/**LFS-aware** rendering; large-file graceful degradation. *Error:* LFS object missing /
  too large → degrade, never blank.
- **Commit list / commit detail** — per-path history; diff, parents, **signed/verified** status; the repo's
  hash format advertised (SHA-1+`sha1dc` default / SHA-256 opt-in, `01 §3`); lightweight commit-DAG viz.
- **Compare view** — arbitrary ref/SHA ↔ ref/SHA diff.
- **Code search results** — symbol/path/literal/trigram lexical (GF-3), **ACL-pre-filtered via the OQ-E
  `Filter`** (`03 §5.3`). *Empty:* no matches / index still building (honest "indexing…"); *Error:* query too
  broad → bounded-cost reject.

### 2.2 Pull/Merge Request — the centrepiece

- **PR overview** — description (`myelin-content`), **linked issues/docs/runs via Refs**, participants,
  status, required-checks summary, **merge-readiness**, event timeline. *State:* a linked artifact the viewer
  can't see degrades to a **permission-stub** (tombstone), never leaks (ADR-03).
- **Diff / files-changed** — unified + split, syntax highlight, **per-file viewed/collapse state**,
  whitespace toggle, rename/move detection, **large-diff virtualization**, intra-line diff, expand-context.
  *State:* a thread whose anchor went **`moved`** (rebased) renders at the shifted range; **`outdated`**
  (partial) renders the surviving sub-range with "view in original context"; **`gone`** renders a tombstone
  on the parent PR — the content-fingerprint four states (`02 §5`, contract 5.7).
- **Inline commenting & threads** — line/range/file comments; resolvable threads; **suggestions**
  (committable from UI); **multi-comment review batching** (start → batch → submit verdict).
- **Review verdicts** — approve / request-changes / comment; **CODEOWNERS + "who must still approve"**
  surfacing (from `list_subjects(pr, review)`, contract 4.4).
- **Incremental review** — "changes since you last reviewed" (`review.head_oid_reviewed`).
- **Merge UX** — merge/squash/rebase, edit message, **auto-merge-when-green**, **merge-queue position**,
  conflict indication (+ single-file web edit; **no** 3-way conflict editor in v1, GF-6). The merge-when-green
  / queue surface is driven by the **durable `ci.result` wait** (`02 §6.4`) — the card shows "queued →
  testing → merged", and on a multi-day HITL hold shows the pending approval (the workflow holds no runtime
  while it waits).
- **Checks/CI panel (the X-1 consumer surface).** Live per-context status from the **`check_status`
  projection** (fed by `ci.check.updated`, `02 §6.1`): each row shows `state`, `required?` (Git's
  branch-protection policy decides), the **humanised `summary`** (a `(template_key, args)` pair, never a raw
  CI string — contract 7.3), and a **jump-to-failure** deep-link resolving the `details_ref` `#step-<n>`
  sub-anchor into CI's run view. **Fork/trust UX (the security-critical affordance):** a check from an
  `untrusted_fork` run shows a distinct **"awaiting maintainer approval to run/trust"** state and is
  **neutral for gating** until a maintainer with `approve_untrusted_ci` endorses it (or re-runs it trusted) —
  the poisoned-pipeline defence made visible (`02 §6.3`). *Empty:* no checks configured. *Loading:* checks
  queued. *Error:* a check `error`/`cancelled` distinct from `failure`.
- **Agent-aware review surface (load-bearing, Myelin-specific).** Agent reviewers/authors are **visually
  distinct and legible as agents** (which agent, why, provenance, the run) — never disguised as humans (ADR-08
  AI-Act). Humans can **request an agent review, dismiss/override**, and see the **audit trail**. An agent PR
  awaiting a HITL gate (`git.merge` `requires_approval = yes`) shows an approval state (the gate is a
  durable-workflow card, surfaced in Chat too — ADR-09/AG-8). Explicit-first: an @-mention of an agent
  reviewer notifies, does not auto-spawn a costed run (CHAT-1).
- **The PR context pane (the wedge).** The linked issue, doc section, CI run, and discussion inline, **each
  permission-filtered per viewer**, kept live by bus events. *Flow:* `GET pr` → git `Id.check` viewer → Refs
  `backlinks(pr, viewer)` (pre-filtered via the `list_objects` `Filter`) → resolve each surviving
  `ArtifactRef` via the owning subsystem's `project` API (cell-local, contract 5.2) → assemble a pane showing
  only what the viewer may see. No subsystem touched another's DB.

### 2.3 Policy & settings

- **Ruleset / branch-protection editor** — ref patterns, required approvals (count / from CODEOWNERS /
  dismiss-stale), **the `required_contexts` set** (Git decides which CI/external contexts gate — X-1),
  linear-history/signed-commits, force-push/deletion bans, **bypass lists** (audited), **agent-specific
  rules** (`agent_needs_human`), and the **`approve_untrusted_ci`** assignee set (who may endorse fork CI).
- **Repo/org settings** — collaborators & teams (compiled to ReBAC relations via `write_tuples`, contract
  4.6), visibility, default branch, allowed merge methods, **event subscriptions** (Myelin webhooks →
  Signals), keys/tokens (delegated to Id; deploy key = repo-scoped machine principal).
- **Fork / network**, **mirror config** (pull/push mirror — a **push-mirror to a non-EU host is denied by
  default** at the **control-plane residency gate**, `transfer_allowed`, contract 10.5 / CR-TEN-2),
  **archive/transfer/delete**.
- **Erasure / redaction admin** — the destructive, **audited history-rewrite** tool (contract 10.6) for
  secrets/PII incidents, with explicit **fork/mirror/clone-cache-invalidation** warnings (the trust-scoped
  cache namespaces + CDN clone class are invalidated, Storage 11.2) and a "this changes every downstream
  hash" confirm dialog. The residual lawful-basis is the ONE platform posture (contract 10.9 / recon §X-7),
  surfaced here as a documented limit, not restated.

### 2.4 Insights

- PR throughput / review-latency / merge-frequency — OLAP-fed off the bus (ADR-10), aggregate-only,
  residency-pinned, **honouring the `restrict` flag** (no analytics for a restricted subject, contract 11.6).

---

## 3. CLI surfaces

### 3.1 Plain git (the server must support — stock clients just-work)

```
git clone <ssh|https-url>
git clone --filter=blob:none / --depth=N / --sparse     # partial / shallow / sparse
git fetch / pull / push
git push --force-with-lease                              # protection rules may reject
git lfs clone / pull / push                              # LFS batch protocol
git clone <bundle-uri> then incremental fetch            # accelerated clone (hot-repo CDN class)
```
Smart-HTTP **protocol v2** is the default. The repo's `object-format` (**sha1+`sha1dc` default / sha256
opt-in**, `01 §3`) is advertised; SHA-256 repos require a modern client.

### 3.2 Myelin CLI (`myelin …`) — a thin client over the SAME API the UI + agents use

```
myelin auth login                          # device/OAuth via shared Identity
myelin repo create|clone|fork|list|view|delete|transfer
myelin repo settings <repo> --visibility private|internal|public --default-branch main
                                            [--object-format sha256|sha1]
myelin pr create [--draft] [--base main] [--head feat] [--reviewer @u] [--agent-review code-review]
myelin pr list|view|checkout|diff
myelin pr review <pr> --approve|--request-changes|--comment [--inline path:line "…"]
myelin pr merge <pr> [--squash|--rebase|--merge] [--auto]   # --auto = merge-when-green (durable ci.result wait)
myelin pr checks <pr>                        # the check_status projection: per-context state/required/summary
myelin pr endorse-fork-ci <pr>               # approve_untrusted_ci (a maintainer trusts the fork run, X-1)
myelin pr ready <pr>
myelin branch protect <pattern> --require-approvals N --require-context ci/build --agent-needs-human
myelin codeowners validate
myelin key add|list|remove ; myelin token create           # delegated to shared Identity
myelin search code "<query>" [--repo …]
myelin subscription add <repo> --on git.pr.opened,git.ref.updated --to <target>
myelin agent review request <pr> --agent <name>            # explicit-first; invoke a (mock→real) reviewer
myelin repo mirror add <repo> --push <url>                 # residency-gated (denied to extra-EU by default)
myelin repo erase-admin <repo> rewrite-history …           # the audited history-rewrite path (10.6)
```
The CLI noun alias `repo` maps to the canonical `git` ArtifactRef token (contract 14; alias is render-time).

---

## 4. API / agent-tool surface

**One API surface, three consumers** (UI, CLI, agents). The HTTP/RPC API is the public gateway surface
(identity-injected, three-surface topology). The agent surface is the `ToolDef` set ([`03 §7`](./03-events-contracts-and-glue.md)),
with the frozen `requires_approval` defaults (`git.merge` = yes, `open_pr` = no). Agents act through
`EffectApi` (plan-then-apply), never direct writes; the same endpoints the UI calls, no carve-out (ADR-08).

Representative HTTP endpoints (illustrative):
```
GET  /api/git/repos                          # list_objects(viewer, pull, repo) — the SetExpr Filter push-down
POST /api/git/repos
GET  /api/git/repos/{repo}/prs/{n}           # PR overview (calls project for the context pane)
GET  /api/git/repos/{repo}/prs/{n}/checks    # the check_status projection (X-1 consumer view)
POST /api/git/repos/{repo}/prs               # open PR
POST /api/git/repos/{repo}/prs/{n}/reviews   # submit review
POST /api/git/repos/{repo}/prs/{n}/endorse-fork-ci  # check(approve_untrusted_ci) → stamp endorsed_by
POST /api/git/repos/{repo}/prs/{n}/merge     # merge gate + linearizable ref update (+ enqueue if queued)
GET  /api/git/repos/{repo}/blob/{ref}/{path} # file view (project for unfurl; #L sub content-anchored)
GET  /api/git/search/code?q=…                # always conjoins the OQ-E Filter (search-requires-acl-filter)
```

All write endpoints: `Id.check` → state change + `OutboxTx::emit` in one transaction (BUS-2). All read
endpoints returning cross-subsystem context use `project`/`resolve` (cell-local), never cross-DB.
