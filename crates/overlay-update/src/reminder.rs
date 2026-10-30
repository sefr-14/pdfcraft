//! "Remind me later" (PRD §12): which update was postponed, until when, and whether a check may
//! run on its own. Kept in the app's settings as one JSON object; anything malformed falls back to
//! the defaults. A due reminder only allows one check: it never installs anything.

use chrono::{DateTime, Duration, Months, Utc};
use serde_json::{Value, json};

/// How long "Remind me later" postpones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interval {
    Day,
    #[default]
    Week,
    Month,
}

impl Interval {
    pub const ALL: [Interval; 3] = [Interval::Day, Interval::Week, Interval::Month];

    /// The settings token.
    pub fn token(self) -> &'static str {
        match self {
            Interval::Day => "day",
            Interval::Week => "week",
            Interval::Month => "month",
        }
    }

    pub fn from_token(s: &str) -> Option<Interval> {
        Interval::ALL.into_iter().find(|i| i.token() == s)
    }

    /// `t` plus the interval; a month keeps the day where it exists and clamps to the month's
    /// end otherwise (31 January → 28 or 29 February). Saturates at chrono's range.
    pub fn after(self, t: DateTime<Utc>) -> DateTime<Utc> {
        let later = match self {
            Interval::Day => t.checked_add_signed(Duration::days(1)),
            Interval::Week => t.checked_add_signed(Duration::weeks(1)),
            Interval::Month => t.checked_add_months(Months::new(1)),
        };
        later.unwrap_or(DateTime::<Utc>::MAX_UTC)
    }
}

/// The saved reminder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reminder {
    /// The interval selected last (the dialog's default).
    pub interval: Interval,
    /// The approved sequence that was postponed.
    pub postponed_seq: Option<u64>,
    /// When the app may check on its own again.
    pub postponed_until: Option<DateTime<Utc>>,
    /// The highest approved sequence seen in a valid envelope (replay protection).
    pub highest_seen_seq: u64,
}

impl Reminder {
    pub fn to_json(&self) -> Value {
        json!({
            "remind_interval": self.interval.token(),
            "postponed_seq": self.postponed_seq,
            "postponed_until": self.postponed_until.map(|t| t.to_rfc3339()),
            "highest_seen_seq": self.highest_seen_seq,
        })
    }

    /// Read saved settings leniently: a malformed field is dropped, never an error.
    pub fn from_json(v: &Value) -> Reminder {
        let interval = field(v, "remind_interval").as_str().and_then(Interval::from_token).unwrap_or_default();
        let postponed_until = field(v, "postponed_until").as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc));
        let postponed_seq = field(v, "postponed_seq").as_u64();
        // A postponement is only meaningful whole.
        let (postponed_seq, postponed_until) = match (postponed_seq, postponed_until) {
            (Some(s), Some(t)) => (Some(s), Some(t)),
            _ => (None, None),
        };
        Reminder { interval, postponed_seq, postponed_until, highest_seen_seq: field(v, "highest_seen_seq").as_u64().unwrap_or(0) }
    }

    /// "Remind me later" (or dismissing the dialog) for update `seq`.
    pub fn postpone(&mut self, seq: u64, interval: Interval, now: DateTime<Utc>) {
        self.interval = interval;
        self.postponed_seq = Some(seq);
        self.postponed_until = Some(interval.after(now));
    }

    /// A clock set back would otherwise hold the reminder far in the future: no postponement
    /// lasts longer than its interval from now.
    pub fn normalize_clock(&mut self, now: DateTime<Utc>) {
        if let Some(until) = self.postponed_until {
            let latest = self.interval.after(now);
            if until > latest {
                self.postponed_until = Some(latest);
            }
        }
    }

    /// Whether the app may check on its own now: only when a postponed reminder has come due.
    /// With nothing postponed there is no automatic check (the owner's OV8 decision).
    pub fn due(&self, now: DateTime<Utc>) -> bool {
        self.postponed_until.is_some_and(|t| t <= now)
    }

    /// Whether an automatic check that found `seq` may show it: when the reminder is due, or the
    /// update is newer than the one postponed.
    pub fn may_prompt(&self, seq: u64, now: DateTime<Utc>) -> bool {
        self.due(now) || self.postponed_seq.is_none_or(|p| seq > p)
    }

    /// Note a valid envelope's sequence.
    pub fn saw(&mut self, seq: u64) {
        self.highest_seen_seq = self.highest_seen_seq.max(seq);
    }

    /// After an update to `installed_seq`, forget postponements it covers.
    pub fn installed(&mut self, installed_seq: u64) {
        self.saw(installed_seq);
        if self.postponed_seq.is_some_and(|p| p <= installed_seq) {
            self.postponed_seq = None;
            self.postponed_until = None;
        }
    }
}

