// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 122 — "a message left this client": the signal the browser's sync
//! loop waits for beside its doorbell. Since Step 118 a pass follows a
//! frame, and the backend sends the sender no frame for its own message;
//! without the signal the embedder hears nothing (no `onSync`) until the
//! next pass. Every successful `POST /mls/messages` notes one; the loop
//! drains on it and absorbs the ones a pass caused itself (its delivery
//! receipts). Native is unchanged: its `Timeline` reports the own message.

/// A coalescing flag a sender sets and the sync loop consumes: any number
/// of notes before the loop looks count once.
#[derive(Default)]
pub(crate) struct SentSignal(tokio::sync::Notify);

impl SentSignal {
    /// Note that a message upload succeeded.
    pub(crate) fn note(&self) {
        // A stored permit, at most one: notes coalesce.
        self.0.notify_one();
    }

    /// Consume a pending note: whether a message was sent since the last
    /// time anyone looked.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn take(&self) -> bool {
        use futures_util::FutureExt as _;
        self.0.notified().now_or_never().is_some()
    }

    /// Wait until a message was sent, consuming the note.
    #[cfg(target_arch = "wasm32")]
    pub(crate) async fn wait(&self) {
        self.0.notified().await;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::Arc;

    use httpmock::Method::POST;
    use httpmock::{Mock, MockServer};
    use tempfile::TempDir;

    use crate::client::{Client, ClientBuilder, ServerConfig};
    use crate::keystore::{InMemoryKeyStore, SecureKeyStore};
    use crate::manager::login_mocks::{mount_device_registration, mount_login};

    fn mount_upload(server: &MockServer, status: u16) -> Mock<'_> {
        server.mock(|when, then| {
            when.method(POST).path("/v14/mls/messages");
            then.status(status)
                .header("content-type", "application/json")
                .body(r#"{"events":[],"time":"2026-10-05T10:00:00.000Z"}"#);
        })
    }

    async fn client(server: &MockServer, home: &TempDir) -> Arc<Client> {
        mount_login(server);
        mount_device_registration(server);
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        ClientBuilder::new()
            .server(ServerConfig {
                api_base_url: server.base_url(),
                websocket_url: server.base_url(),
                title: "mock".into(),
            })
            .permit_http()
            .base_dir(home.path().to_path_buf())
            .keystore(keystore)
            .login("alice@example.test".into(), "pw".into(), None)
            .await
            .expect("login")
            .register_device()
            .await
            .expect("register device")
    }

    #[tokio::test]
    async fn a_successful_upload_notes_one_sent_message() {
        let server = MockServer::start();
        let home = TempDir::new().expect("tempdir");
        let client = client(&server, &home).await;
        mount_upload(&server, 201);

        client.http_send(14, b"one").await.expect("upload");
        client.http_send(14, b"two").await.expect("upload");

        assert!(client.sent_signal().take(), "the uploads were noted");
        assert!(
            !client.sent_signal().take(),
            "notes coalesce: the loop drains once for both"
        );
    }

    #[tokio::test]
    async fn a_failed_upload_notes_nothing() {
        let server = MockServer::start();
        let home = TempDir::new().expect("tempdir");
        let client = client(&server, &home).await;
        mount_upload(&server, 500);

        assert!(client.http_send(14, b"lost").await.is_err());

        assert!(!client.sent_signal().take());
    }
}
