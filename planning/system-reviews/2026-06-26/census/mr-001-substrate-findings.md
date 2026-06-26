# MR-001 — Substrate census (adversarial): the silent stubs in the shared organs

Date: 2026-06-26. Status: CENSUS (read-only; no code changed). Author: MR-001 (Opus, security-critical).
Scope: `myelin-identity` + `myelin-identity-service`, `myelin-storage`, `myelin-events`,
`myelin-control-plane`, `myelin-tenancy`.

## Executive summary

- **57 findings** across the five substrate crates (identity 7, storage 9, events 11, control-plane 27,
  tenancy 3). Two of those (control-plane F-cp-12, F-cp-27) are recorded as *genuinely real* for balance.
- **By severity: ~12 CRITICAL, ~18 HIGH, ~13 MEDIUM, ~5 LOW** (the RLS pooled-bleed is one finding counted
  once across storage + tenancy; control-plane contributes the long MEDIUM tail of dead "zero" tripwires).
- **The single most dangerous thing:** the **production authentication graph runs on mock crypto**. Every
  identity verifier/signer wired by default in `identity_app_spec` is a `Structural*` impl that parses (or
  emits) a **plaintext, pipe-delimited string** instead of verifying (or producing) a signature
  (`authenticate.rs:146`, `machine_auth.rs:262`, `mint.rs:164`). There is **no signature, no MAC, no
  attestation** on any credential or token in the default build. Anyone who can present a credential blob can
  **forge any principal, in any tenant, with any grant set** by composing the six-field string
  `"<tenant>|<region>|<subject>|<jti>|<dpop>|<grants>"`. This is a total auth bypass, and the CDC/ drill tests
  pass on it because they *construct that exact string themselves* and assert the parse round-trips (shape, not
  adversary). Combined with the storage RLS pooled-connection bleed (F-storage-2 / F-tenancy-3), the platform
  has no real trust boundary today.
- **The structural pattern across all five crates:** load-bearing state lives in `HashMap`/`BTreeMap`/`Mutex`
  in one process; the "durable store" / "real crypto" / "real broker" / "real backup" is named as a deferred
  floor (P-S12/P-S15/EB-04/etc.); the real backings that *do* exist (NATS JetStream, S3, Valkey, Postgres RLS)
  are `#[cfg(feature = "integration")]` and compiled out of every default `cargo test`; and the tests assert
  struct/serde shape or same-process state transitions, so a stub *is* what the green gate certifies.

---

## A — `myelin-identity` + `myelin-identity-service`

### F-identity-1: Human/SSO credential verification is mock crypto (StructuralVerifier) in the production graph
- **Location:** `crates/myelin-identity-service/src/authenticate.rs:StructuralVerifier` (146–184); wired as the
  default at `HumanSsoAuthenticator::new` → `with_verifier(store, Arc::new(StructuralVerifier::new()))` (288–290).
- **Claimed (per ledger/contract):** P-526 — real OIDC JWKS / SAML XML-DSig / WebAuthn attestation / SSH
  challenge-response credential cryptography; the `Structural*` impl survives only as a `#[cfg(test)]` fake.
- **Built (actual):** mock-crypto. `verify()` splits `credential.material` on `|` into
  `<tenant>|<region>|<subject_key>` and returns a `VerifiedAssertion` from those three plaintext fields. No
  signature, no IdP, no attestation — the doc itself says "this is NOT a security claim about the bytes" (141).
  It is the DEFAULT on the production constructor, not a test-only fake.
- **Gap:** There is no cryptographic verification of any human/SSO credential anywhere in the default build. A
  caller supplies the asserted facts directly in cleartext.
- **Test-passes-on-stub?:** YES — `tests/cdc_4_1_authenticate.rs::cdc_4_1_authenticate_provider_resolves_consumer_trusts_principal`
  (and `_tenant_is_from_credential_not_path`) build the credential via a local `material(tenant,region,subject)`
  helper (test line 39) that formats the very string `StructuralVerifier` parses, then assert the returned tenant
  equals the one they wrote. Pure shape; there is no forged/expired/tampered corpus, so the stub is the system
  under test.
- **Blast radius:** CRITICAL — forge any human identity in any tenant by composing a 3-field string; full
  authentication bypass.
- **Maps to:** P-526 (impl), P-528 (verify); roadmap E0.5.

### F-identity-2: Machine/capability token verification is mock crypto (StructuralTokenVerifier) in the production graph
- **Location:** `crates/myelin-identity-service/src/machine_auth.rs:StructuralTokenVerifier` (262–339); wired as
  the default at `CapabilityAuthenticator::new` → `with_verifier(store, Arc::new(StructuralTokenVerifier::new()), S7Denylist::new())` (403–409).
- **Claimed (per ledger/contract):** P-527 — verify a real PASETO v4 / biscuit caveat-chain signature, the
  caveat chain (monotone attenuation), `iss/aud/exp/nbf/jti`, an RFC 9449 DPoP proof-of-possession, and a real
  TPM/provisioning attestation. The `Structural*` impls survive only as `#[cfg(test)]` fakes.
- **Built (actual):** mock-crypto. `verify()` splits `material` into six `|`-fields
  `<tenant>|<region>|<subject_key>|<jti>|<dpop:0|1>|<grants_csv>` and builds the `CapabilityToken` directly from
  them. The `dpop_bound` is **read from the string flag** (313–321), not from a verified proof; the grant
  authority is **taken verbatim** from the CSV (323–327) — there is no signature over it, so attenuation cannot
  be cryptographically enforced. No PASETO/biscuit/DPoP/TPM crypto exists in the default build.
- **Gap:** A presented token is trusted because it *parses*, not because it is *signed*. The authority/grants,
  tenant, jti, and DPoP-binding are all attacker-suppliable cleartext.
- **Test-passes-on-stub?:** YES — `tests/cdc_4_7_mint_run_token.rs` and `tests/drill_id_d6_run_token.rs` mint a
  token via `StructuralTokenSigner` (F-identity-3) and verify it via `StructuralTokenVerifier`, asserting the
  round-trip (`token.token.contains("repo:acme/web#read")`). Signer and verifier are two halves of the same
  string format; no forged-signature or amplified-grant adversary is in the corpus.
- **Blast radius:** CRITICAL — forge any machine/CI/agent/PAT token with arbitrary grants and tenant; privilege
  escalation and cross-tenant access with no signature to defeat.
- **Maps to:** P-527 (impl), P-528 (verify); roadmap E0.5.

### F-identity-3: Token minting is a plaintext string formatter (StructuralTokenSigner), not a signer, in the production graph
- **Location:** `crates/myelin-identity-service/src/mint.rs:StructuralTokenSigner` (164–189); wired as the
  default at `RunTokenMinter::new` (253) and `RunTokenMinter::with_tuple_store` (265), the latter being what the
  production `StoreBackedCheck::with_index` assembles (`lib.rs:545`).
- **Claimed (per ledger/contract):** P-527 — mint cryptographically signed PASETO/biscuit envelopes with a
  caveat chain, the signing key sourced from the KMS (P-524).
- **Built (actual):** mock. `sign()` returns `format!("{tenant}|{region}|{subject_key}|{jti}|0|{grants}")` — a
  cleartext, unsigned, un-MAC'd string; `dpop` is hardcoded `0`. There is no key, no signature.
- **Gap:** A "minted token" is a forgeable string; it carries no secret an adversary lacks. The KMS-sourced
  signing key (P-524) is not consulted.
- **Test-passes-on-stub?:** YES — `tests/cdc_4_7_mint_run_token.rs::cdc_4_7_minted_token_honoured_within_run_life`
  asserts the minted string contains the expected grant substring; the attenuation tests assert set-intersection
  over the CSV. Nothing asserts unforgeability against a holder of no key.
- **Blast radius:** CRITICAL — any party can mint a valid-looking per-run/capability token; pairs with
  F-identity-2 to make the whole token surface forgeable.
- **Maps to:** P-527 (impl), P-528 (verify); roadmap E0.5.

### F-identity-4: Revocation (S7) and the token denylist are in-memory; the "durable mirror" and "crash" are same-process map copies
- **Location:** `crates/myelin-identity-service/src/machine_auth.rs:S7Denylist` (347–372, `Arc<Mutex<BTreeSet<String>>>`);
  `crates/myelin-identity-service/src/revocation.rs:Inner` (145–162, `mirror: BTreeMap`, `fast: BTreeMap`,
  `run_teardowns: BTreeSet`), `RevocationStore::new` (180).
- **Claimed (per ledger/contract):** P-522/P-527 — a durable, replicated, idempotent S7 denylist (Redis/Valkey
  hot layer + Postgres mirror) that an `authenticate` fail-closes against; a revoke is never lost on crash.
- **Built (actual):** in-memory. Both the "fast" and the "durable mirror" layers are `BTreeMap`s in one process;
  `recover_from_mirror()` copies one in-process map into another. The denylist is an in-process `BTreeSet`.
- **Gap:** No durable store. A real process restart loses every revocation → a revoked/teardown'd token (e.g. a
  killed CI run's per-run token) becomes live again. The replicated multi-instance consistency the contract
  claims does not exist (one process only).
- **Test-passes-on-stub?:** YES — `tests/drill_id_d1_revocation.rs` (the SLA/crash drill): the "SIMULATED CRASH"
  is the single line `s7.recover_from_mirror();` (test line 226), then it asserts the deny still holds. Both
  layers are in-memory in the same process; there is no `kill -9`, no reopen, no second instance. A stub passes
  because the durable layer it "recovers from" is itself the stub.
- **Blast radius:** HIGH — revocations and run-teardowns do not survive a real restart; a revoked credential can
  resurrect (the P-523 crash-recovery drill is unmet).
- **Maps to:** P-522 (impl), P-523 (verify); roadmap E0.3.

### F-identity-5: The S1 principal store (identity system-of-record) is an in-memory HashMap
- **Location:** `crates/myelin-identity-service/src/principal_store.rs:Inner` (249–271, `partitions/profiles/credential_links: HashMap<(String,String), HashMap<…>>`),
  `PrincipalStore` (280–281, `inner: Arc<Mutex<Inner>>`), `PrincipalStore::new` (295). Module doc admits "the
  in-memory store models the SQL S1 table … no live OLTP database until the driver lands (P-S15)" (61–62).
- **Claimed (per ledger/contract):** P-522 — the S1 principal table bound to the live OLTP pool; durable across
  restart; RLS-enforced cross-tenant isolation at the DB.
