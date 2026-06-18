# Phase 2 — Subsystem Architecture: Git Hosting & Code Review

> Phase: `02-holistic-architecture`. Canonical brief: [`VISION.md`](../../../VISION.md)
> (never contradicted). Phase-2 spine: [`architecture-decisions.md`](../architecture-decisions.md)
> (the ADR register) and [`system-overview.md`](../system-overview.md) (the holistic narrative).
> Phase-1 deep-dive this builds on:
> [`01-research/subsystem-deep-dives/git-hosting.md`](../../01-research/subsystem-deep-dives/git-hosting.md);
> structural foundation: [`01-research/technical-structuring.md`](../../01-research/technical-structuring.md).
>
> **Altitude.** This is the *high-level* architecture: role, internal structure, tech direction,
> the views and CLI, how it interacts with the rest of the platform, and what it implies for the
> shared systems. Concrete schemas, the git-core build-vs-embed decision, replication internals,
> diff-anchoring algorithms, and the SHA-1/256 call are **Phase-4** work and are flagged as open.
> Where this doc must take a position to keep Phase 2 coherent, it does so and cites the ADR.

---

## 1. Role & responsibilities — what it OWNS vs delegates

Git hosting is the **system of record for source code and its history**, and the *gravitational
centre of the engineering side*: CI is triggered by it, issues reference its commits/branches,
docs link to its files, chat unfurls its PRs, and it is **where the cross-artifact reference
graph is densest** (`git-hosting.md §1`). The differentiator is **not the git server** — every
competitor has a competent one — it is that this one sits on Myelin's unified
identity/permission/event/reference fabric and is **agent-native** (`git-hosting.md §1.1`).

### Owns (its core competency — `system-overview.md §4`)

- **The Git object store and serving core**: blobs/trees/commits/tags, refs, packfiles, delta
  compression, reachability acceleration (commit-graph, bitmaps, MIDX), GC/repack, partial-clone
  / sparse / shallow serving, and the **Git wire protocol** (smart-HTTP v2 + SSH).
- **Hosting-layer domain entities** that are *not* in git itself (`git-hosting.md §2.2`): the
  Repository (visibility, default branch, settings, tenant binding), Fork/network, the
  **Pull/Merge Request** lifecycle, **Reviews + inline comment threads** (with diff-anchoring),
  **Branch protection / rulesets**, CODEOWNERS, deploy keys (binding), and commit-status/check
  aggregation.
- **The merge gate** — *the place "what is allowed to land" is decided*: branch-protection
  evaluation, required-review/required-check enforcement, the merge queue, and the
  *agent-vs-human* merge policy.
- **What to index** for code/PR search (it owns the indexing logic and incremental update on
  push; Search owns the index plumbing).
- **Its erasure obligations** as a `PersonalDataHolder` — the hardest in the platform because git
  objects are content-addressed and immutable (`git-hosting.md §9`, ADR-12).

### Delegates to the shared systems (ADR-13; `system-overview.md §4`)

| Concern | Delegated to | Git hosting still owns… |
|---|---|---|
| Who a principal is; SSH-key/token/OAuth auth; org/team model; user lifecycle | **Identity & Access** (ADR-03) | the *git-specific* authz decisions (push to protected ref, who-can-merge) expressed as ReBAC relations |
| Emitting/consuming events | **Event Bus** (ADR-04) | the push-hook → transactional-outbox → bus path; per-ref ordering |
| Commit↔issue↔doc↔run edges, backlinks | **Reference Graph** (ADR-13) | producing `ref.created` from commit trailers / PR links; exposing stable `ArtifactRef`s |
| Code/PR/comment index + query | **Search** (ADR-03, ADR-10) | what gets indexed; incremental update on push; permission scoping inputs |
| Durable bytes (LFS, packs-as-blobs, bundles, backups) | **Storage** (ADR-10) | the LFS batch protocol, content-addressing, residency tags |
| "review requested / PR merged / check failed" delivery | **Notifications** (ADR-12) | which events are notifiable + their targets |
| Agent authors/reviewers; trigger dispatch; plan-then-apply | **Agent Fabric** (ADR-08) | registering its `ToolDef`s and the events that drive triggers |
| Long-running / human-gated flows (e.g. agent-PR HITL gate, auto-merge-when-green wait) | **Durable-workflow** (ADR-09) | the merge-queue state machine semantics |
| DSR fan-out, KMS/crypto-shred, audit | **GDPR/Audit** (ADR-12) | implementing `locate/export/rectify/restrict/erase` over git+metadata |

