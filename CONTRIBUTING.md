# Contributing

Kupferdraht is part of the Schwarz Digits open-source program, and the
program's guidelines apply: Code of Conduct, Contributor License
Agreement, pull-request workflow and security reporting are described
in <https://github.com/SchwarzDigits/.github/blob/main/CONTRIBUTING.md>.
The CLA Assistant asks for the agreement on your first pull request; one
signature covers every Schwarz Digits open-source project.

## What this project adds

- **The charter.** [`CLAUDE.md`](CLAUDE.md) is binding for every change:
  architecture invariants, coding conventions and the twelve-phase step
  procedure. Read it first. [`rules/`](rules/) is its condensed form, one
  file per decision; the reasoning is in [`docs/adrs/`](docs/adrs/).
- **Independent implementation.** Wire source code (Kalium or any other
  Wire repository) is never a model for structure, naming or
  implementation. It may be consulted only to observe protocol behaviour
  where the Swagger is ambiguous, and every observation is recorded with
  its source in `docs/wire/api-notes.md` or `docs/observations.md`.
- **The gate.** Every commit passes the five checks listed in the README
  under "Building": `cargo fmt --check`, a warning-free build,
  `clippy -D warnings`, `cargo test --no-fail-fast` and `cargo deny check`.
  A change under `kupferdraht-core/src/wasm/` or `bindings/web/` adds the
  wasm32 checks.
- **Branches and commits.** One branch per step (`step/NN-slug`),
  Conventional-Commits messages (`type(scope): subject`), library and CLI
  land in one commit.
- **Documentation moves with the code.** The doc homes are listed in
  CLAUDE.md "README and Documentation"; an out-of-date doc is a defect.
- **Review pace.** Issues and pull requests are welcome, with no promise
  of timely review; Kupferdraht is experimental software without a
  support commitment.
