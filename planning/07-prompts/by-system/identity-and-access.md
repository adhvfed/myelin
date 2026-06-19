# Phase 7 — Implementation Prompts: Identity & Access (myelin-identity)

> Prompt count: first pass **16** prompts (P-ID-01..P-ID-16) → finer-grained pass **35** prompts
> (P-ID-01..P-ID-35). Every bundled multi-deliverable prompt is split into single-deliverable, clean-context,
> independently-committable units; no milestone/contract/drill/floor from the first pass is dropped. (~2.2× the
> first-pass count where bundling existed.)
>
> Phase: `07-prompts/by-system`. The complete ordered set of clean-context, independently-committable coding
> prompts that operationalize the entire **identity-and-access** roadmap
> ([`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md)) into
> build tasks. Authored to the ledger template
> ([`../00-ledger-overview.md`](../00-ledger-overview.md) §2), in this system's own band order (M0 → M6). The
> consolidated index (`01-ledger-index.md`, Phase 7-B) interleaves these into the single global `P-<NNN>`
> sequence; the `P-ID-NN` ids below are this file's **local placeholders** — the index reassigns stable global
> ordinals and fixes DEPENDS-ON to global ids. Plain-text identifiers (no backticks-as-emphasis). Markdown only;
> no git commits by this document or its author.
>
> **Coverage (this file → roadmap):** every Identity milestone — M0 (lints + glue crate + envelope fields),
> M1 (the whole Id surface: authenticate, check + CaveatContext, list_objects SetExpr push-down + S8,
> write_tuples/zookie, delegation, mint_run_token, pseudonym shred, fail-static, the ReBAC engine + core
> hierarchy), M2 (first consumption: caveat-core promotion + watcher), M3/M4 (the five namespace fragments),
> M5 (30x authz surge + multi-cell + S8 tunables + E2E spine), M6 (dogfood) — maps to ≥1 prompt below. Each
> M1 floor and its follow-on are paired. Date: 2026-06-19.

---

## Canon every prompt in this file assumes (the shared reading set)

Each prompt re-states the precise subset it needs, but all assume the M0 substrate from
[`../00-ledger-overview.md`](../00-ledger-overview.md) §6: the Cargo workspace + the eight glue crates
(myelin-identity is one), the `serve(AppSpec)` harness (three ports, holder auto-registration, ResilientClient,
FailStatic), the transactional outbox + EventHandler template, the twelve committed lints, the
contract-coverage scanner, and the failure-injection harness (the 1x/10x/30x load generator + the
scoped-reversible dependency-break injector + the telemetry-assertion library reading contract 1.8). A prompt's
GATE/DRILLS rows are scenarios on that harness; a prompt's DEFINITION OF DONE requires all committed lints green
and the contract-coverage scanner passing. The frozen Identity architecture is
[`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md)
(sections cited per prompt); the frozen contracts are
[`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md)
§4 (rows 4.1–4.11, 1.8); the rationale is
[`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md)
(OQ-E, X-1, X-7, L-1). The Id drills are
[`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md)
rows ID-D1..ID-D9 (§4.2), with the strategy in
[`../../05-refined-shared-systems-architecture/testing-strategy/README.md`](../../05-refined-shared-systems-architecture/testing-strategy/README.md).

---

### P-ID-01 — Freeze the myelin-identity contract surface: the eleven trait signatures + SetExpr + CaveatContext

- **BAND.** M0.
- **ROADMAP MILESTONE.** ID-M0 (Id named in the substrate — the crate + the frozen ABI) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M0".
- **DEPENDS-ON.** The M0 substrate prompts that lay down the Cargo workspace + the eight glue-crate skeletons and freeze the EventEnvelope (2.1) and the outbox (substrate roadmap SUB-M0; ledger index assigns the global ids). This is the first myelin-identity prompt; it has no intra-Identity predecessor.
- **CANON DOCS (read these first, in full, before writing any code).**
  - [`../../VISION.md`](../../VISION.md) §3 (name-your-floors; agent-native; the strategy pattern), §4 (Rust default).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §5 (the ratchet — a contract that breaks every consumer's build *now*, never silently), §7 (reconcile cross-component contracts at the plan layer — field names + units up front).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §0 (the change list C1–C12), §3 (the polymorphic Principal model + kind discriminant), §7.1 (the frozen ListObjectsResult / SetExpr / ColRef shape — copy it byte-exact), §8.6 (the frozen CaveatContext shape), §11.1 (the full exposed-contract table — the signatures to carry).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) §4 rows 4.1–4.11.
  - [`../00-ledger-overview.md`](../00-ledger-overview.md) §6 (the glue-crate-as-compile-time-contract-carrier convention).
- **DELIVERABLE (what to build + exactly where in the repo).** In crate myelin-identity (the glue crate), define the public contract surface as compile-time carriers — types + trait method signatures, no bodies beyond `unimplemented!()`/stubbed defaults so consumers compile against them: the `Principal{tenant, region, principal_id, kind: PrincipalKind{Human|Agent|Service}, data_role, status}` record; `Credential`, `Zookie`, `CaveatContext{object, field?, transition?, attrs: Map<String,Literal>}`, `ListObjectsResult{Ids{ids: Vec<ObjectId>, zookie}|Filter{set_expr: SetExpr, zookie}}`, the `SetExpr` enum (All|None|Ids(Vec<ObjectId>)|NotIds(Vec<ObjectId>)|InRelation{relation: RelName, via_column: ColRef}|Union(Vec<SetExpr>)|Intersect(Vec<SetExpr>)|Difference(SetExpr,SetExpr)|TupleSet{index: AuthzIndexRef}) and `ColRef{table, column}` exactly as §7.1 freezes them; the trait method signatures for authenticate (4.1), check (4.2), list_objects (4.3), list_subjects + explain (4.4), delegation (4.5), write_tuples (4.6), mint_run_token + revoke (4.7), resolve_pseudonym + erase (4.8), the namespace-fragment admit type (4.9), the Consistency/zookie type (4.10), the FailStatic bound type (4.11). This prompt ships NO service, NO algorithm, and NO event tokens — it ships the frozen call-surface ABI; the iam.* event tokens + envelope projections are P-ID-02. Floor named: bodies are deferred to M1 (P-ID-06..P-ID-21) — this is the M0 band's purpose, not a hidden floor.
- **CONTRACTS TO IMPLEMENT.** Owns (signatures only, frozen shape): 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 4.8, 4.9, 4.10, 4.11.
- **GATE / DRILLS (quantified; must be green to call this done).** No Id drill runs (no service yet). Gate: myelin-identity compiles in the workspace; the contract-coverage scanner sees rows 4.1–4.11 with a provider stub + a consumer CDC slot (an uncommitted contract test is no contract test); any change to a signature here breaks dependent consumer crates' build at compile time (demonstrate by a deliberate temporary signature change in a scratch test → consumer crate fails to compile → revert).
- **TESTS (required).** Unit: a type-level/round-trip test that the SetExpr enum and CaveatContext serialize/deserialize stably (the names + variants are the wire contract). CDC: register the provider-stub + consumer-stub CDC pair slots for 4.1–4.11 so the coverage scanner is satisfied (bodies arrive in M1; the slot existing is what the scanner checks). No mutation floor (no logic yet).
- **DEFINITION OF DONE.** myelin-identity compiles; all eleven owned signatures exist to the frozen §7.1/§8.6/§11.1 shape; the contract-coverage scanner passes for rows 4.1–4.11; all twelve committed lints green; the floor (bodies deferred to M1) is named in writing. No threshold weakened. Committed.
- **COMMIT.** Header `P-<NNN> M0: freeze myelin-identity contract surface (signatures + SetExpr + CaveatContext)`; body lists the eleven owned contract rows frozen, the SetExpr/CaveatContext/ColRef shapes, and the named floor (bodies → M1). Branch first if on default. End with the Co-Authored-By trailer.

---

### P-ID-02 — Register the iam.* event tokens + their EventEnvelope projections (the erasure-vs-immutability envelope split)

- **BAND.** M0.
- **ROADMAP MILESTONE.** ID-M0 (the envelope fields — Id's iam.* tokens + opaque-principal attribution) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M0".
- **DEPENDS-ON.** P-ID-01 (the myelin-identity crate exists); the M0 substrate prompt that freezes the EventEnvelope (2.1) actor/subject/contains_personal_data/data_role fields and the taxonomy register.
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (GDPR-safe by construction; name-your-floors).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §7 (field names + units up front), §5 (the ratchet).
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure-vs-immutability — attribution by opaque principal_id, never erasable PII baked into the immutable envelope).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §3 (the opaque principal_id / erasable profile_ref split), §6 (iam.tuple_written is emitted via the outbox), §11.2 (the iam.* event set: iam.tuple_written, iam.role_granted, iam.break_glass).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 2.1 (EventEnvelope), row 1.8 (telemetry signal names: auth_decision_latency, cache_hit_ratio, staleness_age, revocation_lag, tuple_write_lag, reverse_index_lag).
- **DELIVERABLE.** In crate myelin-identity: register the `iam.*` event token constants (iam.tuple_written, iam.role_granted, iam.break_glass) in the taxonomy, and define their EventEnvelope projections using the already-frozen actor/subject/contains_personal_data/data_role fields — attribution by **opaque principal_id only** (the erasable profile_ref never enters the envelope), so the erasure-vs-immutability split is baked into the envelope shape at M0. Declare the 1.8 telemetry signal name constants Id owns (auth_decision_latency, cache_hit_ratio, staleness_age, revocation_lag, tuple_write_lag, reverse_index_lag) so later prompts assert against named signals, not literals. NO service, NO emit path (the emit path is P-ID-08 write_tuples). Floor named: the bodies that emit these tokens land in M1 (iam.tuple_written → P-ID-08).
- **CONTRACTS TO IMPLEMENT.** Consumed (declares dependency, no body): 2.1 EventEnvelope; owns the iam.* token + projection registration and the 1.8 signal-name constants.
- **GATE / DRILLS.** No drill (no service). Gate: the iam.* tokens + projections compile and are visible in the taxonomy; the control-plane-pii-free lint (wired in P-ID-03) will admit these projections (opaque-id only) and reject any projection carrying a name/email — assert the projection contains no PII field at compile time.
- **TESTS (required).** Unit: a round-trip test that each iam.* envelope projection carries actor/subject by opaque principal_id and contains_personal_data is set correctly; a test that no PII field is present in any iam.* projection. CDC: n/a (token registration, not an RPC). No mutation floor.
- **DEFINITION OF DONE.** The three iam.* tokens + their envelope projections + the 1.8 signal-name constants are registered and compile; the opaque-id-only attribution is enforced in the projection shape; the floor (emit bodies → M1) is named; all twelve committed lints green; coverage scanner green. Committed.
- **COMMIT.** Header `P-<NNN> M0: register iam.* event tokens + EventEnvelope projections`; body lists the three tokens, the opaque-id attribution split, the 1.8 signal constants, and the named floor (emit → M1). Co-Authored-By trailer.

---

### P-ID-03 — Wire the four Id-relevant architecture lints with red + green fixtures

- **BAND.** M0.
- **ROADMAP MILESTONE.** ID-M0 (the four Id-relevant lints in the M0 ratchet) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M0".
- **DEPENDS-ON.** P-ID-01 (the myelin-identity crate exists to lint against), P-ID-02 (the iam.* projections the control-plane-pii-free lint guards); the M0 substrate prompt that establishes the lint harness + the contract-1.6 lint framework.
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (GDPR-safe by construction; name-your-floors).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §5 (the ratchet — each lint ships a red-fixture proving it rejects + a green-fixture proving it admits; wired loud, never `... || true`), §2 (tenant-leak/IDOR is stop-the-bleeding).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §1 (the three platform invariants Id never breaks — (tenant,region) in every partition key; control-plane PII-free; residency-pinned), §2 (the store map S1/S2/S3/S8 — the tables the lints constrain).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 1.6 (the twelve lints).
- **DELIVERABLE.** Wire the four Id-relevant lints from the contract-1.6 set into CI as committed gates, each with a red-fixture (a code sample that MUST be rejected) + a green-fixture (a code sample that MUST be admitted), under tests in crate myelin-identity (or the lint crate's fixtures dir, per the substrate convention): (1) `tenant-predicate` — every query against S1/S2/S3/S8 must carry a `(tenant, region)` predicate; red-fixture = a query missing it; green-fixture = the same query with it. (2) `no-untagged-personal-data` — S1 profile PII and S2 real-identity columns must carry `#[personal_data]`; red-fixture = an untagged PII field; green-fixture = the tagged field. (3) `residency-pin` — no cross-region Id read path; red-fixture = a read crossing region; green-fixture = a region-pinned read. (4) `control-plane-pii-free` — Id's emitted events/routing carry opaque principal_id only, never PII; red-fixture = an iam.* event leaking a name/email; green-fixture = the opaque-id event from P-ID-02. Wire each loud-never-swallowed (no `|| true`). These four lints are one ratchet unit (the M0 committed-gate set for Id); they ship together so the no-IDOR/no-untagged-PII surface is closed in one commit.
- **CONTRACTS TO IMPLEMENT.** Consumed/realized: 1.6 (the four Id-relevant lints), constraining the stores defined by 11.1 (OLTP/RLS) and the partition key 12.1.
- **GATE / DRILLS.** All four lints green-with-both-fixtures: each red-fixture fails the build (the lint fires), each green-fixture passes (the lint admits) — the four are committed CI gates from M0 and stay green forever. Quantified: 4/4 lints emit a green artifact on the green-fixtures and a (captured, expected) failure on the red-fixtures.
- **TESTS (required).** Unit: the eight fixtures (4 red + 4 green) ARE the tests — each asserts the lint's verdict. CDC: n/a (lints, not a contract RPC). No mutation floor.
- **DEFINITION OF DONE.** The four lints are wired into CI loud-never-swallowed; all eight fixtures present and asserting the correct verdict; all twelve committed lints green on the workspace; the contract-coverage scanner passes. No threshold weakened (a lint is never softened to admit a red-fixture). Committed.
- **COMMIT.** Header `P-<NNN> M0: wire the four Id lints with red+green fixtures`; body lists the four lints, each with its red+green fixture verdicts. Co-Authored-By trailer.

---

### P-ID-04 — Identity service shell: the AppSpec the harness wires (boot → migrate → relay → consumers → three ports)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (the service shell Id boots from — 1.1/1.2/1.3) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1".
- **DEPENDS-ON.** P-ID-01 (the frozen contract surface); the M0 substrate prompts (serve(AppSpec) harness, the outbox, the lints).
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (agent-native, GDPR-safe by construction).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §2 (order-by-non-negotiability — Id is the dependency root), §3 (prove-it; observability is part of the pass).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §1 (the dependency root; the internal-RPC surface is how everyone calls check/list_objects).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 1.1/1.2/1.3 (serve(AppSpec), the three surfaces, liveness≠readiness).
- **DELIVERABLE.** In crate myelin-identity, build the Identity service as an AppSpec the harness wires (NOT a hand-rolled main): boot → migrate → outbox relay → consumers → the three ports (public / internal-RPC / metrics-health), with liveness ≠ readiness and graceful drain. The internal-RPC surface is the one every other service calls check/list_objects on. No store, no algorithm yet — this is the bootable shell with empty handler slots wired for the M1 contract bodies (authenticate P-ID-05, check P-ID-09, etc.). Floor named: the handler bodies arrive in their own M1 prompts; the shell ships with fail-closed stubs that deny until wired.
- **CONTRACTS TO IMPLEMENT.** Consumed/wired: 1.1 (serve(AppSpec)), 1.2 (three surfaces), 1.3 (liveness≠readiness).
- **GATE / DRILLS.** The Identity AppSpec boots under the harness; the three ports answer (public, internal-RPC, metrics-health); readiness gates on migrate-complete; a stubbed check returns Deny (fail-closed) until P-ID-09 wires it. Quantified: 3/3 ports up; readiness=false until migrations applied; the metrics port emits the harness liveness signal.
- **TESTS (required).** Unit: the AppSpec boots and the three ports bind; liveness≠readiness (readiness false pre-migrate); a stubbed handler fail-closes. CDC: n/a (no contract body yet — the shell). No mutation floor (no logic yet).
- **DEFINITION OF DONE.** The Identity AppSpec boots under the harness with the three ports + liveness≠readiness + graceful drain; the fail-closed-stub floor is named; lints green; coverage scanner green. Committed.
- **COMMIT.** Header `P-<NNN> M1: Identity service shell (AppSpec + three ports)`; body lists 1.1/1.2/1.3 wired, the fail-closed-stub floor + follow-ons. Co-Authored-By trailer.

---

### P-ID-05 — The S1 principal store: RLS-partitioned, per-tenant/per-subject DEK, PII-tagged, holder-registered

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (the S1 principal store) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.2 rows 11.1/11.3/12.1/10.1.
- **DEPENDS-ON.** P-ID-04 (the service shell), P-ID-03 (the tenant-predicate + no-untagged-personal-data lints); the M1 storage prompts that ship 11.1 (OLTP tier + RLS + encrypted columns), 11.3 (KMS hierarchy), 12.1 ((tenant,region) partition key), 10.1 (PersonalDataHolder spine) — co-built in the same band.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §2 (Id is the dependency root), §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §2 (the S1 store row: principals, orgs/teams/projects, credentials, tokens, SSO/SCIM links, agent-identity records; `(tenant,region)` shard; per-tenant DEK + per-subject sub-key for profile PII), §1 (the three invariants), §3 (the opaque principal_id / erasable profile_ref split).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 11.1, 11.3, 12.1, 10.1.
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §X-7 (the opaque-principal_id / profile-ref split).
- **DELIVERABLE.** In crate myelin-identity: add the S1 principal store (principals, orgs/teams/projects, credentials, tokens, SSO/SCIM links, agent-identity records) as a tenant-scoped RLS Postgres-class store partitioned `(tenant, region)`, per-tenant DEK + per-subject sub-key for profile PII, `#[personal_data]` tags on every PII column, the opaque-stable `principal_id` separate from the erasable `profile_ref`, auto-registered as a PersonalDataHolder via the harness (10.1/1.4). Forward-only online migrations. NO authenticate body (P-ID-06), NO tuples (P-ID-07). This prompt ships the store + its holder registration + its PII tagging only.
- **CONTRACTS TO IMPLEMENT.** Consumed/wired: 11.1 (OLTP/RLS), 11.3 (KMS per-tenant DEK + per-subject sub-key), 12.1 ((tenant,region) partition), 10.1 (PersonalDataHolder on S1).
- **GATE / DRILLS.** `tenant-predicate` lint green on every S1 query; `no-untagged-personal-data` lint green on S1 (red on any untagged PII field — assert the lint fires on a deliberately-untagged scratch field, then remove it). S1 auto-registers as a holder (assert it appears in the holder list). Quantified: 0 S1 queries without a (tenant,region) predicate; S1 present in the PersonalDataHolder registry.
- **TESTS (required).** Unit: an S1 row round-trips under RLS scoped to (tenant,region); a cross-tenant read returns nothing; profile PII is encrypted under the per-subject sub-key; the principal_id is opaque + stable while profile_ref is separable. CDC: n/a (store, not an RPC contract — the consuming contract bodies bring their own CDC). Mutation floor: the RLS scoping + the per-subject-key encryption boundary are mandatory-core — state and meet the mutation-score floor (a mutation dropping the tenant predicate MUST be caught).
- **DEFINITION OF DONE.** S1 exists, is RLS-partitioned `(tenant, region)`, holder-registered, PII-tagged, principal_id/profile_ref split; lints green; mutation floor met; coverage scanner green. Committed.
- **COMMIT.** Header `P-<NNN> M1: S1 principal store (RLS + per-subject DEK + holder)`; body lists 11.1/11.3/12.1/10.1 wired on S1, the PII tagging, the principal_id/profile_ref split, the mutation score. Co-Authored-By trailer.

---

### P-ID-06 — authenticate: the v1 human/SSO credential set (OIDC, SAML, SCIM, passkey, SSH)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (authenticate 4.1, the human/SSO half of the v1 floor) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.1.
- **DEPENDS-ON.** P-ID-05 (the S1 principal store), P-ID-01 (the frozen Principal/Credential signatures).
- **CANON DOCS.**
  - [`../../VISION.md`](../../VISION.md) §3 (agent-native, GDPR-safe by construction).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §2 (the IDOR floor is stop-the-bleeding), §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §3 (the polymorphic Principal), §4 (the auth surfaces — SAML 2.0 / OIDC / SCIM 2.0 / WebAuthn-FIDO2 passkeys / SSH; tenant from the verified credential, never the URL path, ID-3).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.1; row 1.8 (auth_decision_latency).
- **DELIVERABLE.** In crate myelin-identity: implement `authenticate(credential) → Principal` for the v1 human/SSO credential set — OIDC, SAML 2.0, SCIM 2.0, WebAuthn/FIDO2 passkeys, SSH-pubkey — each resolving to the polymorphic Principal{kind, tenant, region, data_role, status}. Tenant is taken from the verified credential, never the URL path (ID-3, the IDOR floor). Emit auth_decision_latency telemetry per request. The capability-token credentials (PAT/CI/agent) and machine-identity (deploy-key/per-job) are P-ID-07; this prompt ships the human/SSO surfaces. Floor named: hardware-attested device binding + full passkey-sync governance + SAML SLO are the P5/P6 follow-on; SCIM deprovision is the v1 authoritative revocation path — record this in writing.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.1 authenticate (the human/SSO half). Consumed/wired: 11.1, 12.1, 1.8.
- **GATE / DRILLS.** A scoped drill: authenticate with an SSO credential whose tenant ≠ the URL-path tenant → resolves to the credential's tenant, never the path's (the ID-3 floor; assert the resolved Principal.tenant = credential's tenant, count of path-derived tenants = 0). Telemetry: auth_decision_latency emits per request. (ID-D3 full cross-tenant proof lands once check/list exist, P-ID-15.)
- **TESTS (required).** Unit: one happy-path test per credential kind (OIDC, SAML, SCIM, passkey, SSH) resolving to the correct Principal{kind, tenant, region}; a test that tenant comes from the credential not the path. CDC: the provider+consumer pair for 4.1 (a gateway-side consumer calling authenticate). Mutation floor: authenticate's credential-verification + tenant-derivation are mandatory-core — state and meet the mutation-score floor for the auth module (a mutation deriving tenant from the path MUST be caught).
- **DEFINITION OF DONE.** authenticate resolves all five v1 human/SSO credential kinds to the polymorphic Principal with tenant-from-credential; the named auth floor (hardware-attestation/passkey-sync/SLO → P5/P6) is recorded; lints green; CDC for 4.1 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: authenticate (v1 human/SSO credential set)`; body lists 4.1 (human/SSO half), the tenant-from-credential proof, the named auth floor + follow-on, the mutation score. Co-Authored-By trailer.

---

### P-ID-07 — authenticate: the capability-token + machine-identity credential set (PAT/CI/agent + deploy-key/per-job)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (authenticate 4.1, the token/machine-identity half) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.1.
- **DEPENDS-ON.** P-ID-06 (the authenticate surface + Principal resolution), P-ID-05 (S1 token records).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §2 (the security floor — an agent must not exceed a human), §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §4 (token format = attenuable bearer PASETO/JWT + macaroon/biscuit caveat chains + DPoP sender-constrains long-lived PATs; revocation = denylist S7 + short TTL), §3 (machine-identity C6: repo-scoped deploy-key → Service principal; per-job token → Service principal, self-hosted-runner scoped to one tenant's SelfHosted jobs).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.1.
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §1 (the machine-identity resolution pins).
- **DELIVERABLE.** In crate myelin-identity: extend `authenticate` to the three capability-token types (PAT, CI-job, agent-run) and machine-identity (repo-scoped deploy-key → Service principal whose authority ceiling is one repo; per-job token → Service principal, self-hosted-runner scoped to one tenant's SelfHosted jobs — the scope flag). Capability tokens = attenuable PASETO/JWT envelopes with macaroon/biscuit caveat chains; DPoP sender-constrains long-lived PATs; revocation surface = denylist (S7 stub here — full S7 in P-ID-13) + short TTL. The per-job-token mid-resume re-mint is wired in P-ID-17 (mint_run_token). Floor named: the S7 denylist is a stub here (full wiring → P-ID-13); record it.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.1 authenticate (the token/machine-identity half). Consumed: 11.1 (S1 token records).
- **GATE / DRILLS.** A self-hosted-runner token cannot resolve a cross-tenant Principal (assert the resolved Principal is scoped to the one tenant; cross-tenant resolution count = 0). An attenuated PAT's caveat chain narrows authority (assert the attenuated token resolves to a strictly-smaller authority than its parent). Telemetry: auth_decision_latency emits. Quantified: 0 cross-tenant runner resolutions; attenuation is monotone (never amplifies).
- **TESTS (required).** Unit: one happy-path test per credential kind (PAT, CI-job, agent-run, deploy-key, per-job) resolving to the correct Principal{kind, tenant, region}; a deploy-key resolves to a repo-scoped Service principal; a self-hosted-runner token cannot act cross-tenant; an attenuated PAT's caveat chain narrows authority. CDC: re-affirm the 4.1 provider+consumer pair exercises a token credential. Mutation floor: the attenuation-never-amplifies check + the self-hosted-runner scope are mandatory-core — state and meet the floor.
- **DEFINITION OF DONE.** authenticate resolves all five token/machine-identity credential kinds; deploy-key is repo-scoped; self-hosted-runner is one-tenant-scoped; PAT attenuation is monotone; the S7-stub floor → P-ID-13 named; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: authenticate (capability-token + machine-identity)`; body lists 4.1 (token/machine half), the runner one-tenant scope, the deploy-key repo scope, the S7-stub floor → P-ID-13, the mutation score. Co-Authored-By trailer.

---

### P-ID-08 — The S3 ReBAC tuple store + write_tuples/zookie (the only emit path, via the outbox)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (write_tuples 4.6, zookie 4.10-write-half, the S3 store) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1".
- **DEPENDS-ON.** P-ID-01 (frozen write_tuples signature), P-ID-04 (the service shell), P-ID-02 (the iam.tuple_written token + projection), the M0 outbox.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; quantified gate), §5 (no-raw-publish — emit only via the outbox).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §6 (tuple shape `RelationTuple{tenant, region, object, relation, subject, caveat?, zookie, expires_at?}`; `(tenant,region)` + object-id-hash partition; per-run grants are auto-expiring tuples; event-sourced; emits iam.tuple_written via the outbox — the only path; reindex-from-source), §8.4 (zookie / read-your-writes — write returns the zookie to stamp on the object), §2 (S3 row).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.6, 4.10 (the write half); rows 2.2/2.4 (the outbox emit path).
- **DELIVERABLE.** In crate myelin-identity: (1) the S3 ReBAC tuple store (SpiceDB-class), `RelationTuple{tenant, region, object, relation, subject, caveat?, zookie, expires_at?}`, `(tenant, region)` + object-id-hash partition, per-tenant DEK, holder-registered, NO cross-tenant tuple and no cross-tenant query path. (2) `write_tuples([Δtuple], precondition?) → zookie` (4.6): atomic write → returns the zookie → emits iam.tuple_written via the outbox (the ONLY emit path — the no-raw-publish lint forbids any other), carrying the write's zookie for S8's watermark. (3) The zookie write-half of 4.10: write_tuples returns the monotonically-advancing zookie to stamp on the object (`page.acl_zookie`, Chat membership). NO check engine (P-ID-09), NO S8 (P-ID-11). Floor named: the read-your-writes consistency *read* half (S8 watermark) lands in P-ID-12; record it.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.6 write_tuples, 4.10 (the zookie write-half). Consumed: 2.2/2.4 (outbox emit + the EventHandler template).
- **GATE / DRILLS.** `tenant-predicate` lint green on every S3 query; `no-raw-publish` lint green (write_tuples emits only via the outbox). Quantified: iam.tuple_written observed on the outbox iff the tuple write committed (0 emits without a committed write, 0 committed writes without an emit); the returned zookie advances monotonically.
- **TESTS (required).** Unit: write_tuples is atomic + returns a monotonically-advancing zookie; the precondition is honoured (a failed precondition aborts the write); the emit is via the outbox only; a per-run grant is an auto-expiring tuple (expires_at == run life); no cross-tenant tuple is writable. CDC: provider+consumer pair for 4.6 (a role-compile caller). Mutation floor: write_tuples' atomicity + the outbox-only emit are mandatory-core — state and meet the floor (a mutation that emits outside the outbox, or commits without emitting, MUST be caught).
- **DEFINITION OF DONE.** S3 exists, partitioned + RLS + holder-registered + no-cross-tenant-path; write_tuples atomic + outbox-emitting + zookie-returning; the read-half-watermark floor → P-ID-12 named; lints (tenant-predicate, no-raw-publish) green; CDC for 4.6 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: S3 tuple store + write_tuples + zookie`; body lists 4.6 + 4.10-write-half, the outbox-only emit, the watermark-read floor → P-ID-12, the mutation score. Co-Authored-By trailer.

---

### P-ID-09 — check: the depth-bounded Zanzibar userset-rewrite evaluation (fail-closed, zookie-snapshot)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (check 4.2 engine) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.2.
- **DEPENDS-ON.** P-ID-08 (S3 + the zookie), P-ID-01 (the frozen check/CaveatContext signature), P-ID-04 (the shell's check slot).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; quantified gate), §7 (one primitive — no bespoke check path per subsystem).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §8 (the check algorithm: depth-bounded userset-rewrite, memoised-per-request, fail-closed on genuine uncertainty, evaluated at the zookie snapshot; the three-layer cache faces), §8.6 (the CaveatContext rider with the literal-only floor for M1).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.2.
- **DELIVERABLE.** In crate myelin-identity: `check(subject, permission, object, zookie?, caveat?) → Allow|Deny|Conditional` (4.2): the depth-bounded userset-rewrite Zanzibar evaluation, memoised-per-request, fail-closed on genuine uncertainty, evaluated at the zookie snapshot. The CaveatContext rider is wired with a **LITERAL-ONLY** predicate floor (the full QueryAst core is P-ID-22/M2). This prompt evaluates against the raw S3 tuples (the namespace engine that compiles fragments is P-ID-10; check here resolves direct + simple inherited relations as the core engine matures alongside). Floor named: the literal-only CaveatContext predicate floor → its follow-on P-ID-22; record it.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.2 check (engine + CaveatContext literal-only floor). Consumed: 4.6 (reads tuples written by write_tuples).
- **GATE / DRILLS.** check is fail-closed on a malformed/uncertain query (assert Deny, not Allow, on uncertainty — count of silent-allows-on-uncertainty = 0); evaluated at the zookie snapshot (a check at an older zookie does not see a newer tuple). Quantified: 0 allows on genuine uncertainty; the evaluation is depth-bounded (a deliberately deep userset chain is bounded, never unbounded recursion).
- **TESTS (required).** Unit: check resolves a direct grant (Allow) and a missing grant (Deny); fail-closed on a malformed query; a literal CaveatContext redacts/gates correctly; the evaluation is memoised-per-request (the same subproblem is computed once); depth-bounded. CDC: provider+consumer pair for 4.2 (a write-path caller gating an action). Mutation floor: check's fail-closed branch + the depth bound are mandatory-core — state and meet the floor (a mutation turning Deny-on-uncertainty into Allow MUST be caught).
- **DEFINITION OF DONE.** check is fail-closed + zookie-snapshot + depth-bounded + memoised + literal-only-caveat-floor (floor + follow-on P-ID-22 named); lints green; CDC for 4.2 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: check (Zanzibar userset-rewrite, fail-closed)`; body lists 4.2 engine, the literal-only caveat floor → P-ID-22, the fail-closed assertion, the mutation score. Co-Authored-By trailer.

---

### P-ID-10 — The ReBAC namespace engine + the fragment-admit contract + the org/team/project core hierarchy

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (the ReBAC engine + admit-contract + core hierarchy 4.9-engine) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1".
- **DEPENDS-ON.** P-ID-09 (check evaluates against the compiled namespace), P-ID-08 (S3 tuples).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §7 (one primitive — every visibility need reduces to four userset operators, no bespoke check path).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §5 (the Zanzibar namespace-configuration model: relations + permissions = union/intersection/exclusion + tuple-to-userset rewrites; the org→team→project core hierarchy with `parent_team->view` inheritance; the four-operator design rule; the fragment-admit contract).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.9 (the engine + admit-contract + core hierarchy).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D3 (§4.2 row).
- **DELIVERABLE.** In crate myelin-identity: the ReBAC engine + the fragment-admit contract (4.9-engine: how a subsystem declares relations + permissions compiled into one cell schema, validated + admitted at build time, Id never inventing object ids) + the core org→team→project hierarchy namespaces with the `parent_team->view` tuple-to-userset inheritance rewrite. The four Zanzibar userset operators (union/intersect/exclusion/tuple-to-userset) each evaluate through check (P-ID-09). Floor named: the engine ships with ONLY the org/team/project core — the five per-subsystem fragments are the M3/M4 follow-on (P-ID-24..P-ID-30); record it.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.9 (engine + admit-contract + core hierarchy).
- **GATE / DRILLS.** ID-D3 (F2): cross-tenant check via a path spoof → 0 cross-tenant tuples readable (the engine resolves only within (tenant,region)); `tenant-predicate` lint green. Telemetry green artifact: cross-tenant-count signal = 0. Quantified: 0 cross-tenant tuples in any check result for a spoofed path.
- **TESTS (required).** Unit: check on the org→team→project hierarchy resolves inheritance correctly (a project-reader granted via team membership Allows; a non-member Denies); the four Zanzibar operators each evaluate; the admit-contract validates a well-formed fragment and rejects a malformed one; a fragment cannot mint object ids. CDC: provider+consumer pair for 4.9-engine (a fragment-declaring caller). Drill: the ID-D3 scenario on the failure-injection harness (spoofed-path cross-tenant read → 0). Mutation floor: the userset-rewrite + the cross-tenant scoping are mandatory-core — state and meet the floor.
- **DEFINITION OF DONE.** The engine + admit-contract + org/team/project core present; the four operators evaluate; ID-D3 emits cross-tenant-count = 0 (dated green artifact); the engine-only floor → the five fragments (P-ID-24..P-ID-30) named; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: ReBAC engine + fragment-admit contract + core hierarchy`; body lists 4.9-engine, ID-D3 green (cross-tenant 0), the engine-only floor → the five fragments, the mutation score. Co-Authored-By trailer.

---

### P-ID-11 — list_objects: the return-shape dispatch + the S4 Ids materialise path + S8 reverse index

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (list_objects 4.3 Ids path + the S8 store) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.3.
- **DEPENDS-ON.** P-ID-10 (the engine), P-ID-08 (S3 + the iam.tuple_written event S8 consumes), P-ID-01 (the frozen SetExpr/ListObjectsResult shape).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; observability), §2 (the leak-free pre-filter is stop-the-bleeding).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §7.1 (the frozen return shape — Ids|Filter under the cardinality cap), §2 (the S4 + S8 store rows; both reindex-from-source faces of S3), §8.7 (S8 carries a revision_watermark).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.3, 1.8 (reverse_index_lag).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §OQ-E (the push-down rationale).
- **DELIVERABLE.** In crate myelin-identity: (1) `list_objects(subject, permission, type, zookie?) → Ids{ids, zookie} | Filter{set_expr, zookie}` (4.3) — the return-shape dispatch: the `Ids` materialise path via S4 (the flattened reachable-set index, small bounded sets, default under a cardinality cap), the `Filter` path stubbed to return a SetExpr (the full lowering is P-ID-12). (2) S8, the per-tenant authz reverse index: a materialised `(subject, relation, object_id)` projection of S3 + a `revision_watermark` column, partitioned `(tenant, region)` + object-type, per-tenant-only with NO cross-tenant query path, fed off the bus by an EventHandler consuming iam.tuple_written (carrying the write's zookie as the watermark), holder-registered (a NEW PersonalDataHolder — it references subjects). Emit reverse_index_lag telemetry. Floor named: the SetExpr→SQL lowering (P-ID-12), the watermark consistency path (P-ID-12), and list_subjects (P-ID-13) are the follow-ons; the Ids↔Filter cardinality cap is a measured tunable (default-to-beat written to the thresholds file now; re-measured at M5, P-ID-31) — the SHAPE is frozen, only the threshold is open. Record both floors.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.3 (the Ids path + the return-shape dispatch). Consumed: 2.4 (the EventHandler feeding S8 from iam.tuple_written), 11.1 (S8 as a co-located projection store), 10.1 (S8 as a new holder), 12.1 ((tenant,region)+type partition).
- **GATE / DRILLS.** S8 ingests iam.tuple_written and advances the watermark (assert reverse_index_lag emits and the watermark advances); the Ids path returns the correct reachable set under the cardinality cap; `tenant-predicate` lint green on S8 queries; S8 auto-registers as a holder. Quantified: S8 watermark advances on each iam.tuple_written; reverse_index_lag emitted; 0 cross-tenant S8 rows.
- **TESTS (required).** Unit: list_objects returns Ids for a small set and Filter (stub) above the cap; the Ids↔Filter switch honours the cardinality cap; S8 ingests iam.tuple_written and advances the watermark; S8 is holder-registered + partitioned + no-cross-tenant-path. CDC: provider+consumer pair for 4.3 (a list consumer). Mutation floor: the cardinality-cap dispatch + the S8 watermark-advance are mandatory-core — state and meet the floor.
- **DEFINITION OF DONE.** list_objects returns Ids|Filter to the frozen shape; S4 + S8 exist (S8 holder-registered, partitioned, no-cross-tenant-path, fed off the bus, carrying the watermark); reverse_index_lag emits; the cardinality-cap floor → P-ID-31 + the lowering/watermark/list_subjects floors → P-ID-12/P-ID-13 named; lints green; CDC for 4.3 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: list_objects Ids path + S4/S8 reverse index`; body lists 4.3 (Ids + dispatch), S8 holder-registered + watermark-carrying, the cardinality-cap floor → P-ID-31, the lowering floor → P-ID-12, the mutation score. Co-Authored-By trailer.

---

### P-ID-12 — list_objects: the SetExpr no-N+1/no-post-filter lowering + the S8 watermark consistency path (ID-D4, ID-D7)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (list_objects 4.3 Filter lowering + 4.10 watermark) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1" (the load-bearing crux).
- **DEPENDS-ON.** P-ID-11 (the return-shape dispatch + S8 + the watermark column), P-ID-08 (the zookie write-half).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; observability), §2 (silent IDOR is stop-the-bleeding).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §7.2 (the no-N+1/no-post-filter lowering — Ids/NotIds → IN/NOT IN under the cap; InRelation/TupleSet → the JOIN against S8 `authz_visible ON av.object_id = consumer.id AND av.subject AND av.relation`; Union/Intersect/Difference → AND/OR/EXCEPT), §7.3 (the five id-column mapping), §7.4 (consistency + the S8 revision watermark), §8.7 (the watermark: at-or-after → JOIN serves; behind → wait-or-fall-back-to-check).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.3, 4.10; row 1.8 (reverse_index_lag).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D4, ID-D7 (§4.2 rows).
- **DELIVERABLE.** In crate myelin-identity: (1) the SetExpr lowering exactly per §7.2 — Ids/NotIds → IN/NOT IN under the cardinality cap; InRelation{relation, via_column} + TupleSet{index} → the JOIN against authz_visible keyed on the consumer's own id column; Union/Intersect/Difference → AND/OR/EXCEPT — replacing the P-ID-11 Filter stub with the real consumer-composable lowering. (2) The S8 watermark consistency path (8.7/7.4): a zookie-stamped scan requiring a fresher revision than S8's watermark waits or falls back to per-row check rather than serving stale (the new-enemy guard) — this is the read-half of 4.10 that P-ID-08 floored. Floor named: none new — this CLOSES the SetExpr-lowering and the watermark-read floors opened in P-ID-11/P-ID-08.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.3 (the Filter lowering), 4.10 (the S8 watermark read-half).
- **GATE / DRILLS.** ID-D4 (F1): a confidential issue / overridden page / private channel must be ABSENT from any list_objects result for an unauthorized viewer, INCLUDING the Filter-lowered S8 JOIN result and under zookie staleness → zero-escape counter = 0. ID-D7 (F8): revoke, immediately re-read with the post-revoke zookie → no stale allow; assert the S8 JOIN waits or falls back to check rather than serving the stale grant → zookie-watermark-honoured signal. Quantified: 0 leaked objects across both Ids and Filter paths; 0 stale allows post-revoke.
- **TESTS (required).** Unit: each SetExpr variant lowers to the correct SQL (Ids → IN; NotIds → NOT IN; InRelation/TupleSet → the authz_visible JOIN; Union/Intersect/Difference → AND/OR/EXCEPT); a stale-S8 scan with a fresher zookie falls back to check; the lowering is no-N+1 (one query). CDC: provider+consumer pair for 4.3 (a board/list consumer pushing down a Filter against its own id column). Drill: ID-D4 and ID-D7 scenarios on the harness. Mutation floor: the SetExpr lowering + the watermark fall-back branch are mandatory-core — a mutation that drops the watermark check or admits a leaked id MUST be caught.
- **DEFINITION OF DONE.** The lowering is no-N+1/no-post-filter for every SetExpr variant; the watermark fall-back works; ID-D4 (zero-escape = 0) and ID-D7 (watermark honoured) emit dated green artifacts; the SetExpr-lowering + watermark-read floors recorded CLOSED; lints green; CDC for 4.3 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: list_objects SetExpr lowering + S8 watermark`; body lists 4.3 lowering + 4.10 watermark-read, ID-D4 (zero-escape 0) + ID-D7 (watermark honoured) green, the floors closed, the mutation score. Co-Authored-By trailer.

---

### P-ID-13 — list_subjects + explain: the SubjectTree/RewriteTrace served by S8 at 50k-member density

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (list_subjects 4.4 + explain) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.4.
- **DEPENDS-ON.** P-ID-11 (S8), P-ID-12 (the S8 watermark), P-ID-10 (the engine).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it at density).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §7.5 (list_subjects at 50k density via S8; the Zanzibar Expand API + explain → RewriteTrace).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.4.
- **DELIVERABLE.** In crate myelin-identity: `list_subjects(object, permission, zookie?) → SubjectTree` + `explain(...) → RewriteTrace` (4.4) served by S8, performant at 50k-member density. The `watcher`-relation read-fanout path is declared in M2 (P-ID-23); this prompt ships the engine + density path. Floor named: the 50k-density proof against a real watchable subsystem lands with the watcher relation (P-ID-23) and Chat channels (M4); the engine path is exercised here against synthetic density.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.4 list_subjects/explain. Consumed: S8 (the reverse index).
- **GATE / DRILLS.** list_subjects at synthetic 50k-member density returns within budget (assert the latency budget for the 50k case); explain returns a RewriteTrace for a resolved permission. Quantified: 50k-density list_subjects within the named budget; explain trace is non-empty + correct.
- **TESTS (required).** Unit: list_subjects returns the correct subject tree for a small object; explain returns a correct RewriteTrace; the 50k-density synthetic case returns within budget. CDC: provider+consumer pair for 4.4 (an admin-inspector / HITL-approver-set consumer). Mutation floor: the expand resolution is core — state and meet the floor.
- **DEFINITION OF DONE.** list_subjects + explain served by S8; 50k-density within budget; the watcher-fanout floor → P-ID-23 named; lints green; CDC for 4.4 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: list_subjects + explain (S8, 50k density)`; body lists 4.4, the density budget met, the watcher-fanout floor → P-ID-23, the mutation score. Co-Authored-By trailer.

---

### P-ID-14 — The revocation list (S7) + idempotent revoke + the SCIM-disable revocation path (ID-D1)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (revoke 4.7-revoke + S7 + the disabled-user surface) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1".
- **DEPENDS-ON.** P-ID-06/P-ID-07 (authenticate + tokens + the S7-stub), P-ID-09 (check).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it — disabled-user-in-5-min is a quantified gate).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §11 (the lifecycle/revocation flows: SCIM-disable, denylist S7, idempotent revoke even on crash, per-run token auto-expiring tuples), §2 (S7 row: Redis/Valkey + PG mirror).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.7 (revoke), 1.8 (revocation_lag).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D1 (§4.2 row).
- **DELIVERABLE.** In crate myelin-identity: (1) S7, the revocation list / token denylist (Redis/Valkey + PG mirror): revoked jtis, suspended principals, per-run agent-token TTLs; partitioned `(tenant, region)`. (2) `revoke(jti|principal_id)` (4.7) idempotent even on crash. (3) Wire the SCIM-disable revocation path so every surface (UI/API/git-wire/agent) denies within N = 5 min. Emit revocation_lag telemetry. The fail-static cache S6 (the zookie-bypass interplay) is P-ID-15; this prompt ships S7 + revoke + the disable path. Floor named: none new — S6's interaction with the denylist is the next prompt's concern.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.7-revoke (idempotent revoke + S7). Consumed: check (the deny path reads the denylist).
- **GATE / DRILLS.** ID-D1 (F8): SCIM-disable a user → every surface denies within N = 5 min; token TTL + denylist all ≤ the bound → deny-latency histogram. Quantified: deny-latency p100 ≤ 5 min; revoke is idempotent across a simulated crash (a double-revoke is a no-op).
- **TESTS (required).** Unit: a revoked jti is denied by check; revoke is idempotent across a simulated crash; a SCIM-disable propagates to every surface within the bound; a per-run token auto-expires at run-life. CDC: provider+consumer pair for 4.7-revoke (a gateway/agent caller checking the denylist). Drill: ID-D1 (SCHED) on the harness. Mutation floor: the idempotent-revoke + the deny-on-denylisted branch are mandatory-core — state and meet the floor.
- **DEFINITION OF DONE.** S7 exists; revoke is idempotent + crash-safe; SCIM-disable denies within 5 min; ID-D1 (SCHED) emits a dated green artifact (deny-latency ≤ 5 min); lints green; CDC for 4.7-revoke passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: revocation list (S7) + idempotent revoke + SCIM-disable`; body lists 4.7-revoke, S7, ID-D1 green (deny ≤ 5 min), the mutation score. Co-Authored-By trailer.

---

### P-ID-15 — The fail-static cache (S6) + the 4.11 staleness bound + the zookie-bypass: ID-D2

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (the fail-static cache 4.11 + S6 + the zookie-bypass half of 4.10) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1".
- **DEPENDS-ON.** P-ID-09 (check), P-ID-12 (the zookie surface), P-ID-14 (S7 + revoke); the M0 FailStatic<T> + ResilientClient primitives (1.9/1.10).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; observability is part of the pass), §8 (decision-shaped: the W bound is `[OPEN — LEGAL]` — a sketch + sign-off, the floor does not wait).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §10 (the fail-static availability cache: correctness stays fail-closed, availability fails static; bounded-staleness `{actor_active, coarse_grants}` in S6; zookie-stamped reads bypass S6; the bound `static_max ≤ revocation SLA` and `≥ agent/CI token TTL`, W = 5 min default-to-beat, the `[OPEN — LEGAL]` L-1 ratification), §2 (S6 row: Redis/Valkey, NEVER source of truth).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.11, 4.10 (the zookie-bypass half), 1.8 (cache_hit_ratio, staleness_age), 1.9/1.10.
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D2 (§4.2 row).
- **DELIVERABLE.** In crate myelin-identity: (1) S6, the fail-static cache built on the M0 FailStatic<T> primitive: bounded-staleness `{actor_active, coarse_grants}` + a decision cache, NEVER a source of truth, keyed `(tenant, region, subject)`, TTL ≤ the revocation SLA. Correctness stays fail-closed (deny when genuinely unsure); availability fails static (an Id-dependency hiccup keeps already-authenticated traffic alive on the cache). Zookie-stamped reads BYPASS S6 (fail-closed-or-wait — the 4.10 bypass half); default-consistency reads are served static during a hiccup. (2) The fail-static bound (4.11): structurally enforce `static_max ≤ revocation SLA` and `≥ agent/CI token TTL`, W = 5 min default-to-beat, written + dated to the thresholds file. Emit cache_hit_ratio, staleness_age telemetry. Floor named: W = 5 min is the engineering default, structurally enforced regardless; DPO ratification of the number is the `[OPEN — LEGAL]` L-1 follow-on (parallel, the floor does not wait) — record it.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.11 (the fail-static bound), 4.10 (the zookie-bypass-S6 half). Consumed: 1.9 ResilientClient, 1.10 FailStatic<T>, 4.7 (the denylist a just-revoked grant is still denied against).
- **GATE / DRILLS.** ID-D2 (F7): break the Id dependency via the scoped-reversible injector → authenticated traffic survives on the coarse fail-static cache; a JUST-REVOKED grant is still denied (the zookie bypass) → fail-static-ratios signal. Quantified: 0 successful authz after the cache during a hiccup for a revoked subject; authenticated traffic survives the hiccup.
- **TESTS (required).** Unit: S6 serves coarse grants during an injected Id-hiccup but a zookie-stamped read bypasses it; a revoked jti is denied even when S6 still holds a stale allow; correctness-fails-closed while availability-fails-static. CDC: provider+consumer pair for 4.11 (a critical-dep caller wrapping check in ResilientClient + FailStatic). Drill: ID-D2 (CI) on the harness. Mutation floor: the fail-closed-vs-fail-static branch and the zookie-bypass branch are mandatory-core — a mutation that serves a revoked subject from S6 MUST be caught.
- **DEFINITION OF DONE.** S6 exists; fail-static-not-fail-closed holds (availability degrades, correctness denies); the zookie bypass works; the W = 5 min bound is written/dated to the thresholds file with the L-1 `[OPEN — LEGAL]` follow-on named; ID-D2 (CI) emits a dated green artifact; lints green; CDC for 4.11 passes; mutation floor met. A red gate becomes a dated "claimed, not proven" row, never edited green. Committed.
- **COMMIT.** Header `P-<NNN> M1: fail-static cache (S6) + the 4.11 bound + zookie-bypass`; body lists 4.11 + 4.10-bypass, ID-D2 green, the W=5min bound + L-1 follow-on, the mutation score. Co-Authored-By trailer.

---

### P-ID-16 — The authz read-replica (S5): the ID-4 first scaling move

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (S5 the read-replica) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1".
- **DEPENDS-ON.** P-ID-05 (S1), P-ID-08 (S3), P-ID-11 (S8).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §8 (measure-before-shard — the read-replica is the named first scaling move, not premature sharding).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §2 (the S5 row — stale-tolerant, read-only, follows S1/S3/S8), §13 (measure-before-shard ID-4; S5 the committed first scaling move).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 11.1.
- **DELIVERABLE.** In crate myelin-identity: S5, the authz read-replica (the ID-4 first scaling move) — stale-tolerant, read-only, following S1/S3/S8, used for the authn/authz hot-path reads where a zookie does not demand freshness (the fail-static partner). Floor named: world-scale tunables (cardinality cap, reverse_index_lag SLO) are re-measured against S5/S8 at M5 (P-ID-31); record it. This is a thin, atomic scaling move (kept separate from S6/S7 because it is its own store with its own staleness semantics).
- **CONTRACTS TO IMPLEMENT.** Consumed/wired: 11.1 (S5 as a read replica of the OLTP tier).
- **GATE / DRILLS.** S5 follows S1/S3/S8 (assert reads against S5 are stale-tolerant + read-only); a zookie-demanding read does NOT use S5 (it goes to the primary or falls back to check). Quantified: 0 writes to S5; a zookie-stamped read bypasses S5.
- **TESTS (required).** Unit: a default-consistency read is served from S5; a zookie-stamped read bypasses S5; S5 is read-only (a write attempt errors). CDC: n/a (a replica, not a new contract — the read contracts already have CDC). Mutation floor: the zookie-bypass-S5 branch is core — state and meet the floor.
- **DEFINITION OF DONE.** S5 exists, read-only + stale-tolerant + following S1/S3/S8; zookie-demanding reads bypass it; the M5-tunables floor → P-ID-31 named; lints green; mutation floor met; coverage scanner green. Committed.
- **COMMIT.** Header `P-<NNN> M1: authz read-replica (S5)`; body lists S5 wired, the zookie-bypass, the M5-tunables floor → P-ID-31. Co-Authored-By trailer.

---

### P-ID-17 — delegation: the monotone-intersection algebra (ID-D5)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (delegation 4.5) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.5.
- **DEPENDS-ON.** P-ID-07 (the capability-token format + machine-identity scopes), P-ID-09 (check — the object-check conjunct).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it — the intersection proof is the green artifact), §2 (the floor that makes "an agent can do what no human role can" structurally impossible is stop-the-bleeding-grade).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §6 (the delegation / on-behalf-of algebra: `effective = agent.policy ∩ delegation ∩ tenant.policy`, monotone intersection, attenuation-never-amplification via macaroon/biscuit caveats; the four conjuncts; the "you cannot delegate authority you do not have" re-check at mint).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.5.
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D5 (§4.2 row).
- **DELIVERABLE.** In crate myelin-identity: `delegation(agent, trigger_actor) → EffectivePolicy` (4.5): the monotone-intersection algebra `agent.policy ∩ delegation ∩ tenant.policy`, computed as attenuation-never-amplification over the macaroon/biscuit caveat chains, with the "you cannot delegate authority you do not have" re-check at mint. It returns the composed decision so the Agent Fabric (M2) never re-implements the algebra. The token-minting half (mint_run_token) is P-ID-18; this prompt ships the algebra. Floor named: none new — the algebra is complete in M1; ID-D5 re-runs in M2 against the live EffectApi (P-ID-23).
- **CONTRACTS TO IMPLEMENT.** Owns: 4.5 delegation. Consumed: 4.2 check (the object-check conjunct).
- **GATE / DRILLS.** ID-D5 (F9): adversarial delegation — an agent is confined to `agent.policy ∩ delegation ∩ tenant.policy`, INCLUDING via a delegator who lost the right (the delegated authority must shrink when the delegator's grant is revoked) → denial counter + an intersection proof (the green artifact is the recorded proof that the effective set is the intersection, never a superset). Quantified: 0 effects outside the intersection; the intersection proof emitted for each adversarial case.
- **TESTS (required).** Unit: the intersection is monotone (adding a conjunct never grows authority); a token's authority attenuates correctly through a caveat chain; revoking the delegator's grant shrinks the agent's effective authority. CDC: provider+consumer pair for 4.5 (an EffectApi-side consumer). Drill: the ID-D5 adversarial-delegation scenario on the harness. Mutation floor: the intersection algebra is mandatory-core — a mutation that turns ∩ into ∪ MUST be caught.
- **DEFINITION OF DONE.** delegation computes the monotone intersection with the mint-time re-check; ID-D5 emits the denial counter + intersection proof (dated green artifact); the M2 re-run of ID-D5 against EffectApi is named (P-ID-23); lints green; CDC for 4.5 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: delegation algebra (monotone intersection)`; body lists 4.5, ID-D5 green (intersection proof, 0 escapes), the M2 re-run → P-ID-23, the mutation score. Co-Authored-By trailer.

