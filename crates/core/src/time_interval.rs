//! Time intervals — recurring schedules used to mute or activate rules and
//! routes during specific windows of wall-clock time.
//!
//! A [`TimeInterval`] is a tenant-scoped named definition that combines a set
//! of cron-like predicates: weekdays, times of day, days of month, months,
//! and years. The interval is "matched" when the current time, evaluated in
//! the interval's [`location`](TimeInterval::location) timezone, satisfies
//! every populated predicate. Empty predicates are treated as "any" (so an
//! interval with only `weekdays = [Sat, Sun]` matches every weekend hour of
//! every day of every year).
//!
//! Multiple [`TimeRange`]s may be supplied per [`TimeInterval`]; the
//! interval matches if **any** of them contains the current instant.
//! Individual predicates within a [`TimeRange`] are AND-ed together. This
//! mirrors Alertmanager's `time_intervals` model so that
//! `acteon import alertmanager` can map them 1:1 into Acteon.
//!
//! Time intervals are evaluated in the dispatch pipeline immediately after
//! silences (and before provider execution). When a referenced interval
//! matches, the action is short-circuited to `ActionOutcome::Muted` with
//! the interval name attached for forensic context.
//!
//! See `docs/book/features/time-intervals.md` for end-to-end usage.

use chrono::{DateTime, Datelike, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Maximum number of time ranges per interval. Bounds match-time work and
/// keeps configurations human-reviewable.
pub const MAX_TIME_RANGES: usize = 64;

/// Maximum length of an interval name (UTF-8 bytes).
pub const MAX_NAME_LEN: usize = 128;

/// Inclusive day-of-month range. Negative values count from end of month
/// (`-1` is the last day) to mirror Alertmanager semantics. The range
/// `start..=end` is interpreted in calendar order; if `start > end` after
/// normalization, the range is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DayOfMonthRange {
    /// First day in the range. `1`-indexed; `-1` is the last day of the month.
    pub start: i32,
    /// Last day in the range (inclusive). `-1` is the last day of the month.
    pub end: i32,
}

impl DayOfMonthRange {
    /// Build a single-day range.
    #[must_use]
    pub const fn single(day: i32) -> Self {
        Self {
            start: day,
            end: day,
        }
    }

    fn validate(self) -> Result<(), String> {
        let valid = |d: i32| (-31..=-1).contains(&d) || (1..=31).contains(&d);
        if !valid(self.start) || !valid(self.end) {
            return Err(format!(
                "day_of_month range {}..={} contains an out-of-range day",
                self.start, self.end
            ));
        }
        // Same-sign ranges must be ordered.
        if self.start.signum() == self.end.signum() && self.start > self.end {
            return Err(format!(
                "day_of_month range start={} must be <= end={}",
                self.start, self.end
            ));
        }
        Ok(())
    }

    fn contains(self, dom: u32, days_in_month: u32) -> bool {
        // `days_in_month` is 28..=31 and `dom` is 1..=31, so all of
        // these `cast_signed()` calls are lossless.
        let month_length = days_in_month.cast_signed();
        let (mut start, mut end) = if self.start > 0 {
            (self.start, self.end)
        } else {
            (month_length + self.start + 1, month_length + self.end + 1)
        };

        if start > month_length {
            return false;
        }

        start = start.max(1);
        end = end.min(month_length);

        let day = dom.cast_signed();
        day >= start && day <= end
    }
}

/// Inclusive month range (1=January, 12=December).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MonthRange {
    /// First month in the range (1..=12).
    pub start: u32,
    /// Last month in the range (1..=12).
    pub end: u32,
}

impl MonthRange {
    fn validate(self) -> Result<(), String> {
        if !(1..=12).contains(&self.start) || !(1..=12).contains(&self.end) {
            return Err(format!(
                "month range {}..={} must be in 1..=12",
                self.start, self.end
            ));
        }
        if self.start > self.end {
            return Err(format!(
                "month range start={} must be <= end={}",
                self.start, self.end
            ));
        }
        Ok(())
    }

