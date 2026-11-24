//! Calendar recurrence is based on local dates, never fixed 24-hour arithmetic.
use crate::SettingsError;
use chrono::{DateTime, Datelike, Duration, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use interface_contracts::agent_work::AgentCalendarSchedule;

fn invalid() -> SettingsError {
    SettingsError::InvalidAgent(
        "Choose an IANA timezone, hour 0–23, minute 0–59 and unique ISO weekdays 1–7.".into(),
    )
}
/// # Errors
/// Rejects unknown zones and malformed calendar fields.
pub fn validate_calendar(schedule: &AgentCalendarSchedule) -> Result<Tz, SettingsError> {
    let timezone: Tz = schedule.timezone.parse().map_err(|_| invalid())?;
    let days: std::collections::BTreeSet<_> = schedule.weekdays.iter().copied().collect();
    if schedule.hour > 23
        || schedule.minute > 59
        || days.len() != schedule.weekdays.len()
        || days.iter().any(|day| !(1..=7).contains(day))
    {
        return Err(invalid());
    }
    Ok(timezone)
}
/// Next occurrences strictly after `after_ms`. Spring gaps are skipped; repeated fall times
/// occur once, at their first instant. At most ten dates are returned.
/// # Errors
/// Refuses invalid dates/zones rather than silently changing schedule semantics.
pub fn calendar_occurrences(
    schedule: &AgentCalendarSchedule,
    after_ms: u64,
    count: usize,
) -> Result<Vec<u64>, SettingsError> {
    let timezone = validate_calendar(schedule)?;
    let after =
        DateTime::<Utc>::from_timestamp_millis(i64::try_from(after_ms).map_err(|_| invalid())?)
            .ok_or_else(invalid)?;
    let mut day = after.with_timezone(&timezone).date_naive();
    let mut occurrences = Vec::new();
    for _ in 0..80 {
        if schedule.weekdays.is_empty()
            || schedule
                .weekdays
                .contains(&u8::try_from(day.weekday().number_from_monday()).map_err(|_| invalid())?)
        {
            let local = day
                .and_hms_opt(u32::from(schedule.hour), u32::from(schedule.minute), 0)
                .ok_or_else(invalid)?;
            let candidate = match timezone.from_local_datetime(&local) {
                LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => Some(at),
                LocalResult::None => None,
            };
            if let Some(at) = candidate
                && at > after
                && let Ok(at) = u64::try_from(at.timestamp_millis())
            {
                occurrences.push(at);
                if occurrences.len() >= count.clamp(1, 10) {
                    break;
                }
            }
        }
        day = day
            .checked_add_signed(Duration::days(1))
            .ok_or_else(invalid)?;
    }
    if occurrences.is_empty() {
        return Err(invalid());
    }
    Ok(occurrences)
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::agent_work::AgentMissedRunPolicy;
    fn millis(value: &str) -> u64 {
        u64::try_from(
            DateTime::parse_from_rfc3339(value)
                .unwrap()
                .timestamp_millis(),
        )
        .unwrap()
    }
    fn schedule(hour: u8, minute: u8) -> AgentCalendarSchedule {
        AgentCalendarSchedule {
            timezone: "America/Chicago".into(),
            weekdays: vec![],
            hour,
            minute,
            missed: AgentMissedRunPolicy::Skip,
            pause_on_failure: true,
        }
    }
    #[test]
    fn calendar_spring_gap_is_skipped_and_fall_repeat_occurs_once() {
        let spring =
            calendar_occurrences(&schedule(2, 30), millis("2026-03-07T09:00:00Z"), 2).unwrap();
        assert_eq!(
            spring,
            vec![
                millis("2026-03-09T07:30:00Z"),
                millis("2026-03-10T07:30:00Z")
            ]
        );
        let fall =
            calendar_occurrences(&schedule(1, 30), millis("2026-11-01T05:00:00Z"), 2).unwrap();
        assert_eq!(
            fall,
            vec![
                millis("2026-11-01T06:30:00Z"),
                millis("2026-11-02T07:30:00Z")
            ]
        );
        assert_eq!(
            calendar_occurrences(&schedule(1, 30), fall[0], 1).unwrap()[0],
            fall[1]
        );
    }
    #[test]
    fn calendar_keeps_local_hour_across_dst_and_rejects_invalid_schedule() {
        let dates =
            calendar_occurrences(&schedule(9, 0), millis("2026-03-07T00:00:00Z"), 3).unwrap();
        assert_eq!(dates[1] - dates[0], 23 * 3600 * 1000);
        let mut invalid = schedule(24, 0);
        assert!(validate_calendar(&invalid).is_err());
        invalid.hour = 9;
        invalid.timezone = "Not/AZone".into();
        assert!(validate_calendar(&invalid).is_err());
        invalid.timezone = "UTC".into();
        invalid.weekdays = vec![1, 1];
        assert!(validate_calendar(&invalid).is_err());
    }
}
