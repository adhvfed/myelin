# MR-NNN Spine — Orchestration Log

Orchestrator: Claude (Opus 4.8). Started 2026-06-26. Source ledger: `09-spine-prompt-ledger.md`.

This log records, per prompt: builder verdict, the independent verification, the cargo gate result,
and the commit. It is the orchestrator's running memory across the batch. The cardinal rule of this
project — *the agent that wrote a floor cannot certify it* — is enforced here: every load-bearing or
security-critical prompt gets an independent verifier agent that never touched the code.

## PROCESS CORRECTION (after MR-007, 2026-06-26)

The gate between prompts MUST include the **full** `cargo test -p myelin-lints` (its `workspace_clean` +
`ci_gate` tests scan ALL `crates/*/src`), not just `--test production_graph_absence` or a per-crate test.
MR-005 and MR-022 each introduced a real architecture-lint violation (`make-it-real-scorecard.rs`
no-host-exec; `tenant_tx.rs` residency-pin) that the narrow tests I ran did not surface — caught only when
MR-007's builder ran the full suite. Both fixed in commit (post-MR-022): added the scorecard runner to the
no-host-exec exclusion (same CI-orchestration class as m3–m6, not a weakening) and gave `connect_pool_with_reset`
a region pin (application_name `myelin:<region>`) + the `@residency-cell-pinned:file` waiver matching `pg.rs`/
`oltp.rs`. **Gate from now on:** `cargo check --workspace --all-targets` + `cargo test -p myelin-lints` (full) +
the touched crates' tests + the relevant `--features integration` proof. Also: `cargo check` (which I used for
the initial baseline) does NOT run tests — the lints suite was already red on main before MR-004 for a different
reason; always use `cargo test` for the gate.

## Quality bar (the gate between every prompt)

1. **Anti-duplication first.** Each prompt opens with grep of `planning/07-prompts/` + crates + design
   specs, AND a ledger-vs-commits cross-check (`git log --grep`, `git show --stat`). Extend, never fork.
2. **Cargo gate.** `cargo check --workspace --all-targets` green before and after; `cargo test` for the
   touched crates green. Halt on red.
3. **Independent verification.** For HARDEN / security / persistence prompts, a separate agent (which did
   not write the code) re-derives the claim against the real artifact — runs the negative corpus, the
   crash/restart, the scanner-on-red-fixture — and reports PASS/FAIL with evidence. Builder's green is
   not accepted on its own word.
4. **Evidence, not assertion.** "It works" must be backed by a command + its output. Red fixtures must be
   shown to actually bite. No green-lying.
5. **Commit per prompt** (`MR-NNN: <title>`), so the next prompt's ledger-vs-commits cross-check works.

## Status

