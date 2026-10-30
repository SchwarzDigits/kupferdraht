# Wire's MLS deviations from RFC 9420

Wire's MLS *protocol* is RFC 9420-compliant — every cryptographic
primitive, the key schedule, the ratchet, and the message format
follow the standard. What deviates is the **delivery and policy
layer** wrapped around it: framing choices on specific endpoints,
extension requirements, identity encoding, and HTTP-level conventions
that have no analogue in the RFC.

This document is the consolidated reference for those deviations.
Every entry below explains:

* what the RFC / pure-MLS expectation is,
* what Wire actually does,
* where Kupferdraht handles it in code,
* and the source-of-truth observation (live test, swagger, or finding).

If you find a new deviation, add it here in the same template — and
add a pointer to it from `docs/observations.md` so the discovery context is
preserved.

> **Scope.** This file is about the MLS layer. Wire's HTTP / auth
> conventions (cookie auth on `/access`, Z-Client header injection,
> ConsumableNotifications, …) live in
> [`api-notes.md`](api-notes.md).

---

## Quick index

| #   | Deviation                                                        | Severity     | Kupferdraht handler |
|-----|------------------------------------------------------------------|--------------|----------------|
| W-1 | `GET /groupinfo` returns raw `VerifiableGroupInfo`, not framed   | parser       | `encryption/external_commit.rs::parse_verifiable_group_info` |
| W-2 | Commit bundle is a triple-concatenation, every segment `MlsMessage`-framed | uploader   | `encryption/external_commit.rs::serialize_bundle` |
| W-3 | Leaf-node lifetime acceptance window is wider than openmls's default | builder      | `ExternalCommitBuilder::skip_lifetime_validation` in `external_commit.rs` |
| W-4 | `BasicCredential.identity` carries `{uuid}:{client_id}@{domain}` | identity     | `encryption/mls_client.rs::wire_identity` + `group_state.rs::wire_user_id_from_leaf_identity` |
| W-5 | `external_senders` group-context extension is mandatory          | extension    | ADR-0004 + Step 1c.4 backend-removal-key fetch |
| W-6 | Ciphersuite pinned to `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`  | parameter    | `encryption/mls_client.rs::MLS_CIPHERSUITE` |
| W-7 | Leaves must declare `[Basic, X509]` credential capabilities      | parameter    | `encryption/mls_client.rs::leaf_capabilities` |
| W-8 | `Z-Client` is nginx-injected from the access-token claim         | transport    | LEARNINGS §1.4 — `client/login_pipeline.rs::register_and_rebind` |
| W-9 | `PUT /v14/clients/{id}` must register the MLS pubkey before key-package upload | flow | observations.md §19 — `login_pipeline.rs::upload_initial_key_packages` |
| W-10 | `GenericMessage` field 262000 carries Kupferdraht's `TaskPayload` extension | schema | ADR-0014 — `wire/proto.rs::build_task_generic_message` + `sync/service.rs::dispatch_mls_message` |

Severity legend:

* **parser** — affects how Kupferdraht decodes a Wire response.
* **uploader** — affects how Kupferdraht encodes a Wire request body.
* **builder** — affects how Kupferdraht drives openmls's group / commit
  construction.
* **identity** — affects what bytes go into MLS credential fields.
* **extension** — affects which MLS extensions are present / required.
* **parameter** — affects ciphersuite / capabilities choice.
* **transport** — affects HTTP wiring around the MLS payload.
* **flow** — affects the *order* of MLS-related calls.
* **schema** — a deliberate Kupferdraht extension of the shared
  `GenericMessage` schema (not a Wire behaviour).

---

## W-1 — `GET /groupinfo` returns raw `VerifiableGroupInfo`

**RFC / pure MLS:** Most MLS deployments wrap every protocol object
that travels over a transport in an `MlsMessage` envelope
(`ProtocolVersion || WireFormat || body`). The `GroupInfo` wire format
discriminant is `0x0004`.

**Wire:** `GET /v14/conversations/{cnv_domain}/{cnv}/groupinfo` returns
the raw TLS-serialized `VerifiableGroupInfo` body. There is **no**
`MlsMessage` envelope — leading byte is whatever `VerifiableGroupInfo`
serializes to (live-validated 2026-04-25, leading byte was `0x20`).

**Effect on Kupferdraht:** The naïve
`MlsMessageIn::tls_deserialize` fails with `UnknownValue(N)` because
the byte at the wire-format position is not a valid MLS wire format.

