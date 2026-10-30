// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 95d — a group's leaves as the device verification surface sees
//! them: the W-4 identity (or why there is none), whether the credential
//! is Basic, and the MLS thumbprint of the leaf's signature key.

use openmls::group::GroupId;
use openmls::prelude::CredentialType;

use crate::encryption::leaf_identity::{LeafIdentity, LeafIdentityError, parse_leaf_identity};
use crate::encryption::mls_client::MlsClient;
use crate::encryption::thumbprint::mls_thumbprint;
use crate::error::KupferdrahtError;

/// One leaf of an MLS group.
#[derive(Debug, Clone)]
pub(crate) struct LeafDevice {
    /// The W-4 identity, or why the leaf has none (X.509 / malformed).
    pub(crate) identity: Result<LeafIdentity, LeafIdentityError>,
    /// `true` for a `BasicCredential` leaf.
    pub(crate) basic: bool,
    /// RFC 7638 thumbprint of the leaf's signature key.
    pub(crate) thumbprint: Option<String>,
}

impl MlsClient {
    /// Every leaf of the group `group_id_bytes`, this device included.
    ///
    /// # Errors
    /// * `KupferdrahtError::Storage` if loading the group fails.
    /// * `KupferdrahtError::InvalidInput` if no group with that id is stored.
    pub fn group_devices(
        &self,
        group_id_bytes: &[u8],
    ) -> Result<Vec<LeafDevice>, KupferdrahtError> {
        debug_assert!(
            !group_id_bytes.is_empty(),
            "group_id_bytes must be non-empty"
        );
        let group_id = GroupId::from_slice(group_id_bytes);
        let group = self.load_group_snapshot(&group_id)?.ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "group_devices: no MLS group with that id is persisted locally".into(),
            )
        })?;
        Ok(group
            .members()
            .map(|member| LeafDevice {
                identity: parse_leaf_identity(&member.credential),
                basic: member.credential.credential_type() == CredentialType::Basic,
                thumbprint: mls_thumbprint(&member.signature_key),
            })
            .collect())
    }

    /// The MLS thumbprint of this device's own signature key.
    #[must_use]
    pub fn own_thumbprint(&self) -> Option<String> {
        mls_thumbprint(self.credential_with_key().signature_key.as_slice())
    }
}
