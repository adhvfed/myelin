# Phase 7 — Implementation Prompts: Production Readiness & Security Hardening (M7, cross-cutting)

> Prompt count: **25** prompts (PR-01..PR-25), global ids **P-522..P-546**, band **M7**.
>
> Phase: `07-prompts/by-system`. This is a **cross-cutting** prompt file — deliberately NOT one-system-per-file
> like its siblings — because the production-readiness band M7 fills *floors that span systems*: the
> structural-credential floor lives in `myelin-identity-service` but the durable-store floor it depends on lives
> in `myelin-storage`/`myelin-substrate`, the KMS-HSM floor lives in `myelin-storage`, the supply-chain floor
> lives in `.github/` + repo root, and the final release gate reads all of them. Authoring each as a prompt in
> its owning-system file would scatter the band and hide the dependency spine that makes M7 a coherent
> "everything that is still a documented floor when the M6 dogfood gate is reached." The file header for the
> band rationale + the floor→filling-prompt cross-reference lives here; the global order is fixed by
> [`../README.md`](../README.md) §2 and the coverage by [`../coverage-matrix.md`](../coverage-matrix.md).
> Authored to the ledger template ([`../00-ledger-overview.md`](../00-ledger-overview.md) §2). Plain-text
> identifiers (no backticks-as-emphasis). Markdown only; no git commits by this document or its author.
>
> **Why M7 exists (read the audit first).** The audit
> [`../production-readiness-audit.md`](../production-readiness-audit.md) verified, against the LATEST LOCAL CODE
> (committed through P-434), that a set of production mechanisms were shipped as **documented EI-01 §1 structural
> floors** — correct in shape, honest in their `Floor named:` notes, but NOT production-real — and that **no
> existing unexecuted prompt (P-435..P-521) fills them**. The "P5/P6 follow-on" the code comments and the M1
> roadmaps reference is an old planning-*phase* label, NOT a ledger prompt id: there is no P-NNN that swaps the
> `StructuralVerifier`/`StructuralTokenVerifier`/`StructuralTokenSigner` for real cryptography, no P-NNN that
> binds a live Postgres/Valkey pool under the in-memory principal/tuple/revocation stores, no P-NNN that backs
> the KMS root with an HSM, and no P-NNN that replaces the modeled-WAL restore with a real `pg_basebackup`/
> `pg_restore`. M7 is the band that fills those floors and gates the platform's first production release. It runs
> AFTER M6 (the dogfood loop is the cheapest load generator and must be green first) and BEFORE any real
> customer tenant data is admitted.
>
> **The two doctrines that bind M7 hardest:** (1) EI-01 §3 prove-it-or-it-isn't-real — a model/mock/dogfood
> NEVER proves a production mechanism, so every M7 mechanism prompt has a SEPARATE verification prompt, and the
> final gate refuses to read a mechanism's own self-claim. (2) EI-01 §1 name-your-floors / code-wins-over-docs —
> M7 itself names the floors it cannot close in code (third-party pentest, HSM procurement, sub-processor DPA)
> as explicit external/human blockers, recorded, never silently swallowed.

---

## Canon every prompt in this file assumes (the shared reading set)

Each prompt re-states the precise subset it needs, but all assume: the M0 substrate
([`../00-ledger-overview.md`](../00-ledger-overview.md) §6) — the Cargo workspace + glue crates, `serve(AppSpec)`,
the transactional outbox + EventHandler template, the twelve committed lints, the contract-coverage scanner, the
failure-injection harness (the 1x/10x/30x load generator + the scoped-reversible dependency-break injector + the
telemetry-assertion library reading contract 1.8); the M1..M6 surfaces are all GREEN (M7 is post-M6). The audit
that motivates each prompt is [`../production-readiness-audit.md`](../production-readiness-audit.md) (cited by
finding number F1..F11 per prompt). The frozen architecture is the refined-shared docs under
[`../../05-refined-shared-systems-architecture/`](../../05-refined-shared-systems-architecture/) and the
contract-index; M7 implements those contracts to their FROZEN shape — it ships the real mechanism behind a seam
the earlier band already froze, so it is a binding/impl swap, not a redesign. The doctrine anchors are
[`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md)
(§1 name-your-floors + code-wins-over-docs; §2 order-by-non-negotiability; §3 prove-it + observability-is-part-of-
the-pass; §5 the committed ratchet) and
[`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) (§1 erasure-vs-
immutability, §5 untrusted-code-execution). A prompt's DEFINITION OF DONE requires all committed lints green and
the contract-coverage scanner passing in addition to its own gate.

**Band-internal ordering (the strict dependency spine).** Implementation prompts ship a mechanism; verification
prompts prove it on real infrastructure and are SEPARATE prompts (a mechanism and its proof are never the same
prompt — EI-01 §3). The spine: durable persistence (P-522/523) is the substrate every other M7 mechanism binds
its real store to → real KMS-HSM (P-524/525) → real auth crypto (P-526/527/528) depends on both → real
backup/restore drivers (P-529/530) depend on durable persistence → tenant-isolation hardening (P-531) depends on
the live pool → secret-handling sweep (P-532/533) is independent → supply-chain governance (P-534..P-539) is
largely independent of the runtime work and can run in parallel after P-522 → the gate-integrity truth-up
(P-540/541) reads everything → the external human-blocker reviews (P-542/543) are recorded → the real sandbox
production exec path (P-544 impl) → the production-path sandbox exec + escape drill on both backends (P-545
verify) → the final fail-closed release gate (P-546) is last and reads them all.

**The floor→filling-prompt cross-reference (this band closes these named floors):**

| Named floor (where recorded) | Filled by |
|---|---|
| In-memory principal/tuple/revocation stores ("models the SQL S1/S3/S7 table…until the driver lands P-S15") — `principal_store.rs`, `tuple_store.rs`, `revocation.rs` | **P-522** (impl), **P-523** (verify) |
| `StructuralVerifier` floor ("real OIDC JWKS / SAML XML-DSig / WebAuthn / SSH … is the named P5/P6 floor") — `authenticate.rs` | **P-526** (impl), **P-528** (verify) |
| `StructuralTokenVerifier`/`StructuralTokenSigner` floor ("real PASETO sign / biscuit caveat crypto / DPoP proof … the named P5/P6 floor") — `machine_auth.rs`, `mint.rs` | **P-527** (impl), **P-528** (verify) |
| `StructuralAttestationVerifier` floor ("without pretending to do TPM crypto") — `self_hosted.rs` | **P-527** (impl), **P-528** (verify) |
| HSM/sealed L0 cell root ("on this in-cell software floor it is a process-held root … the HSM/Shamir-split-recovery backing is the production hardening follow-on") — `kms.rs` `CellRoot` | **P-524** (impl), **P-525** (verify) |
| Modeled-WAL backup/restore ("no live Postgres on this floor … the real `pg_basebackup`/`pg_restore`/WAL replay are the deferred floors P-S12/P-S15") — `backup.rs`, `restore.rs`, `restore_verify.rs` | **P-529** (impl), **P-530** (verify) |
| Supply-chain: actions/images/toolchain unpinned, no cargo-deny, no SBOM/provenance, no SECURITY.md/CODEOWNERS — `.github/`, repo root | **P-534..P-539** |
| Sandbox job-exec floor: neither backend runs `JobSpec.command` through production `launch()` — Firecracker hardcodes `oneshot=true` → `init=/bin/true` (`firecracker.rs:327-328`); gVisor's `spawn_real_runsc` only probes `runsc --version` (`gvisor.rs:227-237`); escape drills pass on special harnesses, not the prod exec path (audit F4) | **P-544** (impl), **P-545** (verify) |

---