**Kupferdraht handling:** `parse_verifiable_group_info` tries
`MlsMessageIn::tls_deserialize` first (so a future Wire change to
envelope-framed delivery still works without code churn) and falls
back to `VerifiableGroupInfo::tls_deserialize` for the current shape.

**Source:** Live-validation 2026-04-25 against a live Wire v14 backend.

---

## W-2 — Commit bundle is a triple concatenation, every segment `MlsMessage`-framed

**RFC / pure MLS:** A commit is a single `MlsMessage(PublicMessage)`
or `MlsMessage(PrivateMessage)`. Welcome messages travel separately
(`/welcome` endpoints in Matrix, `Welcome` extension in pure MLS).

**Wire:** `POST /v14/mls/commit-bundles` accepts a Wire-specific
"CommitBundle" — a concatenation of three TLS-serialized segments,
in this order:

```
commit     : MlsMessage(PublicMessage | PrivateMessage)
group_info : MlsMessage(GroupInfo)           ← wrapped, not raw
welcome?   : MlsMessage(Welcome)             ← optional
```

The asymmetry with W-1 is the trap: `/groupinfo` returns *raw*
`VerifiableGroupInfo`, but `/commit-bundles` expects each segment
`MlsMessage`-framed including `GroupInfo`. Pure-MLS implementations
of `MlsGroup` provide commit and welcome as `MlsMessageOut` already;
the `GroupInfo` segment must be explicitly wrapped via
`MlsMessageOut::from(group_info)` before TLS-serialization. Raw
`GroupInfo` bytes cause Wire to respond
`400 "Unknown ContentType: N"` for whatever byte happens to be at the
wire-format position.

**Kupferdraht handling:** `serialize_bundle` in
`encryption/external_commit.rs` walks commit → group-info, wraps the
GroupInfo, and concatenates. External commits emit no Welcome, so
the bundle has exactly two segments today.

**Source:** LEARNINGS §1.5 (first-pass discovery), [`api-notes.md`](api-notes.md)
`POST /v14/mls/commit-bundles`.

---

## W-3 — Leaf-node lifetime acceptance window is wider than openmls's default

**RFC / pure MLS:** RFC 9420 §7.5 says applications MUST validate leaf
node lifetimes against their local clock, but does **not** pin the
acceptance window — that is an application policy.

**Wire:** Wire's iOS / Android / desktop clients emit leaf lifetimes
that don't fit openmls's default acceptance window. Live-observed
2026-04-25:

```
Lifetime { not_before: 1769693151, not_after: 1776954351 }
                                  ≈ 84-day window
```

openmls's default rejects this with
`LeafNodeValidation::Lifetime(NotCurrent)` when joining via external
commit because the existing peer leaves' lifetimes look "wrong" to
its policy.

**Effect on Kupferdraht:** A leaf's lifetime is the *publisher's* choice —
the joiner cannot retroactively renegotiate a peer's lifetime, and
refusing to join because some other client picked a lifetime openmls
doesn't like is the wrong policy.

**Kupferdraht handling:** `ExternalCommitBuilder::skip_lifetime_validation()`
in `encryption/external_commit.rs::run_external_commit`. This exempts
only the *pre-existing* leaves in the ratchet tree; the local leaf
Kupferdraht publishes through the commit still goes through full
validation against the local clock.

**Source:** Live-validation 2026-04-25 against a live Wire v14 backend
("Kupferdraht Test 2" group, room id `c0ffee02-…@example.com`).

---

## W-4 — `BasicCredential.identity` carries `{uuid}:{client_id}@{domain}`

**RFC / pure MLS:** RFC 9420 leaves `BasicCredential.identity` as
opaque application-defined bytes. The protocol does not parse them.

**Wire:** Wire packs three pieces of identity into the credential
payload:

```
{uuid}:{client_id}@{domain}
```

* `{uuid}`     — Wire user UUID (no domain).
* `{client_id}` — hexadecimal device id from `POST /v14/clients`.
* `{domain}`    — the deployment's domain (e.g. `example.com`).

The `:client_id` middle segment is Wire-specific — pure-MLS deployments
typically just put the user identifier there (or an X.509 SubjectName
for `Credential::X509`).

**Effect on Kupferdraht:**

* When *building* a leaf credential at login time we have to assemble
  the triple in this order.
