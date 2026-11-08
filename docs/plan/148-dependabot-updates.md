# Step 148 — Dependencies kept current by Dependabot

**Status: in progress** — commissioned 2026-11-06 from Step 142's
retrospective (LEARNINGS §5.80) as a check of upstream repository
advisories; turned into this step on 2026-11-08 (Volker): the lockfile
is kept current by a weekly Dependabot pull request that a maintainer
reviews, instead of a tool that judges advisories. After Step 149,
before Step 150 and the v0.2.1 release. Red test approved by Volker on
2026-11-08.

**Course change 2026-11-08 (Volker).** Built on 2026-11-07 and dropped
before it was committed: an unpublished workspace crate that read the
published advisories of all 255 GitHub repositories behind `Cargo.lock`
and decided from their free-text "vulnerable" and "patched" fields
whether one applies to a locked version, with ignores per advisory and
a daily workflow. Its first run reported seven entries and decided one
of them itself (inout 0.2.5). The other six needed a reader: four
because the advisory's own fields were malformed (a range left open
after every fix, the range `none`, a range in the patched field, a bare
version), two because they concern a feature the rusqlite fork does not
build. Measured the same day:

- Of the 79 advisories published in those repositories, 56 are in
  GitHub's reviewed database, so Dependabot alerts on them (half within
  three days of publication, the slowest after 69); 7 more are in
  RustSec only; 16 are in neither.
- Eight of the 16 applied to locked versions: openmls's three (Step
  142, a matter of the fork) and five in transitive crates — zerovec,
  rustix, icu_normalizer and quinn-udp (Step 149; quinn-udp locked,
  never compiled) and inout (this step). All five were fixed by a
  lockfile update within the manifests' requirements: a lockfile kept
  current takes such a fix without anyone knowing the advisory.
- 240 of RustSec's 455 advisories of 2026 carry a GHSA alias, so
  Dependabot's alerts do not replace `cargo deny`.

The SchwarzDigits forks (openmls, rusqlite, refinery; ADR-0042) follow
upstream in their own repositories, not through Kupferdraht's release
procedure (Volker).

## Problem

1. **Some advisories reach no scanner.** A maintainer's repository
   advisory reaches GitHub's reviewed database, which Dependabot matches,
   only once GitHub has reviewed it, and RustSec only if someone files
   it. Five such advisories stood on transitive crates in `Cargo.lock`
   until Step 149 and this step moved them.
2. **The lockfile moves only when a step moves it.** Nothing proposes
   updates; Step 149 moved 16 packages within their requirements at
   once.
3. **`cargo deny` runs only on a pull request or a push to `main`.** An
   advisory published between two pull requests waits for the next one.

## Scope

1. **`.github/dependabot.yml`** with two entries, each weekly, with a
   seven-day cooldown and all updates in one group:
   - `cargo`: `versioning-strategy: lockfile-only` and
     `allow: dependency-type: all` — every update `Cargo.lock` can take
     within the manifests' requirements, transitive crates included,
     which is what `cargo update` does. A release that needs a manifest
     change (a new major, a raised minimum) is left to a step.
   - `github-actions`: the actions the workflows use.
2. **`ci.yml` runs daily** on a schedule, and only the `cargo deny` job
   runs on it.
3. **inout 0.2.6** (GHSA-2943-gp7x-qg39, low, published 2026-11-07, in
   neither database): the lockfile, `THIRD-PARTY-NOTICES` and the
   regenerated `docs/THIRD_PARTY_LICENSES.html`; a pin test holds the
   floor.
4. **Docs:** `README.md` ("Dependency updates" after the gate), the
   changelog (Changed: the weekly pull request and the daily `cargo
   deny`; Security: inout 0.2.6).
5. **How a Dependabot pull request lands** (Volker, 2026-11-08): it is
   no step. CLAUDE.md's "Branches and Commits" gains the exception — it
   merges once CI is green and the third-party notices are updated on
   its branch, without a smoke. Weekly, not monthly, although each pull
   request needs that update by hand. Dependabot's security updates are
   a repository setting (off today), Volker's outside the repository.
   The first pull request shows whether the required `license/cla` check
   passes for `dependabot[bot]`.

## Out of scope

- A check that reads repository advisories and judges them (above).
- The forks' upstream tracking: their own repositories.
- Merging without a reviewer: every Dependabot pull request is merged by
  a maintainer.
- Manifest changes: a new major or a raised minimum is a step.
- The Kotlin bindings' Gradle dependencies and an npm side (none today).

## Tests

- `step_148_inout_floor`: every locked inout on the 0.2 line is at least
  0.2.6 (inout 0.1 stays locked beside it and is not affected); red
  against `main`'s lockfile (0.2.5).
- `.github/dependabot.yml` has no test of its own: GitHub reports an
  invalid file, and the first Dependabot run shows whether
  `lockfile-only`, `dependency-type: all` and one group work together
  for cargo — the documentation lists cargo for each option, not for the
  combination.
- Gate: the five (no wasm code touched). No smoke (no backend surface).
