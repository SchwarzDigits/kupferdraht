-- Step 95d — `room_devices`: the device set last observed per room.
--
-- One row per MLS leaf of the room's group, as `Room::devices` reports
-- it: the W-4 identity (empty when the leaf has none — an e2ei X.509
-- leaf or a malformed Basic one), the credential type ('basic' /
-- 'x509') and the RFC 7638 thumbprint of the leaf's signature key
-- (empty when the key has no P-256 SEC1 encoding). It is the baseline a
-- device-set change is diffed against, persisted so a change that
-- happened while no client ran is still reported at the room's next
-- epoch change. A room without rows has no baseline yet: its first
-- observation records one silently.
--
-- Not a foreign key to `rooms`: `rooms` is replace-managed by every
-- metadata sync, the baseline must outlive that (the read-marker
-- precedent, V25). Cleared by the Step-40 new-device wipe.

CREATE TABLE room_devices (
    room_id     TEXT NOT NULL,
    user_id     TEXT NOT NULL DEFAULT '',
    device_id   TEXT NOT NULL DEFAULT '',
    credential  TEXT NOT NULL,
    thumbprint  TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (room_id, credential, user_id, device_id, thumbprint)
);
