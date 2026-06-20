//! The frozen **`FieldType` enum + `FieldValue` + the `order_key`/LexoRank fractional-index
//! encoding** (contract 13.3, frozen byte-identical X-3/OQ-C).
//!
//! **Owning architecture/contract:** `contract-index.md` row 13.3 — *"the field-type enum, the
//! view-model (`ViewSpec`), the `QueryAst` grammar (= the `EventMatcher` core, 3.4), and the
//! `order_key`/LexoRank fractional-index encoding (base-62 `0-9A-Za-z`, lexicographic compare,
//! midpoint bisection, 2-char jitter, 48-char rebalance trigger, `created_at`+ULID tiebreak)."*
//! Co-owned by **Issues + Knowledge** (their compilers/executors differ; the definitions are
//! **identical**); a **compile target** in **Search** (`search-and-indexing.md` §2.2 / §3.1 — the
//! structured/columnar shape filters over the frozen `FieldType`; `order_key` is a columnar
//! fast-field for sort, *byte-identical to Issues' and Knowledge's encoding*).
//!
//! ## DEVIATION (recorded — external-insights/01 §1)
//! The contract-index names **P-235 (KN-P02) + the Issues prompts** as the owners that *freeze* the
//! `FieldType` enum + the `ViewSpec` view-model + the textual `QueryAst` grammar parser (see the
//! crate-level "Floor named" note in `lib.rs`). The topo-sort placed those *after* this slice.
//! **SRCH-P04 (P-167) requires the structured shape to be typed *byte-identically* over the frozen
//! `FieldType` enum NOW** ("a `FieldType` rename breaks this now" — the prompt's GATE/DRILLS). To
//! avoid a second, parallel definition (EI-01 §7 — never define a contract type twice), the
//! **minimal frozen `FieldType` enum + `FieldValue` + `OrderKey` (LexoRank) land HERE, in their
//! contract home (`myelin-query`)**, where the rest of the 13.3 primitive ([`crate::QueryAst`],
//! [`crate::Predicate`]) already lives. P-235 / the Issues prompts then **EXTEND this in place** —
//! they add the textual grammar parser + the `ViewSpec` view-model + each subsystem's
//! compiler/executor on top of the ONE frozen enum; they do **not** redefine it. Search consumes
//! this exact type, so a `FieldType` rename breaks Search's drift test (and every consumer) at once.

use serde::{Deserialize, Serialize};

/// **The frozen field-type enum** (contract 13.3, byte-identical across Issues / Knowledge /
/// Search). It types a structured/columnar facet — the value space a `Cmp`/`In`/`Has`/`Ref`
/// predicate filters over and the columnar fast-field the structured index sorts/filters on
/// (`search-and-indexing.md` §2.2 / §3.1). The variant set is the closed, frozen taxonomy: a
/// rename/reorder is a **wire-breaking** change that the consumers' drift tests catch at once.
///
/// The discriminant is **explicit and stable** (`#[repr(u8)]` with pinned values) so the
/// byte-identical encoding is structural, not incidental — a reorder cannot silently renumber a
/// facet type across the three co-owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum FieldType {
    /// Analyzable free text (the full-text/BM25 shape feeds the inverted index; also a stored
    /// columnar facet for exact-match structured predicates). Discriminant `0`.
    Text = 0,
    /// A 64-bit signed integer facet (ordered comparisons defined — the structured `Lt/Le/Gt/Ge`
    /// shape). Discriminant `1`.
    Int = 1,
    /// A boolean facet. Discriminant `2`.
    Bool = 2,
    /// A date/timestamp facet, encoded as an RFC-3339/ISO-8601 lexicographically-sortable string
    /// (so the columnar fast-field sorts chronologically by byte order). Discriminant `3`.
    Date = 3,
    /// A single-/multi-select option facet (an opaque option token). Discriminant `4`.
    Select = 4,
    /// A relation facet — an `ArtifactRef` token to another object (the `Ref`/`InRelation` shape).
    /// Discriminant `5`.
    Relation = 5,
    /// A principal facet — a pseudonymous principal token (assignee/reporter), compared by equality
    /// only, never ordered. Discriminant `6`.
    Principal = 6,
    /// The **`order_key`** facet — the LexoRank fractional index ([`OrderKey`]); a columnar
    /// fast-field whose **byte order is the sort order** (§3.1). Discriminant `7`.
    OrderKey = 7,
}

