// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 97c — one transaction per MLS operation (review blocker B8,
//! plan D5). TDD red.
//!
//! openmls writes through `openmls_sqlite_storage` one autocommit
//! statement per record. A merge writes the group state first — the
//! pending commit is gone from then on — then tree, context, epoch
//! secrets and message secrets; a Welcome deletes the key package
//! before it stores the group. A failure in between leaves `mls.db`
//! half in the old epoch, half in the new one, and nothing to retry.
//! Every mutating `MlsClient` operation must run in one transaction:
//! all its writes land, or none.
//!
//! Faults are injected with a SQLite trigger created through a second
//! connection to the same file (`RAISE(ABORT)` on one chosen write);
//! commits are counted with a `commit_hook` on the provider's own
//! connection.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use openmls::prelude::MlsMessageIn;
use openmls_sqlite_storage::Connection;
use tls_codec::DeserializeBytes as _;

use crate::encryption::messages::DecodedMessage;
use crate::encryption::mls_client::MlsClient;
use crate::encryption::provider::KupferdrahtProvider;
use crate::encryption::test_support::{ALICE_USER, BOB_USER};
use crate::error::KupferdrahtError;

const ALICE_CLIENT: &str = "a11ce00000000001";
const BOB_CLIENT: &str = "deadbeefcafef00d";
const GROUP_ID: &[u8] = b"step97c-atomic-group";
/// Placeholder external-sender key (as in the Step-76 fixture generator).
const REMOVAL_KEY: [u8; 65] = [0x04; 65];

/// An `MlsClient` over a file-backed `mls.db` at `dir/name`.
fn file_client(dir: &Path, name: &str, user: &str, client: &str) -> (MlsClient, PathBuf) {
    let path = dir.join(name);
    let conn = Connection::open(&path).expect("open mls db");
    let provider = KupferdrahtProvider::open(conn).expect("provider");
    let mls = MlsClient::new_fresh_with_storage(provider, user, client).expect("MlsClient");
    (mls, path)
}

/// An in-memory `MlsClient` whose connection counts its commits.
fn counted_client(user: &str, client: &str) -> (MlsClient, Arc<AtomicUsize>) {
    let conn = Connection::open_in_memory().expect("in-memory db");
    let commits = Arc::new(AtomicUsize::new(0));
    let hook = commits.clone();
    conn.commit_hook(Some(move || {
        hook.fetch_add(1, Ordering::SeqCst);
        false
    }));
    let provider = KupferdrahtProvider::open(conn).expect("provider");
    let mls = MlsClient::new_fresh_with_storage(provider, user, client).expect("MlsClient");
    (mls, commits)
}

/// Make every write matching `when` (a trigger `WHEN` clause over the
/// `openmls_group_data` row `NEW`) fail from now on.
fn inject_fault(db: &Path, when: &str) {
    let conn = Connection::open(db).expect("second connection");
    conn.execute_batch(&format!(
        "CREATE TRIGGER step97c_fault BEFORE INSERT ON openmls_group_data \
         WHEN {when} BEGIN SELECT RAISE(ABORT, 'injected fault'); END;"
    ))
    .expect("install the fault trigger");
}

fn heal(db: &Path) {
    let conn = Connection::open(db).expect("second connection");
    conn.execute_batch("DROP TRIGGER step97c_fault;")
        .expect("drop the fault trigger");
}

fn one_key_package(member: &MlsClient) -> Vec<u8> {
    member
        .generate_key_packages(1)
        .expect("key package")
        .into_iter()
        .next()
        .expect("one key package")
}

/// The Welcome segment of an Add-commit bundle
/// (`commit || group_info || welcome`).
fn welcome_of(bundle: &[u8]) -> Vec<u8> {
    let (_commit, rest) = MlsMessageIn::tls_deserialize_bytes(bundle).expect("commit segment");
    let (_group_info, welcome) = MlsMessageIn::tls_deserialize_bytes(rest).expect("group info");
    welcome.to_vec()
}

