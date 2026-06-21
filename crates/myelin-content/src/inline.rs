//! The canonical inline grammar (architecture
//! `04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §2.2; contract-index 13.1, frozen X-2/OQ-B).
//!
//! Inline content is a **markdown-subset string** plus a positional array of three
//! **structured nodes** (`mention`/`artifact_ref`/`embed`). The subset is
//! `**bold**`, `*italic*`, `` `code` ``, `~~strike~~`, `[text](url)`. Each structured
//! node round-trips as the single Unicode **Object Replacement Character `U+FFFC`** at
//! the node's offset; the binding is **positional** — the i-th `U+FFFC` in the string
//! binds to `inline_nodes[i]` (§2.2). The string therefore never carries an id, and
//! reference-extraction is a node-array walk, never a regex over prose.
//!
//! ## The one render path (KN-4, architecture 02 §8 / contract 13.1 WASM target)
//! [`parse_inline`] and [`serialize_inline`] are **one implementation** compiled native
//! (server) and to `wasm32-unknown-unknown` (client). There is no second renderer, so
//! the two-divergent-renderers trap (EI-01 §7) is eliminated structurally. The frozen
//! correctness bar is the round-trip invariant
//! `serialize_inline(parse_inline(md)) == md` over the frozen corpus (KN-D2).

use myelin_events::ArtifactRef;
use myelin_identity::Principal;
use serde::{Deserialize, Serialize};

/// The Unicode Object Replacement Character — the single-char logical placeholder a
/// structured inline node occupies in the markdown-subset string (§2.2). Exactly one
/// caret position; positional binding to [`Inline::nodes`].
pub const OBJ: char = '\u{FFFC}';

/// The three platform-load-bearing structured inline nodes (architecture §2.2). These
/// three uniformly produce `refs.edge.created` via the outbox (contract 5.4),
/// **uniformly across Chat, Issues, and Knowledge**. They are stored structured (never
/// embedded in the prose string) precisely so reference-extraction stays reliable
/// server-side (a node-array walk, never a regex over prose).
///
/// This is the frozen X-2/OQ-B surface that `myelin-refs` / `myelin-notif` /
/// `myelin-agent` already consume by `match` and over the JSON wire (the original P-001
/// substrate-seam shape); KN-P01 freezes it in place — the tuple variants and their
/// names are part of the frozen contract and must not change without a whole-workspace
/// contract PR (code-wins-over-docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InlineNode {
    /// `@alice` — an @-mention of a principal; renders to a per-viewer display name
    /// (REF-3). Carries the structured [`Principal`] target.
    Mention(Principal),
    /// A typed reference to any artifact — the PRODUCER of `refs.edge.created` (5.4).
    ArtifactRefNode(ArtifactRef),
    /// An inline unfurl / transclusion request for an artifact.
    Embed(ArtifactRef),
}

/// A run of text carrying a (possibly empty) set of inline marks. Plain text is a run
/// with no marks. Link runs carry the destination url.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Span {
    /// A maximal run of text under one set of marks. `text` is the raw run content with
    /// markdown escapes already resolved; `marks` is the active mark set (innermost
    /// last is irrelevant — marks are a set).
    Text { text: String, marks: Vec<Mark>, link: Option<String> },
    /// A structured node placeholder. Serializes to a single [`OBJ`] code point; the
    /// payload lives in the positional [`Inline::nodes`] array. It carries the mark/link
    /// context it sits inside (a `**￼**` mention keeps its bold), so the node is one
    /// caret position WITHIN formatted text (architecture 02 §8.2).
    Node { marks: Vec<Mark>, link: Option<String> },
}

/// The five markdown-subset marks (§2.2). `Link` carries no data in the mark set (the
/// url lives on the [`Span`]) so that the mark set stays a flat comparable set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    Bold,
    Italic,
    Code,
    Strike,
}

/// The parsed inline value: the ordered run sequence + the positional structured-node
/// array. The i-th [`Span::Node`] in `spans` binds to `nodes[i]`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Inline {
    pub spans: Vec<Span>,
    pub nodes: Vec<InlineNode>,
}

