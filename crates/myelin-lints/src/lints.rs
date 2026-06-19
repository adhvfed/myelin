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

use crate::engine::{blank_string_literals, code_lines, code_statements, Lint, LintId, Violation};

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

/// `no-raw-publish` (§2.11; BUS-2; F5; the Bus's owned slice of contract 1.6 — EB-07 → P-019).
///
/// **Rule.** No bus publish outside `OutboxTx::emit`. There is NO fire-and-forget publish path:
/// a direct broker publish (`broker.publish(`, `nats.publish(`, `producer.send(`, a
/// `publish_now(` symbol, `bus.publish(`) OR a direct call onto the `BusTransport` seam
/// (`transport.put(`, `bus.put(`, `broker.put(` — the relay's broker-side publish, refined-arch
/// §4.1 / §5.2) is the lost-event / causality-break bug class. The ONLY admitted emit path is
/// `OutboxTx::emit` / `.emit(` on the outbox transaction (the event lands in the same DB
/// transaction as the state change; the relay is the only thing on the broker publish side).
///
/// **Note.** The relay crate ITSELF is the one legitimate broker-publish site (it drains the
/// outbox, calling `BusTransport::put`). The workspace scan (`tests/workspace_clean.rs`) excludes
/// `myelin-events/src/relay.rs` for exactly this reason — documented there, not silently skipped
/// (EI-01 §4/§5).
///
/// **EB-07 reconciliation (P-019, coherence rule EI-01 §7).** The `no-raw-publish` lint, its
/// engine, and its red/green fixtures were first shipped by the SUBSTRATE prompt P-S10 → P-017
/// (the four load-bearing lints; the lint harness is shared substrate, EB-07 CONTRACTS field).
/// EB-07 is the Bus's OWNED slice of the same contract-1.6 lint. Rather than duplicate a parallel
/// scanner, this prompt EXTENDS the in-place scanner to also forbid a direct `BusTransport::put`
/// call (`transport.put(` / `bus.put(` / `broker.put(`). That seam did not exist when P-017 ran;
/// it landed in EB-04 → P-013 (the `BusTransport` trait + the relay's `transport.put(subject,
/// envelope, dedup_id)` broker-publish), so the EB-07 red fixture — a write-path handler calling
/// `transport.put(..)` directly — is a genuinely NEW bug-fingerprint the lint must reject. The
/// lint is sharpened, never weakened (EI-01 §5).
pub const NO_RAW_PUBLISH: LintId = LintId("no-raw-publish");

