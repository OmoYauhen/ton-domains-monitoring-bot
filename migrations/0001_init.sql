-- Domains tracked by the service. One row per unique domain name.
CREATE TABLE IF NOT EXISTS domains (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL UNIQUE,
    expires_at      INTEGER,            -- unix seconds, NULL until first successful refresh
    last_checked_at INTEGER,            -- unix seconds of last refresh attempt
    created_at      INTEGER NOT NULL
);

-- Which chat (user) watches which domain. Many users may watch the same domain.
CREATE TABLE IF NOT EXISTS subscriptions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id    INTEGER NOT NULL,
    domain_id  INTEGER NOT NULL REFERENCES domains(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    UNIQUE (chat_id, domain_id)
);

CREATE INDEX IF NOT EXISTS idx_subscriptions_chat   ON subscriptions (chat_id);
CREATE INDEX IF NOT EXISTS idx_subscriptions_domain ON subscriptions (domain_id);

-- Record of proactive expiry alerts already delivered, so we don't spam.
-- Keyed by the expiry it was sent for: if a domain is renewed, expires_at
-- changes and a fresh alert can fire for the new expiry.
CREATE TABLE IF NOT EXISTS alerts_sent (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id        INTEGER NOT NULL,
    domain_id      INTEGER NOT NULL REFERENCES domains(id) ON DELETE CASCADE,
    threshold_days INTEGER NOT NULL,
    expires_at     INTEGER NOT NULL,
    sent_at        INTEGER NOT NULL,
    UNIQUE (chat_id, domain_id, threshold_days, expires_at)
);
