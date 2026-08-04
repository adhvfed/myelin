//! # `metering` — v1 token PRICING: raw token counts → a micro-dollar charge (pure, DB-free)
//!
//! **The v1 metering design (deliberately NON-DISRUPTIVE).** This module is the PURE half of the
//! hosted-agent token meter: it turns a turn's raw provider token usage
//! ([`myelin_agent::TokenUsage`]) into a [`MicroUsd`] charge (a `wholesale` cost + a `markup`
//! platform cut). It holds NO wallet, does NO I/O, and touches NEITHER the reserve/settle
//! [`CostLedger`](myelin_storage::reserve_settle::CostLedger) NOR the [`AgentRunGate`] — those stay
//! exactly as they are. The wallet DEBIT that consumes this price is the driving loop's job
//! ([`crate::skeleton::SkeletonAgent::handle_run`]); the metering is a NEW layer LAYERED ON TOP of
//! the untouched nominal reserve/settle gate.
//!
//! ## The unit — MICRO-DOLLARS (`1 unit = $0.000001`), the wallet's frozen unit
//! Every amount here is [`MicroUsd`] (a `u64`; `$1.00 = 1_000_000`), the SAME unit the durable
//! [`AgentWallet`](myelin_storage::agent_wallet::AgentWallet) debits AND the reserve/settle cost
//! ledger uses — one money type across the platform, so a price composes with a debit or a
//! reservation with no unit conversion.
//!
//! ## The pricing math (integer, checked, documented rounding)
//! Rates are quoted per **million tokens** (per-Mtok) as integer micro-dollars
//! ([`ModelRates`]). For a reported turn:
//!
//! ```text
//! wholesale_micro = (input*input_rate + cached*cached_rate + output*output_rate) / 1_000_000
//! markup_micro    = round( wholesale_micro * 2 / 100 )        // the ~2% platform cut
//! ```
//!
//! - **wholesale ROUNDS DOWN** (integer division): the sub-token remainder of a single call is
//!   dropped. This is acceptable because a run accumulates many tokens across many turns, and the
//!   drop is at most `<1` micro-dollar per turn (a millionth of a dollar). Carrying the remainder
//!   across turns to erase even that systematic under-bill is a named follow-on — v1 keeps it simple
//!   and never OVER-bills.
//! - **markup ROUNDS HALF-UP** (`(wholesale*2 + 50) / 100`): the nearest micro-dollar, ties up. At
//!   micro-dollar scale a half-unit is a two-millionths-of-a-dollar tie.
//! - All arithmetic is CHECKED. An overflow is a LOUD [`PriceError::Overflow`], NEVER a silent wrap
//!   (financial correctness — the `handle_run` caller turns it into a run-abort, not a mis-charge).
//! - [`TokenUsage::NotReported`] prices to ZERO — but a zero price is NOT a sanctioned charge: it is
//!   the caller's FAIL-CLOSED signal (a paid call the provider did not meter must abort the run, not
//!   bill $0). Pricing never fabricates a count.
//!
//! ## Follow-ons (named, NOT built here)
//! - **Wallet reservations** (holding funds at dispatch like the reserve/settle gate does for
//!   `MicroUsd`) — v1 debits per-turn after the fact instead.
//! - **A precise next-call estimate** (a `max_tokens`-based pre-call cap) — v1's pre-step cap is a
//!   coarse `balance > floor` gate (see `handle_run`); the two together bound overspend to one turn.
//! - **Anthropic (and other vendor) rates** — the [`ModelRates`] shape is vendor-neutral; only
//!   [`LUNA_RATES`] is provided today. A new vendor is a new named constant, no code change here.

use myelin_agent::TokenUsage;
use myelin_storage::agent_wallet::MicroUsd;

/// The number of tokens a per-Mtok rate is quoted over (one million). The wholesale sum is divided by
/// this to bring `tokens * (micro-USD per Mtok)` back to micro-USD.
const TOKENS_PER_MTOK: u64 = 1_000_000;

/// The platform markup numerator/denominator — a `2 / 100` (~2%) cut over the wholesale cost.
const MARKUP_NUMERATOR: u64 = 2;
const MARKUP_DENOMINATOR: u64 = 100;

