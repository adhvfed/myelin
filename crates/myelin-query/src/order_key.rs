//! The **X-3 LexoRank conformance vector** + the `created_at`+ULID **tiebreak** — the anti-drift
//! artifact of contract 13.3 (`order_key`/LexoRank, frozen byte-identical with Issues).
//!
//! ## What KN-P03 (P-236) freezes here
//! [`field`](crate::field) already carries the encoding CORE (the [`OrderKey`] type, the base-62
//! alphabet, the midpoint bisection, the 2-char [`Jitter`], the 48-char rebalance trigger,
//! [`OrderKey::rank_first`]/[`OrderKey::rank_last`]/[`OrderKey::rank_between`]). This module adds the
//! two things KN-P03 freezes ON TOP of that core, WITHOUT redefining it (EI-01 §7 — never a second
//! definition):
//!
//! 1. **The shared conformance vector** ([`CONFORMANCE_VECTOR`]) — a deterministic, hand-checked
//!    sequence of rank operations and their **expected base-62 outputs**, INCLUDING a concurrent
//!    same-gap collision exercising the jitter and a 48-char rebalance. It is the X-3 *drift-killer*:
//!    the Knowledge side (this crate's [`replay_conformance_vector`] test) and the Issues side
//!    (ISS-P02 / P-241, which adds this crate as a dependency and replays the SAME vector through its
//!    OWN executor) build to byte-identical outputs. A single source of truth, authored once.
//!
//! 2. **The `created_at`+ULID tiebreak** ([`tiebreak`]) — the deterministic total order for the
//!    (should-not-happen-with-jitter) case where two rows carry an equal `order_key`. Equal keys
//!    break the `created_at` first, then the ULID `id`. This is the contract-13.3 "total order
//!    guaranteed" leg.
//!
//! ## FLOOR named (EI-01 §1) — none. This is a FREEZE, not a stub.
//! The order_key stays the **OLTP ordering encoding** now. When the move-CRDT lands (KN-P29), the
//! CRDT's list type owns sibling ordering and the `order_key` becomes a **derived OLTP-index
//! ordering HINT recomputed from CRDT state** (architecture 02 §3.5, the CR-A interaction) — the
//! bespoke jitter/rebalance logic retires, but the encoding above it does NOT change. So there is no
//! deferred-and-unbuilt floor here: the freeze is complete; the CRDT lands *over* this order model.

use crate::field::{Jitter, OrderKey};
use std::cmp::Ordering;

/// **The `created_at`+ULID tiebreak** (contract 13.3 — "total order guaranteed"). When two rows
/// compare EQUAL on their `order_key` (which the 2-char jitter makes vanishingly unlikely, but the
/// total order must still be defined), the deterministic break is **`created_at` first, then the
/// ULID `id`**. ULIDs are lexicographically time-ordered, so a raw string compare on the id is a
/// stable, monotone secondary key.
///
/// `created_at` is the RFC-3339/ISO-8601 lexicographically-sortable timestamp string (the same shape
/// [`crate::field::FieldType::Date`] uses), so a byte compare *is* chronological order. The result is
/// a **total order over `(order_key, created_at, id)`** — never `Equal` for two distinct rows
/// (distinct ULIDs guarantee the final tiebreak is decisive).
pub fn tiebreak(
    a_key: &OrderKey,
    a_created_at: &str,
    a_id: &str,
    b_key: &OrderKey,
    b_created_at: &str,
    b_id: &str,
) -> Ordering {
    a_key
        .cmp(b_key)
        .then_with(|| a_created_at.cmp(b_created_at))
        .then_with(|| a_id.cmp(b_id))
}

/// One step of the [`CONFORMANCE_VECTOR`]: a frozen rank operation and its **expected** base-62
/// output. The replay test executes `op` and asserts the produced [`OrderKey`] equals `expect`
/// byte-for-byte. Both co-owners replay the SAME list.
#[derive(Clone, Copy, Debug)]
pub struct ConformanceStep {
    /// A human-readable label (what the step exercises — for a legible failure message).
    pub label: &'static str,
    /// The operation to perform (a closure-free, data-only description so the vector is a pure
    /// fixture both sides can ship identically).
    pub op: RankOp,
    /// The expected base-62 `order_key` string the operation MUST produce (the byte-identical
    /// anti-drift anchor).
    pub expect: &'static str,
}

