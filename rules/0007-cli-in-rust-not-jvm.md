# Test CLI in Rust, not the JVM

Condensed from `docs/adrs/0007-cli-in-rust-not-jvm.md`.

- **Implement the test CLI as a Rust binary crate (`kupferdraht-cli`) depending on `kupferdraht-core` directly, with `clap` v4.**
- **Do not build a JVM/Clikt CLI through the generated bindings** — it can only test what is already exposed, inverting the test order.
- **Keep the CLI the canonical first consumer of every new API** — a feature lands with its CLI command in the same PR, before bindings.
- **`#[tokio::main]` only at the binary entry point; one subcommand per file.**
- Natrium's own Kotlin/Clikt CLI is unaffected and serves as the second, cross-language consumer; do not duplicate it.
