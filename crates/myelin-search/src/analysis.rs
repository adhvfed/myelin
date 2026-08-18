use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    English,
    German,
    French,
    Spanish,
    Italian,
    Portuguese,
    Dutch,
    Cjk,
    Code,
    Unknown,
}

impl Language {
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

    fn is_natural(self) -> bool {
        !matches!(self, Language::Cjk | Language::Code | Language::Unknown)
    }
}

pub type Token = String;

pub fn detect_language(text: &str) -> Language {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Language::Unknown;
    }
    if trimmed.chars().any(is_cjk) {
        return Language::Cjk;
    }
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
    if best_hits == 0 {
        Language::Unknown
    } else {
        best
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Analyzer {
    lang: Language,
}

impl Analyzer {
    pub fn for_language(lang: Language) -> Analyzer {
        Analyzer { lang }
    }

    pub fn for_tag(tag: &str) -> Analyzer {
        Analyzer {
            lang: Language::from_tag(tag),
        }
    }

    pub fn language(self) -> Language {
        self.lang
    }

    pub fn analyze(self, text: &str) -> Vec<Token> {
        match self.lang {
            Language::Code => analyze_code(text),
            Language::Cjk => analyze_cjk(text),
            lang => analyze_words(text, lang),
        }
    }

    pub fn analyze_set(self, text: &str) -> BTreeSet<Token> {
        self.analyze(text).into_iter().collect()
    }
}

fn analyze_words(text: &str, lang: Language) -> Vec<Token> {
    let stops = if lang.is_natural() {
        stopwords(lang)
    } else {
        &[]
    };
    let mut out = Vec::new();
    for raw in segment(text) {
        let lowered = raw.to_lowercase();
        let folded = fold(&lowered);
        if folded.is_empty() {
            continue;
        }
        if stops.contains(&folded.as_str()) {
            continue;
        }
        let stemmed = if lang.is_natural() {
            stem(&folded, lang)
        } else {
            folded
        };
        if !stemmed.is_empty() {
            out.push(stemmed);
        }
    }
    out
}

fn segment(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut cur_cjk: Option<bool> = None;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            let cjk = is_cjk(ch);
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

fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if is_combining_mark(ch) {
            continue;
        }
        out.push(base_of(ch));
    }
    out
}

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

fn is_combining_mark(ch: char) -> bool {
    matches!(ch as u32, 0x0300..=0x036F)
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x309F |
        0x30A0..=0x30FF |
        0x3400..=0x4DBF |
        0x4E00..=0x9FFF |
        0xAC00..=0xD7AF
    )
}

fn analyze_cjk(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for run in segment(text) {
        let chars: Vec<char> = run.chars().collect();
        if chars.iter().any(|c| is_cjk(*c)) {
            if chars.len() == 1 {
                out.push(chars[0].to_string());
            } else {
                for w in chars.windows(2) {
                    out.push(w.iter().collect());
                }
            }
        } else {
            let folded = fold(&run.to_lowercase());
            if !folded.is_empty() {
                out.push(folded);
            }
        }
    }
    out
}

fn analyze_code(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut ident = String::new();
    let mut op = String::new();

    let flush_ident = |ident: &mut String, out: &mut Vec<Token>| {
        if ident.is_empty() {
            return;
        }
        let whole = ident.to_lowercase();
        for part in split_identifier(ident) {
            out.push(part.to_lowercase());
        }
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
            flush_ident(&mut ident, &mut out);
            op.push(ch);
        }
    }
    flush_ident(&mut ident, &mut out);
    flush_op(&mut op, &mut out);
    out
}

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

fn stem(word: &str, lang: Language) -> String {
    let suffixes: &[&str] = match lang {
        Language::English => &["ingly", "edly", "ing", "ed", "ly", "es", "s"],
        Language::German => &["ungen", "lich", "isch", "ung", "en", "er", "es", "e", "s"],
        Language::French => &["ement", "ment", "ions", "eux", "es", "er", "e", "s"],
        Language::Spanish => &[
            "mente", "ciones", "cion", "ando", "endo", "os", "as", "es", "a", "o", "s",
        ],
        Language::Italian => &[
            "mente", "zione", "ando", "endo", "are", "ere", "ire", "i", "e", "o", "a",
        ],
        Language::Portuguese => &[
            "mente", "coes", "cao", "ando", "endo", "os", "as", "es", "a", "o", "s",
        ],
        Language::Dutch => &["heid", "ing", "lijk", "en", "er", "je", "s"],
        _ => return word.to_string(),
    };
    for suf in suffixes {
        if word.len() > suf.len() + 2 && word.ends_with(suf) {
            let mut stemmed = word[..word.len() - suf.len()].to_string();
            if lang == Language::English
                && (*suf == "ing" || *suf == "ed" || *suf == "ingly" || *suf == "edly")
            {
                undouble_english(&mut stemmed);
            }
            return stemmed;
        }
    }
    word.to_string()
}

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