/// **Per-million-token micro-dollar rates for one model** (the vendor-neutral pricing shape). Each
/// field is integer micro-dollars charged per 1_000_000 tokens of that tier, so pricing stays exact
/// integer arithmetic (no float money). A cached-input tier cheaper than the standard input tier is
/// the whole point of the prompt cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelRates {
    /// Micro-USD per Mtok of NON-cached prompt (standard input) tokens.
    pub input_per_mtok: u64,
    /// Micro-USD per Mtok of CACHED prompt (cache-hit) tokens — the cheaper tier.
    pub cached_input_per_mtok: u64,
    /// Micro-USD per Mtok of completion (output) tokens.
    pub output_per_mtok: u64,
}

/// **The Luna (`gpt-5.6-luna`) per-Mtok rates.** Input $0.20/Mtok, cached-input $0.02/Mtok, output
/// $1.20/Mtok — expressed as integer micro-dollars per 1_000_000 tokens (`$0.20 = 200_000` micro-USD,
/// etc.). Anthropic (and any other vendor) rates slot in as a sibling `const` of the same shape.
pub const LUNA_RATES: ModelRates = ModelRates {
    input_per_mtok: 200_000,
    cached_input_per_mtok: 20_000,
    output_per_mtok: 1_200_000,
};

/// **A priced turn: the wholesale token cost + the platform markup, in micro-dollars.** The charge
/// the wallet debits is [`Priced::total`] (`wholesale + markup`). Kept as two fields so the split is
/// observable (the wholesale is the provider cost; the markup is the platform's cut).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Priced {
    /// The wholesale token cost (the provider's cost, rounded down per call).
    pub wholesale: MicroUsd,
    /// The platform markup (the ~2% cut, rounded half-up).
    pub markup: MicroUsd,
}

impl Priced {
    /// A zero price (a `NotReported` turn, or the additive identity).
    pub const ZERO: Priced = Priced {
        wholesale: MicroUsd::ZERO,
        markup: MicroUsd::ZERO,
    };

    /// **The total charge the wallet debits** (`wholesale + markup`) — checked, `None` on the (only
    /// astronomically reachable) `u64` overflow so the caller can fail LOUD rather than wrap.
    pub fn total(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

/// **A loud pricing refusal — an overflow in the checked micro-dollar arithmetic.** Never a silent
/// wrap: the `handle_run` caller turns this into a run-abort (a mis-price on a financial op must be
/// loud, EI-01 §2). Only reachable at astronomically large token counts (`> ~9e13` output tokens).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceError {
    /// A `checked_mul`/`checked_add` in the wholesale or markup computation overflowed `u64`.
    Overflow(&'static str),
}

impl core::fmt::Display for PriceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PriceError::Overflow(where_) => write!(
                f,
                "token pricing overflowed u64 at {where_} (loud, never a silent wrap)"
            ),
        }
    }
}

impl std::error::Error for PriceError {}

