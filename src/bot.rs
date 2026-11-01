use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;

use crate::db::Db;
use crate::ton::{self, TonClient};
use crate::util;

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "TON domain expiry monitor. Commands:"
)]
pub enum Command {
    #[command(description = "show this help")]
    Help,
    #[command(description = "start / show welcome")]
    Start,
    #[command(description = "add a domain to watch, e.g. /add foo.ton")]
    Add(String),
    #[command(
        description = "add every .ton domain owned by a domain's wallet, e.g. /addall foo.ton; \
                       without an argument, sweeps the wallets of all domains you watch"
    )]
    Addall(String),
    #[command(description = "stop watching a domain, e.g. /remove foo.ton")]
    Remove(String),
    #[command(description = "list your watched domains and expiry dates")]
    List,
}

/// Build the dispatcher handler tree.
pub fn handler() -> teloxide::dispatching::UpdateHandler<anyhow::Error> {
    Update::filter_message()
        .filter_command::<Command>()
        .endpoint(answer)
}

async fn answer(
    bot: Bot,
    msg: Message,
    cmd: Command,
    db: Db,
    ton: TonClient,
) -> anyhow::Result<()> {
    let chat_id = msg.chat.id;
    match cmd {
        Command::Help => {
            bot.send_message(chat_id, Command::descriptions().to_string())
                .await?;
        }
        Command::Start => {
            bot.send_message(
                chat_id,
                "👋 I watch TON domains and warn you before they expire.\n\n\
                 • /add foo.ton — start watching a domain\n\
                 • /addall foo.ton — watch every domain owned by that domain's wallet\n\
                 • /addall — same, for the wallets of every domain you already watch\n\
                 • /remove foo.ton — stop watching it\n\
                 • /list — see your domains and expiry dates\n\n\
                 I refresh expiry dates daily, alert you as expiry approaches, \
                 and send a full summary on the 1st of each month.",
            )
            .await?;
        }
        Command::Add(arg) => handle_add(&bot, chat_id, &db, &ton, &arg).await?,
        Command::Addall(arg) => handle_addall(&bot, chat_id, &db, &ton, &arg).await?,
        Command::Remove(arg) => handle_remove(&bot, chat_id, &db, &arg).await?,
        Command::List => handle_list(&bot, chat_id, &db).await?,
    }
    Ok(())
}

async fn handle_add(
    bot: &Bot,
    chat_id: ChatId,
    db: &Db,
    ton: &TonClient,
    arg: &str,
) -> anyhow::Result<()> {
    let Some(name) = ton::normalize_domain(arg) else {
        bot.send_message(
            chat_id,
            "Please give a valid TON domain, e.g. `/add foo.ton`.",
        )
        .await?;
        return Ok(());
    };

    let created = db.add_subscription(chat_id.0, &name).await?;
    if !created {
        bot.send_message(chat_id, format!("You're already watching {name}."))
            .await?;
        return Ok(());
    }

    // Fetch info immediately so the user gets instant feedback.
    let reply = match ton.fetch_info(&name).await {
        Ok(Some(info)) => {
            // Persist so /list is correct before the next daily refresh.
            if let Some(id) = db.domain_id(&name).await? {
                db.update_domain_info(
                    id,
                    info.expiring_at,
                    info.owner_address(),
                    info.owner_name(),
                )
                .await?;
            }
            let owner = owner_label(info.owner_name(), info.owner_address());
            match info.expiring_at {
                Some(ts) => format!(
                    "✅ Watching {name}. Expires {}.{}",
                    util::fmt_expiry(Some(ts)),
                    owner
                ),
                None => format!("✅ Watching {name}. No expiry data yet.{owner}"),
            }
        }
        Ok(None) => format!("✅ Watching {name}, but I couldn't find it on-chain yet."),
        Err(e) => {
            tracing::warn!(domain = %name, error = %e, "info fetch on add failed");
            format!("✅ Watching {name}. Lookup failed for now — I'll retry daily.")
        }
    };
    bot.send_message(chat_id, reply).await?;
    Ok(())
}

