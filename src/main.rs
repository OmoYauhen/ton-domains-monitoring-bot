mod bot;
mod cli;
mod config;
mod db;
mod jobs;
mod scheduler;
mod ton;
mod util;

use teloxide::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Db;
use crate::ton::TonClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    // Subcommand handling: `check <domain>` runs a one-off lookup and exits.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("check") {
        match args.get(2) {
            Some(domain) => return cli::run_check(domain).await,
            None => {
                eprintln!("usage: {} check <domain.ton>", args[0]);
                std::process::exit(2);
            }
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,teloxide=warn")),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(db = %cfg.database_url, tonapi = %cfg.tonapi_base, "starting");

    let db = Db::connect(&cfg.database_url).await?;
    let ton = TonClient::new(cfg.tonapi_base.clone(), cfg.tonapi_key.clone(), cfg.tonapi_rps)?;
    let bot = Bot::new(&cfg.bot_token);

    // Start cron scheduler (kept alive for the process lifetime).
    let _sched = scheduler::start(&cfg, db.clone(), bot.clone(), ton.clone()).await?;

    tracing::info!("bot dispatch starting");
    Dispatcher::builder(bot, bot::handler())
        .dependencies(dptree::deps![db, ton])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
