-- Step 144: avatars. The asset keys a profile names, `preview` and `complete` on
-- the wire: a member's from `list-users`, the own from `/self` and from setting
-- it. `team_avatar_key` is the `icon` of the team request, as of the last login.
-- A row of an earlier login reads NULL for all of them until the next refresh.
ALTER TABLE room_members ADD COLUMN avatar_thumbnail_key TEXT;
ALTER TABLE room_members ADD COLUMN avatar_full_key TEXT;
ALTER TABLE self_user ADD COLUMN avatar_thumbnail_key TEXT;
ALTER TABLE self_user ADD COLUMN avatar_full_key TEXT;
ALTER TABLE self_user ADD COLUMN team_avatar_key TEXT;
