use super::format_log_time;
use jiff::civil::date;
use jiff::tz::{Offset, TimeZone};
use jiff::Timestamp;

fn utc(h: i8, m: i8, s: i8, nanos: i32) -> jiff::Zoned {
    date(2024, 3, 5).at(h, m, s, nanos).to_zoned(TimeZone::UTC).unwrap()
}

#[test]
fn log_time_prints_hours_minutes_seconds_milliseconds_in_utc() {
    assert_eq!(format_log_time(&utc(7, 8, 9, 45_000_000)), "07:08:09.045");
}

#[test]
fn log_time_truncates_sub_millisecond_digits_without_rounding() {
    assert_eq!(format_log_time(&utc(23, 59, 58, 999_999_999)), "23:59:58.999");
}

#[test]
fn log_time_zero_pads_milliseconds_to_three_digits() {
    assert_eq!(format_log_time(&utc(0, 0, 0, 0)), "00:00:00.000");
    assert_eq!(format_log_time(&utc(1, 2, 3, 7_000_000)), "01:02:03.007");
}

#[test]
fn log_time_shows_the_wall_clock_of_a_non_zero_offset() {
    let tz = TimeZone::fixed(Offset::from_seconds(5 * 3600 + 30 * 60).unwrap());
    let instant: Timestamp = "2024-03-05T22:00:00.123456Z".parse().unwrap();
    assert_eq!(format_log_time(&instant.to_zoned(tz)), "03:30:00.123");
}

#[test]
fn log_time_shows_the_wall_clock_of_a_negative_offset() {
    let tz = TimeZone::fixed(Offset::from_seconds(-3 * 3600).unwrap());
    let instant: Timestamp = "2024-03-05T01:02:03.500Z".parse().unwrap();
    assert_eq!(format_log_time(&instant.to_zoned(tz)), "22:02:03.500");
}