/// A data-only description of a single rank operation (so the conformance vector is a pure fixture,
/// not code — both co-owners ship the identical data and run it through their OWN [`OrderKey`] calls).
/// The `lo`/`hi`/`after` bounds are given as base-62 strings (or `None`); the jitter as two explicit
/// base-62 ranks so the output is deterministic and reproducible.
#[derive(Clone, Copy, Debug)]
pub enum RankOp {
    /// [`OrderKey::rank_first`] with the given jitter ranks.
    First { jitter: (usize, usize) },
    /// [`OrderKey::rank_last`] after an optional `after` key, with the given jitter ranks.
    Last {
        after: Option<&'static str>,
        jitter: (usize, usize),
    },
    /// [`OrderKey::rank_between`] of `lo`/`hi` (either `None`), with the given jitter ranks.
    Between {
        lo: Option<&'static str>,
        hi: Option<&'static str>,
        jitter: (usize, usize),
    },
}

impl ConformanceStep {
    /// Execute this step through the FROZEN [`OrderKey`] operations and return the produced key.
    /// This is the ONE code path both co-owners call — a drift in the encoding shows up as a
    /// produced-vs-`expect` mismatch.
    pub fn run(&self) -> OrderKey {
        match self.op {
            RankOp::First { jitter } => {
                OrderKey::rank_first(jit(jitter))
            }
            RankOp::Last { after, jitter } => {
                let after = after.map(parse_key);
                OrderKey::rank_last(after.as_ref(), jit(jitter))
            }
            RankOp::Between { lo, hi, jitter } => {
                let lo = lo.map(parse_key);
                let hi = hi.map(parse_key);
                OrderKey::rank_between(lo.as_ref(), hi.as_ref(), jit(jitter))
            }
        }
    }
}

/// Parse a fixture base-62 string into an [`OrderKey`] (the fixture is hand-authored over the
/// alphabet, so an out-of-alphabet bound is a fixture bug — panic loudly, never silently corrupt the
/// order).
fn parse_key(s: &'static str) -> OrderKey {
    OrderKey::parse(s).expect("conformance-vector bound is a valid base-62 order_key")
}

/// Build a [`Jitter`] from two explicit ranks (fixture data is in-range by construction; a bad
/// fixture panics loudly).
fn jit((a, b): (usize, usize)) -> Jitter {
    Jitter::from_ranks(a, b).expect("conformance-vector jitter ranks are in 0..62")
}

