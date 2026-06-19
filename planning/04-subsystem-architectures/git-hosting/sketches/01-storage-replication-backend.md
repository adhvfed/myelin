# Sketch 01 — Storage & replication backend (TE-24)

> Exploration note. Weighs the candidate designs for where authoritative git bytes live and how
> they replicate. This is the **major architecture fork** for the subsystem (Phase-2 §9 ranked it #1
> after erasure). Decision committed in `00-findings.md`; the architecture stage builds the detail.
> Prior art cited inline; re-verified facts flagged. Date: 2026-06-19.

## The problem

A git repo's authoritative state is (1) the **object store** (loose objects + packfiles, content-
addressed by the git object hash) and (2) the **ref store** (mutable named pointers). Both must be:

- **Durable + HA** — a node loss must not lose a repo or block writes for long.
- **Linearizable on protected-ref updates** — a PR merge into `main` and a concurrent force-push
  must serialise; no split-brain where two replicas disagree on the tip of `main` (Phase-1 §3.4,
  §11.2; Phase-2 §3 "linearizable protected-ref merges, no split-brain").
- **Relocatable, never node-pinned** (STOR-5 / GIT-1) — the v1 data model must let a repo move
  between nodes and later move its packs to object-backed storage *without a rewrite*.
- **Residency-pinned** (ADR-11) — every replica/pack/bundle stays in the tenant's region; there is
  no cross-region replica for an EU tenant (Phase-1 §9.3 — residency forecloses non-EU replicas).
- Scale to **millions of mostly-cold repos** (polyrepo long tail) and a **few hot repos/monorepos**.

The fork is **bare-repos-on-replicated-filesystem** vs **object-store-backed packs**, crossed with
the **replication/consistency mechanism** (quorum voting vs primary+WAL vs Raft-on-refs).

## Candidate A — Bare repos on replicated FS, primary + streaming replicas (Gitaly–Praefect model)

Each repo is a bare repo on local disk on a **primary** serving node; N **secondary** nodes hold
replicas; a **router/placement** service (Praefect-equivalent) tracks `repo → {primary, replicas}`
and routes writes to the primary, reads to any in-sync replica. Writes replicate via a per-repo
**replication log** (Praefect uses a WAL-ish queue of RPCs / now a true per-repo WAL).

- **Prior art:** GitLab **Gitaly + Praefect** (Praefect = the router + a Postgres-tracked replication
  state; reads from up-to-date replicas, writes to primary, generation-number consistency). GitLab
  has run this in production at scale.
- **Pros:** stock git semantics (canonical git / libgit2 just work on local disk — synergises with
  the git-core decision, sketch 02); mature reference design; per-repo failover; cheap reads.
- **Cons:** local-disk packs are **node-pinned** unless the FS itself is networked — tension with
  STOR-5 relocatability; primary failover has a window; the Postgres-tracked replication state is an
  extra stateful component; rebalancing a hot repo means a physical move.
- **Consistency:** primary is the linearisation point for ref updates (it owns the ref lock); a merge
  is linearizable *on the primary*; the risk is a stale-primary split-brain on failover, mitigated by
  generation numbers + quorum acknowledgement before ack.

## Candidate B — Object-store-backed packs (Mononoke / JGit-DFS / Gerrit-DFS model)

Authoritative objects live as **content-addressed blobs in the object tier** (S3-compatible, the
Phase-3 `BlobStore`), *not* on a node's local disk. Serving nodes are **stateless compute** that pull
packs/objects from the object store (with a local cache) and write new packs back. The **ref store**
is a separate **transactional/DB-backed** store (since you can't put mutable refs in immutable blobs).

- **Prior art:** Meta **Mononoke** (Rust! — the existence proof a scalable Rust git server is
  feasible; backs Sapling/EdenFS); **JGit DFS** / Gerrit's `DfsRepository` (packs as blobs in a
  pluggable KV/object backend, refs in a separate store); Google's internal git-on-Bigtable lineage.
- **Pros:** **decouples compute from storage** — serving nodes are stateless and horizontally
  scalable; **relocatability is free** (a repo isn't pinned to a node — any node can serve it by
  reading the object store); residency is a property of the object-store prefix; replication/durability
  is **delegated to the object tier** (S3 already replicates within-region); this is the STOR-5
  end-state by construction.
- **Cons:** **higher read latency** (object-store round-trips vs local disk) — mitigated by an
  aggressive node-local pack cache + clone-bundle CDN; **much more to build** (the DFS pack layer,
  the pack cache coherence, the object-store-aware repack/GC); the ref store is now a separate
  consistency problem (below). This is the heaviest option.

