//! # The chat instance of KN-D2 — `render(parse(md)) === md` 100% over a chat message corpus
//! (CHAT-P11 / P-405, M4-C3)
//!
//! **The gate (drill-catalogue: the chat instance of KN-D2 / the content-core round-trip):**
//! `render(parse(md)) === md` over a corpus of chat MESSAGE bodies (the consumed Chat subset; read +
//! send use the IDENTICAL WASM parser). The green artifact is **100% round-trip** (0 fixtures fail,
//! the round-trip-mismatch signal = 0 over the corpus), run on CI.
//!
//! This is the integration-boundary re-assertion of the embedded unit tests: it feeds a corpus of
//! hand-authored message markdown-subset strings (the shapes real chat messages take) through the ONE
//! WASM render path the composer compiles to `wasm32-unknown-unknown`
//! ([`myelin_chat::roundtrips_md`] → [`myelin_content::wasm::render_parse`]/`render_serialize`) and
//! asserts every one is a byte-exact fixed point. Read + send use this IDENTICAL parser — there is no
//! Chat-local renderer (EI-01 §7). It also re-asserts the SHARED `myelin-content` corpus
//! (`myelin_content::corpus::CORPUS`) from the Chat consumer side, so a regression in the frozen
//! grammar fails THIS gate too (the consumer rides the same frozen taxonomy, X-2).

use myelin_chat::roundtrips_md;
use myelin_content::corpus::CORPUS;
use myelin_content::{InlineNode, OBJ};
use myelin_events::ArtifactRef;

/// Synthesise the positional structured-node array for a corpus string: one [`InlineNode`] per
/// `U+FFFC` placeholder (an `artifact_ref` to a corpus URN — the node payload does not affect the
/// markdown-subset round-trip, only the positional placeholder count must match).
fn synth(md: &str) -> Vec<InlineNode> {
    md.chars()
        .filter(|&c| c == OBJ)
        .map(|_| {
            InlineNode::ArtifactRefNode(ArtifactRef("myelin://acme/chat/message/01J0M".into()))
        })
        .collect()
}

/// A corpus of chat MESSAGE-body markdown-subset strings — the shapes a real chat message takes
/// (short prose, bold/italic/strike, inline code, links, snake_case/`f(x)` verbatim, code that holds
/// would-be delimiters). Each must round-trip byte-exact through the ONE WASM render path.
const MESSAGE_CORPUS: &[&str] = &[
    "",
    "hey, can you take a look?",
    "the **deploy** is *blocked* on the migration",
    "run `cargo test --workspace` before pushing",
    "~~ignore~~ that was the old plan",
    "see the [runbook](https://wiki.test/runbook) for the rollback steps",
    "Mixed **bold *and italic*** in a single message.",
    r"a literal asterisk \* and a bracket \[ stay literal",
    "snake_case ids and f(x) call syntax pass through verbatim",
    "100% green & shipping today",
    "`code containing * and ] and ) chars` is verbatim",
    "LGTM, merging",
    "**approved** — ship it",
    "nice catch on the `version` CAS",
];

/// Chat-message corpus strings WITH structured ref nodes (the @-mention / artifact_ref / embed
/// placeholders). The i-th `U+FFFC` binds the i-th synthesised node; the markdown-subset round-trip is
/// over the placeholder string (the node payload does not affect the string round-trip).
fn corpus_with_nodes() -> Vec<String> {
    vec![
        format!("cc {OBJ} please review"),                  // a mention
        format!("see {OBJ} for the root cause"),            // an artifact_ref
        format!("{OBJ} blocks this; also {OBJ}"),           // two refs
        format!("**{OBJ}** owns this"),                     // a node inside bold
        format!("[{OBJ}](https://x.test/p) inline"),        // a node inside a link
        format!("a {OBJ} mid-message {OBJ} and {OBJ} end"), // three refs
        format!("ping {OBJ} about {OBJ} — `not a {OBJ}?`"), // a node before a code span
    ]
}

/// **THE GATE — `render(parse(md)) === md` 100% over the chat message corpus.** Every message body +
/// node-bearing fixture round-trips byte-exact through the ONE WASM render path. A single failure
/// FAILS this gate (the green artifact is 100%, the round-trip-mismatch signal = 0 over the corpus).
#[test]
fn chat_message_corpus_round_trips_100_percent() {
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for &md in MESSAGE_CORPUS {
        total += 1;
        let nodes = synth(md);
        if roundtrips_md(md, &nodes) {
            passed += 1;
        } else {
            mismatches.push(format!("{md:?}"));
        }
    }

    for md in corpus_with_nodes() {
        total += 1;
        let nodes = synth(&md);
        if roundtrips_md(&md, &nodes) {
            passed += 1;
        } else {
            mismatches.push(format!("{md:?}"));
        }
    }

    assert_eq!(
        mismatches.len(),
        0,
        "the round-trip-mismatch signal must be 0 over the corpus; got: {mismatches:?}"
    );
    assert_eq!(
        passed, total,
        "round-trip must be 100% over the chat corpus"
    );
    assert!(total >= 20, "the corpus must be non-trivial (got {total})");
}

/// **The SHARED `myelin-content` corpus round-trips from the Chat consumer side too** — Chat rides the
/// frozen taxonomy (X-2), so a regression in the shared grammar fails this Chat gate as well (the
/// consumer never re-implements the grammar; it links the one frozen renderer).
#[test]
fn shared_content_corpus_round_trips_from_chat_consumer() {
    for f in CORPUS {
        let nodes = synth(f.md);
        assert!(
            roundtrips_md(f.md, &nodes),
            "shared corpus fixture {} did not round-trip via the Chat consumer path",
            f.name
        );
    }
}
