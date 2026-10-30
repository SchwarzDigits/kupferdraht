# ADR-0007: Implement the test CLI in Rust, not on the JVM

**Status:** Accepted
**Date:** 2026-04-21
**Deciders:** Volker Zöpfel

---

## Context

Kupferdraht is implemented in Rust (see ADR-0006). A separate question is the implementation language for the CLI test harness — the binary that exercises each SDK feature against a real staging backend and runs smoke tests during development.

Natrium, the Kotlin Multiplatform library that wraps Kupferdraht, has its own CLI implemented on the JVM using the Clikt framework. Every feature added to Natrium gets a corresponding Clikt command. The pattern works well for Natrium because Natrium is a JVM library.

For Kupferdraht, the obvious alternative to a Rust CLI would be "copy Natrium's approach": a JVM + Clikt CLI that consumes Kupferdraht through the generated Kotlin bindings. Team members already know Clikt. The development patterns are established. It looks like a small decision — just tooling.

The reason to think harder about it is the ordering between CLI, SDK implementation, and UniFFI bindings. The CLI exists to validate the SDK before the UniFFI bindings are released to consumers. If the CLI requires the bindings to exist, the ordering becomes: write SDK code → generate bindings → write CLI → test. If the CLI calls the SDK directly, the ordering becomes: write SDK code → write CLI → test → generate bindings. The second ordering catches bugs earlier and makes the CLI the canonical first consumer of each new feature.

---

## Options Considered

### Option A: Rust CLI calling the Kupferdraht crate directly

A separate binary crate `kupferdraht-cli` in the Kupferdraht workspace, depending on `kupferdraht-core` as a path dependency. Built with `clap` v4 (the Rust analog to Clikt). Runs as a native binary on any developer machine.

- **Pro:** Exercises the Rust API directly, without going through UniFFI. A feature can be tested the same day it is implemented, before bindings are generated.
- **Pro:** The CLI becomes the canonical first consumer of every new API, ensuring that the API is usable from outside the crate before being exposed to downstream languages.
- **Pro:** Single-language toolchain for the core team. No JVM dependency for running tests.
- **Pro:** Fast iteration — `cargo run` rebuilds and executes in seconds; JVM startup adds tens of seconds per invocation.
- **Con:** Developers working primarily on the Natrium wrapper need to be comfortable reading Rust and Clap-style CLI code to extend the Kupferdraht CLI.
- **Con:** Diverges from Natrium's Clikt-based CLI structure. Two CLI codebases with different conventions coexist.

### Option B: JVM CLI using Clikt, consuming Kupferdraht via generated Kotlin bindings

Mirror the Natrium pattern exactly. The CLI is Kotlin + Clikt, running on the JVM, calling Kupferdraht through the UniFFI-generated Kotlin bindings.

- **Pro:** Consistent with Natrium. The same team members can maintain both CLIs with the same idioms.
- **Con:** CLI can only test what the bindings expose. Any feature requires bindings to exist before CLI testing is possible, inverting the intended test order.
- **Con:** Slower iteration due to JVM startup and the binding-generation step between SDK changes and CLI usage.
- **Con:** Bugs in the UniFFI bindings themselves (name mangling, type conversion, async handling) cannot be distinguished from SDK bugs at CLI time — the CLI sees only the bound surface.
- **Con:** Dependency on Natrium's build infrastructure (Gradle, JDK) for what should be a local tooling decision in Kupferdraht.

### Option C: Maintain both CLIs

One Rust CLI for the Kupferdraht-internal test loop, one JVM CLI for Natrium-level testing.

- **Pro:** Covers both testing levels.
- **Con:** Two CLIs to maintain. Feature coverage has to be kept in sync manually. When the two diverge, it is not clear which is authoritative.
- **Con:** No additional test coverage over having only the Rust CLI, because Natrium itself serves as the second consumer of the UniFFI bindings and exercises them in production.

---

## Decision

**We implement the CLI in Rust (Option A).**

The ordering argument is decisive. The whole purpose of the CLI is to validate features before the UniFFI bindings are released. A CLI that depends on the bindings cannot fulfill that purpose — it tests what has already been exposed, not what is currently being built.

The second consumer (Natrium, once a binding release is cut) implicitly tests the same API surface from the JVM side. This gives us the benefit of cross-language validation without maintaining two CLI codebases.

The team-familiarity cost (Kotlin developers working on the Kupferdraht CLI) is acceptable. The CLI commands follow straightforward Clap patterns, one command per file, with the same conventions Natrium already documents (long + short options, session restore at start, structured exit codes). A Kotlin developer reading Rust Clap code should have little difficulty.

---

## Consequences

### Positive

- CLI commands can land in the same PR as the SDK feature they exercise. No multi-PR dance to wait for bindings.
- Every SDK feature is usable from outside the crate before being exposed to downstream consumers, catching API-design issues early.
- Fast iteration — `cargo run` completes in seconds.
- Single-language toolchain for the core development loop.

### Negative

- Kotlin-oriented contributors need basic Rust fluency to extend the CLI. This is a real but bounded cost.
- Natrium-team muscle memory (Clikt idioms, JVM workflow) does not transfer directly to the Kupferdraht CLI.

### Neutral

- Natrium's existing Clikt CLI remains in place and is not affected by this decision. Natrium is a JVM library and its CLI should be on the JVM.
- Downstream cross-language validation still happens — not through a duplicate CLI, but through Natrium itself being the second consumer of the UniFFI bindings.

---

## Implementation Notes

- Workspace layout: `kupferdraht-core/` (library crate) and `kupferdraht-cli/` (binary crate), with `kupferdraht-cli` depending on `kupferdraht-core` as a path dependency.
- CLI framework: `clap` v4 with the derive API. Each subcommand is its own file under `commands/`, one `#[derive(clap::Args)]` struct per command, registered in a root `#[derive(clap::Subcommand)]` enum.
- Async handling: `#[tokio::main]` at the binary entry point only. No `block_on` anywhere else in the CLI.
- Conventions are documented in the CLI section of `PLAN.md` (long + short option forms, session restore on every command, `KUPFERDRAHT_HOME` for state directory override, English-only output, exit codes, logging via `tracing`).

Full command inventory and Natrium-to-Kupferdraht command mapping are in `PLAN.md`.

---

## References

- ADR-0006 — Rust as Kupferdraht's implementation language; this ADR extends that choice to the test harness
- `PLAN.md` — CLI section, including command inventory and conventions
- natrium's `CLAUDE.md` (private repository, not part of this one) — Natrium's CLI conventions that Kupferdraht partially mirrors

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR |
| 1.1 | 2026-10-07 | References: the natrium pointer no longer names a submodule path — the private consumer is not part of this repository. No content change. |