    fn contains(self, month: u32) -> bool {
        month >= self.start && month <= self.end
    }
}

/// Inclusive year range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct YearRange {
    pub start: i32,
    pub end: i32,
}

impl YearRange {
    fn validate(self) -> Result<(), String> {
        if self.start > self.end {
            return Err(format!(
                "year range start={} must be <= end={}",
                self.start, self.end
            ));
        }
        Ok(())
    }

    fn contains(self, year: i32) -> bool {
        year >= self.start && year <= self.end
    }
}

/// Inclusive weekday range (Monday=1 .. Sunday=7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WeekdayRange {
    /// First day in the range (1=Mon..7=Sun).
    pub start: u32,
    /// Last day in the range (1=Mon..7=Sun).
    pub end: u32,
}

impl WeekdayRange {
    /// Build a single-day range.
    #[must_use]
    pub const fn single(day: u32) -> Self {
        Self {
            start: day,
            end: day,
        }
    }

    fn validate(self) -> Result<(), String> {
        if !(1..=7).contains(&self.start) || !(1..=7).contains(&self.end) {
            return Err(format!(
                "weekday range {}..={} must be in 1..=7",
                self.start, self.end
            ));
        }
        if self.start > self.end {
            return Err(format!(
                "weekday range start={} must be <= end={}",
                self.start, self.end
            ));
        }
        Ok(())
    }

    fn contains(self, weekday: Weekday) -> bool {
        let n = weekday.number_from_monday();
        n >= self.start && n <= self.end
    }
}

/// Half-open time-of-day window, `[start, end)`, in the interval's local
/// timezone. `end_minute = 1440` means "end of day" (exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TimeOfDayRange {
    /// Inclusive start in minutes-since-midnight (0..=1439).
    pub start_minute: u32,
    /// Exclusive end in minutes-since-midnight (1..=1440).
    pub end_minute: u32,
}

impl TimeOfDayRange {
    /// Build a `HH:MM`-`HH:MM` window with values in 24-hour clock.
    ///
    /// # Errors
    ///
    /// Returns an error if any component is out of range or if the window
    /// is not strictly forward.
    pub fn from_hm(start_h: u32, start_m: u32, end_h: u32, end_m: u32) -> Result<Self, String> {
        let s = start_h
            .checked_mul(60)
            .and_then(|x| x.checked_add(start_m))
            .ok_or_else(|| "start time overflow".to_owned())?;
        let e = end_h
            .checked_mul(60)
            .and_then(|x| x.checked_add(end_m))
            .ok_or_else(|| "end time overflow".to_owned())?;
        let r = Self {
            start_minute: s,
            end_minute: e,
        };
        r.validate()?;
        Ok(r)
    }

    fn validate(self) -> Result<(), String> {
        if self.start_minute >= 1440 {
            return Err(format!(
                "time-of-day start {} out of range (0..=1439)",
                self.start_minute
            ));
        }
        if self.end_minute == 0 || self.end_minute > 1440 {
            return Err(format!(
                "time-of-day end {} out of range (1..=1440)",
                self.end_minute
            ));
        }
        if self.end_minute <= self.start_minute {
            return Err(format!(
                "time-of-day end_minute={} must be > start_minute={}",
                self.end_minute, self.start_minute
            ));
        }
        Ok(())
    }

    fn contains(self, t: NaiveTime) -> bool {
        let m = t.hour() * 60 + t.minute();
        m >= self.start_minute && m < self.end_minute
    }
}

/// A single composite time predicate. All populated lists are AND-ed
/// together; an empty list means "any value matches" for that field.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TimeRange {
    /// Time-of-day windows (in the interval's [`TimeInterval::location`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub times: Vec<TimeOfDayRange>,
    /// Permitted weekdays.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<WeekdayRange>,
    /// Permitted days of month.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days_of_month: Vec<DayOfMonthRange>,
    /// Permitted months (1=Jan..12=Dec).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub months: Vec<MonthRange>,
    /// Permitted years.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub years: Vec<YearRange>,
}

