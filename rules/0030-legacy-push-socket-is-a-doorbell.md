# The legacy /await push socket is a doorbell for the REST drain

Condensed from `docs/adrs/0030-legacy-push-socket-is-a-doorbell.md`.

Accepted 2026-09-28 (Step 97b implements it). Supersedes items 2–3 of
`rules/0019-auto-negotiate-push-transport.md`.

- **Connect, then drain** on every (re)connect: open the socket first,
  then one REST `since=<cursor>` drain. **Don't** drain before the
  socket is open.
- **A durable frame only rings the doorbell**: it triggers a REST drain
  from the cursor (frames during a running drain coalesce into one
  follow-up drain); its own payload is not dispatched.
- **Only the REST drain advances the cursor.** **Don't** advance it from
  a socket frame.
- **Transient frames** (`transient: true`) dispatch directly, without a
  cursor advance.
- **Periodic drain: superseded by ADR-0039** (2026-10-02) — current rule:
  `rules/0039-drive-sync-by-triggers-timers-only-as-safety-nets.md` (a
  socket silent for a minute is renewed, then drained; without a socket the
  loop drains every pause). Keep reading the socket while a drain runs;
  never cancel a running drain.
