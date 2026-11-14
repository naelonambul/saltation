//! Limits loaded once when the server starts.
const ENV: &str = concat!("SALTATION_TRANSFER_DAILY", "_LIMIT");
const ATM: &str = "SALTATION_ATM_WITHDRAW_LIMIT";
const ATM_DAILY: &str = "SALTATION_ATM_WITHDRAW_DAILY_LIMIT";
/// Largest limit the daily totals and the 12-digit amount fields can carry.
const MAX_LIMIT: i64 = 999_999_999_999;
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub transfer_daily: i64,
    pub atm_withdraw: i64,
    pub atm_withdraw_daily: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            transfer_daily: 50_000_000,
            atm_withdraw: 1_000_000,
            atm_withdraw_daily: 6_000_000,
        }
    }
}
impl Limits {
    pub fn from_env() -> anyhow::Result<Self> {
        let defaults = Self::default();
        Ok(Self {
            transfer_daily: Self::load(ENV, defaults.transfer_daily)?,
            atm_withdraw: Self::load(ATM, defaults.atm_withdraw)?,
            atm_withdraw_daily: Self::load(ATM_DAILY, defaults.atm_withdraw_daily)?,
        })
    }
    fn load(name: &str, default: i64) -> anyhow::Result<i64> {
        match std::env::var(name) {
            Err(std::env::VarError::NotPresent) => Ok(default),
            Ok(raw) => Self::value(name, &raw),
            Err(e) => anyhow::bail!("{name}: {e}"),
        }
    }
    fn value(name: &str, raw: &str) -> anyhow::Result<i64> {
        let value = raw
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .filter(|_| !raw.is_empty() && raw.bytes().all(|c| c.is_ascii_digit()));
        value
            .ok_or_else(|| anyhow::anyhow!("{name} must be an integer from 1 to {MAX_LIMIT} (KRW)"))
    }
    #[cfg(test)]
    fn parse(raw: &str) -> anyhow::Result<Self> {
        Ok(Self {
            transfer_daily: Self::value(ENV, raw)?,
            ..Self::default()
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_limit_requires_positive_integer() {
        assert_eq!(Limits::default().transfer_daily, 50_000_000);
        assert_eq!(Limits::parse("1").unwrap().transfer_daily, 1);
        assert_eq!(
            Limits::parse("999999999999").unwrap().transfer_daily,
            MAX_LIMIT
        );
        assert_eq!(Limits::default().atm_withdraw, 1_000_000);
        assert_eq!(Limits::default().atm_withdraw_daily, 6_000_000);
        for name in [ENV, ATM, ATM_DAILY] {
            assert_eq!(Limits::value(name, "1").unwrap(), 1);
            assert_eq!(Limits::value(name, "999999999999").unwrap(), MAX_LIMIT);
        }
        for raw in [
            "",
            "0",
            "-1",
            "1.0",
            "abc",
            " 1",
            "1000000000000",
            "9223372036854775808",
        ] {
            assert!(Limits::parse(raw).is_err(), "{raw}");
            for name in [ATM, ATM_DAILY] {
                assert!(Limits::value(name, raw).is_err(), "{name}: {raw}");
            }
        }
    }
}
