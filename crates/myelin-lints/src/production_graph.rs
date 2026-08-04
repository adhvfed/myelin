use crate::engine::{blank_string_literals, code_lines, Lint, LintId, Violation};

fn cfg_line_flags(src: &str, is_gate: impl Fn(&str) -> bool) -> Vec<bool> {
    let lines = code_lines(src);
    let max_line = lines.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let mut flags = vec![false; max_line + 1];
    let mut depth: i32 = 0;
    let mut stack: Vec<i32> = Vec::new();
    let mut pending = false;
    let mut pending_depth: i32 = 0;
    let mut nest: i32 = 0;
    for (lineno, code) in &lines {
        if code.trim_start().starts_with("#[") && is_gate(code) {
            if !pending {
                pending_depth = depth;
            }
            pending = true;
        }
        if *lineno < flags.len() {
            flags[*lineno] = !stack.is_empty();
        }
        for ch in blank_string_literals(code).chars() {
            match ch {
                '(' | '[' => nest += 1,
                ')' | ']' if nest > 0 => nest -= 1,
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
                    if pending && depth < pending_depth {
                        pending = false;
                    }
                }
                ';' if pending && nest == 0 => {
                    pending = false;
                }
                _ => {}
            }
        }
    }
    flags
}

const TEST_SUPPORT_GATE: &str = "feature = \"test-support\"";

fn cfg_test_line_flags(src: &str) -> Vec<bool> {
    cfg_line_flags(src, |code| code.contains("cfg(test)"))
}

fn cfg_double_line_flags(src: &str) -> Vec<bool> {
    cfg_line_flags(src, |code| {
        code.contains("cfg(test)") || code.contains(TEST_SUPPORT_GATE)
    })
}

fn text_has_double_gate(text: &str) -> bool {
    text.contains("cfg(test)") || text.contains(TEST_SUPPORT_GATE)
}

fn leading_attr_gate(line: &str) -> (bool, String) {
    let mut s = line.trim_start();
    let mut gated = false;
    while s.starts_with("#[") {
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

fn directly_double_gated(lines: &[(usize, String)], header_idx: usize) -> bool {
    let (gated, rest) = leading_attr_gate(&lines[header_idx].1);
    if gated && !rest.is_empty() {
        return true;
    }
    let mut k = header_idx;
    while k > 0 {
        k -= 1;
        let t = lines[k].1.trim_start();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("#[") {
            let (g, remainder) = leading_attr_gate(&lines[k].1);
            if remainder.is_empty() {
                if g {
                    return true;
                }
                continue;
            }
            break;
        }
        break;
    }
    false
}

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
    flush(&seg, &mut out);
    out
}

pub const NO_STRUCTURAL_CRYPTO_IN_PROD: LintId = LintId("no-structural-crypto-in-prod");

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
            continue;
        }
        if let Some(ty) = structural_crypto_construction(&code) {
            out.push(Violation {
                lint: NO_STRUCTURAL_CRYPTO_IN_PROD,
                line,
                reason: format!(
                    "`{ty}` (mock `Structural*` crypto - parses/emits a plaintext pipe-delimited \
                     string, no signature to defeat) is CONSTRUCTED in the production graph - a \
                     forgeable principal/token/attestation. The `Structural*` verifier/signer may \
                     exist ONLY as a `#[cfg(test)]` double; wire a REAL verifier/signer through the \
                     existing `with_verifier`/`with_signer` seam (real OIDC/SAML/WebAuthn/PASETO/ \
                     biscuit/DPoP/TPM - MR-010/011/012, P-526/527/528). Census SI-001..SI-004."
                ),
            });
        }
    }
    out
}

pub fn no_structural_crypto_in_prod() -> Lint {
    Lint {
        id: NO_STRUCTURAL_CRYPTO_IN_PROD,
        rule: "no Structural* mock-crypto verifier/signer constructed in the production graph",
        scan: scan_no_structural_crypto_in_prod,
    }
}

pub const NO_IN_MEMORY_DURABLE_STORE: LintId = LintId("no-in-memory-durable-store");

