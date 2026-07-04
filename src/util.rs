use chrono::{DateTime, Utc};

/// Current wall-clock time as unix seconds.
pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}

/// Format an optional unix timestamp as a human-readable UTC date, or a dash.
pub fn fmt_expiry(expires_at: Option<i64>) -> String {
    match expires_at {
        Some(ts) => match DateTime::<Utc>::from_timestamp(ts, 0) {
            Some(dt) => {
                let days = days_until(ts);
                let when = dt.format("%Y-%m-%d");
                if days < 0 {
                    format!("{when} (expired {} days ago)", -days)
                } else {
                    format!("{when} (in {days} days)")
                }
            }
            None => "unknown".to_string(),
        },
        None => "not checked yet".to_string(),
    }
}

/// Compact expiry prefix for list rows: `YYYY-MM-DD (X days)`.
/// Falls back to placeholders when the expiry is unknown.
pub fn fmt_expiry_line(expires_at: Option<i64>) -> String {
    match expires_at {
        Some(ts) => match DateTime::<Utc>::from_timestamp(ts, 0) {
            Some(dt) => format!("{} ({} days)", dt.format("%Y-%m-%d"), days_until(ts)),
            None => "??????????? (unknown)".to_string(),
        },
        None => "?????????? (no expiry data)".to_string(),
    }
}

/// Whole days from now until `ts` (negative if in the past).
pub fn days_until(ts: i64) -> i64 {
    (ts - now_ts()).div_euclid(86_400)
}