impl Inline {
    /// Walk the structured nodes (reference-extraction is a node-array walk, never a
    /// regex over prose — §2.2). This is the seam Refs/Search read to emit
    /// `refs.edge.created` uniformly.
    pub fn structured_nodes(&self) -> &[InlineNode] {
        &self.nodes
    }
}

// --- the markdown-subset grammar (frozen v1) ---------------------------------------
//
// The grammar is deliberately a *strict subset* of CommonMark chosen for lossless
// round-trip (architecture 02 §8.3: a markdown-subset string, not inline-range JSON):
//
//   `code`        — backtick spans; inner content is verbatim (no nested marks)
//   **bold**      — double-asterisk
//   *italic*      — single-asterisk
//   ~~strike~~    — double-tilde
//   [text](url)   — link; `text` may itself carry marks
//   U+FFFC        — a structured node placeholder (positional bind to `nodes`)
//
// A backslash escapes the next byte if it is one of the active delimiter bytes or the
// backslash itself, so a literal `*` is `\*`. Round-trip is guaranteed because
// `serialize_inline` re-emits exactly the escapes the parser consumed.
//
// FROZEN disambiguation rule (the one place markdown is ambiguous): a closing delimiter
// run that is LONGER than the opener (`***` closing a `**`) is matched at its LAST
// `width` characters, so surplus delimiters stay as inner content and bind the nested
// emphasis — `**bold *and italic***` parses as bold(`bold ` + italic(`and italic`)).
// This favours nested emphasis (the common authoring shape) over adjacent emphasis;
// adjacent emphasis is written with a separator (`**a** *b*`). This single rule makes
// the round-trip byte-stable; it is part of the frozen v1 grammar (contract 13.1).

/// The characters the parser treats specially and therefore the serializer must escape
/// when they appear as *literal* text. This is exactly the active delimiter set of the
/// frozen subset — `_`/`(`/`)` are NOT delimiters in this grammar (only `*`/`` ` ``/`~`
/// open marks, `[`/`]` open links, `\` escapes), so they are never escaped and pass
/// through verbatim (keeps prose like `snake_case` and `f(x)` byte-stable).
const ESCAPABLE: &[char] = &['\\', '*', '`', '~', '['];

/// Parse a markdown-subset string + its positional structured-node array into an
/// [`Inline`]. The `nodes` argument is the stored `inline_nodes` array (§2.3); the i-th
/// [`OBJ`] in `md` binds to `nodes[i]`. Parsing never fails on arbitrary input — an
/// unbalanced delimiter is treated as literal text, so every byte sequence round-trips.
pub fn parse_inline(md: &str, nodes: &[InlineNode]) -> Inline {
    let chars: Vec<char> = md.chars().collect();
    let mut spans = Vec::new();
    parse_runs(&chars, &mut spans, &[], None);
    Inline {
        spans,
        nodes: nodes.to_vec(),
    }
}

/// Recursive-descent over a char slice. `active` are the marks already open at this
/// nesting level; `link` is the link url if these runs sit inside a `[…](url)`.
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

        // structured-node placeholder — carries its mark/link context (one caret
        // position within formatted text)
        if c == OBJ {
            flush!();
            out.push(Span::Node {
                marks: active.to_vec(),
                link: link.map(|s| s.to_string()),
            });
            i += 1;
            continue;
        }

        // escape
        if c == '\\' && i + 1 < chars.len() && ESCAPABLE.contains(&chars[i + 1]) {
            literal.push(chars[i + 1]);
            i += 2;
            continue;
        }

        // `code` — verbatim, no nested marks, only valid when not already inside code
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

        // **bold**
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

        // *italic* (single, and the next char is not another '*')
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

        // ~~strike~~
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

        // [text](url)
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

