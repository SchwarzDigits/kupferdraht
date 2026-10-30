# ADR-007: Use Tokio as the async runtime for the service crate

**Status:** Accepted
**Date:** 2026-03-15
**Deciders:** Component Owner, Senior Architect

---

## Context

The service crate handles incoming requests over HTTP and forwards them
to a downstream gRPC service. Both legs are I/O-bound and need
concurrent processing of many in-flight requests. Rust offers several
async runtimes, and the choice constrains every dependency in the
crate (HTTP server, gRPC client, instrumentation). Switching runtimes
later means rewriting most of the integration surface, so this is a
decision worth recording.

Constraints in effect:

- The component will run on Linux servers in production but must also
  build cleanly on macOS for developer machines.
- Latency targets are p99 < 50ms for the proxy path, which rules out
  runtimes with known scheduling-pause issues at our concurrency
  levels.
- The team has prior production experience with one of the candidates
  but not the others.

## Options

### Option A: Tokio

- **Pro:** Largest ecosystem; hyper, tonic, reqwest, sqlx, and most
  observability crates target Tokio first. Production-proven at
  scale by many shops. Team has prior experience.
- **Pro:** Multi-threaded scheduler available out of the box; can
  be tuned to single-threaded if needed.
- **Con:** Compile times are heavier due to the larger feature
  surface; full feature set adds ~30s to clean builds.
- **Con:** Some unsafe internals; the team treats this as acceptable
  given audit history.

### Option B: async-std

- **Pro:** API mirrors std more closely, mild learning-curve
  advantage for engineers new to async Rust.
- **Pro:** Lighter compile footprint than Tokio.
- **Con:** Smaller ecosystem; several dependencies we need (tonic
  for gRPC, sqlx for the database) require Tokio or compatibility
  shims.
- **Con:** Maintenance velocity has slowed in the last 18 months.

### Option C: smol / monoio / glommio

- **Pro:** Specialized runtimes; glommio in particular delivers
  excellent thread-per-core performance.
- **Pro:** Minimal compile footprint.
- **Con:** Niche ecosystems; we would build adapters for every
  external integration we need.
- **Con:** No team experience; learning cost is real and would slow
  initial delivery.

## Decision

**We adopt Tokio as the async runtime for this crate.**

The decisive factor is ecosystem alignment: our two required
integrations (tonic for gRPC, sqlx for the database) are Tokio-native,
and forcing them onto another runtime would create maintenance debt
greater than any Tokio downside. Prior team experience tips the
balance further; the documented multi-threaded scheduler tuning options
give us a clear path if latency tuning becomes necessary.

Compile-time cost is acknowledged and accepted; we mitigate it through
disciplined Tokio feature flags (`["macros", "rt-multi-thread", "net"]`
rather than `full`).

## Consequences

### Positive

- Direct use of tonic and sqlx without adapters.
- Mature observability story via `tracing-tokio` integration.
- Established patterns for graceful shutdown and signal handling.

### Negative

- Locked into Tokio's executor model; switching to a thread-per-core
  runtime later means a significant rewrite of any code that
  assumes work-stealing scheduling.
- Compile-time overhead must be managed via feature flags.

### Neutral

- Async-trait situation is no different across the candidates at the
  time of writing; native async-fn-in-trait is stable but ecosystem
  uptake is uneven.

## Implementation Notes

- Add `tokio = { version = "1", features = ["macros",
  "rt-multi-thread", "net", "signal"] }` to the service crate's
  `Cargo.toml`. Avoid the `full` feature set.
- Initial scheduler config: multi-threaded with `worker_threads`
  set to the number of physical cores. Reassess after first load
  tests.
- Graceful shutdown wired via `tokio::signal::ctrl_c()` plus
  `SIGTERM` handler; see step 014 for the implementation.
- Tracing setup uses `tracing-subscriber` with the `tokio` feature
  enabled; spans propagate across `.await` boundaries automatically.

## References

- [Tokio documentation](https://tokio.rs)
- ADR-003: Choose Rust as the language for this component (provided
  the constraint that we needed an async runtime decision at all)
- Step 014: Wire up graceful shutdown

## Revision History

- 2026-03-15: Created
- 2026-03-15: Status changed to Accepted after review with Senior
  Architect