/// **Price ONE turn's raw token usage into a micro-dollar [`Priced`] (pure, checked).** See the
/// module doc for the exact math + rounding. [`TokenUsage::NotReported`] → [`Priced::ZERO`] (the
/// caller's fail-closed signal, NOT a sanctioned $0 charge). A checked-arithmetic overflow →
/// [`PriceError::Overflow`] (loud), never a wrap.
pub fn price(usage: &TokenUsage, rates: &ModelRates) -> Result<Priced, PriceError> {
    let (input, cached_input, output) = match usage {
        TokenUsage::Reported {
            input,
            cached_input,
            output,
        } => (*input, *cached_input, *output),
        // A provider that did not report usage prices to ZERO — the caller reads this as its
        // fail-closed signal (never bill an unmetered call), it is not a sanctioned charge.
        TokenUsage::NotReported => return Ok(Priced::ZERO),
    };

    // wholesale_sum = input*input_rate + cached*cached_rate + output*output_rate  (checked; the units
    // are `tokens * micro-USD-per-Mtok`, brought back to micro-USD by the /1_000_000 below).
    let input_cost = input
        .checked_mul(rates.input_per_mtok)
        .ok_or(PriceError::Overflow("input * input_rate"))?;
    let cached_cost = cached_input
        .checked_mul(rates.cached_input_per_mtok)
        .ok_or(PriceError::Overflow("cached_input * cached_rate"))?;
    let output_cost = output
        .checked_mul(rates.output_per_mtok)
        .ok_or(PriceError::Overflow("output * output_rate"))?;
    let wholesale_sum = input_cost
        .checked_add(cached_cost)
        .and_then(|s| s.checked_add(output_cost))
        .ok_or(PriceError::Overflow("Σ tier costs"))?;

    // wholesale ROUNDS DOWN (the sub-token remainder of this call is dropped — documented, never
    // over-bills; the run accumulates many tokens so the drop is at most <1 micro-USD per turn).
    let wholesale = wholesale_sum / TOKENS_PER_MTOK;

    // markup = round(wholesale * 2 / 100), ROUND HALF-UP: (wholesale*2 + 50) / 100 (checked).
    let markup = wholesale
        .checked_mul(MARKUP_NUMERATOR)
        .and_then(|m| m.checked_add(MARKUP_DENOMINATOR / 2))
        .map(|m| m / MARKUP_DENOMINATOR)
        .ok_or(PriceError::Overflow("markup rounding"))?;

    Ok(Priced {
        wholesale: MicroUsd(wholesale),
        markup: MicroUsd(markup),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Known usage → known wholesale + markup on the Luna rates.** One Mtok of each tier: input
    /// $0.20 + cached $0.02 + output $1.20 = $1.42 wholesale (1_420_000 micro-USD); markup = 2% =
    /// 28_400 micro-USD.
    #[test]
    fn luna_known_usage_prices_to_known_micro_dollars() {
        let priced = price(
            &TokenUsage::Reported {
                input: 1_000_000,
                cached_input: 1_000_000,
                output: 1_000_000,
            },
            &LUNA_RATES,
        )
        .expect("prices without overflow");
        assert_eq!(
            priced.wholesale,
            MicroUsd(1_420_000),
            "0.20 + 0.02 + 1.20 = $1.42 wholesale"
        );
        assert_eq!(
            priced.markup,
            MicroUsd(28_400),
            "2% of 1_420_000 = 28_400 micro-USD"
        );
        assert_eq!(
            priced.total(),
            Some(MicroUsd(1_448_400)),
            "total = wholesale + markup"
        );
    }

    /// **The cached-input tier is CHEAPER than the standard input tier for the same token count.**
    /// The whole point of the prompt cache: identical counts, cached-only < input-only.
    #[test]
    fn cached_tier_is_cheaper_than_standard_input() {
        let n = 500_000u64;
        let input_only = price(
            &TokenUsage::Reported {
                input: n,
                cached_input: 0,
                output: 0,
            },
            &LUNA_RATES,
        )
        .unwrap();
        let cached_only = price(
            &TokenUsage::Reported {
                input: 0,
                cached_input: n,
                output: 0,
            },
            &LUNA_RATES,
        )
        .unwrap();
        // input: 500_000 * 200_000 / 1e6 = 100_000 ; cached: 500_000 * 20_000 / 1e6 = 10_000.
        assert_eq!(input_only.wholesale, MicroUsd(100_000));
        assert_eq!(cached_only.wholesale, MicroUsd(10_000));
        assert!(
            cached_only.wholesale.0 < input_only.wholesale.0,
            "the cache tier is cheaper (10_000 < 100_000)"
        );
    }

    /// **A large token count does NOT overflow** (checked arithmetic holds well past any real run).
    /// 1e12 output tokens: 1e12 * 1_200_000 = 1.2e18 < u64::MAX (~1.8e19), /1e6 = 1.2e12 wholesale.
    #[test]
    fn large_count_does_not_overflow() {
        let priced = price(
            &TokenUsage::Reported {
                input: 1_000_000_000_000,
                cached_input: 0,
                output: 1_000_000_000_000,
            },
            &LUNA_RATES,
        )
        .expect("a large-but-real count prices without overflow");
        // input 1e12 * 200_000 /1e6 = 2e11 ; output 1e12 * 1_200_000 /1e6 = 1.2e12 ; Σ = 1.4e12.
        assert_eq!(priced.wholesale, MicroUsd(1_400_000_000_000));
    }

    /// **An astronomically large count overflows LOUD (never wraps).** `u64::MAX` output tokens times
    /// the output rate cannot fit `u64` — a loud [`PriceError::Overflow`], not a silent wrap.
    #[test]
    fn astronomical_count_overflows_loud() {
        let err = price(
            &TokenUsage::Reported {
                input: 0,
                cached_input: 0,
                output: u64::MAX,
            },
            &LUNA_RATES,
        )
        .expect_err("u64::MAX output tokens overflows the checked multiply");
        assert!(matches!(err, PriceError::Overflow(_)));
        assert!(!err.to_string().is_empty(), "the overflow is loud");
    }

    /// **`NotReported` prices to ZERO** (the caller's fail-closed signal — never a fabricated count).
    #[test]
    fn not_reported_prices_to_zero() {
        let priced = price(&TokenUsage::NotReported, &LUNA_RATES).unwrap();
        assert_eq!(priced, Priced::ZERO);
        assert_eq!(priced.wholesale, MicroUsd::ZERO);
        assert_eq!(priced.markup, MicroUsd::ZERO);
        assert_eq!(priced.total(), Some(MicroUsd::ZERO));
    }

    /// **The markup rounds HALF-UP** (ties round up; below-half rounds down) — the documented
    /// `(wholesale*2 + 50)/100` rule, checked at the boundaries.
    #[test]
    fn markup_rounds_half_up() {
        // wholesale 125 → 125*2/100 = 2.5 → round-half-up = 3.
        let p = price(
            &TokenUsage::Reported {
                input: 0,
                cached_input: 0,
                // output tokens t: wholesale = t*1_200_000/1e6 = t*1.2 (integer). Pick wholesale by
                // choosing output so wholesale lands exactly: output 125 -> 150 wholesale? Instead
                // price a synthetic rate to hit wholesale 125 exactly.
                output: 0,
            },
            &LUNA_RATES,
        )
        .unwrap();
        assert_eq!(p, Priced::ZERO);

        // Use a unit rate so wholesale == token count, to exercise the rounding cleanly.
        let unit = ModelRates {
            input_per_mtok: TOKENS_PER_MTOK, // 1 micro-USD per token → wholesale == input count.
            cached_input_per_mtok: 0,
            output_per_mtok: 0,
        };
        let at_half = price(
            &TokenUsage::Reported {
                input: 125,
                cached_input: 0,
                output: 0,
            },
            &unit,
        )
        .unwrap();
        assert_eq!(at_half.wholesale, MicroUsd(125));
        assert_eq!(at_half.markup, MicroUsd(3), "2.5 rounds HALF-UP to 3");

        let below_half = price(
            &TokenUsage::Reported {
                input: 124,
                cached_input: 0,
                output: 0,
            },
            &unit,
        )
        .unwrap();
        assert_eq!(below_half.wholesale, MicroUsd(124));
        assert_eq!(below_half.markup, MicroUsd(2), "2.48 rounds down to 2");

        let exact = price(
            &TokenUsage::Reported {
                input: 100,
                cached_input: 0,
                output: 0,
            },
            &unit,
        )
        .unwrap();
        assert_eq!(exact.markup, MicroUsd(2), "2.0 stays 2");
    }

    /// **The wholesale ROUNDS DOWN** (the documented sub-token remainder drop — never over-bills).
    /// A single input token on the Luna rate: 1 * 200_000 / 1_000_000 = 0.2 → 0 (dropped).
    #[test]
    fn wholesale_rounds_down_sub_token_remainder() {
        let priced = price(
            &TokenUsage::Reported {
                input: 1,
                cached_input: 0,
                output: 0,
            },
            &LUNA_RATES,
        )
        .unwrap();
        assert_eq!(priced.wholesale, MicroUsd::ZERO, "0.2 micro-USD rounds down to 0");
        // A single OUTPUT token: 1 * 1_200_000 / 1e6 = 1.2 → 1 (dropped 0.2).
        let out1 = price(
            &TokenUsage::Reported {
                input: 0,
                cached_input: 0,
                output: 1,
            },
            &LUNA_RATES,
        )
        .unwrap();
        assert_eq!(out1.wholesale, MicroUsd(1), "1.2 micro-USD rounds down to 1");
    }
}
