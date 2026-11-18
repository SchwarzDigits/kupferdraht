// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 163b — the message kinds in plain names (TDD red).
//!
//! The words themselves are held by `public_vocabulary`. These pins hold what
//! a lexical check cannot see: the description of an attachment a consumer
//! reads carries no locator, token or cipher of the server's, whatever such a
//! field would be called. The download goes through the attachment itself, as
//! the browser's view already has it.

use std::path::{Path, PathBuf};

/// The public fields of an attachment's description, in order.
const DESCRIPTION_FIELDS: [&str; 7] = [
    "message_id",
    "mime_type",
    "filename",
    "size_bytes",
    "sender_user_id",
    "room_id",
    "cache_path",
];

/// Every `.rs` file under `dir`, recursively.
fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The `pub` fields of `pub struct <name> { … }`, wherever under `src/` it is
/// declared.
fn public_fields(name: &str) -> Vec<String> {
    let opener = format!("pub struct {name} {{");
    let mut files = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let source = files
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("read source"))
        .find(|source| source.contains(&opener))
        .unwrap_or_else(|| panic!("no `{opener}` under src/"));
    let body = &source[source.find(&opener).expect("opener")..];
    let body = &body[..body.find("\n}").expect("struct closes")];
    body.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("pub "))
        .filter_map(|line| line.split_once(':').map(|(field, _)| field.to_owned()))
        .collect()
}

#[test]
fn the_bindings_attachment_description_carries_no_locator() {
    assert_eq!(public_fields("AttachmentMeta"), DESCRIPTION_FIELDS);
}

#[test]
fn the_rust_attachment_summary_carries_no_locator() {
    assert_eq!(public_fields("AttachmentSummary"), DESCRIPTION_FIELDS);
}
