# ADR-0027: Attribute inbound messages to the MLS-authenticated leaf credential

**Status:** Accepted
**Date:** 2026-09-26
**Deciders:** Markus Troßbach

## Context

Every Wire MLS application message arrives inside a JSON envelope
(`conversation.mls-message-add`) whose `qualified_from` / `from_client`
fields name the sender. The backend writes the envelope; no MLS signature
covers it, and for a `PrivateMessage` the backend cannot even see which
leaf sent it (the sender data is encrypted). openmls, while decrypting,
authenticates the actual sender: `ProcessedMessage::credential()` is the
credential of the leaf that signed the message. Official Wire clients
attribute an MLS application message to the identity in that credential.
Kupferdraht discards it (`processed.into_content()`) and attributes every
inbound message — text, reaction, receipt, task, case snapshot, marker —
to the envelope (falling back to `"unknown@unknown"`). A compromised or
malicious backend can therefore re-attribute any genuine ciphertext to any
other member; the ADR-0018 case gate, specified on the "MLS-authenticated
sender", checks the envelope in reality; THREAT_MODEL G1 promises sender
authentication.

Wire's `BasicCredential.identity` carries `{uuid}:{client_id}@{domain}`
(mls-deviations W-4); the `client_id` segment is the same string the API
uses (`from_client`). e2ei (X.509 credentials) is a non-goal (NG-7);
Wire's certificate identity format is not publicly specified, and the own
instance runs without e2ei, so no real certificate is available to verify
a parser against. Constraints from prior decisions: MLS is openmls used
directly (ADR-0001), so the authenticated credential is available at the
decrypt site. Found in the production-readiness review of 2026-09-25
(blocker 1).

## Options Considered

### Sender source

#### Option A: Keep the envelope sender (status quo)

**Pros:**
- No change; tolerant of any credential type.

**Cons:**
- Sender authentication rests on backend honesty — breaks G1 and the
  ADR-0018 gate; diverges from official Wire clients.

#### Option B: MLS credential authoritative, envelope as cross-check

Parse the sender from the authenticated leaf credential; on an envelope
mismatch dispatch under the MLS sender and log a `warn!`.

**Pros:**
- The backend can no longer re-attribute messages.
- Wire parity.
- No message loss from backend quirks; mismatches stay visible in logs.

**Cons:**
- A mismatch reaches the log, not the consumer.

#### Option C: MLS credential must match the envelope, else drop

**Pros:**
- Strict; tampering is never silently corrected.

**Cons:**
- The message itself is authentic, so dropping it loses data for no
  security gain.

### Senders with non-Basic (X.509) credentials

#### Option D: Drop with `warn!` until X.509 is supported

**Pros:**
- No unauthenticated attribution anywhere.

**Cons:**
- A regression for mixed e2ei teams — messages from enrolled devices,
  shown today, would be dropped.

#### Option E: Keep the envelope sender for X.509 senders (status quo for them)

**Pros:**
- No regression.
- The credential type is fixed by the signing leaf, so the backend cannot
  route a Basic-signed message into this path.
- Dormant on a non-e2ei deployment.

**Cons:**
- The forgery path stays open for messages signed by X.509 leaves.
- Consumers cannot tell which senders were authenticated.

#### Option F: Parse the certificate SAN now, from the one public example

**Pros:**
- Full support immediately.

**Cons:**
- The format (`wireapp://{base64url-uuid}!{client}@{domain}`) is backed
  only by one example string; a wrong parse drops or — worse —
  mis-attributes messages.
- No real certificate, no live smoke.

#### Option G: Enable e2ei on the own instance first, capture a real certificate

**Pros:**
- Verified format.

**Cons:**
- ACME server + IdP setup; blocks the sender fix on unrelated
  infrastructure.

## Decision

Options B and E.

1. **The authenticated leaf credential is the sender.** For every inbound
   application message signed by a leaf with a Wire `BasicCredential`,
   Kupferdraht parses the sender (qualified user id + device id) from
   `ProcessedMessage::credential()` — exactly
   `{uuid}:{client_id}@{domain}`, three non-empty parts — and dispatches
   under it. The envelope's `qualified_from` / `from_client` is a
   cross-check only: on mismatch the message is dispatched under the MLS
   sender and a `warn!` records both values. A Basic credential that does
   not parse as W-4 is malformed and dropped with a `warn!`.
2. **X.509 senders keep the envelope attribution until X.509 is
   verified.** A message signed by a leaf with a non-Basic credential is
   attributed to the envelope as before this decision and logged at
   `debug!`. X.509 identity parsing is a follow-up step, started once a
   real e2ei certificate can be observed; a public `sender_verified` flag
   is weighed there.

## Consequences

### Positive

- A backend can no longer re-attribute a Basic-credential member's
  messages; G1 holds for them, and the ADR-0018 case gate becomes what it
  specifies (task creators, reactions, receipts, markers likewise).
- Wire parity: Kupferdraht attributes messages the way official clients do.
- One parse path for Wire leaf identities (member lists, kick, sender).

### Negative

- Messages signed by e2ei (X.509) leaves stay attributed by the
  unauthenticated envelope until the follow-up step; the public API does
  not mark which senders are authenticated.
- A member can still re-label its *own* leaf via an Update / UpdatePath
  commit (the RFC 9420 §5.3.1 successor check is not enforced); rejecting
  such a commit is undone today by the `WrongEpoch` → rejoin heal.
  Deferred to the MLS state-hardening follow-up together with narrower
  heal triggers.
- The backend remains the key directory under BasicCredentials: it can
  add a leaf carrying any identity (a ghost device) that then reads and
  sends MLS-authentically "as" that identity. This decision authenticates
  *which leaf* sent a message, not *who owns* the leaf. Official Wire has
  the same trust model without e2ei and offers e2ei certificates and
  per-device MLS thumbprints against it; Kupferdraht has neither yet (open.md
  §64 "Device verification UX").
- An envelope mismatch — a signal of a misbehaving backend — reaches only
  the log, not the consumer.

### Neutral

- No public signature change; the meaning of `sender` changes
  (authenticated) — a `Changed` changelog entry, not `BREAKING`.
- `conversation.typing` is not an MLS message and stays unauthenticated by
  design.

## Implementation Notes

- `encryption/leaf_identity.rs` owns build + parse of the W-4 identity and
  replaces the parsers in `group_state.rs` and `group_kick.rs`; a
  non-Basic credential parses to a typed "unsupported", a malformed Basic
  one to a typed "malformed".
- `DecodedMessage::Application` carries the parsed sender, captured before
  `into_content()`.

## References

- Builds on ADR-0001 (openmls used directly — the authenticated credential
  is available at the decrypt site).
- Related: ADR-0018 (decision 3 presupposes the MLS-authenticated sender —
  this ADR makes the premise true); ADR-0004 (external senders cannot send
  application messages; backend authority stays bounded to proposals);
  ADR-0014 / ADR-0020 (task creator and marker attribution now
  authenticated); ADR-0028 (builds on this decision).
- `docs/wire/mls-deviations.md` W-4; `docs/THREAT_MODEL.md` G1, N4, A5.

## Revision History

- 2026-09-26: Created
