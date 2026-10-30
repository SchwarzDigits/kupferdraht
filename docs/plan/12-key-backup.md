# Step 10 — Key backup (mnemonic)

> **Status:** pending — no backup endpoint in Wire API v14. See [`PLAN.md`](../../PLAN.md) §Status overview
> for the canonical merge state and dates.
>
> **Sub-steps:** (none — single-cycle step, scoping pending).
>
> **Historical names:**
> - "Step 9" — PLAN.md v1, pre-2026-04-21.
> - "Step 10" — PLAN.md v2 (2026-04-23) to 2026-05-05.
> - "Step 12" — 2026-05-05 onward, current. (After ADR-0012 introduced
>   server feature configs as Step 10 and ADR-0011 introduced asset
>   content as Step 11.)
>
> **Branch / commit prefix:** `step/10-*`, `feat(step-10): …`, `chore(step-10): …`.

---

## Step 10 — Key backup (mnemonic)

Encrypted offline backup of identity keys.

### Public API

```rust
impl Client {
    pub async fn backup(&self) -> Arc<Backup>;
}

pub struct Backup { /* ... */ }

impl Backup {
    pub fn generate_mnemonic() -> Vec<String>;
    pub async fn export(
        &self,
        mnemonic: Vec<String>,
    ) -> Result<BackupBlob, KupferdrahtError>;
    pub async fn restore(
        blob: BackupBlob,
        mnemonic: Vec<String>,
        server: ServerConfig,
    ) -> Result<LockedClient, KupferdrahtError>;
}

pub struct BackupBlob {
    pub data: Vec<u8>,
    pub version: u32,
}
```

### Wire API v14 endpoints

- `POST /login` / `POST /v14/clients` during restore (same
  as Step 1) — no new endpoints.

### Wire Message Types

None — backup content is identity keys, not messages.

### Implementation notes

- BIP39 via `bip39` crate. 12 words, 128-bit entropy.
- Mnemonic → AES-256-GCM key via `argon2` crate. Parameters:
  memory cost ≥ 64 MiB, time cost ≥ 3, several seconds of
  wall-clock derivation.
- Blob contents: ECDSA-P256 signing key (current), current
  KeyPackage private keys, ciphersuite tag, version.
- Restore runs a `login`-like flow, then injects restored
  keys before the first `Client::sync`.

### Dependencies

- Step 7 (DEK/KEK + `LockedClient` typing).

### CLI commands

**Step 10 — backup:**

| Command | Options |
|---|---|
| `backup-export` | `-o/--output` |
| `backup-restore` | `-i/--input`, `--api-url` |
| `mnemonic-generate` | — |


