# Review delta — impact of the 26 commits (5f69f29 → f49e714)

_Assessed against HEAD `f49e714`. Two tracks landed after the 2026-07-06 review: the **CI track** (CT-001…CT-006d — sandbox production execution, escape-verification through `launch()`, and real git-over-the-wire) and **MR-009b W0–W3** (the "durable un-gating" — durable backends become the default, in-memory doubles move behind a `test-support` feature)._

## Bottom line

**No prior finding was fixed. Several escalated. The two new surfaces added 4 significant findings (3 HIGH).** These commits activated the exact code paths the review flagged as "latent," so risk rose rather than fell. The engineering itself is sound (the new prod-exec and wire confinement are well-built); the problem is that pre-existing gaps are now on live paths and the new surfaces inherited the review's authorization gap.

## Prior findings — status changes

| Finding | Was | Now | Why |
|---|---|---|---|
| **CI: egress/metadata-SSRF allowlist computed but never enforced** (`ci` #1) | HIGH, dormant (sandbox ran `init=/bin/true`) | **NOW-LIVE HIGH (Firecracker)** | CT-002a runs untrusted `spec.command` in a real microVM with a real **unfiltered** NIC on the shared `tap-myelin`; nothing in-crate filters egress → live metadata-SSRF/exfil. gVisor is safe (`--network=none` unconditional). |
| **Git: crash reconciler uses reflog-length as `update_seq`** (`git` #1) | HIGH (code) | **HIGH — likelihood raised** | `reconcile.rs` untouched; but wire push now advertises `delete-refs`, so the delete+recreate case that resets the reflog is reachable over the wire by any client. |
| **Git: `update_ref_cas` races shared `.git/config`** (`git` #2) | MEDIUM, "plausible" | **MEDIUM — now genuinely hit** | `durable.rs` code unchanged (shifted to :363); real concurrent `git push` to different refs (per-ref `RefLock` allows parallelism) now exercises the unsynchronized config-write window in production. |
| **Identity: PG credential path drops `scope.region()`** (`identity` #b) | LOW, latent (PG behind `integration`) | **NOW-LIVE (LOW)** | MR-009b W2 makes PrincipalStore/RevocationStore durable-by-default, wired at `edge/main.rs`. The durable arm binds the *cell's* region, ignoring `scope.region()` entirely — the model↔durable divergence is now the live behavior. |
| **Identity: `check()` authorizes on bare trailing id** (`identity` #a) | MEDIUM (logic) | **MEDIUM — now on the durable path** | `check_engine.rs` unchanged; the type-blind decision now runs over the PG tuple set by default (`StoreBackedCheck::with_pg`). |
| **CI: gVisor drill uses no-op seccomp** (`ci` #3) | LOW | **Largely mitigated** | CT-003 routes the AG-D4 escape corpus through the real `launch()` (production `SCMP_ACT_ERRNO`); the ALLOW-default survives only in a legacy root-adversary drill. |
| _(review context) AG-D4 corpus ran through drills, not `launch()` (SI-017)_ | noted gap | **FIXED (gVisor) / PARTIAL (Firecracker)** | CT-003 mints a green non-root prod-path attestation for gVisor; Firecracker's is withheld (some attack families `DidNotRun` non-root). |

## Prior findings — UNCHANGED (code untouched, status same)

Still stand verbatim, not addressed by these commits:
- **Search RAG/vector ACL leak** (HIGH) — `engine.rs` untouched.
- **GDPR erasure restore-resurrection** (HIGH) — `fanout.rs` untouched; **still latent** (durable erasure ledger is W6b, not landed).
- **Substrate fail-static authz cache aliasing** (HIGH) — `fail_static.rs` untouched (a live runtime cache, never flag-gated).
- **MCP agent self-approval / HITL batch bypass** (HIGH) — untouched.
- **No object-level authz at the edge** (`xc-tenancy`, MEDIUM) — `AllowAll` is still the edge authorizer (`main.rs:83`).
- **Storage** predicate_sql (still model-only even after un-gating — durable code uses binds+GUCs), tenant-id-in-blob-key (still latent; blob byte-durability is W7), TRUNCATE classifier (unchanged).
- **Knowledge** block_tree SQL interpolation (still latent — knowledge not un-gated) and export XSS (untouched).
- **CI** `digest_pinned` any-length hex (LOW) — byte-identical.
- **GDPR/xc-gdpr** residency/checklist/holders_hit findings — still latent (holders are W6, not landed).

## NEW findings — surfaces that did not exist at review time

The real git-wire path (`git_wire_http.rs`, `git_wire_exec.rs`, `git_receive_pack.rs`, `git_durable.rs`) was newly added and never reviewed:

| # | Severity | Title | Location | Impact |
|---|---|---|---|---|
| N1 | **HIGH** | Wire push bypasses the merge-gate and per-repo branch-protection ruleset | `git_receive_pack.rs:826-829, 123-125` | `PushPolicy::default()` hardcodes protection for `refs/heads/main` + `release/*` only and never evaluates the CI-green/required-review merge-gate. Any principal with `git.wire.receive_pack` can push commits **directly to `main`** that the PR merge path would BLOCK; any other protected branch (e.g. `develop`) gets zero force-push/delete protection. |
| N2 | **HIGH** | No per-repo authorization on the wire — coarse tenant+action only | `gateway.rs:235`, `git_wire_http.rs:221-243` | Extends the `xc-tenancy` object-authz gap to raw git content: a principal granted the wire action can clone/fetch (read full source) **and push to** *any* repo in their tenant, regardless of per-repo ACLs. Tenant isolation itself holds (operating tenant is from the verified token; IDOR rejected pre-lookup). |
| N3 | MEDIUM | Unbounded request-body buffering → host memory DoS | `server.rs:61-63` (`body.collect().await`) | The whole HTTP body is buffered into host RAM before any size check; the sandbox's 64 MiB stdin bound applies only after. Concurrent large `git-receive-pack`/`upload-pack` POSTs exhaust host memory. |
| N4 | MEDIUM | Shallow push connectivity check (tip tree only) | `git_durable.rs` `commit_tree_complete`; ingest `gvisor.rs:1220-1222` (no `fsck`/`rev-list`) | `index-pack --fix-thin` resolves delta bases but not missing parent commits, and only the tip commit's tree is verified. A crafted push whose tip has a missing ancestor is accepted → later `clone`/`fetch` fails client-side; one push can wedge a branch's clonability. |

**Cleared on the new surface (checked, sound):** path-traversal/slug validation (byte-for-byte GT-001 allowlist), no argv/shell injection (untrusted pack only on stdin), operating `(tenant,region)` from the verified token (never the URL), cross-tenant IDOR rejected with 0-leak 404, oid re-hashing on every migrated object (forgery structurally impossible), fail-closed FF/connectivity checks, no token/secret exposure. In the new CI prod-exec: no command injection, guest env constructed not inherited, host mounts bounded/RO, stream-capture memory now capped (CT-002c).

## How this moves the review's themes

- **"Latent-until-wired behind the `integration` flag"** — still broadly accurate for ~70% of the durable substrate (KMS, control-plane registry, GDPR ledgers, blob bytes, outbox, and the whole knowledge/git durable surface remain model floors), but now **wrong for the identity spine and the dedup ledger**: principal/tuple/revocation/dedup are durable-by-default and wired at boot (scanner baseline 17→13). That flip is what makes `identity`-#b live and puts `identity`-#a on the durable path.
- **"Compute-but-don't-enforce"** — reinforced, not resolved: the CI egress allowlist is still computed and unenforced, now on a live datapath.
- **"Action-scoped, not object-scoped, authz — propagated by template"** — reinforced: the new wire path inherited the coarse tenant+action gate (N2) and added a second enforcement gap by bypassing the merge-gate on push (N1).

## Suggested priority (post-delta)

1. **Fail-closed the Firecracker NIC** until a real tap-device egress firewall is emitted (CI #1, now live).
2. **Gate wire push through the merge-gate + repo branch-protection ruleset** and add **per-repo object authorization** to the wire routes (N1, N2) — before removing `AllowAll`.
3. Fix the **git reconciler `update_seq`** (durable monotonic generation) before real delete+recreate traffic (git #1).
4. Bound the **wire request body** at the front door (N3).
5. Carry the still-latent HIGHs (search RAG-ACL, GDPR restore-resurrection, fail-static cache) into their un-gating waves so they are closed *before* those backends flip on.