| MR | Title | Builder | Verify | Gate | Commit |
|----|-------|---------|--------|------|--------|
| baseline | `cargo check --workspace --all-targets` | — | — | GREEN | b4b7799 |
| MR-001 | Census: substrate | 57 findings (~12 CRIT) | orch spot-check: symbols/lines verified | n/a (read-only) | (this) |
| MR-002 | Census: Git + sandbox seam | 10 findings (5 CRIT) | orch spot-check: firecracker/gvisor/RefStore verified | n/a (read-only) | 2e2b6b1 |
| MR-003 | Census synthesis → shortcut-inventory.md | 66 deduped (17 CRIT) | orch verified SI-010 + SI-006 against source | n/a (read-only) | b00d536 |
| MR-004 | Production-graph absence scanners | 3 scanners + 23-entry 2-way ratchet, 153 tests | INDEP verifier: ACCEPT-w/-followups → 4 false-negs found & closed; orch re-checked sites + gate | GREEN (cargo test -p myelin-lints; check workspace) | 0e5a289 |
| MR-005 | Attested scorecards + red-by-default gate | blake3-attested manifest + make-it-real gate (exit 1, red-by-default), 8 tamper tests | INDEP verifier: ACCEPT-w/-followups → gate NOT gameable (no trust-manifest path; live re-run mandatory); found PRE-EXISTING vacuous-green rows | GREEN (cargo test -p myelin-harness; gate exits 1; check workspace) | 1bd35a8 |
| MR-006 | Shape/design review | 4 seams SHAPE-OK, 2 RESHAPE (001 sandbox/off-spine, 002 tenant-tx-conn/on-spine→MR-022) | orch verified seam injectors + SandboxHandle + AgentRuntime against source | n/a (read-only) | (committed w/ log) |
| MR-022 | Persistence foundation (migrations + provider + tenant-tx convention, RESHAPE-002) | apply_validated + SubstrateProvider + with_tenant_tx, 3 live-PG integration tests | INDEP verifier (live PG, app role): ACCEPT — real force-RLS proven, reset-on-release load-bearing, no overclaim, pg.rs untouched | GREEN (3 integ + 842 default + ratchet + workspace) | 87a9c8e (+fix 2fa0260) |
| MR-007 | Durable principal + tuple stores (PG backing via MR-022 convention) | identity_durable.rs + pg conn-twins + new principal/credential_link RLS tables, 3 live-PG tests | INDEP verifier (live PG): ACCEPT-w/-followups → confirmed real force-RLS + durability + outbox co-commit; CAUGHT enum-indirection blinding the MR-004 ratchet → builder extended scanner to follow enums, baseline restored to honest 23 | GREEN (full lints + 3 integ + default + workspace) | 5952615 |
| MR-008 | Durable revocation + expiry stores (RevocationStore→PG; run-token TTL) | new revocation/run_token_teardown RLS tables, expires_at persisted, fail-loud writes/fail-closed reads, 3 live-PG tests | INDEP verifier (live PG): ACCEPT-w/-followups → CAUGHT a REJECT-level expiry fail-open (lexical timestamp compare) → builder fixed to instant-compare (chrono) + fail-closed-on-parse, regression tests added; found+baselined the S7Denylist machine-token revocation gap (→MR-011) | GREEN (full lints + 3+3 integ + default + workspace) | cf8ed01 |
| MR-023 | Events durable persistence + serve() (EventsRuntime: PgRelay outbox + NATS + durable dedup) | DurableDedup + EventsRuntime composition root, 3 live PG+NATS tests (0-lost/0-ghost/emit-iff-committed) | INDEP verifier (live PG+NATS): ACCEPT-w/-followups → 0-lost/0-ghost proven (mark-sent only after durable PubAck), dedup fail-direction safe, tenant-predicate exclusion legitimate; FOLLOW-UP: dedup mark not yet co-committed w/ handler state write (latent → MR-009/023b) | GREEN (full lints + 3 integ + default + no-regression + workspace) | 9c21d66 |
| MR-024 | Control-plane placement registry durable persistence (tenant_placement/cell tables + invariant trigger) | placement_durable.rs + registry_durable.rs, 3 live-PG tests; placement invariant as a REAL DB trigger | orch focused INDEP verify (live PG): cross-region placement REJECTED by trg_placement_invariant via direct psql (bypassing Rust); durability proven; lint exclusion legitimate (pg.rs/identity_durable still linted) | d2a5f92 |
| MR-025 | KMS durable cell-root + KEK/DEK persistence (software-sealed, SealKey from env) | kms_durable.rs + load_or_generate + backup_snapshot_durable, 3 live-PG tests | INDEP verifier (live PG + psql + own tamper probe): ACCEPT-w/-followups → wrong/missing/malformed/TAMPERED seal key fails closed + never mints a new root, root never plaintext at rest, seal key no-leak, crypto-shred survives restore | GREEN (full lints + 3 integ + 422 default + workspace) | (this) |

**Persistence wave COMPLETE** (MR-022 foundation + MR-007/008 identity + MR-023 events + MR-024 control-plane + MR-025 KMS).

