use chrono::{DateTime, Datelike, FixedOffset, Utc};

pub fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() {
        return None;
    }
    let parsed = DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .map(|n| n.and_utc())
        })
        .ok()?;
    (parsed.year() >= 2000).then_some(parsed)
}

pub fn age_since(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (now - then).num_seconds().max(0);
    for (limit, unit) in [(86400, "d"), (3600, "h"), (60, "m")] {
        if seconds >= limit {
            return format!("{}{unit}", seconds / limit);
        }
    }
    format!("{seconds}s")
}

pub fn age(value: &str) -> String {
    parse_iso(value).map_or_else(|| "-".into(), |t| age_since(t, Utc::now()))
}

pub fn helm_updated_age(value: &str) -> String {
    let mut parts = value.split_whitespace();
    let (Some(d), Some(t), Some(z)) = (parts.next(), parts.next(), parts.next()) else {
        return "-".into();
    };
    let t = t.split('.').next().unwrap_or(t);
    DateTime::<FixedOffset>::parse_from_str(&format!("{d} {t} {z}"), "%Y-%m-%d %H:%M:%S %z")
        .map(|p| age_since(p.with_timezone(&Utc), Utc::now()))
        .unwrap_or_else(|_| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        let now = parse_iso("2026-01-02T00:00:00Z").unwrap();
        assert_eq!(
            age_since(parse_iso("2026-01-01T00:00:00Z").unwrap(), now),
            "1d"
        );
        assert_eq!(
            age_since(parse_iso("2026-01-01T23:00:00Z").unwrap(), now),
            "1h"
        );
        assert_eq!(
            age_since(parse_iso("2026-01-01T23:59:30Z").unwrap(), now),
            "30s"
        );
        assert_eq!(age(""), "-");
        assert_eq!(age("0001-01-01T00:00:00Z"), "-");
        assert_eq!(helm_updated_age("kaputt"), "-");
        assert_ne!(
            helm_updated_age("2025-03-01 10:00:00.123456 +0100 CET"),
            "-"
        );
    }
}
