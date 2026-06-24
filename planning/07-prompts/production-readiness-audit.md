# Phase 7 — Production-Readiness Audit + Coverage Matrix (the M7 motivation)

> Phase: `07-prompts`. This document is the **verified disposition** of an eleven-finding production-readiness
> audit against the LATEST LOCAL CODE (committed through P-434; P-435..P-521 NOT yet executed) and against every
> remaining unexecuted prompt. Each finding (and each distinct sub-point) is classified as **already-fixed-in-
> code** (with file:symbol evidence), **handled-by-existing-unexecuted-prompt** (with the P-NNN and the quoted
> deliverable line), **partially-handled** (with the precise residual), or **unhandled**. Every unhandled /
> partial item is filled by a new M7 prompt in
> [`by-system/production-readiness.md`](by-system/production-readiness.md) (P-522..P-546); the run order is in
> [`README.md`](README.md) §2, the band is in [`../06-roadmaps/00-master-sequencing.md`](../06-roadmaps/00-master-sequencing.md)
> §2 M7. Markdown only; no commits by this document. Date: 2026-06-24.
>
> **The load-bearing finding of the whole audit.** The implementation honestly shipped a set of production
> mechanisms as **documented EI-01 §1 structural floors** — correct in shape, with honest `Floor named:` notes —
> but the floor notes (and the M1 roadmaps) point the follow-on at **"P5/P6"**, which is an old planning-PHASE
> label, NOT a ledger PROMPT id. There is **no P-NNN in the existing ledger (P-001..P-521) that fills these
> floors**: no prompt swaps the structural credential verifiers for real cryptography, none binds a live
> Postgres/Valkey pool under the in-memory identity stores, none backs the KMS root with an HSM, none replaces
> the modeled-WAL restore with a real `pg_basebackup`/`pg_restore`, the supply-chain governance floor is
> entirely unaddressed, AND — corrected on re-audit — **neither committed sandbox backend executes
> `JobSpec.command` through its production launch path** (Firecracker hardcodes `oneshot=true` → `init=/bin/true`;
> gVisor only probes `runsc --version`). M7 (P-522..P-546) is the band that fills them, with implementation and
> verification as separate prompts and a final fail-closed release gate.

---

## 1. The disposition legend

- **FIXED** — already real in the committed code; cite file:symbol; no M7 prompt needed (or only a verification re-confirm).
- **EXISTING-PROMPT** — handled by an unexecuted prompt P-435..P-521; cite the P-NNN + the quoted deliverable line.
- **PARTIAL** — partly real / partly floor; state the precise residual + the M7 prompt that fills it.
- **UNHANDLED** — neither real nor covered by any existing prompt; the M7 prompt that fills it.

Evidence convention: paths are relative to repo root `crates/…`; "(non-test)" means a production constructor /
`AppSpec`/`serve` wiring path, not a `#[cfg(test)]` fake.

---

## 2. The coverage matrix (every finding, every sub-point)

### Finding 1 — Production service runtime (`serve(AppSpec)`)

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Env-validated config, fail-fast at boot | **FIXED** | `myelin-substrate/src/serve.rs` config is env-first + validated at boot; `oltp_config_from()` returns `ServeError` on unbounded/zero pool. |
| Three listeners (public/internal/metrics-health) + routing | **FIXED** | `serve.rs` `PortOpener` opens the three surfaces; `Surface` enum. |
| Bounded prod pool | **FIXED** | `myelin-storage/src/oltp.rs` `OltpConfig` (`max_pool_size`, `statement_timeout_ms`, per-tenant cap); `OltpError::PoolSaturated` (fast-fail, never unbounded). |
| Broker/outbox relay | **FIXED** | `serve.rs` `Relay::new()` auto-started at boot; `drain_to_empty()`. |
| Telemetry + trace propagation (real OTel export) | **PARTIAL → P-539** | `serve.rs` telemetry struct exports contract-1.8 signals to an in-test reader; floor note: "the real OpenTelemetry export … lands with the metrics-health surface (P-S13/P-S14)". Residual: real exporter + trace-context middleware. |
| OS signal lifecycle (SIGTERM) + graceful drain | **PARTIAL → P-539** | `serve.rs` graceful `drain()` is real (stop intake → depth-0 → ack-then-exit), BUT the trigger is a deterministic in-process `signal_drain()`; floor note: "the SIGTERM/SIGINT → drain wiring lands with the real ports (P-S13/P-S14)". Residual: real OS-signal handler + bounded deadline. |
| Prod AppSpecs must not discard routes / immediately exit | **FIXED** | `serve.rs` lifecycle serves until drain; non-zero on failed boot; the issues `main.rs` exits non-zero on boot failure. |

