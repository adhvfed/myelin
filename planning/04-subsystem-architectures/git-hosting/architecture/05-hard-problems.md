# 05 — Hard Problems Resolved (with cited prior art)

> Each subsystem-specific hard problem, the resolution, the prior art it stands on, and the named floor
> where v1 is partial. Cross-refs the mechanism in `02-internals-and-algorithms.md`. Date: 2026-06-19.

---

## HP-1 — World-scale git storage (object-backed packs, sharding/replication, smart-transport)

**Resolution.** Repo is the unit of placement. The **linearisation point is the DB ref-store transaction**
(a per-ref CAS, `02 §3-4`) — *not* a bespoke per-repo consensus group — because the git subsystem already
owns the ref lock and the outbox row commits in the same transaction (BUS-2), giving "outbox order ==
ref-update order by construction." **Postgres is the Praefect** (the metadata authority for canonical ref
state). Bulk **pack bytes are decoupled** into the object tier behind `BlobStore`, made durable by a
**primary + quorum-ack WAL-streamed replica set** (consistency and durability replicated *separately*).
Smart-transport (upload-pack/receive-pack, protocol-v2, partial-clone, bundle-URIs) is served by
**sandboxed canonical `git`** in v1 (Stage-1-verified TE-8; `gix` has no server-side serving), with `gix`
in-process for read/diff/blame. See `02 §1, §4`.

**Prior art.** GitLab **Gitaly-Praefect** (a metadata authority decides the canonical replica — collapsed
here onto our existing OLTP) and GitHub **Spokes/DGit** (three-way voting) as the replication menu we
chose *between*; Meta **Mononoke/Sapling** (Rust git/hg server, packs/objects in scalable blob storage —
the "metadata layer is small, bytes live in object storage" insight; Meta Eng. 2022) and **JGit-DFS /
Gerrit pluggable backends** (packs as blobs in a KV/object store) for the byte-decoupling; **Venti**
(Quinlan & Dorward, FAST 2002) for content-addressed object storage. (We deliberately did **not** stand up
a per-repo **Raft** group — Ongaro & Ousterhout, USENIX ATC 2014 — because the DB transaction already
provides the linearisation authority a Raft cohort would add; a rival consensus log over the DB invites
split-brain *between* the two sources of truth, `02 §4.1`.)

**Floor (GF-1/GF-2).** v1 packs on local NVMe behind the `BlobStore` trait (repos relocatable, never
node-pinned — STOR-5); single-cell primary + quorum replicas. Follow-on: object-store-backed pack/delta
management + smart-transport over `BlobStore`; cross-cell active replica sets within-EU.

---

## HP-2 — Git-core build-vs-embed (TE-8): gix vs libgit2 vs shell-out

**Resolution (Stage-1 web-verified).** A layered **`GitCore`** strategy: **canonical `git`** (shelled-out,
sandboxed under the ADR-20/CI-1 profile, streamed) for **wire serving** (`upload-pack`/`receive-pack`/
`ls-refs`) and **maintenance** (repack/commit-graph/bitmaps/MIDX/bundle); **`gix` (gitoxide) preferred,
`libgit2` fallback, IN-PROCESS** for read/diff/blame and the code projection. The deciding fact, **Stage-1
re-verified against the current gitoxide release (2026-06): `gix` has NO server-side `upload-pack`/
`receive-pack`** (only the transport *client*) — so a pure-`gix` server is not viable for v1, and
canonical `git` serves the wire. The in-process Rust policy engine still wraps the shelled
`receive-pack` (quarantine → policy → our ref CAS + outbox in one txn), so the policy+outbox-in-one-
transaction property holds (`01 §2`, `02 §2`). The `GitCore` seam routes each op; wire/maint ops migrate
gix-ward **iff** the OQ-1 spike clears. See `01 §2`.

**Prior art.** **gitoxide** (pure-Rust git, no FFI/unsafe-C surface — the aspiration); the GitLab/Gitea
**shell-out-to-`git` baseline** (the proven-correct server path we adopt for the wire in v1); **Mononoke**
as the existence proof a world-scale *Rust* git server is feasible (the migration target the seam keeps
open).

