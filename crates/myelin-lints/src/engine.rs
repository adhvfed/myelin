//! The lint engine: the typed [`Lint`] / [`Violation`] vocabulary every architecture lint is
//! built from, plus the hermetic line/comment-aware scanning helpers the lints share.
//!
//! **Loud, never swallowed (EI-01 §5).** A lint reports a [`Violation`] as DATA — a typed value
//! carrying the lint id, the 1-based line number, and a human reason. The harness ([`run`])
//! returns `Err(Vec<Violation>)` on any violation; it has no `... || true` / silent-filter path.
//! A test that asserts "this fixture is clean" asserts `run(...).is_ok()`; a test that asserts
//! "this fixture is rejected" asserts `!violations.is_empty()`. The gate cannot pass silently.

use core::fmt;

/// A stable lint identifier (the §2.11 lint name, e.g. `tenant-predicate`). Used in [`Violation`]
/// messages and to assert which lint fired in the fixture matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LintId(pub &'static str);

impl fmt::Display for LintId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// A single lint finding — the typed, LOUD unit a lint emits (never a swallowed boolean).
/// Carries enough to fail the build with a precise, actionable message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Which lint fired.
    pub lint: LintId,
    /// The 1-based line number in the scanned source unit.
    pub line: usize,
    /// A human-readable reason (what the rule forbids + how to fix it).
    pub reason: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] line {}: {}", self.lint, self.line, self.reason)
    }
}

/// An architecture lint: a stable id + a scanner over a unit of source text.
///
/// The scanner takes the full source of one unit (a file, or a fixture string) and returns
/// every [`Violation`] it finds. A lint that finds nothing returns an empty `Vec` (the unit is
/// admitted). The scanner is a pure function of the source text — hermetic and deterministic
/// (no toolchain, no DB, no network), so the gate is reproducible byte-for-byte.
pub struct Lint {
    /// The §2.11 lint name (the stable id).
    pub id: LintId,
    /// One-line rule description (mirrors the §2.11 table cell).
    pub rule: &'static str,
    /// The scanner. Returns one [`Violation`] per offending site.
    pub scan: fn(src: &str) -> Vec<Violation>,
}

impl Lint {
    /// Scan one source unit. The thin typed wrapper over [`Lint::scan`].
    pub fn run(&self, src: &str) -> Vec<Violation> {
        (self.scan)(src)
    }
}

/// Run a set of lints over one source unit and return `Err` LOUDLY on ANY violation (EI-01 §5).
/// `Ok(())` iff every lint admits the unit. There is no swallow path.
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

/// Strip a Rust line-comment (`// ...`) from a line so the lints scan CODE, not prose that
/// happens to mention a forbidden token (e.g. a doc-comment saying "no `publish_now`"). This is
/// a deliberately small, conservative tokeniser: it does not handle `//` inside a string
/// literal, which is acceptable for an architecture lint over our own conventionally-formatted
/// source (the workspace scan proves it on real code; a false-positive would fail loudly and be
/// fixed, never silently). Block comments (`/* */`) are handled by [`strip_block_comments`].
pub fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

/// Remove `/* ... */` block comments (including doc block comments and multi-line spans) from a
/// source unit, replacing each with whitespace of the same line-structure so line numbers are
/// preserved. Conservative (does not track string literals); see [`strip_line_comment`].
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

/// Replace the CONTENTS of every double-quoted string literal on a line with spaces (keeping the
/// surrounding quotes + the line length), so a forbidden token that appears as string DATA — e.g.
/// the migration runner's OWN guard `upper.contains("DROP COLUMN")`, or a SQL-DDL fragment a
/// scanner-checker holds as a literal — is not mistaken for the real construct. This is the
/// string-literal analogue of [`strip_line_comment`]: a lint that targets DDL/code (not the
/// string data that mentions it) scans the blanked form. Conservative: it does not track escaped
/// quotes inside a literal (acceptable for our conventionally-formatted source; a false result
/// fails loudly and is fixed, never silently swallowed — EI-01 §5).
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

/// Iterate the CODE lines of a source unit (block comments stripped, then line comments
/// stripped), yielding `(1-based line number, code-only text)`. The shared front-end every lint
/// scanner uses so all four agree on "what is code".
pub fn code_lines(src: &str) -> Vec<(usize, String)> {
    let no_block = strip_block_comments(src);
    no_block
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, strip_line_comment(line).to_string()))
        .collect()
}

/// Group the code lines into STATEMENTS, yielding `(1-based start line, joined statement text)`.
/// A statement is a run of code text terminated by `;`, `{`, or `}` (the structural boundaries).
/// This lets a lint reason over a fluent builder chain that spans several lines
/// (`sqlx::query(..)\n  .with_tenant(t)\n  .fetch_all(p);`) as ONE unit, so a tenant binder on a
/// later line of the same statement is seen. Comments are already stripped by [`code_lines`].
pub fn code_statements(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start_line = 0usize;
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
        // A statement boundary: the line ends a statement/block. We split greedily on the
        // presence of a terminator anywhere on the line — conservative but sufficient for our
        // conventionally-formatted source (one statement per logical chain).
        if code.contains(';') || code.contains('{') || code.contains('}') {
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
        // a comment that NAMES the forbidden token must NOT trip the lint.
        let src = "let bar = 1; // this line mentions foo in prose";
        assert!(run(&[ban_foo()], src).is_ok());
    }

    #[test]
    fn string_literal_contents_are_blanked_but_quotes_and_length_survive() {
        // a forbidden token held as DATA (a guard checking for the pattern) is blanked out, so a
        // lint targeting the real construct does not trip on the check that forbids it.
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
        // the `foo`s on lines 2 and 3 are inside a block comment → gone; line 4 is real code.
        let viol: Vec<_> = lines.iter().filter(|(_, l)| l.contains("foo")).collect();
        assert_eq!(viol.len(), 1);
        assert_eq!(viol[0].0, 4, "line numbers must survive comment stripping");
    }
}
