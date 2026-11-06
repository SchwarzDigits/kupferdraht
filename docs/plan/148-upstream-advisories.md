# Step 148 — Upstream advisories that neither RustSec nor Dependabot carries

**Status: commissioned 2026-11-06** from Step 142's retrospective
(LEARNINGS §5.80): Volker chose a CI check as a step, and watches
openmls/openmls's security alerts on GitHub in the meantime
(outside the repository). After Step 130b, before the v0.2.1 release —
the release procedure runs the check before it tags.

## Problem

1. **Two scanners, one gap.** `cargo deny` matches the RustSec database;
   Dependabot matches GitHub's global advisory database, which takes a
   repository advisory only once GitHub has reviewed it. openmls's last
   four repository advisories are in neither (measured 2026-11-06,
   `gh api /advisories/<id>` → 404): GHSA-w62v-gv48-63rh (fixed in 0.9.0)
   and the three Step 142 took up — GHSA-gc79-23g3-8g52 (high,
   published 2026-10-08), GHSA-94v4-rg25-x625 and GHSA-p276-g3f3-wwgh.
   v0.2.0 was tagged on 2026-11-03 with the affected openmls 0.9.0 and a
   green `cargo deny`.
2. **The source is readable.** `GET
   /repos/{owner}/{repo}/security-advisories?state=published` lists a
   repository's published advisories with `vulnerabilities[]` —
   `package.ecosystem` (`rust`), `package.name`,
   `vulnerable_version_range`, `patched_versions` — for any token.
   Published advisories on 2026-11-06: openmls/openmls 7, rustls/rustls
   6, RustCrypto/elliptic-curves 3, RustCrypto/AEADs 2, cryspen/hpke-rs
   1; rusqlite, refinery, SQLite3MultipleCiphers, reqwest,
   tokio-tungstenite and uniffi-rs none. The reviewed ones (rustls,
   hpke-rs, RustCrypto, openmls's GHSA-8x3w) reach Dependabot too; the
   check is for the ones that do not, and costs nothing for the rest.
3. **Advisories are seen only on a PR.** `ci.yml`'s `cargo deny` job runs
   on `pull_request` and `push` to `main`, without a schedule: an
   advisory published between two PRs is reported by the next PR, not
   when it appears. (The compliance workflow's weekly Trivy scan reads
   the same two databases.)

## Scope

1. **The check.** For a list of upstream GitHub repositories: read the
   published repository advisories, and report every one with a
   `vulnerabilities[]` entry whose `rust` package is a crate in
   `Cargo.lock` at a version inside `vulnerable_version_range`. A crate
   from a SchwarzDigits fork counts by its version like any other: the
   forks keep upstream's version numbers (ADR-0042), so upstream's
   advisory applies until an ignore entry (3.) says the fork's tag
   carries the fix. Any hit fails the check, naming the advisory, its
   severity, the crate, the locked version and the patched versions.
2. **The list of repositories** — phase 1, with Volker. Candidates: the
   repositories above that publish advisories, plus the crates that hold
   key material, decode peer or backend data, or run the storage
   (openmls, the RustCrypto crates, hpke-rs, rustls, rusqlite, refinery,
   SQLite3MultipleCiphers, sqlite-remote-vfs, prost, tokio-tungstenite).
   The alternative — every GitHub repository a `Cargo.lock` package
   names as `repository` — is complete but costs one request per
   repository per run; phase 1 counts them against the token's rate
   limit. Recommendation: the curated list in a file of its own, each
   entry with its crates.
3. **Ignores with a reason**, like `deny.toml`'s `[advisories] ignore`:
   advisory id, reason (the fork's tag carries the fix, the code path is
   unreachable). An ignore entry that matches no hit fails the check, so
   a stale entry cannot hide the next advisory. Where the list and the
   ignores live (a small TOML beside `deny.toml`): phase 1.
4. **CI.** A job in `ci.yml` on `pull_request` and `push` like
   `cargo deny`, and a daily `schedule` for both, so an advisory turns
   `main` red the day it is published (GitHub mails a failed scheduled
   run to the repository's watchers). The workflow's `GITHUB_TOKEN`
   reads public repositories' advisories; phase 1 confirms it in a run.
5. **Locally and at release.** One command (`gh` authenticated, network)
   that a step can run and that the release procedure in `CHANGELOG.md`
   runs before it tags. Whether it joins the five-command gate of
   CLAUDE.md is a charter question: it needs the network and a token,
   which the five do not. Recommendation: release procedure yes, per-step
   gate no (CI runs it on every PR).
6. **Implementation** — phase 1. GitHub's ranges (`>= 0.9.0, < 0.9.2`,
   `< 0.8.3`, `= 0.9.0`, several `vulnerabilities[]` per advisory for
   several release lines) need a real SemVer comparison, which shell and
   `jq` do not give. Options: a small unpublished workspace crate
   (`tools/advisories`, `semver` + `toml` + `serde_json`; HTTP through
   `gh api` or reqwest), whose matching the five-command gate unit-tests;
   or a Python script in `.github/scripts/`, which would bring a second
   language into the repository with tests outside the gate.
   Recommendation: the workspace crate.
7. **Docs:** the release procedure in `CHANGELOG.md` (a step before the
   tag), CLAUDE.md's gate section if phase 1 decides so (with Volker),
   `README.md` where it describes the checks CI runs. A decision on
   sources and the gate → ADR via `new-adr` with its `rules/` file.

## Out of scope

- Submitting openmls's advisories to RustSec (`rustsec/advisory-db`):
  offered on 2026-11-06, not chosen — openmls's to do.
- Sources outside GitHub: SQLite's own CVEs (the SQLite3MC amalgamation
  embeds SQLite; NVD, not GHSA), mailing lists, Wire's advisories.
- Bumping a dependency automatically: a hit fails the build, the fix is a
  step.
- The browser bundle's npm side (`bindings/web/`): it ships no npm
  dependencies today; a later one brings its own scanner.

## TDD sketch

No network in tests: the advisories come from fixture JSON (openmls's
seven, recorded 2026-11-06), `Cargo.lock` from fixture files.

- Range matching, known vectors: `>= 0.9.0, < 0.9.2` holds 0.9.0 and
  0.9.1, not 0.9.2 or 0.8.9; `< 0.8.3`; `= 0.9.0`; an advisory with one
  entry per release line (openmls's 0.8 and 0.9) hits on either line.
- `Cargo.lock`: crates.io and git sources alike; two versions of one
  crate are both checked; a crate not in the lockfile never hits.
- Step 141's lockfile (openmls 0.9.0) against the fixture → GHSA-gc79,
  GHSA-94v4 and GHSA-p276; today's (0.9.2) → none.
- An ignore entry suppresses its hit and is listed as ignored; an ignore
  entry without a hit fails; an advisory without a `rust` package is
  skipped.
- The report names id, severity, crate, version and patched versions,
  and nothing from the token.
- Gate: the five; the CI job green on the PR. No smoke (no backend
  surface): the first scheduled run on `main` is the live check.