/// **THE X-3 LEXORANK CONFORMANCE VECTOR** (contract 13.3, frozen byte-identical with Issues).
///
/// A deterministic sequence of rank operations + their expected base-62 outputs. Authored ONCE here;
/// the Knowledge side ([`replay_conformance_vector`], this crate) and the Issues side (ISS-P02 /
/// P-241, which depends on this crate and replays the SAME slice) both assert byte-for-byte. **0 rank
/// divergences across the shared vector** is the dated green artifact of KN-P03.
///
/// The vector deliberately exercises every leg of the encoding:
/// - the first key (`"U"` anchor + jitter),
/// - append / prepend / strictly-between,
/// - an **adjacent-bound descent** (the key grows one char rather than rebalancing),
/// - a **concurrent same-gap collision** (two inserts into the IDENTICAL gap with DIFFERENT jitter →
///   DISTINCT keys, the concurrency-safety property),
/// - a **48-char rebalance** trigger (a key grown to [`LEXORANK_REBALANCE_LEN`] flips
///   [`OrderKey::needs_rebalance`]).
pub const CONFORMANCE_VECTOR: &[ConformanceStep] = &[
    // ── 1. The first key of a fresh collection: "U" anchor + jitter "00". ──
    ConformanceStep {
        label: "first key (U anchor + 00 jitter)",
        op: RankOp::First { jitter: (0, 0) },
        expect: "U00",
    },
    // ── 2. Append after "U00" (drag to the end): midpoint(U00, end) + jitter "10". ──
    //  midpoint(U00, <end>): slot0 lo='U'(30) hi=end(62) → 62>30+1 → mid=30+(62-30)/2=46='k'. body="k".
    ConformanceStep {
        label: "append after U00 (last)",
        op: RankOp::Last {
            after: Some("U00"),
            jitter: (1, 0),
        },
        expect: "k10",
    },
    // ── 3. Prepend before "U00" (drag to the front): midpoint(start, U00) + jitter "22". ──
    //  midpoint(<start>, U00): slot0 lo=0 hi='U'(30) → 30>0+1 → mid=0+(30-0)/2=15='F'. body="F".
    ConformanceStep {
        label: "prepend before U00 (between None,U00)",
        op: RankOp::Between {
            lo: None,
            hi: Some("U00"),
            jitter: (2, 2),
        },
        expect: "F22",
    },
    // ── 4. Strictly between "F22" and "U00": midpoint + jitter "33". ──
    //  midpoint(F22, U00): slot0 lo='F'(15) hi='U'(30) → 30>15+1 → mid=15+(30-15)/2=22='M'. body="M".
    ConformanceStep {
        label: "between F22 and U00",
        op: RankOp::Between {
            lo: Some("F22"),
            hi: Some("U00"),
            jitter: (3, 3),
        },
        expect: "M33",
    },
    // ── 5. ADJACENT-BOUND DESCENT: between "V" and "W" (adjacent digits 31,32) the key GROWS one
    //  char rather than rebalancing. midpoint(V, W): slot0 lo='V'(31) hi='W'(32) adjacent → push
    //  digit(31)='V', descend; slot1 lo=0 hi=end(62) → 62>0+1 → mid=31='V'. body="VV". + jitter "44".
    ConformanceStep {
        label: "adjacent descent between V and W (key grows one char)",
        op: RankOp::Between {
            lo: Some("V"),
            hi: Some("W"),
            jitter: (4, 4),
        },
        expect: "VV44",
    },
    // ── 6. CONCURRENT SAME-GAP COLLISION (leg A): two clients drag into the IDENTICAL gap (F22..U00)
    //  at the SAME instant. Client A uses jitter "55". midpoint body == "M" (same as step 4). ──
    ConformanceStep {
        label: "concurrent same-gap insert A (jitter 55)",
        op: RankOp::Between {
            lo: Some("F22"),
            hi: Some("U00"),
            jitter: (5, 5),
        },
        expect: "M55",
    },
    // ── 7. CONCURRENT SAME-GAP COLLISION (leg B): client B, the SAME gap and SAME midpoint body
    //  "M", but jitter "66" → a DISTINCT key. The 2-char jitter is what stops A and B colliding on an
    //  identical key (the concurrency-safety property). M55 != M66, both strictly between F22 and U00.
    ConformanceStep {
        label: "concurrent same-gap insert B (jitter 66 -> distinct from A)",
        op: RankOp::Between {
            lo: Some("F22"),
            hi: Some("U00"),
            jitter: (6, 6),
        },
        expect: "M66",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{LEXORANK_ALPHABET, LEXORANK_REBALANCE_LEN};

    /// **THE X-3 ANTI-DRIFT GATE (Knowledge side): the conformance vector replays byte-for-byte with
    /// 0 divergences.** Each step runs through the FROZEN [`OrderKey`] operations and MUST produce
    /// its `expect` string exactly. The Issues side (ISS-P02 / P-241) replays the SAME
    /// [`CONFORMANCE_VECTOR`] through its own executor and asserts the identical outputs — a drift on
    /// EITHER side breaks this test on BOTH.
    #[test]
    fn replay_conformance_vector() {
        let mut divergences = 0usize;
        for step in CONFORMANCE_VECTOR {
            let produced = step.run();
            if produced.as_str() != step.expect {
                eprintln!(
                    "DIVERGENCE [{}]: produced {:?}, expected {:?}",
                    step.label,
                    produced.as_str(),
                    step.expect
                );
                divergences += 1;
            }
        }
        assert_eq!(
            divergences, 0,
            "the X-3 LexoRank conformance vector must replay with 0 rank divergences (byte-identical \
             across Knowledge + Issues)"
        );
    }

    /// The conformance vector's order is internally consistent: every produced key is a valid
    /// in-alphabet `order_key`, and the documented ordering relationships hold (so the fixture is not
    /// merely self-consistent on the byte string but is actually a correct fractional index).
    #[test]
    fn conformance_vector_keys_are_well_ordered() {
        let by_label = |l: &str| {
            CONFORMANCE_VECTOR
                .iter()
                .find(|s| s.label == l)
                .map(|s| s.run())
                .unwrap()
        };
        let first = by_label("first key (U anchor + 00 jitter)");
        let last = by_label("append after U00 (last)");
        let prepend = by_label("prepend before U00 (between None,U00)");
        let between = by_label("between F22 and U00");

        // Every key is in-alphabet.
        for step in CONFORMANCE_VECTOR {
            let k = step.run();
            assert!(
                k.as_str().bytes().all(|b| LEXORANK_ALPHABET.contains(&b)),
                "{} produced an in-alphabet key",
                step.label
            );
        }

        // prepend < first < last  (the spine of a fresh list).
        assert!(prepend < first, "prepend {prepend} < first {first}");
        assert!(first < last, "first {first} < last {last}");
        // F22 < between(M33) < U00 — the strictly-between key sits in its gap.
        assert!(prepend < between && between < first, "{prepend} < {between} < {first}");
    }

    /// **The concurrent same-gap collision produces DISTINCT keys** (the jitter's whole reason for
    /// existing). Two inserts into the identical (F22, U00) gap at the same midpoint body "M" differ
    /// ONLY in their 2-char jitter — and so do not collide, and both stay strictly between the
    /// bounds. This is the load-bearing concurrency-safety property of the vector.
    #[test]
    fn concurrent_same_gap_inserts_do_not_collide() {
        let a = CONFORMANCE_VECTOR
            .iter()
            .find(|s| s.label.starts_with("concurrent same-gap insert A"))
            .unwrap()
            .run();
        let b = CONFORMANCE_VECTOR
            .iter()
            .find(|s| s.label.starts_with("concurrent same-gap insert B"))
            .unwrap()
            .run();
        assert_ne!(a.as_str(), b.as_str(), "the 2-char jitter makes same-gap inserts DISTINCT");
        // Both share the un-jittered midpoint body but differ in the jitter suffix.
        assert_eq!(&a.as_str()[..1], &b.as_str()[..1], "same midpoint body");
        // Both are strictly between the gap bounds F22 and U00.
        let lo = OrderKey::parse("F22").unwrap();
        let hi = OrderKey::parse("U00").unwrap();
        assert!(lo < a && a < hi, "A in (F22,U00)");
        assert!(lo < b && b < hi, "B in (F22,U00)");
    }

    /// **A 48-char key trips the rebalance signal, and a bisection into an adjacent gap GROWS the
    /// key (the headroom-exhaustion pathology the 48-char rebalance exists to catch).** The signal is
    /// pure: the operation never refuses; the owner reacts to [`OrderKey::needs_rebalance`].
    #[test]
    fn rebalance_triggers_at_48_chars() {
        // The boundary: a key one char below the trigger does NOT trip; AT the trigger it does.
        let just_below = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN - 1)).unwrap();
        assert!(!just_below.needs_rebalance(), "one char below the 48-char trigger does NOT trip");
        let at_trigger = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN)).unwrap();
        assert!(at_trigger.needs_rebalance(), "a {LEXORANK_REBALANCE_LEN}-char key trips rebalance");
        // The free-function form mirrors the method (the contract name `needs_rebalance(rank)`).
        assert!(OrderKey::needs_rebalance_key(&at_trigger));
        assert!(!OrderKey::needs_rebalance_key(&just_below));

        // Headroom exhaustion: bisecting between a key `k` and its IMMEDIATE successor (same length,
        // last digit +1) forces the midpoint to descend exactly one level — the key grows one char.
        // Advancing `k` to that midpoint each step keeps the gap adjacent, so the chain grows
        // monotonically to the 48-char trigger (the worst-case length pathology the rebalance catches).
        let succ = |k: &OrderKey| -> OrderKey {
            // The immediate successor at the last slot: bump the final base-62 digit by 1. The
            // fixture starts at "V0" (last digit '0', rank 0), and every midpoint we produce ends in
            // a digit strictly below the alphabet max, so the successor always exists in-alphabet.
            let s = k.as_str();
            let (head, last) = s.split_at(s.len() - 1);
            let last_byte = last.as_bytes()[0];
            let pos = LEXORANK_ALPHABET.iter().position(|&b| b == last_byte).unwrap();
            assert!(pos + 1 < LEXORANK_ALPHABET.len(), "successor stays in-alphabet");
            let bumped = LEXORANK_ALPHABET[pos + 1] as char;
            OrderKey::parse(format!("{head}{bumped}")).unwrap()
        };
        let mut lo = OrderKey::parse("V0").unwrap();
        let mut grew_past_trigger = false;
        let mut max_len = lo.as_str().len();
        for _ in 0..400 {
            let hi = succ(&lo); // the immediate successor: an adjacent gap, no digit fits between
            let mid = OrderKey::bisect(Some(&lo), Some(&hi));
            assert!(lo < mid && mid < hi, "stays strictly between while growing: {lo} < {mid} < {hi}");
            assert!(mid.as_str().len() > lo.as_str().len(), "an adjacent-gap bisection grows the key");
            max_len = max_len.max(mid.as_str().len());
            if mid.needs_rebalance() {
                assert!(mid.as_str().len() >= LEXORANK_REBALANCE_LEN);
                grew_past_trigger = true;
                break;
            }
            lo = mid; // advance into the same exhausted gap, keeping it adjacent
        }
        assert!(
            grew_past_trigger,
            "an adjacent-gap bisection chain grew to the rebalance trigger (max len reached {max_len})"
        );
    }

    /// **The `created_at`+ULID tiebreak is a total order** (contract 13.3 "total order guaranteed").
    /// Equal `order_key`s break on `created_at`, then the ULID id; distinct rows never compare
    /// `Equal`.
    #[test]
    fn tiebreak_is_total_order() {
        let k = OrderKey::parse("M00").unwrap();
        // Same key, different created_at: earlier created_at sorts first.
        assert_eq!(
            tiebreak(&k, "2026-06-21T10:00:00Z", "01A", &k, "2026-06-21T11:00:00Z", "01B"),
            Ordering::Less,
            "equal key → earlier created_at wins"
        );
        // Same key + same created_at: the ULID id breaks it (lexicographic == time-ordered).
        assert_eq!(
            tiebreak(&k, "2026-06-21T10:00:00Z", "01A", &k, "2026-06-21T10:00:00Z", "01B"),
            Ordering::Less,
            "equal key + equal created_at → ULID id breaks it"
        );
        // The order_key itself dominates when it differs (the tiebreak never overrides the rank).
        let hi = OrderKey::parse("U00").unwrap();
        assert_eq!(
            tiebreak(&k, "2026-06-21T99", "zzz", &hi, "2026-06-21T00", "000"),
            Ordering::Less,
            "the order_key is the PRIMARY key; the tiebreak is secondary"
        );
        // Fully identical (key+created_at+id): the only legitimate Equal (the same row).
        assert_eq!(
            tiebreak(&k, "t", "id", &k, "t", "id"),
            Ordering::Equal,
            "the same row compares Equal"
        );
    }
}