**Hard rule (ADR-01/ADR-13):** no subsystem reads git hosting's DB, and git hosting reads no
other subsystem's DB. Cross-subsystem reads (e.g. the PR context pane) go through `ArtifactRef`
resolution + each subsystem's **projection API**, permission-filtered per viewer
(`system-overview.md §8.1`).

---

## 2. High-level internal structure

Architecture altitude — major components and their seams, not implementation. The subsystem
splits cleanly into a **stateless front door**, a **stateful serving tier**, a **metadata/control
plane**, and an **async event/index path** — the shape `technical-structuring.md §5.3` prescribes
for world-scale inside a cell.

```
            ┌──────────────────────── CLIENTS ────────────────────────┐
            │  git wire (SSH / smart-HTTP v2) · Web UI · Myelin CLI ·  │
            │  API · MCP (external agents, later)                      │
            └───────────────┬─────────────────────────────────────────┘
                            ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │  (A) GIT FRONT DOOR / ROUTER  (stateless-ish, per-cell, region-pinned) │
   │   authn→Principal (Id) · authz gate (Id.check) · placement lookup ·    │
   │   residency check (in-region only) · streams packs (no full buffering) │
   │   · rate-limit / backpressure · SSH & smart-HTTP v2 endpoints          │
   └───────────────┬───────────────────────────────────┬───────────────────┘
                   ▼ (git transactions)                 ▼ (PR/review/API/UI)
   ┌───────────────────────────────┐     ┌──────────────────────────────────┐
   │ (B) REPO SERVING TIER (stateful)│   │ (C) HOSTING CONTROL PLANE (OLTP)  │
   │  git-core engine: upload-pack / │   │  PR / review / comment-thread /   │
   │  receive-pack, pack/delta, GC/  │   │  repo / fork / ruleset / merge-   │
   │  repack, commit-graph+bitmaps,  │   │  queue state · branch-protection  │
   │  reftable/DB ref store, partial-│   │  evaluator · CODEOWNERS resolver ·│
   │  clone/sparse · push-policy hook│   │  diff-anchor service · check/     │
   │  (pre-receive) + outbox emit    │   │  status aggregator · projection   │
   │  (post-receive)                 │   │  API (ArtifactRef → render)       │
   └───────┬───────────────┬─────────┘   └───────┬──────────────────┬────────┘
           │ packs/LFS/    │ outbox events       │ outbox events    │ reads
           ▼ bundles       ▼                     ▼                  │
   ┌──────────────┐   ┌────────────────────────────────────────┐    │
   │ STORAGE      │   │           EVENT BUS (ADR-04)            │◄───┘
   │ (object tier,│   │  ref.updated · pr.* · review.* (envelope│
   │  S3-compat)  │   │  + outbox + per-aggregate order)        │
   └──────────────┘   └───┬────────┬────────┬────────┬──────────┘
                          ▼        ▼        ▼        ▼
                        REFS    SEARCH   AGENTS    NOTIF/CI/OLAP …
```

**(A) Git front door / router.** The SSH + smart-HTTP v2 endpoints. Authenticates to a
`Principal` (SSH pubkey or token via Id), runs the per-action authz gate, resolves
`repo_id → backend node(s)`, **rejects any route that would leave the region** (ADR-11), and
streams the transaction to the serving tier without buffering whole packs in memory
(`git-hosting.md §3.4, §6`). Rate-limiting/backpressure live here. This is the
stateless-ish front door of `technical-structuring.md §5.3`.

**(B) Repo serving tier (stateful).** The git core itself: `upload-pack`/`receive-pack`,
pack/delta storage, reachability acceleration, GC/repack, the ref store, and partial-clone/
sparse serving. **Push-time policy runs here** as a `pre-receive`-equivalent (branch protection,
secret-scan, size/agent rules — *reject before the ref moves*), and event emission runs as a
`post-receive`-equivalent that writes to the **transactional outbox** in the same transaction
that moves the ref (`git-hosting.md §6, §7.1`, ADR-04). Repo placement, replication, and the ref
store are this tier's scale hot-spot (`§11` below).