### P-522 — Bind the live OLTP/cache pool under Identity's principal, tuple, and revocation stores (durable persistence)

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the durable-persistence floor; fills the "until the driver lands P-S15" deviation in `principal_store.rs`/`tuple_store.rs`/`revocation.rs`) — [`../production-readiness-audit.md`](../production-readiness-audit.md) Finding 6.
- **DEPENDS-ON.** P-507 (the dogfood lint/scanner/mutation pipeline is live), P-061 (the restore-verify gate exists), P-507. (M7 is post-M6; the band sort places this after every M6 prompt.)
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (GDPR-safe by construction), §4 (the quality bar).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (code-wins-over-docs: the floor-note in each store crate is the divergence to close), §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §2 (the S1/S3/S5/S7/S8 store table + their backing tiers), §6 (the tuple shape + partition).
  - [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §2 (the OLTP tier 11.1 + RLS + outbox co-location), §3 (the bounded pool).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 11.1, 4.6, 4.7; the existing `myelin-storage::pg` / `oltp` modules (the real Postgres seam already used by chat + storage tests).
  - The code to fix: `crates/myelin-identity-service/src/principal_store.rs`, `tuple_store.rs`, `revocation.rs` (the `Inner { partitions: HashMap … }` / `BTreeMap` floors + their `Floor named:` notes).
- **DELIVERABLE.** In crate `myelin-identity-service`: replace the in-memory `Inner` of the S1 principal store, the S3 tuple store, and the S7 revocation store with bindings to the REAL backing tiers behind the unchanged trait/struct surface — S1/S3 on the `myelin-storage` `OltpPool` (the same RLS-scoped, `(tenant,region)`-partitioned, per-subject-DEK-encrypted Postgres path `crates/myelin-storage/src/pg.rs` + `oltp.rs` already ship and the chat/storage integration tests already exercise); S7 on the Valkey + PG-mirror two-layer path. Keep the byte-for-byte contract semantics (the floor notes assert the seam shape does not change). The constructors used by the Identity `AppSpec`/`serve` wiring now take the pool/cache handles (env-validated config); the in-memory variants survive ONLY as `#[cfg(test)]` fakes, never on a production constructor. Forward-only migrations install the S1/S3/S7 schemas via the P-S15 runner. **Floor named:** the OLAP read store (S5 replica) lag tunables are measured at scale (already P-ID-32, no new floor); cross-cell principal authority is already P-ID-35. No floor is left open by this prompt.
- **CONTRACTS TO IMPLEMENT.** Owns the real binding of: 11.1 (OLTP tier under S1/S3), the S7 store. Consumed: 2.2/2.4 (outbox emit unchanged), 1.4 (holder auto-registration over the real stores), 12.1 ((tenant,region) partition).
- **GATE / DRILLS (quantified; must be green to call this done).** Run under `--features integration` against the live docker-compose stack (Postgres + Valkey), `MYELIN_REQUIRE_DB=1` set so a DB-free run HARD-FAILS (not skips). (1) Restart drill: write principals + tuples + revocations, kill+restart the service, assert 100% of committed rows survive (0 lost — the in-memory floor would lose all). (2) Multi-instance drill: two service instances over one DB observe the same tuple write within the zookie consistency bound; 0 split-brain. (3) `tenant-predicate` lint green on every new S1/S3/S7 query; RLS `WITH CHECK` refuses a cross-region write (assert 42501). Telemetry: `outbox_depth==0` at drain; `cross_tenant_count==0`. Green artifact: the integration suite + an M7 scorecard row, dated.
- **TESTS (required).** Unit: each store's CRUD over the real pool round-trips; a `#[cfg(test)]` fake still satisfies the trait. Integration (the gate above): restart-survival + multi-instance + RLS-refusal. CDC: re-affirm the 11.1 + 4.6 provider+consumer pairs now exercise the live pool, not the fake. Mutation floor: the partition-key + RLS-scope derivation are mandatory-core — state and meet the floor (a mutation dropping the tenant predicate or the region scope MUST be caught).
- **DEFINITION OF DONE.** The three stores bind the live OLTP/cache pool behind the unchanged surface; the in-memory variant is `#[cfg(test)]`-only (a grep proves no production constructor instantiates it); the restart-survival + multi-instance + RLS drills emit dated green artifacts at integration scale; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-522 M7: live OLTP/cache binding under Identity principal/tuple/revocation stores`. Body: 11.1 + S7 real binding; the restart-survival/multi-instance/RLS measured results; the in-memory-is-test-only proof; the mutation score. Co-Authored-By trailer.

---

### P-523 — Verify durable persistence: crash/restart + multi-instance + no in-memory store in the production graph

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the durable-persistence VERIFICATION; separate from P-522 per EI-01 §3) — Finding 6.
- **DEPENDS-ON.** P-522.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (a property is not real until a drill forces the failure and observability watches it survive), §5 (the committed ratchet).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) (the durability/loss drill family).
  - The audit Finding 6 (the list of in-memory floors); the `myelin-identity-service` store modules.
- **DELIVERABLE.** A committed CI verification job + a `dependency-graph` assertion: (a) a build-graph scanner (extend the contract-coverage / lint-gate tooling) that FAILS the build if any production (non-`#[cfg(test)]`) constructor in `myelin-identity-service` (or any service crate) instantiates an in-memory `BTreeMap`/`HashMap`-backed durable store named in the audit — the structural proof that the floor is ABSENT from the production dependency graph. (b) a hardened crash-recovery drill: write N=10000 mixed principal/tuple/revocation rows, `kill -9` the service mid-write-batch, restart, assert 0 committed rows lost and 0 uncommitted rows resurrected. (c) a multi-instance consistency drill at 3 instances over one DB. This prompt PROVES P-522's mechanism on real infrastructure; it does not re-implement it.
- **CONTRACTS TO IMPLEMENT.** None new — verifies 11.1 + S7 + 1.4 already implemented in P-522.
- **GATE / DRILLS (quantified).** The build-graph scanner is green (0 in-memory durable stores in any production constructor) AND red on a deliberately-reintroduced in-memory constructor fixture (proves the scanner bites). The crash-recovery drill: 0 lost / 0 resurrected at N=10000 under `kill -9`. The 3-instance drill: 0 divergence. All under `MYELIN_REQUIRE_DB=1` (DB-free run hard-fails). Dated green artifact on the M7 scorecard.
- **TESTS (required).** The scanner self-test (red fixture ⇒ non-zero, green ⇒ zero). The crash-recovery integration test. The multi-instance integration test. Mutation floor: the scanner's "is-this-a-production-constructor" predicate is mandatory-core.
- **DEFINITION OF DONE.** The in-memory-store-absence scanner is committed + wired loud-never-swallowed + proven to bite; the crash-recovery + multi-instance drills emit dated green artifacts; committed.
- **COMMIT.** Header `P-523 M7: verify durable persistence (crash/restart + multi-instance + no in-memory store in prod graph)`. Body: the scanner + its self-test; the crash-recovery/multi-instance measured results. Co-Authored-By trailer.

---

### P-524 — Back the KMS root with a durable HSM-class KMS: root/KEK lifecycle, rotation, recovery, destruction

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the KMS-HSM floor; fills the "process-held root … the HSM/Shamir-split-recovery backing is the production hardening follow-on" note in `kms.rs`) — Finding 7.
- **DEPENDS-ON.** P-522 (the durable backing the wrapped-key store binds to).
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (GDPR crypto-shred), §4.
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (the floor note in `kms.rs` is the divergence to close), §3.
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (crypto-shred = the erasure lever; a destroyed key must never be resurrectable).
  - [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §4 (the three-level KMS: L0 root HSM/sealed → L1 per-(tenant,region) KEK → L2 DEK; the `KmsAdapter` seam; fail-static availability; never fail-open), §4 the `[OPEN → P6/LEGAL]` HYOK-policy/KMIP residual.
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 11.3.
  - The code to fix: `crates/myelin-storage/src/kms.rs` (`CellRoot` is `RawKey::generate()` in-process; the `KmsEngine` holds KEKs/DEKs in `Mutex<BTreeMap…>`).
- **DELIVERABLE.** In crate `myelin-storage`: behind the existing `KmsAdapter` seam, (a) a real durable KMS adapter (Vault Transit / PKCS#11 HSM-class) for the L0 cell root — the root is generated/sealed in the HSM and NEVER exported to process memory (the `KmsEngine` calls wrap/unwrap THROUGH the adapter, it does not hold the root); a software-dev adapter survives only for `#[cfg(test)]`/dogfood. (b) durable persistence of the wrapped KEK/DEK envelopes (the `Mutex<BTreeMap>` becomes a binding to the OLTP/object tier from P-522 — wrapped only, never plaintext). (c) the L0 root lifecycle: generate, Shamir-split recovery (k-of-n), rotation (root re-wraps all KEKs, O(KEKs)), and durable destruction (a destroyed root/KEK is unrecoverable, the crypto-shred lever). (d) confirm AEAD associated-data binds `(tenant, object/field, key-ref, epoch)` on every seal (already largely present — assert it). Keep all errors fallible (`Result`, never panic/unwrap on a key path) and fail-static-never-open. **Floor named (external/human blocker):** the physical HSM device procurement + the KMIP/external-key-store per-content-class HYOK *policy* remain `[OPEN → P6/LEGAL]` — the ADAPTER + the lifecycle are built and proven against a software-HSM-emulator (e.g. SoftHSM) in CI; production keying ceremony on a real HSM is recorded as a human blocker for P-546.
- **CONTRACTS TO IMPLEMENT.** Owns the real backing of 11.3 (KMS hierarchy + `KeyOrigin`). Consumed: 11.1 (the durable wrapped-envelope store).
- **GATE / DRILLS (quantified).** (1) Root-never-in-process: a memory/heap assertion + an API audit prove no public path returns the L0 root plaintext; 0 root exports. (2) Destruction-is-permanent: destroy a tenant KEK, restore the whole KMS from backup, assert the destroyed key is NOT resurrected (0 recoverable shredded keys — the STOR-D4/crypto-shred floor at the KMS layer). (3) Rotation: rotate the root, assert all KEKs re-wrap, 0 DEK plaintext touched, all ciphertext still decryptable. (4) Fail-static: KMS hiccup → resolved-DEK reads survive the bounded TTL, hard-down → not-ready + shed, 0 fail-open. Run against SoftHSM under `--features integration`. Dated green artifact.
- **TESTS (required).** Unit: wrap/unwrap through the adapter; AAD-mismatch rejects (a ciphertext with a swapped tenant/epoch/key-ref fails to open). Integration: the four drills above. CDC: re-affirm 11.3 provider+consumer pair through the real adapter. Mutation floor ≥ 80% on the wrap/unwrap + the fail-open-prevention branch (mandatory core — KMS is the highest bar; a mutation that returns plaintext-without-key or that opens a swapped-AAD ciphertext MUST be caught).
- **DEFINITION OF DONE.** The HSM-class adapter + durable wrapped-envelope store + L0 lifecycle (generate/split-recover/rotate/destroy) exist behind the unchanged seam; the four drills emit dated green artifacts against SoftHSM; the real-HSM keying ceremony is recorded as a human blocker for P-546; the HYOK/KMIP policy residual stays `[OPEN → P6/LEGAL]`; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-524 M7: durable HSM-class KMS — root never in process, destruction permanent, rotation O(keys)`. Body: 11.3 real backing; root-never-exported + destruction-permanent + rotation + fail-static measured results; the real-HSM-ceremony human blocker; the HYOK/KMIP `[OPEN → P6/LEGAL]` residual; the mutation score. Co-Authored-By trailer.

---

### P-525 — Verify KMS: zeroization, secret-memory handling, and backup-cannot-resurrect-destroyed-keys

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the KMS VERIFICATION; separate from P-524) — Finding 7 + Finding 10 (zeroization).
- **DEPENDS-ON.** P-524.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; observability is part of the pass).
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (a destroyed key must never come back, even via restore).
  - [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §4; the audit Findings 7 + 10.
  - The code: `crates/myelin-storage/src/kms.rs` (`RawKey` Debug-redaction + private bytes), `key_origin.rs` (`Dek` redaction); `kms_failstatic.rs`.
- **DELIVERABLE.** (a) Make transient key material zeroize-on-drop: add `zeroize`/`secrecy`-backed wrappers to `RawKey`/`Dek`/the unwrap buffers so plaintext key bytes are scrubbed when they leave scope (verify the Debug-redaction already present, add the zeroization). (b) A drill that proves a destroyed key cannot be resurrected by ANY restore path: destroy a KEK, run the FULL restore-verify (P-529's real restore once it lands; against the modeled restore today) + the KMS backup_snapshot, grep the restored key set, assert 0 destroyed keys present. (c) A logging/tracing leak test: drive a wrap/unwrap/rotate cycle with a tracing subscriber capturing all spans+logs at TRACE, assert 0 occurrences of raw key bytes in the captured output. This PROVES P-524's mechanism; it does not re-implement it.
- **CONTRACTS TO IMPLEMENT.** None new — verifies 11.3.
- **GATE / DRILLS (quantified).** Zeroization: a test that reads the backing buffer after drop sees 0 non-zero key bytes (best-effort under the `zeroize` guarantee). Resurrection: 0 destroyed keys in any restored set. Leak: 0 raw-key-byte occurrences in TRACE-level capture across wrap/unwrap/rotate/restore. Dated green artifact.
- **TESTS (required).** The zeroize-on-drop test; the no-resurrection integration test; the no-leak tracing test. Mutation floor: the zeroization call sites are mandatory-core (a mutation removing a `zeroize()` MUST be caught).
- **DEFINITION OF DONE.** Key material zeroizes on drop; destroyed keys are proven unrecoverable across restore; the tracing-leak test is green at 0; committed.
- **COMMIT.** Header `P-525 M7: verify KMS — zeroization + no-resurrection-across-restore + no-key-leak-in-traces`. Body: the zeroize wiring; the resurrection + leak measured results. Co-Authored-By trailer.

---

### P-526 — Real human/SSO credential cryptography: OIDC JWKS, SAML XML-DSig, WebAuthn, SSH challenge

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (fills the `StructuralVerifier` floor — "real OIDC JWKS-signature / SAML XML-DSig / WebAuthn attestation / SSH challenge-response verification is the named P5/P6 floor") — Finding 2.
- **DEPENDS-ON.** P-522 (live principal store), P-524 (real KMS for any key material).
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3.
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (the floor note in `authenticate.rs` is the divergence to close), §2 (the IDOR/auth floor is stop-the-bleeding), §3.
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §4 (the auth surfaces — SAML 2.0 / OIDC / SCIM 2.0 / WebAuthn-FIDO2 / SSH; tenant from the verified credential; the hardware-attestation/passkey-sync/SLO residual).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.1.
  - The code to fix: `crates/myelin-identity-service/src/authenticate.rs` (`StructuralVerifier` parses the `"<tenant>|<region>|<subject_key>"` envelope; the real verifier is behind the `CredentialVerifier` seam at `with_verifier`).
- **DELIVERABLE.** In crate `myelin-identity-service`: behind the existing `CredentialVerifier` seam, implement the REAL cryptographic verifiers and wire them as the DEFAULT production verifier (replacing `StructuralVerifier` on every production constructor — `authenticate.rs` line ~289 and the `AppSpec` wiring): (a) OIDC — fetch + cache the IdP JWKS, verify the ID-token JWS signature, validate `iss`/`aud`/`exp`/`nbf`/`iat`/`nonce`, reject on any mismatch; (b) SAML 2.0 — verify the assertion's XML-DSig signature against the IdP cert, validate conditions/audience/notOnOrAfter, replay-protect on assertion id; (c) WebAuthn/FIDO2 — verify the authenticator attestation + assertion signature + challenge + RP-id + counter; (d) SSH — issue + verify a public-key challenge-response. Tenant is taken from the verified credential, never the URL path (the ID-3 floor, now backed by real signature trust). `StructuralVerifier` survives only as a `#[cfg(test)]` fake. **Floor named:** hardware-attested device binding + full passkey-sync governance + SAML SLO remain the named follow-on (now correctly recorded as a post-M7 enterprise increment, NOT a release blocker — record in writing); SCIM deprovision is the authoritative revocation path.
- **CONTRACTS TO IMPLEMENT.** Owns the real cryptographic backing of 4.1 (the human/SSO half). Consumed: 11.1 (live principal store), 11.3 (KMS), 1.8 (auth telemetry).
- **GATE / DRILLS (quantified).** (1) Forged/tampered credential rejected: a JWS with a flipped signature byte, a SAML assertion with a tampered XML node, a WebAuthn assertion with a wrong challenge, an SSH response to a wrong nonce — each → authenticate FAILS (0 forged credentials admitted). (2) Claim validation: an expired (`exp` past), not-yet-valid (`nbf` future), wrong-audience, or replayed credential → FAILS (0 admitted). (3) Tenant-from-credential: cross-tenant path spoof resolves to the credential's tenant, 0 path-derived tenants. Telemetry: `auth_decision_latency` per request. Dated green artifact on the M7 scorecard.
- **TESTS (required).** Unit: one happy-path + one forged + one expired + one wrong-audience per credential kind (OIDC/SAML/WebAuthn/SSH) against test vectors / a mock IdP. CDC: re-affirm 4.1 provider+consumer through the real verifier. Mutation floor (mandatory core — auth is a Tier-0/Tier-4 keystone): the signature-verify + claim-validation branches; a mutation that accepts a bad signature, skips `exp`/`aud`, or derives tenant from the path MUST be caught.
- **DEFINITION OF DONE.** Real OIDC/SAML/WebAuthn/SSH verification is the default production `CredentialVerifier`; `StructuralVerifier` is `#[cfg(test)]`-only (a grep proves no production constructor uses it); the forged/expired/replay drills emit dated green artifacts; the hardware-attestation/passkey-sync residual is recorded as a post-M7 increment; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-526 M7: real human/SSO credential cryptography (OIDC JWKS, SAML XML-DSig, WebAuthn, SSH)`. Body: 4.1 real crypto (human/SSO); the forged/expired/replay measured results; the StructuralVerifier-is-test-only proof; the post-M7 hardware-attestation residual; the mutation score. Co-Authored-By trailer.

---

### P-527 — Real token cryptography: signed capability/machine tokens, DPoP proof, TPM attestation

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (fills the `StructuralTokenVerifier`/`StructuralTokenSigner`/`StructuralAttestationVerifier` floors — "real PASETO sign / biscuit caveat crypto / DPoP proof … the named P5/P6 floor"; "without pretending to do TPM crypto") — Finding 2.
- **DEPENDS-ON.** P-522 (live revocation store S7), P-524 (KMS for the signing keys), P-526 (the verifier seam pattern).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (the floor notes in `mint.rs`/`machine_auth.rs`/`self_hosted.rs` are the divergence to close), §2 (an agent must not exceed a human — the attenuation floor), §3.
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §4 (token = attenuable PASETO/JWT + macaroon/biscuit caveat chains + DPoP sender-constrains long-lived PATs; revocation = denylist S7 + short TTL), §3 (machine-identity; the self-hosted-runner attestation).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.1, 4.7 (`mint_run_token`).
  - The code to fix: `crates/myelin-identity-service/src/machine_auth.rs` (`StructuralTokenVerifier`, the `dpop ∈ {0,1}` flag-only check), `mint.rs` (`StructuralTokenSigner`, `dpop=0`), `self_hosted.rs` (`StructuralAttestationVerifier`).
- **DELIVERABLE.** In crate `myelin-identity-service`: behind the existing `TokenSigner`/`TokenVerifier`/attestation seams, implement the REAL token cryptography and wire it as the DEFAULT on every production constructor (`mint.rs` ~253/265, `machine_auth.rs` ~406): (a) signer — mint capability/machine/run tokens as cryptographically signed PASETO (or biscuit) envelopes with macaroon/biscuit caveat chains; the signing key comes from the KMS (P-524). (b) verifier — verify the signature, the caveat chain (attenuation is monotone, never amplifies), `iss`/`aud`/`exp`/`nbf`/`jti`, and consult the live S7 revocation store. (c) DPoP — verify a real RFC 9449 proof-of-possession (the proof's JWK thumbprint matches the token's `cnf`, the `htm`/`htu`/`iat`/`jti` bind the request, replay-protected) for long-lived PATs. (d) self-hosted-runner attestation — verify a real TPM quote / provisioning signature (`StructuralAttestationVerifier` → a real one). The Structural* impls survive only as `#[cfg(test)]` fakes. **Floor named:** none open — this closes the token-crypto floor entirely (the short-lived per-run token's TTL-as-constraint posture is preserved, now backed by a real signature).
- **CONTRACTS TO IMPLEMENT.** Owns the real cryptographic backing of 4.1 (token half) + 4.7 (`mint_run_token` real signing). Consumed: S7 (live revocation), 11.3 (KMS signing key), 1.8.
- **GATE / DRILLS (quantified).** (1) Forged token rejected: a token with a flipped signature byte, a forged caveat extension that widens authority, an expired token, a token whose `jti` is on the S7 denylist → each FAILS (0 forged/expired/revoked admitted). (2) Attenuation monotone: an attenuated PAT resolves to strictly-smaller authority than its parent (0 amplifications). (3) DPoP: a token presented without its bound proof, or with a replayed proof, → FAILS (0 unbound bearer uses of a sender-constrained token). (4) Attestation: a runner with a forged/absent TPM quote cannot mint a self-hosted-runner token. Dated green artifact.
- **TESTS (required).** Unit: sign→verify round-trip; forged-signature reject; expired reject; revoked-via-S7 reject; attenuation-narrows; DPoP bind+replay; attestation forge-reject. CDC: re-affirm 4.1 (token) + 4.7 through the real signer/verifier. Mutation floor (mandatory core): the signature-verify + caveat-monotonicity + S7-consult + DPoP-bind branches; a mutation that accepts a bad signature, an amplifying caveat, a revoked `jti`, or an unbound DPoP token MUST be caught.
- **DEFINITION OF DONE.** Real signed tokens + caveat verification + DPoP proof + TPM attestation are the default production path; the Structural* impls are `#[cfg(test)]`-only (a grep proves no production constructor uses them); the forge/expire/revoke/attenuate/DPoP/attestation drills emit dated green artifacts; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-527 M7: real token cryptography (signed PASETO/biscuit tokens, DPoP proof, TPM attestation)`. Body: 4.1 (token) + 4.7 real crypto; the forge/expire/revoke/attenuate/DPoP/attestation measured results; the Structural*-is-test-only proof; the mutation score. Co-Authored-By trailer.

---

### P-528 — Verify authentication & authz expiry: no structural verifier in the prod graph; expired grants cannot authorize

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the auth + expiry VERIFICATION; separate from P-526/P-527) — Findings 2 + 3.
- **DEPENDS-ON.** P-526, P-527.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it), §5 (committed ratchet).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §6 (per-run grants are auto-expiring tuples; expired tuples never enter a snapshot), §8.4 (zookie/consistency); §2 (S7).
  - The audit Findings 2 + 3; the existing `revocation.rs` (the S7 store + `RunTokenState` + the 5-min SLA).
- **DELIVERABLE.** A committed CI verification job + drills (PROVE the mechanisms, do not re-implement): (a) a dependency-graph scanner row that FAILS if any production constructor instantiates `StructuralVerifier`/`StructuralTokenVerifier`/`StructuralTokenSigner`/`StructuralAttestationVerifier` (the structural proof the floor is ABSENT from the production graph) — and red on a deliberately-reintroduced use (proves it bites). (b) an expired-grant drill: write a tuple with `expires_at` in the past, take an authz snapshot, run `check`/`list_objects`, assert the expired tuple NEVER authorizes (0 authorizations from expired grants) — across teardown, crash-recovery, replay, and cleanup. (c) a revocation-timing drill: revoke a token, assert every surface denies within the N=5min SLA (ID-D1 re-confirm) and that a just-revoked token is denied even on the fail-static coarse cache (ID-D2 re-confirm) now that tokens are really signed.
- **CONTRACTS TO IMPLEMENT.** None new — verifies 4.1/4.7 (P-526/527) + the S7 expiry/revocation (already shipped).
- **GATE / DRILLS (quantified).** The structural-verifier-absence scanner: 0 production uses, red on the reintroduction fixture. Expired-grant: 0 authorizations from expired tuples across all four lifecycle phases. Revocation: every surface denies within 5 min; just-revoked denied on the coarse cache. Dated green artifact.
- **TESTS (required).** The scanner self-test (red/green fixtures). The expired-grant integration test (the four phases). The revocation-SLA + fail-static drills. Mutation floor: the `expires_at`-vs-clock comparison + the S7-consult are mandatory-core (a mutation that admits an expired tuple or skips the S7 consult MUST be caught).
- **DEFINITION OF DONE.** The structural-verifier-absence scanner is committed, loud, and proven to bite; expired grants are proven unable to authorize across teardown/crash/replay/cleanup; the 5-min revocation SLA + fail-static re-confirm emit dated green artifacts; committed.
- **COMMIT.** Header `P-528 M7: verify auth + expiry (no structural verifier in prod graph; expired grants cannot authorize)`. Body: the absence-scanner + self-test; the expired-grant + revocation-SLA + fail-static measured results. Co-Authored-By trailer.

---

### P-529 — Real backup/restore drivers: WAL shipping, base backups, PITR, destructive clean-target restore

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (fills the modeled-WAL floor — "no live Postgres on this floor … the real `pg_basebackup`/`pg_restore`/WAL replay are the deferred floors P-S12/P-S15") — Finding 8.
- **DEPENDS-ON.** P-522 (the live OLTP/object pool the backup acts on), P-524 (the KMS whose destroyed keys must stay destroyed across restore).
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (silent data loss outranks every feature).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (the floor notes in `backup.rs`/`restore.rs`/`restore_verify.rs` are the divergence to close), §2 #1 (silent data loss is the top tier), §3.
  - [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §7 (backup/restore/cross-seam: WAL archiving + base backups + PITR; the per-aggregate seq cursor as the linearisation point; reindex-from-source for derived; KEK restore-except-shredded); ADR-18.
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 11.5; the existing modeled modules `crates/myelin-storage/src/backup.rs`, `restore.rs`, `restore_verify.rs` (the `ContinuousArchiver` over an abstract `WalOffset`).
- **DELIVERABLE.** In crate `myelin-storage`: behind the existing backup/restore seam, implement the REAL drivers (the abstract `WalOffset` cursor maps onto the real WAL LSN, the modeled archiver onto real ops): (a) continuous WAL archiving (real `archive_command` / WAL-G-class) + periodic base backups (`pg_basebackup`) for the OLTP tier; (b) object-tier versioning + in-region replication for T2; (c) PITR `restore(to_offset T)` via real `pg_restore` + WAL replay to the cross-seam consistency point (the outbox-seq cursor), verifying every referenced `ContentHash` is present, reindexing derived stores from source, restoring KEKs except crypto-shredded; (d) the clean-target restore is DESTRUCTIVE (it restores into a fresh provisioned target, not a model) and runs post-restore re-erasure (a subject crypto-shredded since the backup stays shredded — STOR-D3). The modeled path survives only as a `#[cfg(test)]` fast unit harness. **Floor named:** none open — the real driver replaces the model; cell-scale measurement is the separate verification prompt P-530.
- **CONTRACTS TO IMPLEMENT.** Owns the real backing of 11.5 (backup/restore/cross-seam). Consumed: 11.1 (OLTP), 11.2 (object), 11.3 (KMS).
- **GATE / DRILLS (quantified).** Run against the live docker-compose stack (real Postgres + object store) under `--features integration`, `MYELIN_REQUIRE_DB=1`. STOR-D1: rebuild from real backups to offset T → 0 loss (checksum parity), OLTP↔blob↔index↔offset at ONE consistent point, 0 dangling refs, cold == live. A deliberately-corrupted backup makes the gate FAIL (not silently pass). STOR-D3: a crypto-shredded subject is unrecoverable post-restore (0 resurrected). Dated green artifact; this re-arms the STOR-D1/STOR-D2 PERMANENT gate over the REAL driver.
- **TESTS (required).** Unit: the modeled fast harness still asserts the cross-seam invariants. Integration: the real STOR-D1 + STOR-D3 drills + the corrupted-backup-fails-CI test. CDC: re-affirm 11.5 through the real driver. Mutation floor: the consistency-point selection + the referenced-blob-presence check are mandatory-core (a mutation that restores past T, or admits a dangling ref, MUST be caught).
- **DEFINITION OF DONE.** Real WAL/base-backup/PITR drivers exist behind the unchanged seam; the destructive clean-target STOR-D1 + STOR-D3 drills emit dated green artifacts against real Postgres+object-store; the corrupted-backup case fails CI; the modeled path is `#[cfg(test)]`-only; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-529 M7: real backup/restore drivers (WAL shipping, base backups, PITR, destructive clean-target restore)`. Body: 11.5 real driver; the STOR-D1/STOR-D3 measured results over real Postgres; the corrupted-backup-fails proof; the mutation score. Co-Authored-By trailer.

---

### P-530 — Verify backup/restore: MEASURED RPO/RTO over real data at cell scale

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the backup/restore VERIFICATION; separate from P-529; replaces the modeled-offset RPO/RTO with measured numbers) — Finding 8.
- **DEPENDS-ON.** P-529.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (a target you cannot measure is not a gate).
  - [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §7 (RPO ≤ 5 min, RTO ≤ 1h/tenant ≤ 4h/cell — the ADR-18 numbers); the audit Finding 8 (RPO/RTO were computed from modeled offsets).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) (STOR-D1/STOR-D2).
- **DELIVERABLE.** A scheduled (SCHED) verification job that MEASURES, over real data on the real driver (P-529): (a) RPO — the actual data-loss window between the last archived WAL segment and the crash point, over a continuously-written cell-scale dataset; assert ≤ 5 min. (b) RTO — the wall-clock time to restore a tenant (≤ 1h) and a cell (≤ 4h) over real data volume. (c) STOR-D2 at cell scale under world-scale write load. These are MEASURED numbers written to the thresholds file, not modeled offsets; a miss becomes a dated "claimed, not proven" row, never edited green. This PROVES P-529; it does not re-implement it.
- **CONTRACTS TO IMPLEMENT.** None new — verifies 11.5.
- **GATE / DRILLS (quantified).** Measured RPO ≤ 5 min; measured tenant-RTO ≤ 1h; measured cell-RTO ≤ 4h; 0 loss; over real data at cell scale. Dated green artifact on the M7 scorecard; the thresholds file records the measured numbers.
- **TESTS (required).** The cell-scale restore-measurement drill (SCHED). The continuous-write RPO drill. Mutation floor: n/a (verification job) — but the measurement code's window/clock arithmetic carries unit tests.
- **DEFINITION OF DONE.** RPO/RTO are MEASURED over real data at cell scale and meet ADR-18; the numbers are in the thresholds file; STOR-D2-at-cell-scale is green; a miss is recorded honestly; committed.
- **COMMIT.** Header `P-530 M7: verify backup/restore — measured RPO/RTO over real data at cell scale`. Body: the measured RPO/RTO numbers; STOR-D2 at cell scale; the thresholds-file update. Co-Authored-By trailer.

---

### P-531 — Production tenant-isolation hardening on the live pool: SET LOCAL RLS, reset-on-release, identifier validation, mTLS

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (tenant & residency isolation on the LIVE pool; tightens the in-process/RLS twin onto the real connection pool) — Finding 9.
- **DEPENDS-ON.** P-522 (the live pool).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §2 (cross-tenant leak is stop-the-bleeding), §3.
  - [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §2 (RLS via session/transaction GUCs; the app role is NOBYPASSRLS; the tenant-predicate defence-in-depth), §3 (the bounded pool; reset-on-release); the `residency-pin` discipline.
  - [`../../05-refined-shared-systems-architecture/tenancy-and-control-plane.md`](../../05-refined-shared-systems-architecture/tenancy-and-control-plane.md) §region-pinning (TLS/mTLS + region/endpoint consistency).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 11.1, 12.1, 12.4.
  - The code: `crates/myelin-storage/src/pg.rs` (`set_session_scope_in_region` uses `set_config(..., false)` = session-scoped; the RLS policy + `FORCE ROW LEVEL SECURITY`).
- **DELIVERABLE.** In crate `myelin-storage`: harden tenant isolation for pooled production connections: (a) move the per-request tenant scope to TRANSACTION-local `SET LOCAL` / `set_config(..., true)` so a pooled connection cannot leak a prior tenant's GUC to the next checkout; (b) reset-on-release: every pool checkout begins by clearing the tenant/region GUCs, and a guard FORBIDS a tenant-scoped query on a bare (un-scoped) pooled connection (a query without a set scope errors, not silently runs unscoped); (c) validate any dynamic SQL identifier (schema/table) against an allowlist (no injection via a tenant-controlled identifier); (d) require production TLS/mTLS to Postgres + the object store + the cache, with fail-fast region/endpoint consistency (an out-of-region endpoint refuses at connect). **Floor named:** none open — this is the production tightening of the already-present RLS mechanism.
- **CONTRACTS TO IMPLEMENT.** Tightens 11.1 (RLS + bounded pool) + 12.1/12.4 (residency). Consumed by every tenant-store query.
- **GATE / DRILLS (quantified).** (1) Pool-leak drill: tenant A's connection is released and re-checked-out for tenant B; assert B never sees A's GUC, and a query before the scope is set ERRORS (0 unscoped tenant queries). (2) RLS `WITH CHECK` refuses a cross-region write (42501). (3) An out-of-region endpoint or a non-TLS connection refuses at connect (fail-fast). (4) An injected dynamic identifier is rejected. Under `--features integration`. Dated green artifact.
- **TESTS (required).** The pool-leak/reset-on-release integration test; the cross-region-write refusal; the TLS/region fail-fast test; the identifier-validation test. CDC: re-affirm 11.1 with transaction-local scope. Mutation floor: the reset-on-release + the unscoped-query-guard are mandatory-core (a mutation that skips the reset or admits an unscoped query MUST be caught).
- **DEFINITION OF DONE.** Transaction-local `SET LOCAL` RLS + reset-on-release + unscoped-query-guard + identifier-validation + TLS/mTLS-and-region fail-fast are live on the production pool; the pool-leak + cross-region + TLS drills emit dated green artifacts; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-531 M7: production tenant-isolation hardening (SET LOCAL RLS, reset-on-release, identifier validation, mTLS)`. Body: the transaction-local scope + reset-on-release + guard + TLS/region fail-fast; the pool-leak measured results; the mutation score. Co-Authored-By trailer.

---

### P-532 — Secret-handling sweep: redacted Debug, secrecy/zeroize types, no Serialize on bearer credentials

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (secret handling across every credential/key/token type) — Finding 10.
- **DEPENDS-ON.** P-527 (the real token types), P-524 (the real key types).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (a secret in a log is a compromise), §3.
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §4 (token handling), [`../../05-refined-shared-systems-architecture/storage.md`](../../05-refined-shared-systems-architecture/storage.md) §4 (key handling); the audit Finding 10 (Debug-redaction present on `RawKey`/`Dek`; the sweep needs to cover token/credential/secret-broker types too).
- **DELIVERABLE.** Across `myelin-identity-service`, `myelin-storage`, `myelin-ci` (the secret broker), and any crate carrying a bearer credential/token/key/password: (a) ensure every such type has a redacting `Debug` (no raw bytes/strings), backed by `secrecy::Secret`/`zeroize::Zeroizing` wrappers so the material zeroizes on drop; (b) AUDIT + remove any unnecessary `Serialize`/`Deserialize` on bearer-credential types (a token must not be trivially serializable into a log/trace/JSON sink — where serialization is genuinely needed for transport, it goes through an explicit guarded path, not a derive); (c) add a committed lint or scanner row that FAILS the build if a type tagged secret-bearing derives `Debug`/`Serialize` non-redacting. **Floor named:** none open.
- **CONTRACTS TO IMPLEMENT.** Cross-cutting hardening; no new contract row.
- **GATE / DRILLS (quantified).** A logging/tracing leak test: drive auth + token-mint + key-wrap + secret-broker paths with a TRACE-level subscriber capturing all output, assert 0 occurrences of any raw credential/key/token material. The secret-bearing-derive lint is green and red on a fixture that derives a non-redacting `Debug` on a tagged type. Dated green artifact.
- **TESTS (required).** The no-leak tracing test across all secret paths; the lint red/green fixtures; a zeroize-on-drop test per secret type. Mutation floor: the redaction `Debug` impls + the lint predicate are mandatory-core.
- **DEFINITION OF DONE.** Every bearer credential/key/token type has a redacting `Debug` + secrecy/zeroize backing; unnecessary `Serialize` on bearer types is removed; the secret-bearing-derive lint is committed + bites; the no-leak tracing test is green at 0; committed.
- **COMMIT.** Header `P-532 M7: secret-handling sweep (redacted Debug, secrecy/zeroize, no Serialize on bearer credentials)`. Body: the redaction+zeroize sweep; the removed Serialize derives; the secret-bearing-derive lint; the no-leak measured result. Co-Authored-By trailer.

---

### P-533 — Verify secret handling: no credential/key reaches any log, trace, or error sink

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the secret-handling VERIFICATION; separate from P-532) — Finding 10.
- **DEPENDS-ON.** P-532.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it), §5 (committed ratchet).
  - The audit Finding 10; the M7 secret types from P-532.
