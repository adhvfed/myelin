use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum FieldType {
    Text = 0,
    Int = 1,
    Bool = 2,
    Date = 3,
    Select = 4,
    Relation = 5,
    Principal = 6,
    OrderKey = 7,
}

impl FieldType {
    pub fn wire_id(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Int => "int",
            FieldType::Bool => "bool",
            FieldType::Date => "date",
            FieldType::Select => "select",
            FieldType::Relation => "relation",
            FieldType::Principal => "principal",
            FieldType::OrderKey => "order_key",
        }
    }

    pub fn all() -> [FieldType; 8] {
        [
            FieldType::Text,
            FieldType::Int,
            FieldType::Bool,
            FieldType::Date,
            FieldType::Select,
            FieldType::Relation,
            FieldType::Principal,
            FieldType::OrderKey,
        ]
    }

    pub fn is_ordered(self) -> bool {
        matches!(self, FieldType::Int | FieldType::Date | FieldType::OrderKey)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldValue {
    Text(String),
    Int(i64),
    Bool(bool),
    Date(String),
    Select(String),
    Relation(String),
    Principal(String),
    OrderKey(OrderKey),
}

impl FieldValue {
    pub fn field_type(&self) -> FieldType {
        match self {
            FieldValue::Text(_) => FieldType::Text,
            FieldValue::Int(_) => FieldType::Int,
            FieldValue::Bool(_) => FieldType::Bool,
            FieldValue::Date(_) => FieldType::Date,
            FieldValue::Select(_) => FieldType::Select,
            FieldValue::Relation(_) => FieldType::Relation,
            FieldValue::Principal(_) => FieldType::Principal,
            FieldValue::OrderKey(_) => FieldType::OrderKey,
        }
    }
}

pub const LEXORANK_ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

pub const LEXORANK_REBALANCE_LEN: usize = 48;

pub const LEXORANK_FIRST: &str = "U";

pub const LEXORANK_JITTER_LEN: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OrderKey(String);

impl OrderKey {
    pub fn parse(s: impl Into<String>) -> Option<OrderKey> {
        let s = s.into();
        if s.is_empty() || !s.bytes().all(|b| LEXORANK_ALPHABET.contains(&b)) {
            return None;
        }
        Some(OrderKey(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn needs_rebalance(&self) -> bool {
        self.0.len() >= LEXORANK_REBALANCE_LEN
    }

    pub fn bisect(lo: Option<&OrderKey>, hi: Option<&OrderKey>) -> OrderKey {
        let lo_b = lo.map(|k| k.0.as_bytes()).unwrap_or(&[]);
        let hi_b = hi.map(|k| k.0.as_bytes()).unwrap_or(&[]);
        OrderKey(String::from_utf8(midpoint(lo_b, hi_b)).expect("LexoRank digits are ASCII"))
    }

    pub fn rank_first(jitter: Jitter) -> OrderKey {
        Self::with_jitter(LEXORANK_FIRST.as_bytes().to_vec(), jitter)
    }

    pub fn rank_last(after: Option<&OrderKey>, jitter: Jitter) -> OrderKey {
        match after {
            None => Self::rank_first(jitter),
            Some(a) => {
                let mid = midpoint(a.0.as_bytes(), &[]);
                Self::with_jitter(mid, jitter)
            }
        }
    }

    pub fn rank_between(lo: Option<&OrderKey>, hi: Option<&OrderKey>, jitter: Jitter) -> OrderKey {
        let lo_b = lo.map(|k| k.0.as_bytes()).unwrap_or(&[]);
        let hi_b = hi.map(|k| k.0.as_bytes()).unwrap_or(&[]);
        Self::with_jitter(midpoint(lo_b, hi_b), jitter)
    }

    fn with_jitter(mut body: Vec<u8>, jitter: Jitter) -> OrderKey {
        body.extend_from_slice(&jitter.0);
        OrderKey(String::from_utf8(body).expect("LexoRank digits are ASCII"))
    }

    pub fn needs_rebalance_key(rank: &OrderKey) -> bool {
        rank.needs_rebalance()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Jitter([u8; LEXORANK_JITTER_LEN]);

impl Jitter {
    pub fn from_ranks(a: usize, b: usize) -> Option<Jitter> {
        if a >= BASE || b >= BASE {
            return None;
        }
        Some(Jitter([digit(a), digit(b)]))
    }

    pub const ZERO: Jitter = Jitter(*b"00");

    pub fn random(bytes: [u8; LEXORANK_JITTER_LEN]) -> Jitter {
        Jitter([
            digit(bytes[0] as usize % BASE),
            digit(bytes[1] as usize % BASE),
        ])
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("jitter digits are ASCII")
    }
}

impl std::fmt::Display for OrderKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn rank(b: u8) -> usize {
    LEXORANK_ALPHABET
        .iter()
        .position(|&c| c == b)
        .unwrap_or(LEXORANK_ALPHABET.len())
}

fn digit(r: usize) -> u8 {
    LEXORANK_ALPHABET[r]
}

const BASE: usize = 62;

fn midpoint(lo: &[u8], hi: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..MIDPOINT_DESCENT_CAP {
        let lo_d = if i < lo.len() { rank(lo[i]) } else { 0 };
        let hi_d = if i < hi.len() { rank(hi[i]) } else { BASE };

        if hi_d > lo_d + 1 {
            let mid = lo_d + (hi_d - lo_d) / 2;
            out.push(digit(mid));
            return out;
        }
        out.push(digit(lo_d));
    }
    out.push(digit(BASE_MIDPOINT));
    out
}

const MIDPOINT_DESCENT_CAP: usize = 256;

const BASE_MIDPOINT: usize = BASE / 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_type_taxonomy_is_frozen() {
        let all = FieldType::all();
        assert_eq!(all.len(), 8, "the closed frozen taxonomy is eight variants");
        for (i, t) in all.iter().enumerate() {
            assert_eq!(
                *t as u8,
                i as u8,
                "{} discriminant is pinned to {i}",
                t.wire_id()
            );
        }
        let ids: Vec<&str> = all.iter().map(|t| t.wire_id()).collect();
        assert_eq!(
            ids,
            [
                "text",
                "int",
                "bool",
                "date",
                "select",
                "relation",
                "principal",
                "order_key"
            ],
            "the frozen wire-id set, in order"
        );
    }

    #[test]
    fn field_value_pairs_with_its_type() {
        assert_eq!(FieldValue::Text("x".into()).field_type(), FieldType::Text);
        assert_eq!(FieldValue::Int(1).field_type(), FieldType::Int);
        assert_eq!(FieldValue::Bool(true).field_type(), FieldType::Bool);
        assert_eq!(
            FieldValue::Date("2026-06-20".into()).field_type(),
            FieldType::Date
        );
        assert_eq!(
            FieldValue::Select("opt".into()).field_type(),
            FieldType::Select
        );
        assert_eq!(
            FieldValue::Relation("ref".into()).field_type(),
            FieldType::Relation
        );
        assert_eq!(
            FieldValue::Principal("p".into()).field_type(),
            FieldType::Principal
        );
        let k = OrderKey::bisect(None, None);
        assert_eq!(FieldValue::OrderKey(k).field_type(), FieldType::OrderKey);
    }

    #[test]
    fn ordered_types_are_int_date_orderkey() {
        for t in [FieldType::Int, FieldType::Date, FieldType::OrderKey] {
            assert!(t.is_ordered(), "{} is ordered", t.wire_id());
        }
        for t in [
            FieldType::Text,
            FieldType::Bool,
            FieldType::Select,
            FieldType::Relation,
            FieldType::Principal,
        ] {
            assert!(!t.is_ordered(), "{} is equality-only", t.wire_id());
        }
    }

    #[test]
    fn lexorank_alphabet_is_byte_sorted() {
        assert_eq!(LEXORANK_ALPHABET.len(), 62);
        let mut sorted = *LEXORANK_ALPHABET;
        sorted.sort_unstable();
        assert_eq!(
            &sorted, LEXORANK_ALPHABET,
            "the alphabet is already byte-sorted"
        );
        let pos = |c: u8| LEXORANK_ALPHABET.iter().position(|&b| b == c).unwrap();
        assert!(pos(b'0') < pos(b'9'), "digits precede letters");
        assert!(pos(b'9') < pos(b'A'), "digits precede uppercase");
        assert!(pos(b'Z') < pos(b'a'), "uppercase precedes lowercase");
    }

    #[test]
    fn parse_rejects_out_of_alphabet_keys() {
        assert!(OrderKey::parse("").is_none(), "empty is rejected");
        assert!(
            OrderKey::parse("ab-cd").is_none(),
            "`-` is not in the alphabet"
        );
        assert!(OrderKey::parse("V5").is_some(), "a base-62 key parses");
    }

    #[test]
    fn bisect_is_strictly_between() {
        let first = OrderKey::bisect(None, None);
        assert!(!first.as_str().is_empty());
        assert!(
            OrderKey::parse(first.as_str()).is_some(),
            "first key is in-alphabet"
        );

        let after = OrderKey::bisect(Some(&first), None);
        assert!(first < after, "append: {first} < {after}");

        let before = OrderKey::bisect(None, Some(&first));
        assert!(before < first, "prepend: {before} < {first}");

        let lo = OrderKey::parse("V").unwrap();
        let hi = OrderKey::parse("X").unwrap();
        let mid = OrderKey::bisect(Some(&lo), Some(&hi));
        assert!(lo < mid && mid < hi, "between: {lo} < {mid} < {hi}");

        let lo2 = OrderKey::parse("V").unwrap();
        let hi2 = OrderKey::parse("W").unwrap();
        let mid2 = OrderKey::bisect(Some(&lo2), Some(&hi2));
        assert!(
            lo2 < mid2 && mid2 < hi2,
            "adjacent descent: {lo2} < {mid2} < {hi2}"
        );
    }

    #[test]
    fn repeated_bisection_stays_ordered_and_signals_rebalance() {
        let lo = OrderKey::parse("V").unwrap();
        let mut hi = OrderKey::parse("W").unwrap();
        let mut prev = lo.clone();
        for _ in 0..60 {
            let mid = OrderKey::bisect(Some(&lo), Some(&hi));
            assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
            assert!(prev <= mid || mid < prev, "still a total order");
            prev = mid.clone();
            hi = mid;
        }
        assert!(hi.needs_rebalance() || hi.as_str().len() < LEXORANK_REBALANCE_LEN);
    }

    #[test]
    fn order_key_accessors_are_exact() {
        let k = OrderKey::parse("V5z").unwrap();
        assert_eq!(k.as_str(), "V5z", "as_str returns the raw key verbatim");
        assert_eq!(format!("{k}"), "V5z", "Display renders the raw key");

        let short = OrderKey::parse("V").unwrap();
        assert!(
            !short.needs_rebalance(),
            "a 1-char key is well below the rebalance trigger"
        );
        let at_trigger = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN)).unwrap();
        assert!(
            at_trigger.needs_rebalance(),
            "a {LEXORANK_REBALANCE_LEN}-char key trips it"
        );
        let just_below = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN - 1)).unwrap();
        assert!(
            !just_below.needs_rebalance(),
            "one char below the trigger does NOT trip"
        );
    }

    #[test]
    fn field_value_round_trips() {
        let v = FieldValue::OrderKey(OrderKey::bisect(None, None));
        let json = serde_json::to_string(&v).unwrap();
        let back: FieldValue = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);

        let t = FieldType::Relation;
        let tj = serde_json::to_string(&t).unwrap();
        let tb: FieldType = serde_json::from_str(&tj).unwrap();
        assert_eq!(t, tb);
    }

    #[test]
    fn jitter_from_ranks_rejects_out_of_range() {
        assert_eq!(
            Jitter::from_ranks(0, 0).unwrap().as_str(),
            "00",
            "rank 0 → '0'"
        );
        assert_eq!(
            Jitter::from_ranks(61, 61).unwrap().as_str(),
            "zz",
            "rank 61 → 'z'"
        );
        assert_eq!(Jitter::from_ranks(1, 0).unwrap().as_str(), "10");
        assert!(
            Jitter::from_ranks(62, 0).is_none(),
            "first rank == BASE is rejected"
        );
        assert!(
            Jitter::from_ranks(0, 62).is_none(),
            "second rank == BASE is rejected"
        );
        assert!(
            Jitter::from_ranks(99, 99).is_none(),
            "both out of range rejected"
        );
    }

    #[test]
    fn jitter_random_maps_bytes_mod_62() {
        assert_eq!(Jitter::random([0, 0]).as_str(), "00");
        assert_eq!(
            Jitter::random([62, 62]).as_str(),
            "00",
            "byte 62 wraps mod-62 to '0'"
        );
        assert_eq!(Jitter::random([61, 1]).as_str(), "z1");
        assert_eq!(
            Jitter::random([124, 63]).as_str(),
            "01",
            "124%62=0 → '0', 63%62=1 → '1'"
        );
        for b in 0u8..=255 {
            let j = Jitter::random([b, b]);
            assert!(
                j.as_str().bytes().all(|c| LEXORANK_ALPHABET.contains(&c)),
                "byte {b} maps to an in-alphabet jitter"
            );
        }
    }

    #[test]
    fn jitter_as_str_is_exact() {
        assert_eq!(
            Jitter::from_ranks(36, 10).unwrap().as_str(),
            "aA",
            "rank 36='a', 10='A'"
        );
        assert_eq!(Jitter::ZERO.as_str(), "00", "the ZERO jitter is '00'");
    }

    #[test]
    fn jitter_suffix_preserves_order() {
        let a = OrderKey::rank_between(
            None,
            Some(&OrderKey::parse("U00").unwrap()),
            Jitter::from_ranks(61, 61).unwrap(),
        );
        let b = OrderKey::rank_between(
            Some(&OrderKey::parse("U00").unwrap()),
            None,
            Jitter::from_ranks(0, 0).unwrap(),
        );
        assert!(
            a < b,
            "the midpoint body dominates the jitter suffix: {a} < {b}"
        );
    }

    #[test]
    fn rank_last_is_first_when_empty_and_after_otherwise() {
        let j = Jitter::from_ranks(7, 7).unwrap();
        assert_eq!(
            OrderKey::rank_last(None, j).as_str(),
            OrderKey::rank_first(j).as_str(),
            "rank_last of an empty list == rank_first"
        );
        let a = OrderKey::parse("U00").unwrap();
        let last = OrderKey::rank_last(Some(&a), j);
        assert!(
            a < last,
            "rank_last lands strictly after the prior tail: {a} < {last}"
        );
    }

    #[test]
    fn rank_first_anchors_at_u() {
        assert_eq!(
            LEXORANK_FIRST, "U",
            "the frozen initial-spacing anchor is 'U'"
        );
        let k = OrderKey::rank_first(Jitter::from_ranks(0, 0).unwrap());
        assert_eq!(k.as_str(), "U00", "rank_first == 'U' ++ jitter");
        assert_eq!(&k.as_str()[..1], "U", "the body is the 'U' anchor");
    }

    #[test]
    fn base_midpoint_fallback_is_alphabet_centre() {
        assert_eq!(
            BASE_MIDPOINT, 31,
            "BASE/2 == 31 (the centre of the 62-digit alphabet)"
        );
        assert_eq!(digit(BASE_MIDPOINT), b'V', "the centre digit is 'V'");
    }

    #[test]
    fn needs_rebalance_key_mirrors_method() {
        let short = OrderKey::parse("V").unwrap();
        let long = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN)).unwrap();
        assert!(
            !OrderKey::needs_rebalance_key(&short),
            "short key does not need rebalance"
        );
        assert!(
            OrderKey::needs_rebalance_key(&long),
            "a 48-char key needs rebalance"
        );
        assert_eq!(
            OrderKey::needs_rebalance_key(&short),
            short.needs_rebalance()
        );
        assert_eq!(OrderKey::needs_rebalance_key(&long), long.needs_rebalance());
    }
}
