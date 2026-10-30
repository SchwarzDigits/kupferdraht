# Step 15 — Typed identifier wrappers (`kupferdraht::ids::*`)

> **Status:** **superseded — not pursued** (2026-05-22).
> Step 15 was re-scoped to a minimal "&str ergonomics micropass"
> on branch `step/15-id-params-ref` after a design re-check. The
> remainder of this document is the *original* plan, retained as
> a record of the rejected option. See `CHANGELOG.md` §49 for the
> rejection rationale. The landed scope: re-scoped 2026-05-22 to
> `&str` ID params (CHANGELOG.md §49); soft typed wrappers rejected.
> Closes audit §4.1/§4.2/§6.3. Archived from PLAN.md.
> The audit items §4.1/§4.2/§6.3 listed here as closures
> *are* closed, just by the micropass — not by `kupferdraht::ids::*`.

---

## Step 15 — Typed identifier wrappers

Kupferdraht's public API uses `&str` / `String` throughout for every
identifier: user IDs, room IDs, device IDs, message IDs. The
`docs/api-mapping-matrix-rust-sdk.md` Open Questions section has
flagged this as a deferred clean-up since ADR-0009 — matrix-rust-sdk
uses ruma's typed identifiers for compile-time validation of
identifier mixing, and Kupferdraht accepts the resulting cosmetic Drift
as "Aligned" rather than "1:1" on every ID-bearing row (lines 62,
63, 82, 178, 179, etc.).

Step 14's verb-taxonomy + parameter-shape pass cleared the
neighbouring surface debts. Step 15 closes the last documented
API-shape debt before the public API surface is matrix-rust-sdk-
aligned on every structural axis.

### What "soft typed wrappers" means

Newtype wrappers around `String`, **without** parse-time format
validation. The wrapper exists for one purpose: compile-time
type safety against mixing identifier types (`Client::room(&RoomId)`
cannot accidentally be passed a `UserId`).

```rust
pub struct UserId(String);

impl UserId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl From<String> for UserId { fn from(s: String) -> Self { Self(s) } }
impl From<&str> for UserId { fn from(s: &str) -> Self { Self(s.into()) } }
impl Display for UserId { /* writes the wrapped string verbatim */ }
impl AsRef<str> for UserId { fn as_ref(&self) -> &str { &self.0 } }
```

No `Result`-returning constructor. No `FromStr` with a custom
error. The wrapper accepts whatever Wire serves, and whatever
the caller hands in. Format-validation responsibility stays at
the HTTP boundary (Wire's server rejects malformed identifiers
in 4xx responses, as it always has).

Rationale, alternatives, and trade-offs recorded in
**`CHANGELOG.md §43`** — particularly: Wire's swagger declares
user_id as RFC 4122 UUID format, but Kupferdraht has historically
been permissive (no UUID-format check) and Step 15 preserves
that permissiveness. The win is type safety, not validation.

### matrix-rust-sdk analog

ruma's `UserId`, `RoomId`, `DeviceId`, etc. are typed wrappers
with **strict parse validation** for MXID syntax (`@local:domain`,
`!opaque:domain`, etc.). Kupferdraht adopts the typed-wrapper concept
but skips the validation half — Wire's identifier format is
externally constrained and may evolve (SCIM IDs, e-mail-based
IDs, etc.), so a forward-compat-safe soft wrapper is preferable
to a strict parser that would break on the next Wire schema
extension.

This is an explicit Drift from matrix-rust-sdk recorded in the
api-mapping doc per the existing convention.

### Sub-steps

| Sub-step | Scope | Wire API v14 | Value |
|---|---|---|---|
| 15a | Introduce `kupferdraht-core/src/ids/` module with newtype wrappers `UserId`, `RoomId`, `DeviceId`, `MessageId`, plus the structured `QualifiedUserId { id: UserId, domain: String }`. Newtype around `String`; `Display` / `AsRef<str>` / `From<String>` / `From<&str>` / `new()` infallible constructor. No `FromStr`-with-error, no parse validation. Re-export from `lib.rs` so UDL bindings can reach them. | none (pure Rust) | high |
| 15b | Migrate `Client::*` public method signatures: every `&str` / `String` ID parameter becomes `&UserId` / `&RoomId` / `&DeviceId` / `&MessageId` as appropriate. Every `Option<&str>` ID return becomes `Option<&Wrapper>`. Internal storage stays `String` — only public boundaries change. | none (Wire layer unchanged) | high |
| 15c | Migrate `Room`, `Timeline`, `Account` public signatures the same way. Re-exports in `lib.rs` updated. | none (Wire layer unchanged) | medium |
| 15d | CLI boundary parse: every `--peer` / `--id` / `--user` / `--message` / `--device` flag value is wrapped into the typed ID at the clap-parse layer. Error messages on construct failure surface there once. The 32+ CLI subcommand handlers receive typed wrappers from the parser, not raw strings. | none | medium |

