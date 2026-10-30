# ADR-0009: matrix-rust-sdk as Kupferdraht's conceptual API model

**Status:** Accepted
**Date:** 2026-04-22
**Deciders:** Volker Zöpfel

---

## Context

Kupferdraht's public API surface — the types, scopes, methods, and
event-handler traits that UniFFI consumers (Kotlin, Swift,
TypeScript-via-WASM) and the Rust crate's direct callers see —
needs a structural model. The question: whose existing API do we
shape Kupferdraht after?

The implicit model so far has been **Kalium**. PLAN.md v1 lists
Kalium prior-art tables under every Step. Early Kupferdraht types
(`Session`, `ConversationScope`, `JoinLinkScope`, `MessageScope`,
`UserSessionScope` as implicit parent) mirror Kalium's scope-
nested architecture and Kalium's naming. This was the path of
least resistance: Kalium solves the same problem against the same
backend, its structure is proven, matching it reduces the work of
inventing an API.

Two problems surfaced during the first implementation pass that
warrant revisiting this choice:

1. **License / derivative-work risk.** Kalium is GPL-3.0. Kupferdraht's
   long-term delivery posture targets Apache-2.0-compatible
   consumers (mobile SDK bindings, embeddable native libraries,
   CivicSeal-style downstream products). Structural API mirroring
   — method names, type names, scope nesting — is a gray area for
   derivative-work analysis. Even if the code is independently
   written, API copies can carry legal weight.
2. **Wrong abstractions for Rust.** Kalium is a KMP (Kotlin
   Multiplatform) library. Its architecture is shaped by JVM
   idioms: suspend functions that do platform-specific work
   behind them, `UserSessionScope` as a dependency-injection
   container, `Either<Failure, Success>` for error handling,
   tight coupling to Ktor's plugin model. Rust-native SDKs for
   the same domain (E2EE messaging) look quite different: they
   use builder patterns, `Arc<T>` sharing, `tokio` tasks,
   `Result<T, E>` errors, and organize code by feature modules
   rather than scope nesting. Mirroring Kalium in Rust produces
   idiomatically-wrong Rust.

The natural alternative model: **matrix-rust-sdk**. It is:

