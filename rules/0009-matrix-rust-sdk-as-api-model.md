# matrix-rust-sdk as the API model

Condensed from `docs/adrs/0009-matrix-rust-sdk-as-api-model.md`.

- **Shape the public API after matrix-rust-sdk:** `Client`, `ClientBuilder`, `Room`, `Timeline`, `SyncService`, `EventHandler`.
- **Organize modules by feature** (`client/`, `rooms/`, `timeline/`, `encryption/`, `sync/`, `storage/`), not Kalium-style lifecycle-scope nesting.
- **The public surface speaks matrix-rust-sdk's language — names, values and the prose a consumer reads alike.** Every public name (Rust outside `wire/`, UniFFI, browser exports and `index.d.ts`, CLI verbs and flags, the custom-store contract) takes matrix-rust-sdk's term, or a plain English one where matrix has no such concept — a Wire-only concept included. A public value is a typed projection; `wire/` maps the backend's strings, locators and key material to and from it. Docs use the same terms, say "Wire" to explain the backend's behaviour, and cite its interface (endpoint path, event type, status label) in backticks.
- **Wire's names are the backend's: they live in `wire/` and the Protobuf schema `proto/generic_message.proto` with its generated types.** Anywhere else a Wire name stays only where an interface to Wire binds it, after the maintainer's approval beforehand, marked with a comment containing `Wire compatibility (ADR-0009)` that names the interface, and listed in the check's approved exceptions.
- **The check is `kupferdraht-core/tests/public_vocabulary.rs`.** Say it in matrix-rust-sdk's terms instead of listing: an exception is never added to make it pass; a Step 163 sub-step removes its rows from the temporary list.
- **Do not use Kalium as a model for structure, naming, scope nesting, or implementation** — GPL derivative-work risk + JVM idioms.
- Kalium, and any other Wire repository, is permitted for one thing only: observing Wire's protocol behaviour (what an endpoint expects, what an event means, what a wire format looks like) when the Swagger is ambiguous → record every observation with its source in `docs/wire/api-notes.md` or `docs/observations.md`.
