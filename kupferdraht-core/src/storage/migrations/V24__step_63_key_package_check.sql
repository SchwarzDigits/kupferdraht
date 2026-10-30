-- Step 63: throttle stamp for the automatic key-package pool check.
-- `last_key_package_check_at_ms` — unix-ms of the last replenishment
-- attempt (successful or not); the sync-surface hook checks at most
-- once per 24 h. NULL = never checked (fresh device → first sync
-- checks immediately).
ALTER TABLE sync_state ADD COLUMN last_key_package_check_at_ms INTEGER;
