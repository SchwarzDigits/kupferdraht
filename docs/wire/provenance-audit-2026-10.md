# Provenance audit against Kalium (October 2026)

Before publication, Kupferdraht's tree was compared with Wire's Kalium
to answer one question: does the code show structural or textual
similarity to Kalium beyond what the Wire protocol dictates? Kalium's
source had been in the development tree as a git submodule for the whole
development period (`docs/wire/kalium-readings.md`), so the question
could not be answered by the stated method alone.

**Trees.** Kupferdraht at the state of this document; `wireapp/kalium`
at `9ccecabb04742ec0afa30babe9925bea7313a17c`, the pinned revision.
Baseline for the automated layer: `matrix-org/matrix-rust-sdk` at
`2c2e0f1b`, an unrelated Rust project compared with the same Kalium tree
by the same scripts, so that every number has a reference value.

## Automated layer

Identifiers were split into words and compared after removing the
vocabulary of Wire's OpenAPI document and of the Protobuf schema;
comments were compared as six-word sequences (licence headers excluded);
string literals of ten characters or more, uncommon numeric literals,
SQL table and column names and module names were compared directly.

| measure | Kupferdraht vs Kalium | baseline vs Kalium |
|---|---|---|
| multiword identifiers shared, outside the protocol vocabulary | 3.4 % | 2.2 % |
| shared six-word comment sequences | 2 | 53 |
| shared string literals not in the OpenAPI document | 23 | 10 |
| shared uncommon numeric literals | 26 | 44 |
| shared SQL table names | 0 of 22 | 3 |
| shared SQL column names outside the protocol vocabulary | 1 | 3 |

Every shared identifier, string and number was inspected. The shared
identifiers are Wire API operations and fields the vocabulary filter
did not know (claim key packages, register client, cookie label,
delivery tag and the like); the strings are header names, event types,
error labels and property keys the server sends; the numbers are small
test values. One enum matched a Kalium type outside the schema-dictated
message kinds: `FileSharingPolicy` (below).

## Reading layer

Eight module pairs were read side by side, Kupferdraht's files in full
against their Kalium counterparts, comparing decomposition, order of
operations where the protocol leaves it open, names that are not API
names, comment wording, constants, error texts, test structure and
fixtures, and data models. Each similarity was classed as dictated by
the protocol, generic for any implementation, or distinctive.

| pair | verdict | distinctive items |
|---|---|---|
| login, session, token refresh | protocol only | none |
| client registration, devices, prekeys | review | field order of the register-client body followed Kalium's DTO |
| MLS | protocol only | key-package pool tuning (100, half, 24 h) — the same in Wire web and iOS, so a convention |
| conversations, users | protocol only | none |
| message content, send pipeline, inbound dispatch | protocol only | none |
| assets | protocol only | the file-sharing gate (consumer of the item below) |
| notifications, sockets, sync engine | protocol only | none |
| feature configs, SSO, storage schema | review | `FileSharingPolicy`'s three-state model; the CLI's SSO redirect default |

The readings also recorded, pair by pair, where Kupferdraht's design
differs from Kalium's in the same area: the two-phase login handle and
cross-process token refresh, the self-generated Proteus prekey bundle,
the inverted commit lifecycle and opposite pending-proposal policy in
MLS, the stable-key edit chain chosen against Kalium's id rewrite, the
doorbell-only legacy socket, the raw feature-config tables, and a SQL
schema that shares no table name with Kalium's.

## Findings and what was done

1. **`FileSharingPolicy`** had the variants `Disabled`, `EnabledAll`,
   `EnabledSome(Vec<String>)`, the shape of Kalium's `FileSharingStatus`,
   and parsed an allow-list from a `config.allowedTypes` field that no
   Wire backend defines; the origin was a documented survey of Kalium's
   feature model (`docs/wire/asset-protocol.md` Q7). The enum is
   `Disabled | Enabled` since ADR-0012 revision 1.3.
2. **Name-only echoes**: a trait and a struct that happened to carry the
   same descriptive names as unrelated Kalium types were renamed
   (`TokenRefresh`, `AssetMultipart`); the register-client body is now
   `NewClientRequest`, after the OpenAPI schema's name, with its fields
   in the schema's order.
3. **CLI default**: the SSO redirect defaults `kupferdraht://sso-login/success`
   and `…/failure` matched Kalium's constants; they are
   `kupferdraht://sso/done` and `kupferdraht://sso/error`.
4. **Fixtures and comments**: a Wire infrastructure hostname and a
   `wire.com` address in test fixtures became example domains; a comment
   that referred to a deleted prototype and one that overstated what the
   server accepts were corrected; two documentation statements about
   Kalium that were wrong were corrected in place.
5. **Disclosure**: the readings index lists the quote-hash conformance
   vectors that were taken from Kalium's tests and replaced, the
   WebSocket prototype of April 2026 that was deleted the same day, and
   one case where a comment's attribution was changed from "read in
   Kalium" to "observed live".

## Limits

The automated layer is lexical; the reading layer was performed by
language-model agents with the full files of both sides and reviewed by
the maintainer. Kupferdraht's keystore, recovery, task and case
subsystems, its FFI, wasm and CLI layers were not paired because Kalium
has no counterpart for them. The comparison scripts are not part of the
repository; the method is described above so that it can be repeated.
