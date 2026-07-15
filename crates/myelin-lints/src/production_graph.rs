//! # Production-graph ABSENCE scanners (MR-004) — the evidence-integrity skeleton.
//!
//! Three hermetic source-scanners that DETECT the live make-it-real shortcuts the census named, so
//! nothing below MR-004 can green-lie about having removed them. They are built in the EXACT
//! `myelin-lints` idiom (the [`crate::engine::Lint`] / [`crate::engine::Violation`] vocabulary, the
//! `code_lines`/`code_statements` CODE-only front-end, typed LOUD violations, a red + a green
//! fixture each) — this module EXTENDS the existing engine; it does NOT fork a parallel framework.
//!
//! ## Why these three are NOT part of `all_twelve()` / the `workspace_clean` zero-violation gate
//! The twelve architecture lints (`lints.rs`) gate code that is ALREADY clean — the workspace scan
//! asserts ZERO violations. These three are the OPPOSITE shape: the real tree currently VIOLATES
//! all three (that is the whole point — they document the live shortcuts MR-009/MR-012/MR-013 will
//! remove). Adding them to `all_twelve()` would turn the build red. Instead they ship a **baseline
//! ratchet** (`tests/production_graph_absence.rs`): the test asserts the violation set EQUALS a
//! committed baseline manifest of the CURRENT known-violation sites — not zero. The ratchet can
//! only TIGHTEN toward zero: it fails LOUDLY if (a) a NEW violation appears, or (b) a baseline
//! entry is fixed but not removed from the manifest. That is the mechanism by which MR-009/012/013
//! prove they actually removed the shortcut (EI-01 §5 — an uncommitted gate is no gate).
//!
//! The scanners (the original three + the R2.6 fourth):
//! 1. [`no_structural_crypto_in_prod`] — a `Structural*` mock-crypto verifier/signer CONSTRUCTED in
//!    the production graph (outside `#[cfg(test)]`). Census SI-001..SI-004 (P-526/527/528, MR-012).
//! 2. [`no_in_memory_durable_store`] — a durable-by-contract store/registry/outbox/ledger backed by
//!    an in-memory collection with no pool field. Census SI-006/007/011/018/019/020 (P-522/523,
//!    MR-009).
//! 3. [`no_bare_tenant_pool`] — session-scoped `set_config(..., false)` RLS (leaks across pooled
//!    connections) + the bare raw-pool hatch. Census SI-005 (P-531, MR-013).
//! 4. [`no_permissive_authorizer_in_prod`] — the permissive `AllowAll`/`AllowAllRepos` authorizer
//!    fixtures CONSTRUCTED in the edge production graph (outside `#[cfg(test)]`/`test-support`).
//!    R2.6 (action seam) / R2.1a-R2.1 (object seam).

use crate::engine::{blank_string_literals, code_lines, Lint, LintId, Violation};

// ================================================================================================
// Shared helper: `#[cfg(test)]` region detection.
//
// The three scanners target CONSTRUCTION/WIRING in the PRODUCTION graph — a `Structural*` verifier
// or an in-memory store wired under `#[cfg(test)]` is a TEST double and must NOT be flagged (the
// green fixtures prove `#[cfg(test)]`-gated wiring is admitted). `code_lines` keeps `#[cfg(test)]`
// (it is code, not a comment), so we can track which lines sit inside a `#[cfg(test)]`-gated item.
// ================================================================================================

/// For each 1-based line of `src`, whether it sits inside a gated block whose OPENING attribute
/// `is_gate` matched. A gate attribute arms the NEXT block it opens; the region ends when that
/// block's braces close. Conservative + hermetic (pure fn of source text). The `is_gate` predicate
/// is applied to the CODE-only line text (comments already stripped, string literals intact) so a
/// gate substring inside a `#[cfg(...)]` attribute (e.g. `feature = "test-support"`) is seen.
fn cfg_line_flags(src: &str, is_gate: impl Fn(&str) -> bool) -> Vec<bool> {
    let lines = code_lines(src);
    let max_line = lines.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut flags = vec![false; max_line + 1];
    let mut depth: i32 = 0;
    // The brace depths at which an ACTIVE gated block opened (a stack so nested gated items are
    // handled). While the stack is non-empty we are inside gated code.
    let mut stack: Vec<i32> = Vec::new();
    let mut pending = false; // a gate attribute seen, awaiting the item it attributes.
    let mut pending_depth: i32 = 0; // the brace depth at which the pending gate attribute sits.
    // `()`/`[]` nesting, used ONLY to find the TOP-LEVEL `;` that terminates a BRACELESS gated item
    // (a `use`/`const`/`type`), so a gate attribute cannot leak onto a later unrelated `{`.
    let mut nest: i32 = 0;
    for (lineno, code) in &lines {
        // The gate attribute arms the item it attributes — but ONLY if it is in ATTRIBUTE position
        // (the line starts with `#[`). A `cfg(test)` / `test-support` substring sitting inside a
        // string or a `const`/`type` RHS (e.g. `const D: &str = "cfg(test)";`) must NOT arm the gate
        // (Wave-0 probe B2). Matched on the RAW line so the `test-support` gate's OWN string literal
        // (`feature = "test-support"`) is still seen (blanking it would defeat the gate).
        if code.trim_start().starts_with("#[") && is_gate(code) {
            if !pending {
                pending_depth = depth;
            }
            pending = true;
        }
        // The line's gate status is decided BEFORE this line's own braces take effect.
        if *lineno < flags.len() {
            flags[*lineno] = !stack.is_empty();
        }
        // Blank string literals for the DELIMITER scan so a brace/semicolon inside a string cannot
        // miscount `depth`/`nest` or falsely terminate a pending item.
        for ch in blank_string_literals(code).chars() {
            match ch {
                '(' | '[' => nest += 1,
                ')' | ']' => {
                    if nest > 0 {
                        nest -= 1;
                    }
                }
                '{' => {
                    depth += 1;
                    if pending {
                        stack.push(depth);
                        pending = false;
                    }
                }
                '}' => {
                    if stack.last() == Some(&depth) {
                        stack.pop();
                    }
                    depth -= 1;
                    // A gate that never opened a block before its ENCLOSING scope closed attributed a
                    // BRACELESS item (e.g. a unit enum variant `#[cfg(test)] Fake,`) — drop the
                    // dangling gate so it cannot arm a later unrelated block (Wave-0 probe K).
                    if pending && depth < pending_depth {
                        pending = false;
                    }
                }
                ';' if pending && nest == 0 => {
                    // A BRACELESS statement item (`use`/`const`/`type`) ended: it opened no block, so
                    // the gate attributed only itself. Drop the dangling gate so it does NOT arm the
                    // next unrelated struct's `{` (Wave-0 probes I/G/J — the `pending`-leak root fix).
                    pending = false;
                }
                _ => {}
            }
        }
    }
    flags
}

/// The EXACT `test-support` cargo-feature gate substring the in-memory durable-store scanner (only)
/// treats as a test-double gate (MR-009b Wave 0). Matched EXACTLY (`feature = "test-support"`) — a
/// `#[cfg(feature = "test-support")]` / `#[cfg(any(test, feature = "test-support"))]` block is a
/// test-double gate (the in-memory doubles that downstream crates enable as a DEV-dependency), NOT a
/// production store. This is deliberately NOT broadened to arbitrary features (that would admit a
/// real prod store hidden behind some other feature); the platform convention is that ONLY
/// `test-support` gates the durable-store doubles.
const TEST_SUPPORT_GATE: &str = "feature = \"test-support\"";

/// For each 1-based line of `src`, whether it sits inside a `#[cfg(test)]`-gated block (a test
/// module or a test fn). A `#[cfg(test)]` attribute arms the NEXT block it opens; the region ends
/// when that block's braces close. Conservative + hermetic (pure fn of source text). Used by the
/// `no-structural-crypto-in-prod` scanner (and anywhere the LITERAL `cfg(test)` gate is meant) —
/// deliberately NOT test-support-aware, so that scanner is unaffected by Wave 0.
fn cfg_test_line_flags(src: &str) -> Vec<bool> {
    cfg_line_flags(src, |code| code.contains("cfg(test)"))
}

/// Like [`cfg_test_line_flags`], but ALSO treats the `test-support` cargo-feature gate
/// ([`TEST_SUPPORT_GATE`]) as a test-double gate — so a `#[cfg(feature = "test-support")]` /
/// `#[cfg(any(test, feature = "test-support"))]` struct/enum is recognized as a test double. Used
/// ONLY by the `no-in-memory-durable-store` scanner (via [`parse_struct_defs`]/[`parse_enum_defs`]),
/// so the other two scanners' `cfg(test)` handling is untouched (MR-009b Wave 0).
fn cfg_double_line_flags(src: &str) -> Vec<bool> {
    cfg_line_flags(src, |code| {
        code.contains("cfg(test)") || code.contains(TEST_SUPPORT_GATE)
    })
}

/// Whether `text` carries a double-gate token (`cfg(test)` or the exact `test-support` feature gate).
/// Matched on the raw text (the gate's `feature = "test-support"` lives in a string literal, so it
/// must NOT be blanked). Callers restrict WHERE this is consulted to real attribute positions.
fn text_has_double_gate(text: &str) -> bool {
    text.contains("cfg(test)") || text.contains(TEST_SUPPORT_GATE)
}