**(C) Hosting control plane (OLTP).** The Postgres-class store and services for everything that
is *not* a git object: PR/review/comment/thread/repo/fork/ruleset/merge-queue rows, the
**branch-protection evaluator**, **CODEOWNERS resolver**, **diff-anchor service** (the hard
correctness battleground, `git-hosting.md §4.3`), the **check/status aggregator** (consumes CI),
and the **projection API** that resolves an `ArtifactRef` to a per-viewer rendered projection for
unfurls/embeds (ADR-13). Also emits to the outbox.

**(D) Async event/index path.** Off the bus: the search indexer (incremental on push), reference
graph edge creation, notification routing, the OLAP feed, and the agent trigger engine — all
idempotent on `event_id` (ADR-04). Never in the synchronous push/PR write path
(`system-overview.md §7`).

**Two-transport discipline (ADR-04).** Durable control/domain events (`ref.updated`, `pr.*`,
`review.*`) ride the durable bus. Git hosting has no firehose of its own comparable to CI logs or
chat presence — but **streaming clone/fetch byte transfer** is its high-volume path and stays on
the git wire/object tier, *never* on the durable bus.

---

## 3. The TECH it would use

Consistent with ADR-02/ADR-14 (Rust default; PG + object store). No divergence from Rust is
proposed for git hosting; the one genuine open tech question is *how the git core is realised*,
not *what language* the subsystem is in.

| Layer | Direction | Rationale / citation |
|---|---|---|
| **Service language** | **Rust** | ADR-02/ADR-14 default; matches the "git serving core stays Rust" hot-path guidance (ADR-02). Mononoke is the existence proof a scalable Rust git server is feasible (`git-hosting.md §3.2, §12`). |
| **Git core** | **[OPEN → P4, TE-8]** Embed `gix` (gitoxide, Rust) where mature, **shell out to canonical `git`** for the not-yet-complete server-side paths, libgit2 as fallback. Pragmatic baseline = canonical git behind a Rust service; migrate paths to `gix` as it matures. | `git-hosting.md §12` flags `gix` is *not yet feature-complete for full server-side serving*; this must be re-verified in P4. Phase 2 does **not** foreclose either way. |
| **Ref store** | Filesystem `packed-refs` does **not** scale; direction is **reftable** (Gerrit/JGit format, upstreamed) **or a transactional/DB-backed ref store** | `git-hosting.md §2.3, §3.4` — millions of refs make loose-ref dirs pathological; intersects replication. |
| **Hosting metadata (PR/review/repo/ruleset)** | **Postgres-class** (OLTP), JSONB where flexible, residency-pinned, per-tenant envelope-encrypted | ADR-10/ADR-14: PG for domain state; ADR-12 for encryption/crypto-shred. |
| **Pack/LFS/bundle/backup bytes** | **S3-compatible object store** (MinIO/Ceph self-hostable, EU providers), content-addressed, dedup, residency-pinned | ADR-10 object tier; `git-hosting.md §3.3, §3.4`. Object-store-backed packs (Mononoke/JGit-DFS style) are an *option* flagged to P4 (TE-24). |
| **Repo placement/replication** | **[OPEN → P4, TE-24]** repo-level sharding + N replicas; quorum/voting (Spokes-style) **or** primary+WAL (Praefect-style) **or** Raft on ref updates | `git-hosting.md §3.4, §11`. Must give **linearizable protected-ref merges** with no split-brain. P2 commits the *requirement*, not the mechanism. |
| **Code search** | **Shared Search** (Tantivy-class, ADR-10/ADR-14); git hosting owns *what* to index | `git-hosting.md §4.5`. v1 = per-repo/per-tenant lexical; global semantic/symbol nav deferred (TE-27). |
| **Diff/anchor + projection services** | Rust services in the control plane | `git-hosting.md §4.3`; the projection API is the ADR-13 contract surface. |

**SHA-1 vs SHA-256 (`git-hosting.md §2.1`, TE-23): [OPEN → P4].** Strategic; Phase 2 does not
decide. Both have real costs (SHA-256 buys collision-resistance but risks client/tooling interop;
SHA-1 matches the ecosystem but inherits SHAttered-class weakness). Flagged.

---

## 4. The VIEWS / SCREENS the UI requires

