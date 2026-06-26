# M7 Hardening Strategy

Date: 2026-06-26.

This review starts from the current repository state, not from the original roadmap prose. Recent commits show M6 dogfood completion through P-521, and `testing/scorecards/m6-dogfood.md` marks the M6 gate green while explicitly warning that M7 is next and not yet implemented. The next phase is therefore not feature expansion. It is a production-readiness hardening band that must prove the platform no longer depends on structural floors.

Canonical inputs:

- `testing/scorecards/m6-dogfood.md`: M6 is dogfood-complete, not production-ready.
- `planning/07-prompts/production-readiness-audit.md`: the eleven-finding audit that motivates M7.
- `planning/07-prompts/coverage-matrix.md`: maps every M7 prompt P-522..P-546 to an audit finding.
- `planning/07-prompts/by-system/production-readiness.md`: the implementation and verification prompt bodies.
- `.github/workflows/ci.yml` and `.github/workflows/integration.yml`: current committed gates.
- `scripts/integration-test.sh`: current live-stack integration gate.
- `planning/system-reviews/2026-06-26/03-whole-system-end-to-end-vetting.md`: the whole-system vetting model
  that composes M0..M7 into release confidence.
- `planning/system-reviews/2026-06-26/04-m7-agent-handoff-chunks.md`: the per-prompt sub-agent chunking map for
  resumed ledger execution.

## Strategic Read

The system has a strong M0..M6 proof culture: scorecards, contract coverage, lints, integration gates, and mutation gates are already present. The problem is not lack of tests in general. The problem is that several load-bearing production mechanisms are still represented by honest structural floors, and some existing green evidence proves the floor rather than the production mechanism.

The M7 strategy should therefore be:

1. Preserve the existing ratchets.
2. Add production-graph absence checks for every floor class.
3. Add blackbox drills that prove security and durability properties from outside the implementation.
4. Add supply-chain and evidence-integrity gates so the proof artifacts themselves become hard to spoof.
5. Make the release gate fail-closed and mechanically computed from dated green artifacts.

## The Core M7 Rule

An M7 gate cannot accept any of these as production proof:

- A model of a backend instead of the backend.
- A dogfood/switch test instead of an adversarial drill.
- A special harness path instead of the production path.
- A structural verifier/signer instead of real cryptographic verification.
- A process-local or in-memory store where the release graph requires durability.
- A handwritten scorecard row without an attested command result.
- A skipped KVM/DB/HSM/security-review job.

Every M7 implementation prompt needs a separate verification prompt or release-gate condition. The verification prompt should be able to fail even when the implementation compiles and unit tests pass.

## Hardening Tracks

