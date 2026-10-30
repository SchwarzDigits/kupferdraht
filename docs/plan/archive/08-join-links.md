# Step 8 — Join links

> **Status:** merged — 8a–8c merged 2026-05-04; smoke 2026-05-04 (access + guest-link lifecycle). Redeem (`join-via-link`) pending (needs 2nd account). Archived from PLAN.md.
>
> **Sub-steps:** 8 (issue/fetch/revoke/redeem), 8b (Room::access modes/roles), 8c (POST /code 201-newly-created followup-GET).
>
> **Historical names:**
> - "Step 8" — current.
>
> **Branch / commit prefix:** `step/8-*`, `feat(step-8): …`, `chore(step-8): …`.

---

## Step 8 — Join links

Wire's guest-room-link mechanism. Issue, revoke, and redeem
a code/key pair.

### matrix-rust-sdk analog

No direct equivalent — Matrix's invite model is different.

### Public API

```rust
impl Room {
    pub async fn create_join_link(
        &self,
        password: Option<String>,
    ) -> Result<JoinLink, KupferdrahtError>;

    pub async fn revoke_join_link(&self) -> Result<(), KupferdrahtError>;
    pub async fn current_join_link(&self) -> Result<Option<JoinLink>, KupferdrahtError>;
}

impl Client {
    pub async fn join_via_link(
        &self,
        code: String,
        key: String,
        domain: String,
        password: Option<String>,
    ) -> Result<Arc<Room>, KupferdrahtError>;
}

pub struct JoinLink {
    pub url: String,
    pub password_protected: bool,
}
```

### Wire API v14 endpoints

- `POST /v14/conversations/{id}/code` — create.
- `DELETE /v14/conversations/{id}/code` — revoke.
- `GET /v14/conversations/{id}/code` — fetch current.
- `POST /v14/conversations/join` — redeem → delivers a
  `groupinfo` the client then `external-commit`-joins.

### RFC 9420 sections

- §12.5 — External Commits (shared code path with Step 3).

### Wire Message Types

None — join-link flow is MLS + Wire-specific, no
GenericMessage content.

### Dependencies

- Step 3 (external-commit machinery).
- Can land independently of Steps 4, 5, 7.

### CLI commands

**Step 8 — join links:**

| Command | Options |
|---|---|
| `join-link-generate` | `--id`, `-p/--password` |
| `join-link-revoke` | `--id` |
| `join-link-current` | `--id` |
| `join-via-link` | `--code`, `--key`, `--domain`, `-p/--password` |
