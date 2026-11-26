//! Strict byte-size grammar shared by the operator size settings (`LCG_DOCUMENT_MAX_BYTES`,
//! `LCG_REQUEST_MAX_BYTES`, `LCG_CHUNK_TEXT_MAX_BYTES`).
//!
//! Accepted grammar (after trimming outer whitespace): ASCII digits only (a byte count), or
//! digits immediately followed by a case-sensitive `KiB` or `MiB` — and `GiB` only when the
//! setting's upper bound is at least 1 GiB (otherwise every `GiB` value would be out of range
//! anyway). The value must be positive and at most the caller's upper bound. Everything else
//! (signs, decimals, `_`, inner whitespace, `KB`/`MB`, lowercase units, overflow) is rejected.

use crate::error::Error;

/// Strictly parse `raw` into a byte count for the setting named `var`.
///
/// `max` is the inclusive upper bound; `max_label` is its human form (e.g. `256MiB`) used in the
/// "expected" text of every error.
pub fn parse_size_bytes(raw: &str, var: &str, max: usize, max_label: &str) -> Result<usize, Error> {
    const GIB: usize = 1024 * 1024 * 1024;
    let allow_gib = max >= GIB;
    let units = if allow_gib {
        "KiB, MiB or GiB"
    } else {
        "KiB or MiB"
    };
    let invalid = |reason: &str| {
        Error::Config(format!(
            "invalid {var}=\"{raw}\": {reason}; expected a positive byte count (e.g. 4194304) \
             or a count with a {units} suffix (e.g. 4MiB), at most {max_label}"
        ))
    };
    let s = raw.trim();
    if s.is_empty() {
        return Err(invalid("empty value"));
    }
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(digits_end);
    if digits.is_empty() {
        return Err(invalid("must start with a digit (no sign or fraction)"));
    }
    let multiplier: usize = match unit {
        "" => 1,
        "KiB" => 1024,
        "MiB" => 1024 * 1024,
        "GiB" if allow_gib => GIB,
        _ => {
            return Err(invalid(&format!(
                "unrecognised unit (only {} are accepted; no decimals, whitespace or \
                 underscores)",
                units.replace(" or ", " and ")
            )))
        }
    };
    let n: usize = digits.parse().map_err(|_| invalid("number is too large"))?;
    if n == 0 {
        return Err(invalid("must be greater than zero"));
    }
    let bytes = n
        .checked_mul(multiplier)
        .ok_or_else(|| invalid("number is too large"))?;
    if bytes > max {
        return Err(invalid(&format!(
            "exceeds the upper bound of {max} bytes ({})",
            human_bound(max)
        )));
    }
    Ok(bytes)
}

fn human_bound(max: usize) -> String {
    const MIB: usize = 1024 * 1024;
    const GIB: usize = 1024 * MIB;
    if max.is_multiple_of(GIB) {
        format!("{} GiB", max / GIB)
    } else {
        format!("{} MiB", max / MIB)
    }
}