/// Peel a line's LEADING `#[..]` attributes and report `(any_leading_attr_is_a_double_gate,
/// remaining_code_after_the_attributes)`. This is how a gate is scoped to the item it ACTUALLY
/// attributes: a PURE attribute line (remainder empty) attributes the item on the NEXT line; an
/// attribute WITH code after it (e.g. `#[cfg(test)] Fake,` or `#[cfg(test)] use x as M;`) attributes
/// its OWN co-located item and must NOT be mistaken for the gate of a later struct/enum.
fn leading_attr_gate(line: &str) -> (bool, String) {
    let mut s = line.trim_start();
    let mut gated = false;
    while s.starts_with("#[") {
        // Find the matching `]` of this `#[..]` (bracket-balanced, so `#[cfg(any(..))]` is one attr).
        let mut d: i32 = 0;
        let mut end = None;
        for (idx, ch) in s.char_indices() {
            match ch {
                '[' => d += 1,
                ']' => {
                    d -= 1;
                    if d == 0 {
                        end = Some(idx);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(e) = end else { break };
        if text_has_double_gate(&s[..=e]) {
            gated = true;
        }
        s = s[e + 1..].trim_start();
    }
    (gated, s.to_string())
}

/// Whether the struct/enum whose header is `lines[header_idx]` is DIRECTLY attributed with a
/// double-gate — the gate on the header line's OWN leading attribute (`#[cfg(test)] struct Foo {`)
/// or on a contiguous PURE-attribute line immediately above it. This scopes the test-double gate to
/// the item ACTUALLY attributed instead of reading a poisonable header..=end SPAN (Wave-0 fix #2/#3):
/// a braceless-gate leak, a co-located gated variant, or a `cfg(test)` string in a preceding const
/// can no longer mark an un-gated store `in_test`, because only the ATTRIBUTE(s) bound to THIS item
/// are consulted. A line that is an attribute PLUS a co-located item (`#[cfg(test)] Fake,`) is a real
/// code line whose gate belongs to its own item — it ends the walk and is not honored here.
fn directly_double_gated(lines: &[(usize, String)], header_idx: usize) -> bool {
    // Same-line attribute on the header itself: `#[cfg(test)] pub struct Foo {`.
    let (gated, rest) = leading_attr_gate(&lines[header_idx].1);
    if gated && !rest.is_empty() {
        return true;
    }
    // Walk up the contiguous attribute block (skipping blank/comment-only lines), honoring a PURE
    // attribute line's gate and stopping at the first real code line.
    let mut k = header_idx;
    while k > 0 {
        k -= 1;
        let t = lines[k].1.trim_start();
        if t.is_empty() {
            continue; // blank or comment-only line — attributes may still sit above it
        }
        if t.starts_with("#[") {
            let (g, remainder) = leading_attr_gate(&lines[k].1);
            if remainder.is_empty() {
                // A PURE attribute line: it attributes the item below, so honor its gate.
                if g {
                    return true;
                }
                continue; // a stacked non-gate attribute (e.g. `#[derive(..)]`) — keep looking up
            }
            // An attribute WITH a co-located item — its gate binds THAT item, not our header. Stop.
            break;
        }
        break; // a real code line ends the attribute block
    }
    false
}

/// Strip the double-gated VARIANTS out of an enum body, returning the joined payload text of the
/// SURVIVING (un-gated) variants. Variants are split at TOP-LEVEL commas (outside every `()`/`[]`/
/// `{}` payload), so a same-line `#[cfg(test)] Fake, Memory(Arc<Mutex<Inner>>)` strips ONLY the
/// attributed `Fake` and preserves the co-located un-gated `Memory(..)` arm (Wave-0 fix #4); a
/// multi-line or braced gated variant (`#[cfg(test)] Fake { x: u32 }`) is likewise stripped as a
/// single unit without marking the rest of the enum (fix #3). A non-gate leading attribute (e.g.
/// `#[default]`) is preserved with its variant.
fn stripped_enum_payload(body: &str) -> String {
    let mut out = String::new();
    let mut seg = String::new();
    let mut depth: i32 = 0;
    let flush = |seg: &str, out: &mut String| {
        if !seg.trim().is_empty() && !leading_attr_gate(seg).0 {
            out.push_str(seg);
            out.push('\n');
        }
    };
    // Track `()[]{}` depth and detect TOP-LEVEL commas over the STRING-BLANKED body — mirroring the
    // brace-balance scan in [`parse_enum_defs`] (which counts `{}` over `blank_string_literals`). A
    // delimiter INSIDE a string literal (e.g. `#[doc = "("]` or `#[deprecated = "use Pg( instead"]`)
    // must NOT shift depth; otherwise an unbalanced-open string delimiter keeps depth > 0 for the
    // rest of the body, no top-level comma ever splits, and the whole enum collapses into ONE
    // gate-led segment that `flush` drops — carrying the real un-gated `Memory(..)` arm with it, so
    // the delegating `*Store` is falsely ADMITTED. The SEGMENT TEXT is emitted from the RAW `body`
    // (variant contents preserved verbatim for the gate/attr check); only depth/comma boundaries are
    // computed on the blanked form. `blank_string_literals` is char-for-char length-preserving, so
    // the raw and blanked chars zip 1:1.
    let blanked = blank_string_literals(body);
    for (raw, scan) in body.chars().zip(blanked.chars()) {
        match scan {
            '(' | '[' | '{' => {
                depth += 1;
                seg.push(raw);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                seg.push(raw);
            }
            ',' if depth == 0 => {
                flush(&seg, &mut out);
                seg.clear();
            }
            _ => seg.push(raw),
        }
    }
    flush(&seg, &mut out); // the trailing variant (no trailing comma)
    out
}

// ================================================================================================
// Scanner 1 — `no-structural-crypto-in-prod` (census SI-001..SI-004; P-526/527/528 → MR-012).
// ================================================================================================

/// `no-structural-crypto-in-prod` — the `Structural*` mock-crypto verifiers/signers must not be
/// CONSTRUCTED/WIRED in the production graph.
///
/// **Rule.** A `Structural*` credential/token/attestation verifier or token signer
/// (`StructuralVerifier`, `StructuralTokenVerifier`, `StructuralTokenSigner`, and any
/// `Structural*Verifier`/`Structural*Signer` — e.g. `StructuralAttestationVerifier`) is the FLOOR
/// mock crypto: it parses/emits a plaintext pipe-delimited string, so any principal/tenant/grant is
/// forgeable with no signature to defeat (census theme #1, SI-001..004). Such a type may EXIST as a
/// `#[cfg(test)]` double, but it must never be CONSTRUCTED in the production graph. The scanner
/// flags a CONSTRUCTION site — `Structural*Verifier::new(` / `Structural*Signer::new(`, or an
/// `Arc::new(Structural*…)` wiring — that is OUTSIDE `#[cfg(test)]`. The type DEFINITION, its
/// `impl` blocks, and doc-comments do NOT trip it (only construction/wiring).
///
/// **Scope (precise).** Only `Structural*` names ENDING in `Verifier`/`Signer` are crypto roles, so
/// the GDPR `StructuralErasureFloor` / `StructuralLever` / `StructuralFloorReport` (erasure levers,
/// not crypto) are NOT matched. A bare mention, an `impl … for Structural…`, or a static call like
/// `Structural…::provisioned_material(` (test material) is not a `::new(` construction → not flagged.
///
/// **This documents, it does not fix.** The real fix (real OIDC/SAML/WebAuthn/PASETO/biscuit/DPoP +
/// TPM attestation behind the existing `with_verifier`/`with_signer` seams) is MR-010/011/012
/// (P-526/527/528). MR-004 only ships the absence scanner + the committed baseline of the live
/// construction sites so MR-012's removal is provable.
pub const NO_STRUCTURAL_CRYPTO_IN_PROD: LintId = LintId("no-structural-crypto-in-prod");

/// If `code` constructs a `Structural*Verifier`/`Structural*Signer`, return the constructed type
/// name. A construction is `Type::new(` or an `Arc::new(Type…)` wrapping. The DEFINITION/impl/return
/// forms (`-> StructuralVerifier {`, `impl … for StructuralVerifier`, a bare `StructuralVerifier`
/// return) carry the name but NOT a `::new(` call, so they are not matched.
fn structural_crypto_construction(code: &str) -> Option<String> {
    for (i, _) in code.match_indices("Structural") {
        let rest = &code[i..];
        let ident: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !(ident.ends_with("Verifier") || ident.ends_with("Signer")) {
            continue;
        }
        let after = &rest[ident.len()..];
        // Construction indicators:
        //   `StructuralX::new(`            → a direct constructor call
        //   `Arc::new(StructuralX`         → an Arc-wrapped wiring (the prod-default shape)
        let direct_new = after.starts_with("::new(");
        let arc_wrapped = code.contains(&format!("Arc::new({ident}"));
        if direct_new || arc_wrapped {
            return Some(ident);
        }
    }
    None
}

fn scan_no_structural_crypto_in_prod(src: &str) -> Vec<Violation> {
    let test_flags = cfg_test_line_flags(src);
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        if test_flags.get(line).copied().unwrap_or(false) {
            continue; // a `#[cfg(test)]`-gated construction is a TEST double — admitted.
        }
        if let Some(ty) = structural_crypto_construction(&code) {
            out.push(Violation {
                lint: NO_STRUCTURAL_CRYPTO_IN_PROD,
                line,
                reason: format!(
                    "`{ty}` (mock `Structural*` crypto — parses/emits a plaintext pipe-delimited \
                     string, no signature to defeat) is CONSTRUCTED in the production graph — a \
                     forgeable principal/token/attestation. The `Structural*` verifier/signer may \
                     exist ONLY as a `#[cfg(test)]` double; wire a REAL verifier/signer through the \
                     existing `with_verifier`/`with_signer` seam (real OIDC/SAML/WebAuthn/PASETO/ \
                     biscuit/DPoP/TPM — MR-010/011/012, P-526/527/528). Census SI-001..SI-004."
                ),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`NO_STRUCTURAL_CRYPTO_IN_PROD`].
pub fn no_structural_crypto_in_prod() -> Lint {
    Lint {
        id: NO_STRUCTURAL_CRYPTO_IN_PROD,
        rule: "no Structural* mock-crypto verifier/signer constructed in the production graph",
        scan: scan_no_structural_crypto_in_prod,
    }
}

// ================================================================================================
// Scanner 2 — `no-in-memory-durable-store` (census SI-006/007/011/018/019/020; P-522/523 → MR-009).
// ================================================================================================

/// `no-in-memory-durable-store` — a durable-by-contract store/registry/outbox/ledger must not be
/// backed by an in-memory collection with no pool.
///
/// **Rule.** A load-bearing DURABLE store (a `struct` whose name ends in `Store`/`Registry`/
/// `Outbox`/`Ledger`, plus the named KMS key holder `KmsEngine` — census SI-006) backed by an
/// in-memory collection (`Mutex<HashMap`, `Mutex<BTreeMap`, `RwLock<…`, a bare `HashMap`/`BTreeMap`/
/// `HashSet`/`BTreeSet` field, OR a delegation to an in-memory `Inner`-style sibling struct) with NO
/// pool/connection field (`PgPool`, `Pool<…`, `sqlx`, a `pool:` field, `PoolConnection`) loses all
/// state on restart — the census theme-#2 silent-data-loss floor (SI-007/011/018/019/020/etc.).
///
/// **Scope (precise — legitimate caches/indices are NOT flagged).** The role-name suffix keys on the
/// DURABLE role; an obvious in-memory double/cache is excluded by name (`Mem*` prefix, or a name
/// containing `Cache`/`Index`/`Cursor`/`Snapshot`/`Buffer`/`Projection`/`Telemetry`/`Meter`/
/// `Counter`). A `#[cfg(test)]`-gated struct is excluded (test double). The baseline-ratchet gate
/// additionally scopes WHICH crates are scanned to the SPINE/SUBSTRATE (a documented, loud allowlist
/// in `tests/production_graph_absence.rs`) — product-subsystem stores ride their own subsystem
/// tracks (census §B2), not this spine scanner.
///
/// **Detection completeness (false-negatives closed in the MR-004 verifier review + the MR-007
/// enum-indirection review).** The collection fingerprint matches maps AND sequences
/// (`Vec<`/`VecDeque<`), and `type` aliases that expand to a collection are resolved
/// ([`collection_alias_names`]) so an `Arc<Mutex<Alias>>` field is seen (e.g.
/// `PseudonymErasureLedger`'s `type LedgerByPartition = BTreeMap<…>`). It ALSO follows a durable
/// store's `backend` ENUM into its variants ([`parse_enum_defs`] + `in_memory_backend_enums`): a
/// role struct whose field references an enum with a `Memory(Arc<Mutex<Inner>>)`/`Memory(Mutex<Map>)`
/// variant fires, closing the enum-indirection blind spot (the MR-007 relocation — moving the
/// `HashMap` out of a struct field into a `Memory(..)` variant would otherwise make the struct-only
/// scan permanently silent). A pool-only enum (all variants pool-backed, no collection) is admitted.
/// `FsBlobStore` is NOT exempt — it is `Mutex<HashMap<String, Vec<u8>>>` with no `fs::write`, an
/// in-memory store whose byte backing belongs to the Git/backup track (P-ST-30), in the baseline.
///
/// **KNOWN COVERAGE BOUNDARY — the role-suffix blind spot (LOUD, named; NOT a bug).** This scanner
/// keys on the durable-role NAME suffix (`Store`/`Registry`/`Outbox`/`Ledger`) plus a small precise
/// [`NAMED_DURABLE_HOLDERS`] list (`KmsEngine` SI-006, `MisrouteAudit` SI-028).
/// A census persistence shortcut whose struct has NEITHER is INVISIBLE here — the baseline covers the
/// role-suffix representative PER ORGAN, NOT the full census persistence set. Known-uncovered, each
/// riding a not-yet-authored persistence MR that MUST extend this scanner (add the type to
/// [`NAMED_DURABLE_HOLDERS`] or widen [`DURABLE_ROLE_SUFFIXES`]) when it lands:
/// `Consumer` (SI-024), `Firehose` (SI-037), `InMemoryShredder` (SI-038), `OltpPool` (SI-021),
/// `PlacementService` stickiness (SI-026). (`S7Denylist` SI-020 was added to the named list by MR-008
/// and then REMOVED by MR-011, which deleted the stub type and routed the consult through the durable
/// `RevocationStore`.) A future "the bus / control-plane is durable now" claim is
/// therefore only PARTIALLY gated until that MR extends the scanner — this note exists so that gap is
/// never silently relied upon (the full list + per-organ mapping is in `tests/production_graph_absence.rs`).
///
/// **This documents, it does not fix.** The real fix (durable OLTP/cache backings on the default
/// gate + a real migration runner) is MR-007/008/009 (P-522/523) + the events/control-plane
/// durable-persistence MRs the census §B1 calls for.
pub const NO_IN_MEMORY_DURABLE_STORE: LintId = LintId("no-in-memory-durable-store");

/// An in-memory collection fingerprint in a struct field block (bare or lock-wrapped). Includes the
/// SEQUENCE collections (`Vec<`/`VecDeque<`) because a durable-by-contract ledger/outbox/audit-sink
/// is just as in-memory when its rows live in a `Vec` (e.g. `InMemoryPostPitLedger { records: Vec<…> }`,
/// the `MisrouteAudit` `Arc<Mutex<Vec<…>>>` sink) as in a map. The `<` keeps the match a TYPE-position
/// `Vec<…>`/`VecDeque<…>` (never a substring like `Vector`/`MyVec`); the role-suffix gate then keeps a
/// non-durable struct with an incidental `Vec` field out (only Store/Registry/Outbox/Ledger names are
/// scanned).
const COLLECTION_TOKENS: &[&str] = &[
    "HashMap",
    "BTreeMap",
    "HashSet",
    "BTreeSet",
    "Vec<",
    "VecDeque<",
];
/// A pool/connection fingerprint that proves the store is durable-backed (so it is ADMITTED).
const POOL_TOKENS: &[&str] = &[
    "PgPool",
    "Pool<",
    "sqlx",
    "pool:",
    "PoolConnection",
    "PgStore",
    "ColocatedOltp",
];
/// Durable-role name suffixes (the census theme-#2 systems-of-record).
const DURABLE_ROLE_SUFFIXES: &[&str] = &["Store", "Registry", "Outbox", "Ledger"];
/// Named durable holders that carry key/state material but do NOT end in a role suffix. Loud +
/// reviewed (EI-01 §4) — each is a durable-by-contract organ whose NAME the role-suffix rule misses,
/// added precisely (NOT via a broad new suffix, which would false-positive on report/value types):
///   - `KmsEngine` — the in-cell key STORE (census SI-006), named `*Engine`.
///   - `MisrouteAudit` — the misroute AUDIT SINK whose durable tamper-evident log is the named floor
///     (census SI-028; `records: Arc<Mutex<Vec<…>>>`, in-memory today). Named here rather than via an
///     `Audit` suffix, which would wrongly flag report/value types like `CrossCellAudit` (no
///     collection) and `RestrictionLeakAudit` (a computed gate REPORT, not a system-of-record).
///   - (`S7Denylist` SI-020 was a third named holder, added by MR-008; MR-011 DELETED the type — the
///     machine-auth `authenticate` revocation consult now routes through the durable `RevocationStore`
///     (revocation.rs, the `Store`-suffixed durable-capable S7), so there is no stub set to name. The
///     carried-forward machine-token revocation gap is discharged; the `Store` role-suffix rule covers
///     the surviving durable-capable revocation store.)
const NAMED_DURABLE_HOLDERS: &[&str] = &["KmsEngine", "MisrouteAudit"];
/// Name fragments that mark an obvious NON-durable in-memory double/cache/derived view — excluded so
/// a legitimate cache/index/derived-projection/code-registry is not flagged (the precise-scope
/// discipline the rule names: only a durable SYSTEM-OF-RECORD is a shortcut, never a rebuildable
/// read model or a registry of code/functions, nor the census-genuine byte blob store).
///   - `Cache`/`Index`/`Cursor`/`Snapshot`/`Buffer` — caches/indices (rebuildable lookups).
///   - `Derived`/`ReadStore`/`Projection` — CQRS read models rebuilt from the durable event log
///     (e.g. `DerivedStore`, `OlapReadStore`) — derived, not a system-of-record.
///   - `Upcaster` — a registry of pure upcast FUNCTIONS (code registered at boot), not tenant data
///     (e.g. `UpcasterRegistry`).
///   - `Replicated` — a replication/composition WRAPPER over real `BlobStore` backings
///     (`ReplicatedBlobStore<B> { replicas: Vec<B> }`): its `Vec` holds backing STORES, not data of
///     record; durability is the inner backing's. NOT itself an in-memory system-of-record.
///   - `Telemetry`/`Meter`/`Counter` — metric sinks, not data of record.
///
/// NOTE — `FsBlobStore` is DELIBERATELY NOT excluded here (it is brought into the baseline). It was
/// once excluded on a FALSE "byte-durable" premise; in fact `blob.rs:FsBlobStore` is
/// `objects: Mutex<HashMap<String, Vec<u8>>>` and `put()` just inserts — there is NO `fs::write`/
/// `std::fs`, so it IS an in-memory store. Its real on-disk/object-store byte backing is the
/// Git/backup-durability track (P-ST-30; census SI-014/015/029), distinct from the spine identity/
/// events/control-plane persistence this scanner gates — recorded that way in the baseline manifest.
/// (The real `S3BlobStore` holds an `aws_sdk_s3::Client`, no collection → never fires.)
const NON_DURABLE_NAME_FRAGMENTS: &[&str] = &[
    "Cache",
    "Index",
    "Cursor",
    "Snapshot",
    "Buffer",
    "Projection",
    "Derived",
    "ReadStore",
    "Upcaster",
    "Replicated",
    "Telemetry",
    "Meter",
    "Counter",
];

/// A parsed struct: name, the 1-based line of its `struct NAME {` header, the joined field-block
/// text, and whether it is `#[cfg(test)]`-gated.
struct StructDef {
    name: String,
    line: usize,
    fields: String,
    in_test: bool,
}

/// Parse the brace-delimited `struct NAME { … }` definitions out of `src` (tuple/unit structs have
/// no `{ … }` field block and are skipped). Hermetic line/brace tracking in the `code_lines` idiom.
fn parse_struct_defs(src: &str) -> Vec<StructDef> {
    let lines = code_lines(src);
    // Test-support-aware gating: a `#[cfg(feature = "test-support")]` struct is a test double, just
    // like a `#[cfg(test)]` one (MR-009b Wave 0). Used ONLY by the in-memory durable-store scanner.
    let test_flags = cfg_double_line_flags(src);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (lineno, code) = &lines[i];
        let trimmed = code.trim();
        let is_struct_header = (trimmed.starts_with("struct ")
            || trimmed.starts_with("pub struct ")
            || trimmed.contains(" struct "))
            && code.contains('{');
        if !is_struct_header {
            i += 1;
            continue;
        }
        // Extract the struct name: the identifier after the `struct` keyword.
        let after_kw = trimmed
            .split("struct ")
            .nth(1)
            .unwrap_or("")
            .trim_start();
        let name: String = after_kw
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // Collect the field block until the struct's braces balance.
        let mut depth: i32 = 0;
        let mut fields = String::new();
        let mut j = i;
        let mut started = false;
        while j < lines.len() {
            let (_, jcode) = &lines[j];
            for ch in jcode.chars() {
                if ch == '{' {
                    depth += 1;
                    started = true;
                } else if ch == '}' {
                    depth -= 1;
                }
            }
            if j > i {
                fields.push_str(jcode);
                fields.push('\n');
            }
            if started && depth <= 0 {
                break;
            }
            j += 1;
        }
        // Gate the struct WITHOUT reading a poisonable header..=close SPAN (Wave-0 fix #2). Two
        // honest sources: (a) the HEADER line's own flag — true iff the struct sits inside an
        // enclosing `#[cfg(test)] mod { .. }` (whose opening brace already armed the region); or
        // (b) a gate attribute DIRECTLY on this struct (`#[cfg(test)] struct Foo {` or a pure
        // attribute line just above). A directly-attributed struct arms the gate on its OWN opening
        // brace, so its header flag is false — [`directly_double_gated`] recognizes that case by
        // consulting ONLY the attribute(s) bound to THIS struct. Because we never read the body span,
        // a braceless-gate leak / a co-located gated variant / a `cfg(test)` string in a preceding
        // const can no longer mark an un-gated store `in_test`.
        let header_flag = test_flags.get(*lineno).copied().unwrap_or(false);
        let in_test = header_flag || directly_double_gated(&lines, i);
        out.push(StructDef {
            name,
            line: *lineno,
            fields,
            in_test,
        });
        i = j + 1;
    }
    out
}

/// A parsed enum: name, the 1-based line of its `enum NAME {` header, the joined VARIANT-PAYLOAD
/// text (every variant's `(…)`/`{…}` payload + attributes), and whether it is `#[cfg(test)]`-gated.
/// Used to follow a durable store's `backend` ENUM into its variants — the relocation a struct-only
/// scan misses (moving the `Arc<Mutex<HashMap>>` from a struct field into a `Memory(..)` variant).
struct EnumDef {
    name: String,
    fields: String,
    in_test: bool,
}

/// Parse the brace-delimited `enum NAME { … }` definitions out of `src` (the variant block is the
/// scanned payload). Mirrors [`parse_struct_defs`]'s hermetic line/brace tracking so the same
/// collection/alias/`Inner`-delegate detection applies to variant payloads.
fn parse_enum_defs(src: &str) -> Vec<EnumDef> {
    let lines = code_lines(src);
    // Test-support-aware gating (MR-009b Wave 0), matching [`parse_struct_defs`]: a whole enum gated
    // behind `test-support` is a test double, and — crucially — an individual test/feature-gated
    // VARIANT is stripped from the scanned payload below.
    let test_flags = cfg_double_line_flags(src);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (lineno, code) = &lines[i];
        let trimmed = code.trim();
        let is_enum_header = (trimmed.starts_with("enum ")
            || trimmed.starts_with("pub enum ")
            || trimmed.contains(" enum "))
            && code.contains('{');
        if !is_enum_header {
            i += 1;
            continue;
        }
        let after_kw = trimmed.split("enum ").nth(1).unwrap_or("").trim_start();
        let name: String = after_kw
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // Collect the enum's raw text from the header until its braces balance, then extract the
        // body between the enum's opening `{` and its matching `}`.
        let mut depth: i32 = 0;
        let mut started = false;
        let mut whole = String::new();
        let mut j = i;
        while j < lines.len() {
            let (_, jcode) = &lines[j];
            whole.push_str(jcode);
            whole.push('\n');
            for ch in blank_string_literals(jcode).chars() {
                if ch == '{' {
                    depth += 1;
                    started = true;
                } else if ch == '}' {
                    depth -= 1;
                }
            }
            if started && depth <= 0 {
                break;
            }
            j += 1;
        }
        let body = match (whole.find('{'), whole.rfind('}')) {
            (Some(a), Some(b)) if b > a => &whole[a + 1..b],
            _ => "",
        };
        // Variant-level gate stripping (MR-009b Wave 0, Wave-0 fixes #3/#4): strip ONLY the
        // double-gated variants (splitting at TOP-LEVEL commas so a same-line co-located un-gated
        // arm survives, and a braced gated variant is stripped as a single unit without marking the
        // rest of the enum). The un-gated `Memory(..)` arm of a partly-gated enum is thus still seen.
        let fields = stripped_enum_payload(body);
        // Gate the WHOLE enum WITHOUT reading a poisonable span (Wave-0 fix #2/#3): the header flag
        // (enclosing `#[cfg(test)] mod`) OR a gate attribute DIRECTLY on the enum. A single gated
        // VARIANT never marks the whole enum `in_test` — that is the enum-level-span false-admit
        // (probe E): the un-gated `Memory(..)` arm must still be counted.
        let header_flag = test_flags.get(*lineno).copied().unwrap_or(false);
        let in_test = header_flag || directly_double_gated(&lines, i);
        out.push(EnumDef {
            name,
            fields,
            in_test,
        });
        i = j + 1;
    }
    out
}

/// The names of `type` aliases that EXPAND to an in-memory collection — e.g.
/// `type LedgerByPartition = BTreeMap<…>;`. A field typed `Arc<Mutex<LedgerByPartition>>` is then just
/// as in-memory as a literal `Arc<Mutex<BTreeMap<…>>>`, but the literal-token scan can't see it
/// (the collection is hidden behind the alias). Resolving these closes the type-alias false-negative
/// (e.g. `PseudonymErasureLedger`, census 10.8). The RHS may span lines until `;`.
fn collection_alias_names(src: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let lines = code_lines(src);
    let mut i = 0;
    while i < lines.len() {
        let (_, code) = &lines[i];
        let trimmed = code.trim();
        let is_alias = trimmed.starts_with("type ") || trimmed.starts_with("pub type ");
        if !is_alias {
            i += 1;
            continue;
        }
        // The alias name: the identifier after `type` (before `=`/`<`/whitespace).
        let after_kw = trimmed.split("type ").nth(1).unwrap_or("").trim_start();
        let name: String = after_kw
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        // The RHS: from this line, joined across continuation lines until the `;` terminator.
        let mut rhs = code.clone();
        let mut j = i;
        while !rhs.contains(';') && j + 1 < lines.len() {
            j += 1;
            rhs.push(' ');
            rhs.push_str(&lines[j].1);
        }
        if !name.is_empty() && COLLECTION_TOKENS.iter().any(|t| rhs.contains(t)) {
            out.insert(name);
        }
        i = j + 1;
    }
    out
}

fn scan_no_in_memory_durable_store(src: &str) -> Vec<Violation> {
    let structs = parse_struct_defs(src);
    let collection_aliases = collection_alias_names(src);
    // A field block is in-memory-collection-backed if it holds a literal collection OR references a
    // `type` alias that expands to one (the alias false-negative fix).
    let has_collection = |fields: &str| -> bool {
        COLLECTION_TOKENS.iter().any(|t| fields.contains(t))
            || collection_aliases
                .iter()
                .any(|a| field_references_type(fields, a))
    };
    // Pass 1: the set of in-memory BACKING struct names (any struct whose field block holds a
    // collection — literal or alias — and no pool) — captures the `Inner`-style helpers the role
    // structs delegate to. A test/test-support-gated backing (`s.in_test`) is a TEST DOUBLE and must
    // NOT be counted (MR-009b Wave 0): otherwise a durable enum referencing a test-gated
    // `Memory(Inner)` would still (wrongly) fire even though the double is compiled out of production.
    let in_memory_backings: std::collections::BTreeSet<String> = structs
        .iter()
        .filter(|s| {
            let has_pool = POOL_TOKENS.iter().any(|t| s.fields.contains(t));
            !s.in_test && has_collection(&s.fields) && !has_pool
        })
        .map(|s| s.name.clone())
        .collect();

    // Pass 1b: the set of in-memory BACKEND ENUM names — an enum (a store's `backend: SomeEnum`
    // field) is in-memory-capable if ANY of its variant payloads holds an in-memory collection
    // (literal/alias) OR delegates to an in-memory backing struct (the `Memory(Arc<Mutex<Inner>>)`
    // relocation). This closes the enum-indirection blind spot: a `HashMap` moved out of a struct
    // field into a `Memory(..)` variant is just as in-memory, and a future regression that ships the
    // `Memory` variant as the system-of-record must still fire. A pool-only enum (all variants
    // pool-backed, no collection) is NOT in-memory-capable → not added → the role struct is admitted.
    let in_memory_backend_enums: std::collections::BTreeSet<String> = parse_enum_defs(src)
        .iter()
        .filter(|e| !e.in_test)
        .filter(|e| {
            has_collection(&e.fields)
                || in_memory_backings
                    .iter()
                    .any(|b| field_references_type(&e.fields, b))
        })
        .map(|e| e.name.clone())
        .collect();

    let mut out = Vec::new();
    for s in &structs {
        if s.in_test {
            continue; // a `#[cfg(test)]`-gated store is a test double — admitted.
        }
        let is_role = DURABLE_ROLE_SUFFIXES
            .iter()
            .any(|suf| s.name.ends_with(suf))
            || NAMED_DURABLE_HOLDERS.contains(&s.name.as_str());
        if !is_role {
            continue;
        }
        // Precise scope: an obvious non-durable in-memory double/cache is excluded by name.
        if s.name.starts_with("Mem")
            || NON_DURABLE_NAME_FRAGMENTS
                .iter()
                .any(|frag| s.name.contains(frag))
        {
            continue;
        }
        let has_pool = POOL_TOKENS.iter().any(|t| s.fields.contains(t));
        if has_pool {
            continue; // durable-backed — admitted (the green-fixture shape).
        }
        let direct_in_memory = has_collection(&s.fields);
        // Delegation: a field whose type references an in-memory backing sibling (e.g.
        // `inner: Arc<Mutex<Inner>>`), excluding a self-reference.
        let delegated = in_memory_backings
            .iter()
            .filter(|b| **b != s.name)
            .any(|b| field_references_type(&s.fields, b));
        // Enum-backend delegation: a field whose type references an in-memory-capable backend ENUM
        // (e.g. `backend: TupleBackend` where `TupleBackend` has a `Memory(Arc<Mutex<Inner>>)`
        // variant). This catches the relocation a struct-only scan misses.
        let enum_delegated = in_memory_backend_enums
            .iter()
            .any(|e| field_references_type(&s.fields, e));
        if direct_in_memory || delegated || enum_delegated {
            out.push(Violation {
                lint: NO_IN_MEMORY_DURABLE_STORE,
                line: s.line,
                reason: format!(
                    "durable store `{}` is backed by an in-memory collection with NO pool/connection \
                     field — a load-bearing Store/Registry/Outbox/Ledger (or the KMS key holder) is a \
                     system-of-record and must persist to a real pool; an in-memory map loses ALL \
                     state on restart (the census theme-#2 silent-data-loss floor). Back it with a \
                     real durable pool (PgPool/Pool/sqlx) — MR-007/008/009 (P-522/523).",
                    s.name
                ),
            });
        }
    }
    out
}

/// Whether the field block references `ty` as (part of) a field TYPE — a whole-word match so `Inner`
/// matches `Arc<Mutex<Inner>>` but not `InnerThing` / `MyInner`.
fn field_references_type(fields: &str, ty: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = fields[start..].find(ty) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !{
                let c = fields.as_bytes()[abs - 1] as char;
                c.is_ascii_alphanumeric() || c == '_'
            };
        let after = abs + ty.len();
        let after_ok = after >= fields.len()
            || !{
                let c = fields.as_bytes()[after] as char;
                c.is_ascii_alphanumeric() || c == '_'
            };
        if before_ok && after_ok {
            return true;
        }
        start = abs + ty.len();
    }
    false
}

/// The [`Lint`] value for [`NO_IN_MEMORY_DURABLE_STORE`].
pub fn no_in_memory_durable_store() -> Lint {
    Lint {
        id: NO_IN_MEMORY_DURABLE_STORE,
        rule: "no durable store/registry/outbox/ledger backed by an in-memory collection (no pool)",
        scan: scan_no_in_memory_durable_store,
    }
}

// ================================================================================================
// Scanner 3 — `no-bare-tenant-pool` (census SI-005; P-531 → MR-013).
// ================================================================================================

/// `no-bare-tenant-pool` — tenant RLS must not be established session-scoped, and the raw pool must
/// not be handed out unscoped.
///
/// **Rule (two legs).**
///   1. **Session-scoped RLS leak.** A `set_config('myelin.tenant_id'|'myelin.region'|…tenant GUC…,
///      $n, false)` whose 3rd `is_local` arg is `false` sets the GUC for the whole SESSION — on a
///      POOLED connection it leaks across checkouts to the next tenant (cross-tenant bleed). The
///      transaction-local form (`SET LOCAL …` inside a tx, or `set_config(…, true)`) is admitted.
///   2. **Bare raw-pool hatch.** A `fn pool(…) -> &PgPool` / `-> &Pool<…>` accessor hands out the
///      raw connection pool, letting a caller `.acquire()` a connection that bypasses the
///      tenant-scoped RLS accessor. (A bare `.acquire()` is only reachable THROUGH this hatch; the
///      store's own `scoped_conn` helpers acquire-then-scope internally, which is the sanctioned
///      path — so removing the hatch closes the bare-acquire bypass.)
///
/// **The fix landed (MR-013 / P-531).** The PgStore tenant path was transaction-scoped
/// (`set_config(…, true)` via the MR-022 `with_tenant_tx` convention + reset-on-release pool) and the
/// bare `pool() -> &PgPool` hatch was removed (replaced by `health_check()`), so this scanner is now
/// GREEN over the production tree (the two former baseline anchors `pg.rs:413`/`pg.rs:150` were flipped
/// — see `tests/production_graph_absence.rs`). The scanner stays armed: a regression to a
/// session-scoped `set_config(<tenant GUC>, _, false)` or a new `fn pool(…) -> &PgPool` hatch fires
/// LOUDLY. (The mTLS half of the region pin belongs to the runtime transport layer; deferred there.)
pub const NO_BARE_TENANT_POOL: LintId = LintId("no-bare-tenant-pool");

/// Tenant/region GUC names whose session-scoped `set_config` leaks across pooled connections.
const TENANT_GUCS: &[&str] = &["myelin.tenant_id", "myelin.region", "tenant_id", "tenant."];

fn scan_no_bare_tenant_pool(src: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        // ---- Leg 1: session-scoped `set_config(... , false)` on a tenant/region GUC. ----
        // The GUC name lives INSIDE a string literal, so test the RAW code (literals intact); the
        // `, false)` is-local arg is CODE either way.
        if code.contains("set_config(") {
            let mentions_tenant_guc = TENANT_GUCS.iter().any(|g| code.contains(g));
            // The session-scoped fingerprint: an `is_local` 3rd arg of `false`. We match `, false)`
            // / `, false ,` (the per-call closing form) on a `set_config` line that names a tenant
            // GUC. `set_config(..., true)` (transaction-local) is admitted.
            let session_scoped = code.contains(", false)") || code.contains(", false,");
            if mentions_tenant_guc && session_scoped {
                out.push(Violation {
                    lint: NO_BARE_TENANT_POOL,
                    line,
                    reason: "tenant RLS established with session-scoped `set_config(<tenant GUC>, \
                             $n, false)` — `is_local = false` sets the GUC for the WHOLE SESSION, so \
                             on a POOLED connection it leaks across checkouts to the next tenant \
                             (cross-tenant bleed). Use transaction-local scope: `SET LOCAL …` inside \
                             a transaction, or `set_config(…, true)`, with reset-on-release — MR-013 \
                             (P-531). Census SI-005."
                        .into(),
                });
            }
        }
        // ---- Leg 2: a bare raw-pool hatch accessor `fn pool(…) -> &PgPool` / `-> &Pool<…>`. ----
        let blanked = blank_string_literals(&code);
        let trimmed = blanked.trim();
        let is_pool_accessor = (trimmed.contains("fn pool(") || trimmed.contains("fn pool ("))
            && (trimmed.contains("-> &PgPool") || trimmed.contains("-> &Pool<"));
        if is_pool_accessor {
            out.push(Violation {
                lint: NO_BARE_TENANT_POOL,
                line,
                reason: "a bare raw-pool hatch `fn pool(…) -> &PgPool` hands out the unscoped \
                         connection pool — a caller can `.acquire()` a connection that bypasses the \
                         tenant-scoped RLS accessor (the cross-tenant bleed surface). Remove the \
                         hatch; route every acquisition through the tenant-scoped `scoped_conn` \
                         accessor (acquire-then-`SET LOCAL`) — MR-013 (P-531). Census SI-005."
                    .into(),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`NO_BARE_TENANT_POOL`].
pub fn no_bare_tenant_pool() -> Lint {
    Lint {
        id: NO_BARE_TENANT_POOL,
        rule: "no session-scoped tenant RLS (set_config ..,false) and no bare raw-pool hatch",
        scan: scan_no_bare_tenant_pool,
    }
}

// ================================================================================================
// Scanner 4 — `no-permissive-authorizer-in-prod` (R2.6 — the edge action gate is a real policy).
// ================================================================================================

/// `no-permissive-authorizer-in-prod` — the permissive authorizer fixtures (`AllowAll` — the
/// action-level allow-everything seam fixture — and `AllowAllRepos` — the per-repo object-authz
/// allow-everything fixture) must not be CONSTRUCTED in the edge production graph.
///
/// **Rule.** R2.6 replaced the edge's action-level `Arc::new(AllowAll)` with the explicit
/// `AuthenticatedActionPolicy` mounted-action allowlist and gated `AllowAll` behind
/// `#[cfg(any(test, feature = "test-support"))]`; R2.1a replaced the wire's `AllowAllRepos` in the
/// production composition root with the live `CheckBackedRepoAuthorizer`. A permissive fixture may
/// EXIST as a test double, but a CONSTRUCTION site — `Arc::new(AllowAll)` / `Arc::new(AllowAllRepos)`
/// (any path-qualified form, e.g. `Arc::new(crate::AllowAll)`) — outside a `#[cfg(test)]` /
/// `test-support` gate re-opens the every-principal-may-do-everything hole. The scanner is
/// CONSTRUCTION-shaped (the `no_structural_crypto_in_prod` template, NOT the struct-def template):
/// the type definitions, `impl` blocks, doc mentions, and `DenyAll*` fixtures do not trip it.
///
/// **Scope (precise).** The baseline-ratchet gate scopes this scanner to `crates/myelin-edge/`
/// (`tests/production_graph_absence.rs`): the exact-identifier construction match plus the crate
/// scope keeps other crates' UNRELATED same-named fixtures (for different traits) out — a bare
/// `AllowAll` string scan would false-positive there.
pub const NO_PERMISSIVE_AUTHORIZER_IN_PROD: LintId = LintId("no-permissive-authorizer-in-prod");

/// The permissive authorizer fixture type names whose construction is a production violation.
/// EXACT identifier match (never a substring): `DenyAllRepos`, `AllowAllReposX`, etc. do not match.
const PERMISSIVE_AUTHORIZERS: &[&str] = &["AllowAll", "AllowAllRepos"];

/// If `code` constructs a permissive authorizer fixture — `Arc::new(AllowAll)` /
/// `Arc::new(AllowAllRepos)`, with an optional module path (`Arc::new(crate::AllowAll)`) — return
/// the constructed type name. Both are UNIT structs, so the construction shape is the bare
/// identifier immediately closed by `)`; a call like `Arc::new(AllowAllRepos::something())` or a
/// different type sharing the prefix does not match.
fn permissive_authorizer_construction(code: &str) -> Option<String> {
    for (i, _) in code.match_indices("Arc::new(") {
        let mut rest = code[i + "Arc::new(".len()..].trim_start();
        // Peel leading path segments (`crate::`, `myelin_edge::`, `authz::`, …) to the terminal one.
        loop {
            let ident: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if ident.is_empty() {
                break;
            }
            let after = &rest[ident.len()..];
            if let Some(next) = after.strip_prefix("::") {
                rest = next;
                continue;
            }
            // Terminal path segment: the unit-struct construction closes IMMEDIATELY with `)`.
            if PERMISSIVE_AUTHORIZERS.contains(&ident.as_str())
                && after.trim_start().starts_with(')')
            {
                return Some(ident);
            }
            break;
        }
    }
    None
}

fn scan_no_permissive_authorizer_in_prod(src: &str) -> Vec<Violation> {
    // Test-support-aware gating (like the in-memory durable-store scanner): the fixtures are
    // legitimate under `#[cfg(test)]` AND under `#[cfg(any(test, feature = "test-support"))]`
    // (the gate `AllowAll` itself now lives behind).
    let test_flags = cfg_double_line_flags(src);
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        if test_flags.get(line).copied().unwrap_or(false) {
            continue; // a gated construction is a TEST harness fixture — admitted.
        }
        if let Some(ty) = permissive_authorizer_construction(&code) {
            out.push(Violation {
                lint: NO_PERMISSIVE_AUTHORIZER_IN_PROD,
                line,
                reason: format!(
                    "`{ty}` (a permissive allow-everything authorizer fixture) is CONSTRUCTED in \
                     the edge production graph — every authenticated principal would be authorized \
                     for every action/repo. The fixture may exist ONLY behind `#[cfg(test)]` / \
                     `test-support`; production wires the explicit `AuthenticatedActionPolicy` \
                     mounted-action allowlist (action seam, R2.6) / the live \
                     `CheckBackedRepoAuthorizer` (object seam, R2.1a)."
                ),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`NO_PERMISSIVE_AUTHORIZER_IN_PROD`].
pub fn no_permissive_authorizer_in_prod() -> Lint {
    Lint {
        id: NO_PERMISSIVE_AUTHORIZER_IN_PROD,
        rule: "no permissive AllowAll/AllowAllRepos authorizer constructed in the edge production graph",
        scan: scan_no_permissive_authorizer_in_prod,
    }
}

// ================================================================================================
// The four scanners as a set (for the baseline-ratchet gate). NOT part of `all_twelve()`.
// ================================================================================================

/// The four production-graph ABSENCE scanners, in census order (the R2.6 permissive-authorizer
/// scanner appended). These are NOT wired into `all_twelve()` / `workspace_clean` (the ratchet
/// idiom); the baseline-ratchet test (`tests/production_graph_absence.rs`) runs this set against a
/// committed baseline manifest.
pub fn production_graph_absence_scanners() -> Vec<Lint> {
    vec![
        no_structural_crypto_in_prod(),
        no_in_memory_durable_store(),
        no_bare_tenant_pool(),
        no_permissive_authorizer_in_prod(),
    ]
}

/// The stable ids of the four production-graph absence scanners, in census order.
pub const PRODUCTION_GRAPH_ABSENCE_SCANNERS: [LintId; 4] = [
    NO_STRUCTURAL_CRYPTO_IN_PROD,
    NO_IN_MEMORY_DURABLE_STORE,
    NO_BARE_TENANT_POOL,
    NO_PERMISSIVE_AUTHORIZER_IN_PROD,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_crypto_rejects_prod_wiring_admits_cfg_test_and_real() {
        let red = "fn build() {\n    let v = Arc::new(StructuralVerifier::new());\n}";
        let green_test =
            "#[cfg(test)]\nmod tests {\n    fn t() { let v = Arc::new(StructuralVerifier::new()); }\n}";
        let green_real = "fn build() {\n    let v = Arc::new(OidcVerifier::new(jwks));\n}";
        assert!(!no_structural_crypto_in_prod().run(red).is_empty());
        assert!(no_structural_crypto_in_prod().run(green_test).is_empty());
        assert!(no_structural_crypto_in_prod().run(green_real).is_empty());
    }

    #[test]
    fn structural_crypto_ignores_definitions_and_impls() {
        // The type DEFINITION + impl + a bare return must NOT trip the scanner (only construction).
        let def = "pub struct StructuralVerifier;\nimpl StructuralVerifier {\n    pub fn new() -> StructuralVerifier {\n        StructuralVerifier\n    }\n}\nimpl CredentialVerifier for StructuralVerifier {}";
        assert!(
            no_structural_crypto_in_prod().run(def).is_empty(),
            "the Structural* type definition/impl must not be flagged — only construction"
        );
    }

    #[test]
    fn structural_crypto_ignores_non_crypto_structural_names() {
        // `Structural*` names that are NOT verifiers/signers (GDPR erasure levers) are not crypto.
        let erasure = "let f = StructuralErasureFloor::new(engine, region);\nlet l = StructuralLever::all();";
        assert!(no_structural_crypto_in_prod().run(erasure).is_empty());
    }

    #[test]
    fn in_memory_store_rejects_inmemory_admits_pool() {
        let red = "/// system-of-record\npub struct PrincipalStore {\n    inner: Mutex<BTreeMap<Key, Row>>,\n}";
        let green = "pub struct PrincipalStore {\n    pool: PgPool,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
        assert!(no_in_memory_durable_store().run(green).is_empty());
    }

    #[test]
    fn in_memory_store_catches_inner_delegation() {
        // The real shape: the role struct holds `Arc<Mutex<Inner>>` and `Inner` holds the maps.
        let red = "struct Inner {\n    rows: HashMap<EventId, Row>,\n}\npub struct OutboxStore {\n    inner: Arc<Mutex<Inner>>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    #[test]
    fn in_memory_store_ignores_caches_doubles_and_non_role_structs() {
        let cache = "pub struct PrincipalCache {\n    inner: Mutex<BTreeMap<K, V>>,\n}";
        let mem_double = "pub struct MemConversationStore {\n    inner: Mutex<BTreeMap<K, V>>,\n}";
        let non_role = "pub struct Config {\n    inner: BTreeMap<K, V>,\n}";
        assert!(no_in_memory_durable_store().run(cache).is_empty());
        assert!(no_in_memory_durable_store().run(mem_double).is_empty());
        assert!(no_in_memory_durable_store().run(non_role).is_empty());
    }

    #[test]
    fn in_memory_store_ignores_cfg_test_doubles() {
        let test_double = "#[cfg(test)]\nmod tests {\n    struct FakeStore {\n        inner: BTreeMap<K, V>,\n    }\n}";
        assert!(no_in_memory_durable_store().run(test_double).is_empty());
    }

    #[test]
    fn in_memory_store_resolves_type_alias_collection() {
        // The alias false-negative: the collection hides behind `type Alias = BTreeMap<…>`.
        let red = "type LedgerByPartition = BTreeMap<(String, String), Entry>;\npub struct PseudonymErasureLedger {\n    inner: Arc<Mutex<LedgerByPartition>>,\n}";
        assert!(
            !no_in_memory_durable_store().run(red).is_empty(),
            "a *Ledger backed by an Arc<Mutex<Alias>> where Alias is a collection must be caught"
        );
    }

    #[test]
    fn in_memory_store_catches_vec_backed_ledger() {
        // The Vec false-negative: `records: Vec<Record>` on a *Ledger is in-memory just like a map.
        let red = "pub struct InMemoryPostPitLedger {\n    records: Vec<ErasureRecord>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    #[test]
    fn in_memory_store_catches_named_holder_audit_sink() {
        // `MisrouteAudit` (SI-028) — no role suffix, caught precisely via the named-holder list.
        let red = "pub struct MisrouteAudit {\n    records: Arc<Mutex<Vec<MisrouteAuditRecord>>>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    #[test]
    fn s7_denylist_named_holder_discharged_by_mr011() {
        // MR-011 DELETED the `S7Denylist` stub and routed `CapabilityAuthenticator`'s revocation
        // consult through the durable `RevocationStore`. The type no longer exists in the tree, and it
        // is no longer a NAMED_DURABLE_HOLDER, so the scanner no longer flags a `S7Denylist {...}` shape
        // (the `Denylist` suffix is not a generic role suffix). This pins the discharge: the surviving
        // durable-capable revocation store is covered by the `Store` role-suffix rule (revocation.rs),
        // so the carried-forward machine-token revocation gap is closed without a coverage hole.
        let no_longer_named = "pub struct S7Denylist {\n    revoked: Arc<Mutex<BTreeSet<String>>>,\n}";
        assert!(
            no_in_memory_durable_store().run(no_longer_named).is_empty(),
            "S7Denylist is no longer a named durable holder (deleted by MR-011); the durable \
             RevocationStore (Store suffix) is the covered revocation system-of-record"
        );
        // Belt-and-braces: the surviving durable-capable store still fires under the role-suffix rule.
        let store = "struct Inner {\n    mirror: BTreeMap<MirrorKey, RevocationEntry>,\n}\npub enum RevocationBackend {\n    Memory(Arc<Mutex<Inner>>),\n    Pg(PgRevocationBacking),\n}\npub struct RevocationStore {\n    backend: RevocationBackend,\n}";
        assert!(
            !no_in_memory_durable_store().run(store).is_empty(),
            "the durable-capable RevocationStore still fires (Memory default) under the Store suffix"
        );
    }

    #[test]
    fn in_memory_store_follows_backend_enum_memory_variant() {
        // The MR-007 enum-indirection relocation: the in-memory map moved out of a struct field into
        // a `Memory(..)` variant of a backend ENUM. The role struct must STILL fire (a struct-only
        // scan would go permanently silent here). Two shapes: a direct collection in the variant, and
        // the `Inner`-delegate variant the real TupleStore/PrincipalStore use.
        let direct = "pub enum Backend {\n    Memory(std::sync::Mutex<std::collections::HashMap<K, V>>),\n    Pg(PgPool),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(direct).is_empty(),
            "a *Store whose backend enum has a Memory(Mutex<HashMap>) variant must fire"
        );
        let delegate = "struct Inner {\n    partitions: HashMap<K, V>,\n}\npub enum TupleBackend {\n    Memory(Arc<Mutex<Inner>>),\n    Pg(PgTupleBacking),\n}\npub struct TupleStore {\n    backend: TupleBackend,\n}";
        assert!(
            !no_in_memory_durable_store().run(delegate).is_empty(),
            "a *Store whose backend enum delegates to an in-memory Inner via a Memory(..) variant must fire"
        );
    }

    #[test]
    fn in_memory_store_admits_pool_only_backend_enum() {
        // The green counterpart: a backend ENUM whose variants are ALL pool-backed (no in-memory
        // collection variant) is NOT a system-of-record-in-memory — the role struct is admitted.
        let green = "pub enum Backend {\n    Primary(PgPool),\n    Replica(Pool<Postgres>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            no_in_memory_durable_store().run(green).is_empty(),
            "a *Store whose backend enum has only pool-backed variants must be admitted (no false positive)"
        );
    }

    #[test]
    fn in_memory_store_enum_following_no_false_positive_on_value_enums() {
        // A legitimate non-backend enum (a value/status enum) referenced by a *Store must NOT fire as
        // long as it carries no in-memory collection / Inner-delegate variant — the enum-following is
        // scoped to in-memory-CAPABLE enums, not every enum a store mentions.
        let ok = "pub enum Status {\n    Active,\n    Suspended,\n}\npub struct PrincipalStore {\n    status: Status,\n    pool: PgPool,\n}";
        assert!(no_in_memory_durable_store().run(ok).is_empty());
    }

    #[test]
    fn in_memory_store_admits_replication_wrapper_and_real_backed_blob() {
        // A `Replicated*` composition wrapper holds backing STORES, not data of record — admitted.
        let wrapper =
            "pub struct ReplicatedBlobStore<B: BlobStore> {\n    primary: B,\n    replicas: Vec<B>,\n}";
        // A real backed store (an SDK client, no collection) — admitted.
        let real = "pub struct S3BlobStore {\n    client: Client,\n    bucket: String,\n}";
        // A computed gate REPORT with a map is NOT a system-of-record (not a named holder) — admitted.
        let report = "pub struct RestrictionLeakAudit {\n    per_aggregate: BTreeMap<&'static str, u64>,\n}";
        assert!(no_in_memory_durable_store().run(wrapper).is_empty());
        assert!(no_in_memory_durable_store().run(real).is_empty());
        assert!(no_in_memory_durable_store().run(report).is_empty());
    }

    #[test]
    fn in_memory_store_flags_in_memory_blob_store() {
        // `FsBlobStore` is in-memory (HashMap of bytes, no fs::write) — NOT exempt.
        let red = "pub struct FsBlobStore {\n    objects: Mutex<HashMap<String, Vec<u8>>>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    // ---- MR-009b Wave 0: the `test-support` cargo-feature gate is a test-double gate ----------

    #[test]
    fn test_support_gate_admits_direct_in_memory_store() {
        // A `#[cfg(feature = "test-support")]`-gated in-memory *Store is a TEST DOUBLE (downstream
        // crates enable `test-support` as a DEV-dependency) — admitted, exactly like `#[cfg(test)]`.
        let gated = "#[cfg(feature = \"test-support\")]\npub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";
        assert!(
            no_in_memory_durable_store().run(gated).is_empty(),
            "a #[cfg(feature=\"test-support\")]-gated in-memory *Store is a test double — admitted"
        );
    }

    #[test]
    fn test_support_any_gate_admits_direct_in_memory_store() {
        // The `#[cfg(any(test, feature = "test-support"))]` form (the double compiles in unit tests
        // AND when a downstream crate turns on `test-support`) is likewise a test-double gate.
        let gated = "#[cfg(any(test, feature = \"test-support\"))]\npub struct TupleStore {\n    inner: std::sync::Mutex<std::collections::HashMap<String, Row>>,\n}";
        assert!(
            no_in_memory_durable_store().run(gated).is_empty(),
            "a #[cfg(any(test, feature=\"test-support\"))]-gated in-memory *Store is a test double — admitted"
        );
    }

    #[test]
    fn ungated_in_memory_store_still_bites_the_over_broadening_guard() {
        // The GUARD that Wave 0 did NOT over-broaden: an IDENTICAL in-memory *Store with NO gate
        // still FIRES (red). If this ever goes green, the enhancement admitted a real prod store.
        let ungated = "pub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";
        assert!(
            !no_in_memory_durable_store().run(ungated).is_empty(),
            "an UN-gated in-memory *Store must STILL fire — Wave 0 must not over-broaden"
        );
    }

    #[test]
    fn non_test_support_feature_gate_still_bites() {
        // The gate is matched EXACTLY (`feature = "test-support"`): a store hidden behind some OTHER
        // feature (e.g. `feature = "postgres"`) is NOT admitted — that would be a real prod store
        // behind a feature flag, which the scanner must still catch (no broadening to any feature).
        let other = "#[cfg(feature = \"postgres\")]\npub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";
        assert!(
            !no_in_memory_durable_store().run(other).is_empty(),
            "a store behind a NON-test-support feature must still fire (the gate is matched exactly)"
        );
    }

    #[test]
    fn test_support_gated_backend_variant_and_inner_are_admitted() {
        // The Wave 2+ shape: the role struct's backend enum has a `Memory(..)` variant AND its
        // in-memory `Inner` backing BOTH gated behind test-support, with the `Pg` (pool) variant the
        // always-compiled production default. The gated variant is stripped and the gated `Inner` is
        // not counted as a backing → the *Store is ADMITTED (durable-by-default in production).
        let admitted = "#[cfg(any(test, feature = \"test-support\"))]\nstruct Inner {\n    partitions: std::collections::HashMap<String, Row>,\n}\npub enum TupleBackend {\n    #[cfg(any(test, feature = \"test-support\"))]\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgTupleBacking),\n}\npub struct TupleStore {\n    backend: TupleBackend,\n}";
        assert!(
            no_in_memory_durable_store().run(admitted).is_empty(),
            "a *Store whose ONLY in-memory arm (the Memory variant + Inner) is test-support-gated, \
             with a Pg default, is durable-by-default in production — admitted"
        );
        // The over-broadening guard: the SAME shape with the variant UN-gated still fires.
        let bites = "struct Inner {\n    partitions: std::collections::HashMap<String, Row>,\n}\npub enum TupleBackend {\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgTupleBacking),\n}\npub struct TupleStore {\n    backend: TupleBackend,\n}";
        assert!(
            !no_in_memory_durable_store().run(bites).is_empty(),
            "the same backend enum with an UN-gated Memory(Inner) variant must STILL fire"
        );
    }

    // ---- MR-009b Wave 0 — the ADVERSARIAL BATTERY (probes A/B2/E/G/I/J/K) ---------------------
    //
    // An independent verifier proved the Wave-0 enhancement could be TRICKED into silently ADMITTING
    // a real, un-gated production in-memory durable store via a `pending`-leak / poisonable-span
    // defect. Each probe below reproduces one confirmed false-admit; every one MUST now BITE (the
    // real un-gated store still fires). These lock the fix — if any goes green, a real prod store
    // slipped through. The un-gated store shape is `PrincipalStore { inner: Mutex<BTreeMap<..>> }`
    // (a durable role-suffix store on an in-memory map, no pool) — the same shape the green
    // `test-support` fixtures ADMIT only when it is actually gated.

    // The un-gated, must-BITE store body reused across the leak probes.
    const UNGATED_STORE: &str = "pub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";

    #[test]
    fn probe_i_braceless_use_gate_on_own_line_does_not_leak() {
        // Probe I: a braceless `#[cfg(test)]` `use` on its OWN line preceding the un-gated store. The
        // `pending` gate must NOT leak onto the store's opening brace and mark it `in_test`.
        let src = format!(
            "#[cfg(test)]\nuse std::collections::HashMap as TestMap;\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a braceless `#[cfg(test)] use` must gate only itself — the un-gated store must BITE"
        );
    }

    #[test]
    fn probe_g_same_line_braceless_use_gate_does_not_leak() {
        // Probe G: the gate attribute and the braceless `use` on the SAME line before the store.
        let src = format!(
            "#[cfg(test)] use std::collections::HashMap as TestMap;\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a same-line `#[cfg(test)] use ...;` must gate only itself — the store must BITE"
        );
    }

    #[test]
    fn probe_j_braceless_const_and_type_gate_do_not_leak() {
        // Probe J: a braceless gated `const` (and a gated `type`) preceding the store — both `;`-
        // terminated braceless items must not leak the gate onto the store.
        let via_const = format!("#[cfg(test)]\nconst FAKE_MODE: bool = true;\n{UNGATED_STORE}");
        let via_type = format!("#[cfg(test)]\ntype FakeMap = std::collections::HashMap<String, Row>;\n{UNGATED_STORE}");
        assert!(
            !no_in_memory_durable_store().run(&via_const).is_empty(),
            "a braceless gated `const` must not leak — the store must BITE"
        );
        assert!(
            !no_in_memory_durable_store().run(&via_type).is_empty(),
            "a braceless gated `type` must not leak — the store must BITE"
        );
    }

    #[test]
    fn probe_k_unit_variant_gate_does_not_leak_past_enum() {
        // Probe K: a `#[cfg(test)]` UNIT enum variant (no payload, no `;`) immediately preceding the
        // un-gated store. The dangling gate must be dropped when the enum's brace closes, not leak
        // onto the store.
        let src = format!(
            "pub enum Role {{\n    #[cfg(test)]\n    Fake,\n    Real,\n}}\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a gated unit enum variant must not leak past the enum — the store must BITE"
        );
        // Same-line unit-variant form: `#[cfg(test)] Fake,` on one line.
        let same_line = format!(
            "pub enum Role {{\n    #[cfg(test)] Fake,\n    Real,\n}}\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&same_line).is_empty(),
            "a same-line gated unit variant must not leak — the store must BITE"
        );
    }

    #[test]
    fn probe_b2_string_literal_cfg_test_does_not_poison_the_store() {
        // Probe B2: a `const DOC: &str = "cfg(test)";` before the store. The `cfg(test)` substring is
        // string DATA, not an attribute — it must NOT arm the gate or mark the store `in_test`.
        let src = format!("const DOC: &str = \"cfg(test)\";\n{UNGATED_STORE}");
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a `cfg(test)` string literal must not poison the store — it must BITE"
        );
        // The `test-support` string form is likewise inert as DATA.
        let src2 = format!("const DOC: &str = \"feature = \\\"test-support\\\"\";\n{UNGATED_STORE}");
        assert!(
            !no_in_memory_durable_store().run(&src2).is_empty(),
            "a `test-support` string literal must not poison the store — it must BITE"
        );
    }

    #[test]
    fn probe_a_same_line_gated_variant_preserves_co_located_memory_arm() {
        // Probe A: `#[cfg(test)] Fake, Memory(Arc<Mutex<Inner>>),` on ONE line. Stripping the gated
        // `Fake` must NOT drop the co-located UN-gated `Memory(..)` arm — the store must still BITE.
        let src = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)] Fake, Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgBacking),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(src).is_empty(),
            "the un-gated Memory(Inner) arm co-located after a gated Fake must still fire"
        );
    }

    #[test]
    fn probe_e_gated_braced_variant_does_not_drop_the_whole_enum() {
        // Probe E: a gated BRACED struct-variant `#[cfg(test)] Fake { note: String }` alongside an
        // un-gated `Memory(Arc<Mutex<Inner>>)`. The braced gated variant must not mark the WHOLE enum
        // `in_test` (which would drop it and hide the live Memory arm) — the store must still BITE.
        let src = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    Fake { note: String },\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(src).is_empty(),
            "a gated braced variant must strip only itself; the un-gated Memory arm must still fire"
        );
        // The over-broadening guard for probe E: the SAME enum with Fake also un-gated obviously
        // fires, and (the neutrality half) gating the Memory arm too admits it.
        let all_gated = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    Fake { note: String },\n    #[cfg(test)]\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgBacking),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        // `Inner` is un-gated here, so it is still an in-memory backing, but the enum's only arm that
        // references it (`Memory`) is gated → the enum presents no in-memory arm → admitted only if
        // Inner is also not referenced by a live arm. Assert the intended shape: with BOTH in-memory
        // arms gated and a `Pg` default, the store is admitted.
        let admitted = "#[cfg(test)]\nstruct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgBacking),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        let _ = all_gated;
        assert!(
            no_in_memory_durable_store().run(admitted).is_empty(),
            "when the ONLY in-memory arm (Memory + its Inner) is gated with a Pg default, admit it"
        );
    }

    #[test]
    fn probe_unbalanced_string_delimiter_in_attr_does_not_drop_the_enum() {
        // Regression (MR-009b Wave 0, verifier false-admit): an UNBALANCED OPEN delimiter INSIDE a
        // string literal on a gated variant's attribute (`#[doc = "("]`). Before the fix,
        // `stripped_enum_payload` counted `()[]{}` depth over the RAW body, so the `(` inside the doc
        // string pushed depth to 1 for the rest of the body — no top-level comma ever split, the whole
        // enum collapsed into ONE gate-led segment that `flush` dropped, taking the un-gated
        // `Memory(..)` arm with it. The enum then looked non-in-memory and the `*Store` was ADMITTED.
        // The fix tracks depth/top-level commas over `blank_string_literals(body)` (mirroring the
        // brace scan in `parse_enum_defs`), so a string-literal delimiter can't shift depth.
        let doc_open = "pub enum Backend {\n    #[cfg(test)]\n    #[doc = \"(\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Row>>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(doc_open).is_empty(),
            "an unbalanced `(` inside a gated variant's doc string must not drop the un-gated Memory arm — the store must BITE"
        );
        // The `#[deprecated = "use Pg( instead"]` form — same unbalanced open, different attribute.
        let deprecated_open = "pub enum Backend {\n    #[cfg(test)]\n    #[deprecated = \"use Pg( instead\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Row>>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(deprecated_open).is_empty(),
            "an unbalanced `(` inside a gated variant's `#[deprecated = ...]` string must not drop the Memory arm — the store must BITE"
        );
        // The delegate-to-Inner shape: `Memory(Arc<Mutex<Inner>>)` to a separate un-gated `Inner`,
        // with the same unbalanced-open doc string on the gated variant.
        let delegate_open = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    #[doc = \"(\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(delegate_open).is_empty(),
            "an unbalanced `(` in a doc string must not drop a Memory(Inner) delegate arm — the store must BITE"
        );
        // Causation control: the SAME enum with a BALANCED doc paren (`#[doc = \"(x)\"]`) already bit
        // before the fix and must STILL bite — proving the defect was the UNBALANCED open specifically.
        let balanced = "pub enum Backend {\n    #[cfg(test)]\n    #[doc = \"(x)\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Row>>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(balanced).is_empty(),
            "a balanced doc paren must still bite (the control) — the un-gated Memory arm fires"
        );
    }

    #[test]
    fn probe_neutrality_directly_gated_store_and_mod_still_admitted() {
        // Neutrality: the legitimate test-double shapes the fix must KEEP admitting — a directly
        // `#[cfg(test)]`-attributed store, and a store inside a `#[cfg(test)] mod { .. }`.
        let direct = format!("#[cfg(test)]\n{UNGATED_STORE}");
        assert!(
            no_in_memory_durable_store().run(&direct).is_empty(),
            "a directly `#[cfg(test)]`-attributed in-memory store is a test double — admitted"
        );
        let in_mod = "#[cfg(test)]\nmod tests {\n    pub struct PrincipalStore {\n        inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n    }\n}";
        assert!(
            no_in_memory_durable_store().run(in_mod).is_empty(),
            "an in-memory store inside a `#[cfg(test)] mod` is a test double — admitted"
        );
    }

    #[test]
    fn bare_tenant_pool_rejects_session_scope_admits_set_local() {
        let red = "sqlx::query(\"SELECT set_config('myelin.tenant_id', $1, false)\")";
        let green_local = "sqlx::query(\"SET LOCAL myelin.tenant_id = $1\")";
        let green_true = "sqlx::query(\"SELECT set_config('myelin.tenant_id', $1, true)\")";
        assert!(!no_bare_tenant_pool().run(red).is_empty());
        assert!(no_bare_tenant_pool().run(green_local).is_empty());
        assert!(no_bare_tenant_pool().run(green_true).is_empty());
    }

    #[test]
    fn bare_tenant_pool_rejects_raw_pool_hatch() {
        let red = "    pub fn pool(&self) -> &PgPool {\n        &self.pool\n    }";
        assert!(!no_bare_tenant_pool().run(red).is_empty());
    }

    // ---- R2.6: `no-permissive-authorizer-in-prod` (construction-shaped, edge-scoped) ----------

    #[test]
    fn permissive_authorizer_rejects_prod_construction_of_both_fixtures() {
        let red_action = "fn boot() {\n    let gw = Gateway::builder(authn, human_login, Arc::new(AllowAll));\n}";
        let red_repo = "fn rooted() -> Backend {\n    Backend { repo_authz: Arc::new(AllowAllRepos) }\n}";
        let red_qualified = "fn boot() {\n    let a = Arc::new(crate::AllowAll);\n}";
        for (tag, red) in [("AllowAll", red_action), ("AllowAllRepos", red_repo), ("crate::AllowAll", red_qualified)] {
            let v = no_permissive_authorizer_in_prod().run(red);
            assert!(
                !v.is_empty(),
                "an un-gated `Arc::new({tag})` construction must fire"
            );
        }
    }

    #[test]
    fn permissive_authorizer_admits_test_gated_constructions() {
        // `#[cfg(test)] mod` — the harness-fixture shape used across the edge tests.
        let cfg_test = "#[cfg(test)]\nmod tests {\n    fn t() { let g = Gateway::builder(a(), h(), Arc::new(AllowAll)); }\n}";
        // `test-support`-gated (the gate `AllowAll` itself lives behind post-R2.6).
        let ts = "#[cfg(any(test, feature = \"test-support\"))]\nmod harness {\n    fn t() { let g = Gateway::builder(a(), h(), Arc::new(AllowAllRepos)); }\n}";
        assert!(no_permissive_authorizer_in_prod().run(cfg_test).is_empty());
        assert!(no_permissive_authorizer_in_prod().run(ts).is_empty());
    }

    #[test]
    fn permissive_authorizer_admits_the_real_policy_and_non_permissive_fixtures() {
        // The R2.6 production shape — the explicit mounted-action allowlist policy.
        let real = "fn boot() {\n    let gw = Gateway::builder(authn, human_login, Arc::new(AuthenticatedActionPolicy::mounted()));\n}";
        // The fail-closed fixtures are NOT permissive — never flagged.
        let deny = "fn t() {\n    let d = Arc::new(DenyAllRepos);\n    let d2 = Arc::new(DenyAll);\n}";
        // The type DEFINITION / a doc mention / an impl is not a construction.
        let def = "/// like `Arc::new(AllowAll)` in tests\npub struct AllowAll;\nimpl Authorizer for AllowAll {\n    fn authorize(&self) -> bool { true }\n}";
        // A method call on the type is not the unit-struct construction shape.
        let call = "fn t() {\n    let x = Arc::new(AllowAllRepos::with_flags(f));\n}";
        assert!(no_permissive_authorizer_in_prod().run(real).is_empty());
        assert!(no_permissive_authorizer_in_prod().run(deny).is_empty());
        assert!(
            no_permissive_authorizer_in_prod().run(def).is_empty(),
            "definitions/impl/doc mentions must not be flagged — construction only \
             (the doc-comment `Arc::new(AllowAll)` is stripped by code_lines)"
        );
        assert!(no_permissive_authorizer_in_prod().run(call).is_empty());
    }

    #[test]
    fn the_four_scanners_have_distinct_ids() {
        let ids: Vec<LintId> = production_graph_absence_scanners()
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(ids, PRODUCTION_GRAPH_ABSENCE_SCANNERS.to_vec());
        let mut s: Vec<&str> = ids.iter().map(|i| i.0).collect();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), 4);
    }
}
