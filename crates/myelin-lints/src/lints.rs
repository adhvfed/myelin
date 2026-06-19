//! The four load-bearing architecture lints (§2.11 / contract-index 1.6; P-S10 → P-017).
//!
//! Each lint is a [`Lint`] whose scanner reads CODE (comments stripped by
//! [`engine::code_lines`]) and emits a typed [`Violation`] per offending site. The rules are
//! deliberately conservative scanners (the named "scanner-grade, not type-system-grade" floor in
//! the crate docs): they catch the recurring failure FINGERPRINT (EI-01 §5) without a toolchain
//! plugin, and tighten to a type-system guarantee when the targeted surface lands.
//!
//! Each lint below pairs with a red fixture (a sample it MUST reject) and a green fixture (a
//! sample it MUST admit) in `tests/fixtures/` and is exercised by `tests/fixture_matrix.rs`.

use crate::engine::{code_lines, code_statements, Lint, LintId, Violation};

/// `tenant-predicate` (§2.11; EI-02 §1, ID-3; F2 the IDOR floor).
///
/// **Rule.** Every query-builder call carries a `TenantId` bound; a query that reads a tenant
/// store WITHOUT threading the tenant predicate is rejected. **Fingerprint scanned:** a
/// query-builder construction/execution (`query(...)`, `.from(`, `.select(`, `sqlx::query`,
/// `QueryBuilder::new`, a `SELECT`/`UPDATE`/`DELETE` SQL literal) on a line/statement that does
/// NOT also bind a tenant (`TenantId`, `tenant_id`, `.tenant(`, `WHERE ... tenant`, the RLS
/// guard `with_tenant`/`scoped`). A tenant-less query is the cross-tenant IDOR bug class.
///
/// **Floor (named).** The type-system form — a query-builder type that is unconstructable
/// without a `TenantId` so a tenant-less query fails to COMPILE — lands with the Identity/Storage
/// M1 query-builder. This scanner is the M0 ratchet click that keeps the gate live until then.
pub const TENANT_PREDICATE: LintId = LintId("tenant-predicate");

