// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! The public surface speaks matrix-rust-sdk's language (ADR-0009, part 4).
//!
//! A lexical check over what a consumer sees: the public Rust items outside
//! `wire/` with their docs (the core, the FFI, the browser exports and their
//! `js_name`s, the custom-store contract), the commands and help of the CLI,
//! and `index.d.ts`. Every word of [`WORDS`] found there must be accounted
//! for: in [`TEMPORARY`], a spot a step still has to say in matrix-rust-sdk's
//! terms; or in [`APPROVED`], a spot kept for an interface to
//! Wire, approved beforehand and marked in the code with a comment that
//! contains [`MARK`]. On a mismatch the test prints the list as it is now.
//! Never add a row to make it pass: rename instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The comment that marks an approved compatibility spot on the next line.
const MARK: &str = "Wire compatibility (ADR-0009)";

/// A backend word as identifier segments (`snake_case` and `camelCase` split
/// alike). Every segment but the last must equal the found one; the last
/// may be its start (`config` finds `configs`).
struct Word {
    name: &'static str,
    segments: &'static [&'static str],
    /// Only a whole identifier in code: the word is plain English too.
    whole_in_code_only: bool,
    /// Segments that make the match another term: `ephemeral` before `key` is
    /// HPKE's ephemeral key (RFC 9180), not Wire's ephemeral message.
    not_before: &'static [&'static str],
    /// The Step 163 sub-step that removes it.
    step: &'static str,
}

const fn word(name: &'static str, segments: &'static [&'static str], step: &'static str) -> Word {
    Word {
        name,
        segments,
        whole_in_code_only: false,
        not_before: &[],
        step,
    }
}

const fn term_word(
    name: &'static str,
    segments: &'static [&'static str],
    not_before: &'static [&'static str],
    step: &'static str,
) -> Word {
    Word {
        name,
        segments,
        whole_in_code_only: false,
        not_before,
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
        not_before: &[],
        step,
    }
}