- Written in Rust, targeting Kotlin / Swift / WASM bindings via
  UniFFI (exact parallel to Kupferdraht's delivery surface).
- Apache-2.0 licensed (compatible with Kupferdraht's target).
- Production-deployed across Element, Element X, and third-party
  Matrix clients — tens of thousands of developers have built on
  it.
- In the same domain as Kupferdraht: end-to-end encrypted messaging
  with a server-mediated event stream, group chats, join
  protocols, identity recovery.
- Maintained by an organization (Element) that is not Wire —
  there is zero license / derivative-work risk in shaping
  Kupferdraht's API after it.

A third option — designing a custom Kupferdraht-native API from first
principles — was implicit in ADR-0002 (single Rust crate with
application-level types). It is workable but leaves consumer-
facing onboarding with no prior art to point at.

---

## Options Considered

### Option A: Continue with Kalium as the API model

Keep `Session`, `ConversationScope`, `MessageScope`,
`JoinLinkScope` nested under a top-level session type.
Method names, error variants, and observer shapes mirror
Kalium's `:logic` layer.

- **Pro:** Minimal disruption; the Rust code already started this
  way and partially ships.
- **Pro:** Contributors who know Kalium (Volker, Markus
  Trossbach — the current reviewers) onboard fastest.
- **Con:** License risk. Apache-2.0-compatible delivery is a
  stated goal of Kupferdraht; API derivation from GPL code is at
  best ambiguous, at worst blocking. For a hobby project
  explicitly designed to be adoptable by Apache-2.0-only
  consumers, this is an unforced constraint.
- **Con:** Kalium's abstractions are JVM-shaped. Rust consumers
  see a second-class API where Rust idioms have been Kotlin-
  fied.
- **Con:** "Produce a Wire client that looks like Kalium" is not
  a differentiating goal — it's a step sideways. If a consumer
  wants a Wire client that works like Kalium, they can already
  use Kalium.

### Option B: Shape Kupferdraht's API after matrix-rust-sdk

Top-level `Client` type; `Room` for conversation; `Timeline` for
message streams; `SyncService` for the event-stream manager;
`EncryptionSync` (or similarly-named subscope) for MLS
operations; builder-pattern `ClientBuilder` for construction.
Event handling via a `EventHandler` trait attached to the
`Client`.

- **Pro:** Apache-2.0 license compatibility — mirror at will.
- **Pro:** Established Rust idiom. Consumers from the matrix-
  rust-sdk ecosystem onboard instantly.
- **Pro:** Positions Kupferdraht for a **future backend transition**.
  If the project ever decides to speak Matrix-federation in
  addition to or instead of Wire, the public API already fits;
  only the transport/crypto internals change.
- **Pro:** Forces clean separation between Wire-API-specific
  layer (HTTP client, Swagger types) and public API.
- **Con:** Consumer-facing API diverges from Wire's
  domain vocabulary ("room" vs Wire's "conversation",
  "timeline" vs "messages"). The HTTP layer still uses Wire's
  names (they're externally constrained — see ADR-0008 and
  LEARNINGS.md §4.2), but the public surface uses matrix-rust-
  sdk's.
- **Con:** Requires renaming existing Kupferdraht types. Steps 1–5
  have shipped with Kalium-inspired names; a rename touches
  much of the public API.
- **Con:** Some matrix-rust-sdk concepts don't map cleanly to
  Wire. Wire has no concept of "rooms with state events";
  MLS-only conversations don't need a `RoomState` machinery.
  We'd take the skeleton, not every feature.

### Option C: Custom Kupferdraht-native API from first principles

Design the public API around Kupferdraht's specific goals (MLS-first,
Wire-backend-shaped, Rust-idiomatic) without following any
existing SDK.

- **Pro:** Maximum fit to Kupferdraht's specific needs. No forced
  concepts from another domain.
- **Con:** No onboarding prior art. Consumers learning Kupferdraht
  have to learn its idioms from scratch.
- **Con:** API-design bikeshedding. Every type name, every
  method signature, every scope boundary is a fresh decision
  with no guiding precedent.
- **Con:** Locks Kupferdraht into a single-backend future. If Matrix
  support ever becomes interesting, the API doesn't naturally
  fit, forcing either a second API surface or a breaking
  migration.

---

## Decision

**Kupferdraht's public API follows matrix-rust-sdk's conceptual model
and naming conventions (Option B). Kalium is explicitly not a
model for structure, naming, or implementation — Kalium, and any
other Wire repository, may be consulted only for protocol
observation (what a Wire endpoint expects, what an event means,
what a wire format looks like) where the Wire Swagger is
incomplete, and every observation is recorded with its source.**

The decision has three parts:

### 1. Top-level names and shapes mirror matrix-rust-sdk

Proposed correspondences (final names TBD during implementation):

| matrix-rust-sdk | Kupferdraht |
|---|---|
| `Client` | `Client` (replaces `Session`) |
| `ClientBuilder` | `ClientBuilder` (replaces ad-hoc `login` fn) |
| `Room` | `Room` (replaces `Conversation` in public API) |
| `Timeline` | `Timeline` (new in Kupferdraht) |
| `SyncService` | `SyncService` (replaces `listen_events`) |
| `EventHandler` trait | `EventHandler` (replaces `MessageObserver`) |
| `EncryptionSync` / `OlmMachine` | `MlsClient` or `Encryption` module |
| `StateStore` / `CryptoStore` | separate traits with per-backend implementations |

Internal Wire-specific types (HTTP request/response structs,
Wire event JSON types) keep Wire's vocabulary — see ADR-0008
and LEARNINGS.md §4.2. The translation happens inside Kupferdraht's
module boundary.

### 2. Module organization mirrors matrix-rust-sdk's crate layout

matrix-rust-sdk organizes by feature, not by lifecycle-scope:

- `matrix_sdk::account` (authentication)
- `matrix_sdk::rooms` (room state)
- `matrix_sdk::timeline` (messages)
- `matrix_sdk::encryption` (crypto)
- `matrix_sdk::sync` (event dispatch)
- `matrix_sdk::notification_settings`
- `matrix_sdk::sso`

Kupferdraht's equivalents will be named per feature rather than per
scope nesting. The exact module boundaries are
implementation-time decisions but the principle stands.

### 3. Kalium is demoted to protocol observation

The role Kalium played as "prior art for how to structure the
implementation" is removed. The role Kalium plays going forward
is narrower: **when the Wire Swagger is incomplete or ambiguous
about what an endpoint expects or what an event means, read the
Kalium code that speaks to that endpoint to see what Wire's
server accepts and sends, and record the observation with its
source (file and revision).** Use that purely as a factual
observation about Wire's server behavior, not as an
implementation template.

PLAN.md's "Kalium prior art" tables are removed. Individual
Wire API details that previously cited Kalium files now cite
the Wire Swagger endpoint instead.

---

## Consequences

### Positive

- **License posture cleaner.** Apache-2.0 delivery is
  achievable without derivative-work analysis on every API
  decision.
- **Onboarding story improves.** Anyone who has built on
  matrix-rust-sdk reads Kupferdraht's API and immediately recognizes
  the shape. The Rust messaging-SDK community is small; sharing
  idioms with its largest member multiplies Kupferdraht's usability.
- **Future-proofing against backend change.** If Kupferdraht ever
  grows Matrix support (distant future, speculative), the API
  already fits. Even if it doesn't, the abstraction boundary
  keeps Wire's deviations isolated in the transport layer.
- **Clean separation of concerns enforced by the model.**
  matrix-rust-sdk's layered design (crypto / sync / rooms /
  timeline as distinct modules with explicit interfaces)
  pushes Kupferdraht away from god-object scopes and toward a more
  maintainable module layout.

