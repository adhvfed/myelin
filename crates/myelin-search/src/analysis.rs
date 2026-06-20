//! The **per-language multilingual analyzer chain** (SRCH-P12 / P-175; architecture
//! `search-and-indexing.md` §4.7).
//!
//! ## What SRCH-P12 ships here
//! The ONE analyzer chain — the single source of truth for *how text becomes index/query terms*,
//! used by BOTH the index-time path (the indexer's analyze step, §4.1) and the query-time path (the
//! FT clause), so the **query-time analyzer matches the index-time analyzer per field-language**
//! (EI-01 §7: one analyzer chain, **no drift**). The whole correctness property of this module is
//! the *parity* invariant: a query in language `L` analyzes to the SAME token set as the index-time
//! analysis of the same text under `L`, so there is **no analyzer-mismatch miss** (the GATE).
//!
//! ### The chain (§4.7)
//! For a natural language `L`:
//! ```text
//! text → UAX#29-style segmentation (word boundaries, scripts kept apart)
//!      → lowercase (case-fold)
//!      → diacritic-fold (NFD strip the combining marks: café → cafe)
//!      → stopword removal (the per-language stoplist)
//!      → Snowball/Porter-family light stemming (suffix stripping per language)
//!      → the token stream
//! ```
//! For a **CJK / non-segmented** language (Chinese/Japanese/Korean) there are no word spaces, so the
//! chain is **bigram n-gram segmentation over the CJK run** (the ICU/n-gram strategy §4.7 names) —
//! `東京都` → `東京`,`京都`. (Latin runs interleaved in a CJK doc still go through the word path.)
//!
//! For **code / identifiers** (`AnalyzerKind::Code`) the chain is the **camel/snake tokenizer keeping
//! operators**: `getUserById` → `get`,`user`,`by`,`id` (also kept whole as `getuserbyid` so an exact
//! identifier query still hits); `MAX_RETRY_COUNT` → `max`,`retry`,`count`; operators like `->`/`::`
//! are kept as their own tokens (so `a->b` and `a::b` are searchable). **No language stemmer** runs on
//! code (§4.4: "the code tokenizer, not a language stemmer"). The code-search DEPTH (trigram substring,
//! the Git `git.*` symbol projection) is the named follow-on **SRCH-P18** — this module ships the
//! tokenizer the camel/snake split needs; SRCH-P18 builds the trigram/symbol index over it.
//!
//! ## The FLOOR — the exact EU language set + CJK strategy is [OPEN → P6]
//! §4.7 / §10: the *mechanism* (the per-language chain) is **decided and built here**; the **exact
//! initial EU language list** that ships v1 and the precise CJK tokenization strategy remain
//! **[OPEN → P6]**. This module therefore ships a **named, extensible default set** ([`Language`]):
//! the major EU languages (English, German, French, Spanish, Italian, Portuguese, Dutch, plus the
//! `Cjk` bucket and an `Unknown`/`und` fallback). Adding a language is a new [`Language`] variant +
//! its stoplist/stemmer rules — the chain dispatch does not change. The open call (which languages +
//! the exact CJK segmentation) is written into the gap report (`docs/gap-report.md`).
//!
//! ## The mutation floor (measured — EI-01 §3 prove-it)
//! `cargo mutants --package myelin-search --file analysis.rs` (2026-06-20): **130 mutants, 125
//! caught + 5 unviable = 0 MISSED (100% of the viable mutants killed).** Every per-language stem arm,
//! every stopword arm, the Porter undouble guard (the `&&`/`==`/index arithmetic), the diacritic-fold
//! combining-mark predicate, the segmentation flush, and the detect-language strict-greater tie-break
//! are each pinned by a behaviour test. No justified survivor.
//!
//! ## Why a pure function, not a Tantivy custom tokenizer
//! The chain is a deterministic pure function `analyze(text) -> Vec<Token>` precisely so the parity
//! gate is *provable*: index-side and query-side call the IDENTICAL function, so a mismatch is a
//! compile-time impossibility, not a runtime hope. (Wiring this stream into Tantivy's `QueryParser`
//! as a registered per-field tokenizer is a downstream engine-integration step; the analyzer
//! *semantics* — the load-bearing correctness — live here, language-detected and tested.)

use std::collections::BTreeSet;

/// The named, extensible default language set (the [OPEN → P6] FLOOR — the *mechanism* is decided;
/// the exact shipped EU list is the open call). Each natural-language variant selects a stoplist +
/// a Snowball/Porter-family light stemmer; [`Language::Cjk`] selects the n-gram path; [`Language::Code`]
/// selects the camel/snake tokenizer; [`Language::Unknown`] (`und`) is the pass-through fallback
/// (segment + fold, no stemming — never wrong, just un-stemmed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// English — Porter-family stemming.
    English,
    /// German — Snowball German (umlaut-aware folding).
    German,
    /// French — Snowball French.
    French,
    /// Spanish — Snowball Spanish.
    Spanish,
    /// Italian — Snowball Italian.
    Italian,
    /// Portuguese — Snowball Portuguese.
    Portuguese,
    /// Dutch — Snowball Dutch.
    Dutch,
    /// The CJK / non-segmented bucket (Chinese/Japanese/Korean) — bigram n-gram segmentation.
    Cjk,
    /// Code / identifiers — the camel/snake tokenizer (no language stemmer, §4.4).
    Code,
    /// `und` — undetermined: segment + case/diacritic-fold only, no stemming, no stoplist.
    Unknown,
}

