//! Calendar arithmetic shared by the provider session stores.

/// Howard Hinnant's `civil_from_days`: turns a count of days since the Unix
/// epoch into a proleptic Gregorian `(year, month, day)`.
///
/// Callers own their own ISO rendering — the three provider stores print
/// different fractional-second forms from the same calendar triple.
#[must_use]
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

/// Inverse of [`civil_from_days`]: turns a proleptic Gregorian `(year, month,
/// day)` back into a count of days since the Unix epoch.
#[must_use]
pub fn days_from_civil(mut year: i64, month: i64, day: i64) -> i64 {
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Trailing sub-second field of the frozen ISO timestamp. Each provider store
/// prints a different one and every byte of those strings is a persisted
/// contract, so the choice is the caller's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsoFraction {
    /// `…:SSZ`
    None,
    /// `…:SS.000Z`
    Zeros,
    /// `…:SS.mmmZ`
    Millis,
}

/// Renders milliseconds since the Unix epoch in the fixed-width ISO form the
/// provider session stores freeze. Times before the epoch clamp to it, keeping
/// the sub-second field the caller asked for.
#[must_use]
pub fn unix_millis_iso(milliseconds: i64, fraction: IsoFraction) -> String {
    let seconds = milliseconds.div_euclid(1_000).max(0);
    let millis = milliseconds.rem_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = time / 3_600;
    let minute = (time % 3_600) / 60;
    let second = time % 60;
    let stamp = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
    match fraction {
        IsoFraction::None => format!("{stamp}Z"),
        IsoFraction::Zeros => format!("{stamp}.000Z"),
        IsoFraction::Millis => format!("{stamp}.{millis:03}Z"),
    }
}

/// Seconds since the Unix epoch in the same frozen form.
#[must_use]
pub fn unix_seconds_iso(seconds: i64, fraction: IsoFraction) -> String {
    unix_millis_iso(seconds.max(0).saturating_mul(1_000), fraction)
}

#[cfg(test)]
mod tests {
    use super::{IsoFraction, civil_from_days, days_from_civil, unix_millis_iso, unix_seconds_iso};

    #[test]
    fn days_from_civil_is_the_inverse_of_civil_from_days() {
        for days in -20_000..20_000 {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days, "day {days}");
        }
    }

    #[test]
    fn iso_fractions_keep_each_store_byte_identical() {
        assert_eq!(
            unix_seconds_iso(1_767_225_600, IsoFraction::None),
            "2026-01-01T00:00:00Z"
        );
        assert_eq!(
            unix_seconds_iso(1_767_225_600, IsoFraction::Zeros),
            "2026-01-01T00:00:00.000Z"
        );
        assert_eq!(
            unix_millis_iso(1_767_225_600_500, IsoFraction::Millis),
            "2026-01-01T00:00:00.500Z"
        );
        assert_eq!(
            unix_seconds_iso(-5, IsoFraction::None),
            "1970-01-01T00:00:00Z"
        );
        assert_eq!(
            unix_millis_iso(-500, IsoFraction::Millis),
            "1970-01-01T00:00:00.500Z"
        );
    }

    #[test]
    fn civil_from_days_matches_known_calendar_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_513), (2026, 3, 1));
    }

    #[test]
    fn civil_from_days_walks_leap_and_century_boundaries() {
        let mut expected = (1970, 1, 1);
        for days in 0..(80 * 366) {
            assert_eq!(civil_from_days(days), expected, "day {days}");
            let (year, month, day) = expected;
            let length = match month {
                1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
                4 | 6 | 9 | 11 => 30,
                _ if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
                _ => 28,
            };
            expected = if day < length {
                (year, month, day + 1)
            } else if month < 12 {
                (year, month + 1, 1)
            } else {
                (year + 1, 1, 1)
            };
        }
    }
}
