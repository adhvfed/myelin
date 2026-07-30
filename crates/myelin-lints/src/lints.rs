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
///
/// **EB-09 reconciliation (P-045, the Bus's OWNED slice; coherence rule EI-01 §7).** The
/// `tenant-predicate` lint, its engine, and its query-builder red/green fixtures were first shipped
/// by the SUBSTRATE prompt P-S10 → P-017 (the four load-bearing architecture lints; the lint
/// harness is shared substrate). That P-017 form is the DATA-STORE half: it fires on a
/// query-builder call (`sqlx::query`, `.from(`, …) that is not tenant-bound — the cross-tenant IDOR
/// bug class. EB-09 is the Bus's OWNED slice of the SAME contract-1.6 lint, and its CANON docs name
/// the STREAM/CONSUMER surface: refined-arch event-bus §4.2 (the "whitelist subjects, **never**
/// `*`" rule — gotcha 1) + §7.1 (a stream is provisioned **per (tenant, subsystem)**, subject
/// `evt.<tenant>.<subsystem>...`) + §4.3 (`scope` is a bounded selector, **never** `*`; the
/// transport rejects an unbounded/over-broad scope). This is a genuinely DISTINCT bug fingerprint
/// from the data-store leg: here the construct is a `subscribe` / `consume` / stream provision, and
/// the bug is an UNSCOPED subscription (no `(tenant, subsystem)` scope) or a WILDCARD scope
/// (`scope = *`, a `>`/`.*` wildcard subject) — an over-broad subscription that crosses the
/// tenant/subsystem boundary (the firehose head-of-line stall + cross-tenant frame leak). Rather
/// than duplicate a parallel scanner (EI-01 §7), the in-place scanner is EXTENDED with this second
/// leg — keyed to a loud, named `// @bus-stream` marker (the same marker discipline `@identity-sink`
/// / `@write-path` / `@residency-write` use) so it fires ONLY where a bus subscribe/stream surface
/// is being scanned, and admits the whole current workspace until the consume/subscribe surface
/// (EB-05 / P-043, the firehose subscription protocol EB-21 / P-141) is wired live. The lint is
/// SHARPENED (it now also guards the stream subscribe boundary), never weakened (EI-01 §5). Together
/// with EB-07 (no-raw-publish) + EB-08 (no-cross-sync-cycle) this completes the Bus's THREE of the
/// twelve-lint M0 gate.
///
/// **Floor (named) — the type-system / transport-enforced form is later-band.** This is the *lint
/// leg* (the compile-time rejection of the unscoped/wildcard subscribe fingerprint) only. The
/// transport-enforced form — `subscribe(stream, scope, …)` whose `scope` type cannot represent `*`
/// so an unbounded subscription is impossible by construction, and the server-side reject of an
/// over-broad scope (§4.3) — lands with the resume-cursor subscription protocol (EB-21 / P-141) and
/// the partitioned-per-(tenant, region) streams (EB-12 / P-089). The marker-keyed scanner ships the
/// gate NOW so the unscoped/`*` subscribe bug fingerprint is un-mergeable before that surface exists.
pub const TENANT_PREDICATE: LintId = LintId("tenant-predicate");

/// A bus SUBSCRIBE / CONSUME / stream-provision fingerprint — the construct the EB-09 leg requires
/// to carry a `(tenant, subsystem)` scope. A subscription over the durable bus or the firehose
/// (`subscribe(...)` / `resume(...)` / `.consume(` / a `Consumer`/`Stream` provision) MUST bind a
/// bounded `(tenant, subsystem)` scope (refined-arch event-bus §4.2 gotcha 1 + §7.1 + §4.3).
const BUS_STREAM_SITES: &[&str] = &[
    ".subscribe(",
    "subscribe(",
    ".resume(",
    "resume(",
    ".consume(",
    "provision_stream(",
    "Consumer::bind(",
    "durable_consumer(",
];

/// Tokens that prove a bus subscribe/stream carries a BOUNDED `(tenant, subsystem)` scope (so it is
/// admitted). The §7.1 subject grammar (`evt.<tenant>.<subsystem>...`), a `StreamScope` /
/// `(tenant, subsystem)` scope value, or the explicit binders the consumer template threads.
const BUS_SCOPE_BINDERS: &[&str] = &[
    "StreamScope",
    "TenantId",
    "tenant_id",
    "subsystem",
    "Subsystem",
    "scoped_stream",
    "Scope::Tenant",
    // A bound `scope` value threaded into the subscribe argument list (the idiomatic shape: a
    // `StreamScope` is constructed in a prior statement and passed in). Matched as argument tokens
    // (`, scope`, `(scope`, `scope)`, `scope,`) so the BARE identifier `scope` only counts when it
    // is actually a call argument, never a stray substring (`scoped`, `descope`).
    ", scope",
    "(scope",
    "scope)",
    "scope,",
    ".scope(",
];

/// A WILDCARD / unbounded scope on a bus subscribe — rejected EVEN IF a scope token is present,
/// because `*` (or a `>` / `.*` wildcard subject, or an explicit "all streams" scope) is the
/// over-broad subscription the §4.2 "whitelist subjects, never `*`" rule + the §4.3 "scope is a
/// bounded selector, never `*`" rule forbid. Matched against the BLANKED-string form so the bare
/// glyphs (`= *`, `Scope::All`) are seen, plus the wildcard-subject string literals.
const BUS_WILDCARD_SCOPES: &[&str] = &[
    "Scope::All",
    "Scope::Star",
    "Scope::Wildcard",
    "scope: None",
    "scope = *",
    "scope=*",
    ".scope(*)",
    "AllStreams",
];

/// The wildcard-SUBJECT string-literal fingerprints (a subscribe whose subject is itself a
/// wildcard: NATS `>` / `*` tokens, or an `evt.*` over-broad subject). These are matched against the
/// RAW line (string contents intact), since the wildcard lives INSIDE the subject string literal.
const BUS_WILDCARD_SUBJECTS: &[&str] = &[
    "\"evt.>\"",
    "\"evt.*\"",
    "\".>\"",
    "\"*\"",
    "\">\"",
    "\"evt.*.*\"",
];

