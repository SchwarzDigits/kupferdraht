# Step 11 — Dehydrated device (Web login / recovery)

> **Status: DROPPED 2026-09-25** (Volker: no longer needed). Never started; v14 exposed no DD endpoints (observations §63). Kept as the record of the original idea. Previously: pending — gated on Wire DD-endpoint clarification. See [`PLAN.md`](../../PLAN.md) §Status overview
> for the canonical merge state and dates.
>
> **Sub-steps:** (none — single-cycle step, scoping pending).
>
> - 
>
> **Historical names:**
> - "Step 9" — PLAN.md v1, pre-2026-04-21.
> - "Step 10" — PLAN.md v2 (2026-04-23) to 2026-05-05.
> - "Step 12" — 2026-05-05 onward, current. (After ADR-0012 introduced
>   server feature configs as Step 10 and ADR-0011 introduced asset
>   content as Step 11.)
>
> **Branch / commit prefix:** `step/11-*`, `feat(step-11): …`, `chore(step-11): …`.

---

## Step 11 — Dehydrated device (Web login / recovery)

Server-hosted MLS-client state that a new browser or device
can rehydrate into.

### Public API

```rust
impl Client {
    pub async fn dehydrated_device(&self) -> Arc<DehydratedDevice>;
}

pub struct DehydratedDevice { /* ... */ }

impl DehydratedDevice {
    pub async fn status(&self) -> Result<DdStatus, KupferdrahtError>;
    pub async fn enable(&self, passphrase: String) -> Result<(), KupferdrahtError>;
    pub async fn disable(&self) -> Result<(), KupferdrahtError>;
    pub async fn rehydrate(
        user_id: String,
        passphrase: String,
        server: ServerConfig,
    ) -> Result<Arc<Client>, KupferdrahtError>;
}

pub struct DdStatus {
    pub enabled: bool,
    pub team_policy: DdPolicy,
    pub last_sync_ms: Option<i64>,
}

pub enum DdPolicy { Disabled, Optional, Required }
```

### Wire API v14 endpoints

TBD — Wire's DD support is relatively recent. Inventory
during Step 11 planning via `reference/wire.swagger.json`.
Expect enable/disable/rehydrate endpoints + a team-policy
readout.

### RFC 9420 sections

- §12.5 — External Commits (DD registers itself as a
  distinct leaf in every group).

### Wire Message Types

- Inherits all from Steps 4 + 5 — DD processes the same
  `GenericMessage` stream a local device would.

### Dependencies

- Step 7 (KEK/DEK for the DD state blob).
- Step 5 (event-stream processing — DD runs this same
  pipeline server-side).

### CLI commands

**Step 11 — dehydrated device:**

| Command | Options |
|---|---|
| `dd-status` | — |
| `dd-enable` | `--passphrase-stdin` |
| `dd-disable` | — |
| `dd-rehydrate` | `-u/--user`, `-b/--backend` |

