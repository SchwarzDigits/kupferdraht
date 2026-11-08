// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The third-party attribution files against their sources (Step 150).
//!
//! `THIRD-PARTY-NOTICES` is written by hand and `docs/THIRD_PARTY_LICENSES.html`
//! is generated from a template with a hand-kept block. These tests compare
//! both with what they describe: the crate lists with the locked build graph
//! both ways, each crate's section with its license, the target markers with
//! the graphs per target, and the components outside the graph with
//! `PROVENANCE.txt`, `Cargo.lock` and the Gradle files. The graph comes from
//! `cargo tree --offline --locked`, so it needs every package in the local
//! registry, as `step_90_sqlite3mc_engine` does: `cargo fetch --locked`.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The targets the native builds cover: desktop, Android, iOS.
const NATIVE_TARGETS: [&str; 12] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "x86_64-linux-android",
    "i686-linux-android",
    "aarch64-apple-ios",
    "aarch64-apple-ios-sim",
    "x86_64-apple-ios",
];

/// In the notices' graph but not in the HTML report, each for a reason:
/// cargo-about skips the two unpublished crates, whose block `about.hbs`
/// keeps by hand, and a `cfg(any())` dependency, which no target compiles.
const NOT_IN_THE_REPORT: [&str; 3] = [
    "sqlite-remote-protocol",
    "sqlite-remote-vfs",
    "zerocopy-derive",
];

type CrateVersion = (String, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    EveryTarget,
    BrowserOnly,
    NativeOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    version: String,
    section: String,
    marker: Marker,
}

struct Sources<'a> {
    notices: &'a str,
    about_hbs: &'a str,
    provenance: &'a str,
    lockfile: &'a str,
    amalgamation: &'a str,
}

struct Graphs {
    /// Every crate version of the notices' graph, with its license expression.
    all: BTreeMap<CrateVersion, String>,
    browser: BTreeSet<CrateVersion>,
    native: BTreeSet<CrateVersion>,
}

// --- reading the notices -------------------------------------------------

const OUTSIDE_THE_GRAPH: &str = "Components with notices outside the crate graph";

fn is_underline(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '-' || c == '=')
}

/// Heading and body of each section that lists crates.
fn crate_sections(notices: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = notices.lines().collect();
    let headings: Vec<usize> = (0..lines.len().saturating_sub(1))
        .filter(|&index| !lines[index].is_empty() && is_underline(lines[index + 1]))
        .collect();
    let mut sections = Vec::new();
    for (position, &start) in headings.iter().enumerate() {
        let heading = lines[start];
        if heading.starts_with("Components ") && heading != OUTSIDE_THE_GRAPH {
            let end = headings.get(position + 1).copied().unwrap_or(lines.len());
            sections.push((heading.to_owned(), lines[start + 2..end].join("\n")));
        }
    }
    sections
}

/// The bullets of a section body, each on one line.
fn bullets(body: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current: Option<String> = None;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("- ") {
            items.extend(current.replace(rest.to_owned()));
        } else if let (Some(item), true) = (current.as_mut(), line.starts_with("  ")) {
            item.push(' ');
            item.push_str(line.trim());
        } else {
            items.extend(current.take());
        }
    }
    items.extend(current);
    items
}

fn parse_crate(text: &str, section: &str) -> Entry {
    let mut words = text.split_whitespace();
    let name = words.next().expect("a crate name");
    let version = words
        .next()
        .unwrap_or_else(|| panic!("no version in `{text}`"));
    let marker = match words.collect::<Vec<_>>().join(" ").as_str() {
        "" => Marker::EveryTarget,
        "(browser target only)" => Marker::BrowserOnly,
        "(native targets only)" => Marker::NativeOnly,
        other => panic!("unknown marker `{other}` in `{text}`"),
    };
    entry(name, version, section, marker)
}

fn notice_entries(notices: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (heading, body) in crate_sections(notices) {
        for item in bullets(&body) {
            // What follows the first dash is authors and a URL.
            let crates = item.split(" — ").next().expect("split yields a first part");
            entries.extend(crates.split(", ").map(|text| parse_crate(text, &heading)));
        }
    }
    entries
}

// --- license expressions -------------------------------------------------

/// `expression` split at `separator` outside parentheses.
fn split_top_level<'a>(expression: &'a str, separator: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0_u32, 0);
    for (index, character) in expression.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && index >= start && expression[index..].starts_with(separator) {
            parts.push(&expression[start..index]);
            start = index + separator.len();
        }
    }
    parts.push(&expression[start..]);
    parts
}