/// The EB-09 leg of `tenant-predicate` (P-045, the Bus's owned slice). Inside a bus-subscribe site
/// (a file/line marked `// @bus-stream`), a subscribe/consume/stream-provision that does NOT bind a
/// `(tenant, subsystem)` scope — OR that binds a WILDCARD / unbounded scope (`scope = *`, a `>`/`.*`
/// wildcard subject, an explicit "all streams" scope) — is rejected. A stream is provisioned per
/// `(tenant, subsystem)` (refined-arch event-bus §7.1, subject `evt.<tenant>.<subsystem>...`); the
/// firehose `scope` is a bounded selector, never `*` (§4.3); whitelist subjects, never `*` (§4.2
/// gotcha 1). This is a DISTINCT fingerprint from the data-store query leg: the originator is the
/// bus subscribe surface, scoped by the `@bus-stream` marker so the whole current workspace (no live
/// subscribe surface yet — EB-05 / P-043, EB-21 / P-141) is admitted until those paths land.
fn scan_tenant_predicate_bus_streams(src: &str) -> Vec<Violation> {
    const STREAM_MARKER: &str = "@bus-stream";
    if !src.contains(STREAM_MARKER) {
        return Vec::new();
    }
    // A module-doc (`//!`) or top-of-file marker arms the whole file; otherwise each offending
    // statement needs the marker on its own line or in the attached comment block (per-line arming),
    // so the leg never fires on an unmarked statement (same discipline as the EB-08 write-path leg).
    let raw_lines: Vec<&str> = src.lines().collect();
    let file_armed = raw_lines.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with("//!") && t.contains(STREAM_MARKER)
    });
    let mut out = Vec::new();
    for (line, code) in code_statements(src) {
        let Some(site) = BUS_STREAM_SITES.iter().find(|s| code.contains(**s)) else {
            continue;
        };
        if !is_marker_armed(&raw_lines, line, file_armed, STREAM_MARKER) {
            continue;
        }
        // A wildcard subject lives INSIDE a string literal, so test the RAW statement text (the
        // string contents are blanked in `code` by neither helper here — `code_statements` keeps
        // literals — but the wildcard glyphs are most robustly found on the raw source statement).
        let raw_stmt = raw_statement_text(&raw_lines, line);
        let blanked = blank_string_literals(&code);
        let has_wildcard_scope = BUS_WILDCARD_SCOPES.iter().any(|w| {
            // `= *` / `Scope::All` etc. are CODE, not string data — test the blanked form.
            blanked.contains(*w) || code.contains(*w)
        });
        let has_wildcard_subject = BUS_WILDCARD_SUBJECTS.iter().any(|w| raw_stmt.contains(*w));
        let is_scoped = BUS_SCOPE_BINDERS.iter().any(|b| code.contains(b));
        if has_wildcard_scope || has_wildcard_subject {
            out.push(Violation {
                lint: TENANT_PREDICATE,
                line,
                reason: format!(
                    "a bus subscribe/stream `{site}` uses a WILDCARD / unbounded scope — `scope` is \
                     a bounded selector, NEVER `*` (refined-arch event-bus §4.3); whitelist \
                     subjects, never `*` (§4.2 gotcha 1). An over-broad subscription crosses the \
                     (tenant, subsystem) boundary (cross-tenant frame leak + the firehose \
                     head-of-line stall). Scope the subscription to a bounded `(tenant, subsystem)` \
                     stream (subject `evt.<tenant>.<subsystem>...`, §7.1)."
                ),
            });
        } else if !is_scoped {
            out.push(Violation {
                lint: TENANT_PREDICATE,
                line,
                reason: format!(
                    "a bus subscribe/stream `{site}` has no (tenant, subsystem) scope — a stream is \
                     provisioned PER (tenant, subsystem) (refined-arch event-bus §7.1, subject \
                     `evt.<tenant>.<subsystem>...`); an unscoped subscription is the cross-tenant \
                     frame-leak bug class (the Bus's slice of tenant-predicate, contract 1.6). Thread \
                     a bounded `StreamScope`/`(TenantId, Subsystem)` scope on the subscribe."
                ),
            });
        }
    }
    out
}

/// Per-line marker arming shared by the marker-keyed leg: the marker on the statement's start line,
/// in the attached comment block directly above it (stops at the first non-comment, non-blank line
/// so a marker on an unrelated earlier statement does not leak its arming downward), or file-level.
fn is_marker_armed(raw_lines: &[&str], line: usize, file_armed: bool, marker: &str) -> bool {
    if file_armed {
        return true;
    }
    let idx = line.saturating_sub(1);
    if raw_lines.get(idx).is_some_and(|l| l.contains(marker)) {
        return true;
    }
    let mut i = idx;
    for _ in 0..6 {
        let Some(j) = i.checked_sub(1) else { break };
        i = j;
        let Some(raw) = raw_lines.get(i) else { break };
        let t = raw.trim();
        if t.contains(marker) {
            return true;
        }
        if !t.is_empty() && !t.starts_with("//") {
            break;
        }
    }
    false
}

/// The RAW source text of the statement that STARTS at `line` (1-based), joined across its
/// continuation lines up to the next statement terminator, with string literals INTACT — so a
/// wildcard subject inside a string literal (`subscribe("evt.>", …)`) is visible. The structural
/// boundary mirrors [`engine::code_statements`] (`;` / `{` / `}`), but keeps the original bytes.
fn raw_statement_text(raw_lines: &[&str], line: usize) -> String {
    let mut out = String::new();
    let start = line.saturating_sub(1);
    for raw in raw_lines.iter().skip(start) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(raw.trim());
        if raw.contains(';') || raw.contains('{') || raw.contains('}') {
            break;
        }
    }
    out
}

/// The statement that DEFINES the SQL a query-builder call executes, when the SQL text was hoisted
/// into a local (`let sql = format!("… WHERE tenant_id=$1 …"); sqlx::query(&sql)`).
///
/// Returns the defining `let` statement's code text so the caller can look for the tenant predicate
/// there as well as on the query statement — the query-site fingerprint is textual and statement-
/// local, so without this a composed query (one whose predicate is assembled with `format!`) loses
/// its tenant binder purely by where the string literal sits. Only the NEAREST PRECEDING definition
/// of that exact identifier is consulted (so a later shadowing binding is the one that counts), and
/// only a bare identifier argument (`&sql`, `sql`, `sql.as_str()`) resolves — anything else (an
/// inline literal, an expression) is left to the statement-local check.
fn hoisted_sql_statement<'a>(
    statements: &'a [(usize, String)],
    index: usize,
    code: &str,
    query_sites: &[&str],
) -> Option<&'a str> {
    let site_end = query_sites
        .iter()
        .filter_map(|site| {
            // A site token that already includes its `(` (`.from(`, `query!(`) must not consume it —
            // the argument scan starts at the call's OWN open paren.
            let width = site.len() - usize::from(site.ends_with('('));
            code.find(site).map(|at| at + width)
        })
        .min()?;
    let ident = sql_argument_identifier(query_argument(code, site_end)?)?;
    statements[..index]
        .iter()
        .rev()
        .find(|(_, statement)| binds_identifier(statement, ident))
        .map(|(_, statement)| statement.as_str())
}

/// The FIRST argument text of the call whose `(` follows `after` in `code` (the query-builder call's
/// SQL argument). Stops at the matching `)` or the first top-level `,`.
fn query_argument(code: &str, after: usize) -> Option<&str> {
    let rest = code.get(after..)?;
    let open = rest.find('(')?;
    let bytes = rest.as_bytes();
    let mut depth = 0usize;
    for (offset, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(rest[open + 1..offset].trim());
                }
            }
            b',' if depth == 1 => return Some(rest[open + 1..offset].trim()),
            _ => {}
        }
    }
    None
}

