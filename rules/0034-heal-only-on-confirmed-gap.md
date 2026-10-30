# Heal a room only on a confirmed gap; leave a group only on the backend's word

Condensed from `docs/adrs/0034-heal-only-on-confirmed-gap.md`.

Accepted 2026-09-29 (Step 106 implements it). Builds on
`rules/0031-serialise-drain-and-defer-around-own-commits.md` and
`rules/0032-commit-mls-state-after-persisting-notification.md`.

- **Heal (discard + rejoin) for a later-epoch message only when the
  backend's current epoch for the conversation is ahead of the local
  group** — read it fresh from the conversation record, not the rooms
  cache; otherwise skip the message as undecryptable and report it.
- **A storage error while processing an MLS message never heals** — it
  aborts the notification (rolled back, cursor stays, retried). A missing
  local group still heals.
- **Discard the local group on a Remove of our own leaf only when the
  proposal's sender is the external sender** (the backend); a member's
  Remove proposal changes nothing until a commit applies it.
- **Don't** let an unauthenticated field (the epoch of an undecryptable
  message, a member's proposal) trigger a destructive action.