### Public API

```rust
// kupferdraht-core/src/ids/mod.rs
pub struct UserId(String);
pub struct RoomId(String);
pub struct DeviceId(String);
pub struct MessageId(String);

pub struct QualifiedUserId {
    pub id: UserId,
    pub domain: String,
}

// Each type has identical inherent + trait surface:
impl UserId {
    pub fn new(s: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
}
impl Display for UserId { ... }
impl AsRef<str> for UserId { ... }
impl From<String> for UserId { ... }
impl From<&str> for UserId { ... }

// Sample migrated signatures:
impl Client {
    // 15b — every ID parameter typed
    pub async fn room(&self, room_id: &RoomId) -> Option<Arc<Room>>;
    pub async fn join_room(&self, room_id: &RoomId) -> Result<(), KupferdrahtError>;
    pub async fn leave_room(&self, room_id: &RoomId) -> Result<(), KupferdrahtError>;
    pub async fn invite_members(&self, room_id: &RoomId, user_ids: &[UserId])
        -> Result<(), KupferdrahtError>;
    pub async fn create_dm(&self, peer: &UserId) -> Result<Arc<Room>, KupferdrahtError>;
    pub fn dm_room(&self, peer: &UserId) -> Option<Arc<Room>>;
    pub async fn dm_with(&self, peer: &UserId) -> Result<Arc<Room>, KupferdrahtError>;

    // 15b — return types
    pub fn user_id(&self) -> Option<&UserId>;
    pub fn device_id(&self) -> Option<&DeviceId>;
}
```

The `peer: &UserId` for DM methods is the soft-wrapper around the
qualified `"uuid@domain"` form — Step 14a's single-`&str`
convention transposes into the typed shape without losing the
"one value per ID" property (the typed wrapper carries the same
single qualified string internally).

### Wire Message Types

None — Step 15 is type-system polish on the public API surface.
The Wire HTTP layer (`wire::*`) keeps its existing internal
`QualifiedId { id, domain }` struct and continues to serialise /
deserialise verbatim per the Wire schema. Translation between
the public typed wrappers and the internal Wire types happens
inside `kupferdraht-core/src/wire/` modules (`as_str()` + `split_once('@')`
where qualified, direct `.0` access where opaque).

### Implementation notes

**15a — module layout.** Each ID type gets its own file under
`kupferdraht-core/src/ids/` (e.g. `ids/user_id.rs`, `ids/room_id.rs`,
…) — one type per file per CLAUDE.md §Design. `ids/mod.rs`
re-exports everything. `lib.rs` re-exports the `ids` module so
consumers see `kupferdraht::ids::UserId`, `kupferdraht::ids::RoomId`, etc.

Five types, ~30-50 lines each. The five files share a near-
identical structure — but per CLAUDE.md §"No Custom Macros",
the boilerplate is repeated by hand, not generated via
`macro_rules!`. Repetition over abstraction for this surface.

