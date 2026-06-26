# M7 Agent Handoff Chunks

Date: 2026-06-26.

This document makes the M7 review overlay executable by a resumed ledger agent. The ledger unit remains the
global prompt P-522..P-546: one main agent owns one prompt, assembles the final patch, runs the gate, writes the
scorecard, and commits. Inside that prompt, the main agent may hand off bounded work packets to sub-agents.

The target context model is the existing ledger model: each global prompt is self-contained and large enough to
carry roughly 400k-700k tokens of relevant context when expanded by its CANON DOCS, code reads, test output,
review overlay, and sub-agent results. Do not split a P-NNN into multiple ledger commits unless a blocker creates
a new explicit follow-on prompt. Sub-agent packets are implementation aids, not new roadmap prompts.

## Universal Handoff Rules

Every P-522..P-546 main agent must:

1. Read its prompt body in `planning/07-prompts/by-system/production-readiness.md`.
2. Read the M7 review overlay files that apply:
   - `00-m7-hardening-strategy.md`
   - `01-m7-vetting-gate-matrix.md`
   - `02-blackbox-security-persistence-drills.md`
   - `03-whole-system-end-to-end-vetting.md`
   - this file
3. Create sub-agent packets only for independent read/build/test slices.
4. Keep one authoritative integration branch in the main agent's workspace.
5. Require every sub-agent result to include: files touched or inspected, property proven, command output summary, remaining risk, and whether the old floor would fail.
6. Reject any sub-agent result that proves only a model, mock, dogfood path, or special harness where the prompt requires production proof.
7. Merge only when the prompt's own GATE / DRILLS field is green.

## Context Budget Pattern

Use this approximate split inside each 400k-700k prompt context:

| Context slice | Budget posture |
|---|---|
| Prompt body + M7 overlay | Always loaded by main agent. |
| Canon architecture/testing docs | Loaded by main agent; section-scoped where possible. |
| Codebase read | Split among sub-agents by crate or boundary. |
| Implementation | Main agent owns final patch; sub-agents can draft isolated changes or reports. |
| Verification | Sub-agents can run narrow gates; main agent runs the prompt-level gate. |
| Scorecard/evidence | Main agent writes final generated/dated artifact. |

If context pressure rises, drop broad prose first and keep: prompt body, floor evidence, modified code, gate
commands, and scorecard schema.

## P-522/P-523: Durable Identity Persistence

Main-agent objective: replace in-memory identity durable stores with live OLTP/cache bindings, then prove no
production graph path can regress to in-memory stores.

Sub-agent packets:

- **Store-surface inventory:** inspect principal, tuple, revocation store APIs and production constructors; report exact seams and old-floor constructors.
- **Storage binding design:** inspect `myelin-storage` OLTP/RLS/cache APIs and migrations; propose S1/S3/S7 schema and query plan.
- **Integration drill design:** define restart, kill -9, multi-instance, and RLS refusal tests with `MYELIN_REQUIRE_DB=1`.
- **Static scanner:** extend lints/contract-coverage for no in-memory durable store in production graph, with red fixture.

Main merge gate: live DB/cache tests green; scanner red/green fixtures green; M7 durable-persistence scorecard row written.

## P-524/P-525: KMS/HSM And Secret Memory

Main-agent objective: move the KMS root/lifecycle behind an HSM-class adapter and prove keys zeroize, never export, never resurrect, and never leak.

Sub-agent packets:

- **KMS seam inventory:** map `KmsAdapter`, `CellRoot`, KEK/DEK storage, backup snapshot, fail-static cache.
- **HSM adapter path:** design and implement SoftHSM/PKCS#11 or Vault-Transit-class adapter behind existing seam.
- **Key lifecycle drills:** build root generation, split recovery, rotation, destruction, and restore-after-destroy tests.
- **Secret-memory verification:** add zeroize/secrecy wrappers and TRACE-level key sentinel leak corpus.

Main merge gate: root-never-exported, rotation, destruction, no-resurrection, zeroize, and no-leak artifacts green.

## P-526/P-527/P-528: Real Auth And Token Crypto

Main-agent objective: replace structural auth/token floors with real cryptographic verification and prove old
structural paths are absent.