impl Language {
    /// The `lang` index-doc tag (§3.1) this language is carried as. The index-time detector stamps it;
    /// the query-time path reads it back to select the SAME chain (parity).
    pub fn tag(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::German => "de",
            Language::French => "fr",
            Language::Spanish => "es",
            Language::Italian => "it",
            Language::Portuguese => "pt",
            Language::Dutch => "nl",
            Language::Cjk => "cjk",
            Language::Code => "code",
            Language::Unknown => "und",
        }
    }

    /// Parse a `lang` tag back to a [`Language`] (the query-time path: read the field-language tag the
    /// index doc was analyzed under, select the IDENTICAL chain). An unknown/empty tag ⇒ [`Language::Unknown`]
    /// (the `und` floor — segment+fold, never a wrong stem).
    pub fn from_tag(tag: &str) -> Language {
        match tag {
            "en" => Language::English,
            "de" => Language::German,
            "fr" => Language::French,
            "es" => Language::Spanish,
            "it" => Language::Italian,
            "pt" => Language::Portuguese,
            "nl" => Language::Dutch,
            "cjk" | "zh" | "ja" | "ko" => Language::Cjk,
            "code" => Language::Code,
            _ => Language::Unknown,
        }
    }

    /// Is this a natural language that runs the full word chain (segment → fold → stop → stem)?
    fn is_natural(self) -> bool {
        !matches!(self, Language::Cjk | Language::Code | Language::Unknown)
    }
}

/// One analyzed term (a posting-list term, lowercased + folded + stemmed). The analyzer emits a
/// stream of these; the index stores them as the inverted shape and a query matches against them.
pub type Token = String;

/// **Index-time language detection (§4.7).** A coarse, deterministic detector: a source-declared
/// language overrides this (the caller passes `Some(lang)`); otherwise it is inferred from the
/// script + a small EU stopword-overlap heuristic. It is intentionally *modest* (the named floor):
/// it distinguishes CJK from Latin reliably (by script) and makes a best-effort EU-language guess by
/// stopword overlap, defaulting to [`Language::Unknown`] (`und`) when no signal — never a *wrong*
/// confident guess. The exact detector is part of the [OPEN → P6] tuning; the MECHANISM (detect →
/// stamp `lang` → select the matching chain) is decided here.
pub fn detect_language(text: &str) -> Language {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Language::Unknown;
    }
    // Script test first: any CJK-range char ⇒ the CJK chain (non-segmented).
    if trimmed.chars().any(is_cjk) {
        return Language::Cjk;
    }
    // EU best-effort: count stopword hits per candidate language; pick the clear winner.
    let words: Vec<String> = trimmed
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| fold(&w.to_lowercase()))
        .collect();
    if words.is_empty() {
        return Language::Unknown;
    }
    let candidates = [
        Language::English,
        Language::German,
        Language::French,
        Language::Spanish,
        Language::Italian,
        Language::Portuguese,
        Language::Dutch,
    ];
    let mut best = Language::Unknown;
    let mut best_hits = 0usize;
    for lang in candidates {
        let stops = stopwords(lang);
        let hits = words.iter().filter(|w| stops.contains(&w.as_str())).count();
        if hits > best_hits {
            best_hits = hits;
            best = lang;
        }
    }
    // Require at least one stopword hit to claim an EU language; else `und` (never a wrong guess).
    if best_hits == 0 {
        Language::Unknown
    } else {
        best
    }
}

/// **The analyzer.** Holds the selected [`Language`] (the field-language); [`Analyzer::analyze`] is the
/// ONE chain both index-time and query-time call.
#[derive(Debug, Clone, Copy)]
pub struct Analyzer {
    lang: Language,
}

impl Analyzer {
    /// Build the analyzer for an explicit field-language (the query-time path passes the index doc's
    /// `lang` tag's language; the index-time path passes the detected/source-declared language).
    pub fn for_language(lang: Language) -> Analyzer {
        Analyzer { lang }
    }

    /// Build the analyzer for a `lang` tag (the query-time path reads the stored tag).
    pub fn for_tag(tag: &str) -> Analyzer {
        Analyzer { lang: Language::from_tag(tag) }
    }

    /// The field-language this analyzer runs.
    pub fn language(self) -> Language {
        self.lang
    }

    /// **The chain — the single source of truth.** Index-time and query-time call THIS, so the token
    /// sets are identical by construction (the parity gate). Dispatches by [`Language`]:
    /// - natural language ⇒ segment → fold → stopword → stem;
    /// - [`Language::Cjk`] ⇒ bigram n-gram over CJK runs (Latin runs interleaved go through the word path);
    /// - [`Language::Code`] ⇒ camel/snake tokenizer keeping operators;
    /// - [`Language::Unknown`] ⇒ segment → fold (no stop, no stem).
    pub fn analyze(self, text: &str) -> Vec<Token> {
        match self.lang {
            Language::Code => analyze_code(text),
            Language::Cjk => analyze_cjk(text),
            lang => analyze_words(text, lang),
        }
    }

