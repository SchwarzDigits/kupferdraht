-- Step 112 (ADR-0035) — the account's recovery state.
--
-- Singleton row (id = 1): the browser's recovery data exists since
-- `exported_at_ms` (Unix-epoch ms of the last `enable` / `changePin`), and
-- `pin_verifier` checks the current PIN locally (HMAC-SHA256 keyed from the
-- account's seed) — `changePin` and `verifyPin` spend no attempt at the PIN
-- service. In the server mode the row travels with the databases, so a
-- restored account knows its recovery data exists.

CREATE TABLE recovery_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    exported_at_ms  INTEGER NOT NULL,
    pin_verifier    BLOB NOT NULL
);
