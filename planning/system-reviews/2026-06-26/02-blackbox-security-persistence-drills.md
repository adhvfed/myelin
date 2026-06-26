# Blackbox Security And Persistence Drills For M7

Date: 2026-06-26.

These drills are intentionally written from the outside of the system. They should be implemented as integration or system tests that interact through public/internal production surfaces and live backing services, not by directly calling private helpers. Private unit tests remain useful, but they cannot prove the release boundary.

## Drill Principles

- Attack observable outcomes, not implementation details.
- Force the dependency to exist: no DB/KVM/runsc/KMS dependency means red, not skipped.
- Run against production constructors and production feature flags.
- Include a negative control that would pass on the old floor only if the gate is weak.
- Emit dated artifacts that P-546 can read mechanically.

## Persistence Boundary Drills

### D1: Identity State Survives Process Death

Purpose: kill the in-memory-store floor.

Setup:

- Start production identity service against live Postgres and Valkey.
- Create N=10000 mixed principals, ReBAC tuples, run-token revocations, and expiry records across at least 3 tenants and 2 regions.
- Record committed write IDs through the public/internal API.

Attack:

- Kill the service with `kill -9` during a mixed write batch.
- Restart a fresh process over the same backing services.
- Query through the production API.

Pass:

- 0 committed rows lost.
- 0 uncommitted rows resurrected.
- 0 cross-tenant or cross-region visibility.
- Revoked/expired tokens remain denied.

Old-floor failure mode:

- In-memory principal/tuple/revocation stores lose all or part of the committed set.

### D2: Multi-Instance Identity Consistency

Purpose: prove there is one durable source of truth, not per-process memory.

Setup:

- Run 3 identity service instances over the same live DB/cache.
- Use one writer and two readers.

Attack:

- Write tuple grants/revocations on instance A.
- Read authorization snapshots and `list_objects` through instances B and C within the zookie consistency bound.
- Restart instance B mid-run.

Pass:

- 0 split-brain authorization decisions after the consistency bound.
- 0 stale grant after revocation SLA.
- Restarted instance B observes the durable state without warmup-only magic.

### D3: PITR Restore With Re-Erasure

Purpose: prove backup/restore is real and does not revive erased data.

Setup:

- Use real WAL shipping/base backup/PITR against a live database and object tier.
- Insert ordinary records plus encrypted personal-data holders.
- Destroy a subject DEK/tenant KEK through the production erasure path.

Attack:

- Restore to a clean target from base backup plus WAL.
- Attempt to read the erased subject's plaintext through every holder surface.
- Query metadata and immutable structures expected to survive.

Pass:

- RPO/RTO measured and under thresholds.
- 0 plaintext for erased subject.
- Destroyed keys absent from restored KMS state.
- Immutable structures survive only with pseudonymous/unrecoverable identity.
- Clean target proves the source was not reused.

Old-floor failure mode:

- Modeled restore passes without exercising real WAL, clean-target provisioning, or KMS backup semantics.

## Authentication And Authorization Drills

### A1: Forged Credential Corpus

Purpose: prove real cryptographic verification replaces structural parsing.

Attack corpus:

- OIDC token with flipped JWS signature byte.
- OIDC token with valid signature but wrong `aud`.
- OIDC token with expired `exp` or future `nbf`.
- SAML assertion with signed wrapper plus tampered inner subject.
- SAML assertion replayed by assertion ID.
- WebAuthn assertion with wrong challenge or RP ID.
- SSH challenge response over the wrong nonce.

Pass:

- 0 forged credentials authenticate.
- Tenant is derived only from verified credential claims.
- Path/header tenant spoofing cannot change the tenant.

Old-floor failure mode:

- Structural envelope parser admits strings with valid shape.

### A2: Token Caveat And DPoP Corpus

Purpose: prove machine/capability tokens are signed, attenuated, sender-constrained, and revocable.

Attack corpus:

- Capability token with flipped signature.
- Token with caveat edited to widen authority.
- Expired token.
- Not-yet-valid token.
- Token whose `jti` is revoked in S7.
- DPoP-bound PAT sent without DPoP proof.
- DPoP proof replayed on a different request.
- DPoP proof with wrong `htu`/`htm`.
- Self-hosted runner token request with absent or forged TPM quote.

Pass:

- 0 forged, widened, expired, revoked, unbound, replayed, or unattested tokens authorize.
- Attenuated token authority is a strict subset of parent authority.
- Revocation denies within SLA on every surface.

### A3: Expired Grant Lifecycle

Purpose: prove expired grants never enter snapshots across lifecycle edges.

Attack:

- Write grants already expired, grants expiring during process death, and grants expiring during replay.
- Run `check` and `list_objects` before teardown, after teardown, after crash recovery, and after cleanup.

Pass:

- 0 authorizations from expired tuples in all phases.
- Coarse/fail-static cache denies just-revoked or expired grants.

## Tenant And Residency Drills

### T1: Pooled Connection Tenant Bleed

Purpose: prove transaction-local tenant scoping and reset-on-release.

Attack:

- Tenant A performs a scoped query on pooled connection X.
- Return X to the pool.
- Tenant B receives X and performs a query without manually setting tenant state.
- Attempt both read and write.