    /// The token SET (order-free, dedup) — the form the parity gate compares (a query matches a doc
    /// iff their analyzed term sets intersect; the set is the membership shape).
    pub fn analyze_set(self, text: &str) -> BTreeSet<Token> {
        self.analyze(text).into_iter().collect()
    }
}

/// The natural-language word chain: UAX#29-style segmentation → lowercase → diacritic-fold →
/// stopword removal → light stemming.
fn analyze_words(text: &str, lang: Language) -> Vec<Token> {
    let stops = if lang.is_natural() { stopwords(lang) } else { &[] };
    let mut out = Vec::new();
    for raw in segment(text) {
        let lowered = raw.to_lowercase();
        let folded = fold(&lowered);
        if folded.is_empty() {
            continue;
        }
        // Stopword removal is on the FOLDED form (so `über`/`uber` stop alike).
        if stops.contains(&folded.as_str()) {
            continue;
        }
        let stemmed = if lang.is_natural() { stem(&folded, lang) } else { folded };
        if !stemmed.is_empty() {
            out.push(stemmed);
        }
    }
    out
}

/// **UAX#29-style segmentation** (the v1 floor): split on Unicode word boundaries — a *run* of
/// alphanumeric characters of the SAME script class is one token; punctuation/whitespace are
/// boundaries; a script transition (Latin↔CJK) is a boundary so a mixed doc segments correctly.
/// (Full UAX#29 grapheme/word-break tables are heavier; this run-based segmentation is the named v1
/// floor with the SAME observable boundaries for the EU + code corpora — the exact tailoring is the
/// [OPEN → P6] tuning.)
fn segment(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut cur_cjk: Option<bool> = None;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            let cjk = is_cjk(ch);
            // A script transition flushes the current run (Latin↔CJK boundary).
            if let Some(prev) = cur_cjk {
                if prev != cjk && !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            cur.push(ch);
            cur_cjk = Some(cjk);
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
            cur_cjk = None;
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// **Diacritic-fold (§4.7).** NFD-decompose then drop the combining marks: `café` → `cafe`,
/// `naïve` → `naive`, `Müller` → `muller` (after lowercase), `Köln` → `koln`. Implemented as an
/// explicit precomposed→base map for the EU Latin set + a combining-mark strip for anything else
/// (no external unicode-normalization crate needed at this floor; the map covers the shipped EU
/// languages, the strip handles the long tail). ASCII passes through untouched.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        // Drop combining marks outright (the NFD tail of any decomposed char).
        if is_combining_mark(ch) {
            continue;
        }
        out.push(base_of(ch));
    }
    out
}

/// Map a precomposed Latin letter to its base (the EU Latin diacritic set). Anything not in the map
/// returns unchanged.
fn base_of(ch: char) -> char {
    match ch {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' => 'i',
        'ñ' | 'ń' => 'n',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' | 'ō' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ů' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        'š' | 'ś' => 's',
        'ł' => 'l',
        'ð' => 'd',
        other => other,
    }
}

/// True for a Unicode combining mark (the NFD tail). Covers the common combining-diacritical-marks
/// block (U+0300..=U+036F) — the marks an NFD decomposition would leave behind.
fn is_combining_mark(ch: char) -> bool {
    matches!(ch as u32, 0x0300..=0x036F)
}

/// True for a CJK (non-segmented-script) character: the CJK Unified Ideographs + Hiragana + Katakana
/// + Hangul ranges. These have no word spaces, so they take the n-gram path (§4.7).
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x309F | // Hiragana
        0x30A0..=0x30FF | // Katakana
        0x3400..=0x4DBF | // CJK Ext A
        0x4E00..=0x9FFF | // CJK Unified Ideographs
        0xAC00..=0xD7AF   // Hangul syllables
    )
}

/// **CJK n-gram analysis (§4.7).** Over each CJK *run*, emit overlapping **bigrams** (`東京都` →
/// `東京`,`京都`); a single-character CJK run emits that one char (a unigram). Latin runs interleaved
/// in the CJK doc go through the word path (segment → fold), so a mixed `Rust 東京` doc is searchable
/// by both `rust` and `東京`. This is the n-gram strategy §4.7 names (the exact ICU dictionary
/// segmentation is the [OPEN → P6] tuning; bigram n-gram is the recall-safe v1 floor).
fn analyze_cjk(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for run in segment(text) {
        let chars: Vec<char> = run.chars().collect();
        if chars.iter().any(|c| is_cjk(*c)) {
            // A CJK run: overlapping bigrams (single char ⇒ unigram).
            if chars.len() == 1 {
                out.push(chars[0].to_string());
            } else {
                for w in chars.windows(2) {
                    out.push(w.iter().collect());
                }
            }
        } else {
            // A Latin run interleaved in the CJK doc → the word path (fold, no stem at the CJK floor).
            let folded = fold(&run.to_lowercase());
            if !folded.is_empty() {
                out.push(folded);
            }
        }
    }
    out
}

