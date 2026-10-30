# Commit an inbound notification's MLS state only after its data is persisted

Condensed from `docs/adrs/0032-commit-mls-state-after-persisting-notification.md`.

Accepted 2026-09-28 (Step 97e implements it). Builds on
`rules/0031-serialise-drain-and-defer-around-own-commits.md`.

- **The order:** decrypt and persist to `kupferdraht.db` → commit the MLS
  transaction → advance the cursor. **Don't** commit MLS changes before
  their data is persisted, and **don't** advance the cursor before the
  MLS commit. The window is per page since ADR-0037 (item 1 superseded);
  current rule: `rules/0037-commit-a-drained-page.md`.
- **Idempotent per message id:** every inbound handler tolerates a
  replay; a consumer callback fires only for a fresh effect.
- **Skip silently** on replay what MLS already processed: an older epoch
  or a consumed generation.
- **Roll back** the notification's MLS changes on a `kupferdraht.db` failure;
  the cursor stays before it.
- **One MLS writer per window:** every MLS write outside inbound dispatch
  takes the async MLS write gate. **Don't** let a send or an own commit
  write into an open notification window.