**Note:** "P-S13/P-S14" in the code comments refer to prompts P-030/P-031 (the three-surface topology + liveness≠readiness), which ARE executed — but they did NOT ship the real OS-signal handler or the real OTel exporter (those stayed floors). So the runtime floors are genuinely **UNHANDLED by any executed-or-unexecuted prompt** → **P-539**.

### Finding 2 — Authentication (real credential + token cryptography)

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Real OIDC JWKS signature verification | **UNHANDLED → P-526** | `myelin-identity-service/src/authenticate.rs` `StructuralVerifier` (non-test default at `with_verifier`, line ~289) parses a `"<tenant>\|<region>\|<subject_key>"` envelope; floor note: "real OIDC JWKS-signature … is the named P5/P6 floor". No ledger prompt fills it. |
| Real SAML XML-DSig | **UNHANDLED → P-526** | Same `StructuralVerifier`; floor note names SAML XML-DSig as the P5/P6 floor. |
| Real WebAuthn/passkey attestation | **UNHANDLED → P-526** | Same; floor note names WebAuthn attestation. |
| Real SSH challenge verification | **UNHANDLED → P-526** | Same; floor note names SSH challenge-response. |
| Cryptographically signed machine/capability tokens | **UNHANDLED → P-527** | `machine_auth.rs` `StructuralTokenVerifier` + `mint.rs` `StructuralTokenSigner` (non-test defaults at `mint.rs` ~253/265, `machine_auth.rs` ~406) parse/emit a 6-field envelope; floor note: "real PASETO sign / biscuit caveat crypto / DPoP proof … the named P5/P6 floor". |
| Strict iss/aud/exp/nbf/nonce/replay validation | **PARTIAL → P-526/P-527** | The structural verifiers validate the envelope's structural fields + TTL but do NOT verify a signature; real claim validation comes with the real verifier. |
| Real DPoP proof verification | **UNHANDLED → P-527** | `machine_auth.rs` checks only `dpop ∈ {0,1}` (a flag), no RFC 9449 proof; floor note names DPoP proof as P5/P6. |
| REMOVE Structural* from every production constructor/path | **UNHANDLED → P-526/P-527 (impl) + P-528 (verify)** | `Structural{Verifier,TokenVerifier,TokenSigner,AttestationVerifier}` are wired as defaults on production constructors (`authenticate.rs` ~289, `mint.rs` ~253/265, `machine_auth.rs` ~406, `self_hosted.rs`). P-528 adds an absence-scanner that fails the build on any production use. |

### Finding 3 — Token & authz expiry

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| One authoritative issuance/revocation/expiry store + clock | **FIXED (mechanism), PARTIAL (durability) → P-522** | `revocation.rs` S7 `RevocationStore` (mirror+fast layers, `REVOCATION_SLA_SECS=300`, `RunTokenState{LiveWithinRunLife\|Expired\|TornDown\|Unknown}`) is real in shape — but in-memory `BTreeMap` (Finding 6); durability filled by P-522. |
| Auth enforces token lifetime; expired tuples never enter snapshots | **FIXED** | `mint.rs` refuses non-positive TTL (`NonPositiveTtl`); per-run grants are auto-expiring tuples (`expires_at == run life`); `is_revoked()` consults denylist + `expires_at`. |
| Teardown / crash-recovery / replay / cleanup | **PARTIAL → P-523/P-528** | The auto-expire is the revoke-on-crash defence; full crash/replay durability needs the live store (P-522/P-523); P-528 proves expired grants cannot authorize across all four phases. |
| Tests proving expired grants cannot authorize | **UNHANDLED → P-528** | No end-to-end drill proves an expired tuple never authorizes across teardown/crash/replay/cleanup; P-528 adds it. |

