// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! The public surface carries no Wire vocabulary (ADR-0009, revision 1.2).
//!
//! A lexical check over what a consumer sees: the public Rust items outside
//! `wire/` with their docs (the core, the FFI, the browser exports and their
//! `js_name`s, the custom-store contract), the commands and help of the CLI,
//! and `index.d.ts`. Every word of [`WORDS`] found there must be accounted
//! for: in [`TEMPORARY`], today's occurrences, which the Step 163 sub-step
//! of the word removes; or in [`APPROVED`], a spot kept for an interface to
//! Wire, approved beforehand and marked in the code with a comment that
//! contains [`MARK`]. On a mismatch the test prints the list as it is now.
//! Never add a row to make it pass: rename instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The comment that marks an approved compatibility spot on the next line.
const MARK: &str = "Wire compatibility (ADR-0009)";

/// A Wire word as identifier segments (`snake_case` and `camelCase` split
/// alike). Every segment but the last must equal the found one; the last
/// may be its start (`config` finds `configs`).
struct Word {
    name: &'static str,
    segments: &'static [&'static str],
    /// Only a whole identifier in code: the word is plain English too.
    whole_in_code_only: bool,
    /// The Step 163 sub-step that removes it.
    step: &'static str,
}

const fn word(name: &'static str, segments: &'static [&'static str], step: &'static str) -> Word {
    Word {
        name,
        segments,
        whole_in_code_only: false,
        step,
    }
}

const fn code_word(
    name: &'static str,
    segments: &'static [&'static str],
    step: &'static str,
) -> Word {
    Word {
        name,
        segments,
        whole_in_code_only: true,
        step,
    }
}

/// Wire's words the public surface must not carry (ADR-0009, part 4). A
/// standard's term is no Wire word where Wire took it over: MLS's (RFC 9420)
/// and RFC 7638's `thumbprint` stay.
const WORDS: &[Word] = &[
    word("knock", &["knock"], "163b"),
    word("ephemeral", &["ephemeral"], "163b"),
    word("asset", &["asset"], "163b"),
    word("otr", &["otr"], "163b"),
    word("encryption algo", &["encryption", "algo"], "163b"),
    word("access role", &["access", "role"], "163c"),
    word("access mode", &["access", "mode"], "163c"),
    word("self room", &["self", "room"], "163c"),
    word("qualified", &["qualified"], "163d"),
    word("cookie label", &["cookie", "label"], "163d"),
    code_word("handle", &["handle"], "163d"),
    code_word("set handle", &["set", "handle"], "163d"),
    word("is self", &["is", "self"], "163d"),
    word("avatar key", &["avatar", "key"], "163d"),
    word("thumbnail key", &["thumbnail", "key"], "163d"),
    word("full key", &["full", "key"], "163d"),
    word("removal key", &["removal", "key"], "163d"),
    word("device label", &["device", "label"], "163d"),
    word("client id", &["client", "id"], "163d"),
    word("zuid", &["zuid"], "163d"),
    word("feature config", &["feature", "config"], "163e"),
    word("user property", &["user", "propert"], "163e"),
    word("receipt mode", &["receipt", "mode"], "163e"),
    word("sync marker", &["sync", "marker"], "163e"),
    word("confirmer", &["confirmer"], "163e"),
    word("backend mls key", &["backend", "mls", "key"], "163e"),
    word("self user", &["self", "user"], "163e"),
    word("conversation", &["conversation"], "163e"),
];

