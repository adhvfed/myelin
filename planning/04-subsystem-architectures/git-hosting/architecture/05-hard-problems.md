# 05 — Hard Problems Resolved (with cited prior art)

> Each subsystem-specific hard problem, the resolution, the prior art it stands on, and the named floor
> where v1 is partial. Conformed to the frozen reconciled contracts (X-1 check seam, the OQ-D anchor
> resolver, the X-7 erasure posture). Cross-refs the mechanism in `02-internals-and-algorithms.md`. Date:
> 2026-06-19.

---

## HP-0 — The Git↔CI check seam + merge gate (X-1 / OQ-A — the single most load-bearing cross-subsystem seam)

**Resolution (frozen, contract 5.9).** Git is the **consumer + gate**; CI is the producer. CI emits one
shared **`CheckStatus` fact** per `(commit_oid, context)` as `ci.check.updated` over the durable bus; Git
mirrors it into a **`check_status` projection table** that drives the merge gate. The load-bearing rules:
**Git owns which contexts are `required`** (branch-protection policy — CI reports facts, Git decides which
gate); supersession is **monotonic on `run_attempt`**, not wall-clock (a lower attempt arriving late is
dropped — the bus is at-least-once, so the drop is mandatory); Git **reads `trust_tier` off the fact and
never recomputes trust**; Git **never synchronously calls CI** (it reads its own projection — the dependency
stays acyclic). An `untrusted_fork` success is **neutral for gating** until a maintainer endorses it via
`check(subject, approve_untrusted_ci, repo)` (the frozen ReBAC relation) or it is re-run `trusted` — the
poisoned-pipeline defence. The merge queue is a **durable workflow** per target ref that waits on the rollup
**`ci.result` signal** (distinct from the per-context events) via the `SCHEDULE_AND_RUN_JOB` long-park idiom
— it holds no runtime while CI runs for hours. See `02 §6`, `03 §1.1`.

**Prior art.** GitHub commit-status / checks API + branch protection (the consumer-decides-which-gate model);
the poisoned-pipeline-execution attack class (the reason a fork cannot green its own gate — EI-02 §1); the
transactional outbox (Richardson 2018) + idempotent consumers (Helland 2012) for the at-least-once
supersession; durable workflow / "park on a signal" (Temporal/Cadence; AWS Step Functions) for the
multi-hour `ci.result` wait without holding runtime.

**Floor.** None at the contract altitude — the seam is frozen. The **single-lane** merge queue is GF-8.
**Owed drill (D-10):** the X-1 supersession + fork-endorsement + `ci.result`-wait correctness drill.

---

## HP-1 — World-scale git storage (object-backed packs, sharding/replication, smart-transport)

**Resolution.** Repo is the unit of placement. The **linearisation point is the DB ref-store transaction**
(a per-ref CAS, `02 §3-4`) — *not* a bespoke per-repo consensus group — because the git subsystem already
owns the ref lock and the outbox row commits in the same transaction (BUS-2): "outbox order == ref-update
order by construction." **Postgres is the Praefect.** Bulk **pack bytes are decoupled** into the object tier
behind `BlobStore`, durable via a **primary + quorum-ack WAL-streamed replica set** (consistency and
durability replicated *separately*). Smart-transport is served by **sandboxed canonical `git`** in v1 (TE-8),
with `gix` in-process for read/diff/blame. See `02 §1, §4`.

**Prior art.** GitLab **Gitaly-Praefect** (metadata authority — collapsed onto our OLTP) and GitHub
**Spokes/DGit** (three-way voting) as the menu chosen between; Meta **Mononoke/Sapling** (Rust git server,
packs in scalable blob storage — Meta Eng. 2022) and **JGit-DFS / Gerrit pluggable backends** for the
byte-decoupling; **Venti** (Quinlan & Dorward, FAST 2002) for content-addressed storage. We deliberately did
**not** stand up a per-repo **Raft** group (Ongaro & Ousterhout, USENIX ATC 2014) — the DB transaction
already provides the linearisation authority a Raft cohort would add; a rival consensus log invites
split-brain *between* the two (`02 §4.1`).