- **DELIVERABLE.** A committed CI verification job: a corpus drill that exercises EVERY known secret-carrying path (authenticate all credential kinds with a sentinel secret value, mint every token type, wrap/unwrap a sentinel key, pass a secret through the CI secret broker, trigger an ERROR on each path) with a capturing subscriber over logs + traces + the error-display path, and asserts the sentinel value appears 0 times in any sink — including `Display`/`Error`/panic messages, not only `Debug`. This PROVES P-532's redaction is exhaustive, not partial.
- **CONTRACTS TO IMPLEMENT.** None new — verifies the P-532 sweep.
- **GATE / DRILLS (quantified).** 0 sentinel occurrences across logs + traces + Display/Error/panic output over every secret path. Red on a deliberately-unredacted fixture path (proves it bites). Dated green artifact.
- **TESTS (required).** The sentinel-leak corpus drill; the bite fixture. Mutation floor: the sink-capture + sentinel-match are mandatory-core.
- **DEFINITION OF DONE.** The sentinel-leak corpus drill proves 0 credential/key leaks across all sinks including error/panic paths; the drill is committed + bites; committed.
- **COMMIT.** Header `P-533 M7: verify secret handling (no credential/key in any log/trace/error sink)`. Body: the corpus drill + bite fixture; the 0-leak measured result. Co-Authored-By trailer.

