use crate::{CmpOp, Expr, Predicate, PredicateError, QueryAst};
use myelin_identity::Literal;

pub const MAX_PARSE_DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    UnexpectedChar { ch: char, at: usize },
    Expected { what: &'static str, found: String },
    UnexpectedEof,
    TrailingInput { rest: String },
    UnterminatedString,
    BadInt { text: String },
    TooDeep,
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
            ParseError::TrailingInput { rest } => {
                write!(f, "trailing input after expression: {rest:?}")
            }
            ParseError::UnterminatedString => write!(f, "unterminated string literal"),
            ParseError::BadInt { text } => write!(f, "integer literal out of range: {text:?}"),
            ParseError::TooDeep => {
                write!(
                    f,
                    "expression nesting exceeds the parse-depth ceiling ({MAX_PARSE_DEPTH})"
                )
            }
            ParseError::Oversized(e) => write!(f, "parsed predicate is over budget: {e}"),
        }
    }
}

impl std::error::Error for ParseError {}

pub fn parse_query(src: &str) -> Result<QueryAst, ParseError> {
    let predicate = parse_predicate(src)?;
    QueryAst::validate(&predicate).map_err(ParseError::Oversized)?;
    Ok(QueryAst::compiled_with_source(predicate, src))
}

pub fn parse_predicate(src: &str) -> Result<Predicate, ParseError> {
    let tokens = lex(src)?;
    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
        depth: 0,
    };
    let pred = p.parse_or()?;
    if p.pos != p.tokens.len() {
        return Err(ParseError::TrailingInput {
            rest: p.tokens[p.pos..]
                .iter()
                .map(|t| t.text())
                .collect::<Vec<_>>()
                .join(" "),
        });
    }
    Ok(pred)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
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
                    return Err(ParseError::Expected {
                        what: "`==`",
                        found: "=".into(),
                    });
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
                let ch = src[i..].chars().next().unwrap_or('\u{FFFD}');
                return Err(ParseError::UnexpectedChar { ch, at: i });
            }
        }
    }
    Ok(out)
}

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
            let ch_len = utf8_len(c);
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
        return Err(ParseError::Expected {
            what: "an integer",
            found: "-".into(),
        });
    }
    let text = &src[start..i];
    let n = text
        .parse::<i64>()
        .map_err(|_| ParseError::BadInt { text: text.into() })?;
    Ok((n, i))
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}
fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

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

    fn parse_comparison(&mut self) -> Result<Predicate, ParseError> {
        let lhs = self.parse_field()?;
        let op = self.parse_op()?;
        let rhs = self.parse_value()?;
        Ok(Predicate::Cmp { op, lhs, rhs })
    }

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
        let ast = parse_query("status == 'closed' OR severity >= 3 AND flag == true").unwrap();
        assert_eq!(ast.eval(&ctx()), Ok(true));
        let ast2 = parse_query("(status == 'closed' OR severity >= 3) AND flag == false").unwrap();
        assert_eq!(ast2.eval(&ctx()), Ok(false));
    }

    #[test]
    fn parses_not_and_bang() {
        assert_eq!(
            parse_query("NOT status == 'closed'").unwrap().eval(&ctx()),
            Ok(true)
        );
        assert_eq!(
            parse_query("!(severity > 10)").unwrap().eval(&ctx()),
            Ok(true)
        );
    }

    #[test]
    fn parses_dotted_field_path() {
        let ast = parse_query("payload.status == 'open'").unwrap();
        let c = EvalContext::new().bind("payload.status", Literal::Str("open".into()));
        assert_eq!(ast.eval(&c), Ok(true));
    }

    #[test]
    fn parses_true_false_constants() {
        assert_eq!(
            parse_query("true").unwrap().eval(&EvalContext::new()),
            Ok(true)
        );
        assert_eq!(
            parse_query("false").unwrap().eval(&EvalContext::new()),
            Ok(false)
        );
    }

    #[test]
    fn string_escapes_are_honoured() {
        let ast = parse_query(r"status == 'it\'s open'").unwrap();
        let c = EvalContext::new().bind("status", Literal::Str("it's open".into()));
        assert_eq!(ast.eval(&c), Ok(true));
    }

    #[test]
    fn missing_context_surfaces_not_silent_true() {
        let ast = parse_query("status == 'open'").unwrap();
        assert_eq!(
            ast.eval(&EvalContext::new()),
            Err(EvalError::MissingContext {
                name: "status".into()
            })
        );
    }

    #[test]
    fn rejects_trailing_input() {
        let err = parse_query("status == 'open' garbage").unwrap_err();
        assert!(
            matches!(err, ParseError::TrailingInput { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_unterminated_string() {
        assert_eq!(
            parse_query("status == 'open").unwrap_err(),
            ParseError::UnterminatedString
        );
    }

    #[test]
    fn rejects_unexpected_char() {
        let err = parse_query("status == 'open' & flag").unwrap_err();
        assert!(
            matches!(err, ParseError::UnexpectedChar { ch: '&', .. }),
            "got {err:?}"
        );
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

    #[test]
    fn rejects_overdeep_nesting_at_parse_time() {
        let deep = format!(
            "{}status == 'x'{}",
            "(".repeat(MAX_PARSE_DEPTH + 5),
            ")".repeat(MAX_PARSE_DEPTH + 5)
        );
        assert_eq!(parse_query(&deep).unwrap_err(), ParseError::TooDeep);
    }

    #[test]
    fn oversized_flat_expression_rejected_after_parse() {
        let chain = std::iter::repeat_n("status == 'x'", crate::MAX_PREDICATE_NODES)
            .collect::<Vec<_>>()
            .join(" OR ");
        let err = parse_query(&chain).unwrap_err();
        assert!(
            matches!(err, ParseError::Oversized(PredicateError::TooLarge { .. })),
            "got {err:?}"
        );

        let ok = "status == 'open' AND severity >= 1 AND flag == true";
        assert!(parse_query(ok).is_ok());
    }

    #[test]
    fn parse_is_deterministic() {
        let src = "status == 'open' AND (severity >= 3 OR flag == false)";
        let a = parse_predicate(src).unwrap();
        let b = parse_predicate(src).unwrap();
        assert_eq!(a, b, "the parser is a pure function of its input");
    }
}