const COLLECTION_TOKENS: &[&str] = &[
    "HashMap",
    "BTreeMap",
    "HashSet",
    "BTreeSet",
    "Vec<",
    "VecDeque<",
];
const POOL_TOKENS: &[&str] = &[
    "PgPool",
    "Pool<",
    "sqlx",
    "pool:",
    "PoolConnection",
    "PgStore",
    "ColocatedOltp",
];
const DURABLE_ROLE_SUFFIXES: &[&str] = &["Store", "Registry", "Outbox", "Ledger"];
const NAMED_DURABLE_HOLDERS: &[&str] = &["KmsEngine", "MisrouteAudit"];
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

struct StructDef {
    name: String,
    line: usize,
    fields: String,
    in_test: bool,
}

fn parse_struct_defs(src: &str) -> Vec<StructDef> {
    let lines = code_lines(src);
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
        let after_kw = trimmed
            .split("struct ")
            .nth(1)
            .unwrap_or("")
            .trim_start();
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

struct EnumDef {
    name: String,
    fields: String,
    in_test: bool,
}

fn parse_enum_defs(src: &str) -> Vec<EnumDef> {
    let lines = code_lines(src);
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
        let fields = stripped_enum_payload(body);
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
        let after_kw = trimmed.split("type ").nth(1).unwrap_or("").trim_start();
        let name: String = after_kw
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
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
    let has_collection = |fields: &str| -> bool {
        COLLECTION_TOKENS.iter().any(|t| fields.contains(t))
            || collection_aliases
                .iter()
                .any(|a| field_references_type(fields, a))
    };
    let in_memory_backings: std::collections::BTreeSet<String> = structs
        .iter()
        .filter(|s| {
            let has_pool = POOL_TOKENS.iter().any(|t| s.fields.contains(t));
            !s.in_test && has_collection(&s.fields) && !has_pool
        })
        .map(|s| s.name.clone())
        .collect();

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
            continue;
        }
        let is_role = DURABLE_ROLE_SUFFIXES
            .iter()
            .any(|suf| s.name.ends_with(suf))
            || NAMED_DURABLE_HOLDERS.contains(&s.name.as_str());
        if !is_role {
            continue;
        }
        if s.name.starts_with("Mem")
            || NON_DURABLE_NAME_FRAGMENTS
                .iter()
                .any(|frag| s.name.contains(frag))
        {
            continue;
        }
        let has_pool = POOL_TOKENS.iter().any(|t| s.fields.contains(t));
        if has_pool {
            continue;
        }
        let direct_in_memory = has_collection(&s.fields);
        let delegated = in_memory_backings
            .iter()
            .filter(|b| **b != s.name)
            .any(|b| field_references_type(&s.fields, b));
        let enum_delegated = in_memory_backend_enums
            .iter()
            .any(|e| field_references_type(&s.fields, e));
        if direct_in_memory || delegated || enum_delegated {
            out.push(Violation {
                lint: NO_IN_MEMORY_DURABLE_STORE,
                line: s.line,
                reason: format!(
                    "durable store `{}` is backed by an in-memory collection with NO pool/connection \
                     field - a load-bearing Store/Registry/Outbox/Ledger (or the KMS key holder) is a \
                     system-of-record and must persist to a real pool; an in-memory map loses ALL \
                     state on restart (the census theme-#2 silent-data-loss floor). Back it with a \
                     real durable pool (PgPool/Pool/sqlx) - MR-007/008/009 (P-522/523).",
                    s.name
                ),
            });
        }
    }
    out
}

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

pub fn no_in_memory_durable_store() -> Lint {
    Lint {
        id: NO_IN_MEMORY_DURABLE_STORE,
        rule: "no durable store/registry/outbox/ledger backed by an in-memory collection (no pool)",
        scan: scan_no_in_memory_durable_store,
    }
}

pub const NO_BARE_TENANT_POOL: LintId = LintId("no-bare-tenant-pool");

const TENANT_GUCS: &[&str] = &["myelin.tenant_id", "myelin.region", "tenant_id", "tenant."];

