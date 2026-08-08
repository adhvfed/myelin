use crate::engine::{blank_string_literals, code_lines, code_statements, Lint, LintId, Violation};

pub const TENANT_PREDICATE: LintId = LintId("tenant-predicate");

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

const BUS_SCOPE_BINDERS: &[&str] = &[
    "StreamScope",
    "TenantId",
    "tenant_id",
    "subsystem",
    "Subsystem",
    "scoped_stream",
    "Scope::Tenant",
    ", scope",
    "(scope",
    "scope)",
    "scope,",
    ".scope(",
];

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

const BUS_WILDCARD_SUBJECTS: &[&str] = &[
    "\"evt.>\"",
    "\"evt.*\"",
    "\".>\"",
    "\"*\"",
    "\">\"",
    "\"evt.*.*\"",
];

fn scan_tenant_predicate_bus_streams(src: &str) -> Vec<Violation> {
    const STREAM_MARKER: &str = "@bus-stream";
    if !src.contains(STREAM_MARKER) {
        return Vec::new();
    }
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
        let raw_stmt = raw_statement_text(&raw_lines, line);
        let blanked = blank_string_literals(&code);
        let has_wildcard_scope = BUS_WILDCARD_SCOPES.iter().any(|w| {
            blanked.contains(*w) || code.contains(*w)
        });
        let has_wildcard_subject = BUS_WILDCARD_SUBJECTS.iter().any(|w| raw_stmt.contains(*w));
        let is_scoped = BUS_SCOPE_BINDERS.iter().any(|b| code.contains(b));
        if has_wildcard_scope || has_wildcard_subject {
            out.push(Violation {
                lint: TENANT_PREDICATE,
                line,
                reason: format!(
                    "a bus subscribe/stream `{site}` uses a WILDCARD / unbounded scope - `scope` is \
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
                    "a bus subscribe/stream `{site}` has no (tenant, subsystem) scope - a stream is \
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

fn hoisted_sql_statement<'a>(
    statements: &'a [(usize, String)],
    index: usize,
    code: &str,
    query_sites: &[&str],
) -> Option<&'a str> {
    let site_end = query_sites
        .iter()
        .filter_map(|site| {
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
    let tail = rest[end..].trim();
    (tail.is_empty() || tail.starts_with('.')).then_some(ident)
}

fn binds_identifier(statement: &str, ident: &str) -> bool {
    let bytes = statement.as_bytes();
    let mut cursor = 0usize;
    while let Some(at) = statement[cursor..].find("let ") {
        let start = cursor + at;
        cursor = start + 4;
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

fn with_tenant_tx_regions(src: &str) -> Vec<(usize, usize)> {
    const OPENER: &str = "with_tenant_tx(";
    let lines: Vec<(usize, String)> = code_lines(src)
        .into_iter()
        .map(|(line, code)| (line, blank_string_literals(&code)))
        .collect();
    let mut regions = Vec::new();
    let mut depth = 0usize;
    let mut start_line = 0usize;
    for (line, code) in &lines {
        let bytes = code.as_bytes();
        let mut index = 0usize;
        while index < bytes.len() {
            if depth == 0 {
                let word_start = index == 0
                    || !matches!(bytes[index - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_');
                if word_start && bytes[index..].starts_with(OPENER.as_bytes()) {
                    start_line = *line;
                    index += OPENER.len();
                    depth = 1;
                    continue;
                }
                index += 1;
            } else {
                match bytes[index] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            regions.push((start_line, *line));
                        }
                    }
                    _ => {}
                }
                index += 1;
            }
        }
    }
    regions
}

fn scan_tenant_predicate(src: &str) -> Vec<Violation> {
    const QUERY_SITES: &[&str] = &[
        "sqlx::query",
        "QueryBuilder::new",
        ".from(",
        "query_as!",
        "query!(",
    ];
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
    const CROSS_SCOPE_MARKER: &str = "@tenant-cross-scope:";
    const MARKER_LOOKBACK: usize = 8;
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let statements = code_statements(src);
    let tenant_tx_regions = with_tenant_tx_regions(src);
    for (index, (line, code)) in statements.iter().enumerate() {
        let (line, code) = (*line, code.as_str());
        let is_query = QUERY_SITES.iter().any(|s| code.contains(s));
        if !is_query {
            continue;
        }
        let is_tenant_bound = TENANT_BINDERS.iter().any(|b| code.contains(b))
            || hoisted_sql_statement(&statements, index, code, QUERY_SITES)
                .is_some_and(|sql| TENANT_BINDERS.iter().any(|b| sql.contains(b)))
            || tenant_tx_regions
                .iter()
                .any(|(start, end)| line >= *start && line <= *end);
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
                reason: "query-builder call has no TenantId bound - every tenant-store query \
                         must thread the tenant predicate (the RLS guard / a TenantId arg / a \
                         WHERE tenant clause). A tenant-less query is a cross-tenant IDOR (F2)."
                    .into(),
            });
        }
    }
    out.extend(scan_tenant_predicate_bus_streams(src));
    out
}

pub fn tenant_predicate() -> Lint {
    Lint {
        id: TENANT_PREDICATE,
        rule: "every query-builder call carries a TenantId bound; a tenant-less query is rejected",
        scan: scan_tenant_predicate,
    }
}

pub const NO_RAW_PUBLISH: LintId = LintId("no-raw-publish");

fn scan_no_raw_publish(src: &str) -> Vec<Violation> {
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
                        "raw bus publish `{site}` bypasses OutboxTx::emit - there is NO \
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

pub fn no_raw_publish() -> Lint {
    Lint {
        id: NO_RAW_PUBLISH,
        rule: "no bus publish outside OutboxTx::emit; no fire-and-forget publish path",
        scan: scan_no_raw_publish,
    }
}

pub const NO_RAW_CI_VERDICT: LintId = LintId("no-raw-ci-verdict");

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
                         body trusts as the runner's verified result - a typed `job.done` verdict \
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

pub fn no_raw_ci_verdict() -> Lint {
    Lint {
        id: NO_RAW_CI_VERDICT,
        rule: "no CI stage verdict (SignalPayload::CiJobDone / signal_typed) outside the sanctioned \
               reporter + flow-executor seam",
        scan: scan_no_raw_ci_verdict,
    }
}

pub const NO_HOST_EXEC: LintId = LintId("no-host-exec");

fn scan_no_host_exec(src: &str) -> Vec<Violation> {
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
                         sandbox) - no platform code may shell out to the host kernel directly; \
                         all execution goes through the sandbox seam so the four uniform \
                         guarantees hold (X-6/AG-2)."
                    ),
                });
            }
        }
    }
    out
}

