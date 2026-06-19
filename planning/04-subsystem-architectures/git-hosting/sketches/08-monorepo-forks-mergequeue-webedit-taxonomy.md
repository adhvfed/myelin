# Sketch 08 — Monorepo (TE-25), forks/merge-queue/web-edit (TE-26), taxonomy, sub-artifact refs

> Exploration note. The remaining scope decisions bundled: how big a monorepo we support gracefully;
> the fork storage model; whether merge-queue and web-edit ship in v1; the complete `git.*` event
> taxonomy and the stable `#sub` ArtifactRef grammar (both Phase-4 deliverables). Date: 2026-06-19.

## A. Monorepo ambition (TE-25)

**Decision shape: support large-but-normal monorepos via stock-git scaling features; do NOT build a
Mononoke-class system in v1** (Phase-1 §3.2 — "Myelin almost certainly should not"; Phase-2 §9).

- **What we support:** partial clone (`--filter=blob:none`/tree filters), sparse-checkout/sparse-index,
  shallow clone, and **mandatory-for-monorepos** acceleration: commit-graph + reachability bitmaps +
  MIDX kept fresh by scheduled maintenance (sketch 02 — canonical `git maintenance`). Scalar-lineage
  recommended client config. These are all canonical-git features our shell-out path gets for free.
- **Where the line is drawn:** true hyperscale (tens of millions of files, thousands of concurrent
  committers — Google Piper / Meta Sapling+Mononoke scale) is **out of scope for v1**; that is the
  object-backed-packs follow-on (sketch 01) if ever measured-needed. We support "large but normal"
  (the 99.9% case) and say so honestly.
- **Honesty:** exact breaking thresholds (repo size, file count, push QPS) are workload-dependent
  (Phase-1 §3.2) — **benchmarked in the architecture/testing stage, not guessed here.**

## B. Forks — shared object storage vs independent copies (TE-26)

The tension (Phase-1 §3.2/§12, Phase-2 §9): **alternates/shared-object-storage** (forks in a "network"
share packs — a big storage win, GitHub's model) **vs independent copies** (simpler erasure/residency,
more storage).

- **A. Shared object storage (alternates):** forks share the base repo's object store; a fork only
  stores its unique objects. *Pro:* huge storage win (most fork objects are shared). *Con:* **erasure
  + residency complexity** — crypto-shredding/history-rewriting the base must account for forks sharing
  objects; a shared object can't be GC'd while any fork references it; residency must hold across the
  whole network.
- **B. Independent copies:** each fork is a full repo. *Pro:* clean erasure/residency boundary (a fork
  is its own `(tenant, repo_id)` with its own keys/region — the doctrine's tenant-is-everything model).
  *Con:* storage cost (full copy per fork).
- **Leaning: independent copies in v1, with shared-object-storage as a measured follow-on.** The
  doctrine's **tenant-is-the-unit + per-tenant crypto-shred + residency-pinned** model (EI-02 §1;
  storage §5) is *much* cleaner with independent repos — a fork's erasure/residency is self-contained.
  Cross-tenant forks (public OSS) would *require* independence anyway (you can't share an object store
  across residency boundaries). Shared-object-storage **within one tenant** is the storage-optimisation
  follow-on, gated on measured fork-storage pressure. **Named floor.**

## C. Merge queue — v1 or later? (TE-26)

- Merge queue serialises landing onto a busy protected branch: it tests each PR against the *latest*
  target (or a speculative batch) before merging, preventing "green PR breaks main after a concurrent
  merge."
- It's a **durable state machine** — exactly what `myelin-flow` exists for (contract 9.1; Phase-2 §7.3:
  "Durable-workflow backs the merge queue"). The auto-merge-when-green wait is a durable workflow
  (`describe`/`signal` on `ci.run.passed`).
- **Leaning: ship a *simple* merge queue in v1** (single-lane: re-test head-of-queue against latest
  target, merge on green, advance) because (a) the durable-workflow substrate makes the state machine
  cheap, and (b) auto-merge-when-green is already a flagship flow (Phase-2 §6.1). **Speculative/batched
  multi-lane merge queues are the named follow-on** (the busy-monorepo optimisation), promotion-
  triggered by measured queue depth. This keeps v1 honest: linearizable landing + auto-merge, without
  the speculative-execution complexity.

## D. Web editing / in-UI conflict resolution (TE-26)

- **In-UI file edit + commit, suggestions (committable from the PR), and simple conflict resolution**
  are real review-UX wins (Phase-1 §4.2/§4.6).
- These are **write operations through the same push path** (sketch 03) — a web edit is a commit
  authored by the UI principal, going through the same policy gate. **Suggestions** (a reviewer
  proposes a replacement; the author commits it) reduce to "apply this patch as a commit."