/// The backend's words: a place that says one of them has yet to speak
/// matrix-rust-sdk's language (ADR-0009, part 4). A standard's term Wire took
/// over is the standard's, not the backend's: MLS's (RFC 9420) and RFC 7638's
/// `thumbprint` stay.
const WORDS: &[Word] = &[
    word("knock", &["knock"], "163b"),
    term_word("ephemeral", &["ephemeral"], &["key"], "163b"),
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

/// Spots a step still has to say in matrix-rust-sdk's terms, `(file, word,
/// count)`, with the step that empties them: Step 163's sub-steps emptied
/// the list. A row is temporary by definition; a spot that stays is an
/// approved one in [`APPROVED`].
const TEMPORARY: &[(&str, &str, usize)] = &[];

/// Spots kept for an interface to Wire, approved by the maintainer and marked
/// with [`MARK`]: `(file, word, count, interface)`.
const APPROVED: &[(&str, &str, usize, &str)] = &[];

// ---- the matcher ------------------------------------------------------

/// Prose without its citations of Wire's interface: a backticked endpoint
/// path (`PUT /v{N}/…`), event type (`conversation.rename`) or status label
/// (`404 no-conversation`) names Wire's REST API and stays (ADR-0009, where
/// compatibility binds). A backticked field name (`client_id`) is no citation;
/// a backticked Rust path (`Self::room`) is code, whose words the code lines
/// carry, and leaves the prose too.
fn without_citations(line: &str) -> String {
    let mut out = String::new();
    for (i, part) in line.split('`').enumerate() {
        let quoted = i % 2 == 1;
        let citation = part.contains('/')
            || part.contains("::")
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

/// `word` starts at `segments[at]`: every segment but the last equal, the
/// last a prefix, and the segment after it none of `not_before`.
fn matches_at(segments: &[String], at: usize, word: &Word) -> bool {
    let n = word.segments.len();
    let Some(window) = segments.get(at..at + n) else {
        return false;
    };
    let found = window
        .iter()
        .zip(word.segments)
        .enumerate()
        .all(|(i, (have, want))| {
            if i + 1 == n {
                have.starts_with(want)
            } else {
                have == want
            }
        });
    found
        && !segments
            .get(at + n)
            .is_some_and(|next| word.not_before.iter().any(|stop| next.starts_with(stop)))
}

/// How often `word` occurs in one identifier of code.
fn code_hits(token: &[String], word: &Word) -> usize {
    if word.whole_in_code_only {
        return usize::from(
            token
                .iter()
                .map(String::as_str)
                .eq(word.segments.iter().copied()),
        );
    }
    (0..token.len())
        .filter(|&at| matches_at(token, at, word))
        .count()
}

/// How often `word` occurs in a line of prose, where its segments may be
/// words of their own (`cookie label`) or one identifier (`cookie_label`).
fn prose_hits(text: &str, word: &Word) -> usize {
    if word.whole_in_code_only {
        return 0;
    }
    let segments: Vec<String> = tokens(text).into_iter().flatten().collect();
    (0..segments.len())
        .filter(|&at| matches_at(&segments, at, word))
        .count()
}

/// How often `word` occurs in `text`, read as code or as prose.
fn hits(text: &str, word: &Word, prose: bool) -> usize {
    if prose {
        prose_hits(text, word)
    } else {
        tokens(text)
            .iter()
            .map(|token| code_hits(token, word))
            .sum()
    }
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
        for word in WORDS {
            let found = hits(&text, word, line.prose);
            if found == 0 {
                continue;
            }
            let into = if line.marked {
                &mut *marked
            } else {
                &mut *loose
            };
            *into
                .entry((file.to_owned(), word.name.to_owned()))
                .or_default() += found;
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
fn the_public_surface_speaks_matrix_terms_but_for_the_listed_spots() {
    let (loose, marked) = surface_tally();
    let temporary = listed(TEMPORARY.iter().copied());
    assert!(
        loose == temporary,
        "The backend's words on the public surface differ from TEMPORARY (ADR-0009). \
         Say it in matrix-rust-sdk's terms instead of listing; a row goes when its \
         sub-step lands. Now:\n{}",
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
        hits(text, w, prose) > 0
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
    assert!(!prose("/// as [`Self::room`] answers it").contains("room"));
    assert!(prose("/// the conversation's members").contains("conversation"));
}

#[test]
fn the_temporary_list_is_empty_after_step_163() {
    assert!(
        TEMPORARY.is_empty(),
        "{} spots still to say in matrix-rust-sdk's terms:\n{}",
        TEMPORARY.len(),
        rows(&listed(TEMPORARY.iter().copied()))
    );
}

/// An HPKE "ephemeral key" is the cryptographic term (RFC 9180), not Wire's
/// ephemeral message: the word followed by `key` is no hit, in code or prose.
#[test]
fn an_ephemeral_key_is_the_cryptographic_term() {
    let hit = |text: &str, prose: bool| {
        let w = WORDS
            .iter()
            .find(|w| w.name == "ephemeral")
            .expect("a listed word");
        hits(text, w, prose) > 0
    };
    assert!(hit(
        "/// an ephemeral message disappears after its TTL",
        true
    ));
    assert!(hit("pub fn send_ephemeral_text(&self)", false));
    assert!(!hit("/// the HPKE ephemeral key of the Welcome", true));
    assert!(!hit("pub ephemeral_key: Vec<u8>", false));
    assert!(!hit("ephemeralKey: Uint8Array", false));
}

/// A word of several segments is found in prose when it is written with a
/// space, as a doc comment or a help text writes it.
#[test]
fn a_word_written_with_a_space_in_prose_is_found() {
    let hit = |text: &str, name: &str| {
        let w = WORDS
            .iter()
            .find(|w| w.name == name)
            .expect("a listed word");
        prose_hits(text, w) > 0
    };
    assert!(hit("/// the cookie label of a login", "cookie label"));
    assert!(hit(
        "/// each feature config as the backend sent it",
        "feature config"
    ));
    assert!(hit(
        "/// the backend MLS key of the domain",
        "backend mls key"
    ));
    assert!(!hit("/// the label of the cookie", "cookie label"));
    assert!(!hit("/// a config of the feature", "feature config"));
    assert!(!hit("/// an HPKE ephemeral key", "ephemeral"));
}

#[test]
fn adr_0009_states_the_line_and_its_check() {
    let adr =
        std::fs::read_to_string(workspace().join("docs/adrs/0009-matrix-rust-sdk-as-api-model.md"))
            .expect("ADR-0009");
    assert!(
        adr.contains("### 4. The public surface speaks matrix-rust-sdk's language"),
        "ADR-0009 part 4"
    );
    assert!(adr.contains("| 1.2 |"), "ADR-0009 revision 1.2");
    assert!(adr.contains("| 1.3 |"), "ADR-0009 revision 1.3");
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