### Finding 4 — Sandbox execution

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Firecracker executes JobSpec.command (NOT init=/bin/true) | **UNHANDLED → P-544 (impl) / P-545 (verify)** | **FALSE that this is fixed.** The production `SandboxBackend::launch()` is literally `self.launch_with(spec, hooks, /* oneshot = */ true, spawn_real_vmm)` (`firecracker.rs:327-328`) — it ALWAYS boots `oneshot=true`. `from_spec(.., oneshot=true)` sets boot args `init=/bin/true` (`firecracker.rs:113-114`, `BOOT_ARGS_BASE` :56-57). So the production Firecracker `launch()` boots the VM, runs `/bin/true`, and exits — **`spec.command` is NEVER executed** (`grep spec.command firecracker.rs` is empty). The only command-bearing boot is the DRILL's `init=/bin/bash /dev/vdb` (a test harness). The Firecracker backend has NO production JobSpec.command execution path. |
| gVisor executes JobSpec.command via runsc run --bundle | **PARTIAL → P-544 (impl) / P-545 (verify)** | gVisor is better but STILL not a production exec path: `OciConfig::from_spec` sets `args: spec.command.clone()` (`gvisor.rs:67`), but the production `launch()` → `spawn_real_runsc` (`gvisor.rs:210-211, 227-237`) only runs `runsc --version` (a reachability probe) and returns a `SpawnedRunsc` no-op child — it does NOT spawn `runsc run --bundle <dir>` with that config. The full OCI-bundle run is explicitly deferred ("the full OCI-bundle run path is a CI-P28 follow-on", `gvisor.rs:225-226`). So gVisor carries `spec.command` in the OCI config but never executes it through the production launch path either. |
| Production jailer/cgroup/seccomp/capability/zero-swap/PID/disk/timeout | **FIXED** | `hardening.rs` `HardeningProfile::derive` forces `read_only_root`, `drop_all_caps`, `no_new_privileges`, `seccomp`, `zero_swap` (no swap field exists), `pids_max`, `ephemeral_one_job`; `firecracker.rs`/`gvisor.rs` enforce per-backend; `assert_enforced()` before VMM spawn. (The hardening *posture* is real; what is missing is the *job-exec path the posture would contain*.) |
| Per-job network namespace + allowlist (DNS/IP normalization, metadata/private-range denial) | **FIXED** | `hardening.rs` `EgressEvaluator` always blocks `169.254.169.254` + RFC-1918; default-deny; firecracker omits the NIC when allowlist empty. |
| Real runsc OCI bundle execution + teardown | **PARTIAL → P-544 (impl) / P-545 (verify)** | The OCI config is built and a `runsc` reachability probe runs, and `kill()` whole-container-kills on teardown (`child.kill()`, ephemeral) — but `runsc run --bundle` with the spec is NOT spawned in production (see the gVisor row above). Teardown is real; the *run* is not. |
| Accounting only after actual completion | **PARTIAL → P-544 (impl) / P-545 (verify)** | exactly-once `job.done` terminal report (P-238 runner agent) + reserve/settle bookends are real in shape, BUT since neither backend's production `launch()` runs `spec.command`, "completion" today means "the boot self-test / reachability probe returned," not "the job's command ran to an exit code." P-544 makes metering fire only after actual command completion. |
| Production-path escape drills on committed KVM/gVisor runners | **PARTIAL → P-545 (verify; via the production exec path)** | AG-D4/CI-T1 is wired with `MYELIN_REQUIRE_KVM=1` (DB/KVM-free run HARD-FAILS, not skips) — `testing/scorecards/m2-reactive.md` + `m4-consumers.md`; P-348/P-370 re-confirm on the prod image — but the corpus runs through the **special drill harness** (Firecracker's `init=/bin/bash /dev/vdb` command-drive boot; gVisor's drill-only bundle run), NOT the production `launch()` path (which never executes a command at all). A drill on a harness is not proof of the production exec path. P-545 re-runs the AG-D4 corpus THROUGH the real production exec path on both committed backends. P-542 still adds the INDEPENDENT external review. |

**Finding 4 is PARTIALLY HANDLED — NOT fixed.** The hardening posture, the egress default-deny, the teardown, and the reserve/settle accounting *bookends* are real in shape; but **neither committed backend executes `JobSpec.command` through its production `launch()` path**: Firecracker hardcodes `oneshot=true` → `init=/bin/true` (`firecracker.rs:327-328`), and gVisor's `spawn_real_runsc` only probes `runsc --version` rather than running `runsc run --bundle` with the spec's `args`. The escape drills that pass today run through special harnesses (a command-drive boot / a drill-only bundle), not the generic production exec path. **Residual:** (1) a real microVM/gVisor production JobSpec.command execution path — inject command+env+workspace, execute, capture exit code + stdout/stderr, whole-guest-kill on timeout, meter ONLY after actual completion; (2) a production-path (not harness) escape drill re-running the AG-D4 corpus on BOTH committed backends → ZERO escapes on a real kernel. Filled by **P-544 (impl) + P-545 (verify)**; the independent external sandbox review (P-542) remains a recorded human blocker.

