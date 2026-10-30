# Reference Material

External material in `reference/` is **read-only** — input to our work,
never a product of it. Do **not** modify anything here. Nothing external
is checked in: the one Wire document is fetched on demand, the source
trees are consulted online at pinned revisions — see each entry. There
are no git submodules, so a downstream crate that consumes Kupferdraht
as a git dependency clones nothing extra, and a snapshot of the tree is
complete by itself.

This is the per-source detail behind CLAUDE.md "Implementation References"
(which holds the priority order). Each entry below states what the source
is and exactly what it may be used for.

## `wire.swagger.json` — Wire v14 API (fetched, not committed)

Wire's authoritative v14 OpenAPI document, generated and served by
wire-server (AGPL-3.0) at every backend's `/v14/api/swagger.json`. It is
Wire's, so it is not committed: `scripts/fetch-wire-swagger.sh` downloads
it from the backend named in `KUPFERDRAHT_SWAGGER_URL` to this path
(git-ignored) and prints its SHA-256. Kupferdraht was built
against the document with SHA-256
`aeb634bbdee6bd10e8e215741e1341d238f0b4b87829119d8a153a8b12034040`
(234 paths; first captured 2026-04-22, re-fetched byte-identical on
2026-10-07). A different hash means Wire changed the spec. The
authoritative HTTP reference (CLAUDE.md "Wire API: v14 Only"); deviations
and observations go into `docs/wire/api-notes.md`, never into the
document itself.

## matrix-rust-sdk — primary structural reference (Apache-2.0, Rust)

Consulted online at <https://github.com/matrix-org/matrix-rust-sdk>,
pinned for citations at revision
[`2c2e0f1b`](https://github.com/matrix-org/matrix-rust-sdk/tree/2c2e0f1b152c4a8dcbbb7b5347f5494a57516bc2),
the revision the former submodule tracked. The structural model for
Kupferdraht. **Permitted: everything** — structure,
naming conventions, module layout, UniFFI patterns, observer patterns,
direct code adaptation — Apache-2.0 permits reuse with attribution, so an
adapted passage names its origin in a `NOTICE` file (none exists yet:
Kupferdraht has taken the model, not the code). Consult
especially `matrix-sdk-ffi/` for UniFFI integration and `matrix-sdk/src/`
for the module layout Kupferdraht mirrors. Reference #1 in CLAUDE.md.

## Kalium — Wire-protocol observation only (GPL-3.0, Kotlin)

Not checked out here: consult it online at
<https://github.com/wireapp/kalium>, pinned for citations at revision
[`9ccecabb`](https://github.com/wireapp/kalium/tree/9ccecabb04742ec0afa30babe9925bea7313a17c),
the revision every existing citation in `docs/` links into. **Not** a
structural reference, for three reasons:

1. **License.** Kalium is GPL-3.0. Kupferdraht is written from the spec
   and must not take Kalium-derived code. Structural mirroring — type
   names, method names, scope nesting — is a gray area for
   derivative-work analysis and would put that provenance in question.
   matrix-rust-sdk eliminates this risk.
2. **Wrong idioms.** Kotlin Multiplatform idioms (suspend functions, DI
   scopes, `Either<Failure, Success>`, the Ktor plugin model) produce
   idiomatically wrong Rust. Rust-native SDKs use builders, `Arc<T>`,
   `tokio` tasks, `Result<T, E>`, feature-based modules.
3. **Wrong abstraction level.** Kalium couples app logic tightly to Wire
   concepts at every layer; Kupferdraht's goal is a backend-agnostic public
   API. Kalium was never designed for that.

**Permitted:** observe Wire's protocol behaviour — what an endpoint
expects, what an event means, what a wire format looks like — where
`wire.swagger.json` is ambiguous, and record every observation with its
source (permalink to file and revision) in `docs/wire/api-notes.md` or
`docs/observations.md`. Once, in September 2026, a review of Kalium's
production readiness served as a checklist of defect classes (file
modes, leftover journal files, logging, keystore handling) against
which Kupferdraht's own code was hardened (Steps 77–80, open.md §69);
that is learning from a review, not deriving code. The same rule covers Wire's other repositories
(proteus, wire-server, wire-webapp, wire-android). **Forbidden:** as a model for structure, naming, algorithms, data
structures, or module organization. (Wire field names that come straight
from the server are externally constrained, not a Kalium derivation — they
go into `kupferdraht-core/src/wire/` verbatim.) The Protobuf schema subset in
`kupferdraht-core/proto/generic_message.proto` was transcribed from the copy
of Wire's `messages.proto` that Kalium vendors at this revision; the file
keeps Wire's GPL notice and states the modifications in its header.
Reference #5 in CLAUDE.md.

## natrium — downstream consumer (private, Kotlin)

The Kotlin Multiplatform consumer of Kupferdraht lives in a private
repository and is not part of this one. It shows how the Kotlin API is
called and is consulted only for the consumer's perspective on the
public API.