Sub-agent packets:

- **Credential verifier packet:** OIDC JWKS, SAML XML-DSig, WebAuthn, and SSH challenge verifier implementation/test vectors.
- **Token verifier packet:** signed capability/machine/run token format, caveat monotonicity, KMS signing key use, revocation consult.
- **DPoP/attestation packet:** RFC 9449 proof validation, replay store, TPM/self-hosted runner attestation.
- **Negative corpus packet:** forged, expired, wrong-audience, replayed, widened-caveat, revoked, and path-tenant-spoof tests.
- **Production-graph scanner packet:** no `StructuralVerifier`, `StructuralTokenVerifier`, `StructuralTokenSigner`, or `StructuralAttestationVerifier` in production constructors.

Main merge gate: crypto negative corpus green; expired-grant lifecycle green; structural absence scanner red/green fixtures green.

## P-529/P-530: Real Backup/Restore And Measured RPO/RTO

Main-agent objective: replace modeled WAL/restore with real backup/PITR drivers and measure restore over real data.

Sub-agent packets:

- **Backup driver packet:** WAL archiving/base backup implementation and object-tier versioning.
- **Restore driver packet:** clean-target PITR, consistency point, object presence verification, derived-store reindex.
- **Re-erasure packet:** post-restore erasure ledger replay and destroyed-key non-resurrection.
- **Measurement packet:** RPO/RTO measurement harness, thresholds update, continuous-write dataset.
- **Corruption packet:** deliberately corrupted backup fixture that must fail.

Main merge gate: destructive clean-target restore green; RPO/RTO measured; corrupted backup fails; STOR-D1/STOR-D3 scorecards updated.

## P-531: Tenant And Residency Isolation

Main-agent objective: harden pooled live-store access so tenant/region scope cannot bleed or be bypassed.

Sub-agent packets:

- **Pool-scope audit:** locate all tenant data DB entrypoints and current session GUC handling.
- **Scoped transaction implementation:** `SET LOCAL` / transaction-local context, reset-on-release, unscoped-query guard.
- **Identifier validation:** typed allowlist for dynamic identifiers and red injection fixtures.
- **TLS/region packet:** mTLS and endpoint/region fail-fast checks.
- **Blackbox packet:** pooled Tenant A to Tenant B bleed drill.

Main merge gate: pool-leak, unscoped-query, identifier injection, cross-region, and TLS/region drills green.

## P-532/P-533: Secret Handling

Main-agent objective: make secret leakage structurally hard and prove every known sink is clean.

Sub-agent packets:

- **Secret inventory:** enumerate bearer/key/credential/secret-broker types and their derives/Display/Error paths.
- **Type hardening:** add secrecy/zeroize wrappers, redacted Debug, guarded serialization.
- **Static lint packet:** secret-bearing derive scanner with red fixtures.
- **Sentinel corpus packet:** drive auth/token/KMS/CI secret paths through logs, traces, errors, metrics, panic hooks, and JSON.

Main merge gate: 0 sentinel leaks; static lint red/green fixtures green; zeroize tests green.

## P-534/P-538: Supply Chain And Governance

Main-agent objective: pin inputs, add dependency policy, produce artifact evidence, and define security ownership.

Sub-agent packets:

- **Pinning packet:** SHA-pin GitHub Actions, digest-pin images, add `rust-toolchain.toml`, enforce `--locked`.
- **Policy packet:** `cargo-deny` advisories/licenses/sources, red fixtures.
- **SBOM/provenance packet:** generate SBOM, sign/attest artifacts, verification and tamper tests.
- **Reproducibility packet:** deterministic flags and double-build bit-for-bit gate.
- **Governance packet:** `SECURITY.md`, `CODEOWNERS`, vulnerability-response runbook, required owner mapping.

Main merge gate: pin scanner, cargo-deny, SBOM/provenance verify, reproducibility, and governance checks green.

## P-539: Production Runtime

Main-agent objective: finalize real OS signal lifecycle, OpenTelemetry export, and trace propagation.

Sub-agent packets:

- **Signal lifecycle packet:** SIGTERM/SIGINT handling, intake stop, not-ready, bounded drain, non-zero boot failure.
- **Telemetry export packet:** OpenTelemetry exporter and metrics-health contract wiring.
- **Trace propagation packet:** public gateway to internal RPC to event/outbox/worker traceparent assertions.
- **Runtime drill packet:** process-level signal tests rather than direct `drain()` unit calls.

Main merge gate: real signal drain, OTel export, and trace propagation artifacts green.

## P-540/P-541: Gate Integrity And Truth-Up

Main-agent objective: make gates non-optional and re-run prior scorecards against the M7 production graph.

Sub-agent packets:

- **Workflow optionality scanner:** reject `continue-on-error` or skippable required DB/KVM/sandbox/security jobs.
- **Mutation-required packet:** mandatory-core `cargo-mutants` CI job enforcement and score floors.
- **Scorecard attestation packet:** generated scorecard signing/provenance and tamper fixture.
- **Truth-up packet:** map M0..M6 scorecard rows to M7 production graph commands.
- **Regression packet:** identify any row that was green only on a floor and convert to honest blocker if it fails.

Main merge gate: optionality red fixtures, mutation-required gate, scorecard tamper check, and M7 truth-up scorecard green.

## P-542/P-543: External Review And Pentest Records

Main-agent objective: record external review/pentest status and make unresolved high-severity findings block release.

Sub-agent packets:

- **Crypto review packet:** scope, reviewer, findings register, remediation links.
- **Sandbox review packet:** Firecracker/gVisor production path review, AG-D4/CI-T1 rerun evidence.
- **Pentest packet:** production-representative deployment scope, CVSS/severity register, remediation tests.
- **Completeness scanner:** reject missing reviewer/date/severity/status/owner/remediation fields.

Main merge gate: records complete; mechanical escape rerun green; 0 unrecorded critical/high findings; unresolved items named as P-546 blockers.

## P-544/P-545: Sandbox Production Exec

Main-agent objective: make both committed sandbox backends execute real `JobSpec.command` through production
`launch()`, then run the escape corpus through that same path.

Sub-agent packets:

- **Firecracker packet:** implement vsock guest-agent or command-drive production command execution; preserve self-test oneshot only.
- **gVisor packet:** write OCI bundle and run `runsc run --bundle`, capturing stdout/stderr/exit.
- **Result/accounting packet:** timeout whole-guest/container kill, no orphans, settle exactly once after completion.
- **No-harness scanner packet:** prove real-job path has no `init=/bin/true`/probe-only shortcut and corpus is not routed to drill harness.
- **Escape corpus packet:** AG-D4 families through production `launch()` on Firecracker and gVisor.

Main merge gate: real command/exit/stdout/stderr/timeout/accounting tests green on both backends; production-path escape corpus 0 escapes.

## P-546: Fail-Closed Release Gate

Main-agent objective: compile all M7 evidence into one red-by-default release authorization.

Sub-agent packets:

- **Schema packet:** define release evidence schema: scorecard path, command, date, artifact hash, freshness, dependency requirement, attestation.
- **Reader packet:** implement release-gate binary reading all M0..M7/M7-review scorecards and external registers.
- **Condition packet:** encode all 13 P-546 conditions and old-floor absence checks as strict AND logic.
- **One-condition-red packet:** fixtures for each missing/red/stale/tampered/skipped condition, including persistence loss, forged auth, pooled tenant bleed, secret leak, sandbox harness routing, advisory open, scorecard tamper, missing external review.
- **Report packet:** generate `testing/scorecards/m7-release-gate.md` with blocker list.

Main merge gate: green full-stack fixture returns zero; every one-condition-red fixture returns non-zero; AND-logic mutation floor green.

## Resumption Checklist For A Future Agent

When resuming M7, do this before executing the next prompt:

1. Read recent commits and identify the highest merged P-NNN.
2. Read `testing/scorecards/m6-dogfood.md` and any new M7 scorecards.
3. Read the next prompt body in `production-readiness.md`.
4. Read the matching section in this handoff file.
5. Create sub-agent packets only from that section.
6. Refuse to mark the prompt done until its prompt-level gate is green and its evidence is generated.
7. Commit with the exact P-NNN header requested by the prompt.