pub fn no_host_exec() -> Lint {
    Lint {
        id: NO_HOST_EXEC,
        rule: "no host-execution path bypassing ToolHands::exec (the unified sandbox)",
        scan: scan_no_host_exec,
    }
}

pub const NO_UNTAGGED_PERSONAL_DATA: LintId = LintId("no-untagged-personal-data");

fn scan_no_untagged_personal_data(src: &str) -> Vec<Violation> {
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
    let mut depth: i32 = 0;
    let mut in_struct = false;
    let mut struct_brace_depth: i32 = 0;
    let mut attr_bracket_depth: i32 = 0;
    let mut field_is_tagged = false;

    for (line, code) in &lines {
        let trimmed = code.trim();

        let opens_struct = trimmed.starts_with("struct ")
            || trimmed.starts_with("pub struct ")
            || trimmed.contains(" struct ");
        if opens_struct && code.contains('{') {
            in_struct = true;
            struct_brace_depth = depth + 1;
        }

        if trimmed.contains("#[personal_data") {
            field_is_tagged = true;
            attr_bracket_depth = 0;
        }
        if field_is_tagged {
            attr_bracket_depth +=
                code.matches('[').count() as i32 - code.matches(']').count() as i32;
        }

        if in_struct && depth >= struct_brace_depth {
            if let Some(field_name) = field_identifier(trimmed) {
                let tagged = field_is_tagged && attr_bracket_depth <= 0;
                if PII_FIELDS.contains(&field_name) && !tagged {
                    out.push(Violation {
                        lint: NO_UNTAGGED_PERSONAL_DATA,
                        line: *line,
                        reason: format!(
                            "PII field `{field_name}` is not #[personal_data(...)]-tagged - every \
                             schema field carrying personal data must be tagged so the \
                             crypto-shred erase + the RoPA/data-map fan-out reach it; an untagged \
                             PII column leaves an un-erasable subject (ADR-12)."
                        ),
                    });
                }
                field_is_tagged = false;
                attr_bracket_depth = 0;
            }
        }

        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;
        depth += opens - closes;
        if in_struct && depth < struct_brace_depth {
            in_struct = false;
        }
    }
    out
}

