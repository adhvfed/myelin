use myelin_agent::TokenUsage;
use myelin_storage::agent_wallet::MicroUsd;

const TOKENS_PER_MTOK: u64 = 1_000_000;

const MARKUP_NUMERATOR: u64 = 2;
const MARKUP_DENOMINATOR: u64 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelRates {
    pub input_per_mtok: u64,
    pub cached_input_per_mtok: u64,
    pub output_per_mtok: u64,
}

pub const LUNA_RATES: ModelRates = ModelRates {
    input_per_mtok: 200_000,
    cached_input_per_mtok: 20_000,
    output_per_mtok: 1_200_000,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Priced {
    pub wholesale: MicroUsd,
    pub markup: MicroUsd,
}

impl Priced {
    pub const ZERO: Priced = Priced {
        wholesale: MicroUsd::ZERO,
        markup: MicroUsd::ZERO,
    };

    pub fn total(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceError {
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

pub fn price(usage: &TokenUsage, rates: &ModelRates) -> Result<Priced, PriceError> {
    let (input, cached_input, output) = match usage {
        TokenUsage::Reported {
            input,
            cached_input,
            output,
        } => (*input, *cached_input, *output),
        TokenUsage::NotReported => return Ok(Priced::ZERO),
    };

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

    let wholesale = wholesale_sum / TOKENS_PER_MTOK;

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
        assert_eq!(input_only.wholesale, MicroUsd(100_000));
        assert_eq!(cached_only.wholesale, MicroUsd(10_000));
        assert!(
            cached_only.wholesale.0 < input_only.wholesale.0,
            "the cache tier is cheaper (10_000 < 100_000)"
        );
    }

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
        assert_eq!(priced.wholesale, MicroUsd(1_400_000_000_000));
    }

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

    #[test]
    fn not_reported_prices_to_zero() {
        let priced = price(&TokenUsage::NotReported, &LUNA_RATES).unwrap();
        assert_eq!(priced, Priced::ZERO);
        assert_eq!(priced.wholesale, MicroUsd::ZERO);
        assert_eq!(priced.markup, MicroUsd::ZERO);
        assert_eq!(priced.total(), Some(MicroUsd::ZERO));
    }

    #[test]
    fn markup_rounds_half_up() {
        let p = price(
            &TokenUsage::Reported {
                input: 0,
                cached_input: 0,
                output: 0,
            },
            &LUNA_RATES,
        )
        .unwrap();
        assert_eq!(p, Priced::ZERO);

        let unit = ModelRates {
            input_per_mtok: TOKENS_PER_MTOK,
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