---

### P-ID-18 — mint_run_token: per-run attenuated tokens + self-hosted scope + mid-resume re-mint (ID-D6)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (mint_run_token 4.7) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.7.
- **DEPENDS-ON.** P-ID-17 (delegation — the mint applies the intersection), P-ID-07 (the token format + the self-hosted scope), P-ID-14 (revoke + S7 + auto-expiring tuples).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §4 (the per-run attenuated token; life == run life; self-hosted-runner one-tenant scope; mid-resume re-mint C9), §11 (revoke-on-crash defence-in-depth via expires_at tuples).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.7.
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §1 (the mint_run_token / self-hosted-scope / mid-resume pins).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D6 (§4.2 row).
- **DELIVERABLE.** In crate myelin-identity: `mint_run_token(agent_id, run_id, delegation_caveats, ttl) → token` (4.7): per-run attenuated tokens, life == run life, the expires_at auto-expiring tuple as revoke-on-crash defence-in-depth, the self-hosted-runner one-tenant scope, and the mid-workflow re-mint on resume (callable when a multi-day HITL approval lands days later — the Workflow durable-signal case). The mint applies the delegation intersection (P-ID-17) so a token never exceeds the effective policy. Floor named: none new — mint is complete in M1.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.7 mint_run_token (the mint half; revoke shipped in P-ID-14). Consumed: 4.5 delegation (the intersection the mint applies).
- **GATE / DRILLS.** ID-D6 (F8): kill a run mid-flight → the per-run token is revoked (teardown) AND auto-expires (expires_at) within run-life ≤ W → token-revocation-lag signal. Quantified: token-revocation-lag ≤ W; the token auto-expires at run-life even if teardown is skipped.
- **TESTS (required).** Unit: minting re-checks "cannot delegate what you lack" (the mint applies the intersection); a self-hosted-runner token cannot act cross-tenant; a re-mint on resume yields a fresh attenuated token; a per-run token auto-expires at run-life; a killed run's token is revoked AND auto-expires. CDC: provider+consumer pair for 4.7 (a CI-dispatch / workflow-activity consumer). Drill: ID-D6 (CI) on the harness. Mutation floor: the mint re-check + the auto-expire are mandatory-core — a mutation that skips the re-check or drops expires_at MUST be caught.
- **DEFINITION OF DONE.** mint_run_token produces per-run, self-hosted-scoped, mid-resume-re-mintable, auto-expiring tokens applying the delegation intersection; ID-D6 (CI) emits the token-revocation-lag green artifact; lints green; CDC for 4.7 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: mint_run_token (per-run attenuated + self-hosted scope)`; body lists 4.7-mint, ID-D6 green, the self-hosted scope + mid-resume re-mint, the mutation score. Co-Authored-By trailer.

---

### P-ID-19 — The pseudonym map (S2) + the frozen pseudonym grammar

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (the pseudonym map S2 + the grammar freeze) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.8.
- **DEPENDS-ON.** P-ID-05 (S1 + the opaque principal_id/profile_ref split); the M1 KMS hierarchy (11.3, the per-subject DEK = the erasure lever), the GDPR PersonalDataHolder spine (10.1).
- **CANON DOCS.**
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure-vs-immutability — decide the grammar BEFORE the git data model freezes; pseudonymous-by-default commits never bake erasable PII into the immutable bytes).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §2 (S2 store: tightest RLS, per-subject key = the erasure lever; grammar pinned C5), §3 (opaque principal_id / erasable profile_ref split).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.8, 11.3, 10.1.
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §X-7 (the pseudonym-shred / erasure residual posture), §1.
- **DELIVERABLE.** In crate myelin-identity: (1) S2, the pseudonym map (Postgres-class, tightest RLS, `(tenant, region)`-partitioned): `real_identity ↔ per-tenant pseudonym` mapping, each subject under a per-subject key (the erasure lever), holder-registered. (2) FREEZE the pseudonym grammar `<pseudonym>@<tenant>.noreply` — frozen NOW because the Git data model (M3) is built on it (decide before the git data model freezes, EI-04 §1); Git commits become pseudonymous-by-default in M3 (P-ID-25). NO erase body (P-ID-20). Floor named: Git pseudonymous commits (M3, P-ID-25) consume the grammar — record it.
- **CONTRACTS TO IMPLEMENT.** Owns (the store + grammar half of 4.8). Consumed: 11.3 (per-subject DEK), 10.1 (holder spine — S2 registers).
- **GATE / DRILLS.** `no-untagged-personal-data` lint red on a deliberately-untagged S2 field (assert it fires, then remove); S2 holder-registered (assert it appears in the holder list); the grammar parses/formats to `<pseudonym>@<tenant>.noreply`. Quantified: S2 in the holder registry; the grammar round-trips.
- **TESTS (required).** Unit: an S2 mapping row round-trips under the tightest RLS; the pseudonym grammar parses/formats to `<pseudonym>@<tenant>.noreply`; each subject's mapping is under a distinct per-subject key. CDC: n/a (the store; the erase contract brings its CDC in P-ID-20). Mutation floor: the per-subject-key boundary is core — state and meet the floor.
- **DEFINITION OF DONE.** S2 exists (tightest RLS, per-subject key, holder-registered); the pseudonym grammar is frozen + dated; the Git-consume follow-on (P-ID-25) named; lints green; mutation floor met; coverage scanner green. Committed.
- **COMMIT.** Header `P-<NNN> M1: pseudonym map (S2) + frozen grammar`; body lists S2 + the grammar frozen, the Git-consume follow-on → P-ID-25, the mutation score. Co-Authored-By trailer.

---

### P-ID-20 — resolve_pseudonym + erase: the per-subject crypto-shred lever (ID-D8)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (resolve_pseudonym/erase 4.8 + the crypto-shred) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1", §1.1 row 4.8, §3 (Id's half of erasure-vs-immutability).
- **DEPENDS-ON.** P-ID-19 (S2 + the grammar), P-ID-08 (S3 + write_tuples for tuple erasure); the GDPR erasure-ledger spine (10.8), the restore-verify gate (11.5 / STOR-D1/D2).
- **CANON DOCS.**
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure-vs-immutability — Id owns the identity half: attribution by opaque principal_id + the pseudonym-map per-subject-DEK shred).
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it; the re-erasure receipt is the green artifact).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §11 (the pseudonym-map shred = DSR step 1; resolve_pseudonym/erase), §12 (D8 restore-resurrects-no-authority assertion).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.8, 10.8.
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §X-7 (the pseudonym-shred / erasure residual posture).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D8 (§4.2 row), STOR-D4 (the shred-unrecoverable-in-backups assertion).
- **DELIVERABLE.** In crate myelin-identity: (1) `resolve_pseudonym(subject, tenant)` and the PersonalDataHolder `erase(subject)` (4.8) = DSR step 1: destroy the per-subject DEK (crypto-shred) + shred the pseudonym-map row; write the erasure to the PII-free erasure ledger (10.8) so post-restore re-erasure can replay. (2) Wire the per-subject crypto-shred unit so an erased subject is unrecoverable, including in backups (STOR-D4). Floor named: the audited history-rewrite erasure path (when a body must be expunged) is the M5/on-demand follow-on (10.6, owned by the Git/GDPR roadmaps) — record it.
- **CONTRACTS TO IMPLEMENT.** Owns: 4.8 resolve_pseudonym/erase (Id's PersonalDataHolder erase impl). Consumed: 10.8 (erasure ledger), 11.5 (restore-verify, ID-D8 rides it).
- **GATE / DRILLS.** ID-D8 (F3): restore to a consistent point → no resurrected grants past an erasure; post-restore re-erasure runs → the re-erasure receipt signal (SCHED, rides STOR-D1/D2 the silent-data-loss floor). STOR-D4-adjacent: the per-subject crypto-shred of the pseudonym map is unrecoverable in backups (CI/SCHED). Quantified: 0 resurrected grants past an erasure; 0 recoverable PII for an erased subject post-restore; a dated re-erasure receipt emitted.
- **TESTS (required).** Unit: erase(subject) destroys the per-subject DEK and the pseudonym-map row; an erased subject's real identity is unrecoverable while its opaque principal_id still attributes events; resolve_pseudonym round-trips for a live subject and fails-closed for an erased one; the erasure is written to the PII-free ledger. CDC: provider+consumer pair for 4.8 (the DSR orchestrator + a Git/Audit consumer). Drill: ID-D8 on the harness (SCHED). Mutation floor: the crypto-shred (DEK destroy) + the no-resurrection-post-restore path are mandatory-core — a mutation that leaves the DEK recoverable MUST be caught.
- **DEFINITION OF DONE.** erase crypto-shreds (DEK destroy + map shred) + writes the erasure ledger; an erased subject is unrecoverable incl. in backups; ID-D8 emits a dated re-erasure receipt (no resurrected grants); the history-rewrite follow-on (M5/on-demand) named; lints green; CDC for 4.8 passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M1: resolve_pseudonym + erase + crypto-shred lever`; body lists 4.8 erase impl, ID-D8 green (re-erasure receipt, 0 resurrected), the history-rewrite follow-on. Co-Authored-By trailer.

