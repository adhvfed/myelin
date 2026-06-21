//! The textual **`QueryAst` grammar parser** — compiles a predicate string
//! (`"status == 'open' AND severity >= 3"`) into the ONE frozen [`Predicate`](crate::Predicate)
//! tree (contract 13.3 / 3.4, X-3/OQ-C).
//!
//! ## What this is (the P-235 named floor, now filled)
//! `lib.rs` named the **textual grammar parser** as the Issues/Knowledge co-owned deliverable
//! landing in **P-235 (KN-P02)**: a string surface that compiles into the existing frozen
//! [`Predicate`] tree + the one bounded interpreter. This module is that parser. It does **NOT**
//! define a second predicate engine — it is a *front-end* over the one engine: it produces a
//! [`QueryAst`](crate::QueryAst) wrapping a validated [`Predicate`], and every consumer
//! (saved-view filters, the Bus `EventMatcher`, Notif prefs) evaluates the SAME tree through the
//! SAME [`QueryAst::eval`](crate::QueryAst::eval). The parser validates its own output against the
//! one static cost bound ([`QueryAst::validate`](crate::QueryAst::validate)) so a crafted string
//! can never present an over-budget tree to the interpreter.
//!
//! ## The grammar (bounded, declarative — no UDFs, no loops, no recursion-to-unbounded-depth)
//! ```text
//! query      := or_expr
//! or_expr    := and_expr ( ("OR"  | "or" ) and_expr )*
//! and_expr   := not_expr ( ("AND" | "and") not_expr )*
//! not_expr   := ("NOT" | "not" | "!") not_expr | primary
//! primary    := "(" or_expr ")" | "true" | "false" | comparison
//! comparison := field op value
//! field      := IDENT ( "." IDENT )*            // a dotted field path, e.g. `payload.status`
//! op         := "==" | "!=" | "<" | "<=" | ">" | ">="
//! value      := STRING | INT | "true" | "false" | field   // a field-vs-field or field-vs-literal cmp
//! STRING     := "'" ... "'" | "\"" ... "\""     // single- or double-quoted, with \\ and \' escapes
//! INT        := -?[0-9]+
//! ```
//! A dotted **field path** becomes an [`Expr::Var`](crate::Expr::Var) with the dotted name (the
//! same variable namespace `project_envelope` binds for the matcher: `event.type`,
//! `payload.status`, …). A bare-literal value becomes an [`Expr::Lit`](crate::Expr::Lit).
//!
//! ## Boundedness is structural AND parse-time
//! The recursive-descent parser is itself depth-bounded ([`MAX_PARSE_DEPTH`]) so a pathological
//! deeply-parenthesised string is rejected at parse time (it never blows the parser stack), and the
//! produced tree is re-validated against [`crate::MAX_PREDICATE_NODES`] /
//! [`crate::MAX_PREDICATE_DEPTH`] before it is handed to a consumer — belt and braces.

use crate::{CmpOp, Expr, Predicate, PredicateError, QueryAst};
use myelin_identity::Literal;

/// The maximum recursion depth the recursive-descent parser will descend (each `(`/`NOT` is one
/// level). A string nesting deeper than this is rejected with [`ParseError::TooDeep`] **at parse
/// time** — the parser stack is statically bounded, so a crafted string cannot DoS the compiler
/// itself (defence in depth, distinct from the produced tree's [`crate::MAX_PREDICATE_DEPTH`]).
pub const MAX_PARSE_DEPTH: usize = 64;

