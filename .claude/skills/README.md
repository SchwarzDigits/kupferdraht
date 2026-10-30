# Skills

On-demand procedures Claude Code loads when a `description` matches the
task. The binding rules — including the full **Step Procedure** for
implementing a step — live in `CLAUDE.md` (always in context). Skills here
are only for deliberate, user-triggered acts that aren't part of every
step.

| Skill | What it does |
|---|---|
| `new-adr` | Distills a decision from the conversation into an ADR under `docs/adrs/` plus its same-named `rules/` file, in one commit. |

## Why the step procedure isn't a skill

It used to be (`step-implementation`). But an on-demand skill only governs
work when Claude Code actually loads it — in practice the step procedure
wasn't reliably followed, while the always-loaded charter was. So the
procedure now lives in `CLAUDE.md` "Step Procedure", the reliable
enforcement channel. `new-adr` stays a skill because writing an ADR is a
deliberate, user-triggered act, not a background procedure that must bind
every step.
