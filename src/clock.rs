use chrono::{SecondsFormat, Utc};

/// Current UTC time as a real ISO-8601 string, e.g. `2026-08-22T09:14:07Z`.
pub fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Seconds since the Unix epoch, for timestamped filenames.
pub fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_iso8601_is_valid_and_utc() {
        let ts = now_iso8601();
        let parsed = chrono::DateTime::parse_from_rfc3339(&ts).expect("valid rfc3339");
        assert_eq!(parsed.to_rfc3339_opts(SecondsFormat::Secs, true), ts);
        assert!(ts.ends_with('Z'), "UTC designator: {ts}");
        assert_eq!(ts.len(), 20, "YYYY-MM-DDTHH:MM:SSZ: {ts}");
    }

    #[test]
    fn epoch_secs_matches_chrono() {
        let ours = epoch_secs();
        let theirs = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
        assert!((ours.max(theirs) - ours.min(theirs)) < 5);
    }
}
