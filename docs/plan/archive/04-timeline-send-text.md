# Step 4 — Timeline: send text

> **Status:** merged — Step 4 (Timeline: send text) live 2026-04-25 (Kupferdraht Test 2, msg `cdd6e3d5-…`). Sub-steps: 4a `49acdc4` · 4b `aa8086c` · 4c `7b06098` · 4d `d04ad1b` · 4e `8ba8b5e` · 4f `393992e`. Archived from PLAN.md.
>
> **Sub-steps:** 4a, 4b, 4c, 4d, 4e, 4f.
>
> **Historical names:**
> - "Step 4" — current.
>
> **Branch / commit prefix:** `step/4-*`, `feat(step-4): …`, `chore(step-4): …`.

---

## Step 4 — Timeline: send text

Encrypt a plain text `GenericMessage` as an MLS application
message and POST to Wire.

### matrix-rust-sdk analog

`Room::send(content)` where `content` is a Matrix event
content type. Kupferdraht's `Room::send_text` is the
Wire/GenericMessage equivalent.

Source references:

- `crates/matrix-sdk/src/room/mod.rs::Room::send` — pattern
  for send surface.
- `crates/matrix-sdk-ui/src/timeline/send.rs` — timeline-send
  shape (adapted).

### Public API

```rust
impl Room {
    pub async fn timeline(self: Arc<Self>) -> Arc<Timeline>;
}

pub struct Timeline { /* ... */ }

impl Timeline {
    pub async fn send_text(
        &self,
        body: String,
    ) -> Result<MessageHandle, KupferdrahtError>;
}

pub struct MessageHandle {
    pub message_id: String,            // GenericMessage.message_id
    pub sent_at_ms: i64,
}
```

UDL:

```idl
interface Room {
    // ...

    [Async]
    Timeline timeline();
};

interface Timeline {
    [Async, Throws=KupferdrahtError]
    MessageHandle send_text(string body);
};

dictionary MessageHandle {
    string message_id;
    i64 sent_at_ms;
};
```

### Wire API v14 endpoints

- `POST /v14/mls/messages` — raw bytes, content-type
  `message/mls`. Body is TLS-serialized
  `MlsMessage(PrivateMessage)` produced by
  `MlsGroup::create_message` (§6).

### RFC 9420 sections

- §6.3 — PrivateMessage structure.
- §15 — Application messages.
- §9.4.2 — Handshake-message secrets (background reference;
  openmls handles this).

### Wire Message Types

**`GenericMessage` Protobuf is mandatory here** (CLAUDE.md).
Payload construction:

```rust
GenericMessage {
    message_id: Uuid::new_v4().to_string(),
    content: Some(generic_message::Content::Text(Text {
        content: body.to_string(),
    })),
}
```

Serialized via `prost::Message::encode`, then passed as the
`message` argument to `MlsGroup::create_message`. The MLS
layer encrypts it.

Oneof variants in scope for Step 4: `Text`. All other
variants (Calling, Asset, Reaction, Ephemeral, …) remain
out-of-scope — see CLAUDE.md "Wire Message Types" and
Functional Scope out-of-scope list.

### Store additions

- `timeline_events` table — `message_id`, `room_id`,
  `sender_device_id`, `sender_user_id`, `body_ct`,
  `kind` (text/system), `sent_at_ms`, `delivery_status`.

Step 4 writes outgoing text rows with `kind='text'`,
`delivery_status='submitted'`. Step 5 extends with incoming
rows and upgrades submitted → acknowledged where applicable.

### Implementation notes

- Requires Step 3 for any room Kupferdraht wasn't a creator of.
  `MlsGroup::load` fails cleanly if the group isn't locally
  known.
- The encryption step is synchronous in openmls; the HTTP
  upload is async. `MlsClient` held briefly for encryption,
  dropped before the await on upload (LEARNINGS §3.1).

### TDD starter tests

- `build_text_generic_message(body)` yields a UUID
  `message_id` and a round-trippable protobuf.
- `Timeline::send_text` fails with `MlsOperation` when
  invoked on a room whose group state is absent.
- Ciphertext emitted by `encrypt_message` starts with
  MlsMessage framing for `PrivateMessage` (wire_format `0x0002`).
- On success, `timeline_events` gains one row with
  `delivery_status='submitted'`.

### CLI command

```
kupferdraht-cli send --id <room-id> -m "body"
```

### Smoke test

```bash
kupferdraht-cli send --id <room-id> -m "Hello from Kupferdraht CLI"
# on the counterpart's iOS/desktop: the text arrives in the chat
```

### CLI commands

**Step 4 — send:**

| Command | Options |
|---|---|
| `send` | `--id`, `-m/--message` |
