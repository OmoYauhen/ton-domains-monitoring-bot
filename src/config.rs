use anyhow::Context;

/// Runtime configuration, sourced from environment variables (and `.env`).
#[derive(Clone, Debug)]
pub struct Config {
    /// Telegram bot token from @BotFather.
    pub bot_token: String,
    /// SQLx connection string, e.g. `sqlite://data/ton.db`.
    pub database_url: String,
    /// Base URL of the TON API (tonapi.io compatible).
    pub tonapi_base: String,
    /// Optional TON API bearer token for higher rate limits.
    pub tonapi_key: Option<String>,
    /// Max TON API requests per second (throttled globally).
    pub tonapi_rps: f64,
    /// Cron for the daily refresh + alert job (6-field: sec min hour dom mon dow).
    pub daily_cron: String,
    /// Cron for the monthly report job.
    pub monthly_cron: String,
    /// Days-before-expiry thresholds that trigger a proactive alert.
    pub alert_thresholds: Vec<i64>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN")
            .context("TELEGRAM_BOT_TOKEN is required")?;

        let database_url =
            std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://data/ton.db".to_string());

        let tonapi_base =
            std::env::var("TONAPI_BASE_URL").unwrap_or_else(|_| "https://tonapi.io".to_string());

        let tonapi_key = std::env::var("TONAPI_KEY").ok().filter(|s| !s.is_empty());

        let tonapi_rps = tonapi_rps_from_env();

        // Daily at 03:00 by default.
        let daily_cron =
            std::env::var("DAILY_CRON").unwrap_or_else(|_| "0 0 3 * * *".to_string());

        // 1st day of every month at 09:00 by default.
        let monthly_cron =
            std::env::var("MONTHLY_CRON").unwrap_or_else(|_| "0 0 9 1 * *".to_string());

        let alert_thresholds = std::env::var("ALERT_THRESHOLDS")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter_map(|p| p.trim().parse::<i64>().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| vec![30, 7, 1]);

        Ok(Self {
            bot_token,
            database_url,
            tonapi_base,
            tonapi_key,
            tonapi_rps,
            daily_cron,
            monthly_cron,
            alert_thresholds,
        })
    }
}

/// Read `TONAPI_RPS` (requests per second), defaulting to 1.0 to match the
/// TON API rate limit. Shared by the bot path and the `check` CLI path.
pub fn tonapi_rps_from_env() -> f64 {
    std::env::var("TONAPI_RPS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|&r| r > 0.0)
        .unwrap_or(1.0)
}