**Floor (GF-1/GF-2).** v1 packs on local NVMe behind `BlobStore` (relocatable, never node-pinned — STOR-5);
single-cell primary + quorum replicas. Follow-on: object-store-backed pack/delta + smart-transport over
`BlobStore`; cross-cell active replica sets within-EU.

---

## HP-2 — Git-core build-vs-embed (TE-8): gix vs libgit2 vs shell-out

**Resolution (Stage-1 web-verified, carried forward).** A layered **`GitCore`** strategy: **canonical `git`**
(shelled-out, sandboxed under the X-6 profile, streamed) for **wire serving** + **maintenance**; **`gix`
preferred, `libgit2` fallback, IN-PROCESS** for read/diff/blame/projection. The deciding fact: Stage-1
re-verified (2026-06) that **`gix` has NO server-side `upload-pack`/`receive-pack`** (only the transport
*client*) — so canonical `git` serves the wire. The in-process Rust policy engine still wraps the shelled
`receive-pack` (quarantine → policy → our ref CAS + outbox in one txn). Ops migrate gix-ward per-op **iff**
the OQ-1 spike clears. See `01 §2`, `02 §2`.

**Prior art.** **gitoxide** (pure-Rust git — the aspiration); the GitLab/Gitea **shell-out-to-`git`**
baseline (the proven server path adopted for the wire); **Mononoke** as the migration-target existence proof.

**Floor.** "Wire serving on canonical `git`" is the v1 reality (not a temporary fallback); the follow-on is
"migrate wire ops to a `gix` server per-op once it ships + passes protocol-compat & sandbox-escape drills."
**Owed spike (OQ-1).**

---

## HP-3 — Monorepo ambition (TE-25)

**Resolution.** Support **large-but-normal monorepos** via **partial-clone**, **sparse-checkout/
sparse-index**, **shallow**, and **mandatory fresh commit-graph + reachability bitmaps + MIDX**. We **do
not** build a Mononoke-class virtual filesystem in v1. The threshold is **benchmarked, not guessed** (Phase-1
§3.2). See `02 §1.3`.

**Prior art.** GitHub/GitLab **partial-clone**; Microsoft **Scalar / VFS-for-Git**; git **commit-graph +
reachability bitmaps**; Meta **Mononoke/EdenFS** as the named out-of-scope hyperscale path.

**Floor (GF-4).** v1 caps at benchmarked monorepo limits; Mononoke-class backend is the demand-triggered
follow-on. **Owed drill (D-4):** a monorepo ceiling benchmark.

---

## HP-4 — Diff/comment anchoring across rewrites (TE-22) — now the OQ-D content-fingerprint resolver

**Resolution (frozen, contract 5.7).** Store a **content anchor** — `(anchor_blob_oid, path, side,
line-range, anchored_commit_oid, anchor_fingerprint = BLAKE3(anchored lines + context window))` — and
resolve against a newer blob into exactly one of the unified ladder's four states: **exact (LIVE) / rebased
(MOVED) / partial (OUTDATED) / tombstone (content_gone, GONE)**. The fingerprint makes the `rebased→MOVED`
3-way-context match reliable rather than a guess; the four states are the *same* ladder Knowledge's
block/heading/row anchors and Chat's message anchors use, so a tombstone always carries the root PR. Follow
renames via similarity detection; never silently move a comment wrong; offer "view in original context". See
`02 §5`.

**Prior art.** **Myers** *An O(ND) Difference Algorithm* (1986) / git's xdiff (via **imara-diff**) for the
line mapping; **BLAKE3** (the platform content-hash) for the anchor fingerprint; GitHub/GitLab's
content-anchor + outdated-thread model as the proven UX; git's `patch-id` for the follow-on rebase
carry-over.

**Floor (GF-5).** v1 does per-pair fingerprint remap + the four states. Follow-on: **patch-id-chain
carry-over** so a thread follows a rebased hunk through a multi-commit rebase. **Owed drill (D-7).**

---

## HP-5 — SHA-1 vs SHA-256 (TE-23)