---

### P-ID-21 — Reserved: the M1 → M2 Id exit-gate scorecard (the cross-tenant/fail-static/disabled-user go/no-go)

- **BAND.** M1.
- **ROADMAP MILESTONE.** ID-M1 (the M1 → M2 exit gate consolidated — ID-D3/ID-D2/ID-D1/ID-D4/ID-D7) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M1" (the gate invariant note), §6.
- **DEPENDS-ON.** P-ID-10 (ID-D3), P-ID-12 (ID-D4/ID-D7), P-ID-14 (ID-D1), P-ID-15 (ID-D2), P-ID-20 (ID-D8).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §2 (the gate invariant — no later band done over a red earlier gate), §3 (prove-it — a target you cannot measure is not a gate; a red gate is a dated scorecard row, never edited green), §5 (the committed gate).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §12 (the drills owed).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 1.8 (the green-artifact signals).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D1..ID-D8 (§4.2 rows).
- **DELIVERABLE.** In crate myelin-identity: assemble the M1 → M2 Id exit-gate scorecard as a committed CI artifact — a dated table that re-runs (does not re-implement) the M1 Id drills (ID-D3 cross-tenant 0, ID-D2 fail-static, ID-D1 disabled-in-5-min, ID-D4 leak-free pre-filter incl. the S8 JOIN, ID-D7 watermark, ID-D5 delegation, ID-D6 token-crash, ID-D8 restore) and asserts every one emits its dated green artifact to its named 1.8 signal. This is the build-layer realization of the master-sequencing M1 → M2 hard go/no-go (the reactive layer M2 is not started over a red row). No new logic — it wires the drills into one gate + the thresholds file so a red row is a dated "claimed, not proven" scorecard entry, never edited green. Floor named: ID-D9 (30x surge) + the multi-cell floor drills are M5 (P-ID-31/P-ID-35) — Id is *correct* at M1, *hardened* at M5; record it.
- **CONTRACTS TO IMPLEMENT.** Consumed/verified: the green-artifact obligations of 4.1–4.11 + 1.8 (asserts the M1 surface's drills are all green as one gate).
- **GATE / DRILLS.** The consolidated M1 → M2 gate: ID-D3, ID-D2, ID-D1, ID-D4, ID-D7, ID-D5, ID-D6, ID-D8 each emit a dated green artifact to its named signal (cross-tenant-count 0, fail-static ratios, deny-latency ≤ 5 min, zero-escape 0, watermark honoured, intersection proof, token-revocation lag ≤ W, re-erasure receipt). Quantified: 8/8 M1 Id drills green-and-dated; any red row is a dated scorecard entry, not a softened threshold.
- **TESTS (required).** Integration: the scorecard CI job re-runs each M1 Id drill scenario on the harness and asserts its named signal. CDC: re-affirm the 4.1–4.11 CDC pairs are all present (the coverage scanner is the gate). No new mutation floor (it composes existing ones).
- **DEFINITION OF DONE.** The M1 → M2 exit-gate scorecard exists as a committed CI artifact; all 8 M1 Id drills emit dated green artifacts; the thresholds file holds every default-to-beat; the M5-hardening floor (ID-D9 + multi-cell) named; lints green; coverage scanner green. A red gate becomes a dated "claimed, not proven" row, never edited green. Committed.
- **COMMIT.** Header `P-<NNN> M1: Id M1→M2 exit-gate scorecard`; body lists the 8 M1 Id drills green with their measured numbers, the M5-hardening floor → P-ID-31/P-ID-35. Co-Authored-By trailer.

---

### P-ID-22 — Promote the CaveatContext evaluator from the literal-only floor to the full QueryAst predicate core

- **BAND.** M2.
- **ROADMAP MILESTONE.** ID-M2 (the caveat evaluator promotes to the full QueryAst predicate core) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M2", §3 (the floor-then-full row: literal-only caveat → full QueryAst predicate core).
- **DEPENDS-ON.** P-ID-09 (check + the literal-only CaveatContext floor); the M2 prompt that freezes myelin-query (13.3, the QueryAst / EventMatcher core, contract 3.4).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §7 (one primitive — no second predicate language; abstract at the third copy), §1 (name-your-floors — promote the floor, record it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §8.6 (the CaveatContext rider: field/transition ABAC at check-time, off the hot list_objects path; the caveat reuses the safe non-Turing-complete QueryAst predicate core = the EventMatcher core, contract 3.4 — one DoS-hardened evaluation engine; a caveat needing missing context returns Conditional, never a silent allow), §9 (ABAC edges = caveats on tuples; predicates reuse the one query-AST core).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 4.2 (check + CaveatContext), row 3.4 (the EventMatcher = the frozen QueryAst — bounded interpreter, no UDFs/loops/recursion, statically cost-bounded).
- **DELIVERABLE.** In crate myelin-identity: promote check's CaveatContext evaluator from the M1 literal-only predicate floor to the full safe QueryAst predicate core (the frozen myelin-query QueryAst / EventMatcher core, 3.4). Replace the literal-only path with a call into the one bounded, DoS-hardened, statically-cost-bounded interpreter (no UDFs/loops/recursion); a caveat needing missing context returns Conditional (the caller supplies it), never a silent allow. There is now exactly ONE predicate language in the platform. This unblocks the non-literal field/transition caveats Issues (field.view) and Knowledge (column hiding) need in M3/M4. Floor named: this CLOSES the literal-only floor opened in P-ID-09 — record the closure (the field/transition caveat instances themselves land with their subsystems, P-ID-25/P-ID-26/P-ID-29/P-ID-30).
- **CONTRACTS TO IMPLEMENT.** Owns: 4.2 check (the CaveatContext evaluator, promoted). Consumed: 3.4 (the myelin-query QueryAst predicate core).
- **GATE / DRILLS.** No new own-owner Id drill; the gate is correctness + DoS-boundedness. Quantified: a non-literal predicate (e.g. "field visible iff issue.severity < X") evaluates correctly through the QueryAst core; an adversarial predicate is statically cost-bounded (rejected/bounded, never unbounded execution); a caveat with missing context returns Conditional (count of silent-allows on missing context = 0). The flow-determinism / cost-bound property of the QueryAst core is asserted.
- **TESTS (required).** Unit: a non-literal field caveat redacts correctly; a transition caveat gates correctly; a missing-context caveat returns Conditional not Allow; a deliberately-expensive predicate is cost-bounded (no DoS); the literal cases from P-ID-09 still pass (no regression). CDC: re-affirm the 4.2 provider+consumer pair now exercises a non-literal caveat. Mutation floor: the Conditional-on-missing-context branch is mandatory-core — a mutation turning Conditional into Allow MUST be caught.
- **DEFINITION OF DONE.** The CaveatContext evaluator runs on the full QueryAst core; no second predicate language exists; missing-context → Conditional (0 silent allows); the predicate is cost-bounded; the M1 literal-only floor is recorded closed (follow-on caveat instances → P-ID-25/26/29/30 named); lints green; the 4.2 CDC passes with a non-literal caveat; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M2: promote CaveatContext evaluator to the QueryAst core`; body lists 4.2 caveat-core promotion, the literal-only floor closed, the cost-bound + Conditional-on-missing assertions. Co-Authored-By trailer.

---

### P-ID-23 — The watcher relation + list_subjects fanout + the M2-consumer correctness re-confirm (ID-D5 re-run; SRCH/REF/NOTIF rides)

- **BAND.** M2.
- **ROADMAP MILESTONE.** ID-M2 (the watcher relation + list_subjects fanout; delegation re-run against the live EffectApi; the Filter conjoined by Search/Refs) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M2".
- **DEPENDS-ON.** P-ID-13 (list_subjects + S8), P-ID-17 (delegation), P-ID-18 (mint_run_token); the M2 prompts that build the Agent fabric EffectApi (8.2), Search (6.1 the Filter conjoin), Refs (5.3), and Notif (7.x read-fanout) — those consumers exist in the same band.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §4 (actually-try-it — exercise the real composed thing, chained, not single-handler), §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §7.5 (list_subjects at 50k density via S8; the watcher relation makes Notif's fanout an ordinary expand), §5 (the watcher relation per watchable type, C8), §6 (the delegation algebra observed from the agent side).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.4 (list_subjects watcher), 4.5 (delegation), 8.2 (EffectApi plan-then-apply: schema → capability(check) → delegation → tenant → budget → HITL → apply), 6.1 (Search conjoins the Filter), 5.3 (Refs filters via list_objects).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D5 (re-run), AG-D5; SRCH-D1/D3, REF-D1/D2/D6, NOTIF-D4.
- **DELIVERABLE.** In crate myelin-identity: (1) declare the `watcher` relation per watchable type (4.9, C8) in the namespace engine so `list_subjects(object, watcher)` over S8 serves Notif's read-fanout; verify the 50k-member-density path (the full proof lands with Chat channels in M4, but the engine path is exercised here by Notif). (2) Provide + verify the Id-side of the M2 consumer integrations: confirm delegation/check is the capability+delegation step in the live EffectApi plan-then-apply (8.2), so ID-D5 RE-RUNS against the real EffectApi (proven against the algebra in M1; now against the consumer — the AG-D5/ID-D5 family). Add integration tests (chained, not single-handler) that drive: an agent run through EffectApi confined to the intersection; Search conjoining the list_objects Filter (the search-requires-acl-filter lint); Refs filtering backlinks via list_objects. Floor named: none new — this is the consumption of the M1 surface.
- **CONTRACTS TO IMPLEMENT.** Owns (extends): 4.9 (the watcher relation declaration), 4.4 (list_subjects watcher path). Consumed/verified-from-the-Id-side: 8.2 EffectApi (the check/delegation steps), 6.1 (Search Filter conjoin), 5.3 (Refs).
- **GATE / DRILLS.** ID-D5 re-run (F9) against the live EffectApi: an effect outside `agent.policy ∩ delegation ∩ tenant.policy` is denied → denial counter. The cross-owner drills that ride Id's contracts, asserted to pass as composed: SRCH-D1/SRCH-D3 (confidential never in any result incl. counts/IDF/RAG; cross-tenant 0), REF-D1/REF-D2 (confidential-via-public 0 leak; cross-tenant edge 0), REF-D6/SRCH-D2 (revoke + re-read with post-revoke zookie → excluded within W, the S8 watermark from the consumer side), NOTIF-D4 (confidential subject → humanised tombstone, title never leaks). Quantified: 0 effects outside the intersection; 0 leaked objects across Search/Refs/Notif as composed; revoked grants excluded within W.
- **TESTS (required).** Integration (chained): an agent run via EffectApi attempting an out-of-intersection effect → denied; a Search query conjoining the Filter → confidential rows absent incl. from counts; a Refs backlink traverse → confidential edges absent; a Notif fanout via list_subjects(watcher) at synthetic density → no title leak. CDC: re-affirm the 4.4/4.5 provider+consumer pairs now exercise the live M2 consumers. Mutation floor: re-confirm the delegation intersection mutation floor holds when called via EffectApi.
- **DEFINITION OF DONE.** the watcher relation is declared and list_subjects(watcher) serves the fanout; ID-D5 re-runs green against the live EffectApi; the SRCH/REF/NOTIF cross-owner drills that ride Id's list_objects Filter + S8 watermark emit their green artifacts (Id's F1/F2/F7 hold as composed); chained integration tests pass; lints (search-requires-acl-filter) green; CDCs pass; mutation floor re-confirmed. Committed.
- **COMMIT.** Header `P-<NNN> M2: watcher fanout + Id correctness as composed by the M2 consumers`; body lists the watcher relation, ID-D5 re-run green, the SRCH/REF/NOTIF rides green. Co-Authored-By trailer.

---

### P-ID-24 — The Git ReBAC namespace fragment (ref-glob + CODEOWNERS + protected_push + approve_untrusted_ci): GIT-D8/D11 authz side

- **BAND.** M3.
- **ROADMAP MILESTONE.** ID-M3/M4 (the Git namespace fragment 4.9) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M3).
- **DEPENDS-ON.** P-ID-10 (the ReBAC engine + fragment-admit contract + core hierarchy), P-ID-12 (the list_objects SetExpr conjoin for the PR/repo list); the M3 Git-hosting prompts that build the repo/PR data model (they declare + consume this fragment in the same band).
- **CANON DOCS.**
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §5 (the Git fragment: repo/branch/ref/pull_request/pr_comment with ref-glob-scoped relations, branch-protection as protected_push, CODEOWNERS-as-relations, the `approve_untrusted_ci` relation C7 the fork-endorsement gate reads as an ordinary check; `pull_request.merge = parent_repo->protected_push`), §7.3 (the Git via_column mapping: pr.id / repo.id).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.9 (the Git fragment), 4.3 (the SetExpr conjoin), 5.9 (the CheckStatus seam — approve_untrusted_ci feeds it).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §X-1 (the approve_untrusted_ci relation + the fork-endorsement gate).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) GIT-D8, GIT-D11.
- **DELIVERABLE.** Declare + compile the Git ReBAC namespace fragment into the cell schema via Id's fragment-admit contract: `repo`, `branch`/`ref`, `pull_request`, `pr_comment`, with ref-glob-scoped relations, branch-protection as a tighter `protected_push` permission, CODEOWNERS path-globs compiled to reviewer-requirement tuples (not a bespoke check), the `approve_untrusted_ci` relation (the fork-endorsement gate reads it as an ordinary check; Git reads the trust_tier CI stamps, Id never recomputes trust), and `pull_request.merge = parent_repo->protected_push`. Light up the list_objects SetExpr conjoin for the PR/repo list (the Git via_column = pr.id / repo.id — one query, no N+1). The pseudonymous-by-default commits (the 4.8-grammar consume) are P-ID-25; this prompt ships the authz fragment. Floor named: this is the first of the five fragments promoting the M1 engine-only floor — record the progression.
- **CONTRACTS TO IMPLEMENT.** Owns (fragment content): 4.9 (the Git fragment), consumes 4.3 (the conjoin). Provides the approve_untrusted_ci relation that 5.9 (the X-1 seam) reads.
- **GATE / DRILLS.** GIT-D8 (F2): cross-tenant repo access denied at the front door (tenant from token) → 0 cross-tenant. GIT-D11 (F1): partial-visibility 100k-PR list → the SetExpr JOIN returns only visible rows, 0 leak, ONE query, revoke reflected → zero-escape counter + one-query assertion. Quantified: 0 cross-tenant; 0 leaked PRs; one query for the list.
- **TESTS (required).** Unit: the Git fragment compiles into the cell schema; `pull_request.merge` resolves via protected_push; a CODEOWNERS glob compiles to the right reviewer tuples; approve_untrusted_ci is an ordinary check; the PR list conjoins the Filter in one query. CDC: provider+consumer pair for the 4.9-Git-fragment (Git-hosting consumes the compiled namespace). Drill: GIT-D8/GIT-D11 (CI) on the harness. Mutation floor: the fragment's exclusion/inheritance rewrites are core — state + meet the floor.
- **DEFINITION OF DONE.** The Git fragment compiles + admits; ref-glob/CODEOWNERS/protected_push/approve_untrusted_ci all resolve as ordinary checks; the PR/repo list conjoins the SetExpr in one query (no N+1); GIT-D8/D11 emit dated green artifacts (0 cross-tenant, 0 leak/one-query); the engine-only-floor progression recorded; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M3: Git ReBAC fragment`; body lists 4.9-Git + the SetExpr conjoin, GIT-D8/D11 green, the fragment-floor progression. Co-Authored-By trailer.

---

### P-ID-25 — Git pseudonymous-by-default commits: consuming the 4.8 grammar (GIT-D2)

- **BAND.** M3.
- **ROADMAP MILESTONE.** ID-M3/M4 (Git pseudonymous-by-default commits consuming the 4.8 grammar) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M3).
- **DEPENDS-ON.** P-ID-19 (the frozen pseudonym grammar), P-ID-20 (the erase/crypto-shred lever), P-ID-24 (the Git fragment); the M3 Git-hosting prompts that build the commit data model.
- **CANON DOCS.**
  - [`../../external-insights/04-hard-problems.md`](../../external-insights/04-hard-problems.md) §1 (erasure-vs-immutability — pseudonymous-by-default commits never bake erasable PII into the immutable bytes; Id's grammar is consumed here).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §11 (pseudonymous-by-default commits consume the grammar), §3 (opaque principal_id / erasable profile_ref split).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.8 (pseudonymous commits).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §X-7.
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) GIT-D2.
- **DELIVERABLE.** Make Git commits pseudonymous-by-default by consuming the 4.8 grammar `<pseudonym>@<tenant>.noreply` (the M1 lever now applied): commit author/committer identity is the per-tenant pseudonym, never the erasable real identity — so an erase(subject) crypto-shred leaves the immutable commit bytes carrying only the pseudonym, and the erasure residual == the one platform posture. This is the Id-side identity content consumed by the Git data model (the Git-hosting prompts own the object model). Floor named: the audited history-rewrite path (when a body must be expunged) is the M5/on-demand follow-on (10.6, Git/GDPR roadmaps) — record it.
- **CONTRACTS TO IMPLEMENT.** Consumes 4.8 (pseudonymous commits — the grammar applied to Git identity).
- **GATE / DRILLS.** GIT-D2 (F3): erase a commit author → the pseudonymous-by-default residual == the one platform posture (0 real-identity recoverable from the immutable bytes) → the erasure residual matches. Quantified: pseudonymous residual == posture; 0 real-identity recoverable from committed bytes after erase.
- **TESTS (required).** Unit: a commit's author/committer is the pseudonym `<pseudonym>@<tenant>.noreply`; after erase(subject) the commit bytes carry no recoverable real identity; the opaque principal_id still attributes the commit for authz. CDC: provider+consumer pair for 4.8 (the Git data model + the DSR orchestrator). Drill: GIT-D2 (SCHED) on the harness. Mutation floor: the pseudonym-substitution-at-commit path is core — a mutation that bakes real identity into the bytes MUST be caught.
- **DEFINITION OF DONE.** Git commits are pseudonymous-by-default; GIT-D2 emits a dated green artifact (pseudonymous residual == posture); the history-rewrite follow-on (M5/on-demand) named; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M3: Git pseudonymous-by-default commits`; body lists 4.8-consume, GIT-D2 green (pseudonymous residual), the history-rewrite follow-on. Co-Authored-By trailer.

---

### P-ID-26 — The Knowledge ReBAC namespace fragment (page-tree-with-overrides + row + field caveat): KN-D5/D13 authz side

- **BAND.** M3.
- **ROADMAP MILESTONE.** ID-M3/M4 (the Knowledge namespace fragment 4.9) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M3, Knowledge fragment).
- **DEPENDS-ON.** P-ID-10 (the engine + admit-contract), P-ID-12 (list_objects for row-level ACL), P-ID-22 (the full QueryAst caveat core for field-level column hiding); the M3 Knowledge-platform prompts that build the page/database data model.
- **CANON DOCS.**
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §5 (the Knowledge fragment: `space`/`page`/`block`/`database_row`; page-tree inheritance WITH overrides `page.read = parent_page->read + direct_reader - direct_block`; row-level ACL via list_objects C1; field-level column hiding as a check-time CaveatContext caveat C3), §7.3 (the KN via_column = db_row.id), §8.6 (the field caveat on the full QueryAst core).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.9 (the KN fragment), 4.3 (row-level ACL), 4.2 (the field caveat).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) KN-D5, KN-D13.
- **DELIVERABLE.** Declare + compile the Knowledge ReBAC namespace fragment via Id's admit-contract: `space`, `page`, `block`, `database_row`; page-tree inheritance WITH overrides (`page.read = parent_page->read + direct_reader - direct_block`) so a sub-page can narrow inherited access; row-level ACL pushed down via list_objects (the KN via_column = db_row.id); field-level column hiding as a check-time CaveatContext caveat on the full QueryAst core (P-ID-22). This is the Id-side authz content; the KN data model is the Knowledge-platform prompts'. Floor named: the second of the five fragments promoting the engine-only floor — record it.
- **CONTRACTS TO IMPLEMENT.** Owns (fragment content): 4.9 (the KN fragment), consumes 4.3 (row-level ACL conjoin), 4.2 (the field caveat).
- **GATE / DRILLS.** KN-D5 / KN-D13 (F1/F2): a confidential page/row/field is ABSENT from any list_objects/search result for an unauthorized viewer INCLUDING the COUNT (no count leak), and cross-tenant access is 0 → zero-escape counter + count-leak = 0 + cross-tenant = 0. Quantified: 0 leaked pages/rows/fields incl. COUNT; 0 cross-tenant.
- **TESTS (required).** Unit: page-tree inheritance resolves; an override (direct_block) narrows inherited access correctly; row-level ACL conjoins via list_objects; a field caveat hides a column without leaking it in COUNT. CDC: provider+consumer pair for the 4.9-KN-fragment. Drill: KN-D5/KN-D13 on the harness. Mutation floor: the override-exclusion rewrite + the no-count-leak path are core — state + meet the floor.
- **DEFINITION OF DONE.** The KN fragment compiles + admits; page-tree-with-overrides + row ACL + field caveat all resolve; KN-D5/D13 emit dated green artifacts (0 leak incl. COUNT, 0 cross-tenant); the fragment-floor progression recorded; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M3: Knowledge ReBAC fragment (page-tree overrides + row + field caveat)`; body lists 4.9-KN, KN-D5/D13 green, the fragment-floor progression. Co-Authored-By trailer.

---

### P-ID-27 — The CI ReBAC namespace fragment (secret-non-inheritance + !is_untrusted_fork): CI-D10 fragment side

- **BAND.** M4.
- **ROADMAP MILESTONE.** ID-M3/M4 (the CI namespace fragment 4.9) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M4, CI fragment).
- **DEPENDS-ON.** P-ID-10 (the engine), P-ID-24 (the approve_untrusted_ci relation Git provides); the M4 CI prompts that build the pipeline/run model.
- **CANON DOCS.**
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §5 (the CI fragment: `ci_project`/`environment`/`secret`/`run`; `run.view = parent_repo->pull`; `run.trigger = parent_repo->push`; `secret.read` is NOT inherited — a direct narrow relation, CI-1; the `read & !is_untrusted_fork` ABAC edge C7 — CI stamps trust_tier from run provenance, a fork run is untrusted_fork).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.9 (the CI fragment), 5.9 (CI stamps trust_tier).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §X-1, §1 (CI-1 secret-non-inheritance).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) CI-D10 (the secret-non-inheritance half).
- **DELIVERABLE.** Declare + compile the CI ReBAC namespace fragment via Id's admit-contract: `ci_project`, `environment`, `secret`, `run`; `run.view = parent_repo->pull`; `run.trigger = parent_repo->push`; `secret.read` is a DIRECT NARROW relation, NOT inherited (so secrets never leak via project-read inheritance — CI-1); the `read & !is_untrusted_fork` ABAC edge (CI stamps trust_tier from run provenance; a fork run is untrusted_fork). The self-hosted-runner token scope exercise is P-ID-28; this prompt ships the fragment. Floor named: third of the five fragments promoting the engine-only floor — record it.
- **CONTRACTS TO IMPLEMENT.** Owns (fragment content): 4.9 (the CI fragment). Provides the trust_tier ABAC edge 5.9 reads.
- **GATE / DRILLS.** secret.read is not reachable via project-read inheritance (assert a project-reader cannot read a secret → 0); the !is_untrusted_fork edge stamps trust_tier correctly (a fork run is untrusted_fork). Quantified: 0 secret reads via inheritance; the fork edge gates correctly.
- **TESTS (required).** Unit: the CI fragment compiles; run.view/run.trigger resolve via the repo relations; secret.read is NOT reachable via project-read (a direct grant is required); the !is_untrusted_fork edge stamps trust_tier correctly. CDC: provider+consumer pair for the 4.9-CI-fragment. Drill: the CI-D10 secret-non-inheritance scenario on the harness. Mutation floor: the secret-non-inheritance rewrite is core — a mutation that makes secret.read inheritable MUST be caught.
- **DEFINITION OF DONE.** The CI fragment compiles + admits; secret.read is non-inherited; the !is_untrusted_fork edge works; the secret-non-inheritance assertion is green; the fragment-floor progression recorded; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M4: CI ReBAC fragment (secret-non-inheritance + !is_untrusted_fork)`; body lists 4.9-CI, secret-non-inheritance green, the fragment-floor progression. Co-Authored-By trailer.

---

### P-ID-28 — The self-hosted-runner token scope exercised against the CI fragment: CI-D10 scope side

- **BAND.** M4.
- **ROADMAP MILESTONE.** ID-M3/M4 (the self-hosted-runner token scope 4.7) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M4, CI fragment).
- **DEPENDS-ON.** P-ID-18 (mint_run_token + the self-hosted-runner scope), P-ID-27 (the CI fragment the runner acts against); the M4 CI prompts.
- **CANON DOCS.**
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §4 (the self-hosted-runner token scoped to one tenant's SelfHosted jobs, cannot mint cross-tenant).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.7 (the self-hosted-runner scope), 4.9 (the CI fragment it acts against).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §1 (the self-hosted-runner scope pin).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) CI-D10 (self-hosted-runner scope; 0 cross-tenant job/secret reads).
- **DELIVERABLE.** In crate myelin-identity: confirm + exercise the self-hosted-runner token scope (4.7) against the live CI fragment: a self-hosted-runner token is scoped to one tenant's SelfHosted jobs and cannot mint or act cross-tenant (the no-global-pool property at the identity layer). This drives CI-D10 — a compromised runner is bounded to its tenant's SelfHosted jobs, 0 cross-tenant job/secret reads. The scope mechanism shipped in P-ID-18; this prompt proves it against the CI namespace. Floor named: none new.
- **CONTRACTS TO IMPLEMENT.** Owns (exercised): 4.7 (the self-hosted-runner scope, against the CI fragment).
- **GATE / DRILLS.** CI-D10 (F2): a compromised self-hosted runner is bounded to its tenant's SelfHosted jobs → 0 cross-tenant job/secret reads. Quantified: 0 cross-tenant job/secret reads from a runner token.
- **TESTS (required).** Unit: a self-hosted-runner token cannot act cross-tenant against the CI fragment; it cannot read another tenant's secret or run; it cannot mint a cross-tenant token. CDC: provider+consumer pair for the 4.7 self-hosted scope (a CI-dispatch consumer). Drill: CI-D10 on the harness. Mutation floor: the cross-tenant-scope check is core — a mutation that lets the runner act cross-tenant MUST be caught.
- **DEFINITION OF DONE.** The self-hosted-runner one-tenant scope holds against the CI fragment; CI-D10 emits a dated green artifact (0 cross-tenant job/secret reads); lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M4: self-hosted-runner token scope (CI-D10)`; body lists 4.7-scope exercised, CI-D10 green (0 cross-tenant). Co-Authored-By trailer.