/// A query-string parse failure. Every variant is a *precise, located* rejection — the parser
/// fails closed (it never produces a partial/ambiguous tree), so a malformed filter surfaces a
/// loud error rather than a silently-wrong predicate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// An unexpected character at the given byte offset (e.g. a stray symbol).
    UnexpectedChar { ch: char, at: usize },
    /// A token was expected (named) but a different one (or end-of-input) was found.
    Expected { what: &'static str, found: String },
    /// Input ended before the expression was complete.
    UnexpectedEof,
    /// Trailing tokens after a complete expression (the whole string must parse).
    TrailingInput { rest: String },
    /// An unterminated string literal (no closing quote).
    UnterminatedString,
    /// An integer literal that does not fit `i64`.
    BadInt { text: String },
    /// The parser recursion exceeded [`MAX_PARSE_DEPTH`] (a pathological nesting depth).
    TooDeep,
    /// The produced predicate tree exceeded the one static cost bound (re-validation after parse).
    Oversized(PredicateError),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnexpectedChar { ch, at } => {
                write!(f, "unexpected character {ch:?} at offset {at}")
            }
            ParseError::Expected { what, found } => {
                write!(f, "expected {what}, found {found:?}")
            }
            ParseError::UnexpectedEof => write!(f, "unexpected end of input"),
            ParseError::TrailingInput { rest } => write!(f, "trailing input after expression: {rest:?}"),
            ParseError::UnterminatedString => write!(f, "unterminated string literal"),
            ParseError::BadInt { text } => write!(f, "integer literal out of range: {text:?}"),
            ParseError::TooDeep => {
                write!(f, "expression nesting exceeds the parse-depth ceiling ({MAX_PARSE_DEPTH})")
            }
            ParseError::Oversized(e) => write!(f, "parsed predicate is over budget: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Compile a query string into the frozen [`QueryAst`] (a validated [`Predicate`] tree + the
/// retained textual source). This is the P-235 grammar front-end over the one bounded engine — the
/// produced AST evaluates through [`QueryAst::eval`](crate::QueryAst::eval) exactly like a
/// directly-built tree. The whole string must parse (trailing tokens are an error), the parse depth
/// is bounded, and the tree is re-validated against the one static cost bound.
pub fn parse_query(src: &str) -> Result<QueryAst, ParseError> {
    let predicate = parse_predicate(src)?;
    // Re-validate against the ONE static cost bound (belt and braces over the parse-depth guard).
    QueryAst::validate(&predicate).map_err(ParseError::Oversized)?;
    // Preserve the textual source on the AST (observability + the placeholder-surface handle).
    Ok(QueryAst::compiled_with_source(predicate, src))
}

/// Parse a query string into the bare [`Predicate`] tree (without wrapping it in a [`QueryAst`] /
/// retaining the source). Used by [`parse_query`] and exposed for callers that want the tree.
pub fn parse_predicate(src: &str) -> Result<Predicate, ParseError> {
    let tokens = lex(src)?;
    let mut p = Parser { tokens: &tokens, pos: 0, depth: 0 };
    let pred = p.parse_or()?;
    if p.pos != p.tokens.len() {
        return Err(ParseError::TrailingInput {
            rest: p.tokens[p.pos..].iter().map(|t| t.text()).collect::<Vec<_>>().join(" "),
        });
    }
    Ok(pred)
}

// ───────────────────────────────────── Lexer ─────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// An identifier piece (a field-path segment or a keyword like AND/OR/NOT/true/false).
    Ident(String),
    /// A quoted string literal (the unescaped content).
    Str(String),
    /// An integer literal.
    Int(i64),
    Dot,
    LParen,
    RParen,
    Bang,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Ident(s) => s.clone(),
            Tok::Str(s) => format!("{s:?}"),
            Tok::Int(n) => n.to_string(),
            Tok::Dot => ".".into(),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::Bang => "!".into(),
            Tok::Eq => "==".into(),
            Tok::Ne => "!=".into(),
            Tok::Lt => "<".into(),
            Tok::Le => "<=".into(),
            Tok::Gt => ">".into(),
            Tok::Ge => ">=".into(),
        }
    }
}

