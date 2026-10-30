// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Shared openmls fixtures for integration tests that need real MLS
//! artifacts on the wire — TLS-serialized key packages for claim
//! mocks and exported `GroupInfo` for external-join mocks (the
//! Step 3 / 14a / 68 families).
//!
//! Every helper builds against a throw-away in-memory provider; the
//! artifacts are structurally valid for openmls' parsers, which is
//! what the mocked Wire endpoints must deliver.

#![allow(dead_code)]

use base64::Engine as _;
use openmls::extensions::SenderExtensionIndex;
use openmls::framing::{ProcessedMessageContent, ProtocolMessage};
use openmls::group::{
    GroupContext, GroupId, MIXED_PLAINTEXT_WIRE_FORMAT_POLICY, MlsGroup, MlsGroupCreateConfig,
    MlsGroupJoinConfig, StagedWelcome,
};
use openmls::messages::external_proposals::ExternalProposal;
use openmls::prelude::{
    BasicCredential, Capabilities, Ciphersuite, Credential, CredentialType, CredentialWithKey,
    Extension, Extensions, ExternalSender, KeyPackage, KeyPackageIn, LeafNodeIndex,
    MlsMessageBodyIn, MlsMessageIn, ProtocolVersion, SignaturePublicKey, WireFormat,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::RustCrypto;
use openmls_sqlite_storage::{Codec, Connection, SqliteStorageProvider};
use openmls_traits::OpenMlsProvider;
use openmls_traits::types::SignatureScheme;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tls_codec::{Deserialize as _, DeserializeBytes as _, Serialize as _};

pub const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMP256_AES128GCM_SHA256_P256;
pub const SIGNATURE_SCHEME: SignatureScheme = SignatureScheme::ECDSA_SECP256R1_SHA256;

#[derive(Default)]
pub struct TestJsonCodec;

impl Codec for TestJsonCodec {
    type Error = serde_json::Error;
    fn to_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }
    fn from_slice<T: DeserializeOwned>(slice: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(slice)
    }
}

/// Stand-alone openmls provider for fixture peers (RustCrypto +
/// in-memory `SqliteStorageProvider<TestJsonCodec>`).
pub struct PeerProvider {
    crypto: RustCrypto,
    storage: SqliteStorageProvider<TestJsonCodec, Connection>,
}

impl PeerProvider {
    pub fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("sqlite in-memory");
        let mut storage: SqliteStorageProvider<TestJsonCodec, Connection> =
            SqliteStorageProvider::new(conn);
        storage.run_migrations().expect("openmls migrations");
        Self {
            crypto: RustCrypto::default(),
            storage,
        }
    }
}

impl OpenMlsProvider for PeerProvider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = SqliteStorageProvider<TestJsonCodec, Connection>;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }
    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }
    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

/// Leaf capabilities matching Kupferdraht's own groups (Basic + X509
/// credentials) so fixture leaves survive Kupferdraht's validation.
pub fn wire_capabilities() -> Capabilities {
    Capabilities::new(
        None,
        None,
        None,
        None,
        Some(&[CredentialType::Basic, CredentialType::X509]),
    )
}

