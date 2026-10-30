# Use openmls and proteus directly

Condensed from `docs/adrs/0001-use-openmls-and-proteus-directly.md`.

- **Build MLS on `openmls` and Pre-MLS on `proteus` directly** — the cryptographic foundation.
- **Do not depend on Wire's CoreCrypto** — its internal deps (`rust-pki`, `vergen_lib`) move on git branches and break third-party builds.
- **Do not re-implement MLS** — `openmls` is the audited RFC 9420 reference.
- **Implement the three Wire-specific MLS integration points yourself:** External Senders extension, Wire identity format, Required Capabilities extension.
