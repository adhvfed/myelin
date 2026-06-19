# Sketch 02 — Git-core build-vs-embed (TE-8)

> Exploration note. How the git serving core is realised: `gix` (gitoxide, pure Rust) vs `libgit2`
> bindings vs shell-out to canonical `git`. TE-8 explicitly required **re-verifying gitoxide
> server-side maturity** (Phase-1 §12: "from training knowledge, not re-verified"). I re-verified
> against the web (2026-06). Date: 2026-06-19.

## The re-verified facts (2026-06)

These change the decision from "open" to "clear," so they are load-bearing:

1. **gitoxide (`gix`) still has NO server-side `upload-pack`/`receive-pack`.** As of the latest
   public state, `gix-transport`/`gix-protocol` implement the **client** side of the pack protocol;
   the server side (serving fetch/push over the wire) is a tracked aspiration, not shipped. `gix free
   pack create` can stream a pack, but there is **no production server pack-negotiation path**.
   → *A pure-`gix` server is not viable for v1.* (Phase-1's caution was correct and still holds.)
2. **reftable is production-ready** (Git 2.48–2.51), GitLab uses it for **all new repos** with a
   background migrator, and **Git 3.0 (targeted late 2026) makes reftable the default ref backend.**
   → *reftable is the right ref-store on-disk format now*, not a bet (sketch 04).
3. Canonical `git` itself is the reference implementation of the wire protocol (v2), partial-clone,
   sparse, commit-graph, bitmaps, MIDX, and reftable — all the serving-tier table stakes.

## What the git core must do for us

- **Wire serving:** `upload-pack` (fetch/clone, protocol v2 with server-side ref filtering),
  `receive-pack` (push), partial-clone (`--filter`), shallow, sparse.
- **Object/ref ops:** read blobs/trees/commits/tags; resolve refs (reftable); reachability
  (commit-graph + bitmaps) for diff/merge-base/protected-branch checks.
- **Maintenance:** repack/GC (incremental/geometric), commit-graph + bitmap refresh, MIDX.
- **Read/projection paths (our own):** diff computation, blame, tree listing, the **code projection**
  for Search (sketch 06), the **diff-anchor remap** (sketch 07). These are *our* code over an object
  store, not necessarily the wire path.

## Candidate A — Shell out to canonical `git` for the serving/maintenance hot paths

A Rust service (the front door + control plane) **invokes canonical `git`** (`git-upload-pack`,
`git-receive-pack`, `git repack`, `git commit-graph`, `git maintenance`) as child processes against
the on-disk repo, streaming bytes through without buffering whole packs.

- **Pros:** the **only option that fully supports server-side serving today**; canonical git is the
  most battle-tested, fastest, and most complete (every protocol-v2 feature, reftable, partial-clone);
  zero risk of protocol incompatibility with stock clients (a VISION-level "plain git must just work"
  requirement, Phase-2 §5.1); maintenance commands are first-class.
- **Cons:** process-spawn overhead per request (amortised by the fact that clone/fetch/push are
  already heavyweight); must **sandbox** the child (it runs `pre-receive` policy and touches the FS);
  parsing/streaming the child's stdio is fiddly; couples us to a git binary version (manageable —
  pin + test).
- **Sandbox note:** `receive-pack` runs our `pre-receive` policy hook; per AG-2/CI-1 the doctrine
  wants no host-execution path that bypasses the tool boundary — but **git's own serving is platform
  code, not untrusted customer code**, so it runs in the serving tier's own (hardened, resource-capped)
  process, distinct from the CI/agent untrusted sandbox. The *push-policy* logic is our in-process Rust
  (sketch 03), not a shelled-out user hook.

## Candidate B — `libgit2` bindings (`git2` crate) for in-process object/ref/diff ops

Use `libgit2` in-process for the **read/projection/diff paths** (and potentially an in-process
`receive-pack` via libgit2's smart-protocol support).

- **Pros:** in-process (no spawn) for diff/blame/tree/commit-graph reads — good for the **projection
  API**, **code projection**, and **diff-anchor** services that we call at UI/search QPS; mature,
  widely embedded; C library with solid Rust bindings.
- **Cons:** libgit2's server-side smart-protocol is **less complete/less maintained** than canonical
  git's (partial-clone filters, protocol-v2 nuances, reftable lag); C dependency (memory-safety
  surface, though well-audited); not as fast as canonical git on huge repos.

