// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 18b — shipped-migration immutability guard.
//!
//! CHANGELOG.md §50: a doc-comment-only edit to `V12` / `V14` (commit
//! `f31ac5e`) changed refinery's per-migration checksum — refinery
//! hashes the **whole file content, comments included** — which
//! hard-fails every pre-existing database the moment it is opened
//! (`applied migration … is different than filesystem one …`).
//!
//! A migration file is append-only once it has shipped: never edit it,
//! comments included. This guard pins the SHA-256 of every file under
//! `src/storage/migrations/`. Any edit to a shipped file — or a new
//! file added without an explicit pin — fails the workspace test gate
//! (`cargo test --workspace`, part of the pre-commit gate) until the
//! pinned table is updated deliberately. That turns "I accidentally
//! touched a migration" from a silent runtime break on users'
//! databases into a test failure caught before the change ever lands.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// SHA-256 (lower-case hex) of every migration file, pinned at its
/// shipped content.
///
/// Adding a migration requires appending its digest here — a
/// deliberate, reviewed act that should only ever extend the table
/// with the newest version. Editing a shipped migration changes its
/// digest and fails [`shipped_migrations_match_pinned_digests`]; the
/// correct fix is to revert the edit, not to re-pin, because the old
/// digest is already recorded in every existing database's
/// `refinery_schema_history`.
const PINNED_DIGESTS: &[(&str, &str)] = &[
    (
        "V1__step_1a_session_self_server.sql",
        "f5b4af51532b0fce2483d5f7e0593128dcc32b4420fc58a22c69b6889026524c",
    ),
    (
        "V2__step_1b_devices.sql",
        "53a717ba98c6bd299c189b3ba343372c80c2f8eb8c53e6a6dd23c0a55c7a9a65",
    ),
    (
        "V3__step_1c_backend_mls_keys.sql",
        "7a937f483a0a65597fb02b0b0026cf65527372b336cbd5db738378a7194fa600",
    ),
    (
        "V4__step_2_rooms.sql",
        "819d9f7b2630ea0cd0c6c7a05bd097ef38774a666ff62a1f046ecc2fff430649",
    ),
    (
        "V5__step_2_resume.sql",
        "5ea4c41e31edd8f4b6821584be510a0965fa537426e73914062bb7bca9ae3624",
    ),
    (
        "V6__step_4_timeline_events.sql",
        "cd22b2112334d19a2348bd24b00907245f0c66838f8ccf99c8520c961dce4ee8",
    ),
    (
        "V7__step_5b_sync_state.sql",
        "eedd55eaa78cc23d5ad93d3c0bcb9c74bd9798dc5e9cd2bd95f024cc0546012c",
    ),
    (
        "V8__step_7e_session_device_link.sql",
        "5f67ba61109e2fb40bfda3d133346c03f26e4232d62d46fd96a70e5ba6e1d567",
    ),
    (
        "V9__step_6_ii_a_drop_timeline_events_cascade.sql",
        "1ca8ae7d0cc19f848bb241c8842daccbd273a4a76a3faef920631757d31843b7",
    ),
    (
        "V10__step_6_ii_b_reply_to.sql",
        "27eddee77c5ac6b35f943a0e386235de83469b8f8beeb0fddbdb7d747513433d",
    ),
    (
        "V11__step_6_ii_c_edited_at_ms.sql",
        "4ef1c6e2fe9b36ba079454131c0ed660285126701fb7bf312e437321f204e21b",
    ),
    (
        "V12__step_6_ii_c_edit_chain.sql",
        "76c6e652affd03ea4f60ef72293567e87b161e3e328eb13ddcaf1174bcc35f0e",
    ),
    (
        "V13__step_6_ii_d_redacted_at_ms.sql",
        "ff6b0ec998033a046d1ad9850fc0a187218ec8027735bf2a4bc9f1170e525d90",
    ),
    (
        "V14__step_6_ii_e_reactions.sql",
        "4e94eb0cb408229434a3b12404e0256b6cc1025982f63755a3c31a7cb870f3c7",
    ),
    (
        "V15__step_6_ii_f_read_receipts.sql",
        "ff59581534fb3b706ce81728e100f6935c7d7caaf9412e44a3d6bcc73f708abf",
    ),
    (
        "V16__step_6_ii_g_mentions.sql",
        "992ebd0fa31168f586df738e2a59a2dfa2f2f091041cbfd4c6c1559d53b5bfb0",
    ),
    (
        "V17__step_7b_meta_schema_version.sql",
        "2508110ae238df150bf0a63263defb1707066233084ed45858b822317b15a875",
    ),
    (
        "V18__step_10_server_configs.sql",
        "83d858df5e0e33fb60f9ce16298a410ae1f561487c623b639df03bb19d27ecd2",
    ),
    (
        "V19__step_11_attachments.sql",
        "97e31a7ffb9bae492d986808698b95d4cddb81c26fda57e8c53ab089a888ce56",
    ),
    (
        "V20__step_2b_drop_per_field_columns.sql",
        "f1463d0185dec1c3d85ba92a96d69ee93d7d13779d0305f206f2c4ea8f5f992a",
    ),
    (
        "V21__step_19_drop_self_user_domain.sql",
        "f7d7c1a3eca66e556d36ed87216f0bc346f45935e78cac30301ac4e9b594907c",
    ),
    (
        "V22__step_25b_delivery_receipts.sql",
        "fec3650a5018cb31b20d817942c4a217fe77489c80a4f22c038a0f2b5bd8a26d",
    ),
    (
        "V23__step_60c_tasks.sql",
        "ecadd4c01ad3dbd7b897054a7da0faf07ef6bf6e16941cd2171016f7ff080dfb",
    ),
    (
        "V24__step_63_key_package_check.sql",
        "37638567759fafaf0fd652a5f8981886d59bb4d085a1b324a95fe78431f8ea3c",
    ),
    (
        "V25__step_67_read_markers.sql",
        "f04e269e4f28eaf22232f318dcdb3f74f9d2303e90a14284f0b90bd5dc3c5ef2",
    ),
    (
        "V26__step_72a_member_roles.sql",
        "06c6ce4a384273afa0d12e7f1a3355f2a42454e23b946a1b73600ce00393792c",
    ),
    (
        "V27__step_89_ephemeral_expiry.sql",
        "0ce372487e910f6d85d079bea30d861a30191493fb3485f1596526f9f1f2a34b",
    ),
    (
        "V28__step_79_ephemeral_ttl.sql",
        "c00d8d1766850f48c3caa752c4dcf77e5aba4b59ce0d7570ea11fd0a58c3ad6f",
    ),
    (
        "V29__step_95d_room_devices.sql",
        "98e5e597b2986dc64535d81697f03d9dc070c4551adbfb5d5229ed153ec6a5f9",
    ),
    (
        "V30__step_112_recovery_state.sql",
        "42203ee392c88ce6e12daf12efeacbf55d18aeb13c7378bff2cf4f64c2edff07",
    ),
];

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage/migrations")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every shipped migration's content still hashes to its pinned
/// digest — i.e. no already-released migration file has changed.
#[test]
fn shipped_migrations_match_pinned_digests() {
    let dir = migrations_dir();
    for (name, expected) in PINNED_DIGESTS {
        let path = dir.join(name);
        let bytes =
            fs::read(&path).unwrap_or_else(|e| panic!("pinned migration {name} unreadable: {e}"));
        let actual = sha256_hex(&bytes);
        assert_eq!(
            &actual.as_str(),
            expected,
            "migration {name} changed since it shipped — refinery's checksum \
             would no longer match existing databases (CHANGELOG.md §50). Shipped \
             migrations are immutable: revert the edit. A genuinely new \
             migration must be the newest version and pinned as a NEW entry, \
             never an edit to an existing one."
        );
    }
}

/// No migration file exists on disk without a matching pin, and the
/// pinned table has no stale entry — adding or removing a migration
/// must be an explicit edit to `PINNED_DIGESTS`.
#[test]
fn every_migration_file_is_pinned() {
    let dir = migrations_dir();
    let mut on_disk: Vec<String> = fs::read_dir(&dir)
        .expect("migrations dir readable")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sql"))
        .collect();
    on_disk.sort();
    for name in &on_disk {
        assert!(
            PINNED_DIGESTS.iter().any(|(n, _)| n == name),
            "migration {name} exists on disk but is not pinned in PINNED_DIGESTS \
             — append its SHA-256 (CHANGELOG.md §50 guard)."
        );
    }
    assert_eq!(
        on_disk.len(),
        PINNED_DIGESTS.len(),
        "migration count mismatch: {} files on disk, {} pinned — a pin refers \
         to a file that no longer exists.",
        on_disk.len(),
        PINNED_DIGESTS.len()
    );
}
