# MR-002 — Git + ci-sandbox seam: adversarial stub census

Date: 2026-06-26. Author: MR-002 (census, READ-ONLY — no code changed). Scope: `crates/myelin-git`
(storage/refs/web/api/pack/odb) and the ci-sandbox seam (`crates/myelin-ci-sandbox` reached from
`myelin-ci-dispatch`/`myelin-ci-controlplane`), focused on the production execution path.

## Executive summary

- **10 findings:** 5 CRITICAL, 3 HIGH, 2 MEDIUM. Plus a "what is actually real" counter-list so build
  prompts don't re-stub the few load-bearing organs that ARE genuine.
- **The single most dangerous thing:** the sandbox **has no production execution path at all.** Neither
  committed backend ever runs `spec.command`. Firecracker's production `launch()` hardcodes
  `oneshot=true` → boots `init=/bin/true` (a no-op boot); gVisor's `spawn_real_runsc` only probes
  `runsc --version` and returns a no-op handle. The AG-D4 escape corpus is verified through **separate
  drill harnesses** (`init=/bin/bash /dev/vdb` boot / a drill-only OCI bundle), NOT through `launch()`.
  So the "0 escapes" attestation certifies a code path **no real job will ever take**, and the path a
  real job *would* take (once exec is wired) is **entirely unverified against the corpus.** This is the
  exact catastrophe the doctrine names: one escape into the supply chain.
- **Closely behind it:** Git's ref store — the entry point to every repo — is a pure in-memory
  `Mutex<BTreeMap>` (`RefStore`), documented as "models the reftable-on-OLTP". `RefStore::open` loads
  nothing. Kill the process and every ref + reflog is gone: you lose your repos.