* When *reading* a peer's leaf to surface members (Step 3d), we have
  to strip `:client_id` to obtain a user-scoped `{uuid}@{domain}` —
  multiple devices per user must collapse into one `RoomMember`.

**Kupferdraht handling:**

* One module for both directions (Step 95a, ADR-0027):
  `encryption/leaf_identity.rs` — `wire_identity` (build) and
  `parse_leaf_identity` (read: exactly three non-empty parts, the
  `{client_id}` segment verbatim — it is the same string the API uses
  as `from_client`). Member lists, kick target resolution and inbound
  sender attribution all read through it.
* Sender attribution: an inbound application message is attributed
  to the W-4 identity of the leaf openmls authenticated as its
  signer, not to the envelope's `qualified_from` / `from_client`
  (a cross-check only — a mismatch is logged). A non-Basic (e2ei
  X.509) leaf keeps the envelope attribution until X.509 identity
  parsing is verified; a malformed Basic leaf drops the message.

**Source:** the wire-android repository (GPL-3.0), read as a
protocol observation; the file and date were not recorded. LEARNINGS
§1.3 for the capabilities finding.

---

## W-5 — `external_senders` group-context extension is mandatory

**RFC / pure MLS:** RFC 9420 §12.1.3 defines the
`external_senders` extension. It is **optional** — many pure-MLS
deployments don't use it.

**Wire:** Every Wire MLS group MUST embed an `external_senders`
extension carrying the backend-signed external-sender public key, so
admin-driven Remove proposals signed by Wire's backend validate.

* Key is fetched at login from
  `GET /v14/mls/public-keys` (the `removal` /
  `ecdsa_secp256r1_sha256` entry).
* Key is embedded into every group Kupferdraht creates, and is **inherited**
  via the group context when Kupferdraht joins externally — the joiner
  doesn't add it, but the joiner's leaf capabilities must declare
  support for the extension type so leaf validation passes.

**Kupferdraht handling:** Step 1c.4 fetches and persists the key as a row
in `backend_mls_keys` (`("removal", "ecdsa_secp256r1_sha256")`). It
becomes a group-context extension at create time (Step 6) and is
inherited automatically on join.

**Source:** ADR-0004, [`api-notes.md`](api-notes.md) `GET /v14/mls/public-keys`.

### Trust model — why this is not a confidentiality weakening

RFC 9420 §12.1.8 defines the External Senders mechanism as a
standardised, public-by-design extension. A holder of the
external-sender signature key can:

* **sign Proposals** (typically Remove) — but Proposals do not take
  effect until a *member* of the group commits them.