async fn handle_addall(
    bot: &Bot,
    chat_id: ChatId,
    db: &Db,
    ton: &TonClient,
    arg: &str,
) -> anyhow::Result<()> {
    // Which wallets to sweep: the seed domain's owner, or (no argument) every
    // owner of a domain the user already watches.
    let wallets: Vec<String> = if arg.trim().is_empty() {
        let mut wallets: Vec<String> = db
            .list_domains(chat_id.0)
            .await?
            .into_iter()
            .filter_map(|d| d.owner_address)
            .collect();
        wallets.sort();
        wallets.dedup();
        if wallets.is_empty() {
            bot.send_message(
                chat_id,
                "None of your watched domains has a known owner yet. \
                 Try `/addall foo.ton` instead.",
            )
            .await?;
            return Ok(());
        }
        wallets
    } else {
        let Some(seed) = ton::normalize_domain(arg) else {
            bot.send_message(
                chat_id,
                "Please give a valid TON domain, e.g. `/addall foo.ton`.",
            )
            .await?;
            return Ok(());
        };

        // Resolve the wallet that owns the seed domain.
        let owner_addr = match ton.fetch_info(&seed).await {
            Ok(Some(info)) => info.owner_address().map(str::to_string),
            Ok(None) => {
                bot.send_message(chat_id, format!("Couldn't find {seed} on-chain."))
                    .await?;
                return Ok(());
            }
            Err(e) => {
                tracing::warn!(domain = %seed, error = %e, "addall owner lookup failed");
                bot.send_message(chat_id, "TON API lookup failed — please try again later.")
                    .await?;
                return Ok(());
            }
        };
        let Some(owner_addr) = owner_addr else {
            bot.send_message(chat_id, format!("Couldn't determine the owner of {seed}."))
                .await?;
            return Ok(());
        };
        vec![owner_addr]
    };

    // Enumerate every .ton domain each wallet owns.
    let mut owned: Vec<WalletDomains> = Vec::new();
    let mut failed = 0usize;
    for owner_addr in wallets {
        let items = match ton.fetch_domains_by_owner(&owner_addr).await {
            Ok(items) => items,
            Err(e) => {
                tracing::warn!(owner = %owner_addr, error = %e, "addall owner nfts failed");
                failed += 1;
                continue;
            }
        };

        // The owner's own display name, if the listing resolved one.
        let owner_name: Option<String> = items
            .iter()
            .find_map(|i| i.owner.as_ref().and_then(|o| o.name.as_deref()))
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let mut names: Vec<String> = items
            .into_iter()
            .filter_map(|i| i.dns.as_deref().and_then(ton::normalize_domain))
            .collect();
        names.sort();
        names.dedup();

        if !names.is_empty() {
            owned.push(WalletDomains {
                owner_addr,
                owner_name,
                names,
            });
        }
    }

    if owned.is_empty() {
        let reply = if failed > 0 {
            "Couldn't list the wallet's domains — try again later."
        } else {
            "That wallet doesn't own any .ton domains."
        };
        bot.send_message(chat_id, reply).await?;
        return Ok(());
    }

    let total: usize = owned.iter().map(|w| w.names.len()).sum();
    let wallets_txt = if owned.len() == 1 {
        format!("Wallet {}", owned[0].label())
    } else {
        format!("{} wallets", owned.len())
    };

    // Acknowledge up front: fetching expiries is throttled to ~1 domain/sec.
    bot.send_message(
        chat_id,
        format!(
            "🔍 {} own{} {} .ton domain{}. Fetching expiry dates (~{}s)…",
            wallets_txt,
            if owned.len() == 1 { "s" } else { "" },
            total,
            if total == 1 { "" } else { "s" },
            total,
        ),
    )
    .await?;

    let mut added = 0usize;
    let mut lines = String::new();
    for wallet in &owned {
        if owned.len() > 1 {
            lines.push_str(&format!("\n{}:\n", wallet.label()));
        }
        for name in &wallet.names {
            if db.add_subscription(chat_id.0, name).await? {
                added += 1;
            }

            // Fetch expiry + owner inline (throttled by TonClient).
            let expiry = match ton.fetch_info(name).await {
                Ok(Some(info)) => {
                    if let Some(id) = db.domain_id(name).await? {
                        db.update_domain_info(
                            id,
                            info.expiring_at,
                            info.owner_address().or(Some(wallet.owner_addr.as_str())),
                            info.owner_name().or(wallet.owner_name.as_deref()),
                        )
                        .await?;
                    }
                    info.expiring_at
                }
                // Lookup failed or not found: keep at least the owner we know.
                other => {
                    if let Err(e) = &other {
                        tracing::warn!(domain = %name, error = %e, "addall expiry fetch failed");
                    }
                    if let Some(id) = db.domain_id(name).await? {
                        db.set_owner(id, Some(&wallet.owner_addr), wallet.owner_name.as_deref())
                            .await?;
                    }
                    None
                }
            };
            lines.push_str(&format!("• {} — {}\n", name, util::fmt_expiry(expiry)));
        }
    }

    let owned_by = if owned.len() == 1 {
        format!("owned by {}", owned[0].label())
    } else {
        format!("across {} wallets", owned.len())
    };
    let mut reply = format!(
        "✅ Added {} new domain{} ({} already watched), {}:\n{}",
        added,
        if added == 1 { "" } else { "s" },
        total - added,
        owned_by,
        lines,
    );
    if failed > 0 {
        reply.push_str(&format!(
            "\n⚠️ Couldn't list domains for {} wallet{} — try again later.",
            failed,
            if failed == 1 { "" } else { "s" },
        ));
    }
    bot.send_message(chat_id, reply).await?;
    Ok(())
}