fn lex(src: &str) -> Result<Vec<Tok>, ParseError> {
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b'.' => {
                out.push(Tok::Dot);
                i += 1;
            }
            b'=' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Eq);
                    i += 2;
                } else {
                    return Err(ParseError::Expected { what: "`==`", found: "=".into() });
                }
            }
            b'!' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Ne);
                    i += 2;
                } else {
                    out.push(Tok::Bang);
                    i += 1;
                }
            }
            b'<' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Le);
                    i += 2;
                } else {
                    out.push(Tok::Lt);
                    i += 1;
                }
            }
            b'>' => {
                if bytes.get(i + 1) == Some(&b'=') {
                    out.push(Tok::Ge);
                    i += 2;
                } else {
                    out.push(Tok::Gt);
                    i += 1;
                }
            }
            b'\'' | b'"' => {
                let quote = c;
                let (s, next) = lex_string(bytes, i + 1, quote)?;
                out.push(Tok::Str(s));
                i = next;
            }
            b'-' | b'0'..=b'9' => {
                let (n, next) = lex_int(src, bytes, i)?;
                out.push(Tok::Int(n));
                i = next;
            }
            _ if is_ident_start(c) => {
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                out.push(Tok::Ident(src[start..i].to_string()));
            }
            _ => {
                // Decode the offending char for a precise error (handles multi-byte UTF-8).
                let ch = src[i..].chars().next().unwrap_or('\u{FFFD}');
                return Err(ParseError::UnexpectedChar { ch, at: i });
            }
        }
    }
    Ok(out)
}

/// Lex a quoted string body (after the opening quote at `start`), honouring `\\` and `\<quote>`
/// escapes. Returns the unescaped content + the index just past the closing quote.
fn lex_string(bytes: &[u8], start: usize, quote: u8) -> Result<(String, usize), ParseError> {
    let mut s = String::new();
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' {
            match bytes.get(i + 1) {
                Some(&b'\\') => {
                    s.push('\\');
                    i += 2;
                }
                Some(&q) if q == quote => {
                    s.push(quote as char);
                    i += 2;
                }
                // An unknown escape is preserved verbatim (backslash + char) — no surprising drops.
                Some(&other) => {
                    s.push('\\');
                    s.push(other as char);
                    i += 2;
                }
                None => return Err(ParseError::UnterminatedString),
            }
        } else if c == quote {
            return Ok((s, i + 1));
        } else {
            // Copy the (possibly multi-byte) UTF-8 char through. We only ever index ASCII control
            // bytes; non-ASCII bytes are part of a UTF-8 sequence we copy whole.
            let ch_len = utf8_len(c);
            // SAFETY of slicing: `bytes` came from a `&str`, so a multi-byte sequence is well-formed.
            let chunk = std::str::from_utf8(&bytes[i..(i + ch_len).min(bytes.len())])
                .map_err(|_| ParseError::UnterminatedString)?;
            s.push_str(chunk);
            i += ch_len;
        }
    }
    Err(ParseError::UnterminatedString)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn lex_int(src: &str, bytes: &[u8], start: usize) -> Result<(i64, usize), ParseError> {
    let mut i = start;
    if bytes[i] == b'-' {
        i += 1;
    }
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        // A lone `-` with no digits.
        return Err(ParseError::Expected { what: "an integer", found: "-".into() });
    }
    let text = &src[start..i];
    let n = text.parse::<i64>().map_err(|_| ParseError::BadInt { text: text.into() })?;
    Ok((n, i))
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

// ───────────────────────────────────── Parser ─────────────────────────────────────