---

### P-ID-29 — The Issues ReBAC namespace fragment (confidential-exclusion + field/transition caveats): ISS-D3 authz side

- **BAND.** M4.
- **ROADMAP MILESTONE.** ID-M3/M4 (the Issues namespace fragment 4.9) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M4, Issues fragment).
- **DEPENDS-ON.** P-ID-10 (the engine), P-ID-12 (the list_objects conjoin for the board), P-ID-22 (the QueryAst caveat core for field/transition caveats); the M4 Issues prompts.
- **CANON DOCS.**
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §5 (the Issues fragment: `issue`/`field`/`transition` + the `confidential` exclusion userset — a confidential issue disappears from a normal reader's list_objects BY CONSTRUCTION, not a post-filter; field/transition CaveatContext caveats), §7.3 (the Issues via_column = issue.id), §8.6 (the field/transition caveat on the full QueryAst core).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.9 (the Issues fragment), 4.3 (the board conjoin), 4.2 (field/transition caveats).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ISS-D3 (cross-tenant + confidential IDOR 0 incl. under zookie staleness).
- **DELIVERABLE.** Declare + compile the Issues ReBAC namespace fragment via Id's admit-contract: `issue`, `field`, `transition` + the `confidential` exclusion userset (a confidential issue disappears from a normal project-reader's list_objects by construction); field-level and transition-level visibility as permissions on field/transition sub-objects with ABAC caveats on the QueryAst core (e.g. "field visible iff issue.severity < X"; "transition needs an approver edge"), kept off the hot list_objects path via the CaveatContext. The board/backlog scan conjoins the SetExpr (the Issues via_column = issue.id) — no N+1. This is the Id-side authz content; the Issues data model is its own prompts'. Floor named: fourth of the five fragments promoting the engine-only floor — record it.
- **CONTRACTS TO IMPLEMENT.** Owns (fragment content): 4.9 (the Issues fragment), consumes 4.3 (the board conjoin), 4.2 (the field/transition caveats).
- **GATE / DRILLS.** ISS-D3 (F1/F2): cross-tenant + confidential IDOR → 0 leak INCLUDING under zookie staleness (the confidential exclusion holds by construction; the board conjoin holds under the S8 watermark). Quantified: 0 leaked confidential issues incl. under staleness; 0 cross-tenant.
- **TESTS (required).** Unit: the confidential exclusion userset removes a confidential issue from a normal reader's list_objects (not a post-filter); a field caveat hides a field; a transition caveat gates a transition; the board conjoins in one query. CDC: provider+consumer pair for the 4.9-Issues fragment. Drill: ISS-D3 on the harness. Mutation floor: the confidential-exclusion rewrite is core — a mutation that turns the exclusion into a post-filter MUST be caught.
- **DEFINITION OF DONE.** The Issues fragment compiles + admits; the confidential exclusion + field/transition caveats all resolve; the board conjoins in one query; ISS-D3 emits a dated green artifact (0 leak incl. under staleness, 0 cross-tenant); the fragment-floor progression recorded; lints green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M4: Issues ReBAC fragment (confidential-exclusion + field/transition caveats)`; body lists 4.9-Issues, ISS-D3 green, the fragment-floor progression. Co-Authored-By trailer.

---

### P-ID-30 — The Chat ReBAC namespace fragment (channel.read + message.view + 50k-watcher density): CHAT authz side

- **BAND.** M4.
- **ROADMAP MILESTONE.** ID-M3/M4 (the Chat namespace fragment 4.9) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M3/M4" (Work — M4, Chat fragment).
- **DEPENDS-ON.** P-ID-10 (the engine), P-ID-12 (the list_objects conjoin for the channel list), P-ID-23 (the watcher fanout for channel density); the M4 Chat prompts.
- **CANON DOCS.**
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §5 (the Chat fragment: `channel.read = member + parent_project->read`, `message.view = parent_channel->read`; the per-viewer unfurl is a Refs concern — an unfurl of a confidential issue degrades to a tombstone), §7.3 (the Chat via_column = channel.id/message.id), §7.5 (the 50k-member-channel list_subjects(channel, watcher) density).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.9 (the Chat fragment), 4.3 (the channel-list conjoin), 4.4 (channel watcher density).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) the Chat confidential-unfurl-tombstone + search-as-non-member-0-results rows.
- **DELIVERABLE.** Declare + compile the Chat ReBAC namespace fragment via Id's admit-contract: `channel`, `message`, `unfurl`; `channel.read = member + parent_project->read`; `message.view = parent_channel->read`; the 50k-member-channel `list_subjects(channel, watcher)` density (the Notif read-fanout proof, now at real Chat density); search-as-non-member returns 0 results (rides Id's Filter). This is the Id-side authz content; the Chat data model is its own prompts'. Floor named: this CLOSES the M1 engine-only floor — all five fragments now exist; record the closure.
- **CONTRACTS TO IMPLEMENT.** Owns (fragment content): 4.9 (the Chat fragment), consumes 4.3 (the channel-list conjoin), 4.4 (channel watcher density).
- **GATE / DRILLS.** Chat: a confidential unfurl degrades to a tombstone (0 title leak); search-as-non-member → 0 results (+ the search-requires-acl-filter lint); list_subjects(channel, watcher) at 50k density within budget. Quantified: 0 chat title leak; 0 results for a non-member search; 50k-density fanout within budget.
- **TESTS (required).** Unit: channel.read resolves via member + parent_project; message.view via parent_channel; list_subjects(channel, watcher) at 50k density returns within budget; a non-member's channel/message search returns 0; a confidential unfurl degrades to a tombstone. CDC: provider+consumer pair for the 4.9-Chat fragment. Drill: the Chat confidential-unfurl + search-as-non-member scenarios on the harness. Mutation floor: channel.read + the watcher density path are core — state + meet the floor.
- **DEFINITION OF DONE.** The Chat fragment compiles + admits; channel.read/message.view resolve; the channel-list conjoins in one query; the 50k-watcher density holds; the Chat drills emit dated green artifacts (0 title leak, 0 non-member results); the M1 engine-only floor is recorded CLOSED (all five fragments exist); lints (search-requires-acl-filter) green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M4: Chat ReBAC fragment`; body lists 4.9-Chat, the Chat drills green, the engine-only floor closed (all five fragments). Co-Authored-By trailer.

