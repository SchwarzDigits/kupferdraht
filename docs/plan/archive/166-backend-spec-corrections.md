# Step 166 — The backend specification corrected and ahead of the code

**Status: MERGED 2026-11-25** — commit `176a9f2` (PR #127, merge `88aedd0`); no smoke
(docs); doc-pin tests green; A-11 and A-14 decided like Wire the same day; the Rust
server's coordinator closes its decision 24. Retrospective: LEARNINGS 5.109.

**Commissioned 2026-11-25** (Volker). Docs only, no change of behaviour; the
first of the steps cut from the coordinator's review of Kupferdraht 0.6.0 against the
Rust server and Wire 5.38.9 (its decision 24 and the galley plan's §8 list the
contradictions). It goes first so that Steps 167–174 start from a spec that is right.

## Problem

`docs/backend-spec.md` contradicts the code and Wire at known points, and it still
describes states the code has left: the 1:1 type is `1` in §9.5 and `2` in §9.1 (Wire
uses `2`); §10.2 demands that a commit as `PrivateMessage` be refused while W-2 allows
it (Wire refuses); `mls-missing-group-info` is missing from the label catalogue (§3.3)
although §9.8 and §10.4 name it; several `404` and `403` answers carry no label; the
payloads of `conversation.create` / `.rename` / `.member-join` / `.member-leave` are
missing; `PUT …/access` and the join links are described as deferred while the SDK
uses them (Steps 117, 122, 163c); `qualified_users` is required where the SDK sends
none; A-11, A-13 (`403 not-connected`), A-14 and A-33 (a member removed through the
route) are open; P-9 says the backend sends the second-factor code with a refused
login, while Wire sends it only on `POST /verification-code/send`.

## Scope

1. **Phase 1:** every correction read against the code and Wire's behaviour as the
   observations record it; where an observation is missing, `docs/wire/api-notes.md`
   gets it first (a reading, not a guess).
2. **Corrections in `docs/backend-spec.md`:** §9.5 type `2`; §10.2 and W-2 agree
   (commits are `PublicMessage`, a `PrivateMessage` commit is refused); §10.5 as the
   code does it; `mls-missing-group-info` in §3.3; a label on every `404` / `403`;
   the four event payloads; the access and join-link routes as implemented (§9.4);
   `qualified_users` optional and empty on MLS creation; A-11, A-13, A-14, A-33; P-9
   after Wire's flow.
3. **The target state of Steps 167–174**, each as a short normative paragraph that
   names the step implementing it: `supported_protocols = ["mls"]` (167), the
   personal room by `GET /conversations/mls-self` (168), API v14 and v18 negotiated
   (169), the second-factor flow (170), e2ei as a non-goal with the visible notice and
   cipher suite 2 with its notice (171), ephemeral content of every kind, the link
   preview passed through, `Composite` / `Multipart` shown in part (172), key material
   renewed (173), conversation settings (174).
4. **Docs:** `docs/backend-spec.md`; `docs/wire/api-notes.md` where a reading was
   missing; `CHANGELOG.md` (Changed, docs). No ADR: the decisions behind 167–174 are
   taken in those steps.

## Out of scope

- Any change of code or behaviour; the sections of scope 3 describe what the named
  steps will do, nothing they do today.
- ADR-0008's revision (Step 169 owns it).

## TDD sketch

None — a docs step. The gate is `cargo fmt --check` locally and CI; the existing doc
pins (`step_138_read_state::doc_homes_name_the_read_state` and its siblings) stay
green.

Smoke: none (docs). After the merge the coordinator of the Rust server closes its
decision 24.
