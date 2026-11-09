// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 130c — the native pins for a browser view's missing value (TDD red).
//!
//! `wasm/` never compiles natively, so the sweep is pinned by reading the
//! sources: every view crosses through one serializer, and every `Option`
//! field of a view is `| null` in the hand-written types. What the serializer
//! writes is pinned in the browser (`step_130c_browser_null_views`).

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `(file name, source)` of every module of `wasm/`, sorted.
fn wasm_sources() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/wasm");
    let mut sources: Vec<(String, String)> = std::fs::read_dir(&dir)
        .expect("read src/wasm")
        .map(|entry| {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().expect("file name").to_string_lossy();
            (
                name.into_owned(),
                std::fs::read_to_string(&path).expect("read module"),
            )
        })
        .collect();
    sources.sort();
    sources
}

/// Where `index.d.ts` declares a view with an `Option` field. A new view of
/// that kind has to name its declaration here.
const VIEW_DECLARATIONS: [(&str, &str); 12] = [
    ("AccountView", "AccountEntry"),
    ("AssetMeta", "AssetMeta"),
    ("DeviceInfoView", "DeviceInfo"),
    ("DeviceView", "RoomDevice"),
    ("FeatureConfigView", "FeatureConfig"),
    ("LocationView", "TimelineItem"),
    ("MemberView", "Member"),
    ("MessageView", "MessageEvent_"),
    ("ReadStateChange", "ReadStateChange"),
    ("ResendSummaryView", "ResendSummary"),
    ("RoomView", "Room"),
    ("TimelineItemView", "TimelineItem"),
];

/// The struct a line opens, by name.
fn opened_struct(line: &str) -> Option<&str> {
    let (visibility, rest) = line.split_once("struct ")?;
    if !(visibility.is_empty() || visibility.starts_with("pub")) || !line.ends_with('{') {
        return None;
    }
    rest.split(['<', ' ']).next()
}

/// `(view, field)` of every `Option` field a `Serialize` struct in `src`
/// sends to the page; a field the view skips when missing never crosses as
/// `null`, so it is left out.
fn optional_view_fields(src: &str) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let (mut derives_serialize, mut skipped) = (false, false);
    let mut view: Option<&str> = None;
    for line in src.lines().map(str::trim) {
        if line.starts_with("#[derive(") {
            derives_serialize = line.contains("Serialize");
        } else if let Some(name) = opened_struct(line) {
            view = derives_serialize.then_some(name);
            derives_serialize = false;
        } else if line == "}" {
            view = None;
        } else if line.contains("skip_serializing_if") {
            skipped = true;
        } else if let (Some(name), Some((field, ty))) = (view, line.split_once(':')) {
            if ty.trim_start().starts_with("Option<") && !skipped {
                let field = field.trim().trim_start_matches("pub ");
                fields.push((name.to_owned(), field.to_owned()));
            }
            skipped = false;
        }
    }
    fields
}

fn camel_case(field: &str) -> String {
    let mut parts = field.split('_');
    let mut camel = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            camel.extend(first.to_uppercase());
            camel.push_str(chars.as_str());
        }
    }
    camel
}

/// The braces of `export interface <name>`.
fn interface_body<'a>(types: &'a str, name: &str) -> &'a str {
    let start = types
        .find(&format!("export interface {name} "))
        .unwrap_or_else(|| panic!("index.d.ts declares {name}"));
    let body = &types[start..];
    let open = body.find('{').expect("the interface opens");
    let mut depth = 0;
    for (at, c) in body[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return &body[open..=open + at];
        }
    }
    panic!("{name} closes");
}

/// The type written after `field: ` in a declaration, up to its `;`.
fn declared_type<'a>(body: &'a str, field: &str) -> Option<&'a str> {
    let needle = format!("{field}: ");
    let at = body.match_indices(&needle).map(|(at, _)| at).find(|&at| {
        !body[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    })?;
    let rest = &body[at + needle.len()..];
    let mut depth = 0;
    for (end, c) in rest.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(&rest[..end]),
            '}' => depth -= 1,
            ';' if depth == 0 => return Some(&rest[..end]),
            _ => {}
        }
    }
    Some(rest)
}