fn field_identifier(trimmed: &str) -> Option<&str> {
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
    let colon = body.find(':')?;
    if body.as_bytes().get(colon + 1) == Some(&b':') {
        return None;
    }
    let ident = body[..colon].trim();
    if ident.is_empty() || !is_ident(ident) {
        return None;
    }
    Some(ident)
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub fn no_untagged_personal_data() -> Lint {
    Lint {
        id: NO_UNTAGGED_PERSONAL_DATA,
        rule: "every PII-carrying schema field is #[personal_data(...)]-tagged",
        scan: scan_no_untagged_personal_data,
    }
}

pub const NO_CROSS_DB: LintId = LintId("no-cross-db");

fn scan_no_cross_db(src: &str) -> Vec<Violation> {
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
                         (`myelin_<other>::storage|store|db|schema|repo|pool`) - services may \
                         only couple over the frozen contract crate, never a shared data path; \
                         each service owns its store and opens its own pool (ADR-01/no-cross-db)."
                    .into(),
            });
        }
    }
    out
}

pub fn no_cross_db() -> Lint {
    Lint {
        id: NO_CROSS_DB,
        rule: "a service crate must not depend on another service's storage module",
        scan: scan_no_cross_db,
    }
}

pub const FORWARD_ONLY_MIGRATION: LintId = LintId("forward-only-migration");

fn declared_hot_tables(src: &str) -> std::collections::BTreeSet<String> {
    let mut hot = std::collections::BTreeSet::new();
    for raw in src.lines() {
        let t = raw.trim();
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
    for (line, code) in code_statements(src) {
        let code = blank_string_literals(&code);
        let lower = code.to_ascii_lowercase();
        let trimmed = lower.trim();
        let is_down = lower.contains("-- down")
            || trimmed.starts_with("fn down(")
            || trimmed.starts_with("pub fn down(")
            || trimmed.contains(".down.sql")
            || (trimmed.contains("down:") && lower.contains("migration"))
            || trimmed.contains("drop column");
        if is_down {
            out.push(Violation {
                lint: FORWARD_ONLY_MIGRATION,
                line,
                reason: "a down/rollback migration is forbidden - migrations are FORWARD-ONLY \
                         (a rollback is a NEW forward migration, never a `down`/`DROP COLUMN`); \
                         use expand→backfill→contract (STOR-2/§9)."
                    .into(),
            });
        }
        let targets_hot = hot_tables.iter().any(|t| lower.contains(t.as_str()));
        if is_blocking_ddl(&lower, targets_hot) {
            let reason = if targets_hot {
                "a blocking change (ALTER TABLE / non-concurrent CREATE INDEX) on a DECLARED-HOT \
                 table (`@hot-table`, §9.4) takes a table lock at write QPS - a hot-table change \
                 must be expand→backfill→contract (nullable add → throttled backfill → constrain), \
                 never one blocking step (forward-only-migration/§9.4)."
            } else {
                "a blocking `ALTER TABLE` (ADD COLUMN ... NOT NULL without DEFAULT, or ALTER \
                 COLUMN in place) takes a table lock - on a hot table this stalls writes; use the \
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

pub fn forward_only_migration() -> Lint {
    Lint {
        id: FORWARD_ONLY_MIGRATION,
        rule: "no rollback migration file; no blocking ALTER on a flagged-hot table",
        scan: scan_forward_only_migration,
    }
}

pub const NO_CROSS_SYNC_CYCLE: LintId = LintId("no-cross-sync-cycle");

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
                             Identity - Identity is the SINK of the sync call graph (everyone may \
                             call Identity synchronously; Identity calls no one synchronously). \
                             React over the bus instead so the sync call graph stays acyclic \
                             (EI-02 §3)."
                        ),
                    });
                }
            }
        }
    }
    out.extend(scan_cross_sync_in_write_path(src));
    out
}