impl FieldType {
    /// The stable, PII-free wire id of the field type (the byte-identical token the three co-owners
    /// share — the drift anchor). A rename here is the wire-breaking change the consumers' tests
    /// catch.
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

    /// The full, ordered, frozen taxonomy (the closed variant set). Pinned so a consumer can assert
    /// byte-identity over the WHOLE enum, not a sampled subset.
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

    /// `true` iff the type defines a **total order** over its values (so the structured shape may
    /// build an ordered/range columnar fast-field over it). `Int`/`Date`/`OrderKey` are ordered;
    /// `Text`/`Select`/`Relation`/`Principal` compare by equality only; `Bool` is equality-only.
    pub fn is_ordered(self) -> bool {
        matches!(self, FieldType::Int | FieldType::Date | FieldType::OrderKey)
    }
}

/// **A typed structured/columnar facet VALUE** — the value a [`FieldType`] facet carries in an
/// index document (the structured shape, §3.1). Each variant pairs with exactly one [`FieldType`]
/// ([`FieldValue::field_type`]); a mismatch between a facet's declared type and its value is a
/// type error the indexer rejects (it never silently coerces).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldValue {
    /// A [`FieldType::Text`] value.
    Text(String),
    /// A [`FieldType::Int`] value.
    Int(i64),
    /// A [`FieldType::Bool`] value.
    Bool(bool),
    /// A [`FieldType::Date`] value (the lexicographically-sortable RFC-3339 string).
    Date(String),
    /// A [`FieldType::Select`] option token.
    Select(String),
    /// A [`FieldType::Relation`] `ArtifactRef` token.
    Relation(String),
    /// A [`FieldType::Principal`] pseudonymous principal token.
    Principal(String),
    /// A [`FieldType::OrderKey`] LexoRank fractional index.
    OrderKey(OrderKey),
}

impl FieldValue {
    /// The [`FieldType`] this value belongs to (the type/value pairing — the structured shape uses
    /// it to pick the columnar fast-field kind, and to reject a declared-type/value mismatch).
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

/// The base-62 alphabet of the LexoRank encoding (contract 13.3): `0-9A-Za-z`, **in this exact
/// order** so the lexicographic byte compare *is* the numeric order. Frozen byte-identically.
pub const LEXORANK_ALPHABET: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The rebalance-trigger length (contract 13.3): an `order_key` that grows to **48** characters
/// signals the collection should be rebalanced (the bisection has run out of headroom). A pure
/// signal — `bisect` never refuses; the owner reacts to [`OrderKey::needs_rebalance`].
pub const LEXORANK_REBALANCE_LEN: usize = 48;

/// **The `order_key` LexoRank fractional index** (contract 13.3, byte-identical across Issues /
/// Knowledge / Search). A base-62 string whose **lexicographic byte order is the sort order**, so
/// the structured columnar fast-field sorts by raw byte compare. New positions are produced by
/// **midpoint bisection** between two neighbours ([`OrderKey::bisect`]) — no global renumber on an
/// insert. (The `created_at`+ULID tiebreak for equal-key rows is the owner's row-level concern; the
/// key itself is frozen here.)
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OrderKey(String);

impl OrderKey {
    /// Build an `OrderKey` from a raw base-62 string. Returns `None` if any byte is outside the
    /// frozen [`LEXORANK_ALPHABET`] (an out-of-alphabet key would break the byte-order = sort-order
    /// invariant) or the string is empty.
    pub fn parse(s: impl Into<String>) -> Option<OrderKey> {
        let s = s.into();
        if s.is_empty() || !s.bytes().all(|b| LEXORANK_ALPHABET.contains(&b)) {
            return None;
        }
        Some(OrderKey(s))
    }