fn stopwords(lang: Language) -> &'static [&'static str] {
    match lang {
        Language::English => &[
            "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "is", "are", "was", "be",
            "by", "for", "with", "as", "at", "it", "this", "that",
        ],
        Language::German => &[
            "der", "die", "das", "und", "oder", "aber", "von", "zu", "in", "auf", "ist", "sind",
            "ein", "eine", "mit", "fur", "den", "dem", "des",
        ],
        Language::French => &[
            "le", "la", "les", "et", "ou", "de", "du", "des", "un", "une", "a", "en", "est",
            "sont", "dans", "pour", "avec", "sur", "ce", "que",
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


#[cfg(test)]
mod tests {
    use super::*;

    fn assert_parity(lang: Language, text: &str) {
        let idx = Analyzer::for_language(lang).analyze_set(text);
        let qry = Analyzer::for_tag(lang.tag()).analyze_set(text);
        assert_eq!(idx, qry, "analyzer drift for {:?} on {text:?}", lang);
        assert!(
            !idx.is_empty(),
            "the chain produced no tokens for {:?} on {text:?}",
            lang
        );
    }

    #[test]
    fn query_time_matches_index_time_per_language() {
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

    #[test]
    fn query_in_language_matches_index_tokens() {
        let a = Analyzer::for_language(Language::English);
        let idx = a.analyze_set("the fox runs daily");
        let q = a.analyze("running");
        assert_eq!(q.len(), 1);
        assert!(
            idx.contains(&q[0]),
            "stem parity: 'running' must hit indexed 'runs' → {idx:?}"
        );

        let fr = Analyzer::for_language(Language::French);
        let fidx = fr.analyze_set("les éléments du système");
        let fq = fr.analyze("elements");
        assert!(
            fidx.contains(&fq[0]),
            "diacritic-fold: 'elements' must hit 'éléments' → {fidx:?}"
        );

        let de = Analyzer::for_language(Language::German);
        assert!(
            de.analyze("der die das").is_empty(),
            "German stopwords are removed"
        );
    }

    #[test]
    fn english_chain_stems_folds_stops() {
        let a = Analyzer::for_language(Language::English);
        assert_eq!(a.analyze("running"), vec!["run"]);
        assert_eq!(a.analyze("foxes"), vec!["fox"]);
        assert!(a.analyze("the and of").is_empty());
    }

    #[test]
    fn diacritic_fold_strips_marks() {
        assert_eq!(fold("café"), "cafe");
        assert_eq!(fold("naïve"), "naive");
        assert_eq!(fold(&"Köln".to_lowercase()), "koln");
        assert_eq!(fold("über"), "uber");
        assert_eq!(fold("plain"), "plain");
    }

    #[test]
    fn segment_splits_on_boundaries_and_script() {
        assert_eq!(segment("hello, world!"), vec!["hello", "world"]);
        assert_eq!(segment("Rust東京"), vec!["Rust", "東京"]);
        assert!(segment("   ").is_empty());
    }

    #[test]
    fn cjk_chain_emits_bigrams() {
        assert_eq!(analyze_cjk("東京都"), vec!["東京", "京都"]);
        assert_eq!(analyze_cjk("水"), vec!["水"]);
        let set: BTreeSet<_> = analyze_cjk("Rust東京").into_iter().collect();
        assert!(set.contains("rust"));
        assert!(set.contains("東京"));
    }

    #[test]
    fn cjk_query_matches_via_ngram() {
        let a = Analyzer::for_language(Language::Cjk);
        let body = a.analyze_set("東京都");
        let q = a.analyze("京都");
        assert!(
            q.iter().all(|t| body.contains(t)),
            "CJK n-gram match: {body:?} ⊇ {q:?}"
        );
    }

    #[test]
    fn code_tokenizer_splits_camel_snake_keeps_operators() {
        let a = Analyzer::for_language(Language::Code);
        let toks = a.analyze("getUserById");
        assert!(toks.contains(&"get".to_string()));
        assert!(toks.contains(&"user".to_string()));
        assert!(toks.contains(&"by".to_string()));
        assert!(toks.contains(&"id".to_string()));
        assert!(toks.contains(&"getuserbyid".to_string()));

        let snake = a.analyze("MAX_RETRY_COUNT");
        assert!(snake.contains(&"max".to_string()));
        assert!(snake.contains(&"retry".to_string()));
        assert!(snake.contains(&"count".to_string()));

        let ops = a.analyze("a->b::c");
        assert!(ops.contains(&"->".to_string()), "operator -> kept: {ops:?}");
        assert!(ops.contains(&"::".to_string()), "operator :: kept: {ops:?}");
        assert!(ops.contains(&"a".to_string()) && ops.contains(&"b".to_string()));

        assert_eq!(a.analyze("running"), vec!["running"]);
    }

    #[test]
    fn code_acronym_and_digit_boundaries() {
        assert_eq!(split_identifier("parseHTML5"), vec!["parse", "HTML", "5"]);
        assert_eq!(split_identifier("HTTPServer"), vec!["HTTP", "Server"]);
        assert_eq!(split_identifier("snake_case"), vec!["snake", "case"]);
        assert_eq!(split_identifier("abc_123"), vec!["abc", "123"]);
    }

    #[test]
    fn detect_language_by_script_and_stopwords() {
        assert_eq!(detect_language("東京都"), Language::Cjk);
        assert_eq!(
            detect_language("the quick brown fox and the dog"),
            Language::English
        );
        assert_eq!(detect_language("der die das und ein"), Language::German);
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
            assert_eq!(
                Language::from_tag(lang.tag()),
                lang,
                "tag roundtrip for {lang:?}"
            );
        }
    }

    #[test]
    fn unknown_is_segment_and_fold_no_stem() {
        let a = Analyzer::for_language(Language::Unknown);
        assert_eq!(
            a.analyze("Running THE café"),
            vec!["running", "the", "cafe"]
        );
    }

    #[test]
    fn each_language_stems_its_own_suffix() {
        assert_eq!(stem("running", Language::English), "run");
        assert_eq!(stem("zeitungen", Language::German), "zeit");
        assert_eq!(stem("rapidement", Language::French), "rapid");
        assert_eq!(stem("construcciones", Language::Spanish), "construc");
        assert_eq!(stem("costruzione", Language::Italian), "costru");
        assert_eq!(stem("rapidamente", Language::Portuguese), "rapida");
        assert_eq!(stem("mogelijkheid", Language::Dutch), "mogelijk");
        assert_eq!(stem("running", Language::Code), "running");
        assert_eq!(stem("running", Language::Unknown), "running");
    }

    #[test]
    fn stem_never_goes_below_three_chars() {
        assert_eq!(stem("is", Language::English), "is");
        assert_eq!(stem("ses", Language::English), "ses");
        assert_eq!(stem("buses", Language::English), "bus");
    }

    #[test]
    fn undouble_only_consonants_not_lsz_or_vowels() {
        assert_eq!(stem("hopping", Language::English), "hop");
        assert_eq!(stem("falling", Language::English), "fall");
        assert_eq!(stem("seeing", Language::English), "see");
        assert_eq!(stem("runs", Language::English), "run");
        assert_eq!(stem("hopped", Language::English), "hop");
        assert_eq!(stem("stunning", Language::English), "stun");
        assert_eq!(stem("stunningly", Language::English), "stun");
        assert_eq!(stem("flaggedly", Language::English), "flag");
        assert_eq!(stem("grass", Language::English), "gras");
        assert_eq!(stem("coolly", Language::English), "cool");
    }

    #[test]
    fn undouble_does_not_fire_for_non_ing_ed_suffixes() {
        assert_eq!(stem("bann", Language::German), "bann");
        assert_eq!(stem("passes", Language::English), "pass");
        assert_eq!(stem("brenning", Language::Dutch), "brenn");
    }

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
        assert_eq!(
            Analyzer::for_language(Language::German).analyze("apfel"),
            vec!["apfel"]
        );
    }

    #[test]
    fn stopwords_are_per_language_not_shared() {
        let en = Analyzer::for_language(Language::English).analyze("der");
        assert_eq!(
            en,
            vec!["der"],
            "English does not drop the German stopword 'der'"
        );
    }

    #[test]
    fn fold_drops_explicit_combining_marks() {
        let decomposed = "cafe\u{0301}";
        assert_eq!(fold(decomposed), "cafe");
        assert!(is_combining_mark('\u{0301}'));
        assert!(!is_combining_mark('e'));
    }

    #[test]
    fn detect_language_requires_a_clear_stopword_winner() {
        assert_eq!(detect_language("the rocket launches"), Language::English);
        assert_eq!(detect_language("rocket"), Language::Unknown);
        assert_eq!(detect_language("the de"), Language::English);
    }

    #[test]
    fn segment_keeps_alphanumeric_runs_whole() {
        assert_eq!(segment(",,hello,,"), vec!["hello"]);
        assert_eq!(segment("a1b2"), vec!["a1b2"]);
    }

    #[test]
    fn is_natural_is_only_the_eu_languages() {
        assert!(Language::English.is_natural());
        assert!(Language::Dutch.is_natural());
        assert!(!Language::Code.is_natural());
        assert!(!Language::Cjk.is_natural());
        assert!(!Language::Unknown.is_natural());
    }

}
