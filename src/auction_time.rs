use near_sdk::Timestamp;

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
}
