//! Limits loaded once when the server starts.
const ENV: &str = concat!("SALTATION_TRANSFER_DAILY", "_LIMIT");
/// Largest limit the daily totals and the 12-digit amount fields can carry.
const MAX_LIMIT: i64 = 999_999_999_999;
#[derive(Debug, Clone, Copy)]
/// Largest limit the daily totals (NUMBER(15)) and the 12-digit amount fields can carry.
pub struct Limits {
    pub transfer_daily: i64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            transfer_daily: 50_000_000,
        }
    }
}
impl Limits {
    pub fn from_env() -> anyhow::Result<Self> {
        match std::env::var(ENV) {
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            Ok(raw) => Self::parse(&raw),
            Err(e) => anyhow::bail!("{ENV}: {e}"),
        }
    }
    fn parse(raw: &str) -> anyhow::Result<Self> {
        let value = raw
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .filter(|_| !raw.is_empty() && raw.bytes().all(|c| c.is_ascii_digit()));
        match value {
            Some(transfer_daily) => Ok(Self { transfer_daily }),
            None => anyhow::bail!("{ENV} must be an integer from 1 to {MAX_LIMIT} (KRW)"),
        }
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
        }
    }
}