struct Parser<'a> {
    tokens: &'a [Tok],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<&Tok> {
        let t = self.tokens.get(self.pos);
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// `or_expr := and_expr ( OR and_expr )*`.
    fn parse_or(&mut self) -> Result<Predicate, ParseError> {
        let mut terms = vec![self.parse_and()?];
        while self.match_keyword_or() {
            terms.push(self.parse_and()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Predicate::Or(terms)
        })
    }

    /// `and_expr := not_expr ( AND not_expr )*`.
    fn parse_and(&mut self) -> Result<Predicate, ParseError> {
        let mut terms = vec![self.parse_not()?];
        while self.match_keyword_and() {
            terms.push(self.parse_not()?);
        }
        Ok(if terms.len() == 1 {
            terms.pop().unwrap()
        } else {
            Predicate::And(terms)
        })
    }

    /// `not_expr := (NOT | !) not_expr | primary`.
    fn parse_not(&mut self) -> Result<Predicate, ParseError> {
        if self.match_keyword_not() || matches!(self.peek(), Some(Tok::Bang)) {
            if matches!(self.peek(), Some(Tok::Bang)) {
                self.bump();
            }
            self.depth += 1;
            if self.depth > MAX_PARSE_DEPTH {
                return Err(ParseError::TooDeep);
            }
            let inner = self.parse_not()?;
            self.depth -= 1;
            return Ok(Predicate::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    /// `primary := "(" or_expr ")" | true | false | comparison`.
    fn parse_primary(&mut self) -> Result<Predicate, ParseError> {
        match self.peek() {
            Some(Tok::LParen) => {
                self.bump();
                self.depth += 1;
                if self.depth > MAX_PARSE_DEPTH {
                    return Err(ParseError::TooDeep);
                }
                let inner = self.parse_or()?;
                self.depth -= 1;
                match self.bump() {
                    Some(Tok::RParen) => Ok(inner),
                    other => Err(ParseError::Expected {
                        what: "`)`",
                        found: other.map(Tok::text).unwrap_or_else(|| "<eof>".into()),
                    }),
                }
            }
            Some(Tok::Ident(id)) if id.eq_ignore_ascii_case("true") => {
                self.bump();
                Ok(Predicate::True)
            }
            Some(Tok::Ident(id)) if id.eq_ignore_ascii_case("false") => {
                self.bump();
                Ok(Predicate::False)
            }
            Some(Tok::Ident(_)) => self.parse_comparison(),
            Some(other) => Err(ParseError::Expected {
                what: "a field, `(`, or a boolean constant",
                found: other.text(),
            }),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    /// `comparison := field op value`.
    fn parse_comparison(&mut self) -> Result<Predicate, ParseError> {
        let lhs = self.parse_field()?;
        let op = self.parse_op()?;
        let rhs = self.parse_value()?;
        Ok(Predicate::Cmp { op, lhs, rhs })
    }

    /// `field := IDENT ( "." IDENT )*` → an [`Expr::Var`] with the dotted name.
    fn parse_field(&mut self) -> Result<Expr, ParseError> {
        let mut name = match self.bump() {
            Some(Tok::Ident(s)) => s.clone(),
            other => {
                return Err(ParseError::Expected {
                    what: "a field name",
                    found: other.map(Tok::text).unwrap_or_else(|| "<eof>".into()),
                })
            }
        };
        while matches!(self.peek(), Some(Tok::Dot)) {
            self.bump();
            match self.bump() {
                Some(Tok::Ident(s)) => {
                    name.push('.');
                    name.push_str(s);
                }
                other => {
                    return Err(ParseError::Expected {
                        what: "a field-path segment after `.`",
                        found: other.map(Tok::text).unwrap_or_else(|| "<eof>".into()),
                    })
                }
            }
        }
        Ok(Expr::Var(name))
    }

    fn parse_op(&mut self) -> Result<CmpOp, ParseError> {
        let op = match self.peek() {
            Some(Tok::Eq) => CmpOp::Eq,
            Some(Tok::Ne) => CmpOp::Ne,
            Some(Tok::Lt) => CmpOp::Lt,
            Some(Tok::Le) => CmpOp::Le,
            Some(Tok::Gt) => CmpOp::Gt,
            Some(Tok::Ge) => CmpOp::Ge,
            other => {
                return Err(ParseError::Expected {
                    what: "a comparison operator (== != < <= > >=)",
                    found: other.map(Tok::text).unwrap_or_else(|| "<eof>".into()),
                })
            }
        };
        self.bump();
        Ok(op)
    }

    /// `value := STRING | INT | true | false | field`.
    fn parse_value(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Some(Tok::Str(s)) => {
                let lit = Expr::Lit(Literal::Str(s.clone()));
                self.bump();
                Ok(lit)
            }
            Some(Tok::Int(n)) => {
                let lit = Expr::Lit(Literal::Int(*n));
                self.bump();
                Ok(lit)
            }
            Some(Tok::Ident(id)) if id.eq_ignore_ascii_case("true") => {
                self.bump();
                Ok(Expr::Lit(Literal::Bool(true)))
            }
            Some(Tok::Ident(id)) if id.eq_ignore_ascii_case("false") => {
                self.bump();
                Ok(Expr::Lit(Literal::Bool(false)))
            }
            // A bare identifier on the RHS is a field-vs-field comparison.
            Some(Tok::Ident(_)) => self.parse_field(),
            other => Err(ParseError::Expected {
                what: "a value (string, int, bool, or field)",
                found: other.map(Tok::text).unwrap_or_else(|| "<eof>".into()),
            }),
        }
    }

    fn match_keyword_and(&mut self) -> bool {
        self.match_keyword("and")
    }
    fn match_keyword_or(&mut self) -> bool {
        self.match_keyword("or")
    }
    fn match_keyword_not(&mut self) -> bool {
        self.match_keyword("not")
    }

    fn match_keyword(&mut self, kw: &str) -> bool {
        if let Some(Tok::Ident(s)) = self.peek() {
            if s.eq_ignore_ascii_case(kw) {
                self.bump();
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EvalContext, EvalError};

    fn ctx() -> EvalContext {
        EvalContext::new()
            .bind("status", Literal::Str("open".into()))
            .bind("severity", Literal::Int(3))
            .bind("flag", Literal::Bool(true))
    }

    #[test]
    fn parses_simple_string_equality() {
        let ast = parse_query("status == 'open'").unwrap();
        assert_eq!(ast.source(), "status == 'open'");
        assert_eq!(ast.eval(&ctx()), Ok(true));
        let ast2 = parse_query("status == 'closed'").unwrap();
        assert_eq!(ast2.eval(&ctx()), Ok(false));
    }

    #[test]
    fn parses_int_comparison() {
        assert_eq!(parse_query("severity >= 3").unwrap().eval(&ctx()), Ok(true));
        assert_eq!(parse_query("severity > 3").unwrap().eval(&ctx()), Ok(false));
        assert_eq!(parse_query("severity < 5").unwrap().eval(&ctx()), Ok(true));
        assert_eq!(parse_query("severity != 4").unwrap().eval(&ctx()), Ok(true));
    }

    #[test]
    fn parses_boolean_connectives_with_precedence() {
        // AND binds tighter than OR: `a OR b AND c` == `a OR (b AND c)`.
        let ast = parse_query("status == 'closed' OR severity >= 3 AND flag == true").unwrap();
        assert_eq!(ast.eval(&ctx()), Ok(true));
        // Parens override precedence.
        let ast2 = parse_query("(status == 'closed' OR severity >= 3) AND flag == false").unwrap();
        assert_eq!(ast2.eval(&ctx()), Ok(false));
    }

    #[test]
    fn parses_not_and_bang() {
        assert_eq!(parse_query("NOT status == 'closed'").unwrap().eval(&ctx()), Ok(true));
        assert_eq!(parse_query("!(severity > 10)").unwrap().eval(&ctx()), Ok(true));
    }

    #[test]
    fn parses_dotted_field_path() {
        let ast = parse_query("payload.status == 'open'").unwrap();
        let c = EvalContext::new().bind("payload.status", Literal::Str("open".into()));
        assert_eq!(ast.eval(&c), Ok(true));
    }

    #[test]
    fn parses_true_false_constants() {
        assert_eq!(parse_query("true").unwrap().eval(&EvalContext::new()), Ok(true));
        assert_eq!(parse_query("false").unwrap().eval(&EvalContext::new()), Ok(false));
    }

    #[test]
    fn string_escapes_are_honoured() {
        let ast = parse_query(r"status == 'it\'s open'").unwrap();
        let c = EvalContext::new().bind("status", Literal::Str("it's open".into()));
        assert_eq!(ast.eval(&c), Ok(true));
    }

    #[test]
    fn missing_context_surfaces_not_silent_true() {
        // The parsed tree fails closed exactly like a directly-built one.
        let ast = parse_query("status == 'open'").unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext { name: "status".into() })
        );
    }

    #[test]
    fn rejects_trailing_input() {
        let err = parse_query("status == 'open' garbage").unwrap_err();
        assert!(matches!(err, ParseError::TrailingInput { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_unterminated_string() {
        assert_eq!(parse_query("status == 'open").unwrap_err(), ParseError::UnterminatedString);
    }

    #[test]
    fn rejects_unexpected_char() {
        let err = parse_query("status == 'open' & flag").unwrap_err();
        // `&` alone is not a token (only `&&` would be, which we do not accept — use AND).
        assert!(matches!(err, ParseError::UnexpectedChar { ch: '&', .. }), "got {err:?}");
    }

    #[test]
    fn rejects_missing_operator() {
        let err = parse_query("status 'open'").unwrap_err();
        assert!(matches!(err, ParseError::Expected { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_bad_int() {
        let err = parse_query("severity == 999999999999999999999999").unwrap_err();
        assert!(matches!(err, ParseError::BadInt { .. }), "got {err:?}");
    }

    /// **A deeply-nested parenthesised string is rejected at PARSE time (the parser stack is
    /// statically bounded) — it never blows the stack.** The red half of the cost-bound fixture
    /// for the textual surface.
    #[test]
    fn rejects_overdeep_nesting_at_parse_time() {
        let deep = format!(
            "{}status == 'x'{}",
            "(".repeat(MAX_PARSE_DEPTH + 5),
            ")".repeat(MAX_PARSE_DEPTH + 5)
        );
        assert_eq!(parse_query(&deep).unwrap_err(), ParseError::TooDeep);
    }

    /// **A parsed predicate is re-validated against the ONE static cost bound** — a string that
    /// expands past [`crate::MAX_PREDICATE_NODES`] is rejected (defence in depth over the
    /// parse-depth guard). The green half: a modestly-sized expression parses fine.
    #[test]
    fn oversized_flat_expression_rejected_after_parse() {
        // A long flat OR chain that blows the node budget but stays shallow (so the depth guard
        // does NOT catch it — only the node re-validation does).
        let chain =
            std::iter::repeat_n("status == 'x'", crate::MAX_PREDICATE_NODES).collect::<Vec<_>>().join(" OR ");
        let err = parse_query(&chain).unwrap_err();
        assert!(matches!(err, ParseError::Oversized(PredicateError::TooLarge { .. })), "got {err:?}");

        // The green half: a handful of conjoined comparisons parses + validates fine.
        let ok = "status == 'open' AND severity >= 1 AND flag == true";
        assert!(parse_query(ok).is_ok());
    }

    /// **A round-trip-stable parse: the same string always compiles to the same tree** (the
    /// golden-determinism property the Issues + Knowledge co-owners both build to).
    #[test]
    fn parse_is_deterministic() {
        let src = "status == 'open' AND (severity >= 3 OR flag == false)";
        let a = parse_predicate(src).unwrap();
        let b = parse_predicate(src).unwrap();
        assert_eq!(a, b, "the parser is a pure function of its input");
    }
}
