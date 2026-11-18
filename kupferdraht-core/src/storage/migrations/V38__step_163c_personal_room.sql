-- Step 163c: the account's personal room is stored under the library's own
-- type name (ADR-0009). A database written before carries the backend's name
-- for it; it is rewritten here, and every other room type stays as it is.
UPDATE rooms SET room_type = 'personal' WHERE room_type = 'self_room';