## Candidate C — `gix` (gitoxide) for the read/projection paths where mature

Use `gix` (pure Rust) for **read-only object access, diff, blame, traversal, pack reading** — the
paths where gitoxide is mature and fast — while **never** relying on it for wire serving.

- **Pros:** pure Rust (no C FFI, memory-safe, matches the Rust-default ethos VISION §4); gitoxide's
  **object access and diff are fast and mature**; it's the future-facing choice as the project grows;
  great fit for the **code-projection** and **diff** services that are pure reads over the object DB.
- **Cons:** **no server-side serving** (fact #1) — cannot be the only library; some areas still
  maturing (reftable read/write support is in progress); we'd own more of the gaps.

## The decision shape — a layered, capability-gated mix (not a single library)

The git core is **not one library** — it's a set of capabilities with different maturity. Map each to
the best-fit engine behind **one internal `GitCore` trait** (strategy pattern, mirrors ADR-12.8 / the
mock→real agent seam), so an engine swap is a config change, not a rewrite:

| Capability | v1 engine | Rationale | Migration target |
|---|---|---|---|
| `upload-pack` / `receive-pack` (wire) | **canonical `git`** (shelled-out, sandboxed, streamed) | only complete server-side option (fact #1); stock-client compat | `gix` server-side when it ships + is drilled |
| repack / GC / commit-graph / bitmaps / MIDX | **canonical `git` maintenance`** | most complete + tuned | gix maintenance as it matures |
| ref store (read/write) | **reftable** via canonical git + a **DB-backed transactional ref index** (sketch 04) | reftable is production-ready (fact #2); DB index gives linearizable protected-ref txns | — |
| object/tree/blob read, diff, blame, merge-base | **`gix` (preferred) with `libgit2` fallback** | pure-Rust, fast, in-process; no spawn at UI/search QPS | consolidate on gix |
| code projection (path/symbols/literals) | **`gix` object read + tree-sitter-lite tokenizer** (sketch 06) | pure-Rust read path | — |
| diff-anchor remap | **`gix` diff** (sketch 07) | in-process, called at comment-render QPS | — |

**Why a trait, not "just shell out":** the read/projection/diff paths are called at **interactive and
search QPS** — shelling out per file-view or per code-search-projection would be a spawn storm. Those
go **in-process via `gix`/`libgit2`**. The heavyweight, complete-coverage wire serving goes
**shell-out to canonical git**. The trait records, per capability, which engine serves it, so when
gitoxide ships server-side serving (and passes our drills) we flip the `upload-pack`/`receive-pack`
rows without touching callers.

## Leaning (committed in findings)

**Layered `GitCore` trait**: **canonical `git` (shelled-out, sandboxed, byte-streamed) for wire serving
+ maintenance** (the only complete server-side option in 2026, fact #1); **`gix` (gitoxide) preferred,
`libgit2` fallback, for in-process read/diff/blame/projection paths**; **reftable** as the on-disk ref
format (fact #2) behind a DB-backed transactional ref index for linearizable protected-ref updates.
The whole subsystem service is **Rust** (no language divergence — Phase-2 §3; Mononoke is the existence
proof). **Named follow-on:** migrate the wire-serving rows to `gix` server-side when it ships and passes
the protocol-compat + escape drills.

## Prior art / sources

- gitoxide server-side status (no upload-pack/receive-pack yet): GitoxideLabs/gitoxide discussion
  #1299; crates.io / docs.rs gitoxide.
- reftable production status + Git 3.0 default: git-scm BreakingChanges; GitLab reftable rollout epic
  #12503; DeployHQ "Git 3.0 on the horizon."
- Meta Mononoke (Rust scalable git server) — the Rust-feasibility existence proof (Phase-2 §3).
- Phase-2 git-hosting §3 (tech table, the `[OPEN → P4, TE-8]` row); ADR-02 ("git serving core stays
  Rust"); ADR-12.8 (swappable adapter mandate).

[Sources: GitoxideLabs/gitoxide discussion #1299; git-scm.com/docs/BreakingChanges; gitlab.com epics/12503;
deployhq.com Git 3.0 blog]
