# 01 — Technology, Git-Core Embedding, and the Data Model

> Carries forward the Phase-4 language/DB choice (confirmed; reconciliation forced no change — recon §0),
> the git-core build-vs-embed call (TE-8), the SHA-1/SHA-256 call (TE-23); and lays out the data model now
> conformed to the **frozen** reconciled shapes: the `check_status` projection of the X-1 `CheckStatus`
> fact, the content-anchored line-range fingerprint (OQ-D), per-subject DEK for free-text bodies (11.4),
> and the trust-scoped cache / CDN clone-bundle blob classes (11.2). Cited prior art inline. Date:
> 2026-06-19.

---

## 1. Language / tools / database — the choice (CARRIED FORWARD + CONFIRMED)

Reconciliation did not force a language or DB change for this subsystem (recon §0; the punch-list item for
Git, §4, is all about *conforming to frozen contracts*, not re-choosing the stack). The Phase-4 choice
stands:

| Layer | Choice | Justification (with citation) |
|---|---|---|
| **Service language** | **Rust** | ADR-02/14 default; the hot serving path (pack/delta, reachability, streaming transport) is exactly the latency/throughput/memory-safety profile Rust targets. **Mononoke** (Meta's Rust git/hg server backing a monorepo of millions of files) is the existence proof a world-scale Rust git server is feasible (Sapling/Mononoke, Meta Eng. 2022). No divergence. |
| **Git core** | **Layered `GitCore` trait.** Canonical `git` (shelled-out, sandboxed, streamed) for WIRE SERVING + maintenance (the only complete server-side path — Stage-1 re-verified that `gix` has NO server-side `upload-pack`/`receive-pack`); `gix` (gitoxide) preferred / `libgit2` fallback IN-PROCESS for read/diff/blame/projection. See §2. | TE-8 (Stage-1 web-verified 2026-06). |
| **Hosting metadata DB** | **PostgreSQL** (OLTP), JSONB where flexible (ruleset predicates, diff-anchor side-tables), per-tenant envelope-encrypted (per-subject DEK for free-text), residency-pinned, RLS. | ADR-10/14; substrate OLTP tier (Storage §3.1, contract 11.1). PG gives the transactional outbox in the *same DB* as the domain rows — the BUS-2 invariant (emit iff committed) needs exactly one transaction across rows + outbox. |
| **Ref store** | **reftable format, stored as rows in PostgreSQL** (not a filesystem). See §4.2. | JGit/Gerrit reftable (Hanwen Lin, *reftable* spec, upstreamed into git ~2.45); filesystem `packed-refs` is pathological at millions of refs (Phase-1 §2.3). |
| **Pack/LFS/bundle/backup bytes** | **`BlobStore` (Storage T2)** — content-addressed (BLAKE3 hash-on-write), per-tenant dedup, residency-pinned; **+ the within-EU CDN clone/bundle class (11.2 C3) + the trust-scoped cache namespaces (11.2 C4).** v1 packs on local NVMe behind the trait; STOR-5 keeps the object-backed swap a one-line change. | Storage §3.2/§3.5, contract 11.2; Venti content-addressed storage (Quinlan & Dorward, FAST 2002). |
| **Code search** | **Shared Search (Tantivy)**; git hosting emits the projection, Search owns the index (contract 6.3/6.5). | Search §4.4; Cox trigram index (2012). |
| **Diff / anchor / projection / merge-gate** | **Rust services in the control plane**, using `gix` + `imara-diff` (Rust Myers/Histogram diff) + **BLAKE3** for the content-anchor fingerprint. | imara-diff (Rust port of git's xdiff); Myers, *An O(ND) Difference Algorithm* (1986); BLAKE3 (the platform content-hash, contract 11.2). |
| **Front-door SSH** | **`russh` (pure-Rust SSH server)** in-process, with `AuthorizedKeysCommand`-style lookup against Id. | russh; avoids the per-connection `sshd` fork model (GitLab `gitlab-shell` lesson). |
| **Front-door HTTP** | **`axum`/`hyper`** (the substrate's HTTP stack), streaming bodies. | substrate `00 §4`. |

**EU-deployable / self-hostable confirmation.** Every component is OSS and self-hostable on EU-controlled
infrastructure: Rust binaries, PostgreSQL, MinIO/Ceph for the object tier, NATS JetStream for the bus, no
US-SaaS dependency. The git wire protocol is an open standard; stock `git` clients work unmodified.
**Cross-language glue (contract 1.7 / the old X-5):** there is *no* language boundary here — the whole
subsystem is Rust, so it consumes the Rust-shaped glue crates (`myelin-events`, `myelin-refs`,
`myelin-identity`, `myelin-gdpr`, `myelin-client`, `myelin-flow`, `myelin-substrate`) directly. The
cross-language harness shim (contract 1.7) is a no-op for Git; the hatch exists for the Chat connection
tier, not here.

---

## 2. The git-core embed decision (TE-8) — CARRIED FORWARD (Stage-1 web-verified)

**Decision (unchanged): a layered `GitCore` strategy — canonical `git` (shelled-out, sandboxed, streamed)
for WIRE SERVING (`upload-pack`/`receive-pack`/`ls-refs`) and maintenance (repack/commit-graph/bitmaps/MIDX/
bundle-create); `gix` (gitoxide), `libgit2` fallback, IN-PROCESS for read/diff/blame and the code
projection. Ops migrate Shell→Gix per-op as gitoxide ships server-side support.**

### 2.1 The load-bearing fact (re-verified against the live web, Stage-1)

`gix`/gitoxide is the right *aspiration* (pure-Rust, no FFI/unsafe-C surface, same language, fast object/
pack/diff plumbing) — but Stage-1 re-verified against the current gitoxide release (2026-06) that **`gix`
has NO server-side `upload-pack`/`receive-pack`**: it ships the transport *client*, not the server
negotiation/pack-generation state machines (GitoxideLabs/gitoxide discussion #1299). A pure-`gix` server is
therefore not viable for v1. **Canonical `git` serves the wire in v1** — the v1 default for the wire path,
not a fallback-we-might-need.

### 2.2 The shape, and why not the alternatives

- **Why canonical `git` for the wire + maintenance.** It is the *only* complete, battle-tested server-side
  implementation of protocol-v2 negotiation, pack-objects generation, partial-clone filters, and the
  maintenance ops (geometric repack, commit-graph, reachability bitmaps, MIDX, bundle-create).
  Re-implementing these is a multi-year, correctness/security-critical effort on the system of record for
  source code. It runs **sandboxed** under the unified ADR-20 / X-6 hardening profile (egress default-deny,
  read-only root + tmpfs scratch, caps dropped, no-new-privileges, seccomp, CPU/mem/time-capped) — it
  processes **untrusted client packs**, so the serving-tier `git` process is hardened platform code, and the
  **real-kernel escape drill (X-6)** gates it just as it gates CI/agent execution. Output is **streamed** (no
  whole-pack buffering).
- **Why `gix` in-process for read/diff/blame/projection.** These are the hot, high-fan-out *read* paths the
  front end + the code projection hammer; in-process `gix` avoids a `git` fork per diff and gives a typed
  Rust object model. The diff path also feeds the **content-anchor fingerprint** (§4.4, `02 §5`).
- **Why `libgit2` as the in-process *fallback*.** Where `gix` lacks a read capability (a niche merge-base or
  rename-detection corner), `libgit2` bindings cover it in-process without shelling out. Fallback because it
  carries an unsafe C FFI surface; `gix` is preferred where it suffices.
- **The push-path nuance (in-process policy *around* a shelled receive-pack).** The push-policy engine and
  the outbox write run in **one DB transaction** (BUS-2): `git receive-pack` ingests into a **quarantine**
  object dir; **our in-process Rust policy engine** evaluates the proposed ref moves *before* they apply;
  **our** code performs the ref CAS + outbox insert in one DB txn (`02 §2-3`). The shell does the bytes; our
  Rust owns the decision and the transaction.

### 2.3 The `GitCore` seam (illustrative)

```rust
/// Strategy pattern: each op declares its backend; wire/maintenance ops are Shell in v1,
/// read ops are Gix (libgit2 fallback). Ops migrate Shell→Gix per-op as gitoxide ships them.
pub trait GitCore {
    // WIRE + MAINTENANCE — canonical `git`, sandboxed + streamed (v1):
    fn advertise_refs(&self, repo: RepoLoc, svc: Service) -> Result<RefAdvertisement>;
    fn upload_pack(&self, repo: RepoLoc, neg: Negotiation, out: &mut dyn Write) -> Result<()>;
    fn receive_pack_into_quarantine(&self, repo: RepoLoc, pack: impl Read) -> Result<ProposedRefUpdates>;
    fn repack(&self, repo: RepoLoc, s: RepackStrategy) -> Result<RepackReport>;
    fn write_commit_graph(&self, repo: RepoLoc) -> Result<()>;   // + bitmaps, MIDX, prune, bundle-create
    // READ — gix (libgit2 fallback), in-process:
    fn read_blob(&self, repo: RepoLoc, oid: Oid) -> Result<Bytes>;
    fn diff_blobs(&self, a: Oid, b: Oid) -> Result<Diff>;        // imara-diff; feeds the anchor remap
    fn blame(&self, repo: RepoLoc, path: &str, at: Oid) -> Result<Blame>;
    fn walk_for_projection(&self, repo: RepoLoc, range: OidRange) -> Result<ChangedBlobs>;
}
// ShellGitCore: sandboxed canonical-git (wire+maint). GixCore: in-process gix (read), libgit2 fallback.
// A per-op capability table routes each call; wire/maint ops migrate Gix-ward IFF the OQ-1 spike clears.
```

### 2.4 The migration / honesty

The follow-on is to move wire-serving ops from `ShellGitCore` to a `gix` server implementation **once it
ships and passes a protocol-compat + sandbox-escape drill** — a per-op swap, not a rewrite, behind the
`GitCore` seam. **Owed spike (OQ-1):** a capability-matrix spike running the current gitoxide
`receive-pack`/`pack-objects`/maintenance against a corpus, recording which ops can move off the shell —
gating any migration. The seam exists precisely so the gitoxide bet is *swappable*, not load-bearing.

---

## 3. SHA-1 vs SHA-256 (TE-23) — CARRIED FORWARD (Stage-1 committed)

**Decision (unchanged): the data model is HASH-AGNOSTIC; new repositories default to SHA-1 with git's
`sha1dc` collision-detection; SHA-256 is OPT-IN per repo at creation (immutable per-repo property). Flipping
the *default* to SHA-256 is a named floor (GF-2b), gated on ecosystem/stock-client maturity.**

### 3.1 Rationale — why SHA-1-default *for now*, not SHA-256-default

- **The deciding factor is the ecosystem, not the cryptography.** SHA-1 *is* cryptographically broken for
  collision resistance (Stevens et al., *SHAttered*, CRYPTO 2017); we do not dispute that. But the **system
  of record for source code must interoperate with the world's stock `git` clients, CI runners, IDE
  integrations, and third-party tooling**, and as of v1 the SHA-256 ecosystem — client defaults, SHA-1↔
  SHA-256 interop maturity, hosting/mirror compatibility — is **not** broadly ready. A SHA-256-*default*
  repo would fail to interoperate with much of a team's toolchain — a worse day-one outcome than the
  (already mitigated) SHA-1 collision risk. **`sha1dc`** (git's default collision-detection) is the
  mitigation we rely on for the SHA-1 default — it detects the SHAttered attack class at object-write time.
- **SHA-256 is offered, opt-in.** A repo created with `--object-format=sha256` is fully supported (git ≥
  2.42; the front door advertises the repo's format via protocol-v2 capabilities; a SHA-256 repo requires a
  modern client). A security-forward tenant chooses SHA-256 per repo with eyes open.
- **The default flips when the ecosystem catches up (GF-2b).** Git 3.0 (late-2026) + the maturing interop
  layer move SHA-256 toward viable-default; **when stock-client + tooling compatibility is measured as
  broadly safe, the default flips** — a named floor, measured not re-litigated.
- **Imports/mirrors keep their source format** (we cannot silently rewrite a mirror's hashes).
- **Migration story (named floor):** an opt-in, audited, hash-changing `repo migrate --to sha256`
  (analogous to history-rewrite), communicated as invalidating old clones/refs/signatures. Not auto-run.

### 3.0 Hash-agnostic data model (the load-bearing design choice)

OIDs are stored as `bytea` (20 bytes SHA-1 / 32 bytes SHA-256), every repo row records its immutable
`object_format`, and no code assumes a hash width. This makes the GF-2b flip a default-change, not a
migration.

### 3.2 Interaction with the BlobStore

Orthogonal to the `BlobStore`'s own BLAKE3 content-addressing (Storage §3.2: "git objects keep git's own
hashing; this trait is the blob backing, not the git object model"). A SHA-256 git object is a *byte
string* stored as a (BLAKE3-addressed) blob in the object tier. No conflict. (The same BLAKE3 also produces
the **content-anchor fingerprint** of §4.4 — a different use of the same hash, over the *lines* of a blob.)

---

## 4. The data model

Two stores, never cross-read: the **git object tier** (the content-addressed graph + refs) and the
**hosting OLTP** (Postgres). Both are `(tenant, region)`-partitioned, residency-pinned, per-tenant
envelope-encrypted (per-subject DEK for free-text bodies), and `PersonalDataHolder`s (holder H1).

### 4.1 The git object tier (per repo)

A repository's physical state:

- **Object database** — packfiles (`.pack` + `.idx`), a multi-pack-index (`.midx`), reachability
  **bitmaps**, a **commit-graph**, plus loose objects between repacks. Each object is a git-hashed
  (SHA-1+`sha1dc` default / SHA-256 opt-in — §3) blob/tree/commit/tag. The *bytes* are stored through
  `BlobStore` (content-addressed, so identical packs across forks dedup per-tenant). **Relocatable**: a
  repo's physical home is `placement_of(repo_id)` (contract 12.2), never a hard node path (STOR-5).
- **Ref store** — see §4.2.
- **Server-side ref namespaces** — `refs/heads/*`, `refs/tags/*`, plus hosting namespaces:
  `refs/pull/<n>/head`, `refs/pull/<n>/merge` (the test-merge commit), `refs/keep/*` (GC pins),
  `refs/notes/*`. PR refs are server-managed and not pushable by clients.
- **Clone bundles** are written as the **within-EU CDN clone/bundle blob class** (contract 11.2 C3) —
  content-addressed T2 blobs whose edge POPs are pinned within the tenant's region (no extra-EU edge for
  PII-bearing content; `residency_verify` covers the CDN edge set).

### 4.2 The ref store (reftable-on-OLTP)

Refs live as **reftable-encoded blocks in PostgreSQL**:

```sql
-- One logical ref store per repo; reftable gives O(log n) lookup + atomic multi-ref txn + reflog.
CREATE TABLE git_ref (
  tenant        text   NOT NULL,
  region        text   NOT NULL,
  repo_id       uuid   NOT NULL,
  ref_name      text   NOT NULL,            -- e.g. 'refs/heads/main'
  target_oid    bytea  NOT NULL,            -- 20B SHA-1 / 32B SHA-256; hash-agnostic
  peeled_oid    bytea,                      -- for annotated tags
  update_seq    bigint NOT NULL,            -- per-(repo, ref) monotonic; the per-ref order tiebreak + fence
  PRIMARY KEY (tenant, repo_id, ref_name)
);
-- The reflog: append-only per ref; drives audit + force-push detection + erasure reach.
CREATE TABLE git_reflog (
  tenant text, region text, repo_id uuid, ref_name text,
  old_oid bytea, new_oid bytea, actor_pseudonym text,  -- PSEUDONYM, not name/email (GIT-1; contract 4.8)
  committed_at timestamptz, update_seq bigint, forced boolean
);
CREATE INDEX ON git_ref (tenant, repo_id);
```

**Why OLTP, not files.** (a) millions of refs make loose-ref directories pathological (Phase-1 §2.3); (b)
the ref-update transaction must be the **linearisation point** for per-ref ordering (contract 2.3) — a DB
transaction gives that for free and lets the outbox row commit atomically (BUS-2); (c) ref replication
becomes WAL replication (one mechanism for refs + metadata, `02 §4`). The reftable *format* is reused for
its compact encoding + reflog model; the *backing* is Postgres rows. `update_seq` is the `outbox.seq`
tiebreak the relay publishes in order on, and the recovery **fence/generation number** (`02 §4.2`).

### 4.3 The hosting OLTP (Postgres) — core tables

All carry `(tenant, region)` and the `#[personal_data(...)]` classification on personal fields
(`no-untagged-personal-data` lint, contract 10.2). Personal-data handling is **references-not-payloads +
pseudonym indirection**: author/committer identity is an **opaque pseudonym** resolved through Id, never a
raw name/email (GIT-1, contract 4.8); free-text **bodies are encrypted under a per-subject DEK** (contract
11.4).

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
  title text,                                -- #[personal_data] possible; per-subject DEK if so
  body_md bytea,                             -- ENCRYPTED myelin-content markdown subset under per-subject DEK
  author_pseudonym text,                     -- PSEUDONYM (GIT-1)
  trust_tier text,                           -- 'trusted' | 'untrusted_fork' — head provenance (X-1 input)
  merge_method text, merge_commit_oid bytea,
  base_oid bytea, head_oid bytea,            -- snapshotted for diff/anchor stability
  created_at timestamptz, merged_at timestamptz,
  PRIMARY KEY (tenant, repo_id, pr_number)
);

CREATE TABLE review (
  tenant text, region text, review_id uuid PRIMARY KEY,
  repo_id uuid, pr_number int, reviewer_pseudonym text,
  verdict text,                              -- approve | request_changes | comment
  is_agent boolean, agent_run uuid,          -- agent provenance (ADR-08 legibility; X-6)
  submitted_at timestamptz, head_oid_reviewed bytea  -- for "changes since you last reviewed"
);

CREATE TABLE review_comment (                -- inline + thread comments; THE anchoring battleground
  tenant text, region text, comment_id uuid PRIMARY KEY,  -- stable opaque #comment-<id> (5.7)
  repo_id uuid, pr_number int, thread_id uuid, parent_comment uuid,
  author_pseudonym text, body_md bytea,      -- body ENCRYPTED under per-subject DEK (11.4)
  -- the CONTENT-ANCHORED line range (the OQ-D #sub fingerprint; see 02 §5):
  anchor_path text, anchor_side text,        -- 'old' | 'new'
  anchor_line int, anchor_line_end int,      -- the L<start>-L<end> sub
  anchor_blob_oid bytea,                     -- the blob the anchor was minted against
  anchored_commit_oid bytea,                 -- the commit the anchor was created against
  anchor_fingerprint bytea,                  -- BLAKE3(anchored lines + context window) — the OQ-D resolver key
  anchor_state text DEFAULT 'live',          -- live | moved | outdated | gone (the unified 4-state ladder, 5.7)
  resolved boolean DEFAULT false,
  created_at timestamptz
);

CREATE TABLE ruleset (                        -- branch protection, ref-pattern-scoped
  tenant text, region text, ruleset_id uuid PRIMARY KEY,
  repo_id uuid, ref_pattern text,             -- glob → compiled to a ReBAC ref-glob-scoped relation
  required_approvals int, require_codeowners boolean, dismiss_stale boolean,
  required_contexts jsonb,                    -- the Git-owned `required`-set policy: [{provider,name}] (X-1)
  linear_history boolean, require_signed boolean,
  block_force_push boolean, block_deletion boolean,
  bypass_principals text[],                   -- audited via git.protection.bypass_used
  agent_needs_human boolean                   -- the agent-vs-human merge policy flag (X-6)
);

CREATE TABLE merge_queue_entry (
  tenant text, region text, repo_id uuid, base_ref text,
  pr_number int, position int, state text,    -- queued | testing | merged | failed
  merge_attempt_id uuid,                      -- the idem_key for the ci.result wait (OQ-F)
  enqueued_at timestamptz, workflow_run uuid  -- the DurableExecutor handle (9.1)
);

-- THE X-1 CONSUMER PROJECTION — Git-owned, fed by ci.check.updated, drives the merge gate.
-- Exactly ONE current row per (commit_oid, context); last-writer-wins by run_attempt (5.9).
CREATE TABLE check_status (
  tenant text, region text, repo_id uuid,
  commit_oid bytea,
  ctx_provider text, ctx_name text,           -- CheckContext = {provider: "ci"|"external", name}
  state text,                                 -- queued|in_progress|success|failure|error|neutral|cancelled
  run_ref text,                               -- myelin://<t>/ci/run/<id> — the producing run
  run_attempt int NOT NULL,                   -- MONOTONIC supersession key (>= stored ⇒ supersede; < ⇒ drop)
  trust_tier text,                            -- 'trusted' | 'untrusted_fork' (stamped by CI; Git only reads)
  endorsed_by text,                           -- pseudonym of the approve_untrusted_ci endorser, or NULL
  details_ref text,                           -- myelin://<t>/ci/run/<id>#step-<n> (jump-to-failure, OQ-D)
  summary_template text, summary_args jsonb,  -- the HumanisedRef (template_key, args) — NEVER a raw string
  cost_settled boolean,                       -- reserve/settle bookend closed (11.7) — not "final" until true
  started_at timestamptz, completed_at timestamptz,
  PRIMARY KEY (tenant, repo_id, commit_oid, ctx_provider, ctx_name)   -- the (commit_oid, context) key
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

Plus the substrate-standard `outbox`, `consumer_dedup` (for `ci.check.updated` / `ci.result` / Identity /
GDPR events Git consumes), and the auto-registered `PersonalDataHolder` registration.

**The `check_status` projection is a derived store** (it is rebuilt by `replay` of CI's `ci.check.updated`,
never restored — contract 2.6). It is the X-1 consumer's source of truth for "may this PR merge?"; the
supersession rule and trust evaluation that maintain it are in `02 §6`.

### 4.4 Content-anchor fingerprint (the OQ-D mechanism, stored shape)

A comment's line-range sub `#L<start>-L<end>` is **content-anchored, not positional** (contract 5.7). At
mint time Git stores, in `review_comment`: `anchor_blob_oid`, `anchor_path`, `anchor_side`, `anchor_line`/
`anchor_line_end`, `anchored_commit_oid`, and **`anchor_fingerprint = BLAKE3(anchored lines + a small
context window)`**. On resolution against a newer blob the resolver (`02 §5`) returns one of the unified
ladder's four states and writes `anchor_state ∈ {live, moved, outdated, gone}`. This is the *stored*
substrate behind the resolver; the algorithm is in `02 §5` and the ladder is Refs' (contract 5.7).

### 4.5 The personal-data inventory (drives `PersonalDataHolder`, holder H1; contract 10.1)

| Where personal data lives | Classification / erasure lever |
|---|---|
| Commit author/committer **identity** | **Pseudonym** in object bytes; real identity in Id's erasable map → `erasure = Pseudonymise` (delete the map, contract 4.8). The lever that makes erasure usually free (GIT-1). |
| PR/review/comment **text** (`body_md`, `title`) | inline; `erasure = CryptoShred(subject:<id>)` — encrypted under the **per-subject DEK** (contract 11.4); reaches live + backups by construction. |
| Personal data **inside file content / commit messages authored by others** | the genuinely-hard residual → the **ONE platform erasure posture** (contract 10.9 / recon §X-7): pseudonymous-by-default + history-rewrite + documented lawful-basis limit. **NOT restated here.** See `05 §HP-7` (which references the posture). |
| **LFS blobs** (may contain PII) | content-addressed in `BlobStore`; `erasure = crypto-shred the blob DEK`. |
| **Reflog, push records, SSH-key fingerprints** | operational PII; pseudonymised actor + crypto-shred via the per-tenant blob DEK (reflogs/bitmaps/pack backups shreddable — Storage §5; the H9 cache/CDN class invalidated on history-rewrite). |
| **git-identity ↔ Myelin-user mapping** | this *is* the pseudonym map → owned by Id (`resolve_pseudonym`/`erase`), DSR step 1 (holder H15). |

The full erasure algorithm and the residual-posture *reference* are in
[`05-hard-problems.md`](./05-hard-problems.md) §HP-7 and [`03-events-contracts-and-glue.md`](./03-events-contracts-and-glue.md) §6.
