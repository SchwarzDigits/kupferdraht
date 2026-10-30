# Kupferdraht implementation roadmap — per-step files

This directory holds one file per implementation step. The
canonical source of truth for **what is currently merged, in
progress, or pending** is the status table in
[`../../PLAN.md`](../../PLAN.md) at the repo root — the per-step
files here carry the *prose* (sub-steps, Wire endpoints, public
API, TDD lists, implementation notes), not the status.

## How to read

- Browse [`PLAN.md`](../../PLAN.md) first for the project-wide
  view: status table, cross-cutting commits, suggested
  implementation order, sizes, out-of-scope list.
- Click through to a specific step file when you need the
  detail: each file has its own H1 + metadata blockquote +
  prose body.

## Filename convention

`NN-short-slug.md`, where `NN` is the zero-padded step number
(`01`, not `1`). Sub-step suffixes use lowercase letters and
hyphens: `06e-lifecycle-leave-logout.md`, `06-2-timeline-...`
(Roman-numeral-style suffixes use `-2` for `-II` to keep URLs
clean).

## Renumbering

When a new step is inserted between existing ones, every later
step file is `git mv`'d to its new name and gains an entry in
its Historical Names metadata block. The git history preserves
the rename; old branch names and commit messages referencing
the previous number remain traceable through that block.

The full mechanism is documented in
[`../../PLAN.md`](../../PLAN.md) §Update protocol for this table.
