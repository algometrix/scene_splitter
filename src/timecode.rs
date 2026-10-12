//! Timestamps as people read and type them.

const SECONDS_PER_MINUTE: f64 = 60.0;
const MAX_TIMESTAMP_PARTS: usize = 3;

/// Formats seconds as HH:MM:SS.mmm.
pub fn format_timestamp(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let millis = total_millis % 1000;
    let total_seconds = total_millis / 1000;
    format!(
        "{:02}:{:02}:{:02}.{millis:03}",
        total_seconds / 3600,
        total_seconds / 60 % 60,
        total_seconds % 60
    )
}

/// Formats a length of time the way people say it: "50 s", "3 min 44 s",
/// "1 h 05 min 10 s".
pub fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = total_seconds / 60 % 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours} h {minutes:02} min {seconds:02} s")
    } else if minutes > 0 {
        format!("{minutes} min {seconds:02} s")
    } else {
        format!("{seconds} s")
    }
}

/// Reads "1:03:12", "63:12.5" or "3792" as seconds. Returns `None` for
/// anything that is not a time.
pub fn parse_timestamp(text: &str) -> Option<f64> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.len() > MAX_TIMESTAMP_PARTS {
        return None;
    }
    let mut seconds = 0.0;
    for part in parts {
        let value: f64 = part.trim().parse().ok()?;
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        seconds = seconds * SECONDS_PER_MINUTE + value;
    }
    Some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_read_as_hours_minutes_seconds() {
        assert_eq!(format_timestamp(3792.267), "01:03:12.267");
        assert_eq!(format_timestamp(0.0), "00:00:00.000");
    }

    #[test]
    fn durations_read_the_way_people_say_them() {
        assert_eq!(format_duration(50.0), "50 s");
        assert_eq!(format_duration(224.0), "3 min 44 s");
        assert_eq!(format_duration(3910.0), "1 h 05 min 10 s");
        assert_eq!(format_duration(7719.0), "2 h 08 min 39 s");
    }

    #[test]
    fn parses_hours_minutes_and_seconds() {
        assert_eq!(parse_timestamp("1:03:12"), Some(3792.0));
        assert_eq!(parse_timestamp("01:03:12.5"), Some(3792.5));
    }

    #[test]
    fn parses_minutes_and_plain_seconds() {
        assert_eq!(parse_timestamp("63:12"), Some(3792.0));
        assert_eq!(parse_timestamp(" 90 "), Some(90.0));
    }

    #[test]
    fn rejects_text_that_is_not_a_time() {
        assert_eq!(parse_timestamp(""), None);
        assert_eq!(parse_timestamp("soon"), None);
        assert_eq!(parse_timestamp("1:2:3:4"), None);
        assert_eq!(parse_timestamp("-5"), None);
        assert_eq!(parse_timestamp("1::5"), None);
    }
}