| MR-009 | Durable persistence VERIFY — kill-9/restart + 3-instance across all 5 families | real SIGKILL writer bin + 6-test harness (identity+profile-decrypt, revocation/TTL, events 0-lost, control-plane, KMS); deliverable-2 (prod-default flip) SPLIT → MR-009b | INDEP verifier: ACCEPT (clean) — genuine SIGKILL of a separate OS process (signal 9 asserted, writer blocks forever no-flush), reads from live backends proven via wrong-seal-key fail-closed probe, split honestly deferred (baseline unchanged 24) | GREEN (full lints + 6 integ + default + workspace) | (this) |

**Master-plan Tier-0 done-bar MET: load-bearing state survives kill -9 + restart** (proven, not asserted). The production-default flip (un-gate durable layer + Memory→test-only → scanner green) is MR-009b — large blast radius (15 modules, 6 deps, all Memory variants), deferred not faked.

### E0.5 auth crypto (MR-010 split per credential type — Revision 2)

| MR-010a | OIDC JWKS real crypto (RS256/ES256/EdDSA, alg-confusion + alg:none defence, replay guard) | OidcVerifier behind the CredentialVerifier seam + 18-test negative corpus | INDEP SECURITY verifier: ACCEPT-w/-followups → ran 10 adversarial forgeries, COULD NOT forge a token past it (alg-confusion all key types, jwk/jku/x5u smuggling, ES256 DER malleability, tenant-injection all rejected); found a panic-DoS (alg byte-slice + i64 leeway overflow) on attacker bytes → builder fixed all 4 sites (saturating arith), verify now total over attacker input | GREEN (18 oidc + full lints + workspace; no new crates) | (this) |

MR-010a wires nothing into the prod default (StructuralVerifier still the OIDC default until MR-012); it ADDS the real verifier + proves it. JWKS network fetch/rotation deferred (injected key set).

| MR-010d | SSH pubkey challenge-response (Ed25519/RSA-SHA2, single-use TTL challenge, alg pinning, RSA≥2048 floor) | SshVerifier behind the seam + 19-test corpus (real keypairs/sigs/forgeries) | INDEP SECURITY verifier: ACCEPT-w/-followups → own 12-test adversarial suite, COULD NOT forge/replay/panic past it (consume atomic under 16 threads, domain-sep confirmed, wire-fuzz total); renames legit (ephemeral guard + read-index over PG SoR, not MR-007 evasion); found no-RSA-min-keysize → builder added ≥2048 floor | GREEN (19 ssh + full lints + workspace; no new crates) | (this) |

| MR-010b | SAML 2.0 XML-DSig (hand-built exc-c14n + structural XSW defence, no libxml/openssl) | SamlVerifier behind the seam + 26-test corpus (full XSW family, XXE/billion-laughs, c14n, non-anchored cert) | INDEP SECURITY verifier: ACCEPT-w/-followups → 22 adversarial attacks, COULD NOT authenticate any forgery (no XSW shape, no c14n collision — canonicalization injective, pointer-identity defence); found a deep-nesting stack-overflow DoS → builder added a parse-time depth bound (256) + regression test | GREEN (26 saml + full lints + workspace; no C/SAML crates) | (this) |

**Codex review attempted (user suggestion):** codex CLI is non-functional in this environment — `codex exec`/`doctor`/`login status` all exit 144 with no output (only `--help`/`--version` work; likely an auth/runtime/network issue specific to the API-calling subcommands). Relied on the thorough Claude security audit. If codex is fixed (auth/runtime) it can be a second lens on later security-critical prompts.

| MR-010c | WebAuthn/FIDO2 passkey (assertion + registration; ES256-DER/RS256/EdDSA; CBOR via ciborium) | WebauthnVerifier behind the seam + 25-test corpus (forged sig, counter-clone, alg-confusion, origin look-alikes, CBOR fuzz) | INDEP SECURITY verifier: ACCEPT-w/-followups → 15 adversarial attacks, COULD NOT forge any assertion; counter floor holds (failed sig does NOT advance counter), origin exact-match (punycode rejected), CBOR fuzz no-panic; packed-full/tpm/apple honestly refused-loud | GREEN (25 webauthn + full lints + workspace; +4 pure-Rust crates) | (this) |

