//! Time in a video: stretches of it, and timestamps as people read and
//! type them.

use serde::{Deserialize, Serialize};

pub const SECONDS_PER_MINUTE: f64 = 60.0;
const MAX_TIMESTAMP_PARTS: usize = 3;

/// A stretch of the video, in seconds from its start. `end` is exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeSpan {
    pub start: f64,
    pub end: f64,
}

impl TimeSpan {
    pub fn seconds(&self) -> f64 {
        self.end - self.start
    }

    /// Whether `time` is inside the span and on neither of its edges. A cut
    /// on an edge would leave nothing on one side of it.
    pub fn has_strictly_inside(&self, time: f64) -> bool {
        time > self.start && time < self.end
    }
}

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

/// Formats a moment the way a player shows it, to the second: "2:40", or
/// "1:03:12" from the first hour on.
pub fn format_clock(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let hours = total_seconds / 3600;
    let minutes = total_seconds / 60 % 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Formats a length of time the way people say it: "50 s", "3 min 44 s",
/// "1 h 05 min 10 s".
pub fn format_length(seconds: f64) -> String {
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

/// Formats a length of time that is only roughly known, as it follows the
/// word "about": "50 s", "3 min 44 s", "1 h 52 min". From an hour on the
/// seconds say nothing and are left out.
pub fn format_rough_length(seconds: f64) -> String {
    const SECONDS_PER_HOUR: f64 = 3600.0;
    let whole_seconds = seconds.max(0.0).round();
    if whole_seconds < SECONDS_PER_HOUR {
        return format_length(seconds);
    }
    let total_minutes = (whole_seconds / SECONDS_PER_MINUTE).round() as u64;
    format!("{} h {:02} min", total_minutes / 60, total_minutes % 60)
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

    const SPAN: TimeSpan = TimeSpan {
        start: 2.0,
        end: 6.0,
    };

    #[test]
    fn a_span_is_as_long_as_the_gap_between_its_edges() {
        assert_eq!(SPAN.seconds(), 4.0);
    }

    #[test]
    fn a_time_between_the_edges_is_strictly_inside_a_span() {
        assert!(SPAN.has_strictly_inside(2.5));
    }

    #[test]
    fn the_edges_of_a_span_are_not_strictly_inside_it() {
        assert!(!SPAN.has_strictly_inside(SPAN.start));
        assert!(!SPAN.has_strictly_inside(SPAN.end));
    }

    #[test]
    fn a_time_that_is_not_a_number_is_not_inside_a_span() {
        assert!(!SPAN.has_strictly_inside(f64::NAN));
    }

    #[test]
    fn timestamps_read_as_hours_minutes_seconds() {
        assert_eq!(format_timestamp(3792.267), "01:03:12.267");
        assert_eq!(format_timestamp(0.0), "00:00:00.000");
    }

    #[test]
    fn a_clock_time_shows_hours_only_when_there_are_some() {
        assert_eq!(format_clock(0.0), "0:00");
        assert_eq!(format_clock(160.2), "2:40");
        assert_eq!(format_clock(3792.0), "1:03:12");
    }

    #[test]
    fn a_clock_time_can_be_typed_back_in() {
        assert_eq!(parse_timestamp(&format_clock(3792.0)), Some(3792.0));
    }

    #[test]
    fn lengths_read_the_way_people_say_them() {
        assert_eq!(format_length(50.0), "50 s");
        assert_eq!(format_length(224.0), "3 min 44 s");
        assert_eq!(format_length(3910.0), "1 h 05 min 10 s");
        assert_eq!(format_length(7719.0), "2 h 08 min 39 s");
    }

    #[test]
    fn rough_lengths_leave_the_seconds_out_from_an_hour_on() {
        assert_eq!(format_rough_length(50.0), "50 s");
        assert_eq!(format_rough_length(224.0), "3 min 44 s");
        assert_eq!(format_rough_length(3569.0), "59 min 29 s");
        assert_eq!(format_rough_length(3600.0), "1 h 00 min");
        assert_eq!(format_rough_length(6749.0), "1 h 52 min");
        assert_eq!(format_rough_length(7190.0), "2 h 00 min");
    }

    #[test]
    fn a_rough_length_that_rounds_to_an_hour_reads_as_an_hour() {
        assert_eq!(format_rough_length(3580.0), "59 min 40 s");
        assert_eq!(format_rough_length(3599.6), "1 h 00 min");
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

    const SECONDS_PER_HOUR: f64 = 3600.0;
    const TWENTY_FIVE_HOURS: f64 = 25.0 * SECONDS_PER_HOUR;

    // The rounding tests below stay clear of exact half milliseconds: no
    // decimal half millisecond is exact in binary, so which way it rounds
    // would depend on the representation.

    #[test]
    fn timestamps_round_to_the_nearest_millisecond() {
        assert_eq!(format_timestamp(1.2344), "00:00:01.234");
        assert_eq!(format_timestamp(1.2346), "00:00:01.235");
    }

    #[test]
    fn a_timestamp_that_rounds_up_to_a_full_second_carries_into_the_next_unit() {
        assert_eq!(format_timestamp(0.9996), "00:00:01.000");
        assert_eq!(format_timestamp(59.9996), "00:01:00.000");
        assert_eq!(format_timestamp(3599.9996), "01:00:00.000");
    }

    #[test]
    fn timestamp_hours_keep_counting_past_a_day() {
        assert_eq!(format_timestamp(TWENTY_FIVE_HOURS), "25:00:00.000");
        assert_eq!(
            format_timestamp(100.0 * SECONDS_PER_HOUR + 0.5),
            "100:00:00.500"
        );
    }

    #[test]
    fn a_negative_timestamp_reads_as_zero() {
        assert_eq!(format_timestamp(-5.0), "00:00:00.000");
    }

    #[test]
    fn a_length_rounds_to_the_nearest_second() {
        assert_eq!(format_length(49.25), "49 s");
        assert_eq!(format_length(49.75), "50 s");
    }

    #[test]
    fn a_length_that_rounds_up_to_a_full_minute_or_hour_carries() {
        assert_eq!(format_length(59.5), "1 min 00 s");
        assert_eq!(format_length(3599.5), "1 h 00 min 00 s");
    }

    #[test]
    fn a_length_under_half_a_second_reads_as_zero_seconds() {
        assert_eq!(format_length(0.0), "0 s");
        assert_eq!(format_length(0.25), "0 s");
    }

    #[test]
    fn a_negative_length_reads_as_zero_seconds() {
        assert_eq!(format_length(-5.0), "0 s");
    }

    #[test]
    fn length_hours_keep_counting_past_a_day() {
        assert_eq!(format_length(TWENTY_FIVE_HOURS), "25 h 00 min 00 s");
    }

    #[test]
    fn parses_parts_with_spaces_around_them() {
        assert_eq!(parse_timestamp(" 1 : 03 : 12 "), Some(3792.0));
    }

    #[test]
    fn parses_a_time_of_zero() {
        assert_eq!(parse_timestamp("0:00"), Some(0.0));
        assert_eq!(parse_timestamp("0"), Some(0.0));
    }

    #[test]
    fn parses_fractional_minutes() {
        assert_eq!(parse_timestamp("1.5:00"), Some(90.0));
    }

    #[test]
    fn parses_seconds_past_59_by_adding_them_up() {
        assert_eq!(parse_timestamp("1:90"), Some(150.0));
    }

    #[test]
    fn rejects_a_time_with_a_missing_part() {
        assert_eq!(parse_timestamp("1:"), None);
        assert_eq!(parse_timestamp(":30"), None);
        assert_eq!(parse_timestamp(":"), None);
    }

    #[test]
    fn rejects_a_negative_part_after_the_first() {
        assert_eq!(parse_timestamp("1:-5"), None);
    }

    #[test]
    fn rejects_numbers_that_are_not_finite() {
        assert_eq!(parse_timestamp("inf"), None);
        assert_eq!(parse_timestamp("NaN"), None);
        assert_eq!(parse_timestamp("1:infinity"), None);
    }

    #[test]
    fn a_formatted_timestamp_parses_back_to_the_same_time() {
        // Whole milliseconds that are exact in binary, so nothing is lost.
        let times = [0.0, 0.125, 59.5, 3792.5, TWENTY_FIVE_HOURS + 0.75];
        for time in times {
            assert_eq!(parse_timestamp(&format_timestamp(time)), Some(time));
        }
    }
}
