# Subsystem Deep-Dive: Git Hosting & Code Review

> Phase: `01-research`. This is **research**, not architecture. It maps the territory the
> later architecture phases will build on. It does not commit to a final design. Claims are
> grounded in established Git internals and the public architecture of GitHub, GitLab,
> Gitea/Forgejo, Gerrit, BitBucket, and AWS CodeCommit; where I am unsure I say so
> explicitly (see [§12 Open Questions](#12-open-questions--explicit-uncertainty)).

---

## 1. Purpose & role in Myelin

Git hosting is the **system of record for source code and its history** within Myelin. In
the Myelin model it is one of five subsystems, but it is arguably the *gravitational
centre* of the engineering side: CI is triggered by it, issues reference its commits and
branches, knowledge docs link to its files, and chat references its PRs and diffs. It is
where the cross-artifact reference graph is densest.

Its responsibilities:

- Store Git repositories durably and at world-scale, serving the Git wire protocol over
  SSH and HTTPS (smart-HTTP), plus a read API for browsing.
- Provide code **browsing, blame, and search** over current and historical state.
- Provide the **pull/merge request (PR/MR)** lifecycle: propose change → review →
  approve → merge, with first-class review UX.
- Enforce **branch protection, merge policies, and permissions** as the gatekeeper to
  protected refs (the place "what is allowed to land" is decided).
- **Emit events** for every meaningful change (push, PR opened, review submitted, merge,
  tag) onto the shared event bus, and **consume** events (e.g. CI status, agent actions)
  so the platform and agents react in real time.
- Be **agent-native**: agents are first-class actors that can open PRs, review, comment,
  and (policy permitting) merge — via the same mechanisms as humans, mediated by the
  strategy-pattern agent fabric.

What it is **not** responsible for (delegated to other subsystems / shared systems):
identity & authentication (shared identity/access), running builds (CI), durable object
storage primitives (shared storage), the global search index plumbing (shared search —
though git hosting owns *what* it indexes), and notification delivery (shared
notifications). Git hosting owns the git-specific logic and contributes to those shared
systems.

### Positioning note (vs. competitors)

The bar is GitHub/GitLab-class review UX with Gerrit-class policy rigor available when
wanted, on EU-sovereign infrastructure, with agents as first-class reviewers/authors. The
*differentiator is not the git server* — every competitor has a competent one — it is the
**unified identity/permission/event/reference fabric** and **agent-nativeness**. The
research below therefore treats the raw git-serving layer as "table stakes done well" and
spends proportionally more on review UX, policy, events, and scale.

---

## 2. Core domain concepts & data-model considerations

### 2.1 Git's own object model (the substrate)

Git is a content-addressable object store. Understanding it is load-bearing for every
scaling and erasure decision below.

- **Objects**, addressed by hash: **blob** (file content), **tree** (directory listing →
  blobs/trees + names + modes), **commit** (tree + parents + author/committer +
  message), **tag** (annotated tag → object + message). Immutable; identity = hash of
  content.
- **Hash algorithm**: historically SHA-1 (160-bit); SHA-256 is specified and implemented
  in Git but **interop/transition is still immature** ecosystem-wide. *Decision deferred
  to architecture; flagged as a strategic risk (§12).* Choosing SHA-256 now buys
  collision-resistance and future-proofing but risks tooling/client incompatibility;
  staying SHA-1 matches the ecosystem but inherits SHAttered-class weaknesses.
- **Refs**: mutable named pointers (branches `refs/heads/*`, tags `refs/tags/*`, plus
  server-side namespaces like `refs/pull/*`, `refs/merge-requests/*`, `refs/notes/*`,
  `refs/keep/*`). Stored loose or **packed** (`packed-refs`); at scale, a **reftable**
  (Gerrit/JGit format, now in upstream git) or a database-backed ref store is preferable
  to filesystem-backed refs because millions of refs make loose-ref directories pathological.
- **Packfiles**: objects compressed with zlib + **delta compression** (objects stored as
  deltas against similar objects), plus a **pack index** (`.idx`) and increasingly a
  **multi-pack-index (MIDX)**, **reachability bitmaps**, and **commit-graph** files for
  fast traversal. This is the core of efficient storage and transfer.
- **The reachability problem**: almost every expensive operation (fetch negotiation, GC,
  "what's the diff", "is this commit on a protected branch") is a graph reachability query
  over the commit DAG. Bitmaps + commit-graph exist precisely to accelerate this. World-scale
  hosting lives or dies on making reachability cheap.

### 2.2 Hosting-level domain entities (the layer Myelin owns)

These are *not* in git itself; they are the host's database:

- **Repository**: belongs to an owner (user or org/namespace); has visibility
  (private/internal/public — note "internal" only makes sense in a tenant/org context),
  default branch, settings, on-disk/storage location, and a tenant binding.
- **Fork / network**: a repository derived from another; forks within a "network" can
  share object storage (GitHub's alternates/forks-share-storage model) — important for
  storage economics and for PR-from-fork flows.
- **Pull/Merge Request**: source ref → target ref proposal; carries title, description,
  status (open/merged/closed/draft), reviewers, required checks, merge method, linked
  issues, the diff, and a **review thread model**.
- **Review**: a reviewer's verdict (approve / request-changes / comment) plus inline
  **comments** anchored to (file, line, side, commit/diff-position) — anchoring is a
  notoriously hard data-modeling problem (see §4.3).
- **Comment threads**: resolvable, possibly nested, anchored to a diff position that must
  survive rebases/force-pushes (outdated vs. current).
- **Branch protection / ruleset**: predicate + required approvals + required checks +
  who-can-bypass, scoped by ref pattern.
- **Webhook / subscription**: external or internal consumer registration (largely
  subsumed by the shared event bus in Myelin, see §7).
- **Deploy key / SSH key / access token / app installation**: credentials and machine
  identities (mostly delegated to shared identity, but git hosting needs the binding).
- **CODEOWNERS**: path → required-reviewer mapping; influences who must review.
- **Commit status / check run**: per-commit CI/check results (produced by CI, consumed
  here for merge gating).

### 2.3 Data-model decisions that matter

- **Where do refs live?** Filesystem `packed-refs` does not scale to huge ref counts or
  high write concurrency; reftable or a transactional ref DB is the scalable path. This
  intersects with replication (§6).
- **Where do PR/review/comment metadata live?** Relational/transactional store
  (Postgres-class), *not* in git. Comments anchored to diff positions need a model robust
  to history rewrites.
- **Multi-tenancy in the model.** Every entity carries a tenant/residency tag from day 1
  (a Vision non-negotiable). Repos in different tenants must be isolatable for residency
  and erasure. *Whether tenancy is row-level, schema-level, or hard physical isolation per
  tenant is an architecture decision (§12).*
- **Reference graph contribution.** Commits, branches, PRs, files, and review comments are
  all addressable artifacts that the cross-artifact reference graph must be able to point
  at and resolve. Git hosting must expose stable, resolvable identifiers (e.g.
  `myelin://repo/<id>/commit/<sha>`, `.../pr/<n>#comment-<id>`).

---

## 3. Repository storage at scale

### 3.1 Single-repo physics

- A repo's cost is dominated by packfiles. **Repacking/GC** is periodic, CPU/IO-heavy, and
  must be done without blocking serving (git supports concurrent access during repack, but
  it's resource-intensive). At scale you need scheduled, rate-limited, possibly
  off-peak maintenance, and **incremental/geometric repacking** rather than full repacks.
- **`git gc` / `git maintenance`** semantics: loose object explosion from many small pushes
  is a real operational hazard; the host must aggressively pack and prune unreachable
  objects — but pruning interacts with erasure and with in-flight operations (grace
  periods, `refs/keep`).
- **Reachability bitmaps + commit-graph** must be kept fresh for hot repos; stale
  acceleration structures silently degrade fetch/clone latency.

### 3.2 Monorepo vs. many-repos

This is one of the defining strategic questions and Myelin should *support both well*:

**Many-repos (polyrepo)** — the common case. Challenges are *aggregate* scale: millions of
repos, long-tail cold repos, fork networks, fan-out of events, cross-repo search and
references. Storage strategy = shard repos across many nodes; most repos are tiny and cold.

**Monorepo** — a single enormous repo (Google/Meta-scale: tens of millions of files,
deep history, thousands of concurrent committers). Stresses git in ways vanilla git was
never designed for. Mitigations the industry uses:

- **Partial clone** (`--filter=blob:none`, blob/tree filters): client fetches commits/trees
  but lazily fetches blobs on demand. Requires server support and a robust on-demand fetch
  path. Now mainstream (GitHub, GitLab).
- **Sparse checkout / sparse-index**: client materializes only part of the tree.
- **Shallow clone** (`--depth`): truncated history; cheaper but limits operations.
- **Scalar / VFS-for-Git lineage** (Microsoft): tooling to make monorepos usable; Scalar
  is now partly upstreamed (background maintenance, recommended config).
- **Commit-graph + bitmaps are mandatory**, not optional, for monorepos.
- True hyperscale monorepos eventually outgrow stock git entirely (Google's **Piper**,
  Meta's **Sapling/EdenFS + Mononoke** — a Rust(!) virtual git/hg server backed by
  scalable storage). *Myelin almost certainly should not build a Mononoke-class system in
  v1; it should support large-but-normal monorepos via partial clone/sparse/commit-graph
  and treat true Google-scale as out-of-scope-for-now (§12).* The Rust steer in the Vision
  is notable — Mononoke is the existence proof that a Rust scalable git server is feasible.

> **Uncertainty:** the exact thresholds at which stock-git-on-a-shard stops working
> (repo size, file count, push QPS) are workload-dependent. I won't guess hard numbers;
> the architecture phase should benchmark.

### 3.3 LFS & large files

Git is bad at large binaries (every version stored whole-ish, bloating packs and clones).

- **Git LFS**: pointer files in git; actual bytes in a separate object store via the LFS
  batch API (HTTP). The bytes naturally belong in the **shared storage** subsystem
  (object store with EU residency). LFS objects need their own GC, dedup, and erasure path.
- **Alternatives/adjacent**: `git-annex`, partial-clone-as-LFS-replacement (large blobs
  fetched lazily), and content-defined chunking/dedup for huge assets. *Whether to support
  LFS, partial-clone-only, or both is an architecture decision.*
- LFS interacts with **erasure** (large binary may contain personal data / proprietary
  assets) and **residency** (the bytes must sit in the right region).

### 3.4 Sharding, replication, pack storage at world-scale

This is the heart of "world-scalable from day 1." Established patterns:

- **Repo-level sharding**: a repository is the unit of placement. A directory/placement
  service maps `repo_id → shard/storage node(s)`. (GitHub historically: DGit/Spokes —
  repos replicated across ≥3 file servers with a consensus-ish replication layer; GitLab:
  **Gitaly** + **Gitaly Cluster / Praefect** for replication; Gerrit/Google: JGit +
  scalable backends.)
- **Replication for HA + read-scaling**: N replicas per repo; writes go to a primary (or
  via consensus), reads can be served by replicas. Must handle **consistency**: a fetch
  must not see a half-applied push; a PR-merge must be linearizable on the target ref.
- **Voting/quorum replication** (GitHub Spokes-style three-way voting / Praefect's
  primary+replica with WAL-ish replication) to avoid split-brain on ref updates.
- **Pack storage backends**: stock = bare repos on local disk (XFS/ext4). At scale, options
  include network/replicated block, or object-store-backed pack storage (Mononoke,
  JGit-DFS / Gerrit's pluggable backends store packs as blobs in a KV/object store keyed
  by content). Object-store-backed packs decouple compute from storage and ease
  replication/residency, at a latency/complexity cost.
- **Hot-repo problem**: a handful of repos (large OSS-style, or busy monorepos) get
  disproportionate traffic. Need per-repo replica counts, caching of clone bundles, and
  CDN-style distribution of static clone artifacts (**clone/bundle URIs** — git supports
  serving a precomputed bundle then incremental fetch).
- **Geo-distribution / residency**: replicas may be pinned to regions for residency
  (EU-only) *and* for latency (read replicas near users). These two goals can conflict
  (latency wants global replicas; residency forbids leaving the EU). Myelin's residency
  constraint likely **forecloses non-EU replicas** for EU-tenant data — a deliberate
  trade-off vs. global latency. (§9, §12.)
- **Connection/routing layer**: an SSH/HTTPS front door that authenticates, looks up
  placement, and routes/proxies the git transaction to the right backend (cf. GitLab
  Workhorse + Gitaly, GitHub's babeld/ git proxy). Must stream large transfers efficiently
  (don't buffer whole packs in memory).

---

## 4. Key UX / views required

The Vision demands top-tier UX. The views below are table stakes plus Myelin-specific
agent/cross-artifact affordances.

### 4.1 Repository & code browsing

- **Repo home**: README render, branch/tag switcher, language/size stats, default-branch
  file tree, quick actions (clone URL, open in CLI, create branch/PR).
- **File tree & file view**: fast directory navigation; syntax-highlighted file view with
  permalink by commit SHA; **blame** view (who/when per line, with "ignore-rev" support for
  reformatting commits); raw view; image/binary/LFS-aware rendering; large-file graceful
  degradation.
- **History / commit views**: commit list per path, commit detail (diff, parents, signed
  status), the commit DAG visualization (lightweight; full DAG render is expensive).
- **Compare view**: arbitrary ref/SHA-to-ref/SHA diff.
- **Code search** (see §4.5).

### 4.2 Pull/Merge Request — the centrepiece

Review UX is where products win or lose. Required:

- **PR overview**: description, linked issues (cross-artifact), participants, status,
  required checks summary, merge readiness, timeline of events.
- **Diff/Files-changed view**: the hard one. Needs: unified + split diff, syntax
  highlighting, **per-file collapse/viewed state**, whitespace toggles, rename/move
  detection, large-diff virtualization, intra-line (word-level) diff, and "expand context."
- **Inline commenting & threads**: comment on a line/range/file; resolvable threads;
  **suggestions** (proposed replacement that can be committed from the UI);
  **multi-comment review batching** (start review → batch comments → submit verdict, à la
  GitHub) — strongly preferred over fire-and-forget comments for review quality.
- **Review verdicts**: approve / request changes / comment; required-reviewer and
  CODEOWNERS surfacing; "who must still approve."
- **Commit-by-commit review** + **incremental review** ("changes since you last reviewed")
  — the latter requires robust diff-position tracking across force-pushes/rebases (§4.3).
- **Merge UX**: merge / squash / rebase methods, edit commit message, auto-merge-when-green,
  merge queue surfacing, conflict indication + (ideally) in-UI conflict resolution for
  simple cases.
- **Checks/CI integration panel**: live status from CI events, required vs. optional,
  re-run, logs deep-link.
- **Agent-aware review surface**: a *first-class*, visually distinct representation of
  agent reviewers/authors. Agents leaving review comments, requesting changes, or opening
  PRs must be legible as agents (provenance, which agent, why), not disguised as humans —
  this is both UX and a trust/compliance requirement. Humans must be able to request an
  agent review, dismiss/override it, and see the audit trail.

### 4.3 The diff-anchoring problem (deep, deserves a callout)

Anchoring a comment to "line 42 of file X on diff Y" is fragile: force-push, rebase,
amended commits, and base-branch movement all invalidate naive line numbers. Approaches:
store (blob SHA + line) and/or (commit + path + line + side) and recompute position via
diff mapping; mark threads "outdated" when the anchor no longer exists; offer "show in
original context." This is a primary correctness/UX battleground and a place competitors
visibly differ in quality. *Flagged as a hard problem for the architecture phase.*

### 4.4 Branch protection, settings, policy UX

- Ruleset editor: ref patterns, required approvals (count, from CODEOWNERS, dismiss-stale),
  required checks, linear-history/signed-commits requirements, force-push/deletion bans,
  bypass lists, and *agent-specific* rules (e.g. "agent PRs require human approval").
- Repo/org settings: collaborators & teams, visibility, default branch, merge methods
  allowed, webhooks/event subscriptions, keys/tokens.

### 4.5 Code search & navigation

- **Search**: lexical (regex/substring across repos, GitHub's Blackbird-class trigram
  index) and ideally **semantic/symbol search** ("go to definition / find references"
  across the repo, à la GitHub code navigation / Sourcegraph). This is its own large
  subsystem and leans heavily on **shared search**. Distinctions:
  - *Content search* (find text/regex across files & repos at scale) — trigram or suffix
    indices; massive index size; incremental updates on push.
  - *Code intelligence* (symbols, defs, refs, hover) — needs per-language analysis
    (tree-sitter / LSIF / SCIP indexes), produced ideally by CI and consumed here.
- Honest scope note: **world-scale code search is itself a multi-year engineering effort**
  (GitHub rebuilt theirs as Blackbird; Sourcegraph is a whole company). v1 likely ships
  per-repo / per-tenant lexical search and defers global semantic search. (§12.)

### 4.6 Cross-cutting UX

- Keyboard-first navigation, deep-linkable permalinks for every artifact (for chat/issue
  references), accessibility, mobile-reasonable read views, and **real-time updates**
  (PR/review state changes pushed live via the event bus → UI).

---

## 5. CLI commands expected

Two CLI surfaces matter: (a) **plain `git`** over the wire (must "just work" with stock
clients), and (b) a **Myelin CLI** for hosting-level operations.

### 5.1 Plain git (server must support)

```
git clone <ssh|https-url>
git clone --filter=blob:none / --depth=N / --sparse   # partial/shallow/sparse
git fetch / pull / push
git push --force-with-lease                            # protections may reject
git lfs clone / pull / push                            # if LFS supported
git bundle / clone from bundle URI                     # accelerated clone
```
The server implements the smart-HTTP and SSH endpoints these drive (see §6).

### 5.2 Myelin CLI (`myelin …` — illustrative, names TBD in architecture)

```
myelin auth login                         # device/OAuth via shared identity
myelin repo create / clone / fork / list / view / delete / transfer
myelin repo settings ...                  # visibility, default branch, merge methods
myelin pr create [--draft] [--base] [--head] [--reviewer ...] [--agent-review]
myelin pr list / view / checkout / diff
myelin pr review --approve|--request-changes|--comment [--inline file:line]
myelin pr merge [--squash|--rebase|--merge] [--auto]
myelin pr ready                           # un-draft
myelin branch protect <pattern> --require-approvals N --require-check ...
myelin codeowners validate
myelin key add / token create             # delegated to shared identity
myelin search code "<query>" [--repo ...]
myelin webhook|subscription add ...       # likely "event subscription" in Myelin terms
myelin agent review request <pr> --agent <name>   # invoke an (mock→real) agent reviewer
```
Design intent: the CLI is a thin client over the same APIs the UI and agents use (one
API surface, three consumers: human-UI, human-CLI, agents).

---

## 6. CLI & git protocol (ssh / https / smart-http) details

- **Smart-HTTP**: `GET /info/refs?service=git-upload-pack|git-receive-pack` then
  `POST /git-upload-pack` (fetch) / `POST /git-receive-pack` (push). Auth via
  HTTPS + token/basic (token from shared identity). Supports **protocol v2**
  (ref filtering, server-side filtering, better for huge ref counts — should be the
  default). Stateless-rpc; the front door must stream, not buffer.
- **SSH**: client runs `ssh git@host git-upload-pack '<repo>'`; the server authenticates
  via **SSH public key → user identity** (a custom authorized-keys lookup / `AuthorizedKeysCommand`
  or an in-process SSH server). Then invokes upload-pack/receive-pack against the placed
  repo. Key management lives in shared identity but the SSH front door is git-hosting's.
- **Git protocol (anonymous `git://`)**: insecure, generally **not** offered.
- **Hooks**: `pre-receive` / `update` / `post-receive` are where push-time policy and
  event emission happen server-side:
  - *Pre-receive/update*: enforce branch protection, push limits, secret scanning,
    signed-commit/ DCO checks, file-size limits, agent-vs-human rules — **reject before the
    ref moves**. Must be fast (blocks the push) and run sandboxed.
  - *Post-receive*: emit push events to the event bus, trigger CI, update search index,
    update reference graph. Asynchronous; failures here must not corrupt the push but must
    be retried (outbox pattern, see §7).
- **LFS endpoints**: `POST /<repo>/info/lfs/objects/batch` + upload/download URLs to shared
  object storage.
- **Routing/placement**: every protocol entrypoint must resolve `repo → backend node(s)`,
  enforce authz (shared identity), respect residency (route only to in-region backends),
  and proxy/stream the transaction. Backpressure and rate-limiting live here.
- **Mirroring**:
  - *Pull mirror*: Myelin periodically fetches from an external remote (import / keep in
    sync). Needs credentials, scheduling, conflict policy.
  - *Push mirror*: Myelin pushes to an external remote on update (e.g. legacy GitHub).
  - *Internal mirroring*: distinct from the HA replication of §3 — that is internal
    consistency machinery; mirroring is user-facing sync with foreign hosts. Both must
    respect residency (a push-mirror to a non-EU host is a **residency/erasure leak** and
    likely must be policy-gated, §9).

---

## 7. Events this subsystem EMITs and CONSUMEs

Agent-nativeness requires first-class, well-typed events. The push hook → outbox →
event-bus path is the spine.

### 7.1 Events EMITTED (non-exhaustive, names illustrative)

- `repo.created` / `repo.deleted` / `repo.visibility_changed` / `repo.transferred` /
  `repo.archived`
- `repo.fork.created`
- `branch.created` / `branch.deleted` / `branch.protection_changed`
- `ref.updated` (the core push event: repo, ref, old_sha, new_sha, pusher, commits,
  forced?) — *one logical event with rich payload; CI, search, reference graph, agents all
  consume it*
- `tag.created` / `tag.deleted`
- `commit.pushed` (per-commit, or derivable from `ref.updated`)
- `pr.opened` / `pr.updated` / `pr.ready_for_review` / `pr.closed` / `pr.reopened` /
  `pr.merged` / `pr.synchronized` (head moved)
- `pr.review.requested` / `pr.review.submitted` (approve/request_changes/comment) /
  `pr.review.dismissed`
- `pr.comment.created` / `pr.comment.resolved` / `pr.thread.resolved`
- `pr.check.required_failed` / `pr.merge_blocked` / `pr.merge_queued`
- `codeowners.review_required`
- `protection.bypass_used` (audit-critical)
- `key.added` / `token.created` (may belong to shared identity; emitted/echoed here)

**Design requirements for emitted events:** stable schema + versioning; tenant + residency
tag on every event; actor identity *including human-vs-agent provenance*; idempotency keys;
**transactional outbox** so an event is emitted iff the underlying change committed (no
lost/ghost events on a busy git server). Ordering guarantees per-ref matter (consumers
need to know `ref.updated` order for a given ref).

### 7.2 Events CONSUMED

- **From CI**: `check.run.started/completed`, `pipeline.status` → update PR check status,
  gate/unblock merge, drive merge queue.
- **From agent fabric**: agent requests to open PR / submit review / comment / merge
  (mediated, policy-checked) — agents act *through* the same APIs but their *intents* may
  arrive as events. (Strategy pattern: a `Reviewer` / `Author` trait with mock + real
  impls; git hosting calls the interface, never a concrete agent.)
- **From issue tracker**: issue closed/linked → reflect "closes #123" linkage; possibly
  auto-link PRs to issues.
- **From identity/access**: permission changes, user deletion (→ erasure flow),
  team/membership changes (→ recompute who-can-review/merge).
- **From knowledge/chat**: reference-created events (a doc/chat now references a commit/PR)
  → reference graph backlinks (mostly handled by shared reference graph, but git hosting may
  surface "referenced by").
- **From notifications/search**: mostly downstream of emits; git hosting may consume index
  ack/lag signals.

### 7.3 Agent-native specifics

- Triggers: "on `pr.opened` matching pattern, dispatch agent review" must be a
  declarative, first-class capability (not a bolt-on webhook). This likely lives partly in
  the agent fabric, but git hosting must emit the precise, richly-typed events that make
  such triggers reliable, and must expose the *acting-as-agent* path for the resulting
  actions with full provenance + policy enforcement (agents subject to branch protection
  like anyone — e.g. an agent cannot bypass required human approval unless policy allows).

---

## 8. Permissions model

- **Delegated to shared identity/access** for *who a principal is* and org/team
  structure; git hosting owns *git-specific authorization decisions*.
- Principals: users, teams/orgs, machine identities (deploy keys, tokens, app
  installations), and **agents** (first-class principals).
- Resource scopes: org/namespace → repo → branch (via protection rules) → action
  (read/clone, push, create-PR, review, approve, merge, admin, manage-protections,
  delete).
- **Visibility tiers**: private / internal (tenant-wide) / public — interacts with
  residency (public-but-EU-only is a real, slightly awkward combination).
- **CODEOWNERS** as a permission-adjacent construct (who *must* approve paths).
- **Branch protection bypass** must be auditable (emit `protection.bypass_used`).
- **Agent permissions**: agents get explicit, least-privilege scopes; "can comment" ≠ "can
  approve" ≠ "can merge." Policy can require human-in-the-loop for agent merges. This is
  both a security and a GDPR-accountability matter (automated decision-making transparency).
- Authorization must be enforced at **every** entrypoint: SSH, HTTPS git, API, UI, CLI,
  and the event-triggered agent path — not just the UI.

---

## 9. GDPR / erasure considerations (subsystem-specific)

Git hosting is one of the **hardest** subsystems for GDPR because **git history is
designed to be immutable and content-addressed**, which is in direct tension with the
right to erasure. This deserves serious, honest treatment.

### 9.1 What personal data lives here

- **Commit author/committer name + email** (baked into commit objects → into the commit
  hash). Cannot be changed without rewriting history (which changes all descendant hashes).
- **Personal data in file content, commit messages, PR/review text, comments** — may
  contain names, emails, secrets, or third-party personal data.
- **LFS blobs** may contain personal data (datasets, images of people, etc.).
- **Audit logs, IP addresses, SSH key fingerprints, push records** — operational personal
  data.
- **Mapping of git author identity → Myelin user** (the linkage itself).

### 9.2 The erasure tension and known mitigations

- **Erasing a Myelin user** can detach/anonymize the *hosting-layer* identity (PR
  comments, reviews, the author↔user mapping) relatively cleanly. The hard part is the
  **git object graph**.
- Erasing personal data *inside git history* requires **history rewrite** (filter-repo /
  filter-branch style), which:
  - changes every descendant commit hash → breaks clones, forks, mirrors, references,
    signatures, and any external copy;
  - is effectively impossible to guarantee across all forks/mirrors and user clones;
  - conflicts with immutability and reproducibility expectations.
- Realistic strategy (to be decided in architecture, flagged §12):
  - **Pseudonymization at write time**: map commit author email/name to a stable
    pseudonymous identity where lawful, keeping the real identity only in the
    re-identification table that *can* be erased — but the value committed into the object
    is then non-personal. (Caveat: developers expect their real name in `git log`; this is
    a product/legal tension.)
  - **Redaction tooling**: support history rewrite for a repo as an explicit, audited,
    destructive admin operation (with fork/mirror invalidation), used for secrets/PII
    incidents — accepting it breaks downstream copies.
  - **Tombstoning / quarantine** of objects and **purge from packs on GC** for unreachable
    objects; ensure deleted refs' objects are actually pruned (and not resurrected from
    replicas/bitmaps/reflogs/`refs/keep`).
  - **Reflogs, replicas, backups, bundles, CDN clone caches** are all places a "deleted"
    object can survive — erasure must reach *all* of them with defined SLAs. This is a
    genuinely hard distributed-erasure problem.
- **Legal-basis honesty**: full erasure from immutable VCS history may be **technically
  impossible to fully guarantee**; Myelin must document this limitation, design for
  best-effort + pseudonymization, and surface it to controllers. (This is a known
  industry-wide hard limit — flag to legal/architecture, do not pretend it's solved.)

### 9.3 Residency

- Repo objects, LFS blobs, PR/review metadata, search indices, event payloads, and backups
  must all honor the tenant's residency (EU-only) — including **all replicas, mirrors, CDN
  caches, and search shards**. Residency tags must propagate through every layer (storage,
  events, search). Push-mirroring to foreign hosts is a residency boundary crossing and
  must be policy-controlled.

### 9.4 Auditability

- Every protection bypass, permission change, force-push to protected ref, erasure
  operation, and agent action must be in an immutable audit log (accountability principle).

---

## 10. Dependencies

### 10.1 On shared systems

- **Identity & access**: authentication (SSH keys, tokens, OAuth/device), org/team model,
  user lifecycle (incl. deletion → erasure trigger), machine identities, base authz
  primitives. Git hosting layers git-specific authz on top.
- **Event bus**: the spine for emit/consume (§7); must support ordered, tenant-tagged,
  versioned, idempotent events with a transactional-outbox contract.
- **Agent fabric**: strategy-pattern interfaces for agent authors/reviewers; trigger
  dispatch; provenance/policy enforcement. Git hosting depends on it for the agent-native
  flows but must function with mock agents in development.
- **Storage**: durable object store for LFS blobs, possibly for object-store-backed packs,
  for clone bundles, and for backups — with residency guarantees.
- **Search**: indexing infra for code/content search and PR/comment search; git hosting
  owns the indexing logic + incremental update on push, search owns the index plumbing.
- **Notifications**: review-requested, mentioned, PR-merged, check-failed notifications —
  git hosting emits, notifications delivers.
- **Cross-artifact reference graph**: git hosting both *produces* references (commit
  mentions issue, PR closes issue) and *consumes* backlinks ("this commit is referenced
  by doc X / chat Y"). Needs stable resolvable artifact IDs.

### 10.2 On / for other subsystems

- **CI**: tightest coupling. CI consumes `ref.updated`/`pr.*` to trigger pipelines; git
  hosting consumes `check.*` to gate merges and drive the merge queue. Required-checks
  contract must be jointly designed. CI may also *produce* code-intelligence/SCIP indexes
  consumed by code navigation.
- **Issue tracker**: PR↔issue linking ("closes #N"), auto-close on merge, surfacing linked
  work; cross-references both ways.
- **Knowledge platform**: docs link to files/lines/commits (stable permalinks needed);
  possibly "docs as code" living in repos.
- **Chat**: references to commits/PRs/diffs (rich unfurls); agents+humans discussing a PR
  in a channel; "review this PR" requests originating in chat.

---

## 11. Hardest technical problems for world-scale (summary)

Ranked by my estimate of difficulty/risk:

1. **Distributed, residency-aware erasure of immutable git objects** across replicas,
   reflogs, packs, bitmaps, backups, mirrors, and CDN caches — partly *unsolvable* in the
   strict sense; needs an honest, documented best-effort + pseudonymization design. (§9)
2. **Sharded, replicated, consistent ref updates at scale** (no split-brain; linearizable
   protected-ref merges; quorum/voting replication; reftable/DB ref store). (§3.4)
3. **Monorepo support without building a Mononoke** — partial clone, sparse, commit-graph,
   bitmaps kept fresh; knowing where stock git breaks. (§3.2)
4. **Hot-repo & clone-storm handling** — caching clone bundles, replica fan-out, rate
   limiting, CDN distribution while respecting residency. (§3.4)
5. **World-scale code search & code intelligence** — its own multi-year effort; scope
   carefully for v1. (§4.5)
6. **Diff-position/comment anchoring across rewrites** — correctness + UX. (§4.3)
7. **Transactional outbox at git-push throughput** — never lose/duplicate events feeding
   CI/agents; per-ref ordering. (§7)
8. **SHA-1 vs SHA-256** strategic choice and migration story. (§2.1)
9. **Maintenance (GC/repack/bitmap) at fleet scale** without degrading serving, and
   interacting safely with erasure. (§3.1)
10. **Agent-native action path with provenance + policy** — agents as first-class
    authors/reviewers, legible and bounded. (§4.2, §7.3, §8)

---

## 12. Open questions & explicit uncertainty

Things deferred to architecture, or where I am genuinely unsure:

- **Build vs. embed the git core.** Embed/use **libgit2 / gitoxide (`gix`, Rust) / JGit**,
  shell out to canonical `git`, or build a scalable backend (Mononoke-class)? The Vision's
  Rust steer + `gitoxide` make a Rust path plausible, but `gix` is **not yet feature-complete
  for full server-side serving** (last I am confident about); shelling out to canonical git
  is the pragmatic baseline. *Uncertain — needs current capability assessment in
  architecture phase.*
- **SHA-1 vs SHA-256** as the default object format (interop maturity vs. security).
- **Storage backend**: bare repos on replicated filesystem (Gitaly/Spokes-style) vs.
  object-store-backed packs (Mononoke/JGit-DFS-style). Major architecture fork.
- **Replication/consistency mechanism**: quorum voting vs. primary+WAL replication (Praefect-
  style) vs. consensus (Raft) on ref updates.
- **Monorepo ambition**: how big a monorepo must Myelin support gracefully? Where is the
  "out of scope, use a real Google-scale system" line drawn?
- **LFS vs. partial-clone-native large files** (or both).
- **Multi-tenancy isolation level**: row-level vs. schema vs. physical-per-tenant; how
  residency partitioning maps onto sharding.
- **Code search scope for v1**: per-repo lexical only? cross-repo? semantic/symbol nav?
  How much rides on CI-produced SCIP/LSIF indices.
- **Erasure policy & legal stance**: how much history-rewrite tooling to offer; default to
  pseudonymized commit identities or real names; how to communicate the immutability limit
  to controllers. Needs legal input.
- **Forks & shared object storage**: dedup forks via alternates (storage win, but
  complicates erasure and residency) vs. fully independent copies.
- **Merge queue**: ship in v1 or later? (Strongly affects scale UX for busy repos.)
- **In-UI conflict resolution & web editing**: scope/ambition unclear.
- **Where push-time policy hooks run**: native git hooks vs. an in-process receive-pack
  with embedded policy engine (the latter is faster/safer at scale but more to build).
- **Exact failure thresholds** (repo size, ref count, push QPS) — explicitly NOT guessed;
  must be benchmarked.
- I did **not** independently web-verify every competitor internal detail (Spokes/DGit,
  Blackbird, Mononoke, Praefect, reftable status, gitoxide server-side maturity). These
  are from my own knowledge as of my training; the architecture phase should re-verify
  current facts, especially gitoxide's serving maturity and reftable's upstream status.

---

> _End of research deep-dive. Architecture decisions intentionally deferred; this document
> maps the territory and flags the hard problems and unknowns for `02`/`04`._