fn scan_cross_sync_in_write_path(src: &str) -> Vec<Violation> {
    const WRITE_MARKER: &str = "@write-path";
    if !src.contains(WRITE_MARKER) {
        return Vec::new();
    }
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
        let site_armed = is_marker_armed(&raw_lines, line, file_armed, WRITE_MARKER);
        if site_armed {
            out.push(Violation {
                lint: NO_CROSS_SYNC_CYCLE,
                line,
                reason: format!(
                    "a synchronous cross-subsystem call `{site}` in a WRITE PATH asks another \
                     subsystem for its state (the \"is it green?\" sync call) - the bus is \
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

pub fn no_cross_sync_cycle() -> Lint {
    Lint {
        id: NO_CROSS_SYNC_CYCLE,
        rule: "the sync call graph is acyclic; identity is a sink",
        scan: scan_no_cross_sync_cycle,
    }
}

pub const RESIDENCY_PIN: LintId = LintId("residency-pin");

fn scan_residency_pin(src: &str) -> Vec<Violation> {
    const STORE_SITES: &[&str] = &[
        "PgPool::connect(",
        "PgPoolOptions::",
        "OltpPool::open(",
        "ColocatedOltp::open(",
        "BlobStore::open(",
        "IndexBackend::open(",
        "CacheClient::new(",
        "StreamStore::open(",
    ];
    const REGION_BINDERS: &[&str] = &[
        "Region",
        "region:",
        ".region(",
        ".pinned_to(",
        "ResidencyTag",
        "residency",
    ];
    const WAIVER_MARKER: &str = "@residency-cell-pinned";
    const WAIVER_MARKER_FILE: &str = "@residency-cell-pinned:file";
    if src.contains(WAIVER_MARKER_FILE) {
        return Vec::new();
    }
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (line, code) in code_statements(src) {
        let is_store = STORE_SITES.iter().any(|s| code.contains(s));
        if !is_store {
            continue;
        }
        let is_pinned = REGION_BINDERS.iter().any(|b| code.contains(b));
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
            if !t.is_empty() && !t.starts_with("//") {
                break;
            }
        }
        let waived = here || above;
        if !is_pinned && !waived {
            out.push(Violation {
                lint: RESIDENCY_PIN,
                line,
                reason: "a store/stream/index/cache is constructed WITHOUT a pinned region - \
                         every store must declare its `Region` (no global pool); a region-less \
                         pool lets data leave its residency boundary (ADR-11/residency-pin)."
                    .into(),
            });
        }
    }
    out.extend(scan_residency_write_boundary(src));
    out
}

fn scan_residency_write_boundary(src: &str) -> Vec<Violation> {
    const WRITE_MARKER: &str = "@residency-write";
    if !src.contains(WRITE_MARKER) {
        return Vec::new();
    }
    let file_armed = src.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("//!") && t.contains(WRITE_MARKER)
    });
    const REQUEST_SOURCES: &[&str] = &[
        "req.region",
        "request.region",
        "payload.region",
        "input.region",
        "params.region",
        "body.region",
        "msg.region",
    ];
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
        let writes_region = code.contains("region:")
            || code.contains("region =")
            || code.contains(".region(")
            || code.contains("set_region(")
            || code.contains("row.region");
        if !writes_region {
            continue;
        }
        let pinned_to_cell = CELL_REGION_SOURCES.iter().any(|c| code.contains(c));
        let idx = line.saturating_sub(1);
        let site_armed =
            file_armed || raw_lines.get(idx).is_some_and(|l| l.contains(WRITE_MARKER)) || {
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
                         asserting it against the harness-threaded CELL region - every write must \
                         pin `row.region == cell.region` with the cell's region injected by the \
                         harness (never a request field), or a forged request lands a row in the \
                         wrong region (the cross-region-write bug class; CP-D3 lint leg, \
                         residency-pin layer 3 - refined-arch tenancy §4.3/§5.3)."
                    .into(),
            });
        }
    }
    out
}