## Candidate C — Hybrid: local-disk packs behind the `BlobStore` trait, ref store DB-backed, object-backing as the named follow-on

v1 runs **bare repos on local disk** (Candidate A's serving simplicity + stock-git compatibility) but
**addresses every pack/loose-object/bundle through the Phase-3 `BlobStore` trait** (STOR-1: fs↔object
is a one-line swap) and keeps the **ref store transactional/DB-backed from day one** so it is never
the file-system bottleneck. Replication is **primary + WAL-streamed replicas with quorum-ack on
protected-ref updates** (Praefect-class). The object-backed pack tier (Candidate B) is the **named,
designed-not-built follow-on** the seam is built for.

- **Why this matches the doctrine:** STOR-5 literally says *plan the local-disk → object-store-backed
  transition as explicit sequenced work; the v1 data model must keep repos relocatable, never
  node-pinned*. Candidate C is that instruction made concrete: relocatability comes from (a) the
  `BlobStore` indirection and (b) the DB-backed ref store, so a repo's identity is `(tenant, repo_id)`
  in the control plane, never a node path.
- **The ref store is the linchpin:** put refs in a **transactional store** (see sketch 04) so
  protected-ref linearizability is a DB transaction, not a filesystem lock race — this is the property
  Phase-2 §3 requires, and it is *independent* of where the packs live, which is why we decide it now.

## Replication / consistency mechanism (the sub-fork)

| Mechanism | Linearizable ref update | Split-brain risk | Op weight | Prior art |
|---|---|---|---|---|
| **Primary + WAL-streamed replicas, quorum-ack** | yes (primary owns the ref lock; ack after quorum) | low (generation numbers + fencing) | medium | Praefect; Postgres streaming replication |
| **Quorum/voting on each ref update (3-way)** | yes (majority agrees the new tip) | very low | high (vote per push) | GitHub Spokes / DGit (≥3 file servers, three-phase) |
| **Raft on ref updates** | yes (consensus log of ref moves) | very low | medium-high (a Raft group per repo or per shard) | etcd/Raft (Ongaro & Ousterhout, USENIX ATC 2014); some experimental git servers |

**Leaning:** the **ref store transaction itself** is the linearisation point (the git subsystem owns
the ref lock — bus §2.3 already relies on this: "the git subsystem's *own* ref-update transaction is
the linearisation point"). So the simplest correct design is **the ref store in a consensus-replicated
transactional store** (the cell's Postgres, which is already WAL+PITR and the cross-seam anchor) +
**packs replicated for durability** (quorum-ack streaming). We get linearizability from the ref-store
DB transaction and durability from pack replication — we do **not** need a bespoke per-repo Raft group.
This also means **the outbox row for `git.ref.updated` commits in the same transaction as the ref move**
(BUS-2), giving "outbox order == ref-update order by construction" (bus §2.3) for free.

## Leaning (committed in findings)

**Candidate C**: local-disk packs behind the `BlobStore` trait (object-backed packs are the named
STOR-5 follow-on); **DB-backed transactional ref store** as the linearisation point (reftable-format
on disk for the serving copy — sketch 04); **primary + quorum-ack WAL-streamed replicas** for pack
durability/HA; **repo placement via the Tenancy control plane** (`discover`/`placement_of`,
contract 12.2/12.3) keyed on `(tenant, region, repo_id)`, never a node path. Residency is enforced at
the front door (reject any route leaving the region) and at the object-store prefix.

**Floor named:** object-store-backed packs (Mononoke/JGit-DFS-class) are **designed-not-built**; the
`BlobStore` seam + DB-backed refs are the v1 relocatability guarantee. **Follow-on:** the DFS pack
layer + pack cache + object-store-aware GC, promotion-triggered by **measured** single-node pack
pressure (not predicted — EI-02 §8).

## Prior art

- Gitaly / Praefect (GitLab) — router + replicated bare repos + generation-number consistency.
- GitHub Spokes / DGit — three-way voting replication of bare repos across ≥3 file servers.
- Meta **Mononoke** (Rust) — object-store-backed scalable git/hg server (Sapling/EdenFS backend).
- JGit **DFS** / Gerrit `DfsRepository` — packs-as-blobs in a pluggable backend; refs separate.
- Ongaro & Ousterhout, *In Search of an Understandable Consensus Algorithm (Raft)*, USENIX ATC 2014.
- Quinlan & Dorward, *Venti*, FAST 2002 — content-addressed archival storage (the `BlobStore` model).
- Phase-3 `storage.md` §3.5 (the STOR-5 seam), §3.2 (`BlobStore`); `event-bus.md` §2.3 (per-ref order).