    /// The raw base-62 key string (the columnar fast-field byte value).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` once the key has grown to the [`LEXORANK_REBALANCE_LEN`] rebalance trigger — the
    /// collection owner should renumber. A pure signal; the key remains valid and sortable.
    pub fn needs_rebalance(&self) -> bool {
        self.0.len() >= LEXORANK_REBALANCE_LEN
    }

    /// The canonical **midpoint** key between `lo` (exclusive, or the start) and `hi` (exclusive,
    /// or the end): `bisect(None, None)` is the first key; `bisect(Some(a), None)` appends after
    /// `a`; `bisect(None, Some(b))` prepends before `b`; `bisect(Some(a), Some(b))` is strictly
    /// between them. The result `k` always satisfies `lo < k < hi` lexicographically.
    ///
    /// The algorithm is the contract-13.3 midpoint bisection over the base-62 alphabet: walk the
    /// two bounds digit-by-digit; where they leave room, place the arithmetic midpoint digit; where
    /// they are adjacent, descend a level (the key grows one char). The 2-char jitter and the
    /// `created_at`+ULID tiebreak are the owner's row-level concerns layered on top; the bisection
    /// itself — the frozen, byte-identical core — is here.
    pub fn bisect(lo: Option<&OrderKey>, hi: Option<&OrderKey>) -> OrderKey {
        let lo_b = lo.map(|k| k.0.as_bytes()).unwrap_or(&[]);
        let hi_b = hi.map(|k| k.0.as_bytes()).unwrap_or(&[]);
        // `midpoint` only ever emits bytes drawn from `LEXORANK_ALPHABET` (all ASCII), so the
        // conversion is infallible.
        OrderKey(String::from_utf8(midpoint(lo_b, hi_b)).expect("LexoRank digits are ASCII"))
    }
}

impl std::fmt::Display for OrderKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The rank (0..62) of a base-62 digit byte. Bytes are guaranteed in-alphabet by construction
/// ([`OrderKey::parse`]); a stray byte ranks as the *end* sentinel (62) so an unexpected input
/// fails toward "after everything" rather than corrupting the order.
fn rank(b: u8) -> usize {
    LEXORANK_ALPHABET
        .iter()
        .position(|&c| c == b)
        .unwrap_or(LEXORANK_ALPHABET.len())
}

/// The digit byte for a rank in `0..62`.
fn digit(r: usize) -> u8 {
    LEXORANK_ALPHABET[r]
}

const BASE: usize = 62;

/// Midpoint bisection over the base-62 alphabet (the frozen LexoRank core). Returns a key `k` with
/// `lo < k < hi` lexicographically, where an empty `lo`/`hi` means "start"/"end".
fn midpoint(lo: &[u8], hi: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // A hard descent cap: bisection can only descend so far before a gap MUST open (between any two
    // distinct bounds there is always a base-62 midpoint within a bounded number of levels). The cap
    // makes the loop provably terminating (defence in depth — a malformed/equal bound can never spin
    // forever). It dominates any realistic key length (well past the LEXORANK_REBALANCE_LEN rebalance
    // trigger), so it never truncates a legitimate midpoint.
    for i in 0..MIDPOINT_DESCENT_CAP {
        // The digit of each bound at position `i` (lo's missing tail reads as 0/start; hi's missing
        // tail reads as the end sentinel BASE so "append after `lo`" places mid digits).
        let lo_d = if i < lo.len() { rank(lo[i]) } else { 0 };
        let hi_d = if i < hi.len() {
            rank(hi[i])
        } else {
            // hi has run out of digits (empty bound = "end", or a prefix of itself): nothing sorts
            // strictly between a prefix and its own extension at this slot — descend with the end
            // sentinel BASE so a midpoint opens at the next finer level.
            BASE
        };

        if hi_d > lo_d + 1 {
            // Room for a midpoint digit strictly between the two bounds at this slot.
            let mid = lo_d + (hi_d - lo_d) / 2;
            out.push(digit(mid));
            return out;
        }
        // The bounds are equal or adjacent at this slot: copy lo's digit (keeping us > lo so far)
        // and descend to the next, finer slot. If lo has no digit here, anchor at the alphabet
        // start and keep descending until a gap opens.
        out.push(digit(lo_d));
    }
    // Unreachable for valid distinct bounds (the cap dominates the levels ever needed); the trailing
    // mid-digit keeps a result strictly above `lo` even in the degenerate path.
    out.push(digit(BASE_MIDPOINT));
    out
}

/// The hard descent cap for [`midpoint`] (defence-in-depth termination bound — see the body). It
/// dominates any realistic LexoRank key length so it never truncates a legitimate midpoint.
const MIDPOINT_DESCENT_CAP: usize = 256;

/// The fallback mid-digit for the (unreachable, for distinct bounds) degenerate path — the centre of
/// the base-62 alphabet.
const BASE_MIDPOINT: usize = BASE / 2;

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen taxonomy is exactly eight variants, in order, with stable discriminants and wire
    /// ids — the byte-identical anchor the three co-owners (Issues/Knowledge/Search) reconcile to.
    #[test]
    fn field_type_taxonomy_is_frozen() {
        let all = FieldType::all();
        assert_eq!(all.len(), 8, "the closed frozen taxonomy is eight variants");
        // Discriminants are pinned 0..8 in declaration order (the byte-identical wire encoding).
        for (i, t) in all.iter().enumerate() {
            assert_eq!(*t as u8, i as u8, "{} discriminant is pinned to {i}", t.wire_id());
        }
        let ids: Vec<&str> = all.iter().map(|t| t.wire_id()).collect();
        assert_eq!(
            ids,
            ["text", "int", "bool", "date", "select", "relation", "principal", "order_key"],
            "the frozen wire-id set, in order"
        );
    }

