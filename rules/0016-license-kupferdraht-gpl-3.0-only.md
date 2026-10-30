# License Kupferdraht under GPL-3.0-only

Condensed from `docs/adrs/0016-license-kupferdraht-gpl-3.0-only.md`.

- **Kupferdraht's own license is `GPL-3.0-only`** — `LICENSE`, `Cargo.toml`
  `license`, README "License" state it; no `-or-later`, no dual arm.
- **Keep the dependency graph permissive-only** — the GPL posture must
  stay a free choice, never dependency-forced: no copyleft dependency
  enters the graph (`rules/0010` stays binding); the workspace crates
  are excluded from the license gates via `private = { ignore = true }`
  in `deny.toml` + `about.toml`.
- **External contributions come in under the Schwarz Digits Contributor
  License Agreement** (the program's CONTRIBUTING.md; the CLA Assistant
  asks on the first pull request) — keeps later license grants or a
  relicensing possible.
