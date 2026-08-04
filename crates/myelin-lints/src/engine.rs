use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LintId(pub &'static str);

impl fmt::Display for LintId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub lint: LintId,
    pub line: usize,
    pub reason: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] line {}: {}", self.lint, self.line, self.reason)
    }
}

pub struct Lint {
    pub id: LintId,
    pub rule: &'static str,
    pub scan: fn(src: &str) -> Vec<Violation>,
}

impl Lint {
    pub fn run(&self, src: &str) -> Vec<Violation> {
        (self.scan)(src)
    }
}

pub fn run(lints: &[Lint], src: &str) -> Result<(), Vec<Violation>> {
    let mut found = Vec::new();
    for lint in lints {
        found.extend(lint.run(src));
    }
    if found.is_empty() {
        Ok(())
    } else {
        Err(found)
    }
}

pub fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut index = 0usize;
    let mut in_standard = false;
    let mut escaped = false;
    let mut raw_hashes = None;
    while index < bytes.len() {
        if let Some(hashes) = raw_hashes {
            if bytes[index] == b'"'
                && bytes
                    .get(index + 1..index + 1 + hashes)
                    .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
            {
                raw_hashes = None;
                index += hashes + 1;
            } else {
                index += 1;
            }
            continue;
        }
        if in_standard {
            if escaped {
                escaped = false;
            } else if bytes[index] == b'\\' {
                escaped = true;
            } else if bytes[index] == b'"' {
                in_standard = false;
            }
            index += 1;
            continue;
        }
        if bytes[index] == b'r' {
            let mut cursor = index + 1;
            while bytes.get(cursor) == Some(&b'#') {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'"') {
                raw_hashes = Some(cursor - index - 1);
                index = cursor + 1;
                continue;
            }
        }
        if bytes[index] == b'"' {
            in_standard = true;
            index += 1;
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'/') {
            return &line[..index];
        }
        index += 1;
    }
    line
}