/// Today's occurrences, `(file, word, count)`. Each Step 163 sub-step removes
/// the rows of its words; after 163e the list is empty.
const TEMPORARY: &[(&str, &str, usize)] = &[
    // 163e
    (
        "bindings/web/kupferdraht-tabs/index.d.ts",
        "conversation",
        1,
    ),
    (
        "bindings/web/kupferdraht-tabs/index.d.ts",
        "feature config",
        3,
    ),
    ("kupferdraht-cli/src/main.rs", "conversation", 12),
    ("kupferdraht-cli/src/main.rs", "feature config", 1),
    ("kupferdraht-cli/src/main.rs", "receipt mode", 1),
    ("kupferdraht-core/src/client/builder.rs", "conversation", 1),
    ("kupferdraht-core/src/client/builder.rs", "self user", 1),
    (
        "kupferdraht-core/src/client/membership.rs",
        "conversation",
        1,
    ),
    ("kupferdraht-core/src/client/mod.rs", "conversation", 1),
    ("kupferdraht-core/src/client/mod.rs", "receipt mode", 1),
    ("kupferdraht-core/src/client/rooms.rs", "conversation", 23),
    (
        "kupferdraht-core/src/client/server_configs.rs",
        "feature config",
        6,
    ),
    (
        "kupferdraht-core/src/client/server_configs.rs",
        "receipt mode",
        8,
    ),
    (
        "kupferdraht-core/src/client/server_configs.rs",
        "user property",
        5,
    ),
    ("kupferdraht-core/src/client/sync.rs", "conversation", 3),
    ("kupferdraht-core/src/error.rs", "conversation", 3),
    ("kupferdraht-core/src/ffi/admin.rs", "feature config", 3),
    ("kupferdraht-core/src/ffi/admin.rs", "receipt mode", 2),
    ("kupferdraht-core/src/ffi/admin.rs", "user property", 3),
    ("kupferdraht-core/src/ffi/room.rs", "conversation", 9),
    ("kupferdraht-core/src/ffi/sync.rs", "conversation", 1),
    ("kupferdraht-core/src/rooms/room.rs", "conversation", 16),
    (
        "kupferdraht-core/src/storage/records.rs",
        "backend mls key",
        6,
    ),
    ("kupferdraht-core/src/storage/records.rs", "confirmer", 4),
    (
        "kupferdraht-core/src/storage/records.rs",
        "feature config",
        15,
    ),
    ("kupferdraht-core/src/storage/records.rs", "receipt mode", 2),
    ("kupferdraht-core/src/storage/records.rs", "self user", 8),
    ("kupferdraht-core/src/storage/records.rs", "sync marker", 1),
    (
        "kupferdraht-core/src/storage/records.rs",
        "user property",
        18,
    ),
    (
        "kupferdraht-core/src/sync/event_handler.rs",
        "conversation",
        1,
    ),
    (
        "kupferdraht-core/src/timeline/confirmation.rs",
        "receipt mode",
        2,
    ),
    (
        "kupferdraht-core/src/timeline/task/send.rs",
        "conversation",
        1,
    ),
    (
        "kupferdraht-core/src/timeline/task/types.rs",
        "conversation",
        1,
    ),
    ("kupferdraht-core/src/wasm/manager.rs", "conversation", 3),
    ("kupferdraht-core/src/wasm/manager.rs", "feature config", 2),
];

/// Spots kept for an interface to Wire, approved by the maintainer and marked
/// with [`MARK`]: `(file, word, count, interface)`.
const APPROVED: &[(&str, &str, usize, &str)] = &[];

// ---- the matcher ------------------------------------------------------

/// Prose without its citations of Wire's interface: a backticked endpoint
/// path (`PUT /v{N}/…`), event type (`conversation.rename`) or status label
/// (`404 no-conversation`) names Wire's REST API and stays (ADR-0009, where
/// compatibility binds). A backticked field name (`client_id`) is no citation.
fn without_citations(line: &str) -> String {
    let mut out = String::new();
    for (i, part) in line.split('`').enumerate() {
        let quoted = i % 2 == 1;
        let citation = part.contains('/')
            || part.starts_with(|c: char| c.is_ascii_digit())
            || (part.contains('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '-' || c == '_'));
        if !(quoted && citation) {
            out.push_str(part);
            out.push(' ');
        }
    }
    out
}

/// The identifiers of `line`, each split into lowercase segments.
fn tokens(line: &str) -> Vec<Vec<String>> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(segments)
        .collect()
}

/// `snake_case`, `camelCase` and `SCREAMING_CASE` split into lowercase parts.
fn segments(token: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in token.split('_').filter(|p| !p.is_empty()) {
        let mut current = String::new();
        let mut previous_lower = false;
        for c in piece.chars() {
            if c.is_ascii_uppercase() && previous_lower {
                parts.push(std::mem::take(&mut current));
            }
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
            current.push(c.to_ascii_lowercase());
        }
        parts.push(current);
    }
    parts
}

fn found_in(token: &[String], word: &Word, prose: bool) -> bool {
    if word.whole_in_code_only {
        return !prose
            && token
                .iter()
                .map(String::as_str)
                .eq(word.segments.iter().copied());
    }
    let n = word.segments.len();
    token.windows(n).any(|window| {
        window
            .iter()
            .zip(word.segments)
            .enumerate()
            .all(|(i, (have, want))| {
                if i + 1 == n {
                    have.starts_with(want)
                } else {
                    have == want
                }
            })
    })
}

// ---- what a consumer sees ---------------------------------------------

/// A line of the surface: code, or prose (a doc comment, JSDoc, help text).
struct Line<'a> {
    text: &'a str,
    prose: bool,
    marked: bool,
}

fn is_comment(trimmed: &str) -> bool {
    trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*')
}

fn braces(text: &str) -> i32 {
    let open = text.matches('{').count();
    let close = text.matches('}').count();
    i32::try_from(open).expect("few braces") - i32::try_from(close).expect("few braces")
}