---

### P-534 — SHA-pin GitHub Actions + digest-pin container images + pin the Rust toolchain

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (supply-chain pinning) — Finding 11.
- **DEPENDS-ON.** P-507 (the dogfood CI pipeline these changes harden).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §5 (an uncommitted/unpinned gate is no gate).
  - [`../../VISION.md`](../../VISION.md) §4 (the quality bar).
  - The files to fix: `.github/workflows/*.yml` (actions are tag-pinned `@v4`/`@stable`/`@main`), `docker-compose.dev.yml` (images are tag/`latest`-pinned), repo root (no `rust-toolchain.toml`); `Cargo.lock` (already committed — good).
- **DELIVERABLE.** (a) Replace every `uses:` in `.github/workflows/` with a full commit-SHA pin (`actions/checkout@<sha> # v4.x`), no `@main`/`@vN` floating refs. (b) Replace every container `image:` (in compose files + any CI service image + any release image) with a `name@sha256:<digest>` digest pin. (c) Add a repo-root `rust-toolchain.toml` pinning the exact toolchain channel + version + components, and reference it from CI (drop `@stable`). (d) Keep `Cargo.lock` committed + add `--locked` to CI cargo invocations. **Floor named:** a renovate/dependabot-style digest-bump automation is a post-M7 convenience, named, not a release blocker.
- **CONTRACTS TO IMPLEMENT.** Governance — no contract row.
- **GATE / DRILLS (quantified).** A committed lint/script that scans `.github/` + compose files + the toolchain file and FAILS on any floating ref (a `@vN`/`@main`/`:latest`/`:<tag>`-without-digest), 0 floating refs. Red on a fixture with a floating ref. Dated green artifact.
- **TESTS (required).** The pin-scanner red/green fixtures. A CI run proving `--locked` builds reproduce.
- **DEFINITION OF DONE.** All actions SHA-pinned, all images digest-pinned, the toolchain pinned, CI uses `--locked`; the pin-scanner is committed + bites at 0 floating refs; committed.
- **COMMIT.** Header `P-534 M7: SHA-pin actions + digest-pin images + pin Rust toolchain`. Body: the pinning sweep; the pin-scanner. Co-Authored-By trailer.