pub fn residency_pin() -> Lint {
    Lint {
        id: RESIDENCY_PIN,
        rule: "every store/stream/index/cache declares a region; no global pool",
        scan: scan_residency_pin,
    }
}

pub const CONTROL_PLANE_PII_FREE: LintId = LintId("control-plane-pii-free");

fn scan_control_plane_pii_free(src: &str) -> Vec<Violation> {
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
    let mut prev_was_personal_data = false;
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
                if prev_was_personal_data {
                    out.push(Violation {
                        lint: CONTROL_PLANE_PII_FREE,
                        line: *line,
                        reason: format!(
                            "control-plane struct carries `is_personal=true` field `{field_name}` \
                             (tagged `#[personal_data(...)]`, the generated data-map) - no \
                             control-plane registry column may be classified `is_personal=true` \
                             (refined-arch tenancy §4.3). The control plane carries OPAQUE IDS ONLY \
                             (TenantId/Region/slug/hash); PII is born inside the cell, never here \
                             (ADR-11.4/OQ-I). Tagging does NOT make PII admissible on the control \
                             plane - it makes the leak machine-detectable."
                        ),
                    });
                } else if PII_FIELDS.contains(&field_name) {
                    out.push(Violation {
                        lint: CONTROL_PLANE_PII_FREE,
                        line: *line,
                        reason: format!(
                            "control-plane struct carries PII field `{field_name}` - the control \
                             plane (routing, cross-cell pointers, placement, directory) must carry \
                             OPAQUE IDS ONLY (TenantId/Region/slug/hash), never a name/email/body. \
                             PII is born inside the cell, never in the control plane (ADR-11/OQ-I)."
                        ),
                    });
                }
            }
        }
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

pub fn control_plane_pii_free() -> Lint {
    Lint {
        id: CONTROL_PLANE_PII_FREE,
        rule: "the control plane carries opaque ids only - never a name/email/body",
        scan: scan_control_plane_pii_free,
    }
}

pub const SEARCH_REQUIRES_ACL_FILTER: LintId = LintId("search-requires-acl-filter");

fn scan_search_requires_acl_filter(src: &str) -> Vec<Violation> {
    const SEARCH_SITES: &[&str] = &[
        ".search(",
        ".query_index(",
        "IndexBackend::search",
        ".list_objects_scored(",
        "SearchQuery::execute",
        ".rank(",
    ];
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
                         `Filter` before scoring - search must PRE-filter on permission, never \
                         post-filter; scoring before filtering leaks the existence and rank of \
                         forbidden documents (ADR-03/OQ-E)."
                    .into(),
            });
        }
    }
    out
}

pub fn search_requires_acl_filter() -> Lint {
    Lint {
        id: SEARCH_REQUIRES_ACL_FILTER,
        rule: "every search/list query conjoins the list_objects Filter before scoring",
        scan: scan_search_requires_acl_filter,
    }
}