/// The public items of a Rust file: `pub` lines with the docs and
/// attributes before them, and whole bodies of `pub enum` and `pub trait`.
/// Stops at the test module.
fn rust_surface(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    let mut marked = false;
    let mut body_depth = 0;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|n| n.trim_start().starts_with("mod "))
        {
            break;
        }
        if t.contains(MARK) {
            marked = true;
            continue;
        }
        if body_depth > 0 {
            body_depth += braces(t);
            out.push(Line {
                text: line,
                prose: is_comment(t),
                marked: std::mem::take(&mut marked) && !is_comment(t),
            });
            continue;
        }
        if t.starts_with("///") || t.starts_with("#[") {
            pending.push(line);
            continue;
        }
        if t.starts_with("pub ") {
            for p in pending.drain(..) {
                out.push(Line {
                    text: p,
                    prose: p.trim_start().starts_with("///"),
                    marked: false,
                });
            }
            out.push(Line {
                text: line,
                prose: false,
                marked: std::mem::take(&mut marked),
            });
            if (t.contains(" enum ") || t.contains(" trait ")) && braces(t) > 0 {
                body_depth = braces(t);
            }
        } else if !t.starts_with("//") {
            pending.clear();
            marked = false;
        }
    }
    out
}

/// The CLI's commands and flags: every top-level `enum` and `struct` body
/// of `main.rs` with its help text. Stops at the test module.
fn cli_surface(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut marked = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t == "#[cfg(test)]" {
            break;
        }
        if t.contains(MARK) {
            marked = true;
            continue;
        }
        let opens = depth == 0
            && (t.starts_with("enum ")
                || t.starts_with("struct ")
                || t.starts_with("pub enum ")
                || t.starts_with("pub struct "));
        if depth > 0 || opens {
            depth += braces(t);
            let prose = is_comment(t);
            out.push(Line {
                text: line,
                prose,
                marked: !prose && std::mem::take(&mut marked),
            });
        }
    }
    out
}

/// Every line of `index.d.ts`; JSDoc and comments are prose.
fn typescript_surface(text: &str) -> Vec<Line<'_>> {
    let mut marked = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.contains(MARK) {
            marked = true;
            continue;
        }
        let prose = is_comment(t);
        out.push(Line {
            text: line,
            prose,
            marked: !prose && std::mem::take(&mut marked),
        });
    }
    out
}

// ---- the tally --------------------------------------------------------

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("a directory entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() && name != "wire" {
            rust_files(&path, found);
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
            found.push(path);
        }
    }
}

/// `(file, word) → count`, apart for unmarked and marked spots.
type Tally = BTreeMap<(String, String), usize>;

fn count(file: &str, lines: &[Line<'_>], loose: &mut Tally, marked: &mut Tally) {
    for line in lines {
        let text = if line.prose {
            without_citations(line.text)
        } else {
            line.text.to_owned()
        };
        for token in tokens(&text) {
            for word in WORDS.iter().filter(|w| found_in(&token, w, line.prose)) {
                let into = if line.marked {
                    &mut *marked
                } else {
                    &mut *loose
                };
                *into
                    .entry((file.to_owned(), word.name.to_owned()))
                    .or_default() += 1;
            }
        }
    }
}

fn surface_tally() -> (Tally, Tally) {
    let root = workspace();
    let read = |rel: &str| {
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    };
    let (mut loose, mut marked) = (Tally::new(), Tally::new());
    let mut files = Vec::new();
    rust_files(&root.join("kupferdraht-core/src"), &mut files);
    files.sort();
    for path in files {
        let rel = path
            .strip_prefix(&root)
            .expect("under the workspace")
            .to_string_lossy()
            .replace('\\', "/");
        let text = read(&rel);
        count(&rel, &rust_surface(&text), &mut loose, &mut marked);
    }
    let cli = "kupferdraht-cli/src/main.rs";
    count(cli, &cli_surface(&read(cli)), &mut loose, &mut marked);
    let dts = "bindings/web/kupferdraht-tabs/index.d.ts";
    count(
        dts,
        &typescript_surface(&read(dts)),
        &mut loose,
        &mut marked,
    );
    (loose, marked)
}

fn listed<'a>(rows: impl Iterator<Item = (&'a str, &'a str, usize)>) -> Tally {
    rows.map(|(file, word, n)| ((file.to_owned(), word.to_owned()), n))
        .collect()
}

fn rows(tally: &Tally) -> String {
    tally
        .iter()
        .map(|((file, word), n)| format!("    ({file:?}, {word:?}, {n}),\n"))
        .collect()
}

// ---- the tests --------------------------------------------------------

