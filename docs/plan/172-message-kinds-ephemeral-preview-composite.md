# Step 172 — Message kinds: ephemeral everything, the link preview passed through, `Composite` and `Multipart` shown in part

**Status: commissioned 2026-11-25** (Volker), in three sub-steps, each its own branch,
commit and PR. After Step 171.

- **172a — ephemeral content of every kind:** `Asset`, `Knock` and `Location` sent and
  received inside `Ephemeral` like `Text` today, with the timer and the deletion as
  Wire does them. The precondition of Step 174's room timer.
- **172b — the link preview passed through:** `Text.link_preview` (field 3) declared
  in the proto and carried unchanged through an edit, a quote and a forward, as NG-4
  promises; Kupferdraht generates no preview (no proxy), NG-4 stays. Proto and docs
  agree afterwards.
- **172c — `Composite` and `Multipart` in part:** on receive, at least the text part is
  shown with the note "content not fully supported" instead of the message being
  dropped; never sent. Edits stay as they are (text with mentions and a quote; Wire
  does not edit assets either).

## Problem

`Ephemeral` wraps `Text` alone (`encode_ephemeral_text_generic_message`, Step 89), so a
room timer could not cover a file, a ping or a location. `link_preview` is not in the
proto, so an edit or a quote of a message that carried one drops it, against NG-4's
word. `Composite` (20) and `Multipart` (27) decode to the tolerated-unknown path and
vanish from the timeline although both carry text.

## Scope

1. **172a:** the `Ephemeral` wrapper over `Asset`, `Knock` and `Location`
   (`wire/proto.rs`, `timeline/disappearing.rs`, `sync/dispatch/disappearing.rs`);
   the send paths take a TTL like `send_text` does; the receiver-side expiry
   (self-redaction, `MessageDelete`, Step 89) covers every kind; the CLI's `send-file`,
   `ping` and `location` take `--ttl`.
2. **172b:** `LinkPreview` declared in the proto as Wire declares it (the fields
   transcribed, dates in the header); carried in the timeline row (a column or a JSON
   field — phase 1 decides) and emitted again on edit, quote and forward; never built.
3. **172c:** `Composite` and `Multipart` declared in the proto as far as the text part
   goes; decoded into a text item with a `partial_content` mark the embedder can show;
   `Multipart`'s attachments are not fetched.
4. **Docs:** `docs/wire/message-types.md` (rows and sub-structure notes), the proto
   header, `docs/library.md`, `docs/bindings.md`, `docs/cli.md`,
   `docs/non-goals.md` NG-4 (unchanged in substance, the pointer to 172b),
   `docs/ARCHITECTURE.md` §7 if a column is added (a migration), `CHANGELOG.md`.

## Out of scope

- Generating a link preview; sending `Composite` or `Multipart`; `ButtonAction`;
  editing anything but text.

## TDD sketch

- 172a: proto round trips of `Ephemeral(Asset | Knock | Location)`; a `send-file --ttl`
  produces an `Ephemeral`; an inbound ephemeral asset self-redacts at expiry and the
  cached file goes (Step 84); Wire's own `Ephemeral(Knock)` decodes — red;
- 172b: a `Text` with `link_preview` round-trips; an edit of it carries the same
  preview bytes; a quote of it too — red;
- 172c: a `Composite` with text and buttons becomes a text item marked partial; a
  `Multipart` likewise; neither is dropped — red.

Smoke with the webapp on wire-dev: a self-deleting image in both directions; a message
with a link preview from the webapp keeps its preview after an edit in Kupferdraht; a
poll from the webapp shows its question in Kupferdraht.