/// Alice sends one application message; Bob must read it back.
fn assert_bob_reads_alice(alice: &MlsClient, bob: &MlsClient, text: &[u8]) {
    let ciphertext = alice
        .encrypt_text_message(GROUP_ID, text)
        .expect("alice encrypts");
    match bob
        .process_inbound_message(GROUP_ID, &ciphertext)
        .expect("bob decrypts")
    {
        DecodedMessage::Application { plaintext, .. } => assert_eq!(plaintext, text),
        _ => panic!("expected an application message"),
    }
}

/// Run one operation and return its result with the commits it made.
fn commits_during<T>(
    commits: &AtomicUsize,
    op: impl FnOnce() -> Result<T, KupferdrahtError>,
) -> (T, usize) {
    commits.store(0, Ordering::SeqCst);
    let out = op().expect("operation");
    (out, commits.load(Ordering::SeqCst))
}

/// A merge that fails after the group state (and with it the pending
/// commit) was written must leave the group at the old epoch with its
/// pending commit — so the retry after the fault reaches the new epoch,
/// in step with the member the Welcome added.
#[test]
fn a_merge_that_fails_midway_leaves_the_old_epoch_and_can_be_retried() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (alice, alice_db) = file_client(dir.path(), "alice.db", ALICE_USER, ALICE_CLIENT);
    let bob = MlsClient::new_fresh(BOB_USER, BOB_CLIENT).expect("bob");
    alice
        .create_group(GROUP_ID, &REMOVAL_KEY)
        .expect("create_group");
    let bundle = alice
        .add_members_commit(GROUP_ID, &[one_key_package(&bob)])
        .expect("add bob");
    let before = alice.group_epoch(GROUP_ID).expect("epoch before");

    inject_fault(&alice_db, "NEW.data_type = 'group_epoch_secrets'");
    assert!(
        alice.merge_pending_commit(GROUP_ID).is_err(),
        "the injected fault fails the merge"
    );
    assert_eq!(
        alice.group_epoch(GROUP_ID).expect("the group still loads"),
        before,
        "a failed merge leaves the group at the old epoch"
    );

    heal(&alice_db);
    alice
        .merge_pending_commit(GROUP_ID)
        .expect("the retry merges the pending commit the failed merge kept");
    assert_eq!(
        alice.group_epoch(GROUP_ID).expect("epoch after"),
        before + 1
    );
    bob.absorb_welcome(&welcome_of(&bundle))
        .expect("bob joins by the welcome");
    assert_bob_reads_alice(&alice, &bob, b"after the retried merge");
}

/// A Welcome whose group cannot be stored must keep the key package it
/// was addressed to — so the retry after the fault joins the group.
#[test]
fn a_welcome_that_fails_midway_keeps_the_key_package_and_can_be_retried() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (bob, bob_db) = file_client(dir.path(), "bob.db", BOB_USER, BOB_CLIENT);
    let alice = MlsClient::new_fresh(ALICE_USER, ALICE_CLIENT).expect("alice");
    alice
        .create_group(GROUP_ID, &REMOVAL_KEY)
        .expect("create_group");
    let bundle = alice
        .add_members_commit(GROUP_ID, &[one_key_package(&bob)])
        .expect("add bob");
    alice.merge_pending_commit(GROUP_ID).expect("merge add");
    let welcome = welcome_of(&bundle);

    inject_fault(&bob_db, "1");
    assert!(
        bob.absorb_welcome(&welcome).is_err(),
        "the injected fault fails the join"
    );

    heal(&bob_db);
    let joined = bob
        .absorb_welcome(&welcome)
        .expect("the retry finds the key package the failed join kept");
    assert_eq!(joined, GROUP_ID);
    assert_eq!(
        bob.group_epoch(GROUP_ID).expect("bob's epoch"),
        alice.group_epoch(GROUP_ID).expect("alice's epoch")
    );
    assert_bob_reads_alice(&alice, &bob, b"after the retried welcome");
}

