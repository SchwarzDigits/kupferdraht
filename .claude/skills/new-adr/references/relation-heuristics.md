# ADR Relation Heuristics

This document supports the semantic ADR analysis from Step 4 of the
`new-adr` skill. It defines the four relationship categories and shows
examples to calibrate the categorization against.

The analysis is **always a proposal to the user**, never an
authoritative classification. When in doubt, assign less rather than
more, phrase with reservation ("likely"), and ask the user.

## The four categories

### 1. Builds on

**Definition:** the new ADR presupposes a decision of the old ADR.
Without the old ADR the new ADR's question wouldn't be meaningful to ask
— the old ADR's existence is a precondition.

**Test question:** *"If the old ADR had decided differently, would my new
ADR exist at all?"* — answer: no, the question wouldn't have arisen.

**Examples:**

- ADR-003 "Choose Rust as primary language" → ADR-007 "Use Tokio as
  async runtime"
  Had ADR-003 chosen Go, Tokio would never be a topic — Go has its own
  scheduler.

- ADR-001 "Build a CLI tool for X" → ADR-005 "Use clap for argument
  parsing"
  Had ADR-001 decided to build the tool as a library instead of a CLI,
  the argument-parsing question would be irrelevant.

**Handling in the new ADR:** reference in `## References`. Optionally one
sentence in the Context paragraph: *"Building on the language choice from
ADR-003..."*

### 2. Constrained by

**Definition:** the old ADR narrows the new one's solution space. The
question would stand without the old ADR too, but the available answers
would be different.

**Test question:** *"If the old ADR had decided differently, would I have
weighed different options in the new ADR?"* — answer: yes.

**Examples:**

- ADR-002 "Adopt Apache-2.0 license for the workspace" constrains
  ADR-009 "Choose a logging library"
  A GPL-licensed logging library would have been ruled out as an option,
  because it would contaminate the Apache-2.0 workspace. The question
  "which logging library?" still stands; the available answers are
  restricted.

- ADR-004 "Target Android, iOS, and WASM via UniFFI" constrains
  ADR-011 "Choose an HTTP client"
  `reqwest` with `native-tls` would be out of the question, because the
  TLS-stack choice breaks the cross-compile targets; `rustls` would be
  the only UniFFI-compatible choice.

**Handling in the new ADR:** reference in `## References` and named
**explicitly in the Context** as a constraint. Constraints not in the
Context are invisible — the reviewer later wonders why an obvious option
wasn't considered.

### 3. Supersedes (potential conflict)

**Definition:** the new ADR decides the same topic differently from the
old one. If the old one has status `Accepted`, it is a supersession
candidate — the old one must be switched to `Superseded by ADR-NNN` so
the repo stays consistent.

**Test question:** *"If both ADRs are valid at the same time, is there a
contradictory instruction to the implementation?"* — answer: yes.

**Examples:**

- ADR-007 "Use Tokio as async runtime" → new ADR "Use glommio for
  thread-per-core performance"
  Direct contradiction: you can't have both at once without splitting the
  workspace. If the new ADR is accepted, the old one must be superseded.

- ADR-010 "Inline all FFI bindings under src/ffi/" → new ADR "Move FFI
  bindings to a separate workspace crate"
  Contradiction in the layout decision.

**Important distinction — no conflict when the scopes are disjoint:**

- ADR-007 "Use Tokio in the service crate" and new ADR "Use glommio in
  the data-pipeline crate" are **not** a conflict — both hold at the same
  time in different crates. The skill must not hastily propose
  supersession here; it clarifies with the user whether the scopes are
  truly disjoint.

**Handling in the new ADR:**
- An explicit sentence in the Context paragraph: *"This ADR supersedes
  ADR-XXX 'Title'. The rationale for changing course is..."*
- A reference to ADR-XXX in the References block.
- In the **old ADR**, set the status line to `Superseded by ADR-NNN` and
  append a revision-history entry. Change nothing else.

### 4. Related (FYI)

**Definition:** same domain or same layer, but no direct constraint
relationship. The ADR is worth mentioning to someone who understands the
topic, but it does not necessarily influence the new decision.

**Test question:** *"When I review this new ADR, would I want to look at
the old one for orientation, even though it isn't a constraint?"* —
answer: yes.

**Examples:**

- ADR-009 "Logging via tracing-subscriber" and new ADR "Metrics via
  prometheus exporter"
  Both observability layers, neither building on nor restricting the
  other. For someone who understands observability, the cross-link is
  helpful.

- ADR-013 "Use cargo-deny for dependency license control" and new ADR
  "Use cargo-about for license report generation"
  Both part of the license-compliance stack; cargo-deny and cargo-about
  are complementary tools, not building on each other.

**Handling in the new ADR:** reference in `## References` with a brief
context hint. Do not mention in the Context paragraph, or it dilutes the
real constraints.

## When NO categorization is justified

Not every existing ADR has to land in one of the four categories. If an
ADR addresses a completely unrelated topic, the skill stays silent about
it — better to mention no relationship than to produce relationship spam
that distracts the reviewer.

**Example of "no relationship":** an ADR about branch-naming convention
vs. a new ADR about crypto-library choice. Both are in the same
component, but they talk about completely different aspects.

## Heuristic risks and how to handle them

**Risk 1: false positive on builds-on.**
When the old and new ADR speak the same domain vocabulary, the temptation
to assume "builds on" is strong. Test: is it really true that the old ADR
is a precondition? If no → at most "Related".

**Risk 2: false negative on constrained-by.**
Constraints are often implicit and not in the old ADR's Decision section.
Read the Context and Consequences too; constraints sit there more often.

**Risk 3: false positive on supersession.**
The most common mistake. On suspected supersession, **always** clarify
whether the scopes are disjoint (see the distinction above). Ask the
user: "Does the old ADR still hold for its scope, or should it be
replaced entirely?"

**Risk 4: missing relations with many ADRs.**
With 50+ ADRs the analysis cannot be guaranteed complete. Tell the user
honestly: *"I checked N ADRs against the pitch, the most likely relations
above. Please add what I may have missed."*

## Output format of the analysis

Use this structure for the output so the user can scan it quickly:

```
Analysis against existing ADRs ({N} checked):

Builds on:
  - ADR-NNN "Title"
    Reason: {one sentence}

Constrained by:
  - ADR-NNN "Title"
    Constraint: {one sentence}

Potential supersession:
  - ADR-NNN "Title"
    Conflict: {one sentence}
    Should ADR-NNN be marked Superseded?

Related (FYI, no constraint):
  - ADR-NNN "Title"

Is this analysis correct? Please confirm, add, or correct.
```

Omit empty categories, don't show them as "(none)" — that only makes the
output longer.