Pass:

- Tenant B cannot see Tenant A.
- Bare pooled operations are rejected before query execution.
- `current_setting('myelin.tenant_id')` is transaction-local or cleared outside the transaction.

Old-floor failure mode:

- Session-scoped `set_config(..., false)` leaks state across checkouts.

### T2: Region Endpoint Mismatch

Purpose: prove residency fail-fast.

Attack:

- Configure tenant region `fr-par`.
- Point service to a mismatched DB/object/cache endpoint region.
- Attempt boot and write.

Pass:

- Boot fails before accepting traffic, or write fails closed.
- No cross-region write reaches storage.

### T3: Dynamic Identifier Injection

Purpose: prove dynamic SQL identifiers are typed and allowlisted.

Attack:

- Submit tenant/project/table-like values containing identifier escape sequences through APIs that influence query shape.

Pass:

- Inputs are rejected as invalid identifiers.
- No SQL query executes with attacker-controlled identifier text.

## Secret Handling Drills

### S1: Sentinel Secret Corpus

Purpose: prove secrets cannot leak through logs, traces, panics, errors, metrics, or serialization.

Setup:

- Use unique high-entropy sentinels for raw keys, bearer tokens, DPoP proofs, WebAuthn challenges, SSH nonces, S3 credentials, DB URLs, KMS handles, and CI job secrets.
- Enable maximum logging/tracing.

Attack:

- Drive auth, token minting, KMS wrap/unwrap/rotate, restore, sandbox job execution, failing DB queries, and intentional error paths.
- Capture stdout/stderr, tracing subscriber output, structured logs, metrics labels, panic hooks, API error bodies, and JSON serialization.

Pass:

- 0 raw sentinel occurrences.
- Only approved redacted tokens or stable opaque IDs appear.

### S2: Trait-Derive Regression

Purpose: stop leaks before runtime.

Attack:

- Add red fixtures that derive `Serialize`, `Debug`, `Display`, or `Error` on bearer/key credential types.

Pass:

- Static gate rejects each fixture.

## Sandbox Drills

### X1: Production Command Execution

Purpose: prove `JobSpec.command` runs through production `launch()` on both Firecracker and gVisor.

Attack:

- Submit command that prints known stdout/stderr and exits 0.
- Submit command that exits 42.
- Submit command that sleeps beyond timeout.
- Submit command that tries to leave child processes behind.

Pass:

- Exit code is captured exactly.
- Stdout/stderr match byte-for-byte.
- Timeout kills the whole guest/container within deadline.
- 0 orphaned VMM/runsc/container processes.
- Settlement occurs exactly once after command completion, never on reachability probe.

Old-floor failure mode:

- Firecracker `init=/bin/true` returns success without running command.
- gVisor `runsc --version` probe returns success without running bundle.

### X2: Production-Path Escape Corpus

Purpose: prove the real production path, not the special harness, contains untrusted code.

Attack families:

- Metadata service access.
- RFC1918/private network egress.
- DNS rebinding or IP normalization bypass.
- Host filesystem read/write attempts.
- Capability escalation.
- PID/cgroup escape attempt.
- Long-running process/orphan attempt.
- Secret exfiltration via logs/artifacts.
- Cache/artifact poisoning across trust tiers.
- Workspace persistence across jobs.
- Kernel/syscall escape corpus already used by AG-D4/CI-T1.

Pass:

- 0 escapes on Firecracker production path.
- 0 escapes on gVisor production path.
- 0 host/private network access.
- 0 cross-job persistence.
- 0 secrets in outputs.
- Guard fails red if the corpus is routed to a harness-only path.

## Supply-Chain And Evidence Drills

### C1: Unpinned Input Fixtures

Attack:

- Add fixture workflows using `actions/checkout@v4`, branch-based actions, floating container tags, or `stable` toolchain.

Pass:

- Supply-chain lint rejects each fixture.

### C2: Advisory And License Fixtures

Attack:

- Add cargo-deny fixtures with a known vulnerable advisory, duplicate/banned crate, disallowed license, and disallowed source.

Pass:

- `cargo deny` gate rejects each fixture.

### C3: Scorecard Tamper

Attack:

- Manually edit a generated scorecard row from red to green.
- Delete an artifact referenced by a green row.
- Change a command line after artifact generation.

Pass:

- Attestation/provenance check fails.
- P-546 refuses release.

### C4: Release Gate Red-On-Each-Condition

Purpose: prove P-546 is an AND gate, not a checklist.

Attack:

- For each release condition, create a fixture with exactly that condition missing or red.

Pass:

- Release gate exits non-zero for each fixture.
- Gate exits zero only when every condition has a fresh dated green artifact and every external blocker is closed or explicitly accepted by policy where allowed.

## External Review Drills

Automation cannot replace independent review, but the repo can make review state impossible to hide.

Required records:

- Independent cryptography review of KMS, auth, token, DPoP, attestation.
- Independent sandbox review of Firecracker and gVisor production execution paths.
- Third-party penetration test against a production-representative deployment.
- Findings register with severity, owner, remediation commit, retest date, and status.

Release pass:

- 0 critical/high findings open.
- Every accepted lower-severity finding has a named rationale and owner.
- Missing review record blocks P-546.
