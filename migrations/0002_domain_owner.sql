-- Cache the owning wallet for each domain, refreshed alongside expiry.
-- owner_address is the raw TON account; owner_name is its resolved .ton name
-- (if any), used for friendlier display.
ALTER TABLE domains ADD COLUMN owner_address TEXT;
ALTER TABLE domains ADD COLUMN owner_name TEXT;
