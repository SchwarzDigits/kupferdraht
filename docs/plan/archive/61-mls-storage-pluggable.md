# Step 61 — Unify the MLS-store open behind one crate-internal seam

> **Status:** MERGED to `main` — `876e9c0`, PR #44 (merge `b8dd044`,
> 2026-07-07). Scope: native-safe refactor — no public API / FFI / Wire /
> behaviour change — collapsing the duplicated `mls.db` opens into a single
> crate-internal function (`open_mls_provider`) so the browser/IndexedDB
> backend (a later step) has exactly one place to `cfg`-swap; first step of
> the browser/WASM migration tracked in natrium's
> `natrium-core/docs/kupferdraht-wasm.md` and `kupferdraht-wasm-storage.md`. No dated
> live-smoke record in this file — its own Verification section names "live
> single-account smoke `login` → `resume` → `send`" as the acceptance gate but
> does not log a completed run.
>
> **Historical names:**
> - "Step 43" — named on the `wasm` branch (natrium-tracked migration step),
>   through the 2026-07-07 merge to `main`.
> - "Step 61" — 2026-07-16 onward, current. Renamed during
>   `chore/docs-catchup` lifecycle housekeeping: step number 43 was already
>   taken by the merged-and-archived `43-custom-backend-url` (merged
>   2026-07-05, PR #40); 60 was skipped because it is reserved on another
>   branch.
>
> **Branch / commit prefix:** committed on `wasm` (the WASM migration collects
> its steps on one branch rather than one branch per step); `step/43-*` commit
> messages (historical prefix, predates the 2026-07-16 rename — see
> Historical names).

---

## Goal

Today the app-data store (`kupferdraht.db`) is obtained through the public
`StorageProvider` trait (`open_global` / `open_store`), but the **MLS store
(`mls.db`)** is opened by **two** near-identical inline helpers that reach for
`KupferdrahtHome` + a `DbKey` directly and build a `KupferdrahtProvider`:

- `client/login_pipeline.rs::open_mls_provider` (`:79`), used by
  `upload_initial_key_packages` (`:259`) and `resume_existing_mls_state` (`:458`);
- `client/resume.rs::resume_mls_client` (`:285`) — its own copy of the same
  open-connection / `apply_whole_db_key` / `KupferdrahtProvider::open` sequence.

Collapse both onto **one** crate-internal seam, so:
1. the browser/IndexedDB MLS backend (a later step) has a single, obvious place
   to `cfg`-swap the store, and
2. the copy-pasted open disappears.

## Design — and why *not* the public `StorageProvider` trait

The obvious symmetry would be `StorageProvider::open_mls(account_id) ->
KupferdrahtProvider`. **We deliberately do not do that.** `StorageProvider` is public
API, and a `pub trait` method cannot return the crate-private `KupferdrahtProvider`
(Rust E0446, "private type in public interface"). Making `KupferdrahtProvider` public
cascades: it `impl OpenMlsProvider` with `type StorageProvider =
SqliteStorageProvider<JsonCodec, Connection>`, so `JsonCodec` and the openmls
provider types would leak into the public API too. That is a real public-surface
growth we don't want, and — crucially — **it is not needed for the WASM goal**:
the browser backend is an *internal* `cfg`-gated build of `kupferdraht-core`, not an
externally-injected provider. (External MLS-backend injection by third-party
`StorageProvider` impls isn't needed by anyone today; it can be added later if it
is — see Out of scope.)

So the seam stays **crate-internal**, in the `client` layer where it already
bridges storage (`KupferdrahtHome`, `apply_whole_db_key`) and encryption
(`KupferdrahtProvider`):

- `client/login_pipeline.rs::open_mls_provider` becomes `pub(super)` — the one
  shared seam. Signature is unchanged
  (`(home: Option<&KupferdrahtHome>, account_id, db_key: Option<&DbKey>) ->
  Result<KupferdrahtProvider>`): file-backed under `KupferdrahtHome::account_mls` +
  `apply_whole_db_key` when a home is configured, in-memory otherwise.
- `client/resume.rs::resume_mls_client` drops its inline open and calls
  `super::login_pipeline::open_mls_provider(Some(home), account_id, mls_key)`.
- The public `StorageProvider` doc comment records that `mls.db` is opened
  through this crate-internal seam, not the trait.

The `mls_key: Option<&DbKey>` threading (loaded once in `builder.rs:234/311` via
`load_or_create_key(account-<id>-mls)`) is left as-is — it already flows cleanly;
removing it would need the keystore at the call sites, out of proportion for this
step.

**Where WASM plugs in (later step):** `open_mls_provider` is the single `cfg`
chokepoint. On `wasm32` it will construct a `KupferdrahtProvider` over an
IndexedDB-backed openmls store instead of a keyed `rusqlite::Connection` — the
`KupferdrahtProvider` *type* stays, its backing store switches by target. That work is
out of scope here; this step only guarantees there is exactly one place to do it.

## What changed

- `storage/mod.rs` — trait doc comment only (points at the crate-internal seam);
  the `StorageProvider` trait itself is **unchanged**.
- `client/login_pipeline.rs` — `open_mls_provider` is now `pub(super)` with a
  doc note that it is the shared MLS-open seam.
- `client/resume.rs` — `resume_mls_client` routes through
  `open_mls_provider`; its inline `Connection::open` / `apply_whole_db_key` /
  `KupferdrahtProvider::open` (and the two function-local `use`s) are deleted.

## Verification (TDD / regression)

Behaviour is unchanged — same path (`accounts/<id>/mls.db`), same key
(`account-<id>-mls`), same `KupferdrahtProvider`. The existing suite is the gate:

- `tests/storage_provider.rs::kupferdraht_db_and_mls_db_share_the_per_account_directory`
  (mls.db still lands in `accounts/<id>/`),
- the login→register and resume integration tests (`step_3_room_join`,
  `step_7c_login_ready_client`, resume tests) exercise both call sites,
- `cargo build --workspace` (0 warnings), `clippy --workspace --all-targets -D
  warnings`, `cargo test --workspace`,
- live single-account smoke `login` → `resume` → `send` as the acceptance gate.

## Out of scope

- The WASM/IndexedDB MLS backend and the `cfg`-branch inside `open_mls_provider`
  — the step that consumes this seam.
- Putting MLS on the public `StorageProvider` trait / making `KupferdrahtProvider`
  public (external MLS-backend injection) — deferred; not needed for WASM.
- The `Send`-bound `cfg`-conditionalisation of the `Store` / `GlobalStore`
  traits (rides with the "make `kupferdraht-core` link on wasm" step).
- **D-storage-1** (which fields stay plaintext on web) — a policy decision for
  the IndexedDB step; see natrium `kupferdraht-wasm-storage.md` §5.