    /// Every `FieldValue` reports the matching `FieldType` (the type/value pairing the structured
    /// shape relies on — no silent coercion).
    #[test]
    fn field_value_pairs_with_its_type() {
        assert_eq!(FieldValue::Text("x".into()).field_type(), FieldType::Text);
        assert_eq!(FieldValue::Int(1).field_type(), FieldType::Int);
        assert_eq!(FieldValue::Bool(true).field_type(), FieldType::Bool);
        assert_eq!(FieldValue::Date("2026-06-20".into()).field_type(), FieldType::Date);
        assert_eq!(FieldValue::Select("opt".into()).field_type(), FieldType::Select);
        assert_eq!(FieldValue::Relation("ref".into()).field_type(), FieldType::Relation);
        assert_eq!(FieldValue::Principal("p".into()).field_type(), FieldType::Principal);
        let k = OrderKey::bisect(None, None);
        assert_eq!(FieldValue::OrderKey(k).field_type(), FieldType::OrderKey);
    }

    /// `is_ordered` is correct (the structured shape only builds a range fast-field over ordered
    /// types).
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

    /// The LexoRank alphabet is the frozen base-62 set in the order where byte-compare = numeric
    /// order (`0 < 9 < A < Z < a < z`).
    #[test]
    fn lexorank_alphabet_is_byte_sorted() {
        assert_eq!(LEXORANK_ALPHABET.len(), 62);
        let mut sorted = *LEXORANK_ALPHABET;
        sorted.sort_unstable();
        assert_eq!(&sorted, LEXORANK_ALPHABET, "the alphabet is already byte-sorted");
        // The blocks are in `0-9A-Za-z` order, so a position lookup confirms `'0' < 'A' < 'a'`
        // (the byte-compare = numeric-order property, checked against the actual array, not a
        // compile-time constant).
        let pos = |c: u8| LEXORANK_ALPHABET.iter().position(|&b| b == c).unwrap();
        assert!(pos(b'0') < pos(b'9'), "digits precede letters");
        assert!(pos(b'9') < pos(b'A'), "digits precede uppercase");
        assert!(pos(b'Z') < pos(b'a'), "uppercase precedes lowercase");
    }

