# matrix-rust-sdk as the API model

Condensed from `docs/adrs/0009-matrix-rust-sdk-as-api-model.md`.

- **Shape the public API after matrix-rust-sdk:** `Client`, `ClientBuilder`, `Room`, `Timeline`, `SyncService`, `EventHandler`.
- **Organize modules by feature** (`client/`, `rooms/`, `timeline/`, `encryption/`, `sync/`, `storage/`), not Kalium-style lifecycle-scope nesting.
- **Keep Wire vocabulary inside `wire::*` only — names, raw values and the prose a consumer reads alike.** Every public name (Rust outside `wire/`, UniFFI, browser exports and `index.d.ts`, CLI verbs and flags, the custom-store contract) takes matrix-rust-sdk's term, or a plain English one where matrix has no concept — a Wire-only concept included. No Wire string as a public value, no Wire locator or key material in a public record. Docs say "Wire" only to explain the backend's behaviour, and cite Wire's interface (endpoint path, event type, status label) only in backticks.
- **Wire's names stay only where an interface to Wire binds them:** `wire/` and the Protobuf schema `proto/generic_message.proto` with its generated types; anywhere else only after the maintainer's approval beforehand, marked with a comment containing `Wire compatibility (ADR-0009)` that names the interface, and listed in the check's approved exceptions.
- **The check is `kupferdraht-core/tests/public_vocabulary.rs`.** Rename instead of listing: never add an exception to make it pass; a Step 163 sub-step removes its rows from the temporary list.
- **Do not use Kalium as a model for structure, naming, scope nesting, or implementation** — GPL derivative-work risk + JVM idioms.
- Kalium, and any other Wire repository, is permitted for one thing only: observing Wire's protocol behaviour (what an endpoint expects, what an event means, what a wire format looks like) when the Swagger is ambiguous → record every observation with its source in `docs/wire/api-notes.md` or `docs/observations.md`.
