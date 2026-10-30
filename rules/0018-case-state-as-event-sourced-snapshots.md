# Case state as event-sourced snapshots

Condensed from `docs/adrs/0018-case-state-as-event-sourced-snapshots.md`.

- **Do** carry room case state as full-snapshot `CasePayload` messages in
  `GenericMessage` field 262001 (outside the `content` oneof, no text
  fallback); fold last-writer-wins by `revision`, then server delivery
  order.
- **Do** send the current snapshot immediately after performing an invite
  (MUST); re-broadcast on observed member-joins is allowed (MAY). The
  same hook is shared with ADR-0020 (marker + task batch); this rule
  still owns only the case snapshot.
- **Do** fold core-field changes only from senders who are `wire_admin`
  of the conversation at receive time; `extensions` entries fold from any
  member and carry no semantics.
- **Don't** add a first-snapshot exception for non-admin inviters
  (observations §72): the fold discards that snapshot when no case
  exists yet — accepted residual of the admin gate, not a hook bug.
- **Don't** anchor case state to any single past message (no MLS
  history), send deltas instead of snapshots, or store case values in
  server-side conversation metadata (plaintext to the operator).
- **Don't** add per-aspect top-level `GenericMessage` fields — case
  schema grows by protobuf field addition inside `CasePayload`.