---

### P-535 — cargo-deny: advisory (RUSTSEC) + license + source policy as a committed gate

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (advisory + license policy) — Finding 11.
- **DEPENDS-ON.** P-534.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §5 (committed gates).
  - The repo root (no `deny.toml`; no cargo-audit/cargo-deny in CI).
- **DELIVERABLE.** Add a repo-root `deny.toml` configuring `cargo-deny` for: (a) advisories (RUSTSEC) — a known-vulnerable dependency fails the build; (b) licenses — an allowlist of permitted SPDX licenses, a copyleft/unknown license fails; (c) bans/sources — only the crates.io registry (or an explicit mirror) is permitted, duplicate/yanked crates flagged. Wire `cargo deny check` into CI loud-never-swallowed (no `|| true`). **Floor named:** the license allowlist's exact set is a one-time human/legal sign-off (named, recorded); the mechanism is the deliverable.
- **CONTRACTS TO IMPLEMENT.** Governance — no contract row.
- **GATE / DRILLS (quantified).** `cargo deny check` is green on the current tree AND red on a fixture that adds a crate with a known advisory or a disallowed license (proves it bites). 0 open advisories at release. Dated green artifact.
- **TESTS (required).** The cargo-deny CI step; a red-fixture branch proving an advisory/disallowed-license fails.
- **DEFINITION OF DONE.** `deny.toml` + the `cargo deny check` CI gate exist, are loud, and bite on advisory/license/source violations; 0 open advisories; the license-allowlist legal sign-off is named; committed.
- **COMMIT.** Header `P-535 M7: cargo-deny advisory + license + source policy gate`. Body: `deny.toml`; the advisory/license/source checks; the license-allowlist human sign-off. Co-Authored-By trailer.

---

### P-536 — SBOM generation + build provenance (SLSA-style attestation) on release artifacts

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (SBOM + provenance) — Finding 11.
- **DEPENDS-ON.** P-534, P-535.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §5; [`../../05-refined-shared-systems-architecture/continuous-integration.md`](../../05-refined-shared-systems-architecture/continuous-integration.md) (the supply-chain trust posture, digest-pin-or-fail-closed, sigstore sign/verify, SLSA/SBOM — the same discipline P-366/CI-P23 applies INSIDE the platform; M7 applies it to the platform's OWN release).
  - The `.github/` release path.
- **DELIVERABLE.** In the release workflow: (a) generate a CycloneDX/SPDX SBOM for every released artifact (`cargo sbom`/`syft`); (b) produce a signed build-provenance attestation (SLSA provenance / `cosign attest` / GitHub artifact attestation) binding the artifact digest to the build inputs + the pinned toolchain; (c) sign release artifacts (sigstore/cosign) and publish the SBOM + provenance + signature alongside. **Floor named:** a full SLSA-Level-3 hermetic builder is a post-M7 increment (named); the deliverable is SBOM + signed provenance + signature.
- **CONTRACTS TO IMPLEMENT.** Governance — no contract row.
- **GATE / DRILLS (quantified).** Every release artifact has a published SBOM + a verifiable signed provenance attestation + a verifiable signature; `cosign verify`/the attestation-verify step is green (0 unsigned/unattested artifacts). Dated green artifact.
- **TESTS (required).** The SBOM-generation + attestation + verify steps in CI; a verify step proving a tampered artifact fails attestation.
- **DEFINITION OF DONE.** Released artifacts carry SBOM + signed provenance + signature, all verifiable in CI; a tampered artifact fails verification; committed.
- **COMMIT.** Header `P-536 M7: SBOM + signed build provenance on release artifacts`. Body: the SBOM + provenance + signing; the verify step. Co-Authored-By trailer.

---

### P-537 — Reproducible release builds (the artifact reproduces bit-for-bit from pinned inputs)

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (reproducible release artifacts) — Finding 11.
- **DEPENDS-ON.** P-534, P-536.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it), §5.
  - The pinned toolchain (P-534) + provenance (P-536).
- **DELIVERABLE.** Make the release build reproducible: set deterministic build flags (`SOURCE_DATE_EPOCH`, fixed paths via `--remap-path-prefix`, deterministic linking), pin all inputs (toolchain P-534, `--locked` deps), and add a CI job that builds the release artifact TWICE in independent environments and asserts the output digests are identical. **Floor named:** any non-determinism source that cannot be removed (e.g. an upstream crate's build script) is documented + accepted with a recorded rationale, never hidden.
- **CONTRACTS TO IMPLEMENT.** Governance — no contract row.
- **GATE / DRILLS (quantified).** Two independent builds of the release artifact produce bit-identical digests (0 byte differences); red if they diverge. Dated green artifact; the digest is what P-536's provenance attests.
- **TESTS (required).** The double-build reproducibility CI job; a documented-exceptions list if any.
- **DEFINITION OF DONE.** The release artifact reproduces bit-for-bit from pinned inputs across two independent builds; any irreducible non-determinism is documented; committed.
- **COMMIT.** Header `P-537 M7: reproducible release builds (bit-identical from pinned inputs)`. Body: the determinism flags; the double-build measured result; any documented exceptions. Co-Authored-By trailer.

---

### P-538 — SECURITY.md, vulnerability-response policy, and CODEOWNERS review policy

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (security governance docs + ownership/review) — Finding 11.
- **DEPENDS-ON.** P-507.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (name owners + process), §5.
  - The repo root (no SECURITY.md, no CODEOWNERS).
- **DELIVERABLE.** (a) A repo-root `SECURITY.md` — the vulnerability-disclosure channel, the response SLA, the supported-versions policy, the coordinated-disclosure process. (b) A `CODEOWNERS` file assigning required reviewers per critical area (identity/auth, KMS/crypto, sandbox, storage/backup, CI supply-chain) so a change to a security-critical module requires its owner's review. (c) A documented vuln-response runbook (triage → fix → advisory → release). **Floor named:** the named human owners are placeholders until staffed — recorded as a human prerequisite, not a code blocker.
- **CONTRACTS TO IMPLEMENT.** Governance — no contract row.
- **GATE / DRILLS (quantified).** `SECURITY.md` + `CODEOWNERS` + the runbook exist; a CI check asserts every security-critical path listed in the audit has a CODEOWNERS owner (0 unowned critical paths). Dated green artifact.
- **TESTS (required).** The CODEOWNERS-coverage check over the critical-path list; a red fixture (an unowned critical path).
- **DEFINITION OF DONE.** SECURITY.md + CODEOWNERS + the vuln-response runbook exist; every security-critical path is owned; the unstaffed-owner placeholders are recorded as human prerequisites; committed.
- **COMMIT.** Header `P-538 M7: SECURITY.md + vuln-response policy + CODEOWNERS`. Body: the security docs; the CODEOWNERS coverage; the unstaffed-owner human prerequisite. Co-Authored-By trailer.

---

### P-539 — Production service-runtime finalization: real OS-signal lifecycle + OpenTelemetry export + trace propagation

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the serve(AppSpec) production-runtime floors: the OS-signal drain trigger + the real OTel export named in `serve.rs` as "P-S13/P-S14") — Finding 1.
- **DEPENDS-ON.** P-522 (the live pool the production runtime drains).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (the `serve.rs` floor notes are the divergence to close), §3 (observability is part of the pass).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) + the substrate arch §3.1 (graceful drain), §3.5 (trace context), §3.2 (env-validated config).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 1.1, 1.2, 1.3, 1.8.
  - The code: `crates/myelin-substrate/src/serve.rs` (the in-process `signal_drain()` trigger; "the OS-signal drain trigger is P-S13/P-S14"; "the real OpenTelemetry export … lands with the metrics-health surface").