/// The bare local identifier an argument names, if it is one: `&sql`, `sql`, `&*sql`,
/// `sql.as_str()` → `sql`. A literal / a compound expression yields `None`.
fn sql_argument_identifier(argument: &str) -> Option<&str> {
    let mut rest = argument.trim();
    while let Some(stripped) = rest.strip_prefix('&').or_else(|| rest.strip_prefix('*')) {
        rest = stripped.trim_start();
    }
    rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let ident = &rest[..end];
    if ident.is_empty() || ident.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // Only a plain identifier or a borrow-ish accessor on it (`sql.as_str()`, `sql.as_ref()`).
    let tail = rest[end..].trim();
    (tail.is_empty() || tail.starts_with('.')).then_some(ident)
}

/// Whether `statement` contains a `let` binding of exactly `ident` (`let sql = …`, `let mut sql = …`).
fn binds_identifier(statement: &str, ident: &str) -> bool {
    let bytes = statement.as_bytes();
    let mut cursor = 0usize;
    while let Some(at) = statement[cursor..].find("let ") {
        let start = cursor + at;
        cursor = start + 4;
        // `let` must be a whole word (not the tail of `booklet `).
        let preceded_by_ident = start
            .checked_sub(1)
            .and_then(|i| bytes.get(i))
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        if preceded_by_ident {
            continue;
        }
        let after = statement[cursor..].trim_start();
        let after = after.strip_prefix("mut ").map_or(after, str::trim_start);
        let Some(tail) = after.strip_prefix(ident) else {
            continue;
        };
        if !tail.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            return true;
        }
    }
    false
}

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
        "loc.tenant",
        "envelope->>'tenant'",
    ];
    // A narrow, call-site waiver for queries over PostgreSQL catalog/lock state or other
    // deliberately cross-scope infrastructure that has no tenant column to bind. The marker must
    // be in the contiguous comment block immediately above the query (or on its starting line),
    // and its trailing `:` requires reviewers to record why the query is not a tenant-store read.
    // This is safer than excluding a mixed production module: every ordinary query in that module
    // remains linted, and an unrelated earlier marker cannot leak across intervening code.
    const CROSS_SCOPE_MARKER: &str = "@tenant-cross-scope:";
    const MARKER_LOOKBACK: usize = 8;
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    // Scan at STATEMENT granularity so a tenant binder on a later line of the same fluent
    // query-builder chain (`sqlx::query(..)\n  .with_tenant(t)\n  .fetch_all(p);`) is seen.
    let statements = code_statements(src);
    for (index, (line, code)) in statements.iter().enumerate() {
        let (line, code) = (*line, code.as_str());
        let is_query = QUERY_SITES.iter().any(|s| code.contains(s));
        if !is_query {
            continue;
        }
        // The tenant predicate may live on the query statement itself, OR — when the SQL text is
        // HOISTED into a local because the statement is composed (`let sql = format!("… WHERE
        // tenant_id=$1 …"); sqlx::query(&sql)`) — on the statement that DEFINES the SQL the query
        // executes. Resolving that one binding makes the check HOIST-INVARIANT: the same query
        // gets the same verdict whether its SQL is written inline or built one statement above.
        // It is not a weakening — a hoisted SQL string with NO tenant predicate still has no
        // tenant binder in either statement and still fires (see the unit tests).
        let is_tenant_bound = TENANT_BINDERS.iter().any(|b| code.contains(b))
            || hoisted_sql_statement(&statements, index, code, QUERY_SITES)
                .is_some_and(|sql| TENANT_BINDERS.iter().any(|b| sql.contains(b)));
        let idx = line.saturating_sub(1);
        let marker_here = raw_lines
            .get(idx)
            .is_some_and(|raw| raw.contains(CROSS_SCOPE_MARKER));
        let mut marker_above = false;
        let mut cursor = idx;
        for _ in 0..MARKER_LOOKBACK {
            let Some(previous) = cursor.checked_sub(1) else {
                break;
            };
            cursor = previous;
            let Some(raw) = raw_lines.get(cursor) else {
                break;
            };
            let trimmed = raw.trim();
            if trimmed.contains(CROSS_SCOPE_MARKER) {
                marker_above = true;
                break;
            }
            if !trimmed.is_empty() && !trimmed.starts_with("//") {
                break;
            }
        }
        if !is_tenant_bound && !marker_here && !marker_above {
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
    // ---- The EB-09 leg: the Bus's OWNED slice — the subscribe/stream (tenant, subsystem) scope
    // check (P-045). Inert unless a `// @bus-stream` marker arms it; SHARPENED, never weakened. ----
    out.extend(scan_tenant_predicate_bus_streams(src));
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

/// `no-raw-ci-verdict` (the CI stage-verdict forging surface — P-FLOW CI durability).
///
/// **Rule.** No code constructs a typed CI stage verdict (`SignalPayload::CiJobDone`) or calls the
/// typed-signal delivery seam (`.signal_typed(`) outside the sanctioned sites. The durable
/// `job.done` completion carries a stage PASS/FAIL verdict that the pipeline body trusts as the
/// runner's real result; ANY in-process holder of a tenant executor could otherwise FORGE a passing
/// verdict for a stage that never ran (a supply-chain / merge-gate bypass). The verdict must originate
/// ONLY where the runner's real exit code is verified — modeled EXACTLY on `no-raw-publish` (a typed
/// seam whose one legitimate caller is named, not hidden): the flow executors OWN the encoding
/// (`myelin-flow/src/executor.rs`, `myelin-flow/src/pg_executor.rs`) and the sanctioned CI reporter
/// (`myelin-ci-controlplane/src/ci_pipeline_driver.rs`) is the ONE production delivery site (the
/// durable claimed-job verification lands there in the follow-on). Those three files are NAMED, LOUD
/// exclusions in `lint-gate.rs` (like `myelin-events/src/relay.rs` for `no-raw-publish`); every other
/// file is fully scanned, so a new forging call anywhere else is rejected.
pub const NO_RAW_CI_VERDICT: LintId = LintId("no-raw-ci-verdict");

/// Whether `haystack` contains `token` as a WHOLE identifier word — the char before and after the
/// match are not identifier chars (`[A-Za-z0-9_]`). This flags `SignalPayload::CiJobDone`, a
/// `use … CiJobDone as Done` alias line, a UFCS `DurableExecutor::signal_typed(…)`, and a
/// `.signal_typed`-then-newline line-split (the token sits fully on the line), while NOT flagging a
/// longer identifier that merely CONTAINS the token — e.g. the internal `signal_typed_async` bridge
/// (the following `_` keeps it one word).
fn contains_ci_verdict_token(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(token) {
        let start = from + rel;
        let end = start + token.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

fn scan_no_raw_ci_verdict(src: &str) -> Vec<Violation> {
    // The CI verdict-forging fingerprints, matched as WHOLE-WORD TOKENS on each code line (comments +
    // string literals stripped by `code_lines`): the typed-verdict TYPE `CiJobDone` (its construction
    // OR any `use … as` alias naming it) and the typed-delivery METHOD `signal_typed` (a `.method`
    // call, a UFCS `Trait::signal_typed(` call, or a `.signal_typed`-then-newline line-split). Bare
    // whole-word matching (not a dotted/pathed literal) closes the alias/UFCS/line-split dodges; the
    // internal `signal_typed_async` bridge is NOT flagged (the trailing `_` keeps it one word), and the
    // seam files that define/implement the type live behind the per-lint exclusion in `lint-gate.rs`.
    //
    // HEURISTIC LIMIT (consistent with every other lint): macro-assembled or string-concatenated
    // spellings that never write the literal token on a code line are out of scope — the lint is a
    // pattern gate, not a type system.
    const CI_VERDICT_TOKENS: &[&str] = &["CiJobDone", "signal_typed"];
    let mut out = Vec::new();
    for (line, code) in code_lines(src) {
        for token in CI_VERDICT_TOKENS {
            if contains_ci_verdict_token(&code, token) {
                out.push(Violation {
                    lint: NO_RAW_CI_VERDICT,
                    line,
                    reason: format!(
                        "raw CI verdict token `{token}` forges a stage PASS/FAIL that the pipeline \
                         body trusts as the runner's verified result — a typed `job.done` verdict \
                         must originate ONLY where the runner's real guest exit code is verified (the \
                         CI reporter seam). Do not name/construct `SignalPayload::CiJobDone` or call \
                         `signal_typed` from arbitrary code; route completion through the sanctioned \
                         reporter."
                    ),
                });
            }
        }
    }
    out
}

/// The [`Lint`] value for [`NO_RAW_CI_VERDICT`]. Enforced over the real workspace by the `lint-gate`
/// binary (with the three seam files NAMED-excluded there), NOT part of the frozen `all_twelve`
/// architecture ratchet — a seam-specific lint modeled on `no-raw-publish`, with its own red/green
/// fixtures and self-tests.
pub fn no_raw_ci_verdict() -> Lint {
    Lint {
        id: NO_RAW_CI_VERDICT,
        rule: "no CI stage verdict (SignalPayload::CiJobDone / signal_typed) outside the sanctioned \
               reporter + flow-executor seam",
        scan: scan_no_raw_ci_verdict,
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
/// **P-GA-03 reconciliation (P-051, the GDPR-OWNED slice; coherence rule EI-01 §7).** The
/// `no-untagged-personal-data` lint, its engine, and its red/green fixtures were FIRST shipped by
/// the substrate prompt P-S10 → P-017 (the four load-bearing architecture lints — the lint harness
/// is shared substrate, and contract-index 1.6 assigns this lint to GDPR). P-GA-03 is the GDPR
/// owner's realization of the SAME contract-1.6 lint: it must enforce presence of the ACTUAL frozen
/// `#[personal_data(...)]` attribute — the full SIX-TAG keyword form frozen in P-GA-02 / P-050
/// (`category | role | basis | retention | erasure | subject_locator`; gdpr-and-audit §2.1). The
/// P-017 floor only recognized the SINGLE-LINE attribute form (attribute on the line directly above
/// the field); it FALSELY REJECTED the canonical MULTI-LINE tag (the closing `)]` is the line above
/// the field, not the `#[personal_data(` opener) — every M1 store using the real tag would have
/// failed the build. P-GA-03 SHARPENS the scanner to track the `#[personal_data(...)]` attribute's
/// multi-line bracket span so the frozen shape is admitted, while an UNtagged PII field still fails
/// (code-wins-over-docs, EI-01 §1 — a lint that rejects the contract's own frozen attribute is the
/// bug). GDPR-owned red+green fixtures (`no_untagged_personal_data.gdpr.{red,green}`) prove both
/// verdicts over the canonical form (`tests/gdpr_audit_lints.rs`). The lint is SHARPENED, never
/// weakened (EI-01 §5).
///
/// **Floor (FILLED by P-GA-07 / P-107).** The type-system form — a `#[derive(PersonalData)]`
/// classify-derive that REFUSES TO EXPAND a schema with an untagged PII field (a hard
/// `compile_error!`) — landed in P-107 (`myelin-gdpr-macros::derive_personal_data`, the
/// `compile_fail` doc-test on `myelin_gdpr`). The two are now BELT-AND-BRACES: this SOURCE scanner
/// forces the tag for ANY struct anywhere (a schema author can forget the derive); the macro
/// additionally forbids an untagged PII field on a struct that DOES derive `PersonalData`. (Through
/// the P-GA-02 / P-050 floor the derive was a NO-OP, so this scanner alone FORCED the tag; from
/// P-107 the macro forces it too on a deriving struct.)
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
    // Whether the field about to be scanned is preceded by a `#[personal_data(...)]` attribute.
    // This must survive the MULTI-LINE attribute form the §2.1 / P-GA-02 (P-050) frozen tag uses:
    //
    //     #[personal_data(
    //         category = ContactInfo,
    //         ...
    //         subject_locator = "principal_id",
    //     )]
    //     email: EncryptedField<Email>,
    //
    // A naive "was the IMMEDIATELY-preceding line a `#[personal_data` line?" check (the original
    // P-S10 / P-017 floor) FALSELY REJECTS this canonical shape: the line directly above the field
    // is the attribute's closing `)]`, not the `#[personal_data(` opener, so the tag is missed and
    // every store using the real frozen tag fails the build. The P-GA-03 owner (this prompt) is the
    // one whose CANON docs name that exact multi-line attribute, so it fixes the scanner here
    // (EI-01 §1, code-wins-over-docs: a lint that rejects the contract's own frozen attribute is the
    // bug). We track the open `#[ ... ]` attribute bracket span: once `#[personal_data` opens, the
    // field is treated as tagged until the attribute's brackets balance AND the next field line is
    // consumed. The lint is SHARPENED (it now admits the real frozen shape) — never weakened: an
    // UNtagged PII field still fails (the red fixtures prove it).
    //
    // `attr_open` = how many `[` of an in-flight `#[personal_data(...)]` attribute are still open
    // (the attribute spans lines while this is > 0). `field_is_tagged` latches true the moment a
    // `#[personal_data` attribute begins and stays true until the next struct FIELD is scanned.
    let mut attr_bracket_depth: i32 = 0;
    let mut field_is_tagged = false;

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

        // Open / continue a `#[personal_data(...)]` attribute span. The attribute may be single-line
        // (`#[personal_data(contact)]`, brackets balance on this line) or multi-line (the §2.1 form,
        // brackets stay open across the tag arguments). Once seen, the upcoming field is tagged.
        if trimmed.contains("#[personal_data") {
            field_is_tagged = true;
            attr_bracket_depth = 0; // start counting THIS attribute's bracket span fresh.
        }
        // Track the attribute's `[`/`]` balance only while an attribute is in flight (latched by
        // `field_is_tagged` with no field consumed yet) — so unrelated brackets elsewhere are
        // ignored. A `#[derive(PersonalData)]` line uses `()` not `[ ... ]` args, so it never opens
        // a span; only the `#[personal_data(...)]` helper does.
        if field_is_tagged {
            attr_bracket_depth +=
                code.matches('[').count() as i32 - code.matches(']').count() as i32;
        }

        // Check fields BEFORE updating brace depth so a field on the `struct X {` line is rare;
        // fields are on their own lines inside the body. A field is "tagged" iff a `#[personal_data]`
        // attribute opened above it AND its bracket span has closed (`attr_bracket_depth <= 0`) by
        // the time we reach the field line — i.e. the whole (possibly multi-line) attribute precedes
        // the field, never bleeding into the field's own line.
        if in_struct && depth >= struct_brace_depth {
            if let Some(field_name) = field_identifier(trimmed) {
                let tagged = field_is_tagged && attr_bracket_depth <= 0;
                if PII_FIELDS.contains(&field_name) && !tagged {
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
                // A struct FIELD line resets the tag latch: the attribute (if any) belonged to THIS
                // field; the next field starts untagged unless it carries its own attribute.
                field_is_tagged = false;
                attr_bracket_depth = 0;
            }
        }

        // Update brace depth for the NEXT line.
        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;
        depth += opens - closes;
        // Leave the struct body when brace depth drops below it. NOTE: this must be
        // `< struct_brace_depth` (not `- 1`): for a TOP-LEVEL struct, `struct_brace_depth == 1`, so
        // after the closing `}` brings `depth` to 0 the old `0 < 0` never fired and `in_struct`
        // latched true for the rest of the file — making every later `ident: Type` line (fn params,
        // struct literals) eligible for PII-flagging once they sit on their own line (e.g. after
        // `cargo fmt` reflows a long signature). Resetting at `< struct_brace_depth` confines the
        // scan to the actual struct-definition body.
        if in_struct && depth < struct_brace_depth {
            in_struct = false;
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
/// **Hot-table tightening (P-S15 / P-032, §9.4).** A migration source declares its hot tables
/// inline with a `-- @hot-table NAME` directive (the source-scan mirror of the `AppSpec`
/// `HotTables` declaration the migration runner reads at boot, `myelin-substrate`). On a
/// declared-hot table, a blocking change is rejected: the obviously-blocking forms (an
/// `ADD … NOT NULL` without DEFAULT, an in-place `ALTER COLUMN`) AND a **non-concurrent
/// `CREATE INDEX`** (which a cold table absorbs but a hot table cannot — it locks writes at QPS).
/// A hot-table change must be expand→backfill→contract; the nullable-add expand step + a
/// `CREATE INDEX CONCURRENTLY` stay admitted. On a non-declared (cold) table only the
/// table-INDEPENDENT obviously-blocking `ADD … NOT NULL` (no DEFAULT) / in-place `ALTER COLUMN`
/// fires (a cold table absorbs the brief index lock).
///
/// **Floor (named).** The full per-subsystem hot-table sets are measured-not-predicted (M1+);
/// each subsystem declares its set (`@hot-table` in its migration source + `AppSpec::hot_tables`).
pub const FORWARD_ONLY_MIGRATION: LintId = LintId("forward-only-migration");

/// Parse the `-- @hot-table NAME` declarations out of a migration source (the source-scan mirror
/// of the `AppSpec` hot-table declaration the runner reads, §9.4). Each directive flags one table
/// hot for the per-table tightening below.
fn declared_hot_tables(src: &str) -> std::collections::BTreeSet<String> {
    let mut hot = std::collections::BTreeSet::new();
    for raw in src.lines() {
        let t = raw.trim();
        // Accept `-- @hot-table NAME`, `@hot-table NAME`, `-- @hot-table: NAME`.
        if let Some(rest) = t
            .strip_prefix("-- @hot-table")
            .or_else(|| t.strip_prefix("@hot-table"))
        {
            let name = rest
                .trim_start_matches([':', ' '])
                .split_whitespace()
                .next();
            if let Some(name) = name {
                if !name.is_empty() {
                    hot.insert(name.to_ascii_lowercase());
                }
            }
        }
    }
    hot
}

/// Whether a DDL line is a blocking change (table-lock-under-load bug class). The
/// table-INDEPENDENT half always fires: a blocking `ALTER` (ADD COLUMN … NOT NULL without
/// DEFAULT, or in-place ALTER COLUMN). `hot` ADDS the per-hot-table half (§9.4): a non-concurrent
/// `CREATE INDEX` — fine on a cold table, but on a hot table it locks writes at QPS and must be
/// `CREATE INDEX CONCURRENTLY` (the expand step). A NULLABLE add (the legitimate expand step) is
/// admitted on a hot table; it is the expand→backfill→contract path, not a blocking change.
fn is_blocking_ddl(lower: &str, hot: bool) -> bool {
    let alter_adds_not_null = lower.contains("alter table")
        && lower.contains("add column")
        && lower.contains("not null")
        && !lower.contains("default");
    let alter_column_inplace = lower.contains("alter table") && lower.contains("alter column");
    if alter_adds_not_null || alter_column_inplace {
        return true;
    }
    if hot {
        let non_concurrent_index =
            lower.contains("create index") && !lower.contains("concurrently");
        return non_concurrent_index;
    }
    false
}

fn scan_forward_only_migration(src: &str) -> Vec<Violation> {
    let hot_tables = declared_hot_tables(src);
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
        // (b) A blocking change. Whether this line targets a DECLARED-HOT table widens what
        // counts as blocking (§9.4): on a hot table ANY ALTER / non-concurrent index is blocking.
        let targets_hot = hot_tables.iter().any(|t| lower.contains(t.as_str()));
        if is_blocking_ddl(&lower, targets_hot) {
            let reason = if targets_hot {
                "a blocking change (ALTER TABLE / non-concurrent CREATE INDEX) on a DECLARED-HOT \
                 table (`@hot-table`, §9.4) takes a table lock at write QPS — a hot-table change \
                 must be expand→backfill→contract (nullable add → throttled backfill → constrain), \
                 never one blocking step (forward-only-migration/§9.4)."
            } else {
                "a blocking `ALTER TABLE` (ADD COLUMN ... NOT NULL without DEFAULT, or ALTER \
                 COLUMN in place) takes a table lock — on a hot table this stalls writes; use the \
                 expand→backfill→contract idiom (nullable add, backfill, then constrain) \
                 (forward-only-migration/§9)."
            };
            out.push(Violation {
                lint: FORWARD_ONLY_MIGRATION,
                line,
                reason: reason.into(),
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
///
/// **EB-08 reconciliation (P-044, the Bus's OWNED slice; coherence rule EI-01 §7).** The
/// `no-cross-sync-cycle` lint, its engine, and its `@identity-sink` red/green fixtures were first
/// shipped by the SUBSTRATE prompt P-S11 → P-018 (the remaining eight architecture lints; the lint
/// harness is shared substrate). That P-018 form is the *Identity-sink* half: it fires ONLY inside
/// an `@identity-sink`-marked file (Identity originates no sync cross-service call). EB-08 is the
/// Bus's OWNED slice of the SAME contract-1.6 lint, and its CANON docs name the BROADER, canonical
/// rule (refined-arch event-bus §7.1 + 00-reconciliation §X-1 line 117-118): *the bus is cell-local;
/// heavy cross-system work is async off the bus, never synchronous in the write path — CI emits,
/// Git reads; Git never synchronously calls CI to ask "is it green," it reads its own projection.*
/// This is a genuinely DISTINCT bug fingerprint from the Identity-sink leg: here the originator is
/// ANY subsystem (Git, Issues, Chat …), the call is a synchronous cross-subsystem RPC asking
/// another subsystem for its state INSIDE A WRITE PATH, and the sanctioned alternative is a
/// projection read / a bus reaction (an `EventHandler`), never a sync call. Rather than duplicate a
/// parallel scanner (EI-01 §7), the in-place scanner is EXTENDED with this second leg — keyed to a
/// loud, named `// @write-path` marker (the same marker discipline `@identity-sink` /
/// `@workflow-body` / `@residency-write` use) so it fires ONLY where a write path is being scanned,
/// and admits the whole current (no-write-path-yet) workspace until the producer write paths land
/// (Git M3 GIT-P*, the merge-gate consumer). The lint is SHARPENED (it now also guards the general
/// cross-subsystem write-path acyclicity), never weakened (EI-01 §5).
///
/// **Floor (named) — the runtime acyclicity drill is later-band.** This is the *lint leg* (the
/// compile-time rejection of the bug fingerprint) only. The full call-graph-acyclicity check across
/// ALL service pairs rides the resilient inter-service client (`SyncClient`, P-S16 / P-033) + the
/// per-edge sync-call registry; the live Git→CI "reads its own projection, never a sync call" path
/// lands with the X-1 check-seam consumer (Git M3). The marker-keyed scanner ships the gate NOW so
/// the Git-synchronously-calls-CI bug fingerprint is un-mergeable before that write path exists.
pub const NO_CROSS_SYNC_CYCLE: LintId = LintId("no-cross-sync-cycle");

/// A synchronous OUTBOUND cross-service/cross-subsystem call fingerprint. Shared by BOTH legs of
/// `no-cross-sync-cycle`: the Identity-sink leg (forbidden inside `@identity-sink`) and the EB-08
/// write-path leg (forbidden inside `@write-path`). The reactive/bus path (`.emit(`, an
/// `EventHandler`, a projection read) is NOT a sync call and is always admitted.
const SYNC_OUTBOUND_SITES: &[&str] = &[
    ".call_sync(",
    ".sync_call(",
    "SyncServiceClient",
    ".rpc_call(",
    "reqwest::Client",
    ".send_request(",
];

fn scan_no_cross_sync_cycle(src: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    // ---- Leg 1: the Identity-sink half (P-S11 → P-018). ----------------------------------------
    // Fires INSIDE the identity crate (the sink). Because the scanner is a pure fn of source text,
    // we key off an in-source sink marker the identity crate's modules carry: a `//! IDENTITY-SINK`
    // doc-line, OR the explicit `// @identity-sink` fixture marker. From Identity, ANY sync outbound
    // cross-service call is forbidden (Identity calls no one synchronously).
    let is_identity_sink = src.contains("@identity-sink") || src.contains("IDENTITY-SINK");
    if is_identity_sink {
        for (line, code) in code_lines(src) {
            for site in SYNC_OUTBOUND_SITES {
                if code.contains(site) {
                    out.push(Violation {
                        lint: NO_CROSS_SYNC_CYCLE,
                        line,
                        reason: format!(
                            "a synchronous outbound cross-service call `{site}` originates from \
                             Identity — Identity is the SINK of the sync call graph (everyone may \
                             call Identity synchronously; Identity calls no one synchronously). \
                             React over the bus instead so the sync call graph stays acyclic \
                             (EI-02 §3)."
                        ),
                    });
                }
            }
        }
    }
    // ---- Leg 2: the EB-08 write-path half (the Bus's OWNED slice, P-044). ----------------------
    out.extend(scan_cross_sync_in_write_path(src));
    out
}

/// The EB-08 write-path leg of `no-cross-sync-cycle` (P-044, the Bus's owned slice). Inside a
/// write-path site (a file/line marked `// @write-path`), a SYNCHRONOUS cross-subsystem call asking
/// another subsystem for its state — the "is it green?" sync call — is rejected. The bus is
/// cell-local; heavy cross-system work is async OFF the bus, never synchronous in the write path
/// (refined-arch event-bus §7.1, ADR-11.5); CI emits, Git reads its own projection — Git never
/// synchronously calls CI (00-reconciliation §X-1). This is a DISTINCT fingerprint from the
/// Identity-sink leg: the originator is any subsystem, scoped by the `@write-path` marker so the
/// whole current workspace (no producer write path yet) is admitted until those paths land.
fn scan_cross_sync_in_write_path(src: &str) -> Vec<Violation> {
    const WRITE_MARKER: &str = "@write-path";
    if !src.contains(WRITE_MARKER) {
        return Vec::new();
    }
    // A module-doc (`//!`) or top-of-file marker arms the whole file; otherwise each offending
    // statement needs the marker in its attached comment block (per-line arming), so the leg never
    // fires on an unmarked statement.
    let raw_lines: Vec<&str> = src.lines().collect();
    let file_armed = raw_lines.iter().any(|l| {
        let t = l.trim_start();
        t.starts_with("//!") && t.contains(WRITE_MARKER)
    });
    let mut out = Vec::new();
    for (line, code) in code_statements(src) {
        let Some(site) = SYNC_OUTBOUND_SITES.iter().find(|s| code.contains(**s)) else {
            continue;
        };
        // Per-line arming: the marker on the statement's start line, in the attached comment block
        // directly above it, or file-level (shared with the EB-09 leg via `is_marker_armed`).
        let site_armed = is_marker_armed(&raw_lines, line, file_armed, WRITE_MARKER);
        if site_armed {
            out.push(Violation {
                lint: NO_CROSS_SYNC_CYCLE,
                line,
                reason: format!(
                    "a synchronous cross-subsystem call `{site}` in a WRITE PATH asks another \
                     subsystem for its state (the \"is it green?\" sync call) — the bus is \
                     cell-local; heavy cross-system work is async OFF the bus, never synchronous in \
                     the write path (refined-arch event-bus §7.1, ADR-11.5). CI emits, Git reads \
                     its OWN projection; Git never synchronously calls CI (00-reconciliation §X-1). \
                     React over the bus / read a projection so the cross-subsystem dependency stays \
                     acyclic (EI-02 §3)."
                ),
            });
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
        "OltpPool::open(", // the real OLTP pool constructor (myelin-storage, P-ST-01).
        "ColocatedOltp::open(", // the real co-located OLTP+outbox store constructor (P-ST-02).
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
        let here = raw_lines
            .get(idx)
            .is_some_and(|l| l.contains(WAIVER_MARKER));
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
        let site_armed =
            file_armed || raw_lines.get(idx).is_some_and(|l| l.contains(WRITE_MARKER)) || {
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
/// **Rule (canonical, refined-arch tenancy §4.3).** *No control-plane registry column is classified
/// `is_personal=true` — run through the generated data-map.* The control plane (routing, cross-cell
/// pointers, placement, directory) carries OPAQUE IDS ONLY (`TenantId`/`Region`/slug/hash) — never a
/// name/email/body. PII is born inside the cell, never in the control plane (ADR-11.4; §3.3 "the
/// control plane holds ZERO in-region personal data").
///
/// **Two fingerprints scanned**, on a control-plane struct (one marked `// @control-plane` or named
/// `*Pointer`/`*Routing`/`*Placement`/`CrossCell*`/`*Directory`):
///   1. **the data-map leg (P-CP-04, the canonical rule):** ANY field classified
///      `is_personal=true` — i.e. tagged with the GDPR `#[personal_data(...)]` classify-derive
///      (contract 10.2, the generated data-map) — fires the lint, *regardless of the field name*.
///      This is the authoritative leg: the data-map, not a name guess, is the source of truth for
///      `is_personal`. A `#[personal_data]` tag does NOT make PII admissible on the control plane —
///      it makes the leak *machine-detectable* (the same field would be erasable inside a cell, but
///      the control plane must carry none).
///   2. **the name-fingerprint leg (defence-in-depth, P-S11 substrate floor):** a PII-named field
///      (`name`, `email`, `phone`, `address`, `body`, `display_name`, …) fires even when the author
///      forgot the `#[personal_data]` tag (caught independently by `no-untagged-personal-data`).
///
/// **P-CP-04 frame guard.** The frozen `CrossCellPointer` frame (P-CP-02 / P-027) is `@control-plane`
/// by name (`CrossCell*`); the lint asserts it carries no `is_personal=true` field — a fifth
/// PII-bearing field added to the four-field frame fails the build (refined-arch §6.1).
///
/// **History (EI-01 §7 — sharpened in place, never duplicated).** P-S11 / P-018 shipped this gate
/// keyed to the `@control-plane` marker + the naming fingerprint (the name-fingerprint leg).
/// **P-CP-04 / P-028 (Tenancy ownership) SHARPENS it with the data-map leg** — the canonical
/// `is_personal=true` classification — so a PII column escapes neither by name NOR by tag, and adds
/// the Tenancy twin fixtures over the real `CrossCellPointer` frame (`tests/tenancy_control_plane_lints.rs`).
/// The lint is sharpened, never weakened (EI-01 §5). The live registry-schema CP-D1 drill (the
/// `cell`/`tenant_placement`/`cell_provisioning` tables asserted at 0 PII columns) is the M1
/// follow-on **P-CP-05 / P-080**.
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
    // The data-map leg (P-CP-04): track whether the immediately-preceding non-blank code line was a
    // `#[personal_data(...)]` classify-derive (contract 10.2). A control-plane field carrying that
    // tag is classified `is_personal=true` and fires the lint REGARDLESS of its name — the data-map,
    // not a name guess, is the authoritative `is_personal` signal (refined-arch tenancy §4.3).
    let mut prev_was_personal_data = false;
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
                // Leg 1 — the data-map leg (P-CP-04, canonical): the field is classified
                // `is_personal=true` (a `#[personal_data(...)]` tag on the line just above). Fires
                // regardless of the field's name.
                if prev_was_personal_data {
                    out.push(Violation {
                        lint: CONTROL_PLANE_PII_FREE,
                        line: *line,
                        reason: format!(
                            "control-plane struct carries `is_personal=true` field `{field_name}` \
                             (tagged `#[personal_data(...)]`, the generated data-map) — no \
                             control-plane registry column may be classified `is_personal=true` \
                             (refined-arch tenancy §4.3). The control plane carries OPAQUE IDS ONLY \
                             (TenantId/Region/slug/hash); PII is born inside the cell, never here \
                             (ADR-11.4/OQ-I). Tagging does NOT make PII admissible on the control \
                             plane — it makes the leak machine-detectable."
                        ),
                    });
                } else if PII_FIELDS.contains(&field_name) {
                    // Leg 2 — the name-fingerprint leg (defence-in-depth): an untagged PII-named
                    // field on the control plane (the author forgot the data-map tag too).
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
        // Update the data-map flag for the NEXT field line (mirrors `scan_no_untagged_personal_data`):
        // a `#[personal_data` attr line classifies the field on the line below it; blank lines are
        // ignored so the tag still applies to the next real field.
        if !trimmed.is_empty() {
            prev_was_personal_data = trimmed.contains("#[personal_data");
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
    fn tenant_predicate_admits_only_an_adjacent_explained_cross_scope_query() {
        let green = r#"
            // @tenant-cross-scope: PostgreSQL role catalog has no tenant rows.
            let row = sqlx::query("SELECT current_user FROM pg_roles").fetch_one(&pool);
        "#;
        let red = r#"
            // @tenant-cross-scope: applies only to the catalog query below.
            let catalog_name = "pg_roles";
            let rows = sqlx::query("SELECT * FROM principals").fetch_all(&pool);
        "#;
        assert!(tenant_predicate().run(green).is_empty());
        assert!(!tenant_predicate().run(red).is_empty());
    }

    #[test]
    fn tenant_predicate_is_hoist_invariant_over_composed_sql() {
        // A COMPOSED query (the predicate assembled with `format!`, then executed from a local) is
        // tenant-bound exactly when its SQL binds the tenant — the same verdict the inline form
        // gets. The green case is the shape CT-007 introduced in `settle_ci_job_surface_on_conn`.
        let green = r#"
            let query = format!(
                "UPDATE ci_job
                 SET state=$5
                 WHERE tenant_id=$1 AND region=$2
                   AND ({state_predicate})
                 RETURNING job_id"
            );
            let updated = sqlx::query_scalar::<_, Uuid>(&query)
                .bind(tenant.as_str())
                .fetch_optional(&mut *conn);
        "#;
        assert!(
            tenant_predicate().run(green).is_empty(),
            "a hoisted SQL string that BINDS the tenant must be admitted"
        );

        // ANTI-WEAKENING: hoisting the SQL must NOT launder a tenant-less query.
        let red = r#"
            let query = format!(
                "UPDATE ci_job
                 SET state=$1
                 WHERE run_id=$2::uuid
                 RETURNING job_id"
            );
            let updated = sqlx::query_scalar::<_, Uuid>(&query)
                .bind(state)
                .fetch_optional(&mut *conn);
        "#;
        assert!(
            !tenant_predicate().run(red).is_empty(),
            "a hoisted SQL string with NO tenant predicate must still be rejected"
        );

        // The resolution follows the argument identifier only — an unrelated tenant-bound local
        // does not launder a query that executes a DIFFERENT, tenant-less SQL string.
        let red_other_local = r#"
            let tenant_sql = "SELECT id FROM ci_job WHERE tenant_id=$1";
            let query = "SELECT id FROM ci_job";
            let rows = sqlx::query(&query).fetch_all(&pool);
        "#;
        assert!(
            !tenant_predicate().run(red_other_local).is_empty(),
            "only the SQL the query actually executes may admit it"
        );

        // The NEAREST PRECEDING binding wins: a tenant-bound earlier definition must not admit a
        // query that runs the later, tenant-less rebinding of the same name.
        let red_shadowed = r#"
            let query = "SELECT id FROM ci_job WHERE tenant_id=$1";
            let query = "SELECT id FROM ci_job";
            let rows = sqlx::query(&query).fetch_all(&pool);
        "#;
        assert!(
            !tenant_predicate().run(red_shadowed).is_empty(),
            "a shadowed tenant-bound binding must not admit the later tenant-less one"
        );
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
        assert!(
            !no_raw_publish().run(red).is_empty(),
            "transport.put( must be rejected"
        );
        assert!(
            !no_raw_publish().run(red_bus).is_empty(),
            "bus.put( must be rejected"
        );
        assert!(
            !no_raw_publish().run(red_broker).is_empty(),
            "broker.put( must be rejected"
        );
    }

    #[test]
    fn no_raw_ci_verdict_rejects_forged_verdict_admits_reporter_path() {
        // The forge fingerprints: constructing the typed verdict OR calling the delivery seam.
        let red_construct =
            "let p = SignalPayload::CiJobDone { stage, passed: true, result_refs };";
        let red_deliver = "executor.signal_typed(spec)?;";
        assert!(
            !no_raw_ci_verdict().run(red_construct).is_empty(),
            "constructing SignalPayload::CiJobDone must be rejected"
        );
        assert!(
            !no_raw_ci_verdict().run(red_deliver).is_empty(),
            "calling .signal_typed( must be rejected"
        );
        // The sanctioned path: report completion through the reporter abstraction, no raw verdict.
        let green = "reporter.report_done(&run, idem_token, &report)?;";
        assert!(
            no_raw_ci_verdict().run(green).is_empty(),
            "the reporter path must be admitted"
        );
    }

    #[test]
    fn no_raw_ci_verdict_catches_alias_ufcs_and_line_split_dodges() {
        // Bare whole-word token matching closes the alias / UFCS / line-split evasions a dotted/pathed
        // literal would miss.
        let alias = "use myelin_flow::SignalPayload::CiJobDone as Done;";
        let ufcs = "DurableExecutor::signal_typed(&executor, spec)?;";
        let line_split = "    executor\n        .signal_typed"; // the token sits on the split line
        assert!(
            !no_raw_ci_verdict().run(alias).is_empty(),
            "a `use … CiJobDone as Done` alias must be caught"
        );
        assert!(
            !no_raw_ci_verdict().run(ufcs).is_empty(),
            "a UFCS `Trait::signal_typed(` call must be caught"
        );
        assert!(
            !no_raw_ci_verdict().run(line_split).is_empty(),
            "a `.signal_typed`-then-newline line-split must be caught"
        );
    }

    #[test]
    fn no_raw_ci_verdict_word_boundary_admits_longer_identifiers() {
        // Whole-word matching does NOT flag a longer identifier that merely CONTAINS a token — the
        // internal `signal_typed_async` bridge (trailing `_`) and a `_`-prefixed identifier are admitted.
        let bridge = "bridge(&self.rt, self.signal_typed_async(spec))";
        let prefixed = "let ci_job_done_count = jobs.len();";
        assert!(
            no_raw_ci_verdict().run(bridge).is_empty(),
            "the internal signal_typed_async bridge must be admitted (word boundary)"
        );
        assert!(
            no_raw_ci_verdict().run(prefixed).is_empty(),
            "a longer identifier containing the token as a fragment must be admitted"
        );
        // The trait/impl DEFINITION `fn signal_typed(` IS a whole-word token and IS flagged by the
        // scan — it is protected only by the per-lint exclusion of the seam files in `lint-gate.rs`,
        // never by the scan pretending not to see it.
        let def = "fn signal_typed(&self, spec: TypedSignalSpec) -> Result<SignalOutcome, E> {";
        assert!(
            !no_raw_ci_verdict().run(def).is_empty(),
            "the definition is flagged by the scan; only the per-lint exclusion admits the seam file"
        );
    }

    #[test]
    fn no_raw_publish_admits_unrelated_put_calls() {
        // The fingerprint is HANDLE-QUALIFIED (transport./bus./broker. prefix), so an unrelated
        // `.put(` — e.g. a byte-buffer `BufMut::put`, a cache `.put(k, v)` — is NOT flagged.
        let buf = "buf.put(&bytes[..]);";
        let cache = "cache.put(key, value);";
        assert!(
            no_raw_publish().run(buf).is_empty(),
            "BufMut::put must be admitted"
        );
        assert!(
            no_raw_publish().run(cache).is_empty(),
            "an unrelated cache.put must be admitted"
        );
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

    /// **The hot-table tightening (P-S15/P-032, §9.4).** Once a migration source declares a table
    /// hot (`-- @hot-table NAME`), the lint reads that declaration and forbids a **non-concurrent
    /// `CREATE INDEX`** on it (fine on a cold table, but at write QPS on a hot table it must be
    /// `CONCURRENTLY`) — the per-hot-table half on TOP of the table-independent obviously-blocking
    /// `ALTER` forms. The nullable-add expand step + a `CREATE INDEX CONCURRENTLY` stay admitted.
    #[test]
    fn forward_only_migration_tightens_to_per_hot_table_when_declared() {
        // (1) A non-concurrent CREATE INDEX on a declared-HOT table is rejected; on a non-declared
        //     (cold) table the same index is admitted (it absorbs the brief lock).
        let hot_index = "-- @hot-table issue\nCREATE INDEX idx_issue_status ON issue (status);";
        let cold_index = "CREATE INDEX idx_archive_status ON audit_archive (status);";
        assert!(
            !forward_only_migration().run(hot_index).is_empty(),
            "a non-concurrent CREATE INDEX on a DECLARED-HOT table must be rejected (§9.4)"
        );
        assert!(
            forward_only_migration().run(cold_index).is_empty(),
            "the same non-concurrent index on a non-hot table is admitted"
        );

        // (2) CREATE INDEX CONCURRENTLY (the expand step) is admitted even on a hot table.
        let hot_index_concurrent =
            "-- @hot-table issue\nCREATE INDEX CONCURRENTLY idx_issue_status ON issue (status);";
        assert!(forward_only_migration()
            .run(hot_index_concurrent)
            .is_empty());

        // (3) The expand step (a NULLABLE add) is admitted even on a hot table.
        let hot_expand = "-- @hot-table issue\nALTER TABLE issue ADD COLUMN priority INT;";
        assert!(
            forward_only_migration().run(hot_expand).is_empty(),
            "the expand step (nullable add) is admitted on a hot table"
        );

        // (4) The obviously-blocking ALTER forms still fire on a hot table (the table-independent
        //     half is preserved).
        let hot_blocking_alter =
            "-- @hot-table issue\nALTER TABLE issue ADD COLUMN body TEXT NOT NULL;";
        assert!(!forward_only_migration().run(hot_blocking_alter).is_empty());
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
        assert!(
            !residency_pin().run(red).is_empty(),
            "region-less open still fires"
        );
        assert!(
            residency_pin().run(green).is_empty(),
            "region-pinned open still admits"
        );
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
