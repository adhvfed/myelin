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
//! The three scanners:
//! 1. [`no_structural_crypto_in_prod`] — a `Structural*` mock-crypto verifier/signer CONSTRUCTED in
//!    the production graph (outside `#[cfg(test)]`). Census SI-001..SI-004 (P-526/527/528, MR-012).
//! 2. [`no_in_memory_durable_store`] — a durable-by-contract store/registry/outbox/ledger backed by
//!    an in-memory collection with no pool field. Census SI-006/007/011/018/019/020 (P-522/523,
//!    MR-009).
//! 3. [`no_bare_tenant_pool`] — session-scoped `set_config(..., false)` RLS (leaks across pooled
//!    connections) + the bare raw-pool hatch. Census SI-005 (P-531, MR-013).

use crate::engine::{blank_string_literals, code_lines, Lint, LintId, Violation};

// ================================================================================================
// Shared helper: `#[cfg(test)]` region detection.
//
// The three scanners target CONSTRUCTION/WIRING in the PRODUCTION graph — a `Structural*` verifier
// or an in-memory store wired under `#[cfg(test)]` is a TEST double and must NOT be flagged (the
// green fixtures prove `#[cfg(test)]`-gated wiring is admitted). `code_lines` keeps `#[cfg(test)]`
// (it is code, not a comment), so we can track which lines sit inside a `#[cfg(test)]`-gated item.
// ================================================================================================

/// For each 1-based line of `src`, whether it sits inside a `#[cfg(test)]`-gated block (a test
/// module or a test fn). A `#[cfg(test)]` attribute arms the NEXT block it opens; the region ends
/// when that block's braces close. Conservative + hermetic (pure fn of source text).
fn cfg_test_line_flags(src: &str) -> Vec<bool> {
    let lines = code_lines(src);
    let max_line = lines.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut flags = vec![false; max_line + 1];
    let mut depth: i32 = 0;
    // The brace depths at which an ACTIVE `#[cfg(test)]` block opened (a stack so nested test items
    // are handled). While the stack is non-empty we are inside test-gated code.
    let mut stack: Vec<i32> = Vec::new();
    let mut pending = false; // a `#[cfg(test)]` attribute seen, awaiting its opening brace.
    for (lineno, code) in &lines {
        // `#[cfg(test)]` (and `#[cfg(all(test, ...))]`) arm the next opening block.
        if code.contains("cfg(test)") {
            pending = true;
        }
        // The line's test status is decided BEFORE this line's own braces take effect.
        if *lineno < flags.len() {
            flags[*lineno] = !stack.is_empty();
        }
        for ch in code.chars() {
            match ch {
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
                }
                _ => {}
            }
        }
    }
    flags
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
    let test_flags = cfg_test_line_flags(src);
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
        out.push(StructDef {
            name,
            line: *lineno,
            fields,
            in_test: test_flags.get(*lineno).copied().unwrap_or(false),
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
    let test_flags = cfg_test_line_flags(src);
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
        out.push(EnumDef {
            name,
            fields,
            in_test: test_flags.get(*lineno).copied().unwrap_or(false),
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
    // structs delegate to.
    let in_memory_backings: std::collections::BTreeSet<String> = structs
        .iter()
        .filter(|s| {
            let has_pool = POOL_TOKENS.iter().any(|t| s.fields.contains(t));
            has_collection(&s.fields) && !has_pool
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
/// **This documents, it does not fix.** The real fix (`SET LOCAL` / `set_config(…, true)` +
/// reset-on-release + removing the bare `pool()` hatch + identifier validation + mTLS) is MR-013
/// (P-531). MR-004 only ships the absence scanner + the committed baseline so MR-013's fix is
/// provable. It MUST flag `crates/myelin-storage/src/pg.rs:413` (the `set_config(…, false)` line).
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
// The three scanners as a set (for the baseline-ratchet gate). NOT part of `all_twelve()`.
// ================================================================================================

/// The three production-graph ABSENCE scanners, in census order. These are NOT wired into
/// `all_twelve()` / `workspace_clean` (the real tree violates all three by design); the
/// baseline-ratchet test (`tests/production_graph_absence.rs`) runs this set against a committed
/// baseline manifest.
pub fn production_graph_absence_scanners() -> Vec<Lint> {
    vec![
        no_structural_crypto_in_prod(),
        no_in_memory_durable_store(),
        no_bare_tenant_pool(),
    ]
}

/// The stable ids of the three production-graph absence scanners, in census order.
pub const PRODUCTION_GRAPH_ABSENCE_SCANNERS: [LintId; 3] = [
    NO_STRUCTURAL_CRYPTO_IN_PROD,
    NO_IN_MEMORY_DURABLE_STORE,
    NO_BARE_TENANT_POOL,
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

    #[test]
    fn the_three_scanners_have_distinct_ids() {
        let ids: Vec<LintId> = production_graph_absence_scanners()
            .iter()
            .map(|l| l.id)
            .collect();
        assert_eq!(ids, PRODUCTION_GRAPH_ABSENCE_SCANNERS.to_vec());
        let mut s: Vec<&str> = ids.iter().map(|i| i.0).collect();
        s.sort_unstable();
        s.dedup();
        assert_eq!(s.len(), 3);
    }
}