fn scan_no_raw_publish(src: &str) -> Vec<Violation> {
    // Fire-and-forget / direct-broker publish fingerprints that bypass the outbox. These are
    // METHOD-CALL sites (each carries a leading `.` + a `(`) so a mere mention of the token in an
    // identifier — e.g. a test FN named `outbox_has_only_emit_no_publish_now()` asserting the
    // symbol's ABSENCE — is NOT flagged. The bug class is the dotted CALL on a broker/producer
    // handle, not a free function that happens to contain the word.
    //
    // The last three are the `BusTransport::put` seam (EB-04 → P-013; refined-arch §4.1/§5.2): a
    // direct `transport.put(` / `bus.put(` / `broker.put(` in a write path is the EB-07 red
    // fingerprint. They are HANDLE-QUALIFIED (`transport.`/`bus.`/`broker.` prefix) — NOT a bare
    // `.put(` — so an unrelated `.put(` (e.g. `BufMut::put` on a byte buffer) is NOT flagged; only
    // a call onto a broker/transport handle is. The relay (the one legitimate caller) is excluded
    // by path in `tests/workspace_clean.rs`, named not hidden.
    const RAW_PUBLISH_SITES: &[&str] = &[
        ".publish_now(",
        ".publish(",
        ".publish_event(",
        ".send_to_broker(",
        ".kafka_send(",
        "transport.put(",
        "bus.put(",
        "broker.put(",
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
                         fire-and-forget publish path, and the `BusTransport::put` broker seam is \
                         the relay's alone; an event must be emitted in the SAME transaction as \
                         the state change (the relay is the only broker-publish component). Use \
                         `outbox_tx.emit(draft, cause)` (F5/BUS-2)."
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

// ============================================================================================
// The remaining EIGHT architecture lints (the P-S11 → P-018 slice, completing the twelve).
//
// §2.11 / contract-index 1.6. Each is a hermetic source-scanner in the SAME style as the four
// load-bearing lints above (CODE-only via `code_lines`/`code_statements`, typed LOUD
// `Violation`s, no swallow path), each paired with a red + green fixture in `tests/fixtures/`.
//
// Several of these lints target code that does NOT exist yet (`search-requires-acl-filter`,
// `flow-determinism`, `control-plane-pii-free`, `forward-only-migration` partly). Per the P-S11
// DELIVERABLE we SHIP THE LINT + ITS FIXTURES NOW so the gate is live BEFORE the consumer ships,
// and NAME each as a floor that tightens when the targeted code lands (see the per-lint
// "Floor (named)" notes below). A lint over not-yet-written code is still a committed ratchet
// click: it admits the whole current (empty-of-target) workspace and rejects the bug fingerprint
// the moment the consumer introduces it.
// ============================================================================================

/// `no-cross-db` (§2.11; ADR-01).
///
/// **Rule.** A service crate must not depend on ANOTHER service's storage module. Each service
/// owns its store and opens its own pool; the boundary between services is the frozen contract
/// crate, never a shared data-access path (ADR-01 — the glue crates are the only cross-service
/// coupling). **Fingerprint scanned:** a `use` / path reference into another subsystem's storage
/// internals — `myelin_<other>::storage`, `myelin_<other>::store`, `myelin_<other>::db`,
/// `crate::<...>` is fine (same crate), but reaching across to `myelin_identity::store::*` from,
/// say, the Git crate is the cross-DB coupling bug class.
///
/// The scanner flags any `use myelin_*::{storage|store|db|schema|repo|pool}` path — a reach into
/// a sibling service's data layer. (Depending on a sibling's *contract* surface — its public
/// types, traits, event tokens — is allowed and is NOT a `::storage`/`::store` path.)
pub const NO_CROSS_DB: LintId = LintId("no-cross-db");

fn scan_no_cross_db(src: &str) -> Vec<Violation> {
    // Sibling-storage-module path fingerprints. The leading `myelin_` (Rust crate ident form)
    // means we are crossing a CRATE boundary; the `::storage`/`::store`/... segment means we are
    // reaching into that crate's DATA layer (not its public contract surface).
    const STORAGE_SEGMENTS: &[&str] = &[
        "::storage::",
        "::storage;",
        "::store::",
        "::store;",
        "::db::",
        "::db;",
        "::schema::",
        "::schema;",
        "::repo::",
        "::repo;",
        "::pool::",
        "::pool;",
    ];
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        let trimmed = code.trim();
        // Only `use` statements / path imports cross a crate boundary structurally.
        if !trimmed.starts_with("use ") && !trimmed.contains("use myelin_") {
            continue;
        }
        if !trimmed.contains("myelin_") {
            continue;
        }
        if STORAGE_SEGMENTS.iter().any(|seg| trimmed.contains(seg)) {
            out.push(Violation {
                lint: NO_CROSS_DB,
                line,
                reason: "a service crate reaches into another service's storage module \
                         (`myelin_<other>::storage|store|db|schema|repo|pool`) — services may \
                         only couple over the frozen contract crate, never a shared data path; \
                         each service owns its store and opens its own pool (ADR-01/no-cross-db)."
                    .into(),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`NO_CROSS_DB`].
pub fn no_cross_db() -> Lint {
    Lint {
        id: NO_CROSS_DB,
        rule: "a service crate must not depend on another service's storage module",
        scan: scan_no_cross_db,
    }
}

/// `forward-only-migration` (§2.11; STOR-2, §9).
///
/// **Rule.** No rollback migration file ("rollback" is a NEW forward migration, never a down
/// migration); no blocking `ALTER` on a flagged-hot table. **Fingerprint scanned:** a down/rollback
/// migration marker (`-- down`, `fn down(`, `.down.sql`, `DROP COLUMN`, a `down:` migration field)
/// is the reversible-migration bug class; a bare blocking `ALTER TABLE ... ADD COLUMN ... NOT NULL`
/// / `ALTER TABLE ... ALTER COLUMN` without the expand→backfill→contract idiom is the
/// table-lock-under-load bug class.
///
/// **Floor (named).** The hot-table half is partial here: the per-subsystem hot-table DECLARATION
/// that `forward-only-migration` reads to know WHICH `ALTER`s are forbidden lands with the
/// migration runner + the `AppSpec` hot-table mechanism in **P-S15 / P-032**. Until then this
/// scanner enforces the table-INDEPENDENT half (no down migration; no obviously-blocking
/// `ALTER ... NOT NULL` add) and tightens to the per-table rule when the declaration exists.
pub const FORWARD_ONLY_MIGRATION: LintId = LintId("forward-only-migration");

fn scan_forward_only_migration(src: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        // Blank string-literal CONTENTS so a forbidden token held as DATA — e.g. the migration
        // runner's own guard `upper.contains("DROP COLUMN")` — is not mistaken for real DDL. The
        // lint targets migration DDL, not Rust code that *checks for* the DDL pattern.
        let code = blank_string_literals(&code);
        let lower = code.to_ascii_lowercase();
        let trimmed = lower.trim();
        // (a) A down / rollback migration: reversibility is forbidden (forward-only).
        let is_down = trimmed.starts_with("-- down")
            || trimmed.starts_with("fn down(")
            || trimmed.starts_with("pub fn down(")
            || trimmed.contains(".down.sql")
            || (trimmed.contains("down:") && lower.contains("migration"))
            || trimmed.contains("drop column");
        if is_down {
            out.push(Violation {
                lint: FORWARD_ONLY_MIGRATION,
                line,
                reason: "a down/rollback migration is forbidden — migrations are FORWARD-ONLY \
                         (a rollback is a NEW forward migration, never a `down`/`DROP COLUMN`); \
                         use expand→backfill→contract (STOR-2/§9)."
                    .into(),
            });
        }
        // (b) A blocking ALTER that adds a NOT NULL column or rewrites a column in place — the
        // table-lock-under-load bug class. (The per-hot-table tightening lands in P-S15/P-032.)
        let alter_adds_not_null = lower.contains("alter table")
            && lower.contains("add column")
            && lower.contains("not null")
            && !lower.contains("default");
        let alter_column_inplace =
            lower.contains("alter table") && lower.contains("alter column");
        if alter_adds_not_null || alter_column_inplace {
            out.push(Violation {
                lint: FORWARD_ONLY_MIGRATION,
                line,
                reason: "a blocking `ALTER TABLE` (ADD COLUMN ... NOT NULL without DEFAULT, or \
                         ALTER COLUMN in place) takes a table lock — on a hot table this stalls \
                         writes; use the expand→backfill→contract idiom (nullable add, backfill, \
                         then constrain) (forward-only-migration/§9)."
                    .into(),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`FORWARD_ONLY_MIGRATION`].
pub fn forward_only_migration() -> Lint {
    Lint {
        id: FORWARD_ONLY_MIGRATION,
        rule: "no rollback migration file; no blocking ALTER on a flagged-hot table",
        scan: scan_forward_only_migration,
    }
}

/// `no-cross-sync-cycle` (§2.11; EI-02 §3).
///
/// **Rule.** The synchronous call graph is acyclic; **identity is a sink** (everyone may call
/// Identity synchronously; Identity calls no one synchronously). If A calls B SYNCHRONOUSLY, then
/// B must react to A only over the bus (never a sync call back). **Fingerprint scanned (the
/// source-scanning twin of the build-layer `crate-graph-acyclic` test in `myelin-substrate`):** a
/// SYNC outbound service client call FROM inside `myelin-identity` (`SyncClient`/`ServiceClient`/
/// `.call_sync(`/`reqwest::`/`.get(`/`.post(` to another service) — Identity must not originate a
/// synchronous cross-service call, or it is no longer a sink and a cycle becomes possible.
///
/// **Floor (named).** This is the structural-sink half (Identity originates no sync cross-service
/// call). The full call-graph-acyclicity check across ALL service pairs rides the resilient
/// inter-service client (`SyncClient`, P-S16 / P-033) + the per-edge sync-call registry; this
/// scanner enforces the load-bearing sink invariant now and tightens when the client lands.
pub const NO_CROSS_SYNC_CYCLE: LintId = LintId("no-cross-sync-cycle");

fn scan_no_cross_sync_cycle(src: &str) -> Vec<Violation> {
    // A synchronous OUTBOUND cross-service call fingerprint. From Identity these are forbidden
    // (Identity is the sink). The reactive/bus path (`.emit(`, an EventHandler) is NOT a sync
    // call and is always allowed.
    const SYNC_OUTBOUND_SITES: &[&str] = &[
        ".call_sync(",
        ".sync_call(",
        "SyncServiceClient",
        ".rpc_call(",
        "reqwest::Client",
        ".send_request(",
    ];
    let mut out = Vec::new();
    // This lint only fires INSIDE the identity crate (the sink). The workspace scan passes the
    // file's crate via a marker the scanner detects: an identity source file carries the module
    // path `myelin-identity` in its own header doc OR is scanned with the identity guard. Because
    // the scanner is a pure fn of source text, we key off an in-source sink marker the identity
    // crate's modules carry: a `//! IDENTITY-SINK` doc-line, OR the canonical crate self-reference
    // `crate` within a file that also names itself identity. To stay hermetic and avoid coupling,
    // the fixture marks the sink explicitly with `// @identity-sink`.
    let is_identity_sink = src.contains("@identity-sink") || src.contains("IDENTITY-SINK");
    if !is_identity_sink {
        return out;
    }
    for (line, code) in code_lines(src) {
        for site in SYNC_OUTBOUND_SITES {
            if code.contains(site) {
                out.push(Violation {
                    lint: NO_CROSS_SYNC_CYCLE,
                    line,
                    reason: format!(
                        "a synchronous outbound cross-service call `{site}` originates from \
                         Identity — Identity is the SINK of the sync call graph (everyone may \
                         call Identity synchronously; Identity calls no one synchronously). React \
                         over the bus instead so the sync call graph stays acyclic (EI-02 §3)."
                    ),
                });
            }
        }
    }
    out
}

/// The [`Lint`] value for [`NO_CROSS_SYNC_CYCLE`].
pub fn no_cross_sync_cycle() -> Lint {
    Lint {
        id: NO_CROSS_SYNC_CYCLE,
        rule: "the sync call graph is acyclic; identity is a sink",
        scan: scan_no_cross_sync_cycle,
    }
}

/// `residency-pin` (§2.11; ADR-11, recon §10; index 10.5).
///
/// **Rule.** Every store/stream/index/cache declares a region; **no global pool**; outbound
/// transfer is gated. **Fingerprint scanned:** a store/stream/index/cache CONSTRUCTION
/// (`PgPool::connect(`, `OltpPool::open(`, `ColocatedOltp::open(`, `BlobStore::`, `IndexBackend::`,
/// `CacheClient::new(`, a `Stream::` declaration) on a statement that does NOT also pin a `Region`
/// (`Region`, `region:`, `.region(`, `.pinned_to(`, `ResidencyTag`). A global (region-less) pool is
/// the data-leaves-its-region bug class.
///
/// **Floor (named).** The store-construction surface is M1 (the OLTP client P-ST-01, BlobStore
/// P-ST-03, the index backends). This scanner ships the gate now keyed to those constructor
/// fingerprints; it tightens (adds each concrete constructor) as the stores land. The
/// `residency-pin` STORAGE-half twin is also shipped in P-ST-04 / P-020 over the storage crate.
///
/// **P-ST-04 / P-020 sharpening (the Storage-relevant slice; coherence rule EI-01 §7).** The
/// residency-pin lint was first shipped by the substrate prompt P-S11 → P-018 keyed to *hypothetical*
/// store constructors (`OltpStore::open(`, `PgPool::connect(`). The OLTP tier client it constrains
/// landed in P-ST-01 → P-007 (`myelin-storage`), whose REAL caller-facing store constructors are
/// [`myelin_storage::OltpPool::open`] / [`myelin_storage::ColocatedOltp::open`]. Rather than
/// duplicate a second residency scanner, P-ST-04 EXTENDS the in-place fingerprint set with those two
/// real constructors (replacing the placeholder `OltpStore::open(`), so a caller opening the OLTP
/// store WITHOUT pinning a `Region` on the same construction statement is rejected. The lint is
/// SHARPENED (it now constrains the real surface), never weakened (EI-01 §5).
///
/// **Floor (named) — the runtime region-pin is M1.** Enforcing region-pinning end-to-end on the
/// store *runtime* (the `(tenant, region)` pin flowing through every write so a cross-region write
/// is impossible by construction, STOR-D5) is the M1 prompt **P-ST-15 / P-102**. On the M0 floor the
/// `myelin-storage` pool MODEL is region-agnostic at the pool layer (region lives in the per-query
/// `(tenant, region)` `TenantScope`); the storage crate's own internal pool wiring is therefore
/// admitted today (see `tests/storage_lints.rs`, which proves the lint REJECTS a region-less *caller*
/// open and ADMITS a region-pinned one). The fingerprint is live NOW so the moment a caller opens a
/// store without a region, the gate fires.
///
/// **P-CP-03 / P-026 sharpening — the WRITE-BOUNDARY (layer-3) half; coherence rule EI-01 §7.**
/// `residency-pin` is one of the TWO lints Tenancy owns (contract-index 1.6); the substrate/storage
/// prompts above built the STORE-OPEN half (a region-less pool is the data-leaves-its-region bug).
/// The Tenancy ownership prompt P-CP-03 adds the genuinely-NEW second property the §4.3 rule names:
/// *every write asserts `row.region == cell.region`*, with the cell's region **threaded by the
/// harness**, never taken from a request field (refined-arch tenancy §4.3 / §5.3 layer 3 — the
/// `residency-pin` write-boundary check). This is a DISTINCT bug fingerprint from a region-less
/// store open: here the store IS region-bearing, but a write sets `row.region` from an
/// **untrusted request field** (`req.region` / `request.region` / `payload.region` / `input.region`
/// / `params.region`), so a forged request could land a row in the wrong region. Rather than
/// duplicate a parallel scanner (EI-01 §7), the in-place scanner is EXTENDED: a write-boundary site
/// (marked `// @residency-write`, the same loud, named marker discipline `@identity-sink` /
/// `@workflow-body` use, so the check fires only where a region is actually being written) that
/// derives the row region from a request field — and does NOT assert it against the harness-threaded
/// cell region (`cell.region` / `cell_region` / `ctx.region` / `scope.region` / `self.region`) — is
/// rejected as the cross-region-write bug class (CP-D3 lint leg). A write that pins the row region
/// from the cell handle is admitted. The lint is SHARPENED (it now also guards the write boundary),
/// never weakened (EI-01 §5).
///
/// **Floor (named) — the runtime CP-D3 drill is M1.** This is the *lint leg* (the compile-time
/// rejection) only. The full RUNTIME CP-D3 drill — an actual `row.region != cell.region` write
/// REJECTED at the live write boundary, plus the `residency_verify` attestation — lands once the
/// write boundary exists in **P-CP-12 / P-096** (and Storage's store-layer enforcement P-ST-15 /
/// P-102). The marker-keyed scanner ships the gate NOW so the bug-fingerprint is un-mergeable before
/// the boundary code lands.
pub const RESIDENCY_PIN: LintId = LintId("residency-pin");

fn scan_residency_pin(src: &str) -> Vec<Violation> {
    // Store/stream/index/cache construction fingerprints that MUST pin a region. The OLTP entries
    // are the REAL `myelin-storage` constructors (P-ST-01 → P-007; sharpened in P-ST-04 → P-020):
    // a caller opening the OLTP store/pool must pin a `Region` on the same construction statement.
    const STORE_SITES: &[&str] = &[
        "PgPool::connect(",
        "PgPoolOptions::",
        "OltpPool::open(",       // the real OLTP pool constructor (myelin-storage, P-ST-01).
        "ColocatedOltp::open(",  // the real co-located OLTP+outbox store constructor (P-ST-02).
        "BlobStore::open(",
        "IndexBackend::open(",
        "CacheClient::new(",
        "StreamStore::open(",
    ];
    // Tokens that prove a region/residency is pinned on the same statement.
    const REGION_BINDERS: &[&str] = &[
        "Region",
        "region:",
        ".region(",
        ".pinned_to(",
        "ResidencyTag",
        "residency",
    ];
    // The P-ST-04 / P-020 NAMED-FLOOR waiver marker. A construction site flagged
    // `@residency-cell-pinned` (in a trailing/adjacent comment) is admitted because the cell's
    // region pins it OUT-OF-BAND on the M0 floor (the per-query `(tenant, region)` `TenantScope`
    // carries the region; the per-pool runtime region-pin lands end-to-end in P-ST-15 / P-102,
    // STOR-D5). This is a LOUD, REVIEWED, NAMED waiver in source (EI-01 §4 — named, never a silent
    // skip), NOT a weakening: an UNMARKED region-less store open still fires. The marker lives in a
    // COMMENT (stripped from `code`), so it is matched against the RAW line text, not the code line.
    const WAIVER_MARKER: &str = "@residency-cell-pinned";
    // A FILE-level waiver: a module whose docs carry `@residency-cell-pinned:file` declares the
    // WHOLE file is the M0 region-less pool MODEL (the storage substrate P-ST-15 region-pins). This
    // is the same file-marker discipline `flow-determinism` (`@workflow-body`) /
    // `no-cross-sync-cycle` (`@identity-sink`) use — a loud, named, reviewed floor, not a silent
    // skip. The lint stays fully live on every OTHER (caller/application) file.
    const WAIVER_MARKER_FILE: &str = "@residency-cell-pinned:file";
    if src.contains(WAIVER_MARKER_FILE) {
        return Vec::new();
    }
    // Raw (comment-included) lines so the waiver marker — which lives in a comment — is visible.
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (line, code) in code_statements(src) {
        let is_store = STORE_SITES.iter().any(|s| code.contains(s));
        if !is_store {
            continue;
        }
        let is_pinned = REGION_BINDERS.iter().any(|b| code.contains(b));
        // A statement starts at `line` (1-based); the per-site waiver marker lives in the comment
        // block IMMEDIATELY above the construction (or trailing on its line). Scan the raw text on
        // the statement's start line and a small window of lines just above it (a contiguous run of
        // comment lines — the conventional place for a reviewed, named waiver). The window stops at
        // the first non-comment, non-blank line so a marker on an UNRELATED earlier statement does
        // not leak its waiver downward.
        const WAIVER_LOOKBACK: usize = 8;
        let idx = line.saturating_sub(1);
        let here = raw_lines.get(idx).is_some_and(|l| l.contains(WAIVER_MARKER));
        let mut above = false;
        let mut i = idx;
        for _ in 0..WAIVER_LOOKBACK {
            let Some(j) = i.checked_sub(1) else { break };
            i = j;
            let Some(raw) = raw_lines.get(i) else { break };
            let t = raw.trim();
            if t.contains(WAIVER_MARKER) {
                above = true;
                break;
            }
            // Stop at the first line that is neither blank nor a comment (the waiver must be in the
            // comment block directly attached to THIS construction, not an earlier statement).
            if !t.is_empty() && !t.starts_with("//") {
                break;
            }
        }
        let waived = here || above;
        if !is_pinned && !waived {
            out.push(Violation {
                lint: RESIDENCY_PIN,
                line,
                reason: "a store/stream/index/cache is constructed WITHOUT a pinned region — \
                         every store must declare its `Region` (no global pool); a region-less \
                         pool lets data leave its residency boundary (ADR-11/residency-pin)."
                    .into(),
            });
        }
    }
    // ---- P-CP-03 / P-026: the WRITE-BOUNDARY (layer-3) half. -------------------------------------
    // The §4.3 rule's second clause: every write asserts `row.region == cell.region`, with the cell
    // region threaded by the HARNESS — never taken from a request field. This half is keyed to a
    // `// @residency-write` marker (the loud, named marker discipline `@identity-sink`/`@workflow-body`
    // use) so it fires ONLY where a region is actually being written, and admits the whole current
    // workspace until the write boundary lands (P-CP-12 / P-096). Outside a write-boundary file there
    // is nothing to add.
    out.extend(scan_residency_write_boundary(src));
    out
}

/// The write-boundary (layer-3) leg of `residency-pin` (P-CP-03 / P-026): inside a write-boundary
/// site (a file/line marked `// @residency-write`), a write that sets the row's `region` from an
/// untrusted REQUEST FIELD — rather than from the harness-threaded CELL region — is rejected. A
/// region-mismatched write is the cross-region-write bug class (CP-D3 lint leg). This is a DISTINCT
/// fingerprint from a region-less store open (the store-construction loop above): here the region is
/// present but sourced from the wrong place.
fn scan_residency_write_boundary(src: &str) -> Vec<Violation> {
    // The marker that scopes this leg to actual write-boundary code (so ordinary code that reads a
    // request's region for non-storage purposes is not flagged). Matched on RAW lines (it lives in a
    // comment); a file-level marker arms the whole file, a per-line trailing marker arms one write.
    const WRITE_MARKER: &str = "@residency-write";
    if !src.contains(WRITE_MARKER) {
        return Vec::new();
    }
    let file_armed = src.lines().any(|l| {
        let t = l.trim_start();
        // A module-doc (`//!`) or top-of-file marker arms the whole file.
        t.starts_with("//!") && t.contains(WRITE_MARKER)
    });
    // A row-region ASSIGNMENT whose value is an UNTRUSTED request field. We match the assignment
    // TARGET being a region (`region`) and the SOURCE being a request-shaped handle. Both sets are
    // conservative substrings; the marker keeps the leg scoped so a false positive is loud and rare.
    const REQUEST_SOURCES: &[&str] = &[
        "req.region",
        "request.region",
        "payload.region",
        "input.region",
        "params.region",
        "body.region",
        "msg.region",
    ];
    // Tokens proving the row region is instead derived from / checked against the harness-threaded
    // CELL region (the trusted source). A write that names ANY of these alongside the row region is
    // pinning to the cell, not the request — admitted.
    const CELL_REGION_SOURCES: &[&str] = &[
        "cell.region",
        "cell_region",
        "ctx.region",
        "scope.region",
        "self.region",
        "tenant_scope.region",
        "CellRegion",
        "harness.region",
    ];
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (line, code) in code_statements(src) {
        let from_request = REQUEST_SOURCES.iter().any(|s| code.contains(s));
        if !from_request {
            continue;
        }
        // Only fire if this statement actually writes a region (assigns/sets `region`) — a mere read
        // of `req.region` into a non-region binding is not the write-boundary bug.
        let writes_region = code.contains("region:")
            || code.contains("region =")
            || code.contains(".region(")
            || code.contains("set_region(")
            || code.contains("row.region");
        if !writes_region {
            continue;
        }
        // Admitted if the SAME write derives/asserts the region from the trusted cell handle.
        let pinned_to_cell = CELL_REGION_SOURCES.iter().any(|c| code.contains(c));
        // Per-line marker arming: a `@residency-write` in the statement's comment block (or the file
        // marker) arms this site. Without the file marker, require a per-site marker so the leg never
        // fires on an unmarked statement.
        let idx = line.saturating_sub(1);
        let site_armed = file_armed
            || raw_lines.get(idx).is_some_and(|l| l.contains(WRITE_MARKER))
            || {
                // look just above for the marker in an attached comment block.
                let mut armed = false;
                let mut i = idx;
                for _ in 0..6 {
                    let Some(j) = i.checked_sub(1) else { break };
                    i = j;
                    let Some(raw) = raw_lines.get(i) else { break };
                    let t = raw.trim();
                    if t.contains(WRITE_MARKER) {
                        armed = true;
                        break;
                    }
                    if !t.is_empty() && !t.starts_with("//") {
                        break;
                    }
                }
                armed
            };
        if site_armed && !pinned_to_cell {
            out.push(Violation {
                lint: RESIDENCY_PIN,
                line,
                reason: "a write derives `row.region` from an UNTRUSTED request field instead of \
                         asserting it against the harness-threaded CELL region — every write must \
                         pin `row.region == cell.region` with the cell's region injected by the \
                         harness (never a request field), or a forged request lands a row in the \
                         wrong region (the cross-region-write bug class; CP-D3 lint leg, \
                         residency-pin layer 3 — refined-arch tenancy §4.3/§5.3)."
                    .into(),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`RESIDENCY_PIN`].
pub fn residency_pin() -> Lint {
    Lint {
        id: RESIDENCY_PIN,
        rule: "every store/stream/index/cache declares a region; no global pool",
        scan: scan_residency_pin,
    }
}

/// `control-plane-pii-free` (§2.11; ADR-11, recon §OQ-I).
///
/// **Rule.** The control plane (routing, cross-cell pointers) carries OPAQUE IDS ONLY — never a
/// name/email/body. **Fingerprint scanned:** a control-plane struct (one marked
/// `// @control-plane` or named `*Pointer`/`*Routing`/`*Placement`/`CrossCell*`/`*Directory`) with
/// a PII-bearing field (`name`, `email`, `phone`, `address`, `body`, `display_name`, …). Only
/// opaque ids (`TenantId`, `Region`, slugs, hashes) may cross the control plane.
///
/// **Floor (named).** The concrete control-plane types (the `CrossCellPointer` frame, the routing
/// registry tables) land in Tenancy M0/M1 (**P-CP-02 / P-027, P-CP-05 / P-080**) with their own
/// `control-plane-pii-free` twin lint (P-CP-04 / P-028). This substrate scanner ships the gate now
/// keyed to the `@control-plane` marker + the naming fingerprint; it admits the (empty-of-target)
/// workspace and rejects a PII field the moment a control-plane struct introduces one.
pub const CONTROL_PLANE_PII_FREE: LintId = LintId("control-plane-pii-free");

fn scan_control_plane_pii_free(src: &str) -> Vec<Violation> {
    // PII field-name fingerprints forbidden on a control-plane struct (a superset focused on the
    // free-text / direct-identifier kinds the control plane must never carry).
    const PII_FIELDS: &[&str] = &[
        "name",
        "email",
        "phone",
        "address",
        "body",
        "display_name",
        "full_name",
        "given_name",
        "family_name",
        "first_name",
        "last_name",
        "message",
        "comment",
        "title",
    ];
    let lines = code_lines(src);
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut in_cp_struct = false;
    let mut cp_struct_depth: i32 = 0;
    // Marker: the previous lines flagged this struct as control-plane (a `// @control-plane`
    // attribute line is stripped by code_lines, so we look at the RAW src for the marker on the
    // line just above the struct; simpler: a struct whose NAME matches the control-plane shapes,
    // OR any struct in a file that carries the `@control-plane` file marker).
    let file_is_control_plane = src.contains("@control-plane");
    const CP_NAME_FINGERPRINTS: &[&str] = &[
        "Pointer",
        "Routing",
        "Placement",
        "CrossCell",
        "Directory",
        "ControlPlane",
    ];

    for (line, code) in &lines {
        let trimmed = code.trim();
        let opens_struct = trimmed.starts_with("struct ")
            || trimmed.starts_with("pub struct ")
            || trimmed.contains(" struct ");
        if opens_struct && code.contains('{') {
            let named_cp = CP_NAME_FINGERPRINTS.iter().any(|n| trimmed.contains(n));
            in_cp_struct = file_is_control_plane || named_cp;
            cp_struct_depth = depth + 1;
        }
        if in_cp_struct && depth >= cp_struct_depth - 1 {
            if let Some(field_name) = field_identifier(trimmed) {
                if PII_FIELDS.contains(&field_name) {
                    out.push(Violation {
                        lint: CONTROL_PLANE_PII_FREE,
                        line: *line,
                        reason: format!(
                            "control-plane struct carries PII field `{field_name}` — the control \
                             plane (routing, cross-cell pointers, placement, directory) must carry \
                             OPAQUE IDS ONLY (TenantId/Region/slug/hash), never a name/email/body. \
                             PII is born inside the cell, never in the control plane (ADR-11/OQ-I)."
                        ),
                    });
                }
            }
        }
        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;
        depth += opens - closes;
        if in_cp_struct && depth < cp_struct_depth - 1 {
            in_cp_struct = false;
        }
    }
    out
}

/// The [`Lint`] value for [`CONTROL_PLANE_PII_FREE`].
pub fn control_plane_pii_free() -> Lint {
    Lint {
        id: CONTROL_PLANE_PII_FREE,
        rule: "the control plane carries opaque ids only — never a name/email/body",
        scan: scan_control_plane_pii_free,
    }
}

/// `search-requires-acl-filter` (§2.11; ADR-03, recon §OQ-E).
///
/// **Rule.** Every search/list query conjoins the `list_objects` `Filter` BEFORE scoring —
/// pre-filter, never post-filter. **Fingerprint scanned:** a search/list execution
/// (`.search(`, `index.query(`, `IndexBackend::search`, `.list_objects_scored(`, a
/// `SearchQuery::new(`) on a statement that does NOT also conjoin the ACL filter (`acl_filter`,
/// `.with_acl(`, `Filter::`, `.conjoin_filter(`, `list_objects`, `permission_filter`). A search
/// that scores first and filters after leaks the EXISTENCE/RANK of forbidden docs (the
/// post-filter leak bug class).
///
/// **Floor (named).** The search/list query surface lands in Search M2 (**SRCH-P08 / P-171**, the
/// permission-aware query pipeline). This scanner ships the gate now keyed to the search-call
/// fingerprints; the Search subsystem also ships its OWN `search-requires-acl-filter` twin
/// (SRCH-P01 / P-021). It tightens to the type-system form (a `Scored` result is unconstructable
/// without a `Filter`) when the pipeline lands.
pub const SEARCH_REQUIRES_ACL_FILTER: LintId = LintId("search-requires-acl-filter");

fn scan_search_requires_acl_filter(src: &str) -> Vec<Violation> {
    // Search/list execution fingerprints that MUST conjoin the ACL filter before scoring.
    const SEARCH_SITES: &[&str] = &[
        ".search(",
        ".query_index(",
        "IndexBackend::search",
        ".list_objects_scored(",
        "SearchQuery::execute",
        ".rank(",
    ];
    // Tokens that prove the ACL filter is conjoined on the same statement.
    const ACL_BINDERS: &[&str] = &[
        "acl_filter",
        ".with_acl(",
        "Filter::",
        ".conjoin_filter(",
        "list_objects",
        "permission_filter",
        ".pre_filter(",
    ];
    let mut out = Vec::new();
    for (line, code) in code_statements(src) {
        let is_search = SEARCH_SITES.iter().any(|s| code.contains(s));
        if !is_search {
            continue;
        }
        let is_acl_bound = ACL_BINDERS.iter().any(|b| code.contains(b));
        if !is_acl_bound {
            out.push(Violation {
                lint: SEARCH_REQUIRES_ACL_FILTER,
                line,
                reason: "a search/list query is executed WITHOUT conjoining the list_objects ACL \
                         `Filter` before scoring — search must PRE-filter on permission, never \
                         post-filter; scoring before filtering leaks the existence and rank of \
                         forbidden documents (ADR-03/OQ-E)."
                    .into(),
            });
        }
    }
    out
}

/// The [`Lint`] value for [`SEARCH_REQUIRES_ACL_FILTER`].
pub fn search_requires_acl_filter() -> Lint {
    Lint {
        id: SEARCH_REQUIRES_ACL_FILTER,
        rule: "every search/list query conjoins the list_objects Filter before scoring",
        scan: scan_search_requires_acl_filter,
    }
}

/// `no-llm-in-platform` (§2.11; ADR-08.2, VISION §3).
///
/// **Rule.** No LLM SDK / prompt / model name appears in PLATFORM code; the runtime is behind the
/// `AgentRuntime` strategy seam. **Fingerprint scanned:** an LLM SDK import or a model-name literal
/// (`openai`, `anthropic`, `@anthropic-ai`, a `claude-*`/`gpt-*` model id, `langchain`,
/// `.chat_completion(`, `.completions.create(`, a `system_prompt`/`build_prompt` symbol). The agent
/// BRAIN lives behind `AgentRuntime` (a strategy seam) so the platform never hard-codes a provider.
///
/// **Floor (named).** The `AgentRuntime` strategy seam ships in the agent crate (**AG-P1 / P-130**,
/// which also declares this lint's agent-side twin). This substrate scanner ships the gate now so
/// no LLM dependency can leak into platform code before the seam exists. It excludes the agent
/// crate's OWN runtime-adapter module (the one place an SDK is legitimately referenced — named, not
/// silent), which the workspace scan handles via the exclusion list.
pub const NO_LLM_IN_PLATFORM: LintId = LintId("no-llm-in-platform");

fn scan_no_llm_in_platform(src: &str) -> Vec<Violation> {
    // LLM SDK / prompt / model-name fingerprints forbidden in platform code. These are matched as
    // lowercase substrings of CODE (comments stripped) so a doc-comment naming the rule is not
    // flagged; the agent runtime-adapter module is excluded at the workspace-scan level.
    const LLM_SITES: &[&str] = &[
        "openai",
        "anthropic",
        "langchain",
        "llama_index",
        ".chat_completion(",
        ".completions.create(",
        "system_prompt",
        "build_prompt",
        "model_name",
        "claude-3",
        "gpt-4",
    ];
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        let lower = code.to_ascii_lowercase();
        for site in LLM_SITES {
            if lower.contains(site) {
                out.push(Violation {
                    lint: NO_LLM_IN_PLATFORM,
                    line,
                    reason: format!(
                        "LLM SDK / prompt / model-name fingerprint `{site}` in platform code — no \
                         LLM SDK, prompt, or model name may appear in platform code; the agent \
                         brain lives behind the `AgentRuntime` strategy seam so the platform is \
                         provider-agnostic (ADR-08.2/VISION §3)."
                    ),
                });
            }
        }
    }
    out
}

/// The [`Lint`] value for [`NO_LLM_IN_PLATFORM`].
pub fn no_llm_in_platform() -> Lint {
    Lint {
        id: NO_LLM_IN_PLATFORM,
        rule: "no LLM SDK / prompt / model name in platform code; runtime behind AgentRuntime",
        scan: scan_no_llm_in_platform,
    }
}

/// `flow-determinism` (§2.11; index 9.2, recon §OQ-F).
///
/// **Rule.** A `myelin-flow` workflow body uses ONLY the deterministic `WfCtx` surface; any
/// non-determinism must be journaled through `WfCtx` (`ctx.now()`, `ctx.rand()`, `ctx.activity(`,
/// `ctx.sleep_*`, `ctx.emit(`). **Fingerprint scanned:** a raw non-deterministic call inside a
/// workflow body (`SystemTime::now(`, `Instant::now(`, `Utc::now(`, `rand::`, `thread_rng(`,
/// `Uuid::new_v4(`, `tokio::time::sleep(`, a direct `reqwest::`/IO call) that bypasses `WfCtx`. A
/// raw clock/rng read makes replay diverge (the non-deterministic-replay bug class).
///
/// **Floor (named).** The `WfCtx` surface + the `myelin-flow` crate land in Workflow M2
/// (**P-FLOW-04 / P-199**), and the workflow-determinism lint's red+green fixtures are re-shipped
/// against the REAL `WfCtx` in **P-FLOW-08 / P-200**. This substrate scanner ships the gate now,
/// keyed to a `// @workflow-body` marker (so it only fires inside a workflow body, not in ordinary
/// service code that legitimately reads the clock); it tightens to the real `WfCtx` type when the
/// crate lands.
pub const FLOW_DETERMINISM: LintId = LintId("flow-determinism");

fn scan_flow_determinism(src: &str) -> Vec<Violation> {
    // Raw non-deterministic calls forbidden inside a workflow body (they bypass WfCtx and make
    // replay diverge).
    const NONDET_SITES: &[&str] = &[
        "SystemTime::now(",
        "Instant::now(",
        "Utc::now(",
        "Local::now(",
        "rand::",
        "thread_rng(",
        "Uuid::new_v4(",
        "tokio::time::sleep(",
        "std::thread::sleep(",
    ];
    let mut out = Vec::new();
    // This lint only fires inside a WORKFLOW BODY (marked `// @workflow-body`); ordinary service
    // code legitimately reads the clock. The marker keeps the scanner hermetic until the real
    // `myelin-flow` crate + `WfCtx` type land (P-FLOW-04/P-199), at which point the lint keys off
    // the workflow-fn signature instead.
    let is_workflow_body = src.contains("@workflow-body") || src.contains("WORKFLOW-BODY");
    if !is_workflow_body {
        return out;
    }
    for (line, code) in code_lines(src) {
        for site in NONDET_SITES {
            if code.contains(site) {
                out.push(Violation {
                    lint: FLOW_DETERMINISM,
                    line,
                    reason: format!(
                        "raw non-deterministic call `{site}` in a workflow body bypasses the \
                         deterministic `WfCtx` surface — a workflow must read time/rand/IO only \
                         through `ctx.now()`/`ctx.rand()`/`ctx.activity(..)` so replay is \
                         deterministic; a raw clock/rng read makes replay diverge (index 9.2/OQ-F)."
                    ),
                });
            }
        }
    }
    out
}

/// The [`Lint`] value for [`FLOW_DETERMINISM`].
pub fn flow_determinism() -> Lint {
    Lint {
        id: FLOW_DETERMINISM,
        rule: "a myelin-flow workflow body uses only the deterministic WfCtx surface",
        scan: scan_flow_determinism,
    }
}

/// The remaining EIGHT lints (the P-S11 slice of the twelve), in §2.11 table order.
pub fn remaining_eight() -> Vec<Lint> {
    vec![
        no_cross_db(),
        forward_only_migration(),
        no_cross_sync_cycle(),
        residency_pin(),
        control_plane_pii_free(),
        search_requires_acl_filter(),
        no_llm_in_platform(),
        flow_determinism(),
    ]
}

/// The stable ids of the remaining eight lints (for the matrix + the "exactly eight" regression).
pub const REMAINING_EIGHT: [LintId; 8] = [
    NO_CROSS_DB,
    FORWARD_ONLY_MIGRATION,
    NO_CROSS_SYNC_CYCLE,
    RESIDENCY_PIN,
    CONTROL_PLANE_PII_FREE,
    SEARCH_REQUIRES_ACL_FILTER,
    NO_LLM_IN_PLATFORM,
    FLOW_DETERMINISM,
];

/// The full TWELVE architecture lints (P-S10's four + P-S11's eight), in §2.11 table order. This
/// is the complete committed ratchet; the workspace scan and the fixture matrix both run it.
pub fn all_twelve() -> Vec<Lint> {
    let mut v = load_bearing_four();
    v.extend(remaining_eight());
    v
}

/// The stable ids of all twelve lints, in §2.11 table order.
pub const ALL_TWELVE: [LintId; 12] = [
    TENANT_PREDICATE,
    NO_RAW_PUBLISH,
    NO_HOST_EXEC,
    NO_UNTAGGED_PERSONAL_DATA,
    NO_CROSS_DB,
    FORWARD_ONLY_MIGRATION,
    NO_CROSS_SYNC_CYCLE,
    RESIDENCY_PIN,
    CONTROL_PLANE_PII_FREE,
    SEARCH_REQUIRES_ACL_FILTER,
    NO_LLM_IN_PLATFORM,
    FLOW_DETERMINISM,
];

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
    fn no_raw_publish_rejects_direct_bustransport_put_in_write_path() {
        // EB-07 → P-019: the Bus's owned slice. A direct call onto the `BusTransport::put` broker
        // seam (EB-04 → P-013) in a write path bypasses the outbox — the relay is its only caller.
        let red = "self.transport.put(subject, envelope, event_id).await?;";
        let red_bus = "bus.put(subject, envelope, event_id);";
        let red_broker = "broker.put(subject, envelope, event_id);";
        assert!(!no_raw_publish().run(red).is_empty(), "transport.put( must be rejected");
        assert!(!no_raw_publish().run(red_bus).is_empty(), "bus.put( must be rejected");
        assert!(!no_raw_publish().run(red_broker).is_empty(), "broker.put( must be rejected");
    }

    #[test]
    fn no_raw_publish_admits_unrelated_put_calls() {
        // The fingerprint is HANDLE-QUALIFIED (transport./bus./broker. prefix), so an unrelated
        // `.put(` — e.g. a byte-buffer `BufMut::put`, a cache `.put(k, v)` — is NOT flagged.
        let buf = "buf.put(&bytes[..]);";
        let cache = "cache.put(key, value);";
        assert!(no_raw_publish().run(buf).is_empty(), "BufMut::put must be admitted");
        assert!(no_raw_publish().run(cache).is_empty(), "an unrelated cache.put must be admitted");
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

    // ---- the remaining eight (P-S11 → P-018) ----

    #[test]
    fn no_cross_db_rejects_sibling_storage_use_admits_contract_use() {
        let red = "use myelin_identity::store::PrincipalStore;";
        let green = "use myelin_identity::PrincipalId;"; // a sibling's CONTRACT surface is fine.
        assert!(!no_cross_db().run(red).is_empty());
        assert!(no_cross_db().run(green).is_empty());
    }

    #[test]
    fn forward_only_migration_rejects_down_and_blocking_alter_admits_expand() {
        let red_down = "fn down() { /* rollback */ }";
        let red_alter = "ALTER TABLE principals ADD COLUMN email TEXT NOT NULL;";
        let green = "ALTER TABLE principals ADD COLUMN email TEXT;"; // nullable add = expand.
        assert!(!forward_only_migration().run(red_down).is_empty());
        assert!(!forward_only_migration().run(red_alter).is_empty());
        assert!(forward_only_migration().run(green).is_empty());
    }

    #[test]
    fn no_cross_sync_cycle_rejects_identity_sync_call_admits_bus_reaction() {
        let red = "// @identity-sink\nlet r = client.call_sync(req);";
        let green = "// @identity-sink\nctx.emit(draft, cause)?;"; // reacting over the bus is fine.
        let elsewhere = "let r = client.call_sync(req);"; // not in identity → not this lint.
        assert!(!no_cross_sync_cycle().run(red).is_empty());
        assert!(no_cross_sync_cycle().run(green).is_empty());
        assert!(no_cross_sync_cycle().run(elsewhere).is_empty());
    }

    #[test]
    fn residency_pin_rejects_global_pool_admits_pinned_pool() {
        let red = "let pool = PgPool::connect(url).await?;";
        let green = "let pool = PgPool::connect(url).region(Region::EuWest).await?;";
        assert!(!residency_pin().run(red).is_empty());
        assert!(residency_pin().run(green).is_empty());
    }

    #[test]
    fn residency_pin_write_boundary_rejects_region_from_request_admits_region_from_cell() {
        // P-CP-03 / P-026: the write-boundary (layer-3) leg. Inside a `@residency-write` site, a
        // write that sets the row region from an UNTRUSTED request field is rejected; a write that
        // pins it from the harness-threaded CELL region is admitted. The marker keeps the leg
        // scoped (an UNMARKED region-from-request read does NOT fire — it is not a write boundary).
        let red = "// @residency-write\nlet row = Row { region: req.region, tenant_id };";
        let green =
            "// @residency-write\nlet row = Row { region: cell.region, tenant_id }; // pinned to cell";
        let unmarked = "let r = Row { region: req.region, tenant_id };"; // no write-boundary marker.
        assert!(
            !residency_pin().run(red).is_empty(),
            "a write taking row.region from a request field must be rejected"
        );
        assert!(
            residency_pin().run(green).is_empty(),
            "a write pinning row.region to the cell region must be admitted"
        );
        assert!(
            residency_pin().run(unmarked).is_empty(),
            "an UNMARKED region-from-request statement is not a write boundary — must not fire"
        );
    }

    #[test]
    fn residency_pin_write_boundary_reads_cell_region_from_harness_not_request_field() {
        // The TESTS-field assertion: the lint reads the cell `region` from the harness-threaded
        // handle, NOT from a request field. A file-level `@residency-write` marker arms the whole
        // write boundary; the cell-region source (`ctx.region`/`scope.region`/`cell.region`) is the
        // trusted handle, a request field is the forgeable one.
        let from_harness = "//! @residency-write\nfn write(ctx: &CellCtx, req: &Req) {\n    \
                            store.insert(Row { region: ctx.region, data: req.data });\n}";
        let from_request = "//! @residency-write\nfn write(ctx: &CellCtx, req: &Req) {\n    \
                            store.insert(Row { region: req.region, data: req.data });\n}";
        assert!(
            residency_pin().run(from_harness).is_empty(),
            "a row pinned to the harness-threaded cell region must be admitted"
        );
        assert!(
            !residency_pin().run(from_request).is_empty(),
            "a row whose region comes from a request field must be rejected"
        );
    }

    #[test]
    fn residency_pin_store_open_half_is_unchanged_by_the_write_boundary_leg() {
        // Coherence regression (EI-01 §7): the P-CP-03 write-boundary leg is ADDITIVE — it does not
        // alter the store-open half. A region-less pool still fires; a region-pinned pool still
        // admits; neither carries the write marker, so the write leg stays silent on them.
        let red = "let pool = PgPool::connect(url).await?;";
        let green = "let pool = PgPool::connect(url).region(Region::EuWest).await?;";
        assert!(!residency_pin().run(red).is_empty(), "region-less open still fires");
        assert!(residency_pin().run(green).is_empty(), "region-pinned open still admits");
    }

    #[test]
    fn control_plane_pii_free_rejects_pii_field_admits_opaque_ids() {
        let red = "pub struct CrossCellPointer {\n    pub email: String,\n}";
        let green = "pub struct CrossCellPointer {\n    pub tenant_id: TenantId,\n    pub region: Region,\n}";
        assert!(!control_plane_pii_free().run(red).is_empty());
        assert!(control_plane_pii_free().run(green).is_empty());
    }

    #[test]
    fn search_requires_acl_filter_rejects_unfiltered_search_admits_prefiltered() {
        let red = "let hits = index.search(query).await?;";
        let green = "let hits = index.search(query.with_acl(acl_filter)).await?;";
        assert!(!search_requires_acl_filter().run(red).is_empty());
        assert!(search_requires_acl_filter().run(green).is_empty());
    }

    #[test]
    fn no_llm_in_platform_rejects_sdk_admits_runtime_seam() {
        let red = "let client = openai::Client::new(key);";
        let green = "let out = agent_runtime.run(plan).await?;"; // behind the AgentRuntime seam.
        assert!(!no_llm_in_platform().run(red).is_empty());
        assert!(no_llm_in_platform().run(green).is_empty());
    }

    #[test]
    fn flow_determinism_rejects_raw_clock_in_workflow_admits_wfctx() {
        let red = "// @workflow-body\nlet t = SystemTime::now();";
        let green = "// @workflow-body\nlet t = ctx.now();";
        let elsewhere = "let t = SystemTime::now();"; // not a workflow body → not this lint.
        assert!(!flow_determinism().run(red).is_empty());
        assert!(flow_determinism().run(green).is_empty());
        assert!(flow_determinism().run(elsewhere).is_empty());
    }

    #[test]
    fn remaining_eight_is_exactly_the_named_eight() {
        let lints = remaining_eight();
        assert_eq!(lints.len(), 8);
        let ids: Vec<LintId> = lints.iter().map(|l| l.id).collect();
        assert_eq!(ids, REMAINING_EIGHT.to_vec());
    }

    #[test]
    fn all_twelve_is_the_four_plus_the_eight_in_order() {
        let lints = all_twelve();
        assert_eq!(lints.len(), 12, "the full ratchet is exactly twelve lints");
        let ids: Vec<LintId> = lints.iter().map(|l| l.id).collect();
        assert_eq!(ids, ALL_TWELVE.to_vec());
        // No id is duplicated across the four + the eight.
        let mut sorted: Vec<&str> = ids.iter().map(|i| i.0).collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 12, "all twelve lint ids must be distinct");
    }
}
