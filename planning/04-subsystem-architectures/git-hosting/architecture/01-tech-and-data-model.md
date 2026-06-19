# 01 — Technology, Git-Core Embedding, and the Data Model

> Resolves the language/DB choice (with written justification), the git-core build-vs-embed call (TE-8),
> the SHA-1/SHA-256 call (TE-23), and lays out the complete data model: the git object tier and the
> hosting OLTP. Cited prior art inline. Date: 2026-06-19.

---

## 1. Language / tools / database — the choice + written justification

| Layer | Choice | Justification (with citation) |
|---|---|---|
| **Service language** | **Rust** | ADR-02/14 default; the hot serving path (pack/delta, reachability, streaming transport) is exactly the latency/throughput/memory-safety profile Rust targets. **Mononoke** (Meta's Rust git/hg server backing a monorepo of millions of files) is the existence proof a world-scale Rust git server is feasible (Sapling/Mononoke, Meta Eng. 2022). No divergence proposed. |
| **Git core** | **Layered `GitCore` trait. Canonical `git` (shelled-out, sandboxed, streamed) for WIRE SERVING + maintenance — the only complete server-side path (Stage-1 re-verified: `gix` has NO server-side `upload-pack`/`receive-pack`); `gix` (gitoxide) preferred / `libgit2` fallback IN-PROCESS for read/diff/blame/the code projection.** See §2 — the TE-8 resolution. | TE-8 (Stage-1 web-verified 2026-06); `gix` server-side gap re-assessed in §2. |
| **Hosting metadata DB** | **PostgreSQL** (OLTP), JSONB where flexible (ruleset predicates, diff-anchor side-tables), per-tenant envelope-encrypted, residency-pinned, RLS. | ADR-10/14; the substrate OLTP tier (Storage §3.1). PG gives the transactional outbox in the *same DB* as the domain rows — the BUS-2 invariant (emit iff committed) needs exactly one transaction across rows + outbox. |
| **Ref store** | **reftable format, stored as rows in PostgreSQL** (not on a filesystem). See §4 of `02-internals-and-algorithms.md`. | JGit/Gerrit reftable (Hanwen Lin, *reftable* spec, upstreamed into git ~2.45); filesystem `packed-refs` is pathological at millions of refs (Phase-1 §2.3). |
| **Pack/LFS/bundle/backup bytes** | **`BlobStore` (Storage T2)** — content-addressed (BLAKE3 hash-on-write), per-tenant dedup, residency-pinned. v1 packs on local NVMe behind the trait; STOR-5 keeps the object-backed swap a one-line change. | Storage §3.2, §3.5; Venti content-addressed storage (Quinlan & Dorward, FAST 2002). |
| **Code search** | **Shared Search (Tantivy)**; git hosting emits the projection, Search owns the index. | Search §4.4; Cox trigram index (2012). |
| **Diff / anchor / projection / merge-gate** | **Rust services in the control plane**, using `gix` + `imara-diff` (Rust Myers/Histogram diff) for diffing. | imara-diff (Rust port of git's xdiff); Myers, *An O(ND) Difference Algorithm* (1986). |
| **Front-door SSH** | **`russh` (pure-Rust SSH server)** in-process, with `AuthorizedKeysCommand`-style lookup against Id. | russh; avoids the per-connection `sshd` fork model (GitLab's `gitlab-shell` lesson — an in-process SSH server scales connection handling far better). |
| **Front-door HTTP** | **`axum`/`hyper`** (the substrate's HTTP stack), streaming bodies. | substrate `00 §4`. |

**EU-deployable / self-hostable confirmation.** Every component above is OSS and self-hostable on
EU-controlled infrastructure: Rust binaries, PostgreSQL, MinIO/Ceph for the object tier, NATS JetStream
for the bus, no US-SaaS dependency. The git wire protocol is an open standard; stock `git` clients work
unmodified. **Glue-contract implementability across a language boundary**: there is *no* language
boundary here — the whole subsystem is Rust, so it consumes the Rust-shaped glue crates (`myelin-events`,
`myelin-refs`, `myelin-identity`, `myelin-gdpr`, `myelin-client`, `myelin-substrate`) directly, with no
wire re-implementation needed (unlike the Chat connection tier, TE-21). This is the simplest possible
position on contract X-5.

---

## 2. The git-core embed decision (TE-8) — resolved (Stage-1 web-verified)

**Decision (a layered `GitCore` strategy, the Stage-1-verified position): use canonical `git`
(shelled-out, sandboxed, streamed) for WIRE SERVING (`upload-pack`/`receive-pack`/`ls-refs`) and
maintenance (repack/commit-graph/bitmaps/MIDX/bundle); use `gix` (gitoxide), with `libgit2` as a
fallback, IN-PROCESS for read/diff/blame and the code projection. Operations migrate from the shell
backend to `gix` per-op as `gix` ships server-side support.**

### 2.1 The load-bearing fact (re-verified against the live web, Stage-1)

`gix`/gitoxide is the right *aspiration* (pure-Rust, no FFI/unsafe-C surface, same language as the
service, fast object/pack/diff plumbing) — but **Stage-1 re-verified against the current gitoxide
release (2026-06) that `gix` has NO server-side `upload-pack`/`receive-pack`**: it ships the transport
*client*, not the server negotiation/pack-generation state machines (GitoxideLabs/gitoxide discussion
#1299). **A pure-`gix` server is therefore not viable for v1.** This flips the earlier (training-knowledge)
"build receive-pack on gix" sketch to the honest position: **canonical `git` serves the wire in v1.** This
is *not* a fallback-we-might-need — it is the v1 default for the wire path.

### 2.2 The shape, and why not the alternatives

- **Why canonical `git` for the wire + maintenance.** It is the *only* complete, battle-tested
  server-side implementation of protocol-v2 negotiation, pack-objects generation, partial-clone filters,
  and the maintenance ops (geometric repack, commit-graph, reachability bitmaps, MIDX, bundle-create).
  Re-implementing these is a multi-year effort and a correctness/security minefield on the system of
  record for source code. It runs **sandboxed** (ADR-20/CI-1 unified sandbox profile: egress-deny,
  read-only root + tmpfs scratch, caps dropped, CPU/mem/time-capped) — it processes **untrusted client
  packs**, so the serving-tier `git` process is hardened platform code (the sandbox profile is OQ-3).
  Output is **streamed** (no whole-pack buffering).
- **Why `gix` in-process for read/diff/blame/projection.** These are the hot, high-fan-out *read* paths
  the front end and the code projection hammer; in-process `gix` avoids a `git` fork per diff and gives a
  typed Rust object model (no pipe parsing). `gix`'s read/plumbing layer (object DB, pack decode, delta
  resolution, commit-graph traversal, `imara-diff`) is mature for exactly these.
- **Why `libgit2` as the in-process *fallback* (not primary, not absent).** Where `gix` lacks a read
  capability (e.g. a niche merge-base or rename-detection corner), `libgit2` bindings cover it in-process
  without shelling out — it is more complete than `gix` today for some reads. It is a fallback because it
  carries an unsafe C FFI surface; `gix` is preferred where it suffices.
- **The push-path nuance (in-process policy *around* a shelled receive-pack).** We still want the
  push-policy engine and the outbox write in **one DB transaction** (BUS-2). We get this by running
  `git receive-pack` into a **quarantine** object dir and wrapping it: `git` does the pack-ingest +
  negotiation; **our in-process Rust policy engine** evaluates the proposed ref moves *before* they are
  applied, and **our** code performs the ref CAS + outbox insert in one DB txn (`02 §2-3`). So "shell out
  for the wire" and "in-process policy + outbox in one transaction" are not in tension — the shell does
  the byte plumbing, our Rust owns the decision and the transaction.
- **Why NOT shell-out for *everything*.** Forking `git` per read (diff/blame) is poor at the read
  fan-out the UI + projection generate; `gix` in-process is the win there.
- **Why NOT build a Mononoke (TE-25 line).** Out of scope for v1; see `05 §HP-3`.

### 2.3 The `GitCore` seam (illustrative)

```rust
/// Strategy pattern: each op declares its backend; the wire/maintenance ops are Shell in v1,
/// the read ops are Gix (libgit2 fallback). Ops migrate Shell→Gix per-op as gitoxide ships them.
pub trait GitCore {
    // WIRE + MAINTENANCE — canonical `git`, sandboxed + streamed (v1):
    fn advertise_refs(&self, repo: RepoLoc, svc: Service) -> Result<RefAdvertisement>;
    fn upload_pack(&self, repo: RepoLoc, neg: Negotiation, out: &mut dyn Write) -> Result<()>;
    fn receive_pack_into_quarantine(&self, repo: RepoLoc, pack: impl Read) -> Result<ProposedRefUpdates>;
    fn repack(&self, repo: RepoLoc, s: RepackStrategy) -> Result<RepackReport>;
    fn write_commit_graph(&self, repo: RepoLoc) -> Result<()>;   // + bitmaps, MIDX, prune, bundle-create
    // READ — gix (libgit2 fallback), in-process:
    fn read_blob(&self, repo: RepoLoc, oid: Oid) -> Result<Bytes>;
    fn diff_blobs(&self, a: Oid, b: Oid) -> Result<Diff>;        // imara-diff
    fn blame(&self, repo: RepoLoc, path: &str, at: Oid) -> Result<Blame>;
    fn walk_for_projection(&self, repo: RepoLoc, range: OidRange) -> Result<ChangedBlobs>;
}
// ShellGitCore: sandboxed canonical-git (wire+maint). GixCore: in-process gix (read), libgit2 fallback.
// A per-op capability table routes each call; wire/maint ops migrate Gix-ward IFF the OQ-1 spike clears.
```

### 2.4 The migration / honesty

The follow-on is to move wire-serving ops from `ShellGitCore` to a `gix` server implementation **once it
ships and passes a protocol-compat + sandbox-escape drill** — a per-op swap, not a rewrite, behind the
`GitCore` seam. **Owed spike (OQ-1):** a capability-matrix spike that runs the current gitoxide
`receive-pack`/`pack-objects`/maintenance against a corpus and records which ops can move off the shell —
gating any such migration. This is an explicit open item (`07 §OQ-1`); the seam exists precisely so the
gitoxide bet is *swappable*, not load-bearing for v1.

---

## 3. SHA-1 vs SHA-256 (TE-23) — resolved (Stage-1 committed)

**Decision (Stage-1 committed direction): the data model is HASH-AGNOSTIC; new repositories default to
SHA-1 with git's `sha1dc` collision-detection; SHA-256 is OPT-IN per repo at creation; the format is an
immutable per-repo property. Flipping the *default* to SHA-256 is a named floor (GF-2b), gated on
ecosystem/stock-client maturity.**

### 3.1 Rationale — why SHA-1-default *for now*, not SHA-256-default

- **The deciding factor is the ecosystem, not the cryptography.** SHA-1 *is* cryptographically broken for
  collision resistance (Stevens et al., *SHAttered*, CRYPTO 2017); we do not dispute that. But the
  **system of record for source code must interoperate with the world's stock `git` clients, CI runners,
  IDE integrations, and third-party tooling**, and as of v1 the SHA-256 ecosystem — client defaults,
  SHA-1↔SHA-256 interop maturity, hosting/mirror compatibility — is **not** broadly ready. Shipping
  SHA-256 *as the default* would make a freshly-created Myelin repo fail to interoperate with a large
  fraction of the tools a team already uses. That is a worse day-one outcome than the (already mitigated)
  SHA-1 collision risk. **`sha1dc`** (git's default collision-detection, which rejects SHAttered-class
  colliding objects) is the mitigation we *do* rely on for the SHA-1 default — it detects the known
  attack class at object-write time.
- **SHA-256 is offered, opt-in, for tenants who want it now.** A repo created with
  `--object-format=sha256` is fully supported (git ≥ 2.42; the front door advertises the repo's format
  via protocol-v2 capabilities; a SHA-256 repo requires a modern client). Because Myelin controls the
  server and ships the Myelin CLI, a security-forward tenant can choose SHA-256 per repo with eyes open.
- **The default flips when the ecosystem catches up (GF-2b).** Git 3.0 (late-2026) and the maturing
  interop layer move SHA-256 toward viable-default; **when stock-client + tooling compatibility is
  measured as broadly safe, the default flips to SHA-256** — a named floor, not a bet re-litigated here.
- **Imports/mirrors keep their source format** (we cannot silently rewrite a mirror's hashes).
- **Migration story (named floor):** an opt-in, audited, hash-changing `repo migrate --to sha256`
  (analogous to history-rewrite, GF-7) using git's SHA-1→SHA-256 conversion; communicated as a
  hash-changing operation that invalidates old clones/refs/signatures. Not auto-run.

### 3.0 Hash-agnostic data model (the load-bearing design choice)

Because the *default* may flip and both formats coexist, the data model is **hash-agnostic**: OIDs are
stored as `bytea` (20 bytes SHA-1 / 32 bytes SHA-256), every repo row records its immutable
`object_format`, and no code assumes a hash width. This is the design property that makes the GF-2b flip a
default-change, not a migration.

### 3.2 Interaction with the BlobStore

This is **orthogonal** to the `BlobStore`'s own BLAKE3 content-addressing (Storage §3.2 explicitly: "git
objects keep git's own hashing; this trait is the blob backing, not the git object model"). A SHA-256 git
object is a *byte string* whose bytes are stored as a (BLAKE3-addressed) blob in the object tier. No
conflict.

---

## 4. The data model

Two stores, never cross-read: the **git object tier** (the content-addressed graph + refs) and the
**hosting OLTP** (Postgres). Both are `(tenant, region)`-partitioned, residency-pinned, per-tenant
envelope-encrypted, and `PersonalDataHolder`s.

### 4.1 The git object tier (per repo)

A repository's physical state:

- **Object database** — packfiles (`.pack` + `.idx`), a multi-pack-index (`.midx`), reachability
  **bitmaps**, and a **commit-graph** file, plus loose objects between repacks. Each object is a
  git-hashed (SHA-1+`sha1dc` default / SHA-256 opt-in — §3) blob/tree/commit/tag. The *bytes* are stored through `BlobStore`
  (content-addressed, so identical packs across forks dedup per-tenant). **Relocatable**: a repo's
  physical home is `placement_of(repo_id)`, never a hard node path (STOR-5).
- **Ref store** — see §4.2.
- **Server-side ref namespaces** — `refs/heads/*`, `refs/tags/*`, plus hosting namespaces:
  `refs/pull/<n>/head`, `refs/pull/<n>/merge` (the test-merge commit), `refs/keep/*` (GC pins),
  `refs/notes/*`. PR refs are server-managed and not pushable by clients.

### 4.2 The ref store (reftable-on-OLTP)

Refs live as **reftable-encoded blocks in PostgreSQL**, not on a filesystem. Sketch:

```sql
-- One logical ref store per repo; reftable gives O(log n) lookup + atomic multi-ref txn + reflog.
CREATE TABLE git_ref (
  tenant        text   NOT NULL,
  region        text   NOT NULL,
  repo_id       uuid   NOT NULL,
  ref_name      text   NOT NULL,            -- e.g. 'refs/heads/main'
  target_oid    bytea  NOT NULL,            -- the git object id (20B SHA-1 / 32B SHA-256; hash-agnostic)
  peeled_oid    bytea,                      -- for annotated tags
  update_seq    bigint NOT NULL,            -- per-(repo, ref) monotonic; the per-ref order tiebreaker
  PRIMARY KEY (tenant, repo_id, ref_name)
);
-- The reflog: append-only per ref, drives audit + force-push detection + erasure reach.
CREATE TABLE git_reflog (
  tenant text, region text, repo_id uuid, ref_name text,
  old_oid bytea, new_oid bytea, actor_pseudonym text,  -- PSEUDONYM, not name/email (GIT-1)
  committed_at timestamptz, update_seq bigint, forced boolean
);
CREATE INDEX ON git_ref (tenant, repo_id);
```

**Why OLTP, not files.** (a) millions of refs make loose-ref directories pathological (Phase-1 §2.3);
(b) the ref-update transaction must be the **linearisation point** for per-ref ordering (Bus §2.3) — a
DB transaction gives that for free and lets the outbox row commit atomically with the ref move (BUS-2);
(c) replication of refs becomes WAL replication (one mechanism for refs + metadata, `02 §4`). The
reftable *format* is reused for its compact encoding + reflog model; the *backing* is Postgres rows.
`update_seq` is the `outbox.seq` tiebreaker the relay publishes in order on.

### 4.3 The hosting OLTP (Postgres) — core tables

All carry `(tenant, region)` and the `#[personal_data(...)]` classification on personal fields (GD-12;
`no-untagged-personal-data` lint). Personal-data handling is **references-not-payloads + pseudonym
indirection**: author/committer identity is stored as an **opaque pseudonym** resolved through Id, never
a raw name/email (GIT-1).

```sql
CREATE TABLE repo (
  tenant text, region text, repo_id uuid PRIMARY KEY,
  parent_project uuid,                       -- ReBAC parent (Id namespace)
  slug text, visibility text,                -- private | internal | public
  default_branch text, object_format text,   -- 'sha1' (default; sha1dc) | 'sha256' (opt-in; GF-2b flip)
  fork_parent uuid,                          -- NULL or the network root (§ forks)
  network_root uuid,                         -- the alternates-sharing object pool root
  archived boolean, created_at timestamptz
);

CREATE TABLE pull_request (
  tenant text, region text, repo_id uuid, pr_number int,
  base_ref text, head_repo uuid, head_ref text,
  state text,                                -- open | merged | closed | draft
  title text, body_md text,                  -- body_md: myelin-content markdown subset (KN-2)
  author_pseudonym text,                     -- PSEUDONYM (GIT-1)
  merge_method text, merge_commit_oid bytea,
  base_oid bytea, head_oid bytea,            -- snapshotted for diff/anchor stability
  created_at timestamptz, merged_at timestamptz,
  PRIMARY KEY (tenant, repo_id, pr_number)
);

CREATE TABLE review (
  tenant text, region text, review_id uuid PRIMARY KEY,
  repo_id uuid, pr_number int, reviewer_pseudonym text,
  verdict text,                              -- approve | request_changes | comment
  is_agent boolean, agent_run uuid,          -- agent provenance (ADR-08 legibility)
  submitted_at timestamptz, head_oid_reviewed bytea  -- for "changes since you last reviewed"
);

CREATE TABLE review_comment (                -- inline + thread comments; THE anchoring battleground
  tenant text, region text, comment_id uuid PRIMARY KEY,
  repo_id uuid, pr_number int, thread_id uuid, parent_comment uuid,
  author_pseudonym text, body_md text,
  -- the diff anchor (see 02 §5):
  anchor_blob_oid bytea, anchor_path text, anchor_side text,  -- 'old' | 'new'
  anchor_line int, anchor_line_end int,      -- range support
  anchored_commit_oid bytea,                 -- the commit the anchor was created against
  outdated boolean DEFAULT false,            -- set when force-push/rebase invalidates the anchor
  resolved boolean DEFAULT false,
  created_at timestamptz
);

CREATE TABLE ruleset (                        -- branch protection, ref-pattern-scoped
  tenant text, region text, ruleset_id uuid PRIMARY KEY,
  repo_id uuid, ref_pattern text,             -- glob → compiled to a ReBAC ref-scoped relation
  required_approvals int, require_codeowners boolean, dismiss_stale boolean,
  required_checks text[], linear_history boolean, require_signed boolean,
  block_force_push boolean, block_deletion boolean,
  bypass_principals text[],                   -- audited via git.protection.bypass_used
  agent_needs_human boolean                   -- the agent-vs-human merge policy flag
);

CREATE TABLE merge_queue_entry (
  tenant text, region text, repo_id uuid, base_ref text,
  pr_number int, position int, state text,    -- queued | testing | merged | failed
  enqueued_at timestamptz, workflow_run uuid  -- the durable-workflow handle
);

CREATE TABLE check_status (                    -- consumed from CI; aggregated for the merge gate
  tenant text, region text, repo_id uuid, commit_oid bytea,
  check_name text, conclusion text,            -- success | failure | pending | …
  ci_run_ref text, updated_at timestamptz,
  PRIMARY KEY (tenant, repo_id, commit_oid, check_name)
);

-- CODEOWNERS is parsed from the repo and cached; resolved to required reviewers per changed path.
CREATE TABLE codeowners_cache (
  tenant text, region text, repo_id uuid, ref text,
  rules jsonb,                                 -- compiled path→owners; refreshed on ref.updated
  PRIMARY KEY (tenant, repo_id, ref)
);

-- The CODE PROJECTION cursor: what blob/symbol set has been emitted to Search per ref.
CREATE TABLE code_projection_cursor (
  tenant text, region text, repo_id uuid, ref text,
  last_indexed_oid bytea, updated_at timestamptz,
  PRIMARY KEY (tenant, repo_id, ref)
);
```

Plus the substrate-standard `outbox`, `consumer_dedup` (for events git hosting consumes), and the
auto-registered `PersonalDataHolder` registration.

### 4.4 The personal-data inventory (drives `PersonalDataHolder`, GD-12)

| Where personal data lives | Classification / erasure lever |
|---|---|
| Commit author/committer **identity** | **Pseudonym** in object bytes; real identity in Id's erasable map → `erasure = Pseudonymise` (delete the map). The lever that makes erasure usually free (GIT-1). |
| PR/review/comment **text** (`body_md`) | inline; `erasure = CryptoShred(subject)` (per-subject DEK) — `05 §HP-7`. |
| Personal data **inside file content / commit messages** | the genuinely-hard residual → `erasure = history-rewrite OR documented limit` (GD-1, `05 §HP-7`). |
| **LFS blobs** (may contain PII) | content-addressed in `BlobStore`; `erasure = crypto-shred the blob DEK`. |
| **Reflog, push records, SSH-key fingerprints** | operational PII; pseudonymised actor + `CryptoShred`/`Pseudonymise`; reflogs are shreddable (Storage §5.4). |
| **git-identity ↔ Myelin-user mapping** | this *is* the pseudonym map → owned by Id (`resolve_pseudonym`/`erase`), step 1 of DSR. |

The full erasure algorithm and the GD-1 reconciliation are in
[`05-hard-problems.md`](./05-hard-problems.md) §HP-7.
