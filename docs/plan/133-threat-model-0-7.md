# Step 133 — THREAT_MODEL 0.7: every claim matches the code

**Status: commissioned 2026-11-01** (transferred from open.md §71: the
documentation claims the production-readiness review of 2026-09-25 measured
against the code; the precedent is Step 82, THREAT_MODEL 0.4). The document
is Volker's — version 0.6 of 2026-10-08, for review by Markus — so phase 1
decides each claim: narrow the text, or build what it promises.

## Problem

The review's verdict "not production ready as a security product in the
sense of THREAT_MODEL.md" named the gap between the document's claims and
the code, not an exploitable weakness. Re-verified 2026-11-01:

1. **Zeroization.** README line 96 ("compiler-enforced zeroization of
   sensitive material"), CLAUDE.md lines 18 and 185 ("compiler-checked",
   "Compiler-enforced", citing G4) and THREAT_MODEL G4 / G6 overstate it.
   Keys and credentials in their holders are `Zeroizing`, but
   `access_token()` returns a plain `String` copy (`current_access_token()`),
   `SessionRecord` holds the tokens as `String`, the login body carries the
   password into a JSON value, and message plaintext on the send and receive
   paths is never zeroized (only the resend payload is).
2. **Auth modes.** The "Auth modes" section describes `DeviceBound` as
   complete and `PasswordProtected` / `PasswordOnly` as interface stubs "at
   the type level"; none of the three names exists in the code.
3. **Reproducible builds** (the trust model's first paragraph) do not
   exist; no build is reproducible or attested.
4. **G4 "forward secrecy applies in the protocol and in local storage":**
   local forward secrecy needs key rotation and `secure_delete`, both
   Step 134; until then the sentence is a promise.
5. **Server trust.** With Basic credentials the backend is the key
   directory and can inject a device; "does not trust the server with
   content" has to be stated as an assumption (Step 95d makes a device
   visible, it does not prevent it; the same holds for every Wire client
   without end-to-end identity).
6. **Keystore architecture** points to "open.md §71 B4" for the Linux
   desktop limit; the limit is `docs/non-goals.md` NG-24 now.

## Scope

1. Phase 1 (Volker, with Markus): per claim, narrow or build.
   Recommendation: narrow all of them — "best-effort zeroization of keys
   and credentials"; the auth modes as a design sketch or dropped;
   reproducible builds out of the trust model until a build pipeline
   exists; G4 pointing at Step 134; the server-trust sentence as an
   assumption in the Assumptions section — unless a build is already a
   step.
2. THREAT_MODEL 0.7 with its Versions row; the README and CLAUDE.md
   wording; `docs/library.md` where it repeats a claim.
3. If phase 1 chooses "build" for zeroization: a non-`Clone` secret type
   with redacted `Debug` end to end (tokens, password, plaintext buffers)
   becomes its own step, not this one.

## Out of scope

- Code changes. This step changes documents; what phase 1 decides to
  build becomes a step.
- The Known Limitations and Appendix sections, unless a claim above
  touches them.

## TDD sketch

Doc pins in `tests/step_133_threat_model_pins.rs`: README, CLAUDE.md and
THREAT_MODEL carry no "compiler-enforced" / "compiler-checked" zeroization
claim; THREAT_MODEL names no auth mode the code lacks (the needle is a
phase-1 decision: absent, or marked as a design sketch); the Versions table
has a 0.7 row; no document points at open.md §71.
