# Step 91 — SQLite3MC in the browser (OPFS-backed wasm storage)

**Status: MERGED 2026-09-24** — `9d587a8` (ADR-0022 + rule, partial
supersession of ADR-0015) and `6bb5277` (the step), fast-forwarded onto
`main` by Volker the same day; archived with this header (the
orientation harness `spike/sqlite3mc-wasm/` retired at archive time,
last at `6bb5277` — its pins live on in `kupferdraht-core/tests/step_91_*.rs`,
the stand-in's wasm arm in `vendor/libsqlite3-sys/src/wasm.rs`, the
consumer smoke in `bindings/web/opfs-smoke/`). Scope shipped: the native
`SqliteStore` / `SqliteGlobalStore` / `openmls_sqlite_storage` on wasm
over the OPFS `sahpool` VFS (one amalgamation via
`SQLITE_WASM_RS_SOURCE_DIR`), the IndexedDB record store + MLS snapshot
deleted, `Kupferdraht.open` fail-closed outside a dedicated worker,
`Client.storageEngine()`, gap-B.28 fixed, CI wasm job
(check + clippy + headless Firefox via `scripts/wasm-test.sh`).
Size: `kupferdraht_bg.wasm` 3.57 MB → 6.74 MB (+89 %, no budget set).
Commissioned 2026-09-24 (Volker); **built 2026-09-24** on
`step/91-sqlite3mc-wasm` (ADR-0022 accepted the same day; W1 = the local
the sister project's checkout, W2 = CI compile check + clippy + headless Firefox via
`scripts/wasm-test.sh`; the six decision spots as proposed — partial
supersession of ADR-0015, fail-closed outside a dedicated worker,
rollback journal + default page size on wasm, keystore stays
IndexedDB + WebCrypto, the FFI wrapper in the vendored stand-in, the
wasm-bindgen line bump). gap-B.28 fixed as the first handgriff. Browser
pins green in headless Firefox and Chrome 153 (2026-09-24). **Smoke
2026-09-24:** no live Wire smoke (no backend surface); session survival
shown with `bindings/web/opfs-smoke/` — two headless Chrome sessions on
one persistent profile and origin, `Kupferdraht.open` in a dedicated worker
`OPEN-OK` both times (the second one reopens the `global.db` the first
wrote, under the keystore's persisted key). The login vertical over OPFS
against a Wire instance stays open (open.md gap-B.29). Driver: one
encryption engine across every target. Step 90 (ADR-0021) moves the native
targets to SQLite3MC; the browser still runs ADR-0015 Part 2 —
IndexedDB object stores with per-record AES-256-GCM and plaintext
index keys (a deliberate metadata-for-functionality trade at the
time). SQLite3MC ships wasm builds, and the sister project already runs it
in production over OPFS (`sqlite-wasm-rs` + the `sahpool` VFS), so
the browser can now get the same whole-DB encryption — closing the
metadata leak and letting wasm run the same SQL storage code the
native targets use.

**Sequencing: after Step 90 lands.** This step inherits the 32-byte
`DbKey`, the cipher decision, and the dependency mechanics from the
native swap; running it against pre-90 main would redo both.

## Decision first (phase 1/7, with Volker)

This step supersedes **only Part 2** of ADR-0015 (persistence).
Part 1 (WASM-only delivery, no JS port) and Part 3 (hand-written
wasm-bindgen surface) stand untouched. Orientation runs `new-adr`
with Volker before any implementation: SQLite3MC-over-OPFS vs. the
shipped IndexedDB per-record design, with the honest trade-offs
(OPFS availability/permissions across browsers, `sahpool`'s
single-writer semantics vs. IndexedDB's transactions, worker
requirements). The verified facts and the sister project's precedent are
the evidence base; the ADR decides, this plan does not preempt it.

## Inputs required from Volker

- **W1**: repo/path of the sister project's SQLite3MC wasm integration
  (`sqlite-wasm-rs` + OPFS `sahpool`) — the template, as I1 was for
  Step 88/90. We adopt, not reinvent.
- **W2**: wasm test execution policy — headless-browser runs
  (`wasm-pack test --headless`) need a browser and OPFS; Node has
  no OPFS. Local-only with a CI compile-check (today's zero-
  execution stance, 20e precondition), or a CI browser job?

## Scope

1. **Orientation to verify, not assume**: build SQLite3MC via
   `sqlite-wasm-rs` for wasm32 per the W1 template; confirm the
   `chacha20` scheme + raw 32-byte `PRAGMA key` works identically
   to native (the Step-88 P1/P2 pins as the reference); confirm
   OPFS `sahpool` persistence survives a page reload; measure the
   wasm binary size delta (record it, no budget set here).
2. **ADR** (see above) + same-named `rules/` file.
3. **Implementation**: the wasm `Store`/`GlobalStore` backends move
   from `storage/indexeddb.rs` to the SQLite path — ideally the
   same schema/migrations the native store runs (refinery grouped,
   Step 87), behind the unchanged `Store` trait; the MLS snapshot
   (W3 path) and the keystore follow the same move. `DbKey` (32
   bytes post-90) keys the DB directly; the per-record `aes-gcm`
   helpers and plaintext index keys are deleted with the old
   backend.
4. **Gap closure as acceptance**: the Step-89 expiry sweep — a
   documented no-op on the IndexedDB store — runs identically on
   wasm afterwards (`bindings.md` note removed in the same commit).
5. **CLI/consumer proof**: wasm has no CLI; the phase-6 equivalent
   is the wasm-bindgen demo/consumer surface exercising open →
   write → reload → read → expire against OPFS in a real browser,
   plus the wasm test suite green under the W2 policy.
6. **Closes gap-B.28 (decided by Volker 2026-09-24)**: the wasm target
   has been red since Step 75 — `wasm/manager.rs` matches
   `TimelineItemKind` without the `TaskListMarker { .. }` arm (E0004),
   unnoticed because nothing in CI builds wasm32. This step adds the
   arm (rendering the marker in the JS-facing timeline view) and a
   `cargo check -p kupferdraht-core --target wasm32-unknown-unknown` job to
   CI, so a red wasm target is a red build from here on; the phase-3 red
   suite starts from a compiling wasm crate.

## Out of scope

- Any change to the native storage path (that is Step 90, already
  landed by then).
- A JS port or bindings-surface redesign (ADR-0015 Parts 1+3).
- Migration of existing browser data: none — test-only deployment,
  fresh origin storage (ADR-0021 context precedent); said plainly
  in the CHANGELOG entry.
- Multi-tab concurrent writers: `sahpool` is single-writer; V1
  documents the constraint (as the sister project does) instead of building
  cross-tab coordination.

## TDD sketch (red first)

- Trait conformance: the existing storage trait tests compile and
  run for wasm32 against the new backend (wasm-pack test, W2
  policy) — red while the backend is the IndexedDB stub.
- Crypto pins (ported from the Step-88 harness): wrong key fails to
  open; reopen with the right key succeeds; a re-run after reload
  sees persisted rows (OPFS survival).
- Expiry: the Step-89 sweep tombstones an expired row on the wasm
  store (today: red by design, the documented no-op).
- Preservation: the native suite untouched; wasm binary builds with
  zero warnings.
