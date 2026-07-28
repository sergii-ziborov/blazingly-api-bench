use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

/// RFC 3339 with a `Z` offset and second precision, matching the seed corpus.
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn normalize(value: DateTime<FixedOffset>) -> String {
    value.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse(value: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(value).ok()
}

pub fn more_than_a_year_ahead(value: DateTime<FixedOffset>) -> bool {
    value.with_timezone(&Utc) > Utc::now() + chrono::Duration::days(365)
}