- **Third:** Git has **no production `WireExecutor`** (every impl is `#[cfg(test)]`) and **no server
  binary** — `git clone`/`push`/`upload-pack`/`receive-pack` byte-serving has zero production backing;
  the doc defers it to "the CI-sandbox runner," which (per finding #1) does not execute anything.
- **What IS real (do not re-stub):** the `git2`/libgit2 read backend (`GixCore`), the Postgres
  check-status projection (`PgCheckStatusProjection`, sqlx), and the `FsBlobStore`/pack-tier
  write-through for object *bytes*. The git crate has **no backup/restore of its own** — that floor
  lives in `myelin-storage` (modeled-WAL, P-529), outside this scope.

---

## SANDBOX findings

### F-sandbox-1: Firecracker production `launch()` boots `init=/bin/true` and never runs `spec.command`
- **Location:** `crates/myelin-ci-sandbox/src/firecracker.rs:327` (`FirecrackerBackend::launch`) →
  `:250` (`launch_with`) → `FcMachineConfig::from_spec:108` (`init=/bin/true` branch at `:113-117`,
  const `BOOT_ARGS_BASE` + `init=/bin/true` at `:56,114`). `spec.command` is **never referenced** in
  the whole file (verified: `grep spec.command src/` returns only `gvisor.rs:67`). Verified to exist.
- **Claimed (per ledger/contract):** P-544 / contract 8.4 — a production launch EXECUTES `spec.command`
  inside the hardened microVM (vsock guest-agent or read-only command drive), capturing stdout/stderr/
  exit, enforcing limits + timeout.
- **Built (actual):** `launch()` calls `launch_with(spec, hooks, /*oneshot=*/ true, spawn_real_vmm)` —
  the boolean is **hardcoded `true`**, so the production boot cmdline is `… init=/bin/true`: userspace
  runs `/bin/true`, init exits, the kernel reboots, the VMM exits 0. The job command is never injected,
  never built into a guest entrypoint, never run. A "successful" launch is a no-op boot.
- **Gap:** the entire job-exec mechanism is absent on the default/production backend. `launch()` proves
  the VM *boots*, not that the *job ran*.
- **Test-passes-on-stub?:** YES. `firecracker.rs` tests
  (`config_from_spec_derives_vcpu_mem_and_read_only_root`, `empty_allowlist_yields_no_network_device_in_the_json`,
  `launch_drives_the_four_guarantees_and_kill_whole_guest_kills`) assert the **config JSON shape**
  (read-only root, no NIC, `init=/bin/true` present) and the four-guarantee hook ordering with a
  `FakeVmm`. None assert that `spec.command` ran or produced output. `tests/cdc_8_4_unified_sandbox.rs`
  launches and asserts a handle, not an execution. A stub IS the implementation, so they pass.
- **Blast radius:** CRITICAL — the platform's untrusted-code execution boundary runs no untrusted code
  in prod; cutting CI/agents over to this would silently never execute jobs (or, once exec is bolted
  on, execute it through an unverified path).
- **Maps to:** P-544 (impl), P-545 (verify).

### F-sandbox-2: gVisor production `launch()` only probes `runsc --version`; the OCI bundle is never run
- **Location:** `crates/myelin-ci-sandbox/src/gvisor.rs:227` (`spawn_real_runsc`, runs
  `runsc --version` at `:229-235`, returns no-op `SpawnedRunsc` at `:236-244`); `OciConfig::from_spec`
  carries `args: spec.command.clone()` at `:67` but `to_json()`'s output is never executed; `launch`
  at `:210`. `SpawnedRunsc::kill` is a no-op `Ok(())` (`:240-244`). Verified.
- **Claimed (per ledger/contract):** P-544 — `spawn_real_runsc` must `runsc run --bundle <dir>` against
  a written OCI bundle whose `config.json` is the built `OciConfig`, capture exit/stdout/stderr, and
  whole-container-kill on teardown.
- **Built (actual):** it shells `runsc --version` purely to confirm the binary is reachable, then
  returns `SpawnedRunsc` (a unit struct). No bundle dir is written, `runsc run` is never invoked, the
  command never executes, nothing is captured, and `kill()` does nothing. The module doc even labels
  the OCI run path "a CI-P28 follow-on" (`:225-226`) — never landed.
- **Gap:** identical to F-sandbox-1 on the second backend — exec is absent; only a liveness probe runs.
- **Test-passes-on-stub?:** YES. `gvisor.rs` tests (`oci_config_enforces_the_backend_independent_hardening`,
  `gvisor_launch_drives_four_guarantees_on_the_same_trait`) assert OCI JSON shape and hook order with a
  `FakeRunsc`; they never run a container. The drill tests use a *separate* bundle (see F-sandbox-4).
- **Blast radius:** CRITICAL — same as F-sandbox-1; the "second backend" parity is shape-only.
- **Maps to:** P-544 (impl), P-545 (verify).

### F-sandbox-3: no exit/stdout/stderr capture, no timeout-kill, no limit enforcement; metering settles on a no-op boot
- **Location:** `crates/myelin-ci-sandbox/src/firecracker.rs:289-297` (`hooks.settle` fires immediately
  after spawn with a placeholder `ResourceUsage { cpu_seconds: cfg.vcpu_count, mem_byte_seconds: … }`);
  `crates/myelin-ci-sandbox/src/gvisor.rs:195-201` (`settle` with the literal placeholder
  `ResourceUsage { cpu_seconds: 1, mem_byte_seconds: 1 }`). No `timeout`/`wait`-with-deadline anywhere
  in either `launch` path; `spec.limits.timeout_secs` is never consumed by `launch`. Verified.
- **Claimed (per ledger/contract):** P-544 — both backends enforce `spec.limits` (cpu/mem/pids) + a
  wall-clock timeout that whole-guest-kills on expiry, capture exit+stdout+stderr into the handle/result,
  and `hooks.settle` fires ONCE, AFTER the command completes, with the REAL `ResourceUsage`.
- **Built (actual):** `settle` fires synchronously right after the no-op boot/probe with a placeholder
  usage; there is no result struct carrying exit/stdout/stderr; there is no timeout watchdog (a job
  that "ran forever" can't, because nothing runs — but the mechanism to bound a real run is absent).
- **Gap:** accounting bookends a non-event; a real run would settle before completing, never capture
  output, and never be killable by timeout.
- **Test-passes-on-stub?:** YES. The `ok_hooks()` `settle` closures are `|_h,_u| Ok(())` and assert
  nothing about timing or usage; no test feeds a non-zero exit, a slow command, or asserts captured
  bytes (there is nothing to capture).
- **Blast radius:** HIGH — billing/metering certifies fictitious usage; a hung real job (post-P-544)
  would have no timeout kill; a non-zero exit would have nowhere to be reported (mask-to-success risk).
- **Maps to:** P-544 (impl), P-545 (verify).

### F-sandbox-4: the AG-D4 escape corpus runs only through special drill harnesses, NOT the production `launch()`
- **Location:** `crates/myelin-ci-sandbox/src/firecracker.rs:414` (`drill_config_json` — boots
  `init=/bin/bash /dev/vdb`, a second read-only drive carrying the corpus, NOT the production cmdline);
  `crates/myelin-ci-sandbox/src/gvisor.rs:352` (`gvisor_drill_config_json` — a drill-only OCI bundle
  whose entrypoint is the corpus script `:362`); drivers `crates/myelin-ci-sandbox/tests/escape_drill_test.rs`,
  `escape_drill_gvisor_test.rs`, `escape_drill_prod_image_reconfirm_test.rs`,
  `escape_drill_ci_committed_gate_reconfirm_test.rs`. Verified.
- **Claimed (per ledger/contract):** P-545 — re-run the seven AG-D4 families THROUGH the production
  `launch()` path (the same runner a real job uses), assert 0 escapes on a real kernel on both backends,
  plus a guard asserting the corpus ran via the production path (`init=/bin/true` absent, real profile).
- **Built (actual):** the corpus is delivered via bespoke boot recipes (`/dev/vdb` PID1 bash on
  Firecracker; a drill-only bundle on gVisor) that bypass `launch()` entirely. Because `launch()` runs
  no command (F-sandbox-1/2), it is **structurally impossible** for the corpus to run through it today.
  The escape gate therefore attests a path real jobs never take, and the (currently empty) real-job
  path is unverified.
- **Test-passes-on-stub?:** YES — worse than passing-on-stub: the drill tests genuinely boot a kernel
  and parse `CONTAINED` markers, so they look maximally credible, but they exercise the harness, not
  production. The "0 escapes" green is real for the harness and meaningless for `launch()`.
- **Blast radius:** CRITICAL — the security keystone. A future regression that wires a weak prod exec
  path would not be caught by the existing corpus (it tests the harness); the gate gives false assurance.
- **Maps to:** P-545 (verify), depends on P-544.

### F-sandbox-5: the CI runner's production dispatch inherits the no-op exec
- **Location:** `crates/myelin-ci-sandbox/src/runner.rs:612` (`backend.launch(&job.spec, &self.hooks)`)
  — the runner's job-execution call site routes straight into the stubbed `launch()` of F-sandbox-1/2.
  Verified.
- **Claimed (per ledger):** the runner dispatches real jobs onto the hardened sandbox; dispatch/
  controlplane (`myelin-ci-dispatch`, `myelin-ci-controlplane`) schedule jobs that actually run.
- **Built (actual):** the runner faithfully calls `launch()`, which does nothing. The whole
  dispatch→controlplane→runner chain terminates in a no-op boot; no command output ever returns up the
  stack to controlplane log/exit handling.
- **Test-passes-on-stub?:** YES — runner/dispatch/controlplane tests assert scheduling, leasing,
  fairness, and event/log *shapes*, never a real command's captured exit/output through the chain.
- **Blast radius:** HIGH — confirms the stub is load-bearing for the entire CI execution chain, not an
  isolated backend detail.
- **Maps to:** P-544/P-545 (the fix is at the backend; this is the consumer that inherits it).

---

## GIT findings

### F-git-1: the ref store (every repo's entry point) is pure in-memory; `open` loads nothing
- **Location:** `crates/myelin-git/src/receive_pack.rs:537` (`struct RefStore`), fields
  `registry: std::sync::Mutex<BTreeMap<RefName, Arc<RefCell>>>` (`:549`), `reflog: std::sync::Mutex<Vec<ReflogEntry>>`
  (`:551`); `RefStore::open` at `:560-575` initializes empty `Mutex::new(BTreeMap::new())`/`Vec::new()`
  — **no sqlx, no pool, no load/hydrate**. Doc self-labels it "models the reftable-on-OLTP ref store"
  (`lib.rs:350`) and "the in-memory model of the single `git_ref` row" (`receive_pack.rs:520-528`).
  Verified.
- **Claimed (per ledger/contract):** the reftable-on-OLTP ref store — the per-ref CAS co-commits the
  ref move + reflog + `git.ref.updated` outbox row in ONE Postgres transaction (BUS-2), durable on the
  OLTP tier (GIT-P20 / the P-522 durable-persistence floor).
- **Built (actual):** refs and reflog live entirely in process memory behind mutexes. The "one
  transaction" is a modeled in-memory critical section; the outbox co-commit uses the shared
  `OutboxStore` (whose own durability is the substrate floor, MR-001 scope). On process restart every
  ref and reflog entry is gone.
- **Gap:** no durable backing for the canonical pointer set. Refs are the index to all objects; losing
  them loses access to the entire repo even if object bytes survive.
- **Test-passes-on-stub?:** YES. `drills_git_d9_receive_pack.rs`, `drills_git_d1_hot_ref_burst.rs`,
  `cdc_2_2_2_3_git_ref_updated.rs` assert CAS linearizability, emit-iff-committed, and per-ref
  parallelism against the in-memory store; the "crash" drill models a crash as a **returned enum**
  (`CrashPoint`/`InjectedCrash`, `receive_pack.rs:444-470`), never a real `kill -9` + restart + reload,
  so a `HashMap`-backed store satisfies every assertion.
- **Blast radius:** CRITICAL — lose your repos (refs) on any restart/crash; the durability drills don't
  catch it because they never restart a real store.
- **Maps to:** P-522/P-523 (durable persistence) + GIT-P20 live OLTP ref store; E1.1 (git HARDEN).

### F-git-2: object migration: bytes can be durable (FsBlobStore) but the oid→hash index is in-memory and never rebuilt
- **Location:** two `QuarantineMigration` impls — `crates/myelin-git/src/receive_pack.rs:427`
  (`InMemoryObjectDb`, records oids in a `Mutex<BTreeSet<Oid>>` at `:398-435`, **stores no bytes**) and
  `crates/myelin-git/src/pack_tier.rs:318` (`PackTierMigration<B: BlobStore>`, writes bytes through the
  real trait). `PackObjectDb::new` (`pack_tier.rs:145-156`) initializes `oid_index: Mutex::new(BTreeMap::new())`,
  `generation`, `accel` all **empty and in-memory — never hydrated from the tier on open.** Verified.
- **Claimed (per ledger/contract):** GIT-P11 / contract 11.2 — accepted quarantine objects are durable
  on the write quorum (`object_packs.rs` `ReplicatedBlobStore`) before the ref CAS acks; objects are
  found by oid through the pack tier across restarts.
- **Built (actual):** the *bytes* path is real where `PackTierMigration` + `FsBlobStore` are wired (a
  genuine content-addressed fs write). BUT the **lookup index** (`oid_index: oid → ContentHash`) lives
  only in memory and `PackObjectDb::new` starts it empty with no scan/rebuild — so after a restart, even
  durable blob bytes are **unfindable** (you have the content but not the oid→hash map). And the only
  migration sink wired in the test/serve harnesses is frequently `InMemoryObjectDb`, which stores oids
  in a set and no bytes at all.
- **Gap:** durability of object *retrieval* is not real even where byte durability is; the index is a
  volatile cache presented as the object DB.
- **Test-passes-on-stub?:** YES. `cdc_11_2_git_object_backed_packs.rs`, `drills_git_d4_object_backed_packs.rs`,
  `cdc_11_2_git_pack_tier_consumer.rs` assert migrate-then-read within one process and quorum-ack
  arithmetic; none restart the `PackObjectDb` and re-resolve an oid from a cold index.
- **Blast radius:** HIGH — objects become unaddressable after restart; combined with F-git-1, a restart
  loses both the refs and the means to find objects.
- **Maps to:** P-522/P-523, GIT-P11/P20; E1.1.

### F-git-3: no production `WireExecutor` — clone/push/upload-pack/receive-pack byte-serving has no production backing
- **Location:** `crates/myelin-git/src/core.rs` — the `WireExecutor` trait (`:268`) and the
  `RoutedGitCore`/`ShellGitCore` seam (`:352,413`) are real, but **every** `impl WireExecutor` and
  `impl ReadBackend` in the crate is inside `#[cfg(test)] mod tests` (the `mod tests` opens at
  `core.rs:511`; impls `Recorder` `:589`, `NoExec` `:649`, `OkExec` `:680`; plus `code_tools.rs:559`,
  `holder.rs:1195` — all in test modules). The module doc states the production X-6-hardened executor
  "lives in the serving tier … wired in GIT-P9/GIT-P13, onto the same CI-sandbox runner" (`core.rs:257-267`,
  `front_door.rs:67-71`). Verified.
- **Claimed (per ledger/contract):** the wire/maintenance ops route to a sandboxed canonical-`git`
  executor (GIT-P13 serving tier) so real `git clone`/`push`/`fetch` work end-to-end (E1.1 oracle:
  real `git` clone/push + `git fsck`).
- **Built (actual):** there is no production `WireExecutor` anywhere in the workspace; the seam has only
  test doubles. The named production home is "the CI-sandbox runner" — which per F-sandbox-1/2 executes
  nothing. So the byte-serving half of git (the part that makes `git clone`/`push` work over the wire)
  has zero production implementation.
- **Gap:** the read path (libgit2, real) exists; the wire-serving path is an unimplemented seam.
- **Test-passes-on-stub?:** YES — `smoke_gitcore_seam.rs` and `drill_git_d8_front_door.rs` drive the
  routing + authz state machine with `OkExec`/recording doubles, asserting the wire/read split and that
  bytes are *handed to* an executor, never that a real `git` process served a real clone/push.
- **Blast radius:** CRITICAL — the daily-driver primitive (clone/push) is not real; the E1.1 oracle
  (real `git` round-trip + `fsck`) cannot pass against what exists.
- **Maps to:** GIT-P13 serving tier; E1.1/E1.2; depends on the sandbox exec path (P-544).

### F-git-4: no runnable git server — no binary, no bound listener; all assembly is in `tests/`
- **Location:** `crates/myelin-git/` has **no** `src/main.rs`, no `src/bin/`, no `[[bin]]` in
  `Cargo.toml` (verified). `FrontDoor` (`front_door.rs:295+`) is an authz/residency routing state
  machine over three dependency ports — **no `axum`/`russh`/`TcpListener`/`bind`** in the file; the doc
  defers "the production `russh`/`axum` transport wiring" to the GIT-P13 floor (`front_door.rs:67-71`).
  Every production assembly of `RefStore::open`/`FrontDoor::new`/`PackTierMigration`/`PackObjectDb::new`
  occurs only under `crates/myelin-git/tests/` (verified by workspace grep). Verified.
- **Claimed (per ledger/roadmap):** git is "the first real daily driver"; the front door is "the one
  pipeline every SSH/HTTP entrypoint funnels through."
- **Built (actual):** git is a library of view-models, command logic, and seams with no entrypoint that
  binds a socket and serves. Nothing wires the pieces together outside test fixtures. (This corroborates
  the roadmap's own "interaction-surface-empty / not runnable software" finding.)
- **Test-passes-on-stub?:** YES — tests assemble the stack in-process and assert behavior; "is it
  runnable as a server" is never an assertion.
- **Blast radius:** HIGH — not lost data, but git is not deployable/usable software today; E0.6/E0.8/
  E1.2 net-new effort.
- **Maps to:** E0.6 (product API), E1.2 (git API), E0.8 (UI shell).

### F-git-web-1: the web UI is server-rendered HTML strings; its e2e test drives STATIC files, not a live backend, and degrades to `partial`
- **Location:** `crates/myelin-git/src/web.rs` (render functions returning HTML `String`, e.g.
  `ForkTrustBadge::render` `:204`, `CheckRowView::render` `:276`); the e2e test
  `crates/myelin-git/tests/e2e_git_p32_web_browser.rs` — `write_page` writes rendered HTML to a temp
  file (`:72-74`) and runs chromium `--dump-dom` on a `file://` URL (`:109-112,128`); chromium is
  located on PATH (`:79`) and absence → records `"partial"` (`:290,298`), not a skip. Verified.
- **Claimed (per ledger/roadmap):** git alone has a real web UI with a headless-chromium e2e
  "rehearsal."
- **Built (actual):** the render layer is real Rust→HTML view-models (and genuinely escapes/handles
  empty/error/permission-denied states). The "browser test," however, renders **static** HTML to a file
  and asserts DOM markers via `--dump-dom`; there is **no live server, no backend, no interaction** —
  it cannot catch a render-vs-backend divergence, and it downgrades to `partial` without chromium.
- **Gap:** UI reality is a static-render rehearsal, not a live app driven against a running git backend.
- **Test-passes-on-stub?:** YES (by construction) — it asserts the shape/DOM of statically rendered
  strings; the entire backend is absent from the test, so any backend stub is irrelevant to it passing.
- **Blast radius:** MEDIUM — no data loss; the "we have a web UI" claim is a render rehearsal, and the
  real interactive surface (E0.7 SolidJS/Tauri) is net-new. Use `web.rs` as a view-model source only.
- **Maps to:** E0.7 (frontend foundation), E1.3 (git web UI).

### F-git-api-1: the API/CLI surface is a route/command CATALOGUE, not a served API or a dispatching binary
- **Location:** `crates/myelin-git/src/api.rs` — `Endpoint`/`http_catalogue()` (`:81,120`),
  `CliCommand` + `handler()` mapping (`:202,255`); the module doc: "NO new handler / NO new contract …
  the route/command CATALOGUE that the host + the `myelin` CLI binary dispatch over" and "production
  `russh`/`axum` transport wiring lives in the front-door host" (`api.rs:5-33`). No HTTP server and no
  CLI binary exist (F-git-4). Verified.
- **Claimed (per ledger/roadmap):** git has a CLI/API command grammar (the partial product-API
  exception).
- **Built (actual):** real, useful catalogue + parse/route logic + an enforced invariant (every write
  route is `Id.check`-gated). But it is data describing routes/commands; nothing serves the routes or
  dispatches the CLI verbs at runtime.
- **Test-passes-on-stub?:** YES — tests assert catalogue invariants (e.g. "every write is id-checked",
  `api.rs` tests at `:347`), which are properties of the enum table, not of a running API.
- **Blast radius:** MEDIUM — the grammar is reusable (good), but there is no callable API/CLI yet.
- **Maps to:** E0.6 (product API), E0.9 (CLI/MCP), E1.2/E1.4.

---

## What IS real (counter-findings — do not re-stub these)

- **`GixCore` read backend over `git2`/libgit2** — `crates/myelin-git/src/gix_backend.rs:56-160`
  (`read_blob`/`diff_blobs`/`blame`) and `core.rs:296` (`ReadBackend`). A real in-process read/diff/
  blame path against on-disk repos (the gix-preferred swap is the documented OQ-1/GIT-P33 floor). This
  is the one genuinely runnable git organ.
- **`PgCheckStatusProjection`** — `crates/myelin-git/src/check_status_store.rs:51,105-138` — a real
  sqlx/Postgres projection with migration + in-transaction `event_id` idempotency, behind
  `--features integration` (`Cargo.toml:180`), "proven against the dev Postgres stack." A real durable
  store. (Note the asymmetry: check-status is on real Postgres while the *ref* store, F-git-1, is not.)
- **`FsBlobStore` + pack-tier write-through for object bytes** — `myelin-storage/src/blob.rs:362,437`
  (real `fs::write` content-addressed store), consumed by `pack_tier.rs` `PackTierMigration`. Byte
  durability is real; only the oid index is volatile (F-git-2).
- **The hardening profile + config builders** (`firecracker.rs` `FcMachineConfig`, `gvisor.rs`
  `OciConfig`, `hardening.rs`) faithfully encode the read-only-root / no-NIC / caps-dropped / nnp /
  seccomp / pids posture — the *recipe* is real; what's missing is *executing a job under it*.

---

## What I did NOT inspect deeply (coverage boundary)

- **`myelin-storage` backup/restore** (`backup.rs`, `restore.rs`, `restore_verify.rs`,
  `replicated_blob.rs`, `s3blob.rs`): confirmed git has **no** backup/restore of its own and the
  storage modules exist, but I did not audit whether the storage WAL/restore is modeled vs real (the
  ledger flags it as the modeled-WAL P-529 floor). That is MR-001/substrate scope. The "real
  *destructive* restore of your repos" (E1.1) cannot be assessed git-side because the ref store itself
  isn't durable (F-git-1); the storage restore path needs its own census.
- **`ReplicatedBlobStore` quorum semantics** (`object_packs.rs`, `myelin-storage/src/replicated_blob.rs`):
  I read the quorum-ack arithmetic and the `QuarantineMigration` wiring but did not verify whether
  replicas are real network nodes or in-process models — likely a model; flag for storage census.
- **The dispatch/controlplane internals** beyond the `launch()` call site: I traced
  `runner.rs:612 → launch()` (F-sandbox-5) but did not audit scheduler/fairness/metering/log-pipeline
  correctness in `myelin-ci-controlplane` (large surface; MR-003+ territory).
- **The escape-corpus content itself** (`escape_corpus.rs`, the seven families' adversarial fidelity):
  I confirmed the corpus is routed through harnesses not `launch()` (F-sandbox-4) but did not grade
  whether each family is a strong test of the property it claims.
- **`snapshot_pool.rs`, `self_hosted.rs`, `surge.rs`, `fleet.rs`** (sandbox/controlplane): skimmed
  names only; the ledger marks pre-warmed pools (CI-P4) + fleet (CI-P14) as their own floors.
- **The git crate's many projection/anchor/merge modules** (`anchor.rs`, `code_projection.rs`,
  `merge_gate.rs`, `lifecycle.rs`, `holder.rs`, etc.): out of the storage/refs/web/api/pack scope of
  this prompt; not audited for stub-ness.
- I did **not** run the test suite or any drill; all judgments are static reads of source + tests.