/// Every mutating operation is one transaction: exactly one commit
/// (one fsync) however many records it writes.
#[test]
fn every_mls_operation_commits_exactly_once() {
    let (alice, alice_commits) = counted_client(ALICE_USER, ALICE_CLIENT);
    let (bob, bob_commits) = counted_client(BOB_USER, BOB_CLIENT);
    let mut counts: Vec<(&str, usize)> = Vec::new();

    let (kps, n) = commits_during(&bob_commits, || bob.generate_key_packages(3));
    counts.push(("generate_key_packages(3)", n));
    let ((), n) = commits_during(&alice_commits, || {
        alice.create_group(GROUP_ID, &REMOVAL_KEY)
    });
    counts.push(("create_group", n));
    let (bundle, n) = commits_during(&alice_commits, || {
        alice.add_members_commit(GROUP_ID, &kps[..1])
    });
    counts.push(("add_members_commit", n));
    let ((), n) = commits_during(&alice_commits, || alice.merge_pending_commit(GROUP_ID));
    counts.push(("merge_pending_commit", n));
    let (_, n) = commits_during(&bob_commits, || bob.absorb_welcome(&welcome_of(&bundle)));
    counts.push(("absorb_welcome", n));
    let (ciphertext, n) = commits_during(&alice_commits, || {
        alice.encrypt_text_message(GROUP_ID, b"counted")
    });
    counts.push(("encrypt_text_message", n));
    let (_, n) = commits_during(&bob_commits, || {
        bob.process_inbound_message(GROUP_ID, &ciphertext)
    });
    counts.push(("process_inbound_message", n));
    let (_, n) = commits_during(&alice_commits, || alice.self_update_commit(GROUP_ID));
    counts.push(("self_update_commit", n));
    let ((), n) = commits_during(&alice_commits, || alice.clear_pending_commit(GROUP_ID));
    counts.push(("clear_pending_commit", n));
    let ((), n) = commits_during(&bob_commits, || bob.discard_group(GROUP_ID));
    counts.push(("discard_group", n));

    let not_once: Vec<_> = counts.iter().filter(|(_, n)| *n != 1).collect();
    assert!(
        not_once.is_empty(),
        "every MLS operation commits exactly once; these did not: {not_once:?}"
    );
}

/// Step 109 (ADR-0037): one MLS window per page, a savepoint per
/// notification. Rolling back to the second notification's savepoint undoes
/// its decryption alone: the first stays processed (a replay of it is
/// stale), the second can be decrypted again — its key was not consumed.
/// The page commits once.
#[test]
fn a_savepoint_undoes_one_notification_of_the_window() {
    let (alice, _) = counted_client(ALICE_USER, ALICE_CLIENT);
    let (bob, commits) = counted_client(BOB_USER, BOB_CLIENT);
    alice
        .create_group(GROUP_ID, &REMOVAL_KEY)
        .expect("alice creates the group");
    let bundle = alice
        .add_members_commit(GROUP_ID, &[one_key_package(&bob)])
        .expect("alice adds bob");
    alice.merge_pending_commit(GROUP_ID).expect("alice merges");
    bob.absorb_welcome(&welcome_of(&bundle)).expect("bob joins");
    let first = alice
        .encrypt_text_message(GROUP_ID, b"first")
        .expect("first");
    let second = alice
        .encrypt_text_message(GROUP_ID, b"second")
        .expect("second");

    commits.store(0, Ordering::SeqCst);
    bob.begin_notification_window().expect("window");
    bob.savepoint_notification(0).expect("savepoint 0");
    bob.process_inbound_message(GROUP_ID, &first)
        .expect("first decrypts");
    bob.savepoint_notification(1).expect("savepoint 1");
    bob.process_inbound_message(GROUP_ID, &second)
        .expect("second decrypts");
    bob.rollback_to_notification(1).expect("rollback to 1");
    bob.commit_notification_window().expect("commit");
    assert_eq!(commits.load(Ordering::SeqCst), 1, "the window commits once");

    assert!(
        !matches!(
            bob.process_inbound_message(GROUP_ID, &first),
            Ok(DecodedMessage::Application { .. })
        ),
        "the first notification stays processed: its key is consumed"
    );
    match bob
        .process_inbound_message(GROUP_ID, &second)
        .expect("the second decrypts again")
    {
        DecodedMessage::Application { plaintext, .. } => assert_eq!(plaintext, b"second"),
        _ => panic!("expected an application message"),
    }
}
