//! Time parsing for the CLI / future scripting interfaces.
//!
//! Accepts three forms, all returning an exact rational in seconds so
//! float roundoff can never leak into seek targets:
//!
//! - `HH:MM:SS[.fff]`   e.g. `00:01:23.500`
//! - integer + `ms`     e.g. `5500ms`
//! - decimal seconds    e.g. `5.5`, `0.0416666` (treated as a rational)

use std::num::ParseIntError;

use num_rational::Rational64;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TimeParseError {
    #[error("empty time string")]
    Empty,
    #[error("bad time component: {0}")]
    BadInt(#[from] ParseIntError),
    #[error("expected HH:MM:SS[.fff], got {0:?}")]
    BadColonForm(String),
    #[error("negative durations are not allowed: {0:?}")]
    Negative(String),
}

pub fn parse(input: &str) -> Result<Rational64, TimeParseError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(TimeParseError::Empty);
    }
    if s.starts_with('-') {
        return Err(TimeParseError::Negative(s.to_string()));
    }

    if s.contains(':') {
        return parse_colon(s);
    }
    if let Some(stripped) = s.strip_suffix("ms") {
        let ms: i64 = stripped.trim().parse()?;
        return Ok(Rational64::new(ms, 1000));
    }
    parse_decimal_seconds(s)
}

fn parse_colon(s: &str) -> Result<Rational64, TimeParseError> {
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<i64>()?, m.parse::<i64>()?, *s),
        [m, s] => (0, m.parse::<i64>()?, *s),
        _ => return Err(TimeParseError::BadColonForm(s.to_string())),
    };
    let sec_r = parse_decimal_seconds(sec)?;
    Ok(Rational64::from_integer(h * 3600 + m * 60) + sec_r)
}

fn parse_decimal_seconds(s: &str) -> Result<Rational64, TimeParseError> {
    if let Some(dot) = s.find('.') {
        let int_part: i64 = if dot == 0 { 0 } else { s[..dot].parse()? };
        let frac_str = &s[dot + 1..];
        if frac_str.is_empty() {
            return Ok(Rational64::from_integer(int_part));
        }
        let frac: i64 = frac_str.parse()?;
        let denom = 10_i64.pow(frac_str.len() as u32);
        Ok(Rational64::new(int_part * denom + frac, denom))
    } else {
        let n: i64 = s.parse()?;
        Ok(Rational64::from_integer(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_seconds() {
        assert_eq!(parse("5").unwrap(), Rational64::from_integer(5));
        assert_eq!(parse("5.5").unwrap(), Rational64::new(55, 10));
        assert_eq!(parse("0.001").unwrap(), Rational64::new(1, 1000));
    }

    #[test]
    fn parses_milliseconds() {
        assert_eq!(parse("5500ms").unwrap(), Rational64::new(5500, 1000));
        assert_eq!(parse("0ms").unwrap(), Rational64::new(0, 1));
    }

    #[test]
    fn parses_hms() {
        assert_eq!(
            parse("01:02:03.500").unwrap(),
            Rational64::from_integer(3723) + Rational64::new(500, 1000)
        );
        assert_eq!(parse("00:00:00").unwrap(), Rational64::from_integer(0));
        // Two-component form is MM:SS, so "00:30" is 30 seconds.
        assert_eq!(parse("00:30").unwrap(), Rational64::from_integer(30));
        assert_eq!(parse("02:30").unwrap(), Rational64::from_integer(2 * 60 + 30));
    }

    #[test]
    fn rejects_negatives() {
        assert!(matches!(parse("-1"), Err(TimeParseError::Negative(_))));
    }
}
