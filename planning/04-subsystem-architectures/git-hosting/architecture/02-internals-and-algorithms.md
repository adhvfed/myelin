# 02 — Internals & Algorithms

> The subsystem-specific algorithms: smart-transport paths (canonical `git` for the wire, Stage-1-verified
> TE-8), the in-process Rust push-policy engine wrapping a sandboxed `receive-pack`, the reftable-on-OLTP
> ref store, replication/consistency (TE-24 — the DB transaction is the linearisation point), GC/repack,
> diff-anchoring across rewrites (TE-22), the merge gate + merge queue, forks (TE-26), monorepo serving
> (TE-25), and the code projection (TE-27). Each hard problem also has a dedicated, prior-art-cited
> write-up in [`05-hard-problems.md`](./05-hard-problems.md); this doc gives the mechanism. Date:
> 2026-06-19.

---

## 1. The git wire protocol (the serving surface)

### 1.1 Smart-HTTP protocol v2 (default)

`GET /<repo>.git/info/refs?service=git-upload-pack` → v2 capability advertisement (the repo's
`object-format`, `fetch`, `ls-refs`, `shallow`, `filter`, `wait-for-done`), then
`POST /<repo>.git/git-upload-pack` (fetch) / `git-receive-pack` (push). **Protocol v2 is the default**
because its server-side `ls-refs` ref-filtering avoids advertising millions of refs on every fetch
(Phase-1 §6) — load-bearing for monorepos and large fork networks. The front door **streams** the
request/response (no whole-pack buffering); negotiation (`want`/`have`) and pack generation run in the
serving tier via **`GitCore::upload_pack`** — in v1 that is **canonical `git` upload-pack, sandboxed +
streamed** (the Stage-1-verified TE-8 position: `gix` has no server-side serving; `01 §2`).

### 1.2 SSH

In-process `russh` server. On connect: extract the offered public key → `Id.authenticate(ssh_pubkey)` →
`Principal` (Id owns the SSH-key→principal map, §10.1 of Phase-2; Id contract 4.1). Then the client's
`git-upload-pack '<repo>'` / `git-receive-pack '<repo>'` command is parsed, `Id.check(principal, pull|
push, repo)` runs, placement is resolved, residency is enforced, and the transaction streams to the
serving tier. In-process (not per-connection `sshd` fork) so connection fan-out under clone-storm is
cheap.

### 1.3 Partial-clone / sparse / shallow (monorepo + large-repo serving)

`upload_pack` honours `filter=blob:none` / `blob:limit=<n>` / `tree:0` (partial clone — client fetches
commits/trees, lazily fetches blobs on a follow-up `fetch` with the wanted OIDs) and `--depth` (shallow).
This is the **monorepo strategy** (TE-25; `05 §HP-3`): support large-but-normal monorepos via
partial-clone + sparse-checkout + a fresh commit-graph + bitmaps, rather than building a virtual
filesystem. Prior art: GitHub/GitLab partial-clone; Microsoft Scalar/VFS-for-Git lineage.

### 1.4 Accelerated clone (hot-repo / clone-storm)