---

### P-ID-31 — World-scale hardening: the 30x authz surge ID-D9 (protected-human-lane shed order)

- **BAND.** M5.
- **ROADMAP MILESTONE.** ID-M5 (the 30x authz surge ID-D9) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M5".
- **DEPENDS-ON.** P-ID-12 (list_objects + S8), P-ID-15 (the protected-human-lane shed order via the fail-static surface); the M5 prompts that build the F6 surge family.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it at scale — the harness multiplies 1x/10x/30x; observability is part of the pass).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §10 (the protected-human-lane shed order on the authz surface), §13 (authz is the highest-QPS shared system).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.11 (the shed order on the authz surface), 1.8 (shed-counts, authz p99), 1.11 (the protected-human-lane shed order).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D9.
- **DELIVERABLE.** In crate myelin-identity: run the 30x authz-surge drill ID-D9 on the failure-injection harness: 30x agent surge on the authz hot path → the protected human lane holds within budget (authz p99 within budget for the human lane), the agent lane sheds (429 + Retry-After per the 1.11 shed order), cross-tenant impact 0. The S8 tunables finalisation is P-ID-32; ID-D8-cell-scale is P-ID-33; this prompt ships the surge proof. Floor named: none new — the surge hardening is complete here.
- **CONTRACTS TO IMPLEMENT.** Owns (finalises): 4.11 (the shed order on the authz surface). Consumed: 1.11 (the protected-human-lane shed order).
- **GATE / DRILLS.** ID-D9 (F6): 30x agent surge → human lane holds (authz p99 within budget for the human lane), agent lane sheds (429 + Retry-After, shed-counts > 0 for the agent lane), cross-tenant impact 0 → shed-counts + authz p99 signals. Quantified: human-lane authz p99 ≤ budget under 30x; agent lane sheds; cross-tenant 0.
- **TESTS (required).** Drill (SCHED): ID-D9 on the harness (the 30x surge with mixed principal kinds). Mutation floor: re-confirm the surge shed-order holds under load (a mutation that sheds the human lane MUST be caught).
- **DEFINITION OF DONE.** ID-D9 emits dated green artifacts (human lane holds, agent sheds, cross-tenant 0); lints green; mutation floor re-confirmed. A red gate becomes a dated "claimed, not proven" row, never edited green. Committed.
- **COMMIT.** Header `P-<NNN> M5: authz surge (ID-D9)`; body lists ID-D9 green with measured numbers (human-lane p99, agent shed-counts, cross-tenant 0). Co-Authored-By trailer.