/// Find the closing delimiter `delim` repeated `width` times starting at `from`, that is
/// not escaped. Returns the index of the first delimiter char of the closing run.
fn find_delim(chars: &[char], from: usize, delim: char, width: usize) -> Option<usize> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 2;
            continue;
        }
        if chars[i] == delim {
            // count the contiguous delimiter run
            let mut run = 0;
            while i + run < chars.len() && chars[i + run] == delim {
                run += 1;
            }
            if run >= width {
                // The closing delimiter is the LAST `width` chars of the run, so any
                // surplus delimiters (`***` closing a `**`) stay as inner content — e.g.
                // `**bold *and italic***` closes bold on the final `**`, leaving the
                // inner `*` to close the italic. This is what makes the nesting
                // round-trip byte-stable.
                return Some(i + run - width);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// Parse a `[text](url)` starting at `open` (the `[`). Returns
/// `(text_end_idx, url, after_close_idx)` where `text_end_idx` is the `]` position and
/// `after_close_idx` is one past the final `)`.
fn parse_link(chars: &[char], open: usize) -> Option<(usize, String, usize)> {
    // Find the matching `]` (a `\` escapes the next char, so a structured-node or marked
    // run inside the text is fine). v1 LIMITATION: link text cannot contain a literal `]`
    // (it terminates the link); authors needing a `]` in link text is out of the frozen
    // v1 subset. The `(` must immediately follow the `]`.
    let mut i = open + 1;
    let mut found = false;
    while i < chars.len() {
        // a `\` escapes the next char inside link text (so `\]` is a literal bracket, not
        // the terminator); a trailing lone `\` (no next char) is just literal.
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
        // Inside a url only `)` is special (it would otherwise terminate the url); a
        // `\)` is a literal paren. Every other char — including `\` and `*` — is taken
        // verbatim, so urls round-trip without spurious escaping. The serializer escapes
        // a literal `)` back to `\)`.
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

/// Serialize an [`Inline`] back to its canonical markdown-subset string. The result is
/// **byte-identical** to the input of [`parse_inline`] for every frozen-corpus fixture
/// (KN-D2). The i-th [`Span::Node`] emits one [`OBJ`]; structured payloads are not
/// rendered into the string (they live in `nodes`).
pub fn serialize_inline(inline: &Inline) -> String {
    let mut s = String::new();
    serialize_spans(&inline.spans, &mut s);
    s
}

/// Serialize the flat run sequence back to the canonical markdown-subset string by
/// **diffing the mark stack between adjacent runs** — the standard nesting-aware inline
/// serialization. Marks open in the canonical order `bold → italic → strike` (the
/// outer→inner nesting the parser produces) and close in reverse, so a nested
/// `**bold *and italic***` re-emits with exactly one delimiter pair per mark. `code`
/// and links are per-run wrappers (the subset forbids them spanning a mark boundary).
fn serialize_spans(spans: &[Span], out: &mut String) {
    // Link is the OUTERMOST wrapper, then marks nest in the SOURCE nesting order the
    // parser recorded (`marks` is ordered outer→inner, code excluded). We diff both the
    // link state and the mark stack between adjacent runs so adjacent runs sharing a
    // link / a mark prefix share their delimiters. Preserving the parsed order (rather
    // than forcing a canonical order) is what makes `serialize(parse(md)) == md` hold
    // for any nesting the parser accepts, not only a canonicalised subset.
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
            // a literal `)` in the url is escaped back to `\)` (the inverse of parse_link)
            for c in url.chars() {
                if c == ')' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push(')');
        }
    }

    // Bring the open link + mark stack to the (marks, link) context a span requires,
    // sharing delimiters with the previous span where they agree. A structured node is
    // a leaf in this same stack, so `**￼**` keeps its bold around the node.
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
        // marks in the source nesting order, excluding the verbatim `code` leaf wrapper
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

/// Emit the leaf content of a run: `code` wrapping (verbatim, no escaping) or escaped
/// plain text. Mark + link delimiters are handled by the stack-diffing caller.
fn emit_leaf(text: &str, is_code: bool, out: &mut String) {
    if is_code {
        out.push('`');
        out.push_str(text); // verbatim — code is raw, NOT escaped
        out.push('`');
    } else {
        out.push_str(&escape(text));
    }
}

/// Re-emit exactly the escapes a canonical encoder needs: a delimiter byte appearing as
/// literal text is backslash-escaped. This is the inverse of the parser's escape rule,
/// so `serialize(parse(md)) == md` for canonically-escaped inputs.
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
        assert_eq!(serialize_inline(&parsed), md, "round-trip mismatch for {md:?}");
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

    /// Pin the PARSED STRUCTURE of a link (not just the round-trip), so a bounds mutation
    /// in `parse_link` that still happens to round-trip is still caught: the link text is
    /// its own marked run carrying the url, and the url is captured exactly.
    #[test]
    fn link_structure_is_exact() {
        let p = parse_inline("a [t](u) b", &[]);
        // expect: "a " | link-run "t"(link=u) | " b"
        let link_runs: Vec<_> = p
            .spans
            .iter()
            .filter_map(|s| match s {
                Span::Text { text, link: Some(u), .. } => Some((text.clone(), u.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(link_runs, vec![("t".to_string(), "u".to_string())]);
        // the surrounding text runs carry NO link
        assert!(p.spans.iter().any(|s| matches!(s, Span::Text { text, link: None, .. } if text == "a ")));
        assert!(p.spans.iter().any(|s| matches!(s, Span::Text { text, link: None, .. } if text == " b")));
    }

    /// Links with marks inside, escaped delimiters in the text, and escaped chars in the
    /// url — the parse_link escape/bounds paths the mutants flagged. Each must round-trip
    /// byte-exact.
    #[test]
    fn link_edge_cases_roundtrip() {
        rt("[**bold link**](https://x.test/p)", &[]);
        rt(r"[a \* b](u)", &[]);          // escaped delimiter inside link text
        rt(r"[t](https://x.test/a\)b)", &[]); // an escaped `)` inside the url
        rt("before [t](u) after", &[]);    // link not at string start
        rt("[t](u)", &[]);                 // link is the whole string (boundary)
        rt("[t](path/with*star)", &[]);    // `*` in a url is verbatim, not a delimiter
    }

    /// A `[` whose `]` runs to (or off) the end of the string is NOT a link — exercises
    /// the `i < chars.len()` / `i + 1 < chars.len()` link-text scan bounds (the mutants
    /// flagged here). Each canonicalises (the bare `[` → `\[`) and is a fixed point.
    #[test]
    fn unterminated_link_text_is_not_a_link() {
        for raw in [r"[no close", r"[trailing backslash \", r"[abc"] {
            let p = parse_inline(raw, &[]);
            assert!(
                p.spans.iter().all(|s| matches!(s, Span::Text { link: None, .. })),
                "{raw:?} must not parse as a link"
            );
            // serialize is idempotent (the canonical form re-parses to the same string)
            let once = serialize_inline(&p);
            let twice = serialize_inline(&parse_inline(&once, &[]));
            assert_eq!(once, twice, "not idempotent for {raw:?}");
        }
    }

    /// Malformed link shapes are NOT links — they fall back to literal text (the
    /// `]`-without-`(`, the unterminated url). The literal `[` is then CANONICALLY escaped
    /// on serialize (a bare `[` is reserved), and the escaped form round-trips. This kills
    /// the `peek == '('` and url-scan-termination mutants in `parse_link`.
    #[test]
    fn malformed_links_fall_back_to_literal() {
        // `]` not followed by `(` → not a link; the bare `[` canonicalises to `\[`
        let p = parse_inline("[text] no paren", &[]);
        assert!(p.spans.iter().all(|s| matches!(s, Span::Text { link: None, .. })));
        let canon = serialize_inline(&p);
        assert_eq!(canon, r"\[text] no paren");
        rt(&canon, &[]); // and the canonical form is a fixed point

        // unterminated url → not a link, the `[` is literal then canonicalises
        let p2 = parse_inline("[text](unterminated", &[]);
        assert!(p2.spans.iter().all(|s| matches!(s, Span::Text { link: None, .. })));
        assert_eq!(serialize_inline(&p2), r"\[text](unterminated");
    }

    #[test]
    fn code_is_verbatim() {
        // a `*` inside code is NOT a mark delimiter and is not escaped
        let parsed = parse_inline("`a*b`", &[]);
        assert_eq!(serialize_inline(&parsed), "`a*b`");
    }

    #[test]
    fn escaped_delimiter_roundtrips() {
        rt(r"a \* b", &[]);
        // `]` is not a delimiter in this grammar, so only `[` needs escaping
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
        // one OBJ ⇒ one Span::Node ⇒ binds nodes[0]
        let node_count = parsed.spans.iter().filter(|s| matches!(s, Span::Node { .. })).count();
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
