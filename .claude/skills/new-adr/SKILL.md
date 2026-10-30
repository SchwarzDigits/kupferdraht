---
name: new-adr
description: >
  Distills an ADR decision from the ongoing conversation into a conformant
  Architecture Decision Record per Blueprint spec §4.4. Reads the
  conversation to extract options, arguments, decision, and concerns.
  Delivers a full ADR draft for discussion (not section by section). Then
  analyzes existing ADRs for relationships (builds-on, constrained-by,
  supersedes, related). Finds the next number, writes the file at
  `docs/adrs/NNN-slug.md`, updates the index, on supersession updates the
  old ADR (status + revision history), and offers a git commit. Use this
  skill WHENEVER someone says: "we should capture this in an ADR", "let's
  write this up as an ADR", "record as an ADR", "new ADR", "let's capture
  this as an ADR", "we should record this decision", or a comparable
  phrasing signalling: the discussion is ripe to write down.
---

# new-adr Skill

Turns a conversation decision into an ADR. Expects the decision context to
have been worked out in the conversation already — Claude reads the
transcript, builds the draft, you edit.

Since the ADR/rules split, the ADR is the rationale layer; the **binding
short form lives in `rules/`** (`rules/README.md`). Every new or changed
ADR pulls its condensed rule along in the **same commit** — see Step 9.

## Preconditions

Check before any action:

1. `docs/adrs/` exists.
2. `docs/adrs/_template.md` exists (the template).
3. `docs/adrs/README.md` exists (the index).
4. `rules/` exists (the condensed rule layer; `rules/README.md`).

If (1) or (2) is missing: **stop** and tell the user the repo was not
instantiated from the blueprint — no ADR setup, so do the setup first.

If only (3) is missing: create the index file with the standard header
(see "Index format" below), then continue.

## Workflow

### Step 1 — Extract the conversation context

Read the conversation so far and extract from what was discussed:

- **What it is about** (title candidate, imperative form)
- **Which options were discussed** (at least 2, with pros and cons as they
  came up in the conversation — rejected options stay in as options too)
- **Which decision was made** (or will likely be made, if not yet stated
  explicitly)
- **Which rationale carries** the decision
- **Which concerns or trade-offs** were mentioned — these become Negative
  Consequences later
- **What was taken as given** — candidates for Context