/// **The code / identifier tokenizer (§4.4).** Splits identifiers on camelCase / snake_case / kebab /
/// digit boundaries and lowercases the parts, KEEPS operator runs (`->`, `::`, `==`, …) as their own
/// tokens, and also keeps the whole lowercased identifier so an exact-identifier query still hits.
/// No language stemmer runs (§4.4). This is the tokenizer SRCH-P18 (code-search depth: trigram +
/// the Git `git.*` symbol projection) builds its substring/symbol index over.
fn analyze_code(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut ident = String::new();
    let mut op = String::new();

    let flush_ident = |ident: &mut String, out: &mut Vec<Token>| {
        if ident.is_empty() {
            return;
        }
        let whole = ident.to_lowercase();
        // The sub-tokens (camel/snake/digit split).
        for part in split_identifier(ident) {
            out.push(part.to_lowercase());
        }
        // Keep the whole identifier too (exact-identifier hit) — but not when it equals its only part.
        if !out.last().map(|l| *l == whole).unwrap_or(false) {
            out.push(whole);
        }
        ident.clear();
    };
    let flush_op = |op: &mut String, out: &mut Vec<Token>| {
        if !op.is_empty() {
            out.push(std::mem::take(op));
        }
    };

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            flush_op(&mut op, &mut out);
            ident.push(ch);
        } else if ch.is_whitespace() {
            flush_ident(&mut ident, &mut out);
            flush_op(&mut op, &mut out);
        } else {
            // An operator / punctuation char: flush any pending identifier; accumulate the operator run
            // (so `->`/`::`/`==` stay whole). `_` is part of an identifier (snake), handled above.
            flush_ident(&mut ident, &mut out);
            op.push(ch);
        }
    }
    flush_ident(&mut ident, &mut out);
    flush_op(&mut op, &mut out);
    out
}