Feeds the shared design-language work and the Phase-4 design sketches. Each must specify
**empty / loading / error** states (VISION §3); the agent-aware and per-viewer-permission states
are called out because they are Myelin-specific. The editor uses the shared `myelin-content`
component for rich text (PR/review bodies, comments) so mentions and `ArtifactRef`s render
consistently (ADR-05).

### 4.1 Repository & code browsing (`git-hosting.md §4.1`)
- **Repo home** — README render, branch/tag switcher, language/size stats, default-branch file
  tree, quick actions (clone URL, "open in CLI", create branch/PR). *Empty:* no commits yet
  (clone/push instructions). *Loading:* cold-repo hydration. *Error:* over-residency-quota / repo
  unavailable.
- **File tree & file view** — fast dir nav; syntax-highlighted view; **blame** (with ignore-rev);
  raw; image/binary/**LFS-aware** rendering; large-file graceful degradation.
- **Commit list / commit detail** — per-path history; diff, parents, **signed/verified** status;
  lightweight commit-DAG viz.
- **Compare view** — arbitrary ref/SHA ↔ ref/SHA diff.
- **Code search results** — lexical now; symbol/"go-to-definition" later (TE-27).

### 4.2 Pull/Merge Request — the centrepiece (`git-hosting.md §4.2`)
- **PR overview** — description, **linked issues/docs/runs (cross-artifact via Refs)**,
  participants, status, required-checks summary, **merge-readiness**, event timeline.
- **Diff / files-changed view** — unified + split, syntax highlight, **per-file viewed/collapse
  state**, whitespace toggle, rename/move detection, **large-diff virtualization**, intra-line
  diff, expand-context.
- **Inline commenting & threads** — line/range/file comments; resolvable threads; **suggestions**
  (committable from UI); **multi-comment review batching** (start → batch → submit verdict).
  *State:* thread **outdated** after force-push/rebase (diff-anchoring, `§4.3` research).
- **Review verdicts** — approve / request-changes / comment; **CODEOWNERS + "who must still
  approve"** surfacing.
- **Incremental review** — "changes since you last reviewed" (depends on diff-position tracking).
- **Merge UX** — merge/squash/rebase, edit message, **auto-merge-when-green**, **merge-queue**
  surfacing, conflict indication (+ simple in-UI resolution, scope TBD).
- **Checks/CI panel** — live status from CI events, required vs optional, re-run, logs deep-link.
- **Agent-aware review surface (Myelin-specific, load-bearing).** Agent reviewers/authors are
  **visually distinct and legible as agents** (which agent, why, provenance) — never disguised as
  humans (`git-hosting.md §4.2`, ADR-08 AI-Act labelling). Humans can **request an agent review,
  dismiss/override** it, and see the **audit trail**. Agent PRs awaiting a HITL gate show an
  approval state (the gate is a durable-workflow card surfaced in Chat too — ADR-09).
- **The PR context pane** — the wedge made concrete (`system-overview.md §8.1`): the linked
  issue, doc section, CI run, and discussion inline, **each permission-filtered per viewer**,
  kept live by bus events. *State:* a referenced artifact the viewer can't see degrades to a
  permission-stub, never leaks (ADR-03).

### 4.3 Policy & settings (`git-hosting.md §4.4`)
- **Ruleset / branch-protection editor** — ref patterns, required approvals (count, from
  CODEOWNERS, dismiss-stale), required checks, linear-history/signed-commits, force-push/deletion
  bans, **bypass lists**, and **agent-specific rules** (e.g. "agent PRs require human approval").
- **Repo/org settings** — collaborators & teams (compiled to ReBAC relations), visibility,
  default branch, allowed merge methods, **event subscriptions** (Myelin term for webhooks),
  keys/tokens.
- **Fork / network**, **mirror config** (pull/push mirror, residency-gated, `§6` research),
  **archive/transfer/delete**.
- **Erasure / redaction admin** — the destructive, audited history-rewrite + crypto-shred tool
  surfaced for secrets/PII incidents, with explicit fork/mirror-invalidation warnings
  (`git-hosting.md §9.2`).

---

## 5. The CLI COMMANDS this subsystem exposes

Two surfaces (`git-hosting.md §5`): **(a) plain `git`** over the wire (must just-work with stock
clients), and **(b) the Myelin CLI** — a thin client over the *same* APIs the UI and agents use
(one API surface, three consumers). Names illustrative; finalised in P4 against the design
language.

### 5.1 Plain git (the server must support)
```
git clone <ssh|https-url>
git clone --filter=blob:none / --depth=N / --sparse   # partial / shallow / sparse
git fetch / pull / push
git push --force-with-lease                            # protection rules may reject
git lfs clone / pull / push                            # if LFS supported (P4)
git bundle / clone from bundle URI                     # accelerated clone (hot-repo)
```
Smart-HTTP **protocol v2** is the default (better for huge ref counts; `git-hosting.md §6`).

### 5.2 Myelin CLI (`myelin …`)
```
myelin auth login                          # device/OAuth via shared Identity
myelin repo create|clone|fork|list|view|delete|transfer
myelin repo settings <repo> --visibility private|internal|public --default-branch main
myelin pr create [--draft] [--base main] [--head feat] [--reviewer @u] [--agent-review triage]
myelin pr list|view|checkout|diff
myelin pr review <pr> --approve|--request-changes|--comment [--inline path:line "…"]
myelin pr merge <pr> [--squash|--rebase|--merge] [--auto]     # --auto = merge-when-green (durable wait)
myelin pr ready <pr>                        # un-draft
myelin branch protect <pattern> --require-approvals N --require-check ci/build --agent-needs-human
myelin codeowners validate
myelin key add|list|remove ; myelin token create     # delegated to shared Identity
myelin search code "<query>" [--repo …]
myelin subscription add <repo> --on pr.opened,ref.updated --to <target>
myelin agent review request <pr> --agent <name>      # invoke a (mock→real) agent reviewer
```

---

## 6. USAGE EXAMPLES (end-to-end)

### 6.1 Human opens a PR, requests an agent review, merges when green

**UI flow.** Dev pushes `feat/login` → opens the **PR create** view (base `main`, head
`feat/login`); the PR overview shows linked issue `ISSUE-412` (auto-detected from the
`Closes ISSUE-412` trailer → Refs), the **required-checks summary** (CI `build`, `test`), and a
**"Request agent review"** affordance. Dev clicks it; the **agent-aware review surface** shows the
mock `code-review` agent's verdict as a *distinctly-styled agent comment*. Dev addresses
comments, enables **auto-merge-when-green**; when CI reports success the merge queue lands it.

**CLI equivalent.**
```
$ git push origin feat/login
$ myelin pr create --base main --head feat/login --reviewer @sarah --agent-review code-review
  → opened PR #88  (linked: ISSUE-412 via trailer; checks: ci/build, ci/test pending)
$ myelin pr merge 88 --squash --auto
  → auto-merge armed: will merge when required checks pass and approvals satisfied
```

**What happens underneath (ADR-04/08/13).** `git push` → front door authz (`Id.check(dev, push,
feat/login)`) → serving tier pre-receive policy passes → ref moves + **outbox emits
`ref.updated`** atomically. `pr create` writes the PR row + outbox `pr.opened`. The `Closes`
trailer → `ref.create` (PR→issue edge in Refs). The agent-review request registers a trigger;
the `code-review` agent wakes on `pr.opened`/`pr.synchronized`, returns an `AgentDecision{effects:
pr.comment×N, review.submit}` (**plan-then-apply** — it proposes, never acts), `EffectApi`
validates against `perms ∩ delegation ∩ tenant` and applies. `--auto` registers a **durable
workflow** that waits on the check aggregator; on `pr.merge_blocked` clearing, it merges (a
linearizable protected-ref update) and emits `pr.merged`.

### 6.2 CI fails on `main` → agent triages → opens an issue → proposes a fix PR (HITL-gated)

This is the agent-native flagship (`system-overview.md §8.2`). CI emits `ci.pipeline.failed`;
a triage agent opens `ISSUE-412`, links it (Refs), posts to chat; a fix agent proposes
**PR #88 on a protected repo** — a *sensitive* effect, so `EffectApi` returns **Gated**, opening a
**durable-workflow HITL gate** surfaced as a Chat approval card. A human approves *days later*;
the workflow resumes and `git.open_pr` applies. Git hosting's role: it **emits** the precise
`pr.opened` event, **enforces** that the agent is subject to branch protection like anyone (an
agent cannot bypass required human approval unless policy allows, `git-hosting.md §7.3, §8`), and
**renders** the agent author legibly in the PR surface.

### 6.3 The PR context pane (the wedge — `system-overview.md §8.1`)
`GET pr #88` → Git checks viewer authz → asks **Refs** for PR#88's edges → Refs pre-filters
targets via **Id.`list-objects`** → Git resolves each surviving `ArtifactRef` via the *owning
subsystem's projection API* (Issues, Knowledge, CI, Chat) → assembles a pane showing **only what
the viewer may see**, kept live by bus update events. No subsystem touched another's DB.

---

## 7. How it INTERACTS with the rest of the platform

### 7.1 Events EMITTED (illustrative names; canonical taxonomy is P3, ADR-13)
`repo.created/deleted/visibility_changed/transferred/archived`, `repo.fork.created`,
`branch.created/deleted/protection_changed`, **`ref.updated`** (the core push event: repo, ref,
old_sha, new_sha, pusher, commits, forced? — CI/Search/Refs/Agents all consume it),
`tag.created/deleted`, `pr.opened/updated/ready_for_review/closed/reopened/merged/synchronized`,
`pr.review.requested/submitted/dismissed`, `pr.comment.created/resolved`, `pr.thread.resolved`,
`pr.check.required_failed`, `pr.merge_blocked/merge_queued`, `codeowners.review_required`,
**`protection.bypass_used`** (audit-critical), `key.added`/`token.created` (echoed from Identity).
All carry the **non-negotiable envelope** (ADR-13): `event_id` (idempotency), `tenant`, `region`,
`actor` (human/agent/service incl. on-behalf-of), `subject` (`ArtifactRef`), `causation_id` +
`correlation_id`, `contains_personal_data`, `visibility`. **Per-ref ordering matters** (consumers
need `ref.updated` order for a given ref) — satisfied by per-aggregate ordering (ADR-04).

### 7.2 Events CONSUMED (`git-hosting.md §7.2`)
- **From CI** — `check.run.*` / `pipeline.status` → update PR check status, gate/unblock merge,
  drive the merge queue. (The **Git↔CI commit-status/checks contract** is the most load-bearing
  cross-subsystem seam, `system-overview.md §4`; jointly designed in P4.)
- **From Agent Fabric** — agent intents to open PR / review / comment / merge arrive as
  *proposed effects* validated by `EffectApi` (plan-then-apply, ADR-08), never as direct writes.
- **From Issues** — issue closed/linked → reflect `Closes #N` linkage / auto-close on merge.
- **From Identity** — permission/membership changes → recompute who-can-review/merge; **user
  deletion → erasure flow** (ADR-12).
- **From Knowledge/Chat** — `ref.created` where a doc/message now references a commit/PR → surface
  "referenced by" (the inverse index lives in Refs).

### 7.3 Shared-system touchpoints
- **Identity & Access (ADR-03).** Authz at **every** entrypoint — SSH, smart-HTTP, API, UI, CLI,
  and the event-triggered agent path (`git-hosting.md §8`). Roles (collaborator/maintainer/admin)
  are the *authoring face*, **compiled to ReBAC relations** (org→team→repo→branch→action). The
  merge gate and CODEOWNERS reduce to relationship checks; `list-objects` powers permission-aware
  repo/PR lists and the context pane.
- **Reference Graph (ADR-13).** Densest producer/consumer on the engineering side. Produces edges
  from commit trailers and PR links (`ref.created`); exposes stable, sub-artifact `ArtifactRef`s
  (`myelin://<tenant>/git/pr/<n>#comment-<id>`, `.../commit/<sha>`, `.../blob/<path>#L42`).
- **Search (ADR-03/10).** Owns *what* to index (code content, PR/comment text, commit messages),
  incremental on push; Search owns the ACL-aware index and `list-objects` pre-filter.
- **Storage (ADR-10).** LFS bytes, optional object-store-backed packs, **clone/bundle artifacts**
  (hot-repo/clone-storm mitigation, `git-hosting.md §3.4`), and backups — all residency-pinned,
  content-addressed.
- **Notifications (ADR-12).** review-requested, mentioned, PR-merged, check-failed.
- **Agent Fabric (ADR-08).** Registers typed `ToolDef`s (below); emits the richly-typed events
  that make agent triggers reliable; renders agents legibly with provenance.
- **Durable-workflow (ADR-09).** Backs **auto-merge-when-green** waits, the **merge queue**, and
  **agent-PR HITL gates** (surfaced as Chat approval cards).
- **OLAP (ADR-10).** PR throughput / review-latency / merge-frequency analytics fed off the bus.

### 7.4 Agent tools & triggers it registers (ADR-08)
Typed `ToolDef`s into the shared `ToolSurface` (name + JSON-schema input + required caps + effect
kind + side-effecting flag), governed once and exposable over MCP later:
`git.open_pr`, `git.submit_review`, `git.comment`, `git.resolve_thread`, `git.suggest_change`,
`git.merge` (**side-effecting, sensitive on protected refs → HITL-gateable**), `git.read_diff`,
`git.read_file`, `git.search_code` (read-only). Triggers like *"on `pr.opened` matching pattern,
dispatch agent review"* are first-class via the shared trigger engine (`EventMatcher` over the
query AST, ADR-07), not bolt-on webhooks. **Agents are subject to branch protection like any
principal** — an agent cannot bypass required human approval unless policy allows (`git-hosting.md
§7.3`).

### 7.5 PersonalDataHolder duties (ADR-12) — the hardest in the platform
Git hosting registers as a `PersonalDataHolder` implementing `locate/export/rectify/restrict/
erase` over **commit author/committer name+email (baked into commit hashes), file content, commit
messages, PR/review/comment text, LFS blobs, push records / SSH-key fingerprints, and the git-
identity↔Myelin-user mapping** (`git-hosting.md §9.1`). The platform's **references-not-payloads +
pseudonymous-identity + crypto-shred** spine (ADR-12) is what makes this tractable: commit objects
and event payloads carry **pseudonymous identities** whose erasable mapping lives outside the
immutable store, so erasure = destroy the pseudonym mapping + crypto-shred per-subject/tenant keys
+ tombstone refs in Refs, **without rewriting immutable history** (`system-overview.md §8.3`). The
residual — personal data *inside* file content / history that was never pseudonymised — needs the
destructive, audited history-rewrite tool (with fork/mirror/CDN invalidation) and is documented as
a **known best-effort limit** (`git-hosting.md §9.2`; **[OPEN — LEGAL]** GD-1/GD-2). **Residency**
(ADR-11): objects, LFS, metadata, indices, event payloads, backups, **all replicas/mirrors/CDN
clone caches** stay in-region; push-mirroring to a foreign host is a residency boundary crossing
and is **policy-gated** (`git-hosting.md §9.3, §6`).

---

## 8. Changes this implies are needed in the shared systems (flag for Phase 3)

These are git-hosting-driven requirements the Phase-3 shared-systems agents must satisfy. None
contradict the spine; several sharpen open items already in ADR-15.

1. **Reference Graph must address sub-artifact granularity for git** — a PR comment, a diff
   position, a `blob#Lnn`, a commit — and resolve them in the projection API (ADR-13 names the
   requirement; the git-specific id grammar and **outdated/tombstoned-anchor** semantics feed P3).
2. **Event Bus per-ref ordering + outbox at git-push throughput.** The outbox helper in
   `myelin-events` must sustain push QPS without lost/ghost events and preserve **per-ref**
   ordering (a stricter aggregate than "per-PR") — flag for the P3 partitioning design (ADR-04;
   `git-hosting.md §7, §11`).
3. **ReBAC must express branch-/ref-pattern-scoped relations and CODEOWNERS-as-relations**
   efficiently (relation by ref glob), and answer `list-objects` for repo/PR lists at scale
   (ADR-03; P3 tuple schema). **Agent-vs-human merge policy** rides the delegation algebra (AG-2).
4. **Storage tier needs first-class git constructs**: content-addressed pack/LFS blobs, **clone/
   bundle artifacts as a cached, residency-pinned, CDN-within-EU distributable** (hot-repo/clone-
   storm), and **crypto-shred granularity** fine enough for per-subject erasure of git-borne PII
   (ADR-10/12; GD-4).
5. **KMS/crypto-shred must cover immutable git objects and reflogs/bitmaps/backups** — erasure
   must reach replicas, reflogs, packs, bitmaps, backups, mirrors, and CDN caches with defined
   SLAs (the genuinely hard distributed-erasure problem, `git-hosting.md §9.2, §11`).
6. **Durable-workflow must model the merge queue and auto-merge-when-green** as first-class
   durable state machines, and agent-PR HITL gates as durable signals (ADR-09).
7. **Search must support code-shaped indexing inputs** (trigram/suffix lexical now; symbol/SCIP
   later), incremental on push, ACL-aware via `list-objects` (ADR-03/10; TE-27).
8. **Identity must own SSH-pubkey→Principal and deploy-key/token/app-installation machine
   identities**, with the git front door as the consumer (`git-hosting.md §8, §10.1`).

---

## 9. Scale hot-spots & open questions for Phase 4

Per ADR-11, the per-subsystem scale hot-spots are owned by this subsystem's Phase-4 agent. The
hardest problems (ranked, `git-hosting.md §11`) and the explicit open questions (`§12`, ADR-15):

- **[OPEN → P4, TE-24] Storage/replication backend** — bare repos on replicated FS (Gitaly/Spokes
  style) vs object-store-backed packs (Mononoke/JGit-DFS style); replication/consistency
  mechanism (quorum voting vs primary+WAL vs Raft) giving **linearizable protected-ref merges, no
  split-brain**. Major architecture fork.
- **[OPEN → P4, TE-8] Git core build-vs-embed** — `gix` vs shell-out-to-`git` vs libgit2;
  **re-verify gitoxide server-side maturity and reftable upstream status** (the research is from
  training knowledge, not re-verified, `git-hosting.md §12`).
- **[OPEN → P4, TE-25] Monorepo ambition** — how large a monorepo Myelin supports gracefully via
  partial-clone/sparse/commit-graph/bitmaps; where the "out of scope, use a Google-scale system"
  line is drawn (Myelin almost certainly should **not** build a Mononoke-class system in v1).
- **[OPEN → P4, TE-22] Diff-position / comment anchoring across force-push/rebase** — the primary
  correctness/UX battleground (`git-hosting.md §4.3`).
- **[OPEN → P4, TE-23] SHA-1 vs SHA-256** default object format + migration story.
- **[OPEN → P4, TE-26] Forks (shared object storage via alternates vs independent copies — storage
  win vs erasure/residency complexity), merge queue in v1?, in-UI conflict resolution / web
  editing scope.**
- **[OPEN → P4, TE-27] Code-search scope for v1** — per-repo lexical only vs cross-repo vs
  semantic/symbol nav; how much rides on CI-produced SCIP/LSIF indices.
- **[OPEN → P4] Multi-tenancy isolation level for git** — row-level vs schema vs physical-per-
  tenant; how residency partitioning maps onto repo sharding (ADR-11 spectrum).
- **[OPEN → P4] Push-policy execution locus** — native git hooks vs an in-process receive-pack
  with embedded policy engine (faster/safer at scale, more to build).
- **[OPEN — LEGAL] Erasure of PII inside immutable git history** (GD-1/GD-2): how much history-
  rewrite tooling to offer; pseudonymised-by-default vs real-name commit identities; how to
  communicate the immutability limit to controllers. Needs counsel/DPO.
- **[OPEN → P4, joint with CI] The Git↔CI checks/commit-status contract** and fork/trust-tier
  signals that gate merges — the most load-bearing cross-subsystem relationship
  (`system-overview.md §4`).

---

## 10. Cross-references
- [`architecture-decisions.md`](../architecture-decisions.md) — ADR-01/02/03/04/05/08/09/10/11/12/13/14.
- [`system-overview.md`](../system-overview.md) — §4 (owns vs delegates), §7 (lifecycles), §8.1
  (PR context pane), §8.2 (agent flagship), §8.3 (DSAR fan-out).
- [`01-research/subsystem-deep-dives/git-hosting.md`](../../01-research/subsystem-deep-dives/git-hosting.md)
  — the deep-dive this builds on (§3 storage scale, §4 views, §6 protocol, §7 events, §9 GDPR,
  §11 hard problems, §12 open questions).
- [`VISION.md`](../../../VISION.md) — non-negotiables (world-scale, top-tier UX, agent-native,
  GDPR/EU-sovereign, Rust-default).
