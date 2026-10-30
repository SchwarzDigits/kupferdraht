# ADR-0004: Integrate Wire's External Senders Extension

**Status:** Accepted
**Date:** 2026-04-21
**Deciders:** Volker Zöpfel

---

## Context

MLS (RFC 9420) defines how groups are managed cryptographically: membership changes (Add, Remove, Update) are proposed and committed by group members themselves. The server's role in the standard MLS model is limited to message distribution. It routes ciphertexts, but it has no protocol-level authority over group state.

Wire extends this model. Wire's backend is registered as an **External Sender** in every MLS group (see RFC 9420 §12.1.8). The backend can sign Proposals — typically membership-related — that group members then commit automatically. This lets the Wire backend enforce organization-level policies: when a user is removed from a team, their membership in all team-related groups is revoked via backend-signed Remove Proposals.

For Kupferdraht, the question is whether to integrate this Wire-specific extension or treat it as Wire's concern and ignore it.

The immediate practical consequence of *not* integrating it: Kupferdraht-managed groups would be incompatible with Wire's other clients. A group created by a Kupferdraht client would lack the External Senders extension, and Wire's own clients (wire-android, wire-desktop) would reject it or behave inconsistently. Conversely, a Kupferdraht client joining a group created by a Wire client would see Proposals signed by the backend and need to validate or commit them.

The longer-term consequence: CivicSeal and comparable deployments need administrative mechanisms to remove users from conversations when they leave the agency, when privileges change, or when a role shifts. Without a backend-mediated mechanism, every administrative change requires an active member of the group to execute it, which is operationally impractical for hundreds of concurrent cases.

---

## Options Considered

### Option A: Do not integrate External Senders

Kupferdraht creates and joins MLS groups as per vanilla RFC 9420, without the Wire-specific extension. No backend-signed Proposals are accepted.

- **Pro:** Cleaner protocol alignment — Kupferdraht would work against any vanilla MLS backend, not just Wire.
- **Con:** Kupferdraht groups are incompatible with Wire groups. A user with both Kupferdraht and Wire clients (or Kupferdraht and wire-desktop, for example) cannot participate in the same groups consistently.
- **Con:** No backend-mediated group administration. Every membership change requires an active member of the group. For organizational deployments this is operationally untenable.
- **Con:** We lose the Wire backend as a trusted administrative channel, which is a useful architectural role even if it carries a specific trust delta.

### Option B: Integrate External Senders, validate Proposals against the registered backend key, auto-commit valid administrative Proposals

Follow Wire's configuration: fetch the backend's External Sender public key during setup, register it in the External Senders extension of every group we create, validate all incoming externally-signed Proposals against that key, and auto-commit valid administrative Proposals with the same logic that other Wire clients use.

- **Pro:** Wire backend compatibility. Kupferdraht clients and Wire clients can interoperate in the same groups without behavioral divergence.
- **Pro:** Backend-mediated administration becomes available, which is an operational necessity for organizational use.
- **Pro:** The External Senders extension is a standard RFC 9420 feature. openmls supports it out of the box. Integration effort on Kupferdraht's side is limited to fetching, registering, validating, and auto-committing.
- **Con:** Broadens the trust relationship with the server. Kupferdraht now trusts the backend with group administration authority, in addition to message routing. This is a specific, bounded trust — the backend can propose membership changes but cannot read content or impersonate members — but it is a trust relationship nonetheless.
- **Con:** A compromised backend could silently add or remove group members. The extension list is publicly visible, so such action is detectable, but detection is ex-post.

### Option C: Integrate External Senders but do not auto-commit — require user confirmation for every administrative Proposal

Accept externally-signed Proposals as valid, but surface them to the user (or the embedding product) for explicit approval before committing.

- **Pro:** Stronger user control. No silent membership change is possible.
- **Con:** Behavioral divergence from other Wire clients. A Wire admin's Remove action would succeed instantly for Wire users but be blocked in Kupferdraht-only participants until they approve. The group state diverges during the delay.
- **Con:** Administrative efficiency breaks down. A compliance-driven removal from one hundred cases would require one hundred manual approvals across the organization.
- **Con:** Confirmation UX is the embedding product's responsibility, not Kupferdraht's. Kupferdraht cannot enforce the approval flow — it can only delay the commit.

---

## Decision

**We integrate the External Senders Extension following Wire's convention and auto-commit valid administrative Proposals (Option B).**

Backend compatibility with Wire is a non-negotiable requirement for Kupferdraht's primary use case. Option A is not viable. Option C introduces behavioral divergence and operational friction that outweighs the marginal security benefit.

The trust delta introduced by the extension is real but bounded and documented. The Wire backend can propose membership changes, but it cannot read content, it cannot impersonate a sender, and its presence as External Sender is cryptographically visible to every group member. This is a known and accepted property of the Wire architecture, not a hidden risk.

---

## Consequences

### Positive

- Full wire-line interoperability with Wire's other clients (wire-android, wire-desktop).
- Backend-mediated group administration becomes available. Organizations can remove users from all their groups through a single backend action rather than per-group manual operations.
- Leverages a standard RFC 9420 feature rather than requiring Wire-specific protocol extensions.
- Integration cost is small — openmls supports External Senders natively.

### Negative

- Kupferdraht trusts the Wire backend with group administration authority, in addition to the baseline trust in it as a message router. A compromised backend can silently add or remove members, visible only ex-post.
- Adds a specific security consideration to the Kupferdraht threat model (L4 in `THREAT_MODEL.md`).

### Neutral

- The External Senders extension applies only to group administration. It has no effect on message content security, which remains end-to-end encrypted with keys only members hold.
- Kupferdraht groups will display the backend's External Sender identity to any user inspecting group configuration — this is by RFC 9420 design and not a Kupferdraht disclosure decision.

---

## Implementation Notes

Required integration points:

1. At session start, fetch the backend's External Sender public key from `GET /mls/public-keys` and cache it locally.
2. When creating a new MLS group, include the External Senders extension in the group configuration, naming the backend as the only external sender.
3. When processing incoming Proposals that are externally signed, validate the signature against the cached backend key before accepting them.
4. Auto-commit validated administrative Proposals using the same logic as other Wire clients.

Implementation touches Step 3 (group creation) and Step 6 (event stream receive path).

Full details in `docs/wire/mls-deviations.md` W-5.

---

## References

- `docs/wire/mls-deviations.md` W-5 — detailed explanation of the mechanism and Kupferdraht's integration
- `THREAT_MODEL.md` — limitation L4 (backend administrative authority)
- [RFC 9420 §12.1.8](https://www.rfc-editor.org/rfc/rfc9420#section-12.1.8)
- ADR-0001 — openmls foundation that provides the External Senders support

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR |
