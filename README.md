# TON Domains Monitoring

A Telegram bot that watches [TON](https://ton.org) `.ton` domains and warns you
before they expire. Multiple users can watch the same domain, and each user is
only notified about the domains they personally added. Expiry dates are refreshed
daily, proactive alerts fire as expiry approaches, and every user gets a full
summary on the 1st of each month.

> Only `.ton` domains are supported — `.t.me` names don't expire, so there's
> nothing to monitor.

## Features

- **Telegram bot** with simple commands: add, remove, and list watched domains.
- **Shared watches** — many users can subscribe to the same domain; it's only
  fetched from the TON API once per refresh.
- **Bulk add by wallet** — `/addall foo.ton` resolves the domain's owner and
  subscribes you to every `.ton` domain that wallet holds, fetching each expiry
  inline (throttled, so it takes a few seconds for large wallets).
- **Owner tracking** — each domain's owning wallet is stored and shown alongside
  its expiry, refreshed daily.
- **Daily refresh** of expiry dates via a [tonapi.io](https://tonapi.io)-compatible API.
- **Proactive alerts** at configurable thresholds (default 30 / 7 / 1 days before
  expiry), sent once per threshold and re-armed automatically if a domain is renewed.
- **Monthly report** — full domain list with expiry dates on the 1st of each month.
- **SQLite** storage, single self-contained binary, all scheduling in-process.

## Bot commands

| Command          | Description                                   |
| ---------------- | --------------------------------------------- |
| `/start`         | Welcome message and quick help                |
| `/help`          | List all commands                             |
| `/add foo.ton`   | Start watching a domain                        |
| `/addall foo.ton`| Watch every `.ton` domain owned by that domain's wallet |
| `/remove foo.ton`| Stop watching a domain                         |
| `/list`          | Show your watched domains, expiry dates, and owners |

## Quick start

1. Create a bot with [@BotFather](https://t.me/BotFather) and copy the token.
2. Configure the environment:
   ```sh
   cp .env.example .env
   # edit .env and set TELEGRAM_BOT_TOKEN
   ```
3. Run the bot:
   ```sh
   cargo run --release
   ```
   Or do a one-off expiry lookup without starting the bot:
   ```sh
   cargo run --release -- check yauhen.ton
   ```

The SQLite database and its parent directory are created automatically on first
run, and migrations are applied at startup.

## CLI

Besides running the bot (the default with no arguments), there's a one-off
lookup subcommand that queries the TON API directly — handy for testing:

```sh
$ ton-domains-monitoring check yauhen.ton
Domain:  yauhen.ton
Expires: 2027-06-26 (in 356 days)
NFT:     0:9de0d19044f932c17da5785d900e8c6bb9eb7d4586d88b44174dcd447376349e
Owner:   yauhen.ton
```

`check` needs only `TONAPI_BASE_URL` / `TONAPI_KEY` (from `.env` or the
environment); it doesn't touch the database or Telegram.

## Configuration

All settings are read from environment variables (a `.env` file is loaded if present).

| Variable             | Default                | Description                                        |
| -------------------- | ---------------------- | -------------------------------------------------- |
| `TELEGRAM_BOT_TOKEN` | — (required)           | Bot token from @BotFather                           |
| `DATABASE_URL`       | `sqlite://data/ton.db` | SQLx SQLite connection string                       |
| `TONAPI_BASE_URL`    | `https://tonapi.io`    | Base URL of the TON API                             |
| `TONAPI_KEY`         | —                      | Optional bearer token for higher rate limits        |
| `TONAPI_RPS`         | `1`                    | Max TON API requests/sec (global throttle)          |
| `DAILY_CRON`         | `0 0 3 * * *`          | Daily refresh + alerts (6-field cron)               |
| `MONTHLY_CRON`       | `0 0 9 1 * *`          | Monthly report (6-field cron)                       |
| `ALERT_THRESHOLDS`   | `30,7,1`               | Days-before-expiry that trigger an alert            |
| `RUST_LOG`           | `info,sqlx=warn,...`   | Tracing filter                                      |

Cron expressions are 6-field: `sec min hour day-of-month month day-of-week`.

## How it works

- The bot polls Telegram for updates and handles commands
  ([`src/bot.rs`](src/bot.rs)).
- An in-process scheduler ([`src/scheduler.rs`](src/scheduler.rs)) runs two cron
  jobs against the shared DB and bot handles.
- The **daily job** ([`src/jobs.rs`](src/jobs.rs)) refreshes every watched
  domain's expiry from the TON API, then sends any due alerts. Each (user, domain)
  is alerted once per threshold bucket per expiry value; renewing a domain changes
  its expiry and re-arms alerts.
- The **monthly job** sends every user their full domain list.

### Data model

- `domains` — one row per unique domain name with its cached `expires_at`.
- `subscriptions` — which `chat_id` watches which domain (many-to-many).
- `alerts_sent` — dedupe log keyed by `(chat, domain, threshold, expires_at)`.

See [`migrations/0001_init.sql`](migrations/0001_init.sql).

## Development

```sh
cargo build        # compile
cargo run          # run (reads .env)
cargo clippy       # lint
cargo fmt          # format
```

## License

MIT