- **One editor render path (KN-4 / DL §8b.2):** PR/review/comment bodies use the shared `myelin-content`
  editor (markdown-subset string for inline, structured `mention`/`artifact_ref` nodes). **Web file
  editing is NOT the rich-text editor** — it's a code editor surface (syntax-highlighted, plain text),
  a separate concern; do not conflate. The *comment/description* editor is the shared one.
- **Leaning: v1 ships** single-file web edit→commit + **suggestions** (committable) + the shared rich
  editor for comments/descriptions. **In-UI 3-way conflict resolution is the named follow-on** (it's a
  fiddly merge-UX surface; the floor is "conflict indicated, resolve locally").

## E. The complete `git.*` event taxonomy (Phase-4 deliverable — Bus §6.3 seed extended)

Grammar: `git.<artifact_type>.<event_name>`, singular, past-tense (Bus §6.1). The full list we own:

```
# repo lifecycle
git.repo.created | deleted | visibility_changed | transferred | archived | unarchived
git.repo.fork_created
# refs / push (aggregate = git/ref/<repo>:<ref>)
git.ref.updated            # the core push event (old_sha,new_sha,forced?,commit_shas,pusher)
git.branch.created | deleted | protection_changed
git.tag.created | deleted
# pull requests (aggregate = git/pr/<id>)
git.pr.opened | updated | marked_ready | closed | reopened | merged | synchronized
git.pr.review_requested | review_submitted | review_dismissed
git.pr.comment_created | comment_resolved | thread_resolved
git.pr.check_required_failed | merge_blocked | merge_queued | merge_queue_advanced
git.pr.codeowners_review_required
# policy / audit
git.protection.bypass_used         # audit-critical
git.repo.history_rewritten         # erasure/redaction admin op (sketch 09) — audit-critical
# cross-cutting (platform-wide)
git.*.erased   (tombstone)         git.*.snapshot  (reindex-from-source / replay)
```
All carry the canonical envelope (contract 2.1); `git.ref.updated` is per-ref ordered (sketch 03/04);
`git.pr.*` is per-PR ordered (aggregate = the PR).

## F. Sub-artifact `#sub` ArtifactRef grammar (Phase-4 deliverable — contract 5.7)

Stable opaque sub-ids, stable across edits so embeds don't dangle (Refs §3.5):

```
myelin://<tenant>/git/repo/<repo_id>
myelin://<tenant>/git/pr/<pr_id>
myelin://<tenant>/git/pr/<pr_id>#comment-<comment_id>     # comment_id = stable opaque mint (sketch 07)
myelin://<tenant>/git/pr/<pr_id>#review-<review_id>
myelin://<tenant>/git/commit/<sha>                        # repo-scoped via repo in path? → see below
myelin://<tenant>/git/blob/<repo_id>/<path>#L<n>[-L<m>]   # line-range anchor (resolves to blob@ref)
myelin://<tenant>/git/branch/<repo_id>:<ref_name>
```
- **commit/blob need the repo in scope** (a sha is repo-local) — we mint `git/commit/<repo_id>:<sha>`
  and `git/blob/<repo_id>/<path>` so the ref is self-contained (Refs rejects scope-less refs, REF-3).
  The architecture stage finalises whether `<type>/<id>` packs repo+sha or uses a `#sub`; the **id is
  always stable and scope-complete**.
- **Line-range anchors** (`#L42-L88`) resolve per-viewer to the blob at a ref and **tombstone** if the
  blob is erased (graceful degrade, never a dangling leak — Refs tombstoning).

## Leaning (committed in findings)

Monorepo: **stock-git scaling features (partial/sparse/shallow + mandatory commit-graph/bitmaps/MIDX),
no Mononoke v1.** Forks: **independent copies in v1** (clean erasure/residency), shared-object-storage
a measured follow-on. Merge queue: **simple single-lane in v1** on `myelin-flow`, speculative multi-lane
follow-on. Web edit: **single-file edit→commit + suggestions + shared rich editor for comments** in v1,
in-UI conflict resolution follow-on. Full **`git.*` taxonomy** and **scope-complete stable `#sub`
grammar** as above (finalised in the architecture stage).

## Prior art / sources

- Partial clone / sparse-index / commit-graph / bitmaps / Scalar (canonical git; Microsoft Scalar
  lineage). Meta Sapling+Mononoke / Google Piper (the out-of-scope hyperscale line).
- GitHub fork networks / alternates (shared object storage); the storage-vs-erasure trade.
- Merge queue as a durable state machine (Phase-2 §7.3; `myelin-flow` contract 9.1).
- Bus §6.1–6.3 (taxonomy grammar + seed); Refs §3.5 / contract 5.7 (sub-artifact refs, REF-3).
