# Storage layer (myelin-storage)

_I read the storage substrate's highest-risk modules in depth: the RLS/IDOR floor (`rls::TenantScope::resolve` / `TenantQuery::predicate_sql`), the co-located outbox co-commit (`coloc::ColocatedTx::commit` / `commit_with_state_fault`), the content-addressed blob tier (`blob::FsBlobStore::put`/`get` re-hash-on-read + `key_path`) and its live S3 backing (`s3blob::S3BlobStore`), the KMS envelope hierarchy (`kms::KmsEngine::resolve_dek`/`rotate_kek`/`destroy_kek`/`backup_snapshot`, `CellRoot::seal`/`unseal`, AES-256-GCM DEK wrap), the durable cost ledger (`reserve_settle::CostLedger::reserve`/`settle`/`cancel_unstarted`), the online-migration ordering gate (`migration::OnlineMigrationRunner::run` + `next_progress`, `is_destructive`/`is_blocking_alter`), the race-safe live migrator (`pg_migrator::PgMigrator::apply` advisory-lock + version table), the tenant-scoped-transaction convention (`tenant_tx::with_tenant_tx` SET LOCAL GUC + `connect_pool_with_reset`), and the live Postgres RLS store (`pg::PgStore`). Overall this is unusually strong, defensively-written code: the OLTP path uses parameterized sqlx binds throughout (no string-built SQL), tenant scoping is enforced both by transaction-scoped `(tenant,region)` GUCs under FORCE ROW LEVEL SECURITY and by explicit predicates; the KMS never exports plaintext key material, redacts Debug, uses vetted RustCrypto AES-256-GCM, walks the full L0→L1→L2 hierarchy on every resolve, and fails closed (0 fail-open) on destroyed/unauthenticating keys; the cost ledger uses checked u64 arithmetic with an idempotent, never-interrupt-in-flight state machine; the migrator serializes DDL on a fixed advisory lock + version table to close the pg_type race; co-commit is atomic in both directions. Most modules are explicitly in-memory MODELS with named driver floors, so several observations are hardening gaps that bite when the real backing lands rather than live exploits. The concrete issues below concern the S3 object backing (a real, integration-compiled surface) and DDL/identifier-sanitization patterns at isolation boundaries._

**Kept findings:** 3  (🔵 3 low)  ·  1 rejected by verifier

---

### 1. 🔵 Unsanitized tenant id interpolated into blob storage key path (cross-tenant collision / path traversal at the isolation boundary)

- **Severity:** low  ·  **Verdict:** 🟨 PLAUSIBLE  ·  **Category:** tenant-isolation
- **Location:** `crates/myelin-storage/src/s3blob.rs:86`

**What:** `S3BlobStore::key_path` (and the identical `blob::FsBlobStore::key_path`, blob.rs:414) build the per-tenant object key as `format!("{}/{}/{}/{}", tenant.0, algo, fan, rest)` using the raw `TenantId` string. `TenantId` is an unvalidated `String` newtype (see rls.rs/tests). A tenant id containing `/` (or `..`) produces a key that overlaps another tenant's keyspace — the `<tenant>/` prefix is the sole documented per-tenant isolation/dedup boundary (§3.2), and this is a live S3 bucket, not just an in-memory map. Tenant ids come from verified tokens today, but nothing structurally prevents a `/`-bearing id from defeating the keyspace split.

**Impact:** If any tenant id ever contains a path separator, two tenants can collide on stored objects or one can address another's keyspace — a cross-tenant leakage/overwrite in the content-addressed tier whose entire isolation guarantee is the tenant key prefix.

**Fix:** Validate/encode the tenant segment at the isolation boundary: reject or percent/hex-encode `TenantId` components before building the key (or assert an opaque-ULID charset on construction). Apply consistently to `FsBlobStore::key_path`, `S3BlobStore::key_path`, and any object-store backing.

> _Verifier note:_ Premise confirmed in source: TenantId is `pub String` (myelin-tenancy/src/lib.rs:112); its only constructor from_token does NO charset validation (lib.rs:121-123) and the pub field permits direct construction; both key_path sites raw-interpolate the tenant segment via `format!("{}/{}/{}/{}", tenant.0, ...)` (s3blob.rs:86, blob.rs:414), and this prefix is the sole documented per-tenant isolation/dedup boundary. So a `/`- or `..`-bearing id would structurally overlap another tenant's keyspace. However, real impact is not confirmable from this unit: the finding itself concedes ids come from verified opaque ULID tokens (lib.rs:86,115 doc), whose Crockford-base32 charset contains no path separator, and whether a slash-bearing id can ever be minted depends on the token-issuance path outside myelin-storage. Downgraded to low: a legitimate defense-in-depth hardening gap (the boundary should reject/encode the segment) with no demonstrated injection vector on the current wiring.

### 2. 🔵 RLS-floor predicate rendered by unescaped single-quote SQL string interpolation

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security-pattern
- **Location:** `crates/myelin-storage/src/rls.rs:210`

**What:** `TenantQuery::predicate_sql` renders the mandatory-core `(tenant, region)` IDOR predicate with `format!("{} WHERE tenant = '{}' AND region = '{}'", table, tenant.0, region.0)` — raw single-quote interpolation of the tenant/region values with no escaping. This is the SQL-injection-shaped pattern living in the module whose whole purpose is the cross-tenant isolation floor. On the current floor it is a model (not executed; the live path in pg.rs correctly uses `$1/$2` binds), so this is a smell, not a live exploit — but it is the exact pattern the P-S12 driver must not inherit.