impl TimeRange {
    fn validate(&self) -> Result<(), String> {
        for r in &self.times {
            r.validate()?;
        }
        for r in &self.weekdays {
            r.validate()?;
        }
        for r in &self.days_of_month {
            r.validate()?;
        }
        for r in &self.months {
            r.validate()?;
        }
        for r in &self.years {
            r.validate()?;
        }
        Ok(())
    }

    fn matches_at(&self, now_local: chrono::NaiveDateTime) -> bool {
        let date = now_local.date();
        let time = now_local.time();

        if !self.times.is_empty() && !self.times.iter().any(|r| r.contains(time)) {
            return false;
        }
        if !self.weekdays.is_empty() && !self.weekdays.iter().any(|r| r.contains(date.weekday())) {
            return false;
        }
        if !self.days_of_month.is_empty() {
            let dim = days_in_month(date.year(), date.month());
            if !self
                .days_of_month
                .iter()
                .any(|r| r.contains(date.day(), dim))
            {
                return false;
            }
        }
        if !self.months.is_empty() && !self.months.iter().any(|r| r.contains(date.month())) {
            return false;
        }
        if !self.years.is_empty() && !self.years.iter().any(|r| r.contains(date.year())) {
            return false;
        }
        true
    }
}

/// A named, tenant-scoped time interval. The interval is "matched" when
/// the current instant falls inside any of the [`TimeRange`]s, evaluated
/// in [`location`](TimeInterval::location).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TimeInterval {
    /// Stable name within the (namespace, tenant) scope. Referenced from
    /// rules as `mute_time_intervals: [name]` / `active_time_intervals`.
    pub name: String,
    /// Namespace this interval belongs to.
    pub namespace: String,
    /// Tenant this interval belongs to. Hierarchical matching applies at
    /// dispatch time (a parent-tenant interval covers child tenants).
    pub tenant: String,
    /// Time ranges; the interval matches if **any** range matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_ranges: Vec<TimeRange>,
    /// IANA timezone for evaluating the ranges. Defaults to UTC if absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Optional human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Identity of the caller that created the interval.
    pub created_by: String,
    /// When this interval was created.
    pub created_at: DateTime<Utc>,
    /// When this interval was last updated.
    pub updated_at: DateTime<Utc>,
}

impl TimeInterval {
    /// Validate the interval's structural invariants. Does NOT evaluate
    /// match logic.
    ///
    /// # Errors
    ///
    /// Returns an error string for the first invariant violated.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("time interval name must not be empty".to_owned());
        }
        if self.name.len() > MAX_NAME_LEN {
            return Err(format!(
                "time interval name exceeds {MAX_NAME_LEN}-byte limit"
            ));
        }
        if self.namespace.is_empty() {
            return Err("time interval namespace must not be empty".to_owned());
        }
        if self.tenant.is_empty() {
            return Err("time interval tenant must not be empty".to_owned());
        }
        if self.time_ranges.len() > MAX_TIME_RANGES {
            return Err(format!(
                "time interval has {} ranges, max is {MAX_TIME_RANGES}",
                self.time_ranges.len()
            ));
        }
        for r in &self.time_ranges {
            r.validate()?;
        }
        if let Some(loc) = &self.location {
            loc.parse::<Tz>()
                .map_err(|e| format!("invalid IANA timezone {loc:?}: {e}"))?;
        }
        Ok(())
    }

    /// Return the resolved timezone, defaulting to UTC.
    #[must_use]
    pub fn timezone(&self) -> Tz {
        self.location
            .as_deref()
            .and_then(|l| l.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::UTC)
    }

    /// Check whether this interval is currently matched at `now`.
    ///
    /// An interval with **no** ranges never matches — guards against an
    /// empty config accidentally muting everything (or activating
    /// everything in the `active_time_intervals` direction).
    #[must_use]
    pub fn matches_at(&self, now: DateTime<Utc>) -> bool {
        if self.time_ranges.is_empty() {
            return false;
        }
        let tz = self.timezone();
        let now_local = now.with_timezone(&tz).naive_local();
        self.time_ranges.iter().any(|r| r.matches_at(now_local))
    }
}

