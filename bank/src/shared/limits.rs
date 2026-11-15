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
        Self::from_lookup(|name| std::env::var(name))
    }
    fn from_lookup(
        get: impl Fn(&str) -> Result<String, std::env::VarError>,
    ) -> anyhow::Result<Self> {
        let defaults = Self::default();
        Ok(Self {
            transfer_daily: Self::load(ENV, defaults.transfer_daily, &get)?,
            atm_withdraw: Self::load(ATM, defaults.atm_withdraw, &get)?,
            atm_withdraw_daily: Self::load(ATM_DAILY, defaults.atm_withdraw_daily, &get)?,
        })
    }
    fn load(
        name: &str,
        default: i64,
        get: &impl Fn(&str) -> Result<String, std::env::VarError>,
    ) -> anyhow::Result<i64> {
        match get(name) {
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
    fn each_environment_variable_sets_only_its_own_limit() {
        for name in [ENV, ATM, ATM_DAILY] {
            let configured = Limits::from_lookup(|key| {
                if key == name {
                    Ok("123".into())
                } else {
                    Err(std::env::VarError::NotPresent)
                }
            })
            .unwrap();
            let defaults = Limits::default();
            assert_eq!(
                configured.transfer_daily,
                if name == ENV {
                    123
                } else {
                    defaults.transfer_daily
                }
            );
            assert_eq!(
                configured.atm_withdraw,
                if name == ATM {
                    123
                } else {
                    defaults.atm_withdraw
                }
            );
            assert_eq!(
                configured.atm_withdraw_daily,
                if name == ATM_DAILY {
                    123
                } else {
                    defaults.atm_withdraw_daily
                }
            );
            let error = Limits::from_lookup(|key| {
                if key == name {
                    Ok("invalid".into())
                } else {
                    Err(std::env::VarError::NotPresent)
                }
            })
            .unwrap_err();
            assert!(error.to_string().contains(name));
        }
    }
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
