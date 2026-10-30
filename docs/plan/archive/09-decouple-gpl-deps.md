# Step 9 — Decouple from GPL dependency stack

> **Status:** merged — ADR-0010. 9a–9e merged 2026-05-04/05; 9d live-validated 2026-05-05. Dependency graph has zero GPL code. Archived from PLAN.md.
>
> **Sub-steps:** 9a (failing tests + stub), 9b (Wire-compatible CBOR encoder), 9c (integration swap), 9d (live-Wire validation), 9e (Cargo.toml + deny.toml cleanup).
>
> **Historical names:**
> - "Step 11" — PLAN.md v1, pre-2026-04-21 ordering revision (per §Suggested implementation order key-changes).
> - "Step 9 (a–e)" — PLAN.md v2 (2026-04-23) onward, current.
>
> **Branch / commit prefix:** `step/9*-*`, `feat(step-9): …`, `chore(step-9): …`.

---

## Step 9 — Decouple from GPL dependency stack

Replace the three GPL-3.0 Wire crates (`proteus-wasm`,
`proteus-traits`, `cbor-codec`) with a minimal internal module
that generates Wire-compatible prekeys using the cryptographic
primitives already in Kupferdraht's dependency graph.

After this step, Kupferdraht's dependency graph contains no GPL code.
This **enables** a future license decision but does not make it.
Kupferdraht remains under its current license until a separate
strategic decision is taken in a follow-up ADR.

**Prerequisites:** Steps 1–8 complete and stable on `main`.
cargo-deny and cargo-about already installed and configured (from
the pre-Step-2 task). Interoperability test infrastructure (Kupferdraht
client talking to wire-android or wire-desktop in the same MLS
group) in place from Step 5.

**ADR:** ADR-0010.

### Why decouple before deciding

The license question is strategic, not technical. It depends on
factors that are not in scope for the implementation work:

- Whether downstream consumers (Natrium, CivicSeal, possible
  external adopters) require a permissively licensed dependency.
- Whether Kupferdraht positions itself as Apache-2.0 infrastructure or
  as an Open-Source-aligned reference implementation.
- The state of the broader Wire ecosystem at the time the decision
  is made.

Step 9 separates the **technical work** (eliminate GPL from the
graph) from the **strategic decision** (which license Kupferdraht ships
under). Doing the technical work first preserves both options.

After Step 9 the choice is open:

- **Stay GPL-3.0.** Easier coexistence with the broader Wire
  ecosystem; aligned with Kalium's posture; downstream consumers
  must also be GPL-3.0.
- **Switch to Apache-2.0.** Maximum freedom for downstream consumers
  (proprietary embedding, dual-license consumers, government
  agencies with their own license requirements); strategically
  positioned as infrastructure SDK rather than as a Wire-aligned
  client.
- **Switch to LGPL-3.0.** Conceptual middle ground but technically
  fragile in mobile and WASM contexts where the dynamic-linking
  model LGPL was designed for does not cleanly apply.

The decision is documented in a future ADR after Step 9 completes.

### Public API

No public API change. Prekey generation is an internal
registration step inside the existing `Client::register` path.
The Wire backend cannot distinguish a Kupferdraht client using the new
implementation from one using the old `proteus-wasm` crate,
because the bytes on the wire are identical.

### Wire API v14 endpoints

- `POST /v14/clients` — already implemented in Step 1. Step 9
  changes only the source of the `prekeys` and `lastkey` fields
  in the request body. The endpoint itself is unchanged.

### Crates used

Already in the dependency graph — no new dependencies:

- `curve25519-dalek` (Apache-2.0/MIT) for Curve25519 identity
  and prekey pairs.
- `ed25519-dalek` (BSD-3-Clause) for signatures.
- `ciborium` (Apache-2.0/MIT) for CBOR serialization.

Removed after this step:

- `proteus-wasm` (GPL-3.0)
- `proteus-traits` (GPL-3.0)
- `cbor-codec` (GPL-3.0)

### Sub-steps

**Step 9a — Prekey module skeleton with tests**