fn canonical_term(term: &str) -> String {
    let term = term.trim();
    let inner = term
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(term);
    let mut choices: Vec<&str> = split_top_level(inner, " OR ")
        .into_iter()
        .map(str::trim)
        .collect();
    choices.sort_unstable();
    if choices.len() == 1 {
        choices[0].to_owned()
    } else {
        format!("({})", choices.join(" OR "))
    }
}

/// One spelling per expression: `/` read as `OR`, the operands sorted.
fn canonical_license(expression: &str) -> String {
    let spelled = expression.replace('/', " OR ");
    let mut terms: Vec<String> = split_top_level(&spelled, " AND ")
        .into_iter()
        .map(canonical_term)
        .collect();
    terms.sort_unstable();
    terms.join(" AND ")
}

fn section_license(heading: &str) -> String {
    const NAMED: [(&str, &str); 8] = [
        ("the MIT License", "MIT"),
        ("the Apache License 2.0", "Apache-2.0"),
        ("the Unicode License v3", "Unicode-3.0"),
        ("the Mozilla Public License 2.0", "MPL-2.0"),
        ("the BSD 3-Clause License", "BSD-3-Clause"),
        ("the ISC License", "ISC"),
        ("the CDLA Permissive 2.0", "CDLA-Permissive-2.0"),
        ("the zlib License", "Zlib"),
    ];
    let named = heading
        .strip_prefix("Components dual-licensed ")
        .or_else(|| heading.strip_prefix("Components licensed under "))
        .unwrap_or_else(|| panic!("a crate section's heading names a license: {heading}"));
    let expression = NAMED
        .iter()
        .find(|(prose, _)| *prose == named)
        .map_or(named, |(_, spdx)| spdx);
    canonical_license(expression)
}

// --- the comparisons -----------------------------------------------------

fn graph_differences(entries: &[Entry], graph: &BTreeSet<CrateVersion>) -> Vec<String> {
    let listed: BTreeSet<CrateVersion> = entries
        .iter()
        .map(|entry| crate_version(&entry.name, &entry.version))
        .collect();
    let stale = listed
        .difference(graph)
        .map(|(name, version)| format!("{name} {version}: in the notices, not in the graph"));
    let missing = graph
        .difference(&listed)
        .map(|(name, version)| format!("{name} {version}: in the graph, not in the notices"));
    stale.chain(missing).collect()
}

fn license_differences(
    entries: &[Entry],
    licenses: &BTreeMap<CrateVersion, String>,
) -> Vec<String> {
    let mut differences = Vec::new();
    for entry in entries {
        let key = crate_version(&entry.name, &entry.version);
        if let Some(license) = licenses.get(&key)
            && canonical_license(license) != section_license(&entry.section)
        {
            differences.push(format!(
                "{} {}: licensed `{license}`, listed under `{}`",
                entry.name, entry.version, entry.section
            ));
        }
    }
    differences
}

fn sections_with_an_unstated_choice(notices: &str) -> Vec<String> {
    crate_sections(notices)
        .into_iter()
        .filter(|(heading, body)| {
            section_license(heading).contains(" OR ") && !flat(body).contains("used here under")
        })
        .map(|(heading, _)| format!("{heading}: does not say which license is used"))
        .collect()
}

/// Whether `marker` fits a crate's normal edges. A crate in neither graph is
/// a build tool or behind a `cfg` no build sets: in no artifact, so its marker
/// says nothing a graph could contradict.
fn marker_fits(marker: Marker, in_browser: bool, in_native: bool) -> bool {
    match (in_browser, in_native) {
        (true, true) => marker == Marker::EveryTarget,
        (true, false) => marker == Marker::BrowserOnly,
        (false, true) => marker == Marker::NativeOnly,
        (false, false) => true,
    }
}

fn marker_differences(
    entries: &[Entry],
    browser: &BTreeSet<CrateVersion>,
    native: &BTreeSet<CrateVersion>,
) -> Vec<String> {
    let mut differences = Vec::new();
    for entry in entries {
        let key = crate_version(&entry.name, &entry.version);
        let (in_browser, in_native) = (browser.contains(&key), native.contains(&key));
        if !marker_fits(entry.marker, in_browser, in_native) {
            differences.push(format!(
                "{} {}: marked {:?}; in the browser graph: {in_browser}, in a native graph: {in_native}",
                entry.name, entry.version, entry.marker
            ));
        }
    }
    differences
}