fn scan_no_bare_tenant_pool(src: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        if code.contains("set_config(") {
            let mentions_tenant_guc = TENANT_GUCS.iter().any(|g| code.contains(g));
            let session_scoped = code.contains(", false)") || code.contains(", false,");
            if mentions_tenant_guc && session_scoped {
                out.push(Violation {
                    lint: NO_BARE_TENANT_POOL,
                    line,
                    reason: "tenant RLS established with session-scoped `set_config(<tenant GUC>, \
                             $n, false)` - `is_local = false` sets the GUC for the WHOLE SESSION, so \
                             on a POOLED connection it leaks across checkouts to the next tenant \
                             (cross-tenant bleed). Use transaction-local scope: `SET LOCAL …` inside \
                             a transaction, or `set_config(…, true)`, with reset-on-release - MR-013 \
                             (P-531). Census SI-005."
                        .into(),
                });
            }
        }
        let blanked = blank_string_literals(&code);
        let trimmed = blanked.trim();
        let is_pool_accessor = (trimmed.contains("fn pool(") || trimmed.contains("fn pool ("))
            && (trimmed.contains("-> &PgPool") || trimmed.contains("-> &Pool<"));
        if is_pool_accessor {
            out.push(Violation {
                lint: NO_BARE_TENANT_POOL,
                line,
                reason: "a bare raw-pool hatch `fn pool(…) -> &PgPool` hands out the unscoped \
                         connection pool - a caller can `.acquire()` a connection that bypasses the \
                         tenant-scoped RLS accessor (the cross-tenant bleed surface). Remove the \
                         hatch; route every acquisition through the tenant-scoped `scoped_conn` \
                         accessor (acquire-then-`SET LOCAL`) - MR-013 (P-531). Census SI-005."
                    .into(),
            });
        }
    }
    out
}

pub fn no_bare_tenant_pool() -> Lint {
    Lint {
        id: NO_BARE_TENANT_POOL,
        rule: "no session-scoped tenant RLS (set_config ..,false) and no bare raw-pool hatch",
        scan: scan_no_bare_tenant_pool,
    }
}

pub const NO_PERMISSIVE_AUTHORIZER_IN_PROD: LintId = LintId("no-permissive-authorizer-in-prod");

const PERMISSIVE_AUTHORIZERS: &[&str] = &["AllowAll", "AllowAllRepos"];