**Resolution (carried forward).** Hash-agnostic model; new repos default to **SHA-1 + `sha1dc`**; **SHA-256
opt-in per repo** (immutable). The deciding factor is the **ecosystem, not the cryptography**: the system of
record must interoperate with stock `git`/CI/IDE tooling, and the SHA-256 ecosystem is not broadly ready —
a SHA-256-*default* repo would fail to interoperate. `sha1dc` mitigates the SHAttered class at write time.
See `01 §3`.

**Prior art.** **SHAttered** (Stevens et al., CRYPTO 2017); git's SHA-256 object-format (≥ 2.42) + maturing
interop; Git 3.0 (late-2026) as the flip trigger.

**Floor (GF-2b).** SHA-1+`sha1dc` default / SHA-256 opt-in; the **default flip** is the measured follow-on;
hash-agnostic model makes it a default-change, not a migration. **OQ-9:** the flip trigger.

---

## HP-6 — Storage/replication backend (TE-24)

**Resolution.** Covered by HP-1: the DB ref-store transaction is the linearisation point ("outbox order ==
ref-update order by construction", BUS-2) — **Postgres is the Praefect** — with content-addressed packs in
the object tier durable via a primary + quorum-ack WAL replica set (consistency and durability decoupled).
Linearizable protected-ref merges, no split-brain, no bespoke per-repo consensus group. The recovery
tiebreaker is the DB ref index; `update_seq` is the fence. See `02 §4`.

**Prior art.** GitLab **Gitaly-Praefect**, GitHub **Spokes/DGit**, **JGit-DFS / Mononoke** packs-as-blobs;
the transactional **outbox** (Richardson 2018).

**Floor (GF-1/GF-2).** Single-cell primary + quorum replicas; object-backed packs + cross-cell active sets
as follow-ons. **Owed: D-5** + **OQ-4** (quorum-ack protocol + the object-backed pack layout).

---

## HP-7 — Git-history author/email erasure — instantiating the ONE platform erasure posture (X-7 / contract 10.9)

**Resolution.** Reconciliation replaced five subsystem-local erasure write-ups with **ONE platform-wide
posture** (contract 10.9 / recon §X-7). This subsystem **instantiates it by reference** — it does *not*
author a Git-local residual statement. Git's *mechanism* half:

1. **Pseudonymous-by-default commit identities (GIT-1) — a COMMIT-TIME PREREQUISITE that gates this data
   model.** Commits are authored to a **stable opaque pseudonym** (`<pseudonym>@<tenant>.noreply`, the frozen
   grammar — contract 4.8); the person↔pseudonym map is Id's erasable record. Erasing the person deletes the
   map ⇒ the immutable commit bytes hold only the opaque pseudonym (DSR step 1). This makes git-history
   erasure *usually a pseudonym-map delete, not a rewrite*. It **must be enforced at commit time** — nearly
   impossible to bolt on later — which is why it is decided **before** the git data model is fixed (`01 §4`
   stores `author_pseudonym`, never name/email).
2. **Per-subject DEK crypto-shred for self-authored bodies.** PR/review/comment **bodies + titles** are
   encrypted under a **per-subject DEK** (contract 11.4); erasure destroys the DEK (reaches live + backups
   by construction). Reflogs, bitmaps, pack-tier backups are shreddable via the per-tenant blob DEK; the
   search index is purge+reindex.
3. **The residual is THE platform posture's residual, not a Git-local one.** Third-party free-text PII (a
   name typed by someone else into their own commit message / comment body) and immutable-byte content
   authored by others are handled under the **ONE documented lawful-basis posture** (contract 10.9): best-
   effort `rectify`/tombstone + the standing `restrict` suppression, plus **(a)** the pseudonymous-by-default
   floor (covers author identity) and **(b)** the **history-rewrite erasure path** — an **audited,
   tamper-evident, rate-limited tenant op** (contract 10.6) with fork/mirror/clone-cache invalidation fan-out
   — for the rare case a body must be expunged, with the understood consequence of changed hashes.

