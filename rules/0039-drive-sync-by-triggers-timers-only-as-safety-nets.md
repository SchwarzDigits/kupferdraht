# Drive sync by triggers, with timers only as safety nets

Condensed from `docs/adrs/0039-drive-sync-by-triggers-timers-only-as-safety-nets.md`.

Accepted 2026-10-02 (Step 118 implements it). Supersedes the "no periodic
drain" of `rules/0030-…` and the reconnect backoff of `rules/0017-…` and
`rules/0019-…`.

- **Fetch the room list only when it may be out of date**: at start; after
  an event the drain does not process itself, or a Welcome; when the drain
  does not start where this client's last one ended, or the backend lost
  the cursor — in the same pass, after the drain; and when it is older than
  5 minutes. **Don't** fetch it every pass or on every reconnect.
- **No timer beside a live socket**: drain when it rings or closes; a close
  reconnects at once, then drains (ADR-0030's order).
- **Renew a socket without a frame for 1 minute** (reconnect, then drain);
  natively on the keepalive tick and only while no drain runs — **never**
  cancel a running drain.
- **Without a socket, drain anyway**; connect at most once per pause.
  **Don't** let receive depend on the socket opening.
- **One pause without a socket on every target: 10 s** (`poll_interval`
  unset) — the polling loop's and a push loop's while its socket is down.
- Keep checking the rooms' device baselines every `sync_once` pass (a local
  read; Step 95d's safety net).