**Floor.** "Wire serving on canonical `git`" is the v1 reality (not a temporary fallback); the follow-on
is "migrate wire ops to a `gix` server per-op once it ships + passes protocol-compat & sandbox-escape
drills." **Owed spike (OQ-1):** a capability-matrix spike running the current gitoxide `receive-pack`/
`pack-objects`/maintenance against a corpus, recording which ops can move off the shell. The `GitCore`
seam exists precisely to make the gitoxide bet swappable rather than load-bearing for v1.

---

## HP-3 — Monorepo ambition (TE-25)

**Resolution.** Support **large-but-normal monorepos** (deep history, many files, but not Google-scale)
via **partial-clone** (`filter=blob:none`/`tree:0`), **sparse-checkout/sparse-index**, **shallow**, and
**mandatory fresh commit-graph + reachability bitmaps + MIDX**. We **do not** build a Mononoke-class
virtual filesystem in v1; that is the explicit "out of scope, use a Google-scale system" line. The
threshold (repo size / file count / push QPS at which stock-git-on-a-shard breaks) is **benchmarked, not
guessed** (Phase-1 §3.2 explicitly refuses to guess). See `02 §1.3`.

**Prior art.** GitHub/GitLab **partial-clone**; Microsoft **Scalar / VFS-for-Git** (background
maintenance, recommended monorepo config — partly upstreamed); git **commit-graph + reachability
bitmaps** (the reachability-acceleration substrate); Meta **Mononoke/EdenFS** as the named
out-of-scope-for-v1 hyperscale path.

**Floor (GF-4).** v1 caps at benchmarked monorepo limits; the Mononoke-class backend is the named,
demand-triggered follow-on. **Owed drill (D-4):** a monorepo benchmark establishing the v1 ceiling.

---

## HP-4 — Diff/comment anchoring across rewrites (TE-22)

**Resolution.** Store a **content anchor** (blob OID + path + side + line-range + the commit it was
created against); on head movement, remap positions via the **blob-to-blob diff** (unchanged hunks map
1:1, changed hunks → `outdated`); follow renames via similarity detection; never silently move a comment
wrong; offer "view in original context". See `02 §5`.

**Prior art.** **Myers** *An O(ND) Difference Algorithm* (1986) / git's xdiff (via **imara-diff**, the
Rust port) for the line mapping; GitHub/GitLab's content-anchor + outdated-thread model as the proven UX
pattern; git's `patch-id` for the follow-on rebase carry-over.

**Floor (GF-5).** v1 does per-pair blob-diff remap + `outdated`. Follow-on: **patch-id-chain carry-over**
so a thread follows a rebased hunk instead of going outdated.

---

## HP-5 — SHA-1 vs SHA-256 (TE-23)

**Resolution (Stage-1 committed).** The data model is **hash-agnostic**; new repos default to **SHA-1 with
git's `sha1dc` collision detection**; **SHA-256 is opt-in per repo** at creation (immutable property).
Imports/mirrors keep their source format. The deciding factor is the **ecosystem, not the cryptography**:
the system of record for source code must interoperate with the world's stock `git` clients/CI/IDE
tooling, and as of v1 the SHA-256 ecosystem (client defaults, SHA-1↔SHA-256 interop, mirror compat) is
not broadly ready — a SHA-256-*default* repo would fail to interoperate with much of a team's existing
toolchain. `sha1dc` mitigates the known (SHAttered) collision class at object-write time. See `01 §3`.

**Prior art.** **SHAttered** (Stevens, Bursztein, Karpman, Albertini, Markov — *The first collision for
full SHA-1*, CRYPTO 2017) — the reason SHA-1's collision resistance is broken and why `sha1dc` exists;
git's SHA-256 object-format (git ≥ 2.42) + its maturing interop layer; the Git 3.0 (late-2026) move toward
SHA-256-viable-default as the trigger for the flip.