| Track | M7 prompts | Main risk | Proof style |
|---|---:|---|---|
| Durable identity persistence | P-522, P-523 | Principal, tuple, or revocation state disappears or diverges after restart or across instances. | Crash/restart and multi-instance blackbox drills over the live DB/cache; production-graph scanner rejects in-memory stores. |
| KMS and secret memory | P-524, P-525 | Keys stay process-local, resurrect after restore, or leak through logs/traces/debug output. | HSM-class adapter tests, no-root-export check, restore-after-destroy drill, zeroization/leak sentinel tests. |
| Real auth/token crypto | P-526, P-527, P-528 | Structural envelope parsing admits forged, replayed, expired, or widened credentials. | Cryptographic negative test vectors, replay tests, no-Structural production graph scanner, expired-grant drill. |
| Backup/restore | P-529, P-530 | Modeled WAL gives false confidence; restore loses data or revives erased data. | Real WAL/base backup/PITR, destructive clean-target restore, measured RPO/RTO at cell scale. |
| Tenant/residency isolation | P-531 | Pooled connections bleed tenant/session state, bare queries bypass scope, or region endpoints mismatch. | Pooled-connection adversarial tests, SET LOCAL/reset-on-release checks, mTLS/region fail-fast tests. |
| Secret handling | P-532, P-533 | Bearer values, keys, or credentials leak through Debug, Error, trace, panic, or serialization. | Sentinel corpus through all logging/error/tracing sinks; static lints against unsafe trait derives. |
| Supply chain and governance | P-534..P-538 | Unpinned inputs, known-vulnerable crates, unclear vulnerability response, unauditable artifacts. | SHA/digest/toolchain pinning, cargo-deny, SBOM, provenance, reproducible build, SECURITY.md/CODEOWNERS. |
| Runtime and observability | P-539 | Production lifecycle drains only under tests, or trace context breaks across surfaces. | Real OS signal tests, bounded drain, OpenTelemetry export and trace propagation assertions. |
| Evidence integrity | P-540, P-541 | Required jobs become optional, green scorecards are stale or hand-edited, old floor proofs remain green. | CI optionality lints, signed/attested scorecards, M7 truth-up against the production graph. |
| External review and pentest | P-542, P-543 | Internal tests miss crypto/sandbox/application security bugs. | Recorded independent review and pentest findings with 0 critical/high open for release. |
| Sandbox production exec | P-544, P-545 | Firecracker/gVisor hardening is proven, but production `JobSpec.command` does not run through that path. | Command/exit/stdout/stderr/timeout tests through production launch, then AG-D4 corpus through both production backends. |
| Final release gate | P-546 | A production release goes green over any open floor or human blocker. | Single fail-closed AND gate over the prior artifacts. |

## Recommended Execution Order

The roadmap order is basically right, but the review should enforce a stricter verification cadence:

1. Land supply-chain pinning and scanner infrastructure early enough that later M7 artifacts are built on pinned inputs.
2. Land P-522/P-523 first for real stateful infrastructure, because many later proofs require live DB/cache behavior.
3. Land P-524/P-525 before real auth/token crypto, because signing and key lifecycle must not depend on process-held roots.
4. Land P-526/P-528 and P-527/P-528 as a pair: implementation cannot count without the production-graph absence scanner and forged/replay/expiry drills.
5. Land P-529/P-530 before any release-grade erasure/restore claim.
6. Land P-531 before truth-up, since old scorecards using pooled DB behavior need to be re-run against transaction-local tenant scoping.
7. Land P-544/P-545 late enough to use durable metering and real KMS-backed job secrets, but before external sandbox review is considered complete.
8. Run P-541 only after the major production mechanisms are in place; it is a truth-up, not a substitute for their verification prompts.
9. Keep P-542/P-543 open as explicit human blockers until evidence is recorded.
10. Run P-546 last and keep it red until every prerequisite emits a dated green artifact.

## What To Avoid

- Do not broaden M7 into new product work.
- Do not merge a production implementation without its paired negative/adversarial proof.
- Do not let `--features integration` tests skip when DB/KVM/runsc/HSM emulator dependencies are absent.
- Do not accept a source grep as the only proof for a security boundary; it is useful as a scanner, but blackbox behavior still needs to fail closed.
- Do not treat the M6 dogfood scorecard as customer-production readiness.

## Done Bar For This Review

M7 is ready to execute when every prompt P-522..P-546 has:

- A named production floor it closes.
- A concrete implementation target.
- A verification artifact that would fail on the old floor.
- A CI or scorecard gate that is red by default.
- A blackbox/adversarial test for every security or persistence claim.
- A human-blocker record when automation cannot produce the proof.

The broader release is ready only when the whole-system campaign in
`03-whole-system-end-to-end-vetting.md` is green: production-graph absence, boundary blackbox drills,
whole-cell chained E2Es, operational failure/scale drills, and evidence/external-review gates all feed P-546 as
fresh attested inputs.

For execution, `04-m7-agent-handoff-chunks.md` is the operational companion: it keeps each global prompt as the
commit-sized ledger unit while splitting the work into sub-agent packets that a resumed Claude agent can hand
off without losing the M7 done-bar.
