# Phase 7-B — The Master Prompt Ledger (the single Phase-8 run order)

> Phase: `07-prompts`. **The consolidated ledger index** — the single, totally-ordered sequence of every Myelin
> implementation prompt, top-to-bottom. This document is the reconciliation layer over the 16 per-system prompt
> files in [`by-system/`](by-system/): it interleaves all of their prompts into ONE global execution order,
> assigns each its stable global `P-<NNN>` id, and is the document Phase 8 reads top-to-bottom and runs one
> agent at a time. The template every prompt follows, the id grammar, and the interleaving rule are defined in
> [`00-ledger-overview.md`](00-ledger-overview.md) (read it first). The build order is the master sequencing
> [`../06-roadmaps/00-master-sequencing.md`](../06-roadmaps/00-master-sequencing.md) (the M0..M6 bands + the
> gate invariant). The coverage verification — every roadmap milestone → its prompt id(s) — is in
> [`coverage-matrix.md`](coverage-matrix.md). Canonical brief: [`../../VISION.md`](../../VISION.md) §7/§8
> (convert the roadmaps into ONE sequence of prompts; run them sequentially, one clean-context agent at a time,
> each committing when done). Markdown only; no commits by this document. Date: 2026-06-19.
>
> **The index is the source of truth for ORDER; the per-system files are the source of truth for CONTENT.** A
> reader executing Phase 8 reads a row here (global id, system, local id, band, title, dependencies), opens the
> named system's `by-system/<system>.md`, finds the prompt by its local id, and runs that prompt's full body
> (CANON DOCS → DELIVERABLE → CONTRACTS → GATE/DRILLS → TESTS → DEFINITION OF DONE → COMMIT). The global id is
> the stable handle; the local id is where the prompt body lives.

---

## 1. The Phase-8 execution protocol

Phase 8 runs this ledger **strictly in `P-<NNN>` order, one prompt at a time, each on a clean-context coding
agent**. The protocol, binding (VISION §8, EI-01 §2/§3/§5):

1. **One prompt, one agent, clean context.** Hand prompt `P-<NNN>` (its full body from the by-system file) to a
   coding agent that knows *only* what the prompt's CANON DOCS field names. The agent reads those docs, builds
   exactly the DELIVERABLE, implements the CONTRACTS to their frozen shape, writes the TESTS, and runs the
   GATE/DRILLS. No agent works two prompts; no prompt is split across agents.

2. **The gate invariant — the next prompt does not start until the prior prompt's gate/drill is green.** A
   prompt is **done** only when its DEFINITION OF DONE conjunction holds: the deliverable compiles in the
   workspace, the contracts are wired to the frozen shape, **every GATE/DRILLS row emits its dated green
   artifact** (PROVEN, not CLAIMED), the tests (unit + the contract CDC pair + the drill scenario) pass, the
   contract-coverage scanner and all twelve committed lints are green, any floor is named with its follow-on,
   any untested-but-named surface is honestly recorded, and **the work is committed** (header
   `P-<NNN> <BAND>: <title>`, one prompt = one commit). Only then does `P-<NNN+1>` start. This is the build-layer
   realization of the master-sequencing band invariant: *no later-band prompt runs over a red earlier gate.*

3. **A red drill is information — fix before proceeding, never weaken the gate.** If a prompt's drill comes back
   red, the agent does **not** advance and does **not** edit the threshold or invert the assertion to manufacture
   green (EI-01 §3). It records a dated "claimed, not proven" / "needs human verification" row in the thresholds
   scorecard, repairs the deliverable until the drill is genuinely green at its quantified threshold, and only
   then commits and proceeds. A red gate that blocks is an escalation, not a workaround. Because the order is
   topological, a red gate at `P-<NNN>` blocks every prompt that depends on it — fixing it is the only path
   forward.

4. **When a prompt's work reveals a needed contract shape change**, it is a whole-workspace contract PR
   (the glue crates are compile-time contract carriers, ADR-01): write down why the code must diverge, fix the
   doc, and escalate — never silently diverge from the frozen signature (EI-01 §1, code-wins-over-docs).

5. **When to insert an intermediate prompt.** If a prompt is discovered mid-build to be too large for one
   clean-context sitting (its context fills with the work itself, >~4000 tokens of agent work), or an incident
   surfaces a missing drill (EI-01: every incident adds a drill), or a floor's follow-on needs its own gateable
   unit — **append a new prompt with the next free ordinal** (`P-522`, `P-523`, …) and slot it into the run
   order by its DEPENDS-ON edges. Never renumber existing ids; the ordinal encodes assignment order, DEPENDS-ON
   encodes the authoritative ordering constraint. The compounding-payoff check (EI-01 closing): if late-band
   prompts are *harder* than early ones, the substrate under-built something — add an intermediate substrate
   prompt rather than bloating the late one.

6. **The two permanent gates ratchet across the whole run.** AG-D4 / CI-T1 (real-kernel sandbox escape = 0,
   re-run on every backend/image/kernel change) and STOR-D1 / STOR-D2 (restore-verify, re-run on every
   store-touching change) are not band-local. They appear as explicit re-confirm prompts at the bands that
   re-run them (the M2 sandbox-escape GATE, the M4 prod-image re-confirm, the M5 cell-scale restore-verify) and
   must stay green for the duration; a regression on either halts the run.

---

## 2. The global execution order (P-001 … P-521)

This single ordered list, top-to-bottom, **is the Phase-8 run order**. It is built by the
`00-ledger-overview.md` §3.2 procedure: primary sort by band (M0 → M6); within a band, topological order by the
cross-system dependency DAG (every prompt comes after every prompt in its DEPENDS-ON); ties broken by (a) the
order-by-non-negotiability tier (the harness, the data-loss/outbox floor, the sandbox-escape gate, the lints,
then Identity, then Tenancy), (b) the critical-path spine before its branches, (c) lower system index. The
DEPENDS-ON column shows the **resolved global ids** of the prompts that must be merged before this one starts
(the pervasive "the M0 substrate prompts" prose dependency resolves to the workspace-skeleton root P-001 and is
satisfied automatically by the band sort; `—` means no in-ledger prerequisite). Every prompt in every
by-system file appears here exactly once.

**Band boundaries:**

| Band | Prompts | Global id range | What the band builds |
|---|---|---|---|
| M0 | 53 | P-001 – P-053 | Substrate, harness, the committed gates |
| M1 | 72 | P-054 – P-125 | Identity + storage durability + tenancy (the dependency root + data-loss floor) |
| M2 | 120 | P-126 – P-245 | The reactive shared layer (refs, search, notif, workflow, agents) + the sandbox-escape GATE |
| M3 | 73 | P-246 – P-318 | The producer subsystems (Git hosting + Knowledge platform) |
| M4 | 101 | P-319 – P-419 | The consumer subsystems (CI + Issues + Chat) |
| M5 | 86 | P-420 – P-505 | World-scale hardening + floor follow-ons + the whole-system E2E wedge |
| M6 | 16 | P-506 – P-521 | Dogfooding: Myelin hosts itself |