/// `text` with its line breaks and indentation gone and the template's
/// escapes read, so a claim is found however it wraps.
fn flat(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// The value of a `key: value` line in `PROVENANCE.txt`.
fn provenance_value<'a>(provenance: &'a str, key: &str) -> &'a str {
    provenance
        .lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
        .unwrap_or_else(|| panic!("`{key}:` missing in PROVENANCE.txt"))
}

fn locked_version<'a>(lockfile: &'a str, package: &str) -> &'a str {
    lockfile
        .split_once(&format!("name = \"{package}\"\nversion = \""))
        .and_then(|(_, rest)| rest.split('"').next())
        .unwrap_or_else(|| panic!("{package} is not locked"))
}

/// The components `embedded-licenses:` names, without their licenses.
fn embedded_components(provenance: &str) -> Vec<String> {
    let mut components = Vec::new();
    for group in provenance_value(provenance, "embedded-licenses").split("; ") {
        let (_, named) = group.split_once('(').expect("license (components)");
        for component in named.trim_end_matches(')').split(", ") {
            for name in component.split(" and ") {
                components.push(name.trim_start_matches("a part of ").to_owned());
            }
        }
    }
    components
}

/// SHA-2's BSD notice as the amalgamation carries it, which a binary
/// redistribution has to reproduce.
fn sha2_notice(amalgamation: &str) -> String {
    let notice: Vec<&str> = amalgamation
        .lines()
        .skip_while(|line| !line.contains("Olivier Gay"))
        .take_while(|line| !line.trim_start().starts_with("*/"))
        .map(|line| line.trim_start().trim_start_matches('*'))
        .collect();
    flat(&notice.join("\n"))
}

fn outside_graph_differences(sources: &Sources<'_>) -> Vec<String> {
    let mut claims = vec![
        format!(
            "SQLite3 Multiple Ciphers {}",
            provenance_value(sources.provenance, "sqlite3mc-version")
        ),
        format!(
            "SQLite {}",
            provenance_value(sources.provenance, "sqlite-version")
        ),
        sha2_notice(sources.amalgamation),
    ];
    for package in ["sqlite-remote-vfs", "sqlite-remote-protocol"] {
        claims.push(format!(
            "{package} {}",
            locked_version(sources.lockfile, package)
        ));
    }
    claims.extend(embedded_components(sources.provenance));
    let files = [
        ("THIRD-PARTY-NOTICES", flat(sources.notices)),
        ("about.hbs", flat(sources.about_hbs)),
    ];
    let mut differences = Vec::new();
    for (file, text) in &files {
        for claim in &claims {
            if !text.contains(claim.as_str()) {
                differences.push(format!("{file} does not say: {claim}"));
            }
        }
    }
    differences
}

/// Every version a Gradle file writes after `prefix`.
fn gradle_versions(gradle_files: &[String], prefix: &str) -> Vec<String> {
    let mut versions = Vec::new();
    for file in gradle_files {
        for (index, _) in file.match_indices(prefix) {
            let rest = &file[index + prefix.len()..];
            versions.extend(rest.split(['"', '@']).next().map(str::to_owned));
        }
    }
    versions
}

/// The notices' bullet for a Maven coordinate, on one line.
fn maven_bullet(notices: &str, coordinate: &str) -> String {
    notices
        .split("\n- ")
        .map(|bullet| bullet.trim_start_matches("- "))
        .find(|bullet| bullet.starts_with(coordinate))
        .map(flat)
        .unwrap_or_default()
}

