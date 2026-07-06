# Support crates (myelin-harness, myelin-lints, myelin-content, myelin-config)

_The four support crates are healthy and unusually well-tested. myelin-content (ADF map, inline grammar, offset bijection, DOM surgery) has robust untrusted-input handling — the parser never fails on arbitrary input, node-array splits clamp to length so there are no panic paths, and round-trip/offset invariants are exhaustively gated. myelin-harness's soundness machinery genuinely asserts: telemetry `Assertion` is `#[must_use]`, an absent signal is RED (not a silent pass), a vacuous predicate is Rejected, and `expect_green` panics loudly; the scorecard ratchet panics on an empty proof and the make-it-real gate binds each PASS to a blake3 attestation over real command output. myelin-lints' engine reports typed violations with no swallow path. Only two issues worth flagging: a latent secret-handling footgun in myelin-config (derived Debug exposes the DB password and S3 secret key), and a documented-but-unenforced label-shape guard in the telemetry library (fails safe to RED, so not a false-green risk)._

**Kept findings:** 2  (🟡 1 medium  ·  🔵 1 low)

---

### 1. 🟡 Config secrets (DB password, S3 secret/access keys) are exposed in plaintext via derived Debug

- **Severity:** medium  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** security
- **Location:** `crates/myelin-config/src/lib.rs:103`

**What:** `S3Config` (line 103) and `MyelinConfig` (line 87) both `#[derive(Clone, Debug, PartialEq, Eq)]` with all-public fields, including `secret_key`, `access_key`, and `database_url` (which carries the Postgres password). There is no custom redacting `Debug` impl and no secret-wrapper newtype. Any `{:?}`/`tracing::debug!(?cfg)` on the config — or on any struct that embeds it — prints the S3 secret key and the DB password verbatim.

**Impact:** On an EU-sovereign, GDPR-safe-by-construction platform this is a credential-in-logs footgun. The crate's entire purpose is the secret/endpoint seam and nothing redacts. A single downstream Debug (a boot-time connection error including the config, a tracing span) would write Scaleway IAM secret keys and the Postgres password into log sinks. This is latent, not active — no current consumer Debug-logs the config — but the derive keeps the door open for any future caller.

**Fix:** Give `S3Config`/`MyelinConfig` a hand-written `Debug` that redacts `secret_key`, `access_key`, and the password component of `database_url` (print `"***"`), or wrap secret fields in a `Secret<String>` newtype whose `Debug`/`Display` redact. Keep structural fields (endpoint, region, bucket, `force_path_style`) visible for diagnostics.

> _Verifier note:_ Read crates/myelin-config/src/lib.rs:87-118. Confirmed both structs `#[derive(Clone, Debug, PartialEq, Eq)]` with pub `secret_key`/`access_key` (lines 110-112) and pub `database_url` (line 90). No custom Debug impl found for either type (grep-level absence in the shown region; the derive line is the only Debug). Kept severity at medium: real footgun but latent — reviewer already noted no consumer currently Debug-logs it, and defaults populate dev creds not prod secrets, so exploitation requires a future prod caller adding a Debug of the config.

### 2. 🔵 Telemetry RejectReason::LabelShapeMismatch is never produced — the documented scalar-vs-labelled guard does not exist

- **Severity:** low  ·  **Verdict:** ✅ CONFIRMED  ·  **Category:** correctness
- **Location:** `crates/myelin-harness/src/telemetry.rs:340`

**What:** `RejectReason::LabelShapeMismatch` (line 340, doc: "A scalar signal was asserted with labels, or a labelled signal asserted as scalar") is declared but never constructed. `assert_signal` (line 479) reads only the `scalars` map via `self.scalar(name)` and never checks whether the name is a labelled signal; `assert_labelled` (line 512) reads only the `labelled` map via `self.labelled(name, ..)` and never checks whether the name is a scalar kind. There is no `SignalName::is_labelled()`/`is_scalar` classification, so the shape check the type advertises is not enforced.

**Impact:** A drill that asserts a labelled signal via `assert_signal` (or a scalar via `assert_labelled`) is not `Rejected` as the doc claims; it silently reads the wrong map, finds the value absent, and returns RED (`observed: None`). This fails safe (never a false green — the module's soundness invariant holds) so it is not a security/soundness hole, but the type's own contract ("a misuse cannot masquerade as a pass", the `LabelShapeMismatch` variant + its doc) is unfulfilled dead code that could mislead future drill authors into thinking the guard protects them.

**Fix:** Either add a `SignalName::is_labelled()` classification and have `assert_signal`/`assert_labelled` return `Assertion::rejected(.., LabelShapeMismatch)` on a shape mismatch (making the documented guard real), or remove the `LabelShapeMismatch` variant and its doc claim so the type does not promise a check it does not perform.

> _Verifier note:_ grep -rn 'LabelShapeMismatch' across crates/ returns exactly one hit: the declaration at telemetry.rs:340. grep for 'is_labelled'/'is_scalar' returns nothing. Read assert_signal (lines 479-506) — only branches on `self.scalar(name)`, returns Red on None. Read assert_labelled (512-539) — only branches on `self.labelled(name, &labels)`, returns Red on None. Neither classifies the SignalName kind nor constructs LabelShapeMismatch. Impact wording confirmed: absent-in-wrong-map yields Red observed:None, which is fail-safe. Low severity is correct — dead-code/contract gap, no false-green risk.
