//! UTC timestamps without a date library.

use std::time::{SystemTime, UNIX_EPOCH};

fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    // Howard Hinnant's days-from-civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, rem / 3_600, rem % 3_600 / 60, rem % 60)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `2026-10-06T08:15:30Z` (no fractional seconds: the macOS app's ISO 8601 decoder rejects them).
pub fn iso8601_utc() -> String {
    iso8601_utc_at(now())
}

/// Formats supplied Unix seconds as an ISO-8601 UTC timestamp.
pub fn iso8601_utc_at(seconds: i64) -> String {
    let (y, mo, d, h, mi, s) = civil(seconds);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20261006T081530Z` for file names.
pub fn local_stamp() -> String {
    let (y, mo, d, h, mi, s) = civil(now());
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// `2026-10-06`
pub fn today() -> String {
    let (y, mo, d, ..) = civil(now());
    format!("{y:04}-{mo:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts_known_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(super::civil(1_790_000_000), (2026, 9, 21, 14, 13, 20));
        assert_eq!(super::civil(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn formats_supplied_epoch_without_reading_clock() {
        assert_eq!(super::iso8601_utc_at(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::iso8601_utc_at(951_782_400), "2000-02-29T00:00:00Z");
    }
}
