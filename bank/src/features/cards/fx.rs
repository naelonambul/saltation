//! Pure card-message validation. Text is retained exactly as received.
use serde_json::Value;
use std::sync::OnceLock;

const MAX_KRW: u64 = 999_999_999_999;

pub(super) fn krw_amount(currency: &str, amount: u64) -> Option<u64> {
    if amount == 0 {
        return None;
    }
    let converted = match currency {
        "KRW" => Some(amount),
        "USD" => amount.checked_mul(1300),
        "EUR" => amount.checked_mul(1400),
        "JPY" if amount.is_multiple_of(100) => (amount / 100).checked_mul(900),
        "CNY" => amount.checked_mul(200),
        _ => None,
    }?;
    (1..=MAX_KRW).contains(&converted).then_some(converted)
}

#[derive(Debug, PartialEq)]
pub(super) struct Validated {
    pub message: Value,
    pub amount: u64,
    pub krw_amount: u64,
    /// Millionths of a degree, preserving exact decimal precision.
    pub latitude: Option<i64>,
    pub longitude: Option<i64>,
}

fn text<'a>(message: &'a Value, field: &'static str) -> Result<&'a str, &'static str> {
    message.get(field).and_then(Value::as_str).ok_or(field)
}
fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}
fn coordinate(value: &str, limit: i64) -> Option<i64> {
    let (negative, unsigned) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if !digits(whole) || fraction.len() > 6 || (unsigned.contains('.') && !digits(fraction)) {
        return None;
    }
    let whole = whole.parse::<i64>().ok()?.checked_mul(1_000_000)?;
    let part = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i64>().ok()? * 10i64.pow(6 - fraction.len() as u32)
    };
    let scaled = whole.checked_add(part)?;
    if scaled > limit * 1_000_000 {
        return None;
    }
    Some(if negative { -scaled } else { scaled })
}

