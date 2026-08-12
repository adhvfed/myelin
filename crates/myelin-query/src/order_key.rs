use crate::field::{Jitter, OrderKey};
use std::cmp::Ordering;

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

#[derive(Clone, Copy, Debug)]
pub struct ConformanceStep {
    pub label: &'static str,
    pub op: RankOp,
    pub expect: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub enum RankOp {
    First {
        jitter: (usize, usize),
    },
    Last {
        after: Option<&'static str>,
        jitter: (usize, usize),
    },
    Between {
        lo: Option<&'static str>,
        hi: Option<&'static str>,
        jitter: (usize, usize),
    },
}

impl ConformanceStep {
    pub fn run(&self) -> OrderKey {
        match self.op {
            RankOp::First { jitter } => OrderKey::rank_first(jit(jitter)),
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

fn parse_key(s: &'static str) -> OrderKey {
    OrderKey::parse(s).expect("conformance-vector bound is a valid base-62 order_key")
}

fn jit((a, b): (usize, usize)) -> Jitter {
    Jitter::from_ranks(a, b).expect("conformance-vector jitter ranks are in 0..62")
}

pub const CONFORMANCE_VECTOR: &[ConformanceStep] = &[
    ConformanceStep {
        label: "first key (U anchor + 00 jitter)",
        op: RankOp::First { jitter: (0, 0) },
        expect: "U00",
    },
    ConformanceStep {
        label: "append after U00 (last)",
        op: RankOp::Last {
            after: Some("U00"),
            jitter: (1, 0),
        },
        expect: "k10",
    },
    ConformanceStep {
        label: "prepend before U00 (between None,U00)",
        op: RankOp::Between {
            lo: None,
            hi: Some("U00"),
            jitter: (2, 2),
        },
        expect: "F22",
    },
    ConformanceStep {
        label: "between F22 and U00",
        op: RankOp::Between {
            lo: Some("F22"),
            hi: Some("U00"),
            jitter: (3, 3),
        },
        expect: "M33",
    },
    ConformanceStep {
        label: "adjacent descent between V and W (key grows one char)",
        op: RankOp::Between {
            lo: Some("V"),
            hi: Some("W"),
            jitter: (4, 4),
        },
        expect: "VV44",
    },
    ConformanceStep {
        label: "concurrent same-gap insert A (jitter 55)",
        op: RankOp::Between {
            lo: Some("F22"),
            hi: Some("U00"),
            jitter: (5, 5),
        },
        expect: "M55",
    },
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

        for step in CONFORMANCE_VECTOR {
            let k = step.run();
            assert!(
                k.as_str().bytes().all(|b| LEXORANK_ALPHABET.contains(&b)),
                "{} produced an in-alphabet key",
                step.label
            );
        }

        assert!(prepend < first, "prepend {prepend} < first {first}");
        assert!(first < last, "first {first} < last {last}");
        assert!(
            prepend < between && between < first,
            "{prepend} < {between} < {first}"
        );
    }

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
        assert_ne!(
            a.as_str(),
            b.as_str(),
            "the 2-char jitter makes same-gap inserts DISTINCT"
        );
        assert_eq!(&a.as_str()[..1], &b.as_str()[..1], "same midpoint body");
        let lo = OrderKey::parse("F22").unwrap();
        let hi = OrderKey::parse("U00").unwrap();
        assert!(lo < a && a < hi, "A in (F22,U00)");
        assert!(lo < b && b < hi, "B in (F22,U00)");
    }

    #[test]
    fn rebalance_triggers_at_48_chars() {
        let just_below = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN - 1)).unwrap();
        assert!(
            !just_below.needs_rebalance(),
            "one char below the 48-char trigger does NOT trip"
        );
        let at_trigger = OrderKey::parse("V".repeat(LEXORANK_REBALANCE_LEN)).unwrap();
        assert!(
            at_trigger.needs_rebalance(),
            "a {LEXORANK_REBALANCE_LEN}-char key trips rebalance"
        );
        assert!(OrderKey::needs_rebalance_key(&at_trigger));
        assert!(!OrderKey::needs_rebalance_key(&just_below));

        let succ = |k: &OrderKey| -> OrderKey {
            let s = k.as_str();
            let (head, last) = s.split_at(s.len() - 1);
            let last_byte = last.as_bytes()[0];
            let pos = LEXORANK_ALPHABET
                .iter()
                .position(|&b| b == last_byte)
                .unwrap();
            assert!(
                pos + 1 < LEXORANK_ALPHABET.len(),
                "successor stays in-alphabet"
            );
            let bumped = LEXORANK_ALPHABET[pos + 1] as char;
            OrderKey::parse(format!("{head}{bumped}")).unwrap()
        };
        let mut lo = OrderKey::parse("V0").unwrap();
        let mut grew_past_trigger = false;
        let mut max_len = lo.as_str().len();
        for _ in 0..400 {
            let hi = succ(&lo);
            let mid = OrderKey::bisect(Some(&lo), Some(&hi));
            assert!(
                lo < mid && mid < hi,
                "stays strictly between while growing: {lo} < {mid} < {hi}"
            );
            assert!(
                mid.as_str().len() > lo.as_str().len(),
                "an adjacent-gap bisection grows the key"
            );
            max_len = max_len.max(mid.as_str().len());
            if mid.needs_rebalance() {
                assert!(mid.as_str().len() >= LEXORANK_REBALANCE_LEN);
                grew_past_trigger = true;
                break;
            }
            lo = mid;
        }
        assert!(
            grew_past_trigger,
            "an adjacent-gap bisection chain grew to the rebalance trigger (max len reached {max_len})"
        );
    }

    #[test]
    fn tiebreak_is_total_order() {
        let k = OrderKey::parse("M00").unwrap();
        assert_eq!(
            tiebreak(
                &k,
                "2026-06-21T10:00:00Z",
                "01A",
                &k,
                "2026-06-21T11:00:00Z",
                "01B"
            ),
            Ordering::Less,
            "equal key → earlier created_at wins"
        );
        assert_eq!(
            tiebreak(
                &k,
                "2026-06-21T10:00:00Z",
                "01A",
                &k,
                "2026-06-21T10:00:00Z",
                "01B"
            ),
            Ordering::Less,
            "equal key + equal created_at → ULID id breaks it"
        );
        let hi = OrderKey::parse("U00").unwrap();
        assert_eq!(
            tiebreak(&k, "2026-06-21T99", "zzz", &hi, "2026-06-21T00", "000"),
            Ordering::Less,
            "the order_key is the PRIMARY key; the tiebreak is secondary"
        );
        assert_eq!(
            tiebreak(&k, "t", "id", &k, "t", "id"),
            Ordering::Equal,
            "the same row compares Equal"
        );
    }
}