**Prior art.** **`git filter-repo`** / BFG (the history-rewrite mechanism); pseudonymisation + tombstones
(Kleppmann *DDIA* ch.5 — "delete the identity, not the fact"); crypto-shred (Boneh & Lipton, *A Revocable
Backup System*, USENIX Security 1996; NIST SP 800-88r1); GDPR Arts. 5/6/17/18 + the "technically-infeasible"
carve-out. The platform spine (references-not-payloads on the bus, pseudonym indirection in Id) makes the
*event-log* half free; this subsystem owns the *git-history* half of the ONE posture.

**Floor (GF-7) + `[OPEN — LEGAL]`.** Pseudonymous-commit-by-default is DECIDED; history-rewrite is the
supported disruptive audited path; the **Art. 17 reach into immutable commit bytes vs. the
documented-lawful-basis limit is ratified by counsel/DPO as ONE statement, not five** (contract 10.9 / recon
§X-7). The structural floor ships regardless. **Owed drill (D-2):** erasure-reaches-every-holder for a git
subject (asserts steps 1-2 hit reflogs/bitmaps/backups/index/refs; asserts the residual is exactly the
platform-posture residual, nothing more).

---

## HP-8 — Forks / merge-queue / web-edit scope (TE-26)

**Resolution.**
- **Forks** share object storage via a per-network content-addressed object pool (`network_root`, git
  `alternates`); cross-tenant forks get independent copies (residency-safe); erasure unaffected (HP-7
  levers). **Fork-PR runs are `untrusted_fork`** (X-1): cache writes confined to `fork:<pr_id>` scope
  (Storage 11.2 C4); the fork cannot reach the trusted cache *or* the trusted gate. See `02 §7`.
- **Merge queue** ships as a **single-lane serialised durable workflow** in v1 (GF-8), woken by the `ci.result`
  durable signal; speculative/parallel batching is the demand-triggered follow-on (OQ-5).
- **Web edit** = view + single-file edit + commit; **no 3-way conflict editor** in v1 (GF-6).

**Prior art.** GitHub forks-share-storage (alternates); GitHub **merge-queue** (the speculative-batch
target); git web-edit UX.

---

## HP-9 — Code-search v1 scope (TE-27)

**Resolution.** Git **owns what to index, Search owns the index** (no cross-DB). v1 is
**symbol/path/literal/trigram-grade**: the code-projection emitter emits, per changed blob, {path, language,
symbols (camel/snake-split), literals, commit message, text}; Search builds trigram indices. Always
ACL-pre-filtered via the **OQ-E `Filter`** (the `search-requires-acl-filter` lint, contract 6.1). See `02
§9`, `03 §5.3`, Search §4.4.

**Prior art.** **Russ Cox**, *Regular Expression Matching with a Trigram Index* (2012) — the
Google-Code-Search/`codesearch` approach; GitHub **Blackbird** as the world-scale-rebuild cautionary tale
(why v1 is scoped down); **SCIP/LSIF** (Sourcegraph/Microsoft) as the named code-intelligence follow-on fed
by CI (contract 6.5).

**Floor (GF-3).** v1 = lexical symbol/path/literal/trigram. Follow-on: AST-aware "find usages" + code
embeddings fed by **CI-produced SCIP indices** — demand-triggered.

---

## HP-10 — Transactional outbox + per-ref ordering at push QPS

**Resolution.** The ref-update transaction is the **linearisation point** (a `FOR UPDATE` row lock on the
single ref row); the **outbox row commits in that same transaction** (BUS-2), so outbox order == ref-update
order by construction. The **aggregate for `git.ref.updated` is the ref** (`git/ref/<repo>:<ref>`), giving
per-ref order while different refs fan out in parallel (contract 2.3). Consumers dedup on `event_id`, order
on `(aggregate, seq)`. See `02 §2-3`.

**Prior art.** The **transactional outbox** (Richardson, *Microservices Patterns*, 2018); idempotent
consumers (Helland, *Idempotence Is Not a Medical Condition*, 2012); Kafka partition-key design (Kreps, 2011)
applied to "partition by the entity whose order you actually need" — the ref.

**Floor.** None at v1 altitude; the **per-ref-ordering-at-QPS drill (D-1 / Bus D-9)** is the proof.