- **DELIVERABLE.** In crate `myelin-substrate`: replace the deterministic in-process drain trigger with a real OS-signal lifecycle — `SIGTERM`/`SIGINT` → begin graceful drain (stop intake, finish in-flight, flush the outbox relay to depth 0, ack-then-exit) with a bounded drain deadline → forced-exit on timeout; and finalize the real OpenTelemetry tracer/meter/logger export + the causality+tenant trace-context propagation middleware (the producer side of the contract-1.8 signal set the harness reads) so the survival signals export to a real collector, not only an in-test reader. Keep env-first validated config + the bounded prod pool (already real). **Floor named:** none open — this closes the P-S13/P-S14-named runtime floors.
- **CONTRACTS TO IMPLEMENT.** Owns the real backing of 1.1 (lifecycle: OS-signal drain) + 1.8 (real telemetry export) + 1.2/1.3 (the surfaces already real). Consumed by every service.
- **GATE / DRILLS (quantified).** (1) A real `SIGTERM` to a running service triggers graceful drain: 0 events lost, `outbox_depth==0` at exit, in-flight requests complete or shed cleanly within the deadline, forced-exit fires if the deadline passes. (2) Trace propagation: a request's tenant + causality trace context appears end-to-end in the exported spans (assert the collector receives them). Under `--features integration`. Dated green artifact.
- **TESTS (required).** The SIGTERM-drain integration test (0 loss, depth-0 exit, deadline); the trace-propagation test against a collector. CDC: re-affirm 1.1 + 1.8 with the real signal lifecycle + export. Mutation floor: the drain-completeness (depth-0 before exit) is mandatory-core (a mutation that exits before drain MUST be caught).
- **DEFINITION OF DONE.** Real `SIGTERM`/`SIGINT` graceful drain + bounded deadline + real OTel export + trace propagation are live; the SIGTERM-drain + trace-propagation drills emit dated green artifacts; lints + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-539 M7: production runtime finalization (OS-signal drain + OpenTelemetry export + trace propagation)`. Body: 1.1 (OS-signal lifecycle) + 1.8 (real export); the SIGTERM-drain + trace measured results; the mutation score. Co-Authored-By trailer.

---

### P-540 — Gate-integrity hardening: required KVM/sandbox jobs FAIL-not-skip; semantic CDC; mandatory mutation jobs

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (gate & evidence integrity — mechanical enforcement) — Finding 5.
- **DEPENDS-ON.** P-534 (pinned CI), P-535 (cargo-deny gate).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (a target you cannot measure is not a gate; never weaken to pass), §5 (an uncommitted gate is no gate).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/README.md`](../../05-refined-shared-systems-architecture/testing-strategy/README.md) §5 (the must-be-early gates); the audit Finding 5 (the existing CI already hard-fails KVM via `MYELIN_REQUIRE_KVM=1` and the contract-coverage scanner is already semantic — this prompt makes the remaining gate-integrity items mechanical and immutable).
  - The `.github/workflows/`, the `contract-coverage.toml` scanner, the `testing/scorecards/`.
- **DELIVERABLE.** (a) Confirm + lock that every required KVM/sandbox-escape job (AG-D4/CI-T1) and every store-touching restore-verify job (STOR-D1/STOR-D2) is set to HARD-FAIL when the backend is unavailable (`MYELIN_REQUIRE_KVM=1` / `MYELIN_REQUIRE_DB=1`), with a CI guard that FAILS if any of these jobs is configured `continue-on-error` or skippable (the gate-cannot-be-made-optional check). (b) Make every mandatory-core module's `cargo-mutants` mutation job a REQUIRED CI job (not advisory) with its committed score floor. (c) Confirm the contract-coverage scanner is semantic (it already verifies file-existence + provider+consumer markers, not marker-word existence) and add a self-test row. (d) Make CI attestations immutable + provenance-bearing: scorecard green artifacts are signed/attested (tie into P-536) so a green row cannot be hand-edited without detection. **Floor named:** none open.
- **CONTRACTS TO IMPLEMENT.** Governance — tightens the meta-gates (1.6 lints + the coverage scanner).
- **GATE / DRILLS (quantified).** The gate-cannot-be-optional guard is green AND red on a fixture that adds `continue-on-error` to a required gate (proves it bites). A KVM-absent run HARD-FAILS the AG-D4 job (0 skips). A mutation job below its floor FAILS the build. A hand-edited scorecard row fails the attestation check. Dated green artifact.
- **TESTS (required).** The optionality-guard red/green fixtures; the mutation-required-job check; the attestation-tamper check.
- **DEFINITION OF DONE.** Required KVM + restore-verify jobs are mechanically unable to be made optional and hard-fail without the backend; mandatory mutation jobs are required; the coverage scanner is semantic with a self-test; scorecard artifacts are attested/immutable; committed.
- **COMMIT.** Header `P-540 M7: gate-integrity hardening (required jobs fail-not-skip; mandatory mutation; immutable attestations)`. Body: the optionality-guard; the required-mutation-jobs; the attestation immutability. Co-Authored-By trailer.

---

### P-541 — The truth-up pass: re-run every claimed-green scorecard against the M7 production graph

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (truth-up of all previously claimed-green scorecards now that the floors are filled) — Finding 5.
- **DEPENDS-ON.** P-523, P-525, P-528, P-530, P-531, P-533, P-540.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (code-wins-over-docs), §3 (PROVEN not CLAIMED), §5.
  - [`../../06-roadmaps/00-master-sequencing.md`](../../06-roadmaps/00-master-sequencing.md) §2 M6-done (the truth-up pass confirms 0 red earlier gates); the `testing/scorecards/` set (sub-m0, id-m1, m2-reactive, m3-producers, m4-consumers, infra).
- **DELIVERABLE.** A truth-up verification pass: re-run every band scorecard's drill set against the M7 production graph (real stores, real crypto, real KMS, real restore) — NOT against the M1..M6 floors. Where a drill was previously green only because it ran against a structural floor (e.g. an auth drill that passed against `StructuralVerifier`), confirm it stays green against the REAL mechanism; where it does not, record a dated "claimed-not-proven" row and BLOCK — never edit green. Produce a single M7 truth-up scorecard (`testing/scorecards/m7-production-readiness.md`) cross-referencing each band gate to its now-real proof. This is a VERIFICATION prompt — it asserts, it does not implement.
- **CONTRACTS TO IMPLEMENT.** None new — verifies the whole stack.
- **GATE / DRILLS (quantified).** Every band scorecard drill is re-confirmed green against the production graph (0 drills passing only on a floor); the M7 truth-up scorecard has 0 red rows; any residual is a dated honest row + a blocker. Dated green artifact.
- **TESTS (required).** The truth-up re-run harness over the existing scorecard drill set, pointed at the production-feature build.
- **DEFINITION OF DONE.** Every previously-green band gate is re-confirmed against the real production mechanisms; the M7 truth-up scorecard exists with 0 red rows (or honest blockers); committed.
- **COMMIT.** Header `P-541 M7: truth-up pass (re-confirm every band gate against the production graph)`. Body: the per-band re-confirmation; the M7 truth-up scorecard. Co-Authored-By trailer.

---

### P-542 — Independent cryptography + sandbox security reviews (external human blockers, recorded)

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (independent crypto + sandbox review — external/human prerequisites) — Findings 2/4/7.
- **DEPENDS-ON.** P-524, P-527, P-529 (the real crypto/KMS/restore must exist to be reviewed), P-348/P-370 (the sandbox-escape gate is already green; this is the independent review of it).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (name your external blockers; never silently swallow), §3.
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5 (one escape is catastrophic — an independent review of the sandbox boundary).
  - The audit Findings 2 (auth crypto), 4 (sandbox), 7 (KMS).
- **DELIVERABLE.** Commission + record the results of independent external reviews: (a) a cryptography review of the auth/token/KMS implementations (P-524/526/527) by an independent reviewer; (b) a sandbox/escape review of the Firecracker + gVisor backends (the AG-D4 boundary) on the committed production runners; (c) a production-path escape-drill execution on committed KVM/gVisor runners (re-run AG-D4/CI-T1 on the exact prod image). The DELIVERABLE that lands in this repo is the RECORD: a `testing/scorecards/m7-external-reviews.md` capturing each review's status (commissioned / in-progress / passed / findings-open), the reviewer, the date, and every finding with its severity + remediation status. **Floor named (explicit human/external blockers):** the third-party penetration test, the independent crypto audit, and the independent sandbox audit are HUMAN/EXTERNAL prerequisites — this prompt RECORDS them and tracks their findings; it cannot itself make them pass. They are hard blockers on P-546.
- **CONTRACTS TO IMPLEMENT.** None — records external review status.
- **GATE / DRILLS (quantified).** AG-D4/CI-T1 re-run on the committed prod KVM + gVisor runners → 0 escapes (mechanical, in-repo). The external-review record exists with each review's status + 0 unrecorded critical/high findings. (The external reviews PASSING is a P-546 blocker, recorded here, not asserted here.) Dated green artifact for the in-repo escape re-run; honest status rows for the external reviews.
- **TESTS (required).** The AG-D4/CI-T1 prod-image re-run (mechanical). The review-record completeness check.
- **DEFINITION OF DONE.** The production-path escape drill re-runs green on the committed runners; the external crypto + sandbox + pentest reviews are commissioned and their status + findings are recorded in `m7-external-reviews.md`; the external-review-pass requirement is named as a P-546 blocker; committed.
- **COMMIT.** Header `P-542 M7: independent crypto + sandbox reviews + production-path escape drill (external blockers recorded)`. Body: the AG-D4/CI-T1 prod re-run result; the external-review record + named human blockers. Co-Authored-By trailer.

---