- **Built (actual):** in-memory. Principals, encrypted profiles, and SSO/SCIM credential links live in
  per-`(tenant,region)` `HashMap`s under one `Mutex`. Cross-tenant isolation is "you can't index another
  partition key" in-process, not DB RLS.
- **Gap:** All identity state is lost on restart; there is no durable principal of record, no real RLS.
- **Test-passes-on-stub?:** YES — `principal_store.rs` in-file tests and `tests/cdc_4_1_*` assert insert/read and
  cross-tenant-empty-read against the live `HashMap` within one process; the model is the implementation.
- **Blast radius:** HIGH — restart loses every principal (mass lockout) and there is no durable authoritative
  identity store; the RLS isolation claim is in-process only.
- **Maps to:** P-522 (impl), P-523 (verify); roadmap E0.3.

### F-identity-6: The S3 relation-tuple store (the ReBAC authorization graph) is an in-memory HashMap
- **Location:** `crates/myelin-identity-service/src/tuple_store.rs:Inner` (200–211, `partitions: HashMap<(String,String), HashMap<TupleKey,StoredTuple>>`),
  `TupleStore` (219–220, `inner: Arc<Mutex<Inner>>`), `TupleStore::new` (240). Module doc: "the in-memory store
  models the SQL S3 table" (62).
- **Claimed (per ledger/contract):** P-522 — the S3 tuple table on the live OLTP pool; the durable authorization
  graph `check`/`list_objects` resolve over; per-tenant-DEK-pinned, RLS-isolated.
- **Built (actual):** in-memory `HashMap`. The "emit via outbox" path writes to the in-memory `OutboxStore`
  (which is itself the events stub — see F-events-1). All authorization tuples vanish on restart.
- **Gap:** The entire authorization graph is non-durable and single-process; the durable store is deferred.
- **Test-passes-on-stub?:** YES — `tests/cdc_4_6_write_tuples.rs`, `tests/cdc_4_2_check.rs` and the rebac
  integration test assert read-after-write and cross-tenant denial against the in-memory map; no restart, no DB.
- **Blast radius:** HIGH — restart loses all authorization grants; combined with F-identity-2 (forgeable tokens)
  the authorization layer is neither durable nor cryptographically anchored.
- **Maps to:** P-522 (impl), P-523 (verify); roadmap E0.3.

### F-identity-7: The production assembly (`identity_app_spec` / `StoreBackedCheck`) wires the stubs as the DEFAULT — and mints a fresh in-process KMS per construction
- **Location:** `crates/myelin-identity-service/src/lib.rs:identity_app_spec` (1534) and
  `StoreBackedCheck::with_index` (540–550): constructs `RevocationStore::new()` (in-memory S7),
  `RunTokenMinter::with_tuple_store(...)` (StructuralTokenSigner default), and
  `KmsEngine::new()` (548 — a fresh random cell root each construction; see F-storage-7). `main.rs` boots over
  `Config::default()` because env-first config is itself a deferred floor (P-S15).
- **Claimed (per ledger/contract):** The production identity service: real crypto verifiers, durable stores,
  KMS-sourced keys, env-driven config — assembled behind the unchanged seams.
- **Built (actual):** partial/stub-by-default. The seams (`with_verifier`/`with_signer`/`with_kms`) exist, but
  **no production caller passes a real crypto verifier/signer or a durable store** — the only non-`#[cfg(test)]`
  constructions in the crate use the `Structural*` defaults and in-memory stores. The prod graph IS the stub
  graph.
- **Gap:** "Swap the real one in behind the seam" never happened on the production path; the seam is wired to the
  mock. The KMS is per-process and ephemeral, so even the encryption that exists cannot survive a restart.
- **Test-passes-on-stub?:** YES — `lib.rs::tests` boots `serve(identity_app_spec(Config::default()))` (1864–1870)
  and asserts a clean lifecycle/drain; it asserts the shell boots, not that any real crypto/store is present. The
  P-528/P-523 "no structural verifier / no in-memory store in the production graph" absence-scanner does not yet
  exist, so nothing fails the build on the mock being in the prod graph.
- **Blast radius:** CRITICAL — this is the finding that makes F-identity-1..6 *production* facts rather than
  test-only fakes: the dangerous defaults are what `main` ships.
- **Maps to:** P-526/527/528 (crypto), P-522/523 (stores), E0.2 (the absence-scanner that must catch this),
  E0.5.

---

## B — `myelin-storage`
(Cryptographic core is genuinely real: RustCrypto AES-256-GCM wrap/unwrap, real crypto-shred, BLAKE3
content-addressing, real fail-static ladder. The rot is in durable persistence, backup/restore, and the
isolation gate — and all real backings are `#[cfg(feature="integration")]`, compiled out of default `cargo test`.)

### F-storage-1: The OLTP "pool" (system of record) is an in-memory permit counter, not a database
- **Location:** `crates/myelin-storage/src/oltp.rs:OltpPool` (128–218); state `Arc<Mutex<PoolState>>` with
  `HashMap<TenantId,u32>` (143–151).
- **Claimed (per ledger/contract):** Tier-1 OLTP client — Postgres-class, one DB per service, system of record
  (contract 11.1).
- **Built (actual):** in-memory model. `acquire`/`PermitGuard` count permits in a `Mutex<HashMap>`; no
  connection, no statement, no DB. Doc: the real `tokio-postgres`/`sqlx` pool "lands when `serve`'s pool body
  does (P-S12)."
- **Gap:** The system-of-record client holds zero durable state; it models saturation accounting only.
- **Test-passes-on-stub?:** YES — `oltp.rs::global_saturation_fast_fails_and_signals`,
  `::acquire_and_release_accounts_permits` assert counter arithmetic; the model *is* the implementation.
- **Blast radius:** HIGH — anything believing `OltpPool` persists loses it all; false durability/backpressure signal.
- **Maps to:** P-522/523; E0.3.

### F-storage-2: RLS GUC is SESSION-scoped (`set_config(...,false)`) on a pooled connection with no reset-on-release + a bare-pool escape hatch → cross-tenant leak on connection reuse
- **Location:** `crates/myelin-storage/src/pg.rs:set_session_scope_in_region` line 413
  (`set_config('myelin.tenant_id',$1,false)`); escape hatch `PgStore::pool` (150–152); `scoped_conn` (374–385);
  RLS policy (183–190) keyed on `current_setting('myelin.tenant_id', true)`.
- **Claimed (per ledger/contract):** RLS at the DB — "a session for tenant A can only ever read tenant A's rows";
  the IDOR floor lives in Postgres (P-531).
- **Built (actual):** real RLS, but the GUC is set with the third arg `false` (SESSION scope), with **no
  `SET LOCAL`/transaction scoping and no `RESET`/`DISCARD ALL`/`after_release` reset**. Every public method
  re-sets the scope, so the happy path is safe, but: (a) `pub fn pool()` hands out the raw pool — a caller can
  `acquire()` and query with no scope, or inherit the previous borrower's still-set `myelin.tenant_id`; (b) a
  reused connection retains tenant A's GUC and returns tenant A's rows to whoever forgot to re-scope.
- **Gap:** Scope must be transaction-scoped (`SET LOCAL` / `set_config(...,true)`) or reset on release; the
  bare `pool()` accessor bypasses scoping entirely.
- **Test-passes-on-stub?:** YES (worse: no test exists for the vector) — the isolation drills run only under
  `--features integration` and always set the scope on the same connection they query; **no test exercises
  connection reuse with a stale GUC or the `pool()` hatch.** The leak is invisible to the suite.
- **Blast radius:** CRITICAL — cross-tenant data read via pooled-connection reuse; the platform's load-bearing
  `CrossTenantCount==0` is not enforced on the reuse path. (Same defect catalogued from the tenancy angle as
  F-tenancy-3.)
- **Maps to:** P-531; E0.5.

### F-storage-3: All real backings (PG/S3/Valkey/RLS) are behind `--features integration`; the default build + CI is 100% in-memory, including the isolation/residency "drills"
- **Location:** `Cargo.toml` `default = []`; `src/backend.rs:blob_store/cache` (33–56) reach
  `S3BlobStore`/`ValkeyCache` only for `Backend::Real`; `src/pg.rs`, `src/s3blob.rs`, `src/valkey.rs` are all
  `#[cfg(feature="integration")]`.
- **Claimed (per ledger/contract):** RustFS/Valkey/Postgres real backings; residency/RLS enforced at the DB.
- **Built (actual):** default build compiles none of them. The IDOR drill runs against `TenantScope::resolve`
  (in-memory model); residency `admit_write` is an in-process `Region` compare.
- **Gap:** The "enforced at the DB" guarantees exist only in an opt-in build ordinary `cargo test` never selects.
- **Test-passes-on-stub?:** YES — `tests/idor_drill.rs::idor_drill_zero_path_derived_tenants` asserts
  `resolve()` returns the token tenant, which `rls.rs:resolve` hardcodes (`path_derived:false`, line 119).
- **Blast radius:** HIGH — a green CI run certifies in-memory behavior; the real Postgres RLS/residency path
  (carrying F-storage-2) is never exercised by default.
- **Maps to:** P-522/P-531; E0.2/E0.3.

### F-storage-4: Backup is a modeled WAL offset + metadata-only object tier — no bytes, no `pg_basebackup`/WAL shipping
- **Location:** `crates/myelin-storage/src/backup.rs:ContinuousArchiver` (259–389), `ObjectTierBackup`
  (398–477), `BaseBackup`/`WalSegment` (168–186).
- **Claimed (per ledger/contract):** continuous WAL archiving + base backups → PITR, RPO ≤ 5 min (P-529).
- **Built (actual):** modeled. `ContinuousArchiver` holds `Vec<WalSegment{end_offset,committed_at}>` and
  `Vec<BaseBackup{at_offset,taken_at}>` — pure scalars, no data. `ObjectTierBackup` stores metadata only, never
  bytes. `measure_rpo` is `committed_at − latest_archived_at`. Doc: "modeled WAL, not a live Postgres" (56–66).
- **Gap:** A modeled offset is not a backup; nothing is shipped off-host; there is no artifact to restore.
- **Test-passes-on-stub?:** YES — `backup.rs::continuous_archiving_holds_rpo_within_the_window` asserts u64
  arithmetic (`rpo == 110`); `object_tier_is_versioned_and_replicated` asserts a version counter.
- **Blast radius:** CRITICAL — silent data loss; RPO/PITR numbers are fiction until the real WAL-G/`pg_basebackup`
  driver lands.
- **Maps to:** P-529/530; E1.1.