**If the conversation context is thin** (e.g. the user only said "let's do
an ADR about X" with no prior discussion): **do not guess**. Tell the user
honestly: *"I find too little concrete substance in the conversation for a
full draft. Can we discuss the topic briefly — which options are you
weighing, what are the trade-offs? Once we've gone through that, I'll write
the draft."*

### Step 2 — Deliver a full draft

Present a complete ADR draft as a Markdown block, with all mandatory
sections from spec §4.4 filled in. For format and section order, see
`references/example-adr.md`.

**What Claude does when filling in:**

- **Title:** imperative form, kebab-case-able (the slug is derived from it
  later).
- **Status:** `Proposed` (default; may change during iteration).
- **Date:** today (`date -I`).
- **Deciders:** recognizable from the conversation context or `{{TBD}}` —
  clarified during iteration.
- **Context:** two to three paragraphs. Describes situation, problem, known
  constraints. **Do not pre-empt the decision here.**
- **Options:** all options discussed in the conversation, including
  rejected ones. Pro and con per option, distilled from the conversation.
- **Decision:** the choice made or foreseeable, with rationale.
- **Consequences:** split into Positive, Negative, Neutral. **Important:**
  Negative Consequences must reflect the trade-offs mentioned in the
  conversation — including the uncomfortable ones. If the conversation
  mentioned no negatives, mark them explicitly in the draft as
  `{{Unclear — not raised in the conversation}}` rather than leaving them
  empty or inventing them.
- **Implementation Notes:** if the conversation contained concrete
  implementation hints, put them here. Otherwise omit.
- **References:** empty in the first draft — filled in Step 4 from the
  semantic analysis.
- **Revision History:** `- YYYY-MM-DD: Created`.

**After the draft:** present it to the user with an explicit invitation to
discuss:

> "Here is my draft. Three spots where I'm unsure or couldn't clearly
> derive from the conversation:
> 1. {spot 1, e.g. 'Negative Consequences — we only touched the
>    compile-time trade-off briefly; I added it as a negative, is that
>    right?'}
> 2. {spot 2}
> 3. {spot 3}
>
> What should we adjust?"

If there are no uncertain spots, omit this — no artificial enumeration.

### Step 3 — Iteration

The user edits. Possible reactions and how to handle them:

- **"All good, continue":** go to Step 4.
- **"Change X":** apply the change, show the updated draft, ask if it fits
  now.
- **"Option C is missing, we touched on it too":** add the option, show the
  draft.
- **"The rationale must be different, Y was really the main reason":**
  rewrite the Decision, show the draft.
- **"Status should be Accepted":** change the status.

**Important:** no endless iteration loop. If after three iterations the
draft still doesn't fit, tell the user: *"We've reworked the draft three
times now. Maybe the decision itself isn't ripe for an ADR yet. Should we
go back to the discussion briefly, or am I systematically misunderstanding
something?"*

### Step 4 — Semantic ADR analysis

**If fewer than 2 ADRs exist:** skip this step.

Otherwise: analyze all existing ADRs and propose relationships to the
draft.

**Procedure:**

1. Read `docs/adrs/README.md` — the index table of all ADRs.
2. Read the full text of all ADRs with status `Accepted` or `Proposed`.
   (Ignore Superseded and Deprecated.)
3. For each ADR read, extract the `## Decision` and `## Context` sections.
4. Against the draft's Decision, check each of the four categories
   (definitions and examples in `references/relation-heuristics.md`):

   - **Builds on** — the draft presupposes a decision of the old one.
   - **Constrained by** — the old ADR narrows the draft's options.
   - **Supersedes (potential conflict)** — the draft decides the same topic
     differently from the old one.
   - **Related** — same domain, same layer, no direct constraint
     relationship.

5. List the result to the user, categorized (format in
   `references/relation-heuristics.md`).

6. **Wait for user confirmation.** Accept corrections.

**The analysis is a proposal, not truth:**
- With little Decision clarity in an old ADR, categorize cautiously.
- With many ADRs (50+), explicitly say "I checked N ADRs, please add what I
  may have missed".
- Never supersede without explicit confirmation.

7. **Update the draft's References section** with the confirmed relations.
8. **If "Builds on" or "Constrained by" were confirmed:** suggest to the
   user mentioning it in the Context paragraph ("Constraints from prior
   decisions: ADR-NNN ..."), if not already there. Apply if they agree.

### Step 5 — Determine the next number

```bash
ls docs/adrs/[0-9][0-9][0-9]-*.md 2>/dev/null | \
  sed -E 's|.*/([0-9]{3})-.*|\1|' | sort -n | tail -1
```

If empty: `001`. Otherwise: highest number + 1, three-digit formatted.

**Safety check:** if the next number surprisingly already exists, stop and
ask the user.

**Hard cap:** number >= 1000 → error.

### Step 6 — Write the file

Path: `docs/adrs/NNN-{slug}.md` where `{slug}` is the kebab-case title,
trimmed to max 50 characters, without leading stop words like "the", "a",
"to".

Content: the iterated draft enriched with References.

**Language:** the ADR content is **always English** (hard rule: English
for committed content). If the conversation and thus the draft were in
German, translate when writing.

**On supersession:** the `Context` field gets a first paragraph that names
the supersession explicitly: *"This ADR supersedes ADR-XXX '{Title of
ADR-XXX}'. The rationale for changing course is …"*

### Step 7 — Update the index

`docs/adrs/README.md` contains a table. Append a new row **at the end of
the table**:

```markdown
| NNN | {Title} | {Status} | YYYY-MM-DD | [→](NNN-{slug}.md) |
```

If the table does not exist yet (freshly created index), create it with
the standard header (see "Index format").

### Step 8 — Update the superseded ADR

**Only if Step 4 produced a confirmed supersession candidate.**

Open the old ADR's file and change **only**:

1. **Status line** from `**Status:** Accepted` to
   `**Status:** Superseded by ADR-NNN`.
2. **Revision History**: append a new entry:
   `- YYYY-MM-DD: Status changed to Superseded by ADR-NNN ({Title of new ADR})`.

**Everything else stays untouched.** Decision, Consequences, Options,
Context — do not change. That is the spec-§4.4 exception.

In `docs/adrs/README.md` update the status column of the old row: from
`Accepted` to `Superseded by ADR-NNN`.

### Step 9 — Write/update the `rules/` file

**If `rules/` does not exist:** skip this step and tell the user.

The ADR is the rationale layer; the **binding short form lives in `rules/`**
as a **same-named sibling file** to the ADR (convention: `rules/README.md`).
`rules/` is a 1:1 mirror of `docs/adrs/`. Both change together — an ADR
without a rule file is a defect (CLAUDE.md freshness rule).

1. **Create the sibling file.** `rules/` gets a file with the **same
   filename as the ADR**. Header line: `Condensed from
   docs/adrs/<filename>.` — the only source reference, **no per-line tag**.
2. **Distill the rule(s).** From the `## Decision` (and possibly
   `## Implementation Notes`) draw one to a few **Do/Don't sentences**, one
   or two lines each. No rationale, no context — those stay in the ADR.
3. **On supersession.** Reduce the **superseded** ADR's rule file to a
   pointer ("Superseded by ADR-NNN; current rule: `<new-filename>`") plus a
   `Don't` guardrail where reverting is a risk. The current rule lives in
   the new file.

The changed `rules/` files belong in the **same commit** as the ADR
(Step 11).

### Step 10 — Show confirmation

After writing, show the user:
- Created file: `docs/adrs/NNN-{slug}.md`
- Changed file: `docs/adrs/README.md`
- On supersession: additionally changed file `docs/adrs/XXX-{old-slug}.md`
- New/changed rule file: `rules/<same-name-as-ADR>.md`
- Brief content overview (title + decision sentence)
- List of the ADR relations as taken into References

### Step 11 — Offer a commit

Ask explicitly:

If **no supersession**:
> "Shall I commit the ADR + rule now? Suggested commit message:
> `docs(adr): add ADR-NNN {short-title} + rule`"

If **with supersession**:
> "Shall I commit the ADR + rule in one commit now?
> Suggested commit message:
> `docs(adr): add ADR-NNN {short-title} + rule, supersede ADR-XXX`"

**Only on confirmation:**
```bash
git add docs/adrs/NNN-{slug}.md docs/adrs/README.md
git add rules/NNN-{slug}.md                      # the same-named rule file (Step 9)
# On supersession, additionally:
git add docs/adrs/XXX-{old-slug}.md rules/XXX-{old-slug}.md
git commit -m "docs(adr): add ADR-NNN {short-title} + rule[, supersede ADR-XXX]"
```

`{short-title}` is the first ~6 words of the title, lowercase except proper
nouns.

If the user declines or doesn't clearly agree: **no commit.** Leave the
stage and file state as is.

## Index format

If `docs/adrs/README.md` has to be created, use this header:

```markdown
# Architecture Decision Records

Index of decisions for this component. Each ADR has its own file.

**Status lifecycle:** Proposed → Accepted → optionally
Superseded by ADR-NNN or Deprecated. Accepted ADRs are immutable —
edit means write a new ADR that supersedes the old one. The only
permitted edits to an accepted ADR are the Status line and the
Revision History block.

See `_template.md` for the format and Blueprint spec §4.4 for the
authoritative rules.

| Nr | Title | Status | Date | Link |
|----|-------|--------|------|------|
```

## What this skill does NOT do

- **Make no decision for you.** The skill distills what was decided in the
  conversation; it does not recommend.
- **Invent no options** that did not come up in the conversation. If the
  skill thinks a weighed option is missing, it can mark that as an uncertain
  spot (see Step 2) — but it does not write it into the draft.
- **No content change to superseded ADRs.** Only the status line and the
  revision history.
- **No multiple supersession** in one run. If several old ADRs are
  superseded, handle only the primary one in the skill and point to the
  others at the end.
- **No push.** Local commit yes, push no.
- **Don't skip the `rules/` derivation** — an ADR without a condensed rule
  in `rules/` is a defect (freshness rule).

## Heuristic references

- `references/relation-heuristics.md` — definitions and examples for the
  four relationship categories from Step 4.
- `references/example-adr.md` — full example ADR for style and depth.
- `rules/README.md` — the convention of the condensed rule layer (Step 9):
  one same-named file per ADR, supersession pointer.