For hot repos the serving tier maintains a **precomputed bundle** in `BlobStore` and advertises a
**bundle-URI** (git's `transfer.bundleURI`): the client fetches the static bundle from the object tier
(CDN-within-EU distributable, residency-pinned) then does an incremental fetch for the delta since the
bundle. This offloads the clone-storm read fan-out from the serving compute to the object tier (Phase-1
§3.4). Bundles are regenerated on a cadence driven by ref-update volume.

---

## 2. The receive-pack path: sandboxed `git` for bytes, in-process Rust for policy + the transaction

Push is the correctness-critical write path. The byte plumbing (pack ingest, negotiation) runs as
**sandboxed canonical `git receive-pack`** into a **quarantine** object dir (the Stage-1-verified TE-8
position — `01 §2`); the **policy decision and the ref-CAS + outbox write run in our in-process Rust**,
all in **one DB transaction** so policy + ref-move + outbox are atomic (BUS-2). The two are not in
tension: `git` does the bytes, *we* own the decision and the transaction.

```
push(repo, packstream, principal):
  1. `git receive-pack` (SANDBOXED: egress-deny, ro-root + tmpfs, caps dropped, capped — ADR-20/OQ-3)
     ingests the pack into a QUARANTINE object dir (git's quarantine model: objects are not yet
     referenced; abort discards them cleanly) and reports the PROPOSED ref updates (old_oid → new_oid).
     The pack-write path runs git's `sha1dc` collision check (the SHA-1 mitigation, `01 §3`).
  2. IN-PROCESS RUST POLICY (the "execution locus" call, below) — for each proposed ref update:
        - Id.check(principal, push | protected_push, repo:ref)   # ReBAC ref-scoped relation
        - ruleset eval: force-push/deletion bans, linear-history, signed-commit,
          required-status (deferred to merge gate for PRs), size limits
        - secret-scan the new objects (regex/entropy; reject before the ref moves)
        - PSEUDONYMITY enforcement (GIT-1, `09`): assert/normalise author identity to the pseudonym
        - agent rule: if principal.kind == agent and ruleset.agent_needs_human → reject direct push
        - REJECT → abort the whole atomic push (all-or-nothing per push), discard quarantine
  3. On accept: migrate quarantined objects into the repo's object DB (via BlobStore).
  4. BEGIN TX:
        UPDATE git_ref ... ; INSERT git_reflog ... ;            # the ref CAS = linearisation point
        OutboxTx::emit(git.ref.updated{repo, ref, old, new, forced, commit_oids, pusher_pseudonym},
                       cause = the push session)                # same tx (BUS-2); aggregate = the ref
     COMMIT.
  5. (async, off the bus) CI triggers, Search code-projection, Refs edges, commit-graph/bitmap refresh.
```

**Execution locus (Phase-2 §9 open item, resolved): in-process Rust policy + transaction, NOT native
shell `pre-receive`/`post-receive` hooks.** Native hooks fork a shell per push and make "policy + outbox
in one transaction" awkward, and a tenant-authored hook script is an arbitrary-code-execution surface. Our
in-process engine (a) keeps the outbox write in the **same DB transaction** as the ref CAS (BUS-2 by
construction); (b) is a typed, testable Rust component, not a hook script; (c) reject-before-the-ref-moves
is guaranteed because the ref CAS is *our* code in step 4, gated by *our* policy in step 2. We do **not**
emit via git's native `post-receive` (that would be a dual write outside the DB transaction — forbidden,
`no-raw-publish`). Tenant-supplied custom checks, if ever offered, run as **CI jobs in the ADR-20
sandbox**, not as hooks (a named non-goal for v1).

---

## 3. The ref store mechanics

Refs are reftable-encoded rows in Postgres (`01 §4.2`). Per-ref atomic compare-and-swap:

```
update_ref(repo, ref, expected_old, new):
  TX: SELECT target_oid FROM git_ref WHERE repo,ref FOR UPDATE;   # row lock = the per-ref lock
      assert target_oid == expected_old  (else: non-fast-forward / lost-update reject)
      UPDATE git_ref SET target_oid=new, update_seq=update_seq+1;
      INSERT git_reflog(...);
      OutboxTx::emit(git.ref.updated, aggregate = repo:ref)       # same TX
  COMMIT
```

The `FOR UPDATE` row lock on the single ref row is the **linearisation point** for that ref. Different
refs of the same repo lock different rows → they advance in parallel (the throughput property), while one
ref is strictly serialised (the per-ref-order property Bus §2.3 requires). `update_seq` becomes the
`outbox.seq` tiebreaker.

---

## 4. Replication & consistency (TE-24) — resolved (Stage-1 committed)

**Decision (Stage-1 committed direction): the linearisation point is the DB ref-store transaction (a
per-ref CAS, `§3`), NOT a bespoke per-repo consensus group. Durability of the bulk pack bytes is a
separate concern, handled by a primary + quorum-ack WAL-streamed replica set behind the `BlobStore` seam.
Consistency (which OID a ref points to) and durability (the pack bytes exist on enough replicas) are
decoupled. v1 is single-cell; cross-cell active sets are a named floor (GF-2).**

### 4.1 Why the DB transaction, not Raft-on-refs / quorum-voting / primary+WAL-only

The hard requirement (Phase-1 §3.4, Phase-2 §3) is **linearizable protected-ref merges with no
split-brain**. The industry menu:

- **GitHub Spokes/DGit** — three-way voting/quorum replication of the *whole* repo.
- **GitLab Gitaly Cluster / Praefect** — primary + WAL-style replication with a coordinator (Praefect is
  the linearisation authority for which replica is canonical).
- **Raft on the ref-update log** — a per-repo consensus group serialising ref CAS through a leader.

