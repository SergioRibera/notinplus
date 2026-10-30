//! How dates, sizes and counts read in the interface, in the active
//! language.

use chrono::{DateTime, Datelike, Local, TimeZone};
use notein_i18n::tr;

fn local(ms: u64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(i64::try_from(ms).ok()?).single()
}

/// Day of `ms` relative to `now`: "today", "yesterday", the weekday
/// within the last week, else the short date (with the year when it
/// is another one).
#[must_use]
pub fn day(ms: u64, now: u64) -> String {
    let (Some(at), Some(today)) = (local(ms), local(now)) else {
        return String::new();
    };
    let days = today
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days();
    let day = format!("{:02}", at.day());
    let month = format!("{:02}", at.month());
    match days {
        d if d <= 0 => tr!("format-today"),
        1 => tr!("format-yesterday"),
        2..=6 => tr!("format-weekday", "day" => at.weekday().number_from_monday()),
        _ if at.year() == today.year() => tr!("format-date", "day" => day, "month" => month),
        _ => tr!(
            "format-date-year",
            "day" => day,
            "month" => month,
            "year" => format!("{:02}", at.year() % 100),
        ),
    }
}

/// How long ago `ms` was: "Just now", "5 min ago", "2 h ago", then the
/// capitalised day.
#[must_use]
pub fn ago(ms: u64, now: u64) -> String {
    let minutes = now.saturating_sub(ms) / 60_000;
    match minutes {
        0 => tr!("format-just-now"),
        1..=59 => tr!("format-minutes-ago", "minutes" => minutes),
        60..=1439 => tr!("format-hours-ago", "hours" => minutes / 60),
        _ => {
            let shown = day(ms, now);
            let mut chars = shown.chars();
            chars.next().map_or_else(
                || shown.clone(),
                |first| first.to_uppercase().collect::<String>() + chars.as_str(),
            )
        }
    }
}

/// Byte size with one decimal in the language's notation: "12.4 MB",
/// "12,4 MB".
#[must_use]
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    #[allow(clippy::cast_precision_loss)] // display only
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', &tr!("format-decimal-separator"))
    }
}

/// Unix millis now.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use notein_i18n::Lang;

    use super::*;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn sizes_use_the_language_decimal_mark() {
        Lang::Es.make_current();
        assert_eq!(size(512), "512 B");
        assert_eq!(size(13_002_342), "12,4 MB");
        assert_eq!(size(1_288_490_189), "1,2 GB");
    }

    #[test]
    fn recent_times_read_as_minutes_or_hours() {
        Lang::Es.make_current();
        let now = 1_800_000_000_000;
        assert_eq!(ago(now - 30_000, now), "Hace un momento");
        assert_eq!(ago(now - 5 * 60_000, now), "Hace 5 min");
        assert_eq!(ago(now - 2 * 3_600_000, now), "Hace 2 h");
        assert_eq!(day(now, now), "hoy");
        assert_eq!(day(now - DAY, now), "ayer");
    }
}
