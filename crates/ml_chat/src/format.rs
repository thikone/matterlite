// SPDX-License-Identifier: AGPL-3.0-only

//! Small text formatters for sizes and times.

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// "0 B", "980 B", "12 KB", "3.4 MB", "1.2 GB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.;
    let mut unit = 0;
    while value >= 1000. && unit < UNITS.len() - 1 {
        value /= 1000.;
        unit += 1;
    }
    if value < 10. {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// When an account was last used, relative to `now` (both ms): "used just now",
/// "used 5 minutes ago", "used 3 hours ago", "used yesterday", "used 12 days ago".
pub fn format_last_used(at: i64, now: i64) -> String {
    let elapsed = (now - at).max(0);
    let minutes = elapsed / 60_000;
    let hours = minutes / 60;
    let days = elapsed / DAY_MS;
    let plural =
        |count: i64, unit: &str| format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    if minutes < 1 {
        "used just now".to_owned()
    } else if hours < 1 {
        format!("used {} ago", plural(minutes, "minute"))
    } else if days < 1 {
        format!("used {} ago", plural(hours, "hour"))
    } else if days == 1 {
        "used yesterday".to_owned()
    } else {
        format!("used {days} days ago")
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
