use near_sdk::Timestamp;

pub(super) fn format_timestamp_utc(timestamp: Timestamp) -> String {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

    let seconds = timestamp / NANOS_PER_SECOND;
    let days = (seconds / SECONDS_PER_DAY) as i64;
    let seconds_of_day = seconds % SECONDS_PER_DAY;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    let nanoseconds = timestamp % NANOS_PER_SECOND;
    let fraction = if nanoseconds == 0 {
        String::new()
    } else {
        format!(".{:09}", nanoseconds)
            .trim_end_matches('0')
            .to_string()
    };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{fraction}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted_days = days_since_epoch + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);

    (year, month, day)
}

pub(super) fn format_time_remaining(remaining_nanoseconds: Timestamp) -> String {
    const NANOS_PER_MINUTE: u64 = 60 * 1_000_000_000;
    const MINUTES_PER_DAY: u64 = 24 * 60;

    if remaining_nanoseconds == 0 {
        return "Auction ended".to_string();
    }

    let total_minutes = remaining_nanoseconds.div_ceil(NANOS_PER_MINUTE);
    let days = total_minutes / MINUTES_PER_DAY;
    let hours = total_minutes % MINUTES_PER_DAY / 60;
    let minutes = total_minutes % 60;
    let mut parts = Vec::new();

    if days > 0 {
        parts.push(format!("{days} {}", if days == 1 { "day" } else { "days" }));
    }
    if hours > 0 {
        parts.push(format!(
            "{hours} {}",
            if hours == 1 { "hour" } else { "hours" }
        ));
    }
    if minutes > 0 {
        parts.push(format!(
            "{minutes} {}",
            if minutes == 1 { "minute" } else { "minutes" }
        ));
    }

    if parts.is_empty() {
        "Less than a minute".to_string()
    } else {
        parts.join(" ")
    }
}

pub(super) fn parse_duration(value: &str) -> Result<Timestamp, String> {
    let mut parts = value.split_whitespace();
    let amount = parts
        .next()
        .ok_or_else(|| "Auction duration is required".to_string())?
        .parse::<u64>()
        .map_err(|_| "Auction duration must start with a positive integer".to_string())?;
    let unit = parts
        .next()
        .ok_or_else(|| "Auction duration must include a unit".to_string())?
        .to_ascii_lowercase();
    if parts.next().is_some() {
        return Err("Auction duration must contain an amount and one unit".to_string());
    }
    if amount == 0 {
        return Err("Auction duration must be greater than zero".to_string());
    }

    let unit_nanos = match unit.as_str() {
        "s" | "second" | "seconds" => 1_000_000_000,
        "m" | "minute" | "minutes" => 60 * 1_000_000_000,
        "h" | "hour" | "hours" => 60 * 60 * 1_000_000_000,
        "d" | "day" | "days" => 24 * 60 * 60 * 1_000_000_000,
        _ => return Err("Auction duration unit must be seconds, minutes, hours, or days".into()),
    };

    amount
        .checked_mul(unit_nanos)
        .ok_or_else(|| "Auction duration exceeds the maximum supported value".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_readable_durations() {
        assert_eq!(parse_duration("24 hours").unwrap(), 86_400_000_000_000);
        assert_eq!(parse_duration("1 day").unwrap(), 86_400_000_000_000);
        assert_eq!(parse_duration("30 m").unwrap(), 1_800_000_000_000);
        assert!(parse_duration("0 hours").is_err());
        assert!(parse_duration("24 fortnights").is_err());
        assert!(parse_duration("24 hours and 1 minute").is_err());
    }

    #[test]
    fn formats_remaining_time_as_days_hours_and_minutes() {
        assert_eq!(
            format_time_remaining((2 * 24 * 60 + 22 * 60 + 22) * 60 * 1_000_000_000),
            "2 days 22 hours 22 minutes"
        );
        assert_eq!(
            format_time_remaining((48 * 60 + 22) * 60 * 1_000_000_000),
            "2 days 22 minutes"
        );
        assert_eq!(format_time_remaining(60 * 60 * 1_000_000_000), "1 hour");
        assert_eq!(format_time_remaining(1_000_000_000), "1 minute");
        assert_eq!(format_time_remaining(0), "Auction ended");
    }

    #[test]
    fn formats_timestamps_as_utc_iso_8601() {
        assert_eq!(format_timestamp_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_timestamp_utc(1_793_433_257_560_811_807),
            "2026-10-31T07:54:17.560811807Z"
        );
    }
}
