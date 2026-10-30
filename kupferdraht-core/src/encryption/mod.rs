// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

pub(crate) mod asset_crypto;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod atomicity_tests;
mod external_commit;
mod external_removal;
mod group_create;
mod group_devices;
mod group_invite;
mod group_kick;
mod group_resync;
mod group_state;
mod leaf_identity;
mod messages;
mod mls_client;
mod mls_lock;
pub(crate) mod prekeys;
mod provider;
#[cfg(test)]
pub(crate) mod test_support;
mod thumbprint;
mod welcome;

// Re-export only the symbols other kupferdraht-core modules actually use.
// `MLS_CIPHERSUITE` and `MLS_SIGNATURE_SCHEME` remain `pub` inside
// `mls_client` — they are not leaked past the encryption boundary.
pub(crate) use external_removal::ExternalRemovalOutcome;
pub(crate) use group_devices::LeafDevice;
pub(crate) use group_state::GroupLeaf;
pub(crate) use leaf_identity::{LeafIdentity, LeafIdentityError};
pub(crate) use messages::DecodedMessage;
/// Wire's pinned MLS ciphersuite (`0x0002` — P-256 / AES-128-GCM /
/// SHA-256), exposed for the `claim_key_packages` query parameter.
/// Reaches across the encryption boundary because the wire transport
/// must spell out the same numeric `ciphersuite=2` as the local MLS
/// stack uses.
pub(crate) use mls_client::MLS_CIPHERSUITE as MLS_CIPHERSUITE_FOR_CLAIM;
pub use mls_client::{INITIAL_KEY_PACKAGE_COUNT, MLS_PUBLIC_KEY_TYPE, MlsClient};
pub(crate) use mls_lock::lock_mls;
pub use provider::KupferdrahtProvider;
