# Attribute inbound messages to the MLS-authenticated leaf credential

Condensed from `docs/adrs/0027-attribute-inbound-messages-to-mls-leaf-credential.md`.

Accepted 2026-09-26 (Step 95a implements it).

- **The sender of an inbound application message is the leaf credential
  openmls authenticated** (`ProcessedMessage::credential()`), parsed as
  Wire's W-4 identity `{uuid}:{client_id}@{domain}` — exactly three
  non-empty parts. Every handler (text, reaction, receipt, knock,
  location, asset, ephemeral, task, case, marker) dispatches under it.
- **The envelope (`qualified_from` / `from_client`) is a cross-check
  only:** on mismatch dispatch under the MLS sender and `warn!` both
  values. **Don't** drop the message, **don't** fall back to the envelope
  for a Basic-credential sender, **don't** invent `unknown@unknown`.
- **A malformed Basic credential** (not W-4) → drop + `warn!`.
- **An X.509 (non-Basic) sender keeps the envelope attribution** and is
  logged at `debug!` — until the X.509 follow-up step verifies Wire's
  certificate identity format against a real certificate. **Don't** parse
  the SAN from an unverified format.
- **One parse path:** build + parse of the W-4 identity live in
  `encryption/leaf_identity.rs`; don't add a second parser.
- This authenticates *which leaf* sent a message, not *who owns* it — the
  backend stays the key directory under BasicCredentials. Don't describe
  it as person or device verification.