/// A real TLS-serialized `KeyPackage` under `identity` (Wire shape:
/// `{uuid}:{client}@{domain}`), base64 as the claim endpoint
/// delivers it.
pub fn key_package_b64(identity: &[u8]) -> String {
    let provider = PeerProvider::new_in_memory();
    let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
    signer.store(provider.storage()).expect("persist signer");
    let credential = CredentialWithKey {
        credential: BasicCredential::new(identity.to_vec()).into(),
        signature_key: signer.public().to_vec().into(),
    };
    let bundle = KeyPackage::builder()
        .leaf_node_capabilities(wire_capabilities())
        .build(CIPHERSUITE, &provider, &signer, credential)
        .expect("key package");
    let bytes = bundle
        .key_package()
        .tls_serialize_detached()
        .expect("TLS serialize key package");
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Stateful fixture peer owning a real MLS group — the counterpart
/// for receive-path tests that need a peer to invite the client
/// (Welcome) and then talk to it (application messages) with
/// consistent epochs. Step 72b; the Step-69 free function below
/// delegates here.
pub struct PeerGroup {
    provider: PeerProvider,
    signer: SignatureKeyPair,
    group: MlsGroup,
    /// Present when the group was created via
    /// [`PeerGroup::new_with_external_sender`]: the keypair whose
    /// public half sits in the group's `external_senders` extension,
    /// so the fixture can forge backend-style Remove proposals
    /// (Step 74).
    external_sender: Option<SignatureKeyPair>,
}

impl PeerGroup {
    /// Create the peer's group under `group_id` with Kupferdraht-shaped
    /// capabilities + ratchet-tree extension (so Welcomes carry the
    /// tree).
    pub fn new(group_id: &[u8], identity: &[u8]) -> Self {
        Self::new_with_credential(group_id, BasicCredential::new(identity.to_vec()).into())
    }

    /// Like [`PeerGroup::new`], but with Wire's `MIXED_PLAINTEXT`
    /// wire-format policy (Kupferdraht's own, `group_create.rs`): the peer
    /// accepts `PublicMessage` commits — what a client under test
    /// uploads — and still encrypts application messages (Step 97d).
    pub fn new_mixed_plaintext(group_id: &[u8], identity: &[u8]) -> Self {
        let provider = PeerProvider::new_in_memory();
        let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
        signer.store(provider.storage()).expect("persist signer");
        let credential = CredentialWithKey {
            credential: BasicCredential::new(identity.to_vec()).into(),
            signature_key: signer.public().to_vec().into(),
        };
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .capabilities(wire_capabilities())
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .build();
        let group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &config,
            GroupId::from_slice(group_id),
            credential,
        )
        .expect("peer group");
        Self {
            provider,
            signer,
            group,
            external_sender: None,
        }
    }

    /// Like [`PeerGroup::new`], but the peer's own leaf carries an
    /// arbitrary `credential` — an e2ei-style X.509 credential or a
    /// Basic one whose identity is not Wire's W-4 shape (Step 95a).
    pub fn new_with_credential(group_id: &[u8], credential: Credential) -> Self {
        let provider = PeerProvider::new_in_memory();
        let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
        signer.store(provider.storage()).expect("persist signer");
        let credential = CredentialWithKey {
            credential,
            signature_key: signer.public().to_vec().into(),
        };
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .capabilities(wire_capabilities())
            .build();
        let group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &config,
            GroupId::from_slice(group_id),
            credential,
        )
        .expect("peer group");
        Self {
            provider,
            signer,
            group,
            external_sender: None,
        }
    }

    /// Like [`PeerGroup::new`], but the group's `external_senders`
    /// extension carries a fixture-held keypair (mirroring Wire's
    /// backend removal key, `group_create.rs`), so
    /// [`PeerGroup::external_remove_proposal`] can forge proposals
    /// that validate on every member — including a Kupferdraht client
    /// that joined via Welcome. Step 74.
    pub fn new_with_external_sender(group_id: &[u8], identity: &[u8]) -> Self {
        let provider = PeerProvider::new_in_memory();
        let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
        signer.store(provider.storage()).expect("persist signer");
        let es = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("external-sender signer");
        let es_entry = ExternalSender::new(
            SignaturePublicKey::from(es.public().to_vec()),
            Credential::from(BasicCredential::new(b"wire-backend".to_vec())),
        );
        let extensions: Extensions<GroupContext> =
            Extensions::single(Extension::ExternalSenders(vec![es_entry]))
                .expect("external_senders extension");
        let credential = CredentialWithKey {
            credential: BasicCredential::new(identity.to_vec()).into(),
            signature_key: signer.public().to_vec().into(),
        };
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .capabilities(wire_capabilities())
            .with_group_context_extensions(extensions)
            .build();
        let group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &config,
            GroupId::from_slice(group_id),
            credential,
        )
        .expect("peer group with external sender");
        Self {
            provider,
            signer,
            group,
            external_sender: Some(es),
        }
    }

    /// Add the owner of `kp_tls` and return the TLS **commit** for
    /// delivery to members that are already in the group (the
    /// Welcome for the new member is discarded — Step 74's tests
    /// grow the roster to get removable peer leaves, they never
    /// drive the added member). Merges, so the peer's epoch advances.
    pub fn add_member_commit(&mut self, kp_tls: &[u8]) -> Vec<u8> {
        let kp_in = KeyPackageIn::tls_deserialize_exact(kp_tls).expect("KeyPackage TLS parse");
        let kp: KeyPackage = kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .expect("KeyPackage validation");
        let (commit, _welcome_unused, _group_info) = self
            .group
            .add_members_without_update(&self.provider, &self.signer, &[kp])
            .expect("add_members_without_update");
        self.group
            .merge_pending_commit(&self.provider)
            .expect("merge add commit");
        commit
            .tls_serialize_detached()
            .expect("TLS serialize add commit")
    }

    /// Add the owner of `kp_tls`, merge, and return `(commit, welcome)`
    /// as TLS bytes — the commit for members already in the group (a
    /// [`PeerDevice`] keeps its epoch in step via
    /// [`PeerDevice::process_commit`]), the Welcome for the new member
    /// (Step 95b: multi-member groups).
    pub fn add_member(&mut self, kp_tls: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let kp_in = KeyPackageIn::tls_deserialize_exact(kp_tls).expect("KeyPackage TLS parse");
        let kp: KeyPackage = kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .expect("KeyPackage validation");
        let (commit, welcome, _group_info) = self
            .group
            .add_members_without_update(&self.provider, &self.signer, &[kp])
            .expect("add_members_without_update");
        self.group
            .merge_pending_commit(&self.provider)
            .expect("merge add commit");
        (
            commit
                .tls_serialize_detached()
                .expect("TLS serialize commit"),
            welcome
                .tls_serialize_detached()
                .expect("TLS serialize welcome"),
        )
    }

    /// Leaf index of the member whose `BasicCredential` identity is
    /// `identity` (Step 95d).
    ///
    /// # Panics
    /// Panics when no member carries that identity.
    pub fn leaf_index_of(&self, identity: &[u8]) -> u32 {
        self.group
            .members()
            .find(|m| m.credential.serialized_content() == identity)
            .expect("member with that identity")
            .index
            .u32()
    }

    /// Remove the member at `leaf`, merge, and return the TLS commit
    /// for the remaining members (Step 95d: a device leaves the group).
    pub fn remove_member(&mut self, leaf: u32) -> Vec<u8> {
        let (commit, _welcome, _group_info) = self
            .group
            .remove_members(&self.provider, &self.signer, &[LeafNodeIndex::new(leaf)])
            .expect("remove_members");
        self.group
            .merge_pending_commit(&self.provider)
            .expect("merge remove commit");
        commit
            .tls_serialize_detached()
            .expect("TLS serialize remove commit")
    }

    /// Forge an external-sender Remove proposal (`PublicMessage`) for
    /// leaf `removed`, signed with the fixture's external-sender key
    /// at the peer's current epoch — the artifact Wire's backend
    /// emits when a device is deleted (Step 28 receive path).
    ///
    /// # Panics
    /// Panics when the group was not created via
    /// [`PeerGroup::new_with_external_sender`].
    pub fn external_remove_proposal(&self, removed: u32) -> Vec<u8> {
        let es = self
            .external_sender
            .as_ref()
            .expect("group must be created with new_with_external_sender");
        let proposal = ExternalProposal::new_remove::<PeerProvider>(
            LeafNodeIndex::new(removed),
            self.group.group_id().clone(),
            self.group.epoch(),
            es,
            SenderExtensionIndex::new(0),
        )
        .expect("new_remove external proposal");
        proposal
            .tls_serialize_detached()
            .expect("serialize external proposal")
    }

    /// Add the owner of `kp_tls` (path-less Add — Wire's shape),
    /// merge the commit so the peer's epoch advances in lockstep
    /// with the Welcome it hands out, and return the TLS
    /// `MlsMessage(Welcome)`.
    pub fn welcome_over_key_package(&mut self, kp_tls: &[u8]) -> Vec<u8> {
        let kp_in = KeyPackageIn::tls_deserialize_exact(kp_tls).expect("KeyPackage TLS parse");
        let kp: KeyPackage = kp_in
            .validate(self.provider.crypto(), ProtocolVersion::Mls10)
            .expect("KeyPackage validation");
        let (_commit, welcome, _group_info) = self
            .group
            .add_members_without_update(&self.provider, &self.signer, &[kp])
            .expect("add_members_without_update");
        self.group
            .merge_pending_commit(&self.provider)
            .expect("merge add commit");
        welcome
            .tls_serialize_detached()
            .expect("TLS serialize welcome")
    }

    /// Process + merge a commit (TLS bytes) another member — e.g. the
    /// client under test — produced, so the peer follows it to the
    /// next epoch (Step 97d).
    ///
    /// # Panics
    /// Panics on a message that is not a mergeable commit.
    pub fn process_commit(&mut self, tls: &[u8]) {
        let msg = MlsMessageIn::tls_deserialize_exact(tls).expect("MlsMessage parse");
        let protocol = match msg.extract() {
            MlsMessageBodyIn::PrivateMessage(m) => ProtocolMessage::PrivateMessage(m),
            MlsMessageBodyIn::PublicMessage(m) => ProtocolMessage::PublicMessage(Box::new(m)),
            _ => panic!("expected an MlsMessage(Private|PublicMessage)"),
        };
        let processed = self
            .group
            .process_message(&self.provider, protocol)
            .expect("process commit");
        match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged) => self
                .group
                .merge_staged_commit(&self.provider, *staged)
                .expect("merge staged commit"),
            _ => panic!("expected a commit"),
        }
    }

    /// The peer's current epoch.
    pub fn epoch(&self) -> u64 {
        self.group.epoch().as_u64()
    }

    /// Encrypt `plaintext` (a serialized `GenericMessage`) as an MLS
    /// application message at the peer's current epoch; TLS bytes as
    /// `conversation.mls-message-add` delivers them.
    pub fn application_message(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.group
            .create_message(&self.provider, &self.signer, plaintext)
            .expect("create application message")
            .tls_serialize_detached()
            .expect("TLS serialize application message")
    }
}