/// A JSON object's field, `null` when absent (never panics, unlike indexing).
fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn intervals_add_a_day_a_week_or_a_calendar_month() {
        let now = t("2026-01-31T09:00:00Z");
        assert_eq!(Interval::Day.after(now), t("2026-02-01T09:00:00Z"));
        assert_eq!(Interval::Week.after(now), t("2026-02-07T09:00:00Z"));
        assert_eq!(Interval::Month.after(now), t("2026-02-28T09:00:00Z"));
        assert_eq!(Interval::Month.after(t("2028-01-31T09:00:00Z")), t("2028-02-29T09:00:00Z"));
        assert_eq!(Interval::Month.after(DateTime::<Utc>::MAX_UTC), DateTime::<Utc>::MAX_UTC);
        assert_eq!(Interval::default(), Interval::Week);
    }

    #[test]
    fn a_postponement_survives_a_round_trip_and_comes_due() {
        let now = t("2026-11-01T12:00:00Z");
        for interval in Interval::ALL {
            let mut r = Reminder::default();
            r.postpone(7, interval, now);
            let back = Reminder::from_json(&serde_json::from_str(&r.to_json().to_string()).unwrap());
            assert_eq!(back, r);
            assert!(!back.due(now));
            assert!(!back.may_prompt(7, now));
            let until = interval.after(now);
            assert!(!back.due(until - Duration::seconds(1)));
            assert!(back.due(until));
            assert!(back.may_prompt(7, until));
        }
    }

    #[test]
    fn nothing_postponed_means_no_automatic_check() {
        let r = Reminder::default();
        assert!(!r.due(t("2030-01-01T00:00:00Z")));
    }

    #[test]
    fn a_newer_update_may_be_offered_while_an_older_one_is_postponed() {
        let now = t("2026-11-01T12:00:00Z");
        let mut r = Reminder::default();
        r.postpone(7, Interval::Month, now);
        assert!(!r.may_prompt(7, now));
        assert!(r.may_prompt(8, now));
    }

    #[test]
    fn an_installation_clears_what_it_covers() {
        let now = t("2026-11-01T12:00:00Z");
        let mut r = Reminder::default();
        r.postpone(7, Interval::Week, now);
        r.installed(6);
        assert_eq!(r.postponed_seq, Some(7));
        r.installed(7);
        assert_eq!((r.postponed_seq, r.postponed_until, r.highest_seen_seq), (None, None, 7));
        assert_eq!(r.interval, Interval::Week, "the chosen interval stays the default");
    }

    #[test]
    fn a_clock_set_back_is_clamped() {
        let mut r = Reminder::default();
        r.postpone(7, Interval::Day, t("2030-01-01T00:00:00Z"));
        let now = t("2026-11-01T12:00:00Z");
        r.normalize_clock(now);
        assert_eq!(r.postponed_until, Some(t("2026-11-02T12:00:00Z")));
    }

    #[test]
    fn malformed_settings_fall_back_to_defaults() {
        for v in [
            json!(null),
            json!("week"),
            json!({"remind_interval": "year", "postponed_seq": -1, "postponed_until": "soon", "highest_seen_seq": "9"}),
            json!({"postponed_seq": 7}),
            json!({"postponed_until": "2026-11-01T12:00:00Z"}),
        ] {
            assert_eq!(Reminder::from_json(&v), Reminder::default(), "{v}");
        }
    }
}
