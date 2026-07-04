use teloxide::prelude::*;
use tokio_cron_scheduler::{Job, JobScheduler};

use crate::config::Config;
use crate::db::Db;
use crate::jobs;
use crate::ton::TonClient;

/// Register the daily (refresh + alerts) and monthly (report) cron jobs
/// and start the scheduler. Returns the scheduler so the caller keeps it alive.
pub async fn start(
    cfg: &Config,
    db: Db,
    bot: Bot,
    ton: TonClient,
) -> anyhow::Result<JobScheduler> {
    let sched = JobScheduler::new().await?;

    // Daily: refresh expiry dates, then send any due alerts.
    {
        let db = db.clone();
        let bot = bot.clone();
        let ton = ton.clone();
        let cfg = cfg.clone();
        let daily_cron = cfg.daily_cron.clone();
        let job = Job::new_async(daily_cron.as_str(), move |_uuid, _l| {
            let db = db.clone();
            let bot = bot.clone();
            let ton = ton.clone();
            let cfg = cfg.clone();
            Box::pin(async move {
                tracing::info!("daily job started");
                if let Err(e) = jobs::run_daily(&db, &bot, &ton, &cfg).await {
                    tracing::error!(error = %e, "daily job failed");
                }
            })
        })?;
        sched.add(job).await?;
    }

    // Monthly: full domain summary to each user on the 1st.
    {
        let db = db.clone();
        let bot = bot.clone();
        let monthly_cron = cfg.monthly_cron.clone();
        let job = Job::new_async(monthly_cron.as_str(), move |_uuid, _l| {
            let db = db.clone();
            let bot = bot.clone();
            Box::pin(async move {
                tracing::info!("monthly report job started");
                if let Err(e) = jobs::run_monthly_report(&db, &bot).await {
                    tracing::error!(error = %e, "monthly report job failed");
                }
            })
        })?;
        sched.add(job).await?;
    }

    sched.start().await?;
    Ok(sched)
}