/// Peer-side Add-commit over a real TLS-serialized `KeyPackage`:
/// returns the TLS bytes of the `MlsMessage(Welcome)` addressed to
/// that package's owner, produced by a throw-away fixture peer
/// (`identity`) whose group carries `group_id`. See [`PeerGroup`]
/// for the stateful variant receive-path tests use.
pub fn welcome_over_key_package(group_id: &[u8], identity: &[u8], kp_tls: &[u8]) -> Vec<u8> {
    PeerGroup::new(group_id, identity).welcome_over_key_package(kp_tls)
}

/// TLS-serialized `GroupInfo` (ratchet tree included) of a group a
/// fixture peer under `identity` established as `group_id` — the
/// body `GET .../groupinfo` must serve for an external-commit join.
pub fn group_info_tls(group_id: &[u8], identity: &[u8]) -> Vec<u8> {
    let provider = PeerProvider::new_in_memory();
    let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
    signer.store(provider.storage()).expect("persist signer");
    let credential = CredentialWithKey {
        credential: BasicCredential::new(identity.to_vec()).into(),
        signature_key: signer.public().to_vec().into(),
    };
    let config = MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .capabilities(wire_capabilities())
        .build();
    let group = MlsGroup::new_with_group_id(
        &provider,
        &signer,
        &config,
        GroupId::from_slice(group_id),
        credential,
    )
    .expect("peer group");
    group
        .export_group_info(provider.crypto(), &signer, true)
        .expect("export group_info")
        .tls_serialize_detached()
        .expect("TLS serialize group_info")
}

