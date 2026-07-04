use crate::ton::{self, TonClient};
use crate::util;

/// `check <domain>` — query the TON API for a single domain and print its
/// expiry and related info. Does not touch the database or Telegram, so it
/// works with only `TONAPI_*` configured.
pub async fn run_check(domain: &str) -> anyhow::Result<()> {
    let Some(name) = ton::normalize_domain(domain) else {
        anyhow::bail!("'{domain}' is not a valid .ton domain");
    };

    let base =
        std::env::var("TONAPI_BASE_URL").unwrap_or_else(|_| "https://tonapi.io".to_string());
    let api_key = std::env::var("TONAPI_KEY").ok().filter(|s| !s.is_empty());
    let ton = TonClient::new(base, api_key, crate::config::tonapi_rps_from_env())?;

    match ton.fetch_info(&name).await? {
        None => {
            println!("Domain '{name}' not found.");
        }
        Some(info) => {
            println!("Domain:  {}", info.name.as_deref().unwrap_or(&name));
            match info.expiring_at {
                Some(ts) => println!("Expires: {}", util::fmt_expiry(Some(ts))),
                None => println!("Expires: no expiry data"),
            }
            if let Some(item) = &info.item {
                if let Some(addr) = &item.address {
                    println!("NFT:     {addr}");
                }
                if let Some(owner) = &item.owner {
                    let who = owner
                        .name
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .or(owner.address.as_deref())
                        .unwrap_or("unknown");
                    println!("Owner:   {who}");
                }
            }
        }
    }
    Ok(())
}