fn kotlin_differences(notices: &str, gradle_files: &[String]) -> Vec<String> {
    const IN_A_BULLET: [(&str, &str); 5] = [
        ("net.java.dev.jna:jna", "net.java.dev.jna:jna:"),
        (
            "org.jetbrains.kotlinx:kotlinx-coroutines-core",
            "org.jetbrains.kotlinx:kotlinx-coroutines-core:",
        ),
        (
            "org.jetbrains.kotlin:kotlin-stdlib",
            "kotlin(\"jvm\") version \"",
        ),
        (
            "org.jetbrains.kotlin:kotlin-stdlib",
            "kotlin(\"android\") version \"",
        ),
        (
            "org.jetbrains.kotlin:kotlin-stdlib",
            "kotlin(\"multiplatform\") version \"",
        ),
    ];
    const IN_THE_TEXT: [(&str, &str); 3] = [
        ("Gradle plugins", "id(\"dev.gobley.cargo\") version \""),
        ("Gradle plugins", "id(\"dev.gobley.uniffi\") version \""),
        (
            "Gradle plugin",
            "id(\"org.jetbrains.kotlin.plugin.atomicfu\") version \"",
        ),
    ];
    let mut differences = Vec::new();
    for (coordinate, prefix) in IN_A_BULLET {
        let bullet = maven_bullet(notices, coordinate);
        for version in gradle_versions(gradle_files, prefix) {
            if !bullet.contains(&version) {
                differences.push(format!(
                    "{coordinate} {version}: in a Gradle file, not in the notices"
                ));
            }
        }
    }
    let text = flat(notices);
    for (name, prefix) in IN_THE_TEXT {
        for version in gradle_versions(gradle_files, prefix) {
            if !text.contains(&format!("{name} {version}")) {
                differences.push(format!(
                    "{name} {version}: in a Gradle file, not in the notices"
                ));
            }
        }
    }
    differences
}

/// The crate versions the report links: `<a …>name version</a>`.
fn report_crates(html: &str) -> BTreeSet<CrateVersion> {
    let mut crates = BTreeSet::new();
    for anchor in html.split("<a ").skip(1) {
        let text = anchor
            .split_once('>')
            .and_then(|(_, rest)| rest.split("</a>").next())
            .unwrap_or_default();
        let words: Vec<&str> = text.split_whitespace().collect();
        if let [name, version] = words[..]
            && version.starts_with(|c: char| c.is_ascii_digit())
            && version.contains('.')
        {
            crates.insert(crate_version(name, version));
        }
    }
    crates
}

// --- the sources ---------------------------------------------------------

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives in the workspace")
        .to_path_buf()
}