/// A second device of the **same** user (Step 86 — the self
/// conversation is an MLS group of one user's devices). Owns its
/// provider + signer, hands out a key package whose private half
/// stays in its own storage (so a Welcome addressed to it can be
/// absorbed), joins through the Welcome the client's establishing
/// Add-commit carries, and decrypts the client's application
/// messages — the "own devices" fan-out pin.
pub struct PeerDevice {
    provider: PeerProvider,
    signer: SignatureKeyPair,
    credential: CredentialWithKey,
    group: Option<MlsGroup>,
}

impl PeerDevice {
    /// `identity` is Wire's leaf shape `{uuid}:{client}@{domain}` —
    /// for a same-user device the `{uuid}` half equals the client's.
    pub fn new(identity: &[u8]) -> Self {
        let provider = PeerProvider::new_in_memory();
        let signer = SignatureKeyPair::new(SIGNATURE_SCHEME).expect("signer");
        signer.store(provider.storage()).expect("persist signer");
        let credential = CredentialWithKey {
            credential: BasicCredential::new(identity.to_vec()).into(),
            signature_key: signer.public().to_vec().into(),
        };
        Self {
            provider,
            signer,
            credential,
            group: None,
        }
    }

    /// One TLS key package, base64 as the claim endpoint delivers it.
    /// Built against this device's provider so the private keys are
    /// on file for [`PeerDevice::join_via_welcome`].
    pub fn key_package_b64(&self) -> String {
        let bundle = KeyPackage::builder()
            .leaf_node_capabilities(wire_capabilities())
            .build(
                CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential.clone(),
            )
            .expect("key package");
        let bytes = bundle
            .key_package()
            .tls_serialize_detached()
            .expect("TLS serialize key package");
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Absorb the `MlsMessage(Welcome)` (TLS bytes) the client's
    /// Add-commit produced for this device — the same join config
    /// Kupferdraht's own `absorb_welcome` uses.
    pub fn join_via_welcome(&mut self, welcome_tls: &[u8]) {
        let msg = MlsMessageIn::tls_deserialize_exact(welcome_tls).expect("MlsMessage parse");
        let MlsMessageBodyIn::Welcome(welcome) = msg.extract() else {
            panic!("segment is not an MlsMessage(Welcome)");
        };
        let config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)
            .build();
        let group = StagedWelcome::build_from_welcome(&self.provider, &config, welcome)
            .expect("stage welcome")
            .skip_lifetime_validation()
            .build()
            .expect("build staged welcome")
            .into_group(&self.provider)
            .expect("into_group");
        self.group = Some(group);
    }