---

### P-ID-32 — World-scale hardening: the S8 measured tunables finalised at scale (cardinality cap + reverse_index_lag SLO)

- **BAND.** M5.
- **ROADMAP MILESTONE.** ID-M5 (S8 tunables finalised) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M5".
- **DEPENDS-ON.** P-ID-11 (list_objects + S8 + the cardinality cap floor), P-ID-16 (S5 the replica), P-ID-31 (the surge load the tunables are measured under).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §8 (measure-not-predict — the cardinality cap is a measured tunable, finalised here), §3 (prove-it at scale).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §13 (S8 as the named first replica; measure-before-shard ID-4; the cardinality cap + reverse_index_lag SLO re-measured under world-scale load), §15 (the two open tunables: the Ids-vs-Filter threshold + the reverse_index_lag SLO).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.3 (the cardinality cap tunable), 1.8 (reverse_index_lag).
- **DELIVERABLE.** In crate myelin-identity: finalise the S8 measured tunables at world-scale load — the Ids↔Filter cardinality cap + the reverse_index_lag freshness SLO re-measured under load (riding the P-ID-31 surge + the cell-scale load) and written/dated to the thresholds file with their measured numbers. This CLOSES the P-ID-11 cardinality-cap floor. Floor named: the cardinality-cap measured-tunable floor (P-ID-11) is CLOSED here; record it.
- **CONTRACTS TO IMPLEMENT.** Owns (finalises): 4.3 (the cardinality cap tunable, finalised at scale), 4.10 (the reverse_index_lag SLO the watermark fallback reads).
- **GATE / DRILLS.** The cardinality cap + reverse_index_lag SLO are re-measured under world-scale load and written/dated to the thresholds file; a list at the measured cap switches Ids↔Filter correctly; a scan inside the measured reverse_index_lag SLO serves from S8, one beyond it falls back to check. Quantified: the two tunables have dated measured numbers in the thresholds file; the Ids↔Filter switch + the lag-SLO fallback honour the measured numbers.
- **TESTS (required).** Drill (SCHED): the tunable-measurement scenario on the harness at world-scale load. Integration: a list at the measured cap, a scan at the measured lag SLO. Mutation floor: the cap-dispatch + the lag-SLO fallback are core — re-confirm the floor holds at the measured numbers.
- **DEFINITION OF DONE.** The S8 cardinality cap + reverse_index_lag SLO are finalised + dated to the thresholds file (the P-ID-11 floor CLOSED); the Ids↔Filter switch + the lag-SLO fallback honour the measured numbers; lints green; mutation floor re-confirmed. Committed.
- **COMMIT.** Header `P-<NNN> M5: S8 tunables finalised at scale`; body lists the cardinality cap + reverse_index_lag SLO measured + dated (the P-ID-11 floor closed). Co-Authored-By trailer.