fn workspace_file(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

/// Crate version → license of one `cargo tree` over the locked graph.
fn cargo_tree(edges: &str, packages: &[&str], targets: &[&str]) -> BTreeMap<CrateVersion, String> {
    let mut command = Command::new(env!("CARGO"));
    command.current_dir(workspace_root()).args([
        "tree",
        "--offline",
        "--locked",
        // CI sets CARGO_TERM_COLOR=always, which would color the `(*)` of a repeated crate.
        "--color",
        "never",
        "-e",
        edges,
        "--prefix",
        "none",
        "--format",
        "{p}|{l}",
    ]);
    for package in packages {
        command.args(["-p", package]);
    }
    for target in targets {
        command.args(["--target", target]);
    }
    let output = command.output().expect("run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree failed; every package must be in the local registry (`cargo fetch --locked`): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(tree_line)
        .filter(|((name, _), _)| !name.starts_with("kupferdraht"))
        .collect()
}

fn tree_line(line: &str) -> Option<(CrateVersion, String)> {
    let (package, license) = line.split_once('|')?;
    let mut words = package.split_whitespace();
    let name = words.next()?.to_owned();
    let version = words.next()?.strip_prefix('v')?.to_owned();
    Some((
        (name, version),
        license.trim_end_matches(" (*)").trim().to_owned(),
    ))
}

fn graphs() -> &'static Graphs {
    static GRAPHS: OnceLock<Graphs> = OnceLock::new();
    GRAPHS.get_or_init(|| {
        let shipped = ["kupferdraht-core", "kupferdraht-cli"];
        Graphs {
            all: cargo_tree("normal,build", &shipped, &["all"]),
            // The markers read normal edges: `cargo tree` resolves a build
            // dependency for the host, so a graph with them differs between
            // a Mac and CI. The CLI is no browser artifact: only the library
            // counts there.
            browser: cargo_tree("normal", &["kupferdraht-core"], &["wasm32-unknown-unknown"])
                .into_keys()
                .collect(),
            native: cargo_tree("normal", &shipped, &NATIVE_TARGETS)
                .into_keys()
                .collect(),
        }
    })
}

fn graph_keys() -> BTreeSet<CrateVersion> {
    graphs().all.keys().cloned().collect()
}

fn gradle_files() -> Vec<String> {
    ["android", "kmp", "kotlin"]
        .map(|binding| workspace_file(&format!("bindings/{binding}/build.gradle.kts")))
        .to_vec()
}

fn crate_version(name: &str, version: &str) -> CrateVersion {
    (name.to_owned(), version.to_owned())
}

fn assert_none(differences: &[String]) {
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// A crate section around `body`, as the notices write one.
fn section(heading: &str, body: &str) -> String {
    format!("{heading}\n{}\n\n{body}\n", "-".repeat(heading.len()))
}

const DUAL: &str = "Components dual-licensed Apache-2.0 OR MIT";

fn entry(name: &str, version: &str, section: &str, marker: Marker) -> Entry {
    Entry {
        name: name.to_owned(),
        version: version.to_owned(),
        section: section.to_owned(),
        marker,
    }
}

// --- the parser ----------------------------------------------------------

#[test]
fn an_entry_wrapped_across_lines_is_one_entry() {
    let notices = section(
        DUAL,
        "- uuid 1.23.1 — Ashley Mannix; Dylan DPC;\n  Hunar Roop Kahlon —\n  https://github.com/uuid-rs/uuid",
    );
    assert_eq!(
        notice_entries(&notices),
        [entry("uuid", "1.23.1", DUAL, Marker::EveryTarget)]
    );
}

#[test]
fn one_entry_names_several_crates_and_versions() {
    let notices = section(
        DUAL,
        "- aead 0.5.2, aead 0.6.1, cipher\n  0.4.4 — RustCrypto Developers — https://github.com/RustCrypto/traits",
    );
    let names: Vec<_> = notice_entries(&notices)
        .into_iter()
        .map(|entry| (entry.name, entry.version))
        .collect();
    assert_eq!(
        names,
        [
            crate_version("aead", "0.5.2"),
            crate_version("aead", "0.6.1"),
            crate_version("cipher", "0.4.4")
        ]
    );
}

#[test]
fn a_target_marker_follows_its_version_even_across_a_line_break() {
    let notices = section(
        DUAL,
        "- windows-link 0.2.1 (native targets\n  only), web-time 1.1.0 (browser target only), serde 1.0.229 — someone — https://example.org",
    );
    assert_eq!(
        notice_entries(&notices),
        [
            entry("windows-link", "0.2.1", DUAL, Marker::NativeOnly),
            entry("web-time", "1.1.0", DUAL, Marker::BrowserOnly),
            entry("serde", "1.0.229", DUAL, Marker::EveryTarget),
        ]
    );
}

#[test]
fn build_metadata_is_part_of_the_version() {
    let notices = section(
        DUAL,
        "- wasi 0.11.1+wasi-snapshot-preview1 — The Cranelift Project Developers — https://example.org",
    );
    assert_eq!(
        notice_entries(&notices)[0].version,
        "0.11.1+wasi-snapshot-preview1"
    );
}

#[test]
fn the_tail_after_the_dash_names_no_crate() {
    let notices = section(
        DUAL,
        "- serde 1.0.229 — the Gradle plugins 0.3.7 — https://example.org. Built with protoc 3.2.0.",
    );
    assert_eq!(
        notice_entries(&notices),
        [entry("serde", "1.0.229", DUAL, Marker::EveryTarget)]
    );
}

#[test]
fn only_the_crate_sections_are_read() {
    let notices = [
        section(
            "Components with notices outside the crate graph",
            "- sqlite-remote-vfs 0.6.0 and sqlite-remote-protocol 0.6.0 — Schwarz Digits",
        ),
        section(
            "Kotlin, Android and Kotlin Multiplatform bindings",
            "- net.java.dev.jna:jna 5.15.0 — Timothy Wall",
        ),
        section("Trademarks", "\"Wire\" is a trademark of Wire Swiss GmbH."),
        section(DUAL, "- serde 1.0.229 — someone — https://example.org"),
        section(
            "Regenerating this file",
            "- cargo 1.98.0 is not a component",
        ),
    ]
    .concat();
    assert_eq!(
        notice_entries(&notices),
        [entry("serde", "1.0.229", DUAL, Marker::EveryTarget)]
    );
}

// --- license expressions -------------------------------------------------

#[test]
fn a_license_expression_has_one_spelling() {
    let dual = canonical_license("MIT OR Apache-2.0");
    assert_eq!(canonical_license("Apache-2.0 OR MIT"), dual);
    assert_eq!(canonical_license("MIT/Apache-2.0"), dual);
    assert_eq!(canonical_license("Apache-2.0/MIT"), dual);
    let with_unicode = canonical_license("(MIT OR Apache-2.0) AND Unicode-3.0");
    assert_eq!(
        canonical_license("Unicode-3.0 AND (Apache-2.0 OR MIT)"),
        with_unicode
    );
    assert_ne!(with_unicode, dual, "an added license is another expression");
}

#[test]
fn a_section_heading_names_its_license() {
    let cases = [
        (DUAL, "Apache-2.0 OR MIT"),
        ("Components licensed under the MIT License", "MIT"),
        (
            "Components licensed under the Apache License 2.0",
            "Apache-2.0",
        ),
        (
            "Components licensed under the Unicode License v3",
            "Unicode-3.0",
        ),
        (
            "Components licensed under the Mozilla Public License 2.0",
            "MPL-2.0",
        ),
        (
            "Components licensed under the BSD 3-Clause License",
            "BSD-3-Clause",
        ),
        ("Components licensed under the ISC License", "ISC"),
        (
            "Components licensed under the CDLA Permissive 2.0",
            "CDLA-Permissive-2.0",
        ),
        ("Components licensed under the zlib License", "Zlib"),
        (
            "Components licensed under MIT OR Unlicense",
            "Unlicense OR MIT",
        ),
        (
            "Components licensed under (MIT OR Apache-2.0) AND Unicode-3.0",
            "Unicode-3.0 AND (Apache-2.0 OR MIT)",
        ),
    ];
    for (heading, expression) in cases {
        assert_eq!(
            section_license(heading),
            canonical_license(expression),
            "{heading}"
        );
    }
}

// --- the comparisons, on small inputs ------------------------------------

#[test]
fn a_stale_version_is_named_with_the_side_that_lacks_it() {
    let entries = [entry("tokio", "1.54.0", DUAL, Marker::EveryTarget)];
    let graph = BTreeSet::from([crate_version("tokio", "1.54.1")]);
    let differences = graph_differences(&entries, &graph).join("\n");
    assert!(
        differences.contains("tokio 1.54.0") && differences.contains("not in the graph"),
        "{differences}"
    );
    assert!(
        differences.contains("tokio 1.54.1") && differences.contains("not in the notices"),
        "{differences}"
    );
}

#[test]
fn a_crate_in_the_section_of_another_license_is_named() {
    // unicode-normalization 0.1.26 added Unicode-3.0 to its license: same crate, same section, a new expression.
    let entries = [entry(
        "unicode-normalization",
        "0.1.26",
        DUAL,
        Marker::EveryTarget,
    )];
    let licenses = BTreeMap::from([(
        crate_version("unicode-normalization", "0.1.26"),
        "(MIT OR Apache-2.0) AND Unicode-3.0".to_owned(),
    )]);
    let differences = license_differences(&entries, &licenses).join("\n");
    assert!(
        differences.contains("unicode-normalization 0.1.26"),
        "{differences}"
    );
}

#[test]
fn a_section_with_a_choice_has_to_say_which_license_is_used() {
    let silent = section(
        "Components licensed under Apache-2.0 OR ISC OR MIT",
        "- rustls 0.23.46 — someone",
    );
    assert_eq!(sections_with_an_unstated_choice(&silent).len(), 1);
    let stating = section(
        "Components licensed under Apache-2.0 OR ISC OR MIT",
        "Dual- or multi-licensed; used here under the Apache License 2.0.\n\n- rustls 0.23.46 — someone",
    );
    assert!(sections_with_an_unstated_choice(&stating).is_empty());
    let no_choice = section(
        "Components licensed under the MIT License",
        "- mio 1.2.4 — someone",
    );
    assert!(sections_with_an_unstated_choice(&no_choice).is_empty());
}

#[test]
fn a_wrong_target_marker_is_named() {
    // chacha20: in the browser graph through hpke-rs-rust-crypto, and marked native only.
    let both = BTreeSet::from([crate_version("chacha20", "0.10.2")]);
    let marked_native = [entry("chacha20", "0.10.2", DUAL, Marker::NativeOnly)];
    assert_eq!(marker_differences(&marked_native, &both, &both).len(), 1);
    let unmarked = [entry("chacha20", "0.10.2", DUAL, Marker::EveryTarget)];
    assert!(marker_differences(&unmarked, &both, &both).is_empty());

    let nowhere = BTreeSet::new();
    let only_browser = [entry("js-sys", "0.3.105", DUAL, Marker::BrowserOnly)];
    let browser = BTreeSet::from([crate_version("js-sys", "0.3.105")]);
    assert!(marker_differences(&only_browser, &browser, &nowhere).is_empty());
    assert_eq!(
        marker_differences(&only_browser, &nowhere, &browser).len(),
        1
    );

    // In no normal graph, a build tool or a `cfg(any())` dependency: in no
    // artifact, so no graph contradicts its marker.
    for marker in [Marker::NativeOnly, Marker::EveryTarget, Marker::BrowserOnly] {
        let build_tool = [entry("cc", "1.11.1", DUAL, marker)];
        assert!(marker_differences(&build_tool, &nowhere, &nowhere).is_empty());
    }
}

fn outside_graph_sources() -> [String; 5] {
    [
        "- SQLite3 Multiple Ciphers 2.5.1 — embeds SQLite 3.53.4 — SHA-2, AEGIS. Copyright (C) Olivier Gay\nall rights\n- sqlite-remote-vfs 0.6.0 and sqlite-remote-protocol 0.6.0 — Schwarz Digits".to_owned(),
        "<h3>SQLite3 Multiple Ciphers 2.5.1 (SQLite 3.53.4)</h3> SHA-2, AEGIS. Copyright (C) Olivier Gay\nall rights <h3>sqlite-remote-vfs 0.6.0 and sqlite-remote-protocol 0.6.0</h3>".to_owned(),
        "sqlite3mc-version: 2.5.1\nsqlite-version: 3.53.4\nembedded-licenses: BSD-3-Clause (SHA-2); MIT (AEGIS)\n".to_owned(),
        "[[package]]\nname = \"sqlite-remote-vfs\"\nversion = \"0.6.0\"\n\n[[package]]\nname = \"sqlite-remote-protocol\"\nversion = \"0.6.0\"\n".to_owned(),
        " * Copyright (C) Olivier Gay\n * all rights\n */\n".to_owned(),
    ]
}

fn differences_of(sources: &[String; 5]) -> Vec<String> {
    outside_graph_differences(&Sources {
        notices: &sources[0],
        about_hbs: &sources[1],
        provenance: &sources[2],
        lockfile: &sources[3],
        amalgamation: &sources[4],
    })
}

#[test]
fn the_components_outside_the_graph_are_checked_in_both_files() {
    assert_none(&differences_of(&outside_graph_sources()));
    for index in [0, 1] {
        let mut stale_remote = outside_graph_sources();
        stale_remote[index] =
            stale_remote[index].replace("sqlite-remote-vfs 0.6.0", "sqlite-remote-vfs 0.5.0");
        assert!(
            !differences_of(&stale_remote).is_empty(),
            "stale sqlite-remote-vfs in file {index}"
        );

        let mut stale_engine = outside_graph_sources();
        stale_engine[index] = stale_engine[index].replace("2.5.1", "2.5.0");
        assert!(
            !differences_of(&stale_engine).is_empty(),
            "stale SQLite3MC in file {index}"
        );

        let mut unnamed = outside_graph_sources();
        unnamed[index] = unnamed[index].replace("AEGIS", "");
        assert!(
            !differences_of(&unnamed).is_empty(),
            "embedded component unnamed in file {index}"
        );

        let mut no_notice = outside_graph_sources();
        no_notice[index] = no_notice[index].replace("Olivier Gay", "someone");
        assert!(
            !differences_of(&no_notice).is_empty(),
            "SHA-2's notice missing in file {index}"
        );
    }
}

#[test]
fn a_changed_gradle_version_is_named() {
    let notices = "- net.java.dev.jna:jna 5.15.0 — x\n- org.jetbrains.kotlinx:kotlinx-coroutines-core 1.8.1 (JVM, Android) and\n  1.10.2 (Multiplatform) — y\n- org.jetbrains.kotlin:kotlin-stdlib 2.0.21 (JVM, Android) and 2.3.10\n  (Multiplatform) — z\nthe gobley cargo and uniffi\nGradle plugins 0.3.7\n";
    let gradle = [
        "api(\"net.java.dev.jna:jna:5.15.0\")\nimplementation(\"org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1\")\nkotlin(\"jvm\") version \"2.0.21\"".to_owned(),
        "api(\"org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2\")\nkotlin(\"multiplatform\") version \"2.3.10\"\nid(\"dev.gobley.cargo\") version \"0.3.7\"".to_owned(),
    ];
    assert_none(&kotlin_differences(notices, &gradle));
    let mut moved = gradle.clone();
    moved[0] = moved[0].replace(
        "kotlinx-coroutines-core:1.8.1",
        "kotlinx-coroutines-core:1.9.0",
    );
    assert!(!kotlin_differences(notices, &moved).is_empty());
    let mut plugin = gradle;
    plugin[1] = plugin[1].replace("0.3.7", "0.3.8");
    assert!(!kotlin_differences(notices, &plugin).is_empty());
}

#[test]
fn the_report_lists_crates_as_cargo_about_writes_them() {
    let html = "<li><a href=\" https://github.com/uuid-rs/uuid \">uuid 1.29.0</a></li>\n<li><a href=\" https://example.org \">wasi 0.11.1+wasi-snapshot-preview1</a></li>\n<h3>sqlite-remote-vfs 0.6.0 and sqlite-remote-protocol 0.6.0</h3>";
    assert_eq!(
        report_crates(html),
        BTreeSet::from([
            crate_version("uuid", "1.29.0"),
            crate_version("wasi", "0.11.1+wasi-snapshot-preview1")
        ])
    );
}

// --- the files themselves ------------------------------------------------

#[test]
fn the_notices_list_the_build_graph_both_ways() {
    let entries = notice_entries(&workspace_file("THIRD-PARTY-NOTICES"));
    assert_none(&graph_differences(&entries, &graph_keys()));
}

#[test]
fn each_crate_stands_in_the_section_of_its_license() {
    let entries = notice_entries(&workspace_file("THIRD-PARTY-NOTICES"));
    assert_none(&license_differences(&entries, &graphs().all));
}

#[test]
fn each_section_with_a_choice_says_which_license_is_used() {
    assert_none(&sections_with_an_unstated_choice(&workspace_file(
        "THIRD-PARTY-NOTICES",
    )));
}

#[test]
fn the_target_markers_match_the_graphs_per_target() {
    let entries = notice_entries(&workspace_file("THIRD-PARTY-NOTICES"));
    assert_none(&marker_differences(
        &entries,
        &graphs().browser,
        &graphs().native,
    ));
}

#[test]
fn the_components_outside_the_graph_match_their_sources() {
    assert_none(&outside_graph_differences(&Sources {
        notices: &workspace_file("THIRD-PARTY-NOTICES"),
        about_hbs: &workspace_file("about.hbs"),
        provenance: &workspace_file("vendor/sqlite3mc/PROVENANCE.txt"),
        lockfile: &workspace_file("Cargo.lock"),
        amalgamation: &workspace_file("vendor/sqlite3mc/sqlite3mc_amalgamation.c"),
    }));
}

#[test]
fn the_kotlin_section_matches_the_gradle_files() {
    assert_none(&kotlin_differences(
        &workspace_file("THIRD-PARTY-NOTICES"),
        &gradle_files(),
    ));
}

#[test]
fn the_html_report_covers_the_notices_graph() {
    let report = report_crates(&workspace_file("docs/THIRD_PARTY_LICENSES.html"));
    let graph: BTreeSet<_> = graph_keys()
        .into_iter()
        .filter(|(name, _)| !NOT_IN_THE_REPORT.contains(&name.as_str()))
        .collect();
    let extra: Vec<_> = report.difference(&graph).collect();
    let missing: Vec<_> = graph.difference(&report).collect();
    assert!(
        extra.is_empty(),
        "in the report, not in the graph: {extra:?}"
    );
    assert!(
        missing.is_empty(),
        "in the graph, not in the report: {missing:?}"
    );
}

#[test]
fn the_notices_header_claims_no_date() {
    let notices = workspace_file("THIRD-PARTY-NOTICES");
    let header = notices
        .split("Components with notices")
        .next()
        .expect("a header");
    assert!(
        !header.contains("2026-"),
        "the test holds the list current, not a date: {header}"
    );
}

#[test]
fn ci_and_the_readme_regenerate_the_report_the_same_way() {
    let command = "cargo about generate about.hbs -m kupferdraht-cli/Cargo.toml -c about.toml";
    let install = "cargo install --locked cargo-about --version 0.9.2 --features cli";
    let ci = workspace_file(".github/workflows/licenses.yml");
    assert!(
        ci.contains(install),
        "without the feature it installs no binary: {install}"
    );
    assert!(
        workspace_file("README.md").contains(install),
        "README installs the same: {install}"
    );
    assert!(
        ci.contains(command),
        "the CI job regenerates with: {command}"
    );
    assert!(
        ci.contains("git diff --exit-code docs/THIRD_PARTY_LICENSES.html"),
        "and fails on a diff"
    );
    assert!(
        workspace_file("README.md").contains(command),
        "README names the same command"
    );
}
