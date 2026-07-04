# CLAUDE.md

Guidance for working in this repository.

## What this is

A Rust Telegram bot that monitors TON `.ton` domain expiry dates. Users
add/remove/list domains via the bot; a background scheduler refreshes expiries
daily, sends proactive alerts before expiry, and a monthly summary on the 1st.
Storage is SQLite. Everything runs in a single binary — the bot dispatcher and
the cron scheduler share `Db`, `Bot`, and `TonClient` handles (all cheaply
`Clone`able).

Each user is notified only about domains **they** added: `subscriptions` stores
`(chat_id, domain_id)`, alert queries join through it, and the monthly report
iterates per `chat_id`. A domain is stored once and shared across all its
watchers.

## Architecture

| File                  | Responsibility                                                        |
| --------------------- | -------------------------------------------------------------------- |
| `src/main.rs`         | Entry point: dispatch `check` subcommand, else load config, connect DB, build bot, start scheduler + dispatcher. |
| `src/cli.rs`          | `check <domain>` one-off lookup (TON API only, no DB/Telegram).       |
| `src/config.rs`       | `Config::from_env()` — all tunables come from env vars / `.env`.      |
| `src/db.rs`           | `Db` — SQLite pool + all queries (runtime `sqlx::query`, not macros). |
| `src/ton.rs`          | `TonClient` — tonapi.io HTTP client; `normalize_domain()` validation. |
| `src/bot.rs`          | teloxide `Command` enum + handlers; `format_domain_list()`.          |
| `src/jobs.rs`         | Job bodies: `refresh_all`, `send_due_alerts`, `run_monthly_report`.  |
| `src/scheduler.rs`    | Registers the daily and monthly cron jobs with `tokio-cron-scheduler`.|
| `src/util.rs`         | Time helpers: `now_ts`, `days_until`, `fmt_expiry`.                  |
| `migrations/`         | SQL migrations, embedded at compile time via `sqlx::migrate!`.        |

## Key conventions & decisions

- **Timestamps are `i64` unix seconds** everywhere in the DB and models. Convert
  to `chrono::DateTime<Utc>` only for display (`util::fmt_expiry`). This avoids
  sqlx/chrono type-mapping friction — do not switch columns to TEXT dates.
- **sqlx runtime queries** (`sqlx::query(...).bind(...)`), not the compile-time
  `query!` macros, so building never needs a live database or `DATABASE_URL`.
  Migrations are still embedded and run at startup.
- **TLS is rustls, not native-tls.** `teloxide`, `reqwest`, and `sqlx` are all
  configured with `default-features = false` + rustls features. Do **not** enable
  a native-tls path — it drags in `openssl-sys`, which fails to build here.
- **Alert dedupe:** for each (chat, domain) we pick the *smallest* configured
  threshold `T` with `days_left <= T` (the current urgency bucket) and send once
  per `(chat, domain, T, expires_at)` via `alerts_sent`. Renewal changes
  `expires_at`, which re-arms alerts. Don't fall back to a larger bucket if the
  smallest is already sent — that would send stale, less-urgent alerts.
- **TON API rate limit:** `TonClient` throttles *all* requests through a shared
  `Arc<Mutex<Option<Instant>>>` (held across the sleep, so calls serialize) to
  `TONAPI_RPS` requests/sec, default 1. The daily refresh loops sequentially, so
  N watched domains take ~N seconds — fine at this scale. If you parallelize
  refresh or add endpoints, route them through `TonClient` so the limit still holds.
- **Refresh failures are non-fatal:** on a TON API error we keep the previous
  `expires_at` and just log; only a successful fetch (including a definitive
  "no expiry" → `NULL`) overwrites it. A "domain not found" (`Ok(None)`) clears
  the info so stale expiries can't trigger alerts.
- **Owner tracking:** `domains.owner_address` / `owner_name` are refreshed daily
  via `update_domain_info`. Display prefers `owner_name`, falling back to a
  shortened address (`bot::owner_label`). `set_owner` is an owner-only update
  used as a fallback in `/addall` when a per-domain expiry fetch fails, so it
  won't clobber an expiry another watcher already populated.
- **`/addall` flow:** resolve seed domain → `item.owner.address` → list that
  wallet's NFTs filtered by the TON DNS collection (`ton::TON_DNS_COLLECTION`) →
  each item's `dns` field is a domain name → subscribe the user to all of them,
  fetching each domain's expiry inline via `fetch_info` (throttled to ~1/sec by
  `TonClient`, so it takes ~N seconds and sends an up-front ack).
- **Domain identity:** `normalize_domain` lowercases, strips `@`/whitespace, and
  requires a `.ton` suffix (with a non-empty label). Only `.ton` is supported —
  `.t.me` names don't expire. Domains are stored once and shared across users via
  the `subscriptions` join table.
- **Cron format is 6-field** (`sec min hour dom month dow`) per the `croner`/
  `tokio-cron-scheduler` crate. Update `.env.example` and README if defaults change.

## Common commands

```sh
cargo build                  # compile
cargo test                   # unit tests (normalization + alert logic)
cargo run                    # run the bot (loads .env)
cargo run -- check foo.ton   # one-off TON API lookup, no DB/bot
cargo clippy                 # lint
cargo fmt                    # format
```

## When adding features

- New bot command → add a variant to `Command` in `src/bot.rs`, a match arm in
  `answer`, and (usually) a `Db` method in `src/db.rs`.
- New scheduled job → add a job body in `src/jobs.rs` and register it in
  `src/scheduler.rs`; expose its cron via `Config`.
- New table/column → add a `migrations/NNNN_*.sql` file (never edit an applied
  migration); add matching queries in `src/db.rs`.
- Any new config → add to `Config::from_env`, `.env.example`, and the README table.

## Not yet implemented / possible next steps

- Unit tests cover `normalize_domain` (`ton.rs`) and the pure alert helpers
  `alert_bucket` / `format_alert` (`jobs.rs`). The DB layer and async job
  orchestration are not yet tested.
- Bot uses long polling; there is no webhook mode.
- No per-user timezone — all dates are UTC.
