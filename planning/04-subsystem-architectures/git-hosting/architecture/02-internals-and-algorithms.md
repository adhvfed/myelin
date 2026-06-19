# 02 — Internals & Algorithms

> The subsystem-specific algorithms, conformed to the frozen reconciled contracts: smart-transport
> (canonical `git`, TE-8); the in-process Rust push-policy engine wrapping a sandboxed `receive-pack`; the
> reftable-on-OLTP ref store; replication (TE-24 — the DB transaction is the linearisation point);
> **diff-anchoring as the content-fingerprint 4-state resolver (OQ-D)**; GC/repack; **the merge gate +
> merge queue implementing the X-1 CheckStatus consumer, monotonic `run_attempt` supersession, the
> fork-endorsement flow, and the `ci.result` durable-signal wait**; forks (TE-26); monorepo (TE-25); the
> code projection (TE-27). Each hard problem also has a prior-art-cited write-up in
> [`05-hard-problems.md`](./05-hard-problems.md). Date: 2026-06-19.

---

## 1. The git wire protocol (the serving surface)

### 1.1 Smart-HTTP protocol v2 (default)

`GET /<repo>.git/info/refs?service=git-upload-pack` → v2 capability advertisement (the repo's
`object-format`, `fetch`, `ls-refs`, `shallow`, `filter`, `wait-for-done`), then
`POST /<repo>.git/git-upload-pack` (fetch) / `git-receive-pack` (push). **Protocol v2 is the default**
because its server-side `ls-refs` ref-filtering avoids advertising millions of refs on every fetch (Phase-1
§6) — load-bearing for monorepos and large fork networks. The front door **streams** request/response (no
whole-pack buffering); negotiation (`want`/`have`) + pack generation run in the serving tier via
**`GitCore::upload_pack`** — in v1 canonical `git` upload-pack, sandboxed + streamed (TE-8).

### 1.2 SSH

In-process `russh` server. On connect: the offered public key → `Id.authenticate(ssh_pubkey)` → `Principal`
(Id owns the SSH-key/deploy-key/PAT→principal map; contract 4.1 machine-identity resolution — a **deploy key
is a repo-scoped machine principal**). Then `git-upload-pack '<repo>'` / `git-receive-pack '<repo>'` is
parsed, `Id.check(principal, pull|push, repo)` runs, `placement_of(repo)` resolves, residency is enforced,
and the transaction streams to the serving tier. In-process (not per-connection `sshd` fork) so connection
fan-out under clone-storm is cheap.

### 1.3 Partial-clone / sparse / shallow (monorepo + large-repo serving)

`upload_pack` honours `filter=blob:none` / `blob:limit=<n>` / `tree:0` (partial clone) and `--depth`
(shallow). This is the **monorepo strategy** (TE-25; `05 §HP-3`): large-but-normal monorepos via
partial-clone + sparse-checkout + a fresh commit-graph + bitmaps, not a virtual filesystem. Prior art:
GitHub/GitLab partial-clone; Microsoft Scalar/VFS-for-Git lineage.

### 1.4 Accelerated clone (hot-repo / clone-storm)

For hot repos the serving tier maintains a **precomputed bundle** in the **within-EU CDN clone/bundle blob
class** (contract 11.2 C3) and advertises a **bundle-URI** (`transfer.bundleURI`): the client fetches the
static, content-addressed bundle from the edge (within-EU POPs only, residency-pinned) then does an
incremental fetch for the delta. This offloads clone-storm read fan-out from serving compute to the object
tier (Phase-1 §3.4). Bundles regenerate on a cadence driven by ref-update volume. Clone-bundle blobs are
**content-addressed**, so an edge cache is a pure content cache — no per-request authz at the edge; the
front door gates *which* tenant may request the bundle URI (Storage §3.2 C3).

---

## 2. The receive-pack path: sandboxed `git` for bytes, in-process Rust for policy + the transaction

Push is the correctness-critical write path. The byte plumbing runs as **sandboxed canonical `git
receive-pack`** into a **quarantine** object dir (TE-8); the **policy decision + the ref-CAS + outbox write
run in our in-process Rust**, all in **one DB transaction** (BUS-2).

