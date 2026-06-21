//! KN-D2 property-style round-trip tests (the corpus gate from the OTHER side: not just
//! the frozen fixtures, but generated inputs over the subset alphabet). The invariant:
//! `serialize_inline(parse_inline(md)) == md` for every *canonical* markdown-subset
//! string, and `parse∘serialize∘parse == parse` (the serializer's output re-parses to an
//! identical AST — idempotency) for ANY input.
//!
//! These complement the embedded `corpus::CORPUS` gate; together they are the KN-D2
//! green artifact (100% round-trip, 0 regressions).

use myelin_content::corpus::CORPUS;
use myelin_content::{parse_inline, serialize_inline, InlineNode, OBJ};
use myelin_events::ArtifactRef;

fn synth(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| InlineNode::Embed(ArtifactRef("myelin://corpus/n".into())))
        .collect()
}

/// Every frozen corpus fixture round-trips byte-identically (the headline KN-D2 gate,
/// re-asserted from the integration boundary).
#[test]
fn corpus_roundtrips_byte_identical() {
    for f in CORPUS {
        let nodes = synth(f.md);
        let got = serialize_inline(&parse_inline(f.md, &nodes));
        assert_eq!(got, f.md, "fixture {} did not round-trip", f.name);
    }
}

/// Idempotency on ARBITRARY input: even when an input is NOT in canonical form (e.g. an
/// unbalanced `*`), `serialize(parse(x))` is a fixed point — parsing it again yields the
/// same string. This is the stability property the editor relies on (a normalise-on-
/// serialize pass converges).
#[test]
fn serialize_is_idempotent_on_arbitrary_input() {
    let inputs = [
        "unbalanced * star",
        "**no close",
        "trailing ~~",
        "[broken](url",
        "mixed **a *b** c*",
        "weird `code with ] and ) chars`",
        "snake_case f(x) stays literal",
        "100% done & ok",
    ];
    for raw in inputs {
        let nodes = synth(raw);
        let once = serialize_inline(&parse_inline(raw, &nodes));
        let twice = serialize_inline(&parse_inline(&once, &nodes));
        assert_eq!(once, twice, "serialize not idempotent for {raw:?}");
    }
}

/// Generated canonical inputs over the subset alphabet round-trip. We build canonically-
/// formed strings (balanced delimiters, properly escaped literals) and assert byte-exact
/// round-trip — a wider net than the hand-authored corpus.
#[test]
fn generated_canonical_inputs_roundtrip() {
    let pieces = ["plain", "**b**", "*i*", "`c`", "~~s~~", "[t](u)"];
    let mut cases = Vec::new();
    for a in pieces {
        for b in pieces {
            cases.push(format!("{a} sep {b}"));
        }
    }
    // a few with structured-node placeholders interleaved
    cases.push(format!("pre {OBJ} mid {OBJ} post"));
    cases.push(format!("**{OBJ}**"));
    cases.push(format!("[{OBJ}](https://x.test/p)"));

    for md in &cases {
        let nodes = synth(md);
        let got = serialize_inline(&parse_inline(md, &nodes));
        assert_eq!(&got, md, "generated case did not round-trip");
    }
}

/// The positional node binding (§2.2): the i-th `U+FFFC` binds `nodes[i]`, and the node
/// array survives parse→serialize unchanged (reference-extraction is a node-array walk).
#[test]
fn node_array_is_preserved_positionally() {
    let nodes = vec![
        InlineNode::ArtifactRefNode(ArtifactRef("myelin://t/a/1".into())),
        InlineNode::Embed(ArtifactRef("myelin://t/b/2".into())),
    ];
    let md = format!("see {OBJ} and {OBJ}");
    let parsed = parse_inline(&md, &nodes);
    assert_eq!(parsed.nodes, nodes);
    assert_eq!(serialize_inline(&parsed), md);
}