### F-storage-5: Restore is a modeled `restore_to_offset` over in-memory rows — no `pg_restore`/WAL replay
- **Location:** `crates/myelin-storage/src/restore.rs:restore_to_offset` (360–415); operates on `&[WalRow]`,
  `BlobPresence` (a `BTreeSet<ContentHash>`), `SourceLog` (a `Vec`); KEK "restore" is `kms.backup_snapshot()` (404).
- **Claimed (per ledger/contract):** PITR restore-to-consistent-point(T) via real `pg_restore` + WAL replay,
  ContentHash verification, reindex-from-source, restore KEKs except crypto-shredded (P-529).
- **Built (actual):** modeled. Row filtering, set-membership blob presence, and `SourceLog` replay over in-memory
  vecs. Doc: "modeled restore, not a live `pg_restore`" (60–70).
- **Gap:** No database is restored; the cross-seam logic is real over the model, but there is no real WAL replay.
- **Test-passes-on-stub?:** YES — `restore.rs::the_whole_restore_lands_at_one_consistent_point`,
  `::a_missing_referenced_hash_makes_restore_fail` assert over hand-built vecs.
- **Blast radius:** CRITICAL — the recovery path the whole durability story rests on never touches a real store.
- **Maps to:** P-529/530; E1.1.

### F-storage-6: The "permanent" restore-verify gate verifies a fresh in-memory `RestoreTarget`, never a restored database
- **Location:** `crates/myelin-storage/src/restore_verify.rs:RestoreVerifyGate::run` (440–527); `RestoreTarget`
  (147–158) is in-memory `Vec`/`BTreeMap`. (The checksum-parity leg uses real BLAKE3; `run_or_fail_ci` (619) is
  properly loud/`#[must_use]`.)
- **Claimed (per ledger/contract):** a permanent gate — spin a clean target, restore T1/T2/T5, assert no
  loss/cross-seam/erasure-held; RED fails CI forever (P-530).
- **Built (actual):** modeled. "Spin a clean target" = build a fresh in-memory `RestoreTarget` driving the
  modeled `restore_to_offset` (F-storage-5). Doc: "no live Postgres / object store on this floor" (66–74).
- **Gap:** The gate proves a model is self-consistent, not that a real backup restores a real database; it cannot
  fail on a real `pg_restore` defect it never runs.
- **Test-passes-on-stub?:** YES — `restore_verify.rs::the_gate_greens_a_whole_restore_with_measured_numbers`
  asserts green over hand-built objects/rows.
- **Blast radius:** HIGH — false assurance: the "durability gate" is green while no real restore was ever verified.
- **Maps to:** P-530; E1.1.

### F-storage-7: The KMS holds all key material in process memory; `backup_snapshot` omits the KEKs and cell root — keys cannot survive a restart or be restored
- **Location:** `crates/myelin-storage/src/kms.rs:KmsEngine` (470–478: `root: CellRoot`, `keks: Mutex<BTreeMap>`,
  `deks: Mutex<BTreeMap>` — all in-process); `KmsEngine::backup_snapshot` (685–698) emits only
  `(DekId, WrappedDek)` — never the KEKs or the cell root; `KmsEngine::new()` generates a fresh random `CellRoot`
  each construction.
- **Claimed (per ledger/contract):** P-524 — HSM-class KMS, root sealed in HSM and never in process; durable
  wrapped KEK/DEK envelopes; backup carries restorable (HSM-sealed) keys excluding crypto-shredded.
- **Built (actual):** in-memory. KEKs/DEKs live only in `Mutex<BTreeMap>`; the snapshot returns wrapped DEKs that
  are ciphertext under KEKs that are not in the snapshot and a root that is never persisted. The crypto-shred
  *exclusion* logic is real; the snapshot is not a restorable backup. On restart (or restore into a fresh engine)
  every wrapped DEK is undecryptable.
- **Gap:** A real KMS backup must carry HSM-sealed KEKs/root; this carries only inner envelopes. `restore.rs:404`
  "restores KEKs" via this DEK-only snapshot, so no KEK is ever restored. This is the same engine
  `identity_app_spec` constructs fresh per process (F-identity-7).
- **Test-passes-on-stub?:** YES — `kms.rs::backup_snapshot_excludes_a_crypto_shredded_tenant` asserts a shredded
  tenant is absent; **no test asserts a snapshot can re-derive working keys** after a fresh-engine restore.
- **Blast radius:** CRITICAL — total key loss on restart/restore ⇒ all encrypted columns/blobs permanently
  unrecoverable (silent data loss); the HSM/Shamir backing is entirely deferred.
- **Maps to:** P-524/525; E0.3/Tier-4.

### F-storage-8: The RLS guard module is a type/string model — `predicate_sql` is never executed and `resolve` hardcodes the verdict
- **Location:** `crates/myelin-storage/src/rls.rs:TenantScope::resolve` (110–123, `path_derived:false` hardcoded);
  `TenantQuery::predicate_sql` (210–217, unescaped `tenant = '{}'` interpolation, never run).
- **Claimed (per ledger/contract):** the (tenant,region)-first RLS tenant-scoping guard = the IDOR floor
  (mandatory-core).
- **Built (actual):** a real, valuable *compile-time* guard (you cannot mint a `TenantScope` from a path; the
  `compile_fail` doctest is genuine) + a string formatter. The runtime `resolve` unconditionally returns the
  token tenant; `predicate_sql` is never executed against any DB in this crate.
- **Gap:** Runtime enforcement is the DB RLS policy (integration-only, carrying F-storage-2). This module is a
  fixture + formatter, not enforcement.
- **Test-passes-on-stub?:** YES — `rls.rs::token_tenant_wins_over_path_tenant` and `idor_drill.rs` pass trivially
  because `resolve` is hardcoded to the asserted answer.
- **Blast radius:** MEDIUM — the compile-time guard has real value; the suite's "IDOR proof" tests a constant;
  real enforcement is elsewhere and unverified by default.
- **Maps to:** P-531; E0.5.

### F-storage-9: The git crypto-shred "reach" verifies only the blob DEK is gone; the reflog/bitmap/pack structures it claims to reach are a hardcoded enum
- **Location:** `crates/myelin-storage/src/git_shred.rs:GitCryptoShredReach::shred_git_structures` (248–277);
  `structures_reached: GitShreddable::ALL.to_vec()` (272).
- **Claimed (per ledger/contract):** crypto-shred reaches reflogs/bitmaps/pack-tier backups — verified, not
  assumed (§5.3, GIT-D2).
- **Built (actual):** it destroys the per-tenant blob DEK and probes its absence (both real), but there are no
  actual reflog/bitmap/pack structures; `structures_reached` is a static list and the receipt asserts coverage by
  enum cardinality.
- **Gap:** "Verified, not assumed" holds for the DEK, not for the structures the gate names.
- **Test-passes-on-stub?:** YES — `git_shred.rs::reach_covers_every_shreddable_structure_and_names_the_residual`
  asserts the hardcoded `GitShreddable::ALL` list.
- **Blast radius:** MEDIUM — the crypto-shred is real; the risk is over-claimed reach coverage.
- **Maps to:** P-532/533; E1.1.

---

## C — `myelin-events`
(By its own docs a "floor": every durable organ is an in-memory model behind a trait; the one real durable client
(`nats.rs`) is `#[cfg(feature="integration")]` and compiled out of every default build/test. Default `cargo test`
exercises only the in-memory models, so a stub *is* the implementation.)

### F-events-1: The transactional outbox is an in-memory `HashMap`, not a DB table
- **Location:** `crates/myelin-events/src/outbox.rs:Inner` (199, `rows: HashMap<EventId,OutboxRow>`, `order:Vec`,
  `next_seq:HashMap`, `claimed:HashSet`), `OutboxStore` (229), `OUTBOX_MIGRATION` const (112),
  `OutboxTransaction::commit` (429).
- **Claimed (per ledger/contract):** contract 2.3 — rows durable in Postgres via `INSERT … RETURNING` inside the
  caller's DB transaction; per-aggregate `seq` via `UNIQUE(aggregate,seq)`; relay claims via
  `SELECT … FOR UPDATE SKIP LOCKED`. The silent-data-loss floor.
- **Built (actual):** in-memory `Arc<Mutex<Inner>>`. `OUTBOX_MIGRATION` is a `const &str` of DDL nothing
  executes; commit mutates the HashMap under a process-local Mutex.
- **Gap:** Zero durability. Process death loses every committed-but-unsent row; `FOR UPDATE SKIP LOCKED` is
  modeled by a `HashSet` in one process — two real relay replicas would both claim every row.
- **Test-passes-on-stub?:** YES — `outbox.rs::commit_makes_event_and_state_durable_together`,
  `::dropped_transaction_emits_nothing_emit_iff_committed`,
  `::eb03_per_aggregate_seq_is_monotonic_and_gap_free_under_concurrent_emitters`. "Durable" = "still in the
  HashMap after `commit()` returned"; no restart, no DB, no second connection.
- **Blast radius:** CRITICAL — committed events lost on restart and double-claimed across replicas (duplicate
  delivery / lost events), the exact failure the floor claims to prevent.
- **Maps to:** P-522/523; E0.3.

### F-events-2: The consumer-dedup ledger is an in-memory `HashSet`, not a DB table
- **Location:** `crates/myelin-events/src/dedup.rs:DedupLedger` (87, `Arc<Mutex<HashSet<(ConsumerName,EventId)>>>`),
  `mark_handled` (102), `CONSUMER_DEDUP_MIGRATION` const (70).
- **Claimed (per ledger/contract):** contract 2.5 effectively-once — `(consumer,event_id)` PK in Postgres,
  `INSERT … ON CONFLICT DO NOTHING` in the same transaction as the handler's state write.
- **Built (actual):** in-memory `HashSet`; the migration DDL is a const nothing runs. `mark_handled` runs *before*
  `handle` then `revert`s on retry — a best-effort emulation, explicitly what the contract says it must not be.
- **Gap:** Dedup state is process-local; after restart the ledger is empty → every redelivery is treated as
  fresh. No transactional co-commit with the handler effect.
- **Test-passes-on-stub?:** YES — `dedup.rs::same_consumer_event_inserted_twice_is_one_effect`,
  `tests/cdc_2_5_consumer_dedup.rs::cdc_2_5_same_pair_inserted_twice_is_one_effect` (HashSet insert true-then-false
  in one process); `cdc_2_5_migration_is_the_frozen_pk_shape` asserts the const string *contains*
  `PRIMARY KEY (consumer, event_id)` — pure shape on a literal.