pub fn strip_block_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    let mut in_block = false;
    while i < bytes.len() {
        if !in_block && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            in_block = true;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if in_block && i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
            in_block = false;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        let c = bytes[i] as char;
        if in_block && c != '\n' {
            out.push(' ');
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

pub fn blank_string_literals(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_str = false;
    for c in line.chars() {
        if c == '"' {
            in_str = !in_str;
            out.push('"');
        } else if in_str {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

pub fn code_lines(src: &str) -> Vec<(usize, String)> {
    let no_block = strip_block_comments(src);
    no_block
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, strip_line_comment(line).to_string()))
        .collect()
}

pub fn code_statements(src: &str) -> Vec<(usize, String)> {
    #[derive(Default)]
    struct StringState {
        standard: bool,
        escaped: bool,
        raw_hashes: Option<usize>,
    }

    fn has_boundary_outside_string(code: &str, state: &mut StringState) -> bool {
        let bytes = code.as_bytes();
        let mut index = 0usize;
        let mut boundary = false;
        while index < bytes.len() {
            if let Some(hashes) = state.raw_hashes {
                if bytes[index] == b'"'
                    && bytes
                        .get(index + 1..index + 1 + hashes)
                        .is_some_and(|suffix| suffix.iter().all(|byte| *byte == b'#'))
                {
                    state.raw_hashes = None;
                    index += hashes + 1;
                } else {
                    index += 1;
                }
                continue;
            }
            if state.standard {
                if state.escaped {
                    state.escaped = false;
                } else if bytes[index] == b'\\' {
                    state.escaped = true;
                } else if bytes[index] == b'"' {
                    state.standard = false;
                }
                index += 1;
                continue;
            }

            if bytes[index] == b'r' {
                let mut cursor = index + 1;
                while bytes.get(cursor) == Some(&b'#') {
                    cursor += 1;
                }
                if bytes.get(cursor) == Some(&b'"') {
                    state.raw_hashes = Some(cursor - index - 1);
                    index = cursor + 1;
                    continue;
                }
            }
            match bytes[index] {
                b'"' => state.standard = true,
                b';' | b'{' | b'}' => boundary = true,
                _ => {}
            }
            index += 1;
        }
        boundary
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let mut start_line = 0usize;
    let mut string_state = StringState::default();
    for (line_no, code) in code_lines(src) {
        let trimmed = code.trim();
        if trimmed.is_empty() {
            continue;
        }
        if current.is_empty() {
            start_line = line_no;
        } else {
            current.push(' ');
        }
        current.push_str(trimmed);
        if has_boundary_outside_string(&code, &mut string_state) {
            out.push((start_line, std::mem::take(&mut current)));
        }
    }
    if !current.is_empty() {
        out.push((start_line, current));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_lint() -> Lint {
        Lint {
            id: LintId("always-ok"),
            rule: "admits everything",
            scan: |_src| Vec::new(),
        }
    }

    fn ban_foo() -> Lint {
        Lint {
            id: LintId("no-foo"),
            rule: "the token `foo` is forbidden",
            scan: |src| {
                code_lines(src)
                    .into_iter()
                    .filter(|(_, l)| l.contains("foo"))
                    .map(|(n, _)| Violation {
                        lint: LintId("no-foo"),
                        line: n,
                        reason: "found `foo`".into(),
                    })
                    .collect()
            },
        }
    }

    #[test]
    fn run_is_ok_on_clean_source() {
        assert!(run(&[ok_lint(), ban_foo()], "let bar = 1;").is_ok());
    }

    #[test]
    fn run_is_err_loudly_on_violation() {
        let err = run(&[ban_foo()], "let foo = 1;").unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].lint, LintId("no-foo"));
        assert_eq!(err[0].line, 1);
    }

    #[test]
    fn line_comments_are_stripped_so_prose_is_not_code() {
        let src = "let bar = 1; // this line mentions foo in prose";
        assert!(run(&[ban_foo()], src).is_ok());
    }

    #[test]
    fn line_comment_markers_inside_strings_are_preserved() {
        let artifact = r#"let subject = "myelin://tenant/git/pr/1"; // trailing prose"#;
        assert_eq!(
            strip_line_comment(artifact).trim(),
            r#"let subject = "myelin://tenant/git/pr/1";"#
        );
        let raw = r##"let subject = r#"runtime://worker/session"#; // trailing prose"##;
        assert_eq!(
            strip_line_comment(raw).trim(),
            r##"let subject = r#"runtime://worker/session"#;"##
        );
    }

    #[test]
    fn string_literal_contents_are_blanked_but_quotes_and_length_survive() {
        let line = r#"if upper.contains("DROP COLUMN") { reject(); }"#;
        let blanked = blank_string_literals(line);
        assert!(
            !blanked.contains("DROP COLUMN"),
            "literal contents must be blanked"
        );
        assert!(
            blanked.contains("upper.contains("),
            "code outside the literal survives"
        );
        assert_eq!(
            blanked.len(),
            line.len(),
            "length (and so column offsets) is preserved"
        );
    }

    #[test]
    fn block_comments_are_stripped_and_line_numbers_preserved() {
        let src = "line1\n/* foo\nfoo */\nlet foo = 1;";
        let lines = code_lines(src);
        let viol: Vec<_> = lines.iter().filter(|(_, l)| l.contains("foo")).collect();
        assert_eq!(viol.len(), 1);
        assert_eq!(viol[0].0, 4, "line numbers must survive comment stripping");
    }

    #[test]
    fn statement_boundaries_ignore_braces_and_semicolons_inside_multiline_strings() {
        let src = r####"
            let row = sqlx::query(&format!(
                "SELECT {COLUMNS} FROM principals; WHERE tenant_id=$1"
            ))
            .bind(tenant_id)
            .fetch_one(&pool);
            let raw = sqlx::query(r#"SELECT {payload}; WHERE tenant_id=$1"#)
                .bind(tenant_id)
                .fetch_one(&pool);
        "####;
        let statements = code_statements(src);
        let query_statements: Vec<_> = statements
            .iter()
            .filter(|(_, statement)| statement.contains("sqlx::query"))
            .collect();
        assert_eq!(query_statements.len(), 2);
        assert!(
            query_statements
                .iter()
                .all(|(_, statement)| statement.contains(".bind(tenant_id)")),
            "string punctuation must not split a query from its tenant binder"
        );
    }
}
