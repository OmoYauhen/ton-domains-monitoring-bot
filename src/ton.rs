use std::sync::Arc;

use anyhow::Context;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant, sleep};

/// Client for a tonapi.io-compatible TON API.
///
/// All requests pass through a shared throttle so the whole process stays
/// within the API's rate limit, regardless of how many `TonClient` clones
/// (bot, scheduler, CLI) issue calls concurrently.
#[derive(Clone)]
pub struct TonClient {
    http: reqwest::Client,
    base: String,
    api_key: Option<String>,
    min_interval: Duration,
    /// Timestamp of the last dispatched request, shared across clones.
    last_request: Arc<Mutex<Option<Instant>>>,
}

/// Subset of the TON API `/v2/dns/{name}` response we care about.
#[derive(Debug, Deserialize)]
pub struct DomainInfo {
    #[serde(default)]
    pub name: Option<String>,
    /// Unix timestamp (seconds) at which the domain expires. Absent for some
    /// non-expiring names.
    #[serde(default)]
    pub expiring_at: Option<i64>,
    #[serde(default)]
    pub item: Option<NftItem>,
}

/// The NFT that backs the domain.
#[derive(Debug, Deserialize)]
pub struct NftItem {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub owner: Option<AccountAddress>,
    /// The domain name this NFT represents (present for TON DNS items).
    #[serde(default)]
    pub dns: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AccountAddress {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl DomainInfo {
    /// Raw owner wallet address, if known.
    pub fn owner_address(&self) -> Option<&str> {
        self.item.as_ref()?.owner.as_ref()?.address.as_deref()
    }

    /// Resolved owner name (e.g. its own `.ton`), if any.
    pub fn owner_name(&self) -> Option<&str> {
        self.item
            .as_ref()?
            .owner
            .as_ref()?
            .name
            .as_deref()
            .filter(|s| !s.is_empty())
    }
}

/// Response wrapper for the account-NFTs endpoint.
#[derive(Debug, Deserialize)]
struct NftItemsResponse {
    #[serde(default)]
    nft_items: Vec<NftItem>,
}

/// The on-chain collection address for TON DNS (`.ton`) domains.
const TON_DNS_COLLECTION: &str = "0:b774d95eb20543f186c06b371ab88ad704f7e256130caf96189368a7d0cb6ccf";

impl TonClient {
    /// Build a client limited to `rps` requests per second (values `<= 0`
    /// disable throttling).
    pub fn new(base: impl Into<String>, api_key: Option<String>, rps: f64) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("ton-domains-monitoring")
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .context("building HTTP client")?;
        let min_interval = if rps > 0.0 {
            Duration::from_secs_f64(1.0 / rps)
        } else {
            Duration::ZERO
        };
        Ok(Self {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            api_key,
            min_interval,
            last_request: Arc::new(Mutex::new(None)),
        })
    }

    /// Block until enough time has passed since the previous request to
    /// respect `min_interval`. The lock is held across the sleep, so callers
    /// are serialized into a strict global rate limit.
    async fn throttle(&self) {
        if self.min_interval.is_zero() {
            return;
        }
        let mut last = self.last_request.lock().await;
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < self.min_interval {
                sleep(self.min_interval - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Fetch full domain info from the TON API.
    ///
    /// Returns `Ok(None)` when the domain is not found, and `Err` on
    /// transport/HTTP errors so callers can distinguish "no data" from
    /// "check failed".
    pub async fn fetch_info(&self, domain: &str) -> anyhow::Result<Option<DomainInfo>> {
        let url = format!("{}/v2/dns/{}", self.base, domain);
        let mut req = self.http.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        self.throttle().await;
        let resp = req.send().await.context("sending TON API request")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp
            .error_for_status()
            .context("TON API returned an error status")?;

        let info: DomainInfo = resp.json().await.context("decoding TON API response")?;
        Ok(Some(info))
    }

    /// List every TON DNS (`.ton`) domain owned by a wallet address.
    ///
    /// Returns the backing NFT items; use `.dns` for the domain name.
    pub async fn fetch_domains_by_owner(&self, owner: &str) -> anyhow::Result<Vec<NftItem>> {
        let url = format!(
            "{}/v2/accounts/{}/nfts?collection={}&limit=1000&indirect_ownership=false",
            self.base, owner, TON_DNS_COLLECTION
        );
        let mut req = self.http.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        self.throttle().await;
        let resp = req
            .send()
            .await
            .context("sending TON API request")?
            .error_for_status()
            .context("TON API returned an error status")?;

        let body: NftItemsResponse = resp.json().await.context("decoding NFT list")?;
        Ok(body.nft_items)
    }
}

/// Normalize and validate a user-supplied domain.
///
/// Lowercases, strips whitespace and a leading `@`, and requires a `.ton`
/// suffix. Only `.ton` domains are supported — `.t.me` names don't expire, so
/// there's nothing to monitor. Returns `None` if it isn't a valid `.ton` domain.
pub fn normalize_domain(input: &str) -> Option<String> {
    let d = input.trim().trim_start_matches('@').to_lowercase();
    if d.is_empty() || d.contains(char::is_whitespace) {
        return None;
    }
    // Require a non-empty label before the ".ton" suffix.
    if d.ends_with(".ton") && d.len() > ".ton".len() {
        Some(d)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_domain;

    #[test]
    fn accepts_and_lowercases_ton_domains() {
        assert_eq!(normalize_domain("foo.ton"), Some("foo.ton".to_string()));
        assert_eq!(normalize_domain("Foo.TON"), Some("foo.ton".to_string()));
    }

    #[test]
    fn trims_whitespace_and_leading_at() {
        assert_eq!(normalize_domain("  foo.ton  "), Some("foo.ton".to_string()));
        assert_eq!(normalize_domain("@foo.ton"), Some("foo.ton".to_string()));
    }

    #[test]
    fn rejects_non_ton_suffixes() {
        // t.me names don't expire, so they aren't supported.
        assert_eq!(normalize_domain("foo.t.me"), None);
        assert_eq!(normalize_domain("foo.com"), None);
        assert_eq!(normalize_domain("foo"), None);
    }

    #[test]
    fn rejects_empty_label_and_blank_input() {
        assert_eq!(normalize_domain(".ton"), None);
        assert_eq!(normalize_domain(""), None);
        assert_eq!(normalize_domain("   "), None);
    }

    #[test]
    fn rejects_internal_whitespace() {
        assert_eq!(normalize_domain("foo bar.ton"), None);
    }
}
