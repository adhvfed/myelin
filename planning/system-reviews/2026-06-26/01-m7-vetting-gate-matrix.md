# M7 Vetting Gate Matrix

Date: 2026-06-26.

This matrix turns the M7 prompt band into reviewable gates. It is deliberately stricter than "tests pass": each row says what old floor it must kill, what evidence should exist, and what cannot count as evidence.

## Gate Classes

| Class | Purpose | Required behavior |
|---|---|---|
| Static source gate | Reject forbidden production graph wiring, unsafe derives, unpinned inputs, optional required jobs. | Red on fixtures that reintroduce the forbidden pattern; green only on non-test production graph. |
| Unit/contract gate | Pin local semantics and contract-provider/consumer pairs. | Mutation-sensitive; keeps existing contract coverage intact. |
| Integration gate | Prove behavior against live DB/cache/object/bus/KMS/sandbox dependencies. | `MYELIN_REQUIRE_DB=1` / `MYELIN_REQUIRE_KVM=1` style hard-fail when dependency is absent. |
| Blackbox adversarial gate | Attack the system from an external boundary. | Proves denial, non-leakage, durability, and no-resurrection by observable outcomes. |
| Evidence-integrity gate | Prove the proof was actually run and cannot be silently weakened. | Attested scorecards, required jobs, red-on-tamper self-tests. |
| Human/external gate | Record independent security review, pentest, and real-HSM ceremony status. | Blocks release while critical/high findings or unrecorded prerequisites remain. |

## Prompt Matrix

| Prompt(s) | Area | Must fail on old floor | Evidence to require | Non-evidence |
|---|---|---|---|---|
| P-522/P-523 | Durable identity stores | In-memory principal/tuple/revocation store loses state after restart and diverges across instances. | Live Postgres/Valkey integration tests; N=10000 crash/restart; 3-instance consistency; scanner proving no production constructor uses in-memory durable store. | CRUD unit tests over in-memory stores; dogfood issue creation; comments saying the store models SQL. |
| P-524/P-525 | KMS root and key lifecycle | Process-held root, plaintext wrapped-key maps, destroyed key resurrects after restore, key bytes appear in traces. | SoftHSM or HSM-class adapter test; root-never-exported assertion; rotation/destruction drills; restore-after-destroy; zeroize/leak tests at TRACE. | AES-GCM unit tests alone; Debug redaction alone; modeled backup without real restore path. |
| P-526/P-527/P-528 | Human, SSO, machine, capability auth | Structural envelopes admit forged signatures, widened caveats, replayed DPoP, expired/revoked tokens, or path-derived tenants. | OIDC/SAML/WebAuthn/SSH negative vectors; PASETO/biscuit/DPoP/TPM negative vectors; no-Structural production scanner; expired-grant lifecycle drill. | Parsing valid-looking strings; checking `dpop=1`; test-only verifiers; happy-path auth only. |
| P-529/P-530 | Backup and restore | Modeled WAL offset passes while real base backup/WAL replay loses data or resurrects shredded keys. | Real base backup + WAL shipping + PITR; destructive clean-target restore; measured RPO/RTO; post-restore re-erasure; restored service smoke. | Restore model over abstract offsets; copying app state in memory; RPO/RTO asserted from configured thresholds. |
| P-531 | Tenant and residency isolation | Pooled connection keeps prior tenant session state, bare queries bypass tenant scope, dynamic identifiers inject, TLS/region mismatch connects. | SET LOCAL transaction scoping; reset-on-release tests; bare connection guard; identifier allowlist tests; mTLS and region fail-fast drill. | RLS DDL existence alone; tenant predicate in one query; single-request happy path. |
| P-532/P-533 | Secret handling | Bearer values or raw keys appear in Debug, Display, Error, panic, trace, metrics, or serialized JSON. | Static lints for secret types; redacted Debug tests; sentinel values through all sinks; deny Serialize on bearer credentials unless explicitly wrapped. | Private fields alone; "do not log" comments; one Debug snapshot. |
| P-534/P-535 | Input pinning and dependency policy | CI accepts branch/tag actions, floating images, floating Rust, vulnerable crates, disallowed licenses/sources. | SHA-pinned actions; digest-pinned images; `rust-toolchain.toml`; `cargo deny check advisories bans licenses sources`; red fixtures for unpinned inputs. | `Cargo.lock` alone; pinned major tags; advisory scans run only locally. |
| P-536/P-537 | SBOM, provenance, reproducibility | Release artifact has no dependency inventory or cannot be reproduced from pinned inputs. | SBOM artifact; provenance attestation; double-build bit-for-bit check; artifact identity recorded in scorecard. | Debug build hash; source archive only; CI logs without signed/attested metadata. |
| P-538 | Security governance | No clear intake, ownership, embargo, response, or review path. | `SECURITY.md`; `CODEOWNERS`; vulnerability response SLA; required owners on security-critical paths. | Informal README text; one maintainer convention. |
| P-539 | Runtime lifecycle and telemetry | Service drains only through deterministic in-process test trigger; trace context disappears across surfaces. | Real SIGTERM/SIGINT test; bounded drain; not-ready during drain; OpenTelemetry export; traceparent propagation across public/internal/outbox. | Unit testing `drain()` directly; metrics struct captured only in tests. |
| P-540 | Gate integrity | Required DB/KVM/sandbox/mutation jobs can be skipped or made advisory; scorecards can be hand-edited. | CI scanner for `continue-on-error` and optional required gates; KVM/DB absent hard-fail tests; required cargo-mutants floors; signed/attested scorecards. | Workflow comments saying "no skip"; local mutation output not required by CI. |
| P-541 | M7 truth-up | M0..M6 scorecards remain green because they still run over structural floors. | Re-run every prior scorecard against the M7 production graph; new `m7-production-readiness.md`; honest blockers for any regression. | Reusing old scorecard files; grep-only confirmation; dogfood green status. |
| P-542/P-543 | External security review and pentest | Internal tests miss high-severity crypto, sandbox, or application-security flaws. | Findings registers with reviewer, date, severity, status, remediation commit; 0 critical/high open for release. | Scheduling email alone; untriaged PDF; accepted risk without named rationale. |
| P-544/P-545 | Sandbox production exec | Firecracker still boots `init=/bin/true`; gVisor still probes `runsc --version`; escape corpus still runs through harness path. | Production `launch()` runs `JobSpec.command`; exit/stdout/stderr captured; timeout kills whole guest/container; accounting settles after completion; AG-D4 corpus through production path on both backends with 0 escapes. | Special command-drive harness; backend reachability probe; hardening-profile unit tests alone. |
| P-546 | Release gate | Any open floor, mock, scan miss, skipped job, stale scorecard, or human blocker still allows release. | Single fail-closed AND gate over all prior artifacts; red-on-each-condition fixtures; dated release scorecard. | Manual release checklist; "all tests pass" without artifact provenance. |