* **sign External Init operations** — admitting outsiders to a group
  (used by Wire's guest-link / invite-link flows).

A holder of the key **cannot**:

* read application messages — it never holds the group's encryption
  secrets and is never added to the ratchet tree.
* send application messages — those require a member signature.
* directly execute a commit — commits are members-only.

Wire registers its backend as an external sender in every group so it
can drive admin operations: removing a deleted or suspended user from
every group they belong to, enforcing team-policy memberships, revoking
guest-link admissions. Member clients (including Kupferdraht) auto-commit
externally-signed Remove proposals after verifying the signature
against the key embedded in the group context — without this
auto-commit the admin operation never takes effect.

Two structural limits constrain misuse:

* **Visibility.** The `external_senders` list is part of the public
  group context; every member sees that the Wire backend has
  administrative signing authority in this group. Adding a new
  external sender requires a committed group-context change, which
  is visible to all members.
* **Observable effect.** A member removed by a backend-signed Remove
  sees the standard "you were removed" system message; covert removal
  is impossible.

The trust split is sharp: the backend is trusted with *membership
administration*, never with *content*. See `docs/THREAT_MODEL.md` L4
for the threat-model entry on this authority.

### Four Kupferdraht responsibilities

To stay interoperable with other Wire clients (wire-android,
wire-desktop, every other openmls-based Wire client):

1. Fetch the backend external-sender public key once at session start
   (`GET /v14/mls/public-keys`) and persist it locally.
2. Embed the key in the `external_senders` extension of every group
   created by Kupferdraht.
3. Validate inbound external-signed Proposals against the embedded
   key before letting them advance to a commit.
4. Auto-commit validated external Proposals with the same policy other
   Wire clients apply — otherwise admin operations behave differently
   in Kupferdraht-joined groups than in pure-Wire groups.

openmls supports the extension out-of-the-box as an RFC 9420 standard
feature. Relevant Kupferdraht Steps: Step 3 (group create) and Step 6
(event stream / proposal handling).

---

## W-6 — Ciphersuite pinned to `MLS_128_DHKEMP256_AES128GCM_SHA256_P256`

**RFC / pure MLS:** RFC 9420 §17.1 lists seven mandatory ciphersuites.
Most pure-MLS clients negotiate or default to
`MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519` (X25519 / Ed25519).

**Wire:** Pins the P-256 ciphersuite
(`MLS_128_DHKEMP256_AES128GCM_SHA256_P256`) — ciphersuite ID `0x0002`.
Reason: iOS clients can back the signature key with Secure Enclave,
which only supports NIST curves (P-256 / P-384 / P-521).

**Effect on Kupferdraht:** Mismatches surface deep in openmls as
`LeafNodeValidation::UnsupportedCredentials` when a KeyPackage with
the "wrong" ciphersuite is claimed into a Wire group.

**Kupferdraht handling:** `encryption/mls_client.rs::MLS_CIPHERSUITE` and
`MLS_SIGNATURE_SCHEME` are constants. Every `KeyPackage` and every
group Kupferdraht creates uses these — there is deliberately no
ciphersuite negotiation surface in the public API.

**Source:** LEARNINGS §1.2.

---

## W-7 — Leaves must declare `[Basic, X509]` credential capabilities

**RFC / pure MLS:** Capabilities are an application-policy choice. A
leaf that supports only `BasicCredential` is RFC-conformant.

**Wire:** Wire's e2ei-enrolled groups embed a
`RequiredCapabilities` extension that lists `X509` as a required
credential type. A leaf that declares only `Basic` is rejected by
openmls's leaf-node validation as
`LeafNodeValidation::UnsupportedCredentials` when claimed into such a
group, even though the leaf itself uses a `BasicCredential`.

The semantics are: "I, as a leaf, can interoperate with peers
presenting either Basic or X509 credentials." Kupferdraht does not itself
enroll for an X.509 cert (that would require e2ei certificate
enrollment, out of scope), but it *must* declare X509 in its
capabilities to coexist with e2ei-enrolled peers.

**Kupferdraht handling:** `encryption/mls_client.rs::leaf_capabilities`
returns `[Basic, X509]` and is used by every `KeyPackage` build, every
group create, and every external-commit join.

**Source:** LEARNINGS §1.3.

---

## W-8 — `Z-Client` is nginx-injected from the access-token claim

**RFC / pure MLS:** No analogue — the MLS protocol has no concept of a
Wire-style "client_id" header. MLS messages carry the sender's leaf
node index and the protocol layer derives identity from there.

**Wire:** The backend scopes every `/mls/*` endpoint to a specific
client device via a `Z-Client: <client_id>` header. The client does
**not** set this header — Wire's nginx layer strips client-supplied
`Z-Client` headers and re-injects the value from the `client_id`
claim inside the access token.

**Effect on Kupferdraht:** A login access token carries only the user
claim (the device wasn't registered yet at login time). Every
`/mls/*` call then fails with `400 "Header Z-Client is required"`.

**Kupferdraht handling:** After `POST /v14/clients` succeeds Kupferdraht
immediately calls `POST /v14/access?client_id=<id>` with the `zuid`
cookie to re-issue the access token bound to the new client. Every
subsequent refresh passes `client_id` along so the token stays
scoped.

**Source:** LEARNINGS §1.4.

---

## W-9 — `PUT /v14/clients/{id}` must register the MLS pubkey before key-package upload

**RFC / pure MLS:** No analogue — pure MLS has no separate "register
the signature key with the directory before publishing key packages"
step; the signature key is part of every key package.

**Wire:** Before
`POST /v14/mls/key-packages/self/{client_id}` is accepted, the device's
MLS signature public key must be registered with Wire via
`PUT /v14/clients/{client_id}` carrying:

```json
{
  "mls_public_keys": {
    "ecdsa_secp256r1_sha256": "<base64 of the raw P-256 public key>"
  }
}
```

Without this, the key-package upload is rejected with
`400 mls-protocol-error: "No key associated to the given identity and
signature scheme"`.

**Kupferdraht handling:** `LoginPipeline::upload_initial_key_packages`
issues the `PUT` immediately before the key-package upload.

**Source:** observations.md §19 (live-discovered during Step 1c live-smoke).

---

## W-10 — `GenericMessage` field 262000 carries Kupferdraht's `TaskPayload` extension

**Not a Wire behaviour** — the one entry in this file where *Kupferdraht*
extends the shared schema (Step 60a, ADR-0014). Task messages travel
as a `TaskPayload` submessage in `GenericMessage` field **262000**,
declared OUTSIDE the `content` oneof so a human-readable `Text`
fallback can ride alongside (encoding modes in
`message-types.md` → TaskPayload). Renumbered from 1001 on
2026-07-17 — pre-merge, before any deployment, so no compatibility
window was needed; the extra tag byte (3 vs. 2) buys headroom Wire's
schema growth (~20 fields in a decade) will never reach.

**Why it is safe against official clients:** protobuf decoders skip
unknown fields, so official Wire clients either render the `Text`
fallback (mode c) or treat the message like an unknown variant and
ignore it (mode a). This is the same forward-compatibility behaviour
Wire itself relies on when shipping new variants. **Live-verified
2026-07-17 against a live backend** (Step 60a smoke): the official
client rendered the mode-c fallback (mention intact) and showed
nothing for a mode-a bare payload — no error, no placeholder, no
crash; a follow-up text from the official client round-tripped,
confirming the group stays healthy. That verification ran under
field number 1001; protobuf's unknown-field handling is
number-independent. **Re-smoked 2026-07-17 under 262000** (Step 60c):
A→B round-trip in both modes; the official client rendered the mode-c
fallback (mention intact — a quote-reply to it arrived as an ordinary
text message, never touching task state); the mode-a bare payload was
fully UX-invisible — no render, **no unread badge, no
conversation-list bump, no push notification** (explicitly checked
after external review raised the badge seam).

**Field-number collision strategy:** 262000 sits far above Wire's
occupied range (currently < ~30), outside protobuf's reserved
19000–19999, and below the 4-byte-tag boundary at 262144. Should a
future Wire schema ever occupy 262000, the
mitigation is a coordinated move to a new high field number in the
next Kupferdraht release: inbound decoding accepts both numbers for one
release window, outbound switches immediately — full-state semantics
(ADR-0014) mean a single re-send heals any task that was emitted
under the colliding number. Watch Wire's `generic_message.proto`
upstream when bumping API versions.

**Kupferdraht handler:** outbound `wire/proto.rs::build_task_generic_message`;
inbound `sync/service.rs::dispatch_mls_message` (field-262000 check runs
BEFORE the oneof match) → `sync/dispatch/task.rs::dispatch_inbound_task`.

---

## What's *not* a deviation

A few things that look like deviations on first contact but turn out
to be either application-layer choices (which RFC 9420 explicitly
permits) or Wire conforming exactly to the spec:

* **Use of `BasicCredential` only, no X509 enrollment yet.** Kupferdraht
  uses `BasicCredential` per ADR-0001 — that's a Kupferdraht scope choice,
  not a Wire deviation. Wire's e2ei groups use `X509Credential` for
  enrolled peers; Kupferdraht coexists with them via W-7.
* **Welcome messages travel inline in the commit bundle when they're
  generated.** RFC 9420 §12.4.3 explicitly permits this; Wire's
  triple-concatenation under W-2 is a transport choice, not a
  protocol violation.
* **`external_senders` containing a backend public key.** RFC 9420
  §12.1.3 defines the extension exactly this way. It's the *mandatory*
  use under W-5 that's Wire-specific, not the extension itself.
* **No `GroupInfo` for a never-established conversation.** Wire
  pre-lists the self conversation's envelope (`epoch: 0`, a
  `group_id`) before any MLS group exists and answers
  `404 mls-missing-group-info` on `/groupinfo` until a member's first
  commit (live 2026-09-24, Step 86). RFC 9420 has nothing to publish
  before a group is created either — the first device creates the
  group and commits, later devices external-join. That is Wire's
  listing model, not a protocol deviation (`docs/wire/api-notes.md`
  §"Step 86 discovery").

---

## Maintenance

* When you add a new deviation entry, also add a row to the **Quick
  index** at the top.
* If a new deviation overlaps with an existing one, prefer extending
  the existing entry over splitting — the goal is one stop per
  topic.
* Live-validated observations always cite the date and the room id
  / endpoint where they were observed.
