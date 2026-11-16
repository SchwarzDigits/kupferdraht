// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 159 — every answer read into memory goes through the answer bound.
//! TDD red.
//!
//! An awaited `.json()`, `.text()` or `.bytes()` on a response anywhere in
//! `kupferdraht-core/src` but the bound's own module reads a body without a
//! bound, which a compressed answer makes cheap to inflate. Test code is not
//! scanned: files of tests only (`*_tests.rs`, `test_support.rs`) and a
//! file's trailing `mod tests` block.
//!
//! An asset blob is no answer: the browser's whole-asset download (W7) reads
//! an encrypted file, which may be larger than any answer.

use std::path::{Path, PathBuf};

/// The one module that reads an answer's body.
const THE_BOUND: &str = "answer_bound.rs";

/// Body reads a `reqwest::Response` offers; `str::bytes` is never awaited.
const READS: [&str; 3] = [".json", ".text", ".bytes"];

/// Functions whose read is no answer.
const NOT_ANSWERS: [&str; 1] = ["download_asset_bytes"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn is_test_only(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    name.ends_with("_tests.rs") || name == "test_support.rs"
}

/// `text` without its trailing `mod tests { … }` block.
fn shipped_part(text: &str) -> &str {
    text.find("\nmod tests {").map_or(text, |at| &text[..at])
}

/// The name of the function `at` lies in: the last `fn` before it.
fn enclosing_fn(text: &str, at: usize) -> &str {
    text[..at].rfind("fn ").map_or("", |start| {
        let name = &text[start + 3..at];
        let end = name
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(name.len());
        &name[..end]
    })
}

/// The line and the function of each awaited body read in `text`.
fn awaited_body_reads(text: &str) -> Vec<(usize, &str)> {
    let mut reads = Vec::new();
    for read in READS {
        for (at, _) in text.match_indices(read) {
            let rest = &text[at + read.len()..];
            let rest = match rest.strip_prefix("::<") {
                Some(turbofish) => turbofish
                    .find(">()")
                    .map_or("", |end| &turbofish[end + 1..]),
                None => rest,
            };
            let Some(rest) = rest.strip_prefix("()") else {
                continue;
            };
            if rest.trim_start().starts_with(".await") {
                reads.push((text[..at].matches('\n').count() + 1, enclosing_fn(text, at)));
            }
        }
    }
    reads.sort_unstable();
    reads
}

#[test]
fn every_answer_is_read_through_the_bound() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.len() > 50,
        "the scan reaches the sources: {}",
        files.len()
    );
    let mut unbounded = Vec::new();
    for file in files {
        if is_test_only(&file) || file.file_name().is_some_and(|n| n == THE_BOUND) {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("read source");
        for (line, function) in awaited_body_reads(shipped_part(&text)) {
            if NOT_ANSWERS.contains(&function) {
                continue;
            }
            let shown = file
                .strip_prefix(&src)
                .unwrap_or(&file)
                .display()
                .to_string();
            unbounded.push(format!("{shown}:{line} ({function})"));
        }
    }
    assert!(
        unbounded.is_empty(),
        "answers read past the bound ({}):\n{}",
        unbounded.len(),
        unbounded.join("\n")
    );
}

#[test]
fn the_scan_finds_each_form_of_a_body_read() {
    let sample = "fn a() {\nresp.json::<Vec<T>>()\n    .await;\n}\nasync fn b_2() {\nresp.text().await;\nresp\n    .bytes()\n    .await;\nid.bytes().all(f);\n}\n";
    assert_eq!(
        awaited_body_reads(sample),
        vec![(2, "a"), (6, "b_2"), (8, "b_2")]
    );
}