```
push(repo, packstream, principal):
  1. `git receive-pack` (SANDBOXED under the X-6 profile: egress-deny, ro-root + tmpfs, caps dropped,
     no-new-privileges, seccomp, capped) ingests the pack into a QUARANTINE object dir (git's quarantine
     model: objects not yet referenced; abort discards them) and reports PROPOSED ref updates
     (old_oid → new_oid). The pack-write runs git's `sha1dc` collision check (the SHA-1 mitigation).
  2. IN-PROCESS RUST POLICY — for each proposed ref update:
        - Id.check(principal, push | protected_push, repo:ref)   # ReBAC ref-glob-scoped relation (4.9)
        - ruleset eval: force-push/deletion bans, linear-history, signed-commit, size limits
          (required-CONTEXT enforcement is deferred to the MERGE GATE for PRs — §6, X-1)
        - secret-scan the new objects (regex/entropy; reject before the ref moves)
        - PSEUDONYMITY enforcement (GIT-1): assert/normalise author identity to the pseudonym
        - agent rule: if principal.kind == agent and ruleset.agent_needs_human → reject direct push
        - REJECT → abort the whole atomic push (all-or-nothing per push), discard quarantine
  3. On accept: migrate quarantined objects into the repo's object DB (via BlobStore; the object-migration
     step does not ack until bytes are durable on the write quorum — §4).
  4. BEGIN TX:
        UPDATE git_ref ... ; INSERT git_reflog ... ;            # the ref CAS = linearisation point
        OutboxTx::emit(git.ref.updated{repo, ref, old, new, forced, commit_oids, pusher_pseudonym},
                       cause = the push session)                # same tx (BUS-2); aggregate = the ref
     COMMIT.
  5. (async, off the bus) CI triggers, Search code-projection, Refs edges, commit-graph/bitmap refresh,
     diff-anchor re-resolution for open PR threads on this ref (§5).
```

**Execution locus (resolved): in-process Rust policy + transaction, NOT native shell `pre-receive`/
`post-receive` hooks.** Native hooks fork a shell per push, make "policy + outbox in one transaction"
awkward, and a tenant-authored hook script is an arbitrary-code-execution surface. Our in-process engine
(a) keeps the outbox write in the **same DB transaction** as the ref CAS (BUS-2 by construction); (b) is a
typed, testable Rust component; (c) reject-before-the-ref-moves is guaranteed because the ref CAS is *our*
code in step 4, gated by *our* policy in step 2. We do **not** emit via git's native `post-receive` (a dual
write outside the DB transaction — forbidden, `no-raw-publish`). Tenant-supplied custom checks, if ever
offered, run as **CI jobs in the X-6 sandbox**, not as hooks (a named non-goal for v1).

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

The `FOR UPDATE` row lock on the single ref row is the **linearisation point** for that ref. Different refs
of the same repo lock different rows → they advance in parallel (throughput), while one ref is strictly
serialised (per-ref order, contract 2.3). `update_seq` becomes the `outbox.seq` tiebreak.

---

## 4. Replication & consistency (TE-24) — CARRIED FORWARD (Stage-1 committed)

**Decision (unchanged): the linearisation point is the DB ref-store transaction (a per-ref CAS, §3), NOT a
bespoke per-repo consensus group. Durability of the bulk pack bytes is a separate concern, handled by a
primary + quorum-ack WAL-streamed replica set behind the `BlobStore` seam. Consistency and durability are
decoupled. v1 is single-cell; cross-cell active sets are GF-2.**

### 4.1 Why the DB transaction, not Raft-on-refs / quorum-voting / primary+WAL-only