pub const NO_LLM_IN_PLATFORM: LintId = LintId("no-llm-in-platform");

fn scan_no_llm_in_platform(src: &str) -> Vec<Violation> {
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
                        "LLM SDK / prompt / model-name fingerprint `{site}` in platform code - no \
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

pub fn no_llm_in_platform() -> Lint {
    Lint {
        id: NO_LLM_IN_PLATFORM,
        rule: "no LLM SDK / prompt / model name in platform code; runtime behind AgentRuntime",
        scan: scan_no_llm_in_platform,
    }
}

pub const FLOW_DETERMINISM: LintId = LintId("flow-determinism");

fn scan_flow_determinism(src: &str) -> Vec<Violation> {
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
                         deterministic `WfCtx` surface - a workflow must read time/rand/IO only \
                         through `ctx.now()`/`ctx.rand()`/`ctx.activity(..)` so replay is \
                         deterministic; a raw clock/rng read makes replay diverge (index 9.2/OQ-F)."
                    ),
                });
            }
        }
    }
    out
}

pub fn flow_determinism() -> Lint {
    Lint {
        id: FLOW_DETERMINISM,
        rule: "a myelin-flow workflow body uses only the deterministic WfCtx surface",
        scan: scan_flow_determinism,
    }
}

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

pub fn all_twelve() -> Vec<Lint> {
    let mut v = load_bearing_four();
    v.extend(remaining_eight());
    v
}

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