**15a — `QualifiedUserId` structured form.** Kupferdraht today
flattens the qualified form into a single string `"uuid@domain"`
(see Step 14a's single-`peer: &str` decision, CHANGELOG.md §42).
15a introduces `QualifiedUserId { id: UserId, domain: String }`
as a **structured** alternative — same information, more
type-honest about Wire's two-part wire-format shape. The two
forms coexist:

- `UserId` — the opaque wrapper around the qualified-string
  representation (`"uuid@domain"` rendered verbatim via
  `Display`). Used everywhere the public API takes one ID.
- `QualifiedUserId` — explicit `id` + `domain` fields for code
  that needs the parts. Cheap conversion `UserId::from(&q)`
  via `Display`.

`QualifiedUserId::parse(s: &str) -> Result<Self, ParseError>` is
the **one** fallible constructor in the module — required because
splitting `"uuid@domain"` into two parts can fail (no `@`,
multiple `@`). UserId/RoomId/DeviceId/MessageId stay infallible.

**15b/15c — migration is mechanical.** A `grep` like:

```
grep -nE 'pub (async )?fn .*: &str' kupferdraht-core/src/client/runtime.rs \
  | grep -E 'room_id|user_id|device_id|message_id|peer'
```

surfaces the full migration set up front. Each match is a 1-to-1
substitution: `&str` → `&RoomId` / `&UserId` / etc. as appropriate.
No business logic changes, no protocol changes, no storage-schema
changes. The migration is type-surgery on signatures only.

**15b/15c — internal `String` storage unchanged.** Kupferdraht's
storage layer (`SqliteStore`, `RoomRecord`, etc.) keeps storing
identifiers as `String` columns. The wrapper applies at
hydration time (`UserId::from(row.user_id)` in load paths) and
unwraps at serialisation (`row.user_id = user.as_str().to_owned()`
in store paths). No schema migration.

**15d — CLI parses at flag-parse time.** clap can build typed
arguments via `value_parser`:

```rust
#[arg(long, value_parser = |s: &str| -> Result<UserId, String>
    { Ok(UserId::new(s)) })]
peer: UserId,
```

Since the construct is infallible, the parser never errors — the
flag just yields a `UserId` directly to the handler. Handlers
that today take `&str` from the flag get `&UserId` and pass it
straight to `client.*`.

### Audit closures

Step 15 directly closes the following entries in
`docs/v1-api-ergonomics-audit.md` — per the project convention
of dropping resolved items in the same commit (see prior
closures `de471c4`, `1a60704`, and the §9.1/§9.2 drops in
`f76c9bc`):

- **§4.1 `Client::room(room_id: String)` takes owned String** —
  signature becomes `Client::room(&RoomId)`; the owned-String
  pain point's root cause is removed.
- **§4.2 `MessagesOptions::before(String)`** — signature becomes
  `MessagesOptions::before(&MessageId)`. Same root cause. (If
  `MessageId` is excluded from 15a's scope, §4.2 stays open
  with a note.)
- **§6.3 `"user_id missing @domain: {user_id}"`** — soft typed
  wrappers do not validate qualification, so the error site at
  `kupferdraht-core/src/encryption/mls_client.rs:272` (or wherever
  the current location is at landing time) becomes unreachable.
  Remove the variant or re-contextualise to a different invariant.

Partially touches (signature changes; semantic concern stays
open for a separate pass):

- **§3.4 `Account::qualified_user_id() -> Option<String>`** —
  return type becomes `Option<&QualifiedUserId>` (or
  `Option<&UserId>` depending on the chosen shape). Owned-String
  allocation per call is eliminated. The `Option<...>` shape
  stays — full §3.4 closure requires the separate "infallible
  accessors" pass that addresses §3.1/3.2/3.3 too.

Each sub-step's landing commit **must** drop the closed entries
from `v1-api-ergonomics-audit.md` in the same commit, not as a
follow-up. The commit message references the audit-doc edits
explicitly.

### Out of scope — explicitly _not_ in Step 15

- **Parse-time format validation.** `UserId::parse("garbage")`
  → `Ok(UserId)`. Wire's swagger says user_id is a UUID; Kupferdraht
  stays permissive (CHANGELOG.md §43).
- **`FromStr` with custom errors.** Only `QualifiedUserId::parse`
  is fallible (split on `@` can miss). All other ID types stay
  infallible-construct.
- **Validation against Wire's allowed formats.** No regex, no
  UUID check, no e-mail check. Wire rejects malformed inputs at
  the HTTP boundary as before.
- **The `Option<>` shape on accessors.** §3.1/3.2/3.3 stay open
  for a separate pass. Step 15 only changes the inner type.
- **Internal Wire-layer types** (`wire::dto::QualifiedId`) —
  unchanged. They keep matching Wire's serialisation contract
  verbatim per ADR-0008.
- **Storage schema migration.** Identifiers stay as `String`
  columns. Wrappers apply at the Rust boundary.
- **Custom derive macros.** Per CLAUDE.md §"No Custom Macros",
  the per-type boilerplate is hand-written across the five
  files. Repetition over abstraction.

### TDD starter tests

15a:
- `UserId::new("anything")` is infallible and returns a
  `UserId` whose `as_str()` matches the input verbatim.
- `UserId::new("")` is permitted (no validation); `as_str()`
  returns `""`. Documents the design choice.
- `format!("{}", UserId::new("uuid@domain"))` renders `"uuid@domain"`
  (Display round-trip).
- `UserId::from(String::from("x"))` and `UserId::from("x")` both
  yield the same `UserId`.
- A function `fn takes_room(r: &RoomId)` cannot be called with a
  `&UserId` — compile-fail test under `compile_fail` doctest or
  `trybuild`.
- `QualifiedUserId::parse("uuid@domain")` returns
  `Ok(QualifiedUserId { id: UserId("uuid"), domain: "domain" })`.
- `QualifiedUserId::parse("no-at-sign")` returns
  `Err(ParseError::Unqualified)`.
- `QualifiedUserId::parse("multi@at@sign")` round-trip behaviour
  documented: split on **first** `@` (consistent with
  `split_once('@')`).

15b/15c:
- Every existing test that constructed a `&str` ID literal
  continues to compile after replacing `"my-id"` with
  `&RoomId::from("my-id")` (or equivalent).
- `Client::user_id().map(UserId::as_str)` reproduces today's
  `Client::user_id() -> Option<&str>` value byte-for-byte.
- Mixing IDs across functions is a compile-error, not a runtime
  error.

15d:
- `kupferdraht-cli dm-show --peer "uuid@wire.com"` parses the flag
  into a `UserId`, calls `client.dm_room(&peer)`, and renders the
  result identically to today.
- `--peer ""` (empty) — handler still runs; lookup returns
  `None` (no validation at parse time, behaviour preserved).

### CLI commands

No new CLI commands. Existing commands re-typed at the flag-
parser layer. CLI flag names are unchanged; only the in-handler
type changes from `&str` to `&UserId` / `&RoomId` / etc.

### Open questions

1. **UDL exposure of typed wrappers.** UniFFI can export
   newtypes in two shapes: (a) as a `record` (single-field struct
   visible in bindings), or (b) as an opaque `interface` (handle
   type, opaque to bindings). A third option is to keep the UDL
   surface as `string` and accept Rust↔UDL Drift (the Rust API
   is typed, the binding API sees strings — same pattern as
   ADR-0009 already accepts for several other types). Decision
   deferred to the workspace-wide UDL pass (10e / 11e
   precedents apply); 15a ships without UDL re-exports.
2. **Should `MessageId` be in 15a's scope?** Closing audit §4.2
   depends on it. Argument for: symmetry — same shape as
   RoomId/UserId, same mechanical migration cost. Argument
   against: message_id is less load-bearing in the public API
   (mostly internal to Timeline). Tentative answer: include it,
   for the symmetry / §4.2 closure. To confirm during 15a's
   TDD-precursor discussion.
3. **`QualifiedUserId` vs `UserId` in method signatures.**
   `Client::dm_room(peer: &UserId)` accepts the opaque-string
   form; `Client::dm_room(peer: &QualifiedUserId)` makes the
   two-part structure explicit at the type level. CHANGELOG.md §42
   set `peer: &str` as the convention (single value); the typed
   counterpart is `&UserId`. The structured `QualifiedUserId`
   exists for callers who already have the parts split (Wire
   layer, internal pipeline code). Public API stays `&UserId`.

### Smoke test

```bash
# 15a — module exists and exports compile
cargo doc --workspace --no-deps --open
# (visual check: kupferdraht::ids::{UserId, RoomId, DeviceId, MessageId,
#  QualifiedUserId} all reachable)

# 15b/15c — existing CLI smoke continues to pass byte-for-byte
kupferdraht-cli login --backend production --email "$EMAIL"
kupferdraht-cli sync
kupferdraht-cli rooms
kupferdraht-cli send --id "$ROOM_ID" -m "hello after Step 15"
kupferdraht-cli dm-show --peer "$PEER@wire.com"
# (no behavioural change visible; type-system change only)

# 15d — flag parsing accepts arbitrary strings (no validation)
kupferdraht-cli dm-show --peer "garbage-not-qualified"
# (returns "not found" rather than parse-error)
```
