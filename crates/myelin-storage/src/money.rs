#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MicroUsd(pub u64);

impl MicroUsd {
    pub const ZERO: MicroUsd = MicroUsd(0);

    pub fn checked_add(self, other: MicroUsd) -> Option<MicroUsd> {
        self.0.checked_add(other.0).map(MicroUsd)
    }

    pub fn checked_sub(self, other: MicroUsd) -> Option<MicroUsd> {
        self.0.checked_sub(other.0).map(MicroUsd)
    }

    pub fn fits_bigint(self) -> bool {
        self.0 <= i64::MAX as u64
    }

    pub(crate) fn to_bigint(self) -> Option<i64> {
        if self.fits_bigint() {
            Some(self.0 as i64)
        } else {
            None
        }
    }

    pub(crate) fn from_bigint(value: i64) -> Option<MicroUsd> {
        u64::try_from(value).ok().map(MicroUsd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn bigint_fit_boundary_is_exact() {
        let max = MicroUsd(i64::MAX as u64);
        assert!(max.fits_bigint(), "i64::MAX micro-USD fits bigint");
        assert_eq!(max.to_bigint(), Some(i64::MAX));

        let over = MicroUsd(i64::MAX as u64 + 1);
        assert!(!over.fits_bigint(), "i64::MAX + 1 does NOT fit bigint");
        assert_eq!(over.to_bigint(), None);

        assert_eq!(
            MicroUsd::from_bigint(i64::MAX),
            Some(MicroUsd(i64::MAX as u64))
        );
        assert_eq!(MicroUsd::from_bigint(0), Some(MicroUsd::ZERO));
        assert_eq!(
            MicroUsd::from_bigint(-1),
            None,
            "a corrupt negative bigint is never reinterpreted as spendable money"
        );
    }
}