---

### P-ID-33 — World-scale hardening: ID-D8 at cell scale + the cell-bulkhead drill

- **BAND.** M5.
- **ROADMAP MILESTONE.** ID-M5 (ID-D8 cell-scale re-confirm + cell-bulkhead) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M5".
- **DEPENDS-ON.** P-ID-20 (ID-D8 at M1 scale + the crypto-shred); the M5 prompts that build the cell-bulkhead drill + STOR-D2 at cell scale.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §3 (prove-it at scale — observability is part of the pass).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §12 (ID-D8 at cell scale — restore-resurrects-no-authority), §13 (the cell-bulkhead — a fatal fault in one cell unaffects authz in others).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 11.5 (restore-verify at cell scale), row 1.8 (the re-erasure receipt signal).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) ID-D8 (cell-scale re-confirm), STOR-D2 (at cell scale).
- **DELIVERABLE.** In crate myelin-identity: (1) re-confirm ID-D8 at cell scale (restore-resurrects-no-authority under world-scale load, riding STOR-D2 at cell scale). (2) Participate in the cell-bulkhead drill (a fatal fault in one cell unaffects authz in others). Floor named: none new — the M1 ID-D8 proof is re-confirmed at cell scale here.
- **CONTRACTS TO IMPLEMENT.** Consumed: 11.5 (restore-verify at cell scale), 4.8 (the erase/crypto-shred re-confirmed).
- **GATE / DRILLS.** ID-D8 at cell scale (F3): no resurrected authority under world-scale load → re-erasure receipt. STOR-D2 at cell scale re-confirmed (RPO/RTO under load). The cell-bulkhead scenario: a fatal fault in one cell unaffects authz in others → cell-isolation signal. Quantified: 0 resurrected authority at cell scale; RPO/RTO within budget under load; 0 cross-cell authz impact from a single-cell fault.
- **TESTS (required).** Drill (SCHED): ID-D8-cell-scale + the cell-bulkhead scenario on the harness. Mutation floor: the no-resurrection path is re-confirmed at cell scale.
- **DEFINITION OF DONE.** ID-D8 at cell scale + STOR-D2 at cell scale re-confirmed (dated green artifacts); the cell-bulkhead holds; lints green; mutation floor re-confirmed. A red gate becomes a dated "claimed, not proven" row. Committed.
- **COMMIT.** Header `P-<NNN> M5: ID-D8 at cell scale + cell-bulkhead`; body lists ID-D8 cell-scale green (re-erasure receipt), STOR-D2 cell-scale, the cell-bulkhead isolation. Co-Authored-By trailer.