**Impact:** If this rendering is ever carried into a real execution path, a tenant/region value containing a quote breaks out of the predicate — directly undermining the IDOR floor. Even as a model it invites a future driver to copy the wrong shape.

**Fix:** Render the predicate with bind placeholders (`WHERE tenant = $1 AND region = $2`) and expose the bind values separately, matching the parameterized convention already used in `pg::PgStore`; never string-interpolate tenant/region into SQL.

> _Verifier note:_ Confirmed exactly as described at rls.rs:210-217: predicate_sql renders `format!("{} WHERE tenant = '{}' AND region = '{}'", table.name(), scope.tenant().0, scope.region().0)` — raw single-quote interpolation, no escaping. Also confirmed the finding's own mitigation: this is a non-executed model, not a live path. predicate_sql is referenced only from tests (knowledge/store.rs:409, storage/tests/cdc_11_1.rs:55, idor_drill.rs:66, rls.rs:305) and as an unused `_q` binding with an explanatory comment (identity-service tuple_store.rs:366-367); it is never passed to a DB executor. The live execution path (pg.rs) uses parameterized sqlx::query(...).bind(...) with $1/$2 placeholders throughout (pg.rs:327-335, 365-371, 491-495, 581-585). Low severity is correct: a copy-the-wrong-shape smell in the isolation module, not an exploitable injection.

### 3. 🔵 Forward-only DDL classifier misses TRUNCATE (and other data-destructive statements)

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** data-integrity
- **Location:** `crates/myelin-storage/src/migration.rs:222`

**What:** `is_destructive` only flags substrings `DROP TABLE` / `DROP COLUMN`. It does not catch `TRUNCATE` (unconditional data loss) or `DROP SCHEMA`/`DROP TABLESPACE`. A migration whose DDL is `TRUNCATE issue` passes `is_destructive` (false) and is admitted by both `OnlineMigrationRunner::run` and `PgMigrator::apply`/`apply_validated`, which claim to enforce "forward-only" (a rollback is a new forward migration, never destructive). Being substring-based it is also fragile to false negatives via comments/identifiers, though the misses skew data-destructive here.

**Impact:** A destructive `TRUNCATE` in a migration set is silently admitted and executed by the live migrator, contradicting the forward-only guarantee the runner advertises and the substrate re-exports.

**Fix:** Extend `is_destructive` to also reject `TRUNCATE`, `DROP SCHEMA`, `DROP TABLESPACE` (and consider tokenizing rather than substring-matching so identifiers/comments don't cause false positives/negatives); keep the storage and substrate predicates in lockstep since they share this single authority.

> _Verifier note:_ Confirmed at migration.rs:222-225: is_destructive returns true only for substrings `DROP TABLE`/`DROP COLUMN`, so `TRUNCATE issue`, `DROP SCHEMA`, `DROP TABLESPACE` all return false. Confirmed every forward-only gate delegates solely to this predicate: OnlineMigrationRunner::run (migration.rs:366-368), the substrate boot-time MigrationRunner::run (myelin-substrate/src/migrations.rs:114), and the live driver PgMigrator::apply_validated (pg_migrator.rs:159) plus the under-lock apply_locked (pg_migrator.rs:202). So a TRUNCATE migration is admitted and executed by the live migrator, contradicting the advertised forward-only guarantee. Severity low is appropriate rather than higher: migration DDL is developer-authored static content (Migration.ddl is `&'static str`, embedded in-repo), not attacker-controlled input — this is a correctness/lockstep gap in the classifier, and the recommendation to also reject TRUNCATE/DROP SCHEMA/DROP TABLESPACE (and prefer tokenizing over substring-matching) is valid.

---

<details><summary>Rejected by verifier (false positives / already handled)</summary>

- **S3 object backing cannot serve SHA-256-addressed blobs, breaking object-backed git packs / STOR-D7 on the object tier** (`crates/myelin-storage/src/s3blob.rs`) — The impact/mechanism is wrong. I traced the actual object-tier git path: gitpack.rs::put_object (line 420) stores git objects via `self.blobs.put(&self.tenant, &framed)`, and BlobStore::put ALWAYS computes ContentHash::blake3 (FsBlobStore::put blob.rs:440; S3BlobStore::put s3blob.rs:100) — the caller cannot choose sha256. get_object (gitpack.rs:436-447) resolves the git SHA to the NATIVE blake3 address via native_for_sha and calls blobs.get(native) with a BLAKE3 hash; the sha256 re-verify at gitpack.rs:448 runs at the gitpack layer on already-retrieved bytes. So the BlobStore is NEVER called with a Sha256 ContentHash on the git path, and the S3 AlgoNotVerifiable arm (s3blob.rs:148) is unreachable in production. No trait path ever stores a sha256-addressed blob; the fs sha256 arm (blob.rs:473 rehash) is exercised only by a test that inserts directly into the map (blob.rs:749), bypassing put. Therefore SHA-256 git blobs are NOT unreadable on S3 and object-backed packs / STOR-D7 are not broken. A genuine but inert code-parity divergence does exist (FsBlobStore::get supports a sha256 rehash arm, S3BlobStore::get returns AlgoNotVerifiable), but it is dead code on the current wiring — a nit, not the medium functional regression claimed.

</details>
