# ADR-0016: License Kupferdraht under GPL-3.0-only

**Status:** Accepted
**Date:** 2026-07-23
**Deciders:** Volker Zöpfel

## Context

ADR-0010 decoupled Kupferdraht from the three Wire-origin GPL crates and
explicitly deferred the strategic license decision for Kupferdraht itself
to "a separate ADR with full information". Since then the technical
precondition has held: the dependency graph is all-permissive,
enforced by `cargo deny check` against `deny.toml`'s allow-list —
every strategic option was open.

With the 2026-07 documentation overhaul the deferral came due:
README, `LICENSE`, and `Cargo.toml` must state one consistent
license, so the decision could no longer be postponed.

## Options Considered

### Option A: Keep Apache-2.0

Leave `LICENSE`, `Cargo.toml`, and the README on Apache-2.0.

**Pros:**
- Maximally embeddable: mobile SDKs and proprietary products can
  consume Kupferdraht without obligations.
- No breaking change for downstream consumers.

**Cons:**
- Permissive terms leave nothing to enforce — legally too weak a
  handle against closed redistribution of the Kupferdraht's own work.
- Does not match the project's intended copyleft posture.

### Option B: GPL-3.0-only

**Pros:**
- Strong, widely understood copyleft; large compatible ecosystem.
- The simplest single-license posture — one text, one obligation
  model.
- Definitively no license friction with the Wire ecosystem, whose
  own clients are GPL-3.0.

**Cons:**
- No EU-tailored terms; single legally binding language.
- No network/SaaS clause (that would be AGPL) — hosted use triggers
  no obligations.

### Option C: EUPL-1.2 only

**Pros:**
- EU copyleft, legally binding in 23 official EU languages.
- Copyleft extends to providing the work as a network service.

**Cons:**
- Less widely known; concern that the EUPL might be held
  inapplicable in some context, with no safety net.

### Option D: EUPL-1.2 OR GPL-3.0-only (dual)

EUPL-1.2 as the primary license, GPL-3.0 as the fallback should the
EUPL-1.2 not apply.

**Pros:**
- Copyleft with an explicit safety net for EUPL-inapplicable
  contexts.

**Cons:**
- SPDX `OR` formally means recipient's choice, not a conditional
  fallback — the effective obligations are the milder of the two.
- Two license texts and a prose fallback explanation: more
  complexity than the posture is worth.

## Decision

**Option B: `GPL-3.0-only`.** Apache-2.0 is off the table — a
permissive license is legally too weak to enforce openness of the
Kupferdraht's own work. Against the EUPL variants, plain GPL-3.0 wins on
simplicity, and it rules out any conceivable license friction with
the Wire ecosystem.

The GPL posture is a **free strategic choice, not a dependency-forced
one** — and must stay that way: ADR-0010's dependency rules remain
fully binding. The dependency graph stays permissive-only, so Kupferdraht
remains the only copyleft component in the combined work and the
license could be revisited without dependency work.

## Consequences

### Positive

- Copyleft protects the Kupferdraht's own work from closed redistribution.
- Resolves the license deferral of ADR-0010 — README, `LICENSE`, and
  `Cargo.toml` are consistent again.
- Because the dependency graph stays permissive-only, no dependency
  work would be needed to relicense. Any relicensing requires the
  consent of all rights holders of Kupferdraht's code.

### Negative

- **BREAKING for downstream consumers:** embedders now inherit
  copyleft obligations from Kupferdraht itself (CHANGELOG entry).
- Proprietary embedding requires a separate license grant from all
  rights holders. Inbound contributions come in under the Schwarz
  Digits Contributor License Agreement (the open-source program's
  CONTRIBUTING.md), settled 2026-10-08.
- No network/SaaS copyleft: GPL-3.0 imposes nothing on hosted use.

### Neutral

- The workspace crates are `publish = false`; `deny.toml` and
  `about.toml` exclude them via `private = { ignore = true }` so the
  license gates remain pure dependency policy.
- Wire's Terms of Use constraints (README "Known limitations") are
  orthogonal to this decision.

## Implementation Notes

Shipped with this decision: `LICENSE` = GPL-3.0 text,
`Cargo.toml` `license = "GPL-3.0-only"`,
`private = { ignore = true }` in `deny.toml` + `about.toml` (plus
removal of the stale per-crate GPL exceptions in `about.toml`),
README "License" section, CHANGELOG `[Unreleased]` BREAKING entry.

## References

- Builds on: ADR-0010 "Decouple Kupferdraht from GPL dependency stack" —
  resolves its deferred license decision; its dependency rules stay
  binding and are re-affirmed here.
- Schwarz Digits CONTRIBUTING.md — the Contributor License Agreement that
  governs inbound contributions (settled 2026-10-08; formerly open.md §65).

## Revision History

- 2026-07-23: Created
- 2026-09-25: Amendment, not a supersession — wording on rights
  holding neutralised (no statement on who holds the rights);
  relicensing and separate grants now refer to "all rights holders".
  Why: who holds the rights is a fact outside this ADR's remit and
  changes with every accepted contribution, so naming a sole holder
  would have forced a later contradiction. The decision (GPL-3.0-only)
  is untouched.
- 2026-10-07: Amendment, not a supersession — "clean-room work" becomes
  "Kupferdraht's own work" (three places). Why: a clean-room claim
  denies that Wire source was consulted at all; ADR-0009 permits
  consulting it for protocol observation, and the docs record that it
  was. The provenance the GPL protects is independence, not isolation.
  The decision is untouched.
- 2026-10-08: Amendment, not a supersession — the inbound contribution
  policy is the Schwarz Digits Contributor License Agreement; open.md §65
  resolved. The decision is untouched.
