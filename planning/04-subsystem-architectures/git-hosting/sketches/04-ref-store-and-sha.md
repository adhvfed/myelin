# Sketch 04 — Ref store & SHA-1 vs SHA-256 (TE-23)

> Exploration note. Two coupled data-model calls fixed early: (1) where refs live and how protected-ref
> updates linearize, and (2) the object hash algorithm + migration story. Both are nearly impossible to
> bolt on later. Re-verified reftable status (2026-06). Date: 2026-06-19.

## Part A — The ref store

### Obligations
- Scale to **millions of refs** (monorepos, `refs/pull/*`, `refs/notes/*`) — filesystem `packed-refs`
  and loose-ref directories are pathological at scale (Phase-1 §2.1/§2.3).
- **Linearizable protected-ref updates** (the merge/force-push serialisation point — sketch 03).
- High write concurrency without a global lock.

### Candidates
- **A. Filesystem `packed-refs` + loose refs** — stock git default. *Rejected:* does not scale to huge
  ref counts or high write concurrency (Phase-1 §2.3); loose-ref dir explosion.
- **B. `reftable` on-disk format** (Gerrit/JGit origin, now upstream). **Re-verified (2026-06):
  production-ready in Git 2.48–2.51; GitLab uses it for all new repos with a background migrator; Git
  3.0 (late 2026) makes it the default.** Block-based, sorted, supports millions of refs with log-time
  lookup and atomic transactional updates; far outperforms `files` backend. *Strong fit for the
  serving copy.*
- **C. DB-backed ref store** (refs as rows in the cell Postgres). Gives transactional CAS, easy
  replication, and the outbox-in-the-same-txn property (sketch 03), but a pure-DB ref store means the
  git core can't read refs the native way (the wire serving path expects an on-disk ref backend).

### Leaning — reftable on disk + a DB-backed authoritative ref index (hybrid)
- **The git serving core reads/writes refs via reftable** (canonical git supports it; fast at scale;
  the future default) — so `upload-pack`/`receive-pack` (sketch 02) work natively.
- **The authoritative linearisation + the outbox emit live in the cell Postgres**: a `ref` table
  (`(tenant, repo_id, ref_name) → tip_sha, generation`) where the **protected-ref update is a DB CAS
  on `tip_sha`** inside the same transaction as the outbox insert (sketch 03). reftable is then updated
  within that transaction's commit (a small, ordered write).
- This gives **both**: native git serving (reftable) **and** transactional linearizability + atomic
  outbox (DB). The DB row is the source of truth for "what is the tip and in what order did it move";
  reftable is the serving projection of it (kept consistent by the same txn). On a replica rebuild,
  reftable is reconstructible from the DB ref table — relocatability (sketch 01) holds.

**Why not pure reftable for linearization:** reftable gives atomic *transactional* ref updates on one
node, but our outbox-in-the-same-transaction (BUS-2) and cross-replica generation tracking want a real
transactional store. Co-locating the ref CAS with the outbox in Postgres is the clean seam. The cost is
keeping reftable and the DB row consistent — done by writing both in the one push transaction, with the
DB as the tiebreaker on recovery.

## Part B — SHA-1 vs SHA-256 (TE-23)

### The tension (Phase-1 §2.1)
- **SHA-1 (default `sha1` object format):** matches the entire ecosystem; every client/tool works;
  but inherits SHAttered-class collision weakness (git mitigates with `sha1dc` collision-detection).
- **SHA-256 (`objectformat=sha256`):** collision-resistant, future-proof; but ecosystem interop is
  **still immature** (Phase-1 §2.1) — many clients, forges, CI tools, and signing flows don't fully
  speak SHA-256, and there is **no transparent SHA-1↔SHA-256 interop** in stock git yet (the
  interop/round-trip story is incomplete).

### Candidates
- **A. SHA-1 default (with `sha1dc` collision detection), SHA-256 opt-in per repo.** Pragmatic; what
  GitHub/GitLab effectively do. Stock clients just work.
- **B. SHA-256 default.** Future-proof but **risks breaking "plain git must just work"** (a VISION/
  Phase-2 §5.1 non-negotiable) for the long tail of clients/tools that don't speak it.
- **C. SHA-256 default with a SHA-1 compatibility shim.** The ideal end-state, but the shim
  (transparent dual-hash interop) **does not exist in stock git** today — we'd be building/maintaining
  it. Out of scope for v1.

### How this interacts with the platform `BlobStore` (important non-conflation)
The Phase-3 `BlobStore` uses **BLAKE3** for *blob backing* content-addressing (storage §3.2) — that is
the **pack/loose-object storage address**, a *separate concern* from git's *own* object hash. Git
objects keep git's hash (sha1/sha256); the `BlobStore` may store a pack file whose content-address is
its BLAKE3. **No conflict:** git's object identity is internal to the repo; BLAKE3 is how the pack blob
is addressed in the object tier. (storage §3.2 explicitly carves this out: "git objects keep git's own
hashing; this trait is the *blob backing*.")

### Leaning — SHA-1 default (sha1dc), SHA-256 opt-in, design the data model hash-agnostic
- **v1 default = SHA-1 with collision detection (`sha1dc`)** so stock clients/tools/CI/signing all work
  ("plain git just works" — Phase-2 §5.1). This is the ecosystem-compatible floor.
- **SHA-256 is an opt-in per-repo object format** for tenants who want it and control their toolchain
  (e.g. a greenfield internal monorepo) — we store the repo's `object_format` in the repo metadata and
  the front door advertises it.
- **The data model is hash-length-agnostic** (store the object id as bytes + an `object_format` tag, an
  `ArtifactRef` commit id is the hex of whatever format the repo uses) so a future **SHA-256-default
  flip** (when ecosystem interop matures, plausibly post-Git-3.0) is a default change, not a schema
  rewrite. **Named follow-on:** SHA-256-default + the interop/migration story, promotion-triggered by
  ecosystem maturity (NOT built now).

**Honesty:** SHA-256 buys real security but at a real interop cost *today*; we take the
ecosystem-compatible default and keep the seam, rather than gamble VISION's "just works" bar on
immature interop.

## Leaning (committed in findings)

**Ref store:** **reftable** on-disk serving format (re-verified production-ready, the Git-3.0 default) +
a **DB-backed authoritative ref index in cell Postgres** where the protected-ref CAS + outbox insert
are one transaction (the linearisation point). **Hash:** **SHA-1 + `sha1dc` default**, **SHA-256
opt-in per repo**, **hash-agnostic data model** so a future SHA-256-default flip is a config change.
Both are named-floor-with-follow-on.

## Prior art / sources

- **reftable**: git-scm BreakingChanges (Git 3.0 default), GitLab reftable rollout epic #12503,
  JGit/Gerrit reftable origin; "outperforms files backend by orders of magnitude."
- **SHA-256 transition**: git `object-format` docs; SHAttered (Stevens et al. 2017) + `sha1dc`
  collision detection (the default git mitigation).
- Phase-3 `storage.md` §3.2 (BLAKE3 blob backing vs git object hash carve-out).
- Phase-2 git-hosting §3 (reftable/DB ref-store direction; TE-23 `[OPEN → P4]`).

[Sources: git-scm.com/docs/BreakingChanges; gitlab.com epics/12503]
