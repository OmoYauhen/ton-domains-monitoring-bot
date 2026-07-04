use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::str::FromStr;

/// A domain row joined with the current user's view of it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub id: i64,
    pub name: String,
    pub expires_at: Option<i64>,
    pub owner_address: Option<String>,
    pub owner_name: Option<String>,
}

/// A domain due for an alert: who to notify, which domain, and its expiry.
#[derive(Debug, Clone)]
pub struct DueSubscription {
    pub chat_id: i64,
    pub domain_id: i64,
    pub name: String,
    pub expires_at: i64,
}

/// Thin data-access wrapper around a SQLite connection pool.
#[derive(Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Connect (creating the database file if missing) and run migrations.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        // Ensure the parent directory of the SQLite file exists.
        let path = url
            .trim_start_matches("sqlite://")
            .trim_start_matches("sqlite:");
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating db directory {}", parent.display()))?;
            }
        }

        let opts = SqliteConnectOptions::from_str(url)
            .with_context(|| format!("parsing DATABASE_URL: {url}"))?
            .create_if_missing(true)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .context("connecting to SQLite")?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running migrations")?;

        Ok(Self { pool })
    }

    /// Subscribe a chat to a domain, inserting the domain if new.
    /// Returns `true` if this created a new subscription, `false` if it existed.
    pub async fn add_subscription(&self, chat_id: i64, name: &str) -> anyhow::Result<bool> {
        let now = crate::util::now_ts();
        let mut tx = self.pool.begin().await?;

        sqlx::query("INSERT OR IGNORE INTO domains (name, created_at) VALUES (?, ?)")
            .bind(name)
            .bind(now)
            .execute(&mut *tx)
            .await?;

        let domain_id: i64 = sqlx::query("SELECT id FROM domains WHERE name = ?")
            .bind(name)
            .fetch_one(&mut *tx)
            .await?
            .get("id");

        let res = sqlx::query(
            "INSERT OR IGNORE INTO subscriptions (chat_id, domain_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(chat_id)
        .bind(domain_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(res.rows_affected() > 0)
    }

    /// Look up a domain's id by name.
    pub async fn domain_id(&self, name: &str) -> anyhow::Result<Option<i64>> {
        let row = sqlx::query("SELECT id FROM domains WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get("id")))
    }

    /// Remove a chat's subscription to a domain. Returns `true` if one was removed.
    pub async fn remove_subscription(&self, chat_id: i64, name: &str) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "DELETE FROM subscriptions
             WHERE chat_id = ?
               AND domain_id = (SELECT id FROM domains WHERE name = ?)",
        )
        .bind(chat_id)
        .bind(name)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// List all domains a chat is subscribed to, alphabetically.
    pub async fn list_domains(&self, chat_id: i64) -> anyhow::Result<Vec<Domain>> {
        let rows = sqlx::query(
            "SELECT d.id, d.name, d.expires_at, d.owner_address, d.owner_name
             FROM domains d
             JOIN subscriptions s ON s.domain_id = d.id
             WHERE s.chat_id = ?
             ORDER BY d.expires_at IS NULL, d.expires_at ASC, d.name",
        )
        .bind(chat_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_domain).collect())
    }

    /// All domains that at least one chat is watching (for the refresh job).
    pub async fn all_watched_domains(&self) -> anyhow::Result<Vec<Domain>> {
        let rows = sqlx::query(
            "SELECT id, name, expires_at, owner_address, owner_name
             FROM domains
             WHERE id IN (SELECT DISTINCT domain_id FROM subscriptions)
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_domain).collect())
    }

    /// Update a domain's expiry (may be NULL) and owner, and mark it checked.
    pub async fn update_domain_info(
        &self,
        domain_id: i64,
        expires_at: Option<i64>,
        owner_address: Option<&str>,
        owner_name: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE domains
             SET expires_at = ?, owner_address = ?, owner_name = ?, last_checked_at = ?
             WHERE id = ?",
        )
        .bind(expires_at)
        .bind(owner_address)
        .bind(owner_name)
        .bind(crate::util::now_ts())
        .bind(domain_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Update only a domain's owner columns, leaving expiry untouched.
    /// Used by `/addall`, where expiry is deferred to the daily refresh.
    pub async fn set_owner(
        &self,
        domain_id: i64,
        owner_address: Option<&str>,
        owner_name: Option<&str>,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE domains SET owner_address = ?, owner_name = ? WHERE id = ?")
            .bind(owner_address)
            .bind(owner_name)
            .bind(domain_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Distinct chat ids that have at least one subscription (for monthly report).
    pub async fn all_chat_ids(&self) -> anyhow::Result<Vec<i64>> {
        let rows = sqlx::query("SELECT DISTINCT chat_id FROM subscriptions")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|r| r.get("chat_id")).collect())
    }

    /// All (chat, domain) pairs whose domain expires at or before `cutoff`,
    /// for domains with a known expiry. Used to compute pending alerts.
    pub async fn subscriptions_expiring_before(
        &self,
        cutoff: i64,
    ) -> anyhow::Result<Vec<DueSubscription>> {
        let rows = sqlx::query(
            "SELECT s.chat_id, d.id AS domain_id, d.name, d.expires_at
             FROM subscriptions s
             JOIN domains d ON d.id = s.domain_id
             WHERE d.expires_at IS NOT NULL AND d.expires_at <= ?",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| DueSubscription {
                chat_id: r.get("chat_id"),
                domain_id: r.get("domain_id"),
                name: r.get("name"),
                expires_at: r.get("expires_at"),
            })
            .collect())
    }

    /// Record that an alert was sent. Returns `true` if newly recorded,
    /// `false` if an identical alert had already been logged.
    pub async fn record_alert(
        &self,
        chat_id: i64,
        domain_id: i64,
        threshold_days: i64,
        expires_at: i64,
    ) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "INSERT OR IGNORE INTO alerts_sent
                (chat_id, domain_id, threshold_days, expires_at, sent_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(chat_id)
        .bind(domain_id)
        .bind(threshold_days)
        .bind(expires_at)
        .bind(crate::util::now_ts())
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

/// Build a `Domain` from a query row selecting the standard domain columns.
fn row_to_domain(r: sqlx::sqlite::SqliteRow) -> Domain {
    Domain {
        id: r.get("id"),
        name: r.get("name"),
        expires_at: r.get("expires_at"),
        owner_address: r.get("owner_address"),
        owner_name: r.get("owner_name"),
    }
}