/// The `.ton` domains held by one wallet, as gathered by `/addall`.
struct WalletDomains {
    owner_addr: String,
    owner_name: Option<String>,
    names: Vec<String>,
}

impl WalletDomains {
    /// Display label: resolved name, else the raw address (as `/addall` always showed).
    fn label(&self) -> &str {
        self.owner_name.as_deref().unwrap_or(&self.owner_addr)
    }
}

async fn handle_remove(bot: &Bot, chat_id: ChatId, db: &Db, arg: &str) -> anyhow::Result<()> {
    let name = ton::normalize_domain(arg).unwrap_or_else(|| arg.trim().to_lowercase());
    let removed = db.remove_subscription(chat_id.0, &name).await?;
    let reply = if removed {
        format!("🗑️ Stopped watching {name}.")
    } else {
        format!("You weren't watching {name}.")
    };
    bot.send_message(chat_id, reply).await?;
    Ok(())
}

async fn handle_list(bot: &Bot, chat_id: ChatId, db: &Db) -> anyhow::Result<()> {
    let domains = db.list_domains(chat_id.0).await?;
    bot.send_message(chat_id, format_domain_list(&domains)).await?;
    Ok(())
}

/// Render a user's domain list for both /list and the monthly report.
pub fn format_domain_list(domains: &[crate::db::Domain]) -> String {
    if domains.is_empty() {
        return "You aren't watching any domains yet. Add one with /add foo.ton.".to_string();
    }
    let mut out = format!("Your {} domain{}:\n", domains.len(), if domains.len() == 1 { "" } else { "s" });
    for d in domains {
        // Format: "YYYY-MM-DD (X days) domain.ton (owner.ton)"
        let owner = match owner_display(d.owner_name.as_deref(), d.owner_address.as_deref()) {
            Some(w) => format!(" ({w})"),
            None => String::new(),
        };
        out.push_str(&format!(
            "{} {}{}\n",
            util::fmt_expiry_line(d.expires_at),
            d.name,
            owner
        ));
    }
    out
}

/// Format an owner suffix like " (owner: foo.ton)", preferring the resolved
/// name, falling back to a shortened address, or empty if unknown.
fn owner_label(name: Option<&str>, address: Option<&str>) -> String {
    match owner_display(name, address) {
        Some(w) => format!(" (owner: {w})"),
        None => String::new(),
    }
}

/// The best display string for an owner: resolved name, else shortened address.
fn owner_display(name: Option<&str>, address: Option<&str>) -> Option<String> {
    name.filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| address.map(shorten_address))
}

/// Shorten a raw TON address for display, e.g. `0:9de0…349e`.
fn shorten_address(addr: &str) -> String {
    if addr.len() > 14 {
        format!("{}…{}", &addr[..6], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addall_parses_with_and_without_argument() {
        assert!(matches!(
            Command::parse("/addall foo.ton", "bot"),
            Ok(Command::Addall(a)) if a == "foo.ton"
        ));
        assert!(matches!(
            Command::parse("/addall", "bot"),
            Ok(Command::Addall(a)) if a.is_empty()
        ));
    }
}