Write the test suite first (TDD per CLAUDE.md): the tests describe
what a Wire-compatible prekey set looks like, byte-for-byte, for a
known input seed. Capture known-answer vectors from the current
`proteus-wasm`-based implementation **before** removing it.

- New module: `kupferdraht-core/src/encryption/prekeys.rs` (final
  naming during implementation — follow matrix-rust-sdk-style
  conventions, not Proteus-style).
- Tests for: Curve25519 identity keypair generation from seed,
  prekey batch generation with IDs, Ed25519 signing over prekey
  public keys, CBOR serialization of the complete prekey bundle.
- All tests red; no implementation yet.

**Step 9b — Implement against tests**

Implementation using crates already in the graph. Goal: all tests
from Step 9a green. Known-answer vectors match byte-for-byte
what `proteus-wasm` produces for the same seed.

**Step 9c — Integration swap**

Replace the `proteus-wasm` call sites in `kupferdraht-core/src/client/`
(registration path, Step 1 code) with calls to the new internal
module. `kupferdraht-cli login --register-client` produces the same
wire-level bytes as before.

Regression smoke-test: the full smoke suite (Steps 1–8) passes
unchanged.

**Step 9d — Live interoperability validation**

Register a fresh Kupferdraht client against Wire staging using only
the new prekey code. Verify:

- Backend accepts the registration (HTTP 201 + returned client
  record).
- An iOS or wire-android client can establish an MLS group that
  includes the Kupferdraht client.
- Message send and receive work in both directions.

If any test fails, fix and repeat. Do not proceed to Step 9e
until live validation is fully green against three independent
test clients.

**Step 9e — Dependency cleanup (license-neutral)**

This sub-step removes the GPL dependencies but does **not** change
Kupferdraht's own license. The license decision is deferred.

- Remove `proteus-wasm`, `proteus-traits`, `cbor-codec` from
  `Cargo.toml`. Regenerate `Cargo.lock`.
- Remove the three GPL-3.0 exception entries from `deny.toml`.
- Tighten `deny.toml` allow-list to: Apache-2.0, MIT,
  BSD-3-Clause, BSD-2-Clause, ISC, MPL-2.0, Unicode-DFS-2016.
  GPL in any form: deny going forward.
- `cargo deny check` must be green.
- Regenerate `docs/THIRD_PARTY_LICENSES.html` via
  `cargo about generate`.
- Add a `CHANGELOG.md` entry documenting the before/after dependency
  state and the fact that the license decision is deferred.

**Kupferdraht's `LICENSE` file is NOT changed in this step.** The
project remains under its current license. Updating `LICENSE`,
the `license` fields in `Cargo.toml`, and the relevant CLAUDE.md
and README.md sections happens in a separate follow-up step that
is gated by a strategic license-decision ADR.

### CLI command (step 11)

No new CLI commands. The existing `kupferdraht-cli login
--register-client` path exercises the new prekey module
transparently.

### Smoke test (step 11)

```bash
# Full regression smoke from Step 1 onwards, against a fresh
# account registered with the new prekey code.

./scripts/smoke-full.sh --fresh-registration

# Additionally: cargo-deny with tightened allow-list must pass.
cargo deny check
```

Smoke passes means: new prekey code is accepted by Wire,
interoperates with other Wire clients, and the dependency graph
contains no GPL code. Kupferdraht's own license is unchanged; that
decision is intentionally deferred.

### What Step 9 does NOT do

- Does not change Kupferdraht's `LICENSE` file.
- Does not change the `license` field in any `Cargo.toml`.
- Does not commit Kupferdraht to Apache-2.0, GPL-3.0, or any other
  specific license.
- Does not require updating CLAUDE.md or README.md license posture.

These changes happen later, after a separate strategic decision
documented in its own ADR.

### Dependencies

- Steps 1–8 complete and stable.
- Pre-Step-2 cargo-deny / cargo-about setup task complete.
- Step 5 interoperability test infrastructure (Kupferdraht ↔
  wire-android / wire-desktop) operational.

### CLI commands

**Step 9 — GPL decoupling:** No new CLI commands. The existing
`login --register-client` exercises the new internal prekey module
transparently.