    /// Raw TLS key package (the [`PeerDevice::key_package_b64`] bytes
    /// before base64) — what [`PeerGroup::add_member`] consumes.
    pub fn key_package_tls(&self) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(self.key_package_b64())
            .expect("own key package is base64")
    }

    /// Process + merge a commit (TLS bytes) another member produced, so
    /// this device stays at the group's current epoch.
    ///
    /// # Panics
    /// Panics before [`PeerDevice::join_via_welcome`] or on a message
    /// that is not a mergeable commit.
    pub fn process_commit(&mut self, tls: &[u8]) {
        let group = self.group.as_mut().expect("join_via_welcome first");
        let msg = MlsMessageIn::tls_deserialize_exact(tls).expect("MlsMessage parse");
        let protocol = match msg.extract() {
            MlsMessageBodyIn::PrivateMessage(m) => ProtocolMessage::PrivateMessage(m),
            MlsMessageBodyIn::PublicMessage(m) => ProtocolMessage::PublicMessage(Box::new(m)),
            _ => panic!("expected an MlsMessage(Private|PublicMessage)"),
        };
        let processed = group
            .process_message(&self.provider, protocol)
            .expect("process commit");
        match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged) => group
                .merge_staged_commit(&self.provider, *staged)
                .expect("merge staged commit"),
            _ => panic!("expected a commit"),
        }
    }

    /// Encrypt `plaintext` (a serialized `GenericMessage`) as an MLS
    /// application message from this device, at its current epoch.
    ///
    /// # Panics
    /// Panics before [`PeerDevice::join_via_welcome`].
    pub fn application_message(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let group = self.group.as_mut().expect("join_via_welcome first");
        group
            .create_message(&self.provider, &self.signer, plaintext)
            .expect("create application message")
            .tls_serialize_detached()
            .expect("TLS serialize application message")
    }

    /// Epoch of the joined group.
    ///
    /// # Panics
    /// Panics before [`PeerDevice::join_via_welcome`].
    pub fn epoch(&self) -> u64 {
        self.group
            .as_ref()
            .expect("join_via_welcome first")
            .epoch()
            .as_u64()
    }

    /// Decrypt one `MlsMessage(PrivateMessage)` (TLS bytes, exactly
    /// what `POST /v14/mls/messages` carries) at this device's group
    /// and return the application plaintext (a `GenericMessage`).
    pub fn decrypt_application_message(&mut self, tls: &[u8]) -> Vec<u8> {
        let group = self.group.as_mut().expect("join_via_welcome first");
        let msg = MlsMessageIn::tls_deserialize_exact(tls).expect("MlsMessage parse");
        let protocol = match msg.extract() {
            MlsMessageBodyIn::PrivateMessage(m) => ProtocolMessage::PrivateMessage(m),
            MlsMessageBodyIn::PublicMessage(m) => ProtocolMessage::PublicMessage(Box::new(m)),
            _ => panic!("expected an MlsMessage(Private|PublicMessage)"),
        };
        let processed = group
            .process_message(&self.provider, protocol)
            .expect("process_message");
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => app.into_bytes(),
            _ => panic!("expected an application message"),
        }
    }
}

