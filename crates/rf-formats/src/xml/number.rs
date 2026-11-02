//! Number formatting that matches .NET's, so XML comes out the way RedForge has
//! always written it.

use std::fmt::Write;

/// Splits `{:e}` output ("d.ddde±x") into its digits and `scale`, where
/// `value = 0.DIGITS * 10^scale`.
fn split_exp(text: &str) -> (String, i32) {
    let (mantissa, exp) = text.split_once('e').expect("{:e} has an exponent");
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let exp: i32 = exp.parse().expect("exponent is an integer");
    (digits, exp + 1)
}

/// The shortest round-tripping digits of a finite, positive float, with its scale.
///
/// When the value lies exactly halfway between two shortest candidates, Rust
/// takes the upper one and .NET the one ending in an even digit, so that case is
/// redone .NET's way.
fn shortest_digits(shortest: &str, exact_f64: f64, round_trips: impl Fn(&str) -> bool) -> (String, i32) {
    let (digits, scale) = split_exp(shortest);
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let n = digits.len();

    // A tie needs both neighbours to round-trip; checking the lower one first
    // keeps the expensive exact expansion off the common path.
    let mut lower = digits.as_bytes().to_vec();
    if lower[n - 1] == b'0' {
        return (digits.to_owned(), scale);
    }
    lower[n - 1] -= 1;
    if !round_trips(&format!("0.{}e{scale}", std::str::from_utf8(&lower).unwrap())) {
        return (digits.to_owned(), scale);
    }

    // The exact decimal expansion (an f32 or f64 has fewer than 800 digits).
    let (exact, exact_scale) = split_exp(&format!("{:.800e}", exact_f64));
    if exact_scale == scale && exact.len() > n {
        let (lower, tail) = exact.split_at(n);
        let last = lower.as_bytes()[n - 1];
        let is_tie = tail.as_bytes()[0] == b'5' && tail[1..].bytes().all(|b| b == b'0');
        if is_tie && last != b'9' {
            let mut upper = lower.as_bytes().to_vec();
            upper[n - 1] += 1;
            let upper = String::from_utf8(upper).unwrap();
            let even = if (last - b'0').is_multiple_of(2) { lower } else { upper.as_str() };
            let candidate = format!("0.{even}e{scale}");
            if round_trips(&candidate) {
                let even = even.trim_end_matches('0');
                return (even.to_owned(), scale);
            }
        }
    }
    (digits.to_owned(), scale)
}

/// .NET's general ("G") formatting of shortest round-trip digits, as
/// `float.ToString(CultureInfo.InvariantCulture)` does since .NET Core 3.0.
fn format_general(out: &mut String, negative: bool, digits: &str, scale: i32, max_round_trip_digits: i32) {
    if negative {
        out.push('-');
    }
    let max_digits = (digits.len() as i32).max(max_round_trip_digits);
    let scientific = scale > max_digits || scale < -3;
    let mut dig = digits.bytes().peekable();
    let mut dig_pos = if scientific { 1 } else { scale };
    if dig_pos > 0 {
        while dig_pos > 0 {
            out.push(dig.next().map(char::from).unwrap_or('0'));
            dig_pos -= 1;
        }
    } else {
        out.push('0');
    }
    if dig.peek().is_some() || dig_pos < 0 {
        out.push('.');
        while dig_pos < 0 {
            out.push('0');
            dig_pos += 1;
        }
        out.extend(dig.map(char::from));
    }
    if scientific {
        let e = scale - 1;
        let _ = write!(out, "E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
}

/// `float.ToString(CultureInfo.InvariantCulture)` (RedForge's `FloatUtil.ToString`).
pub fn f32_to_string(v: f32) -> String {
    let mut s = String::new();
    push_f32(&mut s, v);
    s
}

pub fn push_f32(out: &mut String, v: f32) {
    if v.is_nan() {
        out.push_str("NaN");
    } else if v.is_infinite() {
        out.push_str(if v > 0.0 { "Infinity" } else { "-Infinity" });
    } else if v == 0.0 {
        out.push_str(if v.is_sign_negative() { "-0" } else { "0" });
    } else {
        let a = v.abs();
        let (digits, scale) = shortest_digits(&format!("{a:e}"), a as f64, |c| c.parse::<f32>().is_ok_and(|p| p == a));
        format_general(out, v < 0.0, &digits, scale, 9);
    }
}

/// `double.ToString(CultureInfo.InvariantCulture)`.
pub fn f64_to_string(v: f64) -> String {
    let mut out = String::new();
    if v.is_nan() {
        out.push_str("NaN");
    } else if v.is_infinite() {
        out.push_str(if v > 0.0 { "Infinity" } else { "-Infinity" });
    } else if v == 0.0 {
        out.push_str(if v.is_sign_negative() { "-0" } else { "0" });
    } else {
        let a = v.abs();
        let (digits, scale) = shortest_digits(&format!("{a:e}"), a, |c| c.parse::<f64>().is_ok_and(|p| p == a));
        format_general(&mut out, v < 0.0, &digits, scale, 17);
    }
    out
}

/// Parses a float the way RedForge reads them back (invariant culture), also
/// accepting .NET's `Infinity`/`NaN` spellings.
pub fn parse_f32(s: &str) -> Option<f32> {
    let s = s.trim();
    match s {
        "NaN" => Some(f32::NAN),
        "Infinity" | "∞" => Some(f32::INFINITY),
        "-Infinity" | "-∞" => Some(f32::NEG_INFINITY),
        _ => s.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotnet_float_formats() {
        let cases: &[(f32, &str)] = &[
            (0.0, "0"),
            (-0.0, "-0"),
            (1.0, "1"),
            (-3948.3682, "-3948.3682"),
            (0.5, "0.5"),
            (0.0001, "0.0001"),
            (0.00001, "1E-05"),
            (1e8, "100000000"),
            (1e9, "1E+09"),
            (123456790.0, "123456790"),
            (f32::MAX, "3.4028235E+38"),
            (f32::MIN_POSITIVE, "1.1754944E-38"),
        ];
        for &(v, want) in cases {
            assert_eq!(f32_to_string(v), want, "{v:e}");
        }
    }
}