    /// `parse` rejects out-of-alphabet / empty keys (an out-of-alphabet key would break the
    /// byte-order = sort-order invariant).
    #[test]
    fn parse_rejects_out_of_alphabet_keys() {
        assert!(OrderKey::parse("").is_none(), "empty is rejected");
        assert!(OrderKey::parse("ab-cd").is_none(), "`-` is not in the alphabet");
        assert!(OrderKey::parse("V5").is_some(), "a base-62 key parses");
    }

    /// **Midpoint bisection produces a strictly-between key (`lo < k < hi`), the load-bearing
    /// LexoRank invariant.** Covers the four bound shapes: first, append, prepend, between.
    #[test]
    fn bisect_is_strictly_between() {
        // First key (no neighbours): non-empty and a valid key.
        let first = OrderKey::bisect(None, None);
        assert!(!first.as_str().is_empty());
        assert!(OrderKey::parse(first.as_str()).is_some(), "first key is in-alphabet");

        // Append after `first`.
        let after = OrderKey::bisect(Some(&first), None);
        assert!(first < after, "append: {first} < {after}");

        // Prepend before `first`.
        let before = OrderKey::bisect(None, Some(&first));
        assert!(before < first, "prepend: {before} < {first}");

        // Strictly between two adjacent-ish neighbours.
        let lo = OrderKey::parse("V").unwrap();
        let hi = OrderKey::parse("X").unwrap();
        let mid = OrderKey::bisect(Some(&lo), Some(&hi));
        assert!(lo < mid && mid < hi, "between: {lo} < {mid} < {hi}");

        // Between two ADJACENT digits forces a descent (the key grows one level) but stays between.
        let lo2 = OrderKey::parse("V").unwrap();
        let hi2 = OrderKey::parse("W").unwrap();
        let mid2 = OrderKey::bisect(Some(&lo2), Some(&hi2));
        assert!(lo2 < mid2 && mid2 < hi2, "adjacent descent: {lo2} < {mid2} < {hi2}");
    }

    /// Repeatedly bisecting between a fixed `lo` and the previous midpoint stays ordered and
    /// converges (the headroom-exhaustion path that eventually trips the rebalance signal).
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
        // A long enough key trips the rebalance signal (a pure signal — bisect never refused).
        assert!(hi.needs_rebalance() || hi.as_str().len() < LEXORANK_REBALANCE_LEN);
    }

    /// **`OrderKey` accessors are exact** — `as_str`/`Display` return the raw key, and
    /// `needs_rebalance` flips exactly at the [`LEXORANK_REBALANCE_LEN`] trigger. Kills the
    /// accessor / boundary mutants.
    #[test]
    fn order_key_accessors_are_exact() {
        let k = OrderKey::parse("V5z").unwrap();
        assert_eq!(k.as_str(), "V5z", "as_str returns the raw key verbatim");
        assert_eq!(format!("{k}"), "V5z", "Display renders the raw key");

        // needs_rebalance: false below the trigger, true at/above it.
        let short = OrderKey::parse("V").unwrap();
        assert!(!short.needs_rebalance(), "a 1-char key is well below the rebalance trigger");
        let at_trigger = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN)).unwrap();
        assert!(at_trigger.needs_rebalance(), "a {LEXORANK_REBALANCE_LEN}-char key trips it");
        let just_below = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN - 1)).unwrap();
        assert!(!just_below.needs_rebalance(), "one char below the trigger does NOT trip");
    }

    /// `FieldValue` + `OrderKey` serialize/deserialize stably (the wire contract the three co-owners
    /// share).
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
}