### Finding 5 — Gate & evidence integrity

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Mechanically-enforced M2/M3/M4 + permanent gates | **FIXED** | `.github/workflows/ci.yml` + `integration.yml` — every gate is the binary exit code, "deliberately NO `\|\| true` / `continue-on-error` anywhere". |
| Required KVM jobs FAIL not skip | **FIXED** | AG-D4/CI-T1 run `--features integration` with `MYELIN_REQUIRE_KVM=1`; "on a host without /dev/kvm … the drill HARD-FAILS (it does not skip)" (`m2-reactive.md`, `m4-consumers.md`). |
| Semantic CDC validation (not marker-word existence) | **FIXED** | `contract-coverage.toml` + scanner verify row existence + the named CDC file exists on disk + carries BOTH provider+consumer markers + a `landing` prompt for deferred rows; "NEVER weaken this gate". |
| Mandatory mutation jobs | **PARTIAL → P-540** | Mutation floors are stated per-prompt + some are met (e.g. row 1.5 `schemes.rs` 31/0); but make every mandatory-core mutation job a REQUIRED CI job. |
| Immutable provenance-bearing CI attestations | **PARTIAL → P-540 (ties P-536)** | Scorecards are dated committed artifacts but not signed/attested; P-540 makes them immutable/attested. |
| Truth-up of all previously claimed-green scorecards | **UNHANDLED → P-541** | M6 has a truth-up pass (P-510/P-512) against the M1..M6 floors; P-541 re-runs every band gate against the REAL M7 production graph (so an auth drill that passed on `StructuralVerifier` is re-proven on real crypto). |

### Finding 6 — Durable persistence

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Identity principals durable | **UNHANDLED → P-522 (impl) / P-523 (verify)** | `principal_store.rs` `Inner { partitions: HashMap … }`; floor note: "models the SQL S1 table … until the driver lands (P-S15)". P-S15 (=P-032) ships the holder/migration mechanism only, NOT a live pool. |
| ReBAC tuples durable | **UNHANDLED → P-522/P-523** | `tuple_store.rs` `Inner { partitions: HashMap … }`; floor: "models the SQL S3 table … until the driver lands (P-S15)". |
| Revocations durable | **UNHANDLED → P-522/P-523** | `revocation.rs` `Inner { mirror: BTreeMap, fast: BTreeMap … }`; floor: "models the Redis/Valkey + PG-mirror S7 … until the substrate binding lands (P-S15)". |
| Replicas + other systems of record | **PARTIAL → P-522/P-523** | Real `pg.rs`/`oltp.rs` exist and are exercised by `myelin-storage`/`myelin-chat` integration tests — so the durable seam EXISTS — but Identity's production constructors still wire the in-memory variants. |
| NO in-memory "durable mirror" in production | **UNHANDLED → P-523** | P-523 adds a build-graph scanner that fails the build on any in-memory durable store in a production constructor. |
| Crash/restart + multi-instance behavior | **UNHANDLED → P-523** | No crash/restart or multi-instance drill against the real store today; P-523 adds them. |

### Finding 7 — KMS & encryption

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Durable Vault Transit/HSM-class KMS | **PARTIAL → P-524** | `myelin-storage/src/kms.rs` `KmsEngine` is a real software (Vault-Transit-class) engine behind `KmsAdapter`, but the L0 root is `CellRoot::generate()` in-process; floor: "HSM/Shamir-split-recovery backing is the production hardening follow-on". Residual: the HSM-class adapter. |
| Durable root/KEK lifecycle, rotation, recovery, destruction | **PARTIAL → P-524** | Rotation (`rotate_kek` O(keys)) + destruction (excluded from `backup_snapshot`) are real; KEK/DEK held in `Mutex<BTreeMap>` (not durable); Shamir-split recovery not present. |
| Zeroization + secret-memory handling | **PARTIAL → P-525** | `RawKey`/`Dek` redact in `Debug` + keep private bytes, but no `zeroize`-on-drop. P-525 adds it + the verify. |
| AEAD associated data binding tenant/object/field/key-ref/epoch | **FIXED** | `kms.rs` AES-256-GCM with AAD; `pii_key_ref = kms://<tenant>/<dek-epoch>/<class>` travels with ciphertext. (P-524 re-asserts.) |
| Fallible errors (no process panics) | **FIXED** | `resolve_dek`/wrap/unwrap return `Result`; "NEVER a plaintext-without-key fall-through (the 0-fail-open invariant)"; `kms_failstatic.rs` bounded-staleness, never fail-open. |
| Backup/restore that cannot resurrect destroyed keys | **FIXED (model) → P-525 (verify on real restore)** | `backup_snapshot` excludes DEKs whose tenant KEK is destroyed; proven against the modeled restore; P-525 re-proves across the real restore (P-529). |

