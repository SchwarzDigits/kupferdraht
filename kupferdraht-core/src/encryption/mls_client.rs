// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 1c — MLS identity (P-256 + `BasicCredential` + `[Basic, X509]` caps).
//!
//! `MlsClient` owns the MLS signing keypair and the credential payload
//! used for every `KeyPackage` and group operation. Step 1c.1 covers
//! identity creation; Step 1c.2 adds `KeyPackage` batch generation
//! (TLS-serialized, ready for `POST /v14/mls/key-packages/self/{client_id}`
//! in 1c.3). Upload + `/v14/mls/public-keys` fetch + persistent storage
//! land in 1c.3–1c.5.
//!
//! ## Why P-256 (LEARNINGS §1.2)
//! Wire v14 deployments pin ciphersuite `0x0002` so iOS clients
//! can back the signing key with Secure Enclave (only supports NIST
//! curves). Kupferdraht must match — ciphersuite mismatches surface as
//! `LeafNodeValidation::UnsupportedCredentials` deep in openmls.
//!
//! ## Why `[Basic, X509]` capabilities (LEARNINGS §1.3)
//! Wire's e2ei-enrolled groups embed a `RequiredCapabilities` extension
//! that lists `X509` as a supported credential type. `KeyPackage`s whose
//! leaves only declare `Basic` are rejected as `UnsupportedCredentials`
//! when claimed into such groups. Declaring both lets Kupferdraht coexist
//! with e2ei peers without itself enrolling for an X.509 cert.