- **Blast radius:** HIGH — duplicate side effects after any consumer restart; effectively-once degrades silently
  to at-least-once.
- **Maps to:** P-522/523; E0.3.

### F-events-3: The relay publishes to an in-process fake bus by default; "delivery" is a synchronous function call
- **Location:** `crates/myelin-events/src/relay.rs:InProcessBus` (138), `BusInner` (143, `delivered:Vec`,
  `accepted_ids:HashSet`), `put` (213), `Relay::drain_once` (412), `OutboxStore::claim_unsent` (498).
- **Claimed (per ledger/contract):** contract 2.3 relay — the only component on the broker publish side; over the
  wire to a JetStream-class broker with `Nats-Msg-Id` dedup; cross-replica-safe claim.
- **Built (actual):** in-memory. The default/only transport tests exercise is `InProcessBus` — `put` pushes onto a
  `Vec` and dedups via a `HashSet`. "Delivery" is a synchronous method call, not a wire. Claim is over the
  same-process HashMap (F-events-1).
- **Gap:** No network, broker, real ack/redelivery, or cross-process anything.
- **Test-passes-on-stub?:** YES — `relay.rs::sub_d1_kill_between_commit_and_publish_zero_ghost_zero_lost`,
  `::relay_reclaim_after_crash_is_deduplicated_zero_ghost`, `::skip_locked_two_workers_never_double_claim`. The
  "crash"/"sever"/"two workers" are `InProcessBus::sever()` flips and two calls on one shared `Arc<Mutex>`; the
  fake defines the behavior asserted.
- **Blast radius:** CRITICAL — the headline no-ghost/no-loss durable-delivery gate is green against a `Vec`; real
  ordering-under-failure, at-least-once redelivery, and cross-replica claim are unproven.
- **Maps to:** P-522/523 (delivery), P-539 (`serve`); E0.3.

### F-events-4: The real NATS JetStream client exists but is compiled out of every default build/test
- **Location:** `crates/myelin-events/src/nats.rs:NatsJetStreamBus` (37), `connect` (53), `put`/`consume`/`ack`
  (154–240); `Cargo.toml` `integration = ["dep:async-nats", …]`, `default=[]`; tests
  `tests/integration_nats.rs:13` / `tests/smoke_nats_bus.rs:11` both `#![cfg(feature="integration")]`.
- **Claimed (per ledger/contract):** the real durable bus (EB-04) behind the `integration` feature.
- **Built (actual):** real — a genuine `async-nats` JetStream client (durable stream with `duplicate_window`,
  durable pull consumer, explicit ack, `Nats-Msg-Id` dedup) — but feature-gated; default `cargo build`/`test`
  compiles neither the code nor the tests. `pending: Mutex<HashMap>` of un-acked messages is itself process-local.
- **Gap:** The real path is never on the default gate; the outbox→relay→NATS seam ships unproven by default.
- **Test-passes-on-stub?:** N/A for the unit floor (real impl not in it). The gated `smoke_nats_bus.rs::nats_bus_put_consume_ack`
  asserts real broker behavior but only runs with a live NATS, and drives `BusTransport` directly (not the
  relay/outbox).
- **Blast radius:** HIGH — a real organ invisible to the default gate; seam regressions ship unproven.
- **Maps to:** P-522/523, P-539; E0.3.

### F-events-5: The idempotent consumer runtime's cursor/lag/in-flight are all in-memory; "0 lost across reconnect" is re-handing the same in-process objects
- **Location:** `crates/myelin-events/src/consumer.rs:Consumer` (370, `pending:Mutex<HashMap>`,
  `dead_letters:Mutex<Vec>`, `tenant_inflight:Mutex<HashMap<TenantId,HashSet<EventId>>>`), `deliver` (497),
  `lag` (466).
- **Claimed (per ledger/contract):** contract 2.4 seven-rule consumer; drop broker mid-stream → 0 lost across
  reconnect; durable broker cursor; bind-durable-by-name.
- **Built (actual):** in-memory; no broker cursor exists — "the cursor" is the dedup `HashSet` (F-events-2) + a
  per-subject `pending` counter. `PrefetchBound` is `take(bound)` over a test-supplied slice.
- **Gap:** "Reconnect" is the test constructing a new `Consumer`, re-passing the same cloned `DedupLedger` Arc and
  re-feeding the same in-memory `Message`s. No broker, redelivery, durable cursor, or `max_ack_pending`.
- **Test-passes-on-stub?:** YES — `consumer.rs::reconnect_rebinds_by_name_zero_lost_zero_dup`,
  `::retry_does_not_ack_redelivery_reruns_then_succeeds`, `::deliver_lane_honours_bounded_prefetch` assert the
  in-memory counters; the test feeds the messages again itself.
- **Blast radius:** HIGH — the SUB-D2 silent-data-loss floor is proven without a broker.
- **Maps to:** P-522/523, P-539; E0.3.

### F-events-6: Crypto-shred / `PersonalDataHolder` runs against an in-memory `BTreeSet` "KMS" and an in-memory event log
- **Location:** `crates/myelin-events/src/holder.rs:InMemoryShredder` (140, `live:Arc<Mutex<BTreeSet<String>>>`),
  `destroy_key` (173), `BusEventLog` (211), `BusHolder::erase` (366).
- **Claimed (per ledger/contract):** contract 2.7 + 11.3/11.4 — erasure destroys the per-subject DEK in the real
  `KmsEngine::destroy_dek` so live-log AND backup ciphertext is unrecoverable (BUS-D8).
- **Built (actual):** modeled. `destroy_key` removes a `String` from a `BTreeSet`; `is_live` is `set.contains`.
  No KMS, no encryption, no ciphertext.
- **Gap:** No real key destruction; the live `KmsEngine` binding is the deferred floor P-GA-06.
- **Test-passes-on-stub?:** YES — `holder.rs::erase_destroys_dek_emits_tombstones_zero_recoverable`,
  `tests/drills_bus_d8_crypto_shred.rs` — `recoverable_remaining==0` re-checks `is_live` against the same set
  `destroy_key` just mutated (tautological on the fake).
- **Blast radius:** HIGH — a GDPR right-to-erasure gate green against a HashSet; real ciphertext unrecoverability
  unproven.
- **Maps to:** P-522/523 (real KMS binding); GDPR track.

### F-events-7: Post-restore re-erasure ledger is in-memory and the "restore" is a hand-built resurrection
- **Location:** `crates/myelin-events/src/reerase.rs:BusErasureLedger` (102, `Arc<Mutex<BTreeMap<String,ErasedSubject>>>`),
  `re_erase_after_restore` (279); test `tests/cdc_10_8_bus_reerase.rs:87`.
- **Claimed (per ledger/contract):** contracts 10.8/11.5 — a durable, non-shred-erasable erasure ledger that
  survives a real backup/restore so a restored older backup cannot resurrect an erased key (GD-14, 0 resurrected).
- **Built (actual):** modeled. Ledger is an in-memory `BTreeMap`; the "restore that resurrects the key" is the
  test calling `shredder.seal(&key)` and rebuilding a `BusEventLog` by hand.
- **Gap:** No durable ledger, no real restore path (deferred floors P-ST-14, P-GA-06).
- **Test-passes-on-stub?:** YES — `cdc_10_8_bus_reerase.rs::cdc_10_8_provider_restores_consumer_re_erases_zero_resurrected`,
  `reerase.rs::re_erase_after_restore_re_destroys_resurrected_keys` (re-check `is_live` on the set the pass cleared).
- **Blast radius:** HIGH — the "erasure survives restore" GDPR guarantee is unproven against any real backup/ledger.
- **Maps to:** P-522/523; GDPR track.

### F-events-8: The firehose (resume-cursor / zero-loss replay transport) is an in-memory `HashMap` of ring buffers
- **Location:** `crates/myelin-events/src/firehose.rs:Firehose` (618), `windows: HashMap<(String,FirehoseScope),RetentionWindow>`
  (620), `subscribers: HashMap<…,Vec<SubHandle>>` (624), `RetentionWindow` (355), `publish` (384).
- **Claimed (per ledger/contract):** contract 3.5 — a durable firehose with `publish/tail/subscribe/resume`,
  per-(stream,scope) monotonic seq, `(last_seq,now]` backfill losing ZERO ops on reconnect; the CI-log/collab/chat
  live transport.
- **Built (actual):** in-memory ring buffers in a `HashMap`; subscribers a `Vec` of in-process handles. Doc: "the
  real durable transport is the Bus M0 deployment seam P-S12." `DEFAULT_FRAMES=4096` (370) is an admitted placeholder.
- **Gap:** No durable broker, real reconnect/backfill over a wire, or cross-process subscription.
- **Test-passes-on-stub?:** YES — `tests/cdc_3_5_firehose_resume_cursor.rs`, `tests/drills_eb21_firehose_d10.rs`
  drive the in-memory `Firehose`; "reconnect" is another method call on the same object.
- **Blast radius:** HIGH — live delivery / collab / CI-log "zero ops lost on reconnect" unproven against any real
  transport.
- **Maps to:** P-522/523, P-539; E0.3.

### F-events-9: `retention.rs` ships only hardcoded "measured" constants; no retention is enforced anywhere
- **Location:** `crates/myelin-events/src/retention.rs:StreamClass::tuning` (95), literal `window_frames` /
  `p99_reconnect_gap_frames` (99–118), `RetentionTuning::window_exceeds_p99_gap` (162).
- **Claimed (per ledger/contract):** contract 3.5 (EB-30/D-10) — the retention window per class, MEASURED so the
  window exceeds the p99 reconnect gap.
- **Built (actual):** inline literals (`window = 4× gap` by construction); nothing applies them to a real stream;
  `Firehose::new` uses the unrelated `DEFAULT_FRAMES` placeholder.
- **Gap:** "MEASURED" is a comment; no drill measures a p99 gap against a real broker.
- **Test-passes-on-stub?:** YES (trivially) — `retention.rs::every_class_window_exceeds_its_measured_p99_reconnect_gap_with_headroom`
  asserts arithmetic on the literals; `tests/cdc_3_5_retention.rs` cross-checks the same numbers against
  `thresholds.toml` (a consistency check between two copies).
- **Blast radius:** MEDIUM — false confidence retention is data-sized; a too-short window → resync storms in prod.
- **Maps to:** P-522/523; E0.3 (firehose tuning).