/// Returns the first invalid field in the order specified by F-41/F-42.
pub(super) fn validate(message: &Value) -> Result<Validated, &'static str> {
    let key = text(message, "idempotency_key")?;
    if !(1..=40).contains(&key.len()) || !key.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err("idempotency_key");
    }
    if !super::valid_number(text(message, "card_no")?) {
        return Err("card_no");
    }
    let raw = text(message, "amount")?;
    let amount = if digits(raw) {
        raw.parse::<u64>().ok().filter(|n| *n > 0)
    } else {
        None
    }
    .ok_or("amount")?;
    let currency = text(message, "currency")?;
    if !matches!(currency, "KRW" | "USD" | "EUR" | "JPY" | "CNY") {
        return Err("currency");
    }
    let krw_amount = krw_amount(currency, amount).ok_or("amount")?;
    if !(1..=60).contains(&text(message, "merchant_name")?.chars().count()) {
        return Err("merchant_name");
    }
    let mcc = text(message, "mcc")?;
    if mcc.len() != 4 || !digits(mcc) {
        return Err("mcc");
    }
    static COUNTRIES: OnceLock<[bool; 676]> = OnceLock::new();
    let countries = COUNTRIES.get_or_init(|| {
        let list: Vec<String> =
            serde_json::from_str(include_str!("countries.json")).expect("country codes");
        let mut table = [false; 676];
        for code in list {
            let b = code.as_bytes();
            assert!(
                b.len() == 2 && b[0].is_ascii_uppercase() && b[1].is_ascii_uppercase(),
                "invalid ISO-3166 alpha-2 code: {code}"
            );
            table[((b[0] - b'A') as usize) * 26 + ((b[1] - b'A') as usize)] = true;
        }
        table
    });
    let country = text(message, "country")?.as_bytes();
    if country.len() != 2
        || !country[0].is_ascii_uppercase()
        || !country[1].is_ascii_uppercase()
        || !countries[((country[0] - b'A') as usize) * 26 + ((country[1] - b'A') as usize)]
    {
        return Err("country");
    }
    let (latitude, longitude) = match text(message, "entry_mode")? {
        "online" => {
            if message.get("latitude").is_some() {
                return Err("latitude");
            }
            if message.get("longitude").is_some() {
                return Err("longitude");
            }
            (None, None)
        }
        "in_person" => (
            Some(coordinate(text(message, "latitude")?, 90).ok_or("latitude")?),
            Some(coordinate(text(message, "longitude")?, 180).ok_or("longitude")?),
        ),
        _ => return Err("entry_mode"),
    };
    Ok(Validated {
        message: message.clone(),
        amount,
        krw_amount,
        latitude,
        longitude,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message() -> Value {
        json!({"idempotency_key":" key ", "card_no":"0000000000000000", "amount":"0001", "currency":"USD", "merchant_name":" 가게 ", "mcc":"0123", "country":"KR", "entry_mode":"online"})
    }
    #[test]
    fn fixed_rates_ranges_and_overflow() {
        for (currency, amount, expected) in [
            ("USD", 1, 1300),
            ("EUR", 1, 1400),
            ("JPY", 100, 900),
            ("CNY", 1, 200),
            ("KRW", 1, 1),
        ] {
            assert_eq!(krw_amount(currency, amount), Some(expected));
            assert_eq!(krw_amount(currency, 0), None);
            assert_eq!(krw_amount(currency, u64::MAX), None);
            let (unit, rate) = if currency == "JPY" {
                (100, 900)
            } else {
                (1, expected)
            };
            let max = MAX_KRW / rate * unit;
            assert_eq!(krw_amount(currency, max), Some(max / unit * rate));
            assert_eq!(krw_amount(currency, max + unit), None);
        }
        assert_eq!(krw_amount("JPY", 150), None);
        assert_eq!(krw_amount("JPY", u64::MAX / 100 * 100), None);
        assert_eq!(krw_amount("GBP", 1), None);
    }
    #[test]
    fn preserves_text_and_accepts_unissued_synthetic_number() {
        let msg = message();
        let valid = validate(&msg).unwrap();
        assert_eq!(valid.message, msg);
        assert_eq!(
            (
                valid.amount,
                valid.krw_amount,
                valid.latitude,
                valid.longitude
            ),
            (1, 1300, None, None)
        );
        let mut msg = msg;
        msg["merchant_name"] = json!("가".repeat(60));
        msg["idempotency_key"] = json!("~".repeat(40));
        assert!(validate(&msg).is_ok());
    }
    #[test]
    fn every_invalid_field_and_first_error() {
        for (field, value) in [
            ("idempotency_key", json!("")),
            ("idempotency_key", json!("x".repeat(41))),
            ("idempotency_key", json!("한")),
            ("idempotency_key", json!("\n")),
            ("idempotency_key", json!("\u{7f}")),
            ("card_no", json!("1000000000000000")),
            ("card_no", json!("0000000000000001")),
            ("card_no", json!("0000abcdefgh1234")),
            ("card_no", json!("0000")),
            ("amount", json!(1)),
            ("amount", json!("0")),
            ("amount", json!("-1")),
            ("amount", json!("1.0")),
            ("amount", json!("+1")),
            ("amount", json!("1e3")),
            ("amount", json!("18446744073709551616")),
            ("amount", json!("999999999999")),
            ("currency", json!("GBP")),
            ("merchant_name", json!("")),
            ("merchant_name", json!("가".repeat(61))),
            ("mcc", json!("123")),
            ("mcc", json!("123a")),
            ("country", json!("XX")),
            ("country", json!("kr")),
            ("country", json!("XK")),
            ("entry_mode", json!("manual")),
            ("latitude", json!("0")),
            ("longitude", Value::Null),
        ] {
            let mut msg = message();
            msg[field] = value;
            assert_eq!(validate(&msg).unwrap_err(), field, "{msg}");
        }
        for field in [
            "idempotency_key",
            "card_no",
            "amount",
            "currency",
            "merchant_name",
            "mcc",
            "country",
            "entry_mode",
        ] {
            let mut msg = message();
            msg.as_object_mut().unwrap().remove(field);
            assert_eq!(validate(&msg).unwrap_err(), field);
        }
        assert_eq!(validate(&json!({})).unwrap_err(), "idempotency_key");
        let mut msg = message();
        msg["currency"] = json!("JPY");
        msg["amount"] = json!("150");
        assert_eq!(validate(&msg).unwrap_err(), "amount");
    }
    #[test]
    fn exact_coordinates_for_each_entry_mode() {
        let mut msg = message();
        msg["entry_mode"] = json!("in_person");
        assert_eq!(validate(&msg).unwrap_err(), "latitude");
        msg["latitude"] = json!("-90.000000");
        assert_eq!(validate(&msg).unwrap_err(), "longitude");
        msg["longitude"] = json!("180.000000");
        let valid = validate(&msg).unwrap();
        assert_eq!(
            (valid.latitude, valid.longitude),
            (Some(-90_000_000), Some(180_000_000))
        );
        assert_eq!(valid.message, msg);
        for (field, value) in [
            ("latitude", json!("90.000001")),
            ("latitude", json!("-90.000001")),
            ("longitude", json!("180.000001")),
            ("longitude", json!("-180.000001")),
            ("latitude", json!(0)),
            ("latitude", Value::Null),
            ("latitude", json!("1.1234567")),
            ("latitude", json!("1.")),
            ("latitude", json!(".5")),
            ("latitude", json!("1e1")),
            ("latitude", json!("+1")),
            ("latitude", json!(" 1")),
        ] {
            let mut bad = msg.clone();
            bad[field] = value;
            assert_eq!(validate(&bad).unwrap_err(), field, "{bad}");
        }
        msg["latitude"] = json!("90");
        msg["longitude"] = json!("-180");
        assert!(validate(&msg).is_ok());
        msg["latitude"] = json!("-0.123456");
        msg["longitude"] = json!("0");
        assert_eq!(validate(&msg).unwrap().latitude, Some(-123456));
        msg["entry_mode"] = json!("online");
        assert_eq!(validate(&msg).unwrap_err(), "latitude");
        msg.as_object_mut().unwrap().remove("latitude");
        assert_eq!(validate(&msg).unwrap_err(), "longitude");
        msg.as_object_mut().unwrap().remove("longitude");
        assert!(validate(&msg).is_ok());
    }

    #[test]
    fn bank_countries_json_matches_contracts_copy() {
        let dir = std::env::var("CONTRACTS_DIR").unwrap_or_else(|_| "../contracts".into());
        let contract = std::fs::read_to_string(format!("{dir}/codes/countries.json"))
            .expect("contracts/codes/countries.json");
        assert_eq!(
            contract,
            include_str!("countries.json"),
            "bank/src/features/cards/countries.json drifted from contracts/codes/countries.json"
        );
    }
}