/// Split a Wire commit bundle (`commit || group_info || welcome?`,
/// each an `MlsMessage`) into its segments: `(wire_format, bytes)`
/// per segment, in order.
pub fn bundle_segments(bundle: &[u8]) -> Vec<(WireFormat, Vec<u8>)> {
    let mut out = Vec::new();
    let mut rest = bundle;
    while !rest.is_empty() {
        let (msg, after) = MlsMessageIn::tls_deserialize_bytes(rest).expect("bundle segment");
        let consumed = rest.len() - after.len();
        out.push((msg.wire_format(), rest[..consumed].to_vec()));
        rest = after;
    }
    out
}

impl PeerGroup {
    /// Decrypt one `MlsMessage(PrivateMessage)` (TLS bytes, exactly what
    /// `POST /v14/mls/messages` carries) at the peer's group and return
    /// the application plaintext (a `GenericMessage`) — the receiver-side
    /// pin for what a Kupferdraht client put on the wire (Step 79). openmls
    /// tolerates skipped sender generations, so a ciphertext the peer
    /// never saw (a rejected upload) does not block the next one.
    pub fn decrypt_application_message(&mut self, tls: &[u8]) -> Vec<u8> {
        let msg = MlsMessageIn::tls_deserialize_exact(tls).expect("MlsMessage parse");
        let protocol = match msg.extract() {
            MlsMessageBodyIn::PrivateMessage(m) => ProtocolMessage::PrivateMessage(m),
            MlsMessageBodyIn::PublicMessage(m) => ProtocolMessage::PublicMessage(Box::new(m)),
            _ => panic!("expected an MlsMessage(Private|PublicMessage)"),
        };
        let processed = self
            .group
            .process_message(&self.provider, protocol)
            .expect("process_message");
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(app) => app.into_bytes(),
            _ => panic!("expected an application message"),
        }
    }
}
