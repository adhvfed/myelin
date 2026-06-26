# Whole-System End-To-End Vetting Blueprint

Date: 2026-06-26.

This document steps above the M7 prompt list and answers the broader question: how to vet Myelin as an entire system before trusting it with real tenant data. The answer is not "more tests" in the abstract. The answer is a release-confidence model where every essential system property is proven at the narrowest reliable layer, then recomposed through whole-cell blackbox scenarios and a fail-closed release gate.

Canonical inputs:

- `planning/05-refined-shared-systems-architecture/testing-strategy/00-philosophy-levels-and-gates.md`
- `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
- `planning/07-prompts/production-readiness-audit.md`
- `planning/07-prompts/by-system/production-readiness.md`
- `testing/scorecards/m6-dogfood.md`
- `planning/system-reviews/2026-06-26/00-m7-hardening-strategy.md`
- `planning/system-reviews/2026-06-26/01-m7-vetting-gate-matrix.md`
- `planning/system-reviews/2026-06-26/02-blackbox-security-persistence-drills.md`

## The Vetting Thesis

Myelin needs three kinds of confidence, and none can substitute for the others:

1. **Local correctness:** each crate, contract, lint, and mutation-sensitive branch behaves as designed.
2. **Boundary hardness:** trust boundaries fail closed under adversarial input, dependency loss, process death, tenant confusion, and untrusted execution.
3. **Whole-system coherence:** real user and agent workflows still work when subsystems interact through the production gateway, durable stores, event bus, reference graph, workflow engine, and UI.

The release should be blocked unless all three are green and current. A green unit suite cannot prove tenant isolation. A green E2E cannot prove token cryptography. A green dogfood loop cannot prove restore. A green sandbox harness cannot prove production `JobSpec.command` execution.

## Release Confidence Layers

| Layer | What it proves | Main artifacts | Failure mode it catches |
|---|---|---|---|
| L0 Unit/property/mutation | Local behavior and branch sensitivity. | `cargo test`, property tests, `cargo-mutants` floors. | Broken algorithms, unchecked edge cases, dead assertions. |
| L1 Static architecture gates | Forbidden classes are structurally absent. | `myelin-lints`, contract coverage, production-graph scanners. | Raw event publishing, missing tenant predicates, structural verifiers in prod, in-memory durable stores. |
| L2 Contract and per-system integration | Each subsystem honors its frozen contracts over real dependencies. | CDC tests, `--features integration`, infra scorecards. | Provider/consumer drift, DB/cache/blob/bus mismatches, migration bugs. |
| L3 Boundary adversarial drills | Security, persistence, and compliance boundaries fail closed. | M7 blackbox drills, KVM/DB/KMS-required jobs, secret-leak corpus. | Forged credentials, tenant bleed, restore resurrection, sandbox escape, secret leakage. |
| L4 Whole-cell chained E2E | Cross-subsystem workflows work through production surfaces. | E2E-1..E2E-4, browser switch tests, telemetry snapshots. | Event/order bugs, stale projections, missing invalidation, UI workflow gaps. |
| L5 Chaos, restore, and scale | The cell survives real operational failure. | 1x/10x/30x drills, destructive restore, failover, dependency breaks. | Backpressure collapse, data loss, unbounded queues, bad RPO/RTO. |
| L6 Evidence and governance | Green means "run and attested", not "claimed". | Signed scorecards, SBOM/provenance, external review registers, release gate. | Hand-edited scorecards, stale scans, skipped jobs, unresolved critical findings. |

The final release gate should read these layers as an AND, not as weighted confidence. One red Tier-0 security boundary blocks release even if the product workflows are polished.

## The Whole-System Properties To Prove

### 1. Identity Is The Spine

System claim: every public operation derives tenant, region, principal, and authority from verified credentials and live authorization state, never from caller-controlled path shape or stale structural floors.

End-to-end proof:

- Real OIDC/SAML/WebAuthn/SSH and signed machine tokens are used on production constructors.
- Cross-tenant path/header spoofing fails from public gateway through every subsystem.
- Expired and revoked grants cannot authorize through `check`, `list_objects`, Search, Refs, Git, CI, Issues, Knowledge, Chat, or Notif.
- Whole-cell E2Es include at least one unauthorized viewer and assert tombstone/no-count/no-ranking leak.

Release blocker:

- Any `Structural*` verifier/signer in production graph.
- Any surface deriving tenant from URL/path instead of verified credential.
- Any stale authorization path that admits expired/revoked grants.

### 2. Persistence Is Durable And Recoverable

System claim: committed state survives process death, multi-instance operation, restore, and replay; erased data is not resurrected.

End-to-end proof:

- Identity principal/tuple/revocation state survives kill/restart and is shared by multiple instances.
- Core workflows run against real Postgres/cache/object/bus, not in-memory mirrors.
- Destructive clean-target restore proves base backup + WAL + object tier restore.
- Post-restore re-erasure proves old backups cannot revive erased plaintext.
- Whole-cell E2E-4 runs DSAR fan-out, erase, restore, and locate over all holders.

Release blocker:

- Modeled WAL or abstract restore standing in for real restore.
- In-memory "durable" mirrors in production constructors.
- Destroyed keys or erased holder plaintext appearing after restore.

### 3. Tenant And Residency Boundaries Hold Under Reuse

System claim: pooled resources, caches, indexes, queues, artifacts, and projections cannot leak across tenants or regions.

End-to-end proof:

- Pooled connection reuse after Tenant A cannot expose Tenant A to Tenant B.
- RLS uses transaction-local scoping with reset-on-release.
- Search and Refs pre-filter via authorization before ranking/count/backlink projection.
- CI artifacts, caches, logs, and fork trust tiers cannot cross trust or tenant boundaries.
- Region endpoint mismatch fails before accepting writes.

Release blocker:

- Session-scoped tenant state on pooled connections.
- Any tenant data query reachable through a bare pool handle.
- Any count/ranking/backlink leak for unauthorized viewers.

### 4. Agents Cannot Escape Their Delegation

System claim: agents are first-class principals with constrained authority, deterministic plan-then-apply, HITL gates for consequential effects, and auditable/metered execution.

End-to-end proof:

- E2E-2 proves CI failure to triage agent to issue/chat/fix PR/approval/merge through production surfaces.
- Agent proposed effects are deterministic for scripted runs.
- Consequential effects do not mutate before approval.
- Double approval applies once.
- Kill/restart during approval wait resumes correctly.
- Reserve/settle balances exactly once per completed unit.

Release blocker:

- Agent effect path bypasses `EffectApi`.
- Agent can exceed `agent.policy ∩ delegation ∩ tenant.policy`.
- Approval race or replay produces duplicate mutation.

### 5. Untrusted Execution Is Actually Contained

System claim: CI and agent command execution share a hardened production sandbox path that executes the real command while preventing host, network, secret, and persistence escape.

End-to-end proof:

- Firecracker and gVisor production `launch()` execute `JobSpec.command`.
- Exit code/stdout/stderr/timeout semantics are observable.
- Metadata/private network attempts fail.
- Host filesystem and cross-job persistence attempts fail.
- AG-D4 corpus runs through production launch path on both backends with 0 escapes.
- Settlement occurs after actual command completion, not reachability probes.

Release blocker:

- Firecracker real-job path still boots only `init=/bin/true`.
- gVisor real-job path still probes only `runsc --version`.
- Escape corpus runs through a harness-only path.

### 6. Privacy And Erasure Are System-Wide

System claim: GDPR operations are data-map-driven, holder-complete, restore-stable, and honest about residuals.

End-to-end proof:

- DSAR fan-out reaches all holders.
- Erasure destroys or suppresses personal data according to the holder's declared lever.
- Search vectors and embeddings are purged, not hidden.
- Immutable structures retain only approved pseudonymous/unrecoverable residuals.
- Certificate proves holder coverage.

Release blocker:

- Any registered holder absent from data map.
- Any embedding or backup plaintext remains recoverable after erasure.
- Any residual broader than the documented posture.

### 7. Observability Is Part Of The Pass

System claim: every survival property emits production telemetry that operators and release gates can read.

End-to-end proof:

- E2E and drill pass conditions read production metrics/traces/logs, not only test assertions.
- Trace context propagates across public gateway, internal RPC, workflow, event bus, outbox, and worker.
- Real OS signal drain changes readiness and drains within deadline.
- Every gate emits a dated scorecard row with command and artifact hash.

Release blocker:

- A drill survives but emits no production signal.
- Scorecard rows are handwritten or stale.
- Telemetry contains secrets.

## The End-To-End Campaign

### Campaign A: Production Graph Absence

Run before any expensive E2E. This is the cheapest way to prevent false confidence.

Required scanners:

- No structural credential/token/attestation verifier in production graph.
- No in-memory durable identity store in production graph.
- No mock agent runtime unless `--use-mock` is explicitly selected outside production release.
- No bare tenant-data pool handle.
- No secret-bearing unsafe derives.
- No optional required CI/KVM/DB/security jobs.

Pass condition: scanners green and red fixtures prove each scanner bites.

### Campaign B: Boundary Blackbox

Run against a full production-like cell with live DB/cache/object/bus/KMS/sandbox.

Drills:

- Forged credential/token corpus.
- Expired/revoked grant lifecycle.
- Multi-instance and crash/restart persistence.
- Destructive restore and post-restore re-erasure.
- Pooled tenant bleed.
- Secret sentinel leak corpus.
- Production sandbox command and escape corpus.

Pass condition: every denial, non-leakage, and durability property holds from the outside; required dependencies cannot skip.

### Campaign C: Chained Whole-Cell E2E

Run the four canonical whole-system scenarios from the testing strategy:

- E2E-1 PR context pane.
- E2E-2 CI failure to triage agent to issue/chat/fix PR.
- E2E-3 spec-to-ship traceability.
- E2E-4 DSAR fan-out.

M7 adjustment: these must run against the M7 production graph, not M1..M6 floors. That means real auth/token crypto, durable identity stores, real KMS adapter, real restore driver, transaction-local tenant scoping, and production sandbox execution where applicable.

Pass condition: each scenario chains real mutations across subsystems, asserts intermediate state, drives real UI where applicable, and records production telemetry.

### Campaign D: Operational Failure And Scale

Run after boundary and E2E correctness are green.

Drills:

- 1x/10x/30x mixed human/agent/service load.
- Dependency breaks for DB/cache/object/bus/search/KMS.
- Backpressure and human-lane protection.
- Queue drain, replay, outbox depth, and consumer lag.
- Real restore at cell scale with measured RPO/RTO.

Pass condition: SLOs and safety thresholds hold; failures shed or degrade according to policy; no data loss, no cross-tenant leak, no fail-open.

### Campaign E: Evidence And External Review

Run continuously, and again at release cut.

Required evidence:

- Signed/attested scorecards.
- SBOM and provenance.
- Reproducible release artifact.
- Current advisory/license/source policy scan.
- Independent crypto review.
- Independent sandbox review.
- Third-party penetration test.

Pass condition: P-546 can compute release authorization mechanically. Any missing, stale, tampered, red, or unreviewed artifact blocks release.

## Minimum Production-Representative Environment

The vetting environment must be close enough to production that the proofs mean something:

- Public gateway, internal RPC, and metrics/health surfaces all running.
- Real Postgres with RLS enabled and non-superuser app role.
- Real cache, object store, event bus, workflow workers, search/index services.
- HSM-class adapter or SoftHSM emulator for CI, with real-HSM ceremony recorded as a human blocker until complete.
- Firecracker and gVisor/runsc available on committed runner images.
- Browser automation for UI surfaces.
- OpenTelemetry collector or equivalent export sink.
- Separate tenants, regions, trust tiers, and principal types.
- Ability to kill processes, restart services, restore clean targets, and break dependencies reversibly.

Anything unavailable in this environment must be explicit in the release gate. "Skipped because unavailable" is red for production readiness.

## Release Gate Composition

P-546 should not be a dashboard. It should be a compiler for release evidence.

Inputs:

- M0..M6 scorecards re-run or truthed up against the M7 production graph.
- M7 mechanism scorecards.
- M7 blackbox drill artifacts.
- Supply-chain and provenance artifacts.
- External review and pentest findings registers.
- Human-blocker register.

Decision:

- Green only if every required artifact is present, fresh, attested, and green.
- Red if any artifact is missing, stale, tampered, skipped, red, or contradicted by a scanner.
- Red if any critical/high external finding is open without approved release-blocker resolution.
- Red if any old floor remains in the production graph.

The release gate must include one-condition-red fixtures for every input class. A release gate that cannot prove it fails is not a release gate.

## How To Think About Coverage

Coverage should be property coverage, not line coverage.

The core coverage question is: for every claim in the product and architecture, what would make it false, and which gate would catch that falsification?

Examples:

- "Tenant isolation" is false if count/ranking/backlink leaks exist, pooled state bleeds, artifacts cross trust tiers, or region writes land in the wrong cell.
- "Erasure reaches backups" is false if clean-target restore can read old plaintext or resurrect destroyed keys.
- "Agents are governed" is false if a proposed effect mutates before approval or if replay creates duplicates.
- "Sandbox is production-ready" is false if the escape corpus runs only through a special harness.
- "Release is proven" is false if scorecards can be edited by hand.

The plan is complete only when every such falsifier maps to a gate that is automated where possible and recorded as a human blocker where automation is impossible.

## Recommended Next Step

Before implementing more M7 code, create the P-546 release-gate schema early as a red-by-default skeleton. Let every M7 prompt add one generated scorecard and one release-gate input. This prevents the common failure mode where each hardening prompt emits evidence in a slightly different shape and the final gate becomes a manual reconciliation exercise.