### Finding 8 — Backup & restore

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Actual WAL shipping, base backups, PITR | **UNHANDLED → P-529** | `backup.rs` `ContinuousArchiver` models WAL over an abstract `WalOffset`; floor: "no live Postgres on this floor … the real `pg_basebackup`/WAL replay are the deferred floors P-S12/P-S15". P-S12 (=P-010) + P-S15 (=P-032) ship the lifecycle/migration mechanism, NOT real WAL drivers. |
| Real pg_basebackup/pg_restore or equivalent | **UNHANDLED → P-529** | `restore.rs` `restore_to_offset` models PITR over the abstract offset; floor names real `pg_restore` as the deferred floor. |
| Real object-store versioning/replication | **PARTIAL → P-529** | Tier model present (`StoreTier`); real object-store binding lands with P-441 (object-store BlobStore) for the blob tier — but the backup-versioning binding is M7. |
| Clean-target DESTRUCTIVE restore + post-restore re-erasure | **PARTIAL → P-529** | `restore_verify.rs` `RestoreVerifyGate` "spin a clean target" is modeled as an in-memory `RestoreTarget`; floor names the real provisioned DB as deferred. Re-erasure logic present (STOR-D3 modeled). P-529 makes the clean target a real provisioned DB. |
| MEASURED RPO/RTO over real data (not modeled offsets) | **UNHANDLED → P-530** | `measure_rpo()` computes from modeled timestamps; the ADR-18 RPO ≤ 5min / RTO ≤ 1h/4h numbers are asserted against the model, not measured over real data. P-530 measures at cell scale. |

### Finding 9 — Tenant & residency isolation

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Transaction-local RLS via SET LOCAL / set_config(...,true) | **PARTIAL → P-531** | `myelin-storage/src/pg.rs` uses `set_config('myelin.tenant_id', $1, false)` = SESSION-scoped (not transaction-local). On a pooled connection this can leak across checkouts. P-531 moves it to `SET LOCAL` / `set_config(..., true)`. |
| Prohibit tenant ops on bare pooled connections; reset-on-release | **UNHANDLED → P-531** | No reset-on-release guard / unscoped-query guard today. |
| Validate dynamic SQL identifiers | **UNHANDLED → P-531** | No identifier allowlist validation found. |
| Production TLS/mTLS + fail-fast region/endpoint consistency | **PARTIAL → P-531** | In-process residency write boundary + RLS `WITH CHECK` enforce region; real TLS/mTLS + connect-time region fail-fast is M7. |
| RLS policy + FORCE RLS + NOBYPASSRLS + tenant-predicate defence-in-depth | **FIXED** | `pg.rs` `CREATE POLICY … USING (tenant_id = current_setting(...))`, `FORCE ROW LEVEL SECURITY`, app role `NOSUPERUSER NOBYPASSRLS`, explicit tenant predicate threaded in addition to RLS. |

### Finding 10 — Secret handling

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| Redacted Debug impls | **PARTIAL → P-532** | `RawKey`/`Dek` redact in `Debug` (`key_origin.rs:109`, `kms.rs`). Residual: sweep every token/credential/secret-broker type. |
| secrecy/zeroize-backed types | **UNHANDLED → P-532** | No `zeroize`/`secrecy` wrappers found on key/token/credential types (only private fields + Debug redaction). |
| Avoid unnecessary Serialize on bearer credentials | **UNHANDLED → P-532** | Audit + remove unnecessary `Serialize` on bearer types; add a lint. |
| Tests proving credentials/keys cannot leak | **UNHANDLED → P-533** | No logging/tracing leak test exists; P-533 adds the sentinel-leak corpus across all sinks (Debug + Display + Error + panic). |

### Finding 11 — Supply-chain & security governance