### F-events-10: Cross-cell propagation is an in-process fan-out with a hardcoded-zero PII tripwire
- **Location:** `crates/myelin-events/src/crosscell_propagation.rs:CrossCellPropagator` (187), `fan_out` (228),
  `pii_fields_crossed` (197, `AtomicU64` never incremented; reader 271).
- **Claimed (per ledger/contract):** contract 12.6 — mint a PII-free `CrossCellPointer` carried between cells;
  0 PII crosses (CP-D8/GA-D8).
- **Built (actual):** modeled. `fan_out` returns a `Vec<PropagatedPointer>` in-process; nothing carries it across
  a cell (the control-plane `cross_cell_bridge` is the floor). `pii_fields_crossed` is never incremented (the
  module flags it as an equivalent mutant to the constant `0`).
- **Gap:** The "0 PII crosses" proof is type-shape only (the pointer has no payload field); the carrying transport
  is absent and the leak-detector is inert.
- **Test-passes-on-stub?:** YES — `crosscell_propagation.rs::fan_out_produces_one_pointer_per_other_member_cell`
  asserts `pii_fields_crossed()==0` (a constant) and the serialized pointer lacks PII (the latter has some value).
- **Blast radius:** MEDIUM — cross-cell residency partly real (type shape) but transport absent and detector inert.
- **Maps to:** P-522/523; cross-cell/control-plane epic.

### F-events-11: There is no production assembly — no `serve`, no wiring of the real NATS bus to the outbox/relay
- **Location:** `crates/myelin-events/src/lib.rs` (no `serve`/wiring fn; only re-exports + doc-floors at 295/341/437);
  `Cargo.toml` (`myelin-config`, `tokio`, `async-nats` all `optional`, default `[]`).
- **Claimed (per ledger/contract):** a full-package durable event bus underpinning Git→Actions→chat→issues→docs.
- **Built (actual):** none at the integration layer. Every organ is a trait + in-memory floor; the real
  `NatsJetStreamBus` is constructed only in the two feature-gated tests; the composition root (`serve`, P-S12) is
  deferred.
- **Gap:** Nothing in a default build runs a durable bus end-to-end.
- **Test-passes-on-stub?:** YES (vacuously) — no test boots a real assembly.
- **Blast radius:** CRITICAL — at the system level "the event bus" does not durably persist or deliver anything in
  any default-gate build; durability is entirely deferred.
- **Maps to:** P-539, P-522/523; E0.3.

---

## D — `myelin-control-plane`
(Every store is `BTreeMap`/`Vec`; zero `sqlx`/pool in the crate; the only SQL is dead DDL string constants
lint-checked but never executed. Recurring tells: trusts-declared-input region/zookie/runner labels; status-flip
without data movement; dead "zero" tripwire counters never incremented (several self-documented as equivalent
mutants), so every `assert_eq!(counter, 0)` is tautological. F-cp-12 and F-cp-27 are genuinely real.)

### F-cp-1: The entire placement registry is an in-memory `BTreeMap`, not durable
- **Location:** `crates/myelin-control-plane/src/registry.rs:Registry` (110–127, `cells/placements/local_tenants/repo_placements:BTreeMap`,
  `provisioning_log:Vec`), `new()`/`default()` (131–133).
- **Claimed (per ledger/contract):** the registry backing three PII-free Postgres tables; "DDL executes against
  the live pool (P-ST-01/P-S12)."
- **Built (actual):** in-memory; no `sqlx`/`PgPool` in the crate (Cargo.toml: "cargo build stays DB-free").
- **Gap:** All cell inventory, tenant→cell placements, provisioning log, and repo routing vanish on process exit.
- **Test-passes-on-stub?:** YES — `registry.rs::tests::admits_a_single_region_placement` inserts+reads in one
  process; no test drops/reopens; the `BTreeMap` is the stub.
- **Blast radius:** CRITICAL — a control-plane restart loses every tenant→cell routing; `placement_of`/`discover`
  return `None` for all tenants → mass unroutability / double-placement.
- **Maps to:** P-522/523; E0.3.

### F-cp-2: `CREATE TABLE`/`CREATE TRIGGER` DDL is dead string constants — never executed
- **Location:** `crates/myelin-control-plane/src/lib.rs:control_plane_migrations()` (322–386), trigger DDL
  (368–384); runner `crates/myelin-substrate/src/migrations.rs:MigrationRunner::run` (108–141).
- **Claimed (per ledger/contract):** "DDL executes against the live pool"; the trigger installs the HARD
  placement invariant via the same predicate `check_placement_invariant` enforces.
- **Built (actual):** structural-only. `MigrationRunner::run` lexically lints the DDL (`is_destructive`,
  `is_blocking_alter`) then `applied.push(id)` — never opens a connection or executes SQL.
- **Gap:** The architecture's primary DB-level placement-invariant trigger does not exist at runtime; the only
  guard is the in-memory `check_placement_invariant`.
- **Test-passes-on-stub?:** YES — `lib.rs::tests::placement_invariant_trigger_is_installed` /
  `migrations_are_forward_only_and_pii_free` do `ddl.contains("CREATE TRIGGER")` / substring PII checks on a
  constant; semantically broken SQL stays green.
- **Blast radius:** CRITICAL — the claimed DB-enforced residency pin (§5.3 layer 2) is enforced by no database.
- **Maps to:** P-522/523, P-531.

### F-cp-3: `schema.rs` is plain Rust structs, not DB-mapped tables
- **Location:** `crates/myelin-control-plane/src/schema.rs` (1–191) — `Cell`/`TenantPlacement`/`CellProvisioning`/`LocalTenant`.
- **Claimed (per ledger/contract):** "the three PII-free control-plane registry tables."
- **Built (actual):** `#[derive(Clone,Debug,PartialEq,Eq)]` only — no `sqlx::FromRow`, no column binding; nothing
  maps these to the dead DDL, so struct/DDL drift is uncaught. (The PII-free property is real and valuable.)
- **Test-passes-on-stub?:** YES — `schema.rs::tests::registry_schema_is_opaque_only` constructs structs and reads
  fields; pure shape.
- **Blast radius:** MEDIUM.
- **Maps to:** P-522/523, P-531.

### F-cp-4: `place()` stickiness is process-lifetime, not durable
- **Location:** `crates/myelin-control-plane/src/place.rs:PlacementService::place` (236–284) →
  `registry.rs:place_tenant` (168–175, `placements.insert`); id minter `CounterMinter` (`01J0CP-{n}`).
- **Claimed (per ledger/contract):** "stored in `tenant_placement` … a placed tenant always routes to the same
  cell" — a sticky stored fact.
- **Built (actual):** the region→tier→capacity→stability assignment is real and mutation-tested; "stored fact" =
  the in-memory `BTreeMap`. After restart `placements` is empty → `answer_for` returns `None` and a re-`place`
  mints a NEW id and re-assigns.
- **Test-passes-on-stub?:** YES (for durability) — `place.rs::tests::placement_is_a_sticky_stored_fact` operates
  on one in-memory registry, never restarts; the assignment ordering is real, stickiness-across-restart is not
  exercised.
- **Blast radius:** HIGH — a crash between `place` and cell-local phase-2 signup orphans the tenant; restart =
  mass re-assignment.
- **Maps to:** P-522/523.

### F-cp-5: Provisioning gate — real restore-verify/KMS *logic* over modeled inputs; persists nothing
- **Location:** `crates/myelin-control-plane/src/provision.rs:provision_cell`/`decommission_tenant` (223–329);
  activation `registry.rs:set_cell_status` (216–231); log `registry.rs:279–281`.
- **Claimed (per ledger/contract):** CP-D6 — no cell goes `Active` until it passes restore-verify (Storage 11.5)
  + readiness; each step recorded.
- **Built (actual):** partial. Genuinely drives `myelin_storage::RestoreVerifyGate::run` and reads a typed
  `GateVerdict`; `decommission_tenant` genuinely calls `KmsEngine::destroy_kek`. But `GateInputs` are hand-built
  fixtures (no real `pg_restore`), and the `Active` flip + log are in-memory.
- **Test-passes-on-stub?:** PARTIAL — `cp_d6_provisioning_gate_drill.rs::cp_d6_no_traffic_to_an_unverified_cell`
  + in-file `failing_restore_verify_keeps_the_cell_provisioning` would FAIL an always-activate stub (gating is
  behaviorally tested); durability/crash-safety is never tested.
- **Blast radius:** HIGH — gate logic sound in-process; activation/log don't survive restart.
- **Maps to:** P-522/523.

### F-cp-6: Gateway misroute-rejection is real, but the audit "evidence" is a volatile `Arc<Mutex<Vec>>` and `cross_tenant_reads` is a dead counter
- **Location:** `crates/myelin-control-plane/src/placement_of.rs:CellGateway::route` (358–405); audit
  `MisrouteAudit = Arc<Mutex<Vec<..>>>` (222–260); `cross_tenant_reads` (283–335, never incremented).
- **Claimed (per ledger/contract):** "a misroute is REJECTED + REDIRECTED + AUDITED … the audit IS the evidence."
- **Built (actual):** partial. Accept/reject/redirect + fail-closed-on-unknown-tenant is real and mutation-tested;
  the audit lives in an in-process `Mutex<Vec>` (lost on restart, not tamper-evident); `cross_tenant_reads` is a
  hardcoded-0 tripwire.
- **Test-passes-on-stub?:** Decision NO (a foreign-tenant-accepting stub fails
  `cp_d2_misroute_rejection_drill.rs`); audit durability YES (memory `Vec` passes; no restart/tamper test).
- **Blast radius:** HIGH — decision sound; the compliance audit trail and the routing it depends on (F-cp-1) are
  volatile.
- **Maps to:** P-531 (decision) + P-522/523 (durable audit + routing).

### F-cp-7: `discover()` "JOIN" is two in-memory map lookups + an O(n) slug scan; fail-static cache is real but caches volatile routes
- **Location:** `crates/myelin-control-plane/src/discover.rs:Registry::discover` (138–156), `DiscoveryCache`
  (172–312); slug scan `registry.rs:268–270`.
- **Claimed (per ledger/contract):** reads `tenant_placement` JOINed to `cell`; indexes `slug`.
- **Built (actual):** partial. "JOIN" = `BTreeMap` get + get; slug = linear scan (no index). The `DiscoveryCache`
  fail-static (fresh/degraded-static/fail-closed) is real and clock-driven.
- **Test-passes-on-stub?:** Fail-static NO (real — `cache_serves_fail_static_when_control_plane_unreachable`
  drives a `TestClock`); durability/JOIN YES.