fn permissive_authorizer_construction(code: &str) -> Option<String> {
    for (i, _) in code.match_indices("Arc::new(") {
        let mut rest = code[i + "Arc::new(".len()..].trim_start();
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
    let test_flags = cfg_double_line_flags(src);
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        if test_flags.get(line).copied().unwrap_or(false) {
            continue;
        }
        if let Some(ty) = permissive_authorizer_construction(&code) {
            out.push(Violation {
                lint: NO_PERMISSIVE_AUTHORIZER_IN_PROD,
                line,
                reason: format!(
                    "`{ty}` (a permissive allow-everything authorizer fixture) is CONSTRUCTED in \
                     the edge production graph - every authenticated principal would be authorized \
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

pub fn no_permissive_authorizer_in_prod() -> Lint {
    Lint {
        id: NO_PERMISSIVE_AUTHORIZER_IN_PROD,
        rule: "no permissive AllowAll/AllowAllRepos authorizer constructed in the edge production graph",
        scan: scan_no_permissive_authorizer_in_prod,
    }
}

pub fn production_graph_absence_scanners() -> Vec<Lint> {
    vec![
        no_structural_crypto_in_prod(),
        no_in_memory_durable_store(),
        no_bare_tenant_pool(),
        no_permissive_authorizer_in_prod(),
    ]
}

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
        let def = "pub struct StructuralVerifier;\nimpl StructuralVerifier {\n    pub fn new() -> StructuralVerifier {\n        StructuralVerifier\n    }\n}\nimpl CredentialVerifier for StructuralVerifier {}";
        assert!(
            no_structural_crypto_in_prod().run(def).is_empty(),
            "the Structural* type definition/impl must not be flagged - only construction"
        );
    }

    #[test]
    fn structural_crypto_ignores_non_crypto_structural_names() {
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
        let red = "type LedgerByPartition = BTreeMap<(String, String), Entry>;\npub struct PseudonymErasureLedger {\n    inner: Arc<Mutex<LedgerByPartition>>,\n}";
        assert!(
            !no_in_memory_durable_store().run(red).is_empty(),
            "a *Ledger backed by an Arc<Mutex<Alias>> where Alias is a collection must be caught"
        );
    }

    #[test]
    fn in_memory_store_catches_vec_backed_ledger() {
        let red = "pub struct InMemoryPostPitLedger {\n    records: Vec<ErasureRecord>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    #[test]
    fn in_memory_store_catches_named_holder_audit_sink() {
        let red = "pub struct MisrouteAudit {\n    records: Arc<Mutex<Vec<MisrouteAuditRecord>>>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    #[test]
    fn s7_denylist_named_holder_discharged_by_mr011() {
        let no_longer_named = "pub struct S7Denylist {\n    revoked: Arc<Mutex<BTreeSet<String>>>,\n}";
        assert!(
            no_in_memory_durable_store().run(no_longer_named).is_empty(),
            "S7Denylist is no longer a named durable holder (deleted by MR-011); the durable \
             RevocationStore (Store suffix) is the covered revocation system-of-record"
        );
        let store = "struct Inner {\n    mirror: BTreeMap<MirrorKey, RevocationEntry>,\n}\npub enum RevocationBackend {\n    Memory(Arc<Mutex<Inner>>),\n    Pg(PgRevocationBacking),\n}\npub struct RevocationStore {\n    backend: RevocationBackend,\n}";
        assert!(
            !no_in_memory_durable_store().run(store).is_empty(),
            "the durable-capable RevocationStore still fires (Memory default) under the Store suffix"
        );
    }

    #[test]
    fn in_memory_store_follows_backend_enum_memory_variant() {
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
        let green = "pub enum Backend {\n    Primary(PgPool),\n    Replica(Pool<Postgres>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            no_in_memory_durable_store().run(green).is_empty(),
            "a *Store whose backend enum has only pool-backed variants must be admitted (no false positive)"
        );
    }

    #[test]
    fn in_memory_store_enum_following_no_false_positive_on_value_enums() {
        let ok = "pub enum Status {\n    Active,\n    Suspended,\n}\npub struct PrincipalStore {\n    status: Status,\n    pool: PgPool,\n}";
        assert!(no_in_memory_durable_store().run(ok).is_empty());
    }

    #[test]
    fn in_memory_store_admits_replication_wrapper_and_real_backed_blob() {
        let wrapper =
            "pub struct ReplicatedBlobStore<B: BlobStore> {\n    primary: B,\n    replicas: Vec<B>,\n}";
        let real = "pub struct S3BlobStore {\n    client: Client,\n    bucket: String,\n}";
        let report = "pub struct RestrictionLeakAudit {\n    per_aggregate: BTreeMap<&'static str, u64>,\n}";
        assert!(no_in_memory_durable_store().run(wrapper).is_empty());
        assert!(no_in_memory_durable_store().run(real).is_empty());
        assert!(no_in_memory_durable_store().run(report).is_empty());
    }

    #[test]
    fn in_memory_store_flags_in_memory_blob_store() {
        let red = "pub struct FsBlobStore {\n    objects: Mutex<HashMap<String, Vec<u8>>>,\n}";
        assert!(!no_in_memory_durable_store().run(red).is_empty());
    }

    #[test]
    fn test_support_gate_admits_direct_in_memory_store() {
        let gated = "#[cfg(feature = \"test-support\")]\npub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";
        assert!(
            no_in_memory_durable_store().run(gated).is_empty(),
            "a #[cfg(feature=\"test-support\")]-gated in-memory *Store is a test double - admitted"
        );
    }

    #[test]
    fn test_support_any_gate_admits_direct_in_memory_store() {
        let gated = "#[cfg(any(test, feature = \"test-support\"))]\npub struct TupleStore {\n    inner: std::sync::Mutex<std::collections::HashMap<String, Row>>,\n}";
        assert!(
            no_in_memory_durable_store().run(gated).is_empty(),
            "a #[cfg(any(test, feature=\"test-support\"))]-gated in-memory *Store is a test double - admitted"
        );
    }

    #[test]
    fn ungated_in_memory_store_still_bites_the_over_broadening_guard() {
        let ungated = "pub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";
        assert!(
            !no_in_memory_durable_store().run(ungated).is_empty(),
            "an UN-gated in-memory *Store must STILL fire - Wave 0 must not over-broaden"
        );
    }

    #[test]
    fn non_test_support_feature_gate_still_bites() {
        let other = "#[cfg(feature = \"postgres\")]\npub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";
        assert!(
            !no_in_memory_durable_store().run(other).is_empty(),
            "a store behind a NON-test-support feature must still fire (the gate is matched exactly)"
        );
    }

    #[test]
    fn test_support_gated_backend_variant_and_inner_are_admitted() {
        let admitted = "#[cfg(any(test, feature = \"test-support\"))]\nstruct Inner {\n    partitions: std::collections::HashMap<String, Row>,\n}\npub enum TupleBackend {\n    #[cfg(any(test, feature = \"test-support\"))]\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgTupleBacking),\n}\npub struct TupleStore {\n    backend: TupleBackend,\n}";
        assert!(
            no_in_memory_durable_store().run(admitted).is_empty(),
            "a *Store whose ONLY in-memory arm (the Memory variant + Inner) is test-support-gated, \
             with a Pg default, is durable-by-default in production - admitted"
        );
        let bites = "struct Inner {\n    partitions: std::collections::HashMap<String, Row>,\n}\npub enum TupleBackend {\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgTupleBacking),\n}\npub struct TupleStore {\n    backend: TupleBackend,\n}";
        assert!(
            !no_in_memory_durable_store().run(bites).is_empty(),
            "the same backend enum with an UN-gated Memory(Inner) variant must STILL fire"
        );
    }

    const UNGATED_STORE: &str = "pub struct PrincipalStore {\n    inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n}";

    #[test]
    fn probe_i_braceless_use_gate_on_own_line_does_not_leak() {
        let src = format!(
            "#[cfg(test)]\nuse std::collections::HashMap as TestMap;\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a braceless `#[cfg(test)] use` must gate only itself - the un-gated store must BITE"
        );
    }

    #[test]
    fn probe_g_same_line_braceless_use_gate_does_not_leak() {
        let src = format!(
            "#[cfg(test)] use std::collections::HashMap as TestMap;\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a same-line `#[cfg(test)] use ...;` must gate only itself - the store must BITE"
        );
    }

    #[test]
    fn probe_j_braceless_const_and_type_gate_do_not_leak() {
        let via_const = format!("#[cfg(test)]\nconst FAKE_MODE: bool = true;\n{UNGATED_STORE}");
        let via_type = format!("#[cfg(test)]\ntype FakeMap = std::collections::HashMap<String, Row>;\n{UNGATED_STORE}");
        assert!(
            !no_in_memory_durable_store().run(&via_const).is_empty(),
            "a braceless gated `const` must not leak - the store must BITE"
        );
        assert!(
            !no_in_memory_durable_store().run(&via_type).is_empty(),
            "a braceless gated `type` must not leak - the store must BITE"
        );
    }

    #[test]
    fn probe_k_unit_variant_gate_does_not_leak_past_enum() {
        let src = format!(
            "pub enum Role {{\n    #[cfg(test)]\n    Fake,\n    Real,\n}}\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a gated unit enum variant must not leak past the enum - the store must BITE"
        );
        let same_line = format!(
            "pub enum Role {{\n    #[cfg(test)] Fake,\n    Real,\n}}\n{UNGATED_STORE}"
        );
        assert!(
            !no_in_memory_durable_store().run(&same_line).is_empty(),
            "a same-line gated unit variant must not leak - the store must BITE"
        );
    }

    #[test]
    fn probe_b2_string_literal_cfg_test_does_not_poison_the_store() {
        let src = format!("const DOC: &str = \"cfg(test)\";\n{UNGATED_STORE}");
        assert!(
            !no_in_memory_durable_store().run(&src).is_empty(),
            "a `cfg(test)` string literal must not poison the store - it must BITE"
        );
        let src2 = format!("const DOC: &str = \"feature = \\\"test-support\\\"\";\n{UNGATED_STORE}");
        assert!(
            !no_in_memory_durable_store().run(&src2).is_empty(),
            "a `test-support` string literal must not poison the store - it must BITE"
        );
    }

    #[test]
    fn probe_a_same_line_gated_variant_preserves_co_located_memory_arm() {
        let src = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)] Fake, Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgBacking),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(src).is_empty(),
            "the un-gated Memory(Inner) arm co-located after a gated Fake must still fire"
        );
    }

    #[test]
    fn probe_e_gated_braced_variant_does_not_drop_the_whole_enum() {
        let src = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    Fake { note: String },\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(src).is_empty(),
            "a gated braced variant must strip only itself; the un-gated Memory arm must still fire"
        );
        let all_gated = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    Fake { note: String },\n    #[cfg(test)]\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgBacking),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        let admitted = "#[cfg(test)]\nstruct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n    Pg(PgBacking),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        let _ = all_gated;
        assert!(
            no_in_memory_durable_store().run(admitted).is_empty(),
            "when the ONLY in-memory arm (Memory + its Inner) is gated with a Pg default, admit it"
        );
    }

    #[test]
    fn probe_unbalanced_string_delimiter_in_attr_does_not_drop_the_enum() {
        let doc_open = "pub enum Backend {\n    #[cfg(test)]\n    #[doc = \"(\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Row>>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(doc_open).is_empty(),
            "an unbalanced `(` inside a gated variant's doc string must not drop the un-gated Memory arm - the store must BITE"
        );
        let deprecated_open = "pub enum Backend {\n    #[cfg(test)]\n    #[deprecated = \"use Pg( instead\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Row>>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(deprecated_open).is_empty(),
            "an unbalanced `(` inside a gated variant's `#[deprecated = ...]` string must not drop the Memory arm - the store must BITE"
        );
        let delegate_open = "struct Inner {\n    rows: std::collections::HashMap<String, Row>,\n}\npub enum Backend {\n    #[cfg(test)]\n    #[doc = \"(\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<Inner>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(delegate_open).is_empty(),
            "an unbalanced `(` in a doc string must not drop a Memory(Inner) delegate arm - the store must BITE"
        );
        let balanced = "pub enum Backend {\n    #[cfg(test)]\n    #[doc = \"(x)\"]\n    Fake,\n    Memory(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Row>>>),\n}\npub struct TupleStore {\n    backend: Backend,\n}";
        assert!(
            !no_in_memory_durable_store().run(balanced).is_empty(),
            "a balanced doc paren must still bite (the control) - the un-gated Memory arm fires"
        );
    }

    #[test]
    fn probe_neutrality_directly_gated_store_and_mod_still_admitted() {
        let direct = format!("#[cfg(test)]\n{UNGATED_STORE}");
        assert!(
            no_in_memory_durable_store().run(&direct).is_empty(),
            "a directly `#[cfg(test)]`-attributed in-memory store is a test double - admitted"
        );
        let in_mod = "#[cfg(test)]\nmod tests {\n    pub struct PrincipalStore {\n        inner: std::sync::Mutex<std::collections::BTreeMap<String, Row>>,\n    }\n}";
        assert!(
            no_in_memory_durable_store().run(in_mod).is_empty(),
            "an in-memory store inside a `#[cfg(test)] mod` is a test double - admitted"
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
        let cfg_test = "#[cfg(test)]\nmod tests {\n    fn t() { let g = Gateway::builder(a(), h(), Arc::new(AllowAll)); }\n}";
        let ts = "#[cfg(any(test, feature = \"test-support\"))]\nmod harness {\n    fn t() { let g = Gateway::builder(a(), h(), Arc::new(AllowAllRepos)); }\n}";
        assert!(no_permissive_authorizer_in_prod().run(cfg_test).is_empty());
        assert!(no_permissive_authorizer_in_prod().run(ts).is_empty());
    }

    #[test]
    fn permissive_authorizer_admits_the_real_policy_and_non_permissive_fixtures() {
        let real = "fn boot() {\n    let gw = Gateway::builder(authn, human_login, Arc::new(AuthenticatedActionPolicy::mounted()));\n}";
        let deny = "fn t() {\n    let d = Arc::new(DenyAllRepos);\n    let d2 = Arc::new(DenyAll);\n}";
        let def = "/// like `Arc::new(AllowAll)` in tests\npub struct AllowAll;\nimpl Authorizer for AllowAll {\n    fn authorize(&self) -> bool { true }\n}";
        let call = "fn t() {\n    let x = Arc::new(AllowAllRepos::with_flags(f));\n}";
        assert!(no_permissive_authorizer_in_prod().run(real).is_empty());
        assert!(no_permissive_authorizer_in_prod().run(deny).is_empty());
        assert!(
            no_permissive_authorizer_in_prod().run(def).is_empty(),
            "definitions/impl/doc mentions must not be flagged - construction only \
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
