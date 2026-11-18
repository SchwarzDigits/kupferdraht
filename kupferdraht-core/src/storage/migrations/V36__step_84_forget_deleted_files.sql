-- Step 84: a delete for everyone takes the file's row (its key, token and
-- locator) with the message. A message deleted before that still has its row;
-- it goes here, and the client's next start removes the cached copy that no
-- row names any more.
DELETE FROM attachments
 WHERE EXISTS (SELECT 1 FROM timeline_events
                WHERE timeline_events.message_id = attachments.message_id
                  AND timeline_events.room_id = attachments.room_id
                  AND timeline_events.redacted_at_ms IS NOT NULL);