**MR-010 (human/SSO real crypto) COMPLETE** — all four credential types (OIDC, SSH, SAML, WebAuthn) are real and each survived an independent security verifier trying to forge past it. Minor non-blocking follow-ups logged: WebAuthn RSA public-exponent explicit bound (redundant w/ the rsa crate); challenge burned on failed-sig assertion (availability note, not a bypass); MR-012 must flip the prod default (all four are test-wired today, Structural still the prod default until then).

| MR-011 | Machine/capability tokens + DPoP + durable revocation (PASETO v4.public + macaroon HMAC chain + RFC 9449) | capability_crypto.rs real signer/verifier; S7Denylist DELETED → revocation via durable RevocationStore; 17 corpus + 1 live-PG cross-restart test | INDEP SECURITY verifier: ACCEPT-w/-followups → 18 adversarial attacks, COULD NOT forge/amplify/replay (PAE binding, macaroon law doubly-enforced, DPoP sound, footer-key rejected); added own live-PG tenant-PARTITION test (no A→B revocation bleed) + fail-closed-on-store-error confirmed; S7Denylist genuinely gone (baseline 24→23, first net REMOVAL) | GREEN (17 + 1 live-PG + full lints + 848 default + workspace; hmac promoted, no new crate) | (this) |

**E0.5 auth crypto COMPLETE** — OIDC/SSH/SAML/WebAuthn (human/SSO) + machine/capability tokens+DPoP all real, all forge-audited. The carried-forward S7Denylist→durable-revocation gap is DISCHARGED (revoked machine tokens stay denied across restart). MR-011 follow-ups (non-blocking): kind/scheme not in the signed token body (deny-on-mismatch, not a bypass → MR-011b); DpopReplayGuard is per-instance in-memory (correct single-cell; multi-replica DPoP replay within the 60s window needs a shared/TTL'd store — fails closed; defer with multi-cell). NEXT: MR-012 (remove Structural* defaults — flip all the real verifiers to prod default; the no-structural-crypto scanner goes green) + MR-013 (RLS hardening).

| MR-011 | Real machine/capability-token crypto (PASETO v4.public + macaroon attenuation + DPoP, P-527) + route revocation through the durable RevocationStore (discharge the carried-forward S7Denylist gap) | `capability_crypto.rs`: Ed25519/PASETO-v4.public on `ring`, HMAC-SHA256 macaroon caveat chain on `hmac`+`sha2` (K_mac-seeded → extend-only/amplification-proof), RFC-9449 DPoP (Ed25519 proof, jkt-bind, htm/htu, freshness, single-use); real `TokenSigner`/`TokenVerifier` behind the seams; `CapabilityAuthenticator` field S7Denylist→durable `RevocationStore`; 17-test corpus (real forgeries) + live-PG cross-restart revocation test | builder self-verify: 17/17 corpus GREEN (forge/tamper/amplify/removed-caveat/expired/DPoP-missing/wrong-key/replay/wrong-htm-htu/stale-iat/garbage/tenant-injection) + live-PG `integration_mr011_machine_token_revocation_durable` GREEN (revoked machine token stays denied across a FRESH store over the same pool). **INDEP SECURITY verify PENDING** | GREEN (full lints 57 + ratchet 23/16; 848 workspace default ok / 0 failed; check workspace + integration; 1 dep promoted `hmac`) | (this) |

**Carried-forward S7Denylist machine-token revocation gap (MR-008→MR-011): DISCHARGED.** `CapabilityAuthenticator::authenticate` now consults the durable `(tenant, region)`-partitioned `RevocationStore` (the same `with_pg` store as every other revocation); the tenant-less in-memory `S7Denylist` stub is DELETED. Cross-restart denial proven against live PG. Baseline 24→23, no-in-memory 17→16 (S7Denylist removed, not supplemented). The `Structural*` machine-token verifier/signer DEFAULTS remain (MR-012 removes them); the REAL `PasetoCapability{Signer,Verifier}` are available behind `with_signer`/`with_verifier` and proven end-to-end. **MR-011b deferred (named, not faked):** binding `kind` into the signed body (kind is read from the credential scheme, as the structural floor did — a deny-on-mismatch hardening); PASETO footer key-id / key rotation (single anchor today); a full biscuit asymmetric block chain (the macaroon HMAC chain is the sound, simpler construction); the runtime gateway DPoP `htm`/`htu` binding wiring (injected seam today). **MR-012** still owes the prod-default flip (Structural→real verifier/signer as the non-optional default).

## Test environment (verified live 2026-06-26 — every persistence/auth prompt uses this)

Real backends run via `docker-compose.dev.yml` and are UP (confirmed `smoke_backends` integration test
green: pg connect, s3 put/get, rebac tuples, outbox→bus relay, valkey cache):
- **Postgres** `myelin-postgres` on **:5433** — app role `postgres://myelin_app:myelin_app_pw@localhost:5433/myelin`
  (RLS-enforced), admin role `myelin_admin`/`myelin_dev_pw`.
- **Valkey** `:6380` → `REDIS_URL=redis://localhost:6380`. **NATS** `:4222`. **rustfs/S3** `:9000`
  (`S3_ENDPOINT=http://localhost:9000`, key `myelin_dev_access`/`myelin_dev_secret`, region `fr-par`).
- Tests reach them only under `--features integration` (env vars above). Run example:
  `DATABASE_URL=… REDIS_URL=… cargo test -p <crate> --features integration --test <file>`.
- Real PG-backed impls already exist behind the `integration` feature; the in-memory versions are the
  DEFAULT/production path today (the census's core finding). "Make it real" = make the real path the default.

## Carried-forward obligations for later prompts

- **Git smart-transport MR:** populate `KeyBindingIndex` (SSH key→principal) from the durable PG `PrincipalStore`
  on key registration, and issue the SSH auth challenge on the git-transport handshake (MR-010d ships these as
  thin injected layers; the crypto is real + proven, the runtime wiring is deferred — honestly scoped).

- **MR-009 / MR-023b: the durable dedup mark MUST co-commit with the consumer handler's state write.**
  MR-023's `DurableDedupBacking::mark_handled` commits in its OWN autocommit tx BEFORE the handler runs.
  Latent today (no production durable consumer with state writes rides this path yet), but the moment MR-009
  points a real consumer at the durable dedup ledger, a crash between mark-commit and handler-effect =
  SILENT LOSS. Thread the handler's tx so the mark + the handler state write commit atomically (or use the
  consume→handle→ack ordering with the mark inside the handler's tx). Documented in dedup.rs/events_durable.rs.
- **Lint-harness improvement (low-pri, affects all `*_durable.rs`):** the workspace `is_excluded` skips a
  named file from ALL twelve lints, not just the one it false-positives on (tenant-predicate). Low risk today
  (no other lint applies to the durable backings), but a future edit to events_durable/placement_durable/
  kms_durable is unguarded by the other eleven. Add per-lint exclusion granularity when the lint harness
  supports it (a `(path, lint_id)` exclusion instead of a whole-file path).
- **KMS minor (MR-025, non-blocking):** `destroy_kek` leaves orphan wrapped-DEK rows (harmless — KekUnavailable
  on resolve, excluded from snapshot); a sweep could reap them. `upsert_sealed_root` is a root-overwrite
  primitive (safe for restore-into-clean; watch new callers).
- **Low-pri check (pre-existing, not MR-023):** snapshot/reindex events use a deterministic FNV-1a
  `event_id` (not tenant-derived); confirm aggregate keys are globally unique so a cross-tenant consumer
  replaying snapshots can't dedup-collide. Property of the frozen ledger, unchanged by MR-023.
- **[DISCHARGED by MR-011] MUST route `CapabilityAuthenticator` through the durable `RevocationStore`.**
  MR-008 found `S7Denylist` (`machine_auth.rs:347`) is a tenant-less in-memory jti set rebuilt empty on
  construction — a machine-token jti revoked only there RE-VALIDATES after restart (a real revocation gap).
  MR-011 routed `CapabilityAuthenticator::authenticate` through the durable `(tenant, region)`-partitioned
  `RevocationStore` (fail-closed reads), DELETED the stub type, and proved the cross-restart denial against
  live PG (`integration_mr011_machine_token_revocation_durable`). Baseline 24→23, no-in-memory 17→16.
- **Before any real run-token timestamp writer (P-ID-18) / in MR-011:** the run-token expiry guarantee must
  stay structural. MR-008 fixed its OWN expiry comparison to parse instants (was a lexical string compare that
  failed open on non-normalized timestamps), but the shared `Timestamp(String)` type (myelin-events) is still
  unnormalized — any NEW expiry comparison must parse instants (or normalize at the Timestamp boundary, the
  deferred typed-clock change), never lexical-compare raw RFC3339 strings.


- **MR-009 (or the identity route-body MRs) must:** (a) wire the durable `with_pg` PrincipalStore/TupleStore
  into the production boot spec (`identity_app_spec`) as the non-optional default; (b) un-gate the storage
  real-pool layer so the durable code compiles in the default/production build (the `integration` feature
  should gate the live-backend TESTS, not the production durable CODE — this is SI-022, a storage feature-graph
  decision, deliberately deferred from MR-007); (c) do the kill-9/restart proof + the profile-decrypt-across-
  restart proof (needs MR-025 KMS durable root). When (a)+(b) land, the two principal/tuple baseline entries
  flip from present→removed (the ratchet proving the in-memory default is finally gone, not just supplemented).
- **MR-004 ratchet now follows enum variants** (closed the MR-007 enum-indirection blind spot): a durable
  `*Store` whose backend is an in-memory-capable enum fires. Baseline honest at 23 (16 no-in-memory).

## Shape-review outcomes (MR-006) — binding on later prompts

- **RESHAPE-002 (on the spine critical path) → folded into MR-022.** `SET LOCAL` RLS is a silent no-op on a
  bare pooled connection with no transaction (standard PG semantics, confirmed). So MR-022 (persistence
  foundation) must establish the **tenant-scoped-transaction connection convention** (acquire → BEGIN → set
  tenant/region GUC via `SET LOCAL`/`set_config(...,true)` → use → COMMIT + reset-on-release) BEFORE the four
  durable-store MRs (007/008/023/024) bind to the wrong pattern. MR-013 then enforces it. Baked into the MR-022
  prompt + task.
- **RESHAPE-001 (OFF the spine) → CI track.** `SandboxHandle{guest_id}` + `launch()->Result<SandboxHandle>`
  cannot carry a command's exit/stdout/stderr/usage; the result/lifecycle seam must be redrawn before P-544
  (sandbox prod exec). Tracked as a task for the deferred CI long-pole; not a spine blocker.
- **Confirmed SHAPE-OK (harden behind existing seams, no redraw):** identity authz (`with_verifier`/`with_signer`
  injectors exist → MR-010/011 drop in, MR-012 removes default), KMS envelope (MR-025 is additive), GDPR
  tagging→shred→RoPA, and the agent mock→`LlmAgentRuntime` seam (`AgentRuntime::step` clean swap; `EffectApi::apply`
  is the brain-agnostic governance chokepoint → **binding on MR-021: local Claude over MCP routes through
  `mint_run_token → EffectApi`, NOT a bare human PAT**, so agent governance is real from day one).
- **Single-cell dogfood path confirmed clean** through the multi-cell machinery (`DegenerateControlPlane`, shared
  organs, no fork) — multi-cell stays dormant-but-present, not on the critical path.

## Decisions & deviations

- **Census ran MR-001 + MR-002 concurrently** (read-only, disjoint crates, separate output files, both in W1). Sequential build discipline still applies from MR-004 on.
- **Orchestrator verification of census:** spot-checked the load-bearing CRITICAL claims against source rather than trusting verbatim. MR-001 auth-crypto (`StructuralVerifier` `identity-service/authenticate.rs:146`, `StructuralTokenSigner` `mint.rs:164`), RLS bleed (`storage/pg.rs:413` `set_config(...,false)`); MR-002 sandbox (`firecracker.rs:114` `init=/bin/true`, `gvisor.rs:230` `runsc --version` probe, `spec.command` unused at `gvisor.rs:67`), git `RefStore` in-memory (`receive_pack.rs:537`). All confirmed accurate. A path-prefix audit pass on MR-001 fixed 2 cross-crate refs; identity findings were already correctly prefixed `myelin-identity-service`.
- **LEDGER REVISION 1 (orchestrator steering decision, post-MR-003).** The synthesis found the spine's
  durable-persistence coverage (MR-007/008) was identity-crate only, leaving 5 CRITICAL load-bearing substrate
  organs with no spine prompt. Verified two gap claims against source before acting: migration runner `run()`
  (`substrate/migrations.rs:108`) executes no real DDL (doc admits "DDL execution lands with the driver");
  KMS mints a fresh `RawKey::generate()` root per process (`storage/kms.rs:256`) with no durable backing →
  MR-009's restart verify would be hollow. Inserted **MR-022..MR-025** (foundation, events, control-plane, KMS
  root) into the W2 persistence band; expanded MR-009 verify to all four store families. Destination unchanged
  (master plan already requires no-HashMap-for-load-bearing-state); this is the authoring-time split the ledger
  anticipated. Git ref-store/server/backup (SI-012/13/14/15) stay on the Git subsystem track, not the spine.
  Did NOT block the user on a question — autonomous batch, faithful to the master plan, fully reversible (planning).
- **MR-004 verification loop (the cardinal rule in action).** Builder shipped 3 scanners + a two-way baseline
  ratchet (148 tests). An INDEPENDENT verifier (never touched the code) ran it adversarially against the census
  and found 3 real false negatives in `no-in-memory-durable-store` — type-alias collection fields
  (`PseudonymErasureLedger`), `Vec`/`VecDeque`-backed ledgers (`InMemoryPostPitLedger`, SI-028 `MisrouteAudit`),
  and a wrongly-excluded in-memory blob store (`FsBlobStore`, excluded on a FALSE "fs::write byte-durable"
  premise — confirmed 0 `std::fs` calls in blob.rs). Sent back to builder; all 4 closed (baseline 19→23, +6
  admit-tests proving no new false positives; SI-028 caught via a precise named-holder entry, not a blanket
  suffix). This is exactly why builder≠verifier: the gate that everything downstream is certified against had
  holes precisely in the persistence surface it must certify. **Known coverage boundary (documented in
  `production_graph.rs`):** scanner #2 keys on role-suffix/named-holder, so non-suffix census sites (S7Denylist,
  Consumer, Firehose, InMemoryShredder, OltpPool, PlacementService) are NOT yet gated — the events/control-plane
  persistence MRs (MR-023/024) must extend `NAMED_DURABLE_HOLDERS`/`DURABLE_ROLE_SUFFIXES` when they land.
- **Gate note:** `cargo test -p myelin-lints` was already RED on main before MR-004 (m6-scorecard.rs missing
  from the no-host-exec exclusion list — same class as the excluded m3/m4/m5 runners). MR-004 restored it. The
  baseline-green check must use `cargo test`, not just `cargo check`, going forward.
- **Top systemic truths carried forward:** (1) the production auth graph is wired to `Structural*` mock crypto by default → total forgery; (2) tenant RLS bleeds across pooled connections; (3) no durable persistence anywhere load-bearing (identity stores, events outbox, KMS keys, git refs/pack-index all in-memory); (4) the sandbox never runs `spec.command` in prod and the escape gate certifies a path real jobs don't take; (5) git has no prod WireExecutor/server binary; (6) the E0.2 absence-scanners that should mechanically block these do not exist yet → **MR-004 is the true first build dependency.**
