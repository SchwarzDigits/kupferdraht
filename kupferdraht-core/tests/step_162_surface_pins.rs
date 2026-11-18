// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 162 — the names the three surfaces had are gone from every layer,
//! and the matrix-rust-sdk-shaped ones reach the FFI, the browser, the tab
//! mirror and the CLI.
//!
//! Source and doc pins, in the idiom of `step_125_client_handle.rs`; the
//! behaviour is pinned in `step_162_api_without_natrium_shapes.rs`.

use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn workspace_file(rel: &str) -> String {
    let path = workspace().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every file under `rel` (recursively) whose extension is in `extensions`.
fn files_under(rel: &str, extensions: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![workspace().join(rel)];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e))
            {
                found.push(path);
            }
        }
    }
    found
}

/// The names the three surfaces had (Steps 17, 21, 22), in every spelling a
/// layer gave them.
const GONE: &[&str] = &[
    "initiate_sso",
    "initiateSso",
    "finalize_sso",
    "finalizeSso",
    "send_typing_started",
    "send_typing_stopped",
    "sendTypingStarted",
    "sendTypingStopped",
    "startTyping",
    "stopTyping",
    "update_email",
    "updateEmail",
    "email-update",
    "EmailUpdate",
    "sso initiate",
    "sso finalize",
];

#[test]
fn the_old_names_are_gone_from_code_and_its_docs() {
    let mut files = files_under("kupferdraht-core/src", &["rs"]);
    files.extend(files_under("kupferdraht-cli/src", &["rs"]));
    files.extend(files_under(
        "bindings/web/kupferdraht-tabs",
        &["js", "ts", "md"],
    ));
    files.extend(files_under("kupferdraht-core/tests/web", &["js"]));
    for doc in ["docs/library.md", "docs/bindings.md", "docs/cli.md"] {
        files.push(workspace().join(doc));
    }
    let mut hits = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("readable");
        for name in GONE {
            if text.contains(name) {
                hits.push(format!("{}: {name}", file.display()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "old names still present:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_ffi_mirrors_the_new_names() {
    let manager = workspace_file("kupferdraht-core/src/ffi/manager.rs");
    for name in [
        "pub async fn sso_login_url(",
        "pub async fn login_with_sso_callback(",
    ] {
        assert!(manager.contains(name), "ffi/manager.rs lacks `{name}`");
    }
    assert!(
        workspace_file("kupferdraht-core/src/ffi/room.rs")
            .contains("pub async fn typing_notice(&self, typing: bool)"),
        "ffi/room.rs lacks `typing_notice`"
    );
    assert!(
        workspace_file("kupferdraht-core/src/ffi/admin.rs")
            .contains("pub async fn request_email_change("),
        "ffi/admin.rs lacks `request_email_change`"
    );
}

#[test]
fn the_browser_exports_the_new_names() {
    let wasm = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for js_name in [
        "js_name = ssoLoginUrl",
        "js_name = loginWithSsoCallback",
        "js_name = typingNotice",
        "js_name = requestEmailChange",
    ] {
        assert!(wasm.contains(js_name), "wasm/manager.rs lacks `{js_name}`");
    }
}

#[test]
fn the_tab_mirror_carries_the_new_names() {
    let tabs = |name: &str| workspace_file(&format!("bindings/web/kupferdraht-tabs/{name}"));
    let protocol = tabs("protocol.js");
    for row in [
        r#"["Kupferdraht", "ssoLoginUrl", "idempotent", null]"#,
        r#"["Kupferdraht", "loginWithSsoCallback", "mutation", "PendingLogin"]"#,
        r#"["Client", "typingNotice", "idempotent", null]"#,
        r#"["Client", "requestEmailChange", "mutation", null]"#,
    ] {
        assert!(protocol.contains(row), "protocol.js lacks the row {row}");
    }
    let proxy = tabs("proxy.js");
    let types = tabs("index.d.ts");
    for method in [
        "ssoLoginUrl(",
        "loginWithSsoCallback(",
        "typingNotice(",
        "requestEmailChange(",
    ] {
        assert!(proxy.contains(method), "proxy.js lacks `{method}`");
        assert!(types.contains(method), "index.d.ts lacks `{method}`");
    }
}

/// The CLI is the API's first consumer (ADR-0007): a command per surface,
/// and `docs/cli.md` documents each. The callback carries the session, so
/// it is read from stdin or a prompt, never from an argument (Step 137).
#[test]
fn the_cli_documents_a_command_per_surface() {
    let doc = workspace_file("docs/cli.md");
    for heading in [
        "### `sso url`",
        "### `sso callback`",
        "### `typing`",
        "### `email-change`",
    ] {
        assert!(doc.contains(heading), "docs/cli.md lacks {heading}");
    }
    let callback = doc
        .split("### `sso callback`")
        .nth(1)
        .and_then(|rest| rest.split("\n### ").next())
        .expect("the sso callback section");
    assert!(
        callback.contains("--callback-stdin"),
        "sso callback documents --callback-stdin"
    );
}

#[test]
fn the_api_mapping_names_the_matrix_counterparts() {
    let mapping = workspace_file("docs/api-mapping-matrix-rust-sdk.md");
    for name in [
        "get_sso_login_url",
        "login_with_sso_callback",
        "typing_notice",
        "request_email_change",
    ] {
        assert!(
            mapping.contains(name),
            "docs/api-mapping-matrix-rust-sdk.md lacks {name}"
        );
    }
}
