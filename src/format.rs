//! How dates, sizes and counts read in the interface (Spanish).

use chrono::{DateTime, Datelike, Local, TimeZone};

const WEEKDAYS: [&str; 7] = [
    "lunes",
    "martes",
    "miércoles",
    "jueves",
    "viernes",
    "sábado",
    "domingo",
];

fn local(ms: u64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(i64::try_from(ms).ok()?).single()
}

/// Day of `ms` relative to `now`: "hoy", "ayer", the weekday within the
/// last week, else "12/10" (or "12/10/25" in another year).
#[must_use]
pub fn day(ms: u64, now: u64) -> String {
    let (Some(at), Some(today)) = (local(ms), local(now)) else {
        return String::new();
    };
    let days = today
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days();
    match days {
        d if d <= 0 => "hoy".to_owned(),
        1 => "ayer".to_owned(),
        2..=6 => WEEKDAYS[at.weekday().num_days_from_monday() as usize].to_owned(),
        _ if at.year() == today.year() => format!("{:02}/{:02}", at.day(), at.month()),
        _ => format!("{:02}/{:02}/{:02}", at.day(), at.month(), at.year() % 100),
    }
}

/// How long ago `ms` was: "Recién", "Hace 5 min", "Hace 2 h", then the
/// day.
#[must_use]
pub fn ago(ms: u64, now: u64) -> String {
    let minutes = now.saturating_sub(ms) / 60_000;
    match minutes {
        0 => "Recién".to_owned(),
        1..=59 => format!("Hace {minutes} min"),
        60..=1439 => format!("Hace {} h", minutes / 60),
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

/// Byte size with one decimal and a comma: "12,4 MB".
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
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// "1 elemento" / "12 elementos".
#[must_use]
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
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
    use super::*;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn sizes_use_a_decimal_comma() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(13_002_342), "12,4 MB");
        assert_eq!(size(1_288_490_189), "1,2 GB");
    }

    #[test]
    fn counts_pick_the_right_noun() {
        assert_eq!(count(1, "elemento", "elementos"), "1 elemento");
        assert_eq!(count(12, "elemento", "elementos"), "12 elementos");
    }

    #[test]
    fn recent_times_read_as_minutes_or_hours() {
        let now = 1_800_000_000_000;
        assert_eq!(ago(now - 30_000, now), "Recién");
        assert_eq!(ago(now - 5 * 60_000, now), "Hace 5 min");
        assert_eq!(ago(now - 2 * 3_600_000, now), "Hace 2 h");
        assert_eq!(day(now, now), "hoy");
        assert_eq!(day(now - DAY, now), "ayer");
    }
}