## Static Analysis Plan

Add or extend `myelin-lints` for these checks:

- Production graph must not instantiate `StructuralVerifier`, `StructuralTokenVerifier`, `StructuralTokenSigner`, or `StructuralAttestationVerifier`.
- Production graph must not instantiate the named in-memory durable stores for principal, tuple, or revocation state.
- Secret-bearing types must not derive or implement `Serialize`, `Debug`, `Display`, or `Error` without an approved redaction wrapper.
- SQL entrypoints touching tenant data must require a scoped tenant transaction handle, not a bare pool connection.
- Dynamic SQL identifiers must come from allowlisted typed identifiers.
- Required workflows must not contain `continue-on-error` on release, integration, mutation, DB, KVM, sandbox, or security-scan gates.
- GitHub Actions must be SHA-pinned and container images digest-pinned.
- M7 release scorecards must be generated artifacts with attestation metadata, not free-form hand edits.

Each static gate needs a red fixture in the lints crate. A source scanner with no red fixture is too easy to weaken accidentally.

## Integration Test Plan

Keep `scripts/integration-test.sh` as the base live-stack gate, but M7 needs specialized runners:

- `scripts/m7-db-hardening-test.sh`: DB/cache required, destructive restart/restore allowed.
- `scripts/m7-kms-test.sh`: SoftHSM or equivalent required.
- `scripts/m7-sandbox-test.sh`: KVM, Firecracker, runsc required.
- `scripts/m7-release-gate.sh`: reads only generated artifacts and exits non-zero on any missing, stale, red, or unaudited condition.

These scripts should set explicit `MYELIN_REQUIRE_*` variables and hard-fail before running tests if the dependency is unavailable. A skipped backend is a red gate, not a warning.

## Scorecard Requirements

M7 should add these generated scorecards:

- `testing/scorecards/m7-durable-persistence.md`
- `testing/scorecards/m7-kms.md`
- `testing/scorecards/m7-auth.md`
- `testing/scorecards/m7-backup-restore.md`
- `testing/scorecards/m7-tenant-isolation.md`
- `testing/scorecards/m7-secret-handling.md`
- `testing/scorecards/m7-supply-chain.md`
- `testing/scorecards/m7-runtime.md`
- `testing/scorecards/m7-gate-integrity.md`
- `testing/scorecards/m7-production-readiness.md`
- `testing/scorecards/m7-external-reviews.md`
- `testing/scorecards/m7-pentest.md`
- `testing/scorecards/m7-sandbox-exec.md`
- `testing/scorecards/m7-release-gate.md`

Every row should record command, date, dependency requirements, artifact hash, and whether the row is permanent. The P-546 release gate should read these scorecards and fail on stale or missing rows.