fn scan_tenant_predicate(src: &str) -> Vec<Violation> {
    // The query-builder fingerprints that MUST be tenant-bound.
    const QUERY_SITES: &[&str] = &[
        "sqlx::query",
        "QueryBuilder::new",
        ".from(",
        "query_as!",
        "query!(",
    ];
    // Tokens that prove a tenant predicate is threaded on the same statement.
    const TENANT_BINDERS: &[&str] = &[
        "TenantId",
        "tenant_id",
        ".tenant(",
        "with_tenant",
        "scoped_to_tenant",
        "RlsGuard",
        "set_tenant",
    ];
    let mut out = Vec::new();
    // Scan at STATEMENT granularity so a tenant binder on a later line of the same fluent
    // query-builder chain (`sqlx::query(..)\n  .with_tenant(t)\n  .fetch_all(p);`) is seen.
    for (line, code) in code_statements(src) {
        let is_query = QUERY_SITES.iter().any(|s| code.contains(s));
        if !is_query {
            continue;
        }
        let is_tenant_bound = TENANT_BINDERS.iter().any(|b| code.contains(b));
        if !is_tenant_bound {
            out.push(Violation {
                lint: TENANT_PREDICATE,
                line,
                reason: "query-builder call has no TenantId bound — every tenant-store query \
                         must thread the tenant predicate (the RLS guard / a TenantId arg / a \
                         WHERE tenant clause). A tenant-less query is a cross-tenant IDOR (F2)."
                    .into(),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`TENANT_PREDICATE`].
pub fn tenant_predicate() -> Lint {
    Lint {
        id: TENANT_PREDICATE,
        rule: "every query-builder call carries a TenantId bound; a tenant-less query is rejected",
        scan: scan_tenant_predicate,
    }
}

/// `no-raw-publish` (§2.11; BUS-2; F5).
///
/// **Rule.** No bus publish outside `OutboxTx::emit`. There is NO fire-and-forget publish path:
/// a direct broker publish (`broker.publish(`, `nats.publish(`, `producer.send(`, a
/// `publish_now(` symbol, `bus.publish(`) is the lost-event / causality-break bug class. The ONLY
/// admitted emit path is `OutboxTx::emit` / `.emit(` on the outbox transaction (the event lands
/// in the same DB transaction as the state change; the relay is the only thing on the broker
/// publish side).
///
/// **Note.** The relay crate ITSELF is the one legitimate broker-publish site (it drains the
/// outbox). The workspace scan (`tests/workspace_clean.rs`) excludes `myelin-events/src/relay.rs`
/// for exactly this reason — documented there, not silently skipped (EI-01 §4/§5).
pub const NO_RAW_PUBLISH: LintId = LintId("no-raw-publish");

fn scan_no_raw_publish(src: &str) -> Vec<Violation> {
    // Fire-and-forget / direct-broker publish fingerprints that bypass the outbox. These are
    // METHOD-CALL sites (each carries a leading `.` + a `(`) so a mere mention of the token in an
    // identifier — e.g. a test FN named `outbox_has_only_emit_no_publish_now()` asserting the
    // symbol's ABSENCE — is NOT flagged. The bug class is the dotted CALL on a broker/producer
    // handle, not a free function that happens to contain the word.
    const RAW_PUBLISH_SITES: &[&str] = &[
        ".publish_now(",
        ".publish(",
        ".publish_event(",
        ".send_to_broker(",
        ".kafka_send(",
    ];
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        for site in RAW_PUBLISH_SITES {
            if code.contains(site) {
                out.push(Violation {
                    lint: NO_RAW_PUBLISH,
                    line,
                    reason: format!(
                        "raw bus publish `{site}` bypasses OutboxTx::emit — there is NO \
                         fire-and-forget publish path; an event must be emitted in the SAME \
                         transaction as the state change (the relay is the only broker-publish \
                         component). Use `outbox_tx.emit(draft, cause)` (F5/BUS-2)."
                    ),
                });
            }
        }
    }
    out
}

/// The [`Lint`] value for [`NO_RAW_PUBLISH`].
pub fn no_raw_publish() -> Lint {
    Lint {
        id: NO_RAW_PUBLISH,
        rule: "no bus publish outside OutboxTx::emit; no fire-and-forget publish path",
        scan: scan_no_raw_publish,
    }
}

/// `no-host-exec` (§2.11; AG-2; X-6).
///
/// **Rule.** No host-execution path bypassing `ToolHands::exec` (= the unified sandbox). A raw
/// host exec (`std::process::Command`, `tokio::process`, `Command::new(`, `libc::exec`,
/// `nix::unistd::execv`) is the privilege-escape bug class: any platform code that shells out to
/// the host kernel WITHOUT going through the unified sandbox seam is rejected. The ONLY admitted
/// execution path is `ToolHands::exec` / `.exec(` on the sandbox handle.
pub const NO_HOST_EXEC: LintId = LintId("no-host-exec");

fn scan_no_host_exec(src: &str) -> Vec<Violation> {
    // Host-execution fingerprints that bypass the unified sandbox (ToolHands::exec).
    const HOST_EXEC_SITES: &[&str] = &[
        "std::process::Command",
        "tokio::process",
        "Command::new(",
        "libc::exec",
        "nix::unistd::exec",
        "process::Command",
    ];
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        for site in HOST_EXEC_SITES {
            if code.contains(site) {
                out.push(Violation {
                    lint: NO_HOST_EXEC,
                    line,
                    reason: format!(
                        "host-execution path `{site}` bypasses ToolHands::exec (the unified \
                         sandbox) — no platform code may shell out to the host kernel directly; \
                         all execution goes through the sandbox seam so the four uniform \
                         guarantees hold (X-6/AG-2)."
                    ),
                });
            }
        }
    }
    out
}

/// The [`Lint`] value for [`NO_HOST_EXEC`].
pub fn no_host_exec() -> Lint {
    Lint {
        id: NO_HOST_EXEC,
        rule: "no host-execution path bypassing ToolHands::exec (the unified sandbox)",
        scan: scan_no_host_exec,
    }
}

/// `no-untagged-personal-data` (§2.11; ADR-12; recon §10.2).
///
/// **Rule.** Every schema struct FIELD whose name carries PII (`email`, `name`, `phone`,
/// `address`, `ip_addr`/`ip_address`, `full_name`, `given_name`, `family_name`, `display_name`,
/// `dob`/`birth`, `ssn`, `passport`, `body`/`message_body`, `comment_text`) must be preceded by a
/// `#[personal_data(...)]` attribute. An untagged PII column is the un-erasable / un-mapped
/// subject bug class (it escapes the crypto-shred + RoPA fan-out). Only fields INSIDE a
/// `struct ... {` body are considered (so a local `let email = ...` is not flagged).
///
/// **Floor (named).** The type-system form — a `#[personal_data]` classify-derive macro that
/// refuses to expand a schema with an untagged PII field — lands in **P-GA-07 / P-107**. This
/// scanner is the M0 ratchet click.
pub const NO_UNTAGGED_PERSONAL_DATA: LintId = LintId("no-untagged-personal-data");

fn scan_no_untagged_personal_data(src: &str) -> Vec<Violation> {
    // Field-name fingerprints that carry PII (matched as a whole field identifier).
    const PII_FIELDS: &[&str] = &[
        "email",
        "phone",
        "address",
        "ip_addr",
        "ip_address",
        "full_name",
        "given_name",
        "family_name",
        "display_name",
        "first_name",
        "last_name",
        "dob",
        "date_of_birth",
        "ssn",
        "passport",
        "message_body",
        "comment_text",
    ];
    let lines = code_lines(src);
    let mut out = Vec::new();
    let mut depth: i32 = 0; // brace depth; >0 means we are inside SOME `{ }` body.
    let mut in_struct = false;
    let mut struct_brace_depth: i32 = 0;
    // Track whether the immediately-preceding non-blank code line was a #[personal_data] attr.
    let mut prev_was_personal_data = false;

    for (line, code) in &lines {
        let trimmed = code.trim();

        // Detect entering a struct body: `struct Name {` (possibly with generics / derive above).
        let opens_struct = trimmed.starts_with("struct ")
            || trimmed.starts_with("pub struct ")
            || trimmed.contains(" struct ");
        if opens_struct && code.contains('{') {
            in_struct = true;
            struct_brace_depth = depth + 1;
        }

        // Check fields BEFORE updating brace depth so a field on the `struct X {` line is rare;
        // fields are on their own lines inside the body.
        if in_struct && depth >= struct_brace_depth - 1 {
            if let Some(field_name) = field_identifier(trimmed) {
                if PII_FIELDS.contains(&field_name) && !prev_was_personal_data {
                    out.push(Violation {
                        lint: NO_UNTAGGED_PERSONAL_DATA,
                        line: *line,
                        reason: format!(
                            "PII field `{field_name}` is not #[personal_data(...)]-tagged — every \
                             schema field carrying personal data must be tagged so the \
                             crypto-shred erase + the RoPA/data-map fan-out reach it; an untagged \
                             PII column leaves an un-erasable subject (ADR-12)."
                        ),
                    });
                }
            }
        }

        // Update brace depth for the NEXT line.
        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;
        depth += opens - closes;
        if in_struct && depth < struct_brace_depth - 1 {
            in_struct = false;
        }

        // Update the "previous line was a #[personal_data] attr" flag (ignore blank lines).
        if !trimmed.is_empty() {
            prev_was_personal_data = trimmed.contains("#[personal_data");
        }
    }
    out
}

/// Extract a struct-field identifier from a trimmed code line of the form `name: Type,` /
/// `pub name: Type,`. Returns `None` for non-field lines (attributes, braces, method calls).
fn field_identifier(trimmed: &str) -> Option<&str> {
    // A field line has the shape `[pub ]ident : ...`. Reject lines that are clearly not fields.
    if trimmed.is_empty()
        || trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with('{')
        || trimmed.starts_with('}')
        || trimmed.starts_with("struct ")
        || trimmed.starts_with("pub struct ")
        || trimmed.starts_with("fn ")
        || trimmed.starts_with("pub fn ")
        || trimmed.starts_with("let ")
    {
        return None;
    }
    let body = trimmed.strip_prefix("pub ").unwrap_or(trimmed);
    let body = body.strip_prefix("pub(crate) ").unwrap_or(body);
    // Must be `ident :` — the colon binds the field type. Reject `::` (a path) and `:=`/`==`.
    let colon = body.find(':')?;
    // A path separator `::` is not a field colon.
    if body.as_bytes().get(colon + 1) == Some(&b':') {
        return None;
    }
    let ident = body[..colon].trim();
    if ident.is_empty() || !is_ident(ident) {
        return None;
    }
    Some(ident)
}

/// True iff `s` is a single Rust identifier (no spaces, valid ident chars).
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The [`Lint`] value for [`NO_UNTAGGED_PERSONAL_DATA`].
pub fn no_untagged_personal_data() -> Lint {
    Lint {
        id: NO_UNTAGGED_PERSONAL_DATA,
        rule: "every PII-carrying schema field is #[personal_data(...)]-tagged",
        scan: scan_no_untagged_personal_data,
    }
}

/// The four load-bearing lints (the P-S10 slice of the twelve), in §2.11 table order. The
/// fixture matrix and the workspace scan both run this exact set so the gate is the same surface
/// everywhere.
pub fn load_bearing_four() -> Vec<Lint> {
    vec![
        tenant_predicate(),
        no_raw_publish(),
        no_host_exec(),
        no_untagged_personal_data(),
    ]
}

/// The stable ids of the four load-bearing lints (for the regression test that asserts the set is
/// exactly four and named).
pub const LOAD_BEARING_FOUR: [LintId; 4] = [
    TENANT_PREDICATE,
    NO_RAW_PUBLISH,
    NO_HOST_EXEC,
    NO_UNTAGGED_PERSONAL_DATA,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::run;

    #[test]
    fn tenant_predicate_rejects_tenantless_query_admits_bound_query() {
        let red = "let rows = sqlx::query(\"SELECT * FROM principals\").fetch_all(&pool);";
        let green = "let rows = sqlx::query(\"SELECT * FROM principals\").with_tenant(tenant_id);";
        assert!(!tenant_predicate().run(red).is_empty());
        assert!(tenant_predicate().run(green).is_empty());
    }

    #[test]
    fn no_raw_publish_rejects_publish_now_admits_emit() {
        let red = "broker.publish_now(&envelope);";
        let green = "outbox_tx.emit(draft, cause)?;";
        assert!(!no_raw_publish().run(red).is_empty());
        assert!(no_raw_publish().run(green).is_empty());
    }

    #[test]
    fn no_host_exec_rejects_command_admits_toolhands() {
        let red = "let out = std::process::Command::new(\"sh\").output();";
        let green = "let out = tool_hands.exec(job_spec).await?;";
        assert!(!no_host_exec().run(red).is_empty());
        assert!(no_host_exec().run(green).is_empty());
    }

    #[test]
    fn no_untagged_personal_data_rejects_untagged_email_admits_tagged() {
        let red = "pub struct User {\n    pub email: String,\n}";
        let green = "pub struct User {\n    #[personal_data(contact)]\n    pub email: String,\n}";
        assert!(!no_untagged_personal_data().run(red).is_empty());
        assert!(no_untagged_personal_data().run(green).is_empty());
    }

    #[test]
    fn untagged_lint_ignores_non_pii_fields_and_locals() {
        let non_pii = "pub struct Cfg {\n    pub region: String,\n    pub count: u64,\n}";
        let local = "fn f() {\n    let email = lookup();\n}";
        assert!(no_untagged_personal_data().run(non_pii).is_empty());
        assert!(no_untagged_personal_data().run(local).is_empty());
    }

    #[test]
    fn load_bearing_four_is_exactly_the_named_four() {
        let lints = load_bearing_four();
        assert_eq!(lints.len(), 4);
        let ids: Vec<LintId> = lints.iter().map(|l| l.id).collect();
        assert_eq!(ids, LOAD_BEARING_FOUR.to_vec());
    }

    #[test]
    fn run_over_the_four_is_loud_on_any_violation() {
        let dirty = "broker.publish_now(&e);";
        assert!(run(&load_bearing_four(), dirty).is_err());
        let clean = "let x = 1 + 1;";
        assert!(run(&load_bearing_four(), clean).is_ok());
    }
}
