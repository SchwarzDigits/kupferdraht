# Step 135 — X.509 leaves: drop instead of envelope attribution

**Status: in progress 2026-11-02** (branch `step/135-x509-leaves-dropped`; commissioned 2026-11-02 from the review of the external
security scan of 2026-11-02, finding H1 — `sync/sender.rs:50`; the
decision below is Volker's, the same day). ADR-0027 decision 2 keeps the
envelope attribution for a sender whose leaf carries a non-Basic
credential and names "a follow-up step" that no plan file and no PLAN.md
row ever held; THREAT_MODEL G1 repeats the exception. This step is that
follow-up, cut to what can be built without an e2ei certificate.

## Problem

1. `attribute_sender` (`sync/sender.rs:50-58`) answers
   `LeafIdentityError::Unsupported` — every credential type but `Basic`
   (`encryption/leaf_identity.rs:55`) — with the backend-written envelope:
   `qualified_from` / `from_client`, or `unknown@unknown` when the envelope
   names nobody. The message is dispatched, persisted and surfaced under an
   identity nothing authenticated, logged at `debug!`.
2. Under BasicCredentials the backend is the key directory (THREAT_MODEL,
   Trust Model): it can add a device with any credential to any group. A
   device it adds with an X.509 credential is attributed per message to
   whatever the envelope says, so the backend chooses the apparent sender
   per message instead of per device. Step 95d makes the added device
   visible (`Room::devices`, `subscribe_device_changes`); the messages
   themselves carry no mark, and the public API has no `sender_verified`.
3. ADR-0027 chose option E (keep the envelope) over option D (drop) for one
   reason: no regression in a team with e2ei-enrolled devices. Kupferdraht
   never enrolls (NG-7), the own instances run without e2ei, and no
   embedder runs in an e2ei team — the regression is hypothetical while
   the forgery path is open on every deployment.

## Decision (2026-11-02, Volker)

Option (a): a message signed by a non-Basic leaf is dropped —
`attribute_sender` returns `None` — and logged at `warn!` with the message
id and the credential type. No `sender_verified` flag and no gating on the
team's `mlsE2EId` feature config (option (b), weighed and declined: a new
public field with FFI, wasm and bindings mirrors for a case no deployment
has). The flag returns with the real X.509 step, once a certificate can be
parsed.

## Scope

1. `sync/sender.rs`: the `Unsupported` arm drops with `warn!`;
   `UNKNOWN_SENDER` and the X.509 comments go.
2. ADR-0027 revision 1.1: decision 2 becomes "X.509 senders are dropped
   until X.509 is verified" — option D is the decision, with this step's
   reason; revision history; `rules/0027-*.md` in the same commit (the
   "keeps the envelope attribution" bullet).
3. `docs/THREAT_MODEL.md` G1: the exception sentence goes; every surfaced
   message is attributed to the MLS-authenticated leaf, without exception.
   Step 133 (THREAT_MODEL 0.7, merged 2026-11-02) kept the sentence; this
   step removes it with a revision line of its own.
4. `docs/library.md:210-211` and `docs/wire/mls-deviations.md:197`, which
   describe the X.509 fallback: one line each that non-Basic leaves are
   dropped. `RoomDevice` (`docs/library.md:1474`) is unchanged — the device
   list still shows an X.509 leaf with `user_id` / `device_id` `None`.
5. `CHANGELOG.md` `[Unreleased]`, consumer-facing: a message class that
   was delivered under an unauthenticated identity is now dropped and
   logged. No API change, so no `BREAKING:`.

## Out of scope

- X.509 identity parsing and certificate-chain verification: blocked until
  a real e2ei certificate can be observed (ADR-0027, option F).
- A `sender_verified` flag on `TimelineItem` (option (b)); comes with the
  parsing step.
- The leaf capabilities keep advertising `X509` (`mls_client.rs`, "Wire
  e2ei groups require this"): accepting the credential type is an MLS
  capability for group admission, separate from attributing a message.
- Step 132's items and the scan review's other findings.

## TDD sketch

In `sync/sender.rs`'s test module, next to the existing cases: an
`Err(LeafIdentityError::Unsupported)` with a full envelope returns `None`
(today: `Some` with the envelope's identity); a tracing capture (the
Step-101 fixture in `tests/step_101_no_secrets_in_logs.rs` has one) sees
one `warn` event naming the message id. `encryption/leaf_identity.rs`'s
`reports_x509_as_unsupported_not_malformed` stays as it is. A doc pin that
ADR-0027's decision 2, `rules/0027-*.md` and THREAT_MODEL G1 no longer
contain the envelope exception, in the style of the Step-132 doc pins. No
CLI command: nothing to exercise, a dropped message leaves no trace but
the log line ("no CLI (drop path)"). Gate; no smoke ("no smoke (no live
X.509 leaf)").
