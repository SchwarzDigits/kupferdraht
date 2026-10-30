# Integrate Wire's External Senders extension

Condensed from `docs/adrs/0004-integrate-external-senders-extension.md`.

- **Register the Wire backend as External Sender in every MLS group you create** — fetch its key from `GET /mls/public-keys` at session start and cache it.
- **Validate every externally-signed Proposal against the cached backend key, then auto-commit valid administrative Proposals.**
- **Do not gate backend admin Proposals on user confirmation** — it diverges from other Wire clients and lets group state drift.
- Bounded trust: the backend can propose membership changes but cannot read content or impersonate members, and its presence is cryptographically visible.
