use myelin_events::ArtifactRef;
use myelin_identity::Principal;
use serde::{Deserialize, Serialize};

pub const OBJ: char = '\u{FFFC}';

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineNode {
    Mention(Principal),
    ArtifactRefNode(ArtifactRef),
    Embed(ArtifactRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Span {
    Text {
        text: String,
        marks: Vec<Mark>,
        link: Option<String>,
    },
    Node {
        marks: Vec<Mark>,
        link: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Bold,
    Italic,
    Code,
    Strike,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Inline {
    pub spans: Vec<Span>,
    pub nodes: Vec<InlineNode>,
}

impl Inline {
    pub fn structured_nodes(&self) -> &[InlineNode] {
        &self.nodes
    }
}

const ESCAPABLE: &[char] = &['\\', '*', '`', '~', '['];

pub fn parse_inline(md: &str, nodes: &[InlineNode]) -> Inline {
    let chars: Vec<char> = md.chars().collect();
    let mut spans = Vec::new();
    parse_runs(&chars, &mut spans, &[], None);
    Inline {
        spans,
        nodes: nodes.to_vec(),
    }
}

fn parse_runs(chars: &[char], out: &mut Vec<Span>, active: &[Mark], link: Option<&str>) {
    let mut i = 0;
    let mut literal = String::new();

    macro_rules! flush {
        () => {
            if !literal.is_empty() {
                out.push(Span::Text {
                    text: std::mem::take(&mut literal),
                    marks: active.to_vec(),
                    link: link.map(|s| s.to_string()),
                });
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];

        if c == OBJ {
            flush!();
            out.push(Span::Node {
                marks: active.to_vec(),
                link: link.map(|s| s.to_string()),
            });
            i += 1;
            continue;
        }

        if c == '\\' && i + 1 < chars.len() && ESCAPABLE.contains(&chars[i + 1]) {
            literal.push(chars[i + 1]);
            i += 2;
            continue;
        }

        if c == '`' && !active.contains(&Mark::Code) {
            if let Some(end) = find_delim(chars, i + 1, '`', 1) {
                flush!();
                let inner: String = chars[i + 1..end].iter().collect();
                let mut marks = active.to_vec();
                marks.push(Mark::Code);
                out.push(Span::Text {
                    text: inner,
                    marks,
                    link: link.map(|s| s.to_string()),
                });
                i = end + 1;
                continue;
            }
        }

        if c == '*' && peek(chars, i + 1) == Some('*') && !active.contains(&Mark::Bold) {
            if let Some(end) = find_delim(chars, i + 2, '*', 2) {
                flush!();
                let mut marks = active.to_vec();
                marks.push(Mark::Bold);
                parse_runs(&chars[i + 2..end], out, &marks, link);
                i = end + 2;
                continue;
            }
        }

        if c == '*' && peek(chars, i + 1) != Some('*') && !active.contains(&Mark::Italic) {
            if let Some(end) = find_delim(chars, i + 1, '*', 1) {
                flush!();
                let mut marks = active.to_vec();
                marks.push(Mark::Italic);
                parse_runs(&chars[i + 1..end], out, &marks, link);
                i = end + 1;
                continue;
            }
        }

        if c == '~' && peek(chars, i + 1) == Some('~') && !active.contains(&Mark::Strike) {
            if let Some(end) = find_delim(chars, i + 2, '~', 2) {
                flush!();
                let mut marks = active.to_vec();
                marks.push(Mark::Strike);
                parse_runs(&chars[i + 2..end], out, &marks, link);
                i = end + 2;
                continue;
            }
        }

        if c == '[' && link.is_none() {
            if let Some((text_end, url, close)) = parse_link(chars, i) {
                flush!();
                let url_owned = url.clone();
                parse_runs(&chars[i + 1..text_end], out, active, Some(&url_owned));
                i = close;
                continue;
            }
        }

        literal.push(c);
        i += 1;
    }
    flush!();
}

fn peek(chars: &[char], i: usize) -> Option<char> {
    chars.get(i).copied()
}

fn find_delim(chars: &[char], from: usize, delim: char, width: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 2;
            continue;
        }
        if chars[i] == delim {
            let mut run = 0;
            while i + run < chars.len() && chars[i + run] == delim {
                run += 1;
            }
            if run >= width {
                return Some(i + run - width);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

fn parse_link(chars: &[char], open: usize) -> Option<(usize, String, usize)> {
    let mut i = open + 1;
    let mut found = false;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 2;
            continue;
        }
        if chars[i] == ']' {
            found = true;
            break;
        }
        i += 1;
    }
    if !found {
        return None;
    }
    let text_end = i;
    if peek(chars, i + 1) != Some('(') {
        return None;
    }
    let mut j = i + 2;
    let mut url = String::new();
    while j < chars.len() {
        if chars[j] == '\\' && peek(chars, j + 1) == Some(')') {
            url.push(')');
            j += 2;
            continue;
        }
        if chars[j] == ')' {
            return Some((text_end, url, j + 1));
        }
        url.push(chars[j]);
        j += 1;
    }
    None
}

pub fn serialize_inline(inline: &Inline) -> String {
    let mut s = String::new();
    serialize_spans(&inline.spans, &mut s);
    s
}

fn serialize_spans(spans: &[Span], out: &mut String) {
    let mut open: Vec<Mark> = Vec::new();
    let mut open_link: Option<String> = None;

    fn close_marks(open: &mut Vec<Mark>, keep: usize, out: &mut String) {
        while open.len() > keep {
            out.push_str(close_delim(open.pop().unwrap()));
        }
    }
    fn close_link(open_link: &mut Option<String>, out: &mut String) {
        if let Some(url) = open_link.take() {
            out.push_str("](");
            for c in url.chars() {
                if c == ')' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push(')');
        }
    }

    fn enter(
        open: &mut Vec<Mark>,
        open_link: &mut Option<String>,
        marks: &[Mark],
        link: Option<&str>,
        out: &mut String,
    ) {
        if open_link.as_deref() != link {
            close_marks(open, 0, out);
            close_link(open_link, out);
            if let Some(url) = link {
                out.push('[');
                *open_link = Some(url.to_string());
            }
        }
        let want: Vec<Mark> = marks.iter().copied().filter(|m| *m != Mark::Code).collect();
        let common = open
            .iter()
            .zip(want.iter())
            .take_while(|(a, b)| a == b)
            .count();
        close_marks(open, common, out);
        for &m in &want[common..] {
            out.push_str(open_delim(m));
            open.push(m);
        }
    }

    for span in spans {
        match span {
            Span::Node { marks, link } => {
                enter(&mut open, &mut open_link, marks, link.as_deref(), out);
                out.push(OBJ);
            }
            Span::Text { text, marks, link } => {
                enter(&mut open, &mut open_link, marks, link.as_deref(), out);
                emit_leaf(text, marks.contains(&Mark::Code), out);
            }
        }
    }
    close_marks(&mut open, 0, out);
    close_link(&mut open_link, out);
}

fn open_delim(m: Mark) -> &'static str {
    match m {
        Mark::Bold => "**",
        Mark::Italic => "*",
        Mark::Strike => "~~",
        Mark::Code => "`",
    }
}
fn close_delim(m: Mark) -> &'static str {
    open_delim(m)
}

fn emit_leaf(text: &str, is_code: bool, out: &mut String) {
    if is_code {
        out.push('`');
        out.push_str(text);
        out.push('`');
    } else {
        out.push_str(&escape(text));
    }
}

fn escape(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars() {
        if ESCAPABLE.contains(&c) {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn rt(md: &str, nodes: &[InlineNode]) {
        let parsed = parse_inline(md, nodes);
        assert_eq!(
            serialize_inline(&parsed),
            md,
            "round-trip mismatch for {md:?}"
        );
        assert_eq!(parsed.nodes, nodes, "node array not preserved for {md:?}");
    }

    #[test]
    fn plain_text() {
        rt("hello world", &[]);
    }

    #[test]
    fn bold_italic_code_strike() {
        rt("**bold**", &[]);
        rt("*italic*", &[]);
        rt("`code`", &[]);
        rt("~~strike~~", &[]);
    }

    #[test]
    fn link() {
        rt("[text](https://example.com/x)", &[]);
    }

    #[test]
    fn nested_marks() {
        rt("**bold *and italic***", &[]);
        rt("a **b `c` d** e", &[]);
    }

    #[test]
    fn link_structure_is_exact() {
        let p = parse_inline("a [t](u) b", &[]);
        let link_runs: Vec<_> = p
            .spans
            .iter()
            .filter_map(|s| match s {
                Span::Text {
                    text,
                    link: Some(u),
                    ..
                } => Some((text.clone(), u.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(link_runs, vec![("t".to_string(), "u".to_string())]);
        assert!(p
            .spans
            .iter()
            .any(|s| matches!(s, Span::Text { text, link: None, .. } if text == "a ")));
        assert!(p
            .spans
            .iter()
            .any(|s| matches!(s, Span::Text { text, link: None, .. } if text == " b")));
    }

    #[test]
    fn link_edge_cases_roundtrip() {
        rt("[**bold link**](https://x.test/p)", &[]);
        rt(r"[a \* b](u)", &[]);
        rt(r"[t](https://x.test/a\)b)", &[]);
        rt("before [t](u) after", &[]);
        rt("[t](u)", &[]);
        rt("[t](path/with*star)", &[]);
    }

    #[test]
    fn unterminated_link_text_is_not_a_link() {
        for raw in [r"[no close", r"[trailing backslash \", r"[abc"] {
            let p = parse_inline(raw, &[]);
            assert!(
                p.spans
                    .iter()
                    .all(|s| matches!(s, Span::Text { link: None, .. })),
                "{raw:?} must not parse as a link"
            );
            let once = serialize_inline(&p);
            let twice = serialize_inline(&parse_inline(&once, &[]));
            assert_eq!(once, twice, "not idempotent for {raw:?}");
        }
    }

    #[test]
    fn malformed_links_fall_back_to_literal() {
        let p = parse_inline("[text] no paren", &[]);
        assert!(p
            .spans
            .iter()
            .all(|s| matches!(s, Span::Text { link: None, .. })));
        let canon = serialize_inline(&p);
        assert_eq!(canon, r"\[text] no paren");
        rt(&canon, &[]);

        let p2 = parse_inline("[text](unterminated", &[]);
        assert!(p2
            .spans
            .iter()
            .all(|s| matches!(s, Span::Text { link: None, .. })));
        assert_eq!(serialize_inline(&p2), r"\[text](unterminated");
    }

    #[test]
    fn code_is_verbatim() {
        let parsed = parse_inline("`a*b`", &[]);
        assert_eq!(serialize_inline(&parsed), "`a*b`");
    }

    #[test]
    fn escaped_delimiter_roundtrips() {
        rt(r"a \* b", &[]);
        rt(r"\[not a link]", &[]);
    }

    #[test]
    fn structured_node_placeholder_binds_positionally() {
        let nodes = vec![InlineNode::Mention(Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        ))];
        let md = format!("hi {OBJ} there");
        let parsed = parse_inline(&md, &nodes);
        assert_eq!(serialize_inline(&parsed), md);
        let node_count = parsed
            .spans
            .iter()
            .filter(|s| matches!(s, Span::Node { .. }))
            .count();
        assert_eq!(node_count, 1);
        assert_eq!(parsed.structured_nodes().len(), 1);
    }

    #[test]
    fn three_structured_nodes_extract_as_node_walk() {
        let nodes = vec![
            InlineNode::Mention(Principal::stub(
                PrincipalId("bob".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/issues/issue/PROJ-1".into())),
            InlineNode::Embed(ArtifactRef("myelin://acme/knowledge/page/42".into())),
        ];
        let md = format!("{OBJ} filed {OBJ} see {OBJ}");
        let parsed = parse_inline(&md, &nodes);
        assert_eq!(serialize_inline(&parsed), md);
        assert_eq!(parsed.structured_nodes().len(), 3);
    }
}
