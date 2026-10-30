// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Backend selection for `login` / `register` / `activate` / `sso`.
//!
//! A Wire deployment is addressed by its explicit `--api-url` (any
//! self-hosted Wire-compatible instance — backend-spec §20 assumes the
//! base URL is the only thing a client changes). There is no default
//! and no named preset: nothing in the CLI points at a hosted service
//! unless the operator names it.

use kupferdraht::{KupferdrahtError, ServerConfig};

/// Resolve the server coordinates for a session-creating command from
/// an explicit API base URL.
///
/// `ws_url`, when omitted, is derived from the API URL by Wire
/// deployment convention (`https://nginz-https.<domain>` →
/// `wss://nginz-ssl.<domain>`, otherwise the same host with `wss://`).
///
/// # Errors
/// `KupferdrahtError::InvalidInput` when the API URL is not a bare
/// `https://` origin or the WebSocket URL is not `wss://`.
pub fn resolve_server(
    api_url: &str,
    ws_url: Option<&str>,
) -> Result<ServerConfig, KupferdrahtError> {
    let api = api_url.trim_end_matches('/');
    let host = api.strip_prefix("https://").ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "--api-url must start with https:// (got {api_url})"
        ))
    })?;
    if host.is_empty() || host.contains('/') {
        return Err(KupferdrahtError::InvalidInput(format!(
            "--api-url must be a bare https origin without a path (got {api_url})"
        )));
    }
    let websocket_url = match ws_url {
        Some(ws) => {
            if !ws.starts_with("wss://") {
                return Err(KupferdrahtError::InvalidInput(format!(
                    "--ws-url must start with wss:// (got {ws})"
                )));
            }
            ws.to_string()
        }
        None => derive_websocket_url(host),
    };
    Ok(ServerConfig {
        api_base_url: api.to_string(),
        websocket_url,
        title: host.to_string(),
    })
}

/// Wire deployments expose the API on `nginz-https.<domain>` and
/// the WebSocket on `nginz-ssl.<domain>`; hosts outside that naming
/// convention keep their own host.
fn derive_websocket_url(api_host: &str) -> String {
    match api_host.strip_prefix("nginz-https.") {
        Some(domain) => format!("wss://nginz-ssl.{domain}"),
        None => format!("wss://{api_host}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_becomes_server_config() {
        let server = resolve_server("https://nginz-https.chat.example.org", None).unwrap();
        assert_eq!(server.api_base_url, "https://nginz-https.chat.example.org");
    }

    #[test]
    fn api_url_derives_websocket_by_nginz_convention() {
        let server = resolve_server("https://nginz-https.chat.example.org", None).unwrap();
        assert_eq!(server.websocket_url, "wss://nginz-ssl.chat.example.org");
    }

    #[test]
    fn api_url_without_nginz_prefix_keeps_host_for_websocket() {
        let server = resolve_server("https://api.example.org", None).unwrap();
        assert_eq!(server.websocket_url, "wss://api.example.org");
    }

    #[test]
    fn title_is_the_api_host() {
        let server = resolve_server("https://nginz-https.chat.example.org", None).unwrap();
        assert_eq!(server.title, "nginz-https.chat.example.org");
    }

    #[test]
    fn explicit_ws_url_wins_over_derivation() {
        let server = resolve_server(
            "https://nginz-https.chat.example.org",
            Some("wss://push.chat.example.org"),
        )
        .unwrap();
        assert_eq!(server.websocket_url, "wss://push.chat.example.org");
    }

    #[test]
    fn trailing_slash_on_api_url_is_trimmed() {
        let server = resolve_server("https://nginz-https.chat.example.org/", None).unwrap();
        assert_eq!(server.api_base_url, "https://nginz-https.chat.example.org");
    }

    #[test]
    fn non_https_api_url_is_rejected() {
        let err = resolve_server("http://nginz-https.chat.example.org", None).unwrap_err();
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }

    #[test]
    fn api_url_with_a_path_is_rejected() {
        let err = resolve_server("https://nginz-https.chat.example.org/v14", None).unwrap_err();
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }

    #[test]
    fn non_wss_ws_url_is_rejected() {
        let err = resolve_server(
            "https://nginz-https.chat.example.org",
            Some("ws://push.chat.example.org"),
        )
        .unwrap_err();
        assert!(matches!(err, KupferdrahtError::InvalidInput(_)));
    }
}
