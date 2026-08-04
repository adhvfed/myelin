//! # `MicroUsd` — the one money newtype for the whole platform.
//!
//! A single integer money unit shared by BOTH the prepaid agent wallet ([`crate::agent_wallet`])
//! and the reserve/settle cost ledger ([`crate::reserve_settle`]). Previously these were two
//! distinct newtypes (`MicroUsd` for the wallet, `MinorUnits` for the ledger); they are now unified
//! into `MicroUsd` so a value can flow from the wallet into the ledger's `available` param without a
//! type conversion, and there is exactly ONE money type to reason about.
//!
//! ## THE UNIT — MICRO-DOLLARS (`1 unit = $0.000001`), a `u64`
//! Every amount is an integer count of **micro-dollars**: one unit is one millionth of a US dollar,
//! so **$1.00 = 1_000_000 units** and **1 cent = 10_000 units**. This sub-cent scale is deliberate: a
//! hosted-agent task can cost a small fraction of a cent. A `u64` so the arithmetic is exact and a
//! fractional amount is **unrepresentable** — you cannot construct a fractional balance or cost. All
//! arithmetic is checked (`checked_add`/`checked_sub`) — an overflow is a loud `None` the caller turns
//! into a typed error, never a silent wrap.

/// An integer **micro-dollars** amount — the platform's single money unit
/// (`1 MicroUsd = $0.000001`; `$1.00 = 1_000_000`; `1 cent = 10_000`). A `u64` so the arithmetic is
/// exact and a fractional amount is **unrepresentable**. All money arithmetic is checked
/// (`checked_add`/`checked_sub`) — an overflow is a loud typed error, never a silent wrap.
///
/// This is the unit of BOTH the prepaid agent wallet ([`crate::agent_wallet::AgentWallet`]) and the
/// reserve/settle cost ledger ([`crate::reserve_settle::CostLedger`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MicroUsd(pub u64);

impl MicroUsd {
    /// Zero micro-dollars (the additive identity — an empty wallet, a zero movement, a zero-cost
    /// metered unit).
    pub const ZERO: MicroUsd = MicroUsd(0);

    /// Checked addition — `None` on `u64` overflow (the loud-not-silent rule; the caller turns it
    /// into a typed error).
    pub fn checked_add(self, other: MicroUsd) -> Option<MicroUsd> {
        self.0.checked_add(other.0).map(MicroUsd)
    }

    /// Checked subtraction — `None` if it would go negative (a debit/refund can never drive a balance
    /// below zero or make a reservation owe money).
    pub fn checked_sub(self, other: MicroUsd) -> Option<MicroUsd> {
        self.0.checked_sub(other.0).map(MicroUsd)
    }

    /// Whether this amount fits Postgres `bigint` (`i64`) losslessly (`0..=i64::MAX`). Postgres
    /// `bigint` is signed, so a `u64` above `i64::MAX` cannot round-trip — the wallet refuses it
    /// fail-closed rather than corrupting a balance via two's-complement reinterpretation.
    pub fn fits_bigint(self) -> bool {
        self.0 <= i64::MAX as u64
    }

    /// The `bigint` (`i64`) wire value, or `None` if it does not fit (`> i64::MAX`).
    pub(crate) fn to_bigint(self) -> Option<i64> {
        if self.fits_bigint() {
            Some(self.0 as i64)
        } else {
            None
        }
    }

    /// Rebuild a `MicroUsd` from a `bigint` read back from the DB. The `balance_micro >= 0` /
    /// `amount_micro >= 0` CHECK constraints guarantee the stored value is non-negative, so the
    /// `i64 → u64` widening is lossless.
    pub(crate) fn from_bigint(v: i64) -> MicroUsd {
        MicroUsd(v as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The micro-dollar unit is checked: an overflow is a loud `None`, never a silent wrap.
    #[test]
    fn micro_usd_arithmetic_is_checked() {
        assert_eq!(MicroUsd(u64::MAX).checked_add(MicroUsd(1)), None);
        assert_eq!(MicroUsd(5).checked_sub(MicroUsd(10)), None);
        assert_eq!(MicroUsd(10).checked_sub(MicroUsd(10)), Some(MicroUsd::ZERO));
        assert_eq!(
            MicroUsd(1_000_000).checked_add(MicroUsd(10_000)),
            Some(MicroUsd(1_010_000)),
            "$1.00 + 1 cent = 1_010_000 micro-USD"
        );
        assert_eq!(
            MicroUsd(u64::MAX).checked_add(MicroUsd(0)),
            Some(MicroUsd(u64::MAX))
        );
    }

    /// The `bigint` (`i64`) fit boundary: `i64::MAX` fits, `i64::MAX + 1` does not (fail-closed).
    #[test]
    fn bigint_fit_boundary_is_exact() {
        let max = MicroUsd(i64::MAX as u64);
        assert!(max.fits_bigint(), "i64::MAX micro-USD fits bigint");
        assert_eq!(max.to_bigint(), Some(i64::MAX));

        let over = MicroUsd(i64::MAX as u64 + 1);
        assert!(!over.fits_bigint(), "i64::MAX + 1 does NOT fit bigint");
        assert_eq!(over.to_bigint(), None);

        // A value read back from a stored non-negative bigint round-trips losslessly.
        assert_eq!(MicroUsd::from_bigint(i64::MAX), MicroUsd(i64::MAX as u64));
        assert_eq!(MicroUsd::from_bigint(0), MicroUsd::ZERO);
    }
}
