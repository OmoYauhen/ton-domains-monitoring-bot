use teloxide::prelude::*;

use crate::bot::format_domain_list;
use crate::config::Config;
use crate::db::Db;
use crate::ton::TonClient;
use crate::util;

/// Refresh expiry dates for every watched domain, then dispatch any due alerts.
pub async fn run_daily(db: &Db, bot: &Bot, ton: &TonClient, cfg: &Config) -> anyhow::Result<()> {
    refresh_all(db, ton).await?;
    send_due_alerts(db, bot, cfg).await?;
    Ok(())
}

/// Query the TON API for each watched domain and persist its expiry.
pub async fn refresh_all(db: &Db, ton: &TonClient) -> anyhow::Result<()> {
    let domains = db.all_watched_domains().await?;
    tracing::info!(count = domains.len(), "refreshing domain expiry dates");

    for d in domains {
        match ton.fetch_info(&d.name).await {
            Ok(Some(info)) => {
                if let Err(e) = db
                    .update_domain_info(
                        d.id,
                        info.expiring_at,
                        info.owner_address(),
                        info.owner_name(),
                    )
                    .await
                {
                    tracing::error!(domain = %d.name, error = %e, "failed to persist domain info");
                }
            }
            // Domain not found: clear expiry so alerts don't fire on stale data.
            Ok(None) => {
                if let Err(e) = db.update_domain_info(d.id, None, None, None).await {
                    tracing::error!(domain = %d.name, error = %e, "failed to clear domain info");
                }
            }
            Err(e) => {
                // Leave the previous value in place; just log the failure.
                tracing::warn!(domain = %d.name, error = %e, "info refresh failed");
            }
        }
    }
    Ok(())
}

/// Send proactive alerts for domains crossing an expiry threshold.
///
/// For each (chat, domain) with a known expiry, we find the smallest
/// configured threshold `T` such that `days_left <= T`. That is the current
/// urgency bucket; we alert once per bucket per expiry value.
pub async fn send_due_alerts(db: &Db, bot: &Bot, cfg: &Config) -> anyhow::Result<()> {
    let Some(&max_threshold) = cfg.alert_thresholds.iter().max() else {
        return Ok(());
    };

    let cutoff = util::now_ts() + max_threshold * 86_400;
    let due = db.subscriptions_expiring_before(cutoff).await?;

    for sub in due {
        let days_left = util::days_until(sub.expires_at);

        // Smallest threshold this domain currently falls under.
        let Some(bucket) = alert_bucket(days_left, &cfg.alert_thresholds) else {
            continue;
        };

        // Only send if we haven't already alerted for this exact bucket+expiry.
        let is_new = db
            .record_alert(sub.chat_id, sub.domain_id, bucket, sub.expires_at)
            .await?;
        if !is_new {
            continue;
        }

        let text = format_alert(&sub.name, days_left, sub.expires_at);
        if let Err(e) = bot.send_message(ChatId(sub.chat_id), text).await {
            tracing::warn!(chat = sub.chat_id, domain = %sub.name, error = %e, "alert send failed");
        }
    }
    Ok(())
}

/// The current urgency bucket for a domain: the smallest configured threshold
/// `T` such that `days_left <= T`, or `None` if it isn't within any alert
/// window yet. `days_left` may be negative (already expired).
pub fn alert_bucket(days_left: i64, thresholds: &[i64]) -> Option<i64> {
    thresholds.iter().copied().filter(|&t| days_left <= t).min()
}

/// Render the alert message for a domain given how many whole days remain
/// until its expiry (negative = already expired).
pub fn format_alert(name: &str, days_left: i64, expires_at: i64) -> String {
    if days_left < 0 {
        format!("⚠️ {} has EXPIRED ({} days ago).", name, -days_left)
    } else if days_left == 0 {
        format!("⚠️ {name} expires TODAY!")
    } else {
        format!(
            "⏰ {} expires in {} day{} ({}).",
            name,
            days_left,
            if days_left == 1 { "" } else { "s" },
            util::fmt_expiry(Some(expires_at))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Thresholds are intentionally unsorted to prove the logic doesn't rely
    // on caller ordering.
    const THRESHOLDS: [i64; 3] = [30, 1, 7];

    #[test]
    fn bucket_picks_smallest_matching_threshold() {
        // Deep inside the 1-day window -> most urgent bucket.
        assert_eq!(alert_bucket(0, &THRESHOLDS), Some(1));
        assert_eq!(alert_bucket(1, &THRESHOLDS), Some(1));
        // Between 1 and 7 days -> the 7-day bucket.
        assert_eq!(alert_bucket(2, &THRESHOLDS), Some(7));
        assert_eq!(alert_bucket(7, &THRESHOLDS), Some(7));
        // Between 7 and 30 days -> the 30-day bucket.
        assert_eq!(alert_bucket(8, &THRESHOLDS), Some(30));
        assert_eq!(alert_bucket(30, &THRESHOLDS), Some(30));
    }

    #[test]
    fn bucket_is_none_outside_all_windows() {
        assert_eq!(alert_bucket(31, &THRESHOLDS), None);
        assert_eq!(alert_bucket(365, &THRESHOLDS), None);
    }

    #[test]
    fn bucket_handles_already_expired() {
        // Negative days still fall in the most urgent bucket.
        assert_eq!(alert_bucket(-1, &THRESHOLDS), Some(1));
        assert_eq!(alert_bucket(-100, &THRESHOLDS), Some(1));
    }

    #[test]
    fn bucket_with_no_thresholds_is_none() {
        assert_eq!(alert_bucket(0, &[]), None);
    }

    #[test]
    fn format_alert_variants() {
        assert!(format_alert("foo.ton", 5, 0).contains("in 5 days"));
        assert!(format_alert("foo.ton", 1, 0).contains("in 1 day ("));
        assert!(format_alert("foo.ton", 0, 0).contains("TODAY"));
        assert!(format_alert("foo.ton", -3, 0).contains("EXPIRED (3 days ago)"));
    }
}

/// Send every user their full domain list (runs on the 1st of the month).
pub async fn run_monthly_report(db: &Db, bot: &Bot) -> anyhow::Result<()> {
    let chats = db.all_chat_ids().await?;
    tracing::info!(count = chats.len(), "sending monthly reports");

    for chat_id in chats {
        let domains = db.list_domains(chat_id).await?;
        let mut text = String::from("📅 Monthly summary\n\n");
        text.push_str(&format_domain_list(&domains));
        if let Err(e) = bot.send_message(ChatId(chat_id), text).await {
            tracing::warn!(chat = chat_id, error = %e, "monthly report send failed");
        }
    }
    Ok(())
}
