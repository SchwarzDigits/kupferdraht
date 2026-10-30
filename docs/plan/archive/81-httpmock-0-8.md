# Step 81 — httpmock 0.7 → 0.8 (drop async-std, close RUSTSEC-2025-0052)

> **Status:** merged to `main` as `01057df` (2026-09-15; no smoke —
> test infrastructure only, CI green). httpmock 0.7 → 0.8: async-std
> GONE from the tree (verified `cargo tree -i`), RUSTSEC-2025-0052
> ignore removed — deny.toml is down to RUSTSEC-2026-0173 alone.
> ~160 mechanical rename sites across src unit tests + 21 test
> files (`assert_hits→assert_calls`, `hits→calls`,
> `matches→is_true`, `*_contains→*_includes`,
> `body`/`headers`/`query_params` field→method incl. the HeaderMap
> simplification of the copied host-capture helper). One corrected
> expectation (the LEARNINGS-5.22 class again): async-object-pool
> STAYS (0.2, the async-std-free line) — only async-std itself
> leaves. Full 70-target suite green = the migration proof;
> §5.13 pooling/Host-capture behavior re-verified by the
> 69/72/74/75 fixture families passing.

Commissioned 2026-09-10 (transferred from open.md §48, scoped
2026-05-22).

## Problem

`httpmock 0.7` pulls the discontinued `async-std` (dev-dep only,
never shipped — the suppressed RUSTSEC-2025-0052). `httpmock 0.8`
dropped async-std entirely. The probe build (2026-05-22) counted
~70 mechanical compile errors across ~21 test/http files.

## Scope

1. `httpmock = "0.7"` → `"0.8"` in `kupferdraht-core/Cargo.toml`
   (dev-deps).
2. Walk the known rename patterns: `assert_hits(N)` →
   `assert_calls(N)`; `query_params` field → method;
   `matches(|req| …)` → `is_true(…)`/`is_false(…)`; `req.body`
   field → `req.body()`.
3. Remove the RUSTSEC-2025-0052 ignore from `deny.toml`.
4. Re-verify the LEARNINGS §5.13 behavior (httpmock pools servers;
   Host-keyed captures) still holds under 0.8 — the 69/72/74/75
   fixture families depend on it. New-file `--test-threads=1`
   first-run rule stays.

## TDD (red first)

- Lockfile pin (the Step-76 pattern): `async-std` absent from
  `Cargo.lock` (async-object-pool stays — 0.2, the async-std-free
  line; the original "both absent" expectation was corrected in
  red→green, see the status header); deny.toml no longer carries
  `id = "RUSTSEC-2025-0052"`.
- Everything else is preservation: the full suite IS the test.

## Verify

`cargo tree -i async-std` → "did not match any packages"; full
gate. No smoke (test-infrastructure only — no runtime code path
changes).
