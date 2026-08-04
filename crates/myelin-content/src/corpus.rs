use crate::inline::{parse_inline, serialize_inline, InlineNode, OBJ};
use myelin_events::ArtifactRef;

pub struct Fixture {
    pub name: &'static str,
    pub md: &'static str,
}

macro_rules! corpus {
    ($($name:literal => $file:literal),* $(,)?) => {
        pub const CORPUS: &[Fixture] = &[
            $( Fixture { name: $name, md: include_str!(concat!("../corpus/", $file)) } ),*
        ];
    };
}

corpus! {
    "plain"              => "plain.md",
    "bold"               => "bold.md",
    "italic"             => "italic.md",
    "code"               => "code.md",
    "strike"             => "strike.md",
    "link"               => "link.md",
    "nested-marks"       => "nested_marks.md",
    "bold-in-list"       => "bold_in_list.md",
    "code-verbatim"      => "code_verbatim.md",
    "escaped-delimiters" => "escaped_delimiters.md",
    "mention-in-bold"    => "mention_in_bold.md",
    "ref-in-list"        => "ref_in_list.md",
    "embed-in-table"     => "embed_in_table.md",
    "three-nodes"        => "three_nodes.md",
    "ime-paste"          => "ime_paste.md",
    "empty"              => "empty.md",
    "adjacent-marks"     => "adjacent_marks.md",
    "link-with-marks"    => "link_with_marks.md",
}

pub fn synthetic_nodes_for(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| InlineNode::ArtifactRefNode(ArtifactRef("myelin://corpus/node".into())))
        .collect()
}

pub fn roundtrip(md: &str) -> Result<(), (String, String)> {
    let nodes = synthetic_nodes_for(md);
    let got = serialize_inline(&parse_inline(md, &nodes));
    if got == md {
        Ok(())
    } else {
        Err((got, md.to_string()))
    }
}

pub fn corpus_pass_rate() -> (usize, usize) {
    let passed = CORPUS.iter().filter(|f| roundtrip(f.md).is_ok()).count();
    (passed, CORPUS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kn_d2_corpus_roundtrips_100_percent() {
        let mut failures = Vec::new();
        for f in CORPUS {
            if let Err((got, want)) = roundtrip(f.md) {
                failures.push(format!("  [{}] got {got:?} want {want:?}", f.name));
            }
        }
        assert!(
            failures.is_empty(),
            "KN-D2 corpus regressions ({}/{} passed):\n{}",
            CORPUS.len() - failures.len(),
            CORPUS.len(),
            failures.join("\n")
        );
        let (passed, total) = corpus_pass_rate();
        assert_eq!(passed, total, "corpus-pass-rate must be 100%");
        assert!(
            total >= 18,
            "corpus must not be shrunk below its frozen size"
        );
    }
}