/// Number of days in a calendar month, accounting for leap years.
fn days_in_month(year: i32, month: u32) -> u32 {
    use chrono::NaiveDate;
    // Find the first day of the *next* month, then subtract one day.
    let (next_y, next_m) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_y, next_m, 1)
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn days_in_month_handles_leap_year() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2024, 12), 31);
        assert_eq!(days_in_month(2024, 4), 30);
    }

    #[test]
    fn time_of_day_validates_window() {
        assert!(TimeOfDayRange::from_hm(9, 0, 17, 0).is_ok());
        // Equal start/end is rejected (zero-width window).
        assert!(TimeOfDayRange::from_hm(9, 0, 9, 0).is_err());
        // Inverted window is rejected.
        assert!(TimeOfDayRange::from_hm(17, 0, 9, 0).is_err());
        // End-of-day sentinel is allowed.
        assert!(TimeOfDayRange::from_hm(23, 0, 24, 0).is_ok());
        // Overflow rejected.
        assert!(TimeOfDayRange::from_hm(25, 0, 26, 0).is_err());
    }

    #[test]
    fn time_of_day_contains_is_half_open() {
        let r = TimeOfDayRange::from_hm(9, 0, 17, 0).unwrap();
        assert!(r.contains(NaiveTime::from_hms_opt(9, 0, 0).unwrap()));
        assert!(r.contains(NaiveTime::from_hms_opt(16, 59, 59).unwrap()));
        assert!(!r.contains(NaiveTime::from_hms_opt(17, 0, 0).unwrap()));
        assert!(!r.contains(NaiveTime::from_hms_opt(8, 59, 59).unwrap()));
    }

    #[test]
    fn day_of_month_resolves_negative_indices() {
        let r = DayOfMonthRange { start: -1, end: -1 };
        assert!(r.contains(31, 31));
        assert!(r.contains(28, 28));
        assert!(!r.contains(27, 28));
    }

    #[test]
    fn day_of_month_rejects_out_of_range() {
        assert!(DayOfMonthRange { start: 0, end: 5 }.validate().is_err());
        assert!(DayOfMonthRange { start: 1, end: 32 }.validate().is_err());
        assert!(DayOfMonthRange { start: 5, end: 3 }.validate().is_err());
    }

    #[test]
    fn weekday_range_contains() {
        // Monday..=Friday (1..=5)
        let r = WeekdayRange { start: 1, end: 5 };
        assert!(r.contains(Weekday::Mon));
        assert!(r.contains(Weekday::Fri));
        assert!(!r.contains(Weekday::Sat));
        assert!(!r.contains(Weekday::Sun));
    }

    #[test]
    fn empty_predicates_are_any_match() {
        let r = TimeRange::default();
        // 2026-04-12T12:00Z (a Sunday) — empty predicates always match.
        assert!(r.matches_at(utc(2026, 4, 12, 12, 0).naive_utc()));
    }

    #[test]
    fn business_hours_interval_matches_local_time() {
        let interval = TimeInterval {
            name: "business-hours".into(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![TimeRange {
                times: vec![TimeOfDayRange::from_hm(9, 0, 17, 0).unwrap()],
                weekdays: vec![WeekdayRange { start: 1, end: 5 }],
                ..Default::default()
            }],
            location: Some("America/New_York".into()),
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };

        // Mon 2026-04-13 14:00 UTC → 10:00 EDT — inside business hours.
        assert!(interval.matches_at(utc(2026, 4, 13, 14, 0)));

        // Mon 2026-04-13 23:00 UTC → 19:00 EDT — outside.
        assert!(!interval.matches_at(utc(2026, 4, 13, 23, 0)));

        // Sat 2026-04-11 14:00 UTC → 10:00 EDT — wrong weekday.
        assert!(!interval.matches_at(utc(2026, 4, 11, 14, 0)));
    }

    #[test]
    fn weekend_interval_matches() {
        let interval = TimeInterval {
            name: "weekends".into(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![TimeRange {
                weekdays: vec![WeekdayRange { start: 6, end: 7 }],
                ..Default::default()
            }],
            location: None,
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };
        // Sat 2026-04-11 in UTC.
        assert!(interval.matches_at(utc(2026, 4, 11, 0, 0)));
        // Sun 2026-04-12.
        assert!(interval.matches_at(utc(2026, 4, 12, 23, 59)));
        // Mon 2026-04-13.
        assert!(!interval.matches_at(utc(2026, 4, 13, 12, 0)));
    }

    #[test]
    fn end_of_month_via_negative_day() {
        let interval = TimeInterval {
            name: "month-end".into(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![TimeRange {
                days_of_month: vec![DayOfMonthRange { start: -1, end: -1 }],
                ..Default::default()
            }],
            location: None,
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };
        // Apr 30 = last day of April.
        assert!(interval.matches_at(utc(2026, 4, 30, 12, 0)));
        // Apr 29 — not last day.
        assert!(!interval.matches_at(utc(2026, 4, 29, 12, 0)));
        // Feb 29 2024 (leap year) = last day of February.
        assert!(interval.matches_at(utc(2024, 2, 29, 12, 0)));
        // Feb 28 2025 = last day of February.
        assert!(interval.matches_at(utc(2025, 2, 28, 12, 0)));
        assert!(!interval.matches_at(utc(2024, 2, 28, 12, 0)));
    }

    #[test]
    fn empty_time_ranges_never_match() {
        let interval = TimeInterval {
            name: "empty".into(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![],
            location: None,
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };
        assert!(!interval.matches_at(utc(2026, 4, 12, 12, 0)));
    }

    #[test]
    fn validate_rejects_empty_name() {
        let interval = TimeInterval {
            name: String::new(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![],
            location: None,
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };
        assert!(interval.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_timezone() {
        let interval = TimeInterval {
            name: "bad-tz".into(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![TimeRange::default()],
            location: Some("Mars/OlympusMons".into()),
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };
        assert!(interval.validate().is_err());
    }

    #[test]
    fn validate_caps_time_range_count() {
        let interval = TimeInterval {
            name: "many".into(),
            namespace: "prod".into(),
            tenant: "acme".into(),
            time_ranges: vec![TimeRange::default(); MAX_TIME_RANGES + 1],
            location: None,
            description: None,
            created_by: "test".into(),
            created_at: utc(2026, 1, 1, 0, 0),
            updated_at: utc(2026, 1, 1, 0, 0),
        };
        assert!(interval.validate().is_err());
    }

    #[test]
    fn time_range_matches_at_intersects_predicates() {
        // Business hours AND weekday AND month range.
        let r = TimeRange {
            times: vec![TimeOfDayRange::from_hm(9, 0, 17, 0).unwrap()],
            weekdays: vec![WeekdayRange { start: 1, end: 5 }],
            months: vec![MonthRange { start: 4, end: 4 }],
            ..Default::default()
        };
        // Mon 2026-04-13 12:00 UTC — matches all three.
        assert!(r.matches_at(utc(2026, 4, 13, 12, 0).naive_utc()));
        // Wrong month.
        assert!(!r.matches_at(utc(2026, 5, 13, 12, 0).naive_utc()));
        // Right month but Saturday.
        assert!(!r.matches_at(utc(2026, 4, 11, 12, 0).naive_utc()));
        // Right month, weekday, but outside time-of-day.
        assert!(!r.matches_at(utc(2026, 4, 13, 8, 0).naive_utc()));
    }
}