### P-543 — Penetration test execution + findings register (external human blocker, recorded)

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (third-party penetration test — external/human prerequisite) — Finding 5/11.
- **DEPENDS-ON.** P-541 (truth-up green — the platform is internally consistent before pentest), P-542 (the crypto/sandbox reviews).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (name + track external blockers), §3.
  - The audit (all findings); the security-governance docs (P-538).
- **DELIVERABLE.** Commission a third-party penetration test against a production-representative deployment (real stores, real crypto, real KMS, tenant isolation, sandbox). Land in this repo the findings register: `testing/scorecards/m7-pentest.md` — every finding with CVSS/severity, affected component, status (open / remediated / accepted-with-rationale), and the remediation prompt/commit. Critical/high findings MUST be remediated (a remediation may itself spawn an appended M7 prompt with the next free ordinal) before P-546 can go green. **Floor named (explicit human/external blocker):** the pentest itself is a human/external engagement — this prompt RECORDS its execution + findings + remediation; the "0 critical/high open" condition is a hard P-546 blocker.
- **CONTRACTS TO IMPLEMENT.** None — records pentest status + drives remediation.
- **GATE / DRILLS (quantified).** The pentest is executed; the findings register exists; every critical/high finding is remediated (status = remediated, with a linked commit) or explicitly accepted with a recorded rationale + sign-off; 0 critical/high findings left silently open. Honest status rows; the "0 open critical/high" is the P-546 condition.
- **TESTS (required).** Per-finding remediation tests (a regression test for each remediated finding); the register-completeness check.
- **DEFINITION OF DONE.** The pentest is executed and recorded; all critical/high findings are remediated or accepted-with-sign-off; the findings register is complete; the pentest-pass condition is named as the P-546 blocker; committed.
- **COMMIT.** Header `P-543 M7: penetration test execution + findings register (external blocker recorded)`. Body: the pentest status; the findings register + remediation links + any accepted-with-rationale items. Co-Authored-By trailer.

---

### P-544 — Firecracker production JobSpec.command execution: the real microVM job runner

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (fills the sandbox job-exec floor — neither committed backend runs `JobSpec.command` through its production `launch()`; Firecracker hardcodes `oneshot=true` → `init=/bin/true`, gVisor probes only `runsc --version`) — [`../production-readiness-audit.md`](../production-readiness-audit.md) Finding 4.
- **DEPENDS-ON.** P-522 (durable reserve/settle ledger backing real metering), P-524 (the KMS the run token / per-job secrets resolve against). (M7 is post-M6; the band sort places this after the truth-up + external-review prompts.)
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3/§4 (untrusted code runs only inside the sandbox; the quality bar).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (code-wins-over-docs: the `init=/bin/true` / `runsc --version`-only launch is the divergence to close), §3 (prove-it).
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5 (untrusted-code execution; one escape is catastrophic — the boundary must contain a REAL running command, not a no-op boot).
  - [`../../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`](../../04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md) §5.1 (microVM-as-default backend decision), §5.3 (the hardening profile the job runs inside).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 8.4 (the unified sandbox), 8.2 (`accept_only_compute` routing), 1.6 (`no-host-exec` — the launch IS the seam), 11.7 (reserve/settle).
  - The code to fix: `crates/myelin-ci-sandbox/src/firecracker.rs` (`launch()` :327-328 hardcodes `oneshot=true`; `from_spec` :108-117 sets `init=/bin/true`; `grep spec.command` is empty), `crates/myelin-ci-sandbox/src/gvisor.rs` (`OciConfig::from_spec` :67 carries `args: spec.command.clone()` but `spawn_real_runsc` :227-237 only runs `runsc --version` and returns a no-op `SpawnedRunsc`; the OCI-bundle run is a documented CI-P28 follow-on :225-226).
- **DELIVERABLE.** In crate `myelin-ci-sandbox`: implement the REAL job runner behind the unchanged `SandboxBackend::launch` seam so a production launch EXECUTES `spec.command` inside the hardened guest. For **Firecracker** (the default backend): inject `spec.command` + its env + the workspace into the guest and run it — **pick ONE mechanism and justify it in the commit body**; the recommended choice is a **vsock guest-agent** (a tiny init that reads the JobSpec command/env/workspace over a vsock control channel, execs it as the guest entrypoint, streams stdout/stderr back, and reports the exit code), because it keeps the rootfs read-only (no per-job rootfs mutation) and gives a clean stdout/stderr/exit-code channel; an acceptable alternative is a **read-only command drive** (a second `is_read_only=true` drive carrying the command+env that a fixed in-guest init reads and execs — analogous to the drill's `/dev/vdb`, but promoted to the production path and locked to the hardening profile). The production `launch()` MUST stop passing `oneshot=true`/`init=/bin/true` for real jobs (boot args no longer carry `init=/bin/true`; the guest entrypoint is the chosen runner) — **the boot self-test keeps its existing `oneshot=true` path** via `launch_with(.., oneshot=true, ..)`, so the self-test is unaffected. For **gVisor**: make `spawn_real_runsc` actually spawn `runsc run --bundle <dir>` against a written OCI bundle whose `config.json` is the already-built `OciConfig` (which carries `args: spec.command`), instead of probing `runsc --version`; capture the container's exit code + stdout/stderr; whole-container-kill on teardown (the existing `kill()` path). For BOTH backends: enforce the resource limits (`spec.limits` cpu/mem/pids) and a wall-clock **timeout** that whole-guest-kills (the VMM / the runsc container) on expiry; capture exit code + stdout + stderr into the `SandboxHandle` / a result struct; and **meter/account (`hooks.settle`) ONLY after the command actually completes** (today the settle bookend fires after a no-op boot/probe — move it to post-command-completion with the REAL `ResourceUsage`). The `no-host-exec` named exclusion is unchanged (these are still the ONE legitimate VMM-/runsc-spawn sites). **Floor named:** real pre-warmed snapshot pools (CI-P4) and the fleet impl (CI-P14) remain their own prompts (no new floor); per-language guest images are config. No floor is left open by this prompt — the production exec path is real for both committed backends.
- **CONTRACTS TO IMPLEMENT.** Owns the real job-exec backing of 8.4 (the unified sandbox — the Firecracker + gVisor halves actually run the command). Consumed: 8.2 (`accept_only_compute` routing unchanged), 1.6 (`no-host-exec` named exclusion unchanged), 11.7 (reserve/settle now bookends a real command), 4.7 (the per-run token the guest carries).
- **GATE / DRILLS (quantified; must be green to call this done).** Run under `--features integration` with `MYELIN_REQUIRE_KVM=1` (a KVM-free / runsc-free run HARD-FAILS, not skips). (1) **Real command runs:** a `JobSpec` whose `command` exits 0 with known stdout/stderr → the production `launch()` returns that exit code (0) and the captured stdout/stderr matches byte-for-byte; a command that exits non-zero → that non-zero code is captured (NOT masked to 0). Proven on BOTH Firecracker and gVisor. (2) **Timeout kill:** a `JobSpec` whose command sleeps past the timeout → the whole guest/container is killed within the bounded deadline, the result is a timeout (not a hang, not a false success), 0 orphaned VMM/runsc processes after `kill()`. (3) **Accounting after completion:** `hooks.settle` fires exactly once, AFTER the command terminates, with the real `ResourceUsage` (not the placeholder `{cpu_seconds:1, mem_byte_seconds:1}`); a command that never completes never settles a success. (4) **`init=/bin/true` absent from the real-job path:** a grep/test asserts the production `launch()` does NOT boot `oneshot=true`/`init=/bin/true` for a real job (the self-test's oneshot path is the only `init=/bin/true` caller). Telemetry: per-job exit-code + duration on the metrics surface. Dated green artifact on the M7 scorecard.
- **TESTS (required).** Unit: the runner builds the correct guest entrypoint / OCI args from a `JobSpec` (the command/env/workspace are present, the hardening profile is asserted, `init=/bin/true` is NOT in a real-job boot). Integration (the gate above): real-command exit-code + stdout/stderr capture on both backends; timeout-kill; settle-after-completion. CDC: re-affirm 8.4 provider+consumer now exercises a real command execution, not a boot self-test. Mutation floor (mandatory core — the sandbox is a Tier-2/escape keystone): the timeout-kill + the exit-code-capture + the settle-only-after-completion branches; a mutation that swallows a non-zero exit, skips the timeout kill, or settles before completion MUST be caught.
- **DEFINITION OF DONE.** Both committed backends EXECUTE `spec.command` through the production `launch()` path (Firecracker via the chosen vsock-agent/command-drive mechanism with no `init=/bin/true` on the real-job boot; gVisor via real `runsc run --bundle`); resource limits + timeout-kill are enforced; exit code + stdout/stderr are captured; metering fires only after actual completion; the boot self-test's `oneshot` path is preserved; the four drills emit dated green artifacts under `MYELIN_REQUIRE_KVM=1`; lints (incl. `no-host-exec` unweakened) + CDC + mutation floor green; committed.
- **COMMIT.** Header `P-544 M7: Firecracker + gVisor production JobSpec.command execution (real microVM/runsc job runner)`. Body: the chosen Firecracker injection mechanism + its justification; the gVisor `runsc run --bundle` switch; the exit-code/stdout/stderr capture; the timeout-whole-guest-kill; settle-only-after-completion; the no-`init=/bin/true`-on-real-jobs proof; the mutation score. Co-Authored-By trailer.

---

### P-545 — Verify production-path sandbox exec + re-run the AG-D4 escape corpus through the PRODUCTION exec path on both backends

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the sandbox production-exec VERIFICATION; separate from P-544 per EI-01 §3 — a drill/harness is NOT proof of the production exec path) — Finding 4.
- **DEPENDS-ON.** P-544 (the production exec path must exist to be proven), P-529 (real backup/restore is not required for this drill but the durable/runtime prerequisites it leans on must be green), P-542 (the existing AG-D4/CI-T1 prod-image re-run + the external sandbox review the corpus reuses).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it on a real kernel — a property not drilled on the production path is a claim), §5 (the committed ratchet).
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §5 (one escape is catastrophic; the corpus must run through the path real jobs use).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) (AG-D4 / CI-T1 — the real-kernel escape drill family).
  - The audit Finding 4; the now-real `firecracker.rs`/`gvisor.rs` production exec path (P-544); the existing AG-D4 corpus (`crates/myelin-ci-sandbox` escape-drill module + the seven adversarial families).