**Floor (GF-2b).** SHA-1+`sha1dc` default / SHA-256 opt-in in v1; **the default flip to SHA-256** is the
named (measured) follow-on once stock-client + tooling compatibility is broadly safe; an opt-in
hash-changing `migrate --to sha256` is the disruptive per-repo path. The hash-agnostic data model makes
the flip a default-change, not a migration.

---

## HP-6 — Storage/replication backend (TE-24)

**Resolution.** Covered by HP-1: **the DB ref-store transaction is the linearisation point** (a per-ref
CAS; "outbox order == ref-update order by construction" because the ref lock + outbox row commit in one
transaction, BUS-2) — **Postgres is the Praefect** — with **content-addressed packs in the object tier
made durable by a primary + quorum-ack WAL-streamed replica set** (consistency and durability decoupled).
This gives **linearizable protected-ref merges with no split-brain** without a bespoke per-repo consensus
group. Chosen over whole-repo quorum voting (heavy: replicates bytes through consensus) and over
Raft-on-refs (a rival linearisation authority over the DB that invites split-brain between the two,
`02 §4.1`). The recovery tiebreaker is the DB ref index; `update_seq` is the fence/generation number. See
`02 §4`.

**Prior art.** GitLab **Gitaly-Praefect** (metadata-authority pattern, collapsed onto our OLTP),
GitHub **Spokes/DGit** (quorum), **JGit-DFS / Mononoke** packs-as-blobs; the transactional **outbox**
(Richardson 2018) as the consistency-by-construction mechanism.

**Floor (GF-1/GF-2).** Single-cell primary + quorum replicas; object-backed packs + cross-cell active
sets as the follow-ons. **Owed: D-5** (linearizable merge / no split-brain under failover) + **OQ-4** (the
quorum-ack protocol + failover window).

---

## HP-7 — Git-history author/email erasure + the GD-1 reconciliation (the platform's hardest GDPR problem)

**Resolution (the named GD-1 "Erasure vs. Immutability reconciliation", co-owned with Legal/DPO).**

1. **Pseudonymous-by-default commit identities (GIT-1) — a COMMIT-TIME PREREQUISITE that gates this very
   data model.** Commits are authored to a **stable opaque author id** (`<pseudonym>@<tenant>.noreply`);
   the person↔pseudonym mapping lives in **Id's erasable pseudonym map**. Erasing the person deletes the
   map ⇒ the immutable commit bytes hold only the opaque pseudonym. This makes git-history erasure
   *usually a pseudonym-map delete, not a history rewrite*. It **must be enforced at commit time** —
   nearly impossible to bolt on later — which is why it is decided **before** the git data model is fixed
   (`01 §4.3` stores `author_pseudonym`, never name/email).
2. **Crypto-shred for everything else.** PR/review/comment **bodies** are encrypted under a **per-subject
   DEK**; erasure destroys the DEK (reaches live + backups by construction — ciphertext becomes
   unrecoverable). Reflogs, bitmaps, and pack-tier backups are shreddable via the **per-tenant blob DEK**
   (Storage §5.4). The **search index** is purge+reindex (plaintext-derived, not key-shred).
3. **The residual (the honest hard limit).** Personal data baked into **non-pseudonymised file content or
   commit messages**, and the author bytes of legacy/imported SHA-1 history, are **not** reachable by
   crypto-shred (the bytes are immutable and hash-load-bearing). The only levers are **history-rewrite**
   (filter-repo-class — changes every downstream hash, invalidates clones/signatures/refs;
   tenant-initiated, audited, rate-limited; with fork/mirror/clone-cache invalidation) **or** a
   **documented lawful-basis limit** under Art. 17 "technically infeasible / disproportionate effort".
   The engineering posture is **minimise PII in immutable history so the legal question rarely bites**.