- **Blast radius:** MEDIUM.
- **Maps to:** P-522/523; fail-static logic real.

### F-cp-8: Repo placement is an in-memory map; "never node-pinned / no hash recompute" is trivially true
- **Location:** `crates/myelin-control-plane/src/placement_of_repo.rs:register_repo`/`relocate_repo`/`placement_of_repo`
  (233–326) → `registry.rs:126`; `route_repo` (379–412).
- **Claimed (per ledger/contract):** repo-granular, region-pinned + relocatable, never node-pinned; a stored fact
  relocatable without a hash recompute.
- **Built (actual):** structural/in-memory. `register_repo` inserts a row; `relocate_repo` overwrites it. The
  residency pin (`assert_cell_in_region`) reuses the real invariant. No `repo_placement` table even in the dead
  DDL; "no hash recompute" is cheap because nothing was ever hashed; the byte-move is deferred.
- **Test-passes-on-stub?:** Routing/residency NO (`git_residency_leg_repo_grain_drill.rs`,
  `cdc_12_2_repo_grain_git_wire.rs` real); durability + actual relocation YES
  (`repo_relocation_does_not_recompute_a_hash` only checks the stored `cell_id` flipped).
- **Blast radius:** HIGH — repo clone URLs unroutable after restart; relocation flips a pointer without moving
  bytes → 404 at the new cell.
- **Maps to:** P-522/523 + P-CP-22; residency-pin → P-531.

### F-cp-9: Region-immutability "layer 1" is enforced only by the absence of a setter, with `pub region` fields freely settable
- **Location:** `crates/myelin-control-plane/src/registry.rs` (21–27 doc; absent `update_cell_region`);
  `Cell.region`/`TenantPlacement.region` are `pub` (`schema.rs:83,118`); test `region_has_no_update_path` (446–471).
- **Claimed (per ledger/contract):** "no `update_*_region` … structurally read-only."
- **Built (actual):** structural-only by omission. The fields are `pub` — anyone with `&mut Cell` or a new row can
  change region and re-insert; the DB trigger that would enforce it is dead (F-cp-2).
- **Test-passes-on-stub?:** YES — `region_has_no_update_path` relies on a commented-out call to a non-existent
  method; passes against any impl lacking the method.
- **Blast radius:** MEDIUM — silent residency move via direct struct construction, no DB constraint.
- **Maps to:** P-531 / P-522/523.

### F-cp-10: `partition_key` tier-invariance is a struct-shape tautology (no RLS)
- **Location:** `crates/myelin-control-plane/src/isolation.rs:PartitionKey::for_tier` (180–183, ignores `_tier`),
  `partition_key` (190–192), `PoolStore::open`/`rls_tenant`/`pinned_region` (216–246).
- **Claimed (per ledger/contract):** a store opens at Pool tier with the identical `(tenant,region)` key; the RLS
  predicate filters on `PartitionKey::tenant` (contract 12.5).
- **Built (actual):** structural-only. `for_tier` discards the tier arg; `rls_tenant()` is a getter; no RLS/store/DB
  (doc: "RLS predicate is Storage-owned, P-ST-01").
- **Test-passes-on-stub?:** YES — `isolation.rs::partition_key_is_identical_at_every_tier` (328),
  `pool_store_opens_with_the_tier_invariant_partition_key` (349); the impl is the stub.
- **Blast radius:** LOW (no false "blocking" claim) — but the doc overstates "RLS enforced."
- **Maps to:** P-531 (real RLS in Storage; F-storage-2/F-tenancy-3).

### F-cp-11: `residency_verify` trusts a declared `region` label — never probes where data lives
- **Location:** `crates/myelin-control-plane/src/residency_verify.rs:residency_verify_over` (469–517, compare at
  481); `StoreRegionReport` (215–232).
- **Claimed (per ledger/contract):** `residency_verify(tenant) → SignedAttestation{ every_store_region == tenant.region }`
  — the no-global-pool/EU-sovereignty attestation (mandatory-core, claimed 100% mutation).
- **Built (actual):** modeled aggregation over trusted inputs. Compares a caller-supplied `Region` value in
  `StoreRegionReport` to the tenant's region. The aggregate/sign/fail-closed logic is real; the truth source is
  not probed. Doc concedes the report is "a VALUE the store layer feeds in."
- **Gap:** Catches an honestly self-reporting mis-regioned store; cannot catch a store that writes out-of-region
  while reporting in-region. Real residency truth deferred to Storage P-ST-07 / STOR-D5.
- **Test-passes-on-stub?:** RED legs of `cp_d3_residency_verify_drill.rs::cp_d3_residency_verify_m1_store_set`
  fail an always-allow stub (non-vacuous), but cannot distinguish "verified real residency" from "compared two
  strings the test supplied."
- **Blast radius:** HIGH — the headline EU-sovereignty attestation is only as truthful as the unenforced store
  self-report.
- **Maps to:** P-531 + Storage P-ST-07.

### F-cp-12 (HONEST/REAL): `SignedAttestation` keyed-BLAKE3 MAC is genuine crypto
- **Location:** `crates/myelin-control-plane/src/residency_verify.rs:ResidencySigningKey::mac` (320–323),
  `canonical_body` (368–391), `verify` (397–407).
- **Built (actual):** real `blake3::keyed_hash` over a canonical PII-free body; tamper/forgery/wrong-key fail
  verify. The one real cryptographic enforcement in the isolation cluster. Only gap: key is in-process `[u8;32]`,
  not KMS-sourced.
- **Test-passes-on-stub?:** NO — `a_tampered_attestation_fails_verification` (685) fails a `verify→true` stub.
- **Blast radius:** LOW.
- **Maps to:** Storage P-ST-04 (key provenance).

### F-cp-13: `four_layer` write boundary is `Region==Region`; `out_of_region_writes_admitted` is a dead counter (vacuous "0")
- **Location:** `crates/myelin-control-plane/src/four_layer.rs:check_write` (166–179); counter field (124–127),
  getter (153–155); `assert_no_cross_region_query_path` (269–316).
- **Claimed (per ledger/contract):** the runtime leg that REJECTS a `row.region ≠ cell.region` write; the counter
  is "a live counter (not a constant)" (mandatory-core, 91.7% mutation).
- **Built (actual):** structural-only. `check_write` compares a harness-injected `Region`; no store behind it
  (real twin = deferred Postgres RLS `WITH CHECK`, STOR-D5). The `AtomicU64` is `fetch_add`-ed nowhere (module
  admits a documented EQUIVALENT mutant `replace → 0`).
- **Test-passes-on-stub?:** YES — `four_layer_e2e_drill.rs::four_layer_region_pinning_end_to_end` asserts `==0`,
  true for any impl.
- **Blast radius:** MEDIUM — false impression of a runtime egress tripwire.
- **Maps to:** P-531 + Storage RLS twin (P-522/523).

### F-cp-14: `CellFleet` bulkhead — in-memory counters + `bool` fault; cross-cell impact is 0 by loop construction; the "RED" model is a hardcoded formula
- **Location:** `crates/myelin-control-plane/src/bulkhead.rs:CellBulkhead.offer` (215–238), `inject_fatal_fault`
  (193–195), `CellFleet.run_surge` (360–415), `cross_cell_impact` (424–437), `shared_queue_impact` (447–461).
- **Claimed (per ledger/contract):** a fault in one cell leaves others unaffected; cross-cell impact 0 by
  construction; a RED model would show non-zero (mandatory-core, claimed 100%).
- **Built (actual):** modeled. Cell = two `BoundedQueue` counters + `faulted:bool`; fleet = `BTreeMap`.
  `run_surge` only touches the target entry, so impact is structurally 0. `shared_queue_impact` (the "RED" leg) is
  `if surge_requests > shared_capacity {other_cells} else {0}` — a formula never exercising the GREEN path.
- **Test-passes-on-stub?:** YES — `cp_d5_cell_bulkhead_surge_drill.rs::cp_d5_..._cross_cell_impact_zero` passes for
  any model touching one map key.
- **Blast radius:** MEDIUM — proves an arithmetic identity, not real fault isolation.
- **Maps to:** P-531 / CP-D5.

### F-cp-15: `ControlPlane` outage is an `AtomicBool`; degrade-not-cascade is one path checking a bool
- **Location:** `crates/myelin-control-plane/src/cp_outage.rs:ControlPlane.hard_down`/`is_down` (171–183),
  `DataPlane.serve` (321–360), `SignupPlane.signup` (413–454), `CpOutageReport::compute` (492–520).
- **Claimed (per ledger/contract):** hard-down the CP → placed tenants keep serving in-cell; only signup degrades;
  serving-uptime 100% (mandatory-core, 90.9% mutation).
- **Built (actual):** modeled. "Outage" = `down.store(true)`; `serve` resolves from the in-process cache+registry
  (same process); `compute` is integer arithmetic. No real CP transport/partition.
- **Test-passes-on-stub?:** YES — `cp_d4_blast_radius_drill.rs::cp_d4_..._placed_tenants_keep_serving`; a
  bool-flipping model passes.
- **Blast radius:** MEDIUM.
- **Maps to:** P-531; live transport deferred (~P-522/523).

### F-cp-16: `mirror_allowed` decides on declared `region` fields and counts denials issued, not pushes blocked — advisory gate
- **Location:** `crates/myelin-control-plane/src/mirror_allowed.rs:mirror_allowed` (250–297), same-region branch
  (269), `unauthorised_pushes_prevented += 1` (289).
- **Claimed (per ledger/contract):** deny-by-default, 0 unauthorised cross-residency mirror pushes — the C-4
  load-bearing zero (claimed 100%).
- **Built (actual):** partial. The decision logic (same-region allow / crossing→`TransferPolicy` / deny-by-default
  / unknown-tenant fail-closed) is real. But (a) `MirrorTarget.region` is a trusted caller input, not resolved
  from the real host; (b) `unauthorised_pushes_prevented` counts `Deny` verdicts, not blocked egress — nothing
  stops a caller that ignores `Deny`; (c) the real GDPR `TransferGate` is wired only in the dev CDC test, not the
  production DAG.
- **Test-passes-on-stub?:** Decision NO (`extra_eu_target_denied_by_default`, `c4_mirror_gate_drill.rs` fail an
  always-Allow stub); enforcement YES — no test exercises an actual push being blocked.
- **Blast radius:** HIGH — the residency egress gate is advisory; correctness depends on an unverified caller
  contract + trusted host→region input.
- **Maps to:** P-531 + contract 10.5; real enforcement deferred to `myelin-git`.

