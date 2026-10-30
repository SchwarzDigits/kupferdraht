# Step 80 — Inbound-id path hardening (asset cache traversal)

> **Status:** merged to `main` as `ede08b7` (2026-09-11; red
> approved + implemented + review round folded same day, live smoke
> 2026-09-11 — the full Step-75 flow unchanged through the hardened
> boundary; CI green). Scope as planned PLUS the in-flight find:
> the login identity is built via the INFALLIBLE `UserId::new` from
> server parts, so `parse` hardening alone never covered the
> malicious-backend path — materialization-site guards
> (`open_default_store` / `open_mls_provider`, native + wasm arms)
> close every flow regardless of constructor. Four layers over one
> predicate (`storage::is_path_safe_id`): dispatch-boundary
> discard, guarded cache-name helper on both asset paths,
> `UserId::parse` charset, materialization guards. Review round:
> honest second-layer probe (persisted hostile id → download
> refuses, home walk clean), symmetric `open_mls_provider` pin,
> library.md contracts; the `debug_assert`-in-`new` suggestion was
> DECLINED per the Assertions rule (runtime data at four call
> sites — the §15 tokio-worker class). No CLI (hardening, no new
> surface). THREAT_MODEL touch-point handed to Step 82.

**Commissioned 2026-09-10** (Kalium-audit follow-up, second
round — the "backup-<id> could delete the live DB" finding's class
is "file paths derived from foreign data"; Kupferdraht's instance was
worse than a name collision: a remote peer reached the filesystem).

## Problem (verified 2026-09-10)

1. **Path traversal via the inbound `message_id`
   (`timeline/asset_receive.rs:304`).** `compute_cache_path` joins
   the envelope's `message_id` into the cache path
   (`assets/<message_id>.enc`). The id comes from the PEER's
   `GenericMessage` and is validated nowhere (dispatch
   `debug_assert`s non-empty only). A malicious peer sends an asset
   with `message_id = "../../../../tmp/x"`; on download the client
   writes the attacker-uploaded ciphertext blob (arbitrary bytes)
   to an arbitrary path with an `.enc` suffix —
   `create_dir_all(parent)` even creates directories along the way.
   Mitigations that exist: the write needs an active
   `download()`/`save_to` call (no drive-by on sync), and the fixed
   `.enc` suffix protects the DBs and direct exec targets. Still:
   remote peer → arbitrary filesystem write. The send side is safe
   (our own UUID); the peer-sent `filename` never reaches the
   filesystem (metadata only, `save_to` is caller-chosen).
2. **`UserId::parse` accepts path metacharacters
   (`identity/mod.rs:44ff`).** Only "exactly one `@`, both parts
   non-empty" is enforced; `/` and `..` pass, and the account id is
   joined into `base/accounts/<id>` (`storage/home.rs:38`).
   Exploitable only by the user's own (malicious) backend at login
   — low severity, same hardening class.

## Scope

1. **Validate at the dispatch boundary, not per path use**: an
   inbound `message_id` that is not a plausible message id (Wire
   clients send UUIDs; decide the exact accepted shape in
   orientation against live traffic + backend-spec) is rejected at
   `dispatch_mls_message` as a recoverable decode error — the
   envelope rule already logs-and-discards unknown shapes. Runtime
   data ⇒ error, never assert (Assertions rule).
2. **Defense in depth at the path site**: `compute_cache_path`
   additionally refuses any id containing a path separator or `..`
   (belt-and-braces if a new dispatch path forgets the boundary
   check), and the same guard goes into the send-side cache move.
3. **`UserId::parse` charset**: reject `/`, `\`, `..`, control
   characters in localpart and domain (shape stays "one `@`" — no
   full hostname grammar, just path safety).
4. THREAT_MODEL touch-point: note the trust boundary "envelope ids
   are attacker-controlled data" (flag for discussion, not an
   autonomous rewrite).

## TDD sketch

- Inbound asset message with `message_id = "../evil"` → dispatched
  as rejected/discarded, no attachment row, nothing written outside
  the assets dir (fixture: temp home, assert dir listing).
- `compute_cache_path` unit: traversal ids → `InvalidInput`.
- `UserId::parse` unit: `/`, `..`, `\`, control chars → error;
  existing valid ids (uuid@domain, federated domains with dots and
  hyphens) keep parsing.
- Preservation: normal UUID-id asset round-trip (Step 11 family)
  stays green.

## Out of scope

- Backup/export naming (no backup feature exists; Step 12 carries
  the "never derive sibling artifacts inside the live account dir"
  pin when it lands).
- SQLite `busy_timeout` (separate proposal, Step 81 candidate).