**Prior art.** **`git filter-repo`** / BFG (the history-rewrite mechanism); pseudonymisation + tombstones
(Kleppmann *DDIA* ch.5 — "delete the identity, not the fact"); crypto-shred (Boneh & Lipton, *A Revocable
Backup System*, USENIX Security 1996; NIST SP 800-88r1); GDPR Arts. 5/6/17/18 + the
"technically-infeasible" carve-out. The platform spine (references-not-payloads on the bus, pseudonym
indirection in Id) is what makes the *event-log* half free; this subsystem owns the *git-history* half.

**Floor (GF-7) + `[OPEN — LEGAL]` (GD-1/L-2).** Pseudonymous-commit-by-default is DECIDED;
history-rewrite is the supported disruptive path; the **exact Art. 17 reach into immutable commit-object
bytes vs. the documented-lawful-basis-limit is decided by counsel/DPO** before it binds. Carried into the
gap report (E-3) as the *pseudonymous-commit residual limit*. **Owed drill (D-2):**
erasure-reaches-every-holder for a git subject (asserts steps 1-3 hit reflogs/bitmaps/backups/index/refs;
asserts the residual is exactly the immutable-content bytes, nothing more).

---

## HP-8 — Forks / merge-queue / web-edit scope (TE-26)

**Resolution.**
- **Forks** share object storage via a per-network content-addressed object pool (`network_root`,
  git `alternates` model); cross-tenant forks get independent copies (residency-safe). Erasure unaffected
  (handled by HP-7 levers on commit bytes). See `02 §7`.
- **Merge queue** ships as a **single-lane serialised durable workflow** in v1 (GF-8); speculative/parallel
  batching is the demand-triggered follow-on.
- **Web edit** = view + single-file edit + commit; **no 3-way conflict editor** in v1 (GF-6).

**Prior art.** GitHub forks-share-storage (alternates); GitHub **merge-queue** (the speculative-batch
target); git web-edit UX (GitHub/GitLab single-file edit).

---

## HP-9 — Code-search v1 scope (TE-27)

**Resolution.** Git hosting **owns what to index, Search owns the index** (no cross-DB). v1 is
**symbol/path/literal/trigram-grade**: the code-projection emitter emits, per changed blob, a doc of
{path, language, symbols (camel/snake-split identifiers), string/number literals, commit message}; Search
builds trigram/n-gram indices for substring/regex-lite search. Always ACL-pre-filtered via `list_objects`
(the `search-requires-acl-filter` lint). See `02 §9`, `03 §...`, Search §4.4.

**Prior art.** **Russ Cox**, *Regular Expression Matching with a Trigram Index* (2012) — the
Google-Code-Search/`codesearch` approach; GitHub **Blackbird** as the world-scale-rebuild cautionary tale
(why v1 is scoped down); **SCIP/LSIF** (Sourcegraph/Microsoft) as the named code-intelligence follow-on
fed by CI.

**Floor (GF-3).** v1 = lexical symbol/path/literal/trigram. Follow-on: AST-aware / cross-reference /
"find usages" + code embeddings, fed by **CI-produced SCIP indices** — demand-triggered.

---

## HP-10 — Transactional outbox + per-ref ordering at push QPS

**Resolution.** The ref-update transaction is the **linearisation point** (a `FOR UPDATE` row lock on the
single ref row); the **outbox row commits in that same transaction** (BUS-2), so outbox order == ref-update
order by construction. The **aggregate for `git.ref.updated` is the ref** (`git/ref/<repo>:<ref>`), giving
per-ref order while different refs fan out in parallel (Bus §2.3). Consumers dedup on `event_id`, order on
`(aggregate, seq)`. See `02 §2-3`.

**Prior art.** The **transactional outbox** (Richardson, *Microservices Patterns*, 2018); idempotent
consumers (Helland, *Idempotence Is Not a Medical Condition*, 2012); Kafka partition-key design (Kreps,
2011) applied to "partition by the entity whose order you actually need" — the ref.

**Floor.** None at v1 altitude; the **per-ref-ordering-at-QPS drill (D-1 / Bus D-9)** is the proof.