- **DELIVERABLE.** A committed CI verification job (PROVE the P-544 mechanism; do not re-implement it): (a) an **end-to-end production-exec proof** — a real `JobSpec.command` runs end-to-end on BOTH committed backends (Firecracker microVM + gVisor runsc) through the PRODUCTION `launch()` path (not a harness), with the exit code + stdout/stderr captured and the reserve/settle bookend firing post-completion; (b) **re-run the AG-D4 adversarial corpus THROUGH that production exec path** — the seven adversarial families are delivered as the `JobSpec.command` (or its workspace) and executed by the SAME production runner a real job uses (NOT the special drill harness — Firecracker's old `init=/bin/bash /dev/vdb` command-drive boot, gVisor's drill-only bundle), and assert **ZERO escapes** on a real kernel for both backends; (c) a guard/test asserting the corpus ran via the production path (e.g. the production `launch()` entrypoint executed it, `init=/bin/true` was absent, the run carried the real hardening profile) so a future regression to a harness-only drill is caught. Run under `--features integration` with `MYELIN_REQUIRE_KVM=1` (a KVM-free / runsc-free run HARD-FAILS, not skips).
- **CONTRACTS TO IMPLEMENT.** None new — verifies 8.4 (the production exec path implemented in P-544) + re-arms the AG-D4 / CI-T1 permanent gate over the production exec path.
- **GATE / DRILLS (quantified).** (1) A real `JobSpec.command` executes end-to-end through the production `launch()` on BOTH backends; exit code + stdout/stderr captured; 0 harness shortcuts. (2) The AG-D4 corpus (all seven families) re-run through the production exec path → **0 escapes** on a real kernel, on BOTH Firecracker and gVisor. (3) The production-path guard is green AND red on a fixture that routes the corpus through a harness-only path (proves it bites). All under `MYELIN_REQUIRE_KVM=1`. Dated green artifact on the M7 scorecard; this re-arms the AG-D4 / CI-T1 PERMANENT gate over the production exec path (replacing the harness-only attestation).
- **TESTS (required).** The end-to-end production-exec test on both backends; the AG-D4-corpus-through-prod-path escape drill (0 escapes, both backends); the harness-shortcut guard's red/green fixtures. Mutation floor: the "ran-via-production-path" predicate + the escape-detection assertion are mandatory-core (a mutation that lets a harness run masquerade as a prod-path run, or that misses an escape, MUST be caught).
- **DEFINITION OF DONE.** A real `JobSpec.command` is proven to run end-to-end through the production `launch()` on both committed backends; the AG-D4 corpus re-run through that production exec path shows 0 escapes on a real kernel for both; the harness-shortcut guard is committed + proven to bite; the AG-D4 / CI-T1 permanent gate is re-armed over the production exec path; committed.
- **COMMIT.** Header `P-545 M7: verify production-path sandbox exec + AG-D4 escape corpus through the production path (both backends, 0 escapes)`. Body: the end-to-end prod-exec proof on both backends; the AG-D4-through-prod-path 0-escapes measured result; the harness-shortcut guard + its bite fixture; the re-armed permanent gate. Co-Authored-By trailer.

---

### P-546 — THE PRODUCTION-RELEASE GATE (fail-closed; cannot go green over any open floor, mock, scan, or human blocker)

- **BAND.** M7.
- **ROADMAP MILESTONE.** PR-M7 (the final fail-closed production-release gate) — Findings 1–11 (the aggregate).
- **DEPENDS-ON.** P-522, P-523, P-524, P-525, P-526, P-527, P-528, P-529, P-530, P-531, P-532, P-533, P-534, P-535, P-536, P-537, P-538, P-539, P-540, P-541, P-542, P-543, P-544, P-545.
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3/§4/§8 (the done-bar; one agent, proven gates).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (name-your-floors; nothing silently swallowed), §2 (order-by-non-negotiability), §3 (PROVEN not CLAIMED; never weaken to pass), §5 (the committed ratchet).
  - [`../../06-roadmaps/00-master-sequencing.md`](../../06-roadmaps/00-master-sequencing.md) §2 (the M7 band invariant + this gate); the M7 truth-up + external-review + pentest scorecards.
- **DELIVERABLE.** A single committed, FAIL-CLOSED production-release gate (a CI job + a `release-gate` binary + a `testing/scorecards/m7-release-gate.md` scorecard) that goes green if and ONLY IF every condition below holds — it defaults to RED and each condition must flip it, never the reverse:
  1. **No structural/mock impl in the production dependency graph** — the absence scanners (P-523 in-memory stores, P-528 structural verifiers/signers/attestation) are green; 0 `Structural*`/in-memory-durable constructors in any production path; the mock agent runtime is `--use-mock`-gated only.
  2. **Durable persistence proven** — P-522/P-523 green (crash/restart + multi-instance, real OLTP/cache).
  3. **Real crypto proven** — P-526/P-527/P-528 green (OIDC/SAML/WebAuthn/SSH + signed tokens + DPoP + attestation; expired grants cannot authorize).
  4. **Real KMS proven** — P-524/P-525 green (HSM-class adapter, root never in process, destruction permanent, zeroization, no resurrection).
  5. **Real backup/restore proven DESTRUCTIVELY** — P-529/P-530 green; a destructive clean-target restore has been PERFORMED and MEASURED RPO ≤ 5 min / RTO ≤ 1h-tenant / 4h-cell over real data.
  6. **Tenant isolation hardened** — P-531 green (SET LOCAL RLS, reset-on-release, mTLS, region fail-fast).
  7. **Secret handling proven** — P-532/P-533 green (0 credential/key in any sink).
  8. **Production runtime finalized** — P-539 green (OS-signal drain, OTel export, trace propagation).
  9. **Sandbox runs the real command** — P-544/P-545 green: the Firecracker AND gVisor production exec path runs `spec.command` with NO `oneshot`/`init=/bin/true`-only launch in the production graph (a real job's exit code + stdout/stderr are captured, a timeout whole-guest-kills, metering fires only post-completion), AND the production-path escape drill (the AG-D4 corpus re-run through the production `launch()`, not the special harness) is green with **0 escapes** on a real kernel on BOTH committed backends.
  10. **Supply-chain green** — P-534..P-537 green (actions/images/toolchain pinned, `cargo deny check` passes with 0 open advisories + license policy met, SBOM + signed provenance, reproducible build); current advisory + license scans pass AT RELEASE TIME (re-run, not cached).
  11. **Security governance present** — P-538 green (SECURITY.md, CODEOWNERS, vuln-response).
  12. **Gates mechanically enforced** — P-540 green (required jobs fail-not-skip; mandatory mutation; immutable attestations); P-541 truth-up has 0 red rows.
  13. **Independent reviews + pentest** — P-542/P-543: the independent crypto + sandbox reviews and the third-party pentest are COMPLETED with 0 critical/high findings open, OR each open item is explicitly recorded as an external human blocker with a named owner + rationale + sign-off (the gate names them; it does not silently pass over a critical/high).

  The gate's verdict is computed mechanically from the named scorecards (it reads dated green artifacts, never a self-claim). Any single condition RED ⇒ the gate is RED ⇒ no production release. A threshold is never weakened to flip it; a red condition is a dated honest row + an escalation. **Floor named:** the gate explicitly enumerates any remaining external/human blocker (HSM physical keying ceremony, sub-processor DPA, counsel/DPO erasure-residual ratification, staffed security owners) as a NAMED release prerequisite — the gate cannot be green while any critical one is unrecorded.
- **CONTRACTS TO IMPLEMENT.** The release-gate meta-contract (reads the contract-1.8 signals + the scorecards); implements no new system contract.
- **GATE / DRILLS (quantified).** The release-gate binary returns non-zero (RED) unless all 13 conditions are green; it is proven to bite by a fixture that makes any one condition red (e.g. reintroduce a `StructuralVerifier` use, hardcode `oneshot=true`/`init=/bin/true` on a real-job sandbox launch, add an open critical pentest finding, leave an advisory open) → the gate goes RED. 0 conditions may be silently skipped. Dated green artifact = the only thing that authorizes a production release.
- **TESTS (required).** The release-gate self-test: a green full-stack fixture ⇒ zero exit; one-condition-red fixtures (one per condition, including a sandbox-launch reverted to `oneshot=true`/`init=/bin/true`) ⇒ non-zero exit (the gate bites on each). Mutation floor: the gate's AND-of-all-conditions logic is mandatory-core (a mutation that ORs, or that ignores a condition, MUST be caught — the gate must not be weakenable).
- **DEFINITION OF DONE.** The fail-closed release-gate binary + CI job + scorecard exist; the gate is RED by default and green only when all 13 conditions emit dated green artifacts; it is proven to bite on each condition; every external/human blocker is enumerated by name; PROVEN, not claimed; committed. This is the last M7 prompt — the platform is production-releasable only when this gate is green.
- **COMMIT.** Header `P-546 M7: the fail-closed production-release gate`. Body: the 13 conditions + their scorecard sources; the bite self-test per condition; the enumerated external/human blockers; the AND-logic mutation score. Co-Authored-By trailer.

---

## Digest

**The M7 band (25 prompts, P-522..P-546), strict dependency order, implementation vs verification vs gate:**

- **Durable persistence:** P-522 (impl) → P-523 (verify).
- **KMS / HSM:** P-524 (impl) → P-525 (verify). Depends on P-522.
- **Auth crypto:** P-526 (human/SSO impl), P-527 (token impl) → P-528 (verify both + expiry). Depend on P-522 + P-524.
- **Backup/restore:** P-529 (real driver impl) → P-530 (verify measured RPO/RTO). Depend on P-522 + P-524.
- **Tenant isolation:** P-531 (impl on the live pool). Depends on P-522.
- **Secret handling:** P-532 (impl sweep) → P-533 (verify). Depend on P-524 + P-527.
- **Production runtime:** P-539 (impl). Depends on P-522.
- **Supply chain (parallel after P-534):** P-534 (pin) → P-535 (cargo-deny) → P-536 (SBOM/provenance) → P-537 (reproducible) ; P-538 (governance docs).
- **Gate integrity + truth-up:** P-540 (mechanical enforcement) → P-541 (truth-up pass).
- **External human blockers (recorded):** P-542 (crypto+sandbox reviews + prod escape drill) → P-543 (pentest).
- **Sandbox production exec:** P-544 (impl — Firecracker + gVisor run the real `spec.command`; no `init=/bin/true` on real jobs) → P-545 (verify — production-path exec + AG-D4 corpus through the prod path, 0 escapes on both backends). Depend on P-522 + P-524 (P-544) and P-544 + P-542 (P-545).
- **THE RELEASE GATE:** P-546 (fail-closed; reads all of the above; RED by default).

**Implementation prompts:** P-522, P-524, P-526, P-527, P-529, P-531, P-532, P-534, P-535, P-536, P-537, P-538, P-539, P-544.
**Verification prompts:** P-523, P-525, P-528, P-530, P-533, P-540, P-541, P-545.
**External/human-blocker-recording prompts:** P-542, P-543.
**The final fail-closed release gate:** P-546.

**Honesty.** No M7 prompt accepts a model, mock, or dogfood run as proof of a production mechanism: each mechanism's proof is a separate verification prompt running under `--features integration` with `MYELIN_REQUIRE_KVM`/`MYELIN_REQUIRE_DB` so a backend-free run HARD-FAILS. A drill on a special harness is NOT proof of a production exec path — the sandbox impl (P-544) and its production-path escape verification (P-545) are separate prompts. Every external/human prerequisite (real-HSM keying ceremony, independent crypto audit, independent sandbox audit, third-party pentest, sub-processor DPA, counsel/DPO erasure-residual ratification, staffed security owners) is NAMED and RECORDED, never silently swallowed, and is a hard blocker on the P-546 release gate.