---

### P-ID-34 — World-scale hardening: Id as the proven authz spine of the four E2E scenarios (E2E-1..E2E-4)

- **BAND.** M5.
- **ROADMAP MILESTONE.** ID-M5 (the E2E spine) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M5".
- **DEPENDS-ON.** P-ID-12 (check/list_objects), P-ID-17/P-ID-18 (delegation/mint_run_token), P-ID-20 (the pseudonym shred), P-ID-11 (S8 as a holder); the M5 prompts that build the whole-system E2E wedge (E2E-1..E2E-4).
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §4 (actually-try-it — drive the whole composed thing E2E), §3 (prove-it).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §1 (Id answers who/may-they for every read path), §7 (list_objects the spine of permission-filtered lineage), §11 (the DSAR fan-out incl. the pseudonym shred + S8 as a holder).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) rows 4.2/4.3/4.5/4.7/4.8 (the spine contracts), 1.8 (the E2E green-artifact signals).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) the E2E-1..E2E-4 rows (Id as the authz spine).
- **DELIVERABLE.** In crate myelin-identity: provide + verify Id as the authz spine of all four E2E scenarios: E2E-1 (the PR context pane resolves per-viewer via check), E2E-2 (the triage agent runs under delegation/mint_run_token), E2E-3 (spec-to-ship lineage is permission-filtered via list_objects), E2E-4 (DSAR fan-out includes the pseudonym-map shred + S8 as a holder). Integration tests (chained, not single-handler) drive each. Floor named: none new — this composes the M1 surface at E2E scope.
- **CONTRACTS TO IMPLEMENT.** Consumed/verified: 4.2/4.3/4.5/4.7/4.8 (the spine contracts composed in the E2E scenarios).
- **GATE / DRILLS.** The four E2E scenarios green with Id as the authz spine: E2E-1 (per-viewer check, 0 leak), E2E-2 (delegated agent run within the intersection), E2E-3 (permission-filtered lineage, cold-reindex == live), E2E-4 (DSAR 0 holders missed incl. S8 + the pseudonym shred). Quantified: E2E-1..E2E-4 green (0 leak, exactly-once, cold-reindex == live, DSAR 0 holders missed).
- **TESTS (required).** Integration (chained): the Id-side of E2E-1..E2E-4 (the per-viewer check, the delegated agent run, the permission-filtered lineage, the DSAR fan-out incl. S8 + the pseudonym shred). Mutation floor: re-confirm the spine contracts' floors hold when composed E2E.
- **DEFINITION OF DONE.** Id is the proven authz spine of E2E-1..E2E-4 (all green, dated artifacts); lints green; mutation floor re-confirmed. Committed.
- **COMMIT.** Header `P-<NNN> M5: Id as the authz spine of E2E-1..E2E-4`; body lists E2E-1..E2E-4 green with their measured properties. Co-Authored-By trailer.

---

### P-ID-35 — Multi-cell principal authority: the cross-cell read-through over the PII-free bridge (GA-D8/CP-D7/CP-D8 authz side)

- **BAND.** M5.
- **ROADMAP MILESTONE.** ID-M5 (multi-cell principal authority — the deepest open case, the single-home-cell floor's follow-on) — [`../../06-roadmaps/shared/identity-and-access.md`](../../06-roadmaps/shared/identity-and-access.md) §2 "M5" + §3 (the floor row: single-home-cell → multi-cell).
- **DEPENDS-ON.** P-ID-10/P-ID-11 (the engine + S8, single-home-cell), P-ID-20 (the pseudonym shred for per-cell DSR receipts); the M5 control-plane prompt that makes the cross-cell PII-free pointer bridge (12.6) live.
- **CANON DOCS.**
  - [`../../external-insights/01-process-and-quality-doctrine.md`](../../external-insights/01-process-and-quality-doctrine.md) §1 (name-your-floors — multi-cell is the single-home-cell floor's named follow-on), §3 (prove-it — the cross-cell drills).
  - [`../../05-refined-shared-systems-architecture/identity-and-access.md`](../../05-refined-shared-systems-architecture/identity-and-access.md) §13 (multi-cell principal authority SC-2/SC-3: home-cell-authoritative + cross-cell coarse-grant read-through over the OQ-I bridge, zookie-bounded; resolution ALWAYS cell-local — a principal spanning cells is evaluated in the cell holding the object, never by pulling tuples cross-region, preserving no-cross-region-PII), §15 (the open question this closes).
  - [`../../05-refined-shared-systems-architecture/contract-index.md`](../../05-refined-shared-systems-architecture/contract-index.md) row 12.6 (the cross-cell PII-free pointer bridge), rows 4.3/4.10 (the zookie-bounded read-through).
  - [`../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md`](../../05-refined-shared-systems-architecture/00-reconciliation-decisions.md) §OQ-I (the bridge).
  - [`../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`](../../05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md) GA-D8, CP-D7, CP-D8.
- **DELIVERABLE.** In crate myelin-identity: promote single-home-cell principal authority to multi-cell — the cross-cell read-through model goes live over the OQ-I PII-free pointer bridge (12.6): home-cell-authoritative + cross-cell coarse-grant read-through, zookie-bounded. Resolution is ALWAYS cell-local: a principal spanning cells is evaluated in the cell that holds the OBJECT, never by pulling tuples cross-region (preserving no-cross-region-PII, ADR-11). Wire the per-cell DSR receipt set (the DSR fan-out iterates member_cells; each cell's pseudonym-map shred produces a receipt). Floor named: this CLOSES the single-home-cell floor (P-ID-10/11) — record it. This is the deepest remaining Id unknown made real.
- **CONTRACTS TO IMPLEMENT.** Owns (extends to multi-cell): 4.3/4.10 (the cross-cell zookie-bounded read-through). Consumed: 12.6 (the cross-cell PII-free pointer bridge, now live).
- **GATE / DRILLS.** The multi-cell FLOOR drills that ride Id's cross-cell read-through: GA-D8 (per-cell DSR receipt set — multi-cell erasure produces a receipt per cell incl. the pseudonym shred), CP-D7 (cell→cell migration → 0 loss of authority), CP-D8 (cross-cell ref PII-free bridge → no cross-region PII). Quantified: 0 authority lost in a cell→cell migration; 0 cross-region PII in the bridge; a DSR receipt per member_cell incl. the pseudonym shred; cross-cell resolution is always cell-local (count of cross-region tuple pulls = 0).
- **TESTS (required).** Unit: a principal spanning cells is resolved in the object's cell (no cross-region tuple pull); a cross-cell coarse grant is read-through zookie-bounded; a cell→cell migration preserves authority. CDC: provider+consumer pair for the cross-cell read-through over 12.6. Drill: GA-D8/CP-D7/CP-D8 on the harness (SCHED). Mutation floor: the cell-local-resolution + the no-cross-region-pull invariant are core — a mutation that pulls tuples cross-region MUST be caught.
- **DEFINITION OF DONE.** Multi-cell principal authority is live (home-cell-authoritative + cross-cell read-through, zookie-bounded, resolution always cell-local, 0 cross-region tuple pulls); the per-cell DSR receipt set works; GA-D8/CP-D7/CP-D8 emit dated green artifacts (0 authority loss, 0 cross-region PII, per-cell receipt incl. pseudonym shred); the single-home-cell floor is recorded CLOSED; lints (residency-pin) green; CDC passes; mutation floor met. Committed.
- **COMMIT.** Header `P-<NNN> M5: multi-cell principal authority (cross-cell read-through)`; body lists the cross-cell read-through over 12.6, GA-D8/CP-D7/CP-D8 green, the single-home-cell floor closed. Co-Authored-By trailer.

---

## Coverage note (this file → roadmap milestones; the index's coverage matrix is authoritative)

- **ID-M0** (lints + glue crate + envelope fields) → P-ID-01 (the eleven contract signatures + SetExpr/CaveatContext), P-ID-02 (the iam.* tokens + envelope projections + 1.8 signal constants), P-ID-03 (the four Id lints).
- **ID-M1** (the whole Id surface) → P-ID-04 (service shell), P-ID-05 (S1), P-ID-06 (authenticate human/SSO), P-ID-07 (authenticate token/machine-identity), P-ID-08 (S3 + write_tuples + zookie-write), P-ID-09 (check engine), P-ID-10 (ReBAC engine + admit-contract + core hierarchy), P-ID-11 (list_objects Ids path + S4/S8), P-ID-12 (SetExpr lowering + S8 watermark), P-ID-13 (list_subjects + explain), P-ID-14 (S7 + revoke + SCIM-disable), P-ID-15 (fail-static S6 + the 4.11 bound + zookie-bypass), P-ID-16 (S5 read-replica), P-ID-17 (delegation), P-ID-18 (mint_run_token), P-ID-19 (S2 + grammar), P-ID-20 (resolve_pseudonym + erase + crypto-shred), P-ID-21 (the M1→M2 exit-gate scorecard). Drills greened: ID-D3 (P-ID-10), ID-D4/ID-D7 (P-ID-12), ID-D1 (P-ID-14), ID-D2 (P-ID-15), ID-D5 (P-ID-17), ID-D6 (P-ID-18), ID-D8 (P-ID-20), all re-asserted as the consolidated gate by P-ID-21, + the no-untagged-personal-data/STOR-D4 assertions (P-ID-05/P-ID-19/P-ID-20).
- **ID-M2** (first consumption; caveat-core promotion; watcher) → P-ID-22 (CaveatContext → QueryAst core, the literal-only floor closed), P-ID-23 (watcher fanout + ID-D5 re-run against EffectApi + SRCH/REF/NOTIF rides).
- **ID-M3/M4** (the five namespace fragments) → P-ID-24 (Git fragment, GIT-D8/D11), P-ID-25 (Git pseudonymous commits, GIT-D2), P-ID-26 (KN fragment, KN-D5/D13), P-ID-27 (CI fragment, CI-D10 secret-non-inheritance), P-ID-28 (self-hosted-runner scope, CI-D10 scope), P-ID-29 (Issues fragment, ISS-D3), P-ID-30 (Chat fragment). The engine-only floor (P-ID-10) closed by P-ID-30.
- **ID-M5** (surge + S8 tunables + ID-D8 cell + E2E spine + multi-cell) → P-ID-31 (ID-D9 surge), P-ID-32 (S8 tunables finalised; the cardinality-cap floor closed), P-ID-33 (ID-D8 cell-scale + cell-bulkhead), P-ID-34 (E2E-1..E2E-4 spine), P-ID-35 (multi-cell, GA-D8/CP-D7/CP-D8; the single-home-cell floor closed).
- **ID-M6** (dogfood) → NO new Id work (per the roadmap §2 "M6"): Id's authz runs on the platform's own commits/issues/docs/channels/CI; covered by the M6 dogfood prompts (the self-hosting CI graph mints/revokes Id tokens) + a truth-up pass — no dedicated Id implementation prompt is owed. Recorded here so the milestone is not silently dropped.

**Floor → follow-on pairs (each visible, never invisible):** contract bodies deferred (P-ID-01/P-ID-02 M0) → the M1 bodies (P-ID-04..P-ID-20); iam.* emit deferred (P-ID-02) → write_tuples emit (P-ID-08); fail-closed-stub shell (P-ID-04) → the handler bodies (P-ID-06/09/11/12); S7-denylist stub (P-ID-07) → full S7 + revoke (P-ID-14); literal-only caveat (P-ID-09) → QueryAst core (P-ID-22); SetExpr-lowering + watermark-read (P-ID-11/P-ID-08) → the lowering + watermark (P-ID-12); engine-only ReBAC (P-ID-10) → the five fragments (P-ID-24/26/27/29/30, closed by P-ID-30); S8 cardinality cap (P-ID-11) → finalised at scale (P-ID-32); single-home-cell (P-ID-10/11) → multi-cell (P-ID-35); pseudonym shred (P-ID-20) → Git pseudonymous commits (P-ID-25) + audited history-rewrite (M5/on-demand, named, owned by the Git/GDPR roadmaps); authenticate v1 credential set (P-ID-06/07) → hardware-attestation/passkey-sync/SLO (P5/P6, named, post-M5); W=5min bound (P-ID-15) → DPO ratification L-1 (parallel legal, named). The cross-band pseudonym seam (grammar frozen M1, consumed Git M3) keeps declaration order: P-ID-19 (M1) precedes P-ID-25 (M3). The M5-hardening floor (Id correct at M1, hardened at M5) is named in P-ID-21 → P-ID-31/P-ID-35.