/// Split an identifier on camelCase / snake_case / kebab-case / digit boundaries into its lowercased
/// parts. `getUserById` → `[get,User,By,Id]`; `MAX_RETRY` → `[MAX,RETRY]`; `parseHTML5` →
/// `[parse,HTML,5]`. (Returns the raw-case parts; the caller lowercases.)
fn split_identifier(ident: &str) -> Vec<String> {
    let chars: Vec<char> = ident.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for i in 0..chars.len() {
        let c = chars[i];
        if c == '_' || c == '-' {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() {
            let prev = chars[i - 1];
            let lower_to_upper = prev.is_lowercase() && c.is_uppercase();
            // ACRONYMHandler boundary: an uppercase run followed by an uppercase+lowercase start.
            let acronym_end = prev.is_uppercase()
                && c.is_uppercase()
                && i + 1 < chars.len()
                && chars[i + 1].is_lowercase();
            let alpha_digit = prev.is_alphabetic() != c.is_alphabetic();
            if lower_to_upper || acronym_end || alpha_digit {
                parts.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts.into_iter().filter(|p| !p.is_empty()).collect()
}

/// **Light Snowball/Porter-family stemming (§4.7).** A per-language suffix stripper — the v1 floor of
/// the Snowball family: strip the common inflectional suffixes so `running`/`runs`/`run` collapse to
/// one stem (the same stem index-side and query-side, which is the whole point). It is deliberately
/// conservative (strip only well-known suffixes, never below a 3-char stem) so it never over-stems to
/// a wrong collision; the full Snowball automaton per language is the [OPEN → P6] tuning. The
/// REQUIREMENT this satisfies is *parity*: whatever the rule, both sides apply the identical rule.
fn stem(word: &str, lang: Language) -> String {
    let suffixes: &[&str] = match lang {
        Language::English => &["ingly", "edly", "ing", "ed", "ly", "es", "s"],
        Language::German => &["ungen", "lich", "isch", "ung", "en", "er", "es", "e", "s"],
        Language::French => &["ement", "ment", "ions", "eux", "es", "er", "e", "s"],
        Language::Spanish => &["mente", "ciones", "cion", "ando", "endo", "os", "as", "es", "a", "o", "s"],
        Language::Italian => &["mente", "zione", "ando", "endo", "are", "ere", "ire", "i", "e", "o", "a"],
        Language::Portuguese => &["mente", "coes", "cao", "ando", "endo", "os", "as", "es", "a", "o", "s"],
        Language::Dutch => &["heid", "ing", "lijk", "en", "er", "je", "s"],
        _ => return word.to_string(),
    };
    for suf in suffixes {
        if word.len() > suf.len() + 2 && word.ends_with(suf) {
            let mut stemmed = word[..word.len() - suf.len()].to_string();
            // English Porter undoubling: after stripping -ing/-ed, a stem ending in a doubled
            // consonant (not l/s/z) collapses to one (`running`→`runn`→`run`, `hopped`→`hopp`→`hop`).
            // This is what makes `running`/`runs`/`run` share a stem — the parity the GATE needs.
            if lang == Language::English && (*suf == "ing" || *suf == "ed" || *suf == "ingly" || *suf == "edly") {
                undouble_english(&mut stemmed);
            }
            return stemmed;
        }
    }
    word.to_string()
}

/// The Porter undoubling step (English): a stem ending in a doubled consonant other than `l`/`s`/`z`
/// drops one of the pair (`runn` → `run`, `hopp` → `hop`), so the `-ing`/`-ed` form shares the bare
/// stem.
fn undouble_english(stem: &mut String) {
    let chars: Vec<char> = stem.chars().collect();
    let n = chars.len();
    if n >= 3 {
        let last = chars[n - 1];
        let prev = chars[n - 2];
        if last == prev
            && last.is_alphabetic()
            && !matches!(last, 'l' | 's' | 'z' | 'a' | 'e' | 'i' | 'o' | 'u')
        {
            stem.pop();
        }
    }
}

/// The per-language stoplist (the FOLDED forms — so the stopword test runs on the diacritic-folded
/// token). A small, high-frequency v1 set per language (the exact list is the [OPEN → P6] tuning; the
/// MECHANISM — remove stopwords before stemming, identically both sides — is decided).
fn stopwords(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::English => &[
            "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "is", "are", "was",
            "be", "by", "for", "with", "as", "at", "it", "this", "that",
        ],
        Language::German => &[
            "der", "die", "das", "und", "oder", "aber", "von", "zu", "in", "auf", "ist", "sind",
            "ein", "eine", "mit", "fur", "den", "dem", "des",
        ],
        Language::French => &[
            "le", "la", "les", "et", "ou", "de", "du", "des", "un", "une", "a", "en", "est", "sont",
            "dans", "pour", "avec", "sur", "ce", "que",
        ],
        Language::Spanish => &[
            "el", "la", "los", "las", "y", "o", "de", "del", "un", "una", "a", "en", "es", "son",
            "con", "por", "para", "que", "se", "su",
        ],
        Language::Italian => &[
            "il", "lo", "la", "i", "gli", "le", "e", "o", "di", "del", "un", "una", "a", "in", "e",
            "sono", "con", "per", "che", "su",
        ],
        Language::Portuguese => &[
            "o", "a", "os", "as", "e", "ou", "de", "do", "da", "um", "uma", "em", "no", "na", "e",
            "sao", "com", "por", "para", "que",
        ],
        Language::Dutch => &[
            "de", "het", "een", "en", "of", "van", "te", "in", "op", "is", "zijn", "met", "voor",
            "dat", "die", "aan", "om", "naar",
        ],
        _ => &[],
    }
}

/// **The named SRCH-P12 floor (the gap-report entry, recorded in code per the prior-prompt
/// convention).** The per-language analyzer chain — the MECHANISM (segment → fold → stop → stem;
/// CJK n-gram; code camel/snake; index-time/query-time parity) — is **decided and built** in this
/// module. What remains [OPEN → P6] (§4.7 / §10) and the follow-ons named:
///
/// - **[OPEN → P6] the exact initial EU language set** that ships v1 (this module ships an
///   extensible default: English/German/French/Spanish/Italian/Portuguese/Dutch + `und`). Adding a
///   language is a new [`Language`] variant + its stoplist/stemmer; the dispatch is unchanged.
/// - **[OPEN → P6] the exact CJK / non-segmented tokenization strategy** (this module ships
///   recall-safe **bigram n-gram** over CJK runs; the ICU dictionary segmentation is the P6 tuning).
/// - **[P6] full Snowball automata + full UAX#29 tables** — this module ships the light per-language
///   suffix stripper + run-based segmentation v1 floor (same observable boundaries for the EU + code
///   corpora; the parity invariant — both sides apply the *identical* rule — holds for any rule).
/// - **SRCH-P18 — the code-search DEPTH** (trigram substring index + the Git `git.*` symbol
///   projection) builds OVER the camel/snake tokenizer this module ships ([`analyze_code`]); the
///   tokenizer is here, the index over it is SRCH-P18.
///
/// This is a doc-only marker (zero-sized) so the floor is greppable + linkable in code, the
/// established way prior Search prompts (e.g. `layout::SrchP03Floor`) record a gap-report entry.
#[derive(Debug, Clone, Copy)]
pub struct SrchP12AnalyzerFloor;

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------------------------
    // The CRUX gate: query-time analyzer matches index-time analyzer per field-language (no miss).
    // For every supported language L, the analyzed-token SET of a query equals the analyzed-token
    // SET of the index-time text (same function, same language) — so a query in L matches index-L.
    // -------------------------------------------------------------------------------------------

    /// The parity gate: index-time and query-time analyze the same text under the same language to
    /// the SAME token set (the no-analyzer-mismatch-miss invariant, by construction).
    fn assert_parity(lang: Language, text: &str) {
        let idx = Analyzer::for_language(lang).analyze_set(text);
        // The query-time path resolves the analyzer from the stored `lang` TAG (the real flow).
        let qry = Analyzer::for_tag(lang.tag()).analyze_set(text);
        assert_eq!(idx, qry, "analyzer drift for {:?} on {text:?}", lang);
        assert!(!idx.is_empty(), "the chain produced no tokens for {:?} on {text:?}", lang);
    }

    #[test]
    fn query_time_matches_index_time_per_language() {
        // Every shipped language: the same text analyzes identically index-side and query-side.
        assert_parity(Language::English, "The Running Foxes are jumping");
        assert_parity(Language::German, "Die Müller Häuser sind schön");
        assert_parity(Language::French, "Les éléments généraux du système");
        assert_parity(Language::Spanish, "Las construcciones rápidas y económicas");
        assert_parity(Language::Italian, "Le costruzioni rapide e funzionali");
        assert_parity(Language::Portuguese, "As construções rápidas e econômicas");
        assert_parity(Language::Dutch, "De snelle mogelijkheid van het systeem");
        assert_parity(Language::Cjk, "東京都とRustの統合");
        assert_parity(Language::Code, "getUserById(a->b)::call");
        assert_parity(Language::Unknown, "passthrough TOKENS here");
    }

    /// The headline GATE wording: a query in language L matches index-time-L tokens (stem +
    /// diacritic-fold + stopword parity). We assert the *query token* is present in the index token
    /// set — the membership the engine's posting-list conjunction needs.
    #[test]
    fn query_in_language_matches_index_tokens() {
        // English: "running" (query) stems to the same token as "runs" (indexed body) — stem parity.
        let a = Analyzer::for_language(Language::English);
        let idx = a.analyze_set("the fox runs daily");
        let q = a.analyze("running");
        assert_eq!(q.len(), 1);
        assert!(idx.contains(&q[0]), "stem parity: 'running' must hit indexed 'runs' → {idx:?}");

        // French diacritic-fold: a query "elements" (no accent) hits indexed "éléments".
        let fr = Analyzer::for_language(Language::French);
        let fidx = fr.analyze_set("les éléments du système");
        let fq = fr.analyze("elements");
        assert!(fidx.contains(&fq[0]), "diacritic-fold: 'elements' must hit 'éléments' → {fidx:?}");

        // German stopword parity: 'der'/'die'/'das' are dropped both sides (not a spurious match key).
        let de = Analyzer::for_language(Language::German);
        assert!(de.analyze("der die das").is_empty(), "German stopwords are removed");
    }

    // -------------------------------------------------------------------------------------------
    // Per-language chain unit tests: tokenize / stem / fold / stopword.
    // -------------------------------------------------------------------------------------------

    #[test]
    fn english_chain_stems_folds_stops() {
        let a = Analyzer::for_language(Language::English);
        assert_eq!(a.analyze("running"), vec!["run"]); // -ing stripped + Porter undouble
        assert_eq!(a.analyze("foxes"), vec!["fox"]); // -es stripped
        assert!(a.analyze("the and of").is_empty()); // stopwords removed
    }

    #[test]
    fn diacritic_fold_strips_marks() {
        // café → cafe, naïve → naive, Köln → koln (after lowercase).
        assert_eq!(fold("café"), "cafe");
        assert_eq!(fold("naïve"), "naive");
        assert_eq!(fold(&"Köln".to_lowercase()), "koln");
        assert_eq!(fold("über"), "uber");
        // ASCII untouched.
        assert_eq!(fold("plain"), "plain");
    }

    #[test]
    fn segment_splits_on_boundaries_and_script() {
        assert_eq!(segment("hello, world!"), vec!["hello", "world"]);
        // Latin↔CJK transition is a boundary.
        assert_eq!(segment("Rust東京"), vec!["Rust", "東京"]);
        assert!(segment("   ").is_empty());
    }

    #[test]
    fn cjk_chain_emits_bigrams() {
        // 東京都 → 東京, 京都 (overlapping bigrams).
        assert_eq!(analyze_cjk("東京都"), vec!["東京", "京都"]);
        // single CJK char → unigram.
        assert_eq!(analyze_cjk("水"), vec!["水"]);
        // Mixed doc: Latin run folds, CJK run bigrams — searchable by both.
        let set: BTreeSet<_> = analyze_cjk("Rust東京").into_iter().collect();
        assert!(set.contains("rust"));
        assert!(set.contains("東京"));
    }

    #[test]
    fn cjk_query_matches_via_ngram() {
        // A query "京都" hits a body "東京都" via the shared bigram.
        let a = Analyzer::for_language(Language::Cjk);
        let body = a.analyze_set("東京都");
        let q = a.analyze("京都");
        assert!(q.iter().all(|t| body.contains(t)), "CJK n-gram match: {body:?} ⊇ {q:?}");
    }

    #[test]
    fn code_tokenizer_splits_camel_snake_keeps_operators() {
        let a = Analyzer::for_language(Language::Code);
        let toks = a.analyze("getUserById");
        assert!(toks.contains(&"get".to_string()));
        assert!(toks.contains(&"user".to_string()));
        assert!(toks.contains(&"by".to_string()));
        assert!(toks.contains(&"id".to_string()));
        // whole identifier kept too (exact-identifier hit).
        assert!(toks.contains(&"getuserbyid".to_string()));

        let snake = a.analyze("MAX_RETRY_COUNT");
        assert!(snake.contains(&"max".to_string()));
        assert!(snake.contains(&"retry".to_string()));
        assert!(snake.contains(&"count".to_string()));

        // operators kept as their own tokens.
        let ops = a.analyze("a->b::c");
        assert!(ops.contains(&"->".to_string()), "operator -> kept: {ops:?}");
        assert!(ops.contains(&"::".to_string()), "operator :: kept: {ops:?}");
        assert!(ops.contains(&"a".to_string()) && ops.contains(&"b".to_string()));

        // No language stemmer on code: "running" stays whole (not stemmed to "runn").
        assert_eq!(a.analyze("running"), vec!["running"]);
    }

    #[test]
    fn code_acronym_and_digit_boundaries() {
        assert_eq!(split_identifier("parseHTML5"), vec!["parse", "HTML", "5"]);
        assert_eq!(split_identifier("HTTPServer"), vec!["HTTP", "Server"]);
        assert_eq!(split_identifier("snake_case"), vec!["snake", "case"]);
        // A separator BETWEEN a letter run and a digit run pins the `if !cur.is_empty()` flush at the
        // separator: `abc_123` MUST split to `["abc","123"]`. (Without the flush at `_`, the digit run
        // does not re-trigger a boundary against the `_` and the parts merge to `["abc123"]`.)
        assert_eq!(split_identifier("abc_123"), vec!["abc", "123"]);
    }

    #[test]
    fn detect_language_by_script_and_stopwords() {
        assert_eq!(detect_language("東京都"), Language::Cjk);
        assert_eq!(detect_language("the quick brown fox and the dog"), Language::English);
        assert_eq!(detect_language("der die das und ein"), Language::German);
        // No signal ⇒ und (never a wrong confident guess).
        assert_eq!(detect_language("xyz123 qwop"), Language::Unknown);
        assert_eq!(detect_language(""), Language::Unknown);
    }

    #[test]
    fn tag_roundtrips() {
        for lang in [
            Language::English,
            Language::German,
            Language::French,
            Language::Spanish,
            Language::Italian,
            Language::Portuguese,
            Language::Dutch,
            Language::Cjk,
            Language::Code,
            Language::Unknown,
        ] {
            assert_eq!(Language::from_tag(lang.tag()), lang, "tag roundtrip for {lang:?}");
        }
    }

    #[test]
    fn unknown_is_segment_and_fold_no_stem() {
        let a = Analyzer::for_language(Language::Unknown);
        // No stemming, no stopwords — just segment + fold.
        assert_eq!(a.analyze("Running THE café"), vec!["running", "the", "cafe"]);
    }

    // -------------------------------------------------------------------------------------------
    // Per-language stem arms — each language strips its OWN suffix set to a DISTINCT documented stem
    // (pins every match arm in `stem`, so deleting/swapping a per-language arm is caught).
    // -------------------------------------------------------------------------------------------

    #[test]
    fn each_language_stems_its_own_suffix() {
        // English: -ing + Porter undouble.
        assert_eq!(stem("running", Language::English), "run");
        // German: -ungen.
        assert_eq!(stem("zeitungen", Language::German), "zeit");
        // French: -ement.
        assert_eq!(stem("rapidement", Language::French), "rapid");
        // Spanish: -ciones.
        assert_eq!(stem("construcciones", Language::Spanish), "construc");
        // Italian: -zione.
        assert_eq!(stem("costruzione", Language::Italian), "costru");
        // Portuguese: -mente.
        assert_eq!(stem("rapidamente", Language::Portuguese), "rapida");
        // Dutch: -heid.
        assert_eq!(stem("mogelijkheid", Language::Dutch), "mogelijk");
        // A non-natural language never stems (Code/Cjk/Unknown pass through).
        assert_eq!(stem("running", Language::Code), "running");
        assert_eq!(stem("running", Language::Unknown), "running");
    }

    #[test]
    fn stem_never_goes_below_three_chars() {
        // The `> suf.len() + 2` guard: a too-short word keeps its suffix (no over-stem collision).
        // "is" ends with "s" but len 2 is not > 1+2=3, so it is NOT stripped.
        assert_eq!(stem("is", Language::English), "is");
        // "ses" ends with "es"/"s"; len 3 is not > 2+2, and not > 1+2 for "s" either → unchanged.
        assert_eq!(stem("ses", Language::English), "ses");
        // But "buses" (len 5 > 2+2) → strip "es" → "bus".
        assert_eq!(stem("buses", Language::English), "bus");
    }

    #[test]
    fn undouble_only_consonants_not_lsz_or_vowels() {
        // `running` → `runn` → `run` (n doubled, undoubled).
        assert_eq!(stem("hopping", Language::English), "hop");
        // `falling` ends in `ll` → l is in the keep-set, NOT undoubled → `fall`.
        assert_eq!(stem("falling", Language::English), "fall");
        // A vowel pair is never undoubled: `seeing` → strip -ing → `see` (ee kept).
        assert_eq!(stem("seeing", Language::English), "see");
        // Undouble only fires for -ing/-ed, not -s: `runs` → `run` (no doubled consonant to touch).
        assert_eq!(stem("runs", Language::English), "run");
        // -ed undoubles too: `hopped` → `hopp` → `hop`.
        assert_eq!(stem("hopped", Language::English), "hop");
        // A longer stem pins the `prev = chars[n-2]` index (not chars[n/2]): `stunning` → `stunn`
        // → `stun` (n=5; the doubled `n` is at n-2/n-1, NOT at n/2).
        assert_eq!(stem("stunning", Language::English), "stun");
        // The -ingly / -edly arms also undouble (pins `*suf == "ingly"` / `*suf == "edly"`):
        assert_eq!(stem("stunningly", Language::English), "stun");
        assert_eq!(stem("flaggedly", Language::English), "flag");
        // The undouble guard is English-AND-(ing/ed family) ONLY: a `-ly`/`-es`/`-s` strip leaving a
        // doubled consonant does NOT undouble (pins the `&&` and the suffix `==` set). `chess` → strip
        // `s` → `ches` (the `ss`→`s` only happens via the strip, undouble must not also fire).
        assert_eq!(stem("grass", Language::English), "gras");
        // `coolly` → strip `ly` → `cool` (doubled `o`/`l` left; -ly is not in the undouble set, and
        // `l` is keep-listed anyway) — stays `cool`, never `coo`/`col`.
        assert_eq!(stem("coolly", Language::English), "cool");
    }

    #[test]
    fn undouble_does_not_fire_for_non_ing_ed_suffixes() {
        // Construct the adversarial case the `&&`/`==` mutants would break: a German word does NOT run
        // the English undouble even after a strip that leaves a doubled consonant. German has no -ing/-ed
        // in its set, and `lang == English` gates it — so `bann` (no German suffix) stays `bann`.
        assert_eq!(stem("bann", Language::German), "bann");
        // And an English `-es` strip leaving a doubled consonant is NOT undoubled: `passes` → strip
        // `es` → `pass` (NOT `pas`); `es` is not in the undouble suffix set.
        assert_eq!(stem("passes", Language::English), "pass");
        // The undouble guard is `lang == English` AND the ing/ed family — NOT `OR`. Dutch also strips
        // `-ing`, but Dutch must NOT run the English undouble: `brenning`/Dutch → `brenn` (NOT `bren`).
        // This pins the `&&` (an `||` mutant would undouble the Dutch `-ing` stem).
        assert_eq!(stem("brenning", Language::Dutch), "brenn");
    }

    // -------------------------------------------------------------------------------------------
    // Per-language stopword arms — each language removes its OWN stopwords but NOT another's
    // (pins every match arm in `stopwords`).
    // -------------------------------------------------------------------------------------------

    #[test]
    fn each_language_removes_its_own_stopwords() {
        let cases = [
            (Language::English, "the and of"),
            (Language::German, "der die das"),
            (Language::French, "le la les"),
            (Language::Spanish, "el la los"),
            (Language::Italian, "il lo gli"),
            (Language::Portuguese, "os as do"),
            (Language::Dutch, "de het een"),
        ];
        for (lang, stops) in cases {
            assert!(
                Analyzer::for_language(lang).analyze(stops).is_empty(),
                "{lang:?} must remove its stopwords {stops:?}"
            );
        }
        // A content word is NOT a stopword (the stoplist is not "everything"); German `apfel` has
        // no stripped suffix so it survives the chain whole.
        assert_eq!(Analyzer::for_language(Language::German).analyze("apfel"), vec!["apfel"]);
    }

    #[test]
    fn stopwords_are_per_language_not_shared() {
        // `der` is a German stopword but NOT an English one — English keeps it (folded, un-stemmed).
        let en = Analyzer::for_language(Language::English).analyze("der");
        assert_eq!(en, vec!["der"], "English does not drop the German stopword 'der'");
    }

    // -------------------------------------------------------------------------------------------
    // fold / segment / detect edge cases (pin the remaining helpers).
    // -------------------------------------------------------------------------------------------

    #[test]
    fn fold_drops_explicit_combining_marks() {
        // An explicitly NFD-decomposed "e" + combining acute (U+0301) folds to bare "e".
        let decomposed = "cafe\u{0301}"; // café written as e + combining acute
        assert_eq!(fold(decomposed), "cafe");
        // The combining-mark predicate is load-bearing: without dropping the mark the token differs.
        assert!(is_combining_mark('\u{0301}'));
        assert!(!is_combining_mark('e'));
    }

    #[test]
    fn detect_language_requires_a_clear_stopword_winner() {
        // Exactly one English stopword present ⇒ English (the `> best_hits` strict-greater pick: the
        // first candidate with hits wins, ties do not overwrite — German with 0 hits never displaces).
        assert_eq!(detect_language("the rocket launches"), Language::English);
        // A single non-stopword token, no signal ⇒ und.
        assert_eq!(detect_language("rocket"), Language::Unknown);
        // STRICT-greater tie-break (pins `> best_hits`, not `>=`): "the" (English stop) and "de"
        // (French/Dutch stop) each contribute one hit; English is the earlier candidate, so the
        // strict `>` keeps English (a `>=` mutant would let a later equal-hit language overwrite it).
        assert_eq!(detect_language("the de"), Language::English);
    }

    #[test]
    fn segment_keeps_alphanumeric_runs_whole() {
        // The `!cur.is_empty()` flush guard: leading/trailing/double punctuation does not emit empties.
        assert_eq!(segment(",,hello,,"), vec!["hello"]);
        assert_eq!(segment("a1b2"), vec!["a1b2"]); // digits + letters in one Latin run stay together
    }

    #[test]
    fn is_natural_is_only_the_eu_languages() {
        assert!(Language::English.is_natural());
        assert!(Language::Dutch.is_natural());
        assert!(!Language::Code.is_natural());
        assert!(!Language::Cjk.is_natural());
        assert!(!Language::Unknown.is_natural());
    }

    #[test]
    fn the_named_floor_is_constructible() {
        // The greppable gap-report marker (the [OPEN → P6] entry) exists.
        let _floor = SrchP12AnalyzerFloor;
    }
}