| Sub-point | Disposition | Evidence / Filling prompt |
|---|---|---|
| SHA-pinned GitHub actions | **UNHANDLED → P-534** | `.github/workflows/ci.yml` uses `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `jlumbroso/free-disk-space@main` (tag/branch, not SHA). |
| Digest-pinned images | **UNHANDLED → P-534** | `docker-compose.dev.yml` uses `postgres:16`, `rustfs/rustfs:latest`, `valkey/valkey:8`, etc. (tags, not digests). |
| Pinned Rust toolchain | **UNHANDLED → P-534** | No `rust-toolchain.toml`; CI uses `@stable`. (`Cargo.lock` IS committed — good.) |
| cargo-audit/cargo-deny advisory + license policy | **UNHANDLED → P-535** | No `deny.toml`; no cargo-audit/cargo-deny in CI. |
| SBOM + build provenance | **UNHANDLED → P-536** | No SBOM generation, no SLSA provenance/attestation in workflows. |
| SECURITY.md + ownership/review policy + vuln response | **UNHANDLED → P-538** | No `SECURITY.md`, no `CODEOWNERS`. |
| Reproducible release artifacts | **UNHANDLED → P-537** | No reproducible-build config / double-build check. |
| Committed loud lint/coverage/integration gates | **FIXED** | `ci.yml`/`integration.yml` — lints, contract-coverage meta-gate, integration gate, all loud-never-swallowed. |

---

## 3. Summary of dispositions

- **FIXED (no new prompt, or verify-only):** Finding 1 (config/ports/pool/relay/no-exit), Finding 3 (lifetime enforcement, expired-tuple exclusion), Finding 4 (sandbox *hardening posture*: jailer/cgroup/seccomp/caps/zero-swap/PID + egress default-deny + ephemeral teardown — these are real; the job-exec path is NOT, see below), Finding 5 (mechanical gates, fail-not-skip KVM, semantic CDC), Finding 7 (AEAD/AAD, fallible errors, crypto-shred model), Finding 9 (RLS policy/FORCE/NOBYPASSRLS/tenant-predicate), Finding 11 (committed loud gates).
- **PARTIAL (real mechanism, production residual):** Finding 1 (OTel/OS-signal → P-539), **Finding 4 (sandbox job-exec: neither backend runs `spec.command` through production `launch()` — Firecracker `oneshot=true`/`init=/bin/true`, gVisor probes `runsc --version` only; the escape drills run through special harnesses, not the prod exec path → P-544 impl / P-545 verify)**, Finding 7 (HSM root → P-524; zeroize → P-525), Finding 8 (real restore drivers / measured RPO/RTO → P-529/P-530), Finding 9 (SET LOCAL / reset-on-release / TLS → P-531), Finding 10 (redaction sweep → P-532).
- **UNHANDLED (no existing prompt fills it):** Finding 2 (ALL real auth/token crypto → P-526/P-527/P-528), **Finding 4 (a real Firecracker + gVisor production JobSpec.command execution path → P-544; a production-path escape drill on both backends → P-545)**, Finding 6 (durable identity stores → P-522/P-523), Finding 8 (real WAL/PITR drivers → P-529), Finding 10 (zeroize/Serialize/leak-tests → P-532/P-533), Finding 11 (ALL supply-chain → P-534..P-538).
- **Important non-finding:** the audit's claim that the prompt PROSE "ships real crypto / real pg_basebackup" is true of the prose, but the EXECUTED CODE shipped documented structural floors instead — the code-wins-over-docs divergence (EI-01 §1) is exactly what M7 closes. No existing prompt re-opens those floors, because each was *named* a floor and the named follow-on ("P5/P6") was never a ledger prompt.

**Conclusion:** the platform is **production-shaped, not production-ready**. The mechanisms whose *shape* is frozen and proven are: the sandbox **hardening posture** (jailer/cgroup/seccomp/caps/egress default-deny/ephemeral teardown — real), the RLS policy, the AEAD/AAD crypto, the gate machinery, the bounded pool, the graceful-drain logic. The mechanisms still on a documented floor — durable identity stores, real credential/token cryptography, the HSM root, real backup/restore drivers, secret zeroization, the entire supply-chain governance surface, AND (corrected on re-audit) **a real sandbox JobSpec.command execution path** (neither Firecracker nor gVisor runs `spec.command` through its production `launch()`; the escape drills pass only on special harnesses) — are filled by M7 (P-522..P-546), with separate verification prompts and a fail-closed release gate (P-546) that cannot go green over any open floor, mock, scan miss, unrun production exec path, or unrecorded external/human blocker.
