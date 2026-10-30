# Authorize inbound edits and deletes by the original sender (Wire's rule)

Condensed from `docs/adrs/0028-authorize-edits-and-deletes-by-original-sender.md`.

Accepted 2026-09-26 (Step 95b implements it). Builds on
`rules/0027-attribute-inbound-messages-to-mls-leaf-credential.md`.

- **Both require the target in the same room**, resolved via message id
  or current edit id, checked against the ADR-0027 sender.
- **Edit:** only when the edit's sender user id equals the target row's
  sender user id (any device of that user).
- **Delete:** only when (a) the delete's sender user is the target's
  sender, (b) the delete's sender is the self user and the target is
  **ephemeral** (own-device sync of a receiver-side expiry — revised
  2026-09-28, observations §91), or (c) the target is the self user's
  own ephemeral message (from any member — receivers enforce expiry for
  the sender). **Don't** accept a third party's delete of someone
  else's ephemeral message, nor a self-sent delete of another member's
  ordinary message.
- **No admin override.** A rejected edit/delete is dropped with a
  `warn!` — no row change, no timeline diff, no task/case fold change.
- **Sending side:** `Timeline::edit` / `Timeline::redact` refuse another
  user's message (`InvalidInput`, nothing uploaded); own = any device of
  the self user; an id unknown locally goes out unchanged.
- Enforce in the dispatch layer (`sync/dispatch/edit.rs`, `delete.rs`),
  resolving the target in the same room
  (`Store::load_timeline_event_in_room` — rows are keyed per room);
  **don't** break the public `Store` trait (new methods get a default body).