The hard requirement (Phase-1 §3.4, Phase-2 §3) is **linearizable protected-ref merges with no split-brain**.
The menu: GitHub Spokes/DGit (whole-repo voting), GitLab Gitaly-Praefect (primary + WAL with a coordinator
authority), Raft on the ref-update log. **We use the DB transaction as the linearisation point** — and *not*
a bespoke consensus group — because **the git subsystem already owns the ref lock and the outbox row commits
in the same transaction (BUS-2)**: "outbox order == ref-update order *by construction*." A single DB CAS
gives a linearizable protected-ref merge *and* per-ref event ordering *and* exactly-once emit, in one
mechanism we already have. A per-repo Raft cohort would (a) duplicate a linearisation authority the DB
already provides, (b) add an operational consensus tier per repo, (c) still need the DB for the
outbox/metadata — buying a second source of truth and a split-brain *between* the Raft log and the DB. The
DB is **the** tiebreaker. This is the **Praefect-shaped** position collapsed onto our OLTP: **Postgres is
the Praefect.** The **bulk bytes (packs) are decoupled** into the object tier (the Mononoke/JGit-DFS "packs
as blobs" insight), so durability replicates *separately* from linearisation.

### 4.2 The mechanism

- **Consistency / linearisation:** the ref CAS in the DB (§3). The ref row's `FOR UPDATE` lock is the
  per-ref serialisation point; a protected-ref merge is a ref CAS on `base_ref` and serialises there. No
  split-brain — exactly one authoritative ref state (the committed DB row). The hosting OLTP is itself
  HA-replicated (synchronous quorum-commit Postgres — Storage's OLTP tier), so the ref authority survives a
  node loss with a linearizable failover (the DB's own consensus, not a git-specific one).
- **Durability of pack bytes:** a repo's pack/loose bytes are written through `BlobStore` to a **primary +
  quorum-ack WAL-streamed replica set**: a push's object-migration step (§2 step 3) does not ack until bytes
  are durable on a write quorum of in-region replicas. Followers serve reads once they hold the referenced
  bytes. Because packs are content-addressed, a follower needs only the *ref record* + the bytes — which is
  what makes repos **relocatable** (STOR-5/GF-1).
- **Recovery consistency:** the DB ref index is the tiebreaker. A recovering serving node reconciles its
  on-disk reftable/pack view *to* the authoritative `git_ref` rows: any ref tip in the DB whose objects are
  present is served; any local ref ahead of the DB (an un-acked push) is discarded. Fencing uses
  `update_seq` as a generation number — a node serving a stale `update_seq` is behind and refreshes before a
  read that needs read-your-writes (the `update_seq` is the zookie/fence). **The DB commit is the only thing
  that ever "happened"; everything else reconciles to it.**
- **Residency:** all replicas of an EU-tenant repo are pinned in-region (ADR-11); the control plane rejects
  placement that would put a replica outside the tenant's region. `residency_verify` (contract 12.4) covers
  the replica set + the CDN clone edge set. No non-EU replicas, ever.

### 4.3 Honesty / floor

v1 ships **single-cell** placement (primary + in-cell quorum replicas); geo read-replica within-EU and
cross-cell active set are GF-2. The object-store-backed pack tier that makes §4.1's decoupling *fully* real
is GF-1 — v1 may keep packs on local NVMe behind the `BlobStore` trait, the quorum-WAL replica set providing
durability; the swap is a `BlobStore`-impl change (STOR-5). The exact quorum-ack protocol + failover window
is OQ-4 / drill D-5.

---

## 5. Diff-anchoring across rewrites (TE-22) — the content-fingerprint 4-state resolver (OQ-D)

The problem (Phase-1 §4.3): a comment anchored to "line 42 of file X" must survive force-push, rebase,
amend, and base-branch movement, and degrade legibly when it cannot. **Reconciliation froze this as the
unified `#sub` line-range resolver** (contract 5.7): Git mints a **content-anchored** `#L<a>-L<b>` storing a
BLAKE3 fingerprint, and resolves through the *same four-state ladder* Knowledge/Chat use.

**Decision: store a content anchor — `(anchor_blob_oid, path, side, line-range, anchored_commit_oid,
anchor_fingerprint = BLAKE3(anchored lines + context window))` — and resolve against a newer blob into
exactly one of four states: exact(live) / rebased(moved) / partial(outdated) / tombstone(content_gone).**

### 5.1 The mint + resolve algorithm

```
mint anchor (on comment create):
  store (anchor_blob_oid, path, side, anchor_line, anchor_line_end, anchored_commit_oid,
         anchor_fingerprint = BLAKE3(lines[anchor_line..=anchor_line_end] ++ context_window))   # 01 §4.4

resolve(anchor, new_head) → one of {LIVE, MOVED, OUTDATED, GONE}:   # the contract-5.7 four states
  1. EXACT: anchor_blob_oid still present in new_head's tree at `path` (or a rename target, via git's
     similarity rename detection -M) → return the exact range. state = LIVE.
  2. Else diff old_blob → new_blob (imara-diff / Myers 1986), build a line-interval map:
       - the fingerprinted lines are found at a SHIFTED position (3-way context match on the BLAKE3
         fingerprint + window) → REBASED: return the shifted range. state = MOVED.
       - SOME anchored lines survive, some are gone → PARTIAL: return the surviving sub-range.
         state = OUTDATED (Git's named "outdated-line-range" case).
       - the anchored content is ENTIRELY gone → GONE: Tombstone{root: the PR, reason: content_gone}.
  persist anchor_state; the #sub resolver (Refs ladder, 5.7) maps LIVE/MOVED/OUTDATED/GONE → projection/flag.
```

This is the GitHub/GitLab-class content-anchor approach, now made **fingerprint-based** so a `rebased`
(MOVED) match is reliable rather than a guess, and aligned to the one Refs ladder (so a tombstone always
carries the root PR — "this referenced PR #N (that line is no longer present)"). The resolution feeds Refs
when an embed/unfurl resolves the `#L<a>-L<b>` sub (contract 5.2/5.7); Git is the **owner's sub-anchor
resolver** the ladder calls in step 3.

### 5.2 The named floor (GF-5)

v1 does per-pair blob-diff fingerprint remap + the four states. The follow-on is **patch-id-chain
carry-over** across a rebase (matching `git patch-id` across the pre/post-rebase commit sequence) so a
thread follows a *rebased* hunk through a multi-commit rebase rather than degrading — hardening the
`rebased→MOVED` case for the harder "changes since you last reviewed" correctness (Phase-1 §4.2). v1 uses
`review.head_oid_reviewed` for a plain "changes since you last reviewed" diff.

---

## 6. The merge gate & merge queue — the X-1 CheckStatus consumer (the hardest seam)

This section implements **contract 5.9** (the Git↔CI check seam). Git is the **consumer + gate**; CI is the
producer. The two load-bearing rules: **Git owns which contexts are `required`** (branch-protection policy),
and **Git reads `trust_tier` off the fact** — it never recomputes trust and never synchronously calls CI.

### 6.1 The `check_status` consumer (the projection that drives the gate)

Git consumes `ci.check.updated` (idempotent on `event_id`, `consumer_dedup` ledger). The event carries the
frozen `CheckStatus` struct (small, PII-free). The consumer applies the **monotonic `run_attempt`
supersession** rule into the `check_status` projection table (`01 §4.3`):

```
on ci.check.updated{ CheckStatus{ commit_oid, context, state, required:_, run, run_attempt,
                                  trust_tier, details_ref, summary, cost_settled, ... } }:
  key = (tenant, repo, commit_oid, context.provider, context.name)        # the (commit_oid, context) key
  SELECT run_attempt AS stored FROM check_status WHERE key FOR UPDATE;
  if NOT EXISTS or run_attempt >= stored:        # >= supersedes (monotonic on the COUNTER, not wall-clock)
      UPSERT check_status SET state, run_ref=run, run_attempt, trust_tier, details_ref,
             summary_template, summary_args, cost_settled, started_at, completed_at;
  else:                                          # a LOWER run_attempt arriving late = stale re-delivery
      DROP (the bus is at-least-once; this drop is mandatory)
```

`run_attempt` (not `completed_at`) is authority — "clocks are not authority; the attempt counter is" (X-1).
A re-run of `test/unit` bumps `run_attempt`; its result supersedes the prior; a late-arriving older result
is dropped. The **`required` flag on the fact is advisory** — Git's *own* `ruleset.required_contexts`
policy decides which contexts gate this `base_ref` (CI reports facts; Git decides which gate).

### 6.2 The merge gate (the "what is allowed to land" decision)

On a merge request (`git.merge` tool, `myelin pr merge`, or merge-queue activation), the gate evaluates,
for the PR's `base_ref` and matching `ruleset`:

```
may_merge(pr) =
  1. Id.check(actor, merge, pr)  → reduces to parent_repo->protected_push (contract 4.9). zookie-stamped
     (read-your-writes, so a just-granted permission counts — contract 4.10).
  2. required approvals satisfied (count + CODEOWNERS + not-dismissed-stale; CODEOWNERS via list_subjects).
  3. FOR EACH required_context in ruleset.required_contexts:
        row = current check_status[(commit_oid = pr.head_oid, context)]
        require row.state == success
        require ACCEPTABLE TRUST POSTURE (the fork-endorsement rule, §6.3)
  4. linear-history / signed-commit / conflict-free constraints.
  5. AGENT POLICY: if actor.kind == agent and ruleset.agent_needs_human → GATED (an EffectApi HITL gate,
     X-6 / AG-8; requires_approval = yes for git.merge). An agent cannot bypass required human approval
     unless delegation allows.
  6. any bypass emits git.protection.bypass_used (audit-critical, contract 10.6).
```

The merge itself is a **linearizable ref CAS on `base_ref`** in the DB transaction (§3-4) — the protected-ref
merge serialises on the ref row, no split-brain.

### 6.3 The fork / trust-tier gate (the security-critical half — the poisoned-pipeline defence)

A check whose `trust_tier = untrusted_fork` (a PR from a fork, or any run that executed untrusted
contributor code) is **recorded but cannot satisfy a `required` context by itself**:

```
acceptable_trust(check_status row, repo) =
     row.trust_tier == trusted
  OR row.endorsed_by IS NOT NULL                    # a maintainer endorsed this exact run
  OR (a context re-run exists under trust_tier == trusted that supersedes it)

# until endorsed/re-run-trusted, an untrusted_fork SUCCESS is treated as NEUTRAL for gating:
gate_state(row) = if row.state == success and not acceptable_trust(row, repo) then NEUTRAL else row.state
```

**Endorsement** is an ordinary permission, not bespoke logic: a maintainer calls
`check(subject, approve_untrusted_ci, repo)` (the frozen ReBAC relation, contract 4.9 / identity §5); on
allow, Git stamps `endorsed_by = subject.pseudonym` on the current row and re-evaluates the gate. The
alternative endorsement path is the standard **"approve and run"**: a maintainer re-runs the context, CI
re-dispatches it under `trust_tier = trusted`, the new `CheckStatus` (higher `run_attempt`) supersedes, and
the gate goes green without an explicit endorsement record. **Rationale:** a fork PR must never turn its own
gate green by running attacker-controlled CI config (EI-02 §1; the classic poisoned-pipeline-execution
attack). Trust is *stamped by CI* from run provenance + the `read & !is_untrusted_fork` ABAC edge (identity
§5 C7); **Git does not recompute trust — it reads `trust_tier` off the fact.** The storage-tier half of this
defence is the trust-scoped cache namespace (Storage 11.2 C4): an `untrusted_fork` run's cache writes cannot
reach the `trusted` cache scope, so it cannot poison a later trusted run.

### 6.4 The merge queue (a durable workflow on the rollup `ci.result` signal)

`--auto` (merge-when-green) and the merge queue are **durable workflows** (`DurableExecutor`, contract 9.1;
ADR-09). A merge queue serialises merges into a busy `base_ref`; it is **one durable workflow per target
ref**. The workflow uses the **`SCHEDULE_AND_RUN_JOB` long-park idiom** (contract 9.2, OQ-F) and waits on
the **rollup `ci.result` signal** — *distinct from the per-context `ci.check.updated` events*:

```
merge_queue(base_ref):                                          # one durable workflow per target ref
  for each queued PR (FIFO; GF-8 single-lane):
    1. compute the speculative merge commit; dispatch the required CI:
         ctx.activity(SCHEDULE_AND_RUN_JOB, JobSpec{ kind: ci, ..., idem_token = entry.merge_attempt_id })
       # the activity dispatches + reserves (11.7) and RETURNS — it does NOT block on completion
    2. ctx.wait_for_signal("ci.result", idem_key = entry.merge_attempt_id)   # parks: holds NO runtime (9.4)
       # ... woken hours later by signal(run, "ci.result", {overall, contexts}, idem_key=merge_attempt_id) ...
    3. on a SUCCESS rollup for all required contexts → run §6.2 may_merge + the §3-4 linearizable merge;
       emit git.pr.merged. on FAILURE/ERROR → dequeue with a HUMANISED reason (contract 7.3) and continue.
```

**Why two channels.** The per-context `ci.check.updated` events drive the **always-visible PR checks UI**
(via the `check_status` projection); the single `ci.result` rollup signal drives the **merge-queue
workflow's resume**. Both are emitted by CI via its outbox. The signal payload is
`{commit_oid, overall: success|failure, contexts: [CheckContext], idem_token}`; `DurableExecutor::signal` is
**idempotent on `idem_key`** (a double-delivery is one wake — contract 9.1, OQ-F). The `merge_attempt_id` is
the `idem_token`, minted by the workflow at dispatch and stamped on the job, so producer (runner) and
consumer (workflow) agree on the key with no coordination.

A HITL approval (e.g. an agent-authored merge awaiting a human) is the **same durable-signal mechanism**
(contract 9.4) — the workflow can park days for an `approval` signal while holding no runtime.

**Floor (GF-8):** v1 is a **single-lane serialised queue** (correctness first — one PR tested+merged at a
time per base ref); the follow-on is a speculative/parallel batched queue once base-ref merge throughput is
measured as a bottleneck (OQ-5).

---

## 7. Forks & shared object storage (TE-26) — CARRIED FORWARD

**Decision (unchanged): forks within a network share object storage via a content-addressed object pool
(the `network_root`), NOT independent copies; erasure and residency ride the per-tenant content-addressing
in `BlobStore`.**

A fork's objects are stored against the network root's object pool (git's `alternates` model: GitHub's
forks-share-storage). A PR-from-fork references objects in the shared pool; the fork adds only its new
objects (the storage-economics win). The Phase-1 §12 worry that this complicates erasure/residency does
**not** hold:

- **Residency** — the object pool is `(tenant, region)`-scoped in `BlobStore`; forks across tenants do
  **not** share a pool (cross-tenant dedup is deliberately forgone — that would be a residency/isolation
  leak). A fork **across tenants** gets an independent copy in the forking tenant's region. Pool-sharing is
  *within a tenant's region* — residency-safe by construction.
- **Erasure** — content-addressed + reference-counted in the pool: erasing a fork drops its refs; objects
  unreachable from *any* network member are pruned on GC. Personal data in history is handled by the
  platform erasure posture's levers (pseudonym-map delete / history-rewrite) on the *commit bytes*,
  independent of pool-sharing.
- **Fork-PR trust** — a fork PR's runs are `untrusted_fork` (X-1, §6.3); its cache writes are confined to
  the `fork:<pr_id>` scope (Storage 11.2 C4). The fork cannot reach the trusted cache *or* the trusted gate.

---

## 8. GC / repack / maintenance at fleet scale

The hazard (Phase-1 §3.1): loose-object explosion from many small pushes; stale acceleration structures
silently degrade clone latency; full repacks are CPU/IO-heavy.

- **Geometric repacking** (git's `--geometric`): incremental repacks keeping a geometric size progression,
  avoiding full repacks. Scheduled, rate-limited, off-peak per cell.
- **Reachability bitmaps + commit-graph + MIDX kept fresh** on a cadence after ref-update bursts; staleness
  is a monitored signal (the telemetry signal set, contract 1.8). For hot repos these are mandatory.
- **Prune interacts with erasure**: unreachable objects are pruned with a grace period and `refs/keep` pins;
  the **erasure path reaches reflogs, bitmaps, and backups** (Storage §5) — crypto-shred via the per-tenant
  blob DEK reaches reflogs/bitmaps/pack backups; the immutable *commit-object bytes* are the residual (the
  platform posture, contract 10.9, `05 §HP-7`).
- Maintenance runs as **durable-workflow activities** (contract 9.2; a crashed repack resumes) and passes
  through **reserve/settle** (contract 11.7) if it is spend-bearing compute. **History-rewrite** runs as
  such an activity *and* is an **audited op** (contract 10.6) with **fork/mirror/clone-cache invalidation
  fan-out** (`03 §6`, `06 §GDPR`).

---

## 9. The code projection (TE-27) — the indexable output Git owns

Git hosting **owns what to index**; Search owns the index (contract 6.3/6.5; Search §4.4). On each
`git.ref.updated` to an indexed ref (default branch + configured refs), the **code-projection emitter**:

```
for each blob changed between last_indexed_oid and new tip (code_projection_cursor):
  emit a projection doc per blob:
    { artifact_ref: myelin://<tenant>/git/blob/<repo>:<ref>:<path>,
      path, language (detected),
      symbols: [ identifiers split on camelCase/snake_case, def-like names ],
      literals: [ string/number literals ],
      text: <blob text>,                      # Search builds trigrams (Cox 2012) — we supply the text
      commit_message: <tip commit message>, blob_oid }
  via OutboxTx::emit (so replay re-emits the same path — contract 2.6 / SEARCH-1)
update code_projection_cursor
```

**Symbol/path/literal/trigram-grade (the GF-3 floor)** — "find this identifier/path/literal across the
repo" without an AST. The projection rides the **outbox**, so `replay(scope, since)` re-emits exactly the
same `*.snapshot` docs for a cold Search reindex (the only rebuild path). **Follow-on (GF-3, contract 6.5):**
AST-aware "find usages" fed by **SCIP/LSIF indices produced by CI** — git consumes the CI-produced index and
projects "find usages"; named, demand-triggered.

**Permission scoping**: the index doc's `acl_object_type` is the repo, so Search's `list_objects(viewer,
read, repo)` push-down (the OQ-E `Filter`, §10 / `03 §5`) pre-filters per viewer (no leak/no N+1, the
`search-requires-acl-filter` lint).

**Restriction-safe**: the emitter skips a restricted subject's content (the `restrict` suppression, `03 §6`).

---

## 10. Scaling / sharding within the cell topology + hot-spots

- **Repo is the unit of placement** (a primary + in-region replica set per repo; §4). The control plane
  maps `placement_of(repo) → cell + placement group` (contract 12.2), region-pinned + relocatable; the
  front door is stateless and scales horizontally.
- **The ref store** is the write hot-spot — a busy default branch serialises on its ref row (the DB CAS is
  the linearisation point, §3-4). Mitigated by per-ref (not per-repo) locking so only the *one* hot ref
  serialises; everything else is parallel. The DB is the single linearisation authority — no rival
  consensus group.
- **Clone-storm / hot-repo** is the read hot-spot — mitigated by CDN bundle-URIs (§1.4, Storage 11.2 C3),
  follower read replicas (§4), and the protected-human-lane shed order (ADR-16, the OQ-K per-surface budget:
  speculative → batch/CI → agent → human-last) so a CI/agent clone storm sheds before a human's interactive
  fetch.
- **The `list_objects` push-down** (the OQ-E `Filter`, `03 §5`) keeps repo/PR list scans leak-free **and**
  fast: the consumer JOINs against Identity's per-tenant authz reverse index over `repo.id` / `pr.id` — one
  query, no N+1.
- **Per-tenant in-flight caps** (OQ-K) ensure one tenant's push/clone burst cannot starve another; fairness
  is per-tenant; the per-surface shed budgets are the named v1 floor tuned by drills (D-6).
- **The fan-out of `git.ref.updated`** to CI/Search/Refs/Agents is async off the bus, never synchronous — a
  push completes when the ref moves + the outbox commits, not when consumers catch up.