pub fn load_bearing_four() -> Vec<Lint> {
    vec![
        tenant_predicate(),
        no_raw_publish(),
        no_host_exec(),
        no_untagged_personal_data(),
    ]
}

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

        let red_other_local = r#"
            let tenant_sql = "SELECT id FROM ci_job WHERE tenant_id=$1";
            let query = "SELECT id FROM ci_job";
            let rows = sqlx::query(&query).fetch_all(&pool);
        "#;
        assert!(
            !tenant_predicate().run(red_other_local).is_empty(),
            "only the SQL the query actually executes may admit it"
        );

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
    fn tenant_predicate_admits_query_inside_with_tenant_tx_closure() {
        let green = r#"
            bridge(runtime, async move {
                with_tenant_tx(&pool, &tenant_id, &region, |connection| {
                    Box::pin(async move {
                        sqlx::query(CREATE_MANAGED_SECRET)
                            .bind(&row_tenant)
                            .bind(&row_region)
                            .fetch_optional(&mut *connection)
                            .await
                    })
                })
                .await
            })
        "#;
        assert!(
            tenant_predicate().run(green).is_empty(),
            "a query inside a with_tenant_tx RLS closure must be admitted"
        );

        let red_bare = r#"
            let rows = sqlx::query("SELECT * FROM secret").fetch_all(&pool);
        "#;
        assert!(
            !tenant_predicate().run(red_bare).is_empty(),
            "a bare tenant-less query must still be rejected"
        );

        let red_outside = r#"
            with_tenant_tx(&pool, &tenant_id, &region, |connection| {
                Box::pin(async move {
                    sqlx::query(SCOPED_READ).fetch_all(&mut *connection).await
                })
            })
            .await;
            let leaked = sqlx::query("SELECT * FROM secret").fetch_all(&pool);
        "#;
        assert!(
            !tenant_predicate().run(red_outside).is_empty(),
            "a query outside the with_tenant_tx closure must still be rejected"
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
        let green = "reporter.report_done(&run, idem_token, &report)?;";
        assert!(
            no_raw_ci_verdict().run(green).is_empty(),
            "the reporter path must be admitted"
        );
    }

    #[test]
    fn no_raw_ci_verdict_catches_alias_ufcs_and_line_split_dodges() {
        let alias = "use myelin_flow::SignalPayload::CiJobDone as Done;";
        let ufcs = "DurableExecutor::signal_typed(&executor, spec)?;";
        let line_split = "    executor\n        .signal_typed";
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
        let def = "fn signal_typed(&self, spec: TypedSignalSpec) -> Result<SignalOutcome, E> {";
        assert!(
            !no_raw_ci_verdict().run(def).is_empty(),
            "the definition is flagged by the scan; only the per-lint exclusion admits the seam file"
        );
    }

    #[test]
    fn no_raw_publish_admits_unrelated_put_calls() {
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

    #[test]
    fn no_cross_db_rejects_sibling_storage_use_admits_contract_use() {
        let red = "use myelin_identity::store::PrincipalStore;";
        let green = "use myelin_identity::PrincipalId;";
        assert!(!no_cross_db().run(red).is_empty());
        assert!(no_cross_db().run(green).is_empty());
    }

    #[test]
    fn forward_only_migration_rejects_down_and_blocking_alter_admits_expand() {
        let red_down = "fn down() { /* rollback */ }";
        let red_alter = "ALTER TABLE principals ADD COLUMN email TEXT NOT NULL;";
        let green = "ALTER TABLE principals ADD COLUMN email TEXT;";
        assert!(!forward_only_migration().run(red_down).is_empty());
        assert!(!forward_only_migration().run(red_alter).is_empty());
        assert!(forward_only_migration().run(green).is_empty());
    }

    #[test]
    fn forward_only_migration_does_not_reinterpret_embedded_sql_as_rust_code() {
        let registry = r##"pub const HISTORICAL_DDL: &str = r#"
ALTER TABLE archived_record ALTER COLUMN source_id SET NOT NULL;
"#;"##;
        assert!(forward_only_migration().run(registry).is_empty());
    }

    #[test]
    fn forward_only_migration_tightens_to_per_hot_table_when_declared() {
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

        let hot_index_concurrent =
            "-- @hot-table issue\nCREATE INDEX CONCURRENTLY idx_issue_status ON issue (status);";
        assert!(forward_only_migration()
            .run(hot_index_concurrent)
            .is_empty());

        let hot_expand = "-- @hot-table issue\nALTER TABLE issue ADD COLUMN priority INT;";
        assert!(
            forward_only_migration().run(hot_expand).is_empty(),
            "the expand step (nullable add) is admitted on a hot table"
        );

        let hot_blocking_alter =
            "-- @hot-table issue\nALTER TABLE issue ADD COLUMN body TEXT NOT NULL;";
        assert!(!forward_only_migration().run(hot_blocking_alter).is_empty());
    }

    #[test]
    fn no_cross_sync_cycle_rejects_identity_sync_call_admits_bus_reaction() {
        let red = "// @identity-sink\nlet r = client.call_sync(req);";
        let green = "// @identity-sink\nctx.emit(draft, cause)?;";
        let elsewhere = "let r = client.call_sync(req);";
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
        let red = "// @residency-write\nlet row = Row { region: req.region, tenant_id };";
        let green =
            "// @residency-write\nlet row = Row { region: cell.region, tenant_id }; // pinned to cell";
        let unmarked = "let r = Row { region: req.region, tenant_id };";
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
            "an UNMARKED region-from-request statement is not a write boundary - must not fire"
        );
    }

    #[test]
    fn residency_pin_write_boundary_reads_cell_region_from_harness_not_request_field() {
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
        let green = "let out = agent_runtime.run(plan).await?;";
        assert!(!no_llm_in_platform().run(red).is_empty());
        assert!(no_llm_in_platform().run(green).is_empty());
    }

    #[test]
    fn flow_determinism_rejects_raw_clock_in_workflow_admits_wfctx() {
        let red = "// @workflow-body\nlet t = SystemTime::now();";
        let green = "// @workflow-body\nlet t = ctx.now();";
        let elsewhere = "let t = SystemTime::now();";
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
        let mut sorted: Vec<&str> = ids.iter().map(|i| i.0).collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 12, "all twelve lint ids must be distinct");
    }
}