**Stage-1 committed to the DB transaction as the linearisation point** — and *not* a bespoke per-repo
consensus group — because **the git subsystem already owns the ref lock and the outbox row commits in the
same transaction (BUS-2)**. That means **"outbox order == ref-update order *by construction*"**: a single
DB CAS (`SELECT … FOR UPDATE` on the ref row, then `UPDATE` + outbox insert, `§3`) gives a linearizable
protected-ref merge *and* the per-ref event ordering *and* exactly-once emit, **in one mechanism we
already have**. Standing up a separate Raft cohort per repo would (a) duplicate a linearisation authority
the DB transaction already provides, (b) add an operational consensus tier to run/upgrade/debug per repo,
and (c) still need the DB for the outbox/metadata anyway — so it buys a second source of truth and a
split-brain *between* the Raft log and the DB. The DB is **the** tiebreaker; we do not introduce a rival.

This is the **Praefect-shaped** position (a metadata authority decides the canonical ref state) collapsed
onto our existing OLTP: **Postgres is the Praefect.** The **bulk bytes (packs) are decoupled** into the
object tier (the Mononoke/JGit-DFS "packs as blobs" insight — the metadata layer is small, the bytes live
in storage), so durability is replicated *separately* from the linearisation, and neither rides the
other.

### 4.2 The mechanism