#[test]
fn the_public_surface_carries_only_the_listed_wire_words() {
    let (loose, marked) = surface_tally();
    let temporary = listed(TEMPORARY.iter().copied());
    assert!(
        loose == temporary,
        "Wire words on the public surface differ from TEMPORARY (ADR-0009). \
         Rename instead of listing; a row goes when its sub-step lands. Now:\n{}",
        rows(&loose)
    );
    let approved = listed(APPROVED.iter().map(|&(f, w, n, _)| (f, w, n)));
    assert!(
        marked == approved,
        "marked compatibility spots differ from APPROVED; a spot needs the \
         maintainer's approval and a `{MARK}` comment. Marked now:\n{}",
        rows(&marked)
    );
}

#[test]
fn every_listed_word_is_one_of_the_words() {
    for (file, name, _) in TEMPORARY {
        assert!(
            WORDS.iter().any(|w| w.name == *name),
            "{file}: unknown word {name:?}"
        );
    }
    for (file, name, _, interface) in APPROVED {
        assert!(
            WORDS.iter().any(|w| w.name == *name),
            "{file}: unknown word {name:?}"
        );
        assert!(
            !interface.is_empty(),
            "{file}: an approved spot names its interface"
        );
    }
    for w in WORDS {
        assert!(
            ["163b", "163c", "163d", "163e"].contains(&w.step),
            "{}: {}",
            w.name,
            w.step
        );
    }
}

#[test]
fn the_matcher_finds_wire_words_and_leaves_plain_english_alone() {
    let hit = |text: &str, name: &str, prose: bool| {
        let w = WORDS
            .iter()
            .find(|w| w.name == name)
            .expect("a listed word");
        tokens(text).iter().any(|t| found_in(t, w, prose))
    };
    assert!(hit("pub struct AssetSummary", "asset", false));
    assert!(hit("downloadAsset(roomId)", "asset", false));
    assert!(hit(
        "pub fn feature_configs(&self)",
        "feature config",
        false
    ));
    assert!(hit("WIRE_RECEIPT_MODE", "receipt mode", false));
    assert!(hit("pub is_self: bool", "is self", false));
    assert!(hit("isSelf: boolean", "is self", false));
    assert!(hit("pub fn set_handle(&self)", "set handle", false));
    assert!(hit("pub handle: Option<String>", "handle", false));
    assert!(hit("BackendMlsKey", "backend mls key", false));
    assert!(hit("/// the conversation's members", "conversation", true));
    assert!(!hit("pub struct MessageHandle", "handle", false));
    assert!(!hit("/// the returned handle", "handle", true));
    assert!(!hit(
        "pub fn typing_notice(&self, typing: bool)",
        "is self",
        false
    ));
    assert!(!hit("pub fn assert_ready()", "asset", false));
}

#[test]
fn a_citation_of_wires_interface_is_no_wire_word_in_prose() {
    let prose = |text: &str| without_citations(text);
    assert!(
        !prose("/// `PUT /v{N}/conversations/{domain}/{id}/name` renames")
            .contains("conversations")
    );
    assert!(!prose("/// Wire emits a `conversation.rename` event").contains("conversation.rename"));
    assert!(!prose("/// or `404 no-conversation` (unknown room)").contains("no-conversation"));
    assert!(prose("/// by its Wire `client_id`").contains("client_id"));
    assert!(prose("/// the conversation's members").contains("conversation"));
}

#[test]
fn adr_0009_states_the_line_and_its_check() {
    let adr =
        std::fs::read_to_string(workspace().join("docs/adrs/0009-matrix-rust-sdk-as-api-model.md"))
            .expect("ADR-0009");
    assert!(
        adr.contains("### 4. No Wire vocabulary in the public surface"),
        "ADR-0009 part 4"
    );
    assert!(adr.contains("| 1.2 |"), "ADR-0009 revision 1.2");
    let rule =
        std::fs::read_to_string(workspace().join("rules/0009-matrix-rust-sdk-as-api-model.md"))
            .expect("rules/0009");
    assert!(
        rule.contains("public_vocabulary.rs"),
        "the rule names the check"
    );
    assert!(rule.contains(MARK), "the rule names the mark");
}

#[test]
fn the_adrs_that_fixed_wire_names_point_to_the_line() {
    for adr in [
        "0004-integrate-external-senders-extension.md",
        "0011-asset-content-protocol.md",
        "0012-server-feature-configs.md",
        "0025-scope-keystore-ids-and-cookie-label-per-home.md",
    ] {
        let text =
            std::fs::read_to_string(workspace().join("docs/adrs").join(adr)).expect("the ADR");
        let history = text.split("## Revision History").nth(1).unwrap_or_default();
        assert!(
            history.contains("ADR-0009 (revision 1.2)"),
            "{adr}: a revision entry naming ADR-0009 1.2"
        );
    }
}