| Global | System | Local id | Band | Title | Depends-on (global) |
|---|---|---|---|---|---|
| P-001 | substrate | P-S01 | M0 | Stand up the Cargo workspace and the eight glue-crate skeletons | — |
| P-002 | substrate | P-S02 | M0 | The 1×/10×/30× load generator with mixed principal kinds | P-001 |
| P-003 | substrate | P-S03 | M0 | The scoped-reversible dependency-break injector | P-002 |
| P-004 | substrate | P-S04 | M0 | The telemetry-assertion library, the every-incident-adds-a-drill loop, and the harness self-test | P-002, P-003 |
| P-005 | substrate | P-S05 | M0 | The canonical `EventEnvelope` (the names/units anchor, X-5) | P-001 |
| P-006 | substrate | P-S06 | M0 | `OutboxTx::emit(draft, cause)`: causality correct-by-construction, no `publish_now` | P-005 |
| P-007 | storage | P-ST-01 | M0 | OLTP tier client: harness pool + (tenant,region) RLS guard | P-001, P-006 |
| P-008 | substrate | P-S07 | M0 | The `outbox` table and the relay (SUB-D1, BUS-D4 — the silent-data-loss floor) | P-003, P-004, P-006 |
| P-009 | substrate | P-S08 | M0 | The idempotent event-consumer template and the dedup ledger (SUB-D2) | P-008 |
| P-010 | substrate | P-S12 | M0 | `serve(AppSpec)`: the boot → migrate → relay → consumers → drain lifecycle | P-004, P-008, P-009 |
| P-011 | event-bus | EB-01 | M0 | Freeze the EventEnvelope struct (the names/units anchor) | — |
| P-012 | event-bus | EB-03 | M0 | The transactional outbox table + the OutboxTx::emit same-tx API (per-aggregate ordering correctness) | P-011 |
| P-013 | event-bus | EB-04 | M0 | The FOR UPDATE SKIP LOCKED relay + the BusTransport trait (no-ghost / no-loss delivery) | P-012 |
| P-014 | event-bus | EB-11 | M0 | The Bus survival signals into the telemetry contract + the failure-injection harness self-test | P-012, P-013 |
| P-015 | event-bus | EB-06 | M0 | The consumer_dedup ledger (the effectively-once anchor) | P-011 |
| P-016 | storage | P-ST-02 | M0 | Outbox co-location in the OLTP database + the in-same-transaction co-commit | P-003, P-004, P-007 |
| P-017 | substrate | P-S10 | M0 | The four load-bearing architecture lints (`tenant-predicate`, `no-raw-publish`, `no-host-exec`, `no-untagged-personal-data`), each with red + green fixtures | P-001, P-006, P-009 |
| P-018 | substrate | P-S11 | M0 | The remaining eight architecture lints, each with red + green fixtures | P-001, P-017 |
| P-019 | event-bus | EB-07 | M0 | The no-raw-publish lint (red + green fixtures, wired into CI) | P-012 |
| P-020 | storage | P-ST-04 | M0 | The two storage lints (forward-only-migration + residency-pin) with red+green fixtures | P-005, P-007 |
| P-021 | search | SRCH-P01 | M0 | Ship the search-requires-acl-filter lint (red+green fixtures) + anchor the index-doc names | — |
| P-022 | identity | P-ID-01 | M0 | Freeze the myelin-identity contract surface: the eleven trait signatures + SetExpr + CaveatContext | — |
| P-023 | identity | P-ID-02 | M0 | Register the iam.* event tokens + their EventEnvelope projections (the erasure-vs-immutability envelope split) | P-022 |
| P-024 | identity | P-ID-03 | M0 | Wire the four Id-relevant architecture lints with red + green fixtures | P-022, P-023 |
| P-025 | tenancy | P-CP-01 | M0 | The myelin-tenancy partition-key types: TenantId / Region / ResidencyTag | — |
| P-026 | tenancy | P-CP-03 | M0 | The residency-pin lint with red+green fixtures | P-025 |
| P-027 | tenancy | P-CP-02 | M0 | The frozen-not-live CrossCellPointer frame (the four-field PII-free bridge frame) | P-025 |
| P-028 | tenancy | P-CP-04 | M0 | The control-plane-pii-free lint with red+green fixtures | P-027 |
| P-029 | substrate | P-S09 | M0 | The schema-evolution upcaster registry (forward-only) | P-009 |
| P-030 | substrate | P-S13 | M0 | The three-surface topology + tenant-from-token (SUB-D7, cross-tenant IDOR) | P-003, P-004, P-010, P-017 |
| P-031 | substrate | P-S14 | M0 | Liveness ≠ readiness on the metrics-health surface (SUB-D9) | P-003, P-004, P-030 |
| P-032 | substrate | P-S15 | M0 | `PersonalDataHolder` auto-registration + the forward-only migration runner | P-010, P-018 |
| P-033 | substrate | P-S16 | M0 | The shared resilient inter-service client: timeout + breaker + bulkhead + jittered retry | P-001 |
| P-034 | substrate | P-S17 | M0 | `Retry-After` honouring on the resilient client (SUB-D5, no retry-storm amplification) | P-003, P-004, P-033 |
| P-035 | substrate | P-S19 | M0 | The protected-human-lane shed order and bounded-everything | P-030, P-034 |
| P-036 | substrate | P-S20 | M0 | Agent-generated-load caps + the causal-loop guard (SUB-D8) | P-003, P-004, P-006, P-035 |
| P-037 | substrate | P-S21 | M0 | The contract-coverage scanner (the meta-gate) | P-008, P-009, P-034 |
| P-038 | substrate | P-S22 | M0 | The versioned thresholds file (every Q32 default-to-beat) | P-004 |
| P-039 | substrate | P-S24 | M0 | Green the M0 exit gate: SUB-D1/D2/BUS-D4/D5/D7/D8/D9 + all twelve lints + the harness self-test | P-004, P-008, P-009, P-017, P-018, P-030, P-031, P-034, P-036, P-037, P-038 |
| P-040 | substrate | P-S18 | M0 | The fail-static mechanism (`FailStatic<T>`, bounded-staleness, never fail open) | P-001, P-038 |
| P-041 | substrate | P-S23 | M0 | The shared overlay/state primitives (the design-system bug-class floor) | P-001 |
| P-042 | event-bus | EB-02 | M0 | The taxonomy grammar validator + the seed token table + the three new check-seam/initiative tokens | P-011 |
| P-043 | event-bus | EB-05 | M0 | The idempotent-consumer template (the EventHandler the whole platform is built from) | P-011, P-013 |
| P-044 | event-bus | EB-08 | M0 | The no-cross-sync-cycle lint (red + green fixtures, wired into CI) | P-012, P-043 |
| P-045 | event-bus | EB-09 | M0 | The Bus slice of the tenant-predicate-on-streams lint (red + green fixtures, wired into CI) | P-043 |
| P-046 | event-bus | EB-10 | M0 | The schema-evolution upcaster seam (forward-only; un-upcastable → DLQ) | P-011, P-043 |
| P-047 | storage | P-ST-03 | M0 | BlobStore: the content-addressed trait + the fs-backed floor (BLAKE3 hash-on-write + re-hash-on-read integrity) | P-001, P-006, P-007 |
| P-048 | storage | P-ST-05 | M0 | The forward-only online migration runner (expand→backfill→contract) | P-006, P-007, P-020 |
| P-049 | gdpr | P-GA-01 | M0 | The myelin-gdpr glue-crate skeleton: the PersonalDataHolder trait signature + the core types | — |
| P-050 | gdpr | P-GA-02 | M0 | The #[personal_data] derive-attribute names + the five-tag enum names (frozen so consumers compile) | P-049 |
| P-051 | gdpr | P-GA-03 | M0 | The no-untagged-personal-data lint (with red+green fixtures), committed as a CI gate | P-050 |
| P-052 | refs | REF-P1 | M0 | Ship the myelin-refs glue crate: the ArtifactRef value type + the Issues key + the frozen #sub grammar | — |
| P-053 | refs | REF-P2 | M0 | Wire the four Refs lints into CI with red+green fixtures (the M0 ratchet) | P-052 |
| P-054 | identity | P-ID-04 | M1 | Identity service shell: the AppSpec the harness wires (boot → migrate → relay → consumers → three ports) | P-022 |
| P-055 | gdpr | P-GA-04 | M1 | The PersonalDataHolder auto-registration hook (contract 1.4) + the holder-registered architecture test | P-049 |
| P-056 | substrate | P-S26 | M1 | The restore-verify cross-seam half (SUB-D6, the silent-data-loss floor, with Storage) | P-003, P-004, P-039 |
| P-057 | identity | P-ID-08 | M1 | The S3 ReBAC tuple store + write_tuples/zookie (the only emit path, via the outbox) | P-022, P-023, P-054 |
| P-058 | storage | P-ST-06 | M1 | The three-level KMS hierarchy + fail-static availability posture | P-007, P-047 |
| P-059 | storage | P-ST-11 | M1 | Continuous WAL archiving + base backups + PITR (the RPO floor) | P-016, P-047, P-058 |
| P-060 | storage | P-ST-12 | M1 | restore(to_offset T) to the cross-seam consistency point + reindex-from-source rebuild | P-016, P-047, P-058, P-059 |
| P-061 | storage | P-ST-13 | M1 | THE HEADLINE: the CI-wired restore-verify gate (STOR-D1, the permanent gate) | P-047, P-059, P-060 |
| P-062 | gdpr | P-GA-19 | M1 | The tamper-evident audit log core: outbox-only audit consumer + per-tenant hash-chain + Merkle leaves + minimisation | — |
| P-063 | git | GIT-P3 | M1 | Declare the git PersonalDataHolder H1 intent + apply the #[personal_data] tags (no-untagged lint green) | — |
| P-064 | identity | P-ID-05 | M1 | The S1 principal store: RLS-partitioned, per-tenant/per-subject DEK, PII-tagged, holder-registered | P-024, P-054 |
| P-065 | identity | P-ID-06 | M1 | authenticate: the v1 human/SSO credential set (OIDC, SAML, SCIM, passkey, SSH) | P-022, P-064 |
| P-066 | identity | P-ID-07 | M1 | authenticate: the capability-token + machine-identity credential set (PAT/CI/agent + deploy-key/per-job) | P-064, P-065 |
| P-067 | identity | P-ID-09 | M1 | check: the depth-bounded Zanzibar userset-rewrite evaluation (fail-closed, zookie-snapshot) | P-022, P-054, P-057 |
| P-068 | identity | P-ID-10 | M1 | The ReBAC namespace engine + the fragment-admit contract + the org/team/project core hierarchy | P-057, P-067 |
| P-069 | identity | P-ID-11 | M1 | list_objects: the return-shape dispatch + the S4 Ids materialise path + S8 reverse index | P-022, P-057, P-068 |
| P-070 | identity | P-ID-12 | M1 | list_objects: the SetExpr no-N+1/no-post-filter lowering + the S8 watermark consistency path (ID-D4, ID-D7) | P-057, P-069 |
| P-071 | identity | P-ID-13 | M1 | list_subjects + explain: the SubjectTree/RewriteTrace served by S8 at 50k-member density | P-068, P-069, P-070 |
| P-072 | identity | P-ID-14 | M1 | The revocation list (S7) + idempotent revoke + the SCIM-disable revocation path (ID-D1) | P-065, P-066, P-067 |
| P-073 | identity | P-ID-15 | M1 | The fail-static cache (S6) + the 4.11 staleness bound + the zookie-bypass: ID-D2 | P-067, P-070, P-072 |
| P-074 | identity | P-ID-16 | M1 | The authz read-replica (S5): the ID-4 first scaling move | P-057, P-064, P-069 |
| P-075 | identity | P-ID-17 | M1 | delegation: the monotone-intersection algebra (ID-D5) | P-066, P-067 |
| P-076 | identity | P-ID-18 | M1 | mint_run_token: per-run attenuated tokens + self-hosted scope + mid-resume re-mint (ID-D6) | P-066, P-072, P-075 |
| P-077 | identity | P-ID-19 | M1 | The pseudonym map (S2) + the frozen pseudonym grammar | P-064 |
| P-078 | identity | P-ID-20 | M1 | resolve_pseudonym + erase: the per-subject crypto-shred lever (ID-D8) | P-057, P-077 |
| P-079 | identity | P-ID-21 | M1 | Reserved: the M1 → M2 Id exit-gate scorecard (the cross-tenant/fail-static/disabled-user go/no-go) | P-068, P-070, P-072, P-073, P-078 |
| P-080 | tenancy | P-CP-05 | M1 | The three PII-free control-plane registry tables + the local_tenant directory + the HARD placement invariant | P-007, P-025, P-026, P-028 |
| P-081 | tenancy | P-CP-06 | M1 | discover(slug \| tenant_id): PII-free routing, off the hot path, client-cacheable fail-static | P-080 |
| P-082 | tenancy | P-CP-07 | M1 | place(region, requested_tier) + two-phase signup (PII born inside the cell) | P-028, P-080 |
| P-083 | tenancy | P-CP-11 | M1 | Cell-provisioning gating: restore-verify + readiness before a cell goes active (CP-D6) + scripted-provisioning floor | P-020, P-058, P-080, P-082 |
| P-084 | tenancy | P-CP-08 | M1 | placement_of(tenant_id): the routing answer + the gateway misroute-rejection (CP-D2 tenant-grain) | P-080, P-081, P-082 |
| P-085 | tenancy | P-CP-09 | M1 | residency_verify over the M1 store set (the no-global-pool signed attestation) | P-007, P-016, P-020, P-080, P-082 |
| P-086 | tenancy | P-CP-10 | M1 | The Pool isolation tier (the v1 floor) + Bridge/Dedicated declared-on-demand | P-007, P-080, P-082 |
| P-087 | substrate | P-S25 | M1 | Prove fail-static against a real Identity hiccup (SUB-D4) | P-039, P-040 |
| P-088 | substrate | P-S27 | M1 | The exhaustive `PersonalDataHolder` (H1–H18) confirmation | P-018, P-032 |
| P-089 | event-bus | EB-12 | M1 | Partition the streams under the (tenant, region) key | P-013, P-043 |
| P-090 | event-bus | EB-13 | M1 | Residency-pin the Bus streams (no cross-region read path) | P-089 |
| P-091 | event-bus | EB-14 | M1 | Pin the cross-cell bridge FRAME (CrossCellPointer; designed-not-built) | P-089 |
| P-092 | event-bus | EB-15 | M1 | Register the Bus as a PersonalDataHolder + wire inline-PII crypto-shred to the KMS hierarchy | P-011, P-012 |
| P-093 | event-bus | EB-16 | M1 | Hook the erasure ledger for post-restore re-erasure (the key stays destroyed across a restore) | P-092 |
| P-094 | storage | P-ST-07 | M1 | The KeyOrigin trait (platform-managed \| BYOK \| HYOK) + the structural HYOK enforcement | P-058 |
| P-095 | storage | P-ST-08 | M1 | OLTP + blob envelope encryption wired + classify-driven per-subject/per-tenant key choice | P-007, P-047, P-058, P-094 |
| P-096 | tenancy | P-CP-12 | M1 | The four-layer region-pinning enforced end-to-end (CP-D3 + STOR-D5 + CP-D2 e2e) — the M1→M2 go/no-go | P-026, P-058, P-080, P-084, P-085, P-094 |
| P-097 | tenancy | P-CP-13 | M1 | Self-host parity: the degenerate one-cell control plane runs the identical code path | P-081, P-084, P-096 |
| P-098 | tenancy | P-CP-14 | M1 | The CP-outage blast-radius win (CP-D4): already-placed tenants keep serving, only signup degrades | P-081, P-084, P-096 |
| P-099 | storage | P-ST-09 | M1 | The crypto-shred erase(subject,tenant) six-step algorithm | P-058, P-095 |
| P-100 | storage | P-ST-14 | M1 | Post-restore re-erasure (STOR-D3) + the cell-kill RTO drill (STOR-D2) | P-059, P-060, P-061, P-099 |
| P-101 | storage | P-ST-10 | M1 | GD-4 granularity wiring + the structural GDPR floor (by reference to X-7) | P-095, P-099 |
| P-102 | storage | P-ST-15 | M1 | Residency pinning enforced end-to-end (STOR-D5) | P-007, P-020 |
| P-103 | storage | P-ST-16 | M1 | The reserve/settle cost gate mechanism + the durable per-tenant ledger | P-007 |
| P-104 | storage | P-ST-17 | M1 | The OLAP read store frame (holder + CQRS-fed-by-the-bus contract shape) | P-007, P-058 |
| P-105 | gdpr | P-GA-05 | M1 | The PersonalDataHolder trait bodies + the GDPR-owned holders (H18 own stores + H16 audit carve-out) | P-049, P-055 |
| P-106 | gdpr | P-GA-06 | M1 | The upstream-store holder orchestration (H6/H8/H9/H10/H14/H15) + the canonical erase order + resumable receipts | P-105 |
| P-107 | gdpr | P-GA-07 | M1 | The #[personal_data] classify-derive macro body + the five-tag enum, applied across every M1 store | P-050, P-051 |
| P-108 | gdpr | P-GA-08 | M1 | The SpecialCategory → DPIA router (the data-map-diff marker into the DPIA gate) | P-107 |
| P-109 | gdpr | P-GA-09 | M1 | The data-map / RoPA generator (walk every schema + registered holder → the machine-readable inventory) | P-055, P-107 |
| P-110 | gdpr | P-GA-10 | M1 | The CI data-map diff gate + the DPIA-route on reclassification | P-108, P-109 |
| P-111 | gdpr | P-GA-11 | M1 | The DSR orchestrator API + the state machine + the controller/processor posture gate | P-109 |
| P-112 | gdpr | P-GA-12 | M1 | The data-map-driven per-holder checklist + the resumable fan-out + verifiable receipts + the legal-hold gate | P-105, P-106, P-109, P-111 |
| P-113 | gdpr | P-GA-13 | M1 | DSR tenant-operability: Art. 28 tenant-facing DSR + tenant offboarding (EraseScope::Tenant) + restrict/rectify/portability surfaces | P-111, P-112 |
| P-114 | gdpr | P-GA-14 | M1 | The M1 DSR erasure floor proof: data-map-driven fan-out → 0 recoverable + worker-kill resumability (the coarse-deadline floor) | P-105, P-106, P-112 |
| P-115 | gdpr | P-GA-15 | M1 | The erasure ledger (PII-free, non-shred-erasable) + post-restore re-erasure + the crypto-shred-reaches-backups / restore-resurrects-nothing proof | P-106, P-112, P-114 |
| P-116 | gdpr | P-GA-16 | M1 | The ONE free-text/immutable erasure posture (X-7) written once as the single canonical artifact | P-105 |
| P-117 | gdpr | P-GA-17 | M1 | The structural erasure floor proven on the M1 stores (per-subject DEK shred + pseudonym-map shred + restrict suppression) | P-105, P-106, P-112, P-116 |
| P-118 | gdpr | P-GA-18 | M1 | The pseudonymous-by-default commit-identity prerequisite recorded for Git M3 (X-7's critical-path obligation) | P-116 |
| P-119 | gdpr | P-GA-20 | M1 | The audit CT-style proofs + signed tree heads + independent-witness anchoring + DSR-receipt seal + the H16 carve-out body (GA-D3) | P-062, P-105, P-112 |
| P-120 | refs | REF-P3 | M1 | Register Refs as a PersonalDataHolder (stub surface) + confirm residency-pin | P-052 |
| P-121 | refs | REF-P4 | M1 | Pin the per-tenant DEK for the edge index + R2 cache into the KMS hierarchy | P-120 |
| P-122 | search | SRCH-P02 | M1 | Register Search as a PersonalDataHolder + pin the per-tenant index DEK + confirm residency | P-021 |
| P-123 | git | GIT-P1 | M1 | Freeze the Git ReBAC namespace fragment so Identity's cell schema compiles | — |
| P-124 | git | GIT-P2 | M1 | Register the git.* event tokens in the Bus taxonomy seed | — |
| P-125 | issues | ISS-P01 | M1 | Freeze the Issues ReBAC fragment + the worklog PersonalDataHolder tags (so dependents compile) | — |
| P-126 | storage | P-ST-21 | M2 | Online-migration safety on the restored prod-scale copy (STOR-D8) | P-048, P-060, P-061 |
| P-127 | notif | NOTIF-P1 | M2 | Stand up myelin-notif: the serve(AppSpec) service shell + three ports + the glue-crate contract carriers | — |
| P-128 | chat | CHAT-P1 | M2 | Declare the chat.* event taxonomy (durable-via-outbox vs firehose-only) + freeze the Bus token grammar | — |
| P-129 | ci | CI-P1 | M2 | The JobSpec struct + the SandboxBackend / FleetProvider trait seam + the four-uniform-guarantee wiring hooks | — |
| P-130 | agent | AG-P1 | M2 | Ship the myelin-agent glue crate: the six-trait contract surface + the value enums + the no-llm-in-platform lint | — |
| P-131 | agent | AG-P2 | M2 | The agent data model: run/tool_def/proposed_effect/hitl_gate/trace migrations, (tenant, region)-first + RLS + the tenant-predicate lint | P-130 |
| P-132 | agent | AG-P3 | M2 | The PersonalDataHolder registration seam + the no-untagged-personal-data lint over the agent stores | P-131 |
| P-133 | identity | P-ID-22 | M2 | Promote the CaveatContext evaluator from the literal-only floor to the full QueryAst predicate core | P-067 |
| P-134 | identity | P-ID-23 | M2 | The watcher relation + list_subjects fanout + the M2-consumer correctness re-confirm (ID-D5 re-run; SRCH/REF/NOTIF rides) | P-071, P-075, P-076 |
| P-135 | substrate | P-S28 | M2 | The firehose per-connection in-flight frame caps + slow-consumer drop to `resync_required` | P-035, P-056 |
| P-136 | substrate | P-S29 | M2 | The firehose scope-bounded selector + the per-surface frame shed budgets (D-11 substrate half complete) | P-035, P-135 |
| P-137 | event-bus | EB-17 | M2 | The EventMatcher = the frozen myelin-query QueryAst (bounded, permission-aware interpreter) | P-011 |
| P-138 | event-bus | EB-18 | M2 | Signal curation (match / severity-rank / dedup-window / auto-resolve / publish) | P-043, P-137 |
| P-139 | event-bus | EB-19 | M2 | Automations (the stateless per-event reflex over the matcher) | P-043, P-137 |
| P-140 | event-bus | EB-20 | M2 | Triggers (the stateful per-person promise; fire-once-per-arming guarded UPDATE) | P-043, P-137 |
| P-141 | event-bus | EB-21 | M2 | The firehose resume-cursor subscription protocol (built FIRST) | P-013, P-043 |
| P-142 | event-bus | EB-22 | M2 | The reindex-from-source seam + the *.snapshot event schema (cold == live) | P-012, P-141 |
| P-143 | event-bus | EB-23 | M2 | The reactive/dispatch tier (nested causality + structural loop guards + reserve/settle) | P-138, P-141, P-142 |
| P-144 | event-bus | EB-24 | M2 | The check-seam carriage (ci.check.updated per-aggregate ordering + the ci.result wait_for_signal substrate) | P-042, P-141, P-143 |
| P-145 | storage | P-ST-18 | M2 | OLAP read store fed by the bus (reindex-from-source only) | P-104 |
| P-146 | storage | P-ST-19 | M2 | Reserve/settle fronts agent runs (never interrupt in-flight) | P-103 |
| P-147 | storage | P-ST-20 | M2 | The T3 firehose-archive seam (sealing + per-tenant DEK segments, on a non-CI firehose) | P-047, P-095 |
| P-148 | gdpr | P-GA-21 | M2 | The DSR deadline durable timer + the nearing-deadline warning Signal (on the myelin-flow wheel) | P-111, P-114 |
| P-149 | gdpr | P-GA-22 | M2 | The retention engine: tightest-policy-wins merge + legal-hold-aware suspend-don't-delete (GA-D6) | P-105, P-112 |
| P-150 | gdpr | P-GA-23 | M2 | The consent registry + the sub-processor registry + the transfer_allowed gate (deny extra-EU by default) | P-105, P-149 |
| P-151 | gdpr | P-GA-24 | M2 | The per-derivative erasure fan-out: Search purge+reindex (incl. embeddings) + Refs tombstone + reindex-from-source rectification (GA-D2) | P-112 |
| P-152 | gdpr | P-GA-25 | M2 | restrict suppression into the derived stores (Search/Refs/Notif/Agents/OLAP) — GA-D7 | P-117, P-151 |
| P-153 | gdpr | P-GA-26 | M2 | eDiscovery export + the agent-trace holder seam (8.8) + the history-rewrite resumable-activity skeleton | P-119, P-149 |
| P-154 | refs | REF-P5 | M2 | The edge inverse-index schema migration | P-052, P-120, P-121 |
| P-155 | refs | REF-P6 | M2 | The refs-edge-builder consumer (steady-state == cold-rebuild, idempotent) | P-154 |
| P-156 | refs | REF-P7 | M2 | The refs-projection-invalidator consumer + the no-op cache shim | P-155 |
| P-157 | refs | REF-P8 | M2 | The edge-extraction emit seam (one edge per structured node, emit-iff-committed) | P-155 |
| P-158 | refs | REF-P9 | M2 | The loop-guard causal-depth stamp on every refs.edge.* emit | P-157 |
| P-159 | refs | REF-P10 | M2 | The per-viewer resolution chokepoint (denied -> tombstone, never leak) | P-052, P-154, P-156 |
| P-160 | refs | REF-P11 | M2 | The permission-filtered backlink read: lower the SetExpr ACL filter over source_root (the crux) | P-154, P-159 |
| P-161 | refs | REF-P12 | M2 | The R2 projection cache (bounded, invalidatable holder; replaces the REF-P7 shim) | P-156, P-159 |
| P-162 | refs | REF-P13 | M2 | The bounded cycle-safe recursive-CTE traverse (depth-16, branch-prune) | P-154, P-160 |
| P-163 | refs | REF-P14 | M2 | The TE-7 typed-edge mirror discipline (vocabulary + inverse pairing, synthetic events) | P-155, P-162 |
| P-164 | refs | REF-P15 | M2 | The unified 4-step #sub tombstone ladder (root always carried) + the structural erasure holder | P-052, P-156, P-159 |
| P-165 | refs | REF-P16 | M2 | Reindex-from-source: rebuild byte-parity (the recovery path, one code path, no backdoor) | P-155, P-163 |
| P-166 | search | SRCH-P03 | M2 | The service shell: boot from serve(AppSpec) + the encrypted-from-birth per-tenant index layout | P-021, P-122 |
| P-167 | search | SRCH-P04 | M2 | The IndexBackend trait + Tantivy + the full-text-inverted and structured/columnar shapes | P-166 |
| P-168 | search | SRCH-P05 | M2 | The vector HNSW index shape (incremental insert, soft-delete-then-compact, one doc-id space) | P-167 |
| P-169 | search | SRCH-P06 | M2 | The near-real-time incremental indexer: the bus consumer (projection-fed, idempotent) | P-167, P-168 |
| P-170 | search | SRCH-P07 | M2 | The query-AST compiler: one compile target of the frozen QueryAst (+ read-time rollup/formula inputs) | P-167, P-168 |
| P-171 | search | SRCH-P08 | M2 | The permission-aware query pipeline: conjoin the ACL filter + the Ids/All/None lowering + cross-tenant 0 (SRCH-D3) | P-167, P-169, P-170 |
| P-172 | search | SRCH-P09 | M2 | The SetExpr reverse-index JOIN (InRelation/TupleSet + boolean composition) + the zero-escape leak drill (SRCH-D1) | P-171 |
| P-173 | search | SRCH-P10 | M2 | The zookie/consistency path: no-stale-grant + fail-static bypass (the consistency mechanism) | P-171, P-172 |
| P-174 | search | SRCH-P11 | M2 | Hybrid + vector: RRF fusion + filter-during-traversal (k visible neighbours, the SRCH-D1 vector/RAG half) | P-168, P-171 |
| P-175 | search | SRCH-P12 | M2 | Multilingual analysis: the per-language analyzer chain (EU + CJK + code tokenizer) | P-169 |
| P-176 | search | SRCH-P13 | M2 | The caches: the S5 list_objects filter cache + the hot-query result cache (zookie-bucketed, TTL <= revocation SLA) | P-171, P-172 |
| P-177 | search | SRCH-P14 | M2 | The telemetry contract: the full §4.11 signal set on the metrics-health port (observability is part of the pass) | P-169, P-171 |
| P-178 | search | SRCH-P15 | M2 | Erasure as a real holder: purge + reindex (vectors compacted) + restrict suppression + the HYOK structural skip | P-122, P-168 |
| P-179 | search | SRCH-P16 | M2 | Reindex-from-source: the ONLY rebuild path (bus re-emit -> live indexer, no Postgres backdoor) + SRCH-D5 CI parity | P-169, P-178 |
| P-180 | notif | NOTIF-P2 | M2 | The Notif data model: the nine tenant-partitioned tables (refs-not-strings, dedup UNIQUE, one state column) | P-127 |
| P-181 | notif | NOTIF-P3 | M2 | The Signal-consumer router skeleton (EventHandler, whitelist-never-*, UPSERT, outbox-only emit) + NOTIF-D10 | P-127, P-180 |
| P-182 | notif | NOTIF-P4 | M2 | Register Notif as a PersonalDataHolder (references-not-payloads tombstone-for-free; the holder half of 7.7) | P-180 |
| P-183 | notif | NOTIF-P5 | M2 | list_inbox (the ONE inbox) + the scoped-view filter grammar (the C-9 invariant) + CLI list/show | P-180, P-181 |
| P-184 | notif | NOTIF-P6 | M2 | Read-state: mark / snooze / mark_all_read (the one read-state truth) + CLI read | P-180, P-183 |
| P-185 | notif | NOTIF-P7 | M2 | The deterministic explainable ranking function (priority 0..100, the reason→base→class table, explain-trace) + NOTIF-D1 | P-183 |
| P-186 | notif | NOTIF-P8 | M2 | define_notif_rule (the registration seam) + the stubbed Notif-owned default reason set | P-181, P-185 |
| P-187 | notif | NOTIF-P9 | M2 | humanise (the ONE templating surface, per-viewer-safe) + the template store + NOTIF-D4 (0 title/PII leak) | P-183 |
| P-188 | notif | NOTIF-P10 | M2 | prefs / quiet-hours over the frozen QueryAst (pierce_classes; recipient-tz evaluation) + CLI prefs | P-180, P-181 |
| P-189 | notif | NOTIF-P11 | M2 | The five write-time storm-control mechanisms (suppresses delivery/ranking, never the audit) + NOTIF-D2 | P-180, P-181 |
| P-190 | notif | NOTIF-P12 | M2 | Write-fanout for the bounded high-signal set (the frozen mention(Principal) structured node + the hot-subject cap) | P-181, P-189 |
| P-191 | notif | NOTIF-P13 | M2 | Read-fanout for the unbounded ambient set (the SetExpr watcher push-down JOIN + the zookie watermark) | P-183, P-189 |
| P-192 | notif | NOTIF-P14 | M2 | Escalation on the myelin-flow durable wheel (the frozen chain shape; ack-as-event) + NOTIF-D7 + NOTIF-D8 | P-181, P-188 |
| P-193 | notif | NOTIF-P15 | M2 | The inbox watch live transport (the frozen firehose resume-cursor protocol) + the D-N11 resume leg | P-181, P-183 |
| P-194 | notif | NOTIF-P16 | M2 | The delivery fabric (the idempotent DeliveryAdapter trait + the deterministic mock; in-app stays in-cell) + NOTIF-D9 | P-181, P-187 |
| P-195 | notif | NOTIF-P17 | M2 | reindex-from-source (the only recovery path; cold == live; the replay half of 7.7) + NOTIF-D3 | P-181, P-182, P-194 |
| P-196 | notif | NOTIF-P18 | M2 | Snooze re-surfacing on the same myelin-flow durable timer wheel (one substrate, three uses) | P-184, P-192 |
| P-197 | workflow | P-FLOW-01 | M2 | myelin-flow crate + the six-table data model (forward-only migrations) | — |
| P-198 | workflow | P-FLOW-02 | M2 | The myelin-flow AppSpec service shell (boot + migrate + outbox relay + empty consumer slot) | P-197 |
| P-199 | workflow | P-FLOW-04 | M2 | WfCtx core: activity + now + rand + emit, with the journal/outbox co-commit (FLOW-D5) | P-197, P-198 |
| P-200 | workflow | P-FLOW-08 | M2 | The flow-determinism lint red+green fixtures (proves the lint rejects and admits) | P-199 |
| P-201 | workflow | P-FLOW-03 | M2 | PersonalDataHolder auto-registration over workflow_run/wf_history/wf_signal (structural half) | P-197, P-198 |
| P-202 | workflow | P-FLOW-05 | M2 | Deterministic replay/recovery + lease-based dispatch + crash recovery (FLOW-D1) | P-199 |
| P-203 | workflow | P-FLOW-06 | M2 | DurableExecutor start/describe/cancel + the engine telemetry set | P-202 |
| P-204 | workflow | P-FLOW-07 | M2 | The replay-divergence guard (halt-as-nondeterministic + dead-letter) (FLOW-D2) | P-202 |
| P-205 | workflow | P-FLOW-09 | M2 | Durable signals: DurableExecutor::signal + wf_signal idempotency-by-construction | P-200, P-203 |
| P-206 | workflow | P-FLOW-10 | M2 | The per-effect idem_key rule for batch / partial HITL approval (single vs multi-effect) | P-205 |
| P-207 | workflow | P-FLOW-13 | M2 | Durable timers: the minute-bucket wheel + sleep_until/sleep_for (FLOW-D3 floor) | P-200, P-202 |
| P-208 | workflow | P-FLOW-11 | M2 | WfCtx wait_for_signal + the multi-day HITL approval-card round-trip (FLOW-D4) | P-206, P-207 |
| P-209 | workflow | P-FLOW-12 | M2 | F-4 extended: the per-effect partial-approval drill across a restart + deploy | P-206, P-208 |
| P-210 | workflow | P-FLOW-14 | M2 | Cheap SLA-timer disarm/re-arm (row-update cost, no wheel pollution) | P-207 |
| P-211 | workflow | P-FLOW-15 | M2 | The SCHEDULE_AND_RUN_JOB long-park idiom (dispatch-and-return + park-on-job.done) | P-207, P-208 |
| P-212 | workflow | P-FLOW-16 | M2 | The reserve/settle bookend on every dispatch (FLOW-D6) | P-211 |
| P-213 | workflow | P-FLOW-17 | M2 | mint_run_token mid-workflow re-mint on resume (token life == activity life) | P-208, P-211 |
| P-214 | workflow | P-FLOW-18 | M2 | Loop safety: causal-depth ceiling + shared-root tripwire + bounded activity pool (FLOW-D7) | P-203, P-211 |
| P-215 | workflow | P-FLOW-19 | M2 | The merge-queue durable workflow body, drilled in isolation against a mock ci.result (M2 exit) | P-211, P-212, P-214 |
| P-216 | agent | AG-P4 | M2 | The SKELETON runtime: prove the gateway -> identity -> dispatch -> reserve -> trace path at zero cost | P-130, P-131, P-132 |
| P-217 | agent | AG-P5 | M2 | The MockAgentRuntime: a deterministic scripted brain on the same --use-mock code path users hit | P-216 |
| P-218 | agent | AG-P6 | M2 | Plan-then-apply EffectApi::apply: the schema -> capability -> delegation -> tenant -> budget -> HITL-gate -> apply -> meter pipeline (AG-D1/D2/D3) | P-217 |
| P-219 | agent | AG-P7 | M2 | The delegation-scoped tool-list: the list_objects SetExpr push-down + the apply-time re-check | P-218 |
| P-220 | agent | AG-P8 | M2 | The frozen requires_approval defaults seed + the run --dry-run plan lever + AG-D9 effect-sequence determinism | P-218 |
| P-221 | agent | AG-P9 | M2 | The HITL withhold -> surface -> resume loop and the hitl_gate state machine | P-218 |
| P-222 | agent | AG-P10 | M2 | Per-effect HITL idempotency (C4/OQ-F): partial approval + double-click well-defined (AG-D5 exactly-once) | P-221 |
| P-223 | agent | AG-P11 | M2 | Humanise the HITL card text + agent-authored messages through the ONE templating surface (C9/OQ-L) | P-221 |
| P-224 | agent | AG-P12 | M2 | The five structural loop guards (self-guard / reference-gate / depth-ceiling / shared-root tripwire / idempotent tools) (AG-D7) | P-218 |
| P-225 | agent | AG-P13 | M2 | Per-run identity: mint, scrub the shared token, revoke idempotently, re-mintable on resume (AG-D8 re-mint leg) | P-216, P-221 |
| P-226 | agent | AG-P15 | M2 | ToolHands::exec on the unified sandbox: the kind=agent job spec + the routing split + the four uniform guarantees | P-218, P-225 |
| P-227 | agent | AG-P14 | M2 | The reserve/settle cost gate as the runaway self-limiter (AG-D11) | P-216, P-218, P-220 |
| P-228 | agent | AG-P16 | M2 | The SCHEDULE_AND_RUN_JOB long-park idiom: dispatch-and-return, completion as a durable idempotent signal | P-225, P-226 |
| P-229 | agent | AG-P17 | M2 | The AG-D4 / CI-T1 hard escape GATE: ZERO escapes on a real kernel, green attestation or M3+ is no-go | P-226, P-228 |
| P-230 | git | GIT-P4 | M2 | Register the git #sub mints with Refs (comment-/thread-/L<a>-L<b> kinds) | P-123 |
| P-231 | git | GIT-P5 | M2 | Register git's declare_indexable code-projection spec with Search | P-123 |
| P-232 | git | GIT-P6 | M2 | Declare the X-1 CheckStatus consumer contract (the compiling, not-yet-live seam module) | P-123 |
| P-233 | git | GIT-P7 | M2 | The design-system pass + the X-1 affordances, with fork-trust UX human sign-off | P-232 |
| P-234 | knowledge | KN-P01 | M2 | Freeze myelin-content (the v1 block + inline taxonomy) and compile the WASM render path (KN-D2) | — |
| P-235 | knowledge | KN-P02 | M2 | Freeze myelin-query (FieldType/ViewSpec/QueryAst = the EventMatcher core) + the ADF lossy-map (13.2) | P-234 |
| P-236 | knowledge | KN-P03 | M2 | Freeze the order_key/LexoRank fractional-index encoding + the X-3 conformance vector (byte-identical with Issues) | P-235 |
| P-237 | ci | CI-P2 | M2 | The Firecracker default backend + the backend-independent mandatory hardening profile + the hardened-boot self-test | P-129 |
| P-238 | ci | CI-P3 | M2 | The runner agent + the lease/heartbeat handshake + the exactly-once job.done terminal report | P-129, P-237 |
| P-239 | ci | CI-P5 | M2 | The escape-drill adversarial corpus + the green-attestation format + the AG-D4 / CI-T1 hard GATE | P-237, P-238 |
| P-240 | ci | CI-P4 | M2 | Pre-warmed microVM snapshot pools + the self-hosted runner attestation gate + the tenant-scoped token mint | P-237, P-238 |
| P-241 | issues | ISS-P02 | M2 | Co-own myelin-query byte-identical with Knowledge (the field-type enum / ViewSpec / QueryAst / order_key codec) | P-125 |
| P-242 | issues | ISS-P03 | M2 | Register the complete issue.* event taxonomy + the initiative token (under the Bus grammar) | P-125 |
| P-243 | issues | ISS-P04 | M2 | Declare the Issues IndexSpec (declare_indexable) + the define_notif_rule reason set | P-242 |
| P-244 | chat | CHAT-P2 | M2 | Declare the Chat ReBAC fragment (channel.read + watcher) + freeze the #sub grammar (message-/thread-) | P-128 |
| P-245 | chat | CHAT-P3 | M2 | Register the humanise keys + the define_notif_rule set + the fanout-class, validate the firehose scope, pin the TE-21 language call | P-128, P-244 |
| P-246 | event-bus | EB-26 | M3 | Per-subsystem token-list validation harness + the check-seam consumer leg + per-owner replay carriage (M3) | P-141, P-144 |
| P-247 | identity | P-ID-24 | M3 | The Git ReBAC namespace fragment (ref-glob + CODEOWNERS + protected_push + approve_untrusted_ci): GIT-D8/D11 authz side | P-068, P-070 |
| P-248 | identity | P-ID-25 | M3 | Git pseudonymous-by-default commits: consuming the 4.8 grammar (GIT-D2) | P-077, P-078, P-247 |
| P-249 | identity | P-ID-26 | M3 | The Knowledge ReBAC namespace fragment (page-tree-with-overrides + row + field caveat): KN-D5/D13 authz side | P-068, P-070, P-133 |
| P-250 | tenancy | P-CP-15 | M3 | placement_of(repo) goes live: repo-granular, region-pinned, relocatable, never node-pinned | P-081, P-084, P-096 |
| P-251 | tenancy | P-CP-16 | M3 | The outbound push-mirror residency gate (mirror_allowed, deny-by-default) | P-096, P-250 |
| P-252 | storage | P-ST-22 | M3 | Local-disk git pack/object storage behind the BlobStore trait (relocatable, region-pinned) | P-047 |
| P-253 | storage | P-ST-24 | M3 | Git crypto-shred reach into reflogs/bitmaps/pack-tier backups | P-099, P-101, P-252 |
| P-254 | storage | P-ST-23 | M3 | The within-EU CDN clone/bundle blob class (C3) | P-047, P-102, P-252 |
| P-255 | storage | P-ST-25 | M3 | The outbound push-mirror residency gate seam (C6) | P-102, P-253, P-254 |
| P-256 | gdpr | P-GA-27 | M3 | The producer-subsystem holders (Git H1 / Knowledge H4 / agent-trace H17) register + the DSR fan-out reaches them + the Knowledge instance | P-109, P-112, P-153 |
| P-257 | gdpr | P-GA-28 | M3 | The Git pseudonymous-commit instance of X-7 (10.9 by reference) + GIT-D2 | P-116, P-118, P-256 |
| P-258 | refs | REF-P17 | M3 | Git producer edges + content-anchored line-range sub-anchors + per-blob replay | P-154, P-165 |
| P-259 | refs | REF-P18 | M3 | Knowledge producer edges + block/row sub-anchors + the first real lifecycle mirror (page_parent) | P-163, P-258 |
| P-260 | search | SRCH-P17 | M3 | Knowledge indexing: blocks/pages multilingual + structured facets + JSONB GIN-scan + vector-in-v1 | P-166, P-175, P-179 |
| P-261 | search | SRCH-P18 | M3 | Code search v1 (Git git.* projection): symbol/path/literal + trigram, the code tokenizer | P-166, P-175, P-179 |
| P-262 | search | SRCH-P19 | M3 | Sub-artifact-granular + content-anchored projections (doc blocks, KN rows/fields, Git line-ranges) + SRCH-D5 Git+KN parity | P-260, P-261 |
| P-263 | notif | NOTIF-P19 | M3 | Producer accretion: Git registers reasons + the watcher ReBAC fragment; re-confirm NOTIF-D4 on real Git subjects (GIT-D8) | P-186, P-187, P-191 |
| P-264 | notif | NOTIF-P20 | M3 | Producer accretion: Knowledge registers reasons + the watcher ReBAC fragment; re-confirm NOTIF-D4 on real KN subjects (KN-D5/KN-D13) | P-186, P-187, P-191 |
| P-265 | workflow | P-FLOW-20 | M3 | Resumable maintenance activities + the history-rewrite invalidation fan-out (M3 support) | P-215 |
| P-266 | workflow | P-FLOW-21 | M3 | The cheap SLA-timer re-arm confirmed under Git/Issues + the merge-queue holds-no-runtime re-green (M3) | P-210 |
| P-267 | agent | AG-P18 | M3 | Per-producer Git ToolDefs: git.merge (gated) + open_pr (reversible) registered into the ToolSurface | P-218, P-229 |
| P-268 | agent | AG-P19 | M3 | Per-producer Knowledge ToolDefs (publish/edit gated) + the content-addressed agent-trace holder seam (KN-D11/KN-D12) | P-131, P-132, P-267 |
| P-269 | git | GIT-P8 | M3 | The GitCore layered seam (canonical git for the wire, gix in-process for read/diff/blame) | P-063 |
| P-270 | git | GIT-P9 | M3 | Receive-pack → one-tx ref-CAS + outbox (the silent-data-loss floor, GIT-D9) | P-063, P-269 |
| P-271 | git | GIT-P10 | M3 | Per-ref aggregate ordering at push QPS (the hot-ref burst, GIT-D1) | P-270 |
| P-272 | git | GIT-P11 | M3 | Pack/delta storage on the local-NVMe BlobStore floor (relocatable, never node-pinned) | P-270 |
| P-273 | git | GIT-P12 | M3 | Pseudonymous-by-default commit identities (the erasure-vs-immutability data-model gate) | P-270 |
| P-274 | git | GIT-P13 | M3 | The front door (SSH + smart-HTTP v2): authenticate, check, placement, residency reject, cross-tenant isolation (GIT-D8) | P-270, P-272 |
| P-275 | git | GIT-P14 | M3 | Wire the Git ReBAC fragment LIVE + the FailStatic bound on the Id dependency | P-123, P-274 |
| P-276 | git | GIT-P15 | M3 | The protected-human-lane shed order + the CDN bundle-URI accelerated-clone floor | P-274 |
| P-277 | git | GIT-P16 | M3 | The PR/review/inline-thread lifecycle + branch-protection rulesets + the CODEOWNERS resolver | P-270, P-274 |
| P-278 | git | GIT-P17 | M3 | PR/review/comment bodies on the myelin-content subset + the content-node → refs.edge.created emission | P-230, P-277 |
| P-279 | git | GIT-P18 | M3 | project(ref, viewer) for git artifacts + the ArtifactRef id grammar (per-viewer permission-checked) | P-275, P-277 |
| P-280 | git | GIT-P19 | M3 | The typed-edge mirror (PR-link / commit-trailer lifecycle edges into the Refs projection) | P-277, P-278 |
| P-281 | git | GIT-P20 | M3 | The check_status projection table + run_attempt monotonic supersession (the X-1 consumer core) | P-232, P-277 |
| P-282 | git | GIT-P21 | M3 | The merge gate + the required-set policy (Git owns what is allowed to land) | P-277, P-281 |
| P-283 | git | GIT-P27 | M3 | Code-executing git tools (history-rewrite, SCIP indexing) on the unified sandbox (the AG-D4 gate) | P-277, P-282 |
| P-284 | git | GIT-P22 | M3 | The fork / trust-tier endorsement gate (the poisoned-pipeline defence, GIT-D10 (b)+(c)) | P-281, P-282 |
| P-285 | git | GIT-P23 | M3 | The merge queue as a durable workflow (parks on ci.result; exactly-once merge; GIT-D10 (d) + the aggregate) | P-282, P-284 |
| P-286 | git | GIT-P24 | M3 | Content-anchored inline-thread line ranges (the #sub 4-state resolver, GIT-D7) | P-230, P-277 |
| P-287 | git | GIT-P25 | M3 | The code-projection emitter for search (declare_indexable emit, incremental on push) | P-231, P-270 |
| P-288 | git | GIT-P26 | M3 | Leak-free fast repo/PR lists + the code-search pre-filter (the list_objects SetExpr push-down, GIT-D11) | P-277, P-287 |
| P-289 | git | GIT-P28 | M3 | Agents as first-class authors/reviewers (legible, bounded; HITL on git.merge; AG-D1/D2/D3/D5) | P-283 |
| P-290 | git | GIT-P29 | M3 | Erasure-reaches-every-holder + history-rewrite erasure semantics (the GDPR git-history obligation, GIT-D2 complete) | P-270, P-273 |
| P-291 | git | GIT-P30 | M3 | Reindex-from-source parity (cold rebuild byte-matches live; no cross-DB read; GIT-D3) | P-281, P-287 |
| P-292 | git | GIT-P31 | M3 | Git notification rules + humanise (confidential subject → tombstone, title never leaks; NOTIF-D4-class) | P-277, P-279 |
| P-293 | git | GIT-P32 | M3 | The Web UI + CLI/API (driven in a browser) + the M3 producer-band exit aggregate | P-277, P-282, P-284 |
| P-294 | knowledge | KN-P04 | M3 | The Knowledge service shell over serve(AppSpec) (boot → three surfaces → drain; hot-table flags) | P-234, P-235, P-236 |
| P-295 | knowledge | KN-P05 | M3 | The OLTP store + the (tenant,region) partition + RLS + tenant-predicate discipline (KN-D13) | P-294 |
| P-296 | knowledge | KN-P06 | M3 | The transactional outbox (emit-iff-committed, relay, dedup) + the knowledge.* event taxonomy (KN-D7) | P-295 |
| P-297 | knowledge | KN-P07 | M3 | Transport item 0: the resume-cursor durable collab transport over the firehose (KN-D1, the headline) | P-295, P-296 |
| P-298 | knowledge | KN-P08 | M3 | The editor primitives standalone (serializer + offset model + DOM-surgery), unit-tested before integration | P-234 |
| P-299 | knowledge | KN-P09 | M3 | The integrated single-doc editor over the primitives + the transport (KN-D2 re-run, browser-drive) | P-297, P-298 |
| P-300 | knowledge | KN-P10 | M3 | The block tree (adjacency list + LexoRank) + stable block ids + page hierarchy | P-236, P-295, P-299 |
| P-301 | knowledge | KN-P11 | M3 | Version history + op-log compaction → content-addressed snapshots + op-log GC | P-295, P-300 |
| P-302 | knowledge | KN-P12 | M3 | The sync_block read-projection floor (permission-filtered, not editable multi-home) | P-234, P-300 |
| P-303 | knowledge | KN-P13 | M3 | The per-block CAS merge floor (no silent overwrite) + soft-locks + offline reconcile (KN-D3, the named-floor proof) | P-297, P-300 |
| P-304 | knowledge | KN-P14 | M3 | The Layer-2 per-op authority checks (permission/schema/erasure) + the zookie new-enemy guard | P-300, P-303 |
| P-305 | knowledge | KN-P15 | M3 | The Knowledge ReBAC page-tree namespace fragment (compiled into the cell schema) | P-300 |
| P-306 | knowledge | KN-P16 | M3 | The list_objects SetExpr push-down + write_tuples/zookie ACL writes (KN-D5, zero leak incl. COUNT) | P-304, P-305 |
| P-307 | knowledge | KN-P17 | M3 | The flexible database (JSONB property bag + GIN-indexed projection + views + relations) (KN-D9) | P-235, P-306 |
| P-308 | knowledge | KN-P18 | M3 | The read-time formula/rollup engine (bounded FormulaAst evaluator, never stored) (KN-D10) | P-235, P-307 |
| P-309 | knowledge | KN-P19 | M3 | Refs glue: #sub mints + 4-step tombstone ladder + edge events + resolve/project + TE-7 typed-edge mirror | P-300, P-307 |
| P-310 | knowledge | KN-P20 | M3 | replay(scope)/reindex-from-source (block-granular *.snapshot via the outbox) (KN-D6, cold == live) | P-301, P-309 |
| P-311 | knowledge | KN-P21 | M3 | The Search feed: declare_indexable(IndexSpec) + query/semantic with the Filter conjoined (KN-D5 re-confirm) | P-306, P-309 |
| P-312 | knowledge | KN-P22 | M3 | Notif/humanise glue + watcher rules (the ONE templating surface, no second engine) | P-309 |
| P-313 | knowledge | KN-P23 | M3 | KB-native comment threads over the shared #sub grammar (Floor 4: one scheme, two stores with Chat) | P-234, P-309 |
| P-314 | knowledge | KN-P24 | M3 | The Export/Import service (lossless JSON Art. 20 + Markdown/HTML/PDF/CSV + ADF lossy-map import) | P-234, P-235, P-300 |
| P-315 | knowledge | KN-P25 | M3 | The PersonalDataHolder{locate/export/rectify/restrict} + the #[personal_data] classify-derive tags | P-294, P-311 |
| P-316 | knowledge | KN-P26 | M3 | The erase structural floor: per-subject DEK crypto-shred + pseudonym shred + tombstone/embedding purge (KN-D4) | P-301, P-315 |
| P-317 | knowledge | KN-P27 | M3 | Agent governance: Knowledge ToolDefs + EffectApi apply + HITL withhold + per-effect idem_key + reserve/settle (KN-D11) | P-297, P-304 |
| P-318 | knowledge | KN-P28 | M3 | The AG-7 content-addressed agent-trace holder (erasable, distinct from the audit log) (KN-D12) | P-316, P-317 |
| P-319 | substrate | P-S30 | M4 | Enforce the cross-language harness shim (if Chat diverges) | P-136 |
| P-320 | identity | P-ID-27 | M4 | The CI ReBAC namespace fragment (secret-non-inheritance + !is_untrusted_fork): CI-D10 fragment side | P-068, P-247 |
| P-321 | identity | P-ID-28 | M4 | The self-hosted-runner token scope exercised against the CI fragment: CI-D10 scope side | P-076, P-320 |
| P-322 | identity | P-ID-29 | M4 | The Issues ReBAC namespace fragment (confidential-exclusion + field/transition caveats): ISS-D3 authz side | P-068, P-070, P-133 |
| P-323 | identity | P-ID-30 | M4 | The Chat ReBAC namespace fragment (channel.read + message.view + 50k-watcher density): CHAT authz side | P-068, P-070, P-134 |
| P-324 | tenancy | P-CP-17 | M4 | residency_verify CI-store coverage: the no-global-pool attestation extended over the CI surfaces | P-085, P-096 |
| P-325 | tenancy | P-CP-18 | M4 | Residency-pinned runners: an EU tenant's CI run claimed only by an in-region runner | P-096, P-324 |
| P-326 | substrate | P-S31 | M4 | The firehose backpressure half under connection-storm | P-002, P-135, P-136 |
| P-327 | event-bus | EB-27 | M4 | The check-seam producer leg goes live end-to-end (M4) + CI/Issues/Chat token lists + their replay | P-246 |
| P-328 | storage | P-ST-26 | M4 | The T3 CI log tier: the (job,step,byte-range) index (C2) + #step-<n> resolution | P-147 |
| P-329 | storage | P-ST-27 | M4 | Per-subject CI-log DEK (C1) + the per-tenant-fallback residual | P-095, P-099, P-328 |
| P-330 | storage | P-ST-28 | M4 | Trust-scoped CI cache namespaces (C4) | P-047 |
| P-331 | storage | P-ST-29 | M4 | The OLAP restriction-flag gate (C5) | P-104, P-145 |
| P-332 | gdpr | P-GA-29 | M4 | The CI consumer holder (H2) + the per-subject CI-log DEK crypto-shred reach (CI-D3) | P-151, P-257 |
| P-333 | gdpr | P-GA-30 | M4 | The Issues (H3) + Chat (H5) consumer holders register + the DSR fan-out reaches them + the instances (ISS-D11 / CHAT-D8) | P-151, P-332 |
| P-334 | gdpr | P-GA-31 | M4 | The worklog/productivity/estimate Behavioural classification (OQ-H) + the works-council consultation trigger + the SpecialCategory→DPIA route | P-107, P-108, P-152, P-333 |
| P-335 | refs | REF-P19 | M4 | The Git<->CI CheckStatus seam closes: resolve the check-/step- sub-anchors (Refs' half of X-1) | P-258, P-259 |
| P-336 | refs | REF-P20 | M4 | Issues lifecycle edges: the second real TE-7 mirror (issue_relation) | P-259 |
| P-337 | refs | REF-P21 | M4 | Chat unfurls: the maximal consumer + cross-subsystem traversal complete | P-335, P-336 |
| P-338 | search | SRCH-P20 | M4 | Issues indexing: the FieldType facets + order_key columnar sort (the consumer corpus arrives) | P-260, P-262 |
| P-339 | search | SRCH-P21 | M4 | The Issues Tier-3 board-escalation valve: byte-identical ACL pre-filter (the OLTP-budget escalation seam) | P-172, P-338 |
| P-340 | search | SRCH-P22 | M4 | CI log search: the per-subject-DEK sealed segments + the (job, step, byte-range) index (details_ref resolves) | P-260, P-262 |
| P-341 | search | SRCH-P23 | M4 | Chat indexing: message bodies + search-as-non-member = 0 (the CHAT-D11 analog) + cross-subsystem facets dependable | P-338, P-340 |
| P-342 | notif | NOTIF-P21 | M4 | Consumer accretion: Issues registers reasons + passes the real SLA escalation chain (ISS-D6) | P-183, P-186 |
| P-343 | notif | NOTIF-P22 | M4 | Consumer accretion: Chat registers activity/mentions + the explicit-first agent dispatch boundary + HITL cards (CHAT-D5, CHAT-D17) | P-186, P-187 |
| P-344 | notif | NOTIF-P23 | M4 | Consumer accretion: CI registers status-summary reasons; the CheckStatus.summary HumanisedRef resolves through humanise (X-1) | P-186, P-187 |
| P-345 | workflow | P-FLOW-22 | M4 | The CI-pipeline-as-workflow substrate + reference fixture (CI-D9, CI-D1) | P-215 |
| P-346 | workflow | P-FLOW-23 | M4 | The X-1 seam end-to-end: the merge-queue long-park wakes on the real ci.result (GIT-D10/CI-D8) | P-215 |
| P-347 | agent | AG-P20 | M4 | Per-consumer ToolDefs (Issues transition ABAC / Chat explicit-first / CI deploy gated) + the dispatch drills | P-221, P-268 |
| P-348 | agent | AG-P21 | M4 | AG-D4 / CI-T1 re-confirmed GREEN on the production CI runner image (the M4 hard gate) | P-229, P-347 |
| P-349 | ci | CI-P6 | M4 | The five CI service shells + the complete forward-only data-model migrations | P-237, P-239 |
| P-350 | ci | CI-P7 | M4 | The complete ci.* event taxonomy registered into the Bus seed | P-349 |
| P-351 | ci | CI-P8 | M4 | The CI ReBAC namespace fragment (ci_project/environment/secret/run + read & !is_untrusted_fork) | P-349 |
| P-352 | ci | CI-P9 | M4 | The CI PersonalDataHolder (auto-registered, locate/export typed, erase stubbed to crypto-shred) | P-349 |
| P-353 | ci | CI-P10 | M4 | Trigger & Dispatch: the EventMatcher (= QueryAst) + exactly-once dedup + the trust-tier evaluation and single stamp | P-349 |
| P-354 | ci | CI-P11 | M4 | Trigger & Dispatch: the definition resolution → content-addressed snapshot + the reserve/start handoff | P-349, P-353 |
| P-355 | ci | CI-P12 | M4 | Green-field core: the pull-lease claim query + concurrency groups + affinity + the dead-runner reaper | P-349 |
| P-356 | ci | CI-P13 | M4 | Green-field core: DRR fair-share over fair_key + priority lanes + per-tenant backpressure | P-349, P-355 |
| P-357 | ci | CI-P14 | M4 | Green-field core: the EU fleet autoscaler (FleetProvider + autoscale-on-queue-depth + per-residency-zone pools + fleet events) | P-349, P-355, P-356 |
| P-358 | ci | CI-P15 | M4 | The ci.pipeline durable workflow body + the determinism guard (CI-D9) | P-349, P-354 |
| P-359 | ci | CI-P16 | M4 | The SCHEDULE_AND_RUN_JOB long-park idiom + crash-recovery / effectively-once (CI-D1) | P-355, P-356, P-358 |
| P-360 | ci | CI-P17 | M4 | Reserve/settle = the one metering path + the cost_event ledger + parity CI ↔ agent (CI-D5) | P-359 |
| P-361 | ci | CI-P18 | M4 | The X-1 check_attempt monotonic counter + the ci.check.updated producer (the CheckStatus fact) | P-349, P-358 |
| P-362 | ci | CI-P19 | M4 | The X-1 ci.result rollup signal + the GIT-D10 / CI-D8 check-seam end-to-end GATE (0 double-merge) | P-358, P-361 |
| P-363 | ci | CI-P20 | M4 | Logs over the firehose + the sealed T3 (job, step, byte-range) log tier + ci.log.available pointers | P-238, P-349 |
| P-364 | ci | CI-P21 | M4 | The resume-cursor live-tail + the details_ref jump-to-failure resolution (CI-D11) | P-361, P-363 |
| P-365 | ci | CI-P22 | M4 | Trust-scoped artifacts & caches + the within-EU CDN clone class + per-subject log DEK (CI-D6) | P-349, P-363 |
| P-366 | ci | CI-P23 | M4 | Supply-chain trust: digest-pin-or-fail-closed + sigstore sign/verify + SLSA/SBOM provenance (CI-D4) | P-354 |
| P-367 | ci | CI-P24 | M4 | The in-boundary secret broker (fork-gets-no-secrets, CI-D7) + deployments & the protected-env HITL gate | P-237, P-351 |
| P-368 | ci | CI-P25 | M4 | Cross-fabric surfacing: the list_objects SetExpr push-down + ArtifactRef/#sub mints + project(ref, viewer) | P-349, P-351, P-361 |
| P-369 | ci | CI-P26 | M4 | Cross-fabric surfacing: declare_indexable + humanise registrations + replay(*.snapshot) + the ToolDef registrations | P-350, P-368 |
| P-370 | ci | CI-P27 | M4 | Re-confirm the two permanent gates at the M4 boundary: AG-D4 / CI-T1 on the prod runner image + STOR-D1/STOR-D2 restore-verify on the CI stores | P-239, P-349, P-369 |
| P-371 | issues | ISS-P05 | M4 | The issue-spine migrations (the typed core + JSONB tail + relations + change-log + scheme/cycle/milestone tables) | P-125, P-242 |
| P-372 | issues | ISS-P06 | M4 | The silent-data-loss-safe write path (validate → check → mutate → OutboxTx::emit in one tx) | P-242, P-371 |
| P-373 | issues | ISS-P07 | M4 | Pseudonymous-by-default identity columns + per-subject-DEK free-text + the holder registration | P-371, P-372 |
| P-374 | issues | ISS-P08 | M4 | Hi/Lo human-key allocation (the <PROJECTKEY>-<seqno> stored canonical id) | P-371, P-372 |
| P-375 | issues | ISS-P09 | M4 | The server-arbitrated order_key CAS reorder (the silent-clobber floor) | P-241, P-372 |
| P-376 | issues | ISS-P10 | M4 | The issue body + comments as a myelin-content block subtree (render(parse(md)) === md) | P-372, P-375 |
| P-377 | issues | ISS-P11 | M4 | Governance schemes + the scheme-precedence algebra + the flexible-field model (config, never a migration) | P-371, P-372 |
| P-378 | issues | ISS-P12 | M4 | The data-driven workflow FSM interpreter + the QueryAst guards (the fixed state-category invariant) | P-372, P-377 |
| P-379 | issues | ISS-P13 | M4 | The AST→OLTP-store compiler: the SetExpr push-down lowered first (leak-free, no N+1, no post-filter) | P-241, P-371, P-372, P-377 |
| P-380 | issues | ISS-P14 | M4 | Cost-bounding + the three-tier escalation (the <1s flexible-field latency floor) | P-379 |
| P-381 | issues | ISS-P15 | M4 | The projection-feeder consumer (the measured generated-index promotion) | P-372, P-380 |
| P-382 | issues | ISS-P16 | M4 | The co-equal ViewSpec views + the design-system pass (board/roadmap/backlog/table/calendar/cycle) | P-241, P-379, P-380 |
| P-383 | issues | ISS-P17 | M4 | Refs wiring (resolve/project/#sub/edges/traverse/TE-7 mirror) + the issue.* Search projection emitter | P-379, P-382 |
| P-384 | issues | ISS-P18 | M4 | The event-driven incremental rollup consumer (off the bus, never in the write path) | P-372, P-383 |
| P-385 | issues | ISS-P31 | M4 | Erasure-reaches-every-holder (the PersonalDataHolder fan-out + post-restore re-erasure) | P-373, P-384 |
| P-386 | issues | ISS-P19 | M4 | The time axis (cycles/sprints + milestones) + attachments in BlobStore | P-371, P-384 |
| P-387 | issues | ISS-P20 | M4 | The OLAP read store (CQRS, reindex-from-source only, restriction-flag-honouring) | P-384, P-386 |
| P-388 | issues | ISS-P21 | M4 | The two-pass ID-remapped import engine + the ADF lossy-map (the adoption gate) | P-372, P-384, P-386 |
| P-389 | issues | ISS-P22 | M4 | "My Work" over the ONE Notif inbox + the humanise templates (one read-state truth) | P-243, P-372 |
| P-390 | issues | ISS-P23 | M4 | The Issues ToolDefs + EffectApi plan-then-apply + the mock forecast/triage agents (gated on AG-D4) | P-378, P-387 |
| P-391 | issues | ISS-P24 | M4 | Reserve/settle on every spend-bearing agent run (the same wallet as CI) | P-390 |
| P-392 | issues | ISS-P25 | M4 | The stateful Trigger flagship ("Remind me when unblocked" — exactly-once across a restart) | P-384, P-390 |
| P-393 | issues | ISS-P26 | M4 | The SLA business-calendar engine over myelin-flow (fire_at to-the-second across a restart) | P-378, P-392 |
| P-394 | issues | ISS-P27 | M4 | The CI-red governed-transition guard (closing the X-1 consumer; reads trust_tier off the fact) | P-378, P-390 |
| P-395 | issues | ISS-P28 | M4 | The cross-subsystem reflexes (git/chat/identity/ci consumers) | P-378, P-394 |
| P-396 | issues | ISS-P29 | M4 | The governance admin views (S13–S18; each preceded by its design sketch) | P-377, P-378, P-393 |
| P-397 | issues | ISS-P30 | M4 | Real-time board sync over the firehose resume-cursor protocol (0 ops lost on reconnect) | P-382 |
| P-398 | chat | CHAT-P4 | M4 | The MessageStore trait + the partitioned hot tier + the fs-backed cold-segment tier (the swap seam) | P-128 |
| P-399 | chat | CHAT-P5 | M4 | The outbox co-commit + idempotent send + per-conversation total order (the silent-data-loss floor for chat) | P-128, P-398 |
| P-400 | chat | CHAT-P6 | M4 | Per-subject-DEK message bodies + the PersonalDataHolder + the #sub mint + the replay(scope,since) skeleton | P-399 |
| P-401 | chat | CHAT-P7 | M4 | The Conversation / Membership entity + the membership_by_principal conversation-list index | P-398, P-399 |
| P-402 | chat | CHAT-P8 | M4 | Membership → write_tuples → zookie in one transaction + the new-enemy guard + the send/membership check gate | P-399, P-401 |
| P-403 | chat | CHAT-P9 | M4 | The stateless Rust connection-tier gateway + subscribe/resume/resync_required (the zero-loss-across-reconnect backbone) | P-399, P-400 |
| P-404 | chat | CHAT-P10 | M4 | Firehose-only live delivery (message/presence/typing/read-state/partials) + the protected-human-lane shed order | P-128, P-403 |
| P-405 | chat | CHAT-P11 | M4 | The message body over the frozen myelin-content Chat subset (render(parse(md))===md) + the inline nodes → refs.edge.created | P-400 |
| P-406 | chat | CHAT-P12 | M4 | The composer UI (slash menu + @/# autocomplete + paste-URL→unfurl + draft) + the per-message CAS (no CRDT) | P-400, P-405 |
| P-407 | chat | CHAT-P13 | M4 | The Unfurl Service: the shared per-ref projection cache + the per-viewer list_objects/check gate (the no-leak floor) | P-402, P-405 |
| P-408 | chat | CHAT-P14 | M4 | Erasure-safe unfurls + bus-driven cache invalidation + #sub anchor stability (CHAT-D6 / D7 / D18) | P-404, P-407 |
| P-409 | chat | CHAT-P15 | M4 | project(ref, viewer) for chat/{channel,message,thread} + chat as the densest refs.edge.created producer | P-402, P-407 |
| P-410 | chat | CHAT-P16 | M4 | The read-state hot path (Valkey hot markers + batched PG flush; cache-never-authoritative) | P-399, P-404 |
| P-411 | chat | CHAT-P22 | M4 | The GDPR holder + author crypto-shred across hot/cold/backups + the DSR cascade (CHAT-D8; 0 recoverable PII) | P-400, P-408, P-410 |
| P-412 | chat | CHAT-P17 | M4 | The fanout-class boundary (write-fanout vs read-fanout; celebrity-fanout mitigation) + Activity-as-view | P-245, P-410 |
| P-413 | chat | CHAT-P18 | M4 | The HITL approval-card bridge (per-effect idem_key; withhold→approve→resume; exactly-once across a multi-day kill) | P-402, P-406, P-410 |
| P-414 | chat | CHAT-P19 | M4 | The agent ToolDef set (frozen X-6 defaults) routed through EffectApi + reserve/settle + run --dry-run (the routing-split safety boundary) | P-402, P-413 |
| P-415 | chat | CHAT-P20 | M4 | ACL-filtered Search indexing (declare_indexable + the Filter conjoin) + embeddings-as-PII + the HYOK skip | P-400, P-402 |
| P-416 | chat | CHAT-P21 | M4 | replay(scope, since) full parity: Search/Refs/Notif read-models rebuild, steady-state and recovery share one path (CHAT-D15) | P-400, P-409, P-415 |
| P-417 | chat | CHAT-P23 | M4 | Mention pseudonym-shred (→ [erased user]) + the Art.18 restriction flag at every read path + the LEGAL free-text residual (BY REFERENCE) | P-405, P-411 |
| P-418 | chat | CHAT-P24 | M4 | Agent presence classes + streaming partials (mock-provable; final replaces partial; reconnect resumes the final) (CHAT-D16) | P-128, P-404 |
| P-419 | chat | CHAT-P25 | M4 | Explicit-first agent dispatch (no auto-spawn on mention; reserve-gated) + the agent provenance popover (CHAT-D17) | P-412, P-414, P-418 |
| P-420 | event-bus | EB-29 | M5 | World-scale: the 30× agent surge + per-aggregate order at QPS + crypto-shred reaches backups | P-012, P-143 |
| P-421 | search | SRCH-P28 | M5 | World-scale: restore + cross-seam + re-erase at scale (SRCH-D9, the restore-verify permanent gate) | P-178, P-179 |
| P-422 | search | SRCH-P29 | M5 | World-scale: HYOK cross-store at scale (SRCH-D10) + the backup-scale erasure proof (SRCH-D4 at backup scale) | P-178, P-421 |
| P-423 | ci | CI-P28 | M5 | Floor follow-on: the gVisor second backend behind the SandboxBackend trait + re-greening the escape GATE (trigger-gated) | P-129, P-237 |
| P-424 | identity | P-ID-31 | M5 | World-scale hardening: the 30x authz surge ID-D9 (protected-human-lane shed order) | P-070, P-073 |
| P-425 | identity | P-ID-32 | M5 | World-scale hardening: the S8 measured tunables finalised at scale (cardinality cap + reverse_index_lag SLO) | P-069, P-074, P-424 |
| P-426 | identity | P-ID-33 | M5 | World-scale hardening: ID-D8 at cell scale + the cell-bulkhead drill | P-078 |
| P-427 | identity | P-ID-34 | M5 | World-scale hardening: Id as the proven authz spine of the four E2E scenarios (E2E-1..E2E-4) | P-069, P-070, P-075, P-076, P-078 |
| P-428 | identity | P-ID-35 | M5 | Multi-cell principal authority: the cross-cell read-through over the PII-free bridge (GA-D8/CP-D7/CP-D8 authz side) | P-068, P-069, P-078 |
| P-429 | tenancy | P-CP-19 | M5 | Multi-cell goes live: the CrossCellPointer bridge resolution (always cell-local, 0 PII) | P-027, P-096 |
| P-430 | tenancy | P-CP-20 | M5 | Cross-cell DSR fan-out + cross-cell zookie consistency + multi-cell rebalancing (GA-D8) | P-084, P-429 |
| P-431 | tenancy | P-CP-22 | M5 | Live tenant migration + repo relocation + durable provisioning + measured sizing + restore-verify at cell scale | P-083, P-250, P-429, P-430 |
| P-432 | tenancy | P-CP-21 | M5 | The cell bulkhead under 30× surge (CP-D5): a fault in one cell leaves others unaffected | P-096, P-429 |
| P-433 | substrate | P-S32 | M5 | World-scale: the 30× surge family (SUB-D3) | P-035, P-036, P-326 |
| P-434 | substrate | P-S33 | M5 | Tune the per-surface shed budgets to measured numbers | P-035, P-038, P-326, P-433 |
| P-435 | substrate | P-S34 | M5 | World-scale: online-migration-under-load (SUB-D10) | P-032, P-056, P-433 |
| P-436 | substrate | P-S35 | M5 | World-scale: restore-verify re-confirmed at cell scale (SUB-D6 / STOR-D2) | P-056, P-435 |
| P-437 | substrate | P-S36 | M5 | Tune the resilient-client per-target values to measured numbers | P-033, P-034, P-038, P-433 |
| P-438 | event-bus | EB-25 | M5 | Build the cross-cell PII-free pointer bridge (the M1-frame floor follow-on) | P-091 |
| P-439 | event-bus | EB-30 | M5 | Tune the firehose retention window + re-green D-10 across the KN CAS→CRDT engine_promote boundary | P-141 |
| P-440 | event-bus | EB-31 | M5 | The Bus as the E2E spine + the column-store seam measurement gate | P-327, P-420 |
| P-441 | storage | P-ST-30 | M5 | Object-store BlobStore (the fs-floor follow-on, behind the unchanged trait) | P-047 |
| P-442 | storage | P-ST-31 | M5 | Object-backed git packs (the local-disk-packs follow-on) | P-252, P-254, P-441 |
| P-443 | storage | P-ST-32 | M5 | The cross-cell PII-free pointer bridge live + cell→cell migration (CP-D7) | P-058, P-060, P-099 |
| P-444 | storage | P-ST-34 | M5 | Restore-verify at cell scale + the F6 surge family on the storage lanes | P-061, P-100, P-126, P-146, P-329, P-330, P-441, P-443 |
| P-445 | storage | P-ST-33 | M5 | The multi-cell DSR erase fan-out (iterate member_cells, GA-D8) | P-099, P-443 |
| P-446 | storage | P-ST-35 | M5 | The full DSAR / crypto-shred fan-out across all H1–H18 holders (E2E-4 spine) | P-099, P-100, P-445 |
| P-447 | storage | P-ST-36 | M5 | The E2E-3 storage half: cold-reindex == live for the derived stores | P-060, P-145, P-444 |
| P-448 | gdpr | P-GA-32 | M5 | The full H1–H18 DSR fan-out (GA-D1, 0 holders missed) + STOR-D3 at cell scale | P-114, P-115, P-119, P-334 |
| P-449 | gdpr | P-GA-33 | M5 | Multi-cell DSR fan-out (the member_cells iteration over the cross-cell PII-free bridge, GA-D8) | P-113, P-448 |
| P-450 | gdpr | P-GA-34 | M5 | The E2E-4 DSAR fan-out flagship (the whole-system GDPR-by-construction proof) | P-115, P-119, P-448, P-449 |
| P-451 | gdpr | P-GA-35 | M5 | History-rewrite as a first-class audited op (GA-10) + audit tamper-evidence at cell scale (GA-D3 / GA-D6, the E2E-3 leg) | P-119, P-149, P-153 |
| P-452 | gdpr | P-GA-36 | M5 | The outbound push-mirror residency gate (GA-11, deny extra-EU by default / allow within-EU CDN) | P-150 |
| P-453 | refs | REF-P22 | M5 | World-scale: the 30x surge + the protected-human-lane shed order (REF-D10) | P-337 |
| P-454 | refs | REF-P23 | M5 | World-scale: the hot-artifact reach index R4 (measured-trigger; the REF-P11 floor's follow-on) | P-160, P-453 |
| P-455 | refs | REF-P24 | M5 | World-scale: reindex-parity at full scale across both TE-7 mirrors (REF-D4 at scale) | P-165, P-337 |
| P-456 | refs | REF-P25 | M5 | World-scale: restore + re-erase at backup scale (REF-D5 at backup scale) | P-164, P-455 |
| P-457 | refs | REF-P26 | M5 | World-scale: the cross-cell backlink fan-out build (the REF-P10 floor's follow-on) | P-159, P-454 |
| P-458 | refs | REF-P27 | M5 | World-scale: the whole-system E2E wedge (E2E-1 PR pane / E2E-3 spec-to-ship / E2E-4 DSAR) | P-455, P-456, P-457 |
| P-459 | search | SRCH-P24 | M5 | World-scale: the freshness budget under load (SRCH-D7 full-scale) | P-341 |
| P-460 | search | SRCH-P25 | M5 | World-scale: the 30x agent/CI query surge + the protected-human-lane shed order (SRCH-D6) | P-176, P-341 |
| P-461 | search | SRCH-P26 | M5 | World-scale: the tuned filtered-ANN strategy + HNSW<->IVF-PQ promotion (SRCH-D8 recall@k) | P-168, P-174 |
| P-462 | search | SRCH-P27 | M5 | World-scale: the measured projection-feeder promotion (GIN scan -> generated index, OQ-C > 5%) | P-260, P-338 |
| P-463 | search | SRCH-P30 | M5 | World-scale: the object-store index backstop (the fs-backed BlobStore -> object-store swap) | P-421, P-422 |
| P-464 | search | SRCH-P31 | M5 | World-scale: cross-cell federated search (designed-and-extends, scatter-gather + residency-free merge) | P-459, P-463 |
| P-465 | search | SRCH-P32 | M5 | World-scale: the whole-system E2E wedge (E2E-1 PR pane / E2E-3 reindex-parity / E2E-4 DSAR fan-out) | P-421, P-422, P-464 |
| P-466 | notif | NOTIF-P24 | M5 | Cross-cell inbox aggregation (the multi-cell floor's follow-on; always-cell-local resolution) | P-183, P-187 |
| P-467 | notif | NOTIF-P25 | M5 | The 30×-agent-surge shed budget (the F6 surge family; human-last lane) + NOTIF-D5 | P-181, P-189, P-191 |
| P-468 | notif | NOTIF-P26 | M5 | The EU-sovereign delivery provider follow-on (swap the real provider into the DeliveryAdapter trait; [OPEN — LEGAL]) | P-187, P-194 |
| P-469 | notif | NOTIF-P27 | M5 | The erasure residual instanced (the X-7 posture for Notif) + NOTIF-D6 | P-182, P-187 |
| P-470 | notif | NOTIF-P28 | M5 | The E2E wedge: Notif's E2E-1 leg (the PR context pane — per-viewer humanise + live firehose updates) | P-187, P-193 |
| P-471 | notif | NOTIF-P29 | M5 | The E2E wedge: Notif's E2E-2 leg (the HITL flagship — approval card + explicit-first + exactly-once across a kill) | P-183, P-185 |
| P-472 | notif | NOTIF-P30 | M5 | The E2E wedge: Notif's E2E-4 DSAR leg + STOR-D2 at cell scale (the permanent gate; the last Notif prompt) | P-466, P-469 |
| P-473 | workflow | P-FLOW-24 | M5 | Crypto-shred reaching history: the PersonalDataHolder erase path completed (FLOW-D9) | P-346 |
| P-474 | workflow | P-FLOW-25 | M5 | Restore-verify to a consistent point: in-flight runs resume, no vanished result (FLOW-D10) | P-473 |
| P-475 | workflow | P-FLOW-26 | M5 | World-scale: the 1M+ timer cell-scale run + the per-cell promotion threshold (FLOW-D3 full) | P-207, P-346 |
| P-476 | workflow | P-FLOW-27 | M5 | World-scale: the 30x agent-workflow surge with lane shedding (FLOW-D8) | P-212, P-475 |
| P-477 | workflow | P-FLOW-28 | M5 | The E2E-2 flagship: the durable-workflow + HITL spine across the kill + days-later approval | P-473, P-474, P-475, P-476 |
| P-478 | agent | AG-P22 | M5 | The 30x agent-dispatch surge family (AG-D6): the human lane holds, the agent lane sheds, the shed budget tuned | P-227, P-347 |
| P-479 | agent | AG-P23 | M5 | Erasure reaches the trace + agent memory (AG-D10): the Fabric's full DSR holder bodies | P-132, P-268 |
| P-480 | agent | AG-P24 | M5 | The E2E-2 flagship: CI-fail -> triage agent -> issue -> chat -> fix-PR across a service kill | P-221, P-229, P-347, P-478, P-479 |
| P-481 | agent | AG-P25 | M5 | Name the LlmAgentRuntime post-M5 swap + the external MCP endpoint + long-term memory: the seam doc | P-130, P-480 |
| P-482 | git | GIT-P33 | M5 | World-scale floor follow-ons: object-backed packs, cross-cell replication, speculative queue, SHA-256, SCIP | P-269, P-293 |
| P-483 | git | GIT-P34 | M5 | World-scale hardening (the F6 surge family, GIT-D6) + git's slices of the four whole-system E2E scenarios | P-482 |
| P-484 | knowledge | KN-P29 | M5 | The Yrs CRDT promotion over the unchanged transport + the online engine_promote migration (KN-D1 re-green) | P-297, P-303 |
| P-485 | knowledge | KN-P30 | M5 | Cross-cell collab: true cross-cell op fan-out over the PII-free CrossCellPointer bridge | P-295, P-484 |
| P-486 | knowledge | KN-P31 | M5 | Facet/rollup materialisation + the object-store BlobStore swap (KN-D9/D10 at scale, KN-P17/P18/P05/P11 floors resolved) | P-307, P-308 |
| P-487 | knowledge | KN-P32 | M5 | The all-hands-doc surge controls + the concurrent-same-gap LexoRank storm (KN-D8 + the F6 leg) | P-297, P-484 |
| P-488 | knowledge | KN-P33 | M5 | Knowledge's legs of the whole-system E2E wedge (E2E-1 PR context pane + E2E-3 spec-to-ship lineage) | P-306, P-309 |
| P-489 | ci | CI-P29 | M5 | Floor follow-ons: the time-series log tier + the hierarchical scheduler (each measured-trigger-gated) | P-356, P-363 |
| P-490 | ci | CI-P30 | M5 | World-scale hardening: the 30x CI surge family (CI-D2) + the tuned DRR/shed-budget numbers + the pre-warm buffer sizing | P-240, P-355, P-356 |
| P-491 | ci | CI-P31 | M5 | World-scale hardening: residency at cell scale (CI-R3) + the self-hosted runner trust boundary (CI-D10) | P-240, P-357 |
| P-492 | ci | CI-P32 | M5 | World-scale hardening: the PersonalDataHolder crypto-shred erase path — erasure-reaches-every-holder (CI-D3) | P-352, P-365 |
| P-493 | ci | CI-P33 | M5 | CI's slices of the whole-system E2E wedge: E2E-1 (PR context pane) + E2E-3 (spec-to-ship traceability) | P-361, P-364 |
| P-494 | ci | CI-P34 | M5 | CI's slice of the whole-system E2E wedge: E2E-2 the agent-native flagship (CI-fail → triage agent → issue → chat → fix-PR) | P-239, P-369 |
| P-495 | issues | ISS-P32 | M5 | The measured floor follow-ons (move-CRDT / materialised rollup / distributed-SQL / cross-cell / Monte-Carlo / column-store) | P-375, P-384 |
| P-496 | issues | ISS-P33 | M5 | World-scale hardening (the F6 surge family + the scale benchmarks) | P-379, P-393, P-495 |
| P-497 | issues | ISS-P34 | M5 | E2E-1: the PR context pane (Issues' linked-issue resolves per-viewer, 0 leak) | P-383, P-496 |
| P-498 | issues | ISS-P35 | M5 | E2E-2: the agent-native flagship (CI-fail → triage → issue → chat → fix-PR) | P-390, P-391, P-394 |
| P-499 | issues | ISS-P36 | M5 | E2E-3: spec-to-ship traceability (the spec→issue→PR→CI lineage per-viewer) | P-383, P-387 |
| P-500 | chat | CHAT-P26 | M5 | World-scale surge hardening: the 30x agent-surge + deploy-herd (F6) + tuning the per-surface shed budgets (CHAT-D3 / D4-at-scale) | P-399, P-419 |
| P-501 | chat | CHAT-P27 | M5 | The whole-system E2E wedge participation (E2E-1 pane + E2E-2 the agent-native flagship terminal surface + E2E-4 DSAR holder) | P-408, P-500 |
| P-502 | chat | CHAT-P28 | M5 | ScyllaDB hot-tier promotion (M5-C-S2; the named M4-C1 floor; a MessageStore trait swap) + the object-store BlobStore swap | P-398, P-399, P-411 |
| P-503 | chat | CHAT-P29 | M5 | Mega-channel channel-sharded home-node (M5-C-S3; the named M4-C2 delivery floor; Phoenix/Discord guild model) | P-403, P-404, P-500 |
| P-504 | chat | CHAT-P30 | M5 | Cross-org / federated channels (M5-C-X1; designed-not-built → on the frozen cross-cell PII-free pointer bridge) | P-401, P-402 |
| P-505 | chat | CHAT-P31 | M5 | Comment-threading consolidation onto the Chat threading primitive (M5-C-X2; OQ-L; a store/transport swap, not a rewrite) | P-244, P-404 |
| P-506 | storage | P-ST-37 | M6 | Dogfood: the restore-verify gate runs on Myelin's own commits + the truth-up pass | P-444, P-446, P-447 |
| P-507 | substrate | P-S37 | M6 | Dogfood: run the lints, the contract-coverage scanner, and the mutation gate as Myelin CI jobs | P-008, P-009, P-017, P-018, P-033, P-035, P-036, P-037, P-040 |
| P-508 | tenancy | P-CP-23 | M6 | Dogfooding: Myelin self-hosts as exactly one cell + the lints run as Myelin CI | P-097, P-431 |
| P-509 | ci | CI-P35 | M6 | Dogfooding: Myelin's own build/test/lint/mutation pipeline runs as a Myelin CI pipeline + the switch test | P-370, P-492, P-494 |
| P-510 | substrate | P-S38 | M6 | The every-incident-adds-a-drill loop on Myelin's tracker + the truth-up pass | P-004, P-507 |
| P-511 | gdpr | P-GA-37 | M6 | Dogfood: the GDPR/Audit machinery live on Myelin's own commits + a self-served DSR | P-450, P-451, P-452 |
| P-512 | gdpr | P-GA-38 | M6 | The truth-up pass: confirm every PROVEN GDPR gate rests on a dated green artifact | P-511 |
| P-513 | refs | REF-P28 | M6 | Dogfooding: the reference graph over Myelin's own work + the self-hosting CI graph | P-454, P-455, P-456, P-457, P-458 |
| P-514 | refs | REF-P29 | M6 | Dogfooding: the reference-graph switch-test surfaces driven in a browser | P-513 |
| P-515 | search | SRCH-P33 | M6 | Dogfooding: Search over Myelin's own work + the switch test + the self-hosting CI graph | P-459, P-465 |
| P-516 | workflow | P-FLOW-29 | M6 | Dogfooding: Myelin's own pipelines / merge queue / SLA timers as myelin-flow workflows | P-477 |
| P-517 | agent | AG-P26 | M6 | Dogfood: the platform's own agents run on the platform's own commits/issues/chat | P-229, P-478, P-479, P-480, P-481 |
| P-518 | git | GIT-P35 | M6 | Dogfood: Myelin hosts its own repositories (the switch test) | P-483 |
| P-519 | knowledge | KN-P34 | M6 | Dogfooding: Myelin's own docs in Knowledge + the switch test driven in a browser | P-484, P-488 |
| P-520 | issues | ISS-P37 | M6 | Dogfood: Myelin tracks its own issues (the switch test) | P-497 |
| P-521 | chat | CHAT-P32 | M6 | The switch test: drive the real Chat UI in a browser (the 13 screens + the responsive cases) | P-399, P-501, P-502, P-505 |

---

## 3. Totals

- **Total prompts: 521** (across all 16 systems; one clean-context, independently-committable unit each).
- **Aggregate token size: ~645k tokens** of authored prompt content (the by-system prompt bodies sum to ~2.58 MB
  of Markdown ≈ 645k tokens) — squarely inside the VISION §7 target band of **400k–700k tokens**. Per prompt
  this averages ~1240 tokens of prompt body; each prompt then directs ~1500–4000 tokens of *executing-agent
  work* (the bounded code + tests it asks for), per `00-ledger-overview.md` §4.
- **Per-band counts:**

| Band | Prompt count |
|---|---|
| M0 | 53 |
| M1 | 72 |
| M2 | 120 |
| M3 | 73 |
| M4 | 101 |
| M5 | 86 |
| M6 | 16 |
| **Total** | **521** |

- **Per-system counts:**

| System | Prompt count |
|---|---|
| substrate | 38 |
| event-bus | 30 |
| identity | 35 |
| tenancy | 23 |
| storage | 37 |
| gdpr | 38 |
| refs | 29 |
| search | 33 |
| notif | 30 |
| workflow | 29 |
| agent | 26 |
| git | 35 |
| knowledge | 34 |
| ci | 35 |
| issues | 37 |
| chat | 32 |
| **Total** | **521** |

**Integrity.** The order is a total order with **0 dependency-precedence violations** (every resolved
DEPENDS-ON id precedes its dependent) and the dependency graph is **acyclic**. **Coverage is COMPLETE** — every
roadmap milestone across all 16 systems maps to ≥1 prompt and every prompt maps to exactly one primary
milestone (see [`coverage-matrix.md`](coverage-matrix.md)); **no cross-system DEPENDS-ON points at a
non-existent prompt.**