use base64::Engine as _;
use openmls::prelude::{
    BasicCredential, Capabilities, Ciphersuite, CredentialType, CredentialWithKey, KeyPackage,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_traits::OpenMlsProvider;
use openmls_traits::types::SignatureScheme;
use tls_codec::Serialize as _;

use crate::encryption::leaf_identity::wire_identity;
use crate::encryption::provider::{KupferdrahtProvider, TxProvider};
use crate::error::KupferdrahtError;

/// Initial `KeyPackage` batch size uploaded at registration time
/// (PLAN.md Step 1c.2). Matches Wire's expectation that a freshly-
/// registered client publishes enough pool to serve near-term claims
/// without an early refill round.
pub const INITIAL_KEY_PACKAGE_COUNT: usize = 100;

/// The ciphersuite Wire v14 deployments pin. P-256 / AES-128-GCM / SHA-256.
pub const MLS_CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;

/// Signature scheme paired with `MLS_CIPHERSUITE`.
pub const MLS_SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;

/// Wire's `mls_public_keys` JSON key for the signature algorithm above.
pub const MLS_PUBLIC_KEY_TYPE: &str = "ecdsa_secp256r1_sha256";

/// Capability set declared on every `KeyPackage`'s leaf node and on
/// every group Kupferdraht creates / joins. Both `Basic` and `X509`
/// credential types must be present so e2ei-enrolled peers accept us.
pub(crate) fn leaf_capabilities() -> Capabilities {
    Capabilities::new(
        None,
        None,
        None,
        None,
        Some(&[CredentialType::Basic, CredentialType::X509]),
    )
}

/// MLS-side state for a single Wire device.
///
/// Holds:
/// - the long-lived ECDSA-P256 signing keypair (also persisted in
///   the openmls storage table the moment the client is constructed);
/// - the `BasicCredential` carrying Wire's
///   `{uuid}:{client_id}@{domain}` identity;
/// - the [`KupferdrahtProvider`] composing `RustCrypto` (crypto + rand) with
///   `SqliteStorageProvider<JsonCodec, Connection>` — `KeyPackage`
///   private halves and future per-group epoch state are written
///   through the provider automatically.
///
/// The connection inside the provider can be file-backed or in-memory;
/// in-memory is the default for tests via `MlsClient::new_fresh`.
pub struct MlsClient {
    signature_keys: SignatureKeyPair,
    credential_with_key: CredentialWithKey,
    provider: KupferdrahtProvider,
}

impl MlsClient {
    /// Create a fresh MLS identity with an **in-memory** provider. Used
    /// by unit tests across the `encryption` modules that don't need
    /// persistence across a process restart.
    ///
    /// # Errors
    /// Same as [`Self::new_fresh_with_storage`].
    #[cfg(test)]
    pub fn new_fresh(user_id: &str, client_id: &str) -> Result<Self, KupferdrahtError> {
        Self::new_fresh_with_storage(KupferdrahtProvider::open_in_memory()?, user_id, client_id)
    }

    /// Create a fresh MLS identity backed by the supplied
    /// [`KupferdrahtProvider`]. The signature keypair is written to the
    /// provider's storage so [`Self::resume_with_storage`] can reload
    /// it on the next process start (PLAN.md Step 1c.6).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `user_id` is not in the
    ///   `<uuid>@<domain>` shape.
    /// * `KupferdrahtError::Generic` if ECDSA key generation fails.
    /// * `KupferdrahtError::Storage` if persisting the signature keypair
    ///   fails (only possible with a file-backed provider).
    pub fn new_fresh_with_storage(
        provider: KupferdrahtProvider,
        user_id: &str,
        client_id: &str,
    ) -> Result<Self, KupferdrahtError> {
        debug_assert!(!user_id.is_empty(), "user_id must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        let identity = wire_identity(user_id, client_id)?;
        let signature_keys = SignatureKeyPair::new(MLS_SIGNATURE_SCHEME)
            .map_err(|e| KupferdrahtError::Generic(format!("MLS SignatureKeyPair: {e:?}")))?;
        provider.with_write_tx(|p| {
            signature_keys
                .store(p.storage())
                .map_err(|e| KupferdrahtError::Storage(format!("store signature keypair: {e}")))
        })?;
        let credential_with_key = CredentialWithKey {
            credential: BasicCredential::new(identity).into(),
            signature_key: signature_keys.to_public_vec().into(),
        };
        Ok(Self {
            signature_keys,
            credential_with_key,
            provider,
        })
    }

    /// Reload an MLS identity from a previously-persisted storage
    /// provider (PLAN.md Step 1c.6 resume path). Looks up the signature
    /// keypair by `(public_key, MLS_SIGNATURE_SCHEME)`.
    ///
    /// Exercised only by the internal test module today; will be wired
    /// into `ClientBuilder` in the dedicated re-login-idempotency
    /// cycle before first message send.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `user_id` is not `<uuid>@<domain>`
    ///   or the signature key is empty.
    /// * `KupferdrahtError::Storage` if the signature keypair is not in the
    ///   storage — a corrupted or wrong-user database.
    #[allow(dead_code)]
    pub fn resume_with_storage(
        provider: KupferdrahtProvider,
        user_id: &str,
        client_id: &str,
        signature_public_key: &[u8],
    ) -> Result<Self, KupferdrahtError> {
        debug_assert!(!user_id.is_empty(), "user_id must be non-empty");
        debug_assert!(!client_id.is_empty(), "client_id must be non-empty");
        debug_assert!(
            !signature_public_key.is_empty(),
            "signature_public_key must be non-empty"
        );
        let identity = wire_identity(user_id, client_id)?;
        let signature_keys = provider.with_read_tx(|p| {
            SignatureKeyPair::read(p.storage(), signature_public_key, MLS_SIGNATURE_SCHEME)
                .ok_or_else(|| {
                    KupferdrahtError::Storage(
                        "resume: signature keypair not found in openmls storage".into(),
                    )
                })
        })?;
        let credential_with_key = CredentialWithKey {
            credential: BasicCredential::new(identity).into(),
            signature_key: signature_keys.to_public_vec().into(),
        };
        Ok(Self {
            signature_keys,
            credential_with_key,
            provider,
        })
    }

    /// Base64-encoded P-256 signature public key (the value Wire
    /// expects as `mls_public_keys.ecdsa_secp256r1_sha256` in
    /// `POST /v14/clients` and `PUT /v14/clients/{id}`).
    #[must_use]
    pub fn signature_public_key_b64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.signature_keys.public())
    }

    /// Raw P-256 signature public key (uncompressed point or DER, per
    /// the openmls `BasicCredential` convention).
    #[must_use]
    pub fn signature_public_key(&self) -> &[u8] {
        self.signature_keys.public()
    }

    /// Read access to the credential bundle for `KeyPackage` and
    /// group operations. Returned reference is enough for openmls's
    /// builder APIs.
    #[must_use]
    pub(crate) fn credential_with_key(&self) -> &CredentialWithKey {
        &self.credential_with_key
    }

    /// Read access to the P-256 signature keypair. Needed by sibling
    /// submodules (e.g. [`super::external_commit`]) that drive openmls
    /// builders; kept `pub(crate)` so the private half never leaks past
    /// the encryption boundary.
    #[must_use]
    pub(crate) fn signature_keys(&self) -> &SignatureKeyPair {
        &self.signature_keys
    }

    /// The provider that runs each MLS operation in its own transaction
    /// (Step 97c) — sibling submodules wrap every operation in
    /// `with_write_tx` / `with_read_tx` and hand openmls the
    /// [`TxProvider`] it passes in.
    #[must_use]
    pub(crate) fn provider(&self) -> &KupferdrahtProvider {
        &self.provider
    }

    /// Close `mls.db` (Step 114: a purge deletes it next): the provider is
    /// swapped for an empty in-memory one, so later MLS operations find no
    /// group instead of touching the file.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the stand-in cannot be opened or
    /// `mls.db` not closed.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn close_storage(&mut self) -> Result<(), KupferdrahtError> {
        let open = std::mem::replace(&mut self.provider, KupferdrahtProvider::open_in_memory()?);
        open.close()
    }

    /// Open a notification window (Step 97e, ADR-0032; per page since
    /// Step 109, ADR-0037): the inbound operations of a drained page join
    /// it; it commits only after the page's data is persisted. See
    /// [`KupferdrahtProvider::begin_window`].
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the write lock stays busy.
    pub(crate) fn begin_notification_window(&self) -> Result<(), KupferdrahtError> {
        self.provider.begin_window()
    }

    /// Commit the open notification window.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` when the commit fails (rolled back then).
    pub(crate) fn commit_notification_window(&self) -> Result<(), KupferdrahtError> {
        self.provider.commit_window()
    }

    /// Mark the start of a page's notification `index` inside the open
    /// window (Step 109, ADR-0037).
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    pub(crate) fn savepoint_notification(&self, index: u32) -> Result<(), KupferdrahtError> {
        self.provider.savepoint(index)
    }

    /// Undo the window's MLS changes since notification `index` began.
    ///
    /// # Errors
    /// `KupferdrahtError::Storage` on `SQLite` failure.
    pub(crate) fn rollback_to_notification(&self, index: u32) -> Result<(), KupferdrahtError> {
        self.provider.rollback_to_savepoint(index)
    }

    /// Roll the open notification window back.
    pub(crate) fn rollback_notification_window(&self) {
        self.provider.rollback_window();
    }

    /// Generate a batch of `count` fresh `KeyPackage`s and TLS-serialize
    /// each so it is ready to be base64-wrapped into the JSON body of
    /// `POST /v14/mls/key-packages/self/{client_id}` (PLAN.md Step 1c.3).
    ///
    /// Every `KeyPackage`:
    /// * uses [`MLS_CIPHERSUITE`] (P-256) matching Wire v14 deployments;
    /// * is signed with [`MLS_SIGNATURE_SCHEME`] via the owned
    ///   [`SignatureKeyPair`];
    /// * declares `[Basic, X509]` in its leaf capabilities (see
    ///   [`leaf_capabilities`]);
    /// * is a regular `KeyPackage` — no last-resort extension, that is
    ///   a separate pool.
    ///
    /// The private halves are written to `mls.db` as a side effect —
    /// the whole batch in one transaction (Step 97c).
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if `count == 0`.
    /// * `KupferdrahtError::Generic` if openmls rejects the build call.
    /// * `KupferdrahtError::Encoding` if TLS serialization fails.
    pub fn generate_key_packages(&self, count: usize) -> Result<Vec<Vec<u8>>, KupferdrahtError> {
        if count == 0 {
            return Err(KupferdrahtError::InvalidInput(
                "generate_key_packages: count must be non-zero".into(),
            ));
        }
        self.provider.with_write_tx(|provider| {
            let mut serialized = Vec::with_capacity(count);
            for _ in 0..count {
                serialized.push(self.build_one_key_package(provider)?);
            }
            debug_assert_eq!(serialized.len(), count);
            Ok(serialized)
        })
    }

    fn build_one_key_package(
        &self,
        provider: &TxProvider<'_>,
    ) -> Result<Vec<u8>, KupferdrahtError> {
        let bundle = KeyPackage::builder()
            .leaf_node_capabilities(leaf_capabilities())
            .build(
                MLS_CIPHERSUITE,
                provider,
                &self.signature_keys,
                self.credential_with_key.clone(),
            )
            .map_err(|e| KupferdrahtError::Generic(format!("KeyPackage build: {e:?}")))?;
        bundle
            .key_package()
            .tls_serialize_detached()
            .map_err(|e| KupferdrahtError::Encoding(format!("TLS serialize KeyPackage: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmls::prelude::Credential;

    #[test]
    fn new_fresh_uses_p256_signature_scheme() {
        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("new_fresh");
        // SignatureKeyPair carries its scheme as a constructor parameter;
        // the only way to verify it externally is to feed the public key
        // back through the same scheme. As a smoke test, assert the key
        // is non-empty (P-256 uncompressed point is 65 bytes; we don't
        // pin the exact length because openmls may switch encodings).
        assert!(!client.signature_public_key().is_empty());
        // P-256 uncompressed: 0x04 || X(32) || Y(32) = 65 bytes.
        // openmls 0.8 uses raw uncompressed points for ECDSA_SECP256R1_SHA256.
        assert_eq!(client.signature_public_key().len(), 65);
    }

    #[test]
    fn new_fresh_credential_carries_wire_identity() {
        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("new_fresh");
        let credential: &Credential = &client.credential_with_key().credential;
        let identity = credential.serialized_content();
        let expected = b"a11ce000-0000-4000-8000-000000000001:a11ce00000000001@wire.com";
        assert_eq!(identity, expected);
    }

    #[test]
    fn new_fresh_rejects_user_id_without_at_domain() {
        let err = MlsClient::new_fresh("not-a-qualified-user-id", "client")
            .err()
            .expect("expected an error from a non-qualified user_id");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn signature_public_key_b64_decodes_to_raw_key() {
        let client = MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("new_fresh");
        let raw = client.signature_public_key().to_vec();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(client.signature_public_key_b64())
            .expect("base64 decode");
        assert_eq!(raw, decoded);
    }

    #[test]
    fn leaf_capabilities_declare_basic_and_x509() {
        let caps = leaf_capabilities();
        let credentials = caps.credentials();
        assert!(credentials.contains(&CredentialType::Basic));
        assert!(credentials.contains(&CredentialType::X509));
    }

    // ---- Step 1c.2 failing tests (`KeyPackage` batch generation) ----

    use openmls::prelude::{KeyPackageIn, ProtocolVersion};
    use tls_codec::DeserializeBytes;

    fn fresh_client() -> MlsClient {
        MlsClient::new_fresh(
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
        )
        .expect("new_fresh")
    }

    fn deserialize_key_package(bytes: &[u8]) -> KeyPackageIn {
        let (kp_in, rest) = KeyPackageIn::tls_deserialize_bytes(bytes)
            .expect("TLS-serialized KeyPackage must round-trip through KeyPackageIn");
        assert!(
            rest.is_empty(),
            "TLS-serialized KeyPackage must be self-delimiting — no trailing bytes"
        );
        kp_in
    }

    #[test]
    fn initial_key_package_count_is_one_hundred() {
        // PLAN.md Step 1c.2 names 100 as the initial batch size.
        assert_eq!(INITIAL_KEY_PACKAGE_COUNT, 100);
    }

    #[test]
    fn generate_key_packages_returns_requested_count() {
        let client = fresh_client();
        let batch = client.generate_key_packages(5).expect("generate");
        assert_eq!(batch.len(), 5);
    }

    #[test]
    fn generate_key_packages_rejects_zero_count() {
        let client = fresh_client();
        let err = client
            .generate_key_packages(0)
            .expect_err("zero count must be rejected");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn generated_key_packages_round_trip_through_tls_codec() {
        let client = fresh_client();
        let batch = client.generate_key_packages(3).expect("generate");
        for bytes in &batch {
            let _kp_in = deserialize_key_package(bytes);
        }
    }

    #[test]
    fn generated_key_packages_are_unique() {
        let client = fresh_client();
        let batch = client.generate_key_packages(5).expect("generate");
        let unique: std::collections::HashSet<&Vec<u8>> = batch.iter().collect();
        assert_eq!(
            unique.len(),
            batch.len(),
            "each KeyPackage must have unique init/encryption keys — no byte-duplicates"
        );
    }

    #[test]
    fn generated_key_packages_carry_wire_identity() {
        let client = fresh_client();
        let batch = client.generate_key_packages(1).expect("generate");
        let kp_in = deserialize_key_package(&batch[0]);
        let credential = kp_in.unverified_credential();
        let identity = credential.credential.serialized_content();
        assert_eq!(
            identity, b"a11ce000-0000-4000-8000-000000000001:a11ce00000000001@wire.com",
            "KeyPackage leaf must carry Wire's `<uuid>:<client_id>@<domain>` identity"
        );
    }

    #[test]
    fn generated_key_packages_validate_under_wire_ciphersuite() {
        let client = fresh_client();
        let batch = client.generate_key_packages(1).expect("generate");
        let kp_in = deserialize_key_package(&batch[0]);
        let verifier = openmls_rust_crypto::RustCrypto::default();
        let kp = kp_in
            .validate(&verifier, ProtocolVersion::Mls10)
            .expect("KeyPackage must validate (signature + leaf + version)");
        assert_eq!(
            kp.ciphersuite(),
            MLS_CIPHERSUITE,
            "KeyPackage must declare Wire's P-256 ciphersuite"
        );
    }

    #[test]
    fn generated_key_packages_declare_basic_and_x509_leaf_capabilities() {
        let client = fresh_client();
        let batch = client.generate_key_packages(1).expect("generate");
        let kp_in = deserialize_key_package(&batch[0]);
        let verifier = openmls_rust_crypto::RustCrypto::default();
        let kp = kp_in
            .validate(&verifier, ProtocolVersion::Mls10)
            .expect("validate");
        let credentials = kp.leaf_node().capabilities().credentials();
        assert!(
            credentials.contains(&CredentialType::Basic),
            "leaf must accept peers presenting Basic credentials"
        );
        assert!(
            credentials.contains(&CredentialType::X509),
            "leaf must accept peers presenting X509 credentials (Wire e2ei groups require this)"
        );
    }

    // ---- Step 1c.5 failing tests (persistent openmls state) ----

    use openmls_sqlite_storage::Connection;
    use tempfile::TempDir;

    fn open_file_conn(dir: &TempDir) -> Connection {
        let path = dir.path().join("mls.db");
        Connection::open(&path).expect("open mls.db")
    }

    #[test]
    fn new_fresh_with_storage_persists_signature_keypair_to_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let user_id = "a11ce000-0000-4000-8000-000000000001@wire.com";
        let client_id = "a11ce00000000001";

        // First session: create + drop, forcing the Connection to flush.
        let public_key = {
            let provider = KupferdrahtProvider::open(open_file_conn(&dir)).expect("provider 1");
            let client = MlsClient::new_fresh_with_storage(provider, user_id, client_id)
                .expect("new_fresh_with_storage");
            client.signature_public_key().to_vec()
        };
        assert!(!public_key.is_empty(), "public key must be populated");

        // Second session: open the same file, resume, assert the public
        // key matches the one written in session 1.
        let provider = KupferdrahtProvider::open(open_file_conn(&dir)).expect("provider 2");
        let resumed = MlsClient::resume_with_storage(provider, user_id, client_id, &public_key)
            .expect("resume_with_storage must find the persisted keypair");
        assert_eq!(
            resumed.signature_public_key(),
            public_key.as_slice(),
            "resumed MlsClient must carry the same signature public key"
        );
    }

    #[test]
    fn resume_with_storage_fails_when_key_not_in_storage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let provider = KupferdrahtProvider::open(open_file_conn(&dir)).expect("provider");
        // No `new_fresh_with_storage` has been called against this
        // provider, so the signature key table is empty.
        let result = MlsClient::resume_with_storage(
            provider,
            "a11ce000-0000-4000-8000-000000000001@wire.com",
            "a11ce00000000001",
            // A public key the store has never seen.
            &[0xDE, 0xAD, 0xBE, 0xEF],
        );
        let Err(err) = result else {
            panic!("resume on empty storage must fail");
        };
        assert!(
            matches!(err, KupferdrahtError::Storage(_)),
            "expected Storage, got {err:?}"
        );
    }

    #[test]
    fn generate_key_packages_persists_private_keys_across_reopens() {
        // Open a file-backed provider, generate a few KeyPackages, drop
        // the MlsClient (and its provider) so the file is flushed, then
        // reopen and confirm the openmls `key_packages` table has rows
        // (we inspect the raw table to avoid coupling the test to
        // openmls_sqlite_storage internals).
        let dir = tempfile::tempdir().expect("tempdir");
        let user_id = "a11ce000-0000-4000-8000-000000000001@wire.com";
        let client_id = "a11ce00000000001";
        {
            let provider = KupferdrahtProvider::open(open_file_conn(&dir)).expect("provider");
            let client = MlsClient::new_fresh_with_storage(provider, user_id, client_id)
                .expect("new_fresh_with_storage");
            let _ = client.generate_key_packages(3).expect("generate");
        }
        // Reopen the file directly and count rows in openmls's
        // key_packages table.
        let raw = open_file_conn(&dir);
        let count: i64 = raw
            .query_row("SELECT COUNT(*) FROM openmls_key_packages", [], |row| {
                row.get(0)
            })
            .expect("count key_packages");
        assert!(
            count >= 3,
            "expected at least 3 persisted KeyPackage rows, got {count}"
        );
    }
}
