use myelin_content::corpus::CORPUS;
use myelin_content::{parse_inline, serialize_inline, InlineNode, OBJ};
use myelin_events::ArtifactRef;

fn synth(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| InlineNode::Embed(ArtifactRef("myelin://corpus/n".into())))
        .collect()
}

#[test]
fn corpus_roundtrips_byte_identical() {
    for f in CORPUS {
        let nodes = synth(f.md);
        let got = serialize_inline(&parse_inline(f.md, &nodes));
        assert_eq!(got, f.md, "fixture {} did not round-trip", f.name);
    }
}

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

#[test]
fn generated_canonical_inputs_roundtrip() {
    let pieces = ["plain", "**b**", "*i*", "`c`", "~~s~~", "[t](u)"];
    let mut cases = Vec::new();
    for a in pieces {
        for b in pieces {
            cases.push(format!("{a} sep {b}"));
        }
    }
    cases.push(format!("pre {OBJ} mid {OBJ} post"));
    cases.push(format!("**{OBJ}**"));
    cases.push(format!("[{OBJ}](https://x.test/p)"));

    for md in &cases {
        let nodes = synth(md);
        let got = serialize_inline(&parse_inline(md, &nodes));
        assert_eq!(&got, md, "generated case did not round-trip");
    }
}

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
