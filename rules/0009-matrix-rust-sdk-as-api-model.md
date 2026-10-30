# matrix-rust-sdk as the API model

Condensed from `docs/adrs/0009-matrix-rust-sdk-as-api-model.md`.

- **Shape the public API after matrix-rust-sdk:** `Client`, `ClientBuilder`, `Room`, `Timeline`, `SyncService`, `EventHandler`.
- **Organize modules by feature** (`client/`, `rooms/`, `timeline/`, `encryption/`, `sync/`, `storage/`), not Kalium-style lifecycle-scope nesting.
- **Keep Wire vocabulary inside `wire::*` only** — Wire field names never reach the public API.
- **Do not use Kalium as a model for structure, naming, scope nesting, or implementation** — GPL derivative-work risk + JVM idioms.
- Kalium, and any other Wire repository, is permitted for one thing only: observing Wire's protocol behaviour (what an endpoint expects, what an event means, what a wire format looks like) when the Swagger is ambiguous → record every observation with its source in `docs/wire/api-notes.md` or `docs/observations.md`.