/// What disagrees between the views of `src` and their declarations in
/// `types`: an optional field not declared `| null`, a view without a row.
fn undeclared_fields(src: &str, types: &str) -> Vec<String> {
    let mut wrong = Vec::new();
    for (view, field) in optional_view_fields(src) {
        let Some((_, declaration)) = VIEW_DECLARATIONS.iter().find(|(v, _)| *v == view) else {
            wrong.push(format!("{view} has no row in VIEW_DECLARATIONS"));
            continue;
        };
        let camel = camel_case(&field);
        match declared_type(interface_body(types, declaration), &camel) {
            Some(ty) if ty.trim_end().ends_with("| null") => {}
            other => wrong.push(format!("{view}.{camel} in {declaration}: {other:?}")),
        }
    }
    wrong
}

#[test]
fn a_wrong_type_and_a_view_without_a_row_are_named() {
    let types =
        "export interface Member { displayName: string | null; handle: string; role: string }";
    let src = r#"
#[derive(serde::Serialize)]
struct MemberView {
    display_name: Option<String>,
    handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(serde::Deserialize)]
struct OptionsFromThePage {
    limit: Option<u32>,
}

#[derive(serde::Serialize)]
struct NewView {
    field: Option<String>,
}
"#;

    assert_eq!(
        undeclared_fields(src, types),
        [
            "MemberView.handle in Member: Some(\"string\")",
            "NewView has no row in VIEW_DECLARATIONS"
        ]
    );
}

#[test]
fn no_view_reaches_the_page_past_the_one_serializer() {
    let offenders: Vec<String> = wasm_sources()
        .into_iter()
        .filter(|(name, _)| name != "js_view.rs")
        .filter_map(|(name, src)| {
            let hits = src.matches("serde_wasm_bindgen::to_value").count()
                + src.matches("serde_wasm_bindgen::Serializer").count();
            (hits > 0).then(|| format!("{name}: {hits}"))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "serialized past js_view::to_js: {offenders:?}"
    );
}

#[test]
fn the_one_serializer_writes_a_missing_value_as_null() {
    let src = workspace_file("kupferdraht-core/src/wasm/js_view.rs");

    assert!(
        src.contains("serialize_missing_as_null(true)"),
        "js_view::to_js writes None as null"
    );
}

#[test]
fn every_optional_view_field_is_declared_nullable() {
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    let wrong: Vec<String> = wasm_sources()
        .iter()
        .flat_map(|(_, src)| undeclared_fields(src, &types))
        .collect();

    assert!(
        wrong.is_empty(),
        "a view's optional field and its type disagree: {wrong:#?}"
    );
}

#[test]
fn storage_engine_is_declared_as_it_crosses() {
    let types = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");

    assert!(
        types.contains("storageEngine(): Promise<StorageEngineInfo | null>;"),
        "a missing engine crosses as null, like every missing view"
    );
}

#[test]
fn the_breaking_change_is_named_for_the_embedder() {
    let changelog = workspace_file("CHANGELOG.md");
    let section = changelog
        .split("\n## [")
        .find(|section| section.contains("a missing value in a browser view"))
        .expect("CHANGELOG names the change");
    assert!(
        section.contains("BREAKING: a missing value in a browser view"),
        "the entry is marked BREAKING"
    );

    let bindings = workspace_file("docs/bindings.md");
    assert!(
        bindings.contains("A missing value in a view crosses as `null`"),
        "docs/bindings.md states the rule"
    );
}

#[test]
fn the_browser_test_runs_in_the_suite_and_under_clippy() {
    // A whole word: a name run together with its neighbour runs neither.
    for rel in ["scripts/wasm-test.sh", ".github/workflows/ci.yml"] {
        assert!(
            workspace_file(rel)
                .split_whitespace()
                .any(|word| word == "step_130c_browser_null_views"),
            "{rel} names the browser test"
        );
    }
}