### Negative

- **Rename cost.** Every shipped Kupferdraht public-API type (from
  Steps 1–5 plus 2b/1 + 4a + 6-shipped-as-blocked) needs
  renaming or aliasing. Even conservatively scoped, this
  touches session.rs, conversations.rs, the UDL file, every
  CLI command, and every downstream consumer (currently just
  the CLI itself).
- **Domain-vocabulary dissonance.** Wire users see the UI term
  "conversation"; Kupferdraht consumers see `Room`. A mapping layer
  exists but the mismatch is visible at the API boundary. Docs
  and examples need to explain it.
- **Some matrix-rust-sdk concepts don't apply.** Matrix has
  "rooms with state events"; Wire has MLS groups with
  protocol-level membership. Matrix has "device verification";
  Wire has per-client MLS identities. We take the skeleton, not
  the whole skeleton.
- **Decision made partway through implementation.** The first
  implementation pass used Kalium-inspired names. This ADR
  invalidates those names for the second pass. The rename is
  planned-but-not-executed as of this ADR.

### Neutral

- The **HTTP layer does not change** due to this decision. Wire
  Swagger names (`qualified_conversation`, `key_package_ref`,
  `mls_public_keys`, …) stay as they are because they are
  externally constrained by Wire's server.
- The **MLS crypto primitives do not change**. `openmls`-derived
  types and operations stay where they are; only their
  enclosing Kupferdraht type names shift.
- **Protocol-level semantics are unaffected.** An MLS `Welcome`
  is still a `Welcome`; a commit-bundle still has the same
  three segments. Whether we call the enclosing method
  `process_welcome_message` (Kalium-ish) or `on_invite` /
  `on_room_added` (Matrix-ish) is naming, not semantics.

---

## Implementation Notes

- **Rename pass as part of the v14 reimplementation.** The
  ADR-0008 v14 upgrade already requires re-validation of every
  HTTP call. Pair it with the rename so one refactor pass
  covers both changes.
- **Matrix-rust-sdk repo is not added as a submodule.** The
  reference is conceptual; consult its public docs
  (<https://docs.rs/matrix-sdk>) and source (Apache-2.0,
  GitHub) on a per-feature basis, but don't vendor it.
- **The CLI follows.** `kupferdraht-cli` command names and output
  shapes are consumer-facing too; they get one rename pass to
  match the public API surface (e.g. `conversation-create` →
  `room-create`, `conversation-rejoin` → `room-join`,
  `listen` → `sync` or similar — exact names during
  implementation).
- **Kalium file references in commit messages / PLAN.md
  disappear.** This ADR retroactively scopes Kalium's role to
  protocol observation only.
- **observations.md notes stay.** Entries that referenced Kalium
  as "where we looked for API shape" are historical truth at
  the time — they are not rewritten. Future entries follow
  the "protocol observation only" rule and cite their source.

---

## References

- matrix-rust-sdk: <https://github.com/matrix-org/matrix-rust-sdk>
  (Apache-2.0)
- matrix-sdk crate docs: <https://docs.rs/matrix-sdk>
- ADR-0002 — single Rust crate with application-level types
  (structural precondition: Kupferdraht exposes types, not just
  primitives)
- ADR-0008 — target Wire API v14 (the reimplementation pass
  this ADR pairs with)
- LEARNINGS.md §4.1 "Kalium is a reference trap" — session
  finding that drove this decision

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-22 | Initial ADR |
| 1.1 | 2026-10-07 | Amendment, not a supersession: "API protocol observation" (JSON shapes) widened to "protocol observation" (what an endpoint expects, what an event means), with the duty to record every observation with its source. Why: this is what the project practised and documented from the start — edit chains, leave semantics, the events protocol — so the rule now states the practice instead of a narrower claim. The rule names any Wire repository, not Kalium alone: proteus (the prekey wire format), wire-server, wire-webapp and wire-android were read the same way. The Context's "Apache-2.0-compatible delivery" was the posture of April 2026; ADR-0016 (2026-07-23) chose GPL-3.0-only. The decision — matrix-rust-sdk as the model, Kalium never one — is unchanged. |