### F-cp-17: `DependencyBreaker` / `Scope::Cell` fault injector is decorative in the D4/D5 drills
- **Location:** `tests/cp_d4_blast_radius_drill.rs` (100–119, 179–193),
  `tests/cp_d5_cell_bulkhead_surge_drill.rs` (79–94, 195–211).
- **Claimed (per ledger/contract):** forces the outage with the scoped-reversible dependency-break injector (the
  T-3 seam) the same way every later drill rides.
- **Built (actual):** structural. The SUT never consults the breaker — D4: `if breaker.is_broken(..){cp.hard_down()}`
  (`serve` reads the bool, not the breaker); D5: the breaker marks a cell broken but `run_surge`/`inject_fatal_fault`
  operate on the `BTreeMap` directly. Deleting the breaker leaves all isolation assertions unchanged.
- **Test-passes-on-stub?:** YES — the breaker assertions test the breaker's own ledger, not isolation.
- **Blast radius:** LOW–MEDIUM — inflates apparent fault-injection rigor.
- **Maps to:** testing-strategy T-3.

### F-cp-18: Live migration "moves" zero bytes — it `clone()`s an in-memory struct the caller already holds
- **Location:** `crates/myelin-control-plane/src/migration.rs:LiveMigration::migrate_tenant` (351–455); the "copy"
  at 408–409 (`target.source = source.source.clone()`); `CellTenantCopy` (249–260).
- **Claimed (per ledger/contract):** online cell→cell move: copy SoT to target, reindex, atomic cut-over,
  crypto-shred source, 0 loss across-seam (CP-D7, M5/P-431).
- **Built (actual):** modeled / in-memory. `source` and `target` are two local structs in the same process (no
  second cell, no transport, no destination address). The "move" is `clone()` + a registry-row edit;
  `restore_to_offset` verification is real logic over in-memory vecs.
- **Test-passes-on-stub?:** PARTIAL — `cp_d7_live_migration_drill.rs` +
  `migrate_tenant_zero_loss_in_region_source_shredded` assert the placement cut over and the in-memory KEK was
  removed (a canned-`Ok` stub fails); but one process cannot distinguish "moved between cells" from "cloned a
  local struct."
- **Blast radius:** HIGH — the headline CP-D7 "0-loss data move" is structurally unbuilt.
- **Maps to:** P-522/523, M5 CP-D7.

### F-cp-19: The "durable workflow" is decorative — migration steps run synchronously outside the run; resumability never exercised
- **Location:** `crates/myelin-control-plane/src/migration.rs:migrate_tenant` (`executor.start(...)` at 386; steps
  2–4 run as plain sync Rust at 398–443); `FlowExecutor` = in-memory `Arc<Mutex<HashMap>>`
  (`crates/myelin-flow/src/executor.rs:252`).
- **Claimed (per ledger/contract):** runs as a `DurableExecutor` run (contract 9.1) — crash-safe + resumable +
  idempotent; resumes from its cursor instead of half-moving a tenant.
- **Built (actual):** structural-only. `start()` records an in-memory run row; reindex/cut-over/shred then execute
  synchronously, not as journaled effects. No resume/replay cursor.
- **Test-passes-on-stub?:** YES — `migration_run_is_idempotent_on_idem_key` does NOT call `migrate_tenant` twice;
  it re-calls `executor.start(...)` with the same key and checks run_ids match — proving only the executor's
  idempotency. A constant-`RunId` stub passes.
- **Blast radius:** MEDIUM — a crash mid-migration can leave a half-moved tenant despite the "never half-moves"
  claim.
- **Maps to:** P-522/523, contract 9.1.

### F-cp-20: Migration "0-loss numbers" are structurally fixed, not measured from the target
- **Location:** `crates/myelin-control-plane/src/migration.rs` (410–453): `rows_migrated` derived from `source`
  (410–412); `cross_seam_mismatches = report.dangling_ref_count` (452, necessarily 0 because dangling refs already
  errored at 406).
- **Built (actual):** modeled — the receipt's "measured" evidence never inspects the target; both numbers are
  deterministic functions of the fixture source.
- **Test-passes-on-stub?:** YES — `migrate_tenant_zero_loss_..._source_shredded` asserts `rows_migrated==2`,
  `cross_seam_mismatches==0`; a source-derived stub passes identically.
- **Blast radius:** MEDIUM — a real target silently dropping rows still receipts "0 loss."
- **Maps to:** P-522/523, CP-D7.

### F-cp-21: `relocate_repo_durably` claims copy→reindex→shred but only flips a registry row
- **Location:** `crates/myelin-control-plane/src/migration.rs:relocate_repo_durably` (498–529).
- **Claimed (per ledger/contract):** the same copy→reindex→cut-over→shred-source mechanism at repo grain.
- **Built (actual):** partial — body is `executor.start(...)` (in-memory run) + `registry.relocate_repo(...)`
  (in-memory `BTreeMap` edit). No `restore_to_offset`, reindex, `destroy_kek`, or byte copy.
- **Test-passes-on-stub?:** YES — `durable_repo_relocation_updates_placement_and_redirects` only asserts the
  placement flipped + run id non-empty + cross-region rejected; that stub is the impl.
- **Blast radius:** MEDIUM — repo bytes not relocated/shredded; old cell retains them despite "shred-source."
- **Maps to:** P-522/523, §5.2 C-1.

### F-cp-22: Crypto-shred is an in-memory `BTreeMap::remove`, not real key destruction
- **Location:** `crates/myelin-control-plane/src/migration.rs:441–443` →
  `myelin-storage/src/kms.rs:KmsEngine::destroy_kek` (664); `KmsEngine` = `Mutex<BTreeMap<KekId,StoredKek>>` (470).
- **Claimed (per ledger/contract):** destroy source-cell key material → forever unreadable (live AND in every
  source backup) (Storage 11.3).
- **Built (actual):** in-memory. `destroy_kek` = `keks.remove(id).is_some()`. No HSM, no backup interaction
  (kms.rs:678–684 concedes the HSM/backup behavior is the hardening follow-on; see F-storage-7).
- **Test-passes-on-stub?:** NO for in-memory behavior (`resolve_dek` returns `Err` after; a no-op stub fails) —
  but it only proves map semantics, not crypto-shred.
- **Blast radius:** HIGH — the GDPR erasure guarantee rests on key material in a process-local `BTreeMap`.
- **Maps to:** P-522/523, Storage 11.3.

### F-cp-23: Cross-cell bridge resolves against an in-memory resolver registry with no reachability probe; `cross_cell_raw_rows` is a dead counter
- **Location:** `crates/myelin-control-plane/src/cross_cell_bridge.rs:CrossCellBridge::resolve` (326–349),
  `CellResolverRegistry = HashMap<CellId, Arc<dyn CellLocalResolver>>` (243–265); `cross_cell_raw_rows` (291,
  383–395, never `fetch_add`).
- **Claimed (per ledger/contract):** dispatches a cross-cell resolve to the home cell over the resilient client
  (contract 12.6 "LIVE"); the counter ticks above 0 on a raw-row regression.
- **Built (actual):** structural dispatch via in-process trait objects; `resolve` calls `resolver.resolve_in_cell(...)`
  directly — no network, no reachability check; an unregistered home cell silently → `Gone` tombstone (344). The
  counter is initialized 0 with no writer (self-documented MISSED/EQUIVALENT mutant, 64–71).
- **Test-passes-on-stub?:** YES — every test (`cp_d8_cross_cell_bridge_drill.rs`, `cdc_12_6_bridge_resolution_live.rs`)
  supplies its own stub resolver; the bridge under test is shape-only; `assert_eq!(cross_cell_raw_rows(),0)` is
  tautological.
- **Blast radius:** MEDIUM — unreachable vs unregistered conflation hides outages as `Gone`; "0 PII" proof is
  structural.
- **Maps to:** P-531 + P-CP-19 transport floor.

### F-cp-24: Runner-claim region pin trusts a declared `runner_region` value — no allowlist, no attestation, no backend
- **Location:** `crates/myelin-control-plane/src/runner_claim_pin.rs:RunnerClaimPin::admit_claim` (185–200) —
  `if *runner_region == self.tenant_region {Ok} else {Err}`; `out_of_region_claims_admitted` never incremented.
- **Claimed (per ledger/contract):** an EU-resident tenant's CI run is claimed only by an in-region runner — the
  authoritative region-pin (§5.4, CI-R3).
- **Built (actual):** pure value comparison. `runner_region` is caller-supplied; no allowlist, no attestation, no
  DB/lease lookup; `tenant_region` is also a constructor arg, not read from `tenant_placement`.
- **Test-passes-on-stub?:** YES — `ci_r3_runner_claim_drill.rs` +
  `admit_claim_admits_in_region_rejects_out_of_region` feed literal regions; a one-line `==` is the whole impl.
- **Blast radius:** HIGH — if upstream passes the tenant's region as the runner's (or trusts a spoofable
  self-report), an out-of-region runner is admitted with this assertion green.
- **Maps to:** P-531, CI-R3 / §5.4.

### F-cp-25: Cross-cell zookie consistency is arithmetic over two caller-supplied timestamps
- **Location:** `crates/myelin-control-plane/src/multi_cell.rs:CrossCellZookieReader::read_through` (320–341) —
  `home_minted_at_secs.saturating_sub(member_observed_at_secs) <= 300`.
- **Claimed (per ledger/contract):** the hardest sub-problem — a zookie read in a member cell observes bounded
  staleness (Zanzibar-class), never a stale-read past the bound.
- **Built (actual):** pure function. Both timestamps are caller-supplied `u64`s; no snapshot read, member clock, or
  replication-lag measurement; the `Zookie` is cloned through, never inspected.
- **Test-passes-on-stub?:** YES — `ga_d8_cross_cell_dsr_fanout_drill.rs` +
  `zookie_within_budget_is_admitted_bounded_stale`/`zookie_past_bound_is_refused` feed literal `(1000,940)`.
- **Blast radius:** MEDIUM — "hardest sub-problem" reduces to threshold arithmetic; a real cross-cell read could
  serve stale grants.
- **Maps to:** P-531, P-CP-20.

### F-cp-26: Cross-cell DSR fan-out & multi-cell rebalance operate on in-memory eraser maps / placement rows — no data touched
- **Location:** `crates/myelin-control-plane/src/multi_cell.rs:CrossCellDsrFanOut::fan_out` (201–233),
  `add_member_cell`/`rebalance_member_cell` (373–446).