- **Consistency / linearisation:** the ref CAS in the DB (`§3`). The ref row's `FOR UPDATE` lock is the
  per-ref serialisation point; a protected-ref merge is just a ref CAS on `base_ref` and serialises there.
  No split-brain is possible because there is exactly one authoritative ref state — the committed DB row.
  The hosting OLTP is itself HA-replicated (synchronous quorum-commit Postgres — Storage's OLTP tier),
  so the ref authority survives a node loss with a linearizable failover (the DB's own consensus, not a
  git-specific one).
- **Durability of pack bytes:** a repo's pack/loose bytes are written through `BlobStore` to a **primary
  + quorum-ack WAL-streamed replica set**: a push's object-migration step (`§2` step 3) does not ack
  until the bytes are durable on a write quorum of in-region replicas. Followers serve reads (clone/fetch)
  once they hold the referenced bytes. Because packs are content-addressed, a follower needs only the
  *ref record* (which OIDs are current) plus the bytes — it pulls missing bytes from the shared object
  view, which is what makes repos **relocatable** (STOR-5/GF-1): moving a repo is moving its placement +
  ref records, not copying packs through a consensus group.
- **Recovery consistency (the Stage-1 open item, formalised):** on a replica/node recovery, **the DB ref
  index is the tiebreaker.** A recovering serving node reconciles its on-disk reftable/pack view *to* the
  authoritative `git_ref` rows: any ref tip in the DB whose objects are present is served; any local ref
  ahead of the DB (an un-acked, never-committed push) is discarded (its quarantine never merged, or its
  objects are unreferenced and GC-eligible). Fencing uses the ref `update_seq` as a generation number —
  a node serving a stale `update_seq` for a ref is behind and refreshes before serving a read that needs
  read-your-writes (the `update_seq` is the zookie/fence). **The DB commit is the only thing that ever
  "happened"; everything else reconciles to it.**
- **Residency:** all replicas of an EU-tenant repo are pinned in-region (ADR-11); the control plane
  rejects placement that would put a replica outside the tenant's region. No non-EU replicas, ever.

### 4.3 Honesty / floor

v1 ships **single-cell** placement (primary + in-cell quorum replicas); the "geo read-replica within-EU
for latency" and "cross-cell active set" are GF-2. The object-store-backed pack tier that makes §4.1's
decoupling *fully* real (followers pulling bytes from an object store rather than a primary's blob view)
is GF-1 — v1 may keep packs on local NVMe behind the `BlobStore` trait, with the quorum-WAL replica set
providing durability; the swap to object-store-backed packs is a `BlobStore`-impl change, not a
re-architecture (STOR-5). The exact quorum-ack protocol + failover window is **OQ-4/D-5**.

---

## 5. Diff-anchoring across rewrites (TE-22) — resolved

The problem (Phase-1 §4.3): a comment anchored to "line 42 of file X" must survive force-push, rebase,
amend, and base-branch movement, and degrade legibly when it cannot.

**Decision: store a *content* anchor (blob OID + path + side + line-range + the commit it was created
against), and remap positions across new diffs with an interval-tree line-mapping derived from the
blob-to-blob diff; mark a thread `outdated` (never silently move it wrong) when the anchored content no
longer exists; offer "view in original context."**

### 5.1 The anchor + remap algorithm

```
store anchor: (anchor_blob_oid, path, side, line, line_end, anchored_commit_oid)   # 01 §4.3

on PR head movement (push / force-push / rebase) to new_head:
  for each open comment thread:
    1. Does anchor_blob_oid still exist in new_head's tree at `path` (or a rename target)?
       - If the exact blob OID is still present (content unchanged) → anchor is EXACT; keep position.
    2. Else compute the diff old_blob → new_blob (imara-diff / Myers, 1986) and build an
       interval map of line ranges (unchanged hunks map 1:1; changed hunks are "anchor inside a
       changed region").
       - If the anchored line range falls entirely in an UNCHANGED hunk → remap to the new line
         numbers (the position carries over cleanly).
       - If it falls in a CHANGED/deleted hunk → mark thread `outdated=true`; keep the original
         (blob, line) so "view in original context" still renders.
    3. Rename detection: git's similarity rename detection (the `-M` heuristic) maps `path`→new path;
       if found, follow it before step 1-2.
```

This is the GitHub/GitLab-class approach (content anchor + diff-position remap + outdated fallback). The
**named floor (GF-5):** v1 does per-pair blob-diff remap; the follow-on is **patch-id-chain carry-over**
across a rebase (matching `git patch-id` across the pre/post-rebase commit sequence) so a thread can
follow a *rebased* hunk rather than going `outdated` — the harder "changes since you last reviewed"
correctness case (Phase-1 §4.2). v1 uses `review.head_oid_reviewed` to compute "changes since you last
reviewed" as a plain diff between the last-reviewed head and the current head.

---

## 6. The merge gate & merge queue

### 6.1 The merge gate (the "what is allowed to land" decision)

On a merge request (`git.merge` tool, `myelin pr merge`, or merge-queue activation), the gate evaluates,
for the PR's `base_ref` and matching `ruleset`:

1. `Id.check(actor, merge, pr)` → reduces to `parent_repo->protected_push` (Id namespace `01 §...`,
   contract 4.9).
2. Required approvals satisfied (count + CODEOWNERS + not-dismissed-stale).
3. Required checks green (`check_status` aggregated from CI events).
4. Linear-history / signed-commit / conflict-free constraints.
5. **Agent policy**: if the actor is an agent and `ruleset.agent_needs_human`, the merge is **Gated**
   (an Agent `EffectApi` HITL gate, ADR-08/AG-8) — an agent cannot bypass required human approval unless
   policy allows. This is the agent-vs-human merge policy, riding Id's delegation algebra (AG-2).
6. Any bypass emits `git.protection.bypass_used` (audit-critical).

The merge itself is a **linearizable ref CAS on `base_ref`** in the DB transaction (§3-4) — the
protected-ref merge serialises on the ref row, no split-brain (the DB is the linearisation authority, §4).

### 6.2 The merge queue (a durable workflow)

`--auto` (merge-when-green) and the merge queue are **durable workflows** (`DurableExecutor`, contract
9.1; ADR-09): a `merge_queue_entry` rows the PR; a workflow holds `state=waiting`, woken by a durable
**signal** when `check_status` clears or an approval arrives (`ci.result`/`approval` signals, possibly
*days* later for a HITL gate). On clearance the workflow runs the §6.1 gate and the §4 linearizable
merge, then emits `git.pr.merged`. **Floor (GF-8):** v1 is a **single-lane serialised queue** (correctness
first — one PR tested+merged at a time per base ref); the follow-on is a speculative/parallel batched
queue (GitHub-merge-queue-class) once base-ref merge throughput is measured as a bottleneck.

---

## 7. Forks & shared object storage (TE-26) — resolved

**Decision: forks within a network share object storage via a content-addressed object pool (the
`network_root`), NOT independent copies; erasure and residency are handled by the per-tenant
content-addressing already in `BlobStore`.**

### 7.1 Mechanism + the erasure/residency reconciliation

A fork's objects are stored against the **network root's object pool** (git's `alternates` model:
GitHub's forks-share-storage). A PR-from-fork references objects in the shared pool; the fork only adds
its new objects. This is the storage-economics win (a fork of a huge repo costs only its delta).

The Phase-1 §12 worry was that shared object storage *complicates erasure and residency*. It does **not**,
because:

- **Residency** — the object pool is `(tenant, region)`-scoped in `BlobStore`; forks across tenants do
  **not** share a pool (cross-tenant dedup is deliberately forgone, Storage §3.2 — that would be a
  residency/isolation leak). A fork **across tenants** (a public repo forked into another tenant) gets an
  independent copy in the forking tenant's region. So pool-sharing is *within a tenant's region*, which
  is residency-safe by construction.
- **Erasure** — because objects are content-addressed and reference-counted in the pool, erasing a fork
  drops its refs; objects unreachable from *any* network member are pruned on GC. Personal data in
  history is handled by the GD-1 levers (pseudonym map delete / history-rewrite), which operate on the
  *commit bytes*, independent of pool-sharing. The pool does not make erasure harder; it makes the
  reachability bookkeeping a per-network (not per-repo) GC, which §8 already does.

---

## 8. GC / repack / maintenance at fleet scale

The hazard (Phase-1 §3.1): loose-object explosion from many small pushes; stale acceleration structures
silently degrade clone latency; full repacks are CPU/IO-heavy.

- **Geometric repacking** (git's `--geometric`): incremental repacks that keep a geometric size
  progression, avoiding full repacks (git-maintenance design). Scheduled, rate-limited, off-peak per
  cell.
- **Reachability bitmaps + commit-graph + MIDX kept fresh** on a cadence after ref-update bursts;
  staleness is a monitored signal (X-1 telemetry). For hot repos these are mandatory, not optional.
- **Prune interacts with erasure**: unreachable objects are pruned with a grace period and `refs/keep`
  pins; the **erasure path must reach reflogs, bitmaps, and backups** (Storage §5.4) — crypto-shred via
  the per-tenant blob DEK reaches reflogs/bitmaps/backups; only the immutable *commit-object bytes* are
  the residual (GD-1, `05 §HP-7`).
- Maintenance runs as **durable-workflow activities** (so a crashed repack resumes) and passes through
  **reserve/settle** if it is spend-bearing compute (`03 §reserve-settle`).

---

## 9. The code projection (TE-27) — the indexable output Git owns

Git hosting **owns what to index**; Search owns the index (contract 6.5; Search §4.4). On each
`git.ref.updated` to an indexed ref (default branch + configured refs), the **code-projection emitter**:

```
for each blob changed between last_indexed_oid and new tip (code_projection_cursor):
  emit a git.<...>.snapshot-shaped projection doc per blob:
    { artifact_ref: myelin://<tenant>/git/blob/<repo>:<ref>:<path>,
      path, language (detected),
      symbols: [ lightweight per-language tokenizer output —
                 identifiers split on camelCase/snake_case, def-like names ],
      literals: [ string/number literals ],
      trigrams: (Search builds these; we supply the text),  # Cox trigram index (2012)
      commit_message: <tip commit message>, blob_oid }
  via OutboxTx::emit (so reindex-from-source replays the same path — SEARCH-1)
update code_projection_cursor
```

This is **symbol/path/literal/trigram-grade (the GF-3 floor)** — "find this identifier/path/literal
across the repo" without an AST. The projection rides the **outbox**, so `replay(scope, since)` re-emits
exactly the same `*.snapshot` docs for a cold Search reindex (the only rebuild path, SEARCH-1). The
follow-on (GF-3): **AST-aware / cross-reference** code intelligence fed by **SCIP/LSIF indices produced
by CI** (Phase-1 §4.5) — git hosting consumes the CI-produced index and projects "find usages"; this is
the named later step, demand-triggered.

**Permission scoping**: the index doc's `acl_object_type` is the repo (Id namespace), so Search's
`list_objects(viewer, read, repo)` pre-filter (no leak/no N+1, the `search-requires-acl-filter` lint)
gates results per viewer.

---

## 10. Scaling / sharding within the cell topology + hot-spots

- **Repo is the unit of placement** (a primary + in-region replica set per repo; §4). The control plane
  maps `repo_id → cell → placement`; the front door is stateless and scales horizontally.
- **The ref store** is the write hot-spot — a busy default branch serialises on its ref row (the DB CAS
  is the linearisation point, §3-4). Mitigated by per-ref (not per-repo) locking so only the *one* hot
  ref serialises; everything else is parallel. The DB is the single linearisation authority — no rival
  consensus group.
- **Clone-storm / hot-repo** is the read hot-spot — mitigated by bundle-URIs from the object tier (§1.4),
  follower read replicas (§4), and the protected-human-lane shed order (ADR-16) so a CI/agent clone storm
  sheds before a human's interactive fetch.
- **Per-tenant in-flight caps** (X-3) ensure one tenant's push/clone burst cannot starve another;
  fairness is per-tenant.
- **The fan-out of `git.ref.updated`** to CI/Search/Refs/Agents is async off the bus, never synchronous —
  a push completes when the ref moves + the outbox commits, not when consumers catch up.