- **Claimed (per ledger/contract):** DSR asks each member cell to erase the subject in that cell over the bridge
  transport; rebalance moves a tenant's workload (contract 10.4, GA-D8).
- **Built (actual):** in-memory. `fan_out` iterates a `BTreeMap<CellId, Arc<dyn CellLocalEraser>>` of in-process
  stubs (no transport); no actual store erasure (per-cell shred is the deferred Identity leg). `rebalance` edits
  the `member_cells: Vec`/`placements: BTreeMap` — moves nothing physical.
- **Test-passes-on-stub?:** YES (fan-out) — `ga_d8_..._drill.rs` + `cdc_10_4_cross_cell_dsr_fanout.rs` register
  stub erasers; completeness proves iteration/dedup, not erasure. PARTIAL (rebalance) — region invariant fires.
- **Blast radius:** HIGH (DSR) — a "complete, 0 cells missed" GDPR erasure receipt is minted while no real store
  was touched; MEDIUM (rebalance).
- **Maps to:** P-522/523, P-531, P-CP-20 / GA-D8 / contract 10.4.

### F-cp-27 (mostly HONEST): Self-host "bootstrap" is an in-memory one-row registry — but parity is genuinely tested
- **Location:** `crates/myelin-control-plane/src/self_host.rs:DegenerateControlPlane::bootstrap`/`with_endpoint`
  (102–301).
- **Built (actual):** thin in-memory assembly that delegates all methods to the shared
  `Registry`/`PlacementService`/`CellGateway`/`residency_verify` — the parity (no fork) is real and correct by
  design; but "stand up a control plane" = constructing a struct (no process/listener/persistence).
- **Test-passes-on-stub?:** NO (parity real) — `self_host_parity_drill.rs` / `cp_d23_dogfood_self_host_drill.rs`
  exercise the shared API end-to-end and would catch a fork. Limitation: "real operations" = in-memory registry ops.
- **Blast radius:** LOW — by-design degenerate config; harm only if read as production-deployment readiness.
- **Maps to:** roadmap P-CP-13/P-CP-23.

---

## E — `myelin-tenancy`

### F-tenancy-1: The "tenant-isolation primitive" is value types only — zero enforcement, zero state, zero RLS
- **Location:** `crates/myelin-tenancy/src/lib.rs` (entire file, 1–695) — `TenantId` (111–132), `Region` (148–165),
  `ResidencyTag` (180–197), `CrossCellPointer` (385–441).
- **Claimed (per ledger/contract):** the DAG-root tenant/region/residency substrate — the `(tenant,region)`
  partition key injected by the harness, identical at every isolation tier, the seam every store threads for
  isolation (contract 12.1/12.5, ADR-11).
- **Built (actual):** structural-only. The crate is `#[derive(Serialize/Deserialize)]` newtypes over `String` plus
  a four-field `CrossCellPointer`. No tenant-context propagation, no RLS, no session-scoping, no tenant guard, no
  connection handling, no state. Only runtime dep is `serde`.
- **Gap:** The crate names the partition key but enforces nothing with it; enforcement is entirely deferred to
  consumers (control-plane — in-memory, §D; storage RLS — F-tenancy-3).
- **Test-passes-on-stub?:** YES — `lib.rs::tests::surface_partition_key_types_exist`,
  `cdc_12_1_store_handle_parameterised_by_tenant_region` (516), `cross_cell_pointer_round_trips_its_four_fields`
  (574) are type-construction/serde-shape/compile-fixture assertions. The `StoreHandle` "consumer that refuses
  cross-tenant reads" is a test-local `HashMap` struct defined inside the test (522–543).
- **Blast radius:** MEDIUM (as a crate) — the type discipline (opaque token, no `From<String>`, no `Region`
  setter) is genuinely good and PII-safe; the harm is the illusion that "tenancy" is an isolation organ when it is
  a glue-type vocabulary.
- **Maps to:** P-531 (the actual enforcement is unbuilt here by design).

### F-tenancy-2: `CrossCellPointer` is explicitly FROZEN-NOT-LIVE — the bridge type with no resolution path
- **Location:** `crates/myelin-tenancy/src/lib.rs:CrossCellPointer` (385–441); no `resolve()` by design
  (doc 43–51, 331–341).
- **Claimed (per ledger):** the four-field PII-free cross-cell bridge frame ISS/KN/CHAT compile against (contract
  12.6, §6.1).
- **Built (actual):** modeled / type-only, and honestly labelled so; resolution is deferred to P-CP-19 (the
  control-plane side that implements `resolve` is itself in-memory — F-cp-23).
- **Test-passes-on-stub?:** YES — `cross_cell_pointer_round_trips_its_four_fields`,
  `cdc_12_6_consumer_constructs_frame_and_sees_only_four_fields` (632): serde key-set + accessor shape only.
- **Blast radius:** LOW — honest, named floor.
- **Maps to:** P-531 / P-CP-19.

### F-tenancy-3: The real RLS enforcement (in `myelin-storage`) uses session-scoped `set_config(...,false)` on pooled connections with NO reset-on-release — pooled-connection tenant bleed
- **Location:** `crates/myelin-storage/src/pg.rs:set_session_scope_in_region` (405–420), GUC at **413**
  (`set_config('myelin.tenant_id',$1,false)` — 3rd arg `false` = SESSION scope, not `SET LOCAL`); pool acquire
  sites 210/253/286/326/380/433/455; RLS policy `pg.rs:183–190` uses `current_setting('myelin.tenant_id', true)`.
- **Claimed (per ledger):** every session sets `myelin.tenant_id`/`myelin.region`; the IDOR floor — a session for
  tenant A can only read tenant A's rows — via `FORCE ROW LEVEL SECURITY`.
- **Built (actual):** real RLS, but session-scoped on a pooled connection with no reset. `conn` comes from
  `self.pool.acquire()`; the GUCs are set with `false` (session lifetime); there is **no
  `RESET`/`DISCARD ALL`/`after_release`/`Drop`** (grep confirmed none). On return to the pool the connection
  retains the last tenant's GUC.
- **Gap:** Exactly the P-531 pooled-connection-bleed. Today every public method re-runs `set_session_scope` after
  `acquire()` (in-crate defended; `WHERE tenant_id=$1` is defence-in-depth), BUT (a) the GUC is never cleared on
  release, so any bare-acquire path (or a query before re-scoping) inherits the previous tenant; (b) `SET LOCAL` /
  `set_config(...,true)` (transaction-scoped, auto-reset) is the correct primitive and is not used;
  (c) `current_setting(...,true)` returns NULL when unset → fails quiet (0 rows), not loud. (Same defect as
  F-storage-2.)
- **Test-passes-on-stub?:** N/A to a stub, but: the isolation drills always set the scope on the same connection
  they then query, so the reused/un-reset pooled-connection vector is structurally untested.
- **Blast radius:** CRITICAL — cross-tenant read/write leak if any path acquires a pooled connection without
  immediately re-scoping; this is the platform's actual tenant-isolation backstop. Fix: `SET LOCAL` /
  `set_config(...,true)` inside a transaction, or a pool `after_release` running `DISCARD ALL`.
- **Maps to:** P-531; E0.5.

---

## What I did NOT have time to inspect deeply (coverage boundary)

- **Identity:** I read the crypto organs (`authenticate.rs`, `machine_auth.rs`, `mint.rs`), the three store
  backings (`principal_store.rs`, `tuple_store.rs`, `revocation.rs`), the prod assembly (`lib.rs:identity_app_spec`
  / `StoreBackedCheck`), and `main.rs` — but **not** line-by-line: `check_engine.rs`, `delegation.rs`, `expand.rs`,
  `list_objects.rs`/`lowering.rs`, `namespace.rs`, `reverse_index.rs`, `read_replica.rs`, `pseudonym_store.rs`/
  `pseudonym_erase.rs`, the subsystem fragments (`git/knowledge/issue/chat/ci_fragment.rs`), `multi_cell.rs`,
  `failstatic_cache.rs`. The check/delegation algebra is plausibly real logic over the in-memory tuple store;
  expect the same "real logic, in-memory backing" shape. **`StructuralAttestationVerifier` (P-527, the
  self-hosted-runner TPM attestation) was named in the ledger as living in a `self_hosted.rs` that is NOT in these
  five crates** (likely `myelin-ci-*`) — flag it for the CI-track census.
- **Storage:** the agent confirmed the crypto core is real and the persistence/backup/restore/RLS organs are
  modeled; NOT deeply read: `reerase.rs`, `residency.rs` (120–713), `pg_migrator.rs`/`pgrelay.rs`/`migration.rs`
  (real sqlx, integration-gated), `coloc.rs`, `holder*.rs`, `multi_cell_erase.rs`, `cell_migration.rs`,
  `migration_under_load.rs`, `olap*.rs`, `ci_log_index.rs`, `firehose_archive.rs`, `reserve_settle.rs`,
  `gitpack.rs`/`object_packs.rs`/`replicated_blob.rs`, `cdn.rs`/`mirror.rs`, `cache.rs`. Expect the same
  modeled-on-this-floor pattern.
- **Events:** read the durability organs (outbox/dedup/relay/consumer/nats/holder/reerase/firehose/retention/
  crosscell); NOT deeply read: `check_seam.rs`, `crosscell.rs`, `reindex.rs`, `telemetry.rs`, `taxonomy.rs`,
  `upcast.rs`, `envelope.rs`, `harness.rs` (test-support), and the long tail of `drills_*`/`cdc_*` (but every one
  sampled drives the in-memory floor types, so the conclusion generalizes).
- **Control-plane:** read the registry/placement/provisioning/isolation/residency/bridge/migration/runner-claim/
  multi-cell clusters; NOT read: `holder.rs` (280 lines), `dogfood.rs` (526 lines), `discover.rs` cache lifecycle
  in full, `cdc_12_4`/`ci_r3_residency_verify_ci` (skimmed, mirror F-cp-11).
- **Cross-cutting, not done here:** no workspace-wide search for a `serve()`/bin target that wires a *real*
  Postgres/NATS/KMS pool into any of these crates (both control-plane and events assert a DB-free default build —
  the real composition root P-S12/P-S15 appears not to exist yet, which is itself the headline of the roadmap);
  no `cargo test`/`--features integration` run (read-only census, no live PG/NATS/KVM available); no mutation-score
  re-derivation. The absence-scanners P-523/P-528 (E0.2) that should fail the build on a `Structural*`/in-memory
  store in the prod graph **do not yet exist** — building them is the first dependency, because today nothing
  mechanically stops the dangerous defaults from shipping.
